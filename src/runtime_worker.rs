//! Versioned line protocol primitives for out-of-process tool workers.
//!
//! Each serialized message is one JSON object without a trailing newline, so it can be
//! passed directly to `qi_subprocess_write_line` and read with
//! `qi_subprocess_read_line[_timeout]`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerMessage {
    Start {
        version: u32,
        request_id: String,
        tool: String,
        input: Value,
        deadline_unix_ms: Option<i64>,
    },
    Cancel {
        version: u32,
        request_id: String,
        reason: String,
    },
    Progress {
        version: u32,
        request_id: String,
        sequence: u64,
        payload: Value,
    },
    Finish {
        version: u32,
        request_id: String,
        code: i64,
        payload: Value,
    },
}

impl WorkerMessage {
    pub fn start(
        request_id: impl Into<String>,
        tool: impl Into<String>,
        input: Value,
        deadline_unix_ms: Option<i64>,
    ) -> Self {
        Self::Start {
            version: WORKER_PROTOCOL_VERSION,
            request_id: request_id.into(),
            tool: tool.into(),
            input,
            deadline_unix_ms,
        }
    }

    pub fn to_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    pub fn from_line(line: &str) -> serde_json::Result<Self> {
        let message: Self = serde_json::from_str(line)?;
        let version = match &message {
            Self::Start { version, .. }
            | Self::Cancel { version, .. }
            | Self::Progress { version, .. }
            | Self::Finish { version, .. } => *version,
        };
        if version != WORKER_PROTOCOL_VERSION {
            return Err(serde::de::Error::custom(format!(
                "unsupported worker protocol version {version}"
            )));
        }
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn protocol_round_trip_is_single_line() {
        let message = WorkerMessage::start("req-1", "lookup", json!({"key": "value"}), Some(42));
        let line = message.to_line().unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(WorkerMessage::from_line(&line).unwrap(), message);
    }

    #[test]
    fn protocol_rejects_unknown_version() {
        let line = r#"{"type":"cancel","version":2,"request_id":"r","reason":"stop"}"#;
        assert!(WorkerMessage::from_line(line).is_err());
    }
}
