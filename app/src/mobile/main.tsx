// 手机远程 UI 入口。与桌面 main.tsx 分家:不做窗口分流、不锁 DevTools、不 bootAppearance 界面密度,
// 只干三件事——置位远程门控、装 IPC 桥、把整个 ChatWindow 搬进真浏览器。
//
// 顺序要害:markRemoteUi + installRemoteTransport 必须先于任何会 invoke 的组件求值。故:
//  - 这两个副作用在模块体最早期同步执行(它们的 import 无副作用);
//  - ChatWindow / NewSessionPanel 用 React.lazy 延迟到渲染期加载,那时桥早已就位。
import React, { useEffect } from "react";
import ReactDOM from "react-dom/client";
import { markRemoteUi, REMOTE_SETTINGS_EVENT, SELECT_SESSION_EVENT } from "../remoteMode";
import { installRemoteTransport, NEW_SESSION_EVENT, getToken } from "./transport";
import { getSettings } from "../api";
import { TokenGate } from "./TokenGate";
import { I18nProvider, useT } from "../i18n";
import { TooltipLayer } from "../Tooltip";
import { installInputModality } from "../input-modality";
import { bootAppearance } from "../appearance";
import { setHostOsUnknown } from "../platform";
import "../fonts";
import "../styles.css";
import "./mobile.css";

markRemoteUi();
installRemoteTransport();
installInputModality();
bootAppearance({ scale: false });
// 远程端 hostOs 语义未定义:显式钉死 unknown(isMac()/isWindows()/isMacPanel() 恒 false,
// 与此前从未探测的隐式行为一致)。约定:不要用它们门控远程行为,远程布局门控一律用 remoteUi()。
setHostOsUnknown();

// 7M-3:iOS 软键盘弹出时 dvh **不变**,WebKit 直接把整页向上顶(window.scrollY>0),
// 100dvh 的根容器于是有一截被推出屏幕——底部的 composer 正好落在键盘后面。
// visualViewport 才是「此刻真正看得见的那块」,用它写 --app-height 给布局吃;
// 每次变化顺手滚回原点,不让被顶起的顶栏留在屏外。安卓侧由 mobile.html 的
// interactive-widget=resizes-content 让浏览器自己缩内容,这段是幂等的兜底。
const viewport = window.visualViewport;
if (viewport) {
  const syncAppHeight = () => {
    // 双指放大（7M-11 放开了缩放）时 visualViewport.height 按缩放比缩小、同样触发 resize，
    // 但那不是键盘：照写进去会把整页压成缩放后的可视高度并强制滚回原点，布局塌陷。
    // 缩放态一律不动，恢复到 1 时 resize 再来一次自然同步。
    if (viewport.scale > 1.01) return;
    document.documentElement.style.setProperty("--app-height", `${Math.round(viewport.height)}px`);
    window.scrollTo(0, 0);
  };
  viewport.addEventListener("resize", syncAppHeight);
  syncAppHeight();
}

// settings-changed 的远程替身:12s 轮询 get_settings,内容变了就派发 REMOTE_SETTINGS_EVENT
// (appearance/i18n 订阅它跟随主题/语言)。设置改动是低频事件,12s 时延可接受;
// 未配对时不打(闸门页也用不上),失败静默等下一拍。
// 首拍也广播:手动配对路径下,boot 期的 getSettings 在存 token 前就 401 了、主题/语言
// 停在浏览器默认;apply 幂等,首拍重放正确值即修正,扫码路径下重放同值无副作用。
let lastSettingsJson = "";
// 7M-5：轮询体抽出来,好在「刚拿到令牌」的那一刻立刻打一发——只靠 setInterval 的话
// 手动配对后要等最多 12s 才纠正主题/语言,中途整页闪一次换色。
const pullSettings = () => {
  if (!getToken()) return;
  void getSettings()
    .then((s) => {
      const json = JSON.stringify(s);
      if (json === lastSettingsJson) return;
      lastSettingsJson = json;
      window.dispatchEvent(new CustomEvent(REMOTE_SETTINGS_EVENT, { detail: s }));
    })
    .catch(() => {});
};
// 扫码路径:令牌在装桥时就从 hash 收走了,这里已能直接对齐。
pullSettings();
window.setInterval(pullSettings, 12_000);

const ChatWindow = React.lazy(() =>
  import("../views/ChatWindow").then((m) => ({ default: m.ChatWindow })),
);
const NewSessionPanel = React.lazy(() =>
  import("../views/NewSessionPanel").then((m) => ({ default: m.NewSessionPanel })),
);

function RemoteApp() {
  // 新建会话在桌面开独立窗;远程转页内事件,这里叠加渲染 NewSessionPanel(会话列表/对话仍在底层)。
  // 事件 detail 里带卡片菜单的 cwd/provider 预填——桌面走 URL query / ns-prefill,远程两条都不通。
  const [newSession, setNewSession] = React.useState<null | { cwd?: string | null; provider?: string | null }>(null);
  React.useEffect(() => {
    const open = (e: Event) => {
      const d = (e as CustomEvent).detail as { cwd?: string | null; provider?: string | null } | undefined;
      setNewSession({ cwd: d?.cwd, provider: d?.provider });
    };
    window.addEventListener(NEW_SESSION_EVENT, open);
    return () => window.removeEventListener(NEW_SESSION_EVENT, open);
  }, []);
  return (
    <>
      <ChatWindow />
      {newSession && (
        <div className="remote-new-session-overlay">
          <NewSessionPanel
            onClose={() => setNewSession(null)}
            prefill={newSession}
            // 启动成功:临时负 id 经页内事件送 ChatWindow 选中新会话(侧栏点选的同一通道;
            // 桥不 reveal,没有这一步用户会落回「去侧栏选会话」空态)。null 不导航。
            onLaunched={(tempId) => {
              if (typeof tempId === "number") {
                window.dispatchEvent(new CustomEvent(SELECT_SESSION_EVENT, { detail: tempId }));
              }
            }}
          />
        </div>
      )}
    </>
  );
}

/// 7M-1：ChatWindow 那个 chunk 有 1.2MB,走 Tailscale 中继要好几秒。fallback 是 null 时
/// 这几秒是纯白屏——在手机上和「点了没反应/连挂了」无法区分。给一行同底色的加载态。
/// 页面标题跟随界面语言（7M-7 尾：mobile.html 里硬编码「Meowo 远程」，英文界面下
/// 浏览器标签页仍是中文）。首帧的静态 title 由 mobile.html 给，这里在语言就绪后改写。
function RemoteTitle() {
  const t = useT();
  useEffect(() => { document.title = t.remote.gateTitle; }, [t]);
  return null;
}

function RemoteBoot() {
  const t = useT();
  return <div className="remote-boot" role="status">{t.remote.gateLoading}</div>;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <I18nProvider>
      <React.Suspense fallback={<RemoteBoot />}>
        <TokenGate onReady={pullSettings}>
          <RemoteApp />
        </TokenGate>
      </React.Suspense>
      <RemoteTitle />
      <TooltipLayer />
    </I18nProvider>
  </React.StrictMode>,
);
