# 模块 B：屏幕吸边缩略 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把贴纸窗口拖到屏幕左/右边缘时缩成一根竖条（纵向状态色点），悬停滑出完整列表，离开收回；拖离边缘恢复正常浮动尺寸。

**Architecture:** Rust 侧提供纯函数 `edge_for_rect`（可单测）判定窗口是否贴边，三个命令 `snap_collapse/snap_expand/snap_restore` 用物理像素直接 `set_size`/`set_position`，并在 `on_window_event` 的 `Moved` 上 emit `snap-changed { edge }`。前端 `App.tsx` 维护 `normal/collapsed/expanded` 状态机：用防抖计时器判定窗口"移动停止"后再触发吸附/恢复（避免在系统拖拽循环里 resize），折叠态渲染 `CollapsedStrip` 竖条、悬停展开。

**Tech Stack:** Tauri v2.11（Rust）+ React 18/TS。Rust 单测 `cargo test -p meowo-app`；前端 vitest。

设计来源：`docs/superpowers/specs/2026-06-04-release-and-polish-design.md` 模块 B。

> **关键设计决策（与 spec 一致，记录原因）：**
> 1. **全程物理像素**：`outer_position`/`outer_size` 与 `Monitor::work_area` 都是物理像素，直接比较/定位，无需 scale 换算；仅竖条宽度按 `scale_factor` 把逻辑 14px 换成物理像素，保证高 DPI 不过细。
> 2. **resize 不放进拖拽循环**：`WindowEvent::Moved` 拖动期间连续触发；Rust 仅 emit 检测到的边缘，真正 `set_size`/`set_position` 由前端在防抖（移动停止 ~250ms）后 invoke 命令完成，避免与系统拖拽抢位。
> 3. **Task 2/4 含窗口几何与状态机，自动化测试只覆盖纯函数与组件渲染**；吸附/悬停/恢复行为需运行 app 人工验证（见各任务说明）。

---

### Task 1: Rust 纯函数 `edge_for_rect` + Rect/Edge 类型与单测

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（顶部常量区之后新增 `Rect`/`Edge`/`edge_for_rect` 与文件末尾 `#[cfg(test)] mod tests`）

> 背景：`lib.rs` 现无测试模块。`Edge` 需 `Serialize`+`Deserialize`（既作 emit 负载又作命令入参，JS 侧用 `"left"`/`"right"` 字符串），`serde` 已是 meowo-app 依赖。判定规则：窗口左边距工作区左边、或右边距工作区右边 ≤ threshold 即吸附；两边都在阈值内取更近的一边；都不满足返回 None。

- [ ] **Step 1: 写失败单测**

在 `app/src-tauri/src/lib.rs` **文件末尾**追加：

```rust
#[cfg(test)]
mod tests {
    use super::{edge_for_rect, Edge, Rect};

    const WORK: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };

    #[test]
    fn left_within_threshold() {
        let win = Rect { x: 5, y: 0, w: 300, h: 400 };
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Left));
    }

    #[test]
    fn right_within_threshold() {
        // 右边距 = (0+1920) - (x+300) = 5
        let win = Rect { x: 1920 - 300 - 5, y: 0, w: 300, h: 400 };
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Right));
    }

    #[test]
    fn center_is_none() {
        let win = Rect { x: 800, y: 400, w: 300, h: 400 };
        assert_eq!(edge_for_rect(win, WORK, 20), None);
    }

    #[test]
    fn threshold_boundary_inclusive() {
        let win = Rect { x: 20, y: 0, w: 300, h: 400 }; // 左边距正好 20
        assert_eq!(edge_for_rect(win, WORK, 20), Some(Edge::Left));
    }

    #[test]
    fn just_outside_threshold_none() {
        let win = Rect { x: 21, y: 0, w: 300, h: 400 }; // 左边距 21
        assert_eq!(edge_for_rect(win, WORK, 20), None);
    }

    #[test]
    fn picks_nearer_edge() {
        // 小工作区：左边距 5，右边距 10 → 取更近的左
        let work = Rect { x: 0, y: 0, w: 320, h: 400 };
        let win = Rect { x: 5, y: 0, w: 305, h: 400 };
        assert_eq!(edge_for_rect(win, work, 20), Some(Edge::Left));
    }

    #[test]
    fn respects_work_area_offset() {
        // 工作区不从 0 开始（如左侧任务栏/多屏）
        let work = Rect { x: 100, y: 0, w: 1000, h: 1040 };
        let win = Rect { x: 110, y: 0, w: 300, h: 400 }; // 左边距 10
        assert_eq!(edge_for_rect(win, work, 20), Some(Edge::Left));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p meowo-app edge`
Expected: FAIL —— `cannot find function edge_for_rect`（编译错误）。

- [ ] **Step 3: 实现类型与纯函数**

在 `app/src-tauri/src/lib.rs` 中，紧接 `const STALE_THRESHOLD_MS` 那一行（约 14 行）**之后**插入：

```rust
/// 吸边判定阈值（物理像素）：窗口边缘距工作区边缘不超过此值即认为贴边。
const SNAP_THRESHOLD: i32 = 20;
/// 竖条逻辑宽度（实际物理宽度 = 该值 * 显示器 scale_factor）。
const STRIP_W_LOGICAL: f64 = 14.0;

/// 矩形（物理像素），用于吸边判定的纯计算。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// 吸附的边（仅左/右）。JS 侧序列化为 "left"/"right"。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
}

/// 判定窗口 `win` 是否贴在工作区 `work` 的左或右边缘（阈值 `threshold`）。
/// 两边都在阈值内时取更近的一边；都不满足返回 None。纯函数，便于单测。
pub fn edge_for_rect(win: Rect, work: Rect, threshold: i32) -> Option<Edge> {
    let left_gap = (win.x - work.x).abs();
    let right_gap = ((work.x + work.w) - (win.x + win.w)).abs();
    if left_gap <= threshold && left_gap <= right_gap {
        return Some(Edge::Left);
    }
    if right_gap <= threshold {
        return Some(Edge::Right);
    }
    None
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p meowo-app`
Expected: PASS（7 个 edge 测试）。

- [ ] **Step 5: clippy**

Run: `cargo clippy -p meowo-app -- -D warnings`
Expected: 无警告。

- [ ] **Step 6: 提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): 新增 edge_for_rect 吸边判定纯函数(Rect/Edge,7 单测)"
```

---

### Task 2: Rust 吸附命令 + 窗口移动事件

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（新增 `SnapPayload`、`snap_collapse`/`snap_expand`/`snap_restore` 命令、`on_window_event` 处理器，并注册命令）

> 背景：自定义命令无需改 capabilities（capabilities 只管 JS→core 插件边界，命令内部调 `set_size`/`set_position` 是 Rust 内部调用）。`tauri::{Emitter, Manager}` 已在 lib.rs 顶部导入。`Monitor::work_area()` 返回 `PhysicalRect`（`.position.x/.y`、`.size.width/.height`）。**若编译期发现 `work_area()` 不存在**（不同 2.x 小版本），退回用 `monitor.size()`+`monitor.position()`（忽略任务栏），并在提交信息注明。
>
> **本任务无法自动化测试**（依赖真实窗口/显示器）。验证方式：Step 末尾 `cargo check`+`clippy` 编译通过；运行行为留待 Task 4 完成后人工验证。

- [ ] **Step 1: 新增命令与事件处理器**

在 `app/src-tauri/src/lib.rs` 中，`edge_for_rect` 函数之后新增：

```rust
/// snap-changed 事件负载：当前检测到的吸附边（None 表示不贴边）。
#[derive(Clone, serde::Serialize)]
struct SnapPayload {
    edge: Option<Edge>,
}

/// 竖条物理宽度：逻辑宽度 * 显示器缩放，至少 1px。
fn strip_width_phys(scale: f64) -> i32 {
    ((STRIP_W_LOGICAL * scale).round() as i32).max(1)
}

/// 折叠成竖条：贴到指定边、宽度缩为竖条、高度与 y 保持不变。
#[tauri::command]
fn snap_collapse(window: tauri::WebviewWindow, edge: Edge) -> Result<(), String> {
    let m = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("no monitor")?;
    let wa = m.work_area();
    let strip_w = strip_width_phys(m.scale_factor());
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    let x = match edge {
        Edge::Left => wa.position.x,
        Edge::Right => wa.position.x + wa.size.width as i32 - strip_w,
    };
    window
        .set_size(tauri::PhysicalSize::new(strip_w as u32, size.height))
        .map_err(|e| e.to_string())?;
    window
        .set_position(tauri::PhysicalPosition::new(x, pos.y))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 展开成全宽（仍贴边）：宽度恢复为记住的逻辑宽度，y 与高度不变。
#[tauri::command]
fn snap_expand(window: tauri::WebviewWindow, edge: Edge, width: f64) -> Result<(), String> {
    let m = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("no monitor")?;
    let wa = m.work_area();
    let phys_w = ((width * m.scale_factor()).round() as i32).max(1);
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    let x = match edge {
        Edge::Left => wa.position.x,
        Edge::Right => wa.position.x + wa.size.width as i32 - phys_w,
    };
    window
        .set_size(tauri::PhysicalSize::new(phys_w as u32, size.height))
        .map_err(|e| e.to_string())?;
    window
        .set_position(tauri::PhysicalPosition::new(x, pos.y))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 恢复正常浮动：尺寸设回记住的逻辑宽高，位置维持用户当前拖到的地方。
#[tauri::command]
fn snap_restore(window: tauri::WebviewWindow, width: f64, height: f64) -> Result<(), String> {
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 2: 注册命令并接 `on_window_event`**

在 `run()` 的 `tauri::Builder` 链中：

1. 把 `invoke_handler` 的 `generate_handler!` 列表（现有 `get_overview, ..., set_archived`）末尾追加三个命令：

```rust
        .invoke_handler(tauri::generate_handler![
            get_overview,
            get_project_tasks,
            get_live_sessions,
            focus_session,
            set_archived,
            snap_collapse,
            snap_expand,
            snap_restore
        ])
```

2. 在 `.invoke_handler(...)` 之后、`.setup(...)` 之前插入 `on_window_event`：

```rust
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Moved(pos) = event {
                if let (Ok(Some(m)), Ok(size)) = (window.current_monitor(), window.outer_size()) {
                    let wa = m.work_area();
                    let win = Rect { x: pos.x, y: pos.y, w: size.width as i32, h: size.height as i32 };
                    let work = Rect {
                        x: wa.position.x,
                        y: wa.position.y,
                        w: wa.size.width as i32,
                        h: wa.size.height as i32,
                    };
                    let edge = edge_for_rect(win, work, SNAP_THRESHOLD);
                    let _ = window.emit("snap-changed", SnapPayload { edge });
                }
            }
        })
```

- [ ] **Step 3: 编译与静态检查**

Run: `cargo check -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: 编译通过、无警告。（若报 `no method work_area`，按本任务背景说明退回 `m.size()`/`m.position()` 方案：`work` 用整屏，`wa.position`→`m.position()`、`wa.size`→`m.size()`。）

- [ ] **Step 4: 提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): 吸附命令 snap_collapse/expand/restore + Moved 事件 emit snap-changed"
```

---

### Task 3: 前端 `CollapsedStrip` 竖条组件 + vitest

**Files:**
- Create: `app/src/views/CollapsedStrip.tsx`
- Test: `app/src/views/CollapsedStrip.test.tsx`

> 背景：`Sticker.tsx` 里每个会话的指示器逻辑为：`!connected`→停止环；`status==="running"`→spinner；`status==="waiting"`→needs；否则在线点。竖条复用同一判定，渲染纵向小圆点。组件保持纯展示（交互回调由 `App` 注入），便于单测。`Item` 类型 = `LiveSession & { connected: boolean }`，与 `Sticker` 一致。

- [ ] **Step 1: 写失败测试**

新建 `app/src/views/CollapsedStrip.test.tsx`：

```tsx
import { describe, it, expect, afterEach } from "vitest";
import { render, cleanup } from "@testing-library/react";
import { CollapsedStrip } from "./CollapsedStrip";
import type { LiveSession } from "../api";

type Item = LiveSession & { connected: boolean };

function mk(over: Partial<Item> = {}): Item {
  return {
    session: { id: 1, project_id: 1, cc_session_id: "s", status: "running", started_at: 0, last_event_at: 0, ended_at: null },
    project_name: "proj",
    task_title: "t",
    current_activity: null,
    column: "doing", todo_done: 0, todo_total: 0, todos: [],
    pid: 1, connected: true, archived: false,
    ...over,
  } as Item;
}

afterEach(() => cleanup());

describe("CollapsedStrip", () => {
  it("每个非归档会话渲染一个圆点，按状态给类名", () => {
    const data: Item[] = [
      mk({ session: { id: 1, project_id: 1, cc_session_id: "a", status: "running", started_at: 0, last_event_at: 0, ended_at: null }, connected: true }),
      mk({ session: { id: 2, project_id: 1, cc_session_id: "b", status: "waiting", started_at: 0, last_event_at: 0, ended_at: null }, connected: true }),
      mk({ session: { id: 3, project_id: 1, cc_session_id: "c", status: "ended", started_at: 0, last_event_at: 0, ended_at: null }, connected: false }),
    ];
    const { container } = render(<CollapsedStrip data={data} edge="left" onExpand={() => {}} onLeave={() => {}} />);
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(3);
    expect(container.querySelectorAll(".cstrip-running").length).toBe(1);
    expect(container.querySelectorAll(".cstrip-waiting").length).toBe(1);
    expect(container.querySelectorAll(".cstrip-stop").length).toBe(1);
  });

  it("归档会话不计入竖条", () => {
    const data: Item[] = [mk({ archived: true }), mk({ session: { id: 2, project_id: 1, cc_session_id: "b", status: "running", started_at: 0, last_event_at: 0, ended_at: null } })];
    const { container } = render(<CollapsedStrip data={data} edge="right" onExpand={() => {}} onLeave={() => {}} />);
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(1);
  });

  it("edge 决定容器修饰类", () => {
    const { container } = render(<CollapsedStrip data={[]} edge="right" onExpand={() => {}} onLeave={() => {}} />);
    expect(container.querySelector(".cstrip-right")).toBeTruthy();
  });
});
```

- [ ] **Step 2: 运行确认失败**

Run: `cd app && bunx vitest run src/views/CollapsedStrip.test.tsx`
Expected: FAIL —— 找不到 `./CollapsedStrip`。

- [ ] **Step 3: 实现组件**

新建 `app/src/views/CollapsedStrip.tsx`：

```tsx
import { LiveSession } from "../api";

type Item = LiveSession & { connected: boolean };
type Edge = "left" | "right";

/// 竖条：纵向排列各非归档会话的状态色点。悬停展开、离开收回由 App 注入回调。
export function CollapsedStrip({
  data,
  edge,
  onExpand,
  onLeave,
}: {
  data: Item[];
  edge: Edge;
  onExpand: () => void;
  onLeave: () => void;
}) {
  const items = data.filter((l) => !l.archived);
  return (
    <div
      className={"cstrip cstrip-" + edge}
      onMouseEnter={onExpand}
      onMouseLeave={onLeave}
    >
      <div className="cstrip-drag" data-tauri-drag-region />
      <div className="cstrip-dots">
        {items.map((l) => {
          const cls = !l.connected
            ? "cstrip-stop"
            : l.session.status === "running"
            ? "cstrip-running"
            : l.session.status === "waiting"
            ? "cstrip-waiting"
            : "cstrip-on";
          return <span key={l.session.id} className={"cstrip-dot " + cls} />;
        })}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cd app && bunx vitest run src/views/CollapsedStrip.test.tsx`
Expected: PASS（3 个用例）。

- [ ] **Step 5: 提交**

```bash
git add app/src/views/CollapsedStrip.tsx app/src/views/CollapsedStrip.test.tsx
git commit -m "feat(app): 新增 CollapsedStrip 竖条组件(纵向状态色点,3 测试)"
```

---

### Task 4: App 状态机接线 + 竖条样式

**Files:**
- Modify: `app/src/App.tsx`（吸附状态机 + snap-changed 监听 + 防抖触发命令）
- Modify: `app/src/styles.css`（竖条样式）

> 背景：`App.tsx` 现仅 `return <Sticker data={live} />`。本任务加状态机：监听 `snap-changed`，用防抖（移动停止 ~250ms）判定边缘，再 invoke 命令。状态 `normal/collapsed/expanded`，吸附边与正常尺寸存 localStorage、重启沿用。
>
> **本任务核心逻辑无法 jsdom 自动化测试**（依赖 Tauri 事件/命令/真实窗口）。验证方式：`bunx tsc --noEmit` + `bunx vitest run`（确保不破坏既有测试）通过后，**运行 app 人工验证**吸附/悬停/恢复（见 Step 4）。

- [ ] **Step 1: 重写 `App.tsx`**

把 `app/src/App.tsx` 整体替换为：

```tsx
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getLiveSessions, LiveSession } from "./api";
import { Sticker } from "./views/Sticker";
import { CollapsedStrip } from "./views/CollapsedStrip";

type Item = LiveSession & { connected: boolean };
type Edge = "left" | "right";
type Mode = "normal" | "collapsed" | "expanded";

const SNAP_KEY = "meowo-snap-edge";
const SIZE_KEY = "meowo-normal-size";
const SETTLE_MS = 250; // 移动停止判定
const LEAVE_MS = 300; // 离开收回防抖

function loadSize(): { w: number; h: number } {
  try {
    const s = JSON.parse(localStorage.getItem(SIZE_KEY) || "");
    if (typeof s?.w === "number" && typeof s?.h === "number") return s;
  } catch {
    /* ignore */
  }
  return { w: 340, h: 440 }; // 与 tauri.conf.json 默认一致
}

export function App() {
  const [live, setLive] = useState<Item[]>([]);
  const [mode, setMode] = useState<Mode>("normal");
  const [edge, setEdge] = useState<Edge | null>(() => {
    const s = localStorage.getItem(SNAP_KEY);
    return s === "left" || s === "right" ? s : null;
  });

  const modeRef = useRef(mode);
  const edgeRef = useRef(edge);
  modeRef.current = mode;
  edgeRef.current = edge;
  const settleTimer = useRef<number | null>(null);
  const leaveTimer = useRef<number | null>(null);

  const refresh = useCallback(async () => {
    setLive((await getLiveSessions()) as Item[]);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    const un = listen("board-changed", () => refresh());
    return () => {
      un.then((f) => f());
    };
  }, [refresh]);

  // 监听窗口移动检测到的边缘，防抖判定移动停止后再吸附/恢复。
  useEffect(() => {
    const un = listen<{ edge: Edge | null }>("snap-changed", (e) => {
      const detected = e.payload.edge;
      if (settleTimer.current) window.clearTimeout(settleTimer.current);
      settleTimer.current = window.setTimeout(async () => {
        const m = modeRef.current;
        if (detected && m === "normal") {
          // 记住当前正常尺寸，吸附折叠
          try {
            const sz = await getCurrentWindow().outerSize();
            const sf = await getCurrentWindow().scaleFactor();
            localStorage.setItem(SIZE_KEY, JSON.stringify({ w: sz.width / sf, h: sz.height / sf }));
          } catch {
            /* ignore */
          }
          localStorage.setItem(SNAP_KEY, detected);
          setEdge(detected);
          await invoke("snap_collapse", { edge: detected }).catch(() => {});
          setMode("collapsed");
        } else if (!detected && m !== "normal") {
          // 拖离边缘：恢复正常尺寸
          const { w, h } = loadSize();
          localStorage.removeItem(SNAP_KEY);
          setEdge(null);
          await invoke("snap_restore", { width: w, height: h }).catch(() => {});
          setMode("normal");
        }
      }, SETTLE_MS);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // 重启沿用：若上次是吸附态，启动后折叠回竖条。
  useEffect(() => {
    if (edgeRef.current) {
      invoke("snap_collapse", { edge: edgeRef.current })
        .then(() => setMode("collapsed"))
        .catch(() => {});
    }
    // 仅启动跑一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onExpand = useCallback(() => {
    if (leaveTimer.current) window.clearTimeout(leaveTimer.current);
    if (modeRef.current !== "collapsed" || !edgeRef.current) return;
    const { w } = loadSize();
    invoke("snap_expand", { edge: edgeRef.current, width: w })
      .then(() => setMode("expanded"))
      .catch(() => {});
  }, []);

  const onLeave = useCallback(() => {
    if (leaveTimer.current) window.clearTimeout(leaveTimer.current);
    leaveTimer.current = window.setTimeout(() => {
      if (modeRef.current === "expanded" && edgeRef.current) {
        invoke("snap_collapse", { edge: edgeRef.current })
          .then(() => setMode("collapsed"))
          .catch(() => {});
      }
    }, LEAVE_MS);
  }, []);

  if (mode === "collapsed" && edge) {
    return <CollapsedStrip data={live} edge={edge} onExpand={onExpand} onLeave={onLeave} />;
  }
  // expanded 与 normal 都显示完整贴纸；expanded 态额外挂 onMouseLeave 收回。
  return (
    <div
      style={{ height: "100vh" }}
      onMouseLeave={mode === "expanded" ? onLeave : undefined}
      onMouseEnter={mode === "expanded" ? onExpand : undefined}
    >
      <Sticker data={live} />
    </div>
  );
}
```

- [ ] **Step 2: 竖条样式**

在 `app/src/styles.css` 末尾追加：

```css
/* 吸边竖条 */
.cstrip {
  height: 100vh;
  width: 100%;
  background: var(--cc-bg);
  border: 1px solid var(--cc-border);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 6px 0;
  overflow: hidden;
  backdrop-filter: blur(6px);
}
.cstrip-left { border-radius: 0 var(--r-lg) var(--r-lg) 0; }
.cstrip-right { border-radius: var(--r-lg) 0 0 var(--r-lg); }
.cstrip-drag { width: 100%; height: 14px; flex: none; }
.cstrip-dots {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 7px;
  overflow-y: auto;
}
.cstrip-dot { width: 8px; height: 8px; border-radius: 50%; flex: none; }
.cstrip-on { background: var(--cc-accent, #4ade80); }
.cstrip-running { background: var(--cc-text-dim); animation: cstrip-pulse 1.2s ease-in-out infinite; }
.cstrip-waiting { background: #f5a623; }
.cstrip-stop { background: transparent; border: 1.5px solid var(--cc-text-faint); }
@keyframes cstrip-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.35; } }
```

- [ ] **Step 3: 类型检查 + 前端测试回归**

Run: `cd app && bunx tsc --noEmit && bunx vitest run`
Expected: tsc 无输出；vitest 全绿（含 CollapsedStrip 3 个 + 既有 21 个）。

- [ ] **Step 4: 运行 app 人工验证（Task 2+4 的真实行为）**

Run: `cd app && bun run tauri dev`（或已构建的 app）
人工确认：
1. 把窗口拖到屏幕左边缘释放 → 约 250ms 后缩成左侧竖条，竖条按会话状态显示色点。
2. 鼠标移到竖条上 → 滑出完整贴纸（仍贴左）。
3. 鼠标移开 → 约 300ms 后收回竖条。
4. 拖动竖条离开边缘 → 恢复到之前的正常尺寸与浮动。
5. 右边缘同理。
6. 吸附态下关闭再开 → 重启后仍为竖条。
若几何/单位有偏差（如高 DPI 下竖条过宽/位置偏移），在此微调 `snap_collapse/expand` 的物理像素计算与 `SETTLE_MS`。

- [ ] **Step 5: 提交**

```bash
git add app/src/App.tsx app/src/styles.css
git commit -m "feat(app): 吸边缩略状态机+竖条样式(防抖吸附/悬停展开/拖离恢复/重启沿用)"
```

---

## 自检（Self-Review）

- **Spec 覆盖**：
  - 状态机 normal/snapped-collapsed/snapped-expanded → Task 4 `Mode`。
  - 转移表（拖动释放贴边→折叠、竖条 enter→展开、leave 防抖 300ms→折叠、拖离→恢复）→ Task 4 snap-changed 防抖 + onExpand/onLeave。
  - 仅左右边、竖条形态、记住吸附边+正常尺寸到 localStorage、重启沿用 → Task 1 `Edge` 仅 Left/Right、Task 4 `SNAP_KEY`/`SIZE_KEY` + 启动 effect。
  - 纯函数 `edge_for_rect`（左/右/居中/阈值）→ Task 1 七个单测。
  - 命令 `snap_collapse/expand/restore` 用 set_position/set_size + current_monitor 工作区 → Task 2。
  - `on_window_event` 处理 Moved → emit `snap-changed { collapsed/edge }`（本实现 emit `{ edge }`，collapsed 由前端状态推导，语义等价且更简）→ Task 2。
  - 前端 `CollapsedStrip` 纵向状态点、enter→snap_expand、leave→snap_collapse；折叠渲染竖条否则 Sticker → Task 3 + Task 4。
  - 测试：edge_for_rect 单测 + CollapsedStrip 多状态点数/类名 → Task 1 + Task 3。
- **占位扫描**：无 TBD/“适当处理”，代码步骤均完整。Task 2/4 的真实行为验证以 Step 形式明确为"运行 app 人工确认"，非占位。
- **类型一致性**：`Edge`（Rust `Left/Right` ↔ JS `"left"/"right"` 经 serde lowercase）；命令入参名 `edge`/`width`/`height` 与 JS `invoke` 的 `{ edge, width }`/`{ width, height }` 一致（Tauri 自动 camel→snake，此处均单词无需转换）；`Item` 类型在 App/Sticker/CollapsedStrip 一致；`snap_collapse(edge)`/`snap_expand(edge,width)`/`snap_restore(width,height)` 签名在 Task 2 定义、Task 4 调用一致。
- **风险点（已在任务内标注）**：`Monitor::work_area()` 版本差异退路；高 DPI 物理像素换算需人工微调；window-state 插件可能与重启沿用的尺寸交互——Step 4 人工验证时一并观察。

## 非目标

- 上/下边缘吸附与横向缩略条。
- 竖条上展示标题/进度（仅状态色点）。
- 多显示器跨屏吸附的精细处理（以当前所在显示器工作区为准）。
