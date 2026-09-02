import { useEffect, useId, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useT } from "../../i18n";
import type { QuestionAnswerDraft, StructuredQuestion } from "./askUserQuestion";

/** AskUserQuestion 的题面。多问题用 tab 切换——全部竖排会把卡片堆得比对话区还高，
 *  把输入框挤出可视区（用户实拍反馈）；单问题不渲染 tab 条。
 *
 *  两种形态：
 *  - display：纯展示或点选排队（旧路径，屏幕识别落键）。点选按问题 keyed 排队
 *    （queuedAnswers 的键是问题下标）：落键时只写**屏幕识别确认在屏**的那题
 *    （聚焦题判定见 matchFocusedQuestion），其余题的排队留着等轮到它。
 *  - answer：真正的作答面（broker 挂起代答）。每题独立草稿（单选点即换、多选勾选、
 *    自定义输入），tab 上带已答 ✓，提交按钮在卡片层。 */
export type QuestionPanelsProps = { items: StructuredQuestion[] } & (
  | {
      mode?: "display";
      interactive: boolean;
      queuedAnswers: ReadonlyMap<number, readonly string[]>;
      onToggle: (questionIndex: number, label: string) => void;
    }
  | {
      mode: "answer";
      answers: ReadonlyMap<number, QuestionAnswerDraft>;
      onSelect: (questionIndex: number, label: string) => void;
      onCustom: (questionIndex: number, text: string) => void;
      /// 自定义回答框里按 Enter 直接提交（7C-8）。调用方自带「答完了没」的守卫，
      /// 这里只负责把回车转过去。屏幕识别卡的同款输入框早就是回车即提交，
      /// 唯独这张作答卡的回车是空操作。
      onSubmit?: () => void;
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
  // tab 条的 roving tabindex：整组只占一个 Tab 停靠点，←/→/Home/End 换页签并自动激活
  // （与点击同语义——焦点到了内容没换，比不换焦点更迷惑）。id 前缀按实例隔离。
  const tabsRef = useRef<HTMLDivElement>(null);
  const idPrefix = useId();
  const onTabsKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const tabs = Array.from(tabsRef.current?.querySelectorAll<HTMLElement>('[role="tab"]') ?? []);
    if (tabs.length === 0) return;
    const at = tabs.indexOf(document.activeElement as HTMLElement);
    if (at < 0) return;
    const next = e.key === "ArrowRight" ? (at + 1) % tabs.length
      : e.key === "ArrowLeft" ? (at - 1 + tabs.length) % tabs.length
      : e.key === "Home" ? 0
      : e.key === "End" ? tabs.length - 1
      : null;
    if (next === null) return;
    e.preventDefault();
    setActive(next);
    tabs[next]?.focus();
  };
  if (!item) return null;
  const answered = (question: number) => {
    if (props.mode === "answer") {
      const draft = props.answers.get(question) ?? EMPTY_DRAFT;
      return draft.selected.length > 0 || draft.custom.trim().length > 0;
    }
    // display 形态的「已答」= 该题已有排队答案（未落键前也是用户的明确意图）。
    return (props.queuedAnswers.get(question)?.length ?? 0) > 0;
  };
  const draft = props.mode === "answer" ? (props.answers.get(index) ?? EMPTY_DRAFT) : EMPTY_DRAFT;
  return (
    <>
      {items.length > 1 && (
        <div className="chat-question-tabs" role="tablist" aria-label={t.chat.questionTitle} ref={tabsRef} onKeyDown={onTabsKeyDown}>
          {items.map((question, tabIndex) => (
            <button
              key={tabIndex}
              type="button"
              role="tab"
              id={`${idPrefix}-tab-${tabIndex}`}
              aria-selected={tabIndex === index}
              aria-controls={`${idPrefix}-panel`}
              tabIndex={tabIndex === index ? 0 : -1}
              className={tabIndex === index ? "is-active" : ""}
              onClick={() => setActive(tabIndex)}
            >
              {answered(tabIndex) && <span aria-label={t.chat.questionAnswered}>✓ </span>}
              {question.header || `#${tabIndex + 1}`}
            </button>
          ))}
        </div>
      )}
      <div
        className="chat-question-panel"
        // 只渲染当前题的面板：多问题时它是 tablist 的 tabpanel，单问题时这两个属性无害。
        {...(items.length > 1 ? { role: "tabpanel", id: `${idPrefix}-panel`, "aria-labelledby": `${idPrefix}-tab-${index}` } : {})}
      >
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
                // IME 合成中的 Enter 是选词，不是提交（同 composer 与新建面板的守卫）。
                onKeyDown={(event) => {
                  if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
                  event.preventDefault();
                  props.onSubmit?.();
                }}
              />
            </div>
          </>
        ) : props.interactive ? (
          <div className="chat-approval-options">
            {item.options.map((option, optionIndex) => (
              <button
                type="button"
                className={props.queuedAnswers.get(index)?.includes(option.label) ? "is-selected" : ""}
                key={`${optionIndex}:${option.label}`}
                onClick={() => props.onToggle(index, option.label)}
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
