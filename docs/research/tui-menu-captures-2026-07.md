# TUI 菜单真机取证(2026-07-23)

弹窗识别的文法必须以真机取证为据(声明前提,见 `registry.rs::selector_anchors` 的注释)。
本轮用 `app/src-tauri/tests/capture_model_menu.rs` 对三家做了 PTY 取证,工具本身也修了两处:

- **命令与回车必须分两次写**(间隔 400ms):合并写会被 TUI 当粘贴块,`\r` 只换行不提交
  (probe_enter.rs 的既有结论,gemini 上复现——`/model` 留在 composer 里)。
- 新增环境变量:`MEOWO_CAPTURE_MENU_CMD`(菜单命令,opencode 用 `/models`)、
  `MEOWO_CAPTURE_PROBE_KEYS=1`(菜单开着时追加 ↓、Enter 各一拍,取证交互语义)、
  `MEOWO_CAPTURE_SUBMIT`(cr|lf|crlf|kitty|keypad,提交键矩阵)、
  `MEOWO_CAPTURE_BOOT_QUIET_MS` / `MEOWO_CAPTURE_BOOT_TIMEOUT_MS`(启动静默阈值/总时限,
  codex 的更新器需要放宽)。

用法:
```
MEOWO_CAPTURE_EXE=<cli 路径> [MEOWO_CAPTURE_MENU_CMD=/models] [MEOWO_CAPTURE_PROBE_KEYS=1] \
  cargo test -p meowo-app --test capture_model_menu capture_model_menu -- --ignored --nocapture
```

## gemini(gemini-cli 0.51.0)——取证完整,已接线

`/model` 存在(此前存疑:它不在自家 slash 命令研究表里,实测有效)。对话框形态:

```
╭──────────────────────────────╮
│ Select Model│
│ ● 1. Auto│
│      Let Gemini CLI decide the best model for the task: …│
│   2. Manual│
│      Manually select a model│
│ Remember model for future sessions: false (Press Tab to toggle)│
│ (Press Esc to close)│
╰──────────────────────────────╯
```

- **全框线包裹**:内容行首尾都是 `│`;**编号项** `1.`;**焦点标记 `●`**(非 `❯`);
  **没有**任何 "enter to select"/"↑↓" 导航提示行;Esc 关闭;Tab 切换记忆开关。
- 交互语义(按键探针证实):`↓` 把 `●` 移到下一项;`Enter` 确认焦点项
  (选 Manual 后展开 7 个模型的同款子菜单,`●` 复位到第 1 项)。
- 已据此实现 `detectFramedNumberedMenu`(terminalAttention.ts),仅 expectMenu 窗口启用;
  子菜单是同款形态,选完模型的第二跳自动获得同样的卡片。
- 另获事实:gemini 状态栏有 "Shift+Tab to accept edits"——它有模式循环键,
  但 screen marker 文案未取证,mode_controls 暂未声明。

## codex——自动更新 + 本机网络双重拦路,未取证

每次启动都强制先跑 `Updating Codex via powershell … install.ps1`;该下载在本机**直连
失败**(Invoke-RestMethod WebException——更新器不吃系统代理),于是每次启动都卡死在
更新上,TUI 永远不出现。`codex update` 手动跑同样失败。放宽探针静默阈值到 30s
(MEOWO_CAPTURE_BOOT_QUIET_MS)也无济于事:更新期间零输出。
**结论**:等 codex 修好代理支持/用户网络可直连时重跑取证;在此之前不接线。
副产物:更新前的首屏证实其 composer 光标字形是 `›`(U+203A),`›` 已收进字形表。

## opencode(1.17.20)——TUI 输出正常,输入在裸 ConPTY 里整条死路

全屏 TUI 正常启动(composer "Ask anything…",状态栏 "tab agents / ctrl+p commands"),
但写入的任何按键**连回显都没有**:`/models` 分别配 `\r`、`\n`、`\r\n`、kitty `\x1b[13u`
提交全部无反应,命令前预送 focus-in(`\x1b[I`)也无效——TUI 对输入管道完全无感。
疑因:opencode 的 Go TUI 在 Windows 上走控制台输入 API(ReadConsoleInput)的路径与
裸 ConPTY 输入管道的翻译不合拍。**下一步**:单独调查 meowo 生产路径(PtyBroker +
xterm)里 opencode 的输入是否可用——若同样不可用,这是比菜单识别更基础的兼容缺陷;
若可用,差异点(环境变量/终端模式握手)就是取证探针要补的东西。在此之前不接线。

## 结论摘要

| agent | 菜单命令 | 形态证据 | 交互证据 | 识别接线 |
|---|---|---|---|---|
| claude | `/model`(预设内联) | 测试夹具齐全 | 齐全 | ✅ 数字选择器/审批/信任 |
| kimi | `/model` | 真机 capture | probe_enter | ✅ 光标菜单 |
| gemini | `/model` | 本轮真机 | 本轮真机(↓/Enter) | ✅ 框线数字菜单(本轮) |
| codex | `/model` | 无(更新器+网络拦路) | 无 | ❌ 等环境恢复后重跑 |
| opencode | `/models` | 无(输入通道死,按键矩阵已穷举) | 无 | ❌ 先查生产路径输入是否可用 |

## kimi 审批面板(2026-07-24,源码取证)——已接线

**取证途径说明**:kimi.exe 的 launcher shim 在裸 ConPTY 下即退(见 kimi/mod.rs
`interrupt_input` 注释),`capture_approval_panel` 自动探针抓不到画面。本轮改用
**官方开源仓库源码取证**(MoonshotAI/kimi-code @ 0.29,
`apps/kimi-code/src/tui/components/dialogs/approval-panel.ts` + `reverse-rpc/approval/adapter.ts`),
面板形态与按键语义均为源码确证:

```
────────────────────────────────────────
  ▶ Run this command?            ← 标题按工具定制:Bash=Run this command? /
                                    Write=Write this file? / Edit=Apply these edits? /
                                    TaskStop=Stop this task? / ExitPlanMode=Ready to build with this plan? /
                                    其他=Approve <tool>?
  $ echo hello                   ← display 块(shell 前缀 $;diff/文件另有渲染)
  ▶ 1. Approve once              ← 选中项前缀 ▶(非 ❯)
    2. Approve for this session
    3. Reject
    4. Reject with feedback      ← requires_feedback:选中后进入行内输入,不直接提交
  ↑/↓ select · 1/2/3/4 choose · ↵ confirm   ← 稳定签名行
────────────────────────────────────────
```

- **数字键直接选中并提交**(`selectAndSubmit`),GUI 按钮只需打 `"1"`/`"2"`/`"3"`,
  无需光标相对移动;↑/↓ 循环导航,Enter 提交,Esc/Ctrl-C/Ctrl-D = 拒绝。
- 默认选项四档(ExitPlanMode/plan_review 为 Approve…+Reject+Revise;goal_start 另有一套);
  「Reject with feedback」「Revise」需要现场输入反馈,GUI 卡片不收,想写反馈去终端。
- kimi 的 PermissionRequest hook 是 observation-only(官方文档,5s 超时),broker
  审批桥对它保持关闭;已据此实现 `kimi:command-approval`(terminalAttention.ts)
  屏幕识别兜底,provider 门控,ChatWindow 复用命令审批卡渲染。
