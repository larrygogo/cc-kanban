# 模块 D：空态美化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Sticker 空列表的 `（空）` 占位替换为按 tab 区分的居中图标 + 主文案 +（部分）提示文案。

**Architecture:** 新增纯展示组件 `EmptyState({ tab })`，根据当前 tab 查表渲染 lucide 风格内联 SVG 图标、主文案与可选提示。`Sticker` 在 `shown.length === 0` 时把当前 `tab` 传给它。无新增状态、无副作用。

**Tech Stack:** React 18 + TypeScript + Vite，测试用 vitest + @testing-library/react（jsdom 环境，已配置）。

设计来源：`docs/superpowers/specs/2026-06-04-release-and-polish-design.md` 模块 D。

---

### Task 1: EmptyState 组件（含失败测试）

**Files:**
- Modify: `app/src/views/Sticker.tsx`（新增并导出 `EmptyState`，在空列表分支调用）
- Test: `app/src/views/Sticker.test.tsx`（更新既有空态断言 + 新增四 tab 断言）

> 背景：`Sticker.test.tsx:23-27` 现有用例 `空数据显示（空）` 断言 `（空）` 文案存在。本模块删除该文案，所以该用例必须改写，否则会红。`EmptyState` 需 `export`（非 default）以便直接单测四个 tab，无需点击切换 tab（避免 localStorage 在 jsdom 跨用例污染）。

- [ ] **Step 1: 改写既有空态用例 + 新增四 tab 用例（失败测试）**

把 `app/src/views/Sticker.test.tsx` 顶部的 import 改为同时引入 `EmptyState`：

```tsx
import { Sticker, EmptyState } from "./Sticker";
```

将原 `it("空数据显示（空）", ...)`（约 23-27 行）整体替换为下面这一条（断言改为 all tab 的新主文案）：

```tsx
  it("空数据显示 all 空态主文案", () => {
    const { container } = render(<Sticker data={[]} />);
    expect(screen.getByText("还没有会话")).toBeTruthy();
    expect(container.querySelector("[data-tauri-drag-region]")).toBeTruthy();
  });
```

在 `describe("Sticker", ...)` 块的末尾（最后一个 `it` 之后、`});` 之前）追加：

```tsx
  it.each([
    ["all", "还没有会话", "在终端运行 Claude Code，进度会自动出现在这里"],
    ["waiting", "没有等待交互的会话", "有会话需要你回复时会出现在这里"],
    ["running", "当前没有运行中的会话", null],
    ["archived", "没有归档的会话", "点卡片右上角按钮可收纳会话"],
  ] as const)("EmptyState[%s] 渲染主文案与提示", (tab, title, hint) => {
    render(<EmptyState tab={tab} />);
    expect(screen.getByText(title)).toBeTruthy();
    if (hint) {
      expect(screen.getByText(hint)).toBeTruthy();
    }
  });

  it("EmptyState[running] 不渲染提示文案", () => {
    const { container } = render(<EmptyState tab="running" />);
    expect(container.querySelector(".stk-empty-hint")).toBeNull();
  });
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cd app && bunx vitest run src/views/Sticker.test.tsx`
Expected: FAIL —— `EmptyState` 未从 `./Sticker` 导出（编译/导入错误），或新文案找不到。

- [ ] **Step 3: 实现 EmptyState 组件并接入 Sticker**

在 `app/src/views/Sticker.tsx` 中，于 `function match(...)` 之后、`export function Sticker(...)` 之前，新增以下内容（`EMPTY` 配置表 + `EmptyIcon` + `EmptyState`）：

```tsx
const EMPTY: Record<Tab, { title: string; hint: string | null }> = {
  all: { title: "还没有会话", hint: "在终端运行 Claude Code，进度会自动出现在这里" },
  waiting: { title: "没有等待交互的会话", hint: "有会话需要你回复时会出现在这里" },
  running: { title: "当前没有运行中的会话", hint: null },
  archived: { title: "没有归档的会话", hint: "点卡片右上角按钮可收纳会话" },
};

function EmptyIcon({ tab }: { tab: Tab }) {
  const common = {
    width: 28, height: 28, viewBox: "0 0 24 24", fill: "none",
    stroke: "currentColor", strokeWidth: 1.6, strokeLinecap: "round",
    strokeLinejoin: "round", "aria-hidden": true,
  } as const;
  switch (tab) {
    case "all": // 显示器
      return (
        <svg {...common}>
          <rect width="20" height="14" x="2" y="3" rx="2" />
          <line x1="8" y1="21" x2="16" y2="21" />
          <line x1="12" y1="17" x2="12" y2="21" />
        </svg>
      );
    case "waiting": // 对话气泡
      return (
        <svg {...common}>
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
        </svg>
      );
    case "running": // 播放
      return (
        <svg {...common}>
          <polygon points="6 3 20 12 6 21 6 3" />
        </svg>
      );
    case "archived": // 归档盒
      return (
        <svg {...common}>
          <rect width="20" height="5" x="2" y="3" rx="1" />
          <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8" />
          <path d="M10 12h4" />
        </svg>
      );
  }
}

export function EmptyState({ tab }: { tab: Tab }) {
  const { title, hint } = EMPTY[tab];
  return (
    <div className="stk-empty">
      <span className="stk-empty-icon"><EmptyIcon tab={tab} /></span>
      <div className="stk-empty-title">{title}</div>
      {hint && <div className="stk-empty-hint">{hint}</div>}
    </div>
  );
}
```

然后把 `Sticker` 中空列表分支（当前约 126-128 行）：

```tsx
        {shown.length === 0 ? (
          <div className="stk-empty">（空）</div>
        ) : (
```

替换为：

```tsx
        {shown.length === 0 ? (
          <EmptyState tab={tab} />
        ) : (
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cd app && bunx vitest run src/views/Sticker.test.tsx`
Expected: PASS（含新增的 4 条 `it.each` 与 running 无提示用例）。

- [ ] **Step 5: 提交**

```bash
git add app/src/views/Sticker.tsx app/src/views/Sticker.test.tsx
git commit -m "feat(app): 空态按 tab 渲染图标+主文案+提示(EmptyState 组件)"
```

---

### Task 2: 空态样式

**Files:**
- Modify: `app/src/styles.css:141-145`（`.stk-empty` 规则块）

> 现有 `.stk-empty`（141-145 行）是 `font-size:12px; color:faint; padding:6px 2px;`。改为纵向居中 flex 容器，并新增三个子样式。注意 `.stk-empty` 的容器是 `.stk-scroll`（`flex:1; overflow-y:auto`），所以用 `min-height:100%` 让空态在可用高度内垂直居中。

- [ ] **Step 1: 替换 `.stk-empty` 规则并新增子样式**

将 `app/src/styles.css` 中这段（141-145 行）：

```css
.stk-empty {
  font-size: 12px;
  color: var(--cc-text-faint);
  padding: 6px 2px;
}
```

替换为：

```css
.stk-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  min-height: 100%;
  padding: 24px 16px;
  text-align: center;
}
.stk-empty-icon {
  color: var(--cc-text-faint);
  display: inline-flex;
}
.stk-empty-title {
  font-size: 13px;
  color: var(--cc-text-dim);
}
.stk-empty-hint {
  font-size: 11px;
  color: var(--cc-text-faint);
  line-height: 1.5;
  max-width: 220px;
}
```

- [ ] **Step 2: 类型检查 + 跑前端测试套件**

Run: `cd app && bunx tsc --noEmit && bunx vitest run`
Expected: tsc 无输出（通过）；vitest 全绿（含 `api.test.ts`、`App.test.tsx`、`LiveView.test.tsx`、`Sticker.test.tsx`）。

- [ ] **Step 3: 提交**

```bash
git add app/src/styles.css
git commit -m "style(app): 空态容器纵向居中+图标/主文案/提示样式"
```

---

## 自检（Self-Review）

- **Spec 覆盖**：模块 D 表格 4 个 tab 的图标/主文案/提示 → Task 1 的 `EMPTY` 表与 `EmptyIcon` 全部覆盖；`.stk-empty` 改 flex 居中 + 新增 `.stk-empty-icon/-title/-hint` → Task 2 覆盖；vitest 四 tab 断言 → Task 1 Step 1 覆盖。
- **占位扫描**：无 TBD / “适当处理” 类占位，每个代码步骤均含完整代码。
- **类型一致性**：`Tab` 类型沿用 `Sticker.tsx` 已有定义（`"all"|"waiting"|"running"|"archived"`）；`EmptyState` 的 props 形状 `{ tab: Tab }` 在测试与实现中一致；`EMPTY` 用 `Record<Tab, ...>` 保证四个 key 齐全。
- **回归风险**：既有 `（空）` 文案断言已在 Task 1 Step 1 改写，不会遗留红用例。

## 非目标

- 不改动卡片渲染、tab 切换逻辑或数据流。
- 不引入 lucide-react 依赖（保持内联 SVG，与现有 `TabIcon`/`ArchiveIcon` 风格一致）。
