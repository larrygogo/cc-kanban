# Agent 插件化：codex / claude 迁入与四套抽象收敛

日期：2026-07-09
前置：[2026-07-09-agent-plugin-architecture.md](./2026-07-09-agent-plugin-architecture.md)（kimi 试点已完成，commit `441e1d4`）

---

## 0. 现状

`meowo-agent` 已落地：`AgentId` / `Variant` / `Installation` / `ConfigFormat` / `AuthScheme` / 注册表，kimi 走通全链路（探测 → 接线 → 状态 → 鉴权）。claude、codex 仍散在 `meowo-app` 的 `setup/`、`account/` 与 `meowo-reporter` 的 `agent.rs` 里。

试点期刻意只支持了 kimi 一种形态。把另外两家的真实形态摊开看，**当前接口不够用**——这是迁移前必须先解决的，否则会写出一堆 `match id { }` 把插件层变成新的分支中枢。

---

## 1. 三家形态对照（迁移的全部难点都在这张表里）

| | claude | codex | kimi（已迁） |
|---|---|---|---|
| 数据目录 | `~/.claude`（env `CLAUDE_CONFIG_DIR`） | `~/.codex`（env `CODEX_HOME`） | `~/.kimi-code` / `~/.kimi` |
| hooks 文件 | `settings.json` | `hooks.json` | `config.toml` |
| hooks 格式 | JSON，条目**带 `matcher`** | JSON，条目**无 `matcher`** | TOML `[[hooks]]` |
| 事件数 | 8 条 `(event, matcher)` | 5 条 event | 6 条 event |
| command 形态 | `"<exe>"`（**无参数**） | `"<exe>" --provider codex` | `<exe> --provider kimi` |
| 认领规则 | `reporter_exe_path`（单可执行、禁带参） | `claim_provider_cmd` | `claim_provider_cmd` |
| 接线副作用 | 生成 `~/.meowo/statusline.sh` 并改写 `statusLine` | 写 `config.toml` 的 `[hooks.state]` trusted_hash（SHA-256） | 无 |
| 凭据 | `~/.claude/.credentials.json`，**macOS 走 Keychain** | `~/.codex/auth.json` | `<data>/credentials/kimi-code.json` |
| 可执行解析 | PATH / `~/.local/bin` | **argv**：bun exe ／ `node <codex.js>` ／ standalone exe | 单可执行路径 |
| 变体差异 | 暂只有一个 | 暂只有一个（但可执行有三种落法） | modern / legacy |

结论：`Variant` 现在把「事件表、command 形态、认领规则」隐含在 `ConfigFormat::KimiToml` 里，是试点期的偷懒；`ExeSpec` 只能产出单个路径，撑不起 codex 的 `node <js>`；`AuthScheme.credentials_rel` 是纯文件路径，撑不起 claude 的 Keychain。

---

> **进度**：Phase A ✅（`90b0198`）· Phase B ✅ · Phase C ✅ · Phase D 待做。

## 2. 接口改造（Phase A · 无行为变更）✅

**A1. 抽出 `HookSpec`，让事件表成为变体的声明**

```rust
pub struct HookSpec {
    pub config_rel: &'static str,      // "settings.json" | "hooks.json" | "config.toml"
    pub format:     ConfigFormat,      // ClaudeJson | CodexJson | KimiToml
    pub events:     &'static [HookEvent],
    pub command:    CommandSpec,
}
pub struct HookEvent { pub name: &'static str, pub matcher: Option<&'static str> }
pub enum CommandSpec {
    /// `"<exe>"`，禁带参数（claude）。认领 = 文件名恰为 meowo-reporter[.exe] 且无余参。
    BareQuoted,
    /// `"<exe>" --provider <id>`（codex/kimi）。认领 = 余参恰为 ["--provider", id]。
    WithProvider,
}
```

`Variant.config: ConfigFormat` → `Variant.hooks: &'static HookSpec`。
`ConfigFormat::{ensure_hooks, has_reporter, claimed_reporter}` 改为 `HookSpec::` 上的方法，事件表与 command 形态从入参取，不再写死。`config_rel` 从 `ConfigFormat` 派生（当前是 1:1 巧合）改为显式字段。

`claim_provider_cmd` / `reporter_exe_path` 合并为 `CommandSpec::claim(cmd, agent_id) -> Option<String>`，两套认领规则各是一个分支。**注意语义差**：claude 的 `session_start_has_reporter`（判「已接入」）与 `reporter_path_from_hooks`（判「二进制在哪」）刻意不同——前者只看 SessionStart，后者广扫全事件。kimi 已用 `has_reporter` / `claimed_reporter` 表达了这对差异，claude 直接复用。

**A2. `ExeSpec` → `LaunchSpec`，产出 argv**

```rust
pub enum LaunchCandidate {
    /// 可执行文件：<base>/<sub>/<stem>[.exe] → argv = [path]
    Exe { base: Base, sub: &'static str },
    /// Node 脚本：argv = ["node", js_path]（codex npm 全局）
    NodeScript { locate: fn() -> Option<String> },
}
pub enum Base { DataDir, Home }
pub struct LaunchSpec { pub stem: &'static str, pub candidates: &'static [LaunchCandidate] }
impl LaunchSpec { pub fn probe(&self, data_dir, home) -> Option<Vec<String>> }
```

`Installation.exe: Option<PathBuf>` → `Installation.launch: Option<Vec<String>>`；`exe_command()` → `launch_argv()`（找不到回退 `[stem]`）。`Agent::resume_args` / `launch_args` 直接消费它。

`NodeScript` 带 `fn()` 而非声明式路径,是因为 npm 全局前缀要跑 `npm root -g` 式探测——把这点不纯留在 codex 插件里，好过把它塞进声明表硬凑。

**A3. `AuthScheme.credentials` 支持 Keychain**

```rust
pub enum CredentialSource {
    /// 相对 data_dir 的 JSON 文件
    File(&'static str),
    /// macOS 登录 Keychain 的通用密码；其它平台回退 File
    KeychainOrFile { service: &'static str, account: &'static str, file: &'static str },
}
```
`AuthScheme { credentials: CredentialSource, token_url, client_id, default_base_url }`；`token_url`/`client_id` 对 codex 无 OAuth 刷新需求时置空 → 改成 `refresh: Option<OAuthRefresh>`。

**A4. 澄清「已安装」的两个含义**（这是「已安装 / 未检测到数据目录」自相矛盾的根因）

- `Installation.launch.is_some()` ＝ **可执行装了**（能启动/恢复会话）
- `data_dir.is_dir()` ＝ **用过/配置过**（能接线、能读会话）

`AgentPlugin` 明确给出两个查询：`is_launchable()` / `is_configured()`。前端卡片与 `ProviderSetup::detect` 各取所需，不再混用。

**验收**：kimi 全链路行为不变，`cargo test --workspace` 全绿；`meowo-agent` 新增 `HookSpec` / `CommandSpec` / `LaunchSpec` 单测。**不碰 claude/codex 代码。**

---

## 3. Phase B · 迁 codex（先难在「副作用」，后难在「argv」）✅

**落地时的两点偏离设计**
- `SetupBehavior` trait 没建。codex 的副作用只需要「hooks.json 写完之后再做点事」这一个切点，
  一个 `AfterWrite` 函数指针就够；为一个实现造 trait 是空架子。claude 的 statusLine 写在
  **同一个 settings.json 里**，需要的是 `amend`（写前改文本）而非 `after_write`——Phase C 再引入。
- 接线编排提成了 `setup::wire_hooks(inst, agent_id, after)`，kimi/codex 共用。三个「绝不」
  （解析失败绝不写、写前必备份、一律原子写）在这一处集中兑现，不再各写一遍。

**一处行为改进**：`hooks` 键存在但非 object 时，旧实现返回「无改动」，上层会把它当成
「已是目标状态」谎报成功；现在如实回传 `Abandon(ConfigUnreadable)`，「修复连接」会给出原因。
同理，SessionStart 上挂着废弃的 `cc-reporter` 时，旧实现不认领、重复追加一条；现在认领并更新。

codex 排在 claude 前面：它的两个难点（接线副作用、argv 可执行）是 claude 也要用的机制，且 codex 没有 Keychain 这类平台分叉，做起来干净。

**B1** `plugins/codex.rs`：变体表（单变体 `stable`）+ `HookSpec{ hooks.json, CodexJson, 5 events, WithProvider }` + `LaunchSpec{ bun exe → NodeScript → standalone exe }`。

**B2** `ConfigFormat::CodexJson` 实现 `ensure_hooks` / `has_reporter` / `claimed_reporter`：搬 `setup/codex.rs` 的 `ensure_codex_hooks` + `reporter_path_from_codex` + `hooks_json_has_reporter`。现有 8 个单测一并搬入（含「hooks 键非 object 放弃」「事件值非 array 跳过」两条回归）。

**B3** 接线副作用建模。**不进 meowo-agent**（要 `sha2` / `fsutil`）：在 `meowo-app` 定义

```rust
trait SetupBehavior {
    fn post_wire(&self, inst: &Installation, wrote: bool) -> Option<RepairReason> { None }
}
```
codex 的 `post_wire` = 现有 `claimed_codex_entries` + `ensure_trusted_hashes` + 写 `config.toml`。保持现语义：**best-effort，失败仍返回成功**（hooks 已接上，退化只是 codex 弹一次 Trust all）。`codex_hook_hash` 的三条真机向量测试原样保留——它是 codex 升级改算法时的绊线，不能丢。

**B4** `setup/codex.rs` 收缩为 `ProviderSetup` 的 I/O 编排（与 `setup/kimi.rs` 同形）；`codex_hooks_status()` 走 `inst.hooks.has_reporter()`；`reporter/codex.rs` 的 `codex_home()` / `codex_launch_prefix()` 变成 `codex_install()` 的薄封装。

**B5** `account/codex.rs`：`auth.json` 路径改由 `Installation.credentials_path()` 给。codex 无 token 刷新，`refresh: None`。

**验收**：`~/.codex` 副本 dry-run —— 5 条 hooks、`[hooks.state]` 键仍是 `<path>:<snake>:<group>:<handler>` 真实索引、三条哈希向量不变；claude/kimi 零变化。

---

## 4. Phase C · 迁 claude（matcher + statusLine + Keychain）✅

**落地时的偏离设计与顺带修复**

- **`Amend` 切点已引入**（Phase B 预告的那个）。claude 的 statusLine 与 hooks 同住 `settings.json`，
  故走「写前改写」。`wire_hooks` 的幂等判定随之**后移到 amend 之后、与最初读到的文本比对**——
  这是把旧 `apply()` 里「`settings == orig` 再决定写不写」的**正确语义**搬进通用编排，不是修 bug；
  但若照搬 kimi/codex 那套「只看 `ensure_hooks` 结果」，hooks 已就位而 statusLine 待接时**就会**
  被误跳过。约定写进了 `Amend` 的文档：**无改动必须原样返回入参文本**，否则重新序列化会让文本
  比较误判为有改动，每次启动重写一遍用户配置。

- **npm 候选是 `Exe` 而非 `NodeScript`**：`npm view @anthropic-ai/claude-code bin` → `bin/claude.exe`，
  该包分发的是原生二进制，不是 JS 入口（与 codex 不同，别照抄）。且这条只对 Windows 有意义：
  unix 上 npm 生成的 shim 是无扩展名的 `claude`，`OnPath` 就能命中；Windows 上是 `claude.cmd`，
  `exe_on_path("claude.exe")` 看不见它。

- **§6 第三条「启动和检测也有问题」预言命中**：claude 的 `launch_args`/`resume_args` 此前硬编码
  裸名 `"claude"`，而 `is_installed()` 有 `~/.local/bin` 兜底——于是**检测说「已装」、wt.exe 却报
  `0x80070002` 系统找不到指定的文件**（新设备装完打不开）。三者现在同源走 `Installation::launch_argv()`。

- **`wire_hooks` 增补「已认领路径须仍存在」过滤**：旧 claude 的 `resolve_reporter_native` 有
  `exists()` 检查，通用版漏了。app 换目录后 hooks 里的旧路径会被当成目标写回去、hooks 静默失效
  （sidecar 明明就在手边）。kimi/codex 一并受益。

- **`hooks` 键非 object 时从「直接覆盖」改为 `Abandon`**：旧 claude `ensure_hooks` 会
  `settings["hooks"] = json!({})` 覆盖用户手改坏的值。现与 codex 同为放弃写入（绝不写坏）。

- **新增 `HookSpec::parses()`**：`check_provider_hooks` 此前对 claude 是「解析失败 → Unknown」，
  对 kimi/codex 却是「→ Missing」。后者会催用户点「修复连接」，而修复必因 `Abandon(ConfigUnreadable)`
  拒写，陷入死循环。三家现已统一为 Unknown。

- **claude 开始认领废弃的 `cc-reporter`**（与 §3 对 codex 的改进同）：旧 `reporter_exe_path` 只认
  meowo-reporter，遇到 cc-reporter 不认领、重复追加一条；现在认领并更新路径。

- **`~/.claude.json` 未进 `Variant`**：账号信息住在 home 下、**不在** data_dir 内（与凭据不同源）。
  `CLAUDE_CONFIG_DIR` 设置时它的落点未经证实，故 `account/claude.rs` 保持原样读 home，不硬塞进
  变体模型。C4 的其余部分（凭据路径 / Keychain / client_id / token_url）已由 `AuthScheme` 驱动。

- **macOS Keychain 分支的验证手段**：本机无 macOS toolchain（`cargo check --target aarch64-apple-darwin`
  因缺 `cc` 失败）。改用一个 scratch crate 真实依赖 meowo-agent、把 macOS 分支代码逐字贴入并去掉
  cfg gate、在 Windows 上编译，断言 `keychain_spec() == ("Claude Code-credentials", "root")`——
  service 名一旦漂移，macOS 用户就读不到既有登录态。

- **`cargo +nightly fmt` 不可用于本仓**：无 `rustfmt.toml`，HEAD 从未按默认 rustfmt 风格格式化，
  跑一次会重排 50 个文件。本轮未跑 fmt。`cargo clippy -- -D warnings`（stable）通过；nightly clippy
  在 `account/kimi.rs:311` 有一条既存 `question_mark` 违规，与本轮无关。

**C1** `plugins/claude.rs`：变体表（单变体 `stable`）+ `HookSpec{ settings.json, ClaudeJson, 8 条带 matcher, BareQuoted }` + `LaunchSpec{ ~/.local/bin → npm(Win) → PATH }` + `AuthScheme{ KeychainOrFile }`。
`DataDirSpec.env` 语义对齐：`CLAUDE_CONFIG_DIR` 直接就是数据目录（与 `KIMI_SHARE_DIR` 同）——已支持。

**C2** `ConfigFormat::ClaudeJson`：搬 `ensure_hooks`（matcher 感知的定位/追加）+ `session_start_has_reporter` + `reporter_path_from_hooks`。**必须保住** `find_reporter_entry_with_matcher` 不认领「命令含 meowo-reporter 子串的用户 hook」这条（`node tools/meowo-reporter-notify.js`）——现有单测已覆盖，一并搬。
`HOOK_SPECS` 与 `scripts/install-hooks.mjs` 的 `SPECS` 必须一致，**加一条对该 JS 文件的绊线测试**（现在只有注释在提醒，靠不住）。

**C3** claude 的 `post_wire` = statusLine 包装脚本。这段有一条来之不易的纪律，**原样保留**：脚本先落盘成功、settings 才指向它；幂等命中但脚本文件缺失时重建为自渲染版兜底。四个相关单测搬进来。

**C4** `account/claude.rs`：`credentials_path()` 与 macOS Keychain 分支改由 `CredentialSource` 驱动；`CLIENT_ID` / `TOKEN_URL` / `USAGE_URL` 进 `AuthScheme`（`USAGE_URL` 是 account 侧的，加进 `AuthScheme` 还是留 app？→ 留 app 的 `AccountBehavior`，`AuthScheme` 只管「凭据在哪 + 怎么刷新」）。

**验收**：`CLAUDE_CONFIG_DIR=<副本>` dry-run —— 8 条 (event,matcher) 齐、用户 `PreToolUse:Bash` 原封、`statusLine` 指向脚本且脚本存在、顶层键无丢失。

---

## 5. Phase D · 收敛四套抽象

三家都进 `plugins/` 后，把行为面归位：

| 现在 | 归位后 |
|---|---|
| `ProviderKey` 枚举（分支中枢） | 仅作 DB 列 / 前端 key 的强类型；分支一律走 `registry::by_id`。`ProviderKey::ALL` ↔ `registry::all()` 配对测试 |
| `meowo-reporter::agent::Agent` | `ReporterBehavior`：`stop_outputs` / `read_context` / `write_rename` / `transcript` / `process_names`。入参 `&Installation`；`resume_args`/`launch_args`/`is_installed` 由 `Installation.launch_argv()` 直接给，**从 trait 删除** |
| `meowo-app::setup::ProviderSetup` | `SetupBehavior`：只剩 `post_wire`。`detect`/`apply` 变成注册表驱动的**通用**函数（读配置 → `hooks.ensure_hooks` → 备份 → 原子写 → `post_wire`），三家共用一份 |
| `meowo-app::account::ProviderAccount` | `AccountBehavior`：`account` / `usage` / `usage_supported`。凭据路径与刷新参数来自 `Installation.auth` |

`setup::apply_provider` / `check_provider_hooks` / `account::for_provider` 全部改为 `by_id(key.as_str())` 查表。`install_script`（一句话安装命令）挂到 `AgentPlugin` 上。

**Phase D 的收益判据**：加一个新 agent 需要改的文件数从 ~10 降到 2（`plugins/<new>.rs` + `registry.rs` 的一行）。这条要在文档里当验收标准写死，否则很容易滑回去。

---

## 6. 顺带修掉的既有问题

- **`is_installed` 口径不一**（A4）：`KimiAgent::is_installed()` = 可执行在 PATH，`KimiSetup::detect()` = 数据目录存在。前端卡片同时显示「已安装」与「未检测到该 agent 的数据目录」，正是这两者打架。
- **kimi legacy `client_id` 未证实**：`AUTH_LEGACY` 现在复用新版值。等 `刷新 token 响应体` 到手——`invalid_client` 就改 `plugins/kimi.rs` 一个 const。
- ~~**用户搁置的「启动和检测也有问题」**：大概率就是 A4 + `LaunchSpec`（codex 的 argv 三种落法）。Phase A/B 做完再回头看，很可能不用单独修。~~
  → **Phase C 已修**。判断基本正确，但 claude 那一半直到 C1 给它 `LaunchSpec` 才解决：裸名 `"claude"`
  交给 wt.exe，而 wt 继承的是 app 启动时的 PATH 快照，看不到 native installer 刚加的 `~/.local/bin`。
- ~~**`scripts/install-hooks.mjs` 与 `HOOK_SPECS` 靠注释同步**（C2）：加绊线测试。~~
  → 已加 `hook_specs_match_install_hooks_mjs`（解析 mjs 的 SPECS 字面量与 `plugins/claude.rs` 的 EVENTS 逐条比对）。

---

## 7. 排期与风险

| Phase | 范围 | 风险 | 可独立合入 |
|---|---|---|---|
| A 接口改造 | 只动 `meowo-agent` + kimi 调用点 | 低（无行为变更，单测护栏） | ✅ |
| B codex | `plugins/codex.rs` + setup/account/reporter 收缩 | 中（trusted_hash 索引语义、argv） | ✅ |
| C claude | `plugins/claude.rs` + statusLine + Keychain | 中高（statusLine 顺序纪律、macOS 分叉无法在 Windows 上验证） | ✅ |
| D 收敛 | 删三套 trait，注册表驱动 | 中（改动面广但纯机械，测试全在） | ✅ |

**共同纪律（每个 Phase 都不许破）**
1. 解析失败绝不写；写前 `backup_once`；一律原子写。
2. 用户自有 hook 一概不动——认领规则严格，不裸 `contains`。
3. 每个 Phase 结束：`cargo test --workspace` + `cargo clippy -D warnings` + 对应 agent 的**真实配置副本** dry-run（`XXX_HOME=<副本>`，绝不对真目录跑）。
4. dry-run 只打印结构性摘要，绝不 dump 配置原文（`~/.kimi/config.toml` 含 `api_key`，`auth.json`/`.credentials.json` 含 token）。
5. DB schema 与前端契约不变；`ProviderKey` 字符串值不变。

**建议**：A 和 B 一次做完（B 会立刻检验 A 的接口是否真的够用，分开做等于让 A 空转一轮）；C 单独一轮；D 单独一轮。
