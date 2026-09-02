// 配对闸门:决定手机端是否放行到 ChatWindow。令牌来源两条——
//  1. 扫码/点链接进来时 URL fragment `#token=…`(桌面二维码编码的正是它):transport 在装桥前
//     就收走并清 hash(primeTokenFromHash,时序原因见其注释),这里只读 localStorage。
//  2. 已配对过:localStorage 里有令牌,直接放行。
// 令牌失效(rpc 收 401)时 transport 广播 auth-lost,这里清态退回配对页,并说明原因——
// 静默弹回而不解释,用户只会反复点「连接」。
import { useEffect, useState, type ReactNode } from "react";
import { getToken, setToken, clearToken, onAuthLost, probeToken, primeFileToken } from "./transport";
import { useT } from "../i18n";

export function TokenGate({ children, onReady }: { children: ReactNode; onReady?: () => void }) {
  const t = useT();
  const [ready, setReady] = useState<boolean>(() => getToken() != null);
  const [manual, setManual] = useState("");
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<null | "invalid" | "unreachable" | "expired">(null);

  // 401 弹回配对页:此时 hash 早清了,刷新也回不去,必须把「需要重新扫码」说出来。
  useEffect(() => onAuthLost(() => {
    setReady(false);
    setError("expired");
  }), []);

  if (ready) return <>{children}</>;

  const submit = () => {
    const token = manual.trim();
    if (!token || checking) return;
    setChecking(true);
    setError(null);
    // 先验后放行:直接 setReady 的话,错令牌要等第一发业务请求 401 才弹回,
    // 界面闪一下又回到原地,零解释。
    void probeToken(token).then((result) => {
      setChecking(false);
      if (result === "ok") {
        setToken(token);
        primeFileToken();
        setReady(true);
        // 令牌刚落地:立刻对齐一次主题/语言,不等 12s 的下一拍(7M-5)。
        onReady?.();
      } else {
        // 7M-6:令牌错 → 重抄令牌;连不上 → 检查网络/看桌面端还在不在。
        setError(result === "unauthorized" ? "invalid" : "unreachable");
      }
    });
  };

  return (
    <div className="remote-gate">
      <div className="remote-gate-card">
        <h1>{t.remote.gateTitle}</h1>
        <p>{t.remote.gateHint}</p>
        {error === "expired" && <p className="remote-gate-err" role="alert">{t.remote.gateErrExpired}</p>}
        {error === "invalid" && <p className="remote-gate-err" role="alert">{t.remote.gateErrInvalid}</p>}
        {error === "unreachable" && <p className="remote-gate-err" role="alert">{t.remote.gateErrUnreachable}</p>}
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
          placeholder={t.remote.gateTokenLabel}
          aria-label={t.remote.gateTokenLabel}
        />
        <button onClick={submit} disabled={!manual.trim() || checking}>
          {checking ? t.remote.gateConnecting : t.remote.gateConnect}
        </button>
      </div>
    </div>
  );
}

// 供入口在挂载前清理旧令牌(极少用,预留)。
export { clearToken };
