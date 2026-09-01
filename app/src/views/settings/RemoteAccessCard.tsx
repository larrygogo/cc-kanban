// 设置页「网络」分区顶部：手机远程访问（局域网/Tailscale + token）开关与扫码配对。
//
// token 是进门凭据，绝不上网线：二维码把它放 URL fragment（`#token=`，不进服务端日志
// 也不随请求发出），手机端首次打开即存 localStorage 并清掉 hash。开关/端口经 set_settings
// 落盘后由后端 remote::apply 热生效；token 由 remote_access_info 惰性生成（首次开启才产生）。
import { useCallback, useEffect, useState } from "react";
import { renderSVG } from "uqr";
import { regenerateRemoteToken, remoteAccessInfo, type RemoteAccessInfo, type RemoteBindMode } from "../../api";
import { appConfirm } from "../../confirm";
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
  // patchError 与兄弟分区同款：开关/端口落盘被拒时不能静默回滚（开关自己弹回去零解释）。
  const [settings, patch, patchError] = useSettingsState();
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
  // 持久化值可能是未知字符串(手改 settings.json / 旧数据损坏),直接传给 Dropdown 会
  // 显示空标签。未知值回退 "all",与后端 unknown→all 的兼容语义一致。
  const rawBind = settings?.remote_access_bind;
  const bind: RemoteBindMode = rawBind === "loopback" || rawBind === "tailscale" ? rawBind : "all";

  // 开关/端口落盘后重取信息：remote::apply 是 fire-and-forget,set_settings resolve 时
  // server 往往还没 bind 完——立刻 refresh 会读到 apply 前的 last_error(端口被占也显
  // 「一切正常」的假二维码)。再延迟补一拍,拿到 bind 结果后的真 last_error。
  const refreshSettled = () => {
    refresh();
    window.setTimeout(refresh, 600);
  };
  const toggle = () => {
    void patch({ remote_access_enabled: !enabled }).then(refreshSettled);
  };
  const changePort = (next: number) => {
    void patch({ remote_access_port: next }).then(refreshSettled);
  };
  // 换绑定模式同样走 apply 的差异比对热重启;tailscale 模式找不到接口会被后端拒启,
  // last_error 红字如实显示(不静默回退 0.0.0.0)。
  const changeBind = (next: RemoteBindMode) => {
    void patch({ remote_access_bind: next }).then(refreshSettled);
  };
  const bindOptions = [
    { value: "all" as const, label: t.remote.bindAll },
    { value: "loopback" as const, label: t.remote.bindLoopback },
    { value: "tailscale" as const, label: t.remote.bindTailscale },
  ];

  const url =
    enabled && selectedIp && info?.token
      ? // 二维码按本实例**实际监听**端口生成（boundPort）：多实例共享 settings.json 时
        // info.port 是被另一实例改过的配置值，照它生成会指向别的实例的 server（实拍：
        // dev 改 18622 后，安装版 QR 指向 18622 而它自己还听在 18621）。
        `http://${selectedIp}:${info.boundPort ?? info.port}/#token=${info.token}`
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

      <div className="row">
        <div className="row-text">
          <div className="row-label">{t.remote.bind}</div>
          {/* 选 all 时把明文风险顶到说明位——这是唯一把令牌暴露给整个同网段的模式。 */}
          <div className="row-desc">{bind === "all" ? t.remote.bindWarnAll : t.remote.bindDesc}</div>
        </div>
        <Dropdown value={bind} options={bindOptions} onChange={changeBind} />
      </div>

      {patchError && (
        <div className="row">
          <div className="row-text">
            <div className="proxy-err" role="alert">{patchError}</div>
          </div>
        </div>
      )}

      {info?.lastError && (
        <div className="row">
          <div className="row-text">
            <div className="proxy-err">{t.remote.startError(info.lastError)}</div>
          </div>
        </div>
      )}

      {/* 配置与实际监听分叉（多实例共享 settings.json，另一实例改端口写盘而本实例
          listener 未跟随）：不是错误——本实例服务本身健康，但配置值已不可信，说明白。 */}
      {enabled && info?.boundPort != null && info.boundPort !== port && (
        <div className="row">
          <div className="row-text">
            <div className="row-desc">{t.remote.boundMismatch(port, info.boundPort)}</div>
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
              {/* 不配文字标签:选项本身是「Tailscale · IP」已自解释,窄列里标签只会被挤成竖排。 */}
              {ipOptions.length > 1 && (
                <div className="remote-ip-pick">
                  <Dropdown value={selectedIp ?? ""} options={ipOptions} onChange={setSelectedIp} />
                </div>
              )}
              {/* 地址不明文上屏(那串 token 又长又吵还含凭据),只给复制按钮;二维码即是地址本体。 */}
              <div className="remote-url-row">
                <button type="button" className="remote-copy" onClick={copy}>
                  {copied ? t.remote.copied : t.remote.copy}
                </button>
                {/* 换发即吊销:apply 比对 token 差异重启 server,已配对手机全部 401 回配对页。
                    但无可逆破坏——重新扫码即可配对,不销毁任何数据,故不用 danger 红(S-14)。 */}
                <button
                  type="button"
                  className="remote-copy"
                  onClick={() => {
                    void appConfirm(t.remote.regenerateConfirm, { title: t.remote.regenerate, confirmLabel: t.remote.regenerate }).then((yes) => {
                      if (!yes) return;
                      void regenerateRemoteToken().then(refreshSettled).catch(() => {});
                    });
                  }}
                >
                  {t.remote.regenerate}
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
