//! 会话写命令与它们共享的输入校验。

use tauri::State;

/// 可安全用于 agent 的 resume 参数与 provider 自有的会话路径。
pub(crate) fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

// 三条写库命令都 async + spawn_blocking：同步命令跑在主线程，而 reporter 的 hook 进程
// 会并发写同一个库——撞上写锁时 busy_timeout（3s）就直接冻住消息泵；rename 还要写
// provider 的 telemetry 文件。
#[tauri::command]
pub(crate) async fn rename_session(
    app: tauri::AppHandle,
    state: State<'_, super::AppState>,
    cwd: Option<String>,
    session_id: String,
    title: String,
    provider: Option<String>,
) -> Result<(), String> {
    if !is_safe_id(&session_id) {
        return Err("无效 session_id".into());
    }
    let title: String = title.trim().chars().take(80).collect();
    if title.is_empty() {
        return Err("标题不能为空".into());
    }
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Provider persistence is best-effort; the local database remains the UI source of truth.
        if let Some(telemetry) =
            meowo_agent::resolve(provider.as_deref()).and_then(|agent| agent.telemetry())
        {
            let _ = telemetry.write_rename(&session_id, cwd.as_deref(), &title);
        }
        if let Ok(store) = super::open_store(&db_path) {
            if let Ok(Some(id)) = store.find_session_id_pub(&session_id) {
                let _ = store.set_session_title(id, &title, super::now_ms());
            }
        }
        super::watch::emit_board_changed(&app, "rename");
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn set_archived(
    app: tauri::AppHandle,
    state: State<'_, super::AppState>,
    session_id: i64,
    archived: bool,
) -> Result<(), String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        super::open_store(&db_path)?
            .set_session_archived(session_id, archived, super::now_ms())
            .map_err(|error| error.to_string())?;
        super::watch::emit_board_changed(&app, "set_archived");
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn set_session_note(
    app: tauri::AppHandle,
    state: State<'_, super::AppState>,
    session_id: String,
    note: String,
) -> Result<(), String> {
    if !is_safe_id(&session_id) {
        return Err("无效 session_id".into());
    }
    let note: String = note.chars().take(500).collect();
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        super::open_store(&db_path)?
            .set_session_note(&session_id, &note, super::now_ms())
            .map_err(|error| error.to_string())?;
        super::watch::emit_board_changed(&app, "note");
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 已有会话中途附加目录:落库(merge 进 sessions.extra_dirs,resume/接管重启时回放)
/// + **尽力即时生效**——会话有托管 PTY 时直接把 `/add-dir <dir>`(agent 自己的运行时
/// 命令)写进去,运行中的进程当场拿到访问权;没有托管 PTY(断开/外部终端)只落库,
/// 下次恢复回放兜底。目录校验与新建同规;按会话 provider 查能力声明,不支持的 agent
/// 如实拒绝。
#[tauri::command]
pub(crate) async fn add_session_extra_dir(
    app: tauri::AppHandle,
    state: State<'_, super::AppState>,
    session_id: i64,
    dir: String,
) -> Result<bool, String> {
    let db_path = state.db_path.clone();
    let ptys = state.ptys.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let d = crate::terminal::validate_new_session_cwd(&dir)?;
        let store = super::open_store(&db_path)?;
        let provider = store
            .session_provider(session_id)
            .map_err(|error| error.to_string())?;
        let supported = meowo_agent::resolve(Some(&provider))
            .and_then(|agent| agent.extra_dir_flag())
            .is_some();
        if !supported {
            return Err("该 Agent 不支持附加目录".into());
        }
        let added = store
            .add_session_extra_dir(session_id, &d)
            .map_err(|error| error.to_string())?;
        if added {
            // 即时生效 best-effort:写失败(无托管 PTY/队列满)不报错——库里已记,
            // resume 回放兜底,报错反而会让用户以为整个动作失败了。
            //
            // **绝不盲注**:终端正挂着审批/提问模态(broker 有在途审批,或屏幕检测
            // blocked)时,注入串尾的 \r 会按在模态的高亮项上——等于替用户批准一条
            // 从未过目的命令。此时只落库,恢复回放兜底。
            //
            // 路径**不加引号**。曾经恒加,理由写的是「不加的话含空格路径会在 CLI 的
            // slash 参数解析处断开」——那是个没验证过的假设,实拍反证(用户截图):
            // claude 的 /add-dir 不做 shell 式引号剥离,把引号当成路径字面量的一部分,
            // 报错原文形如
            //   Path C:\...\xbot-admin-front"C:\...\xbot-dashboard" was not found.
            // 引号还让路径不再以盘符开头、于是被当成相对路径拼到了 cwd 后面。同一条
            // 报错也说明它取的是「命令后的整行」而不是按空格切分,所以裸路径对含空格的
            // 目录同样成立(原假设担心的那种断开不存在)。
            // "/add-dir" 是 claude 的运行时命令;将来第二个声明 extra_dir_flag 的
            // agent 落地时,运行时命令串也要纳入插件声明,不得沿用这里的字面量。
            let modal = ptys.approval_session_ids().contains(&session_id)
                || ptys.screen_states().get(&session_id).is_some_and(|sight| sight.state == "blocked");
            if !modal {
                let _ = ptys.write(session_id, format!("/add-dir {d}\r").as_bytes());
            }
            super::watch::emit_board_changed(&app, "extra_dirs");
        }
        Ok(added)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 移除一个附加目录（落库侧）。语义是「下次恢复不再带上」——运行中进程已持有的权限
/// 没有运行时撤销命令,收不回;不做目录存在性校验(目录已被删掉正是要移除它的理由)。
#[tauri::command]
pub(crate) async fn remove_session_extra_dir(
    app: tauri::AppHandle,
    state: State<'_, super::AppState>,
    session_id: i64,
    dir: String,
) -> Result<bool, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let removed = super::open_store(&db_path)?
            .remove_session_extra_dir(session_id, &dir)
            .map_err(|error| error.to_string())?;
        if removed {
            super::watch::emit_board_changed(&app, "extra_dirs");
        }
        Ok(removed)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 对话页改选启动选项后的落库（合并单键写回）：模型/权限是**启动参数**，resume/接管
/// 时回放（`terminal::splice_stored_launch_args`）——不落库的话每次重启都回默认档
/// （用户实拍：会话里切到 1M 上下文档，重启后回落 200K）。
#[tauri::command]
pub(crate) async fn set_session_launch_selection(
    session_id: i64,
    option: String,
    choice: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = super::open_store(&super::db_path()).map_err(|e| e.to_string())?;
        let mut selections: std::collections::HashMap<String, String> = store
            .session_launch_args(session_id)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        selections.insert(option, choice);
        let json = serde_json::to_string(&selections).map_err(|e| e.to_string())?;
        store
            .set_session_launch_args(session_id, &json)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 对话窗权限下拉的回显：读会话存的启动选项（sessions.launch_args，新建/接管改选时落库，
/// 见 `terminal::splice_stored_launch_args`）。有存值时前端把「沿用原设置」直接亮成那一档——
/// 一句黑盒不如具体档位（用户实拍反馈）。读库/解析失败返回空 map：回显是增强，不值得报错。
#[tauri::command]
pub(crate) async fn session_launch_selections(
    session_id: i64,
) -> Result<std::collections::HashMap<String, String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(super::open_store(&super::db_path())
            .ok()
            .and_then(|store| store.session_launch_args(session_id).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::is_safe_id;

    #[test]
    fn session_ids_accept_provider_shapes_and_reject_shell_or_path_syntax() {
        assert!(is_safe_id("a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        assert!(is_safe_id("session_a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        for invalid in [
            "",
            "../../etc/passwd",
            "a/b",
            "a.b",
            "abc; calc",
            "trailing ",
        ] {
            assert!(!is_safe_id(invalid), "unexpectedly accepted {invalid:?}");
        }
        assert!(!is_safe_id(&"a".repeat(129)));
    }
}
