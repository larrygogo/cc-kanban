//! Rust 后端与 TypeScript 前端之间的 Tauri IPC DTO。

use serde::{Deserialize, Serialize};

/// 一条待办。`status` 用字面量而非枚举：来源是各家 agent 的自由文本状态，
/// 归一化后仍可能出现本版本不认识的值，前端按未知处理即可，不该让整份反序列化失败。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct TodoDto {
    pub content: String,
    /// `pending` / `in_progress` / `completed`。
    pub status: String,
}

/// 一次子任务委派的展示信息。真正的子任务时间线不在这里——它住在 provider 的侧车流里，
/// 由 `get_subagent_transcript` 按 `ToolUse.id` 在用户展开时才读取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct SubagentRef {
    /// 委派时写的一句话任务描述（claude/kimi 的 `description` 参数）。
    pub description: String,
    /// 子 agent 类型（`subagent_type`，如 general-purpose / explore）。
    pub agent_type: Option<String>,
    /// 这次调用派出几个子任务。kimi 的 `AgentSwarm` 一次可以派出十几个，
    /// 展开前就把规模显示出来；普通单发委派为 1。
    pub count: u32,
}

/// 一次委派的结局统计。挂在**主链的工具结果**上，于是折叠状态下就能显示进度——
/// 不必先展开（展开要读侧车流，那是按需 I/O）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct SubagentOutcome {
    pub running: u32,
    pub completed: u32,
    pub failed: u32,
    /// 后台委派的任务 id（claude 启动回执里的 `agentId`）。后台子任务的真结局可能不走
    /// task-notification，而是主 agent 用 `TaskOutput` 拉取——那条回执挂在 TaskOutput 自己的
    /// 调用上，只有这个 id 能把它归回原委派。前端按「同 id 首见者为委派本体」路由。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// 一个子任务的完整时间线。一次委派可能对应多条（kimi 的 `AgentSwarm`），
/// 故 `get_subagent_transcript` 返回的是列表而不是单份 items。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct SubagentRun {
    /// 分支标签（kimi 的 `agent-3`）。单发委派没有可显示的分支名时为 None。
    pub label: Option<String>,
    /// 归一化状态：`running` / `completed` / `failed`。None = 该 provider 没有留下状态
    /// 信号（claude 的 meta.json 只记身份不记结果）。
    pub status: Option<String>,
    pub items: Vec<ChatItem>,
}

/// Provider 日志经插件解析后交给聊天归一化层的稳定消息单元。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatItem {
    UserText {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    AssistantText {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    AssistantDelta {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    /// 回合级错误（API Error 等，CC 以 model=`<synthetic>` 写盘的系统插入文案）。
    /// 独立变体而非复用 AssistantText：前端要渲染成错误气泡，不能与模型正文同皮——
    /// 「API Error: Overloaded」被当成模型的话平铺在对话里，用户读不出这是故障。
    TurnError {
        id: String,
        timestamp: Option<String>,
        /// 短中文标签（与看板卡片的 error_label 同源，见 claude 插件 classify_error）。
        label: String,
        text: String,
    },
    Reasoning {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    ReasoningDelta {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    ToolUse {
        id: String,
        timestamp: Option<String>,
        name: String,
        summary: String,
        /// Some = 这条是子任务委派（claude/kimi 的 `Agent` 工具）。委派出去的工作记在主
        /// transcript 之外的侧车流里，前端据此渲染成可展开条目，展开时才按需拉取。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent: Option<SubagentRef>,
    },
    ToolResult {
        id: String,
        timestamp: Option<String>,
        tool_use_id: Option<String>,
        text: String,
        is_error: bool,
        /// 这条结果是某次子任务委派的回执时，带上各分支的结局统计。前端按
        /// `tool_use_id` 配到对应的委派上，于是折叠状态下也能显示状态。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent: Option<SubagentOutcome>,
    },
    Meta {
        id: String,
        timestamp: Option<String>,
        kind: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "snake_case")]
pub enum PendingReviewKind {
    Approval,
    Question,
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "snake_case")]
pub enum LoginOutcome {
    Success,
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct LoginDoneEvent {
    pub operation_id: String,
    pub provider: String,
    pub outcome: LoginOutcome,
}

impl PendingReviewKind {
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "approval" => Some(Self::Approval),
            "question" => Some(Self::Question),
            "plan" => Some(Self::Plan),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct AgentModeDto {
    pub dimension: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatHistoryDto {
    #[cfg_attr(test, ts(type = "number"))]
    pub session_id: i64,
    /// CLI 侧会话 id：前端以它为键的动作（重命名 rename_session、置顶 STAR_KEY）
    /// 从对话窗标题菜单发起时需要，数字主键替代不了。
    pub cc_session_id: String,
    pub title: String,
    pub status: String,
    pub provider: String,
    pub cwd: Option<String>,
    /// 附加目录(--add-dir):会话除 cwd 外还能访问的仓。空 = 单目录会话。
    /// 标题菜单按它列出完整目录清单——加了目录必须看得见。
    pub extra_dirs: Vec<String>,
    pub supported: bool,
    pub items: Vec<ChatItem>,
    #[cfg_attr(test, ts(type = "number"))]
    pub offset: u64,
    pub reset: bool,
    pub pending_review: Option<PendingReviewKind>,
    pub model: Option<String>,
    pub agent_modes: Vec<AgentModeDto>,
    #[cfg_attr(test, ts(type = "number | null"))]
    pub context_pct: Option<i64>,
    #[cfg_attr(test, ts(type = "number | null"))]
    pub context_window: Option<i64>,
    pub current_activity: Option<String>,
    /// 会话进程是否仍被认为存活（与看板 `session_connected` 同口径：pid 在进程表里，
    /// 或距最近事件不足宽限期）。status 是 hook 驱动的离散快照，进程死后 reaper 收尾前
    /// DB 里可能残留 running——前端展示运行态必须以此校正，否则出现「假运行中」。
    pub connected: bool,
    /// 最近一轮以错误收场（transcript 分析口径，与侧栏/贴纸的 `LiveItem.errored` 同源）。
    /// 不做 transcript 分析的 agent（codex/kimi）恒为 false。
    pub errored: bool,
    /// 本 GUI 进程正托管着该会话的 PTY。决定「结束会话」入口的可见性：只有自己托管的
    /// 进程才能从 GUI 结束；外部终端里跑的会话（connected 但非托管）不该亮这个入口。
    pub pty_managed: bool,
    /// 由 agent 自己的后台守护进程托管（claude FleetView 的后台会话），与看板
    /// `LiveItem.background` 同源。这类会话既不在用户的终端里，也接管不了（杀进程会被
    /// supervisor 拉回来），输入框的引导文案必须换一套——让用户回终端的 FleetView。
    pub background: bool,
    /// 已归档（与看板 `LiveItem.archived` 同一列）。对话窗标题栏据此在「归档 / 取消归档」
    /// 之间切换：归档只改看板可见性，不动会话本身——归档态的会话照样能继续对话。
    pub archived: bool,
    /// Agent 自己维护的待办清单（快照式待办工具经 hook 落库）。空 = 该会话没有清单，
    /// 或该 agent 的待办是增量事件而非快照（当前版本的 Claude Code 即如此）。
    pub todos: Vec<TodoDto>,
    pub has_more: bool,
    /// hook 驱动的最近往来（UserPromptSubmit / Stop 落库），与 transcript 解析无关。
    /// items 为空（transcript 未落盘/未定位）或该 agent 不提供结构化 transcript 时，
    /// 前端用它们渲染临时时间线——「会话已在工作」不该显示成一片空白。
    pub last_user_text: Option<String>,
    pub last_ai_text: Option<String>,
    /// 跨 provider 接续链：本会话接替的上一段会话 id。Some 时对话页时间线头部
    /// 渲染「由上一段会话接续」的提示。
    #[cfg_attr(test, ts(type = "number | null"))]
    pub predecessor_id: Option<i64>,
    /// 本会话已被哪个后继接替。Some 时对话页渲染「已切换至…」横幅并禁发——
    /// 向被接替的会话续话会让接续链分叉。
    #[cfg_attr(test, ts(type = "number | null"))]
    pub superseded_by: Option<i64>,
}

/// `switch_session_provider` 的返回：新会话的临时负 id（前端拿它走既有的 binding
/// 轮询换真 id）+ 交接文件路径（注入 prompt 时引用；注入失败落回输入框当草稿）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct SwitchStartedDto {
    #[cfg_attr(test, ts(type = "number"))]
    pub temp_id: i64,
    pub handoff_path: String,
}

/// 接续链上的一段会话（`get_session_lineage` 的行，按时间升序）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct LineageEntryDto {
    #[cfg_attr(test, ts(type = "number"))]
    pub id: i64,
    pub provider: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub started_at: i64,
    #[cfg_attr(test, ts(type = "number | null"))]
    pub ended_at: Option<i64>,
    /// statusline 快照里的模型展示名；provider 不支持或首帧未到时缺失。
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct PendingApprovalDto {
    #[cfg_attr(test, ts(type = "number"))]
    pub session_id: i64,
    pub request_id: String,
    pub provider: String,
    pub tool_name: String,
    pub description: Option<String>,
    pub input: String,
    #[cfg_attr(test, ts(type = "unknown[]"))]
    pub permission_suggestions: Vec<serde_json::Value>,
    /// 题面卡可否在对话窗内直接作答：broker 仍在 approvals 表里持有该 request_id
    /// （PreToolUse 挂起中）时为 true。挂起结算/超时后轮询把它翻回 false，前端卡片
    /// 随之降级为纯展示。审批卡不使用该字段（恒 false）。
    #[serde(default)]
    pub answerable: bool,
}

/// 一次轮询同时取回审批与 AskUserQuestion 题面。此前是两条独立的 400ms 轮询——
/// 目标高度相关（都是「会话在等用户吗」），拆开只是白付一倍 IPC。push 事件仍是
/// 主路径，这条轮询是冷启动兜底（emit 不排队，对话窗晚开时只有轮询能补回题面）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct PendingInteractionDto {
    pub approval: Option<PendingApprovalDto>,
    pub question: Option<PendingApprovalDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct PtyOutputEvent {
    #[cfg_attr(test, ts(type = "number"))]
    pub session_id: i64,
    /// 自 PTY 启动以来，本帧首字节的绝对偏移。
    #[cfg_attr(test, ts(type = "number"))]
    pub offset: u64,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct PtyExitEvent {
    #[cfg_attr(test, ts(type = "number"))]
    pub session_id: i64,
    pub code: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct ManagedTerminalSnapshotDto {
    #[cfg_attr(test, ts(type = "number"))]
    pub session_id: i64,
    pub active: bool,
    /// 画面来源是本 GUI 托管的 PTY（而非后台会话旁路）。前端拿它判「终端可写」：
    /// 旁路快照的 active 只代表旁观连接活着，不代表能输入——曾拿 active 当判据，
    /// 恢复会话被旁路活性短路，托管 PTY 根本没起。
    pub managed: bool,
    pub data: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub start_offset: u64,
    #[cfg_attr(test, ts(type = "number"))]
    pub end_offset: u64,
    pub exited: bool,
    pub exit_code: Option<u32>,
    /// PTY 当前生效的行列数（0 = 未知：无活跃 PTY / 尚未设置尺寸）。前端在终端视图
    /// **隐藏**时用它把 xterm 网格钉到 PTY 真实尺寸——隐藏态宿主是屏外停靠盒，按盒子
    /// fit 出来的网格与 PTY 脱节，隐藏期到达的帧会按错误宽度换行/错行叠画（实拍花屏）。
    pub cols: u16,
    pub rows: u16,
    /// PTY 输出流里**此刻处于开启态**的 DEC 私有模式（备用屏 1049、鼠标上报 1000-1006、
    /// 括号粘贴 2004 等，见 pty.rs `ModeTracker::TRACKED`）。前端 reset 后回放时先按它
    /// 补写 `CSI ? n h` 作为基线：这些开关只在 TUI 启动时发一次，1MiB backlog 很快把
    /// 它们淘汰，重开窗口/重对齐的回放起点在其后——不补的话 xterm 退回主屏、关掉鼠标
    /// 上报，而 TUI 仍按全屏 + 鼠标模式画（实拍：两条滚动条、滚轮滚的不是 TUI 的内容）。
    /// 空 = 无活跃 PTY / 旁路快照 / 已退出定格。
    #[serde(default)]
    pub modes: Vec<u16>,
}

/// 工作区里有改动的一个文件（git status --porcelain 的一行归一化结果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct GitChangedFileDto {
    /// 相对仓库根的路径（正斜杠形式，与 porcelain 输出一致）。
    pub path: String,
    /// 归一化状态字母：M/A/D/T 取自 porcelain 的 X 或 Y；未跟踪（??）记为 "U"。
    pub status: String,
}

/// 对话页「Diff」按钮的可见性与弹层文件列表的数据源。
/// `git_available`/`is_repo` 任一为 false 时前端不显示入口，其余字段无意义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct GitDiffSummaryDto {
    /// git 可执行文件能否启动（未安装/被杀软拦下时为 false）。
    pub git_available: bool,
    /// cwd 是否在某个 git 工作树内。
    pub is_repo: bool,
    /// 当前分支名；detached HEAD 或查询失败时为 None。
    pub branch: Option<String>,
    pub files: Vec<GitChangedFileDto>,
}

/// 单个文件的统一 diff 文本。未跟踪文件的 `diff` 是合成的伪 diff
/// （`--- /dev/null` / `+++ b/<path>` 头 + 每行加 `+` 前缀）；二进制文件只有
/// 一行占位 "(binary file)"。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiffDto {
    pub path: String,
    pub status: String,
    pub diff: String,
    /// diff 文本超过上限被截断时为 true（前端据此显示截断提示）。
    pub truncated: bool,
}

/// 「文件」页签目录树的一个条目（list_dir_entries 的返回元素）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    /// 条目名（不含路径）。
    pub name: String,
    /// 相对会话 cwd 的路径（正斜杠形式），展开/读取时回传给后端。
    pub rel_path: String,
    pub is_dir: bool,
}

/// 「文件」页签单文件文本内容（read_file_text 的返回）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct FileTextDto {
    /// 相对会话 cwd 的路径（正斜杠形式），原样回显请求参数。
    pub rel_path: String,
    /// 文件文本；binary 为 true 时为空串（前端显示二进制占位）。
    pub text: String,
    /// 文本超过上限被截断时为 true（前端据此显示截断提示）。
    pub truncated: bool,
    /// 前 8KB 嗅探到 NUL 时为 true（按二进制处理，不返回文本）。
    pub binary: bool,
}

/// 「文件」页签搜索里一个文件内的单行内容命中。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct SearchLineHitDto {
    /// 行号（1 起）。
    pub line: u32,
    /// 命中行文本：窗口对准首个命中处（左侧截掉时带 … 前缀）、超长截断。
    pub preview: String,
}

/// 「文件」页签搜索的一个条目（按文件/目录分组，search_project_files 的返回元素）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct SearchFileHitDto {
    /// 相对会话 cwd 的路径（正斜杠形式）。
    pub rel_path: String,
    /// 目录条目为 true（目录只有名称命中，lines 恒空）。
    pub is_dir: bool,
    /// 文件/目录名本身命中了关键字。
    pub name_match: bool,
    /// 展示用的内容命中行（每文件封顶若干条）。
    pub lines: Vec<SearchLineHitDto>,
    /// 该文件内容命中的**总**行数（可大于 lines.len()，前端角标显示全量）。
    pub total_line_matches: u32,
}

/// 「文件」页签搜索结果（名称命中条目在前、内容命中在后，各按路径排序）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct SearchResultDto {
    pub files: Vec<SearchFileHitDto>,
    /// 命中数/扫描量到达上限提前收兵时为 true（前端提示细化关键词）。
    pub truncated: bool,
}

/// 「文件」页签的图片预览（read_image_preview 的返回）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct ImagePreviewDto {
    /// `data:<mime>;base64,…` 形式的图片数据；too_large 时为 None。
    pub data_url: Option<String>,
    /// 超出预览上限（base64 过大会卡 IPC/渲染），前端显示「过大」占位。
    pub too_large: bool,
}

/// 检测到的本地打开方式（文件面板「打开」按钮/菜单用，list_file_openers 的返回元素）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
#[serde(rename_all = "camelCase")]
pub struct FileOpenerDto {
    /// 回传 open_path_with 的标识：Windows 是系统关联处理器的名字（exe 路径），
    /// 其余平台是探测表里的编辑器 id（"vscode"/"cursor"…）。
    pub id: String,
    /// 菜单展示名（"Visual Studio Code"…）。
    pub name: String,
    /// 应用图标 PNG data URL；拿不到（UWP 资源图标/非 Windows 平台）为 None，前端用通用图标兜底。
    pub icon: Option<String>,
}

impl From<crate::broker::ApprovalRequest> for PendingApprovalDto {
    fn from(request: crate::broker::ApprovalRequest) -> Self {
        Self {
            session_id: request.session_id,
            request_id: request.request_id,
            provider: request.provider,
            tool_name: request.tool_name,
            description: request.description,
            input: request.input,
            permission_suggestions: request.permission_suggestions,
            // 可作答与否取决于 broker 是否还持有请求（approvals 表），不是请求本身的属性；
            // 由 emit/轮询处按持有状态覆写。
            answerable: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_snapshot_uses_the_frontend_camel_case_contract() {
        let value = serde_json::to_value(ManagedTerminalSnapshotDto {
            session_id: 7,
            active: true,
            managed: true,
            data: "QUJD".into(),
            start_offset: 10,
            end_offset: 13,
            exited: false,
            exit_code: None,
            cols: 120,
            rows: 40,
            modes: vec![1049, 1006],
        })
        .unwrap();
        assert_eq!(value["sessionId"], 7);
        assert_eq!(value["startOffset"], 10);
        assert_eq!(value["endOffset"], 13);
        assert!(value.get("start_offset").is_none());
    }

    /// GUI 边界必须走 DTO 的理由，钉成测试：原始 `broker::ApprovalRequest` 在
    /// `permission_suggestions` 为空时把字段整个 skip 掉（reporter 线路的减负），而前端类型
    /// （ts-rs 从 DTO 生成）承诺该字段**恒在**——app 曾直接把 ApprovalRequest emit 给前端，
    /// codex 的审批（从不带 suggestions）一弹就让 ChatWindow 在 `.map` 上崩掉。
    #[test]
    fn dto_always_carries_permission_suggestions_even_when_empty() {
        let request = crate::broker::ApprovalRequest {
            session_id: 7,
            request_id: "req-1".into(),
            provider: "codex".into(),
            tool_name: "Bash".into(),
            description: None,
            input: "{}".into(),
            permission_suggestions: vec![],
            pre_tool_use: false,
        };
        // 原始线路结构：空列表 → 字段消失（这正是不能拿它喂前端的原因）。
        let raw = serde_json::to_value(&request).unwrap();
        assert!(raw.get("permissionSuggestions").is_none());

        // DTO：字段恒在，空时是 `[]` 而不是缺席。
        let dto_value = serde_json::to_value(PendingApprovalDto::from(request)).unwrap();
        assert_eq!(dto_value["permissionSuggestions"], serde_json::json!([]));
        assert_eq!(dto_value["sessionId"], 7);
        assert_eq!(dto_value["requestId"], "req-1");
    }

    #[test]
    fn chat_contract_keeps_tagged_items_and_rejects_unknown_review_kinds() {
        let item = ChatItem::ToolResult {
            id: "result-1".into(),
            timestamp: None,
            tool_use_id: Some("tool-1".into()),
            text: "ok".into(),
            is_error: false,
            subagent: None,
        };
        let value = serde_json::to_value(item).unwrap();
        assert_eq!(value["type"], "tool_result");
        assert_eq!(value["tool_use_id"], "tool-1");
        // 非子任务的回执不该带这个键——旧前端与快照比对都按「缺席」理解。
        assert!(value.get("subagent").is_none());
        assert_eq!(
            PendingReviewKind::from_stored("question"),
            Some(PendingReviewKind::Question)
        );
        assert_eq!(PendingReviewKind::from_stored("future"), None);
    }
}
