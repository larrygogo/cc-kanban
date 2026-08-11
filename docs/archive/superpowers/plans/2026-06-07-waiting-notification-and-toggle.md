# 待交互通知 + 通知总开关 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 会话进入「待交互」时弹一条去重桌面通知，并新增一个统一控制所有桌面通知（待交互 + 错误）的总开关，默认开启。

**Architecture:** 复用已有的 5s 轮询 `spawn_liveness_watch` 与错误通知管线。新增 `notifications_enabled` 设置项（默认 ON、向后兼容），轮询每轮读取它门控所有 `.show()`；新增 `notified_waiting` 去重 map + 纯函数 `waiting_fingerprint`（错误优先），待交互指纹用会话 `last_event_at`。不改 DB schema、不加 hook。

**Tech Stack:** Rust（serde、tauri v2 + tauri-plugin-notification）、React 18 + TypeScript（vitest）。

---

## 文件结构

- `app/src-tauri/src/lib.rs`（改）：`Settings` 加 `notifications_enabled`（默认 ON）+ 手动 `Default`；新增纯函数 `waiting_fingerprint`；重写 `spawn_liveness_watch` 的会话扫描块（错误 + 待交互双通知，受总开关门控）；测试模块加用例。
- `app/src/api.ts`（改）：`Settings` 类型加 `notifications_enabled`。
- `app/src/views/About.tsx`（改）：`GeneralSection` 加「桌面通知」开关行，并让 `setSettings` 调用始终发送完整 `Settings`。
- `README.md`（改）：桌面通知特性说明。

---

## Task 1: Settings 增 `notifications_enabled`（默认 ON + 向后兼容）

**Files:**
- Modify: `app/src-tauri/src/lib.rs:752-758`（`Settings` 结构）
- Test: `app/src-tauri/src/lib.rs`（`#[cfg(test)] mod tests`）

- [ ] **Step 1: 写失败测试**

在 `app/src-tauri/src/lib.rs` 底部 `#[cfg(test)] mod tests` 块里，给现有的 `use super::{...}` 追加 `Settings`，并新增测试：

```rust
    #[test]
    fn settings_defaults_notifications_on() {
        // 空文件 / 老文件缺字段 → 默认开启（向后兼容）
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert!(empty.notifications_enabled);
        let legacy: Settings = serde_json::from_str(r#"{"archive_hide_days":7}"#).unwrap();
        assert!(legacy.notifications_enabled);
        assert_eq!(legacy.archive_hide_days, 7);
        // 显式关闭可被尊重
        let off: Settings = serde_json::from_str(r#"{"notifications_enabled":false}"#).unwrap();
        assert!(!off.notifications_enabled);
        // 整文件缺失/解析失败时用 Default，也应为 ON
        assert!(Settings::default().notifications_enabled);
    }
```

`serde_json` 已是依赖，测试模块需能用——若 `mod tests` 顶部没有 `use serde_json;`，无需加（用全路径 `serde_json::from_str` 即可，crate 已在依赖中）。

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p meowo-app settings_defaults_notifications_on`
Expected: 编译失败 —— `Settings` 无 `notifications_enabled` 字段。

- [ ] **Step 3: 改 `Settings` 结构 + 手动 Default**

把 `app/src-tauri/src/lib.rs` 的 `Settings` 定义（当前）：

```rust
/// 应用设置（持久化到 ~/.meowo/settings.json）。
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct Settings {
    /// 归档条目自动隐藏的天数；0 = 永不隐藏。
    #[serde(default)]
    archive_hide_days: u32,
}
```

改为（去掉 `Default` derive，加字段 + 字段级默认 + 手动 Default）：

```rust
fn default_true() -> bool {
    true
}

/// 应用设置（持久化到 ~/.meowo/settings.json）。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Settings {
    /// 归档条目自动隐藏的天数；0 = 永不隐藏。
    #[serde(default)]
    archive_hide_days: u32,
    /// 桌面通知总开关（待交互 + 错误）。缺省为开启，兼容老 settings.json。
    #[serde(default = "default_true")]
    notifications_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { archive_hide_days: 0, notifications_enabled: true }
    }
}
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p meowo-app settings_defaults_notifications_on`
Expected: PASS。

- [ ] **Step 5: 全 app 测试 + clippy**

Run: `cargo test -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: PASS，无警告。

- [ ] **Step 6: 提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): Settings 增 notifications_enabled 总开关（默认开启）"
```

---

## Task 2: 待交互通知 + 总开关门控（轮询逻辑）

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（新增 `waiting_fingerprint`；重写 `spawn_liveness_watch` 扫描块）
- Test: `app/src-tauri/src/lib.rs`（`#[cfg(test)] mod tests`）

- [ ] **Step 1: 写 `waiting_fingerprint` 失败测试**

在 `#[cfg(test)] mod tests` 的 `use super::{...}` 追加 `waiting_fingerprint`，并新增测试：

```rust
    #[test]
    fn waiting_fingerprint_rules() {
        // 连接中、待交互、未出错 → 用 last_event_at 作指纹
        assert_eq!(waiting_fingerprint(false, "waiting", 123), Some("123".to_string()));
        // 出错优先：errored 时不发待交互
        assert_eq!(waiting_fingerprint(true, "waiting", 123), None);
        // 非 waiting 状态不发
        assert_eq!(waiting_fingerprint(false, "running", 123), None);
        assert_eq!(waiting_fingerprint(false, "ended", 123), None);
        // 指纹随 last_event_at 变化（新的等待回合 → 新指纹 → 会再弹一次）
        assert_ne!(
            waiting_fingerprint(false, "waiting", 1),
            waiting_fingerprint(false, "waiting", 2)
        );
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p meowo-app waiting_fingerprint_rules`
Expected: 编译失败 —— `waiting_fingerprint` 未定义。

- [ ] **Step 3: 实现 `waiting_fingerprint`**

在 `app/src-tauri/src/lib.rs` 的 `should_notify` 函数正下方加入：

```rust
/// 待交互通知指纹：errored 时不发（None，错误优先）；status==waiting 且未出错时用
/// last_event_at 作指纹（每个新的等待回合是新指纹）；其它状态返回 None。纯函数，便于单测。
fn waiting_fingerprint(errored: bool, status: &str, last_event_at: i64) -> Option<String> {
    if errored || status != "waiting" {
        None
    } else {
        Some(last_event_at.to_string())
    }
}
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p meowo-app waiting_fingerprint_rules`
Expected: PASS。

- [ ] **Step 5: 重写 `spawn_liveness_watch` 加入待交互通知与总开关门控**

把整个 `spawn_liveness_watch` 函数（当前 `app/src-tauri/src/lib.rs:902-963`）替换为：

```rust
/// 周期轮询：收尾进程已死的卡住会话；存活集合变化或有收尾时发 board-changed 让前端刷新。
/// 同时对「连接中」会话做去重桌面通知：出错（优先）或进入待交互时各弹一次。
/// 总开关（settings.notifications_enabled）只门控是否 .show()，去重 map 始终更新，
/// 故中途打开开关不会把积压的旧错误/待交互一次性炸出来。启动首扫只播种不弹。
fn spawn_liveness_watch(app: tauri::AppHandle, db_path: PathBuf) {
    use std::collections::HashMap;
    use tauri_plugin_notification::NotificationExt;
    std::thread::spawn(move || {
        let mut last: Vec<i64> = Vec::new();
        let mut notified: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次错误指纹
        let mut notified_waiting: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次待交互指纹
        let mut seeded = false;
        loop {
            if let Ok(store) = Store::open(&db_path) {
                let sys = System::new_with_specifics(
                    RefreshKind::new().with_processes(ProcessRefreshKind::new()),
                );
                let orphaned = store.end_orphaned_idle(ORPHAN_IDLE_MS, now_ms()).unwrap_or(0);
                let (alive, reaped) = reap_and_alive_ids(&store, &sys, now_ms());
                if alive != last || reaped > 0 || orphaned > 0 {
                    let _ = app.emit("board-changed", ());
                    last = alive;
                }

                // 通知总开关：每轮读一次（文件读极廉价；设置改动 5s 内生效）。
                let notify_on = load_settings().notifications_enabled;

                // 错误 + 待交互通知：仅扫连接中的会话（活跃，数量少）。
                let mut present: HashMap<String, String> = HashMap::new();
                for s in store.live_sessions().unwrap_or_default() {
                    if s.session.status == "ended" || !pid_is_claude(&sys, s.pid.unwrap_or(0)) {
                        continue;
                    }
                    let sid = s.session.cc_session_id.clone();
                    present.insert(sid.clone(), String::new()); // 标记本轮已扫描；retain 只清理本轮彻底消失的会话

                    let meowo_store::TranscriptInfo { title, error } =
                        meowo_store::title::resolve_transcript_path(None, s.cwd.as_deref(), &sid)
                            .and_then(|p| p.to_str().map(meowo_store::analyze_transcript))
                            .unwrap_or_default();

                    // 错误通知（优先）。
                    if let Some(e) = &error {
                        let prev = notified.get(&sid).map(|s| s.as_str());
                        if seeded && notify_on && should_notify(prev, Some(&e.fingerprint)) {
                            let _ = app
                                .notification()
                                .builder()
                                .title("会话出错")
                                .body(format!("{} · {}", s.project_name, e.label))
                                .show();
                        }
                        notified.insert(sid.clone(), e.fingerprint.clone());
                    } else {
                        notified.remove(&sid); // 错误消失：下次再错会重新通知
                    }

                    // 待交互通知（errored 时 waiting_fingerprint 返回 None，自动让位给错误）。
                    match waiting_fingerprint(error.is_some(), &s.session.status, s.session.last_event_at) {
                        Some(fp) => {
                            let prev = notified_waiting.get(&sid).map(|s| s.as_str());
                            if seeded && notify_on && should_notify(prev, Some(&fp)) {
                                let body_title = title
                                    .filter(|t| !t.trim().is_empty())
                                    .unwrap_or_else(|| s.task_title.clone());
                                let _ = app
                                    .notification()
                                    .builder()
                                    .title("等待你回复")
                                    .body(format!("{} · {}", s.project_name, body_title))
                                    .show();
                            }
                            notified_waiting.insert(sid.clone(), fp);
                        }
                        None => {
                            notified_waiting.remove(&sid);
                        }
                    }
                }
                // 清掉本轮彻底消失（已结束/超出 100 条上限）的残留条目，防止 map 无限增长。
                // 边缘情况：会话彻底消失后又带着完全相同的未解决错误/待交互重新出现，会再弹一次——可接受。
                notified.retain(|k, _| present.contains_key(k));
                notified_waiting.retain(|k, _| present.contains_key(k));
                seeded = true;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}
```

- [ ] **Step 6: 编译 + 全 app 测试 + clippy**

Run: `cargo test -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: PASS，无警告。

- [ ] **Step 7: 提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): 会话待交互时去重桌面通知 + 总开关门控全部通知"
```

---

## Task 3: 前端总开关（类型 + 设置页）

**Files:**
- Modify: `app/src/api.ts:85-88`（`Settings` 类型）
- Modify: `app/src/views/About.tsx`（`GeneralSection`）

- [ ] **Step 1: 扩展 `Settings` 类型**

`app/src/api.ts` 的 `Settings` 类型（当前）：

```ts
export type Settings = {
  /** 归档条目自动隐藏的天数；0 = 永不隐藏。 */
  archive_hide_days: number;
};
```

改为：

```ts
export type Settings = {
  /** 归档条目自动隐藏的天数；0 = 永不隐藏。 */
  archive_hide_days: number;
  /** 桌面通知总开关（待交互 + 错误）。 */
  notifications_enabled: boolean;
};
```

- [ ] **Step 2: 在 `GeneralSection` 加开关行并发送完整 Settings**

`app/src/views/About.tsx` 的 `GeneralSection` 函数（当前 116-155 行）整体替换为：

```tsx
function GeneralSection() {
  const [autostart, setAutostart] = useState(false);
  const [hideDays, setHideDays] = useState(0);
  const [notifyOn, setNotifyOn] = useState(true);
  useEffect(() => {
    invoke<boolean>("get_autostart").then(setAutostart).catch(() => {});
    getSettings()
      .then((s) => {
        setHideDays(s.archive_hide_days);
        setNotifyOn(s.notifications_enabled);
      })
      .catch(() => {});
  }, []);
  const toggleAutostart = () => {
    const next = !autostart;
    setAutostart(next);
    invoke("set_autostart", { enabled: next }).catch(() => setAutostart(!next));
  };
  // 设置项写库统一发送完整 Settings（后端 set_settings 接收整个对象）。
  const persist = (next: { archive_hide_days: number; notifications_enabled: boolean }) =>
    setSettings(next);
  const changeHideDays = (days: number) => {
    const prev = hideDays;
    setHideDays(days);
    persist({ archive_hide_days: days, notifications_enabled: notifyOn }).catch(() => setHideDays(prev));
  };
  const toggleNotify = () => {
    const next = !notifyOn;
    setNotifyOn(next);
    persist({ archive_hide_days: hideDays, notifications_enabled: next }).catch(() => setNotifyOn(!next));
  };
  return (
    <>
      <div className="sec-title">通用</div>
      <div className="row-card">
        <div className="row">
          <div className="row-text">
            <div className="row-label">开机自启</div>
            <div className="row-desc">登录系统后自动启动 Meowo</div>
          </div>
          <Switch checked={autostart} onChange={toggleAutostart} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">桌面通知</div>
            <div className="row-desc">会话需要你回复或出错时弹系统通知</div>
          </div>
          <Switch checked={notifyOn} onChange={toggleNotify} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">归档自动隐藏</div>
            <div className="row-desc">归档超过所选时长后，自动从「已归档」中隐藏</div>
          </div>
          <Dropdown value={hideDays} options={HIDE_OPTIONS} onChange={changeHideDays} />
        </div>
      </div>
      <div className="sec-hint">更多设置项陆续补充中…</div>
    </>
  );
}
```

- [ ] **Step 3: 类型检查 + 前端测试**

Run（从 `app/` 目录）: `cd app && bunx tsc --noEmit && bunx vitest run`
Expected: 无类型错误；现有测试全部通过（无回归）。

> 说明：若 `Sticker.test.tsx` / `LiveView.test.tsx` 等里有用到 `Settings` 字面量（如 mock `getSettings`），缺 `notifications_enabled` 会导致 tsc 报错——若报错，给那些字面量补 `notifications_enabled: true`。先跑 tsc 看是否需要。

- [ ] **Step 4: 提交**

```bash
git add app/src/api.ts app/src/views/About.tsx
git commit -m "feat(app): 设置页新增桌面通知总开关"
```

---

## Task 4: 整体验证 + 文档

**Files:**
- Modify: `README.md`（特性列表）

- [ ] **Step 1: 全量测试 + lint**

Run（仓库根）:

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Expected: PASS，无警告。

Run（前端）:

```bash
cd app && bunx tsc --noEmit && bunx vitest run
```

Expected: PASS。

- [ ] **Step 2: 更新 README 桌面通知特性**

`README.md` 的「特性」列表里，把现有这条（错误提醒）：

```markdown
- **错误提醒**：会话因工具调用解析失败 / 需要重新登录 / 认证失败而卡死时，卡片转红并归入「待交互」，同时弹一条去重的桌面通知（同一错误只弹一次）。
```

改为两条（错误状态 + 统一桌面通知）：

```markdown
- **错误提醒**：会话因工具调用解析失败 / 需要重新登录 / 认证失败而卡死时，卡片转红并归入「待交互」。
- **桌面通知**：会话需要你回复（待交互）或出错时弹一条去重的系统通知（同一情形只弹一次）；可在设置里用总开关统一开关，默认开启。
```

- [ ] **Step 3: 手动冒烟（可选，需真实环境）**

Run: `cd app && bun run tauri dev`
验证：开关默认开 → 某会话停下等输入时弹「等待你回复」；同一等待回合不重复弹；回复后再次等待会再弹一次；出错的会话弹「会话出错」而非待交互；设置里关掉总开关后不再弹（约 5s 内生效）。

- [ ] **Step 4: 提交**

```bash
git add README.md
git commit -m "docs: README 补桌面通知（待交互 + 总开关）特性"
```

---

## 自查记录

- **Spec 覆盖**：总开关字段默认 ON + 向后兼容 → Task 1；待交互通知 + 错误优先 + 去重 + 立即弹 → Task 2（`waiting_fingerprint` + 循环重写）；总开关门控全部 `.show()`、OFF 仍更新 map、首扫播种 → Task 2；前端开关行 + 完整 Settings 写入 → Task 3；测试计划（Settings 默认、waiting_fingerprint、复用 should_notify）→ Task 1/2；README → Task 4。✅
- **占位符**：无 TBD/TODO，所有步骤含完整代码与命令。✅
- **类型一致**：`Settings{archive_hide_days,notifications_enabled}`（Rust & TS 一致）、`default_true`、`waiting_fingerprint(bool,&str,i64)->Option<String>`、`should_notify`（复用）、`notified`/`notified_waiting`/`notify_on`/`present` 全程一致；`meowo_store::TranscriptInfo{title,error}` 解构与 Task 1 已合并特性的定义一致。✅
