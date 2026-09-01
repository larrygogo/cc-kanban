//! `check_agent_updates` 命令：各**已安装** agent 的版本与更新状态。
//!
//! 本机版本复用 `crate::probe_cli_version`（`<launch_argv> --version`，进程级缓存）；
//! 最新版本按 agent 各自的权威源拉取——npm 系走 registry 的 `/latest` JSON，kimi 走
//! `code.kimi.com` 的纯文本版本号（不走 npm）。出站一律经 `ports::HostPorts::for_agent`
//! 的 ureq 客户端（代理按 agent 解析）。
//!
//! 探测失败（网络/解析/形态不对）一律表现为 `latest_version: None, update_available: false`，
//! 绝不报错给前端——探测失败 ≠ 没有新版，UI 据此不显示更新入口即可。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use meowo_agent::{Body, HttpRequest};

/// 单个请求的超时。最多 5 个 agent 顺序拉取，最坏 ~25s，可接受。
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// 结果缓存时长：版本源不会分钟级变化，设置页反复打开不该每次都打 5 个请求。
const CACHE_TTL: Duration = Duration::from_secs(600);

/// `check_agent_updates` 的返回值。字段名保持 snake_case（serde 默认），与前端约定一致。
#[derive(Clone, Debug, serde::Serialize)]
pub struct AgentUpdateInfo {
    pub provider: String,
    pub installed_version: Option<String>,
    /// None = 探测失败/无来源，UI 据此不显示更新入口。
    pub latest_version: Option<String>,
    pub update_available: bool,
}

/// 最新版本的权威来源。
enum LatestSource {
    /// npm registry：`https://registry.npmjs.org/<pkg>/latest` 的 JSON `.version`。
    Npm(&'static str),
    /// 纯文本版本号 URL（kimi：发布物不在 npm 上）。
    PlainText(&'static str),
}

/// 各 agent 的最新版本来源。返回 None 的 agent 不参与更新检查（latest_version 恒 None）。
fn latest_source(id: meowo_agent::AgentId) -> Option<LatestSource> {
    Some(match id.as_str() {
        "claude" => LatestSource::Npm("@anthropic-ai/claude-code"),
        "codex" => LatestSource::Npm("@openai/codex"),
        "gemini" => LatestSource::Npm("@google/gemini-cli"),
        "opencode" => LatestSource::Npm("opencode-ai"),
        "kimi" => LatestSource::PlainText("https://code.kimi.com/kimi-code/latest"),
        _ => return None,
    })
}

/// 版本号形态校验：`x.y.z` 纯数字三段。取回的文本若被中间设备换成一页 HTML（Cloudflare
/// 人机校验页以 HTTP 200 返回是实测发生过的），这里拦下——同 claude 直下安装 `parse_version`
/// 的防投毒思路。
fn is_version_triple(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').count() == 3
        && s.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// 解析 npm registry `/latest` JSON 的 `.version` 字段。纯函数，便于单测。
fn parse_npm_version(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let version = v.get("version")?.as_str()?.trim();
    is_version_triple(version).then(|| version.to_string())
}

/// 解析纯文本版本号（kimi）。trim 后必须是 x.y.z 数字形态，否则当探测失败。纯函数。
fn parse_plain_version(body: &str) -> Option<String> {
    let v = body.trim();
    is_version_triple(v).then(|| v.to_string())
}

/// 从可能带杂文本的版本串里提取第一个数字版本段（`x.y` 或 `x.y.z`，不足三段补零）。
/// 容忍 `v1.2.3` 前缀与 `kimi-code 0.39.1` 这类杂文本；提取不到 → None。
fn extract_version(s: &str) -> Option<[u64; 3]> {
    let bytes = s.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if !bytes[idx].is_ascii_digit() {
            idx += 1;
            continue;
        }
        let start = idx;
        while idx < bytes.len() && (bytes[idx].is_ascii_digit() || bytes[idx] == b'.') {
            idx += 1;
        }
        let token = &s[start..idx];
        let parts: Vec<&str> = token.split('.').collect();
        if !(2..=3).contains(&parts.len())
            || parts
                .iter()
                .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
        {
            continue; // 形态不对，扫下一个数字段
        }
        let mut v = [0u64; 3];
        let mut ok = true;
        for (i, p) in parts.iter().enumerate() {
            match p.parse() {
                Ok(n) => v[i] = n,
                Err(_) => {
                    ok = false; // 溢出等，扫下一个数字段
                    break;
                }
            }
        }
        if ok {
            return Some(v);
        }
    }
    None
}

/// 比较两个版本串。任一侧提取不出数字版本段 → None（调用方据此不显示更新入口）。
fn compare_versions(installed: &str, latest: &str) -> Option<std::cmp::Ordering> {
    Some(extract_version(installed)?.cmp(&extract_version(latest)?))
}

/// 10 分钟结果缓存，key = provider。命中且未过期直接返回，不再探测/拉取。
static CACHE: LazyLock<Mutex<HashMap<String, (Instant, AgentUpdateInfo)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 安装/更新成功后调用：清掉该 agent 的更新检查结果，设置页立刻能看到新版本号。
pub(crate) fn invalidate_update_cache(provider: &str) {
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(provider);
}

/// 拉取某 agent 的最新版本。任何网络/解析失败 → None。
fn fetch_latest(id: meowo_agent::AgentId) -> Option<String> {
    let source = latest_source(id)?;
    let ports = crate::ports::HostPorts::for_agent(id);
    let url: String = match &source {
        LatestSource::Npm(pkg) => format!("https://registry.npmjs.org/{pkg}/latest"),
        LatestSource::PlainText(u) => u.to_string(),
    };
    let body = ports
        .as_ports()
        .http
        .send(&HttpRequest {
            method: "GET",
            url: &url,
            headers: &[],
            body: Body::Empty,
            timeout: FETCH_TIMEOUT,
        })
        .ok()?;
    match source {
        LatestSource::Npm(_) => parse_npm_version(&body),
        LatestSource::PlainText(_) => parse_plain_version(&body),
    }
}

/// 探测单个已安装 agent（含缓存读/写）。必须在 blocking 池调用。
fn probe_one(plugin: &'static dyn meowo_agent::AgentPlugin) -> AgentUpdateInfo {
    let provider = plugin.id().as_str().to_string();
    // 中毒恢复而非 unwrap：缓存只是备忘录，读到写一半的旧值无害（同 probe_cli_version）。
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, info)) = cache.get(&provider) {
            if at.elapsed() < CACHE_TTL {
                return info.clone();
            }
        }
    }
    let installed_version = crate::probe_cli_version(plugin);
    let latest_version = fetch_latest(plugin.id());
    let update_available = match (installed_version.as_deref(), latest_version.as_deref()) {
        (Some(i), Some(l)) => {
            compare_versions(i, l).is_some_and(|o| o == std::cmp::Ordering::Less)
        }
        _ => false,
    };
    let info = AgentUpdateInfo {
        provider: provider.clone(),
        installed_version,
        latest_version,
        update_available,
    };
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(provider, (Instant::now(), info.clone()));
    info
}

/// 各已安装 agent 的版本与更新状态。ureq 是同步的，整体放 blocking 池（同 install.rs 的
/// `resolve_install_body`）。多个 agent 顺序拉取：最多 5 个、各有 5s 超时，不引入并行复杂度。
#[tauri::command]
pub(crate) async fn check_agent_updates() -> Vec<AgentUpdateInfo> {
    tauri::async_runtime::spawn_blocking(|| {
        meowo_agent::all()
            .iter()
            .filter(|p| p.is_installed())
            .map(|p| probe_one(*p))
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn compare_basic() {
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Some(Ordering::Equal));
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.2.3", "1.2.4"), Some(Ordering::Less));
    }

    #[test]
    fn compare_tolerates_prefix_and_noise() {
        assert_eq!(compare_versions("v1.2.3", "1.2.4"), Some(Ordering::Less));
        assert_eq!(
            compare_versions("kimi-code 0.39.1", "0.39.2"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_versions("kimi-code 0.39.1", "0.39.1"),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn compare_pads_short_versions() {
        assert_eq!(compare_versions("1.2", "1.2.1"), Some(Ordering::Less));
        assert_eq!(compare_versions("1.2", "1.2.0"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.2.1", "1.2"), Some(Ordering::Greater));
    }

    #[test]
    fn compare_rejects_non_numeric() {
        assert_eq!(compare_versions("abc", "1.2.3"), None);
        assert_eq!(compare_versions("1.2.3", "最新版"), None);
        assert_eq!(compare_versions("", "1.2.3"), None);
    }

    #[test]
    fn npm_version_parses() {
        assert_eq!(
            parse_npm_version(r#"{"name":"@openai/codex","version":"0.42.0"}"#),
            Some("0.42.0".to_string())
        );
    }

    #[test]
    fn npm_version_rejects_missing_field_and_html() {
        assert_eq!(parse_npm_version(r#"{"name":"x"}"#), None);
        assert_eq!(parse_npm_version("<html><body>403</body></html>"), None);
        assert_eq!(parse_npm_version(r#"{"version":"1.2"}"#), None); // 形态不对
    }

    #[test]
    fn plain_version_parses() {
        assert_eq!(parse_plain_version("0.39.1"), Some("0.39.1".to_string()));
        assert_eq!(parse_plain_version("0.39.1\n"), Some("0.39.1".to_string()));
    }

    #[test]
    fn plain_version_rejects_html_poison() {
        assert_eq!(
            parse_plain_version("<html>Just a moment...</html>"),
            None
        );
        assert_eq!(parse_plain_version("v0.39.1"), None); // 纯文本源必须裸数字
        assert_eq!(parse_plain_version(""), None);
    }
}
