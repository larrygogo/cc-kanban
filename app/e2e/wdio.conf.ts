import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { TauriCapabilities } from "@wdio/tauri-service";

// 本文件在 ESM 下运行（app/package.json "type":"module"），无 __dirname → 从 import.meta.url 求。
const here = dirname(fileURLToPath(import.meta.url));

// E2E 二进制路径：Cargo workspace 根在 app/src-tauri，target 也在它下面
// （app/e2e → app → src-tauri → target/debug）。
// 名字取自 cargo package "meowo-app"（e2e/run.mjs 用 `tauri build --debug --no-bundle` 产出，不改名）。
const appBinary = join(
  here,
  "..",
  "src-tauri",
  "target",
  "debug",
  process.platform === "win32" ? "meowo-app.exe" : "meowo-app",
);

// 用 TauriCapabilities 定型（含 `tauri:options`），避免直接写字面量触发多余属性检查。
// 内嵌 WebDriver provider（默认，全平台）由 tauri-plugin-wdio-webdriver 提供，无需外置 tauri-driver。
const capabilities: TauriCapabilities[] = [
  {
    browserName: "tauri",
    "tauri:options": { application: appBinary },
  },
];

export const config: WebdriverIO.Config = {
  runner: "local",
  tsConfigPath: join(here, "tsconfig.json"),
  specs: [join(here, "specs", "**", "*.e2e.ts")],
  // 串行：贴纸 app 是单例窗口，且测试要观察全局刷新节奏，不并行。
  maxInstances: 1,
  capabilities,
  services: [
    [
      "@wdio/tauri-service",
      {
        // 内嵌 provider（tauri-plugin-wdio-webdriver 提供的 WebDriver 服务器），不依赖外置 tauri-driver。
        driverProvider: "embedded",
        // 等内嵌 WebDriver 绑上端口的上限。默认 60s 在 GitHub Actions 的冷 runner 上不够：
        // debug 构建 + 首次 WebView2 初始化 + 无任何预热，实测超时（run 33733314763 报
        // 「did not become ready on port 4445 within 60000ms」）。本机通常几秒就绪，
        // 放宽只影响真出问题时的等待时长，不拖慢正常路径。
        startTimeout: 180_000,
        // Windows：按本机 WebView2 版本自动下载匹配的 msedgedriver（驱动 Edge 内核所需）。
        autoDownloadEdgeDriver: true,
        // 把前端 console 与后端日志转发到 wdio 输出，便于排查。
        captureFrontendLogs: true,
        captureBackendLogs: true,
      },
    ],
  ],
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 180_000 },
  logLevel: "warn",
};
