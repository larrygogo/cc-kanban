use meowo_reporter::hook::HookEvent;

#[test]
fn empty_and_truncated_json_is_err() {
    assert!(HookEvent::parse("").is_err());
    assert!(HookEvent::parse("{").is_err());
    assert!(HookEvent::parse("{\"a\":").is_err());
}

#[test]
fn non_object_json_is_err() {
    assert!(HookEvent::parse("[]").is_err());
    assert!(HookEvent::parse("\"x\"").is_err());
    assert!(HookEvent::parse("42").is_err());
}

#[test]
fn missing_hook_event_name_is_err() {
    assert!(HookEvent::parse(r#"{"session_id":"a"}"#).is_err());
}

#[test]
fn null_tool_input_yields_no_todo_snapshot_and_no_bash() {
    let ev =
        HookEvent::parse(r#"{"hook_event_name":"PostToolUse","session_id":"a","tool_input":null}"#)
            .unwrap();
    // tool_input 缺席 = 「没有 todos 键」，不是空清单——必须返回 None 让调用方跳过同步，
    // 否则空列表会把整份待办表 DELETE 掉。
    assert_eq!(ev.todo_items(), None);
    assert_eq!(ev.bash_command(), None);
}

#[test]
fn permission_suggestions_are_preserved_for_the_gui_broker() {
    let ev = HookEvent::parse(
        r#"{
        "hook_event_name":"PermissionRequest",
        "session_id":"a",
        "permission_suggestions":[{
            "type":"addRules",
            "behavior":"allow",
            "destination":"localSettings",
            "rules":[{"toolName":"Bash","ruleContent":"cargo test"}]
        }]
    }"#,
    )
    .unwrap();
    assert_eq!(ev.permission_suggestions.len(), 1);
    assert_eq!(ev.permission_suggestions[0]["destination"], "localSettings");
}

#[test]
fn todos_not_array_yields_no_snapshot() {
    // todos 键存在但不是数组：与键缺失同等对待（None，跳过同步）——这不是 agent
    // 在表达「清空」，是畸形输入。
    let ev = HookEvent::parse(r#"{"hook_event_name":"PostToolUse","session_id":"a","tool_name":"TodoWrite","tool_input":{"todos":"oops"}}"#).unwrap();
    assert_eq!(ev.todo_items(), None);
}

/// 键缺失（None）与显式空数组（Some(空)）必须区分：前者跳过同步保持现状，
/// 后者是 agent 明确清空，合法语义照清。
#[test]
fn explicit_empty_todos_array_is_some_empty() {
    let ev = HookEvent::parse(r#"{"hook_event_name":"PostToolUse","session_id":"a","tool_name":"TodoWrite","tool_input":{"todos":[]}}"#).unwrap();
    assert_eq!(ev.todo_items(), Some(vec![]));
}

#[test]
fn todo_element_missing_content_gets_placeholder() {
    // 缺 content 的行不丢：占位保留，否则快照只剩部分行、进度计数失真。
    let ev = HookEvent::parse(r#"{"hook_event_name":"PostToolUse","session_id":"a","tool_name":"TodoWrite","tool_input":{"todos":[{"status":"completed"},{"content":"ok","status":"in_progress"}]}}"#).unwrap();
    let items = ev.todo_items().expect("显式 todos 键应解析出快照");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].content, "（未命名事项）");
    assert_eq!(items[0].status.as_str(), "completed");
    assert_eq!(items[1].content, "ok");
}

#[test]
fn bash_command_non_string_is_none() {
    let ev = HookEvent::parse(r#"{"hook_event_name":"PostToolUse","session_id":"a","tool_name":"Bash","tool_input":{"command":123}}"#).unwrap();
    assert_eq!(ev.bash_command(), None);
}
