import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = dirname(fileURLToPath(import.meta.url));

/**
 * 前端不得按 agent 身份**分支**。
 *
 * 与后端 `host_code_does_not_branch_on_agent_identity` 同一条纪律：agent 的能力差异
 * 由插件声明、经 descriptor / chatUi 下发，前端只消费。历史上清理过两批——
 * `NetworkSection` 自维护的代理能力名单、`terminalAttention` 按 `provider ===` 的
 * 整句识别规则。
 *
 * 这类回潮**没有任何失败信号**：不会有测试变红、也不会报错，只会让下一个接进来的
 * agent 悄悄少一块能力（用户侧表现为「为什么 gemini 没有这个」），所以只能靠守卫拦。
 *
 * 只禁**比较**（`=== "claude"`、`includes("kimi")`），不禁数据声明
 * （`provider: "claude"`、`["claude"]`）——后者是首帧占位、默认文法自述、预览页固件，
 * 都会被后端下发的真值覆盖，不构成能力判断。
 */
const AGENT_IDS = ["claude", "codex", "kimi", "gemini", "opencode"];

/** 承载「按 agent 差异化行为」的文件：这里出现身份比较，几乎必然是该下发的能力。 */
const GUARDED_FILES = [
  "terminalAttention.ts",
  "views/settings/NetworkSection.tsx",
  "views/ChatWindow.tsx",
  "views/Sticker.tsx",
  "views/ChatSidebar.tsx",
  "api.ts",
];

/**
 * 身份**比较**的形态。数据声明（`x: "claude"` / `["claude"]`）刻意不在内。
 *
 * `[!=]=+` 而不是 `[!=]==`：松散相等 `p == "kimi"` 同样是分支，只写三等号会漏。
 * 字符串方法一并覆盖 startsWith/endsWith/indexOf/match/test——它们都能拿来做身份判断。
 */
const COMPARISONS = (id: string) => [
  new RegExp(`[!=]=+\\s*["']${id}["']`),
  new RegExp(`["']${id}["']\\s*[!=]=+`),
  new RegExp(`\\.(includes|has|startsWith|endsWith|indexOf|match|test)\\(\\s*["']${id}["']`),
  new RegExp(`\\bcase\\s+["']${id}["']`),
];

/**
 * 通过**中间变量**做身份判断的形态：`const P = "claude"; p === P`。
 *
 * 单看每一行都合法（一行是数据声明、另一行是变量比较），只有连起来看才是身份分支。
 * 故先收集「值为 agent id 的常量名」，再把它们当作 id 的别名重新扫一遍。
 */
function agentIdAliases(source: string): Map<string, string> {
  const aliases = new Map<string, string>();
  const declaration = /\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*["']([^"']+)["']/g;
  for (let m = declaration.exec(source); m; m = declaration.exec(source)) {
    if (AGENT_IDS.includes(m[2])) aliases.set(m[1], m[2]);
  }
  return aliases;
}

/** 变量别名的比较形态（`p === P`、`switch` 的 `case P`）。 */
const ALIAS_COMPARISONS = (name: string) => [
  new RegExp(`[!=]=+\\s*${name}\\b`),
  new RegExp(`\\b${name}\\s*[!=]=+`),
  new RegExp(`\\.(includes|has|startsWith|endsWith|indexOf)\\(\\s*${name}\\s*\\)`),
  new RegExp(`\\bcase\\s+${name}\\b`),
];

describe("架构守卫", () => {
  it("前端不按 agent 身份分支（能力一律由后端下发）", () => {
    const offences: string[] = [];
    for (const file of GUARDED_FILES) {
      const source = readFileSync(join(SRC, file), "utf8");
      const aliases = agentIdAliases(source);
      source.split("\n").forEach((line, index) => {
        const code = line.trim();
        // 注释里举例说明是正常的。
        if (code.startsWith("//") || code.startsWith("*") || code.startsWith("/*")) return;
        for (const id of AGENT_IDS) {
          if (COMPARISONS(id).some((re) => re.test(code))) {
            offences.push(`${file}:${index + 1} 按 ${id} 分支 → ${code}`);
          }
        }
        for (const [name, id] of aliases) {
          if (ALIAS_COMPARISONS(name).some((re) => re.test(code))) {
            offences.push(`${file}:${index + 1} 经变量 ${name} 按 ${id} 分支 → ${code}`);
          }
        }
      });
    }
    expect(offences).toEqual([]);
  });
});
