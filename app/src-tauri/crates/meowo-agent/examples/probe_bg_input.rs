//! 探针：往一个**活着的**后台 worker 的 PTY 旁路写按键，看服务端认不认。
//!
//! 用法：`cargo run -p meowo-agent --example probe_bg_input -- <session-id> [按键]`
//!
//! 二进制里挖出来的服务端逻辑是这样的（`claude` 2.1.220）：
//!
//! ```js
//! if (帧.kind === 数据) {
//!   if (期望令牌 && !已认证集合.has(本连接)) { 回 {t:"auth-required"}; return }
//!   if (!已退出 && !_) { pty.write(帧.payload) }
//! }
//! ```
//!
//! 所以「按键石沉大海」只可能卡在两处：没通过认证（会收到 `auth-required`），或那个 `_`
//! 闸门。探针把控制帧原样打出来，用来把这两种可能分开。

use meowo_agent::RuntimeCap;
use std::io::{Read, Write};

const KIND_DATA: u8 = 0;
const KIND_CTRL: u8 = 1;

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let session_id = args.next().expect("用法: probe_bg_input <session-id> [按键]");
    let keys = args.next().unwrap_or_else(|| "zzz".into());

    let endpoint = meowo_agent::plugins::claude::fleet::CLAUDE_RUNTIME
        .background_endpoint(&session_id)
        .expect("花名册里没有这个会话（worker 已退出？）");
    println!("sock={} pid={:?}", endpoint.sock, endpoint.pid);

    // ATTACH=1：先在**控制管道**上发 attach，再往 PTY 写按键。
    // 猜想：后台 worker 默认不进交互态，得先被正式 attach 才会消费 stdin——这正是
    // 控制命令表里有 attach（带 cols/rows/caps）而 PTY 通道上没有的原因。
    if std::env::var_os("ATTACH").is_some() {
        let control = endpoint.control.as_ref().expect("没有控制通道");
        let request = format!(
            r#"{{"proto":1,"op":"attach","short":"{}","auth":"{}","cols":100,"rows":34,"caps":{{"terminal":"meowo","mux":null,"ssh":false}}}}"#,
            control.job, control.auth
        );
        let mut pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&control.sock)
            .expect("连不上控制管道");
        pipe.write_all(format!("{request}\n").as_bytes()).unwrap();
        let mut reply = [0u8; 4096];
        let n = pipe.read(&mut reply).unwrap_or(0);
        println!("ATTACH 回应: {}", String::from_utf8_lossy(&reply[..n]).trim());
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&endpoint.sock)
        .expect("连不上 PTY 管道");
    let mut reader = file.try_clone().expect("复制句柄失败");
    let mut writer = file;

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let screen_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let shared = screen_buf.clone();
    let mut echo = writer.try_clone().expect("复制句柄失败");
    // NOREAD=1：完全不起读线程。用来分辨「按键被服务端拒了」与「本地读写抢同一个句柄
    // 把写卡住了」——Windows 上同步句柄的阻塞 ReadFile 会挡住并发 WriteFile。
    let no_read = std::env::var_os("NOREAD").is_some();
    let auth_token = endpoint.auth.clone();
    let skip_auth = std::env::var_os("NOAUTH").is_some();
    // INLINE=1：认证与按键都在读线程里发（见下），DUMP 分支随后打印画面看按键有没有落进去。
    let inline_keys = std::env::var_os("INLINE").map(|_| keys.clone());
    std::thread::spawn(move || {
        if no_read {
            return;
        }
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut screen = Vec::new();
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            while buf.len() >= 5 {
                let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if buf.len() < 5 + len {
                    break;
                }
                let kind = buf[4];
                let payload: Vec<u8> = buf[5..5 + len].to_vec();
                buf.drain(..5 + len);
                if kind == KIND_CTRL {
                    let text = String::from_utf8_lossy(&payload).to_string();
                    println!("CTRL {text}");
                    if text.contains("\"ping\"") {
                        let _ = echo.write_all(&frame(KIND_CTRL, br#"{"t":"pong"}"#));
                    }
                    if text.contains("\"live\"") {
                        // 认证与按键都在**读线程内部**发：此刻线程正在处理帧、没卡在
                        // read() 上，写不会和自己的阻塞读抢同一个句柄。
                        // NOAUTH=1：跳过认证直接发数据帧。服务端该回 auth-required——
                        // 收得到就证明我的帧确实被当成数据帧读了（认证那道闸门是真的），
                        // 收不到就说明帧格式或类型字节本身没被识别。
                        if let Some(token) = auth_token.as_deref().filter(|_| !skip_auth) {
                            let auth = format!(r#"{{"t":"auth","token":"{token}"}}"#);
                            match echo.write_all(&frame(KIND_CTRL, auth.as_bytes())) {
                                Ok(()) => println!("认证已发出"),
                                Err(e) => println!("认证写失败: {e}"),
                            }
                        }
                        if let Some(keys) = inline_keys.as_deref() {
                            match echo.write_all(&frame(KIND_DATA, keys.as_bytes())) {
                                Ok(()) => println!("按键 {keys:?} 已发出（读线程内）"),
                                Err(e) => println!("按键写失败: {e}"),
                            }
                        }
                        let _ = tx.send("live".into());
                    }
                } else {
                    screen.extend_from_slice(&payload);
                    *shared.lock().unwrap() = screen.clone();
                    let _ = tx.send(format!("screen:{}", screen.len()));
                }
            }
        }
    });

    // 等 live，再认证。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(msg) if msg == "live" => break,
            _ => {}
        }
    }
    if std::env::var_os("DUMP").is_some() {
        std::thread::sleep(std::time::Duration::from_millis(1800));
        let raw = screen_buf.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&raw);
        let visible: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect();
        let tail: Vec<char> = visible.chars().rev().take(400).collect();
        let tail: String = tail.into_iter().rev().collect();
        println!("--- 画面尾部 ---\n{tail}\n---");
        println!("画面里出现 zzz: {}", visible.contains("zzz"));
        std::process::exit(0);
    }
    if let Some(token) = endpoint.auth.as_deref() {
        let auth = format!(r#"{{"t":"auth","token":"{token}"}}"#);
        writer.write_all(&frame(KIND_CTRL, auth.as_bytes())).unwrap();
        println!("已发认证（token 前 8 位 {}）", &token[..8.min(token.len())]);
    } else {
        println!("这个 worker 没设 ptyAuth，服务端对输入 fail-open");
    }
    std::thread::sleep(std::time::Duration::from_millis(400));

    // DUMP=1：只连上看一屏，不发任何按键。用来事后核对上一轮按键有没有真的进到输入框。
    println!("--- 发按键 {keys:?} ---");
    writer.write_all(&frame(KIND_DATA, keys.as_bytes())).unwrap();
    writer.flush().unwrap();
    println!("写入返回了（没阻塞）");

    // 收 2 秒回音：出现 auth-required = 认证没过；只有画面 = 按键进去了。
    let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut frames = 0;
    while std::time::Instant::now() < end {
        if rx.recv_timeout(std::time::Duration::from_millis(200)).is_ok() {
            frames += 1;
        }
    }
    println!("发按键后收到 {frames} 帧");
}
