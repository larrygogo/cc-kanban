// 轻量 i18n：嵌套字典 + context。语言来源 Settings.language（auto/zh/en，auto 按
// navigator.language 解析）；仿 appearance.ts——localStorage 缓存防首屏闪错语言，
// settings-changed 实时切换并消除 fetch-vs-subscribe 竞态。
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { getSettings, type Settings, type RepairReason } from "../api";
import { remoteUi, REMOTE_SETTINGS_EVENT } from "../remoteMode";
import { zh, type Dict } from "./zh";
import { en } from "./en";

export type Lang = "zh" | "en";

const CACHE_KEY = "meowo-lang";

export function resolveLang(setting: string | undefined): Lang {
  if (setting === "zh" || setting === "en") return setting;
  return /^zh\b|^zh-/i.test(navigator.language) ? "zh" : "en";
}

function readCache(): Lang {
  const c = localStorage.getItem(CACHE_KEY);
  return c === "en" ? "en" : c === "zh" ? "zh" : resolveLang(undefined);
}

const DICTS: Record<Lang, Dict> = { zh, en };
const I18nCtx = createContext<Dict>(zh);

/** 取当前语言字典：const t = useT(); t.tabs.all */
export function useT(): Dict {
  return useContext(I18nCtx);
}

/** 语言选择器的选项表（设置页与引导页共用一份）。「中文 / English」是语言自称，刻意不翻译。 */
export function languageOptions(t: Dict): { value: "auto" | "zh" | "en"; label: string }[] {
  return [
    { value: "auto", label: t.settings.langAuto },
    { value: "zh", label: "中文" },
    { value: "en", label: "English" },
  ];
}

/** 「修复连接」失败原因 → 本地化提示。reason=null 但仍失败（罕见边界）落到泛化文案。 */
export function repairFailMessage(t: Dict, reason: RepairReason | null): string {
  switch (reason) {
    case "need-login":
      return t.newSession.repairNeedLogin;
    case "reporter-not-found":
      return t.newSession.repairNoReporter;
    case "not-detected":
      return t.newSession.repairNotDetected;
    default:
      return t.newSession.repairFailed;
  }
}

export function I18nProvider({ children, initial }: { children: ReactNode; initial?: Lang }) {
  const [lang, setLang] = useState<Lang>(() => initial ?? readCache());
  // <html lang> 跟随当前语言（屏幕阅读器朗读、字体回退、连字符断行都依赖它）。
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);
  useEffect(() => {
    if (initial) return; // 测试注入固定语言时不订阅
    let eventApplied = false;
    // cleanup 可能先于 listen resolve：cancelled 标记保证 resolve 后立即注销，防监听器泄漏。
    let cancelled = false;
    let un: (() => void) | undefined;
    const apply = (s: Settings) => {
      const l = resolveLang(s.language);
      setLang(l);
      try { localStorage.setItem(CACHE_KEY, l); } catch { /* ignore */ }
    };
    try {
      listen<Settings>("settings-changed", (e) => { eventApplied = true; apply(e.payload); })
        .then((f) => {
          if (cancelled) f();
          else un = f;
        })
        .catch(() => {});
    } catch { /* 非 Tauri 环境 */ }
    // 远程收不到 Tauri 事件:mobile 入口轮询派发的 DOM 事件同源切语言(桌面永不派发)。
    const onRemote = (e: Event) => {
      eventApplied = true;
      apply((e as CustomEvent).detail as Settings);
    };
    if (remoteUi()) window.addEventListener(REMOTE_SETTINGS_EVENT, onRemote);
    getSettings().then((s) => { if (!cancelled && !eventApplied) apply(s); }).catch(() => {});
    return () => {
      cancelled = true;
      un?.();
      if (remoteUi()) window.removeEventListener(REMOTE_SETTINGS_EVENT, onRemote);
    };
  }, [initial]);
  return <I18nCtx.Provider value={DICTS[lang]}>{children}</I18nCtx.Provider>;
}
