# 交互体验优化清单（UX backlog）

> 2026-08-12 六路并行深度调研汇总：看板主窗 / 对话窗 / 托管终端与新建会话 /
> 设置·账号·引导·更新 / 窗口管理与通知 / 全局交互基础设施。
> 每条给出位置与建议做法；行号为调研时点参考，以实际代码为准。
> 与 [consistency-backlog](architecture/consistency-backlog.md) 同纪律：
> 修一条勾一条，新发现追加，不搞一次性大重写。

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
| U0-11 | 生产构建下托管终端**无法复制**：Ctrl+C 发 `^C` 中断 agent，Ctrl+Shift+C 被 devtools-guard 吃掉，右键菜单 PROD 全局禁用 | `ManagedTerminal.tsx:232-237`、`devtools-guard.ts:7,14-19` | 有选区时 Ctrl/Cmd+C 走 `getSelection()`+clipboard；devtools-guard 放行 Ctrl+Shift+C；补终端右键菜单 | 大部分已修（复制快捷键已落地；终端右键菜单待做） |
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
| U1-14 | 侧栏无搜索（看板有，`getLiveSessionsPage` 已支持 search 位），会话上百条只能滚动翻页找 | `ChatSidebar.tsx:366,731-742` | 接入 search 参数，或做 Ctrl+K 命令面板 | 已修（侧栏搜索框 + Ctrl/Cmd+F 聚焦，下沉后端 search 通道；Ctrl+K 命令面板另议） |
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
| U1-28 | 代码块无复制按钮/语法高亮/语言角标——AI 输出里最高频「要拿走」的内容只能框选 | `ChatMarkdown.tsx:73-96`、`styles.css:3884-3885` | pre 包复制按钮容器 + language 角标；高亮可按需引 shiki/prism | 大部分已修（复制按钮 + 语言角标）；语法高亮待评估依赖 |

## 三、按交互面的其余发现（中影响，按面分组）

### 看板主窗

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| B-1 | 状态槽三种视觉形态（36px 徽标 / 16px 虚线环 / 9px 点），扫读无统一锚线 | `Sticker.tsx:619-635`、`styles.css:572-783` | 统一直径圆形徽标，填充/描边/色相区分 |
| B-2 | `screen_state==="blocked"` 无 pill 无文案，waiting 黄与 pending 琥珀肉眼难分 | `Sticker.tsx:623,709` | 已修（blocked 也给「待操作」pill，不再只靠琥珀环色相） |
| B-3 | waiting tab「等最久优先」的倒排无任何 UI 提示，像列表坏了 | `query.rs:203`、`Sticker.tsx:714` | 已修（waiting tab 时间改「已等待 X」+ 警示色，倒排排序自解释） |
| B-4 | 切 tab/搜索不重置滚动位置；切 tab 中间态先渲染旧数据子集（waiting 的 ASC 到达后整列表翻转） | `App.tsx:351-379`、`Sticker.tsx:377-385` | 部分已修（切 tab/搜索滚动回顶；切换期中间态过渡仍待做） |
| B-5 | `context` 菜单模式下星标/便签/重命名/归档零可见入口、零提示 | `Sticker.tsx:680-686,728` | hover 时给极淡 ⋯ 提示或一次性引导 |
| B-6 | 每次 board-changed 整窗口重查（页越滚越大，500 条时每事件重查 500 行） | `App.tsx:292` | 只重查首页 + 增量修补，或仅刷新可视区 |
| B-7 | 搜索不含便签内容（用户亲手写的记忆锚点搜不到；JOIN 已在 SELECT 里） | `query.rs:184-191` | LIKE 条件加 `sn.note` |
| B-8 | 便签块无行数限制，500 字便签撑满窗口且与 82px 估高差 3.6 倍致滚动抽搐 | `styles.css:873-884`、`Sticker.tsx:392` | 3 行 clamp + 点击展开；estimateSize 分档 |
| B-9 | 编辑中滚动列表虚拟化卸载 EditBox → 草稿静默丢失，滚回显示原文 | `Sticker.tsx:71,393` | 草稿提升到 Sticker 层按 sessionId 存 |
| B-10 | 卡片菜单键盘完全不可达（无 autofocus/方向键/焦点归还，DOM 挂在滚动区外 Tab 进不去）；项目自己的 `useMenuPopup` 有完整实现 | `CardContextMenu.tsx:57-131` vs `menu.tsx:146-171` | 已修（挂载聚焦首项 + ↑↓/Home/End/Enter + 关闭归还焦点，测试钉住） |
| B-11 | 卡片 `role="button"` 内嵌 4 个可聚焦控件，100 张卡 = 200-400 次 Tab | `Sticker.tsx:650-653,715,729,794` | 整卡一个 Tab 停靠点 + 卡内 roving |
| B-12 | tablist 无方向键/`aria-controls`/tabpanel；无任何窗口级快捷键（Ctrl+F 搜索、Esc 三级回退） | `Sticker.tsx:548-576` | 部分已修（Ctrl+F 搜索 + Esc 回退链已加；tablist 方向键待做） |
| B-13 | focus toast 无 Esc/点外关闭，带动作类型不自动消失且盖掉小窗列表下半部 | `Sticker.tsx:898-935`、`styles.css:384-393` | 已修（Esc 关闭 toast） |
| B-14 | 自绘滚动条 4px 且仅悬停贴纸才显形，无点击轨道跳转 | `styles.css:478-490` | 热区扩 12px（视觉不变），溢出时低透明常显 |
| B-15 | loadMore 的三点 loader 落在底部淡出遮罩里且不计入滚动高度 | `Sticker.tsx:839-845`、`styles.css:357-367` | loader 作为内层容器正常流子元素 |
| B-16 | tab 滑块宽度硬编码 1/3 与 TAB_KEYS 靠注释同步；星标后卡片瞬移无 FLIP 动画 | `styles.css:265-276`、`Sticker.tsx:863` | 已修（滑块按选中按钮实测 left/width 定位 + ResizeObserver 重测，1/3 硬编码与 TAB_KEYS 的注释耦合消除；星标 FLIP 动画仍待做） |

### 对话窗

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| C-1 | 「加载更早」一次性全量重读且只能点一次 | `chat.rs:97`、`ChatWindow.tsx:1314-1319` | 改 offset 增量前向分页 |
| C-2 | 工具调用折叠组收起态看不出「还有工具在跑」；工具输出 pre 无 max-height 撑爆几十屏 | `Transcript.tsx:90-96`、`styles.css:3322` | 已修（组摘要按无回执数显示跳动点；.chat-tool pre 限高 40vh + overscroll contain） |
| C-3 | 消息无任何时间信息（timestamp 字段有传递但未渲染） | `Message.tsx:212-317` | 消息分组显示相对时间 / 按天分隔条 |
| C-4 | 斜杠补全只前缀匹配、带参数即消失；`@` 附件提及能力已有但无补全菜单；补全菜单 ARIA 断裂 | `ChatWindow.tsx:863-865,178-192,2391-2449` | 子串匹配 + 参数提示行 + `@` 文件补全 + combobox 属性 |
| C-5 | 支持粘贴图片但不支持拖拽放入 | `ChatWindow.tsx:2451-2457`（无 onDrop） | `.chat-compose` 加 drop 区复用 pasteAttachments |
| C-6 | Ctrl+Enter 打断并发送的提示挂点被删成孤儿文案，用户发现不了 | `ChatWindow.tsx:2680-2684`、`zh.ts:294` | tip 挂到发送圆钮 data-tip |
| C-7 | 草稿只在内存 Map，关窗即丢（附件反而落盘了） | `ChatWindow.tsx:538,949-983` | 按 sessionId 落 localStorage |
| C-8 | 一敲键盘「接管」按钮随 sendError 一起消失，needsTakeover 还悬着 | `ChatWindow.tsx:2450,2702-2714` | 接管条显隐挂 needsTakeover |
| C-9 | 审批卡插入文档流下推 composer，「允许一次」深色主按钮与拒绝仅隔 8px，`rm -rf` 与 `ls` 同视觉权重 | `styles.css:3343-3399`、`ChatWindow.tsx:2357-2360` | 入场占位防位移；按风险分级主按钮；加间距 |
| C-10 | 审批提交失败空 catch 完全静默；长命令详情嵌套两层滚动无展开/复制 | `ChatWindow.tsx:1861-1863`、`styles.css:3346,3378` | 失败写卡内错误行；加「展开全部/复制命令」 |
| C-11 | 多问题/多选题渲染成 tab 却不能卡内作答，只给「去终端」 | `ChatWindow.tsx:1910-1911,2314` | queuedAnswer 升级 Map；至少单问题多选可点 |
| C-12 | 切会话整屏清空 + 全屏加载文案三段跳；外部切换时侧栏不 scrollIntoView 当前项 | `ChatWindow.tsx:972-976,2105`、`ChatSidebar.tsx:633` | 骨架屏 + loading 超 150ms 才显示；activeId 变化 scrollIntoView |
| C-13 | 归档后自动跳转逻辑两份（侧栏用本地 ordered，ChatWindow 另发查询），目标不可预期 | `ChatSidebar.tsx:527-530` vs `ChatWindow.tsx:651-655` | 抽 `useSessionActions` 统一 |
| C-14 | 流式输出 650ms 一批的蹦字观感；切终端再切回强制丢阅读位置（DOM 实为 hidden 未卸载） | `ChatWindow.tsx:1047-1051,1269-1287` | 部分已修（pty-output 驱动 200ms 合流提前刷新，托管会话蹦字节奏从 650ms 压到输出帧粒度；650ms 轮询保留兜底。真·transcript watch push 待做） |
| C-15 | 任何发送错误都把占位符改成「尚未接管」（与真实原因无关）；会话结束时 composer 整块卸载藏起草稿 | `ChatWindow.tsx:2443-2449,2374-2390` | 占位符只随 needsTakeover；gate 态禁用而非卸载 |
| C-16 | 超长粘贴固定 250ms 后回车，TUI 可能没消化完；`sessionLaunchSelections` 无 stale 守卫会把旧会话启动档落到新会话 | `ChatWindow.tsx:405-414,727-730` | 按长度动态间隔/屏幕确认；补 stale 清理 |
| C-17 | 快捷键体系整体缺位：无切会话/收展侧栏/切视图/新建/查找；侧栏平铺 tabindex 上百次 Tab | `ChatWindow.tsx:1948-1962`、`ChatSidebar.tsx:629-647` | 已修（Ctrl+K/B/1/2/N/F + ? 速查表全部落地；侧栏 roving tabindex 归 C-17 后续小项） |
| C-18 | 断线语言缺失：分不出「agent 进程没了」和「IPC 通道断了」 | `zh.ts:153-160`、`ChatWindow.tsx:565` | IPC 连续失败单列「同步中断」横幅与 tone 解耦 |

### 托管终端 / 跳转恢复 / 新建会话

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| T-1 | 初始化遮罩 25s 兜底后静默撤下留无解释黑屏，无重试/结束入口 | `ManagedTerminal.tsx:66-67,357-360` | 超时显示提示条 +「结束/重新接管」动作 |
| T-2 | 隐藏期离屏 xterm 固定 1000×700 与 PTY 网格不一致，二次折行致屏幕识别漏检 | `styles.css:3938`、`ManagedTerminal.tsx:418-430` | 离屏尺寸对齐 PTY 当前 cols/rows |
| T-3 | 无 WebGL/Canvas 渲染器、无 Unicode11 宽表（emoji/框线错位半格）、无搜索 addon、scrollback 5000 硬编码、alt-screen 下滚轮翻不了历史 | `ManagedTerminal.tsx:204-224`、`package.json:23-25` | 引 addon-webgl（失败回退）+ unicode11 + search；scrollback 进设置 |
| T-4 | 链接需 Ctrl+点击但无提示；文件路径（最高频可点内容）一律不可点 | `ManagedTerminal.tsx:200-203`、`settings.rs:556-562` | hover 提示；文件路径独立通道走文件管理器 |
| T-5 | 操作条 hover 才显形，无 `:focus-within` 无 `hover:none` 兜底；PTY 假死红条只给「关闭」不给「结束并恢复」 | `styles.css:4026-4029`、`pty.rs:1318-1328` | 补 focus-within/hover:none；假死错误内联恢复按钮 |
| T-6 | attach 成功零正反馈（窗口在别的虚拟桌面时=「点了没反应」）；`permission_denied` 无一键跳系统设置 | `terminal.rs:1900-1904`、`Sticker.tsx:913-923` | 成功 toast「已在 X 打开」；macOS 权限直达按钮 |
| T-7 | 恢复全程零反馈（后端乐观复活已 emit board-changed，前端没用它做即时态）；重复恢复判重静默 Ok 但 reveal 再跑一遍又开一个镜像标签 | `Sticker.tsx:316-322`、`pty.rs:892-921`、`terminal.rs:1953-1964` | 已修（卡片 is-opening 置灰 + 「正在恢复会话…」toast，settle 即清；后端判重冒泡仍待做） |
| T-8 | 秒退探测只盖前 1 秒，banner 后报错的 CLI 探测不到；`waitForTerminalReady` 45s 无进度无取消 | `terminal.rs:2033-2049`、`ChatWindow.tsx:424-458` | 5s 短期观察者 + 失败带 tail 提示；等待显示秒数 + 「去终端页看」 |
| T-9 | 对话页恢复/接管硬编码 100×30 起 PTY，首屏画完再 resize 重排一遍 | `ChatWindow.tsx:1435,1478` | 读当前 xterm cols/rows |
| T-10 | 高风险启动档（bypassPermissions）与普通档同视觉零警示；契约无 description/risk 字段；未知 id 直接裸露 | `NewSessionPanel.tsx:336`、`LaunchOption.ts` | 契约加 description/risk，下拉渲染副标题 + 警示色 |
| T-11 | relay 的 `--model` 静默压过用户在面板选的模型 | `terminal.rs:1704-1716` | relay 启用时 model 选项置灰注明「由中转固定为 X」 |
| T-12 | 启动后面板一闪而灭，无 toast 无占位卡（组件文档承诺的 emit 不存在，codex 到首 turn 才出卡） | `NewSessionPanel.tsx:45,180-181` | 补 toast + 临时负 id 先占「正在启动」卡片 |
| T-13 | 临时 id→真实 id 重绑换 key 整只重挂终端，首屏清空重画 | `ChatWindow.tsx:1053-1062,2284` | 不换 key，内部 rearm 平滑切换 |
| T-14 | 双视图同写同一 PTY：resize 无仲裁（两窗尺寸不同时 TUI 反复重排）、输入无互斥无提示 | `pty.rs:2046-2051,1307-1329` | 聚焦视图为尺寸主控；检测到外部视图在线时提示输入可能交错 |
| T-15 | 外部会话角标回落 DB status 但与托管会话同呈现，无「不新鲜」区分；「运行中→空闲」四层节流叠加最坏 1.7s；fallback 规则命中时角标依然「很自信」 | `pty.rs:1636-1651`、`detect.rs:371-413`、`useBoardRefresh.ts:6` | 无 screen_state 弱化样式；降级转变走高优通道；fallback 回传降级为中性点 |
| T-16 | 强制收尾 emit pty-exit 但进程可能仍在（zombie），UI 显示「已结束」无区分 | `pty.rs:445-446,1073-1085` | PtyExitEvent 加 forced 字段区分文案 |

### 设置 / 账号 / 引导 / 更新

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| S-1 | 30 项设置零搜索；「Agent」分区承载账号/配额/登录/安装/中转五件事（key 叫 account 显示叫 Agent）；「在贴纸显示配额」埋在已登录账号卡深处 | `About.tsx:26,604-630`、`AccountSection.tsx:624-651` | 部分已修（「Agent」分区改名「账号与用量」；设置搜索与展示项挪位待做） |
| S-2 | 切分区 `key={sec}` 整树重挂 + 重拉数据（含联网配额查询）；设置窗 620×460 不可缩放，长内容嵌套滚动陷阱 | `About.tsx:643,188-194`、`window.rs:113-115` | CSS 隐藏代替卸载 / 数据提升共享；放开纵向缩放 |
| S-3 | 通用/会话/外观三分区保存失败完全静默（开关自己弹回去），只有网络分区显示错误 | `About.tsx:129-476` 各 patch 调用 | 已修（useSettingsState 暴露 lastError；通用/会话/外观/账号四分区渲染错误行，网络分区原有） |
| S-4 | 设置窗不订阅 settings-changed（现成的 useSettingsEffect 没用），与 Onboarding 同开时整对象写回互相覆盖 | `state.ts:44-66` | 已修（state.ts 订阅 settings-changed） |
| S-5 | 调不透明度/字号时设置窗自己纹丝不动（密度只在贴纸窗生效），等于闭眼调 | `appearance.ts:78-80`、`About.tsx:446-478` | 行旁放迷你贴纸预览（Onboarding 已有 obm-card 组件） |
| S-6 | 配额无「更新于 X 分钟前」，设置页不自动刷新（贴纸 5 分钟刷）；对未登录 provider 也发配额查询 | `AccountSection.tsx:632-646,1082` | 已修（设置页 5 分钟自动刷新 + 「更新于 HH:MM」；挂载刷新过滤对齐贴纸——未登录/中转不再发请求） |
| S-7 | 登录 pending 态零指引（不说在哪个终端开了窗、等多久）；「取消等待」英文译成裸 Cancel 生歧义 | `AccountSection.tsx:464-466`、`en.ts:569` | pending 显示「已在 {终端} 打开…最长 5 分钟」；en 改 Stop waiting |
| S-8 | 安装是不可中断黑盒（无进度/耗时/取消）；失败日志路径纯文本不可点；「修复连接」按钮凭空出现零解释；修复失败指向不存在的「运行终端」 | `AccountSection.tsx:436-452,568-573,495-505`、`zh.ts:118` | 部分已修（安装中 ≥5s 显示已耗时；失败日志改可点按钮与预下载体量说明待做） |
| S-9 | 切账号不说明「仅对新会话生效」（后端注释明确，UI 无一字）；后端错误硬编码中文直出（英文界面显示中文） | `profile/mod.rs:337-339,312-443`、`AccountSection.tsx:999` | 部分已修（账号列表补「切换仅对之后新建/恢复的会话生效」说明行；后端错误结构化 reason 码待做） |
| S-10 | `listAgents()` 失败静默吞错 → Agent 分区整页空白，分不清空态和故障 | `AccountSection.tsx:1030-1032,1093-1094` | 已修（检测中/失败重试/名单空三态区分，不再整页空白） |
| S-11 | 引导 6 步无一步讲装 agent/登录（走完看板还是空的）；宣称有「已归档」tab（实际没有）；reopen 提示只在最后一步（跳过者永远看不到） | `Onboarding.tsx:345-352,402`、`zh.ts:686` | 加「连接你的 AI CLI」一步；订正文案；reopen 提示挪到跳过路径 |
| S-12 | hooks 静默改写 `~/.claude/settings.json` 全程零告知；首启自动导入 7 天历史会话零说明 | `lib.rs:1151-1152`、`watch.rs:793-820` | 引导加「Meowo 如何读到进度」一步（明写备份与移除方式）；导入完成给一次性提示 |
| S-13 | 打开「关于」10s 空窗（delayMs=10000），checking/error 态不渲染（文案已有但零引用）；更新窗无「稍后/跳过此版本/更新日志」，下载不可取消 | `useUpdate.ts:11`、`About.tsx:519`、`Updater.tsx:97-133` | 大部分已修（checking/error 态、unknown 初值、「稍后」按钮、完整更新日志链接；下载取消待做） |
| S-14 | 确认框主按钮通用「确定」不说后果；可逆登出与不可逆删除同用 danger 红（严重度拉平）；confirm 打不开静默当取消 | `ConfirmWindow.tsx:116-119`、`AccountSection.tsx:358-793`、`confirm.tsx:16` | appConfirm 加 confirmLabel；danger 只留删除/合并；catch 上报可见错误 |
| S-15 | 12 个死文案键（含 5.2 需要的 about.checking 和已有样式的 proxy `*Why` 解释）；`usageUnavailable` 在已登录分支说「请确认已登录」自相矛盾 | `zh.ts:513-658` 各处 | 该接的接上，其余删除；错误文案立「发生了什么+能做什么（指向可见控件）」规则 |

### 窗口管理与通知

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| W-1 | 吸附阈值 20 物理像素（150% 缩放下等效 13 逻辑像素），高 DPI 手感随机；条厚度却按逻辑值乘 scale，两处口径不一 | `snap.rs:5,150-152` | 已修（阈值按 scale_factor 换算，与 STRIP_W_LOGICAL 同口径） |
| W-2 | 吸附预览只有窗口自身 4px 发光条，无落点预览 | `App.tsx:705`、`styles.css:1003-1015` | 屏幕边缘画缩略条幽灵轮廓 |
| W-3 | 展开态点一下拖拽条（不移动）即意外折叠（陈旧 lastEdgeRef） | `App.tsx:557-558,462-484` | 已修（松手时位移 < 4px 视为纯点击，不做吸附/还原判定） |
| W-4 | 折叠态启动闪「细条内容装在 360×440 大框」；展开过渡期看板在 28px 宽窗口内布局抖动 | `App.tsx:157-162,663-667`、`useShowWhenReady.ts:20-22` | 部分已修（折叠启动的 show 推迟到 snap_collapse 落地，闪帧消除；展开过渡占位待做） |
| W-5 | 缩略条主轴无上限（60 会话超 1080p 工作区，溢出被裁无提示）；DPI/显示器变化不重算 | `App.tsx:55-57`、`snap.rs:251`、`CollapsedStrip.tsx:47-54` | 上限 + `+N` 徽章；collapsed 态监听 onScaleChanged 重跑 |
| W-6 | pin 状态折叠态不可见不可操作；「找回贴纸」强行永久改写 pin 偏好 | `Sticker.tsx:995-1005`、`App.tsx:415` | 缩略条加 pin 标记；找回改临时置顶 |
| W-7 | 贴纸首显走普通 `show()` 可能抢焦点（`show_window_no_activate` 已有只给对话窗用） | `useShowWhenReady.ts:25`、`window.rs:34-38,437-449` | 改走 no_activate 路径 |
| W-8 | 无点击穿透，opacity 可低至 25%：几乎看不见的置顶窗口 100% 吃鼠标 | `settings.rs:331`（无 set_ignore_cursor_events） | 加穿透模式开关 + 修饰键临时恢复 |
| W-9 | 拖拽期间每个 Moved 全量枚举显示器 ×2 处并 emit | `lib.rs:1040-1063,775-800` | 缓存包围盒，仅显示器事件失效；edge 变化才 emit |
| W-10 | Windows 托盘图标零状态表达（只有 tooltip），macOS 有彩色徽章——可见性差一个数量级；托盘计数把出错混进「待交互」 | `window.rs:634-641`、`watch.rs:611-619` | set_icon 动态合成角标（macOS render_status_rgba 可移植）；拆 error 计数 |
| W-11 | 两平台托盘菜单构成与左键语义不一致（Win 无「打开对话窗」项，macOS 无「找回贴纸」）；Windows 无显示/隐藏开合 | `window.rs:596-664`、`menubar.rs:240-249` | 统一左键=开合主界面，菜单两平台对齐 |
| W-12 | macOS 面板任何失焦即收起，无法常驻（pin 按钮也不渲染） | `panel.rs:65-72`、`Sticker.tsx:995` | 加「保持打开」开关或 ⌥+点击常驻 |
| W-13 | waiting/pending 指纹掺 `last_event_at` 会重复轰炸（blocked 已论证过并排除，结论只用了 1/3）；审批 toast 用默认 5s 时长无按钮 | `watch.rs:277-301,391-416` | 指纹去时间戳；Pending/Blocked 用 Duration::Long + 「打开会话」按钮 |
| W-14 | 通知点击跳转失败静默丢弃返回值（focus_session_terminal 明明有分类结果）；标题不带项目名，多会话时通知中心一排同名 | `watch.rs:411-413,637-739` | 已修（标题带项目名；点击定位失败回退打开对话窗） |
| W-15 | 关「通知」后任务栏仍闪（attention_flash 独立门控但文案不说明）；macOS 通知串行阻塞投递 + 废弃 API | `watch.rs:631-633,766-769`、`macos/notify.rs:30-79` | 门控从属或文案说明；换 UNUserNotificationCenter |
| W-16 | 双屏内侧边也可吸附（鼠标必经之路 + 零延迟展开=必然误触）；非矩形排布包围盒留「死区」窗口可拖进去消失 | `lib.rs:1073-1081,748-799`、`snap.rs:38-53` | 只吸虚拟桌面外侧边；钳位改「相交面积最大的显示器」 |
| W-17 | 位置/尺寸/吸附边/置顶分散四套存储无原子性（localStorage 清空即半吊子状态）；折叠态 min_size 放开是「尺寸毒化」根因 | `lib.rs:897-905`、`App.tsx:24-95`、`snap.rs:288-291` | 收敛进 settings.rs；折叠时禁 resizable 而非放开 min_size |
| W-18 | Linux 分支吸边「看起来支持实则坏掉」（两个原语恒返回假值） | `snap.rs:203-224` | 整体门控 `cfg(windows)` |

### 全局基础设施

| # | 问题 | 位置 | 建议 |
|---|---|---|---|
| G-1 | 点外关闭两套策略：卡片菜单无滚动监听（列表滚走菜单钉在原位指向另一张卡），dd 菜单无 blur 关闭 | `menu.tsx:105-131` vs `CardContextMenu.tsx:57-79` | 部分已修（CardContextMenu 补滚动关闭，两套策略行为对齐；useDismissable 抽取待做） |
| G-2 | 两窗口重命名失焦语义相反：贴纸失焦=丢弃、侧栏失焦=保存（backlog P2-6 只评了代码复用，没评用户可感知语义） | `Sticker.tsx:58-102,654-662` vs `ChatSidebar.tsx:51-68` | 组件可不合并，失焦语义必须统一 |
| G-3 | 确认/弹窗三种壳（原生小窗/页内 modal/行内编辑），页内 modal 无焦点陷阱无 inert（aria-modal 承诺落空） | `ChatWindow.tsx:308-330`、`ConfirmWindow.tsx` | 抽带焦点陷阱的 AppModal 或改行内 |
| G-4 | `card_menu_mode=button` 贴纸常显、侧栏 hover 才显——同一设置两种表现 | `styles.css:825-843` vs `:3234-3239` | button 模式两处都常显 |
| G-5 | 原生 title 残留 17 处与 Tooltip 混用（不止 backlog 认可的 dd-menu 内） | `ChatWindow.tsx:2069`、`Message.tsx:161` 等 | 逐处换 data-tip |
| G-6 | Esc 层级三种让路约定（preventDefault / 捕获 stopPropagation / tagName 白名单）+ Tooltip 第四种；独立窗口 Esc 关闭只实现 1/6 | `menu.tsx:117`、`Message.tsx:76`、`ChatWindow.tsx:1949`、`Tooltip.tsx:74` | 已修（escLayers 层栈落地：菜单/右键菜单/弹层/灯箱/切换器/补全全部注册，窗口级「Esc=拒绝审批」按栈让位；各层自己的截停约定保留为双保险） |
| G-7 | focus ring 仅 1 处自定义，`.slider` 等 outline:none 无替代（WCAG 2.4.7 失败） | `styles.css:2002-2011,2269` | 已修（--dur token 同批；data-im=kbd 下全局 :focus-visible 兜底环 + .slider 放开 outline） |
| G-8 | 动效零 token：13 个时长值、3 个缓动散落，同类元素有的渐变有的瞬变 | `styles.css` 全文 | 已修（--dur-fast/base/slow 三档 token，45 处 transition 收敛；0.4s 长淡入与具名动画保留） |
| G-9 | 用量读数屏文字四种主题组合全部 <4.5:1（最低 3.28）——绕过已校准 token 写死 alpha；7 处文本再叠 opacity 击穿基线（最低 2.81） | `styles.css:441-449,1728,3731` 等 | 已修（用量读数回 --cc-text-dim 保留雕刻 text-shadow；6 处文本 opacity 叠加删除） |
| G-10 | 终端页强制深色的变量覆盖漏整个状态色族（浅色用户切终端页 err/warn 只有 3.3-4:1）；flat 主题逐条硬抄已漏 `.run-mask`（flat+浅色下运行卡片是纯黑块） | `styles.css:3785-3809,2287-2363,597-604` | 已修（--ind-face token：.stk-ind 与 .run-mask 同底色，flat+浅色不再有纯黑块；浅面徽标配色整组压暗一档、文字改白。近似色留视觉验证轮微调） |
| G-11 | 「密度」实为字号缩放且只作用于贴纸窗（设置文案与 token 注释两张皮）；`--sp-*`/`--fs-*` 全定值，46 处 calc 之外的间距不缩放致版式失衡；`.stk-ind`/`.cstrip-*`/`.tip` 等完全不吃密度 | `appearance.ts:78-80`、`styles.css:26,85-105` | 已修（--sp/--fs 阶梯整梯乘 --cc-ui，非贴纸窗恒 1 零回归；.stk-ind/.run-core/.ring-stop/.sdot/.needs-error/.ctx-menu 一并参数化；设置项改名「界面缩放」并注明只作用于贴纸窗。--cc-ui 下发给 chat 窗口仍待做；.cstrip-* 刻意不缩放——细条厚度固定，点放大会溢出） |
| G-12 | 英文约 2.2× 宽无省略保护：`.stab`/`.seg-btn` nowrap 无 overflow 处理，窄窗+英文+大密度直接溢出 | `styles.css:293-309,1918-1937` | 补 ellipsis；`.seg` 允许 shrink |
| G-13 | 零 Intl：24 小时制硬编码、英文月日顺序错、相对时间手拼 | `AccountSection.tsx:64-90`、`helpers.ts:14-21` | 见 U1-19 |
| G-14 | 「本周」实为「近 7 天」，与日历应用语义不同 | `ChatSidebar.tsx:95-110` | 文案改「近 7 天」（成本最低） |
| G-15 | 悬停是若干功能唯一入口且无 hover:none 兜底（终端操作条、自绘滚动条）；Tooltip 无触摸/长按路径而它承载完整路径、账号全名等唯一信息 | `styles.css:4026-4029,478-490`、`Tooltip.tsx:77-79` | 已修（终端操作条 focus-within/hover:none + TooltipLayer 触屏长按 500ms 显示、抬手保留 1.2s） |
| G-16 | 每个会话状态点都是 `role="status"` live region（N 条会话 = N 个播报源，状态秒级跳变播报风暴）；审批卡 `role="alert"` 包裹交互按钮打断朗读且焦点不入 | `ChatSidebar.tsx:663,794` 等、`ApprovalCard.tsx:22` | 状态点改 `role="img"` 只留 1 个播报；审批卡改 alertdialog + 移焦 |
| G-17 | 状态三色灰度值几乎相同（159/160/169），waiting↔pending 对比 1.13:1，reduced-motion 下动效通道也没了——无第三通道 | `styles.css:566-591,3207-3212` | 大部分已修（形状语义落地：实心=在跑、空心环=等你说话、方块=要你决策——侧栏点/标题栏徽标/缩略条/卡片在线点四处统一；RunBadge 已有 pill 文字+流动差异不再加形状。视觉需真机复核） |
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
8. **PTY 事件解耦**（T-14 附带）：`pty-output`/`pty-exit` 从硬编码 `label=="chat"` 改
   app 级 emit + sessionId 过滤（前端已在过滤），解除窗口 label 耦合。

## 做得好的（保留，勿在重构中弄丢）

- 恢复时权限改选合并写回会话存档并自动沿用（`terminal.rs:1410-1515`）——新建面板应复用此模式（U1-15）。
- 后台会话的错误建模：第一次按键领回对话页、退出码与「没接上」严格区分（`ManagedTerminal.tsx:295-300,771-786`）。
- 账号用量「缓存先显示再刷新 + 500ms 最短 spinner + 失败保留旧值标注缓存」（`AccountSection.tsx:1062-1084`）。
- 确认窗默认焦点在取消、Esc 取消、活跃账号可删且回落默认（`ConfirmWindow.tsx:100-116`）。
- i18n 基础设施（`Dict = typeof zh` 编译期对齐、subscribe-first、`<html lang>` 跟随）。
- 引导内设置真·即时生效、步骤点可跳（`Onboarding.tsx:250-321,409-416`）。

## 工程侧勘误（顺手改）

- consistency-backlog P0-2 状态应改「已修」：四处已全部走 `appConfirm`，全仓已无 plugin-dialog confirm 用法。
