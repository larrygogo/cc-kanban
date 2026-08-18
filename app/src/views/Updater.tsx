import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { getVersion } from "@tauri-apps/api/app";
import { useUpdate } from "../useUpdate";
import { useShowWhenReady } from "../useShowWhenReady";
import { useT } from "../i18n";
import { getLiveSessionsPage } from "../api";
import { appConfirm } from "../confirm";
import logoUrl from "../../src-tauri/icons/128x128.png";
import { useEscClose } from "../hooks/useEscClose";

// 紧凑高度（检查中/已最新/失败：只有状态行+按钮）与增高高度（发现新版：带更新说明滚动区）。
// 与 Rust 侧 open_update_window_impl 的初始 inner_size 保持一致。
const COMPACT_H = 252;
const TALL_H = 500;
const WIN_W = 400;

// 软件更新窗口（label "updater"）：检查/下载/安装的唯一所有者。
// 主窗红点与设置页按钮只负责经 open_update_window 命令打开本窗口,
// 不再有跨窗口 trigger-update/update-failed 事件协议(旧协议曾致按钮死锁)。
export function Updater() {
  const t = useT();
  // 窗口以 visible:false 创建（window.rs），首帧渲染后再显示，消除打开瞬间的白框闪烁。
  useShowWhenReady();
  const { status, version, notes, progress, download, install, recheck } = useUpdate();
  const [current, setCurrent] = useState("");
  useEffect(() => {
    getVersion().then(setCurrent).catch(() => {});
  }, []);
  const close = () => getCurrentWindow().close().catch(() => {});
  useEscClose(close); // Esc = 「稍后」：关窗不打断下载（下载是后端进程级任务）

  // 重启安装前先看有没有托管会话在跑：应用退出会 shutdown 全部托管 PTY（lib.rs RunEvent::Exit），
  // 正在跑的 agent 会被当场杀掉——必须过一次危险确认，不能静默。查询失败不阻断更新（按无会话处理）。
  const [installing, setInstalling] = useState(false);
  const restartAndInstall = async () => {
    if (installing) return;
    setInstalling(true);
    try {
      let managed = 0;
      try {
        const page = await getLiveSessionsPage("all", null, null, 200);
        managed = page.items.filter((l) => l.connected && l.pty_managed).length;
      } catch {
        /* 查询失败按 0 处理，不挡更新 */
      }
      if (managed > 0) {
        const ok = await appConfirm(t.updater.restartConfirm(managed), {
          title: t.updater.restart,
          danger: true,
        });
        if (!ok) return;
      }
      await install();
    } finally {
      setInstalling(false);
    }
  };

  // 有更新说明可读时（发现新版/下载中）窗口增高，其余状态收回紧凑高度并保持居中。
  // Windows 上 resizable(false) 会把窗口 min/max 锁死成当前尺寸，程序化 setSize 也被钳住——
  // 必须先临时放开 resizable、改完尺寸立刻锁回，再重新居中。
  const tall = (status === "available" || status === "downloading" || status === "ready") && !!notes;
  useEffect(() => {
    const resize = async () => {
      const w = getCurrentWindow();
      await w.setResizable(true);
      await w.setSize(new LogicalSize(WIN_W, tall ? TALL_H : COMPACT_H));
      await w.setResizable(false);
      await w.center();
    };
    try {
      void resize().catch(() => {});
    } catch {
      /* 非 Tauri 环境（测试/浏览器） */
    }
  }, [tall]);

  // dev 专用:一键切换「有可用更新」mock(见 useUpdate 的 devMockUpdate),生产构建剔除。
  const MOCK_KEY = "meowo-mock-update";
  const toggleDevMock = () => {
    if (localStorage.getItem(MOCK_KEY)) localStorage.removeItem(MOCK_KEY);
    else localStorage.setItem(MOCK_KEY, "9.9.9");
    void recheck();
  };

  return (
    <div className="updater">
      <div className="up-bar" data-tauri-drag-region>
        {import.meta.env.DEV && (
          <button className="up-devmock" onClick={toggleDevMock}>
            dev: 切换有更新预览
          </button>
        )}
        <button className="winclose" data-tip={t.settings.close} aria-label={t.settings.close} onClick={close}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
            <line x1="6" y1="6" x2="18" y2="18" />
            <line x1="18" y1="6" x2="6" y2="18" />
          </svg>
        </button>
      </div>
      <div className={"up-body" + (tall ? "" : " up-compact")}>
        <div className="up-head">
          <img className="up-logo" src={logoUrl} width={48} height={48} alt="" />
          <div className="up-head-text">
            <div className="up-name">Meowo</div>
            <div className="up-cur">{t.updater.current(current || "—")}</div>
          </div>
        </div>

        {status === "checking" && <div className="up-status">{t.updater.checking}</div>}

        {status === "latest" && (
          <>
            <div className="up-status">{t.updater.latest}</div>
            <button className="sbtn" onClick={() => void recheck()}>{t.updater.recheck}</button>
          </>
        )}

        {status === "error" && (
          <>
            <div className="up-status up-err">{t.updater.error}</div>
            <button className="sbtn" onClick={() => void recheck()}>{t.updater.retry}</button>
          </>
        )}

        {(status === "available" || status === "downloading" || status === "ready") && (
          <>
            <div className="up-status up-new">{t.updater.found(version ?? "")}</div>
            {notes && (
              <div className="up-notes">
                <div className="up-notes-title">{t.updater.notes}</div>
                {/* release notes 是 Markdown 源文本,这里按纯文本保行渲染(不引 md 解析器) */}
                <pre className="up-notes-body">{notes}</pre>
              </div>
            )}
            {status === "available" ? (
              <button className="sbtn primary" onClick={() => void download()}>{t.updater.download}</button>
            ) : status === "downloading" ? (
              <div className="up-dl">
                {/* progress 为 null 时总大小未知：indeterminate 进度条按 ARIA 规范不给 aria-valuenow。 */}
                <div
                  className={"up-prog" + (progress == null ? " up-prog-indet" : "")}
                  role="progressbar"
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={progress == null ? undefined : Math.round(progress)}
                >
                  <div className="up-prog-fill" style={{ width: `${progress ?? 100}%` }} />
                </div>
                <div className="up-status-dl">
                  {progress != null ? t.updater.downloadingPct(progress) : t.updater.downloading}
                </div>
                <div className="up-hint">{t.updater.restartHint}</div>
              </div>
            ) : (
              <>
                <div className="up-status">{t.updater.ready}</div>
                <div className="up-hint">{t.updater.readyHint}</div>
                {/* 「稍后」语义化关窗：此前关窗是唯一的推迟方式，但右上角 ✕ 读不出
                    「更新还在、随时可装」——并排给出两条路，主次分明。 */}
                <div className="up-actions">
                  <button className="sbtn" onClick={close}>{t.updater.later}</button>
                  <button className="sbtn primary" disabled={installing} onClick={() => void restartAndInstall()}>{t.updater.restart}</button>
                </div>
              </>
            )}
            {/* manifest 的 notes 缺失/过简时，完整改动去 Releases 看——想先读改动再决定的
                用户此前要绕道设置页才能找到这个链接。 */}
            <button className="up-changelog" onClick={() => void invoke("open_url", { url: "https://github.com/larrygogo/meowo/releases" }).catch(() => {})}>
              {t.updater.fullChangelog}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
