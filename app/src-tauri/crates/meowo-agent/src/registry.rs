//! 注册表：全项目唯一的 agent 分支中枢。加/改 agent 只动 `plugins/`，不动这里的调用方。

use crate::{
    caps::TelemetryCap,
    id::AgentId,
    variant::{Installation, Variant},
};
use std::path::Path;

/// 对话页快速切模型的一个预设项：前端点选后向 PTY 发送 `/model <id>`。
/// `label` 是展示名；描述文案是翻译资产，留在前端 i18n（按 `id` 取）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ModelPreset {
    pub id: &'static str,
    pub label: &'static str,
}

/// 一个 agent 插件。**必填的只有身份、变体表与进程名**；其余都是能力槽，不声明即由框架降级。
pub trait AgentPlugin: Sync {
    fn id(&self) -> AgentId;
    fn display_name(&self) -> &'static str;

    /// 变体表，**按优先级排列**（新版在前）。首个变体同时充当「全新安装该装到哪」的默认。
    fn variants(&self) -> &'static [Variant];

    /// 会话本体的进程名白名单（basename，小写）。owner_pid 上溯 + meowo-app 判活共用。
    fn process_names(&self) -> &'static [&'static str];

    /// 某进程名是否是**本 agent** 的会话本体。与全局的 [`is_agent_process`] 刻意不同：那个问的是
    /// 「这是不是某个 agent」，这个问的是「这是不是**我**」。
    ///
    /// 上溯抓 PID 必须用这一个。gemini 把 `node` 收进了自己的进程名（它没有别的可执行），于是全局
    /// 判定下 `is_agent_process("node.exe")` 恒为真——若 reporter 拿它去上溯，任何 agent 的 hook 都
    /// 可能在父链上撞见一个**无关的** node 祖先（VS Code 的集成终端就是一个），把它误认成自己的
    /// 会话本体。更糟的是两个不同 agent 的会话会因此认领到**同一个** pid，而 `set_session_pid` 的
    /// pid 独占语义会让后来者把前者直接收尾成 `ended`——两个活着的会话互相抹杀。
    ///
    /// 这不是假想：实测中 gemini 与 opencode 的会话正是这样抢到了同一个 node 祖先并互相顶掉的。
    fn owns_process(&self, name: &str) -> bool {
        let base = basename_lower(name);
        self.process_names().contains(&base.as_str())
    }

    // ═══ 声明式能力 ═══

    /// 追加在启动 argv 之后的 resume 子命令（claude `--resume`、kimi `-r`、codex `resume`），
    /// 其后再接 session_id。空 = 该 agent 不支持恢复会话。
    fn resume_args(&self) -> &'static [&'static str] {
        &[]
    }

    /// 官方安装引导脚本的地址（None = 无一键方案）。`windows` 决定取 `.ps1` 还是 `.sh`。
    ///
    /// 返回的是**地址**而非 `irm <url> | iex` 这类命令串：宿主取回内容、判定它确实是脚本
    /// （[`install::is_runnable_script`]）之后才落盘执行。`claude.ai` 与 `chatgpt.com` 都在
    /// Cloudflare 后面，其人机校验页以 **HTTP 200** 返回，裸管道会把那坨 HTML 喂给解释器。
    ///
    /// 声明了 [`direct_install`](Self::direct_install) 的 agent 会优先走直下，此项仅作回退。
    fn install_script(&self, _windows: bool) -> Option<crate::install::InstallScript> {
        None
    }

    /// 直下安装能力：绕开引导脚本（及其身后的人机校验），从发布物地址直取二进制并校验 SHA-256。
    /// None = 该 agent 只能走引导脚本。
    fn direct_install(&self) -> Option<&'static dyn crate::install::InstallCap> {
        None
    }

    /// 该 agent 是否把任务标题写进终端标签页标题。true → meowo-app 可按标题精确切到对应标签；
    /// false → 按任务标题找标签会错抓同名无关标签，应改走窗口级定位。
    fn sets_terminal_tab_title(&self) -> bool {
        false
    }

    /// meowo-reporter 是否应在 hook 时往本标签 ConPTY 写 session_id token，让 meowo-app 能按 token
    /// 精确切到该标签（解决同窗口同目录两会话标签同名分不清）。agent 后续可能覆盖 token，
    /// 因此声明此能力不排斥同时声明 `sets_terminal_tab_title`，app 会以 token 为最高优先级。
    fn writes_tab_token(&self) -> bool {
        false
    }

    /// 对话页输入框 `/` 前缀的补全候选。这些命令由该 agent 的 TUI 自己执行，meowo 只把文本
    /// 原样送进 PTY——所以**宁缺毋滥**：只列常用且跨版本稳定的。空 = 前端不提供补全。
    ///
    /// 此前这张表硬编码在前端（按 provider 的 `Record`，未知 agent 落一份通用 fallback）。
    /// fallback 恰恰是谎言：gemini 没有 `/status`（是 `/stats`）、opencode 没有 `/model`
    /// （是 `/models`），补出来的命令发进去只会报 unknown command。命令表是 agent 的事实，
    /// 归插件声明。
    fn slash_commands(&self) -> &'static [&'static str] {
        &[]
    }

    /// 对话页快速切模型的预设（前端发送 `/model <id>`）。只有 `/model` 接受**内联参数**的
    /// agent 才该声明——交互式菜单型的 `/model` 在对话页发出去用户什么也看不见。
    /// 空 = 不提供快捷切换（前端只展示当前模型，不给菜单）。
    fn model_presets(&self) -> &'static [ModelPreset] {
        &[]
    }

    /// 会话内可交互的模式维度。空 = 没有稳定且经过验证的切换方式。
    fn mode_controls(&self) -> &'static [crate::chat_ui::ModeControl] {
        &[]
    }

    /// 内置命令里**裸发送会弹出交互界面**（选择器/面板）的那部分。ChatWindow 把它们
    /// 路由到菜单识别通道（发出后开屏幕识别窗口，把 CLI 弹出的选择器转成 GUI 按钮），
    /// 而不是裸发——裸发的话用户在对话页什么都看不见，后续输入还会打进那个看不见的
    /// 菜单里。只声明经过验证的；`model_menu_command` 会在总装时自动并入，不必重复。
    fn menu_slash_commands(&self) -> &'static [&'static str] {
        &[]
    }

    /// 启动/恢复时可能抢占 composer 的交互式提示。只声明经过验证、必须由用户亲自处理的
    /// 文本标记；ChatWindow 命中后会展示终端并停止自动发送。
    fn startup_attention_markers(&self) -> &'static [&'static str] {
        crate::chat_ui::COMMON_STARTUP_ATTENTION_MARKERS
    }

    /// 数字选择器锚点(见 [`crate::chat_ui::SelectorAnchor`])。默认空:未经真机取证的
    /// provider 不做纯编号菜单卡片化,识别层宁可不弹卡(发送侧有软拦兜底)。
    fn selector_anchors(&self) -> &'static [crate::chat_ui::SelectorAnchor] {
        &[]
    }

    /// 屏幕上可操作提示的识别规格（见 [`crate::chat_ui::AttentionPattern`]）。默认空：
    /// 未取证的 provider 不做整句识别——误报会凭空弹卡并锁住输入框，比不弹更糟。
    fn attention_patterns(&self) -> &'static [crate::chat_ui::AttentionPattern] {
        &[]
    }

    /// 中断当前回合的按键序列。默认 None:未取证的 provider 不出中断按钮。现状是五家
    /// 都覆写成了 Esc,但证据分级不同——claude(官方文档)、codex(状态行自述 "esc to
    /// interrupt",真机 capture 在档,见 docs/research/tui-menu-captures-2026-07.md)、
    /// kimi(2026-07 真机人工确认)为确证;gemini/opencode 是约定推断,各自插件里写明了
    /// 存疑点。声明它的代价低(误发 Esc 至多是多停一个回合),但**发出去的消息撤不回**,
    /// 这个键是 GUI 上唯一能叫停当前回合的手段,故不取证也宁可给。
    fn interrupt_input(&self) -> Option<&'static str> {
        None
    }

    /// 自定义斜杠命令的发现规格（用户/项目目录里放了什么命令文件，补全就出什么）。
    /// None = 该 agent 无自定义命令机制，或其机制未经调研——**没验证过的不声明**。
    fn custom_commands(&self) -> Option<&'static crate::chat_ui::CustomCommandSpec> {
        None
    }

    /// 新建会话的启动选项（选择 → CLI flag 的映射表，见 [`crate::launch_options`]）。
    /// 空 = 面板不给该 agent 任何选项栏。宿主用 [`crate::resolve_launch_args`] 把前端的选择
    /// 翻译成 argv——用户输入永远不直接进命令行。
    fn launch_options(&self) -> &'static [crate::LaunchOption] {
        &[]
    }

    /// 对话页能力总装：内置表（插件按变体声明）∪ 从安装实况发现的自定义命令 + 模型预设。
    /// GUI 每次打开会话问一次——装了新命令/换了版本，下一次询问就反映，无需重启。
    fn chat_ui(&self, ctx: &crate::chat_ui::ChatUiContext) -> crate::chat_ui::ChatUi {
        let mut custom = match (self.custom_commands(), self.resolve()) {
            (Some(spec), Some(inst)) => spec.discover(&inst.data_dir, ctx.cwd),
            _ => Vec::new(),
        };
        let mut runtime_commands_pending = false;
        if let (Some(session_id), Some(transcript)) = (
            ctx.session_id,
            self.telemetry()
                .and_then(|telemetry| telemetry.transcript()),
        ) {
            if transcript.supports_runtime_slash_commands() {
                runtime_commands_pending = true;
                if let Some(path) = transcript.resolve_transcript_path(
                    None,
                    ctx.cwd.and_then(Path::to_str),
                    session_id,
                ) {
                    if let Some(commands) = transcript.runtime_slash_commands(&path) {
                        custom.extend(commands);
                        runtime_commands_pending = false;
                    }
                }
            }
        }
        // 模型菜单命令天然属于「会弹交互界面」的一类：用户在补全里点它/手敲它时，
        // 应得到与「切换模型」按钮一致的菜单识别待遇，插件不必在两处重复声明。
        let mut menu_slash_commands = self.menu_slash_commands().to_vec();
        if let Some(command) = self.model_menu_command() {
            if !menu_slash_commands.contains(&command) {
                menu_slash_commands.push(command);
            }
        }
        crate::chat_ui::ChatUi {
            slash_commands: crate::chat_ui::merge_commands(self.slash_commands(), custom),
            model_presets: self.model_presets().to_vec(),
            model_menu_command: self.model_menu_command(),
            mode_controls: self.mode_controls().to_vec(),
            menu_slash_commands,
            startup_attention_markers: self.startup_attention_markers().to_vec(),
            selector_anchors: self.selector_anchors().to_vec(),
            attention_patterns: self.attention_patterns().to_vec(),
            interrupt_input: self.interrupt_input(),
            runtime_commands_pending,
            attachment_mention: self.attachment_mention(ctx.version),
            clipboard_image_paste: self.clipboard_image_paste(ctx.version),
            version: ctx.version.map(str::to_string),
        }
    }

    /// TUI 支持 Ctrl-V 原生粘贴剪贴板图片时,返回 composer 占位符的验证正则;None = 不支持。
    ///
    /// 只有官方文档/实测确认「TUI 收到 Ctrl-V 会自己读系统剪贴板并把图片作为图像块附加」
    /// 的 agent 才该声明(claude 与 kimi 的文档都明确此行为)。版本探测不到时应返回 None
    /// 兜底——发一个对方不认的 Ctrl-V 只会往 composer 里塞脏字节。
    fn clipboard_image_paste(&self, _version: Option<&str>) -> Option<&'static str> {
        None
    }

    /// 附件是否可用该 CLI 的 `@路径` 提及注入。
    ///
    /// 只有**实测**过该家把 @提及当自己的惯例处理的 agent 才该返回 true:claude/gemini
    /// 是提交时原生附加(禁用读文件工具仍能答出内容);kimi 的 @ 是文本级一等语法
    /// (wire.jsonl 实测原样入 prompt、模型自行读取,与用户手敲等价);codex/opencode
    /// 未验证前不开。version 是宿主探测到的已装 CLI 版本——探测不到时应返回 false
    /// 兜底:宁可多一段指令文本,不可发出对方不认的语法。
    fn attachment_mention(&self, _version: Option<&str>) -> bool {
        false
    }

    /// 多账号（profile）的隔离规格。`None` = 该 agent **不支持**多账号。
    ///
    /// 目前只有 gemini 是 None：它的数据目录无法被环境变量覆盖（`GEMINI_DIR` 实测无效，设了它
    /// gemini 照样读 `~/.gemini`）。谎称支持的后果不是报错，而是**两个 profile 静默共用同一份凭据**
    /// ——切了账号却毫无效果，且没有任何迹象。宁可如实说不支持。
    fn profile(&self) -> Option<&'static crate::profile::ProfileSpec> {
        None
    }

    /// 某个 profile 在本机的安装实况：数据目录、hooks 规格、凭据位置全部落在 profile 根底下。
    /// 接线（给这个 profile 挂 hooks）与读它的登录态都走它。
    ///
    /// 与 [`resolve`](Self::resolve) 的区别：那个给的是**默认账号**（agent 自己的目录）的实况。
    fn installation_for_profile(&self, root: &std::path::Path) -> Option<Installation> {
        let spec = self.profile()?;
        let v = self.variants().first()?;
        let home = crate::home_dir();
        let mut inst = v.installation_at(self.id(), spec.data_dir(root), home.as_deref());
        inst.profile = Some((root.to_path_buf(), spec));
        Some(inst)
    }

    /// 该 agent 的 hook 事件名 → **meowo 规范事件名**（`SessionStart` / `UserPromptSubmit` /
    /// `PostToolUse` / `Stop` / `SessionEnd` / `PermissionRequest` / `PreToolUse`，即 dispatch 的消化面）。
    ///
    /// 默认原样透传——claude/codex/kimi/opencode 的事件名本就是规范名。**只有 gemini 需要它**：
    /// 它把「用户提交」「回合结束」叫成 `BeforeAgent` / `AfterAgent`，而配置里必须写它认识的名字。
    ///
    /// 翻译放在这里而不是 dispatch 里，是为了守住「加 agent 只动 `plugins/`」——否则 dispatch 的
    /// match 上迟早会长出一排 `if provider == "..."`。
    fn canonical_event<'a>(&self, raw: &'a str) -> &'a str {
        raw
    }

    // ═══ 能力槽 ═══

    /// 哪些工具的调用参数里带着**整份待办快照**（`tool_input.todos`）。
    ///
    /// 工具名是各家自己的事，且会随版本改：kimi 叫 `TodoList`，claude 早期叫 `TodoWrite`
    /// （现版本改成了增量的 `TaskCreate`/`TaskUpdate`，不是快照，故不在此列）。
    /// 此前 dispatch 里写死 `"TodoWrite"`，两家都对不上——待办表一直是空的。
    ///
    /// 只声明**快照式**的工具：整份列表覆盖写，语义与 `sync_todos` 一致。
    fn todo_snapshot_tools(&self) -> &'static [&'static str] {
        &[]
    }

    /// 打开「选模型」交互菜单的斜杠命令。
    ///
    /// 与 [`Self::model_presets`] 互补而非重复：声明了预设的 agent（只有 claude，它的
    /// `/model <id>` 接受内联参数）直接按预设发命令；其余四家的 `/model` 是交互式菜单，
    /// 内联参数无效——对它们只能「发出这条命令，再把 CLI 弹出的菜单渲染成 GUI 按钮」。
    /// 菜单内容由 CLI 自己给，宿主不必维护一份会过时的模型清单。
    fn model_menu_command(&self) -> Option<&'static str> {
        None
    }

    /// PermissionRequest hook 的决策输出会被该 agent **采纳**（阻塞式审批，如 claude/codex 的
    /// 310s hook）。GUI 审批桥只对声明它的 agent 生效：observation-only 的 PermissionRequest
    /// （kimi，5s 超时、忽略 hookSpecificOutput）若也弹 GUI 审批卡，卡片既控制不了真实审批，
    /// 用户点「允许」还会错误清掉 pending_review——真实提示仍在终端里等人。
    /// kimi 的 GUI 审批改走屏幕识别兜底（terminalAttention 的 `kimi:command-approval`，
    /// 形态与按键语义为官方源码取证，见 docs/research/tui-menu-captures-2026-07.md）。
    fn permission_hook_decides(&self) -> bool {
        false
    }

    /// 跨账号会话迁移规格（见 [`crate::profile::CrossAccountSession`]）。
    /// `None` = 不支持：恢复该 agent 的会话时沿用它原先所属的账号。
    fn cross_account_session(&self) -> Option<&'static crate::profile::CrossAccountSession> {
        None
    }

    /// 该 agent TUI 的屏幕状态规则（见 [`crate::screen`]）。空 = 不做屏幕检测，
    /// 宿主连终端仿真都不会为它建（reader 热路径零开销）。
    ///
    /// 规则是**数据**：加 agent 只在自己的插件里声明一张表，不必碰宿主的检测引擎——
    /// 与 `canonical_event` 同一条纪律（守住「加 agent 只动 `plugins/`」）。
    fn screen_rules(&self) -> &'static [crate::screen::ScreenRule] {
        &[]
    }

    /// 会话遥测（Stop 正文/模型、上下文占用、transcript、重命名回写）。None = 全部降级。
    fn telemetry(&self) -> Option<&'static dyn TelemetryCap> {
        None
    }

    /// 会话运行形态（哪些会话由 agent 自己的后台守护进程托管）。None = 该 agent 没有后台会话
    /// 这回事，宿主把所有会话一视同仁。
    fn runtime(&self) -> Option<&'static dyn crate::caps::RuntimeCap> {
        None
    }

    /// 账号与用量。None = 该 agent 无账号概念，卡片不显示登录态与用量。
    fn account(&self) -> Option<&'static dyn crate::account::AccountCap> {
        None
    }

    /// API Key 登录。None = 该 agent 不支持（或不需要——它的交互式登录足够）。
    /// 声明它的 agent，前端在未登录时额外给出「填 API Key」入口，登出时顺带清除 key。
    fn api_key_login(&self) -> Option<&'static dyn crate::account::ApiKeyLoginCap> {
        None
    }

    /// 接线副作用（写前改写 / 落盘后处理）。None = 纯 hooks 合并即可（kimi）。
    fn wiring(&self) -> Option<&'static dyn crate::wiring::WiringCap> {
        None
    }

    /// 该 agent 怎么才能被套上代理（支不支持 SOCKS、写哪些环境变量、能否写进自己的配置文件）。
    /// None = 无从配置代理。差异见 [`crate::proxy`]。
    fn proxy(&self) -> Option<&'static crate::proxy::ProxySpec> {
        None
    }

    /// API 中转。None = 不支持，宿主和前端都不得提供中转入口。
    fn relay(&self) -> Option<&'static dyn crate::relay::RelayCap> {
        None
    }

    /// meowo 能否显示该 agent 的**上下文占用**（贴纸上那个百分比液柱）。
    ///
    /// 默认 `true`：claude（statusLine）、codex/kimi（会话日志的 token_count）都拿得到。
    /// 返回 `false` 的 agent 前端会显式标注「上下文占用：不支持」，而不是留一片空白让用户以为是
    /// bug——是官方不给数据 / 我们没在插件里实现，两种都如实告知。gemini/opencode 见各自插件的说明。
    fn provides_context(&self) -> bool {
        true
    }

    /// 幂等接线：把 meowo-reporter 的 hooks 挂到该 agent 的配置里。全程 best-effort，绝不 panic。
    /// 返回 `None` = 成功/已是目标状态；`Some(reason)` = 无法接线（供「修复连接」回传前端）。
    ///
    /// 数据目录不存在＝该 agent 没装过：绝不凭空创建它的配置目录。
    fn wire(&self, ctx: &crate::wiring::WiringContext) -> Option<crate::config::RepairReason> {
        let id = self.id();
        let Some(inst) = self.resolve() else {
            eprintln!("Meowo repair[{id}]: 解析不到安装实况，跳过");
            return Some(crate::config::RepairReason::NotDetected);
        };
        if !inst.is_configured() {
            eprintln!(
                "Meowo repair[{id}]: {} 不存在（未安装），跳过",
                inst.data_dir.display()
            );
            return Some(crate::config::RepairReason::NotDetected);
        }
        crate::wiring::wire_hooks(&inst, id.as_str(), self.wiring(), ctx)
    }

    /// 该 agent 是否已在本机配置过（数据目录存在）——接线前的门槛。
    fn is_configured(&self) -> bool {
        self.resolve().is_some_and(|i| i.is_configured())
    }

    // ═══ 以下由变体表派生，通常不必覆写 ═══

    /// 本机实况：逐变体 probe，命中即返回；都不中 → None（＝未安装）。
    fn detect(&self) -> Option<Installation> {
        let home = crate::home_dir()?;
        self.variants()
            .iter()
            .find_map(|v| v.probe(self.id(), &home))
    }

    /// 未安装时的默认落点（首选变体的默认目录）。不保证目录存在。
    fn default_installation(&self) -> Option<Installation> {
        let home = crate::home_dir()?;
        let v = self.variants().first()?;
        let dir = v.data_dir.default_dir(&home)?;
        Some(v.installation_at(self.id(), dir, Some(&home)))
    }

    /// 探测到就用实况，否则退回默认落点。**路径解析的唯一入口**：读配置、找凭据、拼可执行都走它，
    /// 于是「kimi 的目录到底是哪个」只在此处回答一次。
    fn resolve(&self) -> Option<Installation> {
        self.detect().or_else(|| self.default_installation())
    }

    /// 裸启动一个全新会话的 argv。绝对路径优先——meowo-app 拉起的终端继承 app 启动那一刻的 PATH
    /// 快照，未必含刚装好的 agent（native installer 只改持久 PATH），裸名会让 wt/powershell 报
    /// 0x80070002。候选全不中时回退裸名交给 PATH 兜底。
    fn launch_argv(&self) -> Vec<String> {
        if let Some(i) = self.resolve() {
            return i.launch_argv();
        }
        // 连默认落点都推不出（home 缺失）：回退首选变体声明的裸名。
        let stem = self
            .variants()
            .first()
            .map_or(self.id().as_str(), |v| v.launch.stem);
        vec![stem.to_string()]
    }

    /// 恢复断开会话的完整 argv = 启动 argv + resume 子命令 + session_id。
    /// 该 agent 未声明 resume 子命令 → None。
    fn resume_argv(&self, session_id: &str) -> Option<Vec<String>> {
        let sub = self.resume_args();
        if sub.is_empty() {
            return None;
        }
        let mut argv = self.launch_argv();
        argv.extend(sub.iter().map(|s| s.to_string()));
        argv.push(session_id.to_string());
        Some(argv)
    }

    /// 该 agent 的可执行是否装在本机——**与 `launch_argv` 同源**，杜绝「检测说已安装、启动却找不到
    /// 文件」。
    ///
    /// 「在 PATH 上」也算已装：claude 的变体表把它写成了末位 `OnPath` 候选，kimi/codex 没有该候选
    /// （给它们加上会让「候选全不中即回退裸名」的单测随跑测机器的 PATH 漂移），故在此统一兜底一次。
    /// 两条路径都命中不了才算未装，此时 `launch_argv` 回退的裸名确实解析不出可执行。
    fn is_installed(&self) -> bool {
        self.resolve().is_some_and(|i| i.is_launchable())
            || self
                .variants()
                .first()
                .is_some_and(|v| crate::launch::exe_on_path(&v.launch.file_name()))
    }
}

static CLAUDE: crate::plugins::claude::Claude = crate::plugins::claude::Claude;
static KIMI: crate::plugins::kimi::Kimi = crate::plugins::kimi::Kimi;
static CODEX: crate::plugins::codex::Codex = crate::plugins::codex::Codex;
static GEMINI: crate::plugins::gemini::Gemini = crate::plugins::gemini::Gemini;
static OPENCODE: crate::plugins::opencode::Opencode = crate::plugins::opencode::Opencode;

/// 全部 agent。五家均在插件层——加 agent 只写 `plugins/<new>/` 再在此补一行。
static ALL: &[&dyn AgentPlugin] = &[&CLAUDE, &KIMI, &CODEX, &GEMINI, &OPENCODE];

pub fn all() -> &'static [&'static dyn AgentPlugin] {
    ALL
}

/// 历史默认 agent。DB 里 `sessions.provider` 为 NULL/空的老会话即它，与 `meowo_store::DEFAULT_PROVIDER`
/// 及建表 SQL 的 `DEFAULT 'claude'` 同值（配对断言见 `meowo_reporter::agent` 的测试——那里同时依赖两个 crate）。
pub const DEFAULT_ID: AgentId = crate::id::CLAUDE;

/// 按身份串取插件（`"claude"` / `"kimi"` / `"codex"`，与 DB / 前端 provider key 同值）。
pub fn by_id(id: &str) -> Option<&'static dyn AgentPlugin> {
    ALL.iter().copied().find(|p| p.id().as_str() == id)
}

/// DB 列 / 命令行 `--provider` 的字符串 → 已注册插件。**身份解析的唯一入口。**
///
/// - `None` / 空串 → 默认插件（老会话没写过 provider 列）。
/// - 已注册的 id → 该插件。
/// - **未知 id → `None`**，绝不降级成默认。
///
/// 最后一条是刻意的：旧的 `ProviderKey::from_str` 把未知串静默解析成 `Claude`，于是一个由更新版
/// meowo 写入、本版本尚不认识的 provider，其会话会被当成 claude 来 resume / 读 transcript / 查用量
/// ——全都指向错误的 CLI。宁可让调用方拿到 `None` 后降级为「不提供 agent 专属能力」，也不冒名顶替。
pub fn resolve(provider: Option<&str>) -> Option<&'static dyn AgentPlugin> {
    match provider.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => by_id(id),
        None => by_id(DEFAULT_ID.as_str()),
    }
}

/// 便捷：按身份取该 agent 在本机的实况（路径 / hooks 规格 / 凭据 / 启动 argv）。
pub fn installation(id: AgentId) -> Option<Installation> {
    by_id(id.as_str())?.resolve()
}

/// 进程名 → 小写 basename（可含路径）。精确比对的公共前置，杜绝子串误匹配。
fn basename_lower(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
}

/// 进程名（可含路径、大小写不敏感）是否属于**任一**已知 agent 本体——取 basename 精确比对。
///
/// 供 meowo-app 的判活/清理使用（它问的是「这个 pid 还是个 agent 吗」）。
///
/// **上溯抓 PID 不要用它**，用 [`AgentPlugin::owns_process`]：自 gemini 收了 `node` 起，这里对
/// `node.exe` 恒为真，拿它上溯会让任意 agent 撞上无关的 node 祖先。原委见 `owns_process` 的文档。
pub fn is_agent_process(name: &str) -> bool {
    let base = basename_lower(name);
    ALL.iter()
        .any(|p| p.process_names().contains(&base.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_id_matches_declared_id() {
        for id in ["claude", "kimi", "codex", "gemini", "opencode"] {
            assert_eq!(by_id(id).map(|p| p.id().as_str()), Some(id));
        }
        assert!(by_id("nope").is_none());
    }

    /// 注册表与前端/DB 的 provider key 集合必须逐一对应——漏注册会让该 agent 的所有链路静默退化。
    #[test]
    fn registry_covers_every_provider_key() {
        let mut ids: Vec<&str> = all().iter().map(|p| p.id().as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["claude", "codex", "gemini", "kimi", "opencode"]);
    }

    /// 未知 provider 串**绝不**降级成默认插件——旧 `ProviderKey::from_str` 正是这么把未知 agent
    /// 的会话冒名成 claude 的。None/空串则走默认（老会话没写过 provider 列）。
    #[test]
    fn resolve_maps_unknown_to_none_and_empty_to_default() {
        assert_eq!(resolve(Some("kimi")).map(|p| p.id().as_str()), Some("kimi"));
        assert_eq!(resolve(None).map(|p| p.id().as_str()), Some("claude"));
        assert_eq!(resolve(Some("")).map(|p| p.id().as_str()), Some("claude"));
        assert_eq!(resolve(Some("  ")).map(|p| p.id().as_str()), Some("claude"));
        // 曾以 "gemini" 作未注册反例——它现在是注册过的 agent 了。反例必须选一个**永远**不会被
        // 注册的串，否则这条断言会随着新 agent 的加入悄悄失去意义。
        assert_eq!(
            resolve(Some("gemini")).map(|p| p.id().as_str()),
            Some("gemini")
        );
        assert!(resolve(Some("nonsense")).is_none());
        assert!(resolve(Some("not-an-agent")).is_none());
    }

    #[test]
    fn default_id_is_registered() {
        assert!(by_id(DEFAULT_ID.as_str()).is_some());
    }

    #[test]
    fn relay_is_an_explicit_plugin_capability_and_kimi_legacy_is_rejected() {
        for id in ["claude", "codex", "kimi"] {
            assert!(
                by_id(id).and_then(|plugin| plugin.relay()).is_some(),
                "{id} 必须显式声明 relay"
            );
        }
        let kimi = by_id("kimi").unwrap().relay().unwrap();
        assert!(kimi.supports_variant("modern"));
        assert!(!kimi.supports_variant("legacy"));
    }

    #[test]
    fn every_plugin_declares_at_least_one_variant() {
        for p in all() {
            assert!(!p.variants().is_empty(), "{} 无变体", p.id());
        }
    }

    /// 进程名白名单是判活/上溯的依据，任一 agent 漏声明会让它的会话被当成死进程 reap。
    #[test]
    fn every_plugin_declares_process_names() {
        for p in all() {
            assert!(!p.process_names().is_empty(), "{} 无进程名", p.id());
            assert!(
                p.process_names()
                    .iter()
                    .all(|n| n == &n.to_ascii_lowercase()),
                "{} 的进程名须为小写（is_agent_process 按小写 basename 精确比对）",
                p.id()
            );
        }
    }

    #[test]
    fn is_agent_process_exact_basename_not_substring() {
        // 精确命中（含路径、大小写）。
        assert!(is_agent_process("claude.exe"));
        assert!(is_agent_process("kimi.exe"));
        assert!(is_agent_process("codex.exe"));
        assert!(is_agent_process("opencode.exe"));
        assert!(is_agent_process("C:/x/Kimi.EXE"));
        assert!(is_agent_process("/usr/bin/claude"));
        // 子串不应误匹配（这正是修复点）。
        assert!(!is_agent_process("kimi-desktop"));
        assert!(!is_agent_process("kimichat.exe"));
        assert!(!is_agent_process("claude-helper.exe"));
        assert!(!is_agent_process("opencode-helper.exe"));
        assert!(!is_agent_process(""));
    }

    /// `node` **算** agent 进程——这条曾经反着断言（`assert!(!is_agent_process("node"))`，当时
    /// codex 的注释还写着「不收 node.exe，过宽」），gemini 进来后不得不翻转：它没有自己的可执行，
    /// 会话本体就是一个跑 `bundle/gemini.js` 的 node 进程，不收它，owner_pid 上溯就抓不到宿主。
    ///
    /// 代价如实记在这里：判活对 node 从此变宽——某个 gemini 会话的 PID 被系统回收、又恰好落给
    /// 另一个 node 进程时，那个已死的会话会被误判为仍活着。接受它，是因为反面（不收 node）意味着
    /// 每个 gemini 会话从一开始就抓不到 PID：那是必然的坏，而这个是偶然的坏。详见 `plugins::gemini`。
    #[test]
    fn node_counts_as_agent_because_gemini_has_no_binary_of_its_own() {
        assert!(is_agent_process("node"));
        assert!(is_agent_process("node.exe"));
        assert!(is_agent_process("C:/Program Files/nodejs/node.exe"));
        // 变宽的只有「node 本身」：仍是精确 basename 比对，名字里含 node 的无关进程不会被误收。
        assert!(!is_agent_process("nodemon"));
        assert!(!is_agent_process("node-gyp.exe"));
    }

    /// 上溯抓 PID 只认**自己**的进程名——这是 `owns_process` 与全局 `is_agent_process` 的分野。
    ///
    /// 回归背景（实测踩到的，不是假想）：gemini 没有自己的可执行，会话本体就是个 node 进程，于是
    /// `node` 进了它的白名单。全局判定从此对 `node.exe` 恒为真；若 reporter 拿全局判定去上溯，
    /// **任何** agent 的 hook 都会在父链上撞见无关的 node 祖先并认领它的 pid。两个不同 agent 的
    /// 会话因此持有同一个 pid，而 `set_session_pid` 的 pid 独占语义会让后来者把前者收尾成
    /// `ended`——两个活着的会话互相抹杀。gemini 与 opencode 当场就这么互抹了。
    #[test]
    fn owns_process_is_per_agent_not_global() {
        let gemini = by_id("gemini").unwrap();
        let claude = by_id("claude").unwrap();
        let opencode = by_id("opencode").unwrap();

        // node 确实是 gemini 的会话本体。
        assert!(gemini.owns_process("node.exe"));
        assert!(gemini.owns_process("C:/Program Files/nodejs/node.exe"));

        // 全局判定对 node 为真——正因如此，它不能被用来上溯。
        assert!(is_agent_process("node.exe"));
        // 而别的 agent 绝不把 node 当成自己的会话本体。
        assert!(
            !claude.owns_process("node.exe"),
            "claude 的会话本体不是 node"
        );
        assert!(
            !opencode.owns_process("node.exe"),
            "opencode 是原生二进制，不是 node"
        );
        assert!(!by_id("codex").unwrap().owns_process("node.exe"));

        // 各自只认自己，互不越界。
        assert!(claude.owns_process("claude.exe"));
        assert!(opencode.owns_process("/usr/local/bin/opencode"));
        assert!(!claude.owns_process("opencode.exe"));
        assert!(!opencode.owns_process("claude.exe"));
        // 仍是精确 basename 比对，不做子串匹配。
        assert!(!gemini.owns_process("nodemon"));
        assert!(!gemini.owns_process("node-gyp.exe"));
    }

    /// resume argv = 启动 argv + 该 agent 声明的 resume 子命令 + session_id。
    /// 写错子命令会让「恢复会话」拉起一个报 unknown command 的终端。
    #[test]
    fn resume_argv_appends_declared_subcommand_and_session_id() {
        // launch_argv 读真实的 USERPROFILE；别的测试会临时改它。见 `crate::env_guard`。
        let _env = crate::env_guard();
        let cases = [
            (crate::id::CLAUDE, vec!["--resume"]),
            (crate::id::KIMI, vec!["-r"]),
            (crate::id::CODEX, vec!["resume"]),
            (crate::id::GEMINI, vec!["--resume"]),
            // `--continue` 只会续「最近一个」，恢复不了点开的那个——必须是接 id 的 `--session`。
            (crate::id::OPENCODE, vec!["--session"]),
        ];
        for (id, sub) in cases {
            let p = by_id(id.as_str()).unwrap();
            let argv = p.resume_argv("ID").expect("三家均声明了 resume 子命令");
            let n = argv.len();
            assert_eq!(argv[n - 1], "ID", "{id} 的末位应是 session_id");
            assert_eq!(
                &argv[n - 1 - sub.len()..n - 1],
                sub.as_slice(),
                "{id} 的 resume 子命令不符"
            );
            // 前缀即启动 argv：同源，杜绝「能启动却恢复不了」。
            assert_eq!(&argv[..n - 1 - sub.len()], p.launch_argv().as_slice());
        }
    }

    /// 启动 argv 非空，且首元素（绝对路径或回退裸名）指向该 agent 自己。
    #[test]
    fn launch_argv_is_nonempty_and_points_at_the_agent() {
        let _env = crate::env_guard();
        for p in all() {
            let argv = p.launch_argv();
            assert!(!argv.is_empty(), "{} 启动 argv 为空", p.id());
            // codex 的 npm 形态是 ["node", "<...>/codex.js"]，故查「某个元素含 id」而非首元素。
            assert!(
                argv.iter()
                    .any(|a| a.to_ascii_lowercase().contains(p.id().as_str())),
                "{} 的启动 argv 未指向自己：{argv:?}",
                p.id()
            );
        }
    }

    /// 已安装 ⇒ 启动 argv 的首元素真能启动。「能启动」不等于「是绝对路径」：`OnPath` 命中或
    /// PATH 兜底时 argv 是裸名（刻意不固化 shim 路径），那它就必须真的在 PATH 上。
    #[test]
    fn installed_implies_launch_argv_is_runnable() {
        let _env = crate::env_guard();
        for p in all() {
            if !p.is_installed() {
                continue; // 本机没装（CI 上常见）
            }
            let argv = p.launch_argv();
            let head = &argv[0];
            if head == p.id().as_str() || head == "node" {
                let name = crate::exe_file_name(head);
                assert!(
                    crate::launch::exe_on_path(&name),
                    "{} 回退裸名时应在 PATH 上",
                    p.id()
                );
            } else {
                assert!(
                    std::path::Path::new(head).is_file(),
                    "{} 启动 argv 指向的文件应存在：{head}",
                    p.id()
                );
            }
        }
    }

    /// 代理能力表钉死调研结论。这些值一旦写错，后果是**静默连不上**：给 claude 配了 socks，
    /// 它既不报错也不走代理，用户完全无从排查。故在此逐条固定。
    #[test]
    fn proxy_spec_pins_researched_capabilities() {
        let spec = |id: &str| {
            *by_id(id)
                .unwrap()
                .proxy()
                .unwrap_or_else(|| panic!("{id} 应声明代理能力"))
        };

        // 只有 claude 能写进自己的配置文件（settings.json 的 env 块）——这决定了「用户自己在终端
        // 敲命令也走代理」只对 claude 成立，是整个功能的覆盖面所在。
        assert!(
            spec("claude").config_env,
            "claude 的 settings.json env 块可写代理"
        );
        assert!(
            !spec("codex").config_env,
            "codex 无此配置键（issue #6060 未合）"
        );
        assert!(!spec("kimi").config_env, "kimi 的 config.toml 无 proxy 键");

        // SOCKS：claude 官方明确不支持；codex 未编译 reqwest 的 socks feature；kimi 支持。
        assert!(!spec("claude").socks);
        assert!(!spec("codex").socks);
        assert!(spec("kimi").socks);

        // 不支持 socks 的两家：给它们一个 socks 串必须当场拒绝，且一个键都不写。
        for id in ["claude", "codex"] {
            assert!(
                spec(id).accepts("socks5://127.0.0.1:1080").is_err(),
                "{id} 应拒绝 socks"
            );
            assert!(
                spec(id).env_for("socks5://127.0.0.1:1080").is_empty(),
                "{id} 不该写任何 socks 键"
            );
            assert!(spec(id).accepts("http://127.0.0.1:7890").is_ok());
        }
        // kimi 的 socks 走 ALL_PROXY（写进 HTTPS_PROXY 未必被识别）。
        assert_eq!(
            spec("kimi").env_for("socks5://127.0.0.1:1080"),
            vec![("ALL_PROXY", "socks5://127.0.0.1:1080".to_string())]
        );
    }

    /// 对话页能力表钉死调研结论。斜杠命令由各 CLI 的 TUI 自己执行，写错的后果是补全出一条
    /// 发进去报 unknown command 的命令；模型预设写错的后果更隐蔽——`/model <id>` 对交互式
    /// 菜单型的 CLI 发出去**什么也不发生**。故逐条固定。
    #[test]
    fn chat_ui_caps_pin_researched_capabilities() {
        // 五家都声明了命令表：/help 人人都有；每条都以 "/" 开头且不含空格（补全项是整命令，
        // 参数由用户自己接）。
        for p in all() {
            let cmds = p.slash_commands();
            assert!(!cmds.is_empty(), "{} 应声明斜杠命令表", p.id());
            assert!(cmds.contains(&"/help"), "{} 缺 /help", p.id());
            for c in cmds {
                assert!(
                    c.starts_with('/') && !c.contains(' '),
                    "{} 的命令 {c:?} 不合形",
                    p.id()
                );
            }
        }

        // 只有 claude 的 `/model` 接受内联参数——其余四家是交互式菜单，声明预设＝给出一个
        // 点了没反应的菜单。
        let presets = by_id("claude").unwrap().model_presets();
        assert!(!presets.is_empty());
        assert!(presets.iter().any(|m| m.id == "sonnet"));
        for id in ["codex", "kimi", "gemini", "opencode"] {
            assert!(
                by_id(id).unwrap().model_presets().is_empty(),
                "{id} 的 /model 是交互式菜单，不该声明预设"
            );
        }

        // @提及注入:只有实测过的 claude/gemini(提交时原生附加)与 kimi(@ 是它自己的
        // 文本级一等语法,模型自行读取)声明,且版本探测不到时必须兜底 false(宁可多一段
        // 指令文本,不可发出对方不认的语法)。
        for id in ["claude", "gemini", "kimi"] {
            assert!(by_id(id).unwrap().attachment_mention(Some("1.0.0")), "{id}");
            assert!(
                !by_id(id).unwrap().attachment_mention(None),
                "{id} 版本未知须兜底"
            );
        }
        for id in ["codex", "opencode"] {
            assert!(
                !by_id(id).unwrap().attachment_mention(Some("1.0.0")),
                "{id} 的 @提及未经验证,不该声明"
            );
        }
        // Ctrl-V 原生图片粘贴:只有文档/实测确认 TUI 会读系统剪贴板的 claude/kimi 声明,
        // 占位符正则非空且版本未知时兜底 None。
        for id in ["claude", "kimi"] {
            let marker = by_id(id).unwrap().clipboard_image_paste(Some("1.0.0"));
            assert!(marker.is_some_and(|m| !m.is_empty()), "{id}");
            assert!(
                by_id(id).unwrap().clipboard_image_paste(None).is_none(),
                "{id} 版本未知须兜底"
            );
        }
        for id in ["codex", "gemini", "opencode"] {
            assert!(
                by_id(id)
                    .unwrap()
                    .clipboard_image_paste(Some("1.0.0"))
                    .is_none(),
                "{id} 的剪贴板图片粘贴未经验证,不该声明"
            );
        }

        assert_eq!(
            by_id("claude").unwrap().mode_controls()[0].dimension,
            "permission"
        );
        assert_eq!(
            by_id("codex").unwrap().mode_controls()[0].dimension,
            "collaboration"
        );
        for id in ["gemini", "opencode"] {
            assert!(by_id(id).unwrap().mode_controls().is_empty());
        }

        // claude 的 cycle 是盲切且空闲期 transcript 不写模式记录，屏幕回显标记是唯一的
        // 即时反馈通道：六个文档承诺的模式都要在表里，value 与 transcript 的
        // permissionMode 取值一致，marker 全小写（前端按小写子串匹配屏幕）。
        let echo = by_id("claude").unwrap().mode_controls()[0].screen_markers;
        assert_eq!(
            echo.iter().map(|m| m.value).collect::<Vec<_>>(),
            vec![
                "bypassPermissions",
                "acceptEdits",
                "plan",
                "auto",
                "dontAsk",
                "default"
            ]
        );
        assert!(echo.iter().all(|m| m.marker == m.marker.to_lowercase()));

        // 交互式命令路由：声明的每一条必须真在内置表里（否则前端对着不存在的命令空转），
        // 交互式 `/model` 型 CLI 的 model_menu_command 要在总装时自动并入。
        let ctx = crate::ChatUiContext::default();
        let claude_ui = by_id("claude").unwrap().chat_ui(&ctx);
        assert_eq!(
            claude_ui.menu_slash_commands,
            vec!["/config", "/mcp", "/memory", "/model", "/resume"]
        );
        for command in &claude_ui.menu_slash_commands {
            assert!(
                claude_ui.slash_commands.iter().any(|c| c.name == *command),
                "{command} 不在 claude 的命令表里"
            );
        }
        assert!(by_id("kimi")
            .unwrap()
            .chat_ui(&ctx)
            .menu_slash_commands
            .contains(&"/model"));
        // opencode 的切模型命令是 /models(复数):此前声明成 /model,发出去是 unknown
        // command,模型按钮的 20 秒识别窗白等——菜单命令必须与命令表事实一致。
        let opencode_menu = by_id("opencode").unwrap().chat_ui(&ctx).menu_slash_commands;
        assert!(opencode_menu.contains(&"/models") && !opencode_menu.contains(&"/model"));

        // 选择器锚点是插件声明的识别文法:claude 有真机取证的两个锚点项;其余 provider
        // 未取证必须为空——空锚点 = 纯编号菜单不卡片化,宁可软拦也不误把正文变按钮。
        let claude_anchors = by_id("claude").unwrap().selector_anchors();
        assert_eq!(
            claude_anchors
                .iter()
                .map(|anchor| (anchor.marker, anchor.kind))
                .collect::<Vec<_>>(),
            vec![("type something", "input"), ("chat about this", "chat")]
        );
        for id in ["codex", "kimi", "gemini", "opencode"] {
            assert!(by_id(id).unwrap().selector_anchors().is_empty());
        }

        // 中断键:五家都用 Esc。确证——claude(官方文档)/codex(状态行实测)/
        // kimi(用户真机确认);约定推断——gemini/opencode(见各插件注释:探针受 launcher/
        // 会话生命周期所限抓不到运行中证据,但通行约定即 Esc,且「打断并发送」键错也低风险)。
        for id in ["claude", "codex", "kimi", "gemini", "opencode"] {
            assert_eq!(by_id(id).unwrap().interrupt_input(), Some("\x1b"));
        }

        // 目录信任不是 Claude 私有概念：Gemini 当前使用同一标题，Codex 使用 directory
        // 版本；Kimi/OpenCode 即便当前没有，也继承精确的通用标题以兼容后续版本。
        for id in ["claude", "codex", "kimi", "gemini", "opencode"] {
            let markers = by_id(id).unwrap().startup_attention_markers();
            assert!(markers.iter().any(|marker| marker.contains("folder")));
            assert!(markers.iter().any(|marker| marker.contains("directory")));
        }

        // 命令表如实反映各家差异（这正是硬编码 fallback 撒过谎的地方）。
        let has = |id: &str, c: &str| by_id(id).unwrap().slash_commands().contains(&c);
        assert!(has("gemini", "/stats") && !has("gemini", "/status"));
        assert!(has("opencode", "/models") && !has("opencode", "/model"));
        assert!(has("codex", "/model") && has("kimi", "/model"));
        assert!(
            !has("claude", "/code-review"),
            "Claude skills 必须来自会话的 skill_listing，不能再写死进基础表"
        );

        // 自定义命令的发现规格：调研过的四家各按自己的目录/格式声明；kimi 未验证有此机制，
        // 如实不声明——瞎猜一个目录的后果是永远扫不出东西还装作支持。
        let spec = |id: &str| by_id(id).unwrap().custom_commands();
        assert!(spec("kimi").is_none());
        for (id, ext, project) in [
            ("claude", "md", true),
            ("codex", "md", false),
            ("gemini", "toml", true),
            ("opencode", "md", true),
        ] {
            let s = spec(id).unwrap_or_else(|| panic!("{id} 应声明自定义命令规格"));
            assert_eq!(s.ext, ext, "{id} 的命令文件格式");
            assert_eq!(s.project_dir.is_some(), project, "{id} 的项目级目录声明");
            assert!(s.user_dir.is_some(), "{id} 应有用户级目录");
        }
    }

    /// 启动选项表钉死调研结论 + 结构不变量。写错 flag 的后果是新会话直接启动失败或
    /// **静默不生效**（CLI 把未知 flag 当参数吞掉），故逐条固定。
    #[test]
    fn launch_options_pin_researched_flags_and_invariants() {
        // 结构不变量：default 指向存在的 choice，且 default 项不传任何 flag——
        // 「默认」的诚实含义是「行为由 CLI 自己决定」。choice id 不重复。
        for p in all() {
            for opt in p.launch_options() {
                let def = opt
                    .choices
                    .iter()
                    .find(|c| c.id == opt.default)
                    .unwrap_or_else(|| panic!("{}::{} 的 default 不在 choices 里", p.id(), opt.id));
                assert!(
                    def.args.is_empty(),
                    "{}::{} 的默认项不该传 flag",
                    p.id(),
                    opt.id
                );
                let mut ids: Vec<_> = opt.choices.iter().map(|c| c.id).collect();
                ids.sort_unstable();
                ids.dedup();
                assert_eq!(
                    ids.len(),
                    opt.choices.len(),
                    "{}::{} choice id 重复",
                    p.id(),
                    opt.id
                );
            }
        }

        // 矩阵：claude 有模型 + 权限两栏；codex/gemini 各一栏审批；kimi 有权限 + 计划两栏
        // （`--auto` / `--yolo` / `--plan`，见 `kimi --help`），但**没有 model**——它的模型
        // 别名来自用户 config.toml，不是产品固定值。opencode 仍未调研到稳定 flag。
        let opts = |id: &str| by_id(id).unwrap().launch_options();
        assert_eq!(
            opts("claude").iter().map(|o| o.id).collect::<Vec<_>>(),
            vec!["model", "permission"]
        );
        assert_eq!(
            opts("codex").iter().map(|o| o.id).collect::<Vec<_>>(),
            vec!["approval"]
        );
        assert_eq!(
            opts("gemini").iter().map(|o| o.id).collect::<Vec<_>>(),
            vec!["approval"]
        );
        assert!(opts("opencode").is_empty());
        // kimi 的表随变体而定（旧 Python kimi-cli 的 flag 不同），本机未装 modern 时为空。
        let kimi = opts("kimi");
        if !kimi.is_empty() {
            assert_eq!(
                kimi.iter().map(|o| o.id).collect::<Vec<_>>(),
                vec!["permission", "work"]
            );
            assert!(kimi.iter().all(|o| o.choices.iter().all(|c| c
                .args
                .iter()
                .all(|arg| matches!(*arg, "--auto" | "--yolo" | "--plan")))));
        }

        // flag 字面量抽查（与 `--help` 实测对齐）。
        let arg_of = |id: &str, opt: &str, choice: &str| {
            opts(id)
                .iter()
                .find(|o| o.id == opt)
                .and_then(|o| o.choices.iter().find(|c| c.id == choice))
                .map(|c| c.args.to_vec())
                .unwrap()
        };
        assert_eq!(
            arg_of("claude", "model", "opusplan"),
            vec!["--model", "opusplan"]
        );
        assert_eq!(
            arg_of("claude", "permission", "plan"),
            vec!["--permission-mode", "plan"]
        );
        assert_eq!(
            arg_of("codex", "approval", "yolo"),
            vec!["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert_eq!(arg_of("gemini", "approval", "yolo"), vec!["--yolo"]);
    }

    /// 能力槽的降级语义：不声明 telemetry 的 agent，调用方拿到 None 而不是一个空实现。
    /// 三家目前都有 telemetry 与结构化 transcript；只有 claude 从 transcript 解析标题。
    #[test]
    fn telemetry_slot_reflects_declared_capabilities() {
        let claude = by_id("claude").unwrap().telemetry().expect("claude 有遥测");
        assert!(claude.transcript().is_some());
        assert!(claude.resolves_transcript_title());

        for id in ["kimi", "codex"] {
            let t = by_id(id)
                .unwrap()
                .telemetry()
                .unwrap_or_else(|| panic!("{id} 有遥测"));
            assert!(t.transcript().is_some(), "{id} 提供结构化对话 transcript");
            assert!(!t.resolves_transcript_title(), "{id} 的标题走首条 prompt");
        }

        // codex 不支持重命名回写（走 app-server JSON-RPC，成本高）→ 默认实现返回 false。
        assert!(!by_id("codex")
            .unwrap()
            .telemetry()
            .unwrap()
            .write_rename("s", None, "t"));
    }

    /// 只有 claude 有「agent 自己拉起后台会话」这回事（FleetView）。其余 agent 不声明
    /// runtime，宿主据此把它们的会话一视同仁，而不是去猜一个空索引的含义。
    #[test]
    fn runtime_slot_is_declared_only_where_background_sessions_exist() {
        assert!(by_id("claude").unwrap().runtime().is_some());
        for id in ["kimi", "codex", "gemini", "opencode"] {
            assert!(by_id(id).unwrap().runtime().is_none(), "{id} 没有后台会话");
        }
    }
}
