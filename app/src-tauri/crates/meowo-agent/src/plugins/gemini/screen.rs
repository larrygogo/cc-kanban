//! Gemini CLI 的屏幕状态规则（语义移植自 herdr gemini.toml 2026.06.10.1，Apache-2.0）。
//!
//! gemini 的 TUI 把交互框画成带 `│` 竖线的盒子，审批文案就嵌在盒内；运行态则靠
//! 「esc to cancel」提示。规则比 claude 少得多——它的状态信号本来就稀疏，宁可只认
//! 这几条确凿的，也不为覆盖率编造弱规则（失准要往少报 blocked 的方向偏）。

use crate::screen::{Matcher, Region, ScreenRule, ScreenState};

pub(super) static RULES: &[ScreenRule] = &[
    ScreenRule::new(
        "apply_or_allow_change",
        ScreenState::Blocked,
        300,
        Region::WholeScreen,
        Matcher::Any(&[
            Matcher::Contains(&["│ apply this change"]),
            Matcher::Contains(&["│ allow execution"]),
            Matcher::All(&[
                Matcher::Contains(&["yes"]),
                Matcher::AnyContains(&[
                    "waiting for user confirmation",
                    "│ do you want to proceed",
                    "do you want to proceed?",
                ]),
            ]),
            // 选项指针行本身带 yes/allow —— 拆成两个谓词会误命中「指针在 A 行、
            // yes 在别处」的普通输出。
            Matcher::LineStartsWithContaining("\u{276F}", &["yes", "allow"]),
        ]),
    )
    .visible(),
    ScreenRule::new(
        "esc_cancel_working",
        ScreenState::Working,
        100,
        Region::WholeScreen,
        Matcher::Contains(&["esc to cancel"]),
    )
    .visible(),
];
