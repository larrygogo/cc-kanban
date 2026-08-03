//! OpenCode 的屏幕状态规则（语义移植自 herdr opencode.toml 2026.06.10.1，Apache-2.0）。
//!
//! opencode 的审批面板有醒目的 `△ Permission required` 标题；没有标题时靠底部快捷键
//! 组合（dismiss + confirm/submit/toggle + 选择键）识别。运行态有两种信号：中断提示
//! 与进度条。
//!
//! 屏幕检测对 opencode 的价值高于其它几家：它是目前唯一**没有 `telemetry` 能力**的
//! 插件（不解析 transcript，因而没有对话预览、上下文占用这些遥测信号）。卡片上的
//! 实时状态几乎只能靠这里的规则给出——规则失准对它的代价相应更大。

use crate::screen::{Matcher, Region, ScreenRule, ScreenState};

/// 进度条字符：连续多个即为进行中。
const PROGRESS_BLOCKS: &str = "\u{25A0}\u{2B1D}";

pub(super) static RULES: &[ScreenRule] = &[
    ScreenRule::new(
        "permission_required",
        ScreenState::Blocked,
        300,
        Region::WholeScreen,
        Matcher::Any(&[
            Matcher::Contains(&["\u{25B3} permission required"]),
            // 无标题时的形态：三类快捷键同时在场才算数（单看 "esc dismiss" 会命中
            // 各种非审批的浮层）。
            Matcher::All(&[
                Matcher::Contains(&["esc dismiss"]),
                Matcher::AnyContains(&["enter confirm", "enter submit", "enter toggle"]),
                Matcher::AnyContains(&["↑↓ select", "⇆ tab"]),
            ]),
        ]),
    )
    .visible(),
    ScreenRule::new(
        "interrupt_hint_working",
        ScreenState::Working,
        110,
        Region::WholeScreen,
        Matcher::AnyContains(&[
            "esc to interrupt",
            "ctrl+c to interrupt",
            "press esc to interrupt",
            "esc again to interrupt",
        ]),
    )
    .visible(),
    ScreenRule::new(
        "progress_bar_working",
        ScreenState::Working,
        100,
        Region::WholeScreen,
        Matcher::CharRunAtLeast(PROGRESS_BLOCKS, 4),
    )
    .visible(),
];
