//! Agent 一键安装 / 交互式登录 / hooks 接线与状态检测。从 lib.rs 抽出。

#[cfg(target_os = "windows")]
use crate::envpath;
use crate::settings::load_settings;
#[cfg(target_os = "windows")]
use crate::terminal::pwsh_available;
use crate::terminal::spawn_in_terminal;
use crate::{account, agent_id, db_path, install_for, ports, setup};
use std::path::PathBuf;

/// 后台安装结束事件：ok=true 表示进程 0 退出；code 为退出码（无法取得时 None）。
/// camelCase：`log_path` 须序列化成前端拿得到的 `logPath`（其余字段是单词，改名前后一致）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallDone {
    provider: String,
    ok: bool,
    code: Option<i32>,
    /// 安装脚本的完整输出落盘处（失败时供用户/我们排查）。UI 不展示英文原文，只给路径。
    /// 建不出日志文件时为 None——不因为记不了日志就让安装失败。
    log_path: Option<String>,
}

/// 安装成功、或登录成功之后，顺手把 hooks 接上——不必等下次启动的 `setup::apply_all`，
/// 也不必让用户自己去点「修复连接」。
///
/// **best-effort，绝不影响主流程**：接线失败不该让安装报失败，更不该让登录报失败。
///
/// 为什么两个时机都要接（只接一个等于没接）：`wire()` 的门槛是数据目录存在，而各家的
/// 配置文件生成时机不同——
///
/// | agent | 装完后 | 接线结果 |
/// |---|---|---|
/// | claude | `~/.claude` 未必存在（`claude.exe install` 只装 launcher） | 目录在则成功，否则 `NotDetected` |
/// | kimi | `~/.kimi-code/bin` 被建，但 `config.toml` 要 `kimi login` 才有 | 必然 `NeedLogin` |
/// | codex | `~/.codex` 未必存在 | 目录在则成功，否则 `NotDetected` |
///
/// 也就是说 kimi 只有登录后才接得上；claude/codex 装完可能还没目录，但登录必然会创建它
/// （写凭据）。故两处都调一次。
pub(crate) fn wire_hooks_best_effort(id: meowo_agent::AgentId, occasion: &str) {
    // 与启动时的 apply_all 同一道门：dev 构建不自动抢全局 hook 槽位（五个调用点
    // 收敛在此一处）。手动「修复连接」走 repair_provider_hooks→apply_provider，不经此门。
    if !setup::auto_wire_enabled() {
        eprintln!("Meowo repair[{id}]: dev 构建跳过{occasion}后自动接线");
        return;
    }
    match setup::apply_provider(id) {
        None => eprintln!("Meowo repair[{id}]: {occasion}后已自动接线"),
        Some(reason) => {
            // NeedLogin / NotDetected 都是意料之中：前端的「修复连接」按钮仍在，用户可手动重试。
            eprintln!(
                "Meowo repair[{id}]: {occasion}后自动接线未生效（{reason:?}），留给用户手动修复"
            );
        }
    }
}

use meowo_protocol::ipc::{LoginDoneEvent, LoginOutcome};

/// 每个 agent 当前这一轮登录等待的「代次」。
///
/// 递增它即可让**正在跑**的 watch 线程在下一个轮询点自行退出（它每轮比对自己出生时的代次）。
/// 用代次而不是一个布尔取消位，是因为两件事都要处理：
///
/// - **取消**：用户点「取消登录」，代次 +1，旧线程停下，且不再 emit——否则它会迟到地把
///   一个 `ok:false` 打到用户已经重新发起的那一轮上。
/// - **重发起**：用户连点两次登录，第二次的代次 +1 把第一个线程也停掉，避免两个线程
///   同时 emit（一个成功一个超时，前端状态取决于谁先到）。
///
/// 按 agent 分开：分别登录两个 agent 本就该允许并发（各自一个终端、一个 watch 线程）。
#[derive(Default)]
struct LoginSlot {
    epoch: u64,
    operation_id: Option<String>,
}

#[derive(Default)]
struct LoginOperations {
    slots: std::collections::HashMap<&'static str, LoginSlot>,
}

impl LoginOperations {
    /// 无 id 的裸递增。生产路径只剩「按 operationId 收尾」（cancel/finish/take），
    /// 它只被代次语义的测试使用，故门控到 test。
    #[cfg(test)]
    fn bump(&mut self, key: meowo_agent::AgentId) -> u64 {
        let slot = self.slots.entry(key.as_str()).or_default();
        slot.epoch += 1;
        slot.operation_id = None;
        slot.epoch
    }

    fn epoch(&self, key: meowo_agent::AgentId) -> u64 {
        self.slots.get(key.as_str()).map_or(0, |slot| slot.epoch)
    }

    fn begin(&mut self, key: meowo_agent::AgentId, operation_id: &str) -> u64 {
        let slot = self.slots.entry(key.as_str()).or_default();
        slot.epoch += 1;
        slot.operation_id = Some(operation_id.to_owned());
        slot.epoch
    }

    fn cancel(&mut self, key: meowo_agent::AgentId, operation_id: &str) -> bool {
        let Some(slot) = self.slots.get_mut(key.as_str()) else {
            return false;
        };
        if slot.operation_id.as_deref() != Some(operation_id) {
            return false;
        }
        slot.epoch += 1;
        slot.operation_id = None;
        true
    }

    /// 登出路径用：与 bump 一样递增代次让 watch 线程静默退场，但把被取消的 operationId
    /// 拿回来——前端的 pending 只靠 login-done 清除（useLoginOperations），「登录等待中点
    /// 退出登录」时必须由登出方按这个 id 补发一个 Cancelled，否则按钮永久卡在等待态。
    /// 无进行中操作 → None：不得凭空发事件。
    fn take(&mut self, key: meowo_agent::AgentId) -> Option<String> {
        let slot = self.slots.entry(key.as_str()).or_default();
        slot.epoch += 1;
        slot.operation_id.take()
    }

    fn finish(&mut self, key: meowo_agent::AgentId, epoch: u64, operation_id: &str) -> bool {
        let Some(slot) = self.slots.get_mut(key.as_str()) else {
            return false;
        };
        if slot.epoch != epoch || slot.operation_id.as_deref() != Some(operation_id) {
            return false;
        }
        slot.operation_id = None;
        true
    }
}

static LOGIN_STATE: std::sync::LazyLock<std::sync::Mutex<LoginOperations>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(LoginOperations::default()));

/// 取下一个代次（用于新起的 watch 线程），同时使该 agent 所有旧线程失效。
/// 生产路径只剩「按 operationId 收尾」（见 take_login_operation），裸 bump 只被测试使用。
#[cfg(test)]
pub(crate) fn bump_login_epoch(key: meowo_agent::AgentId) -> u64 {
    LOGIN_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .bump(key)
}

/// 当前代次。watch 线程每轮拿它与自己出生时的代次比对，不等则说明已被取消/取代。
pub(crate) fn login_epoch(key: meowo_agent::AgentId) -> u64 {
    LOGIN_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .epoch(key)
}

fn begin_login_operation(key: meowo_agent::AgentId, operation_id: &str) -> u64 {
    LOGIN_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .begin(key, operation_id)
}

fn cancel_login_operation(key: meowo_agent::AgentId, operation_id: &str) -> bool {
    LOGIN_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cancel(key, operation_id)
}

/// 登出路径用：取消该 agent 当前登录操作并拿回 operationId（无进行中操作 → None）。
/// watch 线程因代次失效静默退出，补发 login-done 的责任移交给登出方。
fn take_login_operation(key: meowo_agent::AgentId) -> Option<String> {
    LOGIN_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take(key)
}

fn finish_login_operation(key: meowo_agent::AgentId, epoch: u64, operation_id: &str) -> bool {
    LOGIN_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .finish(key, epoch, operation_id)
}

/// 取消该 agent 正在进行的登录等待。
///
/// 起因：点完登录后如果终端被关掉（用户手动关、崩溃、或 agent 自己退出），后端毫不知情——
/// 它只轮询账号文件，会一直等到 **5 分钟**超时才 emit `login-done`。这五分钟里按钮一直是
/// 「等待登录…」且不可点，用户既不能重来也不知道发生了什么。
///
/// 检测「终端是否还活着」不可行：`wt.exe` 拉起窗口后自身立即退出，真正跑 `claude auth login`
/// 的是它的孙进程；而 `powershell -NoExit` 又会一直活着。三种终端行为不一致，靠监视进程
/// 判断只会在某些终端上失灵。给用户一个**始终有效**的出口，比一个时灵时不灵的检测更好。
///
/// 不等价于「登录失败」：用户可能已经在终端里登完了。故取消后仍重查一次账号——
/// 真登上了就报成功。
#[tauri::command]
pub(crate) async fn cancel_login(
    app: tauri::AppHandle,
    provider: String,
    operation_id: String,
) -> Result<(), String> {
    let key = agent_id(&provider).ok_or("未知 agent")?;
    let provider = key.as_str().to_string();
    if !cancel_login_operation(key, &operation_id) {
        return Err("登录操作已结束或已被替换".into());
    }
    // 由本命令负责收尾 emit，让前端无论如何都能落回可点状态。
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Emitter;
        let ok = account::account_of(key).is_some(); // 也许真登上了，只是用户嫌慢
                                                     // 真登上了就和正常登录成功一样接线——否则「取消时其实已登录」这条路径会漏掉接线。
        if ok {
            wire_hooks_best_effort(key, "登录");
        }
        let _ = app.emit(
            "login-done",
            LoginDoneEvent {
                operation_id,
                provider,
                outcome: if ok {
                    LoginOutcome::Success
                } else {
                    LoginOutcome::Cancelled
                },
            },
        );
    })
    .await
    .map_err(|e| e.to_string())
}

/// 轮询账号解析结果，直到出现（登录成功）或超时，然后 emit `login-done`。
///
/// **为什么轮询 `account()` 而不是 watch 凭据文件**：macOS 上 claude 把凭据存进登录 Keychain，
/// `.credentials.json` 根本不存在，watch mtime 永远等不到。`account()` 已封装三家（含 Keychain）
/// 的判定，且全是本地读，2s 一轮开销可忽略。
///
/// 超时上限 5 分钟：登录走浏览器 OAuth，用户可能中途放弃；spawn 出去的是 detach 的终端，
/// 拿不到退出码，不设上限线程就永久泄漏。用户不想等满 5 分钟时可以 [`cancel_login`]。
/// `profile` = 正在登进哪个账号（`None` = 默认账号）。**必须按它轮询**：登录写的是那个 profile
/// 的凭据，若去查当前活跃账号，用户登完了这里也永远看不到——login-done 只会在 5 分钟后超时。
pub(crate) fn watch_login(
    app: tauri::AppHandle,
    key: meowo_agent::AgentId,
    provider: String,
    profile: Option<String>,
    operation_id: String,
    // 出生代次。由 login_agent 在 spawn 终端**之前**注册（否则冷启动的几秒里用户点取消，
    // cancel_login 查不到 operationId 只能报错，而随后注册的 watcher 又白等 5 分钟）。
    // 被 cancel_login 或下一次 login_agent 递增后，本线程静默退出、不 emit——
    // 收尾的 emit 归那一方，否则会有两个 login-done 打架。
    epoch: u64,
) {
    const POLL: std::time::Duration = std::time::Duration::from_secs(2);
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
    std::thread::spawn(move || {
        use tauri::Emitter;
        let start = std::time::Instant::now();
        // 先查再睡：账号若已就绪（用户很快登完，或本就登录着）立刻 emit，不白等一个轮询周期。
        // 每轮只是本地读一个 JSON——三家的 account() 都不联网、不 spawn 子进程（claude 读
        // ~/.claude.json；macOS 的 Keychain 只在读**凭据**/刷新 token 时才碰，不在此路径上）。
        let ok = loop {
            if login_epoch(key) != epoch {
                eprintln!("Meowo login[{provider}]: 已被取消或被新一轮登录取代，停止轮询");
                return; // 收尾 emit 归取消方 / 新线程
            }
            // 查的是**正在登录的那个账号**的凭据，不是当前活跃的。
            let logged_in = match profile.as_deref() {
                Some(p) => crate::profile::installation_of(key, p)
                    .and_then(|inst| account::account_in(key, &inst))
                    .is_some(),
                None => account::account_of(key).is_some(),
            };
            if logged_in {
                break true;
            }
            if start.elapsed() >= TIMEOUT {
                eprintln!(
                    "Meowo login[{provider}]: 等待登录超时（{}s），停止轮询",
                    TIMEOUT.as_secs()
                );
                break false;
            }
            std::thread::sleep(POLL);
        };
        // 登录成功 → 此时配置文件才刚生成（kimi 的 config.toml 由 `kimi login` 写；claude/codex
        // 的数据目录也因写凭据而必然存在），是唯一都接得上 hooks 的时机。
        //
        // 多账号下**必须接那个 profile 的线**，不是默认账号的：新建 profile 时也接过一次，但那时
        // 它还没登录——kimi 那种「配置文件由 login 生成」的 agent，当时必然以 NeedLogin 失败。
        // 这里是它唯一的补救时机；接错了对象，profile 的会话就永远不会上板。
        if ok {
            match profile.as_deref() {
                Some(p) => match crate::profile::wire_profile(key, p) {
                    None => eprintln!("Meowo repair[{key}/{p}]: 登录后已自动接线"),
                    Some(reason) => eprintln!(
                        "Meowo repair[{key}/{p}]: 登录后自动接线未生效（{reason:?}），留给用户手动修复"
                    ),
                },
                None => wire_hooks_best_effort(key, "登录"),
            }
        }
        if !finish_login_operation(key, epoch, &operation_id) {
            return;
        }
        let _ = app.emit(
            "login-done",
            LoginDoneEvent {
                operation_id,
                provider,
                outcome: if ok {
                    LoginOutcome::Success
                } else {
                    LoginOutcome::Timeout
                },
            },
        );
    });
}

/// 在终端里拉起该 agent 的交互式登录（`claude auth login` / `codex login` / `kimi login`），
/// 并起后台任务等登录完成，完成或超时后 emit `login-done`。
///
/// argv 与 `new_session` 同源，故同样是**绝对路径优先、必要时回退裸名**：spawn 出的终端继承
/// app 启动时的 PATH 快照，未必含刚装好的 agent，裸名会让 wt/powershell 报 0x80070002；但当
/// 可执行只在 PATH 上（`LaunchCandidate::OnPath`，如 fnm 管理的 codex）时回退裸名是刻意的
/// ——那类路径带版本/进程号无法静态声明，且 PATH 上的往往是 shim，固化它会绕过其环境准备。
///
/// `profile` = 显式指定登进**哪个账号**；`use_active` 为 true 时由后端解析当前活跃账号。
/// 这个区分是必要的：`profile = None, use_active = false` 明确表示默认账号，而顶部快捷入口传
/// `use_active = true`。登录会把
/// 凭据写进**那个 profile 的**目录，而这只由注入的隔离变量（`CLAUDE_CONFIG_DIR` 等）决定。漏掉它，
/// 新账号的登录就会把默认账号的凭据覆盖掉——用户以为自己加了个账号，其实是把原来那个换掉了。
#[tauri::command]
pub(crate) async fn login_agent(
    app: tauri::AppHandle,
    provider: String,
    terminal: Option<String>,
    profile: Option<String>,
    use_active: bool,
    operation_id: String,
) -> Result<(), String> {
    if operation_id.is_empty()
        || operation_id.len() > 128
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("登录操作 ID 无效".into());
    }
    let key = agent_id(&provider).ok_or("未知 agent")?;
    let provider = key.as_str().to_string(); // 归一：emit 用规范串
                                             // 登录写的是**目标 profile** 的凭据，故实况也按它取（默认账号 → agent 自己的目录）。
    let active = use_active
        .then(|| crate::profile::active_id(key.as_str()))
        .flatten();
    let profile = select_login_profile(profile, use_active, active);
    let inst = match profile.as_deref() {
        Some(p) => crate::profile::installation_of(key, p).ok_or("解析不到该账号的目录")?,
        None => install_for(key).ok_or("解析不到该 agent 的安装实况")?,
    };
    let argv = inst.login_argv().ok_or("该 agent 未声明登录入口")?;
    // 登录尤其需要代理：codex / kimi 的 device auth 在需代理的网络里会直接卡死（codex #4242、
    // kimi-cli #1234 都是这个症状），拉起一个连不上的登录终端毫无意义。
    let mut env = crate::proxy::launch_env(key);
    // 隔离变量：决定这次登录的凭据写进哪个目录。
    env.extend(crate::profile::env_of(key, profile.as_deref()));
    let term = terminal
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| load_settings().resume_terminal);

    // spawn 之前先注册操作：冷启动首次 spawn 可达数秒，这期间用户就可能点取消，
    // cancel_login 必须能按 operationId 找到它。取消发生在 spawn 期间时，epoch 已被
    // 递增，watcher 线程第一轮比对就会静默退出，不会复活已取消的操作。
    let epoch = begin_login_operation(key, &operation_id);
    // 冷启动首次 spawn 控制台子进程可达数秒；放 blocking 池不挡事件循环。
    let ok =
        tauri::async_runtime::spawn_blocking(move || spawn_in_terminal(&argv, None, &term, &env))
            .await
            .map_err(|e| e.to_string())?;
    if !ok {
        // 终端没拉起来就没有可等的登录；注销操作，别让一个查不到进展的 watcher 挂着。
        let _ = cancel_login_operation(key, &operation_id);
        return Err("启动终端失败：无法拉起登录流程".into());
    }
    watch_login(app, key, provider, profile, operation_id, epoch);
    Ok(())
}

fn select_login_profile(
    explicit: Option<String>,
    use_active: bool,
    active: Option<String>,
) -> Option<String> {
    if use_active {
        active
    } else {
        explicit
    }
}

/// 用 API Key 完成登录（声明了 `ApiKeyLoginCap` 的 agent，当前只有 gemini）。
///
/// 与 [`login_agent`] 是并列入口，不是替代：那条拉终端走交互式流程，这条同步落盘、当场生效——
/// gemini 的交互式登录只剩 OAuth 一条路，而 Google 已对个人账号关闸（*This client is no longer
/// supported for Gemini Code Assist for individuals*），点它必然失败；key 又没有对应的登录子命令，
/// 只能由宿主替用户写进 CLI 认的位置（`~/.gemini/.env` + settings 的 selectedType）。
///
/// key 是机密：不写日志、不进 Settings、不通过事件广播；具体落盘位置与权限由插件负责。
/// 成功后顺手接线，与交互式登录成功后的行为一致（settings.json 此刻必然存在）。
///
/// 不 emit `login-done`：本命令同步返回，前端拿到 Ok 即重查账号。若此前有一轮交互式登录还在
/// 轮询等待，它下一轮（≤2s）就会看到账号出现，自行以 success 收尾——两条路互不打架。
#[tauri::command]
pub(crate) async fn api_key_login(
    provider: String,
    key: String,
    profile: Option<String>,
) -> Result<(), String> {
    let id = agent_id(&provider).ok_or("未知 agent")?;
    let cap = meowo_agent::by_id(id.as_str())
        .and_then(|p| p.api_key_login())
        .ok_or("该 agent 不支持 API Key 登录")?;
    // 写进**哪个账号**：显式指定的那个，否则当前活跃账号——与 logout 同一套语义。
    let profile = match profile {
        Some(p) => Some(p),
        None => crate::profile::active_id(id.as_str()),
    };
    let inst = match profile.as_deref() {
        Some(p) => crate::profile::installation_of(id, p).ok_or("解析不到该账号的目录")?,
        None => install_for(id).ok_or("解析不到该 agent 的安装实况")?,
    };
    tauri::async_runtime::spawn_blocking(move || {
        cap.save_api_key(&inst, &key)?;
        wire_hooks_best_effort(id, "登录");
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 退出官方账号。优先调用 CLI 自带的非交互式登出命令；没有登出入口的 CLI（当前为 Kimi）
/// 只删除变体表声明的凭据文件，不碰 config、会话或 hooks。
#[tauri::command]
pub(crate) async fn logout_agent(
    app: tauri::AppHandle,
    provider: String,
    profile: Option<String>,
) -> Result<(), String> {
    let key = agent_id(&provider).ok_or("未知 agent")?;
    // 登出**哪个账号**：显式指定的那个，否则当前活跃账号。
    //
    // 这里曾经写死默认账号（`install_for`），后果是：你切到另一个账号后点「退出登录」，被清掉的
    // 却是**默认账号**的凭据——而你想登出的那个原封不动。删凭据是不可逆的，这种错尤其伤。
    let profile = match profile {
        Some(p) => Some(p),
        None => crate::profile::active_id(key.as_str()),
    };
    let inst = match profile.as_deref() {
        Some(p) => crate::profile::installation_of(key, p).ok_or("解析不到该账号的目录")?,
        None => install_for(key).ok_or("解析不到该 agent 的安装实况")?,
    };
    // 账号隔离变量。**登出命令尤其需要它**：`claude auth logout` 认的是 `CLAUDE_CONFIG_DIR`，
    // 不注入的话它会跑去清默认账号的凭据——命令执行成功、结果南辕北辙。
    let env = crate::profile::env_of(key, profile.as_deref());
    // 若登录流程还在轮询，登出必须让它停止；否则旧线程可能稍后误报登录成功。
    // 等待线程按代次失效静默退出（不 emit），但前端的 pending 只靠 login-done 清除——
    // 这里按被取消的 operationId 补发 Cancelled，否则「重新登录等待中点退出登录」后
    // 按钮永久卡在等待态。无进行中操作（None）则不发，免得塞一个对不上号的 login-done。
    if let Some(operation_id) = take_login_operation(key) {
        use tauri::Emitter;
        let _ = app.emit(
            "login-done",
            LoginDoneEvent {
                operation_id,
                provider: key.as_str().to_string(),
                outcome: LoginOutcome::Cancelled,
            },
        );
    }

    tauri::async_runtime::spawn_blocking(move || {
        if let Some(argv) = inst.logout_argv() {
            let (program, args) = argv.split_first().ok_or("登出命令为空")?;
            let mut command = std::process::Command::new(program);
            command
                .args(args)
                .envs(env)
                .stdout(std::process::Stdio::null());
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            let output = command
                .output()
                .map_err(|e| format!("启动登出命令失败：{e}"))?;
            if !output.status.success() {
                let detail = stderr_excerpt(&output.stderr);
                let suffix = if detail.is_empty() {
                    String::new()
                } else {
                    format!("：{detail}")
                };
                return Err(format!(
                    "登出命令失败，退出码：{:?}{suffix}",
                    output.status.code()
                ));
            }
        } else {
            let path = inst
                .credentials_path()
                .ok_or("该 agent 未声明登出入口或凭据位置")?;
            remove_credentials_file(&path)?;
        }

        // 支持 API Key 登录的 agent（gemini）：key 也是凭据，登出必须一并清掉——只删
        // oauth_creds.json 的话，`.env` 里的 key 会让账号立刻又显示「已登录（API Key）」。
        if let Some(cap) = meowo_agent::by_id(key.as_str()).and_then(|p| p.api_key_login()) {
            cap.clear_api_key(&inst)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn stderr_excerpt(stderr: &[u8]) -> String {
    const MAX_CHARS: usize = 500;
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    let mut excerpt: String = trimmed.chars().take(MAX_CHARS).collect();
    if trimmed.chars().count() > MAX_CHARS {
        excerpt.push('…');
    }
    excerpt
}

fn remove_credentials_file(path: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除凭据失败（{}）：{e}", path.display())),
    }
}

#[cfg(test)]
mod logout_tests {
    use super::{remove_credentials_file, select_login_profile, stderr_excerpt};

    #[test]
    fn logout_stderr_excerpt_is_trimmed_and_bounded() {
        assert_eq!(
            stderr_excerpt(b"  authentication failed\r\n"),
            "authentication failed"
        );
        let long = "x".repeat(501);
        let excerpt = stderr_excerpt(long.as_bytes());
        assert_eq!(excerpt.chars().count(), 501);
        assert!(excerpt.ends_with('…'));
    }

    #[test]
    fn credential_file_logout_is_idempotent_and_keeps_siblings() {
        let dir = std::env::temp_dir().join(format!("meowo-logout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let credentials = dir.join("credentials.json");
        let config = dir.join("config.toml");
        std::fs::write(&credentials, "secret").unwrap();
        std::fs::write(&config, "hooks = []").unwrap();

        remove_credentials_file(&credentials).unwrap();
        remove_credentials_file(&credentials).unwrap();
        assert!(!credentials.exists());
        assert!(config.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn login_target_distinguishes_active_from_explicit_default() {
        assert_eq!(
            select_login_profile(None, true, Some("work".into())).as_deref(),
            Some("work")
        );
        assert_eq!(
            select_login_profile(None, false, Some("work".into())),
            None,
            "显式默认账号不能被当前活跃 profile 替换"
        );
        assert_eq!(
            select_login_profile(Some("personal".into()), false, Some("work".into())).as_deref(),
            Some("personal")
        );
    }
}

/// 按插件给出的计划直下安装：下载 → 校验大小 → 校验 SHA-256 → 用二进制自身完成安装。
///
/// 走这条路的 agent 完全不碰引导脚本，也就完全不碰它身后的 Cloudflare。附带的好处是多了
/// 一道摘要校验——裸管道连脚本内容都不校验，更别说最终的二进制。
///
/// 任何一步失败都删掉半成品：留着一个校验不过的可执行文件，比没有更危险。
pub(crate) fn run_direct_install(
    id: meowo_agent::AgentId,
    plan: &meowo_agent::InstallPlan,
    log: Option<&mut std::fs::File>,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    /// 往日志追一行。写成自由函数而不是闭包：闭包会一直借着 `log`，后面还要把它交给子进程输出。
    fn note(log: &mut Option<&mut std::fs::File>, msg: &str) {
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{msg}");
        }
    }

    let dest = std::env::temp_dir().join(&plan.file_name);
    let mut log = log;
    note(
        &mut log,
        &format!(
            "Downloading {} {} from {}",
            plan.file_name, plan.version, plan.url
        ),
    );

    // 进度每 10% 写一行日志（UI 只显示「安装中…」，不透传细节）。250 MB 在慢网上要几分钟，
    // 没有这几行的话日志看上去就像卡死了。
    //
    // 整段包在块里：`on_progress` 可变借着 `log`，出块借用才结束，后面还要把 `log` 交给子进程输出。
    let written = {
        let mut last_decile = 0u64;
        let mut on_progress = |done: u64, total: Option<u64>| {
            let Some(t) = total.filter(|t| *t > 0) else {
                return;
            };
            let decile = done * 10 / t;
            if decile > last_decile {
                last_decile = decile;
                if let Some(f) = log.as_mut() {
                    let _ = writeln!(f, "  {}% ({done} / {t} bytes)", decile * 10);
                }
            }
        };
        // 250 MB 的二进制同样走该 agent 的代理——境内直连境外发布源常常就是卡在这一步。
        ports::HostPorts::for_agent(id)
            .as_ports()
            .http
            .download(
                &plan.url,
                &dest,
                std::time::Duration::from_secs(600),
                &mut on_progress,
            )
            .map_err(|e| {
                let _ = std::fs::remove_file(&dest);
                format!("下载失败：{e}")
            })?
    };

    let fail = |dest: &std::path::Path, msg: String| -> String {
        let _ = std::fs::remove_file(dest);
        msg
    };
    if written != plan.size {
        return Err(fail(
            &dest,
            format!("下载不完整：期望 {} 字节，实得 {written}", plan.size),
        ));
    }

    // 摘要在流式写完后重读一遍算——250 MB 的文件不进内存，Sha256 也是增量喂。
    let mut f = std::fs::File::open(&dest).map_err(|e| fail(&dest, e.to_string()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut f, &mut hasher).map_err(|e| fail(&dest, e.to_string()))?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != plan.sha256 {
        return Err(fail(
            &dest,
            format!(
                "校验和不匹配（期望 {}，实得 {actual}），已删除下载的文件",
                plan.sha256
            ),
        ));
    }
    note(&mut log, "Checksum OK");

    // unix 上得先加执行位，否则下一步 spawn 直接 EACCES。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&dest)
            .map_err(|e| fail(&dest, e.to_string()))?
            .permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&dest, perm).map_err(|e| fail(&dest, e.to_string()))?;
    }

    // 由二进制自己装 launcher 与 shell 集成（claude 是 `claude.exe install`）。
    if !plan.post_install_args.is_empty() {
        note(
            &mut log,
            &format!(
                "Running: {} {}",
                plan.file_name,
                plan.post_install_args.join(" ")
            ),
        );
        let mut cmd = std::process::Command::new(&dest);
        cmd.args(&plan.post_install_args);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd
            .output()
            .map_err(|e| fail(&dest, format!("执行安装失败：{e}")))?;
        if let Some(f) = log.as_mut() {
            let _ = f.write_all(&out.stdout);
            let _ = f.write_all(&out.stderr);
        }
        if !out.status.success() {
            return Err(fail(
                &dest,
                format!("安装程序以退出码 {} 结束", out.status.code().unwrap_or(-1)),
            ));
        }
    }

    // 装完即删临时二进制（官方脚本也这么做）。删不掉不算失败。
    let _ = std::fs::remove_file(&dest);
    note(&mut log, "Installation complete");
    Ok(())
}

/// 取回官方安装引导脚本，并确认它**确实是脚本**。
///
/// 这一步是新加的，起因是一份真实的安装日志：
///
/// ```text
/// Installing, please wait...
/// Installation failed: Just a moment...*{box-sizing:border-box…
/// ```
///
/// `claude.ai` 与 `chatgpt.com` 都在 Cloudflare 后面，其人机校验页以 **HTTP 200** 返回。原先的
/// `irm <url> | iex` / `curl -fsSL <url> | bash` 不会因此报错——`irm` 拿到 HTML 就交给 PowerShell
/// 执行，`curl -f` 也只挡非 2xx。于是用户对着一屏 CSS 发懵。
///
/// 现在 shell 只跑本地文件，联网与判定都在这里。
pub(crate) fn resolve_install_body(
    id: meowo_agent::AgentId,
    script: &meowo_agent::InstallScript,
) -> Result<String, String> {
    use meowo_agent::{Body, HttpRequest};
    // Command 变体（npm 等）：命令体就是要跑的内容，不联网、无需 challenge 判定（不经 CF）。
    let url = match script {
        meowo_agent::InstallScript::Command { body, .. } => return Ok(body.to_string()),
        meowo_agent::InstallScript::Fetch { url, .. } => *url,
    };
    let ports = ports::HostPorts::for_agent(id);
    let body = ports
        .as_ports()
        .http
        .send(&HttpRequest {
            method: "GET",
            url,
            headers: &[],
            body: Body::Empty,
            timeout: std::time::Duration::from_secs(30),
        })
        .map_err(|e| format!("下载安装脚本失败：{e}"))?;

    if meowo_agent::looks_like_challenge(&body) {
        return Err(format!(
            "{url} 返回了 Cloudflare 人机校验页，而不是安装脚本。这是间歇性的，稍后重试通常即可；\
             若持续失败，请在浏览器里打开该地址手动安装。"
        ));
    }
    if !meowo_agent::is_runnable_script(&body) {
        return Err(format!("{url} 返回的不是安装脚本（可能被中间设备拦截）。"));
    }
    Ok(body)
}

/// 把取回的脚本原样写进临时文件（按 provider 命名，允许并行安装互不覆盖），返回其路径。
/// **不再包一层 shell**：脚本本身已含错误处理，包装只会让「哪一行失败」更难看清。
///
/// 路径带随机后缀并用 `create_new`（O_CREAT|O_EXCL 语义，不跟随符号链接）独占创建：
/// 固定路径 + 覆盖写是经典 TOCTOU——共享 /tmp 上另一账户可预先放同名符号链接，
/// 写入被重定向到任意文件；或在写与执行的间隙换掉内容，让我们以当前用户身份执行它。
/// 抢占防护由独占创建保证（已存在即失败换名重试），随机后缀只负责避免碰撞。
pub(crate) fn write_install_script(
    provider: &str,
    body: &str,
    windows: bool,
) -> std::io::Result<String> {
    use std::io::Write;
    let ext = if windows { "ps1" } else { "sh" };
    for _ in 0..16 {
        let path = std::env::temp_dir().join(format!(
            "meowo-install-{provider}-{:016x}.{ext}",
            script_nonce()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // Unix：0600——脚本按路径交给解释器读，无需执行位；共享 tmp 下也不给
        // 其他账户在「写入与执行的间隙」读改的机会。
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(body.as_bytes())?;
                return Ok(path.to_string_lossy().into_owned());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "无法独占创建安装脚本临时文件",
    ))
}

/// 临时脚本名的随机后缀。不为此引入 rand：防抢占靠 `create_new` 的独占语义，
/// 后缀只需避免自撞——纳秒时钟 ⊕ 进程号 ⊕ 打散的递增计数足够。
fn script_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() ^ u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    clock
        ^ (u64::from(std::process::id()) << 32)
        ^ SEQ
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// 构造后台安装子进程（不弹窗口）。平台差异只在此：Windows 用 pwsh(优先)/powershell +
/// CREATE_NO_WINDOW；其它平台用该 agent 声明的解释器（claude/kimi 要 bash，codex 官方写的是 sh）。
/// stdin/stdout/stderr 由调用方统一设。
#[cfg(target_os = "windows")]
pub(crate) fn build_install_command(script_path: &str, _unix_shell: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let shell = if pwsh_available() {
        "pwsh"
    } else {
        "powershell"
    };
    let mut c = std::process::Command::new(shell);
    // Bypass 仍需要：跑的是刚下载的未签名官方脚本。但它已经过 is_runnable_script 判定，
    // 且来自变体表里硬编码的 https 地址，不是用户输入。
    c.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script_path,
    ])
    .creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn build_install_command(script_path: &str, unix_shell: &str) -> std::process::Command {
    let mut c = std::process::Command::new(unix_shell);
    c.arg(script_path);
    c
}

/// 一键安装某 agent：后台跑其官方安装脚本（不弹终端窗口），装完 emit install-done、
/// 前端重查检测转「已装」。安装命令是受信硬编码串（Agent::install_script），非用户输入。
#[tauri::command]
pub(crate) async fn install_agent(app: tauri::AppHandle, provider: String) -> Result<(), String> {
    let agent = meowo_agent::resolve(Some(&provider)).ok_or("未知 agent")?;
    let id = agent.id(); // Copy；两条安装路径的收尾线程都要用它接线
    let provider = id.as_str().to_string(); // 归一：文件名/emit 全用规范串，消除路径注入面+大小写不一致
    let windows = cfg!(target_os = "windows");

    // 优先直下：绕开引导脚本，也就绕开它身后的 Cloudflare。plan() 只做两次小请求（版本号 + 清单），
    // 失败（发布物 schema 变了 / 下载服务本地区不可用）就回退到引导脚本，不让用户卡死在这条路上。
    if let Some(cap) = agent.direct_install() {
        let planned = tauri::async_runtime::spawn_blocking(move || {
            let p = ports::HostPorts::for_agent(id);
            cap.plan(&p.as_ports())
        })
        .await
        .map_err(|e| e.to_string())?;
        match planned {
            Ok(plan) => {
                let log_path = install_log_path(&provider);
                tauri::async_runtime::spawn_blocking(move || {
                    use tauri::Emitter;
                    let mut log = log_path
                        .as_ref()
                        .and_then(|p| std::fs::File::create(p).ok());
                    let logged = log.is_some();
                    let res = run_direct_install(id, &plan, log.as_mut());
                    if let (Some(f), Err(e)) = (log.as_mut(), res.as_ref()) {
                        use std::io::Write;
                        let _ = writeln!(f, "Installation failed: {e}");
                    }
                    let log_path = logged
                        .then(|| log_path.map(|p| p.to_string_lossy().into_owned()))
                        .flatten();
                    let ok = res.is_ok();
                    // 装好了就顺手接线；失败（多半是数据目录还没建）不影响安装结果。
                    if ok {
                        wire_hooks_best_effort(id, "安装");
                    }
                    let _ = app.emit(
                        "install-done",
                        InstallDone {
                            provider,
                            ok,
                            code: Some(if ok { 0 } else { 1 }),
                            log_path,
                        },
                    );
                });
                return Ok(());
            }
            Err(e) => {
                eprintln!("Meowo install[{provider}]: 直下不可用（{e}），回退官方引导脚本");
            }
        }
    }

    let script = agent
        .install_script(windows)
        .ok_or("该 agent 没有可用的一键安装命令")?;

    let unix_shell = script.unix_shell();
    let script_url = script.source();
    // 取脚本 + 判定放在 spawn 之前：被 Cloudflare 拦时直接回传一句人话，而不是把校验页写进日志、
    // 再让子进程去执行它。Command 变体（npm）在此直接返回命令体，不联网。放 blocking 池是因为
    // ureq 是同步的，不能堵住 tauri 的事件循环。
    let body = tauri::async_runtime::spawn_blocking(move || resolve_install_body(id, &script))
        .await
        .map_err(|e| e.to_string())??;
    let path = write_install_script(&provider, &body, windows).map_err(|e| e.to_string())?;

    // 安装输出落盘：此前 stdout/stderr 直接丢进 Stdio::null()，判成功只看退出码，于是「装是装上了
    // 但没接好」的半成功一律报成功。claude 就是这样——`claude.exe install` 在 Windows 上打印一行
    // 「请自己把 ~/.local/bin 加进 PATH」然后 exit 0，那行警告被丢掉，用户直到手敲 claude 才发现。
    // 重定向到文件而非读管道：既不必起读线程，也没有管道写满把子进程卡死的风险。
    // UI 仍不展示英文原文（只给路径），保持进度文案随界面语言。
    let log_path = install_log_path(&provider);

    // spawn 放 blocking 线程：GUI 进程首次 spawn 子进程可能被杀软扫描拖慢，勿堵事件循环。
    // spawn 成功即返回 Ok；结果走 install-done 事件；spawn 失败回传 Err，前端立即显示错误。
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use std::io::Write;
        use std::process::Stdio;
        // 日志建不出来（磁盘满/权限）不该让安装失败：退回丢弃输出，log_path 报 None。
        let mut log = log_path
            .as_ref()
            .and_then(|p| std::fs::File::create(p).ok());
        // 抬头行由我们写：脚本不再被包一层 shell，这行原先是包装打印的。它是用户在日志里
        // 判断「有没有跑起来」的唯一信号，别随包装一起丢掉。
        if let Some(f) = log.as_mut() {
            let _ = writeln!(
                f,
                "Installing {provider} from {url}, please wait...",
                url = script_url
            );
        }
        let log = log;
        let (out, err) = match log
            .as_ref()
            .and_then(|f| Some((f.try_clone().ok()?, f.try_clone().ok()?)))
        {
            Some((a, b)) => (Stdio::from(a), Stdio::from(b)),
            None => (Stdio::null(), Stdio::null()),
        };
        let logged = log.is_some();
        // 官方安装脚本内部用 curl / irm 去下 250MB 的二进制——那是**脚本自己**发的请求，不经
        // ports.rs 的 ureq 客户端，拿不到我们解析的代理。它是我们的直接子进程，故 .envs() 有效
        // （wt / wezterm / Terminal.app 那几条路就不行，见 terminal::spawn_in_terminal）。
        // 不设这个，境内用户在直下失败回退到脚本路径时，仍会卡在下载上。
        let proxy_env = crate::proxy::launch_env_for_install(id);
        let mut command = build_install_command(&path, unix_shell);
        // `.envs()` 只会覆盖同名键，不能消掉 ALL_PROXY / 小写变体；off 必须真直连。
        for key in crate::terminal::PROXY_ENV_KEYS {
            command.env_remove(key);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .env("CODEX_NON_INTERACTIVE", "1")
            .envs(proxy_env)
            .spawn()
            .map_err(|e| format!("启动安装失败：{e}"))?;
        // 等退出 + emit done 放独立线程，让 spawn_blocking 尽快归还线程池。
        std::thread::spawn(move || {
            use tauri::Emitter;
            let code = child.wait().ok().and_then(|s| s.code());
            let log_path = logged
                .then(|| log_path.map(|p| p.to_string_lossy().into_owned()))
                .flatten();
            // 装好了就顺手接线；失败（多半是数据目录还没建）不影响安装结果。
            if code == Some(0) {
                wire_hooks_best_effort(id, "安装");
            }
            let _ = app.emit(
                "install-done",
                InstallDone {
                    provider,
                    ok: code == Some(0),
                    code,
                    log_path,
                },
            );
        });
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 安装日志落点：与 board.db 同目录（`~/.meowo`）。provider 已由调用方经 ProviderKey 归一，
/// 不含路径分隔符。父目录建不出来则返回 None（安装本身照跑，只是没日志）。
pub(crate) fn install_log_path(provider: &str) -> Option<PathBuf> {
    let dir = db_path().parent()?.to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("install-{provider}.log")))
}

/// 该 agent 可执行**所在目录**——仅当 launch argv 首元素是绝对路径时才有意义。
///
/// 命中 `LaunchCandidate::OnPath`（裸名）说明它本就在 PATH 上；node 脚本形态的 argv[0] 是 `node`，
/// 同理。两种情况都不需要提示用户改 PATH，故返回 None。
#[cfg(target_os = "windows")]
pub(crate) fn agent_bin_dir(key: meowo_agent::AgentId) -> Option<PathBuf> {
    let inst = install_for(key)?;
    let argv = inst.launch_argv();
    let exe = std::path::Path::new(argv.first()?);
    if !exe.is_absolute() {
        return None;
    }
    exe.parent().map(|d| d.to_path_buf())
}

/// 该 agent 装好了、但它的 bin 目录不在**持久** PATH 上 → 返回该目录，供前端提示「加入 PATH」。
/// 无需处理（已在 PATH / 未安装 / 非 Windows）时返回 None。
///
/// 不看进程 PATH：那是 app 启动时的快照，装完之后必然假阴性（详见 envpath 模块文档）。
#[tauri::command]
pub(crate) async fn agent_path_gap(provider: String) -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        // async + spawn_blocking：解析安装形态与查注册表 PATH 都是 IO，不进主线程。
        tauri::async_runtime::spawn_blocking(move || {
            // 未知 agent → None：不去猜它的可执行装在哪，更不该据此劝用户改 PATH。
            let dir = agent_bin_dir(agent_id(&provider)?)?;
            let dir = dir.to_string_lossy().into_owned();
            (!envpath::dir_on_persistent_path(&dir)).then_some(dir)
        })
        .await
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        // unix 上 PATH 由 shell profile 决定，改法因 shell 而异，不代用户动。
        let _ = provider;
        Ok(None)
    }
}

/// 把该 agent 的 bin 目录写进**用户级** PATH（幂等）。
///
/// 只收 provider、不收路径：目录由后端从 `Installation` 推导，杜绝前端把任意路径写进 PATH。
#[tauri::command]
pub(crate) async fn add_agent_to_user_path(provider: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let key = agent_id(&provider).ok_or("未知 agent")?;
        let dir = agent_bin_dir(key).ok_or("未找到该 agent 的可执行目录")?;
        // async + spawn_blocking：写完注册表要 SendMessageTimeoutW(HWND_BROADCAST) 广播
        // WM_SETTINGCHANGE，桌面上任何一个响应慢的顶层窗口都能让它等接近 5s——同步命令
        // 会拿这段等待冻住主线程消息泵。
        tauri::async_runtime::spawn_blocking(move || {
            envpath::add_dir_to_user_path(&dir.to_string_lossy())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = provider;
        Err("当前平台不支持".into())
    }
}

/// provider 的 meowo-reporter hooks 接入状态（供「新建会话」面板引导）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HooksStatus {
    Installed,
    Missing,
    Unknown,
}

/// hooks 接入三态判定。纯路径 + 格式规格版，便于用临时文件单测（不碰真实数据目录）。
///
/// 文件不存在=Missing；读失败**或解析失败**=Unknown；能解析但 SessionStart 没挂当前
/// meowo-reporter=Missing；挂了=Installed。
///
/// 「解析失败 → Unknown」是核心不变量：配置暂时不可读/损坏时若误报 Missing，前端会催用户点
/// 「修复连接」，而修复必然因 `Abandon(ConfigUnreadable)` 拒写（绝不写坏用户文件），陷入死循环。
pub(crate) fn hooks_status_at(
    path: &std::path::Path,
    hooks: &meowo_agent::config::HookSpec,
    agent_id: &str,
) -> HooksStatus {
    if !path.exists() {
        return HooksStatus::Missing;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return HooksStatus::Unknown;
    };
    if !hooks.parses(&text) {
        return HooksStatus::Unknown;
    }
    if hooks.has_reporter(&text, agent_id) {
        HooksStatus::Installed
    } else {
        HooksStatus::Missing
    }
}

/// 某 agent 的 hooks 接入状态：读实况变体的配置文件，判定交给该变体的格式适配器。
/// 解析不出实况（无 home 等）=Unknown。
pub(crate) fn plugin_hooks_status(
    inst: Option<meowo_agent::Installation>,
    agent_id: &str,
) -> HooksStatus {
    let Some(inst) = inst else {
        return HooksStatus::Unknown;
    };
    hooks_status_at(&inst.config_path(), inst.hooks, agent_id)
}

/// 检测某 provider 的 meowo-reporter hooks 是否已接入（新建会话面板据此提示是否会入库）。
/// 未知 provider → Unknown（不冒名默认 agent 去查它的 hooks）。
#[tauri::command]
pub(crate) async fn check_provider_hooks(provider: String) -> Result<HooksStatus, String> {
    // async + spawn_blocking：要解析安装形态并读 agent 配置文件，不进主线程。
    tauri::async_runtime::spawn_blocking(move || check_provider_hooks_blocking(&provider))
        .await
        .map_err(|e| e.to_string())
}

fn check_provider_hooks_blocking(provider: &str) -> HooksStatus {
    let Some(id) = agent_id(provider) else {
        return HooksStatus::Unknown;
    };
    plugin_hooks_status(install_for(id), id.as_str())
}

/// 「修复连接」结果：最新接线状态 + 失败原因（None = 成功/已是目标状态）。
/// reason 供前端给出精准提示（如 kimi 未登录 → 「请先登录」）而非泛化文案。
#[derive(Debug, serde::Serialize)]
pub(crate) struct RepairResult {
    status: HooksStatus,
    reason: Option<setup::RepairReason>,
}

/// 手动修复某 provider 的 hooks：立即执行一次 setup::apply_provider，然后返回最新状态与失败原因。
/// 用于「新建会话」面板或设置里的「修复连接」按钮，无需重启 Meowo。
#[tauri::command]
pub(crate) async fn repair_provider_hooks(provider: String) -> Result<RepairResult, String> {
    // async + spawn_blocking：apply_provider 要读改写多个 agent 配置文件，是这批点击触发
    // 命令里最重的一个，绝不能占主线程。
    tauri::async_runtime::spawn_blocking(move || {
        let Some(id) = agent_id(&provider) else {
            eprintln!("Meowo repair[{provider}]: 未知 agent，跳过");
            return RepairResult {
                status: HooksStatus::Unknown,
                reason: Some(setup::RepairReason::NotDetected),
            };
        };
        eprintln!("Meowo repair[{provider}]: 开始修复接线…");
        let reason = setup::apply_provider(id);
        let status = check_provider_hooks_blocking(&provider);
        eprintln!("Meowo repair[{provider}]: reason={reason:?} → 状态={status:?}");
        RepairResult { status, reason }
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod login_operation_tests {
    use super::LoginOperations;

    #[test]
    fn a_new_operation_invalidates_the_previous_completion() {
        let mut state = LoginOperations::default();
        let agent = meowo_agent::id::CLAUDE;
        let first_epoch = state.begin(agent, "first");
        let second_epoch = state.begin(agent, "second");

        assert!(second_epoch > first_epoch);
        assert!(!state.finish(agent, first_epoch, "first"));
        assert!(state.finish(agent, second_epoch, "second"));
        assert!(!state.finish(agent, second_epoch, "second"));
    }

    #[test]
    fn cancel_only_matches_the_current_operation_and_is_agent_scoped() {
        let mut state = LoginOperations::default();
        let claude = meowo_agent::id::CLAUDE;
        let codex = meowo_agent::id::CODEX;
        let codex_epoch = state.begin(codex, "codex-op");
        state.begin(claude, "claude-op");

        assert!(!state.cancel(claude, "stale-op"));
        assert!(state.cancel(claude, "claude-op"));
        assert!(!state.cancel(claude, "claude-op"));
        assert!(state.finish(codex, codex_epoch, "codex-op"));
    }

    #[test]
    fn take_returns_the_cancelled_operation_and_invalidates_its_watcher() {
        let mut state = LoginOperations::default();
        let claude = meowo_agent::id::CLAUDE;
        let epoch = state.begin(claude, "op-1");

        // 登出取走 operationId：等待线程的 finish 必然失败（代次已前进、id 已被取走），
        // 它静默退场、不 emit——补发 login-done 的责任移交给登出方，两边不会各发一个。
        assert_eq!(state.take(claude).as_deref(), Some("op-1"));
        assert!(!state.finish(claude, epoch, "op-1"));
        // 没有进行中操作时再取 → None：登出方不得凭空发 login-done。
        assert_eq!(state.take(claude), None);
    }
}
