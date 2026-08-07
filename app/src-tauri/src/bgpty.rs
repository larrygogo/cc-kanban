//! Claude Code 后台会话（FleetView）的 PTY 旁路客户端。
//!
//! 这些会话由 claude 自己的 daemon supervisor 托管，不在任何终端里，meowo 既 spawn 不了
//! 也接管不了（杀进程会被按 `respawnFlags` 拉回来，见 [`meowo_agent::plugins::claude::fleet`]）。
//! 但 claude 给它们留了一条**官方旁路**：每个 worker 一个 PTY socket，FleetView 自己就是
//! 靠它把后台会话的画面接回前台的。地址与令牌在 `~/.claude/daemon/roster.json`：
//!
//! ```json
//! "ptySock": "\\\\.\\pipe\\cc-daemon-<hash>-pty-<short>",   // macOS 是 unix socket 路径
//! "ptyAuth": "<32 hex>"
//! ```
//!
//! ## 线路格式
//!
//! 帧 = `[4 字节大端长度][1 字节类型][payload]`，长度**只算 payload**（不含类型字节）。
//! 类型 `0` = PTY 原始字节，类型 `1` = UTF-8 的 JSON 控制帧。
//!
//! 连接后服务端依次发：`{"t":"hello","replPid":…,"version":…}` → 环形缓冲里的历史输出
//! （整屏重放）→ `{"t":"live"}` → `{"t":"ping"}`。客户端必须回 `{"t":"pong"}`（服务端按
//! 「连续几次没应答」踢连接），并发 `{"t":"auth","token":<ptyAuth>}`——键盘输入受此门控，
//! 未认证的 DATA 帧会被静默丢弃并换回一个 `{"t":"auth-required"}`。
//!
//! 客户端还可发 `{"t":"resize","cols":N,"rows":M}`（实测立刻触发 TUI 重绘）与
//! `{"t":"kill","sig":"SIGTERM"|"SIGKILL"}`——后者是**结束一个后台会话的正当入口**，
//! 比对着 pid 下手干净得多。会话退出时服务端发 `{"t":"exit","code":…,"signal":…}`。
//!
//! 以上为 2026-07-28 对 claude 2.1.220 的实测与其二进制内 ptyHost 实现的取证。
//!
//! ## 送话不走这条路
//!
//! 这条 socket **写不进键盘输入**，而且原因不在我们这边——2026-07-28 用探针
//! （`crates/meowo-agent/examples/probe_bg_input.rs`）逐段排除过：
//!
//! - 帧格式没错：不带认证发同样的数据帧，服务端**会**回 `{"t":"auth-required"}`，
//!   说明它确实被当成数据帧读了；带认证发就不回，说明认证也过了。
//! - 服务端那道闸门是开的：`if(!已退出 && !pty已关)` 里的第二个标记只在 Windows 上被
//!   SIGTERM 关闭 PTY 时置位，对活着的 worker 恒为假。所以 `pty.write(payload)` 必然执行。
//! - 控制通道上先发 `{"op":"attach",…}`（回 `{"ok":true,"via":"cold"}`）也没用。
//!
//! 也就是说字节确实进了 PTY，是 **claude 的后台 worker 自己不读 stdin**：输入框始终空着。
//! 所以本模块没有 `write`——宁可没有这个方法，也不要一个发出去没反应的假接口。
//! （还没试过的最后一条线索：花名册里与 `ptySock` 并列的 `rendezvousSock`。）
//!
//! 送话走 supervisor 的**控制通道**（[`send_prompt`]），那才是 claude 自己 UI 走的路。
//! 注意它与这条 PTY 旁路是**两套编码**：控制通道是 JSON-lines，这条是长度前缀帧。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 单帧 payload 上限。取 claude 自己在 roster 代码里用的 8 MiB：既容得下一次整屏重放，
/// 又不至于让一个坏掉的长度字段把内存吃干。
const MAX_FRAME: usize = 8 * 1024 * 1024;

/// 帧类型字节。
const KIND_DATA: u8 = 0;
const KIND_CTRL: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    /// PTY 原始字节（服务端 → 客户端是终端输出）。
    Data(Vec<u8>),
    /// JSON 控制帧的原文。解析交给调用方——控制帧种类会随 claude 版本增加，
    /// 这里认不出的类型必须原样透传而不是丢弃。
    Ctrl(String),
}

/// 把一帧编码成线路字节。
pub(crate) fn encode(frame: &Frame) -> Vec<u8> {
    let (kind, payload) = match frame {
        Frame::Data(bytes) => (KIND_DATA, bytes.as_slice()),
        Frame::Ctrl(text) => (KIND_CTRL, text.as_bytes()),
    };
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

/// 控制帧的便捷构造。`t` 是 claude 那边的判别字段。
pub(crate) fn ctrl(json: serde_json::Value) -> Frame {
    Frame::Ctrl(json.to_string())
}

/// 流式拆帧器：socket 读到的字节是任意切分的，一帧可能跨多次 read，一次 read 也可能带来
/// 好几帧（实测首次连接就是 hello + 数十 KB 重放 + live + ping 一起到）。
#[derive(Default)]
pub(crate) struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    /// 喂入新读到的字节，取出其中已完整的帧。
    ///
    /// 返回 `Err` 表示流已经不可信（长度字段越界），调用方应当断开——继续读下去只会
    /// 把噪音当帧解析。
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 5 {
                return Ok(out);
            }
            let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
            let len = len as usize;
            if len > MAX_FRAME {
                return Err(format!("帧长 {len} 超过上限 {MAX_FRAME}"));
            }
            if self.buf.len() < 5 + len {
                return Ok(out);
            }
            let kind = self.buf[4];
            let payload = self.buf[5..5 + len].to_vec();
            self.buf.drain(..5 + len);
            match kind {
                KIND_DATA => out.push(Frame::Data(payload)),
                KIND_CTRL => out.push(Frame::Ctrl(String::from_utf8_lossy(&payload).into_owned())),
                // 未知类型：跳过而不是断开。claude 加一种帧不该让整条通道失效。
                _ => {}
            }
        }
    }
}

/// 控制帧里的 `t` 字段。认不出的返回 None，调用方忽略即可。
pub(crate) fn ctrl_kind(text: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()?
        .get("t")?
        .as_str()
        .map(str::to_string)
}

/// 一条已连上的旁路：读端交给独立线程阻塞读，写端留在注册表里发控制帧。
type Wire = (Box<dyn Read + Send>, Box<dyn Write + Send>);

/// 连上 worker 的 PTY socket。
///
/// Windows 是命名管道——用 `OpenOptions` 以读写打开即可，不必引入额外依赖；
/// 其余平台是 unix socket。两边都返回一对「读端 / 写端」，读端交给独立线程阻塞读。
#[cfg(target_os = "windows")]
pub(crate) fn connect(sock: &str) -> Result<Wire, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(sock)
        .map_err(|e| format!("连接后台会话 PTY 失败（{sock}）：{e}"))?;
    let reader = file
        .try_clone()
        .map_err(|e| format!("复制 PTY 管道句柄失败：{e}"))?;
    Ok((Box::new(reader), Box::new(file)))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn connect(sock: &str) -> Result<Wire, String> {
    let stream = std::os::unix::net::UnixStream::connect(sock)
        .map_err(|e| format!("连接后台会话 PTY 失败（{sock}）：{e}"))?;
    let reader = stream
        .try_clone()
        .map_err(|e| format!("复制 PTY socket 失败：{e}"))?;
    Ok((Box::new(reader), Box::new(stream)))
}

/// 每个后台会话在本进程里的一条旁路连接。
struct BgSession {
    /// PTY 旁路的地址。resize/kill 每次另开一条短连接发，**不复用**读循环那个句柄——
    /// 见 [`BgSession::send_once`]。
    sock: String,
    /// 只给读循环自己用（pong / auth）：那两下发生在处理帧的间隙，此刻线程没卡在 read 上。
    writer: Mutex<Box<dyn Write + Send>>,
    /// 与托管 PTY 同口径的环形回看缓冲（上限见 [`BACKLOG_LIMIT`]），供对话页的终端视图
    /// 按 offset 拉增量。
    backlog: Mutex<VecDeque<u8>>,
    output_end: AtomicU64,
    /// 连接已断或会话已退出。读线程置位后不再有新字节，快照据此翻成非活跃。
    closed: AtomicBool,
    exit_code: Mutex<Option<u32>>,
}

/// backlog 上限，与 [`crate::pty`] 的托管 PTY 取同一个数：同一个终端视图在拉它们，
/// 回看长度不该因为「这个会话是谁起的」而不同。
const BACKLOG_LIMIT: usize = 1024 * 1024;

impl BgSession {
    fn send(&self, frame: &Frame) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        writer
            .write_all(&encode(frame))
            .and_then(|()| writer.flush())
            .map_err(|e| format!("写后台会话 PTY 失败：{e}"))
    }

    /// 另开一条短连接发一个控制帧，发完就断。
    ///
    /// 不能复用读循环那个句柄：Windows 上同步句柄的阻塞 `ReadFile` 会把并发的 `WriteFile`
    /// 一起挡住，而读循环绝大多数时间正卡在 read 上——从别的线程走 [`Self::send`] 发
    /// resize/kill 会无限期挂住调用它的那个 spawn_blocking 工位。实测：起了读线程时写卡满
    /// 40 秒不返回，不起读线程时立刻返回。
    ///
    /// 控制帧不需要认证（服务端只对**数据帧**查令牌），所以新连接连上就能发，不必重走握手。
    fn send_once(&self, frame: &Frame) -> Result<(), String> {
        let (_reader, mut writer) = connect(&self.sock)?;
        writer
            .write_all(&encode(frame))
            .and_then(|()| writer.flush())
            .map_err(|e| format!("写后台会话 PTY 失败：{e}"))
    }
}

/// 本进程连着的后台会话。与 [`crate::pty::PtyBroker`] 平行而不合并：那边管的是自己 spawn 的
/// 进程（有 child、能杀、能重启），这边只是搭在别人 PTY 上的一条旁路，生命周期不由我们做主。
#[derive(Clone, Default)]
pub(crate) struct BgPtyRegistry {
    sessions: Arc<Mutex<HashMap<i64, Arc<BgSession>>>>,
}

impl BgPtyRegistry {
    /// 接上一个后台会话。已经接着就直接返回——重复 attach 会多出一条连接、多一份重放。
    pub(crate) fn attach(
        &self,
        session_id: i64,
        endpoint: &meowo_agent::BackgroundEndpoint,
    ) -> Result<(), String> {
        // 已接上且还活着就直接返回——重复 attach 会多出一条连接、多一份重放。
        // 已关闭的残留条目（留着供 snapshot 交代退出码）不算数，直接被新连接顶掉。
        if self.is_active(session_id) {
            return Ok(());
        }
        let (reader, writer) = connect(&endpoint.sock)?;
        let session = Arc::new(BgSession {
            sock: endpoint.sock.clone(),
            writer: Mutex::new(writer),
            backlog: Mutex::new(VecDeque::new()),
            output_end: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            exit_code: Mutex::new(None),
        });
        // 登记要**持锁复核**一次：上面的 is_active 与这里之间有一整个 connect() 的窗口，
        // 而一次窗口打开就有三处并发调 attach（对话页的 attach effect、终端首帧 resize 的
        // 兜底、stop 前的兜底）。不复核的话两条连接都建成，后 insert 的把前一个 Arc 顶掉，
        // 被顶掉那条的 read_loop 线程与管道句柄成了孤儿，活到 worker 退出为止——每开一个
        // 后台会话漏一次。复核发现别人抢先就丢弃自己这条（drop 关句柄），不 spawn 读线程。
        {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if sessions
                .get(&session_id)
                .is_some_and(|existing| !existing.closed.load(Ordering::Acquire))
            {
                return Ok(());
            }
            sessions.insert(session_id, session.clone());
        }

        let auth = endpoint.auth.clone();
        let worker = session.clone();
        // 阻塞读独占一个线程：命名管道的 read 没有超时，塞进任何共享池都会占死一个工位。
        std::thread::spawn(move || {
            read_loop(reader, &worker, auth.as_deref());
            // 只置位，**不从表里摘掉**。摘掉的话 snapshot 会因为 `self.get()` 落空而退回
            // 托管 PTY 那条路，对一个 meowo 从没 spawn 过的会话它只能给出一份
            // start==end==0 的空快照——前端当成「重置了，重新对齐」，最后一屏画面和从
            // `{"t":"exit"}` 帧里解出来的退出码就此丢失，用户只看到终端突然变空白。
            // 条目留着由下一次 attach 顶掉（见 `attach` 开头的 is_active 判断）。
            worker.closed.store(true, Ordering::Release);
        });
        Ok(())
    }

    pub(crate) fn is_active(&self, session_id: i64) -> bool {
        self.get(session_id)
            .is_some_and(|session| !session.closed.load(Ordering::Acquire))
    }

    fn get(&self, session_id: i64) -> Option<Arc<BgSession>> {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(&session_id).cloned())
    }

    /// 按 `since` 取增量，形状与托管 PTY 的快照完全一致——对话页的终端视图分不出
    /// 自己看的是哪一种，也不必分。未接上返回 None，调用方回落到别的来源。
    pub(crate) fn snapshot(&self, session_id: i64, since: u64) -> Option<crate::pty::PtySnapshot> {
        use base64::Engine as _;
        let session = self.get(session_id)?;
        let backlog = session
            .backlog
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let end = session.output_end.load(Ordering::Acquire);
        let start = end.saturating_sub(backlog.len() as u64);
        let skip = since.saturating_sub(start).min(backlog.len() as u64) as usize;
        // as_slices + extend_from_slice 确定走 memcpy（与 pty.rs snapshot 同款理由）。
        let (front, back) = backlog.as_slices();
        let mut data: Vec<u8> = Vec::with_capacity(backlog.len() - skip);
        if skip < front.len() {
            data.extend_from_slice(&front[skip..]);
            data.extend_from_slice(back);
        } else {
            data.extend_from_slice(&back[skip - front.len()..]);
        }
        drop(backlog);
        let closed = session.closed.load(Ordering::Acquire);
        let exit_code = *session
            .exit_code
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Some(crate::pty::PtySnapshot {
            session_id,
            active: !closed,
            data: base64::engine::general_purpose::STANDARD.encode(data),
            start_offset: start + skip as u64,
            end_offset: end,
            exited: closed,
            exit_code,
        })
    }

    /// 改终端尺寸。走控制帧、不经过输入门控，实测立刻触发对面 TUI 重绘。
    pub(crate) fn resize(&self, session_id: i64, cols: u16, rows: u16) -> Result<(), String> {
        self.get(session_id)
            .ok_or("该后台会话未接入")?
            .send_once(&ctrl(
                serde_json::json!({"t": "resize", "cols": cols, "rows": rows}),
            ))
    }

    /// 结束一个后台会话。这是 claude 自己给的入口：由 ptyHost 去终止它的子进程，
    /// 而不是我们对着 pid 下手——后者会被 supervisor 按 respawnFlags 原地拉回来。
    pub(crate) fn kill(&self, session_id: i64) -> Result<(), String> {
        self.get(session_id)
            .ok_or("该后台会话未接入")?
            .send_once(&ctrl(serde_json::json!({"t": "kill", "sig": "SIGTERM"})))
    }
}

/// 向一个后台会话**发消息**。
///
/// 不走 PTY：worker 不消费 stdin，往它的 PTY 写按键石沉大海（裸 ASCII 与 win32-input-mode
/// 编码都实测无效）。送话要经 supervisor 的控制通道，由它转交给作业——这也是 claude 自己
/// 的 UI 走的路。协议是 **JSON-lines**（与 PTY 那条的长度前缀帧不同）：发一行请求，
/// 收一行 `{"ok":true,"op":"reply"}`。
///
/// 阻塞（连 socket + 等一行响应），只在 spawn_blocking 里调。
pub(crate) fn send_prompt(
    control: &meowo_agent::BackgroundControl,
    text: &str,
) -> Result<(), String> {
    // 整套「连 socket + 发一行 + 等一行回执」丢进独立线程，主线程限时等结果：Wire 是类型
    // 擦除过的 Read/Write（Windows 那端还是命名管道 File，本就没有 set_read_timeout），
    // 没法在句柄上设超时。守护进程收下连接却不回话（supervisor 卡死）时，原来的写法会让
    // read 永久阻塞——占死一个 spawn_blocking 工位，而用户点的「发送」永远不返回。
    // 超时后这条线程留给 OS 收尾（它至多阻塞到对端关闭），但每次发送最多产生一条。
    let (tx, rx) = std::sync::mpsc::channel();
    let control = control.clone();
    let text = text.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(send_prompt_blocking(&control, &text));
    });
    match rx.recv_timeout(std::time::Duration::from_millis(SEND_PROMPT_TIMEOUT_MS)) {
        Ok(result) => result,
        Err(_) => Err("后台会话没有回执（守护进程可能无响应），消息未确认送达。".to_string()),
    }
}

/// 等一行回执的上限。守护进程正常时是本机 socket 上的一次往返（毫秒级）；给到 10s 是留给
/// 它正忙着写盘/换页的余量，再久就该让用户拿回控制权，而不是对着一个不返回的按钮等。
const SEND_PROMPT_TIMEOUT_MS: u64 = 10_000;

fn send_prompt_blocking(
    control: &meowo_agent::BackgroundControl,
    text: &str,
) -> Result<(), String> {
    let (mut reader, mut writer) = connect(&control.sock)?;
    let request = serde_json::json!({
        "proto": 1,
        "op": "reply",
        "short": control.job,
        "text": text,
        "auth": control.auth,
    });
    writer
        .write_all(format!("{request}\n").as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| format!("向后台会话发送失败：{e}"))?;

    // 响应就一行，且很短；读到换行即止，避免在 socket 上干等。
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    while response.len() < 4096 {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => response.push(byte[0]),
            Err(e) => return Err(format!("等待后台会话回执失败：{e}")),
        }
    }
    let text = String::from_utf8_lossy(&response);
    let ok = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("ok").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        // 守护进程会在回执里说明原因（作业不存在、令牌不对…），原样带给用户。
        Err(format!("后台会话拒绝了这条消息：{}", text.trim()))
    }
}

/// 读线程本体：拆帧、应答保活、认证、把 PTY 字节喂进 backlog。
///
/// 三件必须做的事，少一件连接就废：`ping` 要回 `pong`（服务端按连续未应答踢人）、
/// `live` 之后要把 auth 送出去（否则输入门控永远关着）、`exit` 要记下退出码再收摊。
fn read_loop(mut reader: Box<dyn Read + Send>, session: &BgSession, auth: Option<&str>) {
    let mut decoder = Decoder::default();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let read = match reader.read(&mut buf) {
            Ok(0) | Err(_) => return, // EOF / 管道断
            Ok(n) => n,
        };
        let Ok(frames) = decoder.push(&buf[..read]) else {
            return; // 流已不可信，断开好过把噪音当帧
        };
        for frame in frames {
            match frame {
                Frame::Data(bytes) => append(session, &bytes),
                Frame::Ctrl(text) => match ctrl_kind(&text).as_deref() {
                    Some("ping") => {
                        let _ = session.send(&ctrl(serde_json::json!({"t": "pong"})));
                    }
                    Some("live") => {
                        if let Some(token) = auth {
                            let _ = session
                                .send(&ctrl(serde_json::json!({"t": "auth", "token": token})));
                        }
                    }
                    Some("exit") => {
                        let code = serde_json::from_str::<serde_json::Value>(&text)
                            .ok()
                            .and_then(|v| v.get("code").and_then(serde_json::Value::as_u64))
                            .map(|code| code as u32);
                        *session
                            .exit_code
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = code;
                        return;
                    }
                    // hello / auth-required / 将来新增的：不认识就不理会，连接照旧。
                    _ => {}
                },
            }
        }
    }
}

/// 把 PTY 字节追加进环形回看缓冲，并推进累计偏移。
fn append(session: &BgSession, bytes: &[u8]) {
    let mut backlog = session
        .backlog
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    backlog.extend(bytes.iter().copied());
    // drain 一次成段移除，不逐字节 pop_front（缓冲满后那是每 chunk 上万次，还在锁内）。
    let excess = backlog.len().saturating_sub(BACKLOG_LIMIT);
    if excess > 0 {
        backlog.drain(..excess);
    }
    // 偏移记的是**累计产出**，不是缓冲里还剩多少——前端按它对齐增量，不能因为
    // 老字节被挤掉就倒退。
    session
        .output_end
        .fetch_add(bytes.len() as u64, Ordering::AcqRel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_survive_a_round_trip() {
        for frame in [
            Frame::Data(b"\x1b[2J hello".to_vec()),
            Frame::Ctrl(r#"{"t":"pong"}"#.to_string()),
            Frame::Data(Vec::new()),
        ] {
            let mut decoder = Decoder::default();
            assert_eq!(decoder.push(&encode(&frame)).unwrap(), vec![frame]);
        }
    }

    /// 真机首帧的字节，逐字节锁住线路格式：长度字段**只算 payload**。
    /// 这条断言的价值在于它是从 claude 2.1.220 的实际输出抄下来的——口径要是错了
    /// （曾经就把类型字节算进了长度），解析会整体错位一个字节。
    #[test]
    fn the_length_prefix_counts_only_the_payload() {
        let hello = br#"{"t":"hello","replPid":9372,"version":"2.1.220"}"#;
        let mut wire = vec![0, 0, 0, hello.len() as u8, 1];
        wire.extend_from_slice(hello);
        assert_eq!(hello.len(), 48);
        assert_eq!(wire.len(), 53, "整帧 = 4 长度 + 1 类型 + 48 payload");

        let frames = Decoder::default().push(&wire).unwrap();
        assert_eq!(
            frames,
            vec![Frame::Ctrl(String::from_utf8_lossy(hello).into_owned())]
        );
        assert_eq!(
            ctrl_kind(&String::from_utf8_lossy(hello)).as_deref(),
            Some("hello")
        );
    }

    /// socket 的切分与帧边界无关：一帧可能跨多次 read，一次 read 也可能带来好几帧
    /// （实测首连就是 hello + 数十 KB 重放 + live + ping 一起到）。
    #[test]
    fn a_frame_may_arrive_in_any_number_of_pieces() {
        let wire: Vec<u8> = [
            encode(&Frame::Ctrl(r#"{"t":"live"}"#.into())),
            encode(&Frame::Data(b"screen".to_vec())),
            encode(&Frame::Ctrl(r#"{"t":"ping"}"#.into())),
        ]
        .concat();

        // 一次到齐。
        assert_eq!(Decoder::default().push(&wire).unwrap().len(), 3);

        // 逐字节喂：帧只在收齐的那一刻出现，且顺序不变。
        let mut decoder = Decoder::default();
        let mut got = Vec::new();
        for byte in &wire {
            got.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(got.len(), 3);
        assert_eq!(got[1], Frame::Data(b"screen".to_vec()));
    }

    #[test]
    fn an_unknown_frame_type_is_skipped_without_killing_the_stream() {
        let mut wire = vec![0, 0, 0, 2, 9]; // 类型 9：将来某个版本新加的
        wire.extend_from_slice(b"hi");
        wire.extend_from_slice(&encode(&Frame::Ctrl(r#"{"t":"live"}"#.into())));

        let frames = Decoder::default().push(&wire).unwrap();
        assert_eq!(frames, vec![Frame::Ctrl(r#"{"t":"live"}"#.into())]);
    }

    /// 坏掉的长度字段必须当场翻脸：把它当帧长会先分配一坨内存，再把后续字节全解析错。
    #[test]
    fn an_oversized_length_is_rejected_rather_than_allocated() {
        let wire = vec![0xff, 0xff, 0xff, 0xff, 0];
        assert!(Decoder::default().push(&wire).is_err());
    }

    #[test]
    fn ctrl_kind_tolerates_junk() {
        assert_eq!(
            ctrl_kind(r#"{"t":"exit","code":0}"#).as_deref(),
            Some("exit")
        );
        assert_eq!(ctrl_kind("{ not json"), None);
        assert_eq!(ctrl_kind(r#"{"no":"t"}"#), None);
    }

    /// 本机花名册里第一个还连得上的后台会话。没有就返回 None（CI 与他人机器上跳过）。
    fn a_real_background_endpoint() -> Option<meowo_agent::BackgroundEndpoint> {
        let runtime = meowo_agent::resolve(Some("claude"))?.runtime()?;
        runtime
            .session_runtimes()
            .into_iter()
            .filter(|(_, form)| form.background)
            .find_map(|(session_id, _)| runtime.background_endpoint(&session_id))
    }

    /// 端到端联通：连上一个**真实**的后台会话，确认整屏重放收得到。
    ///
    /// 这条用例守的是合成用例守不住的东西——线路格式、握手顺序、以及「claude 真的会在连接
    /// 建立时把画面推过来」。协议是 agent 的内部约定，改了就该在这里当场红。
    /// 全程只读：不发任何输入帧。
    #[test]
    fn a_real_background_session_replays_its_screen_on_attach() {
        let Some(endpoint) = a_real_background_endpoint() else {
            eprintln!("跳过：本机没有可连的后台会话");
            return;
        };
        let registry = BgPtyRegistry::default();
        registry.attach(1, &endpoint).expect("连得上");

        // 重放是连上就推的，但要过一次线程调度；轮询到有字节为止，最多 2 秒。
        let mut snapshot = registry.snapshot(1, 0).expect("已接入");
        for _ in 0..40 {
            if snapshot.end_offset > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            snapshot = registry.snapshot(1, 0).expect("已接入");
        }
        assert!(snapshot.active, "连上后应处于活跃");
        assert!(
            snapshot.end_offset > 0,
            "连上后应当收到整屏重放，实际一个字节都没有——线路格式或握手顺序变了"
        );

        // 增量口径：从末尾拉应当拿到空数据，且偏移不倒退。
        let tail = registry.snapshot(1, snapshot.end_offset).expect("已接入");
        assert_eq!(tail.start_offset, tail.end_offset);
        assert!(tail.end_offset >= snapshot.end_offset);
    }

    #[test]
    fn ctrl_builds_the_json_claude_expects() {
        let Frame::Ctrl(text) = ctrl(serde_json::json!({"t": "resize", "cols": 120, "rows": 30}))
        else {
            panic!("应当是控制帧");
        };
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["t"], "resize");
        assert_eq!(parsed["cols"], 120);
    }
}
