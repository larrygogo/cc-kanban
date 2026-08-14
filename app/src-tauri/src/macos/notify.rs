use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

use mac_notification_sys::{
    get_bundle_identifier_or_default, send_notification, set_application, NotificationResponse,
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

static TX: OnceLock<Sender<NotifyJob>> = OnceLock::new();
/// 点击跳转需要的两件套：打开对话窗口的句柄 + 判会话归属的 broker。init 时一并种下。
static CLICK_CONTEXT: OnceLock<(AppHandle, crate::pty::PtyBroker)> = OnceLock::new();

/// 启动一次：设应用归属 + 起串行通知线程。5s 轮询线程只投递、绝不阻塞（避免在轮询里同步等回调致 CPU 飙升）。
pub fn init(app: &AppHandle) {
    let bundle = get_bundle_identifier_or_default("Meowo");
    let _ = set_application(&bundle);
    let _ = CLICK_CONTEXT.set((app.clone(), app.state::<crate::AppState>().ptys.clone()));

    let (tx, rx) = mpsc::channel::<NotifyJob>();
    std::thread::spawn(move || {
        // 串行：每条通知同步等待用户交互，阻塞仅限本线程。
        for job in rx {
            if let Ok(NotificationResponse::Click) =
                send_notification(&job.title, None, &job.body, None)
            {
                // 点击后通知不会自动从"通知中心"消失，主动移除本应用的已投递通知。
                // 与 send_notification 同线程调用（该线程已在跑通知中心的 runloop）。
                clear_delivered();
                // 托管会话：PTY 归 Meowo，没有可聚焦的外部终端——打开对话窗口落在该会话上。
                // 对话功能关闭（轻量模式）时对话窗不是合法落点，改把同一个 PTY 镜像进
                // 外部终端（reveal_session 在关闭态强制走 attach 路径）。
                if let Some((app, ptys)) = CLICK_CONTEXT.get() {
                    if ptys.is_managed(job.session_id) {
                        if crate::settings::load_settings().chat_enabled {
                            crate::window::open_chat_window_detached(app.clone(), job.session_id);
                        } else if let Err(error) =
                            crate::terminal::reveal_session(app, ptys, job.session_id)
                        {
                            eprintln!("通知点击打开外部终端失败: {error}");
                        }
                        continue;
                    }
                }
                // 外部终端会话：按 pid->tty 切到该会话所在终端。resume_argv 传空 = 不允许
                // resume 回退，resume_kind 仅占位（仍按设置取，保持一致）。
                // resume_argv 为空 → 不会走 resume 回退，env 前缀无用武之地，传空串。
                crate::macos::terminal::focus_session_terminal(
                    job.pid,
                    None,
                    &[],
                    crate::resume_terminal_kind(),
                    "",
                );
            }
        }
    });
    let _ = TX.set(tx);
}

/// 移除本应用在"通知中心"里所有已投递的通知。mac-notification-sys 不给单条句柄/标识，
/// 只能整体清空——对"会话等待/出错"这类瞬时提醒正合适。走已废弃的 NSUserNotificationCenter
/// （与 mac-notification-sys 内部用的是同一套），故用 removeAllDeliveredNotifications。
fn clear_delivered() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: 标准 objc 消息发送；defaultUserNotificationCenter 返回进程级单例（不归我们持有，
    // 故取裸指针而非 Retained；可能为 nil，已判空），removeAllDeliveredNotifications 无参无返回。
    unsafe {
        let center: *mut AnyObject = msg_send![
            class!(NSUserNotificationCenter),
            defaultUserNotificationCenter
        ];
        if !center.is_null() {
            let _: () = msg_send![center, removeAllDeliveredNotifications];
        }
    }
}

/// 投递一条通知任务（非阻塞）。OnceLock 未初始化时静默丢弃。
pub fn post(job: NotifyJob) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(job);
    }
}
