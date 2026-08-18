//! claude 的会话遥测：Stop 正文来自 hook 负载，标题与重命名回写走 transcript。
//! 上下文占用不在此——claude 经 statusline 上报（见 meowo-reporter 的 `statusline`）。

use crate::caps::{HookContext, StopOutputs, TelemetryCap};
use crate::transcript::TranscriptSpec;

use super::transcript::{self as ct, CLAUDE_TRANSCRIPT};

pub struct ClaudeTelemetry;
pub static TELEMETRY: ClaudeTelemetry = ClaudeTelemetry;

impl TelemetryCap for ClaudeTelemetry {
    fn stop_outputs(&self, ctx: &HookContext) -> StopOutputs {
        // Claude 的 Stop hook 直接带 AI 正文；模型走 statusline（不在此处）。
        StopOutputs {
            last_ai: ctx.last_assistant_message.map(str::to_string),
            model: None,
        }
    }

    fn transcript(&self) -> Option<&'static dyn TranscriptSpec> {
        Some(&CLAUDE_TRANSCRIPT)
    }

    fn resolves_transcript_title(&self) -> bool {
        true
    }

    fn write_rename(&self, session_id: &str, cwd: Option<&str>, title: &str) -> bool {
        write_custom_title(session_id, cwd, title)
    }
}

/// 往会话 transcript 追加一条 custom-title 记录（与 Claude Code `/rename` 写入格式一致），
/// 使 `claude --resume` 列表与贴纸都显示新名。定位失败/打开失败/写失败返回 false。
/// session_id 已由命令层校验为安全形态（无路径分隔符/穿越），此处直接拼路径。
fn write_custom_title(session_id: &str, cwd: Option<&str>, title: &str) -> bool {
    use std::io::Write;
    // reconstruct 的 Some ⇒ 文件存在，无需再 filter(exists)。
    let Some(path) = CLAUDE_TRANSCRIPT
        .resolve_cwd(cwd, session_id)
        .and_then(|c| ct::reconstruct_transcript_path(&c, session_id))
        .or_else(|| ct::find_transcript_by_session(session_id))
    else {
        return false;
    };
    let record = serde_json::json!({
        "type": "custom-title",
        "customTitle": title,
        "sessionId": session_id,
    });
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&path) else {
        return false;
    };
    // 先缓冲成完整一行再一次性 write_all：该 transcript 同时被运行中的 claude 进程追加。
    //
    // 保证不撕裂的**不是**「单次 write」，而是 `O_APPEND`（上面 `.append(true)`）——它让每一次
    // write 系统调用都是「原子地定位到末尾再写入」。于是即便 write_all 因短写循环了多次，每次
    // 底层 write 仍各自原子，最坏也只是插在对方两行**之间**，绝不会插进对方**一行的中间**。
    // （`writeln!(f, "{record}")` 则相反：它经 Display 把一行拆成多次小块写，每块之间都是别人
    // 可趁虚而入的窗口，两边的行都会被撕成非法 JSON。故必须先拼成整行再一次交出去。）
    let mut line = record.to_string();
    line.push('\n');
    f.write_all(line.as_bytes()).is_ok()
}
