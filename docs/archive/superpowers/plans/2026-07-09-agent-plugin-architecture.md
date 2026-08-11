# Agent 插件化架构设计（内部 trait + 注册表 + 变体层）

日期：2026-07-09
决策：内部编译期插件（非动态/非 manifest）· 目标先理顺自家维护 · 本次范围＝设计 + 迁 kimi 试点

---

## 1. 目标 / 非目标

**目标**
- 一个 agent = 一个自包含模块，实现统一契约；加/改 agent 不再散动 ~10 文件、4 套抽象。
- **一等的「变体(Variant)」层**：同一 agent 的多版本差异（数据目录 / 配置格式 / 鉴权）收敛在一处声明，核心逻辑只写一遍。
- 用注册表取代 `ProviderKey` 枚举做分支中枢；枚举仅为 DB 兼容保留、由注册表派生。

**非目标（本轮）**
- 不做动态加载（.dll/.so/子进程协议）、不做对外 manifest 协议、不支持第三方 agent。
- 不改数据库 schema、不改前端交互；`ProviderKey` 字符串值（claude/kimi/codex）保持不变。

---

## 2. 现状痛点

一个 agent 摊在 4 个互相独立的抽象里，全靠 `ProviderKey` 枚举串联：

| 抽象 | 位置 | 职责 |
|---|---|---|
| `ProviderKey` 枚举 | `meowo-store/models.rs` | 唯一身份；加 agent 必改枚举 |
| `Agent` trait | `reporter/agent.rs` | 运行时：进程名/resume/launch/read_context/transcript/rename |
| `ProviderSetup` trait | `app/setup/` | hooks 接线：detect/apply |
| `ProviderAccount` trait | `app/account/` | 账号 + 用量 |

**没有「版本/变体」概念**：kimi 的 `~/.kimi`(旧 Python) vs `~/.kimi-code`(新 Node)、`hooks = []` 内联 vs `[[hooks]]`、旧/新 OAuth client_id —— 全靠散落在 `kimi_share_dir` / `ensure_kimi_hooks` / `ensure_valid_kimi_token` 的临时 `if`/fallback 硬凑。这次排查一路打了 4 处补丁就是症状。

---

## 3. 目标架构

### 3.1 跨 crate 的现实约束
契约分布在两个二进制：`meowo-reporter`（hook 子进程）与 `meowo-app`（Tauri GUI），共享 `meowo-store`。
- **可共享的纯逻辑**：身份、变体探测、路径解析、配置格式读写、鉴权参数 —— 只依赖 std/serde/toml，痛点也正在这层。
- **reporter 专属**：会话文件解析（wire.jsonl / transcript）。
- **app 专属**：hooks 落盘（app fsutil）、联网取用量（ureq）。

→ 结论：**新建轻量共享 crate `meowo-agent`** 承载「身份 + 变体 + 配置格式适配器 + 鉴权描述 + 注册表」。reporter/app 各自实现自己的行为面，但都以共享的 `Installation` 为输入，不再各自重推路径。

### 3.2 crate 布局
```
meowo-agent (新, 纯逻辑)
├─ id.rs         AgentId（&'static str 身份）+ 与 ProviderKey 互转
├─ variant.rs    Variant / Installation / DataDirSpec / ExeSpec
├─ config.rs     ConfigFormat（KimiToml | CodexJson | ClaudeJson）+ ensure/has_reporter 纯函数
├─ auth.rs       AuthScheme（client_id / 刷新端点 / 凭据字段映射）
├─ registry.rs   &[&dyn AgentPlugin] + by_id / all
└─ plugins/      claude.rs / kimi.rs / codex.rs —— 每个声明 variants() + 探测

meowo-reporter   trait ReporterBehavior（stop_outputs/read_context/transcript…），入参 &Installation
meowo-app        trait SetupBehavior（wire_hooks/status）+ AccountBehavior（account/usage），入参 &Installation
```

### 3.3 核心类型（草案）
```rust
// meowo-agent
pub struct AgentId(pub &'static str);          // "claude" | "kimi" | "codex"

pub trait AgentPlugin: Sync {
    fn id(&self) -> AgentId;
    fn display_name(&self) -> &'static str;
    fn variants(&self) -> &'static [Variant];  // 按优先级排列
    fn detect(&self) -> Option<Installation> {  // 默认：逐 variant probe，命中即返回
        self.variants().iter().find_map(Variant::probe)
    }
}

/// 同一 agent 的一个版本形态——所有差异收敛于此。
pub struct Variant {
    pub tag: &'static str,          // "modern" | "legacy"
    pub data_dir: DataDirSpec,      // env 覆盖 + 候选目录（如 ~/.kimi-code 优先 → ~/.kimi）
    pub config: ConfigFormat,       // hooks 配置格式适配器
    pub auth: Option<AuthScheme>,   // OAuth client_id / 刷新端点 / 凭据字段映射
    pub exe: ExeSpec,
}

/// probe 命中后的运行时事实（这台机器上该 agent 的实况）。
pub struct Installation {
    pub id: AgentId,
    pub variant_tag: &'static str,
    pub data_dir: PathBuf,
    pub config: ConfigFormat,
    pub auth: Option<AuthScheme>,
    pub exe: PathBuf,
}

/// hooks 配置格式：统一「确保接入 / 判断已接入」，纯函数便于单测。
pub enum ConfigFormat { KimiToml, CodexJson, ClaudeJson }
impl ConfigFormat {
    pub fn ensure_hooks(&self, cur_text: &str, reporter: &str, provider: &str) -> EnsureOutcome;
    pub fn has_reporter(&self, cur_text: &str, provider: &str) -> bool;
}
pub enum EnsureOutcome { Changed(String), Unchanged, Abandon(RepairReason) }
```

**变体层如何消灭 kimi 的坑**（一张表代替 4 处补丁）：
```rust
// plugins/kimi.rs
const VARIANTS: &[Variant] = &[
    Variant { tag:"modern", data_dir: DataDirSpec::env_or(&["~/.kimi-code"]),
              config: ConfigFormat::KimiToml, auth: Some(AUTH_NEW), exe: … },
    Variant { tag:"legacy", data_dir: DataDirSpec::env_or(&["~/.kimi"]),
              config: ConfigFormat::KimiToml, auth: Some(AUTH_LEGACY), exe: … },
];
```
- 目录选择：`detect()` 逐 variant probe，命中的 `Installation.data_dir` 即真相 → 取代 `kimi_share_dir` 的 fallback。
- 接线：`Installation.config.ensure_hooks(...)` 内部已处理 `hooks=[]` 空内联无损替换 → 取代散落逻辑。
- 鉴权：`Installation.auth`（含 legacy/new client_id + 刷新端点）→ 取代写死的 `KIMI_CLIENT_ID`，直击这次 400 的根因。

### 3.4 身份与注册表
- `AgentId` 为字符串身份，注册表 `meowo_agent::registry::all()/by_id(id)` 是唯一分支中枢。
- `ProviderKey` 枚举保留（DB 列、前端 key 不变），提供 `AgentId ⇄ ProviderKey` 互转；`ALL` 与注册表配对测试守住一致性。

---

## 4. 试点：迁移 kimi ✅ 已完成

选 kimi：变体差异最复杂，最能验证 `Variant`/`ConfigFormat`/`AuthScheme` 三件套。

**步骤**
1. ✅ 建 `meowo-agent` crate（id/variant/config/auth/registry + plugins/kimi，18 个单测）。
2. ✅ `ConfigFormat::KimiToml`：`ensure_kimi_hooks` + `toml_text_has_reporter` 迁入并合并为 `ensure_hooks`/`has_reporter`/`claimed_reporter`（含 `hooks=[]` 无损替换）。`has_reporter` 由逐行状态机换成 `toml_edit` 解析。
3. ✅ `plugins/kimi.rs` 变体表（modern `~/.kimi-code` / legacy `~/.kimi`）+ 默认 `detect()`。
4. ✅ reporter 侧：`kimi_share_dir`/`kimi_exe`/`kimi_installed` 收敛为 `kimi_install()` 的薄封装；app 侧 setup/hooks-status 改走 `Installation`。
5. ✅ `context_window` 等会话读取以 `Installation.config_path()` 为根。
6. ✅ 鉴权：`ensure_valid_kimi_token` / `kimi_base_url` / 凭据路径均取自 `Installation.auth`，`KIMI_TOKEN_URL`/`KIMI_CLIENT_ID`/`DEFAULT_BASE_URL` 三个写死常量消失。
7. ✅ 全量编译 + `cargo test --workspace` 全绿 + 用真实 `~/.kimi` 副本 dry-run 验证。

**验收结果**：无 env 覆盖时正确识别 `变体=legacy` → `~/.kimi/config.toml`，6 条 `[[hooks]]`、SessionStart 已接线；claude/codex 行为零变化（仍走旧路径）。

**遗留**：`AUTH_LEGACY` 目前复用新版 client_id（旧版值未知）。若刷新 token 返回 `invalid_client`，只需改 `plugins/kimi.rs` 里这一个 const，account 侧不动——这正是变体层要买到的东西。

**env 覆盖的诚实标签**：`KIMI_SHARE_DIR` 指向的目录没有版本形态信号，命中它的变体把 tag 记为 `env-override` 而非谎报 `modern`。

---

## 5. 后续（试点通过后）
- claude、codex 各按同法迁入 `plugins/`，逐步收敛 4 套抽象。
- 收尾后 `ProviderSetup`/`ProviderAccount`/`Agent` 三 trait 退化为「行为面」trait，统一由注册表驱动。

## 6. 风险 / 兼容
- 新增 crate + 跨 crate 移动纯逻辑：编译期可控，行为等价由既有单测护栏。
- 试点期间只有 kimi 走新路径，claude/codex 走旧路径，互不影响，可分阶段合入。
- 数据库 / 前端契约不变，无迁移风险。
