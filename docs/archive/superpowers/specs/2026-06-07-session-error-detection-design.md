# 会话错误状态检测 + 提醒 — 设计

> 日期：2026-06-07
> 状态：已通过设计评审，待写实现计划

## 背景与问题

Meowo 的会话状态**完全靠 5 个 Claude Code hook 推导**（`SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd`）。但 Claude Code **没有"出错"这个 hook 事件**。当 harness 发生致命错误（如"工具调用解析失败，重试也失败"、需要重新登录）时：

- 该回合异常终止，但没有任何信号进库；
- 经验证，致命 abort 时 `Stop` hook **大概率不触发**（abort 后 transcript 只有 `turn_duration`，无 `stop_hook_summary`）；
- 结果：卡片要么一直橙色转圈（`running` 卡住），要么显示成与"正常等你回复"无异的黄色 `waiting`。

**出错信息对用户完全不可见。** 本设计让出错会话在贴纸上以红色露出，并在出错时弹一次桌面通知。

## 检测判据（已用真实 transcript 验证）

致命错误在 transcript JSONL 里表现为 **CC 合成的一条 assistant 正文**，内容是固定文案，例如：

```jsonc
{"type":"assistant","message":{"content":[
  {"type":"thinking","thinking":""},
  {"type":"text","text":"The model's tool call could not be parsed (retry also failed)."}
]}}
```

**判据：会话最新一回合的最后一条 assistant 正文，匹配下列"卡死"文案之一**：

| 匹配规则 | 短中文标签 |
|---------|-----------|
| 含 `could not be parsed (retry also failed)` | 工具调用解析失败 |
| 以 `Please run /login` 开头，或含 `API Error: 403` | 需要重新登录 |
| 以 `Failed to authenticate` 开头，或含 `API Error: 401` | 认证失败 |

**刻意排除**：`API Error: 529 / 500`、`ECONNRESET`、超时等临时性错误——这类在真实库里有 948 条，绝大多数自愈，标红会满屏误报。

### 自愈语义

正常回合的最后 assistant 正文是真实回答，不匹配 → 不报。用户继续、跑通后，最后正文变成新回答 → 自动消红。**无需独立的"清除"逻辑，每次重算即可。**

## 架构

核心决策：**错误状态走"实时计算"，不写 DB，不改 schema**。理由：①`Stop` abort 时不触发，写不进去；②写状态会和 reporter 的 `running/waiting` 互相覆盖；③会话标题本就是这么实时算的（见 `get_live_sessions`），保持一致。

```
meowo-store 新增纯函数 analyze_transcript(path) -> TranscriptInfo { title, error }
   │ 单次扫文件，同时产出标题和错误（替代现在只出标题的 title_from_transcript 读法，避免双读）
   │
   ├─► get_live_sessions（前端拉取时，每个展示中的会话）
   │     → 计算 errored + error_label，挂到 LiveItem
   │     → 前端渲染红点 + 红色错误文案，归入「待交互」tab
   │
   └─► spawn_liveness_watch（5s 轮询，仅对连接中的会话）
         → 计算错误指纹，新错误才弹一次桌面通知（去重）
```

## 组件与改动点

### 1. meowo-store：`analyze_transcript`（新增纯函数）

- 位置：`crates/meowo-store/src/title.rs`（或同模块新文件），与现有标题解析共用一次文件读取。
- 输入：transcript 路径。输出：

  ```rust
  pub struct TurnError {
      pub label: String,        // 短中文标签，用于卡片显示
      pub raw: String,          // 原始英文文案，用于 tooltip
      pub fingerprint: String,  // 出错 assistant 消息的 uuid，用于通知去重
  }
  pub struct TranscriptInfo {
      pub title: Option<String>,
      pub error: Option<TurnError>,
  }
  pub fn analyze_transcript(path: &str) -> TranscriptInfo;
  ```

- 单次遍历：沿用现有 `title_from_transcript` 的逐行扫描，额外记录"最后一条带 text 的 assistant 消息"的正文与 uuid；遍历结束后用判据匹配该正文。
- 错误处理：读不到 / 解析失败 → 两者都为 `None`，绝不 panic（沿用 best-effort 风格）。
- 保留 `resolve_title` 等现有 API；内部改为复用 `analyze_transcript`，对外行为不变。

### 2. meowo-app 后端：`get_live_sessions`（lib.rs）

- 现已对每个展示中的会话调 `resolve_title` 实时解析标题。改为：解析出 transcript 路径后调一次 `analyze_transcript`，同时拿到标题与错误。
- `LiveItem` 增字段：`errored: bool`、`error_label: Option<String>`、`error_raw: Option<String>`。

### 3. meowo-app 后端：桌面通知（lib.rs + 依赖）

- 引入 `tauri-plugin-notification`（加依赖、注册插件、配置 capability/权限）。
- 在 `spawn_liveness_watch` 的 5s 轮询内：
  - 对**连接中**的会话调 `analyze_transcript` 取错误指纹；
  - 维护 `HashMap<cc_session_id, 上次通知的指纹>`（在轮询线程闭包里持有，跨迭代保留）；
  - 去重规则（抽成纯函数 `should_notify(prev: Option<&str>, cur: Option<&str>) -> bool` 便于单测）：
    - 当前有错误且指纹 ≠ 上次 → 弹通知并更新；
    - 指纹相同 → 不弹（同一错误不反复）；
    - 当前无错误 → 清除该条目（下次再错会重新通知）；
  - **启动首次扫描只播种 map、不弹**，避免历史 transcript 一上来炸一堆通知。
- 通知文案：标题「会话出错」，正文「{项目名} · {error_label}」。

### 4. meowo-app 前端（api.ts + Sticker.tsx）

- `LiveSession` 类型增 `errored: boolean`、`error_label: string | null`、`error_raw: string | null`。
- `Sticker.tsx`：
  - `indicator`：优先级 `断开 > errored > running > waiting > 在线`。即 `!connected` 仍显示断开环；连接中且 `errored` → 红点（新增 `.needs-error` 样式）。
  - `match("waiting", ...)` 改为 `connected && (status === "waiting" || l.errored)`；
  - `match("running", ...)` 加 `&& !l.errored`（出错的从运行中挪到待交互）；
  - 卡片 sub 行：`errored` 时显示红色 `error_label`，`title` 属性挂 `error_raw`。

## 设计决策（已与用户确认）

1. **检测范围**：仅"真卡死"类（工具解析失败 + 登录/认证失败），不含临时性 API 错误。
2. **UI 呈现**：归入「待交互」tab + 红色状态点（不新增独立 tab）。
3. **提醒**：本次顺手做桌面通知，带去重防骚扰。
4. **错误文案**：显示短中文标签，原始英文进 tooltip。
5. **断开优先**：断开的会话即使曾出错也只显示"断开"（红色仅在连接中有意义）。

## 测试计划

- **meowo-store `analyze_transcript`**（Rust 单测，喂构造 JSONL 片段）：
  - parse-fail / login / auth 三类命中，标签正确；
  - 正常回合结尾不报；
  - recover 后（错误回合之后又有成功回合）不报；
  - 529/500/ECONNRESET 不报；
  - 标题与错误能在同一次调用中并出。
- **通知去重 `should_notify`**（Rust 纯函数单测）：None→Some 弹、Some→同 Some 不弹、Some→新 Some 弹、Some→None 清除。
- **前端 `match()` / indicator**（vitest，沿用 `Sticker.test.tsx` 模式）：errored 归入 waiting、不进 running；断开优先于 errored。

## 非目标（YAGNI）

- 不覆盖临时性 API 错误（529/500/网络抖动）。
- 不做通知的点击跳转 / 声音 / 免打扰时段（后续若做通用通知功能再统一处理）。
- 不改 DB schema、不新增 hook、不动 meowo-reporter。
- 不做独立「出错」tab。
