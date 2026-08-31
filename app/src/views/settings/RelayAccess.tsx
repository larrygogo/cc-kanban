import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  getRelaySecretStatus,
  getRelaySecrets,
  listRelayModels,
  setRelaySecret,
  type AgentDescriptor,
  type RelayAuth,
  type RelayProtocol,
  type RelayRule,
  type Settings,
} from "../../api";
import { useT } from "../../i18n";
import { formatBackendError } from "../../i18n/errors";
import { Segmented, Switch } from "./widgets";
import { Dropdown } from "../menu";

type AccessMode = "official" | "relay";

/**
 * RelayAccess 只看这三个字段。
 *
 * 收窄成 Pick 而不是要完整的 `AgentDescriptor`：否则调用方（ProviderCard 手工拼一个 agent 对象）
 * 得为了满足类型去编造它根本不读的字段——比如 `supports_proxy`，随手填个 `false` 就是在说谎。
 */
type RelayAgent = Pick<AgentDescriptor, "id" | "display_name" | "relay">;

const relayDefault = (agent: RelayAgent): RelayRule => ({
  enabled: false,
  base_url: "",
  model: "",
  protocol: agent.relay?.default_protocol ?? "",
  auth: agent.relay?.default_auth ?? "bearer",
  env_options: [],
});

function RelayInput({
  value,
  placeholder,
  onCommit,
}: {
  value: string;
  placeholder: string;
  onCommit: (value: string) => void;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = () => {
    const next = text.trim();
    if (next !== value) onCommit(next);
  };
  return (
    <>
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
    </>
  );
}

export function modelMenuPlacement(
  rect: Pick<DOMRect, "top" | "bottom">,
  viewportHeight: number,
  contentHeight: number,
) {
  const gap = 5;
  const edge = 6;
  const below = Math.max(0, viewportHeight - rect.bottom - gap - edge);
  const above = Math.max(0, rect.top - gap - edge);
  const wanted = Math.min(260, Math.max(80, contentHeight));
  const opensUp = below < Math.min(wanted, 160) && above > below;
  const available = opensUp ? above : below;
  const maxHeight = Math.max(0, Math.min(wanted, available));
  return {
    opensUp,
    top: opensUp ? Math.max(edge, rect.top - gap - maxHeight) : rect.bottom + gap,
    maxHeight,
  };
}

function ModelPicker({
  value,
  fallback,
  remote,
  loading,
  error,
  placeholder,
  customHint,
  relayLabel,
  refreshLabel,
  onOpen,
  onRefresh,
  onCommit,
}: {
  value: string;
  fallback: readonly string[];
  remote: string[];
  loading: boolean;
  error: string | null;
  placeholder: string;
  customHint: string;
  relayLabel: string;
  refreshLabel: string;
  onOpen: () => void;
  onRefresh: () => void;
  onCommit: (value: string) => void;
}) {
  const [text, setText] = useState(value);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ left: 0, top: 0, width: 0, maxHeight: 260, opensUp: false });
  // ↑/↓ 键盘导航的高亮项（-1 = 无高亮，此时 Enter 走自由文本提交）；输入或重开菜单时复位。
  const [activeIdx, setActiveIdx] = useState(-1);
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => setText(value), [value]);

  const commit = (next = text) => {
    const trimmed = next.trim();
    if (trimmed && trimmed !== value) onCommit(trimmed);
  };
  const commitRef = useRef(commit);
  commitRef.current = commit;
  const show = () => {
    setActiveIdx(-1);
    setOpen(true);
    onOpen();
  };
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = rootRef.current?.getBoundingClientRect();
      if (rect) {
        const placement = modelMenuPlacement(
          rect,
          window.innerHeight,
          menuRef.current?.scrollHeight ?? 260,
        );
        setPos({ left: rect.left, width: rect.width, ...placement });
      }
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, remote.length, fallback.length, loading, error, text]);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      const node = event.target as Node;
      if (!rootRef.current?.contains(node) && !menuRef.current?.contains(node)) {
        commitRef.current();
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);

  const remoteSet = new Set(remote);
  const options = [...new Set([...remote, ...fallback])].filter((model) =>
    model.toLowerCase().includes(text.trim().toLowerCase()),
  );
  const choose = (model: string) => {
    setText(model);
    setActiveIdx(-1);
    onCommit(model);
    setOpen(false);
  };

  return (
    <div className={`relay-model-picker${open ? " open" : ""}`} ref={rootRef}>
      <input
        className="relay-model-input"
        value={text}
        placeholder={placeholder}
        autoComplete="off"
        spellCheck={false}
        role="combobox"
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open && activeIdx >= 0 && activeIdx < options.length ? `${listId}-${activeIdx}` : undefined}
        onFocus={show}
        onChange={(e) => { setText(e.target.value); setActiveIdx(-1); setOpen(true); }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            if (!open) { show(); return; }
            setActiveIdx((i) => Math.min(i + 1, options.length - 1));
            return;
          }
          if (e.key === "ArrowUp" && open) {
            e.preventDefault();
            setActiveIdx((i) => Math.max(i - 1, -1));
            return;
          }
          if (e.key === "Enter") {
            // 有键盘高亮项则选中它；否则按自由文本提交（combobox 允许输入任意模型名）。
            const picked = open && activeIdx >= 0 ? options[activeIdx] : undefined;
            if (picked) choose(picked);
            else { commit(); setOpen(false); }
            e.currentTarget.blur();
          }
          if (e.key === "Escape") {
            // 必须 preventDefault：React 合成事件先于 window 冒泡监听（useEscClose），
            // 不吃掉的话它会看到 defaultPrevented=false 把整扇设置窗关掉。也不能 blur——
            // blur 后 activeElement 变 body，useEscClose 的输入框让位守卫同样失效。
            e.preventDefault();
            if (open) {
              // 菜单开着：只收菜单，焦点留在输入框。
              setOpen(false);
              setActiveIdx(-1);
            }
            // 菜单已关：什么都不做——保持聚焦，useEscClose 的输入框守卫自然让位。
          }
        }}
      />
      <button
        className={`relay-model-refresh${loading ? " loading" : ""}`}
        type="button"
        data-tip={refreshLabel}
        aria-label={refreshLabel}
        disabled={loading}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => { setOpen(true); onRefresh(); }}
      >
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M13 4.5V1.8m0 2.7h-2.7M13 4.5A5.5 5.5 0 1 0 13.4 11" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
      </button>
      <svg className="relay-model-chev" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="m3 4.5 3 3 3-3" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" /></svg>
      {open && createPortal(
        <div
          className={`relay-model-menu${pos.opensUp ? " up" : ""}`}
          ref={menuRef}
          id={listId}
          role="listbox"
          style={{ left: pos.left, top: pos.top, width: pos.width, maxHeight: pos.maxHeight }}
        >
          {options.map((model, i) => (
            <button
              type="button"
              role="option"
              id={`${listId}-${i}`}
              aria-selected={model === value}
              className={`relay-model-option${model === value ? " sel" : ""}${i === activeIdx ? " active" : ""}`}
              key={model}
              onClick={() => choose(model)}
            >
              <span>{model}</span>
              {remoteSet.has(model) && <em>{relayLabel}</em>}
            </button>
          ))}
          {loading && <div className="relay-model-status"><span className="relay-model-spinner" />{refreshLabel}</div>}
          {!loading && error && <div className="relay-model-status error">{error}</div>}
          {!loading && options.length === 0 && !error && <div className="relay-model-status">{customHint}</div>}
          <div className="relay-model-custom">{customHint}</div>
        </div>,
        document.body,
      )}
    </div>
  );
}

export function RelayAccess({
  agent,
  settings,
  patch,
}: {
  agent: RelayAgent;
  settings: Settings | null;
  patch: (p: Partial<Settings>) => Promise<string | null>;
}) {
  const capability = agent.relay;
  if (!capability) return null;
  return <RelayAccessSupported agent={agent} settings={settings} patch={patch} capability={capability} />;
}

function RelayAccessSupported({ agent, settings, patch, capability }: {
  agent: RelayAgent;
  settings: Settings | null;
  patch: (p: Partial<Settings>) => Promise<string | null>;
  capability: NonNullable<AgentDescriptor["relay"]>;
}) {
  const t = useT();
  const relay = settings?.relay ?? { per_agent: {} };
  const rule = relay.per_agent[agent.id] ?? relayDefault(agent);
  const [configuring, setConfiguring] = useState(false);
  const [secretSaved, setSecretSaved] = useState(false);
  const [secretValue, setSecretValue] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [remoteModels, setRemoteModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [modelsAttempted, setModelsAttempted] = useState(false);
  const modelsRequestSeq = useRef(0);
  const mode: AccessMode = rule.enabled || configuring ? "relay" : "official";

  useEffect(() => {
    getRelaySecretStatus()
      .then((status) => setSecretSaved(status[agent.id] ?? false))
      .catch(() => {});
    getRelaySecrets()
      .then((secrets) => setSecretValue(secrets[agent.id] ?? ""))
      .catch(() => {});
  }, [agent.id]);
  useEffect(() => {
    if (rule.enabled) setConfiguring(false);
  }, [rule.enabled]);
  useEffect(() => {
    modelsRequestSeq.current += 1; // 令旧地址/协议的在途响应失效
    setRemoteModels([]);
    setModelsError(null);
    setModelsAttempted(false);
    setModelsLoading(false);
  }, [agent.id, rule.base_url, rule.protocol, rule.auth]);

  const normalizeRule = (next: RelayRule): RelayRule => ({
    ...next,
    protocol: next.protocol || capability.default_protocol,
    auth: next.auth || capability.default_auth,
  });
  const fetchModels = () => {
    if (modelsLoading) return;
    setModelsAttempted(true);
    if (!rule.base_url.trim() || !secretSaved) {
      setModelsError(t.relay.modelFetchNeedConfig);
      return;
    }
    setModelsLoading(true);
    setModelsError(null);
    const seq = ++modelsRequestSeq.current;
    const effective = normalizeRule(rule);
    void listRelayModels(agent.id, effective.base_url, effective.protocol, effective.auth)
      .then((models) => {
        if (seq === modelsRequestSeq.current) setRemoteModels(models);
      })
      .catch((e) => {
        if (seq === modelsRequestSeq.current) setModelsError(t.relay.modelFetchFailed(formatBackendError(e, t.locale)));
      })
      .finally(() => {
        if (seq === modelsRequestSeq.current) setModelsLoading(false);
      });
  };

  const saveRule = async (next: RelayRule) => {
    setErr(null);
    const normalized = normalizeRule(next);
    const e = await patch({
      relay: { per_agent: { ...relay.per_agent, [agent.id]: normalized } },
    });
    setErr(e);
    return e;
  };
  const ready = (next: RelayRule, hasSecret = secretSaved) => {
    const normalized = normalizeRule(next);
    return Boolean(normalized.base_url.trim() && normalized.model.trim() && hasSecret
      && (!capability.protocols.length || normalized.protocol));
  };
  const saveField = (next: RelayRule) => {
    // 正在配置且最后一个必填项刚补齐时直接完成切换，不要求用户先切回官方再点一次中转。
    void saveRule({ ...next, enabled: rule.enabled || (configuring && ready(next)) });
  };
  const changeMode = (next: AccessMode) => {
    if (next === "official") {
      setConfiguring(false);
      void saveRule({ ...rule, enabled: false });
      return;
    }
    if (ready(rule)) {
      void saveRule({ ...rule, enabled: true });
    } else {
      setConfiguring(true);
      setErr(null);
    }
  };
  const saveSecret = (value: string) => {
    setErr(null);
    const normalized = value.trim();
    void setRelaySecret(agent.id, normalized)
      .then(() => {
        const saved = normalized.length > 0;
        setSecretValue(normalized);
        setSecretSaved(saved);
        if (!saved && rule.enabled) {
          void saveRule({ ...rule, enabled: false });
        } else if (configuring && ready(rule, saved)) {
          void saveRule({ ...rule, enabled: true });
        }
      })
      .catch((e) => setErr(formatBackendError(e, t.locale)));
  };

  // 未启用时把「还差哪几项」直接列出来——「填齐才自动启用」的规则此前只有一句笼统说明,
  // 用户不知道自己还缺什么。
  const missingFields = (() => {
    const n = normalizeRule(rule);
    const out: string[] = [];
    if (!n.base_url.trim()) out.push(t.relay.baseUrl);
    if (!n.model.trim()) out.push(t.relay.model);
    if (!secretSaved) out.push(t.relay.secret);
    if (capability.protocols.length && !n.protocol) out.push(t.relay.protocol);
    return out;
  })();

  const authOptions: { value: RelayAuth; label: string }[] = capability.auth_modes.map((option) => ({
    value: option.value,
    label: option.value === "bearer" ? t.relay.bearer : option.value === "api_key" ? t.relay.apiKeyHeader : option.label,
  }));
  const protocolOptions: { value: RelayProtocol; label: string }[] = capability.protocols;
  const fallbackModels = capability.suggestions.find(
    (group) => group.protocol === normalizeRule(rule).protocol,
  )
    ?? capability.suggestions.find((group) => group.protocol === "");

  return (
    <div className="provider-access" data-testid={`agent-access-${agent.id}`}>
      <div className="provider-access-head">
        <span className="row-label">{t.relay.accessMode}</span>
        <Segmented
          value={mode}
          label={`${agent.display_name} ${t.relay.accessMode}`}
          options={[
            { value: "official", label: t.relay.official },
            { value: "relay", label: t.relay.title },
          ]}
          onChange={changeMode}
        />
      </div>
      {mode === "relay" && (
        <>
          <div className="relay-fields">
            <label className="relay-field">
              <span>{t.relay.baseUrl}</span>
              <RelayInput
                value={rule.base_url}
                placeholder={t.relay.baseUrlPlaceholder}
                onCommit={(base_url) => saveField({ ...rule, base_url })}
              />
            </label>
            <label className="relay-field">
              <span>{t.relay.model}</span>
              <ModelPicker
                value={rule.model}
                placeholder={t.relay.modelPlaceholder}
                fallback={fallbackModels?.models ?? []}
                remote={remoteModels}
                loading={modelsLoading}
                error={modelsError}
                customHint={t.relay.modelHint}
                relayLabel={t.relay.relayModel}
                refreshLabel={modelsLoading ? t.relay.fetchingModels : t.relay.fetchModels}
                onOpen={() => { if (!modelsAttempted) fetchModels(); }}
                onRefresh={fetchModels}
                onCommit={(model) => saveField({ ...rule, model })}
              />
            </label>
            {authOptions.length > 1 && (
              <div className="relay-field relay-select-field">
                <span>{t.relay.authType}</span>
                <Dropdown
                  value={rule.auth}
                  options={authOptions}
                  onChange={(auth: RelayAuth) => saveField({ ...rule, auth })}
                />
              </div>
            )}
            {protocolOptions.length > 0 && (
              <div className="relay-field relay-select-field">
                <span>{t.relay.protocol}</span>
                <Dropdown
                  value={rule.protocol || capability.default_protocol}
                  options={protocolOptions}
                  onChange={(protocol: RelayProtocol) => saveField({ ...rule, protocol })}
                />
              </div>
            )}
            {/* 密钥明文展示：中转令牌粘贴错一个字符就是 401，用户需要肉眼核对；本就仅本机可见。 */}
            <label className="relay-field relay-secret-field">
              <span>{t.relay.secret}</span>
              <RelayInput
                value={secretValue}
                placeholder={t.relay.secretPlaceholder}
                onCommit={saveSecret}
              />
            </label>
            {/* 插件声明的附加环境变量（目前只有 claude 的两个）；勾选随规则持久化，启动会话时注入。 */}
            {(capability.env_options?.length ?? 0) > 0 && (
              <div className="relay-field relay-env-options">
                <span>{t.relay.envOptions}</span>
                {capability.env_options!.map((option) => {
                  const selected = (rule.env_options ?? []).includes(option.id);
                  const label =
                    option.id === "disable_nonessential_traffic"
                      ? t.relay.envDisableNonessential
                      : option.id === "no_attribution_header"
                        ? t.relay.envNoAttribution
                        : option.label;
                  return (
                    <div className="relay-env-option" key={option.id}>
                      <span className="relay-env-option-text">
                        <span className="relay-env-option-label">{label}</span>
                        <span className="relay-env-option-desc">{`${option.env[0]}=${option.env[1]}`}</span>
                      </span>
                      <Switch
                        checked={selected}
                        label={label}
                        onChange={() => {
                          const next = new Set(rule.env_options ?? []);
                          if (selected) next.delete(option.id);
                          else next.add(option.id);
                          saveField({ ...rule, env_options: [...next] });
                        }}
                      />
                    </div>
                  );
                })}
              </div>
            )}
          </div>
          <div className="row-desc relay-coverage">
            {rule.enabled
              ? t.relay.coverage
              : missingFields.length
              ? t.relay.missingFields(missingFields)
              : t.relay.completeToEnable}
          </div>
        </>
      )}
      {err && <div className="row-desc proxy-err" role="alert">{t.relay.saveFailed(err)}</div>}
    </div>
  );
}
