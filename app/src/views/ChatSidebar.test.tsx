import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { ChatSidebar } from "./ChatSidebar";
import type { LiveSession } from "../api";

function session(id: number, title: string, extra: Partial<LiveSession> = {}): LiveSession {
  return {
    session: { id, cc_session_id: `cc-${id}`, status: "ended" },
    project_name: "meowo",
    task_title: title,
    connected: false,
    pending_review: null,
    cwd: "C:/Users/me/workspace/meowo",
    provider: "claude",
    ...extra,
  } as unknown as LiveSession;
}

describe("ChatSidebar", () => {
  afterEach(cleanup);
  beforeEach(() => {
    invoke.mockReset();
    localStorage.clear();
  });

  it("lists sessions and switches on click", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "get_live_sessions_page") {
        return Promise.resolve([session(163, "解决冲突", { connected: true }), session(150, "旧任务")]);
      }
      return Promise.resolve();
    });
    const onSelect = vi.fn();
    render(<ChatSidebar activeId={163} approvalAwaitingIds={new Set()} onSelect={onSelect} onCollapse={() => {}} />);
    const active = await screen.findByRole("button", { name: /解决冲突/ });
    expect(active.getAttribute("aria-current")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: /旧任务/ }));
    expect(onSelect).toHaveBeenCalledWith(150);
  });

  /// 一个跑了一阵的目录能堆出几十条历史会话，把真正在跑的那一两条挤出视野。
  /// 未运行的默认收起，点一下才展开——但绝不能真的丢掉。
  describe("分组里的未运行会话默认收起", () => {
    const mixed = [
      session(1, "在跑的", { connected: true }),
      session(2, "停了的"),
      session(3, "也停了"),
    ];
    const renderGrouped = async (activeId = 1) => {
      localStorage.setItem("meowo-chat-sidebar-grouped", "1");
      invoke.mockImplementation((command: string) =>
        Promise.resolve(command === "get_live_sessions_page" ? mixed : undefined));
      render(<ChatSidebar activeId={activeId} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
      await screen.findByRole("button", { name: /在跑的/ });
    };

    it("默认只显示在跑的，其余折进一个可点的入口", async () => {
      await renderGrouped();
      expect(screen.queryByRole("button", { name: /停了的/ })).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "显示 2 个未运行会话" }));
      expect(screen.getByRole("button", { name: /停了的/ })).toBeTruthy();
      expect(screen.getByRole("button", { name: /也停了/ })).toBeTruthy();
      // 再点一下收回去。
      fireEvent.click(screen.getByRole("button", { name: "收起未运行会话" }));
      expect(screen.queryByRole("button", { name: /停了的/ })).toBeNull();
    });

    /// 正开着的会话无论死活都得留在视野里，否则用户一进来就找不到自己在哪。
    it("当前打开的那条即使已断开也不收起", async () => {
      await renderGrouped(2);
      expect(screen.getByRole("button", { name: /停了的/ })).toBeTruthy();
      expect(screen.getByRole("button", { name: "显示 1 个未运行会话" })).toBeTruthy();
    });

    /// 一条在跑的都没有时不收——展开分组后空空如也比排得长更难用。
    it("整组都没在跑时照常全列", async () => {
      localStorage.setItem("meowo-chat-sidebar-grouped", "1");
      invoke.mockImplementation((command: string) =>
        Promise.resolve(command === "get_live_sessions_page" ? [session(8, "旧一"), session(9, "旧二")] : undefined));
      render(<ChatSidebar activeId={0} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
      await screen.findByRole("button", { name: /旧一/ });
      expect(screen.getByRole("button", { name: /旧二/ })).toBeTruthy();
      expect(screen.queryByRole("button", { name: /显示 .* 个未运行会话/ })).toBeNull();
    });
  });

  it("reports collapse to the parent", async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === "get_live_sessions_page" ? [session(1, "任务A")] : undefined));
    const onCollapse = vi.fn();
    render(<ChatSidebar activeId={1} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={onCollapse} />);
    await screen.findByRole("button", { name: /任务A/ });
    fireEvent.click(screen.getByRole("button", { name: "收起会话列表" }));
    // 折叠状态归 ChatWindow 持有（展开入口在标题栏），侧栏只上报意图。
    expect(onCollapse).toHaveBeenCalled();
  });

  /** jsdom 里滚动尺寸恒为 0，手动装出「已经滚到底」的几何。 */
  function fakeScrolledToBottom(list: HTMLElement) {
    Object.defineProperty(list, "scrollHeight", { value: 1000, configurable: true });
    Object.defineProperty(list, "clientHeight", { value: 400, configurable: true });
    Object.defineProperty(list, "scrollTop", { value: 600, configurable: true });
  }

  it("滚到底继续翻页，直到后端给不满一页", async () => {
    const all = Array.from({ length: 150 }, (_, i) => session(1000 - i, `会话 ${i}`));
    const limits: number[] = [];
    invoke.mockImplementation((command: string, args: { limit: number }) => {
      if (command !== "get_live_sessions_page") return Promise.resolve();
      limits.push(args.limit);
      return Promise.resolve(all.slice(0, args.limit));
    });
    render(<ChatSidebar activeId={1000} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    await screen.findByRole("button", { name: /会话 0/ });
    expect(limits).toEqual([60]);
    expect(screen.queryByRole("button", { name: /会话 60/ })).toBeNull();

    const list = screen.getByRole("navigation");
    fakeScrolledToBottom(list);

    fireEvent.scroll(list);
    await screen.findByRole("button", { name: /会话 60/ });
    expect(limits).toEqual([60, 120]);

    fireEvent.scroll(list);
    await screen.findByRole("button", { name: /会话 149/ });
    expect(limits).toEqual([60, 120, 180]);

    // 150 < 180：后端已经给不满，到此为止，再滚也不应该再发请求。
    fireEvent.scroll(list);
    fireEvent.scroll(list);
    expect(limits).toEqual([60, 120, 180]);
  });

  it("翻页请求失败后回退，不留死 loading 行，且重滚可重试", async () => {
    const all = Array.from({ length: 150 }, (_, i) => session(1000 - i, `会话 ${i}`));
    const limits: number[] = [];
    invoke.mockImplementation((command: string, args: { limit: number }) => {
      if (command !== "get_live_sessions_page") return Promise.resolve();
      limits.push(args.limit);
      // 第二次（首个翻页请求）失败，其余成功。
      if (limits.length === 2) return Promise.reject(new Error("db busy"));
      return Promise.resolve(all.slice(0, args.limit));
    });
    render(<ChatSidebar activeId={1000} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    await screen.findByRole("button", { name: /会话 0/ });
    const list = screen.getByRole("navigation");
    fakeScrolledToBottom(list);

    fireEvent.scroll(list);
    await waitFor(() => expect(limits).toEqual([60, 120]));
    // 失败后 loading 行必须消失（曾经的 bug：limit 卡在高位，loading 行永挂、滚动失效）。
    await waitFor(() => expect(screen.queryByText("正在加载会话…")).toBeNull());
    expect(screen.queryByRole("button", { name: /会话 60/ })).toBeNull();

    // 再滚：limit 已回退，应当能重新发起同样的翻页请求并成功。
    fireEvent.scroll(list);
    await screen.findByRole("button", { name: /会话 60/ });
    expect(limits).toEqual([60, 120, 120]);
  });

  it("翻页只在尾部追加，不重排用户正看着的前缀", async () => {
    const all = Array.from({ length: 120 }, (_, i) => session(1000 - i, `会话 ${i}`));
    let calls = 0;
    invoke.mockImplementation((command: string, args: { limit: number }) => {
      if (command !== "get_live_sessions_page") return Promise.resolve();
      calls += 1;
      if (calls === 1) return Promise.resolve(all.slice(0, args.limit));
      // 翻页响应（新返回形状）：后端把一条更深处的活会话排到了整页最前——
      // 侧栏不能照单全收把它插到用户视口上方，只能把新条目续在尾部。
      return Promise.resolve({
        items: [session(1, "新活会话", { connected: true }), ...all.slice(0, args.limit - 1)],
        next_cursor: { last_event_at: 1, id: 1 },
      });
    });
    render(<ChatSidebar activeId={1000} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    await screen.findByRole("button", { name: /会话 0/ });
    const list = screen.getByRole("navigation");
    fakeScrolledToBottom(list);

    fireEvent.scroll(list);
    await screen.findByRole("button", { name: /新活会话/ });
    // 只看会话条目：每条右侧还有个「⋯」菜单按钮，混进来会把索引全打乱。
    const names = within(list).getAllByRole("button")
      .filter((b) => b.className.includes("chat-sidebar-item") && !b.className.includes("menu"))
      .map((b) => b.textContent ?? "");
    expect(names[0]).toContain("会话 0");
    expect(names[59]).toContain("会话 59");
    // 新条目（含被后端顶到最前的活会话）只允许出现在原有 60 条之后。
    expect(names.findIndex((n) => n.includes("新活会话"))).toBe(60);
  });

  it("按 sessionTone 渲染状态点:running 脉冲、pending 召唤、断开/已结束不加点", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "get_live_sessions_page") {
        return Promise.resolve([
          session(1, "在跑", { connected: true, session: { id: 1, cc_session_id: "cc-1", status: "running" } } as Partial<LiveSession>),
          session(2, "待审批", { connected: true, pending_review: "approval", session: { id: 2, cc_session_id: "cc-2", status: "waiting" } } as Partial<LiveSession>),
          session(3, "在等", { connected: true, session: { id: 3, cc_session_id: "cc-3", status: "waiting" } } as Partial<LiveSession>),
          session(4, "已断开"),
        ]);
      }
      return Promise.resolve();
    });
    render(<ChatSidebar activeId={1} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    const dotOf = async (name: RegExp) =>
      (await screen.findByRole("button", { name })).querySelector(".chat-sidebar-dot");
    expect((await dotOf(/在跑/))?.className).toContain("is-running");
    // pending 优先于 waiting:它有明确的动作召唤。
    expect((await dotOf(/待审批/))?.className).toContain("is-pending");
    expect((await dotOf(/在等/))?.className).toContain("is-waiting");
    // 断开/已结束:图标置灰已表达不活跃,不再叠点。
    expect(await dotOf(/已断开/)).toBeNull();
  });

  it("待授权会话亮琥珀徽标并压过状态点,授权清掉后回落", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "get_live_sessions_page") {
        return Promise.resolve([
          session(1, "要授权", { connected: true, session: { id: 1, cc_session_id: "cc-1", status: "running" } } as Partial<LiveSession>),
          session(2, "没事"),
        ]);
      }
      return Promise.resolve();
    });
    const dotOf = async (name: RegExp) =>
      (await screen.findByRole("button", { name })).querySelector(".chat-sidebar-dot");
    const { rerender } = render(<ChatSidebar activeId={2} approvalAwaitingIds={new Set([1])} onSelect={() => {}} onCollapse={() => {}} />);
    // 授权比 running 更紧急：同一个 presence 位换亮琥珀徽标。
    expect((await dotOf(/要授权/))?.className).toContain("is-approval");
    expect(await dotOf(/没事/)).toBeNull();
    rerender(<ChatSidebar activeId={2} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    await waitFor(() => expect(screen.getByRole("button", { name: /要授权/ }).querySelector(".chat-sidebar-dot")?.className).toContain("is-running"));
  });

  /**
   * 目录筛选走**后端**专用 cwd 参数(斜杠归一后精确匹配,不是 search 的子串 LIKE:
   * 子串会让另一种斜杠写法的会话整批消失,还把兄弟目录/标题命中漏进来):前端过滤
   * 只能过滤已加载的这一页,筛出来的清单是残缺的。切目录还必须把翻页状态清回首页,
   * 否则新目录一上来就顶着上一个目录翻到的 limit 拉几百条。
   */
  it("目录筛选把 cwd 传进后端专用参数,并把翻页重置回首页", async () => {
    const args: { search: string | null; cwd: string | null; limit: number }[] = [];
    invoke.mockImplementation((command: string, a: { search: string | null; cwd: string | null; limit: number }) => {
      if (command === "recent_cwds") return Promise.resolve(["C:/Users/me/workspace/meowo", "D:/tmp/scratch"]);
      if (command !== "get_live_sessions_page") return Promise.resolve();
      args.push({ search: a.search, cwd: a.cwd, limit: a.limit });
      return Promise.resolve(
        a.cwd === "D:/tmp/scratch"
          ? [session(9, "临时活儿", { cwd: "D:/tmp/scratch" })]
          : Array.from({ length: 60 }, (_, i) => session(100 - i, `会话 ${i}`)),
      );
    });
    render(<ChatSidebar activeId={100} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    await screen.findByRole("button", { name: /会话 0/ });
    expect(args).toEqual([{ search: null, cwd: null, limit: 60 }]);

    // 先翻一页，让 limit 抬到 120——切目录后它必须回到 60。
    const list = screen.getByRole("navigation");
    fakeScrolledToBottom(list);
    fireEvent.scroll(list);
    await waitFor(() => expect(args).toHaveLength(2));
    expect(args[1]).toEqual({ search: null, cwd: null, limit: 120 });

    fireEvent.click(screen.getByRole("button", { name: /全部目录/ }));
    fireEvent.click(await screen.findByRole("option", { name: "scratch" }));
    expect(await screen.findByRole("button", { name: /临时活儿/ })).toBeTruthy();
    expect(args[args.length - 1]).toEqual({ search: null, cwd: "D:/tmp/scratch", limit: 60 });
    // 上一个目录的会话不能残留。
    expect(screen.queryByRole("button", { name: /会话 0/ })).toBeNull();
  });

  /**
   * 分组不另起一套排序：组按「组内第一条会话的位置」排、组内保持原顺序，于是后端的
   * connected-first + 时间倒序原样透过来——用户原来靠什么找会话，分组后还靠什么找。
   * 组折起来时，组头的汇总点是这一组唯一的状态出口，必须取组内最强的召唤。
   */
  it("分组开关:组序由已有排序派生,组头汇总最强状态,可折叠并记住", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "recent_cwds") return Promise.resolve([]);
      if (command !== "get_live_sessions_page") return Promise.resolve();
      return Promise.resolve([
        // 后端已排好序：活跃的在前。scratch 的活会话排在最前 → scratch 组就该在最前，
        // 哪怕 meowo 组的会话更多。
        session(1, "临时活儿", { cwd: "D:/tmp/scratch", connected: true, session: { id: 1, cc_session_id: "cc-1", status: "waiting" } } as Partial<LiveSession>),
        session(2, "改侧栏", { cwd: "C:/Users/me/workspace/meowo", connected: true, session: { id: 2, cc_session_id: "cc-2", status: "running" } } as Partial<LiveSession>),
        // 斜杠方向与大小写不同的同一目录，必须并进同一组而不是裂成两组。
        session(3, "发版", { cwd: "C:\\Users\\me\\workspace\\Meowo" }),
      ]);
    });
    render(<ChatSidebar activeId={2} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    await screen.findByRole("button", { name: /改侧栏/ });
    // 默认关：一个像素都不改，没有组头。
    expect(screen.queryByRole("button", { name: /^scratch/ })).toBeNull();

    // 分组方式换成了 Group by 菜单：先开下拉（按钮显示当前档「不分组」），再选「按目录」。
    fireEvent.click(screen.getByRole("button", { name: "不分组" }));
    fireEvent.click(screen.getByRole("option", { name: "按目录" }));
    const list = screen.getByRole("navigation");
    const heads = within(list).getAllByRole("button").filter((b) => b.className.includes("group-head"));
    expect(heads.map((h) => h.textContent)).toEqual(["scratch1", "meowo2"]);
    // scratch 组只有一条 waiting，meowo 组里 running 最强（另一条已断开不参与）。
    expect(heads[0].querySelector(".chat-sidebar-dot")?.className).toContain("is-waiting");
    expect(heads[1].querySelector(".chat-sidebar-dot")?.className).toContain("is-running");

    fireEvent.click(heads[1]);
    expect(screen.queryByRole("button", { name: /改侧栏/ })).toBeNull();
    // 折起来后组头的汇总点还在——否则「折叠即失明」，用户会错过组里在跑的东西。
    expect(heads[1].querySelector(".chat-sidebar-dot")?.className).toContain("is-running");

    // 侧栏收起时本组件整个卸载，折叠状态与开关都得落盘，回来才不是全展开。
    cleanup();
    render(<ChatSidebar activeId={2} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    expect(await screen.findByRole("button", { name: /临时活儿/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /改侧栏/ })).toBeNull();
  });

  /** 按状态分组：组序按召唤强度（出错 > 运行中 > 已结束），不派生原排序——
   *  状态视图的意义就是「先看要处理的」。 */
  it("按状态分组:组序按召唤强度", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "recent_cwds") return Promise.resolve([]);
      if (command !== "get_live_sessions_page") return Promise.resolve();
      return Promise.resolve([
        session(1, "在跑的", { connected: true, session: { id: 1, cc_session_id: "cc-1", status: "running" } } as Partial<LiveSession>),
        session(2, "出错的", { connected: true, errored: true, session: { id: 2, cc_session_id: "cc-2", status: "running" } } as Partial<LiveSession>),
        session(3, "结束的"),
      ]);
    });
    localStorage.setItem("meowo-chat-sidebar-group-mode", "state");
    render(<ChatSidebar activeId={1} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    await screen.findByRole("button", { name: /在跑的/ });
    const list = screen.getByRole("navigation");
    const heads = within(list).getAllByRole("button").filter((b) => b.className.includes("group-head"));
    expect(heads.map((h) => h.textContent)).toEqual(["出错了1", "运行中1", "已结束1"]);
  });

  /**
   * 会话菜单与看板卡片是同一个 CardContextMenu、同一批动作。侧栏条目从 <button> 改成
   * role=button 的 <div> 是为了能在里面放「⋯」按钮（button 套 button 非法）。
   */
  describe("会话菜单", () => {
    const listed = [session(1, "改侧栏", { connected: true }), session(2, "旧任务")];
    /** menuMode 跟随「卡片菜单」设置：不传 = 右键模式（getSettings 无响应时的占位默认）。 */
    const renderList = async (activeId = 1, cardMenuMode?: "button" | "context") => {
      invoke.mockImplementation((command: string) => {
        if (command === "get_live_sessions_page") return Promise.resolve(listed);
        if (command === "get_settings" && cardMenuMode) return Promise.resolve({ card_menu_mode: cardMenuMode });
        return Promise.resolve();
      });
      render(<ChatSidebar activeId={activeId} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
      await screen.findByRole("button", { name: /改侧栏/ });
    };

    it("右键条目弹出菜单，归档走 set_archived 并把条目乐观摘掉", async () => {
      await renderList();
      fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "归档" }));
      expect(invoke).toHaveBeenCalledWith("set_archived", { sessionId: 2, archived: true });
      // 侧栏取的是 "all"（不含已归档）：点完当场消失，不等下一轮刷新。
      await waitFor(() => expect(screen.queryByRole("button", { name: /旧任务/ })).toBeNull());
    });

    /// 已归档 = 已收纳，列表里就不该再有它——正开着它的对话也一样，且右边要跟着换走，
    /// 否则「收起来了却还摊在桌上」。
    it("归档当前会话：摘掉它并切到下一条", async () => {
      const onSelect = vi.fn();
      invoke.mockImplementation((command: string) =>
        Promise.resolve(command === "get_live_sessions_page" ? listed : undefined));
      render(<ChatSidebar activeId={1} approvalAwaitingIds={new Set()} onSelect={onSelect} onCollapse={() => {}} />);
      await screen.findByRole("button", { name: /改侧栏/ });
      fireEvent.contextMenu(screen.getByRole("button", { name: /改侧栏/ }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "归档" }));
      expect(invoke).toHaveBeenCalledWith("set_archived", { sessionId: 1, archived: true });
      expect(onSelect).toHaveBeenCalledWith(2);
      await waitFor(() => expect(screen.queryByRole("button", { name: /改侧栏/ })).toBeNull());
    });

    /// 归档的不是当前会话时不该乱切——用户只是在收拾别的会话。
    it("归档别的会话不动当前对话", async () => {
      const onSelect = vi.fn();
      invoke.mockImplementation((command: string) =>
        Promise.resolve(command === "get_live_sessions_page" ? listed : undefined));
      render(<ChatSidebar activeId={1} approvalAwaitingIds={new Set()} onSelect={onSelect} onCollapse={() => {}} />);
      await screen.findByRole("button", { name: /改侧栏/ });
      fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "归档" }));
      expect(onSelect).not.toHaveBeenCalled();
    });

    /// 触发方式与看板共用 card_menu_mode，且**二选一**：两个入口并存时，改了设置的人
    /// 会以为没生效（另一个还在），没改的人平白多一个不知从哪来的按钮。
    it("按钮模式下才有「⋯」，右键交还给系统", async () => {
      await renderList(1, "button");
      const menus = await screen.findAllByRole("button", { name: "更多操作" });
      expect(menus).toHaveLength(2);
      const contextEvent = fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      expect(contextEvent).toBe(true); // 未 preventDefault
      expect(screen.queryByRole("menuitem", { name: "归档" })).toBeNull();
    });

    /// 菜单吃掉的 Esc 必须标记 preventDefault：对话窗有窗口级的「Esc = 拒绝审批」监听
    /// （以 defaultPrevented 让路），而 document 冒泡在 window 之前——不标记的话，
    /// 关个右键菜单的同一次按键会顺手把 agent 的审批请求拒了。
    it("Esc 关菜单时吃掉这次按键，不落到「拒绝审批」上", async () => {
      await renderList();
      fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      expect(await screen.findByRole("menuitem", { name: "归档" })).toBeTruthy();
      // fireEvent 返回 !defaultPrevented：false = 这次按键已被菜单消费掉。
      expect(fireEvent.keyDown(document, { key: "Escape" })).toBe(false);
      await waitFor(() => expect(screen.queryByRole("menuitem", { name: "归档" })).toBeNull());
    });

    it("右键模式下不渲染「⋯」按钮", async () => {
      await renderList(1, "context");
      fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      expect(await screen.findByRole("menuitem", { name: "归档" })).toBeTruthy();
      expect(screen.queryByRole("button", { name: "更多操作" })).toBeNull();
    });

    it("「⋯」按钮同样打开菜单，重命名就地编辑并写回后端", async () => {
      await renderList(1, "button");
      const menus = await screen.findAllByRole("button", { name: "更多操作" });
      fireEvent.click(menus[1]);
      fireEvent.click(await screen.findByRole("menuitem", { name: "重命名" }));
      const input = screen.getByPlaceholderText("输入名称，回车保存");
      fireEvent.change(input, { target: { value: "改个名" } });
      fireEvent.keyDown(input, { key: "Enter" });
      expect(invoke).toHaveBeenCalledWith("rename_session", {
        cwd: "C:/Users/me/workspace/meowo", sessionId: "cc-2", title: "改个名", provider: "claude",
      });
    });

    it("便签写完看得见，失败给出提示", async () => {
      invoke.mockImplementation((command: string) => {
        if (command === "get_live_sessions_page") return Promise.resolve([session(2, "旧任务", { note: "先跑一遍测试" })]);
        if (command === "set_session_note") return Promise.reject(new Error("db busy"));
        return Promise.resolve();
      });
      render(<ChatSidebar activeId={0} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
      // 已有便签直接显示在条目里——写了看不见的备忘等于没写。
      expect(await screen.findByText("先跑一遍测试")).toBeTruthy();
      fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "编辑便签" }));
      const input = screen.getByPlaceholderText("写点备忘…");
      fireEvent.change(input, { target: { value: "改成先看日志" } });
      fireEvent.keyDown(input, { key: "Enter" });
      expect(await screen.findByText("便签保存失败，请稍后再试。")).toBeTruthy();
    });

    it("置顶与看板共用同一份存储（键沿用 meowo-starred）", async () => {
      await renderList();
      fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "置顶" }));
      expect(JSON.parse(localStorage.getItem("meowo-starred") ?? "[]")).toEqual(["cc-2"]);
    });

    /// 「置顶」得真的顶上去：只亮标记不动位置，等于名不副实。
    it("置顶的会话排到最前，其余相对次序不变", async () => {
      await renderList();
      const names = () => screen.getAllByRole("button")
        .filter((b) => b.className.includes("chat-sidebar-item") && !b.className.includes("menu"))
        .map((b) => b.textContent ?? "");
      expect(names()[0]).toContain("改侧栏");
      fireEvent.contextMenu(screen.getByRole("button", { name: /旧任务/ }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "置顶" }));
      await waitFor(() => expect(names()[0]).toContain("旧任务"));
      expect(names()[1]).toContain("改侧栏");
    });

    /// 置顶的会话已断开时也不该被折进「显示 N 个未运行会话」——用户刚亲手顶上来的。
    it("分组视图里置顶的会话不被折进未运行", async () => {
      localStorage.setItem("meowo-chat-sidebar-grouped", "1");
      localStorage.setItem("meowo-starred", JSON.stringify(["cc-2"]));
      invoke.mockImplementation((command: string) =>
        Promise.resolve(command === "get_live_sessions_page" ? listed : undefined));
      render(<ChatSidebar activeId={1} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
      expect(await screen.findByRole("button", { name: /旧任务/ })).toBeTruthy();
      expect(screen.queryByRole("button", { name: /显示 .* 个未运行会话/ })).toBeNull();
    });
  });

  it("survives a backend without the sessions command", async () => {
    // demo/旧后端对未知命令返回 undefined：侧栏必须静默降级为空列表，不能崩掉整个窗口。
    invoke.mockResolvedValue(undefined);
    render(<ChatSidebar activeId={1} approvalAwaitingIds={new Set()} onSelect={() => {}} onCollapse={() => {}} />);
    expect(await screen.findByText("暂无会话")).toBeTruthy();
  });
});
