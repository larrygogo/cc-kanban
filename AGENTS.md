# AGENTS.md

Meowo — 桌面看板应用，托管多个 AI CLI（claude/kimi/codex/gemini/opencode）会话。Tauri v2：前端 React/TS（`app/`），Rust 后端（`app/src-tauri/`，workspace 含 `meowo-store` / `meowo-agent` / `meowo-reporter` / `meowo-protocol` 四个 crate），官网在 `site/`。

## 构建与验证（改动后必跑）

```bash
cd app && bunx tsc --noEmit          # 类型
cd app && bun run test               # vitest 全量（改动大时）；单文件 bunx vitest run <file>
cd app/src-tauri && cargo check
cd app/src-tauri && cargo clippy --workspace --all-targets   # 必须零警告（CI -D warnings）
cd app/src-tauri && cargo test       # 或按 crate：cargo test -p meowo-store 等
```

- E2E：`cd app && node e2e/run.mjs`——**需要 Node 22 LTS**（Node ≥26 的 undici 与 wdio 不兼容，run.mjs 会预检拒绝）。会构建 E2E 专用二进制（`--features e2e`），WDIO 权限文件临时拷入跑完即删。
- 前端契约：`app/src/generated/contracts/` 由 ts-rs 在 `cargo test` 时生成，改了后端 DTO 要跑测试再提交生成物；CI 校验无 diff。
- i18n：`zh.ts` 是基准，`Dict = typeof zh` 编译期对齐——加键 zh/en 必须同步，有 key 集奇偶测试。

## 代码纪律

- **注释用中文、解释「为什么」**（附上实拍/实测出处），不解释「是什么」。
- 最小改动；不翻案 `docs/ux-backlog.md` 等文档里已定论的决策（如托盘角标不采纳、dev 数据共享是用户明确要求）。标了「用户要求/实拍定案」的注释先问再动。
- 弹层统一走 `escLayers` / `useDismissable`；新建自绘弹层必须接入（Esc 误拒审批有前科）。
- 用户向错误文案走 `formatBackendError` + `i18n/errors.ts` 双语映射，后端错误用 `模块/代码` 结构化 reason。
- 状态判定单点：`activityTone`（`app/src/activity.ts`）是看板/侧栏/后端 `tab_class` 的共同地基，别另起炉灶。
- **改 `api.ts` 的 invoke 传参必须同步 `remote.rs` 桥臂结构体**（deny_unknown_fields；手机端 400 事故出处）。桥有样例 payload 防护测试，改样例时同步。

## 平台注意事项

- Windows 是主开发机；`cfg(macos)` 代码本机编译不到，逻辑自查 + 靠 CI（ci.yml 双矩阵）。动 macOS 代码在 PR 里标注「未经本机编译」。
- 吸边/预览窗仅 Windows 实装（`snap*.rs`，Linux 休眠、macOS 面板无吸边）。
- `app/src-tauri` 下跑测试/构建时注意：运行中的 dev 实例会锁 `target/debug/meowo-app.exe`（os error 5 时先停实例）。
- 真机探针在 `app/src-tauri/tests/probe_*.rs`（ignored，spawn 真实 CLI，可复跑）。

## 发版

1. 写 `docs/release-notes/vX.Y.Z.md`（tag 同名，release.yml 强制，正文自动取自它）。
2. bump `app/src-tauri/Cargo.toml`（版本唯一真源）与 `app/package.json`，`cargo check` 刷新 Cargo.lock。
3. 推 main 等 CI 绿 → `git tag vX.Y.Z && git push origin vX.Y.Z` → release.yml 串行构建 win + macOS(universal)，产 draft release。
4. macOS 真机按 `docs/macos-acceptance-checklist.md` 过一遍。

## 多实例

dev（debug 构建）与安装版共享 `~/.meowo`（数据目录刻意不隔离，用户要求）；dev 有独立 identifier（WebView2 数据/window-state 隔离）且不注册单实例插件——**验证用的 dev 实例用完要收掉**，别留僵尸进程。外库只读聚合（foreign，id+2^40）只在 `MEOWO_DB` 显式指开时激活。
