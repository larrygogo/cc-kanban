//! 终端集成：定位并聚焦会话所在的终端标签页（Windows UIA+Win32 / macOS AppleScript），
//! 以及在指定目录拉起 resume / 新建会话的终端进程。从 lib.rs 抽出。

use crate::proc::*;
use crate::session_command::is_safe_id;
use crate::settings::load_settings;
use crate::watch::emit_board_changed;
#[cfg(target_os = "windows")]
use crate::wezterm;
use crate::{db_path, now_ms, open_store};
#[cfg(target_os = "windows")]
use std::collections::HashSet;
use std::path::PathBuf;

/// 点击连接中会话后的实际定位结果。前端必须区分“会话已断开”和“进程仍在、但终端无法定位”，
/// 否则后者会表现成毫无反应，用户还会误以为重启 Meowo 能解决。
#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // macOS 专属失败原因在 Windows 构建中不会构造，反之亦然。
pub(crate) enum FocusSessionResult {
    Focused,
    HostFocused,
    AliveButNotFound,
    PermissionDenied,
    UnsupportedTerminal,
    ProcessEnded,
}

/// 枚举可见顶层窗口，返回第一个进程 pid 命中 targets 的窗口 HWND。
#[cfg(target_os = "windows")]
pub(crate) fn find_window_for_pids(
    targets: &HashSet<u32>,
) -> Option<windows_sys::Win32::Foundation::HWND> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };

    struct Ctx<'a> {
        targets: &'a HashSet<u32>,
        found: Option<HWND>,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut Ctx);
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        let mut wpid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut wpid);
        if ctx.targets.contains(&wpid) {
            ctx.found = Some(hwnd);
            return 0; // FALSE：停止枚举
        }
        TRUE
    }

    let mut ctx = Ctx {
        targets,
        found: None,
    };
    unsafe {
        EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM);
    }
    ctx.found
}

/// 用纯 Win32 EnumWindows+GetClassNameW 收集所有可见的 Windows Terminal 顶层窗口 HWND(as isize)。
/// 替代 UIA matcher 从桌面根逐节点跨进程爬树找窗口——后者默认 depth=7、每访问一个元素一次
/// CurrentClassName RPC，几十~上百窗口累计可达数百 ms；本函数纯进程内，微秒级。
#[cfg(target_os = "windows")]
pub(crate) fn enum_wt_hwnds() -> Vec<isize> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, IsWindowVisible,
    };

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        let mut buf = [0u16; 64];
        let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len > 0 {
            let cls = String::from_utf16_lossy(&buf[..len as usize]);
            if cls == "CASCADIA_HOSTING_WINDOW_CLASS" {
                let out = &mut *(lparam as *mut Vec<isize>);
                out.push(hwnd as isize);
            }
        }
        TRUE
    }

    let mut out: Vec<isize> = Vec::new();
    unsafe {
        EnumWindows(Some(cb), &mut out as *mut Vec<isize> as LPARAM);
    }
    out
}

/// claude 会把任务标题写进 Windows Terminal 标签页，并加一个**会随状态变化**的前缀符号：
/// 运行时是 braille spinner(⠐⠂…)，空闲/待输入时是 ✳(U+2733)，可能还有其它符号。
/// 归一化：剥掉开头所有「非字母数字」字符（覆盖任意状态符号 + 空格；任务标题几乎总以
/// 字母/数字/CJK 开头），并去掉尾部空白与截断省略号(…/...)。纯函数，便于单测。
#[allow(dead_code)] // 跨平台纯函数：Windows 上 WT/WezTerm 聚焦共用，非 Windows 仅单测使用
pub(crate) fn normalize_tab_title(s: &str) -> &str {
    s.trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim_end()
        .trim_end_matches(['…', '.'])
        .trim_end()
}

/// 标签页标题 `tab_name` 与会话标题 `want` 的匹配强度：2=精确(归一化后相等)，1=单向包含，0=不匹配。
/// 包含是**双向**的：兼容 claude 对长标题的截断(tab 标题是 want 的前缀)与轻微漂移。
/// `want` 为空或占位("(未命名会话)")时不参与匹配(返回 0)，避免误命中无关标签页。纯函数。
#[allow(dead_code)] // 同上：Windows 上 WT/WezTerm 聚焦共用，非 Windows 仅单测调用
pub(crate) fn tab_match_score(tab_name: &str, want: &str) -> u8 {
    let want = want.trim();
    if want.is_empty() || want == "(未命名会话)" {
        return 0;
    }
    let norm = normalize_tab_title(tab_name);
    if norm.is_empty() {
        return 0;
    }
    if norm == want {
        2
    } else if norm.contains(want) || want.contains(norm) {
        1
    } else {
        0
    }
}

/// 用 UI Automation 把对应会话的 Windows Terminal 标签页切到前台。
///
/// WT 单进程托管多标签/多窗口，按进程 PID 无法区分标签页（所有标签页同一个 HWND）。
/// 但 claude 会把任务标题写进标签页标题，故按标题精确定位标签页：枚举所有 WT 窗口的
/// TabItem，取匹配分最高的标签页，`Select` 选中后置前其窗口。命中返回 true；失败/无匹配返回 false。
///
/// 性能：仅当出现「多个同分标签页」需要消歧时，才用 `console_group_pids(root_pid)` 做一次进程扫描
/// (昂贵，要枚举系统所有进程)；常见的唯一精确匹配走纯 UIA 路径(~十几 ms)，不扫进程。
///
/// 注意：本函数必须在「干净 COM apartment 的线程」上调用（见 `focus_session` 的后台线程）。
/// `UIAutomation::new()` 会 CoInitialize 当前线程，Tauri 主线程已是 STA，复用会因 apartment 冲突失败。
#[cfg(target_os = "windows")]
pub(crate) fn focus_terminal_tab(root_pid: u32, want: &str, token: Option<&str>) -> bool {
    use uiautomation::patterns::UISelectionItemPattern;
    use uiautomation::types::{ControlType, Handle, TreeScope, UIProperty};
    use uiautomation::variants::Variant;
    use uiautomation::{UIAutomation, UIElement};

    let Ok(automation) = UIAutomation::new() else {
        return false;
    };

    // WT 顶层窗口：先用纯 Win32 EnumWindows+GetClassNameW 直接拿 HWND（进程内、微秒级），再
    // element_from_handle 只进入这几个窗口做 UIA。绕开 crate matcher 从桌面根逐节点 RPC 爬树
    // （默认 depth=7、每节点一次 CurrentClassName 跨进程调用，几十~上百窗口下可达 50-300ms）。
    // 保留 HWND 与 UIElement 配对：HWND 用于 GetWindowThreadProcessId 取窗口 pid（消歧用）与置前，
    // UIElement 用于 UIA 枚举标签页。
    let wt_windows: Vec<(isize, UIElement)> = enum_wt_hwnds()
        .into_iter()
        .filter_map(|h| {
            automation
                .element_from_handle(Handle::from(h))
                .ok()
                .map(|el| (h, el))
        })
        .collect();
    if wt_windows.is_empty() {
        return false;
    }

    // 标签页条件(TabItem)；其容器条件(TabView=ControlType::Tab)用于把搜索根收窄到标签条子树。
    let Ok(tab_cond) = automation.create_property_condition(
        UIProperty::ControlType,
        Variant::from(ControlType::TabItem as i32),
        None,
    ) else {
        return false;
    };
    let tabview_cond = automation
        .create_property_condition(
            UIProperty::ControlType,
            Variant::from(ControlType::Tab as i32),
            None,
        )
        .ok();
    // 缓存请求：让 FindAll 随元素一次性带回 Name，用 get_cached_name 读取，免每个 TabItem 一次
    // CurrentName 跨进程 RPC。
    let cache_req = automation.create_cache_request().ok();
    if let Some(ref cr) = cache_req {
        let _ = cr.add_property(UIProperty::Name);
    }

    // 取某 WT 窗口的 (TabItem, name) 列表。关键提速：先 find_first 定位 TabView 容器(ControlType::Tab，
    // 命中即停)，把 FindAll 的根从整窗收窄到标签条子树——避免对整窗 Descendants 全扫(含终端内容面板，
    // 实测每窗口 ~20ms)。容器内优先直接子(Children)，拿不到再容器 Descendants(兼容 TabItem 嵌套)；
    // 连容器都没有才退化为整窗 Descendants(异常布局兜底)。name 优先走缓存(get_cached_name)。
    let collect_tabs = |win: &UIElement| -> Vec<(UIElement, String)> {
        let find_tabitems = |root: &UIElement, scope: TreeScope| -> Vec<UIElement> {
            match &cache_req {
                Some(cr) => root
                    .find_all_build_cache(scope, &tab_cond, cr)
                    .unwrap_or_default(),
                None => root.find_all(scope, &tab_cond).unwrap_or_default(),
            }
        };
        let mut tabs: Vec<UIElement> = Vec::new();
        if let Some(tv) = tabview_cond
            .as_ref()
            .and_then(|c| win.find_first(TreeScope::Descendants, c).ok())
        {
            tabs = find_tabitems(&tv, TreeScope::Children);
            if tabs.is_empty() {
                tabs = find_tabitems(&tv, TreeScope::Descendants);
            }
        }
        if tabs.is_empty() {
            tabs = find_tabitems(win, TreeScope::Descendants);
        }
        tabs.into_iter()
            .map(|t| {
                let name = if cache_req.is_some() {
                    t.get_cached_name()
                        .or_else(|_| t.get_name())
                        .unwrap_or_default()
                } else {
                    t.get_name().unwrap_or_default()
                };
                (t, name)
            })
            .collect()
    };

    // 收集所有命中标签页：(匹配分, 窗口 HWND, 窗口 pid, 标签元素)。【不短路】——同一标题在多个窗口/标签
    // 出现时，按 console_group_pids(root_pid) 消歧到本会话所属窗口，否则会聚焦到错的同名标签。
    // want 来源因 agent 而异：claude/kimi=任务标题（kimi 另配 token 精确）、codex=cwd 末段目录名
    // (匹配 codex 自己写的 project-name 标签标题)。单个会话即精确命中；多个同名标签退窗口级。
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut matches: Vec<(u8, isize, u32, UIElement)> = Vec::new();
    for (hwnd, win) in &wt_windows {
        let mut win_pid: u32 = 0;
        unsafe {
            GetWindowThreadProcessId(*hwnd as HWND, &mut win_pid);
        }
        for (tab, name) in collect_tabs(win) {
            // token(=session_id 末 8 位，meowo-reporter 写进 kimi 标签) 命中即最高优先级 3、全局唯一——
            // 压倒按标题的语义匹配，且无需进程组消歧。否则退回标题匹配(0-2，含 codex 的 project-name)。
            let score = match token {
                Some(t) if !t.is_empty() && name.contains(t) => 3,
                _ => tab_match_score(&name, want),
            };
            if score > 0 {
                matches.push((score, *hwnd, win_pid, tab));
            }
        }
    }
    let max_score = matches.iter().map(|m| m.0).max().unwrap_or(0);
    if max_score == 0 {
        return false;
    }
    // 只保留最高分候选。
    matches.retain(|m| m.0 == max_score);
    // 唯一候选直接用；多个同分时按 console_group_pids(root_pid) 选与本会话同进程组的窗口（窗口宿主
    // WindowsTerminal.exe 是本会话进程的祖先，故其 pid 落在进程组里）——修「两个同名终端点击跳错」。
    // 选出本会话所属窗口(进程组含其窗口 pid)的候选。同一窗口里多个同名标签无法区分（UIA 不暴露
    // tab→进程），此时【不猜】——返回 false 让上层走窗口级定位，避免切到错的同名标签
    // （如 codex/kimi 同在某目录、标签都显示该目录名时，点哪个都别误切到另一个）。
    let idx = if matches.len() == 1 {
        0
    } else {
        let group = console_group_pids(root_pid);
        let in_group: Vec<usize> = (0..matches.len())
            .filter(|&i| group.contains(&matches[i].2))
            .collect();
        match in_group.as_slice() {
            [i] => *i,         // 唯一属于本会话窗口的候选 → 精确命中
            _ => return false, // 0 个或多个(同窗口多同名标签) → 不猜，退回窗口级
        }
    };
    let (_, hwnd, _, tab) = &matches[idx];
    // 选中该标签页（即使其窗口当前在后台也会切换激活标签页），再置前其窗口（直接用 HWND，免再取 native handle）。
    if let Ok(p) = tab.get_pattern::<UISelectionItemPattern>() {
        let _ = p.select();
    }
    force_foreground(*hwnd as HWND);
    true
}

/// 用 AttachThreadInput 绕过 Windows 后台进程 SetForegroundWindow 限制，可靠置顶目标窗口。
#[cfg(target_os = "windows")]
pub(crate) fn force_foreground(hwnd: windows_sys::Win32::Foundation::HWND) {
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
        SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    unsafe {
        let target_thread = GetWindowThreadProcessId(hwnd, null_mut());
        let fg = GetForegroundWindow();
        let fg_thread = if fg.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, null_mut())
        };
        let cur = GetCurrentThreadId();

        if fg_thread != 0 && fg_thread != cur {
            AttachThreadInput(cur, fg_thread, 1);
        }
        if target_thread != 0 && target_thread != cur {
            AttachThreadInput(cur, target_thread, 1);
        }

        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);

        if target_thread != 0 && target_thread != cur {
            AttachThreadInput(cur, target_thread, 0);
        }
        if fg_thread != 0 && fg_thread != cur {
            AttachThreadInput(cur, fg_thread, 0);
        }
    }
}

/// 聚焦某会话的终端。`title_based`=该 agent 是否把任务标题写进 WT 标签（claude 写→按任务标题精确切标签；
/// codex 不写→改用 cwd 末段目录名匹配它的 project-name 标签）。无论哪种，最终都能按进程组
/// 找到宿主窗口置前。
/// 必须在后台线程调用（保证干净 COM apartment + 不阻塞调用方）。返回实际定位结果；
/// focus_session 会把结果交给贴纸提示，「点击通知」回调则忽略结果。仅 Windows。
#[cfg(target_os = "windows")]
pub(crate) fn focus_session_terminal(
    pid: i64,
    title: Option<String>,
    cwd: Option<String>,
    token: Option<String>,
    title_based: bool,
) -> FocusSessionResult {
    // 匹配 WT 标签优先级：token(session_id 末 8 位，仅 kimi：meowo-reporter 写进其标签)
    // > 任务标题(claude/kimi)；codex 使用 cwd 末段目录名；最后才做窗口级兜底。
    // token 全局唯一，能区分同窗口同目录的同名标签——这是 kimi 精确聚焦的关键；codex 暂无此手段(见 agent.rs)。
    let want = if title_based {
        title
    } else {
        cwd_tab_hint(cwd.as_deref())
    };
    let want_str = want.as_deref().unwrap_or("");
    let has_token = token.as_deref().is_some_and(|t| !t.is_empty());
    if (!want_str.is_empty() || has_token)
        && focus_terminal_tab(pid as u32, want_str, token.as_deref())
    {
        return FocusSessionResult::Focused;
    }
    // 兜底：按进程组找宿主顶层窗口置前（命中正确窗口，但不保证切到具体标签）。宿主
    // WindowsTerminal.exe/conhost 是会话进程的祖先，其窗口 pid 落在进程组里 → 可靠命中正确窗口。
    let targets = console_group_pids(pid as u32);
    // WezTerm 宿主：自绘 GUI 无 UIA TabItem，上面的 WT 标签定位必然不中；组内探到
    // wezterm-gui 就走 wezterm cli 精确切 pane(内含窗口置前)，不再落通用兜底。
    match wezterm::focus_pane(&targets, want_str, token.as_deref(), cwd.as_deref()) {
        wezterm::FocusPaneResult::Focused => return FocusSessionResult::Focused,
        wezterm::FocusPaneResult::HostFocused => return FocusSessionResult::HostFocused,
        wezterm::FocusPaneResult::NotWezTerm => {}
    }
    if let Some(hwnd) = find_window_for_pids(&targets) {
        force_foreground(hwnd);
        return FocusSessionResult::HostFocused;
    }
    FocusSessionResult::UnsupportedTerminal
}

/// 从 cwd 取末段目录名，作为「不写标签标题」的 agent(codex) 的 WT 标签匹配线索——这类会话的
/// 标签默认显示当前目录名。空/根目录返回 None（退回窗口级定位）。
#[cfg(target_os = "windows")]
pub(crate) fn cwd_tab_hint(cwd: Option<&str>) -> Option<String> {
    let c = cwd?.trim_end_matches(['/', '\\']);
    std::path::Path::new(c)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// iTerm2 是否安装（任意常见位置）：先查标准路径，再用 mdfind 按 bundle id 兜底。
#[cfg(target_os = "macos")]
pub(crate) fn iterm_installed() -> bool {
    use std::path::Path;
    if Path::new("/Applications/iTerm.app").exists() {
        return true;
    }
    if let Ok(home) = std::env::var("HOME") {
        if Path::new(&home).join("Applications/iTerm.app").exists() {
            return true;
        }
    }
    std::process::Command::new("mdfind")
        .arg("kMDItemCFBundleIdentifier == 'com.googlecode.iterm2'")
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

/// Ghostty 是否安装（任意常见位置）：先查标准路径，再用 mdfind 按 bundle id 兜底。
#[cfg(target_os = "macos")]
pub(crate) fn ghostty_installed() -> bool {
    use std::path::Path;
    if Path::new("/Applications/Ghostty.app").exists() {
        return true;
    }
    if let Ok(home) = std::env::var("HOME") {
        if Path::new(&home).join("Applications/Ghostty.app").exists() {
            return true;
        }
    }
    std::process::Command::new("mdfind")
        .arg("kMDItemCFBundleIdentifier == 'com.mitchellh.ghostty'")
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

/// 读设置得出「打开未连接会话」用的终端宿主（macOS）。缺省 Terminal.app；
/// 选了 iTerm2 但未安装时回退 Terminal.app（避免 AppleScript 静默失败）。
///
/// 注意：设置为 ghostty 时这里也落到 Terminal.app——Ghostty 没有 AppleScript 支持，
/// 聚焦（按 tty 定位窗口）与 focus 流程里的 resume 回退都无法定位它；只有
/// `spawn_in_terminal` 的**新开终端**路径支持 Ghostty。这是已知取舍，不是疏漏。
#[cfg(target_os = "macos")]
pub(crate) fn resume_terminal_kind() -> crate::term_script::TermKind {
    use crate::term_script::TermKind;
    match crate::term_script::resume_kind_from_setting(&load_settings().resume_terminal) {
        TermKind::ITerm2 if iterm_installed() => TermKind::ITerm2,
        TermKind::ITerm2 => TermKind::Terminal,
        other => other,
    }
}

/// 聚焦终端时的 resume 回退命令：按 provider 分发（不再硬编码 claude）。是否真的回退由
/// `focus_session_terminal` 校验进程死活后决定（进程存活时绝不 resume，防 fork 重复会话）。
/// 未知 agent、或该 agent 未声明 resume 子命令 → 空 argv：只聚焦终端，不回退 resume。
///
/// **刻意定义在 `cfg` 之外**：它只用平台无关的 agent API，目前仅 macOS 调用。若把它埋进
/// `#[cfg(target_os = "macos")]` 块里，Windows 上的编译器根本不会看它——Phase 2 改了
/// `resume_args` 的签名，正是这样一路漏到 macOS CI 才炸的。逻辑留在 cfg 外，cfg 块里只放
/// 平台专属的 API 调用。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn resume_argv_for(provider: Option<&str>, session_id: Option<&str>) -> Vec<String> {
    session_id
        .zip(meowo_agent::resolve(provider))
        .and_then(|(session_id, a)| {
            let sub = a.resume_args();
            if sub.is_empty() {
                return None;
            }
            let mut argv = crate::relay::augment_argv(a.id(), a.launch_argv());
            argv.extend(sub.iter().map(|s| s.to_string()));
            argv.push(session_id.to_string());
            Some(argv)
        })
        .unwrap_or_default()
}

/// 拉起该 provider 时要注入的代理环境变量。未知 agent → 空（不注入）。
///
/// claude 也返回空——它的代理已写进 settings.json 的 `env` 块，对所有启动方式生效，无须再注入。
/// 详见 [`crate::proxy::launch_env`]。
///
/// 与 `resume_argv_for` 同理**刻意定义在 cfg 之外**：只用平台无关的 agent API，
/// 埋进 cfg 块会让另一平台的编译器看不到它，改签名时一路漏到对方 CI 才炸。
/// **恢复某个会话**时要注入的环境变量。
///
/// Claude 的会话可跨账号继续：恢复前会把会话资料同步到当前活跃账号，因此这里也必须使用当前
/// 活跃账号。其余 provider 尚未声明跨账号会话迁移能力，仍沿用该会话原先所属的账号。
/// 恢复路径改为一律按 `prepare_session_for_active_profile` 算出的实际账号取 env 之后，
/// 本函数只剩 macOS 的聚焦回退路径在用（它没有 prepare 的结果可依，只能自行推导）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn launch_env_for_session(
    provider: Option<&str>,
    session_id: &str,
) -> Vec<(String, String)> {
    let stored = profile_of_session(session_id);
    let active = provider.and_then(crate::profile::active_id);
    let profile = resume_profile(provider, stored, active);
    launch_env_for_profile(provider, profile.as_deref())
}

/// 声明了跨账号迁移的 agent 用**当前活跃账号**恢复（恢复前会把会话资料同步过去）；
/// 其余沿用该会话原先所属的账号。判据取自插件声明，不认识任何具体 agent。
fn supports_cross_account_resume(provider: Option<&str>) -> bool {
    meowo_agent::resolve(provider).is_some_and(|agent| agent.cross_account_session().is_some())
}

/// 同 [`launch_env_for_session`]：非 macOS 下只有它的单测在用。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn resume_profile(
    provider: Option<&str>,
    stored: Option<String>,
    active: Option<String>,
) -> Option<String> {
    if supports_cross_account_resume(provider) {
        active
    } else {
        stored
    }
}

/// 该会话（按 agent 的 session id）跑在哪个账号上。查不到 → None（默认账号）。
fn profile_of_session(session_id: &str) -> Option<String> {
    let store = meowo_store::Store::open(crate::db_path()).ok()?;
    let sid = store.find_session_id_pub(session_id).ok()??;
    store.session_profile(sid).ok().flatten()
}

fn ensure_session_profile_available(provider: &str, session_id: &str) -> Result<(), String> {
    // 跨账号迁移的 agent 恢复时用当前活跃账号；旧账号即使已删除，也不该阻止一个仍能在
    // 其他目录找到 transcript 的会话被接管。目标账号由 active_id 保证一定是已注册且
    // 目录存在的 profile。
    if supports_cross_account_resume(Some(provider)) {
        return Ok(());
    }
    let Some(profile) = profile_of_session(session_id) else {
        return Ok(());
    };
    let agent = meowo_agent::resolve(Some(provider)).ok_or("未知 agent")?;
    validate_session_profile_reference(
        Some(&profile),
        crate::profile::exists(agent.id().as_str(), &profile),
    )
}

/// 目标已有同长同 mtime 的副本时跳过复制。跨账号 takeover/restart 会背靠背同步两次
/// （杀进程前先做一次可恢复副本 + 启动前补最终增量），无此判断第二遍会把整棵会话树
/// （transcript 可达数百 MB）原样重拷一遍。Windows 的 CopyFileEx 保留写入时间，macOS
/// 的 clonefile 亦然；平台不保留 mtime 时判定不成立，退化为照常复制，只是失去优化。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn file_unchanged(source: &std::path::Path, target: &std::path::Path) -> bool {
    let Some((source_meta, target_meta)) = std::fs::metadata(source)
        .ok()
        .zip(std::fs::metadata(target).ok())
    else {
        return false;
    };
    source_meta.len() == target_meta.len()
        && source_meta
            .modified()
            .ok()
            .zip(target_meta.modified().ok())
            .is_some_and(|(source_mtime, target_mtime)| source_mtime == target_mtime)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn copy_dir_merge(source: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let destination = target.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_dir_merge(&entry.path(), &destination)?;
        } else if !file_unchanged(&entry.path(), &destination) {
            std::fs::copy(entry.path(), destination).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// 按插件声明的规格，把一个会话的 session 级数据复制到目标账号目录。
/// 目录名与布局全部来自 `spec`——本函数不认识任何具体 agent。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn sync_session_files(
    spec: &meowo_agent::profile::CrossAccountSession,
    source: &std::path::Path,
    target_root: &std::path::Path,
    session_id: &str,
) -> Result<(), String> {
    // source 形如 <root>/<transcript_dir>/<项目>/<id><ext>，上溯三级即数据根。
    let source_root = source
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .ok_or("会话路径格式异常")?;
    if source_root == target_root {
        return Ok(());
    }
    let project = source
        .parent()
        .and_then(|path| path.file_name())
        .ok_or("会话项目路径格式异常")?;
    let target = target_root
        .join(spec.transcript_dir)
        .join(project)
        .join(format!("{session_id}{}", spec.transcript_ext));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if !file_unchanged(source, &target) {
        std::fs::copy(source, &target).map_err(|error| format!("同步会话失败：{error}"))?;
    }

    // 回滚历史、环境快照、任务等按 session id 分目录保存的数据桶。
    for bucket in spec.session_buckets {
        let from = source_root.join(bucket).join(session_id);
        if from.is_dir() {
            copy_dir_merge(&from, &target_root.join(bucket).join(session_id))?;
        }
    }
    if !spec.subagents_beside_transcript {
        return Ok(());
    }
    // 子 agent 数据位于 transcript 同级的 `<session-id>/` 目录。
    let subagents = source.with_extension("");
    if subagents.is_dir() {
        copy_dir_merge(
            &subagents,
            &target.parent().unwrap_or(target_root).join(session_id),
        )?;
    }
    Ok(())
}

/// 把一个会话的资料同步到当前活跃账号目录，返回恢复时该用的账号。
/// 只复制 session 级数据（按插件声明的规格），绝不复制 credentials/settings/plugins。
/// 源文件保留，因此之后切回任意账号仍可按最新副本继续。
///
/// 未声明跨账号迁移的 agent 直接返回会话原账号，不做任何搬运。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn prepare_session_for_active_profile(
    provider: &str,
    session_id: &str,
) -> Result<Option<String>, String> {
    let Some(agent) = meowo_agent::resolve(Some(provider)) else {
        return Ok(profile_of_session(session_id));
    };
    let Some(spec) = agent.cross_account_session() else {
        return Ok(profile_of_session(session_id));
    };
    // 找不到本地 transcript 时不能硬报错：查找只覆盖默认数据目录与托管 profile 目录，
    // 用户自设该 agent 的 config-home（插件本就当一等配置支持）或 transcript 被清理策略
    // 删掉时都查不到，而 agent 自己的 resume 找得到会话。退回该会话记录的账号原样恢复，
    // 只跳过跨账号同步——同步本来也无从做起。
    let Some(source) = agent
        .telemetry()
        .and_then(|cap| cap.transcript())
        // 只给 session_id：hook 路径与 cwd 都不在手边，交给该 agent 的全局查找。
        .and_then(|spec| spec.resolve_transcript_path(None, None, session_id))
    else {
        return Ok(profile_of_session(session_id));
    };
    let target_profile = crate::profile::active_id(provider);
    let target_root = crate::profile::data_dir(provider, target_profile.as_deref())
        .ok_or("无法定位当前账号目录")?;
    sync_session_files(spec, &source, &target_root, session_id)?;
    Ok(target_profile)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn record_resumed_profile(session_id: &str, profile: Option<&str>) {
    let Ok(store) = open_store(&db_path()) else {
        return;
    };
    let Ok(Some(id)) = store.find_session_id_pub(session_id) else {
        return;
    };
    let _ = store.set_session_profile(id, profile);
}

#[cfg(all(test, any(target_os = "windows", target_os = "macos")))]
mod cross_account_resume_tests {
    use super::sync_session_files;

    /// 用 **claude 插件声明的真实规格**跑同步：目录名不再写死在宿主里，这条测试
    /// 因此同时验证了「规格取自插件」与「按规格搬对了东西」。
    fn claude_spec() -> &'static meowo_agent::profile::CrossAccountSession {
        meowo_agent::resolve(Some("claude"))
            .and_then(|agent| agent.cross_account_session())
            .expect("claude 声明了跨账号会话迁移")
    }

    #[test]
    fn copies_only_session_scoped_claude_data() {
        let root = std::env::temp_dir().join(format!("meowo-cross-account-{}", std::process::id()));
        let source_root = root.join("source");
        let target_root = root.join("target");
        let session = "session-1";
        let transcript = source_root
            .join("projects/project")
            .join(format!("{session}.jsonl"));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "conversation").unwrap();
        for bucket in ["file-history", "session-env", "tasks"] {
            let dir = source_root.join(bucket).join(session);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("item"), bucket).unwrap();
        }
        let subagent = transcript.with_extension("").join("subagents");
        std::fs::create_dir_all(&subagent).unwrap();
        std::fs::write(subagent.join("agent.jsonl"), "child").unwrap();
        std::fs::write(source_root.join(".credentials.json"), "secret").unwrap();

        sync_session_files(claude_spec(), &transcript, &target_root, session).unwrap();

        assert_eq!(
            std::fs::read_to_string(
                target_root
                    .join("projects/project")
                    .join(format!("{session}.jsonl"))
            )
            .unwrap(),
            "conversation"
        );
        for bucket in ["file-history", "session-env", "tasks"] {
            assert!(target_root
                .join(bucket)
                .join(session)
                .join("item")
                .is_file());
        }
        assert!(target_root
            .join("projects/project")
            .join(session)
            .join("subagents/agent.jsonl")
            .is_file());
        assert!(!target_root.join(".credentials.json").exists());
        assert_eq!(
            std::fs::read_to_string(&transcript).unwrap(),
            "conversation"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

fn validate_session_profile_reference(profile: Option<&str>, exists: bool) -> Result<(), String> {
    match profile {
        None => Ok(()),
        Some(_) if exists => Ok(()),
        Some(profile) => Err(format!("该会话所属账号“{profile}”已被删除，无法恢复")),
    }
}

#[cfg(test)]
mod session_profile_tests {
    use super::{resume_profile, validate_session_profile_reference};

    #[test]
    fn deleted_profile_blocks_resume_but_default_profile_does_not() {
        assert!(validate_session_profile_reference(None, false).is_ok());
        assert!(validate_session_profile_reference(Some("work"), true).is_ok());
        let error = validate_session_profile_reference(Some("deleted"), false).unwrap_err();
        assert!(error.contains("deleted"));
        assert!(error.contains("无法恢复"));
    }

    #[test]
    fn claude_resume_uses_active_account_while_other_agents_keep_the_stored_one() {
        assert_eq!(
            resume_profile(Some("claude"), None, Some("work".into())).as_deref(),
            Some("work")
        );
        // 切回默认账号必须得到明确的 None，不能又回落到会话之前所属的 profile。
        assert_eq!(
            resume_profile(Some("claude"), Some("work".into()), None),
            None
        );
        assert_eq!(
            resume_profile(
                Some("codex"),
                Some("original".into()),
                Some("active".into())
            )
            .as_deref(),
            Some("original")
        );
    }
}

/// 同上，但指定账号（profile）。`profile = None` → 用该 agent **当前活跃**的账号。
pub(crate) fn launch_env_for_profile(
    provider: Option<&str>,
    profile: Option<&str>,
) -> Vec<(String, String)> {
    let Some(a) = meowo_agent::resolve(provider) else {
        return Vec::new();
    };
    let id = match profile {
        Some(profile) => Some(profile.to_string()),
        None => crate::profile::active_id(a.id().as_str()),
    };
    launch_env_for_exact_profile(a, id.as_deref())
}

fn launch_env_for_exact_profile(
    agent: &'static dyn meowo_agent::AgentPlugin,
    profile: Option<&str>,
) -> Vec<(String, String)> {
    let mut env = crate::proxy::launch_env(agent.id());
    // 中转接入（relay）的环境变量：API base / key。与账号隔离变量正交，两者都要。
    env.extend(crate::relay::launch_env(agent.id()));
    env.extend(crate::profile::env_of(agent.id(), profile));
    env
}

fn launch_env_for_resume_target(provider: &str, profile: Option<&str>) -> Vec<(String, String)> {
    meowo_agent::resolve(Some(provider))
        .map(|agent| launch_env_for_exact_profile(agent, profile))
        .unwrap_or_default()
}

#[tauri::command]
pub(crate) async fn focus_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    pid: i64,
    title: Option<String>,
    cwd: Option<String>,
    session_id: Option<String>,
    provider: Option<String>,
) -> Result<FocusSessionResult, String> {
    if pid <= 0 {
        return Err("无效 pid".into());
    }
    // session_id 经 is_safe_id 校验（仅 `[A-Za-z0-9_-]`，杜绝注入：macOS 分支会把 id 注入 AppleScript）。
    // 必须用宽松校验——kimi 的 `session_<uuid>` 不合 UUID 形态，用严格 is_session_id 会把连接态的
    // kimi 卡挡在定位之前（Windows 上 session_id 实际并不参与 focus，仅 pid+title）。
    if let Some(id) = session_id.as_deref() {
        if !is_safe_id(id) {
            return Err("无效 session_id".into());
        }
        // 前端列表可能已过期，PID 也可能被同类 Agent 进程复用。只校验“这个 PID 是 Agent”不足以
        // 证明它仍属于用户点击的会话；必须与 DB 当前绑定一致，避免精准打开跳到另一会话。
        let id = id.to_string();
        let (owns_pid, sid) = tauri::async_runtime::spawn_blocking(move || {
            let store = open_store(&db_path())?;
            let Some(sid) = store.find_session_id_pub(&id).map_err(|e| e.to_string())? else {
                return Ok::<_, String>((false, None));
            };
            let owns = store
                .session_pid(sid)
                .map(|bound| bound == Some(pid))
                .map_err(|e| e.to_string())?;
            Ok((owns, Some(sid)))
        })
        .await
        .map_err(|e| e.to_string())??;
        if !owns_pid {
            return Ok(FocusSessionResult::ProcessEnded);
        }
        // 会话跑在 Meowo 自己的 PTY 里时，压根没有外部终端窗口可找：下面那套 WT 标签 / 窗口
        // 定位必然落空，用户只会收到一句「当前终端不支持自动跳转，会话仍在原终端运行」——
        // 而它根本不在什么原终端里。改按 session_open_in 把用户带到它真正所在的地方
        // （对话窗口，或 attach 到同一 PTY 的外部终端）。
        //
        // 注：只有**托管**会话走这里。用户自己在终端里敲起来的会话不归 Meowo 持有，没有 PTY
        // 可 attach，仍走下面的窗口定位——那本来就是它该去的地方。
        if let Some(sid) = sid.filter(|sid| state.ptys.is_managed(*sid)) {
            // reveal_session 含同步 IO（load_settings）与外部终端 spawn（杀软扫描可达数秒），
            // 与本文件其他 reveal_session 调用点一致放 blocking 池，不占 async 运行时线程。
            let app = app.clone();
            let ptys = state.ptys.clone();
            tauri::async_runtime::spawn_blocking(move || reveal_session(&app, &ptys, sid))
                .await
                .map_err(|e| e.to_string())??;
            return Ok(FocusSessionResult::Focused);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if !pid_alive_agent_quick(pid) {
            return Ok(FocusSessionResult::ProcessEnded);
        }
        // 该 provider 是否把任务标题写进 WT 标签：决定按标题切标签还是按 cwd 目录名切标签。
        // 缺省(None)→默认 agent；未知 agent → false，走窗口级定位兜底（不按标题瞎切标签）。
        let title_based =
            meowo_agent::resolve(provider.as_deref()).is_some_and(|a| a.sets_terminal_tab_title());
        // 只有声明 writes_tab_token 的 agent 才拿 session token 匹配；盲传 sid8 可能偶然命中
        // 别的标签文本，并以最高分跳错会话。Codex 首条消息后会覆盖 token，此时自然回退 cwd 匹配。
        let token = meowo_agent::resolve(provider.as_deref())
            .filter(|a| a.writes_tab_token())
            .and(session_id.as_deref())
            .map(meowo_reporter::tabtitle::short_sid)
            .filter(|s| !s.is_empty());
        tauri::async_runtime::spawn_blocking(move || {
            focus_session_terminal(pid, title, cwd, token, title_based)
        })
        .await
        .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = provider;
    #[cfg(target_os = "macos")]
    {
        let _ = title;
        if !pid_alive_agent_quick(pid) {
            return Ok(FocusSessionResult::ProcessEnded);
        }
        // ps/osascript（含首次 TCC 授权弹窗）可能长时间阻塞，放 blocking 池，不挡主线程事件循环；
        // 与旧 fire-and-forget 不同，这里 await 结果，让贴纸能解释“为什么没有跳转”。
        tauri::async_runtime::spawn_blocking(move || {
            let resume_argv = resume_argv_for(provider.as_deref(), session_id.as_deref());
            // 保留恢复参数供聚焦期间进程退出的判定路径使用；实际恢复改由前端明确确认。
            // 账号按**该会话自己的**取（没有 session_id 就退回当前活跃账号）。
            let env = match session_id.as_deref() {
                Some(sid) => launch_env_for_session(provider.as_deref(), sid),
                None => launch_env_for_profile(provider.as_deref(), None),
            };
            let env_prefix = env_prefix_posix(&env);
            crate::macos::terminal::focus_session_terminal(
                pid,
                cwd.as_deref(),
                &resume_argv,
                resume_terminal_kind(),
                &env_prefix,
            )
        })
        .await
        .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (pid, title, cwd, session_id);
        Err("当前平台不支持".into())
    }
}

/// 在系统文件管理器中打开会话的项目目录（卡片右键菜单用）。
/// 目录须真实存在——DB 记录的 cwd 可能过期（项目被移动/删除），不存在时明确报错而非静默无事发生。
/// 不经 shell 直接 spawn 文件管理器，目录路径作为独立 argv 传入，无注入面。
#[tauri::command]
pub(crate) async fn open_project_dir(cwd: String) -> Result<(), String> {
    // async + spawn_blocking：is_dir 是文件 IO，spawn 子进程更是可被杀软拖到秒级的操作
    // （同类教训见 path_has_exe 的注释——0.2.0 设置页卡死的根因），不进主线程。
    tauri::async_runtime::spawn_blocking(move || open_project_dir_blocking(&cwd))
        .await
        .map_err(|e| e.to_string())?
}

fn open_project_dir_blocking(cwd: &str) -> Result<(), String> {
    let dir = cwd.trim();
    if dir.is_empty() || !std::path::Path::new(dir).is_dir() {
        return Err("目录不存在".into());
    }
    #[cfg(target_os = "windows")]
    {
        // kimi 等 provider 写入的 cwd 可能是正斜杠形式，explorer 对正斜杠路径会打开默认目录而非目标。
        let dir = dir.replace('/', "\\");
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    // macOS：open 偶发慢（Finder 冷启动），放后台线程；status() 等待回收，避免僵尸进程。
    #[cfg(target_os = "macos")]
    {
        let dir = dir.to_string();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("open").arg(&dir).status();
        });
    }
    Ok(())
}

/// 把 `cwd` 收敛成「可安全传给 wt -d」的目录：必须非空、真实存在的目录，且不含会破坏 wt
/// 命令行解析的元字符(`;` `"`)。不满足则返回 None（调用方退化为不带 -d）。
/// 在 PATH 各目录中查找指定文件是否存在。不 spawn `where` 子进程——GUI 进程冷启动后
/// 首次 spawn 控制台子进程要数秒（新建 conhost + 杀软扫描），而同步命令跑在主线程，
/// 会把整个事件循环（所有窗口）堵死，这正是 0.2.0 设置页在 Windows 上"卡死"的根因。
/// 用 symlink_metadata 而非 exists()：wt.exe 通常是 App Execution Alias
/// （APPEXECLINK reparse point），fs::metadata 跟随它会失败、误判为不存在。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn path_has_exe(path_var: &std::ffi::OsStr, exe: &str) -> bool {
    std::env::split_paths(path_var).any(|dir| dir.join(exe).symlink_metadata().is_ok())
}

/// Windows Terminal（wt.exe）是否在 PATH 上。进程内缓存：安装状态运行期间基本不变，
/// resume_session 每次恢复会话都要查询，保持微秒级。
#[cfg(target_os = "windows")]
pub(crate) fn wt_available() -> bool {
    use std::sync::OnceLock;
    static WT_ON_PATH: OnceLock<bool> = OnceLock::new();
    *WT_ON_PATH.get_or_init(|| std::env::var_os("PATH").is_some_and(|p| path_has_exe(&p, "wt.exe")))
}

/// PowerShell 7（pwsh.exe）是否在 PATH 上。进程内缓存，同 wt_available。
/// 一键安装用它优先于 Windows PowerShell 5.1（见 build_install_command 说明）。
#[cfg(target_os = "windows")]
pub(crate) fn pwsh_available() -> bool {
    use std::sync::OnceLock;
    static PWSH_ON_PATH: OnceLock<bool> = OnceLock::new();
    *PWSH_ON_PATH
        .get_or_init(|| std::env::var_os("PATH").is_some_and(|p| path_has_exe(&p, "pwsh.exe")))
}

/// 定位 Windows Terminal 的 settings.json（Store 版 / Preview / 未打包版三处）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn wt_settings_path() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    [
        r"Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json",
        r"Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json",
        r"Microsoft\Windows Terminal\settings.json",
    ]
    .into_iter()
    .map(|rel| base.join(rel))
    .find(|p| p.is_file())
}

/// 去掉 JSONC 注释（WT settings.json 允许 // 与 /* */，且字符串里常有 URL 的 //）。
/// 按字节扫描、正确跳过字符串与转义，不破坏多字节 UTF-8（profile 名可能含中文）。纯函数便于单测。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn strip_jsonc_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]); // 保留转义字符，避免把 \" 误判为字符串结束
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
        } else if c == b'"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

/// 从 WT settings.json 的 JSON 取默认 profile 名：defaultProfile 为 GUID 时在 profiles.list
/// 按 guid 找 name（大小写不敏感）；本身是名字则直接用。找不到则 None。纯函数便于单测。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_wt_default_profile(v: &serde_json::Value) -> Option<String> {
    let def = v.get("defaultProfile").and_then(|x| x.as_str())?.trim();
    if def.is_empty() {
        return None;
    }
    if !def.starts_with('{') {
        return Some(def.to_string()); // 直接配的是 profile 名
    }
    // 新格式 profiles.list 是数组；老格式 profiles 直接是数组。
    let list = v.get("profiles").and_then(|p| {
        p.get("list")
            .and_then(|l| l.as_array())
            .or_else(|| p.as_array())
    })?;
    list.iter().find_map(|prof| {
        let guid = prof.get("guid").and_then(|g| g.as_str())?;
        guid.eq_ignore_ascii_case(def)
            .then(|| {
                prof.get("name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            })
            .flatten()
    })
}

/// 用户 WT 默认 profile 名（多为 PowerShell）。进程内缓存：与 wt_available 一致，运行期基本不变
/// （改了默认 profile 需重启 app 才生效）。读不到/解析失败/无匹配 → None，调用方退化为不带 -p。
#[cfg(target_os = "windows")]
pub(crate) fn wt_default_profile() -> Option<String> {
    use std::sync::OnceLock;
    static PROFILE: OnceLock<Option<String>> = OnceLock::new();
    PROFILE
        .get_or_init(|| {
            let raw = std::fs::read_to_string(wt_settings_path()?).ok()?;
            let v: serde_json::Value = serde_json::from_str(&strip_jsonc_comments(&raw)).ok()?;
            parse_wt_default_profile(&v)
        })
        .clone()
}

#[cfg(target_os = "windows")]
pub(crate) fn safe_cwd(cwd: Option<&str>) -> Option<String> {
    let d = cwd?.trim();
    // 含 ; " 会破坏命令行解析；以 - 开头会被 wt 当成选项（真实 Windows 路径不会以 - 开头）。
    if d.is_empty() || d.contains([';', '"']) || d.starts_with('-') {
        return None;
    }
    std::path::Path::new(d).is_dir().then(|| d.to_string())
}

/// macOS resume 的 cwd 准入：None/空白走无目录脚本（合法）；给了目录就必须真实存在——
/// 目录已删时 AppleScript 里 `cd` 失败被 `&&` 短路，resume 根本没跑，osascript 却返回成功
/// （假恢复：终端空空，DB 却已乐观复活）。与 Windows 侧 safe_cwd 的 is_dir 校验同一纪律。
/// 纯函数便于在非 macOS 上单测。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn mac_resume_cwd_valid(cwd: Option<&str>) -> bool {
    match cwd.map(str::trim).filter(|d| !d.is_empty()) {
        Some(dir) => std::path::Path::new(dir).is_dir(),
        None => true,
    }
}

/// 把 resume 命令 argv 拼成交给 `powershell -Command` / `cmd /k` 的单行命令串。
/// kimi/codex 的可执行是 USERPROFILE 下的绝对路径，用户名可含空格 / $ / ' / % 等合法字符：
/// - PowerShell：含空白或 $ ` ' 的参数用**单引号字面量**包裹（内嵌单引号翻倍）——双引号内 $ 与反引号
///   仍会被插值展开（如 C:\Users\a$b 被吞成 C:\Users\a），单引号内一切按字面处理；带引号的命令路径
///   需以调用运算符 `&` 前缀。
/// - cmd：含空白的参数加双引号。cmd 没有字面量引用机制，引号内成对的 %VAR% 仍会展开——属 cmd 本身
///   限制，用户名含 % 的机器请改用 wt/powershell（此处不做 ^ 转义：引号内 ^ 会按字面残留）。
///
/// 纯函数便于单测。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn shell_join_for_windows(args: &[String], powershell: bool) -> String {
    if powershell {
        let quoted: Vec<String> = args
            .iter()
            .map(|a| {
                if a.chars().any(char::is_whitespace)
                    || a.contains([
                        '$', '`', '\'', '"', '&', ';', '|', '<', '>', '(', ')', '{', '}',
                    ])
                {
                    format!("'{}'", a.replace('\'', "''"))
                } else {
                    a.clone()
                }
            })
            .collect();
        let joined = quoted.join(" ");
        if quoted.first().is_some_and(|f| f.starts_with('\'')) {
            format!("& {joined}")
        } else {
            joined
        }
    } else {
        args.iter()
            .map(|a| {
                if a.chars().any(char::is_whitespace) {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ═══ 代理环境变量的注入前缀 ═══
//
// codex / kimi 没法从配置文件配代理（见 meowo_agent::proxy 的能力表），只认进程环境变量。而
// `Command::env()` 在这里**靠不住**：wt 会把请求交给**已存在的** Windows Terminal 实例去开标签、
// wezterm 交给 mux server、macOS 的 Terminal.app 更是早就在跑了——新进程都不是我们的子进程，
// 继承不到我们设的 env。唯一可靠的办法是把赋值**写进命令串**本身。
//
// 于是代理串（用户填的）会进到 shell 命令里 → 三种 shell 各自的转义必须做对，否则就是注入面。
// 值虽已过 validate（无空格、协议白名单、host/port 合法），仍按「一律正确转义」处理，不赌。

/// 所有启动路径都先清掉继承的代理变量。否则「直连」只是没有新增变量，仍会继承 Meowo
/// 自己启动时的 HTTPS_PROXY / ALL_PROXY；自定义 HTTP 代理也可能被旧的 ALL_PROXY 抢走。
pub(crate) const PROXY_ENV_KEYS: [&str; 8] = [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
    "no_proxy",
];

/// PowerShell：先 `$env:K=$null; `，再 `$env:K='v'; `。单引号字面量内一切按字面处理（双引号内 `$`/反引号会插值），
/// 内嵌单引号翻倍转义。与 `shell_join_for_windows` 的引用规则同源。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn env_prefix_powershell(env: &[(String, String)]) -> String {
    let clear: String = PROXY_ENV_KEYS
        .iter()
        .map(|k| format!("$env:{k}=$null; "))
        .collect();
    let set: String = env
        .iter()
        .map(|(k, v)| format!("$env:{k}='{}'; ", v.replace('\'', "''")))
        .collect();
    format!("{clear}{set}")
}

/// POSIX：`K='v' ` —— 命令前缀式赋值（只作用于这一条命令，无需 export）。
///
/// **键名必须不加引号**，否则 shell 不再把它识别为赋值（`'K=v' cmd` 会被当成一个命令名）——
/// 这正是不能把 `K=v` 当作一个 argv 项塞进 AppleScript 的原因（那边每项都套了 `quoted form`）。
/// 键名是我们自己的常量（安全），值按 POSIX 单引号规则转义（`'` → `'\''`）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn env_prefix_posix(env: &[(String, String)]) -> String {
    let clear = format!("unset {}; ", PROXY_ENV_KEYS.join(" "));
    let set: String = env
        .iter()
        .map(|(k, v)| format!("{k}='{}' ", v.replace('\'', r"'\''")))
        .collect();
    format!("{clear}{set}")
}

/// POSIX shell 参数逐项单引号包裹并拼接；单引号按 `'\''` 转义。
///
/// 例：`["a", "b'c"] -> "'a' 'b'\''c'"`。
///
/// 刻意留在 cfg 之外（allow 而非 cfg 门控）：纯字符串逻辑全平台可编译可单测，
/// Ghostty 本身也支持 Linux——将来加 Linux 终端集成时直接复用（纪律同 `resume_argv_for`）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn shell_join_for_posix(args: &[String]) -> String {
    args.iter()
        .map(|arg| format!("'{}'", arg.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 组装给 Ghostty 执行的一条 `sh -lc` 命令。
///
/// - `env_prefix` 形如 `source '<tmp>' && rm -f '<tmp>' && `（见 `env_source_prefix_posix`）；
/// - `cwd` 非空时先 `cd` 到目标目录（转义同上）；
/// - `argv` 逐项按 POSIX 单引号规则转义并拼接。
///
/// `argv` 为空时返回 `None`，调用方应视为不可执行。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn ghostty_shell_command(cwd: Option<&str>, argv: &[String], env_prefix: &str) -> Option<String> {
    if argv.is_empty() {
        return None;
    }
    let run = format!("{env_prefix}{}", shell_join_for_posix(argv));
    let cmd = match cwd.map(str::trim) {
        Some("") | None => run,
        Some(dir) => format!("cd '{}' && {run}", dir.replace('\'', r"'\''")),
    };
    Some(cmd)
}

/// 用 Ghostty 新开终端并执行恢复命令。
///
/// 经 `open -na Ghostty --args -e /bin/sh -lc <cmd>` 拉起——`open` 走 LaunchServices，
/// `Command::env` 传不进去，env 注入只能靠命令里 source 临时文件（正好与既有纪律一致）。
/// `-n` 是必须的：不带它时对已运行的应用 `--args` 根本不会送达；代价是每次都新起一个
/// Ghostty 实例。返回值表示是否成功发起 spawn（不等待命令执行完成）。
#[cfg(target_os = "macos")]
fn resume_session_ghostty(cwd: Option<&str>, argv: &[String], env_prefix: &str) -> bool {
    let Some(cmd) = ghostty_shell_command(cwd, argv, env_prefix) else {
        return false;
    };
    std::process::Command::new("open")
        .args(["-na", "Ghostty", "--args", "-e", "/bin/sh", "-lc", &cmd])
        .spawn()
        .is_ok()
}

/// macOS 恢复会话的 env 注入文件：赋值写进临时文件（unix 下创建即 0600），终端命令只出现
/// `source '<tmp>' && rm -f '<tmp>' && ` 前缀——密钥值不再落在可见命令行上。
///
/// 起因：恢复会话的 env 带着中转 API key（ANTHROPIC_API_KEY / KIMI_MODEL_API_KEY /
/// GEMINI_API_KEY），此前由 [`env_prefix_posix`] 拼成 `K='sk-…' ` 前缀直接进终端命令——
/// iTerm2 会把这行命令写进 ~/.zsh_history，Terminal.app 则留在滚动缓冲区，都是明文落盘。
/// 文件由恢复命令 source 成功后立即自删；命令若没来得及执行（窗口被直接关掉），残留文件
/// 权限 0600 仅本人可读，并随 $TMPDIR 周期清理。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn env_source_prefix_posix(env: &[(String, String)]) -> Result<String, String> {
    // source 进来的赋值必须 export 才会传给恢复出来的子进程；unset 清掉继承的代理变量
    // （语义同 env_prefix_posix），值按同一套 POSIX 单引号规则转义。
    let mut content = format!("unset {}\n", PROXY_ENV_KEYS.join(" "));
    for (key, value) in env {
        content.push_str(&format!(
            "export {key}='{}'\n",
            value.replace('\'', r"'\''")
        ));
    }
    let dir = std::env::temp_dir();
    // create_new 杜绝符号链接/抢占覆写；撞名（概率可忽略）换名重试。
    for _ in 0..3 {
        let mut token = [0u8; 8];
        if getrandom::fill(&mut token).is_err() {
            // OS RNG 不可用属于极端退化；混入进程号即可，文件本就是 0600。
            token = u64::from(std::process::id()).to_le_bytes();
        }
        let token_hex: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("meowo-env-{}-{token_hex}", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // 0600 必须与创建同一步完成：先建后 chmod 会留出一个默认权限的窗口期，
        // 而密钥内容恰好在这个窗口期内写入。
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let Ok(mut file) = options.open(&path) else {
            continue;
        };
        use std::io::Write as _;
        if let Err(error) = file.write_all(content.as_bytes()) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("写入启动配置失败：{error}"));
        }
        drop(file);
        // 路径按同一套单引号规则转义后拼进 source/rm（temp_dir 一般不会带引号，纪律不松）。
        let quoted = path.to_string_lossy().replace('\'', r"'\''");
        return Ok(format!("source '{quoted}' && rm -f '{quoted}' && "));
    }
    Err("无法创建 env 注入临时文件".into())
}

/// 单 pid 判活（廉价版，resume 前奏专用）：Windows 走 Toolhelp 快照（1-3ms，避免 sysinfo 全进程
/// OpenProcess 刷新的 30-120ms 拖慢「点下即显示已连接」），Unix 走一次 ps。
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn pid_alive_agent_quick(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        snapshot_processes()
            .get(&(pid as u32))
            .map(|(_, name)| meowo_agent::is_agent_process(name))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        pid_is_agent_ps(pid)
    }
}

/// resume 的跨平台前奏（须在后台线程调用）：乐观复活 → 兜底刷新。
/// 返回真的复活了才是 Some(sid)——供 spawn 失败回滚,绝不回滚未被本次复活的真连接会话。
///
/// 乐观复活:resume 是看板主动发起的,已知恢复哪个会话——先复活并清旧 pid,卡片即刻显示已连接,
/// 不必等 hook(尤其 codex 的 session_start hook 要到首个 turn 才触发)。旧 pid 死活经
/// pid_alive_agent_quick 校验后以 dead_pid 传入,由 store 层 `pid=?` 守卫原子闭合 TOCTOU
/// (见 revive_for_resume)。emit 兜底刷新,不依赖 db watcher 存活。
///
/// **不做**恢复计划解析：两个调用方都在调用前自己算过一遍并丢弃这里的结果，而
/// resolve_resume_plan 在 DB cwd 失真时要 read_dir 整个 projects 目录再逐行读 JSONL
/// （50-500ms），白算一遍很贵。计划由调用方负责传给 broker。
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn prepare_resume(app: &tauri::AppHandle, session_id: &str) -> Option<i64> {
    let revived = (|| {
        let store = open_store(&db_path()).ok()?;
        let sid = store.find_session_id_pub(session_id).ok().flatten()?;
        let dead_pid = store
            .session_pid(sid)
            .ok()
            .flatten()
            .filter(|&p| p > 0 && !pid_alive_agent_quick(p));
        match store.revive_for_resume(sid, now_ms(), dead_pid) {
            Ok(true) => Some(sid),
            _ => None,
        }
    })();
    emit_board_changed(app, "resume");
    revived
}

/// 只读解析恢复计划；不得改状态。restart 路径必须先确认计划有效，再结束原进程。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn resolve_resume_plan(
    session_id: &str,
    cwd: Option<&str>,
    provider: &str,
) -> (Option<String>, Vec<String>) {
    let agent = meowo_agent::resolve(Some(provider));
    // resume 必须在会话原项目目录下运行才找得到会话。DB 的 cwd 可能为空或失真（旧会话 / 压缩漏
    // SessionStart / 目录被移动）。能从 transcript 读出权威 cwd 的 agent（claude）据此纠正；
    // 其余原样采信 DB——此前这里无条件走 claude 的解析路径，非 claude 会话靠「在 ~/.claude/projects
    // 里找不到就回退 DB cwd」的巧合才拿到正确结果。
    let resolved = match agent
        .and_then(|a| a.telemetry())
        .and_then(|t| t.transcript())
    {
        Some(spec) => spec.resolve_cwd(cwd, session_id),
        None => meowo_agent::default_resolve_cwd(cwd),
    };
    // 恢复命令按 provider 取（claude --resume / kimi -r …）；可执行名+参数均来自受信 agent 定义。
    // 未知 agent、或该 agent 未声明 resume 子命令 → 空 argv：调用方的 spawn 会失败并回滚复活，
    // 好过拿 claude 的参数去拉起别的 CLI。
    let resume = agent
        .and_then(|a| a.resume_argv(session_id))
        .unwrap_or_default();
    (resolved, resume)
}

/// 把会话的启动选项拼回恢复 argv：读存的选择 map（sessions.launch_args，claim 时落库），
/// 合并本次接管的覆盖（用户在恢复时改权限模式），按插件声明表现场翻译成 flag。
/// 覆盖非空时把合并结果写回——本次选择成为会话新的持久形态，之后的恢复自动沿用；
/// 这也覆盖「当年不是以跳过权限新建、恢复时想切过去」的场景（存量会话同样适用）。
///
/// 权限模式等选项是**启动参数**而非会话状态——不回放的话，每次 resume/接管重启的
/// 进程都会重置成 CLI 默认（实拍反馈「每次接管权限模式都会变」）。
/// 插入点在 launch 前缀之后、resume 子命令之前，与首次启动的参数次序一致。
/// 读库/解析失败一律静默跳过：回放是增强，不能因此拦掉恢复本身。
/// 把会话的附加目录拼回恢复 argv（sessions.extra_dirs，claim 时落库）：每个目录一对
/// `<flag> <dir>`，插入点与 [`splice_stored_launch_args`] 相同（launch 前缀之后、resume
/// 子命令之前）。不回放的话恢复的进程就丢了那些仓的访问权——附加目录是**启动参数**。
/// 已消失的目录跳过（agent 对不存在的目录报错会拦掉整次恢复）；读库失败静默跳过。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn splice_stored_extra_dirs(resume: &mut Vec<String>, provider: &str, sid: i64) {
    let Some(agent) = meowo_agent::resolve(Some(provider)) else { return };
    let Some(flag) = agent.extra_dir_flag() else { return };
    let Ok(store) = open_store(&db_path()) else { return };
    let dirs: Vec<String> = store
        .session_extra_dirs(sid)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let args: Vec<String> = dirs
        .into_iter()
        .filter(|d| std::path::Path::new(d).is_dir())
        .flat_map(|d| [flag.to_string(), d])
        .collect();
    if args.is_empty() {
        return;
    }
    let sub_len = agent.resume_args().len();
    let insert_at = resume.len().saturating_sub(sub_len + 1);
    for (offset, arg) in args.into_iter().enumerate() {
        resume.insert(insert_at + offset, arg);
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn splice_stored_launch_args(
    resume: &mut Vec<String>,
    provider: &str,
    sid: i64,
    overrides: Option<&std::collections::HashMap<String, String>>,
) {
    let Some(agent) = meowo_agent::resolve(Some(provider)) else { return };
    let Ok(store) = open_store(&db_path()) else { return };
    let mut selections: std::collections::HashMap<String, String> = store
        .session_launch_args(sid)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    if let Some(over) = overrides.filter(|o| !o.is_empty()) {
        for (option, choice) in over {
            selections.insert(option.clone(), choice.clone());
        }
        if let Ok(json) = serde_json::to_string(&selections) {
            let _ = store.set_session_launch_args(sid, &json);
        }
    }
    // 用运行时观测校正存档的模型档：用户在**终端里**手动 /model 切换不经 GUI，存档
    // 不知道——不校正的话，恢复回放的旧档会把那次切换覆盖掉（实拍：1M 档回落 200K）。
    // 观测源是 statusline 上报的 session_context（display_name + 精确窗口大小），只做
    // 保守推断，认不出就保持存档——见 [`corrected_model_choice`]。本次接管带显式覆盖
    // 时用户刚亲手选过，观测不再插手。
    if overrides.is_none_or(|o| !o.contains_key("model")) {
        if let Some(stored) = selections.get("model").cloned() {
            let observed = store
                .session_header(sid)
                .ok()
                .and_then(|h| store.session_context(&h.cc_session_id).ok());
            if let Some(ctx) = observed {
                if let Some(next) =
                    corrected_model_choice(&stored, ctx.model.as_deref(), ctx.window_size)
                {
                    if next != stored {
                        selections.insert("model".into(), next);
                        if let Ok(json) = serde_json::to_string(&selections) {
                            let _ = store.set_session_launch_args(sid, &json);
                        }
                    }
                }
            }
        }
    }
    if selections.is_empty() {
        return;
    }
    // 未知 option/choice 由 resolve_launch_args 兜底（忽略/落默认），用户输入进不了 argv。
    let args = meowo_agent::resolve_launch_args(agent.launch_options(), &selections);
    if args.is_empty() {
        return;
    }
    // resume argv = launch 前缀 + resume 子命令 + 会话 id（registry::resume_argv 的构造）。
    let sub_len = agent.resume_args().len();
    let insert_at = resume.len().saturating_sub(sub_len + 1);
    for (offset, arg) in args.into_iter().enumerate() {
        resume.insert(insert_at + offset, arg);
    }
}

/// 由运行时观测（statusline 的模型显示名 + 精确上下文窗口）推断存档模型档应改成哪档。
/// 返回 None = 证据不足，保持存档不动。规则刻意保守：
/// - 存档 `opusplan` 不动（执行期在 Opus/Sonnet 间切换是它的正常形态，观测必然「不符」）；
/// - 家族(fable/opus/sonnet/haiku)从 display_name 小写包含判断，认不出则沿用存档家族；
/// - 窗口 ≥500K 视为 1M 档，加 `[1m]` 后缀（haiku 无 1M 变体，恒裸）；窗口缺失则沿用
///   存档的后缀——绝不在证据缺位时改写用户的选择。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn corrected_model_choice(
    stored: &str,
    display_name: Option<&str>,
    window_size: Option<i64>,
) -> Option<String> {
    if stored == "opusplan" || stored == "default" {
        return None;
    }
    let family_of = |s: &str| {
        let lower = s.to_lowercase();
        ["fable", "opus", "sonnet", "haiku"]
            .into_iter()
            .find(|f| lower.contains(f))
    };
    let stored_family = family_of(stored)?;
    let family = display_name.and_then(family_of).unwrap_or(stored_family);
    let one_m = match window_size {
        Some(w) => w >= 500_000,
        None => stored.ends_with("[1m]"),
    };
    Some(if one_m && family != "haiku" {
        format!("{family}[1m]")
    } else {
        family.to_string()
    })
}

/// resume 的终端 spawn 失败时回滚乐观复活（收尾回 ended）：GUI 构建下 stderr 不可见，
/// 至少让卡片立即回落「已断开」，而不是假显示「已连接」直到 120s 宽限过期。
/// 只对 prepare_resume 返回 Some(确实复活过)的会话调用——未被本次复活的真连接会话不得误收尾。
///
/// 走 pid CAS 版（end_session_if_unclaimed,`pid IS NULL` 守卫）而非裸 end_session：
/// 复活与回滚之间新进程的 hook 可能已认领 pid（会话真活了）,对称于 revive_for_resume 的
/// `pid=?` 守卫,绝不误杀刚认领的活会话——裸时间戳守卫挡不住按到达时刻盖章的 hook。
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn rollback_failed_resume(sid: i64) {
    if let Ok(store) = open_store(&db_path()) {
        let _ = store.end_session_if_unclaimed(sid, now_ms());
    }
}

/// 在 `cwd` 打开一个终端并运行 `argv`，终端类型由 `terminal`（同 settings.resume_terminal 取值）决定。
/// resume（`claude --resume <id>`）与 new（裸 `claude`）共用——唯一区别是传入的 argv。成功返回 true。
/// Windows：powershell/cmd/wezterm/wt，缺失回退链同 resume 旧逻辑；wt 分支独立传 argv 不拼 shell 串。
#[cfg(target_os = "windows")]
pub(crate) fn spawn_in_terminal(
    argv: &[String],
    cwd: Option<&str>,
    terminal: &str,
    env: &[(String, String)],
) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let dir = safe_cwd(cwd);
    // 选了 wt/默认但没装 wt → 回退 PowerShell；选了 wezterm 但已卸载 → 落回 wt/powershell。
    let eff = match terminal {
        "powershell" => "powershell",
        "cmd" => "cmd",
        "wezterm" if wezterm::available() => "wezterm",
        _ if wt_available() => "wt",
        _ => "powershell",
    };
    let spawned: std::io::Result<()> = match eff {
        "powershell" => {
            let cmd = format!(
                "{}{}",
                env_prefix_powershell(env),
                shell_join_for_windows(argv, true)
            );
            let mut c = Command::new("powershell");
            c.args(["-NoExit", "-Command", &cmd]);
            if let Some(d) = &dir {
                c.current_dir(d);
            }
            c.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
        }
        "cmd" => {
            // cmd 没有能覆盖 %, !, ^, 嵌套引号等全部情况的字面 argv 语法。把真实 argv 放进
            // PowerShell EncodedCommand，cmd 只看到固定开关与 base64，避免用户路径/中转模型变成语法。
            let wrapped = wrap_with_env_windows(argv, env);
            let cmd = shell_join_for_windows(&wrapped, false);
            let mut c = Command::new("cmd");
            c.raw_arg("/k").raw_arg(cmd);
            if let Some(d) = &dir {
                c.current_dir(d);
            }
            c.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
        }
        // wezterm / wt 都不是我们的子进程（前者由 mux server 起、后者交给已存在的 wt 实例），
        // Command::env() 传不过去 → 有代理要注入时，改成让它们跑一层 PowerShell 来设变量。
        // 无代理时保持原样（直接跑 agent），把行为变更严格限制在用了代理的用户身上。
        "wezterm" => wezterm::resume(dir.as_deref(), &wrap_with_env_windows(argv, env)),
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
            args.extend(wrap_with_env_windows(argv, env));
            Command::new("wt").args(&args).spawn().map(|_| ())
        }
    };
    match spawned {
        Ok(()) => true,
        Err(e) => {
            eprintln!("打开终端 {eff} 失败：{e}");
            false
        }
    }
}

/// 给「不是我们子进程」的终端（wt / wezterm）用：把 argv 包进一层 PowerShell，好让代理环境变量
/// 能经命令串设进去。即使 `env` 为空也要包一层，以落实「直连」必须清掉继承代理的语义。
///
/// 必须使用 `-EncodedCommand`：Windows Terminal 会把普通 `-Command` 参数里的 `;` 重新解释成
/// 自己的多命令分隔符，于是八条清理环境变量语句会各开一个 tab，并把末尾 agent 路径的引号拆坏。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn wrap_with_env_windows(argv: &[String], env: &[(String, String)]) -> Vec<String> {
    use base64::Engine;

    let cmd = format!(
        "{}{}",
        env_prefix_powershell(env),
        shell_join_for_windows(argv, true)
    );
    let utf16le: Vec<u8> = cmd.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16le);
    vec![
        "powershell".into(),
        "-NoExit".into(),
        "-EncodedCommand".into(),
        encoded,
    ]
}

/// macOS 版：按 terminal 选 Terminal.app/iTerm2/Ghostty（iTerm2/Ghostty 未装回退 Terminal）。
/// Terminal/iTerm2 走 AppleScript；Ghostty 无 AppleScript，走 `open -na`。成功 true。
#[cfg(target_os = "macos")]
pub(crate) fn spawn_in_terminal(
    argv: &[String],
    cwd: Option<&str>,
    terminal: &str,
    env: &[(String, String)],
) -> bool {
    if terminal.eq_ignore_ascii_case("ghostty") && ghostty_installed() {
        // env/cwd 的处理次序与下方 AppleScript 路径一字不差：先建密钥注入文件，再验 cwd。
        let Ok(env_prefix) = env_source_prefix_posix(env) else {
            return false;
        };
        if !mac_resume_cwd_valid(cwd) {
            return false;
        }
        return resume_session_ghostty(cwd, argv, &env_prefix);
    }
    use crate::term_script::TermKind;
    let kind = match crate::term_script::resume_kind_from_setting(terminal) {
        TermKind::ITerm2 if iterm_installed() => TermKind::ITerm2,
        TermKind::ITerm2 => TermKind::Terminal,
        other => other,
    };
    // env 里可能带中转 API key：写进 0600 临时文件由终端命令 source，不再拼进可见命令行
    // （iTerm2 会把它写进 shell history，Terminal.app 留在滚动缓冲区）。建不出文件时宁可
    // 恢复失败（调用方回滚乐观复活），也不回退到把密钥敲进终端的旧形式。
    let Ok(env_prefix) = env_source_prefix_posix(env) else {
        return false;
    };
    // cwd 必须真实存在：目录已删时 AppleScript 里 cd 失败被 && 短路，resume 没跑却返回成功
    // （假恢复）。返回 false 由调用方回滚乐观复活。
    if !mac_resume_cwd_valid(cwd) {
        return false;
    }
    crate::macos::terminal::resume_session_mac(cwd, argv, kind, &env_prefix)
}

/// 其它平台无终端集成。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn spawn_in_terminal(
    _argv: &[String],
    _cwd: Option<&str>,
    _terminal: &str,
    _env: &[(String, String)],
) -> bool {
    false
}

/// 校验并归一「新建会话」的工作目录：非空、真实存在的目录。返回 trim 后的路径。
pub(crate) fn validate_new_session_cwd(cwd: &str) -> Result<String, String> {
    let d = cwd.trim();
    if d.is_empty() {
        return Err("请选择工作目录".into());
    }
    if !std::path::Path::new(d).is_dir() {
        return Err("目录不存在".into());
    }
    Ok(d.to_string())
}

/// 新建一个全新会话：由 Meowo PTY 裸启动指定 provider 的 CLI（无 session_id），并按
/// `session_open_in` 把用户带到对话窗口或 attach 后的外部终端。
/// 会话入库仍靠 CLI hook；SessionStart 后 reporter 用一次性 token 将临时 PTY 绑定到真实 session id。
/// `terminal` 仅为旧前端兼容参数：托管模式下用哪个外部终端由设置里的 `resume_terminal` 决定。
#[tauri::command]
// 参数形状即前端 invoke 的调用契约(与 get_live_sessions_page 同一豁免理由):
// cwd/options/workGroup/extraDirs 都是正交的启动维度,合并成 struct 破坏旧前端兼容。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn new_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    cwd: String,
    provider: String,
    terminal: Option<String>,
    options: Option<std::collections::HashMap<String, String>>,
    // 附加目录(跨仓同一需求 = 一个会话):每个以 agent 声明的 flag(claude --add-dir)
    // 进 argv;原件 claim 时落库,resume/接管重启回放。
    extra_dirs: Option<Vec<String>>,
) -> Result<(), String> {
    let dir = validate_new_session_cwd(&cwd)?;
    let agent = meowo_agent::resolve(Some(&provider)).ok_or("未知 agent")?;
    // 附加目录:逐个过与主目录同款的存在性校验(拼进 argv 的路径必须真实),并剔除
    // 与主目录重复的。声明了才支持——没声明的 agent 收到附加目录要如实拒绝,
    // 静默丢弃会让用户以为 agent 看得到那些仓。
    let extras: Vec<String> = {
        let mut out = Vec::new();
        for d in extra_dirs.unwrap_or_default() {
            let v = validate_new_session_cwd(&d)?;
            if v != dir && !out.contains(&v) {
                out.push(v);
            }
        }
        out
    };
    let extra_flag = agent.extra_dir_flag();
    if !extras.is_empty() && extra_flag.is_none() {
        return Err(format!("{} 不支持附加目录", agent.display_name()));
    }
    // 启动选项：前端只回传 choice id，此处按插件声明表翻译成 flag——未知 id 被忽略/落默认，
    // 用户输入永远进不了 argv。放在 relay 增补**之前**：中转声明的 `--model` 必须最后压轴
    // （中转端点只认它配置的那个模型，用户选的别名对它无意义，claude 以最后一个 --model 为准）。
    // 选择 map 单独留一份：随临时 PTY 暂存、claim 认领时写进 sessions.launch_args，
    // resume/接管重启进程时经声明表翻译回放（权限模式等是启动参数，不回放每次重启都
    // 重置成 CLI 默认）。存选择而非 flag：接管时改权限只需按选项维度合并。
    let selections = options.clone().unwrap_or_default();
    let mut argv = agent.launch_argv();
    argv.extend(meowo_agent::resolve_launch_args(
        agent.launch_options(),
        &selections,
    ));
    // 附加目录:每个一对 `<flag> <dir>`(路径已过存在性校验)。
    if let Some(flag) = extra_flag {
        for d in &extras {
            argv.push(flag.to_string());
            argv.push(d.clone());
        }
    }
    let argv = crate::relay::augment_argv(agent.id(), argv);
    // 代理 + 中转 **+ 当前活跃账号的隔离变量**（`CLAUDE_CONFIG_DIR` 等），三者都在
    // `launch_env_for_profile` 里。
    //
    // 这里曾经只注入代理（`proxy::launch_env`），于是多账号完全不生效：设置页明明切到了另一个
    // 账号，新开的会话却仍跑在默认账号上——而且毫无迹象，用户只能靠 `/status` 里的邮箱才发现。
    // 新建会话是**用户切换账号后最先走的一条路**，漏了它等于整个功能没做。
    let env = launch_env_for_profile(Some(&provider), None);
    // 参数为兼容旧前端保留；托管模式不再由这里预选外部终端，视图与终端类型分别由
    // session_open_in / resume_terminal 决定。
    let _ = terminal;
    let broker = state.ptys.clone();
    let reveal_broker = state.ptys.clone();
    let window_app = app.clone();
    // PTY 冷启动与杀软扫描可能阻塞数秒，放 blocking 池；首次 SessionStart hook 会把临时 PTY
    // 认领为真实数据库 session。
    let temp_id = tauri::async_runtime::spawn_blocking(move || {
        broker.start_pending(
            app,
            &argv,
            Some(&dir),
            &env,
            100,
            30,
            &provider,
            &selections,
            None,
            &extras,
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    // 用临时负 id 就能 attach：claim 只改注册表的键，subscriber 挂在 ManagedPty 上，
    // 认领前后都指着同一个 PTY，不会断流。
    tauri::async_runtime::spawn_blocking(move || {
        reveal_session(&window_app, &reveal_broker, temp_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 恢复一个已断开的会话：由 Meowo 持有 PTY，并打开同步对话窗口。外部终端若需要，
/// 再通过 attach 连接到同一 PTY；这样从卡片恢复的会话也能在 GUI 中直接发送消息与审批。
///
/// 恢复命令由 `provider` 决定（claude: `claude --resume <id>` / kimi: `kimi -r <id>`，见 agent::resume_args）。
/// 安全：`session_id` 经 is_safe_id 校验（仅 `[A-Za-z0-9_-]`，无空格/元字符）；可执行名与参数来自受信的
/// agent::resume_args（非用户输入）；wt 分支各 argv 独立传入，powershell/cmd 命令串只由这些受信片段拼成，从源头杜绝注入。
#[tauri::command]
pub(crate) async fn resume_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    cwd: Option<String>,
    session_id: String,
    provider: String,
) -> Result<(), String> {
    if !is_safe_id(&session_id) {
        return Err("无效 session_id".into());
    }
    ensure_session_profile_available(&provider, &session_id)?;
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let broker = state.ptys.clone();
        let db = state.db_path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let store = open_store(&db)?;
            let sid = store
                .find_session_id_pub(&session_id)
                .map_err(|e| e.to_string())?
                .ok_or("会话不存在")?;
            if session_agent_alive(&store, sid)? {
                return Err("会话仍在外部终端运行，请先在终端页选择接管".into());
            }
            start_managed_resume(app, broker, sid, cwd, session_id, provider)
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (app, cwd, provider);
        Err("当前平台不支持".into())
    }
}

/// 把仍在外部终端中的 Agent 安全迁移到 Meowo：先验证 PID 与恢复计划，再结束旧进程，
/// 最后以同一个 session id 在托管 PTY 中恢复。前端会在执行前明确二次确认。
#[tauri::command]
pub(crate) async fn takeover_managed_terminal(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session_id: i64,
    cols: u16,
    rows: u16,
    // 接管时可改启动选项（如把权限模式切成跳过）；None = 沿用会话存的选择。
    options: Option<std::collections::HashMap<String, String>>,
) -> Result<(), String> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let broker = state.ptys.clone();
        let db = state.db_path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let store = open_store(&db)?;
            let session = store.get_session(session_id).map_err(|e| e.to_string())?;
            let provider = store
                .session_provider(session_id)
                .map_err(|e| e.to_string())?;
            let cwd = store.session_cwd(session_id).map_err(|e| e.to_string())?;
            // takeover 与 resume/start 的守卫互为镜像：那两条要求进程**已死**，这条专治
            // 进程**还活着**——先确认恢复计划有效，再结束旧进程。判活口径必须一致（直接
            // 复用 session_agent_alive，含 pid 归属校验——换代残留的 pid 属于别的会话，
            // 杀不得），否则同一会话可能两条路都放行，对同一个 session id 起出第二个 agent。
            let pid = if session_agent_alive(&store, session_id)? {
                store.session_pid(session_id).map_err(|e| e.to_string())?
            } else {
                None
            };

            ensure_session_profile_available(&provider, &session.cc_session_id)?;
            let (_, resume) =
                resolve_resume_plan(&session.cc_session_id, cwd.as_deref(), &provider);
            if resume.is_empty() {
                return Err("该 Agent 不支持恢复会话".into());
            }
            // 先做一次可恢复副本再结束外部进程；结束后 start_managed_resume_sized 会再同步
            // 最终增量。这样目标目录不可写/源文件缺失时不会先把用户仍可用的会话杀掉。
            prepare_session_for_active_profile(&provider, &session.cc_session_id)?;
            if let Some(pid) = pid {
                terminate_agent_for_restart(pid)?;
            }
            start_managed_resume_sized(
                app,
                broker,
                session_id,
                cwd,
                session.cc_session_id,
                provider,
                crate::pty::TerminalSize::new(cols, rows),
                options,
            )
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (app, state, session_id, cols, rows, options);
        Err("当前平台不支持".into())
    }
}

/// 在用户选定的外部终端里起 attach 客户端，把托管 PTY 镜像过去。
/// Agent 与 PTY 都不迁移——外部窗口只是同一个 PTY 的第二个视图。
///
/// **两个平台都先查重**：该会话已有在线的外部视图（broker 订阅表非空）时不再开新窗口，
/// 把已有视图带到前台。没有这层，每点一次卡片就多一个镜像窗口/标签——默认设置
/// （session_open_in=terminal）下点连接中的卡片就是这条路，连点几下就冒出几个标签。
/// macOS 按订阅者 pid 反查宿主精确聚焦；Windows 拿不到 attach 客户端所在的具体标签，
/// 但能按进程组命中宿主顶层窗口（与 focus_session_terminal 的兜底同一套原语）——
/// 「窗口级置前」不如精确切标签，但远好于无限多开。
pub(crate) fn attach_in_external_terminal(
    broker: &crate::pty::PtyBroker,
    sid: i64,
) -> Result<(), String> {
    broker.ensure_attachable(sid)?;
    let terminal = load_settings().resume_terminal;
    #[cfg(target_os = "windows")]
    match broker.external_viewer(sid) {
        crate::pty::ExternalViewer::Pid(pid) => {
            // attach 客户端是控制台程序，宿主（WindowsTerminal/conhost/wezterm）是它的
            // 进程组祖先，窗口 pid 落在组内 → find_window_for_pids 可靠命中正确窗口。
            // 聚焦失败必须报错让用户看见（同 macOS 分支原则）：静默成功就是「点了没反应」，
            // 而静默新开则回到「连点冒标签」的老问题。
            let targets = console_group_pids(pid);
            if let Some(hwnd) = find_window_for_pids(&targets) {
                force_foreground(hwnd);
                return Ok(());
            }
            return Err("外部终端已在线，但没能带到前台，请手动切换".into());
        }
        // 旧 reporter 的订阅没有 pid：无从定位，维持原行为（新开一扇）。
        crate::pty::ExternalViewer::Legacy => {}
        // 无在线视图 → 落到下方正常新开一扇。
        crate::pty::ExternalViewer::None => {}
    }
    // 在线判定与激活目标一次取齐（见 ExternalViewer）：拆成两问会被「关窗口同时点卡片」
    // 的 detach 竞态穿插，把新 reporter 误判成旧 reporter、按设置激活错误应用。
    #[cfg(target_os = "macos")]
    match broker.external_viewer(sid) {
        crate::pty::ExternalViewer::Pid(pid) => {
            // 激活目标从订阅者 pid 反查实际宿主（精确 tab → 宿主级置前），绝不看「恢复
            // 终端」设置——设置与视图实际所在的应用可能不一致，按设置激活会跳错应用，
            // Ghostty 未运行时甚至凭空弹一扇空白窗。聚焦失败也不退回设置路径，但必须
            // 报错让用户看见（同 reveal_session 的原则）：静默返回成功就是「点了没反应」。
            return if crate::macos::terminal::focus_attach_viewer(pid as i64) {
                Ok(())
            } else {
                Err("外部终端已在线，但没能带到前台，请手动切换".into())
            };
        }
        // 旧 reporter 的订阅没有 pid，才退回应用级兜底。
        crate::pty::ExternalViewer::Legacy => {
            activate_resume_terminal_app(&terminal);
            return Ok(());
        }
        // 无在线视图 → 落到下方正常新开一扇。
        crate::pty::ExternalViewer::None => {}
    }
    let reporter = crate::setup::sibling_reporter().ok_or("找不到 meowo-reporter attach 客户端")?;
    // endpoint/token/protocol 不进 argv：attach 客户端自行读 discovery 文件
    //（与审批桥接同一来源，含 pid 判活），token 不暴露在进程参数里。
    let argv = vec![
        reporter,
        "attach".into(),
        "--session".into(),
        sid.to_string(),
    ];
    if spawn_in_terminal(&argv, None, &terminal, &[]) {
        Ok(())
    } else {
        Err("打开外部同步终端失败".into())
    }
}

/// 把设置里选定的终端应用带到前台（`open -a`，不带 `-n` 不会新起实例、也不传参）。
/// 镜像与 spawn 路径同一套「未装回退 Terminal」判定——激活目标要对准视图实际所在的应用。
/// best-effort：激活失败没有更好的补救，也不值得为此报错打断用户。
#[cfg(target_os = "macos")]
fn activate_resume_terminal_app(terminal: &str) {
    let app = if terminal.eq_ignore_ascii_case("ghostty") && ghostty_installed() {
        "Ghostty"
    } else if terminal.to_ascii_lowercase().contains("iterm") && iterm_installed() {
        "iTerm"
    } else {
        "Terminal"
    };
    let _ = std::process::Command::new("open").args(["-a", app]).spawn();
}

/// 把用户带到会话所在的视图，按 `session_open_in` 分发。
///
/// 两种取值下 agent 都由 Meowo 的 PTY 持有——差的只是拿什么界面看它：`chat` 用对话窗口，
/// `terminal` 用 attach 客户端把同一个 PTY 镜像进用户选的外部终端。故这里不做任何进程决策。
///
/// attach 失败**不**回退去开对话窗口：外部终端起不来是要让用户看见的错误，静默换成 GUI
/// 只会让人以为设置没生效。
pub(crate) fn reveal_session(
    app: &tauri::AppHandle,
    broker: &crate::pty::PtyBroker,
    sid: i64,
) -> Result<(), String> {
    // 对话功能关闭（轻量模式）时无视 session_open_in 强制走外部终端——此时 chat 不是
    // 合法落点，静默改道比开一个「不该存在」的窗口诚实。
    let settings = load_settings();
    if settings.session_open_in == "terminal" || !settings.chat_enabled {
        return attach_in_external_terminal(broker, sid);
    }
    // 同步等窗口创建结果：PTY 已经拉起、窗口却没开时，把错误交还调用方，
    // 而不是让前端误报成功（用户「点了没反应」会再点一次，重复起会话）。
    crate::window::open_chat_window_impl(app, sid, true)
}

/// 从看板卡片恢复：会话此刻还没有任何视图，故成功后按设置把用户带过去。
/// 100x30 只是首帧占位尺寸——对话窗口/attach 客户端挂上来后会立即按真实容器发 resize。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn start_managed_resume(
    app: tauri::AppHandle,
    broker: crate::pty::PtyBroker,
    sid: i64,
    cwd: Option<String>,
    session_id: String,
    provider: String,
) -> Result<(), String> {
    start_managed_resume_sized(
        app.clone(),
        broker.clone(),
        sid,
        cwd,
        session_id,
        provider,
        crate::pty::TerminalSize::new(100, 30),
        None,
    )?;
    reveal_session(&app, &broker, sid)
}

/// 恢复会话到托管 PTY 的**唯一**实现。刻意不开窗：从对话窗口内发起的恢复
/// （start_managed_terminal / takeover）窗口已经在了，再调 open_chat_window 会触发
/// chat-session-changed，把用户正在编辑的输入连同 history 一起重置。
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_managed_resume_sized(
    app: tauri::AppHandle,
    broker: crate::pty::PtyBroker,
    sid: i64,
    cwd: Option<String>,
    session_id: String,
    provider: String,
    terminal_size: crate::pty::TerminalSize,
    option_overrides: Option<std::collections::HashMap<String, String>>,
) -> Result<(), String> {
    ensure_session_profile_available(&provider, &session_id)?;
    let (resolved, mut resume) = resolve_resume_plan(&session_id, cwd.as_deref(), &provider);
    if resume.is_empty() {
        return Err("该 Agent 不支持恢复会话".into());
    }
    // 可判定的失败前置判定：CLI 被卸载 / 项目目录没了时，spawn 只会抛一句原始系统错误
    // （`os error 2` 之类），用户拿不到任何可执行的下一步。这两种失败都有明确修复动作，
    // 在做乐观复活/账号迁移等副作用之前就拦下（新建路径的 validate_new_session_cwd 同理，
    // 恢复路径此前漏了）。
    if let Some(plugin) = meowo_agent::resolve(Some(&provider)) {
        if !plugin.is_installed() {
            return Err(format!(
                "{} 未安装或已被卸载，无法恢复会话。请到 设置 → Agent 重新安装",
                plugin.display_name()
            ));
        }
    }
    if let Some(dir) = resolved.as_deref() {
        if !std::path::Path::new(dir).is_dir() {
            return Err(format!(
                "项目目录已不存在：{dir}。目录被移动或删除后无法在原位置恢复会话"
            ));
        }
    }
    // 回放会话的启动选项（权限模式等；接管时的改选合并写回），恢复后的进程与选择同参。
    splice_stored_launch_args(&mut resume, &provider, sid, option_overrides.as_ref());
    // 回放附加目录（--add-dir）：恢复的进程与首次启动同一份跨仓访问范围。
    splice_stored_extra_dirs(&mut resume, &provider, sid);
    // takeover 调用本函数前已经结束旧进程；普通 resume 的旧进程本就不在。此刻复制能拿到
    // 完整的最后一帧 transcript，也不会与 Claude 正在追加同一个文件发生竞争。
    let target_profile = prepare_session_for_active_profile(&provider, &session_id)?;
    let revived = prepare_resume(&app, &session_id);
    // 一律按 prepare 算出的**实际**账号取 env，不再按 agent 身份分支重新推导。
    // target_profile 已经涵盖三种情形：跨账号迁移成功 → 活跃账号；声明了迁移但找不到
    // transcript（用户自设 config-home / 被清理策略删掉）→ 回退该会话原账号；未声明
    // 迁移 → 原账号。重新推导反而会丢掉中间那种回退，给出一个会话资料并不在那儿的账号。
    let env = launch_env_for_resume_target(&provider, target_profile.as_deref());
    if let Err(error) = broker.start(
        app.clone(),
        sid,
        &resume,
        resolved.as_deref(),
        &env,
        terminal_size,
        &provider,
    ) {
        if let Some(id) = revived {
            rollback_failed_resume(id);
        }
        emit_board_changed(&app, "resume-failed");
        return Err(error);
    }
    // 托管 PTY 已接管：摘掉 bg 旁路条目。不摘的话快照分发仍能查到旁路的定格画面
    // （endOffset 是大数），会顶掉新 PTY 从 0 起的输出——前端把新输出全判成「已写过」
    // 丢弃，表现为恢复会话后终端打字无回显。try_state：测试环境没有 AppState 时跳过。
    {
        use tauri::Manager as _;
        if let Some(state) = app.try_state::<crate::AppState>() {
            state.bg_ptys.detach(sid);
        }
    }
    // 只有会跨账号迁移的 agent 需要把账号写回 DB——它的会话账号刚刚可能变了。
    // 其余 agent 的 target_profile 恒等于原账号，写回是纯粹的冗余 DB 写。
    if supports_cross_account_resume(Some(&provider)) {
        record_resumed_profile(&session_id, target_profile.as_deref());
    }
    // 秒退探测：CLI 拒绝启动时（典型：resume 一个正被另一进程占用的会话，claude 直接报错
    // 退出），spawn 本身是成功的，错误只打印在 PTY 里就死了——不在这截获，用户看到的只有
    // 「点了没反应」。
    //
    // 以 25ms 为粒度轮询到 1 秒，且**一见到输出就返回**：正常启动的 TUI 几十毫秒内就会
    // 吐首屏，此时进程显然没有秒退，再等下去纯粹是让用户干看着。此前固定 5×200ms 睡满，
    // 成功路径必然白等 1 秒——那是这条链路上最确定的一笔浪费。
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
        if let Some((code, tail)) = broker.exit_info(sid) {
            emit_board_changed(&app, "resume-failed");
            let code = code.map_or_else(|| "?".into(), |c| c.to_string());
            return Err(if tail.is_empty() {
                format!("Agent 启动后立即退出（退出码 {code}）")
            } else {
                format!("Agent 启动后立即退出（退出码 {code}）：{tail}")
            });
        }
        // 有输出 = 进程活着并已开始工作，没必要再守着看它会不会秒退。
        if broker.output_len(sid) > 0 {
            return Ok(());
        }
    }
    Ok(())
}

/// 会话的 agent 进程此刻是否真的还活着。
///
/// 刻意**不**复用 `session_connected`：那是**看板显示**语义，带 RESUME_GRACE_MS 宽限窗口，
/// 会把「刚乐观复活、进程尚未起来」的会话报成已连接。拿它当接管守卫会误拒用户，且它读的是
/// `session_query` 的缓存快照可能与实时进程表给出相反结论。守卫要的是进程事实，故实时查。
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn session_agent_alive(store: &meowo_store::Store, sid: i64) -> Result<bool, String> {
    let Some(pid) = store
        .session_pid(sid)
        .map_err(|e| e.to_string())?
        .filter(|&pid| pid > 0)
    else {
        return Ok(false);
    };
    if !pid_alive_agent_quick(pid) {
        return Ok(false);
    }
    // 进程活着还不够，pid 得仍归本会话：/clear 换代后旧行可能残留一个「活着但已被
    // 新会话认领」的 pid（end_session 清 pid 之前的存量数据）。拿它当「外部仍在运行」
    // 会误拒接管——而那个「外部终端」根本不存在；更糟的是 takeover 会照着它杀错新会话的进程。
    if store
        .pid_held_by_other_live(sid, pid)
        .map_err(|e| e.to_string())?
    {
        return Ok(false);
    }
    Ok(true)
}

/// Windows：`terminate_agent_for_restart` 用的已验证 agent 进程句柄。
/// 校验后立刻 OpenProcess 钉住进程身份：此后原进程退出、pid 被系统回收复用，
/// TerminateProcess 与退出等待都只作用于原进程对象（已退出则操作失败），绝不会落到
/// 复用该 pid 的无关进程上——闭合「DB 校验 pid 归属」与「kill」之间的 TOCTOU 窗口
/// （此前 sysinfo 先按快照复核进程名、kill 时再按 pid 重新 OpenProcess，两段之间可错杀）。
#[cfg(target_os = "windows")]
struct AgentProcessHandle(windows_sys::Win32::Foundation::HANDLE);

/// [`AgentProcessHandle::open_verified`] 的结局分类：区分开「进程已自然退出」（不算失败，
/// 与函数顶部判活同一语义）和「pid 被非 agent 进程复用」（白名单拦截，什么都不杀）。
#[cfg(target_os = "windows")]
enum AgentProcessOpen {
    Opened(AgentProcessHandle),
    Exited,
    NotAgent,
}

#[cfg(target_os = "windows")]
impl AgentProcessHandle {
    /// 打开 pid 并复核可执行名仍在 agent 白名单内。名字取自句柄钉住的进程对象本身
    /// （与 Toolhelp 快照同一套 is_agent_process 白名单，但不怕快照后 pid 复用）。
    fn open_verified(pid: i64) -> AgentProcessOpen {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_TERMINATE,
        };
        if !(1..=u32::MAX as i64).contains(&pid) {
            return AgentProcessOpen::Exited;
        }
        unsafe {
            let handle = OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid as u32,
            );
            // 打不开几乎总是「校验到 kill 的间隙内自然退出」；权限类失败由调用方
            // 再以判活复核兜底（见 terminate_agent_for_restart）。
            if handle.is_null() {
                return AgentProcessOpen::Exited;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) == 0 {
                CloseHandle(handle);
                return AgentProcessOpen::Exited;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            if !meowo_agent::is_agent_process(&path) {
                CloseHandle(handle);
                return AgentProcessOpen::NotAgent;
            }
            AgentProcessOpen::Opened(AgentProcessHandle(handle))
        }
    }

    fn terminate(&self) -> bool {
        unsafe { windows_sys::Win32::System::Threading::TerminateProcess(self.0, 1) != 0 }
    }

    /// 进程对象是否仍在运行（未 signaled）。句柄钉住身份，pid 复用不影响判断。
    fn alive(&self) -> bool {
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(self.0, 0)
                == windows_sys::Win32::Foundation::WAIT_TIMEOUT
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for AgentProcessHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn terminate_agent_for_restart(pid: i64) -> Result<(), String> {
    // 确认弹窗停留期间进程可能已经自然结束；此时无需报错，直接进入恢复流程。
    if !pid_alive_agent_quick(pid) {
        return Ok(());
    }
    // Windows：校验后立刻持句柄钉住进程身份（见 AgentProcessHandle），之后的 terminate 与
    // 退出等待全走句柄、不按 pid 重开。进程名白名单复核不收反升：改在句柄钉住的进程对象上
    // 取镜像路径复核（open_verified）。
    #[cfg(target_os = "windows")]
    let proc_handle = match AgentProcessHandle::open_verified(pid) {
        AgentProcessOpen::Opened(h) => h,
        // 校验到 kill 的间隙内自然退出：与顶部判活同一语义，不算失败。但判活仍报活
        // （权限不足等打不开句柄的情形）必须拦下——否则恢复会拉起第二个进程。
        AgentProcessOpen::Exited if !pid_alive_agent_quick(pid) => return Ok(()),
        AgentProcessOpen::Exited => return Err("无法结束原会话进程".into()),
        // pid 被非 agent 进程复用：白名单拦截，什么都不杀。
        AgentProcessOpen::NotAgent => return Err("无法结束原会话进程".into()),
    };
    #[cfg(target_os = "windows")]
    let kill = |force: bool| {
        // Windows 只有 TerminateProcess 一档（与旧 sysinfo kill 相同），无 TERM/KILL 之分。
        let _ = force;
        proc_handle.terminate()
    };
    #[cfg(target_os = "windows")]
    let alive = || proc_handle.alive();
    // macOS 上 sysinfo 的进程可见性不稳定（判活本来也走 ps），直接以独立 argv 发送信号，不经 shell。
    #[cfg(target_os = "macos")]
    let kill = |force: bool| {
        let pid = pid.to_string();
        std::process::Command::new("kill")
            .args([if force { "-KILL" } else { "-TERM" }, pid.as_str()])
            .status()
            .is_ok_and(|s| s.success())
    };
    #[cfg(target_os = "macos")]
    let alive = || pid_alive_agent_quick(pid);

    if !kill(false) && alive() {
        return Err("无法结束原会话进程".into());
    }

    // 给 Agent 的退出清理/SessionEnd hook 留出时间；若仍存活再强制结束，避免恢复出双进程。
    for _ in 0..30 {
        if !alive() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if alive() {
        if !kill(true) && alive() {
            return Err("原会话仍在运行，未重新打开".into());
        }
        for _ in 0..20 {
            if !alive() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if alive() {
            return Err("原会话仍在运行，未重新打开".into());
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    Ok(())
}

/// 用户确认后结束“仍存活但无法定位终端”的原 Agent，并在设置中的受支持终端恢复同一会话。
/// PID 必须仍属于该 session，且进程名仍是受支持 Agent；两层校验避免 PID 复用或过期 UI 误杀进程。
#[tauri::command]
pub(crate) async fn restart_session_supported(
    app: tauri::AppHandle,
    pid: i64,
    cwd: Option<String>,
    session_id: String,
    provider: String,
) -> Result<(), String> {
    if pid <= 0 || !is_safe_id(&session_id) {
        return Err("无效会话".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let store = open_store(&db_path()).map_err(|e| e.to_string())?;
        let sid = store
            .find_session_id_pub(&session_id)
            .map_err(|e| e.to_string())?
            .ok_or("会话不存在")?;
        if store.session_pid(sid).map_err(|e| e.to_string())? != Some(pid) {
            return Err("会话进程已变化，请刷新后重试".into());
        }

        // 先验证完整恢复计划，再动原进程；未知 provider/无恢复能力时必须保持原会话原样。
        let (resolved, resume) = resolve_resume_plan(&session_id, cwd.as_deref(), &provider);
        if resume.is_empty() {
            return Err("该 Agent 不支持恢复会话".into());
        }
        // 与托管接管一致：跨账号副本先落稳，再结束仍可用的外部进程。
        prepare_session_for_active_profile(&provider, &session_id)?;
        // 账号目录可能已被删除。必须在终止仍可用的原进程之前拦住，否则恢复失败还会顺手杀掉会话。
        ensure_session_profile_available(&provider, &session_id)?;
        terminate_agent_for_restart(pid)?;

        // 原进程确认结束后才复活 DB 状态；恢复计划沿用终止前已验证的结果。
        let target_profile = prepare_session_for_active_profile(&provider, &session_id)?;
        let revived = prepare_resume(&app, &session_id);
        // 与托管恢复同一口径：按 prepare 算出的实际账号取 env（理由见 start_managed_resume_sized）。
        let env = launch_env_for_resume_target(&provider, target_profile.as_deref());
        let ok = spawn_in_terminal(
            &resume,
            resolved.as_deref(),
            &load_settings().resume_terminal,
            &env,
        );
        if ok {
            if supports_cross_account_resume(Some(&provider)) {
                record_resumed_profile(&session_id, target_profile.as_deref());
            }
            Ok(())
        } else {
            if let Some(id) = revived {
                rollback_failed_resume(id);
            }
            emit_board_changed(&app, "restart-failed");
            Err("启动受支持的终端失败".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 观测校正的推断规则：升 1M / 降 200K / 换家族都要对，证据不足与 opusplan 恒不动。
#[cfg(test)]
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod corrected_model_choice_tests {
    use super::corrected_model_choice;

    #[test]
    fn upgrades_to_1m_when_observed_window_is_large() {
        assert_eq!(
            corrected_model_choice("fable", Some("Fable 5"), Some(1_000_000)).as_deref(),
            Some("fable[1m]")
        );
    }

    #[test]
    fn downgrades_when_observed_window_is_standard() {
        assert_eq!(
            corrected_model_choice("fable[1m]", Some("Fable 5"), Some(200_000)).as_deref(),
            Some("fable")
        );
    }

    #[test]
    fn switches_family_from_display_name() {
        assert_eq!(
            corrected_model_choice("opus", Some("Sonnet 5"), Some(1_000_000)).as_deref(),
            Some("sonnet[1m]")
        );
    }

    #[test]
    fn keeps_stored_suffix_without_window_evidence() {
        assert_eq!(
            corrected_model_choice("fable[1m]", Some("Fable 5"), None).as_deref(),
            Some("fable[1m]")
        );
        assert_eq!(
            corrected_model_choice("fable", None, None).as_deref(),
            Some("fable")
        );
    }

    #[test]
    fn haiku_never_gets_a_1m_variant() {
        assert_eq!(
            corrected_model_choice("haiku", Some("Haiku 4.5"), Some(1_000_000)).as_deref(),
            Some("haiku")
        );
    }

    #[test]
    fn opusplan_and_unknown_choices_are_left_alone() {
        assert_eq!(corrected_model_choice("opusplan", Some("Opus 5"), Some(1_000_000)), None);
        assert_eq!(corrected_model_choice("default", Some("Opus 5"), Some(1_000_000)), None);
        assert_eq!(corrected_model_choice("custom-x", Some("Opus 5"), Some(1_000_000)), None);
    }
}

#[cfg(test)]
mod proxy_env_tests {
    use super::*;

    fn decode_wrapped_command(wrapped: &[String]) -> String {
        use base64::Engine;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&wrapped[3])
            .unwrap();
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&utf16).unwrap()
    }

    fn env(v: &str) -> Vec<(String, String)> {
        vec![
            ("HTTPS_PROXY".into(), v.into()),
            ("HTTP_PROXY".into(), v.into()),
        ]
    }

    #[test]
    fn powershell_prefix_sets_vars_before_the_command() {
        let p = env_prefix_powershell(&env("http://127.0.0.1:7890"));
        assert!(p.starts_with("$env:HTTPS_PROXY=$null; "));
        assert!(p.ends_with(
            "$env:HTTPS_PROXY='http://127.0.0.1:7890'; $env:HTTP_PROXY='http://127.0.0.1:7890'; "
        ));
        // 与命令串拼起来必须是「先赋值、再执行」。
        let cmd = format!(
            "{p}{}",
            shell_join_for_windows(&["claude".to_string()], true)
        );
        assert!(cmd.starts_with("$env:HTTPS_PROXY="));
        assert!(cmd.ends_with("claude"));
    }

    #[test]
    fn posix_prefix_leaves_the_name_unquoted() {
        // 关键：**键名不能带引号**——`'K=v' cmd` 会被 shell 当成一个命令名而不是赋值。
        // 值则必须单引号包裹（AppleScript 会把它原样拼进 shell 命令串）。
        let p = env_prefix_posix(&env("http://127.0.0.1:7890"));
        assert!(p.starts_with("unset HTTPS_PROXY HTTP_PROXY ALL_PROXY NO_PROXY "));
        assert!(
            p.ends_with("HTTPS_PROXY='http://127.0.0.1:7890' HTTP_PROXY='http://127.0.0.1:7890' ")
        );
        assert!(!p.starts_with('\''), "键名不得被引起来");
    }

    /// 恢复会话的 env 带着中转 API key：赋值必须写进 0600 临时文件，可见命令行只剩
    /// `source <tmp> && rm -f <tmp> &&`——否则 iTerm2 把它写进 ~/.zsh_history、
    /// Terminal.app 留在滚动缓冲区，都是明文落盘。
    #[test]
    fn posix_env_moves_secrets_into_a_sourced_file() {
        let secret_env = vec![
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant-secret".to_string()),
            (
                "HTTPS_PROXY".to_string(),
                "http://127.0.0.1:7890".to_string(),
            ),
        ];
        let prefix = env_source_prefix_posix(&secret_env).unwrap();
        // 可见命令行只有 source/rm 与文件路径——密钥值绝不能出现在其中。
        assert!(prefix.starts_with("source '"));
        assert!(prefix.contains("' && rm -f '"));
        assert!(prefix.ends_with("' && "));
        assert!(!prefix.contains("sk-ant-secret"), "prefix={prefix}");
        assert!(!prefix.contains("7890"), "prefix={prefix}");
        // source 与 rm 指向同一文件；文件内容含 unset 清理与 export 赋值。
        let path = prefix
            .trim_start_matches("source '")
            .split('\'')
            .next()
            .unwrap();
        assert!(prefix.contains(&format!("rm -f '{path}'")));
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.starts_with("unset HTTPS_PROXY HTTP_PROXY ALL_PROXY NO_PROXY "));
        assert!(content.contains("export ANTHROPIC_API_KEY='sk-ant-secret'\n"));
        assert!(content.contains("export HTTPS_PROXY='http://127.0.0.1:7890'\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600,
                "密钥文件必须创建即 0600"
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    /// env 文件与内联前缀同一套转义纪律：值里的单引号闭合不了，注入留在字符串里。
    #[test]
    fn posix_env_file_escapes_quotes_in_values() {
        let evil = vec![("K".to_string(), "a'b;calc".to_string())];
        let prefix = env_source_prefix_posix(&evil).unwrap();
        let path = prefix
            .trim_start_matches("source '")
            .split('\'')
            .next()
            .unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains(r"export K='a'\''b;calc'"));
        std::fs::remove_file(path).unwrap();
    }

    /// 代理串是用户输入，会被拼进 shell 命令串——三种 shell 的转义都必须挡住「闭合引号后接命令」
    /// 的注入。值虽已过 validate（无空格、协议白名单），这里仍按「一律正确转义」把关，不赌。
    #[test]
    fn quoting_survives_a_value_with_quotes() {
        let evil = vec![("HTTPS_PROXY".to_string(), "http://a'b;calc".to_string())];

        // PowerShell：单引号翻倍 → 引号无法闭合，`;calc` 留在字符串里。
        let ps = env_prefix_powershell(&evil);
        assert!(ps.ends_with("$env:HTTPS_PROXY='http://a''b;calc'; "));

        // POSIX：`'` → `'\''`，同样无法闭合。
        let sh = env_prefix_posix(&evil);
        assert!(sh.ends_with(r"HTTPS_PROXY='http://a'\''b;calc' "));
    }

    #[test]
    fn empty_env_clears_inherited_proxy_before_launching() {
        // off 的意义是直连，故空 env 也必须清掉继承的代理，而不是原样启动。
        assert!(env_prefix_powershell(&[]).contains("$env:ALL_PROXY=$null;"));
        assert!(env_prefix_posix(&[]).starts_with("unset HTTPS_PROXY"));
        let argv = vec![
            "claude".to_string(),
            "--resume".to_string(),
            "abc".to_string(),
        ];
        let wrapped = wrap_with_env_windows(&argv, &[]);
        assert_eq!(wrapped[0], "powershell");
        assert_eq!(wrapped[2], "-EncodedCommand");
        assert!(
            !wrapped[3].contains(';'),
            "WT 可见的参数里不能再出现命令分隔符"
        );
        assert!(decode_wrapped_command(&wrapped).contains("$env:ALL_PROXY=$null;"));
    }

    /// wt / wezterm 不是我们的子进程（前者交给已存在的 wt 实例、后者交给 mux server），
    /// Command::env() 传不过去 → 必须包一层 PowerShell 把赋值写进命令串。
    #[test]
    fn wt_and_wezterm_get_wrapped_in_a_shell_when_proxied() {
        let argv = vec![
            "C:/x/codex.exe".to_string(),
            "resume".to_string(),
            "sid".to_string(),
        ];
        let w = wrap_with_env_windows(&argv, &env("http://127.0.0.1:7890"));
        assert_eq!(w[0], "powershell");
        assert_eq!(w[1], "-NoExit");
        assert_eq!(w[2], "-EncodedCommand");
        assert!(!w[3].contains(';'), "WT 不得把脚本拆成多个 tab：{}", w[3]);
        let decoded = decode_wrapped_command(&w);
        assert!(decoded.starts_with("$env:HTTPS_PROXY=$null; "));
        assert!(decoded.contains("$env:HTTPS_PROXY='http://127.0.0.1:7890'; "));
        assert!(decoded.contains("codex.exe"), "原命令必须还在：{decoded}");
        assert!(decoded.ends_with("resume sid"));
    }

    /// macOS resume 的 cwd 准入：None/空白合法（走无目录脚本）；给了目录就必须真实存在，
    /// 否则 AppleScript 里 cd 失败被 && 短路，resume 没跑却返回成功（假恢复）。
    #[test]
    fn mac_resume_cwd_valid_requires_existing_dir() {
        assert!(mac_resume_cwd_valid(None));
        assert!(mac_resume_cwd_valid(Some("")));
        assert!(mac_resume_cwd_valid(Some("   ")));
        let dir = std::env::temp_dir();
        assert!(mac_resume_cwd_valid(dir.to_str()));
        assert!(!mac_resume_cwd_valid(Some(
            "C:/definitely-not-exist/meowo-xyz-123"
        )));
        // 文件不是目录，同样拒收。
        let file = dir.join("meowo-cwd-valid-test-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(!mac_resume_cwd_valid(file.to_str()));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn ghostty_shell_command_quotes_everything() {
        let argv = vec![
            "claude".to_string(),
            "--resume".to_string(),
            "id'123".to_string(),
        ];
        let cmd =
            ghostty_shell_command(Some("/tmp/a b/c'd"), &argv, "source '/tmp/e' && ").expect("cmd");
        assert_eq!(
            cmd,
            "cd '/tmp/a b/c'\\''d' && source '/tmp/e' && 'claude' '--resume' 'id'\\''123'"
        );
    }

    #[test]
    fn ghostty_shell_command_handles_cwdless_and_empty_argv() {
        let argv = vec!["codex".to_string(), "resume".to_string(), "sid".to_string()];
        let cmd = ghostty_shell_command(None, &argv, "source '/tmp/e' && ").expect("cmd");
        assert_eq!(cmd, "source '/tmp/e' && 'codex' 'resume' 'sid'");
        assert!(ghostty_shell_command(None, &[], "source '/tmp/e' && ").is_none());
    }

    /// open_verified 的白名单复核：测试进程自身不是 agent，必须被拦下（NotAgent）——
    /// 这正是 pid 复用场景的防线：句柄钉住后按镜像路径复核，不杀错进程。
    #[cfg(target_os = "windows")]
    #[test]
    fn open_verified_rejects_the_non_agent_test_process() {
        match AgentProcessHandle::open_verified(std::process::id() as i64) {
            AgentProcessOpen::NotAgent => {}
            AgentProcessOpen::Opened(_) => panic!("非 agent 进程不得通过白名单复核"),
            AgentProcessOpen::Exited => panic!("当前进程明明活着，不该判成 Exited"),
        }
    }

    /// 打不开句柄的 pid 一律按 Exited 归类（调用方再以判活复核区分自然退出与权限问题）。
    #[cfg(target_os = "windows")]
    #[test]
    fn open_verified_reports_exited_for_unopenable_pids() {
        for pid in [0, -1, i64::MAX, 0x0FFF_FFFF] {
            assert!(
                matches!(
                    AgentProcessHandle::open_verified(pid),
                    AgentProcessOpen::Exited
                ),
                "pid={pid} 应判为 Exited"
            );
        }
    }
}
