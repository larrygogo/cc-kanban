use meowo_store::{SessionStatus, Store};

// ===== Task 1: live_sessions =====

#[test]
fn live_sessions_includes_ended_sessions() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();

    let (s1, _) = store.start_session(pid, "r", 100).unwrap();
    store.on_user_prompt(s1, "活的", 110).unwrap();
    let (s2, _) = store.start_session(pid, "w", 200).unwrap();
    store
        .set_session_status(s2, SessionStatus::Waiting, 210)
        .unwrap();
    let (s3, _) = store.start_session(pid, "st", 300).unwrap();
    store
        .set_session_status(s3, SessionStatus::Stale, 310)
        .unwrap();
    let (s4, _) = store.start_session(pid, "e", 400).unwrap();
    store.end_session(s4, 410).unwrap();

    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    // 四个都在（ended 也保留）
    assert_eq!(live.len(), 4);
    let statuses: Vec<&str> = live.iter().map(|l| l.session.status.as_str()).collect();
    assert!(statuses.contains(&"running"));
    assert!(statuses.contains(&"waiting"));
    assert!(statuses.contains(&"stale"));
    assert!(statuses.contains(&"ended"));
}

#[test]
fn live_session_carries_project_name_title_and_progress() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "proj", 100).unwrap();
    let (s1, _t1) = store.start_session(pid, "r", 100).unwrap();
    store.on_user_prompt(s1, "实现登录", 110).unwrap();
    store
        .sync_todos(
            s1,
            &[
                meowo_store::TodoInput {
                    content: "a".into(),
                    status: meowo_store::TodoStatus::Completed,
                },
                meowo_store::TodoInput {
                    content: "b".into(),
                    status: meowo_store::TodoStatus::InProgress,
                },
            ],
            120,
        )
        .unwrap();

    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert_eq!(live.len(), 1);
    let l = &live[0];
    assert_eq!(l.project_name, "proj");
    assert_eq!(l.task_title, "实现登录");
    assert_eq!(l.column, "doing");
    assert_eq!(l.todo_total, 2);
    assert_eq!(l.todo_done, 1);
    assert_eq!(l.todos.len(), 2);
    assert_eq!(l.todos[0].content, "a");
}

#[test]
fn session_note_upsert_delete_and_surfaces_in_live() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (s1, _) = store.start_session(pid, "sess-a", 100).unwrap();
    store.on_user_prompt(s1, "标题", 110).unwrap();

    // 初始无便签
    assert_eq!(
        store.live_sessions(None, None, None, None, 1000).unwrap()[0].note,
        None
    );

    // 写入便签 → live_sessions 带出（前后空白被 trim）
    store
        .set_session_note("sess-a", "  记得 review  ", 120)
        .unwrap();
    assert_eq!(
        store.live_sessions(None, None, None, None, 1000).unwrap()[0]
            .note
            .as_deref(),
        Some("记得 review")
    );

    // upsert 覆盖旧便签
    store.set_session_note("sess-a", "改主意了", 130).unwrap();
    assert_eq!(
        store.live_sessions(None, None, None, None, 1000).unwrap()[0]
            .note
            .as_deref(),
        Some("改主意了")
    );

    // 清空（trim 后为空）→ 删除该行，回到 None
    store.set_session_note("sess-a", "   ", 140).unwrap();
    assert_eq!(
        store.live_sessions(None, None, None, None, 1000).unwrap()[0].note,
        None
    );
}

// ===== Task 2: 过滤未命名空卡 =====

#[test]
fn live_sessions_returns_new_columns_as_none_by_default() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();
    let _ = sid;

    let live = store.live_sessions(None, None, None, None, 1000).unwrap();
    let s = live
        .iter()
        .find(|l| l.session.cc_session_id == "cc1")
        .unwrap();
    assert_eq!(s.pending_review, None);
    assert_eq!(s.last_ai_text, None);
    assert_eq!(s.last_user_text, None);
}

#[test]
fn live_sessions_paginates_without_ended_cap() {
    // 分页模式下不再对已结束会话做 100 条兜底截断；
    // 旧但仍在 running 的会话会按 last_event_at 排序，可能落到后续页，但全量查询时一定存在。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    // 一个很旧但仍 running 的会话（挂在后台终端里数天未发消息的合法长连接）。
    let (old_running, _) = store.start_session(pid, "old-running", 100).unwrap();
    store.set_session_pid(old_running, 4242, 110).unwrap();
    // 120 条更近活跃的已结束会话。
    for i in 0..120 {
        let (s, _) = store
            .start_session(pid, &format!("ended-{i}"), 1000 + i)
            .unwrap();
        store.end_session(s, 2000 + i).unwrap();
    }

    // 第 0 页 100 条全是更近活跃的已结束会话，旧 running 不在其中。
    let first_page = store
        .live_sessions(Some("all"), None, None, None, 100)
        .unwrap();
    assert_eq!(first_page.len(), 100);
    assert!(
        !first_page
            .iter()
            .any(|s| s.session.cc_session_id == "old-running"),
        "旧 running 不应出现在全为已结束会话的首页"
    );

    // 用首页最后一条 cursor 取第二页：剩余 20 条已结束 + 1 条旧 running。
    let last = first_page.last().unwrap();
    let second_page = store
        .live_sessions(
            Some("all"),
            None,
            Some(last.session.last_event_at),
            Some(last.session.id),
            100,
        )
        .unwrap();
    assert_eq!(second_page.len(), 21);
    assert!(
        second_page
            .iter()
            .any(|s| s.session.cc_session_id == "old-running"),
        "cursor 分页应把旧 running 带到第二页"
    );

    // 全量拉取时旧 running 仍存在，且已结束会话不再受 100 条限制。
    let all = store.live_sessions(None, None, None, None, 1000).unwrap();
    assert!(
        all.iter().any(|s| s.session.cc_session_id == "old-running"),
        "连接中的旧会话在全量结果中必须存在"
    );
    let ended = all.iter().filter(|s| s.session.status == "ended").count();
    assert_eq!(ended, 120);
}

#[test]
fn live_sessions_cursor_tie_respects_filter() {
    // 回归（审查发现）：游标条件 `(last_event_at < ts) OR (last_event_at = ts AND id < id)`
    // 若不整体加括号，与 filter 条件用 AND 拼接时，SQL 的 AND 优先级高于 OR，会让第二个
    // OR 分支绕过 filter（如 archived=0），把不该出现的行混进分页结果。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();

    // a 与 b 的 last_event_at 相同（游标边界 tie）；a 先建 id 更小，且被归档，b 不归档。
    let (a, _) = store.start_session(pid, "a1", 100).unwrap();
    let (b, _) = store.start_session(pid, "b1", 100).unwrap();
    store.set_session_archived(a, true, 200).unwrap();

    // 以 b 的 (last_event_at, id) 为游标：a 的 last_event_at 与游标相等且 id 更小，
    // 命中游标条件的第二分支；但 a 已归档，filter="all" 应把它排除，不能因游标 tie 而绕过。
    let page = store
        .live_sessions(Some("all"), None, Some(100), Some(b), 100)
        .unwrap();
    assert!(
        page.iter().all(|l| l.session.id != a),
        "归档会话不应因游标边界 tie 绕过 filter 混入分页结果"
    );
}

/// 角标的分工：`total`/`archived` 由 SQL 数；running/waiting 只供**原料**（候选行），
/// 因为它们的语义含「此刻还连着」——那要查进程表，store 层看不见（判定在 app 层的 tab_class）。
/// 两边都必须与列表口径对齐：列表会丢弃 ping 与「已结束的空壳」，角标不剔除就会数出
/// 用户在列表里找不到的会话。
#[test]
fn live_counts_split_totals_from_connectivity_candidates() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();

    // running ×2
    let (r1, _) = store.start_session(pid, "r1", 100).unwrap();
    store.set_session_title(r1, "任务 r1", 100).unwrap();
    store
        .set_session_status(r1, SessionStatus::Running, 110)
        .unwrap();
    let (r2, _) = store.start_session(pid, "r2", 200).unwrap();
    store.set_session_title(r2, "任务 r2", 200).unwrap();
    store
        .set_session_status(r2, SessionStatus::Running, 210)
        .unwrap();

    // waiting ×1（status=waiting）
    let (w1, _) = store.start_session(pid, "w1", 300).unwrap();
    store.set_session_title(w1, "任务 w1", 300).unwrap();
    store
        .set_session_status(w1, SessionStatus::Waiting, 310)
        .unwrap();

    // pending_review ×1：status 仍是 running，但够格进 waiting
    let (p1, _) = store.start_session(pid, "p1", 320).unwrap();
    store.set_session_title(p1, "任务 p1", 320).unwrap();
    store
        .set_session_status(p1, SessionStatus::Running, 330)
        .unwrap();
    store
        .set_pending_review(p1, meowo_store::PendingReview::Question, 340)
        .unwrap();

    // ended ×3，其中 1 条归档
    let mut archived_id = None;
    for i in 0..3 {
        let (s, _) = store
            .start_session(pid, &format!("ended-{i}"), 400 + i)
            .unwrap();
        store
            .set_session_title(s, &format!("任务 ended-{i}"), 400 + i)
            .unwrap();
        store.end_session(s, 500 + i).unwrap();
        if i == 0 {
            archived_id = Some(s);
        }
    }
    store
        .set_session_archived(archived_id.unwrap(), true, 600)
        .unwrap();

    // 模拟旧版本数据库留下的「已结束、却还留着 pending_review」历史残留。
    let (dead, _) = store.start_session(pid, "dead-with-residue", 700).unwrap();
    store.set_session_title(dead, "任务 dead", 700).unwrap();
    store.end_session(dead, 720).unwrap();
    store
        .set_pending_review(dead, meowo_store::PendingReview::Approval, 730)
        .unwrap();

    // 列表口径外的两条：ping 探针（哪怕 running）与已结束的未命名空壳。
    // 它们都不会出现在列表里，totals / candidates 也必须数不到。
    let (ping, _) = store.start_session(pid, "ping-probe", 800).unwrap();
    store.set_session_title(ping, "ping", 800).unwrap();
    store
        .set_session_status(ping, SessionStatus::Running, 810)
        .unwrap();
    let (shell, _) = store.start_session(pid, "empty-shell", 820).unwrap();
    store.end_session(shell, 830).unwrap();

    let (total, archived) = store.live_sessions_totals().unwrap();
    assert_eq!(total, 8, "ping 与已结束空壳不该进 total");
    assert_eq!(archived, 1);

    // 候选 = 未归档 且 **未结束** 且（status∈{running,waiting} 或 有 pending_review）。
    // ping 探针虽是 running，也不许进候选（列表里根本看不到它）。
    let cands = store.live_count_candidates().unwrap();
    assert_eq!(cands.len(), 4, "2 running + 1 waiting + 1 pending_review");
    assert_eq!(
        cands.iter().filter(|c| c.status == "running").count(),
        3,
        "含带 pending 的那条"
    );
    assert_eq!(cands.iter().filter(|c| c.status == "waiting").count(), 1);
    assert!(
        cands.iter().all(|c| c.status != "ended"),
        "已结束的不该成为候选"
    );

    // 关键：那条「已结束 + pending_review 残留」绝不能混进候选。它算不进 running/waiting
    // （session_connected 对 ended 恒 false），却会让候选集合随历史无限膨胀、白白判活。
    assert_eq!(
        cands.iter().filter(|c| c.pending_review.is_some()).count(),
        1,
        "只该有活着的那条 pending，已结束的残留不算"
    );
}

#[test]
fn live_sessions_cursor_loads_all_non_archived() {
    // 验证 cursor 分页不会漏掉任何非归档会话。
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();

    // 250 条非归档会话，last_event_at 从 1000 递增到 3499。
    // 都给标题：未命名的 ended 会话会被 totals 按「空壳」剔除（对齐列表口径），
    // 本测试关心的是游标翻页完整性，不掺空壳因素。
    for i in 0..250 {
        let (s, _) = store
            .start_session(pid, &format!("s-{i}"), 1000 + i)
            .unwrap();
        store
            .set_session_title(s, &format!("任务 {i}"), 1000 + i)
            .unwrap();
        store.end_session(s, 2000 + i).unwrap();
    }
    // 50 条归档会话，last_event_at 更晚（4500..），确保不会被误算进 all
    for i in 0..50 {
        let (s, _) = store
            .start_session(pid, &format!("arch-{i}"), 4500 + i)
            .unwrap();
        store
            .set_session_title(s, &format!("归档 {i}"), 4500 + i)
            .unwrap();
        store.end_session(s, 4600 + i).unwrap();
        store.set_session_archived(s, true, 4700 + i).unwrap();
    }

    let (total, archived) = store.live_sessions_totals().unwrap();
    assert_eq!(total, 300);
    assert_eq!(archived, 50);

    let mut loaded: Vec<i64> = Vec::new();
    let mut cursor: Option<(i64, i64)> = None;
    loop {
        let page = match cursor {
            Some((ts, id)) => store
                .live_sessions(Some("all"), None, Some(ts), Some(id), 80)
                .unwrap(),
            None => store
                .live_sessions(Some("all"), None, None, None, 80)
                .unwrap(),
        };
        if page.is_empty() {
            break;
        }
        loaded.extend(page.iter().map(|s| s.session.id));
        let last = page.last().unwrap();
        cursor = Some((last.session.last_event_at, last.session.id));
        if page.len() < 80 {
            break;
        }
    }

    assert_eq!(loaded.len(), 250, "应加载全部 250 条非归档会话");
    let loaded_set: std::collections::HashSet<_> = loaded.iter().copied().collect();
    assert_eq!(loaded_set.len(), 250, "不应有重复 session");
}

#[test]
fn live_sessions_waiting_includes_pending_review() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();

    let (running, _) = store.start_session(pid, "running", 100).unwrap();
    store
        .set_session_status(running, SessionStatus::Running, 110)
        .unwrap();

    let (waiting, _) = store.start_session(pid, "waiting", 200).unwrap();
    store
        .set_session_status(waiting, SessionStatus::Waiting, 210)
        .unwrap();

    let (pending, _) = store.start_session(pid, "pending", 300).unwrap();
    store
        .set_session_status(pending, SessionStatus::Running, 310)
        .unwrap();
    store
        .set_pending_review(pending, meowo_store::PendingReview::Question, 320)
        .unwrap();

    let waiting_page = store
        .live_sessions(Some("waiting"), None, None, None, 100)
        .unwrap();
    let ids: std::collections::HashSet<i64> = waiting_page.iter().map(|l| l.session.id).collect();
    assert!(ids.contains(&waiting), "status=waiting 应在 waiting 分页");
    assert!(ids.contains(&pending), "pending_review 应在 waiting 分页");
    assert!(!ids.contains(&running), "纯 running 不应在 waiting 分页");
}

#[test]
fn live_sessions_search_scoped_to_tab() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    // running: 标题含 "login"
    let (r, _) = store.start_session(pid, "r", 100).unwrap();
    store.on_user_prompt(r, "实现 login 登录", 110).unwrap();
    store
        .set_session_status(r, SessionStatus::Running, 110)
        .unwrap();
    // waiting: 标题含 "login"
    let (w, _) = store.start_session(pid, "w", 200).unwrap();
    store.on_user_prompt(w, "login 待回复", 210).unwrap();
    store
        .set_session_status(w, SessionStatus::Waiting, 210)
        .unwrap();
    // running: 标题不含 "login"
    let (r2, _) = store.start_session(pid, "r2", 300).unwrap();
    store.on_user_prompt(r2, "别的任务", 310).unwrap();
    store
        .set_session_status(r2, SessionStatus::Running, 310)
        .unwrap();

    // 全部 tab 搜 login：命中 r + w，不含 r2
    let all = store
        .live_sessions(Some("all"), Some("login"), None, None, 100)
        .unwrap();
    assert_eq!(all.len(), 2);
    // running tab 搜 login：只命中 r（w 是 waiting，被 filter 排除）
    let run = store
        .live_sessions(Some("running"), Some("login"), None, None, 100)
        .unwrap();
    assert_eq!(run.len(), 1);
    assert_eq!(run[0].session.cc_session_id, "r");
    // waiting tab 搜 login：只命中 w
    let wait = store
        .live_sessions(Some("waiting"), Some("login"), None, None, 100)
        .unwrap();
    assert_eq!(wait.len(), 1);
    assert_eq!(wait[0].session.cc_session_id, "w");
}

#[test]
fn live_sessions_search_matches_cwd_and_escapes_wildcards() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    let (a, _) = store.start_session(pid, "a", 100).unwrap();
    store.on_user_prompt(a, "无关标题", 110).unwrap();
    store.set_session_cwd(a, "C:/work/my_proj", 120).unwrap();
    let (b, _) = store.start_session(pid, "b", 200).unwrap();
    store.on_user_prompt(b, "无关", 210).unwrap();
    store.set_session_cwd(b, "C:/work/myXproj", 220).unwrap();

    // 搜 cwd 片段命中 a
    let hit = store
        .live_sessions(Some("all"), Some("my_proj"), None, None, 100)
        .unwrap();
    // `_` 被转义为字面下划线：只命中 my_proj，不命中 myXproj
    assert!(hit.iter().any(|l| l.session.cc_session_id == "a"));
    assert!(
        !hit.iter().any(|l| l.session.cc_session_id == "b"),
        "`_` 应作字面量、不作通配"
    );
}

/// 目录筛选是斜杠归一的**精确**匹配,不是 search 的子串 LIKE:历史数据里同一目录
/// 存着两种斜杠写法,任选一种筛选都必须双双命中;兄弟目录(前缀关系)与标题命中
/// 不得漏进目录限定的结果。
#[test]
fn live_sessions_cwd_filter_normalizes_slashes_and_is_exact() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    let (fwd, _) = store.start_session(pid, "fwd", 100).unwrap();
    store.on_user_prompt(fwd, "正斜杠", 110).unwrap();
    store.set_session_cwd(fwd, "C:/work/proj", 120).unwrap();
    let (back, _) = store.start_session(pid, "back", 200).unwrap();
    store.on_user_prompt(back, "反斜杠", 210).unwrap();
    store.set_session_cwd(back, r"C:\work\proj", 220).unwrap();
    // 兄弟目录:名字是筛选目录的前缀延伸,子串语义会误命中。
    let (sibling, _) = store.start_session(pid, "sibling", 300).unwrap();
    store.on_user_prompt(sibling, "兄弟目录", 310).unwrap();
    store.set_session_cwd(sibling, "C:/work/proj2", 320).unwrap();
    // 标题含目录字符串:目录筛选不该被标题命中污染。
    let (titled, _) = store.start_session(pid, "titled", 400).unwrap();
    store.on_user_prompt(titled, "讨论 C:/work/proj 的问题", 410).unwrap();
    store.set_session_cwd(titled, "C:/elsewhere", 420).unwrap();

    for spelling in ["C:/work/proj", r"C:\work\proj", "C:/work/proj/"] {
        let hits = store
            .live_sessions_filtered(Some("all"), None, Some(spelling), None, None, 100)
            .unwrap();
        let ids: Vec<&str> = hits.iter().map(|l| l.session.cc_session_id.as_str()).collect();
        assert!(ids.contains(&"fwd") && ids.contains(&"back"), "写法 {spelling} 应命中两种存法: {ids:?}");
        assert_eq!(hits.len(), 2, "写法 {spelling} 不得混入兄弟目录/标题命中: {ids:?}");
    }
}

#[test]
fn live_sessions_waiting_sorted_ascending_and_paginates() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    // 3 条 waiting，last_event_at 递增：等最久(最小 last_event_at)应先出
    for i in 0..3 {
        let (s, _) = store.start_session(pid, &format!("w{i}"), 100 + i).unwrap();
        store.on_user_prompt(s, &format!("t{i}"), 100 + i).unwrap();
        store
            .set_session_status(s, SessionStatus::Waiting, 100 + i)
            .unwrap();
    }
    let page = store
        .live_sessions(Some("waiting"), None, None, None, 2)
        .unwrap();
    assert_eq!(page.len(), 2);
    // ASC：w0(最久) 在最前
    assert_eq!(page[0].session.cc_session_id, "w0");
    assert_eq!(page[1].session.cc_session_id, "w1");
    // 用末条 cursor 取下一页（ASC 游标方向）
    let last = page.last().unwrap();
    let next = store
        .live_sessions(
            Some("waiting"),
            None,
            Some(last.session.last_event_at),
            Some(last.session.id),
            2,
        )
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].session.cc_session_id, "w2");
}

#[test]
fn live_sessions_search_none_is_backcompat() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    let (s, _) = store.start_session(pid, "s", 100).unwrap();
    store.on_user_prompt(s, "任务", 110).unwrap();
    // search=None 与不搜一致：返回该会话
    let r = store
        .live_sessions(Some("all"), None, None, None, 100)
        .unwrap();
    assert_eq!(r.len(), 1);
    // search=Some("") 空串按不搜处理
    let r2 = store
        .live_sessions(Some("all"), Some("  "), None, None, 100)
        .unwrap();
    assert_eq!(r2.len(), 1);
}

/// 回归（审查发现）：live_sessions 曾把 provider 空串/纯空白原样透出，而兄弟读路径
/// （session_header / session_provider）都有「空值回退 DEFAULT_PROVIDER」的防御——
/// 上层拿到空串会把会话按未知 agent 处理（丢掉 transcript 能力）。
#[test]
fn live_sessions_falls_back_to_default_provider_for_blank_provider() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    // set_session_provider 是原样透写（store 不校验、不归一），用它造出脏 provider 的行。
    let (empty, _) = store.start_session(pid, "empty-provider", 100).unwrap();
    store.set_session_provider(empty, "").unwrap();
    let (blank, _) = store.start_session(pid, "blank-provider", 200).unwrap();
    store.set_session_provider(blank, "   ").unwrap();

    let live = store.live_sessions(None, None, None, None, 100).unwrap();
    let got = |id: i64| {
        live.iter()
            .find(|l| l.session.id == id)
            .map(|l| l.provider.as_str())
    };
    assert_eq!(got(empty), Some(meowo_store::DEFAULT_PROVIDER));
    assert_eq!(got(blank), Some(meowo_store::DEFAULT_PROVIDER));
}

/// 看板把 agent 自建的后台会话整个藏了，总数必须按**同一口径**扣掉——只在
/// running/waiting 候选里扣是不够的：一条已结束但有标题的后台会话不在候选集里，
/// 却照样计进 total，于是角标数出一条用户在列表里找不到的会话。
#[test]
fn hidden_sessions_are_subtracted_with_the_same_rule_as_totals() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/repo", "repo", 100).unwrap();

    let mk = |cc: &str, title: &str, ended: bool, archived: bool, at: i64| {
        let (id, _) = store.start_session(pid, cc, at).unwrap();
        store.set_session_title(id, title, at).unwrap();
        if ended {
            store.end_session(id, at + 1).unwrap();
        }
        if archived {
            store.set_session_archived(id, true, at + 2).unwrap();
        }
        id
    };
    mk("mine", "我的会话", false, false, 100);
    mk("bg-running", "后台在跑", false, false, 200);
    mk("bg-ended", "后台已结束", true, false, 300); // 候选集捞不到它，但它在 total 里
    mk("bg-archived", "后台已归档", true, true, 400);
    // 空壳（已结束 + 未命名）本就不进 total，重复扣会把数字扣穿。
    let (shell, _) = store.start_session(pid, "bg-shell", 500).unwrap();
    store.end_session(shell, 501).unwrap();

    let (total, archived) = store.live_sessions_totals().unwrap();
    assert_eq!(total, 4, "四条有标题的；未命名空壳不算");
    assert_eq!(archived, 1);

    let hidden = ["bg-running", "bg-ended", "bg-archived", "bg-shell"].map(String::from);
    let (hidden_total, hidden_archived) = store.live_totals_for(&hidden).unwrap();
    assert_eq!(hidden_total, 3, "空壳本就不在 total 里，不能重复扣");
    assert_eq!(hidden_archived, 1);
    assert_eq!(total - hidden_total, 1, "藏完只剩用户自己那条");
    assert_eq!(archived - hidden_archived, 0);

    // 一个都不隐藏时不该误伤。
    assert_eq!(store.live_totals_for(&[]).unwrap(), (0, 0));
}
