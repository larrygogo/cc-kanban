// 设置窗口的共享状态层：设置对象的默认占位与读写 hook。
// 各 section（general/appearance/account）与主 About 共用，避免从 About.tsx 反向导入成环。
import { useRef, useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { getSettings, setSettings, type Settings } from "../../api";

export const SETTINGS_DEFAULTS: Settings = {
  notifications_enabled: true,
  attention_flash_enabled: true,
  auto_update_enabled: true,
  theme: "dark",
  // 与后端 settings.rs 的 default_opacity() 一致：曾占位 94，设置页读数先显 94% 再跳 100。
  opacity: 100,
  ui_scale: 100,
  resume_terminal: "terminal",
  language: "auto",
  terminal_open_mode: "card",
  session_open_in: "terminal",
  card_menu_mode: "button",
  preview_enabled: true,
  terminal_font_size: 12,
  terminal_line_height: "normal",
  // 占位与真实默认（appearance.ts / 后端 settings.rs）保持一致：flat / neutral。
  sticker_style: "flat",
  sticker_color: "neutral",
  // 首帧占位（get_settings() resolve 前）。真实默认值由后端 settings 给，前端不据此做任何判断。
  sticker_quota_providers: ["claude"],
  default_agent: "claude",
  proxy: { mode: "system", url: "", per_agent: {} },
  relay: { per_agent: {} },
};

// 设置读写：本地保留完整对象，每次只 patch 改动字段后整对象写回（后端 set_settings 收整对象，
// 漏字段会被 serde 默认值覆盖 → 必须整对象提交）。写失败则回读后端保持一致。
//
// patch 返回「错误文案 或 null」：后端会**拒收**非法配置（如代理地址填错），此前一律静默回滚，
// 用户只会看到输入框自己弹回去，不知道为什么。代理设置项要把这个原因显示出来。
// 既有调用方忽略返回值即可，行为不变。
export function useSettingsState() {
  const [settings, setSettingsState] = useState<Settings | null>(null);
  // 最近一次 patch 的失败原因（成功清空）。通用/会话/外观分区此前完全忽略 patch 返回值：
  // 后端拒收/写盘失败时开关「自己弹回去」零解释——正是本文件头注释想解决、却只在网络
  // 分区落实了的那件事。各分区渲染同一条错误行即可（见 About.tsx 的 SettingsError）。
  const [lastError, setLastError] = useState<string | null>(null);
  const ref = useRef<Settings>(SETTINGS_DEFAULTS);
  const loadRef = useRef<Promise<Settings> | null>(null);
  // 跨窗同步：设置窗与 Onboarding 同开时各持一份快照，整对象写回会把对方刚改的字段
  // 静默打回去（后端只保护 profiles/onboarding_seen）。订阅广播让本快照跟上任何来源
  // 的写入；自己 patch 引发的回声是同值覆盖，无害。
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    try {
      listen<Settings>("settings-changed", (event) => {
        ref.current = event.payload;
        setSettingsState(event.payload);
        loadRef.current = Promise.resolve(event.payload);
      })
        .then((dispose) => {
          if (cancelled) dispose();
          else unlisten = dispose;
        })
        .catch(() => {});
    } catch {
      /* 非 Tauri 环境（测试/浏览器） */
    }
    return () => {
      cancelled = true;
      try { unlisten?.(); } catch { /* noop */ }
    };
  }, []);
  // 所有整对象写必须串行。并发 set_settings 不仅会产生“旧请求最后落盘”，还会争用后端
  // 同一个 pid 临时文件；队列同时保证失败回读发生在下一次 patch 之前。
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const reload = (fresh = false): Promise<Settings> => {
    if (fresh || !loadRef.current) {
      loadRef.current = getSettings()
        .then((s) => {
          ref.current = s;
          setSettingsState(s);
          return s;
        })
        .catch((err: unknown) => {
          // 被拒的 Promise 不能留在缓存里：否则首读失败后，之后每次 reload() 都拿到同一个
          // 已拒 Promise——第一次 patch 必丢，还把真正的保存错误盖成误导性的首读错误。
          // 先清空再 rethrow，让下次调用重新拉取。（清空是安全的：本 handler 挂链最早，
          // 任何可能替换缓存的 reload(true) 都排在它之后执行。）
          loadRef.current = null;
          throw err;
        });
    }
    return loadRef.current;
  };
  useEffect(() => {
    void reload().catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const patch = (p: Partial<Settings>): Promise<string | null> => {
    const task = saveQueue.current.then(async (): Promise<string | null> => {
      try {
        // 首帧操作不再静默丢弃：等真实设置回来后再基于它合并。
        await reload();
        const next = { ...ref.current, ...p };
        ref.current = next;
        setSettingsState(next);
        await setSettings(next);
        setLastError(null);
        return null;
      } catch (err) {
        // 后端拒收：等待回读完成再放行队列中的下一次 patch，避免旧回读覆盖新操作。
        try {
          await reload(true);
        } catch {
          // 原始保存错误更有用；回读失败不覆盖它。
        }
        setLastError(String(err));
        return String(err);
      }
    });
    saveQueue.current = task.then(() => undefined);
    return task;
  };
  return [settings, patch, lastError] as const;
}
