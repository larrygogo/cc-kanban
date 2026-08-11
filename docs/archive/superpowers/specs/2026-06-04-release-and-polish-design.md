# Meowo 发布与体验完善 设计文档

**日期**：2026-06-04
**状态**：已通过设计评审，待写实现计划

## 目标

为已具备核心功能的 Meowo 桌面贴纸补齐发布管线与试用体验，共 4 个独立模块：发布管线（CI + 在线更新）、屏幕吸边缩略、首次本地会话导入、空态美化。

## 背景与现状

- Rust workspace：`meowo-store`（SQLite 读写 + transcript 标题解析）、`meowo-reporter`（CC hooks 上报，含 transcript 解析与项目命名，**是 lib + bin**）、`app/src-tauri`（meowo-app，Tauri v2 贴纸）。
- 前端：React 18 + Vite + TS，`app/src/views/Sticker.tsx` 单视图（tab：全部/待交互/运行中/已归档）。
- 数据：`~/.meowo/board.db`（WAL）；hooks → meowo-reporter → DB ← meowo-app（notify 文件监听刷新）。
- 已有 Tauri 插件：`tauri-plugin-window-state`、`tauri-plugin-autostart`。
- **当前仓库仅本地 git，无远端、无 CI、无 updater。**

## 已确定的关键决策

1. **发布/更新路线**：GitHub 仓库 `larrygogo/meowo` + GitHub Actions + GitHub Releases + `tauri-plugin-updater`。
2. **吸边缩略形态**：竖条（纵向状态色点 + 计数），悬停滑出完整列表；仅支持左/右两边。
3. **首次导入范围**：近 7 天有活动、最多 30 条，全部标记为 `ended`（历史/已断开）。

## 模块设计

### D. 空态美化（纯前端）

**现状**：`Sticker.tsx` 空列表渲染 `<div className="stk-empty">（空）</div>`。

**设计**：新增 `EmptyState({ tab }: { tab: Tab })` 组件，按 tab 渲染居中的图标 + 主文案 +（部分）提示文案。图标用 lucide 风格内联 SVG（~28px，`var(--cc-text-faint)`）。

| tab | 图标 | 主文案 | 提示 |
|-----|------|--------|------|
| all | 显示器 | 还没有会话 | 在终端运行 Claude Code，进度会自动出现在这里 |
| waiting | 对话气泡 | 没有等待交互的会话 | 有会话需要你回复时会出现在这里 |
| running | 播放 | 当前没有运行中的会话 | （无） |
| archived | 归档盒 | 没有归档的会话 | 点卡片右上角按钮可收纳会话 |

**样式**：`.stk-empty` 改为 flex 纵向居中容器，新增 `.stk-empty-icon`（28px，faint）、`.stk-empty-title`（13px，`--cc-text-dim`）、`.stk-empty-hint`（11px，`--cc-text-faint`，居中换行）。

**测试**：`Sticker.test.tsx` 用 vitest + @testing-library，对四个 tab 在 `data=[]` 时断言渲染对应主文案。

### C. 首次导入近期会话（meowo-reporter lib + meowo-app 启动钩子）

**导入器位置**：`meowo-reporter/src/import.rs`，导出 `pub fn import_recent(store: &Store, now_ms: i64, opts: ImportOpts) -> Result<usize, ImportError>`。复用 meowo-reporter 已有的 `transcript`（标题）与项目命名逻辑。

```rust
pub struct ImportOpts {
    pub within_ms: i64,   // 默认 7 天
    pub max_count: usize, // 默认 30
}
```

**扫描与解析**：
1. 遍历 `~/.claude/projects/*/`，每个 `*.jsonl` 的文件名（去扩展名）= `cc_session_id`。
2. 取每个文件 `mtime`，过滤 `now_ms - mtime <= within_ms`，按 mtime 倒序取前 `max_count`。
3. 每个会话：
   - **cwd**：逐行 JSON 解析，取含顶层 `"cwd"` 字段的条目（取最后一个）。读不到则跳过项目派生，用编码目录名末段兜底作 `project_name`。
   - **title**：`meowo_store::title::title_from_transcript(path)`（custom > ai）。读不到则 `(未命名会话)`。
   - **last_event_at**：文件 mtime（毫秒）。
   - **project**：有 cwd 时复用 meowo-reporter 的 `project_root_and_name(cwd)`（owner/repo 或目录名）；无 cwd 时用兜底名、root 用编码目录名。

**写入**：新增 store 方法
```rust
pub fn import_session(
    &self,
    cc_session_id: &str,
    project_id: i64,
    title: &str,
    cwd: Option<&str>,
    last_event_at: i64,
) -> Result<(), StoreError>
```
直接以 `status='ended'`、`pid=NULL`、`started_at = ended_at = last_event_at = <mtime>` 插入，`ON CONFLICT(cc_session_id) DO NOTHING`（绝不覆盖真实会话）。同时建对应 task（标题），不导入 todo。

**触发（meowo-app）**：启动时检查标记文件 `~/.meowo/imported.json`：
- 不存在 → 后台线程调用 `meowo_reporter::import::import_recent(&store, now_ms, ImportOpts::default())` → 写标记文件（内容含导入条数与时间戳）→ `app.emit("board-changed", ())` 刷新 UI。
- 存在 → 跳过。
- 后台线程执行，不阻塞窗口创建；出错仅记日志不影响启动。

**测试**：`meowo-reporter/tests/import_test.rs`，临时 `HOME`/`USERPROFILE` 指向 tempdir，造 3 个 transcript（1 个超 7 天、1 个含 cwd+ai-title、1 个 cc_session_id 已存在于 DB），断言：仅近 7 天被导入、上限生效、已存在的不被覆盖、title/cwd/project 正确。

### B. 屏幕吸边缩略（meowo-app Rust + 前端）

**状态机**：
- `normal`：浮动、全尺寸、可缩放（现状）。
- `snapped-collapsed`：贴左/右边缘，竖条（宽 ~14px，高保持），仅显示纵向状态色点。
- `snapped-expanded`：贴边缘，恢复全宽，悬停时进入。

**转移**：
| 从 | 事件 | 到 | 动作 |
|----|------|----|----|
| normal | 拖动释放，窗口左或右边缘距屏幕工作区边缘 < 20px | snapped-collapsed | 记住当前尺寸到 localStorage，吸到边缘，缩为竖条 |
| snapped-collapsed | 竖条 `onMouseEnter` | snapped-expanded | 恢复全宽（贴边） |
| snapped-expanded | `onMouseLeave`（防抖 300ms） | snapped-collapsed | 缩回竖条 |
| snapped-* | 拖离边缘（手动 resize/move 出阈值） | normal | 恢复记住的尺寸 |

仅支持左右边（竖条形态）。记住吸附边 + 上次正常尺寸存 `localStorage`，重启沿用。

**Rust（lib.rs）**：
- 纯函数 `fn edge_for_rect(win: Rect, monitor_work: Rect, threshold: i32) -> Option<Edge>`（`Edge::Left|Right`），便于单测。
- 命令 `snap_collapse(edge)`、`snap_expand()`、`snap_restore()`：用 `window.set_position/set_size` 调整；`window.current_monitor()` 取工作区。
- `on_window_event` 处理 `WindowEvent::Moved`：拖动释放后判定边缘，命中则 `app.emit("snap-changed", { collapsed: true, edge })`。

**前端**：
- `App.tsx` 监听 `snap-changed`，维护 `snap` 状态。
- 新增 `CollapsedStrip({ data, edge })`：纵向排列各会话状态色点（复用 indicator 逻辑：灰环/绿点/琥珀/虚线环），`onMouseEnter` → `invoke("snap_expand")`，容器 `onMouseLeave` → 防抖 `invoke("snap_collapse")`。
- 折叠时渲染 `CollapsedStrip`，否则渲染 `Sticker`。

**测试**：Rust 单测 `edge_for_rect`（左命中、右命中、居中不命中、阈值边界）；vitest 测 `CollapsedStrip` 在多状态 data 下渲染对应数量与类名的点。

### A. 发布管线（CI + 在线更新）

#### A1 持续集成
`.github/workflows/ci.yml`，触发 push / PR：
- `cargo test --workspace`、`cargo clippy --workspace -- -D warnings`
- `cd app && bun install && bunx tsc --noEmit && bunx vitest run`
- Windows runner（`windows-latest`）。

#### A2 发布 + 在线更新
1. 建 GitHub 仓库 `larrygogo/meowo`，推送 main。
2. 加 `tauri-plugin-updater`（Rust crate + JS 包 + capability 权限）。
3. `tauri signer generate` 生成密钥对；公钥写入 `tauri.conf.json` → `plugins.updater.pubkey`，`bundle.createUpdaterArtifacts: true`，`endpoints` 指向 GitHub Releases 的 `latest.json`。
4. `.github/workflows/release.yml`：push tag `v*` → `tauri-apps/tauri-action` 在 `windows-latest` 构建、用 `TAURI_SIGNING_PRIVATE_KEY` + `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（仓库 Secrets）签名 → 创建 GitHub Release，上传 NSIS 安装包 + `latest.json`。
5. App 启动后台 `check()`：有新版 → 托盘菜单项与一个轻量提示「有新版本 vX.Y.Z」→ 点击 `downloadAndInstall()` → 重启。
6. **手动前置**（写入 spec/计划，由用户执行）：在 GitHub 仓库 Secrets 添加 `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。

**测试**：CI 工作流本身即验证；updater 检查逻辑做最小化（失败静默、不影响主流程），手动验证一次完整 tag → Release → 客户端收到更新。

## 实现顺序

D（空态）→ C（首次导入）→ B（吸边缩略）→ A1（CI）→ A2（发布+更新）。每步独立可交付、可单独测试与提交。

## 非目标（YAGNI）

- 上/下边缘吸附与横向缩略条。
- 导入历史会话的 todo 明细。
- macOS / Linux 打包与更新（当前仅 Windows）。
- 增量/静默自动更新（先做「提示 + 手动点装」）。
