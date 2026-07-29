//! 应用设置：持久化、默认值、i18n 文案，以及设置页相关命令。

use crate::{apply_language, db_path};
use std::path::PathBuf;
use tauri::Emitter;
use tauri_plugin_autostart::ManagerExt;

fn default_true() -> bool {
    true
}
/// 外观默认值（与前端 appearance.ts / styles.css 的初值保持一致）。
fn default_theme() -> String {
    "dark".to_string()
}
fn default_opacity() -> u32 {
    100
}
fn default_ui_scale() -> u32 {
    100
}
fn default_resume_terminal() -> String {
    "terminal".to_string()
}
fn default_language() -> String {
    "auto".to_string()
}
/// 打开终端的方式：card = 点击卡片直接打开（默认），button = 卡片上单独的打开按钮。
fn default_terminal_open_mode() -> String {
    "card".to_string()
}
/// 打开会话时把用户带到哪个**视图**：terminal = 外部终端（默认），chat = Meowo 对话窗口。
///
/// 只是视图之争，不是「谁持有 agent」之争：两种取值下会话都由 Meowo 的 PTY 持有，
/// terminal 只是改为用 attach 客户端把同一个 PTY 镜像到用户选的外部终端里。
///
/// 缺省 terminal：点卡片去终端是终端托管之前就有的习惯，老用户升级不该被静默改掉落点。
fn default_session_open_in() -> String {
    "terminal".to_string()
}
/// 卡片菜单（星标/便签/重命名/归档等）触发方式：button = 卡片上的常显菜单按钮（默认），
/// context = 右键菜单，两者二选一。
fn default_card_menu_mode() -> String {
    "button".to_string()
}
/// 贴纸风格：flat = 扁平（默认），elevated = 立体感。
fn default_sticker_style() -> String {
    "flat".to_string()
}
/// 贴纸底色预设 key（neutral = 无色，默认）。
fn default_sticker_color() -> String {
    "neutral".to_string()
}
/// 在贴纸底栏显示配额的 provider key 列表，默认仅 claude。
fn default_sticker_quota_providers() -> Vec<String> {
    vec!["claude".to_string()]
}
/// 新建会话默认选中的 agent（provider key）。缺省 claude。
fn default_default_agent() -> String {
    "claude".to_string()
}

/// 应用设置（持久化到 ~/.meowo/settings.json）。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Settings {
    /// 归档条目自动隐藏的天数；0 = 永不隐藏。
    #[serde(default)]
    pub(crate) archive_hide_days: u32,
    /// 桌面通知总开关（待交互 + 错误）。缺省为开启，兼容老 settings.json。
    #[serde(default = "default_true")]
    pub(crate) notifications_enabled: bool,
    /// 会话需要关注(出错/待审批/待交互)且 Meowo 窗口都不在前台时,请求任务栏注意力
    /// (Windows 任务栏闪烁)。与 toast 独立开关:通知会消失,任务栏高亮驻留到用户点开。
    /// 缺省开启,兼容老 settings.json。
    #[serde(default = "default_true")]
    pub(crate) attention_flash_enabled: bool,
    /// 自动检查并下载软件更新。缺省开启，兼容老 settings.json。
    #[serde(default = "default_true")]
    pub(crate) auto_update_enabled: bool,
    /// 外观模式：dark / light / system（跟随系统）。缺省 dark，兼容老 settings.json。
    #[serde(default = "default_theme")]
    pub(crate) theme: String,
    /// 贴纸背景不透明度（百分比 25–100）。缺省 100（完全不透明）。
    #[serde(default = "default_opacity")]
    pub(crate) opacity: u32,
    /// 界面密度/字号缩放（百分比，紧凑 90 / 标准 100 / 宽松 112）。
    #[serde(default = "default_ui_scale")]
    pub(crate) ui_scale: u32,
    /// 打开未连接会话用的终端（macOS）：terminal = Terminal.app，iterm = iTerm2，ghostty = Ghostty。缺省 terminal，兼容老 settings.json。
    #[serde(default = "default_resume_terminal")]
    pub(crate) resume_terminal: String,
    /// 界面/通知语言：auto（跟随系统）/ zh / en。缺省 auto，兼容老 settings.json。
    #[serde(default = "default_language")]
    pub(crate) language: String,
    /// 打开终端方式：card = 点击卡片（默认），button = 卡片单独打开按钮。兼容老 settings.json。
    #[serde(default = "default_terminal_open_mode")]
    pub(crate) terminal_open_mode: String,
    /// 打开会话落到哪个视图：terminal = 外部终端（默认），chat = Meowo 对话窗口。
    /// 兼容老 settings.json（缺席 → terminal）。
    #[serde(default = "default_session_open_in")]
    pub(crate) session_open_in: String,
    /// 卡片菜单触发方式：button = 卡片菜单按钮（默认），context = 右键菜单。兼容老 settings.json。
    #[serde(default = "default_card_menu_mode")]
    pub(crate) card_menu_mode: String,
    /// 是否显示卡片 hover「轻推」预览（最近一条 AI 正文）。缺省开启，兼容老 settings.json。
    #[serde(default = "default_true")]
    pub(crate) preview_enabled: bool,
    /// 贴纸风格：flat = 扁平（默认），elevated = 立体感。缺省 flat，兼容老 settings.json。
    #[serde(default = "default_sticker_style")]
    pub(crate) sticker_style: String,
    /// 贴纸底色预设 key（neutral/classic/slate/moss/plum/rose/amber）。缺省 neutral，兼容老 settings.json。
    #[serde(default = "default_sticker_color")]
    pub(crate) sticker_color: String,
    /// 在贴纸底栏显示配额的 provider key 列表（如 "claude"/"kimi"/"codex"）。
    /// 缺省 ["claude"]，旧 settings.json 无此字段时反序列化给默认，不 panic。
    #[serde(default = "default_sticker_quota_providers")]
    pub(crate) sticker_quota_providers: Vec<String>,
    /// 「新建会话」面板默认选中的 agent（claude/kimi/codex）。缺省 claude，兼容老 settings.json。
    #[serde(default = "default_default_agent")]
    pub(crate) default_agent: String,
    /// 出站代理：用量查询 / OAuth 刷新 / 下载 agent 二进制 / 自更新。
    /// 可按 agent 覆盖（`api.anthropic.com` 走代理、Kimi 直连是常态）。见 [`crate::proxy`]。
    #[serde(default)]
    pub(crate) proxy: crate::proxy::ProxySettings,
    /// 多账号：每个 agent 的**自定义** profile 列表（键＝agent id）。
    ///
    /// **默认账号不在里面**——它是隐式的，指向 agent 自己的目录（`~/.claude`），且不注入任何
    /// 环境变量。于是不建 profile 的用户零感知：这两个字段为空时，一切与从前一模一样。
    #[serde(default)]
    pub(crate) profiles: std::collections::BTreeMap<String, Vec<crate::profile::Profile>>,
    /// 每个 agent 当前**活跃**的 profile id。键缺席 = 用默认账号。
    #[serde(default)]
    pub(crate) active_profile: std::collections::BTreeMap<String, String>,
    /// 用户给**默认账号**起的名字（键＝agent id）。缺席 → 前端显示本地化的「默认账号」。
    ///
    /// 默认账号本身是隐式的（不在 `profiles` 里），但「不能改名」纯粹是当初的疏漏，不是设计：
    /// 名字只是个显示串，不碰任何文件。而两个账号里有一个永远叫「默认账号」，用起来很别扭。
    #[serde(default)]
    pub(crate) default_profile_names: std::collections::BTreeMap<String, String>,
    /// API 中转元数据。密钥单独存储，绝不随 Settings 序列化或事件下发。
    #[serde(default)]
    pub(crate) relay: crate::relay::RelaySettings,
    /// 使用引导是否已看过。缺省 false —— 新装用户及老用户升级后首次启动各自动弹一次引导窗口，
    /// 看完（或点关闭）即置 true，之后只能从托盘/设置手动再看。
    #[serde(default)]
    pub(crate) onboarding_seen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            archive_hide_days: 0,
            notifications_enabled: true,
            attention_flash_enabled: true,
            auto_update_enabled: true,
            theme: default_theme(),
            opacity: default_opacity(),
            ui_scale: default_ui_scale(),
            resume_terminal: default_resume_terminal(),
            language: default_language(),
            terminal_open_mode: default_terminal_open_mode(),
            session_open_in: default_session_open_in(),
            card_menu_mode: default_card_menu_mode(),
            preview_enabled: true,
            sticker_style: default_sticker_style(),
            sticker_color: default_sticker_color(),
            sticker_quota_providers: default_sticker_quota_providers(),
            default_agent: default_default_agent(),
            proxy: crate::proxy::ProxySettings::default(),
            // 空 = 只有默认账号（agent 自己的目录），不注入任何环境变量。
            profiles: Default::default(),
            active_profile: Default::default(),
            default_profile_names: Default::default(),
            relay: crate::relay::RelaySettings::default(),
            onboarding_seen: false,
        }
    }
}

/// 解析生效语言：settings.language 为 zh/en 用之；auto 按系统 locale（zh* → zh，其余 en）。
pub(crate) fn ui_lang(settings: &Settings) -> &'static str {
    match settings.language.as_str() {
        "zh" => "zh",
        "en" => "en",
        _ => {
            if sys_locale::get_locale()
                .map(|l| l.starts_with("zh"))
                .unwrap_or(false)
            {
                "zh"
            } else {
                "en"
            }
        }
    }
}

/// Rust 侧用户可见文案（仅通知/托盘/窗口标题数条，不引 i18n 库）。
pub(crate) fn tr(lang: &str, key: &str) -> &'static str {
    match (lang, key) {
        ("en", "notify.error") => "Session error",
        ("en", "notify.waiting") => "Waiting for your reply",
        ("en", "notify.pending.approval") => "Approve a tool call?",
        ("en", "notify.pending.question") => "A session is asking you a question",
        ("en", "notify.pending.plan") => "Plan awaiting approval",
        ("en", "tray.recall") => "Recall sticker",
        ("en", "tray.guide") => "Getting started",
        ("en", "tray.settings") => "Settings",
        ("en", "tray.website") => "Website",
        ("en", "tray.quit") => "Quit",
        ("en", "window.settings") => "Settings",
        ("en", "window.updater") => "Software Update",
        ("en", "window.newSession") => "New Session",
        ("en", "window.onboarding") => "Getting started",
        (_, "notify.error") => "会话出错",
        (_, "notify.waiting") => "等待你回复",
        (_, "notify.pending.approval") => "需要你批准工具调用",
        (_, "notify.pending.question") => "会话在问你问题",
        (_, "notify.pending.plan") => "计划待批准",
        (_, "tray.recall") => "找回贴纸",
        (_, "tray.guide") => "使用引导",
        (_, "tray.settings") => "设置",
        (_, "tray.website") => "官方网站",
        (_, "tray.quit") => "退出",
        (_, "window.settings") => "设置",
        (_, "window.updater") => "软件更新",
        (_, "window.newSession") => "新建会话",
        (_, "window.onboarding") => "使用引导",
        _ => "",
    }
}

fn settings_path() -> PathBuf {
    db_path().with_file_name("settings.json")
}

static SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn load_settings_unlocked() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn load_settings() -> Settings {
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_settings_unlocked()
}

/// 落盘 settings.json（原子写）。**只管存**——不校验代理、不重建托盘、不 emit 事件，那些是
/// [`set_settings`] 这条用户路径的事。profile 的增删/切换走它。
fn save_settings_unlocked(s: &Settings) -> Result<(), String> {
    let body = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // 原子写：后台轮询线程每 5s 裸读本文件，直写可能被读到半截而回退默认值。
    crate::fsutil::write_atomic(&path, &body).map_err(|e| e.to_string())
}

/// 在同一把锁内完成 settings 的读取、修改与落盘，防止并发命令互相覆盖整份文件。
pub(crate) fn update_settings<T>(
    update: impl FnOnce(&mut Settings) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut settings = load_settings_unlocked();
    let result = update(&mut settings)?;
    save_settings_unlocked(&settings)?;
    Ok(result)
}

// 本文件的 command 一律 async + spawn_blocking：同步命令跑在主线程，settings.json 虽小，
// 但杀软扫描/同步盘接管目录时任何一次读写都可能拖到秒级，冻住消息泵。
#[tauri::command]
pub(crate) async fn get_settings() -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(load_settings)
        .await
        .map_err(|e| e.to_string())
}

/// 引导窗口用：把「已看过引导」落盘。刻意不走 set_settings（那条会校验代理、写各 agent 配置、
/// 重建托盘、广播事件），引导只需翻一个布尔，用轻量的 update_settings 单改单存即可。
#[tauri::command]
pub(crate) async fn mark_onboarding_seen() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        update_settings(|s| {
            s.onboarding_seen = true;
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `set_settings` 落盘前的字段保护：profiles 三字段由独立账号命令维护、`onboarding_seen` 由
/// [`mark_onboarding_seen`] 单独落盘，而设置窗口回传的是打开时的整对象快照。以磁盘最新值
/// 回填这些字段——否则窗口打开期间创建/改名/切换的账号会被旧快照覆盖，刚完成的引导标记
/// 也会被写回 false（引导重复弹出）。纯函数便于单测。
fn preserve_independently_managed_fields(incoming: &mut Settings, current: &Settings) {
    incoming.profiles = current.profiles.clone();
    incoming.active_profile = current.active_profile.clone();
    incoming.default_profile_names = current.default_profile_names.clone();
    incoming.onboarding_seen = current.onboarding_seen;
}

#[tauri::command]
pub(crate) async fn set_settings(
    app: tauri::AppHandle,
    mut settings: Settings,
) -> Result<(), String> {
    // 后端兜底钳值（与前端 appearance.ts 一致），防越界值落盘后被 5s 轮询线程读到。
    settings.opacity = settings.opacity.clamp(25, 100);
    settings.ui_scale = settings.ui_scale.clamp(50, 200);
    // 代理地址落盘前校验。非法值一旦写进去，后台只会静默降级直连，用户对着「用量查不到」
    // 毫无线索——在这里拦下，把具体原因回给设置页。
    // 先清洗再校验：粘贴进来的地址常混入零宽字符（中转还有全角冒号的情况），肉眼看着
    // 完全正确却过不了校验；洗完仍不合法的才是真错误。
    settings.proxy.normalize();
    settings.proxy.validate()?;
    settings.relay.normalize();
    settings.relay.validate()?;
    // 落盘 + 写各 agent 配置都是文件 IO，且 apply_to_agent_configs 要排队等启动线程的同一把
    // 锁——同步命令会拿这些卡主线程消息泵，故整段挪进 blocking 池。
    let io_app = app.clone();
    let settings = tauri::async_runtime::spawn_blocking(move || -> Result<Settings, String> {
        // profiles 三个字段由独立账号命令维护，onboarding_seen 由 mark_onboarding_seen 单独落盘；
        // 设置窗口可能持有较旧的整对象快照，一次外观/网络保存不得把窗口打开期间的并发变更覆盖掉。
        update_settings(|current| {
            preserve_independently_managed_fields(&mut settings, current);
            *current = settings.clone();
            Ok(())
        })?;
        // 代理落盘后立刻写进各 agent 自己的配置（claude 的 settings.json env 块），改完即生效——
        // 否则用户改了代理还得重启 Meowo 才作数。best-effort：写不进去不影响 Meowo 自己的设置已保存。
        let reports = crate::proxy::apply_to_agent_configs();
        let _ = io_app.emit("proxy-applied", &reports);
        Ok(settings)
    })
    .await
    .map_err(|e| e.to_string())??;
    // 切语言后重建托盘菜单/窗口标题（无条件重建，菜单仅两项，幂等且廉价）。
    // 托盘/菜单对象有线程亲和（muda 在其创建线程即主线程上操作），不能在 blocking 池里碰。
    let lang = ui_lang(&settings);
    let menu_app = app.clone();
    app.run_on_main_thread(move || apply_language(&menu_app, lang))
        .map_err(|e| e.to_string())?;
    // 通知贴纸窗口实时套用新设置。
    let _ = app.emit("settings-changed", settings);
    Ok(())
}

/// 某 agent（`agent = None` → 全局规则）当前**生效**的代理串；`None` 表示直连。
///
/// 存在的理由只有一个：自更新走 `tauri-plugin-updater`（内部是 reqwest），**不经过 ports.rs 的
/// ureq 客户端**，拿不到我们解析出来的代理。前端更新窗口只能靠这个命令取值，再喂给
/// `check({ proxy })`。设置页也用它显示「system 模式下实际读到的环境变量代理是什么」。
///
/// 注意：解析结果可能是 `socks5://`，而 updater 的 reqwest 未必编进 socks 支持——前端据此提示。
#[tauri::command]
pub(crate) async fn get_effective_proxy(agent: Option<String>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::ports::resolve_proxy(agent.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

/// 设置窗口用：读取/切换开机自启（原来只在托盘，托盘精简后搬到设置页）。
#[tauri::command]
pub(crate) fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    // dev 下自启会注册 dev 二进制(开机连不上 dev server → 白屏)，一律视为关闭，避免误导。
    if tauri::is_dev() {
        return Ok(false);
    }
    Ok(app.autolaunch().is_enabled().unwrap_or(false))
}

#[tauri::command]
pub(crate) fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    // dev 下拒绝写入：否则会把 target/debug 的调试二进制注册进开机自启，开机白屏。仅安装版可用。
    if tauri::is_dev() {
        return Err(
            "开机自启仅在安装版可用（dev 下会注册调试二进制，开机连不上 dev server）".into(),
        );
    }
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| e.to_string())?;
        // auto-launch 写 Run 项用 format!("{} {}", path, args)——路径不加引号。路径含空格(如用户名
        // "First Last" → C:\Users\First Last\...)会被 Windows 拆成「程序+参数」，开机自启直接失败。
        // enable 成功后把该 Run 值重写为带引号的可执行路径修正（值名与插件一致 = package_info().name）。
        #[cfg(target_os = "windows")]
        quote_autostart_run_value(&app);
        Ok(())
    } else {
        mgr.disable().map_err(|e| e.to_string())
    }
}

/// 把 HKCU\...\Run 下本应用的自启项值重写为带引号的可执行路径，修正 auto-launch 不加引号、
/// 含空格路径开机自启失败的问题。失败不致命（仅日志），不影响开关状态。
#[cfg(target_os = "windows")]
fn quote_autostart_run_value(app: &tauri::AppHandle) {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[autostart] current_exe 失败，跳过路径加引号: {e}");
            return;
        }
    };
    let name = app.package_info().name.clone(); // 与 tauri-plugin-autostart 的 Run 项名一致
    let value = format!("\"{}\"", exe.display());
    let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_SET_VALUE,
    );
    match run {
        Ok(run) => {
            if let Err(e) = run.set_value(&name, &value) {
                eprintln!("[autostart] 重写带引号路径失败: {e}");
            }
        }
        Err(e) => eprintln!("[autostart] 打开 Run 注册表键失败: {e}"),
    }
}

pub(crate) const SITE_URL: &str = "https://meowo.io";

fn is_allowed_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host_str() {
        Some("meowo.io") => true,
        Some("github.com") => {
            let path = url.path().trim_end_matches('/');
            path == "/larrygogo/meowo" || path.starts_with("/larrygogo/meowo/")
        }
        _ => false,
    }
}

/// 在默认浏览器打开 `url`。Windows 用 explorer、macOS 用 open（均不经 shell）。
/// 只做「打开」这一件事——放行哪些链接由两个命令各自的校验负责。
fn spawn_browser(url: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    // macOS：open 偶发慢（默认浏览器冷启动），放后台线程不挡主线程。
    // status() 而非 spawn()：spawn 后不 wait，Unix 上 Child 被 drop 不会 reap，
    // 常驻托盘的本进程会积累 <defunct> 僵尸；已在后台线程，阻塞等待无害。
    #[cfg(target_os = "macos")]
    std::thread::spawn(move || {
        let _ = std::process::Command::new("open").arg(&url).status();
    });
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = url;
    Ok(())
}

/// 设置/关于页与托盘用：打开官网或本仓库链接。只放行白名单前缀——这些入口是**应用自己**
/// 发起的导航，用户没有机会审视目标，必须收紧到可信域，杜绝被滥用打开任意/恶意目标。
#[tauri::command]
pub(crate) fn open_url(url: String) -> Result<(), String> {
    if !is_allowed_url(&url) {
        return Err("不允许的链接".into());
    }
    spawn_browser(url)
}

/// 对话内容里的链接用：用户**主动点击**模型输出中的 URL，与浏览器/聊天工具的行为一致，
/// 不做域名白名单——但 scheme 必须是 http/https：explorer/open 对 file:、ms-settings: 等
/// scheme 同样来者不拒，任由 transcript 内容触发本地程序是注入通道，不是链接。
#[tauri::command]
pub(crate) fn open_link(url: String) -> Result<(), String> {
    let parsed = url::Url::parse(&url).map_err(|_| "无效链接")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("只支持 http/https 链接".into());
    }
    spawn_browser(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_defaults_to_claude() {
        assert_eq!(Settings::default().default_agent, "claude");
    }

    #[test]
    fn external_url_allowlist_checks_origin_and_repo_path() {
        assert!(is_allowed_url("https://meowo.io/docs"));
        assert!(is_allowed_url(
            "https://github.com/larrygogo/meowo/releases/latest"
        ));
        assert!(!is_allowed_url("https://meowo.io.evil.example/"));
        assert!(!is_allowed_url(
            "https://github.com/larrygogo/meowo-malware"
        ));
        assert!(!is_allowed_url("https://meowo.io@evil.example/"));
        assert!(!is_allowed_url("https://meowo.io:444/"));
        assert!(!is_allowed_url("http://meowo.io/"));
    }

    #[test]
    fn old_settings_json_without_default_agent_deserializes() {
        // 旧 settings.json 无 default_agent 字段：serde default 兜底 claude，不 panic。
        let v: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(v.default_agent, "claude");
    }

    #[test]
    fn open_link_rejects_non_http_schemes() {
        // 校验只看 scheme 是否放行，不真的打开浏览器——非 windows/macos 的 spawn_browser
        // 是 no-op，windows 上 explorer 对 Err 分支根本不会执行。
        assert!(open_link("javascript:alert(1)".into()).is_err());
        assert!(open_link("file:///C:/Windows/System32/calc.exe".into()).is_err());
        assert!(open_link("ms-settings:display".into()).is_err());
        assert!(open_link("not a url".into()).is_err());
    }

    #[test]
    fn old_settings_json_without_session_open_in_defaults_to_terminal() {
        // 老用户的 settings.json 没有这个字段：必须落到 terminal（= 托管之前的既有习惯），
        // 且不 panic。
        let v: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(v.session_open_in, "terminal");
        assert_eq!(Settings::default().session_open_in, "terminal");
    }

    #[test]
    fn session_open_in_round_trips_through_json() {
        let v: Settings = serde_json::from_str(r#"{"session_open_in":"chat"}"#).unwrap();
        assert_eq!(v.session_open_in, "chat");
        // 写回去的值必须原样保住，否则用户改了设置却看不出任何变化。
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""session_open_in":"chat""#));
    }

    #[test]
    fn auto_update_defaults_on_for_new_and_old_settings() {
        assert!(Settings::default().auto_update_enabled);
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(old.auto_update_enabled);
        let disabled: Settings = serde_json::from_str(r#"{"auto_update_enabled":false}"#).unwrap();
        assert!(!disabled.auto_update_enabled);
    }

    #[test]
    fn old_settings_json_without_proxy_defaults_to_system() {
        // 老 settings.json 完全没有 proxy 段 → 跟随系统环境变量（而非直连）。
        let v: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(v.proxy.mode, "system");
        assert!(v.proxy.per_agent.is_empty());
        assert!(v.proxy.validate().is_ok());
    }

    #[test]
    fn per_agent_proxy_roundtrips_through_settings_json() {
        // 设置页写入的形态能原样读回（含 per_agent 覆盖）。
        let src = r#"{"proxy":{"mode":"custom","url":"http://127.0.0.1:7890",
                     "per_agent":{"kimi":{"mode":"off","url":""}}}}"#;
        let v: Settings = serde_json::from_str(src).unwrap();
        assert_eq!(
            v.proxy.resolve(Some("claude")).as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(v.proxy.resolve(Some("kimi")), None);
        let text = serde_json::to_string(&v).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.proxy, v.proxy);
    }

    #[test]
    fn old_settings_without_relay_defaults_off_and_contains_no_secret_field() {
        let v: Settings = serde_json::from_str("{}").unwrap();
        assert!(v.relay.per_agent.is_empty());
        let text = serde_json::to_string(&v).unwrap();
        assert!(!text.contains("api_key"));
        assert!(!text.contains("secret"));
    }

    /// 设置窗口回传的是打开时的整对象快照：由独立命令维护的字段必须以磁盘最新值为准，
    /// 尤其 onboarding_seen——窗口打开期间刚完成引导，一次外观保存不得把它写回 false。
    #[test]
    fn set_settings_preserves_independently_managed_fields() {
        let mut incoming = Settings {
            opacity: 60, // 这次保存真正想改的字段
            ..Default::default()
        };
        let mut current = Settings {
            onboarding_seen: true, // 窗口打开期间完成了引导
            ..Default::default()
        };
        current.profiles.insert("claude".into(), vec![]); // 期间建过账号
        current
            .active_profile
            .insert("claude".into(), "work".into()); // 期间切了账号
        current
            .default_profile_names
            .insert("claude".into(), "公司号".into());

        preserve_independently_managed_fields(&mut incoming, &current);

        assert!(incoming.onboarding_seen, "引导标记不得被旧快照写回 false");
        assert!(incoming.profiles.contains_key("claude"));
        assert_eq!(
            incoming.active_profile.get("claude"),
            Some(&"work".to_string())
        );
        assert_eq!(
            incoming.default_profile_names.get("claude"),
            Some(&"公司号".to_string())
        );
        // 其余字段仍以用户提交的快照为准——那才是这次保存的内容。
        assert_eq!(incoming.opacity, 60);
    }
}
