import { useEffect, useState } from "react";
import { useT } from "../../i18n";
import type { QuestionAnswerDraft, StructuredQuestion } from "./askUserQuestion";

/** AskUserQuestion 的题面。多问题用 tab 切换——全部竖排会把卡片堆得比对话区还高，
 *  把输入框挤出可视区（用户实拍反馈）；单问题不渲染 tab 条。
 *
 *  两种形态：
 *  - display：纯展示或点选排队（旧路径，屏幕识别落键）。可点选排队只存在于单问题
 *    单选（约束见调用处注释），多问题恒为纯展示，tab 不与点选状态相互作用。
 *  - answer：真正的作答面（broker 挂起代答）。每题独立草稿（单选点即换、多选勾选、
 *    自定义输入），tab 上带已答 ✓，提交按钮在卡片层。 */
export type QuestionPanelsProps = { items: StructuredQuestion[] } & (
  | {
      mode?: "display";
      interactive: boolean;
      queuedAnswer: string | null;
      onToggle: (label: string) => void;
    }
  | {
      mode: "answer";
      answers: ReadonlyMap<number, QuestionAnswerDraft>;
      onSelect: (questionIndex: number, label: string) => void;
      onCustom: (questionIndex: number, text: string) => void;
    }
);

const EMPTY_DRAFT: QuestionAnswerDraft = { selected: [], custom: "" };

export function QuestionPanels(props: QuestionPanelsProps) {
  const t = useT();
  const { items } = props;
  const [active, setActive] = useState(0);
  // 新一轮提问（题面数组换了）回到第一题。
  useEffect(() => { setActive(0); }, [items]);
  const index = Math.min(active, items.length - 1);
  const item = items[index];
  if (!item) return null;
  const answered = (question: number) => {
    if (props.mode !== "answer") return false;
    const draft = props.answers.get(question) ?? EMPTY_DRAFT;
    return draft.selected.length > 0 || draft.custom.trim().length > 0;
  };
  const draft = props.mode === "answer" ? (props.answers.get(index) ?? EMPTY_DRAFT) : EMPTY_DRAFT;
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
            >
              {answered(tabIndex) && <span aria-label={t.chat.questionAnswered}>✓ </span>}
              {question.header || `#${tabIndex + 1}`}
            </button>
          ))}
        </div>
      )}
      <div className="chat-question-panel">
        {item.question && <span className="chat-approval-prewrap">{item.header ? `${item.header} · ${item.question}` : item.question}</span>}
        {props.mode === "answer" ? (
          <>
            <div className="chat-approval-options">
              {item.options.map((option, optionIndex) => {
                const selected = draft.selected.includes(option.label);
                return (
                  <button
                    type="button"
                    className={selected ? "is-selected" : ""}
                    key={`${optionIndex}:${option.label}`}
                    onClick={() => props.onSelect(index, option.label)}
                  >
                    <i aria-hidden="true">{selected ? "✓" : ""}</i>
                    <span><b>{option.label}</b>{option.description && <small>{option.description}</small>}</span>
                  </button>
                );
              })}
            </div>
            <div className="chat-approval-custom">
              <input
                value={draft.custom}
                onChange={(event) => props.onCustom(index, event.target.value)}
                placeholder={t.chat.customAnswerPlaceholder}
              />
            </div>
          </>
        ) : props.interactive ? (
          <div className="chat-approval-options">
            {item.options.map((option, optionIndex) => (
              <button
                type="button"
                className={props.queuedAnswer === option.label ? "is-selected" : ""}
                key={`${optionIndex}:${option.label}`}
                onClick={() => props.onToggle(option.label)}
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
