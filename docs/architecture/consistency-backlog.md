# 设计不一致清单（consistency backlog）

> 2026-08-11 全仓深度调研（后端/前端/文档三路）汇总。每条给出位置与建议做法；
> 「状态」列跟踪收敛进度。修一条勾一条，新发现追加，不搞一次性大重写
> （与 [refactor-roadmap.md](refactor-roadmap.md) 同一纪律）。
> 行号为调研时点参考，以实际代码为准。

## P0 —— 行为可疑 / 用户可感知的不一致

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| P0-1 | `by_id()` 与 `resolve()` 混用，疑似空 provider 行为分叉 | `chat.rs`、`session_query.rs` 等 | **核实后降级**：store 已在全部读取路径把 NULL/空串归一化为 DEFAULT_PROVIDER（`store.rs:905,1073`、`query.rs:462`，测试 `store.rs:1325` 钉住），分叉在实践中被挡住，非活 bug。分工已写进 `registry.rs::by_id` 文档：前端/设置精确 id 用 by_id，可能来自 DB 的值一律 resolve | 已修（文档化） |
| P0-2 | 确认对话框两套：删除账号（最危险操作）走系统 MessageBox，结束会话走应用主题小窗 `appConfirm`；两处注释互相矛盾 | `AccountSection.tsx:358,757,774,790` vs `confirm.tsx` | 统一到 `appConfirm`，删 plugin-dialog 用法 | 修复中 |
| P0-3 | 首帧占位与真实默认不一致：`menuMode` 首帧 `"context"` 真默认 `"button"`（会闪一次）；`api.ts` 文档写 `session_open_in` 默认 chat，后端实为 terminal（有测试钉住） | `Sticker.tsx:181`、`ChatSidebar.tsx:499`、`api.ts:551-555` vs `settings.rs:37` | 占位改成真默认；订正 api.ts 注释 | 修复中（注释） |
| P0-4 | README/官网对 opencode 接线说法冲突：官网 DocsContent 称 opencode 走原生 hook，实际是 `~/.config/opencode/plugin/` 桥接插件（README 与代码一致） | `site/components/pages/DocsContent.tsx:126(zh)/:80(en)` | 订正官网文案 | 待修 |
| P0-5 | README hook 超时描述错误：写「均 5s」，实际 PermissionRequest 310s（脚本与官网均正确） | `README.md:190`、`README.en.md:191` vs `scripts/install-hooks.mjs:72` | 订正 README | 待修 |

## P1 —— 架构纪律被绕开

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| P1-1 | IPC 契约只有一半走 ts-rs：裸 `Serialize` + 前端手写类型，snake_case 与 protocol 的 camelCase 并存 | 前端 `api.ts` | **首批已修**：`AgentDescriptor`（移入 `meowo_agent::descriptor`，组装下沉插件层）与 `ChatUi` 一族共 18 型就地加 ts-rs 导出（`cargo test -p meowo-agent --lib` 生成，CI 契约步骤已扩）；`SelectorAnchor.kind` 顺手升为真枚举。**刻意保持 snake_case 线格式**——camelCase 统一会波及全部测试夹具，收益不成比例，记录不做。**二批已修**：store 的 `LiveSession`/`Session`/`Todo`/`LiveSessionCounts` 导出（i64 逐字段 `ts(type="number")`，与 protocol 同法），api.ts 仅保留 status/column 等取值收窄与 `LiveItem` app 层增补（后者在宿主 crate，刻意不参与契约导出——契约步骤不编译 tauri 宿主）。剩余：`Settings`（settings.rs，同样受宿主 crate 限制，需评估搬 protocol 或接受手写）、`Account`/`ProviderUsage` 等 | 大部分已修 |
| P1-2 | 手写 `listen()` 样板 15 处、cleanup 三种变体，统一的 `useTauriEvent` 仅 4 个使用点；subscribe-first 模式手抄 6 份 | `App.tsx`、`Sticker.tsx`、`ChatSidebar.tsx`、`ChatWindow.tsx`、`useUpdate.ts`、`ManagedTerminal.tsx` 等 | 迁 `useTauriEvent`；为 settings 抽 `useSettings()` hook | 修复中（部分） |
| P1-3 | 重复 invoke 未进 api.ts：`rename_session`(3 份)、`set_archived`(4)、`set_session_note`(2)、`open_project_dir`(3)、`open_new_session_window`(5) | Sticker/ChatSidebar/ChatWindow/About | 收编进 api.ts 封装 | 修复中 |
| P1-4 | 工作区新改动：`persist_queued_image` 使插件 crate 直接写宿主临时目录；32MB 上限常量在 agent/app 两 crate 各定义一遍；`$TEMP/meowo-paste` 目录约定两处独立实现 | `plugins/claude/transcript.rs` vs `chat.rs` | 已收敛：`fsutil::paste_root()` + `PASTE_MAX_BYTES` 单点定义，两处消费；crate 文档改为如实描述落盘边界（transcript 解析管线为纯函数共享，落盘不再上提） | 已修 |
| P1-5 | 纯 DB 命令落在 `terminal.rs`（`set_session_launch_selection`/`session_launch_selections`） | `terminal.rs` → `session_command.rs` | 已移入 `session_command.rs`；返回 `HashMap` 暂不立 DTO（纯 string map，无字段可失配），若将来加字段再进 protocol | 已修 |
| P1-6 | 架构守卫覆盖名单滞后（宿主 6 文件、前端 6 文件手工名单） | `lib.rs`、`architecture.test.ts` | 已修：宿主守卫扩至全部 31 个 src 文件（含 macOS——include_str 不参与 cfg 编译；`#[test]` 属性补作跳过触发器修掉深度计数失衡的盲区）；前端改为目录自动枚举 + 文件数下限断言防枚举空转。扩列时实测全部生产代码本就干净——纪律已内化，守卫防回潮 | 已修 |
| P1-7 | `ChatWindow.tsx` 的 `claudeCommandApprovalDetails`/`kimiCommandApprovalDetails` 语义上仍是前端按 agent 形态分支（以函数名规避了守卫） | `ChatWindow.tsx:76-133` | 解析逻辑下沉到插件 telemetry/chat_ui，前端只渲染 | 待修 |

## P2 —— 重复实现 / 命名漂移

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| P2-1 | agent 身份三种叫法并存：`provider`（DB/DTO/前端）、`agent`（插件层）、`AgentId`（类型），同一文件两词混用 | 全仓 | 约定：持久化字段名保留 `provider`（DB 兼容），代码内新写一律 `agent`；不做批量改名 | 已记录约定 |
| P2-2 | transcript 增量读取两份实现（`read_chat_delta`/`read_transcript_delta`） | `agent/transcript.rs` | 已收敛：`read_chat_delta` 复用 `read_transcript_delta`（`impl AsRef<Path>`），NoChange 不早退以保留空文件首读下发默认模式的行为；`analyze`/`analyze_shared` 本就共用该函数 | 已修 |
| P2-3 | 有界等待子进程逐字重复两处；进程级缓存两种写法；两套手写 LRU | `lib.rs`、`agent/transcript.rs` vs `chat.rs:31-95` | 前两项已收敛：抽 `run_cli_capture(plugin, args, timeout)`，缓存统一 LazyLock。LRU **评估后不合并**：淘汰逻辑仅 5 行同构，但 `ChatMtimes` 的 version 戳还承担并发提交校验（put_if_current），跨 crate 抽象收益小于间接成本 | 已修（LRU 记录不做） |
| P2-4 | `ToolCall` vs `ToolUse` 两套叫法（领域事件 vs IPC），映射函数纯改名 | `agent/transcript.rs:126-153` | 保留双层（边界适配是刻意的），但字段名对齐一种 | 已记录（保留双层） |
| P2-5 | 遗留品牌：备份后缀 `.cckb-bak`、测试临时目录前缀 `cckb-*`；plans 目录 6 份带 `cc-kanban-planN-` 中缀 | `agent/wiring.rs:55`、`lib.rs:2257,2295` | 测试前缀可直接改；`.cckb-bak` 涉及用户盘上既有备份文件，改名需兼容读旧后缀，单独做 | 待修 |
| P2-6 | 行内编辑器两份（`EditorInput`/`EditBox`）+ 两套 CSS；roving 键盘导航两份；点外关闭两种策略（pointerdown vs click 捕获）；board-changed 节流两份同参实现 | `ChatSidebar.tsx:53` vs `Sticker.tsx:62`；`menu.tsx:145` vs `widgets.tsx:78`；`menu.tsx:104` vs `CardContextMenu.tsx:57`；`App.tsx:325` vs `ChatSidebar.tsx:411` | 各收敛一份 | 待修 |
| P2-7 | 图标：`sticker/icons.tsx` 图标库与 ~30 处内联 SVG 并存，chevron/check 形状抄 4-5 份、粗细尺寸不一；无 Button 组件，按钮类名 5 套族群（`.sbtn`/`.ns-btn`/`.stk-act`/`.icon-btn`/`.chat-send-takeover`） | 见前端调研 | 扩 icons.tsx 收编内联 SVG；抽 Button/IconButton；分批做 | 待修 |
| P2-8 | 「置顶」语义两个（星标置顶 star 与窗口置顶 pin）共用一个中文词；存储键/变量/图标名各不同 | `zh.ts:58`、`sticker/types.ts:8-9` | 文案区分（如「星标」/「窗口置顶」） | 待修 |
| P2-9 | 注释语言：4 个模块英文头（`chat.rs`/`managed_terminal.rs`/`session_command.rs`/`session_query.rs`），全仓其余中文；api.ts 混用 `///` 与 `/** */` | 各文件头 | 顺手统一为中文/`/** */` | 修复中（api.ts） |
| P2-10 | CSS：`.chat-compose button` 全局 accent 皮致 8 处「解毒」规则（结构性反模式）；同一"白色叠层"概念 7 种 alpha 裸值；`.chip` 唯一不吃主题变量；flat 主题 78 行逐条覆盖而非 token | `styles.css:3441+`、`:154-279`、`:232-236`、`:2293-2370` | compose 皮改显式类并删解毒规则；叠层立 token；`.chip` 移去 poster 专用 CSS | 待修 |
| P2-11 | Tooltip 与原生 `title` 混用（同一侧栏条目两种气泡） | `ChatSidebar.tsx:673,724,727,826`、`ChatWindow.tsx:257` | 统一 `data-tip`（长文本截断场景确认后再换） | 待修 |
| P2-12 | i18n 7 个 `as Record<string,string>` 逃逸区绕过编译期 key 检查，漏翻静默降级为 id | `zh.ts:130-479`、`en.ts` 对应 | 运行时比对已覆盖 key 对齐；给逃逸区补一条「消费处 fallback 必须可读」的约定即可，不强改类型 | 已记录约定 |
| P2-13 | `getOverview`/`getProjectTasks` 全链路死代码；`Todo` 同名异形；`getLiveSessionsPage` 兼容分支无删除时机 | `api.ts`、`session_query.rs`、`store/query.rs` | 死代码已删（前端 api + 两条 Tauri 命令 + store 的 `overview`/`project_tasks`/`ProjectOverview`/`TaskCard` + 7 个测试）；`Todo` 现由 ts-rs 生成、与 protocol 的 `TodoDto`（hook 输入形）各司其职；兼容分支仍待标注 | 大部分已修 |
| P2-14 | 插件间模式漂移：Relay 结构体可见性与位置不一（F1）；`launch_options` 变体门控只有 kimi 做（F2）；`HookEvent.timeout` 字段承载秒/毫秒两种单位（F5）；事件白名单只有 kimi/gemini 有（F6）；`attachment_mention` 三家逐字重复（F7）；登出策略三分但宿主侧删凭据是同一段隐式逻辑（F9）；`login` 的 `Some(&[])` 隐式表示裸启动（F10）；绊线测试只断言部分 agent（F11）；impl 块方法顺序各异（F12） | `plugins/*/mod.rs` | 定一份「插件排版与惯例」清单写进 plugins/mod.rs rustdoc，新改动照做，存量顺手改 | 待修 |
| P2-15 | 通知指纹四套并行函数靠"让位"约定串联；`pending_review` 三来源靠 `pending_review_live` 校正；存活判定三处口径靠注释宣称一致 | `watch.rs`、`chat.rs:163-182`、`session_query.rs` | 各写一个单点函数供三处调用（roadmap 阶段 3/4 的延续） | 待修 |

## P3 —— 文档与代码偏差（详见 docs 治理）

| # | 问题 | 状态 |
|---|---|---|
| P3-1 | `agent-plugin.md` 能力槽清单停在 5 槽（实际 14+），`FsPort` 前后自相矛盾 | 已修 |
| P3-2 | `refactor-roadmap.md` 进度停在 2026-07-19 | 已修 |
| P3-3 | `2026-07-03-chat-window-design.md` 描述与实现相反且无标注 | 已修（归档+横幅） |
| P3-4 | README 遗漏对话窗/新建会话/多账号/屏幕检测等 0.4-0.5 功能；crates 列表缺 2 个；resume 描述 claude-only；技术栈/CI 描述过时 | 待修 |
| P3-5 | `site/README.md` 结构章节列了不存在的组件 | 待修 |
| P3-6 | 49 份历史 specs/plans 与 2 份 0.1.x release notes 混在活文档里 | 已修（归档） |

## 刻意不做（与 roadmap「不做」对齐）

- 不引入 Redux/Zustand；不给 store 加转发层；不重写 `AgentPlugin` 能力槽本体。
- 不为统一命名而批量改 DB 列/持久化字段（`provider` 列、`meowo-starred` 键保持原样）。
- `TranscriptEvent`→`ChatItem` 双层保留：边界适配是刻意设计，虽当前只有一个消费者。
- `config.rs` 不为缩短文件而机械拆分。
