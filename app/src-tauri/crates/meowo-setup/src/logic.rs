//! 壳的纯逻辑层：零平台依赖，单测全在这（CI 三平台矩阵都跑）。
//! 原则：所有「决策」（透传判定、版本关系、路径规范、参数拼装）都住在这里，
//! Windows 模块只做 IO 与系统调用——UI 的 JS 侧不保存业务真相，目录等一律
//! 显示 Rust 规范化后的回推结果，避免两处实现同一套规则。

use std::cmp::Ordering;

/// 任一参数以 `/` 开头 → 透传模式（/S /P /R /UPDATE /NS /D= 全涵盖）。
/// `--flag` 等 Unix 风格不算：那不是 NSIS 语义，走正常 UI 更不容易吞掉误操作。
pub fn is_passthrough<I: IntoIterator<Item = String>>(args: I) -> bool {
    args.into_iter().any(|a| a.starts_with('/'))
}

/// 版本关系：semver 优先，任一侧解析失败回退「三段数字」比较（NSIS 侧
/// DisplayVersion 恒为 x.y.z，但注册表可能被第三方工具改花，不能 panic）。
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    #[cfg(windows)]
    if let (Ok(x), Ok(y)) = (
        semver::Version::parse(a.trim()),
        semver::Version::parse(b.trim()),
    ) {
        return x.cmp(&y);
    }
    numeric_triplet(a).cmp(&numeric_triplet(b))
}

/// 宽容取前三段数字：非数字字符截断、缺段补 0。"1.2.3-beta" → (1,2,3)，"垃圾" → (0,0,0)。
fn numeric_triplet(s: &str) -> (u64, u64, u64) {
    let mut it = s.trim().split('.').map(|seg| {
        let digits: String = seg.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u64>().unwrap_or(0)
    });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    /// 空输入。
    Empty,
    /// 不是 `X:\` 开头的本地绝对路径（含相对路径与 UNC）。
    Relative,
    /// 命中受保护目录前缀（currentUser 安装写不进去，提前拒绝好过静默失败）。
    Forbidden,
    /// 含 Windows 不允许的文件名字符。
    InvalidChars,
}

/// 路径规范化（与 NSIS DIRECTORY 页语义对齐）：
/// 去首尾空白与尾反斜杠；末段不是产品名（不分大小写）则补 `\Meowo`；
/// `forbidden` 为受保护目录前缀列表（生产侧从环境变量取，测试注入）。
pub fn normalize_install_dir_with(input: &str, forbidden: &[String]) -> Result<String, PathError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(PathError::Empty);
    }
    let bytes = trimmed.as_bytes();
    // 只接受 `X:\`：UNC/相对路径在 /D= 与卸载注册表链路上都是坑，直接拒。
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || (bytes[2] != b'\\' && bytes[2] != b'/')
    {
        return Err(PathError::Relative);
    }
    // 盘符后的部分不允许 Windows 保留字符（冒号只允许出现在位置 1）。
    if trimmed[2..]
        .chars()
        .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*' | ':') || (c as u32) < 0x20)
    {
        return Err(PathError::InvalidChars);
    }
    let mut norm = trimmed.replace('/', "\\");
    while norm.ends_with('\\') && norm.len() > 3 {
        norm.pop();
    }
    let last = norm.rsplit('\\').next().unwrap_or("");
    if !last.eq_ignore_ascii_case("meowo") {
        if !norm.ends_with('\\') {
            norm.push('\\');
        }
        norm.push_str("Meowo");
    }
    let lower = norm.to_ascii_lowercase();
    for f in forbidden {
        let mut p = f.trim().to_ascii_lowercase().replace('/', "\\");
        if p.is_empty() {
            continue;
        }
        if !p.ends_with('\\') {
            p.push('\\');
        }
        // 前缀命中或恰为该目录本身（补出的 \Meowo 使相等分支基本不可达，防御着写）。
        if lower.starts_with(&p) || format!("{lower}\\") == p {
            return Err(PathError::Forbidden);
        }
    }
    Ok(norm)
}

/// 内芯静默参数：`/D=` 必须**最后一个**、值不加引号、无尾反斜杠——NSIS 硬规则，
/// 调用侧统一 `raw_arg` 逐个追加（std 的引号规则会破坏 `/D=` 语义）。
pub fn build_silent_args(dir: &str, desktop_shortcut: bool) -> Vec<String> {
    let mut args = vec!["/S".to_string()];
    if !desktop_shortcut {
        args.push("/NS".to_string());
    }
    args.push(format!("/D={}", dir.trim_end_matches('\\')));
    args
}

/// 从 GetCommandLineW 的完整命令行剥掉 argv[0]（引号包裹或空格分隔两种形态），
/// 返回原始尾串——透传模式必须整串转交，std 的逐参数引号化会破坏 `/D=C:\a b` 语义。
pub fn strip_argv0(cmdline: &str) -> &str {
    let s = cmdline.trim_start();
    let rest = if let Some(stripped) = s.strip_prefix('"') {
        match stripped.find('"') {
            Some(i) => &stripped[i + 1..],
            None => "",
        }
    } else {
        match s.find(' ') {
            Some(i) => &s[i..],
            None => "",
        }
    };
    rest.trim_start()
}

/// 极简 base64（标准字母表 + padding）：只给 UI 内联 logo 用，
/// 不为 20 行编码器引一个新依赖版本进 Cargo.lock。
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn passthrough_rules() {
        assert!(!is_passthrough(v(&[])));
        assert!(is_passthrough(v(&["/S"])));
        assert!(is_passthrough(v(&["/S", "/D=C:\\Tmp\\Meowo"])));
        assert!(is_passthrough(v(&["前置", "/UPDATE"])));
        // Unix 风格不触发：不是 NSIS 语义。
        assert!(!is_passthrough(v(&["--help", "-v"])));
    }

    #[test]
    fn version_relations() {
        assert_eq!(compare_versions("0.5.13", "0.5.14"), Less);
        assert_eq!(compare_versions("0.5.14", "0.5.14"), Equal);
        assert_eq!(compare_versions("0.6.0", "0.5.14"), Greater);
        // 两位数段不能按字符串比。
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Greater);
        // 非法串走回退路径，不 panic。
        assert_eq!(compare_versions("垃圾", "0.5.14"), Less);
        // 预发布 < 正式：semver 语义，只在 Windows（semver 依赖锁在 cfg(windows)，
        // 其余平台回退三段数字比较会判 Equal——生产代码只跑在 Windows，无碍）。
        #[cfg(windows)]
        assert_eq!(compare_versions("1.2.3-beta", "1.2.3"), Less);
    }

    #[test]
    fn dir_normalization() {
        let no = &[];
        assert_eq!(
            normalize_install_dir_with(r"D:\Apps", no).unwrap(),
            r"D:\Apps\Meowo"
        );
        // 已是 Meowo（任意大小写）不重复补。
        assert_eq!(
            normalize_install_dir_with(r"D:\Apps\meowo", no).unwrap(),
            r"D:\Apps\meowo"
        );
        // 尾反斜杠剥除（/D= 不允许）。
        assert_eq!(
            normalize_install_dir_with(r"D:\Apps\Meowo\", no).unwrap(),
            r"D:\Apps\Meowo"
        );
        // 盘根。
        assert_eq!(normalize_install_dir_with(r"D:\", no).unwrap(), r"D:\Meowo");
        // 正斜杠归一。
        assert_eq!(
            normalize_install_dir_with("D:/Apps", no).unwrap(),
            r"D:\Apps\Meowo"
        );
        assert_eq!(normalize_install_dir_with("  ", no), Err(PathError::Empty));
        assert_eq!(
            normalize_install_dir_with(r"Apps\Meowo", no),
            Err(PathError::Relative)
        );
        assert_eq!(
            normalize_install_dir_with(r"\\server\share", no),
            Err(PathError::Relative)
        );
        assert_eq!(
            normalize_install_dir_with(r"D:\a<b", no),
            Err(PathError::InvalidChars)
        );
    }

    #[test]
    fn dir_forbidden_prefixes() {
        let forbidden = v(&[r"C:\Program Files", r"C:\Windows\"]);
        assert_eq!(
            normalize_install_dir_with(r"C:\Program Files\Meowo", &forbidden),
            Err(PathError::Forbidden)
        );
        // 大小写不敏感。
        assert_eq!(
            normalize_install_dir_with(r"c:\windows\system32", &forbidden),
            Err(PathError::Forbidden)
        );
        // 相似前缀不误伤。
        assert_eq!(
            normalize_install_dir_with(r"C:\Program Files Extra", &forbidden).unwrap(),
            r"C:\Program Files Extra\Meowo"
        );
    }

    #[test]
    fn silent_args_shape() {
        assert_eq!(build_silent_args(r"D:\Apps\Meowo", true), ["/S", r"/D=D:\Apps\Meowo"]);
        assert_eq!(
            build_silent_args(r"D:\Apps\Meowo\", false),
            ["/S", "/NS", r"/D=D:\Apps\Meowo"]
        );
        // /D= 恒为最后一个（NSIS 硬规则）。
        let args = build_silent_args(r"C:\a b\Meowo", false);
        assert!(args.last().unwrap().starts_with("/D="));
    }

    #[test]
    fn argv0_stripping() {
        assert_eq!(
            strip_argv0(r#""C:\Down loads\setup.exe" /S /D=C:\a b\Meowo"#),
            r"/S /D=C:\a b\Meowo"
        );
        assert_eq!(strip_argv0(r"setup.exe /S"), "/S");
        assert_eq!(strip_argv0(r#""C:\x\setup.exe""#), "");
        assert_eq!(strip_argv0("setup.exe"), "");
    }

    #[test]
    fn base64_basics() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
