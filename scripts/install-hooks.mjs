// scripts/install-hooks.mjs
// 用法：bun scripts/install-hooks.mjs <meowo-reporter 可执行文件绝对路径> [settingsPath]
// 把 Meowo 的 hooks 幂等合并进 settings.json，不破坏已有配置。
// 仅装 Claude Code 的 hooks（写入 ~/.claude/settings.json；会话默认 provider=claude）。
// codex / kimi 不经此脚本——它们由各自 CLI 的原生 hook 配置接入，hook 命令各带 --provider codex|kimi。
import { readFileSync, writeFileSync, copyFileSync, renameSync, existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";

const reporter = process.argv[2];
if (!reporter) {
  console.error("用法: bun scripts/install-hooks.mjs <meowo-reporter 可执行文件绝对路径> [settingsPath]");
  process.exit(1);
}

// 优先级：命令行第 2 参 > 环境变量 > 默认 ~/.claude/settings.json
const settingsPath =
  process.argv[3] ||
  process.env.MEOWO_SETTINGS ||
  join(homedir(), ".claude", "settings.json");

mkdirSync(dirname(settingsPath), { recursive: true });

let settings = {};
if (existsSync(settingsPath)) {
  try {
    settings = JSON.parse(readFileSync(settingsPath, "utf8"));
  } catch (e) {
    console.error(`解析 ${settingsPath} 失败: ${e.message}`);
    process.exit(1);
  }
}
// 合并前的结构校验：settings 必须是普通对象。顶层为数组时，下面的 `hooks ??= {}` 会把
// 属性挂到数组上、JSON.stringify 时静默丢弃；标量同理。宁可报错退出也不写坏用户配置。
if (typeof settings !== "object" || settings === null || Array.isArray(settings)) {
  console.error(`${settingsPath} 结构异常：顶层必须是 JSON 对象，请修复或备份后删除该文件再重试`);
  process.exit(1);
}
settings.hooks ??= {};

// 注意：此表须与 app/src-tauri/crates/meowo-agent/src/plugins/claude/mod.rs 的 EVENTS 保持一致（两处各维护一份，
// 改一处必同步另一处）。meowo-app 的 hook_specs_match_install_hooks_mjs 单测会解析本文件比对，
// 不一致即失败——不要改这里的字面量格式（逐行 ["事件", "matcher"],）。
const SPECS = [
  ["SessionStart", "*"],
  ["UserPromptSubmit", "*"],
  ["PostToolUse", "*"],
  ["Stop", "*"],
  ["SessionEnd", "*"],
  ["PermissionRequest", "*"],
  ["PreToolUse", "AskUserQuestion"],
  ["PreToolUse", "ExitPlanMode"],
];
// 写入的 command：双引号包住路径以防空格
const command = `"${reporter}"`;

// 认领规则：命令恰为我方 reporter 可执行文件（可带引号、**不得带参数**），只按文件名判定。
// 与 Rust 侧 `CommandSpec::claim` 同规则，两点都要紧：
//   - 按文件名（而非整串）认领，故 reporter 路径变了（debug→release、换安装目录）也认得出。
//     旧实现只删「command 完全相同」的条目，路径一变就留下旧条目、再追加一条 → 每换一次路径
//     重复翻一倍，而 Claude Code 会把同事件下的条目逐条执行，重复 N 条即每次事件派生 N 个进程。
//   - 禁带参数，才不会误伤用户自己的 hook（如 `node tools/meowo-reporter-notify.js`）。
const REPORTERS = new Set(["meowo-reporter", "meowo-reporter.exe", "cc-reporter", "cc-reporter.exe"]);
function isOurs(cmd) {
  if (typeof cmd !== "string") return false;
  const m = cmd.trim().match(/^"([^"]+)"$|^([^\s"]+)$/);
  const path = m && (m[1] ?? m[2]);
  return !!path && REPORTERS.has(path.split(/[\\/]/).pop().toLowerCase());
}

for (const [event, matcher] of SPECS) {
  // 等人的 hook（审批、AskUserQuestion 代答）用长超时，与 Rust 侧 EVENTS 的 with_timeout 同步。
  const waitsOnUser =
    event === "PermissionRequest" || (event === "PreToolUse" && matcher === "AskUserQuestion");
  const timeout = waitsOnUser ? 310 : 5;
  settings.hooks[event] ??= [];
  // hooks[event] 非数组（如被手改成对象）时下面的 .map 会抛裸 TypeError，先校验再合并。
  if (!Array.isArray(settings.hooks[event])) {
    console.error(`${settingsPath} 结构异常：hooks.${event} 必须是数组，请修复后重试`);
    process.exit(1);
  }
  // 同一 (event, matcher) 下只留一条我方 hook：第一条更新为当前路径，其余删除。
  // 用户自有的 hook（isOurs 不认领的）一概不动——包括与我方 hook 同壳的。
  let kept = false;
  settings.hooks[event] = settings.hooks[event]
    .map((entry) => {
      if (entry.matcher !== matcher) return entry; // 别的 matcher（含用户自有）→ 不动
      const hooks = (entry.hooks ?? []).filter((h) => {
        if (!isOurs(h.command)) return true; // 用户自有 hook → 留
        if (kept) return false; // 我方第 2+ 条 → 重复注册，删
        kept = true;
        h.command = command;
        // 等人的 hook 必须强制迁移到长超时（旧装机残留 5 会掐断代答等待）；
        // 普通 hook 仍限制 5 秒，且保留用户已有的自定义值。
        if (waitsOnUser) h.timeout = timeout;
        else h.timeout ??= timeout;
        return true;
      });
      return { ...entry, hooks };
    })
    .filter((entry) => entry.matcher !== matcher || (entry.hooks ?? []).length > 0); // 删空后的壳不留
  if (!kept) {
    settings.hooks[event].push({
      matcher,
      hooks: [{ type: "command", command, timeout }],
    });
  }
}

// 防半截 JSON（这是用户 Claude Code 的全部配置）：旧文件先备份为 .bak，
// 内容写同目录 .tmp 再 rename 原子替换——同目录 rename 才保证原子。
if (existsSync(settingsPath)) copyFileSync(settingsPath, settingsPath + ".bak");
writeFileSync(settingsPath + ".tmp", JSON.stringify(settings, null, 2));
renameSync(settingsPath + ".tmp", settingsPath);
console.log(`已写入 ${settingsPath}，挂载: ${SPECS.map(([e, m]) => `${e}:${m}`).join(", ")}`);
