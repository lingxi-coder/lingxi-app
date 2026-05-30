//! Output sinks: plain stdout vs JSON-NDJSON stdout.
//!
//! Locked NDJSON schema per plan M5-12 Task 0 step 3:
//!
//! ```text
//! {"event":"turn_start","ts":"<rfc3339-Z>","session_id":"<uuid>"}
//! {"event":"text","ts":"<rfc3339-Z>","content":"<chunk>"}
//! {"event":"tool_call","ts":"<rfc3339-Z>","tool":"<name>","input":{…}}
//! {"event":"tool_result","ts":"<rfc3339-Z>","tool":"<name>","result":{…}}
//! {"event":"turn_end","ts":"<rfc3339-Z>","stop_reason":"<r>","cost":{…}}
//! {"event":"command_output","ts":"<rfc3339-Z>","name":"<n>","display":"<d>"}
//! {"event":"error","ts":"<rfc3339-Z>","code":"<c>","message":"<m>"}
//! ```
//!
//! Every line is a JSON object terminated by `\n` (LF only, even on
//! Windows — `--json` is a machine-readable mode).

use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::io::Write;
use tokio::sync::Mutex;

/// Sink for CLI output. Implementations live in this module.
#[async_trait]
pub trait OutputSink: Send + Sync {
    /// Emit free-form text. In plain mode goes straight to stdout; in JSON
    /// mode goes as a `{"event":"text",…}` line.
    async fn text(&self, s: &str);
    /// Emit a `turn_start` marker. No-op in plain mode.
    async fn turn_start(&self);
    /// Emit a `turn_end` marker with stop reason + cost. No-op in plain
    /// mode (the orchestrator's `OutputStream::emit_end_turn` already
    /// prints to stdout there).
    async fn turn_end(&self, stop_reason: &str, total_usd: f64, in_tokens: u64, out_tokens: u64);
    /// Emit a tool-call announcement.
    async fn tool_call(&self, tool: &str, input: &serde_json::Value);
    /// Emit a tool-result announcement.
    async fn tool_result(&self, tool: &str, result: &serde_json::Value);
    /// Emit the output of a slash command.
    async fn command_output(&self, name: &str, display: &str);
    /// Emit an error. Plain mode goes to stderr; JSON mode goes to stdout.
    async fn error(&self, code: &str, message: &str);
}

/// Plain-text stdout sink (default).
pub struct PlainSink {
    out: Mutex<std::io::Stdout>,
}

impl PlainSink {
    /// Construct a new sink wrapping `std::io::stdout()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Mutex::new(std::io::stdout()),
        }
    }
}

impl Default for PlainSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputSink for PlainSink {
    async fn text(&self, s: &str) {
        let mut g = self.out.lock().await;
        let _ = g.write_all(s.as_bytes());
        let _ = g.flush();
    }
    async fn turn_start(&self) { /* no header in plain mode */
    }
    async fn turn_end(&self, _r: &str, _u: f64, _i: u64, _o: u64) { /* no footer */
    }
    async fn tool_call(&self, tool: &str, _input: &serde_json::Value) {
        let mut g = self.out.lock().await;
        let _ = writeln!(g, "[tool: {tool}]");
        let _ = g.flush();
    }
    async fn tool_result(&self, _tool: &str, _r: &serde_json::Value) {
        /* swallowed in plain mode */
    }
    async fn command_output(&self, _name: &str, display: &str) {
        let mut g = self.out.lock().await;
        let _ = writeln!(g, "{display}");
        let _ = g.flush();
    }
    async fn error(&self, _code: &str, message: &str) {
        eprintln!("lingxi-cli: {message}");
    }
}

/// JSON-NDJSON stdout sink (one JSON object per line).
pub struct JsonSink {
    out: Mutex<std::io::Stdout>,
    session_id: String,
}

impl JsonSink {
    /// Construct a new sink keyed to the given session id (used in
    /// `turn_start` events).
    #[must_use]
    pub fn new(session_id: lingxi_protocol::SessionId) -> Self {
        Self {
            out: Mutex::new(std::io::stdout()),
            session_id: session_id.to_string(),
        }
    }

    async fn emit(&self, obj: serde_json::Value) {
        let mut g = self.out.lock().await;
        let _ = writeln!(g, "{obj}");
        let _ = g.flush();
    }

    fn ts() -> String {
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }
}

#[async_trait]
impl OutputSink for JsonSink {
    async fn text(&self, s: &str) {
        self.emit(json!({"event": "text", "ts": Self::ts(), "content": s}))
            .await;
    }
    async fn turn_start(&self) {
        self.emit(json!({
            "event": "turn_start",
            "ts": Self::ts(),
            "session_id": self.session_id,
        }))
        .await;
    }
    async fn turn_end(&self, stop: &str, usd: f64, i: u64, o: u64) {
        self.emit(json!({
            "event": "turn_end",
            "ts": Self::ts(),
            "stop_reason": stop,
            "cost": {"total_usd": usd, "input_tokens": i, "output_tokens": o},
        }))
        .await;
    }
    async fn tool_call(&self, tool: &str, input: &serde_json::Value) {
        self.emit(json!({
            "event": "tool_call",
            "ts": Self::ts(),
            "tool": tool,
            "input": input,
        }))
        .await;
    }
    async fn tool_result(&self, tool: &str, result: &serde_json::Value) {
        self.emit(json!({
            "event": "tool_result",
            "ts": Self::ts(),
            "tool": tool,
            "result": result,
        }))
        .await;
    }
    async fn command_output(&self, name: &str, display: &str) {
        self.emit(json!({
            "event": "command_output",
            "ts": Self::ts(),
            "name": name,
            "display": display,
        }))
        .await;
    }
    async fn error(&self, code: &str, message: &str) {
        self.emit(json!({
            "event": "error",
            "ts": Self::ts(),
            "code": code,
            "message": message,
        }))
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_format_rfc3339_z_seconds() {
        let s = JsonSink::ts();
        // Format: "YYYY-MM-DDTHH:MM:SSZ" (no millis, no offset, terminal Z).
        assert!(s.ends_with('Z'));
        assert_eq!(s.len(), 20);
    }

    #[tokio::test]
    async fn plain_sink_command_output_writes_to_stdout() {
        // We can't capture stdout from inside the same process easily;
        // this test exists to guarantee the method doesn't panic with
        // an unusual character set.
        let sink = PlainSink::new();
        sink.command_output("clear", "Conversation cleared.").await;
    }
}
