import { useEffect, useState } from "react";
import type { StructuredQuestion } from "./askUserQuestion";

/** AskUserQuestion 的题面（从 ChatWindow.tsx 原样搬出，行为不变）。多问题用 tab 切换
 *  ——全部竖排会把卡片堆得比对话区还高，把输入框挤出可视区（用户实拍反馈）；单问题
 *  不渲染 tab 条。可点选排队只存在于单问题形态（约束见调用处注释），多问题恒为纯展示，
 *  tab 不与点选状态相互作用。 */
export function QuestionPanels({ items, interactive, queuedAnswer, onToggle }: {
  items: StructuredQuestion[];
  interactive: boolean;
  queuedAnswer: string | null;
  onToggle: (label: string) => void;
}) {
  const [active, setActive] = useState(0);
  // 新一轮提问（题面数组换了）回到第一题。
  useEffect(() => { setActive(0); }, [items]);
  const index = Math.min(active, items.length - 1);
  const item = items[index];
  if (!item) return null;
  return (
    <>
      {items.length > 1 && (
        <div className="chat-question-tabs" role="tablist">
          {items.map((question, tabIndex) => (
            <button
              key={tabIndex}
              type="button"
              role="tab"
              aria-selected={tabIndex === index}
              className={tabIndex === index ? "is-active" : ""}
              onClick={() => setActive(tabIndex)}
            >{question.header || `#${tabIndex + 1}`}</button>
          ))}
        </div>
      )}
      <div className="chat-question-panel">
        {item.question && <span className="chat-approval-prewrap">{item.header ? `${item.header} · ${item.question}` : item.question}</span>}
        {interactive ? (
          <div className="chat-approval-options">
            {item.options.map((option, optionIndex) => (
              <button
                type="button"
                className={queuedAnswer === option.label ? "is-selected" : ""}
                key={`${optionIndex}:${option.label}`}
                onClick={() => onToggle(option.label)}
              >
                <span><b>{option.label}</b>{option.description && <small>{option.description}</small>}</span>
              </button>
            ))}
          </div>
        ) : (
          <div className="chat-approval-options is-static">
            {item.options.map((option, optionIndex) => (
              <div className="chat-approval-option-static" key={`${optionIndex}:${option.label}`}>
                <span><b>{option.label}</b>{option.description && <small>{option.description}</small>}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
