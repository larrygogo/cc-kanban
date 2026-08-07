// preinstall 守卫：误跑 npm/yarn/pnpm install 时直接报错退出。
// 本仓库用 bun 管依赖——npm 装会产出第二份 lockfile（package-lock.json，.gitignore 已挡），
// 且装出的 node_modules 与 CI 的 `bun install --frozen-lockfile` 不是同一棵依赖树，
// 是「本地绿 / CI 红」的温床（实际发生过：本地 node_modules 曾是 npm 装的）。
//
// 由 **bun 自己**执行（package.json 的 preinstall 写的是 `bun ../scripts/ensure-bun.mjs`）：
// 用 node 执行的话，只装了 bun、没装 node 的环境连 `bun install` 都跑不起来——守卫把
// 想拦的和想放的一起拦了。npm 侧若机器上也没有 bun，会以「找不到命令」失败，同样达到
// 阻止的目的，只是提示不如下面这句具体。
//
// 检测靠 npm_config_user_agent：各安装器都会把自己写进去，形如 `bun/1.2.0 npm/? node/...`
// 或 `npm/10.8.1 node/v22...`。匹配 `bun/` 而不是裸 `bun`——后者会被路径/环境里任何含
// "bun" 的串误命中；大小写归一后再比，不赌各版本的大小写写法。
const agent = process.env.npm_config_user_agent ?? "";
if (!agent.toLowerCase().includes("bun/")) {
  console.error(
    `[meowo] 本项目用 bun 管理依赖，请运行 \`bun install\`（检测到安装器: ${agent || "未知"}）`,
  );
  process.exit(1);
}
