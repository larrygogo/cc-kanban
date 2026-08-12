#!/usr/bin/env bun
// 下载并校验新版 ConPTY(conpty.dll + OpenConsole.exe)到 app/src-tauri/binaries/conpty/,
// 由 tauri.windows.conf.json 的 bundle.resources 装进安装目录(与主程序同层)。
//
// 为什么打包它:托管 PTY 的僵死(输出停滞、Resize/ClosePseudoConsole 挂起)是 conhost
// 的内部死锁,微软在 Windows Terminal 仓库持续修,但**系统 conhost 随 OS 更新**——用户
// 机器上往往是几年前的老版本。portable-pty 的 load_conpty() 优先加载 DLL 搜索路径里的
// conpty.dll(psuedocon.rs;启动时的 SetDefaultDllDirectories 收紧后,应用目录仍在搜索
// 顺序内,见 lib.rs 的 harden_dll_search_path),打包新版等于把「有 bug 的实现」整个
// 换掉,是 ConPTY 卡死一类问题的治本项。conpty.dll 会以自己所在目录的 OpenConsole.exe
// 为宿主进程,两个文件必须成对部署。
//
// 供应链纪律:版本与全部 SHA-256 锁死在下面的常量里;下载跳转(api.nuget.org 在部分
// 网络下 302 到区域镜像)无碍——按内容哈希校验,来源不可信也放不进假货。升级版本时
// 重算三个哈希一并更新。非 Windows 构建直接跳过(mac 打包不带它)。
import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const VERSION = "1.24.260710001";
const PKG_SHA256 = "175640566a3b59c4b132070ee96c2c77e5ab7edd2e92732a5eb3610bbf63d90e";
// 包内 x64 成员(Meowo 目前只发 x64;上 arm64 时在此加表项并扩 resources 映射)。
const MEMBERS = [
  {
    entry: "runtimes/win-x64/native/conpty.dll",
    out: "conpty.dll",
    sha256: "39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8",
  },
  {
    entry: "build/native/runtimes/x64/OpenConsole.exe",
    out: "OpenConsole.exe",
    sha256: "b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160",
  },
];

if (process.platform !== "win32") {
  console.log("fetch-conpty: 非 Windows 构建,跳过");
  process.exit(0);
}

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const workspace = join(root, "app", "src-tauri");
const outDir = join(workspace, "binaries", "conpty");
mkdirSync(outDir, { recursive: true });

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");
const upToDate = MEMBERS.every((m) => {
  const path = join(outDir, m.out);
  return existsSync(path) && sha256(readFileSync(path)) === m.sha256;
});

if (!upToDate) {
  const url = `https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/${VERSION}/microsoft.windows.console.conpty.${VERSION}.nupkg`;
  console.log(`fetch-conpty: 下载 ConPTY ${VERSION} …`);
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) throw new Error(`fetch-conpty: 下载失败 HTTP ${response.status}`);
  const pkg = Buffer.from(await response.arrayBuffer());
  const actual = sha256(pkg);
  if (actual !== PKG_SHA256) {
    throw new Error(`fetch-conpty: 包哈希不符(供应链告警)\n  期望 ${PKG_SHA256}\n  实际 ${actual}`);
  }
  const tmp = mkdtempSync(join(tmpdir(), "conpty-"));
  try {
    writeFileSync(join(tmp, "pkg.zip"), pkg);
    // Windows 10+ 自带的 bsdtar 可直接抽取 zip 成员,不引第三方解压依赖。两个坑:
    // 必须用 System32 绝对路径(PATH 里 git-bash 的 GNU tar 在前,它不认 zip);
    // 归档与成员全用相对路径 + cwd(bsdtar 把绝对路径里的「C:」当远程主机名)。
    const bsdtar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
    execSync(`"${bsdtar}" -xf pkg.zip ${MEMBERS.map((m) => `"${m.entry}"`).join(" ")}`, { cwd: tmp });
    for (const m of MEMBERS) {
      const extracted = join(tmp, ...m.entry.split("/"));
      const got = sha256(readFileSync(extracted));
      if (got !== m.sha256) {
        throw new Error(`fetch-conpty: ${m.out} 哈希不符(供应链告警)\n  期望 ${m.sha256}\n  实际 ${got}`);
      }
      copyFileSync(extracted, join(outDir, m.out));
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
  console.log(`fetch-conpty: 就绪 ${outDir}`);
} else {
  console.log("fetch-conpty: 已就绪(哈希匹配),跳过下载");
}

// dev 便利:target/debug 存在就同步一份——`bun tauri dev` 的 exe 在那里,同目录有
// conpty.dll 才会被 portable-pty 选中,dev 也跑打包版(与线上同一实现,别让 dev
// 永远测不到它)。运行中的 dev 实例可能锁着旧文件,失败只警告不阻断。
const debugDir = join(workspace, "target", "debug");
if (existsSync(debugDir)) {
  for (const m of MEMBERS) {
    try {
      copyFileSync(join(outDir, m.out), join(debugDir, m.out));
    } catch (error) {
      console.warn(`fetch-conpty: 同步 ${m.out} 到 target/debug 失败(dev 实例占用?): ${error.message}`);
    }
  }
}
