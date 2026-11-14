//! The one JSON contract for buffered results, and NDJSON event lines for
//! streams — the same shapes Burrow's engine uses:
//!
//! `{"ok":true,"engine":"sift","version":"0.1.0","command":"clean","data":{…}}`
//! `{"ok":false,…,"error":{"kind":"refused","message":"…"}}`

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
pub struct EngineError {
    pub kind: String,
    pub message: String,
}

impl EngineError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        EngineError { kind: kind.into(), message: message.into() }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for EngineError {}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::new("io", e.to_string())
    }
}

/// Successful envelope.
pub fn ok<T: Serialize>(command: &str, data: &T) -> Value {
    json!({
        "ok": true,
        "engine": crate::ENGINE,
        "version": crate::VERSION,
        "command": command,
        "data": serde_json::to_value(data).unwrap_or(Value::Null),
    })
}

/// Failed envelope.
pub fn err(command: &str, e: &EngineError) -> Value {
    json!({
        "ok": false,
        "engine": crate::ENGINE,
        "version": crate::VERSION,
        "command": command,
        "error": { "kind": e.kind, "message": e.message },
    })
}

/// One NDJSON event line (no trailing newline).
pub fn event_line<T: Serialize>(event: &T) -> String {
    serde_json::to_string(event).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_shapes() {
        let v = ok("status", &json!({"a": 1}));
        assert_eq!(v["ok"], true);
        assert_eq!(v["engine"], "sift");
        assert_eq!(v["command"], "status");
        assert_eq!(v["data"]["a"], 1);
        assert!(v.get("error").is_none());

        let e = err("clean", &EngineError::new("refused", "nope"));
        assert_eq!(e["ok"], false);
        assert_eq!(e["error"]["kind"], "refused");
        assert!(e.get("data").is_none());
    }
}
