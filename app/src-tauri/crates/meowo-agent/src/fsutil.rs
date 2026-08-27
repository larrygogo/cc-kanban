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
