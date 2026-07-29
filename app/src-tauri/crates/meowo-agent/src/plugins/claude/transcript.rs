//! claude 的 transcript：`~/.claude/projects/<encode(cwd)>/<session_id>.jsonl` 的路径布局，
//! 与该 JSONL 的增量解析（标题 / 卡死错误 / 上下文占用 / 正文预览）。
//!
//! 这些代码此前住在 `meowo-store`（`analyze.rs` + `title.rs` + `transcript_spec.rs`），于是
//! 「读一个 JSONL 文件」平白拖着 rusqlite 依赖，claude 专属的路径布局也伪装成了通用的 store API。
//! 通用部分（`TranscriptInfo` / trait / `TranscriptCache`）见 `crate::transcript`。

#[cfg(test)]
use crate::transcript::ChatItem;
use crate::transcript::{
    SubagentRef, SubagentSpec, SubagentStream, TranscriptEvent, TranscriptInfo, TranscriptParser,
    TranscriptSpec, TurnError,
};
use std::path::{Path, PathBuf};

fn text_from_content(value: &serde_json::Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    value
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| {
                    part.as_str().map(str::to_string).or_else(|| {
                        part.get("text")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    })
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn compact_json(value: Option<&serde_json::Value>, max: usize) -> String {
    let mut s = value
        .map(|v| {
            if let Some(text) = v.as_str() {
                text.to_string()
            } else {
                v.to_string()
            }
        })
        .unwrap_or_default();
    if s.chars().count() > max {
        s = s.chars().take(max).collect::<String>();
        s.push('…');
    }
    s
}

fn tool_summary(name: &str, input: Option<&serde_json::Value>) -> String {
    let key = match name {
        "Bash" => "command",
        "WebSearch" => "query",
        "Read" | "Write" | "Edit" => "file_path",
        // 子任务委派：摘要取那句任务描述。缺了这条会落到下面的整包 JSON 兜底，
        // 而 prompt 动辄上千字——摘要行会变成一段被截断的 prompt 泥巴。
        "Agent" | "Task" => "description",
        _ => "",
    };
    if !key.is_empty() {
        if let Some(s) = input.and_then(|v| v.get(key)).and_then(|v| v.as_str()) {
            return compact_json(Some(&serde_json::Value::String(s.to_string())), 800);
        }
    }
    compact_json(input, 800)
}

fn parse_transcript_events(line: &str) -> Vec<TranscriptEvent> {
    parse_events(line, false)
}

/// `allow_sidechain`：读**子任务侧车流**时为 true。侧车文件里每一行都带 `isSidechain`，
/// 主流解析主动丢弃它们（子任务过程不该混进主时间线），但读侧车流时它们正是全部内容。
fn parse_events(line: &str, allow_sidechain: bool) -> Vec<TranscriptEvent> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    if !allow_sidechain && v.get("isSidechain").and_then(|x| x.as_bool()) == Some(true) {
        return Vec::new();
    }
    let kind = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    let base_id = v
        .get("uuid")
        .or_else(|| v.pointer("/message/id"))
        .and_then(|x| x.as_str())
        .unwrap_or("message");
    let timestamp = v
        .get("timestamp")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    let content = v.pointer("/message/content");
    match kind {
        "user" => {
            if let Some(text) = content
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                return vec![TranscriptEvent::UserMessage {
                    id: base_id.to_string(),
                    timestamp,
                    text: text.to_string(),
                }];
            }
            content
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(
                    |(i, block)| match block.get("type").and_then(|x| x.as_str()) {
                        Some("text") => block
                            .get("text")
                            .and_then(|x| x.as_str())
                            .filter(|s| !s.trim().is_empty())
                            .map(|text| TranscriptEvent::UserMessage {
                                id: format!("{base_id}:{i}"),
                                timestamp: timestamp.clone(),
                                text: text.to_string(),
                            }),
                        Some("tool_result") => {
                            let text = text_from_content(
                                block.get("content").unwrap_or(&serde_json::Value::Null),
                            );
                            Some(TranscriptEvent::ToolResult {
                                id: format!("{base_id}:{i}"),
                                timestamp: timestamp.clone(),
                                tool_call_id: block
                                    .get("tool_use_id")
                                    .and_then(|x| x.as_str())
                                    .map(str::to_string),
                                text: compact_json(Some(&serde_json::Value::String(text)), 4000),
                                is_error: block
                                    .get("is_error")
                                    .and_then(|x| x.as_bool())
                                    .unwrap_or(false),
                                // claude 的委派回执不带结局（异步子任务的完成经 notification
                                // 另行回来），主链上没有可靠信号，如实留空。
                                subagent: None,
                            })
                        }
                        _ => None,
                    },
                )
                .collect()
        }
        "assistant" => content
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(
                |(i, block)| match block.get("type").and_then(|x| x.as_str()) {
                    Some("text") => block
                        .get("text")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(|text| TranscriptEvent::AssistantMessage {
                            id: format!("{base_id}:{i}"),
                            timestamp: timestamp.clone(),
                            text: text.to_string(),
                        }),
                    Some("thinking") => block
                        .get("thinking")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(|text| TranscriptEvent::Reasoning {
                            id: format!("{base_id}:{i}"),
                            timestamp: timestamp.clone(),
                            text: text.to_string(),
                        }),
                    Some("tool_use") => {
                        let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("Tool");
                        Some(TranscriptEvent::ToolCall {
                            id: block
                                .get("id")
                                .and_then(|x| x.as_str())
                                .unwrap_or(base_id)
                                .to_string(),
                            timestamp: timestamp.clone(),
                            name: name.to_string(),
                            summary: tool_summary(name, block.get("input")),
                            subagent: CLAUDE_SUBAGENTS.detect_call(name, block.get("input")),
                        })
                    }
                    _ => None,
                },
            )
            .collect(),
        "system" if v.get("subtype").and_then(|x| x.as_str()) == Some("compact_boundary") => {
            vec![TranscriptEvent::Metadata {
                id: base_id.to_string(),
                timestamp,
                kind: "compact".into(),
            }]
        }
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

/// 上下文窗口基准（标准 200k）。1M-context 变体无法从 transcript 的 model 字段可靠识别，
/// 故统一按 200k 估算并封顶 100%；后续若需精确可按 model 调整。
const CONTEXT_WINDOW: u64 = 200_000;

// ═══ 解析：JSONL 逐行 fold ═══

/// 把 assistant 正文清洗成卡片预览：合并所有空白为单空格、按**字符**截断到 ~180。
/// 单次遍历完成「折叠空白 + 计数截断」，命中上限即提前返回——大消息不再整条 collapse/分配。
pub(crate) fn preview_text(s: &str) -> Option<String> {
    const MAX: usize = 180;
    let mut out = String::new();
    let mut count = 0usize; // out 中的字符数
    let mut pending_space = false; // 词间是否有待补的单空格（行首/行尾不补）
    for ch in s.chars() {
        if ch.is_whitespace() {
            if count > 0 {
                pending_space = true;
            }
            continue;
        }
        // 写入该非空白字符（连同可能的前导空格）前先判断是否会超限。
        let need = if pending_space { 2 } else { 1 };
        if count + need > MAX {
            out.push('…');
            return Some(out);
        }
        if pending_space {
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        out.push(ch);
        count += 1;
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 把 assistant 正文归类为「卡死错误」短标签；非卡死返回 None。
/// 刻意排除 529/500/ECONNRESET 等临时错误（多数自愈，标红会误报）。
/// 真实卡死错误都是独立短文案；长正文（如讨论/引用错误日志的正常回答）不判错，避免误报。
pub(crate) fn classify_error(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.chars().count() > 200 {
        return None;
    }
    if t.contains("could not be parsed (retry also failed)") {
        return Some("工具调用解析失败");
    }
    if t.starts_with("Please run /login") || t.contains("API Error: 403") {
        return Some("需要重新登录");
    }
    if t.starts_with("Failed to authenticate") || t.contains("API Error: 401") {
        return Some("认证失败");
    }
    None
}

/// 增量解析的累积状态：标题（custom/ai 分开存，custom 优先）、最近一条 assistant 正文、
/// 最近一条 usage。逐行 fold，故对「只追加」的 transcript 可跨多次调用累积，无需重头扫。
#[derive(Default, Clone)]
struct ParseState {
    custom: Option<String>,
    ai: Option<String>,
    last_text: Option<(String, String)>, // (正文, uuid)
    last_usage: Option<u64>,             // 最近一条 assistant 的上下文已用 token
}

impl ParseState {
    /// 折叠一行 JSONL：只关心 title / assistant 行，其它快速跳过（不解析）。
    fn fold_line(&mut self, line: &str) {
        let has_title = line.contains("-title");
        let has_assistant = line.contains("\"assistant\"");
        if !has_title && !has_assistant {
            return;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("custom-title") => {
                if let Some(s) = v.get("customTitle").and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        self.custom = Some(s.to_string());
                    }
                }
            }
            Some("ai-title") => {
                if let Some(s) = v.get("aiTitle").and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        self.ai = Some(s.to_string());
                    }
                }
            }
            Some("assistant") => {
                // 上下文已用量：每条 assistant（含纯 tool_use）都带 usage，取最新一条。
                if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                    let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                    let used = g("input_tokens")
                        + g("cache_creation_input_tokens")
                        + g("cache_read_input_tokens")
                        + g("output_tokens");
                    if used > 0 {
                        self.last_usage = Some(used);
                    }
                }
                // 取该 assistant 消息 content 数组里所有 text 块，空格拼接（对齐 moshi）；无 text 块则 None（如纯 tool_use）。
                let text = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .and_then(|arr| {
                        let joined = arr
                            .iter()
                            .filter(|x| x.get("type").and_then(|t| t.as_str()) == Some("text"))
                            .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if joined.is_empty() {
                            None
                        } else {
                            Some(joined)
                        }
                    });
                if let Some(text) = text {
                    let uuid = v
                        .get("uuid")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.last_text = Some((text, uuid));
                }
            }
            _ => {}
        }
    }

    /// 从累积状态产出 TranscriptInfo。
    fn to_info(&self) -> TranscriptInfo {
        let error = self.last_text.as_ref().and_then(|(text, uuid)| {
            classify_error(text).map(|label| TurnError {
                label: label.to_string(),
                raw: text.clone(),
                fingerprint: uuid.clone(),
            })
        });
        let context_pct = self.last_usage.map(|u| {
            ((u as f64 / CONTEXT_WINDOW as f64) * 100.0)
                .round()
                .min(100.0) as u8
        });
        TranscriptInfo {
            title: self.custom.clone().or_else(|| self.ai.clone()),
            error,
            context_tokens: self.last_usage,
            context_pct,
            preview: self.last_text.as_ref().and_then(|(t, _)| preview_text(t)),
        }
    }
}

/// 单次遍历 transcript（全量）：解析标题（custom-title 优先于 ai-title）、最后一条 assistant
/// 正文（卡死归类）与上下文已用量。读不到/空 → 全 None。热路径请用 [`crate::TranscriptCache`]。
pub fn analyze_transcript(path: &str) -> TranscriptInfo {
    let Ok(content) = std::fs::read_to_string(path) else {
        return TranscriptInfo::default();
    };
    let mut st = ParseState::default();
    for line in content.lines() {
        st.fold_line(line);
    }
    st.to_info()
}

/// ClaudeParser：把私有的 ParseState 包成 TranscriptParser trait 对象（逐字节等价，仅转发）。
struct ClaudeParser(ParseState);

impl TranscriptParser for ClaudeParser {
    fn fold_line(&mut self, line: &str) {
        self.0.fold_line(line);
    }
    fn to_info(&self) -> TranscriptInfo {
        self.0.to_info()
    }
}

// ═══ 路径布局：~/.claude/projects/<encode(cwd)>/<session_id>.jsonl ═══

/// 从 CC transcript JSONL 取会话标题：最后一条 custom-title 优先，否则最后一条 ai-title。
/// 读不到/无标题返回 None。只解析含 "-title" 的行，避免全量 JSON 解析开销。
pub fn title_from_transcript(path: &str) -> Option<String> {
    use std::io::BufRead;
    // 流式逐行读：transcript 可达数 MB，且 reporter 在每个 hook 事件都调用本函数，
    // 整体 read_to_string 会反复把整文件吃进内存——改 BufReader 降峰值内存（扫描复杂度不变）。
    let file = std::fs::File::open(path).ok()?;
    let mut custom: Option<String> = None;
    let mut ai: Option<String> = None;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { continue }; // 单行非 UTF-8 等只跳过，不放弃整文件
        if !line.contains("-title") {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("custom-title") => {
                if let Some(s) = v.get("customTitle").and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        custom = Some(s.to_string());
                    }
                }
            }
            Some("ai-title") => {
                if let Some(s) = v.get("aiTitle").and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        ai = Some(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    custom.or(ai)
}

/// 把 cwd 编码成 Claude Code 在 ~/.claude/projects 下的子目录名：
/// 非 ASCII 字母数字的字符一律换成 `-`（与 CC 的 `[^a-zA-Z0-9] -> '-'` 规则一致，
/// 含下划线、中文、括号等）。
fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// 默认账号的 projects 目录。
fn projects_dir() -> Option<std::path::PathBuf> {
    Some(crate::home_dir()?.join(".claude").join("projects"))
}

/// 全部 `CLAUDE_CONFIG_DIR`：默认账号的 `~/.claude` 在前，meowo 管理的各 profile 目录在后。
/// claude 把 `projects/`（transcript）与 `sessions/`（进程索引，见 [`super::fleet`]）都放在
/// 这一层，两者的「扫哪些账号」是同一个问题，故解析只此一处。
pub(super) fn config_dirs() -> Vec<std::path::PathBuf> {
    let Some(home) = crate::home_dir() else {
        return Vec::new();
    };
    std::iter::once(home.join(".claude"))
        .chain(managed_config_dirs())
        .collect()
}

/// Meowo 管理的 Claude 账号数据目录。Claude 会把 transcript 一并放进
/// `CLAUDE_CONFIG_DIR/projects`，所以跨账号查看/恢复时必须把这些目录也纳入候选。
fn managed_projects_dirs() -> Vec<std::path::PathBuf> {
    managed_config_dirs()
        .into_iter()
        .map(|dir| dir.join("projects"))
        .collect()
}

fn managed_config_dirs() -> Vec<std::path::PathBuf> {
    let Some(home) = crate::home_dir() else {
        return Vec::new();
    };
    let root = std::env::var_os("MEOWO_DB")
        .map(std::path::PathBuf::from)
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| home.join(".meowo"))
        .join("profiles")
        .join("claude");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// 根据 cwd + session_id 重建 transcript 路径：
/// ~/.claude/projects/<encode(cwd)>/<session_id>.jsonl。
///
/// 默认目录与各 managed profile 目录**一视同仁按 mtime 取最新**，不能「默认目录存在
/// 即返回」：跨 profile 恢复会话时宿主把 transcript 复制进目标 CLAUDE_CONFIG_DIR，
/// 原文件留在默认目录成为不再增长的陈旧副本——偏爱默认目录会让对话页从此只看到
/// 副本的截止时刻，会话明明在跑、新消息却永远不出现。
pub fn reconstruct_transcript_path(cwd: &str, session_id: &str) -> Option<std::path::PathBuf> {
    let relative = std::path::PathBuf::from(encode_cwd(cwd)).join(format!("{session_id}.jsonl"));
    let default = projects_dir()?.join(&relative);
    std::iter::once(default.clone())
        .chain(
            managed_projects_dirs()
                .into_iter()
                .map(|projects| projects.join(&relative)),
        )
        .filter(|path| path.exists())
        .max_by_key(|path| transcript_freshness(path))
        .or(Some(default))
}

/// 候选副本的新鲜度：mtime 为主、字节数为副。跨 profile 恢复用 `fs::copy` 同步副本
/// （mtime 原样保留，见 terminal.rs 的 sync 注释），两份副本的 mtime **恒平手**——
/// 纯 mtime 的 `max_by_key` 平手取最后一个候选,等于按目录遍历顺序钉死在任意一份上,
/// 之后哪份先被写入还会在轮询间反复翻转（chat 端表现为 reset 全量重载抖动）。
/// transcript 是追加式日志：平手时字节更多的那份才是含后续消息的活副本。
fn transcript_freshness(path: &std::path::Path) -> (Option<std::time::SystemTime>, u64) {
    match path.metadata() {
        Ok(meta) => (meta.modified().ok(), meta.len()),
        Err(_) => (None, 0),
    }
}

/// 在指定 Claude 数据目录中按 cwd 构造 transcript 路径。宿主在跨账号恢复前用它把会话
/// 精确同步到目标 `CLAUDE_CONFIG_DIR`，而不接触该目录里的凭据与设置。
pub fn transcript_path_in(
    data_dir: &std::path::Path,
    cwd: &str,
    session_id: &str,
) -> std::path::PathBuf {
    data_dir
        .join("projects")
        .join(encode_cwd(cwd))
        .join(format!("{session_id}.jsonl"))
}

/// 不依赖 cwd，直接在 ~/.claude/projects/*/ 下按 `<session_id>.jsonl` 找 transcript。
/// transcript 文件名即 session_id（全局唯一），对 cwd 缺失/编码不一致都免疫。
pub fn find_transcript_by_session(session_id: &str) -> Option<std::path::PathBuf> {
    let file = format!("{session_id}.jsonl");
    let mut projects = vec![projects_dir()?];
    projects.extend(managed_projects_dirs());
    projects
        .into_iter()
        .flat_map(|projects| {
            std::fs::read_dir(projects)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path().join(&file))
                .filter(|candidate| candidate.exists())
                .collect::<Vec<_>>()
        })
        .max_by_key(|path| transcript_freshness(path))
}

/// 从 transcript JSONL 里读出会话工作目录(cwd)：取第一条带非空 "cwd" 字段的记录。
/// cwd 在文件靠前的消息记录里，故逐行读、命中即返回，避免把大文件整体读入。
pub fn cwd_from_transcript(path: &str) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    for line in std::io::BufReader::new(file).lines() {
        // 单行读失败（如非 UTF-8 字节）只跳过该行，不放弃整个文件。
        let Ok(line) = line else { continue };
        if !line.contains("\"cwd\"") {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
            if !c.trim().is_empty() {
                return Some(c.to_string());
            }
        }
    }
    None
}

/// 解析 transcript 文件路径，依次尝试：1) hook 给的 path；2) cwd+session_id 重建；
/// 3) 按 session_id 全局查找。供「同时要标题+错误」的调用方先拿路径再 analyze。
/// 注意：与 resolve_title 不同，本函数只做路径定位，不保证文件内含有标题；
/// 第一个候选文件存在即返回，不会因「文件无标题」继续回落。
fn resolve_path(
    transcript_path: Option<&str>,
    cwd: Option<&str>,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(p) = transcript_path {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    if let Some(cwd) = cwd {
        if let Some(p) = reconstruct_transcript_path(cwd, session_id) {
            if p.exists() {
                return Some(p);
            }
        }
    }
    find_transcript_by_session(session_id)
}

/// 扫 buf 里**完整行**（到最后一个 `\n` 为止）中的 skill_listing，更新 `latest`，
/// 返回处理到的字节数（最后一个 `\n` 之后的位置；无 `\n` 时为 0）。
/// 按字节找行边界再 lossy 解码：lossy 会把非法序列换成 U+FFFD，先解码再算偏移会错位。
fn scan_complete_lines(buf: &[u8], latest: &mut Option<Vec<crate::SlashCommand>>) -> usize {
    let Some(end) = buf.iter().rposition(|&byte| byte == b'\n') else {
        return 0;
    };
    let text = String::from_utf8_lossy(&buf[..end]);
    for line in text.lines() {
        if let Some(commands) = skill_listing_from_line(line) {
            *latest = Some(commands);
        }
    }
    end + 1
}

/// runtime 能力清单通常在 transcript 开头；恢复会话后 Claude 也可能在末尾写一份更新清单。
/// 各读 4 MiB，避免为了几十个 skill 在打开 ChatWindow 时全量扫描数百 MiB 的长会话。
/// 返回（清单，已扫描到的字节偏移）——偏移落在行首，供增量扫描继续。
fn full_skill_scan(path: &std::path::Path) -> (Option<Vec<crate::SlashCommand>>, u64) {
    use std::io::{Read, Seek, SeekFrom};
    const WINDOW: u64 = 4 * 1024 * 1024;
    let mut latest = None;
    let Ok(mut file) = std::fs::File::open(path) else {
        return (None, 0);
    };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return (None, 0);
    };
    let mut head = Vec::new();
    if file.by_ref().take(WINDOW).read_to_end(&mut head).is_err() {
        return (None, 0);
    }
    let scanned = scan_complete_lines(&head, &mut latest);
    if len <= WINDOW {
        return (latest, scanned as u64);
    }
    let start = len - WINDOW;
    let mut tail = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut tail).is_err() {
        return (latest, scanned as u64);
    }
    // 起点可能落在一条 JSON 中间；跳到下一行行首再扫，避免把残片里的换行误当 JSONL 边界。
    let Some(newline) = tail.iter().position(|&byte| byte == b'\n') else {
        return (latest, scanned as u64);
    };
    let offset = newline + 1;
    let scanned = scan_complete_lines(&tail[offset..], &mut latest);
    (latest, start + (offset + scanned) as u64)
}

/// skill 清单的增量扫描缓存。ChatWindow 在 `runtime_commands_pending` 期间随每次 650ms
/// 轮询重新探测；无缓存时每次都重读最多 8 MiB 窗口 + 重扫。缓存后：文件未变只花一次
/// stat；transcript 追加（流式会话的常态）只读并扫新增的字节——JSONL 只追加不改写，
/// 首次全量扫过的区间不会再长出新清单。文件被截断/替换（len 变小或 mtime 倒退）时全量重扫。
struct SkillScan {
    len: u64,
    mtime: Option<std::time::SystemTime>,
    /// 下一次增量扫描的起点（上一条完整行之后的字节偏移）。
    scanned_to: u64,
    latest: Option<Vec<crate::SlashCommand>>,
}

static SKILL_SCANS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, SkillScan>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// 从 `from` 起读追加的字节并扫完整行；返回新的扫描终点。读不动（文件被并发替换等）返回 None。
fn scan_appended(
    path: &std::path::Path,
    from: u64,
    latest: &mut Option<Vec<crate::SlashCommand>>,
) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(from + scan_complete_lines(&buf, latest) as u64)
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

fn skill_listing_from_line(line: &str) -> Option<Vec<crate::SlashCommand>> {
    if !line.contains("\"skill_listing\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let attachment = value.get("attachment")?;
    if attachment.get("type").and_then(|kind| kind.as_str()) != Some("skill_listing") {
        return None;
    }
    let content = attachment
        .get("content")
        .and_then(|content| content.as_str())
        .unwrap_or("");
    let content_names: Vec<&str> = content
        .lines()
        .filter_map(|line| line.strip_prefix("- ")?.split_once(":"))
        .map(|(name, _)| name.trim())
        .collect();
    let names: Vec<&str> = attachment
        .get("names")
        .and_then(|names| names.as_array())
        .map(|names| names.iter().filter_map(|name| name.as_str()).collect())
        .unwrap_or(content_names);
    let commands = names
        .into_iter()
        .filter(|name| valid_skill_name(name))
        .take(256)
        .map(|name| {
            let prefix = format!("- {name}:");
            let description = content
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .map(|description| {
                    let description = description.trim();
                    let mut text: String = description.chars().take(240).collect();
                    if description.chars().count() > 240 {
                        text.push('…');
                    }
                    text
                });
            crate::SlashCommand::runtime(format!("/{name}"), description)
        })
        .collect();
    Some(commands)
}

fn runtime_skill_commands(path: &std::path::Path) -> Option<Vec<crate::SlashCommand>> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return None;
    };
    let len = metadata.len();
    let mtime = metadata.modified().ok();
    let mut scans = SKILL_SCANS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = scans.get_mut(path) {
        if entry.len == len && entry.mtime == mtime {
            return entry.latest.clone();
        }
        // 只增长 → 增量扫描追加部分；变短或同长不同 mtime（原地改写）/读失败 → 当作被替换，落回全量。
        if len > entry.len {
            let mut latest = entry.latest.take();
            if let Some(scanned_to) = scan_appended(path, entry.scanned_to, &mut latest) {
                *entry = SkillScan {
                    len,
                    mtime,
                    scanned_to,
                    latest,
                };
                return entry.latest.clone();
            }
        }
        scans.remove(path);
    }
    let (latest, scanned_to) = full_skill_scan(path);
    // 条目极小（一个路径 + 几十条命令），上限只为杜绝病态增长。
    if scans.len() >= 32 {
        scans.clear();
    }
    scans.insert(
        path.to_path_buf(),
        SkillScan {
            len,
            mtime,
            scanned_to,
            latest: latest.clone(),
        },
    );
    latest
}

// ═══ SubagentSpec 实现 ═══

/// Claude 的子任务侧车布局（实测 Claude Code 2.x）：
///
/// ```text
/// <projects>/<proj>/<session-id>.jsonl          ← 主 transcript
/// <projects>/<proj>/<session-id>/subagents/
///     agent-<agent-id>.jsonl                    ← 子任务全过程（与主流同格式，行带 isSidechain）
///     agent-<agent-id>.meta.json                ← {agentType, description, toolUseId, spawnDepth}
/// ```
///
/// `meta.json` 的 `toolUseId` 就是主链那条 `Agent` 工具调用的 id——现成外键，
/// 不必靠 `parentUuid` 做图归并。
pub struct ClaudeSubagents;
pub static CLAUDE_SUBAGENTS: ClaudeSubagents = ClaudeSubagents;

impl ClaudeSubagents {
    /// `<dir>/<session>.jsonl` → `<dir>/<session>/subagents/`
    fn stream_dir(main_transcript: &Path) -> Option<PathBuf> {
        let dir = main_transcript.with_extension("").join("subagents");
        dir.is_dir().then_some(dir)
    }
}

impl SubagentSpec for ClaudeSubagents {
    fn detect_call(
        &self,
        tool_name: &str,
        input: Option<&serde_json::Value>,
    ) -> Option<SubagentRef> {
        // `Task` 是旧版名，`Agent` 是当前名；两个都认，历史会话才不会退化成裸工具调用。
        if !matches!(tool_name, "Agent" | "Task") {
            return None;
        }
        let input = input?;
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
            // claude 的一次 Agent 调用恒定对应一个子任务；同时派多个是多条 tool_use。
            count: 1,
        })
    }

    fn locate_streams(&self, main_transcript: &Path, tool_use_id: &str) -> Vec<SubagentStream> {
        Self::locate_one(main_transcript, tool_use_id)
            .map(|path| {
                vec![SubagentStream {
                    label: None,
                    // claude 的 meta.json 只记身份不记结局，没有可靠的状态信号。
                    status: None,
                    path,
                }]
            })
            .unwrap_or_default()
    }

    fn parse_stream_line(&self, line: &str) -> Vec<TranscriptEvent> {
        parse_events(line, true)
    }
}

impl ClaudeSubagents {
    fn locate_one(main_transcript: &Path, tool_use_id: &str) -> Option<PathBuf> {
        let dir = Self::stream_dir(main_transcript)?;
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // meta 只有几行；整读无妨。
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if meta.get("toolUseId").and_then(|v| v.as_str()) != Some(tool_use_id) {
                continue;
            }
            // agent-<id>.meta.json → agent-<id>.jsonl
            let stem = path.file_name()?.to_str()?.strip_suffix(".meta.json")?;
            let stream = dir.join(format!("{stem}.jsonl"));
            return stream.is_file().then_some(stream);
        }
        None
    }
}

// ═══ TranscriptSpec 实现 ═══

/// Claude Code 的 transcript 规格。
pub struct ClaudeTranscript;

/// 全局唯一 claude transcript 规格实例，供插件的 transcript 能力槽以 &'static 返回。
pub static CLAUDE_TRANSCRIPT: ClaudeTranscript = ClaudeTranscript;

impl TranscriptSpec for ClaudeTranscript {
    fn new_parser(&self) -> Box<dyn TranscriptParser> {
        Box::new(ClaudeParser(ParseState::default()))
    }

    fn supports_chat(&self) -> bool {
        true
    }

    fn subagents(&self) -> Option<&'static dyn SubagentSpec> {
        Some(&CLAUDE_SUBAGENTS)
    }

    fn parse_transcript_line(&self, line: &str) -> Vec<TranscriptEvent> {
        parse_transcript_events(line)
    }

    fn agent_modes_from_line(&self, line: &str) -> Vec<crate::AgentMode> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return Vec::new();
        };
        value
            .get("permissionMode")
            .and_then(|mode| mode.as_str())
            .filter(|mode| !mode.trim().is_empty())
            .map(|mode| vec![crate::AgentMode::new("permission", mode)])
            .unwrap_or_default()
    }

    fn supports_runtime_slash_commands(&self) -> bool {
        true
    }

    fn runtime_slash_commands(&self, path: &std::path::Path) -> Option<Vec<crate::SlashCommand>> {
        runtime_skill_commands(path)
    }

    fn resolve_transcript_path(
        &self,
        transcript_path: Option<&str>,
        cwd: Option<&str>,
        session_id: &str,
    ) -> Option<std::path::PathBuf> {
        resolve_path(transcript_path, cwd, session_id)
    }

    /// 解析会话标题，依次尝试：
    /// 1) hook 给的 transcript_path；2) cwd+session_id 重建路径；3) 按 session_id 全局查找。
    fn resolve_title(
        &self,
        transcript_path: Option<&str>,
        cwd: Option<&str>,
        session_id: &str,
    ) -> Option<String> {
        if let Some(p) = transcript_path {
            if std::path::Path::new(p).exists() {
                if let Some(t) = title_from_transcript(p) {
                    return Some(t);
                }
            }
        }
        if let Some(cwd) = cwd {
            if let Some(p) = reconstruct_transcript_path(cwd, session_id) {
                if let Some(t) = p.to_str().and_then(title_from_transcript) {
                    return Some(t);
                }
            }
        }
        // 兜底：cwd 缺失（旧会话）或编码不一致时，按 session_id 直接找文件。
        let p = find_transcript_by_session(session_id)?;
        title_from_transcript(p.to_str()?)
    }

    /// 已知 cwd（DB 记录）不再盲信：先校验其对应目录下确有该会话的 transcript。DB 的 cwd 可能
    /// 失真——会话早于 hook 接线、SessionStart 丢失、项目目录事后被移动/重命名——盲信会让
    /// `claude --resume` 在错误目录下启动、报「No conversation found」，且只能靠用户在 Claude Code
    /// 里手动 resume 一次（SessionStart hook 重写 cwd）才自愈。校验不过则按 session_id 全局反查
    /// transcript、从其内容读出权威 cwd；全局也找不到（transcript 已被 Claude Code 按
    /// cleanupPeriodDays 清理）时回退 DB cwd。
    fn resolve_cwd(&self, cwd: Option<&str>, session_id: &str) -> Option<String> {
        let known = crate::transcript::default_resolve_cwd(cwd);
        if let Some(c) = &known {
            if reconstruct_transcript_path(c, session_id).is_some_and(|p| p.exists()) {
                return known;
            }
        }
        find_transcript_by_session(session_id)
            .and_then(|p| cwd_from_transcript(p.to_str()?))
            .or(known)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_permission_mode_from_user_and_mode_records() {
        assert_eq!(
            CLAUDE_TRANSCRIPT.agent_modes_from_line(
                r#"{"type":"user","permissionMode":"acceptEdits","message":{"content":"hi"}}"#,
            ),
            vec![crate::AgentMode::new("permission", "acceptEdits")]
        );
        assert_eq!(
            CLAUDE_TRANSCRIPT
                .agent_modes_from_line(r#"{"type":"permission-mode","permissionMode":"plan"}"#,),
            vec![crate::AgentMode::new("permission", "plan")]
        );
        assert_eq!(
            CLAUDE_TRANSCRIPT
                .agent_modes_from_line(r#"{"type":"user","message":{"permissionMode":"nested"}}"#,),
            Vec::<crate::AgentMode>::new(),
            "只采信 Claude transcript 的顶层权威字段"
        );
    }

    #[test]
    fn chat_parser_extracts_text_tools_and_skips_sidechain() {
        let assistant = r#"{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"thinking","thinking":"先检查项目"},{"type":"text","text":"我来处理"},{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let items = parse_chat_items(assistant);
        assert!(matches!(&items[0], ChatItem::Reasoning { text, .. } if text == "先检查项目"));
        assert!(matches!(&items[1], ChatItem::AssistantText { text, .. } if text == "我来处理"));
        assert!(
            matches!(&items[2], ChatItem::ToolUse { name, summary, .. } if name == "Bash" && summary == "cargo test")
        );

        let result = r#"{"type":"user","uuid":"u1","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok","is_error":false}]}}"#;
        assert!(
            matches!(&parse_chat_items(result)[0], ChatItem::ToolResult { tool_use_id: Some(id), text, is_error: false, .. } if id == "tool-1" && text == "ok")
        );

        let sidechain = r#"{"type":"assistant","uuid":"sub","isSidechain":true,"message":{"content":[{"type":"text","text":"hidden"}]}}"#;
        assert!(parse_chat_items(sidechain).is_empty());
    }

    #[test]
    fn chat_delta_waits_for_complete_line_and_resumes_from_offset() {
        use std::io::Write;
        let path = write_tmp(
            "chat-delta",
            r#"{"type":"user","uuid":"u1","message":{"content":"hello"}}"#,
        );
        let first = crate::read_chat_delta(&CLAUDE_TRANSCRIPT, &path, 0, None);
        assert!(first.items.is_empty());
        assert_eq!(first.offset, 0);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file).unwrap();
        writeln!(file, r#"{{"type":"assistant","uuid":"a1","message":{{"content":[{{"type":"text","text":"world"}}]}}}}"#).unwrap();
        let second = crate::read_chat_delta(&CLAUDE_TRANSCRIPT, &path, first.offset, first.mtime);
        assert_eq!(second.items.len(), 2);
        let third = crate::read_chat_delta(&CLAUDE_TRANSCRIPT, &path, second.offset, second.mtime);
        assert!(third.items.is_empty());
        assert_eq!(third.offset, second.offset);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn classify_matches_stuck_errors() {
        assert_eq!(
            classify_error("The model's tool call could not be parsed (retry also failed)."),
            Some("工具调用解析失败")
        );
        assert_eq!(
            classify_error("Please run /login · API Error: 403 Request not allowed"),
            Some("需要重新登录")
        );
        assert_eq!(
            classify_error("API Error: 403 Request not allowed"),
            Some("需要重新登录")
        );
        assert_eq!(
            classify_error(
                "Failed to authenticate. API Error: 401 Invalid authentication credentials"
            ),
            Some("认证失败")
        );
        assert_eq!(
            classify_error("API Error: 401 Invalid authentication credentials"),
            Some("认证失败")
        );
    }

    #[test]
    fn classify_ignores_transient_and_normal() {
        assert_eq!(
            classify_error("API Error: 529 Overloaded. This is a server-side issue"),
            None
        );
        assert_eq!(classify_error("API Error: 500 status code (no body)"), None);
        assert_eq!(
            classify_error("Unable to connect to API (ECONNRESET)"),
            None
        );
        assert_eq!(classify_error("这是一段正常的助手回答。"), None);
    }

    #[test]
    fn classify_ignores_long_text_quoting_error() {
        // 正常长回答里引用错误文案（如调试 API 的会话）不应被判为卡死。
        let long = format!(
            "{}先看日志里的 API Error: 403 Request not allowed，这是因为……",
            "分析：".repeat(100)
        );
        assert_eq!(classify_error(&long), None);
    }

    fn write_tmp(name: &str, content: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("cc_analyze_{}_{}.jsonl", std::process::id(), name));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn analyze_detects_parse_abort_and_title() {
        let content = concat!(
            r#"{"type":"ai-title","aiTitle":"做某功能"}"#,
            "\n",
            r#"{"type":"assistant","uuid":"u-err-1","message":{"role":"assistant","content":[{"type":"thinking","thinking":""},{"type":"text","text":"The model's tool call could not be parsed (retry also failed)."}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":1000}"#,
            "\n",
        );
        let p = write_tmp("parse", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.title.as_deref(), Some("做某功能"));
        let e = info.error.expect("应检测到错误");
        assert_eq!(e.label, "工具调用解析失败");
        assert_eq!(e.fingerprint, "u-err-1");
    }

    #[test]
    fn analyze_no_error_on_normal_ending() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"已完成，结果如下。"}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":500}"#,
            "\n",
        );
        let p = write_tmp("normal", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.error, None);
    }

    #[test]
    fn analyze_recovered_after_error_not_flagged() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u-err","message":{"role":"assistant","content":[{"type":"text","text":"The model's tool call could not be parsed (retry also failed)."}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":100}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"继续"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"u-ok","message":{"role":"assistant","content":[{"type":"text","text":"好的，已经修好了。"}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":200}"#,
            "\n",
        );
        let p = write_tmp("recover", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.error, None);
    }

    #[test]
    fn analyze_skips_tooluse_only_assistant() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u-err","message":{"role":"assistant","content":[{"type":"text","text":"Please run /login · API Error: 403 Request not allowed"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"u-tool","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            "\n",
        );
        let p = write_tmp("toolonly", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(
            info.error.map(|e| e.label),
            Some("需要重新登录".to_string())
        );
    }

    #[test]
    fn preview_text_collapses_and_truncates() {
        assert_eq!(
            preview_text("  hi\n\n  there  "),
            Some("hi there".to_string())
        );
        assert_eq!(preview_text("   \n\t  "), None);
        let long: String = "あ".repeat(200);
        let p = preview_text(&long).unwrap();
        // 按字符截断到 180 + 省略号；多字节字符不会被截半。
        assert_eq!(p.chars().count(), 181);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn analyze_concatenates_multiple_text_blocks_in_one_assistant() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"先说开场白"},{"type":"tool_use","id":"t","name":"Bash","input":{}},{"type":"text","text":"再说结论"}]}}"#,
            "\n",
        );
        let p = write_tmp("concat", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.preview.as_deref(), Some("先说开场白 再说结论"));
    }

    #[test]
    fn analyze_exposes_last_assistant_preview() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"first turn"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"u2","message":{"role":"assistant","content":[{"type":"text","text":"  need your\n  confirmation  "}]}}"#,
            "\n",
        );
        let p = write_tmp("preview", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.preview.as_deref(), Some("need your confirmation"));
    }

    #[test]
    fn analyze_missing_file_is_empty() {
        let info = analyze_transcript("C:/no/such/file-xyz.jsonl");
        assert_eq!(info, TranscriptInfo::default());
    }

    #[test]
    fn analyze_extracts_latest_context_usage() {
        // 两条 assistant：取最新一条的 usage。50000+50000+0+10000 = 110000 → 55%。
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","usage":{"input_tokens":10,"cache_creation_input_tokens":1000,"cache_read_input_tokens":2000,"output_tokens":500},"content":[{"type":"text","text":"早些的回合"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"u2","message":{"role":"assistant","usage":{"input_tokens":50000,"cache_creation_input_tokens":50000,"cache_read_input_tokens":0,"output_tokens":10000},"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            "\n",
        );
        let p = write_tmp("usage", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.context_tokens, Some(110_000));
        assert_eq!(info.context_pct, Some(55));
    }

    #[test]
    fn analyze_context_pct_caps_at_100() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","usage":{"input_tokens":300000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0},"content":[{"type":"text","text":"超长上下文"}]}}"#,
            "\n",
        );
        let p = write_tmp("usage_cap", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.context_pct, Some(100));
    }

    #[test]
    fn analyze_no_usage_is_none() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"没有 usage 字段"}]}}"#,
            "\n",
        );
        let p = write_tmp("usage_none", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.context_tokens, None);
        assert_eq!(info.context_pct, None);
    }

    /// 增量解析器逐行 fold 的结果须与 analyze_transcript 全量解析逐字段一致。
    #[test]
    fn claude_parser_matches_full_scan() {
        let content = concat!(
            r#"{"type":"ai-title","aiTitle":"标题X"}"#,
            "\n",
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","usage":{"input_tokens":40000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0},"content":[{"type":"text","text":"hi there"}]}}"#,
            "\n",
        );
        let mut parser = CLAUDE_TRANSCRIPT.new_parser();
        for line in content.lines() {
            parser.fold_line(line);
        }
        let p = write_tmp("parser_full", content);
        let full = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(parser.to_info(), full);
        assert_eq!(parser.to_info().title.as_deref(), Some("标题X"));
        assert_eq!(parser.to_info().context_tokens, Some(40000));
    }

    #[test]
    fn resolve_title_reads_custom_title() {
        let p = write_tmp(
            "resolve_title",
            "{\"type\":\"custom-title\",\"customTitle\":\"我的标题\"}\n",
        );
        let path = p.to_str().unwrap();
        let via_spec = CLAUDE_TRANSCRIPT.resolve_title(Some(path), None, "sid");
        let via_fn = title_from_transcript(path);
        std::fs::remove_file(&p).ok();
        assert_eq!(via_spec, via_fn);
        assert_eq!(via_spec.as_deref(), Some("我的标题"));
    }

    #[test]
    fn encode_cwd_windows_path() {
        assert_eq!(encode_cwd(r"C:\Users\me\proj"), "C--Users-me-proj");
    }

    #[test]
    fn encode_cwd_unix_path() {
        assert_eq!(encode_cwd("/tmp/x y"), "-tmp-x-y");
    }

    #[test]
    fn encode_cwd_replaces_all_non_alphanumeric() {
        // CC 规则是 [^a-zA-Z0-9] 全替换：下划线、中文、括号都变 '-'。
        assert_eq!(encode_cwd(r"C:\a_b\my(中文)"), "C--a-b-my----");
    }

    #[test]
    fn cwd_from_transcript_skips_metadata_takes_message() {
        // 模拟真实 transcript：开头元数据无 cwd，消息记录才带 cwd。
        let content = concat!(
            "{\"type\":\"leafUuid\",\"sessionId\":\"s\"}\n",
            "{\"type\":\"permissionMode\",\"sessionId\":\"s\"}\n",
            "{\"type\":\"user\",\"cwd\":\"C:\\\\Users\\\\me\\\\proj\",\"sessionId\":\"s\"}\n",
        );
        let path = write_tmp("cwd_test", content);
        let got = cwd_from_transcript(path.to_str().unwrap());
        std::fs::remove_file(&path).ok();
        assert_eq!(got.as_deref(), Some(r"C:\Users\me\proj"));
    }

    #[test]
    fn resolve_cwd_prefers_known() {
        // 已知 cwd 校验不过（其下无 transcript）且全局也找不到 → 回退已知 cwd（已清理场景）。
        assert_eq!(
            CLAUDE_TRANSCRIPT
                .resolve_cwd(Some(r"C:\a\b"), "anyid")
                .as_deref(),
            Some(r"C:\a\b")
        );
        assert_eq!(
            CLAUDE_TRANSCRIPT.resolve_cwd(Some("  "), "no-such-session-id-xxx"),
            None
        );
    }

    #[test]
    fn resolve_cwd_corrects_stale_db_cwd_via_global_search() {
        // DB 记录的 cwd 已失真（其对应目录下没有该会话的 transcript）时，应按 session_id 全局反查
        // 并从 transcript 内容读出权威 cwd——否则 resume 会在错误目录下启动、报 No conversation found，
        // 用户只能去 Claude Code 手动 resume 一次（hook 重写 cwd）才能自愈。
        let sid = format!("resolve-cwd-stale-{}", std::process::id());
        let home = std::env::temp_dir().join(format!("cc_home_{}", std::process::id()));
        // encode_cwd(r"C:\real\proj") == "C--real-proj"
        let proj = home.join(".claude").join("projects").join("C--real-proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join(format!("{sid}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"cwd\":\"C:\\\\real\\\\proj\",\"sessionId\":\"{sid}\"}}\n"
            ),
        )
        .unwrap();
        // 改的是**进程全局**的 USERPROFILE：持锁期间不许别的测试去解析安装路径，
        // 否则它们会在这个窗口里解析不到 claude 而随机变红。见 `crate::env_guard`。
        let _env = crate::env_guard();
        let old_home = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);
        let corrected = CLAUDE_TRANSCRIPT.resolve_cwd(Some(r"C:\stale\gone"), &sid);
        let verified_ok = CLAUDE_TRANSCRIPT.resolve_cwd(Some(r"C:\real\proj"), &sid); // 校验通过 → 原样返回，不做全局扫描
        match old_home {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(corrected.as_deref(), Some(r"C:\real\proj"));
        assert_eq!(verified_ok.as_deref(), Some(r"C:\real\proj"));
    }

    #[test]
    fn transcript_lookup_includes_managed_claude_profiles() {
        let sid = format!("profile-shared-{}", std::process::id());
        let home = std::env::temp_dir().join(format!("cc_profile_home_{}", std::process::id()));
        let transcript = home
            .join(".meowo/profiles/claude/work/projects/C--shared-project")
            .join(format!("{sid}.jsonl"));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            format!(r#"{{"type":"user","sessionId":"{sid}","cwd":"C:\\shared\\project"}}"#),
        )
        .unwrap();

        let _env = crate::env_guard();
        let old_home = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);
        assert_eq!(
            find_transcript_by_session(&sid).as_deref(),
            Some(transcript.as_path())
        );
        assert_eq!(
            reconstruct_transcript_path(r"C:\shared\project", &sid).as_deref(),
            Some(transcript.as_path())
        );
        match old_home {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
        let _ = std::fs::remove_dir_all(home);
    }

    /// 跨 profile 恢复会话后,默认 ~/.claude 里留下的是不再增长的陈旧副本,续写发生在
    /// managed profile 的同名文件里。路径重建必须按 mtime 挑最新,而不是默认目录存在
    /// 即返回——否则对话页从此只看到副本的截止时刻(真实案例:0.5.8 新消息不上屏)。
    #[test]
    fn transcript_reconstruction_prefers_the_freshest_copy_over_the_default_dir() {
        let sid = format!("stale-copy-{}", std::process::id());
        let home = std::env::temp_dir().join(format!("cc_stale_copy_home_{}", std::process::id()));
        let relative = std::path::PathBuf::from("C--shared-project").join(format!("{sid}.jsonl"));
        let stale = home.join(".claude/projects").join(&relative);
        let fresh = home
            .join(".meowo/profiles/claude/work/projects")
            .join(&relative);
        for path in [&stale, &fresh] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "{}\n").unwrap();
        }
        // 默认目录副本停在一小时前;managed profile 里的延续文件刚刚还在写。
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let _env = crate::env_guard();
        let old_home = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);
        assert_eq!(
            reconstruct_transcript_path(r"C:\shared\project", &sid).as_deref(),
            Some(fresh.as_path())
        );
        // 反向:默认目录才是最新时仍选默认——不是无脑偏爱 managed 目录。
        std::fs::File::options()
            .write(true)
            .open(&fresh)
            .unwrap()
            .set_modified(old - std::time::Duration::from_secs(3600))
            .unwrap();
        assert_eq!(
            reconstruct_transcript_path(r"C:\shared\project", &sid).as_deref(),
            Some(stale.as_path())
        );
        match old_home {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
        let _ = std::fs::remove_dir_all(home);
    }

    /// mtime 平手时按字节数取超集。跨 profile 恢复用 fs::copy 同步副本,mtime 原样保留,
    /// 两份 mtime **恒相同**——纯 mtime 的 max_by_key 平手取最后候选,等于按遍历顺序
    /// 钉死在任意一份上;续写副本随后长大,凭大小才能稳定选中它,且不随目录顺序翻转。
    #[test]
    fn transcript_reconstruction_breaks_mtime_ties_by_size() {
        let sid = format!("tie-copy-{}", std::process::id());
        let home = std::env::temp_dir().join(format!("cc_tie_copy_home_{}", std::process::id()));
        let relative = std::path::PathBuf::from("C--shared-project").join(format!("{sid}.jsonl"));
        let in_default = home.join(".claude/projects").join(&relative);
        let in_managed = home
            .join(".meowo/profiles/claude/work/projects")
            .join(&relative);
        let same = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let write = |path: &std::path::Path, content: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(same)
                .unwrap();
        };
        // 正向:managed 侧是长出来的续写。
        write(&in_default, "{}\n");
        write(&in_managed, "{}\n{}\n");

        let _env = crate::env_guard();
        let old_home = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", &home);
        assert_eq!(
            reconstruct_transcript_path(r"C:\shared\project", &sid).as_deref(),
            Some(in_managed.as_path())
        );
        // 反向:默认侧更大时选默认——证明比的是大小,不是目录遍历顺序。
        write(&in_default, "{}\n{}\n{}\n");
        assert_eq!(
            reconstruct_transcript_path(r"C:\shared\project", &sid).as_deref(),
            Some(in_default.as_path())
        );
        match old_home {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn discovers_runtime_slash_commands_from_claudes_authoritative_skill_listing() {
        let path = std::env::temp_dir().join(format!(
            "claude-runtime-skills-{}.jsonl",
            std::process::id()
        ));
        let line = serde_json::json!({
            "type": "attachment",
            "attachment": {
                "type": "skill_listing",
                "content": "- code-review: Review the current diff\n- frontend-design:frontend-design: Design guidance\n- stale-skill: Must not be shown",
                // names 是 Claude 对**本会话实际启用项**的权威清单；content 可能含兼容说明，
                // 不能反过来把未启用项猜成命令。
                "names": ["code-review", "frontend-design:frontend-design"]
            }
        });
        std::fs::write(&path, format!("{}\n", line)).unwrap();
        let commands = CLAUDE_TRANSCRIPT.runtime_slash_commands(&path).unwrap();
        let _ = std::fs::remove_file(path);

        assert_eq!(
            commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            vec!["/code-review", "/frontend-design:frontend-design"]
        );
        assert_eq!(
            commands[0].description.as_deref(),
            Some("Review the current diff")
        );
        assert_eq!(commands[1].description.as_deref(), Some("Design guidance"));
        assert!(commands
            .iter()
            .all(|command| command.source == crate::SlashSource::Builtin));
    }

    /// 缓存契约：追加走增量扫描（残行不入账），截断/重写落回全量重扫。
    #[test]
    fn runtime_skill_scan_picks_up_appended_listings_incrementally() {
        let path = std::env::temp_dir().join(format!(
            "claude-runtime-skills-append-{}.jsonl",
            std::process::id()
        ));
        let listing = |names: &[&str]| {
            serde_json::json!({
                "type": "attachment",
                "attachment": { "type": "skill_listing", "content": "", "names": names }
            })
        };
        let names_of = |commands: Option<Vec<crate::SlashCommand>>| {
            commands
                .unwrap()
                .iter()
                .map(|command| command.name.clone())
                .collect::<Vec<_>>()
        };
        std::fs::write(&path, format!("{}\n", listing(&["first"]))).unwrap();
        assert_eq!(
            names_of(CLAUDE_TRANSCRIPT.runtime_slash_commands(&path)),
            vec!["/first"]
        );

        // 追加一份更新清单 + 一条未写完的残行；增量扫描应取到新清单、跳过残行。
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(file, "{}\n{{\"partial", listing(&["second"])).unwrap();
        drop(file);
        assert_eq!(
            names_of(CLAUDE_TRANSCRIPT.runtime_slash_commands(&path)),
            vec!["/second"]
        );

        // 文件被截断重写（长度变短）→ 全量重扫，不沿用旧偏移。
        std::fs::write(&path, format!("{}\n", listing(&["third"]))).unwrap();
        assert_eq!(
            names_of(CLAUDE_TRANSCRIPT.runtime_slash_commands(&path)),
            vec!["/third"]
        );
        let _ = std::fs::remove_file(path);
    }

    /// 子任务委派：摘要取描述而非整包 input（prompt 上千字会把摘要行淹掉），
    /// 并带上让前端渲染成可展开条目的 SubagentRef。旧版工具名 `Task` 同样识别。
    #[test]
    fn agent_tool_call_carries_subagent_ref_and_readable_summary() {
        let line = r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Agent","input":{"description":"验证审批双轨","subagent_type":"general-purpose","prompt":"You are a code-review VERIFIER. 很长很长的正文……"}}]}}"#;
        let items = parse_chat_items(line);
        let ChatItem::ToolUse {
            summary, subagent, ..
        } = &items[0]
        else {
            panic!("应解析成工具调用，实际：{items:?}");
        };
        assert_eq!(summary, "验证审批双轨", "摘要应是描述，不是整包 input JSON");
        let subagent = subagent.as_ref().expect("Agent 调用应带 SubagentRef");
        assert_eq!(subagent.description, "验证审批双轨");
        assert_eq!(subagent.agent_type.as_deref(), Some("general-purpose"));

        // 旧版名 Task 同样认，否则历史会话里的子任务会退化成裸工具调用。
        let legacy = line.replace("\"name\":\"Agent\"", "\"name\":\"Task\"");
        let ChatItem::ToolUse { subagent, .. } = &parse_chat_items(&legacy)[0] else {
            panic!("Task 也应解析成工具调用");
        };
        assert!(subagent.is_some());

        // 普通工具不该被误判成子任务。
        let bash = r#"{"type":"assistant","uuid":"a2","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let ChatItem::ToolUse { subagent, .. } = &parse_chat_items(bash)[0] else {
            panic!("应解析成工具调用");
        };
        assert!(subagent.is_none());
    }

    /// 侧车流按 meta.json 的 toolUseId 外键定位；流内每行都是 sidechain，
    /// 必须**不**被主流那道 sidechain 守卫丢掉，否则展开永远是空的。
    #[test]
    fn locates_subagent_stream_by_tool_use_id_and_parses_sidechain_lines() {
        let root = std::env::temp_dir().join(format!("claude-subagent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("projects/proj");
        let main = project.join("session-1.jsonl");
        let subagents = project.join("session-1/subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        std::fs::write(&main, "").unwrap();
        std::fs::write(
            subagents.join("agent-abc.meta.json"),
            r#"{"agentType":"general-purpose","description":"查一下","toolUseId":"toolu_target","spawnDepth":1}"#,
        )
        .unwrap();
        std::fs::write(
            subagents.join("agent-abc.jsonl"),
            format!(
                "{}\n{}\n",
                r#"{"type":"assistant","uuid":"s1","isSidechain":true,"agentId":"abc","message":{"content":[{"type":"text","text":"子任务结论"}]}}"#,
                r#"{"type":"assistant","uuid":"s2","isSidechain":true,"agentId":"abc","message":{"content":[{"type":"tool_use","id":"st1","name":"Bash","input":{"command":"rg foo"}}]}}"#
            ),
        )
        .unwrap();
        // 另一个子任务，确保是按 toolUseId 精确匹配而不是撞见第一个就返回。
        std::fs::write(
            subagents.join("agent-other.meta.json"),
            r#"{"toolUseId":"toolu_other"}"#,
        )
        .unwrap();
        std::fs::write(subagents.join("agent-other.jsonl"), "").unwrap();

        let runs = crate::transcript::read_subagent_chat(&CLAUDE_TRANSCRIPT, &main, "toolu_target");
        assert_eq!(runs.len(), 1, "claude 一次调用恒对应一个子任务");
        let items = &runs[0].items;
        assert!(
            matches!(&items[0], ChatItem::AssistantText { text, .. } if text == "子任务结论"),
            "sidechain 行必须被解析，实际：{items:?}"
        );
        assert!(matches!(&items[1], ChatItem::ToolUse { name, .. } if name == "Bash"));

        // 对不上的 toolUseId 不该硬凑一个流出来。
        assert!(
            crate::transcript::read_subagent_chat(&CLAUDE_TRANSCRIPT, &main, "toolu_missing")
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 非 claude agent 走默认实现：直接采信 DB cwd，不去翻 ~/.claude/projects。
    #[test]
    fn default_resolve_cwd_trusts_db_value() {
        use crate::transcript::default_resolve_cwd;
        assert_eq!(default_resolve_cwd(Some("/x/y")).as_deref(), Some("/x/y"));
        assert_eq!(default_resolve_cwd(Some("   ")), None);
        assert_eq!(default_resolve_cwd(None), None);
    }
}
