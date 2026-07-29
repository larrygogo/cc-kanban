// demo 专用:用 @tauri-apps/api/mocks 拦截全部 IPC,数据源是内存 store。
// 舞台状态(窗口形态/字幕/收尾)也放这里,分镜动作改完 notify() 即驱动 React 重渲染。
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { Settings, type ProviderAccountPayload, type ProviderUsage } from "../api";
import { Item } from "./data";

export type StageMode = "normal" | "docking" | "strip" | "expanded";

export type Store = {
  sessions: Item[];
  stage: { mode: StageMode; caption: string | null; finale: boolean; glow: boolean };
  settings: Settings;
};

export const store: Store = {
  sessions: [],
  stage: { mode: "normal", caption: null, finale: false, glow: false },
  settings: {
    notifications_enabled: true,
    attention_flash_enabled: true,
    auto_update_enabled: true,
    theme: "dark",
    opacity: 100,
    ui_scale: 100,
    resume_terminal: "wt",
    language: "zh",
    terminal_open_mode: "card",
    session_open_in: "terminal",
    card_menu_mode: "button",
    preview_enabled: true,
    terminal_font_size: 12,
    terminal_line_height: "normal",
    sticker_style: "flat",
    sticker_color: "neutral",
    sticker_quota_providers: ["claude", "codex"],
    default_agent: "claude",
    proxy: { mode: "system", url: "", per_agent: {} },
  },
};

const subs = new Set<() => void>();
export function subscribe(fn: () => void): () => void {
  subs.add(fn);
  return () => subs.delete(fn);
}
export function notify(): void {
  subs.forEach((f) => f());
}

function mkUsage(fiveH: number, sevenD: number): ProviderUsage {
  return {
    lanes: [
      { kind: "five_hour", used_pct: fiveH, used: null, limit: null, unit: null, resets_at: "2026-06-18T20:00:00Z" },
      { kind: "seven_day", used_pct: sevenD, used: null, limit: null, unit: null, resets_at: "2026-06-24T08:00:00Z" },
    ],
    note: null,
  };
}

function mkAccount(provider: string, fiveH: number, sevenD: number): ProviderAccountPayload {
  return {
    provider,
    account: { email: "demo@example.com", display_name: "Demo User", organization: null, plan: "Pro", login_label: null },
    usage: mkUsage(fiveH, sevenD),
    usage_supported: true,
  };
}

export function installMocks(): void {
  mockWindows("main");
  mockIPC((cmd, args) => {
    switch (cmd) {
      case "host_os":
        return "windows";
      case "list_agents":
        // 后端 list_agents:agent 名单(展示名 + 安装态),前端 useAgents 取展示名并交叉过滤底栏配额。
        // demo 演示多 Agent:装了 Claude Code / Codex / Kimi / Gemini CLI（不独尊 Claude）。
        return [
          { id: "claude", display_name: "Claude Code", installed: true },
          { id: "codex", display_name: "Codex", installed: true },
          { id: "kimi", display_name: "Kimi Code", installed: true },
          { id: "gemini", display_name: "Gemini CLI", installed: true },
        ];
      case "agent_chat_ui": {
        // 对话页能力按会话查询（真实后端由安装实况组装）。demo 只演 claude 的对话窗：
        // 内置表 + 一条「项目里发现的自定义命令」，让补全菜单的两类来源都露脸。
        if ((args as { provider?: string })?.provider !== "claude") return null;
        return {
          slash_commands: [
            ...["/clear", "/compact", "/config", "/cost", "/help", "/init", "/mcp", "/memory", "/model", "/resume", "/review", "/status"]
              .map((name) => ({ name, description: null, source: "builtin" })),
            { name: "/deploy", description: "部署到预发环境", source: "project" },
          ].sort((a, b) => a.name.localeCompare(b.name)),
          model_presets: [
            { id: "fable", label: "Fable" },
            { id: "opus", label: "Opus" },
            { id: "sonnet", label: "Sonnet" },
            { id: "haiku", label: "Haiku" },
            { id: "opusplan", label: "Opus Plan" },
          ],
          mode_controls: [{ dimension: "permission", cycle_input: "\u001b[Z", options: [], screen_markers: [] }],
          menu_slash_commands: ["/config", "/mcp", "/memory", "/model", "/resume"],
          startup_attention_markers: ["do you trust the files in this folder", "do you trust the contents of this directory", "trust this folder", "workspace not trusted", "workspace trust dialog"],
          selector_anchors: [{ marker: "type something", kind: "input" }, { marker: "chat about this", kind: "chat" }],
          interrupt_input: "",
          runtime_commands_pending: false,
          attachment_mention: true,
          clipboard_image_paste: "\\[Image #\\d",
          version: "2.1.215 (Claude Code)",
        };
      }
      case "get_settings":
        return store.settings;
      case "get_accounts": {
        // demo 假数据：Claude 与 Codex 都有账号与用量，底栏配额呈现多 provider。
        return [mkAccount("claude", 62, 38), mkAccount("codex", 45, 22)];
      }
      case "refresh_usage": {
        const a = args as { provider: string };
        if (a.provider === "claude") return mkUsage(62, 38);
        if (a.provider === "codex") return mkUsage(45, 22);
        return { lanes: [], note: null };
      }
      case "get_live_sessions":
        return store.sessions;
      case "rename_session": {
        const a = args as { sessionId: string; title: string };
        const s = store.sessions.find((x) => x.session.cc_session_id === a.sessionId);
        if (s) s.task_title = a.title;
        notify();
        return null;
      }
      case "set_archived": {
        const a = args as { sessionId: number; archived: boolean };
        const s = store.sessions.find((x) => x.session.id === a.sessionId);
        if (s) {
          s.archived = a.archived;
          s.archived_at = a.archived ? Date.now() : null;
        }
        notify();
        return null;
      }
      case "set_session_note": {
        const a = args as { sessionId: string; note: string };
        const s = store.sessions.find((x) => x.session.cc_session_id === a.sessionId);
        if (s) s.note = a.note;
        notify();
        return null;
      }
      case "plugin:event|listen":
        return 1;
      default:
        // focus_session / resume_session / snap_* / plugin:window|* 等一律 no-op
        return null;
    }
  });
}
