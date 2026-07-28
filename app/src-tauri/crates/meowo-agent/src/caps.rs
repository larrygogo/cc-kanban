//! 能力槽。meowo 提供全部能力位，agent 只声明自己有的那些；不声明的由框架降级。
//!
//! 这与「每个 agent 实现十几个方法、其中一半返回 `false`/`None`」的形态不同：`writes_tab_token()`
//! 返回 `false`、`transcript()` 返回 `None`、`usage_supported()` 返回 `false` 这类「我没有这个能力」
//! 的表达，统一成能力查询返回 `None`。codex 不支持重命名回写，就不实现那个方法；kimi 不读
//! transcript，就不提供 `TranscriptSpec`。
//!
//! 能力方法**不接** reporter 的 `HookEvent`——那个类型依赖 `meowo_store::TodoInput`，让插件层
//! 反向依赖 DB 层。改为传只含所需字段的 [`HookContext`]，能力看到的正是它需要的。

use crate::transcript::TranscriptSpec;

/// hook 事件里被 agent 能力用到的那几个字段。由调用方（reporter 的 dispatch）从 hook 负载构造。
#[derive(Debug, Default, Clone, Copy)]
pub struct HookContext<'a> {
    pub session_id: &'a str,
    /// hook 携带的 transcript 路径（codex 的 rollout / claude 的 jsonl）。缺失时能力自行兜底查找。
    pub transcript_path: Option<&'a str>,
    /// hook 携带的最近一条 AI 正文（claude / codex 带，kimi 不带）。
    pub last_assistant_message: Option<&'a str>,
}

/// Stop 时要落库的输出：最近一条 AI 正文 + 模型展示名。
#[derive(Debug, Default, PartialEq)]
pub struct StopOutputs {
    pub last_ai: Option<String>,
    pub model: Option<String>,
}

/// 会话上下文占用快照。
#[derive(Debug, Default, PartialEq)]
pub struct ContextUsage {
    /// 已用百分比（0–100，已 clamp）。
    pub used_pct: i64,
    /// 上下文窗口大小（token）。
    pub window: i64,
    /// 模型展示名（usage 记录里顺带能读到的 agent 才填,如 kimi）。None = 该通道不知道模型,
    /// 落库时不覆盖已有值。没有它,kimi 的模型要等第一次 Stop 才出现——新会话第一回合
    /// 跑得再久,卡片上也一直没有模型。
    pub model: Option<String>,
}

/// 一条待办的原始快照。`status` 保留 agent **自己写的词**（claude 是 `completed`、
/// kimi 是 `done`），归一化交给 DB 层的 `TodoStatus::from_str`——插件层不依赖 store，
/// 也不该替它决定枚举取值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoSnapshot {
    pub content: String,
    pub status: String,
}

/// 一个会话的运行形态：**谁在托管这个会话的进程**。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionRuntime {
    /// 由 agent 自己的后台守护进程托管，既不在用户的终端里，也不在 meowo 的托管 PTY 里
    /// （claude 的 FleetView 后台会话，见 [`crate::plugins::claude::fleet`]）。
    ///
    /// 这类会话照常触发 hook、照常建卡，但 meowo 的「接管」（杀进程 + `--resume` 重开）对它
    /// 无效：agent 的 supervisor 会把进程拉回来。前端据此收起接管/结束这些注定失败的入口。
    pub background: bool,
    /// 后台作业 id（claude 的 `jobId`），供界面标注是哪一个作业。非后台会话为 None。
    pub job_id: Option<String>,
    /// agent 自己记的作业状态（claude: `done` / `working` / `blocked`）。
    ///
    /// 看板原有的「运行中 / 等你输入」是按进程存活与 hook 时间推出来的，对后台会话一律
    /// 只能得出「运行中」——一屏后台卡片状态全一样，用户没法从中分辨哪个还在干活。
    /// 这是 agent 自己的说法，直接用。
    pub job_state: Option<String>,
    /// 这个后台会话是用户那条**交互式对话转入后台**来的，而非凭空派出的新任务。
    ///
    /// claude 在 FleetView 里按 ← 进 agents 模式就会这样 fork 一份：新 session id、新卡片，
    /// 历史却是从源会话复制的，连终端画面都和源会话一模一样。不标出来的话，用户看到的
    /// 就是看板上凭空多出一张与已有卡片长得完全相同的卡。
    pub from_interactive: bool,
    /// agent **预热**出来的空 worker，还没被派上任何活（claude 的 `dispatch.source: "spare"`）。
    ///
    /// 进 agents 模式时 claude 会先备一个待命进程，好让下一次派活立刻就能开跑。它照样有
    /// session id、照样触发 hook，于是看板上多出一张永远没内容的「(未命名会话)」。
    /// 被派上活之后这个标记自然消失。
    pub spare: bool,
    /// 这个后台作业是从哪个会话 fork 出来的（源会话的 id）。
    ///
    /// 后台会话不单独占卡片（用户看不懂凭空多出来的卡，也几乎操作不了它们），改为折叠到
    /// 源会话卡上标一个数。只有 fork 型作业查得到源——花名册的 `dispatch.launch` 里带着
    /// 源 transcript 的路径。而且花名册是**活**记录：worker 退出后条目消失，计数跟着归零，
    /// 正合适（已经结束的作业不该继续挂在父卡片上）。
    pub forked_from: Option<String>,
}

/// 后台会话的**旁路接入点**：agent 自己为托管会话留的那条 socket。
///
/// claude 的 FleetView 正是靠它把后台会话的画面接回前台的（`roster.json` 的 `ptySock` /
/// `ptyAuth`）。有了它，meowo 不必跟 supervisor 抢进程就能看画面、改尺寸、结束会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundEndpoint {
    /// Windows 是命名管道路径，其余平台是 unix socket 路径。
    pub sock: String,
    /// 输入门控令牌。None = 该 worker 没设（服务端此时对输入 fail-open）。
    pub auth: Option<String>,
    /// PTY 宿主进程 pid，供调用方判活。
    pub pid: Option<i64>,
    /// 向这个会话**发消息**的通道。与 PTY 旁路刻意分开：往 PTY 写按键对后台 worker 无效
    /// （它不消费 stdin），送话要经它的守护进程转交。
    pub control: Option<BackgroundControl>,
}

/// agent 守护进程的控制通道：一句话投递给指定作业。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundControl {
    /// 控制 socket（claude: `\\.\pipe\cc-daemon-<pipe.key>-control`）。
    pub sock: String,
    /// 控制令牌（claude: `daemon/control.key` 的内容）。
    pub auth: String,
    /// 作业短 id（claude: roster 的 key / `jobId`），指明这句话投给谁。
    pub job: String,
}

/// 会话运行形态能力：回答「这些 session id 各自由谁托管」。
///
/// 与「进程是否活着」（宿主查进程表）刻意分开：那问的是此刻，这问的是出身——后台会话的进程
/// 死掉之后索引仍在，卡片该有的标注不随之消失。
pub trait RuntimeCap: Sync {
    /// 全量扫出「session id → 运行形态」。实现按 agent 自己的落盘索引来，调用方负责缓存
    /// （宿主在会话列表的热路径上带 TTL，见 meowo-app 的 `SessionRuntimeCache`）。
    fn session_runtimes(&self) -> std::collections::HashMap<String, SessionRuntime>;

    /// 该后台会话的旁路接入点。None = 不是后台会话、agent 不提供这条路，或 worker 已退出
    /// （花名册里查无此人）。调用方据此决定能不能接上去看画面。
    fn background_endpoint(&self, _session_id: &str) -> Option<BackgroundEndpoint> {
        None
    }
}

/// 会话遥测能力：从 hook 负载或该 agent 的会话文件里取出「正文 / 模型 / 上下文占用 / 标题」，
/// 以及把重命名写回 agent 自己的持久层。
///
/// 全部方法都有默认实现——一个 agent 只覆写它真正支持的那些。
pub trait TelemetryCap: Sync {
    /// Stop 时取最近 AI 正文 + 模型。claude 用 hook 携带的正文（模型走 statusline）；
    /// codex 正文走 hook、模型读 rollout；kimi 两者都从 wire.jsonl 一次读出。
    fn stop_outputs(&self, _ctx: &HookContext) -> StopOutputs {
        StopOutputs::default()
    }

    /// 从会话日志读最近一次上下文占用。claude 返回 None（走 statusline）。
    fn read_context(&self, _ctx: &HookContext) -> Option<ContextUsage> {
        None
    }

    /// 从会话日志读**当前的待办快照**。
    ///
    /// 与 hook 路径互补：hook 只在 meowo 在场时捕获得到，而会话日志是 agent 自己一直在写的。
    /// 有了它，「中途才启动 meowo」「hook 曾漏接」「早先解析有误」这几种情况都能按需重建，
    /// 不必干等 agent 下一次调用待办工具。None = 该 agent 的日志里读不到待办。
    fn read_todos(&self, _ctx: &HookContext) -> Option<Vec<TodoSnapshot>> {
        None
    }

    /// 该 agent 的 transcript 规格：提供「定位 + 标题解析 + 增量分析」。
    /// codex/kimi 的 spec 只供结构化对话；标题仍走首条 prompt、预览/模型走 stop_outputs。
    fn transcript(&self) -> Option<&'static dyn TranscriptSpec> {
        None
    }

    /// 是否由 transcript 解析标题。与 [`transcript`](Self::transcript) 刻意分开：可以有
    /// 「提供了 transcript 规格（供预览/上下文分析）但标题另有来源」的 agent。
    fn resolves_transcript_title(&self) -> bool {
        false
    }

    /// 把重命名同步到该 agent 自己的持久层，使它自身的会话列表/恢复列表也显示新名字。
    /// 返回是否成功落地（失败不阻断调用方更新 DB 标题）。默认不支持。
    fn write_rename(&self, _session_id: &str, _cwd: Option<&str>, _title: &str) -> bool {
        false
    }
}
