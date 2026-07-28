//! Claude Code 的**会话运行形态**索引：`<CLAUDE_CONFIG_DIR>/sessions/<pid>.json`。
//!
//! Claude Code 2.1.x 的 FleetView 允许把当前对话移到后台（"Your conversation moved to the
//! background"），也允许直接从那里派出一个全新会话。这些后台会话有自己的 session id 与
//! transcript，也照常触发 SessionStart hook——于是 meowo 会给它们建卡，看板上凭空多出一张
//! 用户没开过的卡片。
//!
//! 建卡本身没错，错在它像一张普通卡片：它既不在用户的终端窗口里，也不在 meowo 的托管 PTY 里，
//! 而是由 claude 自己的 daemon supervisor 拉起（`~/.claude/jobs/<job>/state.json` 记着
//! `backend: "daemon"` 与 `respawnFlags`）。meowo 的「接管」是**杀掉进程 + `--resume` 重开**，
//! 对它用等于跟 supervisor 抢进程：杀完就被按 respawnFlags 拉回来，FleetView 里的作业状态还丢了。
//! 所以这类会话必须先能被认出来，界面才好据此收起接管/结束这些注定失败的按钮。
//!
//! 认出来的钥匙是 claude 自己落盘的进程索引，每个 claude 进程一份：
//!
//! ```json
//! {"pid":17196,"sessionId":"18aed26e-…","cwd":"C:\\Users\\35122","kind":"bg",
//!  "jobId":"18aed26e","status":"idle","updatedAt":1785205060373}
//! ```
//!
//! `kind` 是 `"interactive"`（用户终端里那个）或 `"bg"`（daemon 托管的后台会话）。
//!
//! # 后台会话的三种出身
//!
//! 光认出「这是后台会话」还不够。进一次 agents 模式，claude 一口气能造出三种东西，
//! 它们都有独立 session id、都触发 hook、在看板上都是一张卡——用户看到的是凭空多出
//! 几张长得差不多的卡片，其中两张还和已有的那张内容一模一样。花名册的
//! `dispatch.source` 说清了各自是什么：
//!
//! - `"slash"` + `launch.fork: true`：把**当前这段对话**转入后台。历史是从源会话复制的，
//!   连终端画面都和源会话逐字相同。持久侧的记号是 `state.json` 的 `interactiveLineage`。
//! - `"spare"`：**预热**的待命进程，还没派活，`seed.intent` 是空的。它的作业目录常常连
//!   `state.json` 都还没写。看板上就是那张永远没内容的「(未命名会话)」。
//! - `"fleet"` / 其余：正常派出的新任务，是真的独立会话。
//!
//! 宿主拿这些出身做的事是：后台会话一律不建卡，能查到源的折叠成源会话卡上的一个数
//! （见 meowo-app 的 `hidden_background`）。代价是查不到源的作业就此不可见——包括
//! 转入后台后正在干活的那些（`state: "working"`）。这是明确的取舍：这类会话在 meowo 里
//! 几乎操作不了（键盘输入无效、发消息在手动模式下不执行），一张普通样子的卡等于承诺了
//! 一堆做不到的事。

use crate::caps::{BackgroundControl, BackgroundEndpoint, RuntimeCap, SessionRuntime};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct ClaudeRuntime;

/// 全局唯一实例，供插件的 runtime 能力槽以 `&'static` 返回。
pub static CLAUDE_RUNTIME: ClaudeRuntime = ClaudeRuntime;

impl RuntimeCap for ClaudeRuntime {
    fn session_runtimes(&self) -> HashMap<String, SessionRuntime> {
        index_sessions(&super::transcript::config_dirs())
    }

    fn background_endpoint(&self, session_id: &str) -> Option<BackgroundEndpoint> {
        super::transcript::config_dirs()
            .iter()
            .find_map(|dir| endpoint_in_roster(&dir.join("daemon"), session_id))
    }
}

/// 从 `daemon/roster.json` 里挑出这个会话的 worker。
///
/// 花名册是 supervisor 的**活**记录：worker 退出后条目可能短暂残留，但那条 socket 已经没了，
/// 连接自然会失败——所以这里不做存活判断，把「连得上吗」留给真正去连的人回答，
/// 免得多一处会过期的判断。
fn endpoint_in_roster(daemon: &Path, session_id: &str) -> Option<BackgroundEndpoint> {
    let text = std::fs::read_to_string(daemon.join("roster.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("workers")?
        .as_object()?
        .iter()
        .find(|(_, worker)| worker.get("sessionId").and_then(|v| v.as_str()) == Some(session_id))
        .and_then(|(short, worker)| {
            Some(BackgroundEndpoint {
                sock: worker.get("ptySock")?.as_str()?.to_string(),
                auth: worker
                    .get("ptyAuth")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                pid: worker.get("pid").and_then(serde_json::Value::as_i64),
                control: control_channel(daemon, short),
            })
        })
}

/// supervisor 的控制通道。管道名里那截随机串在 `daemon/pipe.key`，令牌在 `daemon/control.key`
/// ——两个文件缺一条这条路就走不通（守护进程没起来时它们本就不存在）。
///
/// 非 Windows 平台上 claude 用 unix socket，路径同样由 pipe.key 拼出；这里保持同一套拼法。
fn control_channel(daemon: &Path, short: &str) -> Option<BackgroundControl> {
    let key = std::fs::read_to_string(daemon.join("pipe.key")).ok()?;
    let auth = std::fs::read_to_string(daemon.join("control.key")).ok()?;
    let key = key.trim();
    let auth = auth.trim();
    if key.is_empty() || auth.is_empty() {
        return None;
    }
    let sock = if cfg!(windows) {
        format!(r"\\.\pipe\cc-daemon-{key}-control")
    } else {
        daemon
            .join(format!("cc-daemon-{key}-control"))
            .to_str()?
            .to_string()
    };
    Some(BackgroundControl {
        sock,
        auth: auth.to_string(),
        job: short.to_string(),
    })
}

/// claude 写的一份进程索引。只取判定运行形态要用的字段；其余（cwd/name/status/procStart…）
/// 各有更可靠的来源（DB、进程表），不在这里重复一套。
#[derive(serde::Deserialize)]
struct SessionFile {
    #[serde(rename = "sessionId")]
    session_id: String,
    /// `"interactive"` | `"bg"`。缺失（旧版 CLI）时按非后台处理——宁可少标一个徽章，
    /// 也不要把用户自己开的会话误标成后台的而藏掉接管按钮。
    #[serde(default)]
    kind: Option<String>,
    #[serde(rename = "jobId", default)]
    job_id: Option<String>,
    /// 同一个 session id 可能有多份索引（后台会话被 `--resume` 拉回前台会以新 pid 再写一份），
    /// 取最新的那份。缺失按 0 处理，让任何带时间戳的记录都能盖过它。
    #[serde(rename = "updatedAt", default)]
    updated_at: i64,
}

/// 建「session id → 运行形态」索引。两个来源，缺一不可：
///
/// - `jobs/<short>/state.json`：**持久**记录，作业结束后仍在。打底。
/// - `sessions/<pid>.json`：进程活着时才有，`kind` 更准（后台会话被 `--resume` 拉回前台
///   时它会写成 `interactive`）。覆盖前者。
///
/// 曾经只扫 `sessions/`，以为「进程死后索引文件会残留」——实测**会被删掉**。后果是会话
/// 一结束就退化成普通卡片，界面把「在 Meowo 中接管」给了它，而这类会话的正文常常没落盘
/// （claude 的 fork/resume worker 只写两行元数据），`--resume` 只能得到
/// `No conversation found` 加一个退出码 1。出身必须由持久记录说了算。
fn index_sessions(config_dirs: &[PathBuf]) -> HashMap<String, SessionRuntime> {
    let mut jobs: HashMap<String, JobRecord> = HashMap::new();
    for dir in config_dirs {
        for (session_id, job) in read_jobs(&dir.join("jobs")) {
            jobs.insert(session_id, job);
        }
    }
    // 打底：作业目录里记着的一律是后台会话，时间戳给 0，任何 sessions 索引都能盖过它。
    let mut newest: HashMap<String, (i64, Shape)> = jobs
        .iter()
        .map(|(id, job)| {
            let shape = Shape {
                background: true,
                job_id: Some(job.short.clone()),
            };
            (id.clone(), (0, shape))
        })
        .collect();
    for dir in config_dirs {
        for file in read_session_files(&dir.join("sessions")) {
            let shape = Shape {
                background: file.kind.as_deref() == Some("bg"),
                job_id: file.job_id,
            };
            match newest.get(&file.session_id) {
                // `>=` 而非 `>`：同一毫秒的两份索引里后扫到的胜出，至少是确定性的。
                Some((seen, _)) if *seen > file.updated_at => {}
                _ => {
                    newest.insert(file.session_id, (file.updated_at, shape));
                }
            }
        }
    }
    // 出身与作业状态另外合并进来：它们只有作业目录和花名册知道，而上面那轮以
    // `sessions/<pid>.json` 为准的覆盖会把打底记录整个换掉——不单独合，卡片就会
    // 连「这是从哪来的」都答不上来。
    let spares = spare_sessions(config_dirs);
    let parents = fork_parents(config_dirs);
    newest
        .into_iter()
        .map(|(id, (_, shape))| {
            let job = jobs.get(&id);
            let runtime = SessionRuntime {
                background: shape.background,
                job_id: shape.job_id.or_else(|| job.map(|j| j.short.clone())),
                job_state: job.and_then(|j| j.state.clone()),
                from_interactive: job.is_some_and(|j| j.interactive_lineage),
                spare: spares.contains(&id),
                forked_from: parents.get(&id).cloned(),
            };
            (id, runtime)
        })
        .collect()
}

/// 由 `sessions/<pid>.json` 与作业目录共同定夺的那部分：是不是后台会话、属于哪个作业。
struct Shape {
    background: bool,
    job_id: Option<String>,
}

/// 作业目录里那份**持久**记录。worker 退出后它还在，所以卡片的出身由它说了算。
struct JobRecord {
    /// 作业短码，也就是目录名。
    short: String,
    /// claude 自己在 FleetView 里显示的作业状态：`done` / `working` / `blocked`。
    state: Option<String>,
    /// 这个作业是用户那段交互式对话转入后台来的。
    interactive_lineage: bool,
}

/// 扫 `jobs/<short>/state.json`，取出 `(session id, 作业记录)`。
///
/// 只认 `sessionId`——`resumeSessionId` 在 fork 场景下指向的是**源**会话，拿它当键会把
/// 用户自己那条交互式会话误标成后台的，接管入口就此消失。
fn read_jobs(dir: &Path) -> Vec<(String, JobRecord)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let short = entry.file_name().to_str()?.to_string();
            let text = std::fs::read_to_string(entry.path().join("state.json")).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            let session_id = value.get("sessionId")?.as_str()?.to_string();
            let record = JobRecord {
                short,
                state: value
                    .get("state")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                interactive_lineage: value
                    .get("interactiveLineage")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            };
            Some((session_id, record))
        })
        .collect()
}

/// 花名册里还在待命、尚未派活的预热 worker 的 session id。
///
/// 只有花名册知道这件事（`dispatch.source`），而这正合适：预热 worker 一旦退出就没有
/// 「它曾经待命过」这回事可标了，条目跟着消失反倒是对的。
fn spare_sessions(config_dirs: &[PathBuf]) -> std::collections::HashSet<String> {
    live_workers(config_dirs)
        .into_iter()
        .filter(|worker| {
            worker
                .pointer("/dispatch/source")
                .and_then(|v| v.as_str())
                .is_some_and(|source| source == "spare")
        })
        .filter_map(|worker| Some(worker.get("sessionId")?.as_str()?.to_string()))
        .collect()
}

/// 「作业 session id → 它 fork 自哪个会话」。
///
/// `dispatch.launch.sessionId` 在 fork 场景下是**源 transcript 的路径**，取文件名去掉扩展名
/// 就是源会话 id。只认 `fork: true`：resume 自己（重启同一个作业）时这个字段填的是它自己，
/// 当成父子关系会让作业变成自己的父亲。
fn fork_parents(config_dirs: &[PathBuf]) -> HashMap<String, String> {
    live_workers(config_dirs)
        .into_iter()
        .filter(|worker| {
            worker
                .pointer("/dispatch/launch/fork")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|worker| {
            let child = worker.get("sessionId")?.as_str()?.to_string();
            let source = worker.pointer("/dispatch/launch/sessionId")?.as_str()?;
            let parent = Path::new(source).file_stem()?.to_str()?.to_string();
            (parent != child).then_some((child, parent))
        })
        .collect()
}

/// 花名册里所有 worker 条目。花名册是 supervisor 的**活**记录，worker 退出后条目就没了。
fn live_workers(config_dirs: &[PathBuf]) -> Vec<serde_json::Value> {
    config_dirs
        .iter()
        .filter_map(|dir| std::fs::read_to_string(dir.join("daemon").join("roster.json")).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter_map(|value| value.get("workers")?.as_object().cloned())
        .flat_map(|workers| workers.into_values())
        .collect()
}

/// 目录不存在（没装 claude / 旧版 CLI 不写这个目录）返回空；单个文件坏了只跳过它自己。
fn read_session_files(dir: &Path) -> Vec<SessionFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        // 每份索引只有几行，整读无妨。
        .filter_map(|path| std::fs::read_to_string(&path).ok())
        .filter_map(|text| serde_json::from_str::<SessionFile>(&text).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试一个独立的 CLAUDE_CONFIG_DIR，避免并发串扰（与 mod.rs 的 home 夹具同法）。
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meowo-fleet-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) {
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join(name), body).unwrap();
    }

    #[test]
    fn background_sessions_are_told_apart_from_interactive_ones() {
        let dir = &scratch("kinds");
        write(
            dir,
            "17196.json",
            r#"{"pid":17196,"sessionId":"bg-one","kind":"bg","jobId":"18aed26e","updatedAt":10}"#,
        );
        write(
            dir,
            "40276.json",
            r#"{"pid":40276,"sessionId":"mine","kind":"interactive","updatedAt":10}"#,
        );
        let index = index_sessions(std::slice::from_ref(dir));

        let bg = index.get("bg-one").expect("后台会话应在索引里");
        assert!(bg.background);
        assert_eq!(bg.job_id.as_deref(), Some("18aed26e"));
        assert!(!index["mine"].background, "用户自己开的会话不是后台会话");
    }

    /// 后台会话被 `--resume` 拉回前台后会以新 pid 再写一份索引，两份文件同一个 session id。
    /// 取 updatedAt 最新的那份——否则卡片会一直挂着「后台」徽章，接管按钮永久藏着。
    #[test]
    fn the_newest_index_wins_when_one_session_has_several() {
        let dir = &scratch("newest");
        write(
            dir,
            "100.json",
            r#"{"pid":100,"sessionId":"s","kind":"bg","jobId":"j","updatedAt":10}"#,
        );
        write(
            dir,
            "200.json",
            r#"{"pid":200,"sessionId":"s","kind":"interactive","updatedAt":20}"#,
        );
        assert!(!index_sessions(std::slice::from_ref(dir))["s"].background);
    }

    /// kind 缺失（旧版 CLI）按非后台处理：宁可少标一个徽章，也不要藏掉真能用的接管按钮。
    #[test]
    fn a_missing_kind_is_not_treated_as_background() {
        let dir = &scratch("nokind");
        write(
            dir,
            "1.json",
            r#"{"pid":1,"sessionId":"old","updatedAt":1}"#,
        );
        assert!(!index_sessions(std::slice::from_ref(dir))["old"].background);
    }

    /// 花名册给的是「怎么连上这个后台会话」——sock 与输入令牌。查得到才谈得上接管。
    #[test]
    fn the_roster_yields_the_socket_and_token_for_a_background_session() {
        let dir = scratch("roster");
        let daemon = dir.join("daemon");
        std::fs::create_dir_all(&daemon).unwrap();
        std::fs::write(
            daemon.join("roster.json"),
            r#"{"proto":1,"workers":{
                "aaa":{"pid":11,"sessionId":"other","ptySock":"\\\\.\\pipe\\a","ptyAuth":"tok-a"},
                "bbb":{"pid":22,"sessionId":"mine","ptySock":"\\\\.\\pipe\\b","ptyAuth":"tok-b"}}}"#,
        )
        .unwrap();

        let found = endpoint_in_roster(&daemon, "mine").expect("花名册里有这个会话");
        assert_eq!(found.sock, r"\\.\pipe\b");
        assert_eq!(found.auth.as_deref(), Some("tok-b"));
        assert_eq!(found.pid, Some(22));

        assert!(endpoint_in_roster(&daemon, "nobody").is_none());
        assert!(endpoint_in_roster(&daemon.join("nope"), "mine").is_none());
    }

    /// 没有 ptySock 的条目（worker 刚登记、socket 还没建）不能当成可接管——
    /// 返回半个 endpoint 只会让调用方拿着空路径去连。
    #[test]
    fn an_entry_without_a_socket_is_not_an_endpoint() {
        let dir = scratch("roster-partial");
        std::fs::create_dir_all(dir.join("daemon")).unwrap();
        let daemon = dir.join("daemon");
        std::fs::write(
            daemon.join("roster.json"),
            r#"{"workers":{"a":{"sessionId":"mine","pid":1}}}"#,
        )
        .unwrap();
        assert!(endpoint_in_roster(&daemon, "mine").is_none());
    }

    /// 作业结束后  会被 claude 删掉，而  留着。
    /// 出身必须由持久记录说了算——否则会话一结束就退化成普通卡片，界面会给出那个
    /// 必然失败的「在 Meowo 中接管」（正文没落盘， 找不到会话）。
    #[test]
    fn a_finished_background_session_is_still_known_by_its_job_dir() {
        let dir = &scratch("jobs");
        let job = dir.join("jobs").join("f64c2382");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            r#"{"sessionId":"gone","state":"done","name":"x"}"#,
        )
        .unwrap();
        // sessions/ 里已经没有它了（进程退出，索引被删）。
        write(dir, "1.json", r#"{"pid":1,"sessionId":"other","kind":"interactive"}"#);

        let index = index_sessions(std::slice::from_ref(dir));
        let gone = index.get("gone").expect("作业目录还在，就仍算后台会话");
        assert!(gone.background);
        assert_eq!(gone.job_id.as_deref(), Some("f64c2382"));
        assert!(!index["other"].background);
    }

    /// 后台会话被  拉回前台后， 说了算：它现在是交互式的，
    /// 接管入口该回来。job 目录还在也不能把它按回后台。
    #[test]
    fn a_live_session_index_overrides_the_job_record() {
        let dir = &scratch("jobs-override");
        let job = dir.join("jobs").join("abc");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(job.join("state.json"), r#"{"sessionId":"s"}"#).unwrap();
        write(dir, "9.json", r#"{"pid":9,"sessionId":"s","kind":"interactive","updatedAt":5}"#);

        assert!(!index_sessions(std::slice::from_ref(dir))["s"].background);
    }

    /// 进 agents 模式把当前对话转入后台，得到的是一份 fork：新 session id、新卡片，内容
    /// 却和源会话逐字相同。卡片必须能说出它的来历与真实作业状态，否则用户只看到看板上
    /// 凭空多出一张和已有卡片一模一样的卡（正是这个功能最初被报上来的样子）。
    #[test]
    fn a_conversation_moved_to_the_background_is_marked_as_such() {
        let dir = &scratch("lineage");
        let job = dir.join("jobs").join("38243981");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            r#"{"sessionId":"forked","state":"working","interactiveLineage":true}"#,
        )
        .unwrap();

        let forked = &index_sessions(std::slice::from_ref(dir))["forked"];
        assert!(forked.from_interactive, "它是从终端那段对话转入后台的");
        assert_eq!(forked.job_state.as_deref(), Some("working"));
        assert!(!forked.spare);
    }

    /// 出身与作业状态只有作业目录知道，而 `sessions/<pid>.json` 会把打底记录整个覆盖掉。
    /// 覆盖之后仍要留着这两样——否则一个活着的后台会话反而比结束了的知道得更少。
    #[test]
    fn the_job_record_still_supplies_the_origin_after_a_live_index_wins() {
        let dir = &scratch("lineage-override");
        let job = dir.join("jobs").join("abc");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            r#"{"sessionId":"s","state":"blocked","interactiveLineage":true}"#,
        )
        .unwrap();
        write(dir, "9.json", r#"{"pid":9,"sessionId":"s","kind":"bg","updatedAt":5}"#);

        let runtime = &index_sessions(std::slice::from_ref(dir))["s"];
        assert!(runtime.background);
        assert_eq!(runtime.job_state.as_deref(), Some("blocked"));
        assert!(runtime.from_interactive);
        assert_eq!(runtime.job_id.as_deref(), Some("abc"));
    }

    /// 进 agents 模式时 claude 会先备一个待命进程。它照样有 session id、照样触发 hook，
    /// 于是看板上多出一张永远没内容的「(未命名会话)」——得让界面认得出这是什么。
    #[test]
    fn a_prewarmed_worker_is_told_apart_from_a_real_job() {
        let dir = &scratch("spare");
        let daemon = dir.join("daemon");
        std::fs::create_dir_all(&daemon).unwrap();
        std::fs::write(
            daemon.join("roster.json"),
            r#"{"workers":{
                "b282bca7":{"sessionId":"idle-one","dispatch":{"source":"spare"}},
                "f64c2382":{"sessionId":"real-one","dispatch":{"source":"fleet"}}}}"#,
        )
        .unwrap();
        write(dir, "1.json", r#"{"pid":1,"sessionId":"idle-one","kind":"bg"}"#);
        write(dir, "2.json", r#"{"pid":2,"sessionId":"real-one","kind":"bg"}"#);

        let index = index_sessions(std::slice::from_ref(dir));
        assert!(index["idle-one"].spare, "待命进程还没被派活");
        assert!(!index["real-one"].spare, "这个是真派出去的任务");
    }

    /// 后台会话不单独占卡片，而是折叠到源会话卡上标个数——前提是查得出源。fork 型作业的
    /// 源在花名册的 `dispatch.launch.sessionId`（一条 transcript 路径）里。
    #[test]
    fn a_forked_job_points_back_at_the_session_it_came_from() {
        let dir = &scratch("parents");
        let daemon = dir.join("daemon");
        std::fs::create_dir_all(&daemon).unwrap();
        std::fs::write(
            daemon.join("roster.json"),
            r#"{"workers":{
                "38243981":{"sessionId":"child","dispatch":{"launch":{
                    "mode":"resume","fork":true,
                    "sessionId":"C:\\Users\\x\\.claude\\projects\\p\\parent-one.jsonl"}}},
                "64f7170e":{"sessionId":"self","dispatch":{"launch":{
                    "mode":"resume","fork":false,"sessionId":"self.jsonl"}}}}}"#,
        )
        .unwrap();
        write(dir, "1.json", r#"{"pid":1,"sessionId":"child","kind":"bg"}"#);
        write(dir, "2.json", r#"{"pid":2,"sessionId":"self","kind":"bg"}"#);

        let index = index_sessions(std::slice::from_ref(dir));
        assert_eq!(index["child"].forked_from.as_deref(), Some("parent-one"));
        // resume 自己不是 fork：认了的话作业会变成自己的父亲，卡片上凭空多出一个计数。
        assert_eq!(index["self"].forked_from, None);
    }

    #[test]
    fn broken_files_and_missing_dirs_do_not_sink_the_whole_index() {
        let dir = &scratch("broken");
        write(dir, "bad.json", "{ not json");
        write(dir, "notes.txt", "ignored");
        write(dir, "ok.json", r#"{"sessionId":"good","kind":"bg"}"#);

        let index = index_sessions(&[dir.to_path_buf(), dir.join("nope")]);
        assert!(index["good"].background);
        assert_eq!(index.len(), 1, "坏文件与非 json 不该进索引");
        assert!(index_sessions(&[]).is_empty());
    }
}
