//! opencode 插件。它与另四家的根本差别只有一条：**opencode 没有 command hook**。
//!
//! 它的扩展面只有 TS 插件——启动时扫 `<配置目录>/{plugin,plugins}/*.ts`（源码 `config/plugin.ts` 的
//! glob，单复数都收，只扫一层）。所以 meowo 的「接线」在这里落成一份由我们生成的桥接插件：它订阅
//! opencode 的事件，把负载 spawn 给 meowo-reporter。生成与认领见 [`ConfigFormat::OpencodeTs`]。
//!
//! 这条路看着绕，实则**是五家里 reporter 侧最干净的一个**：负载由我们自己构造，直接吐成 claude
//! 同款的 `{hook_event_name, session_id, cwd}`，dispatch 一行都不用改（对比 gemini 还得译事件名）。
//!
//! 事件映射（左：opencode 的事件/hook；右：我们构造的规范名）：
//!
//! | opencode | 规范名 | 备注 |
//! |---|---|---|
//! | `event: session.created` | `SessionStart` | `properties.info` 里同时有 id 与 directory |
//! | hook `chat.message` | `UserPromptSubmit` | 顺带把 parts 里的文本拼成 `prompt`（标题靠它） |
//! | hook `tool.execute.after` | `PostToolUse` | |
//! | `event: session.idle` | `Stop` | 每个回合跑完发一次，正是 Stop 语义 |
//! | `event: session.deleted` | `SessionEnd` | opencode 没有「会话结束」，删除是唯一确定的终结 |
//! | hook `experimental.session.compacting` | `PreCompact` | /compact 开始（session id 字段名类推未验证） |
//! | `event: session.compacted` | `PostCompact` | 压缩完成（同上） |
//!
//! 因此 opencode 的会话收尾与 codex 一样靠 Stop + 判活，而不是一条可靠的 SessionEnd。

pub mod account;
mod screen;

use crate::{
    auth::{AuthScheme, CredentialSource},
    config::{CommandSpec, ConfigFormat, HookEvent, HookSpec, MissingConfig},
    id::{self, AgentId},
    launch::{LaunchCandidate, LaunchSpec, Root},
    registry::AgentPlugin,
    variant::{DataDirSpec, Variant},
};

/// 多账号：**必须隔离两个目录**——这是 opencode 独有的坑。
///
/// 它把配置与数据分了家：插件读**配置**目录（`OPENCODE_CONFIG_DIR`），凭据写**数据**目录
/// （`~/.local/share/opencode/auth.json`）。只设 `OPENCODE_CONFIG_DIR` 的话，几个 profile 的插件
/// 是分开了，凭据却仍然共用同一份 auth.json——账号看起来切了，其实压根没切。这种「看起来隔离了、
/// 实际没有」是最坏的一种失败：没有任何报错，你只会莫名其妙地在用另一个账号。
///
/// 数据目录的覆盖变量是 **`XDG_DATA_HOME`**（实测：`OPENCODE_DATA_DIR` 无效，而设了 XDG_DATA_HOME
/// 后 `opencode db path` 确实跟着搬家）。opencode 在它下面再拼一层 `opencode/`，故凭据落在
/// `<root>/data/opencode/auth.json`。
static PROFILE: crate::profile::ProfileSpec = crate::profile::ProfileSpec {
    envs: &[
        // 首条必须是 data_dir（hooks/插件落这儿）——profile.rs 有绊线测试盯着。
        ("OPENCODE_CONFIG_DIR", "config"),
        ("XDG_DATA_HOME", "data"),
    ],
    data_rel: "config",
    creds_rel: "data/opencode/auth.json",
};

/// 凭据是 `~/.local/share/opencode/auth.json`——**不在 data_dir 底下**。
///
/// opencode 把配置与数据分了家：插件必须落在**配置**目录（`~/.config/opencode`，即我们的 data_dir），
/// 凭据却写在**数据**目录。另四家两者同处一地，`CredentialSource::File`（相对 data_dir）一直够用；
/// 这一家逼出了 [`CredentialSource::HomeFile`]（相对 home）。实测依据：`opencode auth list` 的输出
/// 直接打印了这个路径。
///
/// 登录入口是 `opencode auth login`（交互式选 provider，再走 OAuth 或粘 API key）。
static AUTH: AuthScheme = AuthScheme {
    credentials: CredentialSource::HomeFile(".local/share/opencode/auth.json"),
    // token 由 opencode 自己维护，Meowo 只读不刷。
    refresh: None,
    default_base_url: "",
    login: Some(&["auth", "login"]),
    // `opencode auth logout` **是交互式的**（不带参数时让你从列表里选一个 provider），拿它去做
    // 非交互执行只会卡住。故声明为空，由宿主直接删 auth.json——那恰好就是「登出全部 provider」，
    // 正是这个按钮该有的语义。
    logout_args: &[],
};

/// 桥接插件转发的事件集，**写的已经是规范名**（负载由我们构造，无需再译）。
///
/// 这张表不驱动生成——模板里那几个 `hook_event_name` 是手写的实在代码。它的作用是让
/// `has_reporter`（只看 `SessionStart`）等通用逻辑有据可依，并由下方绊线测试保证「表里声明的」
/// 与「模板里真发的」永远是同一批。
static EVENTS: [HookEvent; 7] = [
    HookEvent::plain("SessionStart"),
    HookEvent::plain("UserPromptSubmit"),
    HookEvent::plain("PostToolUse"),
    HookEvent::plain("Stop"),
    HookEvent::plain("SessionEnd"),
    HookEvent::plain("PreCompact"),
    HookEvent::plain("PostCompact"),
];

/// 落点是数据目录下的 `plugin/` 子目录——该子目录**未必存在**（用户没装过插件就没有它），
/// 故 `write_atomic` 会按需建出父目录。
///
/// `MissingConfig::CreateFrom("")`：文件不存在＝还没接线，从空文本起生成即可。这与 kimi 的
/// 「配置缺失＝没登录，拒绝创建」不同——那是**用户的**文件，而这个文件从来只有 meowo 会写。
static HOOKS: HookSpec = HookSpec {
    config_rel: "plugin/meowo-reporter.ts",
    format: ConfigFormat::OpencodeTs,
    missing: MissingConfig::CreateFrom(""),
    events: &EVENTS,
    // 这两项在 OpencodeTs 下不参与「渲染命令行」（插件里 spawn 的是 argv 数组，不拼字符串），
    // 但仍是认领语义的一部分：`--provider opencode` 与模板里的 argv 一致。
    command: CommandSpec {
        quote_exe: false,
        with_provider: true,
        ps_call_operator: false,
    },
};

/// opencode 是 **bun 编译出的原生二进制**（实测 1.17.20：npm 包的 `bin` 直指 `bin/opencode.exe`，
/// 184 MB，postinstall 拉平台包）。所以不像 gemini 那样要 node 包装，也因此有个干净的进程名可查。
static LAUNCH: LaunchSpec = LaunchSpec {
    stem: "opencode",
    candidates: &[
        // npm 全局（实测 Windows）：%APPDATA%\npm\node_modules\opencode-ai\bin\opencode.exe
        LaunchCandidate::Exe {
            root: Root::Env("APPDATA"),
            sub: "npm/node_modules/opencode-ai/bin",
        },
        LaunchCandidate::Exe {
            root: Root::Env("USERPROFILE"),
            sub: "AppData/Roaming/npm/node_modules/opencode-ai/bin",
        },
        // 官方 `curl -fsSL https://opencode.ai/install | bash` 的落点。
        LaunchCandidate::Exe {
            root: Root::Home,
            sub: ".opencode/bin",
        },
        // brew / scoop / AUR 等：交给 PATH。
        LaunchCandidate::OnPath,
    ],
};

/// 数据目录＝**配置目录**（插件往这里放），不是 `~/.local/share/opencode`（那里只有 SQLite 库）。
///
/// Windows 上同样是 `~/.config/opencode`——opencode 用 `xdg-basedir`，而那个包没有 Windows 特判，
/// 于是 `%APPDATA%` 完全不参与。这一条**已实测**（1.17.20 首次运行即建出 `C:\Users\<u>\.config\opencode`），
/// 别想当然地改成 AppData。
/// opencode（Bun 编译）的代理支持，实测二进制：Bun 运行时的 fetch 认 `HTTP_PROXY`/`HTTPS_PROXY`/
/// `NO_PROXY`（大小写都在）。
///
/// **SOCKS 不声明**：二进制里 `SocksProxyAgent` 仅 3 处、未见接到主 API 请求上——宁可只认稳的 HTTP
/// 代理，也不给一个可能静默失效的 socks（填错的代价就是「设了却连不上」，那正是最该避免的）。
/// 只认环境变量（`config_env=false`）。
static PROXY: crate::proxy::ProxySpec = crate::proxy::ProxySpec {
    socks: false,
    config_env: false,
    http_keys: &["HTTPS_PROXY", "HTTP_PROXY"],
    socks_keys: &[],
};

static VARIANTS: [Variant; 1] = [Variant {
    tag: "stable",
    data_dir: DataDirSpec {
        env: Some("OPENCODE_CONFIG_DIR"),
        // `~/.opencode` 也在 opencode 的配置目录搜索链上（`config/paths.ts`），作次选。
        candidates: &[".config/opencode", ".opencode"],
    },
    hooks: &HOOKS,
    auth: Some(&AUTH),
    launch: &LAUNCH,
}];

pub struct Opencode;

impl AgentPlugin for Opencode {
    fn screen_rules(&self) -> &'static [crate::screen::ScreenRule] {
        screen::RULES
    }

    fn id(&self) -> AgentId {
        id::OPENCODE
    }
    fn display_name(&self) -> &'static str {
        "OpenCode"
    }
    fn variants(&self) -> &'static [Variant] {
        &VARIANTS
    }
    /// 原生二进制，进程名就是它自己——不必像 gemini 那样把 `node` 收进来。
    /// reporter 由插件在 opencode 进程内 `Bun.spawn` 派生，故父链上第一个 agent 进程必是它。
    fn process_names(&self) -> &'static [&'static str] {
        &["opencode", "opencode.exe"]
    }
    /// `opencode --session <id>`（`--continue` 是「续最近一个」，不接 id，表达不了「恢复指定会话」）。
    fn resume_args(&self) -> &'static [&'static str] {
        &["--session"]
    }
    /// opencode 的切模型命令是 **`/models`**(复数,弹选择器;没有 `/model`——下方
    /// slash_commands 的注释与 registry 测试都钉着这一事实,此前这里却声明成 `/model`,
    /// 发出去是 unknown command,模型按钮 20 秒识别窗白等)。菜单不接受内联参数,故无
    /// model_presets;其全屏选择器的形态未经真机取证,识别不到时 GUI 会给「去终端」横幅。
    fn model_menu_command(&self) -> Option<&'static str> {
        Some("/models")
    }
    /// `opencode models` 直接把完整清单打到 stdout（每行一个 `provider/model`，实测
    /// 2026-08-06 / 1.18.14）。有它就不必走 TUI 菜单那条路——opencode 的 TUI 输入在
    /// ConPTY 下至今不通（`/models` 发不进去，两轮取证均如此），而这条命令一问就有，
    /// 还不打扰正在跑的会话。
    fn model_list_command(&self) -> Option<&'static [&'static str]> {
        Some(&["models"])
    }
    /// Esc 中断当前回合。**约定推断**且**可用性存疑**:opencode 的全屏 Go TUI 在 ConPTY
    /// 探针下输入通道整条不通(`/models` 都发不进去),故连 Esc 能不能被它收到都未验证——
    /// 声明它只是让「打断并发送」按钮出现,实际是否奏效取决于 opencode 的输入路径是否可用
    /// (这是比中断键更底层的兼容问题)。风险仍低(见 kimi 注释)。
    fn interrupt_input(&self) -> Option<&'static str> {
        Some("\x1b")
    }
    /// opencode 的切模型命令是 `/models`（复数，弹选择器），没有 `/model`/`/status`/`/clear`
    /// ——清上下文对应 `/new`。此前前端的通用 fallback 把这三个都补给它了。
    fn slash_commands(&self) -> &'static [&'static str] {
        &[
            "/compact", "/exit", "/help", "/init", "/models", "/new", "/share", "/undo",
        ]
    }
    /// 自定义命令：`<配置目录>/command/*.md`（我们的 data_dir 正是它的配置目录）+ 项目级
    /// `.opencode/command/`。嵌套目录的命名语义未验证过 → 只收顶层，宁可少收也不编造名字。
    fn custom_commands(&self) -> Option<&'static crate::CustomCommandSpec> {
        static SPEC: crate::CustomCommandSpec = crate::CustomCommandSpec {
            user_dir: Some("command"),
            project_dir: Some(".opencode/command"),
            ext: "md",
            namespace_sep: None,
        };
        Some(&SPEC)
    }
    /// 一键安装：
    /// - **Unix**：官方引导脚本 `https://opencode.ai/install`（bash；它自己按平台拉预编译二进制）。
    /// - **Windows**：官方脚本是 bash，装不了；走 npm（`opencode-ai` 的 postinstall 拉平台包）。
    fn install_script(&self, windows: bool) -> Option<crate::install::InstallScript> {
        Some(if windows {
            crate::install::InstallScript::Command {
                body: "npm install -g opencode-ai",
                unix_shell: "bash",
            }
        } else {
            crate::install::InstallScript::Fetch {
                url: "https://opencode.ai/install",
                unix_shell: "bash",
            }
        })
    }
    fn account(&self) -> Option<&'static dyn crate::account::AccountCap> {
        Some(&account::ACCOUNT)
    }
    fn profile(&self) -> Option<&'static crate::profile::ProfileSpec> {
        Some(&PROFILE)
    }
    fn proxy(&self) -> Option<&'static crate::proxy::ProxySpec> {
        Some(&PROXY)
    }
    fn relay(&self) -> Option<&'static dyn crate::RelayCap> {
        Some(&RELAY)
    }
    /// **上下文占用不支持**：opencode 没声明 telemetry（其会话 token 在它自己的 SQLite 库里，
    /// 不经 hook 负载给出），meowo 这侧拿不到——如实标注「不支持」，不留空白。
    fn provides_context(&self) -> bool {
        false
    }
}

// ═══ API 中转 ═══
//
// opencode 天生就是「自带 provider」的：中转＝往它的配置里加一个自定义 provider。它没有 kimi 那种
// 单一 base_url 环境变量，但认 **`OPENCODE_CONFIG_CONTENT`**——一段内联 JSON 配置，与磁盘上的配置
// （及 `plugin/` 下自动加载的 reporter 插件）**合并**，不是替换（实测：设了它跑 `opencode models`，
// 自定义 provider 的模型如实列出，reporter 插件照常在）。
//
// 于是 launch_env 只回一个环境变量：一段声明了自定义 provider（baseURL + apiKey + 模型）并把默认
// `model` 指向它的 JSON。协议决定用哪个 ai-sdk 适配包：OpenAI 兼容端点用 `@ai-sdk/openai-compatible`，
// Anthropic 格式用 `@ai-sdk/anthropic`（二进制里这两个包名各出现 200+ / 90+ 次，是它内置支持的）。
// key 走 `options.apiKey`，ai-sdk 按包各自加正确的鉴权头（openai→Bearer、anthropic→x-api-key），
// 不必我们操心。

static RELAY: OpencodeRelay = OpencodeRelay;
static RELAY_PROTOCOLS: [crate::RelayOption; 2] = [
    crate::RelayOption {
        value: "openai",
        label: "OpenAI 兼容",
    },
    crate::RelayOption {
        value: "anthropic",
        label: "Anthropic Messages",
    },
];
static RELAY_AUTH: [crate::RelayOption; 1] = [crate::RelayOption {
    value: "bearer",
    label: "Bearer Token",
}];
static RELAY_SUGGESTIONS: [crate::RelaySuggestionGroup; 2] = [
    crate::RelaySuggestionGroup {
        protocol: "openai",
        models: &["gpt-5.4", "gpt-5.3-codex"],
    },
    crate::RelaySuggestionGroup {
        protocol: "anthropic",
        models: &["claude-sonnet-5", "claude-opus-4-8"],
    },
];

pub struct OpencodeRelay;

impl crate::RelayCap for OpencodeRelay {
    fn ui(&self) -> crate::RelayUi {
        crate::RelayUi {
            protocols: &RELAY_PROTOCOLS,
            auth_modes: &RELAY_AUTH,
            default_protocol: "openai",
            default_auth: "bearer",
            suggestions: &RELAY_SUGGESTIONS,
            env_options: &[],
        }
    }
    fn launch_env(&self, config: crate::RelayConfig<'_>, key: &str) -> Vec<(String, String)> {
        let npm = if config.protocol == "anthropic" {
            "@ai-sdk/anthropic"
        } else {
            "@ai-sdk/openai-compatible"
        };
        let model = config.model.trim();
        let base = config.base_url.trim().trim_end_matches('/');
        // 用 serde_json 组装，base_url/model/key 里的特殊字符自动转义（都可能来自用户输入）。
        let content = serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "meowo-relay": {
                    "npm": npm,
                    "name": "Meowo Relay",
                    "options": { "baseURL": base, "apiKey": key },
                    "models": { model: { "name": model } }
                }
            },
            "model": format!("meowo-relay/{model}")
        });
        vec![("OPENCODE_CONFIG_CONTENT".into(), content.to_string())]
    }
    fn augment_argv(
        &self,
        _config: crate::RelayConfig<'_>,
        _has_secret: bool,
        argv: Vec<String>,
    ) -> Vec<String> {
        argv // 模型经配置内容的默认 model 指定，不改 argv
    }
    fn model_request(&self, config: crate::RelayConfig<'_>) -> crate::RelayModelRequest {
        crate::RelayModelRequest {
            auth: if config.protocol == "anthropic" {
                crate::RelayModelAuth::ApiKey
            } else {
                crate::RelayModelAuth::Bearer
            },
            anthropic_version: config.protocol == "anthropic",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EnsureOutcome;
    use crate::RelayCap;

    /// 代理：认 HTTP 代理（HTTPS_PROXY/HTTP_PROXY）；**不支持 SOCKS**——给 socks 串时 env_for
    /// 返回空，绝不写一个它不认识的代理让请求静默连不上。
    #[test]
    fn proxy_accepts_http_but_rejects_socks() {
        let http = PROXY.env_for("http://127.0.0.1:7890");
        assert!(http.iter().any(|(k, _)| *k == "HTTPS_PROXY"));
        assert!(PROXY.accepts("socks5://h:1").is_err());
        assert!(PROXY.env_for("socks5://127.0.0.1:1080").is_empty());
    }

    /// 中转：只回一个 `OPENCODE_CONFIG_CONTENT`，内容是一段声明自定义 provider 并把默认 model
    /// 指向它的 JSON。协议决定 ai-sdk 适配包。
    #[test]
    fn relay_injects_custom_provider_config() {
        let cfg = crate::RelayConfig {
            base_url: "https://relay.example/v1/",
            model: " gpt-5.4 ",
            protocol: "openai",
            auth: "bearer",
            env_options: &[],
        };
        let env = RELAY.launch_env(cfg, "sk-key");
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "OPENCODE_CONFIG_CONTENT");
        let v: serde_json::Value = serde_json::from_str(&env[0].1).expect("合法 JSON");
        let p = &v["provider"]["meowo-relay"];
        assert_eq!(p["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(p["options"]["baseURL"], "https://relay.example/v1"); // 尾斜杠去掉
        assert_eq!(p["options"]["apiKey"], "sk-key");
        assert_eq!(p["models"]["gpt-5.4"]["name"], "gpt-5.4"); // model 已 trim
        assert_eq!(v["model"], "meowo-relay/gpt-5.4");
    }

    /// anthropic 协议换适配包（openai-compatible → anthropic）。
    #[test]
    fn relay_switches_sdk_package_by_protocol() {
        let cfg = crate::RelayConfig {
            base_url: "https://r",
            model: "claude-sonnet-5",
            protocol: "anthropic",
            auth: "bearer",
            env_options: &[],
        };
        let env = RELAY.launch_env(cfg, "k");
        let v: serde_json::Value = serde_json::from_str(&env[0].1).unwrap();
        assert_eq!(v["provider"]["meowo-relay"]["npm"], "@ai-sdk/anthropic");
    }

    /// 生成的插件里真正 `hook_event_name` 的那几个，必须与 [`EVENTS`] 声明的完全一致。
    ///
    /// 这条绊线守的是一个静默故障：模板里加了个事件却忘了写进 EVENTS，`has_reporter` 之类按表
    /// 判断的逻辑就会与实际行为脱节；反过来（表里有、模板不发）则是声明了一个永远不会到来的事件。
    #[test]
    fn template_emits_exactly_the_declared_events() {
        let plugin_src = match HOOKS.ensure_hooks("", "C:/x/meowo-reporter.exe", "opencode") {
            EnsureOutcome::Changed(s) => s,
            other => panic!("空文本应生成插件，实得 {other:?}"),
        };
        for ev in EVENTS {
            assert!(
                plugin_src.contains(&format!("hook_event_name: \"{}\"", ev.name)),
                "模板没发 {}，但 EVENTS 声明了它",
                ev.name
            );
        }
        // 反向：模板里出现的 hook_event_name 不能多于 EVENTS。
        let emitted = plugin_src.matches("hook_event_name: \"").count();
        assert_eq!(emitted, EVENTS.len(), "模板发的事件数与 EVENTS 不符");
    }

    /// 接线产物落在 `plugin/` 子目录里，且认领能把 reporter 路径读回来（换路径即重写）。
    #[test]
    fn plugin_lands_under_plugin_subdir_and_roundtrips() {
        let home = std::env::temp_dir().join(format!("meowo-opencode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(crate::join_rel(&home, ".config/opencode")).unwrap();

        let inst = VARIANTS[0]
            .probe(id::OPENCODE, &home)
            .expect("配置目录在 → 应命中");
        assert_eq!(
            inst.config_path(),
            crate::join_rel(&home, ".config/opencode")
                .join("plugin")
                .join("meowo-reporter.ts")
        );
        assert!(inst.is_configured());

        // 生成 → 已接入；SessionStart 是 has_reporter 的判据。
        let reporter = "C:/x/meowo-reporter.exe";
        let src = match HOOKS.ensure_hooks("", reporter, "opencode") {
            EnsureOutcome::Changed(s) => s,
            other => panic!("期望 Changed，实得 {other:?}"),
        };
        assert!(HOOKS.has_reporter(&src, "opencode"));
        assert_eq!(
            HOOKS.claimed_reporter(&src, "opencode").as_deref(),
            Some(reporter)
        );
        assert_eq!(
            HOOKS.ensure_hooks(&src, reporter, "opencode"),
            EnsureOutcome::Unchanged
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 配置目录优先 `~/.config/opencode`，`~/.opencode` 作次选（两者都在 opencode 的搜索链上）。
    #[test]
    fn prefers_xdg_config_dir_over_dot_opencode() {
        let home = std::env::temp_dir().join(format!("meowo-opencode-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);

        // 只有 ~/.opencode → 用它。
        std::fs::create_dir_all(home.join(".opencode")).unwrap();
        assert_eq!(
            VARIANTS[0].probe(id::OPENCODE, &home).unwrap().data_dir,
            home.join(".opencode")
        );

        // ~/.config/opencode 出现 → 抢先。
        let xdg = crate::join_rel(&home, ".config/opencode");
        std::fs::create_dir_all(&xdg).unwrap();
        assert_eq!(
            VARIANTS[0].probe(id::OPENCODE, &home).unwrap().data_dir,
            xdg
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// npm 全局装的是**原生 exe**（不是 js），故 argv 就是它本身——不该混进 `node`。
    #[test]
    fn npm_global_yields_native_exe_not_node() {
        let home = std::env::temp_dir().join(format!("meowo-opencode-npm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let bin = crate::join_rel(&home, "AppData/Roaming/npm/node_modules/opencode-ai/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(crate::exe_file_name("opencode"));
        std::fs::write(&exe, b"").unwrap();

        let _env = crate::env_guard();
        std::env::remove_var("APPDATA");
        std::env::set_var("USERPROFILE", &home);

        let argv = LAUNCH.probe(None, Some(&home)).expect("exe 在 → 应命中");
        assert_eq!(argv, vec![exe.to_string_lossy().into_owned()]);
        assert_ne!(argv[0], "node");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 凭据**不在 data_dir 底下**：插件落配置目录（`~/.config/opencode`），凭据在数据目录
    /// （`~/.local/share/opencode/auth.json`）。若按「相对 data_dir」解析，就会去
    /// `~/.config/opencode/auth.json` 找一个永远不存在的文件——登录态于是恒为「未登录」，
    /// 登录按钮永远不消失。
    #[test]
    fn credentials_live_in_the_data_dir_not_the_config_dir() {
        let _env = crate::env_guard();
        let home = std::env::temp_dir().join(format!("meowo-oc-cred-{}", std::process::id()));
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("HOME", &home);
        // OPENCODE_CONFIG_DIR 若被真实环境设过，会改写 data_dir，干扰断言。
        std::env::remove_var("OPENCODE_CONFIG_DIR");

        let inst = VARIANTS[0].installation_at(
            id::OPENCODE,
            crate::join_rel(&home, ".config/opencode"),
            Some(&home),
        );
        assert_eq!(
            inst.credentials_path(),
            Some(crate::join_rel(&home, ".local/share/opencode/auth.json")),
            "凭据该相对 home 解析，而不是拼到配置目录下"
        );
        // 登录入口：`opencode auth login`。
        let login = inst.login_argv().expect("opencode 有登录入口");
        assert_eq!(
            &login[login.len() - 2..],
            &["auth".to_string(), "login".to_string()]
        );
    }

    /// resume 是 `--session <id>`——写成 `--continue` 会拉起「最近一个会话」而不是点开的那个。
    #[test]
    fn resume_targets_the_given_session() {
        let _env = crate::env_guard();
        let argv = Opencode
            .resume_argv("ses_abc")
            .expect("声明了 resume 子命令");
        let n = argv.len();
        assert_eq!(
            &argv[n - 2..],
            &["--session".to_string(), "ses_abc".to_string()]
        );
    }
}
