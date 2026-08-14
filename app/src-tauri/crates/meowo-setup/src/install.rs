//! 安装编排：三条执行路径（透传 / NSIS GUI 兜底 / UI 驱动的静默安装）。
//! 内芯 `/S` 的语义与约束见 ../../nsis/README.md 与 installer.nsi 的 MEOWO 注释。

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::detect;
use crate::logic;
use crate::payload::{self, ExtractError};

/// 与 app 侧先例（src/install.rs）同值：子进程不弹控制台窗。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const EXIT_EMPTY_PAYLOAD: i32 = 2;
pub const EXIT_EXTRACT_FAILED: i32 = 3;

/// GetCommandLineW 的原始尾串（剥 argv[0]）。透传必须整串转交：
/// std 的逐参数引号化会破坏 NSIS `/D=C:\a b` 的无引号语义。
fn raw_cmdline_tail() -> String {
    use windows_sys::Win32::System::Environment::GetCommandLineW;
    // SAFETY: GetCommandLineW 返回进程生命周期内有效的 NUL 结尾 UTF-16 串。
    unsafe {
        let p = GetCommandLineW();
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        let full = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        logic::strip_argv0(&full).to_string()
    }
}

/// 透传模式：/S /P /UPDATE 等一律原样转交内芯并回传退出码，全程无 UI。
pub fn run_passthrough() -> i32 {
    let extracted = match payload::extract() {
        Ok(e) => e,
        Err(ExtractError::EmptyPayload) => return EXIT_EMPTY_PAYLOAD,
        Err(ExtractError::Io(_)) => return EXIT_EXTRACT_FAILED,
    };
    let tail = raw_cmdline_tail();
    let mut cmd = Command::new(&extracted.exe);
    if !tail.is_empty() {
        cmd.raw_arg(&tail);
    }
    cmd.creation_flags(CREATE_NO_WINDOW);
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(0),
        Err(_) => EXIT_EXTRACT_FAILED,
    }
}

/// 兜底：无/坏 WebView2 时跑内芯自己的 GUI（NSIS 自绘一键界面）。
/// 等待退出后再返回（Extracted Drop 清理 temp）；壳此时无窗口，等待零成本。
pub fn run_inner_gui() -> i32 {
    let extracted = match payload::extract() {
        Ok(e) => e,
        Err(ExtractError::EmptyPayload) => {
            message_box("安装程序载荷缺失（开发构建）。请使用正式发布的安装包。");
            return EXIT_EMPTY_PAYLOAD;
        }
        Err(ExtractError::Io(e)) => {
            message_box(&format!(
                "写入临时文件失败，可能被杀毒软件拦截。\n\n{e}"
            ));
            return EXIT_EXTRACT_FAILED;
        }
    };
    match Command::new(&extracted.exe).status() {
        Ok(s) => s.code().unwrap_or(0),
        Err(e) => {
            message_box(&format!("启动安装程序失败：{e}"));
            EXIT_EXTRACT_FAILED
        }
    }
}

/// 兜底路径没有自绘 UI，错误只能靠系统 MessageBox。
fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "Meowo 安装".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: 两个指针都指向本函数栈上的 NUL 结尾缓冲。
    unsafe {
        MessageBoxW(std::ptr::null_mut(), wide.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

pub struct InstallReq {
    pub dir: String,
    pub chat_enabled: bool,
    pub desktop_shortcut: bool,
}

pub enum InstallError {
    /// 目标位置写探针失败。
    NotWritable,
    /// 载荷释出失败（detail 供错误页展示）。
    Extract(String),
    /// 降级预卸载失败（退出码）。
    Uninstall(i32),
    /// 内芯 /S 非零退出（含 WebView2 段静默 Abort）。
    Inner(i32),
}

/// UI 安装线程主体：写探针 → 降级预卸载 → 释出 → /S 静默装 → 写 ChatEnabled 种子。
pub fn perform_install(req: &InstallReq) -> Result<(), InstallError> {
    if !writable_probe(Path::new(&req.dir)) {
        return Err(InstallError::NotWritable);
    }

    // 降级预卸载：/S 下内芯不跑 MeowoApplyReinstall（Page 函数），语义由壳补齐。
    // `_?=` 让卸载器在原目录同步执行——否则它自拷贝到 temp 后立刻返回，等不到真完成。
    if let Some(existing) = detect::detect_existing() {
        if logic::compare_versions(&existing.version, env!("MEOWO_APP_VERSION"))
            == std::cmp::Ordering::Greater
            && existing.uninstall_exe.is_file()
        {
            let status = Command::new(&existing.uninstall_exe)
                .raw_arg("/S")
                .raw_arg(format!("_?={}", existing.install_dir.display()))
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .map_err(|_| InstallError::Uninstall(-1))?;
            if !status.success() {
                return Err(InstallError::Uninstall(status.code().unwrap_or(-1)));
            }
        }
    }

    let extracted = payload::extract().map_err(|e| match e {
        ExtractError::EmptyPayload => InstallError::Extract("empty payload (dev build)".into()),
        ExtractError::Io(e) => InstallError::Extract(e.to_string()),
    })?;

    let mut cmd = Command::new(&extracted.exe);
    for arg in logic::build_silent_args(&req.dir, req.desktop_shortcut) {
        cmd.raw_arg(arg);
    }
    let status = cmd
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| InstallError::Extract(e.to_string()))?;
    if !status.success() {
        return Err(InstallError::Inner(status.code().unwrap_or(-1)));
    }

    // 「对话窗口功能」种子：/S 下内芯刻意不写（不代表用户选择），由壳落。
    // 失败不阻断——应用侧默认全功能，seed.rs 读不到种子就走默认。
    write_chat_seed(req.chat_enabled);
    Ok(())
}

fn write_chat_seed(enabled: bool) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    if let Ok((key, _)) =
        RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\larrygogo\Meowo")
    {
        let _ = key.set_value("ChatEnabled", &u32::from(enabled));
    }
}

/// 沿路径向上找最深已存在祖先目录做写探针。
fn writable_probe(dir: &Path) -> bool {
    let mut probe_at = dir;
    loop {
        if probe_at.exists() {
            break;
        }
        match probe_at.parent() {
            Some(p) => probe_at = p,
            None => return false,
        }
    }
    let file = probe_at.join(format!("~meowo-probe-{}.tmp", std::process::id()));
    match std::fs::File::create(&file) {
        Ok(_) => {
            let _ = std::fs::remove_file(&file);
            true
        }
        Err(_) => false,
    }
}
