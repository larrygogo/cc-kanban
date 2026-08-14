//! 跨 provider 会话切换：把当前会话的结构化历史导出成交接文件，结束旧进程，
//! 在同一工作目录用目标 agent 起新会话，并经 pending 暂存把接续链留给 claim 落库。
//!
//! 流程顺序是安全性的一部分：**先导出、后杀进程**——导出失败时旧会话分毫未动；
//! 接续链只在 claim 认领时写库——目标 CLI 起不来时旧会话不会被错误标记为已接替，
//! 用户仍可 resume 回旧引擎。

use std::collections::HashMap;

use meowo_protocol::ipc::{LineageEntryDto, SwitchStartedDto};

/// 交接文件的注入语里引用的文件名。固定名 + 时间戳目录：路径可预测性无所谓
/// （内容只是用户自己的会话记录），同秒多次切换靠目录里的 session id 区分。
const HANDOFF_FILE: &str = "handoff.md";

#[tauri::command]
pub(crate) async fn switch_session_provider(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session_id: i64,
    target_provider: String,
    options: Option<HashMap<String, String>>,
) -> Result<SwitchStartedDto, String> {
    let broker = state.ptys.clone();
    let db = state.db_path.clone();
    // 文件 IO + 杀进程 + DB + PTY spawn，全程不碰主线程消息泵（命令线程纪律）。
    tauri::async_runtime::spawn_blocking(move || {
        let store = crate::open_store(&db)?;
        let header = store
            .session_header(session_id)
            .map_err(|e| e.to_string())?;
        if header.superseded_by.is_some() {
            return Err("该会话已被接替，请在最新的延续会话上切换".into());
        }
        let source = meowo_agent::by_id(&header.provider).ok_or("未知来源 agent")?;
        // 来源门槛 = 现成的结构化 transcript 能力谓词，不新增能力槽也不按身份分支：
        // 没有结构化历史就没有可交接的东西（gemini/opencode 只能当切换目标）。
        let spec = source
            .telemetry()
            .and_then(|telemetry| telemetry.transcript())
            .filter(|spec| spec.supports_chat())
            .ok_or("该 Agent 不提供结构化会话记录，无法携带上下文切换")?;
        let target = meowo_agent::resolve(Some(&target_provider)).ok_or("未知目标 agent")?;
        if !target.is_installed() {
            return Err(format!("{} 未安装", target.display_name()));
        }
        let cwd = header
            .cwd
            .clone()
            .ok_or("会话缺少工作目录，无法在原目录延续")?;
        let dir = crate::terminal::validate_new_session_cwd(&cwd)?;

        // == 导出（先导后杀：这里失败旧会话分毫未动）==
        let transcript = spec
            .resolve_transcript_path(None, header.cwd.as_deref(), &header.cc_session_id)
            .ok_or("未找到会话记录文件")?;
        let delta = meowo_agent::read_chat_delta(spec, &transcript, 0, None);
        if delta.items.is_empty() {
            return Err("会话记录为空，没有可交接的上下文".into());
        }
        let context = store
            .session_context(&header.cc_session_id)
            .unwrap_or_default();
        let meta = meowo_agent::HandoffMeta {
            from_provider_name: source.display_name(),
            to_provider_name: target.display_name(),
            cwd: Some(dir.as_str()),
            model: context.model.as_deref(),
            title: header.title.as_deref(),
        };
        let body = meowo_agent::render_history(&delta.items, &meta);
        let handoff_dir = meowo_agent::fsutil::handoff_root()
            .join(format!("{}-{}", crate::now_ms(), session_id));
        std::fs::create_dir_all(&handoff_dir).map_err(|e| format!("创建交接目录失败: {e}"))?;
        let handoff_path = handoff_dir.join(HANDOFF_FILE);
        std::fs::write(&handoff_path, &body).map_err(|e| format!("写入交接文件失败: {e}"))?;

        // == 结束旧进程（判活口径与 takeover 相同，含 pid 归属校验——换代残留的 pid
        // 属于别的会话，杀不得）==
        if crate::terminal::session_agent_alive(&store, session_id)? {
            if let Some(pid) = store.session_pid(session_id).map_err(|e| e.to_string())? {
                crate::terminal::terminate_agent_for_restart(pid)?;
            }
        }
        // 显式收尾而不等 reaper：看板不留「假运行中」窗口。
        store
            .end_session(session_id, crate::now_ms())
            .map_err(|e| e.to_string())?;

        // == 起新会话（与 new_session 同构）。刻意**不继承**旧 launch_args：不同 agent
        // 的 option id 会撞名，旧选择对新 provider 无意义甚至有害。==
        let selections = options.unwrap_or_default();
        let mut argv = target.launch_argv();
        argv.extend(meowo_agent::resolve_launch_args(
            target.launch_options(),
            &selections,
        ));
        let argv = crate::relay::augment_argv(target.id(), argv);
        let env = crate::terminal::launch_env_for_profile(Some(&target_provider), None);
        let temp_id = broker.start_pending(
            app.clone(),
            &argv,
            Some(&dir),
            &env,
            100,
            30,
            target.id().as_str(),
            &selections,
            Some(session_id),
        )?;
        crate::watch::emit_board_changed(&app, "switch_provider");
        Ok(SwitchStartedDto {
            temp_id,
            handoff_path: handoff_path.to_string_lossy().into_owned(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 接续链回看（冷路径：侧栏链徽标的弹层用）。按时间升序返回链上各段。
#[tauri::command]
pub(crate) async fn get_session_lineage(
    state: tauri::State<'_, crate::AppState>,
    session_id: i64,
) -> Result<Vec<LineageEntryDto>, String> {
    let db = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let store = crate::open_store(&db)?;
        let chain = store
            .session_lineage_chain(session_id)
            .map_err(|e| e.to_string())?;
        Ok(chain
            .into_iter()
            .map(|entry| LineageEntryDto {
                id: entry.id,
                provider: entry.provider,
                started_at: entry.started_at,
                ended_at: entry.ended_at,
                model: entry.model,
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}
