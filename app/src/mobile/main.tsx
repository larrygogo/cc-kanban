// 手机远程 UI 入口。与桌面 main.tsx 分家:不做窗口分流、不锁 DevTools、不 bootAppearance 界面密度,
// 只干三件事——置位远程门控、装 IPC 桥、把整个 ChatWindow 搬进真浏览器。
//
// 顺序要害:markRemoteUi + installRemoteTransport 必须先于任何会 invoke 的组件求值。故:
//  - 这两个副作用在模块体最早期同步执行(它们的 import 无副作用);
//  - ChatWindow / NewSessionPanel 用 React.lazy 延迟到渲染期加载,那时桥早已就位。
import React from "react";
import ReactDOM from "react-dom/client";
import { markRemoteUi, REMOTE_SETTINGS_EVENT } from "../remoteMode";
import { installRemoteTransport, NEW_SESSION_EVENT, getToken } from "./transport";
import { getSettings } from "../api";
import { TokenGate } from "./TokenGate";
import { I18nProvider } from "../i18n";
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

// settings-changed 的远程替身:12s 轮询 get_settings,内容变了就派发 REMOTE_SETTINGS_EVENT
// (appearance/i18n 订阅它跟随主题/语言)。设置改动是低频事件,12s 时延可接受;
// 未配对时不打(闸门页也用不上),失败静默等下一拍。
// 首拍也广播:手动配对路径下,boot 期的 getSettings 在存 token 前就 401 了、主题/语言
// 停在浏览器默认;apply 幂等,首拍重放正确值即修正,扫码路径下重放同值无副作用。
let lastSettingsJson = "";
window.setInterval(() => {
  if (!getToken()) return;
  void getSettings()
    .then((s) => {
      const json = JSON.stringify(s);
      if (json === lastSettingsJson) return;
      lastSettingsJson = json;
      window.dispatchEvent(new CustomEvent(REMOTE_SETTINGS_EVENT, { detail: s }));
    })
    .catch(() => {});
}, 12_000);

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
          <NewSessionPanel onClose={() => setNewSession(null)} prefill={newSession} />
        </div>
      )}
    </>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <I18nProvider>
      <React.Suspense fallback={null}>
        <TokenGate>
          <RemoteApp />
        </TokenGate>
      </React.Suspense>
      <TooltipLayer />
    </I18nProvider>
  </React.StrictMode>,
);
