//! 「查看改动」弹层的 git 数据命令：工作树相对 HEAD 的变更清单与单文件 diff。
//! 不经 shell 直接 spawn git（路径作为独立 argv，无注入面）；子进程可能被杀软拖慢，
//! 统一走 spawn_blocking，不进主线程（同类教训见 terminal.rs open_project_dir 的注释）。

use meowo_protocol::ipc::{GitChangedFileDto, GitDiffSummaryDto, GitFileDiffDto};
use std::path::Path;
use std::process::Command;

/// diff 文本与未跟踪文件读取的统一上限（超出即截断并置 truncated）。
const MAX_DIFF_BYTES: usize = 200 * 1024;
/// 未跟踪文件的二进制嗅探窗口：前 8KB 出现 NUL 即按二进制处理。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 工作树改动摘要：git 可用性、是否仓库、分支名与变更文件列表。
/// 按钮可见性由前端按 `is_repo` 决定，故此命令对「不是仓库」返回正常 DTO 而非报错。
#[tauri::command]
pub(crate) async fn git_diff_summary(cwd: String) -> Result<GitDiffSummaryDto, String> {
    tauri::async_runtime::spawn_blocking(move || git_diff_summary_blocking(&cwd))
        .await
        .map_err(|e| e.to_string())?
}

fn git_diff_summary_blocking(cwd: &str) -> Result<GitDiffSummaryDto, String> {
    let dir = cwd.trim();
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return Err("目录不存在".into());
    }
    let unavailable = || GitDiffSummaryDto {
        git_available: false,
        is_repo: false,
        branch: None,
        files: vec![],
    };
    // spawn 失败（git 未安装/被拦）不算错误——前端只是不显示按钮。
    let inside = match git_output(dir, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(output) => output,
        Err(_) => return Ok(unavailable()),
    };
    if !inside.status.success() || stdout_text(&inside.stdout).trim() != "true" {
        return Ok(GitDiffSummaryDto {
            is_repo: false,
            ..unavailable()
        });
    }
    let branch = git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| stdout_text(&o.stdout).trim().to_string())
        .filter(|name| !name.is_empty() && name != "HEAD");
    // --no-renames：-z 格式下重命名条目会带两个路径，禁掉后每个条目恒为「XY <path>\0」。
    // -uall：整个目录都未跟踪时默认只报一条「?? dir/」，前端会把它当文件渲染
    //（尾斜杠让文件名为空、点开按文件读目录直接报错）；展开成具体文件才是「更改」的语义。
    let status = git_output(dir, &["status", "--porcelain", "-z", "--no-renames", "-uall"])
        .map_err(|e| e.to_string())?;
    if !status.status.success() {
        return Err("git status 执行失败".into());
    }
    Ok(GitDiffSummaryDto {
        git_available: true,
        is_repo: true,
        branch,
        files: parse_porcelain_z(&status.stdout),
    })
}

/// 单个文件的 diff。tracked 走 `git diff HEAD -- <path>`（staged+unstaged 合一）；
/// untracked 直接读盘合成伪 diff（`--- /dev/null` / `+++ b/<path>` 头 + 每行 `+` 前缀，
/// 见 GitFileDiffDto 注释）。
#[tauri::command]
pub(crate) async fn git_file_diff(
    cwd: String,
    path: String,
    untracked: bool,
) -> Result<GitFileDiffDto, String> {
    tauri::async_runtime::spawn_blocking(move || git_file_diff_blocking(&cwd, &path, untracked))
        .await
        .map_err(|e| e.to_string())?
}

fn git_file_diff_blocking(cwd: &str, path: &str, untracked: bool) -> Result<GitFileDiffDto, String> {
    let dir = cwd.trim();
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return Err("目录不存在".into());
    }
    if untracked {
        return untracked_file_diff(dir, path);
    }
    let output = git_output(dir, &["diff", "HEAD", "--", path]).map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("git diff 执行失败".into());
    }
    let text = stdout_text(&output.stdout);
    // 跟踪文件里的二进制(图片、含 NUL 的源码等):git 只回「Binary files a/… and b/…
    // differ」这一行,没有 hunk。原样透出会渲染成几行灰底 meta(看着像坏掉),归一成
    // 与 untracked 分支同一个「(binary file)」占位,前端据此显示「二进制文件,无法显示
    // diff」,而不是误报「文件与最新提交一致」。
    if diff_is_binary(&text) {
        return Ok(GitFileDiffDto {
            path: path.to_string(),
            status: "M".into(),
            diff: "(binary file)\n".into(),
            truncated: false,
        });
    }
    let (diff, truncated) = truncate_utf8(text, MAX_DIFF_BYTES);
    Ok(GitFileDiffDto {
        path: path.to_string(),
        status: "M".into(),
        diff,
        truncated,
    })
}

/// `git diff` 对二进制文件只吐一行 `Binary files a/… and b/… differ`(新增/删除侧
/// 是 `/dev/null`),没有 `@@` hunk。命中即视为二进制,归一到 `(binary file)` 占位。
fn diff_is_binary(diff: &str) -> bool {
    diff.lines()
        .any(|line| line.starts_with("Binary files ") && line.ends_with(" differ"))
}

/// 未跟踪文件的伪 diff：读盘（封顶 MAX_DIFF_BYTES），二进制嗅探命中则只回占位行。
fn untracked_file_diff(dir: &str, path: &str) -> Result<GitFileDiffDto, String> {
    use std::io::Read;
    // path 正常来自 git status，但前端传什么都进得来：裸 join 时绝对路径会整个顶掉
    // cwd，`..` 能一路爬出工作目录。统一走 resolve_inside（canonicalize + 前缀校验），
    // 越界返回与文件页签同一句「路径超出工作目录」。tracked 分支不需要——那边 path
    // 是 git 的 pathspec，git 自己限定在仓库内。
    let full_path = crate::fsutil::resolve_inside(dir, path)?;
    let file = std::fs::File::open(&full_path).map_err(|e| format!("无法读取文件：{e}"))?;
    let mut bytes = Vec::new();
    file.take((MAX_DIFF_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("无法读取文件：{e}"))?;
    let truncated = bytes.len() > MAX_DIFF_BYTES;
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Ok(GitFileDiffDto {
            path: path.to_string(),
            status: "U".into(),
            diff: "(binary file)\n".into(),
            truncated: false,
        });
    }
    if truncated {
        bytes.truncate(MAX_DIFF_BYTES);
    }
    // 与 stdout_text 同一解码口径：untracked 的 GBK 文件同样不能整屏替换符。
    let text = crate::fsutil::decode_text_bytes(&bytes);
    let mut diff = format!("--- /dev/null\n+++ b/{path}\n");
    for line in text.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    Ok(GitFileDiffDto {
        path: path.to_string(),
        status: "U".into(),
        diff,
        truncated,
    })
}

fn git_output(dir: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    // Windows：GUI 进程 spawn 控制台子进程会闪现 conhost 黑窗，压掉。
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command.output()
}

/// git 输出解码走 fsutil 的统一口径（严格 UTF-8 → GBK → lossy）：GBK 源码文件
/// （中文 Windows 常见）的 diff 此前被无脑 lossy 解成整屏替换符。
fn stdout_text(bytes: &[u8]) -> String {
    crate::fsutil::decode_text_bytes(bytes)
}

/// 截断到 cap 字节（落在 UTF-8 字符边界上），返回是否发生了截断。
fn truncate_utf8(mut text: String, cap: usize) -> (String, bool) {
    if text.len() <= cap {
        return (text, false);
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

/// 解析 `git status --porcelain -z --no-renames` 输出：每个条目是 `XY <path>\0`。
/// 状态归一化：X/Y 任一为 M/A/D/T 取该字母（X 优先）；`??` 记为 "U"（未跟踪）。
fn parse_porcelain_z(bytes: &[u8]) -> Vec<GitChangedFileDto> {
    let mut files = Vec::new();
    for entry in bytes.split(|b| *b == 0) {
        if entry.len() < 4 {
            continue;
        }
        let x = entry[0];
        let y = entry[1];
        // entry[2] 是空格分隔符；路径原样保留（porcelain 用正斜杠，前端按它展示）。
        let path = String::from_utf8_lossy(&entry[3..]).into_owned();
        // 目录条目(尾斜杠,如 -uall 缺席时的「?? dir/」)不是文件:渲染出来是一行空名,
        // 点开按文件读目录必报错。防御性跳过——正常路径下 -uall 已把目录展开成文件。
        if path.is_empty() || path.ends_with('/') {
            continue;
        }
        let status: String = if x == b'?' && y == b'?' {
            "U".into()
        } else if matches!(x, b'M' | b'A' | b'D' | b'T') {
            // X 位优先：已暂存的变更比工作区的更能代表「相对 HEAD」的状态。
            (x as char).to_string()
        } else if matches!(y, b'M' | b'A' | b'D' | b'T') {
            (y as char).to_string()
        } else {
            // 不认识的状态字母（R/C 等，--no-renames 下本不该出现）：原样透出非空格位。
            let other = if x == b' ' { y } else { x };
            if other == b' ' || other == b'?' {
                continue;
            }
            (other as char).to_string()
        };
        files.push(GitChangedFileDto { path, status });
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_z_maps_status_letters() {
        let raw = b" M src/a.rs\0M  src/b.rs\0A  src/c.rs\0D  src/d.rs\0?? src/new.rs\0";
        let files = parse_porcelain_z(raw);
        let got: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.status.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("src/a.rs", "M"),
                ("src/b.rs", "M"),
                ("src/c.rs", "A"),
                ("src/d.rs", "D"),
                ("src/new.rs", "U"),
            ]
        );
    }

    #[test]
    fn porcelain_z_skips_untracked_directory_entries() {
        // -uall 缺席或旧输出里的目录条目(尾斜杠):不是文件,不能进「更改」清单。
        let raw = b"?? target-check/\0?? src/new.rs\0";
        let files = parse_porcelain_z(raw);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/new.rs");
    }

    #[test]
    fn diff_is_binary_matches_git_binary_marker() {
        // 修改中的二进制:git 的三行块(头 + index + Binary files … differ)。
        let modified = "diff --git a/x.png b/x.png\nindex abc..def 100644\nBinary files a/x.png and b/x.png differ\n";
        assert!(diff_is_binary(modified));
        // 新增二进制:比对一侧是 /dev/null。
        let added = "diff --git a/x.bin b/x.bin\nBinary files /dev/null and b/x.bin differ\n";
        assert!(diff_is_binary(added));
        // 正常文本 diff 不误判(即便正文里恰好有以 Binary files 开头的一行,也不以
        //  differ 收尾)。
        let text = "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old\n+Binary files list differ here, ok\n";
        assert!(!diff_is_binary(text));
        assert!(!diff_is_binary(""));
    }

    #[test]
    fn porcelain_z_skips_short_entries_and_keeps_utf8_paths() {
        let raw = b"\0M  \0?? \xe6\x96\xb0\xe5\xbb\xba.txt\0";
        let files = parse_porcelain_z(raw);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "新建.txt");
        assert_eq!(files[0].status, "U");
    }

    /// untracked 的 path 由前端透传，必须挡住 `..` 与绝对路径逃逸——两种目标都真实
    /// 存在，确保拒绝来自包含校验（「路径超出工作目录」）而不是「路径不存在」。
    #[test]
    fn untracked_file_diff_rejects_paths_outside_cwd() {
        let root = std::env::temp_dir().join(format!("meowo-git-escape-{}", std::process::id()));
        let inside = root.join("repo");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(root.join("outside.txt"), "secret").unwrap();
        std::fs::write(inside.join("ok.txt"), "hello").unwrap();
        let cwd = inside.to_str().unwrap();
        assert_eq!(
            untracked_file_diff(cwd, "../outside.txt").unwrap_err(),
            "路径超出工作目录"
        );
        // 绝对路径 join 会整个顶掉 cwd，是逃逸里最顺手的一种。
        let abs = root.join("outside.txt");
        assert_eq!(
            untracked_file_diff(cwd, abs.to_str().unwrap()).unwrap_err(),
            "路径超出工作目录"
        );
        // 仓库内的正常文件不受影响。
        let ok = untracked_file_diff(cwd, "ok.txt").unwrap();
        assert!(ok.diff.contains("+hello"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GBK 源码文件（中文 Windows 常见）的 diff 输出必须解出中文，而不是整屏 U+FFFD。
    #[test]
    fn stdout_text_decodes_gbk_bytes() {
        let mut raw = b"+++ b/a.txt\n+".to_vec();
        raw.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4]); // GBK 的「中文」
        raw.push(b'\n');
        let text = stdout_text(&raw);
        assert!(text.contains("中文"), "GBK diff 被解成了：{text}");
        // 纯 ASCII / UTF-8 输出照旧原样。
        assert_eq!(stdout_text("diff --git 中".as_bytes()), "diff --git 中");
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        let (text, truncated) = truncate_utf8("abc".repeat(4), 5);
        assert_eq!(text, "abcab");
        assert!(truncated);
        let (text, truncated) = truncate_utf8("中文测试".into(), 4);
        assert_eq!(text, "中");
        assert!(truncated);
        let (text, truncated) = truncate_utf8("short".into(), 100);
        assert_eq!(text, "short");
        assert!(!truncated);
    }
}
