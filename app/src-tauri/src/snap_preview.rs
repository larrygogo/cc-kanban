//! W-2 原生吸附预览窗（仅 Windows 实装）：拖拽中候选边出现时，在**屏幕边缘**显示一条
//! 原生预览条（28px 厚、贴候选边、主轴长度 = 缩略条落点长度），移开候选边/松手即消失。
//!
//! 此前的预览是画在贴纸窗口内缘的 `.snap-ghost` 虚线框——幽灵条贴在窗口自己身上，
//! 与「落点在屏幕边缘」的直觉差一层。原生悬浮条与真实落点同位同形，且
//! set_ignore_cursor_events(true) 不吃鼠标、skip_taskbar 不占任务栏、focused(false)
//! 不抢焦点。创建失败即熔断（BROKEN），Moved 处理据 update 返回值让前端回退 .snap-ghost。
//!
//! 两个 Tauri 命令需全平台注册（generate_handler 不做 cfg 分流）：非 Windows 为空操作
//! 桩——前端本就不会在非 Windows 调用（吸边整体仅 Windows，W-18）。

/// 预览窗 label：main.tsx 按它路由到 SnapPreview 视图（一整条幽灵色块，无其他 UI）。
#[cfg(target_os = "windows")]
const LABEL: &str = "snap-preview";

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use tauri::{Emitter, Manager};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    use super::LABEL;
    use crate::snap::{Edge, Rect};

    /// 前端给的缩略条主轴逻辑长度（stripExtent，f64 的位模式；0 = 尚未配置）。
    /// 配置走 snap_preview_set_extent 命令，随会话数变化推送，不在拖拽热路径上。
    static EXTENT_BITS: AtomicU64 = AtomicU64::new(0);
    /// 预览页已挂载握手（snap_preview_ready）：此前窗口保持 hidden——WebView2 首帧
    /// 未上屏时 show 会露白/透明空框，宁可第一次拖拽晚一拍显示，也不闪。
    static READY: AtomicBool = AtomicBool::new(false);
    /// 当前是否希望预览条可见（候选边存在）。READY 握手/建窗完成时按它决定要不要补 show。
    static WANT_VISIBLE: AtomicBool = AtomicBool::new(false);
    /// 建窗失败即熔断：本次运行不再重试（失败原因多为系统级，拖拽中反复重试只会反复
    /// 卡），update 返回 false，前端照常画 .snap-ghost。
    static BROKEN: AtomicBool = AtomicBool::new(false);
    /// 建窗子线程是否已在跑（只起一次）。
    static CREATING: AtomicBool = AtomicBool::new(false);
    /// 最近一次 update 算出的预览条几何（物理像素）：建窗子线程在建好后应用它——
    /// 建窗是异步的，建好那一刻的几何只能从这里拿。
    static LATEST_RECT: std::sync::Mutex<Option<Rect>> = std::sync::Mutex::new(None);

    /// 最近一次候选边：建窗失败重发 snap-changed 时要用——Moved 只在边**变化**时
    /// emit（W-9），不重发的话失败的那次拖拽整段无预览（ghost 已被 native=true 收起）。
    static LAST_EDGE: std::sync::Mutex<Option<Edge>> = std::sync::Mutex::new(None);

    pub fn set_extent(extent: f64) {
        EXTENT_BITS.store(
            extent.clamp(1.0, crate::snap::SIZE_MAX_LOGICAL).to_bits(),
            Ordering::Relaxed,
        );
    }

    /// 预览页挂载握手：标记 READY，若此刻候选边仍在（WANT_VISIBLE）则补 show——
    /// 几何在 update/建窗线程里已就位，这里只负责显影。
    pub fn on_ready(window: &tauri::WebviewWindow) {
        if window.label() != LABEL {
            return;
        }
        READY.store(true, Ordering::Relaxed);
        if WANT_VISIBLE.load(Ordering::Relaxed) {
            let _ = window.show();
        }
    }

    /// 应用几何 + 按 READY 门显影（窗口已存在时的每帧驱动）。
    fn place(w: &tauri::WebviewWindow, r: Rect) {
        let _ = w.set_size(tauri::PhysicalSize::new(r.w.max(1) as u32, r.h.max(1) as u32));
        let _ = w.set_position(tauri::PhysicalPosition::new(r.x, r.y));
        // 页面就绪前不 show（防白帧）；已可见时重复 show 是无谓的 SetWindowPos，跳过。
        if READY.load(Ordering::Relaxed) && !w.is_visible().unwrap_or(false) {
            let _ = w.show();
        }
        // 抬到置顶带最上层：贴纸被拖动时是**激活的**置顶窗，预览窗 focused(false) 永不
        // 激活，z 序沉在贴纸之下——候选边与窗口重叠时预览条被盖住（实拍：「ghost 在
        // 贴纸下面」）。每帧落位后重新 SetWindowPos(TOPMOST) 抬回；NOACTIVATE 保拖拽
        // 循环不丢焦点（焦点易主会让 OS 提前结束拖动，见 create_window 注释）。
        if let Ok(h) = w.hwnd() {
            unsafe {
                SetWindowPos(
                    h.0 as *mut std::ffi::c_void,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    /// 建窗子线程：同步 build 绝不能跑在 Moved 事件回调里（实测 on_window_event 内
    /// build() 直接不返回、整个事件泵被卡死——设置/引导窗同款理由一律子线程建窗，
    /// 见 window.rs open_onboarding 注释）。建好后应用最新几何；此刻 READY 必然为
    /// false（页面刚起步加载），不 show，显影交给 ready 握手或下一帧 Moved。
    fn create_window(app: tauri::AppHandle) {
        // 与贴纸主窗同款的 transparent（tauri.conf.json 的 main 已验证 WebView2 透明
        // 可行）。visible(false) 创建、READY 握手后才 show：加载期不闪白框。
        // focused(false) 不抢焦点——拖拽循环里焦点易主会让 OS 提前结束拖动。
        // shadow(false) 与 main 一致（tauri.conf.json 的 main 也是 shadow:false）——缺省
        // shadow:true 会让 tao 走「无边框+阴影」路径：WM_NCCALCSIZE 按不可见边框 insets
        // （150% 缩放下左/右/底各 11、顶 2）收缩 client 区，set_inner_size 再反向补偿
        // 把外框撑大——实测外框 55px vs 请求 42px，且内容区偏移 (+11,+2)，预览条比真实
        // 缩略条厚一圈还不贴边。预览条只是幽灵色块，不需要 DWM 阴影，关掉后
        // winrect==client==请求值，与 main 的行为一致。
        let builder = tauri::WebviewWindowBuilder::new(
            &app,
            LABEL,
            tauri::WebviewUrl::App("index.html".into()),
        )
        .title("")
        .decorations(false)
        .shadow(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .visible(false)
        .inner_size(28.0, 28.0);
        match builder.build() {
            Ok(w) => {
                // 点击穿透：预览条只是视觉提示，绝不吃鼠标（拖拽的落点判定不受影响）。
                let _ = w.set_ignore_cursor_events(true);
                if let Some(r) = *LATEST_RECT.lock().unwrap_or_else(|e| e.into_inner()) {
                    place(&w, r);
                }
            }
            Err(e) => {
                eprintln!("[snap-preview] 创建预览窗失败，本次运行回退 .snap-ghost: {e}");
                BROKEN.store(true, Ordering::Relaxed);
                // 本次候选边出现时 update 已返回 true、前端收起了 ghost——Moved 只在
                // 边变化时重发 snap-changed（W-9），不主动补一发的话这次拖拽整段无预览。
                // 立刻以 native=false 重发，前端当次拖拽即回退窗口内缘 ghost。
                let edge = *LAST_EDGE.lock().unwrap_or_else(|e| e.into_inner());
                let _ = app.emit("snap-changed", crate::snap::SnapPayload { edge, native: false });
            }
        }
        CREATING.store(false, Ordering::Relaxed);
    }

    /// 当前缓存的预览条主轴逻辑长度；前端未推送过时退到 48（stripExtent 的下限）。
    fn extent_logical() -> f64 {
        let bits = EXTENT_BITS.load(Ordering::Relaxed);
        if bits == 0 {
            48.0
        } else {
            f64::from_bits(bits)
        }
    }

    /// Moved 处理驱动：候选边存在则定位/显影预览条并返回 true（前端据此收起
    /// .snap-ghost）；候选边消失（None）则隐藏。仅在预览窗不可用时返回 false。
    ///
    /// `win`/`work` 为物理像素：`work` 是窗口相交面积最大的显示器工作区（与
    /// snap_collapse 落点同口径）；`scale` 是该显示器缩放（阈值/厚度换算与 snap.rs
    /// 常量同口径）。
    pub fn update(
        app: &tauri::AppHandle,
        edge: Option<Edge>,
        win: Rect,
        work: Rect,
        scale: f64,
    ) -> bool {
        let Some(edge) = edge else {
            *LAST_EDGE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            hide(app);
            return !BROKEN.load(Ordering::Relaxed);
        };
        *LAST_EDGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(edge);
        WANT_VISIBLE.store(true, Ordering::Relaxed);
        let r = crate::snap::preview_strip_rect(edge, win, work, extent_logical(), scale);
        *LATEST_RECT.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        if let Some(w) = app.get_webview_window(LABEL) {
            place(&w, r);
        } else if !BROKEN.load(Ordering::Relaxed) && !CREATING.swap(true, Ordering::Relaxed) {
            // 首次出现候选边：起子线程懒建窗（不能在 Moved 回调里同步 build，见
            // create_window 注释）。建好前的几十~几百毫秒没有原生条——返回 true 让
            // 前端也收起 ghost（避免建好后双预览并存）；若建窗失败（BROKEN），create_window
            // 会立刻重发 native=false 让前端当次拖拽即回退 ghost（见那里的注释）。
            let app = app.clone();
            std::thread::spawn(move || create_window(app));
        }
        true
    }

    /// 隐藏预览条（幂等）。调用点：候选边消失（update 的 None 分支）、
    /// WM_EXITSIZEMOVE（拖拽/缩放手势结束的兜底）、主窗失焦、snap_collapse/
    /// snap_restore/unsnap 落地。应用退出时窗口随 App 销毁，不留孤儿窗。
    pub fn hide(app: &tauri::AppHandle) {
        WANT_VISIBLE.store(false, Ordering::Relaxed);
        if let Some(w) = app.get_webview_window(LABEL) {
            let _ = w.hide();
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::{hide, on_ready, set_extent, update};

#[tauri::command]
#[cfg(target_os = "windows")]
pub(crate) fn snap_preview_set_extent(extent: f64) {
    set_extent(extent);
}

#[tauri::command]
#[cfg(target_os = "windows")]
pub(crate) fn snap_preview_ready(window: tauri::WebviewWindow) {
    on_ready(&window);
}

// 非 Windows 空操作桩（见文件头：命令需全平台注册，前端不会调到）。
#[tauri::command]
#[cfg(not(target_os = "windows"))]
pub(crate) fn snap_preview_set_extent(_extent: f64) {}

#[tauri::command]
#[cfg(not(target_os = "windows"))]
pub(crate) fn snap_preview_ready(_window: tauri::WebviewWindow) {}
