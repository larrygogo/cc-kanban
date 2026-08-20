// 外观（明暗模式 / 不透明度 / 界面密度）的运行时套用。
// 两个窗口（贴纸 main、设置 about）都在 main.tsx 里 boot：
//   - 明暗模式 + 不透明度：两窗都套用（不透明度只影响走 --cc-bg 的贴纸/缩略条，设置窗口无副作用）。
//   - 界面密度（--cc-ui）：仅贴纸窗口套用（设置窗口固定尺寸，不缩放）。
// 数据源是 ~/.meowo/settings.json，经 get_settings 读取；任一窗口改设置后后端广播
// settings-changed，这里实时重套用（顺带做了设置窗口里的明暗即时预览）。
import { listen } from "@tauri-apps/api/event";
import { getSettings, type Settings, type StickerStyle, type ThemeMode } from "./api";
import { remoteUi, REMOTE_SETTINGS_EVENT } from "./remoteMode";

/** 贴纸底色预设：swatch = 设置页色板里显示的鲜亮代表色（小圆点便于区分）；
 *  dark/light = 该色在深/浅主题下实际套用的贴纸底色 RGB（低饱和微染，配合不透明+毛玻璃才不刺眼）。 */
export type StickerColorPreset = { swatch: string; dark: string; light: string };
export const STICKER_COLORS: Record<string, StickerColorPreset> = {
  neutral: { swatch: "#ffffff", dark: "33, 33, 35", light: "247, 247, 249" }, // 无色（默认，中性不染色；swatch 白底红斜杠示意「无」）
  classic: { swatch: "#d97757", dark: "38, 38, 36", light: "250, 249, 245" }, // 经典原色（暖褐）
  slate: { swatch: "#5b8db8", dark: "29, 37, 47", light: "239, 244, 250" }, // 石青
  moss: { swatch: "#6fae6a", dark: "30, 41, 33", light: "239, 248, 240" }, // 苔绿
  plum: { swatch: "#a87cc8", dark: "43, 34, 48", light: "248, 242, 251" }, // 暮紫
  rose: { swatch: "#d7748f", dark: "47, 34, 39", light: "251, 241, 244" }, // 玫粉
  amber: { swatch: "#d9a441", dark: "46, 40, 27", light: "251, 246, 232" }, // 琥珀
};
export const STICKER_COLOR_KEYS = Object.keys(STICKER_COLORS);
const DEFAULT_STICKER_COLOR = "neutral";
const DEFAULT_STICKER_STYLE: StickerStyle = "flat";

/** 把（颜色预设 key × 生效主题）解析为贴纸底色 RGB；未知 key 回退默认（无色）。纯函数，便于单测。 */
export function stickerBgRgb(color: string, theme: "dark" | "light"): string {
  const p = STICKER_COLORS[color] ?? STICKER_COLORS[DEFAULT_STICKER_COLOR];
  return theme === "light" ? p.light : p.dark;
}

type Appearance = {
  theme: ThemeMode;
  opacity: number;
  ui_scale: number;
  sticker_style: StickerStyle;
  sticker_color: string;
};

const CACHE_KEY = "meowo-appearance";
const DEFAULTS: Appearance = {
  theme: "dark",
  opacity: 100,
  ui_scale: 100,
  sticker_style: DEFAULT_STICKER_STYLE,
  sticker_color: DEFAULT_STICKER_COLOR,
};

let current: Appearance = DEFAULTS;
let scaleEnabled = false;

function clampPct(n: number, lo: number, hi: number): number {
  if (!Number.isFinite(n)) return hi;
  return Math.max(lo, Math.min(hi, n));
}

/** 把外观模式解析为实际生效的深/浅；system 时读系统偏好。 */
function resolveTheme(t: ThemeMode): "dark" | "light" {
  if (t === "light") return "light";
  if (t === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return "dark";
}

function apply(a: Appearance): void {
  current = a;
  const root = document.documentElement;
  const theme = resolveTheme(a.theme);
  root.setAttribute("data-theme", theme);
  // 贴纸风格（立体感/扁平）：CSS 用 [data-sticker-style="flat"] 抹平所有立体效果。
  root.setAttribute("data-sticker-style", a.sticker_style);
  // 贴纸底色：内联设 --cc-bg-rgb（天然盖过 :root[data-theme=light] 的默认值），随生效主题取深/浅一套。
  root.style.setProperty("--cc-bg-rgb", stickerBgRgb(a.sticker_color, theme));
  // 不透明度下限与 UI 一致（25–100）：放低下限以便配合系统 acrylic 透出更明显的模糊桌面；
  // 仍留 25% 底，避免手改 settings.json 为极小值时渲染出全透明的空底板。
  root.style.setProperty("--cc-opacity", String(clampPct(a.opacity, 25, 100) / 100));
  if (scaleEnabled) {
    root.style.setProperty("--cc-ui", String(clampPct(a.ui_scale, 50, 200) / 100));
  }
}

function pick(s: Partial<Settings> | null | undefined): Appearance {
  return {
    theme: s?.theme ?? DEFAULTS.theme,
    opacity: s?.opacity ?? DEFAULTS.opacity,
    ui_scale: s?.ui_scale ?? DEFAULTS.ui_scale,
    sticker_style: s?.sticker_style ?? DEFAULTS.sticker_style,
    sticker_color: s?.sticker_color ?? DEFAULTS.sticker_color,
  };
}

function readCache(): Appearance {
  try {
    const c = JSON.parse(localStorage.getItem(CACHE_KEY) || "");
    if (c && typeof c === "object") return pick(c as Partial<Settings>);
  } catch {
    /* ignore */
  }
  return DEFAULTS;
}

function writeCache(a: Appearance): void {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(a));
  } catch {
    /* ignore */
  }
}

/**
 * 启动时调用：先用缓存同步套用（避免浅色用户首屏闪深色），再异步拉真实设置校正，
 * 并订阅 settings-changed 实时套用、prefers-color-scheme 跟随系统。
 * @param opts.scale 是否套用界面密度（仅贴纸窗口传 true）。
 */
export function bootAppearance(opts: { scale: boolean }): void {
  scaleEnabled = opts.scale;
  apply(readCache());
  // settings-changed 是权威实时源：先注册监听，一旦收到过，就让稍后才 resolve 的初始 getSettings
  // 不再覆盖（消除 fetch-vs-subscribe 竞态——否则 in-flight 的旧读取结果可能压掉刚到的新值）。
  let eventApplied = false;
  listen<Settings>("settings-changed", (e) => {
    eventApplied = true;
    const a = pick(e.payload);
    apply(a);
    writeCache(a);
  }).catch(() => {});
  // 远程收不到 Tauri 事件:mobile 入口轮询发现设置变化后派发 DOM 事件,这里同源套用
  // ——桌面改主题/贴纸色,手机不再要刷新才跟上。桌面窗永不派发此事件,零影响。
  if (remoteUi()) {
    window.addEventListener(REMOTE_SETTINGS_EVENT, (e) => {
      eventApplied = true;
      const a = pick((e as CustomEvent).detail as Partial<Settings>);
      apply(a);
      writeCache(a);
    });
  }
  getSettings()
    .then((s) => {
      if (eventApplied) return;
      const a = pick(s);
      apply(a);
      writeCache(a);
    })
    .catch(() => {});
  window
    .matchMedia("(prefers-color-scheme: light)")
    .addEventListener("change", () => {
      if (current.theme === "system") apply(current);
    });
}
