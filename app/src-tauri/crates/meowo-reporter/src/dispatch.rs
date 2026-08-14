use crate::hook::HookEvent;
use meowo_store::{PendingReview, SessionStatus, Store, StoreError};

use std::path::Path;

/// 把一个 hook 事件落到库。未知/缺字段一律降级为「无操作」，绝不报错冒泡。
///
/// `provider` 是**原始字符串**（可能是本版本尚不认识的 id）：SessionStart 时原样写进
/// `sessions.provider`（未知值也保留，绝不冒名成默认 agent）；需要能力（stop 正文、context、
/// transcript 标题、tab token）时才对已注册插件 `by_id` 查询，查不到就整段降级为无操作。
pub fn dispatch(
    store: &Store,
    ev: &HookEvent,
    now_ms: i64,
    provider: &str,
) -> Result<(), StoreError> {
    // 事件名先由该 agent 译成规范名。只有 gemini 真的要译（它把「用户提交」「回合结束」叫成
    // `BeforeAgent` / `AfterAgent`），其余四家原样透传；未注册的 provider 同样透传，其未知事件
    // 照旧落到末尾的 `_ => {}`。翻译表归插件所有——这里刻意不出现任何 agent 的名字。
    let event = meowo_agent::by_id(provider).map_or(ev.hook_event_name.as_str(), |p| {
        p.canonical_event(&ev.hook_event_name)
    });
    match event {
        "SessionStart" => {
            let Some(cwd) = ev.cwd.as_deref() else {
                return Ok(());
            };
            if ev.session_id.is_empty() {
                return Ok(());
            }
            let sid = create_session(store, ev, cwd, provider, now_ms)?;
            // resume 一个已结束会话时，SessionStart 也要复活它（置 running、清 ended_at），否则卡片停在
            // 断开态直到用户发首条消息才重连。
            store.revive_if_ended(sid, now_ms)?;
            apply_title(store, ev, sid, now_ms, provider)?;
            write_tab_token(store, sid, ev, provider);
        }
        "UserPromptSubmit" => {
            if let Some(sid) = lookup_or_create(store, ev, provider, now_ms)? {
                store.clear_pending_review(sid, now_ms)?;
                // 新回合开始:上一回合的活动名已是过时信息,清掉(方法内不 touch,下面的
                // on_user_prompt / touch_session 才是本事件的状态翻转)。
                store.clear_current_activity(sid, now_ms)?;
                if let Some(prompt) = ev.prompt_text() {
                    // on_user_prompt 内部会 touch；避免文本消息对 sessions 做两次相同 UPDATE。
                    store.on_user_prompt(sid, &prompt, now_ms)?;
                    store.set_last_user_text(sid, &prompt, now_ms)?;
                } else {
                    // 纯图片内容块同样代表用户开启了新回合，也必须从 waiting/stale 转回 running。
                    store.touch_session(sid, now_ms)?;
                }
                // 给已注册（含压缩漏掉 SessionStart）的会话补抓 PID；每用户回合一次，开销可忽略。
                if let Some(p) = crate::proc::owner_pid(provider) {
                    store.set_session_pid(sid, p as i64, now_ms)?;
                }
                apply_title(store, ev, sid, now_ms, provider)?;
                write_tab_token(store, sid, ev, provider);
            }
        }
        "PostToolUse" => {
            if let Some(sid) = lookup_or_create(store, ev, provider, now_ms)? {
                store.clear_pending_review(sid, now_ms)?;
                // 待办工具名由插件声明：kimi 叫 `TodoList`，claude 旧版叫 `TodoWrite`。
                // 此前这里写死 `"TodoWrite"`，两家现版本都对不上，待办表一直是空的。
                let todo_tool = ev.tool_name.as_deref().is_some_and(|name| {
                    meowo_agent::by_id(provider)
                        .is_some_and(|plugin| plugin.todo_snapshot_tools().contains(&name))
                });
                // claude 现版本的增量任务工具（TaskCreate/TaskUpdate）：逐条累积而非覆盖写。
                let todo_delta_tool = ev.tool_name.as_deref().is_some_and(|name| {
                    meowo_agent::by_id(provider)
                        .is_some_and(|plugin| plugin.todo_delta_tools().contains(&name))
                });
                match ev.tool_name.as_deref() {
                    _ if todo_tool => {
                        store.sync_todos(sid, &ev.todo_items(), now_ms)?;
                    }
                    _ if todo_delta_tool => {
                        // 解析不出（结果文本改版等）降级为 touch——任务工具是元操作，
                        // 不该像普通工具那样把名字写进 current_activity。
                        match ev.todo_delta() {
                            Some(delta) => store.apply_todo_delta(sid, &delta, now_ms)?,
                            None => store.touch_session(sid, now_ms)?,
                        }
                    }
                    Some("Bash") => {
                        if let Some(cmd) = ev.bash_command() {
                            store.set_current_activity(sid, &format!("› {cmd}"), now_ms)?;
                        }
                    }
                    _ => {
                        // 非 Bash 工具也写活动名:此前只有 Bash 写 current_activity,Edit/Read
                        // 等长跑期间前端一直显示上一条早已完成的 Bash 命令,像卡死在某步。
                        // set_current_activity 内部自带 touch_session,与原兜底分支等价。
                        match ev
                            .tool_name
                            .as_deref()
                            .filter(|name| !name.trim().is_empty())
                        {
                            Some(name) => store.set_current_activity(sid, name, now_ms)?,
                            None => store.touch_session(sid, now_ms)?,
                        }
                    }
                }
                if let Some(c) = read_context(provider, ev) {
                    // model:usage 通道顺带读到就写(kimi),None 时 COALESCE 保留旧值——
                    // 不带的话 kimi 的模型要等第一次 Stop 才落库,新会话第一回合一直是空的。
                    store.set_session_context(
                        &ev.session_id,
                        Some(c.used_pct),
                        Some(c.window),
                        c.model.as_deref(),
                        now_ms,
                    )?;
                }
            }
        }
        "Stop" => {
            if let Some(sid) = lookup_or_create(store, ev, provider, now_ms)? {
                store.clear_pending_review(sid, now_ms)?;
                store.set_session_status(sid, SessionStatus::Waiting, now_ms)?;
                // 最近 AI 正文 + 模型由 agent 决定来源：claude 用 Stop hook 携带的正文（模型走 statusline）；
                // kimi 的 Stop hook 不带，读会话 wire.jsonl 一次出正文 + 模型。
                let out = telemetry(provider)
                    .map(|t| t.stop_outputs(&ev.agent_ctx()))
                    .unwrap_or_default();
                if let Some(msg) = out.last_ai {
                    store.set_last_ai_text(sid, &msg, now_ms)?;
                }
                // stop_outputs 的模型(config.update 通道,/model 切换即时反映)比 usage 通道
                // (最后一条 usage.record,切换后仍是旧回合的模型)更新。写过它之后,下面的
                // read_context 就不许再带 model——否则后写的旧值把刚切换的模型顶回去。
                let stop_model_written = out.model.is_some();
                if let Some(model) = out.model {
                    store.set_session_context(&ev.session_id, None, None, Some(&model), now_ms)?;
                }
                apply_title(store, ev, sid, now_ms, provider)?;
                write_tab_token(store, sid, ev, provider);
                if let Some(c) = read_context(provider, ev) {
                    // model:usage 通道顺带读到就写(kimi),None 时 COALESCE 保留旧值——
                    // 不带的话 kimi 的模型要等第一次 Stop 才落库,新会话第一回合一直是空的。
                    store.set_session_context(
                        &ev.session_id,
                        Some(c.used_pct),
                        Some(c.window),
                        c.model.as_deref().filter(|_| !stop_model_written),
                        now_ms,
                    )?;
                }
            }
        }
        "SessionEnd" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.clear_pending_review(sid, now_ms)?;
                store.end_session(sid, now_ms)?;
            }
        }
        "PermissionRequest" => {
            if let Some(sid) = lookup_session(store, ev)? {
                let kind = match ev.tool_name.as_deref() {
                    Some("ExitPlanMode") => PendingReview::Plan,
                    Some("AskUserQuestion") => PendingReview::Question,
                    _ => PendingReview::Approval,
                };
                store.set_pending_review(sid, kind, now_ms)?;
            }
        }
        "PreToolUse" => {
            if let Some(sid) = lookup_session(store, ev)? {
                let kind = match ev.tool_name.as_deref() {
                    Some("AskUserQuestion") => Some(PendingReview::Question),
                    Some("ExitPlanMode") => Some(PendingReview::Plan),
                    _ => None, // 安装侧已用 matcher 限定;这里再兜一层防御
                };
                if let Some(kind) = kind {
                    store.set_pending_review(sid, kind, now_ms)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// 本次 hook 所属的账号（profile）。由 meowo-app 拉起 agent 时注入，reporter 作为 hook 子进程继承。
///
/// 缺席 = 默认账号（用户自己在终端里跑的 agent，或压根没建过 profile）——那正是我们想记的 NULL。
fn profile_from_env() -> Option<String> {
    std::env::var("MEOWO_PROFILE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 该 agent 的遥测能力（未注册 / 无此能力 → None，调用方整段跳过）。
fn telemetry(provider: &str) -> Option<&'static dyn meowo_agent::TelemetryCap> {
    meowo_agent::by_id(provider)?.telemetry()
}

/// 从会话日志读上下文占用。claude 无此能力（走 statusline），返回 None。
fn read_context(provider: &str, ev: &HookEvent) -> Option<meowo_agent::ContextUsage> {
    telemetry(provider)?.read_context(&ev.agent_ctx())
}

fn apply_title(
    store: &Store,
    ev: &HookEvent,
    sid: i64,
    now_ms: i64,
    provider: &str,
) -> Result<(), StoreError> {
    // 是否由 transcript 解析标题由 agent 决定（claude 是；kimi/codex 否，靠首条 prompt 命名）。
    let Some(agent) = telemetry(provider) else {
        return Ok(());
    };
    if !agent.resolves_transcript_title() {
        return Ok(());
    }
    // 提供解析器的 transcript 规格（claude=ClaudeTranscript；无则不解析）。
    let Some(spec) = agent.transcript() else {
        return Ok(());
    };
    // cwd 优先用事件携带的，否则回退到 SessionStart 时存进库的 cwd。
    let cwd_owned: Option<String> = match ev.cwd.clone() {
        Some(c) => Some(c),
        None => store.session_cwd(sid).ok().flatten(),
    };
    if let Some(title) = spec.resolve_title(
        ev.transcript_path.as_deref(),
        cwd_owned.as_deref(),
        &ev.session_id,
    ) {
        store.set_session_title(sid, &title, now_ms)?;
    }
    Ok(())
}

/// 仅当该 provider 需由 meowo-reporter 补 token 时，把 `<cwd 末段目录名> ·<sid8>` 写进本标签的 WT
/// 标题——sid8=session_id 末 8 位、全局唯一，meowo-app 据此精确切到该标签（解决同窗口同目录两会话标签
/// 同名分不清）。meowo-reporter 是 hook 子进程、继承本会话的 ConPTY，写 CONOUT$ 只影响自己这个标签。
/// 非 Windows / 非 WT(CONOUT$ 打不开) 静默 no-op。
fn write_tab_token(store: &Store, sid: i64, ev: &HookEvent, provider: &str) {
    if !meowo_agent::by_id(provider).is_some_and(|p| p.writes_tab_token()) {
        return;
    }
    let sid8 = crate::tabtitle::short_sid(&ev.session_id);
    if sid8.is_empty() {
        return;
    }
    // 可见前缀优先用任务标题(贴合卡片，如 "hi")，无/占位则回退 cwd 末段目录名。
    let base = store
        .session_title(sid)
        .ok()
        .flatten()
        .filter(|t| t != "(未命名会话)")
        .or_else(|| {
            ev.cwd
                .as_deref()
                .map(|c| c.trim_end_matches(['/', '\\']))
                .and_then(|c| Path::new(c).file_name().and_then(|s| s.to_str()))
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "session".to_string());
    crate::tabtitle::set_tab_title(&format!("{base} ·{sid8}"));
}

/// 建会话（项目 upsert + 会话 + provider + cwd + 抓 PID），返回 sid。SessionStart 与懒创建共用。
fn create_session(
    store: &Store,
    ev: &HookEvent,
    cwd: &str,
    provider: &str,
    now_ms: i64,
) -> Result<i64, StoreError> {
    let (root, name) = project_root_and_name(cwd);
    let pid = store.upsert_project_by_root(&root, &name, now_ms)?;
    let (sid, _) = store.start_session(pid, &ev.session_id, now_ms)?;
    // DB 把 NULL/缺省的 provider 列视作默认 agent，故默认 agent 不必写库；其余（含本版本尚不
    // 认识的未知 id）一律原样写入——绝不因「查不到插件」就把它落成 NULL/默认。
    if provider != meowo_agent::DEFAULT_ID.as_str() {
        store.set_session_provider(sid, provider)?;
    }
    // 多账号：这个会话跑在哪个账号上。meowo 拉起 agent 时注入 `MEOWO_PROFILE`，而 reporter 是
    // agent 的 hook 子进程，于是继承得到它。用户自己在终端敲 agent（不经 meowo）时没有这个变量
    // → 记成默认账号，正确。恢复会话时据此回到同一个账号。
    // best-effort：profile 写失败绝不能中断建会话。曾因迁移漏 bump 版本、老库缺 profile 列，
    // 这里一个 `?` 让 profile 会话的 SessionStart 半途而废——排在后面的 cwd/pid 全没写上，
    // 会话从此识别不到工作区、还会在 120s 无事件后被误收尾成掉线。
    if let Some(p) = profile_from_env() {
        if let Err(e) = store.set_session_profile(sid, Some(&p)) {
            eprintln!("meowo-reporter: 记录会话 profile 失败（继续建会话）: {e}");
        }
    }
    store.set_session_cwd(sid, cwd, now_ms)?;
    if let Some(p) = crate::proc::owner_pid(provider) {
        store.set_session_pid(sid, p as i64, now_ms)?;
    }
    Ok(sid)
}

fn lookup_session(store: &Store, ev: &HookEvent) -> Result<Option<i64>, StoreError> {
    if ev.session_id.is_empty() {
        return Ok(None);
    }
    store.find_session_id_pub(&ev.session_id)
}

/// 查会话；查不到且事件带 cwd 时就地懒创建——让「hooks 中途装上 / SessionStart 漏掉（压缩等）」
/// 的会话在下一条带 cwd 的活动事件（UserPromptSubmit/PostToolUse/Stop）上也能补建上板，不必重开。
fn lookup_or_create(
    store: &Store,
    ev: &HookEvent,
    provider: &str,
    now_ms: i64,
) -> Result<Option<i64>, StoreError> {
    if ev.session_id.is_empty() {
        return Ok(None);
    }
    if let Some(sid) = store.find_session_id_pub(&ev.session_id)? {
        // 会话曾被误清成 ended（如 kimi 的 pid 一度不被 app 认作存活而被 reap），但仍有活动事件到来
        // → 统一自愈复活（清 ended_at、置 running），不再只在 UserPromptSubmit 一条路径上修。
        store.revive_if_ended(sid, now_ms)?;
        // cwd 只有建会话时写一次；SessionStart 落库中断过的半态会话（cwd 恒 NULL、识别不到
        // 工作区）靠后续任一带 cwd 的事件在此自愈。已有值不覆盖。
        if let Some(cwd) = ev.cwd.as_deref() {
            store.backfill_session_cwd(sid, cwd)?;
        }
        return Ok(Some(sid));
    }
    match ev.cwd.as_deref() {
        Some(cwd) => Ok(Some(create_session(store, ev, cwd, provider, now_ms)?)),
        None => Ok(None),
    }
}

/// cwd 的 git 根（向上找 .git）作为项目 root；无 git 则用 cwd 本身。
/// name 优先取 git remote origin 的 owner/repo，否则用末段目录名。
pub(crate) fn project_root_and_name(cwd: &str) -> (String, String) {
    let root = find_git_root(cwd).unwrap_or_else(|| cwd.to_string());
    let name = owner_repo_from_git(&root).unwrap_or_else(|| {
        Path::new(&root)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&root)
            .to_string()
    });
    (root, name)
}

fn find_git_root(start: &str) -> Option<String> {
    let mut dir = Path::new(start);
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_string_lossy().to_string());
        }
        dir = dir.parent()?;
    }
}

/// 从 URL 提取 owner/repo：去掉结尾 .git，把 ':' 当 '/'，按 '/' 切分去空，取最后两段。
/// 支持 https://github.com/o/r.git 与 git@github.com:o/r.git。
fn owner_repo_from_url(url: &str) -> Option<String> {
    let normalized = url.trim().trim_end_matches(".git").replace(':', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        let n = parts.len();
        Some(format!("{}/{}", parts[n - 2], parts[n - 1]))
    } else {
        None
    }
}

/// 读 <root>/.git/config，取 [remote "origin"] 的 url，解析 owner/repo。
fn owner_repo_from_git(root: &str) -> Option<String> {
    let cfg = Path::new(root).join(".git").join("config");
    let content = std::fs::read_to_string(cfg).ok()?;
    let mut in_origin = false;
    for line in content.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_origin = l == r#"[remote "origin"]"#;
            continue;
        }
        if in_origin {
            if let Some(rest) = l.strip_prefix("url") {
                if let Some(eq) = rest.find('=') {
                    let url = rest[eq + 1..].trim();
                    return owner_repo_from_url(url);
                }
            }
        }
    }
    None
}

/// 供测试调用的公开包装。
#[doc(hidden)]
pub fn owner_repo_from_url_pub(url: &str) -> Option<String> {
    owner_repo_from_url(url)
}
