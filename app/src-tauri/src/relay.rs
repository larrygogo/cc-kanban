//! API 中转（LLM gateway）配置与启动注入。
//!
//! 与网络代理不同，中转改变的是模型 API 的目标地址、凭据和模型。首版刻意只作用于
//! Meowo 自己拉起的新建/恢复会话：不改写 agent 自身的配置，因此关闭后无需
//! 猜测和恢复用户原值，也不会与 hooks 接线争用同一份配置文件。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

fn default_auth() -> String {
    "bearer".into()
}

/// 单个 agent 的中转规则。密钥不在这里，见 `relay-secrets.json`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RelayRule {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) base_url: String,
    #[serde(default)]
    pub(crate) model: String,
    /// 由插件定义的协议 value；无可选协议时为空串。
    #[serde(default)]
    pub(crate) protocol: String,
    /// 由插件定义的凭据类型 value。
    #[serde(default = "default_auth")]
    pub(crate) auth: String,
    /// 用户勾选的附加环境变量选项 id（插件 `RelayUi::env_options` 声明的子集）。
    #[serde(default)]
    pub(crate) env_options: Vec<String>,
}

impl Default for RelayRule {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            model: String::new(),
            protocol: String::new(),
            auth: default_auth(),
            env_options: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RelaySettings {
    #[serde(default)]
    pub(crate) per_agent: BTreeMap<String, RelayRule>,
}

impl RelaySettings {
    /// 落盘前清洗规则字段。网页「复制」按钮和聊天工具常把零宽/双向控制字符带进剪贴板，
    /// 中文输入法也容易敲出全角冒号——它们在输入框里肉眼不可见或难以分辨，只拦不洗的话，
    /// 用户对着一条「明明正确」的地址毫无办法。
    pub(crate) fn normalize(&mut self) {
        for rule in self.per_agent.values_mut() {
            rule.base_url = sanitize_relay_field(&rule.base_url);
            rule.model = sanitize_relay_field(&rule.model);
        }
    }

    pub(crate) fn rule(&self, id: meowo_agent::AgentId) -> Option<&RelayRule> {
        self.rule_with_secret(id, has_secret(id.as_str()))
    }

    fn rule_with_secret(
        &self,
        id: meowo_agent::AgentId,
        secret_present: bool,
    ) -> Option<&RelayRule> {
        if !secret_present {
            return None;
        }
        let rule = self.per_agent.get(id.as_str()).filter(|r| r.enabled)?;
        let plugin = meowo_agent::by_id(id.as_str())?;
        let cap = plugin.relay()?;
        let installation = plugin.resolve()?;
        cap.supports_variant(installation.variant_tag)
            .then_some(rule)
    }

    pub(crate) fn enabled(&self, id: meowo_agent::AgentId) -> bool {
        self.rule(id).is_some()
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        for (id, rule) in &self.per_agent {
            if !rule.enabled {
                continue;
            }
            let plugin = meowo_agent::by_id(id).ok_or_else(|| format!("未知中转 agent：{id}"))?;
            let cap = plugin
                .relay()
                .ok_or_else(|| format!("{id} 插件不支持 API 中转"))?;
            let variant = plugin
                .resolve()
                .ok_or_else(|| format!("无法解析 {id} 的安装形态"))?;
            // 前缀 agent id：payload 携带整份 per_agent，报错的可能不是当前正在编辑的那个。
            validate_http_url(&rule.base_url).map_err(|e| format!("{id} 的{e}"))?;
            if rule.model.trim().is_empty() {
                return Err(format!("{id} 的中转模型不能为空"));
            }
            cap.validate(relay_config_for(rule, cap), variant.variant_tag)?;
            if !has_secret(id) {
                return Err(format!("请先保存 {id} 的中转密钥"));
            }
        }
        Ok(())
    }
}

fn relay_config_for<'a>(
    rule: &'a RelayRule,
    cap: &'static dyn meowo_agent::RelayCap,
) -> meowo_agent::RelayConfig<'a> {
    let ui = cap.ui();
    meowo_agent::RelayConfig {
        base_url: &rule.base_url,
        model: &rule.model,
        protocol: if rule.protocol.is_empty() {
            ui.default_protocol
        } else {
            &rule.protocol
        },
        auth: if rule.auth.is_empty() {
            ui.default_auth
        } else {
            &rule.auth
        },
        env_options: &rule.env_options,
    }
}

fn supported_relay_plugins() -> impl Iterator<Item = &'static dyn meowo_agent::AgentPlugin> {
    meowo_agent::all().iter().copied().filter(|plugin| {
        plugin.relay().is_some_and(|cap| {
            plugin
                .resolve()
                .is_some_and(|installation| cap.supports_variant(installation.variant_tag))
        })
    })
}

/// 去掉粘贴带入的不可见字符：零宽系、双向控制、BOM、软连字符。
/// 中转（地址/模型/密钥）与代理地址（proxy.rs normalize）共用。
pub(crate) fn strip_invisible(raw: &str) -> String {
    raw.chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{00AD}'
                    | '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2060}'..='\u{2064}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{FEFF}'
            )
        })
        .collect()
}

/// 地址/模型名的完整清洗：去不可见字符、全角 ASCII 折半角（ｈ→h、：→:）、掐头尾空白。
fn sanitize_relay_field(raw: &str) -> String {
    strip_invisible(raw)
        .chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn validate_http_url(raw: &str) -> Result<(), String> {
    let s = raw.trim();
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return Err("中转地址必须以 http:// 或 https:// 开头".into());
    }
    let uri = url::Url::parse(s).map_err(|_| "中转地址格式无效".to_string())?;
    if uri.host_str().is_none() {
        return Err("中转地址缺少主机名".into());
    }
    Ok(())
}

type SecretMap = BTreeMap<String, String>;
static RELAY_SECRETS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn secrets_path() -> std::path::PathBuf {
    crate::db_path().with_file_name("relay-secrets.json")
}

fn read_secrets() -> SecretMap {
    read_secrets_from(&secrets_path())
}

fn read_secrets_from(path: &std::path::Path) -> SecretMap {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_secrets_to(path: &std::path::Path, secrets: &SecretMap) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string(secrets).map_err(|e| e.to_string())?;
    meowo_agent::fsutil::write_atomic_secure(path, &body).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn update_secret_at(path: &std::path::Path, agent: &str, secret: &str) -> Result<(), String> {
    // 必须把整个读-改-写包在同一把锁内；仅靠原子 rename 只能防半截文件，不能防两个调用方
    // 都基于旧快照写回而互相覆盖。
    let _guard = RELAY_SECRETS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut secrets = read_secrets_from(path);
    // 令牌里的零宽字符会原样进 Authorization 头，中转端一律 401，用户毫无线索——先洗掉。
    let cleaned = strip_invisible(secret);
    let value = cleaned.trim();
    if value.is_empty() {
        secrets.remove(agent);
    } else {
        secrets.insert(agent.to_string(), value.to_string());
    }
    write_secrets_to(path, &secrets)
}

fn has_secret(id: &str) -> bool {
    read_secrets().get(id).is_some_and(|s| !s.is_empty())
}

fn secret(id: meowo_agent::AgentId) -> Option<String> {
    read_secrets()
        .get(id.as_str())
        .filter(|s| !s.is_empty())
        .cloned()
}

// 三条密钥命令一律 async + spawn_blocking：同步命令跑在主线程，而这几条都要读写
// relay-secrets.json（set 还要解析安装形态）——文件被杀软/同步盘拖住时会冻住消息泵。
/// 返回当前安装形态支持中转的插件密钥状态。
#[tauri::command]
pub(crate) async fn get_relay_secret_status() -> Result<BTreeMap<String, bool>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let secrets = read_secrets();
        supported_relay_plugins()
            .map(|id| {
                let id = id.id().as_str();
                (
                    id.to_string(),
                    secrets.get(id).is_some_and(|s| !s.is_empty()),
                )
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// 用户明确要求在本机设置页查看已保存密钥；仅通过本地 Tauri IPC 返回，不进入 Settings/日志。
///
/// 安全评审（S3）结论：**保留明文返回**。设置页把它回填进可编辑输入框（失焦即整值保存），
/// 用户靠肉眼核对令牌排查 401——改成掩码不但砍掉该能力，掩码值还会在失焦时被原样存回、
/// 直接写坏密钥。外泄前提是渲染层已被攻破 + 存在出网信道；后者已由 CSP 收紧
/// （img-src 不再放行任意 https）切断。后续若要进一步收紧，应连同前端改成
/// 「不回填、留空表示不变」，单改后端做不到。
#[tauri::command]
pub(crate) async fn get_relay_secrets() -> Result<BTreeMap<String, String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let secrets = read_secrets();
        supported_relay_plugins()
            .map(|plugin| plugin.id().as_str())
            .filter_map(|id| secrets.get(id).map(|value| (id.to_string(), value.clone())))
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// 空串表示删除。密钥不写日志、不进入 Settings，也不通过事件广播。
#[tauri::command]
pub(crate) async fn set_relay_secret(agent: String, secret: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plugin = meowo_agent::by_id(&agent).ok_or_else(|| "未知 agent".to_string())?;
        let cap = plugin
            .relay()
            .ok_or_else(|| "该 agent 不支持 API 中转".to_string())?;
        let installation = plugin
            .resolve()
            .ok_or_else(|| "无法解析 agent 安装形态".to_string())?;
        if !cap.supports_variant(installation.variant_tag) {
            return Err("当前安装版本不支持 API 中转".into());
        }
        update_secret_at(&secrets_path(), &agent, &secret)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 渲染层要求的中转地址是否与已保存地址同源（scheme+host+port 全等）。
/// 任一侧解析失败（未保存过 / 非法 URL / 非 http 系的 opaque origin）一律 false——
/// 密钥宁可不发，也不发去没核对过的地方。路径允许不同：同源即同一中转服务。
fn same_relay_origin(saved: &str, requested: &str) -> bool {
    url::Url::parse(saved.trim())
        .ok()
        .zip(url::Url::parse(requested.trim()).ok())
        .is_some_and(|(saved, requested)| {
            // opaque origin（如 file:）互不相等，天然被拒；显式限定 http(s) 再加一道保险。
            matches!(saved.scheme(), "http" | "https") && saved.origin() == requested.origin()
        })
}

fn models_url(base_url: &str) -> Result<String, String> {
    validate_http_url(base_url)?;
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/models") {
        Ok(base.to_string())
    } else {
        Ok(format!("{base}/models"))
    }
}

fn parse_model_ids(body: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "中转返回的模型列表不是有效 JSON".to_string())?;
    let list = value
        .as_array()
        .or_else(|| value.get("data").and_then(|v| v.as_array()))
        .or_else(|| value.get("models").and_then(|v| v.as_array()))
        .ok_or_else(|| "中转返回的模型列表格式不受支持".to_string())?;

    let mut ids = BTreeSet::new();
    for item in list {
        let id = item
            .as_str()
            .or_else(|| item.get("id").and_then(|v| v.as_str()))
            .or_else(|| item.get("name").and_then(|v| v.as_str()));
        if let Some(id) = id.map(str::trim).filter(|id| !id.is_empty()) {
            ids.insert(id.to_string());
            if ids.len() >= 500 {
                break;
            }
        }
    }
    if ids.is_empty() {
        return Err("中转没有返回可用模型".into());
    }
    Ok(ids.into_iter().collect())
}

/// 使用已保存的中转密钥查询兼容 `/models` 端点。密钥只在后端组装请求，不返回前端。
///
/// base_url 由渲染层传入，必须与**已落盘**的该 agent 中转地址同源（scheme+host+port）：
/// 不校验的话，被攻破的渲染层可以把已存密钥打包发往任意 host（GET /models 即带
/// Authorization），等于一条现成的密钥外泄信道。合法流程不受影响——设置页的地址字段
/// 即改即存，前端发来的一定是已保存的 rule.base_url。同源不成立时直接拒绝，
/// 也不发无密钥的裸请求（那是免费的 SSRF 探测面）。
#[tauri::command]
pub(crate) async fn list_relay_models(
    agent: String,
    base_url: String,
    protocol: String,
    auth: String,
) -> Result<Vec<String>, String> {
    use meowo_agent::{Body, HttpError, HttpRequest};

    let id = crate::agent_id(&agent).ok_or_else(|| "未知 agent".to_string())?;
    let saved_base_url = crate::settings::load_settings()
        .relay
        .per_agent
        .get(id.as_str())
        .map(|rule| rule.base_url.clone())
        .unwrap_or_default();
    if !same_relay_origin(&saved_base_url, &base_url) {
        return Err("中转地址与已保存的中转配置不一致，请先保存中转地址".into());
    }
    let plugin = meowo_agent::by_id(&agent).ok_or_else(|| "未知 agent".to_string())?;
    let cap = plugin
        .relay()
        .ok_or_else(|| "该 agent 不支持 API 中转".to_string())?;
    let installation = plugin
        .resolve()
        .ok_or_else(|| "无法解析 agent 安装形态".to_string())?;
    let ui = cap.ui();
    let config = meowo_agent::RelayConfig {
        base_url: &base_url,
        model: "",
        protocol: if protocol.is_empty() {
            ui.default_protocol
        } else {
            &protocol
        },
        auth: if auth.is_empty() {
            ui.default_auth
        } else {
            &auth
        },
        // 拉模型列表不需要附加环境变量；它们只随会话启动注入。
        env_options: &[],
    };
    cap.validate(config, installation.variant_tag)?;
    let request = cap.model_request(config);
    let url = models_url(&base_url)?;
    let key = secret(id).ok_or_else(|| "请先保存中转密钥".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut headers = vec![("Accept", "application/json")];
        let authorization;
        if request.auth == meowo_agent::RelayModelAuth::ApiKey {
            headers.push(("x-api-key", key.as_str()));
        } else {
            authorization = format!("Bearer {key}");
            headers.push(("Authorization", authorization.as_str()));
        }
        if request.anthropic_version {
            headers.push(("anthropic-version", "2023-06-01"));
        }
        let ports = crate::ports::HostPorts::for_agent(id);
        let body = ports
            .as_ports()
            .http
            .send(&HttpRequest {
                method: "GET",
                url: &url,
                headers: &headers,
                body: Body::Empty,
                timeout: Duration::from_secs(15),
            })
            .map_err(|e| match e {
                HttpError::Status(code, _) => format!("中转返回 HTTP {code}"),
                HttpError::Transport(message) => format!("连接中转失败：{message}"),
            })?;
        parse_model_ids(&body)
    })
    .await
    .map_err(|e| format!("查询模型失败：{e}"))?
}

/// 合并到代理环境变量之后。返回值只交给终端启动器，不持久化、不打印。
pub(crate) fn launch_env(id: meowo_agent::AgentId) -> Vec<(String, String)> {
    let settings = crate::settings::load_settings();
    let Some(rule) = settings.relay.rule(id) else {
        return vec![];
    };
    let Some(key) = secret(id) else { return vec![] };
    meowo_agent::by_id(id.as_str())
        .and_then(|plugin| plugin.relay())
        .map_or_else(Vec::new, |cap| {
            cap.launch_env(relay_config_for(rule, cap), &key)
        })
}

/// Codex 用 CLI 的临时 `-c` 覆盖声明 provider，避免改写全局 config.toml。
/// Claude/Kimi 只需环境变量，argv 原样返回。
pub(crate) fn augment_argv(id: meowo_agent::AgentId, argv: Vec<String>) -> Vec<String> {
    let settings = crate::settings::load_settings();
    let Some(rule) = settings.relay.rule(id) else {
        return argv;
    };
    meowo_agent::by_id(id.as_str())
        .and_then(|plugin| plugin.relay())
        .map_or(argv.clone(), |cap| {
            cap.augment_argv(relay_config_for(rule, cap), has_secret(id.as_str()), argv)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_requires_http_and_host() {
        assert!(validate_http_url("https://relay.example/v1").is_ok());
        assert!(validate_http_url("http://127.0.0.1:4000").is_ok());
        assert!(validate_http_url("relay.example/v1").is_err());
        assert!(validate_http_url("socks5://relay.example").is_err());
    }

    /// 用户反馈：地址「明明是 https:// 开头」却报前缀错误——粘贴带入的零宽字符/全角冒号
    /// 在输入框里不可见。normalize 洗掉后应能通过校验并落盘干净值。
    #[test]
    fn pasted_invisible_and_fullwidth_chars_are_sanitized() {
        assert_eq!(
            sanitize_relay_field("\u{200B}https://relay.example/v1\u{FEFF}"),
            "https://relay.example/v1"
        );
        assert_eq!(
            sanitize_relay_field("https\u{FF1A}//relay.example"),
            "https://relay.example"
        );
        assert_eq!(sanitize_relay_field("\u{202A}model-x\u{202C}"), "model-x");
        let mut settings = RelaySettings::default();
        settings.per_agent.insert(
            "claude".into(),
            RelayRule {
                enabled: false,
                base_url: "\u{200B}https://relay.example/v1".into(),
                model: "\u{2060}model-x ".into(),
                ..RelayRule::default()
            },
        );
        settings.normalize();
        let rule = &settings.per_agent["claude"];
        assert_eq!(rule.base_url, "https://relay.example/v1");
        assert_eq!(rule.model, "model-x");
        assert!(validate_http_url(&rule.base_url).is_ok());
    }

    #[test]
    fn secret_invisible_chars_are_stripped_before_save() {
        let dir =
            std::env::temp_dir().join(format!("meowo-relay-secret-strip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("relay-secrets.json");
        update_secret_at(&path, "claude", "\u{FEFF}sk-abc\u{200B} ").unwrap();
        assert_eq!(
            read_secrets_from(&path).get("claude").map(String::as_str),
            Some("sk-abc")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S2：密钥只随「已保存中转配置的 origin」出门。host/端口/scheme 任一不同、
    /// 未保存过地址、任一侧非法都不放行；仅路径不同（同一服务的不同前缀）放行。
    #[test]
    fn relay_secret_only_travels_to_the_saved_origin() {
        assert!(same_relay_origin(
            "https://relay.example/v1",
            "https://relay.example/v1"
        ));
        assert!(same_relay_origin(
            "https://relay.example/v1",
            "https://relay.example/other/path"
        ));
        assert!(!same_relay_origin(
            "https://relay.example/v1",
            "https://evil.example/v1"
        ));
        assert!(!same_relay_origin(
            "https://relay.example/v1",
            "http://relay.example/v1" // 降级到明文也算换了地方
        ));
        assert!(!same_relay_origin(
            "https://relay.example/v1",
            "https://relay.example:8443/v1"
        ));
        assert!(!same_relay_origin("", "https://relay.example/v1"));
        assert!(!same_relay_origin("not-a-url", "https://relay.example/v1"));
        assert!(!same_relay_origin("file:///tmp/x", "file:///tmp/x"));
    }

    #[test]
    fn models_endpoint_and_common_response_shapes() {
        assert_eq!(
            models_url("https://relay.example/v1/").unwrap(),
            "https://relay.example/v1/models"
        );
        assert_eq!(
            models_url("https://relay.example/v1/models").unwrap(),
            "https://relay.example/v1/models"
        );
        assert_eq!(
            parse_model_ids(r#"{"data":[{"id":"z"},{"id":"a"}]}"#).unwrap(),
            vec!["a", "z"]
        );
        assert_eq!(
            parse_model_ids(r#"{"models":[{"name":"model-a"},"model-b"]}"#).unwrap(),
            vec!["model-a", "model-b"]
        );
        assert!(parse_model_ids(r#"{"data":[]}"#).is_err());
    }

    #[test]
    fn disabled_rules_do_not_require_fields() {
        let mut settings = RelaySettings::default();
        settings
            .per_agent
            .insert("claude".into(), RelayRule::default());
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn enabled_rule_without_secret_is_not_effective() {
        let mut settings = RelaySettings::default();
        settings.per_agent.insert(
            "claude".into(),
            RelayRule {
                enabled: true,
                ..RelayRule::default()
            },
        );
        assert!(settings
            .rule_with_secret(meowo_agent::AgentId::new("claude"), false)
            .is_none());
    }

    #[test]
    fn empty_protocol_and_auth_use_plugin_defaults() {
        let rule = RelayRule::default();
        let kimi = meowo_agent::by_id("kimi").unwrap().relay().unwrap();
        let config = relay_config_for(&rule, kimi);
        assert_eq!(config.protocol, "kimi");
        assert_eq!(config.auth, "bearer");
    }

    #[test]
    fn claude_header_kind_and_kimi_protocol_are_explicit() {
        let mut rule = RelayRule {
            enabled: true,
            base_url: "https://relay.example/v1/".into(),
            model: "model-x".into(),
            protocol: "anthropic".into(),
            auth: "api_key".into(),
            env_options: Vec::new(),
        };
        let claude = meowo_agent::by_id("claude").unwrap().relay().unwrap();
        assert_eq!(
            claude.launch_env(relay_config_for(&rule, claude), "secret"),
            vec![
                (
                    "ANTHROPIC_BASE_URL".into(),
                    "https://relay.example/v1".into()
                ),
                ("ANTHROPIC_API_KEY".into(), "secret".into()),
            ]
        );
        rule.auth = "bearer".into();
        assert_eq!(
            claude.launch_env(relay_config_for(&rule, claude), "secret")[1].0,
            "ANTHROPIC_AUTH_TOKEN"
        );
        let kimi = meowo_agent::by_id("kimi").unwrap().relay().unwrap();
        let kimi_env = kimi.launch_env(relay_config_for(&rule, kimi), "secret");
        assert!(kimi_env.contains(&("KIMI_MODEL_PROVIDER_TYPE".into(), "anthropic".into())));
        assert!(kimi_env.contains(&("KIMI_MODEL_NAME".into(), "model-x".into())));
    }

    /// claude 的两个附加环境变量：勾选才注入，未声明的 id 被忽略；其它 agent 没有可选项，
    /// 即使规则里带了 id（用户手工改过设置文件）也什么都不会注入。
    #[test]
    fn claude_env_options_are_injected_only_when_selected() {
        let mut rule = RelayRule {
            enabled: true,
            base_url: "https://relay.example/v1".into(),
            model: "model-x".into(),
            protocol: String::new(),
            auth: "bearer".into(),
            env_options: vec![
                "disable_nonessential_traffic".into(),
                "bogus_id".into(), // 未声明：validate 会拒，launch_env 也必须忽略
            ],
        };
        let claude = meowo_agent::by_id("claude").unwrap().relay().unwrap();
        // 未声明的 id 在保存校验阶段就被拦下。
        assert!(claude
            .validate(relay_config_for(&rule, claude), "stable")
            .is_err());
        let env = claude.launch_env(relay_config_for(&rule, claude), "secret");
        assert!(env.contains(&(
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into()
        )));
        assert!(!env
            .iter()
            .any(|(k, _)| k == "CLAUDE_CODE_ATTRIBUTION_HEADER"));
        assert!(!env.iter().any(|(k, _)| k.starts_with("BOGUS")));

        rule.env_options = vec!["no_attribution_header".into()];
        assert!(claude
            .validate(relay_config_for(&rule, claude), "stable")
            .is_ok());
        let env = claude.launch_env(relay_config_for(&rule, claude), "secret");
        assert!(env.contains(&("CLAUDE_CODE_ATTRIBUTION_HEADER".into(), "0".into())));
        assert!(!env
            .iter()
            .any(|(k, _)| k == "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"));

        let kimi = meowo_agent::by_id("kimi").unwrap().relay().unwrap();
        let kimi_env = kimi.launch_env(relay_config_for(&rule, kimi), "secret");
        assert!(!kimi_env.iter().any(|(k, _)| k.starts_with("CLAUDE_CODE_")));
    }

    #[test]
    fn codex_uses_ephemeral_responses_provider() {
        let rule = RelayRule {
            enabled: true,
            base_url: "https://relay.example/v1".into(),
            model: "gpt-relay".into(),
            protocol: String::new(),
            auth: "bearer".into(),
            env_options: Vec::new(),
        };
        let codex = meowo_agent::by_id("codex").unwrap().relay().unwrap();
        let argv = codex.augment_argv(relay_config_for(&rule, codex), true, vec!["codex".into()]);
        assert_eq!(argv[0], "codex");
        assert!(argv.iter().any(|a| a == "model_provider=\"meowo-relay\""));
        assert!(argv
            .iter()
            .any(|a| a == "model_providers.meowo-relay.wire_api=\"responses\""));
        assert!(argv.iter().any(|a| a == "model=\"gpt-relay\""));
    }

    #[test]
    fn concurrent_secret_updates_keep_every_agent_entry() {
        let dir = std::env::temp_dir().join(format!("meowo-relay-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = std::sync::Arc::new(dir.join("relay-secrets.json"));
        let mut writers = Vec::new();
        for index in 0..8 {
            let path = path.clone();
            writers.push(std::thread::spawn(move || {
                update_secret_at(&path, &format!("agent-{index}"), &format!("secret-{index}"))
                    .unwrap();
            }));
        }
        for writer in writers {
            writer.join().unwrap();
        }
        let saved = read_secrets_from(&path);
        for index in 0..8 {
            assert_eq!(
                saved.get(&format!("agent-{index}")).map(String::as_str),
                Some(format!("secret-{index}").as_str())
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
