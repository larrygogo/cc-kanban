// 小红书海报生成器：复用真实 Sticker / CollapsedStrip 组件渲染界面，外层用 CSS 排版成 1080×1440。
// 界面状态由 mock store 直接构造（不走 demo 时间轴），所以标题说什么、图里就是什么。
// 用法：vite 起服务后访问 /poster.html?s=<scene>，由 scripts/export-poster.mjs 逐个截图。
import ReactDOM from "react-dom/client";
import { detectHostOs } from "../platform";
import { bootAppearance } from "../appearance";
import { installMocks, store } from "../demo/mock";
import { makeSession } from "../demo/data";
import { Sticker } from "../views/Sticker";
import { CollapsedStrip } from "../views/CollapsedStrip";
import type { Tab } from "../views/sticker/types";
import logoUrl from "../../src-tauri/icons/128x128@2x.png";
import "../fonts";
import "../styles.css";
import "./poster.css";

localStorage.clear();
const FIXED_NOW = 1_780_000_000_000;
Date.now = () => FIXED_NOW;
installMocks();
// 用 app 自己的界面缩放（--cc-ui）把贴纸放大到海报可读的尺寸。
// 不能在外层用 CSS zoom / transform：贴纸列表的行高是 getBoundingClientRect().height 实测的，
// 外部缩放会让测得的行高连同缩放一起变大，而定位坐标系没变 —— 卡片间距会被撑开。
store.settings.ui_scale = 165;
bootAppearance({ scale: true });

// ── 会话数据：与文案叙述一致的一组真实感状态 ──
const s1 = makeSession({
  title: "重构吸边状态机",
  project: "larrygogo/meowo",
  ctx: 64,
  todoDone: 4,
  todoTotal: 5,
  lastAi: "clippy 通过，写入 src/snap/mod.rs。",
});
const s2 = makeSession({
  title: "接入账号用量面板",
  project: "larrygogo/autopilot",
  status: "waiting",
  ctx: 43,
  todoDone: 2,
  todoTotal: 4,
  lastAi: "要应用这 3 处修改吗？(y / n)",
});
// 等更久的那个：待交互按等待时长排序，它会顶到 s2 前面（正是「自动排队」要讲的事）
const s5 = makeSession({
  title: "生成数据库迁移脚本",
  project: "larrygogo/cc-relay",
  status: "waiting",
  ctx: 71,
  todoDone: 3,
  todoTotal: 6,
  agoMin: 21,
  lastAi: "要覆盖已存在的 migration 文件吗？(y / n)",
});
const s3 = makeSession({
  title: "升级 tauri 到 2.3",
  project: "larrygogo/cc-relay",
  status: "stale",
  agoMin: 12,
  lastAi: "已更新 Cargo.toml，等你确认几处 breaking change。",
});
const s4 = makeSession({
  title: "修复 statusline 兼容性",
  project: "larrygogo/clawmo-ios",
  status: "ended",
  connected: false,
  agoMin: 180,
  lastAi: "兼容性修好并已合并，收工。",
});
store.sessions = [s1, s2, s5, s3, s4];

// 带便签的一份（卡片管理场景用）
const noted = { ...s1, note: "记得先确认 API key" };

function Shot({ filter = "all", items = store.sessions, width = 620, height = 740 }: {
  filter?: Tab;
  items?: typeof store.sessions;
  width?: number;
  height?: number;
}) {
  return (
    <div className="shot">
      <div
        className="shot-window"
        style={{ ["--shot-w" as string]: `${width}px`, ["--shot-h" as string]: `${height}px` }}
      >
        <Sticker filter={filter} onFilterChange={() => {}} data={items} search="" onSearchChange={() => {}} />
      </div>
    </div>
  );
}

function Points({ items, tone = "orange" }: { items: string[]; tone?: "orange" | "green" }) {
  return (
    <ul className={"points points-" + tone}>
      {items.map((t) => <li key={t}>{t}</li>)}
    </ul>
  );
}

function Footer() {
  return (
    <footer className="brand">
      <span className="url">meowo.io</span>
      <span className="meta">免费 · 开源 MIT · Windows / macOS</span>
    </footer>
  );
}

// ── 六个场景 ──
const SCENES: Record<string, () => JSX.Element> = {
  cover: () => (
    <div className="poster">
      <div className="kicker">Meowo 喵呜 · 桌面 AI 会话看板</div>
      <h1 className="h1">
        AI 早就跑完了<br />
        正在等你确认<br />
        <mark>而你不知道</mark>
      </h1>
      <p className="sub">Claude Code / Codex / Kimi，全都贴在桌面一角</p>
      <Shot filter="all" items={[s1, s2, s5]} width={620} height={620} />
      <Footer />
    </div>
  ),

  waiting: () => (
    <div className="poster">
      <h2 className="h2">「待交互」自动排队</h2>
      <p className="sub">等最久的那个，自己顶到最前面</p>
      <Shot filter="waiting" width={640} height={560} />
      <Points items={[
        "按等待时长排序，不用你记",
        "系统通知只弹一次，不反复骚扰",
        "点通知，直接跳回那个终端标签",
      ]} />
    </div>
  ),

  context: () => (
    <div className="poster">
      <h2 className="h2">Context 还剩多少<br /><em>卡片上就写着</em></h2>
      <p className="sub">取自 statusline 的准确值，不是估算</p>
      <Shot filter="all" items={[s1, s2, s5]} width={640} height={620} />
      <Points tone="green" items={[
        "每张卡片显示 Context 已用百分比",
        "底栏常驻 5 小时 / 7 天配额利用率",
        "不用等它突然爆掉才发现",
      ]} />
    </div>
  ),

  manage: () => (
    <div className="poster">
      <h2 className="h2">点开就能管</h2>
      <p className="sub">置顶 · 便签 · 改名 · 归档，悬停才出现</p>
      <Shot filter="all" items={[noted, s2, s3, s4]} width={640} height={640} />
      <div className="chips">
        {["置顶会话", "本地便签", "直接改名", "归档隐藏", "标题 / 仓库名搜索"].map((c) => (
          <span key={c} className="chip">{c}</span>
        ))}
      </div>
    </div>
  ),

  strip: () => (
    <div className="poster">
      <h2 className="h2">不用时<br />变成吸边<em>红绿灯</em></h2>
      <p className="sub">拖到屏幕左 / 右 / 顶边缘，松手就吸住</p>
      <div className="strip-stage">
        <div className="screen">
          <div className="screen-hint">你的屏幕</div>
          <div className="screen-app">
            {Array.from({ length: 8 }, (_, i) => <i key={i} />)}
          </div>
          <div className="strip-dock">
            <div className="strip-window">
              <CollapsedStrip data={store.sessions} edge="right" onExpand={() => {}} />
            </div>
          </div>
        </div>
        <div className="strip-callout">条上一个色点就是一个会话，颜色即它此刻的状态</div>
      </div>
      <Points items={[
        "悬停展开偷看，移开自动收回",
        "可置顶、不透明度 25%–100% 可调",
        "macOS 上是菜单栏面板，不占 Dock",
      ]} />
    </div>
  ),

  download: () => (
    <div className="poster poster-center">
      <img className="logo" src={logoUrl} width={132} height={132} alt="" />
      <h2 className="h2">装上就能用</h2>
      <p className="sub">启动时自动接入，不用改配置</p>
      <Points items={[
        "自动接进 Claude Code 的 hooks 与 statusLine",
        "先备份、再原子写入，不破坏已有配置",
        "数据只落本地 SQLite（~/.meowo/board.db）",
        "首启导入近 7 天历史会话，不从空白开始",
        "深色 / 浅色 / 跟随系统，界面密度可调",
      ]} />
      <a className="cta">meowo.io</a>
      <p className="meta-center">按你的系统直接给安装包 · Windows / macOS</p>
      <p className="meta-center">免费 · 开源 MIT · GitHub: larrygogo/meowo</p>
    </div>
  ),
};

(async () => {
  await detectHostOs();
  const scene = new URLSearchParams(location.search).get("s") ?? "cover";
  const View = SCENES[scene] ?? SCENES.cover;
  ReactDOM.createRoot(document.getElementById("root")!).render(<View />);
  // 供导出脚本探测：渲染完成 + 可用场景清单
  (window as unknown as { __poster: unknown }).__poster = { scenes: Object.keys(SCENES) };
})();
