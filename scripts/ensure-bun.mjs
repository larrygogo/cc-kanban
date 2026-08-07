// preinstall 守卫：误跑 npm/yarn/pnpm install 时直接报错退出。
// 本仓库用 bun 管依赖——npm 装会产出第二份 lockfile（package-lock.json，.gitignore 已挡），
// 且装出的 node_modules 与 CI 的 `bun install --frozen-lockfile` 不是同一棵依赖树，
// 是「本地绿 / CI 红」的温床（实际发生过：本地 node_modules 曾是 npm 装的）。
// 检测靠 npm_config_user_agent：各安装器都会把自己写进去（bun 的形如 "bun/1.x.y npm/? ..."）。
// app/ 与 site/ 的 preinstall 都指向本文件（相对路径 ../scripts/ensure-bun.mjs）。
const agent = process.env.npm_config_user_agent ?? "";
if (!agent.includes("bun")) {
  console.error(
    `[meowo] 本项目用 bun 管理依赖，请运行 \`bun install\`（检测到安装器: ${agent || "未知"}）`,
  );
  process.exit(1);
}
