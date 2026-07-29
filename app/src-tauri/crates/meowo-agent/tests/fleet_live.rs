//! 用**本机真实的 claude 进程索引**验证后台会话识别。数据不存在时自动跳过——CI 与
//! 他人机器上不会失败。
//!
//! 存在的理由与 `subagent_live` 相同：`sessions/<pid>.json` 是 Claude Code 的内部落盘约定，
//! 没有文档、随版本演化。合成用例只能证明「按我以为的格式解析是对的」，证明不了「格式还是
//! 我以为的那个」——而一旦 `kind` 改名或换值，后台会话就会悄悄退化成普通卡片，接管按钮
//! 重新出现，用户又会点到一条注定失败的路上去，且没有任何报错。

use meowo_agent::plugins::claude::fleet::CLAUDE_RUNTIME;
use meowo_agent::RuntimeCap;
use std::path::PathBuf;

fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    let dir = home.join(".claude").join("sessions");
    dir.is_dir().then_some(dir)
}

/// 独立于被测代码再解析一遍真实文件，两边对账。
fn raw_entries() -> Vec<(String, bool)> {
    let Some(dir) = sessions_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter_map(|value| {
            let id = value.get("sessionId")?.as_str()?.to_string();
            let kind = value.get("kind").and_then(|k| k.as_str());
            Some((id, kind == Some("bg")))
        })
        .collect()
}

/// 出身（转入后台 / 预热待命）与作业状态同样是没文档的内部约定，且分散在三处文件里：
/// `state` 与 `interactiveLineage` 在 `jobs/<short>/state.json`，`dispatch.source` 在
/// `daemon/roster.json`。任何一处改名，卡片就会退回「一屏后台会话全写着运行中、看不出
/// 哪张是从哪来的」——正是这套标注要解决的问题，而且同样不会报错。
#[test]
fn real_background_jobs_still_tell_us_where_they_came_from() {
    let index = CLAUDE_RUNTIME.session_runtimes();
    let backgrounds: Vec<_> = index.iter().filter(|(_, r)| r.background).collect();
    if backgrounds.is_empty() {
        eprintln!("跳过：本机当前没有后台会话");
        return;
    }
    for (id, runtime) in &backgrounds {
        eprintln!(
            "{id} job={:?} state={:?} 转入后台={} 预热={}",
            runtime.job_id, runtime.job_state, runtime.from_interactive, runtime.spare
        );
    }
    // 作业目录里的记录必有 `state`。一个都读不到 = 字段没了或目录布局变了。
    assert!(
        backgrounds.iter().any(|(_, r)| r.job_state.is_some()),
        "本机有后台会话，却一个作业状态都读不出来——jobs/<short>/state.json 的字段变了"
    );
}

#[test]
fn real_claude_session_index_is_still_shaped_the_way_we_read_it() {
    let raw = raw_entries();
    if raw.is_empty() {
        eprintln!("跳过：本机没有 ~/.claude/sessions 索引（未装 claude 或版本过旧）");
        return;
    }
    let index = CLAUDE_RUNTIME.session_runtimes();

    // 每份真实索引都得进得了表。进不去 = sessionId 字段没了，或目录布局变了。
    for (session_id, _) in &raw {
        assert!(
            index.contains_key(session_id),
            "真实索引里的会话 {session_id} 没被认出来——sessions/*.json 的字段或位置变了"
        );
    }
    // 后台标记必须与文件里的 kind 一致。这里刻意不要求本机**存在** bg 会话（多数时候没有），
    // 只要求「有的话读得对」；一条都没有时打印出来，免得测试静默地什么也没验证。
    let backgrounds: Vec<&str> = raw
        .iter()
        .filter(|(_, bg)| *bg)
        .map(|(id, _)| id.as_str())
        .collect();
    if backgrounds.is_empty() {
        eprintln!("提示：本机当前没有 kind=\"bg\" 的会话，只验证了交互式会话那一半");
    } else {
        // 排查用：`-- --nocapture` 能直接看到后端认出了哪几个，省得靠 GUI 反推。
        eprintln!("本机后台会话 {} 个：{backgrounds:?}", backgrounds.len());
    }
    for (session_id, expected) in &raw {
        assert_eq!(
            index[session_id].background, *expected,
            "会话 {session_id} 的后台标记与 sessions/*.json 里的 kind 对不上"
        );
    }
}
