use meowo_store::{TodoDelta, TodoInput, TodoStatus};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct HookEvent {
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    /// SessionStart 的触发来源（claude：`startup` / `resume` / `clear` / `compact`）。
    /// compact 是回合中途的续接：其 cwd 是 Bash 持久 shell cd 漂移后的当前目录，不是
    /// 会话工作区，落库判断要靠它区分。其他 agent 无此字段时为 None。
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// 用户输入。Claude 为纯字符串；kimi-code 为内容块数组 `[{"type":"text","text":...}]`。
    /// 存成 Value 兼容两者（否则 kimi 的数组会让整个事件反序列化失败），取文本走 `prompt_text()`。
    #[serde(default)]
    pub prompt: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    /// PostToolUse 携带的工具结果。目前只有增量待办要读它——TaskCreate 分配的任务编号
    /// 不在 tool_input 里，只出现在结果文本 `Task #N created successfully: …` 中。
    #[serde(default)]
    pub tool_response: Option<serde_json::Value>,
    /// Claude PermissionRequest 提供的“本次允许之外”的原生选项（例如写入项目/用户权限规则）。
    /// 其他 Agent 没有该字段时保持空列表。
    #[serde(default)]
    pub permission_suggestions: Vec<serde_json::Value>,
    /// 回合结束时 hook 携带的最近一条 AI 正文。各家字段名不同，靠 alias 收束到同一个字段：
    /// claude/codex 是 `last_assistant_message`，kimi 是 `assistant_message`，
    /// gemini 的 `AfterAgent` 叫 `prompt_response`。
    #[serde(default, alias = "assistant_message", alias = "prompt_response")]
    pub last_assistant_message: Option<String>,
}

/// 各家的字段名不同：claude 的 `TodoWrite` 用 `content`，kimi 的 `TodoList` 用 `title`。
/// 两者都只是「这条待办的文字」，用 alias 收进同一个字段，不必为此分叉解析。
#[derive(Debug, Deserialize)]
struct RawTodo {
    // 内容字段可选：缺 content/title 的行不丢，由调用方补占位文案——静默丢掉会让
    // 快照只剩部分行，进度计数失真（宁可占位，不可静默丢）。
    #[serde(alias = "title", alias = "subject", alias = "text")]
    content: Option<String>,
    #[serde(default)]
    status: String,
}

impl HookEvent {
    pub fn parse(s: &str) -> Result<HookEvent, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// 投影成 agent 能力所需的那几个字段。能力层刻意不认识 `HookEvent` 本身——它依赖
    /// `meowo_store::TodoInput`，让插件层反向依赖 DB 层。
    pub fn agent_ctx(&self) -> meowo_agent::HookContext<'_> {
        meowo_agent::HookContext {
            session_id: &self.session_id,
            transcript_path: self.transcript_path.as_deref(),
            last_assistant_message: self.last_assistant_message.as_deref(),
        }
    }

    /// 从 tool_input.todos 提取 TodoInput 列表。
    ///
    /// 返回 `Option` 是为区分两种「空」：`todos` 键缺失/不是数组 → `None`——这不是
    /// 清单语义（旧版 claude 有同名的读操作调用，根本不带 todos 参数），调用方必须
    /// 跳过同步，否则空列表会把整份待办表 DELETE 掉；显式 `todos: []` →
    /// `Some(vec![])`，agent 明确清空是合法语义，照清。
    pub fn todo_items(&self) -> Option<Vec<TodoInput>> {
        let input = self.tool_input.as_ref()?;
        let arr = input.get("todos")?.as_array()?;
        Some(
            arr.iter()
                .filter_map(|v| serde_json::from_value::<RawTodo>(v.clone()).ok())
                .map(|t| TodoInput {
                    // 缺内容字段的行给占位文案保留进快照（理由见 RawTodo 注释）。
                    content: t.content.unwrap_or_else(|| "（未命名事项）".to_string()),
                    status: TodoStatus::from_str(&t.status),
                })
                .collect(),
        )
    }

    /// 从增量待办工具（claude 的 TaskCreate/TaskUpdate）的调用中提取一条 TodoDelta。
    ///
    /// 不看工具名、靠字段区分：带 `taskId` 的是更新，带 `subject` 无 `taskId` 的是新建
    /// ——与 `RawTodo` 用 alias 收束各家字段名同一思路，将来别家的增量工具字段对得上
    /// 就直接复用。字段缺失（既无 taskId 也无 subject）返回 None，调用方降级为无操作。
    pub fn todo_delta(&self) -> Option<TodoDelta> {
        let input = self.tool_input.as_ref()?;
        // taskId 可能是字符串 "1" 也可能是数字 1，统一成字符串存。
        let task_id = input.get("taskId").and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
        let subject = input
            .get("subject")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(external_id) = task_id {
            let status_raw = input.get("status").and_then(|v| v.as_str());
            // "deleted" 是删行而不是一种状态；TodoStatus::from_str 会把它降级成 Pending，
            // 必须在映射前截住。
            let deleted = status_raw == Some("deleted");
            return Some(TodoDelta::Update {
                external_id,
                content: subject,
                status: status_raw
                    .filter(|_| !deleted)
                    .map(TodoStatus::from_str),
                deleted,
            });
        }
        let content = subject?;
        // 新建的编号只在结果文本里；抠不到（PreToolUse 还没结果 / CC 改了结果文案）不能
        // 把这条 Create 整条丢掉——用 subject 拼一个确定性占位编号先落行，后续 TaskUpdate
        // 带着同一 subject 与真编号到达时，由 store 层把占位行换绑到真编号
        // （"pending-" 前缀是 hook 与 store 两侧的约定，见 apply_todo_delta）。
        let external_id = self
            .tool_response
            .as_ref()
            .and_then(response_text)
            .and_then(|t| task_number(&t))
            .unwrap_or_else(|| format!("pending-{content}"));
        Some(TodoDelta::Create {
            external_id,
            content,
        })
    }

    /// 把用户输入规整成纯文本：Claude 的字符串原样；kimi 的内容块数组拼接各 text 块（忽略图片等非文本块）。
    pub fn prompt_text(&self) -> Option<String> {
        match self.prompt.as_ref()? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Array(arr) => {
                let s = arr
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("");
                (!s.is_empty()).then_some(s)
            }
            _ => None,
        }
    }

    /// 取 Bash 工具的 command 字段（用于「当前动作」显示）。
    pub fn bash_command(&self) -> Option<String> {
        self.tool_input
            .as_ref()?
            .get("command")?
            .as_str()
            .map(|s| s.to_string())
    }
}

/// 把 tool_response 摊平成文本。hook 的结果字段形状不统一：可能是裸字符串、内容块数组
/// `[{"type":"text","text":…}]`，或再包一层 `{"content": …}`——三种都见过，逐层剥。
fn response_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let s = arr
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            (!s.is_empty()).then_some(s)
        }
        serde_json::Value::Object(o) => o
            .get("content")
            .and_then(response_text)
            .or_else(|| o.get("text").and_then(|t| t.as_str()).map(String::from)),
        _ => None,
    }
}

/// 从 `Task #N created successfully: …` 里抠出编号 N。手写扫描而非正则——本 crate 是
/// hook 热路径，不为一个模式引 regex 依赖。
fn task_number(text: &str) -> Option<String> {
    let rest = &text[text.find("Task #")? + "Task #".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then_some(digits)
}
