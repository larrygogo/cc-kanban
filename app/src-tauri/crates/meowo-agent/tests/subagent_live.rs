//! 用**本机真实会话**验证子任务侧车定位。数据不存在时自动跳过——CI 与他人机器上不会失败。
//!
//! 存在的理由：子任务的定位链路依赖各家 CLI 的落盘布局，而那是外部约定、且随版本演化。
//! 合成用例只能证明「按我以为的格式解析是对的」，证明不了「格式还是我以为的那个」。
//! 尤其是「子任务仍在运行」这一路径——结果尚未写入，只能靠开场 prompt 反查——单元测试
//! 极易把它测成永远通过。

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
