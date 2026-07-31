//! Meowo 持有的 PTY broker。结构化对话仍走 transcript；这里仅负责原始 ANSI 终端的双向镜像。

use base64::Engine;
use meowo_protocol::broker::{read_handshake, BrokerRequest, CURRENT_PROTOCOL_VERSION};
pub(crate) use meowo_protocol::broker::{ApprovalDecision, ApprovalRequest};
#[cfg(not(test))]
use meowo_protocol::broker::{BrokerDiscovery, APPROVAL_BROKER_FILE};
pub(crate) use meowo_protocol::ipc::ManagedTerminalSnapshotDto as PtySnapshot;
use meowo_protocol::ipc::{PtyExitEvent as PtyExit, PtyOutputEvent as PtyOutput};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use tauri::{Emitter, Manager};

const BACKLOG_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct TerminalSize {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

impl TerminalSize {
    pub(crate) const fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}

struct ManagedPty {
    session_id: AtomicI64,
    /// Option 是给收尾用的：ClosePseudoConsole（drop）让 conhost 退出、释放资源。
    /// 注意它**不能**唤醒已阻塞的 reader（本机实证），所以收尾从不等 reader。None = 已关闭。
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// 收尾只许跑一次：waiter（轮询到进程退出）与 reader（万一真的 EOF）都会尝试触发。
    finalized: AtomicBool,
    /// PTY 输入的有界队列入口；对 ConPTY 管道的阻塞写全部由独立 writer 线程承担。
    /// 子进程不读 stdin 时管道写会**无限期阻塞**，绝不能发生在 write() 的调用线程上
    /// （它可能是 IPC/blocking 池线程，历史上还是主线程——一次卡住就冻结整应用）。
    /// 队满由 write() 的有界等待兜住；writer 线程写失败即退出并丢弃 rx，之后 try_send
    /// 以 Disconnected 快速失败。
    input_tx: mpsc::SyncSender<Vec<u8>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    backlog: Mutex<VecDeque<u8>>,
    /// 自 PTY 启动以来累计输出的字节位置；与 backlog 锁内更新，供快照和实时帧去重排序。
    output_end: AtomicU64,
    subscribers: Mutex<Vec<AttachSubscriber>>,
}

/// 一个在线的外部同步终端（attach 客户端）。pid 是客户端上报的自身进程号，供查重
/// 路径反查宿主终端窗口做精确聚焦；旧 reporter 不上报 → None。
struct AttachSubscriber {
    id: u64,
    pid: Option<u32>,
    tx: mpsc::Sender<Vec<u8>>,
}

#[derive(Clone)]
struct CompletedPty {
    data: Vec<u8>,
    start_offset: u64,
    end_offset: u64,
    code: Option<u32>,
    /// 入表顺序，淘汰时取最小者。不能按 session id 淘汰：pending 启动失败的条目
    /// 是递减的负数 id，按 id 取 min 会恰好先扔掉最新的失败诊断。
    seq: u64,
}

/// [`CompletedPty::seq`] 的全局递增源。只求单调，跨 broker 共用无妨。
static COMPLETED_SEQ: AtomicU64 = AtomicU64::new(0);

struct AttachState {
    endpoint: Mutex<Option<SocketAddr>>,
    token: String,
    started: AtomicBool,
    next_subscriber: AtomicU64,
    next_pending: AtomicI64,
    pending: Mutex<HashMap<String, i64>>,
    bindings: Mutex<HashMap<i64, i64>>,
    approvals: Mutex<HashMap<String, PendingApproval>>,
    /// 显式注册的 GUI 审批消费者。窗口存在/可见不等于已经订阅了目标 session。
    approval_consumers: Mutex<HashMap<String, i64>>,
    app: Mutex<Option<tauri::AppHandle>>,
}

struct PendingApproval {
    request: ApprovalRequest,
    response: mpsc::Sender<ApprovalDecision>,
}

#[derive(Clone)]
pub(crate) struct PtyBroker {
    sessions: Arc<Mutex<HashMap<i64, Arc<ManagedPty>>>>,
    /// 已登记、但还在锁外 openpty+spawn 的会话（纯集合，值无语义）。冷启动叠加杀软扫描时
    /// spawn 可达数秒，绝不能用 sessions 锁跨过它——snapshot/write/resize/stop 都是主线程
    /// 上的同步 Tauri 命令，持锁期间它们全部排队，一个会话冷启动卡顿就冻结整应用。
    /// 占位只承担「防重复启动」语义：读路径把它当作尚未运行——snapshot 回 inactive 空帧
    /// （与启动前一致，前端本就在等 start 返回），write/resize/stop 按「未运行」快速失败。
    starting: Arc<Mutex<HashSet<i64>>>,
    /// GUI 退出时置位。shutdown 先置位再抢 sessions 锁 drain；start 登记前在同一把锁内
    /// 复核它——「spawn 完成时 shutdown 已 drain 完」的会话必须当场杀掉，不能塞回表里孤儿化。
    shutting_down: Arc<AtomicBool>,
    completed: Arc<Mutex<HashMap<i64, CompletedPty>>>,
    attach: Arc<AttachState>,
}

impl Default for PtyBroker {
    fn default() -> Self {
        let token = random_token();
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            starting: Arc::new(Mutex::new(HashSet::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(Mutex::new(HashMap::new())),
            attach: Arc::new(AttachState {
                endpoint: Mutex::new(None),
                token,
                started: AtomicBool::new(false),
                next_subscriber: AtomicU64::new(1),
                next_pending: AtomicI64::new(-1),
                pending: Mutex::new(HashMap::new()),
                bindings: Mutex::new(HashMap::new()),
                approvals: Mutex::new(HashMap::new()),
                approval_consumers: Mutex::new(HashMap::new()),
                app: Mutex::new(None),
            }),
        }
    }
}

/// 从 ANSI 输出流里提取人能读的尾部文本：剥掉 CSI/OSC 转义与控制字符，
/// 取非空行的最后一段，限长 `max_chars`。给「Agent 秒退」的报错信息用。
fn readable_tail(data: &[u8], max_chars: usize) -> String {
    // 先按字节剥转义（UTF-8 多字节原样保留，最后统一 lossy 解码——逐字节转 char 会把中文拆成乱码）。
    let mut bytes: Vec<u8> = Vec::with_capacity(data.len().min(4096));
    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        if byte == 0x1b {
            i += 1;
            match data.get(i) {
                Some(b'[') => {
                    i += 1;
                    while i < data.len() && !(0x40..=0x7e).contains(&data[i]) {
                        i += 1;
                    }
                }
                Some(b']') => {
                    i += 1;
                    while i < data.len() && data[i] != 0x07 && data[i] != 0x1b {
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        if byte == b'\n' || byte >= 0x20 && byte != 0x7f || byte >= 0x80 {
            bytes.push(byte);
        }
        i += 1;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = lines
        .into_iter()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" · ");
    let mut out: String = tail.chars().take(max_chars).collect();
    if out.len() < tail.len() {
        out.push('…');
    }
    out
}

/// PTY 会话的唯一收尾路径：写库、通知看板与对话窗、掐断 attach、释放伪终端。
/// 幂等（finalized 原子门），由两处触发——waiter 轮询到进程退出（主力：Windows ConPTY
/// 在子进程退出后**不会**给 reader EOF，本机实证连 drop master 都唤不醒阻塞中的 read），
/// 以及 reader 万一真读到 EOF。收尾**从不等待 reader**；它若永远阻塞，就带着句柄躺着。
fn finalize_exit(broker: &PtyBroker, app: &tauri::AppHandle, managed: &Arc<ManagedPty>) {
    if managed.finalized.swap(true, Ordering::AcqRel) {
        return;
    }
    // 到这里进程必已退出（waiter try_wait 确认过）或即将退出（EOF 场景），wait 不会久等。
    let code = managed
        .child
        .lock()
        .ok()
        .and_then(|mut child| child.wait().ok())
        .map(|s| s.exit_code());
    // 释放伪终端让 conhost 退出。它救不了阻塞的 reader，但能停掉资源。
    if let Ok(mut master) = managed.master.lock() {
        drop(master.take());
    }
    let session_id = managed.session_id.load(Ordering::Acquire);
    let final_data = managed
        .backlog
        .lock()
        .map(|backlog| backlog.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    let end_offset = managed.output_end.load(Ordering::Acquire);
    let start_offset = end_offset.saturating_sub(final_data.len() as u64);
    if let Ok(mut completed) = broker.completed.lock() {
        // 退出输出只为诊断与终端回放保留；限制条数，避免长期运行无限增长。
        // 按入表顺序淘汰最旧的一条（见 CompletedPty::seq 注释）。
        if completed.len() >= 24 {
            if let Some(oldest) = completed
                .iter()
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(id, _)| *id)
            {
                completed.remove(&oldest);
            }
        }
        completed.insert(
            session_id,
            CompletedPty {
                data: final_data,
                start_offset,
                end_offset,
                code,
                seq: COMPLETED_SEQ.fetch_add(1, Ordering::Relaxed),
            },
        );
    }
    if let Ok(mut sessions) = broker.sessions.lock() {
        if sessions
            .get(&session_id)
            .is_some_and(|current| Arc::ptr_eq(current, managed))
        {
            sessions.remove(&session_id);
        }
    }
    // 必须显式掐断订阅，不能指望「subscribers 随 ManagedPty 一起 drop」：attach 的
    // 服务线程自己持有这个 Arc（等客户端输入），tx 又在 Arc 里——彼此等对方先死，
    // 谁都死不了。结果是外部同步终端在会话结束后永远定格在一片静止画面上。
    // 清掉 tx → 转发线程 rx 断开并关 socket → 客户端收到 EOF 正常退出。
    if let Ok(mut subscribers) = managed.subscribers.lock() {
        subscribers.clear();
    }
    if session_id < 0 {
        if let Ok(mut pending) = broker.attach.pending.lock() {
            pending.retain(|_, id| *id != session_id);
        }
    } else {
        // 托管 PTY 是这个 agent 进程的唯一持有者——它退出，会话就真的结束了。必须主动
        // 收尾：resume 路径已经乐观复活过 DB（prepare_resume），没人回滚的话卡片会一直
        // 假显示「已连接」，直到 pid 判活的宽限窗口过期才自愈。这同时覆盖了「PTY 起来了
        // 但 CLI 秒退（不在 PATH）」——那种情况 start() 返回 Ok，调用方的回滚够不着。
        // 写失败时卡片会假显示「已连接」，靠 pid 判活的宽限窗口自愈；留一条日志方便定位。
        match crate::open_store(&crate::db_path()) {
            Ok(store) => {
                if let Err(error) = store.end_session(session_id, crate::now_ms()) {
                    eprintln!("PTY 退出后回写会话结束状态失败（等待 pid 宽限窗口自愈）: {error}");
                }
            }
            Err(error) => {
                eprintln!("PTY 退出后打开数据库失败（等待 pid 宽限窗口自愈）: {error}");
            }
        }
        if let Ok(mut bindings) = broker.attach.bindings.lock() {
            bindings.retain(|_, real| *real != session_id);
        }
        crate::watch::emit_board_changed(app, "pty-exit");
    }
    if let Some(window) = app.get_webview_window("chat") {
        let _ = window.emit("pty-exit", PtyExit { session_id, code });
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        // OS RNG 不可用属于极端退化；仍混入进程/时间，且服务只监听 loopback。
        let seed = format!("{}-{:?}", std::process::id(), std::time::SystemTime::now());
        for (i, byte) in seed.bytes().enumerate() {
            bytes[i % 32] ^= byte;
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// TUI 启动探测(`ESC[6n` 光标位置查询)的**唯一**应答者。
///
/// claude 等 TUI 启动时先发 `ESC[6n` 探测终端,**得不到回应就永远不画第一帧**;而此刻
/// 往往还没有任何视图挂着(对话窗 WebView 冷启动要一两秒,外部 attach 更晚),查询只能
/// 躺在 backlog 里等回放。谁替它答?此前的答案是「每个视图自己答」:xterm 自动应答、
/// attach 客户端 DsrFilter 代答、前端回放代答——应答者一多,再叠上重连/重挂对同一段
/// backlog 的重扫,同一个查询就会被答多次;多出来的应答落进 agent 输入框,成了孤立的
/// 杂字符(真实案例:恢复会话后 claude 的 composer 里凭空多一个 C)。
///
/// 现在收成单一所有者:PTY reader 对输出流做**单趟**扫描,首个可见字节之前的 `ESC[6n]`
/// 由后端当场代答 `ESC[1;1R`——TUI 只要一个基准值,真实排版靠后续的清屏与绝对定位
/// 序列,(1,1) 与 attach 客户端 DsrFilter 的既有行为一致。单趟意味着每个查询字节只被
/// 检视一次,重连、回放、组件重挂都不可能触发第二次应答。首帧画出后 TUI 的实时查询
/// 交还给活着的视图(xterm 以真实光标位置应答),本扫描器永久停机。
///
/// **已代答的探测同时从流中摘除**:单一应答者的另一半含义是下游根本看不到已答的查询。
/// 摘除之前,同一条首帧前探测会被订阅在先的 attach 客户端(DsrFilter 对实时查询全数
/// 代答)或与首个可见字节挤进同一事件帧的 GUI xterm 再答一遍——多出的应答落进 agent
/// 输入框。摘除之后 DsrFilter/xterm 只会遇到首帧后的实时查询,那正是它们该答的。
struct StartupProbeScanner {
    /// 已见到可见字节:探测期结束,feed 恒原样透传。
    painted: bool,
    state: ProbeScanState,
    /// 疑似探测前缀的暂存(`ESC`/`ESC[`/`ESC[6`,最多 3 字节):确定是探测则丢弃
    /// (已代答),确定不是则原样冲入输出。只在首帧前使用,扣住几个字节无副作用;
    /// 回到 Ground 时恒为空,painted 翻转只发生在 Ground,故停机时不会扣留字节。
    hold: Vec<u8>,
}

#[derive(Clone, Copy)]
enum ProbeScanState {
    Ground,
    Esc,
    /// ESC + 中间字节(0x20-0x2F,如 charset 指定 `ESC ( B`):负载不是画面,等最终字节。
    /// 此前这类序列落回 Ground,负载字节被误判为可见输出,扫描器提前停机——探测从此
    /// 没人答,TUI 卡在首帧前(25s 兜底后黑屏)。
    EscIntermediate,
    /// CSI:hold 非空 = 仍可能是探测;hold 已冲出 = 已排除,透传到最终字节。
    Csi,
    Osc,
    OscEsc,
    /// DCS/SOS/PM/APC 字符串(ESC P/X/^/_):负载不是画面,吞到 ST(ESC \)。
    Str,
    StrEsc,
}

impl StartupProbeScanner {
    fn new() -> Self {
        Self {
            painted: false,
            state: ProbeScanState::Ground,
            hold: Vec::new(),
        }
    }

    fn painted(&self) -> bool {
        self.painted
    }

    fn flush(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.hold);
    }

    /// 返回(应转发/入 backlog 的字节, 本 chunk 需要代答的探测个数)。已代答的探测不在
    /// 返回字节里;疑似探测前缀暂存到下一 chunk,撕裂在边界上的查询照样只答一次、不外流。
    /// 可见性口径与前端 hasVisibleOutput 一致:ESC 序列与 ≤0x20 的空白控制字节都不算画面。
    fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, usize) {
        let mut out = Vec::with_capacity(self.hold.len() + chunk.len());
        if self.painted {
            out.extend_from_slice(chunk);
            return (out, 0);
        }
        let mut probes = 0;
        for (index, &byte) in chunk.iter().enumerate() {
            match self.state {
                ProbeScanState::Ground => {
                    if byte == 0x1b {
                        self.hold.push(byte);
                        self.state = ProbeScanState::Esc;
                    } else if byte > 0x20 && byte != 0x7f {
                        // 可见字节:探测期结束,本字节与其后全部原样透传。
                        self.painted = true;
                        out.push(byte);
                        out.extend_from_slice(&chunk[index + 1..]);
                        return (out, probes);
                    } else {
                        out.push(byte);
                    }
                }
                // Esc 状态 hold 恒为 [ESC](只从 Ground 进入)。
                ProbeScanState::Esc => match byte {
                    0x5b => {
                        self.hold.push(byte);
                        self.state = ProbeScanState::Csi;
                    }
                    0x5d => {
                        self.flush(&mut out);
                        out.push(byte);
                        self.state = ProbeScanState::Osc;
                    }
                    // ESC ESC:冲出前一个,新的接着暂存(hold 恰好不变)。
                    0x1b => out.push(0x1b),
                    0x20..=0x2f => {
                        self.flush(&mut out);
                        out.push(byte);
                        self.state = ProbeScanState::EscIntermediate;
                    }
                    b'P' | b'X' | b'^' | b'_' => {
                        self.flush(&mut out);
                        out.push(byte);
                        self.state = ProbeScanState::Str;
                    }
                    // 双字节 ESC 序列(ESC 7 等):吞掉 kind 字节回到地面。
                    _ => {
                        self.flush(&mut out);
                        out.push(byte);
                        self.state = ProbeScanState::Ground;
                    }
                },
                ProbeScanState::EscIntermediate => {
                    out.push(byte);
                    // 中间字节(0x20-0x2F)可连续多个;其余任意字节都当最终字节收尾。
                    if !(0x20..=0x2f).contains(&byte) {
                        self.state = ProbeScanState::Ground;
                    }
                }
                ProbeScanState::Csi => {
                    if self.hold.is_empty() {
                        // 已排除探测的 CSI:透传到最终字节。
                        out.push(byte);
                        if (0x40..=0x7e).contains(&byte) {
                            self.state = ProbeScanState::Ground;
                        }
                    } else if self.hold.len() == 2 && byte == b'6' {
                        self.hold.push(byte);
                    } else if self.hold.len() == 3 && byte == b'n' {
                        // 完整 `ESC[6n`:代答,并把这四个字节从流中摘除。
                        self.hold.clear();
                        probes += 1;
                        self.state = ProbeScanState::Ground;
                    } else {
                        // 参数不是恰好 "6":排除探测,冲出暂存,本字节照常处理。
                        self.flush(&mut out);
                        out.push(byte);
                        if (0x40..=0x7e).contains(&byte) {
                            self.state = ProbeScanState::Ground;
                        }
                    }
                }
                ProbeScanState::Osc => {
                    out.push(byte);
                    if byte == 0x07 {
                        self.state = ProbeScanState::Ground;
                    } else if byte == 0x1b {
                        self.state = ProbeScanState::OscEsc;
                    }
                }
                ProbeScanState::OscEsc => {
                    // ST(ESC \)收尾;其余当 OSC 内容继续吞。
                    out.push(byte);
                    self.state = if byte == 0x5c {
                        ProbeScanState::Ground
                    } else {
                        ProbeScanState::Osc
                    };
                }
                ProbeScanState::Str => {
                    out.push(byte);
                    if byte == 0x1b {
                        self.state = ProbeScanState::StrEsc;
                    }
                }
                ProbeScanState::StrEsc => {
                    out.push(byte);
                    self.state = match byte {
                        0x5c => ProbeScanState::Ground,
                        0x1b => ProbeScanState::StrEsc,
                        _ => ProbeScanState::Str,
                    };
                }
            }
        }
        (out, probes)
    }
}

/// 从**展示用**字节流中移除完整的 `ESC[6n` 查询。只用于 attach 回放:客户端 DsrFilter
/// 会对流里的每个查询代答一遍,而回放里的查询全是历史——首帧前的探测已在 reader 处
/// 被摘除根本不进 backlog(StartupProbeScanner),留下的是首帧后的查询,当年已由活着
/// 的视图答过,迟到的代答会打进正跑着的 agent 输入框(「重开外部同步终端后 composer
/// 里多一个 C」的直接来源)。backlog 本体与偏移一个字节都不能动:GUI 快照按偏移对齐
/// 增量。跨 backlog 裁剪边界的残缺前缀不匹配、原样保留(既有的碎片语义)。
fn strip_dsr_queries(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(b"\x1b[6n") {
            i += 4;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(2, 500),
        cols: cols.clamp(2, 500),
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl PtyBroker {
    pub(crate) fn set_app_handle(&self, app: tauri::AppHandle) {
        if let Ok(mut current) = self.attach.app.lock() {
            *current = Some(app);
        }
    }

    /// 发给前端的必须是 [`PendingApprovalDto`]（GUI 边界的稳定形态），**不能**是原始
    /// [`ApprovalRequest`]：后者是 reporter↔app 的线路结构，`permission_suggestions` 空时
    /// 会被 `skip_serializing_if` 整个略去——而前端类型（ts-rs 从 DTO 生成）承诺该字段恒在，
    /// 拿到瘦负载就在 `.map` 上崩整个 ChatWindow。codex 的审批从不带 suggestions，必踩。
    fn emit_approval(&self, event: &str, request: &ApprovalRequest) {
        if let Some(app) = self.attach.app.lock().ok().and_then(|app| app.clone()) {
            let _ = app.emit(
                event,
                meowo_protocol::ipc::PendingApprovalDto::from(request.clone()),
            );
        }
    }

    /// 审批 broker 只有在对应对话窗确实可用时才能接管请求。外部终端启动的 agent 也能从
    /// discovery 文件发现 broker，但那不代表此刻有 GUI 消费者；若直接入队，会让原 TUI
    /// 无提示等满五分钟。
    ///
    /// 窗口不存在：没有进行中的输入可打断，新开窗口直接落在该会话上，等消费者注册。
    /// 窗口已存在：**同样切到该会话**并闪任务栏。曾经这里只闪不切（怕打断用户在别的会话
    /// 里的输入），代价是「窗口开在会话 A、会话 B 来要审批」这种常态下用户得在 10s 内自己
    /// 注意到闪烁并点开 B，否则请求被撤回、答成 pass 退回 TUI——那 10s 是按 WebView2 冷启动
    /// 量的，不是按人的反应时间量的。取舍已经翻面：宁可切走一次，也不要让审批悄悄回落。
    /// 前端的侧栏授权徽标（approvalAwaitingIds）随之退居兜底：它覆盖切换竞态、以及消费者
    /// 已注册在别的会话时到达的请求。
    ///
    /// 两个分支都只做**有界**等待，等不到就返回 false 让调用方撤回请求并答 `pass`，
    /// 提示回落到 agent 自己的审批界面。已存在的窗口尤其不能无界等（此前直接返回
    /// true）：窗口可能最小化、侧栏收起时徽标无处渲染，而 hook 正压着 TUI 自己的面板
    /// ——handle_approval 会盲等满 300s，期间哪儿都看不到提示。
    fn ensure_approval_window(&self, session_id: i64) -> bool {
        if self.has_approval_consumer(session_id) {
            return true;
        }
        let Some(app) = self.attach.app.lock().ok().and_then(|app| app.clone()) else {
            return false;
        };
        // 无论窗口在不在，都要把它切到**这个**会话上：注册审批消费者的是对话页那套
        // useEffect，只有会话切过去了才会注册。曾经在窗口已存在时只闪一下任务栏就干等，
        // 于是「窗口开在会话 A、会话 B 来要审批」这种常态下，用户得在 10 秒内自己注意到
        // 闪烁、在侧栏找到 B 并点开，否则请求被撤回、答成 pass 退回 TUI——那 10 秒是按
        // WebView2 冷启动量的，不是按人的反应时间量的。
        crate::window::open_chat_window_detached(app.clone(), session_id);
        if let Some(window) = app.get_webview_window("chat") {
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
        // 等前端完成 session 切换并显式注册。窗口可见只能证明 WebView 存在，不能证明它已监听
        // pending-approval；以消费者租约为准，避免请求落在两个 useEffect 之间。
        // 期限 10s：首次 WebView2 冷启动（内核初始化 + bundle 加载 + React 挂载 + 注册 IPC）
        // 实测可超 2s；等待期间请求已在 approvals 表里，注册完成的窗口靠轮询也能立刻取到，
        // 不会出现「窗口弹出却空无一物」。占的是本连接自己的 handler 线程，不挤别人。
        for _ in 0..400 {
            if self.has_approval_consumer(session_id) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    pub(crate) fn start(
        &self,
        app: tauri::AppHandle,
        session_id: i64,
        argv: &[String],
        cwd: Option<&str>,
        env: &[(String, String)],
        terminal_size: TerminalSize,
    ) -> Result<(), String> {
        if argv.is_empty() {
            return Err("该 Agent 不支持恢复会话".into());
        }
        // 锁内只做「查重 + 登记启动占位」便立即放锁，openpty+spawn 移到锁外：冷启动叠加
        // 杀软扫描时 spawn 可达数秒，而 snapshot/write/resize/stop 都是主线程上的同步
        // Tauri 命令，持锁跨过 spawn 会让它们全部排队——一个会话冷启动卡顿就冻结整应用。
        // 已在运行或已有占位（另一个 start 正在锁外 spawn）→ 按重复启动收敛，与原先
        // 持锁排队后看到 contains 的语义一致。
        if !self.begin_start(session_id)? {
            return Ok(());
        }
        let result = self.start_spawned(app, session_id, argv, cwd, env, terminal_size);
        if result.is_err() {
            // 启动失败清占位，重试才进得来；completed 快照已在 begin_start 摘掉，与旧行为一致。
            self.end_start(session_id);
        }
        result
    }

    /// 登记「启动中」占位。contains 检查与占位插入在同一锁程内原子完成，两个并发 start
    /// 只有一个拿得到占位。锁序约定：**starting → sessions → completed**（全代码库唯一
    /// 嵌套持锁处；其余路径都只单锁，构成不了 ABBA）。
    /// 不变量：本函数返回后 completed[sid] 必为空，之后出现的条目一定来自本次调用之后的
    /// finalize。判重提前返回的分支同样要摘：start 按 Ok 收敛后调用方照常跑秒退探测（只凭
    /// completed 判断「起没起来」），上一代退出的定格快照会被误读成本次启动秒退——当前
    /// 这一代（运行中/启动中）尚未 finalize，completed 里的任何条目对该探测都是噪音。
    fn begin_start(&self, session_id: i64) -> Result<bool, String> {
        let mut starting = self.starting.lock().map_err(|_| "PTY 状态锁已损坏")?;
        let sessions = self.sessions.lock().map_err(|_| "PTY 状态锁已损坏")?;
        let duplicate = sessions.contains_key(&session_id) || !starting.insert(session_id);
        if let Ok(mut completed) = self.completed.lock() {
            completed.remove(&session_id);
        }
        Ok(!duplicate)
    }

    /// 摘掉启动占位（成功在登记入表之后、失败在收尾之后调用）。
    fn end_start(&self, session_id: i64) {
        if let Ok(mut starting) = self.starting.lock() {
            starting.remove(&session_id);
        }
    }

    /// spawn 完成后的登记。shutdown 先置 `shutting_down` 再抢同一把锁 drain，故在锁内复核：
    /// 复核看到的若是已置位，说明 drain 已结束，登记进去就是没人收尾的孤儿——调用方当场收尾。
    fn register_spawned(&self, session_id: i64, managed: &Arc<ManagedPty>) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|_| "PTY 状态锁已损坏")?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("应用正在退出，放弃登记新会话".into());
        }
        sessions.insert(session_id, managed.clone());
        Ok(())
    }

    /// start 的锁外段：openpty → spawn → 登记 → 起 waiter/reader 线程。
    /// 调用方持有 starting 占位；本函数任一步失败都由调用方清占位。
    fn start_spawned(
        &self,
        app: tauri::AppHandle,
        session_id: i64,
        argv: &[String],
        cwd: Option<&str>,
        env: &[(String, String)],
        terminal_size: TerminalSize,
    ) -> Result<(), String> {
        let pair = native_pty_system()
            .openpty(size(terminal_size.cols, terminal_size.rows))
            .map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new(&argv[0]);
        command.args(&argv[1..]);
        if let Some(cwd) = cwd.filter(|c| !c.trim().is_empty()) {
            command.cwd(cwd);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        // 所有托管会话（新建和恢复）都必须把本机鉴权通道传给 hook 子进程；此前只有
        // start_pending 注入，导致历史会话恢复后 PermissionRequest 无法抵达 GUI。
        if let Ok(endpoint) = self.attach.endpoint.lock() {
            if let Some(endpoint) = *endpoint {
                command.env("MEOWO_PTY_ENDPOINT", endpoint.to_string());
                command.env("MEOWO_PTY_TOKEN", &self.attach.token);
                command.env("MEOWO_PTY_PROTOCOL", CURRENT_PROTOCOL_VERSION.to_string());
            }
        }
        // 终端归一化:GUI 的托管 PTY 是给人看的全彩 xterm.js 渲染面,颜色必须可用。TERM
        // 之外还得管颜色开关——meowo 常从设了 NO_COLOR 的上游派生(典型:被某个 agent 的
        // "捕获输出用"子环境启动,那里为拿干净输出会置 NO_COLOR=1),CommandBuilder 默认
        // 继承整份父环境,这个 no-color.org 的业界停用信号就一路带进子 agent,Claude Code
        // 等 CLI 见之即把颜色全数剥掉。显式摘掉继承来的 NO_COLOR,再补上正向信号,让托管
        // 终端的着色只由这里说了算,不受外部环境摆布。FORCE_COLOR 兜住 ConPTY 上 isatty
        // 偶发探测不到 tty 的情况;真实色深由 COLORTERM/TERM 抬到 truecolor。
        command.env_remove("NO_COLOR");
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("FORCE_COLOR", "1");

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        drop(pair.slave);
        // 容量 128 × 单次 ≤64KB：正常按键/粘贴远用不满；持续塞满只可能是子进程长时间不读 stdin。
        let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(128);
        let managed = Arc::new(ManagedPty {
            session_id: AtomicI64::new(session_id),
            master: Mutex::new(Some(pair.master)),
            input_tx,
            child: Mutex::new(child),
            backlog: Mutex::new(VecDeque::new()),
            output_end: AtomicU64::new(0),
            subscribers: Mutex::new(Vec::new()),
            finalized: AtomicBool::new(false),
        });
        // writer 线程：唯一直接触碰 ConPTY 输入管道的地方。写失败（管道断）即退出；
        // ManagedPty 被收尾丢弃后 tx 断开，recv 出错线程随之结束。它若卡死在一次
        // write 上，就与 reader 同等待遇——带着句柄躺着，收尾从不等它。
        std::thread::spawn(move || {
            while let Ok(chunk) = input_rx.recv() {
                if writer
                    .write_all(&chunk)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    return;
                }
            }
        });
        if let Err(error) = self.register_spawned(session_id, &managed) {
            // 应用正在退出：shutdown 的 drain 已结束，这个会话塞进去也没人收尾——当场按
            // drain 的同等待遇杀掉子进程并释放伪终端（否则 Windows 上 conhost 孤儿化）。
            if let Ok(mut child) = managed.child.lock() {
                let _ = child.kill();
            }
            if let Ok(mut master) = managed.master.lock() {
                drop(master.take());
            }
            return Err(error);
        }
        // 先入表、再摘占位：两步之间不留「既不在表也无占位」的空窗，并发 start 漏不进来。
        self.end_start(session_id);

        // waiter：收尾的主触发器。Windows ConPTY 在子进程退出后不给 reader EOF（本机实证，
        // 连 drop master 都唤不醒阻塞中的 read），所以收尾绝不能挂在 reader 上——这里轮询
        // try_wait，进程一退就直接执行 finalize_exit。
        // 轮询而非阻塞 wait：wait 要一直握着 child 锁，stop() 的 kill 会和它死锁。
        let waiter = managed.clone();
        let waiter_broker = self.clone();
        let waiter_app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if waiter.finalized.load(Ordering::Acquire) {
                return;
            }
            let exited = waiter
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok())
                .flatten()
                .is_some();
            if exited {
                finalize_exit(&waiter_broker, &waiter_app, &waiter);
                return;
            }
        });

        // pty-output 合帧：reader 每读 16KB 就直发一条事件的话，构建/日志刷屏时每秒数百次
        // 「序列化 → 主线程事件循环 → WebView2 IPC → JS」会把整个界面拖卡。专职 emitter
        // 线程把一帧（16ms）内到达的 chunk 聚成一条事件再发；交互场景（距上次 emit 已超过
        // 一帧）走快路径立即发出，不给按键回显加可感知延迟。
        // 有界通道 + 阻塞 send：宁可反压 reader（等效于子进程输出慢一点），不丢终端字节——
        // 前端 xterm 按 offset 对齐增量渲染，事件流缺一段就得等 snapshot 重对齐。
        let (emit_tx, emit_rx) = mpsc::sync_channel::<(u64, Vec<u8>)>(64);
        let emitter_app = app.clone();
        let emitter_managed = managed.clone();
        std::thread::spawn(move || {
            const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
            // 单帧上限：重输出时一帧最多聚 256KB，base64 后 ~341KB，别让单条事件无限膨胀。
            const MAX_FRAME_BYTES: usize = 256 * 1024;
            let mut last_emit = std::time::Instant::now() - FRAME;
            while let Ok((offset, mut frame)) = emit_rx.recv() {
                // 距上次 emit 不足一帧才聚合；chunk 偏移天然连续（单一 reader 顺序投递），
                // 聚合帧仍以首 chunk 的 offset 对齐。
                let frame_end = last_emit + FRAME;
                while frame.len() < MAX_FRAME_BYTES {
                    let now = std::time::Instant::now();
                    if now >= frame_end {
                        break;
                    }
                    // Timeout/Disconnected 都先把手头的发出去；断开由外层 recv 收尾退出。
                    match emit_rx.recv_timeout(frame_end - now) {
                        Ok((_, more)) => frame.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }
                // 先确认对话窗存在再构造 payload：窗口关着时 base64 全是白做的。
                if let Some(window) = emitter_app.get_webview_window("chat") {
                    let payload = PtyOutput {
                        session_id: emitter_managed.session_id.load(Ordering::Acquire),
                        offset,
                        data: base64::engine::general_purpose::STANDARD.encode(&frame),
                    };
                    let _ = window.emit("pty-output", &payload);
                }
                last_emit = std::time::Instant::now();
            }
        });

        let broker = self.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            // 启动探测代答必须在 reader 单趟流上做(见 StartupProbeScanner):任何基于
            // 快照/回放的重扫都可能把同一个查询答第二遍,多出的应答会落进 agent 输入框。
            let mut probe_scanner = StartupProbeScanner::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // 首帧前:扫描并把已代答的探测从流中摘除(理由见 StartupProbeScanner:
                        // 下游 DsrFilter/xterm 看不到已答的查询就不可能再答第二遍),疑似探测
                        // 前缀暂存到下一读。首帧后:零拷贝直通,不再产生任何暂存。
                        let (data, probes): (std::borrow::Cow<'_, [u8]>, usize) =
                            if probe_scanner.painted() {
                                (std::borrow::Cow::Borrowed(&buf[..n]), 0)
                            } else {
                                let (cleaned, probes) = probe_scanner.feed(&buf[..n]);
                                (std::borrow::Cow::Owned(cleaned), probes)
                            };
                        // try_send 不阻塞 reader;队列满(128 条积压)说明子进程根本不读
                        // 输入,丢一条代答无关大局——那种进程连首帧都不会有人等到。
                        for _ in 0..probes {
                            let _ = managed.input_tx.try_send(b"\x1b[1;1R".to_vec());
                        }
                        // 整个 chunk 都被摘除/暂存(如恰好只有一条探测):没有字节要分发。
                        if data.is_empty() {
                            continue;
                        }
                        // 分发必须发生在追加 backlog 的同一把锁内：handle_attach 在一个
                        // backlog→subscribers 临界区里「回放 + 注册订阅者」，若这里先放掉
                        // backlog 锁再单独锁 subscribers，恰在缝隙注册的订阅者会把同一个
                        // chunk 从回放和通道各收一次——外部终端上屏重复的原始 ANSI。
                        // send 是无界通道的非阻塞投递，双锁临界区不会久持。
                        let send_chunk = |data: &[u8]| {
                            if let Ok(mut subscribers) = managed.subscribers.lock() {
                                let chunk = data.to_vec();
                                subscribers
                                    .retain(|subscriber| subscriber.tx.send(chunk.clone()).is_ok());
                            }
                        };
                        let offset = if let Ok(mut backlog) = managed.backlog.lock() {
                            let offset = managed.output_end.load(Ordering::Relaxed);
                            backlog.extend(data.iter().copied());
                            while backlog.len() > BACKLOG_LIMIT {
                                backlog.pop_front();
                            }
                            managed
                                .output_end
                                .store(offset + data.len() as u64, Ordering::Release);
                            send_chunk(&data);
                            offset
                        } else {
                            let offset = managed
                                .output_end
                                .fetch_add(data.len() as u64, Ordering::AcqRel);
                            send_chunk(&data);
                            offset
                        };
                        // 对话窗的实时帧交 emitter 合帧后发出（见上），backlog/订阅者不受影响。
                        let _ = emit_tx.send((offset, data.into_owned()));
                    }
                }
            }
            // reader 退出（EOF/出错）时 emit_tx 随闭包 drop，emitter 发完残余后自行结束。
            finalize_exit(&broker, &app, &managed);
        });
        Ok(())
    }

    /// 在真实 agent session id 尚未产生前启动 PTY。hook 继承的一次性 token 会在首次落库后
    /// 通过 loopback 服务把这个负数临时 id 原子替换成数据库 id。
    pub(crate) fn start_pending(
        &self,
        app: tauri::AppHandle,
        argv: &[String],
        cwd: Option<&str>,
        env: &[(String, String)],
        cols: u16,
        rows: u16,
    ) -> Result<i64, String> {
        let endpoint = self
            .attach
            .endpoint
            .lock()
            .map_err(|_| "attach 状态锁已损坏")?
            .ok_or("attach 服务未启动")?;
        let launch_token = random_token();
        let temp_id = self.attach.next_pending.fetch_sub(1, Ordering::Relaxed);
        self.attach
            .pending
            .lock()
            .map_err(|_| "PTY 临时会话锁已损坏")?
            .insert(launch_token.clone(), temp_id);
        let mut launch_env = env.to_vec();
        launch_env.extend([
            ("MEOWO_PTY_ENDPOINT".into(), endpoint.to_string()),
            ("MEOWO_PTY_TOKEN".into(), self.attach.token.clone()),
            ("MEOWO_PTY_LAUNCH".into(), launch_token.clone()),
            (
                "MEOWO_PTY_PROTOCOL".into(),
                CURRENT_PROTOCOL_VERSION.to_string(),
            ),
        ]);
        if let Err(error) = self.start(
            app,
            temp_id,
            argv,
            cwd,
            &launch_env,
            TerminalSize::new(cols, rows),
        ) {
            if let Ok(mut pending) = self.attach.pending.lock() {
                pending.remove(&launch_token);
            }
            return Err(error);
        }
        Ok(temp_id)
    }

    pub(crate) fn write(&self, session_id: i64, data: &[u8]) -> Result<(), String> {
        if data.len() > 64 * 1024 {
            return Err("单次 PTY 输入过大".into());
        }
        let session = self
            .sessions
            .lock()
            .map_err(|_| "PTY 状态锁已损坏")?
            .get(&session_id)
            .cloned()
            .ok_or("PTY 会话未运行")?;
        // 有界等待入队，绝不直接写管道（理由见 ManagedPty::input_tx）。到点仍满说明
        // 子进程长时间不读 stdin（挂死/被暂停），报错比无限阻塞调用线程诚实。
        let mut chunk = data.to_vec();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            chunk = match session.input_tx.try_send(chunk) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => return Err("PTY 输入通道已关闭".into()),
                Err(mpsc::TrySendError::Full(chunk)) => chunk,
            };
            if std::time::Instant::now() >= deadline {
                return Err("Agent 未在读取输入，输入已积压，请稍后重试".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    pub(crate) fn resize(&self, session_id: i64, cols: u16, rows: u16) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| "PTY 状态锁已损坏")?
            .get(&session_id)
            .cloned()
            .ok_or("PTY 会话未运行")?;
        let result = session
            .master
            .lock()
            .map_err(|_| "PTY 尺寸锁已损坏")?
            .as_ref()
            .ok_or("PTY 已结束")?
            .resize(size(cols, rows))
            .map_err(|e| e.to_string());
        result
    }

    /// 取会话输出快照。`since` 是调用方已持有的输出末尾偏移（首次传 0）——只返回它之后的
    /// 新字节，避免每次轮询都把整个 backlog（上限 1 MiB）拷贝 + base64 + 过 IPC 传一遍。
    ///
    /// `since` 落在 backlog 起点之前（被裁剪掉了）时返回现存的全部 backlog；晚于当前末尾
    /// （会话换了/被重置）时退化为空增量，**不会**自动回退成全量。调用方一律按响应里的
    /// start_offset/end_offset 对齐，并在重启 PTY 后把自己的 since 归零。
    pub(crate) fn snapshot(&self, session_id: i64, since: u64) -> PtySnapshot {
        let session = self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(&session_id).cloned());
        let active = session.as_ref().and_then(|s| {
            // 临界区内只做区间计算与切片拷贝：逐字节遍历整个 ring 会阻塞 PTY reader 线程写入。
            s.backlog.lock().ok().map(|b| {
                let end = s.output_end.load(Ordering::Acquire);
                let start = end.saturating_sub(b.len() as u64);
                let skip = since.saturating_sub(start).min(b.len() as u64);
                // since 超前于 end（会话被重置）时 skip 会被夹到 len，退化为空增量；
                // 此时 start_offset == end_offset，前端据此识别并重新对齐。
                let data: Vec<u8> = b.iter().skip(skip as usize).copied().collect();
                (data, start + skip, end)
            })
        });
        let completed = if session.is_none() {
            self.completed
                .lock()
                .ok()
                .and_then(|items| items.get(&session_id).cloned())
        } else {
            None
        };
        let (data, start_offset, end_offset) = if let Some(item) = completed.as_ref() {
            // 已退出的会话：completed 是定格快照，同样按 since 裁剪。
            let skip = since
                .saturating_sub(item.start_offset)
                .min(item.data.len() as u64);
            (
                item.data[skip as usize..].to_vec(),
                item.start_offset + skip,
                item.end_offset,
            )
        } else if let Some((data, start, end)) = active {
            (data, start, end)
        } else {
            (Vec::new(), 0, 0)
        };
        PtySnapshot {
            session_id,
            active: session.is_some(),
            data: base64::engine::general_purpose::STANDARD.encode(data),
            start_offset,
            end_offset,
            exited: completed.is_some(),
            exit_code: completed.and_then(|item| item.code),
        }
    }

    /// 当前有活跃托管 PTY 的会话集合。存活校正用:hook 尚未认领 pid(如 codex 首回合前)
    /// 或 120s 事件宽限过期时,meowo 自己 spawn 的 agent 也必须算「已连接」——PTY 在即进程在。
    /// 不含 starting 占位(与 snapshot 的 active 口径一致:spawn 完成前按未运行处理)。
    pub(crate) fn active_session_ids(&self) -> HashSet<i64> {
        self.sessions
            .lock()
            .map(|sessions| sessions.keys().copied().collect())
            .unwrap_or_default()
    }

    /// 单会话版 [`Self::active_session_ids`](对话窗轮询按会话取,不必整表拷贝)。
    pub(crate) fn is_active(&self, session_id: i64) -> bool {
        self.sessions
            .lock()
            .map(|sessions| sessions.contains_key(&session_id))
            .unwrap_or(false)
    }

    pub(crate) fn stop(&self, session_id: i64) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| "PTY 状态锁已损坏")?
            .get(&session_id)
            .cloned()
            .ok_or("PTY 会话未运行")?;
        let result = session
            .child
            .lock()
            .map_err(|_| "PTY 进程锁已损坏")?
            .kill()
            .map_err(|e| e.to_string());
        // kill 之后收尾由 waiter 在 ~200ms 内接手（finalize_exit）；这里不用做别的。
        result
    }

    /// 启动仅监听 loopback 的 attach 服务。协议不暴露到 LAN，且握手必须携带 256-bit token。
    pub(crate) fn start_attach_server(&self) -> Result<(), String> {
        if self.attach.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) => {
                self.attach.started.store(false, Ordering::Release);
                return Err(error.to_string());
            }
        };
        let endpoint = listener.local_addr().map_err(|e| e.to_string())?;
        *self
            .attach
            .endpoint
            .lock()
            .map_err(|_| "attach 状态锁已损坏")? = Some(endpoint);
        // 外部终端没有托管 PTY 注入的环境变量。把仅监听 loopback 的端点和随机 token
        // 登记到当前用户的数据目录，让同一用户启动的 reporter 也能把审批转交 GUI。
        // `pid` 是这份登记的有效性凭据：正常退出时 `shutdown` 会删文件，但崩溃时删不掉，
        // 而端口可能已被无关进程回收——reporter 必须靠 pid 判活来识别陈旧文件（见 attach.rs）。
        #[cfg(not(test))]
        if let Some(dir) = crate::db_path().parent() {
            let discovery = BrokerDiscovery {
                endpoint: endpoint.to_string(),
                token: self.attach.token.clone(),
                pid: std::process::id(),
                protocol_version: CURRENT_PROTOCOL_VERSION,
            };
            if let Ok(json) = serde_json::to_vec(&discovery) {
                let _ = std::fs::create_dir_all(dir);
                let path = dir.join(APPROVAL_BROKER_FILE);
                // 写失败要留痕：agent 侧的表现只是「审批默默回到 TUI」，没有日志根本查不到磁盘满/无权限。
                if let Err(error) = std::fs::write(&path, json) {
                    eprintln!(
                        "审批 broker discovery 文件写入失败（外部终端的审批将回落 TUI）: {error}"
                    );
                }
                // token 等于 PTY 完全接管权。父目录权限已经挡住他人，这里再收紧到 0600 作纵深防御。
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
        let broker = self.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let broker = broker.clone();
                std::thread::spawn(move || {
                    let _ = broker.handle_attach(stream);
                });
            }
        });
        Ok(())
    }

    /// GUI 退出时的清理。不做的话：(1) discovery 文件残留，下一个 reporter 会连向一个
    /// 早已不属于我们的端口；(2) 托管 PTY 的子进程被孤儿化（Windows 上 conhost 一并残留）。
    pub(crate) fn shutdown(&self) {
        // 先置位再抢锁 drain：登记路径（register_spawned）在同一把锁内复核这个标志，
        // 置位之后完成的 spawn 会被拒并当场收尾，drain 之后不会有新会话混进表里。
        // Release：纯 store 不能用 AcqRel（那是给读改写的）。这里要的正是「置位对随后
        // 拿到同一把锁的线程可见」，Release 与登记路径的 Acquire 读配对即可。
        self.shutting_down.store(true, Ordering::Release);
        #[cfg(not(test))]
        if let Some(dir) = crate::db_path().parent() {
            let _ = std::fs::remove_file(dir.join(APPROVAL_BROKER_FILE));
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, managed) in sessions.drain() {
                if let Ok(mut child) = managed.child.lock() {
                    let _ = child.kill();
                }
                // 同 stop：不关伪终端的话 conhost 会作为孤儿留在系统里。
                if let Ok(mut master) = managed.master.lock() {
                    drop(master.take());
                }
            }
        }
    }

    /// 该会话此刻是否由 Meowo 的 PTY 持有。
    pub(crate) fn is_managed(&self, session_id: i64) -> bool {
        self.sessions
            .lock()
            .is_ok_and(|sessions| sessions.contains_key(&session_id))
    }

    /// 会话已退出时，取退出码与输出尾部的可读文本（诊断用）。仍在运行/从未运行 → None。
    /// CLI 拒绝启动（如 resume 一个正被占用的会话）时，原因只存在于这段输出里。
    pub(crate) fn exit_info(&self, session_id: i64) -> Option<(Option<u32>, String)> {
        let completed = self.completed.lock().ok()?.get(&session_id).cloned()?;
        Some((completed.code, readable_tail(&completed.data, 240)))
    }

    /// 该会话累计产出的输出字节数（仍在运行时）。读一个原子量，可用于高频轮询。
    /// 秒退探测用它当「进程确实活起来了」的信号，好提前结束等待。
    pub(crate) fn output_len(&self, session_id: i64) -> u64 {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| {
                sessions
                    .get(&session_id)
                    .map(|s| s.output_end.load(Ordering::Acquire))
            })
            .unwrap_or(0)
    }

    /// 临时 id → 真实会话 id 的绑定结果。**只读不消费**：对话窗口会重复轮询，负 id 期间
    /// 还可能因 `key={sessionId}` 重挂而再读一次；一次性消费会让其中一方永远等不到真实 id。
    /// 绑定表随 PTY 退出清理（见 reader 线程），不会无限增长。
    pub(crate) fn binding(&self, temp_id: i64) -> Option<i64> {
        self.attach
            .bindings
            .lock()
            .ok()
            .and_then(|bindings| bindings.get(&temp_id).copied())
    }

    /// 该会话此刻是否有外部同步终端（attach 客户端）在线。
    ///
    /// 订阅表只在 handle_attach 里增删：客户端断开（外部终端窗口被关）时 read 循环 EOF、
    /// 同一函数尾部立即摘除订阅——「非空」与「确有活的外部视图」严格同步，可作
    /// 「再点卡片该聚焦已有窗口还是新开一个」的判据。对话窗口不经 socket 订阅，不计入。
    ///
    /// 目前仅 macOS 的 attach 查重调用（Windows 刻意不做，见 attach_in_external_terminal）；
    /// 逻辑平台无关，留在 cfg 外全平台编译与单测。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn has_external_viewer(&self, session_id: i64) -> bool {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| {
                sessions.get(&session_id).map(|session| {
                    session
                        .subscribers
                        .lock()
                        .map(|subscribers| !subscribers.is_empty())
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// 该会话最近一个上报了自身 pid 的外部订阅者。查重命中后的激活目标从这里反查
    /// 实际宿主，绝不看「恢复终端」设置——设置与视图实际所在的应用可能不一致。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn external_viewer_pid(&self, session_id: i64) -> Option<u32> {
        self.sessions.lock().ok().and_then(|sessions| {
            sessions.get(&session_id).and_then(|session| {
                session
                    .subscribers
                    .lock()
                    .ok()
                    .and_then(|subscribers| {
                        subscribers.iter().rev().find_map(|subscriber| subscriber.pid)
                    })
            })
        })
    }

    /// attach 前置校验：会话确实由本进程的 PTY 持有，且 attach 服务已在监听。
    /// 刻意不返回 endpoint/token——它们经 discovery 文件（unix 下 0600）交给客户端，
    /// 不进外部终端的进程参数（argv 对同机其他进程可见，token 等于 PTY 完全接管权）。
    pub(crate) fn ensure_attachable(&self, session_id: i64) -> Result<(), String> {
        if !self
            .sessions
            .lock()
            .map_err(|_| "PTY 状态锁已损坏")?
            .contains_key(&session_id)
        {
            return Err("该会话尚未由 Meowo 接管".into());
        }
        self.attach
            .endpoint
            .lock()
            .map_err(|_| "attach 状态锁已损坏")?
            .ok_or("attach 服务未启动")?;
        Ok(())
    }

    /// 此刻 broker 手里还压着审批请求的会话（批量版，供看板/角标一次取齐）。
    ///
    /// 这是「agent 真的在等你批」的**实时**事实源：`PermissionRequest` hook 阻塞期间请求挂在
    /// 这儿，hook 一返回（放行 / 拒绝 / 交还终端）就没了。DB 里的 `pending_review` 则要等到
    /// 下一个 hook 事件（PostToolUse/Stop）才清——被放行的工具跑多久，那个标记就滞留多久。
    pub(crate) fn approval_session_ids(&self) -> HashSet<i64> {
        self.attach
            .approvals
            .lock()
            .map(|approvals| {
                approvals
                    .values()
                    .map(|pending| pending.request.session_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn pending_approval(&self, session_id: i64) -> Option<ApprovalRequest> {
        self.attach
            .approvals
            .lock()
            .ok()?
            .values()
            .find(|pending| pending.request.session_id == session_id)
            .map(|pending| pending.request.clone())
    }

    #[cfg(test)]
    pub(crate) fn resolve_approval(
        &self,
        session_id: i64,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        let mut approvals = self
            .attach
            .approvals
            .lock()
            .map_err(|_| "审批状态锁已损坏")?;
        if approvals
            .get(request_id)
            .ok_or("审批请求已结束")?
            .request
            .session_id
            != session_id
        {
            return Err("审批请求不属于该会话".into());
        }
        let pending = approvals.remove(request_id).expect("刚验证存在的审批请求");
        drop(approvals);
        pending
            .response
            .send(decision)
            .map_err(|_| "Agent 已不再等待审批".into())
    }

    pub(crate) fn resolve_approval_choice(
        &self,
        session_id: i64,
        request_id: &str,
        choice: &str,
    ) -> Result<(), String> {
        let mut approvals = self
            .attach
            .approvals
            .lock()
            .map_err(|_| "审批状态锁已损坏")?;
        let pending = approvals.get(request_id).ok_or("审批请求已结束")?;
        if pending.request.session_id != session_id {
            return Err("审批请求不属于该会话".into());
        }
        let decision = match choice {
            "allow_once" => ApprovalDecision::Allow,
            "deny" => ApprovalDecision::Deny,
            value if value.starts_with("suggestion:") => {
                let index = value["suggestion:".len()..]
                    .parse::<usize>()
                    .map_err(|_| "无效的审批选项")?;
                let suggestion = pending
                    .request
                    .permission_suggestions
                    .get(index)
                    .cloned()
                    .ok_or("审批选项已失效")?;
                ApprovalDecision::AllowWithPermissions(vec![suggestion])
            }
            _ => return Err("无效的审批选项".into()),
        };
        let pending = approvals.remove(request_id).expect("刚验证存在的审批请求");
        drop(approvals);
        pending
            .response
            .send(decision)
            .map_err(|_| "Agent 已不再等待审批".into())
    }

    /// 对话窗关闭后 GUI 已不再能消费审批；立即把所有挂起请求交还各自 Agent 的 TUI。
    pub(crate) fn pass_pending_approvals(&self) {
        let pending = self
            .attach
            .approvals
            .lock()
            .map(|mut approvals| approvals.drain().map(|(_, item)| item).collect::<Vec<_>>())
            .unwrap_or_default();
        for item in pending {
            let _ = item.response.send(ApprovalDecision::Pass);
        }
    }

    fn has_approval_consumer(&self, session_id: i64) -> bool {
        self.attach
            .approval_consumers
            .lock()
            .is_ok_and(|consumers| consumers.values().any(|session| *session == session_id))
    }

    pub(crate) fn register_approval_consumer(
        &self,
        session_id: i64,
        consumer_id: String,
    ) -> Result<(), String> {
        if session_id <= 0 || consumer_id.is_empty() || consumer_id.len() > 128 {
            return Err("审批消费者无效".into());
        }
        self.attach
            .approval_consumers
            .lock()
            .map_err(|_| "审批消费者状态锁已损坏".to_string())?
            .insert(consumer_id, session_id);
        Ok(())
    }

    /// 对话窗被销毁时的租约兜底。租约平时靠前端卸载时 unregister，但窗口销毁瞬间
    /// 那次 IPC 未必执行得到；残留租约会让 `ensure_approval_window` 误判「有 GUI
    /// 消费者」而把审批入队空等 300s，而不是立即交还 TUI。chat 窗是单例，
    /// 所有 consumer 都属于它，直接清空即可。
    pub(crate) fn clear_approval_consumers(&self) {
        if let Ok(mut consumers) = self.attach.approval_consumers.lock() {
            consumers.clear();
        }
    }

    pub(crate) fn unregister_approval_consumer(&self, consumer_id: &str) {
        let session_id = self
            .attach
            .approval_consumers
            .lock()
            .ok()
            .and_then(|mut consumers| {
                let session_id = consumers.remove(consumer_id)?;
                (!consumers.values().any(|session| *session == session_id)).then_some(session_id)
            });
        let Some(session_id) = session_id else { return };
        let pending = self
            .attach
            .approvals
            .lock()
            .map(|mut approvals| {
                let ids = approvals
                    .iter()
                    .filter(|(_, item)| item.request.session_id == session_id)
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>();
                ids.into_iter()
                    .filter_map(|id| approvals.remove(&id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for item in pending {
            let _ = item.response.send(ApprovalDecision::Pass);
        }
    }

    fn handle_attach(&self, mut stream: TcpStream) -> Result<(), String> {
        stream.set_nodelay(true).ok();
        // 握手必须限时：loopback 上任何本地进程都能 connect，不设读超时的话，连上后
        // 一言不发就永久占住一个 handler 线程（每连接一线程），反复建连即可耗尽线程数。
        // 认证通过进入转发模式后再放开（attach 空闲时本来就没有输入帧）。
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let handshake = read_handshake(&mut stream).map_err(|e| e.to_string())?;
        let BrokerRequest::Attach {
            token,
            session_id,
            cols,
            rows,
            nonce,
            pid: client_pid,
        } = handshake
        else {
            return match handshake {
                BrokerRequest::Claim {
                    token,
                    launch_token,
                    session_id,
                } => self.handle_claim(&token, &launch_token, session_id),
                BrokerRequest::Approval { token, request } => {
                    self.handle_approval(&token, request, stream)
                }
                BrokerRequest::Attach { .. } => unreachable!(),
            };
        };
        if token != self.attach.token {
            return Err("attach 认证失败".into());
        }
        // 第六段是客户端 nonce，当前只用于禁止旧五段协议被误接入。
        if nonce.len() < 8 {
            return Err("attach nonce 无效".into());
        }
        // 认证已通过——此后拒绝必须把原因写回给客户端再断开。客户端此时已进入
        // raw 转发模式，收到的字节会原样上屏；一言不发地 drop socket，对端只会
        // 得到一个 exit 0 的 EOF，用户面对的就是一扇纯空白的终端窗。
        let refuse = |mut stream: TcpStream, error: String| -> Result<(), String> {
            let _ = stream.write_all(format!("\r\nMeowo attach: {error}\r\n").as_bytes());
            Err(error)
        };
        // 外部终端从 spawn 到连上有秒级窗口，期间 SessionStart 的 claim 可能已把临时负 id
        // 重绑成真实 id；握手里带的还是旧 id，按绑定表翻译后再查，否则「新会话开在外部
        // 终端」会间歇性打开一扇只写着「PTY 会话未运行」的死窗口。
        let session_id = self.binding(session_id).unwrap_or(session_id);
        if let Err(error) = self.resize(session_id, cols, rows) {
            return refuse(stream, error);
        }
        let session = match self
            .sessions
            .lock()
            .map_err(|_| "PTY 状态锁已损坏".to_string())
            .and_then(|sessions| {
                sessions
                    .get(&session_id)
                    .cloned()
                    .ok_or_else(|| "PTY 会话未运行".to_string())
            }) {
            Ok(session) => session,
            Err(error) => return refuse(stream, error),
        };
        let subscriber_id = self.attach.next_subscriber.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        // 与 reader 的锁顺序一致：先 backlog 后 subscribers，确保回放与订阅之间没有输出缺口。
        let backlog = {
            let backlog = session.backlog.lock().map_err(|_| "PTY 回放锁已损坏")?;
            session
                .subscribers
                .lock()
                .map_err(|_| "PTY 订阅锁已损坏")?
                .push(AttachSubscriber {
                    id: subscriber_id,
                    pid: client_pid,
                    tx,
                });
            backlog.iter().copied().collect::<Vec<_>>()
        };
        // 回放给外部终端前滤掉历史查询(理由见 strip_dsr_queries):否则客户端 DsrFilter
        // 每次重开外部同步终端都会把它们再代答一遍。订阅之后的实时字节不过滤——
        // 新查询正是 DsrFilter 该答的。
        let backlog = strip_dsr_queries(&backlog);
        let mut output = stream.try_clone().map_err(|e| e.to_string())?;
        std::thread::spawn(move || {
            if output.write_all(&backlog).is_ok() {
                for chunk in rx {
                    if output.write_all(&chunk).is_err() {
                        break;
                    }
                }
            }
            // PTY 退出时 subscribers 随 ManagedPty 一起 drop；关闭 socket 唤醒客户端与服务端读循环。
            let _ = output.shutdown(Shutdown::Both);
        });

        stream.set_read_timeout(None).ok();
        // 出错也必须走下方的 subscriber 清理，不能 `?` 提前返回——残留的订阅项会让
        // 转发线程带着半开 socket 等到 finalize_exit 才被收走。
        let mut frame_error = None;
        loop {
            let mut header = [0u8; 5];
            if stream.read_exact(&mut header).is_err() {
                break;
            }
            let kind = header[0];
            let len = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
            if len > 64 * 1024 {
                break;
            }
            let mut payload = vec![0u8; len];
            if stream.read_exact(&mut payload).is_err() {
                break;
            }
            let current_id = session.session_id.load(Ordering::Acquire);
            let result = match kind {
                1 => self.write(current_id, &payload),
                2 if payload.len() == 4 => self.resize(
                    current_id,
                    u16::from_be_bytes([payload[0], payload[1]]),
                    u16::from_be_bytes([payload[2], payload[3]]),
                ),
                _ => break,
            };
            if let Err(error) = result {
                frame_error = Some(error);
                break;
            }
        }
        if let Ok(mut subscribers) = session.subscribers.lock() {
            subscribers.retain(|subscriber| subscriber.id != subscriber_id);
        }
        match frame_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// claim 的 sessions 锁内段：真实 id 已被占用 → Err；临时会话已登记 → 在同一锁程内
    /// 完成「取出 + 改 id + 重绑」，外部观察者看不到空窗；尚未登记 → Ok(None)，由调用方
    /// 决定等（启动占位还在）还是报错（真的已结束）。
    fn try_claim_rebind(
        &self,
        temp_id: i64,
        real_id: i64,
    ) -> Result<Option<Arc<ManagedPty>>, String> {
        let mut sessions = self.sessions.lock().map_err(|_| "PTY 状态锁已损坏")?;
        if sessions.contains_key(&real_id) {
            return Err("真实 PTY 会话已存在".into());
        }
        let Some(managed) = sessions.remove(&temp_id) else {
            return Ok(None);
        };
        managed.session_id.store(real_id, Ordering::Release);
        sessions.insert(real_id, managed.clone());
        Ok(Some(managed))
    }

    fn handle_claim(&self, token: &str, launch_token: &str, real_id: i64) -> Result<(), String> {
        if token != self.attach.token {
            return Err("PTY claim 认证失败".into());
        }
        if real_id <= 0 {
            return Err("PTY claim session 无效".into());
        }
        // 先只读 token，直到 sessions 重绑完成才消费。极快启动的 agent 可能在 start() 完成
        // 注册前就触发 hook；这时保留 token，下一次认领仍可成功。
        let temp_id = *self
            .attach
            .pending
            .lock()
            .map_err(|_| "PTY 临时会话锁已损坏")?
            .get(launch_token)
            .ok_or("PTY claim token 无效或已使用")?;
        // start 的 openpty+spawn 在锁外进行（冷启动+杀软扫描可达数秒），claim 又是一次性的
        // （reporter 不重试）——子进程已起、登记未落的窗口里绝不能按「已结束」把这次绑定
        // 错杀掉。占位还在就等它落地：占的是本连接自己的 handler 线程，不挤别人。
        let mut managed = None;
        for _ in 0..200 {
            if let Some(current) = self.try_claim_rebind(temp_id, real_id)? {
                managed = Some(current);
                break;
            }
            let still_starting = self
                .starting
                .lock()
                .map_err(|_| "PTY 状态锁已损坏")?
                .contains(&temp_id);
            if !still_starting {
                // 占位刚被摘掉：可能是登记落表（先 insert 后 remove，正常时上面一轮就该
                // 命中），也可能是启动失败清了占位——复查一次再下「已结束」的结论。
                managed = self.try_claim_rebind(temp_id, real_id)?;
                if managed.is_none() {
                    return Err("临时 PTY 会话已结束".into());
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        // 占位最长存留一个 spawn 周期；5s 还没落地按启动失败处理，token 保留供重认。
        let managed = managed.ok_or("PTY 启动登记超时")?;
        if let Ok(mut pending) = self.attach.pending.lock() {
            pending.remove(launch_token);
        }
        if let Ok(mut bindings) = self.attach.bindings.lock() {
            bindings.insert(temp_id, real_id);
        }
        // 保持 Arc 活到映射完成，避免极短命进程在重绑边界提前析构。
        drop(managed);
        Ok(())
    }

    fn handle_approval(
        &self,
        token: &str,
        request: ApprovalRequest,
        mut stream: TcpStream,
    ) -> Result<(), String> {
        if token != self.attach.token {
            return Err("审批通道认证失败".into());
        }
        if request.session_id <= 0 || request.request_id.len() < 8 {
            return Err("审批请求无效".into());
        }
        // 先入表再等窗口：冷启动的 WebView2 完成消费者注册可能晚于任何固定等待窗口。
        // 请求在表里，晚注册的窗口靠 getPendingApproval 轮询就能找回它；反过来（等到了
        // 才入表）则超时瞬间请求人间蒸发，用户面对一扇刚弹出的空窗口。
        let (tx, rx) = mpsc::channel();
        self.attach
            .approvals
            .lock()
            .map_err(|_| "审批状态锁已损坏")?
            .insert(
                request.request_id.clone(),
                PendingApproval {
                    request: request.clone(),
                    response: tx,
                },
            );
        if !self.ensure_approval_window(request.session_id) {
            // 窗口没起来：撤回请求，把决定权交还 agent 自己的审批界面。
            if let Ok(mut approvals) = self.attach.approvals.lock() {
                approvals.remove(&request.request_id);
            }
            self.emit_approval("pending-approval-cleared", &request);
            return stream.write_all(b"pass\n").map_err(|e| e.to_string());
        }
        self.emit_approval("pending-approval", &request);
        let decision = rx.recv_timeout(std::time::Duration::from_secs(300)).ok();
        if let Ok(mut approvals) = self.attach.approvals.lock() {
            approvals.remove(&request.request_id);
        }
        self.emit_approval("pending-approval-cleared", &request);
        let response = format!("{}\n", decision.unwrap_or(ApprovalDecision::Pass).as_wire());
        stream
            .write_all(response.as_bytes())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 注：曾有一个直接驱动 ConPTY 的实证测试，结论已固化到 finalize_exit / waiter 的注释里
    // （子进程退出后 reader 不 EOF；drop master 也唤不醒阻塞中的 read）。测试本身依赖
    // 测试环境里 ConPTY 的交互行为（cmd /c 甚至不退出），过于 flaky，不保留。

    #[test]
    fn readable_tail_strips_ansi_and_keeps_last_lines() {
        // 秒退诊断的原料是 TUI 首帧：清屏序列 + 标题 OSC + 真正的报错行。
        let raw = b"\x1b[2J\x1b[H\x1b]0;claude\x07noise line\r\nError: Session e9a is already in use\r\n\x1b[31mPlease close the other client.\x1b[0m\r\n";
        let tail = readable_tail(raw, 240);
        assert!(tail.contains("already in use"), "tail={tail}");
        assert!(tail.contains("Please close"), "tail={tail}");
        assert!(!tail.contains('\x1b'));
        // 中文按 UTF-8 完整解码，不得拆成乱码字节。
        let zh = readable_tail("错误：会话被占用\n".as_bytes(), 240);
        assert_eq!(zh, "错误：会话被占用");
        // 限长截断加省略号。
        let long = readable_tail(&[b'a'; 500], 10);
        assert_eq!(long, "aaaaaaaaaa…");
    }

    #[test]
    fn pty_size_is_clamped_to_safe_bounds() {
        let tiny = size(0, 1);
        assert_eq!((tiny.cols, tiny.rows), (2, 2));
        let huge = size(u16::MAX, u16::MAX);
        assert_eq!((huge.cols, huge.rows), (500, 500));
    }

    #[test]
    fn inactive_snapshot_is_empty_and_large_input_is_rejected() {
        let broker = PtyBroker::default();
        let snapshot = broker.snapshot(42, 0);
        assert!(!snapshot.active);
        assert!(!snapshot.exited);
        assert!(snapshot.data.is_empty());
        assert_eq!(snapshot.exit_code, None);
        assert!(broker.write(42, &vec![0; 64 * 1024 + 1]).is_err());
    }

    /// 最小可用的 ManagedPty：不碰真 PTY，供占位/登记/shutdown 的状态机测试。
    #[derive(Debug)]
    struct DummyChild;
    impl portable_pty::ChildKiller for DummyChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(DummyChild)
        }
    }
    impl portable_pty::Child for DummyChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(None)
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            Err(std::io::Error::other("dummy child never exits"))
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    fn dummy_managed(session_id: i64) -> Arc<ManagedPty> {
        // rx 直接丢弃：假会话没有 writer 线程，写入以 Disconnected 快速失败即可。
        let (input_tx, _) = mpsc::sync_channel(1);
        Arc::new(ManagedPty {
            session_id: AtomicI64::new(session_id),
            master: Mutex::new(None),
            finalized: AtomicBool::new(false),
            input_tx,
            child: Mutex::new(Box::new(DummyChild)),
            backlog: Mutex::new(VecDeque::new()),
            output_end: AtomicU64::new(0),
            subscribers: Mutex::new(Vec::new()),
        })
    }

    /// has_external_viewer 的口径：订阅表非空才算有外部视图；订阅被摘（窗口关闭）
    /// 或会话不存在都判 false——判 true 会让「点卡片」只激活应用而不开新视图，误判即失联。
    #[test]
    fn external_viewer_presence_follows_the_subscriber_table() {
        let broker = PtyBroker::default();
        assert!(!broker.has_external_viewer(7), "无此会话必须判 false");
        let managed = dummy_managed(7);
        broker.sessions.lock().unwrap().insert(7, managed.clone());
        assert!(!broker.has_external_viewer(7), "无订阅必须判 false");
        let (tx, _rx) = mpsc::channel::<Vec<u8>>();
        managed.subscribers.lock().unwrap().push(AttachSubscriber {
            id: 1,
            pid: None,
            tx,
        });
        assert!(broker.has_external_viewer(7));
        managed.subscribers.lock().unwrap().clear();
        assert!(!broker.has_external_viewer(7), "订阅摘除后必须回落 false");
    }

    /// 去重后的激活目标必须指向真实宿主：pid 从订阅表反查，取最近注册且上报了 pid 的
    /// 订阅者；无订阅或全部未上报（旧 reporter）→ None，调用方退回应用级兜底。
    #[test]
    fn external_viewer_pid_follows_the_most_recent_reporting_subscriber() {
        let broker = PtyBroker::default();
        assert_eq!(broker.external_viewer_pid(7), None, "无此会话必须判 None");
        let managed = dummy_managed(7);
        broker.sessions.lock().unwrap().insert(7, managed.clone());
        assert_eq!(broker.external_viewer_pid(7), None, "无订阅必须判 None");
        let (tx1, _rx1) = mpsc::channel::<Vec<u8>>();
        managed.subscribers.lock().unwrap().push(AttachSubscriber {
            id: 1,
            pid: None,
            tx: tx1,
        });
        assert_eq!(
            broker.external_viewer_pid(7),
            None,
            "旧 reporter 未上报 pid → None"
        );
        let (tx2, _rx2) = mpsc::channel::<Vec<u8>>();
        managed.subscribers.lock().unwrap().push(AttachSubscriber {
            id: 2,
            pid: Some(4242),
            tx: tx2,
        });
        let (tx3, _rx3) = mpsc::channel::<Vec<u8>>();
        managed.subscribers.lock().unwrap().push(AttachSubscriber {
            id: 3,
            pid: Some(5353),
            tx: tx3,
        });
        assert_eq!(
            broker.external_viewer_pid(7),
            Some(5353),
            "取最近注册的上报者"
        );
    }

    #[test]
    fn starting_placeholder_suppresses_duplicate_starts_until_cleared() {
        let broker = PtyBroker::default();
        assert!(broker.begin_start(7).unwrap());
        // 占位期间第二个 start 必须被判为重复（contains 检查与占位插入在同一锁程内原子完成）。
        assert!(!broker.begin_start(7).unwrap());
        broker.end_start(7);
        // 启动失败清掉占位后，重试必须能重新登记。
        assert!(broker.begin_start(7).unwrap());
        broker.end_start(7);
        // 已在运行的会话同样压住新占位（走 sessions.contains 分支）。
        broker.sessions.lock().unwrap().insert(8, dummy_managed(8));
        assert!(!broker.begin_start(8).unwrap());
    }

    /// 判重提前返回同样要摘掉 completed 残留：start 按 Ok 收敛后，调用方的秒退探测只凭
    /// completed 判断「起没起来」，上一代退出的定格快照会被误报成本次启动秒退。
    #[test]
    fn duplicate_start_clears_the_stale_completed_snapshot() {
        let broker = PtyBroker::default();
        let stale = || CompletedPty {
            data: b"old".to_vec(),
            start_offset: 0,
            end_offset: 3,
            code: Some(1),
            seq: 0,
        };
        // sessions.contains 分支（会话仍在运行，completed 里躺着上一代的退出快照）。
        broker.sessions.lock().unwrap().insert(7, dummy_managed(7));
        broker.completed.lock().unwrap().insert(7, stale());
        assert!(!broker.begin_start(7).unwrap());
        assert!(broker.exit_info(7).is_none(), "残留快照必须被摘掉");
        // starting 占位分支（另一个 start 正在锁外 spawn；finalize 先插 completed 再摘
        // sessions 的间隙里，completed 也可能出现当前占位之前的写入）。
        broker.begin_start(8).unwrap();
        broker.completed.lock().unwrap().insert(8, stale());
        assert!(!broker.begin_start(8).unwrap());
        assert!(broker.exit_info(8).is_none());
        broker.end_start(8);
    }

    #[test]
    fn a_starting_session_reads_as_not_yet_running() {
        let broker = PtyBroker::default();
        broker.begin_start(7).unwrap();
        // snapshot：inactive 且非 exited 的空帧——与启动前一致，前端按既有「未连接」路径
        // 处理，绝不会把启动中的会话误判成已退出（completed 已在登记占位时摘掉）。
        let snapshot = broker.snapshot(7, 0);
        assert!(!snapshot.active);
        assert!(!snapshot.exited);
        assert!(snapshot.data.is_empty());
        // write/resize/stop 快速失败，不在启动中的会话上排队等待。
        assert!(broker.write(7, b"x").is_err());
        assert!(broker.resize(7, 80, 24).is_err());
        assert!(broker.stop(7).is_err());
        assert!(!broker.is_managed(7));
        broker.end_start(7);
    }

    #[test]
    fn claim_waits_for_the_inflight_start_to_register() {
        let broker = PtyBroker::default();
        broker
            .attach
            .pending
            .lock()
            .unwrap()
            .insert("launch".into(), -5);
        broker.begin_start(-5).unwrap();
        let token = broker.attach.token.clone();

        // 模拟 start 的锁外段：spawn 完成后登记入表、再摘占位。
        let registrar = broker.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            registrar
                .sessions
                .lock()
                .unwrap()
                .insert(-5, dummy_managed(-5));
            registrar.end_start(-5);
        });

        // claim 一次性、reporter 不重试：登记未落的窗口里必须等，不能按「已结束」错杀。
        broker.handle_claim(&token, "launch", 9).unwrap();
        handle.join().unwrap();
        let sessions = broker.sessions.lock().unwrap();
        assert!(sessions.contains_key(&9));
        assert!(!sessions.contains_key(&-5));
        drop(sessions);
        assert_eq!(broker.binding(-5), Some(9));
        assert!(broker
            .attach
            .pending
            .lock()
            .unwrap()
            .get("launch")
            .is_none());
    }

    #[test]
    fn claim_after_a_failed_start_keeps_its_token() {
        let broker = PtyBroker::default();
        broker
            .attach
            .pending
            .lock()
            .unwrap()
            .insert("launch".into(), -5);
        broker.begin_start(-5).unwrap();
        broker.end_start(-5); // 启动失败：清占位、不入表
        let token = broker.attach.token.clone();
        assert!(broker.handle_claim(&token, "launch", 9).is_err());
        // token 不消费：早到/迟到的 claim 都不得断送下一次认领（原有语义保持）。
        assert_eq!(
            broker.attach.pending.lock().unwrap().get("launch"),
            Some(&-5)
        );
    }

    #[test]
    fn registration_after_shutdown_is_rejected() {
        let broker = PtyBroker::default();
        broker.shutdown();
        // drain 结束后完成的 spawn 必须登记失败（调用方当场收尾），不能混进表里孤儿化。
        assert!(broker.register_spawned(7, &dummy_managed(7)).is_err());
        assert!(!broker.sessions.lock().unwrap().contains_key(&7));
    }

    #[test]
    fn completed_snapshot_preserves_output_offsets() {
        let broker = PtyBroker::default();
        broker.completed.lock().unwrap().insert(
            42,
            CompletedPty {
                data: b"tail".to_vec(),
                start_offset: 96,
                end_offset: 100,
                code: Some(0),
                seq: 0,
            },
        );

        let snapshot = broker.snapshot(42, 0);
        assert!(!snapshot.active);
        assert!(snapshot.exited);
        assert_eq!(
            snapshot.data,
            base64::engine::general_purpose::STANDARD.encode(b"tail")
        );
        assert_eq!(snapshot.start_offset, 96);
        assert_eq!(snapshot.end_offset, 100);
        assert_eq!(snapshot.exit_code, Some(0));
    }

    /// 启动探测代答的核心契约:首个可见字节之前的 `ESC[6n` 答、且每个只答一次,同时
    /// **从流中摘除**——下游(attach DsrFilter、GUI xterm)看不到已答的查询,就不可能
    /// 再答第二遍;其余字节一个不动。画过首帧后永久停机、原样透传(实时查询交还给
    /// 活着的视图,xterm 以真实位置应答)。这是「新建会话卡初始化」与「恢复会话
    /// composer 多个 C」两个 bug 的共同守卫。
    #[test]
    fn startup_probe_scanner_answers_and_strips_prepaint_queries_once() {
        let mut scanner = StartupProbeScanner::new();
        // claude 冷启动的真实形态:查光标、藏光标、清屏——探测被摘除,其余原样保留。
        assert_eq!(
            scanner.feed(b"\x1b[6n\x1b[?25l\x1b[2J"),
            (b"\x1b[?25l\x1b[2J".to_vec(), 1)
        );
        // 跨 chunk 撕裂的查询:前缀暂存不外流,补齐后计数,字节不重复不丢失。
        assert_eq!(scanner.feed(b"\x1b["), (vec![], 0));
        assert_eq!(scanner.feed(b"6n"), (vec![], 1));
        // 撕裂后排除探测的序列:暂存的前缀原样冲出。
        assert_eq!(scanner.feed(b"\x1b[6"), (vec![], 0));
        assert_eq!(scanner.feed(b"m"), (b"\x1b[6m".to_vec(), 0));
        // OSC 标题文本与空白控制字节都不算画面,探测期未结束。
        assert_eq!(
            scanner.feed(b"\x1b]0;title\x07 \r\n\x1b[6n"),
            (b"\x1b]0;title\x07 \r\n".to_vec(), 1)
        );
        // 首个可见字节后停机:此后的查询原样透传,由活着的视图应答。
        assert_eq!(scanner.feed(b"W"), (b"W".to_vec(), 0));
        assert_eq!(scanner.feed(b"\x1b[6n"), (b"\x1b[6n".to_vec(), 0));
    }

    #[test]
    fn startup_probe_scanner_ignores_lookalikes_and_stops_at_paint() {
        let mut scanner = StartupProbeScanner::new();
        // 参数不是恰好 "6" 的 CSI-n 都不是光标探测(DSR 状态查询/DECXCPR 变体等),
        // 一律不答、原样保留。
        let lookalikes: &[u8] = b"\x1b[16n\x1b[?6n\x1b[6;1n\x1b[0n";
        assert_eq!(scanner.feed(lookalikes), (lookalikes.to_vec(), 0));
        // 同一 chunk 里查询在可见字节之前:计入并摘除,可见字节起原样透传(含后续查询)。
        let mut scanner = StartupProbeScanner::new();
        assert_eq!(
            scanner.feed(b"\x1b[6nhello\x1b[6n"),
            (b"hello\x1b[6n".to_vec(), 1)
        );
    }

    /// charset 指定(`ESC ( B`)与 DCS/APC 等字符串序列的负载不是画面:此前 ESC 的
    /// 中间字节/串负载被误判为可见字节,扫描器提前停机——之后的探测没人答,TUI 卡在
    /// 首帧前(25s 兜底后黑屏)。
    #[test]
    fn startup_probe_scanner_survives_charset_and_string_sequences() {
        let mut scanner = StartupProbeScanner::new();
        assert_eq!(scanner.feed(b"\x1b(B\x1b)0"), (b"\x1b(B\x1b)0".to_vec(), 0));
        assert_eq!(
            scanner.feed(b"\x1bP+q544e\x1b\\"),
            (b"\x1bP+q544e\x1b\\".to_vec(), 0)
        );
        assert_eq!(
            scanner.feed(b"\x1b_Ga=q\x1b\\"),
            (b"\x1b_Ga=q\x1b\\".to_vec(), 0)
        );
        // 这些序列之后的探测仍有人答。
        assert_eq!(scanner.feed(b"\x1b[6n"), (vec![], 1));
        // 真正的可见字节才停机。
        assert_eq!(scanner.feed(b"x"), (b"x".to_vec(), 0));
        assert!(scanner.painted());
    }

    /// attach 回放的展示流要滤掉历史查询(客户端 DsrFilter 会代答流里的每个查询,
    /// 历史查询的迟到应答会打进 agent 输入框);形似而非与残缺前缀原样保留。
    #[test]
    fn strip_dsr_queries_removes_only_complete_queries() {
        assert_eq!(strip_dsr_queries(b"ab\x1b[6ncd"), b"abcd");
        assert_eq!(strip_dsr_queries(b"\x1b[6n\x1b[6n"), b"");
        assert_eq!(strip_dsr_queries(b"\x1b[16n\x1b[?6n"), b"\x1b[16n\x1b[?6n");
        // backlog 裁剪边界留下的残缺前缀:不匹配,不误删。
        assert_eq!(strip_dsr_queries(b"\x1b[6"), b"\x1b[6");
    }

    /// since 的三种边界：命中中段只回增量、追平后回空、落在已裁剪区之前退化为全量。
    /// 这是「轮询不再每次传整个 backlog」的核心契约。
    #[test]
    fn snapshot_returns_only_bytes_after_since() {
        let decode = |s: &str| base64::engine::general_purpose::STANDARD.decode(s).unwrap();
        let broker = PtyBroker::default();
        broker.completed.lock().unwrap().insert(
            7,
            CompletedPty {
                data: b"abcdef".to_vec(),
                start_offset: 100,
                end_offset: 106,
                code: Some(0),
                seq: 0,
            },
        );

        // since 在区间中段：只回它之后的字节，start_offset 前移到 since。
        let mid = broker.snapshot(7, 103);
        assert_eq!(decode(&mid.data), b"def");
        assert_eq!((mid.start_offset, mid.end_offset), (103, 106));

        // since 已追平末尾：空增量，稳态轮询的常态——此前每次都要传整份。
        let caught_up = broker.snapshot(7, 106);
        assert!(decode(&caught_up.data).is_empty());
        assert_eq!((caught_up.start_offset, caught_up.end_offset), (106, 106));

        // since 早于 backlog 起点（那段已被裁剪）：退化为全量，不能假装数据还在。
        let stale = broker.snapshot(7, 40);
        assert_eq!(decode(&stale.data), b"abcdef");
        assert_eq!(stale.start_offset, 100);

        // since 超前于末尾（会话被重置）：不得 panic 或越界，回空让前端按 offset 重新对齐。
        let ahead = broker.snapshot(7, 999);
        assert!(decode(&ahead.data).is_empty());
    }

    #[test]
    fn attach_server_rejects_bad_token_and_tokens_are_unique() {
        let first = PtyBroker::default();
        let second = PtyBroker::default();
        assert_eq!(first.attach.token.len(), 64);
        assert_ne!(first.attach.token, second.attach.token);
        first.start_attach_server().unwrap();
        let endpoint = first.attach.endpoint.lock().unwrap().unwrap();
        let mut stream = TcpStream::connect(endpoint).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        writeln!(stream, "MEOWO1 wrong 1 80 24 nonce1234").unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);

        first
            .attach
            .pending
            .lock()
            .unwrap()
            .insert("launch-token".into(), -7);
        assert!(first.handle_claim("wrong", "launch-token", 9).is_err());
        assert_eq!(
            first.attach.pending.lock().unwrap().get("launch-token"),
            Some(&-7)
        );

        first.attach.bindings.lock().unwrap().insert(-7, 9);
        // binding 是只读非消费：对话窗口会重复轮询，重挂后还会再读——一次性消费会让
        // 后到的读方永远等不到真实 id。绑定表由 PTY 退出路径清理（reader 线程 retain）。
        assert_eq!(first.binding(-7), Some(9));
        assert_eq!(first.binding(-7), Some(9));
    }

    #[test]
    fn resolving_wrong_session_does_not_consume_approval() {
        let broker = PtyBroker::default();
        let (tx, rx) = mpsc::channel();
        let request = ApprovalRequest {
            session_id: 7,
            request_id: "request-123".into(),
            provider: "codex".into(),
            tool_name: "Bash".into(),
            description: Some("run tests".into()),
            input: "{}".into(),
            permission_suggestions: vec![],
        };
        broker.attach.approvals.lock().unwrap().insert(
            request.request_id.clone(),
            PendingApproval {
                request: request.clone(),
                response: tx,
            },
        );
        assert_eq!(broker.pending_approval(7).unwrap().tool_name, "Bash");
        assert!(broker
            .resolve_approval(8, "request-123", ApprovalDecision::Allow)
            .is_err());
        broker
            .resolve_approval(7, "request-123", ApprovalDecision::Deny)
            .unwrap();
        assert!(matches!(rx.recv().unwrap(), ApprovalDecision::Deny));
        assert!(broker.pending_approval(7).is_none());
    }

    #[test]
    fn resolving_agent_suggestion_returns_its_original_permission_update() {
        let broker = PtyBroker::default();
        let (tx, rx) = mpsc::channel();
        let suggestion = serde_json::json!({
            "type": "addRules",
            "behavior": "allow",
            "destination": "localSettings",
            "rules": [{ "toolName": "Bash", "ruleContent": "cargo test" }],
        });
        broker.attach.approvals.lock().unwrap().insert(
            "request-options".into(),
            PendingApproval {
                request: ApprovalRequest {
                    session_id: 7,
                    request_id: "request-options".into(),
                    provider: "claude".into(),
                    tool_name: "Bash".into(),
                    description: None,
                    input: r#"{"command":"cargo test"}"#.into(),
                    permission_suggestions: vec![suggestion.clone()],
                },
                response: tx,
            },
        );

        broker
            .resolve_approval_choice(7, "request-options", "suggestion:0")
            .unwrap();
        assert_eq!(
            rx.recv().unwrap(),
            ApprovalDecision::AllowWithPermissions(vec![suggestion])
        );
    }

    #[test]
    fn closing_approval_consumer_passes_every_pending_request() {
        let broker = PtyBroker::default();
        let mut receivers = Vec::new();
        for id in ["request-1", "request-2"] {
            let (tx, rx) = mpsc::channel();
            broker.attach.approvals.lock().unwrap().insert(
                id.into(),
                PendingApproval {
                    request: ApprovalRequest {
                        session_id: 7,
                        request_id: id.into(),
                        provider: "codex".into(),
                        tool_name: "Bash".into(),
                        description: None,
                        input: "{}".into(),
                        permission_suggestions: vec![],
                    },
                    response: tx,
                },
            );
            receivers.push(rx);
        }
        broker.pass_pending_approvals();
        assert!(broker.attach.approvals.lock().unwrap().is_empty());
        assert!(receivers
            .into_iter()
            .all(|rx| matches!(rx.recv().unwrap(), ApprovalDecision::Pass)));
    }

    #[test]
    fn approval_consumer_lease_is_session_scoped_and_releases_on_last_unregister() {
        let broker = PtyBroker::default();
        broker
            .register_approval_consumer(7, "consumer-a".into())
            .unwrap();
        broker
            .register_approval_consumer(7, "consumer-b".into())
            .unwrap();
        let (tx, rx) = mpsc::channel();
        broker.attach.approvals.lock().unwrap().insert(
            "request-7".into(),
            PendingApproval {
                request: ApprovalRequest {
                    session_id: 7,
                    request_id: "request-7".into(),
                    provider: "codex".into(),
                    tool_name: "Bash".into(),
                    description: None,
                    input: "{}".into(),
                    permission_suggestions: vec![],
                },
                response: tx,
            },
        );

        broker.unregister_approval_consumer("consumer-a");
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(broker.pending_approval(7).is_some());

        broker.unregister_approval_consumer("consumer-b");
        assert!(matches!(rx.recv().unwrap(), ApprovalDecision::Pass));
        assert!(broker.pending_approval(7).is_none());
    }

    #[test]
    fn destroying_chat_window_clears_every_consumer_lease() {
        // 窗口销毁时前端的 unregister 未必执行得到；关窗兜底必须把租约清干净，
        // 否则残留租约会让下一个审批入队空等 300s 而不是立即交还 TUI。
        let broker = PtyBroker::default();
        broker
            .register_approval_consumer(7, "consumer-a".into())
            .unwrap();
        broker
            .register_approval_consumer(8, "consumer-b".into())
            .unwrap();
        broker.clear_approval_consumers();
        assert!(!broker.has_approval_consumer(7));
        assert!(!broker.has_approval_consumer(8));
    }

    #[test]
    fn external_approval_passes_immediately_without_gui_consumer() {
        let broker = PtyBroker::default();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = broker.clone();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            server.handle_attach(stream).unwrap();
        });
        let request = ApprovalRequest {
            session_id: 77,
            request_id: "external-request-77".into(),
            provider: "codex".into(),
            tool_name: "Bash".into(),
            description: Some("build release".into()),
            input: "{}".into(),
            permission_suggestions: vec![],
        };
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&request).unwrap());
        let mut stream = TcpStream::connect(endpoint).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        writeln!(stream, "MEOWOAPPROVAL1 {} {}", broker.attach.token, encoded).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert_eq!(response, "pass\n");
        assert!(broker.pending_approval(77).is_none());
        handle.join().unwrap();
    }

    #[test]
    fn external_v2_approval_uses_the_shared_framing_and_passes_without_consumer() {
        let broker = PtyBroker::default();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = broker.clone();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            server.handle_attach(stream).unwrap();
        });
        let request = ApprovalRequest {
            session_id: 78,
            request_id: "external-request-78".into(),
            provider: "codex".into(),
            tool_name: "Bash".into(),
            description: Some("build release".into()),
            input: "{}".into(),
            permission_suggestions: vec![],
        };
        let mut stream = TcpStream::connect(endpoint).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        meowo_protocol::broker::write_v2_handshake(
            &mut stream,
            &BrokerRequest::Approval {
                token: broker.attach.token.clone(),
                request,
            },
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert_eq!(response, "pass\n");
        assert!(broker.pending_approval(78).is_none());
        handle.join().unwrap();
    }
}
