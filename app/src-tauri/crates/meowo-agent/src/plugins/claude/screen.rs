//! Claude Code 的屏幕状态规则（语义移植自 herdr claude.toml 2026.07.13.1，Apache-2.0；
//! CC 2.1.233 的运行形态变更按 2026-08-17 ConPTY 实拍补充——标题 spinner 从盲文帧换成
//! 月相帧 ◐◑，OSC 9;4 进度序列不再发送，状态行括号里的中断提示被移除）。
//!
//! 改这里之前请先取证：宿主的 `screen_detect_explain` / `screen_detect_explain_text`
//! 会打印真实末屏与命中规则，照着真实屏幕改，不要凭想象改。

use crate::screen::{Matcher, Region, ScreenRule, ScreenState};

/// claude 的 spinner 帧写在终端标题里：盲文字符 + 空格开头。
///
/// 用**整个盲文区间**而不是枚举某个十帧序列——原始规则就是 `^[\x{2800}-\x{28FF}] `。
/// 手工枚举必然漏帧：曾把上界写成 U+281F，于是 ⠹⠸⠼⠴⠦⠧ 六帧转到时状态闪回空闲。
const BRAILLE_LO: char = '\u{2800}';
const BRAILLE_HI: char = '\u{28FF}';

/// 审批表单的导航提示，各版本文案不同，任一出现即可。
const NAV_HINTS: &[&str] = &[
    "tab/arrow keys to navigate",
    "arrow keys to navigate",
    "arrows to navigate",
    "↑/↓ to navigate",
    "↑↓ to navigate",
];

pub(super) static RULES: &[ScreenRule] = &[
    // 标题带 spinner 帧 = 正在跑，优先级最高：它压过屏幕上残留的一切文本。
    ScreenRule::new(
        "osc_title_working",
        ScreenState::Working,
        1100,
        Region::Title,
        Matcher::StartsWithCharInRange(BRAILLE_LO, BRAILLE_HI),
    )
    .visible(),
    // CC 2.1.233（2026-08 ConPTY 实拍取证）标题 spinner 换成月相圆形帧：运行中
    // `◐ Claude Code` / `◑ <任务名>`，结束回到 `✳ <任务名>`。上一条盲文规则在新版
    // 永不命中，正在跑的会话于是被 ❯ 提示框（live_prompt_box）判成可见 idle——
    // 看板黄环「等你」错挂整个长任务，正是「状态不及时」实拍反馈的根因。
    // 区间取整段半圆/四分圆族（实拍见 ◐◑，旋转帧自然含 ◒◓）：手工枚举漏帧的教训
    // 见 BRAILLE 常量注释。
    ScreenRule::new(
        "osc_title_moon_working",
        ScreenState::Working,
        1090,
        Region::Title,
        Matcher::StartsWithCharInRange('\u{25D0}', '\u{25D7}'),
    )
    .visible(),
    // ctrl+o 的详细 transcript 覆盖层：屏幕全被它占据，不代表状态变化。
    ScreenRule::new(
        "transcript_viewer",
        ScreenState::Idle,
        1000,
        Region::BottomNonEmpty(3),
        Matcher::All(&[
            Matcher::Contains(&["showing detailed transcript"]),
            Matcher::Any(&[
                Matcher::Contains(&["ctrl+o", "to toggle"]),
                Matcher::Contains(&["ctrl+e", "show all"]),
                Matcher::Contains(&["ctrl+e", "collapse"]),
                Matcher::Contains(&["↑↓ scroll"]),
                Matcher::Contains(&["? for shortcuts"]),
            ]),
        ]),
    )
    .hold(),
    // 分隔线之后的选择型审批表单：确认 + 取消 + 导航提示三者齐备才算数。
    ScreenRule::new(
        "live_blocked_form",
        ScreenState::Blocked,
        980,
        Region::AfterLastRule,
        Matcher::All(&[
            Matcher::Contains(&["enter to select", "esc to cancel"]),
            Matcher::AnyContains(NAV_HINTS),
        ]),
    )
    .visible(),
    ScreenRule::new(
        "dynamic_workflow_prompt",
        ScreenState::Blocked,
        980,
        Region::WholeScreen,
        Matcher::Contains(&["run a dynamic workflow?", "esc to cancel"]),
    )
    .visible(),
    // /btw 覆盖层挂着说明后台仍在跑。
    ScreenRule::new(
        "btw_overlay_working",
        ScreenState::Working,
        975,
        Region::BottomNonEmpty(5),
        // 逐字对齐原始规则 `^\s*/btw(?:\s|$)` 与 `esc to close\s*$`：
        // `/btw` 后必须是空白或行尾（否则 `/btwhatever` 也命中），提示必须在**行尾**
        // （否则「press esc to close this dialog」这类正文会把空闲误判成运行中——
        // 该规则 975 优先级，会压过提示框的 idle 判定）。
        Matcher::All(&[
            Matcher::LineRegex(r"^\s*/btw(?:\s|$)"),
            Matcher::LineRegex(r"esc to close\s*$"),
        ]),
    )
    .visible(),
    // 运行中的屏幕状态行（CC 2.1.233 实拍）：`✢ Brewing… (7s · ↓ 129 tokens)`。
    // 帧字符开头 + `… (` + 计时数字三者齐备才算——回合结束行 `✻ Baked for 11s`
    // 没有括号计时，不命中。标题月相是第一证据，本条兜屏幕侧（用户自设终端标题、
    // 标题事件丢失时仍能判对）；970 压过 live_prompt_box（950）：运行中输入框照画
    // ❯，不能再被它判成可见 idle。注意新版括号里没有 esc to interrupt（该提示已被
    // 上游移除），不能沿用 codex/opencode 的中断提示锚。
    // 字符类只收真实 spinner 帧（· ✢ ✳ ✶ ✻ ✽，实拍取证）：`*`/`+` 是最常见的
    // Markdown 列表符，不是帧——收进来的话，已答完的末行
    // `* 已修复三处… (2 个文件)` 会让空闲会话永远显示「运行中」（实拍回归）。
    ScreenRule::new(
        "status_line_working",
        ScreenState::Working,
        970,
        Region::BottomNonEmpty(6),
        Matcher::LineRegex(r"^[·✢✳✶✻✽]\s+\S.*(…|\.\.\.)\s*\(\d+"),
    )
    .visible(),
    // 提示框体内以 ❯ 开头且无任何审批文案 = 在等你输入的空闲态（可见证据）。
    ScreenRule::new(
        "live_prompt_box",
        ScreenState::Idle,
        950,
        Region::PromptBoxBody,
        Matcher::All(&[
            Matcher::LineStartsWith(&["\u{276F}"]),
            Matcher::Not(&[
                Matcher::Contains(&["enter to select"]),
                Matcher::Contains(&["esc to cancel"]),
                Matcher::Contains(&["tab/arrow keys"]),
                Matcher::Contains(&["arrow keys to navigate"]),
                Matcher::Contains(&["↑/↓ to navigate"]),
                // 计划批准对话框没有上面任何一条按键提示,选项行又以 ❯ 开头——不否决
                // 就会被本条(950,可见 idle)压过 plan_approval_prompt(830,blocked)。
                Matcher::Contains(&["written up a plan"]),
            ]),
        ]),
    )
    .visible(),
    // 模型选单覆盖层。
    ScreenRule::new(
        "model_picker_menu",
        ScreenState::Idle,
        900,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::Contains(&["select model", "enter to set as default", "esc to cancel"]),
            Matcher::Not(&[
                Matcher::Contains(&["do you want to proceed?"]),
                Matcher::Contains(&["enter to select"]),
            ]),
        ]),
    )
    .hold(),
    // bash 权限弹窗：问句 + bash 证据 + 编号选项行，三重 AND 防误报。
    ScreenRule::new(
        "bash_permission_prompt",
        ScreenState::Blocked,
        850,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::Contains(&["do you want to proceed?"]),
            Matcher::AnyContains(&[
                "bash command",
                "bash(",
                "contains expansion",
                "tab to amend",
                "ctrl+e to explain",
            ]),
            Matcher::Any(&[
                Matcher::BareYes,
                Matcher::NumberedOption('1', "yes"),
                Matcher::NumberedOption('2', "no"),
            ]),
        ]),
    )
    .visible(),
    ScreenRule::new(
        "generic_permission_prompt",
        ScreenState::Blocked,
        840,
        Region::AfterLastRule,
        Matcher::All(&[
            Matcher::Contains(&["do you want to proceed?", "esc to cancel"]),
            Matcher::Any(&[
                Matcher::NumberedOption('1', "yes"),
                Matcher::NumberedOption('2', "yes"),
                Matcher::NumberedOption('2', "no"),
                Matcher::NumberedOption('3', "no"),
            ]),
        ]),
    )
    .visible(),
    // 计划模式批准提示（2.1.227 plan-file 流程，实拍取证）：
    //   Claude has written up a plan and is ready to execute. Would you like to proceed?
    //   ❯ 1. Yes, and bypass permissions / 2. Yes, manually approve edits / …
    // 它**不触发**任何 hook（上游回归，PreToolUse:ExitPlanMode 与 PermissionRequest 都
    // 缺席），没有 pending_review 兜底，本条是「待交互」状态的唯一来源。指纹拆成两段
    // 短子串（region_contains 逐行匹配，整句在窄终端会折行）再叠编号 Yes 项，问句
    // "would you like to proceed?" 绝不单独作证——正文引用该句的场合太多。
    ScreenRule::new(
        "plan_approval_prompt",
        ScreenState::Blocked,
        830,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::Contains(&["written up a plan", "would you like to proceed?"]),
            Matcher::Any(&[
                Matcher::NumberedOption('1', "yes"),
                Matcher::NumberedOption('2', "yes"),
            ]),
        ]),
    )
    .visible(),
    // 弱证据：刻意**不带** visible——防抖不直通，且不足以压过 hook 的事实源。
    // 空提示符行（独立一行只有 ❯）出现 = 提示框其实空着在等输入，否决之。
    ScreenRule::new(
        "legacy_no_prompt_blocker",
        ScreenState::Blocked,
        300,
        Region::WholeScreen,
        Matcher::All(&[
            Matcher::Any(&[
                Matcher::All(&[
                    Matcher::Contains(&["do you want to"]),
                    Matcher::AnyContains(&["yes", "\u{276F}"]),
                ]),
                Matcher::All(&[
                    Matcher::Contains(&["would you like to"]),
                    Matcher::AnyContains(&["yes", "\u{276F}"]),
                ]),
                Matcher::Contains(&["waiting for permission"]),
                Matcher::Contains(&["do you want to allow this connection?"]),
                Matcher::Contains(&["tab to amend"]),
                Matcher::Contains(&["ctrl+e to explain"]),
                Matcher::Contains(&["do you want to proceed?", "esc to cancel"]),
                Matcher::Contains(&["review your answers"]),
                Matcher::Contains(&["skip interview and plan immediately"]),
            ]),
            Matcher::Not(&[Matcher::SoleCharLineIn("\u{276F}")]),
        ]),
    ),
    // 标题 ✳ 前缀 = 空闲。
    ScreenRule::new(
        "osc_title_idle",
        ScreenState::Idle,
        250,
        Region::Title,
        Matcher::StartsWithCharIn("\u{2733}"),
    )
    .visible(),
    // 进度清零（OSC 9;4;0）= 这一轮结束了。与 osc_title_idle 同级的第二条兜底：
    // 标题未必总带 ✳（用户自设了终端标题、或 CLI 版本不写标题），而进度序列照发。
    ScreenRule::new(
        "osc_progress_idle",
        ScreenState::Idle,
        250,
        Region::Progress,
        Matcher::LineRegex(r"^4;0"),
    ),
];
