//! 远程访问桥：局域网/Tailscale 内的手机浏览器经 HTTP 使用对话页。
//!
//! # 威胁模型与安全基线（审计从这里开始）
//!
//! 与 pty.rs 的 attach broker 不同，本服务**默认监听所有网卡**——全程明文 HTTP，
//! 网络层安全交给绑定面收敛与局域网/Tailscale 假设（文档明示：公网勿开）。绑定面
//! 由 `remote_access_bind` 设置收敛（settings.rs）：all（0.0.0.0，缺省兼容）/
//! loopback / tailscale；tailscale 模式找不到接口**拒绝启动**，绝不静默回退 0.0.0.0。
//! 应用层只保两件事：
//!
//! 1. **token 门**：`/rpc` 走 `X-Meowo-Token` 头、`/file` 走 `?token=` 查询串
//!    （`<img src>` 发不了自定义头），常量时间比较。token 由 [`generate_token`]
//!    严格生成（OS RNG 失败即拒绝；pty.rs 的 broker token 也复用它——只听 loopback
//!    不等于可以接受可预测的弱随机）。静态资源不鉴权（构建产物无秘密）。
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
//!
//! 注意：白名单内的 `write_managed_terminal` 本身就等于「在宿主机上执行任意命令」
//! （往 agent CLI 的 PTY 写任意按键）——这是产品功能（远程发消息/打断/答题），
//! 不是疏漏；它成立的前提正是上面的 token 门 + 网络层假设。
//!
//! `stop_managed_terminal`（结束会话）曾列在拒绝面（「远端不给杀会话」）。2026-09-04 改为
//! 放行：用户实际需要在手机上结束会话；且拒绝它并不构成安全边界——`takeover_managed_terminal`
//! 本就先杀旧进程，`write_managed_terminal` 更能直接送 Ctrl+C / `exit`。它只作用于本会话的
//! 进程树（托管 PTY 或按 pid 兜底的孤儿），仍不是宿主级生杀。
//!
//! # 与 Tauri command 的耦合约定
//!
//! 桥接臂直接调用各 `#[tauri::command]` 函数本体（`app.state::<AppState>()` 取
//! State，先例见 lib.rs `install_downloaded_update` / watch.rs）。参数结构体
//! `rename_all = "camelCase"` + `deny_unknown_fields`，与前端 invoke 的调用契约
//! 逐名对齐；返回值原样透传 serde 结果（DTO 的 case 本就不统一，桥不做二次加工）。
//! 若某命令日后新增 `Window` 参数，这里会**编译期变红**——显式耦合，属预期。
//!
//! 例外：`new_session` 的返回值与桌面契约刻意分叉——桌面回 ()（reveal_session 即导航），
//! 桥透传临时负 id 供移动页选中新会话（详见该臂注释）；参数契约仍逐名对齐。
//!
//! 反例教训（2026-08-31）：api.ts 给 `newSession` 加 `extraDirs` 时没同步桥臂，
//! deny_unknown_fields 把手机端每次新建会话都打成 400（字段空也发 `[]`，无可选豁免）。
//! **改 api.ts 的 invoke 传参时必须同步本文件对应臂**——契约仍是人肉对齐，但有机制
//! 兜底：tests::every_bridged_command_sample_payload_parses 给每条臂钉了一份镜像
//! api.ts 实参的样例 payload，api.ts 加键后同步样例、桥臂没跟上即红。

use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
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
    "managed_terminal_grid",
    "managed_terminal_binding",
    "write_managed_terminal",
    "resize_managed_terminal",
    "start_managed_terminal",
    "takeover_managed_terminal",
    // 结束会话：只杀本会话进程树（模块头注释记录了放行理由）。
    "stop_managed_terminal",
    "attach_background_session",
    "send_background_prompt",
    "session_launch_selections",
    "set_session_launch_selection",
    // 审批/交互提问
    "pending_interaction",
    "awaiting_interaction_sessions",
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
    "list_subdirectories",
    // 只读杂项
    "get_settings",
    "host_os",
    // /file 降级凭据领取:须持主 token 才调得动,回的是只够读图片目录的次级凭据。
    "file_access_token",
    // 账号载荷只有展示字段(email/plan/登录方式标签),无任何密钥;新建会话页
    // 靠它提示「当前用的是非默认账号」,不加白会静默丢提示。
    "get_accounts",
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
    /// 实际绑定的监听地址：换绑定模式（all/loopback/tailscale）要触发重启，差异比对靠它。
    bind: std::net::IpAddr,
    token: String,
    /// /file 降级凭据(见 Ctx::file_token)。每次启动新发,重启即吊销旧值。
    file_token: String,
    /// notify_one 存 permit：serve 任务尚未开始等也不会丢关停信号。
    shutdown: Arc<tokio::sync::Notify>,
}

/// 严格 token 生成：OS RNG 不可用直接失败（调用方应拒绝启用对应功能）。
/// pty.rs 的 broker/launch token 亦走本函数——那里曾有 pid+时间戳的弱随机回退，
/// 已删除：loopback 语境的论证（只听本机）不成立，本机任意进程同样能来试门。
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

/// UDP 路由探针（零依赖）：`connect` 只让内核按路由表选出口网卡、不实际发包，
/// 再读 `local_addr` 拿本机在该网卡上的地址。local_ips 与 tailscale_ipv4 共用。
fn route_probe_ip(probe: &str) -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect(probe).ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

/// 本机 Tailscale IPv4（100.64.0.0/10），tailscale 绑定模式的探测。不枚举网卡、不 spawn
/// `tailscale ip -4`（CLI 不一定在 PATH，还多一个进程依赖）：Tailscale 接管 100.64/10 的
/// 路由，CGNAT 探针的 local_addr 即 tailnet 地址；没装 Tailscale 时探针按默认路由回
/// 局域网地址，classify_ip 兜住判 None。Windows 上 Tailscale 是 Wintun 接口，路由表行为一致。
fn tailscale_ipv4() -> Option<std::net::IpAddr> {
    let ip = route_probe_ip("100.100.100.100:80")?;
    (classify_ip(ip) == Some("tailscale")).then_some(ip)
}

/// 绑定模式 → 实际监听地址。纯函数（探测结果由调用方注入）便于单测。
/// tailscale 模式找不到地址是 Err——调用方拒绝启动，绝不静默回退 0.0.0.0：
/// 用户显式收窄过的暴露面，不能因 Tailscale 掉线悄悄放大回全网卡。
/// 未知值按 all 处理：旧版没有此字段，行为就是 0.0.0.0（手改 settings.json 塞错值同理）。
fn resolve_bind_addr(
    mode: &str,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Result<std::net::IpAddr, String> {
    match mode {
        "loopback" => Ok(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        "tailscale" => tailscale_ip.ok_or_else(|| {
            "未找到 Tailscale 接口（100.x 地址），远程访问未启动——请确认 Tailscale 已登录，或改用其他绑定模式".to_string()
        }),
        _ => Ok(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
    }
}

/// 可达地址枚举：`100.100.100.100`（Tailscale 的 CGNAT 探针）取 tailnet 出口，
/// `8.8.8.8` 取默认网卡出口。Tailscale 探测在前：有 tailnet 时它是唯一不挑手机所在
/// 网络的地址，列表首位即前端默认选中项。
/// 归类失败（如 TUN 假地址）逐个丢弃，全空则设置页回退提示手输 IP。
fn local_ips() -> Vec<IpCandidate> {
    let mut out: Vec<IpCandidate> = Vec::new();
    for probe in ["100.100.100.100:80", "8.8.8.8:80"] {
        let Some(ip) = route_probe_ip(probe) else {
            continue;
        };
        // 没装 Tailscale 时 CGNAT 探针照样按默认路由回局域网地址——归类兜住，去重兜住。
        let Some(kind) = classify_ip(ip) else {
            continue;
        };
        let ip = ip.to_string();
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
    /// 本实例**实际监听**的端口（server 在跑时）；未运行为 None。与 `port`（settings
    /// 配置值）可能分叉：多实例共享 settings.json，另一实例改端口写盘后本实例的
    /// listener 不会跟随——QR/提示必须按真实监听生成，否则指向别的实例的 server。
    bound_port: Option<u16>,
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
    let (last_error, bound_port) = {
        let runtime = app.state::<RemoteRuntime>();
        let inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        (inner.last_error.clone(), inner.running.as_ref().map(|r| r.port))
    };
    Ok(RemoteAccessInfo {
        enabled: settings.remote_access_enabled,
        port: settings.remote_access_port,
        token: settings.remote_access_token,
        ips,
        last_error,
        bound_port,
    })
}

/// /rpc get_settings 的出口消毒:token 不回显给远端。调用方虽已持 token 过了鉴权,
/// 但「token 只在宿主机取得」是审计线——回显会让未来的 token 轮换在换发瞬间被旧
/// 凭据读走新值。纯函数,便于单测钉住这条安全语义。
fn strip_remote_token(mut s: crate::settings::Settings) -> crate::settings::Settings {
    s.remote_access_token = String::new();
    // 代理地址可带 user:pass@,一并脱敏。持 token 者本就能执行命令,这不构成提权;但把
    // 「令牌泄露」的后果从「能操作这台机器」扩大到「顺手拿走一份可离线复用的代理凭据」
    // 是另一回事,与上面那条出口消毒的审计线也不一致。host/port 保留,远端仍看得出配了什么。
    s.proxy.url = crate::proxy::redact_credentials(&s.proxy.url).into_owned();
    for rule in s.proxy.per_agent.values_mut() {
        rule.url = crate::proxy::redact_credentials(&rule.url).into_owned();
    }
    s
}

/// 远程新建会话的目录浏览（只回目录名，不读文件内容）。远端持 token 者本就能以
/// 任意 cwd 起会话（new_session 在白名单），列目录不扩大能力面；文件读取仍不下放。
///
/// 已知取舍(自审 M7):这确实给了持 token 者「枚举整机目录名」的读信息面,与 /file
/// 的严格 scope 不同调。判定:配对 token 的信任模型本就是「设备主人」(它能发消息、
/// 起会话、批审批),目录名相对这些能力不是更高密级;真正的密级线是文件**内容**,
/// 仍锁死在 /file 的两个图片目录。若未来引入低权限分享 token,此命令须随之分级。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirListing {
    /// 当前目录（规范化绝对路径）；列磁盘根时为空串。
    path: String,
    /// 上一级路径。磁盘根的上一级用空串表示「磁盘列表」；已在磁盘列表时为 None。
    parent: Option<String>,
    dirs: Vec<DirChild>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirChild {
    name: String,
    path: String,
}

/// 仅经 /rpc 桥接调用（桌面有系统目录对话框，不需要它），故不做 #[tauri::command]。
pub(crate) async fn list_subdirectories(path: Option<String>) -> Result<DirListing, String> {
    tauri::async_runtime::spawn_blocking(move || list_subdirectories_sync(path.as_deref()))
        .await
        .map_err(|e| e.to_string())?
}

/// 顶层入口：Windows 枚举实际存在的盘符，类 Unix 只有 `/`。
fn filesystem_roots() -> Vec<PathBuf> {
    if cfg!(windows) {
        (b'A'..=b'Z')
            .map(|c| PathBuf::from(format!("{}:\\", c as char)))
            .filter(|p| p.is_dir())
            .collect()
    } else {
        vec![PathBuf::from("/")]
    }
}

fn list_subdirectories_sync(path: Option<&str>) -> Result<DirListing, String> {
    let raw = path.map(str::trim).filter(|p| !p.is_empty());
    let Some(raw) = raw else {
        let dirs = filesystem_roots()
            .into_iter()
            .map(|p| DirChild {
                name: p.display().to_string(),
                path: p.display().to_string(),
            })
            .collect();
        return Ok(DirListing {
            path: String::new(),
            parent: None,
            dirs,
        });
    };
    // dunce 而非 std::fs::canonicalize：后者在 Windows 回 \\?\ 前缀路径，
    // 展示难看，回填成 cwd 后部分 agent 也不认。
    let dir = dunce::canonicalize(raw).map_err(|_| "目录不存在或不可读".to_string())?;
    if !dir.is_dir() {
        return Err("不是目录".to_string());
    }
    let mut dirs = Vec::new();
    for ent in std::fs::read_dir(&dir)
        .map_err(|_| "目录不可读".to_string())?
        .flatten()
    {
        // file_type 不追符号链接：目录软链下钻可能成环，跳过（手输路径仍可进）。
        if !ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        // 隐藏目录多为工具内部产物，浏览列表里是噪音；手输路径仍可进入。
        if name.starts_with('.') {
            continue;
        }
        dirs.push(DirChild {
            path: ent.path().display().to_string(),
            name,
        });
    }
    dirs.sort_by_key(|d| d.name.to_lowercase());
    // 防超大目录打爆载荷；远超一屏的列表本也没法浏览，靠手输缩小范围。
    dirs.truncate(500);
    let parent = dir
        .parent()
        .map(|p| p.display().to_string())
        .or(Some(String::new()));
    Ok(DirListing {
        path: dir.display().to_string(),
        parent,
        dirs,
    })
}

/// 设置页「重新生成令牌」（**桌面专用命令，不进 /rpc 白名单**——远端不许自己续命）。
/// 换发即吊销：apply 比对 token 差异会重启 server，所有已配对手机 401 回配对页。
#[tauri::command]
pub(crate) async fn regenerate_remote_token(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::settings::update_settings(|s| {
            s.remote_access_token = generate_token()?;
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    apply(&app);
    Ok(())
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
            &settings.remote_access_bind,
        )
        .await;
    });
}

async fn apply_with(
    app: &tauri::AppHandle,
    enabled: bool,
    port: u32,
    token: String,
    bind_mode: &str,
) {
    let runtime = app.state::<RemoteRuntime>();
    let _serial = runtime.apply_lock.lock().await;

    let set_error = |error: Option<String>| {
        let mut inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.last_error = error;
    };

    // 绑定地址解析在锁外做（tailscale 模式有一次 UDP 路由探测，见 tailscale_ipv4）。
    // 找不到 Tailscale 接口是硬错误：停掉旧 server、报错拒绝启动——静默回退 0.0.0.0
    // 会把用户显式收窄的暴露面悄悄放大回全网卡明文。
    let bind_addr = if enabled {
        let probe = if bind_mode == "tailscale" {
            tailscale_ipv4()
        } else {
            None
        };
        match resolve_bind_addr(bind_mode, probe) {
            Ok(a) => a,
            Err(e) => {
                let to_stop = {
                    let mut inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.running.take()
                };
                if let Some(running) = to_stop {
                    running.shutdown.notify_one();
                }
                set_error(Some(e));
                return;
            }
        }
    } else {
        // 占位值，disabled 分支不会用到。
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
    };

    // 临界区不跨 await：先在锁内比对并摘下旧 server，再在锁外发关停信号。
    let to_stop = {
        let mut inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        let unchanged = matches!(
            &inner.running,
            Some(r) if enabled && u32::from(r.port) == port && r.token == token && r.bind == bind_addr
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

    // 重启换发(改端口/token/重新生成令牌)时,旧 server 的 shutdown 已 notify,但它的
    // listener 要等那个 detached 任务下一次 poll 才 drop——这中间同端口 bind 会
    // WSAEADDRINUSE。短退避重试几次,躲开这段交接窗口;真被别的进程占着才如实报错。
    let listener = {
        let mut attempt = 0u32;
        loop {
            match tokio::net::TcpListener::bind((bind_addr, port16)).await {
                Ok(l) => break l,
                Err(e) if attempt < 10 => {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    let _ = e;
                }
                Err(e) => {
                    set_error(Some(format!("绑定 {bind_addr}:{port16} 失败：{e}")));
                    return;
                }
            }
        }
    };
    let shutdown = Arc::new(tokio::sync::Notify::new());
    // /file 降级凭据:RNG 不可用就直接不启动(与主 token 同一严格标准——静默退化成
    // 「file_token 恒空串」会让空 token 匹配逻辑埋雷)。
    let file_token = match generate_token() {
        Ok(t) => t,
        Err(e) => {
            set_error(Some(format!("生成图片凭据失败：{e}")));
            return;
        }
    };
    let router = build_router(app.clone(), token.clone(), file_token.clone());
    {
        let mut inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.running = Some(Running {
            port: port16,
            bind: bind_addr,
            token,
            file_token,
            shutdown: shutdown.clone(),
        });
        inner.last_error = None;
    }
    if bind_addr.is_unspecified() {
        eprintln!(
            "[remote] 远程访问已启动：{bind_addr}:{port16}（全网卡明文 HTTP：token、聊天与按键内容对同网设备可见，不可信网络请改用 Tailscale/loopback 绑定）"
        );
    } else {
        eprintln!("[remote] 远程访问已启动：{bind_addr}:{port16}");
    }
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
    /// /file 专用降级凭据(每次 server 启动随机新发,经 /rpc file_access_token 领取):
    /// <img src> 的 query 里不再携带主 token——它泄露(devtools/长按开新页/截图)只丢
    /// 「读两个图片目录」的能力,不丢 /rpc 全量能力。主 token 在 /file 上仍被接受
    /// (领取前的首屏图片经它回退加载)。
    file_token: Arc<str>,
}

fn build_router(app: tauri::AppHandle, token: String, file_token: String) -> Router {
    let ctx = Ctx {
        app,
        token: token.into(),
        file_token: file_token.into(),
    };
    // /rpc 的 token 门用 route_layer 前置到 body 提取之前:未鉴权请求在读 body 前就被
    // 401 掐掉,堵住「48MB body 在鉴权前就 buffer 进内存」的预鉴权内存耗尽面(0.0.0.0
    // 上任何同网设备可打)。route_layer 只作用于本 Router 的已定义路由,不波及 /file
    // (query token)与静态资源。
    let rpc = Router::new()
        .route("/rpc/{command}", post(rpc_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            ctx.clone(),
            require_token,
        ));
    Router::new()
        .merge(rpc)
        .route("/file", get(file_handler))
        .fallback(get(static_handler))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(ctx)
}

/// /rpc 前置鉴权:只看 header,不碰 body。放行才让 handler 去提取 body。
async fn require_token(
    State(ctx): State<Ctx>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let provided = req
        .headers()
        .get("x-meowo-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !token_matches(&ctx.token, provided) {
        return err_status(StatusCode::UNAUTHORIZED, "remote/unauthorized");
    }
    next.run(req).await
}

/// invoke 语义对齐：Ok → 200 + JSON 值；Err(String) → 400 + JSON 字符串（前端
/// transport 据此 reject，错误串走既有 formatBackendError 链路）。
///
/// 7M-7：桥自身产生的错误（鉴权/未知命令/参数不合法/未就绪）改用 `remote/<code>`
/// 结构化 reason——它们此前是裸中文串，英文界面下直接漏中文，也进不了 errors.ts 的
/// 映射表。业务命令自己的 Err 原样透传，仍走各自的 sentinel。
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
    serde_json::from_slice(raw).map_err(|e| format!("remote/bad_args：{e}"))
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
        return err_status(StatusCode::UNAUTHORIZED, "remote/unauthorized");
    }
    if !BRIDGED_COMMANDS.contains(&command.as_str()) {
        // 未放行与不存在同响应，不区分——不给探测面。
        return err_status(StatusCode::NOT_FOUND, "remote/unknown_command");
    }
    dispatch(&ctx.app, &command, &body).await
}

/// 各臂参数结构体（模块级）。原先是 dispatch 各臂内的匿名 `struct A`，测试够不着；
/// 机械外提（字段、serde 属性一字未改）只为让「样例 payload → 不 400」契约测试拿得到
/// 类型（见 tests::every_bridged_command_sample_payload_parses）。camelCase +
/// deny_unknown_fields 由宏统一强制——新臂手写漏了 deny_unknown_fields 会退化成
/// 静默丢键，不许再有这个自由度。
macro_rules! bridged_args {
    ($name:ident { $( $(#[$attr:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct $name {
            $( $(#[$attr])* $field : $ty, )*
        }
    };
}

// sessionId 单参臂共用。
bridged_args!(SessionArg { session_id: i64 });

bridged_args!(GetLiveSessionsPageArgs {
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
    #[serde(default)]
    include_pending: Option<bool>
});

bridged_args!(SearchChatTranscriptsArgs { query: String });

bridged_args!(RecentCwdsArgs { limit: usize });

bridged_args!(RenameSessionArgs {
    #[serde(default)]
    cwd: Option<String>,
    session_id: String,
    title: String,
    #[serde(default)]
    provider: Option<String>
});

bridged_args!(SetArchivedArgs {
    session_id: i64,
    archived: bool
});

bridged_args!(SetSessionNoteArgs {
    session_id: String,
    note: String
});

bridged_args!(StopManagedTerminalArgs {
    session_id: i64
});

bridged_args!(GetChatHistoryArgs {
    session_id: i64,
    offset: u64,
    #[serde(default)]
    full: Option<bool>,
    #[serde(default)]
    before: Option<u64>
});

bridged_args!(GetSubagentTranscriptArgs {
    session_id: i64,
    tool_use_id: String
});

bridged_args!(ManagedTerminalSnapshotArgs {
    session_id: i64,
    #[serde(default)]
    since: Option<u64>
});

bridged_args!(WriteManagedTerminalArgs {
    session_id: i64,
    data: String
});

bridged_args!(ResizeManagedTerminalArgs {
    session_id: i64,
    cols: u16,
    rows: u16
});

bridged_args!(StartManagedTerminalArgs {
    session_id: i64,
    cols: u16,
    rows: u16,
    #[serde(default)]
    options: Option<std::collections::HashMap<String, String>>
});

bridged_args!(TakeoverManagedTerminalArgs {
    session_id: i64,
    cols: u16,
    rows: u16,
    #[serde(default)]
    options: Option<std::collections::HashMap<String, String>>
});

bridged_args!(SendBackgroundPromptArgs {
    session_id: i64,
    text: String
});

bridged_args!(SetSessionLaunchSelectionArgs {
    session_id: i64,
    option: String,
    choice: String
});

bridged_args!(RegisterApprovalConsumerArgs {
    session_id: i64,
    consumer_id: String
});

bridged_args!(UnregisterApprovalConsumerArgs { consumer_id: String });

bridged_args!(ResolvePendingApprovalArgs {
    session_id: i64,
    request_id: String,
    choice: String
});

bridged_args!(SavePastedAttachmentArgs {
    file_name: String,
    data_base64: String
});

bridged_args!(AgentChatUiArgs {
    provider: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    session_id: Option<i64>
});

bridged_args!(AgentModelsArgs { provider: String });

bridged_args!(NewSessionArgs {
    cwd: String,
    provider: String,
    /// 旧前端兼容参数，桌面命令同样忽略。
    #[serde(default)]
    #[allow(dead_code)]
    terminal: Option<String>,
    #[serde(default)]
    options: Option<std::collections::HashMap<String, String>>,
    // 移动页与桌面共用 NewSessionPanel，api.ts 恒带 extraDirs（空也为 []）——
    // 缺这个字段 deny_unknown_fields 会把手机端每次新建都打成 400。
    #[serde(default)]
    extra_dirs: Option<Vec<String>>
});

bridged_args!(CheckProviderHooksArgs { provider: String });

bridged_args!(ListSubdirectoriesArgs {
    #[serde(default)]
    path: Option<String>
});

/// 白名单命令 → 各 command 函数本体。参数结构体见上方模块级定义（camelCase +
/// deny_unknown_fields），与 api.ts 的 invoke 传参逐名对齐。
async fn dispatch(app: &tauri::AppHandle, command: &str, body: &[u8]) -> Response {
    macro_rules! args {
        ($t:ty) => {
            match parse::<$t>(body) {
                Ok(v) => v,
                Err(message) => return err_status(StatusCode::BAD_REQUEST, &message),
            }
        };
    }

    let state = app.state::<crate::AppState>();
    match command {
        "get_live_sessions_page" => {
            let a = args!(GetLiveSessionsPageArgs);
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
                    a.include_pending,
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
            let a = args!(SearchChatTranscriptsArgs);
            reply(crate::chat::search_chat_transcripts(state, a.query).await)
        }
        "recent_cwds" => {
            let a = args!(RecentCwdsArgs);
            reply(crate::session_query::recent_cwds(state, a.limit).await)
        }
        "rename_session" => {
            let a = args!(RenameSessionArgs);
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
            let a = args!(SetArchivedArgs);
            reply(
                crate::session_command::set_archived(app.clone(), state, a.session_id, a.archived)
                    .await,
            )
        }
        "set_session_note" => {
            let a = args!(SetSessionNoteArgs);
            reply(
                crate::session_command::set_session_note(app.clone(), state, a.session_id, a.note)
                    .await,
            )
        }
        "get_chat_history" => {
            let a = args!(GetChatHistoryArgs);
            reply(
                crate::chat::get_chat_history(state, a.session_id, a.offset, a.full, a.before)
                    .await,
            )
        }
        "get_subagent_transcript" => {
            let a = args!(GetSubagentTranscriptArgs);
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
        "managed_terminal_grid" => {
            let a = args!(SessionArg);
            reply(crate::managed_terminal::managed_terminal_grid(state, a.session_id).await)
        }
        "managed_terminal_snapshot" => {
            let a = args!(ManagedTerminalSnapshotArgs);
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
            let a = args!(WriteManagedTerminalArgs);
            reply(
                crate::managed_terminal::write_managed_terminal(state, a.session_id, a.data).await,
            )
        }
        "resize_managed_terminal" => {
            let a = args!(ResizeManagedTerminalArgs);
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
            let a = args!(StartManagedTerminalArgs);
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
            let a = args!(TakeoverManagedTerminalArgs);
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
        "stop_managed_terminal" => {
            let a = args!(StopManagedTerminalArgs);
            reply(crate::managed_terminal::stop_managed_terminal(state, a.session_id).await)
        }
        "attach_background_session" => {
            let a = args!(SessionArg);
            reply(crate::managed_terminal::attach_background_session(state, a.session_id).await)
        }
        "send_background_prompt" => {
            let a = args!(SendBackgroundPromptArgs);
            reply(
                crate::managed_terminal::send_background_prompt(state, a.session_id, a.text).await,
            )
        }
        "session_launch_selections" => {
            let a = args!(SessionArg);
            reply(crate::session_command::session_launch_selections(a.session_id).await)
        }
        "set_session_launch_selection" => {
            let a = args!(SetSessionLaunchSelectionArgs);
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
        // 无参命令,载荷忽略(同 get_settings/host_os)。
        "awaiting_interaction_sessions" => reply_ok(
            crate::managed_terminal::awaiting_interaction_sessions(state),
        ),
        "register_approval_consumer" => {
            let a = args!(RegisterApprovalConsumerArgs);
            reply(crate::managed_terminal::register_approval_consumer(
                state,
                a.session_id,
                remote_consumer_id(&a.consumer_id),
            ))
        }
        "unregister_approval_consumer" => {
            let a = args!(UnregisterApprovalConsumerArgs);
            crate::managed_terminal::unregister_approval_consumer(
                state,
                remote_consumer_id(&a.consumer_id),
            );
            reply_ok(())
        }
        "resolve_pending_approval" => {
            let a = args!(ResolvePendingApprovalArgs);
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
            let a = args!(SavePastedAttachmentArgs);
            reply(crate::chat::save_pasted_attachment(a.file_name, a.data_base64).await)
        }
        "list_agents" => reply_ok(crate::list_agents().await),
        "agent_chat_ui" => {
            let a = args!(AgentChatUiArgs);
            reply_ok(crate::agent_chat_ui(a.provider, a.cwd, a.session_id).await)
        }
        "agent_models" => {
            let a = args!(AgentModelsArgs);
            reply_ok(crate::agent_models(a.provider).await)
        }
        "new_session" => {
            let a = args!(NewSessionArgs);
            // 只启动托管 PTY，不 reveal（桌面命令的 reveal_session 会在宿主机弹窗/开
            // 外部终端——手机新建会话时桌面凭空开窗不可接受）。
            // 返回值与桌面契约刻意**不同**：桌面命令回 ()（reveal 即导航，前端无需句柄）；
            // 桥没有 reveal 这条导航，把临时负 id 透传给移动页——前端据此立即选中新会话
            // （T-13 binding 轮询认领成真 id），否则用户被丢回「去侧栏选会话」空态。
            // api.ts newSession 为 Promise<number | null>：桌面 undefined→null，桥→temp_id。
            let broker = state.ptys.clone();
            reply(
                crate::terminal::new_session_inner(
                    app.clone(),
                    broker,
                    a.cwd,
                    a.provider,
                    a.options,
                    a.extra_dirs,
                )
                .await,
            )
        }
        "check_provider_hooks" => {
            let a = args!(CheckProviderHooksArgs);
            reply(crate::install::check_provider_hooks(a.provider).await)
        }
        "list_subdirectories" => {
            let a = args!(ListSubdirectoriesArgs);
            reply(list_subdirectories(a.path).await)
        }
        "get_settings" => reply(
            crate::settings::get_settings()
                .await
                .map(strip_remote_token),
        ),
        "host_os" => reply_ok(crate::host_os()),
        "file_access_token" => {
            let runtime = app.state::<RemoteRuntime>();
            let inner = runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
            match &inner.running {
                Some(r) => reply_ok(r.file_token.clone()),
                // 请求既然进得来,server 必在跑;此臂只是锁竞态下的兜底。
                None => err_status(StatusCode::SERVICE_UNAVAILABLE, "remote/not_ready"),
            }
        }
        // reply_ok 前提:get_accounts 返回 Vec 而非 Result(Result 会被 serde 包成
        // {"Ok":…} 且照样 200)——改它签名时这里必须复检。
        "get_accounts" => reply_ok(crate::get_accounts().await),
        // BRIDGED_COMMANDS 有而这里漏写的项落到此处：fail-closed，宁 404 不放行。
        _ => err_status(StatusCode::NOT_FOUND, "remote/unknown_command"),
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
    // 主 token 或 /file 降级凭据(见 Ctx::file_token)均可:降级凭据是 <img src> 的
    // 常规载体,主 token 只在前端尚未领到降级凭据的首屏回退时出现。
    if !token_matches(&ctx.token, &q.token) && !token_matches(&ctx.file_token, &q.token) {
        return err_status(StatusCode::UNAUTHORIZED, "remote/unauthorized");
    }
    let bytes = tauri::async_runtime::spawn_blocking(move || read_scoped_file(&q.path)).await;
    match bytes {
        Ok(Ok(bytes)) => {
            let mime = mime_for(sniff_image_ext(&bytes));
            // 安全头(缺一不可):
            // - no-store:URL query 带 token,不许进浏览器/中间缓存。
            // - nosniff:附件内容用户可控,禁浏览器把 png 猜成 html 一类。
            // - CSP default-src 'none' + sandbox:SVG 附件可含 <script>,经 <img> 加载
            //   不执行,但用户长按「在新标签打开图片」会让它成为同源顶层文档、脚本运行、
            //   读走 localStorage 里的主 token(=write_managed_terminal=宿主命令执行)。
            //   sandbox(无 token)彻底关掉脚本与同源,file_token 分级才不被绕过。
            (
                [
                    ("content-type", mime),
                    ("cache-control", "no-store"),
                    ("x-content-type-options", "nosniff"),
                    ("content-security-policy", "default-src 'none'; sandbox"),
                ],
                bytes,
            )
                .into_response()
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
///
/// 压缩与缓存：vite.mobile.config.ts 给 assets/ 下的文本产物预生成同名 .br，客户端
/// Accept-Encoding 含 br 且 .br 在场就回它（首屏 1.1MB → 约 0.45MB）；没有 .br 或客户端
/// 不收就回原文件，行为不变。assets/ 文件名带内容哈希，标一年 immutable，二次打开零下载；
/// mobile.html 不带哈希，no-cache 让浏览器每次回源确认（1KB，代价可忽略）。
async fn static_handler(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    let Some(root) = static_root(&ctx.app) else {
        return err_status(
            StatusCode::NOT_FOUND,
            "手机端页面未构建（bun run build:mobile）",
        );
    };
    let Some(rel) = sanitize_static_path(uri.path()) else {
        return err_status(StatusCode::NOT_FOUND, "无此资源");
    };
    let wants_br = headers
        .get("accept-encoding")
        .and_then(|v| v.to_str().ok())
        .is_some_and(accepts_brotli);
    let full = root.join(&rel);
    let read = tauri::async_runtime::spawn_blocking(move || {
        if wants_br {
            let mut br = full.clone().into_os_string();
            br.push(".br");
            if let Ok(bytes) = std::fs::read(br) {
                return Ok((bytes, true));
            }
        }
        std::fs::read(full).map(|bytes| (bytes, false))
    })
    .await;
    match read {
        Ok(Ok((bytes, brotli))) => {
            let ext = rel.rsplit('.').next().unwrap_or("");
            let mut resp = (
                [
                    ("content-type", mime_for(ext)),
                    ("cache-control", cache_control_for(&rel)),
                    // 同一 URL 按 Accept-Encoding 回不同实体，中间缓存必须按它分桶。
                    ("vary", "accept-encoding"),
                ],
                bytes,
            )
                .into_response();
            if brotli {
                resp.headers_mut()
                    .insert("content-encoding", HeaderValue::from_static("br"));
            }
            resp
        }
        _ => err_status(StatusCode::NOT_FOUND, "无此资源"),
    }
}

/// Accept-Encoding 里是否接受 brotli：逐项拆 `br;q=0.8`，`q=0` 视为明确拒绝。
/// 不处理 `*`——只有显式列出 br 的客户端才给压缩体，保守但不会送出对方解不开的东西。
fn accepts_brotli(accept_encoding: &str) -> bool {
    accept_encoding.split(',').any(|item| {
        let mut parts = item.split(';');
        let name = parts.next().unwrap_or("").trim();
        if !name.eq_ignore_ascii_case("br") {
            return false;
        }
        !parts.any(|p| {
            p.trim()
                .strip_prefix("q=")
                .and_then(|q| q.trim().parse::<f32>().ok())
                .is_some_and(|q| q == 0.0)
        })
    })
}

/// 缓存策略：assets/ 下文件名带 vite 内容哈希，改内容必换名，可放心长缓存；其余
/// （mobile.html）名字固定，每次回源确认。
fn cache_control_for(rel: &str) -> &'static str {
    if rel.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
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
    // tauri.conf.json 用 map 形式 `resources: { "../dist-mobile": "dist-mobile" }` 打包:
    // 落点钉在 resource_dir()/dist-mobile。列表形式 `["../dist-mobile"]` 在 tauri-cli
    // 2.11.2 实测静默打包不出任何文件(解包安装器验证过),不要改回去。仍保留 `_up_`
    // 探测位:列表形式修好后若有人切回,`..` 会折叠成 `_up_` 段,双探不误伤。
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

    /// 白名单与 dispatch 手工分列两处,易漂移:白名单有、match 漏写 arm 的命令会撞
    /// `_ => 404` 静默变哑(看着已放行、实际永远「未知命令」)。逐条断言每个白名单命令
    /// 在本文件源码里有 `"<cmd>" =>` 臂——dispatch 里没有组合臂(唯一的 `|` 在 mime_for),
    /// substring 命中即等价「有独立臂」。反向(有臂无白名单)不查:那种命令 rpc_handler
    /// 的白名单前置检查直接 404,不构成越权,只是死代码。
    #[test]
    fn every_bridged_command_has_a_dispatch_arm() {
        let src = include_str!("remote.rs");
        for cmd in BRIDGED_COMMANDS {
            let needle = format!("\"{cmd}\" =>");
            assert!(
                src.contains(&needle),
                "{cmd} 在白名单但 dispatch 无对应臂——会静默 404"
            );
        }
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
            "stop_managed_terminal",
            "pending_interaction",
            "awaiting_interaction_sessions",
            "list_subdirectories",
            "get_accounts",
        ] {
            assert!(BRIDGED_COMMANDS.contains(&cmd), "{cmd} 应在白名单");
        }
    }

    /// /rpc get_settings 的出口消毒:remote_access_token 绝不回显(安全语义,曾无测试
    /// 裸奔——自审)。其余字段原样。
    #[test]
    fn strip_remote_token_blanks_only_the_token() {
        let s = crate::settings::Settings {
            remote_access_token: "1f1325d4deadbeef".into(),
            remote_access_port: 18621,
            remote_access_enabled: true,
            ..Default::default()
        };
        let out = strip_remote_token(s);
        assert_eq!(out.remote_access_token, "");
        assert_eq!(out.remote_access_port, 18621);
        assert!(out.remote_access_enabled);
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
            bound_port: Some(18621),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""lastError":"端口占用""#), "{json}");
        assert!(json.contains(r#""ips":[{"ip":"192.168.1.5","kind":"lan"}]"#), "{json}");
        assert!(json.contains(r#""port":18620"#), "{json}");
        assert!(json.contains(r#""boundPort":18621"#), "{json}");
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

    /// 绑定模式解析（纯函数）：loopback/all 直接给地址；tailscale 模式消费探测结果——
    /// 有 100.x 地址绑它，没有是硬错误（调用方拒绝启动），绝不静默回退 0.0.0.0；
    /// 未知值按 all（旧版无此字段，行为兼容）。tailscale_ipv4 本体走真路由表，不在这里测。
    #[test]
    fn resolve_bind_addr_maps_modes_and_refuses_silent_fallback() {
        let loopback = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        let any = std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED);
        let ts: std::net::IpAddr = "100.64.0.7".parse().unwrap();

        assert_eq!(resolve_bind_addr("loopback", None).unwrap(), loopback);
        assert_eq!(resolve_bind_addr("all", None).unwrap(), any);
        assert_eq!(resolve_bind_addr("garbage", None).unwrap(), any);
        assert_eq!(resolve_bind_addr("tailscale", Some(ts)).unwrap(), ts);
        let err = resolve_bind_addr("tailscale", None).unwrap_err();
        assert!(err.contains("Tailscale"), "{err}");
        // 非 tailscale 模式不消费探测结果（传了也忽略）。
        assert_eq!(resolve_bind_addr("all", Some(ts)).unwrap(), any);
        assert_eq!(resolve_bind_addr("loopback", Some(ts)).unwrap(), loopback);
    }

    /// 目录浏览契约:只列目录、藏点开头、按名排序;根列表(空参)给磁盘/根且无 parent。
    #[test]
    fn list_subdirectories_lists_sorted_dirs_and_hides_dotdirs() {
        let base = std::env::temp_dir().join(format!("meowo-lsdir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("beta")).unwrap();
        std::fs::create_dir_all(base.join("Alpha")).unwrap();
        std::fs::create_dir_all(base.join(".hidden")).unwrap();
        std::fs::write(base.join("file.txt"), b"x").unwrap();

        let l = list_subdirectories_sync(base.to_str()).unwrap();
        let names: Vec<&str> = l.dirs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta"]);
        // 子项 path 可直接作下一跳参数;parent 存在(还没到根)。
        assert!(l.dirs[0].path.ends_with("Alpha"), "{}", l.dirs[0].path);
        assert!(l.parent.is_some());

        let roots = list_subdirectories_sync(None).unwrap();
        assert!(roots.parent.is_none() && !roots.dirs.is_empty());
        assert!(list_subdirectories_sync(Some("::no-such-dir::")).is_err());

        std::fs::remove_dir_all(&base).ok();
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

    #[test]
    fn brotli_negotiation_reads_q_values() {
        assert!(accepts_brotli("gzip, deflate, br"));
        assert!(accepts_brotli("br;q=0.9, gzip"));
        assert!(accepts_brotli("BR"));
        assert!(!accepts_brotli("gzip, deflate"));
        assert!(!accepts_brotli("br;q=0"));
        assert!(!accepts_brotli("br; q=0.0, gzip"));
        assert!(!accepts_brotli("*"));
        assert!(!accepts_brotli(""));
    }

    #[test]
    fn hashed_assets_get_immutable_cache() {
        assert_eq!(
            cache_control_for("assets/ChatWindow-DafP6on7.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control_for("mobile.html"), "no-cache");
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

    /// 参数契约的机制性防护（头注 2026-08-31 反例的兜底）：每条白名单臂一份样例
    /// payload，逐键镜像 api.ts 对应 invoke 的实参对象——含可选/条件包含的键（凡可能
    /// 出现在 payload 的键样例里都有），断言能反序列化进该臂结构体，即「这一发不 400」。
    /// api.ts 加键而桥臂没跟上时，样例按 api.ts 同步更新后本测试即红。白名单新增命令
    /// 在 sample_payload_parse_check 缺样例会撞 `_` 臂失败——样例与命令同生同灭，
    /// 与 every_bridged_command_has_a_dispatch_arm 同思路。
    #[test]
    fn every_bridged_command_sample_payload_parses() {
        for cmd in BRIDGED_COMMANDS {
            if let Err(e) = sample_payload_parse_check(cmd) {
                panic!("{cmd} 样例 payload 未通过：{e}");
            }
        }
        // api.ts 多处显式发 null（非缺键，见 :568/:885/:941）——Option 字段必须吃 null。
        assert!(
            parse::<GetLiveSessionsPageArgs>(
                br#"{"filter":"all","search":null,"cwd":null,"beforeLastEventAt":null,"beforeId":null,"limit":50,"includeForeign":null,"includePending":null}"#
            )
            .is_ok()
        );
        assert!(
            parse::<NewSessionArgs>(
                br#"{"cwd":"C:/repo","provider":"claude","options":null,"extraDirs":null}"#
            )
            .is_ok()
        );
        assert!(parse::<ListSubdirectoriesArgs>(br#"{"path":null}"#).is_ok());
        assert!(parse::<AgentChatUiArgs>(br#"{"provider":"claude","cwd":null,"sessionId":null}"#).is_ok());
    }

    /// 单臂样例校验：Ok = 样例能进该臂结构体；Err = 解析失败或缺样例。
    /// 注释行号是样例键的出处（api.ts 的 invoke 调用点；host_os/file_access_token
    /// 不经 api.ts，走 mobile/transport.ts 直发）。无参臂 dispatch 不读载荷，样例 `{}`
    /// 用 IgnoredAny 只是钉死「传 {} 即可」这一事实（空 body 由 parse 归一为 {}）。
    fn sample_payload_parse_check(cmd: &str) -> Result<(), String> {
        match cmd {
            // api.ts:566（getLiveSessionsPage：filter/limit 恒有，search/cwd/游标键恒发、
            // 空值发 null，includeForeign/includePending 恒发）
            "get_live_sessions_page" => parse::<GetLiveSessionsPageArgs>(
                br#"{"filter":"running","search":"fix","cwd":"C:/repo","beforeLastEventAt":1725000000000,"beforeId":42,"limit":50,"includeForeign":false,"includePending":true}"#,
            )
            .map(|_| ()),
            // api.ts:526
            "search_chat_transcripts" => {
                parse::<SearchChatTranscriptsArgs>(br#"{"query":"login"}"#).map(|_| ())
            }
            // api.ts:946
            "recent_cwds" => parse::<RecentCwdsArgs>(br#"{"limit":8}"#).map(|_| ()),
            // api.ts:451（renameSession：cwd/provider 可 null，sessionId 是 cc_session_id 字符串）
            "rename_session" => parse::<RenameSessionArgs>(
                br#"{"cwd":"C:/repo","sessionId":"cc-abc","title":"renamed","provider":"claude"}"#,
            )
            .map(|_| ()),
            // api.ts:456
            "set_archived" => {
                parse::<SetArchivedArgs>(br#"{"sessionId":7,"archived":true}"#).map(|_| ())
            }
            // api.ts:390
            "stop_managed_terminal" => {
                parse::<StopManagedTerminalArgs>(br#"{"sessionId":7}"#).map(|_| ())
            }
            // api.ts:461
            "set_session_note" => {
                parse::<SetSessionNoteArgs>(br#"{"sessionId":"cc-abc","note":"memo"}"#).map(|_| ())
            }
            // api.ts:182（full/before 可选——undefined 键被 JSON 丢弃，给了就须在结构体内）
            "get_chat_history" => parse::<GetChatHistoryArgs>(
                br#"{"sessionId":7,"offset":0,"full":true,"before":100}"#,
            )
            .map(|_| ()),
            // api.ts:193
            "get_subagent_transcript" => {
                parse::<GetSubagentTranscriptArgs>(br#"{"sessionId":7,"toolUseId":"tu-1"}"#)
                    .map(|_| ())
            }
            // api.ts:362（since 可选）
            "managed_terminal_snapshot" => {
                parse::<ManagedTerminalSnapshotArgs>(br#"{"sessionId":7,"since":12}"#).map(|_| ())
            }
            // api.ts:368
            "write_managed_terminal" => {
                parse::<WriteManagedTerminalArgs>(br#"{"sessionId":7,"data":"ls\r"}"#).map(|_| ())
            }
            // api.ts:371
            "resize_managed_terminal" => {
                parse::<ResizeManagedTerminalArgs>(br#"{"sessionId":7,"cols":80,"rows":24}"#)
                    .map(|_| ())
            }
            // api.ts:322（options 可选）
            "start_managed_terminal" => parse::<StartManagedTerminalArgs>(
                br#"{"sessionId":7,"cols":80,"rows":24,"options":{"permissionMode":"default"}}"#,
            )
            .map(|_| ()),
            // api.ts:344（options 可选）
            "takeover_managed_terminal" => parse::<TakeoverManagedTerminalArgs>(
                br#"{"sessionId":7,"cols":80,"rows":24,"options":{"permissionMode":"default"}}"#,
            )
            .map(|_| ()),
            // api.ts:339
            "send_background_prompt" => {
                parse::<SendBackgroundPromptArgs>(br#"{"sessionId":7,"text":"go on"}"#).map(|_| ())
            }
            // api.ts:355
            "set_session_launch_selection" => parse::<SetSessionLaunchSelectionArgs>(
                br#"{"sessionId":7,"option":"model","choice":"opus"}"#,
            )
            .map(|_| ()),
            // api.ts:428
            "register_approval_consumer" => {
                parse::<RegisterApprovalConsumerArgs>(br#"{"sessionId":7,"consumerId":"c1"}"#)
                    .map(|_| ())
            }
            // api.ts:431
            "unregister_approval_consumer" => {
                parse::<UnregisterApprovalConsumerArgs>(br#"{"consumerId":"c1"}"#).map(|_| ())
            }
            // api.ts:434
            "resolve_pending_approval" => parse::<ResolvePendingApprovalArgs>(
                br#"{"sessionId":7,"requestId":"r1","choice":"allow"}"#,
            )
            .map(|_| ()),
            // api.ts:247
            "save_pasted_attachment" => {
                parse::<SavePastedAttachmentArgs>(br#"{"fileName":"a.png","dataBase64":"aGk="}"#)
                    .map(|_| ())
            }
            // api.ts:810（cwd/sessionId 可 null）
            "agent_chat_ui" => {
                parse::<AgentChatUiArgs>(br#"{"provider":"claude","cwd":"C:/repo","sessionId":7}"#)
                    .map(|_| ())
            }
            // api.ts:818
            "agent_models" => {
                parse::<AgentModelsArgs>(br#"{"provider":"claude"}"#).map(|_| ())
            }
            // api.ts:941（options 可选、extraDirs 恒发——空也发 []，2026-08-31 事故的字段）
            "new_session" => parse::<NewSessionArgs>(
                br#"{"cwd":"C:/repo","provider":"claude","options":{"permissionMode":"default"},"extraDirs":["C:/other"]}"#,
            )
            .map(|_| ()),
            // api.ts:972
            "check_provider_hooks" => {
                parse::<CheckProviderHooksArgs>(br#"{"provider":"claude"}"#).map(|_| ())
            }
            // api.ts:885（path 键恒发、无参发 null）
            "list_subdirectories" => {
                parse::<ListSubdirectoriesArgs>(br#"{"path":"C:/"}"#).map(|_| ())
            }
            // sessionId 单参臂，共用 SessionArg——出处：api.ts:967 getSessionLineage /
            // :218 refreshSessionModel / :229 refreshSessionTodos / :381 managedTerminalGrid /
            // :365 managedTerminalBinding / :331 attachBackgroundSession /
            // :349 sessionLaunchSelections / :421 pendingInteraction /
            // :425 dismissInteractiveQuestion
            "get_session_lineage"
            | "refresh_session_model"
            | "refresh_session_todos"
            | "managed_terminal_grid"
            | "managed_terminal_binding"
            | "attach_background_session"
            | "session_launch_selections"
            | "pending_interaction"
            | "dismiss_interactive_question" => {
                parse::<SessionArg>(br#"{"sessionId":7}"#).map(|_| ())
            }
            // 无参臂，dispatch 忽略载荷——出处：api.ts:537 getLiveSessionsCounts /
            // :871 awaitingInteractionSessions / :802 listAgents / :830 getSettings /
            // :926 getAccounts；platform.ts:7 host_os（transport.ts:87 直发 body "{}"）、
            // transport.ts:108 file_access_token（rpc("file_access_token", {})）
            "get_live_sessions_counts"
            | "awaiting_interaction_sessions"
            | "list_agents"
            | "get_settings"
            | "host_os"
            | "file_access_token"
            | "get_accounts" => parse::<serde::de::IgnoredAny>(br#"{}"#).map(|_| ()),
            // 白名单新增项必须补样例——落到此臂是样例漏写，不是命令坏了。
            _ => Err(format!("{cmd} 在 sample_payload_parse_check 缺 payload 样例")),
        }
    }
}
