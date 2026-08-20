use meowo_reporter::{db_path, dispatch::dispatch, hook::HookEvent};
use meowo_store::Store;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

mod attach;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "attach") {
        if let Err(error) = attach::run(&args) {
            eprintln!("Meowo attach failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    // 任何错误都吞掉并以 0 退出——绝不阻塞 Agent。显式诊断开关只写 stderr，便于定位
    // hook 已执行但没有入库/没有进入 GUI broker 的问题，默认行为仍完全静默。
    if let Err(error) = run() {
        if std::env::var_os("MEOWO_REPORTER_DEBUG").is_some() {
            eprintln!("Meowo reporter diagnostic: {error}");
        }
    }
    std::process::exit(0);
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;

    // statusline 子命令：解析 CC statusline JSON 写入上下文用量，再把 stdin 原样透传到 stdout，
    // 供管道下游（claude-hud）照常渲染。解析/写库失败都不影响透传。
    if std::env::args().nth(1).as_deref() == Some("statusline") {
        // db_path 为 None（解析不到 home）时跳过写库只做透传——绝不能因此卡住状态栏。
        if let Some(store) = db_path().and_then(|p| Store::open(p).ok()) {
            meowo_reporter::statusline::record(&store, &buf, now_ms());
        }
        // 无下游时这行就是状态栏；被包装脚本链下游时其 stdout 会被丢弃，仅写库生效。
        print!("{}", meowo_reporter::statusline::minimal_line(&buf));
        return Ok(());
    }

    let ev = HookEvent::parse(&buf)?;
    // 解析不到 home 就没有库可写——连审批桥的 discovery 文件都躺在库旁边，整条链路无从
    // 谈起。直接 no-op 返回：main 最终以 0 退出，绝不阻塞 agent。
    let Some(path) = db_path() else {
        return Ok(());
    };
    let store = Store::open(path)?;
    let now = now_ms();
    // agent 提供方：kimi 的 hook 命令带 `--provider kimi`；Claude 不带 → 默认 claude。
    let provider = parse_provider();
    let canonical_event = meowo_agent::by_id(&provider).map_or(ev.hook_event_name.as_str(), |p| {
        p.canonical_event(&ev.hook_event_name)
    });
    // Codex 的 hook 可能继承 workspace 沙箱：能读 ~/.meowo/board.db，却不能写。审批桥不能被
    // 这次遥测写入失败短路，否则 PermissionRequest 永远只会落回终端。先记下错误，完成所有
    // 不依赖写库的 broker 工作后再返回；main 最终仍按契约吞错并以 0 退出。
    let dispatch_error = dispatch(&store, &ev, now, &provider).err();
    if canonical_event == "SessionStart" {
        if let Some(session_id) = store.find_session_id_pub(&ev.session_id)? {
            // claim 带上会话本体 pid：broker 靠它区分「/clear 原地换代」与「会话内 Bash
            // 起的嵌套 agent 继承 PTY 环境变量后误认领」（见 app 侧 pty.rs 的换代守卫）。
            attach::notify_claim(session_id, meowo_reporter::proc::owner_pid(&provider));
        }
    }
    // GUI 审批桥只对「PermissionRequest hook 会阻塞等待并采纳决策输出」的 provider 生效
    // （见 AgentPlugin::permission_hook_decides）。kimi 的该事件是 observation-only 且 5s
    // 超时：若也弹 GUI 审批卡，卡片控制不了真实审批，点「允许」还会错误清掉
    // pending_review——真实提示仍留在终端里等人。这类 provider 的待批状态仍由 dispatch
    // 落库（卡片显示「去终端处理」），只是不接管决策。
    let hook_decides = meowo_agent::by_id(&provider).is_some_and(|p| p.permission_hook_decides());
    if canonical_event == "PermissionRequest" && hook_decides {
        if let Some(session_id) = store.find_session_id_pub(&ev.session_id)? {
            if let Some(decision) = attach::request_approval(
                session_id,
                &provider,
                ev.tool_name.as_deref().unwrap_or("Tool"),
                ev.tool_input.as_ref(),
                &ev.permission_suggestions,
                false,
            ) {
                let (output, settled) = approval_outcome(decision);
                if let Some(output) = output {
                    println!("{output}");
                }
                // 决策已尘埃落定（GUI 里点了允许/拒绝）→ **当场**清「待批准」。不清的话，
                // 这个标记要等下一个 hook 事件（PostToolUse/Stop）才被顺带清掉——被批准的
                // 工具跑多久，卡片就错挂「待批准」多久；拒绝更要等到回合结束的 Stop。
                //
                // best-effort（吞错）：codex 的 hook 可能继承只读沙箱（见上方 dispatch 的注释），
                // 清不掉绝不能影响已经打给 agent 的决策输出。
                if settled {
                    let _ = store.clear_pending_review(session_id, now_ms());
                }
            }
        }
    }
    // AskUserQuestion 代答桥：PreToolUse 是唯一能在表单渲染前拦下提问的闸门
    // （PermissionRequest 层的 allow+updatedInput / deny 都拦不住表单，2.1.234 实测）。
    // broker 挂起等 GUI 作答；答案以 DenyWith 回来 → 输出 permissionDecision deny +
    // reason（模型把它当答复采纳继续）。Allow/Pass 都不输出 → 工具继续、表单照常出现
    // （Allow 正是旧 broker 自动放行段的回包，跨版本兼容在此闭合）。ExitPlanMode 的
    // PreToolUse 不进此分支。
    if canonical_event == "PreToolUse"
        && ev.tool_name.as_deref() == Some("AskUserQuestion")
        && hook_decides
    {
        if let Some(session_id) = store.find_session_id_pub(&ev.session_id)? {
            if let Some(decision) = attach::request_approval(
                session_id,
                &provider,
                "AskUserQuestion",
                ev.tool_input.as_ref(),
                &[],
                true,
            ) {
                let (output, settled) = pretooluse_outcome(decision);
                if let Some(output) = output {
                    println!("{output}");
                }
                // 已代答 → 提问不再悬着，当场清「待回答」（与审批桥同一时序理由）。
                if settled {
                    let _ = store.clear_pending_review(session_id, now_ms());
                }
            }
        }
    }
    if let Some(error) = dispatch_error {
        return Err(Box::new(error));
    }
    Ok(())
}

/// 从命令行解析 `--provider <name>` / `--provider=<name>` 的**原始字符串**，缺省默认 agent。
///
/// 刻意**不**归一到已注册的 `AgentId`：该参数由我们自己写进各 agent 的 hooks 命令行，但**跨版本**
/// 时更新版 meowo 可能写入本版本尚不认识的 id（如 `gemini`）。若在此回退成 `DEFAULT_ID`，dispatch
/// 就会把这个未知会话落库成默认 agent（甚至因默认不写库而落成 NULL）——等于把未知 provider 冒名成
/// claude，正是 `meowo_agent::resolve` 契约要杜绝的。
///
/// 故原样返回：dispatch 把它原样写进 `sessions.provider`（未知值也保留），仅在需要能力时才对
/// **已注册**插件做 `by_id` 查询，查不到就整段降级。缺省（claude 不带 `--provider`）返回默认 id。
fn parse_provider() -> String {
    let args: Vec<String> = std::env::args().collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--provider" {
            if let Some(v) = it.next() {
                return v.clone();
            }
        } else if let Some(v) = a.strip_prefix("--provider=") {
            return v.to_string();
        }
    }
    meowo_agent::DEFAULT_ID.as_str().to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// GUI 审批决策 → （打给 agent 的 hook 输出，审批是否已尘埃落定）。
///
/// 第二个分量决定要不要当场清 `pending_review`：Allow/Deny 都是「有人做了决定」，提示不再悬着；
/// **Pass 不算**——GUI 消费者消失时交还 agent 原终端（不输出 hook 决策即可恢复原生提示），
/// 用户还没批，标记必须留着，否则卡片会在提示仍悬着时谎报「运行中」。
fn approval_outcome(
    decision: meowo_protocol::broker::ApprovalDecision,
) -> (Option<serde_json::Value>, bool) {
    use meowo_protocol::broker::ApprovalDecision as Decision;
    match decision {
        Decision::Allow => (
            Some(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": { "behavior": "allow" }
                }
            })),
            true,
        ),
        Decision::AllowWithPermissions(updated) => (
            Some(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "allow",
                        "updatedPermissions": updated,
                    }
                }
            })),
            true,
        ),
        Decision::Deny => (
            Some(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "deny",
                        "message": "Denied in Meowo."
                    }
                }
            })),
            true,
        ),
        // 审批桥不会产生带文本的拒绝（那是提问代答的形态），到达即按普通拒绝处理、
        // 文本透传——比丢弃决策（放行被拒的工具）安全。
        Decision::DenyWith(message) => (
            Some(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "deny",
                        "message": message,
                    }
                }
            })),
            true,
        ),
        Decision::Pass => (None, false),
    }
}

/// AskUserQuestion 代答桥的决策 → （PreToolUse hook 输出，提问是否已了结）。
///
/// 与 `approval_outcome` 的关键差异：这里**只有 DenyWith（GUI 已代答）产生输出**。
/// Allow/Pass 都静默——工具继续执行、TUI 表单照常出现，提问仍悬着等人（Allow 是旧
/// broker 自动放行段的回包，Pass 是挂起超时/无消费者的降级）；PreToolUse 输出 allow
/// 反而会跳过后续权限流程，绝不能发。
fn pretooluse_outcome(
    decision: meowo_protocol::broker::ApprovalDecision,
) -> (Option<serde_json::Value>, bool) {
    use meowo_protocol::broker::ApprovalDecision as Decision;
    match decision {
        Decision::DenyWith(reason) => (
            Some(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            })),
            true,
        ),
        Decision::Allow | Decision::AllowWithPermissions(_) | Decision::Deny | Decision::Pass => {
            (None, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{approval_outcome, pretooluse_outcome};
    use meowo_protocol::broker::ApprovalDecision;

    /// Allow/Deny = 决策落地：既要输出 hook 决策，也要清「待批准」。此前没有第二个分量，
    /// 批准后卡片会一直挂着「待批准」直到下一个 hook 事件——被批准的工具跑多久就错多久。
    #[test]
    fn allow_and_deny_settle_the_review() {
        let (output, settled) = approval_outcome(ApprovalDecision::Allow);
        assert!(settled);
        assert_eq!(
            output.unwrap()["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );

        let (output, settled) = approval_outcome(ApprovalDecision::Deny);
        assert!(settled);
        assert_eq!(
            output.unwrap()["hookSpecificOutput"]["decision"]["behavior"],
            "deny"
        );

        let suggestion = serde_json::json!({"type": "addRules"});
        let (output, settled) = approval_outcome(ApprovalDecision::AllowWithPermissions(vec![
            suggestion.clone(),
        ]));
        assert!(settled);
        let out = output.unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(
            out["hookSpecificOutput"]["decision"]["updatedPermissions"][0],
            suggestion
        );
    }

    /// Pass = 交还原终端：不输出决策（恢复原生提示），也**不**清「待批准」——用户还没批。
    #[test]
    fn pass_returns_to_terminal_and_keeps_the_review_pending() {
        let (output, settled) = approval_outcome(ApprovalDecision::Pass);
        assert!(output.is_none());
        assert!(!settled);
    }

    /// 代答桥只有 DenyWith 产生 PreToolUse 输出（deny+reason=答案直达模型并了结提问）；
    /// Allow（旧 broker 自动放行回包）/Pass（超时降级）/裸 Deny 一律静默——工具继续、
    /// 表单照常、提问仍悬着。PreToolUse 绝不能输出 allow（会跳过后续权限流程）。
    #[test]
    fn pretooluse_only_answers_settle_the_question() {
        let (output, settled) =
            pretooluse_outcome(ApprovalDecision::DenyWith("【Meowo 代答】晚饭 → 火锅".into()));
        assert!(settled);
        let out = output.unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["hookEventName"],
            "PreToolUse"
        );
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            out["hookSpecificOutput"]["permissionDecisionReason"],
            "【Meowo 代答】晚饭 → 火锅"
        );

        for decision in [
            ApprovalDecision::Allow,
            ApprovalDecision::AllowWithPermissions(vec![]),
            ApprovalDecision::Deny,
            ApprovalDecision::Pass,
        ] {
            let (output, settled) = pretooluse_outcome(decision);
            assert!(output.is_none());
            assert!(!settled);
        }
    }

    /// 审批桥收到 DenyWith（不该发生但协议允许）按普通拒绝处理并透传文本，
    /// 不得静默丢弃决策放行被拒的工具。
    #[test]
    fn approval_outcome_passes_deny_with_message_through() {
        let (output, settled) = approval_outcome(ApprovalDecision::DenyWith("原因".into()));
        assert!(settled);
        let out = output.unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert_eq!(out["hookSpecificOutput"]["decision"]["message"], "原因");
    }
}
