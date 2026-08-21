//! codex（OpenAI Codex CLI）会话解析。codex 的 hooks 与 claude 同款：Stop hook 直带
//! `last_assistant_message`（故最近 AI 正文走 hook payload，不在此读），标题靠首条 prompt 命名
//! （rollout 首条 user 文本被 AGENTS.md/指令包裹，不适合当标题）。唯一需从会话文件补的是【模型】
//! ——Stop hook 不携带模型，需读 rollout 的 `turn_context.model`。
//!
//! rollout：`{CODEX_HOME 或 ~/.codex}/sessions/<YYYY>/<MM>/<DD>/rollout-<ISO>-<session_uuid>.jsonl`，
//! 每行一个事件 `{type, payload}`。首行 `type=session_meta`；其后 `type=turn_context` 的
//! `payload.model` 即模型（如 "gpt-5.5"），通常在文件靠前（首回合）。

use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::transcript::ChatItem;
use crate::transcript::{ChatOnlyParser, TranscriptEvent, TranscriptParser, TranscriptSpec};

fn chat_id(prefix: &str, line: &str) -> String {
    let mut hash = crate::codec::FNV1A_OFFSET;
    crate::codec::fnv1a(&mut hash, line.as_bytes());
    format!("codex-{prefix}-{hash:016x}")
}

fn content_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                // 0.148 的 Reasoning.summary_text 是裸字符串数组，消息 content 是对象数组。
                part.as_str().map(str::to_string).or_else(|| {
                    part.get("text")
                        .or_else(|| part.get("input_text"))
                        .or_else(|| part.get("output_text"))
                        .and_then(|text| text.as_str())
                        .map(str::to_string)
                })
            })
            .collect::<Vec<_>>()
            .join(""),
        other => other.to_string(),
    }
}

/// codex 0.148+ 的统一逐项完成事件 `event_msg/item_completed`：老版本的
/// `event_msg/user_message`、`agent_message`、`agent_reasoning` 不再写入 rollout，
/// 用户与 AI 正文只剩这里（`response_item/message` 虽也存一份 assistant 正文，但
/// 与本事件同 id 重复，且其 user 形态裹着 environment_context，维持整体跳过）。
///
/// CommandExecution 刻意不产出：它是某次 `custom_tool_call`（exec）的子事件，命令
/// 与输出已由 `custom_tool_call` / `custom_tool_call_output` 主链承载，再发一遍
/// 就是同一条命令上屏两次。FileChange 没有主链对应物，是文件改动唯一的结构化记录。
fn completed_item_events(
    payload: &serde_json::Value,
    timestamp: Option<String>,
    line: &str,
) -> Vec<TranscriptEvent> {
    let Some(item) = payload.get("item") else {
        return Vec::new();
    };
    let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // rollout 的 item.id 全局唯一；缺失时退回整行哈希。
    let id = |prefix: &str| {
        item.get("id")
            .and_then(|v| v.as_str())
            .map(|i| format!("codex-{prefix}-{i}"))
            .unwrap_or_else(|| chat_id(prefix, line))
    };
    match kind {
        "UserMessage" => {
            let text = item.get("content").map(content_text).unwrap_or_default();
            (!text.trim().is_empty())
                .then(|| TranscriptEvent::UserMessage {
                    id: id("user"),
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        "AgentMessage" => {
            let text = item.get("content").map(content_text).unwrap_or_default();
            (!text.trim().is_empty())
                .then(|| TranscriptEvent::AssistantMessage {
                    id: id("assistant"),
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        "Reasoning" => {
            // 0.148 的思考多为 encrypted_content（summary_text/raw_content 皆空），
            // 只在真有可读摘要时上屏。
            let text = item
                .get("summary_text")
                .map(content_text)
                .filter(|t| !t.trim().is_empty())
                .or_else(|| item.get("raw_content").map(content_text))
                .unwrap_or_default();
            (!text.trim().is_empty())
                .then(|| TranscriptEvent::Reasoning {
                    id: id("reasoning"),
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        "FileChange" => {
            let Some(changes) = item.get("changes").and_then(|c| c.as_object()) else {
                return Vec::new();
            };
            // 摘要列文件名（带操作），展开给每个文件的正文/补丁；正文可能是整个文件。
            // 两处上限都按**字符**计——与 claude 的 compact_json 同单位（800/4000 同量级）。
            const DETAIL_LIMIT: usize = 4000;
            const SUMMARY_LIMIT: usize = 800;
            let mut names = Vec::new();
            let mut detail = String::new();
            let mut detail_chars = 0usize;
            for (path, change) in changes {
                let op = change
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("update");
                let name = Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);
                names.push(name.to_string());
                if detail_chars >= DETAIL_LIMIT {
                    continue;
                }
                let body = change
                    .get("content")
                    .or_else(|| change.get("unified_diff"))
                    .or_else(|| change.get("diff"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let header = format!("[{op}] {path}\n");
                detail_chars += header.chars().count();
                detail.push_str(&header);
                let room = DETAIL_LIMIT.saturating_sub(detail_chars);
                if room > 0 && !body.is_empty() {
                    let mut rest = body.chars();
                    let clipped: String = rest.by_ref().take(room).collect();
                    detail_chars += clipped.chars().count();
                    detail.push_str(&clipped);
                    // 迭代器还有剩 = 被截断；避免对整个 body 再做一次 O(n) 计数。
                    if rest.next().is_some() {
                        detail.push_str("\n…");
                    }
                    detail.push('\n');
                }
            }
            let mut summary = names.join(", ");
            if summary.chars().count() > SUMMARY_LIMIT {
                summary = summary.chars().take(SUMMARY_LIMIT).collect();
                summary.push('…');
            }
            let call_id = id("patch");
            vec![
                TranscriptEvent::ToolCall {
                    id: call_id.clone(),
                    timestamp: timestamp.clone(),
                    name: "patch".into(),
                    summary,
                    subagent: None,
                },
                TranscriptEvent::ToolResult {
                    id: format!("{call_id}-out"),
                    timestamp,
                    tool_call_id: Some(call_id),
                    text: detail,
                    is_error: item.get("status").and_then(|v| v.as_str()) == Some("failed"),
                    subagent: None,
                },
            ]
        }
        _ => Vec::new(),
    }
}

fn parse_transcript_events(line: &str) -> Vec<TranscriptEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let payload = value.get("payload").unwrap_or(&value);
    let outer = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match (outer, kind) {
        ("event_msg", "user_message") => {
            let text = payload.get("message").map(content_text).unwrap_or_default();
            (!text.trim().is_empty())
                .then(|| TranscriptEvent::UserMessage {
                    id: chat_id("user", line),
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        ("event_msg", "agent_message") => {
            let text = payload.get("message").map(content_text).unwrap_or_default();
            (!text.trim().is_empty())
                .then(|| TranscriptEvent::AssistantMessage {
                    id: chat_id("assistant", line),
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        ("event_msg", "agent_reasoning") => {
            let text = payload
                .get("text")
                .or_else(|| payload.get("message"))
                .map(content_text)
                .unwrap_or_default();
            (!text.trim().is_empty())
                .then(|| TranscriptEvent::Reasoning {
                    id: chat_id("reasoning", line),
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        ("response_item", "reasoning") => {
            let text = payload
                .get("summary")
                .or_else(|| payload.get("content"))
                .or_else(|| payload.get("text"))
                .map(content_text)
                .unwrap_or_default();
            (!text.trim().is_empty())
                .then(|| TranscriptEvent::Reasoning {
                    id: chat_id("reasoning", line),
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        ("response_item", "function_call") | ("response_item", "custom_tool_call") => {
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("tool")
                .to_string();
            let summary = payload
                .get("arguments")
                .or_else(|| payload.get("input"))
                .map(content_text)
                .unwrap_or_default();
            vec![TranscriptEvent::ToolCall {
                id: payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| chat_id("tool", line)),
                timestamp,
                name,
                summary,
                // codex 当前没有子任务：工具集只有 exec/wait，`task_started` 是回合级事件
                // （payload 带 turn_id / model_context_window）而非委派。
                subagent: None,
            }]
        }
        ("response_item", "function_call_output")
        | ("response_item", "custom_tool_call_output") => {
            vec![TranscriptEvent::ToolResult {
                // codex 没有子任务概念（见 mod.rs 的 subagents 说明）。
                subagent: None,
                id: chat_id("result", line),
                timestamp,
                tool_call_id: payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                text: payload.get("output").map(content_text).unwrap_or_default(),
                is_error: payload
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            }]
        }
        ("event_msg", "item_completed") => completed_item_events(payload, timestamp, line),
        // 回合收尾带错误（如 unauthorized：refresh token 被吊销）：终端里满屏红字，
        // 对话窗此前却一片空白——这是 rollout 里唯一的回合级错误信号。
        ("event_msg", "task_complete") => {
            let Some(error) = payload.get("error").filter(|e| !e.is_null()) else {
                return Vec::new();
            };
            let info = error
                .get("codex_error_info")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let text = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(info)
                .to_string();
            if text.trim().is_empty() {
                return Vec::new();
            }
            let label = if info == "unauthorized" {
                "需要重新登录"
            } else {
                "回合出错"
            };
            vec![TranscriptEvent::TurnError {
                id: chat_id("error", line),
                timestamp,
                label: label.into(),
                text,
            }]
        }
        ("event_msg", "context_compacted") => vec![TranscriptEvent::Metadata {
            id: chat_id("compact", line),
            timestamp,
            kind: "compact".into(),
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
fn parse_chat_items(line: &str) -> Vec<ChatItem> {
    parse_transcript_events(line)
        .into_iter()
        .map(ChatItem::from)
        .collect()
}

pub struct CodexTranscript;
pub static CODEX_TRANSCRIPT: CodexTranscript = CodexTranscript;

impl TranscriptSpec for CodexTranscript {
    fn new_parser(&self) -> Box<dyn TranscriptParser> {
        Box::new(ChatOnlyParser)
    }

    fn resolve_transcript_path(
        &self,
        transcript_path: Option<&str>,
        _cwd: Option<&str>,
        session_id: &str,
    ) -> Option<PathBuf> {
        transcript_path
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .or_else(|| find_rollout(session_id))
    }

    fn resolve_title(
        &self,
        _transcript_path: Option<&str>,
        _cwd: Option<&str>,
        _session_id: &str,
    ) -> Option<String> {
        None
    }

    fn parse_transcript_line(&self, line: &str) -> Vec<TranscriptEvent> {
        parse_transcript_events(line)
    }

    fn agent_modes_from_line(&self, line: &str) -> Vec<crate::AgentMode> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return Vec::new();
        };
        let outer = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = value.get("payload").unwrap_or(&value);
        let mut modes = Vec::new();
        if outer == "turn_context" {
            if let Some(mode) = payload
                .pointer("/collaboration_mode/mode")
                .and_then(|v| v.as_str())
            {
                modes.push(crate::AgentMode::new("collaboration", mode));
            }
            if let Some(approval) = payload.get("approval_policy").and_then(|v| v.as_str()) {
                modes.push(crate::AgentMode::new("approval", approval));
            }
            if let Some(sandbox) = payload
                .pointer("/sandbox_policy/type")
                .and_then(|v| v.as_str())
            {
                modes.push(crate::AgentMode::new("sandbox", sandbox));
            }
        } else if outer == "event_msg"
            && payload.get("type").and_then(|v| v.as_str()) == Some("task_started")
        {
            if let Some(mode) = payload
                .get("collaboration_mode_kind")
                .and_then(|v| v.as_str())
            {
                modes.push(crate::AgentMode::new("collaboration", mode));
            }
        }
        modes
    }

    fn supports_chat(&self) -> bool {
        true
    }

    fn supports_analysis(&self) -> bool {
        false
    }
}

/// codex 在本机的实况（数据目录/hooks 规格/凭据/启动 argv）。变体表见
/// `meowo_agent::plugins::codex`：数据目录 `CODEX_HOME` 优先，否则 `~/.codex`。
/// 检测/接线/状态/账号/会话读取全部经此一处解析路径。
pub fn codex_install() -> Option<crate::Installation> {
    crate::registry::installation(crate::id::CODEX)
}

/// codex 数据根。`codex_install()` 的便捷取值。
pub fn codex_home() -> Option<PathBuf> {
    codex_install().map(|i| i.data_dir)
}

/// codex 的启动前缀 argv（不含 `resume <id>`）：bun 全局 exe ／ `node <codex.js>` ／ 独立安装 exe。
/// 都没有则 None（调用方回退裸名 codex）。优先级与理由见变体表的 `LAUNCH`。
pub fn codex_launch_prefix() -> Option<Vec<String>> {
    codex_install()?.launch
}

/// 在 `~/.codex/sessions` 下按 session_id 找 rollout 文件（文件名内嵌 uuid，以 `<uuid>.jsonl` 结尾）。
/// 递归 walk 年/月/日（限深，避免误入无关深目录）。仅作 transcript_path 缺失时的兜底。
fn find_rollout(session_id: &str) -> Option<PathBuf> {
    let sessions = codex_home()?.join("sessions");
    let suffix = format!("{session_id}.jsonl");
    walk_find(&sessions, &suffix, 6)
}

fn walk_find(dir: &Path, suffix: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(suffix))
        {
            return Some(p);
        }
    }
    for d in subdirs {
        if let Some(f) = walk_find(&d, suffix, depth - 1) {
            return Some(f);
        }
    }
    None
}

/// 读文件前 max_lines 行为一个 String（模型在文件靠前，无需读全量）。
fn read_head_lines(path: &Path, max_lines: usize) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    let mut out = String::new();
    for line in BufReader::new(f)
        .lines()
        .take(max_lines)
        .map_while(Result::ok)
    {
        out.push_str(&line);
        out.push('\n');
    }
    Some(out)
}

/// 纯解析：从 rollout 文本取第一条 `turn_context` 的 `payload.model`。便于单测，不碰文件系统。
pub fn parse_model(content: &str) -> Option<String> {
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("turn_context") {
            if let Some(m) = v
                .get("payload")
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(m.to_string());
            }
        }
    }
    None
}

/// 取某 codex 会话的模型展示名：优先用 hook 给的 transcript_path，否则按 session_id 在 sessions 下找。
/// 读 rollout 前若干行解析 `turn_context.model`。定位/解析失败返回 None（卡片模型留空，不阻断）。
pub fn read_model(transcript_path: Option<&str>, session_id: &str) -> Option<String> {
    let path = transcript_path
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| find_rollout(session_id))?;
    // turn_context 在首回合、文件靠前；读前 200 行足够，避免长会话全量读。
    let head = read_head_lines(&path, 200)?;
    parse_model(&head)
}

/// 从 rollout 文本取**最后一条 info 非 null** 的 token_count 的 (input_tokens, model_context_window)。
/// codex 会话开头的 token_count `info` 为 null（只有 rate_limits），跳过。used 取 last_token_usage.input_tokens
/// （最近一次请求的 context 输入量，已含 cached_input_tokens）。
pub fn parse_context(content: &str) -> Option<(i64, i64)> {
    let mut last: Option<(i64, i64)> = None;
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = v.get("payload");
        if payload.and_then(|p| p.get("type")).and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.and_then(|p| p.get("info")).filter(|i| !i.is_null()) else {
            continue;
        };
        let used = info
            .get("last_token_usage")
            .and_then(|l| l.get("input_tokens"))
            .and_then(|x| x.as_i64());
        let window = info.get("model_context_window").and_then(|x| x.as_i64());
        if let (Some(u), Some(w)) = (used, window) {
            last = Some((u, w));
        }
    }
    last
}

/// 读文件尾部最多 max_bytes 字节为 lossy UTF-8（首个半截行交给 parse_context 跳过）。
/// `pub(super)`：account 的 token_count 尾部扫描复用同一份有界读——rollout 可达数十 MB，
/// 绝不能整个读进内存。
pub(super) fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(size.saturating_sub(max_bytes)))
        .ok()?;
    let mut buf = Vec::new();
    f.take(max_bytes).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// codex 会话最近上下文占用：定位 rollout（hook 的 transcript_path 优先，否则按 id 找），
/// 尾部读取最后一条 token_count。定位/解析失败返回 None。
pub fn read_context(
    transcript_path: Option<&str>,
    session_id: &str,
) -> Option<crate::caps::ContextUsage> {
    let path = transcript_path
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| find_rollout(session_id))?;
    const TAIL_BYTES: u64 = 256 * 1024;
    let text = read_tail(&path, TAIL_BYTES)?;
    let (used, window) = parse_context(&text)?;
    if window <= 0 {
        return None;
    }
    let pct = (used * 100 / window).clamp(0, 100);
    Some(crate::caps::ContextUsage {
        used_pct: pct,
        window,
        // codex 的 token_count 事件不带模型名。
        model: None,
    })
}

// ═══ 能力槽 ═══

pub struct CodexTelemetry;
pub static TELEMETRY: CodexTelemetry = CodexTelemetry;

impl crate::caps::TelemetryCap for CodexTelemetry {
    /// codex 的 Stop hook 直带 AI 正文（同 claude）；模型 Stop 不带，从 rollout 的 turn_context 补。
    fn stop_outputs(&self, ctx: &crate::caps::HookContext) -> crate::caps::StopOutputs {
        crate::caps::StopOutputs {
            last_ai: ctx.last_assistant_message.map(str::to_string),
            model: read_model(ctx.transcript_path, ctx.session_id),
        }
    }

    fn read_context(&self, ctx: &crate::caps::HookContext) -> Option<crate::caps::ContextUsage> {
        read_context(ctx.transcript_path, ctx.session_id)
    }

    fn transcript(&self) -> Option<&'static dyn TranscriptSpec> {
        Some(&CODEX_TRANSCRIPT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_collaboration_approval_and_sandbox_dimensions() {
        let modes = CODEX_TRANSCRIPT.agent_modes_from_line(
            r#"{"type":"turn_context","payload":{"approval_policy":"on-request","sandbox_policy":{"type":"workspace-write"},"collaboration_mode":{"mode":"plan"}}}"#,
        );
        assert_eq!(
            modes,
            vec![
                crate::AgentMode::new("collaboration", "plan"),
                crate::AgentMode::new("approval", "on-request"),
                crate::AgentMode::new("sandbox", "workspace-write"),
            ]
        );
        assert_eq!(
            CODEX_TRANSCRIPT.agent_modes_from_line(
                r#"{"type":"event_msg","payload":{"type":"task_started","collaboration_mode_kind":"default"}}"#,
            ),
            vec![crate::AgentMode::new("collaboration", "default")]
        );
    }

    #[test]
    fn parse_context_takes_last_nonnull_token_count() {
        let rollout = r#"
{"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":7.0}}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":6766,"cached_input_tokens":4480},"model_context_window":258400}}}
"#;
        // 跳过 info=null 那条；取最后一条 info 非 null 的：input_tokens=6766, window=258400。
        assert_eq!(parse_context(rollout), Some((6766, 258400)));
    }

    #[test]
    fn parse_context_none_when_no_token_count() {
        assert_eq!(
            parse_context(r#"{"type":"turn_context","payload":{"model":"gpt-5.5"}}"#),
            None
        );
    }

    #[test]
    fn parse_model_takes_first_turn_context() {
        let rollout = r#"
{"type":"session_meta","payload":{"id":"x","cwd":"/p","model_provider":"openai"}}
{"type":"turn_context","payload":{"model":"gpt-5.5","cwd":"/p","effort":"medium"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}
{"type":"turn_context","payload":{"model":"gpt-5.3-codex"}}
"#;
        assert_eq!(parse_model(rollout).as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn parse_model_none_when_absent() {
        let rollout = r#"{"type":"session_meta","payload":{"id":"x"}}
{"type":"turn_context","payload":{"cwd":"/p"}}"#;
        assert_eq!(parse_model(rollout), None);
        assert_eq!(parse_model(""), None);
    }

    #[test]
    fn chat_parser_uses_clean_events_and_tool_records_without_duplicates() {
        let user = r#"{"timestamp":"t1","type":"event_msg","payload":{"type":"user_message","message":"修复测试"}}"#;
        let assistant = r#"{"timestamp":"t2","type":"event_msg","payload":{"type":"agent_message","message":"开始处理"}}"#;
        let tool = r#"{"timestamp":"t3","type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"cargo test\"}","call_id":"c1"}}"#;
        let reasoning = r#"{"timestamp":"t2","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"先运行测试"}]}}"#;
        assert!(
            matches!(&parse_chat_items(user)[0], ChatItem::UserText { text, .. } if text == "修复测试")
        );
        assert!(
            matches!(&parse_chat_items(assistant)[0], ChatItem::AssistantText { text, .. } if text == "开始处理")
        );
        assert!(
            matches!(&parse_chat_items(tool)[0], ChatItem::ToolUse { name, summary, .. } if name == "shell_command" && summary.contains("cargo test"))
        );
        assert!(matches!(
            &parse_chat_items(reasoning)[0],
            ChatItem::Reasoning { text, .. } if text == "先运行测试"
        ));
        // 原始 response_item user message 常含指令包，与 event_msg.user_message 重复，必须跳过。
        assert!(parse_chat_items(
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[]}}"#
        )
        .is_empty());
    }

    /// codex 0.148 换了 rollout 形态：user/agent 正文只在 `event_msg/item_completed` 里
    ///（老的 `event_msg/user_message` 等不再写入），漏解析的直接后果是对话窗只剩 exec 组。
    /// 样本取自真机 rollout（0.148.0）。
    #[test]
    fn parses_codex_0148_item_completed_events() {
        let user = r#"{"timestamp":"t1","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u1","content":[{"type":"text","text":"我想做一个功能","text_elements":[]}]}}}"#;
        assert!(matches!(
            &parse_chat_items(user)[0],
            ChatItem::UserText { id, text, .. } if text == "我想做一个功能" && id == "codex-user-u1"
        ));

        let agent = r#"{"timestamp":"t2","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"m1","content":[{"type":"Text","text":"我先检查项目结构"}],"phase":"commentary"}}}"#;
        assert!(matches!(
            &parse_chat_items(agent)[0],
            ChatItem::AssistantText { text, .. } if text == "我先检查项目结构"
        ));

        // 0.148 的思考通常整段加密（summary_text/raw_content 皆空）→ 不产出。
        let encrypted = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","id":"r1","summary_text":[],"raw_content":[]}}}"#;
        assert!(parse_chat_items(encrypted).is_empty());
        // 真有可读摘要（裸字符串数组）时上屏。
        let readable = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","id":"r2","summary_text":["先运行测试"]}}}"#;
        assert!(matches!(
            &parse_chat_items(readable)[0],
            ChatItem::Reasoning { text, .. } if text == "先运行测试"
        ));

        // CommandExecution 是 exec custom_tool_call 的子事件，产出即同一命令上屏两次。
        let exec = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"e1","command":["pwsh","-Command","ls"],"exit_code":0}}}"#;
        assert!(parse_chat_items(exec).is_empty());
    }

    #[test]
    fn file_change_item_becomes_patch_call_with_result() {
        // 路径用正斜杠：Unix 的 Path::file_name 不认反斜杠分隔符，Windows 路径样本会让
        // 此测试仅在 Windows 通过（rollout 由产生它的机器解析，生产中路径总是本平台形态）。
        let line = r##"{"timestamp":"t3","type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","id":"fc1","changes":{"/p/README.md":{"type":"add","content":"# 标题\n正文"}}}}}"##;
        let items = parse_chat_items(line);
        assert_eq!(items.len(), 2);
        assert!(matches!(
            &items[0],
            ChatItem::ToolUse { id, name, summary, .. }
                if name == "patch" && summary == "README.md" && id == "codex-patch-fc1"
        ));
        assert!(matches!(
            &items[1],
            ChatItem::ToolResult { tool_use_id: Some(call), text, is_error: false, .. }
                if call == "codex-patch-fc1" && text.contains("[add]") && text.contains("# 标题")
        ));
    }

    /// 上限一律按字符计：中文正文（每字符 3 字节）不得把详情撑到字节口径的数倍，
    /// 超限文件仍要进摘要（只省正文），摘要本身也有封顶。
    #[test]
    fn file_change_caps_detail_and_summary_by_chars() {
        let body: String = "中".repeat(5000);
        let files: Vec<String> = (0..300)
            .map(|i| format!(r#""/p/文件{i:03}.rs":{{"type":"update","content":"{body}"}}"#))
            .collect();
        let line = format!(
            r#"{{"type":"event_msg","payload":{{"type":"item_completed","item":{{"type":"FileChange","id":"fc2","changes":{{{}}}}}}}}}"#,
            files.join(",")
        );
        let items = parse_chat_items(&line);
        assert_eq!(items.len(), 2);
        let (summary, text) = match (&items[0], &items[1]) {
            (ChatItem::ToolUse { summary, .. }, ChatItem::ToolResult { text, .. }) => {
                (summary, text)
            }
            other => panic!("形状不对: {other:?}"),
        };
        // 详情：字符封顶（截断记号与文件头允许少量溢出），且带截断记号。
        assert!(text.chars().count() < 4100, "实际 {}", text.chars().count());
        assert!(text.contains('…'));
        // 摘要：300 个文件名全进 names 但被封顶在 800 字符 + 记号。
        assert_eq!(summary.chars().count(), 801);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn task_complete_error_becomes_turn_error() {
        let unauthorized = r#"{"timestamp":"t4","type":"event_msg","payload":{"type":"task_complete","last_agent_message":null,"error":{"message":"Your access token could not be refreshed.","codex_error_info":"unauthorized"}}}"#;
        assert!(matches!(
            &parse_chat_items(unauthorized)[0],
            ChatItem::TurnError { label, text, .. }
                if label == "需要重新登录" && text.contains("could not be refreshed")
        ));
        // 正常收尾（error=null）不产出。
        assert!(parse_chat_items(
            r#"{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"done","error":null}}"#
        )
        .is_empty());
    }
}
