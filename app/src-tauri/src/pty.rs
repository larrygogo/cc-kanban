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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use tauri::{Emitter, Manager};

/// 锁中毒(持锁线程 panic 过)对用户只有一个含义:内部坏了,重启才能恢复。
/// 具体哪把锁属于排障细节,不抛到界面;前端 errors.ts 按本串前缀做双语映射,改文案需同步。
const LOCK_POISONED: &str = "内部状态异常，请重启 Meowo";

const BACKLOG_LIMIT: usize = 1024 * 1024;

/// 屏幕检测的启动宽限：spawn 后头几秒是启动 splash / 首帧探测期，扫出来的多半是噪音
/// （herdr 同款纪律）。宽限内不发布任何状态，前端不显示角标。
const DETECT_STARTUP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// 屏幕检测节拍（herdr 对已识别 agent 同为 300ms）。防抖的 3 次确认在该节奏下约 600ms
/// 收敛，落在 700ms 时间上限之内，不需要单独的加密轮询档。
const DETECT_TICK: std::time::Duration = std::time::Duration::from_millis(300);

/// 待处理题面在 broker 侧的存活上限。取 150s：略早于前端题面卡自己的 180s 兜底过期，
/// 保证「卡还在但表里已空」而不是反过来（表里残留会让轮询把答过的题重新弹出来）。
const INTERACTIVE_QUESTION_TTL_MS: i64 = 150_000;

/// 同时处于**握手阶段**的 attach 连接数上限。10s 读超时只保证单个连接不永久占线程，
/// 挡不住超时窗口内的堆积；正常场景同时握手的连接是个位数，32 只挡失控/恶意建连。
const MAX_ATTACH_HANDSHAKES: usize = 32;

/// 一个已占用的握手名额：Drop 归还，handle_attach 的任何 return/panic 路径都不会漏还。
/// 只计握手阶段，认证后的长驻转发不占名额。None 形态留给单测直连。
struct HandshakeSlot(Arc<AtomicUsize>);

impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// vt100 标题回调：OSC 0/2 设置的窗口标题是检测证据（claude 用盲文 spinner/✳ 前缀）。
/// 标题是不受信任的模型输出——过滤控制字符并限长；空 payload 视为清除。
#[derive(Default)]
struct ScreenTitle {
    title: Option<String>,
    /// OSC 9 的 payload（`ESC ] 9 ; …`）。ConEmu 系的进度序列 `9;4;<state>;<pct>` 走这里，
    /// agent 用它汇报「跑着 / 完成 / 出错」，是标题之外的第二个状态信号源。
    progress: Option<String>,
}

/// 标题与进度都是**不受信任的模型输出**：过滤控制字符并限长，避免它们污染判定
/// 或在诊断输出里注入转义序列。
fn sanitize_osc(raw: &[u8]) -> Option<String> {
    let text: String = String::from_utf8_lossy(raw)
        .chars()
        .filter(|ch| !ch.is_control())
        .take(256)
        .collect();
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

impl vt100::Callbacks for ScreenTitle {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = sanitize_osc(title);
    }

    /// vt100 只内建处理 OSC 0/1/2/52，其余走这里。取 OSC 9 当进度信号；
    /// 空 payload 视为清除（与标题同一约定）。
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let [b"9", rest @ ..] = params {
            // 参数已按 `;` 切开，重新拼回原始 payload 形态（规则按 `4;0` 这种前缀匹配）。
            let joined = rest
                .iter()
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect::<Vec<_>>()
                .join(";");
            self.progress = sanitize_osc(joined.as_bytes());
        }
    }
}

/// 托管会话的屏幕检测状态，随 [`ManagedPty`] 生灭。
///
/// 检测的输入必须是**终端仿真后的末屏文本**而非 backlog 原始字节——规则匹配会被全屏
/// 重绘/光标定位序列打穿（herdr 调研的核心教训）。parser 由 reader 线程喂字节（纯内存，
/// 与 StartupProbeScanner 同量级），判定由独立的检测节拍线程按需读取。
struct ScreenProbe {
    /// `None` = 该 provider 没有规则集：**不建 parser**，reader 热路径上一个字节都不解析。
    /// 门必须设在生产端：只在 tick 里判 provider 的话，无规则的会话照样每 chunk 跑完整
    /// VT 状态机、常驻一份 grid，产出全部丢弃——开销在 reader，不在 ticker。
    parser: Option<Mutex<vt100::Parser<ScreenTitle>>>,
    debounce: Mutex<crate::detect::ScreenDebounce>,
    /// 上次扫描时的 output_end。无新输出且无待确认降级时整轮跳过——空闲会话零开销。
    scanned_end: AtomicU64,
    started_at: std::time::Instant,
    /// agent 标签（"claude" 等），由调用方从 DB 的 sessions.provider 传入（**不从 argv[0]
    /// 猜**，见 PtyBroker::start 的文档），决定用哪套规则。
    provider: String,
}

impl ScreenProbe {
    fn new(rows: u16, cols: u16, provider: String) -> Self {
        let parser = crate::detect::provider_supported(&provider).then(|| {
            Mutex::new(vt100::Parser::new_with_callbacks(
                rows,
                cols,
                0,
                ScreenTitle::default(),
            ))
        });
        Self {
            parser,
            debounce: Mutex::new(crate::detect::ScreenDebounce::default()),
            scanned_end: AtomicU64::new(0),
            started_at: std::time::Instant::now(),
            provider,
        }
    }

    /// 取末屏快照（行 + 标题）。锁内只做 grid 逐行导出，不跑规则。
    fn snapshot(&self) -> Option<crate::detect::ScreenSnapshot> {
        let parser = self.parser.as_ref()?.lock().ok()?;
        let screen = parser.screen();
        let (_, cols) = screen.size();
        let lines: Vec<String> = screen.rows(0, cols).collect();
        let callbacks = parser.callbacks();
        Some(
            crate::detect::ScreenSnapshot::new(lines, callbacks.title.clone())
                .with_progress(callbacks.progress.clone()),
        )
    }

    /// 跑一轮检测。`current_end` 是会话此刻的累计输出偏移（[`ManagedPty::output_end`]）。
    /// 返回 Some = 发布状态发生变化。
    fn tick(
        &self,
        current_end: u64,
        now: std::time::Instant,
    ) -> Option<crate::detect::ScreenState> {
        // parser 为 None 即无规则集（门在构造时，见字段注释），snapshot() 会返回 None，
        // 这里不必再判 provider。
        if now.duration_since(self.started_at) < DETECT_STARTUP_GRACE {
            return None;
        }
        let pending = self
            .debounce
            .lock()
            .map(|debounce| debounce.pending())
            .unwrap_or(false);
        if !pending && self.scanned_end.load(Ordering::Acquire) == current_end {
            return None;
        }
        let snapshot = self.snapshot()?;
        self.scanned_end.store(current_end, Ordering::Release);
        let eval = crate::detect::evaluate(&self.provider, &snapshot)?;
        self.debounce.lock().ok()?.observe(&eval, now)
    }
}

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
    /// spawn 后立刻捕获的子进程 pid。杀子孙树按它查快照——绝不能事后再 lock child 拿
    /// process_id：child 锁可能被卡死的收尾长持（历史死锁教训见 reap_child 注释）。
    child_pid: Option<u32>,
    /// 本会话的 ConPTY 宿主（conhost）pid：openpty 前后差集捕获（见 start_spawned），
    /// 非 Windows / 捕获失败为 None。升级链最后一档与退出清理的锤子——对僵死的 conhost
    /// 而言 ClosePseudoConsole 永不返回，直接 TerminateProcess 它才能解除挂在 ConPTY
    /// 内核管道上的等待（实拍等价于任务管理器手杀 conhost，agent 随之真正可杀）。
    conhost_pid: Option<u32>,
    /// 「结束会话」的请求时刻。waiter 据此升级强杀：TerminateProcess 对卡死在 ConPTY
    /// 内核 I/O 的进程会**静默无效**（portable-pty 0.9 的 kill 恒返回 Ok，不代表进程真死），
    /// 只等 try_wait 的话收尾永不触发，UI 永远「运行中」。只记首次，重复点击不重置计时。
    stop_requested_at: Mutex<Option<std::time::Instant>>,
    backlog: Mutex<VecDeque<u8>>,
    /// 自 PTY 启动以来累计输出的字节位置；与 backlog 锁内更新，供快照和实时帧去重排序。
    output_end: AtomicU64,
    subscribers: Mutex<Vec<AttachSubscriber>>,
    /// 屏幕检测状态（终端仿真 + 防抖）。见 [`ScreenProbe`]。
    probe: ScreenProbe,
    /// 最近一次生效的 PTY 尺寸（cols<<16|rows 打包，0 = 尚未设置）。resize 的同值短路用：
    /// 前端切换视图/多视图并存时会重复下发同一尺寸，不短路的话每次都是一发 SIGWINCH、
    /// TUI 整屏重排 + 屏幕状态机扫描位点清零。
    last_size: AtomicU32,
}

/// 一个在线的外部同步终端（attach 客户端）。pid 是客户端上报的自身进程号，供查重
/// 路径反查宿主终端窗口做精确聚焦；旧 reporter 不上报 → None。
struct AttachSubscriber {
    id: u64,
    pid: Option<u32>,
    tx: mpsc::Sender<Vec<u8>>,
}

/// [`PtyBroker::external_viewer`] 的判定结果。在线判定与激活目标必须在同一次订阅表
/// 锁内取齐——拆成两次调用（先问在不在、再问 pid）时，detach 的 retain 可以恰好插在
/// 中间，把新 reporter 的会话误判成「有订阅但没 pid」的旧 reporter，走错兜底路径。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalViewer {
    /// 无在线外部视图（或会话不存在）。
    None,
    /// 有在线视图但都没上报 pid（旧 reporter）。
    Legacy,
    /// 最近注册且上报了 pid 的在线视图。
    Pid(u32),
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
    /// launch_token → 新建会话时的启动选项**选择 map**（option id → choice id）。
    /// claim 认领时写进 sessions.launch_args——权限模式等选项是启动参数，不落库的话
    /// resume/接管的重启进程会重置回 CLI 默认（实拍反馈）。
    pending_launch_args: Mutex<HashMap<String, HashMap<String, String>>>,
    /// launch_token → 被接替的旧会话 id（跨 provider 切换）。与 pending_launch_args
    /// 同构：会话行 claim 认领时才存在，接续链也只能在那一刻落库。CLI 起不来 / claim
    /// 不发生时这条暂存自然过期，旧会话**不会**被错误标记为已接替——这是切换失败的
    /// 关键防线（用户仍可 resume 回旧引擎）。
    pending_lineage: Mutex<HashMap<String, i64>>,
    bindings: Mutex<HashMap<i64, i64>>,
    approvals: Mutex<HashMap<String, PendingApproval>>,
    /// 显式注册的 GUI 审批消费者。窗口存在/可见不等于已经订阅了目标 session。
    /// 键分两族：桌面对话窗的租约（前端自取 id）与远程桥的 `remote:` 前缀租约
    /// （remote.rs 在服务端强制加前缀，两族互不可冒充）。远端租约带 TTL
    /// （[`REMOTE_CONSUMER_TTL_MS`]）：手机切后台/被杀不会走 unregister，残留
    /// 租约会压制桌面召唤并让审批静默空等 300s；桌面租约不设 TTL——窗口销毁
    /// 兜底（release_desktop_consumers）已覆盖它的残留形态，语义不动。
    approval_consumers: Mutex<HashMap<String, ConsumerLease>>,
    /// 已自动放行、等用户处理的 AskUserQuestion 题面（session_id → (题面, 入表时刻)）。
    ///
    /// `interactive-question` 事件是 fire-and-forget：对话窗冷启动要 1~2s，而 emit
    /// 在切窗后微秒级发出，此刻没有任何监听者，Tauri 的 emit **不排队不重放**——事件
    /// 就此消失，用户面对一扇没有题面卡的窗口。常规审批靠「入表 + 等消费者注册 + 前端
    /// 400ms 轮询」两道保险兜住同一问题；这条路径为了让 TUI 表单零延迟出现，刻意不等
    /// 消费者，那就更不能把入表也一起丢。
    interactive_questions: Mutex<HashMap<i64, (ApprovalRequest, i64)>>,
    app: Mutex<Option<tauri::AppHandle>>,
}

struct PendingApproval {
    request: ApprovalRequest,
    response: mpsc::Sender<ApprovalDecision>,
}

/// 远端审批消费者租约的保鲜期。手机端（useApprovalChannel 远程模式）每 20s 重注册
/// 续约，60s = 三个续约周期都缺席才判死——网络抖动不误杀，锁屏/关页 1 分钟内桌面
/// 召唤恢复。
const REMOTE_CONSUMER_TTL_MS: i64 = 60_000;

/// 远端消费者的 id 前缀。由 remote.rs 的桥接臂在**服务端**强制添加：桌面端无法
/// 伪造远端身份，远端也无法冒充桌面租约。
pub(crate) const REMOTE_CONSUMER_PREFIX: &str = "remote:";

/// 审批消费者租约：哪条会话 + 最近一次注册/续约时刻。
struct ConsumerLease {
    session_id: i64,
    seen_ms: i64,
}

fn is_remote_consumer(consumer_id: &str) -> bool {
    consumer_id.starts_with(REMOTE_CONSUMER_PREFIX)
}

/// 租约是否仍然有效：桌面租约恒新鲜（无 TTL），远端租约按 TTL 判定。纯函数供单测。
fn consumer_lease_fresh(consumer_id: &str, seen_ms: i64, now_ms: i64) -> bool {
    !is_remote_consumer(consumer_id) || now_ms.saturating_sub(seen_ms) <= REMOTE_CONSUMER_TTL_MS
}

/// 「此刻正被注视」比「租约还有效」严格得多。远端心跳 20s 一发,租约 60s 才过期——
/// 这段差是**给审批领卡留的宽限**(手机锁屏 60s 内切回仍能批),但拿去抑制桌面 toast
/// 就错了:手机装兜里 20-60s,人根本没在看,桌面却被压着不弹「等你回复」。故 viewed
/// 信号用 1.5×心跳(30s)判活:漏一次心跳就不再算「在看」,jitter 不误伤活跃手机。
const REMOTE_CONSUMER_VIEWING_MS: i64 = 30_000;

fn consumer_lease_viewing(consumer_id: &str, seen_ms: i64, now_ms: i64) -> bool {
    !is_remote_consumer(consumer_id) || now_ms.saturating_sub(seen_ms) <= REMOTE_CONSUMER_VIEWING_MS
}

#[derive(Clone)]
pub(crate) struct PtyBroker {
    sessions: Arc<Mutex<HashMap<i64, Arc<ManagedPty>>>>,
    /// chat 窗此刻正在展示终端视图的会话 id（0 = 没有）。emitter 只对它做 base64 + emit：
    /// pty-output 只有一个消费者（chat 窗的 ManagedTerminal，单实例），其余托管会话的
    /// 实时帧发出去也只是在 JS 侧被 sessionId 过滤丢弃——N 个并发会话齐跑时，白付的
    /// 编码与 WebView2 IPC 正好是压垮前端的那部分。切换会话由前端重新注册 + 快照全量
    /// 补齐（rearm 的既有机制），不漏字节。
    viewed_session: Arc<AtomicI64>,
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
    /// 屏幕检测节拍线程只起一次（幂等门）。
    detect_started: Arc<AtomicBool>,
}

impl Default for PtyBroker {
    fn default() -> Self {
        let token = random_token();
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            viewed_session: Arc::new(AtomicI64::new(0)),
            starting: Arc::new(Mutex::new(HashSet::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(Mutex::new(HashMap::new())),
            detect_started: Arc::new(AtomicBool::new(false)),
            attach: Arc::new(AttachState {
                endpoint: Mutex::new(None),
                token,
                started: AtomicBool::new(false),
                next_subscriber: AtomicU64::new(1),
                next_pending: AtomicI64::new(-1),
                pending: Mutex::new(HashMap::new()),
                pending_launch_args: Mutex::new(HashMap::new()),
                pending_lineage: Mutex::new(HashMap::new()),
                bindings: Mutex::new(HashMap::new()),
                approvals: Mutex::new(HashMap::new()),
                approval_consumers: Mutex::new(HashMap::new()),
                interactive_questions: Mutex::new(HashMap::new()),
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

/// 收尸：拿 exit code，进程还活着就先杀。**不许无条件阻塞 wait**——EOF 触发的收尾里
/// 子进程可能并没退出（conhost 先死而 agent 卡死），阻塞 wait 会拿着 child 锁永久不还，
/// stop()/shutdown() 全在这把锁上排队，表现为「结束会话」点了没反应、退出应用卡住。
/// EOF 后终端流已死，画面与输入都永远回不来，还活着的子进程只是僵尸，杀掉是唯一出路。
fn reap_child(child: &mut (dyn portable_pty::Child + Send + Sync)) -> Option<u32> {
    reap_child_within(
        child,
        std::time::Duration::from_millis(500),
        std::time::Duration::from_secs(2),
    )
}

/// `grace`：EOF 与退出之间的正常宽限；`kill_wait`：kill 后等退出码的上限。
/// kill 后**绝不调无限阻塞的 wait()**——TerminateProcess 对卡死在 ConPTY 内核 I/O 的进程
/// 会静默无效（portable-pty 0.9 的 kill 恒返回 Ok），wait 会拿着 child 锁永久不还，
/// stop()/shutdown() 全在这把锁上排队。超时拿不到退出码就返回 None：
/// 一个退出码不值得拿整个收尾路径陪葬。
fn reap_child_within(
    child: &mut (dyn portable_pty::Child + Send + Sync),
    grace: std::time::Duration,
    kill_wait: std::time::Duration,
) -> Option<u32> {
    // EOF 与进程退出之间存在正常的毫秒级窗口（unix 上关掉 tty 的进程还在收尾）：
    // 先宽限轮询一小段，能等到就拿真实退出码。waiter 路径 try_wait 确认过退出，首轮即返回。
    let deadline = std::time::Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.exit_code()),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            // 宽限到点还活着（或 try_wait 出错拿不到结论）：kill 后有限轮询收尸。
            // 正常被杀的进程毫秒级就能等到退出码；等不到的是内核态僵尸，放弃。
            _ => {
                let _ = child.kill();
                let kill_deadline = std::time::Instant::now() + kill_wait;
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => return Some(status.exit_code()),
                        _ if std::time::Instant::now() >= kill_deadline => return None,
                        _ => std::thread::sleep(std::time::Duration::from_millis(10)),
                    }
                }
            }
        }
    }
}

/// [`offer_with_deadline`] 的结果：调用方要区分「等不到位」与「接收端已死」——
/// write() 对前者报「输入已积压」、对后者报「通道已关闭」；reader 对两者都只是丢帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Offer {
    Sent,
    TimedOut,
    Disconnected,
}

/// 有界等待入队：队满时最多等 `wait`，到点放弃。**绝不阻塞 send**——有界通道的
/// 阻塞 send 会把接收端的停滞原样反压到调用线程（输入侧的教训见 ManagedPty::input_tx，
/// 输出侧的教训见 reader 的 emit 投递点）。
fn offer_with_deadline<T>(tx: &mpsc::SyncSender<T>, item: T, wait: std::time::Duration) -> Offer {
    let deadline = std::time::Instant::now() + wait;
    let mut item = item;
    loop {
        item = match tx.try_send(item) {
            Ok(()) => return Offer::Sent,
            Err(mpsc::TrySendError::Disconnected(_)) => return Offer::Disconnected,
            Err(mpsc::TrySendError::Full(item)) => item,
        };
        if std::time::Instant::now() >= deadline {
            return Offer::TimedOut;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

/// [`PtyBroker::write`] 的输入积压错误（有界等待超时）。attach 输入循环靠它区分
/// 「临时积压」与「会话真没了」：前者丢帧继续转发，后者才断开镜像连接。
pub(crate) const INPUT_BACKLOGGED: &str = "Agent 未在读取输入，输入已积压，请稍后重试";

/// [`PtyBroker::resize`] 的尺寸通道忙错误（master 锁有界等待超时）。与积压同理属临时
/// 状态，attach 循环对它同样只跳过本帧。
pub(crate) const RESIZE_BUSY: &str = "PTY 尺寸通道忙（可能已僵死），本次调整跳过";

/// 「结束会话」的升级档位。时间轴：stop 请求 → [`STOP_KILL_TREE_AFTER`] 仍活 →
/// 杀子孙树 + 关伪终端 → 共 [`STOP_FORCE_FINALIZE_AFTER`] 仍活 → 强制收尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopStage {
    Wait,
    KillTree,
    ForceFinalize,
}

const STOP_KILL_TREE_AFTER: std::time::Duration = std::time::Duration::from_secs(1);
const STOP_FORCE_FINALIZE_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// 纯谓词，时间点由调用方注入（waiter 传真实 now，测试传构造的时刻）。
fn stop_stage(stop_requested_at: Option<std::time::Instant>, now: std::time::Instant) -> StopStage {
    let Some(at) = stop_requested_at else {
        return StopStage::Wait;
    };
    let elapsed = now.saturating_duration_since(at);
    if elapsed >= STOP_FORCE_FINALIZE_AFTER {
        StopStage::ForceFinalize
    } else if elapsed >= STOP_KILL_TREE_AFTER {
        StopStage::KillTree
    } else {
        StopStage::Wait
    }
}

/// 有界拿锁：try_lock 轮询到 deadline，拿不到返回 None；中毒锁按全仓策略恢复
/// （确认框同款 into_inner，见 confirm.rs）。
///
/// **保证路径（waiter 升级链、finalize、shutdown）绝不允许无界 `lock()`**——`master`
/// 锁的持有者可能正卡死在 ConPTY syscall 里：conhost 僵死时 ResizePseudoConsole /
/// ClosePseudoConsole 都会永不返回（实拍：外部 attach 的握手 resize 先卡死持锁，
/// 「结束会话」的升级链再无界等锁，waiter 线程陪葬，ForceFinalize 永远轮不到——
/// 0.5.13 仍「结束不了」的直接根因）。
fn lock_within<T>(
    mutex: &Mutex<T>,
    wait: std::time::Duration,
) -> Option<std::sync::MutexGuard<'_, T>> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// stop 升级第二档：补刀 + 杀子孙树 + 关伪终端。child 锁只包 kill 那一下，杀树在锁外
/// （按 spawn 时捕获的 child_pid 查即时快照，root 此刻还活着，子孙表可信）。
/// drop master（ClosePseudoConsole）让 conhost 退出——这是解除「卡死在 ConPTY 内核
/// I/O」的唯一杠杆：conhost 一死，挂在管道上的内核等待多半随之解除，TerminateProcess
/// 才能真正生效。
///
/// 本函数在 waiter 线程上同步调用，而 waiter 还要负责推进 ForceFinalize 档，故这里
/// **一步都不许阻塞**：锁全部有界（持有者可能卡死在 ConPTY syscall，见 lock_within），
/// ClosePseudoConsole 丢牺牲线程——conhost 僵死时它同样永不返回，让那根线程带着句柄
/// 躺着（与 reader/writer 同一纪律），换 waiter 的 3s 兜底必定触发。锁拿不到时放弃的
/// 只是「关伪终端」这根杠杆，收尾与 UI 解锁由 ForceFinalize 无条件保证。
fn escalate_stop(managed: &Arc<ManagedPty>) {
    if let Some(mut child) = lock_within(&managed.child, std::time::Duration::from_millis(500)) {
        let _ = child.kill();
    }
    if let Some(pid) = managed.child_pid {
        crate::proc::kill_descendants(pid);
    }
    let cleanup = managed.clone();
    std::thread::spawn(move || {
        if let Some(mut master) = lock_within(&cleanup.master, std::time::Duration::from_secs(5)) {
            drop(master.take());
        }
    });
}

/// PTY 会话的唯一收尾路径：写库、通知看板与对话窗、掐断 attach、释放伪终端。
/// 幂等（finalized 原子门），由两处触发——waiter 轮询到进程退出（主力：Windows ConPTY
/// 在子进程退出后**不会**给 reader EOF，本机实证连 drop master 都唤不醒阻塞中的 read），
/// 以及 reader 万一真读到 EOF。收尾**从不等待 reader**；它若永远阻塞，就带着句柄躺着。
/// 同理**从不无界等锁**：这是「结束必定生效」的最后一档，completed/sessions/emit/写库
/// 必须无条件执行——锁有界、可能卡死的 ConPTY 释放丢牺牲线程（见 lock_within 的实拍）。
fn finalize_exit(broker: &PtyBroker, app: &tauri::AppHandle, managed: &Arc<ManagedPty>) {
    if managed.finalized.swap(true, Ordering::AcqRel) {
        return;
    }
    // reap 本身有界（宽限 + kill 后限时收尸，见 reap_child），锁也必须有界：child 锁的
    // 持有者理论上都短持，但保证路径不赌这个——拿不到就放弃退出码，一个退出码不值得
    // 拿整个收尾陪葬（与 reap 内部的原则一脉相承）。
    let code = lock_within(&managed.child, std::time::Duration::from_millis(500))
        .and_then(|mut child| reap_child(child.as_mut()));
    // 释放伪终端让 conhost 退出。它救不了阻塞的 reader，但能停掉资源。牺牲线程 + 有界
    // 拿锁：conhost 僵死时 ClosePseudoConsole 永不返回，锁的持有者（卡死的 resize）也
    // 永不放手——两种形态都不许把收尾拖死在这一步。
    let cleanup = managed.clone();
    std::thread::spawn(move || {
        if let Some(mut master) = lock_within(&cleanup.master, std::time::Duration::from_secs(5)) {
            drop(master.take());
        }
    });
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
        // 负 id（未认领的临时会话）没有库写入，直接发退出事件。
        if let Some(window) = app.get_webview_window("chat") {
            let _ = window.emit("pty-exit", PtyExit { session_id, code });
        }
    } else {
        // 托管 PTY 是这个 agent 进程的唯一持有者——它退出，会话就真的结束了。必须主动
        // 收尾：resume 路径已经乐观复活过 DB（prepare_resume），没人回滚的话卡片会一直
        // 假显示「已连接」，直到 pid 判活的宽限窗口过期才自愈。这同时覆盖了「PTY 起来了
        // 但 CLI 秒退（不在 PATH）」——那种情况 start() 返回 Ok，调用方的回滚够不着。
        // 写失败时卡片会假显示「已连接」，靠 pid 判活的宽限窗口自愈；留一条日志方便定位。
        //
        // pty-exit 先于写库发出：对话窗的解锁（按钮卸载、退出遮罩）只看这个事件，而
        // end_session 在库忙（busy_timeout 3s，双实例并发写时是常态）时会把 emit 拖到
        // 秒级——「结束会话」的 UI 反馈没有理由等一笔迟早会自愈的库写入。
        if let Some(window) = app.get_webview_window("chat") {
            let _ = window.emit("pty-exit", PtyExit { session_id, code });
        }
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
        // launch token 存续到 PTY 退出（/clear 换代要拿它重认领），此刻随绑定一起清：
        // 先收集本会话名下的临时 id，再按值反查摘掉 pending 里对应的 token。
        let mut temp_ids: Vec<i64> = Vec::new();
        if let Ok(mut bindings) = broker.attach.bindings.lock() {
            bindings.retain(|temp, real| {
                if *real == session_id {
                    temp_ids.push(*temp);
                    return false;
                }
                true
            });
        }
        if !temp_ids.is_empty() {
            if let Ok(mut pending) = broker.attach.pending.lock() {
                pending.retain(|_, id| !temp_ids.contains(id));
            }
        }
        crate::watch::emit_board_changed(app, "pty-exit");
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

/// cols/rows 打包进一个 u32（resize 同值短路的原子比较用）。0 保留为「尚未设置」。
fn pack_size(cols: u16, rows: u16) -> u32 {
    (u32::from(cols) << 16) | u32::from(rows)
}

/// [`pack_size`] 的逆运算。0（尚未设置）解回 (0, 0)，快照侧以 0 表达「未知」。
fn unpack_size(packed: u32) -> (u16, u16) {
    ((packed >> 16) as u16, (packed & 0xffff) as u16)
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
    /// 召唤策略（[`approval_summon_action`]，第三回摆锤后的落点）：
    /// - 用户正盯着**别的会话**（窗口在眼前且有消费者租约）：不切窗——正在 A 会话里
    ///   阅读/打字被瞬间拽到 B 是夺屏（实拍反馈，第二回）。改为徽标 + 任务栏闪烁召唤，
    ///   并返回 true 让请求**整个等待期（300s）保持可领取**：用户点过去的瞬间，消费者
    ///   注册 + 轮询就能取到卡片；claude 的 TUI 权限框与 hook 竞速，终端里也照常可答。
    ///   这与「只闪不切 + 10s 撤回」的第一代不同：撤回窗口是按 WebView2 冷启动量的，
    ///   而这里请求不撤回，人有充分的反应时间，审批不会悄悄回落。
    /// - 窗口不在眼前（隐藏/最小化/未创建）：没有进行中的注视可打断，照旧切到该会话
    ///   并安静唤醒，等消费者注册。
    ///
    /// 切会话召唤的分支只做**有界**等待，等不到就返回 false 让调用方撤回请求并答
    /// `pass`，提示回落到 agent 自己的审批界面——窗口起不来时 hook 不该压着 TUI
    /// 盲等满 300s。
    fn ensure_approval_window(&self, session_id: i64) -> ApprovalHold {
        match approval_gate(
            self.has_fresh_remote_consumer(session_id),
            crate::settings::load_settings().chat_enabled,
        ) {
            // 手机端正看着这条会话：请求直接可领取（手机 400ms 轮询取卡），桌面窗
            // 一概不动——人在手机上，切窗/闪烁都是对空椅子表演。
            ApprovalGate::Claimable => return ApprovalHold::RemoteClaimable,
            // 对话功能关闭（轻量模式）且手机端不在场：GUI 不是合法消费者，立即返回
            // false 让调用方答 pass 回落 TUI——不能走下面的 10s 消费者等待，那只会把
            // 终端审批面板的出现拖慢同样久。桌面租约检查也被跳过：关闭后即便残留已开
            // 的对话窗，审批也统一走 TUI，语义不摇摆（chat_enabled 关的是桌面对话窗，
            // 远程通道由 remote_access_enabled 自己管）。
            ApprovalGate::FallbackTui => return ApprovalHold::Reject,
            ApprovalGate::Desktop => {}
        }
        let Some(app) = self.attach.app.lock().ok().and_then(|app| app.clone()) else {
            return ApprovalHold::Reject;
        };
        match approval_summon_action(
            self.has_approval_consumer(session_id),
            self.has_any_approval_consumer(),
            crate::window::chat_window_in_view(&app),
        ) {
            SummonAction::Ready => return ApprovalHold::Desktop,
            SummonAction::Hold => {
                // 不切窗只召唤：请求已由调用方先入表，pending-approval 事件让前端
                // 点亮侧栏徽标（approvalAwaitingIds），闪烁提醒有事发生。
                if let Some(window) = app.get_webview_window("chat") {
                    let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
                }
                return ApprovalHold::Desktop;
            }
            SummonAction::Summon => {}
        }
        // 切会话召唤：注册审批消费者的是对话页那套 useEffect，只有会话切过去了才会注册。
        //
        // quiet：唤醒不抢焦点——用户可能正在外部终端里打字（实拍反馈），召唤注意力
        // 交给下面的任务栏闪烁；卡片可答性不依赖焦点（请求在表里，注册后轮询可取）。
        crate::window::open_chat_window_quiet(app.clone(), session_id);
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
                return ApprovalHold::Desktop;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        ApprovalHold::Reject
    }

    /// `provider` 是 agent id（"claude"/"codex"/"kimi"），决定屏幕检测用哪套规则。
    /// 由调用方从 DB 的 `sessions.provider` 传入——**不从 argv[0] 猜**：包装启动
    /// （npx/bunx/fnm shim/中转脚本）下 argv[0] 根本不是 agent 名，猜错就整条会话
    /// 没有状态检测，且失败是静默的。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &self,
        app: tauri::AppHandle,
        session_id: i64,
        argv: &[String],
        cwd: Option<&str>,
        env: &[(String, String)],
        terminal_size: TerminalSize,
        provider: &str,
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
        let result = self.start_spawned(app, session_id, argv, cwd, env, terminal_size, provider);
        if result.is_err() {
            // 启动失败清占位，重试才进得来；completed 快照已在 begin_start 摘掉，与旧行为一致。
            self.end_start(session_id);
        }
        result
    }

    /// 登记「启动中」占位。contains 检查与占位插入在同一锁程内原子完成，两个并发 start
    /// 只有一个拿得到占位。锁序约定：**starting → sessions → completed**。
    ///
    /// 另一处嵌套持锁是 `screen_states`/`external_viewer`（sessions → 会话内部锁），
    /// 与本处不共享除 sessions 外的任何锁，且都是单向获取（没有「先拿会话内部锁再拿
    /// sessions」的反向路径），构成不了 ABBA。新增跨表持锁前请先核对这两条锁序。
    /// 不变量：本函数返回后 completed[sid] 必为空，之后出现的条目一定来自本次调用之后的
    /// finalize。判重提前返回的分支同样要摘：start 按 Ok 收敛后调用方照常跑秒退探测（只凭
    /// completed 判断「起没起来」），上一代退出的定格快照会被误读成本次启动秒退——当前
    /// 这一代（运行中/启动中）尚未 finalize，completed 里的任何条目对该探测都是噪音。
    fn begin_start(&self, session_id: i64) -> Result<bool, String> {
        let mut starting = self.starting.lock().map_err(|_| LOCK_POISONED)?;
        let sessions = self.sessions.lock().map_err(|_| LOCK_POISONED)?;
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
        let mut sessions = self.sessions.lock().map_err(|_| LOCK_POISONED)?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("应用正在退出，放弃登记新会话".into());
        }
        sessions.insert(session_id, managed.clone());
        Ok(())
    }

    /// start 的锁外段：openpty → spawn → 登记 → 起 waiter/reader 线程。
    /// 调用方持有 starting 占位；本函数任一步失败都由调用方清占位。
    #[allow(clippy::too_many_arguments)]
    fn start_spawned(
        &self,
        app: tauri::AppHandle,
        session_id: i64,
        argv: &[String],
        cwd: Option<&str>,
        env: &[(String, String)],
        terminal_size: TerminalSize,
        provider: &str,
    ) -> Result<(), String> {
        let pty_size = size(terminal_size.cols, terminal_size.rows);
        // conhost 捕获：CreatePseudoConsole 以本进程为父 spawn 宿主进程却不暴露它的 pid，
        // openpty 前后对直接子进程里的宿主取差集锁定它（用途见 ManagedPty::conhost_pid）。
        // 进程级锁把「快照 → openpty → 快照」串行化，并发 start 的差集才不会混入对方的
        // 宿主；锁内只有毫秒级的 openpty 与两次只读快照，不含秒级的 spawn_command。
        let (pair, conhost_pid) = {
            static OPENPTY_LOCK: Mutex<()> = Mutex::new(());
            let _guard = OPENPTY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let before = crate::proc::conhost_children();
            let pair = native_pty_system()
                .openpty(pty_size)
                .map_err(|e| e.to_string())?;
            let conhost = crate::proc::conhost_children()
                .difference(&before)
                .next()
                .copied();
            (pair, conhost)
        };
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
        let child_pid = child.process_id();
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
            child_pid,
            conhost_pid,
            stop_requested_at: Mutex::new(None),
            backlog: Mutex::new(VecDeque::new()),
            output_end: AtomicU64::new(0),
            subscribers: Mutex::new(Vec::new()),
            finalized: AtomicBool::new(false),
            probe: ScreenProbe::new(pty_size.rows, pty_size.cols, provider.to_string()),
            last_size: AtomicU32::new(pack_size(pty_size.cols, pty_size.rows)),
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
        std::thread::spawn(move || {
            let mut tree_killed = false;
            loop {
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
                // 「结束会话」的升级链。stop() 只发 TerminateProcess，对卡死在 ConPTY 内核
                // I/O 的进程它会静默无效（kill 恒返回 Ok 是谎报）——只等 try_wait 的话这里
                // 永远转下去，UI 永远「运行中」。到点就加码，保证结束必定发生。
                let stop_requested_at = waiter
                    .stop_requested_at
                    .lock()
                    .ok()
                    .and_then(|at| *at);
                match stop_stage(stop_requested_at, std::time::Instant::now()) {
                    StopStage::Wait => {}
                    StopStage::KillTree => {
                        if !tree_killed {
                            escalate_stop(&waiter);
                            tree_killed = true;
                        }
                    }
                    StopStage::ForceFinalize => {
                        // 最后一把锤子：直接 TerminateProcess conhost。走到这一档说明
                        // TerminateProcess 对 agent 静默无效（卡死在 ConPTY 内核 I/O）、
                        // ClosePseudoConsole 也没能奏效（牺牲线程可能正卡在里面）——杀掉
                        // conhost 会解除挂在管道上的全部内核等待（实拍等价于任务管理器
                        // 手杀），agent 随之真正可杀，finalize 的有界收尸多半还能等到
                        // 退出码。纯句柄操作不碰任何锁，永不阻塞 waiter。
                        if let Some(pid) = waiter.conhost_pid {
                            crate::proc::kill_pid(pid);
                        }
                        // 强制收尾把会话从 UI 摘除（finalize 会 emit pty-exit + 移出
                        // sessions，前端两条感知路径都解锁）；万一 conhost 也杀不掉，
                        // zombie 带着句柄躺在系统里。
                        finalize_exit(&waiter_broker, &waiter_app, &waiter);
                        return;
                    }
                }
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
        let emitter_viewed = self.viewed_session.clone();
        std::thread::spawn(move || {
            const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
            // 单帧上限：重输出时一帧最多聚 256KB，base64 后 ~341KB，别让单条事件无限膨胀。
            const MAX_FRAME_BYTES: usize = 256 * 1024;
            let mut last_emit = std::time::Instant::now() - FRAME;
            // 聚合中撞上偏移不连续的 chunk 时暂存于此，作为下一帧的开头。
            let mut pending: Option<(u64, Vec<u8>)> = None;
            loop {
                let (offset, mut frame) = match pending.take() {
                    Some(head) => head,
                    None => match emit_rx.recv() {
                        Ok(head) => head,
                        Err(_) => return,
                    },
                };
                // 距上次 emit 不足一帧才聚合，聚合帧以首 chunk 的 offset 对齐。
                // 连续性校验：reader 超时丢帧后偏移必有洞，洞是前端重对齐的唯一信号，
                // 合帧绝不能把洞两侧拼成「看似连续」的一帧把它抹平——不连续就先发
                // 手头这帧，下一帧从洞后重新对齐。
                let frame_end = last_emit + FRAME;
                while frame.len() < MAX_FRAME_BYTES {
                    let now = std::time::Instant::now();
                    if now >= frame_end {
                        break;
                    }
                    // Timeout/Disconnected 都先把手头的发出去；断开由下轮 recv 收尾退出。
                    match emit_rx.recv_timeout(frame_end - now) {
                        Ok((next_offset, more)) => {
                            if next_offset != offset + frame.len() as u64 {
                                pending = Some((next_offset, more));
                                break;
                            }
                            frame.extend_from_slice(&more);
                        }
                        Err(_) => break,
                    }
                }
                // 只喂 chat 窗正在看的会话（viewed_session）：pty-output 只有那一个消费者，
                // 别的托管会话的帧发出去也只是被 JS 按 sessionId 过滤丢弃——N 个会话齐跑时
                // 这些白付的 base64 + WebView2 IPC 正是压垮前端的那部分。不是它就整帧跳过；
                // 窗口关着同理（base64 白做）。错过的字节由前端切会话时的快照全量补齐。
                let session_id = emitter_managed.session_id.load(Ordering::Acquire);
                if emitter_viewed.load(Ordering::Acquire) == session_id {
                    if let Some(window) = emitter_app.get_webview_window("chat") {
                        let payload = PtyOutput {
                            session_id,
                            offset,
                            data: base64::engine::general_purpose::STANDARD.encode(&frame),
                        };
                        let _ = window.emit("pty-output", &payload);
                    }
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
                        // 喂屏幕状态机（纯内存解析，锁只与低频的检测节拍争用）。放在
                        // backlog 之外：它不参与偏移/回放语义，坏了也只影响状态角标。
                        // parser 为 None（provider 无规则集）时整段跳过，不付解析成本。
                        if let Some(parser) = managed.probe.parser.as_ref() {
                            if let Ok(mut parser) = parser.lock() {
                                parser.process(&data);
                            }
                        }
                        // 分发必须发生在追加 backlog 的同一把锁内：handle_attach 在一个
                        // backlog→subscribers 临界区里「回放 + 注册订阅者」，若这里先放掉
                        // backlog 锁再单独锁 subscribers，恰在缝隙注册的订阅者会把同一个
                        // chunk 从回放和通道各收一次——外部终端上屏重复的原始 ANSI。
                        // send 是无界通道的非阻塞投递，双锁临界区不会久持。
                        let send_chunk = |data: &[u8]| {
                            if let Ok(mut subscribers) = managed.subscribers.lock() {
                                // 绝大多数会话没有外部 attach 客户端：先判空再拷贝，
                                // 否则构建刷屏时每个 chunk 都白付一次 to_vec。
                                if subscribers.is_empty() {
                                    return;
                                }
                                let chunk = data.to_vec();
                                subscribers
                                    .retain(|subscriber| subscriber.tx.send(chunk.clone()).is_ok());
                            }
                        };
                        let offset = if let Ok(mut backlog) = managed.backlog.lock() {
                            let offset = managed.output_end.load(Ordering::Relaxed);
                            backlog.extend(data.iter().copied());
                            // drain 一次成段移除：逐字节 pop_front 在缓冲满后每个 16KB chunk
                            // 要做上万次，且发生在与 snapshot() 相争的同一把锁内。
                            let excess = backlog.len().saturating_sub(BACKLOG_LIMIT);
                            if excess > 0 {
                                backlog.drain(..excess);
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
                        // 有界等待而非阻塞 send：emitter/UI 卡住时反压会一路传导——reader
                        // 停读 → conhost 输出缓冲满 → 子进程写 stdout 阻塞 → 不再读 stdin，
                        // 整个 agent 表现为挂死（「输入已积压」正是这么来的）。超时就丢这帧：
                        // backlog/订阅者在上面已无损喂过，丢的只是对话窗实时帧；emitter 的
                        // 连续性校验会把偏移洞暴露给前端，前端按缺口拉 snapshot 重对齐。
                        let _ = offer_with_deadline(
                            &emit_tx,
                            (offset, data.into_owned()),
                            std::time::Duration::from_secs(2),
                        );
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_pending(
        &self,
        app: tauri::AppHandle,
        argv: &[String],
        cwd: Option<&str>,
        env: &[(String, String)],
        cols: u16,
        rows: u16,
        provider: &str,
        launch_selections: &HashMap<String, String>,
        // 跨 provider 切换时 = 被接替的旧会话 id；普通新建传 None。claim 认领时落接续链。
        predecessor: Option<i64>,
    ) -> Result<i64, String> {
        let endpoint = self
            .attach
            .endpoint
            .lock()
            .map_err(|_| LOCK_POISONED)?
            .ok_or("attach 服务未启动")?;
        let launch_token = random_token();
        let temp_id = self.attach.next_pending.fetch_sub(1, Ordering::Relaxed);
        self.attach
            .pending
            .lock()
            .map_err(|_| LOCK_POISONED)?
            .insert(launch_token.clone(), temp_id);
        // 启动选项随 token 暂存：会话行此刻还不存在（hook 认领时才建），claim 落库。
        if !launch_selections.is_empty() {
            if let Ok(mut map) = self.attach.pending_launch_args.lock() {
                map.insert(launch_token.clone(), launch_selections.clone());
            }
        }
        if let Some(old_sid) = predecessor {
            if let Ok(mut map) = self.attach.pending_lineage.lock() {
                map.insert(launch_token.clone(), old_sid);
            }
        }
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
            provider,
        ) {
            if let Ok(mut pending) = self.attach.pending.lock() {
                pending.remove(&launch_token);
            }
            if let Ok(mut map) = self.attach.pending_launch_args.lock() {
                map.remove(&launch_token);
            }
            if let Ok(mut map) = self.attach.pending_lineage.lock() {
                map.remove(&launch_token);
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
            .map_err(|_| LOCK_POISONED)?
            .get(&session_id)
            .cloned()
            .ok_or("PTY 会话未运行")?;
        // 有界等待入队，绝不直接写管道（理由见 ManagedPty::input_tx）。到点仍满说明
        // 子进程长时间不读 stdin（挂死/被暂停），报错比无限阻塞调用线程诚实。
        match offer_with_deadline(
            &session.input_tx,
            data.to_vec(),
            std::time::Duration::from_secs(2),
        ) {
            Offer::Sent => Ok(()),
            Offer::Disconnected => Err("PTY 输入通道已关闭".into()),
            Offer::TimedOut => Err(INPUT_BACKLOGGED.into()),
        }
    }

    pub(crate) fn resize(&self, session_id: i64, cols: u16, rows: u16) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| LOCK_POISONED)?
            .get(&session_id)
            .cloned()
            .ok_or("PTY 会话未运行")?;
        let clamped = size(cols, rows);
        // 同值短路：前端切视图/attach 客户端并存时会重复下发同一尺寸，每次都过
        // master.resize = 一发 SIGWINCH（TUI 整屏重排）+ 扫描位点清零，纯浪费还闪屏。
        let packed = pack_size(clamped.cols, clamped.rows);
        if session.last_size.load(Ordering::Acquire) == packed {
            return Ok(());
        }
        // 有界拿锁：ResizePseudoConsole 在 conhost 僵死时**永不返回**，第一个撞上的调用
        // 会握着 master 锁卡死在 syscall 里（牺牲掉自己所在的 IPC/attach 线程）。后来者
        // 必须在这里快速失败，不能一个个排进同一把锁把线程池搭光；升级链/收尾同样等着
        // 这把锁的有界让路（见 escalate_stop / finalize_exit）。
        let result = lock_within(&session.master, std::time::Duration::from_millis(500))
            .ok_or(RESIZE_BUSY)?
            .as_ref()
            .ok_or("PTY 已结束")?
            .resize(clamped)
            .map_err(|e| e.to_string());
        // 屏幕状态机与 PTY 同尺寸，否则 TUI 按新宽度重排后 grid 里全是错位文本。
        // 同时清掉扫描位点：新尺寸下 grid 重排，即便字节数没变也该重判一次
        //（正常路径上 SIGWINCH 会触发 TUI 重绘、字节数自然会变，这里是兜底）。
        if result.is_ok() {
            session.last_size.store(packed, Ordering::Release);
            if let Some(parser) = session.probe.parser.as_ref() {
                if let Ok(mut parser) = parser.lock() {
                    parser.screen_mut().set_size(clamped.rows, clamped.cols);
                }
                session.probe.scanned_end.store(0, Ordering::Release);
            }
        }
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
                let skip = since.saturating_sub(start).min(b.len() as u64) as usize;
                // since 超前于 end（会话被重置）时 skip 会被夹到 len，退化为空增量；
                // 此时 start_offset == end_offset，前端据此识别并重新对齐。
                // as_slices + extend_from_slice 确定走 memcpy——iter().skip().collect()
                // 是否被特化成块拷贝取决于 std 版本，不赌。
                let (front, back) = b.as_slices();
                let mut data: Vec<u8> = Vec::with_capacity(b.len() - skip);
                if skip < front.len() {
                    data.extend_from_slice(&front[skip..]);
                    data.extend_from_slice(back);
                } else {
                    data.extend_from_slice(&back[skip - front.len()..]);
                }
                (data, start + skip as u64, end)
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
        // 隐藏态的前端要拿它把 xterm 网格钉到 PTY 真实尺寸（0 = 未知，前端跳过）。
        // 已退出的定格快照不再有新帧，尺寸没有对齐价值，维持 0。
        let (cols, rows) = unpack_size(
            session
                .as_ref()
                .map_or(0, |s| s.last_size.load(Ordering::Acquire)),
        );
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
            // 本 GUI 托管的 PTY:活着才算「托管中」。已退出的定格快照不许再让前端把它
            // 当可写终端(恢复流程按 managed 判断要不要真正拉起新进程)。
            managed: session.is_some(),
            data: base64::engine::general_purpose::STANDARD.encode(data),
            start_offset,
            end_offset,
            exited: completed.is_some(),
            exit_code: completed.and_then(|item| item.code),
            cols,
            rows,
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
            .map_err(|_| LOCK_POISONED)?
            .get(&session_id)
            .cloned()
            .ok_or("PTY 会话未运行")?;
        // 先落时间戳再 kill：即使 kill 无效（见下），waiter 的升级链也已武装，收尾有保证。
        // get_or_insert：重复点击不重置计时，否则连点会把升级一直往后推。
        if let Ok(mut at) = session.stop_requested_at.lock() {
            at.get_or_insert_with(std::time::Instant::now);
        }
        // kill 的结果不上抛：Windows 上 portable-pty 0.9 恒返回 Ok（返回值判断反了又被
        // .ok() 吞掉），本就不代表进程真死；而报错会让前端退回可点的「结束会话」按钮，
        // 与「升级链已在推进」的实际状态脱节。真正的保证在 waiter：T1 仍活 → 杀树 +
        // ClosePseudoConsole；T2 仍活 → 强制 finalize_exit。
        if let Ok(mut child) = session.child.lock() {
            let _ = child.kill();
        }
        // kill 生效的正常路径下，收尾由 waiter 在 ~200ms 内接手（finalize_exit）。
        Ok(())
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
            .map_err(|_| LOCK_POISONED)? = Some(endpoint);
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
            // 线程 per 连接且鉴权在 spawn 之后：loopback 上任何本地进程都能 connect，
            // 10s 握手读超时只保证单个连接不永久占线程，挡不住窗口内的无界堆积——
            // 失控进程反复建连即可耗尽线程数。名额封顶，超了直接关连接（正常客户端会重试）。
            let inflight = Arc::new(AtomicUsize::new(0));
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    // accept 出错（句柄耗尽等）多为持续状态，裸 continue 会热旋占满一个核。
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                };
                // 单 accept 线程，load 后再 add 无竞争窗口。
                if inflight.load(Ordering::Acquire) >= MAX_ATTACH_HANDSHAKES {
                    drop(stream);
                    continue;
                }
                inflight.fetch_add(1, Ordering::AcqRel);
                let slot = HandshakeSlot(inflight.clone());
                let broker = broker.clone();
                std::thread::spawn(move || {
                    let _ = broker.handle_attach(stream, Some(slot));
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
            // ClosePseudoConsole 对僵死的 conhost 永不返回，退出路径同样不许被它拖死
            //（表现为窗口没了、进程残留）：句柄批量取出丢牺牲线程。正常情况毫秒级关完、
            // 赶在进程退出前生效；僵死情况同步关也一样关不掉，不欠新的孤儿。
            let mut masters = Vec::new();
            for (_, managed) in sessions.drain() {
                // 锁有界（保证路径纪律，见 lock_within）：拿不到 child 锁时 kill_descendants
                // 仍按 pid 兜杀，直接子进程随后被杀树/句柄回收覆盖。
                if let Some(mut child) =
                    lock_within(&managed.child, std::time::Duration::from_millis(500))
                {
                    let _ = child.kill();
                }
                // kill 只及直接子进程（portable-pty 不建 Job Object）：agent 拉起的
                // MCP server 等孙进程要按快照兜杀，否则应用退出后残留在系统里。
                if let Some(pid) = managed.child_pid {
                    crate::proc::kill_descendants(pid);
                }
                // conhost 先杀再关句柄：僵死的 conhost 会让下面牺牲线程里的
                // ClosePseudoConsole 永不返回、孤儿常驻；先 TerminateProcess 它，
                // 句柄关闭立即完成，agent 若还挂在内核等待上也随之解除。
                // 反正整个会话都在退出，强杀与优雅关闭殊途同归。
                if let Some(pid) = managed.conhost_pid {
                    crate::proc::kill_pid(pid);
                }
                // 同 stop：不关伪终端的话 conhost 会作为孤儿留在系统里。
                if let Some(mut master) =
                    lock_within(&managed.master, std::time::Duration::from_millis(500))
                {
                    masters.push(master.take());
                }
            }
            if !masters.is_empty() {
                std::thread::spawn(move || drop(masters));
            }
        }
    }

    /// chat 窗终端视图声明「正在看」的会话（register_terminal_viewer 命令）。
    /// emitter 只对它推送实时帧，见 [`Self::viewed_session`]。
    pub(crate) fn set_viewer(&self, session_id: i64) {
        self.viewed_session.store(session_id, Ordering::Release);
    }

    /// 注销「正在看」，带 CAS 语义：只在 viewed 仍是自己时清零——React 重挂时旧实例的
    /// cleanup 可能晚于新实例的注册落地，无条件清零会把新注册抹掉，实时流就此断掉
    /// （只剩快照兜底，表现为终端每 80ms 一跳的卡顿画面）。
    pub(crate) fn clear_viewer(&self, session_id: i64) {
        let _ = self.viewed_session.compare_exchange(
            session_id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
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

    /// 该会话此刻的外部同步终端（attach 客户端）状态，在线判定与激活目标一次锁内取齐
    /// （拆开取会被 detach 竞态穿插，见 [`ExternalViewer`]）。
    ///
    /// 订阅表只在 handle_attach 里增删：客户端断开（外部终端窗口被关）时 read 循环 EOF、
    /// 同一函数尾部立即摘除订阅——判定与「确有活的外部视图」严格同步，可作「再点卡片该
    /// 聚焦已有窗口还是新开一个」的判据。对话窗口不经 socket 订阅，不计入。激活目标取
    /// 最近一个上报 pid 的订阅者，供反查实际宿主，绝不看「恢复终端」设置——设置与视图
    /// 实际所在的应用可能不一致。
    ///
    /// Windows/macOS 的 attach 查重共用（见 attach_in_external_terminal）；
    /// 逻辑平台无关，留在 cfg 外全平台编译与单测。
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub(crate) fn external_viewer(&self, session_id: i64) -> ExternalViewer {
        let Some(session) = self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(&session_id).cloned())
        else {
            return ExternalViewer::None;
        };
        let Ok(subscribers) = session.subscribers.lock() else {
            return ExternalViewer::None;
        };
        if subscribers.is_empty() {
            return ExternalViewer::None;
        }
        match subscribers.iter().rev().find_map(|subscriber| subscriber.pid) {
            Some(pid) => ExternalViewer::Pid(pid),
            None => ExternalViewer::Legacy,
        }
    }

    /// 所有托管会话已发布的屏幕状态（sid → "working"|"idle"|"blocked"）。
    /// 看板列表 DTO 消费；只含仍在运行且已过启动宽限、有规则集的会话。
    pub(crate) fn screen_states(&self) -> HashMap<i64, &'static str> {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter_map(|(sid, managed)| {
                        let state = managed.probe.debounce.lock().ok()?.published()?;
                        Some((*sid, state.as_str()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 诊断用：某会话的末屏快照与 provider 标签（explain 命令现场重跑规则）。
    pub(crate) fn screen_probe_snapshot(
        &self,
        session_id: i64,
    ) -> Option<(crate::detect::ScreenSnapshot, String)> {
        let managed = self
            .sessions
            .lock()
            .ok()?
            .get(&session_id)
            .cloned()?;
        let snapshot = managed.probe.snapshot()?;
        Some((snapshot, managed.probe.provider.clone()))
    }

    /// 启动屏幕检测节拍线程（幂等）。每 [`DETECT_TICK`] 过一遍所有托管会话：
    /// 无新输出且无待确认降级的会话整轮跳过，空闲时近乎零开销。任何会话状态变化
    /// 都合流成一次看板刷新（emit_board_changed 自带 300ms 合并窗口）。
    pub(crate) fn start_screen_detect(&self) {
        if self.detect_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let broker = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(DETECT_TICK);
            if broker.shutting_down.load(Ordering::Acquire) {
                return;
            }
            let sessions: Vec<Arc<ManagedPty>> = broker
                .sessions
                .lock()
                .map(|sessions| sessions.values().cloned().collect())
                .unwrap_or_default();
            let now = std::time::Instant::now();
            let changed = sessions
                .iter()
                .filter(|managed| {
                    let end = managed.output_end.load(Ordering::Acquire);
                    managed.probe.tick(end, now).is_some()
                })
                .count()
                > 0;
            if changed {
                if let Some(app) = broker.attach.app.lock().ok().and_then(|app| app.clone()) {
                    crate::watch::emit_board_changed(&app, "screen-state");
                }
            }
        });
    }

    /// attach 前置校验：会话确实由本进程的 PTY 持有，且 attach 服务已在监听。
    /// 刻意不返回 endpoint/token——它们经 discovery 文件（unix 下 0600）交给客户端，
    /// 不进外部终端的进程参数（argv 对同机其他进程可见，token 等于 PTY 完全接管权）。
    pub(crate) fn ensure_attachable(&self, session_id: i64) -> Result<(), String> {
        if !self
            .sessions
            .lock()
            .map_err(|_| LOCK_POISONED)?
            .contains_key(&session_id)
        {
            return Err("该会话尚未由 Meowo 接管".into());
        }
        self.attach
            .endpoint
            .lock()
            .map_err(|_| LOCK_POISONED)?
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

    /// 此刻挂着同步题面(AskUserQuestion)的会话(批量版,供远程徽标扫描)。
    /// 与 [`Self::interactive_question`] 同一 TTL 口径:过期条目就地清掉,不点亮徽标。
    pub(crate) fn interactive_question_session_ids(&self) -> HashSet<i64> {
        let Ok(mut questions) = self.attach.interactive_questions.lock() else {
            return HashSet::new();
        };
        let now = crate::now_ms();
        questions.retain(|_, (_, at)| now.saturating_sub(*at) < INTERACTIVE_QUESTION_TTL_MS);
        questions.keys().copied().collect()
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
            .map_err(|_| LOCK_POISONED)?;
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
            .map_err(|_| LOCK_POISONED)?;
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

    /// 该会话待处理的 AskUserQuestion 题面（前端轮询用，补 emit 丢失的事件）。
    ///
    /// 超过 [`INTERACTIVE_QUESTION_TTL_MS`] 的条目视为过期并顺手清掉：题面卡的去留最终由
    /// 前端的收卡信号决定（作答/取消后 transcript 增长或回合结束），后端无从得知用户
    /// 何时答完，只能靠 TTL 兜底——与前端的 180s 兜底同一量级，早于它过期即可。
    pub(crate) fn interactive_question(&self, session_id: i64) -> Option<ApprovalRequest> {
        let mut questions = self.attach.interactive_questions.lock().ok()?;
        let now = crate::now_ms();
        questions.retain(|_, (_, at)| now.saturating_sub(*at) < INTERACTIVE_QUESTION_TTL_MS);
        questions
            .get(&session_id)
            .map(|(request, _)| request.clone())
    }

    /// 用户已处理完该题面（前端收卡时调用）：撤下表，避免轮询把答过的题重新弹出来。
    pub(crate) fn clear_interactive_question(&self, session_id: i64) {
        if let Ok(mut questions) = self.attach.interactive_questions.lock() {
            questions.remove(&session_id);
        }
    }

    /// 对话窗/手机端此刻停在哪些会话上。审批消费者租约由对话页的 useEffect 在切到某
    /// 会话后注册、切走时注销，语义正是「有个视图正显示这条会话」——通知抑制复用它当
    /// 「用户正在看」的信号，不必再造一套上报。手机端新鲜租约同样算「在看」（人在
    /// 手机上看着，桌面 toast 不必弹）；过期远端租约不算。
    pub(crate) fn viewed_session_ids(&self) -> HashSet<i64> {
        let now = crate::now_ms();
        self.attach
            .approval_consumers
            .lock()
            .map(|consumers| {
                consumers
                    .iter()
                    // 用更严的 viewing 判活(非 60s 租约):兜里的手机不该压住桌面 toast。
                    .filter(|(id, lease)| consumer_lease_viewing(id, lease.seen_ms, now))
                    .map(|(_, lease)| lease.session_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn has_approval_consumer(&self, session_id: i64) -> bool {
        let now = crate::now_ms();
        self.attach.approval_consumers.lock().is_ok_and(|consumers| {
            consumers.iter().any(|(id, lease)| {
                lease.session_id == session_id && consumer_lease_fresh(id, lease.seen_ms, now)
            })
        })
    }

    /// 手机端是否正看着这条会话（新鲜的 `remote:` 租约）。审批/提问闸门据此
    /// 短路：远端就位时请求直接可领取，且**不召唤桌面窗**——人在手机上，弹桌面窗
    /// 是夺屏（与「用户正盯着别的会话不切窗」同一条产品判断）。
    fn has_fresh_remote_consumer(&self, session_id: i64) -> bool {
        let now = crate::now_ms();
        self.attach.approval_consumers.lock().is_ok_and(|consumers| {
            consumers.iter().any(|(id, lease)| {
                lease.session_id == session_id
                    && is_remote_consumer(id)
                    && consumer_lease_fresh(id, lease.seen_ms, now)
            })
        })
    }

    /// 有任何**桌面**会话的消费者租约 = 对话窗的 WebView 活着且正显示着某条会话。
    /// 与 [`crate::window::chat_window_in_view`] 合取才构成「桌面用户正在看」：
    /// 隐藏到托盘的窗口租约仍在（只有销毁才清），单凭租约不能断定有人注视。
    /// **排除远端租约**：它表示「手机在看某会话」，与「桌面窗此刻是否被注视」无关。
    /// 若把远端租约算进来，别的会话有手机在看时会把本会话的召唤从 Summon 误降为 Hold
    /// （桌面用户其实没在看任何会话，却不切窗过去，审批空等 300s）。
    fn has_any_approval_consumer(&self) -> bool {
        let now = crate::now_ms();
        self.attach.approval_consumers.lock().is_ok_and(|consumers| {
            consumers.iter().any(|(id, lease)| {
                !is_remote_consumer(id) && consumer_lease_fresh(id, lease.seen_ms, now)
            })
        })
    }

    /// 注册即续约：同 id 重复注册刷新 seen_ms（远端 20s 心跳走的就是这条）。
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
            .map_err(|_| LOCK_POISONED.to_string())?
            .insert(
                consumer_id,
                ConsumerLease {
                    session_id,
                    seen_ms: crate::now_ms(),
                },
            );
        Ok(())
    }

    /// 对话窗被销毁时的租约兜底。租约平时靠前端卸载时 unregister，但窗口销毁瞬间
    /// 那次 IPC 未必执行得到；残留租约会让 `ensure_approval_window` 误判「有 GUI
    /// 消费者」而把审批入队空等 300s，而不是立即交还 TUI。
    ///
    /// 只清**桌面**租约：chat 窗是单例，非 `remote:` 的 consumer 都属于它；手机端
    /// 新鲜租约与桌面窗生命周期无关，不许被关窗连坐（过期远端租约顺手清掉）。
    /// 随后仅把「无任何新鲜消费者」会话的挂起审批交还 TUI——手机端正看着的会话
    /// 继续可领取。
    pub(crate) fn release_desktop_consumers(&self) {
        let now = crate::now_ms();
        if let Ok(mut consumers) = self.attach.approval_consumers.lock() {
            consumers.retain(|id, lease| {
                is_remote_consumer(id) && consumer_lease_fresh(id, lease.seen_ms, now)
            });
        }
        self.pass_approvals_without_consumer();
    }

    /// 把没有任何新鲜消费者的会话的挂起审批交还各自 TUI。
    fn pass_approvals_without_consumer(&self) {
        let watched = self.viewed_session_ids();
        let pending = self
            .attach
            .approvals
            .lock()
            .map(|mut approvals| {
                let ids = approvals
                    .iter()
                    .filter(|(_, item)| !watched.contains(&item.request.session_id))
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

    pub(crate) fn unregister_approval_consumer(&self, consumer_id: &str) {
        let now = crate::now_ms();
        let session_id = self
            .attach
            .approval_consumers
            .lock()
            .ok()
            .and_then(|mut consumers| {
                let lease = consumers.remove(consumer_id)?;
                // 只剩过期远端租约也算「没人看了」：它们不会来领卡，压着不放只会空等 300s。
                let still_watched = consumers.iter().any(|(id, other)| {
                    other.session_id == lease.session_id
                        && consumer_lease_fresh(id, other.seen_ms, now)
                });
                (!still_watched).then_some(lease.session_id)
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

    fn handle_attach(
        &self,
        mut stream: TcpStream,
        handshake_slot: Option<HandshakeSlot>,
    ) -> Result<(), String> {
        stream.set_nodelay(true).ok();
        // 握手必须限时：loopback 上任何本地进程都能 connect，不设读超时的话，连上后
        // 一言不发就永久占住一个 handler 线程（每连接一线程），反复建连即可耗尽线程数。
        // 认证通过进入转发模式后再放开（attach 空闲时本来就没有输入帧）。
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let handshake = read_handshake(&mut stream).map_err(|e| e.to_string())?;
        // 握手字节已收齐——名额到此归还。后面是即时的解析/鉴权，或已认证的长驻转发/
        // 审批等待，那些不该占握手名额（外部终端可以合法开很多个）。
        drop(handshake_slot);
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
        let session = match self
            .sessions
            .lock()
            .map_err(|_| LOCK_POISONED.to_string())
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
            // as_slices + extend_from_slice 确定走 memcpy（与 snapshot 同款理由）：
            // 这里正持着 backlog + subscribers 两把锁，逐字节拷 1MiB 会把 PTY 读线程
            // 卡在门外一整段肉眼可感的时间。
            let (front, back) = backlog.as_slices();
            let mut data: Vec<u8> = Vec::with_capacity(backlog.len());
            data.extend_from_slice(front);
            data.extend_from_slice(back);
            data
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

        // 尺寸对齐放在订阅+回放**之后**且只 best-effort：resize 要过 master 锁 + 一次
        // ResizePseudoConsole，conhost 僵死时两者都可能卡死/失败。此前它排在回放之前
        // 还带一票否决（refuse），僵死会话的外部终端于是一个字节都收不到——纯黑屏，
        // 连「定格的最后画面」这条诊断线索都没有（实拍）。backlog 在我们自己内存里，
        // 与 conhost 死活无关；失败的代价只是尺寸不齐，画面凑合能看、回放照常。
        // 会话真不存在的拒绝由上方 sessions 查询兜底，不靠 resize 探测。
        if let Err(error) = self.resize(session_id, cols, rows) {
            eprintln!("attach 握手 resize 失败（尺寸未对齐，回放照常）: {error}");
        }

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
                // 输入积压 / 尺寸通道忙都是**临时**状态（agent 正忙、或另一次 resize 卡在
                // syscall 里）：丢掉这一帧继续转发，别把整条镜像连接杀掉——外部同步终端
                // 因为打字太快被整个断开、画面定格，比丢几个按键糟得多。其余错误
                // （会话没了 / 输入通道已关闭）才是终态，照旧断开清理。
                if error == INPUT_BACKLOGGED || error == RESIZE_BUSY {
                    continue;
                }
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

    /// claim 的 sessions 锁内段：目标 id 已被占用 → Err；`from_id` 已登记 → 在同一锁程内
    /// 完成「取出 + 改 id + 重绑」，外部观察者看不到空窗；尚未登记 → Ok(None)，由调用方
    /// 决定等（启动占位还在）还是报错（真的已结束）。首次认领时 `from_id` 是负数临时 id；
    /// /clear 换代时是被接替的旧真实 id（见 [`Self::handle_reclaim`]）。
    fn try_claim_rebind(
        &self,
        from_id: i64,
        to_id: i64,
    ) -> Result<Option<Arc<ManagedPty>>, String> {
        let mut sessions = self.sessions.lock().map_err(|_| LOCK_POISONED)?;
        if sessions.contains_key(&to_id) {
            return Err("真实 PTY 会话已存在".into());
        }
        let Some(managed) = sessions.remove(&from_id) else {
            return Ok(None);
        };
        managed.session_id.store(to_id, Ordering::Release);
        sessions.insert(to_id, managed.clone());
        Ok(Some(managed))
    }

    fn handle_claim(&self, token: &str, launch_token: &str, real_id: i64) -> Result<(), String> {
        if token != self.attach.token {
            return Err("PTY claim 认证失败".into());
        }
        if real_id <= 0 {
            return Err("PTY claim session 无效".into());
        }
        let temp_id = *self
            .attach
            .pending
            .lock()
            .map_err(|_| LOCK_POISONED)?
            .get(launch_token)
            .ok_or("PTY claim token 无效或已过期")?;
        // launch token 不消费，随 PTY 生命周期存续（退出路径清理）：agent 在同一进程里
        // /clear 换新会话时，SessionStart 会带着继承的同一 token 再次认领。历次认领的
        // 语义由 bindings 区分——无绑定 = 启动首次认领；同 id 重放（compact 后补发等）
        // 幂等成功；异 id = 原地换代，PTY 从旧会话行换绑到新行。
        let prior = self
            .attach
            .bindings
            .lock()
            .map_err(|_| LOCK_POISONED)?
            .get(&temp_id)
            .copied();
        if let Some(old_sid) = prior {
            if old_sid == real_id {
                return Ok(());
            }
            return self.handle_reclaim(temp_id, old_sid, real_id);
        }
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
                .map_err(|_| LOCK_POISONED)?
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
        if let Ok(mut bindings) = self.attach.bindings.lock() {
            bindings.insert(temp_id, real_id);
        }
        // 新建时的选项选择此刻才有会话行可写：落进 sessions.launch_args（JSON 对象），
        // resume/接管重启进程时经插件声明表翻译回放（见 terminal.rs 的 splice_stored_launch_args）。
        // 本函数跑在 attach 连接的 handler 线程上，DB 写不碰主线程消息泵。
        let stored = self
            .attach
            .pending_launch_args
            .lock()
            .ok()
            .and_then(|mut map| map.remove(launch_token));
        if let Some(args) = stored.filter(|a| !a.is_empty()) {
            if let (Ok(store), Ok(json)) = (
                crate::open_store(&crate::db_path()),
                serde_json::to_string(&args),
            ) {
                let _ = store.set_session_launch_args(real_id, &json);
            }
        }
        // 跨 provider 切换的接续链此刻才有会话行可指：双向成对写入（set_session_lineage
        // 单事务）。best-effort 容错与 launch_args 同款——落库失败只丢「同一张卡」的
        // 视觉折叠，不阻断认领；分叉被 store 层拒绝时同理（并发切换只有先到者成链）。
        let lineage = self
            .attach
            .pending_lineage
            .lock()
            .ok()
            .and_then(|mut map| map.remove(launch_token));
        if let Some(old_sid) = lineage {
            if let Ok(store) = crate::open_store(&crate::db_path()) {
                if let Err(e) = store.set_session_lineage(real_id, old_sid) {
                    eprintln!("接续链落库失败（{old_sid} → {real_id}）: {e}");
                }
            }
        }
        // 保持 Arc 活到映射完成，避免极短命进程在重绑边界提前析构。
        drop(managed);
        Ok(())
    }

    /// /clear 换代：agent 进程原地开新会话（新 cc_session_id → reporter 建了新 DB 行），
    /// PTY 却还绑在旧行上——不换绑的话旧卡片拿着活终端但状态定格，新会话则被当成
    /// 外部起的、只给「在外部终端同步打开」（实拍反馈）。这里把 PTY 换绑到新行，旧行
    /// 标记结束并写接续链（superseded_by），看板折叠成同一张卡；开着旧段的对话窗靠
    /// 轮询 header 里的 supersededBy 自动跟随跳转。
    fn handle_reclaim(&self, temp_id: i64, old_sid: i64, new_sid: i64) -> Result<(), String> {
        // 首次认领已完成（bindings 有值），不存在 starting 占位竞态，无需等待循环。
        let managed = self
            .try_claim_rebind(old_sid, new_sid)?
            .ok_or("旧 PTY 会话已结束")?;
        if let Ok(mut bindings) = self.attach.bindings.lock() {
            bindings.insert(temp_id, new_sid);
        }
        // DB 收尾 best-effort（与首次认领的 launch_args 落库同款容错）：失败只丢同卡
        // 折叠与选项回放，不阻断换绑——PTY 归属是主语义，必须先落。
        // 「旧段结束 + 启动选项复制 + 接续链成对写入」在 store 层一个事务里原子完成
        // （supersede_session）：此前是三笔独立事务，中途失败留半态——孤立的旧活卡，
        // 或用户看不见、reaper 却还管着的行。整体失败时旧段由 agent 的 SessionEnd(clear)
        // hook 兜底收尾，不依赖到达顺序。
        match crate::open_store(&crate::db_path()) {
            Ok(store) => {
                if let Err(e) = store.supersede_session(old_sid, new_sid, crate::now_ms()) {
                    eprintln!("换代收尾落库失败（{old_sid} → {new_sid}）: {e}");
                }
            }
            Err(e) => eprintln!("换代后打开数据库失败（{old_sid} → {new_sid}）: {e}"),
        }
        if let Some(app) = self.attach.app.lock().ok().and_then(|app| app.clone()) {
            crate::watch::emit_board_changed(&app, "pty-reclaim");
        }
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
        // 外部终端跑的会话（不是本 broker 托管的 PTY）：审批与提问一律留在终端处理。
        // 用户就坐在那个终端前，TUI 的权限框/提问表单当场可答；GUI 这边弹卡、召唤
        // 对话窗反而是打扰——尤其审批会被 GUI 租约抢走挂起，终端里那个正等着的人
        // 看不到任何权限框（实拍反馈）。提问回 allow（表单零延迟出现）、审批回 pass
        // （决定权交还 agent 自己的界面），两者都不入表、不发事件、不动窗口。
        let externally_run = !self
            .sessions
            .lock()
            .is_ok_and(|sessions| sessions.contains_key(&request.session_id));
        if externally_run {
            let decision = if request.tool_name == "AskUserQuestion" {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Pass
            };
            return stream
                .write_all(format!("{}\n", decision.as_wire()).as_bytes())
                .map_err(|e| e.to_string());
        }
        // AskUserQuestion 不是权限请求而是提问：「允许提问」这个决定没有信息量（且它属于
        // 「需用户交互」的工具，连 bypassPermissions 都拦不住触发），弹一张 JSON 审批卡
        // 纯属摩擦。直接放行让 TUI 表单立即出现；结构化题面转发给对话窗，选择列表卡从
        // 题面同步渲染，不必等屏幕识别从终端文本反推（识别只做作答就绪信号与兜底）。
        //
        // 刻意不走 ensure_approval_window 的消费者等待：放行不需要 GUI 表态，等满一个
        // 10s 窗口只会把终端表单的出现拖慢同样久。窗口没开/开晚了也无碍——表单在终端里
        // 本来就能答，事件错过时既有的屏幕识别路径仍会长出可作答的卡。
        if request.tool_name == "AskUserQuestion" {
            // 先入表再切窗：窗口冷启动期间事件会打进虚空（见 interactive_questions 的
            // 文档），前端轮询靠这张表把错过的题面补回来。
            if let Ok(mut questions) = self.attach.interactive_questions.lock() {
                questions.insert(request.session_id, (request.clone(), crate::now_ms()));
            }
            if let Some(app) = self.attach.app.lock().ok().and_then(|app| app.clone()) {
                // 与审批同一套召唤策略（approval_summon_action）：用户正盯着别的会话时
                // 不切窗只闪烁 + 亮徽标，题面已入表，用户过去时轮询可取；窗口不在眼前
                // 才切会话安静唤醒。quiet 同理不抢焦点。
                // 对话功能关闭时整段跳过：提问已放行、表单在终端里作答，弹窗与闪烁都是噪音。
                // 手机端正看着这条会话时同样整段跳过：题面已入表（上面），手机轮询可取，
                // 桌面既不切窗也不闪烁——与审批闸门的 Claimable 分支同一条判断。
                if !self.has_fresh_remote_consumer(request.session_id)
                    && crate::settings::load_settings().chat_enabled
                {
                    match approval_summon_action(
                        self.has_approval_consumer(request.session_id),
                        self.has_any_approval_consumer(),
                        crate::window::chat_window_in_view(&app),
                    ) {
                        SummonAction::Summon => {
                            crate::window::open_chat_window_quiet(app.clone(), request.session_id);
                        }
                        SummonAction::Ready | SummonAction::Hold => {}
                    }
                    if let Some(window) = app.get_webview_window("chat") {
                        let _ =
                            window.request_user_attention(Some(tauri::UserAttentionType::Critical));
                    }
                }
            }
            self.emit_approval("interactive-question", &request);
            return stream
                .write_all(format!("{}\n", ApprovalDecision::Allow.as_wire()).as_bytes())
                .map_err(|e| e.to_string());
        }
        // 先入表再等窗口：冷启动的 WebView2 完成消费者注册可能晚于任何固定等待窗口。
        // 请求在表里，晚注册的窗口靠 getPendingApproval 轮询就能找回它；反过来（等到了
        // 才入表）则超时瞬间请求人间蒸发，用户面对一扇刚弹出的空窗口。
        let (tx, rx) = mpsc::channel();
        self.attach
            .approvals
            .lock()
            .map_err(|_| LOCK_POISONED)?
            .insert(
                request.request_id.clone(),
                PendingApproval {
                    request: request.clone(),
                    response: tx,
                },
            );
        let hold = self.ensure_approval_window(request.session_id);
        if matches!(hold, ApprovalHold::Reject) {
            // 窗口没起来：撤回请求，把决定权交还 agent 自己的审批界面。
            if let Ok(mut approvals) = self.attach.approvals.lock() {
                approvals.remove(&request.request_id);
            }
            self.emit_approval("pending-approval-cleared", &request);
            return stream.write_all(b"pass\n").map_err(|e| e.to_string());
        }
        self.emit_approval("pending-approval", &request);
        // 等 GUI 决策的同时监视「已在终端结算」的信号(见 await_approval_outcome)——TUI
        // 权限框与 hook 是并行竞速的(claude 官方 hooks 文档明载),用户完全可能直接在终端
        // 作答。此前这里盲等满 300s,对话页的审批卡片残留成幽灵卡(实拍:终端已批准,切回
        // 对话页倒计时还在走)。store 打不开时探针恒 None,退化为断连+超时两路,不会更坏。
        let store = crate::open_store(&crate::db_path()).ok();
        // 仅远端 Claimable 时启用「消费者消失即放弃」:手机租约过期且本会话再无任何新鲜
        // 消费者 = 无人会来领卡,回落 TUI 好过盲等 300s。桌面路径(含召唤后 300s 保持
        // 可领取)不启用——那是刻意等用户切过来的设计。
        let remote_only = matches!(hold, ApprovalHold::RemoteClaimable);
        let outcome = await_approval_outcome(
            &rx,
            &stream,
            std::time::Instant::now() + std::time::Duration::from_secs(300),
            std::time::Duration::from_millis(500),
            || {
                store
                    .as_ref()
                    .and_then(|s| s.session_pending_review(request.session_id).ok())
                    .map(|pending| pending.is_some())
            },
            || remote_only && !self.has_approval_consumer(request.session_id),
        );
        if let Ok(mut approvals) = self.attach.approvals.lock() {
            approvals.remove(&request.request_id);
        }
        self.emit_approval("pending-approval-cleared", &request);
        let decision = match outcome {
            // 对端已死,写响应只会得到 broken pipe——收尾即可,卡片已随上面的事件清掉。
            ApprovalWait::PeerGone => return Ok(()),
            ApprovalWait::Decision(decision) => decision,
            // 别处结算时 hook 若还活着,pass 让它零决策退出——CLI 反正会丢弃迟到的 hook
            // 结果;超时/领卡方消失沿用既有语义交还终端。
            ApprovalWait::TimedOut | ApprovalWait::ResolvedElsewhere | ApprovalWait::Abandoned => {
                ApprovalDecision::Pass
            }
        };
        let _ = stream.set_nonblocking(false);
        stream
            .write_all(format!("{}\n", decision.as_wire()).as_bytes())
            .map_err(|e| e.to_string())
    }
}

/// 审批入 GUI 通道的第一道闸门（在召唤策略之前）。
#[derive(Debug, PartialEq, Eq)]
enum ApprovalGate {
    /// 手机端正看着目标会话：请求直接可领取，桌面窗一概不动。
    Claimable,
    /// 轻量模式（chat_enabled=false）且手机端不在场：回落 TUI。
    FallbackTui,
    /// 走桌面的既有召唤逻辑（approval_summon_action）。
    Desktop,
}

/// 闸门纯决策（供单测矩阵）。远端优先于 chat_enabled：chat_enabled 关的是桌面
/// 对话窗，不该一并关掉远程通道——远程有自己的开关（没开就注册不了 `remote:` 租约，
/// 第一参恒 false，行为与改动前逐字一致）。
fn approval_gate(remote_watching_target: bool, chat_enabled: bool) -> ApprovalGate {
    if remote_watching_target {
        ApprovalGate::Claimable
    } else if !chat_enabled {
        ApprovalGate::FallbackTui
    } else {
        ApprovalGate::Desktop
    }
}

/// `ensure_approval_window` 的结果:决定 await 循环用哪种耐心等待。
#[derive(Debug, PartialEq, Eq)]
enum ApprovalHold {
    /// 远端手机在看这条会话:靠它轮询来领卡。手机若消失(60s 租约过期且本会话再无任何
    /// 新鲜消费者)即放弃等待、回落 TUI——不空等满 300s(手机锁屏/被杀后审批会盲等)。
    RemoteClaimable,
    /// 桌面消费者已就位或已召唤:整个等待期(300s)保持可领取,用户可能随时切过来。
    Desktop,
    /// 没有合法消费者(轻量模式且手机不在场 / 召唤窗起不来):立即交还 TUI。
    Reject,
}

/// 审批/提问到达时对对话窗的召唤动作。
#[derive(Debug, PartialEq, Eq)]
enum SummonAction {
    /// 目标会话已有消费者（用户就在这条会话上）：无需任何窗口操作。
    Ready,
    /// 用户正盯着**别的**会话：不切窗，只闪任务栏 + 让前端亮徽标，请求保持可领取。
    Hold,
    /// 窗口不在用户眼前（隐藏/最小化/未创建）：切到目标会话安静唤醒。
    Summon,
}

/// 召唤策略的纯决策（供单测）。参数依次是：目标会话是否已有消费者租约、
/// 是否存在任何消费者租约、对话窗是否在用户眼前（可见且未最小化）。
///
/// 「有租约但窗口不在眼前」（隐藏到托盘：WebView 活着、租约未清）必须走 Summon——
/// 那时不存在可打断的注视，而 Hold 的徽标没人看得见，等同把审批藏起来。
fn approval_summon_action(
    has_target_consumer: bool,
    has_any_consumer: bool,
    window_in_view: bool,
) -> SummonAction {
    if has_target_consumer {
        SummonAction::Ready
    } else if has_any_consumer && window_in_view {
        SummonAction::Hold
    } else {
        SummonAction::Summon
    }
}

/// 审批等待的结局:GUI 决策 / 超时 / 对端断开 / 已在别处结算,四路信号谁先到算谁的。
#[derive(Debug, PartialEq, Eq)]
enum ApprovalWait {
    Decision(ApprovalDecision),
    TimedOut,
    PeerGone,
    ResolvedElsewhere,
    /// 唯一的领卡方(远端手机)消失了:放弃等待,回落 TUI(同 pass 语义)。
    Abandoned,
}

/// 审批等待循环(纯逻辑,供单测)。除 GUI 决策与超时外,还监视两路「已在终端结算」的信号:
///
/// 1. **连接断开**(PeerGone):用户在 TUI 作答后 CLI 会取消 hook,reporter 进程一死,
///    对端 EOF/RESET 经 peek 立刻可见。也覆盖 CLI 崩溃/会话被杀。
/// 2. **pending_review 从「已置位」翻到「已清除」**(ResolvedElsewhere):作答后
///    PostToolUse/Stop 落库清位,覆盖 hook 没有被杀的形态。必须先观察到置位才认清除——
///    reporter 在连 broker **之前**就已把它置位,从未见到置位只有两种解释:写库失败
///    (codex 的 hook 可能继承只读沙箱)或迟到的上一工具事件抢先清了位,这两种「清除」
///    都不是本请求的结算;认了就是把活着的审批误撤。再要求连续两拍确认,躲开毫秒级的
///    事件乱序窗口。
///
/// peek 依赖非阻塞模式;set_nonblocking 失败时跳过对端监视(阻塞 peek 会挂死循环),
/// 退化为其余三路。probe 返回 None = 本拍读不到状态(store 没开成/查询失败),跳过。
fn await_approval_outcome(
    rx: &mpsc::Receiver<ApprovalDecision>,
    stream: &TcpStream,
    deadline: std::time::Instant,
    tick: std::time::Duration,
    mut pending_review_probe: impl FnMut() -> Option<bool>,
    // 每拍探一次「该放弃了吗」:远端 Claimable 下,手机租约过期且本会话再无消费者即 true。
    // 桌面路径恒 false(整个 300s 保持可领取)。
    mut abandon_probe: impl FnMut() -> bool,
) -> ApprovalWait {
    let peer_watch = stream.set_nonblocking(true).is_ok();
    let mut seen_pending = false;
    let mut cleared_streak = 0u8;
    loop {
        match rx.recv_timeout(tick) {
            Ok(decision) => return ApprovalWait::Decision(decision),
            // tx 被清理路径摘走(会话结束清租约等):按超时语义交还终端。
            Err(mpsc::RecvTimeoutError::Disconnected) => return ApprovalWait::TimedOut,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if std::time::Instant::now() >= deadline {
            return ApprovalWait::TimedOut;
        }
        if abandon_probe() {
            return ApprovalWait::Abandoned;
        }
        if peer_watch {
            let mut probe = [0u8; 1];
            match stream.peek(&mut probe) {
                Ok(0) => return ApprovalWait::PeerGone,
                // 协议上对端此刻不会再发数据;真有字节也不影响继续等。
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return ApprovalWait::PeerGone,
            }
        }
        match pending_review_probe() {
            Some(true) => {
                seen_pending = true;
                cleared_streak = 0;
            }
            Some(false) if seen_pending => {
                cleared_streak += 1;
                if cleared_streak >= 2 {
                    return ApprovalWait::ResolvedElsewhere;
                }
            }
            _ => {}
        }
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

    /// 「活着但已收不到任何输出」的子进程：EOF 先于进程退出的场景（conhost 先死、
    /// agent 卡死）。wait 在真实世界会无限阻塞，测试里以 Err 代表「未先 kill 就挂死」。
    #[derive(Debug)]
    struct HungChild {
        killed: Arc<AtomicBool>,
    }
    impl portable_pty::ChildKiller for HungChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.killed.store(true, Ordering::Release);
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(HungChild {
                killed: self.killed.clone(),
            })
        }
    }
    impl portable_pty::Child for HungChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            // kill 生效后 try_wait 能拿到退出码——贴近真实 TerminateProcess 成功的行为。
            if self.killed.load(Ordering::Acquire) {
                Ok(Some(portable_pty::ExitStatus::with_exit_code(1)))
            } else {
                Ok(None)
            }
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            // 真实世界的 wait 对杀不死的进程会永久阻塞并拿着 child 锁不还——
            // 收尾路径绝不许再调它，测试里直接以 panic 当哨兵。
            panic!("reap_child 不得再调无限阻塞的 wait()");
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    /// 「TerminateProcess 静默无效」的子进程：kill 恒返回 Ok（portable-pty 0.9 的谎报），
    /// 但 try_wait 永远拿不到退出——卡死在 ConPTY 内核 I/O 的真实场景。
    #[derive(Debug)]
    struct NeverDiesChild {
        killed: Arc<AtomicBool>,
    }
    impl portable_pty::ChildKiller for NeverDiesChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.killed.store(true, Ordering::Release);
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(NeverDiesChild {
                killed: self.killed.clone(),
            })
        }
    }
    impl portable_pty::Child for NeverDiesChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(None)
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            panic!("reap_child 不得再调无限阻塞的 wait()");
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    /// 回归：EOF 收尾遇到还活着的子进程时必须先 kill 再收尸，不得阻塞 wait——
    /// 真实翻车过：conhost 先死、claude 卡死，finalize_exit 拿着 child 锁在 wait 上
    /// 永久不还，stop()（结束终端/结束会话）在同一把锁上排队，按钮点了毫无反应。
    #[test]
    fn reap_child_kills_live_child_instead_of_blocking_wait() {
        let killed = Arc::new(AtomicBool::new(false));
        let mut child: Box<dyn portable_pty::Child + Send + Sync> = Box::new(HungChild {
            killed: killed.clone(),
        });
        let code = reap_child(child.as_mut());
        assert!(killed.load(Ordering::Acquire), "活着的子进程应先被 kill");
        assert_eq!(code, Some(1), "kill 后应立即收到退出码");
    }

    /// 回归：TerminateProcess 静默无效（卡死在 ConPTY 内核 I/O）时，收尸必须在超时后
    /// 放弃返回 None，而不是无限阻塞在 wait() 上拿着 child 锁不还。
    #[test]
    fn reap_child_gives_up_on_unkillable_child() {
        let killed = Arc::new(AtomicBool::new(false));
        let mut child: Box<dyn portable_pty::Child + Send + Sync> = Box::new(NeverDiesChild {
            killed: killed.clone(),
        });
        let start = std::time::Instant::now();
        let code = reap_child_within(
            child.as_mut(),
            std::time::Duration::from_millis(30),
            std::time::Duration::from_millis(50),
        );
        assert!(killed.load(Ordering::Acquire), "宽限到点应尝试 kill");
        assert_eq!(code, None, "杀不死就放弃退出码");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "必须在超时上限附近返回，不得无限等待"
        );
    }

    /// 有界投递的三种结局。队满超时必须在上限附近返回——这是输出链不反压的根基：
    /// emitter/UI 卡死时 reader 靠它丢帧保读，agent 才不会被 meowo 自己拖成假死。
    #[test]
    fn offer_with_deadline_times_out_when_full() {
        let (tx, rx) = mpsc::sync_channel::<u8>(1);
        assert_eq!(
            offer_with_deadline(&tx, 1, std::time::Duration::from_millis(50)),
            Offer::Sent
        );
        let start = std::time::Instant::now();
        assert_eq!(
            offer_with_deadline(&tx, 2, std::time::Duration::from_millis(50)),
            Offer::TimedOut,
            "队满且无人消费:到点放弃"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "必须在超时上限附近返回"
        );
        drop(rx);
        assert_eq!(
            offer_with_deadline(&tx, 3, std::time::Duration::from_millis(50)),
            Offer::Disconnected
        );
    }

    /// 注销「正在看」必须是 CAS：React 重挂时旧实例的 cleanup 可能晚于新实例的注册，
    /// 无条件清零会抹掉新注册、实时流断掉（快照兜底下画面 80ms 一跳）。
    #[test]
    fn viewer_clear_is_cas_scoped_to_own_registration() {
        let broker = PtyBroker::default();
        broker.set_viewer(7);
        broker.set_viewer(9); // 新实例先注册
        broker.clear_viewer(7); // 旧实例的迟到注销
        assert_eq!(broker.viewed_session.load(Ordering::Acquire), 9);
        broker.clear_viewer(9);
        assert_eq!(broker.viewed_session.load(Ordering::Acquire), 0);
    }

    /// 有界拿锁的三种结局。占用超时必须在上限附近返回——这是保证路径（waiter 升级链 /
    /// finalize / shutdown）不被卡死在 ConPTY syscall 里的持锁者拖死的根基
    /// （0.5.13「结束会话仍无效」的回归）。中毒按全仓策略恢复，破坏性收尾不因一次
    /// 别处 panic 永久失效。
    #[test]
    fn lock_within_gives_up_on_held_lock_and_recovers_poison() {
        let mutex = Arc::new(Mutex::new(0u8));
        assert!(lock_within(&mutex, std::time::Duration::from_millis(20)).is_some());
        let guard = mutex.lock().unwrap();
        let start = std::time::Instant::now();
        assert!(
            lock_within(&mutex, std::time::Duration::from_millis(50)).is_none(),
            "锁被占:到点放弃"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "必须在超时上限附近返回"
        );
        drop(guard);
        let poisoner = mutex.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("毒化锁");
        })
        .join();
        assert!(
            lock_within(&mutex, std::time::Duration::from_millis(20)).is_some(),
            "中毒恢复,不永久失效"
        );
    }

    /// stop 升级链的档位判定：纯时间点注入，避开线程/时序 flakiness。
    #[test]
    fn stop_stage_escalates_by_elapsed_time() {
        let now = std::time::Instant::now();
        assert_eq!(stop_stage(None, now), StopStage::Wait, "未请求 stop 不升级");
        assert_eq!(stop_stage(Some(now), now), StopStage::Wait);
        assert_eq!(
            stop_stage(Some(now), now + std::time::Duration::from_millis(500)),
            StopStage::Wait
        );
        assert_eq!(
            stop_stage(Some(now), now + STOP_KILL_TREE_AFTER),
            StopStage::KillTree
        );
        assert_eq!(
            stop_stage(Some(now), now + STOP_FORCE_FINALIZE_AFTER),
            StopStage::ForceFinalize
        );
    }

    /// stop() 必须先武装升级链（落时间戳）再 kill，且 kill 的谎报 Ok 不影响返回值语义。
    #[test]
    fn stop_records_request_timestamp_and_kills() {
        let broker = PtyBroker::default();
        let killed = Arc::new(AtomicBool::new(false));
        let managed = managed_with_child(
            7,
            Box::new(NeverDiesChild {
                killed: killed.clone(),
            }),
            None,
        );
        broker.sessions.lock().unwrap().insert(7, managed.clone());
        assert!(broker.stop(7).is_ok());
        assert!(killed.load(Ordering::Acquire), "stop 应发出 kill");
        let first = managed.stop_requested_at.lock().unwrap().expect("应落时间戳");
        // 重复点击不重置计时，否则连点会把升级一直往后推。
        assert!(broker.stop(7).is_ok());
        assert_eq!(managed.stop_requested_at.lock().unwrap().unwrap(), first);
    }

    /// 升级第二档：补刀 kill；pid 未知时跳过杀树，不 panic 不阻塞。
    /// （master 的 take/drop 无法在此覆盖：测试里造不出 Box<dyn MasterPty>——
    /// 其 Error 是 anyhow，非本 crate 直接依赖；该行为与 finalize_exit 同一句式。）
    #[test]
    fn escalate_stop_kills_without_tree_or_master() {
        let killed = Arc::new(AtomicBool::new(false));
        let managed = managed_with_child(
            7,
            Box::new(NeverDiesChild {
                killed: killed.clone(),
            }),
            None,
        );
        escalate_stop(&managed);
        assert!(killed.load(Ordering::Acquire), "升级档应补刀 kill");
    }

    fn dummy_managed(session_id: i64) -> Arc<ManagedPty> {
        managed_with_child(session_id, Box::new(DummyChild), None)
    }

    fn managed_with_child(
        session_id: i64,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        child_pid: Option<u32>,
    ) -> Arc<ManagedPty> {
        // rx 直接丢弃：假会话没有 writer 线程，写入以 Disconnected 快速失败即可。
        let (input_tx, _) = mpsc::sync_channel(1);
        Arc::new(ManagedPty {
            session_id: AtomicI64::new(session_id),
            master: Mutex::new(None),
            finalized: AtomicBool::new(false),
            input_tx,
            child: Mutex::new(child),
            child_pid,
            conhost_pid: None,
            stop_requested_at: Mutex::new(None),
            backlog: Mutex::new(VecDeque::new()),
            output_end: AtomicU64::new(0),
            subscribers: Mutex::new(Vec::new()),
            probe: ScreenProbe::new(24, 80, "claude".into()),
            last_size: AtomicU32::new(0),
        })
    }

    /// 测试用：往 probe 的屏幕状态机喂字节（parser 必存在，即 provider 有规则集）。
    fn feed_screen(probe: &ScreenProbe, bytes: &[u8]) {
        probe
            .parser
            .as_ref()
            .expect("该 provider 应有规则集")
            .lock()
            .expect("锁未被毒化")
            .process(bytes);
    }

    /// 并发实测：PTY reader 高频喂字节的同时检测线程持续扫描。二者共享 parser 锁
    /// （reader 每 chunk 写、ticker 每轮持锁导出整屏），锁序错了会死锁、导出期间
    /// panic 会毒化锁。这里用真实的两线程竞争跑一遍，并断言结束后状态仍正确——
    /// 静态阅读看不出锁竞争下的活性问题。
    #[test]
    fn screen_probe_survives_concurrent_feed_and_tick() {
        let probe = Arc::new(ScreenProbe::new(24, 80, "claude".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        let feeder = {
            let probe = probe.clone();
            std::thread::spawn(move || {
                let mut written = 0u64;
                while std::time::Instant::now() < deadline {
                    // 模拟构建日志刷屏：带 SGR 的整行输出 + 偶尔全屏重绘。
                    feed_screen(&probe, b"\x1b[1;32m  compiling\x1b[0m crate v0.1.0\r\n");
                    if written.is_multiple_of(50) {
                        feed_screen(&probe, b"\x1b[2J\x1b[H");
                    }
                    written += 1;
                }
                written
            })
        };
        let ticker = {
            let probe = probe.clone();
            std::thread::spawn(move || {
                let after_grace =
                    probe.started_at + DETECT_STARTUP_GRACE + std::time::Duration::from_secs(1);
                let mut ticks = 0u64;
                while std::time::Instant::now() < deadline {
                    // 每轮换一个 end 值，强制真正扫描（不走跳扫短路）。
                    probe.tick(ticks + 1, after_grace);
                    ticks += 1;
                }
                ticks
            })
        };
        let written = feeder.join().expect("feeder 不应 panic（锁未被毒化）");
        let ticks = ticker.join().expect("ticker 不应 panic");
        assert!(written > 0 && ticks > 0, "两侧都应真正跑起来");

        // 竞争结束后 parser 状态仍健康：喂进一屏审批 UI 应被正确判定。
        feed_screen(&probe, b"\x1b[2J\x1b[H");
        feed_screen(
            &probe,
            " Bash command\r\n Do you want to proceed?\r\n \u{276F} 1. Yes\r\n   2. No\r\n"
                .as_bytes(),
        );
        let after_grace = probe.started_at + DETECT_STARTUP_GRACE + std::time::Duration::from_secs(1);
        assert_eq!(
            probe.tick(u64::MAX, after_grace),
            Some(crate::detect::ScreenState::Blocked)
        );
    }

    /// OSC 9 进度序列要能从**真实字节流**走到判定：vt100 不内建处理 OSC 9，靠
    /// `unhandled_osc` 回调取出。漏采它等于让 claude 少一条兜底的 idle 判据
    /// （标题未必总带 ✳——用户自设终端标题、或 CLI 版本不写标题时就没了）。
    #[test]
    fn osc_progress_reaches_the_rules() {
        let probe = ScreenProbe::new(24, 80, "claude".into());
        let after_grace = probe.started_at + DETECT_STARTUP_GRACE + std::time::Duration::from_secs(1);
        // 屏幕上只有历史输出、标题也不带 ✳：唯一的空闲证据就是进度清零。
        feed_screen(&probe, b"\x1b]0;my-terminal\x07");
        feed_screen(&probe, "  cargo build finished\r\n".as_bytes());
        feed_screen(&probe, b"\x1b]9;4;0\x07");
        assert_eq!(
            probe.tick(1, after_grace),
            Some(crate::detect::ScreenState::Idle)
        );
        let snapshot = probe.snapshot().expect("有规则集");
        assert_eq!(snapshot.progress.as_deref(), Some("4;0"));
        assert_eq!(
            crate::detect::evaluate("claude", &snapshot).map(|e| e.rule_id()),
            Some("osc_progress_idle")
        );
    }

    /// 检测规则按调用方传入的 provider 选，与 argv[0] 无关——包装启动（npx/bunx/fnm
    /// shim/中转脚本）下 argv[0] 不是 agent 名，靠它猜会让整条会话静默失去状态检测。
    #[test]
    fn screen_probe_takes_provider_from_caller_not_argv() {
        let probe = ScreenProbe::new(24, 80, "claude".into());
        let after_grace = probe.started_at + DETECT_STARTUP_GRACE + std::time::Duration::from_secs(1);
        feed_screen(&probe, "\x1b]0;\u{2733} claude\x07".as_bytes());
        assert_eq!(
            probe.tick(1, after_grace),
            Some(crate::detect::ScreenState::Idle),
            "provider 正确时规则命中"
        );
        // provider 是包装器名（argv[0] 猜出来的典型错值）→ 无规则集：连 parser 都不建，
        // reader 热路径一个字节都不解析（门在生产端，不是消费端）。
        let mistaken = ScreenProbe::new(24, 80, "npx".into());
        assert!(
            mistaken.parser.is_none(),
            "无规则集的 provider 不该建 parser——否则每 chunk 白跑一遍 VT 状态机"
        );
        assert_eq!(
            mistaken.tick(
                1,
                mistaken.started_at + DETECT_STARTUP_GRACE + std::time::Duration::from_secs(1)
            ),
            None
        );
    }

    /// 冷启动路径：题面必须能被**轮询**取回。`interactive-question` 事件在对话窗
    /// 冷启动（WebView2 1~2s）时打进虚空且不重放，入表是唯一的兜底。会话须为本
    /// broker 托管——外部终端会话的题面留在终端作答，根本不入表。收卡时撤表，
    /// 避免轮询把答过的题重新弹成幽灵卡。
    #[test]
    fn interactive_question_survives_a_missed_event_and_clears_on_dismiss() {
        let broker = PtyBroker::default();
        broker
            .sessions
            .lock()
            .unwrap()
            .insert(11, dummy_managed(11));
        let request = ApprovalRequest {
            session_id: 11,
            request_id: "request-11-ask".into(),
            provider: "claude".into(),
            tool_name: "AskUserQuestion".into(),
            input: r#"{"questions":[{"question":"晚饭吃什么？","options":[{"label":"火锅"}]}]}"#
                .into(),
            description: None,
            permission_suggestions: vec![],
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server_side, _) = listener.accept().unwrap();
        let token = broker.attach.token.clone();
        broker
            .handle_approval(&token, request.clone(), server_side)
            .unwrap();
        drop(client);

        // 事件即使无人接收，题面仍在表里等轮询来取。
        assert_eq!(broker.interactive_question(11).as_ref(), Some(&request));
        // 别的会话取不到别人的题面。
        assert_eq!(broker.interactive_question(12), None);
        // 收卡撤表后不再返回——否则下一轮轮询会把答过的题重新弹出来。
        broker.clear_interactive_question(11);
        assert_eq!(broker.interactive_question(11), None);
    }

    /// 表里的题面按 TTL 自净：后端无从得知用户何时在终端答完，卡的去留由前端信号决定，
    /// 这里只保证不会无限堆积、也不会在前端 180s 兜底之后还残留着把旧题弹回来。
    #[test]
    fn stale_interactive_questions_expire() {
        let broker = PtyBroker::default();
        let request = ApprovalRequest {
            session_id: 13,
            request_id: "request-13-ask".into(),
            provider: "claude".into(),
            tool_name: "AskUserQuestion".into(),
            input: "{}".into(),
            description: None,
            permission_suggestions: vec![],
        };
        // 直接以「刚好超过 TTL」的时刻入表，模拟一条没人处理的陈旧题面。
        broker.attach.interactive_questions.lock().unwrap().insert(
            13,
            (request, crate::now_ms() - INTERACTIVE_QUESTION_TTL_MS - 1),
        );
        assert_eq!(broker.interactive_question(13), None, "过期题面不得返回");
        assert!(
            broker
                .attach
                .interactive_questions
                .lock()
                .unwrap()
                .is_empty(),
            "过期条目应被顺手清掉，不留堆积"
        );
    }

    /// 召唤策略:目标会话已有消费者→Ready;用户盯着别的会话(有租约+窗口在眼前)→Hold
    /// 不夺屏;窗口不在眼前(含「租约在但窗口隐藏」的托盘态)→Summon 切会话召唤。
    #[test]
    fn summon_policy_never_yanks_a_watched_window() {
        use super::{approval_summon_action, SummonAction};
        assert_eq!(approval_summon_action(true, true, true), SummonAction::Ready);
        // 在眼前与否不影响 Ready:请求靠事件+轮询送达已注册的消费者。
        assert_eq!(approval_summon_action(true, true, false), SummonAction::Ready);
        assert_eq!(approval_summon_action(false, true, true), SummonAction::Hold);
        // 租约还在但窗口收进托盘:徽标没人看得见,必须召唤。
        assert_eq!(approval_summon_action(false, true, false), SummonAction::Summon);
        assert_eq!(approval_summon_action(false, false, true), SummonAction::Summon);
        assert_eq!(approval_summon_action(false, false, false), SummonAction::Summon);
    }

    fn approval_request(session_id: i64, tool: &str) -> ApprovalRequest {
        ApprovalRequest {
            session_id,
            request_id: format!("request-{session_id}-ask"),
            provider: "claude".into(),
            tool_name: tool.into(),
            description: None,
            input: r#"{"questions":[]}"#.into(),
            permission_suggestions: vec![],
        }
    }

    fn approval_roundtrip(broker: &PtyBroker, session_id: i64, tool: &str) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server_side, _) = listener.accept().unwrap();
        let token = broker.attach.token.clone();
        broker
            .handle_approval(&token, approval_request(session_id, tool), server_side)
            .unwrap();
        let mut reply = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(client), &mut reply).unwrap();
        reply.trim().to_string()
    }

    /// AskUserQuestion 是提问不是权限：必须立即回 allow（TUI 表单零延迟），且**绝不入
    /// 审批表**——入了表 GUI 会看到一张没人能消解的幽灵审批卡。对照：普通工具在无 GUI
    /// 消费者时按既有语义回 pass 交还终端。会话为本 broker 托管（外部会话另有整段
    /// 短路，见下一个测试）。
    #[test]
    fn ask_user_question_is_auto_allowed_without_queueing() {
        let broker = PtyBroker::default();
        broker.sessions.lock().unwrap().insert(7, dummy_managed(7));
        assert_eq!(approval_roundtrip(&broker, 7, "AskUserQuestion"), "allow");
        assert!(
            broker.attach.approvals.lock().unwrap().is_empty(),
            "自动放行的提问不得滞留在审批表里"
        );
        // 普通工具不受影响：无 GUI 消费者 → 撤回并交还终端（pass）。
        assert_eq!(approval_roundtrip(&broker, 7, "Bash"), "pass");
    }

    /// 外部终端跑的会话（非本 broker 托管）：提问与审批一律留在终端处理。提问回
    /// allow（终端表单零延迟）、审批回 pass（决定权交还 agent 界面），且都不入表——
    /// 入了表对话窗就会长出题面卡/审批卡，而用户明确要求外部会话的这些交互只在
    /// 终端出现（GUI 抢走审批时，终端前的人反而看不到权限框）。
    #[test]
    fn externally_run_sessions_keep_interactions_in_the_terminal() {
        let broker = PtyBroker::default();
        assert_eq!(approval_roundtrip(&broker, 99, "AskUserQuestion"), "allow");
        assert_eq!(
            broker.interactive_question(99),
            None,
            "外部会话的题面不得入表——否则对话窗轮询会把卡弹出来"
        );
        assert_eq!(approval_roundtrip(&broker, 99, "Bash"), "pass");
        assert!(
            broker.attach.approvals.lock().unwrap().is_empty(),
            "外部会话的审批不得挂进 broker"
        );
    }

    /// 审批等待循环的四路信号。场景即实拍 bug：TUI 权限框与 hook 并行竞速，用户在终端里
    /// 批准后 GUI 卡片却要盲等满 300s 才消失——断连与 pending_review 清位两路信号都必须
    /// 能提前结束等待，且「从未见置位的清除」不得误当结算。
    #[test]
    fn approval_wait_settles_on_terminal_side_signals() {
        use std::time::{Duration, Instant};
        fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (server, _) = listener.accept().unwrap();
            (client, server)
        }
        let tick = Duration::from_millis(5);
        let far = || Instant::now() + Duration::from_secs(5);

        // 对端断开（CLI 取消 hook → reporter 进程死）→ PeerGone。
        let (client, server) = pair();
        let (_tx, rx) = mpsc::channel::<ApprovalDecision>();
        drop(client);
        assert_eq!(
            await_approval_outcome(&rx, &server, far(), tick, || Some(true), || false),
            ApprovalWait::PeerGone
        );

        // pending_review 先置位再清除（作答后 PostToolUse 落库）→ 连续两拍确认后结算。
        let (_client2, server2) = pair();
        let (_tx2, rx2) = mpsc::channel::<ApprovalDecision>();
        let mut ticks = 0;
        let outcome = await_approval_outcome(&rx2, &server2, far(), tick, move || {
            ticks += 1;
            Some(ticks <= 2) // 两拍置位，此后清除
        }, || false);
        assert_eq!(outcome, ApprovalWait::ResolvedElsewhere);

        // 从未观察到置位的「清除」不是本请求的结算（写库失败 / 迟到的上一工具事件）：
        // 只能走到超时。
        let (_client3, server3) = pair();
        let (_tx3, rx3) = mpsc::channel::<ApprovalDecision>();
        assert_eq!(
            await_approval_outcome(
                &rx3,
                &server3,
                Instant::now() + Duration::from_millis(40),
                tick,
                || Some(false),
                || false
            ),
            ApprovalWait::TimedOut
        );

        // 领卡方(远端手机)中途消失 → Abandoned(回落 TUI,不空等满超时)。
        let (_client5, server5) = pair();
        let (_tx5, rx5) = mpsc::channel::<ApprovalDecision>();
        let mut probes = 0;
        assert_eq!(
            await_approval_outcome(&rx5, &server5, far(), tick, || Some(false), move || {
                probes += 1;
                probes > 2 // 前两拍还在,之后手机租约过期
            }),
            ApprovalWait::Abandoned
        );

        // GUI 决策最高优先：先于任何监视信号被消费。
        let (_client4, server4) = pair();
        let (tx4, rx4) = mpsc::channel::<ApprovalDecision>();
        tx4.send(ApprovalDecision::Allow).unwrap();
        assert_eq!(
            await_approval_outcome(&rx4, &server4, far(), tick, || Some(true), || false),
            ApprovalWait::Decision(ApprovalDecision::Allow)
        );
    }

    /// 字节流 → 终端仿真 → 规则判定的端到端：真实风格的 ANSI（全屏重绘、光标定位、SGR、
    /// OSC 标题、跨 chunk 撕裂的多字节字符）必须被仿真层消化，规则拿到的是干净末屏——
    /// 这正是「不能对原始 backlog 做字符串匹配」的验证。
    #[test]
    fn screen_probe_detects_states_from_raw_ansi() {
        let feed = feed_screen;
        let probe = ScreenProbe::new(24, 80, "claude".into());
        // 启动宽限之外的时间基准。
        let after_grace = probe.started_at + DETECT_STARTUP_GRACE + std::time::Duration::from_secs(1);

        // working：OSC 0 标题带盲文 spinner 帧 + 带 SGR 的滚动输出，
        // 其中「构建中」两个多字节字符故意撕裂在两个 chunk 之间。
        feed(&probe, b"\x1b]0;\xe2\xa0\x8b Reticulating splines\x07");
        let text = "\u{1b}[1;32m构建中\u{1b}[0m cargo build...\r\n".as_bytes();
        let (head, tail) = text.split_at(9); // 切在多字节字符中间
        feed(&probe, head);
        feed(&probe, tail);
        assert_eq!(
            probe.tick(1, after_grace),
            Some(crate::detect::ScreenState::Working)
        );
        // 宽限期内不发布任何状态。
        let fresh = ScreenProbe::new(24, 80, "claude".into());
        assert_eq!(fresh.tick(1, fresh.started_at), None);

        // 审批弹窗：spinner 停转、标题换掉（真实 claude 行为——标题规则 1100 优先级
        // 压过屏幕规则，spinner 挂着时永远判 working），清屏重绘出对话框 → blocked
        // 立即发布（不等确认期）。
        feed(&probe, b"\x1b]0;Claude Code\x07");
        feed(&probe, b"\x1b[2J\x1b[H");
        feed(&probe, " Bash command\r\n".as_bytes());
        feed(&probe, "   cargo test --all\r\n".as_bytes());
        feed(&probe, "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\r\n".as_bytes());
        feed(&probe, " Do you want to proceed?\r\n".as_bytes());
        feed(&probe, " \u{276F} 1. Yes\r\n".as_bytes());
        feed(&probe, "   2. No\r\n".as_bytes());
        feed(&probe, " Esc to cancel\r\n".as_bytes());
        assert_eq!(
            probe.tick(2, after_grace),
            Some(crate::detect::ScreenState::Blocked)
        );

        // 放行后回到提示框：标题换 ✳、屏上是 ❯ 提示框 → 可见 idle 直接发布（防抖直通）。
        feed(&probe, "\x1b]0;\u{2733} claude\x07".as_bytes());
        feed(&probe, b"\x1b[2J\x1b[H");
        feed(&probe, "  tool output done\r\n".as_bytes());
        feed(&probe, "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\r\n".as_bytes());
        feed(&probe, " \u{276F} \r\n".as_bytes());
        feed(&probe, "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\r\n".as_bytes());
        feed(&probe, "  ? for shortcuts\r\n".as_bytes());
        assert_eq!(
            probe.tick(3, after_grace),
            Some(crate::detect::ScreenState::Idle)
        );

        // 无新输出（序号不变）且无待确认：整轮跳过。
        assert_eq!(probe.tick(3, after_grace), None);
    }

    /// 三态判定与订阅表严格同步：None 误判会新开重复窗口，Legacy/Pid 误判会不开新视图。
    #[test]
    fn external_viewer_state_follows_the_subscriber_table() {
        let broker = PtyBroker::default();
        assert_eq!(
            broker.external_viewer(7),
            ExternalViewer::None,
            "无此会话必须判 None"
        );
        let managed = dummy_managed(7);
        broker.sessions.lock().unwrap().insert(7, managed.clone());
        assert_eq!(
            broker.external_viewer(7),
            ExternalViewer::None,
            "无订阅必须判 None"
        );
        let (tx1, _rx1) = mpsc::channel::<Vec<u8>>();
        managed.subscribers.lock().unwrap().push(AttachSubscriber {
            id: 1,
            pid: None,
            tx: tx1,
        });
        assert_eq!(
            broker.external_viewer(7),
            ExternalViewer::Legacy,
            "在线但未上报 pid（旧 reporter）→ Legacy"
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
            broker.external_viewer(7),
            ExternalViewer::Pid(5353),
            "取最近注册的上报者"
        );
        managed.subscribers.lock().unwrap().clear();
        assert_eq!(
            broker.external_viewer(7),
            ExternalViewer::None,
            "订阅摘除后必须回落 None"
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

        // claim 不重试（reporter 一次性）：登记未落的窗口里必须等，不能按「已结束」错杀。
        broker.handle_claim(&token, "launch", 9).unwrap();
        handle.join().unwrap();
        let sessions = broker.sessions.lock().unwrap();
        assert!(sessions.contains_key(&9));
        assert!(!sessions.contains_key(&-5));
        drop(sessions);
        assert_eq!(broker.binding(-5), Some(9));
        // token 不随认领消费：/clear 换代要带着它重认领，存续到 PTY 退出路径清理。
        assert_eq!(
            broker.attach.pending.lock().unwrap().get("launch"),
            Some(&-5)
        );
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

    /// /clear 换代的 claim 状态机（不触 DB 的分支）：同 id 重放幂等成功、launch token
    /// 不被消费；旧会话已不在 sessions（PTY 已亡）时换代报错且不动绑定表。
    #[test]
    fn reclaim_is_idempotent_on_replay_and_rejects_dead_session() {
        let broker = PtyBroker::default();
        let token = broker.attach.token.clone();
        broker
            .attach
            .pending
            .lock()
            .unwrap()
            .insert("launch-token".into(), -7);
        broker.attach.bindings.lock().unwrap().insert(-7, 9);

        // compact 等场景会对同一真实 id 重发 SessionStart：重放必须幂等成功。
        broker.handle_claim(&token, "launch-token", 9).unwrap();
        // token 存续（/clear 换代还要靠它重认领），不随认领消费。
        assert_eq!(
            broker.attach.pending.lock().unwrap().get("launch-token"),
            Some(&-7)
        );

        // 换代到新 id，但旧会话已不在 sessions（PTY 已退出）：报错，且绑定保持原值。
        assert!(broker.handle_claim(&token, "launch-token", 10).is_err());
        assert_eq!(broker.binding(-7), Some(9));
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

    /// 测试固件：向审批表塞一条挂起请求，返回决策接收端。
    fn seed_pending_approval(
        broker: &PtyBroker,
        session_id: i64,
        request_id: &str,
    ) -> mpsc::Receiver<ApprovalDecision> {
        let (tx, rx) = mpsc::channel();
        broker.attach.approvals.lock().unwrap().insert(
            request_id.into(),
            PendingApproval {
                request: ApprovalRequest {
                    session_id,
                    request_id: request_id.into(),
                    provider: "codex".into(),
                    tool_name: "Bash".into(),
                    description: None,
                    input: "{}".into(),
                    permission_suggestions: vec![],
                },
                response: tx,
            },
        );
        rx
    }

    /// 测试固件：把远端租约人为推回过去，模拟 TTL 过期/临期。
    fn age_consumer_lease(broker: &PtyBroker, consumer_id: &str, by_ms: i64) {
        broker
            .attach
            .approval_consumers
            .lock()
            .unwrap()
            .get_mut(consumer_id)
            .unwrap()
            .seen_ms -= by_ms;
    }

    #[test]
    fn closing_approval_consumer_passes_every_pending_request() {
        // 没有任何消费者时关窗：全部挂起审批交还 TUI（release 的退化形态 = 旧全清语义）。
        let broker = PtyBroker::default();
        let receivers = vec![
            seed_pending_approval(&broker, 7, "request-1"),
            seed_pending_approval(&broker, 7, "request-2"),
        ];
        broker.release_desktop_consumers();
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
    fn destroying_chat_window_clears_every_desktop_lease() {
        // 窗口销毁时前端的 unregister 未必执行得到；关窗兜底必须把桌面租约清干净，
        // 否则残留租约会让下一个审批入队空等 300s 而不是立即交还 TUI。
        let broker = PtyBroker::default();
        broker
            .register_approval_consumer(7, "consumer-a".into())
            .unwrap();
        broker
            .register_approval_consumer(8, "consumer-b".into())
            .unwrap();
        broker.release_desktop_consumers();
        assert!(!broker.has_approval_consumer(7));
        assert!(!broker.has_approval_consumer(8));
    }

    #[test]
    fn closing_chat_window_spares_fresh_remote_leases_and_their_approvals() {
        // 关桌面对话窗不许连坐手机端：远端新鲜租约保留，其会话的挂起审批继续可领取；
        // 只有无人看的会话的审批被交还 TUI。
        let broker = PtyBroker::default();
        broker
            .register_approval_consumer(7, "chat-a".into())
            .unwrap();
        broker
            .register_approval_consumer(8, "remote:phone".into())
            .unwrap();
        let desktop_rx = seed_pending_approval(&broker, 7, "request-7");
        let remote_rx = seed_pending_approval(&broker, 8, "request-8");

        broker.release_desktop_consumers();

        assert!(!broker.has_approval_consumer(7), "桌面租约应被清掉");
        assert!(broker.has_approval_consumer(8), "远端新鲜租约应保留");
        assert!(
            matches!(desktop_rx.recv().unwrap(), ApprovalDecision::Pass),
            "无人看的会话的审批交还 TUI"
        );
        assert!(
            matches!(remote_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "手机端正等着批的卡不得被 pass"
        );
        assert!(broker.pending_approval(8).is_some());
    }

    #[test]
    fn stale_remote_lease_counts_as_absent_everywhere() {
        // 过期远端租约（手机锁屏/被杀，没走 unregister）不得压制任何判定：
        // 目标会话判「无消费者」、关窗时其审批照常交还、顺手从表里清掉。
        let broker = PtyBroker::default();
        broker
            .register_approval_consumer(7, "remote:phone".into())
            .unwrap();
        age_consumer_lease(&broker, "remote:phone", REMOTE_CONSUMER_TTL_MS + 1);

        assert!(!broker.has_approval_consumer(7));
        assert!(!broker.has_fresh_remote_consumer(7));
        assert!(!broker.viewed_session_ids().contains(&7));

        let rx = seed_pending_approval(&broker, 7, "request-7");
        broker.release_desktop_consumers();
        assert!(matches!(rx.recv().unwrap(), ApprovalDecision::Pass));
        assert!(
            broker.attach.approval_consumers.lock().unwrap().is_empty(),
            "过期远端租约应随关窗清理"
        );
    }

    #[test]
    fn unregister_ignores_stale_remote_lease_when_deciding_last_watcher() {
        // 桌面租约注销时若同会话只剩过期远端租约，等同没人看：审批立即交还 TUI，
        // 不许被一个不会来领卡的幽灵租约压着空等 300s。
        let broker = PtyBroker::default();
        broker
            .register_approval_consumer(7, "chat-a".into())
            .unwrap();
        broker
            .register_approval_consumer(7, "remote:phone".into())
            .unwrap();
        age_consumer_lease(&broker, "remote:phone", REMOTE_CONSUMER_TTL_MS + 1);
        let rx = seed_pending_approval(&broker, 7, "request-7");

        broker.unregister_approval_consumer("chat-a");
        assert!(matches!(rx.recv().unwrap(), ApprovalDecision::Pass));
    }

    #[test]
    fn remote_lease_reregistration_refreshes_ttl() {
        // 手机端 20s 心跳 = 同 id 重注册；临期租约续约后必须重新算新鲜。
        let broker = PtyBroker::default();
        broker
            .register_approval_consumer(7, "remote:phone".into())
            .unwrap();
        age_consumer_lease(&broker, "remote:phone", REMOTE_CONSUMER_TTL_MS - 1_000);
        assert!(broker.has_fresh_remote_consumer(7), "临期未过期仍新鲜");
        broker
            .register_approval_consumer(7, "remote:phone".into())
            .unwrap();
        age_consumer_lease(&broker, "remote:phone", 1_000);
        assert!(broker.has_fresh_remote_consumer(7), "续约后重新起算");
    }

    /// 闸门矩阵：远端在场恒可领取（含轻量模式——chat_enabled 关的是桌面窗，不关远程
    /// 通道）；远端缺席时轻量模式回落 TUI、常规模式走桌面召唤。远程未开时注册不出
    /// remote: 租约，第一参恒 false，桌面语义与改动前逐字一致。
    #[test]
    fn approval_gate_matrix() {
        assert_eq!(approval_gate(true, true), ApprovalGate::Claimable);
        assert_eq!(approval_gate(true, false), ApprovalGate::Claimable);
        assert_eq!(approval_gate(false, false), ApprovalGate::FallbackTui);
        assert_eq!(approval_gate(false, true), ApprovalGate::Desktop);
    }

    #[test]
    fn external_approval_passes_immediately_without_gui_consumer() {
        let broker = PtyBroker::default();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = broker.clone();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            server.handle_attach(stream, None).unwrap();
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
            server.handle_attach(stream, None).unwrap();
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
