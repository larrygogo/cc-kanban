import { invoke } from "@tauri-apps/api/core";
import { appConfirm } from "./confirm";
import type { ChatHistoryDto } from "./generated/contracts/ChatHistoryDto";
import type { ChatItem as GeneratedChatItem } from "./generated/contracts/ChatItem";
import type { SubagentRun as GeneratedSubagentRun } from "./generated/contracts/SubagentRun";
import type { ManagedTerminalSnapshotDto } from "./generated/contracts/ManagedTerminalSnapshotDto";
import type { PendingApprovalDto } from "./generated/contracts/PendingApprovalDto";
import type { PendingInteractionDto } from "./generated/contracts/PendingInteractionDto";
import type { LoginDoneEvent } from "./generated/contracts/LoginDoneEvent";

/**
 * agent 身份串（`"claude"` / `"kimi"` / …），与 Rust 侧 `meowo_agent::AgentId` 同值。
 *
 * 刻意**不是**联合类型：agent 名单由后端的 `list_agents()` 下发，前端不再维护自己的一份
 * （此前加一个 CLI 要同步 4 处：这个联合、PROVIDERS 表、测试里的 EXPECTED_KEYS、Rust 的枚举）。
 * DB 里还可能存着本版本尚不认识的 id，联合类型会对它撒谎。
 */
export type AgentId = string;

/** 后端下发的一个 agent。前端认识 agent 的唯一途径。 */
export type AgentDescriptor = {
  id: AgentId;
  /** 产品名，不翻译。 */
  display_name: string;
  /** 可执行是否装在本机。 */
  installed: boolean;
  /**
   * 能否被套上代理。为 false 时设置页**不给它代理行**——给一个读不到代理配置的 agent 显示
   * 输入框，等于请用户去配一个静默不生效的代理。
   */
  supports_proxy: boolean;
  /**
   * 代理能否写进该 agent 自己的配置文件——true 则不管由谁启动都生效，false 只覆盖
   * Meowo 拉起的会话。仅用于选覆盖面文案。
   */
  proxy_covers_all_launches: boolean;
  /** 是否支持 SOCKS 代理。false 时设置页当场拒绝 socks:// 串（否则静默连不上）。 */
  proxy_accepts_socks: boolean;
  /**
   * 有没有账号概念。为 false 时**不得显示登录态、也不得给出登录入口**——该 agent 的
   * `login_argv()` 是 None，按钮点下去只会报「拉起登录失败」。
   *
   * 不能靠「账号查不出来」来推断：那与「真的没登录」长得一模一样。
   */
  supports_account: boolean;
  /**
   * 能否有多个账号。false（gemini）→ 不给「添加账号」入口。
   *
   * 不能靠「账号列表只有一条」推断——那与「只建了默认账号」无法区分。
   */
  supports_profiles: boolean;
  /**
   * 能否用 API Key 登录（gemini：OAuth 被官方停用，key 是唯一活路，且 CLI 没有输入 key 的
   * 登录子命令，必须由 meowo 提供入口）。为 true 时未登录卡片额外给「填 API Key」输入。
   *
   * 老后端不下发此字段 → undefined，按「不支持」处理（不给一个后端接不住的入口）。
   */
  supports_api_key_login?: boolean;
  /**
   * meowo 能否显示该 agent 的上下文占用（贴纸百分比液柱）。false（gemini/opencode）时
   * 卡片显式标注「上下文占用：不支持」，不留空白让用户以为是 bug。
   *
   * 老后端不下发此字段 → undefined，按「支持」处理（不误标已有能力的 agent 为不支持）。
   */
  supports_context?: boolean;
  /**
   * 新建会话的启动选项（选择 → CLI flag 映射，由插件声明）。空/缺失 = 面板不给该 agent
   * 选项栏。前端只回传 choice id；翻译成命令行参数在后端按同一张声明表进行。
   */
  launch_options?: LaunchOption[];
  /** 插件未声明该能力时为 null，界面不得显示中转入口。 */
  relay?: RelayCapability | null;
};

/** 启动选项的一个可选值。label 是产品词；细文案由 i18n 按 `<option>.<choice>` 取，缺省回退 label。 */
export type LaunchChoice = { id: string; label: string; args: string[] };
/** 一栏启动选项（单选）。 */
export type LaunchOption = { id: string; choices: LaunchChoice[]; default: string };

/** 快速切模型的一个预设项。描述文案在前端 i18n（chat.modelDesc）按 id 取。 */
export type ModelPreset = { id: string; label: string };

/** 一条斜杠命令。builtin 的描述走前端 i18n；user/project 是从命令文件头里读出的。 */
export type SlashCommand = {
  name: string;
  description: string | null;
  source: "builtin" | "user" | "project";
};

export type ModeInput = { data: string; submit: boolean };
export type ModeOption = { value: string; inputs: ModeInput[] };
/** TUI 状态栏上代表某个模式值的稳定文案片段；cycle 盲切后靠它从屏幕即时回显落点。 */
export type ModeScreenMarker = { marker: string; value: string };
export type ModeControl = {
  dimension: string;
  cycle_input: string | null;
  options: ModeOption[];
  /** 空表 = 该维度无屏幕回显能力，显示只随 transcript 状态走。 */
  screen_markers: ModeScreenMarker[];
};

/**
 * 对话页能力，由**安装实况**组装：插件内置表 ∪ 用户/项目目录里发现的自定义命令 + CLI 版本。
 * 不随 `list_agents()` 静态下发——它依赖会话的 cwd（项目级命令）且随安装变化。
 */
export type ChatUi = {
  slash_commands: SlashCommand[];
  model_presets: ModelPreset[];
  /**
   * 打开「选模型」交互菜单的命令（预设为空时才有意义）。除 claude 外几家的 `/model`
   * 不接受内联参数，只能发出它再把 CLI 弹出的菜单渲染成按钮——清单由 CLI 现给。
   */
  model_menu_command?: string | null;
  /** Provider 声明的多维模式交互能力；当前值由 ChatHistory 的增量状态提供。 */
  mode_controls: ModeControl[];
  /** 裸发送会弹出交互界面的内置命令（含 model_menu_command，后端总装时已并入）。 */
  menu_slash_commands: string[];
  /** 启动时必须转到终端人工处理的提示文本片段（框架通用值 + provider 补充）。 */
  startup_attention_markers: string[];
  /** 数字选择器锚点(插件声明的识别文法):空 = 该 agent 的纯编号菜单不做卡片化。 */
  selector_anchors: { marker: string; kind: "input" | "chat" }[];
  /**
   * 整句可操作提示（审批框、长会话确认）的识别规格，由插件声明。空 = 该 agent 未取证，
   * 不做整句识别——误报会凭空弹卡并锁住输入框，比不弹更糟。
   *
   * 只含该会话自己那家的规则：识别层因此不必再按 provider 分支，也不会拿别家的
   * 界面原文去匹配本家屏幕（引用同一句话的普通输出不该弹卡）。
   */
  attention_patterns: { id: string; patterns: string[]; last: boolean }[];
  /** 中断当前回合的按键序列(如 Esc);null = 未取证,GUI 不提供强制插话入口。 */
  interrupt_input: string | null;
  /** runtime skill 清单尚未落盘；ChatWindow 应随 transcript 增量继续探测。 */
  runtime_commands_pending: boolean;
  /** 附件可用该 CLI 原生的 `@路径` 提及注入;false = 退回通用指令文本兜底。 */
  attachment_mention: boolean;
  /** TUI 支持 Ctrl-V 原生粘贴剪贴板图片时的 composer 占位符正则;null = 不支持。 */
  clipboard_image_paste: string | null;
  /** 探测到的已装 CLI 版本（`--version` 首行）；探测失败为 null。 */
  version: string | null;
};

export type RelayCapability = {
  protocols: { value: string; label: string }[];
  auth_modes: { value: string; label: string }[];
  default_protocol: string;
  default_auth: string;
  suggestions: { protocol: string; models: string[] }[];
  /** 可勾选的附加环境变量（如 Claude Code 的两个流量/归因开关）；插件未声明时缺省。 */
  env_options?: { id: string; label: string; env: [string, string] }[];
};

export type Todo = {
  id: number;
  task_id: number;
  content: string;
  status: "pending" | "in_progress" | "completed";
  order_idx: number;
};

export type Task = {
  id: number;
  project_id: number;
  session_id: number | null;
  title: string;
  column: "todo" | "doing" | "done";
  column_locked: boolean;
  current_activity: string | null;
  created_at: number;
  updated_at: number;
};

export type Project = {
  id: number;
  root_path: string;
  name: string;
  created_at: number;
  updated_at: number;
};

export type ProjectOverview = {
  project: Project;
  active_sessions: number;
  todo_count: number;
  doing_count: number;
  done_count: number;
  last_activity_at: number;
};

export type TaskCard = {
  task: Task;
  todos: Todo[];
  session_status: string | null;
};

export function getOverview(): Promise<ProjectOverview[]> {
  return invoke("get_overview");
}

export function getProjectTasks(projectId: number): Promise<TaskCard[]> {
  // JS 传 projectId，Tauri 自动转成 Rust 命令的 project_id 参数。
  return invoke("get_project_tasks", { projectId });
}

export type Session = {
  id: number;
  project_id: number;
  cc_session_id: string;
  status: "running" | "waiting" | "ended" | "stale";
  started_at: number;
  last_event_at: number;
  ended_at: number | null;
};

export type LiveSession = {
  session: Session;
  project_name: string;
  task_title: string;
  current_activity: string | null;
  column: "todo" | "doing" | "done";
  todo_done: number;
  todo_total: number;
  todos: Todo[];
  pid: number | null;
  connected: boolean;
  /** 本 GUI 进程正托管该会话的 PTY；门控贴纸卡片菜单「结束会话」的可见性。 */
  pty_managed: boolean;
  /**
   * 托管会话的屏幕检测状态（后端对 PTY 末屏做终端仿真 + 规则匹配，见 pty.rs ScreenProbe）。
   * 比 DB status 实时：blocked = 屏幕上挂着审批/提问 UI。null/缺省 = 非托管会话、
   * 无规则集或启动宽限内——此时卡片回退 DB status 口径。
   */
  screen_state?: "working" | "idle" | "blocked" | null;
  /**
   * 由 agent 自己的后台守护进程托管（Claude Code FleetView 的后台会话）。这类卡片是
   * agent 在会话中途自行派生的——用户没开过它，看板却凭空多出一张卡。界面据此标注来历，
   * 并收起接管/结束/定位终端：那几条路对它注定失败（supervisor 会把被杀的进程拉回来，
   * 它也没有终端窗口可切）。
   */
  background: boolean;
  archived: boolean;
  archived_at: number | null;
  cwd: string | null;
  errored: boolean;
  error_label: string | null;
  error_raw: string | null;
  /** 最近一条 AI 正文的轻推预览（清洗+截断），hover 卡片时速览；无正文回合为 null。 */
  preview: string | null;
  /** 用户给会话挂的便签（手写备忘，存本地 DB）；无便签为 null。 */
  note: string | null;
  /** 上下文已用百分比（来自 Claude Code statusline，准确）；无 statusline 数据为 null。 */
  context_pct: number | null;
  /** 上下文窗口大小（200000 或 1000000）；无 statusline 数据为 null。 */
  context_window: number | null;
  /** 模型展示名（Claude Code statusline 的 model.display_name，如 "Opus"）；无则 null。 */
  model: string | null;
  /** 待审批子态:回合中途等用户介入(批准工具/回答提问/批准计划);无则 null。 */
  pending_review: "approval" | "question" | "plan" | null;
  /** 最近一条 AI 正文(锚 Stop hook);无则 null,卡片回退 preview。 */
  last_ai_text: string | null;
  /** 最近一条用户消息(锚 UserPromptSubmit);独立字段,不被工具活动覆盖。 */
  last_user_text: string | null;
  /** agent 身份（如 "claude"）。DB 里可能存着本版本不认识的 id，故不是联合类型。 */
  provider: AgentId;
  /** 会话所属的账号（profile id）；null = 默认账号。 */
  profile: string | null;
  /** 所属账号的展示名（profile 已删时回退 id 本身）；null = 默认账号，不显示徽章。 */
  profile_name: string | null;
};

export type LiveSessionCounts = {
  total: number;
  running: number;
  waiting: number;
  archived: number;
};

export type ChatItem = GeneratedChatItem;
export type ChatHistory = ChatHistoryDto;
/** 一次委派可能派出多个子任务（kimi 的 AgentSwarm），故按分支返回。 */
export type SubagentRun = GeneratedSubagentRun;

export type PendingApproval = PendingApprovalDto;

/**
 * 取对话历史。offset>0 只回增量；offset=0 的首读默认只回尾部若干条（长会话可达上千条，
 * 全量会把巨大的 JSON 压到主线程），`full` 传 true 才返回整段——用于「加载更早的对话」。
 */
export function getChatHistory(sessionId: number, offset: number, full?: boolean): Promise<ChatHistory> {
  return invoke("get_chat_history", { sessionId, offset, full });
}

/**
 * 取一次子任务委派的完整时间线（用户展开那条 Agent 调用时才调用）。
 *
 * 子任务过程不在主 transcript 里，而在 provider 各自的侧车流中（claude 的
 * `subagents/agent-*.jsonl`、kimi 的 `agents/agent-N/wire.jsonl`）。刻意不并进
 * 650ms 的历史轮询：一个会话可能有几十个子任务，跟着热路径一起读毫无必要。
 */
export function getSubagentTranscript(sessionId: number, toolUseId: string): Promise<SubagentRun[]> {
  return invoke("get_subagent_transcript", { sessionId, toolUseId });
}

/**
 * 重读会话当前模型并落库。模型平时由 Stop hook 写入，而 `/model` 切换不产生 Stop——
 * GUI 驱动切换后调它，对话页与贴纸才会立刻反映新模型，而不是等下一条消息跑完。
 */
export function refreshSessionModel(sessionId: number): Promise<string | null> {
  return invoke("refresh_session_model", { sessionId });
}

/**
 * 用会话日志里的待办快照重建 DB，返回条数。
 *
 * 待办平时由 hook 落库，但 hook 只在 meowo 在场时捕获得到——中途才启动、hook 漏接、
 * 或早先解析有误（如状态别名不认识）时，DB 会与 agent 的真实清单脱节，而日志一直是对的。
 * 切换会话时调一次即可；agent 不从日志提供待办时返回 0 并保持 DB 现状。
 */
export function refreshSessionTodos(sessionId: number): Promise<number> {
  return invoke("refresh_session_todos", { sessionId });
}

export function openChatWindow(sessionId: number): Promise<void> {
  return invoke("open_chat_window", { sessionId });
}

/** 在默认浏览器打开对话内容里的链接。后端只放行 http/https，其余 scheme 一律拒绝。 */
export function openLink(url: string): Promise<void> {
  return invoke("open_link", { url });
}

/**
 * 把粘贴进对话输入框的图片/文件内容落成临时文件，返回绝对路径。
 * webview 剪贴板拿不到源文件路径（File 只有内容），附件协议却是「路径列表交给 CLI 读」，
 * 只能由宿主代为落盘。
 */
export function savePastedAttachment(fileName: string, dataBase64: string): Promise<string> {
  return invoke("save_pasted_attachment", { fileName, dataBase64 });
}

/**
 * 读系统剪贴板**图像**的指纹(只读不写);剪贴板里不是图像时为 null。
 * 用途:发送粘贴图片前比对「剪贴板里还是不是刚粘贴的那张图」,匹配才向 PTY 发 Ctrl-V
 * 走 TUI 自己的原生图片附加——不匹配绝不能发,否则附给 agent 的是错的图。
 */
export function clipboardImageFingerprint(): Promise<string | null> {
  return invoke("clipboard_image_fingerprint");
}

/// 会话是否可能仍由**外部**终端持有——即托管前需要先接管（杀掉旧进程）而非直接恢复。
///
/// `stale` 一并算入：它只表示久未有事件，进程未必已死。判活的事实源在后端（实时查进程表），
/// 前端这份 status 是轮询快照，只能保守。宁可多让用户确认一次接管（takeover 对已死进程同样
/// 安全——它会先判活，死了就直接恢复），也不要给出一个必然被后端拒绝的「直接恢复」按钮。
export function isExternallyHeld(status?: string): boolean {
  return status === "running" || status === "waiting" || status === "stale";
}

/// 对话窗/侧栏展示层的会话状态口径(后端 tab_class 是它的跨语言镜像,改任一端须同步)。
/// 优先级:connected 为假时绝不展示「在跑/在等」——DB 的 running 在进程死后、reaper
/// 收尾前是滞留值,直接展示就是假运行中;ended 同理让位于存活观测(恢复会话的轮询
/// 窗口期 DB 还挂着旧 'ended',而 PTY 已在跑,此时按 waiting 过渡而不是谎报已结束);
/// errored(出错,数据源为 LiveSession.errored / ChatHistoryDto.errored)高于 pending:
/// 出错必须先被看见;pending(待审批/待交互)高于 waiting:有明确动作召唤。
export type SessionTone = "running" | "pending" | "waiting" | "offline" | "ended" | "error";
export function sessionTone(connected: boolean, status?: string, pendingReview?: unknown, errored?: boolean): SessionTone {
  if (status === "ended" && !connected) return "ended";
  if (!connected) return "offline";
  if (errored) return "error";
  if (pendingReview) return "pending";
  if (status === "running") return "running";
  return "waiting";
}

export type ManagedTerminalSnapshot = ManagedTerminalSnapshotDto;
export function startManagedTerminal(sessionId: number, cols: number, rows: number): Promise<void> {
  return invoke("start_managed_terminal", { sessionId, cols, rows });
}
/**
 * 接上一个后台会话的画面（Agent 自己托管的会话，见 LiveSession.background）。
 *
 * 走的是 Agent 留给自己 UI 的那条旁路 socket：能看画面、能改尺寸、能结束，但**不能发消息**
 * ——键盘输入另有一道 attach 流程尚未打通。所以接上之后只把终端视图当只读画面用。
 */
export function attachBackgroundSession(sessionId: number): Promise<void> {
  return invoke("attach_background_session", { sessionId });
}

/**
 * 向后台会话发一条消息。走 Agent 守护进程的控制通道，不经过 PTY——后台 worker 不消费
 * stdin，往它的终端写按键会石沉大海。发出后 agent 就开始干活，回复照常落进 transcript。
 */
export function sendBackgroundPrompt(sessionId: number, text: string): Promise<void> {
  return invoke("send_background_prompt", { sessionId, text });
}

export function takeoverManagedTerminal(sessionId: number, cols: number, rows: number): Promise<void> {
  return invoke("takeover_managed_terminal", { sessionId, cols, rows });
}
/**
 * 取终端输出快照。`since` 传上次拿到的 endOffset，只回增量——不传则全量（首帧用）。
 * backlog 上限 1 MiB，轮询里省掉的就是这一整份的 base64 + IPC 传输。
 */
export function managedTerminalSnapshot(sessionId: number, since?: number): Promise<ManagedTerminalSnapshot> {
  return invoke("managed_terminal_snapshot", { sessionId, since });
}
export function managedTerminalBinding(sessionId: number): Promise<number | null> {
  return invoke("managed_terminal_binding", { sessionId });
}
export function writeManagedTerminal(sessionId: number, data: string): Promise<void> {
  return invoke("write_managed_terminal", { sessionId, data });
}
export function resizeManagedTerminal(sessionId: number, cols: number, rows: number): Promise<void> {
  return invoke("resize_managed_terminal", { sessionId, cols, rows });
}
export function stopManagedTerminal(sessionId: number): Promise<void> {
  return invoke("stop_managed_terminal", { sessionId });
}

/// 「结束会话」的唯一流程:应用内确认模态(appConfirm,系统原生 MessageBox 样式与应用
/// 脱节已弃用;window.confirm 会被 webview 吞掉,同样不可用)→ 杀托管 PTY 进程。
/// 对话页标题栏与终端页操作条两个入口共用,确认文案/停止协议改这里一处。
/// busy 态与错误呈现由调用方负责(两处 UI 槽位不同):`onConfirmed` 在用户确认后、真正
/// 结束前触发,给调用方置 busy;返回 false = 用户取消;抛错 = 结束失败,调用方必须让它可见。
export async function confirmStopSession(
  sessionId: number,
  text: { title: string; message: string },
  onConfirmed?: () => void,
): Promise<boolean> {
  const yes = await appConfirm(text.message, { title: text.title, danger: true });
  if (!yes) return false;
  onConfirmed?.();
  await stopManagedTerminal(sessionId);
  return true;
}
/**
 * 该会话待处理的审批 + AskUserQuestion 题面，一次取回（合并自两条 400ms 轮询）。
 * push 事件（pending-approval / interactive-question）是主路径；事件在对话窗冷启动时
 * 会打进虚空（Tauri emit 不排队、不重放），轮询这条是把错过的卡补回来的唯一途径。
 */
export function pendingInteraction(sessionId: number): Promise<PendingInteractionDto> {
  return invoke("pending_interaction", { sessionId });
}
/** 收卡时撤下后端的待处理题面，避免轮询把已答过的题重新弹出来。 */
export function dismissInteractiveQuestion(sessionId: number): Promise<void> {
  return invoke("dismiss_interactive_question", { sessionId });
}
export function registerApprovalConsumer(sessionId: number, consumerId: string): Promise<void> {
  return invoke("register_approval_consumer", { sessionId, consumerId });
}
export function unregisterApprovalConsumer(consumerId: string): Promise<void> {
  return invoke("unregister_approval_consumer", { consumerId });
}
export function resolvePendingApproval(sessionId: number, requestId: string, choice: string): Promise<void> {
  return invoke("resolve_pending_approval", { sessionId, requestId, choice });
}
export function openAttachedTerminal(sessionId: number): Promise<void> {
  return invoke("open_attached_terminal", { sessionId });
}

export function getLiveSessionsCounts(): Promise<LiveSessionCounts> {
  return invoke("get_live_sessions_counts");
}

export type StickerFilter = "all" | "running" | "waiting" | "archived";

export type PageCursor = { last_event_at: number; id: number };

/**
 * 会话分页响应。`next_cursor` 是后端的 **SQL 扫描位置**（排序前）：items 会做
 * connected-first 排序，末项不再是本页时间上最旧的一条，拿末项当游标会重复/漏页。
 * 翻下一页必须回传 next_cursor；null = 已到底。
 */
export type LiveSessionsPage = { items: LiveSession[]; next_cursor: PageCursor | null };

export function getLiveSessionsPage(
  filter: StickerFilter,
  search: string | null,
  cursor: PageCursor | null,
  limit: number,
  /** 目录筛选:斜杠归一的精确匹配(后端做),与 search 的子串搜索语义分开——
      同一目录的两种斜杠写法都命中,兄弟目录/标题命中不漏进来。 */
  cwd: string | null = null
): Promise<LiveSessionsPage> {
  return invoke<unknown>("get_live_sessions_page", {
    filter,
    search: search && search.trim() ? search : null,
    cwd: cwd && cwd.trim() ? cwd : null,
    // Tauri 按 camelCase 匹配 Rust 命令参数；snake_case 键会被静默当成缺失（Option → None），
    // 游标永远失效、「加载更多」重复返回第一页。
    beforeLastEventAt: cursor?.last_event_at ?? null,
    beforeId: cursor?.id ?? null,
    limit,
  }).then((res) => {
    // 旧后端 / demo mock 仍返回裸数组：给不满 limit 视作到底，满页时按旧约定用末项续查
    // （旧后端本就只有这套语义）。undefined = 后端没有该命令，静默降级为空列表。
    if (Array.isArray(res)) {
      const rows = res as LiveSession[];
      const last = rows[rows.length - 1];
      return {
        items: rows,
        next_cursor: rows.length >= limit && last
          ? { last_event_at: last.session.last_event_at, id: last.session.id }
          : null,
      };
    }
    const page = res as Partial<LiveSessionsPage> | null | undefined;
    if (page && Array.isArray(page.items)) {
      return { items: page.items, next_cursor: page.next_cursor ?? null };
    }
    return { items: [], next_cursor: null };
  });
}

export type ThemeMode = "dark" | "light" | "system";
export type TerminalLineHeight = "compact" | "normal" | "relaxed";

export type Settings = {
  /** 桌面通知总开关（待交互 + 错误）。 */
  notifications_enabled: boolean;
  /** 需要关注且 Meowo 都不在前台时,请求任务栏注意力(Windows 任务栏闪烁)。 */
  attention_flash_enabled: boolean;
  /** 自动检查并在后台下载软件更新。 */
  auto_update_enabled: boolean;
  /** 外观模式：深色 / 浅色 / 跟随系统。 */
  theme: ThemeMode;
  /** 贴纸背景不透明度（百分比 25–100）。 */
  opacity: number;
  /** 界面密度/字号缩放（百分比，紧凑 90 / 标准 100 / 宽松 112）。 */
  ui_scale: number;
  /** 打开未连接会话用的终端。macOS：terminal/iterm/ghostty；Windows：wt/powershell/cmd。 */
  resume_terminal: ResumeTerminal;
  /** 界面/通知语言：auto（跟随系统）/ zh / en。 */
  language: LangSetting;
  /** 打开终端方式：card = 点击卡片（默认）/ button = 卡片上单独的打开按钮。 */
  terminal_open_mode: TerminalOpenMode;
  /**
   * 打开会话落到哪个视图：chat = Meowo 对话窗口（默认）/ terminal = 外部终端。
   *
   * 两种取值下 agent 都由 Meowo 的 PTY 持有，差的只是用哪个界面看它——terminal 是把同一个
   * PTY attach 到 `resume_terminal` 选定的终端里，不是另起一个进程。
   */
  session_open_in: SessionOpenIn;
  /** 卡片菜单触发方式：context = 右键菜单（默认）/ button = 卡片菜单按钮（触屏友好），二选一。 */
  card_menu_mode: CardMenuMode;
  /** 是否在卡片显示对话预览（你的提问 + AI 回复两行）。缺省开启。 */
  preview_enabled: boolean;
  /** 终端（PTY 画面）字号（px，10–18）。缺省 12。 */
  terminal_font_size: number;
  /** 终端行高预设：compact / normal（默认，1.22）/ relaxed。 */
  terminal_line_height: TerminalLineHeight;
  /** 贴纸风格：elevated = 立体感（默认）/ flat = 扁平。 */
  sticker_style: StickerStyle;
  /** 贴纸底色预设 key（neutral/classic/slate/moss/plum/rose/amber）。 */
  sticker_color: string;
  /** 在贴纸底栏显示配额的 agent id 列表（后端给默认值）。 */
  sticker_quota_providers: AgentId[];
  /** 「新建会话」面板默认选中的 agent（后端给默认值）。 */
  default_agent: AgentId;
  /** 出站代理（用量查询 / OAuth 刷新 / 下载 agent 二进制 / 自更新），可按 agent 覆盖。 */
  proxy: ProxySettings;
  /** API 中转元数据；密钥由独立命令保存，不在此对象中。 */
  relay?: RelaySettings;
};

export type RelayAuth = string;
export type RelayProtocol = string;
export type RelayRule = {
  enabled: boolean;
  base_url: string;
  model: string;
  protocol: RelayProtocol | "";
  auth: RelayAuth;
  /** 勾选的附加环境变量选项 id；旧设置文件没有该字段，读取时按空数组处理。 */
  env_options?: string[];
};
export type RelaySettings = {
  per_agent: Partial<Record<AgentId, RelayRule>>;
};

/** 中转密钥是否已保存；只返回布尔状态，不返回密钥正文。 */
export function getRelaySecretStatus(): Promise<Record<AgentId, boolean>> {
  return invoke("get_relay_secret_status");
}

/** 读取本机已保存的中转密钥，供用户在设置页直接查看和修改。 */
export function getRelaySecrets(): Promise<Partial<Record<AgentId, string>>> {
  return invoke("get_relay_secrets");
}

/** 保存或替换中转密钥。空串用于删除；密钥不会进入 Settings。 */
export function setRelaySecret(agent: AgentId, secret: string): Promise<void> {
  return invoke("set_relay_secret", { agent, secret });
}

/** 从当前中转的兼容 `/models` 端点读取模型 ID；凭据仅由后端读取。 */
export function listRelayModels(
  agent: AgentId,
  baseUrl: string,
  protocol: RelayProtocol | "",
  auth: RelayAuth,
): Promise<string[]> {
  return invoke("list_relay_models", { agent, baseUrl, protocol, auth });
}

/**
 * 代理模式：
 * - `off`：直连，忽略环境变量。
 * - `system`：跟随系统环境变量（HTTPS_PROXY / ALL_PROXY / HTTP_PROXY）。
 * - `custom`：用 `url` 指定。
 */
export type ProxyMode = "off" | "system" | "custom";

export type ProxyRule = {
  mode: ProxyMode;
  /** `custom` 时的代理地址：`http://host:port` / `socks5://host:port`，可带 `user:pass@`。 */
  url: string;
};

export type ProxySettings = ProxyRule & {
  /** agent id → 覆盖规则。没有条目的 agent 一律跟随全局。 */
  per_agent: Record<AgentId, ProxyRule>;
};

/**
 * 某 agent（不传 = 全局规则）当前**生效**的代理串；null = 直连。
 *
 * 设置页用它显示 system 模式下实际读到的环境变量代理；更新窗口用它给 updater 传 proxy
 * （自更新走 reqwest，不经后端的 ureq 客户端，只能这样把设置递过去）。
 */
export function getEffectiveProxy(agent?: AgentId): Promise<string | null> {
  return invoke("get_effective_proxy", { agent: agent ?? null });
}

export type AvailableUpdate = {
  version: string;
  body?: string | null;
  downloadState: "available" | "downloading" | "ready";
};

/** 检查更新。后端会显式执行「自定义代理」或「直连」，不回退到系统环境代理。 */
export function checkUpdate(): Promise<AvailableUpdate | null> {
  return invoke("check_update");
}

/** 下载最近一次 checkUpdate() 返回的更新；进度经 update-download-progress 事件通知。 */
export function downloadUpdate(): Promise<"downloading" | "ready"> {
  return invoke("download_update");
}

/** 安装已下载并通过签名校验的更新。Windows 会在安装前退出应用。 */
export function installDownloadedUpdate(): Promise<void> {
  return invoke("install_downloaded_update");
}


export type ResumeTerminal = "terminal" | "iterm" | "ghostty" | "wt" | "wezterm" | "powershell" | "cmd";
export type LangSetting = "auto" | "zh" | "en";
export type TerminalOpenMode = "card" | "button";
export type SessionOpenIn = "chat" | "terminal";
export type CardMenuMode = "context" | "button";
export type StickerStyle = "elevated" | "flat";

/** 本机实际可用的「打开未连接会话」终端 key（供设置页过滤下拉项）。 */
export function availableTerminals(): Promise<ResumeTerminal[]> {
  return invoke("available_terminals");
}

/** 全部已注册 agent 及其本机安装状态。展示名、安装态都来自这里，前端不再硬编码 agent 名单。 */
export function listAgents(): Promise<AgentDescriptor[]> {
  return invoke("list_agents");
}

/**
 * 对话页能力查询。按会话的 provider + cwd 组装：装了什么版本、配了什么自定义命令，
 * 补全就出什么。未知 provider → null，调用方降级为不补全、不给模型菜单。
 */
export function agentChatUi(provider: AgentId, cwd: string | null, sessionId?: number): Promise<ChatUi | null> {
  return invoke("agent_chat_ui", { provider, cwd, sessionId });
}
/**
 * 该 agent 的模型清单，**由 CLI 自己列举**（非交互子命令，后端带进程级缓存）。
 * 空表 = 该 CLI 没有列举子命令，调用方回落到「弹 TUI 菜单 + 屏幕识别」那条路。
 */
export function agentModels(provider: string): Promise<string[]> {
  // 旧后端/demo 对未知命令返回 undefined——统一兜成空表，调用方不必各自判空。
  return invoke<string[] | null>("agent_models", { provider }).then((models) => models ?? []);
}

/**
 * 按 id 取展示名。未知 id（DB 里存着本版本不认识的 agent，或 list_agents 尚未 resolve）
 * 回退为 id 本身——显示 `"gemini"` 好过显示 `"Claude Code"`。
 */
export function agentName(agents: AgentDescriptor[], id: AgentId): string {
  return agents.find((a) => a.id === id)?.display_name ?? id;
}

export function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export function setSettings(settings: Settings): Promise<void> {
  return invoke("set_settings", { settings });
}

// 纯函数：根据 todo 列表算完成度。
export function todoProgress(todos: Todo[]): { done: number; total: number; percent: number } {
  const total = todos.length;
  const done = todos.filter((t) => t.status === "completed").length;
  const percent = total === 0 ? 0 : Math.round((done / total) * 100);
  return { done, total, percent };
}

export type UsageKind = "five_hour" | "seven_day" | "opus" | "weekly" | "balance" | "other";
export type UsageLane = {
  kind: UsageKind;
  used_pct: number | null;
  used: number | null;
  limit: number | null;
  unit: string | null;
  resets_at: string | null;
};
export type ProviderUsage = { lanes: UsageLane[]; note: string | null };
export type Account = {
  email: string | null;
  display_name: string | null;
  organization: string | null;
  plan: string | null;
  login_label: string | null;
};
export type ProviderAccountPayload = {
  provider: string;
  account: Account | null;
  usage: ProviderUsage | null;
  usage_supported: boolean;
  relay_enabled?: boolean;
  /** 当前活跃账号的展示名；null/缺省 = 默认账号（不显示任何账号提示）。 */
  active_profile_name?: string | null;
};

export function getAccounts(): Promise<ProviderAccountPayload[]> {
  return invoke("get_accounts");
}
export function refreshUsage(provider: string): Promise<ProviderUsage> {
  return invoke("refresh_usage", { provider });
}

/** 某 provider 的 meowo-reporter hooks 接入状态。unknown = 无法确认（读取失败/位置未知）。 */
export type HooksStatus = "installed" | "missing" | "unknown";

/** 新建一个全新会话：起托管 PTY，视图与终端类型由设置的 session_open_in / resume_terminal 决定。 */
/** `options`：启动选项的选择（option id → choice id），映射成 flag 由后端按插件声明表完成。 */
export function newSession(cwd: string, provider: AgentId, options?: Record<string, string>): Promise<void> {
  return invoke("new_session", { cwd, provider, options });
}

/** 最近使用过的工作目录（新建面板快捷选择）。 */
export function recentCwds(limit: number): Promise<string[]> {
  return invoke("recent_cwds", { limit });
}

/** 检测某 provider 的 meowo-reporter hooks 是否已接入（决定新建后会不会入库）。 */
export function checkProviderHooks(provider: AgentId): Promise<HooksStatus> {
  return invoke("check_provider_hooks", { provider });
}

/** 修复接线失败的原因（后端 setup::RepairReason），null = 成功/已是目标状态。 */
export type RepairReason =
  | "not-detected"
  | "need-login"
  | "reporter-not-found"
  | "config-unreadable"
  | "write-failed";

export type RepairResult = { status: HooksStatus; reason: RepairReason | null };

/** 手动修复某 provider 的 hooks：立即执行一次 setup::apply_provider，返回最新状态与失败原因。 */
export function repairProviderHooks(provider: AgentId): Promise<RepairResult> {
  return invoke("repair_provider_hooks", { provider });
}

/** 一键安装某 agent（在终端跑官方安装脚本）。装完在窗口重新聚焦/手动刷新时重检安装状态。 */
export function installAgent(provider: AgentId): Promise<void> {
  return invoke("install_agent", { provider });
}

/** 后台安装结束事件 payload（对应后端 install-done）。logPath 为安装脚本输出的落盘处（可能为 null）。 */
export type InstallDone = { provider: AgentId; ok: boolean; code: number | null; logPath: string | null };

/**
 * 该 agent 装好了、但它的 bin 目录不在持久 PATH 上 → 返回该目录；无需处理时 null。
 *
 * 存在的理由：官方安装器不保证写 PATH（claude 在 Windows 上只打印一行提示就 exit 0），
 * 而 meowo 启动 agent 走绝对路径、察觉不到，用户要到手敲 `claude` 才发现打不开。
 * 非 Windows 恒为 null——unix 的 PATH 由 shell profile 决定，不代用户改。
 */
export function agentPathGap(provider: AgentId): Promise<string | null> {
  return invoke("agent_path_gap", { provider });
}

/** 把该 agent 的 bin 目录写进用户级 PATH（幂等）。已开的终端需重开才能看到。 */
export function addAgentToUserPath(provider: AgentId): Promise<void> {
  return invoke("add_agent_to_user_path", { provider });
}

/**
 * 在终端里拉起该 agent 的交互式登录（claude 是 `auth login`，codex/kimi 是 `login`）。
 * 登录走浏览器 OAuth、终端是 detach 的，拿不到退出码——后端改为轮询账号解析结果，
 * 完成或超时（5 分钟）后 emit `login-done`。terminal 省略则用设置里的默认终端。
 */
/**
 * 拉起交互式登录。`profile` = 登进哪个账号：省略表示当前活跃账号，显式 null 表示默认账号。
 *
 * 多账号下要明确调用意图：省略时会操作当前活跃账号（通常是默认账号）；若要操作账号列表中的
 * 特定一行，应显式传它的 id，默认账号则显式传 null。
 */
export function loginAgent(
  provider: AgentId,
  terminal?: string,
  profile?: string | null,
  operationId: string = createLoginOperationId(provider),
): Promise<void> {
  return invoke("login_agent", {
    provider,
    terminal,
    profile: profile ?? null,
    useActive: profile === undefined,
    operationId,
  });
}

export function createLoginOperationId(provider: AgentId): string {
  const suffix = Math.random().toString(36).slice(2);
  return `login-${provider}-${Date.now().toString(36)}-${suffix}`;
}

/** 一个账号（profile）。`id === null` 即**默认账号**——agent 自己的目录，不可删除。 */
export type ProfileView = {
  id: string | null;
  /** 展示名。默认账号为空串，由前端本地化。 */
  name: string;
  active: boolean;
  /** 该账号自己的登录态。null = 未登录。 */
  account: Account | null;
};

/**
 * 某 agent 的账号列表（默认账号 + 自定义），每个都带自己的登录态。
 *
 * 不支持多账号的 agent（gemini：数据目录不可被环境变量覆盖）只会返回**一条**默认账号——
 * 前端据此不给「添加账号」入口。
 */
export function listProfiles(provider: AgentId): Promise<ProfileView[]> {
  return invoke("list_profiles", { provider });
}

/** 新建账号（建目录 + 接线）。返回它的 id。不会自动切过去，也不会自动登录。 */
export function createProfile(provider: AgentId, name: string): Promise<string> {
  return invoke("create_profile", { provider, name });
}

/**
 * 给账号改名。`id = null` → 默认账号（它的名字单独存，不在 profiles 里）。
 *
 * 只改展示名，不动它的目录（id）——id 是目录名，改了等于换了个账号。
 */
export function renameProfile(
  provider: AgentId,
  id: string | null,
  name: string,
): Promise<void> {
  return invoke("rename_profile", { provider, id, name });
}

/** 切换活跃账号。`id = null` → 切回默认账号。只影响此后新拉起的会话。 */
export function setActiveProfile(provider: AgentId, id: string | null): Promise<void> {
  return invoke("set_active_profile", { provider, id });
}

/** 删除账号，**连同它的整个目录**（凭据、配置、该账号的会话历史）。不可逆。 */
export function deleteProfile(provider: AgentId, id: string): Promise<void> {
  return invoke("delete_profile", { provider, id });
}

/**
 * 把账号**合并进默认账号**：数据目录并入（不覆盖默认账号已有的文件）、会话改挂默认账号、
 * 账号从列表移除。与删除的根本区别是数据全留下。有进行中的会话时后端会拒绝。
 */
export function mergeProfileIntoDefault(provider: AgentId, id: string): Promise<void> {
  return invoke("merge_profile_into_default", { provider, id });
}

/**
 * 取消该 agent 的登录等待。
 *
 * 点完登录后如果终端被关掉（手动关、崩溃、agent 自己退出），后端毫不知情——它只轮询账号文件，
 * 会一直等到 5 分钟超时。这个出口让用户立刻落回可点状态。
 *
 * 后端仍会 emit 带同一 operationId 的 `login-done`；取消前会再查一次账号并返回明确 outcome。
 */
export function cancelLogin(provider: AgentId, operationId: string): Promise<void> {
  return invoke("cancel_login", { provider, operationId });
}

/**
 * 用 API Key 登录（`supports_api_key_login` 的 agent，当前只有 gemini）。同步落盘、当场生效：
 * 后端把 key 写进 CLI 自己认的位置（gemini：`~/.gemini/.env` + settings 的 selectedType），
 * resolve 后重查账号即可。`profile` 语义同 logoutAgent（省略/null = 当前活跃账号）。
 */
export function apiKeyLogin(provider: AgentId, key: string, profile?: string | null): Promise<void> {
  return invoke("api_key_login", { provider, key, profile: profile ?? null });
}

/** 退出官方账号。不会删除模型配置、会话、hooks 或中转配置。 */
/**
 * 退出登录。`profile` = 登出哪个账号（省略/null = 当前活跃账号）。
 *
 * 多账号下**必须传**：登出会清掉那个账号目录里的凭据，漏传就会去清默认账号的——而删凭据是
 * 不可逆的。它与「删除账号」不是一回事：登出只清凭据，目录、配置、会话历史都留着，还能再登回来；
 * 而默认账号压根删不掉（那是 agent 自己的目录），登出是它唯一的退出手段。
 */
export function logoutAgent(provider: AgentId, profile?: string | null): Promise<void> {
  return invoke("logout_agent", { provider, profile: profile ?? null });
}

/** 登录结束事件 payload。operationId 将结果严格关联到发起它的那一轮登录。 */
export type LoginDone = LoginDoneEvent;

/** 该 provider 是否已登录：账号能解析出来就算登录（三家判据各异，已在后端 account() 内收敛）。 */
export function isLoggedIn(payload: ProviderAccountPayload | undefined): boolean {
  return !!payload?.account;
}
