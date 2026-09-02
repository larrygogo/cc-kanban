// E2E 编排器：一条命令跑通「构建 E2E 专用二进制 → 跑 WDIO → 清理」。跨平台（node + spawnSync）。
//
// 为什么不是直接 `wdio run`：E2E 需要一个特制的 app 二进制——注入了 WDIO 插件（--features e2e）、
// 打开了 withGlobalTauri、且前端带了 @wdio/tauri-plugin 桥（VITE_E2E=1）。这些**绝不能进生产**，
// 故都以构建期开关注入，并在此临时拷入 WDIO capability、跑完即删。
import { spawnSync } from "node:child_process";
import { copyFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = join(here, ".."); // app/
const capDst = join(appDir, "src-tauri", "capabilities", "wdio.json");

// Node ≥ 26 预检：其内置 undici v8 严格执行 Fetch 规范，拒绝 wdio `webdriver` 手动设的
// Content-Length / Connection 头，创建会话时报 UND_ERR_INVALID_ARG（webdriverio#15265）。
// 快速失败并给出指引，好过让用户对着一屏 undici 栈发懵。修复前请用 Node 22 LTS 跑 E2E。
const nodeMajor = Number(process.versions.node.split(".")[0]);
if (nodeMajor >= 26) {
  console.error(
    `\n✗ 当前 Node ${process.versions.node} 与 WebdriverIO 不兼容（webdriverio#15265）：\n` +
      `  Node ≥ 26 的 undici v8 会拒绝 wdio 设的 Content-Length/Connection 请求头，创建会话即失败。\n` +
      `  请用 Node 22 LTS 跑 E2E（如 \`fnm use 22\` / \`nvm use 22\`，或用便携版 Node 22 的 node.exe 直接执行 e2e/run.mjs）。\n`,
  );
  process.exit(1);
}

function run(bin, args, env = {}) {
  console.log(`\n▶ ${bin} ${args.join(" ")}`);
  const r = spawnSync(bin, args, {
    cwd: appDir,
    stdio: "inherit",
    env: { ...process.env, ...env },
    shell: process.platform === "win32", // Windows 下解析 bun/.cmd 需要 shell
  });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`退出码 ${r.status}: ${bin} ${args.join(" ")}`);
}

// WDIO 权限只在 e2e 构建存在：若把 wdio.json 常驻 capabilities/，非 e2e 构建下 `wdio:default`
// 权限未定义会直接编译失败。故临时拷入、跑完必清理。
const cleanup = () => {
  try {
    rmSync(capDst, { force: true });
  } catch {
    /* noop */
  }
};
// 正常/异常退出走下面的 finally；但 finally 不覆盖信号中断（Ctrl-C 的 SIGINT / SIGTERM）——
// 若在耗时数分钟的构建期间被中断而不清理，残留的 wdio.json 会让后续普通构建因 `wdio:default`
// 权限未定义而编译失败。故显式捕获信号清理后再退出。
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    cleanup();
    process.exit(1);
  });
}
// 兜底：清理任何上一次被硬中断遗留的残留，再拷入本次的。
cleanup();
copyFileSync(join(here, "wdio.capability.json"), capDst);
try {
  // 构建 E2E 二进制：
  //  --features e2e            注入 tauri-plugin-wdio(-webdriver)
  //  --config …tauri.e2e…      合并 withGlobalTauri=true（不改生产 tauri.conf.json）
  //  --debug --no-bundle       出到 target/debug 的裸可执行（wdio.conf 指向此处），不打安装包
  //  VITE_E2E=1                前端注入 @wdio/tauri-plugin 桥 + board-changed 观测计数
  run(
    "bun",
    ["x", "tauri", "build", "--debug", "--no-bundle", "--features", "e2e", "--config", "src-tauri/tauri.e2e.conf.json"],
    { VITE_E2E: "1" },
  );
  // 跑 WDIO（embedded provider：首次会自动匹配/下载 msedgedriver）。
  // 必须用 Node（preflight 已保证 ≤22）而不是 bun：bun 的 fetch 与 Node≥26 的 undici 同样
  // 拒绝 wdio webdriver 手动设置的 Content-Length/Connection 头（实测 bun 下创建会话即
  // UND_ERR_INVALID_ARG）——预检挡了 Node 版本却没换运行时，等于白挡。process.execPath
  // 即当前 Node；`bun x` 只是装包/解析便利，不能用来跑 wdio 本体。
  const require = createRequire(import.meta.url);
  // @wdio/cli 的 exports 只放 "."，bin 不能直接 resolve——从主入口推包根再进 bin。
  const wdioBin = join(dirname(require.resolve("@wdio/cli")), "..", "bin", "wdio.js");
  run(process.execPath, [wdioBin, "run", "e2e/wdio.conf.ts"]);
} finally {
  cleanup();
}
