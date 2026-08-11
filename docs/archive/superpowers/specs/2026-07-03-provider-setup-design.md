# 多 provider 自动接线（ProviderSetup）— 设计

> 日期：2026-07-03
> 状态：设计已获用户批准（mjs 取舍经追问确认）；待用户审阅本 spec 后进入实现计划
> 前置：多 provider 接口化路线的下一块（此前已完成 ProviderKey / ProviderAccount / TranscriptSpec 抽象）。消除「开箱即用只对 claude 成立」的最大缺口。

## 背景与目标

meowo-app 启动时的无感接线（`ccsetup.rs`）只覆盖 Claude Code。codex/kimi 会话能上板，靠的是本机开发期**手工写**的配置（`~/.codex/hooks.json`、`~/.kimi-code/config.toml [[hooks]]`），且硬编码指向仓库 dev 构建产物——新装用户 codex/kimi 完全上不了板，本机配置也会因 `cargo clean`/挪目录而静默失效。

目标：启动时对**检测到已安装**的每个 provider 幂等自动接线，行为哲学与 claude 现状完全一致（静默、best-effort、备份一次、原子写、已正确一字不改）。

## 关键技术事实（源码调研 + 本机实证，2026-07-03）

### Codex（codex-cli 0.142.3，源码 tag rust-v0.142.3）

1. hooks 走 `~/.codex/hooks.json`（`$CODEX_HOME` 优先），格式 Claude 同款，**顶层只允许 `{"hooks": {...}}`**（deny_unknown_fields，不能塞自定义键）。feature 默认启用，无需显式开启。
2. 事件全集 10 个：PreToolUse/PermissionRequest/PostToolUse/PreCompact/PostCompact/SessionStart/UserPromptSubmit/SubagentStart/SubagentStop/Stop。**无 SessionEnd**。payload 刻意 Claude 兼容（含 `last_assistant_message`、`prompt` 等；多一个 `turn_id`）。
3. **信任机制**：每条 hook 须在 `~/.codex/config.toml` 的 `[hooks.state.'<hooks.json 绝对路径 display 串>:<snake_case 事件名>:<组索引>:<handler索引>']` 下有匹配的 `trusted_hash`，否则该 hook 不运行，TUI 启动时弹审查界面（Review / Trust all / Continue without）。
4. **trusted_hash 算法已破解并本机 3/3 复算命中**：对归一化身份对象做 canonical JSON（key 字母序、紧凑）后 SHA-256：
   ```
   {"event_name":"<snake_case>","hooks":[{"async":false,"command":"<命令原串>","timeout":<t>,"type":"command"}]}
   ```
   （matcher 非 None 时追加 `"matcher"` 键；UserPromptSubmit/Stop 的 matcher 被强制归一化为 None。）⇒ **第三方可预信任**。
5. 风险：哈希算法是内部实现（文档不给算法），跨版本可能变。兜底无损：失配时 codex TUI 提示一次「Trust all and continue」。

### Kimi（kimi-code 0.20.1，源码 MoonshotAI/kimi-code）

1. hooks 在 `~/.kimi-code/config.toml`（`$KIMI_SHARE_DIR` 优先）顶层 `[[hooks]]` 数组，`{event, command, timeout}`。**无信任机制**，新 session 即生效。事件全集 16 个（含 CC 的全部 7 个消化事件）。
2. **kimi 自己会全量重写 config.toml**（login/provider 命令等触发）：数据保留（未知键透传、[[hooks]] round-trip），**注释与排版全丢**⇒ 幂等判定不得依赖注释标记，须按 `(event, command)` 内容匹配。
3. **一条非法 hook 连坐整段**：event 名拼错/字段非法会让 kimi 该次运行静默忽略**全部** hooks ⇒ 写前必须白名单校验 event 名、timeout 限 1–600。
4. kimi 对语法损坏的 config.toml 拒绝写入（保护）；我们同样：解析失败即放弃。
5. 旧 `~/.kimi` 目录是前代 CLI 遗产，不读不写。

### 现状锚点

- `ccsetup.rs` 骨架全部可复用：严格认领判定（防误伤用户 hook）、幂等合并、备份一次（`.cckb-bak`）+ tmp+rename 原子写、reporter 路径两级解析（优先复用已配置且存在的路径 → 否则 app 同目录 sidecar）。
- 集成点：`lib.rs:2363` `std::thread::spawn(ccsetup::apply)`。
- dispatch.rs 消化 7 种事件名，provider 无关。
- workspace 无 TOML 依赖，需新增 `toml_edit`（结构保持编辑，用于 kimi config.toml 与 codex config.toml 的 hooks.state 子树）。

## 方案取舍

| 方案 | 说明 | 结论 |
|------|------|------|
| **A. setup/ 模块 + ProviderSetup trait（选定）** | 模仿 `account/` 的既有组织：`setup/{mod,claude,codex,kimi}.rs`，trait + 注册表遍历；claude.rs = 现 ccsetup.rs 平移零行为变更 | 与项目 trait 惯例同构，每 provider 独立可测 |
| B. ccsetup.rs 单文件内加函数 | 少仪式 | 文件已 525 行，三 provider 逻辑异构（JSON/JSON+TOML/TOML），必然膨胀失焦；弃 |
| C. 下沉为 `meowo-reporter install` 子命令 | app 与手动脚本共用 | 改变 reporter「一次性 hook 进程」职责定位，sidecar 调用参数化复杂；YAGNI，弃 |

## 设计

### 模块结构（`app/src-tauri/src/setup/`）

- `mod.rs`：`trait ProviderSetup { fn provider(&self) -> ProviderKey; fn detect(&self) -> bool; fn apply(&self); }` + `ALL_SETUP` 注册表 + `apply_all()`（遍历、每个独立 best-effort，一家失败不影响他家）+ 共享工具（备份一次、原子写、sidecar 路径解析）。
- `claude.rs`：现 `ccsetup.rs` **原样平移**（含全部测试），仅套上 trait 壳。行为零变更（含 statusline 包装——claude 专属概念，留在 claude.rs）。
- `codex.rs`、`kimi.rs`：新增。
- `lib.rs:2363` 改为 `std::thread::spawn(setup::apply_all)`。
- `detect()`：对应数据目录存在即视为已安装（claude: `~/.claude`；codex: `~/.codex`；kimi: `~/.kimi-code`；各自尊重 env 覆盖 CLAUDE_CONFIG_DIR/CODEX_HOME/KIMI_SHARE_DIR）。不存在 → 静默跳过。

### reporter 命令形态（与 claude 的差异）

claude 的 hook command 是**裸路径**（`"<path>"`），codex/kimi 的是**带参数**（`<path> --provider codex|kimi`）。认领判定各 provider 自持：解析命令首 token 的 file_name == meowo-reporter[.exe] 且余参恰为 `--provider <self>`（同 claude 的严格哲学：不裸 contains，不误伤用户自有 hook）。路径解析沿用两级语义：优先复用该 provider 配置中已存在且文件存在的 reporter 路径（不折腾工作中的配置——本机 dev 路径因此被保留），否则 app 同目录 sidecar。

### codex.rs

1. **hooks.json 幂等合并**：读 `~/.codex/hooks.json`（不存在 → 从 `{"hooks":{}}` 起）；解析失败 → 放弃。按 SPECS 逐事件：已有认领条目 → 路径不符则更新；无 → 追加 `{type:"command", command:"<path> --provider codex", timeout:5}`。顶层保持仅 `hooks` 键。
2. **SPECS（5 事件）**：SessionStart / UserPromptSubmit / PostToolUse / Stop / **PermissionRequest**。比本机手工版多 PostToolUse（工具活动展示）与 PermissionRequest（待审批子态）；无 SessionEnd（codex 不支持，会话收尾靠 Stop + 既有 liveness 判活）。不配 PreToolUse（其 matcher 目标 AskUserQuestion/ExitPlanMode 是 claude 专属工具，codex 永不触发）。实现期以真实 payload 验证 PostToolUse/PermissionRequest 与 dispatch 的兼容性，不符则裁掉该事件（SPECS 表收缩即可，不影响骨架）。
3. **预信任**：对 hooks.json 中**每条我们认领的** hook，按已验证配方计算 trusted_hash，用 `toml_edit` 写入 `~/.codex/config.toml` 的 `[hooks.state]` 子树（只动该子树）。纪律：state 键中的路径与 `Path::display()` 输出逐字符一致（Windows 反斜杠）；每组恒单 handler、索引恒 `0:0`；显式写 timeout；条目内容任何变动同步重算哈希。已有等值哈希 → 不动（幂等）。
4. **顺序**：先 hooks.json 落盘成功，再写 config.toml 信任（反序会留下指向不存在配置的信任残渣；正序失败的最坏情形 = codex 弹一次审查提示，无损）。
5. 备份：两文件各自 `.cckb-bak` 一次（hooks.json 全新创建则无需备份）。原子写。

### kimi.rs

1. `toml_edit` 读改写 `~/.kimi-code/config.toml`：解析失败 → 放弃不写（与 kimi 自身写保护一致）。
2. **SPECS（5 事件）**：SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd（与本机手工版一致，即当前已验证工作的组合）。PermissionRequest 待实现期以真实 payload 验证后决定是否加入。
3. 幂等合并：遍历 `[[hooks]]`，按认领判定定位我方条目；路径不符则更新 command；缺事件则追加 `{event, command, timeout=5}`。用户自有 hooks 条目一概不动。
4. **写前自校验**：即将写出的每条 event 名 ∈ kimi 16 事件白名单、timeout ∈ [1,600]——防连坐（一条非法项会让 kimi 静默禁用全部 hooks）。
5. 备份 `.cckb-bak` 一次 + 原子写。不依赖注释做任何标记。

### 数据流与错误处理

- `apply_all` 在启动后台线程跑，每 provider 独立 try：读不到/解析失败/找不到 reporter/目录不存在 → 该 provider 静默跳过，绝不影响启动、绝不写坏文件（沿用 claude 现有哲学）。
- codex 哈希算法跨版本漂移的兜底：hook 不运行 + codex TUI 一次性审查提示，用户点 Trust all 即恢复；我们下次启动幂等判定发现哈希已被 codex 改写为等值 → 不再折腾。
- 本机手工配置的接管：路径复用语义保留 dev 路径；缺失事件（codex 的 PostToolUse/PermissionRequest）自动补齐并预信任。

### 测试

- 纯函数单测（每 provider）：合并逻辑（空配置/已有用户 hooks/路径升级/幂等二跑无改动）、认领判定（带参数形态、不误伤）、codex trusted_hash 计算（以本机三条真实哈希为测试向量）、kimi 白名单校验拒绝非法项。
- dry-run ignored 测试（对真实配置副本跑 apply，人工核对），沿用 claude 现有模式。
- 手动验收：本机三 provider 接线后各起一个新会话上板；codex TUI 不弹审查提示（预信任生效）。
- 回归：claude 平移后 `ccsetup` 原测试全绿、真机接线行为不变。

### 范围外（有意不做）

- **`install-hooks.mjs` 维持 claude-only**（经用户询问后确认取舍）：装 app 的用户已被自动接线全覆盖，mjs 仅剩「从源码编译不启动 app」的开发者场景；用 JS 重实现 trusted_hash 与结构保持 TOML 编辑会造成 Rust/JS 双实现永久同步负担（claude SPECS 双份同步已是记录在案的债，不再翻倍）。**本期补偿**：README 补 codex/kimi 手动接线配置片段。**指定后续项（终态）**：给 meowo-reporter 加 `setup` 子命令——接线逻辑放 meowo-reporter lib（meowo-app 已 link，自动接线调同一份代码），mjs 退役、双份 SPECS 债一并清除。为此本期 setup/ 模块的合并逻辑保持纯函数化（不依赖 Tauri/app 状态），未来跨 crate 迁移仅是搬运。
- statusline/Context% 的 codex/kimi 对等物：属 TranscriptSpec 孤岛，另行立项。
- 接线开关 UI：与 claude 对齐的静默哲学，不加设置项（用户离席未确认，若要开关请在审阅时提出）。

## 决议记录

1. 触发策略 = 与 claude 对齐：启动静默幂等接线、检测到 CLI 才动、无设置开关。（用户默认认可）
2. codex SPECS 含 PostToolUse + PermissionRequest（比手工版多两个事件，解锁工具活动与待审批子态）；kimi 维持手工版 5 事件。（用户默认认可，实现期以真实 payload 验证）
3. codex 预信任采用「复算 trusted_hash 写入 config.toml」，接受算法漂移时退化为 codex 内一次 Trust all 的兜底。（用户默认认可）
4. `install-hooks.mjs` 不扩展；README 补手动片段；`meowo-reporter setup` 子命令为指定后续项。（用户追问后确认）
