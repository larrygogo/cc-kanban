use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use crate::macos::panel;

/// 菜单栏空闲图标（全空闲时回落）：单色 36×36 条形图，黑+alpha 掩码、其余透明，照真实 app logo 比例复刻。
/// macOS 按菜单栏明暗自动反色。原始 RGBA（行优先、上到下），不走 PNG 解码以免引入 image-png feature。
const MENUBAR_ICON_RGBA: &[u8] = include_bytes!("../../icons/menubar-template.rgba");
const MENUBAR_ICON_SIZE: u32 = 36;

// ── 状态图标：运行/待办的 SF Symbol + 数字，拼成动态单色模板图 ──
// 离线（Swift）把数字 0-9 + arrow.triangle.2.circlepath(运行) + hand.raised.fill(待办)
// 渲染成定高 36 的 alpha 掩码并拼接成图集；运行时按计数切片拼接，set_icon 动态更新。
const GLYPH_H: usize = 36;
/// 各字形宽度，顺序：0-9、运行、待办。须与 menubar-glyphs.bin 的生成顺序一致。
const GLYPH_W: [usize; 12] = [15, 9, 14, 14, 15, 14, 15, 13, 15, 15, 31, 21];
const GLYPH_RUN: usize = 10;
const GLYPH_WAIT: usize = 11;
/// 定高 alpha 图集（按 GLYPH_W 顺序拼接，每字形 GLYPH_H*W 字节）。
const GLYPH_ATLAS: &[u8] = include_bytes!("../../icons/menubar-glyphs.bin");

const SYM_NUM_GAP: usize = 5; // 图标与其数字之间（留一点呼吸间隔）
const DIGIT_GAP: usize = 1; // 数字之间
const PAIR_GAP: usize = 16; // 运行组与待办组之间（两个状态间留明显间隔）

// ── 状态徽章：彩色圆/胶囊 + 内嵌深色数字（单字→正圆，多字/99+→胶囊）──
const H_BADGE: f32 = 34.0; // 徽章高（画布 GLYPH_H=36，上下各留 1px）
const BADGE_GAP: usize = 10; // 运行徽章与待交互徽章之间
const PAD_X: usize = 7; // 徽章内水平内边距（内容两侧）
const INNER_GAP: usize = 1; // 徽章内数字/加号之间
const INK: [u8; 3] = [0x1a, 0x1a, 0x1b]; // 徽章内数字/加号 深色墨（在绿/黄上都够对比）
const RUN_RGB: [u8; 3] = [0x34, 0xd3, 0x99]; // 运行 绿 #34d399（同 .dot-run/.dot-active）
const WAIT_RGB: [u8; 3] = [0xfb, 0xbf, 0x24]; // 待交互 黄 #fbbf24（同 .dot-wait）

const GLYPH_PLUS: usize = 12; // 合成「+」字形（不在图集，运行时画）；数字超 99 显示 99+
const PLUS_W: usize = 15; // 「+」占位宽
const PLUS_ARM: f32 = 7.5; // 「+」臂半长（中心到端点）
const PLUS_TH: f32 = 3.0; // 「+」笔画半宽

/// 徽章内容宽（数字 + 可选「+」，含内部间隔）。
fn content_w(glyphs: &[usize]) -> usize {
    glyphs
        .iter()
        .enumerate()
        .map(|(i, &g)| {
            (if i > 0 { INNER_GAP } else { 0 }) + if g == GLYPH_PLUS { PLUS_W } else { GLYPH_W[g] }
        })
        .sum()
}

/// 徽章外宽：内容 + 两侧内边距，最小为 H_BADGE（单字符即成正圆）。
fn badge_w(glyphs: &[usize]) -> usize {
    (content_w(glyphs) + 2 * PAD_X).max(H_BADGE as usize)
}

/// 在 [x0, x0+w) 内画一个竖向居中的圆角胶囊（圆角=高/2；w==高时即正圆），填充 color，1px 软边抗锯齿。
fn fill_round_rect(rgba: &mut [u8], total: usize, x0: usize, w: usize, color: [u8; 3]) {
    let cx = x0 as f32 + w as f32 / 2.0;
    let cy = GLYPH_H as f32 / 2.0;
    let hw = w as f32 / 2.0;
    let hh = H_BADGE / 2.0;
    let r = hh; // 圆角半径 = 半高
    for y in 0..GLYPH_H {
        for c in 0..w {
            let px = x0 + c;
            let dx = ((px as f32 + 0.5 - cx).abs() - (hw - r)).max(0.0);
            let dy = ((y as f32 + 0.5 - cy).abs() - (hh - r)).max(0.0);
            let dist = (dx * dx + dy * dy).sqrt() - r;
            let a = (0.5 - dist).clamp(0.0, 1.0);
            if a > 0.0 {
                let i = (y * total + px) * 4;
                rgba[i] = color[0];
                rgba[i + 1] = color[1];
                rgba[i + 2] = color[2];
                rgba[i + 3] = (a * 255.0) as u8;
            }
        }
    }
}

/// straight-alpha 的 source-over：把不透明色 rgb（覆盖度 src_a∈[0,1]）叠加到 rgba[i..i+4] 上。
/// out_a = src_a + dst_a·(1-src_a)；RGB 按输出 alpha 归一，避免徽章半透明边缘处 max/预乘导致发灰。
fn blend_over(rgba: &mut [u8], i: usize, rgb: [u8; 3], src_a: f32) {
    let dst_a = rgba[i + 3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    for k in 0..3 {
        let v = (rgb[k] as f32 * src_a + rgba[i + k] as f32 * dst_a * (1.0 - src_a)) / out_a;
        rgba[i + k] = v.round().clamp(0.0, 255.0) as u8;
    }
    rgba[i + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// 把一个数字字形（图集 alpha）以 rgb 墨色叠加到已画的徽章上（居中于定高 36 的格）。
fn blit_glyph(rgba: &mut [u8], total: usize, x0: usize, idx: usize, rgb: [u8; 3]) {
    let off = glyph_offset(idx);
    let w = GLYPH_W[idx];
    for y in 0..GLYPH_H {
        for c in 0..w {
            let ga = GLYPH_ATLAS[off + y * w + c] as f32 / 255.0;
            if ga > 0.0 {
                blend_over(rgba, (y * total + x0 + c) * 4, rgb, ga);
            }
        }
    }
}

/// 画一个竖向居中的「+」（rgb 墨色，叠加到徽章上），横竖臂各 0.5px 软边抗锯齿。
fn blit_plus(rgba: &mut [u8], total: usize, x0: usize, rgb: [u8; 3]) {
    let cx = x0 as f32 + PLUS_W as f32 / 2.0;
    let cy = GLYPH_H as f32 / 2.0;
    for y in 0..GLYPH_H {
        for c in 0..PLUS_W {
            let px = x0 + c;
            let dx = (px as f32 + 0.5 - cx).abs();
            let dy = (y as f32 + 0.5 - cy).abs();
            let cov_h =
                (PLUS_ARM + 0.5 - dx).clamp(0.0, 1.0) * (PLUS_TH + 0.5 - dy).clamp(0.0, 1.0);
            let cov_v =
                (PLUS_TH + 0.5 - dx).clamp(0.0, 1.0) * (PLUS_ARM + 0.5 - dy).clamp(0.0, 1.0);
            let a = cov_h.max(cov_v);
            if a > 0.0 {
                blend_over(rgba, (y * total + px) * 4, rgb, a);
            }
        }
    }
}

fn glyph_offset(idx: usize) -> usize {
    (0..idx).map(|i| GLYPH_H * GLYPH_W[i]).sum()
}

/// 把一组（图标 + 多位数字）追加进拼接序列，元素为 (字形下标, 前置间隔)。
fn push_pair(out: &mut Vec<(usize, usize)>, sym: usize, n: usize) {
    out.push((sym, if out.is_empty() { 0 } else { PAIR_GAP }));
    // 数字最多显示两位；超过 99 显示 99+（末尾追加合成「+」字形）。
    for (i, ch) in n.min(99).to_string().bytes().enumerate() {
        out.push((
            (ch - b'0') as usize,
            if i == 0 { SYM_NUM_GAP } else { DIGIT_GAP },
        ));
    }
    if n > 99 {
        out.push((GLYPH_PLUS, DIGIT_GAP));
    }
}

/// (运行, 待办) → 字形拼接序列（运行组在前）；全零返回空 = 回落 app logo。纯函数，便于单测。
fn status_seq(running: usize, waiting: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if running > 0 {
        push_pair(&mut out, GLYPH_RUN, running);
    }
    if waiting > 0 {
        push_pair(&mut out, GLYPH_WAIT, waiting);
    }
    out
}

/// 把 (运行, 待办) 拼成菜单栏**彩色徽章** RGBA：运行绿徽 / 待交互黄徽，内嵌深色数字。
/// 单字符→正圆，多字符/99+→胶囊。数字色相对徽章固定（不随菜单栏明暗）。全零返回 None（回落 logo）。
fn render_status_rgba(running: usize, waiting: usize) -> Option<(Vec<u8>, u32, u32)> {
    // 复用 status_seq（含 99+ 封顶）的扁平序列，切成 [(色标记, 字形们)] 组；徽章自己排版，忽略其 gap。
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (idx, _gap) in status_seq(running, waiting) {
        if idx == GLYPH_RUN || idx == GLYPH_WAIT {
            groups.push((idx, Vec::new()));
        } else if let Some(g) = groups.last_mut() {
            g.1.push(idx);
        }
    }
    if groups.is_empty() {
        return None;
    }
    let total: usize =
        groups.iter().map(|(_, g)| badge_w(g)).sum::<usize>() + BADGE_GAP * (groups.len() - 1);
    let mut rgba = vec![0u8; total * GLYPH_H * 4];
    let mut x = 0;
    for (i, (marker, glyphs)) in groups.iter().enumerate() {
        if i > 0 {
            x += BADGE_GAP;
        }
        let bw = badge_w(glyphs);
        let color = if *marker == GLYPH_RUN {
            RUN_RGB
        } else {
            WAIT_RGB
        };
        fill_round_rect(&mut rgba, total, x, bw, color);
        // 内容（数字/加号）在徽章内水平居中。
        let mut gx = x + (bw - content_w(glyphs)) / 2;
        for (j, &g) in glyphs.iter().enumerate() {
            if j > 0 {
                gx += INNER_GAP;
            }
            if g == GLYPH_PLUS {
                blit_plus(&mut rgba, total, gx, INK);
                gx += PLUS_W;
            } else {
                blit_glyph(&mut rgba, total, gx, g, INK);
                gx += GLYPH_W[g];
            }
        }
        x += bw;
    }
    Some((rgba, total as u32, GLYPH_H as u32))
}

/// 按 (运行, 待办) 更新菜单栏图标：有计数 → 彩色徽章（非模板）；全零 → 回落单色 logo（模板）。
/// 徽章内数字色相对徽章固定，故无需感知菜单栏明暗。
/// set_icon_with_as_template 原子换图标+模板态（避免闪烁）；清掉标题文字。
pub fn update_tray_status(app: &AppHandle, running: usize, waiting: usize) {
    let Some(tray) = app.tray_by_id("meowo-tray") else {
        return;
    };
    match render_status_rgba(running, waiting) {
        Some((rgba, w, h)) => {
            let _ =
                tray.set_icon_with_as_template(Some(tauri::image::Image::new(&rgba, w, h)), false);
        }
        None => {
            let logo =
                tauri::image::Image::new(MENUBAR_ICON_RGBA, MENUBAR_ICON_SIZE, MENUBAR_ICON_SIZE);
            let _ = tray.set_icon_with_as_template(Some(logo), true);
        }
    }
    let _ = tray.set_title(None::<&str>);
}

/// 托盘右键菜单（打开对话窗口 / 找回贴纸 / 使用引导 / 设置 / 官网 / 退出），按语言构建；
/// 切语言/改设置时由 lib.rs 的 apply_language 重建。菜单构成与 Windows
/// （window::build_tray_menu）对齐：同一组项、同一顺序。`chat_enabled` 为 false
/// （轻量模式）时不放「打开对话窗口」项——入口都不该出现，而不是点了被拦。
/// 设置页切换开关会经 set_settings → apply_language 即时重建。
pub fn build_tray_menu(
    app: &AppHandle,
    lang: &str,
    chat_enabled: bool,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let chat = if chat_enabled {
        Some(MenuItemBuilder::with_id("chat", crate::tr(lang, "tray.chat")).build(app)?)
    } else {
        None
    };
    let recall = MenuItemBuilder::with_id("recall", crate::tr(lang, "tray.recall")).build(app)?;
    let guide = MenuItemBuilder::with_id("guide", crate::tr(lang, "tray.guide")).build(app)?;
    let settings =
        MenuItemBuilder::with_id("settings", crate::tr(lang, "tray.settings")).build(app)?;
    let website =
        MenuItemBuilder::with_id("website", crate::tr(lang, "tray.website")).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", crate::tr(lang, "tray.quit")).build(app)?;
    let mut builder = MenuBuilder::new(app);
    if let Some(chat) = &chat {
        builder = builder.item(chat);
    }
    builder.items(&[&recall, &guide, &settings, &website, &quit]).build()
}

/// 创建 macOS 状态栏托盘：左键切换面板，右键弹「设置 / 退出」菜单。
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let boot_settings = crate::load_settings();
    let menu = build_tray_menu(app, crate::ui_lang(&boot_settings), boot_settings.chat_enabled)?;

    // 菜单栏用单色模板图标（彩色 app 图标在菜单栏里偏花，且不随明暗反色）；Dock/设置页仍用彩色图标。
    let icon = tauri::image::Image::new(MENUBAR_ICON_RGBA, MENUBAR_ICON_SIZE, MENUBAR_ICON_SIZE);
    TrayIconBuilder::with_id("meowo-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Meowo")
        .menu(&menu)
        .show_menu_on_left_click(false) // 左键不弹菜单 => 留给右键
        .on_menu_event(|app, event| match event.id().as_ref() {
            "chat" => crate::open_latest_chat_window(app),
            "recall" => panel::recall_panel(app),
            "guide" => crate::open_onboarding_window(app),
            "settings" => crate::open_settings_window(app),
            "website" => {
                let _ = crate::settings::open_url(crate::settings::SITE_URL.to_string());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            // positioner 需要每次托盘事件记录图标坐标
            tauri_plugin_positioner::on_tray_event(app, &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                panel::toggle_panel(app);
            }
        })
        .build(app)?;
    Ok(())
}

/// macOS 激活策略的引用计数：当前处于「需要 Regular」状态的窗口 label 集合。
/// 设置/引导/更新/新建会话/对话窗口各自在打开时切 Regular、关闭时归还计数；两个窗口同开时
/// 关掉任一个，不能把仍开着的另一个误切回 Accessory（Dock 图标消失、剩余窗口无法获焦），
/// 故按 label 计数、归零才切回。
fn regular_policy_windows() -> &'static Mutex<HashSet<&'static str>> {
    static SET: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 引用计数纯逻辑（便于单测）：登记一个窗口打开。按 label 去重——窗口已存在时打开路径
/// 只是聚焦、会重入，同一窗口不得产生两笔计数。
fn track_regular_open(set: &mut HashSet<&'static str>, label: &'static str) {
    set.insert(label);
}

/// 引用计数纯逻辑（便于单测）：登记一个窗口关闭，返回是否可以切回 Accessory（最后一个
/// 窗口关闭才为 true）。同一窗口的 CloseRequested 与 Destroyed 会各来一次：set 语义下
/// 第二次是空操作，不会提前归零。
fn track_regular_close(set: &mut HashSet<&'static str>, label: &'static str) -> bool {
    set.remove(label);
    set.is_empty()
}

/// 打开设置类窗口前临时切到 Regular 以便获焦（挂在各窗口的创建/聚焦路径上）。
pub fn settings_window_will_open(app: &AppHandle, label: &'static str) {
    if let Ok(mut set) = regular_policy_windows().lock() {
        track_regular_open(&mut set, label);
    }
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
}

/// 归还激活策略计数（挂在各窗口的关闭事件里）：最后一个这类窗口关闭才切回 Accessory。
pub fn settings_window_did_close(app: &AppHandle, label: &'static str) {
    let may_restore = match regular_policy_windows().lock() {
        Ok(mut set) => track_regular_close(&mut set, label),
        // 锁损坏：退化为旧行为（无条件切回），好过永远不切、Dock 图标一直挂着。
        Err(_) => true,
    };
    if may_restore {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 激活策略引用计数：两个窗口同开时关掉任一个不得切回 Accessory；同一窗口的
    /// CloseRequested/Destroyed 双回调与「已存在窗口的重复打开」都不得扰乱计数。
    #[test]
    fn regular_policy_reference_counting() {
        let mut set = HashSet::new();
        track_regular_open(&mut set, "about");
        track_regular_open(&mut set, "chat");
        // 已存在的窗口再次「打开」（聚焦路径）重入，不产生额外计数。
        track_regular_open(&mut set, "chat");
        // 关掉任一个：仍有窗口开着，不切回。
        assert!(!track_regular_close(&mut set, "chat"));
        // 同一窗口的 CloseRequested + Destroyed 双回调：第二次是空操作，不得提前归零。
        assert!(!track_regular_close(&mut set, "chat"));
        // 最后一个窗口关闭 → 归零，切回 Accessory。
        assert!(track_regular_close(&mut set, "about"));
    }

    #[test]
    fn status_seq_groups_and_gaps() {
        // 全空闲 → 空（回落 logo）
        assert_eq!(status_seq(0, 0), Vec::<(usize, usize)>::new());
        // 运行组在前、待办组在后；组间 PAIR_GAP，图标→数字 SYM_NUM_GAP。
        assert_eq!(
            status_seq(3, 2),
            vec![
                (GLYPH_RUN, 0),
                (3, SYM_NUM_GAP),
                (GLYPH_WAIT, PAIR_GAP),
                (2, SYM_NUM_GAP)
            ]
        );
        // 多位数：首位 SYM_NUM_GAP，后续 DIGIT_GAP。
        assert_eq!(
            status_seq(12, 0),
            vec![(GLYPH_RUN, 0), (1, SYM_NUM_GAP), (2, DIGIT_GAP)]
        );
        // 仅待办：待办组打头，前置间隔为 0。
        assert_eq!(status_seq(0, 5), vec![(GLYPH_WAIT, 0), (5, SYM_NUM_GAP)]);
    }

    #[test]
    fn status_seq_caps_at_99_plus() {
        // 恰好 99：两位数，无「+」。
        assert_eq!(
            status_seq(99, 0),
            vec![(GLYPH_RUN, 0), (9, SYM_NUM_GAP), (9, DIGIT_GAP)]
        );
        // 超过 99：显示 99 + 合成「+」。
        assert_eq!(
            status_seq(150, 0),
            vec![
                (GLYPH_RUN, 0),
                (9, SYM_NUM_GAP),
                (9, DIGIT_GAP),
                (GLYPH_PLUS, DIGIT_GAP)
            ]
        );
        // 待交互超 99 同样封顶。
        assert_eq!(
            status_seq(0, 100),
            vec![
                (GLYPH_WAIT, 0),
                (9, SYM_NUM_GAP),
                (9, DIGIT_GAP),
                (GLYPH_PLUS, DIGIT_GAP)
            ]
        );
    }

    #[test]
    fn render_status_none_when_idle_else_sized() {
        assert!(render_status_rgba(0, 0).is_none());
        let (rgba, w, h) = render_status_rgba(3, 2).unwrap();
        assert_eq!(h, GLYPH_H as u32);
        assert_eq!(rgba.len(), (w as usize) * GLYPH_H * 4);
        // 图集长度自洽：等于各字形 H*W 之和。
        assert_eq!(
            GLYPH_ATLAS.len(),
            GLYPH_W.iter().map(|w| GLYPH_H * w).sum::<usize>()
        );
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
    fn render_badge_fill_and_ink() {
        // 运行 2：含绿色徽章填充 + 深色数字墨。
        let (run, _, h) = render_status_rgba(2, 0).unwrap();
        assert_eq!(h, GLYPH_H as u32);
        assert!(has_opaque_rgb(&run, RUN_RGB), "缺绿色徽章");
        assert!(has_opaque_rgb(&run, INK), "缺深色数字墨");
        // 待交互 3：含黄色徽章。
        let (wait, _, _) = render_status_rgba(0, 3).unwrap();
        assert!(has_opaque_rgb(&wait, WAIT_RGB), "缺黄色徽章");
    }

    #[test]
    fn single_digit_is_circle_multi_is_pill() {
        // 单字符徽章 = 正圆（宽 = H_BADGE）；两位数徽章更宽（胶囊）。
        assert_eq!(badge_w(&[2]), H_BADGE as usize);
        assert!(badge_w(&[9, 9]) > H_BADGE as usize);
        assert!(badge_w(&[9, 9, GLYPH_PLUS]) > badge_w(&[9, 9]));
    }
}
