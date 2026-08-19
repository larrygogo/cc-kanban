// 手机远程 UI 的 IPC 桥。桌面用 Tauri invoke(走 IPC 到 Rust command);手机在真浏览器里没有
// Tauri runtime,这里用 @tauri-apps/api/mocks 的 mockIPC 接管 `window.__TAURI_INTERNALS__.invoke`,
// 把每次 invoke(cmd,args) 转成 `POST /rpc/<cmd>`(remote.rs 白名单直调同名 command 本体)。
// ChatWindow/ChatSidebar 等组件因此零改动跑在手机上。
//
// 三类命令特殊处理,不进 /rpc:
//  1. plugin:* —— Tauri 插件桥(event/window/dialog…)。远程没有真窗口:listen 给假订阅 id、
//     其余 no-op。事件通道 v1 全轮询,listen 拿不到推送不影响正确性(见各 hook 的轮询兜底)。
//  2. 宿主执行类(open_link/open_url/open_new_session_window/open_settings/confirm_dialog…)——
//     后端刻意不放行(default-deny)。这里在前端就地兑现浏览器能做的部分(开链接/确认),
//     或转成页内事件(新建会话),其余静默 no-op。
//  3. convertFileSrc —— 图片资源。桌面走 asset:// 协议,远程改指 `/file?path=&token=`(Message.tsx 零改)。
import { mockIPC, mockWindows, mockConvertFileSrc } from "@tauri-apps/api/mocks";
// open_new_session_window 转成的页内导航事件。常量本体在 remoteMode(ChatWindow 也要
// 监听它收抽屉,不能反向 import 移动端模块),这里转出口保持 mobile 侧引用不变。
import { NEW_SESSION_EVENT } from "../remoteMode";

const TOKEN_KEY = "meowo.remote.token";
const AUTH_LOST_EVENT = "meowo:remote-auth-lost";
export { NEW_SESSION_EVENT };

export function getToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setToken(token: string): void {
  try {
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    /* 隐私模式禁写:留内存态,刷新即回配对页,可接受 */
  }
}

export function clearToken(): void {
  try {
    localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* ignore */
  }
}

/** token 失效(401)时广播:TokenGate 监听后清态回到配对页,而非静默失败。 */
function announceAuthLost(): void {
  clearToken();
  window.dispatchEvent(new CustomEvent(AUTH_LOST_EVENT));
}

export function onAuthLost(fn: () => void): () => void {
  window.addEventListener(AUTH_LOST_EVENT, fn);
  return () => window.removeEventListener(AUTH_LOST_EVENT, fn);
}

// 假订阅 id 计数器:plugin:event|listen 的返回值只用于后续 unlisten 配对,远程无真事件源,
// 给个自增数即可满足 @tauri-apps/api/event 的类型契约(它会 await 这个 Promise<number>)。
let fakeListenerId = 0;

async function rpc(cmd: string, args: unknown): Promise<unknown> {
  const token = getToken();
  const res = await fetch(`/rpc/${encodeURIComponent(cmd)}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Meowo-Token": token ?? "",
    },
    body: JSON.stringify(args ?? {}),
  });
  if (res.status === 401) {
    announceAuthLost();
    throw new Error("远程访问令牌已失效,请重新扫码配对");
  }
  const text = await res.text();
  if (!res.ok) {
    // remote.rs 对齐 invoke reject:Err 侧回 JSON string。剥掉引号还原原始错误串,
    // 让 formatBackendError / errors.ts 的 sentinel 匹配链路照常工作。
    let msg = text;
    try {
      const parsed = JSON.parse(text);
      if (typeof parsed === "string") msg = parsed;
    } catch {
      /* 非 JSON 原样抛 */
    }
    throw new Error(msg || `远程请求失败(${res.status})`);
  }
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/** 安装远程传输层。必须在任何组件 import 触发 invoke 前调用(mobile/main.tsx 最早期)。 */
export function installRemoteTransport(): void {
  mockWindows("chat");
  mockConvertFileSrc("windows");

  // mockConvertFileSrc 装的是 asset://…,远程要改指内嵌 server 的 /file。直接覆写 internals 上的实现。
  const internals = (window as unknown as {
    __TAURI_INTERNALS__?: { convertFileSrc?: (path: string, protocol?: string) => string };
  }).__TAURI_INTERNALS__;
  if (internals) {
    internals.convertFileSrc = (path: string) => {
      const token = getToken() ?? "";
      return `/file?path=${encodeURIComponent(path)}&token=${encodeURIComponent(token)}`;
    };
  }

  mockIPC(async (cmd, args) => {
    // — 事件/窗口/插件桥:远程无真窗口,给最小可用桩 —
    if (cmd === "plugin:event|listen") {
      fakeListenerId += 1;
      return fakeListenerId;
    }
    if (cmd.startsWith("plugin:")) {
      // unlisten/emit/window|* 等一律 no-op(轮询通道不依赖它们)
      return null;
    }

    // — 宿主执行类:后端 default-deny,前端就地兑现或转页内 —
    switch (cmd) {
      case "confirm_dialog": {
        // 真浏览器里 window.confirm 可用(桌面 Tauri webview 会吞掉才改走原生小窗,见 confirm.tsx)。
        const a = args as { message?: string; title?: string } | undefined;
        const text = [a?.title, a?.message].filter(Boolean).join("\n\n");
        return window.confirm(text || "确认?");
      }
      case "open_link":
      case "open_url": {
        const a = args as { url?: string } | undefined;
        if (a?.url) window.open(a.url, "_blank", "noopener");
        return null;
      }
      case "open_new_session_window": {
        // 桌面开独立新建窗;远程转页内导航事件,mobile 入口叠加渲染 NewSessionPanel。
        window.dispatchEvent(new CustomEvent(NEW_SESSION_EVENT, { detail: args ?? {} }));
        return null;
      }
      case "open_settings":
      case "open_path_with":
        // 设置页/系统打开在远程不提供,静默忽略(门控已隐藏入口,这里是兜底)
        return null;
    }

    // — 其余:白名单 command,转发到内嵌 server —
    return rpc(cmd, args);
  });
}
