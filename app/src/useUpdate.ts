import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { cancelUpdateDownload, checkUpdate, downloadUpdate, getSettings, installDownloadedUpdate, type Settings } from "./api";

// "unknown"：本会话从未检查过（自动更新被关掉）。不能用 "latest" 顶替——那会让
// 「关于」页在从未联网确认的情况下显示「已是最新版本」，恰恰误导了最需要手动确认的用户。
export type UpdateStatus = "checking" | "unknown" | "latest" | "available" | "downloading" | "ready" | "error";

/// 检查更新；对外暴露状态/版本/更新说明/进度，以及下载、安装和手动重检操作。
/// 非 Tauri 环境（测试/浏览器）或网络失败一律降级为 error，不抛错。
/// automatic=true 时服从设置里的自动更新开关，延迟检查并后台下载；安装始终由更新窗口确认。
export function useUpdate(options: { automatic?: boolean; delayMs?: number } = {}) {
  const { automatic = false, delayMs = 10_000 } = options;
  // automatic 模式的首个真实检查最早也在 delayMs（默认 10s）之后——初值若是 "checking"，
  // 「关于」页会顶着「检查中…」空转十秒（其实什么都没在查）。unknown = 尚未检查，只显示版本号。
  const [status, setStatus] = useState<UpdateStatus>(automatic ? "unknown" : "checking");
  const [version, setVersion] = useState<string | null>(null);
  // 新版本的更新说明（release notes，来自 updater manifest 的 notes 字段），无则 null。
  const [notes, setNotes] = useState<string | null>(null);
  // null = 总大小未知（响应无 Content-Length），UI 显示不带百分比的「下载中…」。
  const [progress, setProgress] = useState<number | null>(0);
  const checkedRef = useRef(false);

  // 返回本次检查的结果状态（调用方拿结果不能依赖异步 state）。
  const recheck = useCallback(async (): Promise<UpdateStatus> => {
    setStatus("checking");
    checkedRef.current = false;
    try {
      const up = await checkUpdate();
      if (up) {
        checkedRef.current = true;
        setVersion(up.version);
        setNotes(up.body?.trim() ? up.body : null);
        setStatus(up.downloadState);
        return up.downloadState;
      }
      checkedRef.current = false;
      setStatus("latest");
      return "latest";
    } catch {
      checkedRef.current = false;
      setStatus("error");
      return "error";
    }
  }, []);

  const download = useCallback(async () => {
    if (!checkedRef.current) return;
    setStatus("downloading");
    setProgress(0);
    try {
      setStatus(await downloadUpdate());
    } catch (err) {
      console.error("[update] 下载失败：", err);
      setStatus("error");
    }
  }, []);

  // 取消进行中的下载（S-13）：后端丢弃下载 future 并广播 update-download-cancelled，
  // 状态由该事件的监听统一落回 available（多窗口一致）。
  const cancelDownload = useCallback(async () => {
    try {
      await cancelUpdateDownload();
    } catch (err) {
      console.error("[update] 取消下载失败：", err);
    }
  }, []);

  const install = useCallback(async () => {
    try {
      await installDownloadedUpdate();
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
    } catch (err) {
      console.error("[update] 安装失败：", err);
      setStatus("error");
    }
  }, []);

  // 更新下载是后端进程级共享任务；每个窗口都订阅同一组事件，晚打开的更新窗口也能接续状态。
  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const register = (pending: Promise<() => void>) => {
      void pending.then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      }).catch(() => {});
    };
    register(listen<{ downloaded: number; contentLength: number | null }>(
      "update-download-progress",
      ({ payload }) => {
        if (disposed) return;
        setStatus("downloading");
        const total = payload.contentLength ?? 0;
        setProgress(total === 0 ? null : Math.min(100, Math.round((payload.downloaded / total) * 100)));
      },
    ));
    register(listen("update-download-finished", () => {
      if (disposed) return;
      setProgress(100);
      setStatus("ready");
    }));
    register(listen("update-download-failed", () => {
      if (disposed) return;
      setStatus("error");
    }));
    register(listen("update-download-cancelled", () => {
      if (disposed) return;
      // 取消 = 回到「发现新版本」：checkedRef 仍在，可立即重新下载。
      setProgress(0);
      setStatus("available");
    }));
    return () => {
      disposed = true;
      unlisteners.forEach((fn) => fn());
    };
  }, []);

  // 更新窗口/设置页沿用即时手动检查；主窗则服从“自动更新”设置并延迟启动网络请求。
  useEffect(() => {
    if (automatic) return;
    void recheck();
  }, [automatic, recheck]);

  useEffect(() => {
    if (!automatic) return;
    let disposed = false;
    let timer: number | undefined;
    let interval: number | undefined;
    let enabled: boolean | undefined;
    let unlisten: (() => void) | undefined;
    const run = () => {
      void recheck().then((next) => {
        if (!disposed && next === "available") void download();
      });
    };
    const applySettings = (settings: Settings) => {
      if (enabled === settings.auto_update_enabled) return;
      enabled = settings.auto_update_enabled;
      if (timer != null) window.clearTimeout(timer);
      if (interval != null) window.clearInterval(interval);
      if (!enabled) {
        // 关掉自动更新 = 没检查，不是「已是最新」。UI 对 unknown 只显示版本号。
        setStatus("unknown");
        return;
      }
      timer = window.setTimeout(() => {
        run();
        interval = window.setInterval(run, 12 * 60 * 60 * 1000);
      }, delayMs);
    };
    void getSettings()
      .then((settings) => {
        if (!disposed) applySettings(settings);
      })
      .catch(() => {
        if (!disposed) setStatus("error");
      });
    void listen<Settings>("settings-changed", ({ payload }) => {
      if (!disposed) applySettings(payload);
    })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {});
    return () => {
      disposed = true;
      if (timer != null) window.clearTimeout(timer);
      if (interval != null) window.clearInterval(interval);
      unlisten?.();
    };
  }, [automatic, delayMs, download, recheck]);

  return { status, version, notes, progress, download, cancelDownload, install, recheck };
}
