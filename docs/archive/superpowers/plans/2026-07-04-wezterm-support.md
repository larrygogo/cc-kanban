# WezTerm 终端兼容 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Windows 上把 WezTerm 作为第四个可选终端接入:恢复会话可在 WezTerm 开新 tab/新窗口,点卡片可精确聚焦到 WezTerm 的对应 pane。

**Architecture:** 新增 `app/src-tauri/src/wezterm.rs` 模块(整体 `#[cfg(target_os = "windows")]`,避开 macOS CI dead_code 地雷),封装 wezterm CLI 的探测/socket 发现/list/spawn/activate;`lib.rs` 只在三处挂接:`available_terminals`、`resume_session` 的终端分支、`focus_session_terminal` 的兜底链。前端加一个下拉选项。macOS 侧本期不做。

**Tech Stack:** Rust (std::process + serde_json,无新依赖),React/TS 前端只改类型与选项数组。

## Global Constraints

- 分支:`feat/wezterm-terminal-support-20260704`,从 `main` 切出(当前 `feat/provider-setup-20260703` 与本功能无关)
- 工作区有两个**不属于本功能**的未提交改动(`app/src-tauri/tauri.conf.json`、`app/vite.config.ts`)和一个 untracked spec 文档——**永远不要 `git add` 它们**,每次 commit 精确指定文件
- commit message 一律中文;代码注释中文且只写必要的
- 所有 `wezterm cli` 调用**必须**带 `--no-auto-start`(否则 GUI 未运行时会偷偷拉起 wezterm-mux-server 并永久阻塞——已实测)
- 所有 `wezterm cli` 调用**必须**显式设置环境变量 `WEZTERM_UNIX_SOCKET=<runtime_dir>\gui-sock-<gui_pid>`(CLI 的自动发现不可靠,已实测连不上)
- 从 GUI 进程 spawn `wezterm.exe`(console 程序)必须加 `CREATE_NO_WINDOW (0x0800_0000)`,否则闪黑窗;`wezterm-gui.exe` 是 GUI 程序,不需要
- Rust 单测命令:仓库根目录 `cargo test -p meowo-app --lib`;若 tauri_build 报 externalBin 缺失,先跑 `node scripts/prepare-sidecar.mjs`

## 已验证事实(执行者不必重新验证)

在 larry 机器(WezTerm 20240203-110809,`C:\Program Files\WezTerm\`,已在 PATH)上实测:

1. `wezterm cli list --format json` 返回数组,关键字段:`pane_id`(u64)、`tab_id`、`title`(string,ConPTY 透传的控制台标题)、`cwd`(**file:/// URL 格式**,如 `file:///C:/Users/larry/Desktop/workspace/`)。**没有 pid 字段**,`tty_name` 为 null → pane 匹配只能靠 title/cwd/token。
2. pane 内进程用 console API 设置标题(SetConsoleTitle 路径)后,`title` 字段**立即**反映 → meowo-reporter 写的 token 标签在 WezTerm 下可读,无需改 meowo-reporter。
3. 进程祖先链:`pwsh(会话) → …shell… → wezterm-gui.exe`;ConPTY 的 `OpenConsole.exe` 是 wezterm-gui 的**兄弟**子进程,不在祖先链上。
4. socket 文件:`%USERPROFILE%\.local\share\wezterm\gui-sock-<gui pid>`;GUI 退出可能残留文件,必须配合「该 pid 的 wezterm-gui.exe 进程活着」校验。
5. `wezterm cli spawn --cwd <dir> -- <argv>` 在已开 GUI 中新建 tab(stdout 输出新 pane_id);`wezterm cli activate-pane --pane-id N` 正常;GUI 未运行时 `wezterm-gui start --cwd <dir> -- <argv>` 新开窗口,cwd 正确。
6. `--no-auto-start` 下 GUI 未运行时 cli 立即 exit 1(`failed to connect to Socket`)。
7. WezTerm 窗口类名 `org.wezfurlong.wezterm`(仅备查;置前走「按 pid 找窗口」的现有 `find_window_for_pids`,不需要类名)。

## 现有代码接口(wezterm.rs 会用到,均在 lib.rs / crate root,子模块用 `crate::` 直接可见,无需改可见性)

- `fn path_has_exe(path_var: &OsStr, exe: &str) -> bool` — PATH 各目录找可执行(用 symlink_metadata,兼容 App Execution Alias)
- `fn snapshot_processes() -> HashMap<u32, (u32, String)>` — Toolhelp 快照:pid → (ppid, 小写进程名)
- `fn console_group_pids(root_pid: u32) -> HashSet<u32>` — 会话进程组(root + 祖先至终端宿主 + 子孙)
- `fn find_window_for_pids(targets: &HashSet<u32>) -> Option<HWND>` — 枚举可见顶层窗口,pid 命中即返回
- `fn force_foreground(hwnd: HWND)` — AttachThreadInput 置前
- `fn tab_match_score(tab_name: &str, want: &str) -> u8` — 标题匹配强度 2/1/0(纯函数,含标题归一化)
- `fn safe_cwd(cwd: Option<&str>) -> Option<String>` — 收敛出可安全传给命令行的真实目录

---

### Task 1: 建分支 + wezterm.rs 纯函数(URL 解析 / JSON 解析 / pane 匹配)

**Files:**
- Create: `app/src-tauri/src/wezterm.rs`
- Modify: `app/src-tauri/src/lib.rs`(仅加一行 `mod wezterm;` 声明)
- Test: 同文件 `#[cfg(test)]`(项目惯例,参照 `term_script.rs`)

**Interfaces:**
- Consumes: `crate::tab_match_score`(见上)
- Produces(后续任务依赖,签名以此为准):
  - `pub(crate) struct PaneInfo { pane_id: u64, title: String, cwd: String }`(cwd 存 file:/// URL 原样)
  - `fn parse_panes(json: &str) -> Vec<PaneInfo>`
  - `fn match_pane(panes: &[PaneInfo], want_title: &str, token: Option<&str>, cwd: Option<&str>) -> Option<u64>`
  - `fn file_url_to_path(url: &str) -> Option<String>`

- [ ] **Step 1: 建分支**

```bash
cd /c/Users/larry/Desktop/workspace/meowo
git checkout -b feat/wezterm-terminal-support-20260704 main
```

若因 `tauri.conf.json`/`vite.config.ts` 的未提交改动与 main 冲突而失败:`git stash push app/src-tauri/tauri.conf.json app/vite.config.ts` → checkout → `git stash pop`。

- [ ] **Step 2: 写失败的单测**

创建 `app/src-tauri/src/wezterm.rs`,先只写模块头和测试(实现留空签名或 `todo!()` 以外的最小桩——**直接写测试 + 未实现的空模块会编译失败,这里选择先写测试和函数签名骨架**,函数体返回显然错误的值,让测试红):

```rust
//! WezTerm 终端集成(仅 Windows):探测、gui socket 发现、cli list/spawn/activate 封装。
//!
//! 关键约束(均已实测,勿"简化"):
//! - 一切 `wezterm cli` 必须带 --no-auto-start,否则 GUI 未运行时会拉起 mux server 并阻塞;
//! - 必须显式设 WEZTERM_UNIX_SOCKET 指向 gui-sock-<pid>,CLI 自动发现不可靠;
//! - cli list 无 pid 字段,pane 匹配只能靠 title(token/任务标题)与 cwd(file:/// URL)。

/// `wezterm cli list --format json` 里本模块关心的字段。cwd 保持 file:/// URL 原样,
/// 比较时经 file_url_to_path 归一化。
#[derive(Debug, PartialEq, serde::Deserialize)]
pub(crate) struct PaneInfo {
    pub pane_id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cwd: String,
}

/// 解析 cli list 输出。解析失败返回空(调用方退化为窗口级聚焦)。
fn parse_panes(json: &str) -> Vec<PaneInfo> {
    Vec::new() // TDD 桩
}

/// percent 解码(%20 → 空格,UTF-8 字节层解码兼容中文路径);非法序列原样保留。
fn percent_decode(s: &str) -> String {
    String::new() // TDD 桩
}

/// file:///C:/Users/x/ → C:\Users\x。非 file URL / 空路径返回 None。
fn file_url_to_path(url: &str) -> Option<String> {
    None // TDD 桩
}

/// pane 的 cwd(file URL)与会话 cwd(Windows 路径)是否同一目录:统一反斜杠、去尾斜杠、
/// ASCII 大小写不敏感(NTFS 语义,非 ASCII 按原样比较,够用)。
fn cwd_matches(pane_cwd_url: &str, session_cwd: &str) -> bool {
    false // TDD 桩
}

/// 从 panes 里选出唯一最佳匹配的 pane_id。打分:token 命中 title=4 >
/// cwd+title 双命中=3 > cwd 命中=2 > title 命中=1。最高分不唯一或全 0 → None(不猜,
/// 调用方退窗口级,语义对齐 WT 的「同窗口多同名标签不猜」)。
fn match_pane(
    panes: &[PaneInfo],
    want_title: &str,
    token: Option<&str>,
    cwd: Option<&str>,
) -> Option<u64> {
    None // TDD 桩
}

#[cfg(test)]
mod tests {
    use super::*;

    // 实测样本的字段子集(多余字段应被 serde 忽略)
    const LIST_JSON: &str = r#"[
      {"window_id":0,"tab_id":0,"pane_id":0,"workspace":"default",
       "title":"cc-TOKEN-abc12345","cwd":"file:///C:/Users/larry/","is_active":true},
      {"window_id":0,"tab_id":1,"pane_id":1,
       "title":"cmd.exe","cwd":"file:///C:/Users/larry/Desktop/workspace/"}
    ]"#;

    #[test]
    fn parse_panes_extracts_fields_and_ignores_unknown() {
        let panes = parse_panes(LIST_JSON);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].pane_id, 0);
        assert_eq!(panes[0].title, "cc-TOKEN-abc12345");
        assert_eq!(panes[1].cwd, "file:///C:/Users/larry/Desktop/workspace/");
    }

    #[test]
    fn parse_panes_bad_json_gives_empty() {
        assert!(parse_panes("not json").is_empty());
        assert!(parse_panes("{}").is_empty());
    }

    #[test]
    fn percent_decode_handles_space_utf8_and_invalid() {
        assert_eq!(percent_decode("a%20b"), "a b");
        // 中文「工」= E5 B7 A5(UTF-8 percent 编码逐字节)
        assert_eq!(percent_decode("%E5%B7%A5"), "工");
        assert_eq!(percent_decode("50%zz"), "50%zz"); // 非法序列原样保留
        assert_eq!(percent_decode("tail%2"), "tail%2"); // 结尾截断原样保留
    }

    #[test]
    fn file_url_to_path_converts_and_rejects() {
        assert_eq!(
            file_url_to_path("file:///C:/Users/larry/Desktop/workspace/").as_deref(),
            Some(r"C:\Users\larry\Desktop\workspace")
        );
        assert_eq!(
            file_url_to_path("file:///C:/a%20b/").as_deref(),
            Some(r"C:\a b")
        );
        assert_eq!(file_url_to_path("https://example.com/x"), None);
        assert_eq!(file_url_to_path("file:///"), None);
    }

    #[test]
    fn cwd_matches_normalizes_slash_trailing_case() {
        let url = "file:///C:/Users/larry/Desktop/workspace/";
        assert!(cwd_matches(url, r"C:\Users\larry\Desktop\workspace"));
        assert!(cwd_matches(url, r"c:\users\larry\desktop\workspace\"));
        assert!(!cwd_matches(url, r"C:\Users\larry"));
        assert!(!cwd_matches("not-a-url", r"C:\Users\larry"));
    }

    fn pane(id: u64, title: &str, cwd: &str) -> PaneInfo {
        PaneInfo { pane_id: id, title: title.into(), cwd: cwd.into() }
    }

    #[test]
    fn match_pane_token_beats_everything() {
        let panes = vec![
            pane(0, "✳ 修复登录", "file:///C:/proj/"),
            pane(1, "kimi abc12345", "file:///C:/other/"),
        ];
        // pane 0 同时命中 title+cwd(3 分),但 token 命中 pane 1(4 分)胜出
        assert_eq!(
            match_pane(&panes, "修复登录", Some("abc12345"), Some(r"C:\proj")),
            Some(1)
        );
    }

    #[test]
    fn match_pane_cwd_unique_hit() {
        let panes = vec![
            pane(0, "pwsh.exe", "file:///C:/proj-a/"),
            pane(1, "pwsh.exe", "file:///C:/proj-b/"),
        ];
        assert_eq!(match_pane(&panes, "", None, Some(r"C:\proj-b")), Some(1));
    }

    #[test]
    fn match_pane_ambiguous_returns_none() {
        // 同目录两个 pane、无 token、标题相同 → 并列,不猜
        let panes = vec![
            pane(0, "pwsh.exe", "file:///C:/proj/"),
            pane(1, "pwsh.exe", "file:///C:/proj/"),
        ];
        assert_eq!(match_pane(&panes, "", None, Some(r"C:\proj")), None);
    }

    #[test]
    fn match_pane_title_disambiguates_same_cwd() {
        // 同目录,但任务标题只命中其一(cwd+title=3 分 vs cwd=2 分)
        let panes = vec![
            pane(0, "✳ 修复登录", "file:///C:/proj/"),
            pane(1, "pwsh.exe", "file:///C:/proj/"),
        ];
        assert_eq!(match_pane(&panes, "修复登录", None, Some(r"C:\proj")), Some(0));
    }

    #[test]
    fn match_pane_no_signal_returns_none() {
        let panes = vec![pane(0, "pwsh.exe", "file:///C:/x/")];
        assert_eq!(match_pane(&panes, "", None, None), None);
        assert_eq!(match_pane(&panes, "不存在的标题", None, Some(r"C:\y")), None);
    }
}
```

在 `lib.rs` 的现有 `mod` 声明区(`mod snap;` 等附近)加:

```rust
#[cfg(target_os = "windows")]
mod wezterm;
```

- [ ] **Step 3: 跑测试确认失败**

```powershell
cargo test -p meowo-app --lib wezterm
```

Expected: FAIL(assert 失败,非编译错误。若编译错误,先修到能编译再看红)。

- [ ] **Step 4: 实现纯函数**

替换各桩:

```rust
fn parse_panes(json: &str) -> Vec<PaneInfo> {
    serde_json::from_str(json).unwrap_or_default()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(h) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|hx| u8::from_str_radix(hx, 16).ok())
            {
                out.push(h);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn file_url_to_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let decoded = percent_decode(rest.trim_start_matches('/'));
    let path = decoded.replace('/', "\\");
    let path = path.trim_end_matches('\\');
    (!path.is_empty()).then(|| path.to_string())
}

fn cwd_matches(pane_cwd_url: &str, session_cwd: &str) -> bool {
    let Some(p) = file_url_to_path(pane_cwd_url) else { return false };
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    norm(&p) == norm(session_cwd)
}

fn match_pane(
    panes: &[PaneInfo],
    want_title: &str,
    token: Option<&str>,
    cwd: Option<&str>,
) -> Option<u64> {
    let score = |p: &PaneInfo| -> u8 {
        if token.is_some_and(|t| !t.is_empty() && p.title.contains(t)) {
            return 4;
        }
        let cwd_hit = cwd.is_some_and(|c| cwd_matches(&p.cwd, c));
        let title_hit = crate::tab_match_score(&p.title, want_title) > 0;
        match (cwd_hit, title_hit) {
            (true, true) => 3,
            (true, false) => 2,
            (false, true) => 1,
            (false, false) => 0,
        }
    };
    let scored: Vec<(u8, u64)> = panes.iter().map(|p| (score(p), p.pane_id)).collect();
    let max = scored.iter().map(|(s, _)| *s).max().filter(|&m| m > 0)?;
    let mut top = scored.iter().filter(|(s, _)| *s == max).map(|(_, id)| *id);
    match (top.next(), top.next()) {
        (Some(one), None) => Some(one),
        _ => None, // 并列:不猜,退窗口级
    }
}
```

注意:`crate::tab_match_score` 当前带 `#[allow(dead_code)] // 跨平台纯函数:非 Windows 上无运行时调用方,仅单测使用` ——本任务后它有了 wezterm 的运行时调用方,把该注释更新为 `// 跨平台纯函数:Windows 上 WT/WezTerm 聚焦共用,非 Windows 仅单测`(allow 保留,macOS 仍无运行时调用方)。

- [ ] **Step 5: 跑测试确认通过**

```powershell
cargo test -p meowo-app --lib wezterm
```

Expected: PASS(9 个测试)。再跑全量确认无回归:`cargo test -p meowo-app --lib`。

- [ ] **Step 6: Commit**

```bash
git add app/src-tauri/src/wezterm.rs app/src-tauri/src/lib.rs
git commit -m "feat(wezterm): wezterm 模块纯函数——cli list 解析、file URL 转路径、pane 匹配打分"
```

---

### Task 2: 探测(available)+ available_terminals + 前端选项

**Files:**
- Modify: `app/src-tauri/src/wezterm.rs`(加 `available()`)
- Modify: `app/src-tauri/src/lib.rs`(`available_terminals` Windows 分支,约 line 1827-1836)
- Modify: `app/src/api.ts:146`(`ResumeTerminal` 类型)
- Modify: `app/src/views/About.tsx:51-55`(`resumeTermOptionsWin`)

**Interfaces:**
- Consumes: `crate::path_has_exe`
- Produces: `pub(crate) fn available() -> bool`(Task 3 的 eff 选择依赖)

- [ ] **Step 1: 实现 available()(无单测——纯环境探测,与 wt_available 同款,逻辑无分支)**

wezterm.rs 中加:

```rust
/// wezterm.exe 是否在 PATH(官方安装器/winget/scoop 均会加)。进程内缓存,同 wt_available。
pub(crate) fn available() -> bool {
    use std::sync::OnceLock;
    static ON_PATH: OnceLock<bool> = OnceLock::new();
    *ON_PATH.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|p| crate::path_has_exe(&p, "wezterm.exe"))
    })
}
```

- [ ] **Step 2: available_terminals 加 wezterm**

lib.rs Windows 分支改为:

```rust
    #[cfg(target_os = "windows")]
    {
        let mut v = Vec::new();
        if wt_available() {
            v.push("wt".to_string());
        }
        if wezterm::available() {
            v.push("wezterm".to_string());
        }
        v.push("powershell".to_string());
        v.push("cmd".to_string());
        v
    }
```

同时更新该命令的文档注释(line 1809):`Windows：powershell/cmd 必有，wt/wezterm 视是否在 PATH`。

- [ ] **Step 3: 前端类型与选项**

`app/src/api.ts:146`:

```ts
export type ResumeTerminal = "terminal" | "iterm" | "wt" | "wezterm" | "powershell" | "cmd";
```

`app/src/views/About.tsx` 的 `resumeTermOptionsWin`(wt 之后插入,与后端列表顺序一致):

```ts
const resumeTermOptionsWin = (t: Dict): { value: ResumeTerminal; label: string }[] => [
  { value: "wt", label: "Windows Terminal" },
  { value: "wezterm", label: "WezTerm" },
  { value: "powershell", label: "PowerShell" },
  { value: "cmd", label: t.settings.cmdPrompt },
];
```

("WezTerm" 是专有名词,不进 i18n,与 "Windows Terminal"/"PowerShell" 同理。)

- [ ] **Step 4: 验证编译与类型**

```powershell
cargo check -p meowo-app
cd app; bun run build; cd ..
```

Expected: 两者无错误(`bun run build` 含 tsc 类型检查;若项目用 `bun run check` 或 `tsc -b`,以 `app/package.json` scripts 里实际存在的类型检查命令为准)。

- [ ] **Step 5: Commit**

```bash
git add app/src-tauri/src/wezterm.rs app/src-tauri/src/lib.rs app/src/api.ts app/src/views/About.tsx
git commit -m "feat(wezterm): 探测 wezterm.exe 并接入设置页终端下拉(available_terminals + 前端选项)"
```

---

### Task 3: 恢复会话走 WezTerm(cli spawn 复用窗口,失败回退新窗口)

**Files:**
- Modify: `app/src-tauri/src/wezterm.rs`(`runtime_dir`/`any_gui`/`resume`)
- Modify: `app/src-tauri/src/lib.rs` `resume_session` Windows 分支(约 line 1047-1117:eff 选择 + spawned 匹配臂 + 类型统一)

**Interfaces:**
- Consumes: `crate::snapshot_processes`、`crate::safe_cwd`(lib.rs 调用侧已算好 dir)
- Produces:
  - `pub(crate) fn resume(dir: Option<&str>, argv: &[String]) -> std::io::Result<()>`
  - `fn any_gui() -> Option<(u32, std::path::PathBuf)>`(Task 4 复用其孪生 `gui_in_group`)

- [ ] **Step 1: 实现 socket 发现与 resume**

wezterm.rs 加:

```rust
use std::collections::HashSet;
use std::path::PathBuf;

/// 从 GUI 进程 spawn console 程序(wezterm.exe)不弹黑窗。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// wezterm 的 runtime 目录(Windows 上固定 $HOME/.local/share/wezterm,已实测)。
fn runtime_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")?;
    Some(PathBuf::from(home).join(".local").join("share").join("wezterm"))
}

/// pid 对应的 gui socket:文件存在才算(GUI 退出会残留旧 sock 文件,必须配进程校验,
/// 因此本函数只接受「来自当前进程快照」的 pid)。
fn sock_for(pid: u32) -> Option<PathBuf> {
    let p = runtime_dir()?.join(format!("gui-sock-{pid}"));
    p.is_file().then_some(p)
}

/// 任一存活的 wezterm-gui 实例及其 socket(resume 用:哪个窗口都行)。
fn any_gui() -> Option<(u32, PathBuf)> {
    crate::snapshot_processes()
        .iter()
        .filter(|(_, (_, name))| name == "wezterm-gui.exe")
        .find_map(|(&pid, _)| sock_for(pid).map(|s| (pid, s)))
}

/// 会话进程组内的 wezterm-gui 实例(focus 用:必须是该会话的宿主,防止把 WT 里的
/// 会话误切到 WezTerm 的同名 pane)。
fn gui_in_group(group: &HashSet<u32>) -> Option<(u32, PathBuf)> {
    crate::snapshot_processes()
        .iter()
        .filter(|(pid, (_, name))| group.contains(pid) && name == "wezterm-gui.exe")
        .find_map(|(&pid, _)| sock_for(pid).map(|s| (pid, s)))
}

/// 跑 wezterm cli(--no-auto-start + 显式 socket,见模块头约束),成功返回 stdout。
fn cli(sock: &std::path::Path, args: &[&str]) -> Option<Vec<u8>> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("wezterm")
        .args(["cli", "--no-auto-start"])
        .args(args)
        .env("WEZTERM_UNIX_SOCKET", sock)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// 在 WezTerm 中恢复会话:GUI 已开则在其中新建 tab(cli spawn),否则/失败则
/// wezterm-gui start 新开窗口。argv 来自受信的 agent::resume_args,独立传参无 shell 拼接。
pub(crate) fn resume(dir: Option<&str>, argv: &[String]) -> std::io::Result<()> {
    if let Some((_, sock)) = any_gui() {
        let mut args: Vec<&str> = vec!["spawn"];
        if let Some(d) = dir {
            args.extend(["--cwd", d]);
        }
        args.push("--");
        args.extend(argv.iter().map(String::as_str));
        if cli(&sock, &args).is_some() {
            return Ok(());
        }
        // cli 失败(GUI 正在退出等竞态)→ 落到新窗口路径
    }
    let mut args: Vec<&str> = vec!["start"];
    if let Some(d) = dir {
        args.extend(["--cwd", d]);
    }
    args.push("--");
    args.extend(argv.iter().map(String::as_str));
    std::process::Command::new("wezterm-gui").args(&args).spawn().map(|_| ())
}
```

- [ ] **Step 2: resume_session 挂接**

lib.rs `resume_session` Windows 分支两处修改。

eff 选择(line 1060-1065)改为:

```rust
            let eff = match load_settings().resume_terminal.as_str() {
                "powershell" => "powershell",
                "cmd" => "cmd",
                // 选了 wezterm 但已卸载 → 落回 wt/powershell 链,与 wt 缺失回退同理
                "wezterm" if wezterm::available() => "wezterm",
                _ if wt_available() => "wt",
                _ => "powershell",
            };
```

spawned 匹配(line 1066-1106):三个现有分支的 `spawn()` 尾部统一补 `.map(|_| ())`,让整个 match 的类型从 `io::Result<Child>` 变为 `io::Result<()>`,并新增 wezterm 臂:

```rust
            let spawned: std::io::Result<()> = match eff {
                "powershell" => {
                    let mut c = Command::new("powershell");
                    c.args(["-NoExit", "-Command", &shell_join_for_windows(&resume, true)]);
                    if let Some(d) = &dir {
                        c.current_dir(d);
                    }
                    c.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
                }
                "cmd" => {
                    let mut c = Command::new("cmd");
                    c.raw_arg("/k").raw_arg(shell_join_for_windows(&resume, false));
                    if let Some(d) = &dir {
                        c.current_dir(d);
                    }
                    c.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
                }
                // WezTerm:GUI 已开则 cli spawn 新 tab,否则 wezterm-gui start 新窗口(模块内回退)。
                "wezterm" => wezterm::resume(dir.as_deref(), &resume),
                _ => {
                    let mut args: Vec<String> = vec!["-w".into(), "0".into(), "nt".into()];
                    if let Some(p) = wt_default_profile() {
                        args.push("-p".into());
                        args.push(p);
                    }
                    if let Some(d) = &dir {
                        args.push("-d".into());
                        args.push(d.clone());
                    }
                    args.extend(resume.iter().cloned());
                    Command::new("wt").args(&args).spawn().map(|_| ())
                }
            };
```

(现有三个分支的注释原样保留,此处为省篇幅未重复;`if let Err(e) = spawned` 后续错误处理逻辑不变。)

- [ ] **Step 3: 编译 + 全量单测**

```powershell
cargo test -p meowo-app --lib
```

Expected: PASS,无警告级回归(`cargo check -p meowo-app` 顺带确认)。

- [ ] **Step 4: Commit**

```bash
git add app/src-tauri/src/wezterm.rs app/src-tauri/src/lib.rs
git commit -m "feat(wezterm): 恢复会话支持 WezTerm——cli spawn 复用已开窗口,未开则 start 新窗"
```

---

### Task 4: 点卡片聚焦到 WezTerm pane(cli list 匹配 + activate-pane + 置前)

**Files:**
- Modify: `app/src-tauri/src/wezterm.rs`(`focus_pane`)
- Modify: `app/src-tauri/src/lib.rs`:
  - `focus_session_terminal` 兜底链(约 line 642-647)
  - `console_group_pids` 的 `terminal_host` 数组(line 294-296)

**Interfaces:**
- Consumes: Task 1 的 `parse_panes`/`match_pane`,Task 3 的 `gui_in_group`/`cli`;`crate::find_window_for_pids`、`crate::force_foreground`、`crate::console_group_pids`
- Produces: `pub(crate) fn focus_pane(group: &HashSet<u32>, want_title: &str, token: Option<&str>, cwd: Option<&str>) -> bool`

- [ ] **Step 1: terminal_host 加 wezterm-gui.exe**

lib.rs `console_group_pids` 中:

```rust
    let terminal_host = [
        "windowsterminal.exe", "conhost.exe", "openconsole.exe", "wt.exe", "wezterm-gui.exe",
    ];
```

(效果:祖先上溯到 wezterm-gui 即停并纳入,不再继续爬到启动器/壳进程附近。)

- [ ] **Step 2: 实现 focus_pane**

wezterm.rs 加:

```rust
/// 会话宿主是 WezTerm 时精确切到其 pane 并置前窗口;宿主不是 WezTerm(组内无 gui)返回 false。
/// pane 匹配不中时仍置前该 GUI 窗口(窗口级定位,与 WT 兜底同语义)——宿主已确认,
/// 不能再落回通用兜底去猜别的窗口。
pub(crate) fn focus_pane(
    group: &HashSet<u32>,
    want_title: &str,
    token: Option<&str>,
    cwd: Option<&str>,
) -> bool {
    let Some((gui_pid, sock)) = gui_in_group(group) else { return false };
    let matched = cli(&sock, &["list", "--format", "json"])
        .and_then(|out| String::from_utf8(out).ok())
        .and_then(|json| match_pane(&parse_panes(&json), want_title, token, cwd));
    if let Some(pane_id) = matched {
        let _ = cli(&sock, &["activate-pane", "--pane-id", &pane_id.to_string()]);
    }
    let mut target = HashSet::new();
    target.insert(gui_pid);
    if let Some(hwnd) = crate::find_window_for_pids(&target) {
        crate::force_foreground(hwnd);
    }
    true
}
```

- [ ] **Step 3: focus_session_terminal 挂接**

lib.rs `focus_session_terminal`(line 642-647)的兜底段改为:

```rust
        // 兜底：按进程组找宿主顶层窗口置前（命中正确窗口，但不保证切到具体标签）。宿主
        // WindowsTerminal.exe/conhost 是会话进程的祖先，其窗口 pid 落在进程组里 → 可靠命中正确窗口。
        let targets = console_group_pids(pid as u32);
        // WezTerm 宿主：自绘 GUI 无 UIA TabItem，上面的 WT 标签定位必然不中；组内探到
        // wezterm-gui 就走 wezterm cli 精确切 pane(内含窗口置前)，不再落通用兜底。
        if wezterm::focus_pane(&targets, want_str, token.as_deref(), cwd.as_deref()) {
            return;
        }
        if let Some(hwnd) = find_window_for_pids(&targets) {
            force_foreground(hwnd);
        }
```

注意:闭包体开头 `want`/`cwd` 的现有推导不变;`cwd` 变量在此处仍可用(闭包 move 进来的 `Option<String>`,`cwd_tab_hint` 只借用不消耗——确认 `let want = if title_based { title } else { cwd_tab_hint(cwd.as_deref()) };` 没有 move 掉 `cwd`,当前代码即如此)。

- [ ] **Step 4: 编译 + 全量单测**

```powershell
cargo test -p meowo-app --lib
```

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add app/src-tauri/src/wezterm.rs app/src-tauri/src/lib.rs
git commit -m "feat(wezterm): 点卡片聚焦 WezTerm pane——cli list 按 token/cwd/标题匹配后 activate-pane 置前"
```

---

### Task 5: 端到端手动验证(dev 实机)

**Files:** 无代码改动;验证不过则回上游任务修。

前置:`bun run tauri dev`(在 `app/` 下;若首次报 sidecar 缺失,先 `node scripts/prepare-sidecar.mjs`)。

- [ ] **Step 1: 检测与聚焦(WezTerm 已开)**

1. 开一个 WezTerm 窗口,在任意项目目录跑 `claude`,随便发个任务 → 看板应出现连接中的卡片(检测与终端无关,应当直接通过)。
2. 把别的窗口切到前台,点卡片 → WezTerm 窗口置前且切到该 pane 所在 tab(开两个 tab、其一跑 claude 验证切换)。
3. 同窗口开两个 tab 在**同一目录**分别跑 claude 两个会话 → 点各自卡片,应按任务标题分别切中;若标题相同(并列),允许只置前窗口不切 tab(设计如此,不算失败)。

- [ ] **Step 2: 设置页**

设置 → 通用 → 「打开未连接会话」下拉应出现 WezTerm 选项;选中它。

- [ ] **Step 3: resume(两条路径)**

1. WezTerm 开着:关掉某个 claude 会话(Ctrl+C 退出),等卡片变「已断开」,点卡片 → 应在**已开的 WezTerm 窗口新建 tab** 并 `claude --resume`,cwd 正确。
2. 完全退出 WezTerm:再点另一个已断开卡片 → 应**新开 WezTerm 窗口** resume。
3. spawn 失败回滚已有覆盖(现有逻辑),不必专测。

- [ ] **Step 4: 回归确认(WT 用户不受影响)**

设置切回 Windows Terminal,在 WT 里跑一个会话:点卡片聚焦、断开后 resume,行为与改动前一致。

- [ ] **Step 5: 全部通过后按 larry 流程收尾**

本地验证通过才可 push;push 后按需开 PR(macOS CI 会首次编译本分支,注意 `#[cfg(target_os = "windows")] mod wezterm;` 已把整个模块挡在 macOS 之外,不应有 dead_code 报错)。

---

## Self-Review 结论(已跑)

- **Spec 覆盖**:探测/设置页/resume 双路径/聚焦(token>cwd>标题、并列不猜)/窗口级兜底/WT 回归 — 各有任务。macOS 侧明确不做(spec 如此)。
- **占位符扫描**:无 TBD/TODO;Task 3 Step 2 注明「现有注释原样保留」是对既有代码的保真指令,非占位。
- **类型一致性**:`PaneInfo.pane_id: u64` ↔ `match_pane -> Option<u64>` ↔ `activate-pane --pane-id` 字符串化;`resume(dir: Option<&str>, argv: &[String]) -> io::Result<()>` ↔ 调用点 `wezterm::resume(dir.as_deref(), &resume)`;`focus_pane(&HashSet<u32>, &str, Option<&str>, Option<&str>) -> bool` ↔ 调用点一致。
