# docs 目录索引

## 现行文档（描述当前实现，随代码维护）

| 位置 | 内容 |
|---|---|
| [architecture/overview.md](architecture/overview.md) | **入口**：架构总览（分层、协议、插件、前端范式、守卫测试） |
| [architecture/agent-plugin.md](architecture/agent-plugin.md) | Agent 插件架构：原则、迁移史与落地记录（能力槽权威清单在 `registry.rs`） |
| [architecture/refactor-roadmap.md](architecture/refactor-roadmap.md) | 协议收敛路线（阶段 1–5 完成，遗留项转 backlog） |
| [architecture/consistency-backlog.md](architecture/consistency-backlog.md) | 已知设计不一致清单（P0–P3，修一条勾一条） |
| [research/tui-menu-captures-2026-07.md](research/tui-menu-captures-2026-07.md) | TUI 菜单真机取证（活文档，被代码注释引用，**勿移动**） |
| [macos-release-secrets.md](macos-release-secrets.md) | macOS 发布 secrets 清单（与 release.yml 对齐） |
| [macos-acceptance-checklist.md](macos-acceptance-checklist.md) | macOS 真机验收清单 |
| images/ | logo 与演示动图（demo.webp 由 `app/scripts/record-demo.mjs` 按相对路径生成，**勿移动**） |

代码里的架构文档（同等权威）：`app/src-tauri/CLAUDE.md`（线程/平台/检测三条硬纪律）、
`crates/meowo-agent/src/plugins/mod.rs` rustdoc（接入新 agent 指南）、`app/e2e/README.md`。

## 存档（历史记录，不再维护）

`archive/superpowers/` 收录 2026-06 ～ 2026-07 的功能 specs（`specs/*-design.md`）与实现
plans（`plans/*.md`）。这些功能都已上线，但**实现已被后续重构改写**，文中的类型名与文件位置
多数已不存在——仅供考古，不要据此理解当前代码。

特别注意：`archive/superpowers/specs/2026-07-03-chat-window-design.md` 选定的方案从未实施，
对话窗实际走 PTY 路线（详见文件头横幅与 overview.md）。

`archive/release-notes-v0.1.*.md` 是仅有的两份手写 release notes；0.2 起改由
GitHub Releases 承载，官网 changelog 构建期抓取。
