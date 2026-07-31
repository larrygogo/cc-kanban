//! 看板刷新合流 + DB 文件监听 + 会话存活轮询 + 去重桌面通知。从 lib.rs 抽出。
//! 这里是「后台线程层」：所有向前端 board-changed / 通知的推送都收敛在此。

use crate::proc::pid_is_agent;
use crate::session_query::RESUME_GRACE_MS;
use crate::settings::{load_settings, tr, ui_lang};
#[cfg(target_os = "windows")]
use crate::terminal::focus_session_terminal;
#[cfg(target_os = "windows")]
use crate::window::update_tray_tooltip;
use crate::{agent_transcript, now_ms};
use meowo_store::Store;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, RefreshKind, System};
use tauri::{Emitter, Manager};

/// board-changed 的全局合流窗口。发出一次后的这段时间内，新来的事件不再各自 emit，
/// 只在窗口末尾补发一次（携带最后一个 reason）。
pub(crate) const BOARD_COALESCE: Duration = Duration::from_millis(300);

/// 合流线程的入口。setup 里 spawn_board_notifier 装填；未装填时 emit_board_changed 退化为直接 emit。
pub(crate) static BOARD_TX: OnceLock<std::sync::mpsc::Sender<&'static str>> = OnceLock::new();

/// 通知前端看板已变更。全部 emit 点都走这里，不要再直接 app.emit("board-changed", ..)。
///
/// 此前各源各自为政：命令处理器写完库立刻裸发，db-watcher 1s 后又为同一次写入回声一次，
/// liveness 每 5s 还可能插一脚——一个归档动作能打出 2~3 次全量刷新，而前端每次刷新是
/// counts + 一整页（页大小随用户滚动增长）+ 折叠条两查询。合流器给出全局上限：
/// 至多每 BOARD_COALESCE 一次 emit，同时保持孤立事件（绝大多数用户操作）零延迟送达。
///
/// reason 只用于调试与日志，前端当前忽略 payload。
pub(crate) fn emit_board_changed(app: &tauri::AppHandle, reason: &'static str) {
    match BOARD_TX.get() {
        Some(tx) => {
            let _ = tx.send(reason);
        }
        // 合流线程尚未启动（setup 之前）或装填失败：宁可多发，不可不发。
        None => {
            let _ = app.emit("board-changed", reason);
        }
    }
}

/// 启动 board-changed 合流线程：前沿立即发，之后每个 BOARD_COALESCE 窗口至多补发一次。
pub(crate) fn spawn_board_notifier(app: tauri::AppHandle) {
    let (tx, rx) = channel::<&'static str>();
    if BOARD_TX.set(tx).is_err() {
        return; // 已启动过
    }
    std::thread::spawn(move || {
        coalesce_loop(rx, |reason| {
            let _ = app.emit("board-changed", reason);
        });
    });
}

/// 合流循环本体，与 tauri 解耦（emit 为闭包）以便单测；生产侧由 spawn_board_notifier 驱动。
/// 语义：leading+trailing——孤立事件零延迟送达；窗口内多次触发合并为窗口末尾的一次，
/// 且排队中的旧事件被最新事件取代（pending 只记最后一个 reason，前端忽略 payload、只重拉最新状态）。
fn coalesce_loop(rx: std::sync::mpsc::Receiver<&'static str>, mut emit: impl FnMut(&'static str)) {
    loop {
        // 空闲：阻塞等第一个事件。发送端存活于 static，recv 出错只可能是进程收尾。
        let Ok(mut reason) = rx.recv() else { return };
        loop {
            emit(reason);
            // 冷却窗口：收集期间到达的事件，只记最后一个 reason。
            let mut pending: Option<&'static str> = None;
            let deadline = Instant::now() + BOARD_COALESCE;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(r) => pending = Some(r),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            // 窗口内有积压 → 立刻补发并重新冷却（故持续高频写库时输出恒定为 1/窗口）；
            // 没有积压 → 回到外层阻塞等待，下一个孤立事件仍是前沿即时送达。
            match pending {
                Some(r) => reason = r,
                None => break,
            }
        }
    }
}

/// watch 建立失败/监听死亡后的重建间隔。
pub(crate) const WATCH_RETRY: Duration = Duration::from_secs(5);

/// 监听 board.db 所在目录变更，去抖后向前端发 "board-changed"。
/// watch 建立失败（全新安装时 ~/.meowo 由 ccsetup/liveness 等并发线程创建，watcher 可能抢先执行
/// 而目录尚不存在）或监听中途死亡（目录被删、notify 后端出错）都不放弃：先确保目录存在、失败 5s 后
/// 重建——否则首启一次失败会让 DB 变更监听在整个进程生命周期内静默失效，前端无轮询兜底、看板冻结。
/// 重建成功后无条件补发一次：监听死亡到重建完成的间隙里，最后一次变更可能没有任何事件送达。
pub(crate) fn spawn_db_watcher(app: tauri::AppHandle, db_path: PathBuf) {
    let watch_dir = db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    // 只关心 db 本体及其 -wal/-journal 伴生文件（写入落这里）；同目录的 settings.json、
    // usage-cache.json 不触发刷新。-shm 刻意排除：它是 WAL 共享内存索引，纯读也会更新其读标记
    // 触碰 mtime，正是 app 自身读库触发 watcher、进而自持刷新的源头（见 run_db_watch_loop）。
    let db_name = db_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board.db".to_string());
    std::thread::spawn(move || {
        // 首轮建立不算重建：启动路径自带首轮数据加载，无需补发。
        let mut rebuilt = false;
        loop {
            let _ = std::fs::create_dir_all(&watch_dir);
            let (tx, rx) = channel();
            let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(_) => {
                    std::thread::sleep(WATCH_RETRY);
                    continue;
                }
            };
            if watcher
                .watch(&watch_dir, RecursiveMode::NonRecursive)
                .is_err()
            {
                std::thread::sleep(WATCH_RETRY);
                continue;
            }
            if rebuilt {
                // 重建成功：从上一轮监听死亡那一刻起事件就丢了，死亡到本次重建之间（含固定 5s
                // 睡眠）的最后一次变更可能没有任何事件送达，间隙内的写入要等下一次变更才被
                // 发现，看板定格在旧数据。无条件补发一次让前端重拉。
                emit_board_changed(&app, "watch-rebuild");
            }
            rebuilt = true;
            run_db_watch_loop(&app, &rx, &db_path, &db_name);
            // 返回即监听已死（通道断开或错误事件）→ 稍后重建 watcher。
            std::thread::sleep(WATCH_RETRY);
        }
    });
}

/// db watcher 的事件循环：trailing debounce（收到相关事件后 drain 到 1s 静默再 emit——SQLite 提交
/// 是 db/-wal/-shm 多个事件的爆发，前沿触发会丢掉尾部事件），但设 2s 总上限：statusline/hook 以
/// ~300ms 节奏持续写库、多会话事件流相位交错时可能永无静默间隙，无上限会让 board-changed 饥饿、
/// 贴纸冻结在旧数据，恰恰是多会话高活跃期最需要刷新的时候。
/// 注意这层 debounce 与 emit_board_changed 的全局合流是两回事：这里压的是「一次提交的多文件事件」，
/// 那里压的是「多个源对同一次变更的重复通知」。
/// 返回即表示监听已死（通道断开或收到 notify 错误事件，如目录被删），由调用方重建。
pub(crate) fn run_db_watch_loop(
    app: &tauri::AppHandle,
    rx: &std::sync::mpsc::Receiver<Result<notify::Event, notify::Error>>,
    db_path: &std::path::Path,
    db_name: &str,
) {
    // 持久只读连接：PRAGMA data_version 跨调用比较须用同一连接才有意义。开不出来（库暂不可用）
    // 则 vconn=None → 下面回退为「有相关文件事件即 emit」的旧行为，宁可多刷也不让看板冻结。
    let vconn = Store::open(db_path).ok();
    // 起始基线刻意留空（不预读 data_version）：本函数在 watcher 重建时会被重跑，若此刻预读当前版本，
    // 会把「上一轮退出到本次基线读取之间那次提交」当成已知基线，其排队事件到达时 data_version 等于
    // 基线 → 被误判无变更而丢掉那次刷新。留 None 则首个相关事件必发一次（版本必 != None），既补上
    // 重建间隙的写入、又只多一次无害刷新；此后正常门控，不影响掐断自持循环。
    let mut last_version: Option<i64> = None;
    let is_board = |res: &Result<notify::Event, notify::Error>| -> bool {
        let Ok(ev) = res else { return false };
        ev.paths.iter().any(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.strip_prefix(db_name)
                    // -shm（WAL 共享内存索引）排除：纯读也会触碰它，是自持刷新循环的源头。
                    .is_some_and(|rest| {
                        rest.is_empty() || (rest.starts_with('-') && rest != "-shm")
                    })
            })
        })
    };
    let debounce = Duration::from_millis(1000);
    let max_wait = Duration::from_millis(2000);
    loop {
        let Ok(first) = rx.recv() else { return }; // watcher 关闭/内部线程死亡 → 重建
        if first.is_err() {
            return; // notify 错误事件（目录被删等）→ 重建
        }
        let mut relevant = is_board(&first);
        let mut broken = false;
        let deadline = std::time::Instant::now() + max_wait;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break; // 事件持续不断也到点先 emit 一次，防饥饿
            }
            match rx.recv_timeout(debounce.min(remaining)) {
                Ok(ev) => {
                    if ev.is_err() {
                        broken = true;
                        break;
                    }
                    relevant = relevant || is_board(&ev);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    broken = true;
                    break;
                }
            }
        }
        if relevant {
            // 只有 data_version 变了（别的连接真的提交过写入）才通知前端刷新；app 自己读库触碰
            // 文件、WAL checkpoint 等「没有逻辑变更」的空事件一律忽略——这才是掐断
            // read→watcher→refresh→read 自持循环、消除空闲期贴纸抖动的根治点。
            // 无持久连接或读版本失败时保守 emit（回退旧行为），不让看板冻结。
            let changed = match vconn.as_ref().and_then(|s| s.data_version().ok()) {
                Some(cur) => {
                    let c = last_version != Some(cur);
                    last_version = Some(cur);
                    c
                }
                None => true,
            };
            if changed {
                emit_board_changed(app, "db-watcher");
            }
        }
        if broken {
            return;
        }
    }
}

/// 轮询一次：把「记录了 pid、但该进程已死」的 live 会话收尾为 ended（self-heal），
/// 并返回仍存活的 session id（升序）与本轮收尾的数量。
///
/// 终端被关/被 /clear 打断时 SessionEnd 往往不触发，会话状态会永远卡在 running/waiting；
/// 进程都没了就该收尾。pid 为空的不动（可能是刚启动还没抓到 pid，宁可不臆测）。
pub(crate) fn reap_and_alive_ids(store: &Store, sys: &System, now_ms: i64) -> (Vec<i64>, usize) {
    const PID_REAP_GRACE_MS: i64 = 10_000;
    let mut alive: Vec<i64> = Vec::new();
    let mut reaped = 0usize;
    for (id, pid, last_event_at) in store.live_session_liveness().unwrap_or_default() {
        match pid {
            Some(p) if p > 0 => {
                if pid_is_agent(sys, p) {
                    alive.push(id);
                // 进程快照先于 DB 查询生成；刚启动的进程可能已由 hook 写入 DB、却尚未出现在该快照。
                // 给新事件一个短宽限，下一轮快照即可确认。真正退出的进程最多晚 10s 收尾。
                } else if now_ms.saturating_sub(last_event_at) < PID_REAP_GRACE_MS {
                    alive.push(id);
                } else if store
                    .end_session_if_pid(id, p, last_event_at, now_ms)
                    .unwrap_or(false)
                {
                    reaped += 1; // 进程已死 / pid 被复用 → 收尾
                }
            }
            _ => {} // pid 未知：不臆测，留给 SessionEnd / 同进程新会话驱逐处理
        }
    }
    alive.sort_unstable();
    (alive, reaped)
}

/// 是否应为「当前错误指纹」弹通知：仅当当前有错误且指纹与上次通知过的不同。
/// 同一错误不反复弹；错误消失（cur=None）不弹（清除条目交给调用方）。纯函数，便于单测。
pub(crate) fn should_notify(prev: Option<&str>, cur: Option<&str>) -> bool {
    match cur {
        None => false,
        Some(c) => prev != Some(c),
    }
}

/// 待交互通知指纹:errored 或 has_pending 时不发(None,让位错误/待审批);
/// status==waiting 且无错无 pending 时用 last_event_at 作指纹;其它状态 None。纯函数。
pub(crate) fn waiting_fingerprint(
    errored: bool,
    has_pending: bool,
    status: &str,
    last_event_at: i64,
) -> Option<String> {
    if errored || has_pending || status != "waiting" {
        None
    } else {
        Some(last_event_at.to_string())
    }
}

/// 待审批通知指纹:errored 时 None(错误优先);pending 为 Some(kind) 时 "{kind}:{last_event_at}";
/// 否则 None。纯函数,便于单测。
pub(crate) fn pending_fingerprint(
    errored: bool,
    pending_review: Option<&str>,
    last_event_at: i64,
) -> Option<String> {
    if errored {
        return None;
    }
    pending_review.map(|kind| format!("{kind}:{last_event_at}"))
}

/// 屏幕检测「blocked」的通知指纹——托管会话的 agent 在屏幕上挂着审批/提问 UI 等人。
///
/// 存在意义是补 hook 的盲区：DB 的 `pending_review` 由 hook 事件驱动，bash 权限弹窗、
/// AskUserQuestion 之外的 TUI 提问等常常没有对应事件，卡片能靠屏幕检测翻琥珀，通知却
/// 一直哑着——「agent 停下来等你」正是最该被通知的一件事。
///
/// 让位规则与其它指纹一致：errored / 已有 pending_review 时返回 None（那两条各自会发，
/// 不重复打扰）。指纹只用 sid + 状态本身，**不掺 last_event_at**：blocked 期间 agent
/// 不产出、时间戳不动，掺进去反而在时间戳因别的原因跳动时重复弹。同一次阻塞只发一条，
/// 状态离开 blocked 后条目被清掉，下次再阻塞才会重新通知。
pub(crate) fn screen_blocked_fingerprint(
    errored: bool,
    has_pending: bool,
    screen_state: Option<&str>,
) -> Option<String> {
    if errored || has_pending {
        return None;
    }
    (screen_state == Some("blocked")).then(|| "blocked".to_string())
}

/// 弹一条「点击即跳到该会话」的桌面通知。落点按会话归属分流：GUI 托管会话（PTY 由 Meowo
/// 持有，进程组里没有可聚焦的外部终端窗口）→ 打开/聚焦对话窗口并切到该会话；外部终端
/// 会话 → 原来的 focus_session_terminal（它自己 spawn 干净线程做 UIA，不阻塞主线程）。
/// 构建+show 放主线程：winrt toast 的 show() 需在 COM STA 上调用，Tauri 主线程即 STA；
/// on_activated 回调由 OS 经 COM 激活机制投递（与消息泵无关），show() 后 Rust 端 Toast
/// 可安全释放（OS 持有通知引用）。app 仅 Windows。
#[cfg(target_os = "windows")]
// 参数数量超限（9 个）是现有设计需要；重构签名风险大，暂以 allow 豁免。
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_session_notification(
    app: &tauri::AppHandle,
    title: String,
    body: String,
    session_id: i64,
    pid: i64,
    focus_title: String,
    focus_cwd: Option<String>,
    focus_token: Option<String>,
    title_based: bool,
) {
    use tauri_winrt_notification::Toast;
    // 安装版用 bundle identifier（解析到开始菜单快捷方式 → 显示 Meowo+图标 + 点击可激活）；
    // dev 下 AUMID 未注册，退回 PowerShell 的 AUMID 仅保证 toast 能弹出；此时 on_activated 回调
    // 根本不会触发（OS 把激活事件投递给 PowerShell 进程而非本进程），点击跳转只在安装版生效。
    let app_id = if tauri::is_dev() {
        Toast::POWERSHELL_APP_ID.to_string()
    } else {
        app.config().identifier.clone()
    };
    let activate_app_id = app_id.clone();
    let click_app = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = Toast::new(&app_id)
            .title(&title)
            .text1(&body)
            .on_activated(move |_| {
                // 点击后 toast 不会自动从"通知中心"消失（Windows 设计如此），主动清掉本应用
                // 的历史通知。回调线程由 OS 经 COM 投递、已初始化 WinRT，可直接调 History。
                clear_delivered_toasts(&activate_app_id);
                // 托管会话的 PTY 归 Meowo，进程组里没有可见终端窗口，focus_session_terminal
                // 必然落空——点击应该打开对话窗口并落在该会话上（点击是明确意图，切会话正合适）。
                if click_app
                    .state::<crate::AppState>()
                    .ptys
                    .is_managed(session_id)
                {
                    crate::window::open_chat_window_detached(click_app.clone(), session_id);
                    return Ok(());
                }
                let title = focus_title.clone();
                let cwd = focus_cwd.clone();
                let token = focus_token.clone();
                std::thread::spawn(move || {
                    let _ = focus_session_terminal(pid, Some(title), cwd, token, title_based);
                });
                Ok(())
            })
            .show();
    });
}

/// 从"通知中心"移除本应用（按 AUMID）所有已投递的 toast。tauri-winrt-notification 不暴露单条
/// 移除/tag，故只能整体清空——对"会话等待/出错"这类瞬时提醒正合适。失败静默（非安装版 AUMID
/// 未注册时会 Err）。
#[cfg(target_os = "windows")]
fn clear_delivered_toasts(app_id: &str) {
    use windows::core::HSTRING;
    use windows::UI::Notifications::ToastNotificationManager;
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.ClearWithId(&HSTRING::from(app_id));
    }
}

#[cfg(target_os = "macos")]
// 参数数量超限（9 个）是现有设计需要；重构签名风险大，暂以 allow 豁免。
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_session_notification(
    _app: &tauri::AppHandle,
    title: String,
    body: String,
    session_id: i64,
    pid: i64,
    _focus_title: String, // macOS 按 pid->tty 定位终端，标题用不上
    _focus_cwd: Option<String>,
    _focus_token: Option<String>,
    _title_based: bool,
) {
    crate::macos::notify::post(crate::macos::notify::NotifyJob {
        title,
        body,
        session_id,
        pid,
    });
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
// 参数数量超限（9 个）是现有设计需要；重构签名风险大，暂以 allow 豁免。
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_session_notification(
    _app: &tauri::AppHandle,
    _title: String,
    _body: String,
    _session_id: i64,
    _pid: i64,
    _focus_title: String,
    _focus_cwd: Option<String>,
    _focus_token: Option<String>,
    _title_based: bool,
) {
}

/// 有新的需关注事件且用户不在看任何 Meowo 窗口时,请求任务栏注意力(Informational:
/// 高亮驻留、不无限闪烁)。目标是 chat 窗口——主贴纸 skipTaskbar,闪它是无效操作;
/// chat 窗口不存在(用户只用贴纸+外部终端)时跳过,toast 已覆盖那条路径。
#[cfg(target_os = "windows")]
fn flash_chat_window_if_unfocused(app: &tauri::AppHandle) {
    use tauri::Manager;
    let any_focused = app
        .webview_windows()
        .values()
        .any(|w| w.is_focused().unwrap_or(false));
    if any_focused {
        return;
    }
    if let Some(w) = app.get_webview_window("chat") {
        let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
}

/// 周期轮询：收尾进程已死的卡住会话；存活集合变化或有收尾时发 board-changed 让前端刷新。
/// 同时对「连接中」会话做去重桌面通知：出错（优先）或进入待交互时各弹一次。
/// 总开关（settings.notifications_enabled）只门控是否 .show()，去重 map 始终更新，
/// 故中途打开开关不会把积压的旧错误/待交互一次性炸出来。启动首扫只播种不弹。
pub(crate) fn spawn_liveness_watch(
    app: tauri::AppHandle,
    db_path: PathBuf,
    tx_cache: Arc<Mutex<meowo_agent::TranscriptCache>>,
) {
    use std::collections::HashMap;
    std::thread::spawn(move || {
        let mut last: Vec<i64> = Vec::new();
        let mut notified: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次错误指纹
        let mut notified_waiting: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次待交互指纹
        let mut notified_pending: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次待审批指纹
        let mut notified_blocked: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次屏幕阻塞指纹
        let mut seeded = false;
        // 托盘状态摘要只在 (运行,待交互) 变化时刷新，避免每轮无谓重画/重设提示。
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let mut last_tray: Option<(usize, usize)> = None;
        loop {
            if let Ok(store) = Store::open(&db_path) {
                let sys = System::new_with_specifics(
                    RefreshKind::new().with_processes(ProcessRefreshKind::new()),
                );
                // 阈值与 session_connected 的 RESUME_GRACE_MS 对齐：pid 未知的会话「已连接」显示
                // 在此窗口后回落断开，DB 的 status 也应同时收尾，否则会话会长期卡在错误的 tab 里
                // （见 RESUME_GRACE_MS 文档：kimi 卸载后 resume「终端起来但命令失败」即命中此路径）。
                let orphaned = store
                    .end_orphaned_idle(RESUME_GRACE_MS, now_ms())
                    .unwrap_or(0);
                let (alive, reaped) = reap_and_alive_ids(&store, &sys, now_ms());
                if alive != last || reaped > 0 || orphaned > 0 {
                    emit_board_changed(&app, "liveness");
                    last = alive;
                }

                // 通知总开关 + 语言：每轮读一次（文件读极廉价；设置改动 5s 内生效）。
                let settings = load_settings();
                let notify_on = settings.notifications_enabled;
                let lang = ui_lang(&settings);
                // 本轮是否有新的需关注事件(与 toast 同一套指纹判定,但不受 notify_on 门控):
                // 循环末尾据此请求一次任务栏注意力。
                let mut attention_fired = false;

                // 错误 + 待交互通知：仅扫连接中的会话（活跃，数量少）。同时统计菜单栏状态摘要。
                let mut present: HashMap<String, String> = HashMap::new();
                let (mut tray_running, mut tray_waiting) = (0usize, 0usize);
                // 「待批准」的存活校正快照(见 session_query::pending_review_live)。托盘摘要与
                // 待审批通知都读 pending_review,不校正的话:权限早已放行、agent 正跑着长工具,
                // 托盘却一直亮「需关注」——那一位要等 PostToolUse 才清。
                // try_state:本线程可能早于 AppState 注册,拿不到就退回不校正(等同旧行为)。
                let approvals = app
                    .try_state::<crate::AppState>()
                    .map(|state| state.ptys.approval_session_ids())
                    .unwrap_or_default();
                // 托管会话的屏幕检测状态（见 pty.rs ScreenProbe）：hook 覆盖不到的阻塞
                // （bash 权限弹窗等）只能靠它发通知。取不到 AppState 就退回空表 = 不发。
                let screen_states = app
                    .try_state::<crate::AppState>()
                    .map(|state| state.ptys.screen_states())
                    .unwrap_or_default();
                for mut s in store
                    .live_sessions(Some("all"), None, None, None, 1000)
                    .unwrap_or_default()
                {
                    s.pending_review = crate::session_query::pending_review_live(
                        &s.provider,
                        s.pending_review.as_deref(),
                        approvals.contains(&s.session.id),
                    )
                    .map(str::to_string);
                    if s.session.status == "ended" || !pid_is_agent(&sys, s.pid.unwrap_or(0)) {
                        continue;
                    }
                    let sid = s.session.cc_session_id.clone();
                    present.insert(sid.clone(), String::new()); // 标记本轮已扫描；retain 只清理本轮彻底消失的会话

                    let meowo_agent::TranscriptInfo {
                        mut title, error, ..
                    } = agent_transcript(&s.provider)
                        .filter(|spec| spec.supports_analysis())
                        .and_then(|spec| {
                            spec.resolve_transcript_path(None, s.cwd.as_deref(), &sid)
                                .and_then(|p| p.to_str().map(str::to_string))
                                .map(|path| {
                                    // 锁外 IO 版：大文件首读不阻塞 get_live_sessions（见 analyze_shared）。
                                    meowo_agent::TranscriptCache::analyze_shared(
                                        &tx_cache, spec, &path,
                                    )
                                })
                        })
                        .unwrap_or_default();
                    if !crate::agent_resolves_transcript_title(&s.provider) {
                        title = None;
                    }
                    // 会话标题：通知正文用，也作点击聚焦时匹配 WT 标签页的标题。transcript 标题优先，否则 DB 标题。
                    let display_title = title
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or_else(|| s.task_title.clone());
                    let pid = s.pid.unwrap_or(0); // 连接中必为有效 pid
                                                  // 该 agent 是否把任务标题写进 WT 标签：决定通知点击是按标题切标签还是窗口级定位。
                    let title_based = meowo_agent::resolve(Some(&s.provider))
                        .is_some_and(|a| a.sets_terminal_tab_title());
                    // 仅对确实由 reporter 写 token 的 agent 使用 sid8；其它 agent 盲匹配会有误命中风险。
                    let tab_token = meowo_agent::resolve(Some(&s.provider))
                        .filter(|a| a.writes_tab_token())
                        .map(|_| meowo_reporter::tabtitle::short_sid(&s.session.cc_session_id))
                        .filter(|t| !t.is_empty());

                    // 菜单栏摘要计数:出错/待交互/待审批/屏幕阻塞 → 需关注(●),运行中 → ○;
                    // 在线空闲不计入。屏幕阻塞同样计入——它与「待审批」是同一件事的两个
                    // 信息源（hook 有事件 / 只在屏幕上），托盘不该因为来源不同而漏计。
                    if error.is_some()
                        || s.session.status == "waiting"
                        || s.pending_review.is_some()
                        || screen_states.get(&s.session.id).copied() == Some("blocked")
                    {
                        tray_waiting += 1;
                    } else if s.session.status == "running" {
                        tray_running += 1;
                    }

                    // 错误通知（优先）。
                    if let Some(e) = &error {
                        let prev = notified.get(&sid).map(|s| s.as_str());
                        let fired = seeded && should_notify(prev, Some(&e.fingerprint));
                        attention_fired |= fired;
                        if fired && notify_on {
                            show_session_notification(
                                &app,
                                tr(lang, "notify.error").into(),
                                format!("{} · {}", s.project_name, e.label),
                                s.session.id,
                                pid,
                                display_title.clone(),
                                s.cwd.clone(),
                                tab_token.clone(),
                                title_based,
                            );
                        }
                        notified.insert(sid.clone(), e.fingerprint.clone());
                    } else {
                        notified.remove(&sid); // 错误消失：下次再错会重新通知
                    }

                    // 待审批通知(错误之后、待交互之前;errored 时 pending_fingerprint 返回 None 自动让位)。
                    match pending_fingerprint(
                        error.is_some(),
                        s.pending_review.as_deref(),
                        s.session.last_event_at,
                    ) {
                        Some(fp) => {
                            let prev = notified_pending.get(&sid).map(|s| s.as_str());
                            let fired = seeded && should_notify(prev, Some(&fp));
                            attention_fired |= fired;
                            if fired && notify_on {
                                let key = match s.pending_review.as_deref() {
                                    Some("question") => "notify.pending.question",
                                    Some("plan") => "notify.pending.plan",
                                    _ => "notify.pending.approval",
                                };
                                show_session_notification(
                                    &app,
                                    tr(lang, key).into(),
                                    format!("{} · {}", s.project_name, display_title),
                                    s.session.id,
                                    pid,
                                    display_title.clone(),
                                    s.cwd.clone(),
                                    tab_token.clone(),
                                    title_based,
                                );
                            }
                            notified_pending.insert(sid.clone(), fp);
                        }
                        None => {
                            notified_pending.remove(&sid);
                        }
                    }

                    // 屏幕阻塞通知（hook 盲区补位：agent 在屏幕上挂着审批/提问 UI 等人，
                    // 而 DB 的 pending_review 没有对应事件）。排在待审批之后、待交互之前：
                    // 有 pending_review 时它返回 None 自动让位，不重复打扰。
                    match screen_blocked_fingerprint(
                        error.is_some(),
                        s.pending_review.is_some(),
                        screen_states.get(&s.session.id).copied(),
                    ) {
                        Some(fp) => {
                            let prev = notified_blocked.get(&sid).map(|s| s.as_str());
                            let fired = seeded && should_notify(prev, Some(&fp));
                            attention_fired |= fired;
                            if fired && notify_on {
                                show_session_notification(
                                    &app,
                                    tr(lang, "notify.blocked").into(),
                                    format!("{} · {}", s.project_name, display_title),
                                    s.session.id,
                                    pid,
                                    display_title.clone(),
                                    s.cwd.clone(),
                                    tab_token.clone(),
                                    title_based,
                                );
                            }
                            notified_blocked.insert(sid.clone(), fp);
                        }
                        None => {
                            notified_blocked.remove(&sid);
                        }
                    }

                    // 待交互通知（errored 时 waiting_fingerprint 返回 None，自动让位给错误）。
                    match waiting_fingerprint(
                        error.is_some(),
                        s.pending_review.is_some(),
                        &s.session.status,
                        s.session.last_event_at,
                    ) {
                        Some(fp) => {
                            let prev = notified_waiting.get(&sid).map(|s| s.as_str());
                            let fired = seeded && should_notify(prev, Some(&fp));
                            attention_fired |= fired;
                            if fired && notify_on {
                                show_session_notification(
                                    &app,
                                    tr(lang, "notify.waiting").into(),
                                    format!("{} · {}", s.project_name, display_title),
                                    s.session.id,
                                    pid,
                                    display_title.clone(),
                                    s.cwd.clone(),
                                    tab_token.clone(),
                                    title_based,
                                );
                            }
                            notified_waiting.insert(sid.clone(), fp);
                        }
                        None => {
                            notified_waiting.remove(&sid);
                        }
                    }
                }
                // 清掉本轮彻底消失（已结束/超出 1000 条上限，见上方 live_sessions 查询）的残留条目，防止 map 无限增长。
                // 边缘情况：会话彻底消失后又带着完全相同的未解决错误/待交互重新出现，会再弹一次——可接受。
                notified.retain(|k, _| present.contains_key(k));
                notified_pending.retain(|k, _| present.contains_key(k));
                notified_blocked.retain(|k, _| present.contains_key(k));
                notified_waiting.retain(|k, _| present.contains_key(k));
                seeded = true;

                // 任务栏注意力:本轮有任何新的需关注事件就闪一次(每轮最多一次)。toast 会自己
                // 消失,任务栏高亮驻留到用户点开——这是「应用在后台时无感知」的补位信号。
                // 仅 Windows:macOS 已有菜单栏徽章承担同一职责,dock 弹跳过于喧闹。
                #[cfg(target_os = "windows")]
                if attention_fired && settings.attention_flash_enabled {
                    flash_chat_window_if_unfocused(&app);
                }
                #[cfg(not(target_os = "windows"))]
                let _ = attention_fired;

                // macOS：把连接中会话的状态摘要画成菜单栏彩色徽章（一眼可见，弥补无吸边缩略条）。
                #[cfg(target_os = "macos")]
                if last_tray != Some((tray_running, tray_waiting)) {
                    crate::macos::menubar::update_tray_status(&app, tray_running, tray_waiting);
                    last_tray = Some((tray_running, tray_waiting));
                }
                // Windows：把摘要写到托盘悬浮提示，鼠标移到托盘一眼可见，不必打开窗口。
                #[cfg(target_os = "windows")]
                if last_tray != Some((tray_running, tray_waiting)) {
                    update_tray_tooltip(&app, tray_running, tray_waiting, lang);
                    last_tray = Some((tray_running, tray_waiting));
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                let _ = (tray_running, tray_waiting);
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

/// 首次启动：~/.meowo/imported.json 不存在时，后台导入近 7 天历史会话并写标记文件。
/// 出错仅静默（下次启动重试），绝不阻塞窗口创建。
pub(crate) fn spawn_first_import(app: tauri::AppHandle, db_path: PathBuf) {
    std::thread::spawn(move || {
        let Some(dir) = db_path.parent().map(|p| p.to_path_buf()) else {
            return;
        };
        let marker = dir.join("imported.json");
        if marker.exists() {
            return; // 已导入过，跳过
        }
        let store = match Store::open(&db_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let now = now_ms();
        if let Ok(count) = meowo_reporter::import::import_recent(
            &store,
            now,
            meowo_reporter::import::ImportOpts::default(),
        ) {
            let body = format!("{{\"imported\":{count},\"at\":{now}}}");
            let _ = std::fs::write(&marker, body);
            if count > 0 {
                emit_board_changed(&app, "first-import");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{coalesce_loop, BOARD_COALESCE};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    /// 驱动一个合流线程：输入端交测试塞事件，emit 出的 reason 经另一通道回收集合。
    fn spawn_collector() -> (
        std::sync::mpsc::Sender<&'static str>,
        Receiver<&'static str>,
        std::thread::JoinHandle<()>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel::<&'static str>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<&'static str>();
        let handle = std::thread::spawn(move || {
            coalesce_loop(rx, move |r| {
                let _ = out_tx.send(r);
            })
        });
        (tx, out_rx, handle)
    }

    fn recv_within(rx: &Receiver<&'static str>, timeout: Duration) -> Option<&'static str> {
        rx.recv_timeout(timeout).ok()
    }

    fn leaked_reason(i: usize) -> &'static str {
        Box::leak(format!("r{i}").into_boxed_str())
    }

    /// 爆发连发 → 恰好两次 emit：前沿立即出第一个，其余合并为窗口末尾的一次且携带最后的 reason
    /// （「最新状态取代排队中的旧事件」，而不是补发窗口内第一个）。
    #[test]
    fn burst_coalesces_to_leading_plus_single_trailing_with_last_reason() {
        let (tx, out, _h) = spawn_collector();
        let t0 = Instant::now();
        for r in ["a", "b", "c", "d"] {
            tx.send(r).unwrap();
        }
        assert_eq!(recv_within(&out, Duration::from_secs(2)), Some("a"));
        assert!(t0.elapsed() < BOARD_COALESCE, "前沿必须零延迟送达");
        assert_eq!(recv_within(&out, Duration::from_secs(2)), Some("d"));
        // 窗口关闭后没有第三次 emit。
        assert_eq!(
            recv_within(&out, BOARD_COALESCE * 3),
            None,
            "窗口内事件已并入 trailing，不得再各自补发"
        );
    }

    /// 静默超过一个窗口后，下一个孤立事件仍是前沿即时送达——合流不把低频操作拖进 trailing 延迟，
    /// 「操作后立即可见」的体感不被破坏。
    #[test]
    fn isolated_event_after_idle_emits_immediately_again() {
        let (tx, out, _h) = spawn_collector();
        tx.send("one").unwrap();
        assert_eq!(recv_within(&out, Duration::from_secs(2)), Some("one"));
        std::thread::sleep(BOARD_COALESCE + Duration::from_millis(200));
        let t0 = Instant::now();
        tx.send("two").unwrap();
        assert_eq!(recv_within(&out, Duration::from_secs(2)), Some("two"));
        assert!(t0.elapsed() < BOARD_COALESCE, "孤立事件必须走前沿");
        assert_eq!(recv_within(&out, BOARD_COALESCE * 3), None);
    }

    /// 持续高频输入（间隔远小于窗口、总时长跨多个窗口）→ 输出被钳在每窗口约一次，
    /// 且最后一次 emit 收敛到最新事件。这正是 statusline/hook 高频写库时前端不被刷爆的保证。
    #[test]
    fn sustained_stream_is_capped_and_converges_to_latest() {
        let (tx, out, _h) = spawn_collector();
        const N: usize = 30;
        let last_expected = leaked_reason(N - 1);
        let sender = std::thread::spawn(move || {
            let t0 = Instant::now();
            for i in 0..N {
                if tx.send(leaked_reason(i)).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            t0.elapsed()
        });
        let mut emits: Vec<&'static str> = Vec::new();
        // 收敛即退，死线只防「永不收敛」挂死，取值放极宽：慢 CI 上 sleep(30ms) 实际可拖到
        // 百余毫秒，按名义时长（30*30ms+余量）设死线曾在 macOS runner 稳定超时——
        // 发送方还没发完就到点，收不到最末事件的 trailing。
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match out.recv_timeout(Duration::from_millis(200)) {
                Ok(r) => emits.push(r),
                Err(RecvTimeoutError::Timeout) => {
                    if sender.is_finished() && emits.last() == Some(&last_expected) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let send_elapsed = sender.join().unwrap();
        assert_eq!(emits.first(), Some(&"r0"), "首个事件必须前沿送达");
        assert_eq!(
            emits.last().copied(),
            Some(last_expected),
            "trailing 必须收敛到最新事件: {emits:?}"
        );
        // 钳频上界按**实际**发送时长折算窗口数（名义 900ms ≈ 3 窗口，慢机上可拉长数倍）：
        // 按名义算会把「合流器正常、机器慢」误判成刷爆。leading + 每窗口至多一次 trailing。
        let windows = send_elapsed.as_millis() as usize / BOARD_COALESCE.as_millis() as usize + 2;
        assert!(
            emits.len() >= 2 && emits.len() <= windows + 2,
            "持续流须被钳在每窗口约一次: {} 次 emit > 窗口数 {windows}",
            emits.len()
        );
    }

    /// 发送端断开（进程收尾）→ 合流线程退出，不空转。已入队的事件仍先补发完。
    #[test]
    fn loop_exits_after_sender_disconnect() {
        let (tx, out, h) = spawn_collector();
        tx.send("last").unwrap();
        drop(tx);
        h.join().expect("发送端断开后合流线程应退出");
        // 断开前已入队的事件不丢：leading 或 trailing 必送达过。
        assert_eq!(recv_within(&out, Duration::from_millis(0)), Some("last"));
    }
}
