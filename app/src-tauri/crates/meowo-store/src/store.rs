use crate::error::StoreError;
use crate::migrations::SCHEMA;
use crate::models::{
    PendingReview, Project, Session, SessionStatus, Task, TaskColumn, Todo, TodoInput, TodoStatus,
};
use rusqlite::Connection;
use std::path::Path;

pub struct Store {
    pub(crate) conn: Connection,
}

/// 对话窗口一次读齐的会话头部信息（sessions + 关联 tasks）。
#[derive(Debug, Clone)]
pub struct SessionHeader {
    pub cc_session_id: String,
    pub status: String,
    pub cwd: Option<String>,
    pub provider: String,
    pub pending_review: Option<String>,
    /// 存活校正所需（对话窗与看板同口径判 connected）：hook 认领的进程 pid 与最近事件时间。
    pub pid: Option<i64>,
    pub last_event_at: i64,
    /// 无关联任务时为 None（调用方回落到占位标题）。
    pub title: Option<String>,
    pub current_activity: Option<String>,
    /// hook 驱动的最近往来（UserPromptSubmit / Stop 落库）。transcript 尚未落盘或该 agent
    /// 不提供结构化 transcript 时，对话窗口用它们渲染临时时间线，而不是一片空白。
    pub last_user_text: Option<String>,
    pub last_ai_text: Option<String>,
}

/// statusline 写入的单会话上下文快照。字段各自可能缺失（provider 不支持 / 首帧未到）。
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub model: Option<String>,
    pub used_pct: Option<i64>,
    pub window_size: Option<i64>,
}

/// `BEGIN IMMEDIATE` 的收尾守卫：drop 时若未显式 commit 就 best-effort ROLLBACK——
/// rusqlite 的 `unchecked_transaction` 只发 deferred BEGIN，而 WAL 下 deferred 事务在
/// 「先 SELECT 后写入」的间隙被别的连接提交写入时，升级写锁会 SQLITE_BUSY_SNAPSHOT 直接
/// 失败（busy handler 不生效）。IMMEDIATE 开局即取写锁（冲突走 busy_timeout 等待），
/// 配本守卫保证任何错误路径都不会把写锁/未决事务留在连接上。
struct ImmediateTx<'a> {
    conn: &'a Connection,
    committed: bool,
}

impl<'a> ImmediateTx<'a> {
    fn begin(conn: &'a Connection) -> Result<Self, StoreError> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            conn,
            committed: false,
        })
    }

    fn commit(mut self) -> Result<(), StoreError> {
        self.conn.execute_batch("COMMIT")?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for ImmediateTx<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

impl Store {
    /// 打开（或新建）数据库，开启 WAL，执行建表。
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Store, StoreError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        // busy_timeout 必须先于 journal_mode：否则并发首次建库时 WAL 切换以 0 超时直接 BUSY。
        conn.pragma_update(None, "busy_timeout", 3000)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Self::init(&conn)?;
        Ok(Store { conn })
    }

    /// 仅用于测试：内存库。
    pub fn open_in_memory() -> Result<Store, StoreError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Self::init(&conn)?;
        Ok(Store { conn })
    }

    /// schema 版本：升 schema/加迁移时 +1。
    /// v2: 新增 session_notes 表（会话便签）。旧库 version<2 时 init 会重跑
    /// `CREATE TABLE IF NOT EXISTS` 把新表补上，再 bump。
    /// v3: sessions 加 pending_review / last_ai_text / last_user_text 三列。
    /// v4: session_context 加 model 列（statusline 的模型展示名）。
    /// v5: sessions 加 provider 列（agent 提供方：claude/kimi…）。
    /// v6: 加 sessions(status, last_event_at) 索引（live_sessions 的「已结束仅取最近 100 条」子查询）。
    /// v7: sessions 加 profile 列（多账号）。该 ALTER 曾在 v6 时代被加进列表却没 bump 版本，
    ///     已升到 v6 的老库被门控挡住、列永远补不上——profile 会话的 SessionStart 写 profile
    ///     报错中断，cwd/pid 全丢（对话窗识别不到工作区的根因）。
    const USER_VERSION: i64 = 7;

    /// 一次性建表 + 迁移 + 建索引，用 `PRAGMA user_version` 门控：已是最新版直接返回，
    /// 避免 statusline/hook 每次 open 都重跑 DDL 与注定失败的 ALTER（hot-path 浪费）。
    /// 迁移/建索引若遇非「列已存在」错误（BUSY/IO）则**不**bump 版本，下次 open 自动重试，
    /// 不再把瞬时错误永久吞掉。
    fn init(conn: &rusqlite::Connection) -> Result<(), StoreError> {
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);
        if version >= Self::USER_VERSION {
            return Ok(());
        }
        conn.execute_batch(SCHEMA)?;
        // 给旧库补列（新库 SCHEMA 已含这些列 → ALTER 必报 duplicate，忽略即可）。
        const ALTERS: [&str; 10] = [
            "ALTER TABLE sessions ADD COLUMN pid INTEGER",
            "ALTER TABLE sessions ADD COLUMN cwd TEXT",
            "ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sessions ADD COLUMN archived_at INTEGER",
            "ALTER TABLE sessions ADD COLUMN pending_review TEXT",
            "ALTER TABLE sessions ADD COLUMN last_ai_text TEXT",
            "ALTER TABLE sessions ADD COLUMN last_user_text TEXT",
            "ALTER TABLE session_context ADD COLUMN model TEXT",
            // 此 'claude' 默认值与 migrations.rs 建表默认值、DEFAULT_PROVIDER 常量为同一事实，
            // 改默认 provider 时需三处同步（models.rs 绊线测试会在改常量时红）。
            "ALTER TABLE sessions ADD COLUMN provider TEXT NOT NULL DEFAULT 'claude'",
            // 多账号：会话跑在哪个 profile 上。NULL = 默认账号——老库补齐后全是 NULL，正是我们要的。
            "ALTER TABLE sessions ADD COLUMN profile TEXT",
        ];
        for sql in ALTERS {
            if let Err(e) = conn.execute(sql, []) {
                if !e.to_string().contains("duplicate column name") {
                    eprintln!("meowo-store migrate 失败: {sql}: {e}");
                    return Ok(()); // 非「列已存在」（BUSY/IO）：不 bump，下次 open 重试
                }
            }
        }
        // 索引：加速按 project / task / pid 的查询与「驱逐旧会话」（小库无感，大库防全表扫）。
        const INDEXES: [&str; 5] = [
            "CREATE INDEX IF NOT EXISTS ix_sessions_project ON sessions(project_id)",
            "CREATE INDEX IF NOT EXISTS ix_sessions_pid ON sessions(pid)",
            "CREATE INDEX IF NOT EXISTS ix_tasks_project_col ON tasks(project_id, column_name)",
            "CREATE INDEX IF NOT EXISTS ix_todos_task ON todos(task_id)",
            // live_sessions 的「已结束仅取最近 100 条」子查询走此索引，避免每次调用全表扫描+排序。
            "CREATE INDEX IF NOT EXISTS ix_sessions_status_lea ON sessions(status, last_event_at DESC)",
        ];
        for sql in INDEXES {
            if let Err(e) = conn.execute(sql, []) {
                eprintln!("meowo-store 建索引失败: {sql}: {e}");
                return Ok(()); // 同上：不 bump，下次重试
            }
        }
        let _ = conn.pragma_update(None, "user_version", Self::USER_VERSION);
        Ok(())
    }

    /// 测试辅助：统计用户表数量。
    pub fn raw_table_count(&self) -> Result<i64, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    // == Task 4: upsert_project_by_root + list_projects ==

    /// 按 root_path upsert 项目，返回 project id。已存在则更新 updated_at。
    pub fn upsert_project_by_root(
        &self,
        root_path: &str,
        name: &str,
        now_ms: i64,
    ) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO projects (root_path, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(root_path) DO UPDATE SET updated_at = MAX(updated_at, ?3), name = ?2",
            rusqlite::params![root_path, name, now_ms],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE root_path = ?1",
            [root_path],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// 写入/更新某会话的上下文用量与模型展示名（来自 Claude Code statusline）。
    pub fn set_session_context(
        &self,
        cc_session_id: &str,
        used_pct: Option<i64>,
        window_size: Option<i64>,
        model: Option<&str>,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO session_context (cc_session_id, used_pct, window_size, model, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(cc_session_id) DO UPDATE SET
                 used_pct = COALESCE(excluded.used_pct, used_pct),
                 window_size = COALESCE(excluded.window_size, window_size),
                 model = COALESCE(excluded.model, model),
                 updated_at = excluded.updated_at
             WHERE excluded.updated_at >= session_context.updated_at",
            rusqlite::params![cc_session_id, used_pct, window_size, model, now_ms],
        )?;
        Ok(())
    }

    /// `PRAGMA data_version`：一个整数，仅当**别的连接**向本库提交过写入时才变化（本连接自身的
    /// 写入不改它，纯读也不改）。跨调用比较须用**同一个持久连接**才有意义。
    /// db-watcher 用它把「真实写入」与「app 读库时新开 WAL 连接触碰 -wal/-shm 文件」的空事件区分开：
    /// 只有版本号变了才通知前端刷新，掐断 read→watcher→refresh→read 的自持刷新循环。
    pub fn data_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    /// 写入/清除某会话的便签：trim 后非空则 upsert，空则删除该行（便签清空即移除）。
    pub fn set_session_note(
        &self,
        cc_session_id: &str,
        note: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let trimmed = note.trim();
        if trimmed.is_empty() {
            self.conn.execute(
                "DELETE FROM session_notes WHERE cc_session_id = ?1",
                [cc_session_id],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO session_notes (cc_session_id, note, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(cc_session_id) DO UPDATE SET note = excluded.note, updated_at = excluded.updated_at",
                rusqlite::params![cc_session_id, trimmed, now_ms],
            )?;
        }
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root_path, name, created_at, updated_at FROM projects ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Project {
                id: r.get(0)?,
                root_path: r.get(1)?,
                name: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // == Task 5: start_session ==

    /// 开始一个会话；若 cc_session_id 已存在则幂等返回既有 (session_id, task_id)。
    /// 新会话会同时建一张占位任务卡。
    pub fn start_session(
        &self,
        project_id: i64,
        cc_session_id: &str,
        now_ms: i64,
    ) -> Result<(i64, i64), StoreError> {
        // 事务保证「会话 + 占位任务」原子落库，避免中途失败留下无任务卡的半态会话。
        let tx = self.conn.unchecked_transaction()?;
        // 会话幂等：已存在则复活为 running（resume/--continue 场景），清掉 ended_at。
        tx.execute(
            "INSERT INTO sessions (project_id, cc_session_id, status, started_at, last_event_at)
             VALUES (?1, ?2, 'running', ?3, ?3)
             ON CONFLICT(cc_session_id) DO UPDATE SET
                 status = 'running',
                 last_event_at = excluded.last_event_at,
                 ended_at = NULL,
                 pending_review = NULL
             WHERE excluded.last_event_at >= sessions.last_event_at",
            rusqlite::params![project_id, cc_session_id, now_ms],
        )?;
        let sid = self
            .find_session_id(cc_session_id)?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        // 占位任务幂等：靠 tasks(session_id) 唯一索引 + INSERT OR IGNORE 防并发重复建卡。
        tx.execute(
            "INSERT OR IGNORE INTO tasks
                (project_id, session_id, title, column_name, column_locked, created_at, updated_at)
             VALUES (?1, ?2, '(未命名会话)', 'todo', 0, ?3, ?3)",
            rusqlite::params![project_id, sid, now_ms],
        )?;
        let tid = self.task_id_of_session(sid)?;
        tx.commit()?;
        Ok((sid, tid))
    }

    pub fn find_session_id_pub(&self, cc_session_id: &str) -> Result<Option<i64>, StoreError> {
        self.find_session_id(cc_session_id)
    }

    pub fn task_id_of_session_pub(&self, session_id: i64) -> Result<i64, StoreError> {
        self.task_id_of_session(session_id)
    }

    pub fn set_current_activity(
        &self,
        session_id: i64,
        activity: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        // 无任务卡的会话(看板删卡/历史导入的残行)不能让整条 hook 分发炸掉:
        // 活动名没有落点就算了,会话本身的状态翻转(touch)照常。
        let Some(tid) = self.task_id_of_session_opt(session_id)? else {
            self.touch_session(session_id, now_ms)?;
            return Ok(());
        };
        // 按字符截断，防超大 Bash 命令整条进库并随轮询全量下发。
        let activity = truncate_chars(activity, 200);
        self.conn.execute(
            "UPDATE tasks SET current_activity = ?1, updated_at = ?2 \
             WHERE id = ?3 AND updated_at <= ?2 \
               AND EXISTS (SELECT 1 FROM sessions \
                           WHERE id = tasks.session_id AND last_event_at <= ?2)",
            rusqlite::params![activity, now_ms, tid],
        )?;
        self.touch_session(session_id, now_ms)?;
        Ok(())
    }

    /// 用户开启新回合：清掉上一回合残留的活动名——否则新回合首个工具调用之前，前端一直
    /// 显示上一回合早已完成的命令，像卡死在某步。只清 tasks、不 touch：调用方紧接着的
    /// on_user_prompt / touch_session 已负责会话状态翻转，这里再 touch 就是双写。
    ///
    /// 清除**无条件**执行(不带 `updated_at <= now` 守卫):hook 事件的 now_ms 来自各进程
    /// 墙钟、并不单调,守卫在时钟回拨时会静默空转,旧命令贯穿整个新回合常显。清除本身
    /// 幂等且方向恒对——新回合开始,旧活动名必然过时。updated_at 取 MAX 防时间倒退。
    pub fn clear_current_activity(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        let Some(tid) = self.task_id_of_session_opt(session_id)? else {
            // 无任务卡的会话:无活动可清,静默通过,别让 UserPromptSubmit 整条分发中止。
            return Ok(());
        };
        self.conn.execute(
            "UPDATE tasks SET current_activity = NULL, updated_at = MAX(updated_at, ?1) \
             WHERE id = ?2",
            rusqlite::params![now_ms, tid],
        )?;
        Ok(())
    }

    /// 直接设置会话任务标题（来自 CC ai-title/custom-title），覆盖占位/旧标题。空标题忽略。
    pub fn set_session_title(
        &self,
        session_id: i64,
        title: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let t = truncate_chars(title.trim(), 80);
        if t.is_empty() {
            return Ok(());
        }
        let tid = self.task_id_of_session(session_id)?;
        self.conn.execute(
            "UPDATE tasks SET title = ?1, updated_at = ?2 \
             WHERE id = ?3 AND updated_at <= ?2 \
               AND EXISTS (SELECT 1 FROM sessions \
                           WHERE id = tasks.session_id AND last_event_at <= ?2)",
            rusqlite::params![t, now_ms, tid],
        )?;
        Ok(())
    }

    /// 读会话当前任务标题（贴纸/卡片显示的那个）；无/空则 None。meowo-reporter 给 WT 标签写 token 时
    /// 用作可见前缀（比 cwd 目录名更贴合卡片）。
    pub fn session_title(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        match self.conn.query_row(
            "SELECT title FROM tasks WHERE session_id = ?1",
            [session_id],
            |r| r.get::<_, String>(0),
        ) {
            Ok(t) if !t.trim().is_empty() => Ok(Some(t)),
            Ok(_) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn find_session_id(&self, cc_session_id: &str) -> Result<Option<i64>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM sessions WHERE cc_session_id = ?1")?;
        let mut rows = stmt.query([cc_session_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn task_id_of_session(&self, session_id: i64) -> Result<i64, StoreError> {
        let id: i64 = self.conn.query_row(
            "SELECT id FROM tasks WHERE session_id = ?1 ORDER BY id LIMIT 1",
            [session_id],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// [`task_id_of_session`] 的容错变体:无任务卡返回 `Ok(None)` 而非 NoRows 错误。
    /// hook 分发路径用它——任务卡可被看板删除,历史导入库也存在无任务行,
    /// 这类会话不该让整条事件分发中止。
    pub(crate) fn task_id_of_session_opt(
        &self,
        session_id: i64,
    ) -> Result<Option<i64>, StoreError> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM tasks WHERE session_id = ?1 ORDER BY id LIMIT 1",
                [session_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn get_task(&self, task_id: i64) -> Result<Task, StoreError> {
        let task = self.conn.query_row(
            "SELECT id, project_id, session_id, title, column_name, column_locked, current_activity, created_at, updated_at
             FROM tasks WHERE id = ?1",
            [task_id],
            |r| {
                Ok(Task {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    session_id: r.get(2)?,
                    title: r.get(3)?,
                    column: r.get(4)?,
                    column_locked: r.get::<_, i64>(5)? != 0,
                    current_activity: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                })
            },
        )?;
        Ok(task)
    }

    // == Task 6: on_user_prompt + touch_session ==

    /// 收到用户 prompt：仅当占位标题时替换为截断后的 prompt(不再写 current_activity，那已由 last_user_text 承担)。
    pub fn on_user_prompt(
        &self,
        session_id: i64,
        prompt: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let tid = self.task_id_of_session(session_id)?;
        let cleaned = truncate_chars(&sanitize_prompt(prompt), 60);
        if !cleaned.is_empty() {
            self.conn.execute(
                "UPDATE tasks SET title = ?1, updated_at = ?2 \
                 WHERE id = ?3 AND title = '(未命名会话)' AND updated_at <= ?2 \
                   AND EXISTS (SELECT 1 FROM sessions \
                               WHERE id = tasks.session_id AND last_event_at <= ?2)",
                rusqlite::params![cleaned, now_ms, tid],
            )?;
            // 非占位标题:不再把 prompt 写进 current_activity(改由 last_user_text 承担)。
        }
        self.touch_session(session_id, now_ms)?;
        Ok(())
    }

    /// 更新会话 last_event_at；若处于 waiting/stale 恢复为 running。
    pub fn touch_session(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions
             SET last_event_at = ?1,
                 status = CASE WHEN status IN ('waiting','stale') THEN 'running' ELSE status END
             WHERE id = ?2 AND last_event_at <= ?1",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }

    // == Task 7: sync_todos + set_task_column + list_todos ==

    /// 用新列表整体替换某会话任务的 todos；未锁定时按 todo 推导列。
    pub fn sync_todos(
        &self,
        session_id: i64,
        todos: &[TodoInput],
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let tid = self.task_id_of_session(session_id)?;
        // 不能沿用 `unchecked_transaction` 的 deferred BEGIN：WAL 下 SELECT 与 DELETE/INSERT
        // 之间若被别的连接提交写入，升级写事务会 SQLITE_BUSY_SNAPSHOT 直接失败（busy handler
        // 不生效），这次 TodoWrite 就被丢了（要等下一条事件才自愈）。BEGIN IMMEDIATE 开局即
        // 取写锁，冲突走 busy_timeout 等待；守卫语义（下方乱序挡写）不变。
        let tx = ImmediateTx::begin(&self.conn)?;
        let (locked, task_updated_at): (bool, i64) = self.conn.query_row(
            "SELECT column_locked, updated_at FROM tasks WHERE id = ?1",
            [tid],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?)),
        )?;
        // Todo hook 可能乱序到达。挡迟到事件只能跟**todo 自己的更新时刻**（task.updated_at，
        // 每次 sync_todos 才推进）比，不能跟 session.last_event_at 比：后者是「会话最后活跃
        // 时刻」，被 Stop、其它工具的 PostToolUse 等**任何**事件推进。一旦这些事件把 session
        // 时钟顶到未来，随后到达的 TodoList——哪怕正是最新快照——就会因 now_ms 落后而被整份
        // 丢弃，界面于是永远停在旧状态（实测：done 的快照进不来，库里一直是 pending）。
        if task_updated_at > now_ms {
            return tx.commit();
        }
        self.conn
            .execute("DELETE FROM todos WHERE task_id = ?1", [tid])?;
        for (i, t) in todos.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO todos (task_id, content, status, order_idx) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![tid, t.content, t.status.as_str(), i as i64],
            )?;
        }
        if !locked {
            let col = derive_column(todos);
            self.conn.execute(
                "UPDATE tasks SET column_name = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![col.as_str(), now_ms, tid],
            )?;
        } else {
            self.conn.execute(
                "UPDATE tasks SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now_ms, tid],
            )?;
        }
        // touch_session 等价逻辑（事务内）：刷新 last_event_at，waiting/stale 复活为 running
        self.conn.execute(
            "UPDATE sessions
             SET last_event_at = ?1,
                 status = CASE WHEN status IN ('waiting','stale') THEN 'running' ELSE status END
             WHERE id = ?2 AND last_event_at <= ?1",
            rusqlite::params![now_ms, session_id],
        )?;
        tx.commit()
    }

    pub fn list_todos(&self, task_id: i64) -> Result<Vec<Todo>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, content, status, order_idx FROM todos WHERE task_id = ?1 ORDER BY order_idx",
        )?;
        let rows = stmt.query_map([task_id], |r| {
            Ok(Todo {
                id: r.get(0)?,
                task_id: r.get(1)?,
                content: r.get(2)?,
                status: r.get(3)?,
                order_idx: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 设置任务列；locked=true 表示手动覆盖，之后自动推导不再生效。
    pub fn set_task_column(
        &self,
        task_id: i64,
        column: TaskColumn,
        locked: bool,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE tasks SET column_name = ?1, column_locked = ?2, updated_at = ?3 \
             WHERE id = ?4 AND updated_at <= ?3",
            rusqlite::params![column.as_str(), locked as i64, now_ms, task_id],
        )?;
        Ok(())
    }

    // == Task 8: 会话状态变更与 stale 标记 ==

    /// 手动设置会话状态（如 waiting / stale），同时更新 last_event_at。
    pub fn set_session_status(
        &self,
        session_id: i64,
        status: SessionStatus,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        if status == SessionStatus::Ended {
            return self.end_session(session_id, now_ms);
        }
        self.conn.execute(
            "UPDATE sessions SET status = ?1, last_event_at = ?2 WHERE id = ?3 AND last_event_at <= ?2",
            rusqlite::params![status.as_str(), now_ms, session_id],
        )?;
        Ok(())
    }

    /// 设置待审批子态,同时刷新 last_event_at(让卡片排到最近活跃,并作为去重指纹)。
    pub fn set_pending_review(
        &self,
        session_id: i64,
        kind: PendingReview,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET pending_review = ?1, last_event_at = ?2 WHERE id = ?3 AND last_event_at <= ?2",
            rusqlite::params![kind.as_str(), now_ms, session_id],
        )?;
        Ok(())
    }

    /// 清除待审批子态(置 NULL)。不动 last_event_at——由同回合的兄弟调用负责时间戳。
    pub fn clear_pending_review(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET pending_review = NULL WHERE id = ?1 AND last_event_at <= ?2",
            rusqlite::params![session_id, now_ms],
        )?;
        Ok(())
    }

    /// 落最近一条 AI 正文:折叠空白 + 截断 200 字符;空/全空白不覆盖旧值。
    /// 不动 last_event_at——Stop 的兄弟 set_session_status 已刷新它。
    pub fn set_last_ai_text(
        &self,
        session_id: i64,
        text: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let cleaned = truncate_chars(&sanitize_prompt(text), 200);
        if cleaned.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE sessions SET last_ai_text = ?1 WHERE id = ?2 AND last_event_at <= ?3",
            rusqlite::params![cleaned, session_id, now_ms],
        )?;
        Ok(())
    }

    /// 落最近一条用户消息:复用 sanitize_prompt(剥图片标记 + 折叠空白) + 截断 200;空不覆盖。
    /// 不动 last_event_at——UserPromptSubmit 的 on_user_prompt(touch_session) 已刷新它。
    pub fn set_last_user_text(
        &self,
        session_id: i64,
        text: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let cleaned = truncate_chars(&sanitize_prompt(text), 200);
        if cleaned.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE sessions SET last_user_text = ?1 WHERE id = ?2 AND last_event_at <= ?3",
            rusqlite::params![cleaned, session_id, now_ms],
        )?;
        Ok(())
    }

    /// 复活被误判收尾的会话：仅当当前为 ended 时，置回 running 并清 ended_at、刷新时间。
    /// 用于「会话曾因 pid 未被认作存活而被 reap 成 ended，但用户其实还在该会话里继续发言」的自愈。
    pub fn revive_if_ended(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET status='running', ended_at=NULL, pending_review=NULL, last_event_at=?1 \
             WHERE id=?2 AND status='ended' AND last_event_at <= ?1",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }

    /// 看板主动 resume 一个会话时调用：复活它(置 running、清 ended_at)、清空 pid、并把 last_event_at
    /// 刷成 now(作为 app 侧「resume 乐观连接」宽限期的起点)。
    /// 清 pid 是关键——旧进程已死，留着会被 reaper 当「pid 已死」立即再收尾；清空后 reaper「pid 未知不臆测」
    /// 不动它(见 live_session_liveness 消费方)，等新进程首个 hook 用 set_session_pid 认领真实 pid。
    /// 解决 codex 这类「session_start hook 要到首个 turn 才触发」的 agent：resume 后不必等发消息即显示已连接。
    /// 命中条件「ended ‖ pid 为空 ‖ pid=已验证死亡的那个 pid」：pid 为空即没有任何 hook 认领过、不是真连接，
    /// 可安全重置(含宽限过期后用户再次点 resume——此时 status 仍是 running 但 pid 空，须刷新 last_event_at
    /// 重启宽限)。`dead_pid` 由调用方校验「记录的该 pid 进程确已死亡」后传入——覆盖「进程刚死、reaper(5s 周期)
    /// 尚未收尾」的窗口：此时 status 仍是 running 且 pid 非空，若不强制复活，本次 resume 会静默 0 行更新，
    /// 随后被 reaper 收尾成 ended、卡片长期显示未连接(codex 要到首条消息 hook 才自愈)。
    /// SQL 里比对 `pid=?3` 而非无条件强制：调用方快照与本 UPDATE 之间若新进程 hook 已认领了新的存活 pid，
    /// 行内 pid 已不等于快照校验过死亡的旧 pid，守卫不命中——「绝不清活连接」的不变量在 DB 层原子闭合，
    /// 不依赖调用方时序。返回是否真的复活了(命中 0 行 = 会话实为连接中，调用方失败回滚时不得误收尾)。
    pub fn revive_for_resume(
        &self,
        session_id: i64,
        now_ms: i64,
        dead_pid: Option<i64>,
    ) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET status='running', ended_at=NULL, pending_review=NULL, pid=NULL, last_event_at=?1 \
             WHERE id=?2 AND last_event_at <= ?1 AND (status='ended' OR pid IS NULL OR pid=?3)",
            rusqlite::params![now_ms, session_id, dead_pid],
        )?;
        Ok(n > 0)
    }

    /// 该 pid 是否已被**另一条**未结束的会话行认领。set_session_pid 保证同一时刻至多
    /// 一条非 ended 行持有某个 pid——存在这样的别行，就说明本会话记录的 pid 是换代残留
    /// （典型：/clear 前的旧行，见 end_session 注释），进程虽活但已不属于本会话，
    /// 接管守卫应视其为「本会话进程已死」。也顺带自愈 end_session 清 pid 之前留下的存量脏行。
    pub fn pid_held_by_other_live(&self, session_id: i64, pid: i64) -> Result<bool, StoreError> {
        let r = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE pid = ?1 AND id <> ?2 AND status <> 'ended')",
            rusqlite::params![pid, session_id],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(r)
    }

    /// 取会话当前记录的 pid（resume 前校验死活用；不存在的会话报错）。
    pub fn session_pid(&self, session_id: i64) -> Result<Option<i64>, StoreError> {
        let r = self.conn.query_row(
            "SELECT pid FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(r)
    }

    /// 结束会话：状态设为 ended，记录 ended_at，并清 pid。
    /// 清 pid 是必须的：/clear 只换会话不换进程，SessionEnd 先于新会话认领 pid 到达时，
    /// 旧行若留着 pid，set_session_pid 的换代清理（只扫非 ended 行）摘不掉它——此后
    /// 接管守卫拿这个「活着但已归新会话」的 pid 判活，会把旧会话误判成「仍在外部终端运行」。
    pub fn end_session(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET status = 'ended', pending_review = NULL, pid = NULL, ended_at = ?1, last_event_at = ?1 WHERE id = ?2 AND last_event_at <= ?1",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }

    /// 仅当会话仍持有调用方观察到的 pid 时收尾。用于进程快照 reaper，闭合“读旧 pid 后新进程
    /// 已重新认领同一会话”的 TOCTOU；返回 false 表示记录已变化，绝不能误杀新连接。
    pub fn end_session_if_pid(
        &self,
        session_id: i64,
        observed_pid: i64,
        observed_last_event_at: i64,
        now_ms: i64,
    ) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET status='ended', pending_review=NULL, ended_at=?1, last_event_at=?1, pid=NULL \
             WHERE id=?2 AND pid=?3 AND last_event_at=?4 AND status<>'ended'",
            rusqlite::params![now_ms, session_id, observed_pid, observed_last_event_at],
        )?;
        Ok(n > 0)
    }

    /// 导入一条历史会话：以 ended 状态写入，started_at=ended_at=last_event_at=mtime。
    /// 用 ON CONFLICT(cc_session_id) DO NOTHING 保证绝不覆盖已存在的真实会话。
    /// 返回 true 表示新插入；false 表示 cc_session_id 已存在被跳过。
    pub fn import_session(
        &self,
        cc_session_id: &str,
        project_id: i64,
        title: &str,
        cwd: Option<&str>,
        last_event_at: i64,
    ) -> Result<bool, StoreError> {
        // 事务保证「会话 + 任务卡」原子落库：DO NOTHING 的幂等判断使半态永不重试，必须避免。
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(
            "INSERT INTO sessions
                (project_id, cc_session_id, status, started_at, last_event_at, ended_at, cwd)
             VALUES (?1, ?2, 'ended', ?3, ?3, ?3, ?4)
             ON CONFLICT(cc_session_id) DO NOTHING",
            rusqlite::params![project_id, cc_session_id, last_event_at, cwd],
        )?;
        if n == 0 {
            return Ok(false); // 已存在，绝不覆盖（事务随 drop 回滚，无写入）
        }
        let sid = self
            .find_session_id(cc_session_id)?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
        let mut t = truncate_chars(title.trim(), 80);
        if t.is_empty() {
            t = "(未命名会话)".to_string();
        }
        // 历史已结束会话的任务卡固定放 done 列，不导入 todo。
        tx.execute(
            "INSERT OR IGNORE INTO tasks
                (project_id, session_id, title, column_name, column_locked, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'done', 0, ?4, ?4)",
            rusqlite::params![project_id, sid, t, last_event_at],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn get_session(&self, session_id: i64) -> Result<Session, StoreError> {
        let s = self.conn.query_row(
            "SELECT id, project_id, cc_session_id, status, started_at, last_event_at, ended_at
             FROM sessions WHERE id = ?1",
            [session_id],
            |r| {
                Ok(Session {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    cc_session_id: r.get(2)?,
                    status: r.get(3)?,
                    started_at: r.get(4)?,
                    last_event_at: r.get(5)?,
                    ended_at: r.get(6)?,
                })
            },
        )?;
        Ok(s)
    }

    pub fn session_pending_review(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row(
                "SELECT pending_review FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    /// 最近有活动的未归档会话 id。托盘点击没有「当前会话」上下文，用它决定打开哪个。
    /// 一条会话都没有时返回 None（调用方回落到打开设置）。
    pub fn latest_session_id(&self) -> Result<Option<i64>, StoreError> {
        match self.conn.query_row(
            "SELECT id FROM sessions WHERE archived = 0 ORDER BY last_event_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        ) {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 对话窗口一次拿齐会话头部信息。此前这些字段由 5 个方法分别查询——它们打的是
    /// sessions/tasks 各自的**同一行**，在 650ms 轮询下每秒多做近十次无谓往返。
    /// tasks 用 LEFT JOIN：会话未必有关联任务，缺行时 title/current_activity 为 None。
    pub fn session_header(&self, session_id: i64) -> Result<SessionHeader, StoreError> {
        self.conn
            .query_row(
                "SELECT s.cc_session_id, s.status, s.cwd, s.provider, s.pending_review, \
                        t.title, t.current_activity, s.last_user_text, s.last_ai_text, \
                        s.pid, s.last_event_at \
                 FROM sessions s LEFT JOIN tasks t ON t.session_id = s.id \
                 WHERE s.id = ?1 LIMIT 1",
                [session_id],
                |row| {
                    // provider 的空值回退必须与 session_provider 一致：DB 里可能是 NULL
                    // 或空串，直接透出去会让上层按未知 agent 处理（丢掉 transcript 能力）。
                    let provider: Option<String> = row.get(3)?;
                    Ok(SessionHeader {
                        cc_session_id: row.get(0)?,
                        status: row.get(1)?,
                        cwd: row.get(2)?,
                        provider: provider
                            .filter(|p| !p.trim().is_empty())
                            .unwrap_or_else(|| crate::DEFAULT_PROVIDER.to_string()),
                        pending_review: row.get(4)?,
                        // 与 session_title 同语义：纯空白标题视作没有标题。
                        title: row
                            .get::<_, Option<String>>(5)?
                            .filter(|t| !t.trim().is_empty()),
                        current_activity: row.get(6)?,
                        last_user_text: row.get(7)?,
                        last_ai_text: row.get(8)?,
                        pid: row.get(9)?,
                        last_event_at: row.get(10)?,
                    })
                },
            )
            .map_err(StoreError::from)
    }

    /// statusline 写入的单会话上下文快照：模型展示名 + 已用百分比 + 上下文窗口大小。
    /// 无 statusline 数据（provider 不支持 / 首帧未到）时各字段为 None——session_context
    /// 按事件懒建行，这里不假定行存在。看板走批量 flatten，这条是对话窗口的单条读法。
    pub fn session_context(&self, cc_session_id: &str) -> Result<SessionContext, StoreError> {
        match self.conn.query_row(
            "SELECT model, used_pct, window_size FROM session_context WHERE cc_session_id = ?1",
            [cc_session_id],
            |row| {
                Ok(SessionContext {
                    model: row.get(0)?,
                    used_pct: row.get(1)?,
                    window_size: row.get(2)?,
                })
            },
        ) {
            Ok(ctx) => Ok(ctx),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(SessionContext::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// 会话对应任务的当前活动文本（工具名/阶段描述，hook 写入）。无任务或空闲为 None。
    pub fn session_current_activity(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        match self.conn.query_row(
            "SELECT current_activity FROM tasks WHERE session_id = ?1 LIMIT 1",
            [session_id],
            |row| row.get(0),
        ) {
            Ok(activity) => Ok(activity),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 记录会话启动时的 cwd（用于重建 transcript 路径取标题）。
    pub fn set_session_cwd(
        &self,
        session_id: i64,
        cwd: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET cwd = ?1, last_event_at = ?2 \
             WHERE id = ?3 AND last_event_at <= ?2",
            rusqlite::params![cwd, now_ms, session_id],
        )?;
        Ok(())
    }

    /// 只在 cwd 尚为 NULL 时补写（半态会话自愈：SessionStart 落库中途失败过的会话 cwd 恒空，
    /// 对话窗因此识别不到工作区）。已有值绝不覆盖，也不动 last_event_at——这不是一次「事件」，
    /// 只是补齐建会话时该写而没写成的字段。
    pub fn backfill_session_cwd(&self, session_id: i64, cwd: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET cwd = ?1 WHERE id = ?2 AND cwd IS NULL",
            rusqlite::params![cwd, session_id],
        )?;
        Ok(())
    }

    /// 设置会话所属 agent provider（agent id，如 `"claude"` / `"kimi"`）。仅在 SessionStart 由 reporter
    /// 写一次；不动 last_event_at（同回合的 set_session_cwd 等已刷新）。
    ///
    /// 入参是**原样字符串**：store 不校验、不归一、不认识任何具体 agent。调用方传的 id 来自
    /// `meowo_agent` 注册表（`AgentId::as_str()`），已由类型保证是注册过的那批。
    pub fn set_session_provider(&self, session_id: i64, provider: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET provider = ?1 WHERE id = ?2",
            rusqlite::params![provider, session_id],
        )?;
        Ok(())
    }

    /// 记下该会话跑在哪个账号（profile）上。`None` = 默认账号，**写成 NULL**。
    ///
    /// 无条件 UPDATE（而非 None 时跳过）是有意的：本函数的语义是「把该会话的账号**设成**这个值」，
    /// None 就该把 profile 落成 NULL——若跳过，一个曾属某 profile 的会话被改回默认账号时会留着旧值。
    /// 幂等、无害；当前唯一调用方（reporter）只在有 `MEOWO_PROFILE` 时传 `Some`，故 None 分支实际不走。
    ///
    /// 恢复会话时按它注入隔离环境变量。不记的话，用户切了账号之后再打开一个旧会话，
    /// 就会拿当前活跃账号的身份去续一段不属于它的对话。
    pub fn set_session_profile(
        &self,
        session_id: i64,
        profile: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET profile = ?1 WHERE id = ?2",
            rusqlite::params![profile, session_id],
        )?;
        Ok(())
    }

    /// 该会话跑在哪个账号上（`None` = 默认账号）。
    pub fn session_profile(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        let r = self.conn.query_row(
            "SELECT profile FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        );
        match r {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 某账号（profile）名下仍处于 running/waiting 的会话数。
    ///
    /// 「合并进默认账号」的安全门：进行中的会话 reporter 仍会把 profile id 写回
    /// `sessions.profile`，此刻合并会让它们变成设置里查不到的幽灵账号。
    pub fn profile_live_session_count(&self, profile: &str) -> Result<i64, StoreError> {
        let n = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE profile = ?1 AND status IN ('running','waiting')",
            [profile],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// 把某账号名下的会话全部改挂默认账号（profile 置 NULL），返回行数。
    /// 「合并进默认账号」用：数据目录并入后，会话归属也跟着并过去。
    pub fn rehome_profile_sessions(&self, profile: &str) -> Result<usize, StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET profile = NULL WHERE profile = ?1",
            [profile],
        )?;
        Ok(n)
    }

    /// 取会话存的 cwd。
    pub fn session_cwd(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        let r = self.conn.query_row(
            "SELECT cwd FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        )?;
        Ok(r)
    }

    /// 取会话所属 agent。未知 id 原样返回；老数据的 NULL/空值回退历史默认值。
    pub fn session_provider(&self, session_id: i64) -> Result<String, StoreError> {
        let provider: Option<String> = self.conn.query_row(
            "SELECT provider FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        Ok(provider
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| crate::DEFAULT_PROVIDER.to_string()))
    }

    /// 记录会话所属进程 PID（来自 reporter 在 SessionStart 抓取的 claude.exe 父进程）。
    ///
    /// 一个 claude 进程同一时刻只属于一个会话：故先把这个 pid 从**其它**会话上摘掉
    /// （它们已被 /clear、resume、或同进程开新会话取代），否则旧会话会因进程仍存活而一直
    /// 误显示「已连接」。pid 复用（旧进程退出、号被新 claude 占用）也由此一并纠正。
    pub fn set_session_pid(
        &self,
        session_id: i64,
        pid: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        // 事务保证「本会话认领 + 旧会话收尾」原子完成，避免交错留下两个会话同持一个 pid。
        let tx = self.conn.unchecked_transaction()?;
        let claimed = tx.execute(
            "UPDATE sessions SET pid = ?1, last_event_at = ?2 WHERE id = ?3 AND last_event_at <= ?2",
            rusqlite::params![pid, now_ms, session_id],
        )?;
        if claimed == 0 {
            tx.commit()?;
            return Ok(());
        }
        // 被同一进程的新会话顶替的旧会话：直接收尾为 ended（pid 清空、记 ended_at），
        // 这样 /clear 一发生旧会话立刻从 live 列表消失，而不是只摘 pid 留个空壳。
        // 时间戳保护：只收尾 last_event_at 更旧的会话，迟到的旧会话 hook 无法反杀更活跃的新会话。
        tx.execute(
            "UPDATE sessions SET pid = NULL, status = 'ended', pending_review = NULL, ended_at = ?2, last_event_at = ?2 \
             WHERE pid = ?1 AND id <> ?3 AND status <> 'ended' \
               AND last_event_at < (SELECT last_event_at FROM sessions WHERE id = ?3)",
            rusqlite::params![pid, now_ms, session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 取所有 live(running/waiting/stale) 会话的 (id, pid, last_event_at)，供 app 做存活清理。
    pub fn live_session_liveness(&self) -> Result<Vec<(i64, Option<i64>, i64)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pid, last_event_at FROM sessions WHERE status IN ('running','waiting','stale')",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 「新建会话」面板的最近目录：去重非空 cwd，按各目录最近一次 last_event_at 倒序取前 limit。
    pub fn recent_cwds(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT cwd FROM sessions \
             WHERE cwd IS NOT NULL AND cwd <> '' \
             GROUP BY cwd \
             ORDER BY MAX(last_event_at) DESC \
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 手动归档/取消归档某会话。不更新 last_event_at，避免排序乱跳。
    /// 归档时记录 archived_at（用于「归档超过 N 天自动隐藏」）；取消归档清空。
    pub fn set_session_archived(
        &self,
        session_id: i64,
        archived: bool,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let archived_at: Option<i64> = if archived { Some(now_ms) } else { None };
        self.conn.execute(
            "UPDATE sessions SET archived = ?1, archived_at = ?2 WHERE id = ?3",
            rusqlite::params![archived as i64, archived_at, session_id],
        )?;
        Ok(())
    }
}

/// 按字符（非字节）截断，避免切坏多字节中文。
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 移除形如 `[Image #N]`（以及任意 `[Image ...]`）的占位标记。
fn strip_image_markers(s: &str) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find("[Image") {
        if let Some(rel_end) = result[start..].find(']') {
            result.replace_range(start..start + rel_end + 1, "");
        } else {
            break; // 没有闭合 ] 就停，避免死循环
        }
    }
    result
}

/// 清洗 prompt：剔除图片标记 + 折叠空白 + 去首尾空白。
fn sanitize_prompt(s: &str) -> String {
    strip_image_markers(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 无 todo -> todo；有 in_progress 或部分完成 -> doing；全 completed -> done。
fn derive_column(todos: &[TodoInput]) -> TaskColumn {
    if todos.is_empty() {
        return TaskColumn::Todo;
    }
    if todos.iter().all(|t| t.status == TodoStatus::Completed) {
        return TaskColumn::Done;
    }
    if todos.iter().any(|t| t.status != TodoStatus::Pending) {
        return TaskColumn::Doing;
    }
    TaskColumn::Todo
}

#[cfg(test)]
mod latest_session_tests {
    use super::*;

    /// 托盘点击靠它决定打开哪个会话：必须取最近活跃的、且跳过已归档的，
    /// 空库返回 None（调用方据此回落到设置窗口，而不是点了没反应）。
    #[test]
    fn latest_session_picks_most_recent_unarchived() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.latest_session_id().unwrap(), None, "空库应为 None");

        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (old, _) = store.start_session(pid, "s-old", 100).unwrap();
        let (recent, _) = store.start_session(pid, "s-recent", 500).unwrap();
        assert_eq!(store.latest_session_id().unwrap(), Some(recent));

        // 归档最新的那个 → 退回次新的，而不是继续返回已归档会话。
        store.set_session_archived(recent, true, 600).unwrap();
        assert_eq!(store.latest_session_id().unwrap(), Some(old));

        // 全部归档 → None。
        store.set_session_archived(old, true, 700).unwrap();
        assert_eq!(store.latest_session_id().unwrap(), None);
    }
}

#[cfg(test)]
mod stale_pid_tests {
    use super::*;

    /// /clear 只换会话不换进程：SessionEnd 先把旧会话收尾，随后新会话才认领同一 pid。
    /// end_session 若不清 pid，旧行会留着一个「活着但已归新会话」的 pid，接管守卫
    /// 据此把旧会话误判成「仍在外部终端运行」——而那个终端并不存在。
    #[test]
    fn end_session_clears_pid() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (old, _) = store.start_session(pid, "s-old", 100).unwrap();
        store.set_session_pid(old, 74519, 100).unwrap();
        store.end_session(old, 200).unwrap();
        assert_eq!(store.session_pid(old).unwrap(), None);
    }

    /// 存量脏数据（end_session 清 pid 之前留下的 ended+pid 行）的自愈判据：
    /// 同一 pid 已被另一条非 ended 行认领 → 本行的 pid 是换代残留，不算「本会话还活着」。
    #[test]
    fn pid_held_by_other_live_detects_reclaimed_pid() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (old, _) = store.start_session(pid, "s-old", 100).unwrap();
        let (new, _) = store.start_session(pid, "s-new", 300).unwrap();
        // 直接写库造出历史脏行：ended 但 pid 未清（end_session 已修，正常路径造不出来）。
        store
            .conn
            .execute(
                "UPDATE sessions SET status='ended', ended_at=200, pid=74519 WHERE id=?1",
                [old],
            )
            .unwrap();
        // 新会话尚未认领该 pid：没有别的活行持有它，旧行的 pid 只能按字面判活。
        assert!(!store.pid_held_by_other_live(old, 74519).unwrap());
        // 新会话认领同一 pid 后：旧行的 pid 判为换代残留。
        store.set_session_pid(new, 74519, 300).unwrap();
        assert!(store.pid_held_by_other_live(old, 74519).unwrap());
        // 认领方自己查自己：pid 归属正常，不受影响。
        assert!(!store.pid_held_by_other_live(new, 74519).unwrap());
    }
}

#[cfg(test)]
mod session_header_tests {
    use super::*;

    /// session_header 合并了原先 5 个单行查询，必须逐字保住它们各自的回退语义——
    /// 对话窗存活校正的数据源：header 必须带 pid 与 last_event_at,否则 chat 侧算不了
    /// session_connected,只能直显 DB status(进程死后 reaper 收尾前=假运行中)。
    #[test]
    fn session_header_carries_liveness_columns() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (sid, _) = store.start_session(pid, "s-live", 100).unwrap();
        let h = store.session_header(sid).unwrap();
        assert_eq!(h.pid, None);
        assert_eq!(h.last_event_at, 100);
        store.set_session_pid(sid, 4321, 200).unwrap();
        let h = store.session_header(sid).unwrap();
        assert_eq!(h.pid, Some(4321));
        assert_eq!(h.last_event_at, 200);
    }

    /// 这些回退（空 provider→默认、空白标题→None、无 task→None）都是静默的，
    /// 丢掉不会报错，只会让上层拿到错误的 agent 或把空白当标题显示。
    #[test]
    fn session_header_preserves_fallbacks_of_the_queries_it_replaced() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();

        // 新建会话：start_session 会一并建 task（带占位标题），activity 尚为空。
        // 与被替换的单查询保持一致即可，这里不假定 title 为 None。
        let (bare, _) = store.start_session(pid, "s-bare", 100).unwrap();
        let h = store.session_header(bare).unwrap();
        assert_eq!(h.cc_session_id, "s-bare");
        assert_eq!(h.title, store.session_title(bare).unwrap());
        assert_eq!(h.current_activity, None);
        // provider 为空 → 回落默认，与 session_provider 一致（不能透出空串，否则上层
        // 按未知 agent 处理，直接丢掉 transcript 能力）。start_session 会写入默认值，
        // 造不出这种旧数据，直接置 NULL / 空串来覆盖两条回退分支。
        // 列有 NOT NULL 约束，实际能出现的脏值是空串/纯空白。
        for blank in ["", "   "] {
            store
                .conn
                .execute(
                    "UPDATE sessions SET provider = ?1 WHERE id = ?2",
                    rusqlite::params![blank, bare],
                )
                .unwrap();
            let h = store.session_header(bare).unwrap();
            assert_eq!(h.provider, crate::DEFAULT_PROVIDER, "provider={blank:?}");
            assert_eq!(h.provider, store.session_provider(bare).unwrap());
        }
        // 还原，免得污染后面对 s-bare 的断言。
        store
            .set_session_provider(bare, crate::DEFAULT_PROVIDER)
            .unwrap();

        // 纯空白标题按「没有标题」处理，与 session_title 一致。
        // set_session_title 会 trim 且忽略空值，造不出这种脏数据，直接写库。
        let (blank, _) = store.start_session(pid, "s-blank", 200).unwrap();
        let blank_task = store.task_id_of_session_pub(blank).unwrap();
        store
            .conn
            .execute("UPDATE tasks SET title = '   ' WHERE id = ?1", [blank_task])
            .unwrap();
        assert_eq!(store.session_header(blank).unwrap().title, None);
        assert_eq!(store.session_title(blank).unwrap(), None);

        // 正常路径：各字段与被替换的单查询逐一一致。
        let (full, _) = store.start_session(pid, "s-full", 300).unwrap();
        store.set_session_title(full, "真实标题", 300).unwrap();
        store.set_session_cwd(full, "C:/work", 300).unwrap();
        store.set_session_provider(full, "codex").unwrap();
        let h = store.session_header(full).unwrap();
        assert_eq!(h.title.as_deref(), Some("真实标题"));
        assert_eq!(h.title, store.session_title(full).unwrap());
        assert_eq!(h.cwd, store.session_cwd(full).unwrap());
        assert_eq!(h.provider, store.session_provider(full).unwrap());
        assert_eq!(
            h.pending_review,
            store.session_pending_review(full).unwrap()
        );
        assert_eq!(
            h.current_activity,
            store.session_current_activity(full).unwrap()
        );
    }
}

#[cfg(test)]
mod recent_cwds_tests {
    use super::*;

    #[test]
    fn recent_cwds_dedups_orders_and_limits() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (id1, _) = store.start_session(pid, "s1", 100).unwrap();
        store.set_session_cwd(id1, "C:/projA", 100).unwrap();
        let (id2, _) = store.start_session(pid, "s2", 200).unwrap();
        store.set_session_cwd(id2, "C:/projB", 300).unwrap();
        let (id3, _) = store.start_session(pid, "s3", 400).unwrap();
        store.set_session_cwd(id3, "C:/projA", 500).unwrap(); // projA 再次活跃至 500

        // projA 最近活跃 500 > projB 300；projA 去重仅一条。
        assert_eq!(
            store.recent_cwds(10).unwrap(),
            vec!["C:/projA".to_string(), "C:/projB".to_string()]
        );
        // limit 生效。
        assert_eq!(store.recent_cwds(1).unwrap(), vec!["C:/projA".to_string()]);
    }
}

#[cfg(test)]
mod profile_rehome_tests {
    use super::*;

    /// rehome 把该账号名下的会话全部改挂默认账号（NULL），默认账号与别的账号的会话不动。
    #[test]
    fn rehome_moves_only_that_profiles_sessions() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (a, _) = store.start_session(pid, "s-a", 100).unwrap();
        let (b, _) = store.start_session(pid, "s-b", 200).unwrap();
        let (c, _) = store.start_session(pid, "s-c", 300).unwrap();
        store.set_session_profile(a, Some("work")).unwrap();
        store.set_session_profile(b, Some("work")).unwrap();
        store.set_session_profile(c, Some("other")).unwrap();

        assert_eq!(store.rehome_profile_sessions("work").unwrap(), 2);
        assert_eq!(store.session_profile(a).unwrap(), None);
        assert_eq!(store.session_profile(b).unwrap(), None);
        // 别的账号不受影响。
        assert_eq!(store.session_profile(c).unwrap(), Some("other".into()));
        // 再合并一次是幂等的（没有行可动）。
        assert_eq!(store.rehome_profile_sessions("work").unwrap(), 0);
    }

    /// 合并的安全门只数 running/waiting：已结束的会话不挡合并。
    #[test]
    fn live_count_only_blocks_on_running_or_waiting() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (ended, _) = store.start_session(pid, "s-ended", 100).unwrap();
        let (running, _) = store.start_session(pid, "s-running", 200).unwrap();
        store.set_session_profile(ended, Some("work")).unwrap();
        store.set_session_profile(running, Some("work")).unwrap();
        store
            .set_session_status(ended, SessionStatus::Ended, 300)
            .unwrap();

        assert_eq!(store.profile_live_session_count("work").unwrap(), 1);
        assert_eq!(store.profile_live_session_count("other").unwrap(), 0);
    }

    /// 会话卡的 profile 必须透出原始 id——app 层据此解析展示名，前端据此决定显不显示徽章。
    #[test]
    fn live_sessions_carries_profile_id() {
        let store = Store::open_in_memory().unwrap();
        let pid = store
            .upsert_project_by_root("C:/root", "root", 100)
            .unwrap();
        let (with, _) = store.start_session(pid, "s-with", 100).unwrap();
        let (without, _) = store.start_session(pid, "s-without", 200).unwrap();
        store.set_session_profile(with, Some("work")).unwrap();

        let page = store
            .live_sessions(Some("all"), None, None, None, 10)
            .unwrap();
        let find = |id: i64| page.iter().find(|s| s.session.id == id).unwrap();
        assert_eq!(find(with).profile.as_deref(), Some("work"));
        assert_eq!(find(without).profile, None);
    }
}
