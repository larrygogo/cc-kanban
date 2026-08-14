//! 出站代理的**声明与解析**：一条规则回答「这股流量走不走代理、走哪个」。
//!
//! 代理是 **per-agent** 的，这不是过度设计：`api.anthropic.com` 常需绕道，而 Kimi 是国内服务，
//! 把它也塞进境外代理只会更慢更不稳。每个调用点本来就知道自己在为哪个 agent 服务
//! （`account::usage_of(id)` / `install_agent(provider)`），于是 per-agent 覆盖能自然落地——
//! 也因此**不需要 `NO_PROXY` 那套按域名匹配的逻辑**：想让某个 agent 直连，把它设成 `off` 即可。
//!
//! 纯函数（规则 → 代理串），环境变量读取经参数注入，故可单测；真正拿它去建 HTTP 客户端的是
//! [`crate::ports`]。

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// 缺省模式：`system`（尊重环境变量）。
///
/// 刻意不是 `off`：ureq 的自由函数（改造前用的就是它）**完全无视** `HTTPS_PROXY`，于是「我明明
/// 配了代理，Meowo 为什么不认」成了此前的常态。默认跟随系统更贴近用户预期，也与 curl / git /
/// npm 的惯例一致。代价是环境里留着失效代理的机器会由直连变成走代理——这种机器上其它工具本来
/// 也是坏的，把它显式设成「直连」即可。
fn default_mode() -> String {
    "system".to_string()
}

/// 一条代理规则。`mode` 决定 `url` 是否被使用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProxyRule {
    /// `off`（直连） / `system`（读环境变量） / `custom`（用 `url`）。
    #[serde(default = "default_mode")]
    pub(crate) mode: String,
    /// `custom` 时的代理地址：`http://host:port` / `socks5://host:port`，可带 `user:pass@`；
    /// 也兼容代理供应商常给的 `host:port:user:pass`。
    #[serde(default)]
    pub(crate) url: String,
}

impl Default for ProxyRule {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            url: String::new(),
        }
    }
}

/// 全局默认规则 + 按 agent 的覆盖。`per_agent` 里没有的 agent 一律跟随全局。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProxySettings {
    #[serde(default = "default_mode")]
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) url: String,
    /// agent id（`claude` / `codex` / `kimi`）→ 覆盖规则。
    #[serde(default)]
    pub(crate) per_agent: BTreeMap<String, ProxyRule>,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            url: String::new(),
            per_agent: BTreeMap::new(),
        }
    }
}

impl ProxySettings {
    /// 落盘前清洗：粘贴带入的零宽/双向控制字符肉眼不可见，却会让「看起来完全正确」的
    /// 地址过不了校验（与中转地址同源的坑，helper 见 relay.rs）。只去不可见字符、掐头尾
    /// 空白，不折全角：代理 URL 可内嵌 user:pass 凭据，凭据字符不该被改写。
    pub(crate) fn normalize(&mut self) {
        let urls = std::iter::once(&mut self.url)
            .chain(self.per_agent.values_mut().map(|rule| &mut rule.url));
        for url in urls {
            *url = crate::relay::strip_invisible(url).trim().to_string();
        }
    }

    /// 某 agent 生效的规则：有覆盖用覆盖，否则用全局。
    fn rule_for(&self, agent: Option<&str>) -> ProxyRule {
        agent
            .and_then(|id| self.per_agent.get(id))
            .cloned()
            .unwrap_or_else(|| ProxyRule {
                mode: self.mode.clone(),
                url: self.url.clone(),
            })
    }

    /// 某 agent 生效的代理串；`None` = 直连。
    /// `agent == None` 用全局规则——供与具体 agent 无关的流量（自更新）使用。
    pub(crate) fn resolve(&self, agent: Option<&str>) -> Option<String> {
        resolve_rule(&self.rule_for(agent), env_proxy)
    }

    /// 落盘前校验：`custom` 必须给出 ureq 能解析的地址。
    ///
    /// 非法值一旦落盘，后台只会静默降级直连，用户对着「用量查不到」毫无线索——所以在这里
    /// 拦下并回传具体原因。
    pub(crate) fn validate(&self) -> Result<(), String> {
        validate_rule(&self.mode, &self.url)?;
        for (id, r) in &self.per_agent {
            validate_rule(&r.mode, &r.url).map_err(|e| format!("{id}：{e}"))?;
            // 针对某个 agent **显式**配的代理，还要过它自己的能力关：给 claude 填 socks5 是无效的
            // （官方不支持），静默放行的后果是它既不报错也不走代理，用户完全无从排查。
            //
            // 只查 custom：`system` 模式下环境变量里是什么，保存时无从预知，也不该因此拒绝保存。
            // 全局规则同理不在此拒绝——全局设成 socks 对 kimi 和 Meowo 自身仍然有效，
            // 不支持的 agent 由 apply_to_agent_configs 回传 `unsupported` 提示，而不是拦下整次保存。
            if r.mode == "custom" {
                if let Some(spec) = meowo_agent::by_id(id).and_then(|p| p.proxy()) {
                    let name = meowo_agent::by_id(id).map_or(id.as_str(), |p| p.display_name());
                    spec.accepts(r.url.trim())
                        .map_err(|e| format!("{name}：{e}"))?;
                }
            }
        }
        Ok(())
    }
}

fn validate_rule(mode: &str, url: &str) -> Result<(), String> {
    if mode != "custom" {
        return Ok(()); // off/system 不看 url，留着旧值无害
    }
    let u = url.trim();
    if u.is_empty() {
        return Err("已选「自定义代理」，但代理地址为空".into());
    }
    validate_url(u)
}

/// ureq **实际**支持的代理协议。SOCKS 系需要 `socks-proxy` feature（见 Cargo.toml），已开启。
///
/// 刻意不含 `socks5h` / `https`：ureq 解析不了它们（会报 Malformed proxy）。与其让用户填进去、
/// 保存成功、再在后台静默失败，不如在设置页当场告诉他可用的是哪几个。
const SCHEMES: [&str; 4] = ["http", "socks4", "socks4a", "socks5"];

/// 校验代理地址：`[协议://][用户:密码@]主机[:端口]`。
///
/// **不能只靠 `ureq::Proxy::new`**：它过于宽松——`"not a url"` 会被它当成主机名而返回 `Ok`，
/// 于是用户在设置页填了句废话，保存成功，然后对着「用量查不到」抓瞎。这里先自己把关，
/// 最后再交给 ureq 兜底。
pub(crate) fn validate_url(url: &str) -> Result<(), String> {
    let normalized = normalize_proxy_url(url)?;
    let u = normalized.as_ref();
    if u.is_empty() {
        return Err("代理地址为空".into());
    }
    if u.chars().any(char::is_whitespace) {
        return Err("代理地址不能含空格".into());
    }

    // 协议：缺省视为 http（与 curl 一致）。
    let (scheme, rest) = match u.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_string(), u),
    };
    if !SCHEMES.contains(&scheme.as_str()) {
        return Err(format!(
            "不支持的代理协议「{scheme}」，可用：{}",
            SCHEMES.join(" / ")
        ));
    }

    // 去掉 user:pass@ 前缀（密码里可能有 @，取最后一个）。
    let hostport = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    let (host, port) = split_host_port(hostport);
    if host.is_empty() {
        return Err("代理地址缺少主机".into());
    }
    if let Some(p) = port {
        if p.parse::<u16>().is_err() {
            return Err(format!("代理端口无效：「{p}」"));
        }
    }

    ureq::Proxy::new(u)
        .map(|_| ())
        .map_err(|e| format!("代理地址无效（{e}）"))
}

/// 把代理供应商常见的 `host:port:user:pass` 规范化为标准 HTTP 代理 URL。
///
/// 密码允许包含额外的 `:`（只切前三个分隔符）；用户名和密码会做 URL 编码，避免其中的 `@`、
/// `/` 等字符改变 URL 结构。已有协议、标准 `user:pass@host` 写法及普通 `host:port` 原样保留。
fn normalize_proxy_url(url: &str) -> Result<Cow<'_, str>, String> {
    let u = url.trim();
    // 只有当 `://` 之前是纯 scheme（不含 `:`）时才认作「已带协议」原样保留。否则像
    // `host:port:user:pa://ss`（口令里恰好含 `://`）会被误判成带 scheme 而跳过规范化，
    // 结构分隔符不转义、后续校验/连接异常。
    if let Some(idx) = u.find("://") {
        if !u[..idx].contains(':') {
            return Ok(Cow::Borrowed(u));
        }
    }

    let (host, tail) = if let Some(rest) = u.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Ok(Cow::Borrowed(u));
        };
        let end = end + 1;
        let Some(tail) = u[end + 1..].strip_prefix(':') else {
            return Ok(Cow::Borrowed(u));
        };
        (&u[..=end], tail)
    } else {
        let Some((host, tail)) = u.split_once(':') else {
            return Ok(Cow::Borrowed(u));
        };
        (host, tail)
    };

    let mut fields = tail.splitn(3, ':');
    let (Some(port), Some(user), Some(pass)) = (fields.next(), fields.next(), fields.next()) else {
        return Ok(Cow::Borrowed(u));
    };
    if host.is_empty() || port.is_empty() || user.is_empty() || pass.is_empty() {
        return Err("host:port:user:pass 四段都不能为空".into());
    }

    // RFC 3986 unreserved 字符（ALPHA / DIGIT / "-._~"）在 URI 中本来就安全，必须原样保留。
    // `NON_ALPHANUMERIC` 会把 `-` / `_` 也写成 `%2D` / `%5F`；虽然标准上等价，但部分代理
    // 客户端会直接拿 URL parser 返回的 userinfo 做 Basic Auth，不主动 percent-decode，导致认证失败。
    // 其余非字母数字字符仍编码，避免 `@` / `:` / `/` 等改变 URL 结构。
    const PROXY_CREDENTIAL_ENCODE_SET: &percent_encoding::AsciiSet =
        &percent_encoding::NON_ALPHANUMERIC
            .remove(b'-')
            .remove(b'.')
            .remove(b'_')
            .remove(b'~');
    let user = percent_encoding::utf8_percent_encode(user, PROXY_CREDENTIAL_ENCODE_SET);
    let pass = percent_encoding::utf8_percent_encode(pass, PROXY_CREDENTIAL_ENCODE_SET);
    Ok(Cow::Owned(format!("http://{user}:{pass}@{host}:{port}")))
}

/// ureq 2.x 自己用字符串切分代理地址，不会按 URL 规则解码 userinfo。其它消费者（reqwest、
/// 模型 CLI）需要标准的百分号编码，因此只在交给 ureq 的最后一刻把用户名和密码还原。
pub(crate) fn ureq_compatible_url(url: &str) -> Cow<'_, str> {
    let (prefix, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), url),
    };
    let Some((credentials, hostport)) = rest.rsplit_once('@') else {
        return Cow::Borrowed(url);
    };
    let Some((user, pass)) = credentials.split_once(':') else {
        return Cow::Borrowed(url);
    };
    if !credentials.contains('%') {
        return Cow::Borrowed(url);
    }
    let user = percent_encoding::percent_decode_str(user).decode_utf8_lossy();
    let pass = percent_encoding::percent_decode_str(pass).decode_utf8_lossy();
    Cow::Owned(format!("{prefix}{user}:{pass}@{hostport}"))
}

/// 拆 `host[:port]`。IPv6 字面量按 `[::1]:1080` 形态处理——否则末位冒号会切进地址内部。
fn split_host_port(hostport: &str) -> (&str, Option<&str>) {
    if let Some(end) = hostport.rfind(']') {
        return (&hostport[..=end], hostport[end + 1..].strip_prefix(':'));
    }
    match hostport.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (hostport, None),
    }
}

// ═══ 写进 agent 自己的配置文件 ═══
//
// 目前只有 claude 能这么干（settings.json 的 `env` 块，官方定义为「作用于每个会话」）——也只有它
// 能做到「用户自己在终端敲 claude 也走代理」。codex / kimi 的配置文件没有这个能力，只能靠进程
// 环境变量，见 terminal 注入与用户级环境变量两条路。差异声明在 `meowo_agent::proxy::ProxySpec`。

/// 单个 agent 的应用结果，回传设置页。
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct AgentProxyReport {
    pub agent: String,
    /// 用户自己在配置里设过、我们**没有覆盖**的键。非空时 UI 必须提示——静默跳过会让用户
    /// 以为代理已生效，实际走的是他自己那个值。
    pub skipped: Vec<String>,
    /// 该 agent 用不了这个代理的原因（如给 claude 配了 SOCKS）。
    pub unsupported: Option<String>,
    /// 写入失败的原因（配置不可读 / 磁盘写不进）。
    pub error: Option<String>,
}

/// 拉起该 agent 时要注入终端的代理环境变量。空 = 不注入。
///
/// **能写进自己配置文件的 agent（claude）返回空**：它的代理已经在 settings.json 的 `env` 块里，
/// 对所有启动方式都生效（包括用户自己开的终端）。再注入一遍进程环境只会多出第二个真相来源，
/// 还会与「用户在 env 块里手设了 HTTPS_PROXY、我们没敢覆盖」的情形语义打架。一个 agent 一种机制。
///
/// codex / kimi 没有配置文件这条路（见 [`meowo_agent::proxy`] 的能力表），进程环境变量是唯一手段——
/// 于是只覆盖得到 **Meowo 自己拉起**的会话；用户在自己终端里敲的，得靠用户级环境变量。
/// ⚠️ **这只是代理变量，不含账号（profile）隔离变量。**
///
/// 拉起 agent 时**不要直接用它**——请用 [`crate::terminal::launch_env_for_profile`]（新建会话）
/// 或 [`crate::terminal::launch_env_for_session`]（恢复会话），它们会在代理之上补上
/// `CLAUDE_CONFIG_DIR` 之类的账号隔离变量。
///
/// `new_session` 曾直接调这里，后果是**多账号完全不生效**：设置页明明切到了另一个账号，新开的
/// 会话却仍跑在默认账号上，而且毫无迹象——用户只能靠 `/status` 里的邮箱才发现。
///
/// 唯一该直连它的是 `login_agent`：登录要写进**指定** profile 的目录（而不是当前活跃的那个），
/// 故它自己 extend 一份 `profile::env_of(目标 profile)`。
pub fn launch_env(id: meowo_agent::AgentId) -> Vec<(String, String)> {
    let can_config = meowo_agent::by_id(id.as_str())
        .and_then(|p| p.proxy())
        .is_some_and(|s| s.config_env);
    if can_config {
        return vec![];
    }
    proxy_env_of(id)
}

/// 跑**安装脚本**时要注入的代理环境变量。
///
/// 与 [`launch_env`] 的差别：**claude 也要**。安装脚本是 shell 里的 `curl` / `irm` 在下载，
/// 它当然不读 claude 的 `settings.json`——那个 `env` 块只作用于 claude 进程自己。
pub fn launch_env_for_install(id: meowo_agent::AgentId) -> Vec<(String, String)> {
    proxy_env_of(id)
}

/// 该 agent 生效的代理 → 它认得的环境变量。
fn proxy_env_of(id: meowo_agent::AgentId) -> Vec<(String, String)> {
    let Some(spec) = meowo_agent::by_id(id.as_str()).and_then(|p| p.proxy()) else {
        return vec![];
    };
    let Some(url) = crate::settings::load_settings()
        .proxy
        .resolve(Some(id.as_str()))
    else {
        return vec![];
    };
    // 不支持的形态（如给 codex 配 socks）→ env_for 返回空，绝不塞一个它不认识的串。
    spec.env_for(&url)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

/// 我们上次写进各 agent 配置的键值：`{"claude": {"HTTPS_PROXY": "..."}}`。
///
/// **认领的唯一依据**。agent 的 `env` 是扁平 map，用户完全可能自己在里面设了 `HTTPS_PROXY`
/// （企业代理很常见）。没有这份记录就分不清「我上次写的」与「用户自己写的」——于是要么不敢关代理，
/// 要么把用户的配置覆盖掉。与 `~/.meowo/imported.json` 同类，是状态而非配置，故不进 settings.json。
type AppliedMap = std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>;

fn applied_path() -> std::path::PathBuf {
    crate::db_path().with_file_name("proxy-applied.json")
}

fn read_applied() -> AppliedMap {
    std::fs::read_to_string(applied_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_applied(m: &AppliedMap) -> Result<(), String> {
    let path = applied_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建代理状态目录失败：{e}"))?;
    }
    let s =
        serde_json::to_string_pretty(m).map_err(|e| format!("序列化代理所有权状态失败：{e}"))?;
    meowo_agent::fsutil::write_atomic(&path, &s)
        .map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

// 关于「让你自己在终端里敲的 codex / kimi 也走代理」：**做不到，且刻意不做。**
//
// 唯一的手段是把 HTTPS_PROXY 写进用户级环境变量（Windows 注册表 / shell profile），但系统里
// 只有一份，于是三家必须共用同一个代理——per-agent 隔离当场作废（「Claude 走境外代理、Kimi 直连」
// 正是最常见的配法）。它还会波及系统上每一个新开的程序，带认证的代理连账号密码一起落进注册表。
//
// 代价远大于收益，故这条路不走：Meowo 只管**自己拉起**的会话（`launch_env` 给进程注入各自的代理），
// claude 另外还能写进它自己的配置文件（`apply_to_agent_configs`）。设置页必须如实标注这个覆盖面，
// 不许含糊——见 `NetworkSection.tsx` 的 coverage 文案。

/// 串行化 [`apply_to_agent_configs`] 的进程内锁（理由见函数首行注释）。
static APPLY_AGENT_CONFIGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 把当前代理设置写进各 agent **自己的**配置文件。启动时与设置保存后各跑一次。
///
/// 逐 agent best-effort：一家失败不影响他家（与 hooks 接线同纪律）。未配置过的 agent
/// （数据目录不存在＝没装）跳过，绝不凭空创建它的配置目录。
pub fn apply_to_agent_configs() -> Vec<AgentProxyReport> {
    // 整个读-改-写（各 agent 配置文件 + proxy-applied.json 所有权状态）都基于先前快照：
    // 启动后台线程（setup）与设置保存命令（主线程）并发跑时，旧快照写回会把另一方刚生效
    // 的修改打掉（典型：启动后几秒内保存代理设置）。与 RELAY_SECRETS_LOCK / USAGE_CACHE_LOCK
    // 同一模式，用进程内 Mutex 串行化。
    let _guard = APPLY_AGENT_CONFIGS_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let settings = crate::settings::load_settings();
    let mut applied = read_applied();
    let mut reports = Vec::new();
    // 若最后的所有权状态写不下来，必须把本轮对 agent 配置的修改回滚；否则下次无法证明
    // 哪些代理键是我们写的，用户会得到一个再也关不掉的代理。
    let mut rollbacks: Vec<(String, std::path::PathBuf, String, bool)> = Vec::new();

    for p in meowo_agent::all() {
        let id = p.id();
        let Some(spec) = p.proxy() else { continue };
        // 只有能写进自己配置文件的 agent 走这条路（目前仅 claude）。
        if !spec.config_env {
            continue;
        }
        if !p.is_configured() {
            continue; // 没装过：绝不凭空创建它的配置
        }
        let Some(inst) = p.resolve() else { continue };
        let path = inst.config_path();

        let mut report = AgentProxyReport {
            agent: id.as_str().to_string(),
            skipped: vec![],
            unsupported: None,
            error: None,
        };

        // 该 agent 生效的代理（per_agent 覆盖 → 全局 → 环境变量）。None = 直连 → 清掉我们写过的键。
        let proxy = settings.proxy.resolve(Some(id.as_str()));
        let desired: Vec<(&'static str, String)> = match proxy.as_deref() {
            None => vec![],
            Some(u) => match spec.accepts(u) {
                Ok(()) => spec.env_for(u),
                // 如给 claude 配了 SOCKS：一个键都不写（写进去它也不认），并如实告知。
                // 这里不当作错误——全局设成 socks 对 kimi 和 Meowo 自身仍是有效的。
                Err(why) => {
                    report.unsupported = Some(why);
                    vec![]
                }
            },
        };

        let owned = applied.get(id.as_str()).cloned().unwrap_or_default();
        let all_keys = spec.all_keys();

        let existed = path.exists();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            // 配置文件还没生成（装了但没跑过）：没有代理要写就无事可做；要写则从空对象起。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if desired.is_empty() {
                    reports.push(report);
                    continue;
                }
                "{}".to_string()
            }
            Err(e) => {
                report.error = Some(format!("读取 {} 失败：{e}", path.display()));
                reports.push(report);
                continue;
            }
        };

        let plan = meowo_agent::proxy::ensure_env(&text, &desired, &owned, &all_keys);
        report.skipped = plan.skipped.iter().map(|s| s.to_string()).collect();

        match plan.outcome {
            meowo_agent::EnsureOutcome::Unchanged => {}
            meowo_agent::EnsureOutcome::Abandon(reason) => {
                report.error = Some(format!(
                    "{} 形态无法安全改写（{reason:?}），已放弃",
                    path.display()
                ));
                reports.push(report);
                continue;
            }
            meowo_agent::EnsureOutcome::Changed(next) => {
                // 写前必备份，与 hooks 接线同纪律：备份失败就放弃本次写入——吞掉错误照常
                // 落盘，等于拿用户的 settings.json 去赌「也许不需要回滚」。
                if path.exists() {
                    if let Err(e) = meowo_agent::backup_once(&path) {
                        report.error = Some(format!("备份 {} 失败：{e}", path.display()));
                        reports.push(report);
                        continue;
                    }
                }
                if let Err(e) = meowo_agent::fsutil::write_atomic(&path, &next) {
                    report.error = Some(format!("写入 {} 失败：{e}", path.display()));
                    reports.push(report);
                    continue;
                }
                rollbacks.push((id.as_str().to_string(), path.clone(), text.clone(), existed));
                eprintln!("Meowo proxy[{id}]: 已写入 {}", path.display());
            }
        }

        // 记下本次写下的键值，作为下次的认领依据。跳过的键不算我们的，不记。
        let mine: std::collections::BTreeMap<String, String> = desired
            .iter()
            .filter(|(k, _)| !plan.skipped.contains(k))
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        if mine.is_empty() {
            applied.remove(id.as_str());
        } else {
            applied.insert(id.as_str().to_string(), mine);
        }
        reports.push(report);
    }

    if let Err(state_err) = write_applied(&applied) {
        let had_config_changes = !rollbacks.is_empty();
        for (agent, path, original, existed) in rollbacks.into_iter().rev() {
            let rollback = if existed {
                meowo_agent::fsutil::write_atomic(&path, &original)
            } else {
                std::fs::remove_file(&path).or_else(|e| {
                    (e.kind() == std::io::ErrorKind::NotFound)
                        .then_some(())
                        .ok_or(e)
                })
            };
            let detail = match rollback {
                Ok(()) => format!("{state_err}；已回滚本次代理配置修改"),
                Err(e) => format!("{state_err}；且回滚 {} 失败：{e}", path.display()),
            };
            if let Some(report) = reports.iter_mut().find(|r| r.agent == agent) {
                report.error = Some(detail);
            }
        }
        // 即使本轮没有实际配置改动，也不能把状态持久化失败静默吞掉。
        if !had_config_changes {
            if let Some(report) = reports.first_mut() {
                report.error = Some(state_err);
            } else {
                reports.push(AgentProxyReport {
                    agent: "meowo".into(),
                    skipped: vec![],
                    unsupported: None,
                    error: Some(state_err),
                });
            }
        }
    }
    reports
}

/// 规则 → 生效代理串。`env` 注入便于单测，不去动进程环境。
fn resolve_rule(rule: &ProxyRule, env: impl Fn() -> Option<String>) -> Option<String> {
    let raw = match rule.mode.as_str() {
        "off" => None,
        "custom" => non_empty(&rule.url),
        // "system" 与任何未知值（老配置/手改坏）都按 system 处理：回退环境变量比直连更接近用户预期。
        _ => env(),
    };
    raw.map(|u| normalize_proxy_url(&u).map(Cow::into_owned).unwrap_or(u))
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// 按 curl / git 的惯例读环境变量：HTTPS 优先（出站全是 https），再 ALL，最后 HTTP。
/// 大小写变体都认——Windows 上多为大写，Unix 上小写常见。
fn env_proxy() -> Option<String> {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .iter()
    .find_map(|k| std::env::var(k).ok().and_then(|v| non_empty(&v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(mode: &str, url: &str) -> ProxyRule {
        ProxyRule {
            mode: mode.into(),
            url: url.into(),
        }
    }
    fn no_env() -> Option<String> {
        None
    }
    fn some_env() -> Option<String> {
        Some("http://env:1080".into())
    }

    #[test]
    fn off_never_proxies_even_with_env() {
        // 关键：off 必须无视环境变量——「国内 agent 直连」正是靠它，若被 env 兜底就白设了。
        assert_eq!(resolve_rule(&rule("off", ""), some_env), None);
        assert_eq!(resolve_rule(&rule("off", "http://x:1"), some_env), None);
    }

    #[test]
    fn custom_uses_url_and_trims() {
        assert_eq!(
            resolve_rule(&rule("custom", " socks5://127.0.0.1:1080 "), some_env).as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
        assert_eq!(
            resolve_rule(&rule("custom", "proxy.example:8080:alice:secret"), some_env).as_deref(),
            Some("http://alice:secret@proxy.example:8080")
        );
        // custom 但地址为空 → 直连（不偷偷回退到 env，那会让 UI 显示的和实际走的不一致）。
        assert_eq!(resolve_rule(&rule("custom", "  "), some_env), None);
    }

    #[test]
    fn system_reads_env_and_unknown_mode_degrades_to_system() {
        assert_eq!(
            resolve_rule(&rule("system", ""), some_env).as_deref(),
            Some("http://env:1080")
        );
        assert_eq!(
            resolve_rule(&rule("system", ""), || Some(
                "proxy.example:8080:bob:p@ss:word".into()
            ))
            .as_deref(),
            Some("http://bob:p%40ss%3Aword@proxy.example:8080")
        );
        assert_eq!(resolve_rule(&rule("system", ""), no_env), None);
        // 手改坏的 mode 按 system 处理，不 panic、不静默直连。
        assert_eq!(
            resolve_rule(&rule("banana", ""), some_env).as_deref(),
            Some("http://env:1080")
        );
    }

    #[test]
    fn per_agent_overrides_global_others_follow() {
        let mut s = ProxySettings {
            mode: "custom".into(),
            url: "http://g:1".into(),
            per_agent: BTreeMap::new(),
        };
        s.per_agent.insert("kimi".into(), rule("off", ""));
        s.per_agent
            .insert("claude".into(), rule("custom", "socks5://c:2"));
        // kimi 直连、claude 走自己的、codex 未覆盖 → 跟随全局。
        assert_eq!(s.resolve(Some("kimi")), None);
        assert_eq!(s.resolve(Some("claude")).as_deref(), Some("socks5://c:2"));
        assert_eq!(s.resolve(Some("codex")).as_deref(), Some("http://g:1"));
        // 与 agent 无关的流量（自更新）用全局。
        assert_eq!(s.resolve(None).as_deref(), Some("http://g:1"));
    }

    #[test]
    fn default_is_system_and_old_settings_deserialize() {
        assert_eq!(ProxySettings::default().mode, "system");
        // 老 settings.json 没有 proxy 段 / 只有半截字段 → serde 补默认，不 panic。
        let s: ProxySettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.mode, "system");
        assert!(s.per_agent.is_empty());
        let s: ProxySettings =
            serde_json::from_str(r#"{"per_agent":{"kimi":{"mode":"off"}}}"#).unwrap();
        assert_eq!(s.per_agent["kimi"].mode, "off");
        assert_eq!(s.mode, "system");
    }

    /// 与中转地址同源的坑：粘贴带入的零宽字符让「看起来完全正确」的代理地址过不了校验。
    /// normalize 洗掉后应能通过，且全局与 per_agent 覆盖都要洗到。
    #[test]
    fn pasted_invisible_chars_in_proxy_urls_are_stripped() {
        let mut settings = ProxySettings {
            mode: "custom".into(),
            url: "\u{200B}http://127.0.0.1:7890 ".into(),
            per_agent: BTreeMap::from([(
                "kimi".into(),
                rule("custom", "\u{FEFF}http://127.0.0.1:1080"),
            )]),
        };
        assert!(settings.validate().is_err(), "洗之前应被校验拦下");
        settings.normalize();
        assert_eq!(settings.url, "http://127.0.0.1:7890");
        assert_eq!(settings.per_agent["kimi"].url, "http://127.0.0.1:1080");
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_custom_and_allows_others() {
        let bad = ProxySettings {
            mode: "custom".into(),
            url: "".into(),
            per_agent: BTreeMap::new(),
        };
        assert!(bad.validate().is_err(), "custom 且地址为空应报错");

        let mut s = ProxySettings::default();
        s.per_agent
            .insert("claude".into(), rule("custom", "not a url"));
        let err = s.validate().unwrap_err();
        assert!(err.contains("claude"), "错误须指出是哪个 agent：{err}");

        // off/system 不看 url；合法 custom 通过。
        let mut ok = ProxySettings {
            mode: "off".into(),
            url: "garbage".into(),
            per_agent: BTreeMap::new(),
        };
        ok.per_agent
            .insert("claude".into(), rule("custom", "http://127.0.0.1:7890"));
        ok.per_agent.insert("kimi".into(), rule("off", ""));
        assert!(ok.validate().is_ok());
    }

    /// 给 claude 显式配 SOCKS 必须**保存时**就报错。放行的话它既不报错也不走代理，
    /// 用户对着「claude 连不上」毫无线索——这正是最该在 UI 上拦下的一类错。
    #[test]
    fn per_agent_socks_is_rejected_for_agents_that_cannot_use_it() {
        let mut s = ProxySettings::default();
        s.per_agent
            .insert("claude".into(), rule("custom", "socks5://127.0.0.1:1080"));
        let err = s.validate().unwrap_err();
        assert!(err.contains("Claude Code"), "错误须指名是哪个模型：{err}");
        assert!(err.contains("SOCKS"), "错误须点明是 SOCKS 的问题：{err}");

        // codex 同样不支持。
        let mut s = ProxySettings::default();
        s.per_agent
            .insert("codex".into(), rule("custom", "socks5://127.0.0.1:1080"));
        assert!(s.validate().is_err());

        // kimi 支持 SOCKS → 放行。
        let mut s = ProxySettings::default();
        s.per_agent
            .insert("kimi".into(), rule("custom", "socks5://127.0.0.1:1080"));
        assert!(s.validate().is_ok(), "kimi 支持 socks，不该被拒");

        // http 代理对三家都合法。
        let mut s = ProxySettings::default();
        for id in ["claude", "codex", "kimi"] {
            s.per_agent
                .insert(id.into(), rule("custom", "http://127.0.0.1:7890"));
        }
        assert!(s.validate().is_ok());
    }

    /// 全局设成 socks **不**拦：它对 kimi 和 Meowo 自身仍然有效。不支持的 agent 由
    /// apply_to_agent_configs 回传 unsupported 提示，而不是拦下整次保存。
    #[test]
    fn global_socks_is_allowed_and_reported_later_not_blocked() {
        let s = ProxySettings {
            mode: "custom".into(),
            url: "socks5://127.0.0.1:1080".into(),
            per_agent: BTreeMap::new(),
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_url_accepts_common_forms() {
        for u in [
            "http://127.0.0.1:7890",   // Clash 默认 HTTP 口
            "socks5://127.0.0.1:1080", // Clash / V2Ray 默认 SOCKS 口
            "socks4a://127.0.0.1:1080",
            "http://user:pass@host:3128", // 企业带认证代理
            "host:3128:user:pass",        // 代理商常见的四段简写
            "[::1]:1080:user:pass",       // 四段简写也支持带方括号的 IPv6
            "host:3128",                  // 省略协议 → 视为 http
            "http://[::1]:1080",          // IPv6 字面量
        ] {
            assert!(
                validate_url(u).is_ok(),
                "{u} 应被接受：{:?}",
                validate_url(u)
            );
        }
    }

    /// ureq::Proxy::new 太宽松（`"not a url"` 会被它当主机名而放行），故这些必须由我们拦下——
    /// 否则用户填了句废话也能保存成功，然后对着「用量查不到」抓瞎。
    #[test]
    fn validate_url_rejects_garbage() {
        for (u, why) in [
            ("not a url", "含空格"),
            ("ftp://h:1", "不支持的协议"),
            // ureq 解析不了这两个：宁可在设置页当场报错，也不要保存成功后在后台静默失败。
            ("socks5h://127.0.0.1:1080", "ureq 不支持 socks5h"),
            ("https://127.0.0.1:7890", "ureq 不支持 https 代理"),
            ("http://:8080", "缺主机"),
            ("http://host:99999", "端口越界"),
            ("http://host:abc", "端口非数字"),
            ("host:8080::pass", "四段简写缺用户名"),
            ("host:8080:user:", "四段简写缺密码"),
            ("", "空"),
        ] {
            assert!(validate_url(u).is_err(), "「{u}」应被拒（{why}）");
        }
    }

    #[test]
    fn four_part_proxy_is_canonicalized_and_credentials_are_encoded() {
        assert_eq!(
            normalize_proxy_url("proxy.example:8080:user-name_test~v1@example:p@ss/word:tail")
                .unwrap(),
            "http://user-name_test~v1%40example:p%40ss%2Fword%3Atail@proxy.example:8080"
        );
        assert_eq!(
            normalize_proxy_url("[2001:db8::1]:1080:user:pass").unwrap(),
            "http://user:pass@[2001:db8::1]:1080"
        );
        assert_eq!(normalize_proxy_url("host:8080").unwrap(), "host:8080");
        assert_eq!(
            normalize_proxy_url("http://user:pass@host:8080").unwrap(),
            "http://user:pass@host:8080"
        );
        // 口令里恰好含 `://` 不能被误判成「已带协议」而跳过规范化——`://` 前有 `:`，非 scheme。
        assert_eq!(
            normalize_proxy_url("proxy.example:8080:user:pa://ss").unwrap(),
            "http://user:pa%3A%2F%2Fss@proxy.example:8080"
        );
        assert_eq!(
            ureq_compatible_url("http://user%40example:p%40ss%2Fword%3Atail@proxy.example:8080"),
            "http://user@example:p@ss/word:tail@proxy.example:8080"
        );
    }
}
