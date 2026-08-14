//! hooks 自动接线的**编排层**：启动时对检测到已安装的 agent 幂等挂上 meowo-reporter hooks。
//!
//! 合并逻辑在 `meowo_agent::config` 的格式适配器，通用编排在 `meowo_agent::wiring`，agent 专属的
//! 副作用（claude 的 statusLine、codex 的 trusted_hash）在各自的 `plugins/<id>/setup.rs`。
//!
//! 本模块只提供两样宿主才知道的事实：sidecar 的 meowo-reporter 在哪，meowo 的数据目录在哪。

pub use meowo_agent::config::RepairReason;

use meowo_agent::wiring::WiringContext;
use meowo_agent::AgentId;

/// app 可执行同目录的 meowo-reporter（打包态 sidecar 与 app 放一起）。
pub(crate) fn sibling_reporter() -> Option<String> {
    let bin = if cfg!(windows) {
        "meowo-reporter.exe"
    } else {
        "meowo-reporter"
    };
    let exe = std::env::current_exe().ok()?;
    let sib = exe.with_file_name(bin);
    sib.exists().then(|| sib.to_string_lossy().into_owned())
}

/// meowo 自己的数据目录（`~/.meowo`，board.db 与 statusline.sh 的所在）。
pub(crate) fn meowo_dir() -> std::path::PathBuf {
    crate::db_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
}

/// 自动接线开关：release 恒开；debug 默认关（dev 不抢 `~/.claude/settings.json` 等
/// 全局 hook 槽位与 statusline.sh——那是单槽位，后启动者会静默改指自己的 sidecar，
/// 安装版的接线随即失效）。调试 hook 链路时设 `MEOWO_DEV_WIRE=1` 恢复自动接线；
/// 手动「修复连接」（apply_provider）不经此门，显式动作永远可用。安装版重启后经
/// wiring 的 `bundled.or(claimed)` 自动抢回槽位。
pub(crate) fn auto_wire_enabled() -> bool {
    auto_wire_enabled_from(
        cfg!(debug_assertions),
        std::env::var("MEOWO_DEV_WIRE").ok().as_deref(),
    )
}

/// 纯函数可测：release（debug=false）恒 true；debug 仅 `MEOWO_DEV_WIRE=1` 时 true。
fn auto_wire_enabled_from(debug: bool, flag: Option<&str>) -> bool {
    !debug || flag == Some("1")
}

/// 接线一个 agent。`reporter` 由调用方预解析，避免 apply_all 里逐个重复查 sidecar。
fn wire(plugin: &dyn meowo_agent::AgentPlugin, reporter: Option<&str>) -> Option<RepairReason> {
    let dir = meowo_dir();
    let ctx = WiringContext {
        fallback_reporter: reporter,
        meowo_dir: &dir,
    };
    plugin.wire(&ctx)
}

/// 前代品牌的数据目录（改名前叫 cc-kanban）。升级上来的用户机器上仍留着它。
const LEGACY_DIRS: [&str; 1] = [".cc-kanban"];

/// 清除前代品牌遗留的 statusline 包装脚本。
///
/// 它是那颗 fork 炸弹的另一半：改名换目录后，新版把 `~/.cc-kanban/statusline.sh` 当成「用户
/// 原有的 statusLine 命令」包进 `~/.meowo/statusline.sh`；旧版再跑一次，又把 meowo 的包进它
/// 自己的。两个脚本互相 `bash` 对方，Claude Code 每渲染一次状态栏就点燃一次无限派生。
///
/// 接线时的解链（`plugins/claude/setup.rs` 的 `unwrap_chain`）已经把 settings 的 statusLine
/// 改指回我们自己的干净脚本、环已断开；这里把前代那半个脚本也抹掉，免得它被重新拉进链条
/// （用户手改、或旧版 app 又被运行一次）。
///
/// **必须在接线之后跑**：接线要读这个脚本才能认出它是包装、并从中剥出用户真正的 statusLine
/// 命令。先删就等于把用户的原命令一起丢了。
///
/// 只删带我方生成标记的文件，同名的用户自有脚本一概不碰；前代的 board.db 等数据一律保留
/// （用户的历史看板还在里面）。
fn sweep_legacy_wrappers() {
    let Some(home) = meowo_dir().parent().map(|p| p.to_path_buf()) else {
        return;
    };
    for dir in LEGACY_DIRS {
        let script = home.join(dir).join("statusline.sh");
        if meowo_agent::remove_generated_wrapper(&script) {
            eprintln!(
                "Meowo: 已清除前代品牌遗留的 statusline 包装脚本 {}",
                script.display()
            );
        }
    }
}

/// 启动后台线程入口：逐 agent 独立 best-effort，一家失败不影响他家。
/// 未配置过的 agent（数据目录不存在＝没装）跳过，绝不凭空创建它的配置。
pub fn apply_all() {
    let reporter = sibling_reporter();
    let mut claude_wired = false;
    for p in meowo_agent::all() {
        if p.is_configured() {
            let reason = wire(*p, reporter.as_deref());
            if p.id() == meowo_agent::id::CLAUDE {
                claude_wired = reason.is_none();
            }
        }
    }
    // 多账号：每个 profile 都要接一遍自己的线——它们各有一个独立的数据目录，hooks 也各写各的。
    //
    // 不只是「补漏」：agent 会重写自己的配置文件（claude 就会全量重写 settings.json），一次接线
    // 不是一劳永逸的。默认账号靠这里每次启动对齐一次，profile 此前却没有任何对齐时机——
    // 建号时接一次、登录后接一次，之后就再也没人管了。
    //
    // 它同时是 claude 的 `hasCompletedOnboarding` 补写时机（见 `plugins::claude::setup`）：
    // 那个标记要在**登录之后**才补得上，而登录早已结束的老 profile 只能靠这里救。
    wire_all_profiles(reporter.as_deref());
    // 只有 claude 接线**成功**才清前代残留——那时才确知 statusLine 已指向我们自己的脚本、
    // 不再引用前代那个。接线若放弃（配置不可读/写不进去），settings 可能仍指着前代脚本，
    // 此时删它只会把状态栏指向一个不存在的文件。顺序与条件都勿改。
    if claude_wired {
        sweep_legacy_wrappers();
    }
    // 代理同样在启动时对齐一次：用户可能在 Meowo 没运行时手改过 agent 的配置，或换了机器。
    // 与 hooks 接线同为 best-effort，失败只留日志。
    let _ = crate::proxy::apply_to_agent_configs();
}

/// 对指定 agent 强制执行一次接线（不管是否 configured）。用于用户手动点击「修复连接」。
/// 返回 `None` = 已接线/已是目标状态；`Some(reason)` = 未能接线及原因（供前端提示）。
/// 给所有已建的 profile 各接一遍线。best-effort：单个失败只留日志，绝不影响其余。
fn wire_all_profiles(reporter: Option<&str>) {
    let s = crate::settings::load_settings();
    for (provider, list) in &s.profiles {
        let Some(agent) = meowo_agent::by_id(provider) else {
            continue;
        };
        for p in list {
            // 目录还没建出来（用户手删了？）→ 跳过，绝不凭空重建一个空账号。
            if !crate::profile::profile_root(provider, &p.id).is_dir() {
                continue;
            }
            match crate::profile::wire_profile(agent.id(), &p.id) {
                None => eprintln!("Meowo repair[{provider}/{}]: 已接线", p.id),
                Some(reason) => {
                    eprintln!(
                        "Meowo repair[{provider}/{}]: 接线未生效（{reason:?}）",
                        p.id
                    )
                }
            }
        }
    }
    let _ = reporter; // wire_profile 自行解析 reporter（与默认账号同源）
}

pub fn apply_provider(id: AgentId) -> Option<RepairReason> {
    let Some(p) = meowo_agent::by_id(id.as_str()) else {
        eprintln!("Meowo repair[{id}]: 无对应插件，跳过");
        return Some(RepairReason::NotDetected);
    };
    wire(p, sibling_reporter().as_deref())
}

#[cfg(test)]
mod tests {
    use super::auto_wire_enabled_from;

    /// 自动接线门控的三分支：release 无条件接；debug 默认不接（不抢全局 hook 槽位）；
    /// debug 且 MEOWO_DEV_WIRE=1 时恢复接线。纯函数传参，不改进程 env。
    #[test]
    fn auto_wire_gate_branches() {
        // release：flag 无论是什么都接线。
        assert!(auto_wire_enabled_from(false, None));
        assert!(auto_wire_enabled_from(false, Some("0")));
        // debug 默认关；只认字面 "1"。
        assert!(!auto_wire_enabled_from(true, None));
        assert!(!auto_wire_enabled_from(true, Some("0")));
        assert!(!auto_wire_enabled_from(true, Some("true")));
        assert!(auto_wire_enabled_from(true, Some("1")));
    }
}
