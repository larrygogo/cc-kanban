pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    root_path   TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- pid/cwd/archived/archived_at 新库直接建在表里；旧库由 Store::migrate 的 ALTER 补齐。
CREATE TABLE IF NOT EXISTS sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    INTEGER NOT NULL REFERENCES projects(id),
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
    -- agent provider：claude（默认）/ kimi…，驱动卡片图标/标签与 resume 命令。旧库由 migrate 的 ALTER 补齐。
    -- 此 'claude' 默认值须与 DEFAULT_PROVIDER 常量、store.rs ALTER 语句中的 'claude' 一致；
    -- 改默认 provider 时三处都要改，models.rs 的绊线测试会在改常量时提醒。
    provider       TEXT NOT NULL DEFAULT 'claude',
    -- 多账号：该会话跑在哪个 profile（账号）上。**NULL = 默认账号**（agent 自己的目录），
    -- 老会话与不经 meowo 启动的会话都是 NULL——恰好是对的。
    --
    -- 恢复会话必须按它注入隔离环境变量，回到**同一个**账号：用当前活跃账号去 resume 一个旧会话，
    -- 会拿错误的身份去续一段不属于它的对话。
    profile        TEXT,
    -- 启动选项的**选择 map**（JSON 对象，option id → choice id，如 {"permission":
    -- "bypassPermissions"}）。新建时的选择在 claim 认领时写入；resume/接管重启进程时
    -- 按插件声明表把它现场翻译成 flag 回放——权限模式等选项是**启动参数**而非会话状态，
    -- 不回放的话每次重启都重置成 CLI 默认。接管时用户改了权限模式会合并写回（成为
    -- 会话新的持久形态）。存选择而非 flag：CLI 改 flag 名不受影响，且能按选项维度合并。
    -- NULL = 从未选过任何选项（或会话非 meowo 启动），恢复时不追加任何 flag。
    launch_args    TEXT
);

CREATE TABLE IF NOT EXISTS tasks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id       INTEGER NOT NULL REFERENCES projects(id),
    session_id       INTEGER REFERENCES sessions(id),
    title            TEXT NOT NULL,
    column_name      TEXT NOT NULL,
    column_locked    INTEGER NOT NULL DEFAULT 0,
    current_activity TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS todos (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id   INTEGER NOT NULL REFERENCES tasks(id),
    content   TEXT NOT NULL,
    status    TEXT NOT NULL,
    order_idx INTEGER NOT NULL
);

-- events: 预留给后续计划的事件审计流，当前管线尚未写入。
CREATE TABLE IF NOT EXISTS events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER REFERENCES sessions(id),
    kind       TEXT NOT NULL,
    payload    TEXT,
    created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_tasks_session ON tasks(session_id) WHERE session_id IS NOT NULL;

-- session_context: 来自 Claude Code statusline 的上下文用量（准确窗口与百分比）。
-- 按 cc_session_id 主键 upsert；statusline 每次渲染刷新。
CREATE TABLE IF NOT EXISTS session_context (
    cc_session_id TEXT PRIMARY KEY,
    used_pct      INTEGER,
    window_size   INTEGER,
    model         TEXT,
    updated_at    INTEGER NOT NULL
);

-- session_notes: 用户给会话挂的本地便签（手写备忘），按 cc_session_id 主键 upsert；
-- 清空便签即删除该行。与 transcript 标题/CC 数据无关，纯用户私有。
CREATE TABLE IF NOT EXISTS session_notes (
    cc_session_id TEXT PRIMARY KEY,
    note          TEXT NOT NULL,
    updated_at    INTEGER NOT NULL
);
"#;
