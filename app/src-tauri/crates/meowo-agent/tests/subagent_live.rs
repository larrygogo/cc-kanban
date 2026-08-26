//! 用**本机真实会话**验证子任务侧车定位。数据不存在时自动跳过——CI 与他人机器上不会失败。
//!
//! 存在的理由：子任务的定位链路依赖各家 CLI 的落盘布局，而那是外部约定、且随版本演化。
//! 合成用例只能证明「按我以为的格式解析是对的」，证明不了「格式还是我以为的那个」。
//! 尤其是「子任务仍在运行」这一路径——结果尚未写入，只能靠开场 prompt 反查——单元测试
//! 极易把它测成永远通过。

use meowo_agent::plugins::claude::transcript::CLAUDE_TRANSCRIPT;
use meowo_agent::plugins::kimi::telemetry::KIMI_TRANSCRIPT;
use meowo_agent::transcript::read_subagent_chat;
use std::path::{Path, PathBuf};

/// 枚举本机所有 kimi 会话的主 wire。
fn kimi_main_wires() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    let sessions = home.join(".kimi-code/sessions");
    let mut out = Vec::new();
    let Ok(workdirs) = std::fs::read_dir(&sessions) else {
        return out;
    };
    for workdir in workdirs.flatten() {
        let Ok(entries) = std::fs::read_dir(workdir.path()) else {
            continue;
        };
        for session in entries.flatten() {
            let wire = session.path().join("agents/main/wire.jsonl");
            if wire.is_file() {
                out.push(wire);
            }
        }
    }
    out
}

/// 主 wire 里所有子任务委派调用的 id（`Agent` 与 `AgentSwarm`），附带它是否已有结果。
fn subagent_calls(wire: &Path) -> Vec<(String, bool)> {
    let Ok(text) = std::fs::read_to_string(wire) else {
        return Vec::new();
    };
    let mut calls: Vec<(String, bool)> = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(event) = value.get("event") else {
            continue;
        };
        let Some(id) = event
            .get("toolCallId")
            .or_else(|| event.get("callId"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        match event.get("type").and_then(|v| v.as_str()) {
            Some("tool.call") => {
                if matches!(
                    event.get("name").and_then(|v| v.as_str()),
                    Some("Agent" | "AgentSwarm")
                ) {
                    calls.push((id.to_string(), false));
                }
            }
            Some("tool.result") => {
                if let Some(entry) = calls.iter_mut().find(|(call, _)| call == id) {
                    entry.1 = true;
                }
            }
            _ => {}
        }
    }
    calls
}

/// 本机所有**含 forked skill** 的 claude 会话主 transcript。
///
/// 先用目录标记筛(`<session>/subagents/*.forked-skill.json`)再读正文:projects 下动辄
/// 上百个会话、单个 transcript 可达数 MB,无差别整读会让这个测试跑成分钟级。
fn claude_forked_transcripts() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(home.join(".claude/projects")) else {
        return out;
    };
    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let main = entry.path();
            if main.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let sidecars = main.with_extension("").join("subagents");
            let Ok(files) = std::fs::read_dir(&sidecars) else {
                continue;
            };
            let forked = files.flatten().any(|f| {
                f.file_name()
                    .to_str()
                    .is_some_and(|n| n.ends_with(".forked-skill.json"))
            });
            if forked {
                out.push(main);
            }
        }
    }
    out
}

/// tool_result 的正文:CC 既可能给字符串,也可能给 `[{type:"text",text}]`。
fn result_text(block: &serde_json::Value) -> String {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// 主链上的 forked skill 委派:(tool_use_id, 是否已结束)。
///
/// 判据只认**回执**——`Skill` 调用本身看不出 fork 与否,这正是这条链路的要害。
fn forked_skill_calls(main: &Path) -> Vec<(String, bool)> {
    let Ok(text) = std::fs::read_to_string(main) else {
        return Vec::new();
    };
    let mut skills: Vec<String> = Vec::new();
    let mut out: Vec<(String, bool)> = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // 子任务自己的委派(isSidechain)不算主会话的。
        if value.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let Some(blocks) = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in blocks {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("tool_use") if block.get("name").and_then(|v| v.as_str()) == Some("Skill") => {
                    if let Some(id) = block.get("id").and_then(|v| v.as_str()) {
                        skills.push(id.to_string());
                    }
                }
                Some("tool_result") => {
                    let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if !skills.iter().any(|s| s == id) {
                        continue;
                    }
                    let head = result_text(block);
                    let head = head.trim_start().lines().next().unwrap_or_default().to_string();
                    if !head.starts_with("Skill \"") || !head.contains("(forked execution") {
                        continue;
                    }
                    out.push((id.to_string(), !head.contains("launched (forked execution")));
                }
                _ => {}
            }
        }
    }
    out
}

/// forked skill(`/code-review` 等)的侧车定位。
///
/// 单列这一条的理由:它与常规 `Agent` 委派**没有共用外键**——fork 出的顶层 agent 的
/// meta 不带 `toolUseId`,只能靠 `/<skill> <args>` 对 `description`、同款多次按序号配对。
/// 这套关联全建立在外部落盘约定上,合成用例只能证明「按我以为的格式解析是对的」。
/// 2026-08-26 实拍:该形态从 7 月底就在,而 GUI 一直没认出来,整场审查的十几个 agent
/// 在进度面板里一个都不显示。
#[test]
fn locates_real_claude_forked_skill_streams() {
    let mains = claude_forked_transcripts();
    if mains.is_empty() {
        eprintln!("跳过：本机没有含 forked skill 的 claude 会话");
        return;
    }
    let (mut total, mut located, mut with_items) = (0, 0, 0);
    for main in &mains {
        for (call, settled) in forked_skill_calls(main) {
            total += 1;
            let runs = read_subagent_chat(&CLAUDE_TRANSCRIPT, main, &call);
            if runs.is_empty() {
                continue;
            }
            located += 1;
            let items: usize = runs.iter().map(|r| r.items.len()).sum();
            if items > 0 {
                with_items += 1;
            }
            eprintln!(
                "  {} [{}] -> {} 条流 / {items} 条目",
                &call[..12.min(call.len())],
                if settled { "已结束" } else { "运行中" },
                runs.len(),
            );
        }
    }
    eprintln!("forked skill 委派 {located}/{total} 可定位；其中 {with_items} 个有可读内容");
    if total == 0 {
        eprintln!("跳过：这些会话里没有 forked skill 委派");
        return;
    }
    // 历史会话的侧车可能已被清理,不苛求全中;但「一个都定位不到」= 关联约定已变,必须红。
    assert!(
        located > 0,
        "{total} 个 forked skill 委派全部定位失败——description 外键或侧车布局已变"
    );
    // 定位到流不等于展开有东西看(这正是用户看到的「空面板」)。
    assert!(
        with_items > 0,
        "{located} 个委派定位到了侧车却全是空流——展开仍是一片空白"
    );
}

/// 进度面板的**前置条件**:面板不读侧车,它认委派靠的是「Skill 调用 + 回执带结局统计」
/// (前端 isSubagentDelegation)。这里用真机数据验这两样在 ChatItem 层真的存在——
/// 用户报的「看不到子任务」正是这一环断掉的表现,而侧车定位得对也救不了它。
#[test]
fn forked_skill_items_carry_what_the_progress_panel_needs() {
    let mains = claude_forked_transcripts();
    if mains.is_empty() {
        eprintln!("跳过：本机没有含 forked skill 的 claude 会话");
        return;
    }
    let (mut delegations, mut with_outcome, mut slash_summary) = (0, 0, 0);
    for main in &mains {
        let delta = meowo_agent::transcript::read_chat_delta(&CLAUDE_TRANSCRIPT, main, 0, None);
        // 委派本体:name=="Skill" 的 tool_use;摘要应是 `/<skill> <args>`(面板行的标题)。
        let mut skill_ids: Vec<String> = Vec::new();
        for item in &delta.items {
            if let meowo_agent::transcript::ChatItem::ToolUse {
                id, name, summary, ..
            } = item
            {
                if name == "Skill" {
                    skill_ids.push(id.clone());
                    if summary.starts_with('/') {
                        slash_summary += 1;
                    }
                }
            }
        }
        // 回执:带 subagent 结局统计的那条才让面板认出委派。
        for item in &delta.items {
            if let meowo_agent::transcript::ChatItem::ToolResult {
                tool_use_id,
                subagent,
                ..
            } = item
            {
                let Some(id) = tool_use_id else { continue };
                if !skill_ids.iter().any(|s| s == id) {
                    continue;
                }
                delegations += 1;
                if subagent.is_some() {
                    with_outcome += 1;
                }
            }
        }
    }
    eprintln!(
        "Skill 回执 {delegations} 条，其中 {with_outcome} 条带结局统计；`/命令` 形态摘要 {slash_summary} 条"
    );
    // 后台 forked skill 的收尾:启动回执标 running 并带 task_id(取自行上的
    // toolUseResult.agentId),结局由只带 <task-id> 的通知按同一个 id 归回。两端任一
    // 缺失,这条委派就永远挂在「运行中」——实拍:一条 11:36 就结束的审查挂了三个半小时。
    let (mut launched, mut with_task_id, mut settled) = (0, 0, 0);
    for main in &mains {
        let delta = meowo_agent::transcript::read_chat_delta(&CLAUDE_TRANSCRIPT, main, 0, None);
        let mut running_ids: Vec<String> = Vec::new();
        let mut settled_ids: Vec<String> = Vec::new();
        for item in &delta.items {
            let meowo_agent::transcript::ChatItem::ToolResult {
                text,
                subagent: Some(outcome),
                ..
            } = item
            else {
                continue;
            };
            if text.trim_start().starts_with("Skill \"") && outcome.running > 0 {
                launched += 1;
                if let Some(id) = &outcome.task_id {
                    with_task_id += 1;
                    running_ids.push(id.clone());
                }
            } else if outcome.running == 0 {
                if let Some(id) = &outcome.task_id {
                    settled_ids.push(id.clone());
                }
            }
        }
        settled += running_ids
            .iter()
            .filter(|id| settled_ids.contains(id))
            .count();
    }
    eprintln!(
        "forked 启动回执 {launched} 条，带 task_id {with_task_id} 条，其中 {settled} 条能靠 task_id 接到结局"
    );
    if launched > 0 {
        // 拿不到 agentId = 两端接不起来 = 必然永挂运行中。
        assert_eq!(
            launched, with_task_id,
            "有 forked 启动回执没拿到 agentId,它的结局通知将无处归属"
        );
    }
    if delegations == 0 {
        eprintln!("跳过：这些会话里没有 Skill 回执");
        return;
    }
    // 一条都不带结局 = 面板永远认不出 forked skill 委派 = 用户看到的空面板。
    assert!(
        with_outcome > 0,
        "{delegations} 条 Skill 回执全都没有结局统计——进度面板认不出 forked skill 委派"
    );
    assert!(slash_summary > 0, "Skill 调用的摘要没有一条是 `/命令` 形态——面板行会没有标题");
}

#[test]
fn locates_real_kimi_subagent_streams_including_still_running_ones() {
    let wires = kimi_main_wires();
    if wires.is_empty() {
        eprintln!("跳过：本机没有 kimi 会话数据");
        return;
    }
    let (mut settled, mut settled_ok, mut running, mut running_ok) = (0, 0, 0, 0);
    for wire in &wires {
        for (call, has_result) in subagent_calls(wire) {
            let found = !read_subagent_chat(&KIMI_TRANSCRIPT, wire, &call).is_empty();
            match has_result {
                true => {
                    settled += 1;
                    settled_ok += usize::from(found);
                }
                false => {
                    running += 1;
                    running_ok += usize::from(found);
                }
            }
        }
    }
    eprintln!("已完成的委派 {settled_ok}/{settled} 可定位；运行中的 {running_ok}/{running} 可定位");
    // 能定位到流不等于流里有可读内容：统计一次 item 构成，避免「展开一片空白」被测成通过。
    if let Some(wire) = wires.iter().find(|wire| !subagent_calls(wire).is_empty()) {
        for (call, _) in subagent_calls(wire).into_iter().take(2) {
            for run in read_subagent_chat(&KIMI_TRANSCRIPT, wire, &call) {
                let mut census = std::collections::BTreeMap::new();
                for item in &run.items {
                    let kind = match item {
                        meowo_agent::transcript::ChatItem::UserText { .. } => "user",
                        meowo_agent::transcript::ChatItem::AssistantText { .. } => "assistant",
                        meowo_agent::transcript::ChatItem::AssistantDelta { .. } => {
                            "assistant_delta"
                        }
                        meowo_agent::transcript::ChatItem::Reasoning { .. } => "reasoning",
                        meowo_agent::transcript::ChatItem::ReasoningDelta { .. } => {
                            "reasoning_delta"
                        }
                        meowo_agent::transcript::ChatItem::ToolUse { .. } => "tool_use",
                        meowo_agent::transcript::ChatItem::ToolResult { .. } => "tool_result",
                        meowo_agent::transcript::ChatItem::TurnError { .. } => "turn_error",
                        meowo_agent::transcript::ChatItem::Meta { .. } => "meta",
                    };
                    *census.entry(kind).or_insert(0usize) += 1;
                }
                eprintln!(
                    "  {} / {:?} -> {:?}",
                    &call[..12.min(call.len())],
                    run.label,
                    census
                );
            }
        }
    }
    if settled == 0 && running == 0 {
        eprintln!("跳过：本机 kimi 会话里没有子任务委派");
        return;
    }
    // 定位靠的是外部落盘约定，个别历史会话可能因目录被清理而落空；要求绝大多数命中即可，
    // 但「一个都定位不到」意味着约定已经变了，必须让测试红。
    let total = settled + running;
    let ok = settled_ok + running_ok;
    assert!(
        ok * 4 >= total * 3,
        "只有 {ok}/{total} 个子任务委派能定位到侧车流——kimi 的落盘布局可能已变"
    );
    if running > 0 {
        assert!(
            running_ok > 0,
            "{running} 个运行中的委派全部定位失败——结果尚未写入时的 prompt 反查已失效"
        );
    }
}
