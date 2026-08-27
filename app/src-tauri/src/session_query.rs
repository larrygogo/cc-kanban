//! 活跃会话查询服务与 Tauri 适配层。

use meowo_agent::SessionRuntime;
use meowo_store::LiveSession;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::State;

/// 必须 ≥ 对话窗 650ms 的历史轮询周期:此前 300ms 意味着单开对话窗时每一轮都击穿缓存,
/// Windows 上每 650ms 付一次 30-120ms 的全进程扫描(proc.rs 注释实测)。900ms 让相邻
/// 轮询共享同一次采样;存活展示容忍 ~1s 的陈旧(RESUME_GRACE 是 120s 量级)。
const PROCESS_SNAPSHOT_TTL_MS: i64 = 900;

/// Counts and list queries share one process-table observation during a UI refresh.
#[derive(Default)]
pub(crate) struct ProcessSnapshotCache {
    slot: Mutex<Option<(i64, Arc<std::collections::HashSet<i64>>)>>,
}

impl ProcessSnapshotCache {
    pub(crate) fn snapshot(&self) -> Arc<std::collections::HashSet<i64>> {
        self.snapshot_with(super::now_ms(), super::proc::agent_pids_snapshot)
    }

    fn snapshot_with(
        &self,
        now: i64,
        sample: impl FnOnce() -> std::collections::HashSet<i64>,
    ) -> Arc<std::collections::HashSet<i64>> {
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        if let Some((sampled_at, pids)) = slot.as_ref() {
            if now.saturating_sub(*sampled_at) < PROCESS_SNAPSHOT_TTL_MS {
                return pids.clone();
            }
        }
        let pids = Arc::new(sample());
        *slot = Some((now, pids.clone()));
        pids
    }
}

/// agent 自建后台会话的索引失效周期。比进程表快照(900ms)宽松:一个会话的**出身**在它整个
/// 生命里不会变,只在新会话出现时才需要重扫,而每次扫描是几个几百字节的 json。
const RUNTIME_SNAPSHOT_TTL_MS: i64 = 3_000;

/// `provider → (session id → 运行形态)`。分两层是为了让热路径按 `&str` 查，不必为每行
/// 会话克隆出一个元组 key。
pub(crate) type SessionRuntimeIndex =
    std::collections::HashMap<&'static str, std::collections::HashMap<String, SessionRuntime>>;

/// agent 自建后台会话的索引缓存。与 [`ProcessSnapshotCache`] 同理:一次界面刷新里
/// 计数与列表两条查询共享同一次扫描。
#[derive(Default)]
pub(crate) struct SessionRuntimeCache {
    slot: Mutex<Option<(i64, Arc<SessionRuntimeIndex>)>>,
}

impl SessionRuntimeCache {
    pub(crate) fn snapshot(&self) -> Arc<SessionRuntimeIndex> {
        self.snapshot_with(super::now_ms(), collect_session_runtimes)
    }

    fn snapshot_with(
        &self,
        now: i64,
        sample: impl FnOnce() -> SessionRuntimeIndex,
    ) -> Arc<SessionRuntimeIndex> {
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        if let Some((sampled_at, index)) = slot.as_ref() {
            if now.saturating_sub(*sampled_at) < RUNTIME_SNAPSHOT_TTL_MS {
                return index.clone();
            }
        }
        let index = Arc::new(sample());
        *slot = Some((now, index.clone()));
        index
    }
}

/// 扫一遍所有声明了 runtime 能力的 agent。目前只有 claude(FleetView 的后台会话);
/// 没声明的 agent 连一个空 map 都不进索引,查表恒为 miss = 一视同仁。
fn collect_session_runtimes() -> SessionRuntimeIndex {
    meowo_agent::all()
        .iter()
        .filter_map(|plugin| Some((plugin.id().as_str(), plugin.runtime()?.session_runtimes())))
        .collect()
}

/// 该会话是否由 agent 自己的后台守护进程托管(claude FleetView 的 `kind: "bg"`)。
/// 查不到 = 普通会话:索引只是**额外**信息源,缺了它不该改变任何既有判断。
pub(crate) fn is_background(
    index: &SessionRuntimeIndex,
    provider: &str,
    cc_session_id: &str,
) -> bool {
    index
        .get(provider)
        .and_then(|sessions| sessions.get(cc_session_id))
        .is_some_and(|runtime| runtime.background)
}

/// 这个会话该不该从看板上拿掉。
///
/// **后台会话一律拿掉**：它们是 agent 自己在会话中途派生的，用户没开过、看不懂凭空多出的
/// 卡片，而且几乎操作不了——键盘输入无效（worker 不消费 stdin），发消息守护进程虽收下但
/// 手动模式下不执行（取证见 `bgpty.rs` 的模块文档）。给它一张和普通会话一样的卡，等于
/// 承诺了一堆做不到的事。
///
/// 源会话卡上也**不留痕**（不标「+N 后台作业」之类的数）：用户既处理不了这些作业，一个
/// 数字除了让人困惑并无用处。代价是后台作业在 meowo 里彻底不可见——这是明确的取舍：宁可
/// 少显示，也不要一屏点开全是死胡同的卡片。要处理它们请回派出它们的终端。
pub(crate) fn hidden_background(
    index: &SessionRuntimeIndex,
    provider: &str,
    cc_session_id: &str,
) -> bool {
    // 眼下就是 is_background 本身。留一个独立名字是因为两者问的是**不同的问题**——
    // 「它是不是后台会话」与「它该不该从看板上消失」——把后者写成前者的别名，将来要放宽
    // 隐藏规则（比如只藏查得到源的那些）时改一处即可，不会让列表和角标各改各的。
    is_background(index, provider, cc_session_id)
}

/// 该会话的运行形态全貌。查不到 = 普通会话,取默认值(全 false / None)——索引只是**额外**
/// 信息源,缺了它不该改变任何既有判断。
fn runtime_of(
    index: &SessionRuntimeIndex,
    provider: &str,
    cc_session_id: &str,
) -> meowo_agent::SessionRuntime {
    index
        .get(provider)
        .and_then(|sessions| sessions.get(cc_session_id))
        .cloned()
        .unwrap_or_default()
}

#[tauri::command]
pub(crate) async fn recent_cwds(
    state: State<'_, super::AppState>,
    limit: usize,
) -> Result<Vec<String>, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        super::open_store(&db_path)?
            .recent_cwds(limit)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(serde::Serialize)]
pub(crate) struct LiveItem {
    #[serde(flatten)]
    pub(crate) inner: LiveSession,
    pub(crate) connected: bool,
    /// 本 GUI 进程正托管该会话的 PTY。门控卡片菜单「结束会话」的可见性——外部终端里
    /// 跑的会话杀不了（要先走接管），与对话窗 ChatHistoryDto.pty_managed 同口径。
    pty_managed: bool,
    /// 由 agent 自己的后台守护进程托管（claude FleetView 的后台会话）。这类卡片是 agent
    /// 在会话中途自行派生的，用户没开过它，界面必须标注出来，并收起接管/结束——那两条
    /// 路对它注定失败（supervisor 会把被杀的进程按 respawnFlags 拉回来）。
    background: bool,
    errored: bool,
    error_label: Option<String>,
    error_raw: Option<String>,
    preview: Option<String>,
    /// 所属账号的展示名（None = 默认账号，前端不显示徽章）。由 profile id 经 settings 解析；
    /// profile 已被删除（查不到）时回退 id 本身——宁可显示原始 id 也不丢归属信息。
    profile_name: Option<String>,
    /// 托管会话的屏幕检测状态（"working"|"idle"|"blocked"），来自 PTY broker 的终端仿真
    /// （见 pty.rs 的 ScreenProbe）。None = 非托管会话 / 无规则集 / 启动宽限内。
    /// 由 live_sessions_blocking 在裁定 tab 归属（tab_class）的同一处填充——DTO 里的
    /// 屏幕状态与该卡的 tab 归属出自同一份快照，enrich 不经手；外库卡恒 None。
    screen_state: Option<&'static str>,
    /// 还在跑的后台子任务数（transcript 分析，见 TranscriptInfo::busy_subagents）。
    /// 前端卡片状态环与 tab 分组按它把「主回合停了但后台在干活」的会话画成运行中——
    /// 必须与后端 tab_class 的 background_busy 同口径，否则环色与所在 tab 打架。
    busy_subagents: u32,
    /// 来自**另一个实例的库**（dev 构建只读聚合安装版 `~/.meowo/board.db` 的会话，
    /// 见 [`foreign_live_items`]）。前端据此标注来源并收起全部操作入口——这些会话的
    /// PTY/审批/收尾都归安装版，本实例只有观察权。id 已加 [`FOREIGN_ID_OFFSET`] 防撞：
    /// 即使某个操作入口漏禁，本库也无此行，落空成 no-op 而不是改错会话。
    foreign: bool,
}

/// profile id → 展示名。查不到（profile 已删）回退 id 本身。纯函数便于单测。
fn resolve_profile_name(
    settings: &super::settings::Settings,
    provider: &str,
    id: &str,
) -> String {
    settings
        .profiles
        .get(provider)
        .and_then(|list| list.iter().find(|p| p.id == id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| id.to_string())
}

/// 翻页游标：**SQL 扫描位置**（排序前）。响应里的 items 会做 connected-first 排序，
/// 末项不再是本页时间上最旧的一条——拿末项当游标会重复/漏页（旧版 loadMore 的坑）。
/// 调用方翻下一页必须回传这里给出的 cursor，而不是自己从 items 推。
#[derive(serde::Serialize, Clone, Copy)]
pub(crate) struct PageCursor {
    pub(crate) last_event_at: i64,
    pub(crate) id: i64,
}

#[derive(serde::Serialize)]
pub(crate) struct LiveSessionsPage {
    pub(crate) items: Vec<LiveItem>,
    /// None = 已扫到底，没有下一页。Some 也可能恰好是最后一行——下一页会拿到空 items + None。
    pub(crate) next_cursor: Option<PageCursor>,
}

#[tauri::command]
pub(crate) async fn get_live_sessions_counts(
    state: State<'_, super::AppState>,
) -> Result<meowo_store::query::LiveSessionCounts, String> {
    let db_path = state.db_path.clone();
    // 进程表采样在 spawn_blocking 里做:冷采样 Windows 上要 30-120ms,不能挂在
    // async-runtime 线程上(且采样持缓存互斥锁,并发命令会排队其后)。
    let snapshots = state.process_snapshots.clone();
    // 托管 PTY 活跃 = 进程必在(meowo 自己 spawn 的),hook 未认领 pid / 事件宽限过期时兜底。
    let pty_live = state.ptys.active_session_ids();
    // 审批存活与屏幕状态:tab 归属(tab_class)的另外两样原料,必须与列表同一来源采样,
    // 否则「黄环(等你)卡在运行中 tab」「计数与列表打架」两类症状都会回来。
    let approvals = state.ptys.approval_session_ids();
    let screen_states = state.ptys.screen_states();
    let runtimes = state.session_runtimes.clone();
    let tx_cache = state.tx_cache.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let alive = snapshots.snapshot();
        let runtimes = runtimes.snapshot();
        let store = super::open_store(&db_path)?;
        let mut counts = counts_for_store(
            &store,
            &tx_cache,
            &alive,
            &pty_live,
            &runtimes,
            &approvals,
            &screen_states,
        )?;
        // 外库(安装版)会话也计入角标——列表并入了外库卡(get_live_sessions_page 的
        // include_foreign),数字必须与列表同视野,否则出现「待交互 0 但列表挂着一排
        // 待交互卡」的打架现场(实拍反馈)。险闸与失败降级同 foreign_live_items:
        // 聚合不上就只数本库,宁可少数不报错。
        if let Some(foreign) = foreign_counts(&db_path, &tx_cache, &alive, &runtimes) {
            counts.total += foreign.total;
            counts.running += foreign.running;
            counts.waiting += foreign.waiting;
            counts.archived += foreign.archived;
        }
        Ok(counts)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 单个库的角标统计(总数/运行中/待交互/归档)。本库与外库共用同一口径——外库把
/// `pty_live`/`approvals`/`screen_states` 传空(那些是本实例 broker 的事实,对外库
/// 会话无意义;审批校正按 broker 为空处理,与 foreign_live_items 同口径),其余判定
/// (进程表、fleet 后台隐藏)观察的是全局事实,两库通用。
#[allow(clippy::too_many_arguments)]
fn counts_for_store(
    store: &meowo_store::Store,
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    alive: &std::collections::HashSet<i64>,
    pty_live: &std::collections::HashSet<i64>,
    runtimes: &SessionRuntimeIndex,
    approvals: &std::collections::HashSet<i64>,
    screen_states: &std::collections::HashMap<i64, &'static str>,
) -> Result<meowo_store::query::LiveSessionCounts, String> {
    let (mut total, mut archived) = store.live_sessions_totals().map_err(|e| e.to_string())?;
    // 列表把后台会话整个藏了（见 hidden_background），总数也得同口径扣掉。不能只在
    // 下面的候选循环里扣：候选集只含「未归档 + 未结束 + running/waiting」，而一条**已
    // 结束**但有标题的后台会话照样计进 total、列表里却永远看不到——那正是「角标数出
    // 一条用户找不到的会话」。所以按 id 直接向 DB 问一次，用与 totals 逐字相同的口径。
    let hidden: Vec<String> = runtimes
        .values()
        .flat_map(|sessions| sessions.iter())
        .filter(|(_, runtime)| runtime.background)
        .map(|(id, _)| id.clone())
        .collect();
    if !hidden.is_empty() {
        let (n, a) = store.live_totals_for(&hidden).map_err(|e| e.to_string())?;
        total -= n;
        archived -= a;
    }
    let candidates = store.live_count_candidates().map_err(|e| e.to_string())?;
    let now = super::now_ms();
    let (mut running, mut waiting) = (0i64, 0i64);
    for candidate in candidates {
        // 后台会话列表里看不到（enrich 无条件丢弃），running/waiting 也不能数——
        // 否则用户会对着一个找不到对应卡片的数字发懵。total 已在上面按同一口径扣过。
        if hidden_background(
            runtimes,
            candidate.provider.as_deref().unwrap_or_default(),
            &candidate.cc_session_id,
        ) {
            continue;
        }
        let connected = session_connected(
            &candidate.status,
            candidate.pid,
            process_alive(candidate.pid, alive, pty_live.contains(&candidate.id)),
            candidate.last_event_at,
            now,
        );
        // 分类原料与列表(live_sessions_blocking)**逐样对齐**:审批存活校正后的
        // pending_review + 实时屏幕状态。tab 归属不再由 SQL 谓词决定(SQL 只出候选
        // 并集),两端唯一的裁判是 tab_class——原料一致,数字与列表就不可能打架。
        // provider 的空值回退也要跟 store 的 LiveSession 同步(NULL/空串按默认 agent),
        // 否则同一条会话列表侧做了审批校正、这里没做,又是一处口径分叉。
        let provider = candidate
            .provider
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or(meowo_store::DEFAULT_PROVIDER);
        let pending_review = pending_review_live(
            provider,
            candidate.pending_review.as_deref(),
            approvals.contains(&candidate.id),
        );
        match tab_class(
            connected,
            &candidate.status,
            pending_review,
            screen_states.get(&candidate.id).copied(),
            || {
                subagents_busy(
                    tx_cache,
                    provider,
                    candidate.cwd.as_deref(),
                    &candidate.cc_session_id,
                )
            },
        ) {
            Some("waiting") => waiting += 1,
            Some("running") => running += 1,
            _ => {}
        }
    }
    Ok(meowo_store::query::LiveSessionCounts {
        total,
        running,
        waiting,
        archived,
    })
}

/// 外库的角标贡献。打开/险闸/查询任何一步失败都返回 None(只数本库)。
fn foreign_counts(
    own_db: &Path,
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    alive: &std::collections::HashSet<i64>,
    runtimes: &SessionRuntimeIndex,
) -> Option<meowo_store::query::LiveSessionCounts> {
    let path = foreign_db_path(own_db)?;
    let store = meowo_store::Store::open_readonly(&path).ok()?;
    if store.user_version().ok()? != meowo_store::Store::CURRENT_USER_VERSION {
        return None;
    }
    let empty = std::collections::HashSet::new();
    let no_screens = std::collections::HashMap::new();
    counts_for_store(&store, tx_cache, alive, &empty, runtimes, &empty, &no_screens).ok()
}

#[tauri::command]
// 参数形状即前端 invoke 的调用契约(Tauri 按 camelCase 逐名匹配),合并成 struct 会
// 破坏旧前端兼容;第 8 个参数为外库聚合开关,与 terminal.rs 的同款豁免一个理由。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn get_live_sessions_page(
    state: State<'_, super::AppState>,
    filter: String,
    search: Option<String>,
    cwd: Option<String>,
    before_last_event_at: Option<i64>,
    before_id: Option<i64>,
    limit: usize,
    // 是否附带外库(安装版)会话。只有贴纸看板传 true——对话侧栏/归档页按 id 加载
    // 会话详情,外库卡点开必然落空,不如从源头不给。None 当 false(旧前端兼容)。
    include_foreign: Option<bool>,
) -> Result<LiveSessionsPage, String> {
    let db_path = state.db_path.clone();
    let tx_cache = state.tx_cache.clone();
    // 采样进 spawn_blocking,理由同 get_live_sessions_counts。
    let snapshots = state.process_snapshots.clone();
    let runtimes = state.session_runtimes.clone();
    let pty_live = state.ptys.active_session_ids();
    let approvals = state.ptys.approval_session_ids();
    let screen_states = state.ptys.screen_states();
    let filter = normalize_filter(filter);
    tauri::async_runtime::spawn_blocking(move || {
        let alive = snapshots.snapshot();
        let runtimes = runtimes.snapshot();
        let mut page = live_sessions_blocking(
            &db_path,
            &tx_cache,
            LiveContext {
                alive: &alive,
                pty_live: &pty_live,
                runtimes: &runtimes,
                approvals: &approvals,
                screen_states: &screen_states,
            },
            &filter,
            search.as_deref(),
            cwd.as_deref(),
            PageReq {
                before_last_event_at,
                before_id,
                limit,
            },
        )?;
        // 外库聚合只做首页(无游标):外库不参与翻页,一次全并入;后续页的游标语义
        // 始终属于本库。稳定排序把外库卡排到各 connected 组的尾部——观察性信息不与
        // 本库的活动卡抢位置。外库卡的屏幕状态恒 None(本地 broker 不托管它们),
        // tab 归属由 foreign_live_items 按同一 tab_class 裁定。
        if include_foreign.unwrap_or(false)
            && before_id.is_none()
            && before_last_event_at.is_none()
        {
            let foreign = foreign_live_items(
                &db_path,
                &tx_cache,
                &alive,
                &runtimes,
                &filter,
                search.as_deref(),
                cwd.as_deref(),
            );
            if !foreign.is_empty() {
                page.items.extend(foreign);
                page.items.sort_by_key(|item| std::cmp::Reverse(item.connected));
            }
        }
        Ok(page)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn normalize_filter(filter: String) -> String {
    if ["all", "running", "waiting", "archived"].contains(&filter.as_str()) {
        filter
    } else {
        "all".into()
    }
}

/// 存活判定的唯一 OR:进程表命中,或该会话的托管 PTY 活跃(meowo 自己 spawn 的进程必在)。
/// 对话窗(chat.rs)、看板计数、会话列表三条通道共用——此前三处手抄,措辞已开始分叉。
pub(crate) fn process_alive(
    pid: Option<i64>,
    alive: &std::collections::HashSet<i64>,
    pty_live: bool,
) -> bool {
    pid.is_some_and(|p| p > 0 && alive.contains(&p)) || pty_live
}

/// transcript 分析的唯一接线:supports_analysis 门控 + 路径解析 + 共享 mtime 缓存。
/// 侧栏/贴纸(enrich)与对话窗(errored)同源于此,「同口径」不再靠手工维持。
pub(crate) fn analyze_transcript(
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    provider: &str,
    cwd: Option<&str>,
    cc_session_id: &str,
) -> Option<meowo_agent::transcript::TranscriptInfo> {
    let spec = super::agent_transcript(provider).filter(|spec| spec.supports_analysis())?;
    let path = spec.resolve_transcript_path(None, cwd, cc_session_id)?;
    let path = path.to_str()?;
    Some(meowo_agent::TranscriptCache::analyze_shared(
        tx_cache, spec, path,
    ))
}

/// 「后台还有子任务在跑」——tab 归属与卡片状态环的共用原料（[`tab_class`] 的懒参数）。
/// 走 [`analyze_transcript`] 的共享 mtime 缓存：文件没动就是纯内存读。
pub(crate) fn subagents_busy(
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    provider: &str,
    cwd: Option<&str>,
    cc_session_id: &str,
) -> bool {
    analyze_transcript(tx_cache, provider, cwd, cc_session_id)
        .is_some_and(|info| info.busy_subagents > 0)
}

/// Resume and newly-spawned sessions remain optimistically connected while hooks claim the PID.
pub(crate) const RESUME_GRACE_MS: i64 = 120_000;

pub(crate) fn session_connected(
    status: &str,
    _pid: Option<i64>,
    pid_alive: bool,
    last_event_at: i64,
    now: i64,
) -> bool {
    if status == "ended" {
        // ended 只让位于**实证**存活(进程表命中 / 本 GUI 托管 PTY 活跃):恢复会话的
        // 窗口期 DB 还挂着旧 'ended' 而 PTY 已在跑,不给存活兜底就会「已结束」徽标配
        // 「结束会话」按钮同框。事件宽限不适用——刚结束的会话天然在宽限期内,套用它
        // 会让所有 ended 会话诈尸 120 秒。
        return pid_alive;
    }
    pid_alive || now.saturating_sub(last_event_at) < RESUME_GRACE_MS
}

/// 「待批准」此刻是否还成立——`pending_review` 的存活校正，与 [`session_connected`] 对
/// `status` 做的事同构：DB 里那一位是 hook 落库的滞后快照，直接展示就会撒谎。
///
/// **claude 这类由 hook 决定权限的 provider**（[`permission_hook_decides`]）：真相在 broker。
/// `PermissionRequest` hook 阻塞期间请求挂在 broker 上；hook 一返回就没了——要么用户在 GUI
/// 里批了/拒了，要么没有 GUI 消费者、交还终端由用户当场处理。三种结局都意味着**没人再等**，
/// 可 DB 里的 `pending_review` 要等下一个 hook 事件（PostToolUse/Stop）才清：被放行的工具
/// 跑 20 分钟，对话页就错挂 20 分钟「Agent 请求权限」，而终端里 agent 正忙着干活。
///
/// Question/Plan 不做这个校正：那两个工具本身就是「停下来等人」，无论提问此刻挂在
/// broker 上（AskUserQuestion 的 PreToolUse 代答挂起）还是表单已落在终端里，等的期间
/// DB 标记都正是对的；代答/作答后由 reporter（settled）与 resolve 命令当场清位。
pub(crate) fn pending_review_live<'a>(
    provider: &str,
    pending_review: Option<&'a str>,
    broker_holds_approval: bool,
) -> Option<&'a str> {
    match pending_review {
        Some("approval")
            if !broker_holds_approval
                && meowo_agent::by_id(provider).is_some_and(|p| p.permission_hook_decides()) =>
        {
            None
        }
        other => other,
    }
}

/// Single definition shared by list filtering and tab counters.
///
/// 阶梯与前端卡片状态环（helpers.ts 的 cardTone）**同序同源**：审批 > 屏幕检测 > DB status。
/// 曾经 tab 归属只看 DB status、卡片环优先屏幕状态——一条 status=running 而屏幕已 idle
/// 的会话就会顶着黄环（等你）待在「运行中」tab 里，用户以为状态坏了（实拍反馈）。
///
/// 入参约定：`pending_review` 传**存活校正后**的值（[`pending_review_live`]），
/// `screen_state` 传 broker 的实时屏幕状态（"working"|"idle"|"blocked"，非托管为 None）。
/// `background_busy` 是「后台还有子任务在跑」的**懒**判定（要读 transcript，只在裁到
/// 「等待中」边缘时才调用；mtime 缓存下稳态零 IO）。列表（live_sessions_blocking /
/// foreign_live_items）与角标（counts_for_store）三处必须喂完全相同的原料——任何一端
/// 少喂一样，数字就会与列表打架。
pub(crate) fn tab_class(
    connected: bool,
    status: &str,
    pending_review: Option<&str>,
    screen_state: Option<&str>,
    background_busy: impl FnOnce() -> bool,
) -> Option<&'static str> {
    if !connected {
        return None;
    }
    if pending_review.is_some() {
        return Some("waiting");
    }
    // 屏幕检测（300ms 级实时）优先于 DB status（hook 事件驱动、天然滞后）：
    // blocked = 屏幕上挂着审批/提问，是真要人，后台再忙也得等交互。
    // idle / DB waiting 只说明**主回合**停了——后台子任务还在独立干活时，把会话标成
    // 「等你」是谎报（实拍反馈：委派了后台审查 agent 的会话在等待区躺了半小时）。
    match screen_state {
        Some("working") => return Some("running"),
        Some("blocked") => return Some("waiting"),
        Some("idle") => {
            return Some(if background_busy() { "running" } else { "waiting" });
        }
        _ => {}
    }
    if status == "waiting" {
        return Some(if background_busy() { "running" } else { "waiting" });
    }
    (status == "running").then_some("running")
}

pub(crate) struct PageReq {
    pub(crate) before_last_event_at: Option<i64>,
    pub(crate) before_id: Option<i64>,
    pub(crate) limit: usize,
}

/// 单页条数上限。limit 是 IPC/远程桥入参，不钳制时超大值会让 `Vec::with_capacity`
/// 做巨额分配——分配失败在 Rust 里是 allocation abort，整个进程直接被带崩。
/// 500 已远超侧栏实际页长（常态 20-60），够补页循环用又不留滥用面。
const MAX_PAGE_LIMIT: usize = 500;

/// 一次列表查询共享的**外部事实观测**：DB 之外、随时可变、必须现采的那几样。三者的采样
/// 都在同一个 spawn_blocking 里完成、生命周期一致，故打包同行——而不是各占一个参数位。
pub(crate) struct LiveContext<'a> {
    /// 进程表快照(TTL 缓存,与角标查询共享一次采样)。
    pub(crate) alive: &'a std::collections::HashSet<i64>,
    /// 托管 PTY 活跃的会话集合:hook 未认领 pid / 事件宽限过期时的存活兜底,
    /// 与对话窗口(chat.rs 的 connected)同口径,两边不再各说各话。
    pub(crate) pty_live: &'a std::collections::HashSet<i64>,
    /// agent 自建后台会话的索引(见 [`SessionRuntimeCache`])。
    pub(crate) runtimes: &'a SessionRuntimeIndex,
    /// 此刻 broker 手里还压着审批请求的会话(见 [`pending_review_live`])。DB 的
    /// `pending_review` 是滞后快照,要靠它校正,否则被放行的工具跑多久就错挂多久「待批准」。
    pub(crate) approvals: &'a std::collections::HashSet<i64>,
    /// 托管会话的实时屏幕状态（sid → "working"|"idle"|"blocked"，见 PtyBroker::screen_states）。
    /// 既填进卡片 DTO，也参与 tab 归属判定（[`tab_class`]）——两者必须同一份快照，
    /// 否则又回到「黄环卡在运行中 tab」的打架现场。
    pub(crate) screen_states: &'a std::collections::HashMap<i64, &'static str>,
}

pub(crate) fn live_sessions_blocking(
    db_path: &Path,
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    live: LiveContext<'_>,
    filter: &str,
    search: Option<&str>,
    // 目录筛选(侧栏「按目录」):斜杠归一的精确匹配,与 search 的子串语义分开走,
    // 见 store 的 live_sessions_filtered 文档注释。
    cwd: Option<&str>,
    page: PageReq,
) -> Result<LiveSessionsPage, String> {
    if page.limit == 0 {
        return Ok(LiveSessionsPage {
            items: Vec::new(),
            next_cursor: None,
        });
    }
    // 入口钳制：limit 来自 IPC/远程桥入参，with_capacity/batch_limit 都由它派生，
    // 在这里收敛一次，下游全部按钳制后的值走（上限理由见 MAX_PAGE_LIMIT）。
    let page = PageReq {
        limit: page.limit.min(MAX_PAGE_LIMIT),
        ..page
    };
    let store = super::open_store(db_path)?;
    let now = super::now_ms();
    // waiting|running 时 SQL 给的是候选并集（见 store 的谓词注释），真正的 tab 归属
    // 在下面按 tab_class 裁定——屏幕状态与审批校正只有这一层看得见。
    let class_filtered = matches!(filter, "waiting" | "running");
    // 每批都比 limit 多取：enrich 之后还要丢弃 ping / 空会话（以及 waiting|running 下
    // 归属不符/未连接的候选），按 limit 取批会让一页严重缩水——侧栏没有「加载更多」，
    // 缩水多少就少显示多少（曾出现 60 条里只剩 11 条，用户以为会话丢了）。
    let batch_limit = page.limit.max(100);
    // 单次请求的扫描上限：补页不能变成无界全表扫描（几乎全被过滤的库——大量空壳、
    // 或 waiting|running 下大量断开——会让循环翻到表尾，每行都过一遍 enrich）。
    // 到达上限就带着扫描位置返回，调用方拿 next_cursor 继续，代价摊到多次请求。
    let scan_cap = batch_limit.saturating_mul(10);
    let mut scanned = 0usize;
    let mut cursor_ts = page.before_last_event_at;
    let mut cursor_id = page.before_id;
    let mut items: Vec<LiveItem> = Vec::with_capacity(page.limit);
    // Some = 页满/到达扫描上限时的续扫位置；None = 已扫到底。
    let mut next_cursor: Option<PageCursor> = None;
    'fill: loop {
        let sessions = store
            .live_sessions_filtered(Some(filter), search, cwd, cursor_ts, cursor_id, batch_limit)
            .map_err(|e| e.to_string())?;
        let raw_len = sessions.len();
        let batch_tail = sessions
            .last()
            .map(|session| (session.session.last_event_at, session.session.id));
        for mut session in sessions {
            scanned += 1;
            // 「待批准」的存活校正,与下面 connected 校正 status 是同一套路数(见
            // pending_review_live):hook 落库的那一位在权限已放行后仍会滞留到 PostToolUse。
            // 校正后的值同时决定**卡片展示**(pill / 状态点)与 **tab 归属**(下面的
            // tab_class)——角标(counts_for_store)吃同一份校正,数字与列表必然一致。
            session.pending_review = pending_review_live(
                &session.provider,
                session.pending_review.as_deref(),
                live.approvals.contains(&session.session.id),
            )
            .map(str::to_string);
            let scan_pos = PageCursor {
                last_event_at: session.session.last_event_at,
                id: session.session.id,
            };
            let pty_managed = live.pty_live.contains(&session.session.id);
            let connected = session_connected(
                &session.session.status,
                session.pid,
                process_alive(session.pid, live.alive, pty_managed),
                session.session.last_event_at,
                now,
            );
            let screen_state = live.screen_states.get(&session.session.id).copied();
            // waiting|running：SQL 只出候选并集，这里按统一口径裁归属（与卡片状态环、
            // 角标同源，见 tab_class 文档）。未连接项 tab_class 恒 None，天然被裁掉。
            let class_ok = !class_filtered
                || tab_class(
                    connected,
                    &session.session.status,
                    session.pending_review.as_deref(),
                    screen_state,
                    || {
                        subagents_busy(
                            tx_cache,
                            &session.provider,
                            session.cwd.as_deref(),
                            &session.session.cc_session_id,
                        )
                    },
                ) == Some(filter);
            if class_ok {
                let runtime = runtime_of(
                    live.runtimes,
                    &session.provider,
                    &session.session.cc_session_id,
                );
                if let Some(mut item) = enrich(tx_cache, session, connected, pty_managed, &runtime)
                {
                    item.screen_state = screen_state;
                    items.push(item);
                }
            }
            if items.len() >= page.limit || scanned >= scan_cap {
                // 游标取当前行的 SQL 位置（严格小于/大于续查，当前行不会重复出现）。
                next_cursor = Some(scan_pos);
                break 'fill;
            }
        }
        // 这一批没取满 batch_limit，说明后面没有数据了；补页到此为止。
        if raw_len < batch_limit {
            break;
        }
        let Some((timestamp, id)) = batch_tail else {
            break;
        };
        cursor_ts = Some(timestamp);
        cursor_id = Some(id);
    }
    // 稳定排序：已连接的顶到最前，同组内保持 SQL 的时间倒序。
    items.sort_by_key(|item| std::cmp::Reverse(item.connected));
    // 本页有归属自定义账号的会话才读一次 settings——全默认账号（没建过 profile 的用户）
    // 不为一个恒空的字段在热路径上付一次读盘。
    if items.iter().any(|item| item.inner.profile.is_some()) {
        let settings = super::settings::load_settings();
        for item in &mut items {
            if let Some(id) = item.inner.profile.as_deref() {
                item.profile_name = Some(resolve_profile_name(&settings, &item.inner.provider, id));
            }
        }
    }
    Ok(LiveSessionsPage { items, next_cursor })
}

/// 列表的丢弃规则（与 store 的 live_sessions_totals / live_count_candidates 口径对齐）：
/// 健康探测（ping），或一条既没标题、没 todo 又已经断开的空壳会话。
fn dropped_from_list(title: &str, connected: bool, todos: &[meowo_store::Todo]) -> bool {
    if title.eq_ignore_ascii_case("ping") {
        return true;
    }
    !connected && is_shell_title(title) && todos.is_empty()
}

/// 还没拿到名字的会话：没标题，或顶着占位标题。
fn is_shell_title(title: &str) -> bool {
    let title = title.trim();
    title.is_empty() || title == "(未命名会话)"
}

/// 补上 transcript 里的标题/错误/预览。返回 `None` 表示这条不该出现在列表里
/// （规则见 [`dropped_from_list`]）。
fn enrich(
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    mut session: LiveSession,
    connected: bool,
    pty_managed: bool,
    runtime: &meowo_agent::SessionRuntime,
) -> Option<LiveItem> {
    // 后台会话一律不建卡（理由见 hidden_background）。排在最前：省掉一次 transcript 读盘，
    // 这类会话的正文本来也多半没落盘。
    if runtime.background {
        return None;
    }
    // DB 数据已能定夺去留时先裁决，省掉 transcript 的文件 IO：只有会从 transcript
    // 补标题的 provider（目前 claude），未命名会话才可能被翻案；其余 provider 标题
    // 以 DB 为准，注定被丢的行不必为解析 transcript 付一次 open/read。
    let resolves_title = super::agent_resolves_transcript_title(&session.provider);
    // 去留只认 **DB 里的标题**，与 store 的 live_sessions_totals 同源。transcript 解析出的
    // 名字只管显示——曾经拿覆盖后的标题做判定，于是空壳会话靠 transcript 里一行 `ai-title`
    // 就赖着不走：store 认为它是空壳（不计数），列表却显示它，角标与列表当场打架。
    // 真实案例：claude 的 fork/resume 后台 worker 只写 ai-title / agent-name 两行元数据，
    // 一条对话都没有，卡片却顶着个名字、点开一片空白、还恢复不了。
    let db_title = session.task_title.clone();
    if !resolves_title && dropped_from_list(db_title.trim(), connected, &session.todos) {
        return None;
    }
    let mut error_label = None;
    let mut error_raw = None;
    let mut preview = None;
    let mut busy_subagents = 0;
    let info = analyze_transcript(
        tx_cache,
        &session.provider,
        session.cwd.as_deref(),
        &session.session.cc_session_id,
    );
    if let Some(info) = info {
        if super::agent_resolves_transcript_title(&session.provider) {
            if let Some(title) = info.title {
                session.task_title = title;
            }
        }
        if let Some(error) = info.error {
            error_label = Some(error.label);
            error_raw = Some(error.raw);
        }
        preview = info.preview;
        busy_subagents = info.busy_subagents;
    }
    if dropped_from_list(db_title.trim(), connected, &session.todos) {
        return None;
    }
    Some(LiveItem {
        inner: session,
        connected,
        pty_managed,
        // 当前隐藏规则下**恒为 false**：上面 `runtime.background` 为真的行已经整条丢掉
        // （后台会话一律不上看板，见 hidden_background）。字段留着不是摆设——放宽隐藏规则
        // 时（比如只藏查不到源的那些）这里立刻有值可发，前端的后台会话分支（贴纸的
        // 「去 FleetView 找它」提示、结束会话入口）也就随之活过来。改隐藏规则的人请连带
        // 检查那几处：它们目前只被合成数据的测试覆盖，线上走不到。
        background: runtime.background,
        errored: error_label.is_some(),
        error_label,
        error_raw,
        preview,
        // 展示名在组页阶段统一解析（见 live_sessions_blocking 尾部）。
        profile_name: None,
        // 屏幕状态由 live_sessions_blocking 在 tab 归属裁定处填充（enrich 不经手）。
        screen_state: None,
        busy_subagents,
        // 外库标记由 foreign_live_items 在聚合时置真，本库路径恒为 false。
        foreign: false,
    })
}

// ---------------------------------------------------------------------------
// 外库只读聚合（dev 构建的监控视野补全）
// ---------------------------------------------------------------------------

/// 外库会话的 id 偏移。两个库的 id 是独立自增序列必然相撞，而前端的行缓存、
/// 事件路由、后端的 pty_live/approvals/screen_states 全按 i64 id 索引——偏移到
/// 高位段一次性避开（2^40 远超任何真实自增值，且在 JS Number 的 2^53 精度内）。
/// 副作用即防线：偏移后的 id 在本库无对应行，漏禁的操作命令落空成 no-op。
const FOREIGN_ID_OFFSET: i64 = 1 << 40;

/// 外库单次拉取上限。外库只在列表**首页**聚合、不参与翻页游标（游标语义属于本库的
/// SQL 扫描位置，双库双游标不值得为一个观察视图付出的复杂度）；活跃会话远小于此数，
/// 截断只可能发生在深归档的 all 列表里，且截的是最旧的部分。
const FOREIGN_SCAN_LIMIT: usize = 200;

/// 安装版生产库的路径（仅 debug 构建返回 Some）。dev 默认与安装版同库（~/.meowo），
/// 同库判定返回 None、聚合自动停用；仅当 `MEOWO_DB` 把本库显式指向别处（如临时
/// 测试库）时，才把生产库的会话**只读**并回看板——监控视野不随库切换变窄。
fn foreign_db_path(own: &Path) -> Option<std::path::PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let path = std::path::PathBuf::from(home).join(".meowo").join("board.db");
    if !path.is_file() {
        return None;
    }
    // canonicalize 双方都成功才可比；任一失败退回字面比较——误判成同库只是少聚合，安全侧。
    let same = match (path.canonicalize(), own.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => path == own,
    };
    (!same).then_some(path)
}

/// 只读拉取外库会话并整形成外库卡。任何失败（打开失败/版本不一致/查询报错）一律
/// 返回空集：观察视图挂了不能连累本库列表整页报错。
///
/// 与本库路径的口径差异，各有理由：
/// - `pty_live`/`approvals` 恒空——那是**本实例** broker 的事实，对外库会话无意义；
///   审批校正传 false 等于以 DB 快照为准，与安装版对外部终端会话的口径一致。
/// - `alive` 进程表与 fleet 运行时索引照用——它们观察的是全局事实（进程、~/.claude），
///   与哪个库无关；后台会话的隐藏规则因此对外库同样生效。
/// - 版本险闸：外库 `user_version` 与本二进制不一致（dev 正在开发 schema 变更）就
///   整体跳过——绝不拿新 SQL 查旧库，也绝不迁移对方的库（open_readonly 已从根上锁死）。
fn foreign_live_items(
    own_db: &Path,
    tx_cache: &Mutex<meowo_agent::TranscriptCache>,
    alive: &std::collections::HashSet<i64>,
    runtimes: &SessionRuntimeIndex,
    filter: &str,
    search: Option<&str>,
    cwd: Option<&str>,
) -> Vec<LiveItem> {
    let Some(path) = foreign_db_path(own_db) else {
        return Vec::new();
    };
    let Ok(store) = meowo_store::Store::open_readonly(&path) else {
        return Vec::new();
    };
    if store.user_version().ok() != Some(meowo_store::Store::CURRENT_USER_VERSION) {
        return Vec::new();
    }
    let Ok(sessions) =
        store.live_sessions_filtered(Some(filter), search, cwd, None, None, FOREIGN_SCAN_LIMIT)
    else {
        return Vec::new();
    };
    let now = super::now_ms();
    let class_filtered = matches!(filter, "waiting" | "running");
    sessions
        .into_iter()
        .filter_map(|mut session| {
            session.pending_review =
                pending_review_live(&session.provider, session.pending_review.as_deref(), false)
                    .map(str::to_string);
            let connected = session_connected(
                &session.session.status,
                session.pid,
                process_alive(session.pid, alive, false),
                session.session.last_event_at,
                now,
            );
            // 与本库路径同一裁判（tab_class）：SQL 只出候选并集，归属在这裁。
            // 屏幕状态传 None——外库会话不由本地 broker 托管，没有实时屏幕可看。
            if class_filtered
                && tab_class(
                    connected,
                    &session.session.status,
                    session.pending_review.as_deref(),
                    None,
                    || {
                        subagents_busy(
                            tx_cache,
                            &session.provider,
                            session.cwd.as_deref(),
                            &session.session.cc_session_id,
                        )
                    },
                ) != Some(filter)
            {
                return None;
            }
            let runtime = runtime_of(runtimes, &session.provider, &session.session.cc_session_id);
            let mut item = enrich(tx_cache, session, connected, false, &runtime)?;
            item.foreign = true;
            item.inner.session.id += FOREIGN_ID_OFFSET;
            // 接续链的 id 指向外库行，本库的 get_session_lineage 解析不了；清掉，当普通卡展示。
            item.inner.predecessor_id = None;
            Some(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_filters_degrade_to_all() {
        assert_eq!(normalize_filter("unknown".into()), "all");
        assert_eq!(normalize_filter("waiting".into()), "waiting");
    }

    /// 真实翻车：终端里 agent 正跑着一条 20 分钟的 shell 命令，对话页却挂着「Agent 请求
    /// 权限 · 待授权」。权限早被放行（GUI 里批了，或没有 GUI 消费者、用户在终端当场批的），
    /// 但 DB 的 `pending_review` 要等 PostToolUse 才清——工具跑多久就错挂多久。
    /// 对 hook 决定权限的 provider，实时真相在 broker：它空了就说明没人再等。
    #[test]
    fn a_settled_permission_request_stops_claiming_the_agent_is_waiting() {
        // claude/codex 的 PermissionRequest hook 阻塞等决策：broker 空 = 已尘埃落定。
        assert_eq!(pending_review_live("claude", Some("approval"), false), None);
        assert_eq!(pending_review_live("codex", Some("approval"), false), None);
        // hook 还压着请求 = agent 真的停在那儿等你批。
        assert_eq!(
            pending_review_live("claude", Some("approval"), true),
            Some("approval")
        );
        // Question/Plan 由 PreToolUse 落库、不经 broker，而那两个工具本身就是停下来等人。
        assert_eq!(
            pending_review_live("claude", Some("question"), false),
            Some("question")
        );
        assert_eq!(
            pending_review_live("claude", Some("plan"), false),
            Some("plan")
        );
        // 不接管决策的 provider（kimi 的该事件 observation-only）：待批状态只有 DB 这一个
        // 来源，broker 空不代表已处理——照旧显示，卡片提示去终端处理。未知 provider 同理。
        assert_eq!(
            pending_review_live("kimi", Some("approval"), false),
            Some("approval")
        );
        assert_eq!(
            pending_review_live("who-knows", Some("approval"), false),
            Some("approval")
        );
        assert_eq!(pending_review_live("claude", None, false), None);
    }

    /// 空壳会话不能靠 transcript 里一行 `ai-title` 赖着不走：store 的计数按 **DB 标题**
    /// 判空壳（已结束 + 未命名 + 无待办），列表若按解析后的标题判，两个数字当场打架。
    /// claude 的 fork/resume 后台 worker 正是这样——只写 ai-title / agent-name 两行元数据，
    /// 一条对话都没有，卡片却顶着个名字、点开一片空白、还恢复不了。
    #[test]
    fn a_transcript_title_cannot_keep_an_empty_session_on_the_board() {
        // 已结束 + DB 未命名 + 无待办 = 空壳，无论 transcript 解析出什么名字都该丢。
        assert!(dropped_from_list("(未命名会话)", false, &[]));
        assert!(dropped_from_list("", false, &[]));
        // 还连着、或有待办、或 DB 本来就有名字 → 都不是空壳。
        assert!(!dropped_from_list("(未命名会话)", true, &[]));
        assert!(!dropped_from_list("真实标题", false, &[]));
    }

    /// 后台会话一律不上看板：用户没开过它们、也几乎操作不了（打字无效，发消息守护进程
    /// 收下但手动模式下不执行）。列表与角标必须同口径剔除，否则角标数出一条用户在列表里
    /// 找不到的会话——这条口径以前打过架。
    #[test]
    fn background_sessions_are_kept_off_the_board_entirely() {
        let index = fixture();
        assert!(hidden_background(&index, "claude", "job-a"));
        assert!(hidden_background(&index, "claude", "spare-one"));
        // 用户自己开的会话、别的 agent、查无此人：一概不剔除。
        assert!(!hidden_background(&index, "claude", "mine"));
        assert!(!hidden_background(&index, "codex", "job-a"));
        assert!(!hidden_background(&index, "claude", "nobody"));
    }

    /// 一个用户自己的会话 + 它派生的两个后台作业 + 一个预热进程。
    fn fixture() -> SessionRuntimeIndex {
        let bg = |parent: Option<&str>, spare: bool| meowo_agent::SessionRuntime {
            background: true,
            spare,
            forked_from: parent.map(str::to_string),
            ..Default::default()
        };
        let claude = [
            ("mine".to_string(), meowo_agent::SessionRuntime::default()),
            ("job-a".to_string(), bg(Some("mine"), false)),
            ("job-b".to_string(), bg(Some("mine"), false)),
            ("spare-one".to_string(), bg(None, true)),
        ]
        .into_iter()
        .collect();
        [("claude", claude)].into_iter().collect()
    }

    /// id → 展示名：命中 settings 用展示名；profile 已删（查不到）回退 id 本身——
    /// 宁可显示原始 id 也不丢归属信息；跨 agent 的同 id 不能串。
    #[test]
    fn profile_name_resolves_from_settings_with_id_fallback() {
        let mut s = super::super::settings::Settings::default();
        s.profiles.insert(
            "claude".into(),
            vec![crate::profile::Profile {
                id: "work".into(),
                name: "工作".into(),
            }],
        );
        assert_eq!(resolve_profile_name(&s, "claude", "work"), "工作");
        // profile 已删 / 别的 agent：回退 id 本身。
        assert_eq!(resolve_profile_name(&s, "claude", "gone"), "gone");
        assert_eq!(resolve_profile_name(&s, "codex", "work"), "work");
    }

    /// 侧栏没有「加载更多」，一次请求拿到多少就显示多少。空会话是在取完一批**之后**
    /// 才被 `enrich` 丢掉的，所以补页循环必须继续往下取直到凑满 limit——否则一批
    /// 空壳会话就能把整个列表压到个位数（真实案例：60 条里只剩 11 条）。
    #[test]
    fn empty_sessions_do_not_shrink_the_page() {
        let dir = std::env::temp_dir().join(format!("meowo-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("page.db");
        let _ = std::fs::remove_file(&db);
        let old = super::super::now_ms() - RESUME_GRACE_MS * 10; // 早于宽限期 → 一定判为未连接

        {
            let store = meowo_store::Store::open(&db).unwrap();
            let project = store
                .upsert_project_by_root("/tmp/proj", "proj", old)
                .unwrap();
            // 先塞 80 条空壳（无标题、无 todo、已结束），再塞 20 条有标题的。按时间倒序，
            // 空壳排在前面，第一批 100 条里只有 20 条能通过过滤。
            for i in 0..100 {
                let cc = format!("cc-{i:03}");
                let (sid, _) = store.start_session(project, &cc, old + i).unwrap();
                if i >= 80 {
                    store
                        .set_session_title(sid, &format!("真实会话 {i}"), old + i)
                        .unwrap();
                }
                store
                    .set_session_status(sid, meowo_store::SessionStatus::Ended, old + i)
                    .unwrap();
            }
            // 再补 60 条有标题的、更早的会话，用来验证补页确实翻到了第二批。
            for i in 0..60 {
                let cc = format!("older-{i:03}");
                let (sid, _) = store.start_session(project, &cc, old - 1000 - i).unwrap();
                store
                    .set_session_title(sid, &format!("更早会话 {i}"), old - 1000 - i)
                    .unwrap();
                store
                    .set_session_status(sid, meowo_store::SessionStatus::Ended, old - 1000 - i)
                    .unwrap();
            }
        }

        let cache = Mutex::new(meowo_agent::TranscriptCache::default());
        let alive = std::collections::HashSet::new();
        let pty_none = std::collections::HashSet::new();
        let no_runtimes = SessionRuntimeIndex::new();
        let no_approvals = std::collections::HashSet::new();
        let no_screens = std::collections::HashMap::new();
        let page = live_sessions_blocking(
            &db,
            &cache,
            LiveContext {
                alive: &alive,
                pty_live: &pty_none,
                runtimes: &no_runtimes,
                approvals: &no_approvals,
                screen_states: &no_screens,
            },
            "all",
            None,
            None,
            PageReq {
                before_last_event_at: None,
                before_id: None,
                limit: 60,
            },
        )
        .unwrap();

        assert_eq!(page.items.len(), 60, "补页应凑满一整页，而不是被空会话压缩");
        assert!(
            page.items
                .iter()
                .all(|item| !item.inner.task_title.trim().is_empty()),
            "空壳会话不该出现在结果里",
        );
        assert!(
            page.next_cursor.is_some(),
            "库里还有第 61+ 条有效会话，next_cursor 不该是 None",
        );
        let _ = std::fs::remove_file(&db);
    }

    /// 排序会把已连接的会话顶到页首，页内末项不再是时间上最旧的一条——旧版调用方拿
    /// 末项当游标，翻页要么重复要么打转。现在后端在响应里带回**扫描位置**游标，调用方
    /// 必须回传它。这里把整个翻页过程跑完，断言既收全、也不重复。
    #[test]
    fn cursor_paging_collects_every_session() {
        let dir = std::env::temp_dir().join(format!("meowo-cursor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("cursor.db");
        let _ = std::fs::remove_file(&db);
        let old = super::super::now_ms() - RESUME_GRACE_MS * 10;
        const TOTAL: i64 = 250;
        let mut alive = std::collections::HashSet::new();

        {
            let store = meowo_store::Store::open(&db).unwrap();
            let project = store.upsert_project_by_root("/tmp/p", "p", old).unwrap();
            for i in 0..TOTAL {
                let (sid, _) = store
                    .start_session(project, &format!("cc-{i:04}"), old + i)
                    .unwrap();
                store
                    .set_session_title(sid, &format!("会话 {i}"), old + i)
                    .unwrap();
                // 每 7 条留一个「活着」的会话散布在时间轴各处：它们会被排序顶到每页最前，
                // 正是压垮游标单调性的那批。其余标记 ended。
                if i % 7 == 0 {
                    let pid = 9000 + i;
                    store.set_session_pid(sid, pid, old + i).unwrap();
                    alive.insert(pid);
                } else {
                    store
                        .set_session_status(sid, meowo_store::SessionStatus::Ended, old + i)
                        .unwrap();
                }
            }
        }

        let cache = Mutex::new(meowo_agent::TranscriptCache::default());
        let pty_none = std::collections::HashSet::new();
        let no_runtimes = SessionRuntimeIndex::new();
        let no_approvals = std::collections::HashSet::new();
        let no_screens = std::collections::HashMap::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor: Option<(i64, i64)> = None;
        const PAGE: usize = 100;
        for _ in 0..20 {
            let page = live_sessions_blocking(
                &db,
                &cache,
                LiveContext {
                    alive: &alive,
                    pty_live: &pty_none,
                    runtimes: &no_runtimes,
                    approvals: &no_approvals,
                    screen_states: &no_screens,
                },
                "all",
                None,
                None,
                PageReq {
                    before_last_event_at: cursor.map(|c| c.0),
                    before_id: cursor.map(|c| c.1),
                    limit: PAGE,
                },
            )
            .unwrap();
            for item in &page.items {
                assert!(
                    seen.insert(item.inner.session.id),
                    "游标翻页重复返回了会话 {}",
                    item.inner.session.id,
                );
            }
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = Some((next.last_event_at, next.id));
        }

        assert_eq!(seen.len(), TOTAL as usize, "游标翻页漏掉了会话");
        let _ = std::fs::remove_file(&db);
    }

    /// limit 是 IPC/远程桥入参：超大值必须先经 MAX_PAGE_LIMIT 钳制，否则
    /// `Vec::with_capacity(limit)` 的巨额分配失败会直接 abort 掉整个进程。
    /// 这里直接传 usize::MAX——未钳制的实现根本走不到断言。
    #[test]
    fn oversized_page_limit_is_clamped() {
        let dir = std::env::temp_dir().join(format!("meowo-limit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("limit.db");
        let _ = std::fs::remove_file(&db);
        let old = super::super::now_ms() - RESUME_GRACE_MS * 10;

        {
            let store = meowo_store::Store::open(&db).unwrap();
            let project = store.upsert_project_by_root("/tmp/p", "p", old).unwrap();
            for i in 0..3 {
                let (sid, _) = store
                    .start_session(project, &format!("cc-{i}"), old + i)
                    .unwrap();
                store
                    .set_session_title(sid, &format!("会话 {i}"), old + i)
                    .unwrap();
                store
                    .set_session_status(sid, meowo_store::SessionStatus::Ended, old + i)
                    .unwrap();
            }
        }

        let cache = Mutex::new(meowo_agent::TranscriptCache::default());
        let alive = std::collections::HashSet::new();
        let pty_none = std::collections::HashSet::new();
        let no_runtimes = SessionRuntimeIndex::new();
        let no_approvals = std::collections::HashSet::new();
        let no_screens = std::collections::HashMap::new();
        let page = live_sessions_blocking(
            &db,
            &cache,
            LiveContext {
                alive: &alive,
                pty_live: &pty_none,
                runtimes: &no_runtimes,
                approvals: &no_approvals,
                screen_states: &no_screens,
            },
            "all",
            None,
            None,
            PageReq {
                before_last_event_at: None,
                before_id: None,
                limit: usize::MAX,
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 3);
        assert!(page.next_cursor.is_none());
        let _ = std::fs::remove_file(&db);
    }

    /// 拿本机真实 board.db 对账：tab 计数与列表实际能显示的条数差多少。totals 现已按
    /// 列表口径剔除 ping/已结束空壳，差额应接近 0；残余是「非 ended 的断开空壳」（多算）
    /// 与「transcript 补出标题的未命名会话」（少算）两类小头。
    ///   cargo test --lib counts_versus_visible -- --ignored --nocapture
    #[test]
    #[ignore = "读本机真实数据；仅供手动对账"]
    fn counts_versus_visible_rows() {
        let Some(home) = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .ok()
        else {
            return;
        };
        let db = std::path::PathBuf::from(home)
            .join(".meowo")
            .join("board.db");
        if !db.exists() {
            eprintln!("本机没有 ~/.meowo/board.db，跳过");
            return;
        }
        let (total, archived) = super::super::open_store(&db)
            .unwrap()
            .live_sessions_totals()
            .unwrap();
        let cache = Mutex::new(meowo_agent::TranscriptCache::default());
        // alive 传空集 → 全部按「未连接」判定，正是列表过滤最狠的情形（可见条数下界）。
        let alive = std::collections::HashSet::new();
        let pty_none = std::collections::HashSet::new();
        let no_runtimes = SessionRuntimeIndex::new();
        let no_approvals = std::collections::HashSet::new();
        let no_screens = std::collections::HashMap::new();
        let visible = live_sessions_blocking(
            &db,
            &cache,
            LiveContext {
                alive: &alive,
                pty_live: &pty_none,
                runtimes: &no_runtimes,
                approvals: &no_approvals,
                screen_states: &no_screens,
            },
            "all",
            None,
            None,
            PageReq {
                before_last_event_at: None,
                before_id: None,
                limit: total as usize + 100,
            },
        )
        .unwrap()
        .items
        .len();
        // total 含归档，列表 filter="all" 不含——对账要拿 total - archived 去比。
        println!("counts.total (含归档，已剔除空壳) = {total}");
        println!("counts.archived                   = {archived}");
        println!("未归档                            = {}", total - archived);
        println!("列表实际可见                      = {visible}");
        println!(
            "差额（应接近 0）                  = {}",
            total - archived - visible as i64
        );
    }

    #[test]
    fn process_snapshot_is_reused_within_ttl_and_refreshed_afterwards() {
        let cache = ProcessSnapshotCache::default();
        let first = cache.snapshot_with(1_000, || [7].into_iter().collect());
        // TTL 必须罩得住对话窗 650ms 的轮询周期:651ms 后仍复用同一次采样,
        // 否则单开对话窗时每轮都冷采样(Windows 上一次 30-120ms)。
        let reused = cache.snapshot_with(1_651, || panic!("must reuse the cached sample"));
        let refreshed = cache.snapshot_with(1_000 + PROCESS_SNAPSHOT_TTL_MS + 1, || {
            [9].into_iter().collect()
        });

        assert!(Arc::ptr_eq(&first, &reused));
        assert_eq!(*refreshed, [9].into_iter().collect());
    }
}
