//! Chat history application service and its thin Tauri adapter.
//!
//! The command only schedules blocking work. Database reads, transcript resolution,
//! incremental file parsing, paging, and mtime concurrency control live here so the
//! crate root no longer owns a second chat state machine.

use meowo_protocol::ipc::{AgentModeDto, ChatHistoryDto as ChatHistory, PendingReviewKind};
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

fn trim_first_page<T>(items: &mut Vec<T>, full: bool, full_read: bool) -> bool {
    if full || !full_read || items.len() <= FIRST_PAGE_ITEMS {
        return false;
    }
    items.drain(..items.len() - FIRST_PAGE_ITEMS);
    true
}

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
/// 每轮都在变,650ms 全采样等于对同一批新增字节做两遍解析(分析器 + 聊天增量各一遍),
/// 且解析在与侧栏共享的缓存锁内。错误徽标容忍 ~5s 延迟,换掉 8 倍的重复解析。
const ERRORED_SAMPLE_MS: i64 = 5_000;

fn load_chat_history(
    db_path: &Path,
    chat_mtimes: &Mutex<ChatMtimes>,
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    live: LiveSignals,
    session_id: i64,
    offset: u64,
    full: bool,
) -> Result<ChatHistory, String> {
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
                    })
                    .collect()
            })
            .unwrap_or_default(),
        has_more: false,
        errored: false,
        pty_managed: live.pty_live,
        background: super::session_query::is_background(
            &live.runtimes,
            &header.provider,
            &header.cc_session_id,
        ),
        archived: header.archived,
        last_user_text: header.last_user_text.clone(),
        last_ai_text: header.last_ai_text.clone(),
    };
    // errored 与侧栏/贴纸走同一入口(session_query::analyze_transcript,同口径由代码保证)。
    // 5s 节流:agent 流式输出期间 transcript 每轮都在变,650ms 全采样会对同一批新增字节
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
    // 路径切换(跨 profile 恢复后解析到另一个数据目录里的延续文件):前端的字节偏移
    // 是对旧文件的记账,对新文件无意义——从头重读并向前端标记 reset,清空重灌。
    let path_changed = prev.as_ref().is_some_and(|entry| entry.path != path);
    let (base_offset, base_mtime) = if path_changed {
        (0, None)
    } else {
        (offset, prev_mtime)
    };
    let mut delta = meowo_agent::read_chat_delta(spec, &path, base_offset, base_mtime);
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
    let mut items = delta.items;
    history.has_more = trim_first_page(&mut items, full, offset == 0 || delta.reset);
    history.items = items;
    Ok(history)
}

/// 读取一次子任务委派的完整时间线（用户在对话页展开时按需调用）。
///
/// 不走 [`load_chat_history`] 的增量路径：侧车流是已经写完的独立文件，整读一次即可，
/// 也不该让 650ms 的历史轮询顺带承担它的成本。
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
/// 一次有界读 + 整份覆盖，不进 650ms 的历史轮询热路径；由前端在切换会话时调一次。
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
        .sync_todos(session_id, &inputs, super::now_ms())
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
pub(crate) async fn get_chat_history(
    state: State<'_, super::AppState>,
    session_id: i64,
    offset: u64,
    full: Option<bool>,
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
            offset,
            full.unwrap_or(false),
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
/// 既避免同名互踩，附件条上又能显示原始文件名。
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
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = paste_root().join(format!("{}-{seq}", super::now_ms()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(safe);
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// 读系统剪贴板**图像**的指纹(尺寸 + RGBA 内容);剪贴板里不是图像时为 None。只读不写。
///
/// 用途:发送粘贴图片附件前,判断「剪贴板里还是不是刚才粘贴进 meowo 的那张图」——
/// 匹配才敢向 CLI 的 PTY 发 Ctrl-V,让 TUI 自己读剪贴板、走它的原生图片附加
/// (claude 的 `[Image #N]`、kimi 的 `[image:…]`);不匹配(用户中途复制过别的东西)
/// 绝不能发,否则会把错的图附给 agent。
#[tauri::command]
pub(crate) async fn clipboard_image_fingerprint() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        let Ok(image) = clipboard.get_image() else {
            return Ok(None);
        };
        use meowo_agent::codec::{fnv1a, FNV1A_OFFSET};
        let mut hash = FNV1A_OFFSET;
        fnv1a(&mut hash, &(image.width as u64).to_le_bytes());
        fnv1a(&mut hash, &(image.height as u64).to_le_bytes());
        fnv1a(&mut hash, &image.bytes);
        Ok(Some(format!("{hash:016x}")))
    })
    .await
    .map_err(|e| e.to_string())?
}

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
    fn pasted_attachment_rejects_oversize_and_empty() {
        // 编码长度粗筛：超过上限的 payload 不必真造 32MB，长度骗过第一道即可验证拒绝。
        let oversized = "A".repeat(PASTE_MAX_BYTES / 3 * 4 + 8);
        assert!(save_pasted_attachment_blocking("big.bin".into(), oversized).is_err());
        assert!(save_pasted_attachment_blocking("empty.bin".into(), String::new()).is_err());
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

    #[test]
    fn first_page_keeps_the_latest_items_only() {
        let mut items: Vec<_> = (0..FIRST_PAGE_ITEMS + 3).collect();
        assert!(trim_first_page(&mut items, false, true));
        assert_eq!(items.len(), FIRST_PAGE_ITEMS);
        assert_eq!(items[0], 3);

        let mut incremental: Vec<_> = (0..FIRST_PAGE_ITEMS + 3).collect();
        assert!(!trim_first_page(&mut incremental, false, false));
        assert_eq!(incremental.len(), FIRST_PAGE_ITEMS + 3);
    }
}
