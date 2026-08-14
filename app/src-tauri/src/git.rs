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
    let status = git_output(dir, &["status", "--porcelain", "-z", "--no-renames"])
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
    let (diff, truncated) = truncate_utf8(stdout_text(&output.stdout), MAX_DIFF_BYTES);
    Ok(GitFileDiffDto {
        path: path.to_string(),
        status: "M".into(),
        diff,
        truncated,
    })
}

/// 未跟踪文件的伪 diff：读盘（封顶 MAX_DIFF_BYTES），二进制嗅探命中则只回占位行。
fn untracked_file_diff(dir: &str, path: &str) -> Result<GitFileDiffDto, String> {
    use std::io::Read;
    let full_path = Path::new(dir).join(path);
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
    let text = String::from_utf8_lossy(&bytes);
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

fn stdout_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
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
        if path.is_empty() {
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
    fn porcelain_z_skips_short_entries_and_keeps_utf8_paths() {
        let raw = b"\0M  \0?? \xe6\x96\xb0\xe5\xbb\xba.txt\0";
        let files = parse_porcelain_z(raw);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "新建.txt");
        assert_eq!(files[0].status, "U");
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
