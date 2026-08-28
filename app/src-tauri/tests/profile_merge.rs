//! 端到端：「合并进默认账号」真的把数据目录、会话归属、设置都并过去了吗。
//!
//! 这是合并命令的安全命门。单测覆盖了 `merge_dir_no_overwrite` 的不覆盖语义与
//! history.jsonl 的追加去重，但「校验 profile → 拒绝进行中会话 → 递归并入 → rehome →
//! 更新 settings → 删目录」这条链路要跨 settings/DB/真实文件系统，任何一环接错
//! （路径解析、顺序、凭据被覆盖）单测都发现不了。
//!
//! 跑在**独立进程**里（集成测试各有自己的二进制）：它要设 `CLAUDE_CONFIG_DIR` /
//! `MEOWO_DB` 这类进程级环境变量，与 lib 单测并行会互相串味。本文件内的用例必须
//! 串行——故只写一个用例，分阶段断言。

use std::path::PathBuf;

fn tmp_dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("meowo-merge-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("建临时目录");
    p
}

fn read_settings(meowo_dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(meowo_dir.join("settings.json")).expect("读 settings.json"),
    )
    .expect("settings.json 应为合法 JSON")
}

#[test]
fn merge_profile_into_default_end_to_end() {
    // claude 默认账号的数据目录（CLAUDE_CONFIG_DIR 指向它，变体表据此解析默认落点）。
    let claude_dir = tmp_dir("claude");
    // Meowo 自己的数据目录（MEOWO_DB 决定 board.db / settings.json / profiles 根落在哪）。
    let meowo_dir = tmp_dir("meowo");
    std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir);
    std::env::set_var("MEOWO_DB", meowo_dir.join("board.db"));

    // settings：一个自定义账号 work（且是活跃账号）。
    std::fs::write(
        meowo_dir.join("settings.json"),
        serde_json::json!({
            "profiles": { "claude": [{ "id": "work", "name": "工作" }] },
            "active_profile": { "claude": "work" }
        })
        .to_string(),
    )
    .unwrap();

    // profile 目录：凭据 + 嵌套的会话数据 + history。
    let profile_dir = meowo_dir.join("profiles").join("claude").join("work");
    std::fs::create_dir_all(profile_dir.join("projects/p1")).unwrap();
    std::fs::write(profile_dir.join(".credentials.json"), "profile-creds").unwrap();
    std::fs::write(profile_dir.join("projects/p1/a.jsonl"), "session-data").unwrap();
    std::fs::write(profile_dir.join("history.jsonl"), "{\"h\":1}\n{\"h\":2}\n").unwrap();

    // 默认账号目录：自己的凭据（绝不能被碰）+ 与 profile 有一行重叠的 history。
    std::fs::write(claude_dir.join(".credentials.json"), "default-creds").unwrap();
    std::fs::write(claude_dir.join("history.jsonl"), "{\"h\":1}\n").unwrap();

    // DB：work 名下两条会话（一条 running、一条 ended），另有默认账号的一条对照。
    let db = meowo_dir.join("board.db");
    let store = meowo_store::Store::open(&db).unwrap();
    let project = store.upsert_project_by_root("C:/root", "root", 100).unwrap();
    let (running, _) = store.start_session(project, "cc-running", 100).unwrap();
    let (ended, _) = store.start_session(project, "cc-ended", 200).unwrap();
    let (_default, _) = store.start_session(project, "cc-default", 300).unwrap();
    store.set_session_profile(running, Some("work")).unwrap();
    store.set_session_profile(ended, Some("work")).unwrap();
    store
        .set_session_status(ended, meowo_store::SessionStatus::Ended, 300)
        .unwrap();

    // ── 阶段 1：该账号还有进行中的会话 → 必须拒绝合并，且 settings/DB/文件都不动 ──
    let err = meowo_app_lib::profile::merge_into_default("claude", "work").unwrap_err();
    // S-9：错误是结构化 reason 码（前端 i18n/errors.ts 按当前语言映射成用户文案），
    // 这里钉住前后端约定的码，防漂移。
    assert!(
        err.contains("profile/has-live-sessions"),
        "拒绝理由应是前后端约定的 reason 码：{err}"
    );
    assert_eq!(
        std::fs::read_to_string(claude_dir.join(".credentials.json")).unwrap(),
        "default-creds"
    );
    assert!(
        !claude_dir.join("projects/p1/a.jsonl").exists(),
        "被拒绝的合并不得动任何文件"
    );
    let s = read_settings(&meowo_dir);
    assert!(
        s["profiles"]["claude"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "work"),
        "被拒绝的合并不得动 settings"
    );
    assert_eq!(
        store.session_profile(running).unwrap().as_deref(),
        Some("work"),
        "被拒绝的合并不得动会话归属"
    );

    // ── 阶段 2：结束那条会话 → 合并成功 ──
    store
        .set_session_status(running, meowo_store::SessionStatus::Ended, 400)
        .unwrap();
    meowo_app_lib::profile::merge_into_default("claude", "work").unwrap();

    // 铁律一：默认账号的凭据原封不动（不覆盖语义）。
    assert_eq!(
        std::fs::read_to_string(claude_dir.join(".credentials.json")).unwrap(),
        "default-creds"
    );
    // profile 独有的文件（含子目录里的）并了过来。
    assert_eq!(
        std::fs::read_to_string(claude_dir.join("projects/p1/a.jsonl")).unwrap(),
        "session-data"
    );
    // history.jsonl 追加去重：两边的历史都保住，重叠行只留一份。
    assert_eq!(
        std::fs::read_to_string(claude_dir.join("history.jsonl")).unwrap(),
        "{\"h\":1}\n{\"h\":2}\n"
    );

    // 会话归属改挂默认账号。
    assert_eq!(store.session_profile(running).unwrap(), None);
    assert_eq!(store.session_profile(ended).unwrap(), None);

    // settings：profile 移除、活跃标记清除。
    let s = read_settings(&meowo_dir);
    assert!(
        !s["profiles"]["claude"]
            .as_array()
            .map(|list| list.iter().any(|p| p["id"] == "work"))
            .unwrap_or(false),
        "合并后 settings 里不该再有这个账号"
    );
    assert!(
        s["active_profile"].get("claude").is_none(),
        "合并后活跃账号应落回默认账号"
    );

    // profile 目录已删（数据在默认目录已有副本）。
    assert!(!profile_dir.exists());

    let _ = std::fs::remove_dir_all(&meowo_dir);
    let _ = std::fs::remove_dir_all(&claude_dir);
}
