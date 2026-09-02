# 交互体验优化清单（UX backlog）

> 2026-08-12 六路并行深度调研汇总：看板主窗 / 对话窗 / 托管终端与新建会话 /
> 设置·账号·引导·更新 / 窗口管理与通知 / 全局交互基础设施。
> 每条给出位置与建议做法；行号为调研时点参考，以实际代码为准。
> 与 [consistency-backlog](architecture/consistency-backlog.md) 同纪律：
> 修一条勾一条，新发现追加，不搞一次性大重写。
> 2026-08-27 复核修复一轮：B-5/7/8/14/15、C-3/5/6/15、G-5/12/14、S-7/11/15 已修，
> W-13 指纹部分已修（toast 时长/按钮遗留）；B-14 轨道跳转、C-15 门态附件钮为小遗留。
> 同日第二轮：C-1/7/8/9/10/12、B-9、B-16(FLIP)、W-5/6、S-2/5、G-2/4 已修；
> C-9 彻底零位移 overlay 方案为大改遗留。
> 同日第三轮：C-4/13/16/17/18、B-4/11/12、G-16、T-10/11、S-8/12/13/14 已修，
> T-1 经核实已修；T-12 补 toast（板内占位卡建议单独立项）；
> S-14 danger 收口其余五处调用点、S-8 安装取消为小遗留。
> 同日第四轮：T-2/3/4/5/8/9/13、W-2/7/9/11/13/16/18、G-1/3、S-1、C-11、B-6 已修，
> W-10（琥珀角标两态）/W-15（文案说明）/C-11（多问题表单）留核心外尾巴；
> 另修 T-17（kimi 屏幕规则失配误报待交互）与滚动条横向溢出、命令输出 ANSI 乱码两处实拍回归。
> 同日第五轮：T-6/7(判重冒泡)/16、S-8/9/14 已修；W-10 托盘角标两轮实拍后定为「不采纳」
> （图标回到发布版原样、dev 角标保留），W-11 补双击=打开对话窗消抖。
> 同日第六轮：T-14/15、U0-11 尾（终端右键菜单）、W-8/12、W-4 尾已修；
> W-12 的 panel.rs 为 cfg(macos) 未在本机编译验证（靠 CI）；T-15 降级通道 1.7s 叠加未做。
> 同日收尾轮：W-17（四套存储收敛 settings.json）、结构性 8（PTY 事件 app 级 emit）、
> C-14（transcript watch push，兜底轮询降 2s）、T-15 尾（board-urgent 高优通道）、
> T-12 尾（pending PTY 占位卡）、C-11 尾（按题 keyed 作答）、W-15 尾（UNUserNotificationCenter）、
> G-13、U1-28 全部落地；另有实拍迭代：B-1 状态槽形态统一、子任务面板逐分支、
> Claude 图标选择器误伤、缩略条 pin 标记撤除。macOS 侧改动均靠 CI 验证。
> 2026-08-31 新一轮六面复审 + 六路并行修复（64 条新发现，62 修 2 缓）：
> 共同模式是上轮「策略级」修复的相邻路径漏网——
> ① 修复覆盖面：撤销归档缺整窗重查（B-6）、收回守卫不认键盘焦点（U1-10）、
> 缩略条缺置信度分层（T-15）、弱化徽标 tooltip 未弱化、侧栏 ⋯/题面 tablist/IME 守卫/
> hover:none 兜底/role=alert 内嵌按钮等「主面修了副本没跟上」一批；
> ② 弹层纪律：LineagePopover 与 ModelPicker 两个自绘弹层未接 escLayers/useDismissable，
> 各带一个 Esc 误拒/误关窗洞；toast 三渲三锚点互压收敛为统一栈；
> ③ 异步链衔接：「结束并恢复」改为等 pty-exit 再 start（ConPTY 僵死场景）、
> 通知连坐清空后决策版指纹重置补发（W-13 内部矛盾）、倒计时归零切超时态；
> ④ 三态容器混用：侧栏/归档区首载失败伪装空态、登出成功穿错误红、更新失败三态分文案；
> ⑤ 错误本地化：settings 保存链接上 formatBackendError、errors.ts 补 INPUT_BACKLOGGED/
> RESIZE_BUSY/proxy scheme/relay 明文守卫四条 + 外部终端条开 tail；
> ⑥ useSettingsState 写队列提为模块级共享（S-2 常驻分区叠加出的跨实例写-写竞态）；
> ⑦ 确认窗底色改 --cc-window-bg（低不透明度下幽灵窗）、托盘单击折叠态走 recall_sticker。
> 缓办两条：Windows 跨重启通知死点击（需启动解析 COM 激活参数）、macOS 通知授权拒绝的
> 设置页可见性（macOS-only 需实拍）。macOS notify.rs 改动本轮仍靠 CI 编译背书。
> 同日尾轮（两条缓办收口 + 小遗留一批）：
> - Windows 死点击：调研确认 tauri-winrt-notification 0.7.2 的 toast XML 无 launch 属性、
>   无 COM activator API，跨进程激活需 sparse package/MSIX 身份（打包形态专项，暂不立项）。
>   落「启动清场」：clear_dead_toasts_at_startup 在 liveness 启动前整清上一进程残留
>   （dev 借用 PowerShell AUMID 故跳过）；补发无需新增——首轮扫描对 pending/blocked
>   不播种，第二轮 prev=None 自然重弹（U0-5 机制）。
> - macOS 授权可见性：notification_authorization_status / open_notification_settings
>   两条命令（getNotificationSettings 异步回调 + spawn_blocking 等待；直达系统设置走
>   T-6 同款 spawn_detached open）；设置页通知开关旁 denied 时给「已在系统设置中关闭 ·
>   打开系统设置」行，前端 IS_MAC 门控查询。cfg(macos) 靠 CI 编译。
> - 小遗留：user-resize-end 重吸附失败回滚落 normal（语义不同于 onExpand/recall 的
>   回原态）；Dropdown typeahead 补四条单测；AccountSection patchError 行补 × 关闭；
>   S-2 搜索态挂载账号分区的配额扇出写注释闭环为有意取舍。
> 同日专项轮（四个结构性专项全部落地）：C-9 审批/题面卡改零位移 overlay
> （零高度锚定 + returnFocusTo 焦点归还）、G-11 尾（--cc-ui 下发对话窗，
> 定值逐面归阶梯）、W-2 原生吸附预览窗（snap-preview 原生窗，Windows 实拍）、
> U1-14 尾（Ctrl+K 扩成命令面板，会话+命令双组混合过滤）。至此交互 backlog
> 无开放项；剩「Windows MSIX 打包专项」与 macOS 实拍验证两个环境依赖项。
> 2026-09-02 实拍复轮：C-19（子任务吸顶标题与视口顶之间 24px 缝隙，滚动内容
> 从缝里透出）、C-20（运行状态条非 Bash 工具只显示工具名）各修各的。
> 2026-09-02 第七轮：六路并行读码 + 实拍新扫 69 条（高 5 / 中 34 / 低 30），
> 编号 7B/7C/7T/7S/7G/7M，见「五」节。同日全部标修（三处有意的行为变更与
> 未经本机验证项列在该节导语）。
> 同日复核（3eedf30 六路逐条对照 + tsc/vitest/clippy/cargo test 全绿）：44 条真修、
> 17 条部分（副本或第二半没跟上，状态列已注明）、4 条已修带小遗留、3 条回归（7G-13
> 浅色失败胶囊变绿字、7M-3×7M-11 缩放互斥、7S-6 端口行撑爆）已修补；本轮 69 条几乎无配套测试，
> 建议 7T-2/4/6、7M-6 补纯函数单测。dist-mobile 需重建才在手机端生效。

## 〇、横切主题（先读这节）

单条问题散在各面，但背后是七个系统性模式，修复时按主题收敛比逐条打补丁划算：

1. **反馈缺失 / 失败静默**：写操作三种反馈方式并存（归档乐观、重命名/便签不乐观）；
   点击跳转、恢复会话、发送消息全程无 in-flight 态；失败静默的地方成片
   （归档、loadMore、审批提交、三个设置分区的保存、确认窗打不开、通知点击跳转失败）。
2. **状态语言单通道**：状态只靠「颜色 + 动效」双通道，色弱与 reduced-motion 下双双失效；
   waiting 黄与 pending 琥珀几乎不可分；缩略条自写一套状态映射与看板口径不一致。
3. **键盘体系缺位**：零全局快捷键；卡片菜单完全无键盘导航；Esc 靠三套互不兼容的
   「让路」约定串联且有误拒审批的洞；focus ring 多处 `outline:none` 无替代。
4. **首帧占位 ≠ 真实默认**：4 处（menuMode ×2、sticker_style、opacity），每次开窗闪一下。
5. **文案承诺与实现不符**：「上方保留了终端输出」但被遮罩盖死；关自动更新后谎报
   「已是最新版本」；引导提到不存在的「已归档」tab；README 说「拖离恢复」但缩略条不可拖；
   「外观更改即时生效」在设置窗自己身上看不到。
6. **正确性级竞态混在 UX 里**：虚拟列表 index key、dismissInteractiveQuestion 竞态、
   snap_expand 与 pin effect 打架、重启后已阻塞会话永不通知、折叠后搜索词残留。
7. **信任与告知**：hooks 静默写 `~/.claude/settings.json` 零告知；首启自动导入历史会话
   零说明；「重启并更新」静默杀掉全部托管会话。

## 一、P0 —— 正确性级缺陷（行为错误，不只是体验）

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| U0-1 | 虚拟列表未传 `getItemKey`，默认按 index 复用测量与 DOM：列表重排（活跃会话跳顶）时高度抽搐、焦点错位、**误点打开错误会话的终端** | `Sticker.tsx:389-401,638` | `getItemKey: (i) => shown[i].session.id`，一行改动 | 已修 |
| U0-2 | 切换到等待 AskUserQuestion 超过 3s 领养窗口的会话时，effect 链用新 sessionId 调 `dismissInteractiveQuestion` → 后端无条件清掉该会话题面，**题面卡永不出现** | `ChatWindow.tsx:1210-1229`、`managed_terminal.rs:365` | dismiss 只在本窗口显式收卡/答完时触发（`dismissTargetRef` 记 sessionId+requestId） | 已修 |
| U0-3 | 偷看展开丢置顶：`snap_expand` 末尾强制置顶，Sticker 挂载后 pin effect 又 `setAlwaysOnTop(pinned)` 盖回去；pin=off 时展开的看板沉到活动窗口之下，表现为「悬停后闪一下就没了」 | `snap.rs:346` × `Sticker.tsx:210-217` | pin effect 加 `mode==="normal"` 守卫，或置顶所有权收归后端按 mode+pin 单点计算 | 已修 |
| U0-4 | 缩略条状态点自写映射，忽略 `pending_review`/`screen_state`：待审批会话显示成绿色运行点，「有人在等你」信号被抹掉 | `CollapsedStrip.tsx:80-93` vs `api.ts:205-212` | 改调全局 `sessionTone()`，pending/blocked 加琥珀档 | 已修 |
| U0-5 | 重启后已阻塞/待交互会话永不提醒：首轮扫描只播种不弹，blocked 指纹恒为 `"blocked"` 不再变化 | `watch.rs:504,344-347,761` | 首轮发现的 blocked/waiting 延迟一周期补发，或给一条「你有 N 个会话在等你」的启动摘要 | 已修 |
| U0-6 | 重命名 modal 焦点在按钮上按 Esc → 不在 tagName 白名单、defaultPrevented 为 false → 窗口级监听**误拒 agent 的审批请求**，modal 还开着 | `ChatWindow.tsx:321,1953-1956` | Esc 挂 `.chat-modal` 容器 + stopPropagation；根治见 U3-2 escStack | 已修 |
| U0-7 | 关闭自动更新后「关于」谎报「已是最新版本」（从未检查过） | `useUpdate.ts:126-129`、`About.tsx:519` | 未检查时状态为 unknown，只显示版本号 | 已修 |
| U0-8 | 登录超时/取消文案指向「刷新」按钮，但该按钮只在已登录态渲染——未登录用户无路可走，只能重开设置窗 | `zh.ts:584,586` vs `AccountSection.tsx:605-631` | 窗口 focus 时一并 `loadAccounts()`；或未登录态给「我已登录，检查」按钮 | 已修 |
| U0-9 | 折叠后搜索词残留：CollapsedStrip 卸载 Sticker 但不清 `search`，再展开时搜索框收起、列表仍被过滤，空态显示「还没有会话」——用户会认定会话全丢了 | `App.tsx:141,698-700`、`Sticker.tsx:170,591` | Sticker 卸载时清 search，或 `searchOpen` 提升到 App 与 search 同源 | 已修 |
| U0-10 | Windows 上点连接中的托管会话不做 attach 去重（macOS 有），且全程无 in-flight 态防连点：每点一次多开一个镜像终端标签 | `terminal.rs:1855-1904` vs `:1870-1890`、`Sticker.tsx:289-323` | attach 握手已带 pid，按 pid 找已有镜像窗口 `force_foreground`；卡片点击加 pending 态 + 在飞去重 | 已修 |
| U0-11 | 生产构建下托管终端**无法复制**：Ctrl+C 发 `^C` 中断 agent，Ctrl+Shift+C 被 devtools-guard 吃掉，右键菜单 PROD 全局禁用 | `ManagedTerminal.tsx:232-237`、`devtools-guard.ts:7,14-19` | 有选区时 Ctrl/Cmd+C 走 `getSelection()`+clipboard；devtools-guard 放行 Ctrl+Shift+C；补终端右键菜单 | 已修（复制快捷键此前已落地；本轮补应用内右键菜单：复制/粘贴/全选/搜索，TUI 鼠标上报让位规则保留，Esc 层/roving 复用 useDismissable） |
| U0-12 | 「重启并更新」无确认直接 relaunch，退出时 `exit_ptys.shutdown()` 杀掉全部托管会话；`restartHint` 文案还在说「不会打断当前工作」 | `useUpdate.ts:55-64`、`lib.rs:1175-1177`、`zh.ts:650` | install 前查运行中会话数，n>0 走 `appConfirm`；订正文案 | 已修 |
| U0-13 | 会话退出遮罩 94% 不透明铺满终端且吃指针事件，文案却说「上方保留了终端输出」——排查退出原因最需要的内容看不见滚不动 | `styles.css:4003`、`zh.ts:253` | 退出态改 banner 或遮罩 `pointer-events:none` + 大降不透明度 | 已修（二次迭代：底部横条方案被实拍否掉——TUI 退出清屏后窄条被读成「接管框不见了」；现为居中醒目卡片 + 遮罩层指针穿透，框在熟悉位置、输出仍可滚可选，测试钉住） |
| U0-14 | 安装 agent 途中切换下拉 = 卡片卸载、`install-done` 监听注销：回来显示「安装」按钮诱导二次安装，失败原因永久丢失（登录已为此把状态提升到页面级，安装没做） | `AccountSection.tsx:197-202,309,1134-1135` | 仿 `useLoginOperations` 抽 `useInstallOperations` 提到 Section 层 | 已修 |
| U0-15 | 恢复失败直出原始 OS 错误（`os error 2`）：CLI 被卸载 / 目录不存在均无前置校验（新建路径有 `validate_new_session_cwd`，恢复路径没有） | `pty.rs:987-990`、`terminal.rs:1678-1687` | 恢复前校验目录存在与可执行（`path_has_exe` 已有），返回可本地化错误码 + 带动作提示 | 已修 |

## 二、速赢（高收益 / 低成本，多数一行到几十行）

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| U1-1 | 卡片长标题截断且无 tooltip（卡片级 `data-tip` 是「打开会话」占掉槽位） | `Sticker.tsx:706,688` | `.stk-title` 加 `data-tip={title}`（Tooltip 内层优先） | 已修 |
| U1-2 | 重命名/便签非乐观更新：Enter 后显示旧值几百 ms~1s，用户以为没保存 | `Sticker.tsx:242-251,277-284` | 与归档对齐走乐观更新，失败用 focusNotice 回滚 | 已修 |
| U1-3 | 对话输入框不随内容长高（`rows=1` + min-height 顶死，max-height:150px 是死代码），写 5 行提示词只能内部滚动 | `ChatWindow.tsx:2435-2438`、`styles.css:3454` | onChange 里 `scrollHeight` 自适应或 `field-sizing:content` | 已修 |
| U1-4 | 发送后自己的消息不上屏，最长等 650ms 轮询，空闲态发送后 0.65~2s 零变化 | `ChatWindow.tsx:1567-1572,1049` | 本地乐观插入 `user_text`（pending 标记），transcript 出现后按排队回执水位线替换 | 已修 |
| U1-5 | 用户上翻后无「回到底部/有新消息」出口，新消息静默追加 | `ChatWindow.tsx:1338-1341,2104-2135` | `followRef===false` 时渲染悬浮回底钮 + 未读计数 | 已修 |
| U1-6 | 平滑滚动与吸底互相干扰：长消息到达时中途 scroll 事件把 `followRef` 置 false，吸底断掉（首帧已知此坑只堵了首帧） | `styles.css:3247`、`ChatWindow.tsx:1289,1338-1341` | 程序化吸底统一临时切 `scroll-behavior:auto` 或加 programmaticScrollRef | 已修 |
| U1-7 | waiting tab 客户端过滤后为空时自动翻页卡死（守卫用 `shown.length` 而非 `data.length`），有下一页却显示空态 | `Sticker.tsx:406-412`、`helpers.ts:81` | 守卫改 `data.length === 0`，或 shown 空且 hasMore 时主动拉下一页 | 已修 |
| U1-8 | connected-first 是 per-page 排序，loadMore 追加后第二页的已连接会话排在第一页断开会话之下 | `session_query.rs:546`、`App.tsx:260-267` | 下沉 SQL `ORDER BY connected DESC, last_event_at …` | 已修（改为前端合并后按 connected 稳定分区——connected 是运行时信息，SQL 层拿不到） |
| U1-9 | 悬停缩略条零延迟展开（收回却有 ~360ms 容差）：屏幕边缘划过即弹出整块看板 | `CollapsedStrip.tsx:64`、`App.tsx:674-694` | 进入侧加 200-300ms hover-intent，进出对称 | 已修 |
| U1-10 | 展开态自动收回不看交互状态：搜索/编辑便签时鼠标离窗 ~360ms 即折叠，Sticker 卸载草稿全丢 | `App.tsx:672-696` | activeElement 为输入类 / 有编辑态 / 菜单开着时暂停收回 | 已修 |
| U1-11 | 缩略条不可拖（无 `data-tauri-drag-region`），README 承诺的「拖离恢复」做不到；引导也完全没教吸边 | `CollapsedStrip.tsx:59-72`、`zh.ts:704-713` | 缩略条加拖拽区（拖动即 `snap_restore`）；引导补「贴边收纳」一步 | 已修（缩略条可拖离恢复；引导「窗口与设置」步按平台追加吸边要点） |
| U1-12 | 归档无撤销：菜单相邻误点后卡片瞬间消失，恢复入口藏在设置深处；归档失败也零提示（重命名/便签失败有 focusNotice） | `Sticker.tsx:866-872`、`CardContextMenu.tsx:101-108` | 原位留 3s「已归档 · 撤销」行；补 archiveFailed 文案 | 已修 |
| U1-13 | 单次轮询失败整屏换成错误行（items 还在 state 里却被盖住），650ms 一发的 IPC 抖动就闪白屏 | `ChatWindow.tsx:1039-1041,2106` | failed 降级为顶部细横幅，连续 N 次才升整屏 | 已修 |
| U1-14 | 侧栏无搜索（看板有，`getLiveSessionsPage` 已支持 search 位），会话上百条只能滚动翻页找 | `ChatSidebar.tsx:366,731-742` | 接入 search 参数，或做 Ctrl+K 命令面板 | 已修（侧栏搜索框 + Ctrl/Cmd+F 聚焦，下沉后端 search 通道；2026-08-31 Ctrl+K 命令面板落地：QuickSwitcher 扩展为会话+命令双组混合过滤，8 条命令扁平数组派发既有动作，跨组平面索引键盘导航，速查表文案同步） |
| U1-15 | 新建面板启动选项零记忆（换 agent 即清空；恢复路径反而有持久化）；工作目录不预填 | `NewSessionPanel.tsx:70,162,339` vs `api.ts:244-251` | `{provider→选择}` 存 settings 回填；无 prefill 时默认 `recent[0]` | 已修 |
| U1-16 | 切回终端 tab 无条件下发 resize（后端同值也照发），TUI 每次全屏重绘 | `ManagedTerminal.tsx:666-679`、`pty.rs:1331-1350` | 前后端各加同值短路 | 已修（回归修补：以前「切回即回底」是全屏重绘的副作用，短路后上翻的视口停在原地——实拍「终端没有回到底部」。现切回可见与进程退出写提示后均显式 `scrollToBottom()`，测试钉住） |
| U1-17 | 首帧占位 ≠ 真实默认 ×4：menuMode(`context`→`button`)×2、sticker_style(`elevated`→`flat`)、opacity(94→100)，每次开窗闪一下；`About.tsx` 的 `??` 字面量盖掉了写对的 `SETTINGS_DEFAULTS` | `Sticker.tsx:181`、`ChatSidebar.tsx:476`、`About.tsx:398-400`、`state.ts:11` | 统一读 `SETTINGS_DEFAULTS`；加「与 Rust `Settings::default()` 逐字段一致」的单测 | 已修 |
| U1-18 | reduced-motion 白名单漏 4 个无限动画 + `scroll-behavior:smooth`；且减动效后 running/waiting/idle 状态点像素级同形 | `styles.css:4033-4047,3247,996-999` | 白名单改兜底通配 + 少数例外；减动效下用形状差异替代动画 | 已修（兜底通配）；减动效下的形状差异归 G-17 状态语言重设计 |
| U1-19 | `fmtResetIn` 硬编码 24 小时制 + 中文月日顺序直接搬给英文；全仓零 `Intl` | `AccountSection.tsx:64-90` | 换 `Intl.DateTimeFormat(lang, …)` / `Intl.RelativeTimeFormat` | 已修（fmtResetIn 走 Intl + 字典加 locale 字段）；fmtAgo 相对时间暂保留手拼 |
| U1-20 | 不透明度/字号滑杆逐像素触发全量写盘 + 读写 claude 配置 + 双事件广播（拖一次 = 75 轮完整流程） | `About.tsx:446-478`、`settings.rs:326-369` | 滑杆本地 state + onPointerUp 提交；或 set_settings 对代理字段未变短路 `apply_to_agent_configs` | 已修 |
| U1-21 | Esc 徽章画在审批卡上但默认焦点几乎总在 textarea（被白名单跳过），按了没反应 | `ChatWindow.tsx:1955,2225,2358` | 审批卡出现时移焦到「拒绝」，或 textarea 空时放行 Esc=拒绝 | 已修 |
| U1-22 | 审批/题面静默过期（300s/180s 只写在注释里），卡片凭空消失无终态 | `ChatWindow.tsx:2336,1231` | 徽章旁加剩余时间；过期切「已超时，已回落终端」终态 | 已修 |
| U1-23 | 终端视图下 broker 审批完全不可见（审批被 hook 劫走、TUI 里不会出现选择框），只剩标题栏小点 | `ChatWindow.tsx:2179 等 5 处 view==="chat" 门控` | 终端视图渲染常驻横幅「有 1 条待授权 · 去处理」 | 已修 |
| U1-24 | 通知被「正在看」抑制后也写进去重表，切走后永不补发 | `watch.rs:732-748` | suppressed 不写指纹，viewing 变 false 时补发 | 已修（随 U0-5 顺手） |
| U1-25 | 搜索框无 IME 合成守卫，拼音中间态经 300ms 防抖打到后端 LIKE（卡片编辑框有守卫，搜索框没有） | `Sticker.tsx:948` vs `helpers.ts:9` | compositionend 后再上报 | 已修 |
| U1-26 | 便签超 500 字后端静默截断，前端无 maxLength 无计数 | `session_command.rs:82`、`Sticker.tsx:75-82` | EditBox 加 maxLength=500 + 接近上限显示计数 | 已修 |
| U1-27 | 有更新时贴纸齿轮被整个改成更新入口，期间贴纸上没有打开设置的入口 | `Sticker.tsx:985-994` | 齿轮保持=设置，更新做独立红点徽章 | 已修 |
| U1-28 | 代码块无复制按钮/语法高亮/语言角标——AI 输出里最高频「要拿走」的内容只能框选 | `ChatMarkdown.tsx:73-96`、`styles.css:3884-3885` | pre 包复制按钮容器 + language 角标；高亮可按需引 shiki/prism | 已修（hljs 模块化高亮：core + 24 个按需语言，只在懒加载的 ChatWindow chunk（主 bundle 无感）；未知/超大/框线块降级纯文本） |

## 三、按交互面的其余发现（中影响，按面分组）

### 看板主窗

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| B-1 | 状态槽三种视觉形态（36px 徽标 / 16px 虚线环 / 9px 点），扫读无统一锚线 | `Sticker.tsx:619-635`、`styles.css:572-783` | 已修（error/在线/断开与 RunBadge 统一为同一直径圆形徽标：红环+「!」/绿环绿芯/灰虚线环，不再是大黑槽浮 9px 小点；RunBadge 本身的 pending 形态语言不变） |
| B-2 | `screen_state==="blocked"` 无 pill 无文案，waiting 黄与 pending 琥珀肉眼难分 | `Sticker.tsx:623,709` | 已修（blocked 也给「待操作」pill，不再只靠琥珀环色相） |
| B-3 | waiting tab「等最久优先」的倒排无任何 UI 提示，像列表坏了 | `query.rs:203`、`Sticker.tsx:714` | 已修（waiting tab 时间改「已等待 X」+ 警示色，倒排排序自解释） |
| B-4 | 切 tab/搜索不重置滚动位置；切 tab 中间态先渲染旧数据子集（waiting 的 ASC 到达后整列表翻转） | `App.tsx:351-379`、`Sticker.tsx:377-385` | 已修（滚动回顶此前已落地；切换期冻结旧列表灰化 + aria-busy 直到新数据就绪；清空搜索有缓存恢复时不灰化） |
| B-5 | `context` 菜单模式下星标/便签/重命名/归档零可见入口、零提示 | `Sticker.tsx:680-686,728` | 已修（菜单按钮常渲染，context 模式下 hover 卡片/:focus-visible 时极淡 ⋯ 浮现，button 模式不变） |
| B-6 | 每次 board-changed 整窗口重查（页越滚越大，500 条时每事件重查 500 行） | `App.tsx:292` | 已修（超过首页时只重查首页 + 尾部按 id 增量修补，connected 分区与游标不变量保持；归档失败回滚走整窗例外） |
| B-7 | 搜索不含便签内容（用户亲手写的记忆锚点搜不到；JOIN 已在 SELECT 里） | `query.rs:184-191` | 已修（LIKE 条件已含 `sn.note`，store 层测试 `live_sessions_search_matches_note_and_recent_texts` 钉住） |
| B-8 | 便签块无行数限制，500 字便签撑满窗口且与 82px 估高差 3.6 倍致滚动抽搐 | `styles.css:873-884`、`Sticker.tsx:392` | 已修（3 行 clamp + 点击展开再点击编辑；estimateSize 分档：带便签 144 / 普通 82） |
| B-9 | 编辑中滚动列表虚拟化卸载 EditBox → 草稿静默丢失，滚回显示原文 | `Sticker.tsx:71,393` | 已修（草稿提升到 Sticker 层 `editDraftsRef` 按 sessionId 存，ref 不触发整板重渲染；提交/取消清草稿） |
| B-10 | 卡片菜单键盘完全不可达（无 autofocus/方向键/焦点归还，DOM 挂在滚动区外 Tab 进不去）；项目自己的 `useMenuPopup` 有完整实现 | `CardContextMenu.tsx:57-131` vs `menu.tsx:146-171` | 已修（挂载聚焦首项 + ↑↓/Home/End/Enter + 关闭归还焦点，测试钉住） |
| B-11 | 卡片 `role="button"` 内嵌 4 个可聚焦控件，100 张卡 = 200-400 次 Tab | `Sticker.tsx:650-653,715,729,794` | 已修（卡内控件 tabIndex=-1，整卡单 Tab 停靠点；卡片 ←/→ roving 搬 DOM 焦点，编辑态不抢键） |
| B-12 | tablist 无方向键/`aria-controls`/tabpanel；无任何窗口级快捷键（Ctrl+F 搜索、Esc 三级回退） | `Sticker.tsx:548-576` | 已修（Ctrl+F + Esc 回退链；tablist ←/→/Home/End + roving tabindex + aria-controls/tabpanel 接线） |
| B-13 | focus toast 无 Esc/点外关闭，带动作类型不自动消失且盖掉小窗列表下半部 | `Sticker.tsx:898-935`、`styles.css:384-393` | 已修（Esc 关闭 toast） |
| B-14 | 自绘滚动条 4px 且仅悬停贴纸才显形，无点击轨道跳转 | `styles.css:478-490` | 已修（热区扩 12px 视觉不变，4px 视觉改 `::before` 绘制，溢出时 0.35 低透明常显；点击轨道跳转未做） |
| B-15 | loadMore 的三点 loader 落在底部淡出遮罩里且不计入滚动高度 | `Sticker.tsx:839-845`、`styles.css:357-367` | 已修（loader 移出虚拟内层容器，作为 `.stk-scroll` 正常流子元素计入滚动高度） |
| B-16 | tab 滑块宽度硬编码 1/3 与 TAB_KEYS 靠注释同步；星标后卡片瞬移无 FLIP 动画 | `styles.css:265-276`、`Sticker.tsx:863` | 已修（滑块按选中按钮实测定位 + ResizeObserver；星标 FLIP：重排前量可见卡位置、useLayoutEffect 补偿 180ms，尊重 reduced-motion） |

### 对话窗

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| C-1 | 「加载更早」一次性全量重读且只能点一次 | `chat.rs:97`、`ChatWindow.tsx:1314-1319` | 已修（offset 增量前向分页：`get_chat_history` 加 `before` 参数 + `read_chat_window` 只读未展示段，可反复点直到文件头；契约向后兼容） |
| C-2 | 工具调用折叠组收起态看不出「还有工具在跑」；工具输出 pre 无 max-height 撑爆几十屏 | `Transcript.tsx:90-96`、`styles.css:3322` | 已修（组摘要按无回执数显示跳动点；.chat-tool pre 限高 40vh + overscroll contain） |
| C-3 | 消息无任何时间信息（timestamp 字段有传递但未渲染） | `Message.tsx:212-317` | 已修（按天分隔条 `.chat-day-sep`：今天/昨天/本地化日期；气泡悬停提示精确时间，messageMeta 测试钉住） |
| C-4 | 斜杠补全只前缀匹配、带参数即消失；`@` 附件提及能力已有但无补全菜单；补全菜单 ARIA 断裂 | `ChatWindow.tsx:863-865,178-192,2391-2449` | 已修（子串匹配前缀优先 + 参数提示行 `.chat-slash-arg-hint`；combobox/listbox/option + aria-activedescendant 接线；`@` 文件补全此前已落地） |
| C-5 | 支持粘贴图片但不支持拖拽放入 | `ChatWindow.tsx:2451-2457`（无 onDrop） | 已修（Tauri `onDragDropEvent` 拖入：对话页进附件列表、终端页写加引号路径进 PTY，拖入时渲染 `.chat-drop-overlay` 提示） |
| C-6 | Ctrl+Enter 打断并发送的提示挂点被删成孤儿文案，用户发现不了 | `ChatWindow.tsx:2680-2684`、`zh.ts:294` | 已修（tip 挂到发送圆钮 data-tip：运行中且有草稿时显示；stopMode 仍显示打断提示） |
| C-7 | 草稿只在内存 Map，关窗即丢（附件反而落盘了） | `ChatWindow.tsx:538,949-983` | 已修（按 sessionId 落 localStorage：400ms 防抖、LRU 保 20 条、旧格式幂等迁移） |
| C-8 | 一敲键盘「接管」按钮随 sendError 一起消失，needsTakeover 还悬着 | `ChatWindow.tsx:2450,2702-2714` | 已修（横幅显隐挂 `sendError \|\| needsTakeover`，文本清空时回落接管文案，按钮只挂 needsTakeover） |
| C-9 | 审批卡插入文档流下推 composer，「允许一次」深色主按钮与拒绝仅隔 8px，`rm -rf` 与 `ls` 同视觉权重 | `styles.css:3343-3399`、`ChatWindow.tsx:2357-2360` | 已修（180ms 高度渐开动画防跳变；危险命令启发式 `isRiskyCommand` → 允许钮红色警示；拒绝/允许间距 8→16px。2026-08-31 彻底零位移落地：六张卡收进 `.chat-approval-overlay`——零高度 flex 子项向上溢出锚定 composer 顶缘，不占文档流；卡片限高 min(70vh,560px) + copy 内部滚动，覆盖消息区处铺 72px 渐变过渡；ApprovalCard 加 returnFocusTo（layout cleanup 判焦点、宏任务双重守卫归还）；终端横幅 U1-23 不受影响） |
| C-10 | 审批提交失败空 catch 完全静默；长命令详情嵌套两层滚动无展开/复制 | `ChatWindow.tsx:1861-1863`、`styles.css:3346,3378` | 已修（失败按 requestId 写卡内错误行 role=alert；`ApprovalCommandDetail` 提供展开全部/复制命令） |
| C-11 | 多问题/多选题渲染成 tab 却不能卡内作答，只给「去终端」 | `ChatWindow.tsx:1910-1911,2314` | 已修（单问题多选此前已落地；本轮经调查聚焦题可从屏文题面反查（matchFocusedQuestion，空白归一子串 + 歧义放弃），queuedAnswers 升级按问题 keyed，只落聚焦题答案、认不出一字不写） |
| C-12 | 切会话整屏清空 + 全屏加载文案三段跳；外部切换时侧栏不 scrollIntoView 当前项 | `ChatWindow.tsx:972-976,2105`、`ChatSidebar.tsx:633` | 已修（骨架屏气泡条 + loading 超 150ms 才显示；activeId 变化 scrollIntoView block:nearest） |
| C-13 | 归档后自动跳转逻辑两份（侧栏用本地 ordered，ChatWindow 另发查询），目标不可预期 | `ChatSidebar.tsx:527-530` vs `ChatWindow.tsx:651-655` | 已修（抽 `useSessionActions`：`pickNextAfterArchive` 统一取序，两窗口归档/批量归档/撤销同入口） |
| C-14 | 流式输出 650ms 一批的蹦字观感；切终端再切回强制丢阅读位置（DOM 实为 hidden 未卸载） | `ChatWindow.tsx:1047-1051,1269-1287` | 已修（pty-output 驱动此前已落地；本轮真·transcript watch push：后端按会话监听 transcript 文件直发 `chat-transcript` 事件，兜底轮询桌面端降至 2s，远程保持 650ms） |
| C-15 | 任何发送错误都把占位符改成「尚未接管」（与真实原因无关）；会话结束时 composer 整块卸载藏起草稿 | `ChatWindow.tsx:2443-2449,2374-2390` | 已修（占位符只随 needsTakeover，普通发送错误只走错误条；gate/被接替态降为上横幅 + composer 禁用不卸载，草稿原地可见。遗留：gate 态下附件/模型按钮未一并禁用） |
| C-16 | 超长粘贴固定 250ms 后回车，TUI 可能没消化完；`sessionLaunchSelections` 无 stale 守卫会把旧会话启动档落到新会话 | `ChatWindow.tsx:405-414,727-730` | 已修（submitGapMs 按长度动态间隔 250ms+50ms/KB 封顶 2s；stale 守卫此前已由 cancelled 标志补齐） |
| C-17 | 快捷键体系整体缺位：无切会话/收展侧栏/切视图/新建/查找；侧栏平铺 tabindex 上百次 Tab | `ChatWindow.tsx:1948-1962`、`ChatSidebar.tsx:629-647` | 已修（Ctrl+K/B/1/2/N/F + ? 速查表全部落地；侧栏 roving tabindex + ↑↓/Home/End 导航） |
| C-18 | 断线语言缺失：分不出「agent 进程没了」和「IPC 通道断了」 | `zh.ts:153-160`、`ChatWindow.tsx:565` | 已修（轮询连续失败 ≥3 次升级「同步中断」横幅 role=alert 错误色，瞬时失败仍琥珀通用文案；与 tone 解耦） |
| C-19 | 子任务展开后滚动，吸顶标题与视口顶之间有一条缝，内容从缝里透出 | `styles.css:3802`（`.chat-scroll` padding-top）+ `styles.css:4460`（sticky summary） | 已修（Chromium 里 sticky top:0 贴滚动容器内容盒上沿，padding-top 变成缝隙，Playwright 实测 24px；顶部留白改文档流内 `.chat-scroll::before` 占位，吸顶贴顶=0；`.chat-sync-warn` 同道受益） |
| C-20 | 运行状态条（SnakeSpinner + currentActivity）非 Bash 工具只显示工具名，看不出在做什么 | `dispatch.rs:114` 兜底分支 | 已修（hook.rs 新增 `activity_detail()`：pattern→file_path→path→query→url→subject→description 逐键试，兼容 claude/kimi 字段名，截 120 字；有细节写「Grep: 个子任务」，无细节维持裸名；dispatch_test 钉死两种形态） |

### 托管终端 / 跳转恢复 / 新建会话

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| T-1 | 初始化遮罩 25s 兜底后静默撤下留无解释黑屏，无重试/结束入口 | `ManagedTerminal.tsx:66-67,357-360` | 已修（超时挂提示条 + 结束会话按钮，真画面到达自动收横幅自愈） |
| T-2 | 隐藏期离屏 xterm 固定 1000×700 与 PTY 网格不一致，二次折行致屏幕识别漏检 | `styles.css:3938`、`ManagedTerminal.tsx:418-430` | 已修（隐藏态用快照携带的 PTY cols/rows 钉齐网格，可见期 resyncGridIfDrifted 反向拉齐） |
| T-3 | 无 WebGL/Canvas 渲染器、无 Unicode11 宽表（emoji/框线错位半格）、无搜索 addon、scrollback 5000 硬编码、alt-screen 下滚轮翻不了历史 | `ManagedTerminal.tsx:204-224`、`package.json:23-25` | 已修（WebGL 失败回退 canvas、unicode-graphemes、addon-search 此前已落地；本轮 scrollback 进设置 terminal_scrollback 500–50000 热应用） |
| T-4 | 链接需 Ctrl+点击但无提示；文件路径（最高频可点内容）一律不可点 | `ManagedTerminal.tsx:200-203`、`settings.rs:556-562` | 已修（悬停链接右下角提示「Ctrl+点击打开链接」；registerLinkProvider 识别文件路径，Ctrl+点击走 reveal_path_in_file_manager） |
| T-5 | 操作条 hover 才显形，无 `:focus-within` 无 `hover:none` 兜底；PTY 假死红条只给「关闭」不给「结束并恢复」 | `styles.css:4026-4029`、`pty.rs:1318-1328` | 已修（操作条兜底此前已落地；假死横幅本轮加「结束并恢复」按钮，stop→start 一步走完） |
| T-6 | attach 成功零正反馈（窗口在别的虚拟桌面时=「点了没反应」）；`permission_denied` 无一键跳系统设置 | `terminal.rs:1900-1904`、`Sticker.tsx:913-923` | 已修（成功弹「已在 X 打开」toast，落点按后端同口径计算；permission_denied 改常驻 toast +「打开系统设置」直达按钮） |
| T-7 | 恢复全程零反馈（后端乐观复活已 emit board-changed，前端没用它做即时态）；重复恢复判重静默 Ok 但 reveal 再跑一遍又开一个镜像标签 | `Sticker.tsx:316-322`、`pty.rs:892-921`、`terminal.rs:1953-1964` | 已修（卡片 is-opening 置灰 + toast 此前已落地；判重本轮冒泡：start 返回 bool，命中时聚焦已有视图不再 reveal，前端提示「已在运行，已切到所在窗口」） |
| T-8 | 秒退探测只盖前 1 秒，banner 后报错的 CLI 探测不到；`waitForTerminalReady` 45s 无进度无取消 | `terminal.rs:2033-2049`、`ChatWindow.tsx:424-458` | 已修（快照带 exit_tail 覆盖整个 45s 等待窗、失败带 tail 提示；等待横幅显示已等秒数 +「终端」跳转钮。5s 观察者经核实无增量价值未加） |
| T-9 | 对话页恢复/接管硬编码 100×30 起 PTY，首屏画完再 resize 重排一遍 | `ChatWindow.tsx:1435,1478` | 已修（gridRef 读当前 xterm cols/rows 作初始网格，未挂载时才退占位值） |
| T-10 | 高风险启动档（bypassPermissions）与普通档同视觉零警示；契约无 description/risk 字段；未知 id 直接裸露 | `NewSessionPanel.tsx:336`、`LaunchOption.ts` | 已修（契约 `LaunchChoice.risk` 落地，四家 yolo/bypass 档标 true 并重生成 TS；下拉警示色 + 风险副标题） |
| T-11 | relay 的 `--model` 静默压过用户在面板选的模型 | `terminal.rs:1704-1716` | 已修（relay 启用时 model 档置灰 + 行下注明「由中转固定为 X」，取不到固定值时宁可不置灰） |
| T-12 | 启动后面板一闪而灭，无 toast 无占位卡（组件文档承诺的 emit 不存在，codex 到首 turn 才出卡） | `NewSessionPanel.tsx:45,180-181` | 已修（pending PTY 合成负 id 占位卡合入看板查询，按 provider/cwd 对账撤卡；占位卡取代 session-starting toast；只进 all/running tab，缩略条未纳入） |
| T-13 | 临时 id→真实 id 重绑换 key 整只重挂终端，首屏清空重画 | `ChatWindow.tsx:1053-1062,2284` | 已修（删除 key 重挂，xterm 实例复用；binding 权威确认同一 PTY 时只换 viewer 注册，画面不闪不 reset） |
| T-14 | 双视图同写同一 PTY：resize 无仲裁（两窗尺寸不同时 TUI 反复重排）、输入无互斥无提示 | `pty.rs:2046-2051,1307-1329` | 已修（失焦视图所有 resize 通道静默、重新聚焦仅真漂移才夺回主控；外部视图在线时对话页常驻提示「两边同时输入会交错」，pty-external-viewers 事件实时推送） |
| T-15 | 外部会话角标回落 DB status 但与托管会话同呈现，无「不新鲜」区分；「运行中→空闲」四层节流叠加最坏 1.7s；fallback 规则命中时角标依然「很自信」 | `pty.rs:1636-1651`、`detect.rs:371-413`、`useBoardRefresh.ts:6` | 已修（弱化/fallback 中性点此前已落地；本轮降级转变（Working→Idle/Blocked）绕过合流直发 `board-urgent`，前端跳过 400ms 节流立即刷新，落地延迟 ~1.7s→~1.0s） |
| T-16 | 强制收尾 emit pty-exit 但进程可能仍在（zombie），UI 显示「已结束」无区分 | `pty.rs:445-446,1073-1085` | 已修（PtyExitEvent 加 forced 字段，ForceFinalize 传 true；退出封面按 forced 显示「已强制结束——进程可能仍在后台残留」） |
| T-17 | kimi 屏幕规则与当前版 TUI 失配：`⠴ <Agent> Running (…)` 与 `🌘 · Tip:` 无规则覆盖，整块活跃工作屏判 idle → 误报「待交互」（只有持有该会话 PTY 的实例会踩） | `kimi/screen.rs:66-93` | 已修（补 `agent_running_status_working`/`moon_tip_status_working` 两条规则 + detect 实拍回归测试） |

### 设置 / 账号 / 引导 / 更新

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| S-1 | 30 项设置零搜索；「Agent」分区承载账号/配额/登录/安装/中转五件事（key 叫 account 显示叫 Agent）；「在贴纸显示配额」埋在已登录账号卡深处 | `About.tsx:26,604-630`、`AccountSection.tsx:624-651` | 已修（分区改名此前已落地；本轮设置窗加搜索：行/卡/分组/分区四级过滤 + 异步内容 MutationObserver 兜底，深埋的配额开关随搜索可达） |
| S-2 | 切分区 `key={sec}` 整树重挂 + 重拉数据（含联网配额查询）；设置窗 620×460 不可缩放，长内容嵌套滚动陷阱 | `About.tsx:643,188-194`、`window.rs:113-115` | 已修（访问过的分区保持挂载 + hidden 隐藏，切回零重挂零重拉；设置窗放开纵向缩放，宽度仍钉 620） |
| S-3 | 通用/会话/外观三分区保存失败完全静默（开关自己弹回去），只有网络分区显示错误 | `About.tsx:129-476` 各 patch 调用 | 已修（useSettingsState 暴露 lastError；通用/会话/外观/账号四分区渲染错误行，网络分区原有） |
| S-4 | 设置窗不订阅 settings-changed（现成的 useSettingsEffect 没用），与 Onboarding 同开时整对象写回互相覆盖 | `state.ts:44-66` | 已修（state.ts 订阅 settings-changed） |
| S-5 | 调不透明度/字号时设置窗自己纹丝不动（密度只在贴纸窗生效），等于闭眼调 | `appearance.ts:78-80`、`About.tsx:446-478` | 已修（外观分区顶部迷你贴纸预览卡，实时反映不透明度/界面缩放/贴纸色/主题/贴纸风格） |
| S-6 | 配额无「更新于 X 分钟前」，设置页不自动刷新（贴纸 5 分钟刷）；对未登录 provider 也发配额查询 | `AccountSection.tsx:632-646,1082` | 已修（设置页 5 分钟自动刷新 + 「更新于 HH:MM」；挂载刷新过滤对齐贴纸——未登录/中转不再发请求） |
| S-7 | 登录 pending 态零指引（不说在哪个终端开了窗、等多久）；「取消等待」英文译成裸 Cancel 生歧义 | `AccountSection.tsx:464-466`、`en.ts:569` | 已修（pending 显示「已在 {终端} 打开登录窗口…最长等待 5 分钟」，终端名取 resume_terminal；en 改 Stop waiting） |
| S-8 | 安装是不可中断黑盒（无进度/耗时/取消）；失败日志路径纯文本不可点；「修复连接」按钮凭空出现零解释；修复失败指向不存在的「运行终端」 | `AccountSection.tsx:436-452,568-573,495-505`、`zh.ts:118` | 已修（耗时显示/日志可点此前已落地；本轮补取消安装（注册表+进程树强杀）、修复按钮 data-tip 说明、修复失败指向订正为「重试/重新安装」） |
| S-9 | 切账号不说明「仅对新会话生效」（后端注释明确，UI 无一字）；后端错误硬编码中文直出（英文界面显示中文） | `profile/mod.rs:337-339,312-443`、`AccountSection.tsx:999` | 已修（说明行此前已落地；后端错误改 `profile/<code>: <detail>` 结构化 reason 码 + errors.ts 双语映射，旧前端兼容的用户向 sentinel 保持原样） |
| S-10 | `listAgents()` 失败静默吞错 → Agent 分区整页空白，分不清空态和故障 | `AccountSection.tsx:1030-1032,1093-1094` | 已修（检测中/失败重试/名单空三态区分，不再整页空白） |
| S-11 | 引导 6 步无一步讲装 agent/登录（走完看板还是空的）；宣称有「已归档」tab（实际没有）；reopen 提示只在最后一步（跳过者永远看不到） | `Onboarding.tsx:345-352,402`、`zh.ts:686` | 已修（新增「连接你的 AI CLI」一步指向设置→账号与用量；reopen 提示在跳过按钮旁常驻。「已归档」经核实确实存在于卡片 ⋯ 菜单，宣称属实无需订正） |
| S-12 | hooks 静默改写 `~/.claude/settings.json` 全程零告知；首启自动导入 7 天历史会话零说明 | `lib.rs:1151-1152`、`watch.rs:793-820` | 已修（引导加「Meowo 如何读到进度」步明写备份/移除方式；导入完成一次性 toast，拉取式规避监听竞态） |
| S-13 | 打开「关于」10s 空窗（delayMs=10000），checking/error 态不渲染（文案已有但零引用）；更新窗无「稍后/跳过此版本/更新日志」，下载不可取消 | `useUpdate.ts:11`、`About.tsx:519`、`Updater.tsx:97-133` | 已修（checking/error 态、unknown 初值、「稍后」、更新日志链接此前已落地；下载取消本轮补上——updater 无取消 API，watch channel + select! 赛跑实现） |
| S-14 | 确认框主按钮通用「确定」不说后果；可逆登出与不可逆删除同用 danger 红（严重度拉平）；confirm 打不开静默当取消 | `ConfirmWindow.tsx:116-119`、`AccountSection.tsx:358-793`、`confirm.tsx:16` | 已修（confirmLabel 全覆盖：删除/合并/结束会话/结束并重开/接管/切档/重启更新各自说后果；RemoteAccessCard 重置密钥可逆降为非 danger；catch 原生错误框兜底） |
| S-15 | 12 个死文案键（含 5.2 需要的 about.checking 和已有样式的 proxy `*Why` 解释）；`usageUnavailable` 在已登录分支说「请确认已登录」自相矛盾 | `zh.ts:513-658` 各处 | 已修（proxy `*Why` 接入 NetworkSection 覆盖面行；usageUnavailable 改为指向「刷新」重试；死键两本字典同步清掉 33 个，tsc 对齐背书） |

### 窗口管理与通知

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| W-1 | 吸附阈值 20 物理像素（150% 缩放下等效 13 逻辑像素），高 DPI 手感随机；条厚度却按逻辑值乘 scale，两处口径不一 | `snap.rs:5,150-152` | 已修（阈值按 scale_factor 换算，与 STRIP_W_LOGICAL 同口径） |
| W-2 | 吸附预览只有窗口自身 4px 发光条，无落点预览 | `App.tsx:705`、`styles.css:1003-1015` | 已修（候选边出现时画 `.snap-ghost` 虚线幽灵条：28px 厚度与真实缩略条一致、主轴按 stripExtent 计算。2026-08-31 原生预览窗落地：label=snap-preview 的透明置顶免焦点窗，几何由 snap.rs 纯函数 `preview_strip_rect` 与 snap_collapse 同公式计算；Moved 处理器每帧驱动显隐/跟随，左键按下门控防程序化移动误闪，READY 握手防首帧白框，事件泵内同步建窗会卡死故走子线程懒建；建窗失败熔断回退 ghost；松手/失焦/吸附落地四处幂等 hide。Windows 实拍验证，截图在 target-verify/tmp/） |
| W-3 | 展开态点一下拖拽条（不移动）即意外折叠（陈旧 lastEdgeRef） | `App.tsx:557-558,462-484` | 已修（松手时位移 < 4px 视为纯点击，不做吸附/还原判定） |
| W-4 | 折叠态启动闪「细条内容装在 360×440 大框」；展开过渡期看板在 28px 宽窗口内布局抖动 | `App.tsx:157-162,663-667`、`useShowWhenReady.ts:20-22` | 已修（折叠启动闪帧此前已消除；本轮展开过渡期渲染 `.snap-expanding` 纯色占位，snap_expand/snap_restore 落地后才挂载看板，悬停偷看与找回两条路径都覆盖） |
| W-5 | 缩略条主轴无上限（60 会话超 1080p 工作区，溢出被裁无提示）；DPI/显示器变化不重算 | `App.tsx:55-57`、`snap.rs:251`、`CollapsedStrip.tsx:47-54` | 已修（按工作区主轴换算容量、溢出折「+N」徽章；onScaleChanged 重跑 snap_collapse；后端 clamp_strip_extent 兜底） |
| W-6 | pin 状态折叠态不可见不可操作；「找回贴纸」强行永久改写 pin 偏好 | `Sticker.tsx:995-1005`、`App.tsx:415` | 已修（找回改临时置顶——`recall_center` 带 pinned 参数，不再改写 localStorage；缩略条 pin 标记加过又被实拍否掉（28px 细条占一格且噪音），已撤） |
| W-7 | 贴纸首显走普通 `show()` 可能抢焦点（`show_window_no_activate` 已有只给对话窗用） | `useShowWhenReady.ts:25`、`window.rs:34-38,437-449` | 已修（新增 `show_sticker` 命令走 no-activate 路径；show_after_grace 的 focus=false 分支同步改） |
| W-8 | 无点击穿透，opacity 可低至 25%：几乎看不见的置顶窗口 100% 吃鼠标 | `settings.rs:331`（无 set_ignore_cursor_events） | 已修（设置项 + set_ignore_cursor_events 热生效；按住 Alt 临时恢复交互（50ms 轮询 GetAsyncKeyState）；仅 Windows 实装） |
| W-9 | 拖拽期间每个 Moved 全量枚举显示器 ×2 处并 emit | `lib.rs:1040-1063,775-800` | 已修（显示器工作区按世代缓存，WM_DISPLAYCHANGE/SETTINGCHANGE 才失效；snap-changed 只在吸附边变化时 emit） |
| W-10 | Windows 托盘图标零状态表达（只有 tooltip），macOS 有彩色徽章——可见性差一个数量级；托盘计数把出错混进「待交互」 | `window.rs:634-641`、`watch.rs:611-619` | 不采纳（圆点/数字徽章两轮迭代均被实拍否掉，待交互角标已整体撤掉：状态表达维持 tooltip 摘要，dev 角标按用户要求保留原样；error 计数拆分若要做可在 tooltip 文案里体现） |
| W-11 | 两平台托盘菜单构成与左键语义不一致（Win 无「打开对话窗」项，macOS 无「找回贴纸」）；Windows 无显示/隐藏开合 | `window.rs:596-664`、`menubar.rs:240-249` | 已修（菜单两平台同项同序；单击=开合主界面、**双击=打开对话窗**——初版单击语义直接替换旧「开对话窗」，双击变两次开合被实拍打回，加 300ms 世代消抖兼容两种习惯） |
| W-12 | macOS 面板任何失焦即收起，无法常驻（pin 按钮也不渲染） | `panel.rs:65-72`、`Sticker.tsx:995` | 已修（复用 pin 按钮 + PIN_KEY：面板 pin 语义=失焦不自动收起，经 set_panel_keep_open 推送；⚠️ panel.rs 为 cfg(macos)，Windows 主机未编译验证靠 CI） |
| W-13 | waiting/pending 指纹掺 `last_event_at` 会重复轰炸（blocked 已论证过并排除，结论只用了 1/3）；审批 toast 用默认 5s 时长无按钮 | `watch.rs:277-301,391-416` | 已修（指纹去时间戳此前已落地；本轮 Pending/Blocked 用 Duration::Long +「打开会话」按钮，winrt-notification 按钮与点正文汇入同一 on_activated） |
| W-14 | 通知点击跳转失败静默丢弃返回值（focus_session_terminal 明明有分类结果）；标题不带项目名，多会话时通知中心一排同名 | `watch.rs:411-413,637-739` | 已修（标题带项目名；点击定位失败回退打开对话窗） |
| W-15 | 关「通知」后任务栏仍闪（attention_flash 独立门控但文案不说明）；macOS 通知串行阻塞投递 + 废弃 API | `watch.rs:631-633,766-769`、`macos/notify.rs:30-79` | 已修（文案说明此前已落地；本轮换 UNUserNotificationCenter（objc2 系依赖现成），异步非阻塞投递 + 授权/前台 banner/点击路由；⚠️ cfg(macos) 本机未编译验证靠 CI） |
| W-16 | 双屏内侧边也可吸附（鼠标必经之路 + 零延迟展开=必然误触）；非矩形排布包围盒留「死区」窗口可拖进去消失 | `lib.rs:1073-1081,748-799`、`snap.rs:38-53` | 已修（贴边检测改并集包围盒——只吸虚拟桌面外侧边；钳位改相交面积最大的显示器，死区进不去） |
| W-17 | 位置/尺寸/吸附边/置顶分散四套存储无原子性（localStorage 清空即半吊子状态）；折叠态 min_size 放开是「尺寸毒化」根因 | `lib.rs:897-905`、`App.tsx:24-95`、`snap.rs:288-291` | 已修（四套存储收敛 settings.json 单一日志源 + localStorage 旧键一次性迁移；折叠态改禁 resizable，用户拖角缩吸附窗的通路物理关闭；window-state 插件对 main 加 denylist） |
| W-18 | Linux 分支吸边「看起来支持实则坏掉」（两个原语恒返回假值） | `snap.rs:203-224` | 已修（Moved 处理器与 snap 常量收窄为 cfg(windows)，Linux 吸边流程整体休眠；macOS 空分支不变） |

### 全局基础设施

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| G-1 | 点外关闭两套策略：卡片菜单无滚动监听（列表滚走菜单钉在原位指向另一张卡），dd 菜单无 blur 关闭 | `menu.tsx:105-131` vs `CardContextMenu.tsx:57-79` | 已修（抽 `useDismissable` 统一承接点外/Esc/层栈/滚动/blur/resize，menu.tsx 与 CardContextMenu 两处接入，7 个单测钉住） |
| G-2 | 两窗口重命名失焦语义相反：贴纸失焦=丢弃、侧栏失焦=保存（backlog P2-6 只评了代码复用，没评用户可感知语义） | `Sticker.tsx:58-102,654-662` vs `ChatSidebar.tsx:51-68` | 已修（两窗口统一失焦=保存、Esc=取消，与 Finder/Explorer 惯例一致） |
| G-3 | 确认/弹窗三种壳（原生小窗/页内 modal/行内编辑），页内 modal 无焦点陷阱无 inert（aria-modal 承诺落空） | `ChatWindow.tsx:308-330`、`ConfirmWindow.tsx` | 已修（抽 `AppModal`：焦点陷阱/初始焦点/焦点归还/兄弟节点 inert，RenameModal/QuickSwitcher/ShortcutSheet 三个接入，8 个单测） |
| G-4 | `card_menu_mode=button` 贴纸常显、侧栏 hover 才显——同一设置两种表现 | `styles.css:825-843` vs `:3234-3239` | 已修（侧栏 ⋯ 按钮去掉 hover 门控，button 模式两处都常显） |
| G-5 | 原生 title 残留 17 处与 Tooltip 混用（不止 backlog 认可的 dd-menu 内） | `ChatWindow.tsx:2069`、`Message.tsx:161` 等 | 已修（Message/ChatMarkdown/GitDiffView/NewSessionPanel/RelayAccess/AccountSection 逐处换 data-tip；dd-menu 内 5 处按 backlog 认可保留并补注释） |
| G-6 | Esc 层级三种让路约定（preventDefault / 捕获 stopPropagation / tagName 白名单）+ Tooltip 第四种；独立窗口 Esc 关闭只实现 1/6 | `menu.tsx:117`、`Message.tsx:76`、`ChatWindow.tsx:1949`、`Tooltip.tsx:74` | 已修（escLayers 层栈落地：菜单/右键菜单/弹层/灯箱/切换器/补全全部注册，窗口级「Esc=拒绝审批」按栈让位；各层自己的截停约定保留为双保险） |
| G-7 | focus ring 仅 1 处自定义，`.slider` 等 outline:none 无替代（WCAG 2.4.7 失败） | `styles.css:2002-2011,2269` | 已修（--dur token 同批；data-im=kbd 下全局 :focus-visible 兜底环 + .slider 放开 outline） |
| G-8 | 动效零 token：13 个时长值、3 个缓动散落，同类元素有的渐变有的瞬变 | `styles.css` 全文 | 已修（--dur-fast/base/slow 三档 token，45 处 transition 收敛；0.4s 长淡入与具名动画保留） |
| G-9 | 用量读数屏文字四种主题组合全部 <4.5:1（最低 3.28）——绕过已校准 token 写死 alpha；7 处文本再叠 opacity 击穿基线（最低 2.81） | `styles.css:441-449,1728,3731` 等 | 已修（用量读数回 --cc-text-dim 保留雕刻 text-shadow；6 处文本 opacity 叠加删除） |
| G-10 | 终端页强制深色的变量覆盖漏整个状态色族（浅色用户切终端页 err/warn 只有 3.3-4:1）；flat 主题逐条硬抄已漏 `.run-mask`（flat+浅色下运行卡片是纯黑块） | `styles.css:3785-3809,2287-2363,597-604` | 已修（--ind-face token：.stk-ind 与 .run-mask 同底色，flat+浅色不再有纯黑块；浅面徽标配色整组压暗一档、文字改白。近似色留视觉验证轮微调） |
| G-11 | 「密度」实为字号缩放且只作用于贴纸窗（设置文案与 token 注释两张皮）；`--sp-*`/`--fs-*` 全定值，46 处 calc 之外的间距不缩放致版式失衡；`.stk-ind`/`.cstrip-*`/`.tip` 等完全不吃密度 | `appearance.ts:78-80`、`styles.css:26,85-105` | 已修（--sp/--fs 阶梯整梯乘 --cc-ui，非贴纸窗恒 1 零回归；.stk-ind/.run-core/.ring-stop/.sdot/.needs-error/.ctx-menu 一并参数化；设置项改名「界面缩放」。2026-08-31：--cc-ui 已下发对话窗（bootAppearance scale 门控含 chat 窗，同一热生效通道；对话窗 calc 外定值逐面归阶梯或注释封口，xterm 字号走 terminal_font_size 不吃 --cc-ui；.chat-compose-hint 补 ellipsis 兜底）。设置/引导等定尺寸窗口刻意保持恒 1；.cstrip-* 刻意不缩放——细条厚度固定，点放大会溢出） |
| G-12 | 英文约 2.2× 宽无省略保护：`.stab`/`.seg-btn` nowrap 无 overflow 处理，窄窗+英文+大密度直接溢出 | `styles.css:293-309,1918-1937` | 已修（tab 文字包 `.stab-label` 吃 ellipsis；`.seg` 允许 shrink；`.seg-btn` 补 min-width:0 + ellipsis） |
| G-13 | 零 Intl：24 小时制硬编码、英文月日顺序错、相对时间手拼 | `AccountSection.tsx:64-90`、`helpers.ts:14-21` | 已修（fmtAgo 走 Intl.RelativeTimeFormat 随界面语言；fmtWaited「已等待 X」时长语义刻意保留字典） |
| G-14 | 「本周」实为「近 7 天」，与日历应用语义不同 | `ChatSidebar.tsx:95-110` | 已修（文案已是「近 7 天」/ Last 7 days） |
| G-15 | 悬停是若干功能唯一入口且无 hover:none 兜底（终端操作条、自绘滚动条）；Tooltip 无触摸/长按路径而它承载完整路径、账号全名等唯一信息 | `styles.css:4026-4029,478-490`、`Tooltip.tsx:77-79` | 已修（终端操作条 focus-within/hover:none + TooltipLayer 触屏长按 500ms 显示、抬手保留 1.2s） |
| G-16 | 每个会话状态点都是 `role="status"` live region（N 条会话 = N 个播报源，状态秒级跳变播报风暴）；审批卡 `role="alert"` 包裹交互按钮打断朗读且焦点不入 | `ChatSidebar.tsx:663,794` 等、`ApprovalCard.tsx:22` | 已修（状态点改 role=img，全窗唯一视觉隐藏汇总播报；审批卡改 alertdialog + 挂载移焦动作区） |
| G-17 | 状态三色灰度值几乎相同（159/160/169），waiting↔pending 对比 1.13:1，reduced-motion 下动效通道也没了——无第三通道 | `styles.css:566-591,3207-3212` | 已修（复核结论：8px 点阵的空心环/方块形状通道曾试曾撤——小尺寸渲染毛病多且破坏格式塔；最终通道=颜色+动效+pill 文字+卡槽环形徽标（B-1：红环+!/绿环绿芯/灰虚线环，24px 下环形成立） |
| G-18 | `.stk-utab` 未选中 opacity 0.42 实测对比 1.90:1，看不出可点 | `styles.css:466-472` | 已修（0.42 → 0.6） |
| G-19 | 设置分区不记忆（每次开窗回 general，最常用的外观永远多点一次） | `About.tsx:594` | 已修（分区选择落 localStorage） |

## 四、结构性收敛（一次投资，多面受益）

1. **escStack**（G-6）：LIFO 栈取代三套让路约定，顺带修 U0-6，五个独立窗口的 Esc 关闭一并接入。
2. **useDismissable + useRovingItems**（G-1、B-10、C-17）：点外关闭与键盘导航各一份实现，
   CardContextMenu / 侧栏列表 / tab 条全部接入。
3. **动效与焦点 token**（G-7、G-8）：`--dur-*`/`--ease-*`/`--focus-ring` 三组 token
   + reduced-motion 兜底通配（U1-18）。
4. **密度参数化**（G-11）：`--sp-*`/`--fs-*` 改 calc，非贴纸窗恒 1 零回归。
5. **主题材质 token**（G-10）：深色调色板抽共用块、flat 改单变量驱动，
   杜绝「漏抄一个变量/选择器」这一类事故。
6. **状态语言重设计**（G-17、B-1、B-2、W-10）：统一徽标形态 + 形状维度 + 缩略条/托盘对齐
   `sessionTone()` 单点口径——这是本应用核心信息通道，值得一次专项。
7. **快捷键体系**（C-17、B-12、G-6）：全局呼出热键 + 窗口内 Ctrl+F/N/, + Ctrl+K 会话切换
   + `?` 速查表，集中注册一层。
8. ~~**PTY 事件解耦**（T-14 附带）：`pty-output`/`pty-exit` 从硬编码 `label=="chat"` 改
   app 级 emit + sessionId 过滤（前端已在过滤），解除窗口 label 耦合。~~ 已落地
   （pty-output/pty-exit/pty-external-viewers 全部 app.emit + 关窗 reset_viewer 兜底）。

## 五、2026-09-02 第七轮新发现（六路读码 + 实拍，69 条，复核后 17 条部分待补）

> 六轮关闭后的一轮新扫描。共同模式仍是「策略级修复的相邻路径漏网」：主面修了副本没跟上
> （B-4 空列表切换、U1-13 看板刷新失败、S-7 跨平台默认终端、S-3 网络分区错误行、
> G-9 的 color-mix 与容器 opacity、G-15 把桌面 tooltip 带到手机）、两个专项叠加后的
> 新遮挡（C-9 overlay × U1-5 浮钮 / × 计划审批正文）、以及远程页游离在 i18n/errors.ts
> 体系之外。文案规范（状态词/动词唯一、≤20 字、禁「——」与分号、否掉「等你」）四路独立
> 报出同一批键，建议一次改字典收口。对比度为按 WCAG 公式手算估值；macOS 条目未经本机编译。
> 先修顺序：7C-1、7C-2、7T-1、7M-1（功能性断点）→ 7C-7、7B-1/2、7G-1 → 7T-4/5、7M-2/3 → 7G-8/9 文案。
> **2026-09-02 收口：69 条全部落地**（tsc / vitest 807 项 / cargo clippy --workspace
> --all-targets / cargo test 全绿）。三处有意的**行为变更**记在这里，别当回归改回去：
> ① 按钮模式下卡片的 Enter 现在直接打开会话（7B-4，按钮模式防的是鼠标误点，键盘主键
> 不该是空操作）；② 当前打开的会话所在的侧栏分组折不动（7C-6，日期桶键固定，折过一次
> 会把用户正开着的会话一起藏掉）；③ 手机端输入控件恒 16px、viewport 去掉
> maximum-scale（7M-11，禁双指缩放是无障碍硬伤，换回可缩放 + 聚焦不跳字号）。
> 未经本机验证：7T-1 的 macOS ⌥ 拖选、7T-7 的 ⌘ 文案（`cfg(macos)` 路径，按 xterm 6.0
> 源码与 metaKey 处理器推导）；7M-2/3/9/11 的真机表现（iOS 安全区与软键盘）。
>
> **2026-09-02 复核轮（另一会话六路逐条对照）**：查出 3 条回归 + 7 处半成品，全部修完。
> 回归：7G-13 浅色规则特异性压过 `.is-failed`（失败胶囊变红底绿字）、7S-6 说明行落在
> 无 wrap 的 `.row` 里把标签挤到 0 宽、7M-11 的自定义回答框漏钉 16px 且与 7M-3 的
> visualViewport 互斥（双指缩放触发 resize 会压塌布局）。半成品：7C-7 的保存分支在
> hidden 之后跑、`scrollHeight` 恒 0 是死代码（改由 onScroll 持续记位置）；7C-9 组头
> `aria-label` 挂在无 role 的 span 上、行级 aria-hidden 后读屏全静音；7T-5 autoFocus
> 在手机上弹软键盘、7T-8「去安装」在远程是死点击（均补 `!remoteUi()`）；7B-8 账号快照
> 预填 usageMap 时漏写 usageMeta；7S-7 错误行双 format 出双前缀；7C-10 react-markdown
> 默认 urlTransform 把非 http 源置空、chip 没了文件名。
> 另落地一项用户实拍需求：transcript 里的**跨会话消息**（另一个 Claude 会话经
> SendMessage 发来）此前整块摊成 XML + 管道名 + 一段英文安全须知，现在解析成带来源的
> 消息块（`parseUserText` 的 crossMessages，见 localCommand.ts）。用户定：不要左侧
> accent 强调条（它把这类消息渲染得比用户自己说的话还重）；长消息收起态限高 220px，
> 超出才给「展开全部」。
>
> **2026-09-02 收尾轮**：复核列的 17 条「部分」与 4 条「带小遗留」全部扫完（第二轮复核
> 另查出 1 条必改——新加的中文注释被写成了字面 Unicode 转义序列，源码里不可读，已还原
> 明文并全仓核对无同类）。逐条：7S-4 账号分区挂一行视觉隐藏的 agent 名单，搜「codex」
> 不再零命中零入口；7G-7 未装项补「未安装」副文案（复用 Dropdown 的 sub 槽）；7S-1 登录
> 终端名先核对 `available_terminals`，卸载后不再念旧名；7G-1 浅色 faint 按 **hover 卡面**
> 再校到 #5e6762（三档 ≥4.5:1）；7G-5 后端通知正文的「等你」改「等待输入」；7G-8/7G-9
> 拆长句、「打断」归一并删掉两个死键；7G-10 引导圆点热区 23→25px；7G-12 残留区的
> li.is-* 就地覆盖压回 faint；7G-16 终端 spinner 补 reduced-motion；7M-9 再扩进度面板与
> 侧栏开关；7S-3 两处 updateReady 措辞统一；7M-7 手机页标题跟随语言、http_/bad_payload
> 映射带 tail 保住排障信息；7C-2 题面展示卡的「仅收起」也给折叠条；7C-6 当前会话所在组
> 点了不再写 folded 状态。


### 看板与贴纸

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| 7B-1 | 切 tab 时闪出错误 tab 的空态：从空「运行中」切「全部」，请求在途先看到「还没有会话 + 新建」（B-4 只冻结非空旧列表，旧列表为空时直接落 EmptyState） | `Sticker.tsx:1027-1046`、`App.tsx:107-111` | 空列表分支加 `switching` 判断显示加载态 | 已修 |
| 7B-2 | board-changed 刷新失败后卡片陈旧却零提示，状态环/时间全是旧值（U1-13 只给了对话窗细横幅） | `App.tsx:334-341`、`Sticker.tsx:1031` | 非空列表顶部细横幅「刷新失败，显示的是缓存」+ 重试 | 已修 |
| 7B-3 | 「全部」tab 在线与历史会话无分界，20+ 张卡滚到中段分不清这一片还是活的（U1-8 只做了排序） | `App.tsx:263,285`、`Sticker.tsx:1053-1436` | 最后一张 connected 卡后插分隔项「历史会话 · N」 | 已修 |
| 7B-4 | 按钮模式下卡片仍 role=button 可聚焦，Enter 是空操作（B-11 保证能到达，未覆盖到达后主键无响应） | `Sticker.tsx:1192,1224` | buttonMode 下 Enter 直接 openTerminal，或焦点搬到 `.stk-open` | 已修 |
| 7B-5 | 外库卡悬停提示「打开会话」，点下去却是「本窗口只读展示」toast（横切主题 5 新实例） | `Sticker.tsx:1239` vs `:564-575` | data-tip 先判 foreign / background | 已修 |
| 7B-6 | 实拍：主目录启动的会话「仓库」栏显示成 `35122`（cwd basename） | `session_query.rs:501` | cwd 等于 home 时显示「主目录」或 `~` | 已修 |
| 7B-7 | 搜索框已开、焦点在卡片时 Ctrl+F 无反应（B-12 只处理未开→开）；设置窗无 Ctrl+F 也无 × 清除（S-1 只做了搜索本身） | `Sticker.tsx:464-468`、`About.tsx:861-880` | 搜索框挂 ref，Ctrl+F 永远 focus+select；设置窗补同款与 × | 已修 |
| 7B-8 | 底栏用量刷新失败后陈旧百分比当新鲜值展示（S-6 只给了设置页「更新于」） | `Sticker.tsx:873-875` | usageMap 带 fetchedAt，失败后降饱和 + tip 上次更新时间 | 部分（复核：getAccounts 预填 usageMap 的路径不写 usageMeta，启动即显示缓存、首次刷新失败仍不标陈旧；ProviderUsage 无 fetched_at） |
| 7B-9 | 连续归档时撤销 toast 无上限堆叠，4 条盖住贴纸一半（U1-12/P1-2 为撤销互不相吞做独立条，未设上限） | `Sticker.tsx:429-438`、`styles.css:435-439` | ≥2 条合并「已归档 N 个 · 撤销全部」或上限 2 条 | 已修 |
| 7B-10 | 模型胶囊无 max-width，中转长 model id 先挤没仓库名（G-12 未覆盖卡片元信息行） | `styles.css:815`、`Sticker.tsx:1367` | 复制 `.stk-profile` 的 max-width + ellipsis，全名进 tip | 已修 |
| 7B-11 | 搜索占位符没说便签可搜（B-7 只改了后端 LIKE） | `zh.ts:40`、`en.ts:37` | 改「搜索标题 / 仓库 / 便签…」 | 已修 |

### 对话窗

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| 7C-1 | **审批卡挂载无条件抢焦点到「拒绝」钮**：用户正在输入框打插话时 broker 卡弹出，下一个空格/回车直接误拒（G-16/U1-21 定的挂载移焦 × 「不锁 composer」叠加出的输入竞态） | `ApprovalCard.tsx:27-34`、`ChatWindow.tsx:2982,3792-3795` | activeElement 是 TEXTAREA/INPUT 时不移焦，仅焦点在 body 时落到动作区 | 已修 |
| 7C-2 | **overlay 卡盖死 transcript 末尾约 500px 且滚不出来**，计划审批时刚写的计划正文正好在这段；「仅收起」后同签名不再弹回（C-9 记「盖住底部」为可接受代价，未评估被盖内容不可达 + 收起单向） | `styles.css:4156-4168,3805`、`ChatWindow.tsx:3655-3668`、`ManagedTerminal.tsx:1089` | 卡片实高写进 `.chat-scroll` padding-bottom；「仅收起」改可再展开的折叠条 | 已修 |
| 7C-3 | 「回到最新 · N」浮钮与排队回执「立即插话」被审批卡压住点不到（U1-5 × C-9 叠加） | `styles.css:4919` (z 5) vs `:4156` (z 8)、`ChatWindow.tsx:3531-3569` | 浮钮 bottom 提到 overlay 实高之上，或挪到卡片顶缘 | 已修 |
| 7C-4 | 远程作答卡「去终端作答」未门控，点后题面交还桌面终端，手机降级为「请在桌面端作答」；另三处远程文案仍指向终端页（C-11 尾未做远程降级） | `ChatWindow.tsx:3712,3108-3119,1956`、`useApprovalChannel.ts:223`、`zh.ts:287` | 按钮按 `!remoteUi()` 门控；三条文案加 Remote 变体 | 已修 |
| 7C-5 | 侧栏目录筛选/归档视图生效后无可见标记，列表只是「少了很多会话」 | `ChatSidebar.tsx:265-280,440,445` | 筛选钮 is-active 小点；归档视图在搜索框上方渲染可关闭状态条 | 已修 |
| 7C-6 | 折叠组吞掉当前会话（日期分桶键固定 `date:today`，折过一次第二天新会话全藏）；activeId 在折叠组内时无条目带 tabIndex=0，Tab 进不了列表（C-17 roving 锚点指向未渲染节点） | `ChatSidebar.tsx:886-889,1244,815-820,184-189` | tabbableId 从实际渲染条目取；activeId 所在组强制展开；日期/状态分组折叠不落盘 | 已修 |
| 7C-7 | 切终端再切回仍强制回底，阅读位置丢失（C-14 标已修但离开 chat 视图时仍重置 followRef/positionedRef） | `ChatWindow.tsx:2032-2045`、`styles.css:4990` | 离开前记 scrollTop/followRef 切回恢复，或隐藏改 content-visibility | 部分（复核：效果靠删掉旧的 followRef 重置成立；新写的 savedScrollRef 保存分支在 hidden 落 DOM 之后才跑、scrollHeight 恒 0，是死代码，注释「display:none 清零 scrollTop」经 Chromium 实测为假，需重做或删干净） |
| 7C-8 | 作答卡自定义回答按 Enter 无反应，屏幕识别卡同款输入框回车即提交 | `QuestionPanels.tsx:114-120` vs `ChatWindow.tsx:3614` | answer 形态 input 加 onKeyDown 提交 | 已修 |
| 7C-9 | 工具组摘要中英混排（只本地化 Bash/Read/Write）；每条运行中工具行都是 role=status，并行时读屏被轮番打断（G-16 漏网副本） | `ToolActivity.tsx:5-11,44`、`Transcript.tsx:174-179` | friendlyToolName 补常见项；行级 aria-hidden，组头一处播报 | 部分（复核：工具名映射与行级 aria-hidden 已做；组头 Transcript.tsx:184 的 aria-label span 无 role 不会被读出，读屏现在完全听不到「运行中」） |
| 7C-10 | markdown 图片无兜底：大图撑破 720px 列出横向滚动条，本地路径是破图标无说明（U1-28 只覆盖代码块） | `ChatMarkdown.tsx:154-198` | `.chat-md img { max-width:100% }`；非 http 源复用 chat-image-chip | 部分（复核：max-width 已做；C:\… / file:// 源被 react-markdown defaultUrlTransform 置空，chip 只剩图标无文件名，需自定义 urlTransform） |

### 终端与新建会话

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| 7T-1 | **终端鼠标上报时选不了字、右键变粘贴**：Windows 需 Shift 拖动无提示；macOS 未设 `macOptionClickForcesSelection`，按 xterm 6.0 源码彻底无法选择（不动 PTY 序列，与「不注入」决策不冲突；macOS 未经本机验证） | `ManagedTerminal.tsx:557-579,768-799` | 构造项加 `macOptionClickForcesSelection:true`；上报模式下右下角复用 link-hint 提示「按住 Shift（macOS ⌥）拖动可选择」 | 已修 |
| 7T-2 | 文件路径链接在含中文的行上错位，每个宽字符左偏 1 格，Ctrl+点击打不开（T-4 未覆盖坐标口径） | `ManagedTerminal.tsx:600-604` | `line.getCell(i).getWidth()` 把字符串下标映射到单元格下标 | 已修 |
| 7T-3 | 终端字号首帧按 12 起，settings 晚到再重排，PTY 多吃一次 resize（U1-17 第 5 处首帧占位） | `ManagedTerminal.tsx:572-576,997` | 挂载前先拿 settings 再 new Terminal，或由 ChatWindow 传入 | 已修 |
| 7T-4 | 带引号的目录路径（资源管理器「复制为路径」）被拒为「目录不存在」 | `NewSessionPanel.tsx:290`、`paths.ts:8-14`、`terminal.rs:1791` | normalizePath 剥首尾成对引号 | 已修 |
| 7T-5 | 新建面板打开无初始焦点，目录已预填却回车无反应（U1-15 预填后暴露） | `NewSessionPanel.tsx:393-402` | 输入框 autoFocus 并全选，或有预填时焦点落启动按钮 | 部分（复核：autoFocus 无 remoteUi() 门控，手机端打开新建面板即弹软键盘） |
| 7T-6 | 输入框 Enter 绕过启动按钮禁用条件，未装 CLI/检测中照发 newSession | `NewSessionPanel.tsx:401` vs `:642,283` | 抽 canLaunch 守卫，Enter 与按钮共用 | 已修 |
| 7T-7 | macOS 上「Ctrl+点击」提示教错键，处理器实际收 metaKey（未经本机验证） | `zh.ts:372-373,137`、`ManagedTerminal.tsx:554,606`、`NewSessionPanel.tsx:496` | 文案走 `isMac()` 写「⌘+点击」 | 已修 |
| 7T-8 | 未检测到 CLI 时是死胡同，无按钮无去处（S-11 在引导里说了去哪装） | `NewSessionPanel.tsx:520-523`、`zh.ts:151` | 警示行加「去安装」按钮调 open_settings | 部分（复核：「去安装」钮在远程是死点击，transport 对 open_settings 静默返回 null，需门控） |
| 7T-9 | 预填 Agent 未安装时静默换成别的 | `NewSessionPanel.tsx:215-217` | Agent 区下留一行「X 未安装，已改用 Y」 | 已修 |
| 7T-10 | 终端右键菜单 Esc 关闭后焦点落空，继续打字没反应（U0-11 焦点归还漏了） | `ManagedTerminal.tsx:1905-1917`、`useDismissable.ts:79` | onClose 补 `terminalRef.focus()` 或传 escFocusReturn | 已修 |

### 设置与更新

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| 7S-1 | Windows 默认档登录提示写「已在 Terminal 打开」，实际弹的是 Windows Terminal/PowerShell（S-7 只接了 resume_terminal 原值） | `AccountSection.tsx:245-254`、`settings.rs:21-23`、`terminal.rs:1638` | 存值不在本平台选项内时按后端同规则回退，或退回不带终端名文案 | 已修（复核遗留：存值 wt/wezterm 已卸载时仍念旧终端名，前端可接 availableTerminals() 对齐后端回退） |
| 7S-2 | 「重启并更新」按下后无进行态，「稍后」/Esc 仍可用，安装器起来前数秒画面不动（U0-12/S-13 止步于确认与失败） | `Updater.tsx:186`、`useUpdate.ts:78-88` | installing 期间状态行与主按钮换「正在安装…」，later/Esc 让位 | 已修 |
| 7S-3 | 更新已下载态在关于页与贴纸底部按钮都压成「有新版本」（U0-7/S-13 只修了 unknown/checking/error） | `About.tsx:711,721`、`zh.ts:38`、`App.tsx:958` | available / downloading / ready 分三条文案 | 已修（复核遗留：sticker.updateReady 与 about.updateReady 措辞不一，违背状态词唯一） |
| 7S-4 | 搜索态账号分区只搜当前 agent，切换器被隐藏，搜「codex」无结果无入口（S-1 未覆盖同分区单卡） | `searchFilter.ts:74`、`AccountSection.tsx:1334` | 搜索态豁免 `.account-agent-switch`，或命中 agent 名自动 pickAgent | 部分（复核：只解决分区已可见时切换器被藏；搜「codex」仍无结果无入口，建议的自动 pickAgent 未做） |
| 7S-5 | 弹层内拖选文字松手在遮罩上会关闭弹层、草稿丢失（G-3 只做了焦点陷阱） | `AppModal.tsx:82` | onMouseDown 记起点，起终点都在遮罩上才 onClose | 已修 |
| 7S-6 | 远程端口非法输入静默回退 18620，无说明 | `RemoteAccessCard.tsx:19-24` | 行内错误「端口须为 1 到 65535」，保留输入直到修正 | 已修（复核回归修补：说明行 flex-basis 100% 落在 nowrap 的 .row 里会把「端口」标签挤到 0 宽，加 .remote-port-row flex-wrap） |
| 7S-7 | 网络分区与远程卡错误行无 × 无 role=alert，与其他四分区不一致（08-31 尾轮只补了账号分区） | `NetworkSection.tsx:325`、`RemoteAccessCard.tsx:146-152` | 改用 About.tsx 的 SettingsError | 已修（复核遗留：远程卡 startError 外再套 formatBackendError，英文界面双前缀） |
| 7S-8 | 确认窗正文 11px + dim 色，危险后果句是整窗最小最淡的字（S-14 只改了按钮） | `styles.css:3490,3493` | 正文升 `--fs-12-5` + `--cc-text` | 已修 |
| 7S-9 | 关于页「打开官网」是唯一 primary，有新版本时两个主按钮；「使用引导」副标写「欢迎使用 Meowo」 | `About.tsx:746,762` | 官网降级普通按钮；副标改「重看首次引导」 | 已修 |
| 7S-10 | 英文 Appearance 分区第一行也叫 Appearance | `en.ts:599,649` | theme 改 Theme / Color scheme | 已修 |

### 视觉、文案与可访问性

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| 7G-1 | **faint 文字档按裸底校准，实际落在叠 surface 的卡面**：活动行/时间/tab 计数/代码块语言角标深色约 4.34:1、hover 3.8:1，浅色 4.0～4.23:1（估算；G-9 修的是 alpha/opacity 叠加） | `styles.css:44-45,157-158`、消费 `:715,825,803,353,4970` | faint 按叠层后的面重校（深约 #9aa09b、浅约 #636c67），或卡内 ≤10.5px 改 dim | 部分（复核实算：浅色 hover 卡面 4.39:1 仍不达标） |
| 7G-2 | 对话输入框占位符 color-mix 78% 透明，约 3.4:1，却是 Enter/Shift+Enter 约定唯一入口（绕回 G-9 同一坑） | `styles.css:4604` | 直接用 `--cc-text-faint` | 已修 |
| 7G-3 | 同一张待批准卡两种橙：徽标琥珀黄、pill 橙红，终端横幅第三种（G-17/B-2 统一了徽标，pill 与横幅硬编码漏网） | `styles.css:879-886` vs `:642`、`:4987-4988` | `.pending-pill` 与终端横幅改吃 `--cc-warn-vivid` | 已修 |
| 7G-4 | 悬停元素被移除后 tooltip 残留（运行状态条卸载不派发 mouseout），直到 Esc/点击 | `Tooltip.tsx:47-57,66-73`、`ChatWindow.tsx:3536` | show() 对无 tip 新目标先 hide；加 `anchor.isConnected` 检查 | 已修 |
| 7G-5 | 「等你」造词回流：对话窗标题栏「等你输入」、终端横幅「Agent 在等你」、读屏汇总、引导「谁在等你」，与徽标「等待输入」不一致；en「Waiting for you」同病 | `zh.ts:207,263,516,973` vs `:19`、`en.ts:198` vs `:17` | 统一「等待输入」；263 改「有 1 条待批准的请求」 | 部分（复核：前端已清零；settings.rs:317 系统通知正文仍「等你」，en waitingTitle 与 zh 不对位） |
| 7G-6 | 断开状态两套词：对话窗「未连接」vs 看板「已断开」，tip 还写「已断开/已停止」 | `zh.ts:208` vs `:48,101,998` | 208 改「已断开」，48 去「/已停止」 | 已修 |
| 7G-7 | 可点的「未安装」下拉项 opacity .5 压成约 2.5:1，但它是安装入口不是禁用项 | `styles.css:1988`、`menu.tsx:219`、`AccountSection.tsx:1350` | opacity 只灰化图标，文字 dim + 「未安装」副文案 | 部分（复核：对比度已修；建议的「未安装」副文案未做，未装项现在只差半透明图标，信息更弱） |
| 7G-8 | 文案规范硬伤集中：分号 `zh.ts:164,166,226,657,659,863,890`；「——」`:362`；超 20 字 `:62,104,106,119,122,747,751,764,779,780,842,846,848,989`；黑话「后端」`:226`、「hooks」`:989,991` | 见左 | 分号拆句；362 改「已强制结束（退出码 X）。进程可能仍在后台，上方保留了终端输出」；hooks→接入 | 部分（复核：分号/破折号/黑话清零；zh.ts:1040 只换词没拆句仍 60 字，gateErr 两条 21～22 字擦线） |
| 7G-9 | 同一动作多个动词：打断/中断、折叠/收起、跳过权限确认/检查、计划模式/计划；切终端页按钮四种标签（打开终端/终端/去终端作答/去终端页切换模型） | `zh.ts:245,457,400,185,584,183,581`、`ChatWindow.tsx:3494,3735,4184,4468` | 统一「中断」「收起」「跳过权限确认」「计划模式」「去终端页」，en 同步 | 部分（复核：zh.ts:506 仍「打断」；answerInTerminal / openTerminal 成死键） |
| 7G-10 | 桌面端五处点击热区不足 24px：toast 关闭叉、卡片 ⋯、用量 provider 标签、引导步骤圆点 7px（G-15 从未审过尺寸） | `styles.css:470-474,948-951,524-526,1566-1579` | B-14 的 `::before` 扩热区手法，视觉不变 | 部分（复核：引导圆点 7+8×2=23px 差 1px，需 inset -9px） |
| 7G-11 | 远程卡按钮引用不存在的 token（--radius-md/--cc-surface-2/--cc-surface-3），hover 零反馈 | `styles.css:2023,2044,2046,2050` | 改 `--cc-surface(-hover)` / `--r-md` / `--r-sm` | 已修 |
| 7G-12 | 「上一任务」残留区整块 opacity .62，已完成项约 2.7:1（G-9 之后新增） | `styles.css:4417,4429` | 去容器 opacity，faint 单色 + 虚线容器 | 部分（复核：opacity 已去；进行中/待办项被 li.is-* 就地覆盖成与新清单同色，「弱化」只剩虚线边） |
| 7G-13 | 浅色主题 color-mix 出的子任务胶囊约 3.96/4.1:1（深色 7.5:1 无事） | `styles.css:4502,4566` | 浅色改 `--cc-accent-text` | 已修（复核回归修补：原 :root[data-theme=light] .chat-subagent-status 特异性压过 .is-failed，浅色失败胶囊变红底绿字；现只覆盖图标与类型/已完成两个绿胶囊，胶囊文字压深 15% 到约 5.3:1） |
| 7G-14 | Windows 中文界面 550/560/590 字重跳成 700，中英混排中文粗英文细 | `styles.css:4390,4399,4492,4753,4795,4799,5012` | 中间字重收到 500 或 600 | 已修 |
| 7G-15 | en 大小写/标点不成对：agent vs the Agent、成对文案一条带句点一条不带、「1 entries」 | `en.ts:143,147,158-162` vs `:267,285`、`:154/155`、`:407` | Agent 统一大写；成对文案统一标点；单数分支 | 已修 |
| 7G-16 | reduced-motion 下 4 个转圈定格成缺口环（U1-18 通配副作用，3452 只修了一处） | `styles.css:2194,5082,4188,1372-1380` | 同一 @media 块补 `border-*-color: currentColor` | 部分（复核：.managed-terminal-spinner 未补，静止仍是缺口环） |

### 手机远程页

| # | 问题 | 位置 | 建议 | 状态 |
|---|---|---|---|---|
| 7M-1 | **配对后整屏空白数秒**：Suspense fallback 为 null，ChatWindow chunk 1.2MB 走 Tailscale 中继像卡死（C-12 骨架屏只覆盖窗内切会话） | `mobile/main.tsx:48-53,92` | fallback 换同底色「正在加载」行；考虑拆 hljs/xterm 出远程 chunk | 已修 |
| 7M-2 | 刘海机顶栏：42px 定高 + safe-area padding-top 被吃空，☰ 溢出到标题行；抽屉头未留白 | `mobile.css:27-29` × `styles.css:3538-3545,3596` | `.chat-topbar` 改 `min-height: calc(42px + env(safe-area-inset-top))`，抽屉头同步 | 已修 |
| 7M-3 | iOS 软键盘弹出时 100dvh 页面被整体顶走，composer 可能藏在键盘后；全仓无 visualViewport 处理（调研标「待真机确认」至今未收口） | `mobile.css:15-24` | visualViewport resize 写 `--app-height`；安卓 `interactive-widget=resizes-content` | 已修（复核回归修补：与 7M-11 互斥，双指放大也触发 visualViewport resize，未判 scale 会把 --app-height 压成缩放后高度并滚回原点；加 scale>1.01 跳过。iOS 真机未验） |
| 7M-4 | 桌面离线最长 60s 才提示（20s 超时 × 3 次），文案「后端」是黑话且含分号 | `transport.ts:165`、`ChatWindow.tsx:1842-1845`、`zh.ts:226` | 轮询类超时压到 5s；远程文案「与桌面端失去连接，正在重试」 | 已修（复核遗留：remote/timeout 译文「正在重试」也用在不重试的发送路径，措辞待中性化） |
| 7M-5 | 手动配对后主题/语言最长 12s 才纠正，中途整页闪换（setInterval 无首发） | `mobile/main.tsx:35-46` | 轮询体抽函数，TokenGate 验证通过时立即调一次 | 已修 |
| 7M-6 | 配对失败不分「令牌错」与「连不上」，401 与网络异常同回 false | `TokenGate.tsx:51-53`、`transport.ts:85-99` | probeToken 回三态 ok / unauthorized / unreachable | 已修 |
| 7M-7 | 远程错误文案硬编码中文、半角标点，errors.ts 无 remote sentinel（S-9 改造漏网） | `TokenGate.tsx:46-69`、`mobile.html:12`、`transport.ts:154-264`、`remote.rs:654,680,698,1149` | 闸门文案入 `remote.*` 字典；remote.rs 改 `remote/<code>` 结构化 reason 并补映射 | 部分（复核：四段落地；mobile.html <title> 仍硬编码中文，remote/http_ 映射丢状态码、bad_payload 丢 cmd） |
| 7M-8 | 远程新建会话遇目录信任/登录询问时「正在启动…」永远转，无出口文案（T-8 的出口按钮被 `!remoteUi()` 门掉） | `ChatWindow.tsx:3488-3497,575-580` | 远程 startingSlow 时渲染「启动等待中，请在桌面端确认」 | 已修 |
| 7M-9 | 关键小按钮触控热区 32～38px 不足 44px：侧栏 ⋯（远程唯一星标/重命名/归档入口，误点直接打开会话）、审批 ✕ 与「拒绝」相邻 | `styles.css:3765,4999,3641,3419`、`mobile.css:84-91` | 触屏块对这四类改 `inset: -12px` | 部分（复核：点名的 .chat-todo-btn 未扩仍 38px，抽屉 ☰ 40px、搜索 × 41px 亦不足 44） |
| 7M-10 | 配对页不跟主题（--bg/--fg/--panel 未定义恒落深色），无 theme-color | `mobile.css:199-242`、`mobile.html` | 改用 `--cc-*` token，补 meta theme-color | 已修 |
| 7M-11 | 安卓 `maximum-scale=1` 禁双指缩放，与「放大交给系统」注释矛盾 | `mobile.html:7-10` × `mobile.css:37-40` | 去 maximum-scale，输入控件钉 16px，`touch-action: manipulation` | 已修（复核补漏：.chat-approval-custom input 仍 13px 已钉 16px，mobile.html 头注释同步） |
| 7M-12 | 触屏残留桌面键盘语义：审批卡「拒绝 Esc」徽章、长按发送钮弹 Ctrl+Enter 说明（G-15 长按 tooltip 带过来的） | `ChatWindow.tsx:3792-3794,4428`、`zh.ts:457` | 两处加 `!remoteUi()` 门控 | 已修 |

## 做得好的（保留，勿在重构中弄丢）

- 恢复时权限改选合并写回会话存档并自动沿用（`terminal.rs:1410-1515`）——新建面板应复用此模式（U1-15）。
- 后台会话的错误建模：第一次按键领回对话页、退出码与「没接上」严格区分（`ManagedTerminal.tsx:295-300,771-786`）。
- 账号用量「缓存先显示再刷新 + 500ms 最短 spinner + 失败保留旧值标注缓存」（`AccountSection.tsx:1062-1084`）。
- 确认窗默认焦点在取消、Esc 取消、活跃账号可删且回落默认（`ConfirmWindow.tsx:100-116`）。
- i18n 基础设施（`Dict = typeof zh` 编译期对齐、subscribe-first、`<html lang>` 跟随）。
- 引导内设置真·即时生效、步骤点可跳（`Onboarding.tsx:250-321,409-416`）。

## 工程侧勘误（顺手改）

- consistency-backlog P0-2 状态应改「已修」：四处已全部走 `appConfirm`，全仓已无 plugin-dialog confirm 用法。
