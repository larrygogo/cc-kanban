// 配对闸门:决定手机端是否放行到 ChatWindow。令牌来源两条——
//  1. 扫码/点链接进来时 URL fragment `#token=…`(桌面二维码编码的正是它):transport 在装桥前
//     就收走并清 hash(primeTokenFromHash,时序原因见其注释),这里只读 localStorage。
//  2. 已配对过:localStorage 里有令牌,直接放行。
// 令牌失效(rpc 收 401)时 transport 广播 auth-lost,这里清态退回配对页,并说明原因——
// 静默弹回而不解释,用户只会反复点「连接」。
import { useEffect, useState, type ReactNode } from "react";
import { getToken, setToken, clearToken, onAuthLost, probeToken, primeFileToken } from "./transport";

export function TokenGate({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState<boolean>(() => getToken() != null);
  const [manual, setManual] = useState("");
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<null | "invalid" | "expired">(null);

  // 401 弹回配对页:此时 hash 早清了,刷新也回不去,必须把「需要重新扫码」说出来。
  useEffect(() => onAuthLost(() => {
    setReady(false);
    setError("expired");
  }), []);

  if (ready) return <>{children}</>;

  const submit = () => {
    const t = manual.trim();
    if (!t || checking) return;
    setChecking(true);
    setError(null);
    // 先验后放行:直接 setReady 的话,错令牌要等第一发业务请求 401 才弹回,
    // 界面闪一下又回到原地,零解释。
    void probeToken(t).then((ok) => {
      setChecking(false);
      if (ok) {
        setToken(t);
        primeFileToken();
        setReady(true);
      } else {
        setError("invalid");
      }
    });
  };

  return (
    <div className="remote-gate">
      <div className="remote-gate-card">
        <h1>Meowo 远程</h1>
        <p>扫码后自动配对。若手动配对,粘贴桌面「远程访问」里的令牌:</p>
        {error === "expired" && (
          <p className="remote-gate-err">桌面端令牌已更换或失效,请重新扫码,或粘贴新令牌</p>
        )}
        {error === "invalid" && (
          <p className="remote-gate-err">令牌不对或桌面端不可达,请核对后重试</p>
        )}
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
        <button onClick={submit} disabled={!manual.trim() || checking}>
          {checking ? "正在连接…" : "连接"}
        </button>
      </div>
    </div>
  );
}

// 供入口在挂载前清理旧令牌(极少用,预留)。
export { clearToken };
