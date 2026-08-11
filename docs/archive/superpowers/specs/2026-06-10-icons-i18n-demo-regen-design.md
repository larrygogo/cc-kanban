# 设计：状态图标统一 + 界面多语言 + demo.gif 重录

日期：2026-06-10　状态：已获用户批准

三个需求，依序实施：图标统一 → i18n → 重录 GIF（demo 复用真实组件，前两项的改动会自动进画面，最后录一次到位）。

## 已确认的范围决策

| 决策点 | 结论 |
|---|---|
| 图标统一范围 | 只换有图形隐喻处（Tab/空状态的气泡→举手）；RunBadge 流光徽标与吸边条色点保持色彩呈现 |
| Windows 托盘状态图标 | 本期不做（macOS 菜单栏已有，Windows 单独立项） |
| 语言 | 中文 + 英文，字典结构预留扩展 |
| Rust 侧文案 | 纳入（约 11 条：通知标题、托盘菜单、设置窗口标题、错误短标签） |

## 任务一：统一状态图标

隐喻基准（macOS 菜单栏已实现，`app/src-tauri/src/macos/menubar.rs:12-21`）：**待交互=举手（hand.raised），运行中=循环箭头（arrow.triangle.2.circlepath）**。

前端"运行中"已是循环箭头（lucide refresh-cw 路径），一致，不动。改动点：

- `app/src/views/Sticker.tsx` TabIcon waiting 分支（:67-72）：对话气泡 → 举手内联 SVG（lucide `hand` 路径，沿用"手写内联 SVG 抄 lucide 路径并注明"的项目惯例）
- `app/src/views/Sticker.tsx` EmptyIcon waiting 分支（:181-186）：同上
- 同步更新 `Sticker.test.tsx` 相关断言（如有针对图标的）

明确不动：RunBadge（`Sticker.tsx:105-132`，承载 Context 百分比）、CollapsedStrip 色点（`CollapsedStrip.tsx:65-73`）、设置页"检查更新"按钮（`About.tsx:113-122`，独立窗口语境是"刷新"，接受与"运行中"形近）。

## 任务二：界面多语言（中 + 英）

### 方案：自研轻量字典（不引 i18next）

体量约 105 条前端文案 + 11 条 Rust 文案、两个窗口，引 i18next 过重。

**前端**（`app/src/i18n/`）：
- `zh.ts`（默认基准）+ `en.ts`：平面 key 字典；`en` 用 `Record<keyof typeof zh, string>` 约束，缺译/多译编译报错
- 带参数文案用函数值（如 `(n: number) => \`\${n} 分钟前\``）
- React context + `useT()` 钩子提供 `t(key)`；两个窗口入口（App / About）都包 Provider
- 仿 `appearance.ts`（:74-99）：localStorage 缓存当前语言防首屏闪错，监听 `settings-changed` 实时切换

**Rust 侧**（11 条）：`lib.rs` 内简单 `fn tr(lang, key) -> &'static str` 静态 match，不引库。消费点：通知标题（lib.rs:1241,1259，liveness 轮询每 5s 已 load_settings，顺带读语言）、托盘菜单（lib.rs:1431-1432 / menubar.rs:95-96，监听 settings-changed 重建菜单）、设置窗口标题（lib.rs:1395）、错误短标签（crates/meowo-store/src/analyze.rs:33-39 产生的 sentinel 保留，展示层映射——见坑 1）。

### 语言设置

- `Settings` struct（lib.rs:913-932）加 `language: String`（`"auto" | "zh" | "en"`，serde default `"auto"`）；api.ts `Settings` 类型同步
- `auto` = 跟随系统：Rust 加 `sys-locale` crate 检测（通知/托盘在 Rust 侧也要语言），暴露 resolved locale 给前端
- 设置页"通用"区（About.tsx GeneralSection）加语言 Dropdown：跟随系统 / 中文 / English
- 沿用整对象 patch 写回（useSettingsState 的"漏字段会被 serde 默认值覆盖"约束）+ `settings-changed` 广播

### 已识别的坑

1. **`"(未命名会话)"` 是数据库 sentinel**（Rust 写库 + SQL 过滤 + 前后端字符串比较）：不翻译存库值，仅展示层映射（`Sticker.tsx:316` 已有先例）。错误短标签同理：库里存中文 sentinel，前端展示层按映射表翻译。
2. **托盘菜单只在启动时构建**：切语言时重建菜单（Win/Linux：lib.rs setup_tray；macOS：menubar.rs）。
3. **遗留死代码**：LiveView / Overview / ProjectBoard 未被路由（仅测试引用），不纳入翻译，连同测试一并删除。
4. **测试**：4 个测试文件的中文断言改为从 zh 字典引用；新增字典 key 对齐由类型系统保证，无需运行时测试。

## 任务三：重录 demo.gif

前两项合并 main 后执行：`cd app && bun run demo:gif`（内部 node 跑 Playwright，自起 vite:14210，输出 docs/images/demo.gif）。画面自动带上：橙色收尾图标（DemoStage 引用的 128x128@2x.png 已是橙色版）、举手 tab 图标、统一后的中文文案（demo 固定中文）。录后用 `check-gif.mjs` 抽帧检查，注意记忆中的坑：必须 node、预览暗部发白是假象、截图 RGB 转 RGBA（管线已内置）。

## 交付

任务一、二各开分支走 PR（CI 验 macOS 编译），任务三随后直接提交。发版不在本设计范围内，三项合并后由用户决定。
