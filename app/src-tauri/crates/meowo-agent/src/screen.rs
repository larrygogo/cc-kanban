//! 屏幕状态检测的**规则声明**：agent 在自己的 TUI 上长什么样，由 agent 插件自己说。
//!
//! 规则是**数据**而不是代码，理由与 `canonical_event` 同源——守住「加 agent 只动
//! `plugins/`」。写成函数的话，宿主侧迟早长出一排 `match provider`，而每加一个 agent
//! 都要改引擎；写成数据则新 agent 只在自己的插件里声明一张表。
//!
//! 求值引擎在宿主侧（`meowo-app` 的 `detect`），它只认这里的类型，不认识任何具体 agent。
//!
//! **失准方向的纪律**：规则匹配的是各家 TUI 的界面文案，必然随对方改版腐坏。一条都没
//! 命中时引擎回退 `Idle`，因此弱证据规则务必不要带 `visible`——宁可漏报一次「agent 在
//! 等你」，也不能把正在跑的会话谎报成阻塞。

/// 屏幕判定出的 agent 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenState {
    Working,
    Idle,
    Blocked,
}

impl ScreenState {
    pub fn as_str(self) -> &'static str {
        match self {
            ScreenState::Working => "working",
            ScreenState::Idle => "idle",
            ScreenState::Blocked => "blocked",
        }
    }
}

/// 规则在屏幕的哪一部分求值。锚定区域是「别匹配到整屏偶发文本」的主要手段——
/// 历史输出里出现过的审批文案不该让当前状态判成阻塞。
#[derive(Debug, Clone, Copy)]
pub enum Region {
    /// 整屏（末屏文本，非滚动历史）。
    WholeScreen,
    /// 底部 N 个非空行（状态行、快捷键提示常驻此处）。
    BottomNonEmpty(usize),
    /// 最后一条水平分隔线（`─`）之后——TUI 通常用它分隔输出与交互区。
    AfterLastRule,
    /// 提示框体：自底向上第 2 条分隔线与其后第一条之间（claude 的输入框）。
    PromptBoxBody,
    /// 最后一个提示符标记行（codex 的 `›`）之后。
    AfterLastPromptMarker,
    /// 终端标题（OSC 0/2），不是屏幕内容。spinner 帧与状态字常写在这儿。
    Title,
}

/// 规则的匹配条件。组合子（All/Any/Not）可嵌套，叶子谓词都针对「区域内的若干行」。
///
/// 文本比较统一在**小写**上做（声明侧请直接写小写字面量）；标题类谓词用原文，
/// 因为它们判的是字符集而非词形。
#[derive(Debug, Clone, Copy)]
pub enum Matcher {
    /// 区域内**全部**出现这些子串（AND）。
    Contains(&'static [&'static str]),
    /// 区域内出现其中**任一**子串（OR）。
    AnyContains(&'static [&'static str]),
    /// 存在某行（trim 前导空白后）以其中任一前缀开头。
    LineStartsWith(&'static [&'static str]),
    /// 存在某行去掉前导空白与可选 `❯` 后，以 `<数字>. <词>` 开头（审批菜单的选项行）。
    NumberedOption(char, &'static str),
    /// 存在某行去掉前导空白与可选 `❯` 后直接是 `yes`（无编号的确认项）。
    BareYes,
    /// 存在某行整行只有一个字符，且它属于该字符集（月相 spinner）。
    SoleCharLineIn(&'static str),
    /// 存在某行以连续的盲文字符开头，其后紧跟任一关键词（盲文 spinner + 状态字）。
    BraillePrefixed(&'static [&'static str]),
    /// 文本以「该字符集中的一个字符 + 空格」开头。用于标题：claude 的 `⠋ ` / `✳ `。
    StartsWithCharIn(&'static str),
    /// 该字符集中的某个字符**独立成词**出现（两侧是空格或边界）。用于 codex 标题 spinner。
    WordCharIn(&'static str),
    /// 区域非空（去空白后有内容）。用于「有标题但既不转也不告警」这类兜底。
    NonEmpty,
    All(&'static [Matcher]),
    Any(&'static [Matcher]),
    /// 任一子条件成立即整条规则失败（否决门）。
    Not(&'static [Matcher]),
}

/// 一条屏幕规则。引擎按 `priority` 降序求值，首个命中者胜；平局时先声明者胜。
#[derive(Debug, Clone, Copy)]
pub struct ScreenRule {
    /// 稳定标识，出现在 explain 输出里——排障时靠它定位是哪条规则判的。
    pub id: &'static str,
    pub state: ScreenState,
    pub priority: i32,
    pub region: Region,
    /// 该判定有**可见证据**（屏幕上确实画着审批 UI / 提示框 / spinner）。
    /// 影响防抖：可见证据直接发布，无证据的降级要等确认期。弱规则不要带它。
    pub visible: bool,
    /// 命中即**保持**当前状态不变（覆盖层：transcript 查看器、模型选单等）。
    /// 这类规则的 `state` 无意义，引擎不会发布它。
    pub hold: bool,
    pub matcher: Matcher,
}

impl ScreenRule {
    /// 声明一条常规规则（非 hold、无可见证据）。链式方法补充其余属性。
    pub const fn new(
        id: &'static str,
        state: ScreenState,
        priority: i32,
        region: Region,
        matcher: Matcher,
    ) -> Self {
        Self {
            id,
            state,
            priority,
            region,
            visible: false,
            hold: false,
            matcher,
        }
    }

    /// 标记该判定有可见证据（防抖直通）。
    pub const fn visible(mut self) -> Self {
        self.visible = true;
        self
    }

    /// 标记为覆盖层规则：命中则保持当前状态。
    pub const fn hold(mut self) -> Self {
        self.hold = true;
        self
    }
}
