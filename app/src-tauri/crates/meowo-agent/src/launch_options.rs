//! 新建会话的启动选项：插件把「用户可选的启动形态」声明成**选择 → CLI flag** 的映射表，
//! 前端照表渲染，宿主照表翻译——用户的选择**永远不会**直接变成命令行参数，能进 argv 的
//! 只有插件声明过的字面量。
//!
//! 这与斜杠命令同一条原则：flag 表是 agent 的事实（claude 是 `--permission-mode plan`，
//! codex 是 `--full-auto`，gemini 是 `--yolo`），归插件声明，GUI 零知识。未声明的 agent
//! 面板上就没有这一栏——不给一个点了没效果的下拉。

use serde::Serialize;
use std::collections::HashMap;

/// 一个可选值。`label` 是产品词（"Opus" / "Plan"），不翻译；细文案由前端 i18n 按
/// `<option>.<choice>` 取，取不到回退 `label`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct LaunchChoice {
    pub id: &'static str,
    pub label: &'static str,
    /// 追加到启动 argv 的参数（字面量，非模板）。默认项必须为空——「默认」的诚实含义是
    /// 「不传任何 flag，行为由 CLI 自己决定」，而不是我们替 CLI 猜一个默认值。
    pub args: &'static [&'static str],
    /// 高风险档（跳过权限确认/拆沙箱一类）。true 时前端以警示色渲染并附风险副标题——
    /// 与普通档同视觉零警示的话，用户会把它当成无害的快捷档随手点上。
    pub risk: bool,
}

/// 一栏启动选项（单选）。
#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct LaunchOption {
    /// 稳定标识（"model" / "approval" / "permission"）。前端按它取组标签文案，
    /// 宿主按它对齐前端传回的选择。
    pub id: &'static str,
    pub choices: &'static [LaunchChoice],
    /// 默认选中的 choice id。
    pub default: &'static str,
}

/// 把前端传回的选择（option id → choice id）翻译成追加的 argv 片段。
///
/// 只认声明表里的组合：未知 option id 被忽略（不是这张表的键就不该影响 argv），未知
/// choice id 落回该选项的默认——**任何情况下都不把用户输入原样拼进命令行**。
pub fn resolve_launch_args(
    options: &[LaunchOption],
    selections: &HashMap<String, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for opt in options {
        let picked = selections
            .get(opt.id)
            .map(String::as_str)
            .unwrap_or(opt.default);
        let choice = opt
            .choices
            .iter()
            .find(|c| c.id == picked)
            .or_else(|| opt.choices.iter().find(|c| c.id == opt.default));
        if let Some(c) = choice {
            out.extend(c.args.iter().map(|s| s.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTIONS: &[LaunchOption] = &[
        LaunchOption {
            id: "model",
            default: "default",
            choices: &[
                LaunchChoice {
                    id: "default",
                    label: "Default",
                    args: &[],
                    risk: false,
                },
                LaunchChoice {
                    id: "opus",
                    label: "Opus",
                    args: &["--model", "opus"],
                    risk: false,
                },
            ],
        },
        LaunchOption {
            id: "permission",
            default: "default",
            choices: &[
                LaunchChoice {
                    id: "default",
                    label: "Default",
                    args: &[],
                    risk: false,
                },
                LaunchChoice {
                    id: "plan",
                    label: "Plan",
                    args: &["--permission-mode", "plan"],
                    risk: false,
                },
            ],
        },
    ];

    fn sel(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolves_declared_choices_in_option_order() {
        let args = resolve_launch_args(OPTIONS, &sel(&[("permission", "plan"), ("model", "opus")]));
        assert_eq!(args, vec!["--model", "opus", "--permission-mode", "plan"]);
    }

    #[test]
    fn missing_selection_falls_to_default_which_adds_nothing() {
        assert!(resolve_launch_args(OPTIONS, &sel(&[])).is_empty());
    }

    /// 注入防线：未知 option 被忽略；未知 choice 落回默认。用户输入在任何分支下都
    /// 不会出现在返回值里。
    #[test]
    fn unknown_ids_never_reach_argv() {
        let evil = sel(&[
            ("model", "opus; rm -rf /"),
            ("--dangerously-skip-permissions", "yes"),
        ]);
        assert!(resolve_launch_args(OPTIONS, &evil).is_empty());
    }
}
