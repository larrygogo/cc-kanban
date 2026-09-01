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
            // 标题行是**整行等于** "question"（原始 `^\s*question\s*$`）。写成前缀会让
            // 「questions about the design」这类正文在其余条件齐备时误判成等待回答。
            Matcher::LineRegex(r"^\s*question\s*$"),
            Matcher::LineRegex(r"^\s*\? "),
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
        // 三个词必须在**同一行**且含 `[N agents running]` 计数（原始
        // `\bkimi[-\w.]*\s+thinking\b.*\[[1-9][0-9]*\s+agents?\s+running\]`）。
        // 拆成 region 级的分散包含会把「屏幕别处有 kimi、另一处有 thinking」误判成运行中。
        Matcher::LineRegex(r"\bkimi[-\w.]*\s+thinking\b.*\[[1-9][0-9]*\s+agents?\s+running\]"),
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
    // 当前版 TUI 的工作状态行：盲文 spinner + `<Agent> Running (…)`（实拍：
    // `⠴ Explore Agent Running (…) · 72 tools · 30m 38s`）。agent 名不定，只能锚
    // 「行首盲文 + running (」；盲文前缀本身只在 spinner 行出现，误报面可控。
    ScreenRule::new(
        "agent_running_status_working",
        ScreenState::Working,
        95,
        Region::WholeScreen,
        Matcher::LineRegex(r"^\s*[\u{2800}-\u{28FF}].*\brunning\s*\("),
    )
    .visible(),
    // 当前版 TUI 工作中的月相提示行：`🌘 · Tip: …`。旧规则只认**独立成行**的月相
    // 字符，新形态是「月相 + 空格 + ·」行首。blocked 规则优先级都在 300+，审批/提问
    // 面板同屏时不会被这条盖掉。
    ScreenRule::new(
        "moon_tip_status_working",
        ScreenState::Working,
        94,
        Region::WholeScreen,
        Matcher::LineRegex(r"^\s*[🌕🌖🌗🌘🌑🌒🌓🌔]\s+·\s"),
    )
    .visible(),
    // 月相**紧贴**状态词的工作行：实拍 `🌗Working...`（swarm 模式下常驻整块工作屏）。
    // 上面两条月相规则一条要求月相独占整行、一条要求「月相 空格 · 空格」，紧贴形态两
    // 头不沾——整块正在干活的屏落进 fallback idle，角标被降成「认不出」的中性灰点、
    // tab 归属还掉进「待交互」（2026-08-31 实拍回归）。
    //
    // 从严的地方在**状态词白名单**：不放宽成「月相开头的任意行」。月相是普通 emoji，
    // AI 正文里出现一个就会把等你输入的会话谎报成在跑——那正是 fallback idle 想避免
    // 的反方向失准。
    ScreenRule::new(
        "moon_word_status_working",
        ScreenState::Working,
        93,
        Region::WholeScreen,
        Matcher::LineRegex(
            r"^\s*[🌕🌖🌗🌘🌑🌒🌓🌔]\s*(?:·\s*)?(?:working|thinking|using|running|compacting)\b",
        ),
    )
    .visible(),
    // 空闲输入框（0.29 源码取证，`custom-editor.ts`）：`injectPromptSymbol` 把提示符
    // 写进首条内容行的第 2 列（普通模式 `>`、bash 模式 `!`），`wrapWithSideBorders`
    // 再把 0 列叠上 `│` 边框——行呈 `│ > ` / `│ ! `。转录里的用户消息走 bullet
    // （user-message.ts），不带这个边框，不会撞车。
    //
    // 本条补的是 kimi 规则集**没有 idle 正向证据**的缺口：此前空闲屏一条规则都不命中，
    // 恒落 FALLBACK_RULE_ID——卡片挂着「认不出来」的中性灰点，而不是有根据的「等你输入」
    // （2026-09-01 实拍）。优先级压在所有 working/blocked 规则之下：转动的 spinner 与
    // 审批/提问面板同屏时照旧胜；底部 8 个非空行盖住输入框三行（含草稿数行）+ footer
    // 两行。框线 + 提示符是屏幕上确实画着的可见证据，与 claude 的 live_prompt_box 同级。
    ScreenRule::new(
        "composer_prompt_idle",
        ScreenState::Idle,
        80,
        Region::BottomNonEmpty(8),
        Matcher::LineStartsWith(&["│ > ", "│ ! "]),
    )
    .visible(),
];
