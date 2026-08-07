// 贴纸看板的共享类型与常量。供主组件 Sticker 及各子模块（icons/helpers/子组件）共用。
import type { LiveSession } from "../../api";

export type Item = LiveSession & { connected: boolean };
/** 看板 tab。已归档不设 tab：归档 = 从看板收纳走，管理入口在设置 → 会话。 */
export type Tab = "all" | "waiting" | "running";

export const PIN_KEY = "meowo-pinned";
export const STAR_KEY = "meowo-starred";
// 用量屏选中的 provider 偏好：折叠/展开会卸载重挂 UsageScreen，持久化以记住上次选择
// （该 provider 仍在活跃列表就沿用，被关/找不到才退回第一个——见 UsageScreen selected 计算）。
export const USAGE_KEY = "meowo-usage-provider";
export const TAB_KEYS: Tab[] = ["all", "waiting", "running"];
