# kimi 工作区信任（workspace-trust）取证与预信任计划

**取证日期**：2026-09-04　**对象**：Kimi Code CLI 0.40.1（`~/.kimi-code/bin/kimi.exe`，bun 单文件打包，内嵌 JS 未压缩）

> 活文档：`trust.rs` 的注释引用本文件。kimi 改算法时以本文的复跑方法重新取证，别凭记忆改。

## 1. 现象

看板上的 claude 会话「切换到 kimi」（`switch_session_provider`）失败：交接文件导出正常、旧进程被杀、
`start_pending` 也确实起了 kimi，但新会话停在 kimi 的「是否信任此目录」确认屏——

- 对话页只渲染结构化 chat，看不到也答不了这道 TUI 提示，用户必须切到「终端」页签手动回车；
- 信任屏期间 kimi 不触发任何 hook → 新会话不落库、接续链不写；
- 而旧会话已被 `end_session` 收尾，看板上表现为「切换后什么都没了」。

新建会话（`new_session_inner`）与 resume 走同一条 PTY 启动路径，症状相同——只是新建时用户更容易自己发现
该去终端页答一下。

## 2. 机制（源码摘录）

`packages/agent-core-v2/src/workspace/workspaceTrust/`，0.40.1 二进制内偏移 ~101007276：

```js
// trustRecord.ts
const TRUST_SCOPE = "workspace-trust";
async function readWorkspaceTrust(docs, root, telemetry) {
  const canonicalKey = trustKey(root);
  if (await docs.get(TRUST_SCOPE, canonicalKey) !== void 0) return true;
  const legacyKey = encodeWorkDirKey(root);            // 旧算法（不折大小写）
  if (legacyKey === canonicalKey) return false;
  const legacy = await docs.get(TRUST_SCOPE, legacyKey);
  if (legacy === void 0) return false;
  try { await docs.set(TRUST_SCOPE, canonicalKey, legacy); await docs.delete(TRUST_SCOPE, legacyKey); } catch {}
  return true;                                          // 命中旧键即迁移到新键
}
function writeWorkspaceTrust(docs, root, trustedAt) {
  return docs.set(TRUST_SCOPE, trustKey(root), { root, trustedAt });
}
function trustKey(root) { return encodeWorkDirKey(canonicalWorkspaceRoot(root)); }

// workspaceTrustService.ts
this.root = workspace.cwd;                              // root 就是 cwd
async initialize() { this.trusted = await readWorkspaceTrust(this.docs, this.root, this.telemetry); }
```

落盘：`<数据目录>/workspace-trust/<key>`，**无扩展名**，内容 `{"root":"<原样路径>","trustedAt":<毫秒>}`。

### 三条决定性质

1. **只看文档是否存在**——`readWorkspaceTrust` 不校验 `root` 字段、不校验 `trustedAt`、无签名。
   所以第三方（meowo）可以像 codex 的 `trusted_hash` 那样**预写**。
2. **不继承父目录**——精确键匹配。信任了 `C:\a` 对 `C:\a\b` 毫无作用。
3. **root 取 cwd**——不向上找 `.git`/项目标记，所以预写只需知道会话的工作目录。

另外 0.40.x 的 `trustKey` 走 canonical（折大小写），而更早版本用 `encodeWorkDirKey(root)`（不折）。
旧记录只在「恰好再次打开该目录」时被迁移，所以**升级 kimi 后历史信任会像被清空一样重新弹提示**
——这正是「以前能切、现在切不了」的直接原因，不是 meowo 的回归。

## 3. key 算法

```
canonical(cwd):
  slashed = cwd 里的 \ 全部换成 /
  shaped  = slashed 匹配 /^(?:[A-Za-z]:[\\/]|\\\\|\/\/)/      # Windows 形状（含 UNC）
  折叠 . 与 ..（纯字符串，不碰文件系统；首段不可越过）
  去掉尾部 /
  shaped ? 整体小写 : 原样                                     # NTFS 查找默认大小写不敏感

slug(name):                                                  # workdir-slug.ts
  小写 → [^a-z0-9._-]+ 替换为 "-" → 去首尾 "-" → 截前 40 → 再去首尾 "-"
  结果为 "" / "." / ".." 时用 "workspace"

key(cwd) = "wd_" + slug(basename(canonical)) + "_" + sha256(canonical) 的十六进制前 12 位
```

`canonicalWorkspaceRoot` 原文用 `path.win32.resolve`；meowo 只会传本机绝对路径，故 Rust 侧改为纯字符串
规范化——**绝不能用 `fs::canonicalize`**，它在 Windows 上返回 `\\?\C:\…` 前缀，哈希会完全对不上。
「POSIX 路径在 Windows 上被 node 补盘符」这类畸形输入不复刻（meowo 不产生）。

### 向量（中性路径，实现的单测用同一批）

| cwd | canonical | key |
|---|---|---|
| `C:\proj\demo` | `c:/proj/demo` | `wd_demo_85b7c7a9d059` |
| `C:\Proj\Demo` | 同上 | 同上 |
| `C:\proj\demo\` | 同上 | 同上 |
| `C:/proj/demo` | 同上 | 同上 |
| `C:\proj\sub\..\demo` | 同上 | 同上 |
| `C:\Users\alice\my.app` | `c:/users/alice/my.app` | `wd_my.app_e93fb0a9fae4` |
| `C:\` | `c:` | `wd_c_826c0d7d3c42` |
| `/home/alice/proj` | `/home/alice/proj`（不折大小写） | `wd_proj_2af6b4a70d44` |
| `C:\proj\` + `a`×45 | 同前缀 | `wd_` + `a`×40 + `_827f2d96c8a3` |
| `C:\proj\我的项目` | `c:/proj/我的项目` | `wd_workspace_bf2803e54aa6` |
| `\\server\share\proj` | `//server/share/proj` | `wd_proj_49ea924784e1` |

算法与 kimi 实际行为一致这一点，另由 `#[ignore]` 真机探针钉住（见第 6 节）——上表只保证我们的实现
不漂移，钉不住 kimi 换代。

### 复跑取证的方法

二进制里 grep 拿不到（151MB 单「行」），用分块 `indexOf` 打印命中处上下文：

```js
const buf = require("node:fs").readFileSync(exePath);
const at = buf.indexOf(Buffer.from("trustedAt", "latin1"));
console.log(buf.subarray(at - 1800, at + 200).toString("latin1").replace(/[^\x20-\x7e]/g, "\n"));
```

## 4. 实现计划

### 步骤 1：`app/src-tauri/crates/meowo-agent/src/trust.rs`（新建）

provider 无关的纯函数 + 一个声明结构：

- `WorkspaceTrustSpec { dir_rel: &'static str }`（kimi 填 `"workspace-trust"`）
- `canonical_root(cwd) -> String`、`slugify_workdir(name) -> String`、`trust_key(cwd) -> String`
- `WorkspaceTrustSpec::record_path(&self, data_dir, cwd) -> PathBuf`
- 写入：已存在则不动（幂等），否则 `fsutil::write_atomic` 写 `{"root":<cwd 原样>,"trustedAt":<now_ms>}`
  - `write_atomic` 的临时名走 `with_extension`，对含 `.` 的 key 会生成形如 `wd_my.tmp.<pid>.<seq>`
    的怪名字，但 rename 目标仍是正确路径，且 `sweep_stale_temp_files` 认得该命名，无需另造一套

### 步骤 2：挂进声明表与能力槽

- `variant.rs`：`Variant` 与 `Installation` 各加 `trust: Option<&'static WorkspaceTrustSpec>`
- `variant.rs`：`Installation::pretrust_workspace(&self, cwd) -> bool`
- `registry.rs`：`AgentPlugin` 在「由变体表派生」区加 `pretrust_workspace(&self, cwd) -> bool`，
  未声明该 spec 的 agent 直接 no-op
- `plugins/kimi/mod.rs`：`modern` 变体声明；`legacy`（Python 版 `~/.kimi`，无此机制）与
  claude / codex / gemini / opencode 一律 `None`
- 前置门槛用 `detect()`（数据目录已存在），与 `wire` 同一条纪律：**绝不凭空创建 agent 的数据目录**

宿主侧不出现任何 agent 身份判断，`host_code_does_not_branch_on_agent_identity` 守卫照旧成立。

### 步骤 3：三个启动落点

均在既有 `spawn_blocking` 闭包内调用（文件 IO 不上主线程，见 `app/src-tauri/CLAUDE.md`）：

| 落点 | 位置 |
|---|---|
| 跨 agent 切换（本次痛点） | `handoff.rs::switch_session_provider`，`start_pending` 之前 |
| 新建会话 | `terminal.rs::new_session_inner`，`start_pending` 之前 |
| 恢复 / 接管重启 | `terminal.rs::resume_session`、`takeover_managed_terminal`，起进程之前 |

**必须按本次实际生效的 profile 解析数据目录**：kimi 的多账号靠 `KIMI_SHARE_DIR` 搬走整个数据目录，
`workspace-trust/` 也跟着搬。写进默认 `~/.kimi-code` 而会话跑在隔离账号上，等于没写。
profile 解析复用 `launch_env_for_profile` 同一条路径（`profile::active_id` → `installation_for_profile`）。

全程 best-effort：解析不到实况、目录不存在、写失败，一律只打日志继续启动——最坏退化成用户手动信任
一次，与现状等价，绝不因为预信任失败挡住会话。

## 5. 安全权衡

预写信任＝**meowo 代表用户批准了「kimi 可在该目录执行」**，把 kimi 的一道确认前置掉了。

判断：可以做。用户在新建会话面板里亲手选定目录、亲手选定 agent 并点启动，语义上已经表达了这份信任；
codex 的 hook `trusted_hash` 预信任是同一性质的既定先例。但边界要守住：

- 只对**用户本次主动发起的启动**预写（新建 / 切换 / 恢复 / 接管），不做批量扫描式预信任；
- 只写 `root` + `trustedAt` 这两个 kimi 自己会写的字段，不夹带任何额外权限开关；
- 不碰 `workspaces.json`（那是 kimi 的工作区名册，与信任判定无关）。

## 6. 验收

1. 单测（`trust.rs`）：第 3 节向量表全覆盖 + 幂等（已存在不重写）+ 归一等价性。
2. `#[ignore]` 真机探针（`app/src-tauri/tests/probe_*.rs` 惯例）：临时目录里跑真实 kimi、手动信任一次，
   读回 `workspace-trust/` 下的文件名与 `trust_key` 的输出逐字符比对——kimi 换算法时靠它发现。
3. 手动验收：取一个从未信任过的目录，对其中的 claude 会话执行「切换到 kimi」，应直达 kimi 主界面，
   看板上出现接续会话卡片（而非停在信任屏、卡片消失）。
4. 门禁（AGENTS.md）：`cargo clippy --workspace --all-targets` 零警告、`cargo test`。

## 7. 本次不做

- **claude 的目录信任**：机制不同（`~/.claude.json` 的 `hasTrustDialogAccepted`），且当前未观察到卡点。
  步骤 2 的 `trust` 槽就是给它和后来者留的位置。
- **前端「该目录未信任」提示**：预写成功后没有可提示的对象；预写失败已退化为 kimi 自己的确认屏。
