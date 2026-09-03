"use client";

import { useEffect, useState } from "react";
import { DownloadIcon } from "./icons";
import type { Asset } from "@/lib/release";
import type { Lang } from "@/lib/i18n";

// "macos" = 认出是 Mac 但分不清芯片（Safari 没有 userAgentData），按 Apple 芯片处理。
type Platform = "windows" | "macos-arm" | "macos-intel" | "macos" | null;

type Props = {
  windows: Asset | null;
  macosArm: Asset | null;
  macosIntel: Asset | null;
  /** 认不出平台、或该平台没有安装包时的去处（下载页 / GitHub releases）。 */
  fallbackHref: string;
  className?: string;
  lang?: Lang;
};

const LABELS = {
  zh: {
    macosArm: "下载 .dmg（Apple 芯片）",
    macosIntel: "下载 .dmg（Intel Mac）",
    windows: "下载 .exe（Windows）",
    latest: "下载最新版",
  },
  en: {
    macosArm: "Download .dmg (Apple silicon)",
    macosIntel: "Download .dmg (Intel Mac)",
    windows: "Download .exe (Windows)",
    latest: "Download latest",
  },
};

type UADataArch = { getHighEntropyValues?: (hints: string[]) => Promise<{ architecture?: string }> };

// 服务端渲染时还不知道访客的系统，先给一个中性按钮；hydrate 后换成对应平台的直链。
function detectBase(): "windows" | "macos" | null {
  if (typeof navigator === "undefined") return null;
  const ua = navigator.userAgent;
  if (/iPhone|iPad|iPod|Android/i.test(ua)) return null; // 移动端没得下
  // iPadOS 桌面模式 UA 带 "Macintosh"，靠触屏点数排除
  if (/Mac/i.test(ua)) return navigator.maxTouchPoints > 1 ? null : "macos";
  if (/Win/i.test(ua)) return "windows";
  return null;
}

// Mac 的芯片只有 Chromium 系（Chrome / Edge / Arc）能经 userAgentData 报出来；
// Safari 与 Firefox 拿不到，UA 串里 Apple 芯片也伪装成 "Intel Mac OS X"，不能拿它判。
// 认不出时按 Apple 芯片给：Sonoma 能装的机器里 Apple 芯片占绝大多数，下载页另有 Intel 入口。
async function detect(): Promise<Platform> {
  const base = detectBase();
  if (base !== "macos") return base;
  const uaData = (navigator as unknown as { userAgentData?: UADataArch }).userAgentData;
  if (!uaData?.getHighEntropyValues) return "macos";
  try {
    const { architecture } = await uaData.getHighEntropyValues(["architecture"]);
    if (architecture === "arm") return "macos-arm";
    if (architecture === "x86") return "macos-intel";
  } catch {
    // 浏览器拒绝给高熵值时保持未知
  }
  return "macos";
}

export default function DownloadButton({
  windows,
  macosArm,
  macosIntel,
  fallbackHref,
  className = "btn btn-primary btn-lg",
  lang = "zh",
}: Props) {
  const [platform, setPlatform] = useState<Platform>(null);
  useEffect(() => {
    let alive = true;
    void detect().then((p) => {
      if (alive) setPlatform(p);
    });
    return () => {
      alive = false;
    };
  }, []);

  const t = LABELS[lang];
  let asset: Asset | null = null;
  let label = t.latest;
  if (platform === "windows" && windows) {
    asset = windows;
    label = t.windows;
  } else if (platform === "macos-intel" && macosIntel) {
    asset = macosIntel;
    label = t.macosIntel;
  } else if ((platform === "macos-arm" || platform === "macos") && macosArm) {
    asset = macosArm;
    label = t.macosArm;
  }
  const href = asset ? asset.url : fallbackHref;
  const external = href.startsWith("http");

  return (
    <a
      className={className}
      href={href}
      {...(external ? { target: "_blank", rel: "noopener noreferrer" } : {})}
    >
      <DownloadIcon />
      {label}
    </a>
  );
}
