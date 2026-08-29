// tauri-nspanel v2 分支基于已弃用的 cocoa crate；其常量与 panel_delegate! 宏在 -D warnings 下会触发
// deprecated 与 unexpected_cfgs(cargo-clippy) 告警。这些来自上游、我们无法消除，按需放行。
#![allow(deprecated, unexpected_cfgs)]
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_nspanel::{
    cocoa::appkit::{NSMainMenuWindowLevel, NSWindowCollectionBehavior},
    panel_delegate, ManagerExt, WebviewWindowExt,
};
use tauri_plugin_positioner::{Position, WindowExt};

#[allow(non_upper_case_globals)]
const NS_NONACTIVATING_PANEL: i32 = 1 << 7; // NSWindowStyleMaskNonActivatingPanel
/// NSWindowStyleMaskResizable。`set_style_mask` 是 `setStyleMask:` 的直传——**整体替换**
/// 而非按位或，只传 NonActivatingPanel 会把 tao 建窗时设好的 Resizable 位一起抹掉，
/// 于是 macOS 用户拖不动贴纸边框（而 tauri.macos.conf.json 明写 `resizable: true`
/// 并给了 minWidth/minHeight，Windows 上是能拖的）。
#[allow(non_upper_case_globals)]
const NS_RESIZABLE: i32 = 1 << 3;

const RESIGN_EVENT: &str = "menubar_panel_did_resign_key";

/// 「保持打开」（W-12）：贴纸 pin 按钮在 macOS 面板上的语义——面板层级本就常驻最前，
/// pin 不再是 always-on-top，而是「失焦不自动收起」。状态由前端 pin 偏好（localStorage）
/// 经 set_panel_keep_open 命令推入；内存标志即可，不另进 settings.rs（pin 偏好本来就
/// 存在前端，两个平台同一份 PIN_KEY）。
static KEEP_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 前端 pin 切换的推入口（window::set_panel_keep_open 命令的 macOS 实现）。
pub fn set_keep_open(open: bool) {
    KEEP_OPEN.store(open, std::sync::atomic::Ordering::Relaxed);
}

/// 贴纸毛玻璃圆角，须与 CSS `:root.platform-macos .sticker` 的 border-radius 一致。
const GLASS_RADIUS: f64 = 16.0;

/// 把已存在的 main 窗口原地转成 NonactivatingPanel，并接好失焦 -> emit 事件。
pub fn convert_main_to_panel(app: &AppHandle) {
    let window = match app.get_webview_window("main") {
        Some(w) => w,
        None => return,
    };

    // 玻璃质感：在透明窗后垫一层 NSVisualEffectView(毛玻璃)，圆角与 CSS .sticker 一致。
    // backdrop-filter 在透明 Tauri 窗里模糊不到桌面，必须用原生特效。state=Active：非激活面板也保持磨砂
    // (不随失焦变暗)；降低贴纸不透明度(--cc-opacity)时即透出磨砂玻璃而非桌面。转 panel 前应用，特效随 contentView 保留。
    let _ = window.set_effects(tauri::utils::config::WindowEffectsConfig {
        effects: vec![tauri::utils::WindowEffect::HudWindow],
        state: Some(tauri::utils::WindowEffectState::Active),
        radius: Some(GLASS_RADIUS),
        color: None,
    });

    let panel = match window.to_panel() {
        Ok(p) => p,
        Err(_) => return,
    };

    let delegate = panel_delegate!(CcPanelDelegate {
        window_did_resign_key
    });
    let handle = app.clone();
    delegate.set_listener(Box::new(move |name: String| {
        if name == "window_did_resign_key" {
            let _ = handle.emit(RESIGN_EVENT, ());
        }
    }));

    panel.set_level(NSMainMenuWindowLevel + 1);
    panel.set_style_mask(NS_NONACTIVATING_PANEL | NS_RESIZABLE);
    panel.set_collection_behaviour(
        NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary,
    );
    panel.set_delegate(delegate);

    // 启动即隐藏，等托盘点击再显示。
    panel.order_out(None);
}

/// 失焦自动隐藏的监听器（在 setup 里调用一次）。
pub fn setup_resign_listener(app: &AppHandle) {
    let handle = app.clone();
    app.listen_any(RESIGN_EVENT, move |_| {
        // 保持打开（pin）时不自动收起（W-12）；托盘点击的显式收起不受影响（toggle_panel）。
        if KEEP_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if let Ok(panel) = handle.get_webview_panel("main") {
            panel.order_out(None);
        }
    });
}

/// 托盘点击：可见则收起，不可见则定位到图标下方再显示。
pub fn toggle_panel(app: &AppHandle) {
    let panel = match app.get_webview_panel("main") {
        Ok(p) => p,
        Err(_) => return,
    };
    if panel.is_visible() {
        panel.order_out(None);
        return;
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.move_window(Position::TrayCenter); // 先定位
    }
    panel.show(); // 后显示
}

/// 托盘菜单「找回贴纸」：只在面板隐藏时定位到图标下方再显示——「找回」是单向带到眼前，
/// 面板已可见时不动作（不能像 toggle 那样反而把它收掉）。与 Windows 托盘菜单同项对齐。
pub fn recall_panel(app: &AppHandle) {
    let panel = match app.get_webview_panel("main") {
        Ok(p) => p,
        Err(_) => return,
    };
    if panel.is_visible() {
        return;
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.move_window(Position::TrayCenter);
    }
    panel.show();
}
