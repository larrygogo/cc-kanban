//! 对话历史应用服务与它的薄 Tauri 适配层。
//!
//! command 只负责调度 blocking 工作。DB 读取、transcript 路径解析、文件增量解析、
//! 分页与 mtime 并发控制都住在这里——crate 根不再持有第二台对话状态机。

use meowo_protocol::ipc::{
    AgentModeDto, ChatHistoryDto as ChatHistory, PendingReviewKind, SubagentProbeDto,
};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Per-session transcript (mtime, path) used to detect same-length rewrites
/// and transcript relocation across profiles.
#[derive(Default)]
pub(crate) struct ChatMtimes {
    entries: std::collections::HashMap<i64, ChatMtimeEntry>,
    tick: u64,
    /// errored 的节流缓存:(采样时刻, 值)。见 [`ERRORED_SAMPLE_MS`]。
    errored: std::collections::HashMap<i64, (i64, bool)>,
}

#[derive(Clone)]
struct ChatMtimeEntry {
    mtime: std::time::SystemTime,
    version: u64,
    /// 上次解析的 transcript 路径。跨 profile 恢复会把会话文件复制到另一个数据目录，
    /// 路径解析随 mtime 切到新文件——字节偏移是对旧文件的记账，必须察觉切换并全量重读。
    path: std::path::PathBuf,
}

impl ChatMtimes {
    const CAP: usize = 32;

    fn get(&self, session_id: i64) -> Option<ChatMtimeEntry> {
        self.entries.get(&session_id).cloned()
    }

    /// Compare-and-swap prevents a slower read from overwriting a newer observation.
    fn put_if_current(
        &mut self,
        session_id: i64,
        seen_version: Option<u64>,
        mtime: std::time::SystemTime,
        path: std::path::PathBuf,
    ) {
        if self.entries.get(&session_id).map(|e| e.version) != seen_version {
            return;
        }
        self.put(session_id, mtime, path);
    }

    fn put(&mut self, session_id: i64, mtime: std::time::SystemTime, path: std::path::PathBuf) {
        self.tick += 1;
        let version = self.tick;
        self.entries.insert(
            session_id,
            ChatMtimeEntry {
                mtime,
                version,
                path,
            },
        );
        if self.entries.len() > Self::CAP {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.version)
                .map(|(id, _)| *id);
            if let Some(id) = oldest {
                self.entries.remove(&id);
            }
        }
    }

    fn errored_cached(&self, session_id: i64, now_ms: i64) -> Option<bool> {
        self.errored
            .get(&session_id)
            .filter(|(sampled_at, _)| now_ms.saturating_sub(*sampled_at) < ERRORED_SAMPLE_MS)
            .map(|(_, value)| *value)
    }

    fn put_errored(&mut self, session_id: i64, now_ms: i64, value: bool) {
        self.errored.insert(session_id, (now_ms, value));
        if self.errored.len() > Self::CAP {
            let oldest = self
                .errored
                .iter()
                .min_by_key(|(_, (sampled_at, _))| *sampled_at)
                .map(|(id, _)| *id);
            if let Some(id) = oldest {
                self.errored.remove(&id);
            }
        }
    }
}

/// Far more than one screen, while keeping first-open IPC and DOM work bounded.
const FIRST_PAGE_ITEMS: usize = 200;

/// 存活信号,带进 [`load_chat_history`]:进程表快照(TTL 缓存,与看板共享)加该会话的
/// 托管 PTY 活性——hook 未认领 pid / 事件宽限过期时的存活兜底,与看板同口径。
/// owned:采样发生在 spawn_blocking 闭包内,持有权直接随值走,不做生命周期穿针。
struct LiveSignals {
    alive: std::sync::Arc<std::collections::HashSet<i64>>,
    pty_live: bool,
    /// broker 此刻还压着这个会话的审批请求。用于 `pending_review` 的存活校正
    /// (见 session_query::pending_review_live):hook 落库的那一位在权限已放行后仍会
    /// 滞留到 PostToolUse——被放行的工具跑 20 分钟,对话页就错挂 20 分钟「Agent 请求权限」。
    broker_approval: bool,
    /// agent 自建后台会话的索引(claude FleetView),与看板共享同一份 TTL 快照。
    runtimes: std::sync::Arc<super::session_query::SessionRuntimeIndex>,
}

/// errored 的重采样间隔:transcript 分析走共享 mtime 缓存,但 agent 流式输出期间文件
/// 每轮都在变,每轮全采样等于对同一批新增字节做两遍解析(分析器 + 聊天增量各一遍),
/// 且解析在与侧栏共享的缓存锁内。错误徽标容忍 ~5s 延迟,换掉数倍的重复解析。
const ERRORED_SAMPLE_MS: i64 = 5_000;

/// get_chat_history 的读取请求：尾部增量记账（offset）、整段读取（full，仅接续段历史等
/// 静态内容用）、向上翻页的上界（before = 当前已加载窗口首行的字节偏移）。
#[derive(Default, Clone, Copy)]
struct ChatRead {
    offset: u64,
    full: bool,
    before: Option<u64>,
}

fn load_chat_history(
    db_path: &Path,
    chat_mtimes: &Mutex<ChatMtimes>,
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    live: LiveSignals,
    session_id: i64,
    read: ChatRead,
) -> Result<ChatHistory, String> {
    let ChatRead { offset, full, before } = read;
    let prev = chat_mtimes
        .lock()
        .ok()
        .and_then(|seen| seen.get(session_id));
    let prev_mtime = prev.as_ref().map(|entry| entry.mtime);
    let prev_version = prev.as_ref().map(|entry| entry.version);
    let store = super::open_store(db_path)?;
    let header = store
        .session_header(session_id)
        .map_err(|e| e.to_string())?;
    let context = store
        .session_context(&header.cc_session_id)
        .map_err(|e| e.to_string())?;
    let mut history = ChatHistory {
        session_id,
        cc_session_id: header.cc_session_id.clone(),
        title: header
            .title
            .clone()
            .unwrap_or_else(|| "(未命名会话)".to_string()),
        status: header.status.clone(),
        provider: header.provider.clone(),
        cwd: header.cwd.clone(),
        extra_dirs: header.extra_dirs.clone(),
        supported: false,
        items: Vec::new(),
        offset,
        reset: false,
        pending_review: super::session_query::pending_review_live(
            &header.provider,
            header.pending_review.as_deref(),
            live.broker_approval,
        )
        .and_then(PendingReviewKind::from_stored),
        model: context.model,
        agent_modes: Vec::new(),
        context_pct: context.used_pct,
        context_window: context.window_size,
        current_activity: header.current_activity.clone(),
        // 与看板 tab_class 的地基同源（session_connected）：DB 的 running 在进程死后、
        // reaper 收尾前是滞留值，直接展示会出现「假运行中」。
        connected: super::session_query::session_connected(
            &header.status,
            header.pid,
            super::session_query::process_alive(header.pid, &live.alive, live.pty_live),
            header.last_event_at,
            super::now_ms(),
        ),
        // 待办由 hook 落库（快照式待办工具），与 transcript 解析无关，故所有 provider 都取。
        todos: store
            .task_id_of_session_pub(session_id)
            .and_then(|task_id| store.list_todos(task_id))
            .map(|todos| {
                todos
                    .into_iter()
                    .map(|todo| meowo_protocol::ipc::TodoDto {
                        content: todo.content,
                        status: todo.status.as_str().to_string(),
                        stale: todo.stale,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        has_more: false,
        earliest: 0,
        errored: false,
        pty_managed: live.pty_live,
        // 托管 PTY 在跑的会话绝不算后台：resume 后 claude 的新旧运行时索引会短暂并存
        // （旧 bg worker 还在刷新时间戳），此时误判为后台会让前端吞掉终端按键并把用户
        // 强制切回对话页。本 GUI 自己 spawn 的 PTY 是最强的「交互态」证据。
        background: !live.pty_live
            && super::session_query::is_background(
                &live.runtimes,
                &header.provider,
                &header.cc_session_id,
            ),
        archived: header.archived,
        last_user_text: header.last_user_text.clone(),
        last_ai_text: header.last_ai_text.clone(),
        predecessor_id: header.predecessor_id,
        superseded_by: header.superseded_by,
    };
    // errored 与侧栏/贴纸走同一入口(session_query::analyze_transcript,同口径由代码保证)。
    // 5s 节流:agent 流式输出期间 transcript 每轮都在变,每轮全采样会对同一批新增字节
    // 做两遍解析(分析器 + 下面的聊天增量各一遍)且解析持共享缓存锁;错误徽标容忍 ~5s 延迟。
    {
        let now_ms = super::now_ms();
        let cached = chat_mtimes
            .lock()
            .ok()
            .and_then(|seen| seen.errored_cached(session_id, now_ms));
        history.errored = match cached {
            Some(value) => value,
            None => {
                let value = super::session_query::analyze_transcript(
                    tx_cache,
                    &history.provider,
                    history.cwd.as_deref(),
                    &header.cc_session_id,
                )
                .map(|info| info.error.is_some())
                .unwrap_or(false);
                if let Ok(mut seen) = chat_mtimes.lock() {
                    seen.put_errored(session_id, now_ms, value);
                }
                value
            }
        };
    }
    let spec = meowo_agent::by_id(&history.provider)
        .and_then(|agent| agent.telemetry())
        .and_then(|telemetry| telemetry.transcript());
    let Some(spec) = spec.filter(|spec| spec.supports_chat()) else {
        return Ok(history);
    };
    history.supported = true;
    let Some(path) =
        spec.resolve_transcript_path(None, history.cwd.as_deref(), &header.cc_session_id)
    else {
        history.reset = offset > 0;
        return Ok(history);
    };
    // 「加载更早」向上翻页：before = 当前已加载窗口的首行字节（首读/上一屏响应里的
    // earliest）。只解析 [0, before) 这一段并取其尾部一屏，长会话反复上翻不再是
    // 整文件重读 × 页数。尾部增量的记账（offset/mtime）不在这条路径上动，轮询不受影响。
    if let Some(end) = before {
        if let Some(window) = meowo_agent::read_chat_window(spec, &path, end, FIRST_PAGE_ITEMS) {
            history.items = window.items;
            history.has_more = window.has_more;
            history.earliest = window.start;
        }
        return Ok(history);
    }
    // 路径切换(跨 profile 恢复后解析到另一个数据目录里的延续文件):前端的字节偏移
    // 是对旧文件的记账,对新文件无意义——从头重读并向前端标记 reset,清空重灌。
    let path_changed = prev.as_ref().is_some_and(|entry| entry.path != path);
    let (base_offset, base_mtime) = if path_changed {
        (0, None)
    } else {
        (offset, prev_mtime)
    };
    let mut delta = meowo_agent::read_chat_delta_paged(
        spec,
        &path,
        base_offset,
        base_mtime,
        if full { 0 } else { FIRST_PAGE_ITEMS },
    );
    if path_changed && offset > 0 {
        delta.reset = true;
    }
    if let (Ok(mut seen), Some(mtime)) = (chat_mtimes.lock(), delta.mtime) {
        seen.put_if_current(session_id, prev_version, mtime, path.clone());
    }
    history.offset = delta.offset;
    history.reset = delta.reset;
    history.agent_modes = delta
        .agent_modes
        .into_iter()
        .map(|mode| AgentModeDto {
            dimension: mode.dimension,
            value: mode.value,
        })
        .collect();
    let items = delta.items;
    // hasMore/earliest 只在整读（首读或 reset 重读）里有意义：首屏被裁成尾部一屏时，
    // start 指向保留窗口的首行字节（>0 = 前面还有更早历史），它就是向上翻页的上界。
    // 增量响应里这两字段前端不采信（earliest 恒置 0 防误用）。
    let full_read = offset == 0 || delta.reset;
    history.has_more = full_read && delta.start > 0;
    history.earliest = if full_read { delta.start } else { 0 };
    history.items = items;
    Ok(history)
}

/// 读取一次子任务委派的完整时间线（用户在对话页展开时按需调用）。
///
/// 不走 [`load_chat_history`] 的增量路径：侧车流是已经写完的独立文件，整读一次即可，
/// 也不该让历史轮询顺带承担它的成本。
fn load_subagent_transcript(
    db_path: &Path,
    session_id: i64,
    tool_use_id: &str,
) -> Result<Vec<meowo_protocol::ipc::SubagentRun>, String> {
    let store = super::open_store(db_path)?;
    let header = store
        .session_header(session_id)
        .map_err(|e| e.to_string())?;
    let spec = meowo_agent::by_id(&header.provider)
        .and_then(|agent| agent.telemetry())
        .and_then(|telemetry| telemetry.transcript())
        .ok_or("该 Agent 不提供结构化会话记录")?;
    let path = spec
        .resolve_transcript_path(None, header.cwd.as_deref(), &header.cc_session_id)
        .ok_or("找不到会话记录文件")?;
    let runs = meowo_agent::transcript::read_subagent_chat(spec, &path, tool_use_id);
    if runs.is_empty() {
        return Err("找不到该子任务的记录".into());
    }
    Ok(runs)
}

/// 实测若干条**未结**委派此刻的状态（用户展开进度面板时按需调用）。
///
/// 折叠状态下的进度只能来自主链回执，而并行委派的回执要等同一步里的工具全部跑完才
/// 一起写盘——整批跑完之前，先收工的子任务在主链上毫无痕迹，面板只能一律显示「在跑」
/// （实拍：4 个 explore 里 1 个已完成，面板仍是 0/4，四行耗时全按委派时刻起算）。
/// 侧车流自己带着终结标记，这里逐条读它的尾部补齐。
///
/// 与 [`load_subagent_transcript`] 同样**不进**历史轮询热路径：只在面板展开
/// 且确有在跑的委派时调用，读的也只是各侧车的尾部窗口。
///
/// 定位不到侧车、或该 provider 不留状态信号时，这条 id 直接缺席返回值——调用方维持
/// 原判即可，「读不到」不等于「已结束」。整个会话读不出来也返回空表而不是错误：这条
/// 路径每秒都会走一次，不该在界面上刷错误。
fn probe_subagents(
    db_path: &Path,
    session_id: i64,
    tool_use_ids: &[String],
) -> Result<std::collections::HashMap<String, SubagentProbeDto>, String> {
    let mut probes = std::collections::HashMap::new();
    if tool_use_ids.is_empty() {
        return Ok(probes);
    }
    let store = super::open_store(db_path)?;
    let header = store
        .session_header(session_id)
        .map_err(|e| e.to_string())?;
    let Some(spec) = meowo_agent::by_id(&header.provider)
        .and_then(|agent| agent.telemetry())
        .and_then(|telemetry| telemetry.transcript())
    else {
        return Ok(probes);
    };
    let Some(path) =
        spec.resolve_transcript_path(None, header.cwd.as_deref(), &header.cc_session_id)
    else {
        return Ok(probes);
    };
    for tool_use_id in tool_use_ids {
        if let Some(probe) = meowo_agent::transcript::probe_subagent_state(spec, &path, tool_use_id)
        {
            probes.insert(tool_use_id.clone(), probe);
        }
    }
    Ok(probes)
}

/// 重读该会话当前的模型并落库。
///
/// 模型平时由 Stop hook 写入，但 `/model` 切换本身不产生 Stop——不发下一条消息就永远不刷新，
/// 对话页和贴纸都还挂着旧模型。GUI 驱动的切换完成后调一次即可：一次有界读，不进热路径。
fn refresh_model(db_path: &Path, session_id: i64) -> Result<Option<String>, String> {
    let store = super::open_store(db_path)?;
    let header = store
        .session_header(session_id)
        .map_err(|e| e.to_string())?;
    let model = meowo_agent::by_id(&header.provider)
        .and_then(|agent| agent.telemetry())
        .map(|telemetry| {
            telemetry.stop_outputs(&meowo_agent::caps::HookContext {
                session_id: &header.cc_session_id,
                transcript_path: None,
                last_assistant_message: None,
            })
        })
        .and_then(|out| out.model);
    if let Some(model) = model.as_deref() {
        store
            .set_session_context(
                &header.cc_session_id,
                None,
                None,
                Some(model),
                super::now_ms(),
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(model)
}

#[tauri::command]
pub(crate) async fn refresh_session_model(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Result<Option<String>, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || refresh_model(&db_path, session_id))
        .await
        .map_err(|e| e.to_string())?
}

/// 用会话日志里的待办快照重建 DB。
///
/// 待办平时由 hook 落库，但 hook 只在 meowo 在场时才捕获得到。以下几种情况 DB 会与
/// agent 的真实清单脱节，而日志里一直是对的：
/// - 中途才启动 meowo（agent 早就调过待办工具）；
/// - hook 曾漏接或写库失败；
/// - 早先的解析有误（如状态别名不认识，已完成项被降级成待办）。
///
/// 一次有界读 + 整份覆盖，不进历史轮询热路径；由前端在切换会话时调一次。
/// 覆盖在 store 层有保守口径（见 sync_todos_rebuild）：DB 现有行已全部标成「上一任务」
/// 残留、且重建快照与它们逐条相同时，说明日志里只有旧任务那一版——保留 stale 不洗白。
fn refresh_todos(db_path: &Path, session_id: i64) -> Result<usize, String> {
    let store = super::open_store(db_path)?;
    let header = store
        .session_header(session_id)
        .map_err(|e| e.to_string())?;
    let Some(todos) = meowo_agent::by_id(&header.provider)
        .and_then(|agent| agent.telemetry())
        .and_then(|telemetry| {
            telemetry.read_todos(&meowo_agent::caps::HookContext {
                session_id: &header.cc_session_id,
                transcript_path: None,
                last_assistant_message: None,
            })
        })
    else {
        // 该 agent 不从日志提供待办（如 claude 现版本用增量事件）——保持 DB 现状，
        // 不能拿「读不到」当成「清单已清空」去覆盖 hook 已经落好的数据。
        return Ok(0);
    };
    let inputs: Vec<meowo_store::TodoInput> = todos
        .into_iter()
        .map(|todo| meowo_store::TodoInput {
            content: todo.content,
            // 状态词归一化在这里做：插件如实带出 agent 写的词（kimi 是 done）。
            status: meowo_store::TodoStatus::from_str(&todo.status),
        })
        .collect();
    let count = inputs.len();
    store
        // 被动重建:不刷会话活跃时刻、不把 waiting 顶回 running(打开对话窗不是会话活动)。
        .sync_todos_rebuild(session_id, &inputs, super::now_ms())
        .map_err(|e| e.to_string())?;
    Ok(count)
}

#[tauri::command]
pub(crate) async fn refresh_session_todos(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Result<usize, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || refresh_todos(&db_path, session_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn get_subagent_transcript(
    state: State<'_, super::AppState>,
    session_id: i64,
    tool_use_id: String,
) -> Result<Vec<meowo_protocol::ipc::SubagentRun>, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        load_subagent_transcript(&db_path, session_id, &tool_use_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn probe_subagent_states(
    state: State<'_, super::AppState>,
    session_id: i64,
    tool_use_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, SubagentProbeDto>, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        probe_subagents(&db_path, session_id, &tool_use_ids)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn get_chat_history(
    state: State<'_, super::AppState>,
    session_id: i64,
    offset: u64,
    full: Option<bool>,
    before: Option<u64>,
) -> Result<ChatHistory, String> {
    let db_path = state.db_path.clone();
    let chat_mtimes: Arc<Mutex<ChatMtimes>> = state.chat_mtimes.clone();
    let tx_cache = state.tx_cache.clone();
    // 进程表采样(TTL 缓存,与看板共享)在 spawn_blocking 里做:冷采样 Windows 上要
    // 30-120ms,不能挂在 async-runtime 线程上。PTY 活性是纯内存查表,留在外面无妨。
    let snapshots = state.process_snapshots.clone();
    let runtimes = state.session_runtimes.clone();
    let pty_live = state.ptys.is_active(session_id);
    let broker_approval = state.ptys.pending_approval(session_id).is_some();
    tauri::async_runtime::spawn_blocking(move || {
        load_chat_history(
            &db_path,
            &chat_mtimes,
            &tx_cache,
            LiveSignals {
                alive: snapshots.snapshot(),
                pty_live,
                runtimes: runtimes.snapshot(),
                broker_approval,
            },
            session_id,
            ChatRead {
                offset,
                full: full.unwrap_or(false),
                before,
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

// 粘贴附件的单文件上限与目录约定统一取自 fsutil（与排队插话图片同源，勿再各写一份）。
use meowo_agent::fsutil::{paste_root, PASTE_MAX_BYTES};

/// 把粘贴进对话输入框的图片/文件落成临时文件，返回绝对路径接入现有附件流程。
///
/// 为什么要宿主代劳：webview 的剪贴板只给 File **内容**，拿不到源文件路径，而附件协议
/// 是「把路径列表交给 CLI 自己读」。落在系统临时目录的 meowo-paste 子目录，交给 OS 的
/// 临时清理策略回收——CLI 在发送后的下一个回合就会读走它。
/// 文件名只取 basename 并过滤路径分隔符（杜绝 `..\` 穿越），落进带时间戳的独立子目录，
/// 既避免同名互踩，附件条上又能显示原始文件名。扩展名过白名单（见
/// [`PASTE_EXT_ALLOWLIST`]）：名单外一律追加 `.bin`，杜绝可执行文件借「粘贴」落盘。
#[tauri::command]
pub(crate) async fn save_pasted_attachment(
    file_name: String,
    data_base64: String,
) -> Result<String, String> {
    // async + spawn_blocking：同步命令跑在主线程，而这里要解码最多 ~43MB 的 base64 再写
    // 最多 32MB 磁盘——粘贴大图/大文件会把消息泵冻住肉眼可见的一段时间。
    tauri::async_runtime::spawn_blocking(move || {
        save_pasted_attachment_blocking(file_name, data_base64)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 粘贴附件允许保留的扩展名（比较前先转小写）。粘贴进对话输入框的只可能是截图/图片
/// 或文本片段，不存在合法的「粘贴一个可执行文件」场景；而落盘路径会进 transcript、
/// 交给 CLI 读取，还能经 open_path_with 的 default 分支（explorer 系统关联）打开——
/// 若放行 .exe/.bat/.lnk 等扩展名，「粘贴」就成了任意代码落盘再拉起的第一跳。
/// 名单外（含无扩展名）不拒绝而是追加 `.bin`：内容原样保留、原始名字仍可辨认，
/// 但系统不再按危险类型关联执行。`bin` 本身在名单内，避免兜底名被二次追加。
const PASTE_EXT_ALLOWLIST: &[&str] = &[
    // 图片
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "heic", "heif", "ico",
    // 文本
    "txt", "md", "markdown", "log", "json", "jsonl", "csv", "tsv", "xml", "yaml", "yml", "toml",
    "ini", "patch", "diff", "bin",
];

/// 白名单外的扩展名（或没有扩展名）→ 追加 `.bin` 使其失去系统关联。大小写不敏感。
fn neutralize_paste_name(safe: String) -> String {
    let allowed = Path::new(&safe)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| PASTE_EXT_ALLOWLIST.contains(&ext.as_str()));
    if allowed {
        safe
    } else {
        format!("{safe}.bin")
    }
}

fn save_pasted_attachment_blocking(
    file_name: String,
    data_base64: String,
) -> Result<String, String> {
    use base64::Engine;
    // 编码后长度 ≈ 4/3 原始大小：先按编码长度挡住超大 payload，再解码。
    if data_base64.len() > PASTE_MAX_BYTES / 3 * 4 + 4 {
        return Err("附件过大（上限 32MB）".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("空附件".into());
    }
    if bytes.len() > PASTE_MAX_BYTES {
        return Err("附件过大（上限 32MB）".into());
    }
    let safe: String = Path::new(&file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .chars()
        .filter(|c| !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(80)
        .collect();
    let safe = if safe.trim().is_empty() {
        "pasted.bin".to_string()
    } else {
        safe
    };
    // 扩展名白名单在 basename 化之后做：此刻名字已不含路径分隔符，判定的就是最终落盘名。
    let safe = neutralize_paste_name(safe);
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = paste_root().join(format!("{}-{seq}", super::now_ms()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(safe);
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// meowo-paste 落盘的回收期限。「交给 OS 临时清理策略」在 Windows 上是空头支票——
/// 默认没人清 %TEMP%，不主动清就永久累积。取 30 天而不是更短：这里的文件会被回读——
/// `queued/` 下是 transcript 渲染的插话/用户消息图片（fsutil::persist_paste_bytes 的
/// 消费方），粘贴目录的路径也进了 transcript 正文供历史附件展示，删早了历史对话里
/// 的图就成了裂图。meowo-handoff 的交接文件没有回读方，但同为一次性临时落盘，
/// 复用同一期限一并回收，不再单独立一套规矩。
const PASTE_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 60 * 60);

/// 启动时后台清理过期的粘贴/插话附件与交接文件。全程尽力而为：任何一步失败都静默跳过，
/// 绝不阻塞启动、不打扰用户。
pub(crate) fn spawn_paste_cleanup() {
    std::thread::spawn(|| {
        cleanup_expired_under(&paste_root());
        cleanup_expired_under(&meowo_agent::fsutil::handoff_root());
    });
}

/// 按 mtime 清掉 `root` 下过期的条目：一次性目录（`{时间戳}-…`）整目录删，
/// 长期目录（paste 的 `queued/`）逐文件清。两个临时根的结构约定一致，共用这一份。
fn cleanup_expired_under(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let now = std::time::SystemTime::now();
    let expired = |meta: &std::fs::Metadata| {
        meta.modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age > PASTE_TTL)
    };
    for entry in entries.flatten() {
        // queued/ 是长期目录（transcript 图片按内容名幂等复用，新旧文件混住）：
        // 按整目录 mtime 删会把仍在被回读的新图连坐，按单个文件逐个清。
        if entry.file_name() == "queued" {
            let Ok(files) = std::fs::read_dir(entry.path()) else { continue };
            for file in files.flatten() {
                if file.metadata().is_ok_and(|meta| expired(&meta)) {
                    let _ = std::fs::remove_file(file.path());
                }
            }
            continue;
        }
        // 其余是 `{时间戳}-{序号}` 的一次性粘贴/交接目录：落盘后不再写入，
        // 目录 mtime 即落盘时间，过期整目录删。
        let Ok(meta) = entry.metadata() else { continue };
        if !expired(&meta) {
            continue;
        }
        if meta.is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        } else {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 原生图片附加的剪贴板快照。发送时逐张把附件写进系统剪贴板（Ctrl-V 让 TUI 自己读、
/// 走它的原生图片附加），这会顶掉用户剪贴板里原有的内容——首次写入前快照，发送结束
/// （成功/失败/回退）由 `clipboard_restore` 还原。发送路径串行，单槽足够。
struct ClipboardSnapshot {
    text: Option<String>,
    image: Option<arboard::ImageData<'static>>,
}
static CLIPBOARD_SNAPSHOT: std::sync::Mutex<Option<ClipboardSnapshot>> =
    std::sync::Mutex::new(None);

/// 剪贴板图片的输入上限。附件路径可以是对话框/拖拽选中的任意本地图片（见下），
/// 唯一能挡的是资源滥用：超大文件与解码炸弹（小 PNG 解出数 GB 位图）都在读盘/解码前拦下。
const CLIPBOARD_IMAGE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const CLIPBOARD_IMAGE_MAX_PIXELS: u64 = 100_000_000;

/// 把本地图片文件写进系统剪贴板（供紧随其后的 Ctrl-V 原生附加）。首次写入前自动快照
/// 现有剪贴板内容。读文件/解码/写剪贴板任一步失败都报错——调用方据此回退指令文本，
/// 绝不能照常发 Ctrl-V（那会把剪贴板里别人的内容附给 agent）。
///
/// 路径不走 resolve_inside：附件可来自粘贴落盘（meowo-paste）、文件对话框或拖拽，
/// 后两者是用户显式选中的任意目录文件，没有可用的 cwd 前缀可校验。风险面有限：
/// 内容必须能按图片解码才会进剪贴板，且后端不提供把剪贴板图片读回渲染层的命令，
/// 构不成读任意文件的回传信道。这里只做尺寸/像素上限，防资源滥用。
#[tauri::command]
pub(crate) async fn clipboard_set_image(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        if size > CLIPBOARD_IMAGE_MAX_BYTES {
            return Err("图片过大（上限 64MB）".into());
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        // 先只读图片头取尺寸，不解码：挡住「小文件解出巨幅位图」的解码炸弹。
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .into_dimensions()
            .map_err(|e| e.to_string())?;
        if u64::from(width) * u64::from(height) > CLIPBOARD_IMAGE_MAX_PIXELS {
            return Err("图片像素过多，无法写入剪贴板".into());
        }
        let rgba = image::load_from_memory(&bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let (width, height) = rgba.dimensions();
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        {
            let mut slot = CLIPBOARD_SNAPSHOT.lock().map_err(|e| e.to_string())?;
            if slot.is_none() {
                *slot = Some(ClipboardSnapshot {
                    text: clipboard.get_text().ok().filter(|t| !t.is_empty()),
                    image: clipboard.get_image().ok().map(|img| arboard::ImageData {
                        width: img.width,
                        height: img.height,
                        bytes: std::borrow::Cow::Owned(img.bytes.into_owned()),
                    }),
                });
            }
        }
        clipboard
            .set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: std::borrow::Cow::Owned(rgba.into_raw()),
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 还原 `clipboard_set_image` 快照下来的剪贴板内容；没有快照时为空操作。
#[tauri::command]
pub(crate) async fn clipboard_restore() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let snapshot = CLIPBOARD_SNAPSHOT
            .lock()
            .map_err(|e| e.to_string())?
            .take();
        let Some(snapshot) = snapshot else { return Ok(()) };
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        if let Some(image) = snapshot.image {
            clipboard.set_image(image).map_err(|e| e.to_string())?;
        } else if let Some(text) = snapshot.text {
            clipboard.set_text(text).map_err(|e| e.to_string())?;
        } else {
            clipboard.clear().map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
/// 读剪贴板文本（终端右键粘贴用，见 ManagedTerminal 的 contextmenu 处理）。
/// navigator.clipboard.readText 在 WebView2 里要站点权限弹窗，走后端 arboard 零打扰；
/// 剪贴板无文本内容（空/图片）回 None，调用方静默跳过。
#[tauri::command]
pub(crate) async fn clipboard_text() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        Ok(clipboard.get_text().ok().filter(|text| !text.is_empty()))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── 对话内容全文搜索 ─────────────────────────────────────────────────────────
// transcript 不落库(库里只有 last_user_text/last_ai_text 摘要),全文搜索按活跃序
// 扫最近会话的 transcript 文件(JSONL)。这是**显式动作**(前端点按钮触发,不随击键),
// 扫描量有硬上限,一次点击的等待压在秒级以内。

/// 全文搜索的一条命中:会话 + 命中处摘录。每会话至多一条——结果回答的是
/// 「哪个会话聊过它」,不是列出每一处。
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranscriptSearchHit {
    pub session_id: i64,
    pub title: String,
    pub project_name: String,
    pub excerpt: String,
}

/// 扫描上限:最近活跃的会话数(活跃+已归档合并后)、单文件读取字节数、总命中数。
const TRANSCRIPT_SEARCH_SESSIONS: usize = 200;
const TRANSCRIPT_SEARCH_FILE_BYTES: u64 = 8 * 1024 * 1024;
const TRANSCRIPT_SEARCH_MAX_HITS: usize = 30;
/// 单行超过它就跳过:transcript 里的粘贴图片是整行 base64,扫它只有噪声没有价值。
const TRANSCRIPT_SEARCH_LINE_BYTES: usize = 200 * 1024;

/// ASCII 大小写折叠的子串查找。不用 `to_lowercase`:某些 Unicode 小写化会变字节长度,
/// 偏移映射回原串有越界/断字风险;逐字节 ASCII 折叠对中文(多字节原样比较)天然正确。
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// 摘录:命中位置前后各 ~60 字节,按 char 边界收敛,粗略还原 JSON 转义。
fn excerpt_around(line: &str, at: usize, len: usize) -> String {
    let start_target = at.saturating_sub(60);
    let end_target = (at + len + 60).min(line.len());
    let start = (0..=start_target)
        .rev()
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(0);
    let end = (end_target..=line.len())
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(line.len());
    let mut text = line[start..end]
        .replace("\\n", " ")
        .replace("\\t", " ")
        .replace("\\\"", "\"");
    if start > 0 {
        text = format!("…{text}");
    }
    if end < line.len() {
        text.push('…');
    }
    text
}

/// 逐行扫一个 transcript 文件,返回首个命中处的摘录。
fn search_file_excerpt(path: &Path, needle: &str) -> Option<String> {
    use std::io::{BufRead, BufReader, Read};
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file.take(TRANSCRIPT_SEARCH_FILE_BYTES));
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.len() > TRANSCRIPT_SEARCH_LINE_BYTES {
            continue;
        }
        if let Some(at) = find_ignore_ascii_case(&line, needle) {
            return Some(excerpt_around(&line, at, needle.len()));
        }
    }
    None
}

fn search_transcripts_blocking(
    db_path: &Path,
    query: &str,
) -> Result<Vec<TranscriptSearchHit>, String> {
    let needle = query.trim();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let store = super::open_store(db_path)?;
    // 活跃与已归档各取一批(用户找旧对话时它多半已被收纳),合并后按活跃序截断。
    let mut sessions = Vec::new();
    for filter in ["all", "archived"] {
        sessions.extend(
            store
                .live_sessions(Some(filter), None, None, None, TRANSCRIPT_SEARCH_SESSIONS)
                .map_err(|e| e.to_string())?,
        );
    }
    sessions.sort_by_key(|row| std::cmp::Reverse(row.session.last_event_at));
    sessions.truncate(TRANSCRIPT_SEARCH_SESSIONS);
    let mut hits = Vec::new();
    for row in sessions {
        if hits.len() >= TRANSCRIPT_SEARCH_MAX_HITS {
            break;
        }
        let Some(spec) = meowo_agent::by_id(&row.provider)
            .and_then(|agent| agent.telemetry())
            .and_then(|telemetry| telemetry.transcript())
            .filter(|spec| spec.supports_chat())
        else {
            continue;
        };
        let Some(path) =
            spec.resolve_transcript_path(None, row.cwd.as_deref(), &row.session.cc_session_id)
        else {
            continue;
        };
        if let Some(excerpt) = search_file_excerpt(&path, needle) {
            hits.push(TranscriptSearchHit {
                session_id: row.session.id,
                title: row.task_title.clone(),
                project_name: row.project_name.clone(),
                excerpt,
            });
        }
    }
    Ok(hits)
}

#[tauri::command]
pub(crate) async fn search_chat_transcripts(
    state: State<'_, super::AppState>,
    query: String,
) -> Result<Vec<TranscriptSearchHit>, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || search_transcripts_blocking(&db_path, &query))
        .await
        .map_err(|e| e.to_string())?
}

// `@` 文件补全不另起命令:复用 fsutil::search_project_files(名称+内容双搜,跳过
// 依赖/构建目录),前端取其中的文件名命中。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_attachment_roundtrip_keeps_name_and_content() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"png-bytes");
        let path = save_pasted_attachment_blocking("shot.png".into(), data).unwrap();
        let path = std::path::PathBuf::from(path);
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("shot.png"));
        assert!(path.starts_with(std::env::temp_dir().join("meowo-paste")));
        assert_eq!(std::fs::read(&path).unwrap(), b"png-bytes");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pasted_attachment_name_cannot_escape_the_paste_dir() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"x");
        let path = save_pasted_attachment_blocking("..\\..\\evil.exe".into(), data).unwrap();
        let path = std::path::PathBuf::from(path);
        // basename 化 + 过滤分隔符：无论名字长什么样，都只能落在 meowo-paste 里。
        assert!(path.starts_with(std::env::temp_dir().join("meowo-paste")));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pasted_attachment_neutralizes_dangerous_extensions() {
        // 白名单外（可执行/脚本/快捷方式/无扩展名）一律追加 .bin，大小写不敏感；
        // 白名单内的图片/文本名原样保留。
        assert_eq!(neutralize_paste_name("pwn.exe".into()), "pwn.exe.bin");
        assert_eq!(neutralize_paste_name("pwn.EXE".into()), "pwn.EXE.bin");
        assert_eq!(neutralize_paste_name("run.bat".into()), "run.bat.bin");
        assert_eq!(neutralize_paste_name("evil.lnk".into()), "evil.lnk.bin");
        assert_eq!(neutralize_paste_name("noext".into()), "noext.bin");
        assert_eq!(neutralize_paste_name("shot.PNG".into()), "shot.PNG");
        assert_eq!(neutralize_paste_name("notes.md".into()), "notes.md");
        assert_eq!(neutralize_paste_name("pasted.bin".into()), "pasted.bin");
        // 全链路：落盘名与判定一致。
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"MZ");
        let path = save_pasted_attachment_blocking("payload.exe".into(), data).unwrap();
        let path = std::path::PathBuf::from(path);
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("payload.exe.bin")
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pasted_attachment_rejects_oversize_and_empty() {
        // 编码长度粗筛：超过上限的 payload 不必真造 32MB，长度骗过第一道即可验证拒绝。
        let oversized = "A".repeat(PASTE_MAX_BYTES / 3 * 4 + 8);
        assert!(save_pasted_attachment_blocking("big.bin".into(), oversized).is_err());
        assert!(save_pasted_attachment_blocking("empty.bin".into(), String::new()).is_err());
    }

    /// 过期条目（mtime 早于 TTL）删除、新鲜条目保留——paste 与 handoff 两个根共用
    /// 这一份清理，规则本身与根无关，用独立临时目录测即可。
    #[test]
    fn cleanup_removes_expired_entries_and_keeps_fresh_ones() {
        let root = std::env::temp_dir().join(format!("meowo-cleanup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // 过期的一次性条目：mtime 拨到 TTL 之前。
        let stale = root.join("1000-1");
        std::fs::write(&stale, b"x").unwrap();
        let old = std::time::SystemTime::now() - PASTE_TTL - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        // 新鲜的一次性目录。
        let fresh = root.join("9999-2");
        std::fs::create_dir_all(&fresh).unwrap();

        cleanup_expired_under(&root);

        assert!(!stale.exists(), "过期条目应被清掉");
        assert!(fresh.exists(), "新鲜条目不该被误删");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn any_path() -> std::path::PathBuf {
        std::path::PathBuf::from("t.jsonl")
    }

    #[test]
    fn stale_mtime_observations_cannot_overwrite_newer_ones() {
        let base = std::time::SystemTime::UNIX_EPOCH;
        let newer = base + std::time::Duration::from_secs(10);
        let mut cache = ChatMtimes::default();
        cache.put(7, base, any_path());
        let version_a = cache.get(7).map(|entry| entry.version);
        let version_b = version_a;
        cache.put_if_current(7, version_b, newer, any_path());
        cache.put_if_current(7, version_a, base, any_path());
        assert_eq!(cache.get(7).map(|entry| entry.mtime), Some(newer));
    }

    #[test]
    fn mtime_cache_evicts_the_stalest_entry_but_keeps_a_hot_session() {
        let base = std::time::SystemTime::UNIX_EPOCH;
        let mut cache = ChatMtimes::default();
        let hot = 1_i64;
        cache.put(hot, base, any_path());
        for i in 0..(ChatMtimes::CAP as i64 + 5) {
            cache.put(100 + i, base, any_path());
            cache.put(
                hot,
                base + std::time::Duration::from_secs(i as u64 + 1),
                any_path(),
            );
        }
        assert!(cache.entries.len() <= ChatMtimes::CAP);
        assert!(cache.get(hot).is_some());
        assert!(cache.get(100).is_none());
        assert!(cache.get(100 + ChatMtimes::CAP as i64 + 4).is_some());
    }

    #[test]
    fn mtime_cache_remembers_the_transcript_path_per_session() {
        let base = std::time::SystemTime::UNIX_EPOCH;
        let mut cache = ChatMtimes::default();
        cache.put(7, base, std::path::PathBuf::from("old.jsonl"));
        let prev = cache.get(7).expect("entry");
        // 模拟 load_chat_history 的路径切换判定:跨 profile 恢复后解析到的新文件
        // 必须被认作「换了文件」,触发从头重读,而不是沿用旧文件的字节偏移。
        assert!(prev.path != std::path::Path::new("new.jsonl"));
        cache.put_if_current(
            7,
            Some(prev.version),
            base,
            std::path::PathBuf::from("new.jsonl"),
        );
        assert_eq!(
            cache.get(7).map(|entry| entry.path),
            Some(std::path::PathBuf::from("new.jsonl"))
        );
    }
}
