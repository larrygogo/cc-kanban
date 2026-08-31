/**
 * broker 审批与 AskUserQuestion 题面的接收通道（从 ChatWindow.tsx 原样抽出，行为不变）：
 * 消费者租约注册、push 事件监听（pending-approval / -cleared / interactive-question）、
 * pendingInteraction 合并轮询兜底、跨会话切换的题面领养、侧栏授权徽标集合、审批倒计时。
 * 作答/落键（decideApproval、queuedAnswer、dismiss 记账）仍在 ChatWindow——它们与
 * 终端写入和屏幕识别交织，是「回应」不是「接收」。
 */
import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import { listen } from "@tauri-apps/api/event";
import {
  awaitingInteractionSessions,
  pendingInteraction,
  registerApprovalConsumer,
  unregisterApprovalConsumer,
  type PendingApproval,
} from "../../api";
import { useT } from "../../i18n";
import { remoteUi } from "../../remoteMode";

/** 远程审批租约续约周期（ms）。须显著小于后端 REMOTE_CONSUMER_TTL_MS(60s),留足网络抖动余量。 */
export const REMOTE_CONSUMER_HEARTBEAT_MS = 20_000;

export function useApprovalChannel({ sessionId, activeSessionRef, viewRef, setView, setSendError }: {
  sessionId: number;
  /** 当前会话 id 的实时镜像（mount-once 监听里的闭包 state 是过期值）。 */
  activeSessionRef: MutableRefObject<number>;
  viewRef: MutableRefObject<"chat" | "terminal">;
  setView: Dispatch<SetStateAction<"chat" | "terminal">>;
  setSendError: Dispatch<SetStateAction<string>>;
}) {
  const t = useT();
  const [approval, setApproval] = useState<PendingApproval | null>(null);
  // 实时镜像：pending-approval-cleared 的 mount-once 监听里要判「卡是否还挂着」，
  // 闭包 state 是过期值。tRef 同理（提示文案随界面语言）。
  const approvalLiveRef = useRef<PendingApproval | null>(null);
  approvalLiveRef.current = approval;
  const tRef = useRef(t);
  tRef.current = t;
  // broker 审批的剩余时间。用户在终端作答的场景不靠它——后端监视断连与 pending_review
  // 清位（pty.rs 的 await_approval_outcome），结算后 pending-approval-cleared 实时收卡；
  // 倒计时只覆盖「两边都没人答」的兜底：300s 超时此前只写在注释里，卡片到点凭空消失像 bug。
  // 后端不下发 deadline，从本窗口首见该 requestId 起本地计时（晚开窗只会显得更宽裕，
  // 不会提前误报超时）；每秒 tick 一次驱动徽章上的倒计时。
  const APPROVAL_TIMEOUT_MS = 300_000;
  const approvalSeenRef = useRef<{ id: string; at: number } | null>(null);
  const [approvalNow, setApprovalNow] = useState(() => Date.now());
  useEffect(() => {
    if (!approval) {
      approvalSeenRef.current = null;
      return;
    }
    if (approvalSeenRef.current?.id !== approval.requestId) {
      approvalSeenRef.current = { id: approval.requestId, at: Date.now() };
      setApprovalNow(Date.now());
    }
    const timer = window.setInterval(() => setApprovalNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [approval]);
  const approvalCountdown = (() => {
    if (!approval || !approvalSeenRef.current) return null;
    const left = Math.max(0, APPROVAL_TIMEOUT_MS - (approvalNow - approvalSeenRef.current.at));
    const totalSeconds = Math.floor(left / 1000);
    return `${Math.floor(totalSeconds / 60)}:${String(totalSeconds % 60).padStart(2, "0")}`;
  })();
  // 归零后徽章切「已超时」态而不是定格 0:00：卡片还在是因为后端的回落结算事件
  // （pending-approval-cleared）尚未到达，定格的 0:00 读不出「正在回落终端处理」这层。
  const approvalTimedOut = !!approval && !!approvalSeenRef.current
    && approvalNow - approvalSeenRef.current.at >= APPROVAL_TIMEOUT_MS;
  // AskUserQuestion 的结构化题面（broker 自动放行后经 interactive-question 事件直达）。
  const [structuredQuestion, setStructuredQuestion] = useState<PendingApproval | null>(null);
  // 非当前会话的待授权请求：侧边栏亮徽标召唤用户自己过去。注意后端 **会** 为 broker 接管的
  // 审批把窗口切到目标会话（见 pty.rs 的 ensure_approval_window，不切的代价是请求 10s 后
  // 回落 TUI），所以这里是兜底：切换竞态期间、以及消费者已注册在别的会话时到达的请求。
  // 只进不出会越攒越多——clear 事件、切到该会话、请求落到当前会话三条路径都负责摘除。
  const [approvalAwaitingIds, setApprovalAwaitingIds] = useState<ReadonlySet<number>>(() => new Set());
  const [brokerOwnsReview, setBrokerOwnsReview] = useState(false);

  useEffect(() => {
    if (sessionId <= 0) return;
    const consumerId = `chat-${sessionId}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
    let disposed = false;
    let retryTimer = 0;
    // 注册失败不能就此放弃：没有租约，后端会把所有审批直接交还终端 TUI，
    // 而这扇窗看起来一切正常（轮询永远拿不到 pending）——用户以为在 GUI 等审批，
    // 实际审批卡在终端里没人看。effect 不会重跑，这里自己做有限退避重试。
    const register = (attempt: number) => {
      void registerApprovalConsumer(sessionId, consumerId).then(() => {
        // effect 可能在 IPC 返回前已经因切会话/关窗清理；再次注销闭合这个竞态窗口。
        if (disposed) void unregisterApprovalConsumer(consumerId).catch(() => {});
      }).catch((error) => {
        console.error("注册审批消费者失败", error);
        if (disposed || attempt >= 5) return;
        retryTimer = window.setTimeout(() => register(attempt + 1), 500 * 2 ** attempt);
      });
    };
    register(0);
    // 远程租约带 60s TTL(防手机锁屏/被杀留幽灵租约)。前端每 20s 重注册续约——
    // registerApprovalConsumer 幂等,重注册即刷新 seen_ms。桌面租约无 TTL,不进此分支。
    let heartbeat = 0;
    if (remoteUi()) {
      heartbeat = window.setInterval(() => {
        void registerApprovalConsumer(sessionId, consumerId).catch(() => {});
      }, REMOTE_CONSUMER_HEARTBEAT_MS);
    }
    return () => {
      disposed = true;
      window.clearTimeout(retryTimer);
      window.clearInterval(heartbeat);
      void unregisterApprovalConsumer(consumerId).catch(() => {});
    };
  }, [sessionId]);

  // push 通道健康证明：本窗口收到过 pending-approval / interactive-question 系事件
  // ⇒ emit 能到达本窗口，轮询只需低频保险。ref 而非 state：翻转不需要重渲染，
  // 下一拍轮询自然读到新值。
  const pushProvenRef = useRef(false);
  // 审批 + 题面的轮询兜底，一次 IPC 取两样（此前是两条 400ms 的独立轮询）。push 事件
  // 是主路径：通道一旦证明可用，轮询退避到 2.5s 只做保险——空闲对话窗的 IPC 频次从
  // ~5 次/秒降到 ~1 次/秒。事件从未到达过（冷启动 1~2s 窗口、通道故障）时保持 400ms，
  // 错过的卡片补回延迟与从前一致。
  useEffect(() => {
    if (sessionId <= 0) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = () => pendingInteraction(sessionId).then(({ approval: nextApproval, question }) => {
      if (cancelled) return;
      if (nextApproval) setBrokerOwnsReview(true);
      setApproval(nextApproval);
      // 已有同一 requestId 的题面卡时不重复设置，避免打断点选状态；answerable 翻转
      // （挂起结算/超时 → 作答卡降级为展示卡）必须放行——它正是靠这条轮询传递的。
      if (question) {
        setStructuredQuestion((current) => {
          if (
            current?.requestId === question.requestId
            && current.answerable === question.answerable
          ) return current;
          setBrokerOwnsReview(true);
          return question;
        });
      }
    }).catch(() => {});
    const tick = () => {
      void poll().then(() => {
        if (cancelled) return;
        timer = window.setTimeout(tick, pushProvenRef.current ? 2_500 : 400);
      });
    };
    tick();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [sessionId]);

  // 远程徽标扫描:pending-approval 系 push 事件到不了浏览器,非当前会话的审批/题面
  // 若不扫,侧栏永不亮徽标——远程挂多会话盯批正是主场景。3s 与看板刷新同量级。
  // 与桌面「进过会话即撤徽标」的礼貌不同,扫描以 broker 实时压着的请求为准:请求
  // 未决就一直亮(正在看的会话卡片本身已在场,徽标略冗余但如实)。
  useEffect(() => {
    if (!remoteUi()) return;
    let cancelled = false;
    const sweep = () => {
      void awaitingInteractionSessions().then((raw) => {
        if (cancelled) return;
        // 防非数组:旧后端没有此命令时桥可能回 null/undefined,不能炸掉整个组件树。
        const ids = Array.isArray(raw) ? raw : [];
        setApprovalAwaitingIds((prev) => {
          if (prev.size === ids.length && ids.every((id) => prev.has(id))) return prev;
          return new Set(ids);
        });
      }).catch(() => {});
    };
    sweep();
    const timer = window.setInterval(sweep, 3_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, []);

  useEffect(() => {
    let unApproval: (() => void) | undefined;
    let unCleared: (() => void) | undefined;
    let cancelled = false;
    listen<PendingApproval>("pending-approval", (event) => {
      pushProvenRef.current = true;
      if (event.payload.sessionId === activeSessionRef.current) {
        setApprovalAwaitingIds((prev) => {
          if (!prev.has(event.payload.sessionId)) return prev;
          const next = new Set(prev);
          next.delete(event.payload.sessionId);
          return next;
        });
        setBrokerOwnsReview(true);
        setApproval(event.payload);
        // 用户正在终端视图里操作时别把界面切回对话（焦点在 xterm，切换会打断输入）；
        // 不在终端才拉回来——审批卡在对话页，没人看就等于卡住。
        if (viewRef.current !== "terminal") setView("chat");
      } else {
        // 别的会话要授权：不切窗不抢焦点（后端只闪任务栏），侧边栏亮徽标等用户自己过去。
        setApprovalAwaitingIds((prev) => {
          if (prev.has(event.payload.sessionId)) return prev;
          const next = new Set(prev);
          next.add(event.payload.sessionId);
          return next;
        });
      }
    }).then((fn) => { if (cancelled) fn(); else unApproval = fn; }).catch(() => {});
    listen<PendingApproval>("pending-approval-cleared", (event) => {
      pushProvenRef.current = true;
      setApprovalAwaitingIds((prev) => {
        if (!prev.has(event.payload.sessionId)) return prev;
        const next = new Set(prev);
        next.delete(event.payload.sessionId);
        return next;
      });
      if (event.payload.sessionId === activeSessionRef.current) {
        // 卡还挂着时被 cleared = 在终端答掉 / 300s 超时回落——不是本窗口的 decideApproval
        // （那条路先行 setApproval(null)，这里读到的已是 null）。凭空消失分不清是自己漏点
        // 还是超时（与题面卡 questionExpired 同一课），说清卡去哪了。
        if (approvalLiveRef.current?.requestId === event.payload.requestId) {
          setSendError(tRef.current.chat.approvalClearedNotice);
        }
        setApproval((current) => current?.requestId === event.payload.requestId ? null : current);
      }
    }).then((fn) => { if (cancelled) fn(); else unCleared = fn; }).catch(() => {});
    return () => { cancelled = true; unApproval?.(); unCleared?.(); };
    // eslint 语义上 setView/setSendError 是稳定 setter、ref 是稳定引用,mount-once 成立。
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // AskUserQuestion 题面直达。事件与 chat-session-changed 存在竞态（后端先切窗再发题面，
  // 前端两个事件到达顺序不保证）：无论当下会话是否匹配都先暂存进 ref，会话切换完成的
  // effect 里再领养，两种到达顺序都不丢卡。
  //
  // 领养窗口只有 3 秒且**收卡时立即清空**：这个 ref 存在的唯一理由是覆盖毫秒级的事件
  // 顺序竞态。窗口留长了就成了幽灵卡的来源——答完收卡后切走再切回来，effect 会把
  // 早已了结的问题重新弹出来（实测踩过）。
  const lastInteractiveQuestionRef = useRef<{ payload: PendingApproval; at: number } | null>(null);
  useEffect(() => {
    let un: (() => void) | undefined;
    let cancelled = false;
    listen<PendingApproval>("interactive-question", (event) => {
      pushProvenRef.current = true;
      lastInteractiveQuestionRef.current = { payload: event.payload, at: Date.now() };
      if (event.payload.sessionId === activeSessionRef.current) {
        // 题面卡就是处理面：抑制 pendingReview 的「去终端处理」降级卡（与 broker 审批同理）。
        setBrokerOwnsReview(true);
        setStructuredQuestion(event.payload);
        // 与审批一致：用户正在终端视图操作时不抢（表单本来就在那里），否则拉回对话页。
        if (viewRef.current !== "terminal") setView("chat");
      } else {
        // 别的会话在提问：与审批同策略（后端不切窗只闪任务栏），侧栏亮徽标等用户自己
        // 过去；过去后题面靠 pendingInteraction 轮询从后端表里取回。
        setApprovalAwaitingIds((prev) => {
          if (prev.has(event.payload.sessionId)) return prev;
          const next = new Set(prev);
          next.add(event.payload.sessionId);
          return next;
        });
      }
    }).then((fn) => { if (cancelled) fn(); else un = fn; }).catch(() => {});
    return () => { cancelled = true; un?.(); };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  // 会话切换：清掉旧会话的题面卡；竞态暂存的题面若属于新会话（事件先于切换抵达）则领养。
  useEffect(() => {
    // 徽标的使命是「把人带过来」，人到了就退场——审批经 pending-approval-cleared 也会清，
    // 但轮询取回（事件错过）与题面（无 cleared 事件）两条路都只能靠这里收尾。
    setApprovalAwaitingIds((prev) => {
      if (!prev.has(sessionId)) return prev;
      const next = new Set(prev);
      next.delete(sessionId);
      return next;
    });
    const last = lastInteractiveQuestionRef.current;
    if (last && last.payload.sessionId === sessionId && Date.now() - last.at < 3_000) {
      setBrokerOwnsReview(true);
      setStructuredQuestion(last.payload);
    } else {
      setStructuredQuestion(null);
    }
  }, [sessionId]);

  return {
    approval, setApproval, approvalCountdown, approvalTimedOut,
    structuredQuestion, setStructuredQuestion,
    approvalAwaitingIds, setApprovalAwaitingIds,
    brokerOwnsReview, setBrokerOwnsReview,
    // 交出去给消解逻辑清空:卡了结时必须连领养暂存一起清,否则切走再切回弹幽灵卡。
    lastInteractiveQuestionRef,
  };
}
