import { invoke } from "@tauri-apps/api/core";

let hostOs: "macos" | "windows" | "other" | null = null;

export async function detectHostOs(): Promise<void> {
  try {
    hostOs = (await invoke<string>("host_os")) as typeof hostOs;
  } catch {
    hostOs = "other";
  }
}

/** 远程（手机浏览器）入口专用：显式把宿主平台钉为 "other"。
 *  hostOs 语义是「宿主桌面 OS」。远程桥虽能经 /rpc/host_os 拿到真实值，
 *  但 isMac()/isWindows() 门控的都是桌面窗口形态（红绿灯顶栏、窗口控制组），
 *  远程端一律走非 Mac/非 Windows 路径——置真值反而会让手机端渲染桌面 Mac 布局。 */
export function setHostOsUnknown(): void {
  hostOs = "other";
}

export function isMac(): boolean {
  return hostOs === "macos";
}

/** Windows 专属能力（Snap Layouts 覆盖窗等）的门控。 */
export function isWindows(): boolean {
  return hostOs === "windows";
}

/** macOS 上以菜单栏面板形态运行（无独立浮窗/吸边）。 */
export function isMacPanel(): boolean {
  return hostOs === "macos";
}
