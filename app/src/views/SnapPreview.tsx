import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

// W-2 原生吸附预览窗（Windows）的全部内容：一整条幽灵色块。颜色/透明度走主题 CSS 变量
// （main.tsx 的 bootAppearance 对本窗同样套主题），与 .snap-ghost 同色系。
// 挂载即向 Rust 握手（snap_preview_ready）：此前窗口保持 hidden（snap_preview.rs 的
// READY 门），杜绝首帧未上屏时的白框/空框闪烁。
export function SnapPreview() {
  useEffect(() => {
    invoke("snap_preview_ready").catch(() => {});
  }, []);
  return <div className="snap-preview-bar" aria-hidden="true" />;
}
