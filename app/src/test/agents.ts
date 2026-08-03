import type { AgentDescriptor, ChatUi, LaunchOption, SlashCommand } from "../api";

/** 测试里已知的五家产品名。真实值由后端 `list_agents()` 下发（须与注册表的 display_name 同值）。 */
const NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  kimi: "Kimi Code",
  gemini: "Gemini CLI",
  opencode: "OpenCode",
};
const RELAYS: Record<string, AgentDescriptor["relay"]> = {
  claude: { protocols: [], auth_modes: [{ value: "bearer", label: "Bearer Token" }, { value: "api_key", label: "API Key (x-api-key)" }], default_protocol: "", default_auth: "bearer", suggestions: [{ protocol: "", models: ["claude-sonnet-5"] }] },
  codex: { protocols: [], auth_modes: [{ value: "bearer", label: "Bearer Token" }], default_protocol: "", default_auth: "bearer", suggestions: [{ protocol: "", models: ["gpt-5.4"] }] },
  kimi: { protocols: [{ value: "kimi", label: "Kimi" }, { value: "anthropic", label: "Anthropic Messages" }, { value: "openai", label: "OpenAI Chat Completions" }], auth_modes: [{ value: "bearer", label: "Bearer Token" }], default_protocol: "kimi", default_auth: "bearer", suggestions: [{ protocol: "kimi", models: ["kimi-for-coding"] }] },
  // gemini：只有一种协议（讲 Gemini 自己的 generateContent，故 protocols 空）+ API Key 认证。
  gemini: { protocols: [], auth_modes: [{ value: "api_key", label: "API Key" }], default_protocol: "", default_auth: "api_key", suggestions: [{ protocol: "", models: ["gemini-2.5-pro"] }] },
  // opencode：中转＝往它配置里注入自定义 provider（OpenAI 兼容 / Anthropic）。
  opencode: { protocols: [{ value: "openai", label: "OpenAI 兼容" }, { value: "anthropic", label: "Anthropic Messages" }], auth_modes: [{ value: "bearer", label: "Bearer Token" }], default_protocol: "openai", default_auth: "bearer", suggestions: [{ protocol: "openai", models: ["gpt-5.4"] }] },
};

/**
 * 能被套上代理的那些（＝插件声明了 ProxySpec）。当前**五家都有**（gemini/opencode 也认标准代理
 * 环境变量，实测其 bundle/二进制）。
 * 与后端 `AgentPlugin::proxy()` 的能力矩阵同源——那边加了代理能力，这里也要跟上。
 */
const SUPPORTS_PROXY = new Set(["claude", "codex", "kimi", "gemini", "opencode"]);

/**
 * 代理能写进自己配置文件（＝不管由谁启动都生效）的那些。与后端 `ProxySpec.config_env`
 * 同源——**仅 claude**：codex/kimi/gemini 只认进程环境变量，故只覆盖 Meowo 拉起的会话。
 */
const PROXY_COVERS_ALL_LAUNCHES = new Set(["claude"]);

/**
 * 支持 SOCKS 代理的那些。与后端 `ProxySpec.socks` 同源，逐个核对过插件声明：
 * claude 官方明确不支持、codex 未编译 reqwest 的 socks feature、opencode 亦为 false，
 * 只有 kimi 与 gemini 认 SOCKS。
 */
const PROXY_ACCEPTS_SOCKS = new Set(["kimi", "gemini"]);

/**
 * 有账号概念的那些（＝插件声明了 account 能力槽）。当前**五家都有**，故这里全收。
 *
 * 它仍然是个可选能力槽：为 false 的 agent 不显示登录态、也不给登录入口（它的 `login_argv()`
 * 会是 None，按钮点下去只会报「拉起登录失败」）。要测那条路径，在用例里手写一个
 * `supports_account: false` 的 descriptor——别改这里，这里必须与后端的真实矩阵同源。
 */
const SUPPORTS_ACCOUNT = new Set(["claude", "codex", "kimi", "gemini", "opencode"]);

/**
 * 能用 API Key 登录的那些（＝插件声明了 ApiKeyLoginCap）。**只有 gemini**：它的 OAuth 被官方
 * 停用（个人版 Code Assist 关闸），key 是唯一活路，且 CLI 没有输入 key 的登录子命令。
 * 与后端 `AgentPlugin::api_key_login()` 同源。
 */
const SUPPORTS_API_KEY_LOGIN = new Set(["gemini"]);

/**
 * 能有多个账号的那些（＝插件声明了 ProfileSpec）。**gemini 不行**：它的数据目录无法被环境变量
 * 覆盖（`GEMINI_DIR` 实测无效），谎称支持会让两个「账号」静默共用同一份凭据。
 */
const SUPPORTS_PROFILES = new Set(["claude", "codex", "kimi", "opencode"]);

/**
 * meowo 能显示上下文占用的那些。**gemini/opencode 不行**：gemini 的 hook 不带 token，opencode
 * 没声明 telemetry（token 在它自己库里）。与后端 `AgentPlugin::provides_context()` 同源。
 */
const SUPPORTS_CONTEXT = new Set(["claude", "codex", "kimi"]);

/**
 * 对话页内置 `/` 补全候选。与后端 `AgentPlugin::slash_commands()` 同源——各家命令表是插件
 * 声明的事实（gemini 是 `/stats` 不是 `/status`，opencode 是 `/models` 不是 `/model`）。
 */
const SLASH_COMMANDS: Record<string, string[]> = {
  claude: ["/clear", "/compact", "/config", "/cost", "/help", "/init", "/mcp", "/memory", "/model", "/resume", "/review", "/status"],
  codex: ["/clear", "/compact", "/diff", "/help", "/model", "/new", "/review", "/status"],
  kimi: ["/clear", "/compact", "/help", "/model", "/status"],
  gemini: ["/chat", "/clear", "/compress", "/help", "/mcp", "/memory", "/stats", "/tools"],
  opencode: ["/compact", "/exit", "/help", "/init", "/models", "/new", "/share", "/undo"],
};

/**
 * 快速切模型预设。**只有 claude**：它的 `/model` 接受内联参数；其余四家是交互式菜单，
 * 声明了预设等于给出一个点了没反应的菜单。与后端 `AgentPlugin::model_presets()` 同源。
 */
const MODEL_PRESETS: Record<string, ChatUi["model_presets"]> = {
  claude: [
    { id: "fable", label: "Fable" },
    { id: "opus", label: "Opus" },
    { id: "sonnet", label: "Sonnet" },
    { id: "haiku", label: "Haiku" },
    { id: "opusplan", label: "Opus Plan" },
  ],
};

/**
 * 造一份 `agent_chat_ui()` 的返回：该 agent 的内置命令表 + 模型预设（无自定义命令）。
 * 要测自定义命令，在用例里 `custom` 传发现出来的条目。未知 provider 回 null——与后端同语义。
 */
/**
 * 整句可操作提示的识别规格。与后端各插件的 `attention_patterns()` 声明同源——
 * 那边改了正则，这里也要跟上，否则前端测试测的是一套过期规则。
 * 未取证的 provider 为空（不做整句识别）。
 */
const ATTENTION_PATTERNS: Record<string, { id: string; patterns: string[]; last: boolean }[]> = {
  claude: [
    {
      id: "claude:long-session-resume",
      patterns: [
        "this session is[^\\n]{0,120}\\bold and[^\\n]{0,80}\\btokens\\b",
        "resuming the full session will consume a substantial portion of your usage limits",
      ],
      last: false,
    },
    {
      id: "claude:command-approval",
      patterns: ["this command requires approval", "do you want to proceed\\?"],
      last: true,
    },
  ],
  kimi: [
    {
      id: "kimi:command-approval",
      patterns: [
        "↑/↓ select · [\\d/]+ choose · ↵ confirm",
        "^\\s*▶\\s*(?:run this command|write this file|apply these edits|stop this task|ready to build with this plan|approve [^\\n?]+)\\?",
      ],
      last: true,
    },
  ],
};

/**
 * 识别文法固件：带上该 provider 由插件声明的整句规则。
 *
 * 改造前测试可以只写 `{ provider: "kimi", selectorAnchors: [] }`，因为识别层按
 * `provider ===` 自带规则；现在规则由后端下发，**不传就等于该 agent 没声明规则**。
 * 用这个构造器而不是手写字面量，测试才与生产路径（chatUi 下发）走同一份数据。
 */
export function grammar(
  provider: string,
  selectorAnchors: { marker: string; kind: "input" | "chat" }[] = [],
): { provider: string; selectorAnchors: typeof selectorAnchors; attentionPatterns: typeof ATTENTION_PATTERNS[string] } {
  return { provider, selectorAnchors, attentionPatterns: ATTENTION_PATTERNS[provider] ?? [] };
}

export function chatUi(provider: string, custom: SlashCommand[] = []): ChatUi | null {
  const builtins = SLASH_COMMANDS[provider];
  if (!builtins) return null;
  const commands: SlashCommand[] = [
    ...builtins
      .filter((name) => !custom.some((c) => c.name === name))
      .map((name) => ({ name, description: null, source: "builtin" as const })),
    ...custom,
  ].sort((a, b) => a.name.localeCompare(b.name));
  return {
    slash_commands: commands,
    attention_patterns: ATTENTION_PATTERNS[provider] ?? [],
    model_presets: MODEL_PRESETS[provider] ?? [],
    // 只有 claude 的 `/model` 接受内联参数；其余几家是交互式菜单，靠这条命令打开。
    // opencode 的命令是 /models(复数)——与后端插件声明同源。
    model_menu_command: (MODEL_PRESETS[provider] ?? []).length > 0 ? null : provider === "opencode" ? "/models" : "/model",
    mode_controls: provider === "claude"
      ? [{
        dimension: "permission", cycle_input: "\u001b[Z", options: [],
        // 与后端 claude 插件同源：官方文档承诺的状态栏指示文本 → transcript 的 permissionMode 值。
        screen_markers: [
          { marker: "bypass permissions on", value: "bypassPermissions" },
          { marker: "accept edits on", value: "acceptEdits" },
          { marker: "plan mode on", value: "plan" },
          { marker: "auto mode on", value: "auto" },
          { marker: "don't ask on", value: "dontAsk" },
          { marker: "manual mode on", value: "default" },
        ],
      }]
      : provider === "codex"
        ? [{ dimension: "collaboration", cycle_input: "\u001b[Z", options: [], screen_markers: [] }]
        : [],
    startup_attention_markers: provider === "claude"
      ? ["do you trust the files in this folder", "do you trust the contents of this directory", "trust this folder", "workspace not trusted", "workspace trust dialog"]
      : ["do you trust the files in this folder", "do you trust the contents of this directory"],
    // 与后端总装同源：claude 声明的五个 + 交互式 /model 型 CLI 并入的 model_menu_command。
    menu_slash_commands: provider === "claude"
      ? ["/config", "/mcp", "/memory", "/model", "/resume"]
      : (MODEL_PRESETS[provider] ?? []).length > 0 ? [] : [provider === "opencode" ? "/models" : "/model"],
    // 与后端同源:选择器锚点是插件声明的识别文法,当前只有 claude 有真机取证。
    selector_anchors: provider === "claude"
      ? [{ marker: "type something", kind: "input" as const }, { marker: "chat about this", kind: "chat" as const }]
      : [],
    // 与后端同源:五家都用 Esc 中断(claude/codex 确证,其余约定推断,见插件注释)。
    interrupt_input: "",
    runtime_commands_pending: false,
    // 与后端同源:claude/gemini(提交时原生附加)与 kimi(@ 是它自己的文本级一等语法)
    // 声明 true。测试桩模拟「已探测到版本」的常态,故不做 version 门控。
    attachment_mention: provider === "claude" || provider === "gemini" || provider === "kimi",
    // 与后端同源:claude/kimi 的 TUI 文档明确 Ctrl-V 读剪贴板原生附加图片,占位符各家不同。
    clipboard_image_paste: provider === "claude" ? "\\[Image #\\d" : provider === "kimi" ? "\\[image[:# ]" : null,
    version: null,
  };
}

/**
 * 启动选项。与后端 `AgentPlugin::launch_options()` 同源：claude 有模型+权限两栏，
 * codex/gemini 各一栏审批，kimi/opencode 未调研到稳定 flag、如实不声明。
 */
const LAUNCH_OPTIONS: Record<string, LaunchOption[]> = {
  claude: [
    {
      id: "model",
      default: "default",
      choices: [
        { id: "default", label: "Default", args: [] },
        { id: "fable", label: "Fable", args: ["--model", "fable"] },
        { id: "opus", label: "Opus", args: ["--model", "opus"] },
        { id: "sonnet", label: "Sonnet", args: ["--model", "sonnet"] },
        { id: "haiku", label: "Haiku", args: ["--model", "haiku"] },
        { id: "opusplan", label: "Opus Plan", args: ["--model", "opusplan"] },
      ],
    },
    {
      id: "permission",
      default: "default",
      choices: [
        { id: "default", label: "Default", args: [] },
        { id: "plan", label: "Plan", args: ["--permission-mode", "plan"] },
        { id: "acceptEdits", label: "Accept Edits", args: ["--permission-mode", "acceptEdits"] },
        { id: "bypassPermissions", label: "Bypass Permissions", args: ["--permission-mode", "bypassPermissions"] },
      ],
    },
  ],
  codex: [
    {
      id: "approval",
      default: "default",
      choices: [
        { id: "default", label: "Default", args: [] },
        { id: "readOnly", label: "Read Only", args: ["--sandbox", "read-only"] },
        { id: "fullAuto", label: "Full Auto", args: ["--full-auto"] },
        { id: "yolo", label: "YOLO", args: ["--dangerously-bypass-approvals-and-sandbox"] },
      ],
    },
  ],
  gemini: [
    {
      id: "approval",
      default: "default",
      choices: [
        { id: "default", label: "Default", args: [] },
        { id: "autoEdit", label: "Auto Edit", args: ["--approval-mode", "auto_edit"] },
        { id: "yolo", label: "YOLO", args: ["--yolo"] },
      ],
    },
  ],
};

/**
 * 造一份 `list_agents()` 的返回：列出全部已知 agent，其中 `installed` 里的标记为已装。
 *
 * 各测试此前各自 mock `availableAgents()` 的 id 数组；descriptor 多了展示名与安装态两个字段，
 * 抽到一处免得三个测试文件各写一遍。
 */
export function descriptors(installed: string[]): AgentDescriptor[] {
  return Object.keys(NAMES).map((id) => ({
    id,
    display_name: NAMES[id],
    installed: installed.includes(id),
    supports_proxy: SUPPORTS_PROXY.has(id),
    proxy_covers_all_launches: PROXY_COVERS_ALL_LAUNCHES.has(id),
    proxy_accepts_socks: PROXY_ACCEPTS_SOCKS.has(id),
    supports_account: SUPPORTS_ACCOUNT.has(id),
    supports_api_key_login: SUPPORTS_API_KEY_LOGIN.has(id),
    supports_profiles: SUPPORTS_PROFILES.has(id),
    supports_context: SUPPORTS_CONTEXT.has(id),
    launch_options: LAUNCH_OPTIONS[id] ?? [],
    relay: RELAYS[id],
  }));
}
