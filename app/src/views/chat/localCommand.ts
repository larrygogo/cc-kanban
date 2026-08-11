// 用户在 CLI 里敲斜杠命令（/clear、/loop …）时，Claude Code 会把这次调用**当成一条
// 用户消息**写进 transcript，正文是一段 XML 包裹：命令名、命令的短描述、参数、命令的
// 标准输出，外加一段给模型看的免责声明（caveat）。原样渲染出来就是几行尖括号标签，
// 对着人显示「<command-name>/clear</command-name>」既难读又不像自己说过的话。
//
// 这里把它拆成结构，由 Message 渲染成命令徽章 + 可展开的输出，而不是在渲染层做正则。

/** 一次本地命令调用。args 为空串表示没带参数。 */
export type LocalCommandCall = { name: string; args: string };

export type UserTextParts = {
  /** 剥掉本地命令包裹后剩下的正文（用户真正打的字）。 */
  text: string;
  commands: LocalCommandCall[];
  /** 命令的标准输出，按出现顺序。空输出不收。 */
  stdout: string[];
  /** 后台任务通知（<task-notification> 的内文）：Claude Code 把它作为 user 消息注入
   *  transcript，原样摊开就是整屏 XML+JSON（实拍反馈）。渲染层收成可展开的一行。 */
  notifications: string[];
  /** 这条消息里出现过本地命令/系统注入包裹——渲染层据此切换形态。 */
  local: boolean;
};

/** 本地命令/系统注入包裹用到的标签。只认这几个，其它尖括号照旧当正文（用户可能真在讲 XML）。
 *  forked-skill-launch 是后台技能启动的机器记录（JSON），对人零信息量，直接丢。 */
const TAGS = ["local-command-caveat", "local-command-stdout", "command-name", "command-message", "command-args", "task-notification", "forked-skill-launch"] as const;
const PAIR = new RegExp(`<(${TAGS.join("|")})>([\\s\\S]*?)<\\/\\1>`, "g");
// 流式写入/截断会留下没配对的半个标签。正文里留一个孤零零的 `</command-args>` 比留着
// 整段 XML 好不到哪去，收尾时一并抹掉。
const STRAY = new RegExp(`<\\/?(${TAGS.join("|")})>`, "g");

export function parseUserText(raw: string): UserTextParts {
  const parts: UserTextParts = { text: "", commands: [], stdout: [], notifications: [], local: false };
  if (!raw.includes("<command-name>") && !raw.includes("<local-command-") && !raw.includes("<task-notification>")) {
    parts.text = raw;
    return parts;
  }
  parts.local = true;
  const rest = raw.replace(PAIR, (_match, tag: string, body: string) => {
    const value = body.trim();
    switch (tag) {
      case "command-name":
        parts.commands.push({ name: value, args: "" });
        break;
      case "command-args":
        // 参数归属最近一条命令；先于命令名出现（顺序在不同版本里变过）时补一条空壳，
        // 免得参数凭空消失。
        if (value) {
          const last = parts.commands[parts.commands.length - 1];
          if (last && !last.args) last.args = value;
          else parts.commands.push({ name: "", args: value });
        }
        break;
      case "local-command-stdout":
        if (value) parts.stdout.push(value);
        break;
      case "task-notification":
        if (value) parts.notifications.push(value);
        break;
      // command-message 是命令的短描述（"clear" 之于 /clear），与命令名重复；
      // caveat 是写给模型的免责声明、forked-skill-launch 是机器记录，对人零信息量。都丢。
      default:
        break;
    }
    return "";
  });
  // 通知消息的前导段（[SYSTEM NOTIFICATION - NOT USER INPUT] …）：写给模型的防误读
  // 声明，对人零信息量，随通知一起收走（只删到段落边界，防御性保留其后内容）。
  const cleaned = parts.notifications.length
    ? rest.replace(/\[SYSTEM NOTIFICATION - NOT USER INPUT\][\s\S]*?(?=\n\s*\n|$)/, "")
    : rest;
  parts.text = cleaned.replace(STRAY, "").trim();
  // 技能/后台命令的另一种落盘形态：命令行是**裸文本**（无 <command-name> 包裹），
  // 旁边跟着 caveat/stdout/forked-skill-launch。把首行的「/xxx [args]」提升为命令，
  // 与 <command-name> 形态同样渲染成徽章（实拍反馈「/code-review 没渲染好」）。
  // 只在本地包裹语境里做——用户在输入框正常打的「/xxx」是发给 CLI 的正文，不动。
  if (parts.commands.length === 0 && parts.text.startsWith("/")) {
    const [first, ...restLines] = parts.text.split("\n");
    const matched = /^\/(\S+)(?:\s+(.*))?$/.exec(first.trim());
    if (matched) {
      parts.commands.push({ name: `/${matched[1]}`, args: matched[2]?.trim() ?? "" });
      parts.text = restLines.join("\n").trim();
    }
  }
  return parts;
}
