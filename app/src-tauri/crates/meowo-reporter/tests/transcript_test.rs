use meowo_agent::plugins::claude::transcript::title_from_transcript;
use std::io::Write;

fn write_tmp(name: &str, body: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

#[test]
fn custom_title_wins_over_ai_title() {
    let body = r#"{"type":"ai-title","aiTitle":"AI Named","sessionId":"s"}
{"type":"user","message":"hi"}
{"type":"custom-title","customTitle":"我的标题","sessionId":"s"}
"#;
    let p = write_tmp("cc_tt_1.jsonl", body);
    assert_eq!(
        title_from_transcript(p.to_str().unwrap()).as_deref(),
        Some("我的标题")
    );
    let _ = std::fs::remove_file(p);
}

#[test]
fn falls_back_to_ai_title() {
    let body = "{\"type\":\"ai-title\",\"aiTitle\":\"Build dashboard\",\"sessionId\":\"s\"}\n";
    let p = write_tmp("cc_tt_2.jsonl", body);
    assert_eq!(
        title_from_transcript(p.to_str().unwrap()).as_deref(),
        Some("Build dashboard")
    );
    let _ = std::fs::remove_file(p);
}

#[test]
fn latest_of_each_kind_wins() {
    let body = r#"{"type":"ai-title","aiTitle":"old","sessionId":"s"}
{"type":"ai-title","aiTitle":"new","sessionId":"s"}
"#;
    let p = write_tmp("cc_tt_3.jsonl", body);
    assert_eq!(
        title_from_transcript(p.to_str().unwrap()).as_deref(),
        Some("new")
    );
    let _ = std::fs::remove_file(p);
}

#[test]
fn none_when_no_title_or_missing_file() {
    let p = write_tmp("cc_tt_4.jsonl", "{\"type\":\"user\"}\n");
    assert_eq!(title_from_transcript(p.to_str().unwrap()), None);
    let _ = std::fs::remove_file(p);
    assert_eq!(title_from_transcript("Z:/nope/none.jsonl"), None);
}

#[test]
fn reconstruct_path_encodes_cwd_and_session() {
    // `reconstruct_transcript_path` 现在的契约是 Some ⇒ 文件存在，不再回吐幻影默认
    // 路径。这里只想钉住 cwd→目录名的编码形状，故走纯拼路径的 `transcript_path_in`
    // （不做存在性校验），避免依赖真实 ~/.claude 里恰好有该文件。
    use meowo_agent::plugins::claude::transcript::transcript_path_in;
    let p = transcript_path_in(
        std::path::Path::new("/home/me/.claude"),
        r"C:\Users\me\proj",
        "abc-123",
    );
    let s = p.to_string_lossy().replace('\\', "/");
    assert!(
        s.ends_with("/.claude/projects/C--Users-me-proj/abc-123.jsonl"),
        "got {s}"
    );
}
