//! wry UI 壳：无边框暗色窗口 + 内嵌单文件 HTML（三态状态机）。
//! 线程模型：wry 的 ipc 回调与 tao 事件循环都在主线程；目录选择（rfd 模态）与
//! 安装（perform_install）各自开线程，结果经 EventLoopProxy 回投主循环。
//! 业务真相全在 Rust 侧（logic.rs），JS 只渲染回推状态。

use std::num::NonZeroIsize;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::windows::WindowExtWindows;
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

use crate::install::{self, InstallError, InstallReq};
use crate::{detect, logic};

enum UserEvent {
    Ipc(String),
    DirPicked(Option<String>),
    InstallDone(Result<(), InstallError>),
}

/// rfd 需要 parent 的窗口句柄，而 tao::Window 不是 Send——把裸 HWND 包成
/// 实现 raw-window-handle 的小结构带进对话框线程。
struct ParentWindow(isize);

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        // SAFETY: HWND 由主线程存活的窗口而来，对话框模态期间窗口不可能被销毁。
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))) })
    }
}

impl HasDisplayHandle for ParentWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: Windows 显示句柄是无状态占位。
        Ok(unsafe {
            DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new()))
        })
    }
}

/// 系统 UI 语言主 ID == LANG_CHINESE(0x04) → zh，否则 en。
fn ui_locale() -> &'static str {
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    // SAFETY: 无参纯查询。
    let lang = unsafe { GetUserDefaultUILanguage() };
    if lang & 0x3FF == 0x04 {
        "zh"
    } else {
        "en"
    }
}

fn default_install_dir() -> String {
    std::env::var("LOCALAPPDATA")
        .map(|p| format!("{}\\Meowo", p.trim_end_matches('\\')))
        .unwrap_or_else(|_| r"C:\Meowo".to_string())
}

/// 受保护目录前缀（currentUser 安装写不进去）。
fn forbidden_prefixes() -> Vec<String> {
    ["ProgramFiles", "ProgramFiles(x86)", "WINDIR", "ProgramData"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .filter(|s| !s.trim().is_empty())
        .collect()
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

/// 事件循环不返回（内部 std::process::exit）；只有窗口/webview 构建失败才 Err，
/// 由 main 兜底到 NSIS GUI。
pub fn run() -> Result<(), String> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Meowo 安装")
        .with_decorations(false)
        .with_resizable(false)
        .with_inner_size(LogicalSize::new(680.0, 520.0))
        .with_visible(false) // 居中并建好 webview 再显示，避免闪跳
        .build(&event_loop)
        .map_err(|e| e.to_string())?;

    // 居中到当前显示器工作区（tao 无 center API）。
    if let Some(monitor) = window.current_monitor() {
        let msize = monitor.size();
        let mpos = monitor.position();
        let wsize = window.outer_size();
        let x = mpos.x + (msize.width as i32 - wsize.width as i32) / 2;
        let y = mpos.y + (msize.height as i32 - wsize.height as i32) / 2;
        window.set_outer_position(PhysicalPosition::new(x, y));
    }

    // 无边框窗口的 DWM 圆角（Win11；Win10 不认该属性，静默忽略）。
    // 先例：app 的 src/window.rs round_window_corners。
    #[allow(clippy::unnecessary_cast)]
    let hwnd_raw = window.hwnd() as isize;
    {
        use windows_sys::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };
        let preference = DWMWCP_ROUND;
        // SAFETY: hwnd 有效；属性指针指向栈上变量。
        unsafe {
            DwmSetWindowAttribute(
                hwnd_raw as _,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                std::ptr::addr_of!(preference).cast(),
                std::mem::size_of_val(&preference) as u32,
            );
        }
    }

    let html = include_str!("ui/index.html").replace(
        "__LOGO_B64__",
        &logic::base64_encode(include_bytes!("../../../icons/128x128@2x.png")),
    );

    let ipc_proxy = proxy.clone();
    let webview = WebViewBuilder::new()
        .with_html(html)
        .with_background_color((28, 28, 30, 255))
        .with_ipc_handler(move |req| {
            let _ = ipc_proxy.send_event(UserEvent::Ipc(req.body().clone()));
        })
        .build(&window)
        .map_err(|e| e.to_string())?;

    window.set_visible(true);
    window.set_focus();

    let push = move |webview: &wry::WebView, payload: serde_json::Value| {
        let _ = webview.evaluate_script(&format!("window.__push({payload})"));
    };

    // 主循环内的少量状态：安装中禁关闭；装完记安装目录给「立即体验」。
    let mut installing = false;
    let mut installed_dir: Option<String> = None;

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if !installing {
                    std::process::exit(0);
                }
            }
            Event::UserEvent(UserEvent::Ipc(raw)) => {
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    return;
                };
                match msg.get("cmd").and_then(|c| c.as_str()).unwrap_or("") {
                    "ready" => {
                        let existing = detect::detect_existing().map(|e| {
                            let relation =
                                match logic::compare_versions(&e.version, env!("MEOWO_APP_VERSION")) {
                                    std::cmp::Ordering::Less => "upgrade",
                                    std::cmp::Ordering::Equal => "same",
                                    std::cmp::Ordering::Greater => "downgrade",
                                };
                            serde_json::json!({ "version": e.version, "relation": relation })
                        });
                        push(
                            &webview,
                            serde_json::json!({
                                "type": "init",
                                "locale": ui_locale(),
                                "version": env!("MEOWO_APP_VERSION"),
                                "defaultDir": default_install_dir(),
                                "existing": existing,
                            }),
                        );
                    }
                    "drag" => {
                        let _ = window.drag_window();
                    }
                    "close" => {
                        if !installing {
                            std::process::exit(0);
                        }
                    }
                    "browse" => {
                        let start = json_str(&msg, "dir");
                        let dialog_proxy = proxy.clone();
                        let hwnd = hwnd_raw;
                        std::thread::spawn(move || {
                            let parent = ParentWindow(hwnd);
                            let mut dlg = rfd::FileDialog::new().set_parent(&parent);
                            let start_path = std::path::PathBuf::from(&start);
                            if start_path.is_dir() {
                                dlg = dlg.set_directory(start_path);
                            }
                            let picked =
                                dlg.pick_folder().map(|p| p.to_string_lossy().into_owned());
                            let _ = dialog_proxy.send_event(UserEvent::DirPicked(picked));
                        });
                    }
                    "install" => {
                        if installing {
                            return;
                        }
                        let raw_dir = json_str(&msg, "dir");
                        let dir =
                            match logic::normalize_install_dir_with(&raw_dir, &forbidden_prefixes())
                            {
                                Ok(d) => d,
                                Err(e) => {
                                    push(
                                        &webview,
                                        serde_json::json!({
                                            "type": "dirError",
                                            "code": dir_error_code(&e),
                                        }),
                                    );
                                    return;
                                }
                            };
                        installing = true;
                        installed_dir = Some(dir.clone());
                        push(&webview, serde_json::json!({ "type": "phase", "value": "installing" }));
                        let req = InstallReq {
                            dir,
                            chat_enabled: msg
                                .get("chat")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true),
                            desktop_shortcut: msg
                                .get("desktop")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true),
                        };
                        let install_proxy = proxy.clone();
                        std::thread::spawn(move || {
                            let result = install::perform_install(&req);
                            let _ = install_proxy.send_event(UserEvent::InstallDone(result));
                        });
                    }
                    "launch" => {
                        if let Some(dir) = &installed_dir {
                            // Windows 子进程不随父进程退出，spawn 后 drop child 即可。
                            let _ = std::process::Command::new(format!("{dir}\\meowo-app.exe"))
                                .current_dir(dir)
                                .spawn();
                        }
                        std::process::exit(0);
                    }
                    _ => {}
                }
            }
            Event::UserEvent(UserEvent::DirPicked(Some(raw))) => {
                match logic::normalize_install_dir_with(&raw, &forbidden_prefixes()) {
                    Ok(dir) => push(&webview, serde_json::json!({ "type": "dir", "value": dir })),
                    Err(e) => push(
                        &webview,
                        serde_json::json!({ "type": "dirError", "code": dir_error_code(&e) }),
                    ),
                }
            }
            Event::UserEvent(UserEvent::DirPicked(None)) => {}
            Event::UserEvent(UserEvent::InstallDone(result)) => {
                installing = false;
                match result {
                    Ok(()) => push(
                        &webview,
                        serde_json::json!({
                            "type": "phase",
                            "value": "done",
                            "dir": installed_dir.clone().unwrap_or_default(),
                        }),
                    ),
                    Err(e) => {
                        let (code, detail) = match e {
                            InstallError::NotWritable => ("not_writable", String::new()),
                            InstallError::Extract(d) => ("extract_failed", d),
                            InstallError::Uninstall(c) => ("uninstall_failed", format!("exit {c}")),
                            InstallError::Inner(c) => ("inner_failed", format!("exit {c}")),
                        };
                        push(
                            &webview,
                            serde_json::json!({
                                "type": "phase", "value": "error",
                                "code": code, "detail": detail,
                            }),
                        );
                    }
                }
            }
            _ => {}
        }
    });
}

fn dir_error_code(e: &logic::PathError) -> &'static str {
    match e {
        logic::PathError::Empty => "empty",
        logic::PathError::Relative => "relative",
        logic::PathError::Forbidden => "forbidden",
        logic::PathError::InvalidChars => "invalid_chars",
    }
}
