//! 进程存活探测：判定某 pid 是否仍是 agent 进程，并提供进程组/快照原语。
//! Windows 走 Toolhelp 快照，macOS/Unix 走 ps。供终端聚焦、看板连接判定、存活轮询共用。
//! 从 lib.rs 抽出（纯进程逻辑，无窗口/DB 依赖）。

// 两个平台都要用：Windows 的 Toolhelp 快照，以及 agent_pids_snapshot 的返回类型。
use std::collections::HashSet;

/// Toolhelp 进程快照：pid -> (父 pid, 可执行名小写)。只读元数据、不开任何进程句柄，数百进程通常
/// 1-3ms。取代 sysinfo 全进程刷新——后者在 ProcessInner::new 里对每个进程无条件 OpenProcess+
/// GetProcessTimes（与 ProcessRefreshKind 无关、关字段也省不掉），数百进程下 30-120ms。
#[cfg(target_os = "windows")]
pub(crate) fn snapshot_processes() -> std::collections::HashMap<u32, (u32, String)> {
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut map: HashMap<u32, (u32, String)> = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return map;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]).to_ascii_lowercase();
                map.insert(entry.th32ProcessID, (entry.th32ParentProcessID, name));
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    map
}

/// 收集与 root_pid 同控制台组的进程 pid：root + 所有祖先(上溯到终端宿主为止) + 所有子孙。
/// 基于 Toolhelp 快照在内存里上溯/BFS，不做全进程句柄刷新（见 snapshot_processes）。
#[cfg(target_os = "windows")]
pub(crate) fn console_group_pids(root_pid: u32) -> HashSet<u32> {
    let snapshot = snapshot_processes();
    let mut set: HashSet<u32> = HashSet::new();
    set.insert(root_pid);
    // 祖先：向上到「终端宿主」为止。遇到桌面壳/系统进程(explorer/sihost/...)就停，
    // 否则会把桌面、任务栏的窗口也算进来，点击时误聚焦到桌面。
    let boundary = [
        "explorer.exe",
        "sihost.exe",
        "svchost.exe",
        "services.exe",
        "wininit.exe",
        "winlogon.exe",
        "csrss.exe",
        "runtimebroker.exe",
        "dwm.exe",
    ];
    let terminal_host = [
        "windowsterminal.exe",
        "conhost.exe",
        "openconsole.exe",
        "wt.exe",
        "wezterm-gui.exe",
    ];
    let mut cur = root_pid;
    for _ in 0..32 {
        let Some(&(ppid, _)) = snapshot.get(&cur) else {
            break;
        };
        if ppid == 0 {
            break;
        }
        let pname = snapshot.get(&ppid).map(|(_, n)| n.as_str()).unwrap_or("");
        if boundary.contains(&pname) {
            break; // 到桌面/系统边界，停止上溯且不纳入
        }
        set.insert(ppid);
        if terminal_host.contains(&pname) {
            break; // 已纳入终端宿主，不再继续上溯
        }
        cur = ppid;
    }
    // 子孙：只从 root 自身往下 BFS（不经过祖先），否则会把终端宿主的「其它标签页」全抓进来。
    let mut frontier = vec![root_pid];
    while let Some(x) = frontier.pop() {
        for (&pid, (ppid, _)) in &snapshot {
            if *ppid == x && set.insert(pid) {
                frontier.push(pid);
            }
        }
    }
    set
}

/// macOS/Unix：单 pid 的 agent 判活（一次 ps 按 comm 校验）。terminal 的 resume 前奏与
/// macos::terminal 的 resume 回退守卫共用此单一实现，避免判活口径分叉（进程存活却被判死 →
/// 回退 resume 对运行中会话 fork 出重复会话）。
/// ps 自身 spawn 失败（瞬时故障）时保守地当「存活/未知」——调用方把 false 当「确认已死」：
/// reaper 会误收尾、聚焦回退会对运行中会话 fork 重复 resume、resume 前奏会把活 pid 当死 pid
/// 传给 revive。只有 ps 成功返回且 comm 不是 agent（含 pid 不存在时的空输出）才判死。
#[cfg(not(target_os = "windows"))]
pub(crate) fn pid_is_agent_ps(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(out) = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
    else {
        return true; // 查不了 ≠ 已死：宁可暂当存活，等下一轮能查时再判
    };
    meowo_agent::is_agent_process(String::from_utf8_lossy(&out.stdout).trim())
}

/// macOS/Unix：一次 `ps -axo pid=,comm=` 批量取「进程名含 claude」的 pid 集合，
/// 供 live_sessions_blocking 整批校验 connected，替代逐 pid spawn ps。
#[cfg(not(target_os = "windows"))]
pub(crate) fn claude_pids_snapshot() -> std::collections::HashSet<i64> {
    let mut set = std::collections::HashSet::new();
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid=,comm="])
        .output()
    else {
        return set;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.split_whitespace();
        let Some(pid) = it.next().and_then(|p| p.parse::<i64>().ok()) else {
            continue;
        };
        // comm 在 macOS 上是可执行文件全路径，可能含空格 → 余下字段拼回。
        let comm = it.collect::<Vec<_>>().join(" ");
        if meowo_agent::is_agent_process(&comm) {
            set.insert(pid);
        }
    }
    set
}

/// 纯函数：`parent_of`（pid → ppid）里 root 的全部子孙（不含 root 自身）。
/// visited 去重不只是效率：Windows 复用 pid 后，快照里陈旧的 ppid 可能指回某个子孙，
/// 构成伪环——不去重 BFS 会死循环。
pub(crate) fn descendant_pids(
    parent_of: &std::collections::HashMap<u32, u32>,
    root: u32,
) -> HashSet<u32> {
    let mut set: HashSet<u32> = HashSet::new();
    let mut frontier = vec![root];
    while let Some(x) = frontier.pop() {
        for (&pid, &ppid) in parent_of {
            if ppid == x && pid != root && set.insert(pid) {
                frontier.push(pid);
            }
        }
    }
    set
}

/// best-effort 强杀单个 pid。打不开句柄（已退出/权限不够）→ false。
/// 不做 agent 白名单校验：这里杀的是「快照瞬间 ppid 链挂在我们子进程下」的任意进程
/// （MCP server 多为 node），按名单过滤反而放跑残留。
/// pub(crate)：pty.rs 的升级链最后一档拿它直接杀 conhost（见 conhost_children）。
#[cfg(target_os = "windows")]
pub(crate) fn kill_pid(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let ok = TerminateProcess(handle, 1) != 0;
        CloseHandle(handle);
        ok
    }
}

/// unix：独立 argv 直接调 kill，不经 shell。SIGKILL——调用方已决定强杀，没有优雅退出环节。
#[cfg(not(target_os = "windows"))]
pub(crate) fn kill_pid(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// unix：一次 `ps -axo pid=,ppid=` 物化 pid → ppid 映射，供 kill_descendants 做子孙 BFS。
#[cfg(not(target_os = "windows"))]
fn snapshot_ppids() -> std::collections::HashMap<u32, u32> {
    let mut map = std::collections::HashMap::new();
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
    else {
        return map;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.split_whitespace();
        if let (Some(pid), Some(ppid)) = (
            it.next().and_then(|p| p.parse::<u32>().ok()),
            it.next().and_then(|p| p.parse::<u32>().ok()),
        ) {
            map.insert(pid, ppid);
        }
    }
    map
}

/// 强杀 root 的整棵子孙树（**不含 root 本身**——root 由调用方的 child.kill() 负责）。
/// portable-pty 不建 Job Object，kill 只及直接子进程；agent 拉起的 MCP server 等孙进程
/// 会在会话结束后残留，这里按快照兜杀。
///
/// pid 复用的缓解：快照在调用瞬间新取（Toolhelp 1-3ms），杀的窗口只有毫秒级；且只杀
/// 「快照时刻 ppid 链挂在 root 下」的 pid。自我保护：root 是本进程时直接放弃，
/// 子孙集合里出现本进程 pid（伪环/复用的极端情形）也过滤掉。
/// 本进程直接子进程中的 ConPTY 宿主（conhost.exe / OpenConsole.exe）pid 集合；
/// 其它平台无此概念，恒空集。CreatePseudoConsole 会以本进程为父 spawn 一个宿主进程
/// 却不暴露它的 pid——openpty 前后取差集即可锁定新会话的宿主，「结束会话」升级链的
/// 最后一档要拿它开刀（TerminateProcess 对卡死在 ConPTY 内核 I/O 的 agent 静默无效，
/// ClosePseudoConsole 又可能对僵死的宿主永不返回，直接杀宿主是唯一确定有效的解法）。
#[cfg(target_os = "windows")]
pub(crate) fn conhost_children() -> HashSet<u32> {
    let me = std::process::id();
    snapshot_processes()
        .into_iter()
        .filter(|(_, (ppid, name))| {
            *ppid == me && (name == "conhost.exe" || name == "openconsole.exe")
        })
        .map(|(pid, _)| pid)
        .collect()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn conhost_children() -> HashSet<u32> {
    HashSet::new()
}

pub(crate) fn kill_descendants(root: u32) {
    let self_pid = std::process::id();
    if root == self_pid {
        return;
    }
    #[cfg(target_os = "windows")]
    let parent_of: std::collections::HashMap<u32, u32> = snapshot_processes()
        .into_iter()
        .map(|(pid, (ppid, _))| (pid, ppid))
        .collect();
    #[cfg(not(target_os = "windows"))]
    let parent_of = snapshot_ppids();
    for pid in descendant_pids(&parent_of, root) {
        if pid != self_pid {
            kill_pid(pid);
        }
    }
}

/// 一次进程表扫描 → **活着的 agent 进程 pid 集合**。
///
/// 判定按 basename 精确匹配 agent 白名单（`meowo_agent::is_agent_process`，与 owner_pid
/// 写入侧同一事实源）：Windows 会复用 pid，只判「pid 是否存在」会把已结束的会话误判为
/// 仍连接。整张表**物化成集合**，跨命令/整轮轮询共享同一份快照。
///
/// 为什么要能共享：一次界面刷新会并发打好几个后端命令（见 `session_query` 的快照缓存），
/// 每个都要判活。各扫各的话，Windows 上就是好几次全进程表枚举，而且两次扫描之间进程可能退出，
/// 导致角标与列表对不上。
pub(crate) fn agent_pids_snapshot() -> HashSet<i64> {
    #[cfg(target_os = "windows")]
    {
        // Toolhelp 快照（1-3ms）而非 sysinfo 全进程刷新（30-120ms，理由见 snapshot_processes）。
        // 快照里的可执行名已是小写 basename，与 is_agent_process 的精确匹配口径一致。
        snapshot_processes()
            .into_iter()
            .filter(|(_, (_, name))| meowo_agent::is_agent_process(name))
            .map(|(pid, _)| pid as i64)
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        claude_pids_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn map(edges: &[(u32, u32)]) -> HashMap<u32, u32> {
        edges.iter().copied().collect()
    }

    #[test]
    fn descendant_pids_walks_chain_and_branches() {
        // 100 → 200 → 300，100 → 400；无关的 900(← 800) 不得入选。
        let m = map(&[(200, 100), (300, 200), (400, 100), (900, 800)]);
        let d = descendant_pids(&m, 100);
        assert_eq!(d, HashSet::from([200, 300, 400]));
    }

    #[test]
    fn descendant_pids_excludes_root_itself() {
        let m = map(&[(200, 100)]);
        assert!(!descendant_pids(&m, 100).contains(&100));
    }

    #[test]
    fn descendant_pids_survives_stale_ppid_cycles() {
        // pid 复用后的陈旧 ppid 可能构成自环/互环，BFS 必须在有限时间内返回。
        let mut m = map(&[(500, 100), (600, 500)]);
        m.insert(700, 700); // 自环
        m.insert(800, 900); // 互环（不挂在 root 下）
        m.insert(900, 800);
        let d = descendant_pids(&m, 100);
        assert_eq!(d, HashSet::from([500, 600]));
    }
}
