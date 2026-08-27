import { memo, useCallback, useEffect, useRef, useState } from "react";
import { getSubagentTranscript, type ChatItem, type SubagentRun } from "../../api";
import { useT } from "../../i18n";
import { formatBackendError } from "../../i18n/errors";
import { reduceChatEvents } from "../../chat/reducer";
import { TodoBadge } from "./TodoBadge";
import { Transcript } from "./Transcript";
import { durationText, type ToolUseItem } from "./shared";

/// 一次子任务委派。子任务的过程不在主 transcript 里（住在 provider 的侧车流），
/// 故这里只在**用户展开时**才去取——一个会话可能派出几十个子任务，跟着历史轮询一起读
/// 毫无必要。取回后缓存在组件里，折叠再展开不会重复请求。
function statusText(status: string, t: ReturnType<typeof useT>): string {
  if (status === "completed") return t.chat.subagentCompleted;
  if (status === "failed") return t.chat.subagentFailed;
  return t.chat.subagentRunning;
}

/// 状态徽标。图标与标题栏进度面板同一套徽章(TodoBadge,用户指定):进行中是旋转的
/// 绿色缺口圆环——静态文字看不出「还在动」，而这正是用户盯着它的原因。
function StatusBadge({ tone, text }: { tone: string; text: string }) {
  return (
    <span className={"chat-subagent-status is-" + tone}>
      <TodoBadge status={tone === "running" ? "in_progress" : tone} small />
      {text}
    </span>
  );
}

/// 子任务时间线的容器：限高 + 内部滚动。子任务动辄几十上百条，直接铺开会把主对话
/// 挤到几屏之外，用户想收起时还得一路往回滚。
function SubagentTimeline({ sessionId, items }: { sessionId: number; items: ChatItem[] }) {
  const t = useT();
  if (items.length === 0) return <div className="chat-subagent-hint">{t.chat.subagentEmpty}</div>;
  return (
    <div className="chat-subagent-scroll">
      <Transcript sessionId={sessionId} items={items} />
    </div>
  );
}

/// 结局统计 → 徽标。多个分支时报出数量，单个只说状态。
function outcomeBadge(
  outcome: { running: number; completed: number; failed: number },
  t: ReturnType<typeof useT>,
): { tone: string; text: string } | null {
  const known = outcome.running + outcome.completed + outcome.failed;
  if (known === 0) return null;
  const one = (tone: string, single: string, many: (n: number) => string) =>
    ({ tone, text: known > 1 ? many(known) : single });
  if (outcome.failed === known) return one("failed", t.chat.subagentFailed, t.chat.subagentTallyFailed);
  if (outcome.completed === known) return one("completed", t.chat.subagentCompleted, t.chat.subagentTallyCompleted);
  if (outcome.running === known) return one("running", t.chat.subagentRunning, t.chat.subagentTallyRunning);
  const parts = [
    outcome.completed && t.chat.subagentTallyCompleted(outcome.completed),
    outcome.failed && t.chat.subagentTallyFailed(outcome.failed),
    outcome.running && t.chat.subagentTallyRunning(outcome.running),
  ].filter(Boolean);
  return { tone: outcome.running ? "running" : "failed", text: parts.join(" · ") };
}

/// memo 理由同 Message；它还持有局部状态与展开时的 3s 轮询，白重渲染的代价更高。
/// outcome 是 tool_result 条目上的对象，条目不变则引用不变，浅比较可命中。
export const SubagentBlock = memo(function SubagentBlock({ sessionId, item, outcome, settled, finishedAt }: {
  sessionId: number;
  item: ToolUseItem;
  /// 主链回执带来的结局统计——**不必展开**就有，展开才拉的是时间线本身。
  outcome?: { running: number; completed: number; failed: number } | null;
  /// 主链上是否已有这次委派的回执。没有 = 还没回来 = 在跑。
  settled?: boolean;
  /// 最终回执的时间戳(TaskOutput 拉取的结局已按 task_id 归回)。与委派时刻相减得
  /// 执行时长;在跑时不用它,用滴答的当前时刻算已耗时。
  finishedAt?: string | null;
}) {
  const t = useT();
  const [runs, setRuns] = useState<SubagentRun[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  // 卸载后晚到的拉取结果不得再 setState（同文件 3s 轮询路径用局部 cancelled；
  // onToggle 触发的首次拉取在 effect 之外，只能用 ref）。
  const mountedRef = useRef(true);
  useEffect(() => () => { mountedRef.current = false; }, []);
  const subagent = item.subagent;
  // 侧车流和主 transcript 一样是逐条事件（kimi 更是 chunk 级增量），必须走同一套归一化——
  // 直接渲染原始事件会把一句话散成几十个碎片气泡。
  const fetchRuns = useCallback(() => getSubagentTranscript(sessionId, item.id)
    .then((fetched) => fetched.map((run) => ({ ...run, items: reduceChatEvents([], run.items, true) }))),
    [sessionId, item.id]);
  const load = () => {
    if (runs || loading) return;
    setLoading(true);
    setError("");
    // 展开拉取由 onToggle 触发（不在 effect 里），卸载守卫用 mountedRef 而非局部 cancelled。
    fetchRuns()
      .then((fetched) => { if (mountedRef.current) setRuns(fetched); })
      .catch((e) => { if (mountedRef.current) setError(formatBackendError(e, t.locale)); })
      .finally(() => { if (mountedRef.current) setLoading(false); });
  };
  // 展开着且还有分支在跑时定期重取：子任务边跑边写，静态快照会一直停在打开那一刻。
  // 收起或全部结束就停——不给已完结的子任务留一个永动的轮询。
  const hasRunning = runs?.some((run) => run.status === "running") ?? false;
  useEffect(() => {
    if (!open || !hasRunning) return;
    let cancelled = false;
    const timer = window.setInterval(() => {
      void fetchRuns().then((fresh) => { if (!cancelled) setRuns(fresh); }).catch(() => {});
    }, 3_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [open, hasRunning, fetchRuns]);
  const count = subagent?.count ?? 1;
  // 展开取过时用逐分支的实时状态；没展开则用主链回执带来的统计（这正是「不展开也能看状态」）。
  const summary = (() => {
    if (runs?.length) {
      const tally = { completed: 0, failed: 0, running: 0 };
      for (const run of runs) {
        if (run.status === "completed" || run.status === "failed" || run.status === "running") {
          tally[run.status] += 1;
        }
      }
      const badge = outcomeBadge(tally, t);
      if (badge) return badge;
    }
    if (outcome) return outcomeBadge(outcome, t);
    // 回执还没写进主链 = 这批子任务派出去了还没回来。一批 fan-out 的结局要等整批结束
    // 才落盘，若只认回执，正在跑的那段时间反而什么都不显示——那恰是最该显示的时候。
    if (!settled) return outcomeBadge({ running: count, completed: 0, failed: 0 }, t);
    return null;
  })();
  const isRunning = summary?.tone === "running";
  const startMs = item.timestamp ? Date.parse(item.timestamp) : NaN;
  // 在跑时的已耗时滴答:定时器只为触发重渲染去取新的 Date.now(),结束/卸载即停,
  // 不给已完结的子任务留常驻定时器;没有委派时间戳时算不出时长,同样不起。
  const ticking = isRunning && Number.isFinite(startMs);
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!ticking) return;
    const timer = window.setInterval(() => setTick((n) => n + 1), 1_000);
    return () => window.clearInterval(timer);
  }, [ticking]);
  const endMs = isRunning ? Date.now() : finishedAt ? Date.parse(finishedAt) : NaN;
  const duration = Number.isFinite(startMs) && Number.isFinite(endMs) && endMs >= startMs
    ? durationText(endMs - startMs, t)
    : null;
  return (
    <details className="chat-subagent" onToggle={(event) => {
      setOpen(event.currentTarget.open);
      if (event.currentTarget.open) load();
    }}>
      <summary>
        {/* 在跑时图标本体脉冲:折叠状态下扫一眼就能看出「里面有活儿」,不必凑近读徽标小字。 */}
        <span className={"chat-subagent-icon" + (isRunning ? " is-running" : "")}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="5" r="2.4" /><circle cx="5.5" cy="18.5" r="2.4" /><circle cx="18.5" cy="18.5" r="2.4" />
            <path d="M12 7.4v3.2M12 10.6H5.5v5.5M12 10.6h6.5v5.5" />
          </svg>
        </span>
        <span className="chat-subagent-label">{count > 1 ? t.chat.subagentBatch(count) : t.chat.subagent}</span>
        <span className="chat-subagent-desc">{subagent?.description || item.summary}</span>
        {/* 汇总状态要展开取过一次才有（状态与时间线同源，都在侧车流里）。 */}
        {summary && <StatusBadge tone={summary.tone} text={summary.text} />}
        {/* 执行时长:在跑=已耗时(逐秒),结束=委派到最终回执的用时。 */}
        {duration && <span className="chat-subagent-time">{duration}</span>}
        {subagent?.agent_type && <span className="chat-subagent-type">{subagent.agent_type}</span>}
        <span className="chat-tool-chevron">›</span>
      </summary>
      <div className="chat-subagent-body">
        {loading && <div className="chat-subagent-hint">{t.chat.subagentLoading}</div>}
        {error && <div className="chat-subagent-hint is-error" role="alert">{error}</div>}
        {runs && (runs.length === 0
          ? <div className="chat-subagent-hint">{t.chat.subagentEmpty}</div>
          // 单个分支直接铺开；一次派一批（swarm 可达十几个）则每个分支各自折叠，
          // 否则一次展开会把十几条完整时间线全部倒进页面。
          : runs.length === 1
          ? <SubagentTimeline sessionId={sessionId} items={runs[0].items} />
          : runs.map((run, index) => (
            <details className="chat-subagent-branch" key={run.label ?? index}>
              <summary>
                <span className="chat-subagent-branch-label">{run.label ?? `#${index + 1}`}</span>
                {run.status && <StatusBadge tone={run.status} text={statusText(run.status, t)} />}
                <span className="chat-subagent-branch-count">{t.chat.subagentSteps(run.items.length)}</span>
                <span className="chat-tool-chevron">›</span>
              </summary>
              <SubagentTimeline sessionId={sessionId} items={run.items} />
            </details>
          )))}
      </div>
    </details>
  );
});
