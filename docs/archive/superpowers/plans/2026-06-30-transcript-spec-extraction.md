# TranscriptSpec 接口抽取 实施计划（Phase 5，claude-only 架构抽取）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 把 meowo-store 里「假设 transcript 永远是 Claude 格式」的最深耦合（title.rs/analyze.rs）收到一个 `TranscriptSpec` 接口后面，`ClaudeTranscript` 为唯一实现并**逐字节等价**委托现有代码；`Agent::transcript()` 接线，generic 调用点（dispatch apply_title、Tauri 增量缓存）改走接口；codex/kimi 仍返回 None（与现状一致）。完成「全做成接口」目标的最后一块，零行为变更。

**Architecture:** `TranscriptSpec`（定位 + 标题 + 增量解析器工厂）与 `TranscriptParser`（逐行 fold 的增量单元）两个 trait 定义在最底层 crate `meowo-store`（与 `ProviderKey` 同层），meowo-reporter（`Agent::transcript`）与 meowo-app 单向引用、无循环依赖。`ClaudeTranscript` 委托现有 `crate::title::*`；`ClaudeParser` 包装现有未改动的 `analyze::ParseState`。性能热路径 `TranscriptCache` 改为持有 `Box<dyn TranscriptParser>`、`analyze(spec, path)`，逻辑（offset+mtime、半行、LRU、等长重写检测）一字不动。

**Tech Stack:** Rust workspace（meowo-store / meowo-reporter / meowo-app）。

## Global Constraints

- **零行为变更（最高优先）**：claude 路径产出逐字节一致——`ClaudeTranscript` 仅**委托**现有 `title::resolve_transcript_path/resolve_title` 与包装未改的 `ParseState`；`TranscriptCache` 内部失效/增量/半行/LRU 逻辑不得改动，只把 `state: ParseState` 换成 `parser: Box<dyn TranscriptParser>`、把 `analyze(path)` 换成 `analyze(spec, path)`。
- **kimi/codex 等价**：`transcript()` 对它们返回 None → 调用点得 None/`unwrap_or_default()`。这与现状等价（现状对 kimi/codex 的 session_id 在 `~/.claude/projects` 查找本就失败返回 default），且更干净（跳过无谓的 claude 目录扫描）。
- **依赖方向**：`TranscriptSpec`/`TranscriptParser`/`ClaudeTranscript`/`CLAUDE_TRANSCRIPT` 定义在 `meowo-store`。**禁止**放 meowo-reporter（会循环依赖）。`Agent::transcript()` 返回 `Option<&'static dyn meowo_store::TranscriptSpec>`。
- **trait 约束**：`TranscriptParser: Send`（`TranscriptCache` 经 `Arc<Mutex<>>` 在 Tauri 主线程与轮询线程间共享）；`TranscriptSpec: Sync`（以 `&'static dyn` 共享）。
- **保留 `resolves_transcript_title` cap**：它与 `transcript()` 概念不同——前者「是否用 transcript 标题命名卡片」，后者「是否提供解析器」。未来 codex 可能有 spec（做 context%/错误）但标题仍走首条 prompt（`resolves_transcript_title=false`）。`apply_title` 必须**两者都用**：先 gate `resolves_transcript_title`，再用 `transcript()` 提供的 spec 解析标题。
- **不强行全 funnel**：claude-inherent 代码（`import.rs` 扫 `~/.claude`、`agent.rs::write_claude_custom_title`）保持直接调用 `meowo_store::title::*` 自由函数（它们是 ClaudeTranscript 的实现building blocks，本就 claude 专属）。只有 provider 会变化的 generic 调用点（apply_title、Tauri 缓存×2）改走接口。
- `title.rs` 现有自由函数、`analyze::ParseState`/`fold_line`/`to_info`/`CONTEXT_WINDOW`/`analyze_transcript` 全部**保留不动**（= claude 实现）。`TranscriptInfo` 不加字段（它本就无 model，context 在 to_info 内部算 —— 对抗评审的 3 修正点天然满足）。
- 代码英文、注释/commit message 中文。
- 分支：`refactor/transcript-spec-extraction-20260630`（从 feat/kimi-code-cli-adapter-20260626 当前 HEAD 切出）。
- 验证命令：
  - `cargo test -p meowo-store`、`cargo test -p meowo-store -p meowo-reporter`
  - `cargo clippy -p meowo-store -p meowo-reporter --all-targets -- -D warnings`
  - meowo-app 编译（含 sidecar 前置）：`node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`

---

## File Structure

- `crates/meowo-store/src/transcript_spec.rs` —— **新建**：`TranscriptParser` + `TranscriptSpec` traits、`ClaudeTranscript` + `CLAUDE_TRANSCRIPT`（委托 title::/analyze::）。
- `crates/meowo-store/src/analyze.rs` —— 加 `ClaudeParser`(包装 ParseState) + `claude_new_parser()`；`TranscriptCache` 参数化（`CacheEntry.parser: Box<dyn TranscriptParser>`、`analyze(spec, path)`）；更新 cache 测试。`ParseState`/`fold_line`/`to_info`/`analyze_transcript` 不动。
- `crates/meowo-store/src/lib.rs` —— `pub mod transcript_spec;` + 导出。
- `crates/meowo-reporter/src/agent.rs` —— `Agent::transcript()` 默认 None；`ClaudeAgent` 返回 `Some(&meowo_store::CLAUDE_TRANSCRIPT)`。
- `crates/meowo-reporter/src/dispatch.rs` —— `apply_title` 改走 `transcript().resolve_title`（保留 `resolves_transcript_title` gate）。
- `app/src-tauri/src/lib.rs` —— 2 处 `tx_cache...analyze(&path)` 调用点改走 `for_provider(...).transcript()` + `analyze(spec, &path)`。

---

### Task 1: meowo-store 新增 TranscriptSpec/TranscriptParser trait 脚手架（纯增量）

**Files:**
- Create: `crates/meowo-store/src/transcript_spec.rs`
- Modify: `crates/meowo-store/src/analyze.rs`（仅**新增** ClaudeParser + claude_new_parser，不改既有项）
- Modify: `crates/meowo-store/src/lib.rs`（加 mod + 导出）

**Interfaces:**
- Produces:
  - `meowo_store::TranscriptParser`（`Send`；`fold_line(&mut self, &str)`、`to_info(&self) -> TranscriptInfo`）
  - `meowo_store::TranscriptSpec`（`Sync`；`new_parser(&self) -> Box<dyn TranscriptParser>`、`resolve_transcript_path(&self, Option<&str>, Option<&str>, &str) -> Option<PathBuf>`、`resolve_title(&self, Option<&str>, Option<&str>, &str) -> Option<String>`）
  - `meowo_store::ClaudeTranscript`、`static meowo_store::CLAUDE_TRANSCRIPT: ClaudeTranscript`
  - `meowo_store::analyze::claude_new_parser() -> Box<dyn TranscriptParser>`

- [ ] **Step 1: 新建 transcript_spec.rs（先放测试）+ 在 lib.rs 声明 mod**

先在 `crates/meowo-store/src/lib.rs` 的 `pub mod` 区加 `pub mod transcript_spec;`（按字母序，`pub mod title;` 附近）——**必须现在加**，否则新文件不在模块树、其测试不编译，下面的 RED 步骤会变成「0 tests run」而非真正的编译失败。

新建 `crates/meowo-store/src/transcript_spec.rs`，先只放测试模块（实现在 Step 3 补）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn claude_parser_matches_parsestate_full_scan() {
        // ClaudeParser 逐行 fold 的结果须与 analyze_transcript 全量解析逐字段一致。
        let content = concat!(
            r#"{"type":"ai-title","aiTitle":"标题X"}"#, "\n",
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","usage":{"input_tokens":40000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0},"content":[{"type":"text","text":"hi there"}]}}"#, "\n",
        );
        let mut parser = crate::analyze::claude_new_parser();
        for line in content.lines() {
            parser.fold_line(line);
        }
        let p = std::env::temp_dir().join(format!("cc_ts_{}.jsonl", std::process::id()));
        std::fs::write(&p, content).unwrap();
        let full = crate::analyze::analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(parser.to_info(), full);
        assert_eq!(parser.to_info().title.as_deref(), Some("标题X"));
        assert_eq!(parser.to_info().context_tokens, Some(40000));
    }

    #[test]
    fn claude_transcript_resolve_title_delegates() {
        // ClaudeTranscript.resolve_title 须与 title::resolve_title 对同一文件得到相同结果。
        let p = std::env::temp_dir().join(format!("cc_ts_title_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, r#"{{"type":"custom-title","customTitle":"我的标题"}}"#).unwrap();
        drop(f);
        let path = p.to_str().unwrap();
        let via_spec = CLAUDE_TRANSCRIPT.resolve_title(Some(path), None, "sid");
        let via_fn = crate::title::resolve_title(Some(path), None, "sid");
        std::fs::remove_file(&p).ok();
        assert_eq!(via_spec, via_fn);
        assert_eq!(via_spec.as_deref(), Some("我的标题"));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p meowo-store transcript_spec`
Expected: 编译失败（`TranscriptSpec`/`CLAUDE_TRANSCRIPT`/`claude_new_parser` 未定义）。

- [ ] **Step 3: 实现 traits + ClaudeTranscript（transcript_spec.rs）**

在 `crates/meowo-store/src/transcript_spec.rs` 顶部（测试模块**之前**）写：

```rust
//! provider 无关的 transcript 抽象。把「定位 transcript + 标题解析 + 增量分析」收成 trait：
//! claude 由 ClaudeTranscript 实现（委托现有 title/analyze，逐字节等价），codex/kimi 暂无实现
//! （Agent::transcript 返回 None，与现状一致——它们的标题/预览/模型走各自别的路径）。
use crate::analyze::TranscriptInfo;
use std::path::PathBuf;

/// 增量解析单元：逐行 fold、按需产出 TranscriptInfo（对应 analyze 的 ParseState）。
/// Send：TranscriptCache 经 Arc<Mutex<>> 在 Tauri 主线程与后台轮询线程间共享。
pub trait TranscriptParser: Send {
    fn fold_line(&mut self, line: &str);
    fn to_info(&self) -> TranscriptInfo;
}

/// 某 provider 的 transcript 规格：定位文件 + 解析标题 + 产出增量解析器。
/// Sync：以 &'static dyn 共享。
pub trait TranscriptSpec: Sync {
    /// 新建一个该 provider 的增量解析器（供 TranscriptCache 在新建/重置条目时调用）。
    fn new_parser(&self) -> Box<dyn TranscriptParser>;
    /// 定位 transcript 文件（hook 路径 → cwd+id 重建 → 全局查找）。
    fn resolve_transcript_path(&self, transcript_path: Option<&str>, cwd: Option<&str>, session_id: &str) -> Option<PathBuf>;
    /// 解析会话标题（读不到/无标题返回 None）。
    fn resolve_title(&self, transcript_path: Option<&str>, cwd: Option<&str>, session_id: &str) -> Option<String>;
}

/// Claude Code 的 transcript 规格：委托 crate::title / crate::analyze 的现有实现，逐字节等价。
pub struct ClaudeTranscript;

impl TranscriptSpec for ClaudeTranscript {
    fn new_parser(&self) -> Box<dyn TranscriptParser> {
        crate::analyze::claude_new_parser()
    }
    fn resolve_transcript_path(&self, transcript_path: Option<&str>, cwd: Option<&str>, session_id: &str) -> Option<PathBuf> {
        crate::title::resolve_transcript_path(transcript_path, cwd, session_id)
    }
    fn resolve_title(&self, transcript_path: Option<&str>, cwd: Option<&str>, session_id: &str) -> Option<String> {
        crate::title::resolve_title(transcript_path, cwd, session_id)
    }
}

/// 全局唯一 claude transcript 规格实例，供 Agent::transcript() 以 &'static 返回。
pub static CLAUDE_TRANSCRIPT: ClaudeTranscript = ClaudeTranscript;
```

- [ ] **Step 4: 在 analyze.rs 新增 ClaudeParser + claude_new_parser（不改既有项）**

在 `crates/meowo-store/src/analyze.rs` 顶部 `use serde::Serialize;` 下加：

```rust
use crate::transcript_spec::TranscriptParser;
```

在 `ParseState` 的 `impl` 块**之后**（`analyze_transcript` 函数附近）新增（**不要改动** `ParseState`/`fold_line`/`to_info`/`CONTEXT_WINDOW`/`analyze_transcript`/`TranscriptCache` 任何既有代码）：

```rust
/// ClaudeParser：把私有的 ParseState 包成 TranscriptParser trait 对象（逐字节等价，仅转发）。
pub struct ClaudeParser(ParseState);

impl TranscriptParser for ClaudeParser {
    fn fold_line(&mut self, line: &str) {
        self.0.fold_line(line);
    }
    fn to_info(&self) -> TranscriptInfo {
        self.0.to_info()
    }
}

/// 新建一个 claude 增量解析器（ClaudeTranscript::new_parser 委托此函数）。
pub fn claude_new_parser() -> Box<dyn TranscriptParser> {
    Box::new(ClaudeParser(ParseState::default()))
}
```

- [ ] **Step 5: lib.rs 加导出**

（`pub mod transcript_spec;` 已在 Step 1 声明。）在 `crates/meowo-store/src/lib.rs` 的 `pub use` 区加：

```rust
pub use transcript_spec::{ClaudeTranscript, TranscriptParser, TranscriptSpec, CLAUDE_TRANSCRIPT};
```

- [ ] **Step 6: 运行测试确认通过 + clippy**

Run: `cargo test -p meowo-store transcript_spec`
Expected: PASS（2 个委托/等价测试）。

Run: `cargo test -p meowo-store`
Expected: 全绿（既有 analyze/title/store 测试不回归——本任务纯增量）。

Run: `cargo clippy -p meowo-store --all-targets -- -D warnings`
Expected: 无警告。

- [ ] **Step 7: 提交**

```bash
git add crates/meowo-store/src/transcript_spec.rs crates/meowo-store/src/analyze.rs crates/meowo-store/src/lib.rs
git commit -m "feat(transcript): meowo-store 新增 TranscriptSpec/TranscriptParser 接口与 ClaudeTranscript（纯增量）"
```

---

### Task 2: 参数化 TranscriptCache + Agent::transcript() 接线 + 调用点改走接口（原子）

> `TranscriptCache::analyze` 的签名改动会波及 meowo-app 的 2 个调用点，故本任务一次性覆盖 meowo-store/meowo-reporter/meowo-app，提交一次保持全绿。

**Files:**
- Modify: `crates/meowo-store/src/analyze.rs`（TranscriptCache 参数化 + cache 测试）
- Modify: `crates/meowo-reporter/src/agent.rs`（Agent::transcript）
- Modify: `crates/meowo-reporter/src/dispatch.rs`（apply_title）
- Modify: `app/src-tauri/src/lib.rs`（2 处调用点）

**Interfaces:**
- Consumes: Task 1 的 `meowo_store::{TranscriptSpec, TranscriptParser, CLAUDE_TRANSCRIPT}` + `analyze::claude_new_parser`。
- Produces: `TranscriptCache::analyze(&mut self, spec: &dyn TranscriptSpec, path: &str) -> TranscriptInfo`；`Agent::transcript(&self) -> Option<&'static dyn meowo_store::TranscriptSpec>`。

- [ ] **Step 1: analyze.rs —— CacheEntry 持有 Box<dyn TranscriptParser>**

把 `CacheEntry`（约 192-198 行）改为：

```rust
/// 单条缓存：已解析到的字节偏移 + 上次解析时的 mtime + 累积解析器 + 最近使用刻度（淘汰用）。
struct CacheEntry {
    offset: u64,
    mtime: Option<std::time::SystemTime>,
    parser: Box<dyn TranscriptParser>,
    last_used: u64,
}
```

并在 analyze.rs 顶部确保已 `use crate::transcript_spec::{TranscriptParser, TranscriptSpec};`（Task 1 已加 TranscriptParser，这里补 TranscriptSpec）。

- [ ] **Step 2: analyze.rs —— TranscriptCache::analyze 接收 spec、用 parser**

把 `TranscriptCache::analyze`（约 221 行起）整体替换为（**仅** 4 处改动：签名加 `spec`、`or_insert_with` 用 `spec.new_parser()`、重置分支用 `spec.new_parser()`、`entry.state.*` → `entry.parser.*`；失效/增量/半行/LRU/mtime 逻辑一字不动）：

```rust
    /// 增量解析 path：只处理上次偏移之后新追加的「完整行」（末尾未结束的半行留到下次）。
    /// 失效检测用 len + mtime 双重校验：len < 偏移（截断）或 len == 偏移但 mtime 变了
    /// （等长重写）→ 从头重解析。打开/读失败 → 返回当前累积结果。
    /// `spec` 决定新建/重置条目时用哪种 provider 的解析器（claude 即 ClaudeParser）。
    pub fn analyze(&mut self, spec: &dyn TranscriptSpec, path: &str) -> TranscriptInfo {
        use std::io::{Read, Seek, SeekFrom};
        self.tick += 1;
        // 容量上限：插入新 key 前先淘汰最久未访问的条目。
        if !self.entries.contains_key(path) && self.entries.len() >= MAX_CACHE_ENTRIES {
            if let Some(k) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&k);
            }
        }
        let tick = self.tick;
        let entry = self.entries.entry(path.to_string()).or_insert_with(|| CacheEntry {
            offset: 0,
            mtime: None,
            parser: spec.new_parser(),
            last_used: tick,
        });
        entry.last_used = tick;

        let Ok(mut f) = std::fs::File::open(path) else {
            return entry.parser.to_info();
        };
        let (len, mtime) = match f.metadata() {
            Ok(m) => (m.len(), m.modified().ok()),
            Err(_) => return entry.parser.to_info(),
        };
        if len < entry.offset || (len == entry.offset && mtime != entry.mtime) {
            // 被截断，或等长但 mtime 变了（同长度重写）→ 重头解析。
            entry.offset = 0;
            entry.parser = spec.new_parser();
        }
        if len == entry.offset {
            entry.mtime = mtime;
            return entry.parser.to_info(); // 无新增，直接复用
        }
        if f.seek(SeekFrom::Start(entry.offset)).is_err() {
            return entry.parser.to_info();
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return entry.parser.to_info();
        }
        // 只吃到最后一个换行为止，保证按完整行解析；其后半行（writer 可能正写一半）留到下次。
        if let Some(nl) = buf.iter().rposition(|&b| b == b'\n') {
            entry.offset += (nl + 1) as u64;
            let chunk = String::from_utf8_lossy(&buf[..=nl]);
            for line in chunk.lines() {
                entry.parser.fold_line(line);
            }
        }
        entry.mtime = mtime;
        entry.parser.to_info()
    }
```

- [ ] **Step 3: analyze.rs —— 更新 cache 测试调用点**

`analyze.rs` 测试模块里所有 `cache.analyze(p.to_str().unwrap())` 改为 `cache.analyze(&crate::transcript_spec::ClaudeTranscript, p.to_str().unwrap())`。具体：
- `cache_incremental_matches_full_and_picks_up_appends`：3 处 `cache.analyze(...)`（i1/i2/i3）。
- `cache_detects_same_length_rewrite_by_mtime`：2 处 `cache.analyze(...)`。

（`analyze_transcript(...)` 一次性自由函数调用**不变**。）

- [ ] **Step 4: 运行 meowo-store 测试 + clippy**

Run: `cargo test -p meowo-store`
Expected: 全绿（cache 增量测试用 ClaudeTranscript spec，结果与 `analyze_transcript` 全量一致——证明逐字节等价）。

Run: `cargo clippy -p meowo-store --all-targets -- -D warnings`
Expected: 无警告。

- [ ] **Step 5: meowo-reporter agent.rs —— Agent::transcript()**

在 `Agent` trait 定义里（`write_rename` 方法之后）加默认实现：

```rust
    /// 该 agent 的 transcript 规格：提供「定位 + 标题解析 + 增量分析」。claude 返回 ClaudeTranscript；
    /// codex/kimi 暂无（None）——它们的标题走首条 prompt、预览/模型走 stop_outputs，不读 transcript 分析。
    fn transcript(&self) -> Option<&'static dyn meowo_store::TranscriptSpec> {
        None
    }
```

在 `ClaudeAgent` 的 impl 里加：

```rust
    fn transcript(&self) -> Option<&'static dyn meowo_store::TranscriptSpec> {
        Some(&meowo_store::CLAUDE_TRANSCRIPT)
    }
```

（`KimiAgent`/`CodexAgent` 不实现，用默认 None。）

- [ ] **Step 6: meowo-reporter dispatch.rs —— apply_title 改走 transcript()**

把 `apply_title`（约 101-120 行）替换为（保留 `resolves_transcript_title` gate，标题解析改走 spec）：

```rust
fn apply_title(store: &Store, ev: &HookEvent, sid: i64, now_ms: i64, provider: ProviderKey) -> Result<(), StoreError> {
    // 是否由 transcript 解析标题由 agent 决定（claude 是；kimi/codex 否，靠首条 prompt 命名）。
    let agent = crate::agent::for_provider(provider);
    if !agent.resolves_transcript_title() {
        return Ok(());
    }
    // 提供解析器的 transcript 规格（claude=ClaudeTranscript；无则不解析）。
    let Some(spec) = agent.transcript() else {
        return Ok(());
    };
    // cwd 优先用事件携带的，否则回退到 SessionStart 时存进库的 cwd。
    let cwd_owned: Option<String> = match ev.cwd.clone() {
        Some(c) => Some(c),
        None => store.session_cwd(sid).ok().flatten(),
    };
    if let Some(title) = spec.resolve_title(
        ev.transcript_path.as_deref(),
        cwd_owned.as_deref(),
        &ev.session_id,
    ) {
        store.set_session_title(sid, &title, now_ms)?;
    }
    Ok(())
}
```

（这替换了原先的 `crate::transcript::resolve_title(...)` 直调。`crate::transcript` 的 `resolve_title` 重导出若因此变为未用，保持不动即可——pub 重导出不触发 dead_code 警告；import.rs 等仍可能用其它重导出项。）

- [ ] **Step 7: meowo-app lib.rs —— 2 处调用点改走 transcript() + analyze(spec, path)**

把 `live_sessions_blocking` 里的调用点（约 196-202 行）替换为（注释说明本处只按 `transcript()` 门控、下方 `info.title` 覆盖标题不再额外查 `resolves_transcript_title`——当前 codex/kimi `transcript()=None` 故零影响；将来若有 `transcript()=Some` 但 `resolves_transcript_title=false` 的 provider，需在此重审是否仍覆盖标题）：

```rust
        // 注：此处仅按 transcript() 是否存在解析；下方对 info.title 的覆盖不再单独 gate
        // resolves_transcript_title。当前只有 claude 有 spec（且 resolves_transcript_title=true），
        // codex/kimi transcript()=None 不进此分支，故零影响。将来若引入「有 spec 但标题走首条
        // prompt」的 provider，需回到这里与 dispatch::apply_title 一致地按 resolves_transcript_title 门控标题。
        let info = meowo_reporter::agent::for_provider(meowo_store::ProviderKey::parse(Some(&s.provider)))
            .transcript()
            .and_then(|spec| {
                spec.resolve_transcript_path(None, s.cwd.as_deref(), &s.session.cc_session_id)
                    .and_then(|p| p.to_str().map(str::to_string))
                    .map(|path| tx_cache.lock().unwrap_or_else(|e| e.into_inner()).analyze(spec, &path))
            });
```

把 `spawn_liveness_watch` 里的调用点（约 1335-1341 行）替换为（同上：仅按 `transcript()` 门控，title 消费不额外查 `resolves_transcript_title`，当前零影响、未来引入非 claude spec 时须与 apply_title 一致重审）：

```rust
                    // 注：同 live_sessions_blocking，仅按 transcript() 门控；将来若有「有 spec 但标题走首条
                    // prompt」的 provider，需在此与 dispatch::apply_title 一致地按 resolves_transcript_title 门控标题。
                    let meowo_store::TranscriptInfo { title, error, .. } =
                        meowo_reporter::agent::for_provider(meowo_store::ProviderKey::parse(Some(&s.provider)))
                            .transcript()
                            .and_then(|spec| {
                                spec.resolve_transcript_path(None, s.cwd.as_deref(), &sid)
                                    .and_then(|p| p.to_str().map(str::to_string))
                                    .map(|path| {
                                        tx_cache.lock().unwrap_or_else(|e| e.into_inner()).analyze(spec, &path)
                                    })
                            })
                            .unwrap_or_default();
```

- [ ] **Step 8: 全栈编译 + 测试**

Run: `cargo test -p meowo-store -p meowo-reporter`
Expected: 全绿。

Run: `cargo clippy -p meowo-store -p meowo-reporter --all-targets -- -D warnings`
Expected: 无警告。

Run: `node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`
Expected: 编译通过（2 处调用点类型对齐：spec 为 `&'static dyn TranscriptSpec`，传入 `analyze`）。

- [ ] **Step 9: 提交**

```bash
git add crates/meowo-store/src/analyze.rs crates/meowo-reporter/src/agent.rs crates/meowo-reporter/src/dispatch.rs app/src-tauri/src/lib.rs
git commit -m "refactor(transcript): TranscriptCache 参数化 + Agent::transcript 接线，generic 调用点改走接口"
```

---

## Self-Review

**1. 覆盖**：trait 脚手架（Task 1，纯增量）+ 缓存参数化 + 接线（Task 2，原子）。对抗评审 3 修正点天然满足：增量解析单元（TranscriptParser/ClaudeParser 包装 ParseState）、context_window 不进 trait（仍是 analyze 内部 const）、TranscriptInfo 无 model（本就无）。

**2. 零行为变更论证**：ClaudeTranscript 委托现有 `title::resolve_transcript_path/resolve_title`；ClaudeParser 仅转发未改的 `ParseState::fold_line/to_info`；TranscriptCache 失效/增量/半行/LRU 逻辑一字未动，只换 state→parser 类型与 analyze 签名。kimi/codex：transcript()=None → 调用点 None/default，与现状（~/.claude 查找失败返回 default）等价。`resolves_transcript_title` cap 保留（与 transcript() 概念区分，避免未来 codex 误判）。

**3. 占位符扫描**：无 TBD；每步给确切文件、行号区间、完整代码、命令与预期。

**4. 类型一致**：`TranscriptSpec`(Sync)/`TranscriptParser`(Send) 定义于 meowo-store；`Agent::transcript()->Option<&'static dyn meowo_store::TranscriptSpec>`；`TranscriptCache::analyze(spec,path)` 全调用点一致更新（meowo-store 测试×5、meowo-app×2）。

## 本计划之外

- codex/kimi 的 TranscriptSpec 实现（context%/错误 from rollout/wire）—— 待真有需求时按其格式实现，本次只留 None 入口。
- 仍属更大 Phase：macOS resume 参数化 / 账号·UsageScreen 抽象。
