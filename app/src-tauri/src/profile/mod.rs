//! 多账号（profile）的宿主侧：目录、存储、接线、环境变量注入。
//!
//! 隔离模型与「为什么不是轮换凭据」见 [`meowo_agent::profile`]。这里只管把它落地：
//!
//! - **目录**：`~/.meowo/profiles/<agent>/<id>/`。agent 的整个数据目录被搬到这里面。
//! - **默认账号是隐式的**：它不在 `settings.profiles` 里，指向 agent 自己的目录（`~/.claude`），
//!   且**不注入任何环境变量**。于是没建过 profile 的用户零感知——这是整个功能的安全底线。
//! - **接线**：新建 profile 时给它的数据目录挂一遍 hooks（复用 `wire_hooks`，只是换个 data_dir）。
//! - **注入**：拉起 agent（新建会话 / 恢复会话 / 登录）时，把该 profile 的环境变量塞进终端。

use std::path::PathBuf;

use meowo_agent::WiringContext;

/// 一个自定义 profile。默认账号**不在**这个列表里。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Profile {
    /// 目录名，同时也是 id。由 [`slug`] 从展示名派生，只含 `[a-z0-9-]`。
    pub(crate) id: String,
    /// 展示名（用户填的，可以是中文）。
    pub(crate) name: String,
}

/// 所有 profile 的根：`~/.meowo/profiles`。
fn profiles_root() -> PathBuf {
    crate::db_path().with_file_name("profiles")
}

/// 某个 profile 的私有根目录：`~/.meowo/profiles/<agent>/<id>`。
pub(crate) fn profile_root(agent: &str, id: &str) -> PathBuf {
    profiles_root().join(agent).join(id)
}

/// 某个账号实际使用的数据目录。`id = None` 明确表示 agent 的默认账号，不表示当前活跃账号。
pub(crate) fn data_dir(agent: &str, id: Option<&str>) -> Option<PathBuf> {
    let plugin = meowo_agent::by_id(agent)?;
    match id {
        Some(id) => plugin
            .installation_for_profile(&profile_root(agent, id))
            .map(|installation| installation.data_dir),
        None => plugin
            .default_installation()
            .map(|installation| installation.data_dir),
    }
}

/// 展示名 → 目录名。**这不是美化，是安全边界**：id 会被直接当成目录名拼进路径，若原样使用用户
/// 输入，一个 `../..` 就能让我们在用户的文件系统上乱建目录、甚至让接线写到别处去。
///
/// 只保留 ASCII 字母数字与 `-`/`_`，其余（含中文、斜杠、点）一律折成 `-`；全被折掉则回退 `profile`。
pub(crate) fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // 折叠连续的 '-'，并去掉首尾的。
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        "profile".to_string()
    } else {
        s
    }
}

/// 在已有 id 中取一个不冲突的：`work` → `work-2` → `work-3`…
fn unique_id(existing: &[Profile], base: &str) -> String {
    if !existing.iter().any(|p| p.id == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|cand| !existing.iter().any(|p| &p.id == cand))
        .unwrap_or_else(|| base.to_string())
}

/// 当前活跃 profile 的 id（None = 默认账号）。
pub(crate) fn active_id(agent: &str) -> Option<String> {
    let s = crate::settings::load_settings();
    active_id_in(&s, agent)
}

/// profile 仍在设置列表中且私有根目录存在。会话恢复前用它拦住“账号已删除、DB 里的旧会话仍
/// 留着 profile id”的情况，避免 agent 在一个空目录里重建出设置页看不见的幽灵账号。
pub(crate) fn exists(agent: &str, id: &str) -> bool {
    let settings = crate::settings::load_settings();
    profile_is_registered_in(&settings, agent, id) && profile_root(agent, id).is_dir()
}

fn profile_is_registered_in(settings: &crate::settings::Settings, agent: &str, id: &str) -> bool {
    settings
        .profiles
        .get(agent)
        .is_some_and(|profiles| profiles.iter().any(|profile| profile.id == id))
}

/// 同上，但从给定的 settings 里取（避免重复读盘）。
fn active_id_in(s: &crate::settings::Settings, agent: &str) -> Option<String> {
    let id = s.active_profile.get(agent)?;
    // 活跃 id 指向一个已被删除的 profile → 视作默认账号，绝不拿着一个不存在的目录去拉起 agent。
    s.profiles
        .get(agent)?
        .iter()
        .any(|p| &p.id == id)
        .then(|| id.clone())
}

/// 当前活跃 profile 的**展示名**（None = 默认账号，前端什么都不显示——没建过 profile 的用户
/// 零感知）。从给定的 settings 里取；active_id_in 已保证 id 在列表中，故能直接取到 name。
pub(crate) fn active_display_name_in(s: &crate::settings::Settings, agent: &str) -> Option<String> {
    let id = active_id_in(s, agent)?;
    let name = s
        .profiles
        .get(agent)?
        .iter()
        .find(|p| p.id == id)?
        .name
        .clone();
    Some(name)
}

/// 会话属于哪个账号——由 meowo 拉起 agent 时注入，reporter 作为 agent 的 hook 子进程会继承它，
/// 据此把会话绑到该 profile 上（`sessions.profile`）。恢复会话时才能回到**同一个**账号。
///
/// 用户自己在终端里敲 `claude`（不经 meowo）时没有这个变量 → 会话记成默认账号，正确。
pub(crate) const PROFILE_ENV: &str = "MEOWO_PROFILE";

/// 拉起该 agent 时要注入的 profile 环境变量。**默认账号 → 空**（什么都不注入）。
///
/// 这是 profile 生效的**唯一**途径：新建会话、恢复会话、拉起登录，三条路径都必须带上它，
/// 漏一条就会静默用错账号——而且不会有任何报错，用户只会发现自己莫名其妙在用另一个身份。
pub(crate) fn env_of(agent: meowo_agent::AgentId, id: Option<&str>) -> Vec<(String, String)> {
    let Some(id) = id else { return Vec::new() };
    let Some(plugin) = meowo_agent::by_id(agent.as_str()) else {
        return Vec::new();
    };
    let Some(inst) = plugin.installation_for_profile(&profile_root(agent.as_str(), id)) else {
        // 该 agent 不支持多账号（gemini）→ 一个变量都不注入。绝不注入半套：
        // 只给 MEOWO_PROFILE 而不给隔离变量，会把一个跑在**默认账号**上的会话记成 profile 的。
        return Vec::new();
    };
    let mut env = inst.profile_env();
    env.push((PROFILE_ENV.to_string(), id.to_string()));
    env
}

/// 某 profile 的安装实况（读它的登录态、给它接线都用它）。`None` = 默认账号或该 agent 不支持多账号。
pub(crate) fn installation_of(
    agent: meowo_agent::AgentId,
    id: &str,
) -> Option<meowo_agent::Installation> {
    meowo_agent::by_id(agent.as_str())?.installation_for_profile(&profile_root(agent.as_str(), id))
}

/// 给某个 profile 的数据目录挂上 hooks。
///
/// 与默认账号的接线走的是同一条 `wire_hooks`——只是 `data_dir` 换成了 profile 的。这也是为什么
/// profile 的会话能和默认账号的会话一样上板：reporter 那一侧根本不知道 profile 的存在。
pub(crate) fn wire_profile(
    agent: meowo_agent::AgentId,
    id: &str,
) -> Option<meowo_agent::RepairReason> {
    let plugin = meowo_agent::by_id(agent.as_str())?;
    let inst = installation_of(agent, id)?;
    let dir = crate::setup::meowo_dir();
    let reporter = crate::setup::sibling_reporter();
    let ctx = WiringContext {
        fallback_reporter: reporter.as_deref(),
        meowo_dir: &dir,
    };
    meowo_agent::wire_hooks(&inst, agent.as_str(), plugin.wiring(), &ctx)
}

// ═══ Tauri 命令 ═══

/// 前端看到的一个账号。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ProfileView {
    /// `None` = **默认账号**（agent 自己的目录）。它永远排在第一个，且不可删除。
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) active: bool,
    /// 该账号的登录态（读它自己的凭据）。None = 未登录。
    pub(crate) account: Option<meowo_agent::Account>,
}

/// 某 agent 的账号列表：默认账号 + 自定义 profile，每个都带自己的登录态。
///
/// 该 agent 不支持多账号（gemini）→ **只返回默认账号一条**，前端据此不给「添加账号」入口。
#[tauri::command]
pub(crate) async fn list_profiles(provider: String) -> Vec<ProfileView> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(plugin) = meowo_agent::by_id(&provider) else {
            return Vec::new();
        };
        let id = plugin.id();
        let s = crate::settings::load_settings();
        let active = active_id_in(&s, &provider);

        // 默认账号：读 agent 自己目录下的登录态。
        // 套餐名只合并给**活跃**的那一行——用量缓存不按 profile 分键，它讲的是活跃账号的事。
        let default_active = active.is_none();
        let mut out = vec![ProfileView {
            id: None,
            // 用户起过名就用它；没起过留空，由前端本地化成「默认账号」——后端不塞译文。
            name: s
                .default_profile_names
                .get(&provider)
                .cloned()
                .unwrap_or_default(),
            active: default_active,
            account: {
                let a = plugin
                    .resolve()
                    .and_then(|inst| crate::account::account_in(id, &inst));
                if default_active {
                    crate::account::with_cached_plan(id, a)
                } else {
                    a
                }
            },
        }];

        // 不支持多账号的 agent 到此为止——绝不列出无从生效的 profile。
        if plugin.profile().is_none() {
            return out;
        }

        for p in s.profiles.get(&provider).into_iter().flatten() {
            let is_active = active.as_deref() == Some(p.id.as_str());
            let account =
                installation_of(id, &p.id).and_then(|inst| crate::account::account_in(id, &inst));
            out.push(ProfileView {
                id: Some(p.id.clone()),
                name: p.name.clone(),
                active: is_active,
                account: if is_active {
                    crate::account::with_cached_plan(id, account)
                } else {
                    account
                },
            });
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// 结构化错误（S-9）：`profile/<code>: <detail>`。invoke 的错误通道只有 String，reason 码
/// 与前端 `i18n/errors.ts` 的映射表一一对应（改这里必须同步那边），英文界面按当前语言出文案。
/// 旧前端不认识这些码时会原样显示——英文短语，不会像硬编码中文那样在英文界面漏中文。
/// 已被旧映射覆盖的用户向 sentinel（「没有这个账号」「账号名不能为空」等）保持原样不动：
/// 换成码反而让旧前端的中文界面退化。
fn coded(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// 新建一个账号：建目录 → 接线 hooks → 存进 settings。返回它的 id。
///
/// **不自动切过去**，也不自动登录：切换与登录是用户的两个独立动作，替他决定只会让人困惑。
#[tauri::command]
pub(crate) async fn create_profile(provider: String, name: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plugin = meowo_agent::by_id(&provider).ok_or("未知 agent")?;
        let agent = plugin.id();
        let spec = plugin.profile().ok_or("该 agent 不支持多账号")?;

        crate::settings::update_settings(|s| {
            let existing = s.profiles.entry(provider.clone()).or_default();
            let id = unique_id(existing, &slug(&name));
            let root = profile_root(&provider, &id);
            for dir in spec.dirs(&root) {
                std::fs::create_dir_all(&dir).map_err(|e| coded("profile/create-dir-failed", e))?;
            }
            if let Some(reason) = wire_profile(agent, &id) {
                eprintln!(
                    "Meowo profile[{provider}/{id}]: 接线未完成（{reason:?}），可稍后手动修复"
                );
            }
            let display = name.trim();
            existing.push(Profile {
                id: id.clone(),
                name: if display.is_empty() {
                    id.clone()
                } else {
                    display.to_string()
                },
            });
            Ok(id)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 给账号改名。`id = None` → **默认账号**（它的名字单独存在 `default_profile_names` 里）。
///
/// **只动展示名，不动 id** —— id 是目录名，改了就等于换了个账号（凭据、会话历史全在那个目录里），
/// 而用户以为自己只是改了个称呼。
#[tauri::command]
pub(crate) async fn rename_profile(
    provider: String,
    id: Option<String>,
    name: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = name.trim();
        if name.is_empty() {
            return Err("账号名不能为空".to_string());
        }
        crate::settings::update_settings(|s| {
            match id {
                // 默认账号：它不在 profiles 里（是隐式的），名字单独存。
                None => {
                    s.default_profile_names
                        .insert(provider.clone(), name.to_string());
                }
                Some(id) => {
                    let p = s
                        .profiles
                        .get_mut(&provider)
                        .and_then(|list| list.iter_mut().find(|p| p.id == id))
                        .ok_or("没有这个账号")?;
                    p.name = name.to_string();
                }
            }
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 切换活跃账号。`id = None` → 切回默认账号。
///
/// 只影响**此后**拉起的会话：已经在跑的会话早已继承了它启动时的环境变量，不会中途改换账号。
#[tauri::command]
pub(crate) async fn set_active_profile(provider: String, id: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::settings::update_settings(|s| {
            match id {
                None => {
                    s.active_profile.remove(&provider);
                }
                Some(id) => {
                    let known = s
                        .profiles
                        .get(&provider)
                        .is_some_and(|v| v.iter().any(|p| p.id == id));
                    if !known {
                        return Err("没有这个账号".to_string());
                    }
                    s.active_profile.insert(provider.clone(), id);
                }
            }
            Ok(())
        })?;
        // 用量缓存是按 agent 存的，换了账号它就过期了——留着会让新账号顶着旧账号的额度。
        if let Some(agent) = meowo_agent::by_id(&provider) {
            crate::account::clear_cached_usage(agent.id());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 删除一个账号：**连同它的整个目录一起删**（凭据、配置、该账号的会话历史）。
///
/// 这是不可逆的，前端必须先确认。删的是 `~/.meowo/profiles/<agent>/<id>`——**只可能**是我们
/// 自己建的目录，绝不会碰到 agent 本体的 `~/.claude`：默认账号没有 id，压根走不到这里。
#[tauri::command]
pub(crate) async fn delete_profile(provider: String, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (removed, was_active) = crate::settings::update_settings(|s| {
            let list = s.profiles.get_mut(&provider).ok_or("没有这个账号")?;
            let pos = list.iter().position(|p| p.id == id).ok_or("没有这个账号")?;
            let removed = list.remove(pos);
            let was_active = s.active_profile.get(&provider).is_some_and(|a| a == &id);
            if was_active {
                s.active_profile.remove(&provider);
            }
            Ok((removed, was_active))
        })?;

        let root = profile_root(&provider, &id);
        if root.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&root) {
                // Settings 已保存但目录没删掉时恢复入口，让用户能够关闭占用进程后重试。
                let restore = crate::settings::update_settings(|s| {
                    let list = s.profiles.entry(provider.clone()).or_default();
                    if !list.iter().any(|p| p.id == removed.id) {
                        list.push(removed.clone());
                    }
                    if was_active {
                        s.active_profile.insert(provider.clone(), id.clone());
                    }
                    Ok(())
                });
                return Err(match restore {
                    Ok(()) => coded("profile/delete-dir-failed", e),
                    Err(r) => coded("profile/delete-dir-failed-unrestored", format!("{e}; {r}")),
                });
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 把一个账号**合并进默认账号**：数据目录递归并入、会话改挂默认账号、账号从设置移除。
///
/// 与删除账号的根本区别是**数据全留下**。安全约束（顺序不能换）：
/// 1. 文件合并用**不覆盖**语义——目标已存在的一律跳过，默认账号的凭据/配置天然不会被碰；
///    `history.jsonl` 特判为按行追加去重。任何一步失败整体中止，settings/DB 都不动。
/// 2. 文件并入完成后才把会话改挂默认账号、从 settings 移除 profile。
/// 3. 最后 best-effort 删 profile 目录——数据在默认目录已有副本，删不掉只是残留，无害。
#[tauri::command]
pub(crate) async fn merge_profile_into_default(provider: String, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || merge_into_default(&provider, &id))
        .await
        .map_err(|e| e.to_string())?
}

/// 合并的同步实现（命令只是它的 spawn_blocking 包装；集成测试直接调它）。
pub fn merge_into_default(provider: &str, id: &str) -> Result<(), String> {
    let plugin = meowo_agent::by_id(provider).ok_or("未知 agent")?;
    // profile 必须在册且目录还在（同 exists 的口径：防「设置里有、目录没了」的半态）。
    if !exists(provider, id) {
        return Err("没有这个账号".to_string());
    }
    // 进行中的会话 reporter 仍会把 profile id 写回 sessions.profile——此刻合并会让它们
    // 变成设置里查不到的幽灵账号，先请用户结束这些会话。
    let store = crate::open_store(&crate::db_path())?;
    let live = store
        .profile_live_session_count(id)
        .map_err(|e| e.to_string())?;
    if live > 0 {
        return Err(coded("profile/has-live-sessions", live));
    }

    let src = data_dir(provider, Some(id)).ok_or("找不到该账号的数据目录")?;
    let dst = data_dir(provider, None).ok_or("找不到默认账号的数据目录")?;
    // 文件合并失败 → 整体中止：settings/DB 尚未动，重试不会有半态。
    merge_dir_no_overwrite(&src, &dst)?;

    // 数据已并入，再把会话归属并过去。
    store
        .rehome_profile_sessions(id)
        .map_err(|e| e.to_string())?;
    crate::settings::update_settings(|s| {
        let list = s.profiles.get_mut(provider).ok_or("没有这个账号")?;
        let pos = list.iter().position(|p| p.id == id).ok_or("没有这个账号")?;
        list.remove(pos);
        if s.active_profile.get(provider).is_some_and(|a| a == id) {
            s.active_profile.remove(provider);
        }
        Ok(())
    })?;
    // 用量缓存按 agent 分键、不按 profile：账号集合变了，那份额度的归属就说不清了
    // （与 set_active_profile 清缓存同理由）。
    crate::account::clear_cached_usage(plugin.id());

    // best-effort 删 profile 目录：merge_dir_no_overwrite 已保证会话日志无损并入
    // （前缀取超集、真分叉存 .bak 侧车），此刻删源目录不会销毁任何唯一副本。
    let root = profile_root(provider, id);
    if root.is_dir() {
        if let Err(e) = std::fs::remove_dir_all(&root) {
            eprintln!(
                "Meowo profile[{provider}/{id}]: 合并后删除账号目录失败（数据已并入默认账号，残留目录无害）：{e}"
            );
        }
    }
    Ok(())
}

/// 递归把 `src` 并入 `dst`，**不覆盖**：目标已存在的文件一律跳过。
/// 两类例外（都是追加式日志，跳过等于丢数据）：
/// - `history.jsonl`：目标已存在时按行追加去重（两边的会话历史都保住）；
/// - 其余 `*.jsonl`（transcript 等会话日志）：走 [`merge_jsonl_no_loss`] 的无损消解。
///   跨 profile 恢复会在默认目录留下同名**陈旧**副本（`sync_claude_session_files` 的
///   拷贝产物），若按「已存在即跳过」处理，唯一含有后续消息的新副本就永远并不过来，
///   随后的删目录把它销毁——对话历史被不可逆地截断到陈旧副本。
fn merge_dir_no_overwrite(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    // 目标根目录可能压根不存在：`data_dir(provider, None)` 只算路径、不建目录，一个从没在
    // 默认账号下跑过该 agent 的用户点「合并进默认账号」时，~/.claude 就是不存在的。
    // 下面只为**子**目录建目录，成不成于是取决于 read_dir 先吐出目录还是文件——先目录就
    // 顺带建出来了，先文件就 fs::copy 报「系统找不到指定的路径」。这一行把它定死。
    std::fs::create_dir_all(dst).map_err(|e| coded("profile/create-dir-failed", e))?;
    for entry in std::fs::read_dir(src).map_err(|e| coded("profile/read-dir-failed", e))? {
        let entry = entry.map_err(|e| coded("profile/read-dir-failed", e))?;
        let name = entry.file_name();
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            if !to.exists() {
                std::fs::create_dir_all(&to).map_err(|e| coded("profile/create-dir-failed", e))?;
            }
            merge_dir_no_overwrite(&from, &to)?;
        } else if name == "history.jsonl" && to.is_file() {
            merge_history_jsonl(&from, &to)?;
        } else if to.is_file() && name.to_string_lossy().ends_with(".jsonl") {
            merge_jsonl_no_loss(&from, &to)?;
        } else if !to.exists() {
            // 目标没有才复制——默认账号的凭据/配置绝不会被覆盖。
            std::fs::copy(&from, &to)
                .map_err(|e| coded("profile/copy-failed", format!("{}: {e}", from.display())))?;
        }
    }
    Ok(())
}

/// 追加式日志（`*.jsonl`）同名冲突的**无损**消解：
/// - 一方是另一方的前缀（同一会话的不同进度快照）→ 目标保留/换成超集，零丢失；
/// - 真分叉（两边各自长出了内容）→ 目标不动，源侧完整存为 `.merge-conflict.bak`
///   侧车。合并后删除 profile 目录不再可能销毁唯一副本；`.bak` 不以 `.jsonl` 结尾，
///   不会被 claude 的 transcript 扫描（按 `<sid>.jsonl` 命名）误认。
fn merge_jsonl_no_loss(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    let read = |p: &std::path::Path| {
        std::fs::read(p).map_err(|e| coded("profile/read-file-failed", format!("{}: {e}", p.display())))
    };
    let src = read(from)?;
    let dst = read(to)?;
    if dst.len() >= src.len() && dst.starts_with(&src) {
        return Ok(()); // 目标已是超集（或相同），源无新内容。
    }
    if src.starts_with(&dst) {
        // 源是目标的续写（目标停在陈旧副本）：用超集覆盖——旧内容一字节不少。
        std::fs::copy(from, to)
            .map_err(|e| coded("profile/copy-failed", format!("{}: {e}", from.display())))?;
        return Ok(());
    }
    // 真分叉：哪边都不能丢。默认侧优先保位，源侧存侧车（重名时递增序号，不覆盖旧侧车）。
    let name = to
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut bak = to.with_file_name(format!("{name}.merge-conflict.bak"));
    let mut n = 1;
    while bak.exists() {
        n += 1;
        bak = to.with_file_name(format!("{name}.merge-conflict-{n}.bak"));
    }
    std::fs::copy(from, &bak)
        .map_err(|e| coded("profile/copy-failed", format!("{}: {e}", from.display())))?;
    Ok(())
}

/// history.jsonl 特判：把源文件里**目标还没有的行**追加到目标末尾（按整行去重，保住两边历史）。
fn merge_history_jsonl(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    let existing =
        std::fs::read_to_string(dst).map_err(|e| coded("profile/read-file-failed", format!("history.jsonl: {e}")))?;
    // mut + insert 判重：源文件**自己**也可能有重复行（两个 profile 从同一份历史分叉出来，
    // 各自又追加过同样的命令）。只对着目标查的话，那些行会被原样追加两遍。
    let mut known: std::collections::HashSet<&str> = existing.lines().collect();
    let incoming =
        std::fs::read_to_string(src).map_err(|e| coded("profile/read-file-failed", format!("history.jsonl: {e}")))?;
    let mut append = String::new();
    // 目标末尾没有换行时先补一个，否则追加的第一行会黏在既有末行上。
    if !existing.is_empty() && !existing.ends_with('\n') {
        append.push('\n');
    }
    for line in incoming.lines() {
        // insert 返回 false = 这行已经见过（目标里有，或本次已追加过一次）。
        if line.is_empty() || !known.insert(line) {
            continue;
        }
        append.push_str(line);
        append.push('\n');
    }
    if append.is_empty() {
        return Ok(());
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(dst)
        .map_err(|e| coded("profile/write-file-failed", format!("history.jsonl: {e}")))?;
    file.write_all(append.as_bytes())
        .map_err(|e| coded("profile/write-file-failed", format!("history.jsonl: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// id 会被拼进文件系统路径——`../` 这类输入必须在这里就被折平，否则接线会写到用户目录之外。
    #[test]
    fn slug_never_escapes_the_profile_root() {
        assert_eq!(slug("work"), "work");
        assert_eq!(slug("Work Account"), "work-account");
        assert_eq!(slug("  My  Account "), "my-account");
        assert_eq!(slug("keep_underscore"), "keep_underscore");

        // 路径穿越必须被折平。
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("a/../b"), "a-b");
        assert_eq!(slug("..\\..\\x"), "x");
        assert_eq!(slug("."), "profile");
        assert_eq!(slug("../.."), "profile");

        // 全是非 ASCII（中文名很常见）→ 折没了，回退到一个安全的常量名。
        assert_eq!(slug("工作账号"), "profile");
        assert_eq!(slug(""), "profile");

        // 兜底断言：任何输入产出的 id 都不含路径分隔符与 '.'。
        for bad in ["../x", "a/b", "a\\b", "..", "a.b", "  ", "🙂"] {
            let s = slug(bad);
            assert!(
                !s.contains('/') && !s.contains('\\') && !s.contains('.'),
                "{bad} → {s}"
            );
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn unique_id_avoids_collisions() {
        let existing = vec![
            Profile {
                id: "work".into(),
                name: "工作".into(),
            },
            Profile {
                id: "work-2".into(),
                name: "工作2".into(),
            },
        ];
        assert_eq!(unique_id(&existing, "personal"), "personal");
        assert_eq!(unique_id(&existing, "work"), "work-3");
        assert_eq!(unique_id(&[], "work"), "work");
    }

    /// 活跃 id 指向一个**已被删除**的 profile → 必须退回默认账号。
    /// 否则我们会拿着一个不存在的目录去拉起 agent：它会在那儿凭空建一个空目录，
    /// 用户莫名其妙地进入一个未登录的账号，而 meowo 还以为一切正常。
    #[test]
    fn stale_active_id_falls_back_to_default() {
        let mut s = crate::settings::Settings::default();
        s.profiles.insert(
            "claude".into(),
            vec![Profile {
                id: "work".into(),
                name: "工作".into(),
            }],
        );

        s.active_profile.insert("claude".into(), "work".into());
        assert_eq!(active_id_in(&s, "claude").as_deref(), Some("work"));

        // 指向已删除的 profile。
        s.active_profile.insert("claude".into(), "gone".into());
        assert_eq!(active_id_in(&s, "claude"), None);

        // 压根没有 profile 的 agent。
        assert_eq!(active_id_in(&s, "codex"), None);
    }

    #[test]
    fn profile_registration_rejects_deleted_and_cross_agent_ids() {
        let mut settings = crate::settings::Settings::default();
        settings.profiles.insert(
            "claude".into(),
            vec![Profile {
                id: "work".into(),
                name: "Work".into(),
            }],
        );
        assert!(profile_is_registered_in(&settings, "claude", "work"));
        assert!(!profile_is_registered_in(&settings, "claude", "deleted"));
        assert!(!profile_is_registered_in(&settings, "codex", "work"));
    }

    /// 默认账号**不注入任何环境变量**——这是「没建 profile 的用户零感知」的全部依据。
    #[test]
    fn default_profile_injects_nothing() {
        assert!(env_of(meowo_agent::id::CLAUDE, None).is_empty());
    }

    /// profile 的环境变量指向它自己的根目录，外加 `MEOWO_PROFILE`（reporter 继承它，会话据此
    /// 绑定到该账号）。opencode 必须拿到**两个**目录变量，只隔离配置目录的话，凭据仍然共用——
    /// 账号看起来切了、其实没切。
    #[test]
    fn profile_env_points_into_its_own_root() {
        let env = env_of(meowo_agent::id::CLAUDE, Some("work"));
        assert_eq!(env[0].0, "CLAUDE_CONFIG_DIR");
        assert_eq!(PathBuf::from(&env[0].1), profile_root("claude", "work"));
        assert_eq!(
            env.iter()
                .find(|(k, _)| k == PROFILE_ENV)
                .map(|(_, v)| v.as_str()),
            Some("work")
        );
        assert_eq!(env.len(), 2, "多注入了变量？实得 {env:?}");

        let env = env_of(meowo_agent::id::OPENCODE, Some("work"));
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"OPENCODE_CONFIG_DIR"));
        assert!(
            keys.contains(&"XDG_DATA_HOME"),
            "凭据所在的数据目录也必须隔离"
        );

        // gemini 不支持多账号（数据目录不可覆盖）→ 无论传什么 id 都不注入。
        assert!(env_of(meowo_agent::id::GEMINI, Some("work")).is_empty());
    }

    /// 活跃 profile 的展示名：命中给展示名；默认账号（无活跃 id）→ None，前端什么都不显示。
    #[test]
    fn active_display_name_only_for_custom_profiles() {
        let mut s = crate::settings::Settings::default();
        assert_eq!(active_display_name_in(&s, "claude"), None);

        s.profiles.insert(
            "claude".into(),
            vec![Profile {
                id: "work".into(),
                name: "工作".into(),
            }],
        );
        s.active_profile.insert("claude".into(), "work".into());
        assert_eq!(
            active_display_name_in(&s, "claude").as_deref(),
            Some("工作")
        );

        // 活跃 id 指向已删除的 profile → 视作默认账号 → None（与 active_id_in 同口径）。
        s.active_profile.insert("claude".into(), "gone".into());
        assert_eq!(active_display_name_in(&s, "claude"), None);
    }

    fn merge_tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meowo-merge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 合并的铁律：**目标已存在的文件绝不覆盖**——默认账号的凭据（.credentials.json）、
    /// 配置天然安全；只有目标没有的文件才从 profile 并过来（含子目录递归）。
    #[test]
    fn merge_never_overwrites_existing_files() {
        let root = merge_tmp("no-overwrite");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("projects/p1")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join(".credentials.json"), "profile-creds").unwrap();
        std::fs::write(src.join("projects/p1/a.jsonl"), "profile-session").unwrap();
        std::fs::write(dst.join(".credentials.json"), "default-creds").unwrap();
        std::fs::write(dst.join("settings.json"), "default-settings").unwrap();

        merge_dir_no_overwrite(&src, &dst).unwrap();

        // 默认账号的凭据原封不动。
        assert_eq!(
            std::fs::read_to_string(dst.join(".credentials.json")).unwrap(),
            "default-creds"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("settings.json")).unwrap(),
            "default-settings"
        );
        // 目标没有的文件（含子目录里的）并过来了。
        assert_eq!(
            std::fs::read_to_string(dst.join("projects/p1/a.jsonl")).unwrap(),
            "profile-session"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// history.jsonl 是两边的会话历史，不能按「不覆盖」丢掉 profile 那边：按行追加去重。
    #[test]
    fn merge_appends_history_lines_deduped() {
        let root = merge_tmp("history");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        // 目标末尾故意不留换行：追加时必须先补，否则会黏到既有末行上。
        std::fs::write(dst.join("history.jsonl"), "{\"a\":1}\n{\"b\":2}").unwrap();
        std::fs::write(src.join("history.jsonl"), "{\"b\":2}\n{\"c\":3}\n").unwrap();

        merge_dir_no_overwrite(&src, &dst).unwrap();

        let merged = std::fs::read_to_string(dst.join("history.jsonl")).unwrap();
        assert_eq!(merged, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n");

        // 目标没有 history.jsonl 时按普通文件复制。
        std::fs::remove_file(dst.join("history.jsonl")).unwrap();
        merge_dir_no_overwrite(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("history.jsonl")).unwrap(),
            "{\"b\":2}\n{\"c\":3}\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 跨 profile 恢复的典型现场：默认目录里躺着**陈旧**的同名 transcript（sync 的拷贝产物），
    /// profile 侧才是含后续消息的续写。「已存在即跳过」会让删目录销毁唯一新副本——
    /// 前缀关系必须取超集覆盖。
    #[test]
    fn merge_overwrites_stale_transcript_with_its_continuation() {
        let root = merge_tmp("jsonl-prefix");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("projects/p1")).unwrap();
        std::fs::create_dir_all(dst.join("projects/p1")).unwrap();
        std::fs::write(dst.join("projects/p1/sid.jsonl"), "{\"m\":1}\n").unwrap();
        std::fs::write(src.join("projects/p1/sid.jsonl"), "{\"m\":1}\n{\"m\":2}\n").unwrap();
        // 反向前缀（目标已是超集）：目标不动，也不产生侧车。
        std::fs::write(dst.join("projects/p1/done.jsonl"), "{\"m\":1}\n{\"m\":2}\n").unwrap();
        std::fs::write(src.join("projects/p1/done.jsonl"), "{\"m\":1}\n").unwrap();

        merge_dir_no_overwrite(&src, &dst).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.join("projects/p1/sid.jsonl")).unwrap(),
            "{\"m\":1}\n{\"m\":2}\n",
            "陈旧副本应被续写超集覆盖"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("projects/p1/done.jsonl")).unwrap(),
            "{\"m\":1}\n{\"m\":2}\n",
            "目标已是超集时不得回退"
        );
        assert!(
            !dst.join("projects/p1/done.jsonl.merge-conflict.bak")
                .exists(),
            "无损消解不应产生侧车"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真分叉（两边各自长出内容）：默认侧保位，源侧完整存 .bak 侧车——删 profile 目录
    /// 后两份内容都还在,且 .bak 不以 .jsonl 结尾,不会被 transcript 扫描误认。
    #[test]
    fn merge_preserves_divergent_transcript_as_sidecar() {
        let root = merge_tmp("jsonl-divergent");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("sid.jsonl"), "{\"m\":1}\n{\"d\":1}\n").unwrap();
        std::fs::write(src.join("sid.jsonl"), "{\"m\":1}\n{\"s\":1}\n").unwrap();

        merge_dir_no_overwrite(&src, &dst).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.join("sid.jsonl")).unwrap(),
            "{\"m\":1}\n{\"d\":1}\n",
            "分叉时默认侧不动"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("sid.jsonl.merge-conflict.bak")).unwrap(),
            "{\"m\":1}\n{\"s\":1}\n",
            "源侧必须完整保为侧车"
        );

        // 再并一次（侧车已占位）：不覆盖旧侧车,递增序号。
        merge_dir_no_overwrite(&src, &dst).unwrap();
        assert!(dst.join("sid.jsonl.merge-conflict-2.bak").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 从没在默认账号下跑过该 agent 时，目标目录压根不存在。此前只为**子**目录建目录，
    /// 于是能不能合并取决于 read_dir 先吐出目录还是文件——先文件就 fs::copy 报
    /// 「系统找不到指定的路径」。非确定性地坏掉比稳定坏掉更难查。
    #[test]
    fn merging_into_a_default_account_that_was_never_used_creates_it() {
        let root = std::env::temp_dir().join(format!("meowo-merge-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        // 只放文件、不放子目录：正是「先吐出文件」那条分支。
        std::fs::write(src.join(".credentials.json"), "{}").unwrap();
        let dst = root.join("never-ran");
        assert!(!dst.exists());

        merge_dir_no_overwrite(&src, &dst).expect("目标目录不存在也该能合并");
        assert_eq!(
            std::fs::read_to_string(dst.join(".credentials.json")).unwrap(),
            "{}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
