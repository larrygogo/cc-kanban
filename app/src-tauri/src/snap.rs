//! 吸边/缩略条：贴边判定的纯几何计算，以及折叠/展开/还原/解除吸附等窗口命令。

/// 吸边判定阈值（**逻辑**像素）：窗口边缘距工作区边缘不超过此值即认为贴边。
/// 调用方（lib.rs 的 Moved 处理）按显示器 scale_factor 换算成物理像素后再比较——
/// 与 STRIP_W_LOGICAL 同口径，保证不同缩放的屏幕上吸附手感一致。
#[cfg(not(target_os = "macos"))]
pub(crate) const SNAP_THRESHOLD: i32 = 20;
/// 竖条逻辑宽度（实际物理宽度 = 该值 * 显示器 scale_factor）。
/// 28 给 10px 圆点 + 内发光/描边留出足够边距，避免被 8px 圆角裁掉上下/左右。
const STRIP_W_LOGICAL: f64 = 28.0;

/// 贴纸正常态最小逻辑尺寸。**必须与 tauri.conf.json 的 minWidth/minHeight、App.tsx 的
/// SIZE_MIN_W/H 三处一致**（折叠时会临时放开，这里是恢复时设回的值）。
/// 高度按「至少完整显示两张会话卡」定：窗框+拖拽区+tab+底栏合计约 132px，
/// 标准密度单卡约 84px + 6px 间距，两张约 174px；330 在宽松密度(112%)下也够。
pub(crate) const STICKER_MIN_W: f64 = 360.0;
pub(crate) const STICKER_MIN_H: f64 = 330.0;

/// 矩形（物理像素），用于吸边判定的纯计算。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// 吸附的边（左/右/顶）。JS 侧序列化为 "left"/"right"/"top"。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    // 注：保持 pub（非 pub(crate)），因 pub fn edge_for_rect 返回 Option<Edge>。
    Left,
    Right,
    Top,
}

/// 判定窗口 `win` 是否贴在工作区 `work` 的左/右/顶边缘（阈值 `threshold`）。
/// 取在阈值内且最近的一边（平局按 左>右>顶 优先）；都不满足返回 None。纯函数，便于单测。
pub fn edge_for_rect(win: Rect, work: Rect, threshold: i32) -> Option<Edge> {
    let left_gap = (win.x - work.x).abs();
    let right_gap = ((work.x + work.w) - (win.x + win.w)).abs();
    let top_gap = (win.y - work.y).abs();
    let mut best: Option<(Edge, i32)> = None;
    for (edge, gap) in [
        (Edge::Left, left_gap),
        (Edge::Right, right_gap),
        (Edge::Top, top_gap),
    ] {
        if gap <= threshold && best.is_none_or(|(_, b)| gap < b) {
            best = Some((edge, gap));
        }
    }
    best.map(|(e, _)| e)
}

/// 两个矩形的相交面积（无重叠为 0）。用于判断窗口是否还落在某个显示器工作区内。纯函数。
pub fn intersection_area(a: Rect, b: Rect) -> i64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0 || h <= 0 {
        0
    } else {
        w as i64 * h as i64
    }
}

/// 把窗口左上角最小幅度地钳进工作区，使窗口完整落在 `work` 内，返回钳制后的 (x, y)。
/// 窗口比工作区还大时，左上角对齐工作区原点。纯函数，便于单测。
pub fn clamp_xy_to_work(win: Rect, work: Rect) -> (i32, i32) {
    let clamp_axis = |pos: i32, size: i32, wpos: i32, wsize: i32| -> i32 {
        if size >= wsize {
            wpos
        } else {
            pos.clamp(wpos, wpos + wsize - size)
        }
    };
    (
        clamp_axis(win.x, win.w, work.x, work.w),
        clamp_axis(win.y, win.h, work.y, work.h),
    )
}

/// 把窗口拉回可视区，防止「多显示器拔插/分辨率变化/拖到屏外」后贴纸消失在所有屏幕之外。
/// - `force=false`（救援）：仅当窗口与所有显示器工作区**完全无交集**时才移回主显示器，不打扰正常摆放。
/// - `force=true`（显式找回）：钳进「相交面积最大／主」显示器工作区，确保完整可见。
#[cfg(target_os = "windows")]
pub(crate) fn pull_on_screen(window: &tauri::WebviewWindow, force: bool) {
    let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    let win = Rect {
        x: pos.x,
        y: pos.y,
        w: size.width as i32,
        h: size.height as i32,
    };
    let Ok(monitors) = window.available_monitors() else {
        return;
    };
    if monitors.is_empty() {
        return;
    }
    let to_work = |m: &tauri::window::Monitor| {
        let wa = m.work_area();
        Rect {
            x: wa.position.x,
            y: wa.position.y,
            w: wa.size.width as i32,
            h: wa.size.height as i32,
        }
    };
    // 找与窗口相交面积最大的显示器工作区。
    let mut best: Option<(i64, Rect)> = None;
    for m in &monitors {
        let work = to_work(m);
        let area = intersection_area(win, work);
        if best.is_none_or(|(a, _)| area > a) {
            best = Some((area, work));
        }
    }
    let (best_area, best_work) = best.unwrap();
    // 救援模式下，只要还跟某个屏有交集就不动。
    if !force && best_area > 0 {
        return;
    }
    // 目标工作区：有交集就用相交最大的那个；完全在屏外则用主显示器（兜底用第一个）。
    let target = if best_area > 0 {
        best_work
    } else {
        window
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| to_work(&m))
            .unwrap_or(best_work)
    };
    let (x, y) = clamp_xy_to_work(win, target);
    if (x, y) != (win.x, win.y) {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// snap-changed 事件负载：当前检测到的吸附边（None 表示不贴边）。
#[cfg(not(target_os = "macos"))]
#[derive(Clone, serde::Serialize)]
pub(crate) struct SnapPayload {
    pub edge: Option<Edge>,
}

/// 竖条物理宽度：逻辑宽度 * 显示器缩放，至少 1px。
fn strip_width_phys(scale: f64) -> i32 {
    ((STRIP_W_LOGICAL * scale).round() as i32).max(1)
}

/// 仅在置顶状态实际变化时调用 set_always_on_top：避免在透明窗口上重复 SetWindowPos
/// 触发额外重绘闪烁（展开/收起/重测尺寸会反复进入这些命令，但置顶状态多数时候没变）。
fn set_top_if_changed(window: &tauri::WebviewWindow, desired: bool) -> Result<(), String> {
    if window.is_always_on_top().map_err(|e| e.to_string())? != desired {
        window
            .set_always_on_top(desired)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 取窗口当前所在显示器。开机自启早期 OS 可能尚未枚举完显示器，current_monitor() 会暂时返回 None，
/// 导致折叠/展开命令硬失败且前端无重试。回退顺序：当前屏 → 主屏 → 首个可用屏，尽量让 snap 命令仍能算位置。
fn window_monitor(window: &tauri::WebviewWindow) -> Result<tauri::window::Monitor, String> {
    if let Ok(Some(m)) = window.current_monitor() {
        return Ok(m);
    }
    if let Ok(Some(m)) = window.primary_monitor() {
        return Ok(m);
    }
    window
        .available_monitors()
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| "no monitor".to_string())
}

/// 真实光标是否在主窗口外接矩形内。用于展开态下判定是否该收回——DOM 的 mouseleave 在
/// 窗口缩放时会误报一串假 leave/enter，不可信；改问 GetCursorPos vs 窗口物理矩形。
/// 取不到坐标/尺寸时一律当作"在内"，避免误折叠。非 Windows 暂恒为 true（不收回）。
#[tauri::command]
pub(crate) fn cursor_over_window(window: tauri::WebviewWindow) -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut p) } == 0 {
            return true;
        }
        let (Ok(pos), Ok(sz)) = (window.outer_position(), window.outer_size()) else {
            return true;
        };
        p.x >= pos.x
            && p.x < pos.x + sz.width as i32
            && p.y >= pos.y
            && p.y < pos.y + sz.height as i32
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        true
    }
}

/// 鼠标左键当前是否按下。用于吸边：data-tauri-drag-region 的 OS 拖动循环里 webview 收不到
/// mouseup，前端改轮询此命令——真正松手(false)才吸附，避免拖拽中停顿被误判为松手。
/// 非 Windows 恒为 false（macOS 走 nspanel 无吸边）。
#[tauri::command]
pub(crate) fn pointer_left_down() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        // VK_LBUTTON = 0x01；返回值高位置 1（i16 为负）表示当前按下。
        unsafe { GetAsyncKeyState(0x01) < 0 }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// 交叉轴居中：让新长度 `new_len` 的窗以原窗口（起点 prev_start、长度 prev_len）中心对齐，
/// 再夹进工作区 [work_start, work_start+work_len) 内，避免居中后越界。纯函数便于单测。
pub(crate) fn center_on(
    prev_start: i32,
    prev_len: i32,
    new_len: i32,
    work_start: i32,
    work_len: i32,
) -> i32 {
    let centered = prev_start + (prev_len - new_len) / 2;
    centered.clamp(
        work_start,
        (work_start + work_len - new_len).max(work_start),
    )
}

/// 折叠成缩略条：贴到指定边，左/右为竖条、顶为横条。交叉轴以原窗口中心对齐
/// （吸顶=水平居中，吸左/右=垂直居中）。`extent` 是沿条主轴的逻辑长度，由前端按内容给出。
#[tauri::command]
pub(crate) fn snap_collapse(
    window: tauri::WebviewWindow,
    edge: Edge,
    extent: f64,
) -> Result<(), String> {
    let extent = extent.clamp(1.0, 20000.0); // 钳上界，防 *scale 后 f64→i32 回绕
    let m = window_monitor(&window)?;
    let wa = m.work_area();
    let scale = m.scale_factor();
    let strip = strip_width_phys(scale); // 条的厚度（物理像素）
    let ext = ((extent * scale).round() as i32).max(1); // 条的主轴长度
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let sz = window.outer_size().map_err(|e| e.to_string())?;
    let (cur_w, cur_h) = (sz.width as i32, sz.height as i32);
    let (ww, wh) = (wa.size.width as i32, wa.size.height as i32);
    // (min_w, min_h, w, h, x, y)
    let (min_w, min_h, w, h, x, y) = match edge {
        Edge::Left => (
            strip,
            0,
            strip,
            ext,
            wa.position.x,
            center_on(pos.y, cur_h, ext, wa.position.y, wh),
        ),
        Edge::Right => (
            strip,
            0,
            strip,
            ext,
            wa.position.x + ww - strip,
            center_on(pos.y, cur_h, ext, wa.position.y, wh),
        ),
        Edge::Top => (
            0,
            strip,
            ext,
            strip,
            center_on(pos.x, cur_w, ext, wa.position.x, ww),
            wa.position.y,
        ),
    };
    // 放开最小宽高限制（tauri.conf 配了 minWidth=360/minHeight=240），否则缩不到缩略条尺寸。
    window
        .set_min_size(Some(tauri::PhysicalSize::new(min_w as u32, min_h as u32)))
        .map_err(|e| e.to_string())?;
    window
        .set_size(tauri::PhysicalSize::new(w as u32, h as u32))
        .map_err(|e| e.to_string())?;
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    // 吸附态强制置顶，保证缩略条始终可见（仅状态变化时设，避免重复触发重绘闪烁）。
    set_top_if_changed(&window, true)?;
    Ok(())
}

/// 偷看展开成全尺寸（仍贴边、保持置顶）：宽高恢复为记住的正常尺寸。
#[tauri::command]
pub(crate) fn snap_expand(
    window: tauri::WebviewWindow,
    edge: Edge,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let (width, height) = (width.clamp(1.0, 20000.0), height.clamp(1.0, 20000.0)); // 钳上界防回绕
    let m = window_monitor(&window)?;
    let wa = m.work_area();
    let scale = m.scale_factor();
    let phys_w = ((width * scale).round() as i32).max(1);
    let phys_h = ((height * scale).round() as u32).max(1);
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let sz = window.outer_size().map_err(|e| e.to_string())?;
    let (cur_w, cur_h) = (sz.width as i32, sz.height as i32);
    let (ww, wh) = (wa.size.width as i32, wa.size.height as i32);
    // 交叉轴以当前（缩略条）中心对齐展开，与折叠态保持同一中心，不跳回左/上对齐。
    let (x, y) = match edge {
        Edge::Left => (
            wa.position.x,
            center_on(pos.y, cur_h, phys_h as i32, wa.position.y, wh),
        ),
        Edge::Right => (
            wa.position.x + ww - phys_w,
            center_on(pos.y, cur_h, phys_h as i32, wa.position.y, wh),
        ),
        Edge::Top => (
            center_on(pos.x, cur_w, phys_w, wa.position.x, ww),
            wa.position.y,
        ),
    };
    // 恢复正常最小尺寸（与 tauri.conf minWidth/minHeight 一致）再展开，就地放大到贴边位置。
    window
        .set_min_size(Some(tauri::LogicalSize::new(STICKER_MIN_W, STICKER_MIN_H)))
        .map_err(|e| e.to_string())?;
    window
        .set_size(tauri::PhysicalSize::new(phys_w as u32, phys_h))
        .map_err(|e| e.to_string())?;
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    set_top_if_changed(&window, true)?;
    Ok(())
}

/// 恢复正常浮动：尺寸设回记住的逻辑宽高，位置维持用户当前拖到的地方。
#[tauri::command]
pub(crate) fn snap_restore(
    window: tauri::WebviewWindow,
    width: f64,
    height: f64,
    pinned: bool,
) -> Result<(), String> {
    // 与 snap_collapse/snap_expand 一致地钳制上界，防异常大值(localStorage 被改坏)经 set_size 设出极端窗口。
    let (width, height) = (width.clamp(1.0, 20000.0), height.clamp(1.0, 20000.0));
    // 恢复正常最小尺寸限制，再设回记住的宽高，置顶还原为用户的 pin 偏好。
    window
        .set_min_size(Some(tauri::LogicalSize::new(STICKER_MIN_W, STICKER_MIN_H)))
        .map_err(|e| e.to_string())?;
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|e| e.to_string())?;
    set_top_if_changed(&window, pinned)?;
    // set_size 走 SetWindowPos(SWP_NOMOVE)：位置约束子类(WM_WINDOWPOSCHANGING)与 Moved 钳位都不触发。
    // 从右/顶边「遗留细条几何」还原放大后(宽/高变大、左上角不动)窗口可能越界出屏；显式拉回工作区内。
    #[cfg(target_os = "windows")]
    pull_on_screen(&window, true);
    Ok(())
}

/// 拖角缩放触发的「解除吸附」：保留用户当前拖出的尺寸/位置，只复位最小尺寸与置顶（按 pin 偏好）。
/// 解除后窗口即普通浮动窗口，再拖到屏幕边缘仍会被吸附逻辑重新吸附。
#[tauri::command]
pub(crate) fn unsnap(window: tauri::WebviewWindow, pinned: bool) -> Result<(), String> {
    window
        .set_min_size(Some(tauri::LogicalSize::new(STICKER_MIN_W, STICKER_MIN_H)))
        .map_err(|e| e.to_string())?;
    set_top_if_changed(&window, pinned)?;
    Ok(())
}
