//! 跨模块共享的文件系统小工具 + 「文件」页签的目录浏览/文件读取命令。
//!
//! write_atomic 实现在 `meowo_agent::fsutil`——插件层写凭据、setup 写配置、account 写用量缓存
//! 都用同一份原子写，避免各自漂移的 tmp+rename 拷贝；这里只做重导出，供 app 内既有调用点保持原路径。
//!
//! list_dir_entries / read_file_text 是会话 cwd 下的只读浏览：rel 路径一律 canonicalize 后
//! 校验仍在 cwd 前缀内（挡 `..` 与符号链接逃逸）。读盘可能被杀软拖慢，统一走 spawn_blocking，
//! 不进主线程（同类教训见 terminal.rs open_project_dir 的注释）。
pub(crate) use meowo_agent::fsutil::write_atomic;

use meowo_protocol::ipc::{
    DirEntryDto, FileOpenerDto, FileTextDto, ImagePreviewDto, SearchFileHitDto, SearchLineHitDto,
    SearchResultDto,
};
use std::path::{Path, PathBuf};

/// 文件文本读取上限（与 git.rs 的 diff 上限同值；超出即截断并置 truncated）。
const MAX_FILE_BYTES: usize = 200 * 1024;
/// 二进制嗅探窗口（与 git.rs 一致）：前 8KB 出现 NUL 即按二进制处理。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 把 cwd + rel 解析成一个保证位于 cwd 内的绝对路径。
/// canonicalize 要求目标存在——浏览/读取场景本来也只操作存在的路径。
/// pub(crate)：git.rs 的未跟踪文件读取同样要挡 `..`/绝对路径逃逸，共用这一份校验。
pub(crate) fn resolve_inside(cwd: &str, rel: &str) -> Result<PathBuf, String> {
    let dir = cwd.trim();
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return Err("目录不存在".into());
    }
    let root = std::fs::canonicalize(dir).map_err(|e| format!("无法定位目录：{e}"))?;
    let full = std::fs::canonicalize(root.join(rel)).map_err(|_| "路径不存在".to_string())?;
    if !full.starts_with(&root) {
        return Err("路径超出工作目录".into());
    }
    Ok(full)
}

/// 把可能不是 UTF-8 的文本字节解码成 String：严格 UTF-8 成功则用之；失败按 GBK 解
/// （中文 Windows 上 GBK 源码文件常见，无脑 lossy 会整屏 U+FFFD）；仍失败才 lossy 兜底。
/// git.rs 的 diff 查看器与这里的文件查看器共用，两边口径必须一致。
pub(crate) fn decode_text_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => return text.to_string(),
        // error_len() == None：输入在一个合法多字节序列中间戛然而止——截断读取所致，
        // 本质仍是 UTF-8。只丢不完整的尾巴，不能整体误判成 GBK。
        Err(e) if e.error_len().is_none() => {
            return String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        Err(_) => {}
    }
    let (text, had_errors) = encoding_rs::GBK.decode_without_bom_handling(bytes);
    if !had_errors {
        return text.into_owned();
    }
    // 截断也可能把 GBK 双字节字符切半（错误只在最后一字节）：去尾重试一次。
    if !bytes.is_empty() {
        let (text, had_errors) = encoding_rs::GBK.decode_without_bom_handling(&bytes[..bytes.len() - 1]);
        if !had_errors {
            return text.into_owned();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// 列出 cwd 下 rel 目录的直接子条目（`.`/`..`/`.git` 除外），目录在前、同组按名称排序。
#[tauri::command]
pub(crate) async fn list_dir_entries(cwd: String, rel: String) -> Result<Vec<DirEntryDto>, String> {
    tauri::async_runtime::spawn_blocking(move || list_dir_entries_blocking(&cwd, &rel))
        .await
        .map_err(|e| e.to_string())?
}

fn list_dir_entries_blocking(cwd: &str, rel: &str) -> Result<Vec<DirEntryDto>, String> {
    let dir = resolve_inside(cwd, rel)?;
    if !dir.is_dir() {
        return Err("不是目录".into());
    }
    let base = rel.trim_matches('/');
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("无法读取目录：{e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // .git 永远跳过：里面是仓库内部结构，对「浏览项目文件」没有信息量还极深。
        if name == ".git" {
            continue;
        }
        let is_dir = entry.file_type().map_err(|e| e.to_string())?.is_dir();
        let rel_path = if base.is_empty() { name.clone() } else { format!("{base}/{name}") };
        entries.push(DirEntryDto { name, rel_path, is_dir });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// 读取 cwd 下 rel 文件的文本内容（封顶 MAX_FILE_BYTES；二进制只回标记不回文本）。
#[tauri::command]
pub(crate) async fn read_file_text(cwd: String, rel: String) -> Result<FileTextDto, String> {
    tauri::async_runtime::spawn_blocking(move || read_file_text_blocking(&cwd, &rel))
        .await
        .map_err(|e| e.to_string())?
}

fn read_file_text_blocking(cwd: &str, rel: &str) -> Result<FileTextDto, String> {
    use std::io::Read;
    let path = resolve_inside(cwd, rel)?;
    if !path.is_file() {
        return Err("不是文件".into());
    }
    let file = std::fs::File::open(&path).map_err(|e| format!("无法读取文件：{e}"))?;
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("无法读取文件：{e}"))?;
    let truncated = bytes.len() > MAX_FILE_BYTES;
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Ok(FileTextDto {
            rel_path: rel.to_string(),
            text: String::new(),
            truncated: false,
            binary: true,
        });
    }
    if truncated {
        bytes.truncate(MAX_FILE_BYTES);
    }
    // 截断点落在多字节字符中间的回退（丢不完整尾巴而非报错）由 decode_text_bytes 统一处理。
    Ok(FileTextDto {
        rel_path: rel.to_string(),
        text: decode_text_bytes(&bytes),
        truncated,
        binary: false,
    })
}

/// 图片预览上限：base64 后随 IPC 进 WebView，8MB 原图 ≈ 11MB 字符串，再大就该用外部应用看了。
const IMAGE_PREVIEW_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// 「文件」页签的图片预览：按扩展名识别格式，读盘转 data URL（CSP 已放行 img-src data:；
/// 不走 asset 协议——其 scope 只覆盖图片缓存目录，项目文件不该为预览开全盘白名单）。
#[tauri::command]
pub(crate) async fn read_image_preview(cwd: String, rel: String) -> Result<ImagePreviewDto, String> {
    tauri::async_runtime::spawn_blocking(move || read_image_preview_blocking(&cwd, &rel))
        .await
        .map_err(|e| e.to_string())?
}

fn read_image_preview_blocking(cwd: &str, rel: &str) -> Result<ImagePreviewDto, String> {
    use base64::Engine as _;
    let path = resolve_inside(cwd, rel)?;
    if !path.is_file() {
        return Err("不是文件".into());
    }
    let mime = image_mime(rel).ok_or_else(|| "不是支持的图片格式".to_string())?;
    let size = path.metadata().map_err(|e| e.to_string())?.len();
    if size > IMAGE_PREVIEW_MAX_BYTES {
        return Ok(ImagePreviewDto { data_url: None, too_large: true });
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("无法读取文件：{e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(ImagePreviewDto { data_url: Some(format!("data:{mime};base64,{encoded}")), too_large: false })
}

/// 扩展名 → 预览 MIME。SVG 也走 <img> + data URL 渲染：img 上下文不执行脚本，无注入面。
fn image_mime(rel: &str) -> Option<&'static str> {
    let name = rel.rsplit('/').next()?;
    let ext = name.rsplit('.').next()?.to_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        _ => return None,
    })
}

/// 搜索命中总数上限：面向「边打边搜」的交互，宁可截断快回，不做全量索引。
const SEARCH_MAX_HITS: usize = 300;
/// 单次搜索最多访问的条目数（文件+目录），防超大工作区把 blocking 池占死。
const SEARCH_MAX_ENTRIES: usize = 20_000;
/// 参与内容搜索的单文件上限：超过按「太大」跳过（比预览的 200KB 放宽一档）。
const SEARCH_MAX_CONTENT_BYTES: u64 = 512 * 1024;
/// 单文件最多报告的内容命中行数：防一个日志文件刷满整个结果窗。
const SEARCH_MAX_HITS_PER_FILE: usize = 5;
/// 内容命中预览的最大字符数（前端单行展示，超长无意义）。
const SEARCH_PREVIEW_CHARS: usize = 200;
/// 搜索跳过的目录名：依赖/构建产物量大且几乎不是用户要找的东西。
/// 目录树浏览不受此限制——树里仍能点进去看。
const SEARCH_SKIP_DIRS: [&str; 10] = [
    ".git", "node_modules", "target", "dist", ".venv", "venv", "__pycache__", ".next", ".nuxt",
    ".cache",
];

/// 在 cwd 下同时按「文件/目录名」与「文本内容」搜索 query（大小写不敏感的子串匹配）。
#[tauri::command]
pub(crate) async fn search_project_files(
    cwd: String,
    query: String,
) -> Result<SearchResultDto, String> {
    tauri::async_runtime::spawn_blocking(move || search_project_files_blocking(&cwd, &query))
        .await
        .map_err(|e| e.to_string())?
}

fn search_project_files_blocking(cwd: &str, query: &str) -> Result<SearchResultDto, String> {
    let root = resolve_inside(cwd, "")?;
    let needle = query.trim().to_lowercase();
    // 单字节查询（单个 ASCII 字符）内容命中噪声太大，直接返回空；
    // CJK 单字 UTF-8 至少 3 字节，不受此限。
    if needle.len() < 2 {
        return Ok(SearchResultDto { files: Vec::new(), truncated: false });
    }
    let mut files: Vec<SearchFileHitDto> = Vec::new();
    let mut visited = 0usize;
    let mut line_rows = 0usize;
    let mut truncated = false;
    // 显式栈迭代，不递归：深目录不担爆栈。条目内排序只为遍历确定性，
    // 结果顺序最终由收尾的整体排序决定。
    let mut stack: Vec<(PathBuf, String)> = vec![(root, String::new())];
    'walk: while let Some((dir, rel_base)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            visited += 1;
            // 上限判据：展示的行命中数 + 命中条目数，谁先到都收兵。
            if visited > SEARCH_MAX_ENTRIES
                || files.len() >= SEARCH_MAX_HITS
                || line_rows >= SEARCH_MAX_HITS
            {
                truncated = true;
                break 'walk;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // DirEntry::file_type 不跟随符号链接：链接目录 is_dir=false，天然不入栈（防环）。
            let Ok(file_type) = entry.file_type() else { continue };
            let rel_path = if rel_base.is_empty() {
                name.clone()
            } else {
                format!("{rel_base}/{name}")
            };
            let name_match = name.to_lowercase().contains(&needle);
            if file_type.is_dir() {
                if SEARCH_SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                if name_match {
                    files.push(SearchFileHitDto {
                        rel_path: rel_path.clone(),
                        is_dir: true,
                        name_match: true,
                        lines: Vec::new(),
                        total_line_matches: 0,
                    });
                }
                stack.push((entry.path(), rel_path));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let (lines, total) = search_file_content(&entry.path(), &needle);
            if !name_match && total == 0 {
                continue;
            }
            line_rows += lines.len();
            files.push(SearchFileHitDto {
                rel_path,
                is_dir: false,
                name_match,
                lines,
                total_line_matches: total,
            });
        }
    }
    // 名称命中条目在前（目录在文件前），内容命中殿后；组内按路径排序。
    files.sort_by(|a, b| {
        b.name_match
            .cmp(&a.name_match)
            .then(b.is_dir.cmp(&a.is_dir))
            .then(a.rel_path.cmp(&b.rel_path))
    });
    Ok(SearchResultDto { files, truncated })
}

/// 单文件内容匹配：返回（展示用命中行 ≤ SEARCH_MAX_HITS_PER_FILE 条，命中总行数）。
/// 太大/读失败/二进制一律按 (空, 0) 跳过——搜索是尽力而为的扫描，
/// 单个文件的问题不该让整次搜索失败。
fn search_file_content(path: &Path, needle: &str) -> (Vec<SearchLineHitDto>, u32) {
    const NONE: (Vec<SearchLineHitDto>, u32) = (Vec::new(), 0);
    let Ok(meta) = path.metadata() else { return NONE };
    if meta.len() > SEARCH_MAX_CONTENT_BYTES {
        return NONE;
    }
    let Ok(bytes) = std::fs::read(path) else { return NONE };
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return NONE;
    }
    let text = String::from_utf8_lossy(&bytes);
    // needle 已在入口整体小写过，逐行再 to_lowercase 是每行一次堆分配——大工作区一次
    // 搜索就是数十万次。ASCII needle（绝大多数）走无分配的字节窗口比较；非 ASCII 退化
    // 为整文件小写一次（单次分配）。小写不产生/吞掉换行，两份文本按行号对齐，
    // 预览仍取原文行。
    let lowered = (!needle.is_ascii()).then(|| text.to_lowercase());
    let mut lines = Vec::new();
    let mut total = 0u32;
    for (index, (line, match_line)) in text
        .lines()
        .zip(lowered.as_deref().unwrap_or(&text).lines())
        .enumerate()
    {
        let hit = if lowered.is_some() {
            match_line.contains(needle)
        } else {
            ascii_contains_ignore_case(match_line, needle)
        };
        if !hit {
            continue;
        }
        total += 1;
        if lines.len() < SEARCH_MAX_HITS_PER_FILE {
            lines.push(SearchLineHitDto {
                line: (index + 1) as u32,
                preview: build_preview(line, needle),
            });
        }
    }
    (lines, total)
}

/// ASCII needle（须已小写）的大小写不敏感子串查找，零分配。
/// needle 长度由入口保证 ≥2（windows(0) 会 panic，恰好被同一道门挡住）。
fn ascii_contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

/// 命中行预览：窗口对准首个命中处——命中在行首附近就从头截，否则从命中前
/// 一小段起截并加 … 前缀（否则长行里命中词会被右侧省略号吞掉，前端无从高亮）。
fn build_preview(line: &str, needle: &str) -> String {
    /// 命中距行首超过此字节数才左移窗口。
    const KEEP_HEAD: usize = 40;
    /// 左移后在命中前保留的字节数（上下文）。
    const CONTEXT_BEFORE: usize = 24;
    let trimmed = line.trim();
    let lower = trimmed.to_lowercase();
    // to_lowercase 对个别字符会变长（如 İ），字节偏移就对不上原文——长度一致才用。
    let pos = if lower.len() == trimmed.len() { lower.find(needle).unwrap_or(0) } else { 0 };
    let (window, prefixed) = if pos > KEEP_HEAD {
        let mut start = pos - CONTEXT_BEFORE;
        while !trimmed.is_char_boundary(start) {
            start -= 1;
        }
        (&trimmed[start..], true)
    } else {
        (trimmed, false)
    };
    let body: String = window.chars().take(SEARCH_PREVIEW_CHARS).collect();
    if prefixed { format!("…{body}") } else { body }
}

/// 已知编辑器探测表（非 Windows 用）：id → 菜单展示名。Windows 走 openwith.rs 的
/// 系统「打开方式」枚举（带图标），不用这份硬编码表；「系统默认应用」不在此表，
/// 前端恒有该项（open_path_with 的 opener="default"）。
#[cfg(not(target_os = "windows"))]
const EDITOR_DEFS: [(&str, &str); 2] = [("vscode", "Visual Studio Code"), ("cursor", "Cursor")];

/// 列出能打开 cwd 下 rel 文件的应用（文件面板「打开」按钮/菜单用）。
/// Windows：系统关联的推荐清单（含图标）；其余平台：探测表命中的编辑器。目录返回空。
#[tauri::command]
pub(crate) async fn list_file_openers(
    cwd: String,
    rel: String,
) -> Result<Vec<FileOpenerDto>, String> {
    // 注册表枚举/图标提取/存在性探测都是 IO（杀软/网络盘可拖慢），照纪律离开主线程。
    tauri::async_runtime::spawn_blocking(move || list_file_openers_blocking(&cwd, &rel))
        .await
        .map_err(|e| e.to_string())?
}

fn list_file_openers_blocking(cwd: &str, rel: &str) -> Result<Vec<FileOpenerDto>, String> {
    let target = resolve_inside(cwd, rel)?;
    if target.is_dir() {
        return Ok(Vec::new());
    }
    #[cfg(target_os = "windows")]
    {
        Ok(crate::openwith::list_for_path(&target))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &target;
        Ok(EDITOR_DEFS
            .iter()
            .filter(|(id, _)| editor_launcher(id).is_some())
            .map(|(id, name)| FileOpenerDto {
                id: (*id).to_string(),
                name: (*name).to_string(),
                icon: None,
            })
            .collect())
    }
}

/// 定位编辑器的启动载体（非 Windows）：macOS 是 /Applications 下的 .app
/// （用 `open -a` 启动），其余平台查 PATH。返回 None = 未安装。
#[cfg(target_os = "macos")]
fn editor_launcher(id: &str) -> Option<PathBuf> {
    let app = match id {
        "vscode" => "/Applications/Visual Studio Code.app",
        "cursor" => "/Applications/Cursor.app",
        _ => return None,
    };
    let path = PathBuf::from(app);
    path.is_dir().then_some(path)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn editor_launcher(id: &str) -> Option<PathBuf> {
    let exe = match id {
        "vscode" => "code",
        "cursor" => "cursor",
        _ => return None,
    };
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).map(|dir| dir.join(exe)).find(|p| p.is_file())
}

/// canonicalize 在 Windows 返回 `\\?\` 扩展前缀路径，explorer/Shell 解析器与多数
/// 应用不认——转成常规展示形态（openwith.rs 的 SHCreateItemFromParsingName 也用）。
#[cfg(target_os = "windows")]
pub(crate) fn display_path(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    }
}

/// fire-and-forget 拉起外部命令并立即返回：只报告「是否拉起成功」，不关心退出码。
/// spawn 失败照常报错；拉起成功后 Child 不能随手 drop——Unix 上不 wait 的子进程退出后
/// 会成 <defunct> 僵尸，挂在常驻托盘的本进程名下越攒越多（教训见 settings.rs
/// spawn_browser 的注释）。故把 wait 挪到后台线程：等待不再挡调用线程，阻塞无害。
/// Windows 无僵尸问题，但统一走一条路径，免得每个调用点各写一份分平台分支。
// 生产调用点全在 macOS/Linux 分支里（Windows 的 explorer 无僵尸问题，维持原样）——
// Windows 构建下只有测试用到它。
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) fn spawn_detached(command: &mut std::process::Command) -> Result<(), String> {
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// 用指定方式打开 cwd 内的文件/目录：opener 为 "default"（系统默认关联/文件管理器）
/// 或 EDITOR_DEFS 里的编辑器 id。路径经 resolve_inside 校验；不经 shell、目标作为
/// 独立 argv 传入，无注入面。spawn 后不等退出（打开动作 fire-and-forget，同 open_project_dir）。
#[tauri::command]
pub(crate) async fn open_path_with(cwd: String, rel: String, opener: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_path_with_blocking(&cwd, &rel, &opener))
        .await
        .map_err(|e| e.to_string())?
}

/// 用系统默认关联打开目标（explorer / open / xdg-open）。open_path_with 的 "default"
/// 分支与 install.rs 的「打开安装日志」共用。spawn 后不等退出（fire-and-forget）。
pub(crate) fn open_with_default_app(target: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // explorer 对正斜杠路径会打开默认目录而非目标（同 open_project_dir 的教训）。
        std::process::Command::new("explorer")
            .arg(display_path(target))
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached(std::process::Command::new("open").arg(target))?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux 同属 Unix：xdg-open 也要 wait 回收，见 spawn_detached。
        spawn_detached(std::process::Command::new("xdg-open").arg(target))?;
    }
    Ok(())
}

fn open_path_with_blocking(cwd: &str, rel: &str, opener: &str) -> Result<(), String> {
    let target = resolve_inside(cwd, rel)?;
    if opener == "default" {
        return open_with_default_app(&target);
    }
    // Windows：opener 是系统「打开方式」处理器的标识，交回 openwith 在系统清单里
    // 挑选并 Invoke（UWP 等非 exe 形态也能正确拉起）；其余平台仍走编辑器探测表。
    #[cfg(target_os = "windows")]
    {
        crate::openwith::invoke(&target, opener)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let launcher = editor_launcher(opener).ok_or_else(|| "未检测到该编辑器".to_string())?;
        #[cfg(target_os = "macos")]
        {
            spawn_detached(
                std::process::Command::new("open")
                    .arg("-a")
                    .arg(&launcher)
                    .arg(&target),
            )?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            spawn_detached(std::process::Command::new(&launcher).arg(&target))?;
        }
        Ok(())
    }
}

/// 在系统文件管理器里定位（选中）cwd 内的文件/目录。
#[tauri::command]
pub(crate) async fn reveal_path_in_file_manager(cwd: String, rel: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || reveal_path_blocking(&cwd, &rel))
        .await
        .map_err(|e| e.to_string())?
}

fn reveal_path_blocking(cwd: &str, rel: &str) -> Result<(), String> {
    let target = resolve_inside(cwd, rel)?;
    #[cfg(target_os = "windows")]
    {
        // `/select,路径` 是单个 argv：explorer 语法如此，含空格也无需引号
        // （CreateProcess 的 argv 不会被二次拆分）。
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", display_path(&target)))
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached(std::process::Command::new("open").arg("-R").arg(&target))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux 文件管理器无统一「选中」协议：退而求其次打开父目录。
        let parent = target.parent().unwrap_or(target.as_path());
        spawn_detached(std::process::Command::new("xdg-open").arg(parent))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 每个用例一个独立临时目录，落盘结构后返回路径；Drop 时自动清掉。
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "meowo-fsutil-test-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn spawn_detached_reports_spawn_failure_and_reaps_success() {
        // 不存在的可执行文件：spawn 本身失败必须报错（调用方据此提示用户）。
        assert!(spawn_detached(&mut std::process::Command::new(
            "meowo-definitely-not-a-real-binary-9f8d7c"
        ))
        .is_err());
        // 能拉起的命令立即返回 Ok，退出回收在后台线程完成。
        #[cfg(target_os = "windows")]
        let mut noop = {
            let mut cmd = std::process::Command::new("cmd");
            cmd.args(["/c", "exit 0"]);
            cmd
        };
        #[cfg(not(target_os = "windows"))]
        let mut noop = std::process::Command::new("true");
        assert!(spawn_detached(&mut noop).is_ok());
    }

    #[test]
    fn resolve_inside_rejects_parent_escape() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(Path::new(tmp.path()).join("sub")).unwrap();
        assert!(resolve_inside(tmp.path(), "sub").is_ok());
        assert!(resolve_inside(tmp.path(), "").is_ok());
        assert!(resolve_inside(tmp.path(), "..").is_err());
        assert!(resolve_inside(tmp.path(), "../..").is_err());
        assert!(resolve_inside(tmp.path(), "sub/../..").is_err());
        assert!(resolve_inside(tmp.path(), "不存在").is_err());
    }

    #[test]
    fn list_dir_entries_skips_git_and_sorts_dirs_first() {
        let tmp = TempDir::new();
        let root = Path::new(tmp.path());
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::write(root.join("zebra.txt"), "z").unwrap();
        std::fs::write(root.join("apple.txt"), "a").unwrap();
        let entries = list_dir_entries_blocking(tmp.path(), "").unwrap();
        let got: Vec<(&str, &str, bool)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.rel_path.as_str(), e.is_dir))
            .collect();
        assert_eq!(
            got,
            vec![
                ("alpha", "alpha", true),
                ("beta", "beta", true),
                ("apple.txt", "apple.txt", false),
                ("zebra.txt", "zebra.txt", false),
            ]
        );
    }

    #[test]
    fn list_dir_entries_builds_nested_rel_paths() {
        let tmp = TempDir::new();
        let root = Path::new(tmp.path());
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        let entries = list_dir_entries_blocking(tmp.path(), "src").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel_path, "src/main.rs");
        assert!(!entries[0].is_dir);
    }

    #[test]
    fn search_finds_names_and_content_grouped_by_file() {
        let tmp = TempDir::new();
        let root = Path::new(tmp.path());
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("alpha_dir")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/alpha.rs"), "fn main() {}\nlet alpha = 1;\n").unwrap();
        std::fs::write(root.join("src/other.rs"), "let alpha = 2;\n").unwrap();
        std::fs::write(root.join("node_modules/pkg/alpha.js"), "alpha").unwrap();
        std::fs::write(root.join(".git/alpha"), "alpha").unwrap();
        std::fs::write(root.join("b.bin"), b"\x00alpha").unwrap();
        // 大小写不敏感；.git/node_modules 整棵跳过；二进制内容不参与。
        // 名称+内容双命中合并为一个条目；名称命中排前（目录又在文件前）。
        let result = search_project_files_blocking(tmp.path(), "Alpha").unwrap();
        assert!(!result.truncated);
        let summary: Vec<(&str, bool, bool, usize, u32)> = result
            .files
            .iter()
            .map(|f| {
                (f.rel_path.as_str(), f.is_dir, f.name_match, f.lines.len(), f.total_line_matches)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("alpha_dir", true, true, 0, 0),
                ("src/alpha.rs", false, true, 1, 1),
                ("src/other.rs", false, false, 1, 1),
            ]
        );
        assert_eq!(result.files[1].lines[0].line, 2);
        assert_eq!(result.files[1].lines[0].preview, "let alpha = 1;");
    }

    #[test]
    fn search_rejects_single_ascii_char_but_accepts_single_cjk() {
        let tmp = TempDir::new();
        let root = Path::new(tmp.path());
        std::fs::write(root.join("a.txt"), "你好世界").unwrap();
        assert!(search_project_files_blocking(tmp.path(), "a").unwrap().files.is_empty());
        let cjk = search_project_files_blocking(tmp.path(), "好").unwrap();
        assert_eq!(cjk.files.len(), 1);
        assert_eq!(cjk.files[0].lines[0].line, 1);
    }

    #[test]
    fn search_caps_shown_lines_but_counts_all_matches() {
        let tmp = TempDir::new();
        let root = Path::new(tmp.path());
        std::fs::write(root.join("log.txt"), "match\n".repeat(20)).unwrap();
        let result = search_project_files_blocking(tmp.path(), "match").unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].lines.len(), SEARCH_MAX_HITS_PER_FILE);
        assert_eq!(result.files[0].total_line_matches, 20);
    }

    #[test]
    fn search_preview_window_follows_the_match() {
        // 命中在长行右侧时窗口左移并带 … 前缀，保证预览里含命中词。
        let preview = build_preview(&format!("{}needle tail", "x".repeat(120)), "needle");
        assert!(preview.starts_with('…'));
        assert!(preview.contains("needle"));
        // 行首附近命中不动窗口。
        assert_eq!(build_preview("  let needle = 1;", "needle"), "let needle = 1;");
    }

    #[test]
    fn list_file_openers_empty_for_dirs() {
        let tmp = TempDir::new();
        assert!(list_file_openers_blocking(tmp.path(), "").unwrap().is_empty());
    }

    /// 三级解码口径：严格 UTF-8 → GBK → lossy；截断切半的多字节字符只丢尾巴。
    #[test]
    fn decode_text_bytes_handles_utf8_gbk_and_truncation() {
        assert_eq!(decode_text_bytes("你好".as_bytes()), "你好");
        // GBK 的「中文测试」解出正确中文，而不是整屏替换符。
        let gbk: &[u8] = &[0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4];
        assert_eq!(decode_text_bytes(gbk), "中文测试");
        // UTF-8 截断在多字节字符中间：丢不完整尾巴，不得整体误判成 GBK。
        let utf8 = "中文".as_bytes();
        assert_eq!(decode_text_bytes(&utf8[..4]), "中");
        // GBK 截断在双字节字符中间：去尾重试，仍解出前面的中文。
        assert_eq!(decode_text_bytes(&gbk[..3]), "中");
        // 两种编码都解不动的字节：lossy 兜底，不 panic。
        assert!(!decode_text_bytes(&[0xFF, 0x81, 0x00]).is_empty());
    }

    #[test]
    fn read_file_text_decodes_gbk_files() {
        let tmp = TempDir::new();
        std::fs::write(
            Path::new(tmp.path()).join("gbk.txt"),
            [0xD6, 0xD0, 0xCE, 0xC4],
        )
        .unwrap();
        let text = read_file_text_blocking(tmp.path(), "gbk.txt").unwrap();
        assert_eq!(text.text, "中文");
        assert!(!text.binary);
    }

    #[test]
    fn read_file_text_reads_text_and_flags_binary() {
        let tmp = TempDir::new();
        let root = Path::new(tmp.path());
        std::fs::write(root.join("a.txt"), "你好，世界").unwrap();
        std::fs::write(root.join("b.bin"), b"\x00\x01\x02").unwrap();
        let text = read_file_text_blocking(tmp.path(), "a.txt").unwrap();
        assert_eq!(text.text, "你好，世界");
        assert!(!text.truncated);
        assert!(!text.binary);
        let bin = read_file_text_blocking(tmp.path(), "b.bin").unwrap();
        assert!(bin.binary);
        assert!(bin.text.is_empty());
        assert!(read_file_text_blocking(tmp.path(), "..").is_err());
    }
}
