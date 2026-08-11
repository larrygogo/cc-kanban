# README 视觉升级 + 浏览器合成演示 GIF 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用可一键重新生成的浏览器合成管线产出新 demo.gif,并对 README 做全面视觉升级(纯中文,内容不增不减)。

**Architecture:** `app/demo.html` 仅 dev 入口(不进生产构建),页面里用 `@tauri-apps/api/mocks` 的 `mockIPC` 喂假会话数据,复用真实 `Sticker`/`CollapsedStrip` 组件渲染在「渐变桌面 + 窗口阴影」舞台上;页面内置确定性时间轴(按帧 seek,CSS 动画统一钉时间),`app/scripts/record-demo.mjs` 用 Playwright 逐帧截图、gifenc 全局调色板 + 帧间差分编码成 GIF。README 重写为居中头部 + 徽章组 + 分组特性区。

**Tech Stack:** React 18 + Vite(现有)、`@tauri-apps/api/mocks`、Playwright(chromium)、gifenc、fast-png、Bun。

**约束(来自 spec):**
- App 本体组件不改动(`Sticker.tsx` / `CollapsedStrip.tsx` / `styles.css` 等零修改);demo 专属覆写全部放 `demo.css`。
- `bun run build` 产物不得包含 demo 入口(vite 默认 rollup input 只有 `index.html`,新增 `demo.html` 不配置即不打包,需验证)。
- demo.gif < 4MB、760 宽展示无明显失真;同输入逐帧确定。

**舞台/分镜总览(880×560 @1x,12fps,约 20s):**

| 时间 | 场景 | 字幕 |
|---|---|---|
| 0–4.5s | 4 张卡:2 运行中(activity/Context% 实时变)、1 闲置、1 已断开 | 所有 Claude Code 会话,一眼看全 |
| 4.5–8.5s | 卡 2 转「待交互」(黄),光标点「待交互」tab 过滤,再切回「全部」 | 谁在等你回复,立刻知道 |
| 8.5–13s | 卡 2 重命名(逐字输入+回车),卡 4 点归档收进「已归档」 | 重命名、归档,即点即管 |
| 13–17.5s | 窗口滑向右缘缩成竖状态条(3 色点),hover 偷看展开再收回 | 吸边缩成一根状态条,不占地方 |
| 17.5–20s | 桌面上 logo + Meowo + slogan 淡入 | — |

**文件清单:**
- Create: `app/demo.html` — demo 页入口
- Create: `app/src/demo/data.ts` — 假会话构造器
- Create: `app/src/demo/mock.ts` — mockIPC + store + 订阅
- Create: `app/src/demo/mock.test.ts`
- Create: `app/src/demo/timeline.ts` — 确定性时间轴引擎
- Create: `app/src/demo/timeline.test.ts`
- Create: `app/src/demo/cursor.ts` — 假光标/点击/打字助手
- Create: `app/src/demo/DemoStage.tsx` — 桌面舞台
- Create: `app/src/demo/demo.css` — 舞台样式 + demo 内覆写
- Create: `app/src/demo/script.ts` — 分镜
- Create: `app/src/demo/main.tsx` — demo 入口,暴露 `window.__demo`
- Create: `app/scripts/record-demo.mjs` — 录制管线
- Modify: `app/package.json` — devDeps + `demo:gif` 脚本
- Replace: `docs/images/demo.gif`
- Create: `docs/images/logo.png`(从 `app/src-tauri/icons/128x128@2x.png` 复制)
- Rewrite: `README.md`

---

### Task 1: 依赖与 demo 入口脚手架

**Files:**
- Modify: `app/package.json`
- Create: `app/demo.html`

- [ ] **Step 1: 安装依赖**

```bash
cd app
bun add -d playwright gifenc fast-png
bunx playwright install chromium
```

- [ ] **Step 2: 创建 `app/demo.html`**

```html
<!doctype html>
<html lang="zh">
  <head><meta charset="UTF-8" /><title>Meowo demo</title></head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/demo/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 3: 验证生产构建不含 demo**

Run: `cd app && bun run build && ls dist`
Expected: `dist/` 只有 `index.html` + assets,无 `demo.html`。

- [ ] **Step 4: Commit**

```bash
git add app/package.json app/bun.lock app/demo.html
git commit -m "chore(demo): 录制管线依赖(playwright/gifenc/fast-png) + demo 页入口"
```

---

### Task 2: 假会话数据与 mockIPC 层

**Files:**
- Create: `app/src/demo/data.ts`
- Create: `app/src/demo/mock.ts`
- Test: `app/src/demo/mock.test.ts`

- [ ] **Step 1: 写 `data.ts`(假会话构造器,字段与 `api.ts` 的 `LiveSession` 一一对应)**

```ts
import { LiveSession } from "../api";

export type Item = LiveSession & { connected: boolean };

let nextId = 1;
const NOW = Date.now();

export function makeSession(p: {
  title: string;
  project: string;
  status?: "running" | "waiting" | "ended" | "stale";
  activity?: string | null;
  ctx?: number | null;
  agoMin?: number;
  connected?: boolean;
  archived?: boolean;
  todoDone?: number;
  todoTotal?: number;
}): Item {
  const id = nextId++;
  return {
    session: {
      id,
      project_id: id,
      cc_session_id: `demo-${id}`,
      status: p.status ?? "running",
      started_at: NOW - 3_600_000,
      last_event_at: NOW - (p.agoMin ?? 0) * 60_000,
      ended_at: null,
    },
    project_name: p.project,
    task_title: p.title,
    current_activity: p.activity ?? null,
    column: "doing",
    todo_done: p.todoDone ?? 0,
    todo_total: p.todoTotal ?? 0,
    todos: [],
    pid: 1000 + id,
    connected: p.connected ?? true,
    archived: p.archived ?? false,
    archived_at: p.archived ? NOW : null,
    cwd: `C:/dev/${p.project.split("/").pop()}`,
    errored: false,
    error_label: null,
    error_raw: null,
    context_pct: p.ctx ?? null,
    context_window: p.ctx != null ? 200_000 : null,
  };
}
```

- [ ] **Step 2: 写失败测试 `mock.test.ts`**

```ts
import { beforeEach, expect, test } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { clearMocks } from "@tauri-apps/api/mocks";
import { makeSession } from "./data";
import { installMocks, store } from "./mock";

beforeEach(() => {
  clearMocks();
  store.sessions = [];
});

test("get_live_sessions 返回 store 内容", async () => {
  installMocks();
  store.sessions = [makeSession({ title: "A", project: "x/y" })];
  const r = await invoke("get_live_sessions");
  expect(r).toHaveLength(1);
});

test("rename_session 改 store 里的标题", async () => {
  installMocks();
  const s = makeSession({ title: "旧名", project: "x/y" });
  store.sessions = [s];
  await invoke("rename_session", { cwd: s.cwd, sessionId: s.session.cc_session_id, title: "新名" });
  expect(store.sessions[0].task_title).toBe("新名");
});

test("set_archived 切换归档位", async () => {
  installMocks();
  const s = makeSession({ title: "A", project: "x/y" });
  store.sessions = [s];
  await invoke("set_archived", { sessionId: s.session.id, archived: true });
  expect(store.sessions[0].archived).toBe(true);
  expect(store.sessions[0].archived_at).not.toBeNull();
});
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cd app && bunx vitest run src/demo/mock.test.ts`
Expected: FAIL(mock.ts 不存在)。

- [ ] **Step 4: 写 `mock.ts`**

舞台状态(窗口形态/字幕/收尾)也放 store,分镜动作改它后 `notify()` 即可驱动 React。

```ts
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { Settings } from "../api";
import { Item } from "./data";

export type StageMode = "normal" | "docking" | "strip" | "expanded";

export type Store = {
  sessions: Item[];
  stage: { mode: StageMode; caption: string | null; finale: boolean };
  settings: Settings;
};

export const store: Store = {
  sessions: [],
  stage: { mode: "normal", caption: null, finale: false },
  settings: {
    archive_hide_days: 0,
    notifications_enabled: true,
    theme: "dark",
    opacity: 97,
    ui_scale: 100,
    resume_terminal: "wt",
  },
};

const subs = new Set<() => void>();
export function subscribe(fn: () => void): () => void {
  subs.add(fn);
  return () => subs.delete(fn);
}
export function notify(): void {
  subs.forEach((f) => f());
}

export function installMocks(): void {
  mockWindows("main");
  mockIPC((cmd, args) => {
    switch (cmd) {
      case "host_os":
        return "windows";
      case "get_settings":
        return store.settings;
      case "get_live_sessions":
        return store.sessions;
      case "rename_session": {
        const a = args as { sessionId: string; title: string };
        const s = store.sessions.find((x) => x.session.cc_session_id === a.sessionId);
        if (s) s.task_title = a.title;
        notify();
        return null;
      }
      case "set_archived": {
        const a = args as { sessionId: number; archived: boolean };
        const s = store.sessions.find((x) => x.session.id === a.sessionId);
        if (s) {
          s.archived = a.archived;
          s.archived_at = a.archived ? Date.now() : null;
        }
        notify();
        return null;
      }
      case "plugin:event|listen":
        return 1;
      default:
        // focus_session / resume_session / snap_* / plugin:window|* 等一律 no-op
        return null;
    }
  });
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cd app && bunx vitest run src/demo/mock.test.ts`
Expected: 3 passed。

- [ ] **Step 6: Commit**

```bash
git add app/src/demo/data.ts app/src/demo/mock.ts app/src/demo/mock.test.ts
git commit -m "feat(demo): 假会话数据 + mockIPC 层(get_live_sessions/rename/archive 可交互)"
```

---

### Task 3: 确定性时间轴引擎

**Files:**
- Create: `app/src/demo/timeline.ts`
- Test: `app/src/demo/timeline.test.ts`

设计要点:
- `at(sec, fn)` 一次性动作;`tween(from, to, apply)` 区间插值(每帧调用,结束后钉在 k=1)。
- `seek(frame)` 幂等向前推进:执行所有到期未执行动作(按时间排序)→ 等两次 rAF 让 React 落地 → 把页面全部 CSS 动画钉到「相对各自首次出现时刻」的时间(新挂载的动画从 0 开始,确定性逐帧)。
- rAF/动画同步通过构造器注入,便于 jsdom 下测试纯调度逻辑。

- [ ] **Step 1: 写失败测试 `timeline.test.ts`**

```ts
import { expect, test } from "vitest";
import { Timeline } from "./timeline";

const noopHooks = { paint: async () => {}, sync: (_ms: number) => {} };

test("seek 按时间序执行到期动作,且只执行一次", async () => {
  const tl = new Timeline(10, noopHooks);
  const log: string[] = [];
  tl.at(0.3, () => log.push("b"));
  tl.at(0.1, () => log.push("a"));
  await tl.seek(3); // t=0.3
  expect(log).toEqual(["a", "b"]);
  await tl.seek(4);
  expect(log).toEqual(["a", "b"]);
});

test("tween 在区间内插值、区间后钉在 1", async () => {
  const tl = new Timeline(10, noopHooks);
  const ks: number[] = [];
  tl.tween(0.0, 1.0, (k) => ks.push(k), (x) => x);
  await tl.seek(0); // k=0
  await tl.seek(5); // k=0.5
  await tl.seek(20); // k=1(超出区间)
  expect(ks).toEqual([0, 0.5, 1]);
});

test("tween 开始前不调用 apply", async () => {
  const tl = new Timeline(10, noopHooks);
  const ks: number[] = [];
  tl.tween(1.0, 2.0, (k) => ks.push(k), (x) => x);
  await tl.seek(5); // t=0.5 < from
  expect(ks).toEqual([]);
});

test("duration 取动作与 tween 的最大时刻", () => {
  const tl = new Timeline(10, noopHooks);
  tl.at(3, () => {});
  tl.tween(1, 5, () => {});
  expect(tl.duration).toBe(5);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd app && bunx vitest run src/demo/timeline.test.ts`
Expected: FAIL(timeline.ts 不存在)。

- [ ] **Step 3: 写 `timeline.ts`**

```ts
type Action = { at: number; run: () => void | Promise<void> };
type Ease = (x: number) => number;
type Tween = { from: number; to: number; apply: (k: number) => void; ease: Ease };
type Hooks = { paint: () => Promise<void>; sync: (ms: number) => void };

export const easeInOut: Ease = (x) =>
  x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2;

export class Timeline {
  readonly fps: number;
  duration = 0;
  private actions: Action[] = [];
  private tweens: Tween[] = [];
  private done = new Set<Action>();
  private hooks: Hooks;

  constructor(fps: number, hooks?: Hooks) {
    this.fps = fps;
    this.hooks = hooks ?? { paint: nextPaint, sync: syncAnimations };
  }

  at(sec: number, run: Action["run"]): void {
    this.actions.push({ at: sec, run });
    this.duration = Math.max(this.duration, sec);
  }

  tween(from: number, to: number, apply: (k: number) => void, ease: Ease = easeInOut): void {
    this.tweens.push({ from, to, apply, ease });
    this.duration = Math.max(this.duration, to);
  }

  /** 跳到第 n 帧(只向前)。 */
  async seek(frame: number): Promise<void> {
    const t = frame / this.fps;
    const due = this.actions
      .filter((a) => a.at <= t && !this.done.has(a))
      .sort((a, b) => a.at - b.at);
    for (const a of due) {
      this.done.add(a);
      await a.run();
    }
    for (const w of this.tweens) {
      if (t < w.from) continue;
      const k = Math.min(1, (t - w.from) / Math.max(w.to - w.from, 1e-9));
      w.apply(w.ease(k));
    }
    await this.hooks.paint();
    this.hooks.sync(t * 1000);
    await this.hooks.paint();
  }
}

function nextPaint(): Promise<void> {
  return new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r())));
}

/** 把页面全部 CSS 动画钉到「相对首次出现时刻」的统一时间轴(逐帧确定)。 */
const seen = new Map<Animation, number>();
function syncAnimations(ms: number): void {
  for (const a of document.getAnimations()) {
    let t0 = seen.get(a);
    if (t0 === undefined) {
      seen.set(a, ms);
      t0 = ms;
    }
    try {
      a.pause();
      a.currentTime = Math.max(0, ms - t0);
    } catch {
      /* 个别动画不可 seek,忽略 */
    }
  }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd app && bunx vitest run src/demo/timeline.test.ts`
Expected: 4 passed。

- [ ] **Step 5: Commit**

```bash
git add app/src/demo/timeline.ts app/src/demo/timeline.test.ts
git commit -m "feat(demo): 确定性时间轴引擎(动作/补间/CSS 动画统一钉时间)"
```

---

### Task 4: 桌面舞台、光标与 demo 入口

**Files:**
- Create: `app/src/demo/cursor.ts`
- Create: `app/src/demo/DemoStage.tsx`
- Create: `app/src/demo/demo.css`
- Create: `app/src/demo/main.tsx`

- [ ] **Step 1: 写 `cursor.ts`(假光标移动/点击涟漪/受控输入)**

```ts
import { Timeline } from "./timeline";

export const cursor = { x: 440, y: 540 };

export function setCursor(x: number, y: number): void {
  cursor.x = x;
  cursor.y = y;
  const el = document.getElementById("demo-cursor");
  if (el) el.style.transform = `translate(${x}px, ${y}px)`;
}

/** from→to 秒内把光标平滑移到 sel 中心(目标位置在补间首帧时再解析,容许元素晚挂载)。 */
export function moveToEl(tl: Timeline, from: number, to: number, sel: string): void {
  let sx = 0, sy = 0, tx = 0, ty = 0, init = false;
  tl.tween(from, to, (k) => {
    if (!init) {
      init = true;
      sx = cursor.x;
      sy = cursor.y;
      const el = document.querySelector(sel);
      if (!el) {
        console.warn("[demo] moveToEl 未找到:", sel);
        tx = sx; ty = sy;
      } else {
        const r = el.getBoundingClientRect();
        tx = r.left + r.width / 2;
        ty = r.top + r.height / 2;
      }
    }
    setCursor(sx + (tx - sx) * k, sy + (ty - sy) * k);
  });
}

/** 点击 sel:光标钉到元素中心、出涟漪、派发真实 click。 */
export function clickEl(sel: string): void {
  const el = document.querySelector<HTMLElement>(sel);
  if (!el) {
    console.warn("[demo] clickEl 未找到:", sel);
    return;
  }
  const r = el.getBoundingClientRect();
  setCursor(r.left + r.width / 2, r.top + r.height / 2);
  ripple(cursor.x, cursor.y);
  el.click();
}

/** 点击涟漪:CSS 动画 fill forwards 结束即透明,无需移除(保持逐帧确定)。 */
function ripple(x: number, y: number): void {
  const d = document.createElement("div");
  d.className = "demo-ripple";
  d.style.left = `${x}px`;
  d.style.top = `${y}px`;
  document.body.appendChild(d);
}

/** 受控 input 逐字输入(React 18 监听原生 input 事件)。 */
export function typeText(tl: Timeline, startSec: number, sel: string, text: string, cps = 14): void {
  for (let i = 1; i <= text.length; i++) {
    tl.at(startSec + i / cps, () => {
      const el = document.querySelector<HTMLInputElement>(sel);
      if (!el) return;
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      set.call(el, text.slice(0, i));
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }
}

export function pressKey(sel: string, key: string): void {
  document
    .querySelector<HTMLInputElement>(sel)
    ?.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
}
```

- [ ] **Step 2: 写 `DemoStage.tsx`**

```tsx
import { useEffect, useReducer } from "react";
import { Sticker } from "../views/Sticker";
import { CollapsedStrip } from "../views/CollapsedStrip";
import { store, subscribe } from "./mock";
import logoUrl from "../../src-tauri/icons/128x128@2x.png";

export function DemoStage() {
  const [, force] = useReducer((x: number) => x + 1, 0);
  useEffect(() => subscribe(force), []);
  const { mode, caption, finale } = store.stage;
  const strip = mode === "strip";
  return (
    <div className="demo-desktop">
      <div className="demo-blob demo-blob-a" />
      <div className="demo-blob demo-blob-b" />
      <div className="demo-grain" />
      <div className={"demo-window demo-mode-" + mode}>
        {strip ? (
          <CollapsedStrip data={store.sessions} edge="right" onExpand={() => {}} />
        ) : (
          <Sticker data={store.sessions} />
        )}
      </div>
      {caption && (
        <div className="demo-caption" key={caption}>
          {caption}
        </div>
      )}
      {finale && (
        <div className="demo-finale">
          <img src={logoUrl} width={88} height={88} alt="" />
          <div className="demo-finale-name">Meowo</div>
          <div className="demo-finale-slogan">你所有的 Claude Code 会话,一眼看全</div>
        </div>
      )}
      <div id="demo-cursor">
        <svg width="20" height="22" viewBox="0 0 20 22">
          <path
            d="M2 1 L2 17 L6.5 13.5 L9.5 20 L12.5 18.7 L9.6 12.3 L15.5 11.8 Z"
            fill="#fff"
            stroke="#1b1917"
            strokeWidth="1.4"
          />
        </svg>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: 写 `demo.css`**

窗口四种形态由 `.demo-mode-*` 控制;贴纸/缩略条在窗口容器内铺满(覆写其 `100vh`);隐藏 fixed 定位的 resize-grip。

```css
/* ===== demo 舞台(仅 demo.html 引入,不影响 App) ===== */
.demo-desktop {
  position: fixed;
  inset: 0;
  overflow: hidden;
  background:
    radial-gradient(1100px 600px at 18% -10%, #2e2a26 0%, transparent 55%),
    radial-gradient(900px 560px at 105% 110%, #26221f 0%, transparent 60%),
    linear-gradient(160deg, #211e1b 0%, #1b1917 55%, #171513 100%);
  font-family: "Segoe UI", "Microsoft YaHei", sans-serif;
}
.demo-blob {
  position: absolute;
  border-radius: 50%;
  filter: blur(90px);
  opacity: 0.16;
}
.demo-blob-a { width: 420px; height: 420px; left: -90px; top: 240px; background: #d97757; }
.demo-blob-b { width: 380px; height: 380px; right: -60px; top: -120px; background: #4ec9a5; opacity: 0.10; }
/* 细噪点:遮 GIF 量化色带;静态内容帧间差分为透明,不增体积 */
.demo-grain {
  position: absolute;
  inset: 0;
  opacity: 0.05;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='160' height='160'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2' seed='7'/%3E%3C/filter%3E%3Crect width='160' height='160' filter='url(%23n)' opacity='0.6'/%3E%3C/svg%3E");
}

/* 贴纸窗口:340×430 居中偏上;dock 时滑向右缘;strip 时变 14px 竖条 */
.demo-window {
  position: absolute;
  left: 270px;
  top: 52px;
  width: 340px;
  height: 430px;
  border-radius: 9px;
  box-shadow: 0 28px 70px rgba(0, 0, 0, 0.55), 0 4px 16px rgba(0, 0, 0, 0.35);
  transition: left 0.7s cubic-bezier(0.4, 0, 0.2, 1), top 0.7s cubic-bezier(0.4, 0, 0.2, 1),
    width 0.3s ease, height 0.3s ease;
}
.demo-mode-docking { left: 539px; top: 52px; }
.demo-mode-strip {
  left: 866px;
  top: 120px;
  width: 14px;
  height: 96px;
  border-radius: 8px 0 0 8px;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
}
.demo-mode-expanded { left: 539px; top: 52px; }
/* 贴纸/缩略条原样式按窗口高度 100vh 铺;demo 里改成跟随容器 */
.demo-window .sticker,
.demo-window .cstrip { height: 100%; }
.demo-window .resize-grip { display: none; }

/* 字幕:底部居中胶囊,换字幕时重新淡入 */
.demo-caption {
  position: absolute;
  left: 50%;
  bottom: 26px;
  transform: translateX(-50%);
  padding: 9px 22px;
  border-radius: 999px;
  background: rgba(20, 18, 16, 0.78);
  border: 1px solid rgba(255, 255, 255, 0.1);
  color: #f5f4ef;
  font-size: 15px;
  letter-spacing: 0.4px;
  white-space: nowrap;
  animation: demo-cap-in 0.45s ease both;
}
@keyframes demo-cap-in {
  from { opacity: 0; transform: translate(-50%, 10px); }
  to { opacity: 1; transform: translate(-50%, 0); }
}

/* 收尾:logo + 名字 + slogan */
.demo-finale {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  background: rgba(23, 21, 19, 0.55);
  backdrop-filter: blur(3px);
  animation: demo-fin-in 0.8s ease both;
}
.demo-finale img { border-radius: 20px; box-shadow: 0 16px 44px rgba(0, 0, 0, 0.5); }
.demo-finale-name { font-size: 26px; font-weight: 700; color: #f5f4ef; letter-spacing: 0.5px; }
.demo-finale-slogan { font-size: 13.5px; color: #b0aaa2; }
@keyframes demo-fin-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

/* 假光标 + 点击涟漪 */
#demo-cursor {
  position: fixed;
  left: 0;
  top: 0;
  z-index: 1000;
  pointer-events: none;
  filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.5));
}
.demo-ripple {
  position: fixed;
  z-index: 999;
  width: 34px;
  height: 34px;
  margin: -17px 0 0 -17px;
  border-radius: 50%;
  border: 2px solid #d97757;
  pointer-events: none;
  animation: demo-ripple 0.5s ease-out both;
}
@keyframes demo-ripple {
  from { opacity: 0.9; transform: scale(0.25); }
  to { opacity: 0; transform: scale(1); }
}
```

- [ ] **Step 4: 写 `main.tsx`(demo 入口)**

```tsx
import ReactDOM from "react-dom/client";
import { detectHostOs } from "../platform";
import { installMocks } from "./mock";
import { buildScript } from "./script";
import { DemoStage } from "./DemoStage";
import "../styles.css";
import "./demo.css";

declare global {
  interface Window {
    __demo: { fps: number; frames: number; seek: (f: number) => Promise<void> };
  }
}

localStorage.clear(); // tab/pin 记忆清零,保证每次录制起点一致
installMocks();

(async () => {
  await detectHostOs(); // host_os → "windows":完整贴纸形态(含 pin/拖拽区)
  ReactDOM.createRoot(document.getElementById("root")!).render(<DemoStage />);
  const tl = buildScript();
  window.__demo = {
    fps: tl.fps,
    frames: Math.ceil((tl.duration + 0.4) * tl.fps),
    seek: (f) => tl.seek(f),
  };
})();
```

注意:`script.ts` 在 Task 5 才有,本任务先放一个最小占位(只摆 4 张卡、不排动作),Task 5 再充实:

```ts
// script.ts(占位版)
import { Timeline } from "./timeline";
import { store, notify } from "./mock";
import { makeSession } from "./data";

export function buildScript(): Timeline {
  const tl = new Timeline(12);
  store.sessions = [
    makeSession({ title: "重构吸边状态机", project: "larrygogo/meowo", activity: "▸ cargo clippy --workspace", ctx: 62, todoDone: 3, todoTotal: 5 }),
    makeSession({ title: "接入账号用量面板", project: "larrygogo/autopilot", activity: "▸ 编辑 src/views/Sticker.tsx", ctx: 41, todoDone: 1, todoTotal: 4 }),
    makeSession({ title: "升级 tauri 到 2.3", project: "larrygogo/cc-relay", status: "stale", agoMin: 12 }),
    makeSession({ title: "修复 statusline 兼容性", project: "larrygogo/clawmo-ios", status: "ended", connected: false, agoMin: 180 }),
  ];
  notify();
  tl.at(0.4, () => { store.stage.caption = "所有 Claude Code 会话,一眼看全"; notify(); });
  tl.at(5, () => {}); // 占位:确保 duration > 0
  return tl;
}
```

- [ ] **Step 5: 类型与现有测试不回归**

Run: `cd app && bunx tsc --noEmit && bunx vitest run`
Expected: 0 错误,全部测试通过。

- [ ] **Step 6: 目检静态舞台**

Run: `cd app && bun run dev`(保持运行),浏览器开 `http://localhost:1420/demo.html`
Expected: 渐变桌面上一扇带阴影的贴纸窗口、4 张卡(2 运行中徽标、1 绿点、1 虚线环)、底部字幕胶囊。检查后停掉 dev。
(代理执行时:用 Playwright 截 frame 0 的图目检,命令见 Task 6 的录制脚本,可先手跑一帧。)

- [ ] **Step 7: Commit**

```bash
git add app/src/demo/cursor.ts app/src/demo/DemoStage.tsx app/src/demo/demo.css app/src/demo/main.tsx app/src/demo/script.ts
git commit -m "feat(demo): 桌面舞台 + 假光标 + demo 入口(占位分镜)"
```

---

### Task 5: 分镜脚本

**Files:**
- Modify: `app/src/demo/script.ts`(替换占位版)

选择器约定(来自 `Sticker.tsx` 的现有 DOM):tab 为 `.tabs .stab:nth-child(n)`(1 全部 / 2 待交互 / 3 运行中 / 4 已归档);卡片为 `.stk-scroll .stk-card:nth-child(n)`;铅笔 `.stk-rename`;归档 `.stk-arch`;重命名输入框 `.stk-edit`。

- [ ] **Step 1: 写完整分镜**

```ts
import { Timeline } from "./timeline";
import { store, notify } from "./mock";
import { makeSession, Item } from "./data";
import { clickEl, moveToEl, typeText, pressKey, setCursor } from "./cursor";

function mut(fn: () => void): () => void {
  return () => {
    fn();
    notify();
  };
}

export function buildScript(): Timeline {
  const tl = new Timeline(12);
  const s1 = makeSession({ title: "重构吸边状态机", project: "larrygogo/meowo", activity: "▸ cargo clippy --workspace", ctx: 62, todoDone: 3, todoTotal: 5 });
  const s2 = makeSession({ title: "接入账号用量面板", project: "larrygogo/autopilot", activity: "▸ 编辑 src/views/Sticker.tsx", ctx: 41, todoDone: 1, todoTotal: 4 });
  const s3 = makeSession({ title: "升级 tauri 到 2.3", project: "larrygogo/cc-relay", status: "stale", agoMin: 12 });
  const s4 = makeSession({ title: "修复 statusline 兼容性", project: "larrygogo/clawmo-ios", status: "ended", connected: false, agoMin: 180 });
  store.sessions = [s1, s2, s3, s4];
  notify();
  setCursor(640, 520);

  // ── 场景 1(0–4.5s):实时变化 ──
  tl.at(0.4, mut(() => { store.stage.caption = "所有 Claude Code 会话,一眼看全"; }));
  tl.at(1.4, mut(() => { s1.current_activity = "▸ cargo test --workspace"; s1.context_pct = 63; }));
  tl.at(2.4, mut(() => { s2.current_activity = "▸ 运行 bunx vitest run"; s2.todo_done = 2; }));
  tl.at(3.4, mut(() => { s1.current_activity = "▸ 写入 src/snap/mod.rs"; s1.context_pct = 64; s1.todo_done = 4; }));

  // ── 场景 2(4.5–8.5s):转待交互 + tab 过滤 ──
  tl.at(4.6, mut(() => { store.stage.caption = "谁在等你回复,立刻知道"; }));
  tl.at(5.0, mut(() => {
    s2.session.status = "waiting";
    s2.current_activity = "等待回复:是否应用这 3 处修改?";
  }));
  moveToEl(tl, 5.4, 6.1, ".tabs .stab:nth-child(2)");
  tl.at(6.2, () => clickEl(".tabs .stab:nth-child(2)"));
  moveToEl(tl, 7.2, 7.7, ".tabs .stab:nth-child(1)");
  tl.at(7.8, () => clickEl(".tabs .stab:nth-child(1)"));

  // ── 场景 3(8.5–13s):重命名 + 归档 ──
  tl.at(8.7, mut(() => { store.stage.caption = "重命名、归档,即点即管"; }));
  moveToEl(tl, 8.8, 9.3, ".stk-scroll .stk-card:nth-child(2) .stk-rename");
  tl.at(9.4, () => clickEl(".stk-scroll .stk-card:nth-child(2) .stk-rename"));
  typeText(tl, 9.6, ".stk-edit", "评审用量面板方案", 11);
  tl.at(10.6, () => pressKey(".stk-edit", "Enter"));
  moveToEl(tl, 11.2, 11.8, ".stk-scroll .stk-card:nth-child(4) .stk-arch");
  tl.at(11.9, () => clickEl(".stk-scroll .stk-card:nth-child(4) .stk-arch"));

  // ── 场景 4(13–17.5s):吸边缩略 + 偷看 ──
  tl.at(13.2, mut(() => { store.stage.caption = "吸边缩成一根状态条,不占地方"; }));
  tl.at(13.4, mut(() => { store.stage.mode = "docking"; }));
  tl.at(14.2, mut(() => { store.stage.mode = "strip"; }));
  moveToEl(tl, 14.6, 15.2, ".demo-window .cstrip");
  tl.at(15.4, mut(() => { store.stage.mode = "expanded"; }));
  tl.at(16.0, () => setCursor(500, 300)); // 光标移开
  tl.at(16.8, mut(() => { store.stage.mode = "strip"; }));

  // ── 场景 5(17.5–20s):收尾 ──
  tl.at(17.6, mut(() => { store.stage.caption = null; store.stage.finale = true; }));
  tl.at(19.6, () => {}); // 钉住总时长 ≈ 20s
  return tl;
}
```

- [ ] **Step 2: 类型检查**

Run: `cd app && bunx tsc --noEmit`
Expected: 0 错误(`Item` 未用则删掉该 import)。

- [ ] **Step 3: 浏览器粗验(可选,与 Task 6 录制目检二选一)**

dev server 下打开 `/demo.html`,Console 执行 `for (let f=0; f<240; f++) await window.__demo.seek(f)` walkthrough 一遍,不应有 `[demo] 未找到` 警告。

- [ ] **Step 4: Commit**

```bash
git add app/src/demo/script.ts
git commit -m "feat(demo): 完整分镜(实时变化/待交互过滤/重命名归档/吸边偷看/收尾)"
```

---

### Task 6: 录制管线与 GIF 产出

**Files:**
- Create: `app/scripts/record-demo.mjs`
- Modify: `app/package.json`(scripts 加 `"demo:gif": "bun scripts/record-demo.mjs"`)
- Replace: `docs/images/demo.gif`

编码策略:全局调色板(均匀采样若干帧合并量化,255 色 + 1 透明位)+ 帧间差分(与上帧相同的像素写透明索引,dispose=1),控制体积;噪点/渐变是静态的,差分后近乎免费。

- [ ] **Step 1: 写 `record-demo.mjs`**

```js
// 录制 demo.gif:起独立端口的 vite → Playwright 逐帧 seek+截图 → gifenc 编码。
// 用法:cd app && bun run demo:gif   (产物写到 ../docs/images/demo.gif)
import { spawn } from "node:child_process";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { GIFEncoder, quantize, applyPalette } from "gifenc";
import { decode } from "fast-png";

const appDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outPath = resolve(appDir, "../docs/images/demo.gif");
const PORT = 14210;
const W = 880, H = 560;
const DELAY_MS = 83; // ≈12fps
const TRANSPARENT = 255; // 调色板末位留给「与上帧相同」

const vite = spawn("bunx", ["vite", "--port", String(PORT), "--strictPort"], {
  cwd: appDir,
  stdio: "pipe",
  shell: true,
});
try {
  await waitFor(`http://localhost:${PORT}/demo.html`);
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: W, height: H }, deviceScaleFactor: 1 });
  page.on("console", (m) => {
    if (m.text().includes("[demo]")) console.warn("页面警告:", m.text());
  });
  await page.goto(`http://localhost:${PORT}/demo.html`);
  await page.waitForFunction(() => !!window.__demo, null, { timeout: 15000 });
  const frames = await page.evaluate(() => window.__demo.frames);
  console.log(`录制 ${frames} 帧 @${W}x${H}`);

  // 第一遍:逐帧 seek + 截图,RGBA 存内存(880*560*4*240 ≈ 470MB,可接受;若内存吃紧改两遍渲染)
  const rgbaFrames = [];
  for (let f = 0; f < frames; f++) {
    await page.evaluate((n) => window.__demo.seek(n), f);
    const png = await page.screenshot({ type: "png" });
    const { data, width, height } = decode(png);
    if (width !== W || height !== H) throw new Error(`帧尺寸异常: ${width}x${height}`);
    rgbaFrames.push(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
    if (f % 24 === 0) console.log(`  ${f}/${frames}`);
  }
  await browser.close();

  // 全局调色板:每 12 帧采样合并量化(255 色),末位补占位色给透明索引
  const samples = [];
  for (let f = 0; f < rgbaFrames.length; f += 12) samples.push(rgbaFrames[f]);
  const merged = new Uint8Array(samples.length * samples[0].length);
  samples.forEach((s, i) => merged.set(s, i * s.length));
  const palette = quantize(merged, 255);
  while (palette.length < 256) palette.push([0, 0, 0]);

  const gif = GIFEncoder();
  let prev = null;
  for (const rgba of rgbaFrames) {
    const index = applyPalette(rgba, palette);
    if (!prev) {
      gif.writeFrame(index, W, H, { palette, delay: DELAY_MS });
    } else {
      const diff = new Uint8Array(index);
      for (let i = 0; i < index.length; i++) if (index[i] === prev[i]) diff[i] = TRANSPARENT;
      gif.writeFrame(diff, W, H, {
        palette,
        delay: DELAY_MS,
        transparent: true,
        transparentIndex: TRANSPARENT,
        dispose: 1,
      });
    }
    prev = index;
  }
  gif.finish();
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, gif.bytes());
  console.log(`完成:${outPath} (${(gif.bytes().length / 1024 / 1024).toFixed(2)} MB)`);
} finally {
  vite.kill();
}

async function waitFor(url) {
  for (let i = 0; i < 60; i++) {
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {
      /* vite 未就绪 */
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("vite dev server 启动超时");
}
```

- [ ] **Step 2: package.json 加脚本**

`app/package.json` 的 `scripts` 增加:`"demo:gif": "bun scripts/record-demo.mjs"`。

- [ ] **Step 3: 录制**

Run: `cd app && bun run demo:gif`
Expected: 输出 `完成:...demo.gif (x.xx MB)`,体积 < 4MB。

- [ ] **Step 4: 目检 GIF**

用 Read 工具看 `docs/images/demo.gif`(展示首帧),并抽查关键时刻——临时在录制脚本里对 f∈{0, 60, 78, 120, 145, 170, 186, 228} 各存一张 PNG 到 `%TEMP%/cc-demo-frames/` 逐张 Read 目检:
- 文字清晰可读、无明显色带/失真
- 场景 2 tab 过滤后只剩待交互卡
- 场景 3 输入框出现且文字逐字递增、归档后卡片消失
- 场景 4 竖条 3 个色点、expanded 形态正常
- 光标位置与点击目标吻合

不达标按「分镜时间/坐标 → demo.css 视觉 → 帧率/尺寸」顺序调参重录(每轮重录后重新目检)。

- [ ] **Step 5: Commit**

```bash
git add app/scripts/record-demo.mjs app/package.json docs/images/demo.gif
git commit -m "feat(demo): Playwright+gifenc 录制管线,重制 demo.gif(差分编码,体积可控)"
```

---

### Task 7: logo 导出与 README 重写

**Files:**
- Create: `docs/images/logo.png`
- Rewrite: `README.md`

- [ ] **Step 1: 导出 logo**

```bash
cp app/src-tauri/icons/128x128@2x.png docs/images/logo.png
```

- [ ] **Step 2: 重写 README.md**

整体结构(内容沿用现有文案,只重新组织;居中头部用纯 HTML——GFM 不渲染 HTML 块内的 markdown):

````markdown
<div align="center">
  <img src="docs/images/logo.png" width="104" alt="Meowo logo" />
  <h1>Meowo</h1>
  <p><b>一个常驻桌面的「贴纸」,你所有 Claude Code 会话的进度,一眼看全。</b></p>
  <p>
    <a href="https://github.com/larrygogo/meowo/actions/workflows/ci.yml"><img src="https://github.com/larrygogo/meowo/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/larrygogo/meowo/releases/latest"><img src="https://img.shields.io/github/v/release/larrygogo/meowo?label=release&color=d97757" alt="Release" /></a>
    <a href="https://github.com/larrygogo/meowo/releases"><img src="https://img.shields.io/github/downloads/larrygogo/meowo/total?color=4ec9a5" alt="Downloads" /></a>
    <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS-555" alt="Platform" />
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT" /></a>
  </p>
  <p>哪个在跑、哪个在等你回复、各自做到哪一步——通过 Claude Code hooks 捕获事件,落进本地 SQLite,<br/>再用一个半透明、可吸边的 Tauri 小窗口实时呈现。无需切来切去找终端。</p>
  <img src="docs/images/demo.gif" alt="Meowo 演示:实时会话贴纸、待交互提醒、重命名归档、吸边缩略" width="760" />
</div>

## 下载

| 平台 | 安装包 | 说明 |
|------|--------|------|
| **Windows** | [cc-kanban_0.2.0_x64-setup.exe](https://github.com/larrygogo/meowo/releases/download/v0.2.0/cc-kanban_0.2.0_x64-setup.exe) | NSIS 安装包 |
| **macOS** | [cc-kanban_0.2.0_universal.dmg](https://github.com/larrygogo/meowo/releases/download/v0.2.0/cc-kanban_0.2.0_universal.dmg) | universal(Intel / Apple Silicon 通用),需 ≥ 14 Sonoma;已签名公证,双击安装直接打开 |

也可到 [Releases](https://github.com/larrygogo/meowo/releases/latest) 获取最新版本。装好后支持应用内(设置 → 关于)检查更新,两个平台均可自动升级到后续版本。

## 特性

### 📌 实时会话看板
…(分组重排现有 21 条,见 Step 2 详述)
````

特性分组映射(现有 bullet → 新分组,文字原样或仅适配上下文微调):

| 新分组 | 收纳的现有条目 |
|---|---|
| 📌 实时会话看板 | 实时会话贴纸 / 状态分类 tab / 状态指示 / 空态引导 / 首次导入 |
| 🚀 点击直达终端 | 点击跳转·恢复 / 恢复终端可选 |
| 🔔 不漏掉任何等待 | 错误提醒 / 桌面通知 |
| 🗂 卡片即点即管 | 会话重命名 / 归档收纳 |
| 🧲 吸边与窗口(仅 Windows) | 吸边缩略 / 窗口置顶 / 位置·尺寸记忆 |
| 🍎 菜单栏面板(仅 macOS) | macOS 菜单栏面板 + 「平台差异」「macOS 权限」两段合并为本组说明 |
| 🎨 外观与系统集成 | 外观自定义 / 系统托盘·菜单栏(开机自启) |
| 📊 账号与用量 | 账号与用量 |
| 🔌 零配置接入 | 零配置接入 |

每组 3-5 行内说明;「工作原理 / 项目结构 / 环境要求 / 快速开始 / 接入 Claude Code / 数据与配置 / 测试 / 路线 / License」九个章节文字保持现状,仅:
- 「平台差异」独立章节取消,并入 🍎 分组(`<details>` 折叠 macOS 权限说明)。
- 「接入 Claude Code」的手动挂 hooks 长说明包进 `<details><summary>手动挂 hooks(可选)</summary>…</details>`。
- 「数据与配置」整节包进 `<details><summary>数据与配置文件位置</summary>…</details>`。

- [ ] **Step 3: 本地渲染检查**

用 IDE/`gh` 预览或推送后看 GitHub 渲染:居中头部、徽章、表格、折叠块、GIF 都正常;全文搜索确认无丢内容(对照旧版 21 条 bullet 逐条点名)。

- [ ] **Step 4: Commit**

```bash
git add docs/images/logo.png README.md
git commit -m "docs(readme): 全面视觉升级——居中头部+徽章组+分组特性区+新演示 GIF"
```

---

### Task 8: 全量验证与收尾

- [ ] **Step 1: 前端全量验证**

Run: `cd app && bunx tsc --noEmit && bunx vitest run && bun run build`
Expected: 类型 0 错误、测试全过、`dist/` 无 demo 产物。

- [ ] **Step 2: Rust 侧无回归(未触碰,跑一遍兜底)**

Run: `cargo clippy --workspace -- -D warnings`
Expected: 通过。

- [ ] **Step 3: 确认 demo.gif 体积**

Run: `ls -la docs/images/demo.gif`
Expected: < 4MB。

- [ ] **Step 4: 收尾**

使用 superpowers:finishing-a-development-branch,推分支开 PR(标题/描述中文,含变更摘要 + 测试计划 + GIF 预览)。

---

## Self-Review

- **Spec 覆盖**:合成管线(Task 1-6)、一键重生成(`demo:gif`)、确定性(timeline 钉动画时间)、不进生产构建(Task 1 Step 3 + Task 8 Step 1 验证)、README 八分组+徽章+表格+折叠(Task 7)、验收标准(Task 6 Step 3-4、Task 8)——全部有任务对应。
- **占位符**:Task 4 的 script.ts 明确标注「占位版」且 Task 5 给出完整替换代码;Task 7 README 给出头部完整 HTML + 分组映射表(正文文字沿用现有 README,映射表即完整指引),无 TBD。
- **类型一致性**:`store.stage.mode` 的四态与 demo.css 的 `.demo-mode-*` 一致;`buildScript` 返回 `Timeline` 与 main.tsx 调用一致;mock 命令名与 `Sticker.tsx`/`api.ts` 的 invoke 名核对一致(rename_session 参数 cwd/sessionId/title;set_archived 参数 sessionId/archived)。
