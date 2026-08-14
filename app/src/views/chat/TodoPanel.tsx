import { useT } from "../../i18n";
import { TodoBadge } from "./TodoBadge";

/// Agent 的待办清单。默认展开当前正在做的那条附近，完成项划掉——与各家 TUI 的呈现一致。
/// 长清单收进 details，避免把对话推走。状态图标与标题栏进度面板同一套徽章(TodoBadge)。
export function TodoPanel({ todos }: { todos: { content: string; status: string }[] }) {
  const t = useT();
  const done = todos.filter((todo) => todo.status === "completed").length;
  const current = todos.find((todo) => todo.status === "in_progress");
  return (
    <details className="chat-todos" open>
      <summary>
        <span className="chat-todos-title">{t.chat.todos}</span>
        <span className="chat-todos-count">{t.chat.todoProgress(done, todos.length)}</span>
        {/* 折叠时也要能看出「此刻在做什么」，否则收起来就等于没有。 */}
        {current && <span className="chat-todos-current">{current.content}</span>}
        <span className="chat-tool-chevron">›</span>
      </summary>
      <ul className="chat-todos-list">
        {todos.map((todo, index) => (
          <li key={`${index}:${todo.content}`} className={"is-" + todo.status}>
            <TodoBadge status={todo.status} small />
            <span className="chat-todo-text">{todo.content}</span>
          </li>
        ))}
      </ul>
    </details>
  );
}
