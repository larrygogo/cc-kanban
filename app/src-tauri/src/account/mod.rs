//! 账号/用量的**编排层**：缓存、限频、注入端口。
//!
//! 各 agent 的凭据格式、OAuth 刷新、用量 API schema 全部住在 `meowo_agent::plugins::<id>::account`，
//! 由 `AgentPlugin::account()` 能力槽取用；本模块不认识任何具体 agent。
//!
//! 缓存与 60s 限频**刻意留在这里**：它们是「meowo 怎么管用量」的策略，不是「某个 agent 怎么查用量」
//! 的知识。此前三家各自在 `usage()` 里重复了同一套读缓存/判鲜/写回逻辑。

use meowo_agent::{AccountCap, AgentId, ProviderUsage};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use meowo_agent::UsageKind;
pub use meowo_agent::{Account, USAGE_UNSUPPORTED};

/// 用量缓存的新鲜期：期内的 force 刷新直接吃缓存，不打 API。
const USAGE_FRESH_MS: i64 = 60_000;

// ═══ 共享 I/O 工具 ═══

fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(PathBuf::from)
}

fn usage_cache_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MEOWO_DB") {
        return Some(PathBuf::from(p).with_file_name("usage-cache.json"));
    }
    home_dir().map(|h| h.join(".meowo").join("usage-cache.json"))
}

fn read_json(path: &std::path::Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ═══ get_accounts 的返回载荷 ═══

/// get_accounts 命令返回的单条 agent 载荷。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderAccountPayload {
    pub provider: String,
    pub account: Option<Account>,
    pub usage: Option<ProviderUsage>,
    pub usage_supported: bool,
    pub relay_enabled: bool,
    /// 当前活跃 profile 的展示名；None = 默认账号（前端不显示任何账号提示——
    /// 没建过 profile 的用户零感知，与 profile 功能的安全底线一致）。
    pub active_profile_name: Option<String>,
}

// ═══ 能力取用 ═══

/// 该 agent 的账号能力（未注册 / 无账号概念 → None）。
pub fn account_cap(id: AgentId) -> Option<&'static dyn AccountCap> {
    meowo_agent::by_id(id.as_str())?.account()
}

/// 该 agent **当前活跃 profile** 的安装实况；没建 profile / 该 agent 不支持多账号 → 默认账号
/// （agent 自己的目录）。
///
/// 账号与用量一律按活跃 profile 读——切了账号，卡片上显示的就该是那个账号的邮箱与额度。
pub fn active_installation(id: AgentId) -> Option<meowo_agent::Installation> {
    let profile = crate::profile::active_id(id.as_str());
    installation_for_profile(id, profile.as_deref())
}

fn installation_for_profile(
    id: AgentId,
    profile: Option<&str>,
) -> Option<meowo_agent::Installation> {
    match profile {
        Some(profile) => crate::profile::installation_of(id, profile),
        None => meowo_agent::by_id(id.as_str())?.resolve(),
    }
}

/// 读**指定实况**的账号信息（只读本地，不联网）。profile 列表逐个读登录态时用它。
pub fn account_in(id: AgentId, inst: &meowo_agent::Installation) -> Option<Account> {
    let p = crate::ports::HostPorts::for_agent(id);
    account_cap(id)?.account(inst, &p.as_ports())
}

/// 读当前活跃账号的信息（只读本地，不联网）。
pub fn account_of(id: AgentId) -> Option<Account> {
    account_in(id, &active_installation(id)?)
}

/// 把用量缓存里捎回来的套餐名合并进账号（[`ProviderUsage::plan`]）。
///
/// kimi 的会员等级只出现在 `/usages` 的响应里——凭据、JWT、本地文件里一概没有（实测）。而
/// [`AccountCap::account`] 是纯本地、登录轮询会 2s 跑一次的，不能为它联网。于是让用量刷新把等级
/// 顺带捎回来，落进缓存，展示时在这里合并。
///
/// **只对活跃账号成立**：缓存按 agent 分键、不按 profile（切账号时 `clear_cached_usage` 会丢弃它），
/// 所以那份套餐名讲的必然是当前活跃的那个账号。把它贴到别的 profile 上，就是把甲的会员等级挂到
/// 乙头上。
///
/// 账号侧自己读得出套餐的（claude 的 `userRateLimitTier`、codex 的 `chatgpt_plan_type`）不覆盖——
/// 那些更直接也更新鲜。
pub fn with_cached_plan(id: AgentId, account: Option<Account>) -> Option<Account> {
    let mut a = account?;
    if a.plan.is_none() {
        a.plan = read_cached_usage(id).and_then(|u| u.plan);
    }
    Some(a)
}

/// 活跃账号 + 缓存里的套餐名。展示用（`get_accounts` / 账号列表的活跃行）。
pub fn account_of_display(id: AgentId) -> Option<Account> {
    with_cached_plan(id, account_of(id))
}

/// 该 agent 当前是否支持用量查询。
pub fn usage_supported(id: AgentId) -> bool {
    // 走中转时用量无从谈起（额度归中转的密钥所有者）。
    if crate::settings::load_settings().relay.enabled(id) {
        return false;
    }
    let Some(inst) = active_installation(id) else {
        return false;
    };
    let p = crate::ports::HostPorts::for_agent(id);
    account_cap(id).is_some_and(|c| c.usage_supported(&inst, &p.as_ports()))
}

/// 取用量。
///
/// - `force == false`：只读缓存，绝不联网。
/// - `force == true`：缓存仍新鲜（< 60s）则复用，否则调 agent 的 `fetch_usage` 拉一次并写回。
///
/// 这段策略此前被三个 agent 各抄了一份；现在它只存在于这里，插件只负责「去 API 拿」。
pub fn usage_of(id: AgentId, force: bool) -> Option<ProviderUsage> {
    if crate::settings::load_settings().relay.enabled(id) {
        return None;
    }
    if !force {
        return read_cached_usage(id);
    }
    if cache_is_fresh(id, USAGE_FRESH_MS) {
        if let Some(cached) = read_cached_usage(id) {
            return Some(cached);
        }
    }
    // 端口按 agent 绑定：代理是 per-agent 的（claude 走代理、kimi 直连是常态）。
    let p = crate::ports::HostPorts::for_agent(id);
    // 固定本次请求对应的 profile。不能在取实况和完成请求时分别读“当前账号”：用户可能在
    // 网络请求途中切换账号，旧请求随后回写就会让新账号顶着旧账号的额度。
    let requested_profile = crate::profile::active_id(id.as_str());
    let inst = installation_for_profile(id, requested_profile.as_deref())?;
    match account_cap(id)?.fetch_usage(&inst, &p.as_ports()) {
        Ok(u) => {
            if !write_cached_usage_for_profile(id, &requested_profile, &u) {
                eprintln!("Meowo usage[{id}]: 刷新期间账号已切换，丢弃旧账号结果");
                return None;
            }
            Some(u)
        }
        Err(e) => {
            eprintln!("Meowo usage[{id}]: {e}");
            None
        }
    }
}

/// 所有注册了账号能力的 agent（供 get_accounts 遍历）。
pub fn all_with_account() -> impl Iterator<Item = &'static dyn meowo_agent::AgentPlugin> {
    meowo_agent::all()
        .iter()
        .copied()
        .filter(|p| p.account().is_some())
}

// ═══ 用量缓存（按 agent 分键） ═══
//
// 当前格式：
//   {"providers": {"claude": ProviderUsage, ...}, "fetched_at_map": {"claude": ms, ...}, ...}
// 旧扁平格式（仅容错读取，仅 claude；写入方已全部移除，保留一个版本周期后可删）：
//   {"usage": Usage, "fetched_at": ms}

/// 读某 agent 的缓存用量。先试新格式 `providers.<id>`，再对 claude 兼容旧扁平 usage 字段。
pub fn read_cached_usage(id: AgentId) -> Option<ProviderUsage> {
    let v = read_json(&usage_cache_path()?)?;
    if let Some(providers) = v.get("providers") {
        if let Some(raw) = providers.get(id.as_str()) {
            if let Ok(pu) = serde_json::from_value::<ProviderUsage>(raw.clone()) {
                return Some(pu);
            }
        }
    }
    // 旧扁平格式容错（仅 claude）。
    if id == meowo_agent::id::CLAUDE {
        if let Some(old_raw) = v.get("usage") {
            let old: meowo_agent::plugins::claude::account::Usage =
                serde_json::from_value(old_raw.clone()).ok()?;
            return Some(meowo_agent::plugins::claude::account::map_to_provider_usage(&old));
        }
    }
    None
}

/// write_cached_usage 的「读-合并-写」临界区锁：贴纸挂载/定时刷新会对多个 agent 同 tick 并发
/// 刷新（各自 spawn_blocking），不串行化时后写者会用旧快照覆盖先写者刚落盘的条目（丢失更新）。
static USAGE_CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 丢弃某 agent 的用量缓存。
///
/// **切换账号时必须调**：缓存是按 agent 分键的，不按 profile——换了账号，那份额度就属于别人了，
/// 留着会让新账号顶着旧账号的用量条，直到下一次刷新才纠正。
pub fn clear_cached_usage(id: AgentId) {
    let Some(p) = usage_cache_path() else { return };
    let _guard = USAGE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut root) = read_json(&p) else {
        return;
    };
    let Some(obj) = root.as_object_mut() else {
        return;
    };
    for key in ["providers", "fetched_at_map"] {
        if let Some(m) = obj.get_mut(key).and_then(|v| v.as_object_mut()) {
            m.remove(id.as_str());
        }
    }
    if let Ok(body) = serde_json::to_string_pretty(&root) {
        let _ = crate::fsutil::write_atomic(&p, &body);
    }
}

/// 把某 agent 的用量写入缓存（providers 分键合并写入，唯一写入方）。
/// 仅当当前活跃 profile 仍等于请求发起时的快照才写缓存。检查与写入共用缓存锁：切换账号
/// 会在更新 settings 后调用 `clear_cached_usage` 并取得同一把锁，因此不存在“检查通过、切换清空、
/// 随后旧请求又写回”的窗口。
fn write_cached_usage_for_profile(
    id: AgentId,
    requested_profile: &Option<String>,
    usage: &ProviderUsage,
) -> bool {
    let _guard = USAGE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !same_profile(requested_profile, &crate::profile::active_id(id.as_str())) {
        return false;
    }
    write_cached_usage_unlocked(id, usage);
    true
}

fn same_profile(requested: &Option<String>, current: &Option<String>) -> bool {
    requested == current
}

fn write_cached_usage_unlocked(id: AgentId, usage: &ProviderUsage) {
    let Some(p) = usage_cache_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 读现有文件合并：只更新本 agent 条目，其它 agent 的缓存原样保留。
    let mut root = read_json(&p).unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = root.as_object_mut() else {
        return;
    };

    let providers = obj
        .entry("providers")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(pm) = providers.as_object_mut() {
        if let Ok(val) = serde_json::to_value(usage) {
            pm.insert(id.as_str().to_string(), val);
        }
    }
    // fetched_at_map（每个 agent 独立时间戳）
    let fat = obj
        .entry("fetched_at_map")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(fm) = fat.as_object_mut() {
        fm.insert(id.as_str().to_string(), serde_json::json!(now_ms()));
    }
    // 原子写：读端（get_accounts/cache_is_fresh）裸读本文件，直写可能被读到半截而解析失败、
    // 整份缓存瞬时作废。
    if let Ok(s) = serde_json::to_string(&root) {
        let _ = meowo_agent::fsutil::write_atomic(&p, &s);
    }
}

/// 某 agent 的缓存是否在 fresh_ms 内。支持新 fetched_at_map 与旧 fetched_at（仅 claude）。
pub fn cache_is_fresh(id: AgentId, fresh_ms: i64) -> bool {
    let Some(v) = usage_cache_path().and_then(|p| read_json(&p)) else {
        return false;
    };
    if let Some(t) = v
        .get("fetched_at_map")
        .and_then(|m| m.get(id.as_str()))
        .and_then(|x| x.as_i64())
    {
        return now_ms() - t < fresh_ms;
    }
    // 旧扁平格式（仅 claude）
    if id == meowo_agent::id::CLAUDE {
        if let Some(t) = v.get("fetched_at").and_then(|x| x.as_i64()) {
            return now_ms() - t < fresh_ms;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_kind_as_str_roundtrip() {
        // as_str→from_str 全量 roundtrip：确保每个 variant 序列化后能正确还原。
        for &k in UsageKind::ALL {
            assert_eq!(
                UsageKind::from_str(k.as_str()),
                k,
                "UsageKind::{k:?} roundtrip 失败"
            );
        }
    }

    #[test]
    fn usage_kind_unknown_maps_to_other() {
        assert_eq!(UsageKind::from_str("nonexistent_kind"), UsageKind::Other);
        assert_eq!(UsageKind::from_str(""), UsageKind::Other);
        assert_eq!(UsageKind::from_str("FIVE_HOUR"), UsageKind::Other); // 大小写不同 → Other
    }

    #[test]
    fn usage_result_only_belongs_to_the_profile_that_started_the_request() {
        assert!(same_profile(&Some("work".into()), &Some("work".into())));
        assert!(same_profile(&None, &None));
        assert!(!same_profile(
            &Some("work".into()),
            &Some("personal".into())
        ));
        assert!(!same_profile(&Some("work".into()), &None));
    }

    /// 账号是**可选**能力槽（不声明它的 agent，卡片不显示登录态、也不给登录入口），但当前五家
    /// 恰好都声明了——所以这里钉的是**矩阵**，不是「全员必须有」。
    ///
    /// 这条曾经写成「每个插件都必须声明 account」。那不是规矩，只是当时三家碰巧都有；
    /// 而更糟的是，前端一直靠「账号查不出来」反推「未登录」，于是一个没有账号能力的 agent 会被
    /// 亮出一个必然失败的登录按钮。契约现在由 `AgentDescriptor::supports_account` 显式承载。
    #[test]
    fn account_capability_matches_the_declared_matrix() {
        let with: std::collections::BTreeSet<&str> =
            all_with_account().map(|p| p.id().as_str()).collect();
        assert_eq!(
            with,
            ["claude", "codex", "gemini", "kimi", "opencode"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "账号能力的覆盖面变了——加/减了 agent，还是漏声明了能力槽？"
        );
        // all_with_account 必须恰好是「声明了 account 的那些」，不多不少。
        for p in meowo_agent::all() {
            assert_eq!(
                p.account().is_some(),
                with.contains(p.id().as_str()),
                "{} 的账号能力与 all_with_account 不一致",
                p.id()
            );
        }
    }

    /// **有账号能力 ⇒ 必须有登录入口。** 两者一旦脱节，前端就会亮出一个点了必失败的登录按钮：
    /// 卡片按「有账号能力但没登录」显示登录按钮，后端的 `login_argv()` 却是 None，只能回一句
    /// 「该 agent 未声明登录入口」。gemini 正是这么翻车的——它没有登录子命令（登录靠裸启动），
    /// 而当时的 `login_args: &[]` 被当成「无入口」。
    #[test]
    fn every_agent_with_account_can_actually_be_logged_in() {
        for p in all_with_account() {
            let inst = p
                .resolve()
                .unwrap_or_else(|| panic!("{} 解析不出安装实况", p.id()));
            assert!(
                inst.login_argv().is_some(),
                "{} 有账号能力却没有登录入口——登录按钮会亮出来，点下去必然失败",
                p.id()
            );
        }
    }
}
