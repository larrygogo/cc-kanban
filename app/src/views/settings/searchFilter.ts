// 设置搜索的行级过滤（S-1）：纯 DOM 操作，从 About.tsx 抽出以便单测。
//
// 过滤粒度：
// - 行：`.row`（设置项行）与 `.usage-sticker-row`（账号卡深处的「在贴纸显示配额」——
//   埋在已登录账号卡里，经搜索可达即可，不必挪位）——标题+说明文本命中即保留；
// - 卡：`.row-card` 自身文本命中（如 agent 名「kimi」）整卡保留；否则有命中行才保留；
// - 分组标题 `.sec-caption`：跟随其后（到下一个标题前）卡片的可见性；
// - 分区 `.main-sec`：导航名命中（如「外观」）整区保留，否则无可见行/卡时整区隐藏；
// - 其余直挂内容（.sec-hint / 外观预览 / 关于页脚等）：搜索时让位。
//
// 显示切换一律走 style.display 而不是 hidden 属性：.row 等类的 display:flex 会盖掉
// [hidden] 的 UA 样式；且 .main-sec 的 hidden 归 React 管（分区切换），不能抢。
const FILTER_ROW = ".row, .usage-sticker-row";

/**
 * 应用一次过滤；返回是否有可见行/卡（驱动「无结果」提示）。
 * q 为空白时清除全部过滤痕迹（返回 false，无搜索态该值不被使用）。
 */
export function applySettingsFilter(body: HTMLElement, q: string, navLabels: Record<string, string>): boolean {
  const query = q.trim().toLowerCase();
  let anyVisible = false;
  for (const secEl of body.querySelectorAll<HTMLElement>(".main-sec")) {
    const setDisplay = (el: HTMLElement, show: boolean) => {
      el.style.display = show ? "" : "none";
    };
    if (!query) {
      secEl.style.display = "";
      for (const el of secEl.querySelectorAll<HTMLElement>(`${FILTER_ROW}, .row-card, .sec-caption`)) {
        el.style.display = "";
      }
      for (const kid of Array.from(secEl.children)) (kid as HTMLElement).style.display = "";
      continue;
    }
    const secKey = secEl.getAttribute("data-sec") ?? "";
    const navHit = (navLabels[secKey] ?? "").toLowerCase().includes(query);
    let secVisible = navHit;
    for (const card of secEl.querySelectorAll<HTMLElement>(".row-card")) {
      // 卡片「自身文本」= 去掉可过滤行后的剩余文本（如 provider 卡的 agent 名）。
      // 行文本不能算进卡片匹配——否则任何一行命中都会让整卡文本命中，行级过滤形同虚设。
      const ownClone = card.cloneNode(true) as HTMLElement;
      for (const r of ownClone.querySelectorAll(FILTER_ROW)) r.remove();
      const cardHit = navHit || (ownClone.textContent ?? "").toLowerCase().includes(query);
      let cardVisible = cardHit;
      for (const row of card.querySelectorAll<HTMLElement>(FILTER_ROW)) {
        const rowHit = cardHit || (row.textContent ?? "").toLowerCase().includes(query);
        setDisplay(row, rowHit);
        if (rowHit) cardVisible = true;
      }
      setDisplay(card, cardVisible);
      if (cardVisible) secVisible = true;
    }
    // 不在卡片里的散行（当前布局没有，兜底）。
    for (const row of secEl.querySelectorAll<HTMLElement>(FILTER_ROW)) {
      if (row.closest(".row-card")) continue;
      const rowHit = navHit || (row.textContent ?? "").toLowerCase().includes(query);
      setDisplay(row, rowHit);
      if (rowHit) secVisible = true;
    }
    // 直挂孩子：分组标题跟随本组卡片可见性，其余（提示/预览/页脚）搜索时让位。
    let caption: HTMLElement | null = null;
    let groupVisible = false;
    const flush = () => {
      if (caption) setDisplay(caption, navHit || groupVisible);
      caption = null;
      groupVisible = false;
    };
    for (const kid of Array.from(secEl.children) as HTMLElement[]) {
      if (kid.classList.contains("sec-caption")) {
        flush();
        caption = kid;
      } else if (kid.classList.contains("row-card")) {
        if (kid.style.display !== "none") groupVisible = true;
      } else if (kid.classList.contains("account-agent-switch")) {
        // 7S-4：账号分区一次只渲染**当前** agent 的卡，别家的名字（"codex"）搜不到；
        // 而通往别家的唯一入口正是这个切换器，搜索时把它一并藏掉就成了死路——
        // 分区还在就留着它。
        setDisplay(kid, secVisible);
      } else {
        setDisplay(kid, false);
      }
    }
    flush();
    setDisplay(secEl, secVisible);
    if (secVisible) anyVisible = true;
  }
  return query !== "" && anyVisible;
}
