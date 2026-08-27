import { invoke } from "@tauri-apps/api/core";
import { appConfirm } from "./confirm";
import type { AgentDescriptor as AgentDescriptorDto } from "./generated/contracts/AgentDescriptor";
import type { ChatUi as ChatUiDto } from "./generated/contracts/ChatUi";
import type { ChatHistoryDto } from "./generated/contracts/ChatHistoryDto";
import type { LaunchChoice } from "./generated/contracts/LaunchChoice";
import type { LaunchOption } from "./generated/contracts/LaunchOption";
import type { LiveSession as LiveSessionDto } from "./generated/contracts/LiveSession";
import type { LiveSessionCounts } from "./generated/contracts/LiveSessionCounts";
import type { Session as SessionDto } from "./generated/contracts/Session";
import type { Todo as TodoRowDto } from "./generated/contracts/Todo";
import type { ModelPreset } from "./generated/contracts/ModelPreset";
import type { ModeControl } from "./generated/contracts/ModeControl";
import type { ModeInput } from "./generated/contracts/ModeInput";
import type { ModeOption } from "./generated/contracts/ModeOption";
import type { ModeScreenMarker } from "./generated/contracts/ModeScreenMarker";
import type { RelayUi } from "./generated/contracts/RelayUi";
import type { SlashCommand } from "./generated/contracts/SlashCommand";
import type { ChatItem as GeneratedChatItem } from "./generated/contracts/ChatItem";
import type { SubagentRun as GeneratedSubagentRun } from "./generated/contracts/SubagentRun";
import type { SubagentProbeDto } from "./generated/contracts/SubagentProbeDto";
import type { ManagedTerminalSnapshotDto } from "./generated/contracts/ManagedTerminalSnapshotDto";
import type { LineageEntryDto } from "./generated/contracts/LineageEntryDto";
import type { PendingApprovalDto } from "./generated/contracts/PendingApprovalDto";
import type { SwitchStartedDto } from "./generated/contracts/SwitchStartedDto";
import type { PendingInteractionDto } from "./generated/contracts/PendingInteractionDto";
import type { LoginDoneEvent } from "./generated/contracts/LoginDoneEvent";
import type { GitDiffSummaryDto } from "./generated/contracts/GitDiffSummaryDto";
import type { GitFileDiffDto } from "./generated/contracts/GitFileDiffDto";
import type { DirEntryDto } from "./generated/contracts/DirEntryDto";
import type { FileTextDto } from "./generated/contracts/FileTextDto";
import type { FileOpenerDto } from "./generated/contracts/FileOpenerDto";
import type { ImagePreviewDto } from "./generated/contracts/ImagePreviewDto";
import type { SearchFileHitDto } from "./generated/contracts/SearchFileHitDto";
import type { SearchLineHitDto } from "./generated/contracts/SearchLineHitDto";
import type { SearchResultDto } from "./generated/contracts/SearchResultDto";

export type {
  GitDiffSummaryDto,
  GitFileDiffDto,
  DirEntryDto,
  FileTextDto,
  FileOpenerDto,
  ImagePreviewDto,
  SearchFileHitDto,
  SearchLineHitDto,
  SearchResultDto,
};

/**
 * agent 身份串（`"claude"` / `"kimi"` / …），与 Rust 侧 `meowo_agent::AgentId` 同值。
 *
 * 刻意**不是**联合类型：agent 名单由后端的 `list_agents()` 下发，前端不再维护自己的一份
 * （此前加一个 CLI 要同步 4 处：这个联合、PROVIDERS 表、测试里的 EXPECTED_KEYS、Rust 的枚举）。
 * DB 里还可能存着本版本尚不认识的 id，联合类型会对它撒谎。
 */
export type AgentId = string;

/**
 * 后端下发的一个 agent。前端认识 agent 的唯一途径。
 *
 * 类型由 ts-rs 从 `meowo_agent::descriptor::AgentDescriptor` 生成（字段注释见生成文件，
 * CI diff 守卫同步）；此处仅把 id 收窄为 `AgentId` 别名、保持既有导出名。
 */
export type AgentDescriptor = Omit<AgentDescriptorDto, "id"> & { id: AgentId };

// 启动选项 / 模型预设 / 斜杠命令 / 模式控制：全部由 ts-rs 生成，原名重导出。
export type { LaunchChoice, LaunchOption, ModelPreset, SlashCommand, ModeInput, ModeOption, ModeScreenMarker, ModeControl };

/**
 * 对话页能力，由**安装实况**组装：插件内置表 ∪ 用户/项目目录里发现的自定义命令 + CLI 版本。
 * 不随 `list_agents()` 静态下发——它依赖会话的 cwd（项目级命令）且随安装变化。
 * 类型由 ts-rs 从 `meowo_agent::chat_ui::ChatUi` 生成。`selector_anchors[].kind` 的取值
 * 约定（"input" | "chat"）与 terminalAttention 对齐，生成类型上是 string——后端为权威。
 */
export type ChatUi = ChatUiDto;

/** API 中转能力表（后端 `RelayUi` 的生成镜像；导出名沿用前端习惯的 RelayCapability）。 */
export type RelayCapability = RelayUi;

/** DB 行类型由 ts-rs 从 meowo-store 生成；此处仅把 status 收窄为已知取值集。 */
export type Todo = Omit<TodoRowDto, "status"> & { status: "pending" | "in_progress" | "completed" };
export type Session = Omit<SessionDto, "status"> & { status: "running" | "waiting" | "ended" | "stale" };

/**
 * 看板/侧栏的一张会话卡 = store 的 `LiveSession`（ts-rs 生成）+ **app 层增补**。
 * 增补字段来自 `session_query.rs` 的 `LiveItem`（serde flatten 到同层）：它们依赖进程表/
 * PTY/屏幕检测，store 层给不出，故仍手写——LiveItem 在 meowo-app crate，不参与契约导出
 * （契约步骤只跑 protocol/agent/store 的 --lib，不编译整个 tauri 宿主）。
 */
export type LiveSession = Omit<LiveSessionDto, "session" | "todos" | "column" | "pending_review" | "provider"> & {
  session: Session;
  todos: Todo[];
  column: "todo" | "doing" | "done";
  /** 待审批子态:回合中途等用户介入(批准工具/回答提问/批准计划);无则 null。 */
  pending_review: "approval" | "question" | "plan" | null;
  /** agent 身份（如 "claude"）。DB 里可能存着本版本不认识的 id，故不是联合类型。 */
  provider: AgentId;
} & {
  // ── 以下为 LiveItem 的 app 层增补 ──
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
  errored: boolean;
  error_label: string | null;
  error_raw: string | null;
  /**
   * 还在跑的后台子任务数（transcript 分析）。主回合停了（status=waiting/屏幕 idle）
   * 但它非零时，卡片环与分组都按「运行中」处理——后台委派还在独立干活，不该催人。
   */
  busy_subagents: number;
  /** 最近一条 AI 正文的轻推预览（清洗+截断），hover 卡片时速览；无正文回合为 null。 */
  preview: string | null;
  /** 所属账号的展示名（profile 已删时回退 id 本身）；null = 默认账号，不显示徽章。 */
  profile_name: string | null;
  /**
   * 来自另一个实例的库（dev 构建只读聚合安装版 ~/.meowo/board.db 的会话）。
   * 该会话的 PTY/审批/收尾归安装版，本实例只有观察权：卡片标注来源，全部操作入口收起。
   * id 已被后端加高位偏移防撞——即使漏禁某个入口，本库无此行，命令落空成 no-op。
   */
  foreign: boolean;
};

export type { LiveSessionCounts };

/** 已有会话中途附加目录的落库半边（resume 回放依据）。运行中进程的即时生效由调用方
 *  另经发送通道发 `/add-dir <dir>`。返回是否真的新增（重复目录 no-op）。 */
export function addSessionExtraDir(sessionId: number, dir: string): Promise<boolean> {
  return invoke("add_session_extra_dir", { sessionId, dir });
}

/** 移除一个附加目录(落库侧,下次恢复不再带上;运行中进程已持有的权限收不回)。 */
export function removeSessionExtraDir(sessionId: number, dir: string): Promise<boolean> {
  return invoke("remove_session_extra_dir", { sessionId, dir });
}

/** 选目录并附加到会话(对话窗 diff 面板 / 侧栏条目菜单共用,免得两处各自漂移):
 *  用户取消返回 null;失败抛原始错误,由调用方走各自的提示通道。 */
export async function pickAndAddExtraDir(sessionId: number): Promise<boolean | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true });
  if (typeof picked !== "string") return null;
  return addSessionExtraDir(sessionId, picked);
}

export type ChatItem = GeneratedChatItem;
export type ChatHistory = ChatHistoryDto;
/** 一次委派可能派出多个子任务（kimi 的 AgentSwarm），故按分支返回。 */
export type SubagentRun = GeneratedSubagentRun;
/** 一次未结委派的侧车实测状态，见 {@link probeSubagentStates}。 */
export type SubagentProbe = SubagentProbeDto;

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
 * 实测若干条**未结**委派此刻的状态（进度面板展开时才调用）。
 *
 * 折叠状态下的进度只能来自主链回执，而并行委派的回执要等同一步里的工具全部跑完才一起
 * 写盘——整批跑完之前，先收工的子任务在主链上毫无痕迹，面板只能一律显示「在跑」。侧车流
 * 自己带着终结标记，这条按需 I/O 逐条读它的尾部补齐；同 {@link getSubagentTranscript}，
 * 刻意不并进 650ms 的历史轮询。
 *
 * 返回值按 tool_use_id 索引，读不出状态的 id 直接缺席——「读不到」不等于「已结束」。
 */
export function probeSubagentStates(
  sessionId: number,
  toolUseIds: string[],
): Promise<Record<string, SubagentProbe>> {
  return invoke("probe_subagent_states", { sessionId, toolUseIds });
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
 * 把本地图片文件写进系统剪贴板，供紧随其后的 Ctrl-V 走 TUI 原生图片附加（支持多张：
 * 逐张写入、逐张等占位符）。首次写入前后端自动快照现有剪贴板内容；发送结束（无论
 * 成败）必须调 clipboardRestore 还原。失败会 reject——调用方据此回退指令文本，
 * 绝不能照常发 Ctrl-V（那会把剪贴板里别人的内容附给 agent）。
 */
export function clipboardSetImage(path: string): Promise<void> {
  return invoke("clipboard_set_image", { path });
}
/** 还原 clipboardSetImage 快照下来的剪贴板内容；没有快照时为空操作。 */
export function clipboardRestore(): Promise<void> {
  return invoke("clipboard_restore");
}
/** 读剪贴板文本(终端右键粘贴)。readText 在 WebView2 要权限弹窗,走后端 arboard 零打扰。 */
export function clipboardText(): Promise<string | null> {
  return invoke("clipboard_text");
}

/**
 * 会话是否可能仍由**外部**终端持有——即托管前需要先接管（杀掉旧进程）而非直接恢复。
 *
 * `stale` 一并算入：它只表示久未有事件，进程未必已死。判活的事实源在后端（实时查进程表），
 * 前端这份 status 是轮询快照，只能保守。宁可多让用户确认一次接管（takeover 对已死进程同样
 * 安全——它会先判活，死了就直接恢复），也不要给出一个必然被后端拒绝的「直接恢复」按钮。
 */
export function isExternallyHeld(status?: string): boolean {
  return status === "running" || status === "waiting" || status === "stale";
}

/**
 * 对话窗/侧栏展示层的会话状态口径(后端 tab_class 是它的跨语言镜像,改任一端须同步)。
 * 优先级:connected 为假时绝不展示「在跑/在等」——DB 的 running 在进程死后、reaper
 * 收尾前是滞留值,直接展示就是假运行中;ended 同理让位于存活观测(恢复会话的轮询
 * 窗口期 DB 还挂着旧 'ended',而 PTY 已在跑,此时按 waiting 过渡而不是谎报已结束);
 * errored(出错,数据源为 LiveSession.errored / ChatHistoryDto.errored)高于 pending:
 * 出错必须先被看见;pending(待审批/待交互)高于 waiting:有明确动作召唤。
 */
/** `focus_session` 的返回值（后端 terminal.rs 同名结果的镜像）：跳转外部终端的结局分类。 */
export type FocusSessionResult =
  | "focused"
  | "host_focused"
  | "alive_but_not_found"
  | "permission_denied"
  | "unsupported_terminal"
  | "process_ended";

export type SessionTone = "running" | "pending" | "waiting" | "offline" | "ended" | "error";
export function sessionTone(connected: boolean, status?: string, pendingReview?: unknown, errored?: boolean, busySubagents?: number): SessionTone {
  if (status === "ended" && !connected) return "ended";
  if (!connected) return "offline";
  if (errored) return "error";
  if (pendingReview) return "pending";
  if (status === "running") return "running";
  // 主回合停了但后台子任务还在跑:不是「等你输入」——没有要输入的东西,工作还在进行。
  // 与后端 tab_class 的 background_busy 同口径(LiveSession.busy_subagents,transcript 分析)。
  return busySubagents && busySubagents > 0 ? "running" : "waiting";
}

export type ManagedTerminalSnapshot = ManagedTerminalSnapshotDto;
/** `options`：恢复时对启动选项的改选（option id → choice id，如 {permission:"bypassPermissions"}），
 *  省略 = 沿用会话存的选择。改选会被后端合并写回，成为会话新的持久形态。 */
export function startManagedTerminal(sessionId: number, cols: number, rows: number, options?: Record<string, string>): Promise<void> {
  return invoke("start_managed_terminal", { sessionId, cols, rows, options });
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

/** `options` 语义同 startManagedTerminal：接管时改权限模式等启动选项，省略 = 沿用。 */
export function takeoverManagedTerminal(sessionId: number, cols: number, rows: number, options?: Record<string, string>): Promise<void> {
  return invoke("takeover_managed_terminal", { sessionId, cols, rows, options });
}
/** 会话存的启动选项（option id → choice id）。恢复权限下拉据此把「沿用原设置」
 *  亮成具体档位；没存过 / 旧后端无此命令时为空。 */
export function sessionLaunchSelections(sessionId: number): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("session_launch_selections", { sessionId }).then((map) => map ?? {}).catch(() => ({}));
}
/** 把单个启动选项写进会话存档（合并写）。模型/权限是启动参数，resume/接管时回放
 *  ——对话页切换后不落库的话，每次重启都回默认档（1M 上下文档回落 200K，实拍）。
 *  写失败要让调用方知道（否则用户切了档、下次 resume 悄悄回默认），错误不在这里吞。 */
export function setSessionLaunchSelection(sessionId: number, option: string, choice: string): Promise<void> {
  return invoke<void>("set_session_launch_selection", { sessionId, option, choice });
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

/**
 * 取 PTY 当前**生效**的网格尺寸 `[cols, rows]`；未知（会话不在/后台旁路/尚未设过）为 `[0, 0]`。
 *
 * 终端页可见时每隔几秒查一次，与本地 fit 出的网格比对——不等就说明某次 resize 没落地，
 * 补发一次把 PTY 拉齐。刻意不复用快照：那个要把整个 backlog（可达 1 MiB）编码重传。
 */
export function managedTerminalGrid(sessionId: number): Promise<[number, number]> {
  return invoke("managed_terminal_grid", { sessionId });
}
export function stopManagedTerminal(sessionId: number): Promise<void> {
  return invoke("stop_managed_terminal", { sessionId });
}
/** 声明 chat 窗终端视图「正在看」的会话——后端 emitter 只对它推送 pty-output 实时帧。 */
export function registerTerminalViewer(sessionId: number): Promise<void> {
  return invoke("register_terminal_viewer", { sessionId });
}
/** 注销「正在看」(后端 CAS,只清自己的注册,重挂竞态安全)。 */
export function unregisterTerminalViewer(sessionId: number): Promise<void> {
  return invoke("unregister_terminal_viewer", { sessionId });
}

/**
 * 「结束会话」的唯一流程:应用内确认模态(appConfirm,系统原生 MessageBox 样式与应用
 * 脱节已弃用;window.confirm 会被 webview 吞掉,同样不可用)→ 杀托管 PTY 进程。
 * 对话页标题栏与终端页操作条两个入口共用,确认文案/停止协议改这里一处。
 * busy 态与错误呈现由调用方负责(两处 UI 槽位不同):`onConfirmed` 在用户确认后、真正
 * 结束前触发,给调用方置 busy;返回 false = 用户取消;抛错 = 结束失败,调用方必须让它可见。
 */
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

/**
 * 会话改名（写入与 Claude Code `/rename` 一致的记录，广播 board-changed）。
 * 看板卡片、侧栏、对话窗标题三个入口共用；以 cc_session_id 为键，provider
 * 让后端选对应插件的回写实现。
 */
export function renameSession(
  cwd: string | null | undefined,
  ccSessionId: string,
  title: string,
  provider?: string | null,
): Promise<void> {
  return invoke("rename_session", { cwd, sessionId: ccSessionId, title, provider });
}

/** 归档 / 还原会话（键为 DB 数字主键，不是 cc_session_id）。 */
export function setArchived(sessionId: number, archived: boolean): Promise<void> {
  return invoke("set_archived", { sessionId, archived });
}

/** 给会话挂 / 改便签（纯本地备忘，键为 cc_session_id）。 */
export function setSessionNote(ccSessionId: string, note: string): Promise<void> {
  return invoke("set_session_note", { sessionId: ccSessionId, note });
}

/** 在系统文件管理器里打开会话的工作目录。 */
export function openProjectDir(cwd: string): Promise<void> {
  return invoke("open_project_dir", { cwd });
}

/** 工作目录的 git 改动摘要；`isRepo` 为 false 时前端不显示「Diff」入口。 */
export function getGitDiffSummary(cwd: string): Promise<GitDiffSummaryDto> {
  return invoke("git_diff_summary", { cwd });
}

/** 单个文件的 diff（untracked=true 时由后端读盘合成伪 diff，见 GitFileDiffDto 注释）。 */
export function getGitFileDiff(cwd: string, path: string, untracked: boolean): Promise<GitFileDiffDto> {
  return invoke("git_file_diff", { cwd, path, untracked });
}

/** 「文件」页签：列出 cwd 下 rel 目录的直接子条目（目录在前，后端已排序并跳过 .git）。 */
export function listDirEntries(cwd: string, rel: string): Promise<DirEntryDto[]> {
  return invoke("list_dir_entries", { cwd, rel });
}

/** 「文件」页签：读取 cwd 下 rel 文件的文本（后端封顶 200KB、嗅探二进制，见 FileTextDto 注释）。 */
export function readFileText(cwd: string, rel: string): Promise<FileTextDto> {
  return invoke("read_file_text", { cwd, rel });
}

/** 「文件」页签：图片预览（后端按扩展名识别、读盘转 data URL，超 8MB 回 tooLarge）。 */
export function readImagePreview(cwd: string, rel: string): Promise<ImagePreviewDto> {
  return invoke("read_image_preview", { cwd, rel });
}

/** 「文件」页签：cwd 下按文件/目录名与文本内容搜索（大小写不敏感，后端带命中/扫描上限）。 */
export function searchProjectFiles(cwd: string, query: string): Promise<SearchResultDto> {
  return invoke("search_project_files", { cwd, query });
}

/** 文件面板「打开」菜单：能打开该文件的应用清单（Windows 为系统「打开方式」推荐列表，
 *  含图标；其余平台为探测到的编辑器）。目录返回空；系统默认应用不在其中，前端恒列。 */
export function listFileOpeners(cwd: string, rel: string): Promise<FileOpenerDto[]> {
  return invoke("list_file_openers", { cwd, rel });
}

/** 用指定应用（opener=清单里的 id）或系统默认应用（opener="default"）打开 cwd 内的文件/目录。 */
export function openPathWith(cwd: string, rel: string, opener: string): Promise<void> {
  return invoke("open_path_with", { cwd, rel, opener });
}

/** 在系统文件管理器里定位（选中）cwd 内的文件/目录。 */
export function revealPathInFileManager(cwd: string, rel: string): Promise<void> {
  return invoke("reveal_path_in_file_manager", { cwd, rel });
}

/** 对话内容全文搜索的一条命中（后端 chat.rs 的 TranscriptSearchHit，serde camelCase）。 */
export type TranscriptSearchHit = {
  sessionId: number;
  title: string;
  projectName: string;
  excerpt: string;
};

/** 对话内容全文搜索：扫最近会话的 transcript 文件（显式动作，不随击键触发）。
 *  每会话至多一条命中，按活跃序返回，后端带扫描上限（秒级以内）。 */
export function searchChatTranscripts(query: string): Promise<TranscriptSearchHit[]> {
  return invoke("search_chat_transcripts", { query });
}

/** 打开「新建会话」独立窗；带 prefill 时预选目录与 agent。 */
export function openNewSessionWindow(prefill?: { cwd?: string | null; provider?: string | null }): Promise<void> {
  return prefill
    ? invoke("open_new_session_window", { cwd: prefill.cwd, provider: prefill.provider })
    : invoke("open_new_session_window");
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
  cwd: string | null = null,
  /** 附带外库(安装版)会话——只有贴纸看板传 true(dev 构建的监控视野补全);
      对话侧栏/归档页按 id 加载详情,外库卡点开必落空,从源头不给。 */
  includeForeign = false
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
    includeForeign,
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
   * 打开会话落到哪个视图：terminal = 外部终端（默认，后端 settings.rs 有测试钉住——
   * 老用户升级不该被静默改掉落点）/ chat = Meowo 对话窗口。
   *
   * 两种取值下 agent 都由 Meowo 的 PTY 持有，差的只是用哪个界面看它——terminal 是把同一个
   * PTY attach 到 `resume_terminal` 选定的终端里，不是另起一个进程。
   */
  session_open_in: SessionOpenIn;
  /**
   * 对话窗口功能总开关（「轻量模式」）。false 时应用只保留贴纸生态：所有 chat 入口
   * 隐藏，审批/交互提问回落终端 TUI。安装器「自定义安装」的勾选写入首选值（Windows）。
   */
  chat_enabled: boolean;
  /** 卡片菜单触发方式：button = 卡片菜单按钮（默认）/ context = 右键菜单，二选一。 */
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
  /** 手机远程访问总开关（局域网/Tailscale + token）。默认关闭。 */
  remote_access_enabled: boolean;
  /** 远程 HTTP 服务端口。默认 18620。 */
  remote_access_port: number;
  /** 远程桥绑定网卡：all = 所有网卡(默认,旧行为) / loopback = 仅本机 / tailscale = 仅 Tailscale。 */
  remote_access_bind: RemoteBindMode;
  /** 远程访问 token。由 remote_access_info 惰性生成落盘；get_settings 不返回明文(恒空串),
   *  set_settings 忽略回传值(后端以磁盘值为准)。 */
  remote_access_token: string;
};

/** 远程桥绑定模式（与后端 settings.rs remote_access_bind 逐值对齐）。 */
export type RemoteBindMode = "all" | "loopback" | "tailscale";

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

/** 归类后的可达地址候选：kind 决定 UI 怎么标注「手机什么情况下连得上」。 */
export type RemoteIpCandidate = {
  ip: string;
  kind: "tailscale" | "lan";
};

/** 设置页远程访问配对信息（桌面专用命令，不经 /rpc）。 */
export type RemoteAccessInfo = {
  enabled: boolean;
  port: number;
  /** 惰性生成的 token；二维码 URL = http://<ip>:<port>/#token=<token>。 */
  token: string;
  /** 可达地址候选，Tailscale 优先（可能为空,回退手输）。 */
  ips: RemoteIpCandidate[];
  /** 最近一次启动失败原因（端口被占等），无错为 null。 */
  lastError: string | null;
};

export function remoteAccessInfo(): Promise<RemoteAccessInfo> {
  return invoke("remote_access_info");
}

/** 重新生成远程访问令牌（桌面专用）。换发即吊销:server 重启,已配对手机全部回配对页。 */
export function regenerateRemoteToken(): Promise<void> {
  return invoke("regenerate_remote_token");
}

/** 全部会话里正在等用户的清单(审批+同步题面)。远程徽标 3s 扫描用——push 事件到不了
 *  浏览器,非当前会话的审批只能靠它点亮侧栏徽标;桌面走事件,不调这条。 */
export function awaitingInteractionSessions(): Promise<number[]> {
  return invoke("awaiting_interaction_sessions");
}

/** 远程新建会话的页内目录浏览（只列目录名）。path 为空 = 磁盘/根列表。 */
export type DirListing = {
  /** 当前目录规范化绝对路径；根列表时为空串。 */
  path: string;
  /** 上一级；空串 = 回磁盘列表；null = 已在顶层。 */
  parent: string | null;
  dirs: { name: string; path: string }[];
};

/** 仅远程模式可用：命令只在 /rpc 白名单登记，桌面 invoke 会被拒（桌面有系统对话框）。 */
export function listSubdirectories(path?: string): Promise<DirListing> {
  return invoke("list_subdirectories", { path: path ?? null });
}

// 纯函数：根据 todo 列表算完成度。
export function todoProgress(todos: Todo[]): { done: number; total: number; percent: number } {
  const total = todos.length;
  const done = todos.filter((t) => t.status === "completed").length;
  const percent = total === 0 ? 0 : Math.round((done / total) * 100);
  return { done, total, percent };
}

export type UsageKind = "five_hour" | "seven_day" | "opus" | "weekly" | "model_weekly" | "balance" | "other";
export type UsageLane = {
  kind: UsageKind;
  used_pct: number | null;
  used: number | null;
  limit: number | null;
  unit: string | null;
  resets_at: string | null;
  /** 泳道附加名。目前只有 model_weekly 用:被限定的模型名(如 "Fable")。可选兼容旧后端。 */
  label?: string | null;
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
/** `options`：启动选项的选择（option id → choice id），映射成 flag 由后端按插件声明表完成。
 *  `extraDirs`：附加目录（跨仓同一需求 = 一个会话 + --add-dir）;仅声明支持的 agent 接受。 */
export function newSession(cwd: string, provider: AgentId, options?: Record<string, string>, extraDirs: string[] = []): Promise<void> {
  return invoke("new_session", { cwd, provider, options, extraDirs });
}

/** 最近使用过的工作目录（新建面板快捷选择）。 */
export function recentCwds(limit: number): Promise<string[]> {
  return invoke("recent_cwds", { limit });
}

export type SwitchStarted = SwitchStartedDto;
export type LineageEntry = LineageEntryDto;

/**
 * 跨 provider 切换：结束当前会话进程，把结构化历史导出成交接文件，在同一目录用
 * 目标 agent 起新会话（返回临时负 id，走既有 binding 轮询换真 id）。接续链由后端
 * 在 claim 认领时落库——目标 CLI 起不来时旧会话不会被标记为已接替。
 */
export function switchSessionProvider(
  sessionId: number,
  targetProvider: AgentId,
  options?: Record<string, string>,
): Promise<SwitchStarted> {
  return invoke("switch_session_provider", { sessionId, targetProvider, options });
}

/** 接续链回看（按时间升序）。不在链上的会话返回仅含自身的单段。 */
export function getSessionLineage(sessionId: number): Promise<LineageEntry[]> {
  return invoke("get_session_lineage", { sessionId });
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
