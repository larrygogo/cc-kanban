//! Kimi 的屏幕状态规则（语义移植自 herdr kimi.toml 2026.06.10.1，Apache-2.0）。
//!
//! kimi 的 `PermissionRequest` hook 在 Meowo 里是 observation-only（`permission_hook_decides`
//! 为 false，理由见 registry 的注释）——屏幕检测因此是它「在等审批/等回答」的**唯一**
//! 实时来源，规则失准对它的代价比对 claude 更高。

use crate::screen::{Matcher, Region, ScreenRule, ScreenState};

/// 月相 spinner，独立成行出现。
const MOON_SPINNER: &str = "🌕🌖🌗🌘🌑🌒🌓🌔";

pub(super) static RULES: &[ScreenRule] = &[
    // 当前版审批面板：确认键 + 具体问句 + 选项行三者齐备。
    ScreenRule::new(
        "current_approval_panel",
        ScreenState::Blocked,
        400,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::Contains(&["↵ confirm"]),
            Matcher::Any(&[
                Matcher::Contains(&["run this command?"]),
                Matcher::Contains(&["write this file?"]),
                Matcher::Contains(&["apply these edits?"]),
                Matcher::Contains(&["stop this task?"]),
                Matcher::Contains(&["ready to build with this plan?"]),
                // 逐字对齐原始规则 `^\s*▶?\s*approve .*\?$`：▶ 可选、其后空白不定，
                // 且**必须以 ? 结尾**。此前写成前缀匹配漏掉了问号约束——"approve this
                // plan" 这类普通输出会被误报成等待审批（blocked 误报是最坏方向：会弹
                // 通知说 agent 在等你，实际没有）。
                Matcher::LineRegex(r"^\s*\u{25B6}?\s*approve .*\?\s*$"),
            ]),
            Matcher::Contains(&[" choose"]),
            Matcher::AnyContains(&["approve", "reject", "revise"]),
        ]),
    )
    .visible(),
    // 提问面板。
    ScreenRule::new(
        "question_panel",
        ScreenState::Blocked,
        390,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::Contains(&["↑↓ select", "esc cancel"]),
            Matcher::LineStartsWith(&["question"]),
            Matcher::LineStartsWith(&["? "]),
            Matcher::AnyContains(&["↵ choose", "↵ toggle", "↵ save"]),
        ]),
    )
    .visible(),
    // 旧版审批面板：弱证据，不带 visible。
    ScreenRule::new(
        "legacy_approval_panel",
        ScreenState::Blocked,
        300,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::Contains(&["requesting approval", "reject"]),
            Matcher::AnyContains(&["approve once", "approve for this session"]),
            Matcher::AnyContains(&["1/2/3/4 choose", "↵ confirm"]),
        ]),
    ),
    // `kimi thinking … [N agents running]` 状态行。
    ScreenRule::new(
        "background_agent_status_working",
        ScreenState::Working,
        120,
        Region::BottomNonEmpty(3),
        Matcher::Contains(&["kimi", "thinking", "running]"]),
    )
    .visible(),
    ScreenRule::new(
        "moon_spinner_working",
        ScreenState::Working,
        100,
        Region::WholeScreen,
        Matcher::SoleCharLineIn(MOON_SPINNER),
    )
    .visible(),
    ScreenRule::new(
        "braille_spinner_working",
        ScreenState::Working,
        90,
        Region::WholeScreen,
        Matcher::BraillePrefixed(&["thinking...", "working...", "using "]),
    )
    .visible(),
];
