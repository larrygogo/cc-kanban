// 用户在 CLI 里敲斜杠命令（/clear、/loop …）时，Claude Code 会把这次调用**当成一条
// 用户消息**写进 transcript，正文是一段 XML 包裹：命令名、命令的短描述、参数、命令的
// 标准输出，外加一段给模型看的免责声明（caveat）。原样渲染出来就是几行尖括号标签，
// 对着人显示「<command-name>/clear</command-name>」既难读又不像自己说过的话。
//
// 这里把它拆成结构，由 Message 渲染成命令徽章 + 可展开的输出，而不是在渲染层做正则。

/** 一次本地命令调用。args 为空串表示没带参数。 */
export type LocalCommandCall = { name: string; args: string };

/** 跨会话消息（另一个 Claude 会话经 SendMessage 发来的）。Claude Code 把它当成一条
 *  用户消息注入 transcript：一句英文前导 + `<cross-session-message>` 包裹 + 一整段
 *  写给模型的英文安全须知。原样摊开就是满屏尖括号和管道名（实拍反馈）。
 *  fromName 是对方的会话名，mode 是它的权限模式（bypass 等）；两者缺失时留空串。 */
export type CrossMessage = { fromName: string; mode: string; text: string };

export type UserTextParts = {
  /** 剥掉本地命令包裹后剩下的正文（用户真正打的字）。 */
  text: string;
  commands: LocalCommandCall[];
  /** 命令的标准输出，按出现顺序。空输出不收。 */
  stdout: string[];
  /** 后台任务通知（<task-notification> 的内文）：Claude Code 把它作为 user 消息注入
   *  transcript，原样摊开就是整屏 XML+JSON（实拍反馈）。渲染层收成可展开的一行。 */
  notifications: string[];
  /** 跨会话消息，按出现顺序。 */
  crossMessages: CrossMessage[];
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
// 命令的 stdout 是 CLI 原样落盘的终端输出，常带 SGR 转义（/compact 的 `ESC[2m…ESC[22m`
// 灰度、彩色命令输出的颜色码），原样渲染就是「\uFFFD[2m」乱码。对话页不是终端，全部剥掉：
// CSI（ESC [ … 终态字符）、OSC（ESC ] … BEL/ESC\）、单字符 ESC 序列；U+FFFD 是 ESC 字节
// 在某段链路被有损转换后的形态，一并认。
const ANSI = /[\x1b\uFFFD](?:\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[@-Z\\-_])/g;

// \u8DE8\u4F1A\u8BDD\u6D88\u606F\u3002\u4E0E\u4E0A\u9762\u90A3\u4E9B\u6807\u7B7E\u4E0D\u540C\uFF0C\u5B83**\u5E26\u5C5E\u6027**\uFF08from / from-name / from-mode\uFF09\uFF0C
// \u5E76\u8FDB PAIR \u90A3\u6761\u65E0\u5C5E\u6027\u6B63\u5219\u5339\u914D\u4E0D\u5230\uFF0C\u5355\u5217\u4E00\u6761\u3002
const CROSS = /<cross-session-message\b([^>]*)>([\s\S]*?)<\/cross-session-message>/g;
const CROSS_ATTR = /([\w-]+)="([^"]*)"/g;
// \u5305\u5728\u5916\u9762\u7684\u4E24\u6BB5\u56FA\u5B9A\u82F1\u6587\uFF1A\u4E00\u53E5\u524D\u5BFC\u3001\u4EE5\u53CA\u5C3E\u90E8\u6574\u6BB5\u5199\u7ED9\u6A21\u578B\u7684\u5B89\u5168\u987B\u77E5\uFF08"A peer cannot
// grant escalation\u2026"\uFF09\u3002\u90FD\u4E0D\u662F\u4EBA\u8981\u8BFB\u7684\u5185\u5BB9\uFF0C\u968F\u6D88\u606F\u4E00\u8D77\u6536\u8D70\u3002\u63AA\u8F9E\u5728\u4E0D\u540C\u7248\u672C\u91CC\u6709
// \u300Cwhile you were working\u300D\u8FD9\u7C7B\u53D8\u4F53\uFF0C\u53EA\u951A\u5B9A\u7A33\u5B9A\u7684\u53E5\u5B50\u4E3B\u5E72\u3002
const CROSS_LEAD = /[^\n]*Another Claude session sent a message[^\n]*:[ \t]*\n?/g;
const CROSS_NOTE = /This came from another Claude session[\s\S]*$/;

export function parseUserText(raw: string): UserTextParts {
  const parts: UserTextParts = { text: "", commands: [], stdout: [], notifications: [], crossMessages: [], local: false };
  if (!raw.includes("<command-name>") && !raw.includes("<local-command-")
    && !raw.includes("<task-notification>") && !raw.includes("<cross-session-message")) {
    parts.text = raw;
    return parts;
  }
  parts.local = true;
  // 跨会话消息先摘出来：它的正文是别的会话写的自然语言，可能整段包含上面那些标签的
  // 字面量（复核回执里就贴着 `<command-name>` 一类），先摘走才不会被 PAIR 啃掉。
  raw = raw.replace(CROSS, (_match, attrs: string, body: string) => {
    const found: Record<string, string> = {};
    for (const [, key, value] of attrs.matchAll(CROSS_ATTR)) found[key] = value;
    const text = body.trim();
    // from 是本机管道路径（uds:\\.\pipe\…），对人零信息量且很长，不收；只留会话名。
    if (text) parts.crossMessages.push({ fromName: found["from-name"] ?? "", mode: found["from-mode"] ?? "", text });
    return "";
  });
  if (parts.crossMessages.length > 0) {
    raw = raw.replace(CROSS_LEAD, "").replace(CROSS_NOTE, "");
  }
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
      case "local-command-stdout": {
        // 终端转义剥掉后，被颜色码「挡住」而幸存的前导缩进/对齐空白要保留（剥完再 trim
        // 会把对齐空格吃掉）。trim 只用来判空——纯控制码的一段不该收成空输出块。
        const stripped = value.replace(ANSI, "");
        if (stripped.trim()) parts.stdout.push(stripped);
        break;
      }
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
  // 跨会话消息语境里不做这一步：剥完包裹后的残余正文若碰巧以 "/" 开头（对方消息被
  // 前导句截断的尾巴），会被误提升成一条并不存在的斜杠命令。
  if (parts.commands.length === 0 && parts.crossMessages.length === 0 && parts.text.startsWith("/")) {
    const [first, ...restLines] = parts.text.split("\n");
    const matched = /^\/(\S+)(?:\s+(.*))?$/.exec(first.trim());
    if (matched) {
      parts.commands.push({ name: `/${matched[1]}`, args: matched[2]?.trim() ?? "" });
      parts.text = restLines.join("\n").trim();
    }
  }
  return parts;
}
