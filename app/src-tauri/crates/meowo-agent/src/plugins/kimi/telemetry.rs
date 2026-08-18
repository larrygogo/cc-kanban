//! kimi-code 会话记录解析。kimi 的 Stop hook **不带** AI 正文（只有 session_id/cwd），
//! 故需从会话的 `agents/main/wire.jsonl` 里读最近一条 AI 文本。
//!
//! wire.jsonl 结构（kimi-code 0.19.2 实测）：每行一个事件，AI 正文在
//! `type="context.append_loop_event"` 且 `event.type="content.part"` 且 `event.part.type="text"`
//! 的 `event.part.text` 里（`part.type="think"` 是思考过程：聊天窗口展示，但不计入最终 AI 正文）。
//! 用户输入则是 `type="turn.prompt"`
//! 或 `type="context.append_message"` 且 `message.role="user"`——遇到即清空缓冲，使最终缓冲恰为
//! 「最后一条用户输入之后的 AI 文本」。用户消息里的 `image_url` part（粘贴图片，blobref 指向
//! wire.jsonl 旁的 `blobs/<sha>`，或内嵌 data: URL）由 [`user_message_text`] 落盘后补
//! `[Image: source: …]` 引用行，与 claude 侧形制一致。

use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::transcript::ChatItem;
use crate::transcript::{
    ChatOnlyParser, SubagentOutcome, SubagentRef, SubagentSpec, SubagentStream, TranscriptEvent,
    TranscriptParser, TranscriptSpec,
};

fn chat_id(prefix: &str, line: &str) -> String {
    let mut hash = crate::codec::FNV1A_OFFSET;
    crate::codec::fnv1a(&mut hash, line.as_bytes());
    format!("kimi-{prefix}-{hash:016x}")
}

/// epoch 毫秒 → ISO-8601 UTC 字符串（`2026-08-14T03:37:12.036Z`）。`ChatItem.timestamp`
/// 是字符串，claude 侧填的就是这种 ISO 形态，kimi 的数字时间统一转成同一表示。
/// 历法换算用 Howard Hinnant 的 civil_from_days（与 Unix 时间同样无闰秒），
/// 不为一个字段引整只时间库。
fn iso_from_epoch_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

/// wire 行的时间。实档字段是 `time`（epoch 毫秒**数字**，kimi-code 0.19/0.2x 实测每行
/// 都带）；此前误读成 `timestamp`，所有 kimi 消息无时间（实拍回归）。ISO 字符串形态的
/// `timestamp` 保留成兼容兜底。
fn line_timestamp(value: &serde_json::Value) -> Option<String> {
    value
        .get("time")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .map(iso_from_epoch_ms)
        .or_else(|| {
            value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        other => other.to_string(),
    }
}

/// 图片 mime → 扩展名。渲染端按扩展名认图（Message.tsx 的 IMAGE_EXTENSIONS），未知类型不落盘。
fn image_ext(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// 用户消息里 image_url part 的 url（kimi 实测是 camelCase 的 `imageUrl`，snake 形态兼容）。
fn image_part_url(part: &serde_json::Value) -> Option<&str> {
    if part.get("type").and_then(|t| t.as_str()) != Some("image_url") {
        return None;
    }
    part.get("imageUrl")
        .or_else(|| part.get("image_url"))
        .and_then(|u| u.get("url"))
        .or_else(|| part.get("url"))
        .and_then(|u| u.as_str())
}

/// 把用户消息里的图片 part 落成本地文件，返回可写进 `[Image: source: …]` 引用的绝对路径。
/// 两种 url 形态（kimi-code 0.2x 实测）：
///
/// - `blobref:<mime>;<sha256>`：正文在 wire.jsonl 旁的 `blobs/<sha256>`（无扩展名），读出复制
/// - `data:<mime>;base64,<data>`：内嵌 base64，解码落盘（同 claude 排队插图的处理）
///
/// 统一落在 `paste_root()/queued/`（asset 协议 scope 内，前端才能渲染缩略图），按内容 sha /
/// 行 id 命名做幂等——增量解析 reset 时会重放同一行。任何失败返回 None：正文照常显示，
/// 这张图退化为不显示。
fn persist_image_url(
    wire_dir: Option<&Path>,
    base_id: &str,
    index: usize,
    url: &str,
) -> Option<PathBuf> {
    use crate::fsutil::PASTE_MAX_BYTES;
    if let Some(rest) = url.strip_prefix("blobref:") {
        let (mime, sha) = rest.split_once(';')?;
        let ext = image_ext(mime)?;
        // sha 直接拼进文件名与路径，过滤成 hex，挡掉脏数据里的路径分隔符。
        let sha: String = sha
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(128)
            .collect();
        if sha.is_empty() {
            return None;
        }
        let src = wire_dir?.join("blobs").join(&sha);
        let len = std::fs::metadata(&src).ok()?.len();
        if len == 0 || len > PASTE_MAX_BYTES as u64 {
            return None;
        }
        let bytes = std::fs::read(&src).ok()?;
        return crate::fsutil::persist_paste_bytes(&format!("kimi-{sha}"), ext, &bytes);
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (mime, data) = rest.split_once(";base64,")?;
        let ext = image_ext(mime)?;
        let safe_id: String = base_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(64)
            .collect();
        if safe_id.is_empty() {
            return None;
        }
        // 编码后长度 ≈ 4/3 原始大小：先按编码长度挡超大 payload，再解码（同粘贴附件的写法）。
        if data.len() > PASTE_MAX_BYTES / 3 * 4 + 4 {
            return None;
        }
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .ok()?;
        return crate::fsutil::persist_paste_bytes(
            &format!("kimi-{safe_id}-{index}"),
            ext,
            &bytes,
        );
    }
    None
}

/// 用户消息正文 + 图片。kimi 把粘贴的图片记成 `image_url` part（blobref 指向 wire.jsonl 旁
/// 的 `blobs/<sha>`，或内嵌 data: URL），而 `value_text` 只拼 text part——图片被静默丢掉，
/// 纯图消息整条消失（实拍反馈：对话里看不到 kimi 会话的图）。这里把图片落盘后补
/// `[Image: source: …]` 行，与 claude 记录粘贴图片的形制一致，前端现成的缩略图链路直接接住。
fn user_message_text(
    value: Option<&serde_json::Value>,
    wire_dir: Option<&Path>,
    base_id: &str,
) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let Some(parts) = value.as_array() else {
        return value_text(value);
    };
    let mut text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
        .collect::<Vec<_>>()
        .join("");
    for (index, part) in parts.iter().enumerate() {
        let Some(url) = image_part_url(part) else {
            continue;
        };
        if let Some(path) = persist_image_url(wire_dir, base_id, index, url) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("[Image: source: {}]", path.display()));
        }
    }
    text
}

fn parse_events(line: &str, wire_dir: Option<&Path>) -> Vec<TranscriptEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let timestamp = line_timestamp(&value);
    // 事件 id：宿主拿它当唯一身份（React key、水位线去重），整行内容哈希会让**内容相同
    // 的两行**必然同 id。loop 事件自带全局唯一且跨读取稳定的 `event.uuid`（kimi-code
    // 0.2x 实测；step.begin/end 成对共享 uuid，但那些行不产出事件），优先用它；其余行
    // 兜底整行哈希——行里带毫秒级 `time` 字段，跨回合的同文案行天然不同。
    let event_uuid = value
        .pointer("/event/uuid")
        .and_then(|v| v.as_str())
        .filter(|uuid| !uuid.is_empty());
    let stable_id = |prefix: &str| match event_uuid {
        Some(uuid) => format!("kimi-{prefix}-{uuid}"),
        None => chat_id(prefix, line),
    };
    match value.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "turn.prompt" => {
            let id = chat_id("user", line);
            let text = user_message_text(
                value
                    .get("input")
                    .or_else(|| value.get("prompt"))
                    .or_else(|| value.get("message")),
                wire_dir,
                &id,
            );
            (!text.trim().is_empty())
                .then_some(TranscriptEvent::UserMessage {
                    id,
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        "context.append_message"
            if value
                .get("message")
                .and_then(|message| message.get("role"))
                .and_then(|role| role.as_str())
                == Some("user") =>
        {
            let Some(message) = value.get("message") else {
                return Vec::new();
            };
            // Kimi 也用 role=user 承载 system reminder、后台任务通知等内部注入。只排除明确
            // 标成非 user 的 origin；旧版没有 origin 的纯用户消息仍需兼容。
            if message
                .get("origin")
                .and_then(|origin| origin.get("kind"))
                .and_then(|kind| kind.as_str())
                .is_some_and(|kind| kind != "user")
            {
                return Vec::new();
            }
            let id = chat_id("user", line);
            let text = user_message_text(
                message.get("content").or_else(|| message.get("text")),
                wire_dir,
                &id,
            );
            (!text.trim().is_empty())
                .then_some(TranscriptEvent::UserMessage {
                    id,
                    timestamp,
                    text,
                })
                .into_iter()
                .collect()
        }
        "context.append_loop_event" => {
            let Some(event) = value.get("event") else {
                return Vec::new();
            };
            let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if matches!(event_type, "tool.result" | "tool_result") {
                let text = event
                    .get("output")
                    .or_else(|| event.pointer("/result/output"))
                    .or_else(|| event.get("result"))
                    .map(value_text)
                    .unwrap_or_default();
                let tool_call_id = event
                    .get("callId")
                    .or_else(|| event.get("call_id"))
                    .or_else(|| event.get("toolCallId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                return vec![TranscriptEvent::ToolResult {
                    // 实档的 tool.result 事件没有 uuid，callId 是它的稳定唯一键
                    // （一次调用恰一份结果）；两者都缺才退回整行哈希。
                    id: event_uuid
                        .map(|uuid| format!("kimi-result-{uuid}"))
                        .or_else(|| tool_call_id.as_ref().map(|id| format!("kimi-result-{id}")))
                        .unwrap_or_else(|| chat_id("result", line)),
                    timestamp,
                    tool_call_id,
                    // 子任务状态就写在这条结果里；顺手取出，折叠态才能显示进度。
                    subagent: KIMI_SUBAGENTS.detect_result(&text),
                    text,
                    is_error: event
                        .get("isError")
                        .or_else(|| event.get("is_error"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                }];
            }
            let part = if event_type == "content.part" {
                event.get("part")
            } else if matches!(event_type, "tool.call" | "tool_call") {
                Some(event)
            } else {
                None
            };
            let Some(part) = part else { return Vec::new() };
            match part
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or(event_type)
            {
                "text" => part
                    .get("text")
                    .and_then(|v| v.as_str())
                    .filter(|text| !text.is_empty())
                    .map(|text| TranscriptEvent::AssistantChunk {
                        id: stable_id("assistant"),
                        timestamp,
                        text: text.to_string(),
                    })
                    .into_iter()
                    .collect(),
                "think" | "thinking" => part
                    .get("think")
                    .or_else(|| part.get("thinking"))
                    .or_else(|| part.get("text"))
                    .and_then(|v| v.as_str())
                    .filter(|text| !text.is_empty())
                    .map(|text| TranscriptEvent::ReasoningChunk {
                        id: stable_id("reasoning"),
                        timestamp,
                        text: text.to_string(),
                    })
                    .into_iter()
                    .collect(),
                "tool.call" | "tool_call" | "tool" => {
                    let name = part
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool")
                        .to_string();
                    let args = part
                        .get("input")
                        .or_else(|| part.get("arguments"))
                        .or_else(|| part.get("args"));
                    let subagent = KIMI_SUBAGENTS.detect_call(&name, args);
                    vec![TranscriptEvent::ToolCall {
                        id: part
                            .get("id")
                            .or_else(|| part.get("callId"))
                            .or_else(|| part.get("toolCallId"))
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| stable_id("tool")),
                        timestamp,
                        name,
                        // 子任务的摘要取那句描述；否则整包 args（含上千字 prompt）会把摘要行淹掉。
                        summary: subagent
                            .as_ref()
                            .map(|call| call.description.clone())
                            .filter(|description| !description.is_empty())
                            .or_else(|| args.map(value_text))
                            .unwrap_or_default(),
                        subagent,
                    }]
                }
                _ => Vec::new(),
            }
        }
        "context.compact" | "context.compacted" => vec![TranscriptEvent::Metadata {
            id: chat_id("compact", line),
            timestamp,
            kind: "compact".into(),
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
fn parse_chat_items(line: &str) -> Vec<ChatItem> {
    parse_chat_items_in(None, line)
}

#[cfg(test)]
fn parse_chat_items_in(wire_dir: Option<&Path>, line: &str) -> Vec<ChatItem> {
    parse_events(line, wire_dir)
        .into_iter()
        .map(ChatItem::from)
        .collect()
}

/// Kimi 的子任务侧车布局（kimi-code 0.2x 实测）：
///
/// ```text
/// sessions/<wd>/session_<id>/agents/main/wire.jsonl       ← 主流
/// sessions/<wd>/session_<id>/agents/agent-0/wire.jsonl    ← 子任务，同格式
/// sessions/<wd>/session_<id>/agents/agent-1/wire.jsonl
/// ```
///
/// 与 claude 不同，kimi **没有 meta 文件做外键**：主链 `Agent` 调用的归属只写在对应
/// `tool.result` 的**输出正文**里（`agent_id: agent-0` 这样的纯文本行）。故定位要回扫主流，
/// 找到该 callId 的结果再抠 id——正是这种差异让子任务能力必须落在插件而非共享路径。
pub struct KimiSubagents;
pub static KIMI_SUBAGENTS: KimiSubagents = KimiSubagents;

/// 紧跟 `key` 之后的取值（跳过 `:`/`=`/引号/空白），只取字母数字与 `-_`。
fn value_after(text: &str, key: &str) -> Option<String> {
    let rest = text.split(key).nth(1)?;
    let rest = rest.trim_start_matches([':', '=', '"', '\'', ' ', '\t']);
    let value: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!value.is_empty()).then_some(value)
}

/// kimi 的状态词 → 归一化的 `running` / `completed` / `failed`。
fn normalize_status(raw: &str) -> Option<&'static str> {
    match raw {
        "completed" | "done" | "success" => Some("completed"),
        "failed" | "error" | "cancelled" | "aborted" => Some("failed"),
        "running" | "not_ready" | "pending" | "queued" => Some("running"),
        _ => None,
    }
}

/// 从工具结果正文里抠出每个子任务的 (agent_id, 状态)。kimi 只以自然语言/XML 形式回吐
/// 这些信息，没有结构化字段：
///
/// - 单发 `Agent`：`agent_id: agent-0` 与 `status: running` 分行写在同一段里
/// - `AgentSwarm`：`<subagent mode="resume" agent_id="agent-3" outcome="completed">`，一份结果里多个
///
/// 两种写法都只认 `agent_id` 这个键（`task_id` 长得很像但不是它），并约束成
/// `agent-<字母数字>`，以免把正文里的其它词当成目录名。保序去重：分支顺序即此顺序。
fn agent_states_from_result(text: &str) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    // XML 形态：逐个 <subagent …> 标签，id 与 outcome 同在标签属性里，必须成对取。
    if text.contains("<subagent") {
        for chunk in text.split("<subagent").skip(1) {
            let attrs = chunk.split('>').next().unwrap_or("");
            let Some(id) = value_after(attrs, "agent_id") else {
                continue;
            };
            if !id.starts_with("agent-") || id.len() <= "agent-".len() {
                continue;
            }
            let status = value_after(attrs, "outcome")
                .as_deref()
                .and_then(normalize_status)
                .map(str::to_string);
            if !out.iter().any(|(seen, _)| *seen == id) {
                out.push((id, status));
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    // 纯文本形态（单发 Agent）：整段只描述一个子任务，且 `status:` 常写在 `agent_id:`
    // 之前，故从整段取状态而不是从 id 之后找。
    let status = value_after(text, "status")
        .as_deref()
        .and_then(normalize_status)
        .map(str::to_string);
    for chunk in text.split("agent_id").skip(1) {
        let rest = chunk.trim_start_matches([':', '=', '"', '\'', ' ', '\t']);
        let id: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !id.starts_with("agent-") || id.len() <= "agent-".len() {
            continue;
        }
        if !out.iter().any(|(seen, _)| *seen == id) {
            out.push((id, status.clone()));
        }
    }
    out
}

/// 取**最长的不含 JSON 转义字符**的片段当特征串：JSON 只转义 `"`、`\` 与控制字符，
/// 避开它们的片段必然在 wire.jsonl 里原样出现，于是无需解析整行就能做子串匹配。
///
/// 取最长而非开头：同一批派发的子任务常常共用一大段开场白（"调研 X 项目（工作目录 …）"），
/// 前缀切片会把它们全配到同一个目录上。最长片段落在各自的正文里，实测能唯一命中。
fn distinctive_slice(text: &str) -> Option<&str> {
    let mut best: Option<(usize, usize)> = None;
    let mut start = 0usize;
    let push = |start: usize, end: usize, best: &mut Option<(usize, usize)>| {
        if end > start && best.is_none_or(|(a, b)| end - start > b - a) {
            *best = Some((start, end));
        }
    };
    for (offset, c) in text.char_indices() {
        if c == '"' || c == '\\' || c.is_control() {
            push(start, offset, &mut best);
            start = offset + c.len_utf8();
        }
    }
    push(start, text.len(), &mut best);
    // 太短的特征串会误配到别的子任务上。
    best.filter(|(a, b)| b - a >= 24).map(|(a, b)| &text[a..b])
}

/// 读文件头部若干字节（lossy）。子 agent 的开场 prompt 在最前面几行，
/// 但其中的 systemPrompt 可能很大，故给足窗口。
fn read_head(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

impl KimiSubagents {
    /// 子任务还在跑时结果尚未写入，`agent_id` 无处可取——而这恰恰是用户最想看进度的时刻。
    /// 好在派发内容会原样出现在对应子 agent 的开场 prompt 里（fan-out 的每个 `item` 经
    /// `prompt_template` 渲染，单发 `Agent` 则是整段 `prompt`），据此反查目录。
    /// 按派发顺序返回，与 TUI 里的 001/002… 编号一致。
    fn locate_by_prompt(agents_dir: &Path, items: &[String]) -> Vec<SubagentStream> {
        /// 开场 prompt 在文件最前面；1 MiB 足以越过 systemPrompt。
        const HEAD_LIMIT: u64 = 1024 * 1024;
        let Ok(entries) = std::fs::read_dir(agents_dir) else {
            return Vec::new();
        };
        let mut heads: Vec<(String, PathBuf, String)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                if !name.starts_with("agent-") {
                    return None;
                }
                let path = entry.path().join("wire.jsonl");
                let head = read_head(&path, HEAD_LIMIT)?;
                Some((name, path, head))
            })
            .collect();
        // 目录名里的编号按数值排序，让同分数的匹配保持 agent-2 < agent-10。
        heads.sort_by_key(|(name, _, _)| {
            name.trim_start_matches("agent-")
                .parse::<u64>()
                .unwrap_or(u64::MAX)
        });
        let mut used = std::collections::HashSet::new();
        items
            .iter()
            .filter_map(|item| {
                let needle = distinctive_slice(item)?;
                let (name, path, _) = heads
                    .iter()
                    .find(|(name, _, head)| !used.contains(name) && head.contains(needle))?;
                used.insert(name.clone());
                Some(SubagentStream {
                    label: Some(name.clone()),
                    // 走到这条路径就说明结果还没落盘，即还在跑。
                    status: Some("running".to_string()),
                    path: path.clone(),
                })
            })
            .collect()
    }
}

impl SubagentSpec for KimiSubagents {
    fn detect_call(
        &self,
        tool_name: &str,
        input: Option<&serde_json::Value>,
    ) -> Option<SubagentRef> {
        // `Agent` 单发一个；`AgentSwarm` 一次派一批——两种形态：
        // fan-out（`items` 每项一个子任务）与 resume（`resume_agent_ids` 恢复既有的一批）。
        if !matches!(tool_name, "Agent" | "AgentSwarm") {
            return None;
        }
        let input = input?;
        let count = input
            .get("items")
            .and_then(|v| v.as_array())
            .map(|items| items.len())
            .or_else(|| {
                input
                    .get("resume_agent_ids")
                    .and_then(|v| v.as_object())
                    .map(|ids| ids.len())
            })
            .unwrap_or(1);
        Some(SubagentRef {
            description: input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            agent_type: input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            count: count.max(1) as u32,
        })
    }

    fn locate_streams(&self, main_transcript: &Path, tool_use_id: &str) -> Vec<SubagentStream> {
        // .../agents/main/wire.jsonl → .../agents/
        let Some(agents_dir) = main_transcript.parent().and_then(Path::parent) else {
            return Vec::new();
        };
        let Ok(main) = std::fs::read_to_string(main_transcript) else {
            return Vec::new();
        };
        let mut ids: Vec<(String, Option<String>)> = Vec::new();
        let mut fanout_items: Vec<String> = Vec::new();
        for line in main.lines() {
            // 只解析确实提到这个 callId 的行，避免为整份主流做全量 JSON 解析。
            if !line.contains(tool_use_id) {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(event) = value.get("event") else {
                continue;
            };
            let call_id = event
                .get("callId")
                .or_else(|| event.get("call_id"))
                .or_else(|| event.get("toolCallId"))
                .and_then(|v| v.as_str());
            if call_id != Some(tool_use_id) {
                continue;
            }
            match event.get("type").and_then(|v| v.as_str()) {
                // resume 形态的权威名单直接写在调用参数里，不必等结果。
                Some("tool.call" | "tool_call") => {
                    if let Some(resume) = event
                        .pointer("/args/resume_agent_ids")
                        .or_else(|| event.pointer("/input/resume_agent_ids"))
                        .and_then(|v| v.as_object())
                    {
                        // 恢复的一批在结果落盘前都在跑；真实结局稍后由结果覆盖。
                        ids.extend(
                            resume
                                .keys()
                                .map(|id| (id.clone(), Some("running".to_string()))),
                        );
                    }
                    // 派发内容：swarm 是 items 逐项，单发 Agent 是整段 prompt。
                    // 两者都用于「结果尚未写入时」按开场 prompt 反查目录。
                    match event
                        .pointer("/args/items")
                        .or_else(|| event.pointer("/input/items"))
                        .and_then(|v| v.as_array())
                    {
                        Some(items) => fanout_items.extend(
                            items
                                .iter()
                                .filter_map(|item| item.as_str())
                                .map(str::to_string),
                        ),
                        None => {
                            if let Some(prompt) = event
                                .pointer("/args/prompt")
                                .or_else(|| event.pointer("/input/prompt"))
                                .and_then(|v| v.as_str())
                            {
                                fanout_items.push(prompt.to_string());
                            }
                        }
                    }
                }
                // fan-out 形态的 id 只在结果里（`<subagent agent_id="agent-3">`）。
                Some("tool.result" | "tool_result") => {
                    if let Some(output) = event
                        .get("output")
                        .or_else(|| event.pointer("/result/output"))
                        .or_else(|| event.get("result"))
                        .map(value_text)
                    {
                        ids.extend(agent_states_from_result(&output));
                    }
                }
                _ => {}
            }
        }
        // 同一批 id 会被参数（resume）和结果各报一次。保序去重，并让**后出现**的状态胜出：
        // 参数只能说明"已派出"，结果才带真实结局。
        let mut merged: Vec<(String, Option<String>)> = Vec::new();
        for (id, status) in ids {
            match merged.iter_mut().find(|(seen, _)| *seen == id) {
                Some(entry) => {
                    if status.is_some() {
                        entry.1 = status;
                    }
                }
                None => merged.push((id, status)),
            }
        }
        // fan-out 的 agent_id 只写在结果里。swarm 仍在跑时结果还没落盘——正是用户最想看
        // 进度的时刻——只能靠开场 prompt 反查目录。
        if merged.is_empty() && !fanout_items.is_empty() {
            return Self::locate_by_prompt(agents_dir, &fanout_items);
        }
        merged
            .into_iter()
            .map(|(id, status)| SubagentStream {
                path: agents_dir.join(&id).join("wire.jsonl"),
                // 一批子任务必须能分辨谁是谁；单发时也显示，kimi 的 id 本身就是可读的。
                label: Some(id),
                status,
            })
            .filter(|stream| stream.path.is_file())
            .collect()
    }

    fn parse_stream_line(&self, line: &str) -> Vec<TranscriptEvent> {
        // 子 agent 的 wire.jsonl 与主流同格式，直接复用。子任务的「用户消息」是编排方
        // 的 prompt，不含粘贴图片，无需 blobref 的目录上下文。
        parse_events(line, None)
    }

    fn detect_result(&self, output: &str) -> Option<SubagentOutcome> {
        let states = agent_states_from_result(output);
        if states.is_empty() {
            return None;
        }
        let mut outcome = SubagentOutcome {
            running: 0,
            completed: 0,
            failed: 0,
            // kimi 的结局直接写在委派自己的回执里，不存在需要跨调用路由的场景。
            task_id: None,
        };
        for (_, status) in &states {
            match status.as_deref() {
                Some("completed") => outcome.completed += 1,
                Some("failed") => outcome.failed += 1,
                // 没写状态的分支按「在跑」计——派出去了但还没有结局。
                _ => outcome.running += 1,
            }
        }
        Some(outcome)
    }
}

pub struct KimiTranscript;
pub static KIMI_TRANSCRIPT: KimiTranscript = KimiTranscript;

impl TranscriptSpec for KimiTranscript {
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
            .or_else(|| {
                session_dir(session_id)
                    .map(|dir| dir.join("agents").join("main").join("wire.jsonl"))
                    .filter(|path| path.exists())
            })
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
        parse_events(line, None)
    }

    fn parse_transcript_line_in(&self, path: &Path, line: &str) -> Vec<TranscriptEvent> {
        // wire.jsonl 旁的 blobs/ 目录是用户图片 blobref 的解析基准。
        parse_events(line, path.parent())
    }

    fn agent_modes_from_line(&self, line: &str) -> Vec<crate::AgentMode> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return Vec::new();
        };
        match value.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "permission.set_mode" => value
                .get("mode")
                .and_then(|v| v.as_str())
                .map(|mode| vec![crate::AgentMode::new("permission", mode)])
                .unwrap_or_default(),
            "plan_mode.enter" => vec![crate::AgentMode::new("work", "plan")],
            "plan_mode.exit" => vec![crate::AgentMode::new("work", "default")],
            _ => Vec::new(),
        }
    }

    fn default_agent_modes(&self) -> Vec<crate::AgentMode> {
        vec![
            crate::AgentMode::new("work", "default"),
            crate::AgentMode::new("permission", "manual"),
        ]
    }

    fn supports_chat(&self) -> bool {
        true
    }

    fn subagents(&self) -> Option<&'static dyn SubagentSpec> {
        Some(&KIMI_SUBAGENTS)
    }

    fn supports_analysis(&self) -> bool {
        false
    }
}

/// kimi 在本机的实况（走哪个变体、数据目录/配置/凭据/可执行在哪）。变体表见
/// `meowo_agent::plugins::kimi`：新版 Node「Kimi Code」`~/.kimi-code` 优先，旧 Python 版
/// `kimi-cli` 的 `~/.kimi` 兼容（两者 hook 格式一致）；都没装则给出新版的默认落点。
/// 检测/接线/状态/账号凭据/会话读取全部经此一处解析路径。
pub fn kimi_install() -> Option<crate::Installation> {
    crate::registry::installation(crate::id::KIMI)
}

/// kimi 数据根。`kimi_install()` 的便捷取值——调用方只关心目录时用它。
pub fn kimi_share_dir() -> Option<PathBuf> {
    kimi_install().map(|i| i.data_dir)
}

/// Meowo 管理的 kimi profile 数据目录（每个 profile 根就是它的 `KIMI_SHARE_DIR`，见
/// `plugins/kimi` 的 `PROFILE` 声明）。多账号会话的 `session_index.jsonl` 落在这些目录里，
/// 而 meowo-app 进程没有 `KIMI_SHARE_DIR`——只查默认目录会让 profile 会话的
/// 改名/摘要/上下文全部静默落空，故会话目录查找必须把它们纳入候选。
/// 目录布局与 claude 侧 `managed_projects_dirs` 同源（`~/.meowo/profiles/<agent>/<id>`），
/// 数据根解析与目录枚举（含 symlink/junction 解引用）收敛在 `crate::managed_profile_dirs`。
fn managed_share_dirs() -> Vec<PathBuf> {
    crate::managed_profile_dirs(crate::id::KIMI.as_str())
}

/// kimi 的启动 argv（单元素：可执行绝对路径；找不到回退裸名 "kimi" 走 PATH）。
/// resume/launch 用：meowo-app 拉起的终端 PATH 未必含 kimi（或 kimi 是 shim/别名），故优先绝对路径，
/// 避免 wt/powershell「系统找不到指定的文件」。
pub fn kimi_launch_argv() -> Vec<String> {
    kimi_install()
        .map(|i| i.launch_argv())
        .unwrap_or_else(|| vec!["kimi".to_string()])
}

/// kimi 可执行是否真实落在某个已知位置（区别于 `kimi_launch_argv` 找不到时回退裸名）。
pub fn kimi_installed() -> bool {
    kimi_install().is_some_and(|i| i.is_launchable())
}

/// 从 `session_index.jsonl` 查 session_id 对应的会话目录（kimi 的目录名带哈希，靠此索引而非自己算）。
/// 依次查默认数据目录与全部受管 profile 目录：session_id 全局唯一，首个命中即返回。
/// reporter 由 profile 里的 kimi 派生时自带 `KIMI_SHARE_DIR`，`kimi_share_dir()` 即命中；
/// meowo-app 没有该变量，profile 会话靠 `managed_share_dirs()` 兜底。
fn session_dir(session_id: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.extend(kimi_share_dir());
    candidates.extend(managed_share_dirs());
    candidates
        .into_iter()
        .find_map(|dir| session_dir_in(&dir, session_id))
}

/// 在**某个**数据目录的 `session_index.jsonl` 里查 session_id。
fn session_dir_in(share_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let idx = share_dir.join("session_index.jsonl");
    let content = std::fs::read_to_string(idx).ok()?;
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("sessionId").and_then(|s| s.as_str()) == Some(session_id) {
            return v
                .get("sessionDir")
                .and_then(|s| s.as_str())
                .map(PathBuf::from);
        }
    }
    None
}

/// 从 wire.jsonl 提取的会话摘要。
#[derive(Debug, Default, PartialEq)]
pub struct WireSummary {
    /// 最近一条 AI 正文（最后一条用户输入之后拼接的 text 片段）。
    pub last_ai: Option<String>,
    /// 模型展示名（由模型 alias 映射，见 model_display）。
    pub model: Option<String>,
}

/// 模型 alias → 展示名。alias 形如 "kimi-code/kimi-for-coding"；取末段，已知模型映射成 kimi
/// 自己显示的名字，未知则用末段本身（不致空白）。
fn model_display(alias: &str) -> String {
    let last = alias.rsplit('/').next().unwrap_or(alias);
    match last {
        "kimi-for-coding" => "K2.7 Code".to_string(),
        // kimi CLI 底栏自己显示 "K3"。
        "k3" => "K3".to_string(),
        other => other.to_string(),
    }
}

/// 纯解析：从 wire.jsonl 文本取最近 AI 文本（拼接最后一回合的 text 片段、跳过 think）+ 模型展示名。
/// 便于单测，不碰文件系统。
pub fn parse_wire(content: &str) -> WireSummary {
    let mut buf = String::new();
    let mut model: Option<String> = None;
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            // 新用户回合开始 → 清空，使最终缓冲只剩最后一回合的 AI 文本。
            "turn.prompt" => buf.clear(),
            "context.append_message" => {
                let role = v
                    .get("message")
                    .and_then(|m| m.get("role"))
                    .and_then(|r| r.as_str());
                if role == Some("user") {
                    buf.clear();
                }
            }
            "config.update" => {
                if let Some(a) = v.get("modelAlias").and_then(|m| m.as_str()) {
                    model = Some(model_display(a));
                }
            }
            "context.append_loop_event" => {
                let part = v
                    .get("event")
                    .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("content.part"));
                let part = part.and_then(|e| e.get("part"));
                if part.and_then(|p| p.get("type")).and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = part.and_then(|p| p.get("text")).and_then(|t| t.as_str()) {
                        buf.push_str(t);
                    }
                }
            }
            _ => {}
        }
    }
    let s = buf.trim();
    WireSummary {
        last_ai: (!s.is_empty()).then(|| s.to_string()),
        model,
    }
}

/// 小文件全量读的上限；超过则改头/尾有界读，避免长会话下每个 Stop 都全量读+解析（近 O(n²)）。
const FULL_READ_CAP: u64 = 512 * 1024;
/// 大文件时：头部读这么多取模型（config.update 在文件靠前），尾部读这么多取最近 AI 正文。
const HEAD_BYTES: u64 = 16 * 1024;
const TAIL_BYTES: u64 = 256 * 1024;

/// 读文件 [offset, offset+len) 字节为 lossy UTF-8（边界处的半截行交给 parse_wire 跳过）。
fn read_range(path: &Path, offset: u64, len: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::with_capacity(len.min(TAIL_BYTES) as usize);
    f.take(len).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 读某 kimi 会话的 wire.jsonl 并解析（定位失败/读失败返回 None）。
/// wire.jsonl 是只增的会话日志，长会话可达数 MB；模型在头部、最近 AI 正文在尾部，故大文件分别
/// 头/尾有界读，small 文件仍一次全读。set_last_ai_text 本就截断 200 字，尾部窗口足够。
pub fn read_summary(session_id: &str) -> Option<WireSummary> {
    let wire = session_dir(session_id)?
        .join("agents")
        .join("main")
        .join("wire.jsonl");
    let size = std::fs::metadata(&wire).ok()?.len();
    if size <= FULL_READ_CAP {
        return Some(parse_wire(&std::fs::read_to_string(&wire).ok()?));
    }
    let head = read_range(&wire, 0, HEAD_BYTES).unwrap_or_default();
    let tail = read_range(&wire, size.saturating_sub(TAIL_BYTES), TAIL_BYTES).unwrap_or_default();
    let tail_summary = parse_wire(&tail);
    Some(WireSummary {
        // 模型**不只**在头部：会话中途 `/model` 切换会在文件后段再写一条 `config.update`
        // （实测尾部可见 `kimi-for-coding → k3 → kimi-for-coding` 这样的切换序列）。
        // 只读头部的话，切换后界面永远停在最初那个模型上。尾部优先、头部兜底。
        model: tail_summary.model.or_else(|| parse_wire(&head).model),
        last_ai: tail_summary.last_ai,
    })
}

/// 从 wire.jsonl 文本取**最后一条** usage.record 的 (used_input_tokens, model_alias)。
/// used = inputOther + inputCacheRead + inputCacheCreation（≈ 该回合请求发送时的 context 输入量，
/// 每次请求都把整个 context 作为 input 发送）；output 不计（本轮新生成，尚未进 context）。
pub fn parse_context(content: &str) -> Option<(i64, String)> {
    let mut last: Option<(i64, String)> = None;
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("usage.record") {
            continue;
        }
        let Some(u) = v.get("usage") else { continue };
        let field = |k: &str| u.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        let used = field("inputOther") + field("inputCacheRead") + field("inputCacheCreation");
        let model = v
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        last = Some((used, model));
    }
    last
}

/// 读 config.toml 里 `[models."<alias>"]` 的 `max_context_size`；找不到回退 262144。
/// 逐行启发式解析（不引 toml 依赖，同 account/kimi.rs 既有范式）。
pub fn context_window(model_alias: &str) -> i64 {
    const FALLBACK: i64 = 262_144;
    let Some(inst) = kimi_install() else {
        return FALLBACK;
    };
    let Ok(content) = std::fs::read_to_string(inst.config_path()) else {
        return FALLBACK;
    };
    let want = format!("[models.\"{model_alias}\"]");
    let mut in_section = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == want;
            continue;
        }
        if !in_section || t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("max_context_size") {
            if let Some(after) = rest.trim_start().strip_prefix('=') {
                if let Ok(n) = after.trim().parse::<i64>() {
                    if n > 0 {
                        return n;
                    }
                }
            }
        }
    }
    FALLBACK
}

/// 读某 kimi 会话最近的上下文占用：wire.jsonl 尾部取最后一条 usage.record，used/window 算百分比。
/// 定位/读/解析失败返回 None。大文件尾部有界读（与 read_summary 同款）。
/// 从 wire.jsonl 文本取**最后一次** `TodoList` 调用的整份待办快照。
/// 状态词原样带出（kimi 写 `done`），归一化留给 DB 层。
pub fn parse_todos(content: &str) -> Option<Vec<crate::caps::TodoSnapshot>> {
    let mut last = None;
    for line in content.lines() {
        // 先做廉价的子串筛，避免为整份日志做全量 JSON 解析。
        if !line.contains("TodoList") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(event) = value.get("event") else {
            continue;
        };
        if !matches!(
            event.get("type").and_then(|v| v.as_str()),
            Some("tool.call" | "tool_call")
        ) || event.get("name").and_then(|v| v.as_str()) != Some("TodoList")
        {
            continue;
        }
        // 用 continue 而不是 `?`：某一行格式异常只该跳过它，不能让整个函数提前返回、
        // 把前面已经找到的快照一并丢掉。
        let Some(todos) = event
            .pointer("/args/todos")
            .or_else(|| event.pointer("/input/todos"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        last = Some(
            todos
                .iter()
                .filter_map(|todo| {
                    let content = todo
                        .get("title")
                        .or_else(|| todo.get("content"))
                        .and_then(|v| v.as_str())?;
                    Some(crate::caps::TodoSnapshot {
                        content: content.to_string(),
                        status: todo
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("pending")
                            .to_string(),
                    })
                })
                .collect(),
        );
    }
    last
}

/// 读某 kimi 会话当前的待办快照。整读上限内全读；超限只读尾部——待办是整份覆盖写的，
/// 最后一次调用必然在尾部。
pub fn read_todos(session_id: &str) -> Option<Vec<crate::caps::TodoSnapshot>> {
    let wire = session_dir(session_id)?
        .join("agents")
        .join("main")
        .join("wire.jsonl");
    let size = std::fs::metadata(&wire).ok()?.len();
    let text = if size <= FULL_READ_CAP {
        std::fs::read_to_string(&wire).ok()?
    } else {
        read_range(&wire, size.saturating_sub(TAIL_BYTES), TAIL_BYTES)?
    };
    parse_todos(&text)
}

pub fn read_context(session_id: &str) -> Option<crate::caps::ContextUsage> {
    let wire = session_dir(session_id)?
        .join("agents")
        .join("main")
        .join("wire.jsonl");
    let size = std::fs::metadata(&wire).ok()?.len();
    let text = if size <= FULL_READ_CAP {
        std::fs::read_to_string(&wire).ok()?
    } else {
        read_range(&wire, size.saturating_sub(TAIL_BYTES), TAIL_BYTES)?
    };
    let (used, model) = parse_context(&text)?;
    let window = context_window(&model);
    if window <= 0 {
        return None;
    }
    let pct = (used * 100 / window).clamp(0, 100);
    Some(crate::caps::ContextUsage {
        used_pct: pct,
        window,
        // usage.record 里就带着模型别名,顺手转成展示名带出——否则模型要等第一次 Stop
        // 才落库,新会话第一回合期间界面上一直是空的。
        model: (!model.is_empty()).then(|| model_display(&model)),
    })
}

/// 把某 kimi 会话改成自定义标题：改写 session `state.json` 的 `title` + `isCustomTitle=true`
/// （后者阻止 kimi 之后用 AI 标题覆盖，与 claude 的 custom-title 同义），使 kimi 自身会话列表与
/// `kimi -r` 列表也显示新名。其余字段原样保留。写回走 [`crate::fsutil::write_atomic`]
/// （pid+序号临时名 + rename + 失败清理），避免与运行中的 kimi 并发写 state.json 撕裂，
/// 也避免并发改名互踩同一个固定临时名、崩溃后残留临时文件。定位/读/解析/写失败返回 false。
pub fn set_custom_title(session_id: &str, title: &str) -> bool {
    let Some(dir) = session_dir(session_id) else {
        return false;
    };
    let path = dir.join("state.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let Some(obj) = v.as_object_mut() else {
        return false;
    };
    obj.insert(
        "title".to_string(),
        serde_json::Value::String(title.to_string()),
    );
    obj.insert("isCustomTitle".to_string(), serde_json::Value::Bool(true));
    let Ok(s) = serde_json::to_string(&v) else {
        return false;
    };
    crate::fsutil::write_atomic(&path, &s).is_ok()
}

// ═══ 能力槽 ═══

pub struct KimiTelemetry;
pub static TELEMETRY: KimiTelemetry = KimiTelemetry;

impl crate::caps::TelemetryCap for KimiTelemetry {
    /// kimi 的 Stop hook 不带正文/模型 → 从 wire.jsonl 一次读出两者（避免双读）。
    fn stop_outputs(&self, ctx: &crate::caps::HookContext) -> crate::caps::StopOutputs {
        match read_summary(ctx.session_id) {
            Some(s) => crate::caps::StopOutputs {
                last_ai: s.last_ai,
                model: s.model,
            },
            None => crate::caps::StopOutputs::default(),
        }
    }

    fn read_context(&self, ctx: &crate::caps::HookContext) -> Option<crate::caps::ContextUsage> {
        read_context(ctx.session_id)
    }

    fn read_todos(&self, ctx: &crate::caps::HookContext) -> Option<Vec<crate::caps::TodoSnapshot>> {
        read_todos(ctx.session_id)
    }

    fn write_rename(&self, session_id: &str, _cwd: Option<&str>, title: &str) -> bool {
        set_custom_title(session_id, title)
    }

    fn transcript(&self) -> Option<&'static dyn TranscriptSpec> {
        Some(&KIMI_TRANSCRIPT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_independent_permission_and_work_modes() {
        assert_eq!(
            KIMI_TRANSCRIPT
                .agent_modes_from_line(r#"{"type":"permission.set_mode","mode":"yolo"}"#,),
            vec![crate::AgentMode::new("permission", "yolo")]
        );
        assert_eq!(
            KIMI_TRANSCRIPT.agent_modes_from_line(r#"{"type":"plan_mode.enter"}"#),
            vec![crate::AgentMode::new("work", "plan")]
        );
        assert_eq!(
            KIMI_TRANSCRIPT.agent_modes_from_line(r#"{"type":"plan_mode.exit"}"#),
            vec![crate::AgentMode::new("work", "default")]
        );
    }

    /// 从会话日志重建待办：取**最后一次** TodoList 的整份快照，状态词原样带出
    /// （kimi 写 `done`，归一化是 DB 层的事），标题字段 title/content 都认。
    #[test]
    fn parse_todos_takes_last_snapshot_and_keeps_agent_wording() {
        // wire.jsonl 是 JSONL：**每行必须是完整 JSON**，不能跨行。
        let wire = r#"
{"type":"context.append_loop_event","event":{"type":"tool.call","name":"TodoList","args":{"todos":[{"title":"旧的一条","status":"pending"}]}}}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"干活"}}}
{"type":"context.append_loop_event","event":{"type":"tool.call","name":"TodoList","args":{"todos":[{"title":"定位根因","status":"done"},{"title":"修复并验证","status":"in_progress"},{"content":"推送分支","status":"pending"}]}}}
"#;
        let todos = parse_todos(wire).expect("应读到待办快照");
        assert_eq!(
            todos
                .iter()
                .map(|t| (t.content.as_str(), t.status.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("定位根因", "done"),
                ("修复并验证", "in_progress"),
                ("推送分支", "pending"),
            ],
            "应取最后一次快照，且保留 agent 自己的状态词"
        );

        // 没有 TodoList 调用时返回 None——「读不到」不等于「清单为空」，
        // 上层据此保持 DB 现状，不能拿空列表覆盖 hook 已落好的数据。
        assert!(parse_todos(r#"{"type":"usage.record","model":"x"}"#).is_none());
    }

    #[test]
    fn parse_context_takes_last_usage_record_and_sums_inputs() {
        let wire = r#"
{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":100,"output":5,"inputCacheRead":200,"inputCacheCreation":0}}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"hi"}}}
{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":727,"output":815,"inputCacheRead":20480,"inputCacheCreation":13}}
"#;
        // 取最后一条：727 + 20480 + 13 = 21220；output 不计。
        assert_eq!(
            parse_context(wire),
            Some((21220, "kimi-code/kimi-for-coding".to_string()))
        );
    }

    #[test]
    fn parse_context_none_when_no_usage_record() {
        let wire = r#"{"type":"turn.prompt","input":"hi"}"#;
        assert_eq!(parse_context(wire), None);
    }

    /// usage 通道的模型展示名(read_context 会带出,PostToolUse 就落库,不必等 Stop):
    /// 已知别名映射成 kimi 自己显示的名字,未知取末段原样。
    #[test]
    fn model_display_maps_known_aliases() {
        assert_eq!(model_display("kimi-code/kimi-for-coding"), "K2.7 Code");
        assert_eq!(model_display("kimi-code/k3"), "K3");
        assert_eq!(model_display("k3"), "K3");
        assert_eq!(model_display("vendor/some-new-model"), "some-new-model");
    }

    #[test]
    fn extracts_last_ai_text_and_model_skipping_think_and_prior_turns() {
        // 两个回合；应只取第二回合的 text，跳过 think、忽略第一回合；模型由 alias 映射。
        let wire = r#"
{"type":"config.update","modelAlias":"kimi-code/kimi-for-coding"}
{"type":"turn.prompt","input":"hi"}
{"type":"context.append_message","message":{"role":"user","content":"hi"}}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":"想一下"}}}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"Hi! "}}}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"How can I help?"}}}
{"type":"turn.prompt","input":"再见"}
{"type":"context.append_message","message":{"role":"user","content":"再见"}}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"再见！"}}}
"#;
        let s = parse_wire(wire);
        assert_eq!(s.last_ai.as_deref(), Some("再见！"));
        assert_eq!(s.model.as_deref(), Some("K2.7 Code"));
    }

    #[test]
    fn none_when_no_text_parts() {
        let wire = r#"{"type":"turn.prompt","input":"hi"}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":"only thinking"}}}"#;
        assert_eq!(parse_wire(wire).last_ai, None);
    }

    #[test]
    fn chat_parser_emits_user_and_mergeable_assistant_deltas() {
        let user = r#"{"type":"turn.prompt","input":"继续"}"#;
        let first = r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"正在"}}}"#;
        let second = r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"处理"}}}"#;
        let thinking = r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":"先分析"}}}"#;
        assert!(
            matches!(&parse_chat_items(user)[0], ChatItem::UserText { text, .. } if text == "继续")
        );
        assert!(
            matches!(&parse_chat_items(first)[0], ChatItem::AssistantDelta { text, .. } if text == "正在")
        );
        assert!(
            matches!(&parse_chat_items(second)[0], ChatItem::AssistantDelta { text, .. } if text == "处理")
        );
        assert!(matches!(
            &parse_chat_items(thinking)[0],
            ChatItem::ReasoningDelta { text, .. } if text == "先分析"
        ));
    }

    #[test]
    fn chat_parser_emits_context_append_message_user_content() {
        let string_content = r#"{"type":"context.append_message","timestamp":"2026-07-18T01:02:03Z","message":{"role":"user","content":"从这里继续"}}"#;
        let array_content = r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"第一段"},{"type":"text","text":"第二段"}]}}"#;
        let assistant = r#"{"type":"context.append_message","message":{"role":"assistant","content":"不要重复解析"}}"#;
        let injected = r#"{"type":"context.append_message","message":{"role":"user","content":"hidden reminder","origin":{"kind":"injection"}}}"#;

        assert!(matches!(
            &parse_chat_items(string_content)[0],
            ChatItem::UserText { timestamp: Some(timestamp), text, .. }
                if timestamp == "2026-07-18T01:02:03Z" && text == "从这里继续"
        ));
        assert!(matches!(
            &parse_chat_items(array_content)[0],
            ChatItem::UserText { text, .. } if text == "第一段第二段"
        ));
        assert!(parse_chat_items(assistant).is_empty());
        assert!(parse_chat_items(injected).is_empty());
    }

    /// 回归：wire 行的时间字段是 `time`（epoch 毫秒**数字**，实档每行都带），不是
    /// `timestamp`——读错字段时所有 kimi 消息无时间。转换成与 claude 一致的 ISO-8601
    /// UTC 字符串；旧形态的 ISO 字符串 `timestamp` 保留兜底。
    #[test]
    fn line_time_epoch_ms_becomes_iso_timestamp() {
        assert_eq!(iso_from_epoch_ms(0), "1970-01-01T00:00:00.000Z");
        // 闰日边界。
        assert_eq!(iso_from_epoch_ms(951_867_296_789), "2000-02-29T23:34:56.789Z");
        // 实档取样值。
        assert_eq!(iso_from_epoch_ms(1_786_678_632_036), "2026-08-14T03:37:12.036Z");

        let line = r#"{"type":"turn.prompt","input":"hi","time":1786678632036}"#;
        assert!(matches!(
            &parse_chat_items(line)[0],
            ChatItem::UserText { timestamp: Some(ts), .. } if ts == "2026-08-14T03:37:12.036Z"
        ));
        let legacy = r#"{"type":"turn.prompt","input":"hi","timestamp":"2026-07-18T01:02:03Z"}"#;
        assert!(matches!(
            &parse_chat_items(legacy)[0],
            ChatItem::UserText { timestamp: Some(ts), .. } if ts == "2026-07-18T01:02:03Z"
        ));
    }

    /// 回归：id 是宿主的唯一身份（React key、水位线去重），整行内容哈希会让内容相同的
    /// 两条记录同 id。loop 事件用自带的 event.uuid（不同事件必不同 id）；同一行重放
    /// （增量 reset）id 必须稳定；工具结果没有 uuid，callId 是它的稳定唯一键。
    #[test]
    fn chat_ids_are_unique_per_record_and_stable_across_reads() {
        let first = r#"{"type":"context.append_loop_event","time":1,"event":{"type":"content.part","uuid":"u-1","part":{"type":"text","text":"好的"}}}"#;
        let twin = r#"{"type":"context.append_loop_event","time":1,"event":{"type":"content.part","uuid":"u-2","part":{"type":"text","text":"好的"}}}"#;
        let id_of = |line: &str| match &parse_chat_items(line)[0] {
            ChatItem::AssistantDelta { id, .. } => id.clone(),
            other => panic!("应是 AssistantDelta：{other:?}"),
        };
        assert_ne!(id_of(first), id_of(twin), "同文案不同事件必须不同 id");
        assert_eq!(id_of(first), id_of(first), "同一行重放 id 必须稳定");

        let result = r#"{"type":"context.append_loop_event","time":2,"event":{"type":"tool.result","toolCallId":"tool-9","result":{"output":"ok"}}}"#;
        assert!(matches!(
            &parse_chat_items(result)[0],
            ChatItem::ToolResult { id, .. } if id == "kimi-result-tool-9"
        ));
    }

    /// 用户粘贴的图片在 wire.jsonl 里是 `image_url` part：blobref 指向 wire 旁的
    /// `blobs/<sha>`（无扩展名）。修复前只拼 text part——图片静默丢失、纯图消息整条消失
    /// （实拍反馈：kimi 会话的图在对话里看不到）。落盘到 paste_root/queued/（asset scope
    /// 内），以 `[Image: source: …]` 行并入正文，前端现成的缩略图链路直接接住。
    #[test]
    fn user_message_blobref_images_are_persisted_and_referenced() {
        let root = std::env::temp_dir().join(format!("kimi-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let wire_dir = root.join("agents").join("main");
        std::fs::create_dir_all(wire_dir.join("blobs")).unwrap();
        let sha1 = format!("{:064x}", std::process::id());
        let sha2 = format!("{:064x}", std::process::id() as u64 + 1);
        std::fs::write(wire_dir.join("blobs").join(&sha1), b"png-bytes-1").unwrap();
        std::fs::write(wire_dir.join("blobs").join(&sha2), b"png-bytes-2").unwrap();

        let line = format!(
            r#"{{"type":"turn.prompt","input":[{{"type":"image_url","imageUrl":{{"url":"blobref:image/png;{sha1}"}}}},{{"type":"image_url","imageUrl":{{"url":"blobref:image/png;{sha2}"}}}},{{"type":"text","text":" 做成类似这样的"}}]}}"#
        );
        let queued = crate::fsutil::paste_root().join("queued");
        let path1 = queued.join(format!("kimi-{sha1}.png"));
        let path2 = queued.join(format!("kimi-{sha2}.png"));
        let _ = std::fs::remove_file(&path1);
        let _ = std::fs::remove_file(&path2);

        let items = parse_chat_items_in(Some(&wire_dir), &line);
        let ChatItem::UserText { text, .. } = &items[0] else {
            panic!("应解析成用户消息");
        };
        assert_eq!(
            text,
            &format!(
                " 做成类似这样的\n[Image: source: {}]\n[Image: source: {}]",
                path1.display(),
                path2.display()
            )
        );
        assert_eq!(std::fs::read(&path1).unwrap(), b"png-bytes-1");
        assert_eq!(std::fs::read(&path2).unwrap(), b"png-bytes-2");

        // 无目录上下文（旧调用形态）时图片解析不出、退化不显示，但文字不丢。
        let items = parse_chat_items(&line);
        assert!(
            matches!(&items[0], ChatItem::UserText { text, .. } if text == " 做成类似这样的")
        );

        let _ = std::fs::remove_file(&path1);
        let _ = std::fs::remove_file(&path2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 纯图消息（没有 text part）也要产出用户消息——修复前被空文本检查整条吞掉。
    #[test]
    fn pure_image_user_message_survives() {
        let root = std::env::temp_dir().join(format!("kimi-imgonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let wire_dir = root.join("agents").join("main");
        std::fs::create_dir_all(wire_dir.join("blobs")).unwrap();
        let sha = format!("{:064x}", std::process::id() as u64 + 2);
        std::fs::write(wire_dir.join("blobs").join(&sha), b"only-image").unwrap();

        let line = format!(
            r#"{{"type":"context.append_message","message":{{"role":"user","content":[{{"type":"image_url","imageUrl":{{"url":"blobref:image/png;{sha}"}}}}]}}}}"#
        );
        let path = crate::fsutil::paste_root()
            .join("queued")
            .join(format!("kimi-{sha}.png"));
        let _ = std::fs::remove_file(&path);

        let items = parse_chat_items_in(Some(&wire_dir), &line);
        assert!(
            matches!(&items[0], ChatItem::UserText { text, .. } if text == &format!("[Image: source: {}]", path.display())),
            "实际：{:?}",
            items
        );

        // blob 已被清理的旧会话：解析不出图也没有文字，消息照旧不出现（与 claude 一致）。
        let missing = line.replace(&sha, &"f".repeat(64));
        assert!(parse_chat_items_in(Some(&wire_dir), &missing).is_empty());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// data: URL 形态（内嵌 base64）：解码落盘，与 claude 排队插图同一条路径。
    #[test]
    fn user_message_data_url_image_is_decoded() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"inline-png-bytes");
        let line = format!(
            r#"{{"type":"turn.prompt","input":[{{"type":"image_url","imageUrl":{{"url":"data:image/png;base64,{data}"}}}},{{"type":"text","text":"看图"}}]}}"#
        );
        let path = crate::fsutil::paste_root()
            .join("queued")
            .join(format!("kimi-{}-0.png", chat_id("user", &line)));
        let _ = std::fs::remove_file(&path);

        let items = parse_chat_items(&line);
        let ChatItem::UserText { text, .. } = &items[0] else {
            panic!("应解析成用户消息");
        };
        assert_eq!(
            text,
            &format!("看图\n[Image: source: {}]", path.display())
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"inline-png-bytes");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn chat_parser_matches_real_kimi_tool_fields() {
        let call = r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"tool-1","name":"Bash","args":{"command":"cargo test"}}}"#;
        let result = r#"{"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"tool-1","result":{"output":"all green"}}}"#;
        assert!(matches!(
            &parse_chat_items(call)[0],
            ChatItem::ToolUse { id, summary, .. }
                if id == "tool-1" && summary.contains("cargo test")
        ));
        assert!(matches!(
            &parse_chat_items(result)[0],
            ChatItem::ToolResult { tool_use_id: Some(id), text, .. }
                if id == "tool-1" && text == "all green"
        ));
    }

    /// kimi 的子任务归属只写在 `Agent` 工具结果的**自然语言正文**里，没有结构化字段。
    /// 这个抠取是整条链路最脆的一环，钉死它的边界。
    #[test]
    fn extracts_agent_ids_and_status_from_free_form_and_swarm_results() {
        let ids = |text: &str| {
            agent_states_from_result(text)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };
        // 单发 Agent：纯文本 `agent_id: agent-0`，且状态写在 id **之前**，要能取到。
        assert_eq!(
            agent_states_from_result(
                "task_id: agent-3hzhkhtr\nstatus: running\nagent_id: agent-0\n"
            ),
            vec![("agent-0".to_string(), Some("running".to_string()))]
        );
        // AgentSwarm：一份结果里多个 XML 属性形式的 id，各带自己的 outcome，保序去重。
        assert_eq!(
            agent_states_from_result(
                r#"<agent_swarm_result><summary>completed: 3</summary>
                   <subagent mode="resume" agent_id="agent-3" outcome="completed">甲</subagent>
                   <subagent agent_id="agent-4" outcome="failed">乙</subagent>
                   <subagent agent_id="agent-3" outcome="completed">重复的不再计一次</subagent>"#
            ),
            vec![
                ("agent-3".to_string(), Some("completed".to_string())),
                ("agent-4".to_string(), Some("failed".to_string())),
            ]
        );
        // 状态必须按分支各取各的，不能让某一个的 outcome 串到别的分支上。
        assert_eq!(
            agent_states_from_result(
                r#"<subagent agent_id="agent-1" outcome="failed">x</subagent>
                   <subagent agent_id="agent-2">没写 outcome</subagent>"#
            ),
            vec![
                ("agent-1".to_string(), Some("failed".to_string())),
                ("agent-2".to_string(), None),
            ]
        );
        // task_id 长得很像但不是它要的那个；必须认 `agent_id` 这个键。
        assert!(ids("task_id: agent-3hzhkhtr\nstatus: running\n").is_empty());
        // 目录名不能被正文里后续的词污染。
        assert_eq!(
            ids("agent_id: agent-12 (resume 用这个 id)"),
            vec!["agent-12"]
        );
        // 光有前缀没有编号不是合法目录名。
        assert!(ids("agent_id: agent-").is_empty());
        assert!(ids("完全无关的输出").is_empty());
    }

    /// AgentSwarm 一次派一批：调用侧要报出规模，定位侧要给出全部分支。
    #[test]
    fn swarm_call_reports_batch_size_and_locates_every_branch() {
        let fanout = r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"tool_s","name":"AgentSwarm","args":{"description":"分组审查全部代码","subagent_type":"explore","items":["甲","乙","丙"],"prompt_template":"审查 {{item}}"}}}"#;
        let ChatItem::ToolUse {
            summary, subagent, ..
        } = &parse_chat_items(fanout)[0]
        else {
            panic!("应解析成工具调用");
        };
        assert_eq!(summary, "分组审查全部代码");
        let subagent = subagent.as_ref().expect("AgentSwarm 也是子任务委派");
        assert_eq!(subagent.count, 3, "fan-out 的规模来自 items 条数");

        // resume 形态：规模来自 resume_agent_ids。
        let resume = r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"tool_r","name":"AgentSwarm","args":{"description":"恢复暂停的 agent","resume_agent_ids":{"agent-3":"继续","agent-4":"继续"}}}}"#;
        let ChatItem::ToolUse { subagent, .. } = &parse_chat_items(resume)[0] else {
            panic!("应解析成工具调用");
        };
        assert_eq!(subagent.as_ref().unwrap().count, 2);

        let root = std::env::temp_dir().join(format!("kimi-swarm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let agents = root.join("agents");
        std::fs::create_dir_all(agents.join("main")).unwrap();
        let main_path = agents.join("main/wire.jsonl");
        let branch = |id: &str, text: &str| {
            std::fs::create_dir_all(agents.join(id)).unwrap();
            std::fs::write(
                agents.join(id).join("wire.jsonl"),
                format!(
                    r#"{{"type":"context.append_loop_event","event":{{"type":"content.part","part":{{"type":"text","text":"{text}"}}}}}}"#
                ) + "\n",
            )
            .unwrap();
        };
        branch("agent-3", "甲的结论");
        branch("agent-4", "乙的结论");
        std::fs::write(
            &main_path,
            format!(
                "{}\n{}\n",
                resume.replace("tool_r", "tool_s"),
                r#"{"type":"context.append_loop_event","event":{"type":"tool.result","callId":"tool_s","output":"<agent_swarm_result><subagent agent_id=\"agent-3\">甲</subagent><subagent agent_id=\"agent-4\">乙</subagent>"}}"#
            ),
        )
        .unwrap();

        let runs = crate::transcript::read_subagent_chat(&KIMI_TRANSCRIPT, &main_path, "tool_s");
        assert_eq!(runs.len(), 2, "一次 swarm 的每个分支都要能展开");
        assert_eq!(runs[0].label.as_deref(), Some("agent-3"));
        assert_eq!(runs[1].label.as_deref(), Some("agent-4"));
        assert!(
            matches!(&runs[0].items[0], ChatItem::AssistantDelta { text, .. } if text == "甲的结论"),
            "实际：{:?}",
            runs[0].items
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 状态挂在主链的工具回执上——折叠状态下就能显示进度，不必先展开去读侧车流。
    #[test]
    fn tool_result_carries_subagent_outcome_for_the_collapsed_badge() {
        let swarm = r#"{"type":"context.append_loop_event","event":{"type":"tool.result","callId":"tool_s","output":"<agent_swarm_result><subagent agent_id=\"agent-1\" outcome=\"completed\">甲</subagent><subagent agent_id=\"agent-2\" outcome=\"failed\">乙</subagent><subagent agent_id=\"agent-3\">还在跑</subagent>"}}"#;
        let ChatItem::ToolResult { subagent, .. } = &parse_chat_items(swarm)[0] else {
            panic!("应解析成工具结果");
        };
        let outcome = subagent.as_ref().expect("swarm 回执应带结局统计");
        assert_eq!(
            (outcome.completed, outcome.failed, outcome.running),
            (1, 1, 1)
        );

        // 单发 Agent 刚派出去：结果里写着 running。
        let single = r#"{"type":"context.append_loop_event","event":{"type":"tool.result","callId":"tool_x","output":"task_id: agent-zzz\nstatus: running\nagent_id: agent-0"}}"#;
        let ChatItem::ToolResult { subagent, .. } = &parse_chat_items(single)[0] else {
            panic!("应解析成工具结果");
        };
        assert_eq!(subagent.as_ref().unwrap().running, 1);

        // 普通工具的回执不该被安上子任务结局。
        let plain = r#"{"type":"context.append_loop_event","event":{"type":"tool.result","callId":"t2","output":"cargo test 通过"}}"#;
        let ChatItem::ToolResult { subagent, .. } = &parse_chat_items(plain)[0] else {
            panic!("应解析成工具结果");
        };
        assert!(subagent.is_none());
    }

    /// 主链识别 + 摘要取描述（args 里的 prompt 上千字，不能进摘要行）。
    #[test]
    fn agent_tool_call_carries_subagent_ref_and_readable_summary() {
        let line = r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"tool_x","name":"Agent","args":{"subagent_type":"explore","description":"评估反混淆效果","prompt":"你在评估一个很长的项目……"}}}"#;
        let ChatItem::ToolUse {
            summary, subagent, ..
        } = &parse_chat_items(line)[0]
        else {
            panic!("应解析成工具调用");
        };
        assert_eq!(summary, "评估反混淆效果");
        let subagent = subagent.as_ref().expect("Agent 调用应带 SubagentRef");
        assert_eq!(subagent.agent_type.as_deref(), Some("explore"));

        // 普通工具不受影响：摘要仍是参数原文，且不标成子任务。
        let plain = r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"t2","name":"Bash","args":{"command":"cargo test"}}}"#;
        let ChatItem::ToolUse { subagent, .. } = &parse_chat_items(plain)[0] else {
            panic!("应解析成工具调用");
        };
        assert!(subagent.is_none());
    }

    /// 侧车流定位：回扫主流找到该 callId 的结果 → 抠 agent_id → `agents/<id>/wire.jsonl`。
    #[test]
    fn locates_subagent_stream_via_main_wire_result() {
        let root = std::env::temp_dir().join(format!("kimi-subagent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let agents = root.join("agents");
        std::fs::create_dir_all(agents.join("main")).unwrap();
        std::fs::create_dir_all(agents.join("agent-0")).unwrap();
        let main = agents.join("main/wire.jsonl");
        std::fs::write(
            &main,
            format!(
                "{}\n{}\n",
                r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"tool_x","name":"Agent","args":{"description":"查一下"}}}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"tool.result","callId":"tool_x","output":"task_id: agent-zzz\nagent_id: agent-0\nstatus: running"}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            agents.join("agent-0/wire.jsonl"),
            format!("{}\n", r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"子任务结论"}}}"#),
        )
        .unwrap();

        let runs = crate::transcript::read_subagent_chat(&KIMI_TRANSCRIPT, &main, "tool_x");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].label.as_deref(), Some("agent-0"));
        assert!(
            matches!(&runs[0].items[0], ChatItem::AssistantDelta { text, .. } if text == "子任务结论"),
            "实际：{:?}",
            runs[0].items
        );
        // 没有对应结果行的 callId 不该猜一个流出来。
        assert!(
            crate::transcript::read_subagent_chat(&KIMI_TRANSCRIPT, &main, "tool_none").is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_resolves_config_path_under_share_dir() {
        // 目录优先级本身由 meowo-agent 的变体表单测覆盖（不碰真实 home）；这里只守住薄封装的一致性：
        // config.toml 必须落在 share_dir 下，argv 非空且指向 kimi。
        let inst = kimi_install().expect("resolve 应总能给出实况或默认落点");
        assert_eq!(
            inst.config_path(),
            kimi_share_dir().unwrap().join("config.toml")
        );
        let argv = kimi_launch_argv();
        assert_eq!(argv.len(), 1);
        assert!(argv[0].to_ascii_lowercase().contains("kimi"));
    }

    /// 回归（多账号）：profile 会话的 `session_index.jsonl` 落在 `~/.meowo/profiles/kimi/<id>/`
    /// 下，而 meowo-app 进程没有 `KIMI_SHARE_DIR`。修复前 `session_dir` 只查默认目录，profile
    /// 会话定位不到 → 改名静默不生效（meowo 里改了名，kimi 自己的会话列表还是旧名）。
    #[test]
    fn session_lookup_and_rename_cover_managed_profile_dirs() {
        let _env = crate::env_guard();
        let home = std::env::temp_dir().join(format!("meowo-kimi-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);

        // 默认账号的数据目录也在，且索引里有**别的**会话——默认目录的查找不能因新候选而回退。
        let default_sid = format!("kimi-default-sid-{}", std::process::id());
        let default_share = home.join(".kimi-code");
        let default_session = default_share.join("sessions/wd/session_default");
        std::fs::create_dir_all(&default_session).unwrap();
        std::fs::write(
            default_share.join("session_index.jsonl"),
            serde_json::json!({"sessionId": default_sid, "sessionDir": default_session})
                .to_string()
                + "\n",
        )
        .unwrap();

        // profile 数据目录 = profile 根（`KIMI_SHARE_DIR` 就指向它，见 plugins/kimi 的 PROFILE）。
        let profile_sid = format!("kimi-profile-sid-{}", std::process::id());
        let profile_share = home
            .join(".meowo")
            .join("profiles")
            .join("kimi")
            .join("work");
        let profile_session = profile_share.join("sessions/wd/session_profile");
        std::fs::create_dir_all(&profile_session).unwrap();
        std::fs::write(
            profile_share.join("session_index.jsonl"),
            serde_json::json!({"sessionId": profile_sid, "sessionDir": profile_session})
                .to_string()
                + "\n",
        )
        .unwrap();
        std::fs::write(profile_session.join("state.json"), r#"{"title":"旧名"}"#).unwrap();

        let old_userprofile = std::env::var("USERPROFILE").ok();
        let old_home = std::env::var("HOME").ok();
        // meowo PTY 里跑测试会带着指向真库的 MEOWO_DB——数据根解析优先读它，
        // 不一并指进临时 home 的话，managed 目录会扫到真实 profile 而不是本用例造的。
        let old_db = std::env::var("MEOWO_DB").ok();
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("HOME", &home);
        std::env::set_var("MEOWO_DB", home.join(".meowo").join("board.db"));

        // 默认目录的会话照常命中；profile 目录的会话也查得到（修复前这里返回 None）。
        assert_eq!(
            session_dir(&default_sid).as_deref(),
            Some(default_session.as_path())
        );
        assert_eq!(
            session_dir(&profile_sid).as_deref(),
            Some(profile_session.as_path())
        );
        assert_eq!(session_dir("no-such-sid"), None);

        // 端到端：profile 会话的改名真正写进它自己目录的 state.json。
        assert!(set_custom_title(&profile_sid, "新名字"));
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(profile_session.join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["title"], "新名字");
        assert_eq!(state["isCustomTitle"], true);
        assert!(!set_custom_title("no-such-sid", "x"));

        match old_userprofile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_db {
            Some(v) => std::env::set_var("MEOWO_DB", v),
            None => std::env::remove_var("MEOWO_DB"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
