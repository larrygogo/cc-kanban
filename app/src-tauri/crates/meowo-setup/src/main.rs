//! Meowo 安装器壳（Windows only）。
//!
//! 路由三分支：
//! 1. 命令行带任何 `/` 开头参数（/S /P /UPDATE /D= …）→ 透传模式：释出内芯、
//!    原始命令行尾串整体转交、退出码透传，全程无 UI（防御性——updater 正常走的是
//!    NSIS 芯自己，不经过壳，见 nsis/README.md）。
//! 2. WebView2 缺失/损坏 → 释出内芯跑它的 GUI（NSIS 自绘一键界面）兜底。
//! 3. 正常路径 → wry 无边框窗口渲染内嵌 HTML（ui 模块）。
//!
//! 非 Windows：空 main。GUI 依赖全部锁在 cfg(windows)（Cargo.toml），CI 的
//! macos 矩阵只编 logic 纯逻辑与单测。

#![cfg_attr(windows, windows_subsystem = "windows")]

// 纯逻辑跨平台编译（单测在 mac 矩阵也跑）；消费者都在 cfg(windows) 模块里，
// 非 Windows 下按 dead_code 豁免。
#[cfg_attr(not(windows), allow(dead_code))]
mod logic;

#[cfg(windows)]
mod detect;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod payload;
#[cfg(windows)]
mod ui;

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    if logic::is_passthrough(std::env::args().skip(1)) {
        std::process::exit(install::run_passthrough());
    }
    if !detect::webview2_present() {
        std::process::exit(install::run_inner_gui());
    }
    // wry 起不来（运行时损坏等）同样兜底到内芯 GUI，绝不空手死。
    if ui::run().is_err() {
        std::process::exit(install::run_inner_gui());
    }
}
