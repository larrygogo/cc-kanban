#!/usr/bin/env bun
// 编译 meowo-reporter 并按 Tauri sidecar 命名规则放进 app/src-tauri/binaries/：
//   meowo-reporter-<target-triple>[.exe]
// 这样 tauri.conf.json 的 bundle.externalBin 才能把它打进安装包（装到主程序同目录，
// 供 ccsetup 无感接线时找到）。
//
// 用法：bun scripts/prepare-sidecar.mjs [--universal]
//   默认       —— 构建目标 triple：优先 TAURI_ENV_TARGET_TRIPLE（tauri 的 before 钩子
//                 会注入），否则取 rustc 宿主 triple（本地 dev / build）。
//   --universal —— macOS 双架构分别编译后 lipo 合并为 meowo-reporter-universal-apple-darwin
//                 （本地 `tauri build --target universal-apple-darwin` 用；release CI 已改为
//                 arm64 / x64 两个独立包，不再走这条）。
import { execSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
// Rust workspace 根在 app/src-tauri(见其 Cargo.toml 的 [workspace]):cargo 从这里起跑,
// 产物也落在它下面的 target/。
const workspace = join(root, "app", "src-tauri");
const outDir = join(workspace, "binaries");
mkdirSync(outDir, { recursive: true });

const run = (cmd) => execSync(cmd, { cwd: workspace, stdio: "inherit" });
const hostTriple = () =>
  execSync("rustc --print host-tuple").toString().trim();

const triple = process.argv.includes("--universal")
  ? "universal-apple-darwin"
  : process.env.TAURI_ENV_TARGET_TRIPLE || hostTriple();

// triple 会插值进 execSync 的 shell 字符串，先钉死字符集（rustc triple 只含
// 字母/数字/_/-，universal-apple-darwin 同），杜绝环境变量注入。
if (!/^[\w-]+$/.test(triple)) {
  console.error(`非法 target triple: ${JSON.stringify(triple)}`);
  process.exit(1);
}

if (triple === "universal-apple-darwin") {
  const arches = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
  for (const t of arches) {
    run(`cargo build --release -p meowo-reporter --target ${t}`);
    // universal 构建会按各架构分别编译 meowo-app，tauri_build 编译期按当前
    // TARGET triple 校验 externalBin，单架构文件也必须在 binaries/ 在场
    copyFileSync(
      join(workspace, "target", t, "release", "meowo-reporter"),
      join(outDir, `meowo-reporter-${t}`),
    );
  }
  const out = join(outDir, "meowo-reporter-universal-apple-darwin");
  const slices = arches
    .map((t) => `"${join(workspace, "target", t, "release", "meowo-reporter")}"`)
    .join(" ");
  run(`lipo -create -output "${out}" ${slices}`);
  chmodSync(out, 0o755); // lipo 按 umask 建文件，不保证可执行位
  console.log(`sidecar 就绪: ${out}`);
} else {
  // 宿主 triple 不传 --target:带了 --target 的产物落在 target/<triple>/release,
  // 而 `tauri build`(不带 --target)的主构建落在 target/release,两棵依赖树互不复用,
  // meowo-store/agent 及全部传递依赖被完整编两遍(release CI 实测白花 1m39s)。
  // 交叉编译(triple ≠ 宿主)仍必须带 --target,产物目录也随之带 triple。
  const cross = triple !== hostTriple();
  run(`cargo build --release -p meowo-reporter${cross ? ` --target ${triple}` : ""}`);
  const ext = triple.includes("windows") ? ".exe" : "";
  const src = join(workspace, "target", ...(cross ? [triple] : []), "release", `meowo-reporter${ext}`);
  const dst = join(outDir, `meowo-reporter-${triple}${ext}`);
  copyFileSync(src, dst);
  console.log(`sidecar 就绪: ${dst}`);
}
