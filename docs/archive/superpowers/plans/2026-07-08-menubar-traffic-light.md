# 菜单栏红绿灯状态图标 Implementation Plan

> ⚠️ **已被实现阶段的设计演进取代（历史文档，勿据此实现）。** 本计划成文时的方案（橙=运行的彩色**圆点+数字**、数字随系统明暗**反色**、`defaults` 探测暗色）在实现中已改为
> **数字内嵌彩色圆/胶囊徽章**：运行=绿 `#34d399`、待交互=黄 `#fbbf24`，数字用**固定深色墨**（不随明暗，故**无** `system_is_dark`/明暗自适应）。
> 以最终 spec [`../specs/2026-07-08-menubar-traffic-light-design.md`](../specs/2026-07-08-menubar-traffic-light-design.md) 与 `app/src-tauri/src/macos/menubar.rs` 的实现为准；下方任务描述保留作演进记录。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 macOS 菜单栏状态图标从单色 `↻N ✋M` 模板字形改成彩色实心点+数字（橙=运行、黄=待交互），空闲回落 app logo。

**Architecture:** 只改 `app/src-tauri/src/macos/menubar.rs` 的渲染层与 `lib.rs` 的托盘刷新循环。渲染改为运行时输出全彩 RGBA（非模板）：`↻/✋` 符号槽换成 Rust 直接光栅化的彩色圆点，数字复用现有图集 alpha 字形但按菜单栏明暗着色（暗白/亮黑）。明暗用 `defaults read -g AppleInterfaceStyle` 在既有 5s 循环里探测，无新依赖、无 objc 观察者，主题切换 ≤5s 生效。

**Tech Stack:** Rust / Tauri v2（`tray.set_icon_with_as_template`）、既有字形图集 `icons/menubar-glyphs.bin`。

## Global Constraints

- 平台：仅 macOS（`#[cfg(target_os = "macos")]`）；不动 Windows 托盘。
- 不新增 crate 依赖（不引入 objc/cocoa）；明暗探测用 `std::process::Command` 调 `defaults`。
- 状态色（sRGB，取自应用 CSS）：运行橙 `#d97757` = `[0xd9,0x77,0x57]`；待交互黄 `#fbbf24` = `[0xfb,0xbf,0x24]`。
- 彩色状态图标用非模板（`as_template = false`）；空闲回落 logo 仍用模板（`as_template = true`）。
- 保留 `icons/menubar-glyphs.bin` 不动（继续用其 0-9 数字字形，弃用下标 10/11 的 ↻/✋）。
- 纯函数 `status_seq` 的现有单测（`status_seq_groups_and_gaps`）必须继续通过——本计划不改其行为。

---

### Task 1: 彩色点+数字渲染

**Files:**
- Modify: `app/src-tauri/src/macos/menubar.rs`（`render_status_rgba`、`update_tray_status`，新增 `DOT_W`/`DOT_D`/`RUN_RGB`/`WAIT_RGB`/`slot_w`/`blit_dot`）
- Test: `app/src-tauri/src/macos/menubar.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: 既有 `status_seq(running, waiting) -> Vec<(usize, usize)>`、`glyph_offset`、常量 `GLYPH_H`/`GLYPH_W`/`GLYPH_RUN`/`GLYPH_WAIT`/`GLYPH_ATLAS`/`MENUBAR_ICON_RGBA`/`MENUBAR_ICON_SIZE` 保持不变。
- Produces:
  - `fn render_status_rgba(running: usize, waiting: usize, dark: bool) -> Option<(Vec<u8>, u32, u32)>`（签名新增 `dark`）
  - `pub fn update_tray_status(app: &AppHandle, running: usize, waiting: usize, dark: bool)`（签名新增 `dark`）

- [ ] **Step 1: 写失败测试**

在 `mod tests` 内，把现有 `render_status_none_when_idle_else_sized` 的两处调用补上 `dark` 实参，并新增一个颜色/明暗测试：

```rust
    #[test]
    fn render_status_none_when_idle_else_sized() {
        assert!(render_status_rgba(0, 0, true).is_none());
        let (rgba, w, h) = render_status_rgba(3, 2, true).unwrap();
        assert_eq!(h, GLYPH_H as u32);
        assert_eq!(rgba.len(), (w as usize) * GLYPH_H * 4);
        assert_eq!(GLYPH_ATLAS.len(), GLYPH_W.iter().map(|w| GLYPH_H * w).sum::<usize>());
    }

    // 扫描：是否存在一个「不透明且 RGB≈target」的像素（容差 8）。
    fn has_opaque_rgb(rgba: &[u8], target: [u8; 3]) -> bool {
        rgba.chunks_exact(4).any(|p| {
            p[3] > 200
                && (p[0] as i32 - target[0] as i32).abs() <= 8
                && (p[1] as i32 - target[1] as i32).abs() <= 8
                && (p[2] as i32 - target[2] as i32).abs() <= 8
        })
    }

    #[test]
    fn render_colors_dot_and_adapts_digits_to_appearance() {
        // 运行 2：应含橙色圆点像素；暗栏数字偏白。
        let (dark_rgba, _, _) = render_status_rgba(2, 0, true).unwrap();
        assert!(has_opaque_rgb(&dark_rgba, RUN_RGB), "缺橙色圆点");
        assert!(has_opaque_rgb(&dark_rgba, [255, 255, 255]), "暗栏数字应为白");
        // 亮栏数字偏黑。
        let (light_rgba, _, _) = render_status_rgba(2, 0, false).unwrap();
        assert!(has_opaque_rgb(&light_rgba, [0, 0, 0]), "亮栏数字应为黑");
        // 待交互 3：应含黄色圆点像素。
        let (wait_rgba, _, _) = render_status_rgba(0, 3, true).unwrap();
        assert!(has_opaque_rgb(&wait_rgba, WAIT_RGB), "缺黄色圆点");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p cc-app --lib menubar 2>&1 | tail -20`
Expected: 编译失败（`render_status_rgba` 参数个数不匹配 / `RUN_RGB` 未定义），或断言失败。

- [ ] **Step 3: 写实现**

在 `menubar.rs` 常量区（`PAIR_GAP` 之后）新增：

```rust
const DOT_W: usize = 16; // 圆点占位槽宽（替代原 ↻/✋ 符号槽）
const DOT_D: f32 = 14.0; // 圆点直径（略小于槽宽，留边）
const RUN_RGB: [u8; 3] = [0xd9, 0x77, 0x57]; // 运行 橙 #d97757
const WAIT_RGB: [u8; 3] = [0xfb, 0xbf, 0x24]; // 待交互 黄 #fbbf24

/// 槽宽：运行/待交互标记用固定圆点槽 DOT_W，其余（数字）用图集字形宽。
fn slot_w(idx: usize) -> usize {
    if idx == GLYPH_RUN || idx == GLYPH_WAIT { DOT_W } else { GLYPH_W[idx] }
}

/// 在 rgba 的 [x0, x0+DOT_W) 槽内画一个竖向居中的实心抗锯齿圆，颜色 rgb。
fn blit_dot(rgba: &mut [u8], total: usize, x0: usize, rgb: [u8; 3]) {
    let cx = x0 as f32 + DOT_W as f32 / 2.0;
    let cy = GLYPH_H as f32 / 2.0;
    let r = DOT_D / 2.0;
    for y in 0..GLYPH_H {
        for c in 0..DOT_W {
            let px = x0 + c;
            let dx = px as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            // 边缘 1px 抗锯齿：dist≤r-0.5 全实，≥r+0.5 全透，其间线性。
            let a = (r + 0.5 - dist).clamp(0.0, 1.0);
            if a > 0.0 {
                let i = (y * total + px) * 4;
                rgba[i] = rgb[0];
                rgba[i + 1] = rgb[1];
                rgba[i + 2] = rgb[2];
                rgba[i + 3] = (a * 255.0) as u8;
            }
        }
    }
}
```

把 `render_status_rgba` 整体替换为（新增 `dark` 参数、彩色输出）：

```rust
/// 把 (运行, 待办) 拼成菜单栏**彩色** RGBA：运行/待交互各一个彩色圆点 + 数字。
/// 数字按菜单栏明暗着色（dark → 白，否则黑）；圆点颜色固定。全零返回 None（回落 logo）。
fn render_status_rgba(running: usize, waiting: usize, dark: bool) -> Option<(Vec<u8>, u32, u32)> {
    let seq = status_seq(running, waiting);
    if seq.is_empty() {
        return None;
    }
    let total: usize = seq.iter().map(|(idx, gap)| gap + slot_w(*idx)).sum();
    let mut rgba = vec![0u8; total * GLYPH_H * 4];
    let fg: u8 = if dark { 255 } else { 0 };
    let mut x = 0;
    for (idx, gap) in seq {
        x += gap;
        if idx == GLYPH_RUN || idx == GLYPH_WAIT {
            let rgb = if idx == GLYPH_RUN { RUN_RGB } else { WAIT_RGB };
            blit_dot(&mut rgba, total, x, rgb);
        } else {
            let off = glyph_offset(idx);
            let w = GLYPH_W[idx];
            for y in 0..GLYPH_H {
                for c in 0..w {
                    let a = GLYPH_ATLAS[off + y * w + c];
                    if a > 0 {
                        let i = (y * total + x + c) * 4;
                        rgba[i] = fg;
                        rgba[i + 1] = fg;
                        rgba[i + 2] = fg;
                        rgba[i + 3] = a;
                    }
                }
            }
        }
        x += slot_w(idx);
    }
    Some((rgba, total as u32, GLYPH_H as u32))
}
```

把 `update_tray_status` 改为（新增 `dark`，彩色用非模板、logo 用模板）：

```rust
/// 按 (运行, 待办) 更新菜单栏图标：有计数 → 彩色点+数字（非模板）；全零 → 回落单色 logo（模板）。
pub fn update_tray_status(app: &AppHandle, running: usize, waiting: usize, dark: bool) {
    let Some(tray) = app.tray_by_id("cc-kanban-tray") else {
        return;
    };
    match render_status_rgba(running, waiting, dark) {
        Some((rgba, w, h)) => {
            let _ = tray.set_icon_with_as_template(Some(tauri::image::Image::new(&rgba, w, h)), false);
        }
        None => {
            let logo = tauri::image::Image::new(MENUBAR_ICON_RGBA, MENUBAR_ICON_SIZE, MENUBAR_ICON_SIZE);
            let _ = tray.set_icon_with_as_template(Some(logo), true);
        }
    }
    let _ = tray.set_title(None::<&str>);
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p cc-app --lib menubar 2>&1 | tail -20`
Expected: `status_seq_groups_and_gaps`、`render_status_none_when_idle_else_sized`、`render_colors_dot_and_adapts_digits_to_appearance` 全 PASS。

- [ ] **Step 5: 提交**

```bash
git add app/src-tauri/src/macos/menubar.rs
git commit -m "feat(menubar): 状态图标改彩色点+数字（橙=运行/黄=待交互），数字随明暗着色"
```

---

### Task 2: 系统明暗探测 + 循环接线

**Files:**
- Modify: `app/src-tauri/src/macos/menubar.rs`（新增 `system_is_dark`）
- Modify: `app/src-tauri/src/lib.rs`（`last_tray` 声明处 :1542 附近；macOS 托盘更新分支 :1685 附近）
- Test: `app/src-tauri/src/macos/menubar.rs`

**Interfaces:**
- Consumes: Task 1 的 `update_tray_status(app, running, waiting, dark)`。
- Produces: `pub fn system_is_dark() -> bool`。

- [ ] **Step 1: 写测试（探测函数不崩、返回布尔）**

在 `mod tests` 内新增：

```rust
    #[test]
    fn system_is_dark_returns_without_panicking() {
        // 仅验证不 panic、返回一个布尔（CI/无 defaults 时应回落 false，不报错）。
        let _ = system_is_dark();
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p cc-app --lib system_is_dark 2>&1 | tail -10`
Expected: 编译失败（`system_is_dark` 未定义）。

- [ ] **Step 3: 实现 `system_is_dark`**

在 `menubar.rs`（`update_tray_status` 之后）新增：

```rust
/// 读系统菜单栏明暗：Dark 模式 → true。用 `defaults`，无需引入 objc 依赖；
/// 读失败/未设置（即浅色）→ false。在既有 5s 托盘循环里调用，主题切换 ≤5s 生效。
pub fn system_is_dark() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "Dark")
        .unwrap_or(false)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p cc-app --lib system_is_dark 2>&1 | tail -10`
Expected: PASS。

- [ ] **Step 5: 接线到 5s 循环**

在 `lib.rs` `last_tray` 声明（约 :1542）**之后**加一行 macOS 专用的上次明暗记录：

```rust
        let mut last_tray: Option<(usize, usize)> = None;
        #[cfg(target_os = "macos")]
        let mut last_dark: Option<bool> = None;
```

把 macOS 托盘更新分支（约 :1685-1687，`update_tray_status` 那三行）替换为：

```rust
                #[cfg(target_os = "macos")]
                {
                    let dark = crate::macos::menubar::system_is_dark();
                    if last_tray != Some((tray_running, tray_waiting)) || last_dark != Some(dark) {
                        crate::macos::menubar::update_tray_status(&app, tray_running, tray_waiting, dark);
                        last_tray = Some((tray_running, tray_waiting));
                        last_dark = Some(dark);
                    }
                }
```

（Windows 分支 :1691 与 non-macos :1696 不动。）

- [ ] **Step 6: 编译确认无 warning/error**

Run: `cargo build -p cc-app 2>&1 | tail -15`
Expected: 编译通过；无 `unused variable: last_dark` 等告警。

- [ ] **Step 7: 提交**

```bash
git add app/src-tauri/src/macos/menubar.rs app/src-tauri/src/lib.rs
git commit -m "feat(menubar): 5s 循环探测系统明暗并驱动状态图标数字反色"
```

---

### Task 3: 真机目视验收

**Files:** 无（仅运行验证）

- [ ] **Step 1: 起 dev**

Run: `cd app && bun run tauri dev`（后台）。等编译完成、`cc-app` 运行。

- [ ] **Step 2: 制造「运行中/待交互」态**

用与 PR 验证同法或真实会话，让菜单栏出现非空闲计数；截图菜单栏图标，确认：橙点跟运行数、黄点跟待交互数、数字清晰。

- [ ] **Step 3: 明暗切换验收**

系统设置切 Dark↔Light（或用 `defaults write -g AppleInterfaceStyle -string Dark` / `defaults delete -g AppleInterfaceStyle` 后重开菜单栏）；≤5s 内数字应从白↔黑反色，点色不变。截图两态。

- [ ] **Step 4: 空闲回落验收**

计数归零后确认菜单栏回落单色三柱 logo 且随明暗自动反色。

- [ ] **Step 5: 无需提交（纯验收）。若发现偏差，回到 Task 1/2 调 `DOT_W`/`DOT_D`/间隔常量。**

---

## Self-Review

- **Spec coverage:** 彩色点+数字(T1)、橙/黄色值(T1 常量)、空闲回落 logo(T1 update_tray_status None 分支)、明暗自适应数字(T1 fg + T2 探测/接线)、只改 macOS/不动 Windows(Global Constraints + T2 仅改 macOS 分支)、复用数字字形/弃用 ↻✋(T1 slot_w + render 分支)、纯函数可测(T1/T2 tests) —— 均有对应任务。
- **Placeholder scan:** 无 TBD/TODO；每个代码步骤含完整代码。
- **Type consistency:** `render_status_rgba(_, _, dark: bool)`、`update_tray_status(_, _, _, dark: bool)`、`system_is_dark() -> bool`、`slot_w(usize)->usize`、`blit_dot(&mut [u8], usize, usize, [u8;3])`、常量 `RUN_RGB`/`WAIT_RGB` 在 T1 定义、T2 循环调用签名一致。
