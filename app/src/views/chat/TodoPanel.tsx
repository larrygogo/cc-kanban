import { useT } from "../../i18n";
import { TodoBadge } from "./TodoBadge";

type TodoRow = { content: string; status: string; stale?: boolean };

function TodoListItems({ todos }: { todos: TodoRow[] }) {
  return (
    <ul className="chat-todos-list">
      {todos.map((todo, index) => (
        <li key={`${index}:${todo.content}`} className={"is-" + todo.status}>
          <TodoBadge status={todo.status} small />
          <span className="chat-todo-text">{todo.content}</span>
        </li>
      ))}
    </ul>
  );
}

/// Agent 的待办清单。默认展开当前正在做的那条附近，完成项划掉——与各家 TUI 的呈现一致。
/// 长清单收进 details，避免把对话推走。状态图标与标题栏进度面板同一套徽章(TodoBadge)。
///
/// stale 行是「上一任务」的残留（用户已开新回合、新任务的清单还没写出来）：不计入
/// 头部进度，收进默认折叠的弱化区——它们不是当前任务的进度。全是残留时整张卡弱化成
/// 一行摘要（默认折叠），不再占一大块误导「当前在做这些」。
export function TodoPanel({ todos }: { todos: TodoRow[] }) {
  const t = useT();
  const fresh = todos.filter((todo) => !todo.stale);
  const staleTodos = todos.filter((todo) => todo.stale);
  const staleDone = staleTodos.filter((todo) => todo.status === "completed").length;

  // 全是上一任务的残留：弱化为一行摘要，展开可回看。
  if (fresh.length === 0) {
    return (
      <details className="chat-todos is-stale-only">
        <summary>
          <span className="chat-todos-title">{t.chat.todoPrevTask(staleDone, staleTodos.length)}</span>
          <span className="chat-tool-chevron">›</span>
        </summary>
        <TodoListItems todos={staleTodos} />
      </details>
    );
  }

  const done = fresh.filter((todo) => todo.status === "completed").length;
  const current = fresh.find((todo) => todo.status === "in_progress");
  return (
    <details className="chat-todos" open>
      <summary>
        <span className="chat-todos-title">{t.chat.todos}</span>
        <span className="chat-todos-count">{t.chat.todoProgress(done, fresh.length)}</span>
        {/* 折叠时也要能看出「此刻在做什么」，否则收起来就等于没有。 */}
        {current && <span className="chat-todos-current">{current.content}</span>}
        <span className="chat-tool-chevron">›</span>
      </summary>
      <TodoListItems todos={fresh} />
      {staleTodos.length > 0 && (
        /* 上一任务的残留：默认折叠的弱化区,不与当前清单混排(混排会把旧账算进进度)。 */
        <details className="chat-todos-stale">
          <summary>
            <span className="chat-todos-stale-title">{t.chat.todoPrevTask(staleDone, staleTodos.length)}</span>
            <span className="chat-tool-chevron">›</span>
          </summary>
          <TodoListItems todos={staleTodos} />
        </details>
      )}
    </details>
  );
}
