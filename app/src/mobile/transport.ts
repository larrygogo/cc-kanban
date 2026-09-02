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
  // 重新配对 = 解除失效闩,后续 401 才能再次触发回闸门;主 token 换代通常意味着
  // server 已重启,旧 /file 降级凭据一并作废,清掉待重领。
  authLostAnnounced = false;
  fileToken = null;
}

export function clearToken(): void {
  try {
    localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* ignore */
  }
  fileToken = null;
}

/** 首帧从 URL fragment 收取 token 并立即清 hash(令牌不进服务器日志、不留地址栏历史)。
 *  必须发生在任何 rpc 之前:mobile/main.tsx 的 bootAppearance 在模块体里就会发
 *  get_settings,若此时 token 还躺在 hash 里没入库,那发 401 回来会把随后才存好的
 *  合法 token 一并清掉——首次扫码永远配对失败,就是这个竞态。 */
export function primeTokenFromHash(): void {
  const m = /(?:^#|[#&])token=([^&]+)/.exec(window.location.hash);
  if (!m) return;
  let token: string;
  try {
    token = decodeURIComponent(m[1]);
  } catch {
    token = m[1];
  }
  try {
    history.replaceState(null, "", window.location.pathname + window.location.search);
  } catch {
    window.location.hash = "";
  }
  setToken(token);
}

/** token 失效(401)时广播:TokenGate 监听后清态回到配对页,而非静默失败。
 *  闩住只发一次——四五条并发轮询同时 401 时,反复清态会让闸门反复重挂。 */
let authLostAnnounced = false;
function announceAuthLost(): void {
  if (authLostAnnounced) return;
  authLostAnnounced = true;
  clearToken();
  window.dispatchEvent(new CustomEvent(AUTH_LOST_EVENT));
}

/** 令牌试探的三态结果(见 probeToken)。 */
export type ProbeResult = "ok" | "unauthorized" | "unreachable";

/** 配对前的令牌试探(供 TokenGate 提交时先验后放行):打一发最轻的白名单命令。
 *  与主路径同一判据:200 + 非 JSON 是门户/反代劫持页,不算验证通过——否则闸门
 *  放行错令牌,用户进到一个每发请求都炸的界面。 */
export async function probeToken(token: string): Promise<ProbeResult> {
  try {
    const res = await fetch("/rpc/host_os", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Meowo-Token": token },
      body: "{}",
      signal: rpcSignal(8_000),
    });
    // 7M-6：401 是「令牌不对」,其余不通是「桌面端不可达」——两种情况用户要做的事
    // 完全不同(重抄令牌 vs 检查网络/唤醒桌面),不能都回一句「令牌不对或不可达」。
    if (res.status === 401) return "unauthorized";
    if (!res.ok) return "unreachable";
    JSON.parse(await res.text());
    return "ok";
  } catch {
    // 超时/DNS/连接拒绝/被劫持成非 JSON:一律算不可达。
    return "unreachable";
  }
}

/** /file 降级凭据:图片 <img src> 的 query 里不再携带主 token(泄露只丢「读图片目录」
 *  能力,不丢 /rpc 全量)。每次 server 启动新发,持主 token 经 /rpc 领取;领到前
 *  convertFileSrc 回退主 token(首屏短暂窗口,与旧行为等价)。 */
let fileToken: string | null = null;

export function primeFileToken(): void {
  if (fileToken || !getToken()) return;
  void rpc("file_access_token", {})
    .then((t) => {
      if (typeof t === "string" && t) fileToken = t;
    })
    .catch(() => {
      /* 领不到就一直用主 token 回退,功能不受损 */
    });
}

/** 请求超时护栏:挂起的 fetch 永不 settle 会把自调度轮询链(审批 tick、对话流 busyRef)
 *  永久冻死——手机换网/桌面休眠后 TCP 黑洞化是常态,必须有截止。 */
function rpcSignal(ms: number): AbortSignal | undefined {
  try {
    return AbortSignal.timeout(ms);
  } catch {
    return undefined; // 老浏览器没有 AbortSignal.timeout:退化为无超时,不因此崩
  }
}

export function onAuthLost(fn: () => void): () => void {
  window.addEventListener(AUTH_LOST_EVENT, fn);
  return () => window.removeEventListener(AUTH_LOST_EVENT, fn);
}

// 假订阅 id 计数器:plugin:event|listen 的返回值只用于后续 unlisten 配对,远程无真事件源,
// 给个自增数即可满足 @tauri-apps/api/event 的类型契约(它会 await 这个 Promise<number>)。
let fakeListenerId = 0;

/** 允许跑满 90s 的慢命令:大附件上传、spawn/接管类(new_session 冷启动 CLI 在慢机上
 *  轻松超 20s——客户端 abort 不撤销服务端副作用,超时误杀会导致「以为失败去重试,
 *  实际起了两个会话」)、深搜/大历史。其余命令(多为轮询)20s 封顶。 */
const SLOW_COMMANDS = new Set([
  "save_pasted_attachment",
  "new_session",
  "start_managed_terminal",
  "takeover_managed_terminal",
  "attach_background_session",
  "search_chat_transcripts",
  "get_chat_history",
]);

/// 每条命令等多久算超时。
/// 7M-4：此前只有 90s（慢命令）与 20s（其余）两档，而**轮询**也吃 20s——桌面端一睡，
/// 手机上要 20s×3 才判定失联（C-18 的连续三次），整整一分钟界面像在正常刷新。
/// 轮询的增量拉取（offset>0）压到 8s：它本来就该毫秒级返回，等更久没有意义。
/// 首读（offset=0）仍走 90s——长会话的 transcript 是真的大。
function rpcTimeout(cmd: string, args: unknown): number {
  if (cmd === "get_chat_history") {
    const offset = (args as { offset?: number } | null)?.offset ?? 0;
    return offset > 0 ? 8_000 : 90_000;
  }
  return SLOW_COMMANDS.has(cmd) ? 90_000 : 20_000;
}

async function rpc(cmd: string, args: unknown): Promise<unknown> {
  const token = getToken();
  // 只接受普通对象载荷:数组/TypedArray/字符串 stringify 后必被后端结构体拒收(400),
  // 二进制还会静默丢数据——在前端就抛清楚。
  if (args != null && (typeof args !== "object" || Array.isArray(args) || ArrayBuffer.isView(args))) {
    throw new Error(`remote/bad_payload:${cmd}`);
  }
  let res: Response;
  try {
    res = await fetch(`/rpc/${encodeURIComponent(cmd)}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Meowo-Token": token ?? "",
      },
      body: JSON.stringify(args ?? {}),
      signal: rpcSignal(rpcTimeout(cmd, args)),
    });
  } catch (e) {
    const name = (e as { name?: string } | null)?.name;
    if (name === "TimeoutError" || name === "AbortError") {
      throw new Error("remote/timeout");
    }
    throw e;
  }
  if (res.status === 401) {
    // 只有「带着当前有效 token 仍被拒」才算失效。空 token 的 401 是「还没配对」
    // (首帧 bootAppearance 的 get_settings 就会踩到),清态会误杀刚扫码存好的令牌;
    // 请求在途中用户重新配对(token 已换代)同理不清。
    if (token && token === getToken()) announceAuthLost();
    throw new Error("remote/unauthorized");
  }
  const text = await res.text();
  if (!res.ok) {
    // remote.rs 对齐 invoke reject:Err 侧回 JSON string。剥掉引号还原原始错误串,
    // 让 formatBackendError / errors.ts 的 sentinel 匹配链路照常工作。
    // 非 JSON(反代/门户劫持的整页 HTML)不上屏,收敛成状态码。
    let msg = "";
    try {
      const parsed = JSON.parse(text);
      if (typeof parsed === "string") msg = parsed;
    } catch {
      /* 落回状态码提示 */
    }
    throw new Error(msg || `remote/http_${res.status}`);
  }
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    // 200 + 非 JSON 只可能是中间层劫持(网关门户页等):当错误抛,别让调用方拿到
    // 字符串后在 .items 上炸出被吞掉的 TypeError,界面静默空白。
    throw new Error("remote/not_json");
  }
}

/** 安装远程传输层。必须在任何组件 import 触发 invoke 前调用(mobile/main.tsx 最早期)。 */
export function installRemoteTransport(): void {
  // 先收 hash 里的 token 再装桥:装完桥的下一行(bootAppearance)就会发第一发 rpc。
  primeTokenFromHash();
  // 已配对(含刚从 hash 收取):顺手领 /file 降级凭据;未配对时 TokenGate 验证通过后再领。
  primeFileToken();
  mockWindows("chat");
  mockConvertFileSrc("windows");

  // mockConvertFileSrc 装的是 asset://…,远程要改指内嵌 server 的 /file。直接覆写 internals 上的实现。
  const internals = (window as unknown as {
    __TAURI_INTERNALS__?: { convertFileSrc?: (path: string, protocol?: string) => string };
  }).__TAURI_INTERNALS__;
  if (internals) {
    internals.convertFileSrc = (path: string) => {
      // 优先降级凭据(primeFileToken 领取);未就绪时回退主 token,保首屏图片可加载。
      const token = fileToken ?? getToken() ?? "";
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
        if (!a?.url) return null;
        // scheme 白名单必须在这儿**再做一遍**:后端 open_link 对 http/https 之外一律拒绝
        // (settings.rs,理由是「任由 transcript 内容触发本地程序是注入通道」),但远程桥
        // 把这条命令在前端就地兑现,那道守卫在手机路径上等于不存在。模型输出里的链接
        // (对话 Markdown 与终端 OSC 8 走同一条通道)于是能把 intent://、market:// 这类
        // 自定义 scheme 直接交给手机 OS 的对应 app。错误串与后端逐字一致,errors.ts 的
        // sentinel 表现成中英文案。
        // 刻意**不传 base**:后端是 `url::Url::parse(&url)`,相对链接直接判无效。带 base
        // 会把 "ht!tp://%%%" 这类串当相对路径解析成同源 http 而放行,两端语义就分叉了。
        let scheme = "";
        try {
          scheme = new URL(a.url).protocol;
        } catch {
          throw new Error("无效链接");
        }
        if (scheme !== "http:" && scheme !== "https:") {
          throw new Error("只支持 http/https 链接");
        }
        window.open(a.url, "_blank", "noopener");
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
