import type { Metadata } from "next";
import DownloadContent from "@/components/pages/DownloadContent";

export const metadata: Metadata = {
  title: "Download · Meowo",
  description:
    "Download Meowo: Windows x64 installer, macOS DMG for Apple silicon or Intel (signed & notarized). No AI CLI required upfront — install and sign in inside the app.",
  alternates: { canonical: "/en/download/", languages: { "zh-CN": "/download/", en: "/en/download/" } },
};

export default function DownloadEn() {
  return <DownloadContent lang="en" />;
}
