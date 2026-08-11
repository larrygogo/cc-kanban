# kimi/codex 待交互 + 上下文百分比 — 设计

> 日期：2026-07-07
> 状态：设计已获用户批准，待用户审阅本 spec 后进入实现计划
> 前置：**块A「多 provider 自动接线（ProviderSetup）」先落地**（见 `2026-07-03-provider-setup-design.md` + 同名 plan）。本 spec 是其扩展，补上它有意留白的两个口子。
> 推进顺序：A（旧 plan）→ C（context%）→ B（待交互）。

## 背景与目标

非 claude 会话（kimi / codex）的卡片缺两类实时信息，本质是「靠 statusLine / PermissionRequest 上报的字段对 claude 之外没接」：

1. **待交互状态不显示（症状1，kimi）**：kimi 会话在等命令审批（终端弹「Run this command?」）时，卡片仍显示成「运行中」空心圆环，而非琥珀色「待交互」。因 `pending_review` 只由 `PermissionRequest`/`PreToolUse` hook 写入（`dispatch.rs`），而 kimi 手工接线的 5 事件不含 `PermissionRequest`。provider-setup spec 明确标注「kimi PermissionRequest 待真实 payload 验证后再议」。
2. **上下文百分比不显示（症状2，kimi + codex）**：卡片 `context_pct` 只来自 claude 专属的 statusLine（`meowo-reporter statusline` 写 `session_context`）。kimi-code / codex 是 TUI，无此机制。provider-setup spec 标注「statusline/Context% 的 codex/kimi 对等物……另行立项」——即本 spec 的块 C。

目标：让 kimi 的待交互、以及 kimi + codex 的上下文百分比，与 claude 卡片对齐显示。哲学同项目现状：best-effort、解析失败静默降级、不改 dispatch 的 provider 无关性（provider 差异封进 `Agent` trait 实现）。

## 关键技术事实（源码调研 + 本机实证，2026-07-07）

### kimi 上下文用量（wire.jsonl `usage.record`）

kimi 会话日志 `~/.kimi-code/sessions/**/agents/main/wire.jsonl` 每回合末尾有一条：

```json
{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":10351,"output":503,"inputCacheRead":9728,"inputCacheCreation":0},"usageScope":"turn","time":1782353042668}
```

- `usage.input*` 三项之和（`inputOther + inputCacheRead + inputCacheCreation`）≈ 该回合发送请求时的**完整 context 输入量**（每次请求都把整个 context 作为 input 发送）。取**最后一条** usage.record 即最新占用。
- **无窗口大小字段**。窗口取自 config.toml `[models."kimi-code/kimi-for-coding"] max_context_size = 262144`；`[loop_control] reserved_context_size = 50000`。
- kimi 的 Stop hook 已在读同一个 wire.jsonl（`kimi::read_summary` 取 last_ai/model），context 解析可复用其定位 + 尾部有界读逻辑（`read_range`/`TAIL_BYTES`）。

### codex 上下文用量（rollout `token_count`）

codex rollout `~/.codex/sessions/<Y>/<M>/<D>/rollout-*-<uuid>.jsonl` 内有：

```json
{"type":"event_msg","payload":{"type":"token_count","info":{
  "total_token_usage":{"input_tokens":6766,"cached_input_tokens":4480,"output_tokens":479,...},
  "last_token_usage":{"input_tokens":6766,"cached_input_tokens":4480,...},
  "model_context_window":258400
},"rate_limits":{"primary":{"used_percent":6.0,...}}}}
```

- `info.last_token_usage.input_tokens` = 最近一次请求的 context 输入量；`info.model_context_window` = 窗口大小，**codex 直接给**，无需查配置。取**最后一条 `info` 非 null** 的 token_count（会话开头的 token_count `info` 为 null，只有 rate_limits）。
- **坑**：`rate_limits.*.used_percent` 是账号 5h/7d 配额，**不是** context，切勿混用。
- rollout 定位复用 `codex::read_model` 现有逻辑（hook 带 `transcript_path` 优先，否则 `find_rollout(session_id)`）。文件可达数 MB → 尾部有界读。

### kimi 待交互（PermissionRequest hook，官方源码 + 文档确认，2026-07-07）

MoonshotAI/kimi-code（TS 开源）确认：kimi 交互式等待用户审批时**会触发 hook，无需监控文件**。

- `packages/agent-core/src/agent/permission/index.ts`：`rpc.requestApproval` 存在（交互式要用户拍板）时 `fireAndForgetTrigger('PermissionRequest', {matcherValue: name, inputData: {turnId, toolCallId, toolName, action, toolInput, display}})`——非阻塞，弹审批界面同时发 hook。审批结束发 `PermissionResult`（inputData 多 `decision`/`scope`/`feedback`/`selectedLabel`）。均 observation-only。
- 文档 `docs/{en,zh}/customization/hooks.md`：16 事件全集；`PermissionRequest` =「即将等待用户审批前触发」；`config.toml [[hooks]]` 支持 `event`/`command`/`timeout`(1–600)/`matcher`(正则)。
- **stdin 基础字段**（所有 hook 顶层）：`hook_event_name` + `session_id` + `cwd`（文档 + 测试确认）。→ dispatch 靠 `session_id` 定位天然满足；kimi 的 `toolName` 在 `inputData`（驼峰），我们顶层读 `tool_name` 得 None → 落 `Approval`，正合 kimi 语义。

### 现状锚点

- `Agent` trait（`meowo-reporter/src/agent.rs`）：已有 `stop_outputs`/`resume_args` 等 provider 专属方法，是本 spec 加 `read_context` 的现成落点。
- `dispatch.rs`：`PermissionRequest` 分支已存在（按 `tool_name` 映射 `ExitPlanMode→Plan` / `AskUserQuestion→Question` / else→`Approval`）；`PostToolUse`、`Stop` 分支是块 C 的挂载点。dispatch 目前 provider 无关，靠 `for_provider(provider)` 取 agent。
- `store.set_session_context(session_id, used_pct: Option<i64>, window: Option<i64>, model: Option<&str>, now)`：statusLine 与 dispatch Stop（更 model）共用的写库出口；块 C 复用它写 `used_pct + window`（model 传 None）。
- `setup/kimi.rs`（块A 产出）：`KIMI_EVENTS: [&str; 5]` + `KIMI_EVENT_WHITELIST`（16 事件，含 `PermissionRequest`）。块 B 在此把 5→6。

## 设计

### 块 C — 上下文百分比（kimi + codex）

**抽象**：`Agent` trait 加

```rust
/// 从会话日志读最近一次的上下文占用（claude 走 statusLine，返回 None）。
fn read_context(&self, _ev: &HookEvent) -> Option<ContextUsage> { None }
```

`ContextUsage { used_pct: i64, window: i64 }`（`used_pct` 已 clamp 0–100）。默认实现返回 None → claude 不改。

- **KimiAgent::read_context**：`kimi::read_context(&ev.session_id)` — 定位会话 wire.jsonl（复用 `session_dir` + 尾部有界读），解析最后一条 `usage.record`，`used = inputOther + inputCacheRead + inputCacheCreation`；`window = kimi::context_window(model)`（读 config.toml 对应 model 的 `max_context_size`，回退 262144）。
- **CodexAgent::read_context**：`codex::read_context(ev.transcript_path.as_deref(), &ev.session_id)` — 定位 rollout（复用 `read_model` 的定位），尾部读，找最后一条 `info` 非 null 的 token_count，`used = info.last_token_usage.input_tokens`，`window = info.model_context_window`。

**挂载（dispatch）**：`PostToolUse` 与 `Stop` 两分支在既有处理后追加：

```rust
if let Some(c) = crate::agent::for_provider(provider).read_context(ev) {
    store.set_session_context(&ev.session_id, Some(c.used_pct), Some(c.window), None, now_ms)?;
}
```

`PostToolUse` = 运行中实时刷新（每次工具调用后 wire/rollout 已追加新用量）；`Stop` = 回合收尾兜底。dispatch 仍不含任何 provider 分支——kimi/codex 差异全在各自 `read_context` 实现里。

**算法校准（实现期实测）**：`used_pct = round(used / 分母 * 100)`。分母初值 kimi=`max_context_size`、codex=`model_context_window`；跑真实会话对着各自 TUI 显示的百分比反推是否需扣减（kimi `reserved_context_size` / codex 的系统 baseline）。校准仅动分母常量，不动结构。

### 块 B — 待交互（kimi）

- **接线**：`setup/kimi.rs` 的 `KIMI_EVENTS` 5→6，加 `PermissionRequest`（已在 `KIMI_EVENT_WHITELIST`，防连坐校验通过）。可选再加 `PermissionResult` 用于审批结束时精确清 `pending_review`（否则靠随后的 UserPromptSubmit/PostToolUse 清，够用）。
- **处理**：dispatch 的 `PermissionRequest` 分支不变，kimi 复用。kimi 无 claude 专属的 `ExitPlanMode`/`AskUserQuestion` 工具 → `tool_name` 读不到 → 缺省落 `Approval`（命令审批），语义正确。`HookEvent` 大概率无需改（只依赖顶层 `session_id`，见技术事实）。
- **实测（端到端确认，非可行性验证）**：源码已确认 PermissionRequest 交互式触发 + 顶层带 session_id。实现期跑一次 kimi 审批场景，dump 最终 stdin，确认 `inputData` 是嵌套还是展开（不影响 Approval 路径，仅为把 payload 形态记准），并确认端到端卡片进「待交互」。
- codex 的待交互由块A（`CODEX_EVENTS` 已含 `PermissionRequest`）覆盖，本 spec 不重复。

## 数据流与错误处理

- 全链路 best-effort：会话文件定位失败 / 解析失败 / 字段缺失 → `read_context` 返回 None，卡片该字段留空，绝不阻断 hook、绝不 panic（沿用 kimi Stop 现有哲学）。
- 长文件：kimi wire.jsonl、codex rollout 均可达数 MB → 一律尾部有界读（复用现有 `TAIL_BYTES` 语义），只需最后一条用量事件。
- `PostToolUse` 每次工具调用触发一次 `read_context`（一次尾部有界读 + 解析）——开销与 kimi Stop 现有读 wire.jsonl 同量级，可接受。
- 块 B payload 不兼容的兜底：serde alias 适配；完全不触发则降级（见上）。

## 测试

- **纯解析单测**（不碰文件系统）：
  - `kimi::parse_context(wire_text) -> Option<(used, model)>`：多回合取最后一条 usage.record、三项求和、无 usage.record 返回 None。
  - `codex::parse_context(rollout_text) -> Option<(used, window)>`：跳过 `info=null` 的 token_count、取最后一条有 info 的、读 last_token_usage.input_tokens + model_context_window。
  - 各以本机真实 wire.jsonl / rollout 片段为 fixture。
- **块 B**：`KIMI_EVENTS` 含 PermissionRequest 且仍全在白名单（复用块A 的白名单绊线测试）。
- **回归**：claude `read_context` 默认 None → statusLine 链路零改动；dispatch 原测试全绿。
- **手动验收**：kimi 会话运行中卡片显示 context%（对齐 TUI）；kimi 触发命令审批 → 卡片进「待交互」；codex 会话卡片显示 context%；claude 卡片百分比不变。

## 实测前置（实现期，需用户配合各跑一次）

1. **块 C 校准**：kimi + codex 各跑一个会话，对着 TUI 的 context% 反推分母是否扣减。
2. **块 B 端到端确认**：PermissionRequest 触发已由官方源码确认（非可行性验证）；跑一个 kimi 审批场景 dump 最终 stdin 记准 payload 形态 + 确认卡片进「待交互」。

## 范围外（有意不做）

- **codex 待交互**：块A 已覆盖，不在本 spec。
- **claude context% 改造**：statusLine 现状良好，不动。
- **UI 改动**：`RunBadge` 已支持 `pct=null`→空环、有值→显示；卡片渲染零改动，纯数据补齐。

## 决议记录

1. 范围 = kimi 待交互 + kimi/codex 上下文百分比；自动接线（块A）用现成旧 plan 先行。（用户 2026-07-07 确认）
2. context% 刷新挂 `PostToolUse` + `Stop`（运行中实时 + 回合兜底）。（用户确认）
3. context% 覆盖 kimi + codex 两个 provider。（用户确认）
4. provider 差异封进 `Agent::read_context`，dispatch 保持 provider 无关。（设计约束）
5. 块 C 有分母校准的实测前置；块 B 的 `PermissionRequest` 交互式触发已由官方源码 + 文档确认（不再是不确定项），仅需端到端确认。（2026-07-07 源码调研）
