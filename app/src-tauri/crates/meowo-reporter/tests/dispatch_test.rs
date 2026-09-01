use meowo_reporter::dispatch::owner_repo_from_url_pub;
use meowo_reporter::hook::HookEvent;
use std::io::Write as _;

#[test]
fn owner_repo_parsing() {
    assert_eq!(
        owner_repo_from_url_pub("https://github.com/larrygogo/autopilot.git").as_deref(),
        Some("larrygogo/autopilot")
    );
    assert_eq!(
        owner_repo_from_url_pub("git@github.com:larrygogo/autopilot.git").as_deref(),
        Some("larrygogo/autopilot")
    );
    assert_eq!(
        owner_repo_from_url_pub("https://gitlab.com/grp/sub/repo.git").as_deref(),
        Some("sub/repo")
    );
}

#[test]
fn parse_user_prompt_event() {
    let json = r#"{
        "hook_event_name": "UserPromptSubmit",
        "session_id": "abc",
        "cwd": "/home/me/proj",
        "prompt": "写个登录"
    }"#;
    let ev = HookEvent::parse(json).expect("parse");
    assert_eq!(ev.session_id, "abc");
    assert_eq!(ev.cwd.as_deref(), Some("/home/me/proj"));
    assert_eq!(ev.hook_event_name, "UserPromptSubmit");
    assert_eq!(ev.prompt_text().as_deref(), Some("写个登录"));
}

#[test]
fn parse_kimi_user_prompt_array_form() {
    // kimi-code 的 prompt 是内容块数组（非字符串）；旧的 Option<String> 会解析失败，现应规整成文本。
    let json = r#"{
        "hook_event_name": "UserPromptSubmit",
        "session_id": "k",
        "cwd": "/p",
        "prompt": [{"type":"text","text":"实现"},{"type":"text","text":"登录"}]
    }"#;
    let ev = HookEvent::parse(json).expect("kimi 数组形式应能解析");
    assert_eq!(ev.prompt_text().as_deref(), Some("实现登录"));
}

#[test]
fn parse_posttooluse_todowrite() {
    let json = r#"{
        "hook_event_name": "PostToolUse",
        "session_id": "abc",
        "cwd": "/p",
        "tool_name": "TodoWrite",
        "tool_input": { "todos": [
            {"content":"a","status":"completed"},
            {"content":"b","status":"in_progress"}
        ]}
    }"#;
    let ev = HookEvent::parse(json).expect("parse");
    assert_eq!(ev.tool_name.as_deref(), Some("TodoWrite"));
    let todos = ev.todo_items();
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[1].content, "b");
}

/// tool_response 形状不统一：裸字符串之外还有内容块数组 / 包 content 一层的对象。
/// 三种都得能抠出编号——漏一种就是那批用户的任务静默不显示。
#[test]
fn todo_delta_reads_task_number_from_block_shaped_responses() {
    for resp in [
        r#""Task #7 created successfully: x""#,
        r#"[{"type":"text","text":"Task #7 created successfully: x"}]"#,
        r#"{"content":[{"type":"text","text":"Task #7 created successfully: x"}]}"#,
    ] {
        let json = format!(
            r#"{{"hook_event_name":"PostToolUse","session_id":"s","tool_name":"TaskCreate",
                "tool_input":{{"subject":"x"}},"tool_response":{resp}}}"#
        );
        let ev = HookEvent::parse(&json).expect("parse");
        match ev.todo_delta() {
            Some(meowo_store::TodoDelta::Create { external_id, content }) => {
                assert_eq!(external_id, "7", "resp={resp}");
                assert_eq!(content, "x");
            }
            other => panic!("应解析出 Create，得到 {other:?}（resp={resp}）"),
        }
    }
}

#[test]
fn parse_tolerates_unknown_fields() {
    let json = r#"{"hook_event_name":"Stop","session_id":"z","extra":123}"#;
    let ev = HookEvent::parse(json).expect("parse");
    assert_eq!(ev.hook_event_name, "Stop");
}

use meowo_reporter::dispatch::dispatch;
use meowo_store::Store;

fn ev(json: &str) -> HookEvent {
    HookEvent::parse(json).unwrap()
}

/// 测试默认走 claude provider；provider 行为单独在 kimi_session_tagged_with_provider 覆盖。
fn disp(store: &Store, ev: &HookEvent, now_ms: i64) -> Result<(), meowo_store::StoreError> {
    dispatch(store, ev, now_ms, meowo_agent::id::CLAUDE.as_str())
}

fn write_transcript(name: &str, body: &[u8]) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body).unwrap();
    p
}

#[test]
fn session_start_then_prompt_then_todos_flow() {
    let store = Store::open_in_memory().unwrap();

    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/home/me/proj"}"#),
        100,
    )
    .unwrap();
    let projects = store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "proj");

    disp(
        &store,
        &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"实现登录"}"#),
        200,
    )
    .unwrap();
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"a","status":"in_progress"}]}}"#), 300).unwrap();

    let sid = store.find_session_id_pub("s1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "实现登录");
    assert_eq!(t.column, "doing");
    assert_eq!(store.list_todos(tid).unwrap().len(), 1);
}

/// 待办工具名由插件声明，**不是**写死的 `TodoWrite`：kimi 叫 `TodoList`，字段也用
/// `title` 而非 `content`。此前 dispatch 里写死一个名字，两家现版本都对不上——
/// 待办表一直是空的，界面自然什么都显示不出来。
#[test]
fn kimi_todo_list_snapshot_lands_in_the_store() {
    let store = Store::open_in_memory().unwrap();
    let kimi = |json: &str| dispatch(&store, &ev(json), 100, meowo_agent::id::KIMI.as_str());
    kimi(r#"{"hook_event_name":"SessionStart","session_id":"k1","cwd":"/home/me/proj"}"#).unwrap();
    kimi(
        // status 用 kimi 真实写的 `done`（不是 claude 的 `completed`）——端到端覆盖
        // 状态别名映射，否则已完成项会静默降级成 pending，界面上一条都勾不上。
        r#"{"hook_event_name":"PostToolUse","session_id":"k1","tool_name":"TodoList","tool_input":{"todos":[
            {"title":"贴纸看板 UX 修复","status":"done"},
            {"title":"聊天窗口 UX 修复","status":"in_progress"}
        ]}}"#,
    )
    .unwrap();

    let sid = store.find_session_id_pub("k1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(
        todos.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(),
        vec!["贴纸看板 UX 修复", "聊天窗口 UX 修复"]
    );
    assert_eq!(todos[0].status.as_str(), "completed");
    assert_eq!(todos[1].status.as_str(), "in_progress");

    // 同名工具在别的 provider 上不该被当成待办（工具名归各插件声明）。
    let claude = |json: &str| dispatch(&store, &ev(json), 200, meowo_agent::id::CLAUDE.as_str());
    claude(r#"{"hook_event_name":"SessionStart","session_id":"c1","cwd":"/home/me/proj"}"#)
        .unwrap();
    claude(
        r#"{"hook_event_name":"PostToolUse","session_id":"c1","tool_name":"TodoList","tool_input":{"todos":[{"title":"x","status":"pending"}]}}"#,
    )
    .unwrap();
    let csid = store.find_session_id_pub("c1").unwrap().unwrap();
    let ctid = store.task_id_of_session_pub(csid).unwrap();
    assert!(store.list_todos(ctid).unwrap().is_empty());
}

/// claude 现版本的任务是增量式（TaskCreate 单条建、TaskUpdate 按编号改），不是 TodoWrite
/// 快照——编号只出现在**结果文本**里，必须从 tool_response 抠出来逐条累积。此前没有这条
/// 路径，新版 claude 会话的待办表恒空，对话窗与标题栏都无从显示任务进度。
#[test]
fn claude_task_delta_accumulates_into_todos() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/home/me/proj"}"#),
        100,
    )
    .unwrap();
    // 两条 TaskCreate：编号在结果文本里（真实 transcript 取证的格式）。
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TaskCreate",
        "tool_input":{"subject":"生成基线迁移","description":"…"},
        "tool_response":"Task #1 created successfully: 生成基线迁移"}"#), 200).unwrap();
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TaskCreate",
        "tool_input":{"subject":"删除旧迁移"},
        "tool_response":"Task #2 created successfully: 删除旧迁移"}"#), 300).unwrap();
    // TaskUpdate：taskId 数字/字符串两种形态都得认。
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TaskUpdate",
        "tool_input":{"taskId":"1","status":"in_progress"}}"#), 400).unwrap();

    let sid = store.find_session_id_pub("s1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(
        todos.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(),
        vec!["生成基线迁移", "删除旧迁移"]
    );
    assert_eq!(todos[0].status.as_str(), "in_progress");
    assert_eq!(todos[1].status.as_str(), "pending");
    // 有 in_progress → 看板列推导为 doing（与快照路径同口径）。
    assert_eq!(store.get_task(tid).unwrap().column, "doing");

    // 完成 + 删除：completed 落状态，deleted 是删行而不是一种状态。
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TaskUpdate",
        "tool_input":{"taskId":1,"status":"completed"}}"#), 500).unwrap();
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TaskUpdate",
        "tool_input":{"taskId":"2","status":"deleted"}}"#), 600).unwrap();
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].status.as_str(), "completed");
    assert_eq!(store.get_task(tid).unwrap().column, "done");

    // 重放同一条 Create（hook 重复投递）不长重复行。
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TaskCreate",
        "tool_input":{"subject":"生成基线迁移(改名)"},
        "tool_response":"Task #1 created successfully: 生成基线迁移(改名)"}"#), 700).unwrap();
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].content, "生成基线迁移(改名)");
}

/// 结果文本改版/缺失时解析放弃，事件降级为 touch——绝不能造错误编号或报错冒泡。
#[test]
fn claude_task_create_without_parsable_response_is_a_noop() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/home/me/proj"}"#),
        100,
    )
    .unwrap();
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TaskCreate",
        "tool_input":{"subject":"x"},"tool_response":"格式变了"}"#), 200).unwrap();
    let sid = store.find_session_id_pub("s1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    assert!(store.list_todos(tid).unwrap().is_empty());
}

#[test]
fn stop_then_end_updates_session_status() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"s2","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"Stop","session_id":"s2"}"#),
        200,
    )
    .unwrap();
    let sid = store.find_session_id_pub("s2").unwrap().unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");

    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionEnd","session_id":"s2"}"#),
        300,
    )
    .unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "ended");
}

#[test]
fn unknown_session_for_prompt_is_ignored_gracefully() {
    let store = Store::open_in_memory().unwrap();
    let r = disp(
        &store,
        &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"ghost","prompt":"x"}"#),
        100,
    );
    assert!(r.is_ok());
}

#[test]
fn posttooluse_bash_sets_current_activity() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"b1","cwd":"/tmp/p"}"#),
        100,
    )
    .unwrap();
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"b1","tool_name":"Bash","tool_input":{"command":"cargo build"}}"#), 200).unwrap();
    let sid = store.find_session_id_pub("b1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    assert_eq!(
        store.get_task(tid).unwrap().current_activity.as_deref(),
        Some("› cargo build")
    );
}

/// 非 Bash 工具也写活动名(此前只有 Bash 写,Edit/Read 长跑期间前端一直显示上一条早已
/// 完成的 Bash 命令);新回合的 UserPromptSubmit 清掉残留活动名。
#[test]
fn non_bash_tool_updates_activity_and_new_prompt_clears_it() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"nb1","cwd":"/tmp/p"}"#),
        100,
    )
    .unwrap();
    let sid = store.find_session_id_pub("nb1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    let activity = || store.get_task(tid).unwrap().current_activity;

    // Bash 语义不变:带 › 前缀的命令。
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"nb1","tool_name":"Bash","tool_input":{"command":"cargo build"}}"#), 200).unwrap();
    assert_eq!(activity().as_deref(), Some("› cargo build"));

    // 非 Bash 工具:写工具名本身。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PostToolUse","session_id":"nb1","tool_name":"Edit"}"#),
        300,
    )
    .unwrap();
    assert_eq!(activity().as_deref(), Some("Edit"));

    // 工具名缺席:回落 touch,不改活动名。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PostToolUse","session_id":"nb1"}"#),
        400,
    )
    .unwrap();
    assert_eq!(activity().as_deref(), Some("Edit"));

    // 新回合:残留活动名清空(否则新回合首个工具调用前一直显示旧命令)。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"nb1","prompt":"next"}"#),
        500,
    )
    .unwrap();
    assert_eq!(activity(), None);
}

#[test]
fn stop_and_end_for_unknown_session_are_ignored() {
    let store = Store::open_in_memory().unwrap();
    assert!(disp(
        &store,
        &ev(r#"{"hook_event_name":"Stop","session_id":"nope"}"#),
        100
    )
    .is_ok());
    assert!(disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionEnd","session_id":"nope"}"#),
        100
    )
    .is_ok());
}

#[test]
fn session_start_with_transcript_sets_ai_title() {
    let store = Store::open_in_memory().unwrap();
    let tp = write_transcript(
        "cc_disp_start.jsonl",
        b"{\"type\":\"ai-title\",\"aiTitle\":\"\xe5\x81\x9a\xe7\x9c\x8b\xe6\x9d\xbf\",\"sessionId\":\"s\"}\n",
    );
    let tps = tp.to_str().unwrap().replace('\\', "\\\\");
    let json = format!(
        r#"{{"hook_event_name":"SessionStart","session_id":"st1","cwd":"/tmp/x","transcript_path":"{tps}"}}"#
    );
    disp(&store, &ev(&json), 100).unwrap();
    let sid = store.find_session_id_pub("st1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    assert_eq!(store.get_task(tid).unwrap().title, "做看板");
    let _ = std::fs::remove_file(tp);
}

/// Stop 事件无 cwd，但 SessionStart 时已存进库，应能用存的 cwd 重建路径并刷新标题。
#[test]
fn stop_refreshes_title_via_stored_cwd() {
    let store = Store::open_in_memory().unwrap();
    // 先 SessionStart（不带 transcript_path），把 cwd 存进库
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"st3","cwd":"/tmp/z"}"#),
        100,
    )
    .unwrap();

    // 写 transcript（用真实路径直接传给下方 Stop）
    let tp = write_transcript(
        "cc_disp_stop.jsonl",
        b"{\"type\":\"ai-title\",\"aiTitle\":\"Stop\xe5\x88\xb7\xe6\x96\xb0\",\"sessionId\":\"s\"}\n",
    );
    let tps = tp.to_str().unwrap().replace('\\', "\\\\");

    // Stop 带 transcript_path 但不带 cwd——由 store 里的 cwd 兜底
    let json =
        format!(r#"{{"hook_event_name":"Stop","session_id":"st3","transcript_path":"{tps}"}}"#);
    disp(&store, &ev(&json), 200).unwrap();

    let sid = store.find_session_id_pub("st3").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    // 标题已从 transcript 刷新
    assert_eq!(store.get_task(tid).unwrap().title, "Stop刷新");
    // 状态应为 waiting
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");
    let _ = std::fs::remove_file(tp);
}

#[test]
fn hookevent_parses_last_assistant_message_and_alias() {
    let a =
        ev(r#"{"hook_event_name":"Stop","session_id":"s","last_assistant_message":"结论更微妙"}"#);
    assert_eq!(a.last_assistant_message.as_deref(), Some("结论更微妙"));
    // 官方文档另称 assistant_message,alias 也要能接住。
    let b = ev(r#"{"hook_event_name":"Stop","session_id":"s","assistant_message":"另一种字段名"}"#);
    assert_eq!(b.last_assistant_message.as_deref(), Some("另一种字段名"));
}

/// UserPromptSubmit 不带 cwd，但 SessionStart 存了，apply_title 应用存的 cwd 重建路径。
#[test]
fn prompt_without_cwd_uses_stored_cwd_for_title() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"st4","cwd":"/tmp/w"}"#),
        100,
    )
    .unwrap();

    let tp = write_transcript(
        "cc_disp_nocwd.jsonl",
        b"{\"type\":\"custom-title\",\"customTitle\":\"NoCwd\xe5\x85\x9c\xe5\xba\x95\",\"sessionId\":\"s\"}\n",
    );
    let tps = tp.to_str().unwrap().replace('\\', "\\\\");

    // UserPromptSubmit 不带 cwd，但带 transcript_path
    let json = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"st4","prompt":"hello","transcript_path":"{tps}"}}"#
    );
    disp(&store, &ev(&json), 200).unwrap();

    let sid = store.find_session_id_pub("st4").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    assert_eq!(store.get_task(tid).unwrap().title, "NoCwd兜底");
    let _ = std::fs::remove_file(tp);
}

#[test]
fn user_prompt_with_transcript_overrides_prompt_title() {
    let store = Store::open_in_memory().unwrap();
    let tp = write_transcript(
        "cc_disp_prompt.jsonl",
        b"{\"type\":\"custom-title\",\"customTitle\":\"My Custom Title\",\"sessionId\":\"s\"}\n",
    );
    let tps = tp.to_str().unwrap().replace('\\', "\\\\");

    // 先 SessionStart（无 transcript）
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"st2","cwd":"/tmp/y"}"#),
        100,
    )
    .unwrap();
    // 再 UserPromptSubmit（带 transcript）
    let json = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"st2","prompt":"首条prompt兜底","transcript_path":"{tps}"}}"#
    );
    disp(&store, &ev(&json), 200).unwrap();
    let sid = store.find_session_id_pub("st2").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    // transcript custom-title 覆盖了 prompt 兜底标题
    assert_eq!(store.get_task(tid).unwrap().title, "My Custom Title");
    let _ = std::fs::remove_file(tp);
}

// == Task 6: PermissionRequest / PreToolUse 置 pending_review ==
#[test]
fn permission_and_pretooluse_set_pending_review() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"p1","cwd":"/p"}"#),
        100,
    )
    .unwrap();

    let kind = |cc: &str| {
        store
            .live_sessions(None, None, None, None, 1000)
            .unwrap()
            .into_iter()
            .find(|l| l.session.cc_session_id == cc)
            .unwrap()
            .pending_review
    };

    // PermissionRequest:无 tool_name/普通工具 → approval。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"p1","tool_name":"Bash"}"#),
        200,
    )
    .unwrap();
    assert_eq!(kind("p1").as_deref(), Some("approval"));
    // PermissionRequest:ExitPlanMode → plan。
    disp(&store, &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"p1","tool_name":"ExitPlanMode"}"#), 210).unwrap();
    assert_eq!(kind("p1").as_deref(), Some("plan"));
    // PreToolUse:AskUserQuestion → question。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PreToolUse","session_id":"p1","tool_name":"AskUserQuestion"}"#),
        220,
    )
    .unwrap();
    assert_eq!(kind("p1").as_deref(), Some("question"));
    // PreToolUse:其它工具 → 无操作(保持上一个 question)。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PreToolUse","session_id":"p1","tool_name":"Read"}"#),
        230,
    )
    .unwrap();
    assert_eq!(kind("p1").as_deref(), Some("question"));
}

// == Task 5: Stop 落 last_ai_text、UserPromptSubmit 落 last_user_text ==
#[test]
fn stop_sets_last_ai_text_and_prompt_sets_last_user_text() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"m1","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"m1","prompt":"切到这个任务"}"#),
        200,
    )
    .unwrap();
    disp(&store, &ev(r#"{"hook_event_name":"Stop","session_id":"m1","last_assistant_message":"调研完成,结论更微妙"}"#), 300).unwrap();

    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "m1")
        .unwrap();
    assert_eq!(s.last_user_text.as_deref(), Some("切到这个任务"));
    assert_eq!(s.last_ai_text.as_deref(), Some("调研完成,结论更微妙"));
}

#[test]
fn pending_review_cleared_by_next_event() {
    for (i, clear_ev) in [
        r#"{"hook_event_name":"PostToolUse","session_id":"c1","tool_name":"Read"}"#,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"c1","prompt":"继续"}"#,
        r#"{"hook_event_name":"Stop","session_id":"c1"}"#,
        r#"{"hook_event_name":"SessionEnd","session_id":"c1"}"#,
    ]
    .iter()
    .enumerate()
    {
        let store = Store::open_in_memory().unwrap();
        disp(
            &store,
            &ev(r#"{"hook_event_name":"SessionStart","session_id":"c1","cwd":"/p"}"#),
            100,
        )
        .unwrap();
        disp(
            &store,
            &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"c1","tool_name":"Bash"}"#),
            200,
        )
        .unwrap();
        // 置位后确认非空。
        let pending = store
            .live_sessions(None, None, None, None, 1000)
            .unwrap()
            .into_iter()
            .find(|l| l.session.cc_session_id == "c1")
            .unwrap()
            .pending_review;
        assert_eq!(pending.as_deref(), Some("approval"), "case {i} 置位前提");
        // 下一个事件清除。
        disp(&store, &ev(clear_ev), 300).unwrap();
        let pending = store
            .live_sessions(None, None, None, None, 1000)
            .unwrap()
            .into_iter()
            .find(|l| l.session.cc_session_id == "c1")
            .unwrap()
            .pending_review;
        assert_eq!(pending, None, "case {i} 应被清除");
    }
}

#[test]
fn image_only_user_prompt_starts_a_new_running_turn() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"img1","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"Stop","session_id":"img1"}"#),
        200,
    )
    .unwrap();
    assert_eq!(
        store
            .get_session(store.find_session_id_pub("img1").unwrap().unwrap())
            .unwrap()
            .status,
        "waiting"
    );

    // kimi 的纯图片 prompt 没有 text 块，但用户确实已经开启新回合。
    dispatch(
        &store,
        &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"img1","prompt":[{"type":"image","data":"..."}]}"#),
        300,
        meowo_agent::id::KIMI.as_str(),
    )
    .unwrap();
    let s = store
        .get_session(store.find_session_id_pub("img1").unwrap().unwrap())
        .unwrap();
    assert_eq!(s.status, "running");
    assert_eq!(s.last_event_at, 300);
}

#[test]
fn provider_defaults_claude_and_kimi_is_tagged() {
    let store = Store::open_in_memory().unwrap();
    // 默认 provider（不带 --provider）→ claude。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"cl1","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    // kimi provider 显式标记。
    dispatch(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"km1","cwd":"/p"}"#),
        110,
        meowo_agent::id::KIMI.as_str(),
    )
    .unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let prov = |sid: &str| {
        live.iter()
            .find(|l| l.session.cc_session_id == sid)
            .unwrap()
            .provider
            .clone()
    };
    assert_eq!(prov("cl1"), "claude");
    assert_eq!(prov("km1"), "kimi");
}

/// 回归（Copilot review）：本版本尚不认识的 provider（跨版本时更新版 meowo 写入的 id，如 `gemini`）
/// 必须**原样落库**，绝不因「查不到插件」就冒名成默认 agent。
///
/// 此前 dispatch 入参是 `AgentId`，未知值在 main.rs 就被回退成 `DEFAULT_ID`，于是这类会话落成
/// NULL/claude——破了 `meowo_agent::resolve` 的「未知 id → None，绝不降级」契约。现在全链路传
/// 原始 `&str`。
#[test]
fn unknown_provider_is_persisted_verbatim_not_coerced_to_default() {
    let store = Store::open_in_memory().unwrap();
    dispatch(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"gm1","cwd":"/p"}"#),
        100,
        "gemini", // 本版本没有这个插件
    )
    .unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let l = live
        .iter()
        .find(|l| l.session.cc_session_id == "gm1")
        .unwrap();
    assert_eq!(
        l.provider, "gemini",
        "未知 provider 必须原样保留，不得冒名成 claude"
    );
}

#[test]
fn activity_event_revives_mis_reaped_ended_session() {
    // 会话被误 reap 成 ended（如 app 的 reap 一度不认 kimi pid）后，任一活动事件都应复活,
    // 不只 UserPromptSubmit。误 reap 走 end_session_if_pid（reaper 路径,留 pid 作墨迹）——
    // revive 靠该墨迹即刻自愈;正常 SessionEnd（清 pid）的迟到尾巴则不复活（见下一测试）。
    let store = Store::open_in_memory().unwrap();
    dispatch(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"rv1","cwd":"/p"}"#),
        100,
        meowo_agent::id::KIMI.as_str(),
    )
    .unwrap();
    let sid = store.find_session_id_pub("rv1").unwrap().unwrap();
    store.set_session_pid(sid, 999, 120).unwrap();
    assert!(store.end_session_if_pid(sid, 999, 120, 150).unwrap()); // 模拟被误 reap
    dispatch(
        &store,
        &ev(
            r#"{"hook_event_name":"PostToolUse","session_id":"rv1","cwd":"/p","tool_name":"Read"}"#,
        ),
        200,
        meowo_agent::id::KIMI.as_str(),
    )
    .unwrap();
    let s = store
        .live_sessions(None, None, None, None, 1000)
        .unwrap()
        .into_iter()
        .find(|l| l.session.cc_session_id == "rv1")
        .unwrap();
    assert_eq!(s.session.status, "running");
    assert_eq!(s.session.ended_at, None); // ended_at 被清，状态自洽
}

/// M3:正常 SessionEnd 之后排队迟到的同回合 PostToolUse 不得诈尸——曾复活成 running 且
/// pid NULL,卡片假活 120s 才被 end_orphaned_idle 再收一次。
#[test]
fn straggler_posttooluse_after_session_end_does_not_resurrect() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"zz1","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionEnd","session_id":"zz1"}"#),
        200,
    )
    .unwrap();
    // SessionEnd 后 300ms 才被排队放行的回合尾巴。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PostToolUse","session_id":"zz1","cwd":"/p","tool_name":"Read"}"#),
        500,
    )
    .unwrap();
    let sid = store.find_session_id_pub("zz1").unwrap().unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "ended", "尾巴不得诈尸");
}

/// M3:SessionEnd 后排队迟到的 Stop 尾巴同样不得诈尸——它绕过活动类 touch,走的是
/// set_session_status(Waiting),该方法必须拒绝在 ended 行上翻状态（复活只归 revive_* 一族）。
#[test]
fn straggler_stop_after_session_end_does_not_resurrect() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"zz2","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionEnd","session_id":"zz2"}"#),
        200,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"Stop","session_id":"zz2"}"#),
        400,
    )
    .unwrap();
    let sid = store.find_session_id_pub("zz2").unwrap().unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "ended", "Stop 尾巴不得把 ended 翻成 waiting");
    assert!(s.ended_at.is_some());
}

/// M1:PermissionRequest/PreToolUse 打在已结束会话上（SessionEnd 后的迟到尾巴）不得把
/// pending 写在尸体上——那样卡片看不见、通知全哑,且永远没人清它。
#[test]
fn permission_request_on_ended_session_is_dropped() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"pe1","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionEnd","session_id":"pe1"}"#),
        200,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"pe1","tool_name":"Bash"}"#),
        210,
    )
    .unwrap();
    let sid = store.find_session_id_pub("pe1").unwrap().unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "ended");
    assert_eq!(
        store.session_pending_review(sid).unwrap(),
        None,
        "尸体上不得挂 pending"
    );
}

/// M1:误 reap 的会话来了 PermissionRequest——agent 正阻塞等授权,不会再有别的活动事件,
/// 这条就是复活与通知的唯一机会,必须复活并置 pending。
#[test]
fn permission_request_revives_mis_reaped_session_and_sets_pending() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"pe2","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    let sid = store.find_session_id_pub("pe2").unwrap().unwrap();
    store.set_session_pid(sid, 777, 120).unwrap();
    assert!(store.end_session_if_pid(sid, 777, 120, 150).unwrap()); // 误 reap(留 pid 墨迹)
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"pe2","tool_name":"Bash"}"#),
        200,
    )
    .unwrap();
    let s = store.get_session(sid).unwrap();
    assert_ne!(s.status, "ended");
    assert_eq!(
        store.session_pending_review(sid).unwrap().as_deref(),
        Some("approval")
    );
}

/// M7:Stop 置 waiting 后,迟到窗内的 PostToolUse 尾巴不得推进 last_event_at——
/// 它是「等待你回复」通知的去重指纹,被尾巴推进会让同一回合弹两次。
#[test]
fn straggler_posttooluse_after_stop_keeps_waiting_fingerprint() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"fp1","cwd":"/p"}"#),
        100,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"Stop","session_id":"fp1"}"#),
        200,
    )
    .unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PostToolUse","session_id":"fp1","cwd":"/p","tool_name":"Read"}"#),
        450,
    )
    .unwrap();
    let sid = store.find_session_id_pub("fp1").unwrap().unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "waiting", "回合尾巴不得顶回 running");
    assert_eq!(s.last_event_at, 200, "通知指纹(last_event_at)不得被尾巴推进");
}

#[test]
fn lazy_creates_session_on_prompt_when_session_start_missing() {
    // 模拟「hooks 中途装上」：没有 SessionStart，直接来 UserPromptSubmit（带 cwd）→ 应就地建会话。
    let store = Store::open_in_memory().unwrap();
    dispatch(
        &store,
        &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"mid1","cwd":"/p","prompt":"中途接入"}"#),
        100,
        meowo_agent::id::KIMI.as_str(),
    )
    .unwrap();
    let l = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = l
        .iter()
        .find(|l| l.session.cc_session_id == "mid1")
        .expect("应懒创建出会话");
    assert_eq!(s.provider, "kimi");
    assert_eq!(s.task_title, "中途接入");
}

/// 半态会话自愈：SessionStart 落库中断过（历史事故：老库缺 profile 列）的会话 cwd 恒 NULL，
/// 对话窗识别不到工作区。后续任一带 cwd 的事件命中已存在会话时应回填 cwd；已有值不覆盖。
#[test]
fn event_with_cwd_backfills_half_created_session() {
    let store = Store::open_in_memory().unwrap();
    // 直接 start_session 造出「建了行、cwd 没写上」的半态。
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "half", 100).unwrap();
    assert_eq!(store.session_cwd(sid).unwrap(), None);

    disp(
        &store,
        &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"half","cwd":"/home/me/proj","prompt":"继续"}"#),
        200,
    )
    .unwrap();
    assert_eq!(
        store.session_cwd(sid).unwrap().as_deref(),
        Some("/home/me/proj")
    );

    // 换个目录再来一条事件：cwd 已有值，保持「会话启动时的目录」不被覆盖。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"PostToolUse","session_id":"half","cwd":"/elsewhere","tool_name":"Bash","tool_input":{"command":"ls"}}"#),
        300,
    )
    .unwrap();
    assert_eq!(
        store.session_cwd(sid).unwrap().as_deref(),
        Some("/home/me/proj")
    );
}

/// hook 的 cwd 跟随 Bash 持久 shell 的 cd 漂移。auto-compact 会以同 session_id 在回合
/// 中途重发 SessionStart(source=compact),带的是漂移后的当前目录——实拍事故:会话工作区
/// 从仓库根被覆写成 `app\src-tauri`,transcript 路径重建从此落空。compact 不得覆写;
/// startup/resume(新进程拉起,cwd=启动目录)保留覆写——手动 resume 纠正搬家后的陈旧
/// cwd 的自愈靠它。
#[test]
fn compact_session_start_must_not_overwrite_cwd() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"cw1","source":"startup","cwd":"/repo"}"#),
        100,
    )
    .unwrap();
    let sid = store.find_session_id_pub("cw1").unwrap().unwrap();
    assert_eq!(store.session_cwd(sid).unwrap().as_deref(), Some("/repo"));

    // 回合中途 auto-compact:shell 已 cd 进子目录,cwd 不可信 → 保持启动目录。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"cw1","source":"compact","cwd":"/repo/app/src-tauri"}"#),
        200,
    )
    .unwrap();
    assert_eq!(
        store.session_cwd(sid).unwrap().as_deref(),
        Some("/repo"),
        "compact 的 SessionStart 不得用漂移 cwd 覆写工作区"
    );

    // 项目搬家后手动 resume:新进程在新目录拉起,覆写即自愈。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"cw1","source":"resume","cwd":"/moved/repo"}"#),
        300,
    )
    .unwrap();
    assert_eq!(
        store.session_cwd(sid).unwrap().as_deref(),
        Some("/moved/repo"),
        "resume 的 SessionStart 应保留覆写语义"
    );

    // 无 source 字段(其他 agent / 旧版):维持原覆写行为,不回退。
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"cw1","cwd":"/plain"}"#),
        400,
    )
    .unwrap();
    assert_eq!(store.session_cwd(sid).unwrap().as_deref(), Some("/plain"));
}

/// compact 的 SessionStart 撞上「行不存在」(hooks 中途装上、SessionStart 曾丢失):
/// 新行没有旧值可保,backfill 也必须把 cwd 写上,不能留 NULL 半态。
#[test]
fn compact_session_start_still_fills_cwd_on_fresh_row() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"cw2","source":"compact","cwd":"/repo/sub"}"#),
        100,
    )
    .unwrap();
    let sid = store.find_session_id_pub("cw2").unwrap().unwrap();
    assert_eq!(store.session_cwd(sid).unwrap().as_deref(), Some("/repo/sub"));
}

/// 新回合（UserPromptSubmit）把现有待办标成「上一任务」残留：任务 A 的清单不能在
/// 任务 B 的待办写出来之前继续冒充当前进度（实拍：待办卡/看板一直显示旧任务的 3/3）。
/// 不删行（读不到 ≠ 已清空）——下一条 TodoWrite 快照到达时整份重插，残留标记随之清零。
#[test]
fn new_prompt_marks_todos_stale_and_next_snapshot_clears() {
    let store = Store::open_in_memory().unwrap();
    disp(
        &store,
        &ev(r#"{"hook_event_name":"SessionStart","session_id":"st1","cwd":"/tmp/p"}"#),
        100,
    )
    .unwrap();
    let sid = store.find_session_id_pub("st1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    // 任务 A：两条待办全部完成。
    disp(&store, &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"st1","prompt":"任务A"}"#), 200).unwrap();
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"st1","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"a","status":"completed"},{"content":"b","status":"completed"}]}}"#), 300).unwrap();
    assert!(store.list_todos(tid).unwrap().iter().all(|t| !t.stale));

    // 用户下发任务 B：旧清单标成残留，但行还在（完成记录留得下来）。
    disp(&store, &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"st1","prompt":"任务B"}"#), 400).unwrap();
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(todos.len(), 2);
    assert!(todos.iter().all(|t| t.stale), "新回合应把旧待办标成上一任务残留");

    // 任务 B 的第一份快照到达：整份重插,残留清零。
    disp(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"st1","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"c","status":"in_progress"}]}}"#), 500).unwrap();
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].content, "c");
    assert!(!todos[0].stale);
}
