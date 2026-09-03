/**
 * 回归测试：贴纸看板空闲期不再持续刷新。
 *
 * 背景：WAL 模式下 meowo-app 每次读库都新开连接触碰 board.db-shm，曾被 db-watcher 误判为
 * 变更而发 board-changed，形成 read→watcher→refresh→read 的自持循环，空闲时贴纸列表一直跳。
 * 修复：db-watcher 用持久连接 + PRAGMA data_version 门控，只有别的连接真正提交写入才发事件；
 * 并把 -shm 事件从监听中排除。前端另加结构相等守卫兜底。
 *
 * 观测点：App 的 board-changed 监听器在 VITE_E2E 构建下把收到的次数累计到
 * window.__MEOWO_BOARD_CHANGED__。空闲若干秒内该计数应几乎不增长。
 */
describe("贴纸看板：空闲刷新回归", () => {
  it("空闲 6 秒内 board-changed 不再持续累计", async () => {
    // 等前端挂载：监听器 effect 运行时把计数初始化为 0（仅 E2E 构建）。
    try {
      await browser.waitUntil(async () => (await readCount()) !== null, { timeout: 30_000 });
    } catch {
      // 失败时连窗口清单一起抛：injected 全 false = 构建没带 VITE_E2E；
      // 某个窗口 injected=true 却没被选中 = 遍历/切窗这层还有洞。
      throw new Error(
        `30s 内未检测到 board-changed 观测计数（该二进制是否以 VITE_E2E=1 + --features e2e 构建？）${await describeWindows()}`,
      );
    }

    // 首屏加载会合法地发若干次 board-changed（首次导入 / 首轮 liveness）；等其平静下来。
    await waitIdle();

    const before = (await readCount()) ?? 0;
    // 空闲观察 6 秒。修复后 db-watcher 仅在真实写入时才发事件，纯空闲应几乎为 0。
    await browser.pause(6_000);
    const after = (await readCount()) ?? 0;

    // 阈值给到 2：容忍 liveness 5s 轮询在存活集变化时至多 1 次合法刷新及边界抖动。
    // 修复前此处会是几十次（每 1~2s 一次）。
    expect(after - before).toBeLessThanOrEqual(2);
  });
});

/** 在**当前**窗口读观测计数；未注入时返回 null。 */
function probeCount(): Promise<number | null> {
  return browser.execute(() => {
    const w = window as unknown as { __MEOWO_BOARD_CHANGED__?: number };
    return typeof w.__MEOWO_BOARD_CHANGED__ === "number" ? w.__MEOWO_BOARD_CHANGED__ : null;
  });
}

/**
 * 读观测计数，必要时**遍历全部窗口**去找注入了观测点的那个。
 *
 * 起因：app 启动期可能不止一个 webview（首启的引导窗是独立窗口），而 WDIO 的会话落在
 * 哪个句柄上不由我们决定。观测点只由贴纸主窗的 useBoardRefresh 注入，落错窗就永远读到
 * null——CI 上表现为「30s 等不到计数」，且时灵时不灵（同一份代码 run 33722782537 绿、
 * 33726651450 红）。原先只读当前句柄，等于把结果押在驱动的默认落点上。
 *
 * 命中后 switchToWindow 把会话留在正确的窗口上，后续读取不再遍历。
 */
async function readCount(): Promise<number | null> {
  const here = await probeCount();
  if (here !== null) return here;
  const current = await browser.getWindowHandle();
  for (const handle of await browser.getWindowHandles()) {
    if (handle === current) continue;
    await browser.switchToWindow(handle);
    const found = await probeCount();
    if (found !== null) return found;
  }
  // 一个都没命中：切回原窗，让诊断看到的是驱动的默认落点。
  await browser.switchToWindow(current);
  return null;
}

/** 超时诊断：列出每个窗口的 URL/标题/是否注入观测点，好一眼分辨是「落错窗」
 *  还是「整个构建就没带 VITE_E2E」。失败路径专用，不影响正常流程。 */
async function describeWindows(): Promise<string> {
  const LF = "\n";
  try {
    const current = await browser.getWindowHandle();
    const rows: string[] = [];
    for (const handle of await browser.getWindowHandles()) {
      await browser.switchToWindow(handle);
      const info = await browser.execute(() => ({
        url: location.href,
        title: document.title,
        injected:
          typeof (window as unknown as { __MEOWO_BOARD_CHANGED__?: number })
            .__MEOWO_BOARD_CHANGED__ === "number",
        mounted: !!document.querySelector("#root")?.firstElementChild,
      }));
      rows.push(
        `    [${handle}] injected=${info.injected} mounted=${info.mounted} title=${JSON.stringify(info.title)} url=${info.url}`,
      );
    }
    await browser.switchToWindow(current);
    if (!rows.length) return `${LF}  （拿不到任何窗口句柄）`;
    return `${LF}  窗口清单（共 ${rows.length} 个）：${LF}${rows.join(LF)}`;
  } catch (e) {
    return `${LF}  （窗口诊断失败：${String(e)}）`;
  }
}

/** 等计数平静：连续 3 秒不变即视为进入空闲（最多等 20 秒）。 */
async function waitIdle(): Promise<void> {
  let stable = 0;
  let prev = await readCount();
  for (let i = 0; i < 20; i++) {
    await browser.pause(1_000);
    const cur = await readCount();
    stable = cur === prev ? stable + 1 : 0;
    prev = cur;
    if (stable >= 3) return;
  }
}
