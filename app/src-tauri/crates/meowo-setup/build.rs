//! 三件事：①从 workspace 根注入应用版本；②嵌入带 PerMonitorV2 的 manifest；
//! ③按 MEOWO_INNER_SETUP 环境变量生成内芯载荷桥文件（缺省生成空占位，保证
//! 本地 clippy/test 与 CI macos 矩阵在没有 NSIS 产物时也能编译）。

use std::path::Path;

fn main() {
    // ── 版本：workspace 根包（meowo-app）的 version 是全仓唯一真源 ──
    let root_manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", root_manifest.display());
    let version = std::fs::read_to_string(&root_manifest)
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.trim_start().starts_with("version"))
                .and_then(|l| l.split('"').nth(1).map(str::to_string))
        })
        .expect("无法从 workspace 根 Cargo.toml 解析 version");
    println!("cargo:rustc-env=MEOWO_APP_VERSION={version}");

    // ── manifest：照抄 app build.rs 的全局 /MANIFEST:EMBED 方式；壳自己的文件
    //    额外声明 PerMonitorV2（wry/tao 需要 DPI 感知，且无 tauri_build 代劳）──
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        let manifest =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        // 注：/MANIFESTINPUT 的路径不能含空格（link.exe 参数不带引号透传）。
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());

        // exe 图标 + 版本信息资源（Explorer/任务栏显示；无 tauri_build 代劳）。
        // 图标复用应用的 icon.ico；.rc 里路径用正斜杠免转义。
        // cfg(windows) 在 build.rs 里按 HOST 判定：mac CI 矩阵没有 embed-resource 依赖，
        // 这个调用必须随依赖一起被裁掉。
        #[cfg(windows)]
        embed_icon_and_version(&version);
    }

    // ── 内芯载荷桥：正式构建由 CI/开发者把 MEOWO_INNER_SETUP 指向
    //    bundle/nsis/Meowo_<ver>_x64-setup.exe；未设或文件不存在时生成空占位，
    //    运行时空载荷 → UI 错误页 / 透传模式 exit 2（见 payload.rs / install.rs）──
    println!("cargo:rerun-if-env-changed=MEOWO_INNER_SETUP");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("payload.rs");
    let bridge = match std::env::var("MEOWO_INNER_SETUP") {
        Ok(p) if Path::new(&p).is_file() => {
            println!("cargo:rerun-if-changed={p}");
            format!("pub static INNER_SETUP: &[u8] = include_bytes!(r\"{p}\");\n")
        }
        _ => "pub static INNER_SETUP: &[u8] = &[];\n".to_string(),
    };
    std::fs::write(&out, bridge).expect("写 payload 桥文件失败");
}

/// exe 图标 + VERSIONINFO（Explorer 文件图标、任务栏、文件属性页）。
/// rc.exe 缺席的开发机上静默跳过（manifest_optional）——CI 的 windows runner 必有。
#[cfg(windows)]
fn embed_icon_and_version(version: &str) {
    let icon = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../icons/icon.ico")
        .canonicalize()
        .expect("找不到 app/src-tauri/icons/icon.ico");
    // canonicalize 带 \\?\ 前缀且是反斜杠；rc.exe 两者都不吃，转正斜杠并剥前缀。
    let icon = icon.display().to_string().replace('\\', "/");
    let icon = icon.trim_start_matches("//?/");
    println!("cargo:rerun-if-changed={icon}");

    let mut nums: Vec<u32> = version.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    nums.resize(4, 0);
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {0},{1},{2},{3}
PRODUCTVERSION {0},{1},{2},{3}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "ProductName", "Meowo"
      VALUE "FileDescription", "Meowo Installer"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "meowo-setup.exe"
      VALUE "CompanyName", "larrygogo"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        nums[0], nums[1], nums[2], nums[3]
    );
    let rc_path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("meowo-setup.rc");
    std::fs::write(&rc_path, rc).expect("写 rc 文件失败");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .expect("编译 exe 资源失败");
}
