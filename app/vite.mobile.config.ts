// 手机远程 UI 的独立构建(mobile.html + src/mobile/main.tsx)。输出 dist-mobile/,
// 由 tauri.conf.json 的 bundle.resources 打进安装包,运行时 remote.rs 从 resource_dir()/dist-mobile
// 静态托管。base="/" 因为内嵌 server 把它挂在根路径。与主 app 构建(tauri)、demo 构建互不影响。
// 用法:cd app && bun run build:mobile
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [react()],
  base: "/",
  build: {
    outDir: resolve(__dirname, "dist-mobile"),
    emptyOutDir: true,
    rollupOptions: {
      input: resolve(__dirname, "mobile.html"),
    },
  },
});
