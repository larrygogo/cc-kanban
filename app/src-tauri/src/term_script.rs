//! 跨平台纯逻辑：供 macOS 终端跳转使用，但不依赖 macOS API，便于在任意平台单测。
#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermKind {
    Terminal,
    ITerm2,
    Other,
}

/// 把 `ps -o tty=` 的输出规范化成 `/dev/ttysNNN`；无控制终端返回 None。
pub fn normalize_tty(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t == "??" || t == "?" {
        return None;
    }
    if let Some(rest) = t.strip_prefix("/dev/") {
        return if rest.is_empty() {
            None
        } else {
            Some(format!("/dev/{rest}"))
        };
    }
    if let Some(num) = t.strip_prefix("ttys") {
        return Some(format!("/dev/ttys{num}"));
    }
    if let Some(num) = t.strip_prefix('s') {
        // ps 偶尔返回 's003'
        return Some(format!("/dev/ttys{num}"));
    }
    Some(format!("/dev/{t}"))
}

/// 进程名按「从 claude 自身向祖先」顺序传入，返回最近的已知终端宿主。
pub fn detect_term_kind(ancestor_names_root_first: &[String]) -> TermKind {
    for name in ancestor_names_root_first {
        let n = name.to_ascii_lowercase();
        if n.contains("iterm") {
            return TermKind::ITerm2;
        }
        if n == "terminal" || n.contains("terminal.app") {
            return TermKind::Terminal;
        }
    }
    TermKind::Other
}

/// attach 查重命中但按 tty 精确聚焦失败（Ghostty/WezTerm 等无 AppleScript 聚焦的宿主）时，
/// 聚焦目标从订阅者 pid 的祖先链推导：取首个 .app 进程的 (pid, bundle 路径)。
/// pid 供 [`raise_process_script`] 按 unix id 精确置前——`open -na` 的 Ghostty 每窗口
/// 一个实例，置前该实例即置前该窗口；bundle 路径供置前失败后 `open` 做应用级激活。
/// 第一项是 attach 客户端自身，必须跳过——生产环境它就在 Meowo.app 里，不跳会把
/// Meowo 自己带到前台。
pub fn viewer_host_entry(ancestors_self_first: &[(i64, String)]) -> Option<(i64, String)> {
    ancestors_self_first.iter().skip(1).find_map(|(pid, comm)| {
        let end = comm.find(".app/")?;
        Some((*pid, comm[..end + 4].to_string()))
    })
}

/// 把指定 unix id 的进程置前的 AppleScript（pid 经 osascript argv 传入，防注入）。
/// 按 pid 而非进程名寻址：`open -na` 起的多个 Ghostty 实例同名，按名只会命中第一个。
/// 进程不存在或无辅助功能权限时 osascript 非零退出，调用方据此走应用级激活兜底。
pub fn raise_process_script() -> &'static str {
    r#"on run argv
	set targetPid to (item 1 of argv) as integer
	tell application "System Events"
		set frontmost of (first process whose unix id is targetPid) to true
	end tell
	return "FOUND"
end run"#
}

/// 设置里的字符串 → 打开未连接会话用的终端宿主：含 "iterm" → iTerm2，其余（含 "terminal"/未知/空）→ Terminal。
pub fn resume_kind_from_setting(s: &str) -> TermKind {
    if s.to_ascii_lowercase().contains("iterm") {
        TermKind::ITerm2
    } else {
        TermKind::Terminal
    }
}

/// 返回按 tty 定位并置前的 AppleScript（tty 通过 osascript argv 传入）。未知宿主返回 None。
pub fn focus_script(kind: TermKind) -> Option<&'static str> {
    match kind {
        TermKind::Terminal => Some(
            r#"on run argv
  set targetTTY to item 1 of argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if (tty of t) is targetTTY then
          set selected of t to true
          set frontmost of w to true
          activate
          return "FOUND"
        end if
      end repeat
    end repeat
  end tell
  return "NOT_FOUND"
end run"#,
        ),
        TermKind::ITerm2 => Some(
            r#"on run argv
  set targetTTY to item 1 of argv
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if (tty of s) is targetTTY then
            select w
            select t
            select s
            activate
            return "FOUND"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  return "NOT_FOUND"
end run"#,
        ),
        TermKind::Other => None,
    }
}

/// 返回新开终端执行 `cd <cwd> && <env 前缀><resume 命令>` 的 AppleScript。
///
/// argv 约定：**item 1 = env 前缀**，item 2 = cwd，
/// item 3..N = resume 命令 argv（来自 agent::resume_args，逐项 quoted form 拼接防注入）——命令由
/// 调用方按 provider 分发（claude/kimi/codex 各异），本脚本不硬编码 claude，使 macOS 与 Windows
/// 共用同一 provider 事实源。
///
/// env 前缀形如 `source '<tmp>' && rm -f '<tmp>' && `：赋值（含中转 API key）写在 0600 临时
/// 文件里由命令自行 source 并删除，密钥值不进可见命令行（见 `terminal::env_source_prefix_posix`）。
/// 它是**唯一不套 `quoted form`** 的一项——source/&& 是必须原样执行的 shell 语法；文件路径与
/// 文件内容已在 Rust 侧按 POSIX 单引号规则转义，拼进来是安全的。
pub fn resume_script(kind: TermKind) -> &'static str {
    match kind {
        TermKind::ITerm2 => {
            r#"on run argv
  set envPrefix to item 1 of argv
  set targetDir to item 2 of argv
  set theCmd to "cd " & quoted form of targetDir & " && " & envPrefix & quoted form of item 3 of argv
  repeat with i from 4 to count of argv
    set theCmd to theCmd & " " & quoted form of item i of argv
  end repeat
  tell application "iTerm2"
    activate
    set newWindow to (create window with default profile)
    tell current session of newWindow to write text theCmd
  end tell
end run"#
        }
        // Terminal 与 Other(回退到 Terminal) 共用
        _ => {
            r#"on run argv
  set envPrefix to item 1 of argv
  set targetDir to item 2 of argv
  set theCmd to "cd " & quoted form of targetDir & " && " & envPrefix & quoted form of item 3 of argv
  repeat with i from 4 to count of argv
    set theCmd to theCmd & " " & quoted form of item i of argv
  end repeat
  tell application "Terminal"
    activate
    do script theCmd
  end tell
end run"#
        }
    }
}

/// 无 cwd 时的恢复脚本（item 1 = env 前缀，item 2..N = resume 命令 argv，不 cd），
/// 镜像 Windows 在 cwd 缺失时不带 -d 的行为。
pub fn resume_script_cwdless(kind: TermKind) -> &'static str {
    match kind {
        TermKind::ITerm2 => {
            r#"on run argv
  set envPrefix to item 1 of argv
  set theCmd to envPrefix & quoted form of item 2 of argv
  repeat with i from 3 to count of argv
    set theCmd to theCmd & " " & quoted form of item i of argv
  end repeat
  tell application "iTerm2"
    activate
    set newWindow to (create window with default profile)
    tell current session of newWindow to write text theCmd
  end tell
end run"#
        }
        _ => {
            r#"on run argv
  set envPrefix to item 1 of argv
  set theCmd to envPrefix & quoted form of item 2 of argv
  repeat with i from 3 to count of argv
    set theCmd to theCmd & " " & quoted form of item i of argv
  end repeat
  tell application "Terminal"
    activate
    do script theCmd
  end tell
end run"#
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tty_variants() {
        assert_eq!(normalize_tty("ttys003"), Some("/dev/ttys003".into()));
        assert_eq!(normalize_tty("s003"), Some("/dev/ttys003".into()));
        assert_eq!(normalize_tty("/dev/ttys012"), Some("/dev/ttys012".into()));
        assert_eq!(normalize_tty("  ttys004  "), Some("/dev/ttys004".into()));
        assert_eq!(normalize_tty("??"), None);
        assert_eq!(normalize_tty(""), None);
    }

    #[test]
    fn detect_term_kind_picks_nearest_known_host() {
        let names = vec![
            "claude".to_string(),
            "zsh".to_string(),
            "login".to_string(),
            "iTerm2".to_string(),
            "launchd".to_string(),
        ];
        assert_eq!(detect_term_kind(&names), TermKind::ITerm2);

        let names2 = vec!["claude".into(), "zsh".into(), "Terminal".into()];
        assert_eq!(detect_term_kind(&names2), TermKind::Terminal);

        let names3 = vec!["claude".into(), "zsh".into(), "WezTerm".into()];
        assert_eq!(detect_term_kind(&names3), TermKind::Other);
    }

    #[test]
    fn resume_kind_from_setting_maps() {
        assert_eq!(resume_kind_from_setting("iterm"), TermKind::ITerm2);
        assert_eq!(resume_kind_from_setting("iTerm2"), TermKind::ITerm2);
        assert_eq!(resume_kind_from_setting("terminal"), TermKind::Terminal);
        assert_eq!(resume_kind_from_setting(""), TermKind::Terminal); // 缺省/未知 → Terminal
        assert_eq!(resume_kind_from_setting("wezterm"), TermKind::Terminal);
    }

    /// 聚焦目标从订阅者 pid 的祖先链推导：跳过第一项（attach 客户端自身——
    /// 生产环境它就在 Meowo.app 里，不跳会把 Meowo 自己带到前台），取其后首个
    /// .app 进程的 (pid, bundle 路径)。pid 用于 System Events 按 unix id 精确置前
    /// （`open -na` 的 Ghostty 每窗口一个实例），bundle 路径用于置前失败后 `open` 激活。
    #[test]
    fn viewer_host_entry_skips_self_and_finds_the_host_bundle() {
        let chain = vec![
            (
                100,
                "/Applications/Meowo.app/Contents/MacOS/meowo-reporter".to_string(),
            ),
            (90, "/bin/zsh".to_string()),
            (
                80,
                "/Applications/Ghostty.app/Contents/MacOS/ghostty".to_string(),
            ),
        ];
        assert_eq!(
            viewer_host_entry(&chain),
            Some((80, "/Applications/Ghostty.app".to_string()))
        );

        let dev = vec![
            (100, "target/debug/meowo-reporter".to_string()),
            (90, "/bin/zsh".to_string()),
            (
                80,
                "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal".to_string(),
            ),
        ];
        assert_eq!(
            viewer_host_entry(&dev),
            Some((80, "/System/Applications/Utilities/Terminal.app".to_string()))
        );

        let self_only = vec![(
            100,
            "/Applications/Meowo.app/Contents/MacOS/meowo-reporter".to_string(),
        )];
        assert_eq!(viewer_host_entry(&self_only), None, "只有自身不算宿主");
        assert_eq!(viewer_host_entry(&[]), None);
        let no_bundle = vec![
            (100, "meowo-reporter".to_string()),
            (90, "/bin/zsh".to_string()),
        ];
        assert_eq!(viewer_host_entry(&no_bundle), None, "无 .app 祖先 → None");
    }

    /// 置前脚本按 unix id 找进程（pid 经 argv 传入，防注入），不依赖进程名——
    /// `open -na` 起的多个 Ghostty 实例同名，按名寻址只会命中第一个。
    #[test]
    fn raise_process_script_targets_unix_id_via_argv() {
        let s = raise_process_script();
        assert!(s.contains("on run argv"));
        assert!(s.contains("item 1 of argv"));
        assert!(s.contains("unix id"));
        assert!(s.contains("frontmost"));
    }

    #[test]
    fn focus_script_present_for_known_hosts_only() {
        assert!(focus_script(TermKind::Terminal)
            .unwrap()
            .contains("tty of t"));
        assert!(focus_script(TermKind::ITerm2).unwrap().contains("tty of s"));
        assert!(focus_script(TermKind::Other).is_none());
    }

    #[test]
    fn resume_script_uses_argv_and_quoted_form() {
        for kind in [TermKind::Terminal, TermKind::ITerm2, TermKind::Other] {
            // 带 cwd：item 1 = env 前缀，item 2 = 目录，命令 argv 从 item 3 起逐项 quoted form 拼接。
            let s = resume_script(kind);
            assert!(s.contains("on run argv"));
            assert!(s.contains("set envPrefix to item 1 of argv"));
            assert!(s.contains("set targetDir to item 2 of argv"));
            assert!(s.contains("repeat with i from 4 to count of argv"));
            assert!(s.contains("quoted form of item i of argv"));
            // 命令由 agent::resume_args 按 provider 提供，脚本不得再硬编码 claude。
            assert!(!s.contains("claude"));
            // 无 cwd：item 1 = env 前缀，item 2 即命令首项，其余从 item 3 起拼接，且不 cd。
            let c = resume_script_cwdless(kind);
            assert!(c.contains("on run argv"));
            assert!(c.contains("set envPrefix to item 1 of argv"));
            assert!(c.contains("set theCmd to envPrefix & quoted form of item 2 of argv"));
            assert!(c.contains("repeat with i from 3 to count of argv"));
            assert!(!c.contains("claude"));
            assert!(!c.contains("cd "));
        }
    }

    /// env 前缀是唯一**不套** `quoted form` 的一项——它是必须原样执行的 shell 语法
    /// （`source '<tmp>' && rm -f '<tmp>' && `），quote 起来就成了一个命令名。命令 argv
    /// 则必须逐项 quoted form（防注入），两者不能混为一谈。
    #[test]
    fn env_prefix_is_the_only_unquoted_item() {
        for kind in [TermKind::Terminal, TermKind::ITerm2] {
            for s in [resume_script(kind), resume_script_cwdless(kind)] {
                assert!(
                    s.contains("envPrefix & quoted form of item"),
                    "env 前缀应原样拼在命令之前"
                );
                assert!(
                    !s.contains("quoted form of envPrefix"),
                    "env 前缀绝不能被 quoted form 包起来"
                );
            }
        }
    }
}
