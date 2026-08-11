# 低风险清理批次 实施计划（前端类型收紧 + 品牌文案中性化 + scripts/docs）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 延续 Phase 0，把前端 provider 收紧成强类型 + 显式默认；把面向所有会话却写死「Claude」的通用文案中性化；给两份 hook SPECS 加交叉同步注释、给手动脚本/README 标注范围与修过时表述。全部低风险、claude 行为零变更。

**Architecture:** `ProviderKey` 联合类型 + `DEFAULT_PROVIDER` 常量定义在 **数据层 `app/src/api.ts`**（对应 Rust 侧 ProviderKey 落在数据层 meowo-store；providers.tsx/demo 从 api.ts 单向 import，符合 UI→数据 的正常分层）。文案/文档改动均为纯字符串/注释，不改 key 名、签名、逻辑。

**Tech Stack:** React/TypeScript（vitest, bun, tsc）+ Rust（meowo-app，仅一处字符串 + 一处注释）。

## Global Constraints

- **零行为变更**：claude 路径产出逐字节一致；所有改动要么是类型收窄（运行时擦除）、要么是纯字符串/注释替换；无逻辑改动。
- `ProviderKey` 联合类型必须与 Rust `meowo_store::ProviderKey` 一致；新增 provider 的同步点共 **4 处**：api.ts 的 `ProviderKey` 联合、providers.tsx 的 `PROVIDERS`、providers.test.tsx 的 `EXPECTED_KEYS`、Rust `meowo_store::ProviderKey::ALL`。
- `providerConfig` 入参**保持 `string`**（防御后端未知值 + 兼容 `providerConfig("__nope__")` 测试）；`PROVIDERS` **保持 `Record<string, ProviderConfig>`**（勿收成 `Record<ProviderKey,...>`，否则逼出 `as` 强转）。
- api.ts 对 ProviderKey 的引用必须是 `import type`/纯类型（运行时擦除）；providers.tsx/demo 从 api.ts import `DEFAULT_PROVIDER`（值）+ `type ProviderKey`。**切勿**让 api.ts 反向 import providers.tsx。
- 中性化术语全批次统一：中文「AI 编程会话」、英文「AI coding session」；机制处统一「各 AI CLI 的 hooks」。
- **保留**：`sticker.agentClaudeCode` 等 claude 专属标签（本就该叫 Claude Code）；`account.*` 整块（账号区 claude 独占，等账号抽象）；README 卡片句的 **Context% 仍限定 Claude**（ccsetup 只包装 claude statusline）。
- **install-hooks.mjs 不加 `--provider` 透传**（它写 claude 专属 settings.json，硬塞 provider 是陷阱）。
- 代码英文、注释/commit message 中文。
- 分支：`refactor/provider-cleanup-batch-20260630`（从 feat/kimi-code-cli-adapter-20260626 当前 HEAD 切出）。
- 验证命令：
  - 前端：`cd app && bun run build`（含 tsc）、`cd app && bun run test`
  - meowo-app 编译（含 sidecar 前置）：`node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`

---

### Task 1: 前端 provider 类型收紧

**Files:**
- Modify: `app/src/api.ts`
- Modify: `app/src/providers.tsx`
- Modify: `app/src/demo/data.ts`
- Modify: `app/src/providers.test.tsx`

**Interfaces:**
- Produces: `api.ts` 导出 `type ProviderKey = "claude"|"kimi"|"codex"` 与 `const DEFAULT_PROVIDER: ProviderKey`；`LiveSession.provider: ProviderKey`。

- [ ] **Step 1: api.ts 新增 ProviderKey + DEFAULT_PROVIDER，收窄 LiveSession.provider**

在 `app/src/api.ts` 顶部 `import { invoke }...` 之后新增：

```ts
/**
 * agent 提供方 key——必须与 Rust 侧 meowo_store::ProviderKey 保持一致。
 * 新增 CLI 的同步点共 4 处：本联合类型、providers.tsx 的 PROVIDERS、
 * providers.test.tsx 的 EXPECTED_KEYS、Rust meowo_store::ProviderKey::ALL。
 */
export type ProviderKey = "claude" | "kimi" | "codex";
/** 缺省 provider，无法识别时回退；与 Rust 侧 DEFAULT_PROVIDER 一致。 */
export const DEFAULT_PROVIDER: ProviderKey = "claude";
```

把 `LiveSession` 里的 provider 字段（约 98-99 行）改为：

```ts
  /** agent 提供方：claude（默认）/ kimi / codex，决定卡片图标与标签。 */
  provider: ProviderKey;
```

- [ ] **Step 2: providers.tsx 兜底改显式默认**

在 `app/src/providers.tsx` 顶部 import 区新增（与现有 `import type { Dict }...` 同处）：

```ts
import { DEFAULT_PROVIDER } from "./api";
```

把 `providerConfig`（约 57-59 行）改为：

```ts
/** 取 provider 配置；未知回退默认 provider。入参保持 string 以防御后端未知值。 */
export function providerConfig(provider: string): ProviderConfig {
  return PROVIDERS[provider] ?? PROVIDERS[DEFAULT_PROVIDER];
}
```

`PROVIDERS` 的声明保持 `export const PROVIDERS: Record<string, ProviderConfig>` 不变。

- [ ] **Step 3: demo/data.ts 同步收窄（唯一会拦 build 的点）**

`app/src/demo/data.ts` 第 2 行的 `import { LiveSession } from "../api";` 改为：

```ts
import { LiveSession, DEFAULT_PROVIDER, type ProviderKey } from "../api";
```

把 makeSession 入参类型里的 `provider?: string;`（约 22 行）改为 `provider?: ProviderKey;`；把默认值 `provider: p.provider ?? "claude",`（约 58 行）改为：

```ts
    provider: p.provider ?? DEFAULT_PROVIDER,
```

- [ ] **Step 4: providers.test.tsx 给 EXPECTED_KEYS 加类型链 + 更新同步注释**

`app/src/providers.test.tsx` 顶部 import 区新增 `import type { ProviderKey } from "./api";`，并把 EXPECTED_KEYS（约 8 行）改为：

```ts
// 期望的 provider key 集合，必须与 Rust 侧 meowo_store::ProviderKey::ALL 保持一致。
// 新增 CLI 的同步点共 4 处：api.ts 的 ProviderKey 联合、providers.tsx 的 PROVIDERS、
// 此 EXPECTED_KEYS、Rust meowo_store::ProviderKey::ALL。类型注解给字面量加单向类型链
// （某元素不再是合法 ProviderKey 时编译报错）；集合完整性仍由下方运行时 toEqual 守护。
const EXPECTED_KEYS: ProviderKey[] = ["claude", "codex", "kimi"];
```

- [ ] **Step 5: 类型检查 + 测试**

Run: `cd app && bun run build`
Expected: tsc 通过、vite build 成功（demo/data.ts 收窄后类型自洽）。

Run: `cd app && bun run test`
Expected: 全绿（providers.test.tsx 3/3，其余不回归）。

- [ ] **Step 6: 提交**

```bash
git add app/src/api.ts app/src/providers.tsx app/src/demo/data.ts app/src/providers.test.tsx
git commit -m "refactor(provider): 前端 provider 收紧为 ProviderKey 联合类型 + 显式默认"
```

---

### Task 2: claude 品牌文案中性化

**Files:**
- Modify: `app/src-tauri/src/settings.rs`
- Modify: `app/src/i18n/zh.ts`
- Modify: `app/src/i18n/en.ts`

**Interfaces:** 无（纯文案字符串值替换，key 名/签名不变）。

- [ ] **Step 1: settings.rs 待审批 question 通知中性化**

在 `app/src-tauri/src/settings.rs` 找到 `notify.pending.question` 的两条（约 120 行 en、129 行 zh）：

把 en 那条 `(\"en\", \"notify.pending.question\") => \"Claude is asking you a question\",` 改为：

```rust
        ("en", "notify.pending.question") => "A session is asking you a question",
```

把 zh 兜底那条 `(_, \"notify.pending.question\") => \"Claude 在问你问题\",` 改为：

```rust
        (_, "notify.pending.question") => "会话在问你问题",
```

（不改 key 名 `notify.pending.question`，不动其它通知文案，不引入 provider 显示名链路。）

- [ ] **Step 2: i18n 通用空态 + 关于页简介中性化**

`app/src/i18n/zh.ts` 的 `empty.allHint`（约 19 行）改为：

```ts
    allHint: "在终端运行 AI 编程会话，进度会自动出现在这里",
```

`app/src/i18n/zh.ts` 的 `about.blurb`（约 152 行）改为：

```ts
    blurb: "常驻桌面贴纸，实时显示所有 AI 编程会话的进度。",
```

`app/src/i18n/en.ts` 的 `allHint`（约 20 行）改为：

```ts
    allHint: "Run an AI coding session in a terminal and progress shows up here",
```

`app/src/i18n/en.ts` 的 `blurb`（约 152 行）改为：

```ts
    blurb: "A desktop sticker showing live progress of all your AI coding sessions.",
```

> 不要动 `account.*`、`sticker.agentClaudeCode`（claude 专属标签，保留）。

- [ ] **Step 3: 验证编译与前端**

Run: `node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`
Expected: 编译通过（settings.rs 仅改字符串字面量，`tr()` 返回类型不变）。

Run: `cd app && bun run build && bun run test`
Expected: tsc 通过（en.ts 受 `Dict = typeof zh` 约束，仅改值不改 key 名，类型对齐照旧）、vitest 全绿。

- [ ] **Step 4: 提交**

```bash
git add app/src-tauri/src/settings.rs app/src/i18n/zh.ts app/src/i18n/en.ts
git commit -m "refactor(copy): 通用文案中性化(通知/空态/简介)，不再写死 Claude"
```

---

### Task 3: scripts/docs 交叉注释 + 范围标注 + README 中性化与纠错

**Files:**
- Modify: `scripts/install-hooks.mjs`
- Modify: `app/src-tauri/src/ccsetup.rs`（仅注释）
- Modify: `README.md`

**Interfaces:** 无（纯注释 + 文档）。

- [ ] **Step 1: 两份 SPECS 互加「保持同步」交叉注释**

`scripts/install-hooks.mjs` 在 `const SPECS = [...]`（约 33 行）上方加一行注释：

```js
// 注意：此表须与 app/src-tauri/src/ccsetup.rs 的 HOOK_SPECS 保持一致（两处各维护一份，改一处必同步另一处）。
```

`app/src-tauri/src/ccsetup.rs` 在 `HOOK_SPECS` 的注释（约 9-10 行）末尾补一行：

```rust
// 注意：此表须与 scripts/install-hooks.mjs 的 SPECS 保持一致。
```

- [ ] **Step 2: install-hooks.mjs 加 Claude-only 头注**

在 `scripts/install-hooks.mjs` 文件头注（约第 3 行后）加两行：

```js
// 仅装 Claude Code 的 hooks（写入 ~/.claude/settings.json；会话默认 provider=claude）。
// codex / kimi 不经此脚本——它们由各自 CLI 的原生 hook 配置接入，hook 命令各带 --provider codex|kimi。
```

- [ ] **Step 3: README 手动安装段标注范围**

在 `README.md` 手动挂 hooks 段（约 155-173 行的 `<details>` 内、169 行后）补一句：

```markdown
> 此脚本仅用于 Claude Code（写入 `~/.claude/settings.json`）。codex / kimi 的接入走各自 CLI 的原生 hook 配置（其 hook 命令带 `--provider codex|kimi`），不经本脚本。
```

- [ ] **Step 4: README 修「五个 hook 事件」过时表述（实际 8 个）**

`README.md` 第 155 行 `补齐五个 hook 事件` → `补齐所需的若干 hook 事件`。

`README.md` 第 169 行 `脚本会给 SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd 五个事件挂上 meowo-reporter` → 改为：

```markdown
脚本会把 meowo-reporter 挂到所需的 hook 事件上（SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd / PermissionRequest，以及 PreToolUse 的 AskUserQuestion / ExitPlanMode，均带 5s 超时上限）
```

`README.md` 第 91 行工作原理图里的事件列表 `触发 hooks(SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd)` → 末尾加 `…`：`触发 hooks(SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd …)`。

- [ ] **Step 5: README 通用文案中性化（保留 Context% 为 Claude 专属）**

`README.md` 第 4 行 tagline `你所有 Claude Code 会话的进度，一眼看全。` → `你所有 AI 编程会话（Claude Code / Codex / Kimi）的进度，一眼看全。`

`README.md` 第 12 行 `通过 Claude Code hooks 捕获每个会话的事件` → `通过各 AI CLI 的 hooks 捕获每个会话的事件`。

`README.md` 第 29 行 `每个 Claude Code 会话一张卡片：项目名、会话标题、**最近一条 AI 正文**(...)、连接状态，以及该会话的 **Context 已用百分比**(取自 Claude Code statusline 的准确值，1M 上下文窗口也能正确显示)。` → 改为（前导从句中性化，Context% 仍限定 Claude）：

```markdown
每个 AI CLI 会话一张卡片：项目名、会话标题、**最近一条 AI 正文**（…）、连接状态；Claude Code 会话还显示 **Context 已用百分比**（取自其 statusline 的准确值，1M 上下文窗口也能正确显示）。
```

`README.md` 第 130 行 `一个装好的 Claude Code(用于产生会话事件)` → `一个装好的 AI 编程 CLI（Claude Code / Codex / Kimi，用于产生会话事件）`。

> 不动第 90-91 行工作原理图中演示 claude+statusline 的具体流程细节（除 Step 4 的事件列表 `…`）；不动 demo（DemoStage.tsx/script.ts）口号——它们渲染进 demo.gif，需连 gif 一起重生，本批次排除。

- [ ] **Step 6: 验证编译（ccsetup.rs 注释改动）**

Run: `node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`
Expected: 编译通过（ccsetup.rs 仅加注释）。

- [ ] **Step 7: 提交**

```bash
git add scripts/install-hooks.mjs app/src-tauri/src/ccsetup.rs README.md
git commit -m "docs(provider): SPECS 交叉注释 + 手动脚本范围标注 + README 中性化与事件纠错"
```

---

## Self-Review

**1. 覆盖**：前端类型收紧（api.ts/providers.tsx/demo/test 4 文件）✅；品牌文案（settings.rs + i18n）✅；scripts/docs（mjs/ccsetup 注释 + README）✅。critique 的修正已纳入：ProviderKey 家放 api.ts（清晰分层）、EXPECTED_KEYS 类型链 + 4 同步点注释、术语统一「AI 编程会话/AI coding session」、不加 --provider 透传、README 不硬编码 provider 列表（仅 tagline/130 行点名一次以助发现）。

**2. 排除项（有意）**：跨语言 SPECS 真正单源（medium，牵动 ccsetup const + 8 单测）；demo DemoStage.tsx:35 / script.ts:29 口号（渲染进 demo.gif，改了与 gif 漂移，需另起 gif 重生任务）；README 第 90-91 流程图 claude+statusline 细节（合理保留）。

**3. 占位符扫描**：无 TBD；每步给确切文件、行号区间、替换前后文本与命令。

**4. 一致性**：ProviderKey/DEFAULT_PROVIDER 定义在 api.ts，providers.tsx/demo/test 均从 api.ts import；providerConfig 入参保持 string；术语全批次统一。

## 本计划之外（后续）

- demo 口号中性化（连 demo.gif 一起重生，见 [[demo-gif-pipeline]]）。
- 跨语言 hook SPECS 真正单源（共享 JSON 运行时读取 + dryrun 比对验收）。
- README 补 codex/kimi 原生 hook 接入写法（新文档任务）。
- 仍属「本计划之外」的更大 Phase：macOS resume 参数化 / meowo-store TranscriptSpec / 账号·UsageScreen 抽象。
