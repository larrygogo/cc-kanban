//! 工作区信任预写：起进程前替用户把工作目录写进 agent 自己的信任名册，免掉「是否信任此目录」
//! 确认屏。
//!
//! 为什么要做：对话页只渲染结构化 chat，看不到也答不了这道 TUI 提示；信任屏期间 agent 不触发
//! 任何 hook，新会话不落库——跨 agent 切换（`switch_session_provider`）表现为「切换后什么都
//! 没了」。取证、算法向量与安全权衡全在 `docs/research/kimi-workspace-trust-2026-09.md`
//! （活文档，**勿移动**）；kimi 改算法时按该文件的复跑方法重新取证，别凭记忆改这里。
//!
//! 本模块 provider 无关：谁声明了 [`WorkspaceTrustSpec`] 谁就走这条路。目前只有 kimi
//! （0.40.x，`<数据目录>/workspace-trust/<key>`，内容 `{"root","trustedAt"}`，读端只看文件
//! 是否存在，不校验字段、不继承父目录、root 取 cwd）。
//!
//! 边界：只对用户本次主动发起的启动预写，只写 agent 自己会写的两个字段，不碰 `workspaces.json`。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 某 agent 的工作区信任名册形态。声明为 `static`，挂在 [`crate::variant::Variant::trust`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceTrustSpec {
    /// 信任记录目录，相对数据目录（kimi：`"workspace-trust"`）。
    pub dir_rel: &'static str,
}

impl WorkspaceTrustSpec {
    /// 某工作目录的信任记录落盘位置：`<data_dir>/<dir_rel>/<key>`，**无扩展名**。
    pub fn record_path(&self, data_dir: &Path, cwd: &str) -> PathBuf {
        data_dir.join(self.dir_rel).join(trust_key(cwd))
    }

    /// 预写信任记录。已存在则分毫不动（幂等；kimi 自己写的记录带它自己的 `trustedAt`，
    /// 不该被我们盖掉）。返回 `Ok(true)` = 本次写入，`Ok(false)` = 早已存在。
    ///
    /// 「只在不存在时创建」交给内核（`create_new`），而不是 `exists()` 再写：后者在两次启动
    /// 同时预写、或与 kimi 自己落盘撞车时，后到的 rename 会盖掉先写的记录（PR #68 review）。
    /// 不走 `write_atomic`：它的 tmp+rename 正是会覆盖的那一步；记录只有百余字节、一次
    /// `write_all` 落盘，读端又只看文件是否存在，半截文件的窗口没有实际后果。
    ///
    /// 调用方须保证 `data_dir` 已存在（「绝不凭空创建 agent 的数据目录」）；这里只补
    /// `dir_rel` 这一层子目录——kimi 首次信任时也是这样建它的。
    pub fn pretrust(&self, data_dir: &Path, cwd: &str) -> std::io::Result<bool> {
        use std::io::Write as _;
        let path = self.record_path(data_dir, cwd);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => return Err(e),
        };
        // `root` 原样写 cwd（kimi 写的就是 `workspace.cwd`），经 serde 转义，反斜杠不会写坏 JSON。
        let body = serde_json::json!({ "root": cwd, "trustedAt": now_ms() }).to_string();
        if let Err(e) = file
            .write_all(body.as_bytes())
            .and_then(|()| file.sync_all())
        {
            // 写失败别留一个空壳：空文件在读端同样算「已信任」，会把这次失败永久化。
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        Ok(true)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Windows 形状的路径：盘符（`C:/`）或 UNC（`//server/share`）。此形状下整体折小写——NTFS 查找
/// 默认大小写不敏感，kimi 0.40.x 的 `canonicalWorkspaceRoot` 正是据此把 `C:\Proj` 与 `c:\proj`
/// 视为同一工作区。POSIX 路径不折。
fn is_windows_shaped(slashed: &str) -> bool {
    let b = slashed.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/')
        || slashed.starts_with("//")
}

/// kimi 的 `canonicalWorkspaceRoot`：`\`→`/`、纯字符串折叠 `.`/`..`（不碰文件系统）、去尾部 `/`、
/// Windows 形状整体小写。
///
/// **绝不能用 `fs::canonicalize` 替代**：它在 Windows 上返回 `\\?\C:\…` 前缀，哈希会完全对不上。
/// 原文用 `path.win32.resolve`，会给相对路径补上进程 cwd、给 POSIX 路径补盘符——meowo 只会传
/// 本机绝对路径，那些畸形输入不复刻。
pub fn canonical_root(cwd: &str) -> String {
    let slashed = cwd.replace('\\', "/");
    let shaped = is_windows_shaped(&slashed);
    // 根前缀：折叠时不可越过。盘符 `C:` / UNC `//server/share` / POSIX 空串（前导 `/`）。
    let (root, rest): (String, &str) = if let Some(body) = slashed.strip_prefix("//") {
        // UNC：server 与 share 两段是根。
        let mut it = body.splitn(3, '/');
        let server = it.next().unwrap_or("");
        let share = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("");
        (format!("//{server}/{share}"), rest)
    } else if shaped {
        (slashed[..2].to_string(), &slashed[3..])
    } else if let Some(rest) = slashed.strip_prefix('/') {
        (String::new(), rest)
    } else {
        (String::new(), slashed.as_str())
    };
    let mut segs: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    let body = segs.join("/");
    if shaped {
        // `C:\` → `c:`（尾部 `/` 已去）；UNC 同理。
        let joined = if body.is_empty() {
            root
        } else {
            format!("{root}/{body}")
        };
        joined.to_lowercase()
    } else if slashed.starts_with('/') {
        // POSIX 绝对路径：根就是那个前导 `/`。
        format!("/{body}")
    } else {
        body
    }
}

/// kimi 的 `workdir-slug.ts`：小写 → 非 `[a-z0-9._-]` 连续段替换为 `-` → 去首尾 `-` → 截前 40 →
/// 再去首尾 `-`；空 / `.` / `..` 时用 `workspace`。
pub fn slugify_workdir(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_run = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    // 替换后只剩 ASCII，按字节截取与 JS 的 `slice(0, 40)` 等价。
    let cut = &trimmed[..trimmed.len().min(40)];
    let cut = cut.trim_matches('-');
    if cut.is_empty() || cut == "." || cut == ".." {
        "workspace".to_string()
    } else {
        cut.to_string()
    }
}

/// `wd_<slug(basename)>_<sha256(canonical) 前 12 位十六进制>`。
pub fn trust_key(cwd: &str) -> String {
    let canonical = canonical_root(cwd);
    let base = canonical.rsplit('/').next().unwrap_or("");
    let digest = Sha256::digest(canonical.as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("wd_{}_{hex}", slugify_workdir(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文档第 3 节的向量表：实现不得漂移。与 kimi 实际行为的一致性由真机探针
    /// （`tests/probe_kimi_workspace_trust.rs`）钉住——这里只钉我们自己。
    #[test]
    fn keys_match_the_documented_vectors() {
        let cases: &[(&str, &str, &str)] = &[
            (r"C:\proj\demo", "c:/proj/demo", "wd_demo_85b7c7a9d059"),
            (r"C:\Proj\Demo", "c:/proj/demo", "wd_demo_85b7c7a9d059"),
            (r"C:\proj\demo\", "c:/proj/demo", "wd_demo_85b7c7a9d059"),
            ("C:/proj/demo", "c:/proj/demo", "wd_demo_85b7c7a9d059"),
            (
                r"C:\proj\sub\..\demo",
                "c:/proj/demo",
                "wd_demo_85b7c7a9d059",
            ),
            (r"C:\proj\.\demo", "c:/proj/demo", "wd_demo_85b7c7a9d059"),
            (
                r"C:\Users\alice\my.app",
                "c:/users/alice/my.app",
                "wd_my.app_e93fb0a9fae4",
            ),
            (r"C:\", "c:", "wd_c_826c0d7d3c42"),
            (
                "/home/alice/proj",
                "/home/alice/proj",
                "wd_proj_2af6b4a70d44",
            ),
            (
                r"C:\proj\我的项目",
                "c:/proj/我的项目",
                "wd_workspace_bf2803e54aa6",
            ),
            (
                r"\\server\share\proj",
                "//server/share/proj",
                "wd_proj_49ea924784e1",
            ),
        ];
        for (cwd, canonical, key) in cases {
            assert_eq!(canonical_root(cwd), *canonical, "canonical of {cwd:?}");
            assert_eq!(trust_key(cwd), *key, "key of {cwd:?}");
        }
        // 超长目录名：slug 截前 40。
        let long = format!(r"C:\proj\{}", "a".repeat(45));
        assert_eq!(
            trust_key(&long),
            format!("wd_{}_827f2d96c8a3", "a".repeat(40))
        );
    }

    #[test]
    fn posix_paths_keep_case_and_fold_dots() {
        assert_eq!(
            canonical_root("/Home/Alice/../alice/proj/"),
            "/Home/alice/proj"
        );
        assert_eq!(canonical_root("/"), "/");
        // `..` 不可越过根。
        assert_eq!(canonical_root(r"C:\..\..\x"), "c:/x");
        assert_eq!(canonical_root(r"\\srv\share\..\..\x"), "//srv/share/x");
    }

    #[test]
    fn slug_rules_match_kimi() {
        assert_eq!(slugify_workdir("Meowo App"), "meowo-app");
        assert_eq!(slugify_workdir("--a--"), "a");
        assert_eq!(slugify_workdir(""), "workspace");
        assert_eq!(slugify_workdir(".."), "workspace");
        assert_eq!(slugify_workdir("我的"), "workspace");
        // 截断后再去首尾 `-`：第 40 位恰好落在连字符上。
        let s = format!("{}-{}", "b".repeat(39), "c".repeat(5));
        assert_eq!(slugify_workdir(&s), "b".repeat(39));
    }

    #[test]
    fn pretrust_writes_once_and_never_overwrites() {
        let data_dir = std::env::temp_dir().join(format!("meowo-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).unwrap();
        let spec = WorkspaceTrustSpec {
            dir_rel: "workspace-trust",
        };
        let cwd = r"C:\proj\demo";
        let path = spec.record_path(&data_dir, cwd);
        assert_eq!(
            path,
            data_dir
                .join("workspace-trust")
                .join("wd_demo_85b7c7a9d059")
        );

        assert!(spec.pretrust(&data_dir, cwd).unwrap());
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(body["root"], cwd);
        assert!(body["trustedAt"].as_u64().is_some_and(|t| t > 0));
        assert_eq!(
            body.as_object().unwrap().len(),
            2,
            "只写 kimi 自己会写的两个字段"
        );

        // 已存在：不动（内容原样，哪怕是 kimi 自己写的别样内容）。
        std::fs::write(&path, "{\"root\":\"x\",\"trustedAt\":1}").unwrap();
        assert!(!spec.pretrust(&data_dir, cwd).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"root\":\"x\",\"trustedAt\":1}"
        );
        // 归一等价：大小写/斜杠不同的同一目录命中同一条记录。
        assert!(!spec.pretrust(&data_dir, "c:/PROJ/demo/").unwrap());
        // 临时文件不残留。
        let leftovers: Vec<_> = std::fs::read_dir(data_dir.join("workspace-trust"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "{leftovers:?}");

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// 并发预写同一目录：恰好一方写入，其余全部报「早已存在」，记录不会被后到者盖掉
    /// （PR #68 review 指出的 exists()+rename 竞态）。
    #[test]
    fn concurrent_pretrust_writes_exactly_once() {
        let data_dir =
            std::env::temp_dir().join(format!("meowo-trust-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).unwrap();
        static SPEC: WorkspaceTrustSpec = WorkspaceTrustSpec {
            dir_rel: "workspace-trust",
        };
        let cwd = r"C:\proj\race";
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let data_dir = data_dir.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    SPEC.pretrust(&data_dir, cwd).unwrap()
                })
            })
            .collect();
        let wrote = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|&wrote| wrote)
            .count();
        assert_eq!(wrote, 1, "只能有一方真正写入");
        let body: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(SPEC.record_path(&data_dir, cwd)).unwrap(),
        )
        .unwrap();
        assert_eq!(body["root"], cwd);
        let _ = std::fs::remove_dir_all(&data_dir);
    }
}
