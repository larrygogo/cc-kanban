//! claude（Anthropic Claude Code）插件。数据目录只有一种（`~/.claude`），特点有三：
//!
//! 1. hooks 条目**带 `matcher`**——同一事件下按 matcher 与用户自有 hook 共存
//!    （如 `PreToolUse:Bash` 预检 vs 我方的 `PreToolUse:AskUserQuestion`）。
//! 2. hooks 与 `statusLine` 同住 `settings.json`，故 statusLine 的包装脚本走写前改写——见 `setup.rs`。
//! 3. 凭据在 macOS 走登录 Keychain，其它平台走 `~/.claude/.credentials.json`——见 `account.rs`。
//!    哪一条由注入的 `KeychainPort` 在运行时判定，本层没有平台 `cfg`。
//!
//! 可执行的落法（顺序即优先级）：
//!
//! | 优先级 | 安装方式 | 落点 |
//! |---|---|---|
//! | 1 | 官方 native installer（`claude.ai/install.ps1\|sh`） | `~/.local/bin/claude[.exe]` |
//! | 2 | npm 全局（Windows） | `%APPDATA%/npm/node_modules/@anthropic-ai/claude-code/bin/claude.exe` |
//! | 3 | PATH 兜底 | 裸名 |
//!
//! 前两条**必须**直查绝对路径：安装脚本只改持久 PATH，运行中的 meowo-app 进程 PATH 是启动时的
//! 旧快照、看不到新目录——装完却「打不开 / 提示找不到文件」正是这么来的（codex 的 standalone
//! 候选同理）。npm 那条只对 Windows 有意义：npm 在 unix 生成的 shim 是无扩展名的 `claude`，
//! `OnPath` 就能命中；Windows 上生成的是 `claude.cmd`，`exe_on_path("claude.exe")` 看不见它，
//! 故直查包内的 `bin/claude.exe`（该 npm 包分发的是原生二进制，不是 JS 入口）。

pub mod account;
mod screen;
pub mod fleet;
pub mod install;
pub mod setup;
pub mod telemetry;
pub mod transcript;

use crate::{
    auth::{AuthScheme, CredentialSource, OAuthRefresh},
    caps::TelemetryCap,
    config::{CommandSpec, ConfigFormat, HookEvent, HookSpec, MissingConfig},
    id::{self, AgentId},
    launch::{LaunchCandidate, LaunchSpec, Root},
    registry::AgentPlugin,
    variant::{DataDirSpec, Variant},
};

/// 接线事件集。`PreToolUse` 用 matcher 限定只在两种工具触发，与用户自有 `PreToolUse:Bash` 共存。
///
/// **此表须与 `scripts/install-hooks.mjs` 的 `SPECS` 保持一致**——由 meowo-app 的绊线测试守卫。
static EVENTS: [HookEvent; 8] = [
    HookEvent::matched("SessionStart", "*"),
    HookEvent::matched("UserPromptSubmit", "*"),
    HookEvent::matched("PostToolUse", "*"),
    HookEvent::matched("Stop", "*"),
    HookEvent::matched("SessionEnd", "*"),
    HookEvent::matched("PermissionRequest", "*").with_timeout(310),
    HookEvent::matched("PreToolUse", "AskUserQuestion"),
    HookEvent::matched("PreToolUse", "ExitPlanMode"),
];

/// `settings.json` 不存在时从空对象建：刚装 Claude Code、没改过设置的用户就没有这个文件。
/// 但**不凭空造 `~/.claude` 目录**——数据目录不存在＝没装，由 `is_configured()` 在上游拦下。
///
/// command 形态：`"<exe>"`（带引号、无参数）。claude 靠 settings 里的位置区分 provider，不带
/// `--provider`；认领规则据此要求余参为空。
static HOOKS: HookSpec = HookSpec {
    config_rel: "settings.json",
    format: ConfigFormat::ClaudeJson,
    missing: MissingConfig::CreateFrom("{}"),
    events: &EVENTS,
    command: CommandSpec {
        quote_exe: true,
        with_provider: false,
        ps_call_operator: false,
    },
};

/// Claude Code 公开 OAuth client id 与刷新端点。macOS 把凭据存进登录 Keychain 的通用密码
/// （service = `Claude Code-credentials`），其它平台落 `<data>/.credentials.json`。
/// `account` 是写回 Keychain 时的条目名兜底值——读得到实际 account 时以实际值为准。
///
/// 用量端点（`api.anthropic.com/api/oauth/usage`）不在此处：`AuthScheme` 只管「凭据在哪 +
/// 怎么刷新」，用量是 account 侧的事。
/// 多账号：`CLAUDE_CONFIG_DIR` 一个变量就把整个数据目录搬走（凭据、settings.json、历史全在里面）。
///
/// macOS 上 claude 的凭据默认在 Keychain 而非文件——但**设了 `CLAUDE_CONFIG_DIR` 之后不影响隔离**：
/// 各 profile 的 hooks / settings / 历史都各自独立，而 Keychain 那份凭据是全局的，等于所有 profile
/// 共享同一个登录身份。**这一条尚未解决**（见 `creds_rel` 指向的文件回退路径），macOS 上要真正切换
/// 账号还需要额外轮换 Keychain 条目。Windows / Linux 无此问题。
static PROFILE: crate::profile::ProfileSpec = crate::profile::ProfileSpec {
    envs: &[("CLAUDE_CONFIG_DIR", "")],
    data_rel: "",
    creds_rel: ".credentials.json",
};

/// Claude 的会话可跨账号继续：`claude --resume <id>` 只认 `CLAUDE_CONFIG_DIR` 下的
/// 会话数据，把这几处 session 级数据复制到目标账号目录后即可在新账号下接着跑。
///
/// 只列 session 级数据——credentials / settings / plugins 一概不搬（那是账号本身，
/// 搬过去等于把两个账号合并）。
static CROSS_ACCOUNT: crate::profile::CrossAccountSession = crate::profile::CrossAccountSession {
    transcript_dir: "projects",
    transcript_ext: ".jsonl",
    session_buckets: &["file-history", "session-env", "tasks"],
    subagents_beside_transcript: true,
};

static AUTH: AuthScheme = AuthScheme {
    credentials: CredentialSource::KeychainOrFile {
        service: "Claude Code-credentials",
        account: "root",
        file: ".credentials.json",
    },
    refresh: Some(OAuthRefresh {
        token_url: "https://platform.claude.com/v1/oauth/token",
        client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
    }),
    default_base_url: "",
    // 实测（claude --help / claude auth --help）：登录在 `auth` 子命令下，**没有** `claude login`。
    // 另有 `claude setup-token`（长期 token），不是交互式 OAuth 登录，不用它。
    login: Some(&["auth", "login"]),
    logout_args: &["auth", "logout"],
};

static LAUNCH: LaunchSpec = LaunchSpec {
    stem: "claude",
    candidates: &[
        LaunchCandidate::Exe {
            root: Root::Home,
            sub: ".local/bin",
        },
        // npm 全局前缀：Windows 上是 %APPDATA%\npm，某些环境 APPDATA 缺失则由 USERPROFILE 推。
        LaunchCandidate::Exe {
            root: Root::Env("APPDATA"),
            sub: "npm/node_modules/@anthropic-ai/claude-code/bin",
        },
        LaunchCandidate::Exe {
            root: Root::Env("USERPROFILE"),
            sub: "AppData/Roaming/npm/node_modules/@anthropic-ai/claude-code/bin",
        },
        LaunchCandidate::OnPath,
    ],
};

static VARIANTS: [Variant; 1] = [Variant {
    tag: "stable",
    data_dir: DataDirSpec {
        env: Some("CLAUDE_CONFIG_DIR"),
        candidates: &[".claude"],
    },
    hooks: &HOOKS,
    auth: Some(&AUTH),
    launch: &LAUNCH,
}];

pub struct Claude;

/// claude 是**唯一**能把代理写进自己配置文件的 agent：`settings.json` 的 `env` 块，官方定义为
/// 「作用于每个会话及其派生子进程」——于是用户自己在终端敲 `claude` 也会走代理。
///
/// SOCKS 明确不支持（官方原文：Claude Code does not support SOCKS proxies），故 `socks: false`，
/// `socks_keys` 留空——填了也没用，只会让用户以为配上了。
static PROXY: crate::proxy::ProxySpec = crate::proxy::ProxySpec {
    socks: false,
    config_env: true,
    http_keys: &["HTTPS_PROXY", "HTTP_PROXY"],
    socks_keys: &[],
};

struct ClaudeRelay;
static RELAY: ClaudeRelay = ClaudeRelay;
static RELAY_AUTH: [crate::RelayOption; 2] = [
    crate::RelayOption {
        value: "bearer",
        label: "Bearer Token",
    },
    crate::RelayOption {
        value: "api_key",
        label: "API Key (x-api-key)",
    },
];
/// 中转文档常附带的两个 Claude Code 环境变量，做成可勾选项随中转注入：
/// 关掉遥测/自动更新等外联（中转场景下它们只会增加噪音），以及去掉归因标识头。
static RELAY_ENV_OPTIONS: [crate::RelayEnvOption; 2] = [
    crate::RelayEnvOption {
        id: "disable_nonessential_traffic",
        label: "Disable non-essential traffic",
        env: ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    },
    crate::RelayEnvOption {
        id: "no_attribution_header",
        label: "Omit attribution header",
        env: ("CLAUDE_CODE_ATTRIBUTION_HEADER", "0"),
    },
];
static RELAY_SUGGESTIONS: [crate::RelaySuggestionGroup; 1] = [crate::RelaySuggestionGroup {
    protocol: "",
    models: &[
        "claude-fable-5",
        "claude-opus-4-8",
        "claude-sonnet-5",
        "claude-haiku-4-5-20251001",
    ],
}];

impl crate::RelayCap for ClaudeRelay {
    fn ui(&self) -> crate::RelayUi {
        crate::RelayUi {
            protocols: &[],
            auth_modes: &RELAY_AUTH,
            default_protocol: "",
            default_auth: "bearer",
            suggestions: &RELAY_SUGGESTIONS,
            env_options: &RELAY_ENV_OPTIONS,
        }
    }
    fn launch_env(&self, config: crate::RelayConfig<'_>, key: &str) -> Vec<(String, String)> {
        let mut env = vec![
            (
                "ANTHROPIC_BASE_URL".into(),
                config.base_url.trim().trim_end_matches('/').into(),
            ),
            (
                (if config.auth == "api_key" {
                    "ANTHROPIC_API_KEY"
                } else {
                    "ANTHROPIC_AUTH_TOKEN"
                })
                .into(),
                key.into(),
            ),
        ];
        // 用户勾选的附加环境变量（只取本插件声明过的 id，未声明的一律忽略）。
        for option in &RELAY_ENV_OPTIONS {
            if config.env_options.iter().any(|id| id == option.id) {
                env.push((option.env.0.into(), option.env.1.into()));
            }
        }
        env
    }
    fn augment_argv(
        &self,
        config: crate::RelayConfig<'_>,
        _has_secret: bool,
        mut argv: Vec<String>,
    ) -> Vec<String> {
        argv.extend(["--model".into(), config.model.trim().into()]);
        argv
    }
    fn model_request(&self, config: crate::RelayConfig<'_>) -> crate::RelayModelRequest {
        crate::RelayModelRequest {
            auth: if config.auth == "api_key" {
                crate::RelayModelAuth::ApiKey
            } else {
                crate::RelayModelAuth::Bearer
            },
            anthropic_version: true,
        }
    }
}

impl AgentPlugin for Claude {
    fn screen_rules(&self) -> &'static [crate::screen::ScreenRule] {
        screen::RULES
    }
    fn cross_account_session(&self) -> Option<&'static crate::profile::CrossAccountSession> {
        Some(&CROSS_ACCOUNT)
    }

    fn id(&self) -> AgentId {
        id::CLAUDE
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    /// PermissionRequest hook 声明了 310s 阻塞（EVENTS 里的 with_timeout），决策输出会被采纳。
    fn permission_hook_decides(&self) -> bool {
        true
    }
    /// 旧版 Claude Code 的 `TodoWrite` 带整份快照。**当前版本已换成增量的
    /// `TaskCreate`/`TaskUpdate`**（单条创建/改状态，不是快照），那条路要靠累积增量才能
    /// 还原列表，不属于这个槽——留着 `TodoWrite` 只为兼容仍在用旧版的用户。
    fn todo_snapshot_tools(&self) -> &'static [&'static str] {
        &["TodoWrite"]
    }
    fn variants(&self) -> &'static [Variant] {
        &VARIANTS
    }
    fn process_names(&self) -> &'static [&'static str] {
        &["claude", "claude.exe"]
    }
    fn proxy(&self) -> Option<&'static crate::proxy::ProxySpec> {
        Some(&PROXY)
    }
    fn relay(&self) -> Option<&'static dyn crate::RelayCap> {
        Some(&RELAY)
    }
    fn resume_args(&self) -> &'static [&'static str] {
        &["--resume"]
    }
    fn slash_commands(&self) -> &'static [&'static str] {
        &[
            "/clear", "/compact", "/config", "/cost", "/help", "/init", "/mcp", "/memory",
            "/model", "/resume", "/review", "/status",
        ]
    }
    /// 启动选项（实测 `claude --help`）：`--model <alias>` 与 `--permission-mode
    /// <default|plan|acceptEdits|bypassPermissions>`。default 项一律不传 flag——CLI 的默认
    /// 行为由 CLI 自己决定，不替它猜。
    fn launch_options(&self) -> &'static [crate::LaunchOption] {
        use crate::{LaunchChoice, LaunchOption};
        static OPTIONS: [LaunchOption; 2] = [
            LaunchOption {
                id: "model",
                default: "default",
                choices: &[
                    LaunchChoice {
                        id: "default",
                        label: "Default",
                        args: &[],
                    },
                    LaunchChoice {
                        id: "fable",
                        label: "Fable",
                        args: &["--model", "fable"],
                    },
                    LaunchChoice {
                        id: "opus",
                        label: "Opus",
                        args: &["--model", "opus"],
                    },
                    LaunchChoice {
                        id: "sonnet",
                        label: "Sonnet",
                        args: &["--model", "sonnet"],
                    },
                    LaunchChoice {
                        id: "haiku",
                        label: "Haiku",
                        args: &["--model", "haiku"],
                    },
                    LaunchChoice {
                        id: "opusplan",
                        label: "Opus Plan",
                        args: &["--model", "opusplan"],
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
                    },
                    LaunchChoice {
                        id: "plan",
                        label: "Plan",
                        args: &["--permission-mode", "plan"],
                    },
                    LaunchChoice {
                        id: "acceptEdits",
                        label: "Accept Edits",
                        args: &["--permission-mode", "acceptEdits"],
                    },
                    LaunchChoice {
                        id: "bypassPermissions",
                        label: "Bypass Permissions",
                        args: &["--permission-mode", "bypassPermissions"],
                    },
                ],
            },
        ];
        &OPTIONS
    }
    /// 用户级 `<数据目录>/commands/*.md`（多账号时随 profile 走）+ 项目级 `.claude/commands/`；
    /// 子目录按 `:` 命名空间（`commands/git/commit.md` → `/git:commit`）。
    fn custom_commands(&self) -> Option<&'static crate::CustomCommandSpec> {
        static SPEC: crate::CustomCommandSpec = crate::CustomCommandSpec {
            user_dir: Some("commands"),
            project_dir: Some(".claude/commands"),
            ext: "md",
            namespace_sep: Some(":"),
        };
        Some(&SPEC)
    }
    /// 内置命令中裸发送会弹出交互界面的那些（官方 CLI 行为）：`/config` 设置面板、
    /// `/mcp` 服务器面板、`/memory` 记忆文件选择器、`/model` 无参时的模型选择器、
    /// `/resume` 会话选择器。只声明内置表里确有、且形态经确认的——`/status`/`/cost`
    /// 之类只读面板不在此列（没有可选项，识别通道无用武之地）。
    fn menu_slash_commands(&self) -> &'static [&'static str] {
        &["/config", "/mcp", "/memory", "/model", "/resume"]
    }
    /// 提交文本里的 `@绝对路径` 会被原生附加(2.1.217 headless 实测:禁用全部读文件工具
    /// 仍能答出文件内容)。图片例外——@提及不产生图像块,前端已按此退回指令文本。
    /// 版本探测不到时兜底 false。
    fn attachment_mention(&self, version: Option<&str>) -> bool {
        version.is_some()
    }
    /// claude TUI 的 Ctrl-V 读系统剪贴板并把图片原生附加,composer 显示 `[Image #N]`。
    /// (@提及对图片无效——不产生图像块,故图片的原生化只有这条剪贴板通道。)
    fn clipboard_image_paste(&self, version: Option<&str>) -> Option<&'static str> {
        version.map(|_| r"\[Image #\d")
    }
    /// claude 的 `/model` 接受内联参数（`/model sonnet`），可以在对话页静默切换；
    /// 别名与官方 CLI 一致（`opusplan`：规划用 Opus、执行用 Sonnet）。
    fn model_presets(&self) -> &'static [crate::ModelPreset] {
        &[
            crate::ModelPreset {
                id: "fable",
                label: "Fable",
            },
            crate::ModelPreset {
                id: "opus",
                label: "Opus",
            },
            crate::ModelPreset {
                id: "sonnet",
                label: "Sonnet",
            },
            crate::ModelPreset {
                id: "haiku",
                label: "Haiku",
            },
            crate::ModelPreset {
                id: "opusplan",
                label: "Opus Plan",
            },
        ]
    }
    /// Claude Code 官方的 `chat:cycleMode` 键位（Shift+Tab）。模式集合会随账号与启动参数
    /// 变化，因此这里只声明“循环下一项”，不向 GUI 虚构一张固定、可直接跳转的列表。
    fn mode_controls(&self) -> &'static [crate::ModeControl] {
        // 屏幕回显标记 = 官方文档承诺的状态栏指示文本（permission-modes 文档）。cycle 是盲切，
        // 且 claude 只在活跃回合往 transcript 写模式记录——空闲切换若无屏幕回显，GUI 标签
        // 纹丝不动，用户只能认为按钮坏了。value 与 transcript 的 `permissionMode` 值一致。
        static MARKERS: [crate::ModeScreenMarker; 6] = [
            crate::ModeScreenMarker {
                marker: "bypass permissions on",
                value: "bypassPermissions",
            },
            crate::ModeScreenMarker {
                marker: "accept edits on",
                value: "acceptEdits",
            },
            crate::ModeScreenMarker {
                marker: "plan mode on",
                value: "plan",
            },
            crate::ModeScreenMarker {
                marker: "auto mode on",
                value: "auto",
            },
            crate::ModeScreenMarker {
                marker: "don't ask on",
                value: "dontAsk",
            },
            crate::ModeScreenMarker {
                marker: "manual mode on",
                value: "default",
            },
        ];
        static MODES: [crate::ModeControl; 1] = [crate::ModeControl {
            dimension: "permission",
            cycle_input: Some("\x1b[Z"),
            options: &[],
            screen_markers: &MARKERS,
        }];
        &MODES
    }
    /// Claude 在首次进入某个 cwd 时会先显示 workspace trust 选择器。它不是可接收聊天文本的
    /// composer；这些片段覆盖现有版本的标题、错误兜底与非交互提示措辞。
    fn startup_attention_markers(&self) -> &'static [&'static str] {
        &[
            "do you trust the files in this folder",
            "do you trust the contents of this directory",
            "trust this folder",
            "workspace not trusted",
            "workspace trust dialog",
        ]
    }
    /// Esc 中断当前回合(官方文档行为):中断后已完成的工作保留,排队消息随即被处理。
    fn interrupt_input(&self) -> Option<&'static str> {
        Some("\x1b")
    }
    /// AskUserQuestion 选择器的固有锚点项(真机截屏取证):单选/多问题标签页形态的
    /// 纯编号菜单靠它们与正文编号列表区分。识别层的选择器文法由此声明,不再硬编码。
    fn selector_anchors(&self) -> &'static [crate::chat_ui::SelectorAnchor] {
        &[
            crate::chat_ui::SelectorAnchor {
                marker: "type something",
                kind: "input",
            },
            crate::chat_ui::SelectorAnchor {
                marker: "chat about this",
                kind: "chat",
            },
        ]
    }
    /// 直下：从 `downloads.claude.ai`（GCS，**无 Cloudflare**）取二进制并校验 SHA-256。
    /// 见 `install.rs`——引导脚本做的正是这三步。
    fn direct_install(&self) -> Option<&'static dyn crate::install::InstallCap> {
        Some(&install::DIRECT_INSTALL)
    }

    /// 回退路径。`claude.ai` 在 Cloudflare 后面，会间歇触发人机校验（其页面以 HTTP 200 返回），
    /// 故优先走 `direct_install`；只有它失败（如发布物 schema 变了）才落到这里。
    fn install_script(&self, windows: bool) -> Option<crate::install::InstallScript> {
        Some(crate::install::InstallScript::Fetch {
            url: if windows {
                "https://claude.ai/install.ps1"
            } else {
                "https://claude.ai/install.sh"
            },
            unix_shell: "bash", // 脚本用了 `[[ ]]`，dash 跑不了
        })
    }
    /// claude 会把**自动生成的会话标题**写进标签页——但只有自然语言首条消息才触发生成；
    /// 首条消息是斜杠命令（如 `/code-review`）的会话标签永远停留在默认 "Claude Code"，
    /// 按任务标题匹配必然落空。故仍保留标题匹配，同时补 reporter token 兜底（见下）。
    fn sets_terminal_tab_title(&self) -> bool {
        true
    }
    /// 标签停在默认标题（斜杠命令会话）或标题漂移时，token 是唯一能精确命中的线索。
    /// 与 kimi 相同的已知折扣：claude 运行中 spinner 持续刷新标题会覆盖 token；Stop 时
    /// 写入的 token 在空闲/等输入期间存活——「点卡片定位过去」主要发生在这个阶段。
    fn writes_tab_token(&self) -> bool {
        true
    }
    fn telemetry(&self) -> Option<&'static dyn TelemetryCap> {
        Some(&telemetry::TELEMETRY)
    }
    /// FleetView 的后台会话（`sessions/<pid>.json` 的 `kind: "bg"`）。
    fn runtime(&self) -> Option<&'static dyn crate::caps::RuntimeCap> {
        Some(&fleet::CLAUDE_RUNTIME)
    }
    fn account(&self) -> Option<&'static dyn crate::account::AccountCap> {
        Some(&account::ACCOUNT)
    }
    fn wiring(&self) -> Option<&'static dyn crate::wiring::WiringCap> {
        Some(&setup::WIRING)
    }
    fn profile(&self) -> Option<&'static crate::profile::ProfileSpec> {
        Some(&PROFILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 斜杠命令会话的标签停在默认 "Claude Code"，标题匹配落空 → 必须同时声明 token 兜底。
    #[test]
    fn terminal_title_capabilities_cover_slash_command_sessions() {
        assert!(Claude.sets_terminal_tab_title());
        assert!(Claude.writes_tab_token());
    }

    fn probe_in(home: &std::path::Path) -> Option<crate::Installation> {
        VARIANTS[0].probe(id::CLAUDE, home)
    }

    /// 每个测试一个独立 home，避免并发串扰。
    fn temp_home(tag: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("meowo-claude-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        home
    }

    fn touch_exe(dir: &std::path::Path, stem: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(crate::exe_file_name(stem));
        std::fs::write(&p, b"").unwrap();
        p
    }

    #[test]
    fn config_and_credentials_sit_under_data_dir() {
        let home = temp_home("layout");
        let inst = probe_in(&home).expect("~/.claude 存在应命中");
        assert_eq!(inst.variant_tag, "stable");
        assert_eq!(
            inst.config_path(),
            home.join(".claude").join("settings.json")
        );
        // Keychain 变体在非 macOS 回退到文件路径；macOS 上调用方改读 Keychain，此路径仅作回退。
        assert_eq!(
            inst.credentials_path(),
            Some(home.join(".claude").join(".credentials.json"))
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn native_installer_local_bin_is_found() {
        // 官方 installer 的落点。PATH 里没有它也必须能启动——这正是「装完打不开」的修复点。
        // 逐段 join：`join(".local/bin")` 在 Windows 上会原样留下 `/`，与 `join_rel` 拼出的
        // `\` 不是同一个字符串（虽指向同一文件），断言会假失败。
        let home = temp_home("localbin");
        let exe = touch_exe(&home.join(".local").join("bin"), "claude");
        let inst = probe_in(&home).unwrap();
        assert!(inst.is_launchable());
        assert_eq!(inst.launch_argv(), vec![exe.to_string_lossy().into_owned()]);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 候选顺序是声明表的一部分，改动会改变「装了多份 claude 时启动哪个」，故绊线守住。
    /// 不实测 npm/OnPath 两条：前者的根是真实环境变量（`APPDATA`/`USERPROFILE`），后者依赖
    /// 进程 PATH——在跑测试的机器上都不可控，实测只会得到一个随环境漂移的假测试。
    #[test]
    fn candidate_order_is_native_then_npm_then_path() {
        let names: Vec<&str> = LAUNCH
            .candidates
            .iter()
            .map(|c| match c {
                LaunchCandidate::Exe {
                    root: Root::Home,
                    sub,
                } => *sub,
                LaunchCandidate::Exe {
                    root: Root::Env(v), ..
                } => *v,
                LaunchCandidate::Exe {
                    root: Root::DataDir,
                    ..
                } => "data-dir",
                LaunchCandidate::NodeScript { .. } => "node-script",
                LaunchCandidate::OnPath => "on-path",
            })
            .collect();
        assert_eq!(
            names,
            vec![".local/bin", "APPDATA", "USERPROFILE", "on-path"]
        );
    }

    /// 登录 argv 接在启动 argv 之后。claude 是 `auth login` 两段——实测没有 `claude login`，
    /// 写错会让「登录」按钮拉起一个报 unknown command 的终端。
    #[test]
    fn login_argv_is_auth_login_appended_to_launch() {
        let home = temp_home("login");
        let exe = touch_exe(&home.join(".local").join("bin"), "claude");
        let inst = probe_in(&home).unwrap();
        let argv = inst.login_argv().expect("claude 应声明登录入口");
        assert_eq!(
            argv,
            vec![
                exe.to_string_lossy().into_owned(),
                "auth".into(),
                "login".into()
            ]
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn events_cover_the_eight_specs_with_matchers() {
        assert_eq!(EVENTS.len(), 8);
        // PreToolUse 恰两条，matcher 分别是两种工具；其余六条 matcher 均为 "*"。
        let pre: Vec<_> = EVENTS
            .iter()
            .filter(|e| e.name == "PreToolUse")
            .map(|e| e.matcher.unwrap())
            .collect();
        assert_eq!(pre, vec!["AskUserQuestion", "ExitPlanMode"]);
        assert!(EVENTS
            .iter()
            .filter(|e| e.name != "PreToolUse")
            .all(|e| e.matcher == Some("*")));
        assert_eq!(
            EVENTS
                .iter()
                .find(|e| e.name == "PermissionRequest")
                .unwrap()
                .timeout,
            310
        );
        assert!(EVENTS
            .iter()
            .filter(|e| e.name != "PermissionRequest")
            .all(|e| e.timeout == 5));
    }
}
