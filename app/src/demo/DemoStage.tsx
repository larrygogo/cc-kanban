// demo 专用:桌面舞台——渐变背景 + 带阴影的贴纸窗口 + 字幕/收尾/假光标。
// 复用真实 Sticker / CollapsedStrip 组件,数据与形态由 mock store 驱动。
// Sticker 现为受控组件:tab(filter)与搜索(search)的状态都由父层持有并回传(受控接线)。
// 其中 tab 过滤仍在 Sticker 内部按 filter 做,只有 search 由父层下沉过滤(见下方 items)——
// 这里如实复刻 App.tsx 的接线,否则点 tab 不切、搜索不过滤(demo 才"真实")。
import { useEffect, useReducer, useState } from "react";
import { Sticker } from "../views/Sticker";
import { CollapsedStrip } from "../views/CollapsedStrip";
import { store, subscribe } from "./mock";
import type { Tab } from "../views/sticker/types";
import { DEMO_STRINGS, type DemoLang } from "./strings";
import logoUrl from "../../src-tauri/icons/128x128@2x.png";

export function DemoStage({ lang = "zh" }: { lang?: DemoLang }) {
  const [, force] = useReducer((x: number) => x + 1, 0);
  useEffect(() => subscribe(force), []);
  const [filter, setFilter] = useState<Tab>("all");
  const [search, setSearch] = useState("");
  const { mode, caption, finale, glow } = store.stage;
  const strip = mode === "strip";

  // 受控搜索:按标题 / 仓库名 / 项目名客户端过滤(真实 app 在父层下沉后端过滤,demo 同样在父层做)。
  // 每次都产出新数组引用——让 Sticker 内 shown/counts 的 useMemo 随会话对象的 in-place 变更重新计算。
  const q = search.trim().toLowerCase();
  const items = store.sessions.filter((l) =>
    !q ||
    (l.task_title ?? "").toLowerCase().includes(q) ||
    (l.cwd ?? "").toLowerCase().includes(q) ||
    (l.project_name ?? "").toLowerCase().includes(q)
  );

  return (
    <div className="demo-desktop">
      <div className="demo-blob demo-blob-a" />
      <div className="demo-blob demo-blob-b" />
      <div className="demo-grain" />
      <div className={"demo-window demo-mode-" + mode}>
        {strip ? (
          <CollapsedStrip data={items} edge="right" onExpand={() => {}} />
        ) : (
          <Sticker
            filter={filter}
            onFilterChange={setFilter}
            data={items}
            search={search}
            onSearchChange={setSearch}
          />
        )}
        {/* 吸边高亮:拖近右缘时对应侧发光(复用真实 app 的 .snap-glow) */}
        {glow && <div className="snap-glow snap-glow-right" />}
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
          <div className="demo-finale-slogan">{DEMO_STRINGS[lang].finale}</div>
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
