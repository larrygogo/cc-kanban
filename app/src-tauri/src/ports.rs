//! `meowo_agent::ports` 的宿主实现：把插件层声明的外部能力接到真实世界。
//!
//! 插件层只认 trait，故 HTTP 栈（ureq）与 macOS `security` 子进程都止步于此文件——插件的单测
//! 注入假实现即可，不需要真网络、也不需要 `#[cfg(target_os)]`。
//!
//! 代理也止步于此：出站请求走不走代理由 [`crate::proxy`] 按 agent 解析，插件层无感。

use meowo_agent::ports::{Body, HttpError, HttpPort, HttpRequest, KeychainPort};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

// ═══ HTTP ═══

/// 按「代理串」缓存 `ureq::Agent`（`""` = 直连）。
///
/// 从前用的是 `ureq::get()` 这类自由函数，它绑的是 ureq 内置默认 Agent——**既不走代理，也不读
/// `HTTPS_PROXY`**。要挂代理就必须自己持有 Agent。
///
/// `ureq::Agent` 内部是 `Arc`，clone 便宜且共享连接池，故按代理串缓存即可让同代理的请求复用连接。
/// 设置改了 → 解析出的代理串变了 → 自然落到新 key，无需任何失效逻辑。
fn agent_for(proxy: Option<&str>) -> ureq::Agent {
    static CACHE: OnceLock<RwLock<HashMap<String, ureq::Agent>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    let key = proxy.unwrap_or_default().to_string();

    if let Some(a) = cache.read().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return a.clone();
    }
    let mut b = ureq::AgentBuilder::new();
    if let Some(p) = proxy {
        let compatible = crate::proxy::ureq_compatible_url(p);
        match ureq::Proxy::new(compatible.as_ref()) {
            Ok(px) => b = b.proxy(px),
            // 落盘前 set_settings 已校验过；能走到这儿多半是用户手改了 settings.json。
            // 降级直连并留日志——总好过让用量查询与安装整条挂掉。
            // 代理地址可带 user:pass@,原样打日志等于把凭据写进控制台/被收集的日志里。
            Err(e) => eprintln!(
                "[proxy] 代理地址无效，本次直连（{}）：{e}",
                crate::proxy::redact_credentials(p)
            ),
        }
    }
    let agent = b.build();
    cache
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, agent.clone());
    agent
}

/// 绑定了某个代理配置的 HTTP 端口。经 [`HostPorts`] 构造，不直接 new。
pub struct UreqHttp {
    /// 已解析好的代理串；None = 直连。
    proxy: Option<String>,
}

impl UreqHttp {
    fn agent(&self) -> ureq::Agent {
        agent_for(self.proxy.as_deref())
    }
}

impl HttpPort for UreqHttp {
    fn send(&self, req: &HttpRequest) -> Result<String, HttpError> {
        let agent = self.agent();
        let mut r = match req.method {
            "POST" => agent.post(req.url),
            _ => agent.get(req.url),
        }
        .timeout(req.timeout);
        for (k, v) in req.headers {
            r = r.set(k, v);
        }
        let resp = match &req.body {
            Body::Empty => r.call(),
            Body::Json(v) => r.send_json(v.clone()),
            Body::Form(pairs) => r.send_form(pairs),
        };
        match resp {
            Ok(resp) => resp
                .into_string()
                .map_err(|e| HttpError::Transport(e.to_string())),
            // 4xx/5xx：ureq 归为 Error::Status，响应体交给调用方判读（OAuth 错误码等）。
            Err(ureq::Error::Status(code, resp)) => Err(HttpError::Status(
                code,
                resp.into_string().unwrap_or_default(),
            )),
            Err(e) => Err(HttpError::Transport(e.to_string())),
        }
    }

    /// 流式下载。agent 的二进制是 240–260 MB——边读边写，绝不先缓冲进内存。
    fn download(
        &self,
        url: &str,
        dest: &std::path::Path,
        timeout: std::time::Duration,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, HttpError> {
        use std::io::{Read, Write};

        let resp = self
            .agent()
            .get(url)
            .timeout(timeout)
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(code, r) => {
                    HttpError::Status(code, r.into_string().unwrap_or_default())
                }
                e => HttpError::Transport(e.to_string()),
            })?;
        // Content-Length 可能缺失（chunked）；缺了就画不了进度条，但下载照常。
        let total: Option<u64> = resp.header("Content-Length").and_then(|s| s.parse().ok());

        let mut file =
            std::fs::File::create(dest).map_err(|e| HttpError::Transport(e.to_string()))?;
        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| HttpError::Transport(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| HttpError::Transport(e.to_string()))?;
            written += n as u64;
            on_progress(written, total);
        }
        file.flush()
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        Ok(written)
    }
}

// ═══ 系统密钥链 ═══

/// macOS 登录 Keychain（经 `security` 子进程）。非 macOS 编译成 [`meowo_agent::NoKeychain`] 的等价物。
pub struct SystemKeychain;

#[cfg(target_os = "macos")]
impl KeychainPort for SystemKeychain {
    fn available(&self) -> bool {
        true
    }

    fn read_password(&self, service: &str) -> Option<String> {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8(out.stdout).ok()?;
        let s = s.trim_end_matches(['\r', '\n']).to_string();
        (!s.is_empty()).then_some(s)
    }

    fn read_account(&self, service: &str) -> Option<String> {
        // `-g`：属性打到 stdout、密码打到 stderr，这里只取属性。
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", service, "-g"])
            .output()
            .ok()?;
        parse_keychain_account(&String::from_utf8_lossy(&out.stdout))
    }

    /// `-U`：条目存在则更新。password 经 argv 传入，仅同用户进程可见，与本仓既有 shell-out 一致。
    fn write_password(&self, service: &str, account: &str, password: &str) -> Result<(), String> {
        let status = std::process::Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                service,
                "-a",
                account,
                "-w",
                password,
            ])
            .status()
            .map_err(|e| format!("写回 Keychain 失败：{e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("写回 Keychain 失败（security add-generic-password 非零退出）".into())
        }
    }
}

/// 非 macOS：无密钥链，凭据走文件。全部方法取 trait 默认实现（`available()` 恒 false）。
#[cfg(not(target_os = "macos"))]
impl KeychainPort for SystemKeychain {}

/// 从 `security find-generic-password -g` 的属性输出里抠出 account（`"acct"<blob>=...`）。
/// 形如 `"acct"<blob>="root"`，或 non-UTF8 时 `0x726F6F74  "root"`（hex + 可读串）→ 取引号内。
/// 纯函数便于单测；仅 macOS 调用，其它平台放行 dead_code。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_keychain_account(attrs: &str) -> Option<String> {
    let key = "\"acct\"<blob>=";
    for line in attrs.lines() {
        let Some(idx) = line.find(key) else { continue };
        let rest = line[idx + key.len()..].trim();
        if rest == "<NULL>" {
            return None;
        }
        let after = &rest[rest.find('"')? + 1..];
        let v = &after[..after.find('"')?];
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

// ═══ 注入点 ═══

static KEYCHAIN: SystemKeychain = SystemKeychain;

/// 一组绑定到具体 agent 的宿主端口。
///
/// 从前是个 `&'static` 全局单例；代理做成 per-agent 之后它必须带状态（该 agent 的代理串），
/// 于是改为**按 agent 现场构造**。这不勉强：每个调用点本来就知道自己在为谁服务
/// （`usage_of(id)` / `install_agent(provider)`），且 `Ports<'a>` 本就带生命周期，
/// 借局部变量即可，不需要任何 `'static` 体操。
pub struct HostPorts {
    http: UreqHttp,
}

impl HostPorts {
    /// 该 agent 生效的端口（代理按 per_agent 覆盖 → 全局 → 环境变量解析）。
    ///
    /// 只有这一个构造器：Rust 侧的出站请求**全部**是 agent 绑定的（用量 / OAuth / 装 CLI）。
    /// 唯一与 agent 无关的流量是自更新，而它走前端的 `get_effective_proxy` 命令
    /// （updater 内部是 reqwest，本就不经这里的 ureq 客户端）。
    pub fn for_agent(id: meowo_agent::AgentId) -> Self {
        Self {
            http: UreqHttp {
                proxy: resolve_proxy(Some(id.as_str())),
            },
        }
    }

    pub fn as_ports(&self) -> meowo_agent::Ports<'_> {
        meowo_agent::Ports {
            http: &self.http,
            keychain: &KEYCHAIN,
        }
    }
}

/// 解析某 agent（`None` = 全局）当前生效的代理串。
///
/// 每次现读 settings.json：文件小，而调用点都是低频的（用量有 60s 限频、安装是偶发动作）。
/// 不缓存，也就不必在设置改动时做失效——改完立刻生效。
pub fn resolve_proxy(agent: Option<&str>) -> Option<String> {
    crate::settings::load_settings().proxy.resolve(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 起一个假代理：返回（端口, 拿到的首个请求行）。经 HTTP 代理请求 https 时，
    /// ureq 必须先发 `CONNECT host:443`——收到它即证明流量确实走了这个代理。
    fn fake_proxy() -> (u16, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定假代理端口");
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("代理应收到连接");
            let mut buf = [0u8; 512];
            let n = sock.read(&mut buf).unwrap_or(0);
            // 只关心「连没连到这儿、发的是不是 CONNECT」，回 502 让请求尽快收场。
            let _ = sock.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (port, h)
    }

    /// 请求必须真的从代理走——本次改造的全部意义所在。
    ///
    /// 若有人把实现退回 `ureq::get()`（内置默认 Agent，既不走代理也不读 HTTPS_PROXY），
    /// 连接压根不会到达假代理，本测试立刻失败。
    #[test]
    fn request_actually_routes_through_the_proxy() {
        use std::time::Duration;

        let (port, proxy) = fake_proxy();
        let agent = agent_for(Some(&format!("http://127.0.0.1:{port}")));
        // 必定失败（对面是假代理）；我们要的是它**朝哪儿发**。
        let _ = agent
            .get("https://api.anthropic.com/api/oauth/usage")
            .timeout(Duration::from_secs(5))
            .call();

        let req = proxy.join().expect("假代理线程");
        assert!(
            req.starts_with("CONNECT api.anthropic.com:443"),
            "请求没走代理（假代理收到的是：{req:?}）"
        );
    }

    #[test]
    fn encoded_proxy_credentials_are_decoded_before_basic_auth() {
        use std::time::Duration;

        let (port, proxy) = fake_proxy();
        let url = format!("http://user%40example:p%40ss%2Fword%3Atail@127.0.0.1:{port}");
        let agent = agent_for(Some(&url));
        let _ = agent
            .get("https://api.anthropic.com/x")
            .timeout(Duration::from_secs(5))
            .call();

        let req = proxy.join().expect("假代理线程");
        assert!(
            req.lines().any(|line| {
                line.eq_ignore_ascii_case(
                    "Proxy-Authorization: basic dXNlckBleGFtcGxlOnBAc3Mvd29yZDp0YWls",
                )
            }),
            "代理认证没有使用原始凭据：{req:?}"
        );
    }

    /// 不同代理串各走各的。
    ///
    /// 直击 per-agent 代理的命门：Agent 是按代理串缓存的，缓存键一旦写塌（比如都落到同一个 key），
    /// claude 的流量就会跑进 kimi 的代理里——而这种串味 bug 光看代码极难发现。
    #[test]
    fn distinct_proxies_do_not_share_a_cached_agent() {
        use std::time::Duration;

        let (port_a, proxy_a) = fake_proxy();
        let (port_b, proxy_b) = fake_proxy();

        for (port, host) in [(port_a, "api.anthropic.com"), (port_b, "api.moonshot.cn")] {
            let agent = agent_for(Some(&format!("http://127.0.0.1:{port}")));
            let _ = agent
                .get(&format!("https://{host}/x"))
                .timeout(Duration::from_secs(5))
                .call();
        }

        assert!(
            proxy_a
                .join()
                .unwrap()
                .starts_with("CONNECT api.anthropic.com:443"),
            "代理 A 应只收到 A 的流量"
        );
        assert!(
            proxy_b
                .join()
                .unwrap()
                .starts_with("CONNECT api.moonshot.cn:443"),
            "代理 B 应只收到 B 的流量"
        );
    }

    #[test]
    fn parse_keychain_account_extracts_acct() {
        let attrs = "keychain: \"/Users/x/Library/Keychains/login.keychain-db\"\n    \"acct\"<blob>=\"root\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n";
        assert_eq!(
            super::parse_keychain_account(attrs).as_deref(),
            Some("root")
        );
        // non-UTF8 时 security 打成 hex + 可读串，取引号内。
        let hexed = "    \"acct\"<blob>=0x726F6F74  \"root\"\n";
        assert_eq!(
            super::parse_keychain_account(hexed).as_deref(),
            Some("root")
        );
        // 没有 acct 行 / NULL → None。
        assert_eq!(super::parse_keychain_account("nothing here"), None);
        assert_eq!(
            super::parse_keychain_account("    \"acct\"<blob>=<NULL>\n"),
            None
        );
    }
}
