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

/** open_new_session_window 在远程转成的页内导航事件（transport 派发、mobile 入口渲染
 *  sheet、ChatWindow 借它收窄屏抽屉）。放这里而非 mobile/transport：桌面组件也要监听,
 *  不能反向 import 移动端模块。 */
export const NEW_SESSION_EVENT = "meowo:remote-new-session";

/** 远程新建会话启动成功后的页内导航事件（detail = 临时负 id）：NewSessionPanel 经
 *  RemoteApp 派发、ChatWindow 监听后走侧栏点选同一条 resetTo 通道选中新会话。
 *  没有它，新建后用户落回「去侧栏选会话」空态——桥不 reveal，新会话无从定位。 */
export const SELECT_SESSION_EVENT = "meowo:remote-select-session";

/** settings-changed 的远程替身:Tauri push 事件到不了浏览器,mobile 入口轮询 get_settings
 *  发现变化后派发此 DOM 事件(detail = 完整 Settings),appearance/i18n 等共享消费方在
 *  remoteUi() 下额外订阅它——桌面改主题/语言,手机不再要刷新才跟上。 */
export const REMOTE_SETTINGS_EVENT = "meowo:remote-settings-changed";
