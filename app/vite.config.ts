/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// tauri CLI 驱动的构建会带 TAURI_ENV_PLATFORM；据此把产物目标钉到实际 WebView
// (Windows=WebView2/Chromium、macOS=WKWebView)，省掉 vite 默认 'modules' 目标的降级转译。
// 直接 `bun run build`(如 CI 的前端构建步骤)时无此变量：回退到发布目标中最低的
// safari13——能过 safari13 转译的代码在 chrome105 上必然可行，CI 验证的因此就是
// 发布目标(的超集严格度)，而非与发布无关的 'modules'。
const tauriPlatform = process.env.TAURI_ENV_PLATFORM;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  build: {
    target: tauriPlatform === "windows" ? "chrome105" : "safari13",
  },
  server: {
    port: 1268,
    strictPort: true,
    // src-tauri 整个排除出 vite 的文件监视(Tauri 官方模板同款):Rust workspace 连同
    // target/ 都住在 app/src-tauri 下,cargo 编译中的 .o 文件被占用,watcher 碰上就
    // EBUSY 崩掉 dev server;前端热更新也本来就不该关心 Rust 产物。
    watch: { ignored: ["**/src-tauri/**"] },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
