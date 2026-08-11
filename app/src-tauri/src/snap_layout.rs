//! 标题栏按钮的原生覆盖窗（仅 Windows，其余平台空实现）：最小化 / 最大化 / 关闭。
//!
//! 无边框窗口里 WebView2 盖满整个客户区，鼠标下的 `WM_NCHITTEST` 发给它的子窗口而
//! 不是本应用的顶层窗口——顶层 wndproc 永远收不到，返回 `HTMAXBUTTON` 等命中码无从
//! 谈起。通行做法（社区 tauri-plugin-frame / tauri-plugin-snap-layout 同理）：在自绘
//! 按钮的矩形上各盖一个**不可见的原生子窗口**，由它的 `WM_NCHITTEST` 返回对应命中码：
//! - 最大化钮 `HTMAXBUTTON`：DWM 据此弹出 Win11 Snap Layouts 悬停浮窗（本模块起点）；
//! - 最小化 `HTMINBUTTON` / 关闭 `HTCLOSE`：与系统按钮同语义，三颗钮行为一致。
//!
//! 副作用与补偿：矩形内的鼠标事件从此进不了 WebView——
//! - hover 视觉：以 `caption-hover` 事件（{kind, hover}）通知前端补类名（`:hover` 不触发）；
//! - 点击：由本模块发 `WM_SYSCOMMAND` 代办（最小化/最大化还原/关闭），与原生按钮同一条
//!   系统代码路径（关闭走 WM_CLOSE → tauri CloseRequested，前端 on_window_event 清理不变）。
//!
//! 覆盖窗**必须是普通（非分层）子窗口**：曾用 WS_EX_LAYERED + alpha=1 做隐身，DWM 的
//! Snap 浮窗探测（ChildWindowFromPointEx 族跳过分层/透明子窗）命不中它。隐身靠「从不
//! 绘制」：WebView2 经 DirectComposition 合成内容，压在这些 GDI 子窗之上。
//!
//! 前端（WindowControls.tsx）在挂载与窗口 resize/DPI 变化后上报三颗按钮的**物理像素**
//! 矩形（客户区坐标系）；卸载时上报 `None` 销毁全部覆盖窗。

/// 单颗按钮的覆盖矩形，物理像素、相对窗口客户区左上角。
#[derive(serde::Deserialize, Clone)]
pub(crate) struct CaptionRect {
    /// "min" | "max" | "close"（未知值忽略）。
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// 前端上报标题栏按钮矩形（`None` 或空列表 = 按钮卸载，销毁全部覆盖窗）。
/// 同步 command 仅做窗口句柄转发（微秒级窗口操作，符合主线程纪律）；
/// 实际 HWND 操作 dispatch 到主线程——子窗口的 wndproc 线程亲和必须落在创建线程。
#[tauri::command]
pub(crate) fn set_caption_overlay_rects(
    window: tauri::WebviewWindow,
    rects: Option<Vec<CaptionRect>>,
) {
    #[cfg(target_os = "windows")]
    {
        let w = window.clone();
        let _ = window.run_on_main_thread(move || imp::update_on_main(&w, rects));
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, rects);
}

#[cfg(target_os = "windows")]
mod imp {
    use super::CaptionRect;
    use std::sync::Once;
    use tauri::Emitter;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, GetWindowLongPtrW,
        IsZoomed, LoadCursorW, RegisterClassW, SendMessageW, SetCursor, SetWindowLongPtrW,
        SetWindowPos, GWLP_USERDATA, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HWND_TOP, IDC_ARROW,
        SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        WM_DESTROY, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE,
        WM_SETCURSOR, WM_SYSCOMMAND, WNDCLASSW, WS_CHILD, WS_VISIBLE,
    };

    /// "MeowoCaption\0"（UTF-16）。类名同时是 FindWindowExW 枚举既有覆盖窗的钥匙。
    static CLASS_NAME: [u16; 13] = [
        b'M' as u16, b'e' as u16, b'o' as u16, b'w' as u16, b'o' as u16, b'C' as u16,
        b'a' as u16, b'p' as u16, b't' as u16, b'i' as u16, b'o' as u16, b'n' as u16, 0,
    ];

    /// 按钮种类 → 命中码 / 点击的系统命令。
    #[derive(Clone, Copy, PartialEq)]
    pub(super) enum Kind {
        Min,
        Max,
        Close,
    }

    impl Kind {
        fn parse(kind: &str) -> Option<Kind> {
            match kind {
                "min" => Some(Kind::Min),
                "max" => Some(Kind::Max),
                "close" => Some(Kind::Close),
                _ => None,
            }
        }
        fn hit_code(self) -> u32 {
            match self {
                Kind::Min => HTMINBUTTON,
                Kind::Max => HTMAXBUTTON,
                Kind::Close => HTCLOSE,
            }
        }
        fn event_name(self) -> &'static str {
            match self {
                Kind::Min => "min",
                Kind::Max => "max",
                Kind::Close => "close",
            }
        }
    }

    /// 挂在覆盖窗 GWLP_USERDATA 上的状态。hover 去抖：hit-test 探测连发，只在
    /// 进入/离开的边沿各发一次事件。
    struct OverlayState {
        window: tauri::WebviewWindow,
        parent: HWND,
        kind: Kind,
        hover: bool,
    }

    fn register_class_once() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(overlay_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: std::ptr::null_mut(),
                // 光标在 WM_SETCURSOR 里显式给箭头（覆盖窗自己没有客户区语义）。
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: CLASS_NAME.as_ptr(),
            };
            RegisterClassW(&wc);
        });
    }

    /// 枚举 parent 下本类名的既有覆盖窗，按 kind 认领。
    unsafe fn find_overlay(parent: HWND, kind: Kind) -> HWND {
        let mut child: HWND = std::ptr::null_mut();
        loop {
            child = FindWindowExW(parent, child, CLASS_NAME.as_ptr(), std::ptr::null());
            if child.is_null() {
                return std::ptr::null_mut();
            }
            let state = GetWindowLongPtrW(child, GWLP_USERDATA) as *mut OverlayState;
            if !state.is_null() && (*state).kind == kind {
                return child;
            }
        }
    }

    /// 主线程上创建/移动/销毁覆盖窗。矩形上报走前端渲染节奏（挂载 + resize/DPI 变化），
    /// 不必逐帧跟随——按钮在客户区里的位置只随窗口宽度变化。
    pub(super) fn update_on_main(window: &tauri::WebviewWindow, rects: Option<Vec<CaptionRect>>) {
        let Ok(parent) = window.hwnd() else { return };
        let parent = parent.0 as HWND;
        let rects = rects.unwrap_or_default();
        unsafe {
            for kind in [Kind::Min, Kind::Max, Kind::Close] {
                let rect = rects
                    .iter()
                    .find(|r| Kind::parse(&r.kind) == Some(kind));
                let existing = find_overlay(parent, kind);
                let Some(r) = rect else {
                    if !existing.is_null() {
                        DestroyWindow(existing);
                    }
                    continue;
                };
                if !existing.is_null() {
                    // HWND_TOP：必须压在 WebView2 兄弟子窗之上，否则命中测试又回到 WebView。
                    SetWindowPos(existing, HWND_TOP, r.x, r.y, r.w, r.h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    continue;
                }
                register_class_once();
                let hwnd = CreateWindowExW(
                    0,
                    CLASS_NAME.as_ptr(),
                    std::ptr::null(),
                    WS_CHILD | WS_VISIBLE,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    parent,
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    continue;
                }
                let state = Box::into_raw(Box::new(OverlayState {
                    window: window.clone(),
                    parent,
                    kind,
                    hover: false,
                }));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                SetWindowPos(hwnd, HWND_TOP, r.x, r.y, r.w, r.h, SWP_NOACTIVATE);
            }
        }
    }

    fn emit_hover(state: &OverlayState, hover: bool) {
        // 只发给本窗口：事件语义是「你的某颗标题钮被悬停」，其他窗口无关。
        let _ = state.window.emit_to(
            state.window.label(),
            "caption-hover",
            serde_json::json!({ "kind": state.kind.event_name(), "hover": hover }),
        );
    }

    unsafe extern "system" fn overlay_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState;
        match msg {
            // 整个覆盖窗就是对应的系统标题钮——最大化钮的 Snap Layouts 浮窗由 DWM 据此弹出。
            // hover 进入判定放这而不是 WM_NCMOUSEMOVE：浮窗悬停期间鼠标静止，
            // 持续到达的只有 hit-test 探测；误报由 TrackMouseEvent 的 WM_NCMOUSELEAVE 纠正。
            WM_NCHITTEST => {
                if !state.is_null() {
                    if !(*state).hover {
                        (*state).hover = true;
                        emit_hover(&*state, true);
                        // 非客户区没有自动的离开通知，显式订阅 WM_NCMOUSELEAVE。
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE | TME_NONCLIENT,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        TrackMouseEvent(&mut tme);
                    }
                    return (*state).kind.hit_code() as LRESULT;
                }
                HTMAXBUTTON as LRESULT
            }
            // 吞掉按下，避免 DefWindowProc 的非客户区默认行为。
            WM_NCLBUTTONDOWN => 0,
            WM_NCLBUTTONUP => {
                if !state.is_null() {
                    // 走系统命令而非 tauri API：同线程 SendMessage 直达父窗默认过程，
                    // 与原生按钮同一条代码路径（含 Snap 复位、CloseRequested 等系统语义）。
                    let parent = (*state).parent;
                    let cmd = match (*state).kind {
                        Kind::Min => SC_MINIMIZE,
                        Kind::Max => {
                            if IsZoomed(parent) != 0 { SC_RESTORE } else { SC_MAXIMIZE }
                        }
                        Kind::Close => SC_CLOSE,
                    };
                    SendMessageW(parent, WM_SYSCOMMAND, cmd as WPARAM, 0);
                }
                0
            }
            WM_NCMOUSELEAVE => {
                if !state.is_null() && (*state).hover {
                    (*state).hover = false;
                    emit_hover(&*state, false);
                }
                0
            }
            // 非客户区命中没有类光标兜底，不处理会沿用上一个窗口留下的光标（如文本梁）。
            WM_SETCURSOR => {
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW));
                1
            }
            WM_DESTROY => {
                if !state.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(state));
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
