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

/** 身份**比较**的形态。数据声明（`x: "claude"` / `["claude"]`）刻意不在内。 */
const COMPARISONS = (id: string) => [
  new RegExp(`[!=]==\\s*["']${id}["']`),
  new RegExp(`["']${id}["']\\s*[!=]==`),
  new RegExp(`\\.includes\\(\\s*["']${id}["']\\s*\\)`),
  new RegExp(`\\.has\\(\\s*["']${id}["']\\s*\\)`),
  new RegExp(`\\bcase\\s+["']${id}["']`),
];

describe("架构守卫", () => {
  it("前端不按 agent 身份分支（能力一律由后端下发）", () => {
    const offences: string[] = [];
    for (const file of GUARDED_FILES) {
      const source = readFileSync(join(SRC, file), "utf8");
      source.split("\n").forEach((line, index) => {
        const code = line.trim();
        // 注释里举例说明是正常的。
        if (code.startsWith("//") || code.startsWith("*") || code.startsWith("/*")) return;
        for (const id of AGENT_IDS) {
          if (COMPARISONS(id).some((re) => re.test(code))) {
            offences.push(`${file}:${index + 1} 按 ${id} 分支 → ${code}`);
          }
        }
      });
    }
    expect(offences).toEqual([]);
  });
});
