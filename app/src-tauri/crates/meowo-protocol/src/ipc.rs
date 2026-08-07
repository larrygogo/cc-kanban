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
    pub title: String,
    pub status: String,
    pub provider: String,
    pub cwd: Option<String>,
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
    pub data: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub start_offset: u64,
    #[cfg_attr(test, ts(type = "number"))]
    pub end_offset: u64,
    pub exited: bool,
    pub exit_code: Option<u32>,
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
            data: "QUJD".into(),
            start_offset: 10,
            end_offset: 13,
            exited: false,
            exit_code: None,
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
