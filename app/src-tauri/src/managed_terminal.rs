//! 托管 PTY 与审批 broker 的薄 Tauri command 适配层。

use tauri::State;

#[tauri::command]
pub(crate) async fn start_managed_terminal(
    app: tauri::AppHandle,
    state: State<'_, super::AppState>,
    session_id: i64,
    cols: u16,
    rows: u16,
    // 恢复时可改启动选项（如把权限模式切成跳过）；None = 沿用会话存的选择。
    options: Option<std::collections::HashMap<String, String>>,
) -> Result<(), String> {
    let db_path = state.db_path.clone();
    let broker = state.ptys.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let store = super::open_store(&db_path)?;
        let session = store.get_session(session_id).map_err(|e| e.to_string())?;
        if !super::session_command::is_safe_id(&session.cc_session_id) {
            return Err("无效 session_id".into());
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            if super::terminal::session_agent_alive(&store, session_id)? {
                return Err("会话仍在外部终端运行，不能重复接管".into());
            }
            let cwd = store.session_cwd(session_id).map_err(|e| e.to_string())?;
            let provider = store
                .session_provider(session_id)
                .map_err(|e| e.to_string())?;
            super::terminal::start_managed_resume_sized(
                app,
                broker,
                session_id,
                cwd,
                session.cc_session_id,
                provider,
                super::pty::TerminalSize::new(cols, rows),
                options,
            )
            .map(|_| ())
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = (app, broker, session, cols, rows, options);
            Err("当前平台暂不支持托管终端".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 屏幕检测的诊断输出：末屏文本 + 标题 + 已发布状态 + 现场重跑规则的命中结果。
/// 「这张卡片为什么显示这个状态」的排障入口（herdr `agent explain` 的最小版）——
/// 规则失准时先看这里的真实屏幕再改规则，不许凭想象改。
#[derive(serde::Serialize)]
pub(crate) struct ScreenDetectExplain {
    provider: String,
    /// 防抖后对外发布的状态（卡片显示的那个）。
    published: Option<&'static str>,
    /// 现场对当前屏重跑规则的原始判定（未过防抖）。
    raw_state: Option<&'static str>,
    matched_rule: &'static str,
    title: Option<String>,
    lines: Vec<String>,
}

/// 对一份快照跑规则并组装诊断结果。在线/离线两个入口共用，保证两者判定完全一致——
/// 离线复现出的结论若和线上不同，这个工具就没有意义了。
fn explain_snapshot(
    provider: String,
    snapshot: crate::detect::ScreenSnapshot,
    published: Option<&'static str>,
) -> ScreenDetectExplain {
    let (raw_state, matched_rule) = match crate::detect::evaluate(&provider, &snapshot) {
        Some(crate::detect::Evaluation::Publish(det)) => (Some(det.state.as_str()), det.rule_id),
        Some(hold @ crate::detect::Evaluation::Hold { .. }) => (None, hold.rule_id()),
        None => (None, "provider_unsupported"),
    };
    ScreenDetectExplain {
        provider,
        published,
        raw_state,
        matched_rule,
        title: snapshot.title,
        lines: snapshot.lines,
    }
}

#[tauri::command]
pub(crate) async fn screen_detect_explain(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Result<ScreenDetectExplain, String> {
    let ptys = state.ptys.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (snapshot, provider) = ptys
            .screen_probe_snapshot(session_id)
            .ok_or("该会话没有托管 PTY 屏幕状态")?;
        let published = ptys.screen_states().get(&session_id).map(|sight| sight.state);
        Ok(explain_snapshot(provider, snapshot, published))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 离线版：直接对一段**末屏文本**跑规则，不需要活会话。
///
/// 规则匹配的是各 agent TUI 的界面文案，必然随对方改版腐坏；用户报「状态显示不对」时，
/// 在线 explain 要求复现现场——而现场往往一次性。有了这个入口，把当时的末屏文本存下来
/// （在线 explain 的 lines 直接可用，或从终端复制）就能反复验证：改一版规则跑一次，
/// 修完还能把它固化成 detect.rs 的单测。
///
/// `title` 是 OSC 0/2 设置的终端标题——claude 的 spinner/✳ 判定全在标题上，漏传它
/// 会得出与线上不同的结论，故显式作为参数而不是从文本里猜。
#[tauri::command]
pub(crate) fn screen_detect_explain_text(
    provider: String,
    text: String,
    title: Option<String>,
) -> ScreenDetectExplain {
    let lines = text.lines().map(str::to_string).collect();
    let snapshot = crate::detect::ScreenSnapshot::new(lines, title);
    // 离线快照没有「已发布状态」：那是防抖后的运行时产物，与单帧无关。
    explain_snapshot(provider, snapshot, None)
}

#[cfg(test)]
mod screen_explain_tests {
    use super::*;

    /// 离线 explain 的价值全在「与线上同一套判定」：结论若与在线不同，拿它复现出的
    /// 结果就是误导。两个入口共用 explain_snapshot，这里钉住行为——含**标题参与判定**
    /// （claude 的 spinner/✳ 全在标题上，漏传会得出不同结论）。
    #[test]
    fn offline_explain_matches_the_live_rules() {
        // 审批 UI：与在线路径同样判 blocked，并给出命中的规则名。
        let blocked = screen_detect_explain_text(
            "claude".into(),
            " Bash command\n Do you want to proceed?\n \u{276F} 1. Yes\n   2. No\n".into(),
            None,
        );
        assert_eq!(blocked.raw_state, Some("blocked"));
        assert_eq!(blocked.matched_rule, "bash_permission_prompt");
        // 离线快照没有运行时的已发布状态。
        assert_eq!(blocked.published, None);

        // 同一屏幕，只是标题带了 spinner 帧 → working（标题规则优先级最高）。
        let working = screen_detect_explain_text(
            "claude".into(),
            " Do you want to proceed?\n".into(),
            Some("\u{280B} building".into()),
        );
        assert_eq!(working.raw_state, Some("working"));
        assert_eq!(working.matched_rule, "osc_title_working");

        // 无规则集的 provider 如实说明，而不是假装判了个 idle。用未注册的身份串——
        // 已注册的五家插件现在都声明了规则（这条断言本身被 gemini 加规则时抓到过）。
        let unsupported = screen_detect_explain_text("not-an-agent".into(), "whatever".into(), None);
        assert_eq!(unsupported.raw_state, None);
        assert_eq!(unsupported.matched_rule, "provider_unsupported");
    }
}

// snapshot/write/resize/stop 一律 async + spawn_blocking：同步命令跑在主线程，而这几条
// 都要抢 PTY 状态锁、拷 backlog（最多 1MiB）或触碰 ConPTY——任何一次卡顿都会冻住消息泵。
#[tauri::command]
pub(crate) async fn managed_terminal_snapshot(
    state: State<'_, super::AppState>,
    session_id: i64,
    since: Option<u64>,
) -> Result<super::pty::PtySnapshot, String> {
    let ptys = state.ptys.clone();
    let bg = state.bg_ptys.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let since = since.unwrap_or(0);
        // 托管 PTY 在跑时它是唯一真相，绝不让 bg 旁路遮蔽：resume 后若还按旁路的定格
        // 画面作答，它的大 endOffset 会把新 PTY 从 0 起的输出全变成「已写过」，前端
        // 整段丢弃——恢复会话后终端打字无回显就是这么来的。resume 成功时旁路条目
        // 已被 detach 摘除，这里的判定是并发窗口与残留条目的双保险。
        if ptys.is_managed(session_id) {
            return ptys.snapshot(session_id, since);
        }
        // 托管 PTY 不在：后台会话（claude FleetView）的画面走旁路 socket。它优先：
        // 接上了就说明用户正在这个窗口看它，托管表里那份必然是空壳。
        bg.snapshot(session_id, since)
            .unwrap_or_else(|| ptys.snapshot(session_id, since))
    })
    .await
    .map_err(|e| e.to_string())
}

/// 接上一个后台会话的画面。它的 PTY 在 claude 自己的守护进程手里，meowo 只能旁路连上去看
/// （读 + 改尺寸 + 结束），不能像托管会话那样重启或写入——键盘输入另有一道 attach 流程未通。
#[tauri::command]
pub(crate) async fn attach_background_session(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Result<(), String> {
    let db_path = state.db_path.clone();
    let bg = state.bg_ptys.clone();
    tauri::async_runtime::spawn_blocking(move || attach_background(&db_path, &bg, session_id))
        .await
        .map_err(|e| e.to_string())?
}

/// 向一个后台会话发消息。走 Agent 守护进程的控制通道，不经过 PTY——那条路对后台 worker
/// 无效（它不消费 stdin）。发出后 agent 就开始干活，回复照常落进 transcript，对话页照常显示。
#[tauri::command]
pub(crate) async fn send_background_prompt(
    state: State<'_, super::AppState>,
    session_id: i64,
    text: String,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("消息为空".into());
    }
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let control = background_endpoint(&db_path, session_id)?
            .control
            .ok_or("找不到 Agent 的控制通道，无法向后台会话发消息")?;
        super::bgpty::send_prompt(&control, &text)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 查花名册拿这个会话的接入点。阻塞（读库 + 读文件），只在 spawn_blocking 里调。
fn background_endpoint(
    db_path: &std::path::Path,
    session_id: i64,
) -> Result<meowo_agent::BackgroundEndpoint, String> {
    let store = super::open_store(db_path)?;
    let session = store.get_session(session_id).map_err(|e| e.to_string())?;
    let provider = store
        .session_provider(session_id)
        .map_err(|e| e.to_string())?;
    meowo_agent::resolve(Some(&provider))
        .and_then(|agent| agent.runtime())
        .and_then(|runtime| runtime.background_endpoint(&session.cc_session_id))
        .ok_or_else(|| "后台会话已结束或被 Agent 收回".to_string())
}

/// 查花名册 → 连上去。阻塞（读文件 + 连 socket），只在 spawn_blocking 里调。
fn attach_background(
    db_path: &std::path::Path,
    bg: &super::bgpty::BgPtyRegistry,
    session_id: i64,
) -> Result<(), String> {
    bg.attach(session_id, &background_endpoint(db_path, session_id)?)
}

#[tauri::command]
pub(crate) fn managed_terminal_binding(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Option<i64> {
    state.ptys.binding(session_id)
}

#[tauri::command]
pub(crate) async fn write_managed_terminal(
    state: State<'_, super::AppState>,
    session_id: i64,
    data: String,
) -> Result<(), String> {
    let ptys = state.ptys.clone();
    let bg = state.bg_ptys.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 后台会话不消费 stdin，往它的 PTY 写按键石沉大海。与其静默吞掉（用户会以为
        // 打进去了），不如明说那条路在哪——送话走对话页，见 send_background_prompt。
        if !ptys.is_managed(session_id) && bg.is_active(session_id) {
            return Err("后台会话不接受终端按键，请在对话页发送消息".to_string());
        }
        ptys.write(session_id, data.as_bytes())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 取 PTY 当前生效的网格尺寸，`[cols, rows]`；未知（会话不在/后台旁路/尚未设过）为 `[0, 0]`。
///
/// 前端可见期每隔几秒查一次，与本地 fit 出的网格比对：不等就说明某次 resize 没落地
/// （撞上 PTY 那把有界锁、会话正在重启），补发一次把 PTY 拉齐。刻意不复用快照——
/// 那个要把整个 backlog（可达 1 MiB）编码重传一遍，不能拿来轮询。
#[tauri::command]
pub(crate) async fn managed_terminal_grid(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Result<[u16; 2], String> {
    let ptys = state.ptys.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (cols, rows) = ptys.grid(session_id);
        Ok([cols, rows])
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn resize_managed_terminal(
    state: State<'_, super::AppState>,
    session_id: i64,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let ptys = state.ptys.clone();
    let bg = state.bg_ptys.clone();
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if ptys.is_managed(session_id) {
            return ptys.resize(session_id, cols, rows);
        }
        if bg.is_active(session_id) {
            return bg.resize(session_id, cols, rows);
        }
        // 先按普通会话办，失败了再考虑后台会话那条路。顺序不能反：resize 在拖窗口时每 80ms
        // 就来一次，而 attach_background 要开一次 SQLite 再读一遍花名册——对着一屏普通会话
        // 反复付这个代价太亏。失败了才试后台，代价只落在真正需要它的那一次上。
        let Err(direct) = ptys.resize(session_id, cols, rows) else {
            return Ok(());
        };
        // 后台会话的画面可能还没接上：对话页发起 attach 与终端视图的首次 resize 在赛跑，
        // 输了就会报「PTY 会话未运行」——画面明明已经在显示了。
        // 接不上就把**普通会话那条错**还回去：对一个从来不是后台会话的 session 说
        //「已经不在 Agent 的花名册里了」，等于拿一个它没用过的功能来解释失败。
        if attach_background(&db_path, &bg, session_id).is_err() {
            return Err(direct);
        }
        bg.resize(session_id, cols, rows)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn stop_managed_terminal(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Result<(), String> {
    let ptys = state.ptys.clone();
    let bg = state.bg_ptys.clone();
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 托管 PTY 优先：那是我们自己 spawn 的进程，stop 一步到位。
        if ptys.is_managed(session_id) {
            return ptys.stop(session_id);
        }
        // 后台会话要走它自己的 kill 控制帧：对着 pid 下手会被 supervisor 按 respawnFlags
        // 原地拉回来，用户看到的是「点了结束，它又活了」。没接上就先接——用户从卡片菜单
        // 直接结束时，这个窗口可能从没打开过它的画面。
        //
        // 接不上就把**普通会话那条错**还回去。托管 PTY 恰好在这一刻退出是常事（结束按钮
        // 按历史轮询的 pty_managed 亮灭），那时该说「PTY 会话未运行」，而不是拿
        //「已经不在 Agent 的花名册里了」去解释一个用户根本没碰过的功能。
        if !bg.is_active(session_id) && attach_background(&db_path, &bg, session_id).is_err() {
            return ptys.stop(session_id);
        }
        bg.kill(session_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 该会话待处理的审批 + AskUserQuestion 题面，一次取回（合并自两条独立轮询）。
/// push 事件是主路径；这条轮询兜底「事件在对话窗冷启动时打进虚空」（emit 不排队）。
/// 出口走 DTO 而非原始 ApprovalRequest：后者空 suggestions 会被 skip 掉字段，
/// 与 ts-rs 生成的前端类型（字段恒在）不符。缘由见 pty.rs 的 emit_approval。
/// 纯内存读（两次 map 查询），同步命令合规。
#[tauri::command]
pub(crate) fn pending_interaction(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> meowo_protocol::ipc::PendingInteractionDto {
    meowo_protocol::ipc::PendingInteractionDto {
        approval: state.ptys.pending_approval(session_id).map(Into::into),
        // 题面走带 answerable 的出口：broker 是否仍持有请求（可代答）由它动态计算。
        question: state.ptys.interactive_question_dto(session_id),
    }
}

/// 前端收卡时撤下题面，避免轮询把已答过的题重新弹出来。
#[tauri::command]
pub(crate) fn dismiss_interactive_question(state: State<'_, super::AppState>, session_id: i64) {
    state.ptys.clear_interactive_question(session_id);
}

/// 全部会话里「正在等用户」的清单（审批 + 同步题面），供远程端徽标轮询——push 事件
/// 到不了浏览器，非当前会话的审批只能靠周期扫描点亮侧栏徽标。数据源取 broker 实时
/// 事实（DB 的 pending_review 会在工具放行后滞留，不可用）。纯内存读，同步命令合规。
#[tauri::command]
pub(crate) fn awaiting_interaction_sessions(state: State<'_, super::AppState>) -> Vec<i64> {
    let mut ids = state.ptys.approval_session_ids();
    ids.extend(state.ptys.interactive_question_session_ids());
    let mut out: Vec<i64> = ids.into_iter().collect();
    out.sort_unstable();
    out
}

/// chat 窗终端视图声明「正在看」哪个会话——emitter 只对它推送 pty-output 实时帧
/// （见 PtyBroker::viewed_session）。纯原子写，同步命令合规。
#[tauri::command]
pub(crate) fn register_terminal_viewer(state: State<'_, super::AppState>, session_id: i64) {
    state.ptys.set_viewer(session_id);
}

/// 注销「正在看」（CAS，只清自己的注册，见 PtyBroker::clear_viewer）。
#[tauri::command]
pub(crate) fn unregister_terminal_viewer(state: State<'_, super::AppState>, session_id: i64) {
    state.ptys.clear_viewer(session_id);
}

#[tauri::command]
pub(crate) fn register_approval_consumer(
    state: State<'_, super::AppState>,
    session_id: i64,
    consumer_id: String,
) -> Result<(), String> {
    state
        .ptys
        .register_approval_consumer(session_id, consumer_id)
}

#[tauri::command]
pub(crate) fn unregister_approval_consumer(state: State<'_, super::AppState>, consumer_id: String) {
    state.ptys.unregister_approval_consumer(&consumer_id);
}

#[tauri::command]
pub(crate) async fn resolve_pending_approval(
    state: State<'_, super::AppState>,
    session_id: i64,
    request_id: String,
    choice: String,
) -> Result<(), String> {
    let ptys = state.ptys.clone();
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        ptys.resolve_approval_choice(session_id, &request_id, &choice)?;
        // reporter 收到决策后也会清 pending_review，但 codex 的 hook 可能继承只读沙箱、
        // 清不掉——标记会一直挂到下一个 hook 事件才被顺带清理。app 进程写库没有这种
        // 限制，这里当场兜底清掉（best-effort：清不掉也不影响已送达的决策）。
        // 「去终端作答」（pass）例外：用户还没答，提问仍悬着（表单即将转到终端），
        // 标记必须留着。
        if choice != "pass" {
            if let Ok(store) = super::open_store(&db_path) {
                let _ = store.clear_pending_review(session_id, super::now_ms());
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn open_attached_terminal(
    state: State<'_, super::AppState>,
    session_id: i64,
) -> Result<(), String> {
    let broker = state.ptys.clone();
    tauri::async_runtime::spawn_blocking(move || {
        super::terminal::attach_in_external_terminal(&broker, session_id)
    })
    .await
    .map_err(|e| e.to_string())?
}
