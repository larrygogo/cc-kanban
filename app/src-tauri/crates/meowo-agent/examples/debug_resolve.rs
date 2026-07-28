//! 临时诊断:验证修复后的路径解析与增量读取对指定会话返回什么。
//! 用法: cargo run --example debug_resolve -- <cwd> <session_id> [offset]

use meowo_agent::plugins::claude::transcript::CLAUDE_TRANSCRIPT;
use meowo_agent::transcript::TranscriptSpec;

fn main() {
    let mut args = std::env::args().skip(1);
    let cwd = args.next().expect("cwd");
    let sid = args.next().expect("session_id");
    let offset: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let path = CLAUDE_TRANSCRIPT
        .resolve_transcript_path(None, Some(&cwd), &sid)
        .expect("path");
    eprintln!("resolved: {}", path.display());
    let delta = meowo_agent::read_chat_delta(&CLAUDE_TRANSCRIPT, &path, offset, None);
    eprintln!(
        "offset {} -> {} reset={} items={}",
        offset,
        delta.offset,
        delta.reset,
        delta.items.len()
    );
    for item in delta.items.iter().rev().take(6).collect::<Vec<_>>().iter().rev() {
        let json = serde_json::to_string(item).unwrap_or_default();
        eprintln!("  {}", json.chars().take(140).collect::<String>());
    }
}
