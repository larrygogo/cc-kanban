//! Agent 插件层：一个 agent = 一个自包含模块，声明自己的**变体表**（同一 agent 的多个版本形态），
//! 由注册表统一驱动。承载「身份 + 变体探测 + 路径解析 + hooks 配置格式 + 鉴权参数」这层纯逻辑
//! ——它跨 `meowo-reporter`（hook 子进程）与 `meowo-app`（GUI）两个二进制共用，此前散在两边，
//! kimi 新旧版差异只能靠各处临时 `if`/fallback 硬凑。
//!
//! 本 crate 不联网、不依赖 Tauri、不碰数据库。文件写入仅限两处自包含的本地落盘：
//! [`fsutil`] 的原子写（hooks 配置 / 凭据 / 缓存回写共用）与 claude transcript 里排队插话
//! 图片的临时目录落盘（`fsutil::paste_root`）；其余落盘与全部联网由 meowo-app 完成，
//! 但都以本层探测出的 [`Installation`] 为输入，不再各自重推路径。
//!
//! 迁移状态（详见 `docs/architecture/agent-plugin.md`）：
//!
//! - Phase 1 ✅ 身份收敛：`AgentId` 是全项目唯一的 agent 身份类型，`meowo_store::ProviderKey`
//!   枚举已删除，DB 的 provider 列退化为原样字符串。解析走 [`resolve`]，未知 id 不再冒名默认 agent。
//! - Phase 2 ✅ 注册表合一：`meowo-reporter` 的 `Agent` trait 与那张并行注册表已折进本 crate 的
//!   [`AgentPlugin`]——进程名、resume/启动 argv、安装脚本、标签页行为是声明式方法，会话遥测走
//!   [`caps::TelemetryCap`] 能力槽。transcript 解析亦随之迁入（[`transcript`] + `plugins/claude/`）。
//! - Phase 3 ✅ 端口注入：[`ports::HttpPort`] / [`ports::KeychainPort`] 由宿主注入，于是账号
//!   （[`account::AccountCap`]）与接线副作用（[`wiring::WiringCap`]）也住进了 `plugins/<id>/`。
//!   本 crate 不依赖 HTTP 栈，插件层没有一行 `#[cfg(target_os)]`。
//! - Phase 4 ✅ 前端描述符：`list_agents()` 下发 id/展示名/安装态，前端不再自带 agent 名单。
//!   图标与品牌色留在前端资产表——位图 logo 与主题相关的颜色是资产，不是能塞进字段的数据。
//!
//! 终局验收：加一个 agent 只需新增 `plugins/<new>/` 与 `registry.rs` 一行（后端零其它改动）；
//! 前端只需在资产表补一个图标，不补也只是显示中性徽标 + id，不会崩、不会冒名成 claude。

pub mod account;
pub mod auth;
pub mod caps;
pub mod chat_ui;
pub mod codec;
pub mod config;
pub mod descriptor;
pub mod fsutil;
pub mod handoff;
pub mod id;
pub mod install;
pub mod launch;
pub mod launch_options;
pub mod plugins;
pub mod ports;
pub mod profile;
pub mod proxy;
pub mod registry;
pub mod relay;
pub mod screen;
pub mod transcript;
pub mod variant;
pub mod wiring;

pub use account::{
    Account, AccountCap, ApiKeyLoginCap, ProviderUsage, UsageKind, UsageLane, USAGE_UNSUPPORTED,
};
pub use auth::{AuthScheme, CredentialSource, OAuthRefresh};
pub use caps::{
    BackgroundControl, BackgroundEndpoint, ContextUsage, HookContext, RuntimeCap, SessionRuntime, StopOutputs,
    TelemetryCap,
};
pub use chat_ui::{
    ChatUi, ChatUiContext, CustomCommandSpec, ModeControl, ModeInput, ModeOption, ModeScreenMarker,
    SlashCommand, SlashSource,
};
pub use config::{
    CommandSpec, ConfigFormat, EnsureOutcome, HookEvent, HookSpec, MissingConfig, RepairReason,
};
pub use handoff::{render_history, HandoffMeta};
pub use id::AgentId;
pub use install::{
    is_runnable_script, looks_like_challenge, looks_like_html, InstallCap, InstallPlan,
    InstallScript,
};
pub use launch::{exe_on_path, LaunchCandidate, LaunchSpec, Root};
pub use launch_options::{resolve_launch_args, LaunchChoice, LaunchOption};
pub use plugins::claude::setup::remove_generated_wrapper;
pub use ports::{Body, HttpError, HttpPort, HttpRequest, KeychainPort, NoKeychain, Ports};
pub use profile::ProfileSpec;
pub use proxy::{is_socks, ProxySpec};
pub use registry::{
    all, by_id, installation, is_agent_process, resolve, AgentPlugin, ModelPreset, DEFAULT_ID,
};
pub use relay::{
    RelayCap, RelayConfig, RelayEnvOption, RelayModelAuth, RelayModelRequest, RelayOption,
    RelaySuggestionGroup, RelayUi,
};
pub use transcript::{
    default_resolve_cwd, read_chat_delta, AgentMode, ChatDelta, ChatItem, TranscriptCache,
    TranscriptEvent, TranscriptInfo, TranscriptParser, TranscriptSpec, TurnError,
};
pub use variant::{DataDirSpec, Installation, Variant};
pub use wiring::{backup_once, wire_hooks, WiringCap, WiringContext};

use std::path::{Path, PathBuf};

/// 测试专用：环境变量互斥锁。
///
/// 进程级环境变量（`USERPROFILE`/`HOME`）是**全局**的，而 Rust 测试默认并发跑。改 env 的测试
/// （如 transcript 的全局搜索用例，会把 `USERPROFILE` 临时指向空的临时目录）与依赖真实 env
/// 解析安装路径的测试（如 registry 的 launch/resume argv 用例）必须互斥——否则前者开的那个窗口
/// 里，后者解析不到 `~/.local/bin/claude.exe`、落到 PATH 兜底的裸名 `claude`，测试随机变红。
///
/// 锁被毒化（持锁的测试 panic 了）时取回内部值继续：一个测试失败不该把其余测试全带崩。
#[cfg(test)]
pub(crate) fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 用户 home（Windows `USERPROFILE` 优先，回退 `HOME`）。所有目录解析的根。
///
/// 空串视为未设置（与 `variant::DataDirSpec::env_override` 同一防护）：显式置空的
/// `USERPROFILE` 若被采纳，`home.join(".claude")` 会拼出相对路径，随 cwd 漂移。
pub fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("HOME").ok().filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// meowo 数据根：`MEOWO_DB` 的父目录；未设置（或父目录为空，如 `MEOWO_DB=board.db`
/// 这种裸文件名——空父目录一 join 就成相对路径，随 cwd 漂移）时回退 `~/.meowo`。
/// 与宿主 `meowo-app` 的 `db_path()` 同语义；跨 crate 收敛（经 wiring 注入）是后续项。
pub(crate) fn meowo_data_root() -> Option<PathBuf> {
    let home = home_dir()?;
    Some(
        std::env::var_os("MEOWO_DB")
            .map(PathBuf::from)
            .and_then(|path| {
                path.parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .map(Path::to_path_buf)
            })
            .unwrap_or_else(|| home.join(".meowo")),
    )
}

/// Meowo 管理的某 agent 的全部 profile 私有根（`<数据根>/profiles/<agent>/<id>`）。
/// claude 的 transcript 查找与 kimi 的会话索引查找共用这一份枚举——多账号数据
/// 「扫哪些目录」是同一个问题，规则只写一处。
pub(crate) fn managed_profile_dirs(agent: &str) -> Vec<PathBuf> {
    let Some(root) = meowo_data_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root.join("profiles").join(agent)) else {
        return Vec::new();
    };
    entries
        .flatten()
        // 用 path().is_dir() 而非 DirEntry::file_type()：后者不解引用 symlink/junction，
        // 被 junction 挪到别的盘的 profile 会被静默跳过（其下会话从此定位不到）。
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect()
}

/// 按 `/` 分段拼接相对路径。直接 `join("a/b")` 在 Windows 上会拼出混合分隔符（`dir\a/b`），
/// 虽多数 API 容忍，但比较/展示都会走样，故统一分段。
pub(crate) fn join_rel(base: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .filter(|s| !s.is_empty())
        .fold(base.to_path_buf(), |p, s| p.join(s))
}

/// 可执行文件名：Windows 补 `.exe`。
pub(crate) fn exe_file_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_rel_splits_segments() {
        let p = join_rel(Path::new("/base"), "credentials/kimi-code.json");
        assert_eq!(
            p,
            Path::new("/base")
                .join("credentials")
                .join("kimi-code.json")
        );
    }

    /// 环境变量被显式置成空串 = 未设置：必须回退/判 None，而不是采纳空串拼出相对路径。
    /// 改进程级 env，须持 [`crate::env_guard`] 与其它 env 测试互斥，并在结束时还原现场。
    #[test]
    fn home_dir_treats_empty_env_vars_as_unset() {
        let _env = crate::env_guard();
        let saved_profile = std::env::var("USERPROFILE").ok();
        let saved_home = std::env::var("HOME").ok();

        // USERPROFILE 置空串 → 回退 HOME（此前会返回 Some("")，join 出随 cwd 漂移的相对路径）。
        std::env::set_var("USERPROFILE", "");
        std::env::set_var("HOME", "/home/fallback");
        assert_eq!(home_dir(), Some(PathBuf::from("/home/fallback")));

        // 两者都空 → None。
        std::env::set_var("HOME", "");
        assert_eq!(home_dir(), None);

        // 常规优先级不受影响：USERPROFILE 非空时优先。
        std::env::set_var("USERPROFILE", "C:/Users/me");
        std::env::set_var("HOME", "/home/fallback");
        assert_eq!(home_dir(), Some(PathBuf::from("C:/Users/me")));

        // 还原现场：env 是进程全局的，置空状态不能留给后续测试。
        match saved_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
