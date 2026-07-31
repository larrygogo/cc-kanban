//! `meowo-app` 与 `meowo-reporter` 的本地 broker 协议。

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const APPROVAL_BROKER_FILE: &str = "approval-broker.json";
pub const LEGACY_ATTACH: &str = "MEOWO1";
pub const LEGACY_CLAIM: &str = "MEOWOCLAIM1";
pub const LEGACY_APPROVAL: &str = "MEOWOAPPROVAL1";
pub const MAX_HANDSHAKE_BYTES: usize = 32 * 1024;
pub const CURRENT_PROTOCOL_VERSION: u16 = 2;
pub const V2_MAGIC: &[u8; 4] = b"MWO2";

/// GUI broker 的发现文件。`pid` 用于拒绝崩溃后遗留的过期端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerDiscovery {
    pub endpoint: String,
    pub token: String,
    pub pid: u32,
    /// 旧 discovery 没有该字段，反序列化为 0 并继续使用 v1。
    #[serde(default)]
    pub protocol_version: u16,
}

/// PermissionRequest 在 reporter 与 GUI 之间传输的稳定形态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub session_id: i64,
    pub request_id: String,
    pub provider: String,
    pub tool_name: String,
    pub description: Option<String>,
    pub input: String,
    /// Agent 提供的“记住此决定”等原生权限更新。旧 reporter 不发送该字段，按空列表处理。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_suggestions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    AllowWithPermissions(Vec<serde_json::Value>),
    Deny,
    Pass,
}

impl ApprovalDecision {
    pub fn as_wire(&self) -> String {
        match self {
            Self::Allow => "allow".into(),
            Self::AllowWithPermissions(updated_permissions) => serde_json::json!({
                "behavior": "allow",
                "updatedPermissions": updated_permissions,
            })
            .to_string(),
            Self::Deny => "deny".into(),
            Self::Pass => "pass".into(),
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim() {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "pass" => Some(Self::Pass),
            encoded => {
                let value: serde_json::Value = serde_json::from_str(encoded).ok()?;
                if value.get("behavior").and_then(|v| v.as_str()) != Some("allow") {
                    return None;
                }
                let permissions = value.get("updatedPermissions")?.as_array()?.clone();
                Some(Self::AllowWithPermissions(permissions))
            }
        }
    }
}

/// 已发布版本使用的单行握手。v2 上线期间服务端继续解码它，旧 reporter/attach 因此无需同步升级。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyHandshake {
    Attach {
        token: String,
        session_id: i64,
        cols: u16,
        rows: u16,
        nonce: String,
    },
    Claim {
        token: String,
        launch_token: String,
        session_id: i64,
    },
    Approval {
        token: String,
        request: ApprovalRequest,
    },
}

/// v2 的统一请求体。JSON 负责字段演进，外层长度帧负责可靠地划定边界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrokerRequest {
    Attach {
        token: String,
        session_id: i64,
        cols: u16,
        rows: u16,
        nonce: String,
        /// attach 客户端自身 pid，GUI 端据此反查宿主终端窗口做精确聚焦。
        /// 旧 reporter（v2 无此字段 / v1 五段行）不发送 → None，聚焦退回应用级兜底。
        #[serde(default)]
        pid: Option<u32>,
    },
    Claim {
        token: String,
        launch_token: String,
        session_id: i64,
    },
    Approval {
        token: String,
        request: ApprovalRequest,
    },
}

impl From<LegacyHandshake> for BrokerRequest {
    fn from(value: LegacyHandshake) -> Self {
        match value {
            LegacyHandshake::Attach {
                token,
                session_id,
                cols,
                rows,
                nonce,
            } => Self::Attach {
                token,
                session_id,
                cols,
                rows,
                nonce,
                pid: None,
            },
            LegacyHandshake::Claim {
                token,
                launch_token,
                session_id,
            } => Self::Claim {
                token,
                launch_token,
                session_id,
            },
            LegacyHandshake::Approval { token, request } => Self::Approval { token, request },
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("握手为空或类型未知")]
    UnknownKind,
    #[error("握手字段数量无效")]
    InvalidShape,
    #[error("握手数字字段无效")]
    InvalidNumber,
    #[error("审批载荷无效")]
    InvalidApproval,
    #[error("握手超过大小限制")]
    TooLarge,
    #[error("握手读写失败")]
    Io,
    #[error("v2 协议版本无效")]
    InvalidVersion,
    #[error("v2 JSON 载荷无效")]
    InvalidJson,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V2Envelope {
    version: u16,
    request: BrokerRequest,
}

/// 从连接首帧读取 v1 单行或 v2 长度帧，并统一成同一个请求枚举。
pub fn read_handshake(reader: &mut impl Read) -> Result<BrokerRequest, ProtocolError> {
    let mut prefix = [0u8; 4];
    reader
        .read_exact(&mut prefix)
        .map_err(|_| ProtocolError::Io)?;
    if &prefix == V2_MAGIC {
        let mut length = [0u8; 4];
        reader
            .read_exact(&mut length)
            .map_err(|_| ProtocolError::Io)?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_HANDSHAKE_BYTES {
            return Err(ProtocolError::TooLarge);
        }
        let mut payload = vec![0; length];
        reader
            .read_exact(&mut payload)
            .map_err(|_| ProtocolError::Io)?;
        let envelope: V2Envelope =
            serde_json::from_slice(&payload).map_err(|_| ProtocolError::InvalidJson)?;
        if envelope.version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidVersion);
        }
        return Ok(envelope.request);
    }

    let mut line = prefix.to_vec();
    while line.len() <= MAX_HANDSHAKE_BYTES {
        let mut byte = [0u8; 1];
        reader
            .read_exact(&mut byte)
            .map_err(|_| ProtocolError::Io)?;
        if byte[0] == b'\n' {
            let line = std::str::from_utf8(&line).map_err(|_| ProtocolError::InvalidShape)?;
            return decode_legacy_handshake(line).map(Into::into);
        }
        line.push(byte[0]);
    }
    Err(ProtocolError::TooLarge)
}

pub fn write_v2_handshake(
    writer: &mut impl Write,
    request: &BrokerRequest,
) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(&V2Envelope {
        version: CURRENT_PROTOCOL_VERSION,
        request: request.clone(),
    })
    .map_err(|_| ProtocolError::InvalidJson)?;
    if payload.len() > MAX_HANDSHAKE_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    writer.write_all(V2_MAGIC).map_err(|_| ProtocolError::Io)?;
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .map_err(|_| ProtocolError::Io)?;
    writer.write_all(&payload).map_err(|_| ProtocolError::Io)
}

pub fn decode_legacy_handshake(line: &str) -> Result<LegacyHandshake, ProtocolError> {
    if line.len() > MAX_HANDSHAKE_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    let parts = line.split_whitespace().collect::<Vec<_>>();
    match parts.first().copied() {
        Some(LEGACY_ATTACH) if parts.len() == 6 => Ok(LegacyHandshake::Attach {
            token: parts[1].to_string(),
            session_id: parts[2].parse().map_err(|_| ProtocolError::InvalidNumber)?,
            cols: parts[3].parse().map_err(|_| ProtocolError::InvalidNumber)?,
            rows: parts[4].parse().map_err(|_| ProtocolError::InvalidNumber)?,
            nonce: parts[5].to_string(),
        }),
        Some(LEGACY_CLAIM) if parts.len() == 4 => Ok(LegacyHandshake::Claim {
            token: parts[1].to_string(),
            launch_token: parts[2].to_string(),
            session_id: parts[3].parse().map_err(|_| ProtocolError::InvalidNumber)?,
        }),
        Some(LEGACY_APPROVAL) if parts.len() == 3 => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(parts[2])
                .map_err(|_| ProtocolError::InvalidApproval)?;
            let request =
                serde_json::from_slice(&bytes).map_err(|_| ProtocolError::InvalidApproval)?;
            Ok(LegacyHandshake::Approval {
                token: parts[1].to_string(),
                request,
            })
        }
        Some(LEGACY_ATTACH) | Some(LEGACY_CLAIM) | Some(LEGACY_APPROVAL) => {
            Err(ProtocolError::InvalidShape)
        }
        _ => Err(ProtocolError::UnknownKind),
    }
}

pub fn encode_legacy_attach(
    token: &str,
    session_id: &str,
    cols: u16,
    rows: u16,
    nonce: &str,
) -> String {
    format!("{LEGACY_ATTACH} {token} {session_id} {cols} {rows} {nonce}\n")
}

pub fn encode_legacy_claim(token: &str, launch_token: &str, session_id: i64) -> String {
    format!("{LEGACY_CLAIM} {token} {launch_token} {session_id}\n")
}

pub fn encode_legacy_approval(
    token: &str,
    request: &ApprovalRequest,
) -> Result<String, serde_json::Error> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(request)?);
    Ok(format!("{LEGACY_APPROVAL} {token} {encoded}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_and_approval_keep_the_existing_camel_case_wire_shape() {
        let discovery = BrokerDiscovery {
            endpoint: "127.0.0.1:1234".into(),
            token: "secret".into(),
            pid: 7,
            protocol_version: CURRENT_PROTOCOL_VERSION,
        };
        assert_eq!(
            serde_json::to_value(discovery).unwrap(),
            serde_json::json!({"endpoint":"127.0.0.1:1234","token":"secret","pid":7,"protocolVersion":2})
        );
        let legacy: BrokerDiscovery = serde_json::from_value(
            serde_json::json!({"endpoint":"127.0.0.1:1","token":"old","pid":8}),
        )
        .unwrap();
        assert_eq!(legacy.protocol_version, 0);

        let request = ApprovalRequest {
            session_id: 9,
            request_id: "request-9".into(),
            provider: "codex".into(),
            tool_name: "Bash".into(),
            description: None,
            input: "{}".into(),
            permission_suggestions: vec![],
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["sessionId"], 9);
        assert_eq!(value["requestId"], "request-9");
        assert_eq!(value["toolName"], "Bash");
    }

    #[test]
    fn approval_decisions_round_trip_and_unknown_values_pass_to_the_tui() {
        for decision in [
            ApprovalDecision::Allow,
            ApprovalDecision::Deny,
            ApprovalDecision::Pass,
        ] {
            assert_eq!(
                ApprovalDecision::from_wire(&decision.as_wire()),
                Some(decision)
            );
        }
        let remembered = ApprovalDecision::AllowWithPermissions(vec![serde_json::json!({
            "type": "addRules",
            "behavior": "allow",
            "destination": "localSettings",
        })]);
        assert_eq!(
            ApprovalDecision::from_wire(&remembered.as_wire()),
            Some(remembered)
        );
        assert_eq!(ApprovalDecision::from_wire("future-value"), None);
    }

    #[test]
    fn legacy_handshakes_keep_exact_published_bytes_and_round_trip() {
        let attach = encode_legacy_attach("token", "17", 80, 24, "nonce1234");
        assert_eq!(attach, "MEOWO1 token 17 80 24 nonce1234\n");
        assert_eq!(
            decode_legacy_handshake(attach.trim_end()).unwrap(),
            LegacyHandshake::Attach {
                token: "token".into(),
                session_id: 17,
                cols: 80,
                rows: 24,
                nonce: "nonce1234".into(),
            }
        );

        let claim = encode_legacy_claim("token", "launch", 17);
        assert_eq!(claim, "MEOWOCLAIM1 token launch 17\n");
        assert!(matches!(
            decode_legacy_handshake(claim.trim_end()).unwrap(),
            LegacyHandshake::Claim { session_id: 17, .. }
        ));

        let request = ApprovalRequest {
            session_id: 17,
            request_id: "request-17".into(),
            provider: "codex".into(),
            tool_name: "Bash".into(),
            description: None,
            input: "{}".into(),
            permission_suggestions: vec![],
        };
        let approval = encode_legacy_approval("token", &request).unwrap();
        assert!(matches!(
            decode_legacy_handshake(approval.trim_end()).unwrap(),
            LegacyHandshake::Approval { request: decoded, .. } if decoded == request
        ));
    }

    #[test]
    fn legacy_decoder_rejects_malformed_unknown_and_oversized_input() {
        assert_eq!(decode_legacy_handshake(""), Err(ProtocolError::UnknownKind));
        assert_eq!(
            decode_legacy_handshake("MEOWO1 token bad 80 24 nonce1234"),
            Err(ProtocolError::InvalidNumber)
        );
        assert_eq!(
            decode_legacy_handshake("MEOWOAPPROVAL1 token !!!"),
            Err(ProtocolError::InvalidApproval)
        );
        assert_eq!(
            decode_legacy_handshake(&"x".repeat(MAX_HANDSHAKE_BYTES + 1)),
            Err(ProtocolError::TooLarge)
        );
    }

    /// attach 去重要精确聚焦宿主窗口，靠客户端上报自身 pid；字段演进走 serde default：
    /// 旧 reporter（v2 无 pid 字段 / v1 五段行）→ None，新字段 round-trip 保留。
    #[test]
    fn attach_pid_defaults_to_none_and_round_trips() {
        let old: BrokerRequest = serde_json::from_value(serde_json::json!({
            "kind": "attach",
            "token": "t",
            "session_id": 1,
            "cols": 80,
            "rows": 24,
            "nonce": "nonce1234"
        }))
        .unwrap();
        assert!(matches!(old, BrokerRequest::Attach { pid: None, .. }));

        let request = BrokerRequest::Attach {
            token: "t".into(),
            session_id: 1,
            cols: 80,
            rows: 24,
            nonce: "nonce1234".into(),
            pid: Some(4242),
        };
        let mut framed = Vec::new();
        write_v2_handshake(&mut framed, &request).unwrap();
        assert_eq!(read_handshake(&mut framed.as_slice()).unwrap(), request);

        let legacy = encode_legacy_attach("t", "1", 80, 24, "nonce1234");
        assert!(matches!(
            read_handshake(&mut legacy.as_bytes()).unwrap(),
            BrokerRequest::Attach { pid: None, .. }
        ));
    }

    #[test]
    fn stream_decoder_accepts_v1_and_v2_as_the_same_request() {
        let expected = BrokerRequest::Claim {
            token: "token".into(),
            launch_token: "launch".into(),
            session_id: 17,
        };
        let legacy = encode_legacy_claim("token", "launch", 17);
        assert_eq!(read_handshake(&mut legacy.as_bytes()).unwrap(), expected);

        let mut framed = Vec::new();
        write_v2_handshake(&mut framed, &expected).unwrap();
        assert_eq!(&framed[..4], V2_MAGIC);
        assert_eq!(read_handshake(&mut framed.as_slice()).unwrap(), expected);
    }

    #[test]
    fn v2_rejects_unknown_versions_truncation_and_oversized_frames() {
        let payload = serde_json::to_vec(&serde_json::json!({
            "version": 99,
            "request": {"kind":"claim","token":"t","launch_token":"l","session_id":1}
        }))
        .unwrap();
        let mut wrong_version = V2_MAGIC.to_vec();
        wrong_version.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        wrong_version.extend_from_slice(&payload);
        assert_eq!(
            read_handshake(&mut wrong_version.as_slice()),
            Err(ProtocolError::InvalidVersion)
        );

        let mut truncated = V2_MAGIC.to_vec();
        truncated.extend_from_slice(&10_u32.to_be_bytes());
        truncated.extend_from_slice(b"{}");
        assert_eq!(
            read_handshake(&mut truncated.as_slice()),
            Err(ProtocolError::Io)
        );

        let mut oversized = V2_MAGIC.to_vec();
        oversized.extend_from_slice(&((MAX_HANDSHAKE_BYTES + 1) as u32).to_be_bytes());
        assert_eq!(
            read_handshake(&mut oversized.as_slice()),
            Err(ProtocolError::TooLarge)
        );
    }
}
