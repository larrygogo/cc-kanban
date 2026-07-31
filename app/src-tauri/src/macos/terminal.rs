use std::io::Write;
use std::process::{Command, Stdio};

use crate::term_script::{
    detect_term_kind, focus_script, normalize_tty, resume_script, resume_script_cwdless, TermKind,
};

/// 由 PID 取控制终端 tty，规范化为 /dev/ttysNNN。
fn tty_for_pid(pid: i64) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    normalize_tty(String::from_utf8_lossy(&out.stdout).trim())
}

/// 从 claude PID 沿父链收集进程名（claude 自身在前 → 祖先在后），用于判定终端宿主。
/// 单次 ps 快照后在内存里走 ppid —— macOS 上 sysinfo parent() 会过早断链（见 lib::pid_is_claude 注释），
/// 链一断就到不了 iTerm/Terminal，iTerm 多 tab 会话会被识成 Other 而无法聚焦，只能回退新开 Terminal。
fn ancestor_names(pid: i64) -> Vec<String> {
    ancestor_chain(pid)
        .into_iter()
        .map(|(_, comm)| comm)
        .collect()
}

/// 同上，但保留每级的 pid——attach 查重聚焦要按宿主实例的 unix id 精确置前。
fn ancestor_chain(pid: i64) -> Vec<(i64, String)> {
    let Ok(out) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,comm="])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // pid -> (ppid, comm 全路径)。comm 在 macOS 上是可执行文件全路径，含 iTerm.app/Terminal.app 便于判定。
    let mut table: std::collections::HashMap<i64, (i64, String)> = std::collections::HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(p), Some(pp)) = (it.next(), it.next()) else {
            continue;
        };
        let (Ok(p), Ok(pp)) = (p.parse::<i64>(), pp.parse::<i64>()) else {
            continue;
        };
        table.insert(p, (pp, it.collect::<Vec<_>>().join(" ")));
    }
    let mut chain = Vec::new();
    let mut cur = pid;
    for _ in 0..32 {
        let Some((ppid, comm)) = table.get(&cur) else {
            break;
        };
        if !comm.is_empty() {
            chain.push((cur, comm.clone()));
        }
        if *ppid <= 1 || *ppid == cur {
            break;
        }
        cur = *ppid;
    }
    chain
}

/// 用 stdin 传脚本、argv 传参数地运行 osascript（防注入）。返回 stdout（trim）。
/// osascript 非零退出（TCC 自动化权限被拒、AppleScript 报错）也算 Err——调用方据此
/// 判定失败（如 resume 回滚），不能把报错当成功。
fn run_osascript(script: &str, args: &[&str]) -> std::io::Result<String> {
    let mut child = Command::new("osascript")
        .arg("-") // 从 stdin 读脚本
        .args(args) // 作为 on run argv 的参数
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    // 写失败（osascript 异常秒退致 EPIPE 等）也必须走到下面的 wait——`?` 提前返回会让 child
    // 无人回收、退出后成僵尸挂在常驻进程名下。先记下错误，wait 完再传播。
    let write_err = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(script.as_bytes()).err(),
        None => None,
    };
    let out = child.wait_with_output()?;
    if let Some(e) = write_err {
        return Err(e);
    }
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "osascript 退出码 {:?}",
            out.status.code()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 尝试切到 agent 进程所在的 Terminal.app/iTerm2 tab，并保留失败原因给贴纸提示。
fn focus_existing_tab(pid: i64) -> crate::terminal::FocusSessionResult {
    let kind = detect_term_kind(&ancestor_names(pid));
    let Some(script) = focus_script(kind) else {
        return crate::terminal::FocusSessionResult::UnsupportedTerminal;
    };
    let Some(tty) = tty_for_pid(pid) else {
        return crate::terminal::FocusSessionResult::AliveButNotFound;
    };
    match run_osascript(script, &[&tty]) {
        Ok(r) if r == "FOUND" => crate::terminal::FocusSessionResult::Focused,
        Ok(_) => crate::terminal::FocusSessionResult::AliveButNotFound,
        Err(_) => crate::terminal::FocusSessionResult::PermissionDenied,
    }
}

/// attach 查重命中：把已有外部视图带到前台。pid 是 attach 客户端自身（订阅时上报），
/// 逐级降精度尝试：
/// 1. 按 tty 精确聚焦所在 tab（Terminal.app/iTerm2 有 AppleScript 字典）；
/// 2. 按宿主实例的 unix id 经 System Events 置前——Ghostty 无 AppleScript 支持，但
///    `open -na` 起的每扇窗口都是独立进程，置前该实例即置前该窗口（需辅助功能权限）；
/// 3. `open` 宿主 .app 做应用级激活——宿主必然在运行（视图进程正活在里面），不带
///    `-n` 只做激活，不会再出「凭空新起一个空白实例」的事故。
///
/// 返回是否做出了任何有效动作。
pub fn focus_attach_viewer(pid: i64) -> bool {
    if focus_existing_tab(pid) == crate::terminal::FocusSessionResult::Focused {
        return true;
    }
    let Some((host_pid, bundle)) = crate::term_script::viewer_host_entry(&ancestor_chain(pid))
    else {
        return false;
    };
    if run_osascript(
        crate::term_script::raise_process_script(),
        &[&host_pid.to_string()],
    )
    .is_ok()
    {
        return true;
    }
    Command::new("open")
        .arg(&bundle)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 点连接中的卡片：切到该 agent 进程所在的终端 tab，并返回可展示的失败原因。
/// 聚焦失败 ≠ 会话已断开：宿主可能是 VS Code/tmux/WezTerm 等无法脚本聚焦的终端（focus_script=None），
/// 或自动化权限被拒——进程仍存活时绝不能回退 resume，否则会对运行中的会话 fork 出重复会话、看板多出
/// 重复卡片（与 Windows 侧「聚焦失败只做窗口级置前、绝不 spawn 新进程」的语义对齐；macOS 无等价的
/// 窗口级手段，宁可提示用户）。进程在聚焦期间退出时也只返回 ProcessEnded，由用户明确选择恢复。
pub fn focus_session_terminal(
    pid: i64,
    cwd: Option<&str>,
    resume_argv: &[String],
    resume_kind: TermKind,
    env_prefix: &str,
) -> crate::terminal::FocusSessionResult {
    let result = focus_existing_tab(pid);
    if result == crate::terminal::FocusSessionResult::Focused {
        return result;
    }
    // 判活走 crate::pid_is_agent_ps（与 reaper/看板同一口径）：口径分叉会让「进程存活却被判死 →
    // 回退 resume 对运行中会话 fork 出重复会话」复发。
    if crate::pid_is_agent_ps(pid) {
        return result;
    }
    // 点击与进程退出竞态时交给前端提示“会话已断开”，由用户明确选择重新打开，避免静默 fork。
    let _ = (cwd, resume_argv, resume_kind, env_prefix);
    crate::terminal::FocusSessionResult::ProcessEnded
}

/// 点已断开的卡片（或跳转回退）：按设置在 Terminal.app / iTerm2 新开窗口执行 resume 命令；有 cwd 则先 cd。
/// `resume_argv` 来自 agent::resume_args（按 provider 分发：claude --resume / kimi -r / codex resume），
/// 与 Windows 共用同一事实源，不再硬编码 claude。返回 osascript 是否执行成功（失败时调用方回滚乐观复活）。
/// `env_prefix`：形如 `source '<tmp>' && rm -f '<tmp>' && ` 的前缀——环境赋值（含中转 API key）
/// 在 0600 临时文件里，密钥不进可见命令行（见 `terminal::env_source_prefix_posix`）。
/// 它作为 argv 的 **item 1** 传给 AppleScript，是唯一不套 `quoted form` 的一项——source/&& 是
/// 必须原样执行的 shell 语法，路径与文件内容已在 Rust 侧转义。
pub fn resume_session_mac(
    cwd: Option<&str>,
    resume_argv: &[String],
    kind: TermKind,
    env_prefix: &str,
) -> bool {
    if resume_argv.is_empty() {
        return false;
    }
    let mut args: Vec<&str> = Vec::with_capacity(resume_argv.len() + 2);
    args.push(env_prefix);
    match cwd {
        Some(dir) if !dir.trim().is_empty() => {
            args.push(dir);
            args.extend(resume_argv.iter().map(String::as_str));
            run_osascript(resume_script(kind), &args).is_ok()
        }
        _ => {
            args.extend(resume_argv.iter().map(String::as_str));
            run_osascript(resume_script_cwdless(kind), &args).is_ok()
        }
    }
}
