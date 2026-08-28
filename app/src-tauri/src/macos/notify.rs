use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use tauri::{AppHandle, Manager};

/// 一条待弹通知；点击后按会话归属分流：GUI 托管会话 → 打开对话窗口并落在该会话
/// （PTY 归 Meowo，没有可聚焦的外部终端）；外部终端会话 → 按 pid->tty 切到对应终端
/// （通知场景无需 resume，故不带 cwd/id）。
pub struct NotifyJob {
    pub title: String,
    pub body: String,
    pub session_id: i64,
    pub pid: i64,
}

/// 点击跳转需要的两件套：打开对话窗口的句柄 + 判会话归属的 broker。init 时一并种下。
static CLICK_CONTEXT: OnceLock<(AppHandle, crate::pty::PtyBroker)> = OnceLock::new();
/// 通知标识 -> 点击路由参数（session_id, pid）。delegate 回调只拿得到 request.identifier，
/// 路由所需参数存在这里；点击或上限清空时移除，防泄漏。
static JOBS: LazyLock<Mutex<HashMap<String, (i64, i64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// delegate 必须常驻：UNUserNotificationCenter.delegate 是 weak 引用，不持有即悬空。
static DELEGATE: OnceLock<Retained<NotificationDelegate>> = OnceLock::new();
/// 通知标识序号：同一会话可能连发多条，identifier 必须唯一才能逐条路由点击。
static SEQ: AtomicU64 = AtomicU64::new(0);
/// JOBS 上限：通知中心滞留条数有限，64 足够覆盖；满即整体清空重来。
const MAX_TRACKED_JOBS: usize = 64;

define_class!(
    // SAFETY:
    // - 超类 NSObject 无子类化要求。
    // - ivars 为单元类型；本类不实现 Drop。
    #[unsafe(super(NSObject))]
    #[name = "MeowoNotificationDelegate"]
    struct NotificationDelegate;

    // SAFETY: NSObjectProtocol 无额外安全要求。
    unsafe impl NSObjectProtocol for NotificationDelegate {}

    // SAFETY: UNUserNotificationCenterDelegate 的两个方法均为 optional，签名与协议声明一致。
    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        // 应用处于前台时也照常弹横幅+声音——UNUserNotificationCenter 默认吞掉前台期间的
        // 通知，而旧实现（NSUserNotificationCenter）总是显示；不实现此方法就是行为回退。
        // SAFETY: 签名与协议声明一致。
        // 方法名按 objc2 协议方法惯例非 snake_case，CI 的 -D warnings 会把它升级成错误。
        #[allow(non_snake_case)]
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn userNotificationCenter_willPresentNotification_withCompletionHandler(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((
                UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::Sound,
            ));
        }

        // SAFETY: 签名与协议声明一致。
        #[allow(non_snake_case)]
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn userNotificationCenter_didReceiveNotificationResponse_withCompletionHandler(
            &self,
            center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &DynBlock<dyn Fn()>,
        ) {
            handle_response(center, response);
            // 协议约定必须调用 completion handler，否则系统按未响应处理。
            completion_handler.call(());
        }
    }
);

impl NotificationDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: 继承自 NSObject 的 init 签名正确；单元 ivars 已在上方初始化。
        unsafe { msg_send![super(this), init] }
    }
}

/// delegate 回调（主线程触发）：只响应点击正文（DefaultAction）；路由动作（开对话窗口 /
/// AppleScript 聚焦终端）可能耗时，放工作线程执行，completion handler 已由调用方立即触发。
fn handle_response(center: &UNUserNotificationCenter, response: &UNNotificationResponse) {
    let identifier = response.notification().request().identifier().to_string();
    let job = JOBS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&identifier);
    // extern static 不受 Rust 类型系统约束，读取要 unsafe；它是 Apple 框架的常量字符串
    // 指针，进程存续期有效，只读比较是安全的。
    let is_default_action = unsafe {
        response
            .actionIdentifier()
            .isEqualToString(UNNotificationDefaultActionIdentifier)
    };
    if !is_default_action {
        return;
    }
    // 点击后通知不会自动从"通知中心"消失，主动移除本应用的已投递通知（与旧实现一致：
    // UNUserNotificationCenter 同样不给单条移除之外的语义，removeAllDeliveredNotifications
    // 整体清空对"会话等待/出错"这类瞬时提醒正合适）。
    center.removeAllDeliveredNotifications();
    if let Some((session_id, pid)) = job {
        std::thread::spawn(move || route_click(session_id, pid));
    }
}

/// 点击路由（工作线程）：与旧串行线程版逐条对应。
fn route_click(session_id: i64, pid: i64) {
    // 托管会话：PTY 归 Meowo，没有可聚焦的外部终端——打开对话窗口落在该会话上。
    // 对话功能关闭（轻量模式）时对话窗不是合法落点，改把同一个 PTY 镜像进
    // 外部终端（reveal_session 在关闭态强制走 attach 路径）。
    if let Some((app, ptys)) = CLICK_CONTEXT.get() {
        if ptys.is_managed(session_id) {
            if crate::settings::load_settings().chat_enabled {
                crate::window::open_chat_window_detached(app.clone(), session_id);
            } else if let Err(error) = crate::terminal::reveal_session(app, ptys, session_id) {
                eprintln!("通知点击打开外部终端失败: {error}");
            }
            return;
        }
    }
    // 外部终端会话：按 pid->tty 切到该会话所在终端。resume_argv 传空 = 不允许
    // resume 回退，resume_kind 仅占位（仍按设置取，保持一致）。
    crate::macos::terminal::focus_session_terminal(pid, None, &[], crate::resume_terminal_kind());
}

/// 启动一次：挂 delegate + 请求通知授权。UNUserNotificationCenter 全程异步投递，
/// 不再需要旧实现的串行工作线程（那条线程为等点击回调会阻塞后续所有通知的投递）。
pub fn init(app: &AppHandle) {
    let _ = CLICK_CONTEXT.set((app.clone(), app.state::<crate::AppState>().ptys.clone()));

    let center = UNUserNotificationCenter::currentNotificationCenter();
    let delegate = NotificationDelegate::new();
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let _ = DELEGATE.set(delegate);

    // UNUserNotificationCenter 必须显式请求授权（旧 NSUserNotificationCenter 不需要）；
    // 拒绝/失败只记日志——通知降级为静默，不影响其余流程。
    let auth_handler: RcBlock<dyn Fn(Bool, *mut NSError)> =
        RcBlock::new(|granted: Bool, error: *mut NSError| {
            if !error.is_null() {
                // SAFETY: completion handler 契约保证 error 非空时指向有效 NSError。
                let desc = unsafe { (*error).localizedDescription() };
                eprintln!("通知授权请求失败: {desc}");
            } else if !granted.as_bool() {
                eprintln!("通知授权被拒绝：会话提醒通知不会显示（可在系统设置中开启）");
            }
        });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &auth_handler,
    );
}

/// 投递一条通知（非阻塞）。调度到主线程执行：UNUserNotificationCenter 文档允许任意线程
/// 调用，但本项目的 AppKit/系统框架交互都集中在主线程，保持一致。init 未跑时静默丢弃。
pub fn post(job: NotifyJob) {
    let Some((app, _)) = CLICK_CONTEXT.get() else {
        return;
    };
    let app = app.clone();
    let _ = app.run_on_main_thread(move || deliver(job));
}

fn deliver(job: NotifyJob) {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let identifier = format!(
        "meowo-notify-{}-{}",
        job.session_id,
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    {
        let mut jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.len() >= MAX_TRACKED_JOBS {
            jobs.clear();
        }
        jobs.insert(identifier.clone(), (job.session_id, job.pid));
    }

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&job.title));
    content.setBody(&NSString::from_str(&job.body));
    let sound = UNNotificationSound::defaultSound();
    content.setSound(Some(&sound));
    // trigger=None：立即投递。
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&identifier),
        &content,
        None,
    );
    let add_handler: RcBlock<dyn Fn(*mut NSError)> = RcBlock::new(|error: *mut NSError| {
        if !error.is_null() {
            // SAFETY: completion handler 契约保证 error 非空时指向有效 NSError。
            let desc = unsafe { (*error).localizedDescription() };
            eprintln!("通知投递失败: {desc}");
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&add_handler));
}
