import { DOCS_CLAUDE_CODE } from "@/lib/site";
import { InfoIcon } from "@/components/icons";
import { type Lang } from "@/lib/i18n";

const TOC = {
  zh: [
    { id: "how", label: "工作原理" },
    { id: "connect", label: "自动接入" },
    { id: "manual", label: "手动接入 Claude Code" },
    { id: "data", label: "数据与配置" },
  ],
  en: [
    { id: "how", label: "How it works" },
    { id: "connect", label: "Auto-connect" },
    { id: "manual", label: "Manual Claude Code setup" },
    { id: "data", label: "Data & config" },
  ],
};

const HEAD = {
  zh: { eyebrow: "文档", title: "文档", lead: "工作原理、接入方式、数据和配置文件的位置。", toc: "目录" },
  en: { eyebrow: "Docs", title: "Docs", lead: "How it works, how it connects, and where data and config files live.", toc: "Contents" },
};

const FLOW = ` Claude Code 会话
   │  触发 hooks（SessionStart / UserPromptSubmit / PostToolUse / Stop …）
   │  渲染 statusline（取得准确的 Context 百分比）
   ▼
 meowo-reporter（命令行，读 stdin 的事件 JSON）
   │  解析事件、标题、项目、todo、Context 用量
   ▼
 ~/.meowo/board.db（SQLite，WAL）
   ▲
   │  文件监听 + 去抖刷新
 meowo-app（Tauri 贴纸，React 前端）`;

const FLOW_EN = ` Claude Code session
   │  fires hooks (SessionStart / UserPromptSubmit / PostToolUse / Stop …)
   │  renders the statusline (for an accurate Context %)
   ▼
 meowo-reporter (CLI, reads event JSON from stdin)
   │  parses events, title, project, todos, Context usage
   ▼
 ~/.meowo/board.db (SQLite, WAL)
   ▲
   │  file watch + debounced refresh
 meowo-app (Tauri sticker, React front end)`;

function EnDocs() {
  return (
    <article className="prose">
      <h2 id="how">How it works</h2>
      <p>Using Claude Code as the example; Codex / Kimi / Gemini CLI use their own CLI hook mechanisms (OpenCode uses a bridge plugin instead), and the data all lands in the same local database.</p>
      <div className="codeblock"><pre>{FLOW_EN}</pre></div>
      <ul>
        <li><strong>meowo-reporter</strong> is a one-shot process with no state. It starts on each hook, reads the event, writes the DB, and exits — never blocking the session.</li>
        <li><strong>meowo-app</strong> watches <code className="inline">~/.meowo/</code> on startup and refreshes the UI whenever the DB changes. It also runs two background tasks: marking idle sessions, and importing history on first launch.</li>
        <li>The two sides talk only through this SQLite file and don't depend on each other at runtime.</li>
      </ul>

      <h2 id="connect">Auto-connect AI coding CLIs</h2>
      <p>Meowo receives session events through each AI coding CLI's hooks. On startup it detects the tools on your machine and idempotently wires <code className="inline">meowo-reporter</code> into their config.</p>
      <div className="callout">
        <span className="ci"><InfoIcon /></span>
        <span><strong>With the installer you usually don't need to do anything manually.</strong> Meowo backs up config before writing, keeps your existing hooks, and won't re-edit when things are already correct. For Claude Code it fills in the required hook events in <code className="inline">~/.claude/settings.json</code> and wraps the statusLine into <code className="inline">~/.meowo/statusline.sh</code> for an accurate Context %. Writes are atomic; the CLI must have run or logged in once to create its config directory.</span>
      </div>
      <p>Once connected, new sessions show up in the window automatically. For Claude Code itself, see the <a href={DOCS_CLAUDE_CODE} target="_blank" rel="noopener noreferrer">official docs</a>.</p>

      <h2 id="manual">Manual Claude Code setup (optional)</h2>
      <p>If you don't want to launch the app first, or need to write to a custom settings path, you can hook it up by hand:</p>
      <div className="codeblock"><pre>{`# 1. Build meowo-reporter (the Rust workspace root is app/src-tauri)
cd app/src-tauri && cargo build --release -p meowo-reporter
# output: app/src-tauri/target/release/meowo-reporter(.exe)

# 2. Wire into ~/.claude/settings.json hooks (use an absolute path)
bun scripts/install-hooks.mjs "<repo-abs-path>/app/src-tauri/target/release/meowo-reporter.exe"`}</pre></div>
      <p>The script attaches the reporter to the hook events it needs (SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd / PermissionRequest, plus PreToolUse's AskUserQuestion / ExitPlanMode; normal hooks get a 5s timeout, PermissionRequest 310s so it can wait for your approval). Running it again with the same path won't duplicate entries or break your existing hooks.</p>
      <div className="callout">
        <span className="ci"><InfoIcon /></span>
        <span>This script only handles Claude Code. codex, kimi, and gemini connect through their own native hook config (the hook command carries <code className="inline">--provider codex|kimi|gemini</code>); opencode has no hook mechanism, so Meowo generates a bridge plugin under <code className="inline">~/.config/opencode/plugin/</code> instead. None of them go through this script.</span>
      </div>

      <h2 id="data">Data & config</h2>
      <ul>
        <li><strong>Database</strong>: <code className="inline">~/.meowo/board.db</code> (SQLite, WAL). Override the path with <code className="inline">MEOWO_DB</code>.</li>
        <li><strong>App settings</strong>: <code className="inline">~/.meowo/settings.json</code> — notification toggle, theme, opacity, UI density, archive auto-hide days, and which terminal resumes sessions all live here.</li>
        <li><strong>Usage cache</strong>: <code className="inline">~/.meowo/usage-cache.json</code>.</li>
        <li><strong>statusLine wrapper</strong>: <code className="inline">~/.meowo/statusline.sh</code>. Generated and maintained by the app; no need to edit.</li>
        <li><strong>First-import marker</strong>: <code className="inline">~/.meowo/imported.json</code>. Delete it to re-import recent history on the next launch.</li>
      </ul>
    </article>
  );
}

function ZhDocs() {
  return (
    <article className="prose">
      <h2 id="how">工作原理</h2>
      <p>以 Claude Code 为例；Codex / Kimi / Gemini CLI 走各自 CLI 的 hook 机制（OpenCode 走桥接插件），数据最终都落到同一份本地数据库。</p>
      <div className="codeblock"><pre>{FLOW}</pre></div>
      <ul>
        <li><strong>meowo-reporter</strong> 是一次性进程，不保存状态。每次触发 hook 时启动，读取事件、写库，然后退出，不会阻塞会话。</li>
        <li><strong>meowo-app</strong> 启动时监听 <code className="inline">~/.meowo/</code> 目录的变化，库一变就刷新界面。它还跑两个后台任务：标记空闲会话，以及首次启动时导入历史会话。</li>
        <li>两端只通过这个 SQLite 文件通信，运行时不互相依赖。</li>
      </ul>

      <h2 id="connect">自动接入 AI 编程 CLI</h2>
      <p>Meowo 通过各个 AI 编程 CLI 的 hooks 接收会话事件。应用启动时会检测本机已有的工具，并将 <code className="inline">meowo-reporter</code> 幂等接入对应配置。</p>
      <div className="callout">
        <span className="ci"><InfoIcon /></span>
        <span><strong>用安装包时一般不需要手动操作。</strong> Meowo 写入前会备份配置，保留已有 hooks，配置已经正确时不会重复修改。以 Claude Code 为例，它会补齐 <code className="inline">~/.claude/settings.json</code> 中所需的 hook 事件，并把 statusLine 包装成 <code className="inline">~/.meowo/statusline.sh</code>，以便拿到准确的 Context 百分比。写入是原子的；前提是相应 CLI 已完成首次运行或登录并生成配置目录。</span>
      </div>
      <p>接入后，新会话会自动出现在窗口里。Claude Code 本身见 <a href={DOCS_CLAUDE_CODE} target="_blank" rel="noopener noreferrer">官方文档</a>。</p>

      <h2 id="manual">手动接入 Claude Code（可选）</h2>
      <p>不想先启动 app，或者要写入自定义的 settings 路径时，可以手动挂：</p>
      <div className="codeblock"><pre>{`# 1. 编译 meowo-reporter（Rust workspace 根在 app/src-tauri）
cd app/src-tauri && cargo build --release -p meowo-reporter
# 产物：app/src-tauri/target/release/meowo-reporter(.exe)

# 2. 接入 ~/.claude/settings.json 的 hooks（用绝对路径）
bun scripts/install-hooks.mjs "<仓库绝对路径>/app/src-tauri/target/release/meowo-reporter.exe"`}</pre></div>
      <p>脚本会把 reporter 挂到需要的 hook 事件上（SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd / PermissionRequest，以及 PreToolUse 的 AskUserQuestion / ExitPlanMode；普通 hook 带 5s 超时，PermissionRequest 为 310s——要等你审批）。用同一个路径重复运行不会重复追加，也不会破坏已有的 hooks。</p>
      <div className="callout">
        <span className="ci"><InfoIcon /></span>
        <span>这个脚本只管 Claude Code。codex、kimi、gemini 的接入走各自 CLI 的原生 hook 配置（hook 命令带 <code className="inline">--provider codex|kimi|gemini</code>）；opencode 没有 hook 机制，Meowo 改为在 <code className="inline">~/.config/opencode/plugin/</code> 下生成桥接插件。它们都不经过这个脚本。</span>
      </div>

      <h2 id="data">数据与配置</h2>
      <ul>
        <li><strong>数据库</strong>：<code className="inline">~/.meowo/board.db</code>（SQLite，WAL）。可用 <code className="inline">MEOWO_DB</code> 覆盖路径。</li>
        <li><strong>应用设置</strong>：<code className="inline">~/.meowo/settings.json</code>。通知开关、主题、不透明度、界面密度、归档自动隐藏天数、用哪个终端恢复会话，都在这里。</li>
        <li><strong>用量缓存</strong>：<code className="inline">~/.meowo/usage-cache.json</code>。</li>
        <li><strong>statusLine 包装脚本</strong>：<code className="inline">~/.meowo/statusline.sh</code>。由 app 生成和维护，不用手改。</li>
        <li><strong>首次导入标记</strong>：<code className="inline">~/.meowo/imported.json</code>。删掉它，下次启动会重新导入最近的历史会话。</li>
      </ul>
    </article>
  );
}

export default function DocsContent({ lang }: { lang: Lang }) {
  const head = HEAD[lang];
  const toc = TOC[lang];
  return (
    <main>
      <section className="pagehead">
        <div className="container">
          <span className="eyebrow">{head.eyebrow}</span>
          <h1 className="h1">{head.title}</h1>
          <p className="lead">{head.lead}</p>
        </div>
      </section>

      <section className="section-sm">
        <div className="container">
          <div className="doc-layout">
            <nav className="doc-toc" aria-label={head.toc}>
              <div className="toc-title">{head.toc}</div>
              {toc.map((t) => (
                <a key={t.id} href={`#${t.id}`}>{t.label}</a>
              ))}
            </nav>
            {lang === "en" ? <EnDocs /> : <ZhDocs />}
          </div>
        </div>
      </section>
    </main>
  );
}
