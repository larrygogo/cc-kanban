//! codex 的接线副作用：hooks.json 落盘之后，往 `config.toml` 的 `[hooks.state]` 写 trusted_hash
//! 预信任，省掉用户在 codex TUI 里的一次 Trust all。
//!
//! hooks 合并逻辑本身在 `crate::config::ConfigFormat::CodexJson`，通用编排在 `crate::wiring`。
use serde_json::Value;

use crate::config::RepairReason;
use crate::variant::Installation;
use crate::wiring::WiringCap;

pub struct CodexWiring;
pub static WIRING: CodexWiring = CodexWiring;

impl WiringCap for CodexWiring {
    fn after_write(&self, inst: &Installation, written: &str) -> Option<RepairReason> {
        write_trusted_hashes(inst, written)
    }
}

/// 我方在 hooks.json 里认领到的一条 hook，附真实位置（group_idx = 组在该事件数组里的下标，
/// handler_idx = handler 在组内 hooks 数组里的下标）——trusted_hash 键必须落在真实位置上，
/// 写死 0:0 会在用户组共存、我方组不在最前时错把哈希覆盖到别人的槽位（详见调用处）。
pub struct ClaimedEntry {
    pub event: String,
    pub command: String,
    pub timeout: u64,
    pub group_idx: usize,
    pub handler_idx: usize,
}

/// 提取合并后全部我方认领条目，供 trusted_hash 按**实际写出的内容 + 真实位置**计算
/// （含既有条目的原样 timeout，如本机 Stop=10）。
///
/// 只认领「整组恰为单 handler 且是我方」的组：trusted_hash 的 canon 公式（见
/// `codex_hook_hash`）本身就是单 handler 形态，组内还有其他 handler 时算出来的哈希对不上
/// codex 真实校验对象——跳过不预信任，退化为 codex 一次 Trust all，无损。
pub fn claimed_codex_entries(root: &Value) -> Vec<ClaimedEntry> {
    let mut out = Vec::new();
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return out;
    };
    for (ev, arr) in hooks {
        for (group_idx, entry) in arr.as_array().into_iter().flatten().enumerate() {
            let Some(hs) = entry.get("hooks").and_then(|x| x.as_array()) else {
                continue;
            };
            if hs.len() != 1 {
                continue; // 组内不止一个 handler：canon 公式套不上，跳过不预信任
            }
            let Some(configured_cmd) = hs[0].get("command").and_then(|c| c.as_str()) else {
                continue;
            };
            if claim_codex_cmd(configured_cmd).is_some() {
                // Codex 先按平台选择 commandWindows/command，再对这个“有效命令”计算信任哈希。
                // 必须复刻该顺序，否则 Windows 覆盖存在时我们写出的 trusted_hash 永远不匹配。
                #[cfg(windows)]
                let effective_cmd = hs[0]
                    .get("commandWindows")
                    .and_then(|c| c.as_str())
                    .unwrap_or(configured_cmd);
                #[cfg(not(windows))]
                let effective_cmd = configured_cmd;
                // codex 源码中默认超时为 600（`.unwrap_or(600).max(1)`），同步本处默认值。
                let t = hs[0].get("timeout").and_then(|t| t.as_u64()).unwrap_or(600);
                out.push(ClaimedEntry {
                    event: ev.clone(),
                    command: effective_cmd.to_string(),
                    timeout: t,
                    group_idx,
                    handler_idx: 0,
                });
            }
        }
    }
    out
}

/// CamelCase 事件名 → codex hooks.state 键用的 snake_case 标签。未知返回 ""。
pub(crate) fn snake_event(ev: &str) -> &'static str {
    match ev {
        "SessionStart" => "session_start",
        "UserPromptSubmit" => "user_prompt_submit",
        "PostToolUse" => "post_tool_use",
        "Stop" => "stop",
        "PermissionRequest" => "permission_request",
        "PreCompact" => "pre_compact",
        "PostCompact" => "post_compact",
        _ => "",
    }
}

/// codex trusted_hash：对归一化身份对象的 canonical JSON（key 字母序、紧凑）做 SHA-256。
/// 算法为 codex 内部实现（源码 fingerprint.rs），本机 0.142.3 三真实向量验证命中；
/// 漂移时向量测试变红，运行期兜底 = codex TUI 一次 Trust all，无损。
/// 手工 format! 拼串而非 serde_json 对象：杜绝 preserve_order 特性统一导致的键序漂移。
pub(crate) fn codex_hook_hash(event_snake: &str, command: &str, timeout: u64) -> String {
    use sha2::{Digest, Sha256};
    let canon = format!(
        r#"{{"event_name":{ev},"hooks":[{{"async":false,"command":{cmd},"timeout":{timeout},"type":"command"}}]}}"#,
        ev = serde_json::to_string(event_snake).unwrap_or_default(),
        cmd = serde_json::to_string(command).unwrap_or_default(),
    );
    format!("sha256:{:x}", Sha256::digest(canon.as_bytes()))
}

/// 向 config.toml 的 [hooks.state] 写入/更新各认领条目的 trusted_hash（只动该子树）。
/// 键格式：'<hooks.json 绝对路径 display 串>:<snake_case 事件>:<group_idx>:<handler_idx>'——
/// 索引取自 entries 里携带的真实位置，而非写死 0:0：ensure_codex_hooks 只在事件数组末尾
/// 追加我方组、从不重排既有条目，故同一台机器上这个真实索引跨启动稳定，可安全做幂等比较。
pub fn ensure_trusted_hashes(
    doc: &mut toml_edit::DocumentMut,
    hooks_path_display: &str,
    entries: &[ClaimedEntry],
) -> bool {
    let mut changed = false;
    // 逐层确保 hooks / hooks.state 为隐式 table（不打扰同级既有内容）。
    if doc.get("hooks").is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert("hooks", toml_edit::Item::Table(t));
    }
    // hooks 键存在但非 table（畸形/误粘 kimi 数组语法）：放弃信任写入，绝不 panic——退化等价于解析失败路径
    let Some(hooks) = doc["hooks"].as_table_mut() else {
        return changed;
    };
    if hooks.get("state").is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        hooks.insert("state", toml_edit::Item::Table(t));
    }
    let Some(state) = hooks["state"].as_table_mut() else {
        return changed;
    };
    for entry in entries {
        let snake = snake_event(&entry.event);
        if snake.is_empty() {
            continue; // 未知事件不写信任（不该发生：entries 来自我方认领条目）
        }
        let key = format!(
            "{hooks_path_display}:{snake}:{}:{}",
            entry.group_idx, entry.handler_idx
        );
        let hash = codex_hook_hash(snake, &entry.command, entry.timeout);
        let cur = state
            .get(&key)
            .and_then(|it| it.get("trusted_hash"))
            .and_then(|v| v.as_str());
        if cur != Some(hash.as_str()) {
            let mut t = toml_edit::Table::new();
            t.insert("trusted_hash", toml_edit::value(hash));
            state.insert(&key, toml_edit::Item::Table(t));
            changed = true;
        }
    }
    changed
}

/// codex 的 hook command 形态：`"<exe>" --provider codex`。与变体表的声明同源。
fn claim_codex_cmd(cmd: &str) -> Option<String> {
    const SHAPE: crate::CommandSpec = crate::CommandSpec {
        quote_exe: true,
        with_provider: true,
        ps_call_operator: false,
    };
    SHAPE.claim(cmd, "codex")
}

/// hooks.json 落盘之后的副作用：往 `config.toml` 的 `[hooks.state]` 写 trusted_hash 预信任。
/// 先 hooks.json 落盘成功、再写预信任（反序会留下指向不存在配置的信任残渣）。
///
/// **纯属锦上添花**：hooks 此时已接上，本步任何失败都不算接线失败——退化 = codex 弹一次
/// Trust all，不影响入库。故一律返回 `None`。
fn write_trusted_hashes(inst: &Installation, hooks_text: &str) -> Option<RepairReason> {
    let hooks_path = inst.config_path();
    let cfg_path = inst.data_dir.join("config.toml");
    let cfg_text = match std::fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    let Ok(mut doc) = cfg_text.parse::<toml_edit::DocumentMut>() else {
        return None;
    };
    let root = crate::config::parse_json_config(hooks_text)?;
    let entries = claimed_codex_entries(&root);
    if ensure_trusted_hashes(&mut doc, &hooks_path.display().to_string(), &entries) {
        // 与 hooks 接线同纪律：写前必备份，备份失败就不写（config.toml 装着用户的模型与
        // provider 配置）。本步是锦上添花，失败不算接线失败，但必须留痕、不许静默吞。
        if cfg_path.exists() {
            if let Err(e) = crate::wiring::backup_once(&cfg_path) {
                eprintln!(
                    "Meowo repair[codex]: {} 备份失败（{e}），跳过 trusted_hash 写入",
                    cfg_path.display()
                );
                return None;
            }
        }
        if let Err(e) = crate::fsutil::write_atomic(&cfg_path, &doc.to_string()) {
            eprintln!(
                "Meowo repair[codex]: {} trusted_hash 写入失败（{e}）",
                cfg_path.display()
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // hooks.json 的合并/判定已迁入 crate::config::ConfigFormat::CodexJson，
    // 相关测试随之搬到该 crate（见 config.rs 的 mod codex）。此处只留 trusted_hash 副作用的测试。

    /// 用变体表声明的格式适配器造出「已接线」的 hooks.json，供下方 trusted_hash 测试消费。
    ///
    /// 直接从注册表取 `HookSpec`（纯声明），不经 `codex_install()`——后者要解析真实 home
    /// （`USERPROFILE`/`HOME`），会让本测试无谓地依赖环境变量。这里根本用不到安装实况。
    fn wired(reporter: &str) -> Value {
        let hooks = crate::by_id("codex").expect("codex 应已注册").variants()[0].hooks;
        match hooks.ensure_hooks("{\"hooks\":{}}", reporter, "codex") {
            crate::EnsureOutcome::Changed(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("期望 Changed，实得 {other:?}"),
        }
    }

    #[test]
    fn claimed_entries_extraction() {
        let v = wired("C:/x/meowo-reporter.exe");
        let entries = claimed_codex_entries(&v);
        // 7 = 变体表 EVENTS 全量（含 PreCompact/PostCompact 两条压缩指示）。
        assert_eq!(entries.len(), 7);
        assert!(entries
            .iter()
            .all(|e| e.command.contains("--provider codex")));
        assert_eq!(
            entries
                .iter()
                .find(|e| e.event == "PermissionRequest")
                .map(|e| e.timeout),
            Some(310)
        );
        assert!(entries
            .iter()
            .filter(|e| e.event != "PermissionRequest")
            .all(|e| e.timeout == 5));
        assert!(entries.iter().any(|e| e.event == "PermissionRequest"));
        // 全新单 handler 组场景：真实位置就是 0:0。
        assert!(entries
            .iter()
            .all(|e| e.group_idx == 0 && e.handler_idx == 0));
    }

    #[test]
    fn codex_hook_hash_matches_real_machine_vectors() {
        // 三条向量取自本机 ~/.codex/config.toml 的真实 trusted_hash（codex-cli 0.142.3），
        // 算法：canonical JSON（key 字母序、紧凑）SHA-256。codex 升级若改算法，此测试变红。
        let cmd = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe --provider codex";
        assert_eq!(
            codex_hook_hash("session_start", cmd, 5),
            "sha256:5e68ee84ac2076b424f12a7a1b346f5c1f5907d4829d6a30239bc49c0e76382c"
        );
        assert_eq!(
            codex_hook_hash("user_prompt_submit", cmd, 5),
            "sha256:aef30ddec757deff63f67240a9b859d1a63c669eaee6a5ee6a30404daaa81523"
        );
        assert_eq!(
            codex_hook_hash("stop", cmd, 10),
            "sha256:cd638b7a1c4a91cd28a5946fbbe5d7e7bf1f3c478d85f329ad95323a9323e403"
        );
    }

    #[test]
    fn snake_event_covers_all_codex_events() {
        // 绊线：变体表新增事件而忘了补 snake_case 映射 → trusted_hash 键写不出，此处失败。
        let inst = crate::installation(crate::id::CODEX).unwrap();
        for ev in inst.hooks.events {
            assert!(
                !snake_event(ev.name).is_empty(),
                "{} 缺 snake_case 映射",
                ev.name
            );
        }
        assert_eq!(snake_event("PermissionRequest"), "permission_request");
        assert_eq!(snake_event("Unknown"), "");
    }

    #[test]
    fn ensure_trusted_hashes_writes_and_is_idempotent() {
        // 从空 config.toml 起：写入 [hooks.state] 各键；已有等值哈希则不动；不碰无关内容。
        // 单组场景（无其他 handler 共存）：真实位置就是 0:0——本机真实向量断言一字不动。
        let mut doc: toml_edit::DocumentMut = "default_model = \"x\"\n".parse().unwrap();
        let cmd = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe --provider codex";
        let entries = vec![ClaimedEntry {
            event: "SessionStart".to_string(),
            command: cmd.to_string(),
            timeout: 5,
            group_idx: 0,
            handler_idx: 0,
        }];
        let hooks_path = r"C:\Users\larry\.codex\hooks.json";
        assert!(ensure_trusted_hashes(&mut doc, hooks_path, &entries));
        let out = doc.to_string();
        assert!(out.contains("default_model = \"x\"")); // 无关内容原样
                                                        // 键 = <display路径>:<snake事件>:0:0，值 = 本机验证过的真实哈希。
        assert!(out.contains(r"C:\Users\larry\.codex\hooks.json:session_start:0:0"));
        assert!(
            out.contains("sha256:5e68ee84ac2076b424f12a7a1b346f5c1f5907d4829d6a30239bc49c0e76382c")
        );
        // 幂等：二跑无改动。
        assert!(!ensure_trusted_hashes(&mut doc, hooks_path, &entries));
    }

    #[test]
    fn ensure_trusted_hashes_tolerates_malformed_hooks_key() {
        // hooks 键存在但非 table（如误粘 kimi 的数组语法）：不得 panic，返回无改动、文档原样。
        let mut doc: toml_edit::DocumentMut = "hooks = \"oops\"\n".parse().unwrap();
        let entries = vec![ClaimedEntry {
            event: "SessionStart".to_string(),
            command: "x --provider codex".to_string(),
            timeout: 5,
            group_idx: 0,
            handler_idx: 0,
        }];
        assert!(!ensure_trusted_hashes(&mut doc, r"C:\h.json", &entries));
        assert_eq!(doc.to_string(), "hooks = \"oops\"\n");
    }

    #[test]
    fn ensure_trusted_hashes_uses_real_group_index_when_user_group_precedes_ours() {
        // C1 回归：用户已有 Stop hook 组在 [0]（如 node my-notify.js），我方组在 [1]——
        // ensure_codex_hooks 只追加不重排，这正是真实机器上会出现的既有组形态。
        let dev = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe";
        let v = json!({ "hooks": { "Stop": [
            { "hooks": [{ "type": "command", "command": "node my-notify.js" }] },
            { "hooks": [{ "type": "command", "command": format!("{dev} --provider codex"), "timeout": 10 }] }
        ]}});
        let entries = claimed_codex_entries(&v);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].group_idx, 1); // 我方组真实位置是 1，不是 0
        assert_eq!(entries[0].handler_idx, 0);

        let mut doc: toml_edit::DocumentMut = "default_model = \"x\"\n".parse().unwrap();
        let hooks_path = r"C:\Users\larry\.codex\hooks.json";
        assert!(ensure_trusted_hashes(&mut doc, hooks_path, &entries));
        let out = doc.to_string();
        assert!(out.contains(r"C:\Users\larry\.codex\hooks.json:stop:1:0"));
        // 用户组的槽位 :0:0 绝不能被写入/覆盖。
        assert!(!out.contains(r"C:\Users\larry\.codex\hooks.json:stop:0:0"));
        // 幂等：二跑无改动，且 0:0 键始终未被触碰。
        assert!(!ensure_trusted_hashes(&mut doc, hooks_path, &entries));
        assert!(!doc
            .to_string()
            .contains(r"C:\Users\larry\.codex\hooks.json:stop:0:0"));
    }

    #[test]
    fn claimed_codex_entries_skips_group_shared_with_other_handler() {
        // C1 回归：我方 handler 与他人 handler 挤在同一组里——canon 哈希公式只支持单 handler
        // 形态，算出来的哈希对不上 codex 真实校验对象，必须跳过不预信任。
        let dev = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe";
        let v = json!({ "hooks": { "Stop": [
            { "hooks": [
                { "type": "command", "command": "node my-notify.js" },
                { "type": "command", "command": format!("{dev} --provider codex"), "timeout": 5 }
            ] }
        ]}});
        assert!(claimed_codex_entries(&v).is_empty());
    }

    /// dry-run：CODEX_HOME=<真实 ~/.codex 的副本> 时跑 CodexSetup::apply，核对副本产物。
    /// 用法：复制 ~/.codex 到临时目录，
    ///       CODEX_HOME=<副本> cargo test -p meowo-agent dryrun_codex -- --ignored --nocapture
    ///
    /// 只打印结构性摘要，**绝不 dump 配置原文**——真实 config.toml/auth.json 含凭据。
    #[test]
    #[ignore]
    fn dryrun_codex() {
        use crate::registry::AgentPlugin;
        let meowo_dir = std::env::temp_dir();
        let ctx = crate::wiring::WiringContext {
            fallback_reporter: None,
            meowo_dir: &meowo_dir,
        };
        let reason = super::super::Codex.wire(&ctx);
        let inst = crate::installation(crate::id::CODEX).unwrap();
        let text = std::fs::read_to_string(inst.config_path()).unwrap_or_default();
        let root: Value = serde_json::from_str(&text).expect("产物应为合法 JSON");
        eprintln!(
            "变体={} 配置={}",
            inst.variant_tag,
            inst.config_path().display()
        );
        eprintln!("wire reason={reason:?}");
        eprintln!(
            "hooks 事件={:?}",
            root["hooks"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
        );
        eprintln!(
            "SessionStart 已接线={}",
            inst.hooks.has_reporter(&text, "codex")
        );
        eprintln!("启动 argv={:?}", inst.launch_argv());

        // trusted_hash：只列键名（值是哈希，非机密，但键含绝对路径，够核对了）。
        let cfg = inst.data_dir.join("config.toml");
        match std::fs::read_to_string(&cfg)
            .ok()
            .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
        {
            Some(doc) => {
                let keys: Vec<String> = doc
                    .get("hooks")
                    .and_then(|h| h.get("state"))
                    .and_then(|s| s.as_table())
                    .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
                    .unwrap_or_default();
                eprintln!("[hooks.state] 键={keys:#?}");
            }
            None => eprintln!("config.toml 不存在或非法"),
        }
    }
}
