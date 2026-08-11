use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Waiting,
    Ended,
    Stale,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Running => "running",
            SessionStatus::Waiting => "waiting",
            SessionStatus::Ended => "ended",
            SessionStatus::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskColumn {
    Todo,
    Doing,
    Done,
}

impl TaskColumn {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskColumn::Todo => "todo",
            TaskColumn::Doing => "doing",
            TaskColumn::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }
    /// 从各家待办工具的 status 字符串映射；无副作用、未知值降级为 Pending，
    /// 故用中缀方法而非 fallible 的 std FromStr。
    ///
    /// **各家用词不同**，必须都认：claude 的 TodoWrite 写 `completed`，kimi 的 TodoList
    /// 写 `done`（本机会话实测：done 910 次、pending 522、in_progress 224）。此前只认
    /// `completed`，kimi 的已完成项全部降级成 Pending——界面上一条都勾不上（0/4）。
    /// 连字符/驼峰等写法一并归一，避免下一个 agent 换个拼法又静默失效。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> TodoStatus {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str()
        {
            "in_progress" | "inprogress" | "doing" | "active" | "started" => TodoStatus::InProgress,
            "completed" | "done" | "complete" | "finished" | "closed" => TodoStatus::Completed,
            _ => TodoStatus::Pending,
        }
    }
}

/// 待审批子态:回合中途等用户介入的三种情形。NULL 态在 store 层用 Option 表达,枚举不含 None 变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingReview {
    Approval,
    Question,
    Plan,
}

impl PendingReview {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingReview::Approval => "approval",
            PendingReview::Question => "question",
            PendingReview::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub root_path: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct Session {
    #[cfg_attr(test, ts(type = "number"))]
    pub id: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub project_id: i64,
    pub cc_session_id: String,
    pub status: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub started_at: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub last_event_at: i64,
    #[cfg_attr(test, ts(type = "number | null"))]
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    pub session_id: Option<i64>,
    pub title: String,
    pub column: String,
    pub column_locked: bool,
    pub current_activity: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct Todo {
    #[cfg_attr(test, ts(type = "number"))]
    pub id: i64,
    #[cfg_attr(test, ts(type = "number"))]
    pub task_id: i64,
    pub content: String,
    pub status: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub order_idx: i64,
}

/// 上报器同步 todo 时的输入项。
#[derive(Debug, Clone, PartialEq)]
pub struct TodoInput {
    pub content: String,
    pub status: TodoStatus,
}

/// 默认 agent provider 名，与 sessions.provider 列的 SQL DEFAULT 'claude'
/// （migrations.rs 建表 + store.rs ALTER）必须保持一致。此常量改动时下方测试会变红、
/// 提醒同步 SQL 字面量，但若只改 SQL 而不改此常量则无法被发现（单向绊线）。
///
/// 这是本 crate 里**唯一**一处提到具体 agent 的地方，且仅因为它是历史 schema 的默认值。
/// `sessions.provider` 一律按原样字符串读写：store 不认识 claude/kimi/codex，也不该认识——
/// 身份解析归 `meowo_agent::resolve`，加 agent 不必动 DB 层。
pub const DEFAULT_PROVIDER: &str = "claude";

#[cfg(test)]
mod todo_status_tests {
    use super::*;

    /// 各家待办工具的状态用词不同，映射漏一个就是**静默失效**——不报错，只是把已完成项
    /// 降级成 Pending，界面上一条都勾不上。实测：claude 写 `completed`，kimi 写 `done`。
    #[test]
    fn maps_each_agents_wording_for_completed_and_in_progress() {
        for s in [
            "completed",
            "done",
            "complete",
            "finished",
            "Done",
            " DONE ",
        ] {
            assert_eq!(
                TodoStatus::from_str(s),
                TodoStatus::Completed,
                "{s:?} 应视为已完成"
            );
        }
        for s in [
            "in_progress",
            "in-progress",
            "inProgress",
            "doing",
            "active",
        ] {
            assert_eq!(
                TodoStatus::from_str(s),
                TodoStatus::InProgress,
                "{s:?} 应视为进行中"
            );
        }
        // 未知值降级为 Pending，而不是让整份解析失败。
        for s in ["pending", "todo", "", "某种没见过的状态"] {
            assert_eq!(TodoStatus::from_str(s), TodoStatus::Pending, "{s:?} 应降级");
        }
    }

    /// 落库用的规范值不能跟着别名漂移——DTO 与前端按它比较。
    #[test]
    fn canonical_strings_are_stable() {
        assert_eq!(TodoStatus::Completed.as_str(), "completed");
        assert_eq!(TodoStatus::InProgress.as_str(), "in_progress");
        assert_eq!(TodoStatus::Pending.as_str(), "pending");
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn default_provider_matches_schema_literal() {
        // 单向绊线：改 DEFAULT_PROVIDER 常量时此断言变红，提醒同步 migrations.rs 的建表
        // 默认值与 store.rs 的 ALTER 语句。但此测试**不会**检测「只改 SQL 字面量而不改常量」
        // 的情况，需在 migrations.rs / store.rs 的 SQL 行旁记下关联注释自行守护。
        assert_eq!(DEFAULT_PROVIDER, "claude");
    }
}
