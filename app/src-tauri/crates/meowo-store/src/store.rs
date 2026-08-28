use crate::error::StoreError;
use crate::migrations::SCHEMA;
use crate::models::{
    PendingReview, Project, Session, SessionStatus, Task, TaskColumn, Todo, TodoInput, TodoStatus,
};
use rusqlite::Connection;
use std::path::Path;

/// Stop 置 waiting 后,这段窗口内到达的**活动类**事件按「同回合迟到尾巴」处理,不把
/// waiting 顶回 running(判定与理由见 [`Store::touch_session`])。窗口宽度权衡:太短挡不住
/// 高负载下的 hook 进程排队,太长会拖慢「未经 UserPromptSubmit 的后台回合」亮起。
const STOP_STRAGGLER_GRACE_MS: i64 = 5_000;

/// 活动类 touch 的共用 SQL（`?1`=now、`?2`=session_id、`?3`=[`STOP_STRAGGLER_GRACE_MS`]）：
/// waiting 仅在距上次事件超过迟到窗时才翻回 running——hook 是并发子进程、reporter 按
/// **到达时刻**盖时间戳，回合末尾的 PostToolUse 完全可能晚于 Stop 落库。Stop 刚置
/// waiting 的短窗内到达的活动事件是同一回合的迟到尾巴，把它当新活动会让已结束的回合
/// 永远显示运行中（实拍：回合结束、last_ai_text 已是终稿，状态却卡 running）。
///
/// 关键：被判为迟到尾巴的那支（waiting 且窗内）**连 last_event_at 也不推进**——
/// 每条尾巴都推进窗口锚点的话，窗口会被越拖越长；且 last_event_at 是 waiting tab
/// 「等最久优先」的排序键与 watch 层多处展示口径，迟到尾巴不该把它刷成新时刻。
/// 两个 CASE 都读**更新前**的行值（SQLite 语义），判定一致。
const ACTIVITY_TOUCH_SQL: &str = "UPDATE sessions
         SET status = CASE WHEN status = 'waiting' AND ?1 - last_event_at > ?3 THEN 'running'
                           ELSE status END,
             last_event_at = CASE WHEN status = 'waiting' AND ?1 - last_event_at <= ?3
                                  THEN last_event_at ELSE ?1 END
         WHERE id = ?2 AND last_event_at <= ?1";

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
    /// 已归档（与看板 `LiveSession.archived` 同一列）。对话窗标题栏据此在「归档 / 取消
    /// 归档」之间切换——归档态只影响看板可见性，不影响会话本身能否继续对话。
    pub archived: bool,
    /// 跨 provider 接续链：本会话接替的上一段会话 id。None = 非切换产生。
    pub predecessor_id: Option<i64>,
    /// 本会话已被哪个后继接替。Some 时对话窗渲染「已切换至…」横幅并禁发——
    /// 向被接替的会话续话会让链分叉（set_session_lineage 拒绝分叉与之呼应）。
    pub superseded_by: Option<i64>,
    /// 附加目录(--add-dir,extra_dirs 列的解析结果)。空 = 单目录会话。
    /// 对话窗标题菜单按它列出会话的完整目录清单。
    pub extra_dirs: Vec<String>,
}

/// 接续链上的一段会话（session_lineage_chain 的行）。model 来自 session_context
/// 的 statusline 快照，provider 不支持或首帧未到时为 None。
#[derive(Debug, Clone)]
pub struct LineageEntry {
    pub id: i64,
    pub cc_session_id: String,
    pub provider: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub model: Option<String>,
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

    /// 只读打开一个**别的实例**拥有的库（dev 构建聚合展示安装版的会话）。
    ///
    /// 与 [`Store::open`] 的差别是刻意的，一条都不能少：
    /// - `SQLITE_OPEN_READ_ONLY`：绝不迁移/写入对方的库——dev 的新 schema 若把生产库
    ///   `execute_batch(SCHEMA)`+bump，安装版旧二进制就会按旧 SQL 读写新库，正是要根治的互踩；
    /// - 不建父目录、不 `init()`、不切 WAL（只读连接改不了 journal_mode，对方开着 WAL 时
    ///   本连接照常能读，同用户下 `-shm`/`-wal` 可读）。
    ///
    /// 调用方在查询前必须用 [`Store::user_version`] 核对版本：本方法不做任何 schema
    /// 假设，对旧版库跑新 SELECT 会直接报「no such column」。
    pub fn open_readonly<P: AsRef<Path>>(path: P) -> Result<Store, StoreError> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "busy_timeout", 3000)?;
        Ok(Store { conn })
    }

    /// 库的 `PRAGMA user_version`。配合 [`Store::open_readonly`] 做跨版本险闸：
    /// 与 [`Store::CURRENT_USER_VERSION`] 不一致就别拿本版本的 SQL 去查它。
    pub fn user_version(&self) -> Result<i64, StoreError> {
        Ok(self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// 本二进制认识的 schema 版本（对外只读暴露，险闸比对用）。
    pub const CURRENT_USER_VERSION: i64 = Self::USER_VERSION;

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
    /// v8: sessions 加 launch_args 列（新建时选的启动选项，resume/接管时回放——
    ///     权限模式是启动参数，不回放每次重启都重置成 CLI 默认）。
    /// v9: todos 加 external_id 列（claude 新版 TaskCreate/TaskUpdate 增量待办的
    ///     agent 侧编号，apply_todo_delta 靠它定位行）。
    /// v10: sessions 加 predecessor_id / superseded_by 两列（跨 provider 切换的接续链，
    ///     成对写入；superseded_by 非 NULL 的行从看板折叠隐藏）。
    /// v11: 废除 stale 会话态（全仓无写端,消费端 tab_class/候选集也不认它,是个只进不出的
    ///     半死态）：历史遗留的 stale 行归一成 waiting,读写两侧的 stale 分支一并删除;
    ///     此后合法 status 只有 running/waiting/ended。
    /// v12: 新增 work_groups 表 + sessions.work_group_id 列（多会话聚合的「工作组」）。
    ///     **功能已整体移除**（跨仓需求收敛为单会话 + --add-dir,见 v13）:结构保留不再
    ///     使用——SQLite 降版本/删列要再做一版迁移,空表零成本;全仓无任何读写端。
    /// v13: sessions 加 extra_dirs 列（附加目录 JSON 数组，claude --add-dir 的回放依据）。
    ///     老库补齐后全是 NULL = 单目录会话，正是我们要的。
    const USER_VERSION: i64 = 13;

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
        const ALTERS: [&str; 16] = [
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
            // 启动选项回放（v8）：NULL = 没选任何选项，恢复时不追加 flag。
            "ALTER TABLE sessions ADD COLUMN launch_args TEXT",
            // 增量待办的 agent 侧编号（v9）：快照路径不写，恒 NULL。
            "ALTER TABLE todos ADD COLUMN external_id TEXT",
            // 跨 provider 接续链（v10）：老库补齐后全是 NULL = 普通会话，正是我们要的。
            "ALTER TABLE sessions ADD COLUMN predecessor_id INTEGER",
            "ALTER TABLE sessions ADD COLUMN superseded_by INTEGER",
            // 工作组挂靠（v12,功能已移除):列保留,全仓无读写端。
            "ALTER TABLE sessions ADD COLUMN work_group_id INTEGER REFERENCES work_groups(id)",
            // 附加目录（v13）：老库补齐后全是 NULL = 单目录会话，正是我们要的。
            "ALTER TABLE sessions ADD COLUMN extra_dirs TEXT",
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
        // v11 数据修正：stale 态废除后老库可能残留 stale 行,归一成 waiting——它们本就
        // 「久未有事件、进程未必已死」,交给 reaper/end_orphaned_idle 按 waiting 口径收尾。
        // 失败不 bump,下次 open 重试(与 ALTER/索引同一纪律)。
        if let Err(e) = conn.execute(
            "UPDATE sessions SET status = 'waiting' WHERE status = 'stale'",
            [],
        ) {
            eprintln!("meowo-store stale 归一失败: {e}");
            return Ok(());
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
        // 会话幂等：已存在则复活（resume/--continue 场景），清掉 ended_at。复活的目标态是
        // **waiting** 不是 running——SessionStart 只说明 CLI 拉起来了、停在输入框等人，没有
        // 回合在跑（实拍反馈：每次恢复会话都被标成运行中，但并不真实）。running 由真实活动
        // （UserPromptSubmit/工具事件的 touch_session）翻转。既有 running/waiting 的行原样
        // 保留：auto-compact 会在回合中途发 SessionStart，此时降级成 waiting 才是说谎。
        // 新插入仍以 running 起步：外部终端新起的会话紧接首回合，且 waiting 起步会让
        // 「等待你回复」通知对每个外来新会话空响一次（watch.rs 的 waiting_fingerprint）。
        tx.execute(
            "INSERT INTO sessions (project_id, cc_session_id, status, started_at, last_event_at)
             VALUES (?1, ?2, 'running', ?3, ?3)
             ON CONFLICT(cc_session_id) DO UPDATE SET
                 status = CASE WHEN sessions.status = 'ended' THEN 'waiting' ELSE sessions.status END,
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
        // 用户开口 = 开新回合,无条件复活(不吃活动类 touch 的迟到窗)。
        self.touch_session_turn_open(session_id, now_ms)?;
        Ok(())
    }

    /// 活动类 touch（PostToolUse/TodoWrite 一族）,语义见 [`ACTIVITY_TOUCH_SQL`]。
    pub fn touch_session(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            ACTIVITY_TOUCH_SQL,
            rusqlite::params![now_ms, session_id, STOP_STRAGGLER_GRACE_MS],
        )?;
        Ok(())
    }

    /// 开新回合的 touch（UserPromptSubmit 的文本/纯图片两条路）：无条件把 waiting
    /// 复活为 running——用户开口就是新回合，不受迟到窗约束。
    pub fn touch_session_turn_open(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions
             SET last_event_at = ?1,
                 status = CASE WHEN status = 'waiting' THEN 'running' ELSE status END
             WHERE id = ?2 AND last_event_at <= ?1",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }

    // == Task 7: sync_todos + set_task_column + list_todos ==

    /// 用新列表整体替换某会话任务的 todos；未锁定时按 todo 推导列。
    /// hook 路径专用（TodoWrite = 真实会话活动）：顺带做活动类 touch（刷新 last_event_at、
    /// 迟到窗外把 waiting 复活为 running）。GUI 打开会话的被动重建走 `sync_todos_rebuild`。
    pub fn sync_todos(
        &self,
        session_id: i64,
        todos: &[TodoInput],
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.sync_todos_inner(session_id, todos, now_ms, true)
    }

    /// 被动重建版（GUI 打开/切换会话时从 agent 日志重建清单）：只同步 todos，**不**碰
    /// sessions 行——重建是读侧行为，不是会话活动。复用 hook 版的 touch 曾把刚恢复成
    /// waiting 的会话一开对话窗就顶回 running（实拍），还会伪造 last_event_at 给早已
    /// 断开的会话续连接宽限。
    pub fn sync_todos_rebuild(
        &self,
        session_id: i64,
        todos: &[TodoInput],
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.sync_todos_inner(session_id, todos, now_ms, false)
    }

    fn sync_todos_inner(
        &self,
        session_id: i64,
        todos: &[TodoInput],
        now_ms: i64,
        is_activity: bool,
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
        // 被动重建（GUI 读侧）**不推进 tasks.updated_at**：updated_at 是 sync_todos 挡迟到
        // 快照的权威时钟,重建把它顶到 now 会让同期在途的真实 TodoWrite（now_ms 更早）被
        // 上方守卫整份丢弃——读侧行为绝不写权威时间戳。列推导仍照做（重建的清单也要上板）。
        if !locked {
            let col = derive_column(todos);
            if is_activity {
                self.conn.execute(
                    "UPDATE tasks SET column_name = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![col.as_str(), now_ms, tid],
                )?;
            } else {
                self.conn.execute(
                    "UPDATE tasks SET column_name = ?1 WHERE id = ?2",
                    rusqlite::params![col.as_str(), tid],
                )?;
            }
        } else if is_activity {
            self.conn.execute(
                "UPDATE tasks SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now_ms, tid],
            )?;
        }
        // touch_session 等价逻辑（事务内,含迟到窗判定,见 ACTIVITY_TOUCH_SQL 注释）。
        // 仅 hook 路径（真实活动）执行,被动重建跳过,理由见 sync_todos_rebuild。
        if is_activity {
            self.conn.execute(
                ACTIVITY_TOUCH_SQL,
                rusqlite::params![now_ms, session_id, STOP_STRAGGLER_GRACE_MS],
            )?;
        }
        tx.commit()
    }

    /// 应用一条增量待办操作（claude 新版 TaskCreate/TaskUpdate）。与 `sync_todos` 的
    /// 覆盖写互斥：增量按到达顺序逐条累积，Update 靠 external_id 定位 Create 落下的行。
    ///
    /// 不做 `sync_todos` 那样的乱序挡写：增量事件天然有序（PostToolUse 按调用顺序发出），
    /// 且状态只会单调前进（pending → in_progress → completed），迟到一条顶多让状态晚一拍，
    /// 挡写反而会把真实增量整条丢掉。
    pub fn apply_todo_delta(
        &self,
        session_id: i64,
        delta: &crate::models::TodoDelta,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        use crate::models::TodoDelta;
        let tid = self.task_id_of_session(session_id)?;
        // 与 sync_todos 同理：BEGIN IMMEDIATE 开局取写锁，避免 WAL 下升级写事务
        // SQLITE_BUSY_SNAPSHOT 直接失败丢事件。
        let tx = ImmediateTx::begin(&self.conn)?;
        match delta {
            TodoDelta::Create {
                external_id,
                content,
            } => {
                // hook 重放/重复投递时同编号会再来一次 Create——更新内容而不是长出重复行。
                let updated = self.conn.execute(
                    "UPDATE todos SET content = ?1 WHERE task_id = ?2 AND external_id = ?3",
                    rusqlite::params![content, tid, external_id],
                )?;
                if updated == 0 {
                    self.conn.execute(
                        "INSERT INTO todos (task_id, content, status, order_idx, external_id)
                         VALUES (?1, ?2, 'pending',
                                 COALESCE((SELECT MAX(order_idx) + 1 FROM todos WHERE task_id = ?1), 0),
                                 ?3)",
                        rusqlite::params![tid, content, external_id],
                    )?;
                }
            }
            TodoDelta::Update {
                external_id,
                content,
                status,
                deleted,
            } => {
                if *deleted {
                    self.conn.execute(
                        "DELETE FROM todos WHERE task_id = ?1 AND external_id = ?2",
                        rusqlite::params![tid, external_id],
                    )?;
                } else {
                    let updated = self.conn.execute(
                        "UPDATE todos SET status = COALESCE(?1, status), content = COALESCE(?2, content)
                         WHERE task_id = ?3 AND external_id = ?4",
                        rusqlite::params![
                            status.map(|s| s.as_str()),
                            content.as_deref(),
                            tid,
                            external_id
                        ],
                    )?;
                    // 行不存在（meowo 半途装上、错过了 Create）且这次带了标题 → 补建自愈；
                    // 只带状态没有文字则跳过——不造一行空待办。
                    if updated == 0 {
                        if let Some(content) = content.as_deref() {
                            self.conn.execute(
                                "INSERT INTO todos (task_id, content, status, order_idx, external_id)
                                 VALUES (?1, ?2, ?3,
                                         COALESCE((SELECT MAX(order_idx) + 1 FROM todos WHERE task_id = ?1), 0),
                                         ?4)",
                                rusqlite::params![
                                    tid,
                                    content,
                                    status.unwrap_or(TodoStatus::Pending).as_str(),
                                    external_id
                                ],
                            )?;
                        }
                    }
                }
            }
        }
        // 尾部与 sync_todos 一致：未锁定时按最新全量推导看板列，刷新任务/会话时钟。
        let snapshot: Vec<TodoInput> = {
            let mut stmt = self.conn.prepare(
                "SELECT content, status FROM todos WHERE task_id = ?1 ORDER BY order_idx",
            )?;
            let rows = stmt.query_map([tid], |r| {
                Ok(TodoInput {
                    content: r.get(0)?,
                    status: TodoStatus::from_str(&r.get::<_, String>(1)?),
                })
            })?;
            rows.collect::<Result<_, _>>()?
        };
        let locked: bool = self.conn.query_row(
            "SELECT column_locked FROM tasks WHERE id = ?1",
            [tid],
            |r| Ok(r.get::<_, i64>(0)? != 0),
        )?;
        if !locked {
            let col = derive_column(&snapshot);
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
        // 活动类 touch(事务内,含迟到窗判定,见 ACTIVITY_TOUCH_SQL 注释)。
        self.conn.execute(
            ACTIVITY_TOUCH_SQL,
            rusqlite::params![now_ms, session_id, STOP_STRAGGLER_GRACE_MS],
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

    // == Task 8: 会话状态变更 ==

    /// 手动设置会话状态（如 waiting），同时更新 last_event_at。
    pub fn set_session_status(
        &self,
        session_id: i64,
        status: SessionStatus,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        if status == SessionStatus::Ended {
            return self.end_session(session_id, now_ms);
        }
        // stale 态已废除（v11,无写端也无消费端）：枚举变体仍在（跨 crate 契约,归 models.rs
        // 的所有者删）,任何来路的 Stale 一律落成 waiting——保住「库里 status 只有
        // running/waiting/ended」的不变量,tab_class/候选集才不会漏掉它。
        let status = if status == SessionStatus::Stale {
            SessionStatus::Waiting
        } else {
            status
        };
        // `status <> 'ended'` 守卫：本方法不做复活。SessionEnd 后排队迟到的 Stop 尾巴若在
        // 此把 ended 翻成 waiting（且不清 ended_at）,就绕过了 revive_if_ended_running 的
        // 窗口判据,诈尸 120s 等 end_orphaned_idle 再收——复活只走 revive_* 一族（自带
        // pid 墨迹/时间窗判据）,真实的 Stop 到达前 lookup_or_create 已完成该复活。
        self.conn.execute(
            "UPDATE sessions SET status = ?1, last_event_at = ?2 \
             WHERE id = ?3 AND last_event_at <= ?2 AND status <> 'ended'",
            rusqlite::params![status.as_str(), now_ms, session_id],
        )?;
        Ok(())
    }

    /// 设置待审批子态,同时刷新 last_event_at(让卡片排到最近活跃,并作为去重指纹)。
    ///
    /// `status <> 'ended'` 守卫：PermissionRequest/PreToolUse 走 lookup_session、可能命中
    /// 已结束的行（正常 SessionEnd 后的迟到尾巴——revive 的窗口判据会拒绝复活它）。写在
    /// 尸体上的 pending 卡片看不见、通知全哑,且没有任何生命周期事件会再清它,永久泄漏；
    /// query.rs 候选集的 `status != 'ended'` 防线由此从「防历史残留」升级为真正的不变量。
    pub fn set_pending_review(
        &self,
        session_id: i64,
        kind: PendingReview,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET pending_review = ?1, last_event_at = ?2 \
             WHERE id = ?3 AND last_event_at <= ?2 AND status <> 'ended'",
            rusqlite::params![kind.as_str(), now_ms, session_id],
        )?;
        Ok(())
    }

    /// 清除待审批子态(置 NULL)。不动 last_event_at——由同回合的兄弟调用负责时间戳。
    ///
    /// 守卫是**严格** `<`：set_pending_review 把 last_event_at 刷成置位时刻,同毫秒交错的
    /// straggler 清除（上一工具的尾巴与新审批同 ms 落库）不得抹掉刚置位的真实审批——
    /// 审批一旦被错误清掉,卡片与通知一起哑掉,agent 却还阻塞在等人。真正的清除事件
    /// （用户回复/回合结束/GUI 决策）必然晚于置位,极罕见的同 ms 合法清除顶多推迟到
    /// 下一个事件补清（审批展示侧另有 pending_review_live 的 broker 校正兜底）。
    pub fn clear_pending_review(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET pending_review = NULL WHERE id = ?1 AND last_event_at < ?2",
            rusqlite::params![session_id, now_ms],
        )?;
        Ok(())
    }

    /// [`Self::clear_pending_review`] 的定向版：只清指定 kind。PostToolUse 用——它只证明
    /// 「某个工具跑完了」,只能注销与该工具对应的那一类等待（普通工具→approval、
    /// AskUserQuestion→question、ExitPlanMode→plan）,不能顺手抹掉别的事件刚挂上的
    /// 其它等待（如同回合稍后 PreToolUse 挂的 question）。
    pub fn clear_pending_review_kind(
        &self,
        session_id: i64,
        kind: PendingReview,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET pending_review = NULL \
             WHERE id = ?1 AND pending_review = ?2 AND last_event_at < ?3",
            rusqlite::params![session_id, kind.as_str(), now_ms],
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
        // 复活为 waiting 而非 running：SessionStart 时 CLI 只是停在输入框，真跑起来由
        // UserPromptSubmit/touch_session 翻转（与 start_session 的幂等分支同一语义）。
        self.conn.execute(
            "UPDATE sessions SET status='waiting', ended_at=NULL, pending_review=NULL, last_event_at=?1 \
             WHERE id=?2 AND status='ended' AND last_event_at <= ?1",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }

    /// 活动事件的懒查路径（reporter 的 lookup_or_create）用：被误收尾成 ended 的会话
    /// （如 reap 一度不认 kimi pid）由任一活动事件复活时直接回 **running**——ended 不是
    /// Stop 写的终态，活动事件本身就是「正在干活」的证明，走 waiting 会被活动类 touch
    /// 的迟到窗压住、误 reap 自愈失效。Stop 经此复活后随手再置 waiting，语义不受影响。
    ///
    /// **不复活正常 SessionEnd 的迟到尾巴**：无条件复活曾让 SessionEnd 之后排队到达的同回合
    /// PostToolUse 把会话诈尸成 running（pid 已被 end_session 清空,只能等 end_orphaned_idle
    /// 120s 后再收）。判据取语义而非裸时间戳:
    /// - `pid IS NOT NULL` = 该 ended 由 reaper 写（end_session_if_pid 留 pid 作墨迹,
    ///   正常 SessionEnd 清 pid）——误 reap,任何时刻的活动事件都自愈;
    /// - pid 为空时仅当事件晚于 ended_at 一个迟到窗以上才复活——正常收尾的尾巴都挤在
    ///   SessionEnd 后的几秒内,而真实的后续活动（orphan 误收后用户继续用）来得更晚。
    pub fn revive_if_ended_running(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET status='running', ended_at=NULL, pending_review=NULL, last_event_at=?1 \
             WHERE id=?2 AND status='ended' AND last_event_at <= ?1 \
               AND (pid IS NOT NULL OR ?1 - COALESCE(ended_at, 0) > ?3)",
            rusqlite::params![now_ms, session_id, STOP_STRAGGLER_GRACE_MS],
        )?;
        Ok(())
    }

    /// 看板主动 resume 一个会话时调用：复活它(置 **waiting**、清 ended_at)、清空 pid、并把 last_event_at
    /// 刷成 now(作为 app 侧「resume 乐观连接」宽限期的起点；session_connected 的宽限对 waiting
    /// 同样生效,见 lib.rs 单测)。置 waiting 不置 running：刚拉起的 CLI 停在输入框等人,
    /// 没有回合在跑——running 由首个 UserPromptSubmit/工具事件翻转。
    /// 清 pid 是关键——旧进程已死，留着会被 reaper 当「pid 已死」立即再收尾；清空后 reaper「pid 未知不臆测」
    /// 不动它(见 live_session_liveness 消费方)，等新进程首个 hook 用 set_session_pid 认领真实 pid。
    /// 解决 codex 这类「session_start hook 要到首个 turn 才触发」的 agent：resume 后不必等发消息即显示已连接。
    /// 命中条件「ended ‖ pid 为空 ‖ pid=已验证死亡的那个 pid」：pid 为空即没有任何 hook 认领过、不是真连接，
    /// 可安全重置(含宽限过期后用户再次点 resume——此时 status 仍是 waiting 但 pid 空，须刷新 last_event_at
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
            "UPDATE sessions SET status='waiting', ended_at=NULL, pending_review=NULL, pid=NULL, last_event_at=?1 \
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

    /// 记录会话的启动选项选择 map（JSON 对象，option id → choice id）。claim 认领时写入；
    /// 接管时用户改选会合并后再次写入。resume/接管按 [`Self::session_launch_args`] 读出、
    /// 经插件声明表翻译成 flag 回放。
    pub fn set_session_launch_args(&self, session_id: i64, launch_args: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET launch_args = ?1 WHERE id = ?2",
            rusqlite::params![launch_args, session_id],
        )?;
        Ok(())
    }

    /// 读会话的启动选项选择 map（JSON 对象）。NULL/行不存在 → None。
    pub fn session_launch_args(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        use rusqlite::OptionalExtension;
        let r = self
            .conn
            .query_row(
                "SELECT launch_args FROM sessions WHERE id = ?1",
                [session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(r.flatten())
    }

    /// 结束会话：状态设为 ended，记录 ended_at，并清 pid。
    /// 清 pid 是必须的：/clear 只换会话不换进程，SessionEnd 先于新会话认领 pid 到达时，
    /// 旧行若留着 pid，set_session_pid 的换代清理（只扫非 ended 行）摘不掉它——此后
    /// 接管守卫拿这个「活着但已归新会话」的 pid 判活，会把旧会话误判成「仍在外部终端运行」。
    ///
    /// **无时间戳守卫**：SessionEnd 是进程亲口报的终局,而 hook 时间戳是各进程到达时刻,
    /// 高负载排队下 SessionEnd 的 now_ms 完全可能小于 straggler 刚盖上的 last_event_at——
    /// 旧守卫（`last_event_at <= now`）会把整次收尾静默丢掉,而 end 是**唯一没有自愈**的
    /// 转换,丢一次卡片就永远活着。误杀方向有自愈:resume 复活走 revive_for_resume/
    /// revive_if_ended,新活动走 revive_if_ended_running。last_event_at 取 MAX 防时钟倒退。
    pub fn end_session(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET status = 'ended', pending_review = NULL, pid = NULL, \
                 ended_at = ?1, last_event_at = MAX(last_event_at, ?1) WHERE id = ?2",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }

    /// 仅当会话仍持有调用方观察到的 pid 时收尾。用于进程快照 reaper，闭合“读旧 pid 后新进程
    /// 已重新认领同一会话”的 TOCTOU；返回 false 表示记录已变化，绝不能误杀新连接。
    ///
    /// 与 end_session 不同,**保留 pid 作墨迹**：reaper 写的 ended 是「快照里没看到进程」的
    /// 推断,可能误 reap（历史:kimi 进程名一度不被快照认作 agent）。留着 pid,后续活动事件
    /// 经 revive_if_ended_running 的 `pid IS NOT NULL` 判据即刻自愈;而正常 SessionEnd 清
    /// pid,其迟到尾巴不会诈尸。pid 已归新会话的场景由 set_session_pid 换代驱逐与
    /// pid_held_by_other_live 兜底,不受此墨迹影响。
    pub fn end_session_if_pid(
        &self,
        session_id: i64,
        observed_pid: i64,
        observed_last_event_at: i64,
        now_ms: i64,
    ) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET status='ended', pending_review=NULL, ended_at=?1, last_event_at=?1 \
             WHERE id=?2 AND pid=?3 AND last_event_at=?4 AND status<>'ended'",
            rusqlite::params![now_ms, session_id, observed_pid, observed_last_event_at],
        )?;
        Ok(n > 0)
    }

    /// resume 失败回滚专用：仅当会话**仍未被任何 hook 认领 pid** 时收尾。
    ///
    /// revive_for_resume 复活时清 pid;若随后 spawn 失败,回滚要把卡片收回「已断开」。
    /// 但复活与回滚之间新进程的 hook 可能已认领 pid（乐观复活的窗口正是给它开的）——
    /// 那说明会话真活了,回滚绝不能误杀。对称于 revive_for_resume 的 pid CAS:
    /// `pid IS NULL` 在 DB 层原子闭合,不依赖调用方时序。返回是否真的收尾了。
    pub fn end_session_if_unclaimed(
        &self,
        session_id: i64,
        now_ms: i64,
    ) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET status='ended', pending_review=NULL, ended_at=?1, \
                 last_event_at=MAX(last_event_at, ?1) \
             WHERE id=?2 AND pid IS NULL AND status <> 'ended'",
            rusqlite::params![now_ms, session_id],
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
                        s.pid, s.last_event_at, s.archived, s.predecessor_id, s.superseded_by, \
                        s.extra_dirs \
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
                        archived: row.get::<_, i64>(11)? != 0,
                        predecessor_id: row.get(12)?,
                        superseded_by: row.get(13)?,
                        // 解析失败按空处理:附加目录是增强信息,坏数据不该拦掉整个头部。
                        extra_dirs: row
                            .get::<_, Option<String>>(14)?
                            .and_then(|json| serde_json::from_str(&json).ok())
                            .unwrap_or_default(),
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

    /// 把 `new_sid` 登记为 `old_sid` 的接替者（跨 provider 切换的接续链）。
    /// 两列在同一 IMMEDIATE 事务里成对写入，链在数据层永远双向一致。
    ///
    /// 拒绝两种破坏链形的写法：自环（old == new）与分叉（old 已有 superseded_by）。
    /// 分叉检查放在 UPDATE 的 WHERE 里而不是先 SELECT：并发下两个后继同时认领
    /// 同一个前驱时，只有先到者改到行，后到者按「已被接替」失败——不产生双头链。
    pub fn set_session_lineage(&self, new_sid: i64, old_sid: i64) -> Result<(), StoreError> {
        if new_sid == old_sid {
            return Err(StoreError::InvalidInput(
                "接续链不允许自环（new_sid == old_sid）".into(),
            ));
        }
        let tx = ImmediateTx::begin(&self.conn)?;
        self.link_lineage_in_tx(new_sid, old_sid)?;
        tx.commit()
    }

    /// 接续链成对写入的事务体（调用方持有事务）：set_session_lineage 与 supersede_session 共用。
    fn link_lineage_in_tx(&self, new_sid: i64, old_sid: i64) -> Result<(), StoreError> {
        let claimed = self.conn.execute(
            "UPDATE sessions SET superseded_by = ?1 \
             WHERE id = ?2 AND superseded_by IS NULL",
            rusqlite::params![new_sid, old_sid],
        )?;
        if claimed == 0 {
            return Err(StoreError::InvalidInput(format!(
                "会话 {old_sid} 不存在或已被接替，拒绝分叉接续链"
            )));
        }
        let linked = self.conn.execute(
            "UPDATE sessions SET predecessor_id = ?1 WHERE id = ?2",
            rusqlite::params![old_sid, new_sid],
        )?;
        if linked == 0 {
            return Err(StoreError::InvalidInput(format!(
                "接替会话 {new_sid} 不存在"
            )));
        }
        Ok(())
    }

    /// /clear 换代的**单事务**收尾：结束旧段 + 启动选项随进程复制到新段 + 接续链成对写入。
    ///
    /// 此前这是三笔独立事务（end_session → set_session_launch_args → set_session_lineage），
    /// 任一步失败都留半态：旧卡孤立地活着,或用户看不见、reaper 却还管着的行。合成一个
    /// IMMEDIATE 事务后要么全落、要么全不落——整体失败时旧段仍由 agent 的 SessionEnd(clear)
    /// hook 兜底收尾,不留只走了一半的链。自环与分叉的拒绝语义与 set_session_lineage 一致
    /// （并发换代只有先到者成链,失败方一行未写）。
    pub fn supersede_session(
        &self,
        old_sid: i64,
        new_sid: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        if new_sid == old_sid {
            return Err(StoreError::InvalidInput(
                "接续链不允许自环（new_sid == old_sid）".into(),
            ));
        }
        let tx = ImmediateTx::begin(&self.conn)?;
        // 旧段结束（end_session 本体,在本事务内执行：无时间戳守卫,清 pid,MAX 防时钟倒退）。
        self.end_session(old_sid, now_ms)?;
        // 启动选项跟着终端进程走：旧段有存档才覆盖,新段日后单独 resume 才能回放权限模式等参数。
        self.conn.execute(
            "UPDATE sessions SET launch_args = (SELECT launch_args FROM sessions WHERE id = ?1) \
             WHERE id = ?2 AND (SELECT launch_args FROM sessions WHERE id = ?1) IS NOT NULL",
            rusqlite::params![old_sid, new_sid],
        )?;
        self.link_lineage_in_tx(new_sid, old_sid)?;
        tx.commit()
    }

    /// 返回 `session_id` 所在接续链的全部段，按 started_at 升序（链头最早）。
    /// 冷路径（回看弹层用），递归 CTE 先沿 predecessor_id 走到根、再沿 superseded_by
    /// 走到尾；链长实际只有几跳，无需索引。不在任何链上的会话返回仅含自身的单段。
    pub fn session_lineage_chain(&self, session_id: i64) -> Result<Vec<LineageEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE
             back(id) AS (
                 SELECT id FROM sessions WHERE id = ?1
                 UNION
                 SELECT s.predecessor_id FROM sessions s JOIN back b ON s.id = b.id
                 WHERE s.predecessor_id IS NOT NULL
             ),
             chain(id) AS (
                 -- 锚点 = 链上最早的**仍存在**的段：predecessor 为 NULL，或指向已被
                 -- 驱逐的行（链断头时从断口处开始，而不是整链查空）。
                 SELECT s.id FROM sessions s JOIN back b ON s.id = b.id
                 WHERE s.predecessor_id IS NULL
                    OR s.predecessor_id NOT IN (SELECT id FROM sessions)
                 UNION
                 SELECT s.superseded_by FROM sessions s JOIN chain c ON s.id = c.id
                 WHERE s.superseded_by IS NOT NULL
             )
             SELECT s.id, s.cc_session_id, s.provider, s.started_at, s.ended_at, sc.model
             FROM sessions s
             JOIN chain c ON s.id = c.id
             LEFT JOIN session_context sc ON sc.cc_session_id = s.cc_session_id
             ORDER BY s.started_at ASC, s.id ASC",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            let provider: Option<String> = row.get(2)?;
            Ok(LineageEntry {
                id: row.get(0)?,
                cc_session_id: row.get(1)?,
                provider: provider
                    .filter(|p| !p.trim().is_empty())
                    .unwrap_or_else(|| crate::DEFAULT_PROVIDER.to_string()),
                started_at: row.get(3)?,
                ended_at: row.get(4)?,
                model: row.get(5)?,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
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

    /// 记下会话的附加目录（JSON 字符串数组,claim 认领时写入）。resume/接管重启按它回放
    /// 附加目录 flag——不回放的话恢复的进程就丢了那些仓的访问权。store 不解析内容,原样存取。
    pub fn set_session_extra_dirs(&self, session_id: i64, json: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET extra_dirs = ?1 WHERE id = ?2",
            rusqlite::params![json, session_id],
        )?;
        Ok(())
    }

    /// 往会话的附加目录里 merge 一个新目录（已有会话中途 /add-dir 的落库半边）。
    /// 归一比较（斜杠方向 + ASCII 大小写 + 尾斜杠）去重，与主 cwd 相同的也拒绝。
    /// 返回是否真的新增——重复添加是 no-op，调用方据此决定要不要发广播。
    pub fn add_session_extra_dir(&self, session_id: i64, dir: &str) -> Result<bool, StoreError> {
        let d = dir.trim();
        if d.is_empty() {
            return Err(StoreError::InvalidInput("目录不能为空".into()));
        }
        let cwd: Option<String> = self
            .conn
            .query_row("SELECT cwd FROM sessions WHERE id = ?1", [session_id], |r| r.get(0))?;
        if cwd.as_deref().is_some_and(|c| norm_dir(c) == norm_dir(d)) {
            return Ok(false);
        }
        let mut dirs: Vec<String> = self
            .session_extra_dirs(session_id)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        if dirs.iter().any(|x| norm_dir(x) == norm_dir(d)) {
            return Ok(false);
        }
        dirs.push(d.to_string());
        let json = serde_json::to_string(&dirs)
            .map_err(|e| StoreError::InvalidInput(e.to_string()))?;
        self.set_session_extra_dirs(session_id, &json)?;
        Ok(true)
    }

    /// 移除一个附加目录（落库侧）：归一比较定位,移除后清单为空则写回 NULL(回到单目录
    /// 会话)。语义是「下次恢复不再带上」——运行中进程已持有的权限没有运行时撤销命令,
    /// 收不回,调用方文案不得承诺即时撤权。返回是否真的移除(没找到 = no-op)。
    pub fn remove_session_extra_dir(&self, session_id: i64, dir: &str) -> Result<bool, StoreError> {
        let mut dirs: Vec<String> = self
            .session_extra_dirs(session_id)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let before = dirs.len();
        dirs.retain(|x| norm_dir(x) != norm_dir(dir));
        if dirs.len() == before {
            return Ok(false);
        }
        if dirs.is_empty() {
            self.conn.execute(
                "UPDATE sessions SET extra_dirs = NULL WHERE id = ?1",
                [session_id],
            )?;
        } else {
            let json = serde_json::to_string(&dirs)
                .map_err(|e| StoreError::InvalidInput(e.to_string()))?;
            self.set_session_extra_dirs(session_id, &json)?;
        }
        Ok(true)
    }

    /// 会话的附加目录 JSON（`None` = 单目录会话）。
    pub fn session_extra_dirs(&self, session_id: i64) -> Result<Option<String>, StoreError> {
        let r = self.conn.query_row(
            "SELECT extra_dirs FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        );
        match r {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
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
        tx.execute(
            "UPDATE sessions SET pid = ?1, last_event_at = ?2 WHERE id = ?3 AND last_event_at <= ?2",
            rusqlite::params![pid, now_ms, session_id],
        )?;
        // 被同一进程的新会话顶替的旧会话：直接收尾为 ended（pid 清空、记 ended_at），
        // 这样 /clear 一发生旧会话立刻从 live 列表消失，而不是只摘 pid 留个空壳。
        //
        // 驱逐**不因认领失败而跳过**：认领 0 行只说明这条事件是本会话的迟到尾巴（时间戳
        // 不推进），pid 归属事实不变——旧代次照样要驱逐。旧代码在此 return，/clear 后
        // 恰好交错时旧行永远留着活卡（幽灵,无任何自愈路径）。
        //
        // 驱逐判据取**会话身份**优先于时间戳：`id < 认领方` = 更早创建的代次（/clear 换代、
        // pid 复用场景恒成立），同毫秒也照驱逐——裸时间戳 `<` 曾让同 δ 的旧行漏网。
        // 时间戳分支（严格 `<`）保住「同进程 /resume 回更早会话」时新近段的收尾;
        // 等时刻且 id 更大的行不动,迟到的旧会话 hook 无法反杀刚认领的新会话。
        tx.execute(
            "UPDATE sessions SET pid = NULL, status = 'ended', pending_review = NULL, \
                 ended_at = ?2, last_event_at = MAX(last_event_at, ?2) \
             WHERE pid = ?1 AND id <> ?3 AND status <> 'ended' \
               AND (id < ?3 OR last_event_at < (SELECT last_event_at FROM sessions WHERE id = ?3))",
            rusqlite::params![pid, now_ms, session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 取所有 live(running/waiting) 会话的 (id, pid, last_event_at)，供 app 做存活清理。
    pub fn live_session_liveness(&self) -> Result<Vec<(i64, Option<i64>, i64)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pid, last_event_at FROM sessions WHERE status IN ('running','waiting')",
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

/// 目录的去重比较键:斜杠方向归一 + 去尾斜杠 + ASCII 不分大小写(Windows 路径习惯)。
/// add/remove_session_extra_dir 共用——两边口径不一致会出现「删不掉刚加的目录」。
fn norm_dir(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase()
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
