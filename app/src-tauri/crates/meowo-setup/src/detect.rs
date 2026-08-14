//! 环境探测（Windows only）：WebView2 运行时在场性 + 已装 Meowo 版本。

use std::path::PathBuf;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use winreg::RegKey;

/// WebView2 Evergreen 运行时的 EdgeUpdate 客户端 GUID（与 NSIS 模板
/// installer.nsi 的 WEBVIEW2APPGUID 同源，三级探测顺序也一致）。
const WEBVIEW2_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

pub fn webview2_present() -> bool {
    let candidates = [
        (
            HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}"),
        ),
        (
            HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}"),
        ),
        (
            HKEY_CURRENT_USER,
            format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}"),
        ),
    ];
    candidates.iter().any(|(root, path)| {
        RegKey::predef(*root)
            .open_subkey(path)
            .ok()
            .and_then(|k| k.get_value::<String, _>("pv").ok())
            .is_some_and(|pv| !pv.trim().is_empty() && pv.trim() != "0.0.0.0")
    })
}

/// 已装的 Meowo（currentUser 安装 → HKCU 卸载键；NSIS 模板写入的字段）。
pub struct Existing {
    pub version: String,
    pub uninstall_exe: PathBuf,
    pub install_dir: PathBuf,
}

pub fn detect_existing() -> Option<Existing> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Meowo")
        .ok()?;
    let version: String = key.get_value("DisplayVersion").ok()?;
    // UninstallString 是带引号写入的（"C:\...\uninstall.exe"），剥首尾引号即可。
    let uninstall: String = key.get_value("UninstallString").ok()?;
    let uninstall_exe = PathBuf::from(uninstall.trim().trim_matches('"'));
    let install_dir = key
        .get_value::<String, _>("InstallLocation")
        .ok()
        .map(|s| PathBuf::from(s.trim().trim_matches('"')))
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| uninstall_exe.parent().map(PathBuf::from))?;
    Some(Existing {
        version,
        uninstall_exe,
        install_dir,
    })
}
