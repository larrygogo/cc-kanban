//! codex（OpenAI Codex CLI）插件。数据目录只有一种（`~/.codex`），复杂度全在**可执行的三种落法**：
//!
//! | 优先级 | 安装方式 | argv |
//! |---|---|---|
//! | 1 | bun 全局（用户多用 bun 装/更新） | `[~/.bun/bin/codex]` |
//! | 2 | npm 全局 | `["node", "<npm>/node_modules/@openai/codex/bin/codex.js"]` |
//! | 3 | 官方独立安装 | `[<data>/packages/standalone/current/bin/codex]` |
//!
//! npm 那条**必须**走 node 包装：直接拉原生 codex.exe 不会真正恢复会话（无 rollout、无 hook）。
//! npm 副本常是过期版（resume 会拉到旧版、每次提示更新），故排在 bun 之后。
//! 独立安装那条也是修「装完仍显示未安装」的关键：安装脚本只改持久 PATH，运行中的 meowo-app
//! 进程 PATH 是启动时的旧快照、看不到新目录，故直查这个固定路径。
//!
//! 接线还有一步**副作用**（往 `config.toml` 写 `[hooks.state]` 的 trusted_hash 预信任）——见 `setup.rs`。

pub mod account;
mod screen;
pub mod setup;
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

/// 接线事件集：dispatch 消化面 ∩ codex 支持面。无 SessionEnd（codex 不支持，会话收尾靠
/// Stop + liveness）；不配 PreToolUse（其 matcher 目标是 claude 专属工具）。
/// PreCompact/PostCompact 的支持面已在实装二进制上取证（0.152.0 的 HookEventsToml 枚举
/// 字符串里两个事件名俱在），供 /compact 期间显示「正在压缩」。
static EVENTS: [HookEvent; 7] = [
    HookEvent::plain("SessionStart"),
    HookEvent::plain("UserPromptSubmit"),
    HookEvent::plain("PostToolUse"),
    HookEvent::plain("Stop"),
    HookEvent::plain("PreCompact"),
    HookEvent::plain("PostCompact"),
    HookEvent::plain("PermissionRequest").with_timeout(310),
];

/// hooks.json 不存在时从空态建——与 kimi「config.toml 缺失即未登录」不同是有意的：
/// 此处不存在不代表用户手改过畸形内容，codex 本就允许该文件缺席。
static HOOKS: HookSpec = HookSpec {
    config_rel: "hooks.json",
    format: ConfigFormat::CodexJson,
    missing: MissingConfig::CreateFrom("{\"hooks\":{}}"),
    events: &EVENTS,
    command: CommandSpec {
        quote_exe: true,
        with_provider: true,
        ps_call_operator: false,
    },
};

/// codex 的 `auth.json` 由 CLI 自己维护（含 OIDC id_token），Meowo 只读不刷新 → `refresh: None`。
/// 用量走 rollout 文件与 auth.json 内的字段，无独立 base_url。
/// 多账号：`CODEX_HOME` 一个变量搬走整个数据目录（凭据 auth.json、hooks.json、rollout 全在里面）。
static PROFILE: crate::profile::ProfileSpec = crate::profile::ProfileSpec {
    envs: &[("CODEX_HOME", "")],
    data_rel: "",
    creds_rel: "auth.json",
};

static AUTH: AuthScheme = AuthScheme {
    credentials: CredentialSource::File("auth.json"),
    refresh: None,
    default_base_url: "",
    // `codex login`（另有 `codex login status`，但 kimi 无 status 子命令，登录态检测统一走读凭据）。
    login: Some(&["login"]),
    logout_args: &["logout"],
};

static LAUNCH: LaunchSpec = LaunchSpec {
    stem: "codex",
    candidates: &[
        LaunchCandidate::Exe {
            root: Root::Home,
            sub: ".bun/bin",
        },
        // npm 全局前缀：Windows 上是 %APPDATA%\npm，某些环境 APPDATA 缺失则由 USERPROFILE 推。
        LaunchCandidate::NodeScript {
            root: Root::Env("APPDATA"),
            rel: "npm/node_modules/@openai/codex/bin/codex.js",
        },
        LaunchCandidate::NodeScript {
            root: Root::Env("USERPROFILE"),
            rel: "AppData/Roaming/npm/node_modules/@openai/codex/bin/codex.js",
        },
        // `current` 是指向当前 release 的 junction/symlink，跨平台稳定。
        LaunchCandidate::Exe {
            root: Root::DataDir,
            sub: "packages/standalone/current/bin",
        },
    ],
};

static VARIANTS: [Variant; 1] = [Variant {
    tag: "stable",
    data_dir: DataDirSpec {
        env: Some("CODEX_HOME"),
        candidates: &[".codex"],
    },
    hooks: &HOOKS,
    auth: Some(&AUTH),
    launch: &LAUNCH,
}];

pub struct Codex;

/// codex **无法**从配置文件配代理：让主进程走代理的 issue（openai/codex#6060）仍开着，PR 被拒。
/// `config.toml` 里那两个看着像的键都不是：`features.network_proxy` 是**沙箱子进程**的代理，
/// `shell_environment_policy.set` 只注入给它派生的 shell——都不影响 codex 自己的 API 请求。
/// 只剩进程环境变量一条路（reqwest 会自动读）。
///
/// SOCKS 不支持：codex-rs 的 reqwest 没编译 `socks` feature（另见 issue #20844：Windows 下
/// SOCKS5 不稳，改用 HTTP 代理即恢复）。
static PROXY: crate::proxy::ProxySpec = crate::proxy::ProxySpec {
    socks: false,
    config_env: false,
    http_keys: &["HTTPS_PROXY", "HTTP_PROXY"],
    socks_keys: &[],
};

struct CodexRelay;
static RELAY: CodexRelay = CodexRelay;
static RELAY_AUTH: [crate::RelayOption; 1] = [crate::RelayOption {
    value: "bearer",
    label: "Bearer Token",
}];
static RELAY_SUGGESTIONS: [crate::RelaySuggestionGroup; 1] = [crate::RelaySuggestionGroup {
    protocol: "",
    models: &[
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.4",
        "gpt-5.3-codex",
    ],
}];

impl crate::RelayCap for CodexRelay {
    fn ui(&self) -> crate::RelayUi {
        crate::RelayUi {
            protocols: &[],
            auth_modes: &RELAY_AUTH,
            default_protocol: "",
            default_auth: "bearer",
            suggestions: &RELAY_SUGGESTIONS,
            env_options: &[],
        }
    }
    fn launch_env(&self, _config: crate::RelayConfig<'_>, key: &str) -> Vec<(String, String)> {
        vec![("MEOWO_CODEX_RELAY_KEY".into(), key.into())]
    }
    fn augment_argv(
        &self,
        config: crate::RelayConfig<'_>,
        has_secret: bool,
        mut argv: Vec<String>,
    ) -> Vec<String> {
        if !has_secret {
            return argv;
        }
        let quoted = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
        for value in [
            "model_provider=\"meowo-relay\"".to_string(),
            "model_providers.meowo-relay.name=\"Meowo Relay\"".to_string(),
            format!(
                "model_providers.meowo-relay.base_url={}",
                quoted(config.base_url.trim().trim_end_matches('/'))
            ),
            "model_providers.meowo-relay.env_key=\"MEOWO_CODEX_RELAY_KEY\"".to_string(),
            "model_providers.meowo-relay.wire_api=\"responses\"".to_string(),
            format!("model={}", quoted(config.model.trim())),
        ] {
            argv.extend(["-c".into(), value]);
        }
        argv
    }
    fn model_request(&self, _config: crate::RelayConfig<'_>) -> crate::RelayModelRequest {
        crate::RelayModelRequest {
            auth: crate::RelayModelAuth::Bearer,
            anthropic_version: false,
        }
    }
}

impl AgentPlugin for Codex {
    fn screen_rules(&self) -> &'static [crate::screen::ScreenRule] {
        screen::RULES
    }

    fn id(&self) -> AgentId {
        id::CODEX
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    /// PermissionRequest hook 声明了 310s 阻塞（EVENTS 里的 with_timeout），决策输出会被采纳。
    fn permission_hook_decides(&self) -> bool {
        true
    }
    fn variants(&self) -> &'static [Variant] {
        &VARIANTS
    }
    fn proxy(&self) -> Option<&'static crate::proxy::ProxySpec> {
        Some(&PROXY)
    }
    fn relay(&self) -> Option<&'static dyn crate::RelayCap> {
        Some(&RELAY)
    }
    fn process_names(&self) -> &'static [&'static str] {
        // 会话本体是原生 codex 二进制；npm 包装时它由 node 启动但 hook 由 codex 自身触发，上溯命中
        // codex(.exe) 即可。不收 node.exe（过宽，会把任意 node 进程误判为 agent）。
        &["codex", "codex.exe"]
    }
    fn resume_args(&self) -> &'static [&'static str] {
        &["resume"]
    }
    /// `/model` 是交互式菜单（不接受内联参数，故无 model_presets）。声明它，GUI 就能
    /// 发出这条命令再把弹出的菜单渲染成按钮——模型清单由 CLI 现给。
    fn model_menu_command(&self) -> Option<&'static str> {
        Some("/model")
    }
    /// Esc 中断当前回合:codex 状态行自述 "esc to interrupt"(真机 capture 在档,
    /// 见 docs/research/tui-menu-captures-2026-07.md)。
    fn interrupt_input(&self) -> Option<&'static str> {
        Some("\x1b")
    }
    /// codex 的 `/model` 是交互式菜单（不接内联参数），故只列命令、不声明模型预设。
    fn slash_commands(&self) -> &'static [&'static str] {
        &[
            "/clear", "/compact", "/diff", "/help", "/model", "/new", "/review", "/status",
        ]
    }
    fn mode_controls(&self) -> &'static [crate::ModeControl] {
        static MODES: [crate::ModeControl; 1] = [crate::ModeControl {
            dimension: "collaboration",
            cycle_input: Some("\x1b[Z"),
            options: &[],
            // codex 的 TUI 指示文案未经验证，宁缺毋滥；显示仍随 transcript 状态走。
            screen_markers: &[],
        }];
        &MODES
    }
    /// 启动选项（实测 `codex --help`）：审批/沙箱形态。模型**不声明**——codex 的模型名随
    /// 版本频繁更迭，声明一份很快就过期的清单等于给出会启动失败的选项。
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
                    risk: false,
                },
                LaunchChoice {
                    id: "readOnly",
                    label: "Read Only",
                    args: &["--sandbox", "read-only"],
                    risk: false,
                },
                LaunchChoice {
                    id: "fullAuto",
                    label: "Full Auto",
                    args: &["--full-auto"],
                    risk: false,
                },
                LaunchChoice {
                    id: "yolo",
                    label: "YOLO",
                    args: &["--dangerously-bypass-approvals-and-sandbox"],
                    risk: true,
                },
            ],
        }];
        &OPTIONS
    }
    /// 自定义 prompt：`~/.codex/prompts/*.md` → `/<文件名>`。只有用户级（codex 无项目级目录），
    /// 平铺无命名空间。
    fn custom_commands(&self) -> Option<&'static crate::CustomCommandSpec> {
        static SPEC: crate::CustomCommandSpec = crate::CustomCommandSpec {
            user_dir: Some("prompts"),
            project_dir: None,
            ext: "md",
            namespace_sep: None,
        };
        Some(&SPEC)
    }
    /// 直取 GitHub Releases，**不走 `chatgpt.com`**。
    ///
    /// 官方命令是 `irm https://chatgpt.com/codex/install.ps1 | iex`，而 `chatgpt.com` 在
    /// Cloudflare 后面（实测 `server: cloudflare` + `cf-ray`），会间歇触发人机校验——其页面以
    /// HTTP 200 返回，裸管道会把那坨 HTML 喂给解释器。
    ///
    /// 但那个地址**只是一个 302**，终点就是下面这个 URL：
    ///
    /// ```text
    /// chatgpt.com/codex/install.ps1
    ///   → 302 → github.com/openai/codex/releases/latest/download/install.ps1
    ///           server: github.com → Windows-Azure-Blob，无 Cloudflare
    /// ```
    ///
    /// 两者内容逐字节相同（实测 sha256 一致）。直接取终点即可绕开 CF，不必去啃 GitHub API 的
    /// 未认证限流（60/h），也不必解压 unix 侧那些 `.tar.gz`——脚本自己会处理。
    ///
    /// `latest/download/` 是 GitHub 的稳定跳转，始终指向最新 release 的同名资产。
    fn install_script(&self, windows: bool) -> Option<crate::install::InstallScript> {
        Some(crate::install::InstallScript::Fetch {
            url: if windows {
                "https://github.com/openai/codex/releases/latest/download/install.ps1"
            } else {
                "https://github.com/openai/codex/releases/latest/download/install.sh"
            },
            unix_shell: "sh", // 官方命令写的就是 `| sh`
        })
    }
    // Codex 在首条 prompt 前尚未写 spinner/project 标题，此时 cwd 匹配没有信号；SessionStart hook
    // 先写 session token，便可精确定位空白新会话。首条 prompt 后 Codex 会覆盖 token 为自己的标题，
    // app 随即回退到 cwd 匹配，因此消息前后都可定位。它不写「任务标题」式标签，故
    // sets_terminal_tab_title 仍取默认 false。
    fn writes_tab_token(&self) -> bool {
        true
    }
    fn telemetry(&self) -> Option<&'static dyn TelemetryCap> {
        Some(&telemetry::TELEMETRY)
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

    #[test]
    fn writes_session_token_before_first_prompt() {
        assert!(Codex.writes_tab_token());
        assert!(!Codex.sets_terminal_tab_title());
    }

    #[test]
    fn permission_hook_waits_for_gui_decision() {
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

    #[test]
    fn config_and_credentials_sit_under_data_dir() {
        let home = std::env::temp_dir().join(format!("meowo-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".codex")).unwrap();

        let v = &VARIANTS[0];
        let dir = v
            .data_dir
            .candidates
            .iter()
            .map(|c| home.join(c))
            .find(|p| p.is_dir())
            .unwrap();
        let inst = v.installation_at(id::CODEX, dir, Some(&home));
        assert_eq!(inst.config_path(), home.join(".codex").join("hooks.json"));
        assert_eq!(
            inst.credentials_path(),
            Some(home.join(".codex").join("auth.json"))
        );
        assert!(inst.is_configured());
        // 三处候选都没有 → 回退裸名。
        assert_eq!(inst.launch_argv(), vec!["codex".to_string()]);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn standalone_install_under_data_dir_is_found() {
        // 「装完仍显示未安装」的回归：PATH 是旧快照，但独立安装的固定路径能直查到。
        let home =
            std::env::temp_dir().join(format!("meowo-codex-standalone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let bin = home
            .join(".codex")
            .join("packages")
            .join("standalone")
            .join("current")
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(crate::exe_file_name("codex"));
        std::fs::write(&exe, b"").unwrap();

        let v = &VARIANTS[0];
        let inst = v.installation_at(id::CODEX, home.join(".codex"), Some(&home));
        assert!(inst.is_launchable());
        assert_eq!(inst.launch_argv(), vec![exe.to_string_lossy().into_owned()]);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn bun_global_beats_standalone() {
        let home = std::env::temp_dir().join(format!("meowo-codex-bun-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let name = crate::exe_file_name("codex");
        for sub in [".codex/packages/standalone/current/bin", ".bun/bin"] {
            let d = crate::join_rel(&home, sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(&name), b"").unwrap();
        }
        let inst = VARIANTS[0].installation_at(id::CODEX, home.join(".codex"), Some(&home));
        assert_eq!(
            inst.launch_argv(),
            vec![home
                .join(".bun")
                .join("bin")
                .join(&name)
                .to_string_lossy()
                .into_owned()]
        );

        let _ = std::fs::remove_dir_all(&home);
    }
}
