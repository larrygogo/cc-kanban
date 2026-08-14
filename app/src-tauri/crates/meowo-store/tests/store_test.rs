use meowo_store::{
    PendingReview, Project, Session, SessionStatus, Store, Task, TaskColumn, Todo, TodoInput,
    TodoStatus,
};

#[test]
fn open_in_memory_creates_tables() {
    let store = Store::open_in_memory().expect("open");
    let count: i64 = store.raw_table_count().expect("count tables");
    // projects / sessions / tasks / todos / events / session_context / session_notes
    assert_eq!(count, 7);
}

// == Task 4 ==
#[test]
fn upsert_project_is_idempotent_by_root() {
    let store = Store::open_in_memory().unwrap();
    let id1 = store
        .upsert_project_by_root("/home/me/proj", "proj", 1000)
        .unwrap();
    let id2 = store
        .upsert_project_by_root("/home/me/proj", "proj", 2000)
        .unwrap();
    assert_eq!(id1, id2);

    let projects: Vec<Project> = store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "proj");
    assert_eq!(projects[0].updated_at, 2000);
}

#[test]
fn upsert_project_updates_name_on_conflict() {
    let store = Store::open_in_memory().unwrap();
    let id1 = store.upsert_project_by_root("/r", "old-name", 100).unwrap();
    let id2 = store
        .upsert_project_by_root("/r", "owner/repo", 200)
        .unwrap();
    assert_eq!(id1, id2);
    assert_eq!(store.list_projects().unwrap()[0].name, "owner/repo");
}

// == Task 5 ==
#[test]
fn start_session_creates_session_and_placeholder_task() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-abc", 200).unwrap();
    assert!(sid > 0 && tid > 0);

    let (sid2, tid2) = store.start_session(pid, "cc-abc", 300).unwrap();
    assert_eq!(sid, sid2);
    assert_eq!(tid, tid2);

    let task: Task = store.get_task(tid).unwrap();
    assert_eq!(task.title, "(未命名会话)");
    assert_eq!(task.column, "todo");
    assert_eq!(task.session_id, Some(sid));
}

// == Task 6 ==
#[test]
fn first_prompt_sets_title_then_later_prompts_keep_title() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-1", 200).unwrap();

    store
        .on_user_prompt(sid, "实现登录功能并写测试", 300)
        .unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "实现登录功能并写测试");

    store.on_user_prompt(sid, "再加个登出按钮", 400).unwrap();
    let t2 = store.get_task(tid).unwrap();
    assert_eq!(t2.title, "实现登录功能并写测试");
}

#[test]
fn long_prompt_title_is_truncated_to_60_chars() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-2", 200).unwrap();
    let long = "字".repeat(80);
    store.on_user_prompt(sid, &long, 300).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title.chars().count(), 60);
}

// == Task 7 ==
#[test]
fn sync_todos_replaces_list_and_derives_column() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-1", 200).unwrap();

    store
        .sync_todos(
            sid,
            &[
                TodoInput {
                    content: "解析".into(),
                    status: TodoStatus::Completed,
                },
                TodoInput {
                    content: "建图".into(),
                    status: TodoStatus::InProgress,
                },
                TodoInput {
                    content: "测试".into(),
                    status: TodoStatus::Pending,
                },
            ],
            300,
        )
        .unwrap();

    let todos: Vec<Todo> = store.list_todos(tid).unwrap();
    assert_eq!(todos.len(), 3);
    assert_eq!(todos[0].content, "解析");
    assert_eq!(store.get_task(tid).unwrap().column, "doing");

    store
        .sync_todos(
            sid,
            &[
                TodoInput {
                    content: "解析".into(),
                    status: TodoStatus::Completed,
                },
                TodoInput {
                    content: "建图".into(),
                    status: TodoStatus::Completed,
                },
            ],
            400,
        )
        .unwrap();
    assert_eq!(store.list_todos(tid).unwrap().len(), 2);
    assert_eq!(store.get_task(tid).unwrap().column, "done");
}

#[test]
fn sync_todos_does_not_override_locked_column() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-1", 200).unwrap();
    store
        .set_task_column(tid, TaskColumn::Done, true, 250)
        .unwrap();

    store
        .sync_todos(
            sid,
            &[TodoInput {
                content: "x".into(),
                status: TodoStatus::InProgress,
            }],
            300,
        )
        .unwrap();
    assert_eq!(store.get_task(tid).unwrap().column, "done");
}

// == Task 8 ==
#[test]
fn stop_sets_waiting_and_end_sets_ended() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _tid) = store.start_session(pid, "cc-1", 200).unwrap();

    store
        .set_session_status(sid, SessionStatus::Waiting, 300)
        .unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");

    store.end_session(sid, 400).unwrap();
    let s: Session = store.get_session(sid).unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(s.ended_at, Some(400));
}

#[test]
fn empty_todos_resets_column_to_todo() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-e", 200).unwrap();
    // 先 doing
    store
        .sync_todos(
            sid,
            &[meowo_store::TodoInput {
                content: "x".into(),
                status: meowo_store::TodoStatus::InProgress,
            }],
            300,
        )
        .unwrap();
    assert_eq!(store.get_task(tid).unwrap().column, "doing");
    // 清空 -> 回 todo
    store.sync_todos(sid, &[], 400).unwrap();
    assert_eq!(store.get_task(tid).unwrap().column, "todo");
    assert_eq!(store.list_todos(tid).unwrap().len(), 0);
}

/// 回归：会话时钟被**别的**事件推进后，更新的 Todo 快照仍必须落库。
///
/// 现场——kimi 边跑边勾选待办：每次 TodoList 带整份快照，中间夹着 Stop / 其它工具的
/// PostToolUse 把 session.last_event_at 顶到未来。旧守卫拿 last_event_at 当判据，于是
/// 「三条全 done」的最新快照因 now_ms 落后被整份丢弃，界面永远停在 pending（0/3）。
#[test]
fn todo_snapshot_lands_even_after_session_clock_advances() {
    use meowo_store::{TodoInput, TodoStatus};
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-todo", 200).unwrap();
    let snapshot = |a: &str, b: &str| {
        [
            TodoInput {
                content: "A".into(),
                status: TodoStatus::from_str(a),
            },
            TodoInput {
                content: "B".into(),
                status: TodoStatus::from_str(b),
            },
        ]
    };

    store
        .sync_todos(sid, &snapshot("in_progress", "pending"), 300)
        .unwrap();
    // 别的事件把会话时钟推到远处（Stop、其它工具的 PostToolUse 都会）。
    store.touch_session(sid, 900).unwrap();
    // 随后到达的待办快照 now_ms=400 < 900，但它是**更新的** todo 状态，必须落库。
    store
        .sync_todos(sid, &snapshot("completed", "completed"), 400)
        .unwrap();

    let todos = store.list_todos(tid).unwrap();
    assert_eq!(
        todos.iter().map(|t| t.status.as_str()).collect::<Vec<_>>(),
        vec!["completed", "completed"],
        "会话时钟领先时，更新的待办快照被错误丢弃了"
    );
}

#[test]
fn all_pending_todos_is_todo_column() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-p", 200).unwrap();
    store
        .sync_todos(
            sid,
            &[
                meowo_store::TodoInput {
                    content: "a".into(),
                    status: meowo_store::TodoStatus::Pending,
                },
                meowo_store::TodoInput {
                    content: "b".into(),
                    status: meowo_store::TodoStatus::Pending,
                },
            ],
            300,
        )
        .unwrap();
    assert_eq!(store.get_task(tid).unwrap().column, "todo");
}

#[test]
fn touch_session_revives_waiting_to_running() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _tid) = store.start_session(pid, "cc-r", 200).unwrap();
    store
        .set_session_status(sid, meowo_store::SessionStatus::Waiting, 300)
        .unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");
    store.touch_session(sid, 400).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "running");
}

#[test]
fn set_current_activity_updates_task() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-a", 200).unwrap();
    store
        .set_current_activity(sid, "› cargo test", 300)
        .unwrap();
    assert_eq!(
        store.get_task(tid).unwrap().current_activity.as_deref(),
        Some("› cargo test")
    );
}

#[test]
fn activity_clear_survives_taskless_sessions_and_clock_skew() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-skew", 200).unwrap();

    // 时钟回拨:活动在 t=900 落库,新回合的 UserPromptSubmit 带着 t=400 到达。
    // 清除必须无条件生效——新回合开始,旧命令必然过时;守卫式清除会让
    // 「› cargo build」贯穿整个新回合常显。
    store
        .set_current_activity(sid, "› cargo build", 900)
        .unwrap();
    store.clear_current_activity(sid, 400).unwrap();
    let task = store.get_task(tid).unwrap();
    assert_eq!(task.current_activity, None);
    // updated_at 不因回拨的清除而倒退。
    assert!(task.updated_at >= 900);

    // 无任务卡的会话(历史导入残行/不存在的行):活动清/设不得让整条 hook 分发中止。
    store.clear_current_activity(987_654, 500).unwrap();
    store.set_current_activity(987_654, "› echo", 500).unwrap();
}

#[test]
fn prompt_with_image_marker_is_cleaned_for_title() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-img", 200).unwrap();
    store
        .on_user_prompt(sid, "[Image #4] 把路径放在最前面", 300)
        .unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "把路径放在最前面");
}

#[test]
fn multiple_image_markers_and_whitespace_collapsed() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-img2", 200).unwrap();
    store
        .on_user_prompt(sid, "[Image #1]  改这个   [Image #2] 和那个 ", 300)
        .unwrap();
    assert_eq!(store.get_task(tid).unwrap().title, "改这个 和那个");
}

#[test]
fn image_only_prompt_keeps_placeholder_title() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-img3", 200).unwrap();
    store.on_user_prompt(sid, "[Image #1]", 300).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "(未命名会话)");
    assert_eq!(t.current_activity, None);
}

// == session cwd ==
#[test]
fn set_and_get_session_cwd() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    assert_eq!(store.session_cwd(sid).unwrap(), None);
    store.set_session_cwd(sid, "C:\\proj", 110).unwrap();
    assert_eq!(store.session_cwd(sid).unwrap().as_deref(), Some("C:\\proj"));
}

// == set_session_title ==
#[test]
fn set_session_title_overrides_placeholder_and_prompt_title() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "s", 100).unwrap();
    store.on_user_prompt(sid, "继续", 110).unwrap(); // 首条填充词当了标题
    assert_eq!(store.get_task(tid).unwrap().title, "继续");
    store
        .set_session_title(sid, "Claude Code 看板", 120)
        .unwrap();
    assert_eq!(store.get_task(tid).unwrap().title, "Claude Code 看板");
}

// == PID 存活检测 ==
#[test]
fn set_pid_and_liveness_query() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.set_session_pid(sid, 4242, 110).unwrap();
    let live = store.live_session_liveness().unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].0, sid);
    assert_eq!(live[0].1, Some(4242));
}

#[test]
fn ended_session_not_in_liveness() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s2", 100).unwrap();
    store.set_session_pid(sid, 9999, 110).unwrap();
    store.end_session(sid, 200).unwrap();
    let live = store.live_session_liveness().unwrap();
    assert!(live.is_empty());
}

#[test]
fn conditional_reap_never_ends_a_session_reclaimed_by_a_new_pid() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "reap-race", 100).unwrap();
    store.set_session_pid(sid, 111, 110).unwrap();
    // reaper 观察到 111 后，新进程先完成认领。
    store.set_session_pid(sid, 222, 120).unwrap();
    assert!(!store.end_session_if_pid(sid, 111, 110, 130).unwrap());
    let session = store.get_session(sid).unwrap();
    assert_eq!(session.status, "running");
    assert_eq!(store.session_pid(sid).unwrap(), Some(222));

    // pid 相同但其间已有新 hook，也必须拒绝用旧快照收尾。
    store.touch_session(sid, 130).unwrap();
    assert!(!store.end_session_if_pid(sid, 222, 120, 140).unwrap());
    assert!(store.end_session_if_pid(sid, 222, 130, 140).unwrap());
    assert_eq!(store.get_session(sid).unwrap().status, "ended");
}

#[test]
fn late_hooks_cannot_overwrite_newer_session_or_task_state() {
    let store = Store::open_in_memory().unwrap();
    let project = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(project, "out-of-order", 100).unwrap();

    store.set_session_title(sid, "新标题", 500).unwrap();
    store.set_session_title(sid, "旧标题", 400).unwrap();
    store.set_current_activity(sid, "新活动", 600).unwrap();
    store.set_current_activity(sid, "旧活动", 550).unwrap();
    // Todo 的迟到守卫只跟**它自己的更新时刻**（task.updated_at）比：先落 750 的新快照，
    // 再来一个 700 的旧快照就该被挡。**不能**跟 session.last_event_at 比——别的事件
    // （下面的 touch_session）会把会话时钟顶到未来，若拿它当判据，真正最新的 Todo 快照
    // 反而会因落后而被整份丢弃（实测：done 进不来，界面一直停在 pending）。
    let stale_todos = [TodoInput {
        content: "旧 Todo".into(),
        status: TodoStatus::Pending,
    }];
    let newest_todos = [TodoInput {
        content: "新 Todo".into(),
        status: TodoStatus::InProgress,
    }];
    store.sync_todos(sid, &newest_todos, 750).unwrap();
    // 会话被别的事件推进到 800，但这不该让随后到达的 700 旧快照能覆盖 750 的新快照。
    store.touch_session(sid, 800).unwrap();
    store.sync_todos(sid, &stale_todos, 700).unwrap();

    store.set_session_cwd(sid, "/new", 900).unwrap();
    store.set_session_cwd(sid, "/old", 850).unwrap();
    store
        .set_session_status(sid, SessionStatus::Waiting, 1_100)
        .unwrap();
    store.set_last_ai_text(sid, "新回复", 1_100).unwrap();
    store
        .set_session_status(sid, SessionStatus::Stale, 1_050)
        .unwrap();
    store.set_last_ai_text(sid, "旧回复", 1_050).unwrap();
    store
        .set_pending_review(sid, PendingReview::Approval, 1_050)
        .unwrap();
    store.end_session(sid, 1_050).unwrap();
    store.set_session_pid(sid, 222, 1_200).unwrap();
    store.set_session_pid(sid, 111, 1_150).unwrap();

    let task = store.get_task(tid).unwrap();
    assert_eq!(task.title, "新标题");
    assert_eq!(task.current_activity.as_deref(), Some("新活动"));
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].content, "新 Todo");
    assert_eq!(store.session_cwd(sid).unwrap().as_deref(), Some("/new"));
    assert_eq!(store.session_pid(sid).unwrap(), Some(222));

    let session = store.get_session(sid).unwrap();
    assert_eq!(session.status, "waiting");
    assert_eq!(session.last_event_at, 1_200);
    let live = store.live_sessions(None, None, None, None, 2_000).unwrap();
    let card = live.iter().find(|s| s.session.id == sid).unwrap();
    assert_eq!(card.last_ai_text.as_deref(), Some("新回复"));
    assert_eq!(card.pending_review, None);
}

// == live_sessions pid + end_orphaned_idle ==

#[test]
fn live_sessions_carries_pid() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.set_session_pid(sid, 1234, 110).unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].pid, Some(1234));
}

#[test]
fn set_pid_evicts_same_pid_from_other_sessions() {
    // /clear 等会在同一进程上开新会话：新会话认领 pid 后，旧会话的 pid 应被摘除，
    // 否则旧会话会因进程仍存活而一直误显示「已连接」。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (old, _) = store.start_session(pid, "old", 100).unwrap();
    store.set_session_pid(old, 7777, 110).unwrap();
    let (new, _) = store.start_session(pid, "new", 200).unwrap();
    store.set_session_pid(new, 7777, 210).unwrap(); // 同一进程认领新会话

    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let of = |cc: &str| live.iter().find(|s| s.session.cc_session_id == cc).unwrap();
    // 旧会话被收尾：pid 摘除 + 状态 ended → 不再误判已连接、状态也收尾。
    assert_eq!(of("old").pid, None);
    assert_eq!(of("old").session.status, "ended");
    // 新会话持有 pid，状态仍 live。
    assert_eq!(of("new").pid, Some(7777));
    assert_ne!(of("new").session.status, "ended");
}

#[test]
fn revive_for_resume_revives_ended_and_clears_pid() {
    // 看板 resume 一个已断开会话：应复活(脱离 ended)并清空 pid——旧进程已死，清 pid 让 reaper 不臆测收尾，
    // 卡片即刻显示已连接，新进程首个 hook 再认领 pid。覆盖 codex「session_start 要到首个 turn 才触发」场景。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.set_session_pid(sid, 5555, 110).unwrap();
    store.end_session(sid, 200).unwrap(); // 断开
    assert!(store.revive_for_resume(sid, 300, None).unwrap()); // 真的复活了
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session.cc_session_id, "s");
    assert_ne!(live[0].session.status, "ended"); // 已复活
    assert_eq!(live[0].pid, None); // pid 已清
}

#[test]
fn revive_for_resume_noop_on_connected_session() {
    // hook 已认领 pid 的活跃会话(非 ended 且 pid 非空、未验证到死 pid)不命中 →
    // pid 原样保留且返回 false，避免误清活跃会话/误触发失败回滚。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.set_session_pid(sid, 6666, 110).unwrap();
    assert!(!store.revive_for_resume(sid, 300, None).unwrap());
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert_eq!(live[0].pid, Some(6666));
}

#[test]
fn revive_for_resume_refreshes_pidless_running_session() {
    // 宽限过期后用户再次点 resume：会话 status 仍 running、pid 空(从未被 hook 认领) → 应刷新 last_event_at
    // 重启 app 侧乐观连接宽限，而不是因「非 ended」被跳过。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap(); // 默认 running、pid 空、last_event_at=100
    assert!(store.revive_for_resume(sid, 500, None).unwrap());
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert_eq!(live[0].pid, None);
    assert_eq!(live[0].session.last_event_at, 500); // 已刷新 → 宽限重启
}

#[test]
fn revive_for_resume_forces_when_pid_confirmed_dead() {
    // 进程刚死、reaper(5s 周期)尚未收尾的窗口内点 resume：status 仍 running 且 pid 非空，
    // 常规守卫不命中；调用方校验到该 pid 进程确已死亡后以 dead_pid=Some(旧 pid) 强制复活，
    // 否则本次 resume 静默 0 行更新、随后被 reaper 收尾成 ended，卡片长期显示未连接。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.set_session_pid(sid, 5555, 110).unwrap(); // running + pid 非空(进程实际已死)
    assert!(store.revive_for_resume(sid, 300, Some(5555)).unwrap());
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert_eq!(live[0].pid, None); // 旧死 pid 已清，reaper 不再臆测收尾
    assert_eq!(live[0].session.last_event_at, 300); // 宽限期重启
    assert_ne!(live[0].session.status, "ended");
}

#[test]
fn revive_for_resume_stale_dead_pid_does_not_clear_new_live_pid() {
    // TOCTOU 守卫：调用方快照校验旧 pid(5555) 已死之后、UPDATE 之前，新进程 hook 认领了
    // 新的存活 pid(7777)——dead_pid=Some(5555) 与行内当前 pid 不等，守卫必须不命中，
    // 绝不能把刚认领的活 pid 清掉(否则 120s 宽限过期后活会话被 end_orphaned_idle 误收尾)。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.set_session_pid(sid, 7777, 200).unwrap(); // 新进程已认领新活 pid
    assert!(!store.revive_for_resume(sid, 300, Some(5555)).unwrap()); // 持旧快照的迟到 UPDATE
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert_eq!(live[0].pid, Some(7777)); // 新活 pid 原样保留
    assert_eq!(live[0].session.last_event_at, 200); // 宽限未被重启
}

#[test]
fn end_orphaned_idle_only_reaps_pidless_stale_sessions() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    // s1：无 pid 且空闲超阈值 → 应被收尾。
    let (s1, _) = store.start_session(pid, "orphan-idle", 1000).unwrap();
    // s2：无 pid 但最近有事件（未超阈值）→ 保留。
    let (s2, _) = store.start_session(pid, "orphan-fresh", 1000).unwrap();
    store.touch_session(s2, 9000).unwrap();
    // s3：带 pid 且空闲很久（claude 在等用户输入）→ 绝不能误杀。
    let (s3, _) = store.start_session(pid, "connected-idle", 1000).unwrap();
    store.set_session_pid(s3, 1234, 1000).unwrap();

    // now=10000, idle阈值=2000。
    let n = store.end_orphaned_idle(2000, 10000).unwrap();
    assert_eq!(n, 1);
    assert_eq!(store.get_session(s1).unwrap().status, "ended");
    assert_eq!(store.get_session(s2).unwrap().status, "running");
    assert_ne!(store.get_session(s3).unwrap().status, "ended"); // 带 pid 不受空闲超时影响
}

// == 审计修复测试 ==

#[test]
fn session_start_revives_ended_session() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _t) = store.start_session(pid, "s", 100).unwrap();
    store.end_session(sid, 200).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "ended");
    // resume：同 session_id 再次 SessionStart 应复活为 running 且清空 ended_at
    let (sid2, _t2) = store.start_session(pid, "s", 300).unwrap();
    assert_eq!(sid2, sid);
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "running");
    assert_eq!(s.ended_at, None);
}

// == archived ==
#[test]
fn archive_flag_roundtrip_in_live_sessions() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    assert!(!store.live_sessions(None, None, None, None, 1000).unwrap()[0].archived);
    assert!(
        store.live_sessions(None, None, None, None, 1000).unwrap()[0]
            .archived_at
            .is_none()
    );
    store.set_session_archived(sid, true, 1234).unwrap();
    let s = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert!(s[0].archived);
    assert_eq!(s[0].archived_at, Some(1234)); // 归档记录时间戳
    store.set_session_archived(sid, false, 5678).unwrap();
    let s2 = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert!(!s2[0].archived);
    assert!(s2[0].archived_at.is_none()); // 取消归档清空时间戳
}

#[test]
fn import_session_inserts_ended_and_skips_existing() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1000).unwrap();

    let inserted = store
        .import_session("hist1", pid, "历史标题", Some("/p"), 5000)
        .unwrap();
    assert!(inserted);

    let sid = store.find_session_id_pub("hist1").unwrap().unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(s.started_at, 5000);
    assert_eq!(s.last_event_at, 5000);
    assert_eq!(s.ended_at, Some(5000));
    assert_eq!(store.session_cwd(sid).unwrap(), Some("/p".to_string()));

    let tid = store.task_id_of_session_pub(sid).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "历史标题");
    assert_eq!(t.column, "done");

    let again = store
        .import_session("hist1", pid, "改标题", Some("/p"), 9000)
        .unwrap();
    assert!(!again);
    let s2 = store.get_session(sid).unwrap();
    assert_eq!(s2.last_event_at, 5000);
    let t2 = store.get_task(tid).unwrap();
    assert_eq!(t2.title, "历史标题");
}

#[test]
fn import_session_does_not_resurrect_real_session() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1000).unwrap();
    let (sid, _) = store.start_session(pid, "live1", 2000).unwrap();

    let inserted = store.import_session("live1", pid, "x", None, 8000).unwrap();
    assert!(!inserted);
    assert_eq!(store.get_session(sid).unwrap().status, "running");
}

// == Task 3: last_ai_text / last_user_text ==
#[test]
fn last_ai_and_user_text_set_with_cleaning() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();

    // 折叠空白;不动 last_event_at(仍是建会话时的 100)。
    store
        .set_last_ai_text(sid, "  调研   完成。\n结论更微妙  ", 120)
        .unwrap();
    store
        .set_last_user_text(sid, "切到这个 [Image #1] 任务", 120)
        .unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "cc1")
        .unwrap();
    assert_eq!(s.last_ai_text.as_deref(), Some("调研 完成。 结论更微妙"));
    assert_eq!(s.last_user_text.as_deref(), Some("切到这个 任务")); // [Image #1] 被 sanitize 剥除
    assert_eq!(s.session.last_event_at, 100);

    // 空串/全空白不覆盖旧值。
    store.set_last_ai_text(sid, "   ", 120).unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "cc1")
        .unwrap();
    assert_eq!(s.last_ai_text.as_deref(), Some("调研 完成。 结论更微妙"));
}

// == Task 2: PendingReview ==
#[test]
fn pending_review_set_and_clear() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();

    // set:写入子态并刷新 last_event_at。
    store
        .set_pending_review(sid, PendingReview::Approval, 500)
        .unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "cc1")
        .unwrap();
    assert_eq!(s.pending_review.as_deref(), Some("approval"));
    assert_eq!(s.session.last_event_at, 500);

    // clear:置 NULL,且不改 last_event_at。
    store.clear_pending_review(sid, 500).unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "cc1")
        .unwrap();
    assert_eq!(s.pending_review, None);
    assert_eq!(s.session.last_event_at, 500);
}

#[test]
fn lifecycle_boundaries_clear_stale_pending_review() {
    let store = Store::open_in_memory().unwrap();
    let project = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(project, "pending-life", 100).unwrap();

    store
        .set_pending_review(sid, PendingReview::Approval, 150)
        .unwrap();
    store.end_session(sid, 200).unwrap();
    let ended = store
        .live_sessions(None, None, None, None, 100)
        .unwrap()
        .into_iter()
        .find(|l| l.session.id == sid)
        .unwrap();
    assert_eq!(ended.pending_review, None, "结束边界应清理旧审批子态");

    // 模拟旧版本数据库留下的 ended + pending_review 残留；resume 必须再次兜底清理。
    store
        .set_pending_review(sid, PendingReview::Plan, 250)
        .unwrap();
    assert!(store.revive_for_resume(sid, 300, None).unwrap());
    let resumed = store
        .live_sessions(None, None, None, None, 100)
        .unwrap()
        .into_iter()
        .find(|l| l.session.id == sid)
        .unwrap();
    assert_eq!(resumed.session.status, "running");
    assert_eq!(resumed.pending_review, None, "新运行周期不得继承旧审批子态");
}

// == Task 5: on_user_prompt 不再写 current_activity ==
#[test]
fn on_user_prompt_no_longer_writes_current_activity() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();

    store.on_user_prompt(sid, "实现登录功能", 200).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "实现登录功能"); // 占位标题被首句替换(保留)
    assert_eq!(t.current_activity, None); // 不再把 prompt 写进 current_activity
}

/// data_version 的两条性质是 db-watcher「只在真实写入时刷新看板」的根基（见 store::data_version）：
/// 1) 本连接自身的写入 / 纯读都不改自己的 data_version —— 故 watcher 的持久连接读版本永不自触发；
/// 2) 别的连接提交写入后，本连接再读 data_version 会变化 —— 故真实写入必被检出。
///
/// 必须用文件库（内存库连接互不共享），并在同进程内开两个独立连接。
#[test]
fn data_version_reflects_only_other_connection_writes() {
    let path = std::env::temp_dir().join(format!("meowo-dv-{}.db", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }

    let a = Store::open(&path).unwrap();
    let v0 = a.data_version().unwrap();

    // 本连接自身写入：不改自己的 data_version。
    let pid = a.upsert_project_by_root("/p", "p", 1).unwrap();
    assert_eq!(
        a.data_version().unwrap(),
        v0,
        "本连接自身写入不应改变自己的 data_version"
    );

    // 纯读：不改 data_version（app 读库不该触发刷新的核心保证）。
    let _ = a.live_sessions(None, None, None, None, 10).unwrap();
    assert_eq!(a.data_version().unwrap(), v0, "纯读不应改变 data_version");

    // 别的连接提交写入：本连接再读即变化。
    let b = Store::open(&path).unwrap();
    b.start_session(pid, "s", 1).unwrap();
    assert_ne!(
        a.data_version().unwrap(),
        v0,
        "别的连接提交写入后 data_version 应变化"
    );

    drop(a);
    drop(b);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

// == 迁移门控 + cwd 回填 ==

/// 复现真实事故：profile 列的 ALTER 曾加进迁移列表却没 bump USER_VERSION，已升到 v6 的
/// 老库被 `version >= USER_VERSION` 早退挡住，列永远补不上——写 profile 报「no such column」，
/// 把 SessionStart 的落库整个中断。此测试钉死：v6 老库 open 后必须能写读 profile。
#[test]
fn migrate_adds_profile_column_to_v6_database() {
    let path = std::env::temp_dir().join(format!("meowo-mig-v6-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    {
        // 手工造一个 v6 时代的 sessions 表：有 provider、没 profile。
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id    INTEGER NOT NULL,
                cc_session_id TEXT NOT NULL UNIQUE,
                status        TEXT NOT NULL,
                started_at    INTEGER NOT NULL,
                last_event_at INTEGER NOT NULL,
                ended_at      INTEGER,
                pid           INTEGER,
                cwd           TEXT,
                archived      INTEGER NOT NULL DEFAULT 0,
                archived_at   INTEGER,
                pending_review TEXT,
                last_ai_text   TEXT,
                last_user_text TEXT,
                provider       TEXT NOT NULL DEFAULT 'claude');
             PRAGMA user_version = 6;",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    let (sid, _) = store.start_session(pid, "s", 1).unwrap();
    store
        .set_session_profile(sid, Some("qh"))
        .expect("v6 老库 open 后 profile 列应已补上");
    assert_eq!(store.session_profile(sid).unwrap().as_deref(), Some("qh"));

    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

/// v9 老库（没有 lineage 两列）open 后必须能写读接续链——与 profile 列同款事故的预防：
/// ALTER 加了但 USER_VERSION 没 bump 的话，老库被门控早退，列永远补不上。
#[test]
fn migrate_adds_lineage_columns_to_v9_database() {
    let path = std::env::temp_dir().join(format!("meowo-mig-v9-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id    INTEGER NOT NULL,
                cc_session_id TEXT NOT NULL UNIQUE,
                status        TEXT NOT NULL,
                started_at    INTEGER NOT NULL,
                last_event_at INTEGER NOT NULL,
                ended_at      INTEGER,
                pid           INTEGER,
                cwd           TEXT,
                archived      INTEGER NOT NULL DEFAULT 0,
                archived_at   INTEGER,
                pending_review TEXT,
                last_ai_text   TEXT,
                last_user_text TEXT,
                provider       TEXT NOT NULL DEFAULT 'claude',
                profile        TEXT,
                launch_args    TEXT);
             PRAGMA user_version = 9;",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    let (a, _) = store.start_session(pid, "old", 1).unwrap();
    let (b, _) = store.start_session(pid, "new", 2).unwrap();
    store
        .set_session_lineage(b, a)
        .expect("v9 老库 open 后 lineage 两列应已补上");
    let header = store.session_header(a).unwrap();
    assert_eq!(header.superseded_by, Some(b));

    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

// == 跨 provider 接续链 ==

#[test]
fn set_session_lineage_writes_both_directions() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (a, _) = store.start_session(pid, "a", 100).unwrap();
    let (b, _) = store.start_session(pid, "b", 200).unwrap();

    store.set_session_lineage(b, a).unwrap();

    let old = store.session_header(a).unwrap();
    let new = store.session_header(b).unwrap();
    assert_eq!(old.superseded_by, Some(b));
    assert_eq!(old.predecessor_id, None);
    assert_eq!(new.predecessor_id, Some(a));
    assert_eq!(new.superseded_by, None);
}

#[test]
fn set_session_lineage_rejects_self_loop_and_fork() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (a, _) = store.start_session(pid, "a", 100).unwrap();
    let (b, _) = store.start_session(pid, "b", 200).unwrap();
    let (c, _) = store.start_session(pid, "c", 300).unwrap();

    assert!(store.set_session_lineage(a, a).is_err(), "自环必须被拒");
    store.set_session_lineage(b, a).unwrap();
    assert!(
        store.set_session_lineage(c, a).is_err(),
        "a 已被 b 接替，c 再认领是分叉"
    );
    // 分叉失败必须原子：c 的 predecessor 不能留下半截写入。
    assert_eq!(store.session_header(c).unwrap().predecessor_id, None);
    // 不存在的前驱同样拒绝。
    assert!(store.set_session_lineage(c, 9999).is_err());
}

#[test]
fn session_lineage_chain_returns_full_chain_from_any_node() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (a, _) = store.start_session(pid, "a", 100).unwrap();
    let (b, _) = store.start_session(pid, "b", 200).unwrap();
    let (c, _) = store.start_session(pid, "c", 300).unwrap();
    store.set_session_lineage(b, a).unwrap();
    store.set_session_lineage(c, b).unwrap();

    for node in [a, b, c] {
        let chain = store.session_lineage_chain(node).unwrap();
        let ids: Vec<i64> = chain.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![a, b, c], "从节点 {node} 查询应得完整有序链");
    }
    // 不在链上的会话返回仅含自身的单段。
    let (d, _) = store.start_session(pid, "d", 400).unwrap();
    let solo = store.session_lineage_chain(d).unwrap();
    assert_eq!(solo.len(), 1);
    assert_eq!(solo[0].id, d);
}

#[test]
fn backfill_session_cwd_only_fills_null_and_keeps_clock() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    // start_session 直接建行即半态会话的形状：cwd 为 NULL。
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.backfill_session_cwd(sid, "C:/work/proj").unwrap();
    assert_eq!(
        store.session_cwd(sid).unwrap().as_deref(),
        Some("C:/work/proj")
    );
    // 回填不是事件：不动 last_event_at（否则会话会被它顶到「刚活跃过」）。
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.last_event_at, 100);
    // 已有值绝不覆盖——cwd 语义是「会话启动时的目录」。
    store.backfill_session_cwd(sid, "C:/other").unwrap();
    assert_eq!(
        store.session_cwd(sid).unwrap().as_deref(),
        Some("C:/work/proj")
    );
}
