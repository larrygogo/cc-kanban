//! hooks 接线的**声明**与**纯函数实现**：一个 [`HookSpec`] 描述某个变体「hooks 写在哪个文件、
//! 什么格式、挂哪些事件、命令长什么样」；三个方法回答「确保接入 / 是否已接入 / 二进制在哪」。
//!
//! 全是纯函数（输入配置文本，输出新文本或判定），落盘由调用方负责——便于单测，也保证
//! 「解析失败绝不写」。带副作用的接线步骤（claude 的 statusline 脚本、codex 的 trusted_hash）
//! 不在这里：它们要 `fsutil`/`sha2`/`db_path`，由 meowo-app 的 `SetupBehavior` 承接。

use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

/// 接线失败的机器可读原因，回传前端以给出精准提示（如未登录 → 「请先登录」）。
/// 序列化为 kebab-case 字符串；`None` 表示成功或已是目标状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepairReason {
    /// agent 数据目录不存在（视为未安装）。
    NotDetected,
    /// 承载 hooks 的配置文件尚未生成（如 kimi 的 config.toml 需先 `kimi login`）。
    NeedLogin,
    /// 找不到 meowo-reporter 二进制（既有 hooks 无有效路径且 app 同目录无 sidecar）。
    ReporterNotFound,
    /// 配置文件读取或解析失败（权限/编码/畸形），为保护用户文件放弃写入。
    ConfigUnreadable,
    /// 写入失败（目录不可写/磁盘满/杀软拦截）。
    WriteFailed,
}

/// `ensure_hooks` 的三态结果。`Changed` 携带待落盘的新文本。
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
    Changed(String),
    Unchanged,
    /// 配置形态无法安全改写 → 放弃，绝不写坏用户文件。
    Abandon(RepairReason),
}

/// hooks 配置的落盘格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    /// kimi：`config.toml` 顶层 `[[hooks]]` array-of-tables。
    KimiToml,
    /// codex：`hooks.json` 的 `{"hooks": {"<Event>": [{"hooks":[{...}]}]}}`，条目**无** matcher。
    /// 顶层只允许 `hooks` 键（codex 侧 deny_unknown_fields）。
    CodexJson,
    /// claude：`settings.json` 的 `{"hooks": {"<Event>": [{"matcher":"...","hooks":[{...}]}]}}`。
    /// 与 `CodexJson` 同构，唯一差别是条目**带 `matcher`**——同一事件下按 matcher 区分多条，
    /// 与用户自有 hook（如 `PreToolUse:Bash` 预检）共存。顶层还承载 `statusLine` 等无关键，
    /// 一律原样保留。
    ClaudeJson,
    /// opencode：产物**不是配置文件，而是一个 TS 插件**。opencode 压根没有 command hook——它只认
    /// JS/TS 插件（启动时扫 `<配置目录>/{plugin,plugins}/*.ts`），故「接线」在这里落成一份由 meowo
    /// 生成的桥接插件：它订阅 opencode 的事件，再把 payload 喂给 meowo-reporter 的 stdin。
    ///
    /// 反而占了个便宜：**payload 由我们自己构造**，可以直接吐成与 claude 同款的
    /// `{hook_event_name, session_id, cwd}`，reporter 侧一行都不用改。
    ///
    /// 代价是这份文件整体归 meowo 所有：幂等判定按「生成的全文是否逐字相同」，用户手改会在下次
    /// 接线时被整体覆盖（文件头已写明「请勿手改」）。与另三家「在用户的配置里合并进我方条目、
    /// 用户自有条目一概不动」是两种不同的所有权模型——因为那三家的文件本就属于用户，
    /// 而这个文件从来只有 meowo 会写。
    OpencodeTs,
}

/// 配置文件不存在时怎么办。声明式，避免每个 agent 的 `apply` 各写一遍 NotFound 分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingConfig {
    /// 从这段初始文本起（如 claude 的 `{}`、codex 的 `{"hooks":{}}`）。
    CreateFrom(&'static str),
    /// 不创建，直接失败（如 kimi 的 config.toml 需先 `kimi login`）。
    Fail(RepairReason),
}

/// hook command 的书写形态。三家各不相同，且**不能随意统一**：
/// 引号与否影响既有配置的幂等判定，`--provider` 与否影响认领规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// 可执行路径是否加双引号（claude/codex 加，kimi 不加——保持与各自现存配置一致）。
    /// 路径含空白字符时本标志被忽略、一律加引号：裸路径会在执行/认领两侧同按空白截断。
    pub quote_exe: bool,
    /// 是否追加 `--provider <id>`（codex/kimi 加；claude 靠 settings 里的位置区分，不带参）。
    pub with_provider: bool,
    /// Windows 上是否在命令串前加 PowerShell 调用运算符 `& `。
    ///
    /// gemini 的 hookRunner 在 Windows 恒经 PowerShell 执行 command（实测 0.51
    /// `getShellConfiguration`：ComSpec 是 pwsh/powershell 用之，否则找 pwsh，兜底
    /// powershell.exe——**没有 cmd 分支**），而 PowerShell 里以引号开头的
    /// `"exe" --provider x` 是字符串表达式、解析即失败——codex 当年靠官方的
    /// `commandWindows` 绕开同一个坑，gemini 没有该字段，只能写 `& "exe" --provider x`。
    /// 非 Windows（bash -c）不加：`&` 在 bash 里是后台运算符。
    pub ps_call_operator: bool,
}

impl CommandSpec {
    /// 写出的 command 串。
    pub fn render(self, reporter: &str, agent_id: &str) -> String {
        // 路径含空白时必须加引号，与 quote_exe 无关：裸写的命令会被执行方与 `parse_hook_command`
        // 同按第一个空白截断——hook 执行失败，认领也永远认不回去，每轮接线追加一条孤儿条目。
        // 无空白时仍按 quote_exe 书写，既有条目（幂等按解析后内容比对）不会被无谓改写。
        let exe = if self.quote_exe || reporter.contains(char::is_whitespace) {
            format!("\"{reporter}\"")
        } else {
            reporter.to_string()
        };
        let exe = if self.ps_call_operator && cfg!(windows) {
            format!("& {exe}")
        } else {
            exe
        };
        if self.with_provider {
            format!("{exe} --provider {agent_id}")
        } else {
            exe
        }
    }

    /// 严格认领：可执行文件名恰为 meowo-reporter[.exe]（或历史遗留 cc-reporter[.exe]，以便升级时
    /// 替换旧路径），且余参**恰好**符合本形态。返回可执行路径。不裸 `contains`，不误伤用户 hook
    /// （如 `node tools/meowo-reporter-notify.js`）。
    pub fn claim(self, cmd: &str, agent_id: &str) -> Option<String> {
        let (path, args) = parse_hook_command(cmd)?;
        if !is_any_reporter(&path) {
            return None;
        }
        let args_ok = if self.with_provider {
            args == ["--provider", agent_id]
        } else {
            args.is_empty()
        };
        args_ok.then_some(path)
    }
}

/// 一个 hook 事件。`matcher` 仅 claude 有（同一事件下按 matcher 区分多条，与用户自有 hook 共存）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookEvent {
    pub name: &'static str,
    pub matcher: Option<&'static str>,
    pub timeout: u64,
}

impl HookEvent {
    pub const fn plain(name: &'static str) -> Self {
        Self {
            name,
            matcher: None,
            timeout: 5,
        }
    }
    pub const fn matched(name: &'static str, matcher: &'static str) -> Self {
        Self {
            name,
            matcher: Some(matcher),
            timeout: 5,
        }
    }
    pub const fn with_timeout(mut self, timeout: u64) -> Self {
        self.timeout = timeout;
        self
    }
}

/// 某变体的 hooks 接线规格。加/改 agent 只写这张声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSpec {
    /// 配置文件相对 `data_dir` 的路径（`settings.json` / `hooks.json` / `config.toml`）。
    pub config_rel: &'static str,
    pub format: ConfigFormat,
    pub missing: MissingConfig,
    pub events: &'static [HookEvent],
    pub command: CommandSpec,
}

impl HookSpec {
    /// 幂等接入 meowo-reporter hooks。用户自有的 hook 条目一概不动。
    pub fn ensure_hooks(&self, cur_text: &str, reporter: &str, agent_id: &str) -> EnsureOutcome {
        match self.format {
            ConfigFormat::KimiToml => kimi_toml::ensure_hooks(self, cur_text, reporter, agent_id),
            ConfigFormat::CodexJson => codex_json::ensure_hooks(self, cur_text, reporter, agent_id),
            ConfigFormat::ClaudeJson => {
                claude_json::ensure_hooks(self, cur_text, reporter, agent_id)
            }
            ConfigFormat::OpencodeTs => opencode_ts::ensure_hooks(cur_text, reporter, agent_id),
        }
    }

    /// 配置文本能否被本格式解析。区分「解析不了」与「解析得了但没挂 reporter」——前者是
    /// 暂时不可读/损坏，不该误报成「未接入」诱导用户去修复（修复也会因 Abandon 而拒写）。
    pub fn parses(&self, cur_text: &str) -> bool {
        match self.format {
            ConfigFormat::KimiToml => cur_text.parse::<DocumentMut>().is_ok(),
            ConfigFormat::CodexJson | ConfigFormat::ClaudeJson => {
                json_common::parse(cur_text).is_some()
            }
            // 这份文件由 meowo 独占生成，任何内容都能被下一次生成整体取代——不存在「读得出但改不动」
            // 的中间态，故恒真（另三家要靠它区分「文件坏了」与「文件好但没接线」）。
            ConfigFormat::OpencodeTs => true,
        }
    }

    /// 新会话是否真能入库：**只看 SessionStart**，且必须是当前 meowo-reporter。仅在别的事件
    /// （如 Stop）挂了 reporter，不能保证新会话被记录，不应误判成已接入。
    pub fn has_reporter(&self, cur_text: &str, agent_id: &str) -> bool {
        self.claimed_at(cur_text, agent_id, Some(SESSION_START))
            .is_some_and(|p| is_current_reporter(&p))
    }

    /// 从既有配置里取出已认领的**当前** meowo-reporter 绝对路径，用于复用其位置（不限事件）。
    /// 历史 cc-reporter 不算：它已废弃，当成目标写回去 hooks 依旧失效。
    pub fn claimed_reporter(&self, cur_text: &str, agent_id: &str) -> Option<String> {
        self.claimed_at(cur_text, agent_id, None)
            .filter(|p| is_current_reporter(p))
    }

    /// 认领到的 reporter 路径（可能是历史 cc-reporter）。`event=None` 表示不限事件。
    fn claimed_at(&self, cur_text: &str, agent_id: &str, event: Option<&str>) -> Option<String> {
        match self.format {
            ConfigFormat::KimiToml => kimi_toml::claimed_at(self, cur_text, agent_id, event),
            // claude/codex 的 hooks 树同构（差别只在写入时是否带 matcher），认领扫描共用一份。
            ConfigFormat::CodexJson | ConfigFormat::ClaudeJson => {
                json_common::claimed_at(self, cur_text, agent_id, event)
            }
            // 插件是一体的：认领到它，就意味着它声明的每个事件都在（`event` 因此无从、也不必过滤）。
            ConfigFormat::OpencodeTs => opencode_ts::claimed(cur_text),
        }
    }
}

/// 决定「新会话能否入库」的事件名。三家同名。
const SESSION_START: &str = "SessionStart";

// ═══ 命令行解析（各格式共用） ═══

/// 解析 hook command 为（可执行路径, 余参）。首 token 支持带双引号或裸路径。
pub fn parse_hook_command(cmd: &str) -> Option<(String, Vec<String>)> {
    // 剥掉 PowerShell 调用运算符前缀（gemini 的 Windows 命令形态，见 CommandSpec::ps_call_operator）：
    // 认领只看「可执行 + 余参」，带不带 `&` 的新旧两种形态都要认，否则升级后旧条目成孤儿、新条目重复追加。
    let c = cmd.trim().trim_start_matches('&').trim_start();
    let (path, rest) = if let Some(r) = c.strip_prefix('"') {
        let end = r.find('"')?;
        (r[..end].to_string(), r[end + 1..].trim())
    } else {
        match c.split_once(char::is_whitespace) {
            Some((p, r)) => (p.to_string(), r.trim()),
            None => (c.to_string(), ""),
        }
    };
    let args = rest.split_whitespace().map(str::to_string).collect();
    Some((path, args))
}

fn file_name_lower(path: &str) -> Option<String> {
    // 同时按 `/` 和 `\` 取末段——**不能**用 `Path::file_name()`：它只认当前平台的分隔符，于是在
    // macOS/Linux 上 Windows 路径 `C:\…\meowo-reporter.exe` 会被整串当成文件名，认领判定当场失败。
    // 而 reporter 路径来自配置文件，写它的平台未必是读它的平台。
    let name = path.trim().rsplit(['/', '\\']).next()?;
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

/// 是 meowo-reporter 或历史遗留的 cc-reporter。
fn is_any_reporter(path: &str) -> bool {
    file_name_lower(path).is_some_and(|n| {
        matches!(
            n.as_str(),
            "meowo-reporter" | "meowo-reporter.exe" | "cc-reporter" | "cc-reporter.exe"
        )
    })
}

/// 是**当前** meowo-reporter（排除历史 cc-reporter）。
fn is_current_reporter(path: &str) -> bool {
    file_name_lower(path)
        .is_some_and(|n| matches!(n.as_str(), "meowo-reporter" | "meowo-reporter.exe"))
}

// ═══ KimiToml ═══

mod kimi_toml {
    use super::*;

    pub fn ensure_hooks(
        spec: &HookSpec,
        cur_text: &str,
        reporter: &str,
        agent_id: &str,
    ) -> EnsureOutcome {
        // 解析失败绝不写（kimi 自身对坏文件同样拒写）。
        let Ok(mut doc) = cur_text.parse::<DocumentMut>() else {
            return EnsureOutcome::Abandon(RepairReason::ConfigUnreadable);
        };
        match merge(spec, &mut doc, reporter, agent_id) {
            Merge::Changed => EnsureOutcome::Changed(doc.to_string()),
            Merge::Unchanged => EnsureOutcome::Unchanged,
            Merge::Abandon => EnsureOutcome::Abandon(RepairReason::ConfigUnreadable),
        }
    }

    enum Merge {
        Changed,
        Unchanged,
        Abandon,
    }

    /// 幂等合并 `[[hooks]]`：逐事件找认领条目（event 相符 + command 可认领），路径不符则更新
    /// command；缺则追加 `{event, command, timeout=5}`。用户条目一概不动。
    ///
    /// 纪律（源码调研 kimi 0.20）：kimi 自身会全量重写此文件（注释全丢）——幂等判定只按
    /// (event, command) 内容匹配，绝不依赖注释标记。
    fn merge(spec: &HookSpec, doc: &mut DocumentMut, reporter: &str, agent_id: &str) -> Merge {
        let desired_cmd = spec.command.render(reporter, agent_id);
        let mut changed = false;

        if doc
            .get("hooks")
            .and_then(|it| it.as_array_of_tables())
            .is_none()
        {
            // 不是 array-of-tables。可安全替换的只有两种：
            //   - 不存在：新版 kimi-code 默认无顶层 hooks 键。
            //   - `hooks = []`：旧 Python 版 kimi-cli 的默认**空内联数组**——语义等价于「无 hooks」，
            //     与 `[[hooks]]` array-of-tables 是同一 TOML 类型（空列表），替换无损，kimi 照常读取。
            // 非空内联数组 / 字符串等畸形形态则保守放弃：无法结构保持地转换，绝不写坏用户配置。
            let replaceable = match doc.get("hooks") {
                None => true,
                Some(it) => it.as_array().is_some_and(|a| a.is_empty()),
            };
            if !replaceable {
                return Merge::Abandon;
            }
            doc.remove("hooks");
            doc.insert("hooks", Item::ArrayOfTables(ArrayOfTables::new()));
            changed = true;
        }
        // 上面已确保 hooks 是 array-of-tables（否则已提前返回），故此处必不为 None——
        // 仍用 let-else 而非 expect/panic。
        let Some(arr) = doc["hooks"].as_array_of_tables_mut() else {
            return Merge::Abandon;
        };

        for ev in spec.events {
            // 同一 event 下只留一条我方 hook：第一条更新路径，其余删除。不去重则重复条目每次
            // 事件各派生一个 reporter 进程，且污染每接线一轮翻一倍、永不收敛（详见
            // `json_common::dedupe_event`）。用户自有 hook（claim 不认领）一概不动。
            let mut found = false;
            let mut i = 0;
            while i < arr.len() {
                let Some(t) = arr.get_mut(i) else { break };
                if t.get("event").and_then(|v| v.as_str()) != Some(ev.name) {
                    i += 1;
                    continue;
                }
                let Some(path) = t
                    .get("command")
                    .and_then(|v| v.as_str())
                    .and_then(|c| spec.command.claim(c, agent_id))
                else {
                    i += 1;
                    continue; // 该事件上的用户自有 hook，不动
                };
                if found {
                    arr.remove(i); // 重复注册，删（不推进 i：后一条已顶上来）
                    changed = true;
                    continue;
                }
                found = true;
                if path != reporter {
                    t.insert("command", value(desired_cmd.clone()));
                    changed = true;
                }
                i += 1;
            }
            if !found {
                let mut t = Table::new();
                t.insert("event", value(ev.name));
                t.insert("command", value(desired_cmd.clone()));
                t.insert("timeout", value(5));
                arr.push(t);
                changed = true;
            }
        }
        if changed {
            Merge::Changed
        } else {
            Merge::Unchanged
        }
    }

    /// 认领到的 reporter 路径。解析失败 → None（调用方对「读不出来」另有 Unknown 通道）。
    pub fn claimed_at(
        spec: &HookSpec,
        cur_text: &str,
        agent_id: &str,
        event: Option<&str>,
    ) -> Option<String> {
        // 迭代器借用 doc；必须先算出 owned String 再让 doc 出作用域。
        let doc = cur_text.parse::<DocumentMut>().ok()?;
        let found = doc
            .get("hooks")
            .and_then(|it| it.as_array_of_tables())
            .into_iter()
            .flat_map(|a| a.iter())
            .filter(|t| match event {
                None => true,
                Some(ev) => t.get("event").and_then(|v| v.as_str()) == Some(ev),
            })
            .find_map(|t| {
                t.get("command")
                    .and_then(|v| v.as_str())
                    .and_then(|c| spec.command.claim(c, agent_id))
            });
        found
    }
}

// ═══ JSON 系（claude / codex）共用 ═══

/// `{"hooks": {"<Event>": [{..., "hooks":[{"command":...}]}]}}` 这棵树的解析与认领扫描。
/// claude 与 codex 的差别只在**写入**时条目是否带 `matcher`，读侧完全一致，故共用。
mod json_common {
    use super::*;
    use serde_json::Value;

    /// 解析 JSON 文本。容忍 UTF-8 BOM——Windows 上不少编辑器/PowerShell 写出的 JSON 带 BOM，
    /// serde_json 会直接报错，曾导致无感接线静默失败。
    pub fn parse(text: &str) -> Option<Value> {
        let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        v.is_object().then_some(v)
    }

    /// 序列化为落盘文本（pretty + 末尾换行，与两家既有文件风格一致）。
    pub fn render(root: &Value) -> EnsureOutcome {
        match serde_json::to_string_pretty(root) {
            Ok(s) => EnsureOutcome::Changed(format!("{s}\n")),
            Err(_) => EnsureOutcome::Abandon(RepairReason::WriteFailed),
        }
    }

    /// 取出已认领的 reporter 路径。`event=None` 不限事件。**不看 matcher**：认领只由 command
    /// 形态决定，与条目挂在哪个 matcher 下无关。
    pub fn claimed_at(
        spec: &HookSpec,
        cur_text: &str,
        agent_id: &str,
        event: Option<&str>,
    ) -> Option<String> {
        let root = parse(cur_text)?;
        let hooks = root.get("hooks")?.as_object()?;
        hooks
            .iter()
            .filter(|(ev, _)| event.is_none_or(|want| ev.as_str() == want))
            .flat_map(|(_, arr)| arr.as_array().into_iter().flatten())
            .flat_map(|entry| {
                entry
                    .get("hooks")
                    .and_then(|x| x.as_array())
                    .into_iter()
                    .flatten()
            })
            .find_map(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .and_then(|c| spec.command.claim(c, agent_id))
            })
    }

    /// 确保顶层 `hooks` 是 object。键不存在 → 建空 object（`true`＝有改动）；
    /// 存在但非 object（用户手改坏形状）→ `None`，调用方 Abandon，绝不覆盖用户文件。
    pub fn ensure_hooks_object(root: &mut Value) -> Option<bool> {
        match root.get("hooks") {
            None => {
                root["hooks"] = serde_json::json!({});
                Some(true)
            }
            Some(h) if !h.is_object() => None,
            Some(_) => Some(false),
        }
    }

    /// 条目的 `matcher` 是否是我们要找的那个。`want=None` 时不作要求——codex 的条目本就无
    /// matcher，claude 的事件表则全带。
    pub fn matcher_is(entry: &Value, want: Option<&str>) -> bool {
        match want {
            None => true,
            Some(m) => entry.get("matcher").and_then(|x| x.as_str()) == Some(m),
        }
    }

    /// 该条目的 matcher 相符、且其 hooks 里有我方 reporter。
    pub fn claims_here(
        spec: &HookSpec,
        entry: &Value,
        matcher: Option<&str>,
        agent_id: &str,
    ) -> bool {
        matcher_is(entry, matcher)
            && entry
                .get("hooks")
                .and_then(|x| x.as_array())
                .is_some_and(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| spec.command.claim(c, agent_id).is_some())
                    })
                })
    }

    /// 同一 (事件, matcher) 下**只留一条**我方 reporter hook：第一条更新为当前路径，其余删除。
    /// 返回是否有改动。
    ///
    /// 去重不是洁癖，是止血。claude/codex 都会把同事件下的条目**逐条执行**，重复 N 条就是每次
    /// 事件派生 N 个 reporter 进程。而重复一旦产生（两代品牌的 app 交替接线时互不认领对方的
    /// reporter、并发写、或手动跑过 install-hooks.mjs），旧实现只在「一条都没有」时才追加、
    /// 从不清理，于是污染每接线一轮翻一倍、永不收敛——真实机器上滚到了每事件 4 条。
    ///
    /// 用户自有的 hook（claim 不认领的）一概不动；条目里的我方 hook 被删空后，整条壳也移除。
    pub fn dedupe_event(
        spec: &HookSpec,
        arr: &mut Vec<Value>,
        matcher: Option<&str>,
        reporter: &str,
        desired_cmd: &str,
        desired_timeout: u64,
        agent_id: &str,
    ) -> bool {
        let mut changed = false;
        let mut kept = false; // 是否已保留过一条我方 hook
        let mut i = 0;
        while i < arr.len() {
            if !matcher_is(&arr[i], matcher) {
                i += 1;
                continue; // 别的 matcher（含用户自有）→ 不动
            }
            let Some(hs) = arr[i].get_mut("hooks").and_then(|x| x.as_array_mut()) else {
                i += 1;
                continue;
            };
            let mut j = 0;
            while j < hs.len() {
                let claimed = hs[j]
                    .get("command")
                    .and_then(|c| c.as_str())
                    .and_then(|c| spec.command.claim(c, agent_id));
                match claimed {
                    Some(path) if !kept => {
                        kept = true;
                        // 路径过期 → 更新；命令**形态**不满足 spec 也要迁移——gemini 在 Windows
                        // 必须带 PowerShell 调用运算符 `&`（无它则解析失败、hook 恒挂），只比路径
                        // 会让旧形态永远留在配置里继续失败。刻意不做整串强一致：手工接线的等价
                        // 形态（裸路径 vs 引号）原样保留，幂等按解析后内容判定。
                        let form_bad = spec.command.ps_call_operator
                            && cfg!(windows)
                            && !hs[j]
                                .get("command")
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .trim_start()
                                .starts_with('&');
                        if path != reporter || form_bad {
                            hs[j]["command"] = serde_json::json!(desired_cmd);
                            changed = true;
                        }
                        // 普通 hook 沿用用户/旧版本已有 timeout；只有声明了非默认值的 hook 需要
                        // 迁移：PermissionRequest 的长等待（310s），以及 gemini 全部事件——它的
                        // timeout 单位是**毫秒**（hookRunner `setTimeout(...,timeout)`，默认 6e4），
                        // 按 claude 的秒语义写 5 会让 hook 5ms 即被整树击杀。
                        if desired_timeout != 5
                            && hs[j].get("timeout").and_then(|v| v.as_u64())
                                != Some(desired_timeout)
                        {
                            hs[j]["timeout"] = serde_json::json!(desired_timeout);
                            changed = true;
                        }
                        j += 1;
                    }
                    Some(_) => {
                        hs.remove(j); // 第 2+ 条 → 重复注册，删
                        changed = true;
                    }
                    None => j += 1, // 用户自有 hook，不动
                }
            }
            if hs.is_empty() {
                arr.remove(i); // 我方 hook 删空后的空壳条目，别留
                changed = true;
            } else {
                i += 1;
            }
        }
        changed
    }
}

// ═══ ClaudeJson ═══

mod claude_json {
    use super::*;
    use serde_json::{json, Value};

    pub fn ensure_hooks(
        spec: &HookSpec,
        cur_text: &str,
        reporter: &str,
        agent_id: &str,
    ) -> EnsureOutcome {
        // 解析失败 / 顶层非对象 → 绝不覆盖用户文件（settings.json 还装着 statusLine 等用户配置）。
        let Some(mut root) = json_common::parse(cur_text) else {
            return EnsureOutcome::Abandon(RepairReason::ConfigUnreadable);
        };
        match merge(spec, &mut root, reporter, agent_id) {
            Merge::Abandon => EnsureOutcome::Abandon(RepairReason::ConfigUnreadable),
            Merge::Unchanged => EnsureOutcome::Unchanged,
            Merge::Changed => json_common::render(&root),
        }
    }

    enum Merge {
        Changed,
        Unchanged,
        Abandon,
    }

    /// 幂等合并。与 codex 版的唯一差别：定位/追加/去重均**按 matcher 区分**——同一事件下用户自有的
    /// 其他 matcher 条目（如 `PreToolUse:Bash` 预检）原封不动。
    fn merge(spec: &HookSpec, root: &mut Value, reporter: &str, agent_id: &str) -> Merge {
        let desired_cmd = spec.command.render(reporter, agent_id);
        // 旧实现在此直接 `json!({})` 覆盖，会写坏用户手改成非 object 的 hooks 键。
        let mut changed = match json_common::ensure_hooks_object(root) {
            Some(c) => c,
            None => return Merge::Abandon,
        };
        let Some(hooks) = root["hooks"].as_object_mut() else {
            return Merge::Abandon;
        };

        for ev in spec.events {
            let entry_val = hooks
                .entry(ev.name.to_string())
                .or_insert_with(|| json!([]));
            let Some(arr) = entry_val.as_array_mut() else {
                continue; // 事件值存在但非 array（畸形形状）：跳过该事件不动，不置空覆盖。
            };
            // 认领 + 更新命令串 + 删重复，一并在此完成。
            changed |= json_common::dedupe_event(
                spec,
                arr,
                ev.matcher,
                reporter,
                &desired_cmd,
                ev.timeout,
                agent_id,
            );
            if !arr
                .iter()
                .any(|e| json_common::claims_here(spec, e, ev.matcher, agent_id))
            {
                let mut entry = json!({ "hooks": [{ "type": "command", "command": desired_cmd, "timeout": ev.timeout }] });
                if let Some(m) = ev.matcher {
                    entry["matcher"] = json!(m);
                }
                arr.push(entry);
                changed = true;
            }
        }
        if changed {
            Merge::Changed
        } else {
            Merge::Unchanged
        }
    }
}

// ═══ CodexJson ═══

mod codex_json {
    use super::*;
    use serde_json::{json, Value};

    pub fn ensure_hooks(
        spec: &HookSpec,
        cur_text: &str,
        reporter: &str,
        agent_id: &str,
    ) -> EnsureOutcome {
        // 解析失败 / 顶层非对象 → 绝不覆盖用户文件。
        let Some(mut root) = json_common::parse(cur_text) else {
            return EnsureOutcome::Abandon(RepairReason::ConfigUnreadable);
        };
        match merge(spec, &mut root, reporter, agent_id) {
            Merge::Abandon => EnsureOutcome::Abandon(RepairReason::ConfigUnreadable),
            Merge::Unchanged => EnsureOutcome::Unchanged,
            Merge::Changed => json_common::render(&root),
        }
    }

    enum Merge {
        Changed,
        Unchanged,
        Abandon,
    }

    fn merge(spec: &HookSpec, root: &mut Value, reporter: &str, agent_id: &str) -> Merge {
        let desired_cmd = spec.command.render(reporter, agent_id);
        #[cfg(windows)]
        let desired_windows_cmd = codex_windows_command(reporter, agent_id);
        // 键不存在：hooks.json 整个文件本就可从空态建，与 kimi「config.toml 缺失即未登录」
        // 不同是有意的——此处不存在不代表用户手改过畸形内容。
        // 键存在但非 object（手改坏形状）：放弃不写，绝不覆盖用户文件。
        let mut changed = match json_common::ensure_hooks_object(root) {
            Some(c) => c,
            None => return Merge::Abandon,
        };
        let Some(hooks) = root["hooks"].as_object_mut() else {
            return Merge::Abandon;
        };

        for ev in spec.events {
            let entry_val = hooks
                .entry(ev.name.to_string())
                .or_insert_with(|| json!([]));
            let Some(arr) = entry_val.as_array_mut() else {
                continue; // 事件值存在但非 array（畸形形状）：跳过该事件不动，不置空覆盖。
            };
            // codex 的条目无 matcher → 传 None（同事件下我方 hook 只应有一条）。
            changed |= json_common::dedupe_event(
                spec,
                arr,
                None,
                reporter,
                &desired_cmd,
                ev.timeout,
                agent_id,
            );
            // Codex 在 Windows 上可能通过 PowerShell 执行 hook；以引号开头的通用 command
            // 会被当成字符串表达式并以 code 1 失败。官方支持 commandWindows 覆盖，因此显式
            // 用一个独立 PowerShell 进程执行 reporter，这在 cmd 与 PowerShell 外层都可工作。
            #[cfg(windows)]
            for entry in arr.iter_mut() {
                let Some(handlers) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                    continue;
                };
                for handler in handlers {
                    let ours = handler
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|cmd| spec.command.claim(cmd, agent_id).is_some());
                    if ours
                        && handler.get("commandWindows").and_then(Value::as_str)
                            != Some(desired_windows_cmd.as_str())
                    {
                        handler["commandWindows"] = json!(desired_windows_cmd.clone());
                        changed = true;
                    }
                }
            }
            if !arr
                .iter()
                .any(|e| json_common::claims_here(spec, e, None, agent_id))
            {
                let handler =
                    json!({ "type": "command", "command": desired_cmd, "timeout": ev.timeout });
                #[cfg(windows)]
                let handler = {
                    let mut handler = handler;
                    handler["commandWindows"] = json!(desired_windows_cmd.clone());
                    handler
                };
                arr.push(json!({ "hooks": [handler] }));
                changed = true;
            }
        }
        if changed {
            Merge::Changed
        } else {
            Merge::Unchanged
        }
    }

    #[cfg(windows)]
    fn codex_windows_command(reporter: &str, agent_id: &str) -> String {
        let reporter = reporter.replace('\'', "''");
        format!(
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command \"& '{reporter}' --provider {agent_id}\""
        )
    }
}

// ═══ OpencodeTs ═══

/// opencode 的接线产物是一份**由 meowo 全权生成的 TS 桥接插件**，不是与用户共处的配置文件。
/// 因此这里没有「合并」，只有「生成 / 比对」：目标全文算出来，与磁盘上的逐字比较，不同即重写。
///
/// reporter 路径以 **JSON 字符串字面量**嵌入。这一步不是偷懒——Windows 路径里的 `\` 若直接塞进
/// TS 源码就是转义符（`C:\Users\...` 里的 `\U` 会让插件加载直接语法错误）。JSON 的字符串转义
/// 恰好是合法 JS 字符串字面量的子集，`serde_json` 来回一趟就同时解决了写入转义与读回解析。
mod opencode_ts {
    use super::*;

    /// 待填的两个洞。用占位符而非 `format!`：模板里全是 JS 的 `{}`，走 `format!` 就得把每一个都
    /// 写成 `{{}}`，模板会变得没法读、也没法直接拷进编辑器验证。
    const REPORTER_SLOT: &str = "__MEOWO_REPORTER__";
    const PROVIDER_SLOT: &str = "__MEOWO_PROVIDER__";

    /// 认领标记：`claimed` 靠它把路径捞回来，故它与模板里的写法必须一字不差。
    const REPORTER_DECL: &str = "const MEOWO_REPORTER = ";

    /// 桥接插件模板。事件映射见 [`crate::plugins::opencode`] 的模块文档。
    const TEMPLATE: &str = r#"// meowo-reporter bridge —— 由 Meowo 生成，请勿手改：下次接线会整体覆盖本文件。
//
// opencode 没有 command hook（只认 TS 插件），所以 meowo 的「接线」在这里落成一份桥接：
// 订阅 opencode 的事件，把负载喂进 meowo-reporter 的 stdin。
//
// 负载刻意与 claude/codex/kimi 的 hook 同形（hook_event_name / session_id / cwd），于是
// reporter 侧不必为 opencode 单开一条解析路径——它根本看不出这一条是插件转发来的。

const MEOWO_REPORTER = __MEOWO_REPORTER__
const MEOWO_PROVIDER = __MEOWO_PROVIDER__

function meowoReport(payload) {
  // 没有 session_id 就无从落库，直接丢弃（reporter 侧同样会拒，这里省一次进程派生）。
  if (!payload.session_id) return
  try {
    const proc = Bun.spawn([MEOWO_REPORTER, "--provider", MEOWO_PROVIDER], {
      stdin: "pipe",
      stdout: "ignore",
      stderr: "ignore",
    })
    proc.stdin.write(JSON.stringify(payload))
    proc.stdin.end()
    // 别让这个短命子进程拖住 opencode 自己的退出。
    proc.unref?.()
  } catch {
    // 上报是尽力而为：出了任何岔子都不能把宿主 opencode 带崩。
  }
}

export const MeowoReporter = async ({ directory }) => ({
  event: async ({ event }) => {
    const props = event?.properties ?? {}
    switch (event?.type) {
      case "session.created":
        meowoReport({
          hook_event_name: "SessionStart",
          session_id: props.info?.id,
          cwd: props.info?.directory ?? directory,
        })
        break
      // opencode 每个回合跑完都发一次 idle——这正是 claude 的 Stop 语义。
      case "session.idle":
        meowoReport({ hook_event_name: "Stop", session_id: props.sessionID, cwd: directory })
        break
      // opencode 没有「会话结束」这一概念，删除会话是唯一确定的终结信号。进程退出的收尾
      // 因此与 codex 一样，交给 meowo-app 的判活去 reap，不在这里硬凑一个 SessionEnd。
      case "session.deleted":
        meowoReport({
          hook_event_name: "SessionEnd",
          session_id: props.info?.id,
          cwd: props.info?.directory ?? directory,
        })
        break
      // 压缩完成 ≈ PostCompact。session id 字段名按 session.idle 的 props.sessionID 类推，
      // 未经真机压缩验证；取不到时 meowoReport 静默丢弃（模板风格本来就是尽力而为）。
      case "session.compacted":
        meowoReport({ hook_event_name: "PostCompact", session_id: props.sessionID, cwd: directory })
        break
    }
  },
  // 压缩开始 ≈ PreCompact。session id 字段名按 chat.message 的 input?.sessionID 类推，
  // 同样未经真机压缩验证。
  "experimental.session.compacting": async (input) => {
    meowoReport({ hook_event_name: "PreCompact", session_id: input?.sessionID, cwd: directory })
  },
  "chat.message": async (input, output) => {
    const text = (output?.parts ?? [])
      .filter((part) => part?.type === "text" && typeof part.text === "string")
      .map((part) => part.text)
      .join("")
    meowoReport({
      hook_event_name: "UserPromptSubmit",
      session_id: input?.sessionID,
      cwd: directory,
      prompt: text,
    })
  },
  "tool.execute.after": async (input) => {
    meowoReport({
      hook_event_name: "PostToolUse",
      session_id: input?.sessionID,
      cwd: directory,
      tool_name: input?.tool,
    })
  },
})
"#;

    /// 生成插件全文。
    pub fn render_plugin(reporter: &str, agent_id: &str) -> String {
        TEMPLATE
            .replace(REPORTER_SLOT, &js_string(reporter))
            .replace(PROVIDER_SLOT, &js_string(agent_id))
    }

    /// Rust 串 → JS 字符串字面量（含引号）。
    fn js_string(s: &str) -> String {
        serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
    }

    /// 目标全文与现状逐字比对——一致即已是目标状态，否则整体重写。
    pub fn ensure_hooks(cur_text: &str, reporter: &str, agent_id: &str) -> EnsureOutcome {
        let next = render_plugin(reporter, agent_id);
        if cur_text == next {
            EnsureOutcome::Unchanged
        } else {
            EnsureOutcome::Changed(next)
        }
    }

    /// 从既有插件里取回 reporter 路径。仍走与另三家同一条严格判定（文件名必须是
    /// `meowo-reporter[.exe]` 或历史 `cc-reporter[.exe]`）：万一用户在同名文件里放了自己的东西，
    /// 我们宁可不认领，也不能把它的路径当成自己的 reporter 写回去。
    pub fn claimed(cur_text: &str) -> Option<String> {
        let rest = cur_text.split_once(REPORTER_DECL)?.1;
        let line = rest.lines().next()?.trim();
        let path: String = serde_json::from_str(line).ok()?;
        is_any_reporter(&path).then_some(path)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 生成 → 认领 → 再生成，必须闭环且幂等。Windows 路径的反斜杠是这里的真正考点：
        /// 它一旦没被转义，产出的就是一份加载即语法错误的插件。
        #[test]
        fn windows_path_survives_render_and_claim_roundtrip() {
            let win = r"C:\Users\larry\AppData\Local\Meowo\meowo-reporter.exe";
            let out = render_plugin(win, "opencode");

            // 路径以转义形态落进源码——绝不能出现裸的单反斜杠。
            assert!(out.contains(
                r#"const MEOWO_REPORTER = "C:\\Users\\larry\\AppData\\Local\\Meowo\\meowo-reporter.exe""#
            ));
            assert!(out.contains(r#"const MEOWO_PROVIDER = "opencode""#));

            // 认领读回的是原始路径（反转义后）。
            assert_eq!(claimed(&out).as_deref(), Some(win));
            // 幂等。
            assert_eq!(
                ensure_hooks(&out, win, "opencode"),
                EnsureOutcome::Unchanged
            );
            // 换了路径 → 重写。
            let moved = r"D:\meowo\meowo-reporter.exe";
            assert_eq!(
                ensure_hooks(&out, moved, "opencode"),
                EnsureOutcome::Changed(render_plugin(moved, "opencode"))
            );
        }

        #[test]
        fn creates_from_empty_and_claims_legacy_reporter() {
            let unix = "/usr/local/bin/meowo-reporter";
            assert_eq!(
                ensure_hooks("", unix, "opencode"),
                EnsureOutcome::Changed(render_plugin(unix, "opencode"))
            );

            // 历史 cc-reporter 认领得到（供升级时替换旧路径），但外层 `claimed_reporter` 会用
            // `is_current_reporter` 把它滤掉，于是接线改写成当前 reporter——与另三家同一套语义。
            let legacy = render_plugin("/old/cc-reporter", "opencode");
            assert_eq!(claimed(&legacy).as_deref(), Some("/old/cc-reporter"));
        }

        /// 认领必须严格：同名文件里若是用户自己的东西，绝不能把它的路径当成我们的 reporter。
        #[test]
        fn never_claims_a_foreign_binary() {
            let foreign = TEMPLATE
                .replace(REPORTER_SLOT, "\"/opt/not-us/notify.sh\"")
                .replace(PROVIDER_SLOT, "\"opencode\"");
            assert_eq!(claimed(&foreign), None);
            // 压根不是我们的插件（没有那行声明）。
            assert_eq!(claimed("export const Whatever = async () => ({})\n"), None);
            assert_eq!(claimed(""), None);
        }
    }
}

/// 解析 hooks.json / settings.json 文本（容忍 BOM，顶层须为对象）。供 meowo-app 的 codex
/// trusted_hash 步骤与 claude statusLine 改写复用，免得它们各自再实现一遍 BOM 容忍。
pub fn parse_json_config(text: &str) -> Option<serde_json::Value> {
    json_common::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// kimi 的接线规格（与 plugins/kimi.rs 同源，此处独立声明以免测试依赖插件表的演化）。
    const KIMI_CMD: CommandSpec = CommandSpec {
        quote_exe: false,
        with_provider: true,
        ps_call_operator: false,
    };
    static KIMI_EVENTS: [HookEvent; 6] = [
        HookEvent::plain("SessionStart"),
        HookEvent::plain("UserPromptSubmit"),
        HookEvent::plain("PostToolUse"),
        HookEvent::plain("Stop"),
        HookEvent::plain("SessionEnd"),
        HookEvent::plain("PermissionRequest"),
    ];
    static KT: HookSpec = HookSpec {
        config_rel: "config.toml",
        format: ConfigFormat::KimiToml,
        missing: MissingConfig::Fail(RepairReason::NeedLogin),
        events: &KIMI_EVENTS,
        command: KIMI_CMD,
    };

    fn changed(text: &str, reporter: &str) -> String {
        match KT.ensure_hooks(text, reporter, "kimi") {
            EnsureOutcome::Changed(s) => s,
            other => panic!("期望 Changed，实得 {other:?}"),
        }
    }

    /// claude 的接线规格（与 plugins/claude.rs 同源，此处独立声明以免测试依赖插件表的演化）。
    const CLAUDE_CMD: CommandSpec = CommandSpec {
        quote_exe: true,
        with_provider: false,
        ps_call_operator: false,
    };
    static CLAUDE_EVENTS: [HookEvent; 8] = [
        HookEvent::matched("SessionStart", "*"),
        HookEvent::matched("UserPromptSubmit", "*"),
        HookEvent::matched("PostToolUse", "*"),
        HookEvent::matched("Stop", "*"),
        HookEvent::matched("SessionEnd", "*"),
        HookEvent::matched("PermissionRequest", "*"),
        HookEvent::matched("PreToolUse", "AskUserQuestion"),
        HookEvent::matched("PreToolUse", "ExitPlanMode"),
    ];
    static CJ: HookSpec = HookSpec {
        config_rel: "settings.json",
        format: ConfigFormat::ClaudeJson,
        missing: MissingConfig::CreateFrom("{}"),
        events: &CLAUDE_EVENTS,
        command: CLAUDE_CMD,
    };

    /// 跑一次 claude 接线，要求有改动，返回解析后的 settings。
    fn claude_changed(text: &str, reporter: &str) -> serde_json::Value {
        match CJ.ensure_hooks(text, reporter, "claude") {
            EnsureOutcome::Changed(s) => serde_json::from_str(&s).expect("产物应为合法 JSON"),
            other => panic!("期望 Changed，实得 {other:?}"),
        }
    }

    /// gemini 形态的接线规格（ClaudeJson + `--provider` + PowerShell 调用运算符 + 毫秒 timeout）。
    /// 与 plugins/gemini 同源，独立声明以免测试依赖插件表的演化。
    static GEMINI_EVENTS: [HookEvent; 2] = [
        HookEvent::plain("SessionStart").with_timeout(5_000),
        HookEvent::plain("AfterAgent").with_timeout(5_000),
    ];
    static GJ: HookSpec = HookSpec {
        config_rel: "settings.json",
        format: ConfigFormat::ClaudeJson,
        missing: MissingConfig::CreateFrom("{}"),
        events: &GEMINI_EVENTS,
        command: CommandSpec {
            quote_exe: true,
            with_provider: true,
            ps_call_operator: true,
        },
    };

    /// 已按旧版接线的 gemini 配置（无 `&` 前缀 + 秒语义 timeout 5）必须被原地迁移：
    /// gemini 把 5 当 5**毫秒**，reporter 一 spawn 即被整树击杀；Windows 上无 `&` 的引号命令
    /// 在 PowerShell 里是字符串表达式、以 code 1 失败。两者都不迁移，会话就永远落不了库。
    #[test]
    fn gemini_migrates_legacy_seconds_timeout_and_command_form() {
        let ccr = "C:/x/meowo-reporter.exe";
        let src = format!(
            r#"{{"hooks":{{
                "SessionStart":[{{"hooks":[{{"type":"command","command":"\"{ccr}\" --provider gemini","timeout":5}}]}}],
                "AfterAgent":[{{"hooks":[{{"type":"command","command":"\"{ccr}\" --provider gemini","timeout":5}}]}}]
            }}}}"#
        );
        let out = match GJ.ensure_hooks(&src, ccr, "gemini") {
            EnsureOutcome::Changed(s) => {
                serde_json::from_str::<serde_json::Value>(&s).expect("产物应为合法 JSON")
            }
            other => panic!("旧接线应被迁移（Changed），实得 {other:?}"),
        };
        let desired = GJ.command.render(ccr, "gemini");
        for ev in ["SessionStart", "AfterAgent"] {
            let h = &out["hooks"][ev][0]["hooks"][0];
            assert_eq!(
                h["command"],
                serde_json::json!(desired),
                "{ev} 命令形态未迁移"
            );
            assert_eq!(
                h["timeout"],
                serde_json::json!(5_000),
                "{ev} timeout 未迁移到毫秒"
            );
        }
        // 幂等：迁移后的产物再跑一遍必须 Unchanged，不得反复改写用户文件。
        let migrated = serde_json::to_string(&out).unwrap();
        assert_eq!(
            GJ.ensure_hooks(&migrated, ccr, "gemini"),
            EnsureOutcome::Unchanged
        );
    }

    // ── ClaudeJson ──

    #[test]
    fn claude_claim_never_touches_user_hooks() {
        // 我们写入的形态：带引号的单可执行路径，无参数。
        assert_eq!(
            CLAUDE_CMD
                .claim("\"C:/x/meowo-reporter.exe\"", "claude")
                .as_deref(),
            Some("C:/x/meowo-reporter.exe")
        );
        assert_eq!(
            CLAUDE_CMD
                .claim("/usr/local/bin/meowo-reporter", "claude")
                .as_deref(),
            Some("/usr/local/bin/meowo-reporter")
        );
        // 不能误伤用户自有 hook：带参数、是别的脚本、或只是路径里含子串。
        // `node tools/meowo-reporter-notify.js` 是这条纪律的原始反例，务必保住。
        assert_eq!(
            CLAUDE_CMD.claim("node tools/meowo-reporter-notify.js", "claude"),
            None
        );
        assert_eq!(
            CLAUDE_CMD.claim("\"C:/x/meowo-reporter.exe\" --flag", "claude"),
            None
        );
        assert_eq!(
            CLAUDE_CMD.claim("/opt/meowo-reporter/run.sh", "claude"),
            None
        );
        assert_eq!(CLAUDE_CMD.claim("meowo-reporter-wrapper", "claude"), None);
        assert_eq!(CLAUDE_CMD.claim("", "claude"), None);
    }

    #[test]
    fn claude_ensure_hooks_adds_all_specs_including_pretooluse_matchers() {
        let v = claude_changed("{}", "C:/x/meowo-reporter.exe");
        for e in [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "Stop",
            "SessionEnd",
            "PermissionRequest",
        ] {
            assert_eq!(v["hooks"][e][0]["matcher"], "*", "{e} matcher");
            assert_eq!(
                v["hooks"][e][0]["hooks"][0]["command"],
                "\"C:/x/meowo-reporter.exe\""
            );
        }
        // PreToolUse：两条，matcher 分别 AskUserQuestion / ExitPlanMode。
        let matchers: Vec<&str> = v["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["matcher"].as_str().unwrap())
            .collect();
        assert!(matchers.contains(&"AskUserQuestion"));
        assert!(matchers.contains(&"ExitPlanMode"));
        // 幂等。
        let text = serde_json::to_string(&v).unwrap();
        assert_eq!(
            CJ.ensure_hooks(&text, "C:/x/meowo-reporter.exe", "claude"),
            EnsureOutcome::Unchanged
        );
    }

    /// 回归（重复注册）：真机 settings.json 的中毒现场——每个事件下 4 条一模一样的 reporter
    /// 条目（PreToolUse 是 4×2 个 matcher = 8 条）。Claude Code 会逐条执行，于是每次事件派生
    /// 4 个 reporter 进程。旧实现只在「一条都没有」时才追加、从不清理，污染永不收敛。
    /// 接线必须把它收敛回每 (事件, matcher) 一条。
    #[test]
    fn claude_ensure_hooks_collapses_duplicate_reporter_entries() {
        let ccr = "C:/x/meowo-reporter.exe";
        let dup = |matcher: &str, n: usize| -> Vec<serde_json::Value> {
            (0..n)
                .map(|_| {
                    json!({ "matcher": matcher, "hooks": [
                    { "type": "command", "command": format!("\"{ccr}\""), "timeout": 5 }] })
                })
                .collect()
        };
        let mut hooks = serde_json::Map::new();
        for ev in [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "Stop",
            "SessionEnd",
            "PermissionRequest",
        ] {
            hooks.insert(ev.into(), json!(dup("*", 4)));
        }
        let mut pre = dup("AskUserQuestion", 4);
        pre.extend(dup("ExitPlanMode", 4));
        hooks.insert("PreToolUse".into(), json!(pre));
        let src = json!({ "hooks": hooks }).to_string();

        let v = claude_changed(&src, ccr);
        for ev in [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "Stop",
            "SessionEnd",
            "PermissionRequest",
        ] {
            assert_eq!(
                v["hooks"][ev].as_array().unwrap().len(),
                1,
                "{ev} 应收敛为 1 条"
            );
        }
        // PreToolUse：两个 matcher 各留 1 条。
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2, "PreToolUse 应收敛为 2 条（每 matcher 一条）");
        for m in ["AskUserQuestion", "ExitPlanMode"] {
            assert_eq!(
                pre.iter().filter(|e| e["matcher"] == m).count(),
                1,
                "{m} 应恰一条"
            );
        }
        // 收敛后幂等。
        assert_eq!(
            CJ.ensure_hooks(&v.to_string(), ccr, "claude"),
            EnsureOutcome::Unchanged
        );
    }

    /// 去重必须只删**我方**的重复，用户自有 hook（含同 matcher 下并存的）一条都不能碰。
    #[test]
    fn claude_dedupe_never_touches_user_hooks() {
        let ccr = "C:/x/meowo-reporter.exe";
        let src = format!(
            r#"{{"hooks":{{"SessionStart":[
                {{"matcher":"*","hooks":[{{"type":"command","command":"node other.js"}}]}},
                {{"matcher":"*","hooks":[{{"type":"command","command":"\"{ccr}\"","timeout":5}}]}},
                {{"matcher":"*","hooks":[
                    {{"type":"command","command":"\"{ccr}\"","timeout":5}},
                    {{"type":"command","command":"node keep-me.js"}}]}},
                {{"matcher":"*","hooks":[{{"type":"command","command":"\"{ccr}\"","timeout":5}}]}}
            ]}}}}"#
        );
        let v = claude_changed(&src, ccr);
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        // 3 条我方重复 → 只留第一条；两条用户 hook 全在（第三条壳因还有 keep-me.js 而保留）。
        let mine = arr
            .iter()
            .flat_map(|e| e["hooks"].as_array().unwrap())
            .filter(|h| h["command"] == format!("\"{ccr}\""))
            .count();
        assert_eq!(mine, 1, "我方 hook 应只剩一条");
        let cmds: Vec<&str> = arr
            .iter()
            .flat_map(|e| e["hooks"].as_array().unwrap())
            .filter_map(|h| h["command"].as_str())
            .collect();
        assert!(cmds.contains(&"node other.js"), "用户 hook 不能被删");
        assert!(
            cmds.contains(&"node keep-me.js"),
            "与我方 hook 同壳的用户 hook 不能被删"
        );
        assert_eq!(
            CJ.ensure_hooks(&v.to_string(), ccr, "claude"),
            EnsureOutcome::Unchanged
        );
    }

    #[test]
    fn claude_ensure_hooks_preserves_user_pretooluse_bash() {
        // 用户自有 PreToolUse:Bash node 预检，不是 meowo-reporter。
        let src = r#"{"hooks":{"PreToolUse":[
            {"matcher":"Bash","hooks":[{"type":"command","command":"node \"x/pre-check.cjs\""}]}
        ]}}"#;
        let v = claude_changed(src, "C:/x/meowo-reporter.exe");
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        // 原 Bash 条目原封保留。
        let bash = pre.iter().find(|e| e["matcher"] == "Bash").unwrap();
        assert_eq!(bash["hooks"][0]["command"], "node \"x/pre-check.cjs\"");
        // 且新增了 AskUserQuestion / ExitPlanMode 两条 meowo-reporter。
        assert!(pre.iter().any(|e| e["matcher"] == "AskUserQuestion"));
        assert!(pre.iter().any(|e| e["matcher"] == "ExitPlanMode"));
    }

    #[test]
    fn claude_ensure_hooks_updates_changed_path_and_keeps_other_hooks() {
        // 同一 matcher 下：一个别的 hook + 一个旧路径的 meowo-reporter。
        let src = r#"{"hooks":{"SessionStart":[
            {"matcher":"*","hooks":[{"type":"command","command":"node other.js"}]},
            {"matcher":"*","hooks":[{"type":"command","command":"\"C:/old/meowo-reporter.exe\"","timeout":5}]}
        ]}}"#;
        let v = claude_changed(src, "C:/new/meowo-reporter.exe");
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "node other.js"
        );
        assert_eq!(
            v["hooks"]["SessionStart"][1]["hooks"][0]["command"],
            "\"C:/new/meowo-reporter.exe\""
        );
        // 没有重复追加（该事件仍是 2 条）。
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn claude_ensure_hooks_claims_legacy_cc_reporter() {
        // 行为改进（与 codex 同）：SessionStart 上挂着废弃的 cc-reporter 时认领并更新，
        // 而非不认领、重复追加一条。
        let src = r#"{"hooks":{"SessionStart":[
            {"matcher":"*","hooks":[{"type":"command","command":"\"C:/old/cc-reporter.exe\"","timeout":5}]}
        ]}}"#;
        let v = claude_changed(src, "C:/new/meowo-reporter.exe");
        assert_eq!(
            v["hooks"]["SessionStart"].as_array().unwrap().len(),
            1,
            "应认领而非追加"
        );
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "\"C:/new/meowo-reporter.exe\""
        );
    }

    #[test]
    fn claude_abandons_when_hooks_key_is_not_object() {
        // 旧实现在此直接 `json!({})` 覆盖，会写坏用户文件。现在如实 Abandon。
        let out = CJ.ensure_hooks(r#"{"hooks":[1,2]}"#, "C:/x/meowo-reporter.exe", "claude");
        assert_eq!(out, EnsureOutcome::Abandon(RepairReason::ConfigUnreadable));
        // 顶层非对象、非法 JSON 同样放弃。
        assert!(matches!(
            CJ.ensure_hooks("[]", "r", "claude"),
            EnsureOutcome::Abandon(_)
        ));
        assert!(matches!(
            CJ.ensure_hooks("{not json", "r", "claude"),
            EnsureOutcome::Abandon(_)
        ));
    }

    #[test]
    fn claude_tolerates_utf8_bom_and_preserves_top_level_keys() {
        // Windows 编辑器/PowerShell 常写出带 BOM 的 JSON；顶层 statusLine 等无关键须原样保留。
        let src = "\u{feff}{\"statusLine\":{\"type\":\"command\",\"command\":\"hud\"},\"model\":\"opus\"}";
        let v = claude_changed(src, "C:/x/meowo-reporter.exe");
        assert_eq!(v["statusLine"]["command"], "hud");
        assert_eq!(v["model"], "opus");
        assert!(v["hooks"]["SessionStart"].is_array());
    }

    #[test]
    fn claude_has_reporter_requires_session_start_and_current_binary() {
        // 只在 Stop 挂了 meowo-reporter：不能保证新会话入库，不应判定为已接入。
        // claimed_reporter（广扫，供路径复用）仍会命中这条，语义刻意不同。
        let stop_only = r#"{"hooks":{"Stop":[{"matcher":"*","hooks":[
            {"type":"command","command":"\"C:/a/b/meowo-reporter.exe\"","timeout":5}]}]}}"#;
        assert!(!CJ.has_reporter(stop_only, "claude"));
        assert_eq!(
            CJ.claimed_reporter(stop_only, "claude").as_deref(),
            Some("C:/a/b/meowo-reporter.exe")
        );

        let session_start = r#"{"hooks":{"SessionStart":[{"matcher":"*","hooks":[
            {"type":"command","command":"\"C:/a/b/meowo-reporter.exe\"","timeout":5}]}]}}"#;
        assert!(CJ.has_reporter(session_start, "claude"));

        // 历史 cc-reporter 不算「已接入」，也不作为可复用路径（写回去 hooks 依旧失效）。
        let legacy = r#"{"hooks":{"SessionStart":[{"matcher":"*","hooks":[
            {"type":"command","command":"\"C:/a/b/cc-reporter.exe\"","timeout":5}]}]}}"#;
        assert!(!CJ.has_reporter(legacy, "claude"));
        assert_eq!(CJ.claimed_reporter(legacy, "claude"), None);

        assert!(!CJ.has_reporter("{}", "claude"));
    }

    #[test]
    fn claude_real_shape_user_settings_merge() {
        // 精确复刻真实 settings.json 结构：PreToolUse(node 预检) + 5 个 meowo-reporter 事件 + claude-hud statusLine。
        let ccr = "C:/Users/larry/workspace/meowo/target/release/meowo-reporter.exe";
        let src = format!(
            r#"{{"hooks":{{
                "PreToolUse":[{{"matcher":"Bash","hooks":[{{"type":"command","command":"node \"x/pre-commit-check.cjs\"","timeout":5000}}]}}],
                "SessionStart":[{{"matcher":"*","hooks":[{{"type":"command","command":"\"{ccr}\"","timeout":5}}]}}],
                "UserPromptSubmit":[{{"matcher":"*","hooks":[{{"type":"command","command":"\"{ccr}\"","timeout":5}}]}}],
                "PostToolUse":[{{"matcher":"*","hooks":[{{"type":"command","command":"\"{ccr}\"","timeout":5}}]}}],
                "Stop":[{{"matcher":"*","hooks":[{{"type":"command","command":"\"{ccr}\"","timeout":5}}]}}],
                "SessionEnd":[{{"matcher":"*","hooks":[{{"type":"command","command":"\"{ccr}\"","timeout":5}}]}}]
            }},"statusLine":{{"type":"command","command":"bash -c 'claude-hud stuff'"}}}}"#
        );
        // fixture 缺 PermissionRequest / PreToolUse(AskUserQuestion|ExitPlanMode) → 追加 3 条。
        let v = claude_changed(&src, ccr);
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "node \"x/pre-commit-check.cjs\""
        );
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(pre.iter().any(|e| e["matcher"] == "AskUserQuestion"));
        assert!(pre.iter().any(|e| e["matcher"] == "ExitPlanMode"));
        assert_eq!(v["hooks"]["PermissionRequest"][0]["matcher"], "*");
        // statusLine 原样保留（它的改写由 meowo-app 的 amend 负责，不在格式适配器里）。
        assert_eq!(v["statusLine"]["command"], "bash -c 'claude-hud stuff'");
        // 再跑一次：此时才幂等。
        let text = serde_json::to_string(&v).unwrap();
        assert_eq!(
            CJ.ensure_hooks(&text, ccr, "claude"),
            EnsureOutcome::Unchanged
        );
    }

    // ── CommandSpec ──

    #[test]
    fn render_covers_three_agent_shapes() {
        let claude = CommandSpec {
            quote_exe: true,
            with_provider: false,
            ps_call_operator: false,
        };
        let codex = CommandSpec {
            quote_exe: true,
            with_provider: true,
            ps_call_operator: false,
        };
        assert_eq!(
            claude.render("C:/x/meowo-reporter.exe", "claude"),
            "\"C:/x/meowo-reporter.exe\""
        );
        assert_eq!(
            codex.render("C:/x/meowo-reporter.exe", "codex"),
            "\"C:/x/meowo-reporter.exe\" --provider codex"
        );
        assert_eq!(
            KIMI_CMD.render("C:/x/meowo-reporter.exe", "kimi"),
            "C:/x/meowo-reporter.exe --provider kimi"
        );
    }

    #[test]
    fn claim_with_provider_is_strict() {
        // 认领：带引号/裸路径两种形态。
        assert_eq!(
            KIMI_CMD
                .claim("\"C:/x/meowo-reporter.exe\" --provider kimi", "kimi")
                .as_deref(),
            Some("C:/x/meowo-reporter.exe")
        );
        assert_eq!(
            KIMI_CMD
                .claim("C:/x/meowo-reporter.exe --provider kimi", "kimi")
                .as_deref(),
            Some("C:/x/meowo-reporter.exe")
        );
        // 历史遗留 cc-reporter 也认领，便于升级时替换旧路径。
        assert_eq!(
            KIMI_CMD
                .claim("C:/x/cc-reporter.exe --provider kimi", "kimi")
                .as_deref(),
            Some("C:/x/cc-reporter.exe")
        );
        // 拒绝：agent 不符 / 无参数 / 多余参数 / 别的可执行 / 子串陷阱。
        assert!(KIMI_CMD
            .claim("C:/x/meowo-reporter.exe --provider codex", "kimi")
            .is_none());
        assert!(KIMI_CMD
            .claim("\"C:/x/meowo-reporter.exe\"", "kimi")
            .is_none());
        assert!(KIMI_CMD
            .claim("C:/x/meowo-reporter.exe --provider kimi --v", "kimi")
            .is_none());
        assert!(KIMI_CMD
            .claim("node meowo-reporter-notify.js --provider kimi", "kimi")
            .is_none());
        assert!(KIMI_CMD
            .claim("C:/x/cc-reporter-not-us.exe --provider kimi", "kimi")
            .is_none());
    }

    #[test]
    fn claim_bare_quoted_rejects_any_argument() {
        // claude 形态：单个（可带引号的）可执行路径，禁带参数。
        let bare = CommandSpec {
            quote_exe: true,
            with_provider: false,
            ps_call_operator: false,
        };
        assert_eq!(
            bare.claim("\"C:/x/meowo-reporter.exe\"", "claude")
                .as_deref(),
            Some("C:/x/meowo-reporter.exe")
        );
        assert_eq!(
            bare.claim("/usr/local/bin/meowo-reporter", "claude")
                .as_deref(),
            Some("/usr/local/bin/meowo-reporter")
        );
        // 带参数 = 不是我们写的那条；用户自有 hook 一概不认领。
        assert!(bare
            .claim("\"C:/x/meowo-reporter.exe\" --flag", "claude")
            .is_none());
        assert!(bare
            .claim("node tools/meowo-reporter-notify.js", "claude")
            .is_none());
        assert!(bare.claim("/opt/meowo-reporter/run.sh", "claude").is_none());
        assert!(bare.claim("meowo-reporter-wrapper", "claude").is_none());
        assert!(bare.claim("", "claude").is_none());
    }

    // ── KimiToml::ensure_hooks ──

    #[test]
    fn adds_all_when_absent_and_preserves_content() {
        let src = "default_model = \"kimi-code/kimi-for-coding\"\n# 用户注释\n[loop_control]\nmax_steps_per_turn = 100\n";
        let out = changed(src, "C:/x/meowo-reporter.exe");
        assert!(out.contains("# 用户注释")); // 结构保持：注释仍在
        assert!(out.contains("max_steps_per_turn = 100"));
        for ev in KIMI_EVENTS {
            assert!(
                out.contains(&format!("event = \"{}\"", ev.name)),
                "{} 未写入",
                ev.name
            );
        }
        assert!(out.contains(r#"command = "C:/x/meowo-reporter.exe --provider kimi""#));
        assert_eq!(
            KT.ensure_hooks(&out, "C:/x/meowo-reporter.exe", "kimi"),
            EnsureOutcome::Unchanged
        );
    }

    #[test]
    fn adopts_manual_and_updates_stale_path() {
        let dev = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe";
        let mut src = String::from("theme = \"light\"\n");
        for ev in KIMI_EVENTS {
            src.push_str(&format!(
                "[[hooks]]\nevent = \"{}\"\ncommand = \"{dev} --provider kimi\"\ntimeout = 5\n\n",
                ev.name
            ));
        }
        // 路径一致：无改动。
        assert_eq!(KT.ensure_hooks(&src, dev, "kimi"), EnsureOutcome::Unchanged);
        // 路径失效换 sidecar：6 条 command 全部更新，用户键 theme 不动。
        let out = changed(&src, "C:/app/meowo-reporter.exe");
        assert_eq!(
            out.matches(r#"command = "C:/app/meowo-reporter.exe --provider kimi""#)
                .count(),
            6
        );
        assert!(out.contains("theme = \"light\""));
    }

    #[test]
    fn updates_legacy_cc_reporter_paths() {
        let mut src = String::from("theme = \"light\"\n");
        for ev in KIMI_EVENTS {
            src.push_str(&format!(
                "[[hooks]]\nevent = \"{}\"\ncommand = \"C:/x/cc-reporter.exe --provider kimi\"\ntimeout = 5\n\n",
                ev.name
            ));
        }
        // 接线前：SessionStart 上挂的是废弃的 cc-reporter → 不算已接入。
        assert!(!KT.has_reporter(&src, "kimi"));
        assert_eq!(KT.claimed_reporter(&src, "kimi"), None);

        let out = changed(&src, "C:/app/meowo-reporter.exe");
        assert_eq!(out.matches("cc-reporter").count(), 0);
        assert_eq!(
            out.matches(r#"command = "C:/app/meowo-reporter.exe --provider kimi""#)
                .count(),
            6
        );
        assert!(out.contains("theme = \"light\""));
        assert!(KT.has_reporter(&out, "kimi"));
        assert_eq!(
            KT.claimed_reporter(&out, "kimi").as_deref(),
            Some("C:/app/meowo-reporter.exe")
        );
        assert_eq!(
            KT.ensure_hooks(&out, "C:/app/meowo-reporter.exe", "kimi"),
            EnsureOutcome::Unchanged
        );
    }

    #[test]
    fn keeps_user_hook_entries() {
        let src =
            "[[hooks]]\nevent = \"Notification\"\ncommand = \"my-notify --ding\"\ntimeout = 3\n";
        let out = changed(src, "C:/x/meowo-reporter.exe");
        assert!(out.contains("my-notify --ding")); // 用户 hook 原样
        assert_eq!(out.matches("--provider kimi").count(), 6);
    }

    #[test]
    fn abandons_on_nonempty_or_malformed_hooks() {
        for src in ["hooks = [1, 2]\n", "hooks = \"oops\"\n"] {
            assert_eq!(
                KT.ensure_hooks(src, "C:/x/meowo-reporter.exe", "kimi"),
                EnsureOutcome::Abandon(RepairReason::ConfigUnreadable),
                "src={src:?}"
            );
        }
    }

    #[test]
    fn abandons_on_invalid_toml() {
        assert_eq!(
            KT.ensure_hooks("this is not = = toml\n", "C:/x/meowo-reporter.exe", "kimi"),
            EnsureOutcome::Abandon(RepairReason::ConfigUnreadable)
        );
    }

    #[test]
    fn replaces_empty_inline_array_from_legacy_kimi() {
        // 旧 Python 版 kimi-cli 的真实结构：顶层标量 + `hooks = []` 空内联数组 + 各 [section] 表。
        let src = "\
default_model = \"kimi-code/kimi-for-coding\"
theme = \"dark\"
hooks = []
merge_all_available_skills = true

[models.\"kimi-code/kimi-for-coding\"]
provider = \"managed:kimi-code\"

[providers.\"managed:kimi-code\"]
type = \"managed\"
api_key = \"secret-should-survive\"

[loop_control]
max_steps_per_turn = 100
";
        let out = changed(src, "C:/app/meowo-reporter.exe");
        // 输出必须仍是合法 TOML（array-of-tables 不能错位插到 section 里）。
        let reparsed: DocumentMut = out.parse().expect("产物应为合法 TOML");
        let arr = reparsed["hooks"]
            .as_array_of_tables()
            .expect("hooks 应为 array-of-tables");
        assert_eq!(arr.len(), 6);
        assert_eq!(
            out.matches(r#"command = "C:/app/meowo-reporter.exe --provider kimi""#)
                .count(),
            6
        );
        // 其余键与 [section] 表原样保留（含 api_key，绝不能丢）。
        assert_eq!(
            reparsed["default_model"].as_str(),
            Some("kimi-code/kimi-for-coding")
        );
        assert_eq!(reparsed["merge_all_available_skills"].as_bool(), Some(true));
        assert_eq!(
            reparsed["providers"]["managed:kimi-code"]["api_key"].as_str(),
            Some("secret-should-survive")
        );
        assert_eq!(
            reparsed["loop_control"]["max_steps_per_turn"].as_integer(),
            Some(100)
        );
        assert_eq!(
            KT.ensure_hooks(&out, "C:/app/meowo-reporter.exe", "kimi"),
            EnsureOutcome::Unchanged
        );
    }

    /// 回归：reporter 路径含空白（如 `C:\Users\Zhang San\...`）时命令必须写成带引号形态。
    /// 修复前裸写的命令被 kimi 与 `parse_hook_command` 同按第一个空白截断：hook 执行失败，
    /// 认领也永远认不回去——每轮接线再追加一条孤儿条目，配置单调膨胀。
    #[test]
    fn quotes_command_when_reporter_path_contains_whitespace() {
        let spaced = "C:/Users/Zhang San/AppData/Local/Meowo/meowo-reporter.exe";
        // render：含空白 → 带引号；无空白 → 维持裸形态（无空格机器上的既有条目不因本修复被改写）。
        assert_eq!(
            KIMI_CMD.render(spaced, "kimi"),
            format!("\"{spaced}\" --provider kimi")
        );
        assert_eq!(
            KIMI_CMD.render("C:/x/meowo-reporter.exe", "kimi"),
            "C:/x/meowo-reporter.exe --provider kimi"
        );
        // 带引号形态能认回完整路径；裸形态确实会被截断而认不回（这正是修复前的膨胀机制）。
        assert_eq!(
            KIMI_CMD
                .claim(&KIMI_CMD.render(spaced, "kimi"), "kimi")
                .as_deref(),
            Some(spaced)
        );
        assert_eq!(
            KIMI_CMD.claim(
                "C:/Users/Zhang San/meowo-reporter.exe --provider kimi",
                "kimi"
            ),
            None
        );

        // 端到端：6 个事件的命令都含带引号的路径。
        let out = changed("theme = \"dark\"\n", spaced);
        let quoted = format!("\"{spaced}\" --provider kimi");
        assert_eq!(out.matches(&quoted).count(), 6);
        // 且 TOML 解析回的命令值与 render 一致——toml_edit 对内含引号的串会选用字面串形态
        // （`command = '"..." --provider kimi'`），故按解析后的值断言，不押注序列化拼写。
        let reparsed: DocumentMut = out.parse().expect("产物应为合法 TOML");
        let arr = reparsed["hooks"]
            .as_array_of_tables()
            .expect("hooks 应为 array-of-tables");
        assert_eq!(arr.len(), 6);
        for t in arr.iter() {
            assert_eq!(
                t.get("command").and_then(|v| v.as_str()),
                Some(quoted.as_str())
            );
        }
        assert!(KT.has_reporter(&out, "kimi"));
        assert_eq!(KT.claimed_reporter(&out, "kimi").as_deref(), Some(spaced));
        // 重复接线必须幂等——claim 认回带引号条目，一条都不再追加（修复前这里每轮 +6 条）。
        assert_eq!(
            KT.ensure_hooks(&out, spaced, "kimi"),
            EnsureOutcome::Unchanged
        );
    }

    // ── has_reporter / claimed_reporter ──

    #[test]
    fn has_reporter_only_counts_session_start() {
        let session_start = "[[hooks]]\nevent = \"SessionStart\"\ncommand = \"/home/u/.local/meowo-reporter --provider kimi\"\ntimeout = 5\n";
        assert!(KT.has_reporter(session_start, "kimi"));
        assert!(!KT.has_reporter(session_start, "codex")); // agent 不符

        // 只在 Stop 挂了 reporter：不能保证新会话入库，不应判定为已接入。
        let stop_only = "[[hooks]]\nevent = \"Stop\"\ncommand = \"/home/u/.local/meowo-reporter --provider kimi\"\ntimeout = 5\n";
        assert!(!KT.has_reporter(stop_only, "kimi"));
        // 但接线时仍能从中取到 reporter 位置（不限事件）。
        assert_eq!(
            KT.claimed_reporter(stop_only, "kimi").as_deref(),
            Some("/home/u/.local/meowo-reporter")
        );

        // Stop 块在前、SessionStart 块在后：不串块。
        let both = format!("{stop_only}\n{session_start}");
        assert!(KT.has_reporter(&both, "kimi"));

        // 非 hooks 结构 / 用户自有命令 / 畸形 TOML → false。
        assert!(!KT.has_reporter(
            "event = \"SessionStart\"\ncommand = \"node a.js\"\n",
            "kimi"
        ));
        assert!(!KT.has_reporter(
            "[[hooks]]\nevent = \"SessionStart\"\ncommand = \"node a.js\"\n",
            "kimi"
        ));
        assert!(!KT.has_reporter("= = 非法 toml", "kimi"));
    }

    // ═══ CodexJson ═══

    mod codex {
        use super::super::*;
        use serde_json::{json, Value};

        /// codex 的接线规格。事件集 = dispatch 消化面 ∩ codex 0.142 支持面：无 SessionEnd
        /// （codex 不支持，会话收尾靠 Stop + liveness）；不配 PreToolUse（matcher 目标是 claude 专属工具）。
        static EVENTS: [HookEvent; 5] = [
            HookEvent::plain("SessionStart"),
            HookEvent::plain("UserPromptSubmit"),
            HookEvent::plain("PostToolUse"),
            HookEvent::plain("Stop"),
            HookEvent::plain("PermissionRequest").with_timeout(310),
        ];
        static CJ: HookSpec = HookSpec {
            config_rel: "hooks.json",
            format: ConfigFormat::CodexJson,
            missing: MissingConfig::CreateFrom("{\"hooks\":{}}"),
            events: &EVENTS,
            command: CommandSpec {
                quote_exe: true,
                with_provider: true,
                ps_call_operator: false,
            },
        };

        fn changed(v: &Value, reporter: &str) -> Value {
            match CJ.ensure_hooks(&v.to_string(), reporter, "codex") {
                EnsureOutcome::Changed(s) => serde_json::from_str(&s).unwrap(),
                other => panic!("期望 Changed，实得 {other:?}"),
            }
        }
        fn outcome(v: &Value, reporter: &str) -> EnsureOutcome {
            CJ.ensure_hooks(&v.to_string(), reporter, "codex")
        }

        #[test]
        fn adds_all_events_when_empty() {
            let out = changed(&json!({}), "C:/x/meowo-reporter.exe");
            for ev in EVENTS {
                let h = &out["hooks"][ev.name][0]["hooks"][0];
                assert_eq!(h["command"], "\"C:/x/meowo-reporter.exe\" --provider codex");
                #[cfg(windows)]
                assert_eq!(
                    h["commandWindows"],
                    "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command \"& 'C:/x/meowo-reporter.exe' --provider codex\""
                );
                assert_eq!(h["timeout"], ev.timeout);
            }
            assert_eq!(
                outcome(&out, "C:/x/meowo-reporter.exe"),
                EnsureOutcome::Unchanged
            );
        }

        #[test]
        fn adopts_manual_wiring_and_fills_missing() {
            // 复刻手工接线形态：裸路径命令、3 事件、Stop timeout=10。
            let dev = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe";
            let entry = |t: u64| json!({ "hooks": [{ "type": "command", "command": format!("{dev} --provider codex"), "timeout": t }] });
            let v = json!({ "hooks": { "SessionStart": [entry(5)], "UserPromptSubmit": [entry(5)], "Stop": [entry(10)] }});
            let out = changed(&v, dev); // 补 PostToolUse/PermissionRequest → 有改动
                                        // 既有条目原样保留（裸路径不被改写为引号形态、timeout 10 不动）——幂等按解析后内容判定。
            assert_eq!(
                out["hooks"]["Stop"][0]["hooks"][0]["command"],
                format!("{dev} --provider codex")
            );
            assert_eq!(out["hooks"]["Stop"][0]["hooks"][0]["timeout"], 10);
            assert!(out["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("--provider codex"));
            assert!(out["hooks"]["PermissionRequest"].is_array());
            assert_eq!(
                out["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
                310
            );
            assert_eq!(outcome(&out, dev), EnsureOutcome::Unchanged);
        }

        #[test]
        fn upgrades_only_permission_timeout() {
            let reporter = "C:/x/meowo-reporter.exe";
            let command = format!("\"{reporter}\" --provider codex");
            let entry = |timeout| json!({ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] });
            let v = json!({ "hooks": {
                "SessionStart": [entry(12)],
                "PermissionRequest": [entry(5)]
            }});
            let out = changed(&v, reporter);
            assert_eq!(out["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 12);
            assert_eq!(
                out["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
                310
            );
        }

        #[test]
        fn updates_stale_path_keeps_user_hooks() {
            let v = json!({ "hooks": { "Stop": [
                { "hooks": [{ "type": "command", "command": "node my-notify.js" }] },
                { "hooks": [{ "type": "command", "command": "\"C:/old/meowo-reporter.exe\" --provider codex", "timeout": 5 }] }
            ]}});
            let out = changed(&v, "C:/new/meowo-reporter.exe");
            assert_eq!(
                out["hooks"]["Stop"][0]["hooks"][0]["command"],
                "node my-notify.js"
            ); // 用户 hook 不动
            assert_eq!(
                out["hooks"]["Stop"][1]["hooks"][0]["command"],
                "\"C:/new/meowo-reporter.exe\" --provider codex"
            );
            assert_eq!(out["hooks"]["Stop"].as_array().unwrap().len(), 2); // 不重复追加
        }

        #[test]
        fn abandons_when_hooks_key_is_non_object() {
            // 手改坏形状：既有实现会整体置 {}，无备份地清掉用户内容。必须放弃。
            // 旧实现返回「无改动」，会被上层当成「已是目标状态」而谎报成功；现在如实回传原因。
            assert_eq!(
                outcome(&json!({ "hooks": 5 }), "C:/x/meowo-reporter.exe"),
                EnsureOutcome::Abandon(RepairReason::ConfigUnreadable)
            );
        }

        #[test]
        fn abandons_on_invalid_or_non_object_json() {
            for src in ["{not json", "[1,2]", "\"scalar\""] {
                assert_eq!(
                    CJ.ensure_hooks(src, "C:/x/meowo-reporter.exe", "codex"),
                    EnsureOutcome::Abandon(RepairReason::ConfigUnreadable),
                    "src={src:?}"
                );
            }
        }

        #[test]
        fn tolerates_utf8_bom() {
            let out = CJ.ensure_hooks("\u{feff}{\"hooks\":{}}", "C:/x/meowo-reporter.exe", "codex");
            assert!(
                matches!(out, EnsureOutcome::Changed(_)),
                "带 BOM 的 JSON 应能解析"
            );
        }

        #[test]
        fn skips_event_with_non_array_value() {
            // 某事件值为畸形形状（非 array）：该事件原样跳过不动，其余事件正常补齐。
            let out = changed(
                &json!({ "hooks": { "Stop": "oops" } }),
                "C:/x/meowo-reporter.exe",
            );
            assert_eq!(out["hooks"]["Stop"], json!("oops"));
            for ev in EVENTS.iter().filter(|e| e.name != "Stop") {
                assert!(out["hooks"][ev.name][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains("--provider codex"));
            }
        }

        #[test]
        fn has_reporter_only_counts_session_start() {
            let wired = changed(&json!({}), "C:/x/meowo-reporter.exe").to_string();
            assert!(CJ.has_reporter(&wired, "codex"));
            assert!(!CJ.has_reporter(&wired, "kimi")); // agent 不符
            assert_eq!(
                CJ.claimed_reporter(&wired, "codex").as_deref(),
                Some("C:/x/meowo-reporter.exe")
            );

            // 只在 Stop 挂了 reporter：不应判定为已接入；但仍能取到二进制位置。
            let stop_only = json!({ "hooks": { "Stop": [
                { "hooks": [{ "type": "command", "command": "\"C:/x/meowo-reporter.exe\" --provider codex", "timeout": 5 }] }
            ]}}).to_string();
            assert!(!CJ.has_reporter(&stop_only, "codex"));
            assert_eq!(
                CJ.claimed_reporter(&stop_only, "codex").as_deref(),
                Some("C:/x/meowo-reporter.exe")
            );

            // 废弃的 cc-reporter 挂在 SessionStart：认领得到（供替换），但不算已接入。
            let legacy = json!({ "hooks": { "SessionStart": [
                { "hooks": [{ "type": "command", "command": "\"C:/x/cc-reporter.exe\" --provider codex", "timeout": 5 }] }
            ]}}).to_string();
            assert!(!CJ.has_reporter(&legacy, "codex"));
            assert_eq!(CJ.claimed_reporter(&legacy, "codex"), None);
            // 接线时会被认领并更新为当前 reporter，而非重复追加。
            let fixed = changed(
                &serde_json::from_str::<Value>(&legacy).unwrap(),
                "C:/new/meowo-reporter.exe",
            );
            assert_eq!(fixed["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
            assert!(CJ.has_reporter(&fixed.to_string(), "codex"));

            // 用户自有 hook / 无 hooks 键 → false。
            assert!(!CJ.has_reporter(
                &json!({ "hooks": { "SessionStart": [{ "hooks": [{ "command": "node a.js" }] }] }})
                    .to_string(),
                "codex"
            ));
            assert!(!CJ.has_reporter("{}", "codex"));
        }
    }
}
