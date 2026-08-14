//! Transcript 抽象（provider 无关）：数据类型 + 解析器 trait + 增量缓存。
//!
//! 「怎么定位 transcript、怎么解析它」是 agent 的能力，故 trait 住在插件层而非 DB 层——
//! 此前这套代码寄生在 `meowo-store` 里，让「读一个 JSONL 文件」平白拖上了 rusqlite 依赖，
//! 也让 claude 专属的 `~/.claude/projects` 路径布局伪装成了通用的 store API。
//!
//! 具体格式由各 agent 插件实现；claude 同时解析标题，codex/kimi 仅提供结构化对话，标题仍走首条 prompt。

use serde::Serialize;
use std::path::{Path, PathBuf};

/// GUI 对话窗口消费的 provider 无关消息单元。终端 ANSI 只负责还原终端，结构化对话始终来自
/// agent 自己的 transcript，避免把光标移动、spinner 和重绘误当正文。
pub use meowo_protocol::ipc::{ChatItem, SubagentOutcome, SubagentRef, SubagentRun};

/// 一条待读取的子任务侧车流。
pub struct SubagentStream {
    /// 分支标签（kimi 的 `agent-3`）；单发委派可为 None。
    pub label: Option<String>,
    /// 归一化状态 `running` / `completed` / `failed`；provider 未留下信号时为 None。
    pub status: Option<String>,
    pub path: PathBuf,
}

/// Provider 私有日志解析后的领域事件。它刻意不依赖 GUI/IPC 的序列化形状：插件只描述
/// “发生了什么”，边界适配器再决定当前前端契约如何表达。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    UserMessage {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    AssistantMessage {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    AssistantChunk {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    /// 回合级错误（API Error 等系统插入文案）。与 AssistantMessage 分开：这不是模型说的
    /// 话，前端按错误气泡渲染（label 与看板卡片的 error_label 同源）。
    TurnError {
        id: String,
        timestamp: Option<String>,
        label: String,
        text: String,
    },
    Reasoning {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    ReasoningChunk {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    ToolCall {
        id: String,
        timestamp: Option<String>,
        name: String,
        summary: String,
        /// 该调用是一次子任务委派时的展示信息（见 [`SubagentSpec::detect_call`]）。
        subagent: Option<SubagentRef>,
    },
    ToolResult {
        id: String,
        timestamp: Option<String>,
        tool_call_id: Option<String>,
        text: String,
        is_error: bool,
        /// 该结果是子任务委派的回执时的结局统计（见 [`SubagentSpec::detect_result`]）。
        subagent: Option<SubagentOutcome>,
    },
    Metadata {
        id: String,
        timestamp: Option<String>,
        kind: String,
    },
}

impl From<TranscriptEvent> for ChatItem {
    fn from(event: TranscriptEvent) -> Self {
        match event {
            TranscriptEvent::UserMessage {
                id,
                timestamp,
                text,
            } => Self::UserText {
                id,
                timestamp,
                text,
            },
            TranscriptEvent::AssistantMessage {
                id,
                timestamp,
                text,
            } => Self::AssistantText {
                id,
                timestamp,
                text,
            },
            TranscriptEvent::AssistantChunk {
                id,
                timestamp,
                text,
            } => Self::AssistantDelta {
                id,
                timestamp,
                text,
            },
            TranscriptEvent::TurnError {
                id,
                timestamp,
                label,
                text,
            } => Self::TurnError {
                id,
                timestamp,
                label,
                text,
            },
            TranscriptEvent::Reasoning {
                id,
                timestamp,
                text,
            } => Self::Reasoning {
                id,
                timestamp,
                text,
            },
            TranscriptEvent::ReasoningChunk {
                id,
                timestamp,
                text,
            } => Self::ReasoningDelta {
                id,
                timestamp,
                text,
            },
            TranscriptEvent::ToolCall {
                id,
                timestamp,
                name,
                summary,
                subagent,
            } => Self::ToolUse {
                id,
                timestamp,
                name,
                summary,
                subagent,
            },
            TranscriptEvent::ToolResult {
                id,
                timestamp,
                tool_call_id,
                text,
                is_error,
                subagent,
            } => Self::ToolResult {
                id,
                timestamp,
                tool_use_id: tool_call_id,
                text,
                is_error,
                subagent,
            },
            TranscriptEvent::Metadata {
                id,
                timestamp,
                kind,
            } => Self::Meta {
                id,
                timestamp,
                kind,
            },
        }
    }
}

/// 一次增量读取结果。`offset` 只推进到最后一个完整换行，写到一半的 JSON 留给下一轮。
/// `start` 是本批 items 起始所对应的字节位置——首屏尾部加载时 > 0（前面还有历史未展示），
/// 供前端判断是否给出「加载更早」入口，并作为下一次向上翻页的上界。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatDelta {
    pub items: Vec<ChatItem>,
    /// 本批完整记录中各维度最后一次出现的模式值；普通增量可能为空。
    pub agent_modes: Vec<AgentMode>,
    pub start: u64,
    pub offset: u64,
    pub reset: bool,
    /// 本次读到的文件 mtime，调用方原样带回下一轮做「等长重写」检测。
    /// 光比长度不够：CLI 压缩/重写 transcript 后字节数可能与上次**完全相同**，
    /// 只看 len 会认为无变化而静默漏掉整段新内容。
    #[serde(skip)]
    pub mtime: Option<std::time::SystemTime>,
}

/// Provider transcript 中观测到的一个独立模式维度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentMode {
    pub dimension: String,
    pub value: String,
}

impl AgentMode {
    pub fn new(dimension: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            dimension: dimension.into(),
            value: value.into(),
        }
    }
}

/// 检测到的回合错误：短中文标签 + 原始文案 + 去重指纹。指纹取**标签**而非出错消息的
/// uuid——自动重试会连发多条同类错误（每条新 uuid），按 uuid 去重会通知轰炸
///（缘由详见 claude 插件 to_info 的注释）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnError {
    pub label: String,
    pub raw: String,
    pub fingerprint: String,
}

/// 单次扫 transcript 的产物：标题、错误与上下文已用量。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptInfo {
    pub title: Option<String>,
    pub error: Option<TurnError>,
    /// 最近一条带 usage 的 assistant 回合的「上下文已用 token 数」
    /// = input + cache_creation + cache_read + output。无 usage 时为 None。
    pub context_tokens: Option<u64>,
    /// 上下文已用百分比（相对 200k 标准窗口，封顶 100）。
    pub context_pct: Option<u8>,
    /// 最近一条 assistant 正文的轻推预览（合并空白、截断）——供卡片 hover 速览，
    /// 不切终端就能判断该会话在问什么/说了什么。无正文回合（纯 tool_use）时为 None。
    pub preview: Option<String>,
}

/// 增量解析单元：逐行 fold、按需产出 TranscriptInfo。
/// Send：TranscriptCache 经 Arc<Mutex<>> 在 Tauri 主线程与后台轮询线程间共享。
pub trait TranscriptParser: Send {
    fn fold_line(&mut self, line: &str);
    fn to_info(&self) -> TranscriptInfo;
}

/// 只提供 GUI 对话、不参与标题/错误/上下文分析的 transcript 共用空解析器。
pub(crate) struct ChatOnlyParser;

impl TranscriptParser for ChatOnlyParser {
    fn fold_line(&mut self, _line: &str) {}

    fn to_info(&self) -> TranscriptInfo {
        TranscriptInfo::default()
    }
}

/// 子任务（subagent）能力：agent 支持把工作委派给子 agent，并把子 agent 的过程记在
/// **主 transcript 之外的侧车流**里时声明它。
///
/// 为什么单列一个槽而不是在解析里加分支：两家的布局与关联方式毫无共同点——
/// claude 是 `<session>/subagents/agent-<id>.{jsonl,meta.json}`，靠 meta 里的 `toolUseId`
/// 做外键；kimi 是 `agents/agent-N/wire.jsonl`，得从主链 `tool.result` 的**输出正文**里
/// 抠出 `agent_id`。把这些塞进共享路径必然长成一排 `if provider ==`。
///
/// 侧车流按需读取（用户展开时），不进 [`read_chat_delta`] 的增量热路径：子任务往往几十条、
/// 一个会话几十个，跟着 650ms 轮询一起读会让长会话首开与稳态轮询都付出无谓代价。
pub trait SubagentSpec: Sync {
    /// 主链上这条工具调用是不是一次子任务委派？是则返回展示信息。
    fn detect_call(
        &self,
        tool_name: &str,
        input: Option<&serde_json::Value>,
    ) -> Option<SubagentRef>;

    /// 定位该次委派的侧车流。**返回列表**：一次调用未必只派一个子任务——kimi 的
    /// `AgentSwarm` 一次能派出十几个（fan-out 一批 items，或 resume 一批既有 agent）。
    /// `main_transcript` = 主 transcript 路径，`tool_use_id` = 主链那条工具调用的 id。
    /// 空列表 = 子任务尚未落盘、记录已清理，或这条根本不是委派。
    fn locate_streams(&self, main_transcript: &Path, tool_use_id: &str) -> Vec<SubagentStream>;

    /// 解析侧车流的一行。两家的侧车流都与主流同格式，但可能有额外前提
    /// （claude 的侧车行全部带 `isSidechain`，主流解析会主动丢弃它们）。
    fn parse_stream_line(&self, line: &str) -> Vec<TranscriptEvent>;

    /// 主链上这条**工具结果**是不是某次委派的回执？是则给出各分支的结局统计。
    ///
    /// 状态就写在主 transcript 里，解析时顺手取到即可——因此折叠状态下也能显示进度，
    /// 不必先展开（展开要读侧车流，那是按需 I/O，不该为了一个徽标付出）。
    /// 默认 None：该 provider 没有在主链留下可靠的结局信号。
    fn detect_result(&self, _output: &str) -> Option<SubagentOutcome> {
        None
    }
}

/// 某 agent 的 transcript 规格：定位文件 + 解析标题 + 产出增量解析器。
/// Sync：以 &'static dyn 共享。
pub trait TranscriptSpec: Sync {
    /// 新建一个该 agent 的增量解析器（供 TranscriptCache 在新建/重置条目时调用）。
    fn new_parser(&self) -> Box<dyn TranscriptParser>;
    /// 定位 transcript 文件（hook 路径 → cwd+id 重建 → 全局查找）。
    fn resolve_transcript_path(
        &self,
        transcript_path: Option<&str>,
        cwd: Option<&str>,
        session_id: &str,
    ) -> Option<PathBuf>;
    /// 解析会话标题（读不到/无标题返回 None）。
    fn resolve_title(
        &self,
        transcript_path: Option<&str>,
        cwd: Option<&str>,
        session_id: &str,
    ) -> Option<String>;

    /// 解析会话的真实工作目录——resume 必须在正确的项目目录下运行才找得到会话。
    ///
    /// 默认实现原样返回 DB 记录的 cwd。能从 transcript 内容读出权威 cwd 的 agent（claude）覆写它，
    /// 以纠正失真的 DB 记录（会话早于 hook 接线、SessionStart 丢失、项目目录事后被移动）。
    ///
    /// 此前这是 `meowo_store::title::resolve_cwd`：一个读 `~/.claude/projects` 的 claude 专属函数，
    /// 却被 app 当通用 API 对所有 agent 调用——非 claude 会话靠「全局找不到就回退 DB cwd」的巧合
    /// 拿到正确结果。现在这个回退就是默认实现本身。
    fn resolve_cwd(&self, cwd: Option<&str>, _session_id: &str) -> Option<String> {
        default_resolve_cwd(cwd)
    }

    /// 把一条完整 transcript JSONL 记录转成领域事件。默认不支持对话窗口。
    fn parse_transcript_line(&self, _line: &str) -> Vec<TranscriptEvent> {
        Vec::new()
    }

    /// 带 transcript 文件路径上下文的行解析：行内引用落在文件旁的资源时（kimi 把用户
    /// 粘贴的图片记成 blobref，正文在 wire.jsonl 旁的 `blobs/` 目录）需要覆写它。
    /// 默认忽略路径、退回 [`TranscriptSpec::parse_transcript_line`]。
    fn parse_transcript_line_in(&self, _path: &Path, line: &str) -> Vec<TranscriptEvent> {
        self.parse_transcript_line(line)
    }

    /// 从一条完整 transcript 记录中提取零到多个模式维度更新。默认不支持。
    fn agent_modes_from_line(&self, _line: &str) -> Vec<AgentMode> {
        Vec::new()
    }

    /// 从文件头开始读取时使用的初始模式。只声明协议有确定默认值的维度；后续记录会覆盖它。
    fn default_agent_modes(&self) -> Vec<AgentMode> {
        Vec::new()
    }

    /// 从 Agent 写入 transcript 的 runtime 能力清单发现斜杠命令。默认没有此类元数据；
    /// 实现必须只返回当前会话明确声明的项目，不能根据版本或名称猜测。
    fn supports_runtime_slash_commands(&self) -> bool {
        false
    }

    /// None 表示权威清单尚未出现；Some（包括空 Vec）表示已经观测到本会话的完整清单。
    fn runtime_slash_commands(&self, _path: &Path) -> Option<Vec<crate::chat_ui::SlashCommand>> {
        None
    }

    /// 此 transcript 是否提供结构化对话。与解析入口分开声明，避免用假数据探测能力。
    fn supports_chat(&self) -> bool {
        false
    }

    /// 首屏尾读时给「模式恢复」用的头部子串预筛标记：头部只有包含其一的行才值得 JSON 解析取模式，
    /// 其余整行跳过——`agent_modes_from_line` 对多数 agent 是一次完整 serde 解析，这是长会话首开的
    /// 关键省时点。默认空：无模式维度的 agent 头部零解析。
    ///
    /// **契约**：凡覆写 `agent_modes_from_line` 又想要首开模式恢复的 agent，这里必须覆盖所有可能
    /// 产出模式的行的判定子串（对 claude 即字段名 `permissionMode`）；留空则仅尾窗内的模式被识别。
    fn agent_mode_markers(&self) -> &[&'static str] {
        &[]
    }

    /// 是否需要主看板后台增量分析标题/错误/预览。仅聊天用的日志应返回 false，避免无意义热读大文件。
    fn supports_analysis(&self) -> bool {
        true
    }

    /// 子任务能力。None = 该 agent 没有子任务概念（codex 当前如此：工具只有 exec/wait，
    /// `task_started` 是回合级事件而非委派），或其子任务过程不落在可读的侧车流里。
    fn subagents(&self) -> Option<&'static dyn SubagentSpec> {
        None
    }
}

/// transcript 缓存失效判据，聊天与看板分析两条读取路径共用。
///
/// 两种情况必须丢弃 offset 从头重读，否则会静默漏掉整段内容：
/// - **截断/重建**：`len < offset`，文件比我们读过的还短。
/// - **等长重写**：`len == offset` 但 mtime 变了。压缩/改写后长度恰好不变时，
///   光比长度看不出任何变化，只有 mtime 会动。
///
/// `prev_mtime` 为 None 时不判等长重写：没有上一次的观测就无从谈「变了」，
/// 拿「有 mtime」去比「没 mtime」会把首次读取误判成重写，白白全量重读一次。
pub(crate) fn transcript_reset(
    len: u64,
    offset: u64,
    mtime: Option<std::time::SystemTime>,
    prev_mtime: Option<std::time::SystemTime>,
) -> bool {
    len < offset || (len == offset && prev_mtime.is_some() && mtime != prev_mtime)
}

/// 单个子任务侧车的读取上限。子任务本身就是「大工作拆出去」，个别会跑出很大的流；
/// 展开是一次同步 IPC，不能让一次点击拖住窗口。超限时读尾部——子任务的结论在末尾。
const SUBAGENT_STREAM_LIMIT: u64 = 4 * 1024 * 1024;

/// 读一条侧车流的全部消息。超限时读尾部——子任务的结论在末尾。
fn read_stream(subagents: &dyn SubagentSpec, path: &Path) -> Option<Vec<ChatItem>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let truncated = len > SUBAGENT_STREAM_LIMIT;
    if truncated {
        file.seek(SeekFrom::Start(len - SUBAGENT_STREAM_LIMIT))
            .ok()?;
    }
    // JSONL 行可能含非法 UTF-8（截断的多字节），lossy 读避免整份作废。
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    // 超限时从中间切入，首行多半是半条 JSON，丢掉。
    if truncated {
        lines.next();
    }
    Some(
        lines
            .flat_map(|line| subagents.parse_stream_line(line))
            .map(ChatItem::from)
            .collect(),
    )
}

/// 读取一次子任务委派的完整时间线。**按需调用**（用户展开时），不进增量热路径。
///
/// 与 [`read_chat_delta`] 的区别是刻意的：那条路径服务 650ms 轮询，必须增量且只读一个流；
/// 这里读的是已经写完的侧车流，整读一次即可，不需要 offset 记账。
///
/// 返回多条：一次委派未必只派一个子任务（kimi 的 `AgentSwarm`）。空列表表示该 agent
/// 没有子任务能力，或这条调用找不到任何侧车流（尚未落盘、已被清理、或根本不是委派）。
pub fn read_subagent_chat(
    spec: &dyn TranscriptSpec,
    main_transcript: &Path,
    tool_use_id: &str,
) -> Vec<SubagentRun> {
    let Some(subagents) = spec.subagents() else {
        return Vec::new();
    };
    subagents
        .locate_streams(main_transcript, tool_use_id)
        .into_iter()
        .filter_map(|stream| {
            Some(SubagentRun {
                label: stream.label,
                status: stream.status,
                items: read_stream(subagents, &stream.path)?,
            })
        })
        .collect()
}

/// 从 `offset` 起只解析新增的完整 JSONL 行。文件被截断/重建时自动从头开始并标记 reset，前端据此
/// 清空旧消息。读取失败原样返回 offset，短暂的文件锁不会让界面丢历史。
pub fn read_chat_delta(
    spec: &dyn TranscriptSpec,
    path: &Path,
    offset: u64,
    prev_mtime: Option<std::time::SystemTime>,
) -> ChatDelta {
    // 文件 IO 与失效判据复用 read_transcript_delta（与看板分析同一份）。
    // NoChange 不早退：空文件首读（offset=0）也要走到下方 base==0 分支下发默认模式。
    let (reset, buf, mtime) = match read_transcript_delta(path, offset, prev_mtime) {
        DeltaOutcome::Unreadable => {
            return ChatDelta {
                items: Vec::new(),
                agent_modes: Vec::new(),
                start: offset,
                offset,
                reset: false,
                mtime: prev_mtime,
            };
        }
        DeltaOutcome::NoChange(mtime) => (false, Vec::new(), mtime),
        DeltaOutcome::Data { reset, buf, mtime } => (reset, buf, mtime),
    };
    let base = if reset { 0 } else { offset };
    let consumed = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let text = String::from_utf8_lossy(&buf[..consumed]);
    let mut items = Vec::new();
    let mut agent_modes = if base == 0 {
        spec.default_agent_modes()
    } else {
        Vec::new()
    };
    for line in text.lines() {
        for mode in spec.agent_modes_from_line(line) {
            if let Some(previous) = agent_modes
                .iter_mut()
                .find(|previous| previous.dimension == mode.dimension)
            {
                *previous = mode;
            } else {
                agent_modes.push(mode);
            }
        }
        items.extend(
            spec.parse_transcript_line_in(path, line)
                .into_iter()
                .map(ChatItem::from),
        );
    }
    ChatDelta {
        items,
        agent_modes,
        start: base,
        offset: base + consumed as u64,
        reset,
        mtime,
    }
}

/// 无 transcript 规格的 agent（codex/kimi）以及 `TranscriptSpec::resolve_cwd` 默认实现共用：
/// 直接采信 DB 记录的 cwd，空白视作没有。
pub fn default_resolve_cwd(cwd: Option<&str>) -> Option<String> {
    cwd.filter(|c| !c.trim().is_empty()).map(str::to_string)
}

/// 单条缓存：已解析到的字节偏移 + 上次解析时的 mtime + 累积解析器 + 最近使用刻度（淘汰用）。
struct CacheEntry {
    offset: u64,
    mtime: Option<std::time::SystemTime>,
    parser: Box<dyn TranscriptParser>,
    last_used: u64,
}

/// transcript 增量解析缓存：transcript 是只追加的 JSONL，没必要每轮把整文件重读重解析
/// （几十 MB → 数百 ms，多个会话叠加可达数秒，每 ~300ms 一次会打满 CPU、拖慢整窗）。
/// 这里按文件路径缓存「已解析到的字节偏移 + 累积状态」，每轮只读+解析新追加的完整行，
/// 把每次刷新从 O(整文件) 降到 O(新增字节) ≈ 接近 0。
#[derive(Default)]
pub struct TranscriptCache {
    entries: std::collections::HashMap<String, CacheEntry>,
    tick: u64, // 单调递增的访问刻度，供 LRU 淘汰
}

/// 缓存条目上限：超出时淘汰最久未访问的条目，防长期运行无界增长。
///
/// 上限必须显著大于一次列表补页可能触碰的 transcript 数（session_query 的扫描上限
/// 是每请求千行级）：条目被挤掉即偏移归零，下次访问整文件重读重解析——一次超上限的
/// 扫描会把**活跃会话**的条目全部轮转出去，之后每 ~300ms 的刷新都退化成 O(整文件)，
/// 恰是本缓存要防的场景。单条目只存偏移与累积摘要（标题/预览等短字符串），放大到
/// 1024 的内存代价可忽略。
const MAX_CACHE_ENTRIES: usize = 1024;

/// read_transcript_delta 的结果：analyze 与 analyze_shared 共用的文件 IO 段。
enum DeltaOutcome {
    /// 打开/metadata/seek/读取失败：沿用已累积状态（不要用 len=0 当真实长度误判截断）。
    Unreadable,
    /// 无新增字节：仅需刷新 mtime。
    NoChange(Option<std::time::SystemTime>),
    /// 读到了新增（或需从头重读）的字节。
    Data {
        reset: bool,
        buf: Vec<u8>,
        mtime: Option<std::time::SystemTime>,
    },
}

/// 从 offset/prev_mtime 快照出发读取 transcript 的增量字节。纯文件 IO、不触碰缓存，
/// 供 analyze（持锁调用）/ analyze_shared（锁外调用）/ [`read_chat_delta`]（对话窗）共用。
/// 失效判据见 [`transcript_reset`]。
fn read_transcript_delta(
    path: impl AsRef<Path>,
    offset: u64,
    prev_mtime: Option<std::time::SystemTime>,
) -> DeltaOutcome {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return DeltaOutcome::Unreadable;
    };
    let (len, mtime) = match f.metadata() {
        Ok(m) => (m.len(), m.modified().ok()),
        Err(_) => return DeltaOutcome::Unreadable,
    };
    let reset = transcript_reset(len, offset, mtime, prev_mtime);
    if !reset && len == offset {
        return DeltaOutcome::NoChange(mtime);
    }
    let base = if reset { 0 } else { offset };
    if f.seek(SeekFrom::Start(base)).is_err() {
        return DeltaOutcome::Unreadable;
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return DeltaOutcome::Unreadable;
    }
    DeltaOutcome::Data { reset, buf, mtime }
}

impl TranscriptCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 增量解析 path：只处理上次偏移之后新追加的「完整行」（末尾未结束的半行留到下次）。
    /// 失效检测用 len + mtime 双重校验：len < 偏移（截断）或 len == 偏移但 mtime 变了
    /// （等长重写）→ 从头重解析。打开/读失败 → 返回当前累积结果。
    /// `spec` 决定新建/重置条目时用哪种 agent 的解析器。
    /// 与 analyze_shared 共用 snapshot/read_transcript_delta/commit 三段（单一事实源），
    /// 差别仅在本方法独占 &mut self、无并发窗口。
    pub fn analyze(&mut self, spec: &dyn TranscriptSpec, path: &str) -> TranscriptInfo {
        let (offset, prev_mtime) = self.snapshot(spec, path);
        match read_transcript_delta(path, offset, prev_mtime) {
            DeltaOutcome::Unreadable => self.current_info(path),
            DeltaOutcome::NoChange(mtime) => self.touch_mtime(path, mtime),
            DeltaOutcome::Data { reset, buf, mtime } => {
                self.commit(spec, path, offset, reset, &buf, mtime)
            }
        }
    }

    /// 与 `analyze` 等价，但供多线程经 `Mutex` 共享缓存时调用：文件 IO（open/metadata/读新增字节）
    /// 全部在锁外进行，只有「取快照」与「提交结果」两个短临界区持锁——避免大 transcript 首读
    /// （数 MB、数百 ms）期间把其它调用方（如 get_live_sessions）一并阻塞在缓存锁上。
    /// 两个线程并发分析同一文件时可能重复读取，但提交前校验偏移快照，只有一方生效，状态不会错乱。
    pub fn analyze_shared(
        cache: &std::sync::Mutex<TranscriptCache>,
        spec: &dyn TranscriptSpec,
        path: &str,
    ) -> TranscriptInfo {
        let lock = || cache.lock().unwrap_or_else(|e| e.into_inner());
        // 短临界区 1：确保条目存在，取（已解析偏移, 上次 mtime）快照。
        let (offset, prev_mtime) = lock().snapshot(spec, path);
        // 锁外做全部文件 IO。失败时与 analyze 同语义：返回当前累积结果。
        match read_transcript_delta(path, offset, prev_mtime) {
            DeltaOutcome::Unreadable => lock().current_info(path),
            DeltaOutcome::NoChange(mtime) => lock().touch_mtime(path, mtime),
            // 短临界区 2：偏移仍与快照一致才提交；其它线程已推进则弃用本次读取、复用其结果。
            DeltaOutcome::Data { reset, buf, mtime } => {
                lock().commit(spec, path, offset, reset, &buf, mtime)
            }
        }
    }

    /// analyze / analyze_shared 临界区 1：确保条目存在（含 LRU 淘汰），返回（偏移, mtime）快照。不做文件 IO。
    fn snapshot(
        &mut self,
        spec: &dyn TranscriptSpec,
        path: &str,
    ) -> (u64, Option<std::time::SystemTime>) {
        self.tick += 1;
        if !self.entries.contains_key(path) && self.entries.len() >= MAX_CACHE_ENTRIES {
            if let Some(k) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&k);
            }
        }
        let tick = self.tick;
        let entry = self
            .entries
            .entry(path.to_string())
            .or_insert_with(|| CacheEntry {
                offset: 0,
                mtime: None,
                parser: spec.new_parser(),
                last_used: tick,
            });
        entry.last_used = tick;
        (entry.offset, entry.mtime)
    }

    /// 当前累积结果；条目不存在（锁外窗口内被 LRU 淘汰）时返回空结果。
    fn current_info(&mut self, path: &str) -> TranscriptInfo {
        self.entries
            .get(path)
            .map(|e| e.parser.to_info())
            .unwrap_or_default()
    }

    /// 无新增时刷新 mtime 并返回累积结果。
    fn touch_mtime(&mut self, path: &str, mtime: Option<std::time::SystemTime>) -> TranscriptInfo {
        match self.entries.get_mut(path) {
            Some(e) => {
                e.mtime = mtime;
                e.parser.to_info()
            }
            None => TranscriptInfo::default(),
        }
    }

    /// analyze / analyze_shared 临界区 2：把读到的字节合并进缓存。仅当条目偏移仍等于快照偏移
    /// （期间无其它线程推进）时生效；否则弃用本次读取，直接返回已有结果。
    fn commit(
        &mut self,
        spec: &dyn TranscriptSpec,
        path: &str,
        snap_offset: u64,
        reset: bool,
        buf: &[u8],
        mtime: Option<std::time::SystemTime>,
    ) -> TranscriptInfo {
        self.tick += 1;
        let tick = self.tick;
        // buf 是否从文件头读起（reset 或条目本就是新建的 0 偏移）——只有这种读取才能安全灌入全新条目。
        let from_zero = reset || snap_offset == 0;
        let entry = match self.entries.get_mut(path) {
            Some(e) => e,
            None => {
                // 条目在锁外窗口被 LRU 淘汰：从头读的可重建灌入；增量读的丢弃，下轮重来。
                if !from_zero {
                    return TranscriptInfo::default();
                }
                self.entries.insert(
                    path.to_string(),
                    CacheEntry {
                        offset: 0,
                        mtime: None,
                        parser: spec.new_parser(),
                        last_used: tick,
                    },
                );
                self.entries
                    .get_mut(path)
                    .expect("刚插入的缓存条目必然存在")
            }
        };
        entry.last_used = tick;
        if entry.offset != snap_offset {
            return entry.parser.to_info();
        }
        if reset {
            entry.offset = 0;
            entry.parser = spec.new_parser();
        }
        if let Some(nl) = buf.iter().rposition(|&b| b == b'\n') {
            entry.offset += (nl + 1) as u64;
            let chunk = String::from_utf8_lossy(&buf[..=nl]);
            for line in chunk.lines() {
                entry.parser.fold_line(line);
            }
        }
        entry.mtime = mtime;
        entry.parser.to_info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 缓存的增量/失效语义要用一个真实解析器才测得出来（断言标题随追加变化），故借 claude 的 spec。
    use crate::plugins::claude::transcript::{analyze_transcript, ClaudeTranscript};

    fn write_tmp(name: &str, content: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("meowo_cache_{}_{}.jsonl", std::process::id(), name));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn transcript_events_map_to_the_stable_ipc_contract() {
        let timestamp = Some("2026-07-19T10:00:00Z".to_string());
        assert_eq!(
            ChatItem::from(TranscriptEvent::AssistantChunk {
                id: "a".into(),
                timestamp: timestamp.clone(),
                text: "delta".into(),
            }),
            ChatItem::AssistantDelta {
                id: "a".into(),
                timestamp: timestamp.clone(),
                text: "delta".into(),
            }
        );
        assert_eq!(
            ChatItem::from(TranscriptEvent::ToolResult {
                id: "result".into(),
                timestamp: timestamp.clone(),
                tool_call_id: Some("call".into()),
                text: "ok".into(),
                is_error: false,
                subagent: None,
            }),
            ChatItem::ToolResult {
                id: "result".into(),
                timestamp: timestamp.clone(),
                tool_use_id: Some("call".into()),
                text: "ok".into(),
                is_error: false,
                subagent: None,
            }
        );
        assert_eq!(
            ChatItem::from(TranscriptEvent::Metadata {
                id: "compact".into(),
                timestamp,
                kind: "compact".into(),
            }),
            ChatItem::Meta {
                id: "compact".into(),
                timestamp: Some("2026-07-19T10:00:00Z".into()),
                kind: "compact".into(),
            }
        );
    }

    /// 失效判据的四种边界。两条读取路径共用它，跑偏一处就是整段内容静默丢失。
    #[test]
    fn transcript_reset_covers_truncation_and_same_length_rewrite() {
        use super::transcript_reset;
        let t1 = std::time::SystemTime::UNIX_EPOCH;
        let t2 = t1 + std::time::Duration::from_secs(1);
        // 正常追加：长度变长，不重读。
        assert!(!transcript_reset(200, 100, Some(t2), Some(t1)));
        // 截断/重建：文件比读过的还短。
        assert!(transcript_reset(50, 100, Some(t2), Some(t1)));
        // 等长重写：长度没变但 mtime 动了。
        assert!(transcript_reset(100, 100, Some(t2), Some(t1)));
        // 等长且 mtime 未变：真的没动过。
        assert!(!transcript_reset(100, 100, Some(t1), Some(t1)));
        // 首次读取（无历史 mtime）：无从判断「变了」，不得误判成重写。
        assert!(!transcript_reset(0, 0, Some(t1), None));
    }

    /// 等长重写必须触发 reset 重读。CLI 压缩/改写 transcript 后字节数可能与上次**完全相同**，
    /// 只比 offset 与文件长度会认为「无变化」，于是整段新内容被静默漏掉——对话窗口表现为
    /// 消息突然不再更新。mtime 是唯一能区分这种情况的信号。
    #[test]
    fn chat_delta_detects_same_length_rewrite() {
        let line_a = format!(
            "{}\n",
            r#"{"type":"user","message":{"role":"user","content":"AAA"}}"#
        );
        let p = write_tmp("chat_rewrite", &line_a);

        let first = read_chat_delta(&ClaudeTranscript, &p, 0, None);
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.offset, line_a.len() as u64);
        assert!(first.mtime.is_some());

        // 同样长度、不同内容地整体重写。mtime 必须比上一次新，否则这个测试本身无意义。
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let line_b = format!(
            "{}\n",
            r#"{"type":"user","message":{"role":"user","content":"BBB"}}"#
        );
        assert_eq!(line_a.len(), line_b.len(), "两行必须等长，否则测不到该分支");
        std::fs::write(&p, &line_b).unwrap();

        let second = read_chat_delta(&ClaudeTranscript, &p, first.offset, first.mtime);
        assert!(second.reset, "等长重写必须 reset，否则新内容会被漏掉");
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.start, 0, "reset 时从头读");

        // 对照：mtime 也没变（真正无变化）时不该 reset，避免每轮白读整个文件。
        let third = read_chat_delta(&ClaudeTranscript, &p, second.offset, second.mtime);
        assert!(!third.reset);
        assert!(third.items.is_empty());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn cache_incremental_matches_full_and_picks_up_appends() {
        use std::io::Write;
        let p = write_tmp(
            "cache_inc",
            concat!(
                r#"{"type":"ai-title","aiTitle":"标题A"}"#,
                "\n",
                r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","usage":{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0},"content":[{"type":"text","text":"hi"}]}}"#,
                "\n",
            ),
        );
        let mut cache = TranscriptCache::new();
        let i1 = cache.analyze(&ClaudeTranscript, p.to_str().unwrap());
        assert_eq!(i1.title.as_deref(), Some("标题A"));
        assert_eq!(i1.context_tokens, Some(1000));

        // 追加新一轮（带更大 usage + 自定义标题），增量解析应读到。
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, r#"{{"type":"custom-title","customTitle":"标题B"}}"#).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"u2","message":{{"role":"assistant","usage":{{"input_tokens":40000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}},"content":[{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#
        )
        .unwrap();
        drop(f);

        let i2 = cache.analyze(&ClaudeTranscript, p.to_str().unwrap());
        // 与全量解析结果一致
        let full = analyze_transcript(p.to_str().unwrap());
        assert_eq!(i2.title.as_deref(), Some("标题B")); // custom 覆盖 ai
        assert_eq!(i2.context_tokens, Some(40000));
        assert_eq!(i2.title, full.title);
        assert_eq!(i2.context_tokens, full.context_tokens);

        // 再次调用、无新增 → 结果稳定。
        let i3 = cache.analyze(&ClaudeTranscript, p.to_str().unwrap());
        assert_eq!(i3.context_tokens, Some(40000));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn cache_detects_same_length_rewrite_by_mtime() {
        // 等长重写：len 不变但 mtime 变了 → 应从头重解析，而不是沿用旧状态。
        let line_a = r#"{"type":"ai-title","aiTitle":"AAAA"}"#;
        let line_b = r#"{"type":"ai-title","aiTitle":"BBBB"}"#;
        assert_eq!(line_a.len(), line_b.len());
        let p = write_tmp("cache_rewrite", &format!("{line_a}\n"));
        let mut cache = TranscriptCache::new();
        assert_eq!(
            cache
                .analyze(&ClaudeTranscript, p.to_str().unwrap())
                .title
                .as_deref(),
            Some("AAAA")
        );

        // 等长重写，循环到 mtime 确认变化为止（兼容粗粒度文件系统，NTFS/APFS 首轮即过）。
        let mtime0 = std::fs::metadata(&p).unwrap().modified().unwrap();
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            std::fs::write(&p, format!("{line_b}\n")).unwrap();
            if std::fs::metadata(&p).unwrap().modified().unwrap() != mtime0 {
                break;
            }
        }
        assert_ne!(
            std::fs::metadata(&p).unwrap().modified().unwrap(),
            mtime0,
            "mtime 未变化，无法验证缓存失效"
        );
        assert_eq!(
            cache
                .analyze(&ClaudeTranscript, p.to_str().unwrap())
                .title
                .as_deref(),
            Some("BBBB")
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn analyze_shared_matches_analyze_on_append_truncate_and_missing() {
        // 锁外 IO 版必须与 analyze 语义一致：首读、追加增量、截断重读、文件缺失四种路径。
        use std::io::Write;
        use std::sync::Mutex;
        let spec = &ClaudeTranscript;
        let p = write_tmp(
            "cache_shared",
            concat!(r#"{"type":"ai-title","aiTitle":"标题A"}"#, "\n"),
        );
        let path = p.to_str().unwrap();
        let cache = Mutex::new(TranscriptCache::new());

        // 首读
        let i1 = TranscriptCache::analyze_shared(&cache, spec, path);
        assert_eq!(i1.title.as_deref(), Some("标题A"));

        // 追加 → 增量读到
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, r#"{{"type":"custom-title","customTitle":"标题B"}}"#).unwrap();
        drop(f);
        let i2 = TranscriptCache::analyze_shared(&cache, spec, path);
        assert_eq!(i2.title.as_deref(), Some("标题B"));

        // 无新增 → 结果稳定
        let i3 = TranscriptCache::analyze_shared(&cache, spec, path);
        assert_eq!(i3.title.as_deref(), Some("标题B"));

        // 截断成更短内容 → 从头重解析
        std::fs::write(&p, concat!(r#"{"type":"ai-title","aiTitle":"C"}"#, "\n")).unwrap();
        let i4 = TranscriptCache::analyze_shared(&cache, spec, path);
        assert_eq!(i4.title.as_deref(), Some("C"));

        // 文件消失 → 沿用已累积结果（与 analyze 一致）
        std::fs::remove_file(&p).ok();
        let i5 = TranscriptCache::analyze_shared(&cache, spec, path);
        assert_eq!(i5.title.as_deref(), Some("C"));
    }
}
