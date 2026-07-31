//! 托管 PTY 的屏幕状态检测：跨平台纯逻辑，不依赖任何 VT 实现，便于在任意平台单测。
//!
//! 输入是**终端仿真后的干净末屏文本**（vt100 grid 重建，无 ANSI 转义），不是原始字节流——
//! 对字节流做字符串匹配会被全屏重绘/光标定位序列打穿，这是本模块存在的前提（接线见
//! `pty.rs` 的 screen 字段）。规则语义移植自 herdr 的 claude 清单（Apache-2.0，
//! 证据驱动维护），保持同一套区域锚定与从严纪律：
//!
//! - **blocked 从严**：只有命中已知的可见审批 UI 才判 blocked；什么都没命中回退
//!   idle（`fallback_idle`），失准的表现是「少报 blocked」而不是误报——安全侧失败。
//! - **防抖**（[`ScreenDebounce`]）：working → 无可见证据的 idle 要连续确认或超时才发布，
//!   吸收 spinner 帧闪烁与清屏瞬间；带可见证据（提示框 `❯`）的 idle 直接放行。
//! - **skip 规则**：transcript 查看器 / 模型选单这类覆盖层不代表状态变化，命中时保持
//!   已发布状态不动。

use std::time::{Duration, Instant};

/// 屏幕判定出的 agent 状态。序列化为 snake_case 字符串给前端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScreenState {
    Working,
    Idle,
    Blocked,
}

impl ScreenState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ScreenState::Working => "working",
            ScreenState::Idle => "idle",
            ScreenState::Blocked => "blocked",
        }
    }
}

/// 终端仿真后的末屏快照。`lines` 自上而下、已去尾部空白与尾部空行；`title` 来自 OSC 0/2。
pub(crate) struct ScreenSnapshot {
    pub(crate) lines: Vec<String>,
    pub(crate) title: Option<String>,
}

impl ScreenSnapshot {
    pub(crate) fn new(mut lines: Vec<String>, title: Option<String>) -> Self {
        for line in &mut lines {
            let trimmed = line.trim_end();
            if trimmed.len() != line.len() {
                line.truncate(trimmed.len());
            }
        }
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        Self { lines, title }
    }
}

/// 一次求值的结论。
pub(crate) enum Evaluation {
    /// 常规判定（含 fallback_idle）。
    Publish(Detection),
    /// 命中覆盖层类规则：屏幕内容不代表状态变化，保持已发布状态。
    Hold { rule_id: &'static str },
}

pub(crate) struct Detection {
    pub(crate) state: ScreenState,
    /// 有可见证据（审批 UI / 提示框 / spinner 标题）：防抖直接放行，不需要确认期。
    pub(crate) visible: bool,
    pub(crate) rule_id: &'static str,
}

impl Evaluation {
    pub(crate) fn rule_id(&self) -> &'static str {
        match self {
            Evaluation::Publish(det) => det.rule_id,
            Evaluation::Hold { rule_id } => rule_id,
        }
    }
}

/// 无规则命中时的回退标签（对齐 herdr 的 default_known_agent_idle_fallback 语义）。
pub(crate) const FALLBACK_RULE_ID: &str = "fallback_idle";

/// 该 provider 是否有规则集。ticker 用它在建快照之前提前跳过（无规则的会话零开销）。
pub(crate) fn provider_supported(provider: &str) -> bool {
    matches!(provider, "claude" | "claude-code" | "codex" | "kimi" | "kimi-code")
}

/// 按 provider 求值。None = 该 provider 没有规则集（不做屏幕检测）。
/// 规则按优先级降序排列，首个命中者胜；一条都没命中回退 idle（blocked 从严）。
pub(crate) fn evaluate(provider: &str, snap: &ScreenSnapshot) -> Option<Evaluation> {
    match provider {
        "claude" | "claude-code" => Some(evaluate_claude(snap)),
        "codex" => Some(evaluate_codex(snap)),
        "kimi" | "kimi-code" => Some(evaluate_kimi(snap)),
        _ => None,
    }
}

/// 从 spawn argv[0] 推 provider 标签：取文件名主干、小写。托管 PTY 跑的必是 agent CLI，
/// 可执行名即身份（"C:\\...\\claude.exe" / "/usr/local/bin/claude" → "claude"）。
pub(crate) fn provider_from_argv0(argv0: &str) -> String {
    let name = argv0
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(argv0)
        .to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    name
}

// ---------------------------------------------------------------------------
// Claude Code 规则（herdr claude.toml 2026.07.13.1 的语义移植，优先级降序）
// ---------------------------------------------------------------------------

fn evaluate_claude(snap: &ScreenSnapshot) -> Evaluation {
    // 匹配统一在小写文本上做（herdr 的 contains 语义：大小写不敏感）；
    // 标题的 spinner/✳ 前缀与大小写无关，用原文。
    let lower: Vec<String> = snap
        .lines
        .iter()
        .map(|line| line.to_lowercase())
        .collect();
    let title = snap.title.as_deref().unwrap_or("");

    // 1100 osc_title_working：标题以盲文 spinner 帧开头。
    if title_starts_with_braille(title) {
        return publish(ScreenState::Working, true, "osc_title_working");
    }
    // 1000 transcript_viewer：ctrl+o 的详细 transcript 覆盖层，不代表状态变化。
    if transcript_viewer(&lower) {
        return Evaluation::Hold {
            rule_id: "transcript_viewer",
        };
    }
    // 980 live_blocked_form：最后一条水平分隔线之后出现「选择型」审批表单。
    if live_blocked_form(&lower) {
        return publish(ScreenState::Blocked, true, "live_blocked_form");
    }
    // 980 dynamic_workflow_prompt。
    if region_contains(&lower, "run a dynamic workflow?") && region_contains(&lower, "esc to cancel")
    {
        return publish(ScreenState::Blocked, true, "dynamic_workflow_prompt");
    }
    // 975 btw_overlay_working：/btw 覆盖层挂着说明后台仍在跑。
    if btw_overlay(&lower) {
        return publish(ScreenState::Working, true, "btw_overlay_working");
    }
    // 950 live_prompt_box：提示框体内以 ❯ 开头且无任何审批文案 → 等输入的空闲态。
    if live_prompt_box(&lower) {
        return publish(ScreenState::Idle, true, "live_prompt_box");
    }
    // 900 model_picker_menu：模型选单覆盖层。
    if model_picker(&lower) {
        return Evaluation::Hold {
            rule_id: "model_picker_menu",
        };
    }
    // 850 bash_permission_prompt。
    if bash_permission(&lower) {
        return publish(ScreenState::Blocked, true, "bash_permission_prompt");
    }
    // 840 generic_permission_prompt。
    if generic_permission(&lower) {
        return publish(ScreenState::Blocked, true, "generic_permission_prompt");
    }
    // 300 legacy_no_prompt_blocker：弱证据，不带可见标志（防抖不直通）。
    if legacy_blocker(&lower) {
        return publish(ScreenState::Blocked, false, "legacy_no_prompt_blocker");
    }
    // 250 osc_title_idle：标题 ✳ 前缀。
    if title.starts_with("\u{2733} ") {
        return publish(ScreenState::Idle, true, "osc_title_idle");
    }
    publish(ScreenState::Idle, false, FALLBACK_RULE_ID)
}

// ---------------------------------------------------------------------------
// Codex 规则（herdr codex.toml 2026.07.18.1 的语义移植，优先级降序）
// ---------------------------------------------------------------------------

/// codex 的标题 spinner 帧集合（盲文十连帧，独立成词出现）。
const CODEX_SPINNER: &str = "\u{280B}\u{2819}\u{2839}\u{2838}\u{283C}\u{2834}\u{2826}\u{2827}\u{2807}\u{280F}";

fn evaluate_codex(snap: &ScreenSnapshot) -> Evaluation {
    let lower: Vec<String> = snap.lines.iter().map(|line| line.to_lowercase()).collect();
    let title = snap.title.as_deref().unwrap_or("");
    let title_lower = title.to_lowercase();

    // 1100 osc_title_blocked：codex 会把「Action Required」写进标题——唯一由标题直接
    // 判 blocked 的宿主。
    if title_lower.contains("action required") {
        return publish(ScreenState::Blocked, true, "osc_title_blocked");
    }
    // 1050 osc_title_working：标题里独立成词的 spinner 帧。
    if word_bounded_char(title, CODEX_SPINNER) {
        return publish(ScreenState::Working, true, "osc_title_working");
    }
    // 1000 transcript_viewer：全屏 transcript 查看器，不代表状态变化。
    let region = after_last_prompt_marker(&lower);
    if ["↑/↓ to scroll", "pgup/pgdn to", "home/end to jump", "q to quit"]
        .iter()
        .all(|needle| region_contains(region, needle))
        && (region_contains(region, "esc to edit prev")
            || region_contains(region, "esc/← to edit prev"))
    {
        return Evaluation::Hold {
            rule_id: "transcript_viewer",
        };
    }
    // 900 live_strong_blocker：提示符标记之后出现确认/提交/放行文案。
    if [
        "press enter to confirm or esc to cancel",
        "enter to submit answer",
        "enter to submit all",
        "allow command?",
    ]
    .iter()
    .any(|needle| region_contains(region, needle))
    {
        return publish(ScreenState::Blocked, true, "live_strong_blocker");
    }
    // 600 weak_blocker：弱证据，不带可见标志。
    let yes_or_pointer = region_contains(&lower, "yes") || region_contains(&lower, "\u{276F}");
    if region_contains(&lower, "[y/n]")
        || region_contains(&lower, "yes (y)")
        || (region_contains(&lower, "do you want to") && yes_or_pointer)
        || (region_contains(&lower, "would you like to") && yes_or_pointer)
    {
        return publish(ScreenState::Blocked, false, "weak_blocker");
    }
    // 500 screen_working_fallback：底部的 `• Working (…esc to interrupt)` 状态行。
    let bottom = bottom_non_empty(&lower, 3);
    let working_line = bottom.iter().any(|line| {
        let rest = line.trim_start();
        let rest = rest
            .strip_prefix('\u{2022}')
            .or_else(|| rest.strip_prefix('\u{25E6}'));
        rest.is_some_and(|tail| {
            tail.trim_start().starts_with("working (") && tail.contains("esc to interrupt")
        })
    });
    if working_line && !region_contains(&bottom, "■ conversation interrupted") {
        return publish(ScreenState::Working, true, "screen_working_fallback");
    }
    // 100 osc_title_idle：有标题但既不转也不告警。
    if !title.trim().is_empty() {
        return publish(ScreenState::Idle, true, "osc_title_idle");
    }
    publish(ScreenState::Idle, false, FALLBACK_RULE_ID)
}

// ---------------------------------------------------------------------------
// Kimi 规则（herdr kimi.toml 2026.06.10.1 的语义移植，优先级降序）。
// kimi 的 PermissionRequest hook 在 Meowo 里是 observation-only（不接管决策），
// 屏幕检测是它「等审批/等回答」状态的唯一实时来源。
// ---------------------------------------------------------------------------

fn evaluate_kimi(snap: &ScreenSnapshot) -> Evaluation {
    let lower: Vec<String> = snap.lines.iter().map(|line| line.to_lowercase()).collect();

    // 400 current_approval_panel：当前版审批面板。
    let approval_prompt = [
        "run this command?",
        "write this file?",
        "apply these edits?",
        "stop this task?",
        "ready to build with this plan?",
    ]
    .iter()
    .any(|needle| region_contains(&lower, needle))
        || lower.iter().any(|line| {
            let rest = line.trim_start();
            let rest = rest
                .strip_prefix('\u{25B6}')
                .map_or(rest, str::trim_start);
            rest.starts_with("approve ") && rest.trim_end().ends_with('?')
        });
    if region_contains(&lower, "↵ confirm")
        && approval_prompt
        && region_contains(&lower, " choose")
        && (region_contains(&lower, "approve")
            || region_contains(&lower, "reject")
            || region_contains(&lower, "revise"))
    {
        return publish(ScreenState::Blocked, true, "current_approval_panel");
    }
    // 390 question_panel：提问面板（↑↓ select · esc cancel + question 标题行 + ? 行）。
    if region_contains(&lower, "↑↓ select")
        && region_contains(&lower, "esc cancel")
        && lower.iter().any(|line| line.trim() == "question")
        && lower.iter().any(|line| line.trim_start().starts_with("? "))
        && ["↵ choose", "↵ toggle", "↵ save"]
            .iter()
            .any(|needle| region_contains(&lower, needle))
    {
        return publish(ScreenState::Blocked, true, "question_panel");
    }
    // 300 legacy_approval_panel：旧版审批面板，弱证据。
    if region_contains(&lower, "requesting approval")
        && region_contains(&lower, "reject")
        && (region_contains(&lower, "approve once")
            || region_contains(&lower, "approve for this session"))
        && (region_contains(&lower, "1/2/3/4 choose") || region_contains(&lower, "↵ confirm"))
    {
        return publish(ScreenState::Blocked, false, "legacy_approval_panel");
    }
    // 120 background_agent_status_working：`kimi thinking … [N agents running]` 状态行。
    let bottom = bottom_non_empty(&lower, 3);
    if bottom.iter().any(|line| {
        line.contains("kimi")
            && line.contains("thinking")
            && line.contains('[')
            && line.contains("running]")
    }) {
        return publish(ScreenState::Working, true, "background_agent_status_working");
    }
    // 100 moon_spinner_working：独立成行的月相 spinner。
    if lower.iter().any(|line| {
        let trimmed = line.trim();
        let mut chars = trimmed.chars();
        matches!((chars.next(), chars.next()), (Some(moon), None) if "🌕🌖🌗🌘🌑🌒🌓🌔".contains(moon))
    }) {
        return publish(ScreenState::Working, true, "moon_spinner_working");
    }
    // 90 braille_spinner_working：盲文 spinner + thinking/working/using 前缀。
    if lower.iter().any(|line| {
        let rest = line.trim_start();
        let stripped = rest.trim_start_matches(|ch: char| ('\u{2800}'..='\u{28FF}').contains(&ch));
        stripped.len() != rest.len()
            && ["thinking...", "working...", "using "]
                .iter()
                .any(|keyword| stripped.trim_start().starts_with(keyword))
    }) {
        return publish(ScreenState::Working, true, "braille_spinner_working");
    }
    publish(ScreenState::Idle, false, FALLBACK_RULE_ID)
}

/// codex 的提示符标记行（`›` 独占或 `› ` 开头，行首不 trim——codex 顶格渲染）。
fn codex_prompt_line(line: &str) -> bool {
    line == "\u{203A}" || line.starts_with("\u{203A} ")
}

/// 最后一个提示符标记行之后的区域；没有标记时是整个屏幕。
fn after_last_prompt_marker(lines: &[String]) -> &[String] {
    let start = lines
        .iter()
        .rposition(|line| codex_prompt_line(line))
        .map_or(0, |index| index + 1);
    &lines[start..]
}

/// 文本里是否有 `set` 中的某个字符**独立成词**出现（两侧是空格或文本边界）。
fn word_bounded_char(text: &str, set: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.iter().enumerate().any(|(index, ch)| {
        set.contains(*ch)
            && index.checked_sub(1).is_none_or(|prev| chars[prev] == ' ')
            && chars.get(index + 1).is_none_or(|next| *next == ' ')
    })
}

fn publish(state: ScreenState, visible: bool, rule_id: &'static str) -> Evaluation {
    Evaluation::Publish(Detection {
        state,
        visible,
        rule_id,
    })
}

fn title_starts_with_braille(title: &str) -> bool {
    let mut chars = title.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(first), Some(' ')) if ('\u{2800}'..='\u{28FF}').contains(&first)
    )
}

fn transcript_viewer(lower: &[String]) -> bool {
    let bottom = bottom_non_empty(lower, 3);
    region_contains(&bottom, "showing detailed transcript")
        && ((region_contains(&bottom, "ctrl+o") && region_contains(&bottom, "to toggle"))
            || (region_contains(&bottom, "ctrl+e") && region_contains(&bottom, "show all"))
            || (region_contains(&bottom, "ctrl+e") && region_contains(&bottom, "collapse"))
            || region_contains(&bottom, "↑↓ scroll")
            || region_contains(&bottom, "? for shortcuts"))
}

fn live_blocked_form(lower: &[String]) -> bool {
    let region = after_last_horizontal_rule(lower);
    region_contains(region, "enter to select")
        && region_contains(region, "esc to cancel")
        && [
            "tab/arrow keys to navigate",
            "arrow keys to navigate",
            "arrows to navigate",
            "↑/↓ to navigate",
            "↑↓ to navigate",
        ]
        .iter()
        .any(|hint| region_contains(region, hint))
}

fn btw_overlay(lower: &[String]) -> bool {
    let bottom = bottom_non_empty(lower, 5);
    let has_btw = bottom.iter().any(|line| {
        let rest = line.trim_start();
        rest.strip_prefix("/btw")
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
    });
    has_btw && bottom.iter().any(|line| line.ends_with("esc to close"))
}

fn live_prompt_box(lower: &[String]) -> bool {
    let Some(body) = prompt_box_body(lower) else {
        return false;
    };
    body.iter()
        .any(|line| line.trim_start().starts_with('\u{276F}'))
        && ![
            "enter to select",
            "esc to cancel",
            "tab/arrow keys",
            "arrow keys to navigate",
            "↑/↓ to navigate",
        ]
        .iter()
        .any(|text| region_contains(body, text))
}

fn model_picker(lower: &[String]) -> bool {
    region_contains(lower, "select model")
        && region_contains(lower, "enter to set as default")
        && region_contains(lower, "esc to cancel")
        && !region_contains(lower, "do you want to proceed?")
        && !region_contains(lower, "enter to select")
}

fn bash_permission(lower: &[String]) -> bool {
    region_contains(lower, "do you want to proceed?")
        && [
            "bash command",
            "bash(",
            "contains expansion",
            "tab to amend",
            "ctrl+e to explain",
        ]
        .iter()
        .any(|evidence| region_contains(lower, evidence))
        && lower.iter().any(|line| {
            bare_yes_option(line) || numbered_option(line, '1', "yes") || numbered_option(line, '2', "no")
        })
}

fn generic_permission(lower: &[String]) -> bool {
    let region = after_last_horizontal_rule(lower);
    region_contains(region, "do you want to proceed?")
        && region_contains(region, "esc to cancel")
        && region.iter().any(|line| {
            numbered_option(line, '1', "yes")
                || numbered_option(line, '2', "yes")
                || numbered_option(line, '2', "no")
                || numbered_option(line, '3', "no")
        })
}

fn legacy_blocker(lower: &[String]) -> bool {
    let yes_or_pointer =
        |_: ()| region_contains(lower, "yes") || region_contains(lower, "\u{276F}");
    let any_evidence = (region_contains(lower, "do you want to") && yes_or_pointer(()))
        || (region_contains(lower, "would you like to") && yes_or_pointer(()))
        || region_contains(lower, "waiting for permission")
        || region_contains(lower, "do you want to allow this connection?")
        || region_contains(lower, "tab to amend")
        || region_contains(lower, "ctrl+e to explain")
        || (region_contains(lower, "do you want to proceed?")
            && region_contains(lower, "esc to cancel"))
        || region_contains(lower, "review your answers")
        || region_contains(lower, "skip interview and plan immediately");
    // 空提示符行（独立一行只有 ❯）出现 = 提示框其实空着在等输入 → 否决弱 blocked。
    any_evidence && !lower.iter().any(|line| line.trim() == "\u{276F}")
}

// ---------------------------------------------------------------------------
// 区域锚定（语义对齐 herdr manifest.rs 的同名函数）
// ---------------------------------------------------------------------------

/// 该行是否是水平分隔线：trim 后以 `─` 开头，且「─ 连跑之后为空」或「连跑 ≥3」
/// （后者容忍 `─── 标题` 式分节线）。
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let run = trimmed.chars().take_while(|&ch| ch == '\u{2500}').count();
    if run == 0 {
        return false;
    }
    let suffix_start = trimmed
        .char_indices()
        .nth(run)
        .map_or(trimmed.len(), |(index, _)| index);
    trimmed[suffix_start..].trim_start().is_empty() || run >= 3
}

/// 最后一条水平分隔线之后的区域；没有分隔线时是整个屏幕。
fn after_last_horizontal_rule(lines: &[String]) -> &[String] {
    let start = lines
        .iter()
        .rposition(|line| is_horizontal_rule(line))
        .map_or(0, |index| index + 1);
    &lines[start..]
}

/// 提示框体：自底向上第 2 条水平分隔线（框顶）之后、到下一条分隔线之前的行。
/// 找不到两条分隔线 = 屏幕上没有提示框。
fn prompt_box_body(lines: &[String]) -> Option<&[String]> {
    let mut rules = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, line)| is_horizontal_rule(line))
        .map(|(index, _)| index);
    let _bottom = rules.next()?;
    let top = rules.next()?;
    let end = lines[top + 1..]
        .iter()
        .position(|line| is_horizontal_rule(line))
        .map_or(lines.len(), |relative| top + 1 + relative);
    Some(&lines[top + 1..end])
}

/// 底部 N 个非空行（保持自上而下顺序）。
fn bottom_non_empty(lines: &[String], n: usize) -> Vec<String> {
    let mut picked: Vec<String> = lines
        .iter()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(n)
        .cloned()
        .collect();
    picked.reverse();
    picked
}

fn region_contains(region: &[String], needle: &str) -> bool {
    region.iter().any(|line| line.contains(needle))
}

/// `^\s*❯?\s*yes\b`（行已小写）。
fn bare_yes_option(line: &str) -> bool {
    let rest = line.trim_start();
    let rest = rest
        .strip_prefix('\u{276F}')
        .map_or(rest, str::trim_start);
    starts_with_word(rest, "yes")
}

/// `^\s*❯?\s*N\.\s*<word>\b`（行已小写）。
fn numbered_option(line: &str, number: char, word: &str) -> bool {
    let rest = line.trim_start();
    let rest = rest
        .strip_prefix('\u{276F}')
        .map_or(rest, str::trim_start);
    let Some(rest) = rest.strip_prefix(number) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('.') else {
        return false;
    };
    starts_with_word(rest.trim_start(), word)
}

fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|tail| !tail.starts_with(char::is_alphanumeric))
}

// ---------------------------------------------------------------------------
// 防抖
// ---------------------------------------------------------------------------

/// working → 「无可见证据的 idle」的确认次数与时间上限（对齐 herdr：3 次或 700ms）。
/// spinner 帧切换/清屏瞬间会闪出一帧什么都匹配不上的屏幕，直接发布会造成状态抖动。
const PENDING_IDLE_CONFIRMS: u8 = 3;
const PENDING_IDLE_CAP: Duration = Duration::from_millis(700);

/// 每会话一份的发布门。[`Self::observe`] 返回 Some = 对外发布的状态发生变化。
#[derive(Default)]
pub(crate) struct ScreenDebounce {
    published: Option<ScreenState>,
    /// (首次观察到待确认 idle 的时刻, 已确认次数)。
    pending_idle: Option<(Instant, u8)>,
}

impl ScreenDebounce {
    pub(crate) fn published(&self) -> Option<ScreenState> {
        self.published
    }

    /// 是否有待确认的降级——ticker 据此在内容序号未变时也继续扫描（确认的正是「屏幕
    /// 稳定不变」这件事，跳扫会让确认永远等不齐）。
    pub(crate) fn pending(&self) -> bool {
        self.pending_idle.is_some()
    }

    pub(crate) fn observe(&mut self, eval: &Evaluation, now: Instant) -> Option<ScreenState> {
        let det = match eval {
            Evaluation::Hold { .. } => return None,
            Evaluation::Publish(det) => det,
        };
        if self.published == Some(det.state) {
            self.pending_idle = None;
            return None;
        }
        if det.state == ScreenState::Idle && !det.visible && self.published == Some(ScreenState::Working)
        {
            let (since, confirms) = self.pending_idle.get_or_insert((now, 0));
            *confirms += 1;
            if *confirms < PENDING_IDLE_CONFIRMS && now.duration_since(*since) < PENDING_IDLE_CAP {
                return None;
            }
        }
        self.pending_idle = None;
        self.published = Some(det.state);
        Some(det.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(lines: &[&str]) -> ScreenSnapshot {
        ScreenSnapshot::new(lines.iter().map(|s| s.to_string()).collect(), None)
    }

    fn snap_titled(lines: &[&str], title: &str) -> ScreenSnapshot {
        ScreenSnapshot::new(
            lines.iter().map(|s| s.to_string()).collect(),
            Some(title.to_string()),
        )
    }

    fn state_of(eval: Option<Evaluation>) -> Option<(ScreenState, &'static str)> {
        match eval? {
            Evaluation::Publish(det) => Some((det.state, det.rule_id)),
            Evaluation::Hold { rule_id } => Some((ScreenState::Idle, rule_id)),
        }
    }

    #[test]
    fn provider_tag_comes_from_argv0_stem() {
        assert_eq!(provider_from_argv0("claude"), "claude");
        assert_eq!(provider_from_argv0(r"C:\Users\x\AppData\npm\claude.CMD"), "claude");
        assert_eq!(provider_from_argv0("/usr/local/bin/claude"), "claude");
        assert_eq!(provider_from_argv0("kimi.exe"), "kimi");
    }

    #[test]
    fn unknown_provider_has_no_rules() {
        assert!(evaluate("wezterm", &snap(&["anything"])).is_none());
    }

    /// 标题盲文 spinner 帧 → working（最高优先级，压过屏幕上的一切）。
    #[test]
    fn braille_title_wins_as_working() {
        let s = snap_titled(&["Do you want to proceed?", "\u{276F} 1. Yes"], "\u{280B} Herding bits…");
        assert_eq!(
            state_of(evaluate("claude", &s)),
            Some((ScreenState::Working, "osc_title_working"))
        );
    }

    #[test]
    fn esc_title_means_idle() {
        let s = snap_titled(&["some scrollback"], "\u{2733} claude");
        assert_eq!(
            state_of(evaluate("claude", &s)),
            Some((ScreenState::Idle, "osc_title_idle"))
        );
    }

    /// 分隔线后的选择型审批表单 → blocked（可见证据）。
    #[test]
    fn selection_form_after_rule_is_blocked() {
        let s = snap(&[
            "  tool output above",
            "────────────────────────",
            " Do you want to make this edit?",
            " \u{276F} 1. Yes",
            "   2. No",
            " Enter to select · Esc to cancel · ↑/↓ to navigate",
        ]);
        assert_eq!(
            state_of(evaluate("claude", &s)),
            Some((ScreenState::Blocked, "live_blocked_form"))
        );
    }

    #[test]
    fn bash_permission_prompt_is_blocked() {
        let s = snap(&[
            " Bash command",
            "   rm -rf ./build",
            " Do you want to proceed?",
            " \u{276F} 1. Yes",
            "   2. No, and tell Claude what to do differently",
        ]);
        assert_eq!(
            state_of(evaluate("claude", &s)),
            Some((ScreenState::Blocked, "bash_permission_prompt"))
        );
    }

    #[test]
    fn generic_permission_prompt_is_blocked() {
        let s = snap(&[
            "────────────────────────",
            " Do you want to proceed?",
            " 1. Yes",
            " 2. Yes, allow all edits during this session",
            " 3. No",
            " Esc to cancel",
        ]);
        assert_eq!(
            state_of(evaluate("claude", &s)),
            Some((ScreenState::Blocked, "generic_permission_prompt"))
        );
    }

    /// 提示框（倒数第二条分隔线之后）里 ❯ 开头且无审批文案 → 可见 idle。
    #[test]
    fn prompt_box_is_visible_idle() {
        let s = snap(&[
            "  previous output",
            "────────────────────────",
            " \u{276F} try \"fix the failing test\"",
            "────────────────────────",
            "  ? for shortcuts",
        ]);
        let Some(Evaluation::Publish(det)) = evaluate("claude", &s) else {
            panic!("应有判定");
        };
        assert_eq!((det.state, det.rule_id), (ScreenState::Idle, "live_prompt_box"));
        assert!(det.visible, "提示框是可见 idle 证据");
    }

    /// 提示框里出现审批导航文案时不能判**可见** idle（选择表单的 ❯ 是选项指针不是提示符）：
    /// not 门否决 live_prompt_box，只能落到无可见证据的 fallback——防抖会扣住它，不会
    /// 在审批表单还挂着时把卡片翻成空闲。
    #[test]
    fn prompt_box_with_approval_text_is_not_visible_idle() {
        let s = snap(&[
            "────────────────────────",
            " \u{276F} 1. Yes",
            "   2. No",
            " Enter to select · Esc to cancel · arrow keys to navigate",
            "────────────────────────",
            "  hint line",
        ]);
        let Some(Evaluation::Publish(det)) = evaluate("claude", &s) else {
            panic!("应有判定");
        };
        assert_ne!(det.rule_id, "live_prompt_box");
        assert!(!det.visible, "审批表单在屏时绝不能给出可见 idle 证据");
    }

    /// transcript 覆盖层 → Hold，不改状态。
    #[test]
    fn transcript_viewer_holds_state() {
        let s = snap(&[
            "  lots of transcript lines",
            " Showing detailed transcript",
            "  ctrl+o to toggle",
        ]);
        assert!(matches!(
            evaluate("claude", &s),
            Some(Evaluation::Hold { rule_id: "transcript_viewer" })
        ));
    }

    /// 什么都没命中 → 回退 idle（blocked 从严：宁可少报）。
    #[test]
    fn no_match_falls_back_to_idle() {
        let s = snap(&["  compiling foo v0.1.0", "  warning: unused variable"]);
        let Some(Evaluation::Publish(det)) = evaluate("claude", &s) else {
            panic!("应有判定");
        };
        assert_eq!((det.state, det.rule_id), (ScreenState::Idle, FALLBACK_RULE_ID));
        assert!(!det.visible);
    }

    /// 空提示符行否决弱 blocked（legacy 规则的 not 门）。
    #[test]
    fn bare_prompt_line_vetoes_legacy_blocker() {
        let s = snap(&["  do you want to refactor? yes we discussed it", " \u{276F}"]);
        let got = state_of(evaluate("claude", &s));
        assert_ne!(got.map(|(state, _)| state), Some(ScreenState::Blocked));
    }

    // -- codex --

    #[test]
    fn codex_action_required_title_is_blocked() {
        let s = snap_titled(&["some output"], "Action Required · codex");
        assert_eq!(
            state_of(evaluate("codex", &s)),
            Some((ScreenState::Blocked, "osc_title_blocked"))
        );
    }

    #[test]
    fn codex_spinner_title_is_working() {
        let s = snap_titled(&["output"], "codex \u{280B} building");
        assert_eq!(
            state_of(evaluate("codex", &s)),
            Some((ScreenState::Working, "osc_title_working"))
        );
    }

    /// 强 blocker 只认提示符标记之后的区域：标记之前的历史文本不算数。
    #[test]
    fn codex_strong_blocker_after_prompt_marker() {
        let s = snap(&[
            "  old scroll: allow command? was answered long ago",
            "\u{203A} previous prompt",
            " Allow command?",
            " press enter to confirm or esc to cancel",
        ]);
        assert_eq!(
            state_of(evaluate("codex", &s)),
            Some((ScreenState::Blocked, "live_strong_blocker"))
        );
    }

    #[test]
    fn codex_working_status_line_is_working() {
        let s = snap(&[
            "  compiling...",
            " \u{2022} Working (3m 12s · esc to interrupt) · 42 tokens",
        ]);
        assert_eq!(
            state_of(evaluate("codex", &s)),
            Some((ScreenState::Working, "screen_working_fallback"))
        );
    }

    /// 有标题但既不转也不告警 → idle；无标题无命中 → fallback。
    #[test]
    fn codex_plain_title_is_idle() {
        let s = snap_titled(&["  quiet"], "codex — ~/repo");
        assert_eq!(
            state_of(evaluate("codex", &s)),
            Some((ScreenState::Idle, "osc_title_idle"))
        );
        let bare = snap(&["  quiet"]);
        assert_eq!(
            state_of(evaluate("codex", &bare)),
            Some((ScreenState::Idle, FALLBACK_RULE_ID))
        );
    }

    // -- kimi --

    #[test]
    fn kimi_approval_panel_is_blocked() {
        let s = snap(&[
            " Requesting approval",
            " Run this command?",
            "   cargo test --all",
            " \u{25B6} Approve once   Reject   Revise",
            " 1/2/3 choose · \u{21B5} confirm · esc cancel",
        ]);
        assert_eq!(
            state_of(evaluate("kimi", &s)),
            Some((ScreenState::Blocked, "current_approval_panel"))
        );
    }

    #[test]
    fn kimi_question_panel_is_blocked() {
        let s = snap(&[
            " Question",
            " ? 晚饭吃什么",
            "   火锅",
            "   寿司",
            " ↑↓ select · \u{21B5} choose · esc cancel",
        ]);
        assert_eq!(
            state_of(evaluate("kimi", &s)),
            Some((ScreenState::Blocked, "question_panel"))
        );
    }

    #[test]
    fn kimi_spinners_are_working() {
        let moon = snap(&["  output", " 🌕 "]);
        assert_eq!(
            state_of(evaluate("kimi", &moon)),
            Some((ScreenState::Working, "moon_spinner_working"))
        );
        let braille = snap(&[" \u{280B} Thinking... 3s"]);
        assert_eq!(
            state_of(evaluate("kimi", &braille)),
            Some((ScreenState::Working, "braille_spinner_working"))
        );
    }

    #[test]
    fn kimi_falls_back_to_idle() {
        let s = snap(&["  plain output", "  nothing pending"]);
        assert_eq!(
            state_of(evaluate("kimi", &s)),
            Some((ScreenState::Idle, FALLBACK_RULE_ID))
        );
    }

    // -- 防抖 --

    fn working_det() -> Evaluation {
        Evaluation::Publish(Detection {
            state: ScreenState::Working,
            visible: true,
            rule_id: "t",
        })
    }

    fn plain_idle() -> Evaluation {
        Evaluation::Publish(Detection {
            state: ScreenState::Idle,
            visible: false,
            rule_id: "fallback_idle",
        })
    }

    fn visible_idle() -> Evaluation {
        Evaluation::Publish(Detection {
            state: ScreenState::Idle,
            visible: true,
            rule_id: "live_prompt_box",
        })
    }

    /// working → 光秃 idle 需要 3 次确认；中途回到 working 则清零。
    #[test]
    fn plain_idle_needs_confirmations_after_working() {
        let mut debounce = ScreenDebounce::default();
        let t0 = Instant::now();
        assert_eq!(debounce.observe(&working_det(), t0), Some(ScreenState::Working));
        assert_eq!(debounce.observe(&plain_idle(), t0), None);
        assert!(debounce.pending());
        assert_eq!(debounce.observe(&plain_idle(), t0), None);
        // 第三次确认发布。
        assert_eq!(debounce.observe(&plain_idle(), t0), Some(ScreenState::Idle));
        assert!(!debounce.pending());
    }

    #[test]
    fn pending_idle_publishes_after_time_cap() {
        let mut debounce = ScreenDebounce::default();
        let t0 = Instant::now();
        debounce.observe(&working_det(), t0);
        assert_eq!(debounce.observe(&plain_idle(), t0), None);
        // 超过 700ms 后第二次观察即发布，不必凑满 3 次。
        assert_eq!(
            debounce.observe(&plain_idle(), t0 + Duration::from_millis(701)),
            Some(ScreenState::Idle)
        );
    }

    #[test]
    fn visible_idle_bypasses_hold() {
        let mut debounce = ScreenDebounce::default();
        let t0 = Instant::now();
        debounce.observe(&working_det(), t0);
        assert_eq!(debounce.observe(&visible_idle(), t0), Some(ScreenState::Idle));
    }

    #[test]
    fn working_resumption_clears_pending() {
        let mut debounce = ScreenDebounce::default();
        let t0 = Instant::now();
        debounce.observe(&working_det(), t0);
        assert_eq!(debounce.observe(&plain_idle(), t0), None);
        // spinner 下一帧回来了：清掉待确认，不发布任何变化。
        assert_eq!(debounce.observe(&working_det(), t0), None);
        assert!(!debounce.pending());
    }

    #[test]
    fn hold_keeps_published_state() {
        let mut debounce = ScreenDebounce::default();
        let t0 = Instant::now();
        debounce.observe(&working_det(), t0);
        assert_eq!(
            debounce.observe(&Evaluation::Hold { rule_id: "transcript_viewer" }, t0),
            None
        );
        assert_eq!(debounce.published(), Some(ScreenState::Working));
    }

    /// blocked 立即发布（审批不能等确认期）。
    #[test]
    fn blocked_publishes_immediately() {
        let mut debounce = ScreenDebounce::default();
        let t0 = Instant::now();
        debounce.observe(&working_det(), t0);
        let blocked = Evaluation::Publish(Detection {
            state: ScreenState::Blocked,
            visible: true,
            rule_id: "live_blocked_form",
        });
        assert_eq!(debounce.observe(&blocked, t0), Some(ScreenState::Blocked));
    }
}
