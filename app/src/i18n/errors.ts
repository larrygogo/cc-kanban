/// 后端错误消息的展示层映射。Rust 侧的 Err(String) 是中文 sentinel(全量 i18n 需要错误码
/// 重构,工程量与收益不成比——务实路线:高频可见错误在这里做双语与口语化,未命中的原样
/// 显示;英文界面下含汉字的未命中项降级为 "Operation failed: {raw}",不让中文直漏)。
/// 注意:这里的 match 串是与后端约定的 sentinel,改 Rust 文案必须同步改这里。

type ErrorEntry = {
  /** 匹配串:exact 为空时按前缀匹配(带参数的错误,如「同步会话失败：{e}」)。 */
  m: string;
  /** 精确匹配(默认按前缀)。 */
  exact?: boolean;
  /** 前缀匹配时把匹配串之后的原文(通常是「：具体原因」)追加到译文后,不丢排障信息。 */
  tail?: boolean;
  zh: string;
  en: string;
};

/** 按声明顺序匹配:精确项放前、长前缀放短前缀前。 */
const ENTRIES: ErrorEntry[] = [
  // ── 会话 / 终端 ──
  { m: "无效 session_id", exact: true, zh: "会话已失效，请重新打开", en: "Session is no longer valid; reopen it" },
  { m: "会话不存在", exact: true, zh: "会话不存在", en: "Session not found" },
  { m: "目录不存在", exact: true, zh: "项目目录不存在", en: "Project directory not found" },
  { m: "未知 agent", exact: true, zh: "未知的 Agent", en: "Unknown agent" },
  { m: "当前平台不支持", exact: true, zh: "当前平台不支持此操作", en: "Not supported on this platform" },
  { m: "当前平台暂不支持托管终端", exact: true, zh: "当前平台暂不支持托管终端", en: "Managed terminals aren't supported on this platform" },
  { m: "该 Agent 不支持恢复会话", exact: true, zh: "该 Agent 不支持恢复会话", en: "This agent can't resume sessions" },
  { m: "该会话尚未由 Meowo 接管", exact: true, zh: "会话尚未接管", en: "Session not taken over yet" },
  { m: "PTY 会话未运行", exact: true, zh: "会话终端未在运行", en: "The session terminal is not running" },
  { m: "PTY 已结束", exact: true, zh: "会话终端已结束", en: "The session terminal has ended" },
  { m: "PTY 输入通道已关闭", exact: true, zh: "会话终端已结束", en: "The session terminal has ended" },
  { m: "单次 PTY 输入过大", exact: true, zh: "输入内容过大", en: "Input too large" },
  { m: "原会话仍在运行，未重新打开", exact: true, zh: "原会话仍在运行，未重新打开", en: "The original session is still running; nothing was reopened" },
  { m: "无法结束原会话进程", exact: true, zh: "无法结束原会话进程", en: "Couldn't stop the original session process" },
  { m: "会话进程已变化，请刷新后重试", exact: true, zh: "会话进程已变化，请刷新后重试", en: "The session process changed; refresh and retry" },
  { m: "会话仍在外部终端运行", zh: "会话仍在外部终端运行", en: "The session is still running in an external terminal" },
  { m: "外部终端已在线，但没能带到前台，请手动切换", exact: true, zh: "外部终端已在线，但没能带到前台，请手动切换", en: "External terminal is online but couldn't be focused; switch manually" },
  { m: "打开外部同步终端失败", zh: "打开外部终端失败", en: "Couldn't open the external terminal" },
  { m: "启动受支持的终端失败", zh: "无法启动受支持的终端", en: "Couldn't launch a supported terminal" },
  { m: "同步会话失败", tail: true, zh: "同步会话失败", en: "Couldn't sync the session" },
  // tail 透传:前端 i18n 也会生成同前缀消息(t.chat.terminalStartExited,含退出码),
  // zh 必须原样保留尾部信息;英文界面下该消息本就是英文,不会命中此中文前缀。
  { m: "Agent 启动后立即退出", tail: true, zh: "Agent 启动后立即退出", en: "The agent exited right after starting; check the Terminal tab —" },
  { m: "该会话所属账号", zh: "会话所属账号已被删除，无法恢复", en: "This session's account was deleted; it can't be resumed" },
  { m: "请选择工作目录", exact: true, zh: "请选择工作目录", en: "Pick a working directory" },
  { m: "无法定位当前账号目录", exact: true, zh: "找不到当前账号的数据目录", en: "Couldn't locate the current account directory" },
  { m: "内部状态异常", zh: "内部状态异常，请重启 Meowo", en: "Internal error — restart Meowo" },
  // ── 审批 ──
  { m: "审批请求已结束", exact: true, zh: "审批请求已结束", en: "The approval request has ended" },
  { m: "审批请求不属于该会话", exact: true, zh: "该审批属于另一条会话", en: "This approval belongs to another session" },
  { m: "Agent 已不再等待审批", exact: true, zh: "Agent 已不再等待审批", en: "The agent is no longer waiting for approval" },
  { m: "无效的审批选项", exact: true, zh: "该审批选项已失效", en: "That approval option is no longer valid" },
  { m: "审批选项已失效", exact: true, zh: "该审批选项已失效", en: "That approval option is no longer valid" },
  // ── 后台会话(bgpty / managed_terminal)──
  { m: "该后台会话未接入", exact: true, zh: "后台会话未连接", en: "Background session not attached" },
  { m: "后台会话已结束或被 Agent 收回", exact: true, zh: "后台会话已结束或被 Agent 收回", en: "The background session has ended or was reclaimed by the agent" },
  { m: "后台会话不接受终端按键", zh: "后台会话不接受终端按键，请在对话页发消息", en: "Background sessions ignore terminal keys; send from the chat page" },
  { m: "找不到 Agent 的控制通道", zh: "连不上后台会话的控制通道", en: "Couldn't reach the background session's control channel" },
  { m: "连接后台会话", tail: true, zh: "无法连接后台会话", en: "Couldn't connect to the background session" },
  { m: "写后台会话", tail: true, zh: "消息没能送进后台会话，请重试", en: "Couldn't deliver to the background session; try again" },
  { m: "向后台会话发送失败", tail: true, zh: "消息没能送进后台会话，请重试", en: "Couldn't deliver to the background session; try again" },
  { m: "等待后台会话回执失败", tail: true, zh: "后台会话未确认收到消息", en: "The background session didn't acknowledge the message" },
  { m: "后台会话没有回执", zh: "后台会话未确认收到消息，请稍后重试", en: "The background session didn't acknowledge the message; try again" },
  { m: "后台会话未确认收到消息", zh: "后台会话未确认收到消息，请稍后重试", en: "The background session didn't acknowledge the message; try again" },
  { m: "后台会话拒绝了这条消息", tail: true, zh: "后台会话拒绝了这条消息", en: "The background session rejected this message" },
  { m: "消息为空", exact: true, zh: "消息为空", en: "Empty message" },
  // ── 切换引擎(handoff)──
  { m: "该会话已被接替", zh: "会话已被接替，请到最新的延续会话上操作", en: "This session was superseded; switch from the latest continuation" },
  { m: "该 Agent 不提供结构化会话记录，无法携带上下文切换", exact: true, zh: "该 Agent 无结构化记录，无法携带上下文切换", en: "This agent has no structured transcript; can't switch with context" },
  { m: "会话记录为空，没有可交接的上下文", exact: true, zh: "会话记录为空，没有可交接的内容", en: "The transcript is empty; nothing to hand off" },
  { m: "未找到会话记录文件", exact: true, zh: "找不到会话记录文件", en: "Transcript file not found" },
  { m: "找不到会话记录文件", exact: true, zh: "找不到会话记录文件", en: "Transcript file not found" },
  { m: "会话缺少工作目录", zh: "会话缺少工作目录，无法在原目录延续", en: "The session has no working directory; can't continue in place" },
  { m: "写入交接文件失败", tail: true, zh: "写入交接文件失败", en: "Couldn't write the handoff file" },
  // ── 对话记录 ──
  { m: "该 Agent 不提供结构化会话记录", exact: true, zh: "该 Agent 不提供结构化会话记录", en: "This agent doesn't provide a structured transcript" },
  { m: "找不到该子任务的记录", exact: true, zh: "找不到该子任务的记录", en: "No record found for this subtask" },
  // ── 账号 / 登录 / 安装 ──
  { m: "没有这个账号", exact: true, zh: "账号不存在", en: "Account not found" },
  { m: "账号名不能为空", exact: true, zh: "账号名不能为空", en: "Account name can't be empty" },
  { m: "该 agent 不支持多账号", exact: true, zh: "该 Agent 不支持多账号", en: "This agent doesn't support multiple accounts" },
  { m: "该 agent 不支持 API 中转", exact: true, zh: "该 Agent 不支持 API 中转", en: "This agent doesn't support API relay" },
  { m: "该 agent 不支持 API Key 登录", exact: true, zh: "该 Agent 不支持 API Key 登录", en: "This agent doesn't support API-key sign-in" },
  { m: "该 agent 未声明登录入口", exact: true, zh: "该 Agent 没有登录入口", en: "This agent has no sign-in method" },
  { m: "登录操作已结束或已被替换", exact: true, zh: "本次登录已结束或被新的登录替换", en: "The sign-in attempt has ended or was replaced" },
  { m: "启动终端失败：无法拉起登录流程", exact: true, zh: "无法打开终端进行登录", en: "Couldn't open a terminal to start sign-in" },
  { m: "解析不到该账号的目录", zh: "找不到该账号的数据目录", en: "Couldn't locate this account's data directory" },
  { m: "找不到该账号的数据目录", exact: true, zh: "找不到该账号的数据目录", en: "Couldn't locate this account's data directory" },
  { m: "找不到默认账号的数据目录", exact: true, zh: "找不到默认账号的数据目录", en: "Couldn't locate the default account's data directory" },
  { m: "解析不到该 agent 的安装实况", zh: "无法确认该 Agent 的安装状态", en: "Couldn't determine this agent's install state" },
  { m: "删除账号目录失败", tail: true, zh: "删除账号目录失败", en: "Couldn't delete the account directory" },
  { m: "该 agent 没有可用的一键安装命令", exact: true, zh: "该 Agent 不支持一键安装", en: "One-click install isn't available for this agent" },
  { m: "未找到该 agent 的可执行目录", exact: true, zh: "找不到该 Agent 的安装目录", en: "Couldn't find this agent's install directory" },
  { m: "下载失败", tail: true, zh: "下载失败", en: "Download failed" },
  { m: "下载安装脚本失败", tail: true, zh: "下载安装脚本失败", en: "Couldn't download the install script" },
  { m: "执行安装失败", tail: true, zh: "安装失败", en: "Installation failed" },
  { m: "启动安装失败", tail: true, zh: "无法启动安装", en: "Couldn't start the installation" },
  { m: "安装程序以退出码", zh: "安装程序执行失败", en: "The installer exited with an error" },
  // ── 代理 ──
  { m: "已选「自定义代理」，但代理地址为空", exact: true, zh: "已选自定义代理，请填写代理地址", en: "Custom proxy selected — enter a proxy address" },
  { m: "代理地址为空", exact: true, zh: "代理地址为空", en: "Proxy address is empty" },
  { m: "代理地址不能含空格", exact: true, zh: "代理地址不能含空格", en: "Proxy address can't contain spaces" },
  { m: "代理地址缺少主机", exact: true, zh: "代理地址缺少主机", en: "Proxy address is missing a host" },
  { m: "代理端口无效", tail: true, zh: "代理端口无效", en: "Invalid proxy port" },
  { m: "代理地址无效", tail: true, zh: "代理地址无效", en: "Invalid proxy address" },
  { m: "host:port:user:pass 四段都不能为空", zh: "host:port:user:pass 四段都不能为空", en: "All four host:port:user:pass parts are required" },
  // ── 更新 ──
  { m: "请先检查更新", exact: true, zh: "请先检查更新", en: "Check for updates first" },
  { m: "更新尚未下载完成", exact: true, zh: "更新还没下载完", en: "The update hasn't finished downloading" },
  // ── 文件 / git / 打开方式 ──
  { m: "附件过大", zh: "附件过大（上限 32MB）", en: "Attachment too large (max 32MB)" },
  { m: "空附件", exact: true, zh: "附件是空的", en: "Empty attachment" },
  { m: "不是文件", exact: true, zh: "所选路径不是文件", en: "The selected path is not a file" },
  { m: "不是目录", exact: true, zh: "所选路径不是目录", en: "The selected path is not a directory" },
  { m: "路径不存在", exact: true, zh: "路径不存在", en: "Path not found" },
  { m: "路径超出工作目录", exact: true, zh: "路径超出工作目录", en: "Path is outside the working directory" },
  { m: "无法读取文件", tail: true, zh: "无法读取文件", en: "Couldn't read the file" },
  { m: "无法读取目录", tail: true, zh: "无法读取目录", en: "Couldn't read the directory" },
  { m: "不是支持的图片格式", exact: true, zh: "不支持的图片格式", en: "Unsupported image format" },
  { m: "git status 执行失败", zh: "git status 执行失败", en: "git status failed" },
  { m: "git diff 执行失败", zh: "git diff 执行失败", en: "git diff failed" },
  { m: "未检测到该编辑器", exact: true, zh: "未检测到该编辑器", en: "Editor not found" },
  { m: "该文件类型没有关联应用", exact: true, zh: "该文件类型没有关联应用", en: "No app is associated with this file type" },
  { m: "该打开方式已不可用", exact: true, zh: "该打开方式已不可用", en: "That app is no longer available" },
  // ── 窗口 ──
  { m: "创建对话窗口失败", tail: true, zh: "打开对话窗口失败", en: "Couldn't open the chat window" },
  { m: "打开对话窗口失败", tail: true, zh: "打开对话窗口失败", en: "Couldn't open the chat window" },
  { m: "切换会话失败", tail: true, zh: "切换会话失败", en: "Couldn't switch session" },
];

/** 是否含汉字(CJK 统一表意文字基本区,后端 sentinel 均落在此区间)。 */
const hasHan = (text: string): boolean => /[一-鿿]/.test(text);

/**
 * 把后端错误(通常是 `invoke` reject 出来的 String)格式化成当前语言的用户文案。
 * @param raw    任意错误值(Error / string / unknown),内部先转字符串。
 * @param locale 当前界面 locale(取 `t.locale`),zh 开头视为中文界面。
 */
export function formatBackendError(raw: unknown, locale: string): string {
  const text = raw instanceof Error ? raw.message : String(raw ?? "");
  const isZh = locale.startsWith("zh");
  for (const entry of ENTRIES) {
    const hit = entry.exact ? text === entry.m : text.startsWith(entry.m);
    if (!hit) continue;
    const base = isZh ? entry.zh : entry.en;
    // tail:把「：具体原因」原样带上——译文负责可读,原因负责排障。
    return entry.tail && !entry.exact ? base + text.slice(entry.m.length) : base;
  }
  // 未命中:中文界面原样显示(原文本就是中文);英文界面遇到中文原文时降级为通用前缀,
  // 保留原文便于反馈排查——比一句纯中文可读。
  if (!isZh && hasHan(text)) return `Operation failed: ${text}`;
  return text;
}
