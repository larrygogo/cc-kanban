//! 跨 provider 切换的上下文交接：把一段会话的 [`ChatItem`] 时间线渲染成一份
//! 自解释的 markdown 交接文件，由目标 agent 用自己的读文件工具阅读。
//!
//! 为什么是「落盘文件 + 一句引用」而不是把历史直接打进新会话的 PTY：真实会话的
//! 历史轻松几百 KB，PTY 单次写有 64 KiB 上限（`PtyBroker::write`），分块写 bracketed
//! paste 的跨界行为未经验证；文件路径一句话则天然绕开长度与转义问题。
//!
//! 本模块是 **provider 无关纯函数**：输入已归一化的 `ChatItem`，不认识任何具体 agent
//! ——from/to 只是展示名字符串。宿主的守卫测试（host_code_does_not_branch_on_agent_identity）
//! 与此纪律同源。

use meowo_protocol::ipc::ChatItem;

/// 交接文件头部所需的会话元信息。全部是展示用字符串，缺失就不写对应行。
pub struct HandoffMeta<'a> {
    /// 来源 agent 展示名（如 "Claude Code"）。
    pub from_provider_name: &'a str,
    /// 目标 agent 展示名。
    pub to_provider_name: &'a str,
    pub cwd: Option<&'a str>,
    /// 原会话模型展示名（statusline 快照，可能缺失）。
    pub model: Option<&'a str>,
    pub title: Option<&'a str>,
}

/// 单条工具结果在交接文件里保留的最大字符数。`ChatItem::ToolResult.text` 在解析期
/// 已被截到 4000，这里再收紧：交接的读者是要**续接任务**的 agent，工具输出细节的
/// 价值远低于对话正文，1200 字符足够回忆起「做过什么、结果如何」。
const TOOL_RESULT_MAX_CHARS: usize = 1200;

/// 交接文件正文（不含文件头）的字符上限。目标 agent 要把整份文件读进上下文窗口，
/// 无限历史装不下；超限时从**头部**丢弃整条消息——最新的内容离当前任务最近。
const BODY_MAX_CHARS: usize = 2 * 1024 * 1024;

/// 把会话时间线渲染成交接 markdown。
///
/// delta 归并与相邻去重**语义对齐前端 `reduceChatEvents`**（app/src/chat/reducer.ts）：
/// delta 并入紧邻的同类前项、否则自立门户；相邻且全等的 user/reasoning 消除。
/// 这样交接文件与用户在对话页看到的时间线一致，不会平白多出重复段。
pub fn render_history(items: &[ChatItem], meta: &HandoffMeta) -> String {
    let reduced = reduce(items);

    // 每条消息渲染成独立块，先收集再按预算从头丢弃。
    let mut blocks: Vec<String> = Vec::new();
    for item in &reduced {
        if let Some(block) = render_item(item, meta) {
            blocks.push(block);
        }
    }

    let total: usize = blocks.iter().map(|b| b.chars().count()).sum();
    let mut omitted = 0usize;
    if total > BODY_MAX_CHARS {
        let mut kept = 0usize;
        // 从尾部往前累计预算，越过预算线的头部整块丢弃。
        let mut cut = blocks.len();
        for (idx, block) in blocks.iter().enumerate().rev() {
            kept += block.chars().count();
            if kept > BODY_MAX_CHARS {
                cut = idx + 1;
                break;
            }
        }
        omitted = cut;
        blocks.drain(..cut);
    }

    let mut out = String::new();
    render_header(&mut out, meta);
    if omitted > 0 {
        out.push_str(&format!("> （更早的 {omitted} 条消息已省略）\n\n"));
    }
    for block in &blocks {
        out.push_str(block);
        out.push('\n');
    }
    out
}

fn render_header(out: &mut String, meta: &HandoffMeta) {
    out.push_str("# 会话交接\n\n");
    out.push_str(&format!(
        "本文件由 meowo 自动生成。用户正在把一段进行中的会话从 {} 切换到 {} 继续。\n\n",
        meta.from_provider_name, meta.to_provider_name
    ));
    if let Some(title) = meta.title.filter(|t| !t.trim().is_empty()) {
        out.push_str(&format!("- 会话标题：{title}\n"));
    }
    if let Some(cwd) = meta.cwd.filter(|c| !c.trim().is_empty()) {
        out.push_str(&format!("- 工作目录：{cwd}\n"));
    }
    if let Some(model) = meta.model.filter(|m| !m.trim().is_empty()) {
        out.push_str(&format!("- 原会话模型：{model}\n"));
    }
    out.push_str(
        "- 下面是原会话的对话记录。工具调用只保留摘要、工具输出有截断；图片以本地文件\
         路径引用（形如 `[Image: source: <路径>]`），需要时可用你的读文件工具查看。\n\n\
         请通读后**直接继续其中的任务**：不要重复已完成的工作，不要向用户复述本文件内容。\n\n\
         ---\n\n",
    );
}

/// 单条消息 → markdown 块（尾带一个换行；调用方再补一个空行）。None = 该变体不进交接。
fn render_item(item: &ChatItem, meta: &HandoffMeta) -> Option<String> {
    match item {
        ChatItem::UserText { text, .. } => Some(format!("## 用户\n\n{text}\n")),
        ChatItem::AssistantText { text, .. } => Some(format!(
            "## 助手（{}）\n\n{text}\n",
            meta.from_provider_name
        )),
        // 内部思考对续接方没有交接价值且体量大；Meta 是渲染提示（compact 边界等）。
        ChatItem::Reasoning { .. } | ChatItem::Meta { .. } => None,
        // delta 在 reduce 阶段已并入最终件，落到这里说明流末尾有孤儿 delta——按正文对待。
        ChatItem::AssistantDelta { text, .. } => Some(format!(
            "## 助手（{}）\n\n{text}\n",
            meta.from_provider_name
        )),
        ChatItem::ReasoningDelta { .. } => None,
        ChatItem::TurnError { label, text, .. } => {
            Some(format!("> ⚠ 回合错误（{label}）：{text}\n"))
        }
        ChatItem::ToolUse { name, summary, .. } => {
            if summary.trim().is_empty() {
                Some(format!("- [工具] {name}\n"))
            } else {
                Some(format!("- [工具] {name}: {summary}\n"))
            }
        }
        ChatItem::ToolResult { text, is_error, .. } => {
            let trimmed = text.trim_end();
            if trimmed.is_empty() {
                return None;
            }
            let mut body: String = trimmed.chars().take(TOOL_RESULT_MAX_CHARS).collect();
            if trimmed.chars().count() > TOOL_RESULT_MAX_CHARS {
                body.push_str("\n…（已截断）");
            }
            let tag = if *is_error { "[失败] " } else { "" };
            // 缩进代码块而不是 fenced：工具输出自己可能含 ``` 围栏，嵌套会把文档撕开。
            let indented = body
                .lines()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            Some(format!("  {tag}输出：\n\n{indented}\n"))
        }
    }
}

/// delta 归并 + 相邻去重，语义对齐前端 `reduceChatEvents`。
fn reduce(items: &[ChatItem]) -> Vec<ChatItem> {
    let mut out: Vec<ChatItem> = Vec::new();
    for item in items {
        match item {
            ChatItem::AssistantDelta { id, timestamp, text } => {
                if let Some(ChatItem::AssistantText { text: prev, .. }) = out.last_mut() {
                    prev.push_str(text);
                } else {
                    out.push(ChatItem::AssistantText {
                        id: id.clone(),
                        timestamp: timestamp.clone(),
                        text: text.clone(),
                    });
                }
            }
            ChatItem::ReasoningDelta { id, timestamp, text } => {
                if let Some(ChatItem::Reasoning { text: prev, .. }) = out.last_mut() {
                    prev.push_str(text);
                } else {
                    out.push(ChatItem::Reasoning {
                        id: id.clone(),
                        timestamp: timestamp.clone(),
                        text: text.clone(),
                    });
                }
            }
            ChatItem::UserText { text, .. } => {
                if let Some(ChatItem::UserText { text: prev, .. }) = out.last() {
                    if prev == text {
                        continue; // 相邻全等 = 双日志入口的重复记录
                    }
                }
                out.push(item.clone());
            }
            ChatItem::Reasoning { text, .. } => {
                if let Some(ChatItem::Reasoning { text: prev, .. }) = out.last() {
                    if prev == text {
                        continue;
                    }
                }
                out.push(item.clone());
            }
            other => out.push(other.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta<'a>() -> HandoffMeta<'a> {
        HandoffMeta {
            from_provider_name: "Claude Code",
            to_provider_name: "Codex",
            cwd: Some("C:/work/proj"),
            model: Some("Opus 5"),
            title: Some("修登录 bug"),
        }
    }

    fn user(text: &str) -> ChatItem {
        ChatItem::UserText {
            id: "u".into(),
            timestamp: None,
            text: text.into(),
        }
    }

    fn assistant(text: &str) -> ChatItem {
        ChatItem::AssistantText {
            id: "a".into(),
            timestamp: None,
            text: text.into(),
        }
    }

    fn delta(text: &str) -> ChatItem {
        ChatItem::AssistantDelta {
            id: "d".into(),
            timestamp: None,
            text: text.into(),
        }
    }

    #[test]
    fn header_carries_meta_and_instructions() {
        let out = render_history(&[], &meta());
        assert!(out.contains("# 会话交接"));
        assert!(out.contains("Claude Code 切换到 Codex"));
        assert!(out.contains("修登录 bug"));
        assert!(out.contains("C:/work/proj"));
        assert!(out.contains("Opus 5"));
        assert!(out.contains("直接继续其中的任务"));
    }

    #[test]
    fn header_skips_missing_meta_lines() {
        let m = HandoffMeta {
            from_provider_name: "A",
            to_provider_name: "B",
            cwd: None,
            model: None,
            title: None,
        };
        let out = render_history(&[], &m);
        assert!(!out.contains("会话标题"));
        assert!(!out.contains("工作目录"));
        assert!(!out.contains("原会话模型"));
    }

    #[test]
    fn deltas_merge_into_adjacent_assistant_text_like_the_frontend_reducer() {
        // 纯 delta 流：拼成一条。
        let out = render_history(&[delta("你"), delta("好")], &meta());
        assert!(out.contains("你好"));
        assert_eq!(out.matches("## 助手").count(), 1);

        // delta 紧随最终件：并入而不是另起一条（与 reduceChatEvents 同语义）。
        let out = render_history(&[assistant("前半"), delta("后半")], &meta());
        assert!(out.contains("前半后半"));
        assert_eq!(out.matches("## 助手").count(), 1);

        // 中间隔了别的消息：delta 自立门户。
        let out = render_history(&[assistant("一"), user("插话"), delta("二")], &meta());
        assert_eq!(out.matches("## 助手").count(), 2);
    }

    #[test]
    fn adjacent_identical_user_and_reasoning_lines_dedupe() {
        let out = render_history(&[user("同一句"), user("同一句"), user("下一句")], &meta());
        assert_eq!(out.matches("同一句").count(), 1);
        assert!(out.contains("下一句"));
    }

    #[test]
    fn reasoning_and_meta_are_dropped() {
        let items = [
            ChatItem::Reasoning {
                id: "r".into(),
                timestamp: None,
                text: "内心戏".into(),
            },
            ChatItem::Meta {
                id: "m".into(),
                timestamp: None,
                kind: "compact".into(),
            },
            user("正文"),
        ];
        let out = render_history(&items, &meta());
        assert!(!out.contains("内心戏"));
        assert!(!out.contains("compact"));
        assert!(out.contains("正文"));
    }

    #[test]
    fn tool_use_and_result_render_with_truncation_and_error_tag() {
        let long = "x".repeat(TOOL_RESULT_MAX_CHARS + 100);
        let items = [
            ChatItem::ToolUse {
                id: "t1".into(),
                timestamp: None,
                name: "Bash".into(),
                summary: "cargo test".into(),
                subagent: None,
                detail: None,
            },
            ChatItem::ToolResult {
                id: "t1r".into(),
                timestamp: None,
                tool_use_id: Some("t1".into()),
                text: long,
                is_error: true,
                subagent: None,
            },
        ];
        let out = render_history(&items, &meta());
        assert!(out.contains("- [工具] Bash: cargo test"));
        assert!(out.contains("[失败] 输出："));
        assert!(out.contains("…（已截断）"));
        // 截断确实生效：正文里连续 x 不超过上限。
        assert!(!out.contains(&"x".repeat(TOOL_RESULT_MAX_CHARS + 1)));
    }

    #[test]
    fn turn_error_renders_as_quote() {
        let items = [ChatItem::TurnError {
            id: "e".into(),
            timestamp: None,
            label: "限流".into(),
            text: "API Error: Overloaded".into(),
        }];
        let out = render_history(&items, &meta());
        assert!(out.contains("> ⚠ 回合错误（限流）：API Error: Overloaded"));
    }

    #[test]
    fn over_budget_drops_whole_messages_from_the_head() {
        // 每条 0.9M 字符：三条 2.7M 超预算，尾部两条 1.8M 在预算内 → 恰好丢头部一条。
        let big = "很".repeat(900_000);
        let items = [
            user(&format!("第一条 {big}")),
            user(&format!("第二条 {big}")),
            user(&format!("第三条 {big}")),
        ];
        let out = render_history(&items, &meta());
        assert!(out.contains("# 会话交接"), "文件头必须保留");
        assert!(out.contains("更早的 1 条消息已省略"));
        assert!(!out.contains("第一条"));
        assert!(out.contains("第二条"));
        assert!(out.contains("第三条"));
    }

    #[test]
    fn empty_history_returns_header_only() {
        let out = render_history(&[], &meta());
        assert!(out.contains("# 会话交接"));
        assert!(!out.contains("## 用户"));
        assert!(!out.contains("## 助手"));
    }
}
