// 手机远程 UI 的独立构建(mobile.html + src/mobile/main.tsx)。输出 dist-mobile/,
// 由 tauri.conf.json 的 bundle.resources 打进安装包,运行时 remote.rs 从 resource_dir()/dist-mobile
// 静态托管。base="/" 因为内嵌 server 把它挂在根路径。与主 app 构建(tauri)、demo 构建互不影响。
// 用法:cd app && bun run build:mobile
import { defineConfig, type Plugin, type ResolvedConfig } from "vite";
import react from "@vitejs/plugin-react";
import { extname, join, resolve } from "node:path";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { brotliCompressSync, constants as zlib } from "node:zlib";

// 预压缩：给 assets/ 下的文本产物各生成一份同名 .br，remote.rs 看到 Accept-Encoding 带 br
// 就直接回它（content-encoding: br）。手机页此前裸传，首屏 1.1MB 压完约 0.45MB。
// 只压 assets/（文件名带内容哈希，.br 与源文件天然配对，不会错配）；mobile.html 不带哈希、
// 也只有 1KB，不压——否则手工覆盖 dist-mobile 里的 html 时旧 .br 会被继续送出去。
// woff2/png 本身已压缩，跳过。用 Node 自带 zlib，不引依赖。
const PRECOMPRESS_EXT = new Set([".js", ".css", ".svg", ".json"]);
function precompressBrotli(): Plugin {
  let config: ResolvedConfig;
  return {
    name: "meowo:precompress-brotli",
    apply: "build",
    configResolved(resolved) {
      config = resolved;
    },
    closeBundle() {
      const dir = join(config.build.outDir, "assets");
      let files: string[];
      try {
        files = readdirSync(dir);
      } catch {
        return;
      }
      for (const name of files) {
        if (!PRECOMPRESS_EXT.has(extname(name))) continue;
        const file = join(dir, name);
        const raw = readFileSync(file);
        const br = brotliCompressSync(raw, {
          params: {
            [zlib.BROTLI_PARAM_QUALITY]: 11,
            [zlib.BROTLI_PARAM_SIZE_HINT]: raw.length,
          },
        });
        // 压不动的（极小文件）不留 .br，省得多一次无谓的文件读取。
        if (br.length < raw.length * 0.9) writeFileSync(`${file}.br`, br);
      }
    },
  };
}

export default defineConfig({
  plugins: [react(), precompressBrotli()],
  base: "/",
  build: {
    outDir: resolve(__dirname, "dist-mobile"),
    emptyOutDir: true,
    rollupOptions: {
      input: resolve(__dirname, "mobile.html"),
    },
  },
});
