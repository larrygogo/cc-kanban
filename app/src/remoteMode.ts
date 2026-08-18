// 手机远程 UI 的唯一门控标志。移动端入口(src/mobile/main.tsx)在渲染任何组件前置位,
// 桌面各窗永远读到 false——共享组件(ChatWindow/ChatSidebar/NewSessionPanel)据此裁掉
// 宿主执行类入口(终端视图/GitDiff/目录浏览/设置窗),桌面行为逐字不变。
//
// 单一标志刻意集中在这里:门控点散落在多个组件里,靠一个纯函数收口,便于审计「远程隐藏了什么」,
// 也让样式作用域(body.remote-ui)与逻辑门控共用同一判据,不产生第二处真值来源。

const FLAG = "__MEOWO_REMOTE__";

/** 置位远程模式。仅移动端入口在 import 副作用最早期调用一次。 */
export function markRemoteUi(): void {
  (globalThis as Record<string, unknown>)[FLAG] = true;
  if (typeof document !== "undefined") {
    document.body.classList.add("remote-ui");
  }
}

/** 是否运行在手机远程 UI。桌面所有窗恒为 false。 */
export function remoteUi(): boolean {
  return (globalThis as Record<string, unknown>)[FLAG] === true;
}
