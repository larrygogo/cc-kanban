// 会话行的结构共享。board-changed 常是「空转」：命令写库后 db-watcher 又为同一次写入报一次、
// liveness 轮询重发、甚至读库触碰 board.db-wal/-shm 的 mtime 也会被 watcher 当成变更而回声。
// 这些刷新拉回的数据与当前完全一致，若照旧整表替换成新对象引用，整个列表都会无谓重渲染。
//
// 行级 JSON 缓存 + 结构共享：每行 stringify 一次、与上次缓存比对，没变的行**复用旧对象引用**。
// 于是（1）空转刷新整表引用不变（sameRefs 命中，跳过 setState）；（2）只有一行变化时，其余行
// 引用稳定，配合卡片层的 memo 只重渲染那一张。全字段 JSON 比较是刻意的**排除法**——按字段挑
// 白名单的签名方案会在漏字段时让 UI 静默不更新。
//
// 缓存只增不清：键是 session.id，量级为窗口生命周期内见过的会话数，几百条小对象，不值得管理。
//
// 看板（App.tsx）与对话窗侧栏（ChatSidebar.tsx）共用这一份：侧栏此前没有这层，滚动加载到
// 几百条后，每次 board-changed 都要「整表重取 + 重建几百个卡片元素 + 全表 DOM reconcile」，
// 而节流后的 board-changed 在多会话齐跑时基本是持续触发的。
export type SessionRow = { session: { id: number } };
export type RowCache<T extends SessionRow> = Map<number, { json: string; item: T }>;

export const reconcileRows = <T extends SessionRow>(cache: RowCache<T>, rows: T[]): T[] =>
  rows.map((row) => {
    const id = row.session.id;
    const json = JSON.stringify(row);
    const hit = cache.get(id);
    if (hit && hit.json === json) return hit.item;
    cache.set(id, { json, item: row });
    return row;
  });

/// 行已结构共享，引用比较即语义比较。
export const sameRefs = <T>(a: T[], b: T[]): boolean =>
  a.length === b.length && a.every((x, i) => x === b[i]);
