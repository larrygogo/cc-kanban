// 配对闸门:决定手机端是否放行到 ChatWindow。令牌来源两条——
//  1. 扫码/点链接进来时 URL fragment `#token=…`(桌面二维码编码的正是它):进页即取走并清 hash,
//     令牌不进服务器日志、不留在地址栏历史。
//  2. 已配对过:localStorage 里有令牌,直接放行。
// 令牌失效(rpc 收 401)时 transport 广播 auth-lost,这里清态退回配对页,提示重新扫码。
import { useEffect, useState, type ReactNode } from "react";
import { getToken, setToken, clearToken, onAuthLost } from "./transport";

function takeTokenFromHash(): string | null {
  const hash = window.location.hash;
  const m = /(?:^#|[#&])token=([^&]+)/.exec(hash);
  if (!m) return null;
  let token: string;
  try {
    token = decodeURIComponent(m[1]);
  } catch {
    token = m[1];
  }
  // 清掉 hash:令牌不留在地址栏/历史。replaceState 不触发导航。
  try {
    history.replaceState(null, "", window.location.pathname + window.location.search);
  } catch {
    window.location.hash = "";
  }
  return token;
}

export function TokenGate({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState<boolean>(() => {
    const fromHash = takeTokenFromHash();
    if (fromHash) {
      setToken(fromHash);
      return true;
    }
    return getToken() != null;
  });
  const [manual, setManual] = useState("");

  useEffect(() => onAuthLost(() => setReady(false)), []);

  if (ready) return <>{children}</>;

  const submit = () => {
    const t = manual.trim();
    if (!t) return;
    setToken(t);
    setReady(true);
  };

  return (
    <div className="remote-gate">
      <div className="remote-gate-card">
        <h1>Meowo 远程</h1>
        <p>扫码后自动配对。若手动配对,粘贴桌面「远程访问」里的令牌:</p>
        <input
          type="password"
          inputMode="text"
          autoComplete="off"
          spellCheck={false}
          value={manual}
          onChange={(e) => setManual(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
          placeholder="访问令牌"
          aria-label="访问令牌"
        />
        <button onClick={submit} disabled={!manual.trim()}>
          连接
        </button>
      </div>
    </div>
  );
}

// 供入口在挂载前清理旧令牌(极少用,预留)。
export { clearToken };
