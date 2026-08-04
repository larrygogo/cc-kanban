//! gemini（Google Gemini CLI）插件。数据目录与 hooks 文件都平平无奇——`~/.gemini/settings.json`
//! 的 hooks 块与 claude 的 `settings.json` **结构完全同构**，故直接复用 [`ConfigFormat::ClaudeJson`]，
//! 一行合并逻辑都不必新写。
//!
//! 真正的两处不同：
//!
//! **一、事件名自成一派。** Gemini 没有 `UserPromptSubmit` 与 `Stop`，对应的叫 `BeforeAgent` /
//! `AfterAgent`，`PostToolUse` 叫 `AfterTool`。写进配置的必须是 Gemini 认识的名字，而 reporter 的
//! dispatch 只认规范名，故本插件覆写 [`canonical_event`](AgentPlugin::canonical_event) 把它们译回去。
//! 这也是「加 agent 只动 `plugins/`」的一次兑现：dispatch 里没有一个 `if provider == "gemini"`。
//!
//! **二、它是纯 JS，没有自己的可执行。** npm 全局装出来的 `gemini.cmd` 只是个 shim，真正跑起来的
//! 进程是 `node`（实测 0.50.0：`bin` 指向 `bundle/gemini.js`）。这逼得
//! [`process_names`](crate::registry::AgentPlugin::process_names) 必须收
//! `node`——代价见那里的注释。
//!
//! 事件表（配置里写左边，dispatch 看到的是右边）：
//!
//! | Gemini | 规范名 | 用途 |
//! |---|---|---|
//! | `SessionStart` | `SessionStart` | 建会话 |
//! | `BeforeAgent` | `UserPromptSubmit` | 用户提交（负载带 `prompt`） |
//! | `AfterTool` | `PostToolUse` | 工具跑完 |
//! | `AfterAgent` | `Stop` | 回合结束（负载带 `prompt_response`＝AI 正文） |
//! | `SessionEnd` | `SessionEnd` | 会话收尾 |

pub mod account;
mod screen;
pub mod telemetry;

use crate::{
    auth::{AuthScheme, CredentialSource},
    caps::TelemetryCap,
    config::{CommandSpec, ConfigFormat, HookEvent, HookSpec, MissingConfig},
    id::{self, AgentId},
    launch::{LaunchCandidate, LaunchSpec, Root},
    registry::AgentPlugin,
    variant::{DataDirSpec, Variant},
};

/// Gemini 的 OAuth 凭据。文件名取自 0.50 bundle 里的常量，与账号信息同处 `~/.gemini`（＝ data_dir）。
///
/// **登录入口是 `Some(&[])`——裸启动**。gemini 没有登录子命令（实测 `gemini --help`：只有
/// mcp / extensions / skills / hooks / gemma），跑 `gemini` 本身首次就会引导你选认证方式
/// （Google 账号 / API key）。这与「没有登录入口」（`login: None`）是两回事。
///
/// 不声明 OAuth 刷新：token 由 gemini 自己维护，Meowo 只读不刷。
static AUTH: AuthScheme = AuthScheme {
    credentials: CredentialSource::File("oauth_creds.json"),
    refresh: None,
    default_base_url: "",
    login: Some(&[]),
    // gemini 没有登出子命令（`gemini --help` 里只有 mcp / extensions / skills / hooks / gemma）
    // → 宿主直接删凭据文件。
    logout_args: &[],
};

/// 接线事件集，写的是 **Gemini 的**事件名（配置里必须如此），译回规范名见 [`Gemini::canonical_event`]。
///
/// 不带 matcher（`plain`）是有意的：Gemini 的工具事件按正则匹配工具名，但生命周期事件的 matcher 是
/// **精确字符串**比对——claude 那套 `matcher: "*"` 搬过来会一条都匹配不上，而缺省即「不限制」。
///
/// timeout 单位是**毫秒**，与 claude 的秒语义相反（实测 0.51 hookRunner：
/// `setTimeout(..., hookConfig.timeout ?? 60000)`）。沿用 `plain` 的默认 5 就是 5ms——reporter
/// 刚 spawn 即被 `taskkill /f /t` 整树击杀，每个 hook 都「超时失败」，会话根本落不了库。
const GEMINI_HOOK_TIMEOUT_MS: u64 = 5_000;
static EVENTS: [HookEvent; 5] = [
    HookEvent::plain("SessionStart").with_timeout(GEMINI_HOOK_TIMEOUT_MS),
    HookEvent::plain("BeforeAgent").with_timeout(GEMINI_HOOK_TIMEOUT_MS),
    HookEvent::plain("AfterTool").with_timeout(GEMINI_HOOK_TIMEOUT_MS),
    HookEvent::plain("AfterAgent").with_timeout(GEMINI_HOOK_TIMEOUT_MS),
    HookEvent::plain("SessionEnd").with_timeout(GEMINI_HOOK_TIMEOUT_MS),
];

/// Gemini 0.50 支持的全部 hook 事件。写进一条它不认识的 event 会怎样尚未实测，但 kimi 的前车之鉴
/// （一条非法 event 令**全部** hooks 静默失效）足以让这张白名单值得存在——EVENTS 有针对它的绊线测试。
pub const EVENT_WHITELIST: [&str; 11] = [
    "BeforeTool",
    "AfterTool",
    "BeforeAgent",
    "AfterAgent",
    "BeforeModel",
    "AfterModel",
    "BeforeToolSelection",
    "SessionStart",
    "SessionEnd",
    "Notification",
    "PreCompress",
];

/// `settings.json` 与 claude 的同名文件同构，且同样承载 hooks 之外的用户配置（主题、模型…），
/// 故一律原样保留。文件不存在时从 `{}` 建——gemini 本就允许它缺席。
///
/// `ps_call_operator`：gemini 在 Windows 恒经 PowerShell 执行 hook（0.51 `getShellConfiguration`
/// 没有 cmd 分支），引号开头的 `"exe" --provider gemini` 是字符串表达式、解析即以 code 1 失败，
/// 必须写成 `& "exe" --provider gemini`。
static HOOKS: HookSpec = HookSpec {
    config_rel: "settings.json",
    format: ConfigFormat::ClaudeJson,
    missing: MissingConfig::CreateFrom("{}"),
    events: &EVENTS,
    command: CommandSpec {
        quote_exe: true,
        with_provider: true,
        ps_call_operator: true,
    },
};

/// 纯 JS，没有原生二进制：**必须**走 node 包装。
///
/// 实测 npm 全局（Windows）：`%APPDATA%\npm\node_modules\@google\gemini-cli\bundle\gemini.js`，
/// 而 `%APPDATA%\npm\gemini.exe` **并不存在**——npm 只生成 `gemini.cmd` / `gemini.ps1` / `gemini`(sh)。
/// 所以 Windows 上绝不能指望 `OnPath`（它查的是 `gemini.exe`），只能直取那个 js。
///
/// unix 侧 npm 全局前缀五花八门（`/usr/local`、`~/.npm-global`、nvm 的版本目录…），逐一枚举是徒劳；
/// 好在那里的 shim 是无扩展名的 `gemini`，`OnPath` 正好能兜住，故末位留它。
static LAUNCH: LaunchSpec = LaunchSpec {
    stem: "gemini",
    candidates: &[
        LaunchCandidate::NodeScript {
            root: Root::Env("APPDATA"),
            rel: "npm/node_modules/@google/gemini-cli/bundle/gemini.js",
        },
        // APPDATA 缺失的环境里由 USERPROFILE 推。
        LaunchCandidate::NodeScript {
            root: Root::Env("USERPROFILE"),
            rel: "AppData/Roaming/npm/node_modules/@google/gemini-cli/bundle/gemini.js",
        },
        LaunchCandidate::OnPath,
    ],
};

/// gemini（node/undici）的代理支持，实测 0.50 bundle：
/// - 读 `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`（大小写都认），用 `setGlobalDispatcher(new ProxyAgent)`
///   挂到全局 fetch。
/// - **SOCKS 支持**：bundle 里 `socks-proxy-agent` / `SocksProxyAgent` / `socks://` 明确接线，
///   socks 串同样从 `HTTPS_PROXY` 读（不读 `ALL_PROXY`），故 socks_keys 与 http_keys 同。
/// - 只认进程环境变量、不写进自己的配置（`config_env=false`）：与 codex/kimi 一样，只覆盖从 Meowo
///   打开的会话。
static PROXY: crate::proxy::ProxySpec = crate::proxy::ProxySpec {
    socks: true,
    config_env: false,
    http_keys: &["HTTPS_PROXY", "HTTP_PROXY"],
    socks_keys: &["HTTPS_PROXY", "HTTP_PROXY"],
};

static VARIANTS: [Variant; 1] = [Variant {
    tag: "stable",
    data_dir: DataDirSpec {
        // Gemini CLI 未见「数据目录」级别的环境变量覆盖（`GEMINI_API_KEY` 之类只管鉴权），故不声明。
        env: None,
        candidates: &[".gemini"],
    },
    hooks: &HOOKS,
    auth: Some(&AUTH),
    launch: &LAUNCH,
}];

pub struct Gemini;

impl AgentPlugin for Gemini {
    fn screen_rules(&self) -> &'static [crate::screen::ScreenRule] {
        screen::RULES
    }

    fn id(&self) -> AgentId {
        id::GEMINI
    }
    fn display_name(&self) -> &'static str {
        "Gemini CLI"
    }
    fn variants(&self) -> &'static [Variant] {
        &VARIANTS
    }

    /// **不得不收 `node`。** gemini 没有自己的可执行——会话本体就是一个跑着 `bundle/gemini.js` 的
    /// node 进程，不收它，owner_pid 上溯就找不到会话宿主（PID 抓不到 → 判活拿不到依据），
    /// 整条会话生命周期都会瘸。
    ///
    /// 代价是 `is_agent_process("node.exe")` 从此为真，判活因而变宽：某个 gemini 会话的 PID 若被
    /// 系统回收、又恰好落给另一个 node 进程，那个已死的会话会被误判为仍然活着。这是**已知且被接受**
    /// 的取舍——反过来（不收 node）的代价是每个 gemini 会话从一开始就没有 PID，那是必然的坏，
    /// 而这个是偶然的坏。
    ///
    /// 上溯本身不受这层宽泛影响：reporter 是 gemini 派生的 hook 子进程，父链上第一个 node 必是它。
    fn process_names(&self) -> &'static [&'static str] {
        &["gemini", "gemini.exe", "node", "node.exe"]
    }

    /// Gemini 的事件名 → meowo 规范名。这是本插件存在的主要理由，见模块文档的对照表。
    fn canonical_event<'a>(&self, raw: &'a str) -> &'a str {
        match raw {
            "BeforeAgent" => "UserPromptSubmit",
            "AfterAgent" => "Stop",
            "AfterTool" => "PostToolUse",
            other => other,
        }
    }

    fn resume_args(&self) -> &'static [&'static str] {
        &["--resume"]
    }
    /// `/model` 是交互式菜单（不接受内联参数，故无 model_presets）。声明它，GUI 就能
    /// 发出这条命令再把弹出的菜单渲染成按钮——模型清单由 CLI 现给。
    fn model_menu_command(&self) -> Option<&'static str> {
        Some("/model")
    }
    /// Esc 中断当前回合(gemini CLI 的取消键即 Esc)。**约定推断**:本机 ConPTY 探针能启动
    /// gemini,但提交 prompt 后它立即 SessionEnd 退出,抓不到「生成中·esc to cancel」那一屏;
    /// 依据同 kimi(通行约定 + claude/codex 均 Esc)。「打断并发送」低风险(见 kimi 注释)。
    fn interrupt_input(&self) -> Option<&'static str> {
        Some("\x1b")
    }
    /// `@路径` 是 gemini 的一等注入语法(官方文档;headless 实测提交 `@绝对路径` 能答出
    /// 文件内容)。版本探测不到时兜底 false。
    fn attachment_mention(&self, version: Option<&str>) -> bool {
        version.is_some()
    }
    /// gemini 没有 `/status`/`/compact`——对应物叫 `/stats` 与 `/compress`。此前前端的通用
    /// fallback 给它补全 `/status`，发进去只会报 unknown command。
    fn slash_commands(&self) -> &'static [&'static str] {
        &[
            "/chat",
            "/clear",
            "/compress",
            "/help",
            "/mcp",
            "/memory",
            "/stats",
            "/tools",
        ]
    }
    /// 启动选项（实测 `gemini --help`）：审批形态。`auto_edit` 走 `--approval-mode`，
    /// 全自动走更老且更稳的 `--yolo`。模型不声明——gemini 的模型名随版本更迭。
    fn launch_options(&self) -> &'static [crate::LaunchOption] {
        use crate::{LaunchChoice, LaunchOption};
        static OPTIONS: [LaunchOption; 1] = [LaunchOption {
            id: "approval",
            default: "default",
            choices: &[
                LaunchChoice {
                    id: "default",
                    label: "Default",
                    args: &[],
                },
                LaunchChoice {
                    id: "autoEdit",
                    label: "Auto Edit",
                    args: &["--approval-mode", "auto_edit"],
                },
                LaunchChoice {
                    id: "yolo",
                    label: "YOLO",
                    args: &["--yolo"],
                },
            ],
        }];
        &OPTIONS
    }
    /// 自定义命令是 TOML：`~/.gemini/commands/**.toml` + 项目级 `.gemini/commands/`；
    /// 子目录按 `:` 命名空间（`commands/git/commit.toml` → `/git:commit`），描述取顶层
    /// `description` 键。
    fn custom_commands(&self) -> Option<&'static crate::CustomCommandSpec> {
        static SPEC: crate::CustomCommandSpec = crate::CustomCommandSpec {
            user_dir: Some("commands"),
            project_dir: Some(".gemini/commands"),
            ext: "toml",
            namespace_sep: Some(":"),
        };
        Some(&SPEC)
    }

    /// 只有 npm 一条安装路，官方没有 `curl|sh` 引导脚本——用 [`InstallScript::Command`] 直接跑
    /// `npm i -g @google/gemini-cli`（两平台命令一致）。前提是本机有 node/npm；没有则安装子进程
    /// 会以「npm 找不到」失败，用户看到重试按钮——这合理，没有 node 本就装不了 gemini-cli。
    fn install_script(&self, _windows: bool) -> Option<crate::install::InstallScript> {
        Some(crate::install::InstallScript::Command {
            body: "npm install -g @google/gemini-cli",
            unix_shell: "bash",
        })
    }

    fn writes_tab_token(&self) -> bool {
        true
    }

    fn telemetry(&self) -> Option<&'static dyn TelemetryCap> {
        Some(&telemetry::TELEMETRY)
    }
    fn account(&self) -> Option<&'static dyn crate::account::AccountCap> {
        Some(&account::ACCOUNT)
    }
    /// OAuth 已死（个人版 Code Assist 被停），API Key 是个人用户唯一的活路——而 gemini 没有
    /// 「输入 key」的登录子命令，只能由宿主替用户把 key 落进 `~/.gemini/.env`。见 account.rs。
    fn api_key_login(&self) -> Option<&'static dyn crate::account::ApiKeyLoginCap> {
        Some(&account::API_KEY_LOGIN)
    }
    fn proxy(&self) -> Option<&'static crate::proxy::ProxySpec> {
        Some(&PROXY)
    }
    fn relay(&self) -> Option<&'static dyn crate::RelayCap> {
        Some(&RELAY)
    }
    /// **上下文占用不支持**：Gemini 的 hook 负载里不带 token 计数（实测 0.50——`AfterAgent` 只给
    /// `prompt_response` 正文），而为了一个百分比去解析它的会话文件不划算。见 telemetry.rs。
    fn provides_context(&self) -> bool {
        false
    }
}

// ═══ API 中转 ═══
//
// gemini-cli 认这四个环境变量（实测 0.50 bundle）：`GEMINI_API_KEY`（密钥）、`GOOGLE_GEMINI_BASE_URL`
// （自定义端点，代码里 `baseUrl || process.env["GOOGLE_GEMINI_BASE_URL"]`）、`GEMINI_MODEL`（模型）、
// `GEMINI_DEFAULT_AUTH_TYPE`（强制走 API-key 认证，取值 `gemini-api-key`——否则 TUI 仍可能去跑 OAuth）。
//
// **只有一种协议**：中转端点必须讲 Gemini 自己的 generateContent 格式（不是 OpenAI/Anthropic），
// 故 protocols 留空（validate 对空协议表不校验），auth 只有 API Key 一种。

static RELAY: GeminiRelay = GeminiRelay;
static RELAY_AUTH: [crate::RelayOption; 1] = [crate::RelayOption {
    value: "api_key",
    label: "API Key",
}];
static RELAY_SUGGESTIONS: [crate::RelaySuggestionGroup; 1] = [crate::RelaySuggestionGroup {
    protocol: "",
    models: &["gemini-2.5-pro", "gemini-2.5-flash"],
}];

pub struct GeminiRelay;

impl crate::RelayCap for GeminiRelay {
    fn ui(&self) -> crate::RelayUi {
        crate::RelayUi {
            protocols: &[],
            auth_modes: &RELAY_AUTH,
            default_protocol: "",
            default_auth: "api_key",
            suggestions: &RELAY_SUGGESTIONS,
            env_options: &[],
        }
    }
    fn launch_env(&self, config: crate::RelayConfig<'_>, key: &str) -> Vec<(String, String)> {
        vec![
            ("GEMINI_API_KEY".into(), key.into()),
            (
                "GOOGLE_GEMINI_BASE_URL".into(),
                config.base_url.trim().trim_end_matches('/').into(),
            ),
            ("GEMINI_MODEL".into(), config.model.trim().into()),
            // 强制 API-key 认证，别让 TUI 回到 OAuth。取值取自 bundle 的 AuthType 常量。
            ("GEMINI_DEFAULT_AUTH_TYPE".into(), "gemini-api-key".into()),
        ]
    }
    fn augment_argv(
        &self,
        _config: crate::RelayConfig<'_>,
        _has_secret: bool,
        argv: Vec<String>,
    ) -> Vec<String> {
        argv // 模型经 GEMINI_MODEL 注入，不改 argv
    }
    fn model_request(&self, _config: crate::RelayConfig<'_>) -> crate::RelayModelRequest {
        // 中转端点讲 Gemini 协议，`/models` 用 x-api-key 取（取不到也无妨，靠 suggestions 兜底）。
        crate::RelayModelRequest {
            auth: crate::RelayModelAuth::ApiKey,
            anthropic_version: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RelayCap;

    /// 代理：HTTP 走 HTTPS_PROXY/HTTP_PROXY；gemini **支持 SOCKS**（socks-proxy-agent），
    /// socks 串同样从 HTTPS_PROXY 读。
    #[test]
    fn proxy_accepts_http_and_socks() {
        let http = PROXY.env_for("http://127.0.0.1:7890");
        assert!(http.iter().any(|(k, _)| *k == "HTTPS_PROXY"));
        // socks 不被拒（socks=true），且写进 HTTPS_PROXY 而非 ALL_PROXY。
        let socks = PROXY.env_for("socks5://127.0.0.1:1080");
        assert!(!socks.is_empty(), "gemini 支持 SOCKS，不该返回空");
        assert!(socks
            .iter()
            .any(|(k, v)| *k == "HTTPS_PROXY" && v == "socks5://127.0.0.1:1080"));
        assert!(PROXY.accepts("socks5://h:1").is_ok());
    }

    /// 中转：把 gemini-cli 指向自定义端点的四个环境变量（变量名实测自 0.50 bundle）。
    #[test]
    fn relay_points_gemini_at_custom_endpoint() {
        let cfg = crate::RelayConfig {
            base_url: "https://relay.example/v1/", // 尾斜杠应被去掉
            model: " gemini-2.5-pro ",             // 前后空白应被 trim
            protocol: "",
            auth: "api_key",
            env_options: &[],
        };
        let env: std::collections::HashMap<_, _> =
            RELAY.launch_env(cfg, "sk-key").into_iter().collect();
        assert_eq!(
            env.get("GEMINI_API_KEY").map(String::as_str),
            Some("sk-key")
        );
        assert_eq!(
            env.get("GOOGLE_GEMINI_BASE_URL").map(String::as_str),
            Some("https://relay.example/v1")
        );
        assert_eq!(
            env.get("GEMINI_MODEL").map(String::as_str),
            Some("gemini-2.5-pro")
        );
        // 强制 API-key 认证，否则 TUI 可能回到 OAuth。
        assert_eq!(
            env.get("GEMINI_DEFAULT_AUTH_TYPE").map(String::as_str),
            Some("gemini-api-key")
        );
    }

    /// gemini 的 hook timeout 单位是毫秒（hookRunner `setTimeout`，默认 60000）。写成 claude
    /// 的秒语义（5）就是 5ms——reporter 一 spawn 即被整树击杀，每个 hook 都超时失败，
    /// 会话永远落不了库。绊线：任何事件的 timeout 掉回秒量级即失败。
    #[test]
    fn hook_timeouts_are_milliseconds_not_seconds() {
        for ev in EVENTS {
            assert!(
                ev.timeout >= 1_000,
                "{} 的 timeout={} 是秒语义残留，gemini 会按毫秒解释",
                ev.name,
                ev.timeout
            );
        }
    }

    /// Windows 上 gemini 恒经 PowerShell 执行 hook：引号开头的命令是字符串表达式、以 code 1
    /// 失败，必须带调用运算符 `&`。非 Windows（bash -c）不得带——`&` 在 bash 是后台运算符。
    /// 两种形态都必须被认领（升级迁移 + 幂等）。
    #[test]
    fn hook_command_is_powershell_invocable_on_windows() {
        let cmd = HOOKS.command.render("C:/x/meowo-reporter.exe", "gemini");
        if cfg!(windows) {
            assert_eq!(cmd, "& \"C:/x/meowo-reporter.exe\" --provider gemini");
        } else {
            assert_eq!(cmd, "\"C:/x/meowo-reporter.exe\" --provider gemini");
        }
        // 新旧两种形态都认领（旧＝无 `&` 的历史接线，升级时要能原地更新而非重复追加）。
        for form in [
            "& \"C:/x/meowo-reporter.exe\" --provider gemini",
            "\"C:/x/meowo-reporter.exe\" --provider gemini",
        ] {
            assert_eq!(
                HOOKS.command.claim(form, "gemini").as_deref(),
                Some("C:/x/meowo-reporter.exe"),
                "未认领形态：{form}"
            );
        }
    }

    /// 防连坐绊线：照 kimi 的教训，一条非法 event 有可能让**全部** hooks 静默失效。
    #[test]
    fn events_all_in_upstream_whitelist() {
        for ev in EVENTS {
            assert!(
                EVENT_WHITELIST.contains(&ev.name),
                "{} 不在 Gemini 事件白名单",
                ev.name
            );
        }
    }

    /// 声明的每个事件都必须译得出规范名，且译出的必须是 dispatch 真正消化的那几个——
    /// 少译一个，该事件就会静静地什么都不做。
    #[test]
    fn every_declared_event_maps_to_a_canonical_name_dispatch_handles() {
        // dispatch 的消化面（见 meowo_reporter::dispatch）。
        const CANONICAL: [&str; 5] = [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "Stop",
            "SessionEnd",
        ];
        for ev in EVENTS {
            let mapped = Gemini.canonical_event(ev.name);
            assert!(
                CANONICAL.contains(&mapped),
                "{} 译成了 dispatch 不认的 {mapped}",
                ev.name
            );
        }
        // 逐条钉死映射本身——写反一条（如 AfterAgent → PostToolUse）不会有任何报错，
        // 只会让「回合结束」永远不发生，卡片永远停在运行中。
        assert_eq!(Gemini.canonical_event("BeforeAgent"), "UserPromptSubmit");
        assert_eq!(Gemini.canonical_event("AfterAgent"), "Stop");
        assert_eq!(Gemini.canonical_event("AfterTool"), "PostToolUse");
        // 同名的原样透传。
        assert_eq!(Gemini.canonical_event("SessionStart"), "SessionStart");
        assert_eq!(Gemini.canonical_event("SessionEnd"), "SessionEnd");
    }

    #[test]
    fn hooks_live_in_settings_json_under_data_dir() {
        let home = std::env::temp_dir().join(format!("meowo-gemini-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".gemini")).unwrap();

        let inst = VARIANTS[0]
            .probe(id::GEMINI, &home)
            .expect("数据目录在 → 应命中");
        assert_eq!(
            inst.config_path(),
            home.join(".gemini").join("settings.json")
        );
        assert!(inst.is_configured());
        // 凭据与配置同处 data_dir（不像 opencode 那样把配置与数据分了家）。
        assert_eq!(
            inst.credentials_path(),
            Some(home.join(".gemini").join("oauth_creds.json"))
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 登录入口 = **裸启动**。gemini 没有登录子命令（`gemini --help` 里只有 mcp / extensions /
    /// skills / hooks / gemma），跑它自己首次就会引导你选认证方式。
    ///
    /// 这条曾经是反的：gemini 被声明成「无鉴权」，`login_argv()` 于是返回 None，后端回一句
    /// 「该 agent 未声明登录入口」——而前端仍旧亮着登录按钮，点下去只得到「拉起登录失败」。
    #[test]
    fn login_entry_is_a_bare_launch_not_a_subcommand() {
        let _env = crate::env_guard();
        let inst = Gemini.resolve().expect("总能推出默认落点");
        let login = inst.login_argv().expect("gemini 有登录入口（裸启动）");
        assert_eq!(
            login,
            inst.launch_argv(),
            "登录 argv 就该是启动 argv 本身，不多任何子命令"
        );
    }

    /// npm 全局的 js 入口必须走 `node` 包装：直接拿裸名 `gemini` 在 Windows 上解析不出可执行
    /// （npm 只放了 `.cmd`/`.ps1`，没有 `.exe`）。
    #[test]
    fn npm_global_yields_node_prefixed_argv() {
        let home = std::env::temp_dir().join(format!("meowo-gemini-npm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let js = crate::join_rel(
            &home,
            "AppData/Roaming/npm/node_modules/@google/gemini-cli/bundle/gemini.js",
        );
        std::fs::create_dir_all(js.parent().unwrap()).unwrap();
        std::fs::write(&js, b"").unwrap();

        // 用 USERPROFILE 那条候选（APPDATA 指向真实机器，测试里不该依赖它）。
        let _env = crate::env_guard();
        std::env::remove_var("APPDATA");
        std::env::set_var("USERPROFILE", &home);

        let argv = LAUNCH.probe(None, Some(&home)).expect("js 在 → 应命中");
        assert_eq!(argv[0], "node", "gemini 没有原生可执行，必须由 node 拉起");
        assert!(argv[1].ends_with("gemini.js"));

        let _ = std::fs::remove_dir_all(&home);
    }
}
