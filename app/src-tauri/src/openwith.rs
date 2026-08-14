//! Windows「打开方式」集成：SHAssocEnumHandlers 按扩展名枚举系统推荐的应用
//! （与 Explorer 右键「打开方式」同源，装没装 VS Code 都以本机真实关联为准），
//! 图标经 SHDefExtractIcon + GDI 抓位图转 PNG data URL 随清单下发。
//!
//! 结果按扩展名进程内缓存：注册表关联在应用运行期间基本不变，而枚举 + 提图标
//! 一次要几十毫秒级；缓存后同类型文件秒开菜单。
//!
//! 打开动作：处理器标识（GetName）实测都是真实 exe 路径（本机取证连 Store 记事本
//! 都是 WindowsApps 内可直接执行的实 exe），**优先直接带参 spawn**——绕开关联层的
//! 「声明类型」检查（Store 记事本没声明 .ts 等扩展名，走 IAssocHandler::Invoke 会被
//! 系统弹「选择应用」对话框而不是打开）。标识不是实文件或 spawn 被拒（ACL）时才
//! 回退 Invoke。前端回传的标识必须先在**重新枚举出的系统清单**里命中才会执行，
//! 不构成任意程序执行面。

use base64::Engine as _;
use meowo_protocol::ipc::FileOpenerDto;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IDataObject, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::{
    PERCEIVED_TYPE_AUDIO, PERCEIVED_TYPE_IMAGE, PERCEIVED_TYPE_VIDEO,
};
use windows::Win32::UI::Shell::{
    AssocGetPerceivedType, IAssocHandler, IShellItem, SHAssocEnumHandlers,
    SHCreateItemFromParsingName, SHDefExtractIconW, ASSOC_FILTER_RECOMMENDED, BHID_DataObject,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

/// 菜单最多列的应用数：Explorer 的推荐清单通常 2~6 个，超过 8 个就是杂讯。
const MAX_OPENERS: usize = 8;
/// 提取图标的像素尺寸：菜单 14px 显示，取 32px 留高分屏余量。
const ICON_SIZE: i32 = 32;

/// 每次调用配一个 COM 生命周期：blocking 池线程不保证初始化过 COM。
/// S_OK/S_FALSE 都必须配对 CoUninitialize；RPC_E_CHANGED_MODE（Err）不配对。
struct ComGuard(bool);
impl ComGuard {
    fn new() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// 小写点前缀扩展名（".rs"）；无扩展名/点文件返回空串（无从枚举关联）。
fn ext_key(path: &Path) -> String {
    path.extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

/// 读取并释放 shell 接口返回的 CoTaskMem 字符串。
unsafe fn take_co_string(ptr: windows::core::PWSTR) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { ptr.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(ptr.0 as *const _)) };
    text
}

/// 枚举适合打开该文件的系统推荐应用（带缓存）。目录/无扩展名返回空。
///
/// 不照单全收注册表关联：`.ts` 这类扩展名在系统眼里是 MPEG 视频流，推荐清单全是
/// 播放器，但本面板里它是 TypeScript 源码。故先嗅探内容——二进制文件（真媒体）
/// 保持系统原清单；文本文件若扩展名被系统感知为音/视频/图片，则整体换用 `.txt`
/// 的推荐清单（文本编辑器），其余文本合并 `.txt` 清单兜底，并把已知代码编辑器
/// 提到最前（主键取首项，不该落到播放器/记事本上）。
pub(crate) fn list_for_path(path: &Path) -> Vec<FileOpenerDto> {
    let ext = ext_key(path);
    if ext.is_empty() {
        return Vec::new();
    }
    let text = is_text_file(path);
    let key = format!("{ext}|{text}");
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<FileOpenerDto>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(&key) {
        return cached.clone();
    }
    let list = if text {
        let mut merged = if ext_perceived_as_media(&ext) {
            let txt = enumerate_openers(".txt");
            if txt.is_empty() { enumerate_openers(&ext) } else { txt }
        } else {
            let mut base = enumerate_openers(&ext);
            for opener in enumerate_openers(".txt") {
                if base.len() >= MAX_OPENERS {
                    break;
                }
                if !base.iter().any(|o| o.id == opener.id) {
                    base.push(opener);
                }
            }
            base
        };
        // 稳定分区：已知代码编辑器保持相对序提前，其余顺延。
        merged.sort_by_key(|o| usize::from(!is_code_editor(&o.id)));
        merged
    } else {
        enumerate_openers(&ext)
    };
    cache.lock().unwrap().insert(key, list.clone());
    list
}

/// 前 8KB 无 NUL 即视为文本（与 fsutil/git 的二进制嗅探同一判据）；读不了按二进制算。
fn is_text_file(path: &Path) -> bool {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else { return false };
    let mut buf = Vec::with_capacity(8 * 1024);
    if file.take(8 * 1024).read_to_end(&mut buf).is_err() {
        return false;
    }
    !buf.contains(&0)
}

/// 系统把该扩展名感知为音频/视频/图片（媒体清单对文本场景失真的信号）。
fn ext_perceived_as_media(ext: &str) -> bool {
    let wide: Vec<u16> = ext.encode_utf16().chain([0]).collect();
    let mut perceived = Default::default();
    let mut flag = Default::default();
    unsafe { AssocGetPerceivedType(PCWSTR(wide.as_ptr()), &mut perceived, &mut flag, None) }
        .is_ok()
        && [PERCEIVED_TYPE_AUDIO, PERCEIVED_TYPE_VIDEO, PERCEIVED_TYPE_IMAGE].contains(&perceived)
}

/// handler 标识（exe 路径）是否指向已知代码编辑器：文本文件的清单里让它们排最前。
fn is_code_editor(id: &str) -> bool {
    let lower = id.to_lowercase();
    ["code.exe", "cursor.exe", "notepad++.exe", "sublime_text.exe", "idea64.exe", "webstorm64.exe", "zed.exe"]
        .iter()
        .any(|exe| lower.ends_with(exe))
}

fn enumerate_openers(ext: &str) -> Vec<FileOpenerDto> {
    let _com = ComGuard::new();
    let wide: Vec<u16> = ext.encode_utf16().chain([0]).collect();
    let mut out: Vec<FileOpenerDto> = Vec::new();
    unsafe {
        let Ok(enumerator) = SHAssocEnumHandlers(PCWSTR(wide.as_ptr()), ASSOC_FILTER_RECOMMENDED)
        else {
            return out;
        };
        'outer: loop {
            let mut handlers: [Option<IAssocHandler>; 8] = Default::default();
            let mut fetched = 0u32;
            if enumerator.Next(&mut handlers, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            for handler in handlers.into_iter().take(fetched as usize).flatten() {
                let Some(id) = handler.GetName().ok().and_then(|p| take_co_string(p)) else {
                    continue;
                };
                // 同一应用可能以多个注册路径出现（ProgId + Applications 键），按 id 去重。
                if out.iter().any(|o| o.id == id) {
                    continue;
                }
                let name = handler
                    .GetUIName()
                    .ok()
                    .and_then(|p| take_co_string(p))
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| id.clone());
                let icon = handler_icon_data_url(&handler);
                out.push(FileOpenerDto { id, name, icon });
                if out.len() >= MAX_OPENERS {
                    break 'outer;
                }
            }
        }
    }
    out
}

/// 用系统清单里名字等于 handler_id 的处理器打开文件。
/// 查找来源必须与 list_for_path 一致：文本文件的清单可能混入 `.txt` 来源的处理器
/// （记事本等），只搜本扩展名会漏——本扩展名与 `.txt` 两个来源都搜。
pub(crate) fn invoke(path: &Path, handler_id: &str) -> Result<(), String> {
    let _com = ComGuard::new();
    let ext = ext_key(path);
    if ext.is_empty() {
        return Err("该文件类型没有关联应用".into());
    }
    let Some(handler) = find_handler(&ext, handler_id)
        .or_else(|| (ext != ".txt").then(|| find_handler(".txt", handler_id)).flatten())
    else {
        return Err("该打开方式已不可用".into());
    };
    // 标识是实 exe：直接带参启动（见模块头——关联层对未声明的扩展名会弹选择器）。
    // 必须在上面的清单命中之后才 spawn：不执行前端传来的任意路径。
    if Path::new(handler_id).is_file()
        && std::process::Command::new(handler_id)
            .arg(crate::fsutil::display_path(path))
            .spawn()
            .is_ok()
    {
        return Ok(());
    }
    invoke_handler(&handler, path)
}

/// 在某扩展名的系统推荐清单里按名字（exe 路径/标识）找处理器。
fn find_handler(ext: &str, handler_id: &str) -> Option<IAssocHandler> {
    let wide: Vec<u16> = ext.encode_utf16().chain([0]).collect();
    unsafe {
        let enumerator =
            SHAssocEnumHandlers(PCWSTR(wide.as_ptr()), ASSOC_FILTER_RECOMMENDED).ok()?;
        loop {
            let mut handlers: [Option<IAssocHandler>; 8] = Default::default();
            let mut fetched = 0u32;
            if enumerator.Next(&mut handlers, Some(&mut fetched)).is_err() || fetched == 0 {
                return None;
            }
            for handler in handlers.into_iter().take(fetched as usize).flatten() {
                let name = handler.GetName().ok().and_then(|p| take_co_string(p));
                if name.as_deref() == Some(handler_id) {
                    return Some(handler);
                }
            }
        }
    }
}

fn invoke_handler(handler: &IAssocHandler, path: &Path) -> Result<(), String> {
    unsafe {
        // \\?\ 扩展前缀路径 SHCreateItemFromParsingName 不认，去前缀再解析。
        let display = crate::fsutil::display_path(path);
        let wide_path: Vec<u16> = display.encode_utf16().chain([0]).collect();
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None)
            .map_err(|e| e.to_string())?;
        let data: IDataObject =
            item.BindToHandler(None, &BHID_DataObject).map_err(|e| e.to_string())?;
        handler.Invoke(&data).map_err(|e| e.to_string())
    }
}

/// 处理器图标 → PNG data URL。UWP 资源图标（"@{…}"）等提取失败时 None。
fn handler_icon_data_url(handler: &IAssocHandler) -> Option<String> {
    unsafe {
        let mut loc_ptr = windows::core::PWSTR::null();
        let mut index = 0i32;
        handler.GetIconLocation(&mut loc_ptr, &mut index).ok()?;
        let location = take_co_string(loc_ptr)?;
        if location.trim().is_empty() || location.starts_with('@') {
            return None;
        }
        let wide: Vec<u16> = location.encode_utf16().chain([0]).collect();
        let mut hicon = HICON::default();
        // nIconSize 低字 = 大图标尺寸；只要大图标（高字 0 = 不要小图标槽）。
        // 返回 HRESULT：S_FALSE = 该位置没有图标，一并按 None 处理。
        SHDefExtractIconW(
            PCWSTR(wide.as_ptr()),
            index,
            0,
            Some(&mut hicon),
            None,
            ICON_SIZE as u32,
        )
        .ok()
        .ok()?;
        if hicon.is_invalid() {
            return None;
        }
        let png = hicon_to_png(hicon);
        let _ = DestroyIcon(hicon);
        let bytes = png?;
        Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    }
}

/// HICON → PNG 字节：GetIconInfo 拿彩色/掩码位图，GetDIBits 抓 32bpp 像素。
/// 老式图标 alpha 通道全 0 时用掩码位图恢复透明区。
unsafe fn hicon_to_png(hicon: HICON) -> Option<Vec<u8>> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(hicon, &mut info) }.ok()?;
    // GetIconInfo 文档要求：两张位图用完必须删，否则 GDI 句柄泄漏。
    let result = unsafe { bitmaps_to_png(info.hbmColor, info.hbmMask) };
    unsafe {
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
    }
    result
}

unsafe fn bitmaps_to_png(color: HBITMAP, mask: HBITMAP) -> Option<Vec<u8>> {
    let mut bm = BITMAP::default();
    if unsafe {
        GetObjectW(
            color.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut _),
        )
    } == 0
    {
        return None;
    }
    let (width, height) = (bm.bmWidth, bm.bmHeight);
    if width <= 0 || height <= 0 || width > 256 || height > 256 {
        return None;
    }
    let hdc: HDC = unsafe { GetDC(None) };
    let pixels = unsafe { grab_bgra(hdc, color, width, height) };
    // alpha 全 0 时才需要掩码（白 = 透明）；现代图标带真 alpha，掩码不抓。
    let mask_pixels = match &pixels {
        Some(px) if px.chunks_exact(4).all(|p| p[3] == 0) => {
            unsafe { grab_bgra(hdc, mask, width, height) }
        }
        _ => None,
    };
    unsafe { ReleaseDC(None, hdc) };
    let mut pixels = pixels?;
    for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
        px.swap(0, 2); // BGRA → RGBA
        if let Some(mask_px) = &mask_pixels {
            px[3] = if mask_px[i * 4] != 0 { 0 } else { 255 };
        }
    }
    let image = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// 以 32bpp 顶朝下抓一张位图的像素（BGRA 顺序）。
unsafe fn grab_bgra(hdc: HDC, bitmap: HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let got = unsafe {
        GetDIBits(
            hdc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        )
    };
    (got != 0).then_some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取证用（与 detect.rs 的 explain 同哲学：改行为前先看真实系统数据）：
    /// 打印系统清单里各处理器的标识（GetName）与展示名，不断言。
    /// 手动跑：cargo test --lib dump_txt_handlers -- --ignored --nocapture
    #[test]
    #[ignore = "取证用，输出依赖本机注册表"]
    fn dump_txt_handlers() {
        for ext in [".txt", ".ts"] {
            let _com = ComGuard::new();
            println!("=== {ext} ===");
            let wide: Vec<u16> = ext.encode_utf16().chain([0]).collect();
            unsafe {
                let Ok(enumerator) =
                    SHAssocEnumHandlers(PCWSTR(wide.as_ptr()), ASSOC_FILTER_RECOMMENDED)
                else {
                    continue;
                };
                loop {
                    let mut handlers: [Option<IAssocHandler>; 8] = Default::default();
                    let mut fetched = 0u32;
                    if enumerator.Next(&mut handlers, Some(&mut fetched)).is_err() || fetched == 0 {
                        break;
                    }
                    for handler in handlers.into_iter().take(fetched as usize).flatten() {
                        let name = handler.GetName().ok().and_then(|p| take_co_string(p));
                        let ui = handler.GetUIName().ok().and_then(|p| take_co_string(p));
                        println!("name={name:?} ui={ui:?}");
                    }
                }
            }
        }
    }
}
