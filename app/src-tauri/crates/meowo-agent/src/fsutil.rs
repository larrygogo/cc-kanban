//! 跨 crate 共享的文件系统小工具。

/// 原子写文件（pid 后缀临时文件 + rename）：读端裸读不会见到半截文件；pid 后缀防多进程
/// 同时写同一路径时临时文件互相覆盖（吸取 kimi 凭据写回的实现）；撞名（死进程残留的 tmp
/// 恰好顶着一个被 OS 复用的 pid）时清残留、换序号重试，而不是把写入判失败。rename 失败时
/// best-effort 清理临时文件。settings.json / usage-cache.json / 各 agent 凭据写回统一走这里，
/// 消除四份各自漂移的 tmp+rename 拷贝。
///
/// 刻意**不**做成端口：它是纯 `std`，测试拿临时目录就能覆盖，注入只会平添间接层。
/// 端口留给真正需要隔离的外部世界——HTTP 与系统密钥链，见 [`crate::ports`]。
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn write_atomic_impl(
    path: &std::path::Path,
    body: &str,
    #[cfg(unix)] forced_mode: Option<u32>,
) -> std::io::Result<()> {
    // 父目录可能还不存在：opencode 的接线产物落在数据目录下的 `plugin/` 子目录里，而该子目录只有
    // 用户装过插件才会有。对既有三家这是 no-op（它们的配置就住在数据目录根上）。
    //
    // 这不与「绝不凭空创建 agent 的数据目录」相抵触：走到这里时数据目录必然已存在——`wire` 用
    // `is_configured()`（数据目录存在）作为前置门槛，不过关的 agent 根本到不了写入这一步。
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 撞名重试上限。撞名只可能来自死进程残留（同 pid 的活进程不存在、本进程内序号唯一），
    // 清掉残留后下一个序号几乎必空；重试防的是残留清不动（杀软/索引器短暂占用）又连续撞名。
    const MAX_TMP_COLLISIONS: u32 = 8;
    let mut collisions = 0;
    let (tmp, mut file) = loop {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp.{}.{seq}", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let inherited = std::fs::metadata(path)
                .ok()
                .map(|m| m.permissions().mode() & 0o777);
            options.mode(forced_mode.or(inherited).unwrap_or(0o666));
        }
        match options.open(&tmp) {
            Ok(file) => break (tmp, file),
            // create_new 撞名：上次崩溃残留的同名 tmp 恰好顶着被 OS 复用的 pid。清残留、
            // 换序号再试——整个写入不该为死进程的残渣报错。
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists
                    && collisions < MAX_TMP_COLLISIONS =>
            {
                collisions += 1;
                let _ = std::fs::remove_file(&tmp);
            }
            Err(e) => return Err(e),
        }
    };
    if let Err(e) = file
        .write_all(body.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(file);
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// 粘贴/插话附件共用的临时根目录（`$TEMP/meowo-paste`）。四个消费方必须指同一处：
/// meowo-app `chat.rs` 的粘贴附件落盘、claude transcript 的排队插话图片落盘、
/// kimi transcript 的用户消息图片落盘、`tauri.conf.json` 的 asset 协议 scope
/// （前端据此渲染缩略图）。改路径四处一起改。
pub fn paste_root() -> std::path::PathBuf {
    std::env::temp_dir().join("meowo-paste")
}

/// 把已解码的图片字节落盘到粘贴附件根的 `queued/` 下，返回可写进 `[Image: source: …]`
/// 引用的绝对路径。按 `<stem>.<ext>` 幂等——transcript 增量解析在 reset/重放时会重解析
/// 同一行，命中即复用不重写。空字节/超 [`PASTE_MAX_BYTES`] 返回 None（调用方退化为不显示图）。
pub fn persist_paste_bytes(stem: &str, ext: &str, bytes: &[u8]) -> Option<std::path::PathBuf> {
    if bytes.is_empty() || bytes.len() > PASTE_MAX_BYTES {
        return None;
    }
    let dir = paste_root().join("queued");
    let path = dir.join(format!("{stem}.{ext}"));
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 0 {
            return Some(path);
        }
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// 跨 provider 切换的交接文件根目录（`$TEMP/meowo-handoff`）。文件由目标 agent 用
/// 自己的读文件工具读取，不经 webview，故**不需要**进 asset scope——与 meowo-paste
/// 的差别仅此一点。回收由 meowo-app 启动时的 TTL 清理负责（chat::spawn_paste_cleanup
/// 同时扫这个根），OS 临时目录策略靠不住（Windows 默认不清 %TEMP%）。
pub fn handoff_root() -> std::path::PathBuf {
    std::env::temp_dir().join("meowo-handoff")
}

/// 粘贴/插话附件的单文件上限。覆盖截图与常规文档；防的是超大 payload 把 base64 +
/// IPC 序列化拖住、把临时目录写爆。粘贴附件与排队插话图片同额度（此前两处各写 32MB）。
pub const PASTE_MAX_BYTES: usize = 32 * 1024 * 1024;

/// 扫掉 `dir` 下 [`write_atomic`] 遗留的临时文件（`<名字>.tmp.<pid>.<序号>`）。
///
/// 起因：原子写崩在「建 tmp」与「rename」之间时，tmp 就永久留在盘上。写入侧只在
/// **撞名**时清残留（见 `write_atomic_impl`），而撞名要求 pid 被 OS 复用且序号也相同
/// ——实际等于永不发生。实测用户的 `~/.meowo` 里躺着两个多月前的
/// `proxy-applied.tmp.30108.54`，无人回收。
///
/// 判据用**年龄**而非「pid 是否存活」：一次原子写从建 tmp 到 rename 是毫秒级，
/// 超过 [`STALE_TEMP_AGE`] 的 tmp 必然是死进程的残渣。这样既不需要查进程表，
/// 也不可能误删正在飞的写入——而 pid 判据反倒有 pid 复用的误判面。
///
/// 只认自己的命名（`.tmp.<纯数字>.<纯数字>` 结尾），不碰任何别的文件。
pub fn sweep_stale_temp_files(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_write_atomic_temp(name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age > STALE_TEMP_AGE);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 超过这个年龄的 tmp 残留即可回收。取 1 小时是大幅留白:原子写本身是毫秒级,
/// 一小时足以覆盖任何被杀软/索引器拖慢的极端情况,又远短于"下次开机"。
const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// 是否是 [`write_atomic`] 造的临时文件名：以 `.tmp.<纯数字>.<纯数字>` 结尾。
/// 收得这么紧是刻意的——清扫器跑在用户的数据目录上，宁可漏扫也不能误删。
fn is_write_atomic_temp(name: &str) -> bool {
    let Some((rest, seq)) = name.rsplit_once('.') else {
        return false;
    };
    let Some((rest, pid)) = rest.rsplit_once('.') else {
        return false;
    };
    rest.ends_with(".tmp")
        && !seq.is_empty()
        && !pid.is_empty()
        && seq.bytes().all(|b| b.is_ascii_digit())
        && pid.bytes().all(|b| b.is_ascii_digit())
}

pub fn write_atomic(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    write_atomic_impl(
        path,
        body,
        #[cfg(unix)]
        None,
    )
}

/// 原子写敏感文件。Unix 上临时文件从创建起即为 0600，避免 rename 后再 chmod 的暴露窗口。
pub fn write_atomic_secure(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    write_atomic_impl(
        path,
        body,
        #[cfg(unix)]
        Some(0o600),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn write_atomic_replaces_content_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("meowo-fsutil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.json");
        super::write_atomic(&p, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\":1}");
        super::write_atomic(&p, "{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\":2}");
        // 临时文件不残留。
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("tmp."))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 上次崩溃残留的同名 tmp + OS 复用 pid + 本进程首次写该路径：create_new 撞名
    /// 不该让整个写入失败（修复前 AlreadyExists 直接上抛，写入报错）。
    #[test]
    fn stale_tmp_with_recycled_pid_does_not_fail_the_write() {
        let dir = std::env::temp_dir().join(format!("meowo-fsutil-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.json");

        // 按当前 pid 与接下来的全局序号预占一批临时名，模拟死进程残留。多占几个以吸收
        // 并发测试同时消耗序号的抖动（数量低于撞名重试上限，写入必然成功）。
        let base = super::TMP_SEQ.load(std::sync::atomic::Ordering::Relaxed);
        for seq in base..base + 4 {
            let stale = p.with_extension(format!("tmp.{}.{seq}", std::process::id()));
            std::fs::write(&stale, "半截的崩溃残留").unwrap();
        }

        super::write_atomic(&p, "{\"ok\":true}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"ok\":true}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_atomic_writers_do_not_share_a_temp_file() {
        let dir = std::env::temp_dir().join(format!(
            "meowo-fsutil-concurrent-{}-{}",
            std::process::id(),
            super::TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    super::write_atomic(&path, &format!("writer-{i}"))
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("writer-"));
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains("tmp.")),
            "并发写入后不得残留临时文件"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_permissions_and_secure_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("meowo-fsutil-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inherited = dir.join("inherited.json");
        std::fs::write(&inherited, "old").unwrap();
        std::fs::set_permissions(&inherited, std::fs::Permissions::from_mode(0o640)).unwrap();
        super::write_atomic(&inherited, "new").unwrap();
        assert_eq!(
            std::fs::metadata(&inherited).unwrap().permissions().mode() & 0o777,
            0o640
        );

        let secret = dir.join("secret.json");
        super::write_atomic_secure(&secret, "secret").unwrap();
        assert_eq!(
            std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::{is_write_atomic_temp, sweep_stale_temp_files, write_atomic, STALE_TEMP_AGE};
    use std::time::{Duration, SystemTime};

    /// 命名判据要收得足够紧:清扫器跑在用户的数据目录上,认错一个名字就是删用户文件。
    #[test]
    fn only_write_atomic_temp_names_are_recognized() {
        assert!(is_write_atomic_temp("settings.json.tmp.1234.7"));
        assert!(is_write_atomic_temp("proxy-applied.tmp.30108.54"));
        // 少一段、段里不是纯数字、或压根没有 .tmp 的,一律不认。
        assert!(!is_write_atomic_temp("settings.json"));
        assert!(!is_write_atomic_temp("settings.json.tmp"));
        assert!(!is_write_atomic_temp("settings.json.tmp.1234"));
        assert!(!is_write_atomic_temp("settings.json.tmp.abc.7"));
        assert!(!is_write_atomic_temp("settings.json.tmp.1234.x"));
        // 用户盘上真实存在过的邻居名字,一个都不能沾。
        assert!(!is_write_atomic_temp("board.db-wal.bak-20260724-095405"));
        assert!(!is_write_atomic_temp("relay-secrets.json"));
    }

    /// 只删「够老的」tmp:新鲜 tmp 可能是另一个实例正在飞的写入,删了就打断它的 rename。
    #[test]
    fn sweeps_only_aged_temp_files_and_never_real_files() {
        let dir =
            std::env::temp_dir().join(format!("meowo-fsutil-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 真文件 + 一个名字很像但不合格的:一个都不能少。
        write_atomic(&dir.join("settings.json"), "{}").unwrap();
        std::fs::write(dir.join("settings.json.tmp.1234"), b"x").unwrap();
        // 新鲜的 tmp 残留:还在保护期内,不动。
        std::fs::write(dir.join("fresh.json.tmp.999.1"), b"x").unwrap();
        // 陈旧的 tmp 残留:该删。File::set_modified 把 mtime 拨回保护期之前。
        let stale = dir.join("proxy-applied.tmp.30108.54");
        std::fs::write(&stale, b"x").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(SystemTime::now() - STALE_TEMP_AGE - Duration::from_secs(60))
            .unwrap();

        sweep_stale_temp_files(&dir);

        assert!(dir.join("settings.json").exists(), "真文件被误删");
        assert!(
            dir.join("settings.json.tmp.1234").exists(),
            "名字不合格的文件被误删"
        );
        assert!(dir.join("fresh.json.tmp.999.1").exists(), "新鲜 tmp 被误删");
        assert!(!stale.exists(), "陈旧 tmp 没被回收");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录不存在时静默返回——启动路径上不该因为这个失败(首次运行时 ~/.meowo 可能还没建)。
    #[test]
    fn missing_directory_is_not_an_error() {
        sweep_stale_temp_files(
            &std::env::temp_dir().join("meowo-sweep-probe-definitely-missing-9f8d7c"),
        );
    }
}
