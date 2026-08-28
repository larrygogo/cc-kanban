// 贴纸主窗口几何（W-17）：正常态尺寸/吸附边/置顶/位置统一存 settings.json（后端 settings.rs
// 的 sticker_window 字段，update_settings 锁内原子落盘）。取代旧版散落的三套 localStorage 键
// （meowo-normal-size / meowo-snap-edge / meowo-pinned）——与 window-state 插件管的位置合计
// 四套存储各写各的，清空 WebView 存储即半吊子状态（有位置没尺寸、有吸附边没尺寸基准）。
// 旧键在 init 时做一次性迁移，迁移后删除；此后 settings.json 是唯一真相源。
import { invoke } from "@tauri-apps/api/core";

export type SnapEdge = "left" | "right" | "top";

/** 与后端 settings.rs StickerWindowState 同构。字段名是 serde 名（snake_case），勿改。 */
export interface StickerWindowState {
  /** 正常（非吸附）态逻辑宽/高；null = 未记录过（按 NORMAL_SIZE_DEFAULT 处理）。 */
  normal_width: number | null;
  normal_height: number | null;
  /** 吸附边；null = 未吸附。 */
  snap_edge: SnapEdge | null;
  /** 用户置顶偏好（吸附/展开态的强制置顶是临时行为，不写这里）。 */
  pinned: boolean;
  /** 正常态窗口左上角（物理像素）；null = 未记录过（交 OS 默认摆放）。 */
  x: number | null;
  y: number | null;
}

// 正常窗口最小尺寸：与 tauri.conf.json 的 minWidth/minHeight、snap.rs 的 STICKER_MIN_* 对齐。
// 低于此即被「吸附态拖角缩成细条」的尺寸毒化（实测 {80,240}/{136,20}），一律回落默认。
const SIZE_MIN_W = 360;
const SIZE_MIN_H = 330;
const SIZE_MAX = 20000; // 上界：与后端 snap::SIZE_MAX_LOGICAL 一致（防异常大值设出极端窗口）
const SIZE_DEFAULT = { w: 360, h: 440 }; // 与 tauri.conf.json 默认 width/height 一致

// 旧版 localStorage 键（仅下方迁移用，勿在别处引用）。
const LEGACY_SNAP_KEY = "meowo-snap-edge";
const LEGACY_SIZE_KEY = "meowo-normal-size";
const LEGACY_PIN_KEY = "meowo-pinned";

let cache: StickerWindowState | null = null;
// 写入串行排队：并发 patch（如吸附同时记尺寸与边）各自读改写，不排队会互相覆盖。
let writeQueue: Promise<void> = Promise.resolve();

const dimOk = (v: unknown, min: number): v is number =>
  typeof v === "number" && Number.isFinite(v) && v >= min && v <= SIZE_MAX;

const isEdge = (v: unknown): v is SnapEdge => v === "left" || v === "right" || v === "top";

/** 后端/迁移来源的原始值 → 合法状态。毒化/越界/非有限数一律丢弃（null → 用方回落默认）。 */
function sanitize(raw: unknown): StickerWindowState {
  const o = (raw ?? {}) as Record<string, unknown>;
  const pos = (v: unknown) =>
    typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= 100_000 ? Math.trunc(v) : null;
  return {
    normal_width: dimOk(o.normal_width, SIZE_MIN_W) ? o.normal_width : null,
    normal_height: dimOk(o.normal_height, SIZE_MIN_H) ? o.normal_height : null,
    snap_edge: isEdge(o.snap_edge) ? o.snap_edge : null,
    pinned: o.pinned === true,
    x: pos(o.x),
    y: pos(o.y),
  };
}

/** 旧版 SIZE_KEY 值（{w,h} JSON）→ 合法尺寸；损坏/毒化返回 null（按缺席算）。 */
function parseLegacySize(raw: string | null): { w: number; h: number } | null {
  try {
    const s = JSON.parse(raw || "");
    if (dimOk(s?.w, SIZE_MIN_W) && dimOk(s?.h, SIZE_MIN_H)) return { w: s.w, h: s.h };
  } catch {
    /* 旧值损坏按缺席算 */
  }
  return null;
}

/**
 * 启动时调用一次：读 settings 里的贴纸几何，并完成 localStorage 旧键的一次性迁移
 * （settings 字段缺席才采纳旧值；迁移后旧键删除）。非 Tauri 环境（测试/浏览器预览）
 * invoke 失败时按全默认继续，不抛错。
 */
export async function initStickerWindowState(): Promise<StickerWindowState> {
  let s: StickerWindowState;
  try {
    s = sanitize(await invoke("get_sticker_window_state"));
  } catch {
    s = sanitize(null);
  }
  try {
    const legacySize = localStorage.getItem(LEGACY_SIZE_KEY);
    const legacyEdge = localStorage.getItem(LEGACY_SNAP_KEY);
    const legacyPin = localStorage.getItem(LEGACY_PIN_KEY);
    if (legacySize !== null || legacyEdge !== null || legacyPin !== null) {
      let migrated = false;
      if ((s.normal_width === null || s.normal_height === null) && legacySize !== null) {
        const sz = parseLegacySize(legacySize);
        if (sz) {
          s.normal_width = sz.w;
          s.normal_height = sz.h;
          migrated = true;
        }
      }
      if (s.snap_edge === null && isEdge(legacyEdge)) {
        s.snap_edge = legacyEdge;
        migrated = true;
      }
      if (!s.pinned && legacyPin === "1") {
        s.pinned = true;
        migrated = true;
      }
      localStorage.removeItem(LEGACY_SIZE_KEY);
      localStorage.removeItem(LEGACY_SNAP_KEY);
      localStorage.removeItem(LEGACY_PIN_KEY);
      if (migrated) {
        await invoke("set_sticker_window_state", { state: s }).catch((err) =>
          console.error("[window-state] 旧几何迁移落盘失败（下次启动重试）：", err)
        );
      }
    }
  } catch {
    /* localStorage 不可用：跳过迁移，settings 值照常生效 */
  }
  cache = s;
  return s;
}

/** 当前缓存的几何（init 之后恒非空；未 init 返回 null，调用方按默认处理）。 */
export function stickerWindowState(): StickerWindowState | null {
  return cache;
}

/**
 * 合并写入（原子）：先同步并进本地缓存（调用方随后 normalSize()/stickerWindowState()
 * 读到的就是新值），再整份落盘 settings.json。落盘经队列串行化，且始终写最新缓存——
 * 连续 patch 的多次写天然合并成最后一次全量写。落盘失败只记日志，缓存值保持。
 */
export function updateStickerWindowState(patch: Partial<StickerWindowState>): void {
  if (cache) cache = sanitize({ ...cache, ...patch });
  writeQueue = writeQueue.then(async () => {
    if (!cache) {
      // 未 init 的极端路径（理论上调用方都在启动 init 之后）：先补 init 再应用 patch。
      await initStickerWindowState().catch(() => null);
      const fresh = stickerWindowState();
      if (fresh) cache = sanitize({ ...fresh, ...patch });
    }
    if (!cache) return;
    await invoke("set_sticker_window_state", { state: cache }).catch((err) =>
      console.error("[window-state] 几何落盘失败：", err)
    );
  });
}

/** 正常尺寸基准：未记录/毒化值一律回落默认（与旧 loadSize 语义一致）。 */
export function normalSize(): { w: number; h: number } {
  const s = cache;
  if (s && s.normal_width !== null && s.normal_height !== null) {
    return { w: s.normal_width, h: s.normal_height };
  }
  return { ...SIZE_DEFAULT };
}
