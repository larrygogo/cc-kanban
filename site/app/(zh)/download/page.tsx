import type { Metadata } from "next";
import DownloadContent from "@/components/pages/DownloadContent";

export const metadata: Metadata = {
  title: "下载 · Meowo",
  description:
    "下载 Meowo：Windows x64 安装包，macOS 按 Apple 芯片 / Intel 各一个 DMG（已签名公证）。无需预装 AI CLI，可在应用内一键安装和登录。",
  alternates: { canonical: "/download/", languages: { "zh-CN": "/download/", en: "/en/download/" } },
};

export default function DownloadPage() {
  return <DownloadContent lang="zh" />;
}
