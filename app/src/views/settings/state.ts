// 设置窗口的共享状态层：设置对象的默认占位与读写 hook。
// 各 section（general/appearance/account）与主 About 共用，避免从 About.tsx 反向导入成环。
import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { getSettings, setSettings, type Settings } from "../../api";
import { useT } from "../../i18n";
import { formatBackendError } from "../../i18n/errors";

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
  chat_enabled: true,
  card_menu_mode: "button",
  preview_enabled: true,
  click_through_enabled: false,
  terminal_font_size: 12,
  terminal_line_height: "normal",
  terminal_scrollback: 5000,
  // 占位与真实默认（appearance.ts / 后端 settings.rs）保持一致：flat / neutral。
  sticker_style: "flat",
  sticker_color: "neutral",
  // 首帧占位（get_settings() resolve 前）。真实默认值由后端 settings 给，前端不据此做任何判断。
  sticker_quota_providers: ["claude"],
  default_agent: "claude",
  proxy: { mode: "system", url: "", per_agent: {} },
  relay: { per_agent: {} },
  remote_access_enabled: false,
  remote_access_port: 18620,
  remote_access_bind: "all",
  remote_access_token: "",
};

// ── 模块级共享态（每个 webview 窗口一份）──
// S-2 后各分区常驻挂载、各持一份 useSettingsState 实例：若快照/队列留在实例级，「所有整
// 对象写必须串行」只在单实例内成立——A 分区改完广播未到时，B 分区基于过旧快照整对象写回，
// 会把 A 刚改的字段静默打回（写-写竞态）。故快照/首读缓存/保存队列提为模块级，串行跨实例
// 生效；value/lastError 仍是各实例自己的 state（错误行只出现在发起操作的那个分区）。
const sharedRef: { current: Settings } = { current: SETTINGS_DEFAULTS };
const sharedLoadRef: { current: Promise<Settings> | null } = { current: null };
// 所有整对象写必须串行。并发 set_settings 不仅会产生“旧请求最后落盘”，还会争用后端
// 同一个 pid 临时文件；队列同时保证失败回读发生在下一次 patch 之前。
const sharedSaveQueue: { current: Promise<void> } = { current: Promise.resolve() };
// 没有任何实例挂载时（设置窗关闭 / 测试 cleanup）把共享态归零：下次开窗重新拉取，
// 与旧「每实例各持一份」的窗口生命周期行为一致，也避免测试间串味。
let sharedSubscribers = 0;
const resetShared = () => {
  sharedRef.current = SETTINGS_DEFAULTS;
  sharedLoadRef.current = null;
  sharedSaveQueue.current = Promise.resolve();
};

// 设置读写：本地保留完整对象，每次只 patch 改动字段后整对象写回（后端 set_settings 收整对象，
// 漏字段会被 serde 默认值覆盖 → 必须整对象提交）。写失败则回读后端保持一致。
//
// patch 返回「错误文案 或 null」：后端会**拒收**非法配置（如代理地址填错），此前一律静默回滚，
// 用户只会看到输入框自己弹回去，不知道为什么。代理设置项要把这个原因显示出来。
// 既有调用方忽略返回值即可，行为不变。
export function useSettingsState() {
  const t = useT();
  const [settings, setSettingsState] = useState<Settings | null>(null);
  // 最近一次 patch 的失败原因（成功清空）。通用/会话/外观分区此前完全忽略 patch 返回值：
  // 后端拒收/写盘失败时开关「自己弹回去」零解释——正是本文件头注释想解决、却只在网络
  // 分区落实了的那件事。各分区渲染同一条错误行即可（见 About.tsx 的 SettingsError）。
  const [lastError, setLastError] = useState<string | null>(null);
  // 跨窗同步：设置窗与 Onboarding 同开时各持一份快照，整对象写回会把对方刚改的字段
  // 静默打回去（后端只保护 profiles/onboarding_seen）。订阅广播让本快照跟上任何来源
  // 的写入；自己 patch 引发的回声是同值覆盖，无害。
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    try {
      listen<Settings>("settings-changed", (event) => {
        sharedRef.current = event.payload;
        setSettingsState(event.payload);
        sharedLoadRef.current = Promise.resolve(event.payload);
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
  const reload = (fresh = false): Promise<Settings> => {
    if (fresh || !sharedLoadRef.current) {
      sharedLoadRef.current = getSettings()
        .then((s) => {
          sharedRef.current = s;
          setSettingsState(s);
          return s;
        })
        .catch((err: unknown) => {
          // 被拒的 Promise 不能留在缓存里：否则首读失败后，之后每次 reload() 都拿到同一个
          // 已拒 Promise——第一次 patch 必丢，还把真正的保存错误盖成误导性的首读错误。
          // 先清空再 rethrow，让下次调用重新拉取。（清空是安全的：本 handler 挂链最早，
          // 任何可能替换缓存的 reload(true) 都排在它之后执行。）
          sharedLoadRef.current = null;
          throw err;
        });
    } else {
      // 复用在途/已缓存的共享 Promise 时，本实例的 state 也要跟上：上面 .then 里的
      // setSettingsState 只属于创建该 Promise 的那个实例，复用者不调会永远停在 null。
      void sharedLoadRef.current.then((s) => setSettingsState(s)).catch(() => {});
    }
    return sharedLoadRef.current;
  };
  useEffect(() => {
    // 共享态生命周期跟随挂载实例数：全部卸载即重置（见上面 resetShared 注释）。
    sharedSubscribers += 1;
    void reload().catch(() => {});
    return () => {
      sharedSubscribers -= 1;
      if (sharedSubscribers <= 0) {
        sharedSubscribers = 0;
        resetShared();
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const patch = (p: Partial<Settings>): Promise<string | null> => {
    const task = sharedSaveQueue.current.then(async (): Promise<string | null> => {
      try {
        // 首帧操作不再静默丢弃：等真实设置回来后再基于它合并。
        await reload();
        const next = { ...sharedRef.current, ...p };
        sharedRef.current = next;
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
        // 产出端统一过 formatBackendError：后端 sentinel 是中文，英文界面此前直出中文
        // 原文。各消费点（NetworkSection save / RelayAccess saveRule / SettingsError）
        // 拿到的都已是当前语言文案。
        const msg = formatBackendError(err, t.locale);
        setLastError(msg);
        return msg;
      }
    });
    sharedSaveQueue.current = task.then(() => undefined);
    return task;
  };
  const clearError = () => setLastError(null);
  return [settings, patch, lastError, clearError] as const;
}
