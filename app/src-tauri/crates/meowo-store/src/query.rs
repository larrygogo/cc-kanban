use crate::error::StoreError;
use crate::models::{Session, Todo};
use crate::store::Store;
use serde::Serialize;
use std::collections::HashMap;

/// 贴纸各分类总数（与前端 tab 一一对应，避免靠已加载数据估算导致闪烁）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct LiveSessionCounts {
    #[cfg_attr(test, ts(type = "number"))]
    pub total: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub running: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub waiting: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub archived: i64,
}

/// 算 running/waiting 角标所需的一行原料。判定「此刻还连着」要查进程表，只有 app 层做得到，
/// 故 store 层不统计、只供料。见 [`Store::live_count_candidates`]。
#[derive(Debug, Clone)]
pub struct LiveCandidate {
    /// 会话 id:app 层用它把「托管 PTY 活跃」并进存活判定(hook 未认领 pid 时 pid 判活为假)。
    pub id: i64,
    pub status: String,
    pub pending_review: Option<String>,
    pub pid: Option<i64>,
    pub last_event_at: i64,
    /// agent 的会话 id 与所属 agent：供 app 层查会话的运行形态(agent 自己落盘的索引)。
    /// 有些会话 SQL 无从判断该不该数——最典型的是 agent 预热出来的待命进程,它在库里
    /// 与常规会话别无二致,只有 agent 自己的花名册知道它还没被派活。
    pub cc_session_id: String,
    pub provider: Option<String>,
    /// 会话工作目录:app 层按它重建 transcript 路径,tab 归属要看「后台子任务还在跑吗」
    /// (transcript 分析),与列表侧 enrich 同一份原料。
    pub cwd: Option<String>,
}

/// 当前活跃区的一张会话卡。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct LiveSession {
    pub session: Session,
    pub project_name: String,
    pub task_title: String,
    pub current_activity: Option<String>,
    pub column: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub todo_done: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub todo_total: i64,
    pub todos: Vec<Todo>,
    #[cfg_attr(test, ts(type = "number | null"))]
    pub pid: Option<i64>,
    pub archived: bool,
    /// 归档时间戳（ms）；未归档为 None。用于「归档超过 N 天自动隐藏」。
    #[cfg_attr(test, ts(type = "number | null"))]
    pub archived_at: Option<i64>,
    /// 会话工作目录，meowo-app 用它重建 transcript 路径以实时解析 AI 标题。
    pub cwd: Option<String>,
    /// 上下文已用百分比（来自 Claude Code statusline，准确）；无 statusline 数据为 None。
    #[cfg_attr(test, ts(type = "number | null"))]
    pub context_pct: Option<i64>,
    /// 上下文窗口大小（200000 或 1000000）；无 statusline 数据为 None。
    #[cfg_attr(test, ts(type = "number | null"))]
    pub context_window: Option<i64>,
    /// 模型展示名（来自 Claude Code statusline 的 model.display_name，如 "Opus"）；无则 None。
    pub model: Option<String>,
    /// 用户给会话挂的便签（手写备忘）；无便签为 None。
    pub note: Option<String>,
    /// 待审批子态：NULL/approval/question/plan(回合中途等用户介入)。
    pub pending_review: Option<String>,
    /// 最近一条 AI 正文(锚 Stop hook 的 last_assistant_message)；无则 None,前端回退 transcript preview。
    pub last_ai_text: Option<String>,
    /// 最近一条用户消息(锚 UserPromptSubmit.prompt)；独立字段,不被工具活动覆盖。
    pub last_user_text: Option<String>,
    /// agent 提供方：claude（默认）/ kimi…，前端据此换图标/标签。
    pub provider: String,
    /// 会话所属的账号（profile id）；None = 默认账号。这里传原始 id，
    /// id → 展示名的解析在 app 层做（它才读得到 settings.json）。
    pub profile: Option<String>,
    /// 跨 provider 接续链：本会话接替的上一段会话 id。Some = 卡片渲染链徽标
    ///（点开可回看链上历史各段）；None = 普通会话。
    #[cfg_attr(test, ts(type = "number | null"))]
    pub predecessor_id: Option<i64>,
    /// 附加目录（一个会话经 --add-dir 访问的其它仓）。空 = 单目录会话。
    /// 卡片按它显示「+N」；JSON 解析在此层做，前端拿到即用。
    pub extra_dirs: Vec<String>,
}

/// 转义 SQL LIKE 通配符，使用户输入里的 `%` `_` `\` 作字面量匹配（配合 `LIKE … ESCAPE '\'`）。
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Store {
    /// 批量取多个 task 的 todos，按 task_id 分组——替代逐 task 调 `list_todos` 的 N+1。
    /// 按固定块大小分批拼 `IN (...)`：单条 `IN` 的占位符数不能超过 SQLite 绑定参数上限
    /// （旧版默认 999），单项目任务很多时一次性塞进去会 `too many SQL variables`。
    /// task_id 唯一（一个 task 只属一个会话），不跨块，故每个分组内仍按 order_idx 有序。
    fn todos_by_task(&self, task_ids: &[i64]) -> Result<HashMap<i64, Vec<Todo>>, StoreError> {
        const CHUNK: usize = 900;
        let mut map: HashMap<i64, Vec<Todo>> = HashMap::new();
        for chunk in task_ids.chunks(CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT id, task_id, content, status, order_idx, stale FROM todos
                 WHERE task_id IN ({placeholders}) ORDER BY task_id, order_idx"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |r| {
                Ok(Todo {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    content: r.get(2)?,
                    status: r.get(3)?,
                    order_idx: r.get(4)?,
                    stale: r.get::<_, i64>(5)? != 0,
                })
            })?;
            for row in rows {
                let todo = row?;
                map.entry(todo.task_id).or_default().push(todo);
            }
        }
        Ok(map)
    }

    /// 活跃区：按 filter（+ 可选 search，作用于当前 tab 内）取会话，附项目名、任务标题、进度。
    /// waiting tab 按 last_event_at ASC（等最久优先）、其它按 DESC；游标方向随排序翻转，cursor 为 null 取首页。
    /// filter: "all" | "running" | "waiting" | "archived"；其它值按 "all"。search 去空白后非空才生效。
    /// running/waiting 返回的是**候选并集**（两者同一条 SQL），最终归属由 app 层 tab_class
    /// 结合进程表/屏幕状态判定后再过滤——见谓词处注释。
    pub fn live_sessions(
        &self,
        filter: Option<&str>,
        search: Option<&str>,
        before_last_event_at: Option<i64>,
        before_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LiveSession>, StoreError> {
        self.live_sessions_filtered(filter, search, None, before_last_event_at, before_id, limit)
    }

    /// [`Self::live_sessions`] 的全参形态,多一个 `cwd` 目录过滤。目录过滤是**精确匹配**,
    /// 不是搜索:同一目录在库里同时存着 `C:/x` 与 `C:\x` 两种写法(历史数据,侧栏分组按
    /// 归一 key 也视作同一目录),双方都归一(反斜杠→斜杠、去尾斜杠、ASCII 不分大小写)后
    /// 比较。若复用 search 的子串 LIKE:另一种写法的会话从「已筛选」列表整批消失,还会把
    /// 兄弟目录(`C:/proj` 命中 `C:/proj2`)和标题/项目名命中漏进本应目录限定的结果里。
    pub fn live_sessions_filtered(
        &self,
        filter: Option<&str>,
        search: Option<&str>,
        cwd: Option<&str>,
        before_last_event_at: Option<i64>,
        before_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LiveSession>, StoreError> {
        use rusqlite::types::Value;
        const SELECT: &str = "SELECT s.id, s.project_id, s.cc_session_id, s.status, s.started_at, s.last_event_at, s.ended_at,
                p.name, t.id, t.title, t.current_activity, t.column_name, s.pid, s.archived, s.cwd, s.archived_at,
                sc.used_pct, sc.window_size, sc.model, sn.note,
                s.pending_review, s.last_ai_text, s.last_user_text, s.provider, s.profile, s.predecessor_id,
                s.extra_dirs
         FROM sessions s
         JOIN projects p ON p.id = s.project_id
         -- 关联子查询取每会话最新一张 task：ux_tasks_session 建立前的旧库可能一会话
         -- 多卡，裸 JOIN t.session_id 会扇出重复行（列表重复卡、计数虚高）。与
         -- task_id_of_session 的「最新卡」口径一致；其余三处 tasks JOIN 同式同义。
         LEFT JOIN tasks t ON t.id = (SELECT MAX(t2.id) FROM tasks t2 WHERE t2.session_id = s.id)
         LEFT JOIN session_context sc ON sc.cc_session_id = s.cc_session_id
         LEFT JOIN session_notes sn ON sn.cc_session_id = s.cc_session_id";

        let mut conditions: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        // superseded_by IS NULL：被跨 provider 切换接替的旧段从所有 tab 折叠隐藏
        // （链尾代表整条链；回看走 get_chat_history 按 id 直取，不受影响）。
        // 该谓词与 live_sessions_totals / live_totals_for / live_count_candidates
        // 四处口径必须同生共死，改任何一处都要同步其余三处。
        //
        // running/waiting 共用一个**候选并集**（与 live_count_candidates 逐字同口径）：
        // 归属哪个 tab 由 app 层的 tab_class 决定——它要看屏幕检测状态（broker 内存里的
        // 实时事实）与审批存活校正，SQL 都看不见。SQL 若按 status 预分 tab，一条
        // status=running 而屏幕已 idle 的会话就会被钉死在运行中 tab，与卡片黄环打架
        //（卡片状态与 tab 归属必须同源，见 session_query.rs 的 tab_class）。
        match filter {
            Some("all") => {
                conditions.push("s.archived = 0 AND s.superseded_by IS NULL".into());
            }
            Some("running") | Some("waiting") => conditions.push(
                "s.status != 'ended' \
                 AND (s.status IN ('running','waiting') OR s.pending_review IS NOT NULL) \
                 AND s.archived = 0 AND s.superseded_by IS NULL"
                    .into(),
            ),
            Some("archived") => {
                conditions.push("s.archived = 1 AND s.superseded_by IS NULL".into());
            }
            _ => {} // None 不过滤（仅测试取证用，生产两处调用都传 Some）
        }

        // 搜索（当前 tab 内 AND 搜索词）：title / cwd / project 名 / 便签 / 最近往来任一命中。
        // %/_/\ 转义成字面量。便签是用户亲手写的记忆锚点、最近往来是「上次聊到哪」的
        // 第一线索——这两处搜不到时用户只能凭记忆逐个点开翻。
        if let Some(q) = search.map(str::trim).filter(|s| !s.is_empty()) {
            let pat = format!("%{}%", escape_like(q));
            conditions.push(
                "(t.title LIKE ? ESCAPE '\\' OR s.cwd LIKE ? ESCAPE '\\' OR p.name LIKE ? ESCAPE '\\' \
                 OR sn.note LIKE ? ESCAPE '\\' OR s.last_user_text LIKE ? ESCAPE '\\' OR s.last_ai_text LIKE ? ESCAPE '\\')".into(),
            );
            for _ in 0..6 {
                params.push(Value::Text(pat.clone()));
            }
        }

        // 目录过滤:斜杠归一 + 去尾斜杠后的精确比较(NOCASE 只管 ASCII,盘符/常见路径够用;
        // CJK 无大小写之分不受影响)。理由见 live_sessions_filtered 文档注释。
        if let Some(dir) = cwd.map(str::trim).filter(|s| !s.is_empty()) {
            conditions.push("RTRIM(REPLACE(s.cwd, '\\', '/'), '/') = ? COLLATE NOCASE".into());
            let normalized = dir.replace('\\', "/");
            params.push(Value::Text(normalized.trim_end_matches('/').to_string()));
        }

        // waiting 等最久优先（ASC），游标取「更大」的；其它 DESC，游标取「更小」的。
        let asc = matches!(filter, Some("waiting"));
        if let (Some(ts), Some(id)) = (before_last_event_at, before_id) {
            // 整体括起：AND 优先级高于 OR，不加括号第二个 OR 分支会绕过 filter（4035ec5 回归）。
            if asc {
                conditions
                    .push("((s.last_event_at > ?) OR (s.last_event_at = ? AND s.id > ?))".into());
            } else {
                conditions
                    .push("((s.last_event_at < ?) OR (s.last_event_at = ? AND s.id < ?))".into());
            }
            params.push(Value::Integer(ts));
            params.push(Value::Integer(ts));
            params.push(Value::Integer(id));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let order = if asc {
            "s.last_event_at ASC, s.id ASC"
        } else {
            "s.last_event_at DESC, s.id DESC"
        };
        let sql = format!("{} {} ORDER BY {} LIMIT ?", SELECT, where_clause, order);
        params.push(Value::Integer(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                let session = Session {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    cc_session_id: r.get(2)?,
                    status: r.get(3)?,
                    started_at: r.get(4)?,
                    last_event_at: r.get(5)?,
                    ended_at: r.get(6)?,
                };
                let project_name: String = r.get(7)?;
                let task_id: Option<i64> = r.get(8)?;
                let task_title: Option<String> = r.get(9)?;
                let current_activity: Option<String> = r.get(10)?;
                let column: Option<String> = r.get(11)?;
                let pid: Option<i64> = r.get(12)?;
                let archived: i64 = r.get(13)?;
                let cwd: Option<String> = r.get(14)?;
                let archived_at: Option<i64> = r.get(15)?;
                let context_pct: Option<i64> = r.get(16)?;
                let context_window: Option<i64> = r.get(17)?;
                let model: Option<String> = r.get(18)?;
                let note: Option<String> = r.get(19)?;
                let pending_review: Option<String> = r.get(20)?;
                let last_ai_text: Option<String> = r.get(21)?;
                let last_user_text: Option<String> = r.get(22)?;
                let provider: Option<String> = r.get(23)?;
                let profile: Option<String> = r.get(24)?;
                let predecessor_id: Option<i64> = r.get(25)?;
                let extra_dirs_json: Option<String> = r.get(26)?;
                Ok((
                    session,
                    project_name,
                    task_id,
                    task_title,
                    current_activity,
                    column,
                    pid,
                    archived,
                    cwd,
                    archived_at,
                    context_pct,
                    context_window,
                    model,
                    note,
                    pending_review,
                    last_ai_text,
                    last_user_text,
                    provider,
                    profile,
                    (predecessor_id, extra_dirs_json),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // todos 批量取回（替代逐会话 list_todos 的 N+1）。
        let task_ids: Vec<i64> = rows.iter().filter_map(|r| r.2).collect();
        let mut todos_map = self.todos_by_task(&task_ids)?;
        let mut out = Vec::with_capacity(rows.len());
        for (
            session,
            project_name,
            task_id,
            task_title,
            current_activity,
            column,
            pid,
            archived,
            cwd,
            archived_at,
            context_pct,
            context_window,
            model,
            note,
            pending_review,
            last_ai_text,
            last_user_text,
            provider,
            profile,
            (predecessor_id, extra_dirs_json),
        ) in rows
        {
            let todos = task_id
                .and_then(|tid| todos_map.remove(&tid))
                .unwrap_or_default();
            // 角标进度只计「当前任务」的行：stale=1 的是上一任务的残留，计入会让
            // 看板在新任务待办写出来之前一直显示旧任务的 3/3（失真）。全 stale 时
            // 归 0/0——没有当前进度可言，角标消失。
            let todo_total = todos.iter().filter(|t| !t.stale).count() as i64;
            let todo_done = todos
                .iter()
                .filter(|t| !t.stale && t.status == "completed")
                .count() as i64;
            out.push(LiveSession {
                session,
                project_name,
                task_title: task_title.unwrap_or_default(),
                current_activity,
                column: column.unwrap_or_else(|| "todo".to_string()),
                todo_done,
                todo_total,
                todos,
                pid,
                archived: archived != 0,
                archived_at,
                cwd,
                context_pct,
                context_window,
                model,
                note,
                pending_review,
                last_ai_text,
                last_user_text,
                // provider 的空值回退与 session_header/session_provider 一致：DB 里可能是
                // NULL 或空串/纯空白，直接透出去会让上层按未知 agent 处理（丢掉 transcript 能力）。
                provider: provider
                    .filter(|p| !p.trim().is_empty())
                    .unwrap_or_else(|| crate::DEFAULT_PROVIDER.to_string()),
                profile,
                predecessor_id,
                // 解析失败按空处理:附加目录是增强信息,坏数据不该让整页列表报错。
                extra_dirs: extra_dirs_json
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// 贴纸角标里**纯 SQL 数得出**的两个总数：`(total, archived)`。
    ///
    /// `running` / `waiting` 不在这里——它们的语义都含「**此刻**还连着」，而「连着」要查进程表
    /// （pid 是否是活着的 agent 进程），SQL 看不见。硬用 SQL 数会把进程早已死掉的会话也算进
    /// 「待交互」，角标催着用户去交互、点进去却是个断开的历史会话；更糟的是列表那边（app 层）
    /// 按 connected 过滤后只剩 2 条，角标却写着 3，两个数字当场打架。
    ///
    /// 故这两类改由 app 层用 [`Self::live_count_candidates`] 的原料算——与列表**同一套判定、
    /// 同一份进程表快照**，数字必然自洽。
    ///
    /// total / archived 同样要对齐列表口径：列表（enrich）会丢弃 ping 会话和「已断开的
    /// 空壳」（未命名且无 todo），这里不剔除的话角标恒大于列表条数，用户会以为会话丢了
    /// （原始症状：角标 60、列表 11）。「断开」SQL 看不见，但 `status = 'ended'` 必判断开
    /// （`session_connected` 对 ended 恒 false），空壳几乎全是 ended（探针/崩溃残留）；
    /// 残余误差只剩两类小头：非 ended 的断开空壳（多算）与 transcript 能补出标题的
    /// 未命名会话（少算），方向相反、量级都小。
    pub fn live_sessions_totals(&self) -> Result<(i64, i64), StoreError> {
        const SQL: &str = "SELECT COUNT(*), COALESCE(SUM(s.archived = 1), 0)
             FROM sessions s
             LEFT JOIN tasks t ON t.id = (SELECT MAX(t2.id) FROM tasks t2 WHERE t2.session_id = s.id)
             WHERE s.superseded_by IS NULL AND NOT (
                 LOWER(TRIM(COALESCE(t.title, ''))) = 'ping'
                 OR (s.status = 'ended'
                     AND TRIM(COALESCE(t.title, '')) IN ('', '(未命名会话)')
                     AND NOT EXISTS (SELECT 1 FROM todos td WHERE td.task_id = t.id)))";
        let (total, archived) = self
            .conn
            .query_row(SQL, [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((total, archived))
    }

    /// 在 [`Self::live_sessions_totals`] 的**同一口径**下，这批 agent 会话各占了多少
    /// total / archived。
    ///
    /// 给列表那边隐藏掉的会话做减法用。隐藏与否只有 app 层知道（要查 agent 自己落盘的
    /// 索引），SQL 无从判断，所以由调用方把 id 报进来。不这么做的话，一条已结束但有标题的
    /// 后台会话仍计进 total、列表里却看不到——正是「角标 60、列表 11」那类打架。
    pub fn live_totals_for(&self, cc_session_ids: &[String]) -> Result<(i64, i64), StoreError> {
        const CHUNK: usize = 900;
        let (mut total, mut archived) = (0i64, 0i64);
        for chunk in cc_session_ids.chunks(CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            // WHERE 子句与 live_sessions_totals 逐字相同——两处口径必须同生共死。
            let sql = format!(
                "SELECT COUNT(*), COALESCE(SUM(s.archived = 1), 0)
                 FROM sessions s
                 LEFT JOIN tasks t ON t.id = (SELECT MAX(t2.id) FROM tasks t2 WHERE t2.session_id = s.id)
                 WHERE s.cc_session_id IN ({placeholders})
                   AND s.superseded_by IS NULL AND NOT (
                     LOWER(TRIM(COALESCE(t.title, ''))) = 'ping'
                     OR (s.status = 'ended'
                         AND TRIM(COALESCE(t.title, '')) IN ('', '(未命名会话)')
                         AND NOT EXISTS (SELECT 1 FROM todos td WHERE td.task_id = t.id)))"
            );
            let (n, a): (i64, i64) = self.conn.query_row(
                &sql,
                rusqlite::params_from_iter(chunk),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            total += n;
            archived += a;
        }
        Ok((total, archived))
    }

    /// 可能落入 running / waiting 的会话（未归档、未结束，且 status/pending_review 使其够格）。
    ///
    /// 只吐出**判定 connected 所需的原料**，不做统计——见 [`Self::live_sessions_totals`] 的说明。
    ///
    /// `status != 'ended'` 这一条不能省。写侧已把它做成不变量（set_pending_review 带
    /// `status <> 'ended'` 守卫，生命周期边界也都清 pending_review），但旧版本数据库仍可能
    /// 存在「ended + pending_review」残留；查询必须防御这类历史数据，否则会把已结束会话捞进来。
    ///
    /// 注意 `session_connected` 对 ended **不是**恒 false——托管 PTY 还活着就判在线（恢复
    /// 窗口期的兜底）。所以「ended 不进角标」这件事是**这条谓词**在守，而不是判活顺手排除
    /// 的；列表侧 running/waiting 的 SQL 带着逐字相同的谓词，两端才同视野。真正会被屏幕
    /// 状态抬进 tab 的那一类（ended + 残留 idle 屏）另由 app 层的 `tab_class` 挡住。
    ///
    /// ping（健康探测）会话即便连着也不会出现在列表里（enrich 无条件丢弃），这里同样
    /// 剔除，否则 running/waiting 角标会数出一条用户在列表里找不到的会话。
    pub fn live_count_candidates(&self) -> Result<Vec<LiveCandidate>, StoreError> {
        let mut st = self.conn.prepare(
            "SELECT s.id, s.status, s.pending_review, s.pid, s.last_event_at,
                    s.cc_session_id, s.provider, s.cwd FROM sessions s
             LEFT JOIN tasks t ON t.id = (SELECT MAX(t2.id) FROM tasks t2 WHERE t2.session_id = s.id)
             WHERE s.archived = 0 AND s.status != 'ended' AND s.superseded_by IS NULL
               AND (s.status IN ('running','waiting') OR s.pending_review IS NOT NULL)
               AND LOWER(TRIM(COALESCE(t.title, ''))) != 'ping'",
        )?;
        let rows = st.query_map([], |r| {
            Ok(LiveCandidate {
                id: r.get(0)?,
                status: r.get(1)?,
                pending_review: r.get(2)?,
                pid: r.get(3)?,
                last_event_at: r.get(4)?,
                cc_session_id: r.get(5)?,
                provider: r.get(6)?,
                cwd: r.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// 兜底收尾「没有 pid、且超过 idle_ms 无任何事件」的 live 会话。返回受影响数。
    ///
    /// 带 pid 的会话由进程存活校验处理（进程在就保留，哪怕空闲很久——那是 claude 在等用户输入，
    /// 仍是连接中，绝不能因空闲误杀）。但 pid 为空（reporter 没抓到 owner pid）的会话无法做存活校验，
    /// 若终端被直接关掉（SessionEnd 丢失），就会永远卡在 live。对这类会话退化为「空闲超时」清理：
    /// 真正活跃的会话每个事件都会刷新 last_event_at，到不了这个阈值。
    ///
    /// `managed_session_ids`：本 GUI 正托管 PTY 的会话集合（进程是 meowo 自己 spawn 的,
    /// 必然活着）。托管会话 hook 可能一直没认领 pid（codex 的 session_start 要到首个 turn
    /// 才触发）,只看 `pid IS NULL` 会把正被托管的会话误收尾——与 session_connected/
    /// process_alive 的「pid 命中 ‖ 托管 PTY 活跃」口径必须一致,这里把托管集作排除集传入。
    pub fn end_orphaned_idle(
        &self,
        idle_ms: i64,
        now_ms: i64,
        managed_session_ids: &std::collections::HashSet<i64>,
    ) -> Result<usize, StoreError> {
        // 空集不能拼出 `NOT IN ()`（SQLite 语法错误）；无排除项时省掉整个子句。
        let exclude = if managed_session_ids.is_empty() {
            String::new()
        } else {
            let placeholders = vec!["?"; managed_session_ids.len()].join(",");
            format!(" AND id NOT IN ({placeholders})")
        };
        let sql = format!(
            "UPDATE sessions SET status='ended', pending_review=NULL, ended_at=?1, last_event_at=?1
             WHERE pid IS NULL AND status IN ('running','waiting')
               AND (?1 - last_event_at) > ?2{exclude}"
        );
        let mut params: Vec<rusqlite::types::Value> =
            Vec::with_capacity(2 + managed_session_ids.len());
        params.push(rusqlite::types::Value::Integer(now_ms));
        params.push(rusqlite::types::Value::Integer(idle_ms));
        params.extend(
            managed_session_ids
                .iter()
                .map(|id| rusqlite::types::Value::Integer(*id)),
        );
        let n = self
            .conn
            .execute(&sql, rusqlite::params_from_iter(params))?;
        Ok(n)
    }
}
