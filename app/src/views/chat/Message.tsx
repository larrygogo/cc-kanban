import { useT } from "../../i18n";
import { type ChatItem } from "../../api";
import { ChatMarkdown } from "../ChatMarkdown";
import { parseUserText } from "./localCommand";

/// 超过这个行数的思考过程收成预览态。与 styles.css 里 `.chat-reasoning.is-long` 的
/// max-height 是同一个意思，改一个要顺带看另一个。
const REASONING_PREVIEW_LINES = 6;

export function Message({ item }: { item: ChatItem }) {
  const t = useT();
  if (item.type === "user_text") {
    // 斜杠命令在 transcript 里是一条 XML 包裹的用户消息。渲染成命令徽章 + 可展开输出，
    // 而不是把 <command-name> 这类标签摊在对话里让人自己读。
    const parts = parseUserText(item.text);
    if (parts.local) {
      // 只剩免责声明（写给模型的 caveat）的那条：对人零信息量，整条不渲染。
      if (!parts.commands.length && !parts.stdout.length && !parts.text) return null;
      return (
        <article className="chat-message is-user is-command">
          {parts.commands.map((command, index) => (
            <span className="chat-command" key={`${command.name}-${index}`}>
              <span className="chat-command-icon" aria-hidden="true">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M4 17l6-6-6-6M12 19h8" /></svg>
              </span>
              <span className="chat-command-name">{command.name || t.chat.localCommand}</span>
              {command.args && <span className="chat-command-args">{command.args}</span>}
            </span>
          ))}
          {parts.text && <div className="chat-text">{parts.text}</div>}
          {/* 命令输出默认收起：/clear 之类没什么可看，/cost、/status 却能刷一屏。 */}
          {parts.stdout.map((out, index) => (
            <details className="chat-tool chat-command-out" key={index}>
              <summary>
                <span className="chat-tool-name">{t.chat.commandOutput}</span>
                <span className="chat-tool-summary">{out}</span>
                <span className="chat-tool-chevron">›</span>
              </summary>
              <pre>{out}</pre>
            </details>
          ))}
        </article>
      );
    }
    // 用户消息保持原文（用户不是在写 markdown，行首 # 变大标题只会失真）。
    return <article className="chat-message is-user"><div className="chat-text">{parts.text}</div></article>;
  }
  if (item.type === "assistant_text" || item.type === "assistant_delta") {
    return (
      <article className="chat-message is-assistant">
        <div className="chat-text chat-md"><ChatMarkdown text={item.text} /></div>
      </article>
    );
  }
  if (item.type === "reasoning" || item.type === "reasoning_delta") {
    // 长推理默认收成**预览**（显示开头几行并渐隐），而不是整段藏起来——既能一眼看到
    // agent 在想什么，又不会让上百行把结论和后续对话挤出屏幕。短的直接摊开，
    // 没必要为几行内容加一次点击。
    const lines = item.text.split("\n").filter((line) => line.trim()).length;
    const long = lines > REASONING_PREVIEW_LINES;
    return (
      <details className={"chat-reasoning" + (long ? " is-long" : "")} open={!long}>
        <summary>
          <span className="chat-timeline-dot" />
          {t.chat.reasoning}
          {long && <span className="chat-reasoning-size">{t.chat.reasoningLines(lines)}</span>}
        </summary>
        <div className="chat-md"><ChatMarkdown text={item.text} /></div>
      </details>
    );
  }
  if (item.type === "tool_use") {
    return (
      <details className="chat-tool">
        <summary>
          <span className="chat-tool-icon"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M4 17l6-6-6-6M12 19h8" /></svg></span>
          <span className="chat-tool-name">{item.name}</span><span className="chat-tool-summary">{item.summary}</span><span className="chat-tool-chevron">›</span>
        </summary>
        <pre>{item.summary}</pre>
      </details>
    );
  }
  if (item.type === "tool_result") {
    return (
      <details className={"chat-tool chat-result" + (item.is_error ? " is-error" : "")}>
        <summary>
          <span className="chat-tool-icon is-file"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M6 3h8l4 4v14H6zM14 3v5h5" /></svg></span>
          <span className="chat-tool-name">{t.chat.toolResult}</span><span className="chat-tool-summary">{item.text}</span><span className="chat-tool-chevron">›</span>
        </summary>
        <pre>{item.text}</pre>
      </details>
    );
  }
  return <div className="chat-meta"><span />{t.chat.compact}<span /></div>;
}
