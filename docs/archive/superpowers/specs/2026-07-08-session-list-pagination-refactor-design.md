# 会话列表：虚拟列表 + 分页重构 — 设计

日期：2026-07-08
分支：feat/new-session-20260706（续用）
状态：已通过头脑风暴，待实现

## 背景与动机

深度排查「虚拟列表 + 动态加载」发现 4 类问题（症状：「全部」列表/搜索看不全）：

- **P0 board-changed 重置分页**：`refresh()` = `loadPage(filter, null)`，cursor=null 时整体替换成第一页 100 条，丢弃已滚动加载的后续页（App.tsx:167-173/193-196）。`listen("board-changed", () => refresh())`（App.tsx:231），而 board-changed 由 board.db 写监听器去抖发出（lib.rs:1623-1708）+ 周期轮询，有活动会话就频繁触发 → 滚动加载后被反复打回 100。
- **P0 搜索只搜「已加载」的会话**：搜索是客户端 `data.filter`（Sticker.tsx:775-787），`data` 仅已加载 ≤N 条 → 未加载的匹配项搜不到。
- **P1 loadMore 触发依赖 `shown` 变化**：触发 effect（Sticker.tsx:822-828）靠 `virtualItems`/`shown.length` 变化重跑；若一页新 data 经过滤后 `shown` 不增（搜索态最易踩）→ effect 不再触发 → 分页卡死。
- **P2 counts 与 list 口径不一致**：`live_sessions_counts.total = COUNT(*)` 含 archived；Sticker `counts.all = countsProp.total` 错（虽当前 all/archived tab 不显示角标未暴露）；且 loadMore 期间不刷新 counts，分页中增删会让 `totalFor` 短暂失真。

**根本判断**：三套机制冲突——keyset 分页 / 每次 DB 变更整体 refresh / 客户端在「已加载数据」上搜索过滤排序。只要保留「board-changed 整体重置」分页就被打回；只要搜索/排序只在已加载数据上做就不全。

## 目标 / 非目标

**目标**
- 「全部」及各 tab 列表滚动能加载到全部、board-changed 不再打回第一页。
- 搜索在**当前 tab 内**搜全库（下沉后端），不只搜已加载。
- 消除 loadMore 卡死与 counts 口径不一致。

**非目标（本次不含）**
- P3 虚拟测量估高抖动（`estimateSize` 固定）——UX 打磨，单独小任务。
- starred 置顶下沉后端（仍客户端浮顶，见决策）。
- 分页并发/竞态守卫的进一步加强（现有 `refreshSeqRef` 保留）。

## 关键设计决策

| 点 | 决策 |
|----|------|
| **搜索作用域** | 搜**当前 tab 内**（= tab 的 filter 条件 AND 搜索词）。运行中→只搜 running；待交互→只搜 waiting；全部→非归档；归档→归档。 |
| **分页驱动** | 由 `reachedEnd` 驱动（`page.length < PAGE_SIZE → reachedEnd`），`hasMore = !reachedEnd`。counts 只用于角标显示，**不参与** loadMore 判定 → P1/P2 一并消。 |
| **board-changed refresh** | 重查前 `W = max(PAGE_SIZE, 已加载数)` 条（cursor=null、limit=W）整体替换 → 保住已加载窗口 + 重排 + 反映更新（解 P0）。前端**节流 ~400ms**。 |
| **排序** | 下沉后端：非 waiting tab `(last_event_at DESC, id DESC)`；waiting tab `ASC`（等最久优先，把现在客户端的 waiting 排序也搬下来，游标方向随之翻转）。 |
| **starred 置顶** | 仍客户端在**已加载窗口内**浮顶（不下沉后端，行为同现在、不回归）。 |
| **搜索状态归属** | 搜索词 `query` 从 Sticker **提升到 App**（App 驱动 loadPage）；搜索框 UI（开合）留在 Sticker，值走 props。 |
| **counts 随搜索** | 不变（角标=各 tab 总数，搜索只筛当前视图）。顺手修 `counts.all = total - archived`。 |

## 后端 `crates/meowo-store/src/query.rs`

### `live_sessions` 加 `search` 参数
```
pub fn live_sessions(
    &self,
    filter: Option<&str>,
    search: Option<&str>,     // 新增：非空则在 filter 基础上 AND 搜索词
    before_last_event_at: Option<i64>,
    before_id: Option<i64>,
    limit: usize,
) -> Result<Vec<LiveSession>, StoreError>
```
- **search 条件**（search 去空白后非空时追加一条 condition，与 filter 用 AND）：
  `(t.title LIKE ?e OR s.cwd LIKE ?e OR p.name LIKE ?e)`，参数为 `%<escaped>%`。
  转义 `%` `_` `\`（`LIKE … ESCAPE '\'`）。SQLite LIKE 对 ASCII 默认大小写不敏感、CJK 无大小写，够用。
- **排序 + 游标方向按 filter**：
  - waiting：`ORDER BY s.last_event_at ASC, s.id ASC`，游标 `(last_event_at > ? OR (last_event_at = ? AND id > ?))`。
  - 其它（all/running/archived）：`ORDER BY s.last_event_at DESC, s.id DESC`，游标 `(last_event_at < ? OR (last_event_at = ? AND id < ?))`。
- 参数拼接顺序须与占位符一致（search 参数、游标参数、limit）。保留 4035ec5 的「游标条件整体括号」。

### `live_sessions_counts` 不变（display-only）

### 调用方连带
- `live_sessions_blocking`（lib.rs:183）+ `get_live_sessions_page` 命令（lib.rs:137）线程 `search` 参数。
- lib.rs:1954 内部调用 `store.live_sessions(Some("all"), None, None, 1000)` → 补 `None` search。

## 后端命令 `app/src-tauri/src/lib.rs`

- `get_live_sessions_page` 命令签名加 `search: Option<String>`，透传到 `live_sessions_blocking` → `live_sessions`。
- `live_sessions_blocking` 签名加 `search: Option<&str>`。

## 前端 `app/src/api.ts`

- `getLiveSessionsPage(filter, search, cursor, limit)` 加 `search: string | null` 参数，`invoke("get_live_sessions_page", { filter, search, ... })`。

## 前端 `app/src/App.tsx`

- 新增 `search` 状态（App 拥有）；`filter`、`search` 变化都重置分页并加载首页。
- `loadPage(filter, search, cursor)`：调 `getLiveSessionsPage(filter, search, cursor, limit)`。
  - **limit 分两种**：普通首页/切 tab/切 search = `PAGE_SIZE`；board-changed refresh = `W = max(PAGE_SIZE, items.length)`。
- `loadMore`：带当前 `search`，cursor=末条；**守卫改 `reachedEnd`**（去掉 `items.length >= totalFor` 判定）。
- `refresh`（board-changed）：`loadPage(filter, search, null)` 但 limit=W；**节流 ~400ms**（trailing）。
- `hasMore = !reachedEnd`（传给 Sticker），不再依赖 counts。
- 把 `search` + `onSearchChange`（setSearch，内部 debounce ~300ms 再触发 loadPage）传给 Sticker。counts 仍传（角标用）。

## 前端 `app/src/views/Sticker.tsx`

- 搜索框 `value` 用 `search` prop、`onChange` 调 `onSearchChange`（开合 `searchOpen` 留本地 UI 态）。**关闭搜索框 = `onSearchChange("")`**（清空 App 搜索并重载首页）。
- `shown`：**去掉客户端搜索过滤**（后端做）。保留 `match(tab)` 作为**切换瞬间的防串档安全网**（稳态下 data 已是该 tab、`match` 全 true 不过滤、不再引发 P1 卡死）；去掉 waiting 的客户端 ASC 重排（后端已 ASC）；保留 starred 客户端浮顶。
- `counts.all = countsProp.total - countsProp.archived`。
- loadMore 触发逻辑不变（稳态 `shown=data`，不再卡死）。

## 边界与测试

- **切 tab / 改 search**：cursor=null 首页替换 + `reachedEnd=false`。切换瞬间旧 tab 数据由 `match` 安全网挡住不串档。
- **board-changed**：节流后重查 W 窗口整体替换，`reachedEnd` 按 `返回数 < W` 重算。
- **搜索无匹配**：后端返回空页 → `reachedEnd=true` → 空态提示。
- **Rust 单测**（query.rs）：`live_sessions` 加 search — 覆盖 (1) 各 tab filter AND search 命中/不命中；(2) `%`/`_` 转义不被当通配；(3) waiting ASC 排序 + 游标翻转分页无重复无遗漏；(4) search=None 与旧行为一致。
- **前端测试**：App loadPage/loadMore/refresh 分页（reachedEnd 驱动、W 窗口 refresh 不丢已加载、搜索重载）；Sticker 搜索框走 props、`shown` 不再客户端搜索、counts.all 口径。
- **手动验证**：全部 tab 滚到底加载到 1235、board-changed（有活动会话）不打回；搜索能命中未加载的会话；待交互按等最久优先。

## 影响文件

- `crates/meowo-store/src/query.rs`：`live_sessions` 加 search + 排序/游标按 filter + 单测。
- `app/src-tauri/src/lib.rs`：`get_live_sessions_page` 命令 + `live_sessions_blocking` 加 search；补 1954 caller。
- `app/src/api.ts`：`getLiveSessionsPage` 加 search。
- `app/src/App.tsx`：search 状态 + loadPage/loadMore/refresh（reachedEnd 驱动、W 窗口、节流）+ 传 props。
- `app/src/views/Sticker.tsx`：搜索框走 props、`shown` 去客户端搜索、counts.all 口径、去 waiting 客户端重排。
- 测试：query.rs 单测；App/Sticker 前端测试。
