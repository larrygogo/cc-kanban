//! Gemini CLI 的屏幕状态规则（语义移植自 herdr gemini.toml 2026.06.10.1，Apache-2.0）。
//!
//! gemini 的 TUI 把交互框画成带 `│` 竖线的盒子，审批文案就嵌在盒内；运行态则靠
//! 「esc to cancel」提示。规则比 claude 少得多——它的状态信号本来就稀疏，宁可只认
//! 这几条确凿的，也不为覆盖率编造弱规则（失准要往少报 blocked 的方向偏）。

use crate::screen::{Matcher, Region, ScreenRule, ScreenState};

pub(super) static RULES: &[ScreenRule] = &[
    // 运行中的加载指示行（gemini 实际形态）：`⠧ Reticulating splines... (esc to cancel, 12s)`
    // ——盲文 spinner 帧与取消提示在**同一行**才是「此刻真在跑」的证据。优先级压过审批
    // 规则(300)：gemini 无标题规则，working 若只有 100 必输，正在跑时屏幕上残留/回显的
    // 审批词会把运行中翻成待交互（失准只许少报 blocked，不许误报）。真在等确认时
    // gemini 会显示 waiting for user confirmation——not 门放行，真审批不被本条吃掉。
    ScreenRule::new(
        "spinner_line_working",
        ScreenState::Working,
        400,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::LineRegex(r"^\s*[\x{2800}-\x{28FF}]\s.*\(esc to cancel"),
            Matcher::Not(&[Matcher::Contains(&["waiting for user confirmation"])]),
        ]),
    )
    .visible(),
    ScreenRule::new(
        "apply_or_allow_change",
        ScreenState::Blocked,
        300,
        Region::WholeScreen,
        Matcher::Any(&[
            // 盒内标题行：`│` 前缀就是 gemini 提示框的左边框，自带锚定。
            Matcher::Contains(&["│ apply this change"]),
            Matcher::Contains(&["│ allow execution"]),
            // proceed 问句绝不单独作证（正文/回显引用它的场合太多），必须同屏配上
            // 真实的选项行：编号 yes 选项（`│ ● 1. Yes, allow once` / `❯ 1. Yes`），
            // yes 按**词边界**匹配——裸子串会命中 eyes/yesterday（曾是整屏分散包含
            // 的形态，正是 detect 的 rules_require_same_line_evidence 测试要禁的）。
            Matcher::All(&[
                Matcher::AnyContains(&[
                    "waiting for user confirmation",
                    "│ do you want to proceed",
                    "do you want to proceed?",
                ]),
                Matcher::Any(&[
                    Matcher::LineRegex(
                        r"^\s*│?\s*(?:[\x{276F}\x{25CF}\x{25CB}]\s*)?\d+\.\s*yes\b",
                    ),
                    Matcher::LineStartsWithContaining("\u{276F}", &["yes", "allow"]),
                ]),
            ]),
            // 选项指针行本身带 yes/allow —— 拆成两个谓词会误命中「指针在 A 行、
            // yes 在别处」的普通输出。
            Matcher::LineStartsWithContaining("\u{276F}", &["yes", "allow"]),
        ]),
    )
    .visible(),
    // 散落的 esc to cancel 兜底（spinner 行被截/丢帧时仍能判运行中）；优先级最低，
    // 输给上面一切更锚定的证据。
    ScreenRule::new(
        "esc_cancel_working",
        ScreenState::Working,
        100,
        Region::WholeScreen,
        Matcher::Contains(&["esc to cancel"]),
    )
    .visible(),
];
