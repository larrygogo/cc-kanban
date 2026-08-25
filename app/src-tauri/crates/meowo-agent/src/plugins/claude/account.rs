//! Claude Code 账号、实时用量的读取与解析。
//! 纯解析/判定/合并函数在此可单测；I/O 与网络经注入的端口完成，见同文件后半部分。
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::account::{Account, AccountCap, ProviderUsage, UsageKind, UsageLane, USAGE_UNSUPPORTED};
use crate::ports::{Body, HttpRequest, Ports};
use crate::variant::Installation;

/// Claude 账号原始信息（内部类型，供 ClaudeProvider::account() 转换使用）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaudeAccountInfo {
    pub email: String,
    pub display_name: String,
    pub organization: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWindow {
    pub utilization: f64,
    pub resets_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Usage {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    pub seven_day_opus: Option<UsageWindow>,
    pub seven_day_sonnet: Option<UsageWindow>,
    /// 按模型限定的每周配额,来自新版 `limits[]` 数组的 `weekly_scoped` 条目(实测
    /// 2026-08:顶层 `seven_day_opus` 等旧字段已为 null,Fable 专属周限只出现在这里)。
    /// `default` 兼容旧缓存反序列化。
    #[serde(default)]
    pub model_weekly: Vec<ScopedWindow>,
    pub extra_usage_enabled: bool,
}

/// 带名字的用量窗口(`limits[]` 里 scope.model 限定的条目)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScopedWindow {
    /// 被限定的模型名(`scope.model.display_name`,如 "Fable")。
    pub label: String,
    pub utilization: f64,
    pub resets_at: String,
}

/// 套餐徽章的候选字段，按「针对本人」到「针对组织」排列，取首个有语义的。
///
/// **刻意不含 `billingType`**：它是计费方式（`stripe_subscription` 等），从来不是套餐。
/// 早先拿它当兜底，于是 `seatTier` 为空的账号（Max 订阅用户很常见——套餐信息只落在限额档位
/// 字段里）徽章上赫然写着「Stripe_subscription」。宁可不显示徽章，也不显示一个错的。
const PLAN_FIELDS: [&str; 4] = [
    "userRateLimitTier",
    "organizationRateLimitTier",
    "seatTier",
    "organizationType",
];

/// 无套餐语义的占位值——命中则继续看下一个候选字段，别把 `default` 显示成「Default」。
fn is_plan_placeholder(s: &str) -> bool {
    matches!(s, "" | "default" | "unknown" | "none" | "null")
}

/// 原始档位串 → 徽章文案：`default_claude_max_20x` → `Max 20x`、`claude_max` → `Max`、`pro` → `Pro`。
///
/// 剥掉 `default_` / `claude_` 前缀后按 `_` 分段；以数字开头的段是倍率（`20x`）保持原样，
/// 其余段首字母大写。返回 None 表示该字段没有可展示的套餐信息。
fn normalize_plan(raw: &str) -> Option<String> {
    let low = raw.trim().to_lowercase();
    let mut s: &str = &low;
    s = s.strip_prefix("default_").unwrap_or(s);
    s = s.strip_prefix("claude_").unwrap_or(s);
    if is_plan_placeholder(s) {
        return None;
    }
    let words: Vec<String> = s
        .split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            if p.starts_with(|c: char| c.is_ascii_digit()) {
                return p.to_string(); // 倍率段：20x、5x
            }
            let mut ch = p.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => String::new(),
            }
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// 从 ~/.claude.json 的根 JSON 解析账号（取 oauthAccount）。无则 None。
pub fn parse_account(root: &serde_json::Value) -> Option<ClaudeAccountInfo> {
    let a = root.get("oauthAccount")?;
    let email = a
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if email.is_empty() {
        return None;
    }
    let display_name = a
        .get("displayName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&email)
        .to_string();
    let organization = a
        .get("organizationName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    // 套餐标签：按 PLAN_FIELDS 顺序取首个有语义的档位字段，规范成展示文案。
    let plan = PLAN_FIELDS
        .iter()
        .find_map(|k| normalize_plan(a.get(*k)?.as_str()?));
    Some(ClaudeAccountInfo {
        email,
        display_name,
        organization,
        plan,
    })
}

/// 解析 /api/oauth/usage 响应。各 bucket 可能为 null/缺失 → Option。
pub fn parse_usage(v: &serde_json::Value) -> Usage {
    fn win(v: &serde_json::Value, key: &str) -> Option<UsageWindow> {
        let w = v.get(key)?;
        if w.is_null() {
            return None;
        }
        let utilization = w.get("utilization").and_then(|x| x.as_f64())?;
        let resets_at = w
            .get("resets_at")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        Some(UsageWindow {
            utilization,
            resets_at,
        })
    }
    let extra_usage_enabled = v
        .get("extra_usage")
        .and_then(|e| e.get("is_enabled"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    // 新版响应的 `limits[]`:模型专属周限(kind=weekly_scoped)只出现在这里。实测样本
    // (2026-08,Fable 专属周限):{"kind":"weekly_scoped","group":"weekly","percent":83,
    // "resets_at":"…","scope":{"model":{"id":null,"display_name":"Fable"}}}。
    // 只收 scope.model 限定的条目;surface 限定或无 scope 的(session/weekly_all 已由
    // 顶层 five_hour/seven_day 覆盖)不重复收。名字缺失(display_name 与 id 都空)的
    // 条目丢弃——没有名字的模型配额没法展示。
    let model_weekly = v
        .get("limits")
        .and_then(|l| l.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter(|e| e.get("kind").and_then(|k| k.as_str()) == Some("weekly_scoped"))
                .filter_map(|e| {
                    let model = e.get("scope")?.get("model")?;
                    let label = model
                        .get("display_name")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            model.get("id").and_then(|x| x.as_str()).filter(|s| !s.is_empty())
                        })?
                        .to_string();
                    let utilization = e.get("percent").and_then(|x| x.as_f64())?;
                    let resets_at = e
                        .get("resets_at")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(ScopedWindow {
                        label,
                        utilization,
                        resets_at,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Usage {
        five_hour: win(v, "five_hour"),
        seven_day: win(v, "seven_day"),
        seven_day_opus: win(v, "seven_day_opus"),
        seven_day_sonnet: win(v, "seven_day_sonnet"),
        model_weekly,
        extra_usage_enabled,
    }
}

/// token 是否需要刷新：到期前留 60s 余量。
pub fn is_token_expired(expires_at_ms: i64, now_ms: i64) -> bool {
    now_ms >= expires_at_ms - 60_000
}

/// 把新 token 合并进原 credentials JSON：只改 claudeAiOauth.{accessToken,refreshToken,expiresAt}，
/// 保留 mcpOAuth 等其它所有字段。返回新 JSON。
pub fn merge_credentials(
    original: &serde_json::Value,
    access_token: &str,
    refresh_token: &str,
    expires_at_ms: i64,
) -> serde_json::Value {
    let mut out = original.clone();
    if let Some(obj) = out.as_object_mut() {
        let oauth = obj
            .entry("claudeAiOauth")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(o) = oauth.as_object_mut() {
            o.insert("accessToken".into(), serde_json::json!(access_token));
            o.insert("refreshToken".into(), serde_json::json!(refresh_token));
            o.insert("expiresAt".into(), serde_json::json!(expires_at_ms));
        }
    }
    out
}

/// 凭据根是否缺少可用的 Anthropic OAuth（→ 第三方/非官方登录，用量接口不适用）。
/// 判定：根缺失、缺 claudeAiOauth、或 access+refresh 双空。纯函数便于单测。
pub fn oauth_credentials_missing(root: Option<&serde_json::Value>) -> bool {
    let Some(oauth) = root.and_then(|r| r.get("claudeAiOauth")) else {
        return true;
    };
    let access = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let refresh = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    access.is_empty() && refresh.is_empty()
}

/// 用量端点是 account 侧的事，不进 `AuthScheme`（后者只管「凭据在哪 + 怎么刷新」）。
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const HTTP_TIMEOUT: Duration = Duration::from_secs(6);

/// claude 的鉴权声明，取自变体表：OAuth client_id、刷新端点、凭据位置不再是本文件里的一把
/// 常量——换个版本形态只改变体表一处。
fn claude_auth(inst: &Installation) -> Option<&'static crate::AuthScheme> {
    inst.auth
}

/// 账号信息（email / 组织 / 套餐）所在的 `.claude.json`。
///
/// **它在哪，取决于有没有设 `CLAUDE_CONFIG_DIR`**——这是 claude 的一处历史遗留，两种情形并不同构：
///
/// - **默认账号**：`~/.claude.json`，在 home 根上，是数据目录（`~/.claude`）的**兄弟**，不在它里面。
/// - **profile（设了 `CLAUDE_CONFIG_DIR`）**：`<配置目录>/.claude.json`，**落在目录里面**（实测：
///   把 `CLAUDE_CONFIG_DIR` 指向一个空目录跑一次 claude，它在里面建出了 `.claude.json` /
///   `projects` / `sessions`，并如实报「Not logged in」）。
///
/// 不区分这两种情形的后果是**串号**：profile 用着账号 B 的凭据，卡片上却显示账号 A 的邮箱。
fn claude_json_path(inst: &Installation) -> Option<PathBuf> {
    if inst.profile.is_some() {
        return Some(inst.data_dir.join(".claude.json"));
    }
    crate::home_dir().map(|h| h.join(".claude.json"))
}

/// Claude Code 把 OAuth 凭据写在 `<data_dir>/.credentials.json`（尊重 `CLAUDE_CONFIG_DIR`）。
/// macOS 改存登录 Keychain，此路径仅作回退。
fn credentials_path(inst: &Installation) -> Option<PathBuf> {
    inst.credentials_path()
}

fn read_json_file(path: &std::path::Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// 当前 Unix 毫秒。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 读账号（~/.claude.json）。
pub fn read_account(inst: &Installation) -> Option<ClaudeAccountInfo> {
    parse_account(&read_json_file(&claude_json_path(inst)?)?)
}

/// Keychain 条目的 service 名与「写回时的 account 兜底值」，均来自变体表的
/// `CredentialSource::KeychainOrFile`。变体表被改成非 Keychain 形态（不该发生）→ 退回历史常量。
fn keychain_spec(inst: &Installation) -> (&'static str, &'static str) {
    match claude_auth(inst).map(|a| a.credentials) {
        Some(crate::CredentialSource::KeychainOrFile {
            service, account, ..
        }) => (service, account),
        _ => ("Claude Code-credentials", "root"),
    }
}

/// 凭据存在哪：有可用密钥链（macOS 登录 Keychain）就在那儿，否则 `<data_dir>/.credentials.json`。
///
/// 这里是一次**运行时**判断而非 `#[cfg(target_os = "macos")]`：平台差异由宿主注入的
/// [`KeychainPort`](crate::ports::KeychainPort) 承担，插件层因此没有一行 `cfg`，且在任意平台
/// 都能用假密钥链把两条分支都测到。
fn read_credentials_root(inst: &Installation, ports: &Ports) -> Option<serde_json::Value> {
    if ports.keychain.available() {
        return serde_json::from_str(&ports.keychain.read_password(keychain_spec(inst).0)?).ok();
    }
    read_json_file(&credentials_path(inst)?)
}

/// 刷新 token 后把新凭据写回原存储（保留其余字段）。
fn write_credentials_root(
    inst: &Installation,
    ports: &Ports,
    value: &serde_json::Value,
) -> Result<(), String> {
    if ports.keychain.available() {
        let (service, fallback_account) = keychain_spec(inst);
        let body = serde_json::to_string(value).map_err(|e| e.to_string())?;
        // 读得到实际 account 就按同名更新；读不到用变体表声明的兜底值。
        let account = ports
            .keychain
            .read_account(service)
            .unwrap_or_else(|| fallback_account.to_string());
        return ports.keychain.write_password(service, &account, &body);
    }
    let path = credentials_path(inst).ok_or("解析不到 claude 凭据路径")?;
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    crate::fsutil::write_atomic_secure(&path, &body).map_err(|e| e.to_string())
}

/// 确保有有效 access token：未过期直接返回；过期则刷新并写回原存储，再返回新 token。
fn ensure_valid_token(inst: &Installation, ports: &Ports) -> Result<String, String> {
    use std::sync::Mutex;
    // 串行化刷新：并发刷新（多窗口/连点）会各自用旋转后失效的 refresh_token 重复请求、互相覆盖。
    // 持锁后下面会重读凭据并重新判过期（双检）——若刚被另一调用方刷新过，直接走 fast-path 返回新
    // token，不再重复刷新。
    static REFRESH_LOCK: Mutex<()> = Mutex::new(());
    let _guard = REFRESH_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let root = read_credentials_root(inst, ports);
    // 读不到可用 OAuth 凭据 → 视为第三方/非官方登录，用量接口不适用，返回标记码。
    if oauth_credentials_missing(root.as_ref()) {
        return Err(USAGE_UNSUPPORTED.into());
    }
    let root = root.expect("credentials present: oauth_credentials_missing 已排除 None");
    let oauth = root.get("claudeAiOauth").ok_or("凭据缺 claudeAiOauth")?;
    let access = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let refresh = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_at = oauth.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);

    if !access.is_empty() && !is_token_expired(expires_at, now_ms()) {
        return Ok(access);
    }
    if refresh.is_empty() {
        return Err("token 已过期且无 refreshToken".into());
    }
    // 刷新端点与 client_id 来自变体表；返回 `invalid_client` 即该变体的 client_id 与本机 claude 不符。
    let Some(spec) = claude_auth(inst).and_then(|a| a.refresh) else {
        return Err("claude 变体表未声明 OAuth 刷新参数".into());
    };
    let text = ports
        .http
        .send(&HttpRequest {
            method: "POST",
            url: spec.token_url,
            headers: &[],
            body: Body::Json(serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh,
                "client_id": spec.client_id,
            })),
            timeout: HTTP_TIMEOUT,
        })
        .map_err(|e| format!("刷新 token 失败：{e}"))?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let new_access = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("刷新响应缺 access_token")?
        .to_string();
    let new_refresh = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or(&refresh)
        .to_string();
    // 钳下限 600s：服务端若异常返回 0/负/极小值，避免写回一个立刻过期的 expiresAt 而陷入每次都刷新。
    // 钳上限 86400s（24h）：防服务端异常返回超大值导致 now_ms() + expires_in*1000 溢出回绕。
    let expires_in = body
        .get("expires_in")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(3600)
        .clamp(600, 86400);
    let new_expires_at = now_ms() + expires_in * 1000;

    let merged = merge_credentials(&root, &new_access, &new_refresh, new_expires_at);
    write_credentials_root(inst, ports, &merged)?;
    Ok(new_access)
}

/// 联网拉实时用量（含按需刷新 token）。缓存/限频/写回归宿主编排层，本函数只负责拿一次。
fn fetch_usage_live(inst: &Installation, ports: &Ports) -> Result<Usage, String> {
    let token = ensure_valid_token(inst, ports)?;
    let bearer = format!("Bearer {token}");
    let text = ports
        .http
        .send(&HttpRequest {
            method: "GET",
            url: USAGE_URL,
            headers: &[("Authorization", &bearer), ("anthropic-beta", OAUTH_BETA)],
            body: Body::Empty,
            timeout: HTTP_TIMEOUT,
        })
        .map_err(|e| format!("请求用量失败：{e}"))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(parse_usage(&v))
}

/// 将旧 Usage 映射为通用 ProviderUsage 泳道格式。
/// five_hour→FiveHour, seven_day→SevenDay, model_weekly→ModelWeekly(带模型名 label),
/// seven_day_opus→Opus 仅在没有 model_weekly 时兜底(旧响应;两边同现会重复表意),
/// (utilization→used_pct, unit "percent", resets_at 原样),extra_usage_enabled→note。
/// seven_day_sonnet 忽略（保持现视觉）。
pub fn map_to_provider_usage(u: &Usage) -> ProviderUsage {
    let mut lanes: Vec<UsageLane> = Vec::new();

    if let Some(w) = &u.five_hour {
        lanes.push(UsageLane {
            kind: UsageKind::FiveHour,
            used_pct: Some(w.utilization),
            used: None,
            limit: None,
            unit: Some("percent".to_string()),
            resets_at: non_empty_str(&w.resets_at),
            label: None,
        });
    }
    if let Some(w) = &u.seven_day {
        lanes.push(UsageLane {
            kind: UsageKind::SevenDay,
            used_pct: Some(w.utilization),
            used: None,
            limit: None,
            unit: Some("percent".to_string()),
            resets_at: non_empty_str(&w.resets_at),
            label: None,
        });
    }
    for w in &u.model_weekly {
        lanes.push(UsageLane {
            kind: UsageKind::ModelWeekly,
            used_pct: Some(w.utilization),
            used: None,
            limit: None,
            unit: Some("percent".to_string()),
            resets_at: non_empty_str(&w.resets_at),
            label: Some(w.label.clone()),
        });
    }
    if u.model_weekly.is_empty() {
        if let Some(w) = &u.seven_day_opus {
            lanes.push(UsageLane {
                kind: UsageKind::Opus,
                used_pct: Some(w.utilization),
                used: None,
                limit: None,
                unit: Some("percent".to_string()),
                resets_at: non_empty_str(&w.resets_at),
                label: None,
            });
        }
    }
    // seven_day_sonnet 忽略（保持现视觉）

    let note = u
        .extra_usage_enabled
        .then(|| "extra_usage_enabled".to_string());

    ProviderUsage {
        lanes,
        note,
        plan: None,
    }
}

/// 空字符串→ None（resets_at 字段处理）。
fn non_empty_str(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

pub struct ClaudeAccount;
pub static ACCOUNT: ClaudeAccount = ClaudeAccount;

impl AccountCap for ClaudeAccount {
    fn account(&self, inst: &Installation, ports: &Ports) -> Option<Account> {
        // `.claude.json` 的账号资料在**退出登录后仍会留着**；必须同时存在凭据才算已登录。
        // 这里只读本地文件/Keychain，不联网，仍可供登录轮询高频调用。
        let credentials = read_credentials_root(inst, ports);
        if oauth_credentials_missing(credentials.as_ref()) {
            return None;
        }
        read_account(inst).map(|a| Account {
            email: Some(a.email),
            display_name: Some(a.display_name),
            organization: a.organization,
            plan: a.plan,
            login_label: None,
        })
    }

    fn fetch_usage(&self, inst: &Installation, ports: &Ports) -> Result<ProviderUsage, String> {
        fetch_usage_live(inst, ports).map(|u| map_to_provider_usage(&u))
    }

    /// 读不到可用的 Anthropic OAuth 凭据（第三方/中转登录，或尚未登录）→ 用量接口不适用。
    fn usage_supported(&self, inst: &Installation, ports: &Ports) -> bool {
        !oauth_credentials_missing(read_credentials_root(inst, ports).as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_account_extracts_fields() {
        let root = json!({"oauthAccount":{"emailAddress":"a@b.com","displayName":"Larry","organizationName":"Acme","seatTier":"max","billingType":"subscription"}});
        let a = parse_account(&root).unwrap();
        assert_eq!(a.email, "a@b.com");
        assert_eq!(a.display_name, "Larry");
        assert_eq!(a.organization.as_deref(), Some("Acme"));
        assert_eq!(a.plan.as_deref(), Some("Max"));
    }

    #[test]
    fn normalize_plan_strips_prefixes_and_keeps_multiplier() {
        assert_eq!(
            normalize_plan("default_claude_max_20x").as_deref(),
            Some("Max 20x")
        );
        assert_eq!(normalize_plan("claude_max").as_deref(), Some("Max"));
        assert_eq!(normalize_plan("max").as_deref(), Some("Max"));
        assert_eq!(normalize_plan("pro").as_deref(), Some("Pro"));
        assert_eq!(normalize_plan("  MAX_5X  ").as_deref(), Some("Max 5x"));
        // 占位值不该变成「Default」这样的假徽章
        assert_eq!(normalize_plan("default"), None);
        assert_eq!(normalize_plan(""), None);
    }

    /// 回归：真实 Max 订阅账号的形状——seatTier 是**空串**，套餐只落在限额档位字段里。
    /// 旧逻辑（seatTier → billingType）于是把徽章显示成「Stripe_subscription」。
    #[test]
    fn plan_prefers_rate_limit_tier_over_billing_type() {
        let root = json!({"oauthAccount":{
            "emailAddress":"a@b.com",
            "seatTier":"",
            "userRateLimitTier":"",
            "billingType":"stripe_subscription",
            "organizationType":"claude_max",
            "organizationRateLimitTier":"default_claude_max_20x",
        }});
        assert_eq!(
            parse_account(&root).unwrap().plan.as_deref(),
            Some("Max 20x")
        );
    }

    /// billingType 是计费方式，永远不能当套餐——没有任何档位字段时宁可不显示徽章。
    #[test]
    fn billing_type_is_never_used_as_plan() {
        let root =
            json!({"oauthAccount":{"emailAddress":"a@b.com","billingType":"stripe_subscription"}});
        assert_eq!(parse_account(&root).unwrap().plan, None);
    }

    /// 用户级档位比组织级更精确（组织可能是别人的），优先取它。
    #[test]
    fn user_tier_wins_over_organization_tier() {
        let root = json!({"oauthAccount":{
            "emailAddress":"a@b.com",
            "userRateLimitTier":"claude_pro",
            "organizationRateLimitTier":"default_claude_max_20x",
        }});
        assert_eq!(parse_account(&root).unwrap().plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn parse_account_none_without_email() {
        assert!(parse_account(&json!({"oauthAccount":{}})).is_none());
        assert!(parse_account(&json!({})).is_none());
    }

    #[test]
    fn parse_usage_full_and_nulls() {
        let v = json!({
            "five_hour":{"utilization":13.0,"resets_at":"2026-06-07T08:50:01Z"},
            "seven_day":{"utilization":19.0,"resets_at":"2026-06-11T12:00:00Z"},
            "seven_day_opus":null,
            "seven_day_sonnet":{"utilization":2.0,"resets_at":"x"},
            "extra_usage":{"is_enabled":false}
        });
        let u = parse_usage(&v);
        assert_eq!(u.five_hour.as_ref().unwrap().utilization, 13.0);
        assert_eq!(
            u.five_hour.as_ref().unwrap().resets_at,
            "2026-06-07T08:50:01Z"
        );
        assert!(u.seven_day_opus.is_none());
        assert_eq!(u.seven_day_sonnet.as_ref().unwrap().utilization, 2.0);
        assert!(!u.extra_usage_enabled);
    }

    #[test]
    fn parse_usage_missing_fields_degrade() {
        let u = parse_usage(&json!({}));
        assert!(u.five_hour.is_none() && u.seven_day.is_none() && !u.extra_usage_enabled);
    }

    #[test]
    fn is_token_expired_margin() {
        let now = 1_000_000_000_000i64;
        assert!(is_token_expired(now, now));
        assert!(is_token_expired(now + 30_000, now));
        assert!(!is_token_expired(now + 120_000, now));
    }

    /// 无密钥链的平台（或注入了假密钥链的测试）走文件分支，且**不联网**。
    /// `usage_supported` 只读凭据，NoHttp 会在任何请求上 panic，据此断言这条路径确实不发请求。
    #[test]
    fn usage_supported_reads_credentials_without_network() {
        use crate::ports::test_doubles::NoHttp;
        let ports = Ports {
            http: &NoHttp,
            keychain: &crate::ports::NoKeychain,
        };
        // 本机大概率没有 claude 凭据文件 → 读不到 OAuth → 不支持用量。真有凭据时也只是返回 true，
        // 两种取值都不该发起网络请求（NoHttp 会 panic）。
        let inst = crate::by_id("claude")
            .unwrap()
            .resolve()
            .expect("总能推出默认落点");
        let _ = ACCOUNT.usage_supported(&inst, &ports);
    }

    #[test]
    fn oauth_credentials_missing_detects_third_party() {
        // 无根、缺 claudeAiOauth、access+refresh 双空 → 缺失（第三方/未登录）。
        assert!(oauth_credentials_missing(None));
        assert!(oauth_credentials_missing(Some(&json!({"mcpOAuth": {}}))));
        assert!(oauth_credentials_missing(Some(
            &json!({"claudeAiOauth": {}})
        )));
        assert!(oauth_credentials_missing(Some(
            &json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}})
        )));
        // 有 access 或有 refresh → 视为官方 OAuth 凭据，不缺失。
        assert!(!oauth_credentials_missing(Some(
            &json!({"claudeAiOauth": {"accessToken": "a", "refreshToken": ""}})
        )));
        assert!(!oauth_credentials_missing(Some(
            &json!({"claudeAiOauth": {"accessToken": "", "refreshToken": "r"}})
        )));
    }

    #[test]
    fn merge_credentials_preserves_other_fields() {
        let original = json!({
            "mcpOAuth":{"some":"value"},
            "claudeAiOauth":{"accessToken":"old","refreshToken":"oldr","expiresAt":1,"scopes":["a"],"subscriptionType":"max"}
        });
        let merged = merge_credentials(&original, "newA", "newR", 999);
        assert_eq!(merged["mcpOAuth"]["some"], "value");
        assert_eq!(merged["claudeAiOauth"]["accessToken"], "newA");
        assert_eq!(merged["claudeAiOauth"]["refreshToken"], "newR");
        assert_eq!(merged["claudeAiOauth"]["expiresAt"], 999);
        assert_eq!(merged["claudeAiOauth"]["scopes"][0], "a");
        assert_eq!(merged["claudeAiOauth"]["subscriptionType"], "max");
    }

    /// 走生产路径的防溢出回归：刷新响应给出病态大的 expires_in 时，写回的 expiresAt 必须被钳在
    /// 24h 内——否则 now_ms() + expires_in*1000 溢出（debug panic / release 回绕成过去的时间戳，
    /// 于是每次拉用量都重刷）。
    #[test]
    fn refresh_clamps_pathological_expires_in() {
        /// 假刷新端点：回一份 expires_in = i64::MAX 的响应。
        struct HugeExpiresIn;
        impl crate::ports::HttpPort for HugeExpiresIn {
            fn send(&self, _req: &HttpRequest) -> Result<String, crate::ports::HttpError> {
                Ok(r#"{"access_token":"new_access","refresh_token":"new_refresh","expires_in":9223372036854775807}"#
                    .to_string())
            }
            fn download(
                &self,
                _url: &str,
                _dest: &std::path::Path,
                _timeout: std::time::Duration,
                _on_progress: &mut dyn FnMut(u64, Option<u64>),
            ) -> Result<u64, crate::ports::HttpError> {
                unreachable!("token 刷新不走下载")
            }
        }

        // profile 实况：凭据落在 <root>/.credentials.json；NoKeychain 恒不可用 → 走文件分支。
        let root = std::env::temp_dir().join(format!("meowo-claude-clamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let inst = crate::by_id("claude")
            .unwrap()
            .installation_for_profile(&root)
            .expect("claude 支持多账号");
        std::fs::write(
            root.join(".credentials.json"),
            json!({"claudeAiOauth":{"accessToken":"old","refreshToken":"oldr","expiresAt":0}})
                .to_string(),
        )
        .unwrap();

        let ports = Ports {
            http: &HugeExpiresIn,
            keychain: &crate::ports::NoKeychain,
        };
        let before = now_ms();
        let token = ensure_valid_token(&inst, &ports).expect("刷新应成功");
        assert_eq!(token, "new_access");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(".credentials.json")).unwrap())
                .unwrap();
        let expires_at = written["claudeAiOauth"]["expiresAt"].as_i64().unwrap();
        assert!(
            expires_at >= before + 86400 * 1000 && expires_at <= now_ms() + 86400 * 1000,
            "expires_in 必须被钳到 86400s（expires_at={expires_at}）"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn map_to_provider_usage_maps_lanes_correctly() {
        let u = Usage {
            five_hour: Some(UsageWindow {
                utilization: 13.0,
                resets_at: "2026-06-07T08:50:01Z".into(),
            }),
            seven_day: Some(UsageWindow {
                utilization: 55.0,
                resets_at: "2026-06-11T12:00:00Z".into(),
            }),
            seven_day_opus: Some(UsageWindow {
                utilization: 30.0,
                resets_at: "2026-06-11T12:00:00Z".into(),
            }),
            seven_day_sonnet: Some(UsageWindow {
                utilization: 2.0,
                resets_at: "x".into(),
            }),
            model_weekly: Vec::new(),
            extra_usage_enabled: true,
        };
        let pu = map_to_provider_usage(&u);
        // 应有 3 条泳道（sonnet 忽略；无 model_weekly 时 seven_day_opus 兜底成 Opus 泳道）。
        assert_eq!(pu.lanes.len(), 3);
        assert_eq!(pu.lanes[0].kind, UsageKind::FiveHour);
        assert_eq!(pu.lanes[0].used_pct, Some(13.0));
        assert_eq!(pu.lanes[0].unit.as_deref(), Some("percent"));
        assert_eq!(
            pu.lanes[0].resets_at.as_deref(),
            Some("2026-06-07T08:50:01Z")
        );
        assert_eq!(pu.lanes[1].kind, UsageKind::SevenDay);
        assert_eq!(pu.lanes[2].kind, UsageKind::Opus);
        // extra_usage_enabled → note。
        assert_eq!(pu.note.as_deref(), Some("extra_usage_enabled"));
    }

    #[test]
    fn parse_usage_reads_weekly_scoped_limits() {
        // 真机响应样本(2026-08):Fable 专属周限只在 limits[],顶层 seven_day_* 全 null。
        let v = json!({
            "five_hour": {"utilization": 35.0, "resets_at": "2026-08-25T19:09:59+08:00"},
            "seven_day": {"utilization": 44.0, "resets_at": "2026-08-27T02:59:59+08:00"},
            "seven_day_opus": null,
            "limits": [
                {"kind": "session", "group": "session", "percent": 35, "resets_at": "2026-08-25T19:09:59+08:00", "scope": null},
                {"kind": "weekly_all", "group": "weekly", "percent": 44, "resets_at": "2026-08-27T02:59:59+08:00", "scope": null},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 83, "severity": "warning",
                 "resets_at": "2026-08-27T02:59:59+08:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}}
            ]
        });
        let u = parse_usage(&v);
        assert_eq!(u.model_weekly.len(), 1);
        assert_eq!(u.model_weekly[0].label, "Fable");
        assert_eq!(u.model_weekly[0].utilization, 83.0);
        assert_eq!(u.model_weekly[0].resets_at, "2026-08-27T02:59:59+08:00");
        // 无 scope.model 的条目(session/weekly_all)不收;limits 缺失整体退化为空。
        assert!(parse_usage(&json!({})).model_weekly.is_empty());
        // display_name 缺失回退 id;两者皆空则丢弃。
        let v2 = json!({"limits": [
            {"kind": "weekly_scoped", "percent": 10.0, "scope": {"model": {"id": "claude-opus-5", "display_name": null}}},
            {"kind": "weekly_scoped", "percent": 20.0, "scope": {"model": {"id": null, "display_name": null}}}
        ]});
        let u2 = parse_usage(&v2);
        assert_eq!(u2.model_weekly.len(), 1);
        assert_eq!(u2.model_weekly[0].label, "claude-opus-5");
        assert_eq!(u2.model_weekly[0].resets_at, "");
    }

    #[test]
    fn map_to_provider_usage_prefers_model_weekly_over_legacy_opus() {
        let u = Usage {
            seven_day_opus: Some(UsageWindow {
                utilization: 30.0,
                resets_at: "2026-06-11T12:00:00Z".into(),
            }),
            model_weekly: vec![ScopedWindow {
                label: "Fable".into(),
                utilization: 83.0,
                resets_at: "2026-08-27T02:59:59+08:00".into(),
            }],
            ..Default::default()
        };
        let pu = map_to_provider_usage(&u);
        // 同现时只出 model_weekly(旧 Opus 泳道与之重复表意)。
        assert_eq!(pu.lanes.len(), 1);
        assert_eq!(pu.lanes[0].kind, UsageKind::ModelWeekly);
        assert_eq!(pu.lanes[0].label.as_deref(), Some("Fable"));
        assert_eq!(pu.lanes[0].used_pct, Some(83.0));
    }

    #[test]
    fn map_to_provider_usage_empty_resets_at_becomes_none() {
        let u = Usage {
            five_hour: Some(UsageWindow {
                utilization: 1.0,
                resets_at: "".into(),
            }),
            ..Default::default()
        };
        let pu = map_to_provider_usage(&u);
        assert!(pu.lanes[0].resets_at.is_none());
    }
}
