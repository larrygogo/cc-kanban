//! 内芯（NSIS 安装器）载荷：build.rs 生成的桥文件（正式构建 include_bytes! 真身，
//! 本地 dev/clippy/mac CI 为空占位），释出到临时目录、Drop 时 best-effort 清理。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

include!(concat!(env!("OUT_DIR"), "/payload.rs"));

pub enum ExtractError {
    /// dev 桩（构建时没喂 MEOWO_INNER_SETUP）。
    EmptyPayload,
    /// 写临时文件失败——最常见诱因是杀软拦"exe 释出 exe"，UI 文案按此措辞。
    Io(std::io::Error),
}

pub struct Extracted {
    dir: PathBuf,
    pub exe: PathBuf,
}

impl Drop for Extracted {
    fn drop(&mut self) {
        // best-effort：安装器进程若还占着（不该发生，所有路径都 wait），留给系统临时清理。
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn extract() -> Result<Extracted, ExtractError> {
    if INNER_SETUP.is_empty() {
        return Err(ExtractError::EmptyPayload);
    }
    // pid + 纳秒：同机并发双开也不撞目录。
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("meowo-setup-{}-{nanos:x}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(ExtractError::Io)?;
    let exe = dir.join("meowo-inner-setup.exe");
    std::fs::write(&exe, INNER_SETUP).map_err(ExtractError::Io)?;
    Ok(Extracted { dir, exe })
}
