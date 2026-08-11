# Agent 插件架构

> meowo 提供能力,agent 声明自己用哪些。加一个 agent = 新增 `plugins/<id>/` 一个目录 + 注册表一行。

## 为什么要改

现状下,「一个 agent 是什么」这件事被切成五份,分住四个 crate:

| 碎片 | 位置 | 管什么 |
|---|---|---|
| `ProviderKey` 枚举 | `meowo-store/models.rs` | 身份 |
| `AgentPlugin` | `meowo-agent/plugins/*.rs` | 变体 / 路径 / hooks 规格 / 鉴权 |
| `Agent` | `meowo-reporter/agent.rs` | 进程名 / resume / stop 正文 / context / rename / 安装脚本 |
| `ProviderSetup` | `app/setup/mod.rs` | 接线副作用(amend / after_write) |
| `ProviderAccount` | `app/account/mod.rs` | 账号 / 用量 |
| `PROVIDERS` 表 | `app/src/providers.tsx` 等 | 展示名 / 图标 / 品牌色 |

四个 trait **每个都自带 `key()` 和自己的 `ALL` 注册表**,再靠三个 "enum↔registry" 绊线单测把碎片钉在一起。
测试能钉住,恰恰说明它们本该是一个东西 —— 这不是抽象,是同一个抽象被复制了四遍。

具体病灶:

- `ProviderKey::from_str` 未知降级为 `Claude`。历史 DB 里任何未知 provider 的会话会被**静默改写**成 claude。
- `lib.rs` 的 `install_for` / `repair_provider_hooks` 是两处硬 `match`,靠编译错误提醒补 arm。
- `About.tsx` 的 `provider === "claude"` 特判 + `styles.css` 的 `--cc-claude`:claude 的品牌色被写进了框架。
- `setup::Amend` / `AfterWrite` 已经是 per-agent 的函数指针,却住在 app 的 setup 模块而非插件里。

## 三条原则

1. **身份是字符串,不是枚举。** 唯一身份类型是 `meowo_agent::AgentId`。`meowo-store` 不认识任何具体
   agent,provider 列存原样字符串;未知 id 原样保留,绝不降级。加 agent 不再需要动 DB 层。

2. **能力是槽位,不是方法。** 一个 `AgentPlugin` 通过能力查询暴露自己支持什么,不支持的返回 `None`,
   由框架降级 —— 而不是让每个 agent 实现十几个方法、其中一半返回 `false`。

3. **IO 是注入的端口,不是直接依赖。** 插件层保持纯声明 + 纯函数。要联网 / 读 keychain / 原子写盘的
   能力,通过宿主注入的端口完成。于是账号、接线这些「重」能力也住得进插件,而插件 crate 依然不依赖
   tauri、不依赖 reqwest。

## 目标形态

```
meowo-agent/
  src/
    api/                  能力 trait + 端口 trait + 数据类型。零 IO 依赖
      caps.rs             AgentPlugin + 各能力 trait
      ports.rs            HttpPort / FsPort / KeychainPort
    plugins/
      claude/
        mod.rs            身份 + 变体表 + 能力装配
        account.rs        impl AccountCap —— 只用注入的 HttpPort
        setup.rs          impl WiringCap —— amend(statusLine)
        telemetry.rs      impl TelemetryCap —— transcript 标题解析
      kimi/ ...
      codex/ ...
    registry.rs           static ALL —— 加 agent 只在此补一行
```

`meowo-store` / `meowo-reporter` / `meowo-app` 都只消费同一张注册表。app 负责在启动时注入端口实现
(reqwest / keychain / 原子写),reporter 注入一套最小实现。

### 能力槽位

```rust
pub trait AgentPlugin: Sync {
    // ── 必填:身份 ──
    fn id(&self) -> AgentId;
    fn descriptor(&self) -> &'static Descriptor;   // 展示名 / 品牌色 / 图标 SVG
    fn variants(&self) -> &'static [Variant];      // 变体表(路径/hooks 规格/鉴权/启动)

    // ── 选填:能力。不支持返回 None,框架降级 ──
    fn launch(&self)    -> Option<&dyn LaunchCap>    { None }  // resume/launch argv、安装脚本
    fn wiring(&self)    -> Option<&dyn WiringCap>    { None }  // amend / after_write
    fn telemetry(&self) -> Option<&dyn TelemetryCap> { None }  // stop 正文 / context / transcript / rename
    fn terminal(&self)  -> Option<&dyn TerminalCap>  { None }  // 进程名 / 标签标题 / tab token
    fn account(&self)   -> Option<&dyn AccountCap>   { None }  // 账号 / 用量
}
```

对比现状:`writes_tab_token()` 返回 `false`、`transcript()` 返回 `None`、`usage_supported()` 返回
`false` 这些「我没有这个能力」的表达,现在统一成能力查询返回 `None`。codex 不支持 rename,就不实现
`TelemetryCap::write_rename`;kimi 不需要 amend,就不提供 `WiringCap`。

> **勘误(2026-08-11)**:上面代码块是设计时点的形态。落地后槽位持续生长,`launch()`/`terminal()`
> 两个名字已不存在,现有 `direct_install`/`launch_options`/`chat_ui`/`profile`/`screen_rules`/
> `telemetry`/`runtime`/`account`/`api_key_login`/`wiring`/`proxy`/`relay`/`custom_commands`/
> `cross_account_session` 等 14+ 个槽。**权威清单以 `crates/meowo-agent/src/registry.rs` 的
> trait 定义与 `plugins/mod.rs` 的接入指南 rustdoc 为准**,本文档不再逐槽追认。

### 端口

```rust
pub trait HttpPort: Sync {
    fn get_json(&self, url: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value, PortError>;
    fn post_form(&self, url: &str, body: &str) -> Result<serde_json::Value, PortError>;
}
pub trait FsPort: Sync {
    fn write_atomic(&self, path: &Path, text: &str) -> Result<(), PortError>;
    fn backup_once(&self, path: &Path);
}
pub trait KeychainPort: Sync {
    fn read(&self, service: &str, account: &str) -> Option<String>;
    fn write(&self, service: &str, account: &str, secret: &str) -> Result<(), PortError>;
}
```

端口以 `&dyn` 传入能力方法,不做全局单例 —— 测试可注入假实现,插件层的单测不再需要真网络。

> **勘误(2026-08-11)**:`FsPort` 最终**没有实现**——Phase 3 落地时认定 `fsutil::write_atomic`
> 是纯 `std`、临时目录即可测,做成端口只会平添间接层(见下文 Phase 3 落地记录)。现存端口只有
> `HttpPort` 与 `KeychainPort`。

### 前端零 agent 知识

后端暴露 `list_agents() -> Vec<AgentDescriptor>`:

```jsonc
{ "id": "claude", "display_name": "Claude Code", "installed": true }
```

**图标与品牌色不下发。** 原本设想 descriptor 带 `icon_svg` 与 `brand_color`,实现时撞上两个事实:

- kimi 的 logo 是**位图 PNG**(渐变颗粒纹理,矢量化会失真)。有条 commit 特意把它从 base64 内嵌改成
  静态资源;为了"下发"再塞回 base64 是倒退。
- claude 的品牌橙在浅色/深色主题下取**不同明度**(`#d97757` / `#b0532f`)。这是 meowo 的视觉决策,
  不是 agent 的定义,一个字符串字段表达不了。

结论:**图标和颜色是资产,不是数据。** 它们留在 `providers.tsx` 的资产表里,以 id 为键。加一个 agent
仍要在那里加一项(总得有人提供图标),但前端**不再有任何 agent 的逻辑分支**。

descriptor 也**不带 `capabilities`**:前端目前没有一处按能力分支,不臆造字段。真需要时再加。

据此删除:`providers.tsx` 的 `PROVIDERS` 表(退化为纯资产表)、`api.ts` 的 `ProviderKey` 联合类型与
`PROVIDER_KEYS`、`About.tsx` 的 `=== "claude"` 特判、`styles.css` 里 `.stk-agent { color: var(--cc-claude) }`
这类给所有 agent 抹上 claude 橙的规则、`i18n` 的 `agentClaudeCode` / `agentKimiCode` / `agentCodex` /
`providerClaudeCode` 四条(产品名不翻译,zh/en 值本就完全相同)。`noAgents` 文案不再点名三家。

未知 id 一律**中性兜底**:显示 id 本身 + 灰色占位徽标。旧的 `providerConfig` 未知时回退
`PROVIDERS.claude`,于是一个本版本不认识的 agent 会顶着 Claude 的赤陶徽标出现在卡片上。

## 迁移阶段

每一阶段独立编译、独立测试通过、可独立发布。

| 阶段 | 内容 | 验收 |
|---|---|---|
| 1 ✅ | **身份收敛**:删 `ProviderKey`,全仓改用 `AgentId`;store 的 provider 列退化为字符串;修掉未知降级 claude 的 bug | 三个 enum↔registry 绊线单测中的两个变得无意义并删除 |
| 2 ✅ | **注册表合一**:reporter 的 `Agent` trait 折进 `AgentPlugin`;transcript 抽象迁出 store;`lib.rs` 两处硬 match 消失 | reporter 不再持有第二张注册表 |
| 3 ✅ | **端口注入**:定义 `ports.rs`;account 的联网逻辑与 setup 的 amend/after_write 搬进 `plugins/<id>/` | app 的 `account/` 与 `setup/` 只剩端口实现与编排 |
| 4 ✅ | **前端描述符**:`list_agents()` 下发;删前端三张表与 claude 特判 | 前端只剩一张视觉资产表,无 agent 逻辑分支 |

### Phase 2 落地记录

`meowo-reporter` 的 `Agent` trait 是 `AgentPlugin` 的一份影子副本(同样的 `key()`、同样的 `ALL`),
两者靠一个配对单测钉在一起。合并后:

- **声明式的**(进程名、resume 子命令、安装脚本、是否写标签标题/token)成了 `AgentPlugin` 上带默认值的方法。
- **有逻辑的**(Stop 正文/模型、上下文占用、transcript 规格、重命名回写)进了 `TelemetryCap` 能力槽,
  不支持就返回 `None`,由调用方降级。
- resume/启动 argv 不再各 agent 手写:`resume_argv()` = `launch_argv()` + 声明的子命令 + session_id,
  杜绝「能启动却恢复不了」。
- 能力方法不接 reporter 的 `HookEvent`(它依赖 `meowo_store::TodoInput`,会让插件层反向依赖 DB 层),
  改传只含所需字段的 `HookContext`。

`meowo-store` 现在只剩 `error` / `migrations` / `models` / `query` / `store` 五个模块,不认识任何 agent。

### Phase 3 落地记录

`ProviderAccount` 与 `ProviderSetup` 住在 app 里,只是因为它们要联网(`ureq`)、要读 macOS Keychain
(`security` 子进程)、要 `db_path()`。这三样都是端口该隔离的东西:

- **`HttpPort` / `KeychainPort`** 由宿主注入。`meowo-agent` 因此不依赖 `ureq`,插件的单测注入假实现
  就能跑(`NoHttp` 用来断言「不该联网的路径确实没联网」)。
- **`WiringContext`** 传入 `meowo_dir` 与 sidecar 路径,插件不必认识 `db_path()`。
- **`fsutil::write_atomic` 刻意不做成端口**:它是纯 `std`,测试拿临时目录就能覆盖,注入只会平添间接层。
  端口留给真正不可测的外部世界。

顺带修正两处分层错误:

1. **缓存与限频原本被三个 agent 各抄一份**。那是「meowo 怎么管用量」的策略,不是「某个 agent 怎么查
   用量」的知识。现在 `AccountCap::fetch_usage` 只负责去 API 拿一次,读缓存/判鲜/写回统一在编排层。
2. **kimi 经 `super::codex::` 横向引用** codex 的 `decode_jwt_payload` / `unix_to_iso8601`。
   一个插件依赖另一个插件是错的方向,提到了 `meowo_agent::codec`。

claude 的六处 `#[cfg(target_os = "macos")]` 收敛成一次 `ports.keychain.available()` 运行时判断,
两条分支在任意平台都可测。`app/src-tauri` 的 `sha2` 与 `toml_edit` 依赖随之删除。

一个能力**不需要某个端口就不碰它** —— codex 的 `fetch_usage` 不用 `ports.http`,因为它的 rate_limits
就写在本地 rollout 里。这正是能力槽的意义。

### Phase 4 落地记录

`useAgents()` hook 把名单取用收成一处。顺带发现 Sticker 自己又拉了一遍安装列表(`availableAgents()`),
与账号页重复 —— `list_agents()` 一次带回 `installed`,那个请求整个删掉了。

对话页的斜杠命令补全与快速切模型预设后来也从前端硬编码表迁走,但**没有**进静态描述符,而是
按会话查询的动态接口 `agent_chat_ui(provider, cwd)`(见 `chat_ui.rs`):插件声明的内置表
(`slash_commands()` / `model_presets()`,按变体) ∪ 从安装实况**发现**的自定义命令
(`CustomCommandSpec`:claude 的 `commands/*.md`、codex 的 `prompts/`、gemini 的
`commands/**.toml`、opencode 的 `command/`,含项目级目录),外加宿主探测的 CLI 版本
(`--version`,进程级缓存)。装了新命令、换了版本,下一次打开会话就反映。

旧的前端硬编码表给未知 agent 落一份通用 fallback —— 而 fallback 恰恰撒谎:gemini 的对应命令是
`/stats`/`/compress` 而非 `/status`/`/compact`,opencode 是 `/models` 而非 `/model`。现在未知
agent 得到的是「不补全、不给菜单」的诚实降级;模型预设只有 claude 声明(唯一 `/model` 接受
内联参数的 CLI,别家是交互式菜单,在对话页发出去用户什么也看不见)。内置命令的描述文案仍留
前端 i18n(翻译是前端资产);自定义命令的描述从命令文件头里读(那是用户写的数据,不是翻译)。

「能力由安装的 CLI 决定」的边界也如实记下:五家 CLI 都没有「自述能力」的接口(没有任何
`list-commands`),内置表这份知识只能由插件整理,可动态化的是**选表与发现**;版本已随
`ChatUiContext` 传入插件,真出现版本分叉时加分支即可,不必改接口。

新建会话的**启动选项**走同一条路(`launch_options.rs`):插件声明「选择 → CLI flag」的映射表
(claude 的 `--model`/`--permission-mode`,codex 的 `--full-auto`/`--sandbox`,gemini 的
`--yolo`/`--approval-mode`),经 descriptor 下发,面板照表渲染。前端只回传 choice id,
`resolve_launch_args` 按声明表翻译成 argv——未知 option 被忽略、未知 choice 落默认,
**用户输入在任何分支下都进不了命令行**。默认项一律不传 flag(「默认」的诚实含义是「行为由
CLI 自己决定」,不替它猜);模型名随版本频繁更迭的 codex/gemini **不声明**模型栏,kimi/opencode
未调研到稳定 flag、如实不声明——给一个点了会启动失败的下拉,比不给更糟。

`--cc-claude` 变量本身**保留**:它是 claude 的品牌色资产。拔掉的是 `.stk-agent { color: var(--cc-claude) }`
这条规则——它给**所有** agent 的徽标容器抹上 claude 的橙,只因为 kimi/codex 恰好用固定色、不吃 `color`。
任何新 agent 只要用 `currentColor` 绘制徽标就会被染成橙色。改由 `tintStyle(id, connected)` 按 agent 注入,
断开态不给 tint(inline style 优先级高于 class,给了就盖掉 `.stk-agent-off` 的灰)。

## 验收:加一个 gemini 要动哪些文件

**后端两处**:

1. 新增 `crates/meowo-agent/src/plugins/gemini/`(`mod.rs` 必需;`account.rs` / `setup.rs` /
   `telemetry.rs` 按需 —— 不需要的能力就不声明)
2. `crates/meowo-agent/src/registry.rs` 补一行 `&GEMINI`

`meowo-store` / `meowo-reporter` / `meowo-app` 均零改动。

**前端一处**:`providers.tsx` 的资产表加一项(图标 + 是否需要底座 + 可选的 tint 变量)。这**无法消除**——
总得有人提供图标文件。但它是一条纯数据,没有逻辑分支;不加也不会崩,只是显示中性徽标 + id 字符串。

守住这个结论的测试:`providers.test.tsx` 断言未知 agent 走中性兜底且**不等于** claude 的图标;
`api.newsession.test.ts` 断言 `listAgents()` 原样透传后端下发的未知 id、`agentName()` 未知回退成 id 本身;
`Sticker.test.tsx` 渲染一个 `provider: "gemini"` 的会话,断言它显示 `"gemini"` 而非 `"Claude Code"`。
