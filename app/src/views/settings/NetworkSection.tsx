// 设置页「网络」分区：代理（全局默认 + 按 Agent 覆盖）。
//
// 这个分区的**核心难点是如实**：三家 CLI 的代理能力差得很远，UI 不能假装它们一样（2026-07 复核）——
//
//   claude → 代理写进它自己的 settings.json（`env` 块，官方支持）→ 谁启动都生效，含你自己开的终端。
//   codex  → 没有任何配置键能代理它自己的 API 请求（`shell_environment_policy.set` 只注入给它派生的
//            子进程，`features.network_proxy` 只管沙箱内的工具执行）。只能靠进程环境变量，而且连这个
//            都只是部分成立——见 openai/codex#4242，它的各个 HTTP client 并未统一读代理环境变量。
//   kimi   → 配置文件同样无处设代理（`[providers.*.env]` 是给 provider 传 GOOGLE_CLOUD_PROJECT 之类的，
//            官方从没说能拿它设代理）；但它的环境变量支持是三家里最好的：官方明确 HTTP(S)/SOCKS 全走，
//            覆盖模型调用、MCP、登录、更新检查等全部出站流量。
//
// 于是覆盖面天生不齐：claude 是「全部会话」，codex / kimi 只覆盖**从 Meowo 打开的**会话——因为进程
// 环境变量只能注入给我们自己拉起的进程，你在别处开的终端我们够不着。每张卡片必须如实标注这一点。
//
// **不走「写进系统环境变量」那条路**：系统里只有一份 HTTPS_PROXY，三家就得共用同一个代理，
// per-agent 隔离当场作废（「Claude 走境外代理、Kimi 直连」正是最常见的配法），还会波及系统上每一个
// 新开的程序。代价远大于收益。详见 Rust 侧 `proxy.rs` 里的同名说明。
//
// 静默是这里最大的失败模式：代理没生效却不告诉用户为什么（SOCKS 不被支持 / 用户自己设过同名变量
// 而我们没覆盖 / 只对部分场景生效），会让人对着「连不上」毫无线索地瞎试。
import { Fragment, useEffect, useState } from "react";
import {
  listAgents,
  getEffectiveProxy,
  type AgentDescriptor,
  type ProxyMode,
  type ProxySettings,
} from "../../api";
import { useT } from "../../i18n";
import { useAgentListRefresh } from "../../useAgents";
import { SETTINGS_DEFAULTS, useSettingsState } from "./state";
import { Segmented, Switch } from "./widgets";
import { Dropdown } from "../menu";

/// 每个模型行的模式，比全局多一个「跟随默认」（= per_agent 里没有该条目）。
type RowMode = ProxyMode | "follow";

/// 后端 apply_to_agent_configs 的单条结果（proxy-applied 事件 / 保存后回传）。
type AgentReport = {
  agent: string;
  skipped: string[];
  unsupported: string | null;
  error: string | null;
};

/// 「能把代理写进自己配置文件（全场景生效）」与「支持 SOCKS」两项，此前是前端自己
/// 维护的两张 agent 名单。它们与后端 ProxySpec 是同一事实的两份拷贝——新 agent 接进来
/// 时前端名单不会自动更新，用户看到的覆盖面说明就是错的，而且不会有任何报错。
/// 现在两项都由 descriptor 随 agent 一起下发（proxy_covers_all_launches /
/// proxy_accepts_socks）。判定与写入一如既往以后端为准，这里只用于选文案。
const isSocks = (u: string) => /^socks[45]?h?:\/\//i.test(u.trim());

/// 代理地址输入框：本地草稿 + 失焦/回车提交。
/// 不做「每键一存」——每敲一个字符都打一次后端校验，非法中间态（如刚敲完 "h"）会疯狂报错。
function UrlInput({
  value,
  placeholder,
  onCommit,
}: {
  value: string;
  placeholder: string;
  onCommit: (url: string) => void;
}) {
  const [text, setText] = useState(value);
  // 磁盘值变了（保存成功 / 保存失败回读）→ 同步草稿，保证 UI 与磁盘一致。
  useEffect(() => setText(value), [value]);
  const commit = () => {
    const u = text.trim();
    // 空着不提交也不报错：用户可能还没填完。空 custom 由后端校验兜底。
    if (u && u !== value) onCommit(u);
  };
  return (
    <input
      className="ns-input"
      type="text"
      autoComplete="off"
      spellCheck={false}
      value={text}
      placeholder={placeholder}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
      }}
    />
  );
}

export function NetworkSection() {
  const t = useT();
  const [settings, patch] = useSettingsState();
  const [agents, setAgents] = useState<AgentDescriptor[]>([]);
  const [err, setErr] = useState<string | null>(null);
  // 各模型（以及全局，键 ""）当前**生效**的代理串。system 模式下这是唯一能看到
  // 「环境变量里到底是什么」的地方——只显示 mode 对用户毫无信息量。
  const [effective, setEffective] = useState<Record<string, string | null>>({});
  // 后端写入 agent 配置的结果：跳过了哪些键、哪家用不了这个代理、哪家写失败了。
  const [reports, setReports] = useState<Record<string, AgentReport>>({});
  // 选了「自定义」但还没填地址的中间态：此时不落盘（后端会拒空地址），只把输入框亮出来。
  const [pending, setPending] = useState<Record<string, boolean>>({});

  const proxy: ProxySettings = settings?.proxy ?? SETTINGS_DEFAULTS.proxy;

  // 只保留**已安装且能被套上代理**的 agent。两层过滤各有其忌讳：
  //   - 配不了代理的 → 给它画输入框，就是请用户配一个静默不生效的代理（这一分区最怕的失败）；
  //   - 没装的 → 还没有可运行的 agent，代理配了也无处生效，先把它装上再说。
  const reloadAgents = () => {
    listAgents()
      .then((all) => setAgents(all.filter((a) => a.supports_proxy && a.installed)))
      .catch(() => {});
  };
  useEffect(reloadAgents, []);
  // 装完一个 agent 它才够格出现在这里（上面按 installed 过滤）——立刻重取，不必重开设置页。
  useAgentListRefresh(reloadAgents);

  // 设置一变就重算生效值（含 system 模式下读到的环境变量）。
  useEffect(() => {
    if (!settings) return;
    let alive = true;
    const keys = ["", ...agents.map((a) => a.id)];
    Promise.all(
      keys.map((k) =>
        getEffectiveProxy(k || undefined)
          .then((p) => [k, p] as const)
          .catch(() => [k, null] as const),
      ),
    ).then((entries) => {
      if (alive) setEffective(Object.fromEntries(entries));
    });
    return () => {
      alive = false;
    };
  }, [settings, agents]);

  // 后端每次写完 agent 配置都会 emit，据此显示「你手设了 HTTPS_PROXY，我没覆盖」这类提示。
  useEffect(() => {
    let un: (() => void) | undefined;
    let alive = true;
    import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<AgentReport[]>("proxy-applied", (e) => {
          setReports(Object.fromEntries(e.payload.map((r) => [r.agent, r])));
        }),
      )
      .then((f) => {
        if (alive) un = f;
        else f();
      })
      .catch(() => {});
    return () => {
      alive = false;
      un?.();
    };
  }, []);

  const save = (next: ProxySettings) => {
    setErr(null);
    void patch({ proxy: next }).then(setErr);
  };

  // ── 全局默认 ──
  const globalMode: ProxyMode = pending[""] ? "custom" : proxy.mode;
  const changeGlobalMode = (m: ProxyMode) => {
    setErr(null);
    // 选 custom 但还没有地址 → 先亮出输入框，等填完再落盘。
    if (m === "custom" && !proxy.url.trim()) {
      setPending((p) => ({ ...p, "": true }));
      return;
    }
    setPending((p) => ({ ...p, "": false }));
    save({ ...proxy, mode: m });
  };

  // ── 按 Agent 覆盖 ──
  const rowMode = (id: string): RowMode =>
    pending[id] ? "custom" : (proxy.per_agent[id]?.mode ?? "follow");

  const changeRowMode = (id: string, m: RowMode) => {
    setErr(null);
    const per = { ...proxy.per_agent };
    if (m === "follow") {
      delete per[id]; // 没有条目 = 跟随全局，不留 {mode:"follow"} 这种后端不认识的值
      setPending((p) => ({ ...p, [id]: false }));
      save({ ...proxy, per_agent: per });
      return;
    }
    if (m === "custom" && !per[id]?.url?.trim()) {
      setPending((p) => ({ ...p, [id]: true }));
      return;
    }
    setPending((p) => ({ ...p, [id]: false }));
    per[id] = { mode: m, url: per[id]?.url ?? "" };
    save({ ...proxy, per_agent: per });
  };

  const commitRowUrl = (id: string, url: string) => {
    setPending((p) => ({ ...p, [id]: false }));
    save({ ...proxy, per_agent: { ...proxy.per_agent, [id]: { mode: "custom", url } } });
  };

  const modeOptions: { value: ProxyMode; label: string }[] = [
    { value: "off", label: t.proxy.off },
    { value: "system", label: t.proxy.system },
    { value: "custom", label: t.proxy.custom },
  ];
  const rowOptions: { value: RowMode; label: string }[] = [
    { value: "follow", label: t.proxy.followGlobal },
    ...modeOptions,
  ];

  const hasEffective = (k: string) => Object.prototype.hasOwnProperty.call(effective, k);

  const effLabel = (k: string) => {
    if (!hasEffective(k)) return null;
    const p = effective[k];
    return p ? t.proxy.effective(p) : t.proxy.effectiveDirect;
  };

  // 有模型正走在一个它不支持的 SOCKS 代理上 → 顶部告警。用生效值判断（含 system 模式读到的环境变量），
  // 不能只看用户填的那一栏。
  const socksBroken = agents.some(
    (a) => !a.proxy_accepts_socks && (effective[a.id] ?? "") !== "" && isSocks(effective[a.id] ?? ""),
  );

  return (
    <>
      <div className="row-card">
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.proxy.mode}</div>
            <div className="row-desc">{t.proxy.modeDesc}</div>
          </div>
          <Segmented value={globalMode} options={modeOptions} onChange={changeGlobalMode} label={t.proxy.mode} />
        </div>

        {globalMode === "custom" && (
          <div className="row proxy-url-row">
            <UrlInput
              value={proxy.url}
              placeholder={t.proxy.urlPlaceholder}
              onCommit={(url) => {
                setPending((p) => ({ ...p, "": false }));
                save({ ...proxy, mode: "custom", url });
              }}
            />
          </div>
        )}

        {globalMode === "system" && (
          <div className="row">
            <div className="row-text">
              <div className="row-desc">
                {effective[""] ? t.proxy.systemHint(effective[""]!) : t.proxy.systemNone}
              </div>
            </div>
          </div>
        )}
      </div>

      {socksBroken && <div className="sec-hint proxy-err">{t.proxy.socksWarn}</div>}
      {effective[""] && isSocks(effective[""]!) && (
        <div className="sec-hint proxy-err">{t.proxy.updaterSocksHint}</div>
      )}

      {agents.length > 0 && (
        <div className="row-card">
          <div className="row">
            <div className="row-text">
              <div className="row-label">{t.proxy.perAgent}</div>
              <div className="row-desc">{t.proxy.perAgentDesc}</div>
            </div>
          </div>
          {agents.map((a) => {
            const m = rowMode(a.id);
            const rep = reports[a.id];
            const full = a.proxy_covers_all_launches;
            const label = effLabel(a.id);
            const proxied = hasEffective(a.id) && (effective[a.id] ?? "") !== "";
            return (
              <Fragment key={a.id}>
                <div className="row">
                  <div className="row-text">
                    <div className="row-label proxy-agent-name">{a.display_name}</div>
                    {/* 自定义模式的输入框已经展示同一个地址，不再在上方重复一遍。 */}
                    {m !== "custom" && label && <div className="row-desc">{label}</div>}
                    {/* 覆盖面：只有真的走了代理才谈得上「生效范围」，直连时说这个是噪音。 */}
                    {proxied && (
                      <div className="row-desc proxy-coverage">
                        {full ? t.proxy.coverageFull : t.proxy.coveragePartial}
                      </div>
                    )}
                    {rep?.unsupported && <div className="row-desc proxy-err">{t.proxy.unsupported(rep.unsupported)}</div>}
                    {rep?.skipped?.length ? (
                      <div className="row-desc proxy-err">{t.proxy.skipped(rep.skipped.join("、"))}</div>
                    ) : null}
                    {rep?.error && <div className="row-desc proxy-err">{t.proxy.applyError(rep.error)}</div>}
                  </div>
                  {/* Dropdown 而非 Segmented：4 个选项 × N 个模型的分段控件会把左侧名字挤到换行。 */}
                  <Dropdown value={m} options={rowOptions} onChange={(v: RowMode) => changeRowMode(a.id, v)} />
                </div>
                {m === "custom" && (
                  <div className="row proxy-url-row">
                    <UrlInput
                      value={proxy.per_agent[a.id]?.url ?? ""}
                      placeholder={t.proxy.urlPlaceholder}
                      onCommit={(url) => commitRowUrl(a.id, url)}
                    />
                  </div>
                )}
              </Fragment>
            );
          })}
        </div>
      )}

      {err && <div className="sec-hint proxy-err">{t.proxy.saveFailed(err)}</div>}
      <div className="sec-hint">{t.proxy.desc}</div>
    </>
  );
}
