//! kimi 插件。两个变体——这正是变体层存在的理由：
//!
//! | tag | 发行 | 数据目录 | hooks 默认形态 | 可执行 |
//! |---|---|---|---|---|
//! | `modern` | Node 版 **Kimi Code** | `~/.kimi-code` | 无顶层 `hooks` 键 | `<data>/bin/kimi` |
//! | `legacy` | 旧 Python 版 **kimi-cli** | `~/.kimi` | `hooks = []` 空内联数组 | `~/.local/bin/kimi` 等 |
//!
//! 两者的 hook 配置格式与 hook stdin 载荷（session_id/cwd/hook_event_name）实测一致，故共用
//! 同一份 [`HookSpec`]——差的只是目录与「空内联数组」这一形态，都已在声明里表达。

pub mod account;
mod screen;
pub mod telemetry;

use crate::{
    auth::{AuthScheme, CredentialSource, OAuthRefresh},
    caps::TelemetryCap,
    config::{CommandSpec, ConfigFormat, HookEvent, HookSpec, MissingConfig, RepairReason},
    id::{self, AgentId},
    launch::{LaunchCandidate, LaunchSpec, Root},
    registry::AgentPlugin,
    variant::{DataDirSpec, Variant},
};

/// 接线事件集。PermissionRequest = kimi 交互式等待用户审批前触发（官方源码确认，observation-only），
/// 用于卡片「待交互」显示。PreCompact/PostCompact 是压缩进行期间的「正在压缩」指示通道
/// （transcript 在压缩期间零新增字节，只能靠 hook；白名单 EVENT_WHITELIST 已含这两个名字，
/// kimi 0.29 二进制确认支持）。
static EVENTS: [HookEvent; 8] = [
    HookEvent::plain("SessionStart"),
    HookEvent::plain("UserPromptSubmit"),
    HookEvent::plain("PostToolUse"),
    HookEvent::plain("Stop"),
    HookEvent::plain("SessionEnd"),
    HookEvent::plain("PermissionRequest"),
    HookEvent::plain("PreCompact"),
    HookEvent::plain("PostCompact"),
];

/// kimi 0.20 支持的全部 hook 事件（HOOK_EVENT_TYPES）。一条非法 event 会让 kimi **静默禁用全部**
/// hooks（源码 salvageConfigData），故 EVENTS 有针对本表的绊线测试。
pub const EVENT_WHITELIST: [&str; 16] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionResult",
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "Interrupt",
    "SessionStart",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
    "Notification",
];

/// config.toml 由 `kimi login` 生成——不存在即「需先登录」，不凭空创建。
/// command 不加引号：与 kimi 现存配置的书写形态一致，避免无谓改写用户在用的条目
///（reporter 路径含空白时仍强制加引号，否则执行与认领同按空白截断，见 `CommandSpec::quote_exe`）。
static HOOKS: HookSpec = HookSpec {
    config_rel: "config.toml",
    format: ConfigFormat::KimiToml,
    missing: MissingConfig::Fail(RepairReason::NeedLogin),
    events: &EVENTS,
    command: CommandSpec {
        quote_exe: false,
        with_provider: true,
        ps_call_operator: false,
    },
};

/// 来源：kimi-code 开源包 `packages/oauth/src/constants.ts`。
/// 多账号：`KIMI_SHARE_DIR` 一个变量搬走整个数据目录（凭据、config.toml、会话 wire.jsonl 全在里面）。
/// 两个变体（modern `~/.kimi-code` / legacy `~/.kimi`）共用同一个环境变量，profile 下不必区分。
static PROFILE: crate::profile::ProfileSpec = crate::profile::ProfileSpec {
    envs: &[("KIMI_SHARE_DIR", "")],
    data_rel: "",
    creds_rel: "credentials/kimi-code.json",
};

const AUTH_MODERN: AuthScheme = AuthScheme {
    credentials: CredentialSource::File("credentials/kimi-code.json"),
    refresh: Some(OAuthRefresh {
        token_url: "https://auth.kimi.com/api/oauth/token",
        client_id: "17e5f671-d194-4dfb-9706-5516cb48c098",
    }),
    default_base_url: "https://api.kimi.com/coding/v1",
    // `kimi login`。config.toml 正是由它生成——故 MissingConfig::Fail(NeedLogin) 的提示可直接
    // 引导用户点登录（两者指向同一个动作）。
    login: Some(&["login"]),
    // 当前 kimi-code CLI 没有 logout 子命令；宿主只删除下方声明的凭据文件。
    logout_args: &[],
};

/// 旧 Python 版的凭据布局与新版相同（实测 `~/.kimi/credentials/kimi-code.json` 字段一致）。
/// **client_id 未经证实**：若刷新 token 返回 `invalid_client`，就把这里换成旧版的值——
/// 变体层的意义正在于此，届时只改这一个 const，account 侧无需再动。
const AUTH_LEGACY: AuthScheme = AUTH_MODERN;

static LAUNCH_MODERN: LaunchSpec = LaunchSpec {
    stem: "kimi",
    candidates: &[
        LaunchCandidate::Exe {
            root: Root::DataDir,
            sub: "bin",
        },
        LaunchCandidate::Exe {
            root: Root::Home,
            sub: ".kimi-code/bin",
        },
    ],
};

/// 旧版常经 uv/pipx 装到 `~/.local/bin`，不在数据目录下。
static LAUNCH_LEGACY: LaunchSpec = LaunchSpec {
    stem: "kimi",
    candidates: &[
        LaunchCandidate::Exe {
            root: Root::DataDir,
            sub: "bin",
        },
        LaunchCandidate::Exe {
            root: Root::Home,
            sub: ".kimi/bin",
        },
        LaunchCandidate::Exe {
            root: Root::Home,
            sub: ".local/bin",
        },
    ],
};

static VARIANTS: [Variant; 2] = [
    Variant {
        tag: "modern",
        data_dir: DataDirSpec {
            env: Some("KIMI_SHARE_DIR"),
            candidates: &[".kimi-code"],
        },
        hooks: &HOOKS,
        auth: Some(&AUTH_MODERN),
        launch: &LAUNCH_MODERN,
    },
    Variant {
        tag: "legacy",
        data_dir: DataDirSpec {
            env: Some("KIMI_SHARE_DIR"),
            candidates: &[".kimi"],
        },
        hooks: &HOOKS,
        auth: Some(&AUTH_LEGACY),
        launch: &LAUNCH_LEGACY,
    },
];

pub struct Kimi;

/// kimi 的 config.toml 没有 proxy 键、也没有环境变量注入机制（`[providers.*.env]` 只作 api_key /
/// base_url 的 fallback），故只能靠进程环境变量。
///
/// **三家里唯一支持 SOCKS 的**：新版 kimi-code 自 v0.12.0 起认全套
/// `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`，含 socks5。按官方文档，SOCKS 通常经
/// `ALL_PROXY` 配置——写进 `HTTPS_PROXY` 未必被识别，故 socks 单列一组键。
///
/// 注意旧的 Python 版 kimi-cli 连环境变量都不认（aiohttp `trust_env=False`，修复 PR 未合）——
/// 那个版本无论怎么配都走不了代理，这里无从区分（变体表按目录形态分，不按 CLI 实现语言分）。
static PROXY: crate::proxy::ProxySpec = crate::proxy::ProxySpec {
    socks: true,
    config_env: false,
    http_keys: &["HTTPS_PROXY", "HTTP_PROXY"],
    socks_keys: &["ALL_PROXY"],
};

struct KimiRelay;
static RELAY: KimiRelay = KimiRelay;
static RELAY_AUTH: [crate::RelayOption; 1] = [crate::RelayOption {
    value: "bearer",
    label: "Bearer Token",
}];
static RELAY_PROTOCOLS: [crate::RelayOption; 3] = [
    crate::RelayOption {
        value: "kimi",
        label: "Kimi",
    },
    crate::RelayOption {
        value: "anthropic",
        label: "Anthropic Messages",
    },
    crate::RelayOption {
        value: "openai",
        label: "OpenAI Chat Completions",
    },
];
static RELAY_SUGGESTIONS: [crate::RelaySuggestionGroup; 3] = [
    crate::RelaySuggestionGroup {
        protocol: "kimi",
        models: &["kimi-for-coding", "kimi-for-coding-highspeed"],
    },
    crate::RelaySuggestionGroup {
        protocol: "anthropic",
        models: &["claude-fable-5", "claude-opus-4-8", "claude-sonnet-5"],
    },
    crate::RelaySuggestionGroup {
        protocol: "openai",
        models: &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.4", "gpt-5.3-codex"],
    },
];

impl crate::RelayCap for KimiRelay {
    fn ui(&self) -> crate::RelayUi {
        crate::RelayUi {
            protocols: &RELAY_PROTOCOLS,
            auth_modes: &RELAY_AUTH,
            default_protocol: "kimi",
            default_auth: "bearer",
            suggestions: &RELAY_SUGGESTIONS,
            env_options: &[],
        }
    }
    fn supports_variant(&self, variant_tag: &str) -> bool {
        variant_tag != "legacy"
    }
    fn launch_env(&self, config: crate::RelayConfig<'_>, key: &str) -> Vec<(String, String)> {
        vec![
            ("KIMI_MODEL_NAME".into(), config.model.trim().into()),
            ("KIMI_MODEL_API_KEY".into(), key.into()),
            (
                "KIMI_MODEL_BASE_URL".into(),
                config.base_url.trim().trim_end_matches('/').into(),
            ),
            ("KIMI_MODEL_PROVIDER_TYPE".into(), config.protocol.into()),
        ]
    }
    fn augment_argv(
        &self,
        _config: crate::RelayConfig<'_>,
        _has_secret: bool,
        argv: Vec<String>,
    ) -> Vec<String> {
        argv
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

impl AgentPlugin for Kimi {
    fn screen_rules(&self) -> &'static [crate::screen::ScreenRule] {
        screen::RULES
    }

    /// 审批面板（ApprovalPanel；形态与按键语义为官方源码取证，见
    /// `apps/kimi-code/src/tui/components/dialogs/approval-panel.ts @ 0.29）。
    ///
    /// kimi 的 `PermissionRequest` hook 是 observation-only（`permission_hook_decides`
    /// 为 false），broker 审批桥对它关闭——屏幕识别是 GUI 给出**可操作**审批的唯一通道。
    /// 按键提示行是面板的稳定签名；提示行被裁掉时退回标题行。
    fn attention_patterns(&self) -> &'static [crate::chat_ui::AttentionPattern] {
        &[crate::chat_ui::AttentionPattern {
            id: "kimi:command-approval",
            patterns: &[
                r"↑/↓ select · [\d/]+ choose · ↵ confirm",
                r"^\s*▶\s*(?:run this command|write this file|apply these edits|stop this task|ready to build with this plan|approve [^\n?]+)\?",
            ],
            // 必须取最后一次：kimi 重绘也不清屏，上一条命令的面板残留在缓冲上方。
            // 取首个匹配会把标题/命令锁在残影上，而数字键打给的是屏幕最下方的活动
            // 面板——用户看着 A 批准了 B。
            last: true,
            details: crate::chat_ui::AttentionDetails::ArrowPanel,
        }]
    }

    fn id(&self) -> AgentId {
        id::KIMI
    }
    fn display_name(&self) -> &'static str {
        "Kimi Code"
    }
    fn variants(&self) -> &'static [Variant] {
        &VARIANTS
    }
    fn process_names(&self) -> &'static [&'static str] {
        &["kimi", "kimi.exe"]
    }
    fn proxy(&self) -> Option<&'static crate::proxy::ProxySpec> {
        Some(&PROXY)
    }
    fn relay(&self) -> Option<&'static dyn crate::RelayCap> {
        Some(&RELAY)
    }
    fn resume_args(&self) -> &'static [&'static str] {
        &["-r"]
    }
    /// 启动 flag 取自 `kimi --help`（kimi-code 0.2x 实测）：`--auto` / `-y, --yolo` 是权限档，
    /// `--plan` 是计划模式。维度 id 与 [`Self::mode_controls`] 对齐，前端两处共用同一套文案。
    ///
    /// **不声明 model**：`-m, --model <alias>` 确实存在，但别名来自用户的 `config.toml`
    /// （`kimi provider` 可增删），不是产品固定值。本表只放插件能担保的字面量——
    /// 硬编码一份别名清单，用户改了 provider 后就会静默传一个不存在的模型。
    fn launch_options(&self) -> &'static [crate::LaunchOption] {
        use crate::{LaunchChoice, LaunchOption};
        static OPTIONS: [LaunchOption; 2] = [
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
                        id: "auto",
                        label: "Auto",
                        args: &["--auto"],
                        risk: false,
                    },
                    LaunchChoice {
                        id: "yolo",
                        label: "Yolo",
                        args: &["--yolo"],
                        risk: true,
                    },
                ],
            },
            LaunchOption {
                id: "work",
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
                        args: &["--plan"],
                        risk: false,
                    },
                ],
            },
        ];
        // 同 mode_controls：只给已验证的 modern Kimi Code；旧 Python kimi-cli 的 flag 不同。
        if crate::installation(crate::id::KIMI)
            .is_some_and(|installation| installation.variant_tag == "modern")
        {
            &OPTIONS
        } else {
            &[]
        }
    }
    /// kimi 的待办工具叫 `TodoList`，参数是整份快照 `{todos:[{title,status}]}`。
    fn todo_snapshot_tools(&self) -> &'static [&'static str] {
        &["TodoList"]
    }
    /// `/model` 是交互式菜单（不接受内联参数，故无 model_presets）。声明它，GUI 就能
    /// 发出这条命令再把弹出的菜单渲染成按钮——模型清单由 CLI 现给。
    fn model_menu_command(&self) -> Option<&'static str> {
        Some("/model")
    }
    /// Esc 中断当前回合。**用户在真实 app 里确认可用**(2026-07:运行中的 kimi 会话按
    /// 「打断并发送」→ Esc 确实停下当前回合)。自动探针抓不到证据(kimi.exe 的 launcher
    /// shim 在 ConPTY 下即退),故走真机人工确认——与 claude(官方文档)/codex(状态行)
    /// 同为确证级别。
    fn interrupt_input(&self) -> Option<&'static str> {
        Some("\x1b")
    }
    /// `@` 是 kimi 自己的一等文件引用语法(官方文档;TUI 输 `@` 有补全)。它是**文本级**
    /// 惯例——wire.jsonl 实测 turn.prompt 原样保留 @路径文本、由模型自行读取,TUI 补全
    /// 同样只是插入路径文本。故注入 `@路径` 与用户手敲等价,即 kimi 的原生形态。
    /// 版本探测不到时兜底 false。
    fn attachment_mention(&self, version: Option<&str>) -> bool {
        version.is_some()
    }
    /// kimi TUI 的原生图片附加：触发键读系统剪贴板并把图片缓存为 blob 原生附加（composer
    /// 显示 `[image #N (W×H)]` 占位；旧版是 `[image:…]`，marker 正则两种形态都覆盖；
    /// wire.jsonl 实测入 prompt 为 image_url blobref）。触发键见 `clipboard_paste_input`。
    /// 版本探测不到时**仍声明**:能力已在真机实测,探测失败只是拿不到版本号,不代表
    /// 能力缺失(同 attachment_mention 的取值理由,但方向相反——那里探测不到时宁可
    /// 不用 `@`,因为文本通道恒可用;这里探测不到就**没有**声明的话,前端会静默退回
    /// 文本通道,把注入指令原文整段打进 TUI,实拍踩过)。剪贴板通道走不通时前端原有
    /// 的 sendWithClipboardImages → sendText 回退仍在,风险有底。
    fn clipboard_image_paste(&self, _version: Option<&str>) -> Option<&'static str> {
        Some(r"\[image[:# ]")
    }
    /// kimi 在 Windows 上的贴图键是 **Alt+V**，不是 Ctrl+V：0.29 与上一版（.bak）二进制
    /// 里都是 `matchesKey(..., win32 ? "alt+v" : ctrl("v"))`；真机探针
    /// （app/src-tauri/tests/probe_clipboard_image.rs）实测 `\x16` 注入 composer 零反应，
    /// `\x1bv`（传统 Meta 编码）~40ms 落出 `[image #1 (120×48)]`。kitty CSI-u 编码的
    /// Alt+V（`ESC[118;3u`）反而不被认——kimi 虽推 kitty 协议，这个键的匹配走的是
    /// 传统 ESC 前缀形态。版本门不设：两版二进制行为一致，探测不到版本时按现状声明。
    fn clipboard_paste_input(&self, _version: Option<&str>) -> Option<&'static str> {
        Some("\x1bv")
    }
    /// kimi 的 `/model` 同样是交互式菜单（二进制里的命令描述就是 `/model: switch model`，
    /// 且失败提示是「Run /model to select one first」），不接受内联参数，故不声明模型预设。
    fn slash_commands(&self) -> &'static [&'static str] {
        &["/clear", "/compact", "/help", "/model", "/status"]
    }
    fn mode_controls(&self) -> &'static [crate::ModeControl] {
        use crate::{ModeControl, ModeInput, ModeOption};
        static PLAN_ON: [ModeInput; 1] = [ModeInput {
            data: "/plan on",
            submit: true,
        }];
        static PLAN_OFF: [ModeInput; 1] = [ModeInput {
            data: "/plan off",
            submit: true,
        }];
        static MANUAL: [ModeInput; 2] = [
            ModeInput {
                data: "/yolo off",
                submit: true,
            },
            ModeInput {
                data: "/auto off",
                submit: true,
            },
        ];
        static YOLO: [ModeInput; 1] = [ModeInput {
            data: "/yolo on",
            submit: true,
        }];
        static AUTO: [ModeInput; 1] = [ModeInput {
            data: "/auto on",
            submit: true,
        }];
        static WORK: [ModeOption; 2] = [
            ModeOption {
                value: "default",
                inputs: &PLAN_OFF,
            },
            ModeOption {
                value: "plan",
                inputs: &PLAN_ON,
            },
        ];
        static PERMISSION: [ModeOption; 3] = [
            ModeOption {
                value: "manual",
                inputs: &MANUAL,
            },
            ModeOption {
                value: "yolo",
                inputs: &YOLO,
            },
            ModeOption {
                value: "auto",
                inputs: &AUTO,
            },
        ];
        static MODES: [ModeControl; 2] = [
            ModeControl {
                dimension: "work",
                cycle_input: Some("\x1b[Z"),
                options: &WORK,
                // kimi 的模式切换有 wire 事件即时落 transcript（telemetry 的 set_mode），
                // 不依赖屏幕回显。
                screen_markers: &[],
            },
            ModeControl {
                dimension: "permission",
                cycle_input: None,
                options: &PERMISSION,
                screen_markers: &[],
            },
        ];
        // 旧 Python 版 kimi-cli 的命令和 wire 事件不同；只给已验证的 modern Kimi Code 声明。
        if crate::installation(crate::id::KIMI)
            .is_some_and(|installation| installation.variant_tag == "modern")
        {
            &MODES
        } else {
            &[]
        }
    }
    /// 装当前 Node 版 Kimi Code（装到 `~/.kimi-code/bin/kimi.exe`，与 modern 变体的候选一致）。
    /// 注意路径里的 `/kimi-code/`——不带它的 `code.kimi.com/install.ps1` 装的是旧 Python `kimi-cli`
    /// （落到 `~/.local/bin/kimi-cli.exe`，检测不到）。
    ///
    /// **不直下**，也**不换入口**，理由都已实测：
    ///
    /// - `code.kimi.com` 是 nginx 直服（`server: nginx`，无 `cf-ray`），压根不在 Cloudflare 后面，
    ///   不会被人机校验拦。判定仍照做——中间设备也可能塞一张 HTML。
    /// - 它的引导脚本有 417 行（claude 的只有 110 行）。除了「取 latest → 读 manifest 的 checksum →
    ///   下载 → 校验」这段与 claude 同构之外，它还要迁移旧 `kimi-cli` 安装（重命名成
    ///   `kimi-legacy.exe`）、备份**正在运行**中被占用的 `kimi.exe`、写用户 PATH。把这些重新实现
    ///   一遍就是在复刻 kimi 的安装语义，上游一改我们就悄悄装坏。
    ///
    /// 对比 claude：它的脚本是段三步胶水，真正的安装由 `claude.exe install` 自己完成，
    /// 所以那边直下是干净的（见 `plugins/claude/install.rs`）。
    fn install_script(&self, windows: bool) -> Option<crate::install::InstallScript> {
        Some(crate::install::InstallScript::Fetch {
            url: if windows {
                "https://code.kimi.com/kimi-code/install.ps1"
            } else {
                "https://code.kimi.com/kimi-code/install.sh"
            },
            unix_shell: "bash",
        })
    }
    /// Kimi Code 0.23 会把当前任务标题写进终端标签。仍保留 reporter token：较早版本不写标题，
    /// 新版也可能在 hook 后才覆盖 token；app 会始终优先匹配 token，再匹配任务标题。
    fn sets_terminal_tab_title(&self) -> bool {
        true
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
    fn profile(&self) -> Option<&'static crate::profile::ProfileSpec> {
        Some(&PROFILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variant::Installation;
    use std::path::Path;

    /// 直接对变体表 probe（不碰真实 home，也不读 env）。
    fn probe_at(home: &Path) -> Option<Installation> {
        VARIANTS.iter().find_map(|v| {
            let dir = v
                .data_dir
                .candidates
                .iter()
                .map(|c| home.join(c))
                .find(|p| p.is_dir())?;
            Some(v.installation_at(id::KIMI, dir, Some(home)))
        })
    }

    #[test]
    fn events_all_in_upstream_whitelist() {
        // 防连坐绊线：一条非法 event 会让 kimi 静默禁用全部 hooks。
        for ev in EVENTS {
            assert!(
                EVENT_WHITELIST.contains(&ev.name),
                "{} 不在 kimi 0.20 事件白名单",
                ev.name
            );
        }
    }

    #[test]
    fn terminal_title_capabilities_cover_old_and_new_kimi() {
        let plugin = Kimi;
        assert!(plugin.sets_terminal_tab_title());
        assert!(plugin.writes_tab_token());
    }

    #[test]
    fn clipboard_image_paste_declared_even_without_version() {
        // 版本探测失败(拿不到版本号)不等于能力缺失:剪贴板发图已真机实测,探测不到
        // 就不声明会让前端静默退回文本通道、把注入指令原文打进 TUI(实拍)。无论版本
        // 是否探到,marker 声明必须一致。
        let plugin = Kimi;
        assert_eq!(plugin.clipboard_image_paste(None), Some(r"\[image[:# ]"));
        assert_eq!(plugin.clipboard_image_paste(Some("1.0")), Some(r"\[image[:# ]"));
    }

    #[test]
    fn clipboard_paste_input_is_alt_v_on_windows() {
        // 回归钉:kimi(win32)的贴图键是 Alt+V(`\x1bv`),不是通用的 Ctrl-V。发错键
        // composer 零反应,占位符超时后前端静默回退指令文本(用户实拍故障)。
        let plugin = Kimi;
        assert_eq!(plugin.clipboard_paste_input(None), Some("\x1bv"));
        assert_eq!(plugin.clipboard_paste_input(Some("0.29.0")), Some("\x1bv"));
    }

    #[test]
    fn prefers_modern_then_legacy() {
        let home = std::env::temp_dir().join(format!("meowo-kimi-variants-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);

        // 都不存在 → 未配置过。
        std::fs::create_dir_all(&home).unwrap();
        assert!(probe_at(&home).is_none());

        // 只有旧版 → legacy 命中，配置路径落在 ~/.kimi/config.toml。
        std::fs::create_dir_all(home.join(".kimi")).unwrap();
        let legacy = probe_at(&home).expect("legacy 应命中");
        assert_eq!(legacy.variant_tag, "legacy");
        assert_eq!(legacy.config_path(), home.join(".kimi").join("config.toml"));
        assert_eq!(
            legacy.credentials_path(),
            Some(
                home.join(".kimi")
                    .join("credentials")
                    .join("kimi-code.json")
            )
        );

        // 新版出现 → 抢先。
        std::fs::create_dir_all(home.join(".kimi-code")).unwrap();
        let modern = probe_at(&home).expect("modern 应命中");
        assert_eq!(modern.variant_tag, "modern");
        assert_eq!(modern.data_dir, home.join(".kimi-code"));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn launch_falls_back_to_bare_name_when_not_found() {
        let home = std::env::temp_dir().join(format!("meowo-kimi-exe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".kimi")).unwrap();

        let inst = probe_at(&home).unwrap();
        assert!(!inst.is_launchable());
        assert_eq!(inst.launch_argv(), vec!["kimi".to_string()]); // 回退裸名走 PATH

        // 旧版真实落点：~/.local/bin/kimi[.exe]
        let local_bin = home.join(".local").join("bin");
        std::fs::create_dir_all(&local_bin).unwrap();
        let exe = local_bin.join(crate::exe_file_name("kimi"));
        std::fs::write(&exe, b"").unwrap();
        let inst = probe_at(&home).unwrap();
        assert!(inst.is_launchable());
        assert_eq!(inst.launch_argv(), vec![exe.to_string_lossy().into_owned()]);

        let _ = std::fs::remove_dir_all(&home);
    }
}
