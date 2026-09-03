//! 钉住 kimi 工作区信任的**键算法**与真机一致。
//!
//! `trust.rs` 的单测只保证我们的实现不漂移（对着文档向量表），钉不住 kimi 换代——kimi 改了
//! `canonicalWorkspaceRoot` / `encodeWorkDirKey`，预写的记录就会落在一个它永远不读的文件名上，
//! 症状退化成「信任屏又出现了」，没有任何报错。所以这里拉起**真实** kimi：在临时目录里让它
//! 弹出信任屏、确认一次，读回 `<数据目录>/workspace-trust/` 下新出现的文件名，与
//! `meowo_agent::trust_key` 逐字符比对。取证背景见 `docs/research/kimi-workspace-trust-2026-09.md`。
//!
//! 副作用：会在用户真实 kimi 数据目录下写入一条针对临时目录的信任记录，跑完即删。
//!
//! `cargo test -p meowo-app --test probe_kimi_workspace_trust -- --ignored --nocapture`

use portable_pty::{native_pty_system, PtySize};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod common;

/// 去掉 CSI/OSC 序列，留可读文本（判信任屏用）。
fn strip_ansi(bytes: &[u8]) -> String {
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
    out
}

fn list_records(dir: &std::path::Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[ignore = "拉起真实 kimi 进程并在其数据目录写一条临时信任记录；手动调研用"]
fn kimi_trust_record_name_matches_trust_key() {
    let kimi = meowo_agent::by_id("kimi").expect("kimi 已注册");
    let inst = kimi.detect().expect("本机未检测到 kimi 数据目录");
    let spec = inst.trust.expect("kimi 的 modern 变体应声明 trust");
    assert!(inst.is_launchable(), "本机未找到 kimi 可执行");
    let argv = inst.launch_argv();
    eprintln!(
        "[probe] 变体 {} 数据目录 {}",
        inst.variant_tag,
        inst.data_dir.display()
    );

    let cwd = std::env::temp_dir().join(format!("meowo-trust-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd_str = cwd.to_str().unwrap().to_string();
    let expected = spec.record_path(&inst.data_dir, &cwd_str);
    let expected_name = expected.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(expected_name, meowo_agent::trust_key(&cwd_str));
    // 上次探针若没清干净，信任屏不会再出现——先删。
    let _ = std::fs::remove_file(&expected);
    let trust_dir = inst.data_dir.join(spec.dir_rel);
    let before = list_records(&trust_dir);
    eprintln!(
        "[probe] cwd {cwd_str}\n[probe] 期望记录 {}",
        expected.display()
    );

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = common::agent_command(&argv[0], &cwd);
    cmd.args(&argv[1..]);
    let mut child = pair.slave.spawn_command(cmd).unwrap();
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

    // 等信任屏（应答 DSR，否则部分 TUI 会等光标位置）。
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut raw = Vec::new();
    let mut saw_trust = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(1500)) {
            Ok(chunk) => {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write_all(b"\x1b[1;1R").and_then(|_| writer.flush());
                }
                // 0.40.1 启动还查询主设备属性（DA1 `ESC[c`），不应答就一直停在首帧（实测：只答
                // DSR 时 1.5s 内只收到 33 字节的查询串，画面空白）。回一个 VT220 身份即可。
                if chunk.windows(3).any(|w| w == b"\x1b[c") {
                    let _ = writer
                        .write_all(b"\x1b[?62;22c")
                        .and_then(|_| writer.flush());
                }
                raw.extend_from_slice(&chunk);
                if strip_ansi(&raw).to_lowercase().contains("trust") {
                    saw_trust = true;
                    // 再吸一小段，让整屏画完。
                    std::thread::sleep(Duration::from_millis(800));
                    while let Ok(c) = rx.try_recv() {
                        raw.extend_from_slice(&c);
                    }
                    break;
                }
            }
            // 输出停顿不等于画完（TUI 可能在等终端应答），耗到 deadline 为止。
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    let screen = strip_ansi(&raw);
    let lines: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    eprintln!(
        "[probe] 首屏（末 25 行，原始 {} 字节）:\n{}",
        raw.len(),
        lines[lines.len().saturating_sub(25)..].join("\n")
    );
    if !saw_trust {
        // 诊断：进程是否秒退（argv / 环境问题），而非信任屏文案变了。
        eprintln!("[probe] argv {argv:?}");
        eprintln!("[probe] 进程状态 {:?}", child.try_wait());
        eprintln!("[probe] 原始输出 {:?}", String::from_utf8_lossy(&raw));
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&cwd);
    }
    assert!(
        saw_trust,
        "30s 内没等到含 trust 字样的画面——信任机制可能已变"
    );

    // 确认信任：默认选项即「信任」，回车提交。kimi 走 kitty 键盘协议，但信任屏是启动早期的
    // 普通提示，裸 `\r` 即可（若某版改成要 CSI-u Enter，这里会等不到文件而失败，届时补 `ESC[13u`）。
    let _ = writer.write_all(b"\r").and_then(|_| writer.flush());

    let mut new_records = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        new_records = list_records(&trust_dir)
            .difference(&before)
            .cloned()
            .collect();
        if !new_records.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // 吸掉确认后的画面，便于失败时诊断。
    let mut after = Vec::new();
    while let Ok(c) = rx.try_recv() {
        after.extend_from_slice(&c);
    }
    let _ = child.kill();
    let _ = child.wait();

    for name in &new_records {
        let body = std::fs::read_to_string(trust_dir.join(name)).unwrap_or_default();
        eprintln!("[probe] 新记录 {name}: {body}");
    }
    if new_records.is_empty() {
        let screen = strip_ansi(&after);
        let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
        eprintln!(
            "[probe] 确认后画面（末 15 行）:\n{}",
            lines[lines.len().saturating_sub(15)..].join("\n")
        );
    }
    // 清理：只删本次产生的记录与临时目录。
    for name in &new_records {
        let _ = std::fs::remove_file(trust_dir.join(name));
    }
    let _ = std::fs::remove_dir_all(&cwd);

    assert_eq!(
        new_records.into_iter().collect::<Vec<_>>(),
        vec![expected_name],
        "kimi 写出的信任记录文件名与 trust_key 不一致——键算法换代了，按文档第 3 节复跑取证"
    );
}
