mod account;
#[cfg(any(target_os = "macos", test))]
mod app_bundle;
mod bgpty;
mod chat;
mod confirm;
mod detect;
#[cfg(target_os = "windows")]
mod envpath;
mod fsutil;
mod git;
#[cfg(target_os = "windows")]
mod openwith;
mod ports;
mod pty;
mod relay;
#[cfg(target_os = "windows")]
mod seed;
// pub：集成测试 `tests/proxy_apply.rs` 要在**独立进程**里跑端到端写入（它会设 CLAUDE_CONFIG_DIR /
// MEOWO_DB 这类进程级环境变量，与 lib 单测并行跑会互相串味）。
#[cfg(target_os = "macos")]
mod macos;
pub mod proxy;
mod settings;
pub mod snap;
mod term_script;
#[cfg(target_os = "windows")]
mod wezterm;

// 由原 lib.rs 巨石按职责拆出的功能模块（详见各文件头部说明）。
// lib.rs 现只保留：托管状态、数据查询命令、会话读写命令、agent 解析 helper 与 run() 装配。
mod handoff;
mod install;
mod managed_terminal;
mod proc;
// pub：集成测试 `tests/profile_merge.rs` 要在**独立进程**里跑端到端合并（它会设
// CLAUDE_CONFIG_DIR / MEOWO_DB 这类进程级环境变量，与 lib 单测并行跑会互相串味）。
pub mod profile;
mod session_command;
mod session_query;
mod snap_layout;
mod terminal;
mod watch;
mod window;

// run() 的 generate_handler 以裸标识符登记这些命令，须在 crate 根作用域可见。
use chat::{
    clipboard_restore, clipboard_set_image, clipboard_text, get_chat_history,
    get_subagent_transcript, refresh_session_model, refresh_session_todos, save_pasted_attachment,
    search_chat_transcripts,
};
use handoff::{get_session_lineage, switch_session_provider};
use fsutil::{
    list_dir_entries, list_file_openers, open_path_with, read_file_text, read_image_preview,
    reveal_path_in_file_manager, search_project_files,
};
use git::{git_diff_summary, git_file_diff};
use install::{
    add_agent_to_user_path, agent_path_gap, api_key_login, cancel_login, check_provider_hooks,
    install_agent, login_agent, logout_agent, repair_provider_hooks,
};
use managed_terminal::{
    dismiss_interactive_question, managed_terminal_binding,
    managed_terminal_snapshot, attach_background_session, open_attached_terminal,
    pending_interaction, register_approval_consumer, register_terminal_viewer,
    resize_managed_terminal, screen_detect_explain, screen_detect_explain_text,
    send_background_prompt,
    resolve_pending_approval, start_managed_terminal, stop_managed_terminal,
    unregister_approval_consumer, unregister_terminal_viewer, write_managed_terminal,
};
#[cfg(test)]
use session_command::is_safe_id;
use session_command::{rename_session, set_archived, set_session_note};
use session_query::{
    get_live_sessions_counts, get_live_sessions_page, recent_cwds,
};
#[cfg(test)]
use session_query::{
    live_sessions_blocking, session_connected, tab_class, LiveContext, PageReq,
    SessionRuntimeIndex, RESUME_GRACE_MS,
};
use session_command::{session_launch_selections, set_session_launch_selection};
use terminal::{
    focus_session, new_session, open_project_dir, restart_session_supported, resume_session,
    takeover_managed_terminal,
};
use window::{
    open_chat_window, open_latest_chat, open_new_session_window, open_onboarding, open_settings,
    open_update_window, recall_center,
};
// 连接判定的进程事实源统一走 proc::agent_pids_snapshot（按平台分流），
// 由 session_query 缓存成一份跨命令共享的快照；lib 这一层不再直接碰进程表。
use watch::{spawn_board_notifier, spawn_db_watcher, spawn_first_import, spawn_liveness_watch};
#[cfg(not(target_os = "macos"))]
use window::setup_tray;
// settings::set_settings 切语言后调用（全平台）。
pub(crate) use window::apply_language;
// macOS 侧以 crate:: 路径引用这些符号（本机编译不到该平台，re-export 免去改 macos/*.rs）。
// macos::menubar 走 crate::tr / crate::ui_lang / crate::load_settings 构建托盘菜单；
// macos::notify 走 crate::resume_terminal_kind 决定聚焦回退终端。
#[cfg(not(target_os = "windows"))]
pub(crate) use proc::pid_is_agent_ps;
#[cfg(target_os = "macos")]
pub(crate) use settings::{load_settings, tr, ui_lang};
#[cfg(target_os = "macos")]
pub(crate) use terminal::resume_terminal_kind;
#[cfg(target_os = "macos")]
pub(crate) use window::open_settings_window;
// macOS 托盘菜单「使用引导」项走 crate::open_onboarding_window。
#[cfg(target_os = "macos")]
pub(crate) use window::open_onboarding_window;
// macOS 托盘菜单「打开对话窗口」项走 crate::open_latest_chat_window。
#[cfg(target_os = "macos")]
pub(crate) use window::open_latest_chat_window;

use relay::{get_relay_secret_status, get_relay_secrets, list_relay_models, set_relay_secret};
use settings::{
    get_autostart, get_effective_proxy, get_settings, mark_onboarding_seen, open_link, open_url,
    set_autostart, set_settings,
};
use snap::{
    cursor_over_window, pointer_left_down, snap_collapse, snap_expand, snap_restore, unsnap,
};
// 出屏约束/吸边检测（run 的窗口事件闭包）只在非 macOS 用这些几何符号。
#[cfg(target_os = "windows")]
use snap::pull_on_screen;
#[cfg(not(target_os = "macos"))]
use snap::{clamp_xy_to_work, edge_for_rect, Rect, SnapPayload, SNAP_THRESHOLD};

use meowo_store::Store;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri_plugin_updater::UpdaterExt;

pub mod setup;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, State};
// Manager：跨平台都要(install_downloaded_update 在 spawn_blocking 里 app.state::<AppState>(),
// 以及 Windows setup 的 app.get_webview_window("main") 出屏救援/子类化)。
// 曾误加 #[cfg(windows)] 门控 → macOS/Linux 上 .state() 找不到方法(E0599),Windows CI 掩盖。
use tauri::Manager;

/// 托管状态只持有库路径。每个命令按需开短连接——库暂时不可用（被独占锁/损坏/
/// 无权限）时只让该次刷新返回错误，不会在启动时 panic 把整个 app 打挂；
/// 下次 board-changed 事件刷新即自动恢复。
struct AppState {
    db_path: PathBuf,
    /// transcript 增量解析缓存（与后台轮询线程共享 Arc）：避免每次刷新重读整文件。
    tx_cache: Arc<Mutex<meowo_agent::TranscriptCache>>,
    /// 「等长重写」检测所需的 mtime 记录。
    chat_mtimes: Arc<Mutex<chat::ChatMtimes>>,
    /// 会话列表和角标共享的短命进程表快照。
    // Arc:采样(冷路径 30-120ms)在各命令的 spawn_blocking 里做,缓存需跨闭包共享。
    process_snapshots: std::sync::Arc<session_query::ProcessSnapshotCache>,
    /// agent 自建后台会话的索引快照(claude FleetView)。同样在 spawn_blocking 里采样。
    session_runtimes: std::sync::Arc<session_query::SessionRuntimeCache>,
    // Arc:confirm_dialog 的窗口销毁回调要在 'static 闭包里访问待决表。
    confirms: std::sync::Arc<confirm::Confirms>,
    /// 最近一次检查得到的更新包；下载命令复用它，确保检查与下载使用同一份代理策略。
    update: Mutex<Option<tauri_plugin_updater::Update>>,
    /// 已下载且通过 updater 签名校验的安装包。只驻留内存，退出应用后重新下载。
    downloaded_update: Mutex<Option<DownloadedUpdate>>,
    /// 防止主窗自动下载与更新窗口手动下载并发执行。
    update_downloading: AtomicBool,
    /// 由 Meowo 持有的交互式 PTY。对话窗口与后续 attach 客户端共享同一 broker。
    ptys: pty::PtyBroker,
    /// 搭在 agent 自己托管的后台会话上的旁路连接（claude FleetView）。与 ptys 平行:
    /// 那边的进程是我们 spawn 的、生杀予夺;这边只是连上去看画面。
    bg_ptys: bgpty::BgPtyRegistry,
}

struct DownloadedUpdate {
    update: tauri_plugin_updater::Update,
    bytes: Vec<u8>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AvailableUpdate {
    version: String,
    body: Option<String>,
    download_state: String,
}

/// 自更新不能直接调用插件的前端 `check()`：其 IPC API 没有暴露 `no_proxy`，传空代理会回退
/// reqwest 的系统代理。这里由后端显式选 `proxy()` 或 `no_proxy()`，使设置里的「直连」名副其实。
#[tauri::command]
async fn check_update(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<AvailableUpdate>, String> {
    let mut builder = app.updater_builder();
    if let Some(proxy) = ports::resolve_proxy(None) {
        if meowo_agent::is_socks(&proxy) {
            return Err(
                "软件自更新仅支持 HTTP 代理；当前生效的是 SOCKS 代理，请临时改用 HTTP 代理端口"
                    .into(),
            );
        }
        let url = proxy.parse().map_err(|e| format!("代理地址无效：{e}"))?;
        builder = builder.proxy(url);
    } else {
        builder = builder.no_proxy();
    }
    let update = builder
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let ready = if let Some(update) = update.as_ref() {
        let mut slot = state
            .downloaded_update
            .lock()
            .map_err(|_| "内部状态异常，请重启 Meowo".to_string())?;
        if slot
            .as_ref()
            .is_some_and(|downloaded| downloaded.update.version != update.version)
        {
            *slot = None;
        }
        slot.as_ref()
            .is_some_and(|downloaded| downloaded.update.version == update.version)
    } else {
        false
    };
    let result = update.as_ref().map(|u| {
        let download_state = if ready {
            "ready"
        } else if state.update_downloading.load(Ordering::Acquire) {
            "downloading"
        } else {
            "available"
        };
        AvailableUpdate {
            version: u.version.clone(),
            body: u.body.clone(),
            download_state: download_state.to_string(),
        }
    });
    *state
        .update
        .lock()
        .map_err(|_| "内部状态异常，请重启 Meowo".to_string())? = update;
    Ok(result)
}

#[tauri::command]
async fn download_update(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let update = state
        .update
        .lock()
        .map_err(|_| "内部状态异常，请重启 Meowo".to_string())?
        .clone()
        .ok_or("请先检查更新")?;
    if state
        .downloaded_update
        .lock()
        .map_err(|_| "内部状态异常，请重启 Meowo".to_string())?
        .as_ref()
        .is_some_and(|downloaded| downloaded.update.version == update.version)
    {
        return Ok("ready".to_string());
    }
    if state
        .update_downloading
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok("downloading".to_string());
    }

    let mut downloaded = 0u64;
    let result = update
        .download(
            |chunk, total| {
                downloaded = downloaded.saturating_add(chunk as u64);
                let _ = app.emit(
                    "update-download-progress",
                    serde_json::json!({
                        "downloaded": downloaded,
                        "contentLength": total,
                    }),
                );
            },
            || {},
        )
        .await;
    state.update_downloading.store(false, Ordering::Release);

    match result {
        Ok(bytes) => {
            let version = update.version.clone();
            *state
                .downloaded_update
                .lock()
                .map_err(|_| "内部状态异常，请重启 Meowo".to_string())? =
                Some(DownloadedUpdate { update, bytes });
            let _ = app.emit(
                "update-download-finished",
                serde_json::json!({ "version": version }),
            );
            Ok("ready".to_string())
        }
        Err(error) => {
            let message = error.to_string();
            let _ = app.emit("update-download-failed", &message);
            Err(message)
        }
    }
}

#[tauri::command]
async fn install_downloaded_update(app: tauri::AppHandle) -> Result<(), String> {
    // async + spawn_blocking：install 要把几十 MB 安装包落盘并拉起安装进程，同步命令会
    // 让所有窗口在退出前的最后几秒集体「未响应」。State 不能跨进 blocking 闭包，改经
    // AppHandle 在闭包内取。
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let downloaded = state
            .downloaded_update
            .lock()
            .map_err(|_| "内部状态异常，请重启 Meowo".to_string())?
            .take()
            .ok_or("更新尚未下载完成")?;
        if let Err(error) = downloaded.update.install(&downloaded.bytes) {
            let mut slot = state
                .downloaded_update
                .lock()
                .map_err(|_| "内部状态异常，请重启 Meowo".to_string())?;
            if slot.is_none() {
                *slot = Some(downloaded);
            }
            return Err(error.to_string());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 退出收尾：更新包已下载校验过但用户没点「重启并更新」时，趁退出静默安装，
/// 下次手动启动即新版。任何一步失败都放弃——包只驻内存，下次启动照常重走下载。
fn install_downloaded_on_exit(app: &tauri::AppHandle) {
    // take：用户已点「重启并更新」的话 slot 为空，不会装第二遍。
    let Some(downloaded) = app
        .state::<AppState>()
        .downloaded_update
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
    else {
        return;
    };
    #[cfg(windows)]
    {
        // 不复用 update.install()：它固定追加 /R 让 NSIS 装完把应用重新拉起，而这里
        // 用户的意图是退出。自己落盘后传 /S /UPDATE（同 updater 的静默升级路径，见
        // installer.nsi $UpdateMode），不带 /R。本进程尚未死透不碍事：NSIS 侧
        // CheckIfAppIsRunning 在静默模式下自行处理残留进程。
        let path = std::env::temp_dir().join(format!(
            "meowo-update-{}-setup.exe",
            downloaded.update.version
        ));
        if std::fs::write(&path, &downloaded.bytes).is_err() {
            return;
        }
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new(&path)
            .args(["/S", "/UPDATE"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        // macOS 的 install 是原地覆盖 .app，不拉起新进程也不退出本进程，退出路径直接调。
        let _ = downloaded.update.install(&downloaded.bytes);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = downloaded;
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn db_path() -> PathBuf {
    if let Ok(p) = std::env::var("MEOWO_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".meowo").join("board.db")
}

/// 从旧品牌 cc-kanban 迁移本地数据目录到 ~/.meowo。
/// 仅在 MEOWO_DB 未覆盖、~/.meowo 不存在而 ~/.cc-kanban 存在时执行一次。
pub(crate) fn migrate_legacy_data() {
    if std::env::var("MEOWO_DB").is_ok() {
        return;
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let old_dir = PathBuf::from(&home).join(".cc-kanban");
    let new_dir = PathBuf::from(&home).join(".meowo");
    if !old_dir.exists() || new_dir.exists() {
        return;
    }
    fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let dest = dst.join(entry.file_name());
            if path.is_dir() {
                copy_dir_all(&path, &dest)?;
            } else {
                std::fs::copy(&path, &dest)?;
            }
        }
        Ok(())
    }
    if let Err(e) = copy_dir_all(&old_dir, &new_dir) {
        eprintln!("Meowo 迁移旧数据目录失败: {e}");
    } else {
        println!(
            "Meowo 已迁移旧数据目录: {} -> {}",
            old_dir.display(),
            new_dir.display()
        );
    }
}

fn open_store(path: &std::path::Path) -> Result<Store, String> {
    Store::open(path).map_err(|e| e.to_string())
}

/// 前端/DB 传来的 provider 串 → agent 身份。**未知 id → `None`**，绝不冒名成默认 agent：
/// 那会让一个本版本尚不认识的 agent 被按 claude 去 resume / 装 / 查用量。调用方据此降级
/// （command 报错、读操作跳过 agent 专属能力）。空/缺省 → 默认 agent（老会话没写过 provider 列）。
fn agent_id(provider: &str) -> Option<meowo_agent::AgentId> {
    meowo_agent::resolve(Some(provider)).map(|p| p.id())
}

/// 该 provider 的 transcript 规格（未知 agent / 无遥测能力 / 不读 transcript → None）。
/// claude/codex/kimi 都可提供；是否从中取标题由独立 capability 决定。
fn agent_transcript(provider: &str) -> Option<&'static dyn meowo_agent::TranscriptSpec> {
    meowo_agent::resolve(Some(provider))?
        .telemetry()?
        .transcript()
}

fn agent_resolves_transcript_title(provider: &str) -> bool {
    meowo_agent::resolve(Some(provider))
        .and_then(|agent| agent.telemetry())
        .is_some_and(|telemetry| telemetry.resolves_transcript_title())
}

/// 某 agent 在本机的安装实况。直接走插件注册表——不再按 id 分支到各自的解析入口。
fn install_for(id: meowo_agent::AgentId) -> Option<meowo_agent::Installation> {
    meowo_agent::by_id(id.as_str())?.resolve()
}

/// 返回所有 provider 的账号 + 缓存用量（不联网）。供多 provider 账号面板使用。
/// async + spawn_blocking：account() / usage_supported() 的 claude 分支会调
/// has_oauth_credentials() → read_credentials_root()，macOS 上可 spawn `security` 子进程；
/// 与 refresh_usage 同款写法，确保不占主线程事件循环（防设置页卡死）。
#[tauri::command]
async fn get_accounts() -> Vec<account::ProviderAccountPayload> {
    tauri::async_runtime::spawn_blocking(|| {
        account::all_with_account()
            .map(|p| account::ProviderAccountPayload {
                provider: p.id().as_str().to_string(),
                account: account::account_of_display(p.id()),
                usage: if settings::load_settings().relay.enabled(p.id()) {
                    None
                } else {
                    account::read_cached_usage(p.id())
                },
                usage_supported: account::usage_supported(p.id()),
                relay_enabled: settings::load_settings().relay.enabled(p.id()),
                active_profile_name: profile::active_display_name_in(
                    &settings::load_settings(),
                    p.id().as_str(),
                ),
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 刷新指定 provider 的用量（可触发网络请求，含 60s 限频）。
/// None 时按 usage_supported 返回 UNAVAILABLE 或 USAGE_UNSUPPORTED。
#[tauri::command]
async fn refresh_usage(provider: String) -> Result<meowo_agent::ProviderUsage, String> {
    let key = agent_id(&provider).ok_or("未知 agent")?;
    tauri::async_runtime::spawn_blocking(move || match account::usage_of(key, true) {
        Some(u) => Ok(u),
        None if account::usage_supported(key) => Err("UNAVAILABLE".into()),
        None => Err(account::USAGE_UNSUPPORTED.to_string()),
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 返回宿主操作系统标识，供前端按平台调整 UI / 交互。
#[tauri::command]
fn host_os() -> String {
    #[cfg(target_os = "macos")]
    {
        "macos".into()
    }
    #[cfg(target_os = "windows")]
    {
        "windows".into()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "other".into()
    }
}

/// 「打开未连接会话」可选且本机确实可用的终端 key（供设置页过滤下拉项）。
/// macOS：terminal 必有，iterm/ghostty 视安装情况；Windows：powershell/cmd 必有，wt/wezterm 视是否在 PATH。
/// async：丢到线程池跑。同步命令内联在主线程，探测一旦变慢（如 macOS 的 mdfind）
/// 会冻结整个事件循环；设置页每次打开都调它，绝不能赌探测耗时。
#[tauri::command]
async fn available_terminals() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        // iterm_installed 可能跑 mdfind（秒级），包 spawn_blocking 以免占住 tokio worker。
        tauri::async_runtime::spawn_blocking(|| {
            let mut v = vec!["terminal".to_string()];
            if terminal::iterm_installed() {
                v.push("iterm".to_string());
            }
            if terminal::ghostty_installed() {
                v.push("ghostty".to_string());
            }
            v
        })
        .await
        .unwrap_or_else(|_| vec!["terminal".to_string()])
    }
    #[cfg(target_os = "windows")]
    {
        let mut v = Vec::new();
        if terminal::wt_available() {
            v.push("wt".to_string());
        }
        if wezterm::available() {
            v.push("wezterm".to_string());
        }
        v.push("powershell".to_string());
        v.push("cmd".to_string());
        v
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::<String>::new()
    }
}

/// 前端看到的一个 agent。**这是前端认识 agent 的唯一途径**——它不再维护自己的 agent 名单。
///
/// 不含图标与品牌色：那是**视觉资产**，不是 agent 的定义。位图 logo（kimi）与主题相关的品牌色
/// （claude 在浅色/深色下取不同明度）都无法诚实地塞进后端的一个字符串字段里，故留在前端的资产表。
/// 描述符定义与组装在 `meowo_agent::descriptor`（全部字段由插件声明推导），此处仅透传。
///
/// 全部已注册 agent 及其本机安装状态。仿 available_terminals：检测廉价（PATH/文件查询），
/// 仍放 blocking 池避免任何意外阻塞事件循环。
#[tauri::command]
async fn list_agents() -> Vec<meowo_agent::descriptor::AgentDescriptor> {
    tauri::async_runtime::spawn_blocking(|| {
        meowo_agent::all()
            .iter()
            .map(|a| meowo_agent::descriptor::AgentDescriptor::of(*a))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default()
}

/// 以插件的 `launch_argv` 起子进程跑一个**非交互**子命令，有界等待后返回 stdout 全文。
/// 正常亚秒返回，但一个挂死的 CLI 不该把 blocking 线程一起挂死——过 deadline 即 kill。
/// `probe_cli_version`（`--version`，5s）与 `agent_models`（模型清单，15s）共用；
/// 必须在 blocking 池调用（spawn + sleep 轮询，见 CLAUDE.md 线程纪律）。
fn run_cli_capture(
    plugin: &'static dyn meowo_agent::AgentPlugin,
    extra_args: &[&str],
    timeout: std::time::Duration,
) -> Option<String> {
    let argv = plugin.launch_argv();
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .args(extra_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    // stdout 必须边跑边排水：不排水的话，子进程输出一超过管道缓冲（~64KB）写端就
    // 阻塞，进程永远退不出——轮询只会等到超时把它 kill，还把空结果错误地进了缓存。
    let mut stdout = child.stdout.take()?;
    let drain = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                break;
            }
        }
    }
    // kill 之后也要 wait 收尸：写端句柄随进程退出关闭，读线程才能到 EOF；
    // 超时前已读到的部分输出保留返回，不随 kill 一起丢。
    let _ = child.wait();
    let bytes = drain.join().ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// 已装 CLI 的版本，`<launch_argv> --version` 探测，**进程级缓存**（版本只随重装变化，而
/// node 系 CLI 冷启动要一秒上下，不能每开一个对话窗都付一次）。探测失败缓存 None，不反复重试。
fn probe_cli_version(plugin: &'static dyn meowo_agent::AgentPlugin) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    static CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let id = plugin.id().as_str().to_string();
    // 中毒恢复而非 unwrap：缓存只是探测结果的备忘录，读到写一半的旧值无害；
    // panic 则让之后每次开对话窗都失败（本函数跑在 blocking 池，用户只会看到「打不开」）。
    if let Some(hit) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
        return hit.clone();
    }
    let version = run_cli_capture(plugin, &["--version"], std::time::Duration::from_secs(5))
        .and_then(|txt| {
            txt.lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string)
        });
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, version.clone());
    version
}

/// 该 agent 支持的模型清单，**问 CLI 自己要**（非交互子命令，见插件的 `model_list_command`）。
///
/// 存在的理由：模型清单必随 CLI 改版漂移，宿主维护一份必然过时；而唯一的替代方案——往会话
/// PTY 里发 `/model`、把弹出的 TUI 菜单靠屏幕识别读回来——要求菜单形态经过取证，opencode
/// 的 TUI 输入在 ConPTY 下至今不通（见 docs/research/tui-menu-captures-2026-07.md）。
/// 这条路只要 CLI 有列举子命令就成立，而且完全不打扰正在跑的会话。
///
/// 进程级缓存：清单只随重装/升级变化，而 CLI 冷启动要一秒上下，不能每次开菜单都付一次。
/// 未声明该能力的 agent 返回空表——调用方据此回落到菜单识别通道。
#[tauri::command]
async fn agent_models(provider: String) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::collections::HashMap;
        use std::sync::{LazyLock, Mutex};
        static CACHE: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
            LazyLock::new(|| Mutex::new(HashMap::new()));

        let Some(plugin) = meowo_agent::resolve(Some(&provider)) else {
            return Vec::new();
        };
        let key = plugin.id().as_str().to_string();
        // 中毒恢复而非 unwrap：缓存只是备忘录，读到写一半的旧值无害（同 probe_cli_version）。
        if let Some(hit) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return hit.clone();
        }
        let Some(args) = plugin.model_list_command() else {
            return Vec::new();
        };
        let models = run_cli_capture(plugin, args, std::time::Duration::from_secs(15))
            .map(|txt| {
                txt.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // 空结果同样入缓存：CLI 没这个子命令/跑失败时不该每次开菜单都重试一遍。
        CACHE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, models.clone());
        models
    })
    .await
    .unwrap_or_default()
}

/// 对话页能力：按会话查询（provider + cwd），由**安装实况**组装——插件的内置表 ∪ 用户/项目
/// 目录里发现的自定义命令 + 探测到的 CLI 版本。装了新命令、换了版本，下次打开会话就反映。
/// 未知 provider → None，前端降级为不补全、不给模型菜单。
#[tauri::command]
async fn agent_chat_ui(
    provider: String,
    cwd: Option<String>,
    session_id: Option<i64>,
) -> Option<meowo_agent::ChatUi> {
    tauri::async_runtime::spawn_blocking(move || {
        let plugin = meowo_agent::resolve(Some(&provider))?;
        let version = probe_cli_version(plugin);
        let external_session_id = session_id.and_then(|session_id| {
            open_store(&db_path())
                .ok()?
                .session_header(session_id)
                .ok()
                .map(|header| header.cc_session_id)
        });
        Some(plugin.chat_ui(&meowo_agent::ChatUiContext {
            cwd: cwd.as_deref().map(std::path::Path::new),
            version: version.as_deref(),
            session_id: external_session_id.as_deref(),
        }))
    })
    .await
    .ok()
    .flatten()
}

/// 用 Win32 窗口子类化在「移动生效前」硬约束贴纸位置，彻底拖不出屏幕（零抖动，
/// 优于事后 set_position 拉回）。拦截 WM_WINDOWPOSCHANGING，把目标坐标钳进所有显示器
/// 工作区的并集包围盒。
#[cfg(target_os = "windows")]
mod win_constrain {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SWP_NOMOVE, SWP_NOSIZE, WINDOWPOS, WM_EXITSIZEMOVE, WM_SIZING,
        WM_WINDOWPOSCHANGING,
    };

    const SUBCLASS_ID: usize = 0x00CC_4A0B;

    /// 用户拖边框缩放时通知前端用的 AppHandle（启动时注入）。
    static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
    /// 本次缩放手势是否已通知过（一次拖拽只发一次 user-resized）。
    static RESIZE_EMITTED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// 注入 AppHandle（在装子类时一并调用）。
    pub fn set_app(app: tauri::AppHandle) {
        let _ = APP.set(app);
    }

    /// 累积所有显示器工作区(rcWork)的并集包围盒。
    struct Bbox {
        has: bool,
        l: i32,
        t: i32,
        r: i32,
        b: i32,
    }

    unsafe extern "system" fn enum_proc(
        hmon: HMONITOR,
        _hdc: HDC,
        _rc: *mut RECT,
        data: LPARAM,
    ) -> i32 {
        let bb = &mut *(data as *mut Bbox);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            let w = mi.rcWork;
            if !bb.has {
                (bb.l, bb.t, bb.r, bb.b, bb.has) = (w.left, w.top, w.right, w.bottom, true);
            } else {
                bb.l = bb.l.min(w.left);
                bb.t = bb.t.min(w.top);
                bb.r = bb.r.max(w.right);
                bb.b = bb.b.max(w.bottom);
            }
        }
        1 // TRUE：继续枚举
    }

    fn virtual_work_bbox() -> Option<(i32, i32, i32, i32)> {
        let mut bb = Bbox {
            has: false,
            l: 0,
            t: 0,
            r: 0,
            b: 0,
        };
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(enum_proc),
                &mut bb as *mut Bbox as LPARAM,
            );
        }
        bb.has.then_some((bb.l, bb.t, bb.r, bb.b))
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _ref: usize,
    ) -> LRESULT {
        if msg == WM_WINDOWPOSCHANGING {
            let wp = &mut *(lparam as *mut WINDOWPOS);
            // 仅在真正移动时约束（SWP_NOMOVE 表示这次不改位置）。
            if (wp.flags & SWP_NOMOVE) == 0 {
                // 取窗口尺寸：SWP_NOSIZE（纯移动，拖拽就是这种）下 wp.cx/cy 无效，
                // 必须用 GetWindowRect 取真实尺寸，否则右/下边界算错、能拖出屏幕。
                let (w, h) = if (wp.flags & SWP_NOSIZE) != 0 {
                    let mut rc: RECT = std::mem::zeroed();
                    if GetWindowRect(hwnd, &mut rc) != 0 {
                        (rc.right - rc.left, rc.bottom - rc.top)
                    } else {
                        (0, 0)
                    }
                } else {
                    (wp.cx, wp.cy)
                };
                if w > 0 && h > 0 {
                    if let Some((l, t, r, b)) = virtual_work_bbox() {
                        // 钳进包围盒；窗口比包围盒还大时左上对齐。
                        let max_x = (r - w).max(l);
                        let max_y = (b - h).max(t);
                        wp.x = wp.x.clamp(l, max_x);
                        wp.y = wp.y.clamp(t, max_y);
                    }
                }
            }
        } else if msg == WM_SIZING {
            // WM_SIZING 仅在用户拖边框缩放时发（程序 set_size 不发）→ 通知前端解除吸附。
            // 一次拖拽手势只发一次，避免刷屏。
            use std::sync::atomic::Ordering;
            if !RESIZE_EMITTED.swap(true, Ordering::Relaxed) {
                if let Some(app) = APP.get() {
                    use tauri::Emitter;
                    let _ = app.emit("user-resized", ());
                }
            }
        } else if msg == WM_EXITSIZEMOVE {
            // 缩放/移动手势结束：若本次确实缩放过（发过 user-resized），通知前端"缩放结束"，
            // 供其按缩放前的吸附状态重新吸回。复位标志，下次拖拽可再次通知。
            use std::sync::atomic::Ordering;
            if RESIZE_EMITTED.swap(false, Ordering::Relaxed) {
                if let Some(app) = APP.get() {
                    use tauri::Emitter;
                    let _ = app.emit("user-resize-end", ());
                }
            }
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    /// 给窗口装上位置约束子类（重复调用安全：同 id 覆盖）。`hwnd` 取自 tauri 的 window.hwnd()。
    pub fn install(hwnd: isize) {
        unsafe {
            SetWindowSubclass(hwnd as HWND, Some(subclass_proc), SUBCLASS_ID, 0);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 把 DLL 搜索路径收紧到「应用目录 + System32」，进程一起来就做（任何 LoadLibrary 之前）。
///
/// 动机是一条真实的加载链：托管 PTY 走 portable-pty，它在 `load_conpty()` 里用**相对
/// 文件名** `ConPtyFuncs::open(Path::new("conpty.dll"))` 尝试 sideload（psuedocon.rs，
/// 上游至 0.9.0 仍如此），相对名会走完整搜索顺序——**当前工作目录也在其中**。而 Meowo
/// 的 cwd 由启动方式决定：从资源管理器双击某目录里的快捷方式、拖文件到图标、或从某个
/// 项目目录起的终端里启动，cwd 就是那个目录。目录里只要躺着一个恶意 `conpty.dll`
/// （压缩包/共享盘/下载目录里被投毒），它就会被加载进本进程——而 ConPTY 句柄等于终端
/// 完全控制权。herdr 为同一问题给 portable-pty 打了 vendored 补丁（哈希校验 + 绝对
/// 路径加载，见 microsoft/terminal#17452）；我们不 vendor，用系统级的搜索路径收紧
/// 达到同一效果：cwd 与 PATH 被整个移出搜索顺序，合法的 sideload（应用目录内）不受影响。
///
/// 幂等且无副作用：Win7+ 恒可用（`SetDefaultDllDirectories` 自 KB2533623），失败也只是
/// 回到默认搜索顺序，不阻断启动——它是纵深防御的一层，不是功能依赖。
fn harden_dll_search_path() {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: 纯进程级标志设置，无指针参数；失败返回 0，无需回滚。
        unsafe {
            windows_sys::Win32::System::LibraryLoader::SetDefaultDllDirectories(
                windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            );
        }
    }
}

pub fn run() {
    // Tauri 的 macOS updater 原地覆盖当前 `.app`，不会把旧外层目录 `cc-kanban.app` 改成
    // `Meowo.app`。在创建任何插件状态前先迁移并从新路径重启，使 updater / autostart 后续都只
    // 看到规范路径。开发态不处在该 bundle 结构中，自动 no-op。
    #[cfg(target_os = "macos")]
    if app_bundle::migrate_legacy_bundle_and_relaunch() {
        return;
    }
    harden_dll_search_path();
    // ConPTY 来源诊断:应用目录里有打包的 conpty.dll(随安装分发 + OpenConsole.exe 宿主,
    // 见 scripts/fetch-conpty.mjs)时 portable-pty 会优先加载它——新版实现修掉了系统
    // conhost 的一批僵死死锁(输出停滞/Resize/Close 挂起),是管道卡死的治本项。缺失则
    // 回退系统 conhost,僵死概率回到用户 OS 版本的水平——这行日志是现场诊断的第一问。
    #[cfg(target_os = "windows")]
    {
        let bundled = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("conpty.dll").exists()))
            .unwrap_or(false);
        eprintln!(
            "ConPTY: {}",
            if bundled {
                "打包版(应用目录 conpty.dll + OpenConsole.exe)"
            } else {
                "系统 conhost(未找到打包版,僵死修复以 OS 版本为准)"
            }
        );
    }
    migrate_legacy_data();
    let path = db_path();
    let tx_cache: Arc<Mutex<meowo_agent::TranscriptCache>> =
        Arc::new(Mutex::new(meowo_agent::TranscriptCache::new()));
    let ptys = pty::PtyBroker::default();
    let approval_ptys = ptys.clone();
    let exit_ptys = ptys.clone();
    if let Err(error) = ptys.start_attach_server() {
        eprintln!("启动 PTY attach 服务失败: {error}");
    }
    let builder = tauri::Builder::default();
    // single-instance 必须最先注册（官方要求，越早越能拦住二次启动）。仅 release：
    // debug 构建豁免——本机开发的常态是「安装版自启 + dev 版并行」，同 identifier 的
    // single-instance 会把 dev 实例当二次启动当场掐死，开发流程整个断掉；双开的已知
    // 代价（discovery 文件互覆、双份轮询）由开发环境自担。线上则根治整类双实例问题：
    // 双击第二次 = 把已有实例的看板带到前台，而不是两个进程互抢 approval-broker.json、
    // 对同一会话各拉一个 agent（CLI 报 already in use 秒退）。
    // cfg! 而非 #[cfg]：两个分支都要过编译——release 专属代码若只在 release 编译，
    // dev 的 clippy/CI 全绿也发现不了它坏了（平台专属代码的同款教训，见 CLAUDE.md）。
    let builder = if cfg!(debug_assertions) {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
    };
    // E2E 构建（cargo --features e2e）才注入 WDIO 内嵌 WebDriver 服务器 + execute/mock/日志桥；
    // 生产构建不含这两个插件（见 Cargo.toml [features].e2e 与 app/e2e/README.md）。
    #[cfg(feature = "e2e")]
    let builder = builder
        .plugin(tauri_plugin_wdio::init())
        .plugin(tauri_plugin_wdio_webdriver::init());
    builder
        // window-state 只持久化/恢复「位置」等，不恢复「尺寸」：main 窗口尺寸改由前端 localStorage
        // (SIZE_KEY) 单独持有。否则吸附态退出会把「细条几何」存进 window-state，与 localStorage 的吸附态
        // (SNAP_KEY) 两套持久化不同步——重启读不到 SNAP_KEY 却被还原成细条尺寸，渲染完整贴纸而没真正吸附。
        // about 设置窗口固定尺寸(resizable=false)，不受影响；折叠/正常尺寸均由前端 snap 逻辑权威设定。
        // VISIBLE 也不恢复：设置/对话窗口以 visible:false 创建、前端首帧后才显示（消除白框闪烁），
        // 插件若按上次退出时的「可见」在创建当口就 show，隐藏创建被当场抵消，白框闪烁复发。
        // 可见性完全由代码显式控制（main 由 tauri.conf 的 visible:true，其余窗口自管）。
        // DECORATIONS 也不恢复：装饰由各建窗代码按平台显式设定（macOS 原生 Overlay 标题栏、
        // 非 macOS 无边框自绘），插件把旧版存下的 decorated:false 在创建当口还原回去，
        // macOS 的原生红绿灯/圆角会被当场剥掉（表现为设置/更新等窗口方角无按钮）。
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(tauri_plugin_window_state::StateFlags::all().difference(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::VISIBLE
                        | tauri_plugin_window_state::StateFlags::DECORATIONS,
                ))
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            db_path: path.clone(),
            tx_cache: tx_cache.clone(),
            chat_mtimes: Arc::new(Mutex::new(chat::ChatMtimes::default())),
            process_snapshots: std::sync::Arc::new(session_query::ProcessSnapshotCache::default()),
            session_runtimes: std::sync::Arc::new(session_query::SessionRuntimeCache::default()),
            bg_ptys: bgpty::BgPtyRegistry::default(),
            confirms: std::sync::Arc::new(confirm::Confirms::default()),
            update: Mutex::new(None),
            downloaded_update: Mutex::new(None),
            update_downloading: AtomicBool::new(false),
            ptys,
        })
        .invoke_handler(tauri::generate_handler![
            get_live_sessions_counts,
            get_live_sessions_page,
            get_chat_history,
            search_chat_transcripts,
            confirm::confirm_dialog,
            confirm::confirm_dialog_payload,
            confirm::confirm_dialog_result,
            get_subagent_transcript,
            refresh_session_model,
            refresh_session_todos,
            save_pasted_attachment,
            clipboard_set_image,
            clipboard_restore,
            clipboard_text,
            open_chat_window,
            snap_layout::set_caption_overlay_rects,
            start_managed_terminal,
            takeover_managed_terminal,
            session_launch_selections,
            set_session_launch_selection,
            managed_terminal_snapshot,
            managed_terminal_binding,
            write_managed_terminal,
            resize_managed_terminal, send_background_prompt,
            screen_detect_explain,
            screen_detect_explain_text,
            pending_interaction,
            dismiss_interactive_question,
            stop_managed_terminal,
            register_terminal_viewer,
            unregister_terminal_viewer,
            register_approval_consumer,
            unregister_approval_consumer,
            resolve_pending_approval,
            open_attached_terminal,
            attach_background_session,
            focus_session,
            resume_session,
            restart_session_supported,
            open_project_dir,
            git_diff_summary,
            git_file_diff,
            list_dir_entries,
            read_file_text,
            read_image_preview,
            search_project_files,
            list_file_openers,
            open_path_with,
            reveal_path_in_file_manager,
            rename_session,
            set_archived,
            set_session_note,
            get_autostart,
            set_autostart,
            get_settings,
            set_settings,
            mark_onboarding_seen,
            get_effective_proxy,
            get_relay_secret_status,
            get_relay_secrets,
            list_relay_models,
            set_relay_secret,
            check_update,
            download_update,
            install_downloaded_update,
            open_settings,
            open_onboarding,
            open_update_window,
            recall_center,
            open_link,
            open_url,
            snap_collapse,
            snap_expand,
            snap_restore,
            unsnap,
            cursor_over_window,
            pointer_left_down,
            get_accounts,
            refresh_usage,
            host_os,
            available_terminals,
            list_agents,
            agent_chat_ui,
            agent_models,
            profile::list_profiles,
            profile::create_profile,
            profile::rename_profile,
            profile::set_active_profile,
            profile::delete_profile,
            profile::merge_profile_into_default,
            new_session,
            switch_session_provider,
            get_session_lineage,
            install_agent,
            agent_path_gap,
            add_agent_to_user_path,
            login_agent,
            api_key_login,
            logout_agent,
            cancel_login,
            check_provider_hooks,
            repair_provider_hooks,
            recent_cwds,
            open_new_session_window,
            open_latest_chat
        ])
        .on_window_event(|window, event| {
            // macOS：面板模式，无出屏约束/吸边；不处理 Moved（避免与 positioner 抢位置、误发 snap-changed）。
            #[cfg(target_os = "macos")]
            let _ = (window, event);
            #[cfg(not(target_os = "macos"))]
            if let tauri::WindowEvent::Moved(pos) = event {
                // 出屏约束与吸附只作用于贴纸主窗口；设置等其它窗口不受限制。
                if window.label() != "main" {
                    return;
                }
                let Ok(size) = window.outer_size() else {
                    return;
                };
                let win = Rect {
                    x: pos.x,
                    y: pos.y,
                    w: size.width as i32,
                    h: size.height as i32,
                };

                // 限制贴纸不被拖出屏幕：把窗口钳进「所有显示器工作区的并集包围盒」。
                // 越界就立刻拉回，拖到边缘即停（吸边仍在界内，不受影响）。多显示器下可在并集内自由移动。
                let vwork = window.available_monitors().ok().and_then(|ms| {
                    let mut it = ms.iter().map(|m| {
                        let wa = m.work_area();
                        (
                            wa.position.x,
                            wa.position.y,
                            wa.position.x + wa.size.width as i32,
                            wa.position.y + wa.size.height as i32,
                        )
                    });
                    let (mut ax, mut ay, mut bx, mut by) = it.next()?;
                    for (x0, y0, x1, y1) in it {
                        ax = ax.min(x0);
                        ay = ay.min(y0);
                        bx = bx.max(x1);
                        by = by.max(y1);
                    }
                    Some(Rect {
                        x: ax,
                        y: ay,
                        w: bx - ax,
                        h: by - ay,
                    })
                });
                if let Some(vwork) = vwork {
                    let (cx, cy) = clamp_xy_to_work(win, vwork);
                    if (cx, cy) != (win.x, win.y) {
                        let _ = window.set_position(tauri::PhysicalPosition::new(cx, cy));
                        return; // 钳正后会再触发一次 Moved（已在界内），那次再算吸附边
                    }
                }

                // 贴边检测（用当前显示器工作区）。
                if let Ok(Some(m)) = window.current_monitor() {
                    let wa = m.work_area();
                    let work = Rect {
                        x: wa.position.x,
                        y: wa.position.y,
                        w: wa.size.width as i32,
                        h: wa.size.height as i32,
                    };
                    // 阈值按缩放放大：SNAP_THRESHOLD 是逻辑手感（20 逻辑像素），而这里全程
                    // 物理像素比较——不乘 scale 的话 150% 屏上等效只剩 13 逻辑像素、200% 剩
                    // 10，同一拖拽动作「有时吸得住有时吸不住」，用户会归因为随机故障
                    // （条厚度 STRIP_W_LOGICAL 早就乘了 scale，两处度量此前口径不一）。
                    let threshold = (f64::from(SNAP_THRESHOLD) * m.scale_factor()).round() as i32;
                    let edge = edge_for_rect(win, work, threshold);
                    let _ = window.emit("snap-changed", SnapPayload { edge });
                }
            }
        })
        .setup(move |app| {
            // 安装器的「对话窗口功能」勾选种子必须最先合并：下面的托盘构建、onboarding
            // 判断等都消费 load_settings()，晚了就会拿开关生效前的旧值建 UI。
            // （同步执行的线程纪律豁免理由见 seed.rs 文件头。）
            #[cfg(target_os = "windows")]
            seed::consume_installer_seed();
            // attach 服务在线程中接收 hook 审批；拿到 AppHandle 后主动推送给对话窗口，
            // 前端轮询仅作为窗口晚打开时的兜底。
            approval_ptys.set_app_handle(app.handle().clone());
            // 托管会话的屏幕状态检测（working/idle/blocked 角标）。在 set_app_handle 之后
            // 启动：状态变化要经 AppHandle 发看板刷新。
            approval_ptys.start_screen_detect();
            // macOS：纯菜单栏 App（隐藏 Dock 图标），main 窗口转 NSPanel，托盘走 menubar 模块。
            #[cfg(target_os = "macos")]
            {
                // bundle 改名后同步刷新旧版留下的 LaunchAgent；先建新项再删旧项，失败不丢设置。
                app_bundle::migrate_legacy_autostart(app.handle());
                app.handle()
                    .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
                // nspanel 插件必须先注册（它 manage(WebviewPanelManager)），to_panel()/get_webview_panel()
                // 才能取到该托管状态；漏注册会在启动时 panic：state() called before manage()。
                // nspanel 是 macOS-only crate，无法放进跨平台 Builder 链，故在此运行时注册。
                app.handle().plugin(tauri_nspanel::init())?;
                crate::macos::panel::convert_main_to_panel(app.handle());
                crate::macos::panel::setup_resign_listener(app.handle());
                crate::macos::menubar::setup_tray(app.handle())?;
                crate::macos::notify::init(app.handle());
            }
            #[cfg(not(target_os = "macos"))]
            {
                setup_tray(app)?;
                // 贴纸以 visible:false 创建（消除启动白框闪烁），前端首帧后自行显示
                // （useShowWhenReady，不抢焦点）；这里兜底：前端没起来也要到点可见。
                // macOS 不需要——面板模式的贴纸本就隐藏创建、显隐归 menubar 管。
                if let Some(w) = app.get_webview_window("main") {
                    crate::window::show_after_grace(&w, false);
                }
            }
            // window-state 恢复后，若贴纸落在所有显示器之外（多屏拔插/分辨率变化）则救回，避免「找不到」。
            #[cfg(target_os = "windows")]
            if let Some(w) = app.get_webview_window("main") {
                pull_on_screen(&w, false);
                // 开机自启安全网：OS 刚登录时显示器/工作区可能尚未枚举完（多屏、副屏在负坐标、外接屏后上电），
                // 此刻 available_monitors() 为空会让上面的 pull_on_screen 直接跳过救援；而贴纸 skipTaskbar 不进
                // 任务栏，窗口若停在上次副屏(未就绪)的坐标就完全不可见、用户以为「没启动」。这里后台等显示器
                // 就绪后强制(force=true)把窗口钳进相交最大/主显示器工作区，保证可见——且不依赖前端 JS 是否跑起来。
                // clamp 对已在屏内的窗口是 no-op，不会无故移动正常摆放的窗口。
                {
                    let wc = w.clone();
                    std::thread::spawn(move || {
                        for _ in 0..40 {
                            // ~6s
                            if wc
                                .available_monitors()
                                .map(|m| !m.is_empty())
                                .unwrap_or(false)
                            {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(150));
                        }
                        pull_on_screen(&wc, true);
                    });
                }
                // 装上位置约束子类：在移动生效前硬钳坐标，彻底拖不出屏幕。
                if let Ok(h) = w.hwnd() {
                    win_constrain::set_app(app.handle().clone()); // 供子类拖边框缩放时通知前端
                    win_constrain::install(h.0 as isize);
                }
            }
            // 无感适配：幂等把 meowo-reporter 接入各 AI CLI（claude: hooks+statusLine；codex/kimi: hooks）。后台跑，失败不影响启动。
            // dev 构建默认不自动接线：hook/statusline 是全局单槽位（~/.claude/settings.json 等），
            // 后启动者必然静默抢线（wiring 的 bundled.or(claimed)），dev 一开安装版的接线就被
            // 改指 target/debug——把槽位留给安装版。手动「修复连接」不经此门，仍可显式接线。
            if setup::auto_wire_enabled() {
                std::thread::spawn(setup::apply_all);
            } else {
                eprintln!("Meowo dev: 跳过启动自动接线(hooks/statusline)；需要时用设置页「修复连接」或设 MEOWO_DEV_WIRE=1");
            }
            // 先起合流线程：其余几个 spawn_* 都经 emit_board_changed 发事件，晚起会让它们的
            // 首批事件退化成直接 emit。
            spawn_board_notifier(app.handle().clone());
            spawn_db_watcher(app.handle().clone(), path.clone());
            spawn_liveness_watch(app.handle().clone(), path.clone(), tx_cache.clone());
            spawn_first_import(app.handle().clone(), path.clone());
            // %TEMP%\meowo-paste 没有任何 OS 侧回收（Windows 不清 %TEMP%），启动时后台
            // 按 mtime 清过期附件（TTL 与回读方的取舍见 chat::PASTE_TTL）。
            chat::spawn_paste_cleanup();
            // 首次启动（新装 / 老用户升级后第一次）自动弹使用引导。延迟一拍让贴纸先画出来，
            // 引导窗口叠在已成型的应用上；看完/关闭时前端置 onboarding_seen=true，之后只手动打开。
            if !crate::settings::load_settings().onboarding_seen {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    window::open_onboarding_window(&handle);
                });
            }
            Ok(())
        })
        .build({
            #[allow(unused_mut)]
            let mut context = tauri::generate_context!();
            // dev 构建独立 identifier：与安装版共 identifier 时，WebView2 用户数据目录
            // （%LOCALAPPDATA%\<identifier>\EBWebView）与 window-state 落盘是同一份——
            // 两个应用寄生同一个 WebView2 浏览器进程，dev 热重建的崩溃/重启会连带杀死
            // 安装版的 webview（实拍：贴纸窗 JS 全灭，托盘「找回贴纸」失灵，重启才恢复）；
            // 窗口位置/吸附状态也互相污染。数据目录（~/.meowo）刻意仍共享（用户要求
            // 不分两份数据）——这里隔离的只是「壳」：webview 配置、窗口状态、单实例键。
            #[cfg(debug_assertions)]
            {
                context.config_mut().identifier = "com.larrygogo.meowo.dev".into();
            }
            context
        })
        .expect("error while running tauri application")
        .run(move |app, event| {
            // 托管 PTY 的子进程不随 GUI 一起死：不显式收尾就会被孤儿化（Windows 上 conhost
            // 一并残留）。同时删掉 approval-broker.json，否则下一个 reporter 会拿着已失效的
            // 端点去连一个可能已被回收的端口。
            if matches!(event, tauri::RunEvent::Exit) {
                exit_ptys.shutdown();
                // 收尾之后再装：安装器会杀残留进程，先把 PTY 善后做完。
                install_downloaded_on_exit(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{
        is_safe_id, live_sessions_blocking, session_connected, tab_class, LiveContext, PageReq,
        SessionRuntimeIndex, RESUME_GRACE_MS,
    };
    use crate::install::{bump_login_epoch, login_epoch};
    use crate::terminal::{
        normalize_tab_title, parse_wt_default_profile, path_has_exe, resume_argv_for,
        shell_join_for_windows, strip_jsonc_comments, tab_match_score,
    };
    use crate::watch::{
        is_viewing_session, pending_fingerprint, screen_blocked_fingerprint, should_notify,
        suppressed_by_viewing, waiting_fingerprint, NotifyKind,
    };

    /// 通知抑制的非对称：用户正看着某会话时不弹它的「等待你回复」（他就在屏幕前），
    /// 但要人做决定的通知（错误/待审批/屏幕阻塞）**永不抑制**——抑制它等于把需要
    /// 决策的事悄悄藏起来。窗口没聚焦则一律不算「正在看」。
    #[test]
    fn viewing_suppresses_only_the_completion_notification() {
        let viewed: std::collections::HashSet<i64> = [7].into_iter().collect();
        // 聚焦 + 停在该会话 = 正在看；换个会话或窗口失焦都不算。
        assert!(is_viewing_session(true, &viewed, 7));
        assert!(!is_viewing_session(true, &viewed, 8));
        assert!(!is_viewing_session(false, &viewed, 7));

        // 正在看：只有「等待你回复」被抑制。
        assert!(suppressed_by_viewing(NotifyKind::Waiting, true));
        for kind in [NotifyKind::Error, NotifyKind::Pending, NotifyKind::Blocked] {
            assert!(
                !suppressed_by_viewing(kind, true),
                "{kind:?} 要人做决定，正在看也必须弹"
            );
        }
        // 没在看：什么都不抑制。
        assert!(!suppressed_by_viewing(NotifyKind::Waiting, false));
    }

    /// 屏幕阻塞通知的让位与去重：错误/已有 pending_review 时不发（那两条各自会发），
    /// 同一次阻塞只发一条，状态离开 blocked 后清空条目、下次阻塞重新通知。
    #[test]
    fn screen_blocked_notification_defers_and_dedupes() {
        // 让位：错误优先、已有 hook 事件驱动的待审批时不重复打扰。
        assert_eq!(screen_blocked_fingerprint(true, false, Some("blocked")), None);
        assert_eq!(screen_blocked_fingerprint(false, true, Some("blocked")), None);
        // 非 blocked 状态不发。
        assert_eq!(screen_blocked_fingerprint(false, false, Some("working")), None);
        assert_eq!(screen_blocked_fingerprint(false, false, None), None);
        // blocked：发，且指纹稳定 —— 同一次阻塞期间不重复弹（agent 不产出、时间戳不动，
        // 指纹刻意不掺 last_event_at，见函数文档）。
        let first = screen_blocked_fingerprint(false, false, Some("blocked"));
        assert!(first.is_some());
        assert!(!should_notify(first.as_deref(), first.as_deref()));
        // 阻塞解除 → None（调用方据此清条目），再次阻塞时 prev 为空 → 重新通知。
        assert_eq!(screen_blocked_fingerprint(false, false, Some("idle")), None);
        assert!(should_notify(None, first.as_deref()));
    }

    /// 宿主侧不得按 agent 身份分支：能力差异一律走 `AgentPlugin` 的能力槽，
    /// 由插件声明（registry.rs 的既定原则「加 agent 只动 plugins/」）。
    ///
    /// 这条断言扫的是**宿主自己的源码**，历史上清理过两批：`detect.rs` 的
    /// `match provider`（改为插件声明屏幕规则）与 `terminal.rs` 的 `== "claude"`
    /// （改为插件声明跨账号会话迁移）。硬编码回潮不会让任何测试失败、也不会有功能
    /// 报错——只会让下一个 agent 悄悄少一块能力，所以只能靠这里守。
    ///
    /// 注释与测试代码不算（写具体 agent 名是正常的举例与固件）。
    #[test]
    fn host_code_does_not_branch_on_agent_identity() {
        // 全量名单：宿主 src/ 下所有模块（含 macOS 专属——include_str 只读文本，不参与
        // cfg 编译，Windows 上照样扫得到）。曾只列 6 个文件，`bgpty.rs`/`relay.rs` 等都在
        // 盲区；实测扩列时全部文件的生产段本就干净（字面量都在各自测试模块里），说明
        // 纪律已内化——守卫的作用是防回潮。新增 src/*.rs 记得补这里（漏补不会红，但该
        // 文件就不设防了）。lib.rs 自身也在列：production_lines 会剔除其内联测试模块。
        const HOST_SOURCES: &[(&str, &str)] = &[
            ("app_bundle.rs", include_str!("app_bundle.rs")),
            ("bgpty.rs", include_str!("bgpty.rs")),
            ("chat.rs", include_str!("chat.rs")),
            ("confirm.rs", include_str!("confirm.rs")),
            ("detect.rs", include_str!("detect.rs")),
            ("envpath.rs", include_str!("envpath.rs")),
            ("install.rs", include_str!("install.rs")),
            ("lib.rs", include_str!("lib.rs")),
            ("managed_terminal.rs", include_str!("managed_terminal.rs")),
            ("proc.rs", include_str!("proc.rs")),
            ("proxy.rs", include_str!("proxy.rs")),
            ("pty.rs", include_str!("pty.rs")),
            ("relay.rs", include_str!("relay.rs")),
            ("session_command.rs", include_str!("session_command.rs")),
            ("session_query.rs", include_str!("session_query.rs")),
            ("settings.rs", include_str!("settings.rs")),
            ("snap.rs", include_str!("snap.rs")),
            ("snap_layout.rs", include_str!("snap_layout.rs")),
            ("term_script.rs", include_str!("term_script.rs")),
            ("terminal.rs", include_str!("terminal.rs")),
            ("watch.rs", include_str!("watch.rs")),
            ("wezterm.rs", include_str!("wezterm.rs")),
            ("window.rs", include_str!("window.rs")),
            ("account/mod.rs", include_str!("account/mod.rs")),
            ("profile/mod.rs", include_str!("profile/mod.rs")),
            ("setup/mod.rs", include_str!("setup/mod.rs")),
            ("macos/menubar.rs", include_str!("macos/menubar.rs")),
            ("macos/mod.rs", include_str!("macos/mod.rs")),
            ("macos/notify.rs", include_str!("macos/notify.rs")),
            ("macos/panel.rs", include_str!("macos/panel.rs")),
            ("macos/terminal.rs", include_str!("macos/terminal.rs")),
        ];
        for (name, source) in HOST_SOURCES {
            for (index, line) in production_lines(source) {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue; // 注释里举例说明是正常的
                }
                for id in ["claude", "codex", "kimi", "gemini", "opencode"] {
                    assert!(
                        !code.contains(&format!("\"{id}\"")),
                        "{name}:{} 按 agent 身份分支了（{id}）——该能力应走 AgentPlugin 的能力槽：\n  {code}",
                        index + 1
                    );
                }
            }
        }
    }

    /// 逐行产出源码的**生产段**（跳过 `#[cfg(test)]` 模块的整个花括号区间），
    /// 附带 1 起始的行号。
    ///
    /// 曾经用 `split("#[cfg(test)]").next()` 一刀切到首个测试标记为止——那假设测试都在
    /// 文件末尾。`terminal.rs` 的内联测试模块在 31% 处，其后还有 43 个顶层定义全部
    /// 逃过检查（实测：在其后植入 `provider == "claude"`，守卫照样通过）。守卫的盲区
    /// 比没有守卫更危险：它让人以为已经防住了。
    fn production_lines(source: &str) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        let mut depth = 0i32; // 测试模块内的花括号深度；0 = 不在测试模块里
        let mut in_test = false;
        for (index, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            // `#[test]` 也作触发器：字符串字面量里的花括号会让深度计数失衡、提前退出
            // 跳过区（lib.rs 自扫时实测踩到），此后遇到下一个测试函数的 `#[test]` 即
            // 重新进入跳过——生产代码不会出现该属性，误触发不存在。
            if !in_test
                && (trimmed.starts_with("#[cfg(test)]")
                    || trimmed.starts_with("#[cfg(all(test")
                    || trimmed.starts_with("#[test]"))
            {
                in_test = true;
                depth = 0;
                continue;
            }
            if in_test {
                // 数花括号找模块结束。字符串/字符字面量里的花括号会干扰计数，但测试
                // 模块里的括号总体平衡，实践中足够；真出偏差会表现为**多**跳过一段，
                // 那是安全方向（守卫更严只会误报，不会漏报）。
                depth += line.matches('{').count() as i32;
                depth -= line.matches('}').count() as i32;
                if depth <= 0 && line.contains('}') {
                    in_test = false;
                }
                continue;
            }
            out.push((index + 1, line));
        }
        out
    }

    /// DLL 搜索路径收紧必须在 `run()` 的**最前面**（任何 LoadLibrary 之前）——托管 PTY
    /// 的 portable-pty 用相对名 sideload `conpty.dll`，cwd 在搜索顺序里（理由详见
    /// `harden_dll_search_path` 的文档）。删掉或往后挪这一行会静默恢复劫持面：没有任何
    /// 功能会因此失败，所以只能靠这条断言守住。
    #[test]
    fn dll_search_path_is_hardened_before_anything_loads() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub fn run() {")
            .expect("run() 必须存在")
            .1;
        let harden = body
            .find("harden_dll_search_path()")
            .expect("run() 必须调用 harden_dll_search_path()");
        // 允许它前面只有 macOS 的 bundle 迁移（那段在任何 DLL 加载之前 return/no-op）。
        let migrate_data = body.find("migrate_legacy_data()").expect("migrate_legacy_data");
        assert!(
            harden < migrate_data,
            "DLL 搜索路径收紧必须早于任何会加载 DLL 的初始化"
        );
    }

    #[test]
    fn waiting_page_skips_disconnected_sql_batches_without_ending_early() {
        let path = std::env::temp_dir().join(format!(
            "meowo-filter-page-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = meowo_store::Store::open(&path).unwrap();
        let project = store.upsert_project_by_root("/p", "p", 1).unwrap();

        // waiting 按最旧优先：先放 120 条断开候选，确保超过后端单批 100 条。
        for i in 0..120 {
            let (sid, _) = store
                .start_session(project, &format!("dead-{i}"), 10 + i)
                .unwrap();
            store
                .on_user_prompt(sid, &format!("dead {i}"), 10 + i)
                .unwrap();
            store.set_session_pid(sid, 10_000 + i, 10 + i).unwrap();
            store
                .set_session_status(sid, meowo_store::SessionStatus::Waiting, 10 + i)
                .unwrap();
        }
        let mut alive = std::collections::HashSet::new();
        for i in 0..2 {
            let (sid, _) = store
                .start_session(project, &format!("alive-{i}"), 1_000 + i)
                .unwrap();
            store
                .on_user_prompt(sid, &format!("alive {i}"), 1_000 + i)
                .unwrap();
            let pid = 20_000 + i;
            store.set_session_pid(sid, pid, 1_000 + i).unwrap();
            store
                .set_session_status(sid, meowo_store::SessionStatus::Waiting, 1_000 + i)
                .unwrap();
            alive.insert(pid);
        }
        drop(store);

        let cache = std::sync::Mutex::new(meowo_agent::TranscriptCache::new());
        let pty_none = std::collections::HashSet::new();
        let no_runtimes = SessionRuntimeIndex::new();
        let no_approvals = std::collections::HashSet::new();
        let no_screens = std::collections::HashMap::new();
        let page = live_sessions_blocking(
            &path,
            &cache,
            LiveContext {
                alive: &alive,
                pty_live: &pty_none,
                runtimes: &no_runtimes,
                approvals: &no_approvals,
                screen_states: &no_screens,
            },
            "waiting",
            None,
            None,
            PageReq {
                before_last_event_at: None,
                before_id: None,
                limit: 2,
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(page.items.iter().all(|l| l.connected));
        assert!(page
            .items
            .iter()
            .all(|l| l.inner.session.cc_session_id.starts_with("alive-")));

        drop(cache);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    /// 代次是「取消登录」的整个机制：watch 线程每轮比对自己出生时的代次，不等就静默退出。
    ///
    /// 两个场景都靠它：用户点取消（代次 +1，旧线程停下且不 emit，收尾 emit 归取消方）；
    /// 用户连点两次登录（第二次把第一个线程也淘汰掉，避免两个 login-done 打架）。
    ///
    /// 用不同 agent 隔离——`LOGIN_EPOCH` 是全局静态，同一 agent 会在测试间串。
    #[test]
    fn login_epoch_invalidates_older_watchers_per_agent() {
        let kimi = meowo_agent::id::KIMI;
        let codex = meowo_agent::id::CODEX;

        // 一个 watch 线程出生时拿到的代次。
        let first = bump_login_epoch(kimi);
        assert_eq!(login_epoch(kimi), first, "刚出生的线程应认得自己这一代");

        // 用户点了取消（或又点了一次登录）：代次前进，老线程下一轮就会看到不等而退出。
        let second = bump_login_epoch(kimi);
        assert!(second > first);
        assert_ne!(login_epoch(kimi), first, "老线程必须发现自己已被取代");
        assert_eq!(login_epoch(kimi), second, "新线程仍然有效");

        // 按 agent 分开：登 kimi 不该把 codex 的等待线程掀掉（两者本就允许并发）。
        let codex_epoch = bump_login_epoch(codex);
        bump_login_epoch(kimi);
        assert_eq!(login_epoch(codex), codex_epoch, "kimi 的登录不该影响 codex");
    }

    /// `bump` 严格递增，且每次 bump 后当前代次就等于它的返回值。
    ///
    /// 不断言「没碰过的 agent 代次为 0」——`LOGIN_EPOCH` 是进程内全局静态，测试并行跑，
    /// 那样的断言会取决于哪个测试先摸了 claude。这里只测不依赖初值的性质。
    #[test]
    fn bump_login_epoch_is_strictly_increasing() {
        let claude = meowo_agent::id::CLAUDE;
        let a = bump_login_epoch(claude);
        let b = bump_login_epoch(claude);
        let c = bump_login_epoch(claude);
        assert!(a < b && b < c, "代次必须严格递增：{a} {b} {c}");
        assert_eq!(login_epoch(claude), c);
    }

    /// 拉起 agent 的 env **必须**带上账号隔离变量，否则多账号完全不生效。
    ///
    /// 回归：`new_session` 曾直接调 `proxy::launch_env`（只有代理变量），于是设置页明明切到了另一个
    /// 账号，新开的会话却仍跑在默认账号上——没有任何报错，用户只能靠 `/status` 里的邮箱才发现。
    /// 新建会话是切换账号后**最先走的一条路**，漏了它等于整个功能没做。
    ///
    /// 这条钉的是 `launch_env_for_profile` 的产出。它**抓不到「有人再次绕过它」**——那只能靠
    /// `proxy::launch_env` 上那段警告和 review。
    #[test]
    fn launch_env_carries_profile_isolation_vars() {
        use crate::terminal::launch_env_for_profile;

        let env = launch_env_for_profile(Some("claude"), Some("work"));
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            keys.contains(&"CLAUDE_CONFIG_DIR"),
            "漏了隔离变量 → 切了账号也不生效，实得 {keys:?}"
        );
        // 会话据此绑定到该账号（reporter 继承这个变量后写进 sessions.profile）。
        assert!(
            keys.contains(&"MEOWO_PROFILE"),
            "会话将无从绑定账号，实得 {keys:?}"
        );

        // opencode 必须拿到**两个**目录变量：只隔离配置目录的话，凭据仍然共用——
        // 账号看起来切了、其实没切，这是最坏的一种失败。
        let env = launch_env_for_profile(Some("opencode"), Some("work"));
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"OPENCODE_CONFIG_DIR"), "实得 {keys:?}");
        assert!(
            keys.contains(&"XDG_DATA_HOME"),
            "凭据所在的数据目录没隔离，实得 {keys:?}"
        );

        // gemini 不支持多账号（数据目录不可被环境变量覆盖）→ 一个隔离变量都不该注入。
        // 只注入 MEOWO_PROFILE 而不隔离目录，会把一个跑在**默认账号**上的会话记成 profile 的。
        let env = launch_env_for_profile(Some("gemini"), Some("work"));
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            !keys.contains(&"MEOWO_PROFILE"),
            "gemini 不支持多账号，实得 {keys:?}"
        );
    }

    /// `resume_argv_for` 只被 macOS 的 focus_session 调用，Windows 上没有调用者——光「能编译」
    /// 不足以防它腐化（dead_code 允许了它）。这里在所有平台实际调它一次，锁住行为：
    /// 已知 agent 给出 `[exe, --resume, id]`，未知/缺 session_id 给空 argv（只聚焦、不 resume）。
    #[test]
    fn resume_argv_for_dispatches_by_provider_and_degrades_safely() {
        let argv = resume_argv_for(Some("claude"), Some("SID"));
        assert_eq!(
            &argv[argv.len() - 2..],
            ["--resume".to_string(), "SID".to_string()]
        );
        assert!(argv[0].to_ascii_lowercase().contains("claude"));

        let kimi = resume_argv_for(Some("kimi"), Some("SID"));
        assert_eq!(
            &kimi[kimi.len() - 2..],
            ["-r".to_string(), "SID".to_string()]
        );

        // 新接入的两家，各自的 resume 子命令都得对上。
        let gemini = resume_argv_for(Some("gemini"), Some("SID"));
        assert_eq!(
            &gemini[gemini.len() - 2..],
            ["--resume".to_string(), "SID".to_string()]
        );
        let opencode = resume_argv_for(Some("opencode"), Some("SID"));
        assert_eq!(
            &opencode[opencode.len() - 2..],
            ["--session".to_string(), "SID".to_string()]
        );

        // 未知 agent → 空 argv：绝不拿 claude 的参数去拉起别的 CLI。反例必须选一个永远不会被
        // 注册的串——这里原本写的是 "gemini"，而它后来真成了一个 agent。
        assert!(resume_argv_for(Some("not-an-agent"), Some("SID")).is_empty());
        // 没有 session_id 就无从 resume。
        assert!(resume_argv_for(Some("claude"), None).is_empty());
        // provider 缺省 → 默认 agent（老会话没写过 provider 列）。
        assert!(!resume_argv_for(None, Some("SID")).is_empty());
    }
    use crate::settings::Settings;
    use crate::snap::{center_on, clamp_xy_to_work, edge_for_rect, intersection_area, Edge, Rect};

    const WORK1: Rect = Rect {
        x: 0,
        y: 0,
        w: 2556,
        h: 1179,
    };

    #[cfg(target_os = "windows")]
    #[test]
    fn tray_tooltip_text_localizes_and_orders_waiting_first() {
        use crate::window::tray_tooltip_text;
        // 入参顺序：(lang, running, waiting)。待交互更紧急，排在运行中之前。
        assert_eq!(tray_tooltip_text("zh", 0, 0), "Meowo");
        assert_eq!(
            tray_tooltip_text("zh", 2, 3),
            "Meowo · 3 个待交互 · 2 个运行中"
        );
        assert_eq!(tray_tooltip_text("zh", 2, 0), "Meowo · 2 个运行中");
        assert_eq!(tray_tooltip_text("en", 0, 2), "Meowo · 2 waiting");
        assert_eq!(
            tray_tooltip_text("en", 1, 1),
            "Meowo · 1 waiting · 1 running"
        );
    }

    #[test]
    fn shell_join_quotes_spaced_paths_for_powershell_and_cmd() {
        let to_vec = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // 无空格（claude）：原样拼接，两种 shell 一致。
        let plain = to_vec(&["claude", "--resume", "ID"]);
        assert_eq!(shell_join_for_windows(&plain, true), "claude --resume ID");
        assert_eq!(shell_join_for_windows(&plain, false), "claude --resume ID");
        // 可执行绝对路径含空格（kimi）：PowerShell 用单引号字面量 + & 调用运算符（双引号内 $/` 会被
        // 插值展开，单引号内一切按字面），cmd 用双引号。
        let spaced = to_vec(&[
            r"C:\Users\First Last\.kimi-code\bin\kimi.exe",
            "-r",
            "session_x",
        ]);
        assert_eq!(
            shell_join_for_windows(&spaced, true),
            r"& 'C:\Users\First Last\.kimi-code\bin\kimi.exe' -r session_x"
        );
        assert_eq!(
            shell_join_for_windows(&spaced, false),
            r#""C:\Users\First Last\.kimi-code\bin\kimi.exe" -r session_x"#
        );
        // node 包装（codex）：命令名无空格、脚本路径参数有空格 → 只 quote 参数，PowerShell 不需要 &。
        let node = to_vec(&[
            "node",
            r"C:\Users\First Last\AppData\Roaming\npm\codex.js",
            "resume",
            "ID",
        ]);
        assert_eq!(
            shell_join_for_windows(&node, true),
            r"node 'C:\Users\First Last\AppData\Roaming\npm\codex.js' resume ID"
        );
        // 用户名含 $（合法字符）：无空格也要单引号包裹，否则 PowerShell 变量插值把路径吞掉。
        let dollar = to_vec(&[r"C:\Users\a$b\.kimi-code\bin\kimi.exe", "-r", "id"]);
        assert_eq!(
            shell_join_for_windows(&dollar, true),
            r"& 'C:\Users\a$b\.kimi-code\bin\kimi.exe' -r id"
        );
        // 路径含单引号（如 O'Brien）：内嵌单引号翻倍。
        let apos = to_vec(&[r"C:\Users\O'Brien\kimi.exe", "-r", "id"]);
        assert_eq!(
            shell_join_for_windows(&apos, true),
            r"& 'C:\Users\O''Brien\kimi.exe' -r id"
        );
        // PowerShell 元字符与 JSON 双引号即便没有空格也必须进入单引号字面量。
        let metachar = to_vec(&[r"C:\Users\A&B\kimi.exe", r#"model=\"x\";calc"#]);
        assert_eq!(
            shell_join_for_windows(&metachar, true),
            r#"& 'C:\Users\A&B\kimi.exe' 'model=\"x\";calc'"#
        );
    }

    #[test]
    fn path_has_exe_scans_path_dirs_without_spawning() {
        let dir = std::env::temp_dir().join("meowo-test-path-has-exe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("wt.exe"), b"stub").unwrap();
        // 单目录命中 / 未命中
        let single = std::env::join_paths([dir.clone()]).unwrap();
        assert!(path_has_exe(&single, "wt.exe"));
        assert!(!path_has_exe(&single, "definitely-absent.exe"));
        // 多目录：前面的目录不存在也不影响后面命中
        let multi =
            std::env::join_paths([std::env::temp_dir().join("meowo-no-such-dir"), dir.clone()])
                .unwrap();
        assert!(path_has_exe(&multi, "wt.exe"));
        // 空 PATH → 找不到
        assert!(!path_has_exe(std::ffi::OsStr::new(""), "wt.exe"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 断开的会话既不「运行中」也不「待交互」——进程都没了，催用户去交互毫无意义。
    /// 这是列表过滤与角标计数**共用**的唯一判定，改坏了两处一起错。
    #[test]
    fn tab_class_excludes_disconnected_sessions() {
        // 连着：三类各归其位。
        assert_eq!(tab_class(true, "running", None, None, || false), Some("running"));
        assert_eq!(tab_class(true, "waiting", None, None, || false), Some("waiting"));
        // pending_review 压过 status：正在等用户批准，不算「自主运行中」。
        assert_eq!(
            tab_class(true, "running", Some("approval"), None, || false),
            Some("waiting")
        );

        // 断开：一律不进这两个 tab，只作为历史留在「全部」里。
        // 尤其是这条——DB 里残留的 pending_review 曾让断开的会话挂在「待交互」里催人，
        // 点进去却只是个死掉的历史会话（兼容旧数据库中的 pending_review 残留）。
        assert_eq!(tab_class(false, "running", Some("approval"), None, || false), None);
        assert_eq!(tab_class(false, "waiting", None, None, || false), None);
        assert_eq!(tab_class(false, "running", None, None, || false), None);

        // 已结束/其它状态：本就不属于这两类。
        assert_eq!(tab_class(true, "ended", None, None, || false), None);
    }

    /// tab 归属与卡片状态环同源：屏幕检测（实时）压过 DB status（hook 驱动、滞后）。
    /// 原始症状（实拍反馈）：status=running 而屏幕已 idle 的会话顶着黄环（等你）
    /// 待在「运行中」tab 里——环说等你、tab 说在跑，用户以为状态坏了。
    #[test]
    fn tab_class_lets_screen_state_override_db_status() {
        // 屏幕说了算：idle/blocked → 待交互，working → 运行中，无论 DB status 停在哪。
        assert_eq!(
            tab_class(true, "running", None, Some("idle"), || false),
            Some("waiting")
        );
        assert_eq!(
            tab_class(true, "running", None, Some("blocked"), || false),
            Some("waiting")
        );
        assert_eq!(
            tab_class(true, "waiting", None, Some("working"), || false),
            Some("running")
        );
        // 审批（校正后仍在压着的）最优先——那是事实源，屏幕规则可能漏报 blocked。
        assert_eq!(
            tab_class(true, "running", Some("approval"), Some("working"), || false),
            Some("waiting")
        );
        // 屏幕状态对 ended 也生效前提是 connected；ended + 屏幕 working 属于恢复窗口，
        // 由 PTY 存活判 connected，这里归运行中，与卡片绿环一致。
        assert_eq!(
            tab_class(true, "ended", None, Some("working"), || false),
            Some("running")
        );
        // 断开永远优先于一切。
        assert_eq!(tab_class(false, "running", None, Some("idle"), || false), None);
    }

    /// 主回合停了(DB waiting / 屏幕 idle)但后台子任务还在跑 → 归「运行中」,不催人。
    /// 实拍反馈:委派了后台审查 agent 的会话在等待区躺了半小时,其实一直在干活。
    /// 但真要人的信号(审批/屏幕 blocked)不被后台忙碌掩盖——批准不点,后台跑完也白等。
    #[test]
    fn tab_class_busy_subagents_keep_session_running() {
        assert_eq!(
            tab_class(true, "waiting", None, None, || true),
            Some("running")
        );
        assert_eq!(
            tab_class(true, "running", None, Some("idle"), || true),
            Some("running")
        );
        // 审批与屏幕 blocked 仍是待交互——那是用户必须处理的。
        assert_eq!(
            tab_class(true, "waiting", Some("approval"), None, || true),
            Some("waiting")
        );
        assert_eq!(
            tab_class(true, "running", None, Some("blocked"), || true),
            Some("waiting")
        );
        // 断开的会话谈不上后台忙碌(子任务与主进程同生共死)。
        assert_eq!(tab_class(false, "waiting", None, None, || true), None);
    }

    #[test]
    fn agent_pids_snapshot_rejects_non_agent_processes() {
        // 当前测试进程存在但不叫 claude → 不算连接（pid 复用防护）；
        // 非法 pid 天然不在集合里。这是存活轮询与看板连接判定共用的事实源。
        let pids = crate::proc::agent_pids_snapshot();
        assert!(!pids.contains(&(std::process::id() as i64)));
        assert!(!pids.contains(&0));
        assert!(!pids.contains(&-1));
    }

    #[test]
    fn session_connected_logic() {
        let now = 1_000_000i64;
        // 结束 + 实证存活(进程表命中/托管 PTY 活跃)→ 连接:恢复会话的窗口期 DB 还挂着
        // 旧 'ended' 而 PTY 已在跑,不让位就会「已结束」徽标配「结束会话」按钮同框。
        assert!(session_connected("ended", Some(123), true, now, now));
        // 结束 + 无实证存活 → 断开,事件宽限**不适用**:刚结束的会话天然在宽限期内,
        // 套用宽限会让所有 ended 会话诈尸 120 秒。
        assert!(!session_connected("ended", Some(123), false, now, now));
        // 活着的 agent 进程 → 连接（与时间无关）。
        assert!(session_connected("running", Some(123), true, 0, now));
        // pid 已认领但此刻没校验成存活 + 刚活动过 → 连接（新建会话：SessionStart 已 set_session_pid，
        // 但 app 进程快照尚未收录该 pid；不该因 pid 非空就丢掉宽限而瞬间判断开沉底）。
        assert!(session_connected(
            "running",
            Some(123),
            false,
            now - 1_000,
            now
        ));
        // pid 已认领但已死/被复用 + 早已不活动 → 断开（pid 复用僵尸会话，靠「近期无活动」兜底）。
        assert!(!session_connected(
            "running",
            Some(123),
            false,
            now - RESUME_GRACE_MS - 1,
            now
        ));
        // pid 未知 + 在 resume 宽限期内 → 连接（刚 resume，等 codex 首个 hook）。
        assert!(session_connected("running", None, false, now - 1_000, now));
        assert!(session_connected("waiting", None, false, now - 1_000, now));
        // pid 未知 + 超出宽限期 → 断开（终端没起来/被关的僵尸会话，不再假连接）。
        assert!(!session_connected(
            "running",
            None,
            false,
            now - RESUME_GRACE_MS - 1,
            now
        ));
    }

    #[test]
    fn intersection_area_overlap_and_disjoint() {
        let win = Rect {
            x: 100,
            y: 100,
            w: 400,
            h: 300,
        };
        assert_eq!(intersection_area(win, WORK1), 400 * 300); // 完全在内
                                                              // 完全在屏外（第二屏被拔掉的旧坐标）
        let off = Rect {
            x: 3000,
            y: 200,
            w: 400,
            h: 300,
        };
        assert_eq!(intersection_area(off, WORK1), 0);
        // 部分相交
        let partial = Rect {
            x: 2400,
            y: 0,
            w: 400,
            h: 300,
        };
        assert_eq!(intersection_area(partial, WORK1), (2556 - 2400) * 300);
    }

    #[test]
    fn clamp_brings_offscreen_window_fully_in() {
        // 在屏右外 → 钳到右边界内（x = 2556 - 400）
        let off = Rect {
            x: 3000,
            y: 200,
            w: 400,
            h: 300,
        };
        assert_eq!(clamp_xy_to_work(off, WORK1), (2556 - 400, 200));
        // 负坐标（屏左上外）→ 钳到原点
        let neg = Rect {
            x: -50,
            y: -30,
            w: 400,
            h: 300,
        };
        assert_eq!(clamp_xy_to_work(neg, WORK1), (0, 0));
        // 已在屏内 → 不动
        let inside = Rect {
            x: 100,
            y: 100,
            w: 400,
            h: 300,
        };
        assert_eq!(clamp_xy_to_work(inside, WORK1), (100, 100));
    }

    #[test]
    fn clamp_window_larger_than_work_aligns_origin() {
        let big = Rect {
            x: 500,
            y: 500,
            w: 3000,
            h: 2000,
        };
        assert_eq!(clamp_xy_to_work(big, WORK1), (0, 0));
    }

    #[test]
    fn strip_jsonc_keeps_strings_and_drops_comments() {
        let src = r#"{
          // 行注释
          "defaultProfile": "{guid}", /* 块注释 */
          "url": "https://example.com/a//b",
          "name": "含 // 的中文 \" 引号"
        }"#;
        let v: serde_json::Value = serde_json::from_str(&strip_jsonc_comments(src)).unwrap();
        assert_eq!(v["defaultProfile"], "{guid}");
        assert_eq!(v["url"], "https://example.com/a//b"); // 字符串里的 // 不能被当注释删掉
        assert_eq!(v["name"], "含 // 的中文 \" 引号"); // 多字节 UTF-8 与转义引号保留
    }

    #[test]
    fn wt_default_profile_resolves_guid_to_name() {
        // GUID 大小写不敏感匹配到 name。
        let v = serde_json::json!({
            "defaultProfile": "{574E775E-4F2A-5B96-AC1E-A2962A402336}",
            "profiles": { "list": [
                {"guid": "{0caa0dad-35be-5f56-a8ff-afceeeaa6101}", "name": "命令提示符"},
                {"guid": "{574e775e-4f2a-5b96-ac1e-a2962a402336}", "name": "PowerShell"}
            ]}
        });
        assert_eq!(parse_wt_default_profile(&v).as_deref(), Some("PowerShell"));
        // defaultProfile 直接是名字。
        let named = serde_json::json!({"defaultProfile": "Ubuntu"});
        assert_eq!(parse_wt_default_profile(&named).as_deref(), Some("Ubuntu"));
        // 老格式 profiles 为数组。
        let legacy = serde_json::json!({
            "defaultProfile": "{abc}", "profiles": [{"guid": "{abc}", "name": "Legacy"}]
        });
        assert_eq!(parse_wt_default_profile(&legacy).as_deref(), Some("Legacy"));
        // 无匹配 / 缺字段 → None。
        assert!(parse_wt_default_profile(
            &serde_json::json!({"defaultProfile": "{zzz}", "profiles": {"list": []}})
        )
        .is_none());
        assert!(parse_wt_default_profile(&serde_json::json!({})).is_none());
    }

    #[test]
    fn center_on_centers_clamps_and_preserves_center() {
        // 基本居中：300 长里放 60 → 起点 +120。
        assert_eq!(center_on(100, 300, 60, 0, 1000), 220);
        // 右/下越界 → 夹到工作区末尾内。
        assert_eq!(center_on(950, 300, 60, 0, 1000), 940);
        // 左/上越界（负） → 夹到工作区起点。
        assert_eq!(center_on(-50, 100, 60, 0, 1000), 0);
        // 重测一致：换长度后中心不变（220 中心=250 → 新起点 210，中心仍 250）。
        assert_eq!(center_on(220, 60, 80, 0, 1000) + 80 / 2, 220 + 60 / 2);
    }

    #[test]
    fn safe_id_accepts_uuid_and_kimi() {
        // claude 的 UUID 与 kimi 的 session_<uuid> 都应通过（focus/resume/rename/note 共用此校验）。
        assert!(is_safe_id("a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        assert!(is_safe_id("00000000-0000-0000-0000-000000000000"));
        assert!(is_safe_id("session_a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
    }

    #[test]
    fn safe_id_rejects_injection_and_malformed() {
        // 含 shell/wt 元字符、空格、路径分隔符/点 → 拒绝（命令注入 + 路径穿越防护）。
        assert!(!is_safe_id("'; calc; '")); // 注入尝试
        assert!(!is_safe_id("abc --resume x; calc"));
        assert!(!is_safe_id("a1b2c3d4-e5f6-7890-abcd-ef1234567890 ")); // 尾空格
        assert!(!is_safe_id("../../etc/passwd")); // 路径穿越
        assert!(!is_safe_id("a/b")); // 路径分隔符
        assert!(!is_safe_id("a.b")); // 点（穿越/扩展名）
        assert!(!is_safe_id("")); // 空
        assert!(!is_safe_id(&"a".repeat(129))); // 超长 >128
    }

    #[test]
    fn tab_title_strips_spinner_prefix() {
        // claude 写入的标题：状态符号 + 空格 + 任务标题。前缀符号会随状态变化。
        assert_eq!(
            normalize_tab_title("⠐ 修复贴纸窗口跳转"),
            "修复贴纸窗口跳转"
        ); // braille spinner
        assert_eq!(
            normalize_tab_title("✳ 修复贴纸窗口跳转"),
            "修复贴纸窗口跳转"
        ); // 空闲 ✳
        assert_eq!(
            normalize_tab_title("⠙ Allow editing titles"),
            "Allow editing titles"
        );
        // 无前缀也应原样（仅去首尾空白）。
        assert_eq!(normalize_tab_title("  纯标题  "), "纯标题");
        // 尾部截断省略号应去掉。
        assert_eq!(normalize_tab_title("✳ 修复贴纸窗口…"), "修复贴纸窗口");
    }

    #[test]
    fn tab_match_exact_after_normalize() {
        // 不论前缀是 spinner 还是 ✳，剥离后都应精确命中(=2)，这是修「时好时坏」的关键。
        assert_eq!(tab_match_score("⠐ 修复贴纸窗口跳转", "修复贴纸窗口跳转"), 2);
        assert_eq!(tab_match_score("✳ 修复贴纸窗口跳转", "修复贴纸窗口跳转"), 2);
        assert_eq!(tab_match_score("修复贴纸窗口跳转", "修复贴纸窗口跳转"), 2);
    }

    #[test]
    fn tab_match_contains_is_weaker() {
        // 标签页标题含会话标题但不完全相等（如 claude 追加了后缀）→ 弱匹配。
        assert_eq!(
            tab_match_score("⠐ 修复贴纸窗口跳转 - done", "修复贴纸窗口跳转"),
            1
        );
        // 长标题被 claude 截断：tab 标题是 want 的前缀 → 双向包含命中(=1)。
        assert_eq!(
            tab_match_score(
                "✳ 修复贴纸连接中会话窗口…",
                "修复贴纸连接中会话窗口跳转问题"
            ),
            1
        );
    }

    #[test]
    fn tab_match_no_match() {
        assert_eq!(tab_match_score("npm run build", "修复贴纸窗口跳转"), 0);
    }

    #[test]
    fn tab_match_empty_or_unnamed_never_matches() {
        // 空标题/未命名占位不参与匹配，避免误命中任意标签页。
        assert_eq!(tab_match_score("⠐ 任意标题", ""), 0);
        assert_eq!(tab_match_score("⠐ 任意标题", "  "), 0);
        assert_eq!(tab_match_score("⠐ (未命名会话)", "(未命名会话)"), 0);
    }

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1040,
    };

    // L/R 用例统一用 y=400（远离顶部），避免被顶部判定干扰。
    #[test]
    fn left_within_threshold() {
        let win = Rect {
            x: 5,
            y: 400,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Left));
    }

    #[test]
    fn right_within_threshold() {
        let win = Rect {
            x: 1920 - 300 - 5,
            y: 400,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Right));
    }

    #[test]
    fn top_within_threshold() {
        let win = Rect {
            x: 800,
            y: 8,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Top));
    }

    #[test]
    fn center_is_none() {
        let win = Rect {
            x: 800,
            y: 400,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, WORK, 20), None);
    }

    #[test]
    fn threshold_boundary_inclusive() {
        let win = Rect {
            x: 20,
            y: 400,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Left));
    }

    #[test]
    fn just_outside_threshold_none() {
        let win = Rect {
            x: 21,
            y: 400,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, WORK, 20), None);
    }

    #[test]
    fn picks_nearer_edge() {
        // 左距 5 < 右距 10，y 远离顶部 → 取左。
        let work = Rect {
            x: 0,
            y: 0,
            w: 320,
            h: 1040,
        };
        let win = Rect {
            x: 5,
            y: 400,
            w: 305,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, work, 20), Some(Edge::Left));
    }

    #[test]
    fn top_nearer_than_left() {
        // 左上角附近：顶距 3 < 左距 10 → 取顶。
        let win = Rect {
            x: 10,
            y: 3,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Top));
    }

    #[test]
    fn respects_work_area_offset() {
        let work = Rect {
            x: 100,
            y: 0,
            w: 1000,
            h: 1040,
        };
        let win = Rect {
            x: 110,
            y: 400,
            w: 300,
            h: 400,
        };
        assert_eq!(edge_for_rect(win, work, 20), Some(Edge::Left));
    }

    #[test]
    fn should_notify_only_on_new_error() {
        assert!(!should_notify(None, None)); // 无错 → 不弹
        assert!(should_notify(None, Some("a"))); // 新错 → 弹
        assert!(!should_notify(Some("a"), Some("a"))); // 同一错误 → 不弹
        assert!(should_notify(Some("a"), Some("b"))); // 换了新错误 → 弹
        assert!(!should_notify(Some("a"), None)); // 错误消失 → 不弹（由清除处理）
    }

    #[test]
    fn pending_fingerprint_rules() {
        // errored 优先 → None(让位错误)。
        assert_eq!(pending_fingerprint(true, Some("approval"), 100), None);
        // pending 为 Some 且未出错 → Some("{kind}:{last_event_at}")。
        assert_eq!(
            pending_fingerprint(false, Some("question"), 100).as_deref(),
            Some("question:100")
        );
        // 无 pending → None。
        assert_eq!(pending_fingerprint(false, None, 100), None);
        // 指纹随 last_event_at 变化(新回合新指纹)。
        assert_ne!(
            pending_fingerprint(false, Some("approval"), 100),
            pending_fingerprint(false, Some("approval"), 200)
        );
    }

    #[test]
    fn waiting_fingerprint_rules() {
        // 错误优先:无指纹。
        assert_eq!(waiting_fingerprint(true, false, "waiting", 100), None);
        // pending 优先:无 waiting 指纹(让位 pending)。
        assert_eq!(waiting_fingerprint(false, true, "waiting", 100), None);
        // 纯 waiting:用 last_event_at 作指纹。
        assert_eq!(
            waiting_fingerprint(false, false, "waiting", 100).as_deref(),
            Some("100")
        );
        // 非 waiting 状态:None。
        assert_eq!(waiting_fingerprint(false, false, "running", 100), None);
    }

    #[test]
    fn settings_defaults_notifications_on() {
        // 空文件 / 老文件缺字段 → 默认开启（向后兼容）
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert!(empty.notifications_enabled);
        // archive_hide_days 已下线；老文件残留该字段应被容忍而非报错。
        let legacy: Settings = serde_json::from_str(r#"{"archive_hide_days":7}"#).unwrap();
        assert!(legacy.notifications_enabled);
        // 显式关闭可被尊重
        let off: Settings = serde_json::from_str(r#"{"notifications_enabled":false}"#).unwrap();
        assert!(!off.notifications_enabled);
        // 整文件缺失/解析失败时用 Default，也应为 ON
        assert!(Settings::default().notifications_enabled);
    }

    #[test]
    fn settings_appearance_defaults_and_back_compat() {
        // 老文件缺外观字段 → 用缺省（dark / 100 / 100），不报错。
        let legacy: Settings = serde_json::from_str(r#"{"archive_hide_days":7}"#).unwrap();
        assert_eq!(legacy.theme, "dark");
        assert_eq!(legacy.opacity, 100);
        assert_eq!(legacy.ui_scale, 100);
        // 显式外观值被尊重。
        let custom: Settings =
            serde_json::from_str(r#"{"theme":"light","opacity":80,"ui_scale":112}"#).unwrap();
        assert_eq!(custom.theme, "light");
        assert_eq!(custom.opacity, 80);
        assert_eq!(custom.ui_scale, 112);
        // Default 与缺省函数一致。
        let d = Settings::default();
        assert_eq!(d.theme, "dark");
        assert_eq!(d.opacity, 100);
        assert_eq!(d.ui_scale, 100);
    }
}

#[cfg(test)]
mod new_session_tests {
    use crate::terminal::validate_new_session_cwd;

    #[test]
    fn validate_cwd_rejects_empty_and_missing() {
        assert!(validate_new_session_cwd("").is_err());
        assert!(validate_new_session_cwd("   ").is_err());
        assert!(validate_new_session_cwd("C:/definitely/not/a/real/dir/xyz123").is_err());
    }

    #[test]
    fn validate_cwd_accepts_existing_dir() {
        let tmp = std::env::temp_dir();
        let got = validate_new_session_cwd(tmp.to_str().unwrap()).unwrap();
        assert_eq!(got, tmp.to_str().unwrap().trim());
    }
}

#[cfg(test)]
mod hooks_check_tests {
    use crate::install::{hooks_status_at, HooksStatus};

    // kimi / codex 的 SessionStart 判定已迁入 meowo_agent::config（KimiToml / CodexJson），
    // 测试随之搬到该 crate（见 has_reporter_only_counts_session_start）。

    /// claude 的接线规格（取自插件层的变体表，与真实接线同源）。
    fn claude_spec() -> &'static meowo_agent::config::HookSpec {
        meowo_agent::by_id("claude")
            .expect("claude 应已注册")
            .variants()[0]
            .hooks
    }

    #[test]
    fn claude_hooks_status_three_way() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("meowo-claude-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let status = |p: &std::path::Path| hooks_status_at(p, claude_spec(), "claude");

        let _ = std::fs::remove_file(&path);
        assert!(matches!(status(&path), HooksStatus::Missing));

        // Installed：command 用 ClaudeJson 认可的「带引号 meowo-reporter 路径、无参数」格式。
        let installed = r#"{"hooks":{"SessionStart":[{"matcher":"*","hooks":[{"type":"command","command":"\"C:/x/meowo-reporter.exe\""}]}]}}"#;
        std::fs::File::create(&path)
            .unwrap()
            .write_all(installed.as_bytes())
            .unwrap();
        assert!(matches!(status(&path), HooksStatus::Installed));

        let foreign =
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"node other.js"}]}]}}"#;
        std::fs::File::create(&path)
            .unwrap()
            .write_all(foreign.as_bytes())
            .unwrap();
        assert!(matches!(status(&path), HooksStatus::Missing));

        // 损坏 JSON → Unknown（核心不变量：不误报 Missing）
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"{not json")
            .unwrap();
        assert!(matches!(status(&path), HooksStatus::Unknown));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「损坏 → Unknown」对三家一致：kimi 的 TOML 与 codex 的 JSON 同样不许误报 Missing。
    #[test]
    fn corrupt_config_is_unknown_for_every_agent() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("meowo-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for id in ["claude", "codex", "kimi"] {
            let spec = meowo_agent::by_id(id).unwrap().variants()[0].hooks;
            let path = dir.join(format!("{id}-{}", spec.config_rel.replace('/', "_")));
            // 对 TOML 与 JSON 都是非法文本。
            std::fs::File::create(&path)
                .unwrap()
                .write_all(b"{not parseable [[[")
                .unwrap();
            assert!(
                matches!(hooks_status_at(&path, spec, id), HooksStatus::Unknown),
                "{id} 损坏配置应为 Unknown"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
