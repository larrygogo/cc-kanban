//! 确认对话框的**原生小窗**宿主:应用样式(webview 渲染,吃主题/字体)+ 原生窗口能力
//! (独立窗口,可拖拽、可拖出主窗边界)。两头要求只有「无边框小窗 + 应用 CSS」同时满足:
//! 系统 MessageBox 样式脱节(用户嫌丑),webview 内嵌模态又出不了窗口边界。
//!
//! 协议:请求方 invoke [`confirm_dialog`](等待结果)→ 建 `confirm-<id>` 小窗(前端按
//! label 前缀路由到 ConfirmWindow 视图)→ 小窗取 payload 渲染 → 用户点按钮 invoke
//! [`confirm_dialog_result`] → 结果经通道送回请求方。小窗被直接关闭(Alt-F4)按取消
//! 收场——确认宁可失败,绝不静默当同意。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Clone, serde::Serialize)]
pub(crate) struct ConfirmPayload {
    title: String,
    message: String,
    danger: bool,
}

type ResultSender = tauri::async_runtime::Sender<bool>;

#[derive(Default)]
pub(crate) struct Confirms {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, (ConfirmPayload, Option<ResultSender>)>>,
    /// 各父窗身上挂着的确认框计数。父窗的 set_enabled 必须引用计数:同一父窗可能并发
    /// 挂多个确认框(用户点不了被禁用的父窗,但 JS 侧事件/定时器仍能再拉起一个),首个
    /// resolve 就恢复父窗会让其余「模态」形同虚设——父窗可点,关掉它还会连带销毁剩下
    /// 的确认框、静默按取消收场。进场 +1、离场 -1,归零才恢复。
    /// 仅非 macOS 使用:mac 不禁用父窗(set_enabled 的 sheet 实现见 confirm_dialog 内注释),
    /// 字段与方法保留全平台编译,引用计数单测不分平台。
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    disabled_parents: Mutex<HashMap<String, usize>>,
}

impl Confirms {
    fn take_sender(&self, id: u64) -> Option<ResultSender> {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(&id)
            .and_then(|(_, sender)| sender.take())
    }

    fn remove(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
    }

    /// 计数 +1;返回是否由本次从 0 变 1(该真正执行禁用)。
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn parent_disabled(&self, label: &str) -> bool {
        let mut parents = self
            .disabled_parents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let count = parents.entry(label.to_string()).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// 计数 -1;返回是否归零(该真正恢复启用)。计数缺失按归零处理——宁可多恢复一次,
    /// 也不能让父窗停留在禁用态。
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn parent_released(&self, label: &str) -> bool {
        let mut parents = self
            .disabled_parents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match parents.get_mut(label) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            _ => {
                parents.remove(label);
                true
            }
        }
    }
}

/// 固定逻辑尺寸:高度给两行消息留量,更长的消息在卡片内滚动。
const WIDTH: f64 = 420.0;
const HEIGHT: f64 = 208.0;

#[tauri::command]
pub(crate) async fn confirm_dialog(
    app: AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, super::AppState>,
    title: String,
    message: String,
    danger: Option<bool>,
) -> Result<bool, String> {
    let confirms = state.confirms.clone();
    let id = confirms.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = tauri::async_runtime::channel::<bool>(1);
    // 中毒恢复（与本文件其余取锁处及全仓策略一致）：确认框是删除/结束会话等破坏性操作的
    // 最后一道闸，一次别处 panic 不该让它从此永久失效。
    confirms
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            id,
            (
                ConfirmPayload {
                    title,
                    message,
                    danger: danger.unwrap_or(false),
                },
                Some(tx),
            ),
        );
    let label = format!("confirm-{id}");
    let builder = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        // 首帧渲染完由前端自行显示(ConfirmWindow 量完内容、调完窗口高度再 show),
        // 消除白框闪烁(与其余窗口同款)。
        .visible(false);
    // 模态的所属关系:请求窗口设为 owner(Windows)/父窗(macOS)。任务栏与最小化行为
    // 跟随父窗,且点击被禁用的父窗时系统会把焦点弹回本窗(配合下方 set_enabled)。
    // 拿不到对应 WebviewWindow(理论上不发生)就退化为独立置顶窗,不拦流程。
    #[cfg(any(windows, target_os = "macos"))]
    let builder = match app.get_webview_window(window.label()) {
        Some(parent) => builder.parent(&parent).map_err(|error| {
            confirms.remove(id);
            error.to_string()
        })?,
        None => builder,
    };
    // macOS:无边框窗口不自动圆角,设透明由前端 .app-confirm.is-window 的 border-radius
    // 呈现(同设置/更新窗口)。
    #[cfg(target_os = "macos")]
    let builder = builder.transparent(true);
    // 非 macOS:原生底色对齐主题,show 瞬间合成帧未上屏也不露白(见 window_background_color)。
    #[cfg(not(target_os = "macos"))]
    let builder = builder.background_color(crate::window::window_background_color(&app));
    let confirm_window = builder.build().map_err(|error| {
        confirms.remove(id);
        error.to_string()
    })?;
    // 居中于请求窗口:建成后用**物理坐标** set_position(与 snap.rs 等处同款)。曾在
    // builder 上给逻辑坐标,而建窗期的逻辑→物理换算按**主屏** DPI——父窗在缩放不同的
    // 副屏上时落点整体偏移(实拍:双屏下确认框弹到莫名其妙的位置,用户根本没察觉)。
    // 物理坐标是全桌面唯一坐标系,不经任何换算;此刻窗口还没 show,挪动不可见。
    // 任一项拿不到就留在系统默认落点。
    if let (Ok(pos), Ok(size), Ok(scale)) = (
        window.outer_position(),
        window.outer_size(),
        window.scale_factor(),
    ) {
        let x = pos.x + ((size.width as f64 - WIDTH * scale) / 2.0).round() as i32;
        let y = pos.y + ((size.height as f64 - HEIGHT * scale) / 2.0).round() as i32;
        let _ = confirm_window.set_position(tauri::PhysicalPosition::new(x, y));
    }
    // Win11:无边框不可缩放窗口 DWM 不自动圆角,显式声明(见 round_window_corners)。
    crate::window::round_window_corners(&confirm_window);
    // 兜底显示:前端没起来(加载失败/崩溃)时到点强制 show。此前缺这个兜底只是悬窗;
    // 下面启用模态后,父窗已被禁用,confirm 再永久隐身等于整个应用被劫持——
    // 必须保证它可见可关(Alt-F4 = 取消,Destroyed 分支收尾)。
    crate::window::show_after_grace(&confirm_window, true);
    // 被直接关闭(Alt-F4/系统关闭)= 取消:sender 还在说明没人给过答案。
    {
        let confirms = confirms.clone();
        confirm_window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(sender) = confirms.take_sender(id) {
                    let _ = sender.try_send(false);
                }
            }
        });
    }
    // 模态:等待期间禁用请求窗口——此前只是置顶,父窗照常可点,一边挂着「结束会话?」
    // 一边还能继续操作。禁用失败(窗口已关/平台不支持)静默退化为非模态,不拦确认本身。
    // 从这里到恢复之间不得有 `?` 提前返回,否则父窗被永久禁用。
    // 经引用计数(见 disabled_parents):并发确认框首个 resolve 不得提前恢复父窗。
    //
    // macOS 除外:runtime 的 set_enabled(false) 实现是往父窗 beginSheet 一张与父窗
    // 同 frame 的半透明灰窗(electron 同款)——sheet 从标题栏下方挂出、底部溢出父窗
    // 一大截,视觉上凭空多出一块巨大灰板(实拍反馈)。mac 退化为置顶非模态,
    // 与 parent() 的子窗关系配合已足够把确认框钉在父窗前。
    #[cfg(not(target_os = "macos"))]
    if confirms.parent_disabled(window.label()) {
        let _ = window.set_enabled(false);
    }
    let ok = rx.recv().await.unwrap_or(false);
    confirms.remove(id);
    // 先恢复父窗、再关模态窗(Win32 模态收尾定式):顺序反了的话,销毁瞬间系统找不到
    // 可激活的所属窗口,焦点会飞到别的应用。收尾后把焦点交还父窗。
    #[cfg(not(target_os = "macos"))]
    if confirms.parent_released(window.label()) {
        let _ = window.set_enabled(true);
    }
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
    }
    let _ = window.set_focus();
    Ok(ok)
}

/// 该调用是否来自 `confirm-<id>` 小窗自己。payload/result 两条命令都必须做这道校验:
/// 确认框是删除会话/结束进程等破坏性操作的最后一道闸,而 Tauri 命令默认任何 webview
/// 都能调——不校验来源的话,被攻破的主窗渲染层可以直接替用户「点同意」,模态形同虚设。
/// 绑定窗口 label 后,只有后端亲手建出的那个小窗能给自己的确认作答。
/// (这只挡「自答确认框」;被攻破的渲染层绕过确认、直调破坏性命令,须由后端授权层解决,
/// 见 lib.rs 各破坏性命令——本文件管不到。)
fn is_confirm_window(window: &tauri::Window, id: u64) -> bool {
    window.label() == format!("confirm-{id}")
}

/// 小窗启动后取渲染内容。走命令而不是 URL 参数:标题/正文是任意用户语言文本,
/// 省掉转义,也不把内容留在窗口 URL 里。仅限对应小窗本人调用(见 is_confirm_window)。
#[tauri::command]
pub(crate) fn confirm_dialog_payload(
    window: tauri::Window,
    state: tauri::State<'_, super::AppState>,
    id: u64,
) -> Result<ConfirmPayload, String> {
    if !is_confirm_window(&window, id) {
        return Err("确认请求不存在".into());
    }
    state
        .confirms
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|(payload, _)| payload.clone())
        .ok_or_else(|| "确认请求不存在".into())
}

/// 小窗按钮的裁决。纯内存转发,同步命令(见 CLAUDE.md 线程纪律)。
/// 仅限对应小窗本人调用:其它窗口伪造的答复一律丢弃(见 is_confirm_window)。
#[tauri::command]
pub(crate) fn confirm_dialog_result(
    window: tauri::Window,
    state: tauri::State<'_, super::AppState>,
    id: u64,
    ok: bool,
) {
    if !is_confirm_window(&window, id) {
        return;
    }
    if let Some(sender) = state.confirms.take_sender(id) {
        let _ = sender.try_send(ok);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同一父窗并发挂两个确认框:首个 resolve 不得恢复父窗(第二个「模态」会形同虚设),
    /// 最后一个离场才恢复;计数缺失按归零处理,不同父窗互不影响。
    #[test]
    fn parent_enable_is_refcounted() {
        let confirms = Confirms::default();
        assert!(confirms.parent_disabled("chat")); // 0→1:执行禁用
        assert!(!confirms.parent_disabled("chat")); // 1→2:已禁用,不重复
        assert!(!confirms.parent_released("chat")); // 2→1:还有确认框挂着,不恢复
        assert!(confirms.parent_released("chat")); // 1→0:恢复
        assert!(confirms.parent_released("chat")); // 计数缺失:宁可多恢复一次
                                                   // 不同父窗互不影响。
        assert!(confirms.parent_disabled("main"));
        assert!(confirms.parent_released("chat"));
        assert!(confirms.parent_released("main"));
    }
}
