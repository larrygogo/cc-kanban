//! 应用窗口（设置 / 更新 / 新建会话）与系统托盘/菜单的创建与本地化。从 lib.rs 抽出。

use crate::settings::{load_settings, tr, ui_lang};
use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
use tauri::Manager;
// Emitter：仅非 macOS 的 recall_sticker 用 w.emit("recall-sticker")；open_new_session 的 ns-prefill
// emit 在其块内另有本地 use（macOS 也走那条），故模块级 Emitter 在 macOS 上无用、按平台门控。
#[cfg(not(target_os = "macos"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
#[cfg(not(target_os = "macos"))]
use tauri::Emitter;

/// 前端调用：打开设置窗口（贴纸 tab 栏的设置按钮）。
/// 必须在子线程创建：同步 command 跑在主线程，直接 build() 会阻塞主线程消息泵，
/// 而 WebView2 初始化依赖消息泵运转 → 卡在初始化 → 白屏。子线程里 build() 把创建
/// dispatch 回主线程异步执行，泵不被阻塞。
#[tauri::command]
pub(crate) fn open_settings(app: tauri::AppHandle) {
    std::thread::spawn(move || open_settings_window(&app));
}

/// `visible:false` 创建的窗口的兜底显示。窗口创建即可见的话，WebView 画出首帧前是
/// 默认白底——打开瞬间闪一下白框；故所有窗口（贴纸/设置/对话/更新/引导/新建会话）都
/// 隐藏创建，由前端首帧后自行 show（useShowWhenReady）。但前端脚本没起来
/// （加载失败/崩溃）时窗口会永久隐身、用户以为点了没反应——到点仍不可见就强制显示，
/// 宁可白屏也要可见可关。`focus` 对贴纸主窗口传 false（窗口配置 focus:false，
/// 开机自启不得抢焦点），其余弹出窗口传 true。
pub(crate) fn show_after_grace(window: &tauri::WebviewWindow, focus: bool) {
    let w = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2000));
        if !w.is_visible().unwrap_or(true) {
            let _ = w.show();
            if focus {
                let _ = w.set_focus();
            }
        }
    });
}

/// Win11 的 DWM 只给带标题栏或 thick frame 的窗口自动圆角；`decorations(false)` +
/// `resizable(false)` 的小窗（设置/引导/更新/新建会话/确认）两者皆无，默认直角——必须
/// 显式声明圆角偏好。对话窗 resizable(true) 带 thick frame 故天然圆角；贴纸主窗
/// transparent + 前端 CSS 圆角，都不需要这里。Win10 不认这个属性（返回错误码），
/// 静默忽略：那代系统全窗直角是系统规范，不强造。macOS 无此概念（transparent +
/// 前端圆角），空实现。
pub(crate) fn round_window_corners(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };
        if let Ok(handle) = window.hwnd() {
            let preference = DWMWCP_ROUND;
            unsafe {
                DwmSetWindowAttribute(
                    handle.0 as _,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    std::ptr::addr_of!(preference).cast(),
                    std::mem::size_of_val(&preference) as u32,
                );
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

/// 生产构建整族关掉 WebView2 的「浏览器加速键」（Ctrl+P 打印 / Ctrl+R·F5 刷新 /
/// Ctrl+S 另存 / Ctrl+O 打开文件 / F7 光标浏览 / Ctrl+F 原生查找条等）：这些默认行为
/// 在桌面应用里全是缺陷,前端 devtools-guard 只能逐键追。编辑键（Ctrl+C/V/X/A/Z）
/// 不属于此列,不受影响;按键事件照常到达页面,应用内的 Ctrl+F 搜索接管不变。
/// tauri 2.11 没透传 wry 的 browser_accelerator_keys,故建成后经 with_webview 自己
/// 调 WebView2 设置。dev 构建放行——F12/Ctrl+R 是日常调试路径,与 devtools-guard 同界。
/// 每次页面加载都会被调一遍（on_page_load 挂载）,重复设置无害。
/// cfg! 而非 #[cfg] 做 debug 判定:release 专属代码 dev 也要过编译(同 single-instance)。
pub(crate) fn disable_browser_accelerators(webview: &tauri::Webview) {
    if cfg!(debug_assertions) {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        let _ = webview.with_webview(|platform| {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
            use windows::core::Interface;
            let apply = || -> windows::core::Result<()> {
                // SAFETY: with_webview 保证闭包在主线程执行,controller 是活的 COM 指针。
                unsafe {
                    platform
                        .controller()
                        .CoreWebView2()?
                        .Settings()?
                        .cast::<ICoreWebView2Settings3>()?
                        .SetAreBrowserAcceleratorKeysEnabled(false)
                }
            };
            if let Err(error) = apply() {
                eprintln!("关闭 WebView2 浏览器加速键失败: {error}");
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = webview;
}

/// 独立窗口的**原生**底色，与 styles.css 的 --cc-window-bg 对齐（dark #1c1c1e / light #f6f6f7）。
///
/// 隐藏创建 + 首帧后 show 只消除了「窗口先于首帧显示」的那类白框；show 的瞬间 WebView2
/// 的合成帧仍可能没上屏（rAF 提交 ≠ 已呈现），这一两帧露出的是原生窗口底色——默认白，
/// 于是还是闪。把原生底色设成与页面主题同色，即便露出来也是同色纯色帧，肉眼不可辨。
/// theme=system 时借主窗口的 theme()（跟随 OS，本应用不调 set_theme）判定明暗。
/// 全平台的独立窗口都用：macOS 的 Overlay 原生标题栏窗口是不透明的，同样要防闪白；
/// 唯一例外是贴纸主窗（transparent + 原生毛玻璃，不经此函数）。
pub(crate) fn window_background_color(app: &tauri::AppHandle) -> tauri::webview::Color {
    let dark = match load_settings().theme.as_str() {
        "light" => false,
        "dark" => true,
        _ => app
            .get_webview_window("main")
            .and_then(|w| w.theme().ok())
            .map(|t| t == tauri::Theme::Dark)
            // 拿不到就按深色：应用默认主题即深色，闪浅不如闪深。
            .unwrap_or(true),
    };
    if dark {
        tauri::webview::Color(0x1c, 0x1c, 0x1e, 0xff)
    } else {
        tauri::webview::Color(0xf6, 0xf6, 0xf7, 0xff)
    }
}

/// 打开（或聚焦）设置窗口。窗口 label 为 "about"（main.tsx 按此 label 路由到设置页）。
/// 托盘左键点击与右键菜单「设置」共用此逻辑。
pub(crate) fn open_settings_window(app: &tauri::AppHandle) {
    // macOS：打开设置窗口前临时切到 Regular 激活策略，否则纯托盘 App 的窗口无法获焦。
    #[cfg(target_os = "macos")]
    crate::macos::menubar::settings_window_will_open(app, "about");

    if let Some(w) = app.get_webview_window("about") {
        let _ = w.set_focus();
    } else {
        let builder = tauri::WebviewWindowBuilder::new(
            app,
            "about",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .title(tr(ui_lang(&load_settings()), "window.settings"))
        .inner_size(620.0, 460.0)
        .min_inner_size(620.0, 460.0)
        .resizable(false)
        // 隐藏创建，前端首帧后自行显示（见 show_after_grace 注释），消除白框闪烁。
        .visible(false)
        .center();
        // 非 macOS：无边框 + 前端自绘标题栏（Windows 圆角由 DWM 处理，见 round_window_corners）；
        // 原生底色对齐主题，show 瞬间合成帧未上屏也不露白（见 window_background_color）。
        #[cfg(not(target_os = "macos"))]
        let builder = builder
            .decorations(false)
            .background_color(window_background_color(app));
        // macOS：原生标题栏 Overlay——系统红绿灯在左上，符合平台惯例（前端按平台隐藏
        // 自绘 ✕ 并给红绿灯留位）；hidden_title 不画原生标题文字，标题仍由前端自绘。
        // 原生窗口自带圆角与阴影，不再需要 transparent + CSS 圆角。
        #[cfg(target_os = "macos")]
        let builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .background_color(window_background_color(app));
        match builder.build() {
            Ok(_about_window) => {
                round_window_corners(&_about_window);
                show_after_grace(&_about_window, true);
                // macOS：设置窗口关闭后归还激活策略计数，最后一个窗口关闭才切回 Accessory（重新隐藏 Dock 图标）。
                #[cfg(target_os = "macos")]
                {
                    let app_handle = app.clone();
                    _about_window.on_window_event(move |e| {
                        if matches!(
                            e,
                            tauri::WindowEvent::CloseRequested { .. }
                                | tauri::WindowEvent::Destroyed
                        ) {
                            crate::macos::menubar::settings_window_did_close(&app_handle, "about");
                        }
                    });
                }
            }
            Err(e) => eprintln!("创建设置窗口失败: {e}"),
        }
    }
}

/// 前端调用：打开使用引导窗口（首次启动自动弹 / 托盘「使用引导」/ 设置页「使用引导」按钮）。
/// 与 open_settings 同理由走子线程创建：同步 command 在主线程 build 会阻塞消息泵致白屏。
#[tauri::command]
pub(crate) fn open_onboarding(app: tauri::AppHandle) {
    std::thread::spawn(move || open_onboarding_window(&app));
}

/// 打开（或聚焦）引导窗口。label 为 "onboarding"（main.tsx 按此 label 路由到引导页）。
pub(crate) fn open_onboarding_window(app: &tauri::AppHandle) {
    // macOS：纯托盘 App 的窗口需临时切 Regular 激活策略才能获焦（同设置窗口）。
    #[cfg(target_os = "macos")]
    crate::macos::menubar::settings_window_will_open(app, "onboarding");

    if let Some(w) = app.get_webview_window("onboarding") {
        let _ = w.set_focus();
        return;
    }
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        "onboarding",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(tr(ui_lang(&load_settings()), "window.onboarding"))
    .inner_size(460.0, 600.0)
    .min_inner_size(460.0, 600.0)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    // 隐藏创建，前端首帧后自行显示（见 show_after_grace 注释），消除白框闪烁。
    .visible(false)
    .center();
    // 平台分支的理由见 open_settings_window 的同款注释。
    #[cfg(not(target_os = "macos"))]
    let builder = builder
        .decorations(false)
        .background_color(window_background_color(app));
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .background_color(window_background_color(app));
    match builder.build() {
        Ok(_onboarding_window) => {
            round_window_corners(&_onboarding_window);
            show_after_grace(&_onboarding_window, true);
            // macOS：引导窗口关闭后归还激活策略计数，最后一个窗口关闭才切回 Accessory（同设置/更新窗口）。
            #[cfg(target_os = "macos")]
            {
                let app_handle = app.clone();
                _onboarding_window.on_window_event(move |e| {
                    if matches!(
                        e,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    ) {
                        crate::macos::menubar::settings_window_did_close(&app_handle, "onboarding");
                    }
                });
            }
        }
        Err(e) => eprintln!("创建引导窗口失败: {e}"),
    }
}

/// 前端调用：打开软件更新窗口（贴纸更新红点 / 设置页「更新到 vX」按钮）。
/// 与 open_settings 同理由走子线程创建：同步 command 在主线程 build 会阻塞消息泵致白屏。
#[tauri::command]
pub(crate) fn open_update_window(app: tauri::AppHandle) {
    std::thread::spawn(move || open_update_window_impl(&app));
}

/// 打开（或聚焦）更新窗口。label 为 "updater"（main.tsx 按此 label 路由到更新页）。
/// 更新窗口是检查/下载/安装的唯一所有者——主窗与设置窗只负责把它打开，
/// 不再有跨窗口 trigger-update/update-failed 事件协议。
pub(crate) fn open_update_window_impl(app: &tauri::AppHandle) {
    // macOS：纯托盘 App 的窗口需临时切 Regular 激活策略才能获焦（同设置窗口）。
    #[cfg(target_os = "macos")]
    crate::macos::menubar::settings_window_will_open(app, "updater");

    if let Some(w) = app.get_webview_window("updater") {
        let _ = w.set_focus();
        return;
    }
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        "updater",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(tr(ui_lang(&load_settings()), "window.updater"))
    // 紧凑初始高度（检查中/已最新/失败态）；发现新版带更新说明时由前端 setSize 增高。
    .inner_size(400.0, 252.0)
    .min_inner_size(400.0, 252.0)
    .resizable(false)
    // 隐藏创建，前端首帧后自行显示（见 show_after_grace 注释），消除白框闪烁。
    .visible(false)
    .center();
    // 平台分支的理由见 open_settings_window 的同款注释。
    #[cfg(not(target_os = "macos"))]
    let builder = builder
        .decorations(false)
        .background_color(window_background_color(app));
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .background_color(window_background_color(app));
    match builder.build() {
        Ok(_update_window) => {
            round_window_corners(&_update_window);
            show_after_grace(&_update_window, true);
            // macOS：更新窗口关闭后归还激活策略计数，最后一个窗口关闭才切回 Accessory（同设置窗口）。
            #[cfg(target_os = "macos")]
            {
                let app_handle = app.clone();
                _update_window.on_window_event(move |e| {
                    if matches!(
                        e,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    ) {
                        crate::macos::menubar::settings_window_did_close(&app_handle, "updater");
                    }
                });
            }
        }
        Err(e) => eprintln!("创建更新窗口失败: {e}"),
    }
}

/// 前端调用：打开「新建会话」窗口（贴纸底栏 + 按钮 / 空状态 CTA / 会话卡片菜单）。
/// 传入 cwd/provider 时，新建面板会预填该路径并选中该模型。
/// 与 open_settings 同理由走子线程创建：同步 command 在主线程 build 会阻塞消息泵致白屏。
#[tauri::command]
pub(crate) fn open_new_session_window(
    app: tauri::AppHandle,
    cwd: Option<String>,
    provider: Option<String>,
) {
    std::thread::spawn(move || open_new_session_window_impl(&app, cwd, provider));
}

/// 打开（或聚焦）新建会话窗口。label 为 "new-session"（main.tsx 按此 label 路由到面板页）。
pub(crate) fn open_new_session_window_impl(
    app: &tauri::AppHandle,
    cwd: Option<String>,
    provider: Option<String>,
) {
    // macOS：纯托盘 App 的窗口需临时切 Regular 激活策略才能获焦（同设置窗口）。
    #[cfg(target_os = "macos")]
    crate::macos::menubar::settings_window_will_open(app, "new-session");

    if let Some(w) = app.get_webview_window("new-session") {
        // 窗口已开：若从另一张卡片带了 cwd/provider 预填，通知面板更新表单（不重开窗口），再聚焦。
        if cwd.is_some() || provider.is_some() {
            use tauri::Emitter;
            let _ = app.emit(
                "ns-prefill",
                serde_json::json!({ "cwd": cwd, "provider": provider }),
            );
        }
        let _ = w.set_focus();
        return;
    }
    let url = match (&cwd, &provider) {
        (None, None) => "index.html".to_string(),
        _ => {
            let mut params = Vec::new();
            if let Some(c) = &cwd {
                params.push(format!(
                    "cwd={}",
                    percent_encode(c.as_bytes(), NON_ALPHANUMERIC)
                ));
            }
            if let Some(p) = &provider {
                params.push(format!(
                    "provider={}",
                    percent_encode(p.as_bytes(), NON_ALPHANUMERIC)
                ));
            }
            format!("index.html?{}", params.join("&"))
        }
    };
    let builder =
        tauri::WebviewWindowBuilder::new(app, "new-session", tauri::WebviewUrl::App(url.into()))
            .title(tr(ui_lang(&load_settings()), "window.newSession"))
            .inner_size(460.0, 520.0)
            .min_inner_size(460.0, 520.0)
            .resizable(false)
            // 隐藏创建，前端首帧后自行显示（见 show_after_grace 注释），消除白框闪烁。
            .visible(false)
            .center();
    // 平台分支的理由见 open_settings_window 的同款注释。
    #[cfg(not(target_os = "macos"))]
    let builder = builder
        .decorations(false)
        .background_color(window_background_color(app));
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .background_color(window_background_color(app));
    match builder.build() {
        Ok(_win) => {
            round_window_corners(&_win);
            show_after_grace(&_win, true);
            #[cfg(target_os = "macos")]
            {
                let app_handle = app.clone();
                _win.on_window_event(move |e| {
                    if matches!(
                        e,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    ) {
                        crate::macos::menubar::settings_window_did_close(
                            &app_handle,
                            "new-session",
                        );
                    }
                });
            }
        }
        Err(e) => eprintln!("创建新建会话窗口失败: {e}"),
    }
}

/// macOS：给 Overlay 标题栏窗口挂一个空 NSToolbar（unified 风格），系统据此把红绿灯
/// 垂直居中到加高的标题栏区（约 52px），与同高的自绘工具栏行对齐。这是 AppKit 的
/// 官方语义，布局由系统持续维护；逐帧挪按钮 frame 的做法会在窗口重排（show/聚焦/
/// 全屏往返）时被 AppKit 重置，builder 的 traffic_light_position 正是因此不生效。
/// toolbar 无任何 item，titlebarAppearsTransparent 下不画任何东西，只改按钮布局。
#[cfg(target_os = "macos")]
pub(crate) fn unify_titlebar_toolbar(window: &tauri::WebviewWindow) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    let w = window.clone();
    // AppKit 对象只能在主线程操作；build() 可能发生在任意子线程。
    let _ = window.run_on_main_thread(move || {
        if let Ok(ns_window) = w.ns_window() {
            let ns_window = ns_window as *mut AnyObject;
            unsafe {
                let toolbar: *mut AnyObject = msg_send![class!(NSToolbar), new];
                let _: () = msg_send![ns_window, setToolbar: toolbar];
                // NSWindowToolbarStyleUnified = 3（macOS 11+，本应用最低 14.0）。
                let _: () = msg_send![ns_window, setToolbarStyle: 3isize];
            }
        }
    });
}

/// 打开单例会话对话窗口。再次点击另一张卡片时复用窗口并发事件切换会话。
/// 创建/切换失败必须传播回前端：静默失败时用户「点了没反应」，再点会重复起会话。
#[tauri::command]
pub(crate) async fn open_chat_window(app: tauri::AppHandle, session_id: i64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_chat_window_impl(&app, session_id, true))
        .await
        .map_err(|e| e.to_string())?
}

/// 无处上报错误的入口（通知/托盘点击等**用户主动**动作）用的 fire-and-forget 变体；
/// 失败仅留日志。点击是明确意图，窗口正常抢焦点。
pub(crate) fn open_chat_window_detached(app: tauri::AppHandle, session_id: i64) {
    std::thread::spawn(move || {
        if let Err(error) = open_chat_window_impl(&app, session_id, true) {
            eprintln!("打开对话窗口失败: {error}");
        }
    });
}

/// 审批/提问唤醒专用的**安静**变体：窗口要出现（否则审批无处可答），但**不抢焦点**——
/// 用户可能正在外部终端里打字，焦点被抢走一次就是一次输入错位（实拍反馈）。
/// 召唤注意力由调用方的任务栏闪烁（request_user_attention）承担。
pub(crate) fn open_chat_window_quiet(app: tauri::AppHandle, session_id: i64) {
    std::thread::spawn(move || {
        if let Err(error) = open_chat_window_impl(&app, session_id, false) {
            eprintln!("打开对话窗口失败: {error}");
        }
    });
}

/// 对话窗此刻是否「在用户眼前」：存在、可见、且未最小化。审批/提问的召唤策略据此
/// 分流——在眼前时窗口里必有用户正在看的会话，强行切会话是夺屏（实拍反馈：正在 A
/// 会话里阅读/打字，B 会话一来审批就被瞬间拽走）；不在眼前（隐藏/最小化/未创建）时
/// 切换无打断，仍走切会话召唤。查询失败按「不在眼前」处理：宁可多召唤一次，
/// 不可让审批悄悄没入后台。
pub(crate) fn chat_window_in_view(app: &tauri::AppHandle) -> bool {
    app.get_webview_window("chat")
        .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false))
}

/// 显示但不激活：Windows 用 SW_SHOWNOACTIVATE（普通 show 会激活并抢焦点）。
/// 其余平台退化为普通 show——macOS 上显示非激活 App 的窗口本就不跨应用抢焦点
/// （激活策略未变），够用。
fn show_window_no_activate(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
        if let Ok(handle) = window.hwnd() {
            unsafe {
                ShowWindow(handle.0 as _, SW_SHOWNOACTIVATE);
            }
            return;
        }
    }
    let _ = window.show();
}

/// 把窗口可靠带到前台。普通 set_focus 在 Windows 上受前台锁限制：托盘点击授予的
/// SetForegroundWindow 权限只在点击后的极短窗口内有效，稍有延迟（查库、WebView 首帧）
/// 就被拒——窗口显示了却垫在当前前台窗口身后，用户看到的就是「点了没反应」
/// （实拍：dev/安装版并存时双击托盘「呼不出」对话窗，其实它开在另一实例的对话窗底下）。
/// 走 AttachThreadInput 的 force_foreground（点卡片拉终端置前的同一套）强置前。
fn raise_window(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "windows")]
    if let Ok(handle) = window.hwnd() {
        crate::terminal::force_foreground(handle.0 as _);
        return;
    }
    let _ = window.set_focus();
}

/// 托盘点击入口：打开最近活跃会话的对话窗口。托盘没有「当前会话」上下文，
/// 取 last_event_at 最新的未归档会话最贴近「看看现在在跑什么」。
/// 一条会话都没有（新装/全归档）时回落到设置窗口，避免点了毫无反应。
/// 入口：非 macOS 托盘左键；macOS 托盘菜单「打开对话窗口」；贴纸底栏按钮（open_latest_chat）。
pub(crate) fn open_latest_chat_window(app: &tauri::AppHandle) {
    let app = app.clone();
    // 查库放子线程：与 open_settings_window 同理由，主线程同步 IO 会阻塞消息泵。
    std::thread::spawn(move || {
        let latest = crate::open_store(&crate::db_path())
            .ok()
            .and_then(|store| store.latest_session_id().ok().flatten());
        match latest {
            Some(session_id) => {
                // 托盘入口没有 UI 可上报；至少留日志。
                if let Err(error) = open_chat_window_impl(&app, session_id, true) {
                    eprintln!("打开对话窗口失败: {error}");
                }
            }
            None => open_settings_window(&app),
        }
    });
}

/// 前端调用（贴纸底栏「打开对话窗口」按钮）：打开最近活跃会话的对话窗口。
/// 同步 command 仅做 thread::spawn（open_latest_chat_window 内部自带子线程查库），不阻塞主线程。
#[tauri::command]
pub(crate) fn open_latest_chat(app: tauri::AppHandle) {
    open_latest_chat_window(&app);
}

/// `activate`：true = 用户主动打开（点卡片/托盘/通知），照常抢焦点；
/// false = 审批/提问唤醒——窗口要出现但不夺焦（用户可能正在外部终端打字，实拍反馈）。
pub(crate) fn open_chat_window_impl(
    app: &tauri::AppHandle,
    session_id: i64,
    activate: bool,
) -> Result<(), String> {
    // 对话功能关闭（轻量模式）时的唯一闸门：四个开窗变体（command/detached/quiet/latest）
    // 全部汇于此，一处拦截全覆盖。用户主动点击（activate）回落设置窗——那里有重新打开
    // 的开关，点了不能毫无反应；审批/提问的安静唤醒则无声吞掉，调用方已各自回落 TUI。
    if !crate::settings::load_settings().chat_enabled {
        if activate {
            open_settings_window(app);
        }
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    crate::macos::menubar::settings_window_will_open(app, "chat");

    if let Some(w) = app.get_webview_window("chat") {
        use tauri::Emitter;
        // emit 失败绝不能静默：窗口会照常聚焦，但停留在旧会话——用户点了卡片 A，
        // 看到的却是会话 B，比不弹窗更具误导性。
        w.emit("chat-session-changed", session_id)
            .map_err(|e| format!("切换会话失败: {e}"))?;
        if activate {
            let _ = w.show();
            raise_window(&w);
        } else if !w.is_visible().unwrap_or(false) {
            // 隐藏（关到托盘等）时无声显形；最小化的留在任务栏——闪烁已在召唤，
            // 强行还原窗口同样是夺屏。
            show_window_no_activate(&w);
        }
        return Ok(());
    }
    let url = format!("index.html?sessionId={session_id}");
    let builder = tauri::WebviewWindowBuilder::new(app, "chat", tauri::WebviewUrl::App(url.into()))
        .title("Meowo · Chat")
        .inner_size(960.0, 760.0)
        .min_inner_size(620.0, 480.0)
        .resizable(true)
        // 安静唤醒的新窗口不带初始焦点（冷启动首帧 show 的激活行为随之收敛）。
        .focused(activate)
        // 隐藏创建，前端首帧后自行显示（见 show_after_grace 注释），消除白框闪烁。
        .visible(false)
        .center();
    // 平台分支的理由见 open_settings_window 的同款注释。
    #[cfg(not(target_os = "macos"))]
    let builder = builder
        .decorations(false)
        .background_color(window_background_color(app));
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .background_color(window_background_color(app));
    let win = builder
        .build()
        .map_err(|e| format!("创建对话窗口失败: {e}"))?;
    // 对话窗标题栏行高 52px（.chat-bar/.chat-sidebar-head），红绿灯默认贴顶会和同行的
    // 标题/按钮错位；挂空 toolbar 让系统把它垂直居中（备忘录式侧栏惯例）。
    #[cfg(target_os = "macos")]
    unify_titlebar_toolbar(&win);
    show_after_grace(&win, activate);
    let app_handle = app.clone();
    win.on_window_event(move |e| {
        if matches!(
            e,
            tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
        ) {
            let ptys = &app_handle.state::<crate::AppState>().ptys;
            // 先清租约再放行挂起审批：窗口没了，租约留着只会让后续审批空等 300s。
            ptys.clear_approval_consumers();
            ptys.pass_pending_approvals();
            #[cfg(target_os = "macos")]
            crate::macos::menubar::settings_window_did_close(&app_handle, "chat");
        }
    });
    // 置前兜底：真正 show 由前端首帧触发（useShowWhenReady），距点击已过去数秒
    // （懒加载 chunk；dev 下 vite 现场编译更久）——前端那句 setFocus 到达时托盘点击
    // 授予的前台权限早已过期，窗口默默垫在当前前台窗口身后。盯到首次可见即强置前；
    // 只对用户主动打开（activate）做，安静唤醒不夺焦的语义不变。
    if activate {
        let w = win.clone();
        std::thread::spawn(move || {
            for _ in 0..300 {
                if w.is_visible().unwrap_or(false) {
                    raise_window(&w);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
    }
    Ok(())
}

/// 「找回贴纸」：把主窗口按当前尺寸居中到主显示器工作区，并显示/取消最小化/置顶/聚焦。
/// 折叠态的「展开 + 还原正常尺寸」由前端在调用本命令前完成（snap_restore），故这里只按当前尺寸居中。
#[tauri::command]
pub(crate) fn recall_center(window: tauri::WebviewWindow) -> Result<(), String> {
    let _ = window.unminimize();
    let _ = window.show();
    // 优先主显示器（找回的「家」最可预期）；取不到回退当前屏。
    let monitor = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten());
    if let Some(m) = monitor {
        let wa = m.work_area();
        let sz = window.outer_size().map_err(|e| e.to_string())?;
        let x = wa.position.x + (wa.size.width as i32 - sz.width as i32) / 2;
        let y = wa.position.y + (wa.size.height as i32 - sz.height as i32) / 2;
        window
            .set_position(tauri::PhysicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
    }
    window.set_always_on_top(true).map_err(|e| e.to_string())?;
    let _ = window.set_focus();
    Ok(())
}

/// 托盘「找回贴纸」：唤起主窗口并通知前端执行完整找回（展开折叠 + 居中到主屏 + 置顶）。
#[cfg(not(target_os = "macos"))]
pub(crate) fn recall_sticker(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        // 原生层几何救援，不依赖 webview 活着：WebView2 浏览器进程崩溃后窗口 JS 全灭，
        // 下面 emit 的事件无人处理，「找回」就彻底失灵（实拍：吸附细条卡在屏边，重启
        // 才恢复）。窗口还是细条尺寸就先撑回正常最小尺寸，再居中主屏；前端若活着，
        // 随后会按它记录的正常尺寸精确还原（snap_restore + recall_center），终态一致。
        let scale = w.scale_factor().unwrap_or(1.0);
        if let Ok(size) = w.outer_size() {
            let logical = size.to_logical::<f64>(scale);
            if logical.width < crate::snap::STICKER_MIN_W || logical.height < crate::snap::STICKER_MIN_H
            {
                let _ = w.set_size(tauri::LogicalSize::new(
                    crate::snap::STICKER_MIN_W,
                    crate::snap::STICKER_MIN_H,
                ));
            }
        }
        let _ = recall_center(w.clone());
        let _ = w.emit("recall-sticker", ());
    }
}

/// 托盘右键菜单（找回贴纸 / 设置 / 官网 / 退出），按语言构建；切语言时由 rebuild_tray_menu 重建。
#[cfg(not(target_os = "macos"))]
pub(crate) fn build_tray_menu(
    app: &tauri::AppHandle,
    lang: &str,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let recall = MenuItemBuilder::with_id("recall", tr(lang, "tray.recall")).build(app)?;
    let guide = MenuItemBuilder::with_id("guide", tr(lang, "tray.guide")).build(app)?;
    let settings = MenuItemBuilder::with_id("settings", tr(lang, "tray.settings")).build(app)?;
    let website = MenuItemBuilder::with_id("website", tr(lang, "tray.website")).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", tr(lang, "tray.quit")).build(app)?;
    MenuBuilder::new(app)
        .items(&[&recall, &guide, &settings, &website, &quit])
        .build()
}

/// 切语言/改设置后让已存在的系统 UI 跟上：重建托盘菜单、改已开设置窗口的标题。
/// `chat_enabled` 决定 macOS 托盘菜单是否包含「打开对话窗口」项（主线程不读设置文件，
/// 由调用方从已持有的 Settings 传入——线程纪律）。
pub(crate) fn apply_language(app: &tauri::AppHandle, lang: &str, chat_enabled: bool) {
    #[cfg(not(target_os = "macos"))]
    let _ = chat_enabled; // Windows 托盘菜单无 chat 项，左键入口在事件回调里自行判断。
    if let Some(tray) = app.tray_by_id("meowo-tray") {
        #[cfg(not(target_os = "macos"))]
        if let Ok(menu) = build_tray_menu(app, lang) {
            let _ = tray.set_menu(Some(menu));
        }
        #[cfg(target_os = "macos")]
        if let Ok(menu) = crate::macos::menubar::build_tray_menu(app, lang, chat_enabled) {
            let _ = tray.set_menu(Some(menu));
        }
    }
    if let Some(w) = app.get_webview_window("about") {
        let _ = w.set_title(tr(lang, "window.settings"));
    }
    if let Some(w) = app.get_webview_window("updater") {
        let _ = w.set_title(tr(lang, "window.updater"));
    }
    // chat 窗口的标题由前端随会话状态动态维护（ChatWindow 的 setTitle effect），
    // 这里不再按语言重设——两处写同一标题会互相覆盖。
}

/// dev 构建的托盘角标：右下角叠一枚琥珀圆点，带深色描边与抗锯齿边缘——托盘图标
/// 只有 16~32px，无描边的硬边色块糊成一团锯齿。像素级绘制而非另备图标资源：
/// 角标只存在于 debug 构建，不值得为它维护一套打包文件。
#[cfg(not(target_os = "macos"))]
fn dev_badge_icon(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    /// 按覆盖率把颜色混进像素（普通 alpha over），cov ∈ [0,1]。
    fn blend(px: &mut [u8], color: [u8; 3], cov: f32) {
        for c in 0..3 {
            px[c] = (f32::from(color[c]) * cov + f32::from(px[c]) * (1.0 - cov)).round() as u8;
        }
        let a = f32::from(px[3]) / 255.0;
        px[3] = ((cov + a * (1.0 - cov)) * 255.0).round() as u8;
    }
    let (w, h) = (icon.width(), icon.height());
    let mut rgba = icon.rgba().to_vec();
    let r = (w.min(h) as f32) * 0.26;
    let rim = (r * 0.22).max(1.0); // 描边厚度随尺寸缩放，最小 1px 保证 16px 下仍有分离感
    let (cx, cy) = (w as f32 - r - 1.0, h as f32 - r - 1.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d = (dx * dx + dy * dy).sqrt();
            let Some(px) = rgba.get_mut(((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize)
            else {
                continue;
            };
            // 半像素过渡的圆盘覆盖率：外层深色圆先铺，内层琥珀圆压上，留出的环即描边。
            let outer = (r + 0.5 - d).clamp(0.0, 1.0);
            if outer > 0.0 {
                blend(px, [40, 34, 28], outer);
            }
            let inner = (r - rim + 0.5 - d).clamp(0.0, 1.0);
            if inner > 0.0 {
                blend(px, [245, 158, 11], inner);
            }
        }
    }
    tauri::image::Image::new_owned(rgba, w, h)
}

/// 构建系统托盘：左键点击直接打开设置；右键菜单提供设置 / 退出。
/// macOS 走 `macos::menubar::setup_tray`（面板模式），故此实现仅用于非 macOS 平台。
#[cfg(not(target_os = "macos"))]
pub(crate) fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let menu = build_tray_menu(app.handle(), ui_lang(&load_settings()))?;

    let mut builder = TrayIconBuilder::with_id("meowo-tray");
    // 图标恒由打包提供，但缺失时不该 unwrap panic 把启动打挂——没图标就建无图标托盘。
    // dev 构建叠橙色角标：共库后 dev 与安装版托盘并排且内容全同，只能靠图标分辨实例。
    if let Some(icon) = app.default_window_icon() {
        if cfg!(debug_assertions) {
            builder = builder.icon(dev_badge_icon(icon));
        } else {
            builder = builder.icon(icon.clone());
        }
    }
    builder
        .tooltip(if cfg!(debug_assertions) { "Meowo (dev)" } else { "Meowo" })
        .menu(&menu)
        // 左键留给「打开对话窗口」，菜单仅在右键弹出（设置仍在右键菜单里）。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "recall" => recall_sticker(app),
            "guide" => open_onboarding_window(app),
            "settings" => open_settings_window(app),
            "website" => {
                let _ = crate::settings::open_url(crate::settings::SITE_URL.to_string());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 仅左键「抬起」时触发，避免按下+抬起各触发一次。
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                // 对话功能关闭（轻量模式）时左键改为找回贴纸——轻量用户的主界面就是贴纸。
                // load_settings 是文件 IO，托盘回调在主线程，放子线程读（线程纪律）。
                let app = tray.app_handle().clone();
                std::thread::spawn(move || {
                    if crate::settings::load_settings().chat_enabled {
                        open_latest_chat_window(&app);
                    } else {
                        recall_sticker(&app);
                    }
                });
            }
        })
        .build(app)?;
    Ok(())
}

/// Windows：把待交互/运行中会话数摘要写进托盘悬浮提示，鼠标移到托盘一眼可见，
/// 弥补桌面端无菜单栏标题。计数为 0 时回落到纯品牌名。
#[cfg(target_os = "windows")]
pub(crate) fn update_tray_tooltip(
    app: &tauri::AppHandle,
    running: usize,
    waiting: usize,
    lang: &str,
) {
    let Some(tray) = app.tray_by_id("meowo-tray") else {
        return;
    };
    // dev 标记在调用点补而不进 tray_tooltip_text：那是带精确断言的纯函数，
    // 单测在 debug 下跑，函数内 cfg! 会让断言随构建漂移。
    let mut tip = tray_tooltip_text(lang, running, waiting);
    if cfg!(debug_assertions) {
        tip = tip.replacen("Meowo", "Meowo (dev)", 1);
    }
    let _ = tray.set_tooltip(Some(tip));
}

/// 构建托盘提示文案（本地化）。待交互更紧急，排在运行中之前。
#[cfg(target_os = "windows")]
pub(crate) fn tray_tooltip_text(lang: &str, running: usize, waiting: usize) -> String {
    if running == 0 && waiting == 0 {
        return "Meowo".into();
    }
    let mut parts: Vec<String> = Vec::new();
    if lang == "en" {
        if waiting > 0 {
            parts.push(format!("{waiting} waiting"));
        }
        if running > 0 {
            parts.push(format!("{running} running"));
        }
    } else {
        if waiting > 0 {
            parts.push(format!("{waiting} 个待交互"));
        }
        if running > 0 {
            parts.push(format!("{running} 个运行中"));
        }
    }
    format!("Meowo · {}", parts.join(" · "))
}
