//! claude 的 transcript：`~/.claude/projects/<encode(cwd)>/<session_id>.jsonl` 的路径布局，
//! 与该 JSONL 的增量解析（标题 / 卡死错误 / 上下文占用 / 正文预览）。
//!
//! 这些代码此前住在 `meowo-store`（`analyze.rs` + `title.rs` + `transcript_spec.rs`），于是
//! 「读一个 JSONL 文件」平白拖着 rusqlite 依赖，claude 专属的路径布局也伪装成了通用的 store API。
//! 通用部分（`TranscriptInfo` / trait / `TranscriptCache`）见 `crate::transcript`。

#[cfg(test)]
use crate::transcript::ChatItem;
use crate::transcript::{
    SubagentOutcome, SubagentRef, SubagentSpec, SubagentStream, TranscriptEvent, TranscriptInfo,
    TranscriptParser, TranscriptSpec, TurnError,
};
use std::path::{Path, PathBuf};

/// 从 `<tag>值</tag>` 里抠值。task-notification 的格式是 CLI 固定生成的单层标签,
/// 不需要真 XML 解析;取不到返回 None。
fn tag_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{tag}>"))? + start;
    Some(text[start..end].trim())
}

/// `<task-notification>` user 消息 → 该 tool_use 的合成回执。形态(真实 transcript 取证):
/// `<task-notification><task-id>…<tool-use-id>toolu_x</tool-use-id><output-file>…
/// <status>completed</status><summary>…</summary></task-notification>`。
///
/// **`tool-use-id` 不是恒有的**:forked skill(`/code-review` 等)的通知只带 `<task-id>`
/// (真机取证 2026-08-26:`<task-id>acb7f0cbde2eb991a</task-id>` + `<status>failed</status>`,
/// 整条通知没有 tool-use-id)。此前这里 `?` 直接短路,那条通知被当成普通用户消息丢掉,
/// 于是后台 skill 的委派永远停在「运行中」——实拍:一条 11:36 就失败结束的审查,在会话
/// 剩下的三个半小时里一直挂着运行中。
///
/// 缺 tool-use-id 时改用 `task-id` 当回执的挂载点,并把它填进 `task_id`——前端
/// (`collectSubagentReceipts`)按 task_id 归属:启动回执早已用同一个 id 登记过属主
/// (见 [`parse_events`] 给启动回执补 agentId 那段),这条结局便落回原委派。这与
/// `TaskOutput` 拉取结局走的是同一条既有路由,不新增机制。
///
/// 两者都缺则无从关联,返回 None,调用方按普通 user 文本处理。
fn task_notification_result(
    text: &str,
    base_id: &str,
    timestamp: Option<String>,
) -> Option<TranscriptEvent> {
    if !text.trim_start().starts_with("<task-notification>") {
        return None;
    }
    let tool_use_id = tag_text(text, "tool-use-id")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let task_id = tag_text(text, "task-id")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // 有 tool-use-id 就直连原委派(无需路由);否则靠 task-id 归属。
    let anchor = tool_use_id.clone().or_else(|| task_id.clone())?;
    // status 缺省按 completed:通知本身就意味着「结束了」,分不清成败时宁可少报失败。
    let failed = tag_text(text, "status").is_some_and(|s| s.eq_ignore_ascii_case("failed"));
    let summary = tag_text(text, "summary").unwrap_or("task completed").to_string();
    Some(TranscriptEvent::ToolResult {
        id: base_id.to_string(),
        timestamp,
        tool_call_id: Some(anchor),
        text: summary,
        is_error: failed,
        subagent: Some(SubagentOutcome {
            running: 0,
            completed: if failed { 0 } else { 1 },
            failed: if failed { 1 } else { 0 },
            // 自带 tool-use-id 时直接落到原委派上,不必再路由;只有 task-id 的
            // (forked skill)要靠它归属回启动回执登记的属主。
            task_id: tool_use_id.is_none().then_some(task_id).flatten(),
        }),
    })
}

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
    // 任务列表工具的摘要是前端的**数据源**,不只是展示:CC 对 TaskCreate/TaskUpdate 这类
    // harness 内部工具不触发 PostToolUse hook(实测,reporter 端手喂事件能落库、真调用
    // 不触发),transcript 是任务列表的唯一来源,前端从 items 里累积重建。
    // TaskUpdate 只留三个关键字段:整包 input 里 metadata/addBlocks 等可能把 JSON 顶过
    // 截断上限,截一刀前端就 parse 不回来了。
    if name == "TaskUpdate" {
        if let Some(input) = input {
            let slim = serde_json::json!({
                "taskId": input.get("taskId"),
                "status": input.get("status"),
                "subject": input.get("subject"),
            });
            return compact_json(Some(&slim), 800);
        }
    }
    // Skill 调用摘要 = 用户敲的那行命令(`/code-review 1692 高强度`)。整包 JSON 兜底
    // (`{"skill":"code-review","args":"…"}`)对用户没有意义;而 forked skill 的委派本体
    // **没有** subagent 信息(见 [`forked_skill_outcome`]),前端的子任务行就靠这个摘要
    // 当标题。刻意与侧车 meta 的 `description` 同形——[`locate_forked`] 拿它做外键。
    if name == "Skill" {
        if let Some(skill) = input.and_then(|v| v.get("skill")).and_then(|v| v.as_str()) {
            return skill_invocation_desc(skill, input.and_then(|v| v.get("args")));
        }
    }
    let key = match name {
        "Bash" => "command",
        "WebSearch" => "query",
        "Read" | "Write" | "Edit" => "file_path",
        // 子任务委派：摘要取那句任务描述。缺了这条会落到下面的整包 JSON 兜底，
        // 而 prompt 动辄上千字——摘要行会变成一段被截断的 prompt 泥巴。
        "Agent" | "Task" => "description",
        // 任务标题;description 同样动辄几百字,兜底 JSON 会被截坏(见上)。
        "TaskCreate" => "subject",
        _ => "",
    };
    if !key.is_empty() {
        if let Some(s) = input.and_then(|v| v.get(key)).and_then(|v| v.as_str()) {
            return compact_json(Some(&serde_json::Value::String(s.to_string())), 800);
        }
    }
    compact_json(input, 800)
}

/// Skill 调用的展示形态:`/<skill> <args>`(args 为空则只有 `/<skill>`)。
///
/// 这个串同时是 forked skill 的**外键**:侧车 meta 的 `description` 恰好是同一形态
/// (实测 `{"description":"/code-review 1692 高强度","name":"code-review","spawnDepth":1}`),
/// fork 出的顶层 agent 又偏偏不带 `toolUseId`——没有别的东西能把它连回主链那条 Skill 调用。
fn skill_invocation_desc(skill: &str, args: Option<&serde_json::Value>) -> String {
    match args.and_then(|v| v.as_str()).map(str::trim) {
        Some(args) if !args.is_empty() => format!("/{skill} {args}"),
        _ => format!("/{skill}"),
    }
}

// 排队插话里内嵌图片的落盘上限与粘贴附件共用同一份（fsutil 单点定义）——
// CC 对单图另有更紧的限制，这里只挡异常脏数据把临时目录写爆。
use crate::fsutil::PASTE_MAX_BYTES;

/// 把 queued_command 里的 base64 图片块落成本地文件，返回可写进 `[Image: source: …]`
/// 引用的绝对路径。落盘走 fsutil 的共享原语（`$TEMP/meowo-paste/queued/`，在 asset 协议
/// scope 内，前端才能渲染缩略图；与粘贴附件同归 OS 临时清理策略；按行 uuid + 块序号
/// 命名幂等）。任何失败都返回 None：插话正文照常显示，只是这张图退化为不显示。
fn persist_queued_image(base_id: &str, index: usize, ext: &str, data: &str) -> Option<PathBuf> {
    // uuid 之外的 id 形态（回退的 "message" 等）也进得来，过滤成文件名安全字符集。
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
    crate::fsutil::persist_paste_bytes(&format!("{safe_id}-{index}"), ext, &bytes)
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
                // 后台任务(Agent 委派/后台 Bash)的完成通知:CLI 以 user 消息注入
                // `<task-notification>…<tool-use-id>…<status>…`。它不是用户说的话,
                // 且携带着后台委派的**真实结局**——主链上唯一的完成信号(启动回执只说
                // launched,侧车 meta 不记结局)。转译成该 tool_use 的合成回执,
                // 折叠徽标/进度面板的「进行中」由此翻转成完成/失败。
                if let Some(event) = task_notification_result(text, base_id, timestamp.clone()) {
                    return vec![event];
                }
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
                            // 后台委派的启动回执(`Async agent launched…`)标 running——
                            // 「已回执」在这里只意味着「派出去了」,真结局由 task-notification
                            // 的合成回执(见 user 分支)后到覆盖。同步委派的回执没有结局
                            // 信号,如实留空,前端按「已回执=完成」处理。
                            let mut subagent = CLAUDE_SUBAGENTS.detect_result(&text);
                            // 启动回执补任务 id:forked skill 的回执正文里**没有**任何 id
                            // (`Skill "x" launched (forked execution…)`),而它的完成通知只带
                            // `<task-id>`——两端要靠 CC 记在**行上**的 `toolUseResult.agentId`
                            // 接起来(真机取证:`"status":"forked","agentId":"acb7f0…"`)。
                            // 不补的话这条委派永远等不到结局,面板恒挂「运行中」。
                            // 只补「在跑且尚无 id」的:已从正文抠到 id 的(Async agent launched)
                            // 不动,非启动回执不碰。
                            if let Some(outcome) = subagent.as_mut() {
                                if outcome.running > 0 && outcome.task_id.is_none() {
                                    outcome.task_id = v
                                        .pointer("/toolUseResult/agentId")
                                        .and_then(|x| x.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(str::to_string);
                                }
                            }
                            // GUI 代答 AskUserQuestion 走 PreToolUse deny,CC 把答案记成
                            // error 回执——对用户它是「已作答」不是失败,按哨兵压平,
                            // 否则对话流红块 + handoff 标 [失败]。在截断前的原始文本上
                            // 判(哨兵在文首,截断本伤不到,防御截断策略变化)。
                            let answered_by_meowo =
                                text.contains(meowo_protocol::broker::QUESTION_ANSWER_MARKER);
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
                                    .unwrap_or(false)
                                    && !answered_by_meowo,
                                subagent,
                            })
                        }
                        _ => None,
                    },
                )
                .collect()
        }
        "assistant" => {
            // model=<synthetic> 是 CC 对「非模型产出的系统插入文案」的官方标记（API 错误
            // 全走它）。命中错误分类的 text 块降级为 TurnError——前端渲染成错误气泡；
            // 非错误的 synthetic 文案（如「No response requested.」）仍按普通正文显示。
            let synthetic = v
                .pointer("/message/model")
                .and_then(|x| x.as_str())
                == Some("<synthetic>");
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
                        .map(|text| {
                            if let Some(label) =
                                synthetic.then(|| classify_error(text, true)).flatten()
                            {
                                return TranscriptEvent::TurnError {
                                    id: format!("{base_id}:{i}"),
                                    timestamp: timestamp.clone(),
                                    label: label.to_string(),
                                    text: text.to_string(),
                                };
                            }
                            TranscriptEvent::AssistantMessage {
                                id: format!("{base_id}:{i}"),
                                timestamp: timestamp.clone(),
                                text: text.to_string(),
                            }
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
            .collect()
        }
        "system" if v.get("subtype").and_then(|x| x.as_str()) == Some("compact_boundary") => {
            vec![TranscriptEvent::Metadata {
                id: base_id.to_string(),
                timestamp,
                kind: "compact".into(),
            }]
        }
        // 运行中排队的插话被 CLI 送入时**不落普通 user 行**：记成 attachment/queued_command，
        // 正文在 attachment.prompt（字符串或 content blocks）。不解析它的话，被处理的插话
        // 在结构化对话里凭空消失——回执消解了、消息却哪儿都看不见（用户实拍反馈）。
        // prompt 里的 image 块是内嵌 base64、没有本地路径（CLI 入队时就把附件行里的路径
        // 吃掉换成了图像块）：落盘成临时文件后以 `[Image: source: …]` 行并入正文——与 CC
        // 记录粘贴图片的形制一致，前端现成的缩略图链路直接接住。
        "attachment" => {
            let attachment = v.get("attachment");
            if attachment.and_then(|a| a.get("type")).and_then(|x| x.as_str())
                != Some("queued_command")
            {
                return Vec::new();
            }
            let prompt = attachment.and_then(|a| a.get("prompt"));
            let mut text = match prompt {
                Some(serde_json::Value::String(s)) => s.trim().to_string(),
                Some(serde_json::Value::Array(blocks)) => blocks
                    .iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|x| x.as_str()) != Some("text") {
                            return None;
                        }
                        block.get("text").and_then(|x| x.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string(),
                _ => String::new(),
            };
            for (i, block) in prompt
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .enumerate()
            {
                if block.get("type").and_then(|x| x.as_str()) != Some("image") {
                    continue;
                }
                let Some(source) = block.get("source") else {
                    continue;
                };
                if source.get("type").and_then(|x| x.as_str()) != Some("base64") {
                    continue;
                }
                // 渲染端按扩展名认图（Message.tsx 的 IMAGE_EXTENSIONS），未知类型不落盘。
                let ext = match source.get("media_type").and_then(|x| x.as_str()) {
                    Some("image/png") => "png",
                    Some("image/jpeg") => "jpg",
                    Some("image/gif") => "gif",
                    Some("image/webp") => "webp",
                    _ => continue,
                };
                let Some(data) = source.get("data").and_then(|x| x.as_str()) else {
                    continue;
                };
                if let Some(path) = persist_queued_image(base_id, i, ext, data) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[Image: source: {}]", path.display()));
                }
            }
            if text.is_empty() {
                return Vec::new();
            }
            // 主 agent 正忙时后台任务的完成通知会走**排队**送入——不落普通 user 行,只有
            // 这条 queued_command attachment(实拍:审查子任务完成于回合中途,通知经
            // enqueue → 本行送达)。不认它,该子任务的徽标就永远「运行中」,通知原文还会
            // 以一坨 XML 的样子出现在时间线里。与 user 行同款转译成合成回执。
            if let Some(event) = task_notification_result(&text, base_id, timestamp.clone()) {
                return vec![event];
            }
            vec![TranscriptEvent::UserMessage {
                id: base_id.to_string(),
                timestamp,
                text,
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

/// 把 assistant 正文归类为「回合错误」短标签；非错误返回 None。
/// `synthetic` = 该消息的 model 为 `<synthetic>`——CC 对「非模型产出的系统插入文案」的
/// 官方标记，API 错误全部以它写盘。鉴权类规则不依赖它（老 transcript 无 model 字段也要
/// 能判）；**通用 API 错误只在 synthetic 下判**——正常回答里聊到「API Error」字样绝不能
/// 标红，这个标记就是零误报的那道门。长正文（讨论/引用错误日志的正常回答）不判错。
///
/// 529/5xx/连接中断这类「临时」错误此前刻意放过（怕自愈路上误报）——那是只能拿文本猜
/// 的年代的防误报代价。判定的输入本就是**最近一条** assistant：重试成功自会有新正文把
/// 错误顶掉，还挂在末尾就意味着 agent 此刻停在错误上，不提示的话会话就静静挂着，
/// 用户以为还在跑（实拍反馈：API Error 在 meowo 里毫无动静）。
pub(crate) fn classify_error(text: &str, synthetic: bool) -> Option<&'static str> {
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
    // 实拍文案（2026-08 各版本）：「API Error: Overloaded」「API Error: Connection lost
    // mid-response. …」「API Error: upstream stream disconnected: unexpected EOF」
    // 「Unable to connect to API (ECONNRESET)」。都以固定前缀开头，synthetic 门内按前缀分两类。
    if synthetic {
        if t.starts_with("Unable to connect to API") {
            return Some("无法连接 API");
        }
        if t.starts_with("API Error") {
            return Some("API 请求出错");
        }
    }
    None
}

/// 增量解析的累积状态：标题（custom/ai 分开存，custom 优先）、最近一条 assistant 正文、
/// 最近一条 usage、还在跑的后台子任务集。逐行 fold，故对「只追加」的 transcript 可跨
/// 多次调用累积，无需重头扫。
#[derive(Default, Clone)]
struct ParseState {
    custom: Option<String>,
    ai: Option<String>,
    last_text: Option<(String, bool)>, // (正文, model 是否 <synthetic>)
    last_usage: Option<u64>,           // 最近一条 assistant 的上下文已用 token
    /// 已发启动回执、尚无结局信号的后台任务 id（Agent 委派的 agentId / 后台 Bash 的
    /// shell id）。结局信号有三形:
    /// user 行的 task-notification、排队送入的通知(queue-operation/attachment)、
    /// TaskOutput 拉取回执。主回合结束后这里非空 = 后台还有活儿在跑。
    running_tasks: std::collections::HashSet<String>,
}

impl ParseState {
    /// 折叠一行 JSONL：只关心 title / assistant / 后台任务信号行，其它快速跳过（不解析）。
    fn fold_line(&mut self, line: &str) {
        let has_title = line.contains("-title");
        let has_assistant = line.contains("\"assistant\"");
        // 后台任务的启动/结局信号:行形态各异(user 回执、排队通知、TaskOutput 回执),
        // 先做廉价子串门卫,命中才付 JSON 解析。
        let has_task_signal = line.contains("Async agent launched")
            || line.contains(BACKGROUND_SHELL_PREFIX)
            || line.contains("<task-notification>")
            || line.contains("<retrieval_status>");
        if !has_title && !has_assistant && !has_task_signal {
            return;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        if has_task_signal {
            self.fold_task_signals(&v);
        }
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
                    // CC 把 API 错误等系统插入文案写成 model=<synthetic> 的 assistant 行，
                    // 这是「这不是模型说的话」的官方标记，供 classify_error 零误报判错。
                    let synthetic = v
                        .get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(|m| m.as_str())
                        == Some("<synthetic>");
                    self.last_text = Some((text, synthetic));
                }
            }
            _ => {}
        }
    }

    /// 折叠后台任务的启动/结局信号，维护 running_tasks 集。
    ///
    /// 启动:user 行里的启动回执——`Async agent launched…agentId: xxx`(Agent 委派)
    /// 与 `Command running in background with ID: xxx`(后台 Bash),见 [`is_launch_receipt`]。
    /// 结局(移除):
    /// - user 行的 `<task-notification>`(内容字符串或 text 块);
    /// - **排队形态**的通知——主 agent 正忙时通知不落 user 行,记成 queue-operation
    ///   enqueue(通知生成时结局已定,即为结束信号)与 attachment/queued_command(送达);
    /// - TaskOutput 拉取回执(`<retrieval_status>` + 终态 status)。
    ///
    /// isSidechain 行不看:子任务自己的委派不算主会话的后台工作。
    fn fold_task_signals(&mut self, v: &serde_json::Value) {
        if v.get("isSidechain").and_then(|x| x.as_bool()) == Some(true) {
            return;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => {
                let content = v.pointer("/message/content");
                if let Some(text) = content.and_then(|x| x.as_str()) {
                    self.settle_from_notification(text);
                    return;
                }
                for block in content.and_then(|x| x.as_array()).into_iter().flatten() {
                    match block.get("type").and_then(|x| x.as_str()) {
                        Some("tool_result") => {
                            let text = text_from_content(
                                block.get("content").unwrap_or(&serde_json::Value::Null),
                            );
                            self.fold_tool_result_text(&text);
                        }
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(|x| x.as_str()) {
                                self.settle_from_notification(text);
                            }
                        }
                        _ => {}
                    }
                }
            }
            // enqueue 即结束信号;remove 是队列记账(送达/撤下),不重复处理。
            Some("queue-operation") => {
                if v.get("operation").and_then(|x| x.as_str()) == Some("enqueue") {
                    if let Some(text) = v.get("content").and_then(|x| x.as_str()) {
                        self.settle_from_notification(text);
                    }
                }
            }
            Some("attachment") => {
                let prompt = v.pointer("/attachment/prompt");
                if let Some(text) = prompt.and_then(|x| x.as_str()) {
                    self.settle_from_notification(text);
                    return;
                }
                for block in prompt.and_then(|x| x.as_array()).into_iter().flatten() {
                    if let Some(text) = block.get("text").and_then(|x| x.as_str()) {
                        self.settle_from_notification(text);
                    }
                }
            }
            _ => {}
        }
    }

    /// `<task-notification>` 正文 → 按 `<task-id>` 移除运行集。必须以标签开头才认——
    /// 用户/助手**引用**通知文案的消息不该误消(讨论这套机制的会话里真会出现)。
    fn settle_from_notification(&mut self, text: &str) {
        if !text.trim_start().starts_with("<task-notification>") {
            return;
        }
        if let Some(id) = tag_text(text, "task-id") {
            self.running_tasks.remove(id);
        }
    }

    /// 工具回执正文:启动回执入集,TaskOutput 拉取的终态回执出集(running/timeout 保持)。
    fn fold_tool_result_text(&mut self, text: &str) {
        if is_launch_receipt(text) {
            if let Some(id) = launch_agent_id(text) {
                self.running_tasks.insert(id);
            }
            return;
        }
        if !text.trim_start().starts_with("<retrieval_status>") {
            return;
        }
        if let (Some(id), Some(status)) = (tag_text(text, "task_id"), tag_text(text, "status")) {
            if matches!(status, "completed" | "failed" | "killed") {
                self.running_tasks.remove(id);
            }
        }
    }

    /// 从累积状态产出 TranscriptInfo。
    fn to_info(&self) -> TranscriptInfo {
        let error = self.last_text.as_ref().and_then(|(text, synthetic)| {
            classify_error(text, *synthetic).map(|label| TurnError {
                label: label.to_string(),
                raw: text.clone(),
                // 指纹取标签而非消息 uuid：CC 自动重试期间每隔几十秒写一条**新 uuid** 的
                // 同类 synthetic 错误（实拍 30s 一条），按 uuid 去重等于每条各弹一次桌面
                // 通知。按标签去重，一轮故障只弹一条；错误消失时 watch 清掉去重条目，
                // 之后再出错（哪怕同标签）照样重新提醒。
                fingerprint: label.to_string(),
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
            busy_subagents: self.running_tasks.len() as u32,
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

/// 数据根解析与目录枚举（含 symlink/junction 解引用）收敛在 `crate::managed_profile_dirs`，
/// 与 kimi 侧共用同一份规则。
fn managed_config_dirs() -> Vec<std::path::PathBuf> {
    crate::managed_profile_dirs(crate::id::CLAUDE.as_str())
}

/// 根据 cwd + session_id 重建 transcript 路径：
/// ~/.claude/projects/<encode(cwd)>/<session_id>.jsonl。
///
/// 默认目录与各 managed profile 目录**一视同仁按 mtime 取最新**，不能「默认目录存在
/// 即返回」：跨 profile 恢复会话时宿主把 transcript 复制进目标 CLAUDE_CONFIG_DIR，
/// 原文件留在默认目录成为不再增长的陈旧副本——偏爱默认目录会让对话页从此只看到
/// 副本的截止时刻，会话明明在跑、新消息却永远不出现。
///
/// **Some ⇒ 文件存在**（对齐 `TranscriptSpec::resolve_transcript_path` 的契约）：所有
/// 候选都不存在时返回 None，而不是回吐一个不存在的默认路径——那会让调用方各自补
/// `.filter(|p| p.exists())`，漏一处就把幻影路径当真（reporter 的测试曾直接 unwrap）。
pub fn reconstruct_transcript_path(cwd: &str, session_id: &str) -> Option<std::path::PathBuf> {
    let relative = std::path::PathBuf::from(encode_cwd(cwd)).join(format!("{session_id}.jsonl"));
    let default = projects_dir()?.join(&relative);
    std::iter::once(default)
        .chain(
            managed_projects_dirs()
                .into_iter()
                .map(|projects| projects.join(&relative)),
        )
        .filter(|path| path.exists())
        .max_by_key(|path| transcript_freshness(path))
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
                // path().is_dir() 解引用 symlink/junction（DirEntry::file_type 不会），
                // 与 managed_profile_dirs 的枚举纪律一致。
                .filter(|entry| entry.path().is_dir())
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
        // reconstruct 的 Some 已保证文件存在，不必再验一次。
        if let Some(p) = reconstruct_transcript_path(cwd, session_id) {
            return Some(p);
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
        // 常规委派靠 meta 的 toolUseId 直连;连不上再试 forked skill(那条路没有外键,
        // 得回主链捞出 `/<skill> <args>` 去对 meta 的 description,见 locate_forked)。
        Self::locate_one(main_transcript, tool_use_id)
            .or_else(|| Self::locate_forked(main_transcript, tool_use_id))
            .map(|path| {
                vec![SubagentStream {
                    label: None,
                    // claude 的 meta.json 只记身份不记结局，没有可靠的状态信号。
                    status: None,
                    finished_at: None,
                    path,
                }]
            })
            .unwrap_or_default()
    }

    fn parse_stream_line(&self, line: &str) -> Vec<TranscriptEvent> {
        parse_events(line, true)
    }

    /// 后台任务的**启动回执**:`Agent` 现版本默认异步,立即回一句
    /// `Async agent launched successfully. …`;后台 Bash(`run_in_background`)同理回
    /// `Command running in background with ID: …`。两者此时都才刚开跑。没有这个信号,
    /// 前端「已回执=已完成」的兜底会把在跑的后台任务标成完成(实拍反馈)。
    ///
    /// 真结局有两条到达路径,必须都认:
    /// 1. `<task-notification>` 合成回执(见 [`task_notification_result`]),自带 tool-use-id;
    /// 2. 主 agent 用 `TaskOutput` 主动拉取——**拉过之后 CLI 不再注入通知**(实拍:一个会话
    ///    7 次委派 4 次只有启动回执),只认通知会让这些子任务永远显示「运行中」。TaskOutput
    ///    的回执挂在它自己的调用上,靠 `task_id`(= 启动回执里的 `agentId`)归回原委派。
    fn detect_result(&self, output: &str) -> Option<SubagentOutcome> {
        // forked skill(`/code-review` 等):委派本体是条 Skill 调用,fork 与否在调用参数里
        // 完全看不出来,只有回执写着 `(forked execution)`——它是这条链路唯一的委派证据。
        if let Some(outcome) = forked_skill_outcome(output) {
            return Some(outcome);
        }
        if is_launch_receipt(output) {
            return Some(SubagentOutcome {
                running: 1,
                completed: 0,
                failed: 0,
                // 启动回执携带任务 id(`agentId: xxx …`),是 task_id → 委派 的映射来源。
                task_id: launch_agent_id(output),
            });
        }
        // TaskOutput 回执(实拍形态):`<retrieval_status>…</retrieval_status>` 开头,
        // 带 `<task_id>` 与 `<status>completed|running|failed</status>`。
        // 没有 task_id 的检索错误(<retrieval_status>error)不认——归不到任何委派。
        if !output.trim_start().starts_with("<retrieval_status>") {
            return None;
        }
        let task_id = tag_text(output, "task_id")?.to_string();
        let status = tag_text(output, "status").unwrap_or("running");
        let (running, completed, failed) = match status {
            "completed" => (0, 1, 0),
            "failed" | "killed" => (0, 0, 1),
            // running / timeout 拉取:仍在跑,如实报 running(与启动回执一致)。
            _ => (1, 0, 0),
        };
        Some(SubagentOutcome {
            running,
            completed,
            failed,
            task_id: Some(task_id),
        })
    }
}

/// 后台 Bash(`run_in_background`)启动回执的固定开头,后面紧跟任务 id。
/// 真机取证:`Command running in background with ID: b78nfkj1v. Output is being written to: …`。
const BACKGROUND_SHELL_PREFIX: &str = "Command running in background with ID: ";

/// 这段工具回执是不是后台任务的**启动回执**。两种形态都认(CC 2.1.246 实拍):
///
/// ```text
/// Async agent launched successfully. …agentId: a9b726e3a088bafea …   ← Agent 委派
/// Command running in background with ID: b78nfkj1v. Output is …      ← 后台 Bash
/// ```
///
/// 后台 Bash 与 Agent 委派共用同一条结局通道(`<task-notification>` 的 `<task-id>` 即
/// 这里的 shell id),漏认启动侧的后果是**只减不加**:主回合停了、后台命令还在跑,
/// 会话却报「等你输入」,子任务面板也一片空白(2026-08-27 实拍)。
///
/// 必须**行首锚定**而不是 contains——与 settle_from_notification 对 `<task-notification>`
/// 的纪律同源:讨论/排查这套机制的会话里,Read/Grep 源码或别的 transcript 的工具结果会
/// **引用**这两句话(连同注释里的 `agentId: xxx` 示例),contains 会据此记入幽灵任务,
/// 而幽灵永远等不到结局信号——会话从此恒挂「运行中」(2026-08-18 实拍:本仓 dogfooding
/// 会话查完子任务链路后自己被钉死在运行中)。真回执的正文以这句话开头,行首锚定天然
/// 免疫行号/注释前缀的引用。
fn is_launch_receipt(text: &str) -> bool {
    let head = text.trim_start();
    head.starts_with("Async agent launched") || head.starts_with(BACKGROUND_SHELL_PREFIX)
}

/// forked skill 的回执 → 结局统计。实测三种首行(CC 2.1.246):
///
/// ```text
/// Skill "code-review" launched (forked execution, running in the background).
/// Skill "code-review" completed (forked execution).
/// ```
///
/// 为什么这条回执值钱:`/code-review` 这类 skill 是 **fork 出去跑**的,主链上只有一条
/// `Skill` 工具调用、**没有** `Agent` 委派,侧车 meta 也不带 `toolUseId`——若不认这条回执,
/// 整场审查的十几个 agent 在 GUI 里一个都看不见(实拍:进度面板全空)。
///
/// 与 [`is_launch_receipt`] 同一条纪律:**行首锚定,禁 contains**。排查这套机制的会话里
/// (比如此刻)Read/Grep 源码的工具结果会原样引用上面这几句,contains 会把它们记成幽灵
/// 委派,而幽灵永远等不到结局——会话从此恒挂「运行中」。真回执以这句话开头。
fn forked_skill_outcome(output: &str) -> Option<SubagentOutcome> {
    let head = output.trim_start().lines().next()?;
    if !head.starts_with("Skill \"") || !head.contains("(forked execution") {
        return None;
    }
    // 引号后的动词才是结局。用 `" <verb> (forked` 整体匹配,避免 skill 名字里恰好含
    // "completed" 之类的词把状态带偏。
    let (running, completed, failed) = if head.contains("\" launched (forked execution") {
        (1, 0, 0)
    } else if head.contains("\" completed (forked execution") {
        (0, 1, 0)
    } else if head.contains("\" failed (forked execution") {
        (0, 0, 1)
    } else {
        // 没见过的动词:不猜。宁可这条委派退回「无结局统计」(前端按未回执=在跑处理),
        // 也不谎报成完成。
        return None;
    };
    Some(SubagentOutcome {
        running,
        completed,
        failed,
        // forked skill 的回执不带任务 id;委派靠 tool_use_id 直连,不需要 task_id 路由。
        task_id: None,
    })
}

/// 从启动回执正文抠任务 id:Agent 委派取 `agentId: a9b726e3a088bafea (internal ID …)`,
/// 后台 Bash 取行首 `Command running in background with ID: b78nfkj1v.` 里的那串。
/// 手写扫描,取到第一个非字母数字为止;形态变了返回 None,徽标退回纯 running。
///
/// 后台 Bash 这一路**行首锚定**(strip_prefix 而非 find):这句话被引用时不能记成幽灵任务,
/// 理由同 [`is_launch_receipt`]。
fn launch_agent_id(output: &str) -> Option<String> {
    let head = output.trim_start();
    let rest = match head.strip_prefix(BACKGROUND_SHELL_PREFIX) {
        Some(rest) => rest,
        None => &output[output.find("agentId: ")? + "agentId: ".len()..],
    };
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (!id.is_empty()).then_some(id)
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

    /// forked skill 的侧车定位。
    ///
    /// fork 出的顶层 agent 的 meta **没有 `toolUseId`**（实测 CC 2.1.246：
    /// `{"agentType":"general-purpose","description":"/code-review 1692 高强度",
    /// "name":"code-review","spawnDepth":1}`），[`Self::locate_one`] 那条外键路走不通。
    /// 唯一可用的关联是 `description` —— 它与主链那条 Skill 调用的 `/<skill> <args>`
    /// 同形（见 [`skill_invocation_desc`]）。
    ///
    /// 同一会话可能把**同一条命令**跑好几次（实拍：`/code-review 1692 高强度` 两次），
    /// 描述就不再唯一。故按**序号**配对：主链里这是第 n 次同款调用，就取第 n 个同款侧车。
    ///
    /// 侧车的先后以 `<stem>.forked-skill.json`(spawn 标记)的修改时间为准。**不能用
    /// `meta.json`**：它并非「写一次就不再动」——真机反证(2026-08-26)`agent-acb7f0cbde2e`
    /// 的 spawn 标记是 11:28:21、meta 却在 11:36:29 被重写，差 487 秒；跨夜 resume 的样本
    /// 差了 15 小时。用它排序会让两次重叠的同款 fork 顺序颠倒，展开第一次看到的是第二次
    /// 那条流的完整记录——**给错记录比给不出记录更糟**。标记文件缺失时退回 meta（聊胜于无）。
    ///
    /// 数量对不上就**拒绝配对**：ordinal 数的是主链调用，而这里只数得到**现存**侧车，
    /// 少一个就整体错位（实测「completed (forked execution)」的内联完成压根不落侧车）。
    /// 此时返回 None，前端显示「该子任务还没有留下记录」——空白是诚实的，错配不是。
    /// 代价：刚派出去、侧车还没落盘的那几秒里，同款的既有委派也一并展不开，随后自愈。
    fn locate_forked(main_transcript: &Path, tool_use_id: &str) -> Option<PathBuf> {
        let dir = Self::stream_dir(main_transcript)?;
        let (desc, ordinal, total) = Self::skill_call_key(main_transcript, tool_use_id)?;
        let mut matches: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            let Some(stem) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".meta.json"))
            else {
                continue;
            };
            let Ok(meta) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<serde_json::Value>(&meta) else {
                continue;
            };
            // 只认顶层 fork：带 toolUseId 的是常规委派（locate_one 的地盘），
            // spawnDepth > 1 的是 fork 内部再派的孙子——它们**有** toolUseId，
            // 指向 fork 自己那条流里的调用，展开时照常走 locate_one，不该被这里截胡。
            if meta.get("toolUseId").is_some() {
                continue;
            }
            if meta.get("description").and_then(|v| v.as_str()) != Some(desc.as_str()) {
                continue;
            }
            let stream = dir.join(format!("{stem}.jsonl"));
            if !stream.is_file() {
                continue;
            }
            // 出生时刻取 spawn 标记；标记不在(旧会话/被清理)才退回 meta。
            let marker = dir.join(format!("{stem}.forked-skill.json"));
            let born = std::fs::metadata(&marker)
                .or_else(|_| entry.metadata())
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            matches.push((born, stream));
        }
        // 数量对不上 = 配对不可靠(理由见函数注释),宁可不给也不给错的。
        if matches.len() != total {
            return None;
        }
        matches.sort_by_key(|(born, _)| *born);
        matches.into_iter().nth(ordinal).map(|(_, path)| path)
    }

    /// 回主 transcript 捞出这条 `Skill` 调用的 `/<skill> <args>`、它是第几次同款调用
    /// （从 0 起），以及同款调用**总数**（用于判断配对是否可靠，见 [`Self::locate_forked`]）。
    /// 按需路径（用户展开子任务才走），整扫一遍可接受；先用子串预筛，
    /// 真正解析 JSON 的只有含 `"Skill"` 的那几行。
    fn skill_call_key(main_transcript: &Path, tool_use_id: &str) -> Option<(String, usize, usize)> {
        let text = std::fs::read_to_string(main_transcript).ok()?;
        let mut calls: Vec<(String, String)> = Vec::new(); // (tool_use_id, desc)
        for line in text.lines() {
            if !line.contains("\"Skill\"") {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(blocks) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                continue;
            };
            for block in blocks {
                if block.get("type").and_then(|v| v.as_str()) != Some("tool_use")
                    || block.get("name").and_then(|v| v.as_str()) != Some("Skill")
                {
                    continue;
                }
                let (Some(id), Some(input)) = (
                    block.get("id").and_then(|v| v.as_str()),
                    block.get("input"),
                ) else {
                    continue;
                };
                let Some(skill) = input.get("skill").and_then(|v| v.as_str()) else {
                    continue;
                };
                calls.push((
                    id.to_string(),
                    skill_invocation_desc(skill, input.get("args")),
                ));
            }
        }
        let index = calls.iter().position(|(id, _)| id == tool_use_id)?;
        let desc = calls[index].1.clone();
        let ordinal = calls[..index].iter().filter(|(_, d)| *d == desc).count();
        let total = calls.iter().filter(|(_, d)| *d == desc).count();
        Some((desc, ordinal, total))
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
            // reconstruct 的 Some ⇒ 文件存在，即校验通过。
            if reconstruct_transcript_path(c, session_id).is_some() {
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

    /// GUI 代答 AskUserQuestion 的 deny 回执带哨兵前缀:对用户它是「已作答」不是工具
    /// 失败——is_error 压平(红色错误块与 handoff 的 [失败] 前缀一并消失)。
    /// 无哨兵的真错误不受影响。
    #[test]
    fn meowo_answered_question_receipt_is_not_an_error() {
        let answered = format!(
            r#"{{"type":"user","uuid":"u1","message":{{"content":[{{"type":"tool_result","tool_use_id":"tool-1","content":"{}晚饭吃什么? → 火锅","is_error":true}}]}}}}"#,
            meowo_protocol::broker::QUESTION_ANSWER_MARKER
        );
        assert!(matches!(
            &parse_chat_items(&answered)[0],
            ChatItem::ToolResult { is_error: false, .. }
        ));

        let failed = r#"{"type":"user","uuid":"u2","message":{"content":[{"type":"tool_result","tool_use_id":"tool-2","content":"command not found","is_error":true}]}}"#;
        assert!(matches!(
            &parse_chat_items(failed)[0],
            ChatItem::ToolResult { is_error: true, .. }
        ));
    }

    /// 排队插话被 CLI 送入时不落普通 user 行，而是 attachment/queued_command——
    /// 不解析它，被处理的插话在结构化对话里凭空消失（实拍回归）。图片块是内嵌 base64
    /// 无本地路径：落盘成临时文件后以 `[Image: source: …]` 行并入正文，接上前端现成的
    /// 缩略图链路；其余 attachment 子类型（skill_listing 等）照旧不进时间线。
    #[test]
    fn queued_command_attachment_becomes_user_message_with_images() {
        let queued = r#"{"type":"attachment","uuid":"q1-test-image","timestamp":"2026-08-11T04:02:17Z","isSidechain":false,"attachment":{"type":"queued_command","prompt":[{"type":"text","text":"这里还要再加一个排序"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]}}"#;
        let items = parse_chat_items(queued);
        assert_eq!(items.len(), 1);
        let ChatItem::UserText { id, text, .. } = &items[0] else {
            panic!("expected UserText, got {:?}", items[0]);
        };
        assert_eq!(id, "q1-test-image");
        let expected_path = std::env::temp_dir()
            .join("meowo-paste")
            .join("queued")
            .join("q1-test-image-1.png");
        assert_eq!(
            text,
            &format!("这里还要再加一个排序\n[Image: source: {}]", expected_path.display())
        );
        assert!(expected_path.exists());
        // 幂等：重解析同一行不重写、结果一致。
        assert_eq!(parse_chat_items(queued), items);
        std::fs::remove_file(&expected_path).ok();

        // prompt 为纯字符串的兼容形状。
        let plain = r#"{"type":"attachment","uuid":"q2","attachment":{"type":"queued_command","prompt":"继续"}}"#;
        assert!(
            matches!(&parse_chat_items(plain)[0], ChatItem::UserText { text, .. } if text == "继续")
        );

        // 解码失败的图片块只丢那张图，正文照常。
        let broken = r#"{"type":"attachment","uuid":"q3","attachment":{"type":"queued_command","prompt":[{"type":"text","text":"看图"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"!!!"}}]}}"#;
        assert!(
            matches!(&parse_chat_items(broken)[0], ChatItem::UserText { text, .. } if text == "看图")
        );

        let skill = r#"{"type":"attachment","uuid":"s1","attachment":{"type":"skill_listing","content":"..."}}"#;
        assert!(parse_chat_items(skill).is_empty());
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
        // 鉴权类不依赖 synthetic 标记（老 transcript 无 model 字段也要能判）。
        assert_eq!(
            classify_error("The model's tool call could not be parsed (retry also failed).", false),
            Some("工具调用解析失败")
        );
        assert_eq!(
            classify_error("Please run /login · API Error: 403 Request not allowed", false),
            Some("需要重新登录")
        );
        assert_eq!(
            classify_error("API Error: 403 Request not allowed", false),
            Some("需要重新登录")
        );
        assert_eq!(
            classify_error(
                "Failed to authenticate. API Error: 401 Invalid authentication credentials",
                false
            ),
            Some("认证失败")
        );
        assert_eq!(
            classify_error("API Error: 401 Invalid authentication credentials", false),
            Some("认证失败")
        );
    }

    #[test]
    fn classify_api_errors_only_behind_synthetic_gate() {
        // synthetic 门内：通用 API 错误按前缀分两类（实拍文案）。
        assert_eq!(
            classify_error("API Error: Overloaded", true),
            Some("API 请求出错")
        );
        assert_eq!(
            classify_error(
                "API Error: Connection lost mid-response. The response above may be incomplete.",
                true
            ),
            Some("API 请求出错")
        );
        assert_eq!(
            classify_error("API Error: upstream stream disconnected: unexpected EOF", true),
            Some("API 请求出错")
        );
        assert_eq!(
            classify_error("Unable to connect to API (ECONNRESET)", true),
            Some("无法连接 API")
        );
        // synthetic 的非错误插入文案（如队列消息「No response requested.」）不判错。
        assert_eq!(classify_error("No response requested.", true), None);
        assert_eq!(classify_error("这是一段正常的助手回答。", true), None);
        // 门外：模型正文聊到「API Error」字样绝不能标红——这正是当年放过 5xx 的原因，
        // 现在由 synthetic 标记承担零误报，门外维持 None。
        assert_eq!(
            classify_error("API Error: 529 Overloaded. This is a server-side issue", false),
            None
        );
        assert_eq!(classify_error("API Error: 500 status code (no body)", false), None);
        assert_eq!(classify_error("Unable to connect to API (ECONNRESET)", false), None);
    }

    #[test]
    fn classify_ignores_long_text_quoting_error() {
        // 正常长回答里引用错误文案（如调试 API 的会话）不应被判为卡死。
        let long = format!(
            "{}先看日志里的 API Error: 403 Request not allowed，这是因为……",
            "分析：".repeat(100)
        );
        assert_eq!(classify_error(&long, false), None);
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
        // 指纹 = 标签（重试连发同类错误只提醒一次，见 to_info 注释）。
        assert_eq!(e.fingerprint, "工具调用解析失败");
    }

    #[test]
    fn chat_parser_renders_synthetic_api_error_as_turn_error() {
        // synthetic + 错误分类命中 → TurnError（前端错误气泡）；label 与卡片 error_label 同源。
        let line = r#"{"type":"assistant","uuid":"a-err","message":{"role":"assistant","model":"<synthetic>","content":[{"type":"text","text":"API Error: Overloaded"}]}}"#;
        let items = parse_chat_items(line);
        assert!(
            matches!(&items[0], ChatItem::TurnError { label, text, .. }
                if label == "API 请求出错" && text == "API Error: Overloaded"),
            "items={items:?}"
        );
        // 非错误的 synthetic 插入文案仍是普通正文，不套错误皮。
        let benign = r#"{"type":"assistant","uuid":"a-ok","message":{"role":"assistant","model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}}"#;
        assert!(matches!(
            &parse_chat_items(benign)[0],
            ChatItem::AssistantText { .. }
        ));
        // 模型正文聊到「API Error」（无 synthetic 标记）绝不误判成错误气泡。
        let normal = r#"{"type":"assistant","uuid":"a-talk","message":{"role":"assistant","model":"claude-opus-4","content":[{"type":"text","text":"API Error: 500 的成因通常是……"}]}}"#;
        assert!(matches!(
            &parse_chat_items(normal)[0],
            ChatItem::AssistantText { .. }
        ));
    }

    #[test]
    fn analyze_flags_synthetic_api_error_at_tail() {
        // 实拍：CC 把 API 错误写成 model=<synthetic> 的 assistant 行；停在末尾就该提示
        //（此前被「临时错误」名单放过，meowo 全程无动静——本测试钉住这条回归）。
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"正在处理。"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"u-api","message":{"role":"assistant","model":"<synthetic>","content":[{"type":"text","text":"API Error: Overloaded"}]}}"#,
            "\n",
        );
        let p = write_tmp("api_err", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        let e = info.error.expect("末尾的 synthetic API 错误应被检测");
        assert_eq!(e.label, "API 请求出错");
        assert_eq!(e.raw, "API Error: Overloaded");
        assert_eq!(e.fingerprint, "API 请求出错");
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
        // meowo PTY 里跑测试会带着指向真库的 MEOWO_DB——数据根解析优先读它，
        // 不一并指进临时 home 的话，managed 目录会扫到真实 profile 而不是本用例造的。
        let old_db = std::env::var("MEOWO_DB").ok();
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("MEOWO_DB", home.join(".meowo").join("board.db"));
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
        match old_db {
            Some(value) => std::env::set_var("MEOWO_DB", value),
            None => std::env::remove_var("MEOWO_DB"),
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
        // 同 transcript_lookup_includes_managed_claude_profiles：MEOWO_DB 必须跟着指进
        // 临时 home，否则 managed 目录解析到真库旁的 profiles，本用例的 fresh 副本扫不到。
        let old_db = std::env::var("MEOWO_DB").ok();
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("MEOWO_DB", home.join(".meowo").join("board.db"));
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
        match old_db {
            Some(value) => std::env::set_var("MEOWO_DB", value),
            None => std::env::remove_var("MEOWO_DB"),
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
        // 同上两例：MEOWO_DB 跟着指进临时 home，managed 侧候选才落在本用例的目录里。
        let old_db = std::env::var("MEOWO_DB").ok();
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("MEOWO_DB", home.join(".meowo").join("board.db"));
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
        match old_db {
            Some(value) => std::env::set_var("MEOWO_DB", value),
            None => std::env::remove_var("MEOWO_DB"),
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

    /// 任务列表工具的摘要是前端重建任务列表的数据源(CC 对它们不触发 hook):
    /// TaskCreate 必须是纯 subject;TaskUpdate 必须是**没截断的合法 JSON**——
    /// 整包兜底会被 metadata/description 顶过 800 上限,截一刀就 parse 不回来了。
    #[test]
    fn task_tool_summaries_are_frontend_parseable() {
        let long = "很长".repeat(600);
        let create = format!(
            r#"{{"type":"assistant","uuid":"a1","message":{{"content":[{{"type":"tool_use","id":"t1","name":"TaskCreate","input":{{"subject":"生成基线迁移","description":"{long}"}}}}]}}}}"#
        );
        let ChatItem::ToolUse { summary, .. } = &parse_chat_items(&create)[0] else {
            panic!("应解析成工具调用");
        };
        assert_eq!(summary, "生成基线迁移");

        let update = format!(
            r#"{{"type":"assistant","uuid":"a2","message":{{"content":[{{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{{"taskId":"3","status":"completed","metadata":{{"note":"{long}"}}}}}}]}}}}"#
        );
        let ChatItem::ToolUse { summary, .. } = &parse_chat_items(&update)[0] else {
            panic!("应解析成工具调用");
        };
        let parsed: serde_json::Value = serde_json::from_str(summary).expect("摘要应是合法 JSON");
        assert_eq!(parsed["taskId"], "3");
        assert_eq!(parsed["status"], "completed");
    }

    /// 后台委派(`Agent` 现版本默认异步)的立即回执只是「派出去了」,不是完成——
    /// 此前不带结局信号,前端「已回执=完成」的兜底把在跑的后台子任务标成完成(实拍反馈)。
    /// 启动回执须标 running 并带任务 id;真结局由 task-notification 或 TaskOutput 回执覆盖。
    #[test]
    fn async_launch_receipt_marks_subagent_running() {
        let line = r#"{"type":"user","uuid":"u1","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_bg","content":[{"type":"text","text":"Async agent launched successfully. (This tool result is internal metadata — never quote or paste any part of it, including the agentId below, into a user-facing reply.)\nagentId: a9b726e3a088bafea (internal ID - do not mention to user.)\noutput_file: C:\\tmp\\x.output"}]}]}}"#;
        let ChatItem::ToolResult { subagent, .. } = &parse_chat_items(line)[0] else {
            panic!("应解析成工具回执");
        };
        let outcome = subagent.as_ref().expect("启动回执应标 running");
        assert_eq!((outcome.running, outcome.completed, outcome.failed), (1, 0, 0));
        // 任务 id 是 TaskOutput 回执归回原委派的唯一外键。
        assert_eq!(outcome.task_id.as_deref(), Some("a9b726e3a088bafea"));

        // 形态变了(没有 agentId 行)只丢外键,running 信号必须保住。
        let no_id = r#"{"type":"user","uuid":"u1b","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_bg2","content":[{"type":"text","text":"Async agent launched successfully."}]}]}}"#;
        let ChatItem::ToolResult { subagent, .. } = &parse_chat_items(no_id)[0] else {
            panic!("应解析成工具回执");
        };
        let outcome = subagent.as_ref().unwrap();
        assert_eq!(outcome.running, 1);
        assert_eq!(outcome.task_id, None);

        // 同步委派的回执(真实结果文本)不带结局信号——「已回执=完成」的兜底对它成立。
        let sync = r#"{"type":"user","uuid":"u2","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_s","content":"探索完成:共 3 个文件"}]}}"#;
        let ChatItem::ToolResult { subagent, .. } = &parse_chat_items(sync)[0] else {
            panic!("应解析成工具回执");
        };
        assert!(subagent.is_none());

        // 工具结果里**引用**回执文案(Read/Grep 源码的输出,行首是行号)不是启动回执:
        // contains 判据会给这类回执错挂 running 徽标(见 is_launch_receipt 注释)。
        let quoted = r#"{"type":"user","uuid":"u3","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_g","content":[{"type":"text","text":"1184  /// `Async agent launched successfully. …`,此时子任务才刚开跑。"}]}]}}"#;
        let ChatItem::ToolResult { subagent, .. } = &parse_chat_items(quoted)[0] else {
            panic!("应解析成工具回执");
        };
        assert!(subagent.is_none());
    }

    /// 主 agent 用 `TaskOutput` 拉取后台子任务结果时,CLI 不再注入 task-notification
    /// (实拍:一个会话 7 次委派 4 次因此没有完成信号)——这条回执必须也被认成结局,
    /// 靠 task_id 归回原委派,否则那些子任务永远显示「运行中」。
    #[test]
    fn taskoutput_receipt_carries_subagent_outcome() {
        // completed(实拍形态,<output> 里是子任务全文,这里截断)。
        let done = CLAUDE_SUBAGENTS.detect_result(
            "<retrieval_status>success</retrieval_status>\n\n<task_id>a60a5992b1bf8b192</task_id>\n\n<task_type>local_agent</task_type>\n\n<status>completed</status>\n\n<output>\n核完了。\n</output>",
        );
        let done = done.expect("completed 拉取应产出结局");
        assert_eq!((done.running, done.completed, done.failed), (0, 1, 0));
        assert_eq!(done.task_id.as_deref(), Some("a60a5992b1bf8b192"));

        // 超时拉取(任务还在跑):如实报 running,不误标完成。
        let waiting = CLAUDE_SUBAGENTS
            .detect_result("<retrieval_status>timeout</retrieval_status>\n\n<task_id>ad39aeb81d8552d59</task_id>\n\n<task_type>local_agent</task_type>\n\n<status>running</status>")
            .expect("timeout 拉取也应带回执");
        assert_eq!((waiting.running, waiting.completed, waiting.failed), (1, 0, 0));

        // failed 变体。
        let failed = CLAUDE_SUBAGENTS
            .detect_result("<retrieval_status>success</retrieval_status>\n<task_id>abc</task_id>\n<status>failed</status>")
            .expect("failed 拉取应产出结局");
        assert_eq!((failed.running, failed.completed, failed.failed), (0, 0, 1));

        // 检索错误没有 task_id → 归不到委派,不产出结局(宁缺毋错)。
        assert!(CLAUDE_SUBAGENTS
            .detect_result("<retrieval_status>error</retrieval_status>\nNo such task")
            .is_none());
        // 普通工具结果碰巧含尖括号不受影响。
        assert!(CLAUDE_SUBAGENTS.detect_result("cat 输出:<status>completed</status>").is_none());
    }

    /// `<task-notification>` user 消息是后台任务的完成通知,不是用户说的话:
    /// 应转译成该 tool_use 的合成回执(带结局统计),而不是一条用户气泡。
    #[test]
    fn task_notification_becomes_synthetic_tool_result() {
        let line = r#"{"type":"user","uuid":"n1","message":{"content":"<task-notification>\n<task-id>abc</task-id>\n<tool-use-id>toolu_bg</tool-use-id>\n<output-file>C:\\tmp\\x.output</output-file>\n<status>completed</status>\n<summary>Background agent done</summary>\n</task-notification>"}}"#;
        let items = parse_chat_items(line);
        let ChatItem::ToolResult {
            tool_use_id,
            text,
            is_error,
            subagent,
            ..
        } = &items[0]
        else {
            panic!("通知应转译成合成回执，实际：{items:?}");
        };
        assert_eq!(tool_use_id.as_deref(), Some("toolu_bg"));
        assert_eq!(text, "Background agent done");
        assert!(!is_error);
        let outcome = subagent.as_ref().expect("合成回执应带结局统计");
        assert_eq!((outcome.running, outcome.completed, outcome.failed), (0, 1, 0));

        // failed 变体:统计与错误位都翻转。
        let failed = line.replace("<status>completed</status>", "<status>failed</status>");
        let ChatItem::ToolResult { is_error, subagent, .. } = &parse_chat_items(&failed)[0] else {
            panic!("failed 通知也应转译成合成回执");
        };
        assert!(is_error);
        assert_eq!(subagent.as_ref().unwrap().failed, 1);

        // 缺 tool-use-id 关联不上 → 保持普通用户文本,宁可显示原文也不造孤儿回执。
        let orphan = r#"{"type":"user","uuid":"n2","message":{"content":"<task-notification>坏格式</task-notification>"}}"#;
        assert!(matches!(&parse_chat_items(orphan)[0], ChatItem::UserText { .. }));

        // 普通用户消息不受影响。
        let plain = r#"{"type":"user","uuid":"n3","message":{"content":"帮我看看这个"}}"#;
        assert!(matches!(&parse_chat_items(plain)[0], ChatItem::UserText { .. }));
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

    /// 主 agent 正忙时后台任务的完成通知走排队送入——不落 user 行,记成 queued_command
    /// attachment(实拍取证)。必须同款转译成合成回执:不认它,子任务徽标永远「运行中」,
    /// 通知原文还会以一坨 XML 出现在时间线里。
    #[test]
    fn queued_task_notification_becomes_synthetic_receipt() {
        let line = r#"{"type":"attachment","uuid":"qn1","timestamp":"2026-08-18T05:22:59Z","isSidechain":false,"attachment":{"type":"queued_command","prompt":"<task-notification>\n<task-id>a83e4da9cafca253c</task-id>\n<tool-use-id>toolu_q</tool-use-id>\n<status>completed</status>\n<summary>审查完成</summary>\n</task-notification>"}}"#;
        let items = parse_chat_items(line);
        let ChatItem::ToolResult { tool_use_id, subagent, .. } = &items[0] else {
            panic!("排队通知应转译成合成回执,实际:{items:?}");
        };
        assert_eq!(tool_use_id.as_deref(), Some("toolu_q"));
        assert_eq!(subagent.as_ref().unwrap().completed, 1);
    }

    /// busy_subagents:启动回执入集,三种结局信号(user 行通知/排队通知/TaskOutput 终态
    /// 回执)任一到达即出集。主回合 Stop 后它非零 = 后台还有活儿,状态判定据此不把会话
    /// 标成「等待中」。
    #[test]
    fn analyzer_tracks_busy_background_subagents() {
        let launch = |id: &str, agent: &str| format!(
            r#"{{"type":"user","uuid":"{id}","message":{{"content":[{{"type":"tool_result","tool_use_id":"t_{id}","content":[{{"type":"text","text":"Async agent launched successfully.\nagentId: {agent} (internal ID - do not mention to user.)"}}]}}]}}}}"#
        );
        // 两个后台委派在跑。
        let mut content = format!("{}\n{}\n", launch("l1", "aaa111"), launch("l2", "bbb222"));
        let p = write_tmp("busy_track", &content);
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 2);

        // user 行通知了结 aaa111。
        content.push_str(r#"{"type":"user","uuid":"n1","message":{"content":"<task-notification>\n<task-id>aaa111</task-id>\n<tool-use-id>t_l1</tool-use-id>\n<status>completed</status>\n<summary>done</summary>\n</task-notification>"}}"#);
        content.push('\n');
        std::fs::write(&p, &content).unwrap();
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 1);

        // 排队形态(enqueue)了结 bbb222——通知生成时结局已定,不必等送达。
        content.push_str(r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-08-18T05:22:59Z","content":"<task-notification>\n<task-id>bbb222</task-id>\n<tool-use-id>t_l2</tool-use-id>\n<status>completed</status>\n</task-notification>"}"#);
        content.push('\n');
        std::fs::write(&p, &content).unwrap();
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 0);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn analyzer_busy_settles_via_taskoutput_and_ignores_noise() {
        let mut content = String::from(
            r#"{"type":"user","uuid":"l1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"Async agent launched successfully.\nagentId: ccc333 (internal ID)"}]}]}}"#,
        );
        content.push('\n');
        // 噪声一:sidechain 里子任务自己的委派,不算主会话的后台工作。
        content.push_str(r#"{"type":"user","uuid":"sc1","isSidechain":true,"message":{"content":[{"type":"tool_result","tool_use_id":"t9","content":[{"type":"text","text":"Async agent launched successfully.\nagentId: zzz999 (internal ID)"}]}]}}"#);
        content.push('\n');
        // 噪声二:用户**引用**通知文案聊天(非开头),不得误消 ccc333。
        content.push_str(r#"{"type":"user","uuid":"quote","message":{"content":"我看到日志里有 <task-notification><task-id>ccc333</task-id></task-notification> 这样的行"}}"#);
        content.push('\n');
        // 噪声三:Read/Grep 源码或别的 transcript 的工具结果**引用**了启动回执文案
        // (行首是行号/注释,不是回执本身)——contains 判据会把 xxx 记成幽灵任务且
        // 永远等不到结局,会话恒挂「运行中」(2026-08-18 dogfooding 实拍)。
        content.push_str(r#"{"type":"user","uuid":"srcread","message":{"content":[{"type":"tool_result","tool_use_id":"tr1","content":[{"type":"text","text":"586  /// 启动:user 行里 `Async agent launched…agentId: xxx` 的工具回执。"}]}]}}"#);
        content.push('\n');
        let p = write_tmp("busy_taskout", &content);
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 1);

        // TaskOutput 的 running 拉取不了结;completed 拉取了结。
        content.push_str(r#"{"type":"user","uuid":"r1","message":{"content":[{"type":"tool_result","tool_use_id":"to1","content":"<retrieval_status>timeout</retrieval_status>\n<task_id>ccc333</task_id>\n<status>running</status>"}]}}"#);
        content.push('\n');
        std::fs::write(&p, &content).unwrap();
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 1);
        content.push_str(r#"{"type":"user","uuid":"r2","message":{"content":[{"type":"tool_result","tool_use_id":"to2","content":"<retrieval_status>success</retrieval_status>\n<task_id>ccc333</task_id>\n<status>completed</status>\n<output>done</output>"}]}}"#);
        content.push('\n');
        std::fs::write(&p, &content).unwrap();
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 0);
        std::fs::remove_file(&p).ok();
    }

    /// 后台 Bash(`run_in_background`)与 Agent 委派共用同一条后台任务通道:启动回执入集、
    /// `<task-notification>` 出集。2026-08-27 实拍的漏判——主回合停了、`gh run watch` 还在
    /// 后台跑,会话却报「等你输入」、子任务面板空白——根因就是启动侧只认 Agent 那一句。
    #[test]
    fn analyzer_tracks_background_shell() {
        // 真机回执原文(id 后紧跟句点,不是空格)。
        let mut content = String::from(
            r#"{"type":"user","uuid":"l1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Command running in background with ID: b78nfkj1v. Output is being written to: C:\tmp\tasks\b78nfkj1v.output. You will be notified when it completes."}]}}"#,
        );
        content.push('\n');
        // 噪声:Read/Grep 源码时**引用**这句话(行首是行号),行首锚定必须挡住——
        // 记成幽灵任务的话它永远等不到结局,会话恒挂「运行中」。
        content.push_str(r#"{"type":"user","uuid":"srcread","message":{"content":[{"type":"tool_result","tool_use_id":"t9","content":"1296  /// Command running in background with ID: ghost1. …"}]}}"#);
        content.push('\n');
        let p = write_tmp("busy_bg_shell", &content);
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 1);

        // 完成通知的 <task-id> 就是 shell id——结局侧无需改动即可了结。
        content.push_str(r#"{"type":"user","uuid":"n1","message":{"content":"<task-notification>\n<task-id>b78nfkj1v</task-id>\n<tool-use-id>t1</tool-use-id>\n<status>completed</status>\n<summary>Background command \"gh run watch\" completed (exit code 0)</summary>\n</task-notification>"}}"#);
        content.push('\n');
        std::fs::write(&p, &content).unwrap();
        assert_eq!(analyze_transcript(p.to_str().unwrap()).busy_subagents, 0);
        std::fs::remove_file(&p).ok();
    }

    /// 后台 Bash 的启动回执 → running 结局统计 + 任务 id(前端据此把这条 Bash 调用
    /// 显示成「后台运行中」,并让完成通知归回它)。
    #[test]
    fn background_shell_launch_receipt_is_running() {
        let outcome = CLAUDE_SUBAGENTS
            .detect_result("Command running in background with ID: bw3x4fnve. Output is being written to: /tmp/bw3x4fnve.output.")
            .expect("后台 Bash 启动回执应识别为在跑");
        assert_eq!(outcome.running, 1);
        assert_eq!(outcome.task_id.as_deref(), Some("bw3x4fnve"));
        // 引用(非行首)不认。
        assert!(CLAUDE_SUBAGENTS
            .detect_result("注释里写着 Command running in background with ID: ghost1.")
            .is_none());
    }

    /// forked skill 的回执识别。真机取证(CC 2.1.246,`/code-review` 后台审查):
    /// 主链只有 Skill 调用,fork 与否只写在回执里。
    #[test]
    fn forked_skill_receipt_maps_to_outcome() {
        let launched = forked_skill_outcome(
            "Skill \"code-review\" launched (forked execution, running in the background).\n\nRunning in the background as @code-review",
        )
        .expect("启动回执应认出");
        assert_eq!((launched.running, launched.completed), (1, 0));
        // forked skill 的回执不带任务 id:委派靠 tool_use_id 直连。
        assert_eq!(launched.task_id, None);

        let done = forked_skill_outcome("Skill \"code-review\" completed (forked execution).\n\nResult:\n…")
            .expect("完成回执应认出");
        assert_eq!((done.running, done.completed), (0, 1));

        let failed = forked_skill_outcome("Skill \"x\" failed (forked execution).")
            .expect("失败回执应认出");
        assert_eq!(failed.failed, 1);
    }

    /// 幽灵免疫:排查这套机制的会话(比如写这段代码的这次)里,Read/Grep 的工具结果会
    /// **引用**回执原文。contains 判据会把引用记成委派,而幽灵永远等不到结局——
    /// 会话从此恒挂「运行中」。与启动回执同一条行首锚定纪律。
    #[test]
    fn quoted_forked_receipt_is_not_a_delegation() {
        // 行首是行号/注释,回执文案在中段。
        assert!(forked_skill_outcome(
            "1278  /// 形如 Skill \"code-review\" launched (forked execution, running in the background)."
        )
        .is_none());
        // 普通 skill(未 fork)的回执不带 (forked execution),不认。
        assert!(forked_skill_outcome("Skill \"run\" completed.").is_none());
        // 没见过的动词不猜:宁可退回「无结局统计」,也不谎报成完成。
        assert!(forked_skill_outcome("Skill \"x\" queued (forked execution).").is_none());
    }

    /// Skill 调用的摘要 = 用户敲的那行命令。它同时是 forked skill 的外键(见 locate_forked),
    /// 形态必须与侧车 meta 的 description 严格一致。
    #[test]
    fn skill_summary_is_the_slash_command() {
        let input = serde_json::json!({"skill":"code-review","args":"1692 高强度"});
        assert_eq!(tool_summary("Skill", Some(&input)), "/code-review 1692 高强度");
        // 无 args / 空 args 只留命令本体。
        let bare = serde_json::json!({"skill":"run"});
        assert_eq!(tool_summary("Skill", Some(&bare)), "/run");
        let blank = serde_json::json!({"skill":"run","args":"   "});
        assert_eq!(tool_summary("Skill", Some(&blank)), "/run");
    }

    /// forked skill 的侧车定位:meta 没有 toolUseId,只能靠 description 配对;同一命令
    /// 跑多次则按**序号**配对(侧车先后以 meta 的写入时刻为准)。
    #[test]
    fn locate_forked_pairs_repeated_invocations_in_order() {
        let root = std::env::temp_dir().join(format!("cc_forked_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("sess").join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        // 主链:同一条命令跑两次,中间夹一次别的命令。
        let main = root.join("sess.jsonl");
        let mut content = String::new();
        for (id, args) in [("t_a", "1692 高强度"), ("t_b", "1719 高强度"), ("t_c", "1692 高强度")] {
            content.push_str(&format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Skill","input":{{"skill":"code-review","args":"{args}"}}}}]}}}}"#
            ));
            content.push('\n');
        }
        std::fs::write(&main, &content).unwrap();

        let write_meta = |stem: &str, desc: &str| {
            std::fs::write(
                dir.join(format!("{stem}.meta.json")),
                format!(
                    r#"{{"agentType":"general-purpose","description":"{desc}","name":"code-review","spawnDepth":1}}"#
                ),
            )
            .unwrap();
        };
        // spawn 标记(.forked-skill.json)才是出生时刻;sleep 拉开 mtime 免得排序不稳。
        let spawn = |stem: &str, desc: &str| {
            write_meta(stem, desc);
            std::fs::write(dir.join(format!("{stem}.jsonl")), "").unwrap();
            std::fs::write(
                dir.join(format!("{stem}.forked-skill.json")),
                r#"{"skillName":"code-review"}"#,
            )
            .unwrap();
        };
        spawn("agent-first", "/code-review 1692 高强度");
        std::thread::sleep(std::time::Duration::from_millis(20));
        spawn("agent-other", "/code-review 1719 高强度");
        std::thread::sleep(std::time::Duration::from_millis(20));
        spawn("agent-second", "/code-review 1692 高强度");
        // 关键:把先出生那个的 meta **重写**成最新(真机如此,实测差 487 秒)。若排序仍按
        // meta.json,下面 t_a/t_c 的断言会整个对调——这正是要防的「展开第一次看到第二次」。
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_meta("agent-first", "/code-review 1692 高强度");
        // 噪声:fork 内部再派的孙子**带** toolUseId,是 locate_one 的地盘,不得被截胡。
        std::fs::write(
            dir.join("agent-child.meta.json"),
            r#"{"agentType":"general-purpose","description":"/code-review 1692 高强度","toolUseId":"toolu_inner","parentAgentId":"first","spawnDepth":2}"#,
        )
        .unwrap();
        std::fs::write(dir.join("agent-child.jsonl"), "").unwrap();

        let pick = |id: &str| {
            ClaudeSubagents::locate_forked(&main, id)
                .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
        };
        // 第一次同款调用 → 第一个同款侧车;第二次 → 第二个(按 spawn 标记,不按 meta)。
        assert_eq!(pick("t_a").as_deref(), Some("agent-first"));
        assert_eq!(pick("t_c").as_deref(), Some("agent-second"));
        assert_eq!(pick("t_b").as_deref(), Some("agent-other"));
        // 主链上没有的 id 配不出东西。
        assert_eq!(pick("toolu_nope"), None);

        // 侧车少一个(内联完成不落盘/被清理)→ 同款的全部拒绝配对。给错记录比给不出更糟:
        // 若仍按序号取,t_a 会拿到 agent-second 那条流,用户对着不属于该次审查的记录做判断。
        for suffix in ["meta.json", "jsonl", "forked-skill.json"] {
            let _ = std::fs::remove_file(dir.join(format!("agent-first.{suffix}")));
        }
        assert_eq!(pick("t_a"), None, "数量对不上时不许配对");
        assert_eq!(pick("t_c"), None, "数量对不上时不许配对");
        // 另一条命令的配对不受牵连(序号与总数都按 desc 分别计)。
        assert_eq!(pick("t_b").as_deref(), Some("agent-other"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// forked skill 的完成通知**不带** `tool-use-id`,只有 `task-id`(真机取证 2026-08-26)。
    /// 此前这条通知被整条丢弃,后台 skill 的委派于是永远停在「运行中」——实拍:一条 11:36
    /// 就结束的审查,在会话剩下的三个半小时里一直挂着。
    #[test]
    fn task_notification_without_tool_use_id_routes_by_task_id() {
        let forked = task_notification_result(
            "<task-notification>\n<task-id>acb7f0cbde2eb991a</task-id>\n<status>failed</status>\n<summary>Agent 失败</summary>\n</task-notification>",
            "u1",
            None,
        )
        .expect("只有 task-id 的通知也要转成回执");
        let TranscriptEvent::ToolResult {
            tool_call_id,
            subagent,
            is_error,
            ..
        } = forked
        else {
            panic!("应是 ToolResult");
        };
        // 挂载点退回 task-id,并把它填进 task_id 交给前端按属主归回原委派。
        assert_eq!(tool_call_id.as_deref(), Some("acb7f0cbde2eb991a"));
        let outcome = subagent.expect("带结局统计");
        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.task_id.as_deref(), Some("acb7f0cbde2eb991a"));
        assert!(is_error);

        // 自带 tool-use-id 的(常规 Agent 委派)维持原语义:直连委派,不进 task_id 路由。
        let direct = task_notification_result(
            "<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>toolu_x</tool-use-id>\n<status>completed</status>\n</task-notification>",
            "u2",
            None,
        )
        .expect("常规通知照常");
        let TranscriptEvent::ToolResult {
            tool_call_id,
            subagent,
            ..
        } = direct
        else {
            panic!("应是 ToolResult");
        };
        assert_eq!(tool_call_id.as_deref(), Some("toolu_x"));
        assert_eq!(subagent.unwrap().task_id, None);

        // 两个 id 都没有 → 无从关联,按普通用户消息处理。
        assert!(task_notification_result(
            "<task-notification>\n<status>completed</status>\n</task-notification>",
            "u3",
            None,
        )
        .is_none());
    }

    /// 启动回执补 agentId:forked skill 的回执正文里没有任何 id,只有行上的
    /// `toolUseResult.agentId` 能把它与只带 task-id 的完成通知接起来。
    #[test]
    fn forked_launch_receipt_picks_up_agent_id_from_the_line() {
        let line = r#"{"type":"user","uuid":"u1","toolUseResult":{"status":"forked","background":true,"agentId":"acb7f0cbde2eb991a"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_skill","content":[{"type":"text","text":"Skill \"code-review\" launched (forked execution, running in the background)."}]}]}}"#;
        let events = parse_events(line, false);
        let Some(TranscriptEvent::ToolResult { subagent, .. }) = events.into_iter().next() else {
            panic!("应解析出一条 ToolResult");
        };
        let outcome = subagent.expect("启动回执带结局统计");
        assert_eq!(outcome.running, 1);
        assert_eq!(outcome.task_id.as_deref(), Some("acb7f0cbde2eb991a"));
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
