// 模型清单的**真实标签**：从 CLI 自己弹出的 `/model` 菜单学，而不是宿主维护一份会过时的。
//
// 背景：插件里的 `model_presets` 有两层——别名（`opus`/`opusplan`…，是 CLI 文档化的稳定
// 契约，写死没问题）与展示标签（"Opus"…，纯 UI 文案，必随 CLI 改版漂移；实际漂过一版：
// 官方菜单早已是 "Opus 5"，我们还写着无版本号的 "Opus"）。
//
// 这里只治标签那一层：用户点开模型菜单时走终端菜单识别通道（CLI 真的弹一次菜单），把识别到
// 的标签按 provider + CLI 版本缓存；之后直接渲染 GUI 下拉、切换仍走 `/model <别名>` 内联，
// 不再打扰会话。CLI 升级 → 版本号变 → 缓存键变 → 自动重新学。

const KEY = "meowo-model-labels";

/** 规范化：去掉大小写与所有非字母数字，用于把菜单标签匹配回别名。 */
const normalize = (text: string): string => text.toLowerCase().replace(/[^a-z0-9]/g, "");

type Store = Record<string, string[]>;

const storeKey = (provider: string, version: string | null): string => `${provider}@${version ?? "unknown"}`;

function readStore(): Store {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) || "{}");
    return raw && typeof raw === "object" ? (raw as Store) : {};
  } catch {
    return {};
  }
}

/** 该 provider + CLI 版本已学到的菜单标签；没学过返回 null。 */
export function loadLearnedLabels(provider: string | null, version: string | null): string[] | null {
  if (!provider) return null;
  const hit = readStore()[storeKey(provider, version)];
  return Array.isArray(hit) && hit.length > 0 ? hit.filter((l): l is string => typeof l === "string") : null;
}

/** 记下这次从 CLI 菜单识别到的标签。只保留当前 provider+版本这一条，旧版本的条目一并清掉。 */
export function saveLearnedLabels(provider: string | null, version: string | null, labels: string[]): void {
  if (!provider || labels.length === 0) return;
  try {
    const store = readStore();
    const key = storeKey(provider, version);
    // 同 provider 的旧版本条目失去意义（CLI 已升级），顺手清掉，别让它无限长。
    for (const existing of Object.keys(store)) {
      if (existing.startsWith(`${provider}@`) && existing !== key) delete store[existing];
    }
    store[key] = labels;
    localStorage.setItem(KEY, JSON.stringify(store));
  } catch {
    /* localStorage 不可用（隐私模式/配额）时静默降级为不学习 */
  }
}

/**
 * 用学到的真实标签替换内置标签。匹配规则：规范化后「标签包含别名」，多个别名命中时取**最长**
 * 的那个——"OpusPlan" 同时含 `opus` 与 `opusplan`，最长匹配才落到 opusplan 上；"Opus5" 只含
 * `opus`。匹配不上的预设保留内置标签（新模型出现在菜单里但我们没有别名时同理跳过，
 * 不凭空造一个点了没用的条目）。
 */
export function applyLearnedLabels<T extends { id: string; label: string }>(
  presets: T[],
  learned: string[] | null,
): T[] {
  if (!learned || learned.length === 0) return presets;
  const taken = new Set<string>();
  const byId = new Map<string, string>();
  for (const label of learned) {
    const normalized = normalize(label);
    let best: T | null = null;
    for (const preset of presets) {
      if (taken.has(preset.id)) continue;
      if (!normalized.includes(normalize(preset.id))) continue;
      if (!best || preset.id.length > best.id.length) best = preset;
    }
    if (best) {
      taken.add(best.id);
      byId.set(best.id, label.trim());
    }
  }
  return presets.map((preset) => {
    const label = byId.get(preset.id);
    return label ? { ...preset, label } : preset;
  });
}
