//! 托管 PTY 的屏幕状态检测**引擎**：跨平台纯逻辑，不依赖任何 VT 实现，也**不认识任何
//! 具体 agent**——规则由各 agent 插件声明（`meowo_agent::screen`），这里只负责求值。
//!
//! 输入是**终端仿真后的干净末屏文本**（vt100 grid 重建，无 ANSI 转义），不是原始字节流——
//! 对字节流做字符串匹配会被全屏重绘/光标定位序列打穿，这是本模块存在的前提（接线见
//! `pty.rs` 的 ScreenProbe）。规则语义移植自 herdr 的清单（Apache-2.0，证据驱动维护），
//! 保持同一套区域锚定与从严纪律：
//!
//! - **blocked 从严**：只有命中已知的可见审批 UI 才判 blocked；什么都没命中回退
//!   idle（`fallback_idle`），失准的表现是「少报 blocked」而不是误报——安全侧失败。
//! - **防抖**（[`ScreenDebounce`]）：working → 无可见证据的 idle 要连续确认或超时才发布，
//!   吸收 spinner 帧闪烁与清屏瞬间；带可见证据（提示框 `❯`）的 idle 直接放行。
//! - **hold 规则**：transcript 查看器 / 模型选单这类覆盖层不代表状态变化，命中时保持
//!   已发布状态不动。

use meowo_agent::screen::{Matcher, Region, ScreenRule};
use std::time::{Duration, Instant};

pub(crate) use meowo_agent::screen::ScreenState;

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

/// 取该 provider 的规则集。经 `meowo_agent::resolve` 解析身份——别名（"claude-code"
/// 等）与未知 provider 的处理都归它，本模块不维护第二份身份表。
fn rules_for(provider: &str) -> &'static [ScreenRule] {
    meowo_agent::resolve(Some(provider)).map_or(&[], |plugin| plugin.screen_rules())
}

/// 该 provider 是否有规则集。ScreenProbe 用它决定要不要建终端仿真器——无规则的会话
/// 连解析都不做（门必须设在生产端，否则 reader 每 chunk 白跑一遍 VT 状态机）。
pub(crate) fn provider_supported(provider: &str) -> bool {
    !rules_for(provider).is_empty()
}

/// 按 provider 求值。None = 该 provider 没有规则集（不做屏幕检测）。
///
/// 规则按 priority 降序求值，首个命中者胜；平局时先声明者胜（与 herdr 一致）。
/// 一条都没命中回退 idle——blocked 从严，见模块文档。
pub(crate) fn evaluate(provider: &str, snap: &ScreenSnapshot) -> Option<Evaluation> {
    let rules = rules_for(provider);
    if rules.is_empty() {
        return None;
    }
    // 匹配统一在小写文本上做（声明侧写小写字面量）；标题另走原文，它判的是字符集。
    let lower: Vec<String> = snap.lines.iter().map(|line| line.to_lowercase()).collect();
    let title = snap.title.as_deref().unwrap_or("");

    let mut best: Option<&ScreenRule> = None;
    for rule in rules {
        // 平局先声明者胜：只有严格更高的优先级才顶掉已命中的。
        if best.is_some_and(|winner| winner.priority >= rule.priority) {
            continue;
        }
        if matches_rule(rule, &lower, title) {
            best = Some(rule);
        }
    }
    Some(match best {
        Some(rule) if rule.hold => Evaluation::Hold { rule_id: rule.id },
        Some(rule) => Evaluation::Publish(Detection {
            state: rule.state,
            visible: rule.visible,
            rule_id: rule.id,
        }),
        None => Evaluation::Publish(Detection {
            state: ScreenState::Idle,
            visible: false,
            rule_id: FALLBACK_RULE_ID,
        }),
    })
}

fn matches_rule(rule: &ScreenRule, lower: &[String], title: &str) -> bool {
    match rule.region {
        // 标题当作单行区域喂进去，同样**小写**——文本类谓词（Contains 等）全按小写约定
        // 比较（codex 的标题是 "Action Required"，不转就匹配不上）。字符集类谓词
        // （StartsWithCharIn / WordCharIn）另走原文 title 参数，它们判的是 spinner 帧
        // 这类无大小写的符号，不受影响。
        Region::Title => {
            let lower_title = title.to_lowercase();
            eval_matcher(
                &rule.matcher,
                std::slice::from_ref(&lower_title),
                title,
            )
        }
        _ => {
            let region = region_of(rule.region, lower);
            eval_matcher(&rule.matcher, region, title)
        }
    }
}

fn eval_matcher(matcher: &Matcher, region: &[String], title: &str) -> bool {
    match matcher {
        Matcher::Contains(needles) => needles.iter().all(|n| region_contains(region, n)),
        Matcher::AnyContains(needles) => needles.iter().any(|n| region_contains(region, n)),
        Matcher::LineStartsWith(prefixes) => region.iter().any(|line| {
            let rest = line.trim_start();
            prefixes.iter().any(|p| rest.starts_with(p))
        }),
        Matcher::LineStartsWithContaining(prefix, needles) => region.iter().any(|line| {
            let rest = line.trim_start();
            rest.starts_with(prefix) && needles.iter().any(|n| rest.contains(n))
        }),
        Matcher::LineRegex(pattern) => {
            compiled_regex(pattern).is_some_and(|re| region.iter().any(|line| re.is_match(line)))
        }
        Matcher::CharRunAtLeast(set, min) => region.iter().any(|line| {
            let mut run = 0usize;
            line.chars().any(|ch| {
                run = if set.contains(ch) { run + 1 } else { 0 };
                run >= *min
            })
        }),
        Matcher::NumberedOption(number, word) => region
            .iter()
            .any(|line| numbered_option(line, *number, word)),
        Matcher::BareYes => region.iter().any(|line| bare_yes_option(line)),
        Matcher::SoleCharLineIn(set) => region.iter().any(|line| {
            let trimmed = line.trim();
            let mut chars = trimmed.chars();
            matches!((chars.next(), chars.next()), (Some(c), None) if set.contains(c))
        }),
        Matcher::BraillePrefixed(keywords) => region.iter().any(|line| {
            let rest = line.trim_start();
            let stripped = rest.trim_start_matches(|ch: char| ('\u{2800}'..='\u{28FF}').contains(&ch));
            stripped.len() != rest.len()
                && keywords
                    .iter()
                    .any(|keyword| stripped.trim_start().starts_with(keyword))
        }),
        Matcher::StartsWithCharIn(set) => {
            let mut chars = title.chars();
            matches!((chars.next(), chars.next()), (Some(c), Some(' ')) if set.contains(c))
        }
        Matcher::StartsWithCharInRange(lo, hi) => {
            let mut chars = title.chars();
            matches!((chars.next(), chars.next()), (Some(c), Some(' ')) if (*lo..=*hi).contains(&c))
        }
        Matcher::WordCharIn(set) => word_bounded_char(title, set),
        Matcher::NonEmpty => region.iter().any(|line| !line.trim().is_empty()),
        Matcher::All(inner) => inner.iter().all(|m| eval_matcher(m, region, title)),
        Matcher::Any(inner) => inner.iter().any(|m| eval_matcher(m, region, title)),
        Matcher::Not(inner) => !inner.iter().any(|m| eval_matcher(m, region, title)),
    }
}

// ---------------------------------------------------------------------------
// 区域锚定（语义对齐 herdr manifest.rs 的同名函数）
// ---------------------------------------------------------------------------

fn region_of(region: Region, lines: &[String]) -> &[String] {
    match region {
        Region::WholeScreen | Region::Title => lines,
        Region::BottomNonEmpty(n) => bottom_non_empty_slice(lines, n),
        Region::AfterLastRule => after_last_horizontal_rule(lines),
        Region::PromptBoxBody => prompt_box_body(lines).unwrap_or(&[]),
        Region::AfterLastPromptMarker => after_last_prompt_marker(lines),
    }
}

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

/// codex 的提示符标记行（`›` 独占或 `› ` 开头）。
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

/// 底部 N 个非空行。返回**切片**而非拷贝：从最后一个非空行往回数 N 个非空行，
/// 取其间的连续区间（中间夹的空行无妨，谓词都跳空白）。
fn bottom_non_empty_slice(lines: &[String], n: usize) -> &[String] {
    let mut seen = 0;
    let mut start = lines.len();
    for (index, line) in lines.iter().enumerate().rev() {
        if !line.trim().is_empty() {
            seen += 1;
            start = index;
            if seen == n {
                break;
            }
        }
    }
    &lines[start..]
}

fn region_contains(region: &[String], needle: &str) -> bool {
    region.iter().any(|line| line.contains(needle))
}

/// 取（并缓存）一条规则正则的编译结果。
///
/// 规则来自插件的 `&'static` 声明，条数固定且很少，故编译一次长期复用——检测每 300ms
/// 跑一轮，每轮重编译几十条正则是纯浪费。缓存键就是模式串本身（同一 `&'static str`）。
///
/// 编译失败返回 None（该条规则不命中），**绝不 panic**：规则是数据，写坏一条不该让
/// 整个状态检测停摆；下面有测试保证内置规则全部可编译。
fn compiled_regex(pattern: &str) -> Option<regex::Regex> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<regex::Regex>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().ok()?;
    cache
        .entry(pattern.to_string())
        .or_insert_with(|| {
            // 统一大小写不敏感：规则声明侧写小写字面量，与其它谓词的约定一致。
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()
        })
        .clone()
}

/// `^\s*❯?\s*yes\b`（行已小写）。
fn bare_yes_option(line: &str) -> bool {
    let rest = line.trim_start();
    let rest = rest.strip_prefix('\u{276F}').map_or(rest, str::trim_start);
    starts_with_word(rest, "yes")
}

/// `^\s*❯?\s*N\.\s*<word>\b`（行已小写）。
fn numbered_option(line: &str, number: char, word: &str) -> bool {
    let rest = line.trim_start();
    let rest = rest.strip_prefix('\u{276F}').map_or(rest, str::trim_start);
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

/// 文本里是否有 `set` 中的某个字符**独立成词**出现（两侧是空格或文本边界）。
fn word_bounded_char(text: &str, set: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.iter().enumerate().any(|(index, ch)| {
        set.contains(*ch)
            && index.checked_sub(1).is_none_or(|prev| chars[prev] == ' ')
            && chars.get(index + 1).is_none_or(|next| *next == ' ')
    })
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
        if det.state == ScreenState::Idle
            && !det.visible
            && self.published == Some(ScreenState::Working)
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

    /// 引擎不认识任何具体 agent：规则全部来自插件声明，宿主侧不得再出现
    /// `match provider`（registry.rs 的既定原则：加 agent 只动 `plugins/`）。
    /// 这条断言钉住「新 agent 声明了规则就自动生效，不必改引擎」。
    #[test]
    fn rules_come_from_plugins_not_a_host_side_match() {
        let source = include_str!("detect.rs");
        let engine = source.split("mod tests").next().expect("引擎段");
        for id in ["claude", "codex", "kimi", "gemini", "opencode"] {
            assert!(
                !engine.contains(&format!("\"{id}\"")),
                "引擎里不该出现 agent 身份串 {id:?}——规则归插件声明"
            );
        }
        // 反向证明这条约束是活的：注册表里声明了规则的插件，引擎立刻认得。
        let declared: Vec<&str> = meowo_agent::all()
            .iter()
            .filter(|plugin| !plugin.screen_rules().is_empty())
            .map(|plugin| plugin.id().as_str())
            .collect();
        assert!(
            declared.len() >= 3,
            "至少 claude/codex/kimi 三家声明了规则，实得 {declared:?}"
        );
        for id in declared {
            assert!(provider_supported(id), "{id} 声明了规则却未被引擎认出");
        }
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

    /// codex 的状态行在不同版本/终端宽度下，`•` 与 `Working` 之间可能不止一个空格
    /// （herdr 原始规则用 `\s+`）。写死单空格会让这些屏幕**漏判成 idle**——而漏判
    /// working 的后果是卡片显示空闲、通知也不发，用户以为它停了。
    #[test]
    fn codex_working_line_tolerates_extra_spaces() {
        for line in [
            " \u{2022} Working (3m 12s \u{00B7} esc to interrupt)",
            " \u{2022}  Working (3m 12s \u{00B7} esc to interrupt)",
            " \u{2022}   Working (12s \u{00B7} esc to interrupt) \u{00B7} 42 tokens",
        ] {
            let s = snap(&["  compiling", line]);
            assert_eq!(
                state_of(evaluate("codex", &s)),
                Some((ScreenState::Working, "screen_working_fallback")),
                "应识别为运行中: {line:?}"
            );
        }
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

    // -- gemini --

    #[test]
    fn gemini_approval_box_is_blocked() {
        let s = snap(&[
            "  │ Apply this change to src/main.rs?",
            "  │ \u{276F} Yes, apply",
            "  │   No",
        ]);
        assert_eq!(
            state_of(evaluate("gemini", &s)),
            Some((ScreenState::Blocked, "apply_or_allow_change"))
        );
    }

    /// 选项指针与 yes 必须在**同一行**：拆成两个谓词会把「某处有 ❯、别处有 yes」的
    /// 普通输出误判成审批（LineStartsWithContaining 存在的理由）。
    #[test]
    fn gemini_pointer_and_yes_must_share_a_line() {
        let same_line = snap(&["  \u{276F} Yes, allow it"]);
        assert_eq!(
            state_of(evaluate("gemini", &same_line)),
            Some((ScreenState::Blocked, "apply_or_allow_change"))
        );
        // 指针行与 yes 分处两行、且无任何审批文案 → 不该判阻塞。
        let split = snap(&["  \u{276F} run the tests", "  the answer is yes"]);
        assert_ne!(
            state_of(evaluate("gemini", &split)).map(|(state, _)| state),
            Some(ScreenState::Blocked)
        );
    }

    #[test]
    fn gemini_esc_to_cancel_is_working() {
        let s = snap(&["  generating response...", "  (esc to cancel)"]);
        assert_eq!(
            state_of(evaluate("gemini", &s)),
            Some((ScreenState::Working, "esc_cancel_working"))
        );
    }

    // -- opencode --

    #[test]
    fn opencode_permission_panel_is_blocked() {
        let titled = snap(&["  \u{25B3} Permission required", "  write src/lib.rs"]);
        assert_eq!(
            state_of(evaluate("opencode", &titled)),
            Some((ScreenState::Blocked, "permission_required"))
        );
        // 无标题时要三类快捷键同时在场。
        let by_hints = snap(&["  some panel", "  ↑↓ select · enter confirm · esc dismiss"]);
        assert_eq!(
            state_of(evaluate("opencode", &by_hints)),
            Some((ScreenState::Blocked, "permission_required"))
        );
        // 只有 dismiss 提示的普通浮层不算审批。
        let plain = snap(&["  some overlay", "  esc dismiss"]);
        assert_ne!(
            state_of(evaluate("opencode", &plain)).map(|(state, _)| state),
            Some(ScreenState::Blocked)
        );
    }

    #[test]
    fn opencode_progress_and_interrupt_are_working() {
        let bar = snap(&["  building", "  \u{25A0}\u{25A0}\u{25A0}\u{25A0}\u{25A0} 62%"]);
        assert_eq!(
            state_of(evaluate("opencode", &bar)),
            Some((ScreenState::Working, "progress_bar_working"))
        );
        // 不足 4 连的方块不算进度条（避免命中普通符号）。
        let short = snap(&["  \u{25A0}\u{25A0} bullet"]);
        assert_eq!(
            state_of(evaluate("opencode", &short)),
            Some((ScreenState::Idle, FALLBACK_RULE_ID))
        );
        let hint = snap(&["  thinking (esc to interrupt)"]);
        assert_eq!(
            state_of(evaluate("opencode", &hint)),
            Some((ScreenState::Working, "interrupt_hint_working"))
        );
    }

    /// 所有内置规则的正则必须可编译。求值层遇到坏正则是「整条规则跳过」（不 panic），
    /// 那对线上是对的容错，但会让写错的规则**静默失效**——只能靠这条测试当场发现。
    #[test]
    fn every_declared_regex_compiles() {
        fn walk(matcher: &meowo_agent::screen::Matcher, out: &mut Vec<&'static str>) {
            use meowo_agent::screen::Matcher::*;
            match matcher {
                LineRegex(pattern) => out.push(pattern),
                All(inner) | Any(inner) | Not(inner) => {
                    inner.iter().for_each(|m| walk(m, out));
                }
                _ => {}
            }
        }
        let mut checked = 0;
        for plugin in meowo_agent::all() {
            for rule in plugin.screen_rules() {
                let mut patterns = Vec::new();
                walk(&rule.matcher, &mut patterns);
                for pattern in patterns {
                    assert!(
                        compiled_regex(pattern).is_some(),
                        "{}／{} 的正则编译失败：{pattern}",
                        plugin.id().as_str(),
                        rule.id
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "应至少有一条规则用了正则");
    }

    /// 规则的「同行耦合」与「整行等值」不能退化成 region 级的分散包含——
    /// 屏幕上别处出现同样的词很常见，退化后会把普通输出误判成需要关注的状态。
    /// 三条均由代码审查发现，此处用它给出的失败场景钉住。
    #[test]
    fn rules_require_same_line_evidence_not_scattered_words() {
        // claude /btw：`/btw` 后须是空白或行尾，提示须在行尾。
        let fake_btw = snap(&[
            "  /btwhatever is this",
            "  press esc to close this dialog and continue",
        ]);
        assert_ne!(
            state_of(evaluate("claude", &fake_btw)).map(|(state, _)| state),
            Some(ScreenState::Working),
            "/btwhatever 与句中的 esc to close 不构成 /btw 覆盖层"
        );

        // kimi 提问面板：标题行必须整行等于 question。
        let fake_question = snap(&[
            " questions about the design",
            " ? which one",
            " ↑↓ select \u{00B7} \u{21B5} choose \u{00B7} esc cancel",
        ]);
        assert_ne!(
            state_of(evaluate("kimi", &fake_question)).map(|(state, _)| state),
            Some(ScreenState::Blocked),
            "「questions about…」不是提问面板的标题行"
        );

        // codex 状态行：Working 与 esc to interrupt 必须同行。
        let split_lines = snap(&[
            " \u{2022} Working (3m 12s)",
            "  earlier output mentioning esc to interrupt",
        ]);
        assert_ne!(
            state_of(evaluate("codex", &split_lines)).map(|(state, _)| state),
            Some(ScreenState::Working),
            "中断提示在别行时不算当前状态行"
        );
        // 同行的真状态行仍然识别。
        let real = snap(&[" \u{2022}  Working (3m 12s \u{00B7} esc to interrupt)"]);
        assert_eq!(
            state_of(evaluate("codex", &real)),
            Some((ScreenState::Working, "screen_working_fallback"))
        );
    }

    /// kimi 的审批标题行必须**以问号结尾**（原始规则 `^\s*▶?\s*approve .*\?$`）。
    /// 漏掉这个约束会把「approve this plan」这类普通输出误报成等待审批——而 blocked
    /// 误报是最坏方向：弹通知说 agent 在等你，实际它跑得好好的。
    #[test]
    fn kimi_approve_title_requires_a_question_mark() {
        let panel = |title: &str| {
            snap(&[
                title,
                " \u{25B6} Approve once   Reject",
                " 1/2/3 choose \u{00B7} \u{21B5} confirm",
            ])
        };
        // 真实审批标题：问号结尾 → blocked。
        assert_eq!(
            state_of(evaluate("kimi", &panel(" \u{25B6}  Approve this command?"))),
            Some((ScreenState::Blocked, "current_approval_panel")),
            "▶ 与 approve 之间多个空格也应识别"
        );
        // 普通输出：无问号 → 不得判 blocked。
        let plain = snap(&[
            " I will approve this plan and continue",
            " \u{25B6} Approve once   Reject",
            " 1/2/3 choose \u{00B7} \u{21B5} confirm",
        ]);
        assert_ne!(
            state_of(evaluate("kimi", &plain)).map(|(state, _)| state),
            Some(ScreenState::Blocked),
            "没有问号的 approve 句子不是审批标题"
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
