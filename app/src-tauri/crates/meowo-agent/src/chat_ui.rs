//! 对话页能力：由**安装实况**组装，而不是一张写死的表。
//!
//! 「agent 的能力由装的是什么决定」在工程上分三层，可动态化程度不同：
//!
//! 1. **真正可发现的**：CLI 版本（宿主探测 `--version`）、用户自定义斜杠命令（文件系统：
//!    claude 的 `commands/`、codex 的 `prompts/`、gemini 的 `commands/*.toml`、opencode 的
//!    `command/`，含项目级目录）。本模块负责这层——装了新命令，下一次询问就反映。
//! 2. **版本相关但查询不到的**：内置命令表、`/model` 是否接受内联参数。五家 CLI 都没有
//!    「自述能力」的接口（没有任何 `list-commands`），这份知识只能由插件整理；但**选表**由
//!    安装实况驱动——变体粒度走 `Variant`，未来真出现版本分叉时 [`ChatUiContext::version`]
//!    已经在场，插件加分支即可、不必改接口。
//! 3. **本质是 meowo 的知识**（resume 参数、接线格式、安装地址）：CLI 永远不会提供，留在插件层。
//!
//! 与 transcript 同理：本模块只**读**文件系统，不写盘、不联网、不 spawn（探测版本的子进程
//! 由宿主负责，经 [`ChatUiContext`] 传入）。

use crate::registry::ModelPreset;
use serde::Serialize;
use std::path::Path;

/// 多家 CLI 共用的启动目录信任标题。即使某个 provider 当前版本尚无此功能，继承这些足够
/// 具体的句子也不会把普通欢迎页误判为交互提示，并能自然覆盖未来新增的同类安全检查。
pub const COMMON_STARTUP_ATTENTION_MARKERS: &[&str] = &[
    // Claude Code、Gemini CLI
    "do you trust the files in this folder",
    // Codex CLI
    "do you trust the contents of this directory",
];

/// 组装对话页能力时的输入，由宿主（meowo-app）构造。
#[derive(Debug, Default, Clone, Copy)]
pub struct ChatUiContext<'a> {
    /// 会话工作目录——发现**项目级**自定义命令用。None = 只发现用户级。
    pub cwd: Option<&'a Path>,
    /// 宿主探测到的已装 CLI 版本（`--version` 首行，探测失败为 None）。
    /// 当前没有按版本分叉的表；它在场是为了真出现版本差异时插件能分支，而不必改接口。
    pub version: Option<&'a str>,
    /// Agent 自己的会话 id（不是 Meowo DB id）。有它时插件可从该会话的 runtime 元数据
    /// 发现真实能力，例如 Claude transcript 的 `skill_listing`。
    pub session_id: Option<&'a str>,
}

/// 一条斜杠命令来自哪里。前端可据此区分展示（内置/用户级/项目级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub enum SlashSource {
    Builtin,
    User,
    Project,
}

/// 一条斜杠命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct SlashCommand {
    /// 含前导 `/`；子目录命令含命名空间（`/frontend:component`）。
    pub name: String,
    /// 自定义命令从文件头取（md 的 frontmatter `description:` / toml 的顶层 `description`）；
    /// 内置命令为 None——描述文案是翻译资产，前端 i18n 按名取。
    pub description: Option<String>,
    pub source: SlashSource,
}

impl SlashCommand {
    fn builtin(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            source: SlashSource::Builtin,
        }
    }

    /// Agent 在当前会话启动时自述的命令/skill。归为 builtin 是因为它不是用户在
    /// commands 目录创建的文件；description 直接采用 Agent 自己给出的说明。
    pub(crate) fn runtime(name: String, description: Option<String>) -> Self {
        Self {
            name,
            description,
            source: SlashSource::Builtin,
        }
    }
}

/// 一次模式切换需要写入 PTY 的输入。斜杠命令要提交回车，快捷键则只写原始字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct ModeInput {
    pub data: &'static str,
    pub submit: bool,
}

/// 可直接跳转到的模式值。一个选项允许由多次输入组成，例如 Kimi 回到 manual 需要同时
/// 关闭 `/yolo` 与 `/auto`，不能让前端理解 provider 私有语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct ModeOption {
    pub value: &'static str,
    pub inputs: &'static [ModeInput],
}

/// 锚点项的角色。此前是裸 `&str`（"input"/"chat" 靠注释约定），升级为枚举后
/// 生成的 TS 类型自然收窄为 `"input" | "chat"`，与前端 terminalAttention 的
/// SelectorAnchor.kind 对齐由类型系统而非注释保证。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub enum SelectorAnchorKind {
    /// 自由输入项(如 Type something)。
    Input,
    /// 转聊天项(如 Chat about this)。
    Chat,
}

/// 数字选择器的锚点项:该 provider 的选择器里**固有出现**的选项文案(小写子串匹配)。
/// 识别层据此把「纯编号的连续 run」认定为选择器本体——会话正文里的普通编号列表没有
/// 这些文案。声明前必须有该 provider 的真机取证;空 = 该 agent 的纯编号菜单不做卡片化
/// (发送侧的未识别交互软拦兜底),宁可不弹卡也不把正文列表变成会打方向键的按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct SelectorAnchor {
    pub marker: &'static str,
    pub kind: SelectorAnchorKind,
}

/// 命中提示后，**详情区**按哪种屏幕文法解析（工具名/命令段/问句怎么从文本里抠出来）。
/// 命名的是**屏幕格式**，不是 agent：新 agent 的审批框长得像哪种就声明哪种，
/// 都不像就 `None`——按钮照常可用，只是详情区不做结构化拆解。
/// 此前前端按 pattern id（`"kimi:command-approval"`）猜格式，是变相的身份分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub enum AttentionDetails {
    /// 不解析，整段命中文本原样展示。
    #[default]
    None,
    /// 「Do you want to proceed?」框式：工具头行 + 命令段 + 问句尾行（claude 真机取证）。
    /// 附带语义：「记住此决定」项是**持久**权限规则。
    ProceedBox,
    /// 「▶ 问题标题」面板式：▶ 标题行 + display 块（kimi 官方源码取证 approval-panel.ts）。
    /// 附带语义：「Approve for this session」只记本会话，不是持久规则。
    ArrowPanel,
}

/// 屏幕上**可操作**提示的识别规格：审批框、长会话确认这类需要用户当场决定的整句提示。
///
/// 与 [`crate::screen::ScreenRule`] 的分工：那套只判**状态**（working/idle/blocked，只读）；
/// 这套要在屏幕上**定位**出一张可点选、会往 PTY 写按键的卡片，故必须给出位置。
///
/// 此前这些整句规则硬编码在前端 `terminalAttention.ts` 里、按 `provider ===` 分支，
/// 循 `selector_anchors` 的先例下放为插件声明——加 agent 只动 `plugins/`。
///
/// **声明前必须有该 agent 的真机取证**：误报会凭空弹出一张卡并锁住输入框，
/// 比不弹更糟。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct AttentionPattern {
    /// 卡片标识（如 `"claude:command-approval"`）。前端按它决定卡片形态与按键语义，
    /// 取值必须与前端已实现的分支一致，不能随意造新的。
    pub id: &'static str,
    /// JS 正则源码（不含分隔符与标志），**任一命中**即算命中该卡片。
    /// 前端统一以 `im`（+ `last` 时加 `g`）标志编译，故 `^`/`$` 按行匹配。
    /// 字面量里的正则元字符要自行转义（如 `proceed\\?`）。
    pub patterns: &'static [&'static str],
    /// 取**最后一次**匹配而非第一次。审批框必须为 true：TUI 重绘不清屏，上一屏的
    /// 审批框会残留在缓冲上方，取第一次会把用户看到的下方活动面板与上方残影搞混
    /// ——真实事故是「用户看着 A 批准了 B」。仅出现一次的提示（长会话确认）用 false。
    pub last: bool,
    /// 详情区的屏幕文法风格（见 [`AttentionDetails`]）。None = 不做结构化拆解。
    pub details: AttentionDetails,
}

/// 屏幕回显标记：TUI 状态栏上代表某个模式值的稳定文案片段（如 claude 的 "plan mode on"）。
///
/// cycle 型控件是**盲切**：写入 cycle_input 后落到哪个值只有 CLI 自己知道，而 transcript
/// 只在活跃回合才记录模式——空闲时（恰是用户切权限模式的典型时机）切换在界面上毫无回显，
/// 用户只能认为按钮坏了。声明这张表后，宿主在写入后短暂轮询 PTY 屏幕，命中即更新显示。
/// 匹配为小写子串；文案取自 provider 文档承诺的指示文本，不是猜测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct ModeScreenMarker {
    pub marker: &'static str,
    pub value: &'static str,
}

/// ChatWindow 中一个独立的模式维度。`cycle_input` 与 `options` 可任选其一或同时存在；
/// 只读维度无需声明 control，仍可由 transcript 状态展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct ModeControl {
    pub dimension: &'static str,
    pub cycle_input: Option<&'static str>,
    pub options: &'static [ModeOption],
    /// 空表 = 该维度无屏幕回显能力，显示只随 transcript 状态走。
    pub screen_markers: &'static [ModeScreenMarker],
}

/// 对话页能力总装的结果，原样下发前端。
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct ChatUi {
    pub slash_commands: Vec<SlashCommand>,
    pub model_presets: Vec<ModelPreset>,
    /// 打开「选模型」菜单的命令。预设为空但有它时，前端发这条命令并把 CLI 弹出的
    /// 交互菜单渲染成按钮——模型清单由 CLI 现给，不必宿主维护一份会过时的。
    pub model_menu_command: Option<&'static str>,
    /// Provider 声明的模式交互能力。模式的真实当前值由 transcript 增量独立提供。
    pub mode_controls: Vec<ModeControl>,
    /// 裸发送会弹出交互界面的内置命令（含 `model_menu_command`，总装时并入）。
    /// 前端对它们走菜单识别通道，而不是裸发后让用户对着空白对话页干等。
    pub menu_slash_commands: Vec<&'static str>,
    /// 框架默认值与 provider 补充的、启动/恢复期间必须由用户在终端里处理的提示文本片段。
    /// 宿主据此不能把
    /// “首屏已有输出”误当成 composer 已就绪，更不能把待发送消息写进交互式选择器。
    pub startup_attention_markers: Vec<&'static str>,
    /// 数字选择器锚点(见 [`SelectorAnchor`]),识别层的选择器文法由插件声明。
    pub selector_anchors: Vec<SelectorAnchor>,
    /// 可操作提示的识别规格(见 [`AttentionPattern`])。整句识别规则由插件声明，
    /// 前端不再按 provider 分支。
    pub attention_patterns: Vec<AttentionPattern>,
    /// 中断当前回合的按键序列(如 Esc)。运行中「强制插话」= 先写它停掉当前回合,
    /// CLI 随即处理排队消息/接受新输入。None = 该 agent 的中断键未经取证,GUI 不提供入口。
    pub interrupt_input: Option<&'static str>,
    /// Ctrl/Shift+Enter 在 composer 插入换行的注入序列(如 claude 的 ESC+CR)。
    /// None = 未声明,前端保持终端原生语义(带修饰的 Enter 与裸 Enter 同码 `\r`)。
    pub newline_input: Option<&'static str>,
    /// 当前会话支持 runtime 命令自发现、但权威清单尚未写入 transcript。前端据此继续随
    /// transcript 增量重试；一旦为 false 就停止探测，避免稳态轮询反复扫描文件。
    pub runtime_commands_pending: bool,
    /// 附件可用该 CLI 的原生 `@路径` 提及注入（提交时由 CLI 原生附加文件内容,transcript
    /// 里也显示为原生提及）。false = 前端退回通用指令文本兜底。由插件按已探测版本声明；
    /// 探测不到版本一律 false——宁可多一段指令文本,不可发出对方不认的语法。
    pub attachment_mention: bool,
    /// 该 CLI 的 TUI 支持 Ctrl-V 原生粘贴剪贴板图片时,composer 里出现的占位符正则
    /// (claude `\[Image #\d`、kimi `\[image[:# ]`)。前端在「剪贴板里仍是刚粘贴的那张图」
    /// 时向 PTY 发 Ctrl-V 触发原生图片附加,并用它从屏幕上验证成功;None = 不支持,
    /// 图片一律走指令文本兜底。
    pub clipboard_image_paste: Option<&'static str>,
    /// 探测到的 CLI 版本，原样回传（展示/排障）。
    pub version: Option<String>,
}

/// 自定义斜杠命令的发现规格——**这才是「命令由 agent 提供」字面成立的那部分**：
/// 用户在 agent 自己的目录里放了什么，补全就出什么。
#[derive(Debug, Clone, Copy)]
pub struct CustomCommandSpec {
    /// 用户级命令目录，相对该 agent 的**数据目录**（多账号时即 profile 的数据目录，天然隔离）。
    pub user_dir: Option<&'static str>,
    /// 项目级命令目录，相对会话 cwd。None = 该 agent 无项目级命令。
    pub project_dir: Option<&'static str>,
    /// 命令文件扩展名（不含点）。`md` 的描述取 frontmatter，`toml` 取顶层 `description` 键。
    pub ext: &'static str,
    /// 子目录的命名空间连接符（claude/gemini 都是 `:`：`commands/git/commit.*` → `/git:commit`）。
    /// None = **只收顶层文件**——用于嵌套语义未经验证的 agent，宁可少收也不编造名字。
    pub namespace_sep: Option<&'static str>,
}

/// 单个目录树的扫描上限。命令目录本该只有几十个文件；真被指到一棵大树（比如 cwd 误配到 home）
/// 时截断保护，宁可补全不满也不把 UI 查询变成全盘遍历。
const MAX_FILES: usize = 200;
const MAX_DEPTH: usize = 3;

impl CustomCommandSpec {
    /// 扫出全部自定义命令：用户级（agent 数据目录下）+ 项目级（cwd 下）。
    /// 目录不存在/不可读一律静默跳过——「没配过自定义命令」是常态，不是错误。
    pub fn discover(&self, data_dir: &Path, cwd: Option<&Path>) -> Vec<SlashCommand> {
        let mut out = Vec::new();
        if let Some(rel) = self.user_dir {
            self.scan(&crate::join_rel(data_dir, rel), SlashSource::User, &mut out);
        }
        if let (Some(rel), Some(cwd)) = (self.project_dir, cwd) {
            self.scan(&crate::join_rel(cwd, rel), SlashSource::Project, &mut out);
        }
        out
    }

    fn scan(&self, root: &Path, source: SlashSource, out: &mut Vec<SlashCommand>) {
        let mut budget = MAX_FILES;
        self.scan_dir(root, source, &mut Vec::new(), &mut budget, out);
    }

    fn scan_dir(
        &self,
        dir: &Path,
        source: SlashSource,
        prefix: &mut Vec<String>,
        budget: &mut usize,
        out: &mut Vec<SlashCommand>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        // 排序保证同一目录内容产出稳定的命令顺序（read_dir 的顺序是平台细节）。
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if *budget == 0 {
                return;
            }
            let path = entry.path();
            if path.is_dir() {
                if self.namespace_sep.is_some() && prefix.len() + 1 < MAX_DEPTH {
                    prefix.push(entry.file_name().to_string_lossy().into_owned());
                    self.scan_dir(&path, source, prefix, budget, out);
                    prefix.pop();
                }
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some(self.ext) {
                continue;
            }
            *budget -= 1;
            let name = match (self.namespace_sep, prefix.is_empty()) {
                (Some(sep), false) => format!("/{}{sep}{stem}", prefix.join(sep)),
                _ => format!("/{stem}"),
            };
            out.push(SlashCommand {
                name,
                description: read_description(&path, self.ext),
                source,
            });
        }
    }
}

/// 从命令文件头里取描述，纯 best-effort：取不到就 None，前端只是少一行说明文字。
fn read_description(path: &Path, ext: &str) -> Option<String> {
    // 描述在文件头部；整读上限防住把大文件塞进命令目录的意外。
    let text = read_head(path, 64 * 1024)?;
    let desc = match ext {
        "toml" => text
            .parse::<toml_edit::DocumentMut>()
            .ok()?
            .get("description")?
            .as_str()?
            .to_string(),
        // md：YAML frontmatter（首行 `---` 到下一个 `---`）里的 `description:`。
        _ => {
            let mut lines = text.lines();
            if lines.next()?.trim() != "---" {
                return None;
            }
            lines
                .take_while(|l| l.trim() != "---")
                .find_map(|l| l.strip_prefix("description:"))?
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string()
        }
    };
    let desc = desc.trim();
    (!desc.is_empty()).then(|| desc.to_string())
}

fn read_head(path: &Path, max: u64) -> Option<String> {
    use std::io::Read;
    let mut buf = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(max)
        .read_to_string(&mut buf)
        .ok()?;
    Some(buf)
}

/// 内置表 ∪ 发现的自定义命令：同名时**自定义覆盖内置**（与 CLI 自身的遮蔽语义一致，
/// 且自定义带着从文件里读出的描述）；自定义内部同名时**项目级覆盖用户级**（claude 的
/// 遮蔽语义：离会话 cwd 越近优先级越高）。最后按名排序供补全稳定展示。
pub(crate) fn merge_commands(builtins: &[&str], custom: Vec<SlashCommand>) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = builtins
        .iter()
        .filter(|b| !custom.iter().any(|c| c.name == **b))
        .map(|b| SlashCommand::builtin(b))
        .collect();
    out.extend(custom);
    // discover 输出用户级在前；只按名字做稳定排序会把用户级留在前面，dedup 留首个便让
    // 用户级盖住项目级——与 claude 语义恰好相反。故同名时按遮蔽优先级再排一层。
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| shadow_rank(a.source).cmp(&shadow_rank(b.source)))
    });
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// 同名命令的遮蔽优先级：数值小者在排序中居前，从而在 `dedup` 中存活。
/// Builtin 实际到不了这一层（同名内置已在上方被过滤），列出只为覆盖全部变体。
fn shadow_rank(source: SlashSource) -> u8 {
    match source {
        SlashSource::Project => 0,
        SlashSource::User => 1,
        SlashSource::Builtin => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meowo-chatui-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const MD_SPEC: CustomCommandSpec = CustomCommandSpec {
        user_dir: Some("commands"),
        project_dir: Some(".claude/commands"),
        ext: "md",
        namespace_sep: Some(":"),
    };

    #[test]
    fn discovers_user_and_project_commands_with_namespaces_and_descriptions() {
        let root = temp_root("md");
        let user = root.join("data").join("commands");
        fs::create_dir_all(user.join("git")).unwrap();
        fs::write(
            user.join("deploy.md"),
            "---\ndescription: \"部署到测试环境\"\n---\n正文",
        )
        .unwrap();
        // 子目录 → 命名空间；无 frontmatter → 无描述。
        fs::write(user.join("git").join("commit.md"), "正文而已").unwrap();
        // 扩展名不符的不收（README、临时文件混进目录是常态）。
        fs::write(user.join("README.txt"), "x").unwrap();
        let proj = root.join("repo").join(".claude").join("commands");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("review.md"), "---\ndescription: 走查清单\n---").unwrap();

        let got = MD_SPEC.discover(&root.join("data"), Some(&root.join("repo")));
        assert_eq!(
            got,
            vec![
                SlashCommand {
                    name: "/deploy".into(),
                    description: Some("部署到测试环境".into()),
                    source: SlashSource::User,
                },
                SlashCommand {
                    name: "/git:commit".into(),
                    description: None,
                    source: SlashSource::User,
                },
                SlashCommand {
                    name: "/review".into(),
                    description: Some("走查清单".into()),
                    source: SlashSource::Project,
                },
            ]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn toml_description_and_flat_only_when_no_namespace_sep() {
        let root = temp_root("toml");
        let user = root.join("commands");
        fs::create_dir_all(user.join("nested")).unwrap();
        fs::write(
            user.join("commit.toml"),
            "description = \"生成提交信息\"\nprompt = \"...\"",
        )
        .unwrap();
        fs::write(user.join("nested").join("x.toml"), "prompt = \"...\"").unwrap();

        // namespace_sep = None：嵌套语义未验证的 agent 只收顶层，宁可少收也不编造名字。
        let flat = CustomCommandSpec {
            user_dir: Some("commands"),
            project_dir: None,
            ext: "toml",
            namespace_sep: None,
        };
        let got = flat.discover(&root, None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "/commit");
        assert_eq!(got[0].description.as_deref(), Some("生成提交信息"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_dirs_are_silently_empty() {
        let root = temp_root("none");
        assert!(MD_SPEC.discover(&root.join("nope"), None).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn merge_prefers_custom_over_builtin_and_sorts() {
        let custom = vec![SlashCommand {
            name: "/review".into(),
            description: Some("自定义走查".into()),
            source: SlashSource::Project,
        }];
        let merged = merge_commands(&["/review", "/clear"], custom);
        assert_eq!(
            merged.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["/clear", "/review"]
        );
        // 同名时自定义覆盖内置——与 CLI 自身的遮蔽语义一致。
        assert_eq!(merged[1].source, SlashSource::Project);
        assert_eq!(merged[1].description.as_deref(), Some("自定义走查"));
    }

    /// discover 输出用户级在前、项目级在后；合并时同名必须项目级胜出（claude 语义），
    /// 不能靠「稳定排序 + dedup 留首个」让用户级盖掉项目级（修复前正是如此）。
    #[test]
    fn project_command_shadows_same_named_user_command() {
        let custom = vec![
            SlashCommand {
                name: "/review".into(),
                description: Some("用户级走查".into()),
                source: SlashSource::User,
            },
            SlashCommand {
                name: "/review".into(),
                description: Some("项目级走查".into()),
                source: SlashSource::Project,
            },
            SlashCommand {
                name: "/deploy".into(),
                description: None,
                source: SlashSource::User,
            },
        ];
        let merged = merge_commands(&[], custom);
        assert_eq!(
            merged.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["/deploy", "/review"]
        );
        assert_eq!(merged[1].source, SlashSource::Project);
        assert_eq!(merged[1].description.as_deref(), Some("项目级走查"));
    }
}
