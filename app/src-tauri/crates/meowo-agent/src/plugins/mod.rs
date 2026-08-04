//! 每个 agent 一个模块：声明自己的变体表，别处不再有该 agent 的专属分支。
//!
//! # 接入一个新 agent
//!
//! 只动这个目录 + 注册表两行。宿主侧**任何**按 agent 身份分支的代码都是 bug，有两条
//! 守卫盯着（`meowo-app` 的 `host_code_does_not_branch_on_agent_identity` 与前端的
//! `architecture.test.ts`）——写不进去，只能改成能力槽。
//!
//! 1. 建 `plugins/<id>/mod.rs`，实现 [`crate::registry::AgentPlugin`]。**必填只有四项**：
//!    `id`、`display_name`、`variants`（变体表）、`process_names`（判活用的进程名）。
//! 2. 在 `plugins/mod.rs` 加 `pub mod <id>;`，在 `registry.rs` 的 `ALL` 数组加一项。
//! 3. 其余全是能力槽，**按取证到的程度逐个补**——不声明就整块降级，不会崩：
//!    - `resume_args`：能恢复会话
//!    - `telemetry`：解析 transcript（对话正文、模型、上下文占用）
//!    - `screen_rules`：屏幕状态检测（见 [`crate::screen`]）
//!    - `attention_patterns`：可操作提示的识别（见 [`crate::chat_ui::AttentionPattern`]）
//!    - `permission_hook_decides`：hook 的审批决策会被该 agent 采纳
//!    - `cross_account_session`：会话可搬到另一账号继续（见 [`crate::profile`]）
//!    - `profile` / `account` / `proxy` / `relay`：多账号、登录态、代理、中转
//!
//! # 为什么默认值都是「不支持」
//!
//! 能力槽的默认实现一律是最保守的那个（`None` / 空表 / `false`），这不是「还没做」的
//! 占位，而是安全默认：
//!
//! - 屏幕规则空 = 不做检测，好过用没取证的规则误报「agent 在等你」；
//! - `permission_hook_decides` 为 false = 不接管审批，好过弹一张点了没用的卡；
//! - `cross_account_session` 为 None = 不搬会话，好过搬出一份 agent 自己读不了的副本。
//!
//! 共同的取舍是**宁可少一块功能，不给一条走不通的路**——后者用户会反复去点，并以为
//! 是自己的问题。声明任何一项之前请先有该 agent 的真机取证。
//!
//! # 规则类能力的取证
//!
//! `screen_rules` / `attention_patterns` 匹配的是各家 TUI 的界面文案，必然随对方改版
//! 腐坏。宿主提供了两个诊断命令：`screen_detect_explain`（对活会话）与
//! `screen_detect_explain_text`（对一段末屏文本，不需要复现现场）。**照着真实屏幕改，
//! 不要凭想象改**——已经因此栽过：手工枚举 spinner 字符集漏了六帧、把 `\s+` 写成单个
//! 空格、漏掉审批行必须以 `?` 结尾的约束，每一处都表现为状态悄悄判错而无人察觉。

pub mod claude;
pub mod codex;
pub mod gemini;
pub mod kimi;
pub mod opencode;
