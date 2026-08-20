import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { App } from "./App";
import { TooltipLayer } from "./Tooltip";
import { lockdownInProduction, suppressNativeFind } from "./devtools-guard";
import { installInputModality } from "./input-modality";
import { bootAppearance } from "./appearance";
import { detectHostOs } from "./platform";
import { I18nProvider } from "./i18n";
import "./fonts"; // 内置字体集的唯一入口(取舍理由见 fonts.ts;demo/poster 也走它,不再各自 import)
import "./styles.css";

// 除常驻的贴纸窗(App)外,其余窗口全部懒加载:此前 7 个窗口共享一个 963KB 的静态 chunk,
// 贴纸窗启动要解析它根本用不到的 xterm(~250KB)与 react-markdown(~150KB)——它们只从
// ChatWindow 可达,拆开后各窗口只付自己那份。窗口本就 visible:false、由前端量好再 show
// (useShowWhenReady),懒加载多的一拍不产生可见闪烁。
const About = React.lazy(() => import("./views/About").then((m) => ({ default: m.About })));
const Updater = React.lazy(() => import("./views/Updater").then((m) => ({ default: m.Updater })));
const NewSessionPanel = React.lazy(() =>
  import("./views/NewSessionPanel").then((m) => ({ default: m.NewSessionPanel })),
);
const Onboarding = React.lazy(() =>
  import("./views/Onboarding").then((m) => ({ default: m.Onboarding })),
);
const ChatWindow = React.lazy(() =>
  import("./views/ChatWindow").then((m) => ({ default: m.ChatWindow })),
);
const ConfirmWindow = React.lazy(() =>
  import("./views/ConfirmWindow").then((m) => ({ default: m.ConfirmWindow })),
);

// E2E 构建（VITE_E2E=1）才注入 @wdio/tauri-plugin 前端桥（console 转发 / invoke 拦截 /
// window.wdioTauri）。生产构建下 VITE_E2E 未定义，该动态 import 被 vite 死代码消除，
// 这个 devDependency 不进产物。见 app/e2e/README.md。
if (import.meta.env.VITE_E2E === "1") {
  void import("@wdio/tauri-plugin");
}

// 正式构建下封死右键菜单与 DevTools 快捷键（dev 放行）。
lockdownInProduction();

// 全部窗口按掉 WebView 原生页内查找条（Ctrl+F/F3），应用内搜索自行接管。
suppressNativeFind();

// 焦点框只在键盘导航时显示（避免打开面板时 WKWebView 自动聚焦首元素亮起 UA 焦点框）。
installInputModality();

// 平台标记（同步，供 CSS 做平台差异，如 macOS 无边框设置窗需自行圆角）。WKWebView 的 UA 含 "Mac"。
if (/Mac/i.test(navigator.userAgent)) {
  document.documentElement.classList.add("platform-macos");
}

// 同一份前端按窗口 label 分流：about 窗口渲染设置页、updater 渲染更新页，其余渲染主贴纸。
const label = (() => {
  try {
    return getCurrentWindow().label;
  } catch {
    return "main";
  }
})();

// 套用外观设置（明暗/不透明度各窗都套；界面密度仅贴纸窗口）。
bootAppearance({ scale: label === "main" });

// 渲染前先探测宿主平台：isMacPanel 等同步判定在首帧与各 effect 中即正确，
// 消除「effect 跑在探测 resolve 前、guard 固化为 false」的竞态。
// detectHostOs 内部兜底（非 Tauri 环境立即落为 other），不会悬挂。
void detectHostOs().then(() => {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <I18nProvider>
        <React.Suspense fallback={null}>
          {label === "about" ? (
            <About />
          ) : label === "updater" ? (
            <Updater />
          ) : label === "new-session" ? (
            <NewSessionPanel />
          ) : label === "onboarding" ? (
            <Onboarding />
          ) : label === "chat" ? (
            <ChatWindow />
          ) : label.startsWith("confirm-") ? (
            <ConfirmWindow />
          ) : (
            <App />
          )}
        </React.Suspense>
        <TooltipLayer />
      </I18nProvider>
    </React.StrictMode>,
  );
});
