//! 进程存活探测：判定某 pid 是否仍是 agent 进程，并提供进程组/快照原语。
//! Windows 走 Toolhelp 快照 + sysinfo，macOS/Unix 走 ps。供终端聚焦、看板连接判定、存活轮询共用。
//! 从 lib.rs 抽出（纯进程逻辑，无窗口/DB 依赖）。

#[cfg(target_os = "windows")]
use sysinfo::Pid;
use sysinfo::System;
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

/// pid 对应的进程是否确实是 claude。
///
/// Windows 会复用 pid：会话结束后它的旧 pid 可能被别的进程（如 esbuild）占用，
/// 只判断「pid 是否存在」会把已结束的会话误判为仍连接。故按进程名甄别是否仍是 agent 本体——
/// 复用 meowo_agent::is_agent_process（取 basename **精确**匹配 claude/kimi 白名单，
/// 与 owner_pid 写入侧同一事实源），避免子串误匹配（如名字恰含 kimi 的无关进程）。
pub(crate) fn pid_is_agent(sys: &System, pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        sys.process(Pid::from_u32(pid as u32))
            .map(|p| meowo_agent::is_agent_process(&p.name().to_string_lossy()))
            .unwrap_or(false)
    }
    // macOS/Unix：sysinfo 对进程的可见性不稳（实测 parent() 会过早返回 None、
    // 最小刷新下 name 是否可靠也无保证），改用 ps 校验，与 meowo-reporter::owner_pid 一致。
    // 仅对「非 ended 的活跃会话」调用，每轮就几个，ps 开销可忽略。
    #[cfg(not(target_os = "windows"))]
    {
        let _ = sys;
        pid_is_agent_ps(pid)
    }
}

/// macOS/Unix：单 pid 的 agent 判活（一次 ps 按 comm 校验）。pid_is_agent 的 Unix 分支与
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

/// 一次进程表扫描 → **活着的 agent 进程 pid 集合**。
///
/// 与 [`pid_is_agent`] 判定同源（都按 basename 精确匹配 agent 白名单，防 Windows pid 复用），
/// 差别只在这里把整张表**物化成集合**：集合可以跨命令共享，`&System` 不行。
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
