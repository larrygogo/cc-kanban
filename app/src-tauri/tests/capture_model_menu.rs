//! 一次性调研工具：在真实 PTY 里把某个 agent 的 TUI 行为抓下来。
//!
//! 存在的理由：TUI 的交互（`/model` 菜单形态、composer 用哪个键提交）各家不同，
//! 只能实测，不能照着别家猜。默认 `#[ignore]`——会真的拉起 agent 进程。手动跑：
//! `cargo test -p meowo-app --test capture_model_menu -- --ignored --nocapture`
//! 或单个：`... capture_submit_key -- --ignored --nocapture`

use portable_pty::{native_pty_system, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod common;

/// 拉起 agent，自动应答 DSR（ESC[6n），等首屏画完。返回（master、输出通道、child、cwd）。
/// TUI 会用 DSR 问光标位置，收不到回应就卡在启动上——真实终端由模拟器应答，这里替它答。
type AgentSession = (
    Box<dyn Write + Send>,
    mpsc::Receiver<Vec<u8>>,
    Box<dyn portable_pty::Child + Send + Sync>,
    std::path::PathBuf,
);

fn boot_agent() -> AgentSession {
    let exe = std::env::var("MEOWO_CAPTURE_EXE").unwrap_or_else(|_| {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap();
        format!("{home}/.kimi-code/bin/kimi")
    });
    let cwd = std::env::temp_dir().join(format!("meowo-capture-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let command = common::agent_command(&exe, &cwd);
    let child = pair.slave.spawn_command(command).unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    // 等首屏画完；期间应答 DSR。
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(1500)) {
            Ok(chunk) => {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write_all(b"\x1b[1;1R");
                    let _ = writer.flush();
                }
            }
            Err(_) => break,
        }
    }
    (writer, rx, child, cwd)
}

/// 从通道收到「安静了 quiet 这么久」或超时为止。
///
/// 必须走读线程：PTY 的 `read` 是阻塞的，在主线程里直接读的话「安静」永远判定不到——
/// 它要等下一批数据来才有机会检查，而没有下一批正是安静的定义。
fn read_until_quiet(rx: &mpsc::Receiver<Vec<u8>>, quiet: Duration, cap: Duration) -> Vec<u8> {
    let start = Instant::now();
    let mut out = Vec::new();
    while start.elapsed() < cap {
        match rx.recv_timeout(quiet) {
            Ok(chunk) => out.extend_from_slice(&chunk),
            Err(_) => break,
        }
    }
    out
}

/// 同 read_until_quiet,但读取期间应答 DSR(ESC[6n)。运行中的 TUI 会反复查光标位置,
/// 收不到应答就卡住甚至退出——boot_agent 返回后就不再应答,故后续读取必须自己答。
fn read_answering_dsr(
    rx: &mpsc::Receiver<Vec<u8>>,
    writer: &mut Box<dyn Write + Send>,
    quiet: Duration,
    cap: Duration,
) -> Vec<u8> {
    let start = Instant::now();
    let mut out = Vec::new();
    while start.elapsed() < cap {
        match rx.recv_timeout(quiet) {
            Ok(chunk) => {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write_all(b"\x1b[1;1R").and_then(|_| writer.flush());
                }
                out.extend_from_slice(&chunk);
            }
            Err(_) => break,
        }
    }
    out
}

/// 实测「文字 → 隔多久 → 回车」中，间隔多短会导致回车被 composer 当成换行而非提交。
///
/// 现场：GUI 发 `/plan on` 后隔 20ms 补 `\r`，结果只换行没提交。斜杠命令要弹补全/校验，
/// composer 处理需要时间；`\r` 追太快就落进「还在编辑」的状态被当换行。这里逐个间隔试，
/// 找出可靠提交所需的最小延迟。用斜杠命令 `/plan on`（正是出问题的那条）当被测文字。
/// 取证「运行中的中断键」:发一个会跑一会儿的 prompt,读运行中的状态行(中断提示通常
/// 就在那,如 codex 的 "esc to interrupt"),再按候选中断键看是否真的停下。GUI 的「强推
/// 插话」靠它——有证据才在插件里声明 interrupt_input,宁缺毋滥。
///
/// 用法(会真的发一条 prompt、消耗额度):
///   MEOWO_CAPTURE_EXE=<cli> [MEOWO_CAPTURE_PROMPT=...] [MEOWO_CAPTURE_INTERRUPT=esc|ctrlc]
///   cargo test -p meowo-app --test capture_model_menu capture_interrupt -- --ignored --nocapture
#[test]
#[ignore = "会拉起真实 agent 进程并发一条 prompt(耗额度);仅供手动调研"]
fn capture_interrupt() {
    // boot_agent 返回后**立刻**发输入——空闲期不应答 DSR 会让某些 TUI(gemini)SessionEnd
    // 退出(能工作的 capture_model_menu 正是 boot 后马上发命令,不留空闲)。
    let (mut writer, rx, mut child, cwd) = boot_agent();
    let prompt = std::env::var("MEOWO_CAPTURE_PROMPT")
        .unwrap_or_else(|_| "写一个 300 字的科幻短故事".into());
    let send = |writer: &mut Box<dyn std::io::Write + Send>, bytes: &[u8]| -> bool {
        writer.write_all(bytes).and_then(|_| writer.flush()).is_ok()
    };
    if !send(&mut writer, prompt.as_bytes()) {
        eprintln!("### 进程不收输入(BrokenPipe):可能是 launcher shim 在 ConPTY 下已退(kimi.exe)。");
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&cwd);
        return;
    }
    std::thread::sleep(Duration::from_millis(400));
    send(&mut writer, b"\r");
    // 运行中画面:状态行通常带中断提示。读一段(回合往往持续输出,给足时间)。
    let running = read_answering_dsr(
        &rx,
        &mut writer,
        Duration::from_millis(2500),
        Duration::from_secs(15),
    );
    eprintln!(
        "=== 运行中画面(可见文本,尾部) ===\n{}",
        tail_lines(&visible(&running), 20)
    );
    eprintln!(
        "=== 运行中原始字节(尾 400,找 interrupt/stop/esc/ctrl 提示) ===\n{:?}",
        tail(&running, 400)
    );
    // 按候选中断键,看是否停下(画面出现 Interrupted / 回到 composer)。
    let key: &[u8] = match std::env::var("MEOWO_CAPTURE_INTERRUPT").as_deref() {
        Ok("ctrlc") => b"\x03",
        _ => b"\x1b",
    };
    send(&mut writer, key);
    let after = read_answering_dsr(
        &rx,
        &mut writer,
        Duration::from_millis(1500),
        Duration::from_secs(10),
    );
    eprintln!(
        "=== 中断键之后(可见文本,尾部) ===\n{}",
        tail_lines(&visible(&after), 20)
    );
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
#[ignore = "会拉起真实 agent 进程；仅供手动调研"]
fn capture_submit_key() {
    // 逐个候选提交键实测。**打印真实画面**由人判断，不靠「输出里没有原文=已提交」那种
    // 判据——输出为空时它恒真，会把读不到画面误判成成功（上一版探针正是栽在这上面）。
    let candidates: [(&str, &[u8]); 5] = [
        ("CR (\\r)", b"\r"),
        ("LF (\\n)", b"\n"),
        ("CRLF", b"\r\n"),
        // xterm 在 bracketed paste 之外发送 Enter 的常见形态；某些 TUI 只认 keypad Enter。
        ("ESC OM (keypad Enter)", b"\x1bOM"),
        ("CSI 13u (kitty Enter)", b"\x1b[13u"),
    ];
    for (label, key) in candidates {
        let (mut writer, rx, mut child, cwd) = boot_agent();
        std::thread::sleep(Duration::from_millis(1200));
        let _ = read_until_quiet(&rx, Duration::from_millis(400), Duration::from_secs(3));
        let cmd = "/plan on";
        if writer
            .write_all(cmd.as_bytes())
            .and_then(|_| writer.flush())
            .is_err()
        {
            eprintln!("### {label}: 跳过（进程不收输入）");
            let _ = child.kill();
            let _ = std::fs::remove_dir_all(&cwd);
            continue;
        }
        std::thread::sleep(Duration::from_millis(500));
        // 记下按键前的画面，便于对照按键后的变化。
        let before = visible(&read_until_quiet(
            &rx,
            Duration::from_millis(400),
            Duration::from_secs(3),
        ));
        let _ = writer.write_all(key).and_then(|_| writer.flush());
        let after = visible(&read_until_quiet(
            &rx,
            Duration::from_millis(1500),
            Duration::from_secs(10),
        ));
        eprintln!("\n### {label}");
        eprintln!("--- 按键前(末4行) ---\n{}", tail_lines(&before, 4));
        eprintln!("--- 按键后(末8行) ---\n{}", tail_lines(&after, 8));
        eprintln!(
            "--- 按键后原始字节(尾500) ---\n{:?}",
            tail(after.as_bytes(), 500)
        );
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&cwd);
    }
}

#[test]
#[ignore = "会拉起真实 agent 进程；仅供手动调研"]
fn capture_model_menu() {
    let exe = std::env::var("MEOWO_CAPTURE_EXE").unwrap_or_else(|_| {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap();
        format!("{home}/.kimi-code/bin/kimi")
    });
    let cwd = std::env::temp_dir().join("meowo-capture-model");
    std::fs::create_dir_all(&cwd).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let command = common::agent_command(&exe, &cwd);
    let mut child = pair.slave.spawn_command(command).unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let (dsr_tx, dsr_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            // TUI 会用 DSR（ESC[6n）问光标在哪，收不到回应就卡在启动上不往下画。
            // 真实终端由终端模拟器应答，这里得自己来。
            if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                let _ = dsr_tx.send(());
            }
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    // 应答线程：写端在主线程用，故这里只发信号，由主循环代写。
    std::thread::spawn(move || {
        while dsr_rx.recv().is_ok() {
            // 占位：实际应答在主线程 write（见下）。
        }
    });

    // 等首屏画完（长会话启动可能慢，给足时间）；期间按需应答 DSR。
    // 静默阈值与总时限可调:codex 启动时会先跑自动更新,期间长时间无输出,
    // 默认 1.5s 的静默判定会提前放行,把菜单命令打进还不存在的 composer。
    let boot_quiet = Duration::from_millis(env_ms("MEOWO_CAPTURE_BOOT_QUIET_MS", 1500));
    let boot_cap = Duration::from_millis(env_ms("MEOWO_CAPTURE_BOOT_TIMEOUT_MS", 45_000));
    let mut boot = Vec::new();
    let deadline = Instant::now() + boot_cap;
    while Instant::now() < deadline {
        match rx.recv_timeout(boot_quiet) {
            Ok(chunk) => {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write_all(b"\x1b[1;1R");
                    let _ = writer.flush();
                }
                boot.extend_from_slice(&chunk);
            }
            Err(_) => break,
        }
    }
    eprintln!(
        "=== 启动画面（可见文本，尾部） ===\n{}",
        tail_lines(&visible(&boot), 25)
    );

    // 菜单命令可覆盖:opencode 是 `/models`(复数),其余家是 `/model`。
    let menu_cmd = std::env::var("MEOWO_CAPTURE_MENU_CMD").unwrap_or_else(|_| "/model".into());
    // 正文与回车分两次写、隔一拍——合并写会被 TUI 当粘贴块,块内 \r 只换行不提交
    // (probe_enter.rs 的实测结论,gemini 上也复现:合并写后 /model 留在 composer 里)。
    writer.write_all(menu_cmd.as_bytes()).unwrap();
    writer.flush().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    // 提交键可选(MEOWO_CAPTURE_SUBMIT):不同 TUI 认的"回车"不同(kimi 是 kitty 协议,
    // 见 probe_enter.rs);opencode 的 \r 实测无效,要逐个试。
    let submit: &[u8] = match std::env::var("MEOWO_CAPTURE_SUBMIT").as_deref() {
        Ok("lf") => b"\n",
        Ok("crlf") => b"\r\n",
        Ok("kitty") => b"\x1b[13u",
        Ok("keypad") => b"\x1bOM",
        _ => b"\r",
    };
    writer.write_all(submit).unwrap();
    writer.flush().unwrap();
    let menu = read_until_quiet(&rx, Duration::from_millis(1500), Duration::from_secs(25));

    eprintln!(
        "=== /model 之后的原始输出（带转义，截断） ===\n{:?}",
        tail(&menu, 2500)
    );
    eprintln!("=== /model 之后的可见文本 ===\n{}", visible(&menu));

    // 交互语义取证(MEOWO_CAPTURE_PROBE_KEYS=1):菜单开着时按 ↓ 再按 Enter,观察焦点
    // 标记是否移动、Enter 是否确认——接线 GUI 按钮前必须证实这两个键的真实效果。
    if std::env::var("MEOWO_CAPTURE_PROBE_KEYS").is_ok() {
        let _ = writer.write_all(b"\x1b[B").and_then(|_| writer.flush());
        let after_down = read_until_quiet(&rx, Duration::from_millis(1200), Duration::from_secs(8));
        eprintln!("=== ↓ 之后的可见文本 ===\n{}", visible(&after_down));
        let _ = writer.write_all(b"\r").and_then(|_| writer.flush());
        let after_enter =
            read_until_quiet(&rx, Duration::from_millis(1200), Duration::from_secs(8));
        eprintln!("=== Enter 之后的可见文本 ===\n{}", visible(&after_enter));
    }

    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&cwd);
}

/// kimi 审批面板取证：发一条必然触发工具审批的 prompt，抓面板原文（选项文案/编号形态/
/// 光标标记），再按数字键证实「1/2/3 直接选中」。
///
/// **Windows 上抓不到**：kimi.exe 的 launcher shim 在裸 ConPTY 下即退（见 kimi/mod.rs
/// `interrupt_input` 注释），探针只能拿到一帧退出清理序列。kimi 审批面板的形态与按键
/// 语义已完成**官方源码取证**（MoonshotAI/kimi-code @ 0.29，approval-panel.ts），结论
/// 固化在 docs/research/tui-menu-captures-2026-07.md 与 terminalAttention.ts 的
/// `kimi:command-approval` 规则里。本探针留给非 Windows 平台复核用。
///
/// 用法（会真的发一条 prompt、消耗额度）：
///   cargo test -p meowo-app --test capture_model_menu capture_approval_panel -- --ignored --nocapture
#[test]
#[ignore = "会拉起真实 agent 进程并发一条 prompt(耗额度);仅供手动调研"]
fn capture_approval_panel() {
    let (mut writer, rx, mut child, cwd) = boot_agent();
    let send = |writer: &mut Box<dyn std::io::Write + Send>, bytes: &[u8]| {
        let _ = writer.write_all(bytes).and_then(|_| writer.flush());
    };
    // echo 带随机串，避开用户配置里可能存在的 Bash 放行规则，确保面板一定弹。
    let probe = format!("echo meowo-approval-probe-{}", std::process::id());
    send(&mut writer, format!("请立即用 Bash 工具执行 `{probe}`，不要做任何其他事。").as_bytes());
    std::thread::sleep(Duration::from_millis(400));
    send(&mut writer, b"\r");
    // 模型应答 + 工具调用 + 面板渲染，给足时间。
    let panel = read_answering_dsr(&rx, &mut writer, Duration::from_millis(3000), Duration::from_secs(90));
    eprintln!("=== 审批面板(可见文本,尾部) ===\n{}", tail_lines(&visible(&panel), 30));
    eprintln!("=== 审批面板原始字节(尾 1500,看光标/编号/样式序列) ===\n{:?}", tail(&panel, 1500));
    // 数字键直接选中(官方文档:1/2/3 直选)。按 1 批准后应看到命令真的跑了。
    send(&mut writer, b"1");
    let after = read_answering_dsr(&rx, &mut writer, Duration::from_millis(2500), Duration::from_secs(30));
    eprintln!("=== 按 1 之后(可见文本,尾部) ===\n{}", tail_lines(&visible(&after), 15));
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&cwd);
}

fn env_ms(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn tail(bytes: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(max)..].iter().collect()
}

fn tail_lines(text: &str, max: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(max)..].join("\n")
}

/// 粗剥 CSI/OSC，只为肉眼看清菜单结构；真正的还原由前端的 xterm 负责。
fn visible(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' || c == '\u{1b}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
