//! 远程访问桥：局域网/Tailscale 内的手机浏览器经 HTTP 使用对话页。
//!
//! # 威胁模型与安全基线（审计从这里开始）
//!
//! 与 pty.rs 的 attach broker 不同，本服务**监听所有网卡**——网络层安全交给
//! Tailscale/局域网（文档明示：公网勿开），应用层只保两件事：
//!
//! 1. **token 门**：`/rpc` 走 `X-Meowo-Token` 头、`/file` 走 `?token=` 查询串
//!    （`<img src>` 发不了自定义头），常量时间比较。token 由 [`generate_token`]
//!    严格生成（OS RNG 失败即拒绝，不做 pty.rs `random_token` 那种弱回退——那个
//!    回退的安全论证依赖「只监听 loopback」，在这里不成立）。静态资源不鉴权
//!    （构建产物无秘密）。
//! 2. **命令白名单（default-deny）**：`/rpc` 只放行 [`BRIDGED_COMMANDS`]。以下
//!    类别**明确拒绝**，新增放行项时逐条对照：
//!    - 密钥类：`get_relay_secrets` / `set_relay_secret`（明文 API key）
//!    - 设置写：`set_settings`（可反手打开更大的暴露面）
//!    - 宿主执行类：`open_url` / `open_link` / `open_path_with` / `open_project_dir`
//!      / `reveal_path_in_file_manager` / `open_attached_terminal`（在宿主机开程序）
//!    - 窗口几何/桌面语义：`snap_*` / `open_chat_window` / `confirm_dialog` 等
//!    - 登录/安装/更新类：`login_agent` / `install_agent` / `check_update` 等
//!    - 文件浏览类：`read_file_text` / `list_dir_entries` / `search_project_files`
//!      / `git_*`（任意本地文件读，v1 砍掉）
//!    - `stop_managed_terminal`（远端不给「杀会话」）
//!
//! 注意：白名单内的 `write_managed_terminal` 本身就等于「在宿主机上执行任意命令」
//! （往 agent CLI 的 PTY 写任意按键）——这是产品功能（远程发消息/打断/答题），
//! 不是疏漏；它成立的前提正是上面的 token 门 + 网络层假设。
//!
//! # 与 Tauri command 的耦合约定
//!
//! 桥接臂直接调用各 `#[tauri::command]` 函数本体（`app.state::<AppState>()` 取
//! State，先例见 lib.rs `install_downloaded_update` / watch.rs）。参数结构体
//! `rename_all = "camelCase"` + `deny_unknown_fields`，与前端 invoke 的调用契约
//! 逐名对齐；返回值原样透传 serde 结果（DTO 的 case 本就不统一，桥不做二次加工）。
//! 若某命令日后新增 `Window` 参数，这里会**编译期变红**——显式耦合，属预期。

use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tauri::Manager;

/// token 最短长度（十六进制字符数，= 16 字节熵）。手改 settings.json 塞短 token 属
/// 自毁行为，这里拦下：门都不开，好过开一扇纸门。generate_token 产出 64 位。
const MIN_TOKEN_LEN: usize = 32;

/// `/rpc` 允许的 body 上限：save_pasted_attachment 的 base64 上限 ~43MB（chat.rs
/// PASTE_MAX_BYTES 32MB × 4/3），取 48MB 留 JSON 包装余量。
const MAX_BODY_BYTES: usize = 48 * 1024 * 1024;

/// `/rpc` 放行名单（default-deny 的唯一事实源）。dispatch 的 match 漏了这里的项会
/// 落 `_` 臂回 404——fail-closed；反向（match 有、名单没有）根本进不了 match。
/// 名单按对话页组件树的实际调用集合收敛（ChatWindow/ChatSidebar/NewSessionPanel
/// /chat/*），不是「所有无害命令」——用不到就不开。
pub(crate) const BRIDGED_COMMANDS: &[&str] = &[
    // 会话列表/侧栏
    "get_live_sessions_page",
    "get_live_sessions_counts",
    "get_session_lineage",
    "search_chat_transcripts",
    "recent_cwds",
    "rename_session",
    "set_archived",
    "set_session_note",
    // 对话流
    "get_chat_history",
    "get_subagent_transcript",
    "refresh_session_model",
    "refresh_session_todos",
    // 托管 PTY（发送/打断/答题都经 write_managed_terminal 的按键序列）
    "managed_terminal_snapshot",
    "managed_terminal_binding",
    "write_managed_terminal",
    "resize_managed_terminal",
    "start_managed_terminal",
    "takeover_managed_terminal",
    "attach_background_session",
    "send_background_prompt",
    "session_launch_selections",
    "set_session_launch_selection",
    // 审批/交互提问
    "pending_interaction",
    "register_approval_consumer",
    "unregister_approval_consumer",
    "resolve_pending_approval",
    "dismiss_interactive_question",
    // 附件（落盘到 %TEMP%/meowo-paste，路径由后端生成）
    "save_pasted_attachment",
    // 新建会话
    "list_agents",
    "agent_chat_ui",
    "agent_models",
    "new_session",
    "check_provider_hooks",
    // 只读杂项
    "get_settings",
    "host_os",
];

/// server 运行态。独立 `app.manage`（不进 AppState：那是命令数据面的托管，这里是
/// 网络面生命周期，改动面越小越好）。
#[derive(Default)]
pub(crate) struct RemoteRuntime {
    /// apply 串行锁：设置连点/启动竞态下两次 apply 交错会双起 server 或漏关旧的。
    /// tokio Mutex——临界区里要 await（bind/优雅关停）。
    apply_lock: tokio::sync::Mutex<()>,
    inner: Mutex<RuntimeInner>,
}

#[derive(Default)]
struct RuntimeInner {
    running: Option<Running>,
    /// 最近一次启动失败原因（端口被占/token 缺失…），设置页（PR4）据此显示。
    last_error: Option<String>,
}

struct Running {
    port: u16,
    token: String,
    /// notify_one 存 permit：serve 任务尚未开始等也不会丢关停信号。
    shutdown: Arc<tokio::sync::Notify>,
}

/// 严格 token 生成：OS RNG 不可用直接失败（调用方应拒绝启用远程访问）。
/// 刻意不复用 pty.rs::random_token——它的弱回退（pid+时间）只在 loopback 语境下可接受。
/// 生产调用方是设置页的 [`remote_access_info`]（随二维码配对一并交付）。
pub(crate) fn generate_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("系统随机数不可用：{e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// 惰性确保 settings 里有可用 token：已有（≥MIN_TOKEN_LEN）直接返回当前设置，否则
/// 生成一枚落盘。刻意先只读判断、缺失才走 update_settings，避免设置页每次打开都写盘。
/// 返回落盘后的完整 Settings（供 remote_access_info 一并读 enabled/port）。
fn ensure_remote_token() -> Result<crate::settings::Settings, String> {
    let existing = crate::settings::load_settings();
    if existing.remote_access_token.len() >= MIN_TOKEN_LEN {
        return Ok(existing);
    }
    crate::settings::update_settings(|s| {
        if s.remote_access_token.len() < MIN_TOKEN_LEN {
            s.remote_access_token = generate_token()?;
        }
        Ok(s.clone())
    })
}

/// 候选地址归类：桌面端无从知道手机在哪个网，只能把每个候选「是什么、什么情况下
/// 能通」标清楚交给用户选。只认得清的两类——Tailscale CGNAT（100.64.0.0/10，手机装
/// Tailscale 即任何网络可达）与 RFC1918 局域网段（须同一 Wi-Fi）。其余一律丢弃：
/// TUN 代理的假地址（198.18/15 等）、link-local、回环、公网——给一个「可能通」的
/// 地址让用户瞎试，不如没有。
fn classify_ip(ip: std::net::IpAddr) -> Option<&'static str> {
    let std::net::IpAddr::V4(v4) = ip else {
        return None;
    };
    let o = v4.octets();
    if o[0] == 100 && (64..128).contains(&o[1]) {
        return Some("tailscale");
    }
    if v4.is_private() {
        return Some("lan");
    }
    None
}

/// 可达地址枚举（零依赖）：UDP `connect` 只让内核按路由表选出口网卡、不实际发包，
/// 再读 `local_addr` 拿本机在该网卡上的地址。`100.100.100.100`（Tailscale 的
/// CGNAT 探针）取 tailnet 出口，`8.8.8.8` 取默认网卡出口。Tailscale 探测在前：
/// 有 tailnet 时它是唯一不挑手机所在网络的地址，列表首位即前端默认选中项。
/// 归类失败（如 TUN 假地址）逐个丢弃，全空则设置页回退提示手输 IP。
fn local_ips() -> Vec<IpCandidate> {
    let mut out: Vec<IpCandidate> = Vec::new();
    for probe in ["100.100.100.100:80", "8.8.8.8:80"] {
        let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if sock.connect(probe).is_err() {
            continue;
        }
        let Ok(addr) = sock.local_addr() else {
            continue;
        };
        // 没装 Tailscale 时 CGNAT 探针照样按默认路由回局域网地址——归类兜住，去重兜住。
        let Some(kind) = classify_ip(addr.ip()) else {
            continue;
        };
        let ip = addr.ip().to_string();
        if !out.iter().any(|c| c.ip == ip) {
            out.push(IpCandidate { ip, kind });
        }
    }
    out
}

/// 单个可达地址候选。`kind`（"tailscale" / "lan"）供前端标注可达条件——地址本身
/// 说明不了「手机在什么情况下连得上」，标签才说明得了。
#[derive(serde::Serialize)]
pub(crate) struct IpCandidate {
    ip: String,
    kind: &'static str,
}

/// 设置页配对信息（**桌面专用命令，不进 /rpc 白名单**——token 只在宿主机取得）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteAccessInfo {
    enabled: bool,
    port: u32,
    /// 惰性生成并持久化的 token；二维码 URL = `http://<ip>:<port>/#token=<token>`。
    token: String,
    /// 归类后的可达地址候选，Tailscale 优先（可能为空，前端回退手输）。
    ips: Vec<IpCandidate>,
    /// 最近一次启动失败原因（端口被占等），供设置页红字提示。
    last_error: Option<String>,
}

/// 设置页调用：惰性生成 token 落盘，返回开关/端口/可达 IP/最近启动错误。
/// 只登记进 `generate_handler`（桌面 invoke），绝不进 `BRIDGED_COMMANDS`——远端不该
/// 能读到 token 本身（它就是进门的凭据）。
#[tauri::command]
pub(crate) async fn remote_access_info(
    app: tauri::AppHandle,
) -> Result<RemoteAccessInfo, String> {
    // token 惰性生成 + settings 读取 + IP 探测都可能触碰文件/网卡,挪进 blocking 池。
    let (settings, ips) = tauri::async_runtime::spawn_blocking(|| -> Result<_, String> {
        let settings = ensure_remote_token()?;
        let ips = local_ips();
        Ok((settings, ips))
    })
    .await
    .map_err(|e| e.to_string())??;
    let last_error = {
        let runtime = app.state::<RemoteRuntime>();
        let inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.last_error.clone()
    };
    Ok(RemoteAccessInfo {
        enabled: settings.remote_access_enabled,
        port: settings.remote_access_port,
        token: settings.remote_access_token,
        ips,
        last_error,
    })
}

/// 常量时间比较（逐字节 |= 异或）。长度不同直接 false——token 长度本就是公开信息
/// （generate_token 恒 64 字符），不构成泄露。
fn token_matches(expected: &str, provided: &str) -> bool {
    let (a, b) = (expected.as_bytes(), provided.as_bytes());
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 按当前 settings 对齐 server 状态（起/停/换端口换 token）。fire-and-forget：
/// 调用点在 setup（主线程）与 set_settings，settings 读取是文件 IO，全部挪进后台。
pub(crate) fn apply(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(settings) =
            tauri::async_runtime::spawn_blocking(crate::settings::load_settings).await
        else {
            return;
        };
        apply_with(
            &app,
            settings.remote_access_enabled,
            settings.remote_access_port,
            settings.remote_access_token,
        )
        .await;
    });
}

async fn apply_with(app: &tauri::AppHandle, enabled: bool, port: u32, token: String) {
    let runtime = app.state::<RemoteRuntime>();
    let _serial = runtime.apply_lock.lock().await;

    // 临界区不跨 await：先在锁内比对并摘下旧 server，再在锁外发关停信号。
    let to_stop = {
        let mut inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        let unchanged = matches!(
            &inner.running,
            Some(r) if enabled && u32::from(r.port) == port && r.token == token
        );
        if unchanged {
            inner.last_error = None;
            return;
        }
        inner.running.take()
    };
    if let Some(running) = to_stop {
        running.shutdown.notify_one();
    }

    let set_error = |error: Option<String>| {
        let mut inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.last_error = error;
    };
    if !enabled {
        set_error(None);
        return;
    }
    if token.len() < MIN_TOKEN_LEN {
        set_error(Some("远程访问 token 缺失或过短，未启动".into()));
        return;
    }
    let port16 = match u16::try_from(port) {
        Ok(p) if p != 0 => p,
        _ => {
            set_error(Some(format!("端口无效：{port}")));
            return;
        }
    };

    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port16)).await {
        Ok(l) => l,
        Err(e) => {
            set_error(Some(format!("绑定端口 {port16} 失败：{e}")));
            return;
        }
    };
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let router = build_router(app.clone(), token.clone());
    {
        let mut inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.running = Some(Running {
            port: port16,
            token,
            shutdown: shutdown.clone(),
        });
        inner.last_error = None;
    }
    eprintln!("[remote] 远程访问已启动：0.0.0.0:{port16}");
    tauri::async_runtime::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.notified().await })
            .await;
        if let Err(e) = result {
            eprintln!("[remote] server 异常退出：{e}");
        }
    });
}

#[derive(Clone)]
struct Ctx {
    app: tauri::AppHandle,
    token: Arc<str>,
}

fn build_router(app: tauri::AppHandle, token: String) -> Router {
    Router::new()
        .route("/rpc/{command}", post(rpc_handler))
        .route("/file", get(file_handler))
        .fallback(get(static_handler))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(Ctx {
            app,
            token: token.into(),
        })
}

/// invoke 语义对齐：Ok → 200 + JSON 值；Err(String) → 400 + JSON 字符串（前端
/// transport 据此 reject，错误串走既有 formatBackendError 链路）。
fn reply<T: serde::Serialize>(result: Result<T, String>) -> Response {
    match result {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e)).into_response(),
    }
}

fn reply_ok<T: serde::Serialize>(v: T) -> Response {
    reply(Ok(v))
}

fn err_status(status: StatusCode, message: &str) -> Response {
    (status, Json(message.to_string())).into_response()
}

/// body 反序列化为该命令的参数结构体。空 body 按 `{}`（无参命令前端传 undefined）。
/// 错误返回消息串（响应由调用宏构造，避免 Err 侧背整个 Response——clippy result_large_err）。
fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    let raw: &[u8] = if body.is_empty() { b"{}" } else { body };
    serde_json::from_slice(raw).map_err(|e| format!("参数无效：{e}"))
}

async fn rpc_handler(
    State(ctx): State<Ctx>,
    Path(command): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let provided = headers
        .get("x-meowo-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !token_matches(&ctx.token, provided) {
        return err_status(StatusCode::UNAUTHORIZED, "未授权");
    }
    if !BRIDGED_COMMANDS.contains(&command.as_str()) {
        // 未放行与不存在同响应，不区分——不给探测面。
        return err_status(StatusCode::NOT_FOUND, "未知命令");
    }
    dispatch(&ctx.app, &command, &body).await
}

/// 白名单命令 → 各 command 函数本体。参数结构体逐命令定义（camelCase +
/// deny_unknown_fields），与 api.ts 的 invoke 传参逐名对齐。
async fn dispatch(app: &tauri::AppHandle, command: &str, body: &[u8]) -> Response {
    use serde::Deserialize;
    macro_rules! args {
        ($t:ty) => {
            match parse::<$t>(body) {
                Ok(v) => v,
                Err(message) => return err_status(StatusCode::BAD_REQUEST, &message),
            }
        };
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct SessionArg {
        session_id: i64,
    }

    let state = app.state::<crate::AppState>();
    match command {
        "get_live_sessions_page" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                filter: String,
                #[serde(default)]
                search: Option<String>,
                #[serde(default)]
                cwd: Option<String>,
                #[serde(default)]
                before_last_event_at: Option<i64>,
                #[serde(default)]
                before_id: Option<i64>,
                limit: usize,
                #[serde(default)]
                include_foreign: Option<bool>,
            }
            let a = args!(A);
            reply(
                crate::session_query::get_live_sessions_page(
                    state,
                    a.filter,
                    a.search,
                    a.cwd,
                    a.before_last_event_at,
                    a.before_id,
                    a.limit,
                    a.include_foreign,
                )
                .await,
            )
        }
        "get_live_sessions_counts" => {
            reply(crate::session_query::get_live_sessions_counts(state).await)
        }
        "get_session_lineage" => {
            let a = args!(SessionArg);
            reply(crate::handoff::get_session_lineage(state, a.session_id).await)
        }
        "search_chat_transcripts" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                query: String,
            }
            let a = args!(A);
            reply(crate::chat::search_chat_transcripts(state, a.query).await)
        }
        "recent_cwds" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                limit: usize,
            }
            let a = args!(A);
            reply(crate::session_query::recent_cwds(state, a.limit).await)
        }
        "rename_session" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                #[serde(default)]
                cwd: Option<String>,
                session_id: String,
                title: String,
                #[serde(default)]
                provider: Option<String>,
            }
            let a = args!(A);
            reply(
                crate::session_command::rename_session(
                    app.clone(),
                    state,
                    a.cwd,
                    a.session_id,
                    a.title,
                    a.provider,
                )
                .await,
            )
        }
        "set_archived" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                archived: bool,
            }
            let a = args!(A);
            reply(
                crate::session_command::set_archived(app.clone(), state, a.session_id, a.archived)
                    .await,
            )
        }
        "set_session_note" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: String,
                note: String,
            }
            let a = args!(A);
            reply(
                crate::session_command::set_session_note(app.clone(), state, a.session_id, a.note)
                    .await,
            )
        }
        "get_chat_history" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                offset: u64,
                #[serde(default)]
                full: Option<bool>,
            }
            let a = args!(A);
            reply(crate::chat::get_chat_history(state, a.session_id, a.offset, a.full).await)
        }
        "get_subagent_transcript" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                tool_use_id: String,
            }
            let a = args!(A);
            reply(crate::chat::get_subagent_transcript(state, a.session_id, a.tool_use_id).await)
        }
        "refresh_session_model" => {
            let a = args!(SessionArg);
            reply(crate::chat::refresh_session_model(state, a.session_id).await)
        }
        "refresh_session_todos" => {
            let a = args!(SessionArg);
            reply(crate::chat::refresh_session_todos(state, a.session_id).await)
        }
        "managed_terminal_snapshot" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                #[serde(default)]
                since: Option<u64>,
            }
            let a = args!(A);
            reply(
                crate::managed_terminal::managed_terminal_snapshot(state, a.session_id, a.since)
                    .await,
            )
        }
        "managed_terminal_binding" => {
            let a = args!(SessionArg);
            reply_ok(crate::managed_terminal::managed_terminal_binding(
                state,
                a.session_id,
            ))
        }
        "write_managed_terminal" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                data: String,
            }
            let a = args!(A);
            reply(
                crate::managed_terminal::write_managed_terminal(state, a.session_id, a.data).await,
            )
        }
        "resize_managed_terminal" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                cols: u16,
                rows: u16,
            }
            let a = args!(A);
            reply(
                crate::managed_terminal::resize_managed_terminal(
                    state,
                    a.session_id,
                    a.cols,
                    a.rows,
                )
                .await,
            )
        }
        "start_managed_terminal" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                cols: u16,
                rows: u16,
                #[serde(default)]
                options: Option<std::collections::HashMap<String, String>>,
            }
            let a = args!(A);
            reply(
                crate::managed_terminal::start_managed_terminal(
                    app.clone(),
                    state,
                    a.session_id,
                    a.cols,
                    a.rows,
                    a.options,
                )
                .await,
            )
        }
        "takeover_managed_terminal" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                cols: u16,
                rows: u16,
                #[serde(default)]
                options: Option<std::collections::HashMap<String, String>>,
            }
            let a = args!(A);
            reply(
                crate::terminal::takeover_managed_terminal(
                    app.clone(),
                    state,
                    a.session_id,
                    a.cols,
                    a.rows,
                    a.options,
                )
                .await,
            )
        }
        "attach_background_session" => {
            let a = args!(SessionArg);
            reply(crate::managed_terminal::attach_background_session(state, a.session_id).await)
        }
        "send_background_prompt" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                text: String,
            }
            let a = args!(A);
            reply(
                crate::managed_terminal::send_background_prompt(state, a.session_id, a.text).await,
            )
        }
        "session_launch_selections" => {
            let a = args!(SessionArg);
            reply(crate::session_command::session_launch_selections(a.session_id).await)
        }
        "set_session_launch_selection" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                option: String,
                choice: String,
            }
            let a = args!(A);
            reply(
                crate::session_command::set_session_launch_selection(
                    a.session_id,
                    a.option,
                    a.choice,
                )
                .await,
            )
        }
        "pending_interaction" => {
            let a = args!(SessionArg);
            reply_ok(crate::managed_terminal::pending_interaction(
                state,
                a.session_id,
            ))
        }
        "register_approval_consumer" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                consumer_id: String,
            }
            let a = args!(A);
            reply(crate::managed_terminal::register_approval_consumer(
                state,
                a.session_id,
                remote_consumer_id(&a.consumer_id),
            ))
        }
        "unregister_approval_consumer" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                consumer_id: String,
            }
            let a = args!(A);
            crate::managed_terminal::unregister_approval_consumer(
                state,
                remote_consumer_id(&a.consumer_id),
            );
            reply_ok(())
        }
        "resolve_pending_approval" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                session_id: i64,
                request_id: String,
                choice: String,
            }
            let a = args!(A);
            reply(
                crate::managed_terminal::resolve_pending_approval(
                    state,
                    a.session_id,
                    a.request_id,
                    a.choice,
                )
                .await,
            )
        }
        "dismiss_interactive_question" => {
            let a = args!(SessionArg);
            crate::managed_terminal::dismiss_interactive_question(state, a.session_id);
            reply_ok(())
        }
        "save_pasted_attachment" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                file_name: String,
                data_base64: String,
            }
            let a = args!(A);
            reply(crate::chat::save_pasted_attachment(a.file_name, a.data_base64).await)
        }
        "list_agents" => reply_ok(crate::list_agents().await),
        "agent_chat_ui" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                provider: String,
                #[serde(default)]
                cwd: Option<String>,
                #[serde(default)]
                session_id: Option<i64>,
            }
            let a = args!(A);
            reply_ok(crate::agent_chat_ui(a.provider, a.cwd, a.session_id).await)
        }
        "agent_models" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                provider: String,
            }
            let a = args!(A);
            reply_ok(crate::agent_models(a.provider).await)
        }
        "new_session" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                cwd: String,
                provider: String,
                /// 旧前端兼容参数，桌面命令同样忽略。
                #[serde(default)]
                #[allow(dead_code)]
                terminal: Option<String>,
                #[serde(default)]
                options: Option<std::collections::HashMap<String, String>>,
            }
            let a = args!(A);
            // 只启动托管 PTY，不 reveal（桌面命令的 reveal_session 会在宿主机弹窗/开
            // 外部终端——手机新建会话时桌面凭空开窗不可接受）。返回 () 与桌面契约对齐。
            let broker = state.ptys.clone();
            reply(
                crate::terminal::new_session_inner(
                    app.clone(),
                    broker,
                    a.cwd,
                    a.provider,
                    a.options,
                )
                .await
                .map(|_temp_id| ()),
            )
        }
        "check_provider_hooks" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct A {
                provider: String,
            }
            let a = args!(A);
            reply(crate::install::check_provider_hooks(a.provider).await)
        }
        // token 不回显给远端:调用方虽已持 token 过了鉴权,但「token 只在宿主机取得」
        // 是审计线——回显会让未来的 token 轮换在换发瞬间被旧凭据读走新值。
        "get_settings" => reply(crate::settings::get_settings().await.map(|mut s| {
            s.remote_access_token = String::new();
            s
        })),
        "host_os" => reply_ok(crate::host_os()),
        // BRIDGED_COMMANDS 有而这里漏写的项落到此处：fail-closed，宁 404 不放行。
        _ => err_status(StatusCode::NOT_FOUND, "未知命令"),
    }
}

/// 远端 consumer 强制 `remote:` 前缀（幂等）。在服务端加而非信任前端：桌面端无法
/// 伪造远端身份、远端也无法冒充桌面 consumer（PR2 的租约语义按前缀区分）。
fn remote_consumer_id(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("remote:") {
        format!("remote:{rest}")
    } else {
        format!("remote:{raw}")
    }
}

#[derive(serde::Deserialize)]
struct FileQuery {
    path: String,
    #[serde(default)]
    token: String,
}

/// `/file?path=&token=`：替代 convertFileSrc 的图片/附件读取。scope 精确镜像
/// tauri.conf.json 的 assetProtocol scope（`$HOME/.claude/image-cache/**` 与
/// `$TEMP/meowo-paste/**`）——桌面端 convertFileSrc 本来也只放行这两处。
async fn file_handler(State(ctx): State<Ctx>, Query(q): Query<FileQuery>) -> Response {
    if !token_matches(&ctx.token, &q.token) {
        return err_status(StatusCode::UNAUTHORIZED, "未授权");
    }
    let bytes = tauri::async_runtime::spawn_blocking(move || read_scoped_file(&q.path)).await;
    match bytes {
        Ok(Ok(bytes)) => {
            let mime = mime_for(sniff_image_ext(&bytes));
            ([("content-type", mime)], bytes).into_response()
        }
        Ok(Err(status)) => err_status(status, "无法读取"),
        Err(_) => err_status(StatusCode::INTERNAL_SERVER_ERROR, "内部错误"),
    }
}

/// 图片按内容嗅探而非扩展名：附件落盘名来自用户文件名，改错扩展名不该导致渲染失败。
/// 只认前端会渲染的几种位图/矢量，其余按二进制流。
fn sniff_image_ext(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, ..] => "jpg",
        [b'G', b'I', b'F', b'8', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        [b'<', ..] => "svg",
        _ => "",
    }
}

fn file_scope_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        roots.push(PathBuf::from(home).join(".claude").join("image-cache"));
    }
    roots.push(std::env::temp_dir().join("meowo-paste"));
    roots
}

fn read_scoped_file(raw: &str) -> Result<Vec<u8>, StatusCode> {
    let path = path_within_roots(raw, &file_scope_roots()).ok_or(StatusCode::FORBIDDEN)?;
    std::fs::read(path).map_err(|_| StatusCode::NOT_FOUND)
}

/// 请求路径与各 root **两侧都 canonicalize** 后做前缀比对：规避 `..` 逃逸、符号链接
/// 逃逸与 Windows `\\?\` 前缀不一致。root 不存在（canonicalize 失败）按不放行。
/// 纯函数便于单测。
fn path_within_roots(raw: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let requested = FsPath::new(raw).canonicalize().ok()?;
    if !requested.is_file() {
        return None;
    }
    for root in roots {
        if let Ok(root) = root.canonicalize() {
            if requested.starts_with(&root) {
                return Some(requested);
            }
        }
    }
    None
}

/// 手机 SPA 静态托管（手写 ServeDir-lite，不引 tower-http）。产物目录：
/// release 走 resource_dir()/dist-mobile（PR3 接 bundle.resources），debug 回退
/// 仓库内 dist-mobile 便于本地调试。目录缺失 → 404 提示先构建。
async fn static_handler(State(ctx): State<Ctx>, uri: axum::http::Uri) -> Response {
    let Some(root) = static_root(&ctx.app) else {
        return err_status(
            StatusCode::NOT_FOUND,
            "手机端页面未构建（bun run build:mobile）",
        );
    };
    let Some(rel) = sanitize_static_path(uri.path()) else {
        return err_status(StatusCode::NOT_FOUND, "无此资源");
    };
    let full = root.join(&rel);
    let read = tauri::async_runtime::spawn_blocking(move || std::fs::read(full)).await;
    match read {
        Ok(Ok(bytes)) => {
            let ext = rel.rsplit('.').next().unwrap_or("");
            ([("content-type", mime_for(ext))], bytes).into_response()
        }
        _ => err_status(StatusCode::NOT_FOUND, "无此资源"),
    }
}

fn static_root(app: &tauri::AppHandle) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist-mobile");
        if dev.is_dir() {
            return Some(dev);
        }
    }
    // tauri.conf.json 用 `resources: ["../dist-mobile"]` 打包(产物在 src-tauri 之外)。
    // Tauri 把越过 tauri 目录的资源路径里的 `..` 折叠成 `_up_` 段以保留在 resource_dir 内,
    // 故安装后实际落点是 resource_dir()/_up_/dist-mobile。两处都探,谁在用谁(未来 Tauri
    // 若改回不折叠也不误伤)。
    let base = app.path().resource_dir().ok()?;
    [base.join("dist-mobile"), base.join("_up_").join("dist-mobile")]
        .into_iter()
        .find(|d| d.is_dir())
}

/// URL path → 产物目录内相对路径。只接受 `[A-Za-z0-9._-]` 的单纯段（vite 产物
/// 命名域），任何 `..`/盘符/反斜杠/空段直接拒绝。`/` → mobile.html。纯函数便于单测。
fn sanitize_static_path(path: &str) -> Option<String> {
    if path == "/" {
        return Some("mobile.html".into());
    }
    let mut parts = Vec::new();
    for seg in path.trim_start_matches('/').split('/') {
        if seg.is_empty()
            || seg.starts_with('.')
            || !seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return None;
        }
        parts.push(seg);
    }
    Some(parts.join("/"))
}

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare_rejects_mismatch_and_empty() {
        assert!(token_matches("abcd", "abcd"));
        assert!(!token_matches("abcd", "abce"));
        assert!(!token_matches("abcd", "abc"));
        assert!(!token_matches("", ""));
        assert!(!token_matches("abcd", ""));
    }

    #[test]
    fn generated_tokens_are_long_and_unique() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.len() >= MIN_TOKEN_LEN);
    }

    /// 白名单是 default-deny 的唯一事实源：敏感命令绝不许出现。此测试是审计基线的
    /// 可执行形式——往名单里加这些项必须先过这里。
    #[test]
    fn bridged_commands_exclude_sensitive_surface() {
        const FORBIDDEN: &[&str] = &[
            "get_relay_secrets",
            "set_relay_secret",
            "set_settings",
            "open_url",
            "open_link",
            "open_path_with",
            "open_project_dir",
            "reveal_path_in_file_manager",
            "open_attached_terminal",
            "open_chat_window",
            "confirm_dialog",
            "login_agent",
            "api_key_login",
            "install_agent",
            "check_update",
            "download_update",
            "install_downloaded_update",
            "read_file_text",
            "list_dir_entries",
            "search_project_files",
            "git_diff_summary",
            "git_file_diff",
            "stop_managed_terminal",
            "snap_expand",
            "snap_collapse",
            "set_autostart",
            "clipboard_text",
            "clipboard_set_image",
        ];
        for cmd in FORBIDDEN {
            assert!(
                !BRIDGED_COMMANDS.contains(cmd),
                "{cmd} 属拒绝面，绝不许进远程白名单"
            );
        }
        // 抽查核心放行项在场（防手滑清空名单导致整功能静默变哑）。
        for cmd in [
            "get_chat_history",
            "write_managed_terminal",
            "pending_interaction",
        ] {
            assert!(BRIDGED_COMMANDS.contains(&cmd), "{cmd} 应在白名单");
        }
    }

    /// 前端 RemoteAccessInfo 类型按 camelCase 读 `lastError`：serde rename 一旦失守,
    /// 设置页错误提示与 QR 端口会静默取到 undefined。
    #[test]
    fn remote_access_info_serializes_camel_case() {
        let info = RemoteAccessInfo {
            enabled: true,
            port: 18620,
            token: "tok".into(),
            ips: vec![IpCandidate { ip: "192.168.1.5".into(), kind: "lan" }],
            last_error: Some("端口占用".into()),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""lastError":"端口占用""#), "{json}");
        assert!(json.contains(r#""ips":[{"ip":"192.168.1.5","kind":"lan"}]"#), "{json}");
        assert!(json.contains(r#""port":18620"#), "{json}");
    }

    /// 归类是配对地址的守门员：Tailscale CGNAT 与 RFC1918 之外（TUN 假地址、
    /// link-local、回环、公网）一律丢弃——错的地址进了二维码，用户只会扫出「打不开」。
    #[test]
    fn classify_ip_labels_known_ranges_and_drops_garbage() {
        let c = |s: &str| classify_ip(s.parse().unwrap());
        assert_eq!(c("100.64.0.1"), Some("tailscale"));
        assert_eq!(c("100.127.255.254"), Some("tailscale"));
        assert_eq!(c("192.168.1.5"), Some("lan"));
        assert_eq!(c("10.0.0.2"), Some("lan"));
        assert_eq!(c("172.16.0.1"), Some("lan"));
        // 100.64/10 之外的 100.x 是普通公网,不得冒充 Tailscale。
        assert_eq!(c("100.128.0.1"), None);
        assert_eq!(c("198.18.0.1"), None); // TUN 代理 fake-ip 常用段
        assert_eq!(c("169.254.1.1"), None); // link-local
        assert_eq!(c("127.0.0.1"), None);
        assert_eq!(c("0.0.0.0"), None);
        assert_eq!(c("8.8.8.8"), None); // 公网
        assert_eq!(c("fe80::1"), None); // v6 一律不收
    }

    #[test]
    fn remote_consumer_prefix_is_idempotent() {
        assert_eq!(remote_consumer_id("abc"), "remote:abc");
        assert_eq!(remote_consumer_id("remote:abc"), "remote:abc");
    }

    #[test]
    fn file_scope_rejects_escape_and_missing_roots() {
        let dir = std::env::temp_dir().join(format!("meowo-remote-test-{}", std::process::id()));
        let inside = dir.join("ok.png");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&inside, b"x").unwrap();
        let outside =
            std::env::temp_dir().join(format!("meowo-remote-out-{}.png", std::process::id()));
        std::fs::write(&outside, b"x").unwrap();

        let roots = vec![dir.clone()];
        // 界内文件放行。
        assert!(path_within_roots(inside.to_str().unwrap(), &roots).is_some());
        // 界外文件、`..` 逃逸、不存在的 root 一律拒绝。
        assert!(path_within_roots(outside.to_str().unwrap(), &roots).is_none());
        let escape = dir
            .join("..")
            .join(outside.file_name().unwrap().to_str().unwrap());
        assert!(path_within_roots(escape.to_str().unwrap(), &roots).is_none());
        assert!(
            path_within_roots(inside.to_str().unwrap(), &[dir.join("does-not-exist")]).is_none()
        );
        // 目录本身不是文件，不放行。
        assert!(path_within_roots(dir.to_str().unwrap(), &roots).is_none());

        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn static_path_sanitizer_blocks_traversal() {
        assert_eq!(sanitize_static_path("/").as_deref(), Some("mobile.html"));
        assert_eq!(
            sanitize_static_path("/assets/index-abc.js").as_deref(),
            Some("assets/index-abc.js")
        );
        assert!(sanitize_static_path("/../secret").is_none());
        assert!(sanitize_static_path("/assets/../../x").is_none());
        assert!(sanitize_static_path("/a\\b").is_none());
        assert!(sanitize_static_path("/.hidden").is_none());
        assert!(sanitize_static_path("/a//b").is_none());
        assert!(sanitize_static_path("/C:/Windows/win.ini").is_none());
    }

    /// camelCase 契约：参数结构体按 camelCase 逐名匹配，未知键必须报错（deny_unknown_fields
    /// 生效）——snake_case 键静默当缺失曾在 get_live_sessions_page 上造成过游标失效。
    #[test]
    fn rpc_args_deserialize_camel_case_and_deny_unknown() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct A {
            session_id: i64,
            #[serde(default)]
            full: Option<bool>,
        }
        let ok: A = serde_json::from_str(r#"{"sessionId":7}"#).unwrap();
        assert_eq!(ok.session_id, 7);
        assert_eq!(ok.full, None);
        assert!(serde_json::from_str::<A>(r#"{"session_id":7}"#).is_err());
        assert!(serde_json::from_str::<A>(r#"{"sessionId":7,"extra":1}"#).is_err());
    }
}
