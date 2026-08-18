// 设置页「网络」分区顶部：手机远程访问（局域网/Tailscale + token）开关与扫码配对。
//
// token 是进门凭据，绝不上网线：二维码把它放 URL fragment（`#token=`，不进服务端日志
// 也不随请求发出），手机端首次打开即存 localStorage 并清掉 hash。开关/端口经 set_settings
// 落盘后由后端 remote::apply 热生效；token 由 remote_access_info 惰性生成（首次开启才产生）。
import { useCallback, useEffect, useState } from "react";
import { renderSVG } from "uqr";
import { remoteAccessInfo, type RemoteAccessInfo } from "../../api";
import { useT } from "../../i18n";
import { useSettingsState } from "./state";
import { Switch } from "./widgets";
import { Dropdown } from "../menu";

/// 端口输入：本地草稿 + 失焦/回车提交（同 NetworkSection 的 UrlInput，避免每键一存打后端）。
function PortInput({ value, onCommit }: { value: number; onCommit: (port: number) => void }) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const n = Number.parseInt(text.trim(), 10);
    // 合法端口才提交；非法（空/越界）回退磁盘值，不落盘。
    if (Number.isInteger(n) && n >= 1 && n <= 65535 && n !== value) onCommit(n);
    else setText(String(value));
  };
  return (
    <input
      className="ns-input remote-port-input"
      type="text"
      inputMode="numeric"
      autoComplete="off"
      spellCheck={false}
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
      }}
    />
  );
}

export function RemoteAccessCard() {
  const t = useT();
  const [settings, patch] = useSettingsState();
  const [info, setInfo] = useState<RemoteAccessInfo | null>(null);
  const [selectedIp, setSelectedIp] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const refresh = useCallback(() => {
    remoteAccessInfo()
      .then((i) => {
        setInfo(i);
        // 保留用户已选地址；失效或首取时落到首位候选（后端 Tailscale 优先排序）。
        setSelectedIp((cur) => (cur && i.ips.some((c) => c.ip === cur) ? cur : (i.ips[0]?.ip ?? null)));
      })
      .catch(() => {});
  }, []);
  useEffect(refresh, [refresh]);

  const enabled = settings?.remote_access_enabled ?? false;
  const port = settings?.remote_access_port ?? 18620;

  // 开关/端口落盘后重取信息：token 首开时才生成，lastError 依端口占用而定，ips 随网卡变。
  const toggle = () => {
    void patch({ remote_access_enabled: !enabled }).then(() => refresh());
  };
  const changePort = (next: number) => {
    void patch({ remote_access_port: next }).then(() => refresh());
  };

  const url =
    enabled && selectedIp && info?.token
      ? `http://${selectedIp}:${info.port}/#token=${info.token}`
      : null;
  const svg = url ? renderSVG(url, { border: 2 }) : null;

  const copy = () => {
    if (!url) return;
    void navigator.clipboard
      ?.writeText(url)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => {});
  };

  // 桌面端猜不出手机在哪个网,只能把每个候选「是什么、什么情况下能通」标清楚让用户选。
  const kindLabel = (kind: "tailscale" | "lan") =>
    kind === "tailscale" ? t.remote.netTailscale : t.remote.netLan;
  const candidates = info?.ips ?? [];
  const ipOptions = candidates.map((c) => ({ value: c.ip, label: `${kindLabel(c.kind)} · ${c.ip}` }));
  const selectedKind = candidates.find((c) => c.ip === selectedIp)?.kind ?? null;

  return (
    <div className="row-card remote-access-card">
      <div className="row">
        <div className="row-text">
          <div className="row-label">{t.remote.enable}</div>
          <div className="row-desc">{t.remote.enableDesc}</div>
        </div>
        <Switch checked={enabled} onChange={toggle} label={t.remote.enable} />
      </div>

      <div className="row">
        <div className="row-text">
          <div className="row-label">{t.remote.port}</div>
          <div className="row-desc">{t.remote.portDesc}</div>
        </div>
        <PortInput value={port} onCommit={changePort} />
      </div>

      {info?.lastError && (
        <div className="row">
          <div className="row-text">
            <div className="proxy-err">{t.remote.startError(info.lastError)}</div>
          </div>
        </div>
      )}

      {enabled &&
        (url && svg ? (
          // 配对行与其余设置行同构:文字列居左,二维码当作行右侧的「控件」。
          <div className="row remote-pair-row">
            <div className="row-text">
              <div className="row-label">{t.remote.scan}</div>
              <div className="row-desc">{t.remote.scanHint}</div>
              {ipOptions.length > 1 && (
                <div className="remote-ip-pick">
                  <span className="row-desc">{t.remote.device}</span>
                  <Dropdown value={selectedIp ?? ""} options={ipOptions} onChange={setSelectedIp} />
                </div>
              )}
              <div className="remote-url-row">
                <code className="remote-url">{url}</code>
                <button type="button" className="remote-copy" onClick={copy}>
                  {copied ? t.remote.copied : t.remote.copy}
                </button>
              </div>
              {selectedKind && (
                <div className="row-desc">
                  {selectedKind === "tailscale" ? t.remote.hintTailscale : t.remote.hintLan}
                </div>
              )}
            </div>
            {/* renderSVG 产出完整 <svg> 字符串，本地生成、不含用户可注入内容。 */}
            <div className="remote-qr" dangerouslySetInnerHTML={{ __html: svg }} />
          </div>
        ) : (
          <div className="row">
            <div className="row-text">
              <div className="row-label">{t.remote.scan}</div>
              <div className="row-desc">{t.remote.noIp}</div>
            </div>
          </div>
        ))}
      {!enabled && (
        <div className="row">
          <div className="row-text">
            <div className="row-desc">{t.remote.offHint}</div>
          </div>
        </div>
      )}
    </div>
  );
}
