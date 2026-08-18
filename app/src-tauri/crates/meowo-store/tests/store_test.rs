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
    // Stop(置 waiting)后的迟到窗内,活动类 touch 是同一回合的迟到尾巴,不得顶回 running
    // ——hook 进程按到达时刻盖章,回合末尾的 PostToolUse 常晚于 Stop 落库(实拍:回合
    // 已结束却永远显示运行中)。
    store.touch_session(sid, 400).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");
    // 窗口之外的活动才算真活动(未经 UserPromptSubmit 的后台回合)。
    store.touch_session(sid, 300 + 6_000).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "running");
}

#[test]
fn turn_open_touch_revives_waiting_immediately() {
    // UserPromptSubmit = 用户开口开新回合,无条件复活,不吃迟到窗——Stop 后 1 秒内
    // 接着发下一句是最常见的操作。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _tid) = store.start_session(pid, "cc-turn", 200).unwrap();
    store
        .set_session_status(sid, meowo_store::SessionStatus::Waiting, 300)
        .unwrap();
    store.touch_session_turn_open(sid, 400).unwrap();
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
    // reaper 写的 ended 保留 pid 作墨迹:误 reap 时后续活动事件靠它即刻自愈
    // (revive_if_ended_running 的 pid IS NOT NULL 判据),正常 SessionEnd 才清 pid。
    assert_eq!(store.session_pid(sid).unwrap(), Some(222));
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
        .set_session_status(sid, SessionStatus::Running, 1_050)
        .unwrap();
    store.set_last_ai_text(sid, "旧回复", 1_050).unwrap();
    store
        .set_pending_review(sid, PendingReview::Approval, 1_050)
        .unwrap();
    // 注意 end_session **不在**此列：SessionEnd 是进程亲口报的终局、唯一无自愈的转换,
    // 刻意不吃时间戳守卫（见 end_session 文档与 late_session_end_still_ends_the_session）。
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
fn end_orphaned_idle_only_reaps_pidless_idle_unmanaged_sessions() {
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
    // s4：无 pid、空闲超阈值,但正被本 GUI 托管 PTY(codex 的 hook 到首个 turn 才认领 pid)
    // → 进程是 meowo 自己 spawn 的、必然活着,绝不能按孤儿收尾(此前误收正在托管的 codex)。
    let (s4, _) = store.start_session(pid, "managed-pidless", 1000).unwrap();

    // now=10000, idle阈值=2000。
    let managed: std::collections::HashSet<i64> = [s4].into_iter().collect();
    let n = store.end_orphaned_idle(2000, 10000, &managed).unwrap();
    assert_eq!(n, 1);
    assert_eq!(store.get_session(s1).unwrap().status, "ended");
    assert_eq!(store.get_session(s2).unwrap().status, "running");
    assert_ne!(store.get_session(s3).unwrap().status, "ended"); // 带 pid 不受空闲超时影响
    assert_ne!(store.get_session(s4).unwrap().status, "ended"); // 托管 PTY 与判活口径一致

    // 空排除集（AppState 未就绪的降级路径）不炸、语义同旧版。
    let none = std::collections::HashSet::new();
    let n2 = store.end_orphaned_idle(2000, 10000, &none).unwrap();
    assert_eq!(n2, 1, "失去托管豁免后 s4 按孤儿收尾");
    assert_eq!(store.get_session(s4).unwrap().status, "ended");
}

// == 审计修复测试 ==

#[test]
fn session_start_revives_ended_session() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _t) = store.start_session(pid, "s", 100).unwrap();
    store.end_session(sid, 200).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "ended");
    // resume：同 session_id 再次 SessionStart 应复活且清空 ended_at。复活的目标态是
    // waiting——CLI 停在输入框没有回合在跑,running 留给真实活动翻转(实拍反馈:
    // 恢复会话被标成运行中但并不真实)。
    let (sid2, _t2) = store.start_session(pid, "s", 300).unwrap();
    assert_eq!(sid2, sid);
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "waiting");
    assert_eq!(s.ended_at, None);
    // 用户开口(UserPromptSubmit → turn_open)才翻 running。
    store.touch_session_turn_open(sid, 400).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "running");
}

#[test]
fn sync_todos_rebuild_does_not_revive_or_touch_session() {
    // GUI 打开会话时的被动重建(refresh_session_todos)不是会话活动:不得把刚恢复成
    // waiting 的会话顶回 running,也不得伪造 last_event_at(那是连接宽限的燃料)。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _t) = store.start_session(pid, "s", 100).unwrap();
    store.end_session(sid, 200).unwrap();
    assert!(store.revive_for_resume(sid, 300, None).unwrap()); // resume → waiting
    store
        .sync_todos_rebuild(
            sid,
            &[TodoInput { content: "a".into(), status: TodoStatus::Pending }],
            400,
        )
        .unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "waiting"); // 没被顶回 running
    assert_eq!(s.last_event_at, 300); // 活跃时刻没被伪造
    // hook 版(真实活动,迟到窗之外)仍照旧翻转。
    store
        .sync_todos(
            sid,
            &[TodoInput { content: "a".into(), status: TodoStatus::Completed }],
            300 + 6_000,
        )
        .unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "running");
}

#[test]
fn session_start_keeps_running_session_running() {
    // auto-compact 会在回合中途发 SessionStart:此时会话真的在跑,幂等分支不得把它
    // 降级成 waiting(只有 ended 才复活为 waiting)。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _t) = store.start_session(pid, "s", 100).unwrap(); // 新插入默认 running
    let (sid2, _t2) = store.start_session(pid, "s", 200).unwrap();
    assert_eq!(sid2, sid);
    assert_eq!(store.get_session(sid).unwrap().status, "running");
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

    // 同毫秒的清除是上一回合尾巴的指纹,不得抹掉刚置位的审批(严格 < 守卫)。
    store.clear_pending_review(sid, 500).unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "cc1")
        .unwrap();
    assert_eq!(s.pending_review.as_deref(), Some("approval"), "同毫秒交错不得清掉真实审批");

    // 更晚的清除:置 NULL,且不改 last_event_at。
    store.clear_pending_review(sid, 501).unwrap();
    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "cc1")
        .unwrap();
    assert_eq!(s.pending_review, None);
    assert_eq!(s.session.last_event_at, 500);
}

/// M5 定向清除：PostToolUse 只证明「该工具跑完了」,只能注销与它对应的那类等待,
/// 不得顺手抹掉别的事件刚挂上的其它等待。
#[test]
fn kind_scoped_clear_only_clears_matching_pending() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc-kind", 100).unwrap();

    store
        .set_pending_review(sid, PendingReview::Question, 500)
        .unwrap();
    // 普通工具的 PostToolUse(approval 类)不清 question。
    store
        .clear_pending_review_kind(sid, PendingReview::Approval, 600)
        .unwrap();
    assert_eq!(
        store.session_pending_review(sid).unwrap().as_deref(),
        Some("question")
    );
    // 同毫秒的同类清除也挡（严格 < 守卫）。
    store
        .clear_pending_review_kind(sid, PendingReview::Question, 500)
        .unwrap();
    assert_eq!(
        store.session_pending_review(sid).unwrap().as_deref(),
        Some("question")
    );
    // 对应工具(AskUserQuestion)跑完:定向清除生效。
    store
        .clear_pending_review_kind(sid, PendingReview::Question, 600)
        .unwrap();
    assert_eq!(store.session_pending_review(sid).unwrap(), None);
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

    // ended 行上的 set_pending_review 必须被拒（M1）：PermissionRequest 的迟到尾巴写在
    // 尸体上,卡片看不见、通知全哑,且没有任何生命周期事件会再清它——永久泄漏。
    store
        .set_pending_review(sid, PendingReview::Plan, 250)
        .unwrap();
    assert_eq!(
        store.session_pending_review(sid).unwrap(),
        None,
        "已结束会话不得再挂 pending_review"
    );
    assert!(store.revive_for_resume(sid, 300, None).unwrap());
    let resumed = store
        .live_sessions(None, None, None, None, 100)
        .unwrap()
        .into_iter()
        .find(|l| l.session.id == sid)
        .unwrap();
    // 复活的目标态是 waiting:resume 后 CLI 停在输入框,没有回合在跑。
    assert_eq!(resumed.session.status, "waiting");
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

// == 并发/竞态修复回归（hook 按到达时刻盖章、straggler 交错一族） ==

/// M2:/clear 换代与新会话认领同毫秒落库(同 δ)——裸时间戳 `<` 曾放过等时刻的旧行,
/// 留下永不消失的幽灵活卡(无任何自愈路径)。驱逐按会话身份(更早创建的代次)判定。
#[test]
fn set_pid_evicts_same_millisecond_old_generation() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (old, _) = store.start_session(pid, "gen-old", 100).unwrap();
    store.set_session_pid(old, 7777, 500).unwrap();
    let (new, _) = store.start_session(pid, "gen-new", 500).unwrap();
    store.set_session_pid(new, 7777, 500).unwrap(); // 同一毫秒认领

    assert_eq!(store.get_session(old).unwrap().status, "ended");
    assert_eq!(store.session_pid(old).unwrap(), None);
    assert_ne!(store.get_session(new).unwrap().status, "ended");
    assert_eq!(store.session_pid(new).unwrap(), Some(7777));
}

/// M2:认领因迟到守卫落空(本行 last_event_at 已被别的 hook 顶到未来)时,驱逐不得跳过——
/// pid 归属事实不随时间戳变,旧代次留着就是幽灵。旧代码在 claim==0 时直接 return。
#[test]
fn eviction_survives_failed_claim() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (old, _) = store.start_session(pid, "old", 100).unwrap();
    store.set_session_pid(old, 8888, 200).unwrap();
    let (new, _) = store.start_session(pid, "new", 300).unwrap();
    store.touch_session_turn_open(new, 400).unwrap(); // 别的 hook 先把新行时钟顶到 400
    store.set_session_pid(new, 8888, 350).unwrap(); // 迟到认领:claim 0 行

    // 认领没推进时间戳(仍 400),但旧代次照样驱逐。
    assert_eq!(store.get_session(new).unwrap().last_event_at, 400);
    assert_eq!(store.get_session(old).unwrap().status, "ended");
    assert_eq!(store.session_pid(old).unwrap(), None);
}

/// M2:SessionEnd 是进程亲口报的终局、唯一无自愈的转换——straggler 把 last_event_at 顶到
/// 未来后,SessionEnd 的 now_ms 较小,旧的 `last_event_at <=` 守卫会把整次收尾静默丢掉。
#[test]
fn late_session_end_still_ends_the_session() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.touch_session(sid, 900).unwrap(); // straggler 已把时钟顶到 900
    store.end_session(sid, 800).unwrap(); // SessionEnd 带着较早的到达章

    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(s.ended_at, Some(800));
    assert_eq!(s.last_event_at, 900, "MAX 防时钟倒退");
}

/// M3:正常 SessionEnd(pid 已清)后的迟到尾巴不复活——曾把会话诈尸成 running 且 pid NULL,
/// 只能等 end_orphaned_idle 的 120s 兜底再收一次。
#[test]
fn straggler_activity_does_not_revive_a_cleanly_ended_session() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.end_session(sid, 200).unwrap();

    store.revive_if_ended_running(sid, 202).unwrap(); // SessionEnd 后 2ms 的排队尾巴
    assert_eq!(store.get_session(sid).unwrap().status, "ended", "尾巴不得诈尸");

    // 迟到窗之外的活动 = 真实后续使用(如 orphan 误收后用户继续用),照常自愈。
    store.revive_if_ended_running(sid, 200 + 6_000).unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "running");
    assert_eq!(s.ended_at, None);
}

/// M3:reaper 误 reap(end_session_if_pid 留 pid 墨迹)后,活动事件**即刻**自愈,
/// 不受迟到窗限制——误 reap 的会话正在干活,每一秒的 ended 都是撒谎。
#[test]
fn activity_revives_reaper_ended_session_immediately() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store.set_session_pid(sid, 999, 110).unwrap();
    assert!(store.end_session_if_pid(sid, 999, 110, 150).unwrap());

    store.revive_if_ended_running(sid, 151).unwrap(); // 1ms 后的活动事件
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "running");
    assert_eq!(s.ended_at, None);
}

/// M7:迟到窗内的活动尾巴连 last_event_at 都不推进——它是「等待你回复」通知的去重指纹,
/// 尾巴推进它会让同一回合弹两次;且窗口锚点若被尾巴逐条拖长,waiting 恢复会被无限顺延。
#[test]
fn straggler_touch_freezes_last_event_at_and_window_anchor() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store
        .set_session_status(sid, SessionStatus::Waiting, 300)
        .unwrap();

    store.touch_session(sid, 400).unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "waiting");
    assert_eq!(s.last_event_at, 300, "尾巴不得推进指纹时间戳");

    store.touch_session(sid, 4_000).unwrap(); // 仍在 Stop(300) 的迟到窗内
    assert_eq!(store.get_session(sid).unwrap().last_event_at, 300);

    // 锚点未被拖长:相对 300 已出窗,真活动照常翻 running 并推进时钟。
    store.touch_session(sid, 5_400).unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "running");
    assert_eq!(s.last_event_at, 5_400);
}

/// M8:resume 失败回滚的 pid CAS——复活与回滚之间 hook 已认领 pid(会话真活了),
/// 回滚绝不能误杀;没人认领才允许收尾。
#[test]
fn rollback_end_only_hits_unclaimed_sessions() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();

    // 情形 A:复活后 hook 已认领 → 回滚拒绝。
    let (claimed, _) = store.start_session(pid, "claimed", 100).unwrap();
    store.end_session(claimed, 200).unwrap();
    assert!(store.revive_for_resume(claimed, 300, None).unwrap());
    store.set_session_pid(claimed, 4242, 350).unwrap();
    assert!(!store.end_session_if_unclaimed(claimed, 400).unwrap());
    assert_ne!(store.get_session(claimed).unwrap().status, "ended");
    assert_eq!(store.session_pid(claimed).unwrap(), Some(4242));

    // 情形 B:spawn 失败、没人认领 → 收尾回「已断开」。
    let (bare, _) = store.start_session(pid, "bare", 100).unwrap();
    store.end_session(bare, 200).unwrap();
    assert!(store.revive_for_resume(bare, 300, None).unwrap());
    assert!(store.end_session_if_unclaimed(bare, 400).unwrap());
    assert_eq!(store.get_session(bare).unwrap().status, "ended");
}

/// M9:/clear 换代收尾单事务——旧段结束 + launch_args 随进程复制 + 接续链成对写入,
/// 要么全落、要么全不落;分叉被拒时一行未写,不留半态。
#[test]
fn supersede_session_is_atomic() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (a, _) = store.start_session(pid, "a", 100).unwrap();
    store
        .set_session_launch_args(a, r#"{"permission":"bypassPermissions"}"#)
        .unwrap();
    let (b, _) = store.start_session(pid, "b", 200).unwrap();

    store.supersede_session(a, b, 300).unwrap();
    let old = store.get_session(a).unwrap();
    assert_eq!(old.status, "ended");
    assert_eq!(old.ended_at, Some(300));
    assert_eq!(store.session_header(a).unwrap().superseded_by, Some(b));
    assert_eq!(store.session_header(b).unwrap().predecessor_id, Some(a));
    assert_eq!(
        store.session_launch_args(b).unwrap().as_deref(),
        Some(r#"{"permission":"bypassPermissions"}"#),
        "启动选项跟着终端进程走"
    );

    // 失败路径:a 已被 b 接替,c 再换代是分叉 → 整体拒绝,c 一行未写(含 launch_args)。
    let (c, _) = store.start_session(pid, "c", 400).unwrap();
    assert!(store.supersede_session(a, c, 500).is_err());
    assert_eq!(store.session_header(c).unwrap().predecessor_id, None);
    assert_eq!(store.session_launch_args(c).unwrap(), None, "回滚必须连 launch_args 复制一起撤销");
    assert_eq!(store.session_header(a).unwrap().superseded_by, Some(b));
    // 自环拒绝。
    assert!(store.supersede_session(c, c, 600).is_err());
}

/// M10:被动重建(GUI 读侧)不推进 tasks.updated_at——它是 sync_todos 挡迟到快照的权威
/// 时钟,重建顶到 now 会让同期在途的真实 TodoWrite 被整份丢弃(界面永远停在旧状态)。
#[test]
fn rebuild_does_not_advance_todo_clock_nor_block_inflight_snapshots() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "s", 100).unwrap();
    let snap = |status: TodoStatus| {
        [TodoInput {
            content: "任务".into(),
            status,
        }]
    };

    store.sync_todos(sid, &snap(TodoStatus::Pending), 300).unwrap();
    assert_eq!(store.get_task(tid).unwrap().updated_at, 300);

    // GUI 打开会话触发重建(墙钟 900,远超在途 hook 的到达章)。
    store
        .sync_todos_rebuild(sid, &snap(TodoStatus::Pending), 900)
        .unwrap();
    assert_eq!(
        store.get_task(tid).unwrap().updated_at,
        300,
        "读侧重建不得写权威时钟"
    );

    // 在途的真实 TodoWrite(盖章 400)必须落库——重建若顶过时钟,这份就被丢了。
    store
        .sync_todos(sid, &snap(TodoStatus::Completed), 400)
        .unwrap();
    let todos = store.list_todos(tid).unwrap();
    assert_eq!(todos[0].status, "completed", "在途快照不得被重建挡掉");
    assert_eq!(store.get_task(tid).unwrap().updated_at, 400);
}

/// M6:stale 态废除。写侧一律归一成 waiting;v10 老库的存量 stale 行由迁移归一。
#[test]
fn deprecated_stale_status_is_normalized_to_waiting() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "s", 100).unwrap();
    store
        .set_session_status(sid, SessionStatus::Stale, 200)
        .unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");
}

/// M6 迁移:v10 老库残留的 stale 行(无写端、无消费端的半死态)open 时归一成 waiting,
/// 交给 reaper / end_orphaned_idle 按 waiting 口径接管。
#[test]
fn migrate_normalizes_stale_rows_in_v10_database() {
    let path = std::env::temp_dir().join(format!("meowo-mig-v10-{}.db", std::process::id()));
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
                launch_args    TEXT,
                predecessor_id INTEGER,
                superseded_by  INTEGER);
             INSERT INTO sessions (project_id, cc_session_id, status, started_at, last_event_at)
             VALUES (1, 's-stale', 'stale', 100, 100);
             INSERT INTO sessions (project_id, cc_session_id, status, started_at, last_event_at)
             VALUES (1, 's-running', 'running', 200, 200);
             PRAGMA user_version = 10;",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let stale = store.find_session_id_pub("s-stale").unwrap().unwrap();
    assert_eq!(store.get_session(stale).unwrap().status, "waiting");
    // 其它状态不受迁移影响。
    let running = store.find_session_id_pub("s-running").unwrap().unwrap();
    assert_eq!(store.get_session(running).unwrap().status, "running");

    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
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
