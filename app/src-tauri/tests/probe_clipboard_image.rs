//! 定位「剪贴板原生图片附加」真机失败的环节（结论已固化，留存作回归探针）。
//!
//! 结论（2026-09 真机实测，kimi 0.29.0 / Windows）：
//!   - 失败环节是**注入按键**：kimi 在 win32 的贴图键是 Alt+V（二进制里
//!     `matchesKey(..., win32 ? "alt+v" : ctrl("v"))`，当前版与上一版 .bak 一致），
//!     生产此前发的 `\x16`（Ctrl-V）被 composer 整个无视——零回显、零报错、零铃声。
//!   - 可用形态是传统 Meta 编码 `\x1bv`（场景 C）：~40ms 落出 `[image #1 (120×48)]`。
//!     kitty CSI-u 编码（Ctrl+V `ESC[118;5u` / Alt+V `ESC[118;3u`）均不被认——
//!     kimi 虽推 `ESC[>7u` 启用 kitty 协议，这个键的匹配走传统 ESC 前缀形态。
//!   - 剪贴板环节无问题：arboard set_image 即时渲染 CF_DIBV5 + "PNG"，跨实例回读成功；
//!     生产 1.5s 占位符等待与 marker 正则 `\[image[:# ]` 也都对（占位符 41ms 即落）。
//!   - 修复：`clipboard_paste_input` 能力由插件声明（kimi = `\x1bv`），前端不再硬编码 `\x16`。
//!
//! 另有两个 PTY 宿主层面的坑（对本探针与未来的探针都适用）：
//!   - portable-pty 的 master 必须活到会话结束，提前 drop = ClosePseudoConsole。
//!   - kimi 启动会发设备查询（DSR `ESC[6n`、DA `ESC[c`），不应答就卡在初始化。
//!
//! `cargo test -p meowo-app --test probe_clipboard_image -- --ignored --nocapture`

use portable_pty::{native_pty_system, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod common;

fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' || c == '\u{1b}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 与 clipboard_set_image（src/chat.rs）同一代码路径：image 解码成 RGBA → arboard set_image。
fn set_clipboard_image(path: &std::path::Path) -> Result<(u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let rgba = image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Owned(rgba.into_raw()),
        })
        .map_err(|e| e.to_string())?;
    Ok((width, height))
}

/// 写一张测试 PNG，返回路径。
fn make_test_png(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("probe-image.png");
    let mut img = image::RgbaImage::new(120, 48);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgba([(x % 256) as u8, (y % 256) as u8, 200, 255]);
    }
    img.save(&path).unwrap();
    path
}

struct Pty {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: mpsc::Receiver<Vec<u8>>,
    // master 必须活到会话结束：ConPTY 对象挂在 master 上，提前 drop 等于 ClosePseudoConsole，
    // 子进程的控制台被拆、读端立刻 EOF（生产 pty.rs 同样把 master 存进 ManagedPty 常驻）。
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

fn spawn_kimi(cwd: &std::path::Path) -> Pty {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap();
    let exe = format!("{home}/.kimi-code/bin/kimi");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let cmd = common::agent_command(&exe, cwd);
    let child = pair.slave.spawn_command(cmd).unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = pair.master.take_writer().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    Pty {
        child,
        writer,
        rx,
        _master: pair.master,
    }
}

/// 像真终端一样应答设备查询：DSR（`ESC[6n` → 光标位置）、DA1（`ESC[c` → 终端能力）。
/// 缺了应答 kimi 会卡在初始化等回复（真机探针实测：只答 DSR 不答 DA 时首屏静默）。
fn answer_queries(pty: &mut Pty, chunk: &[u8], tag: &str) {
    let mut out: Vec<u8> = Vec::new();
    if chunk.windows(4).any(|w| w == b"\x1b[6n") {
        out.extend_from_slice(b"\x1b[1;1R");
    }
    if chunk.windows(3).any(|w| w == b"\x1b[c") || chunk.windows(4).any(|w| w == b"\x1b[0c") {
        out.extend_from_slice(b"\x1b[?1;2c");
    }
    if !out.is_empty() {
        let wr = pty.writer.write_all(&out).and_then(|_| pty.writer.flush());
        eprintln!("[{tag}] 查询应答 {out:?}: {wr:?}");
    }
}

/// 等首屏安静下来（应答设备查询、确认 kitty 协议请求），返回首屏原始字节。
/// Windows 上 Node 冷启动可能先沉默数秒：没见过任何输出前不能按「安静」退出，
/// 只有收到过输出之后再连续静默才算首屏落定。
fn wait_boot(pty: &mut Pty) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let start = Instant::now();
    let mut boot = Vec::new();
    while Instant::now() < deadline {
        match pty.rx.recv_timeout(Duration::from_millis(1500)) {
            Ok(c) => {
                eprintln!("[boot] +{}B @{:?}: {:?}", c.len(), start.elapsed(), &c[..c.len().min(48)]);
                answer_queries(pty, &c, "boot");
                boot.extend_from_slice(&c);
            }
            Err(_) if !boot.is_empty() => break,
            Err(_) => {
                eprintln!("[boot] 静默 @{:?}, try_wait={:?}", start.elapsed(), pty.child.try_wait());
            }
        }
    }
    if boot.windows(6).any(|w| w == b"\x1b[>7u") {
        eprintln!("[确认] kimi 请求启用 kitty 键盘协议（ESC[>7u）");
    } else {
        eprintln!("[注意] 首屏未见 ESC[>7u，kimi 可能没进 kitty 协议");
    }
    let screen = strip_ansi(&boot);
    let lines: Vec<&str> = screen.lines().collect();
    let tail = lines[lines.len().saturating_sub(10)..].join("\n");
    eprintln!("[首屏尾部]\n{tail}");
    std::thread::sleep(Duration::from_millis(800));
    boot
}

/// 注入按键后观察 `secs` 秒，返回增量原始字节。收满整个窗口：kimi 读剪贴板可能
/// 要拉起 PowerShell，冷启动以秒计，静音不等于没反应（生产的 1.5s 等待正是嫌疑点）。
fn watch(pty: &mut Pty, secs: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let start = Instant::now();
    let end = start + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Ok(c) = pty.rx.recv_timeout(Duration::from_millis(500)) {
            eprintln!("[watch] +{}B @{:?}", c.len(), start.elapsed());
            answer_queries(pty, &c, "watch");
            out.extend_from_slice(&c);
        }
    }
    out
}

#[test]
#[ignore = "拉起真实 kimi 进程并动系统剪贴板；手动调研用"]
fn probe_clipboard_image_paste() {
    let cwd = std::env::temp_dir().join(format!("meowo-clipimg-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    let png = make_test_png(&cwd);

    // 0. 剪贴板写入环节校验：set_image 后跨实例回读。
    let (w, h) = set_clipboard_image(&png).expect("set_clipboard_image 失败");
    eprintln!("[剪贴板] set_image 成功：{w}x{h}");
    match arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
        Ok(img) => eprintln!("[剪贴板] 跨实例 get_image 成功：{}x{}", img.width, img.height),
        Err(e) => eprintln!("[剪贴板] 跨实例 get_image 失败：{e}"),
    }

    for (label, key) in [
        ("A: 裸 \\x16（生产现状）", b"\x16".as_slice()),
        ("B: kitty CSI-u Ctrl+V", b"\x1b[118;5u".as_slice()),
        // kimi 0.29 源码：win32 的贴图键是 alt+v（getPasteImageShortcut /
        // matchesKey(... "alt+v" : Key.ctrl("v"))），Ctrl+V 只在非 Windows 生效。
        ("C: 传统 Meta（ESC+v）", b"\x1bv".as_slice()),
        ("D: kitty CSI-u Alt+V", b"\x1b[118;3u".as_slice()),
    ] {
        // 每轮重新把图写进剪贴板：上一轮 kimi 若读了剪贴板，个别平台会动所有权。
        set_clipboard_image(&png).expect("set_clipboard_image 失败");
        let mut pty = spawn_kimi(&cwd);
        wait_boot(&mut pty);
        // 输入通路自检：先打个普通字符，composer 不回显就是注入通路本身断了，
        // 后面 Ctrl-V 无反应就不能算在粘贴头上。
        let _ = pty.writer.write_all(b"x").and_then(|_| pty.writer.flush());
        let echo = watch(&mut pty, 2);
        eprintln!("### {label} → 普通字符 x 回显: {:?}", strip_ansi(&echo));
        let _ = pty.writer.write_all(b"\x7f").and_then(|_| pty.writer.flush());
        std::thread::sleep(Duration::from_millis(400));
        while pty.rx.try_recv().is_ok() {}
        let _ = pty.writer.write_all(key).and_then(|_| pty.writer.flush());
        let after = watch(&mut pty, 8);
        let screen = strip_ansi(&after);
        let lines: Vec<&str> = screen.lines().collect();
        let tail = lines[lines.len().saturating_sub(8)..].join("\n");
        eprintln!("\n### {label} → 按键后画面（尾部）:\n{tail}\n");
        eprintln!(
            "### {label} → marker `\\[image` 出现：{}；铃：{}",
            after
                .windows(6)
                .any(|w| w == b"[image"),
            after.contains(&0x07)
        );
        // 原始字节里找 image 上下文，确认真实形态（颜色序列/换行）。
        if let Some(at) = after.windows(5).position(|w| w == b"image") {
            let lo = at.saturating_sub(24);
            let hi = (at + 48).min(after.len());
            eprintln!("### {label} → 原始上下文: {:?}", &after[lo..hi]);
        }
        // 撤掉可能落进 composer 的残留，再关掉。
        let _ = pty.writer.write_all(b"\x15").and_then(|_| pty.writer.flush());
        std::thread::sleep(Duration::from_millis(300));
        let _ = pty.child.kill();
    }

    let _ = std::fs::remove_dir_all(&cwd);
}
