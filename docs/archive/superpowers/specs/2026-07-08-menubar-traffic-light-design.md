# macOS 菜单栏状态徽章

## 背景与动机

macOS 菜单栏原先的状态图标由 `app/src-tauri/src/macos/menubar.rs` 的 `render_status_rgba`
把预渲染字形图集（`icons/menubar-glyphs.bin`）拼成一张 **单色模板图**：
`↻N`（`arrow.triangle.2.circlepath` + 运行数）后跟 `✋M`（`hand.raised.fill` + 待交互数）。
用户反馈这套符号在小尺寸下观感差，最终确定改成 **数字内嵌在彩色徽章里**
（像未读消息角标：绘制形状，不用 emoji）。

## 目标

- 菜单栏图标改为 **彩色圆/胶囊徽章 + 内嵌深色数字**，颜色沿用应用自身状态色语言：
  - 运行中：绿 `#34d399`（同 `.dot-run`/`.dot-active`；注意橙 `#d97757` 是"转圈扫光"动画色，非状态点色）
  - 待交互：黄 `#fbbf24`（同 `.dot-wait`）
- 单位数 → 正圆徽章；两位数 / `99+` → 胶囊（圆角矩形，随内容变长）。
- 数字/加号用固定深色墨 `#1a1a1b`（在绿、黄上都有足够对比），运行组徽章在前、待交互组在后。
- 计数最多两位；超过 99 显示 `99+`。
- 全空闲（运行 0、待交互 0）时回落单色三柱 app logo（模板图，随明暗自动反色）。

## 非目标（YAGNI）

- 不改 Windows 托盘（Windows 仍是静态 logo + 悬浮提示文案）。
- 不改右键菜单、不改点击行为、不改计数来源（`tray_running` / `tray_waiting` 照旧）。
- **不做菜单栏明暗自适应**：数字色相对彩色徽章固定，与系统 Dark/Light 无关，故无需探测系统外观。

## 视觉规格

画布定高 `GLYPH_H = 36`（Retina 下由系统缩放到菜单栏高度）：

| 元素 | 规格（常量） |
|---|---|
| 徽章高 | `H_BADGE = 34`（画布内上下各留 1px） |
| 圆角 | 半高（`= H_BADGE/2`）→ 单字符即正圆，多字符成胶囊 |
| 徽章内水平内边距 | `PAD_X = 7` |
| 徽章内数字/加号间隔 | `INNER_GAP = 1` |
| 两徽章之间 | `BADGE_GAP = 6` |
| 数字字形 | 复用图集内 SF `0-9` alpha 字形，以深色墨 `INK` 叠加到徽章上 |
| 计数上限 | 最多两位；超过 99 追加合成「+」字形（`PLUS_W=15` / `PLUS_ARM=7.5` / `PLUS_TH=3.0`） |

- 徽章外宽 `badge_w = max(H_BADGE, 内容宽 + 2*PAD_X)`——单字符内容窄，落到最小值 `H_BADGE` 即正圆。
- 只有一种状态非零时只画那一组；两者都为零时回落 logo。

## 实现要点（altitude：函数级，落在 menubar.rs）

1. **纯逻辑保留**：`status_seq(running, waiting)` 仍产出「组标记 + 数字 + 可选 `+`」的扁平序列
   （含 99+ 封顶，`push_pair` 里 `n.min(99)` + 超 99 追加 `GLYPH_PLUS`），便于单测。
2. **渲染** `render_status_rgba(running, waiting) -> Option<(rgba, w, h)>`（无 `dark` 参数）：
   - 用 `status_seq` 切成 `[(色标记, 字形们)]` 组（徽章自己排版，忽略序列里的 gap）。
   - `fill_round_rect`：Rust 光栅化圆角胶囊（1px 软边抗锯齿），填运行绿 / 待交互黄。
   - `blit_glyph` / `blit_plus`：把数字字形、合成「+」以深色墨 `INK` **叠加**到徽章上（居中）。
   - `↻`/`✋` 符号字形（图集下标 10/11）不再使用（图集文件保留不动）。
3. **入口** `update_tray_status(app, running, waiting)`：非空闲 → 彩色徽章 `as_template=false`；
   空闲 → logo `as_template=true`。5s 托盘循环里按 `(running, waiting)` 变化调用（`lib.rs`）。

## 测试

- **纯逻辑单测**：`status_seq` 组序列（只运行 / 只待交互 / 两者 / 全零→空）、99+ 封顶。
- **几何单测**：`badge_w` 单字符 = `H_BADGE`（正圆）、两位数/`99+` > `H_BADGE`（胶囊）。
- **像素单测**：`render_status_rgba` 输出宽高自洽；运行含绿填充 + 深墨、待交互含黄填充。
- **真机目视**：单位数正圆徽章（已确认绿圆「2」）；多位/`99+` 胶囊（渲染 dump 确认绿圆「5」+ 黄胶囊「99+」）；计数归零回落 logo。

## 风险 / 待确认

- **深墨在黄底对比**：`#1a1a1b` 于 `#fbbf24` 对比充足；真机与 dump 均已目视确认可读。
- **数字字形垂直居中**：图集字形定高 36、墨迹居中约 26px，落在 34px 徽章内不溢出。
