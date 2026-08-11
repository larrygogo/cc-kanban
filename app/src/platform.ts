import { invoke } from "@tauri-apps/api/core";

let hostOs: "macos" | "windows" | "other" | null = null;

export async function detectHostOs(): Promise<void> {
  try {
    hostOs = (await invoke<string>("host_os")) as typeof hostOs;
  } catch {
    hostOs = "other";
  }
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
