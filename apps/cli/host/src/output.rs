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
use client::protocol::text::ClientTextDto;
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use serde::Serialize;
use serde_json::json;
use std::io::{IsTerminal, Write};
use tokio::sync::Mutex;

/// Sink for CLI output. Implementations live in this module.
#[async_trait]
pub trait OutputSink: Send + Sync {
    /// Emit free-form text. In plain mode goes straight to stdout; in JSON
    /// mode goes as a `{"event":"text",…}` line.
    async fn text(&self, s: &str);
    /// Emit source-backed assistant text. Display sinks may use the projection's
    /// safe string view; machine-readable sinks override this to serialize the
    /// exact source string.
    async fn source_text(&self, text: &Utf16JsonProjection) {
        self.text(display_projection(text).as_str()).await;
    }
    /// Emit a `turn_start` marker. No-op in plain mode.
    async fn turn_start(&self);
    /// Emit a `turn_end` marker with stop reason + cost. No-op in plain
    /// mode (the orchestrator's `OutputStream::emit_end_turn` already
    /// prints to stdout there).
    async fn turn_end(&self, stop_reason: &str, total_usd: f64, in_tokens: u64, out_tokens: u64);
    /// Emit a tool-call announcement.
    async fn tool_call(&self, tool: &str, input: &Utf16JsonProjection);
    /// Emit a tool-result announcement.
    async fn tool_result(&self, tool: &str, result: &Utf16JsonProjection);
    /// Emit a coalescible liveness update for a still-running tool.
    async fn tool_heartbeat(&self, id: &str, tool: &str, elapsed_ms: u64);
    /// Emit the output of a slash command.
    async fn command_output(&self, name: &str, display: &str);
    /// Emit source-backed slash-command output. Human terminals may render a
    /// display-safe view; machine sinks preserve the exact UTF-16 units.
    async fn command_output_source(&self, name: &str, display: &Utf16JsonProjection) {
        let display = display_projection(display);
        self.command_output(name, &display).await;
    }
    /// Emit a Mod log line outside assistant/model text.
    async fn mod_log(&self, plugin: &str, text: &Utf16JsonProjection);
    /// Transient notification. Plain output has no persistent toast surface.
    async fn mod_toast(&self, _plugin: &str, _text: &Utf16JsonProjection, _timeout_ms: u64) {}
    /// Pinned status; plain output has no live status surface.
    async fn mod_status(&self, _plugin: &str, _text: Option<&Utf16JsonProjection>) {}
    /// Emit an error. Plain mode goes to stderr; JSON mode goes to stdout.
    async fn error(&self, code: &str, message: &str);
}

/// Plain-text stdout sink (default).
pub struct PlainSink {
    out: Mutex<std::io::Stdout>,
    err: Mutex<std::io::Stderr>,
}

impl PlainSink {
    /// Construct a new sink wrapping `std::io::stdout()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Mutex::new(std::io::stdout()),
            err: Mutex::new(std::io::stderr()),
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
    async fn tool_call(&self, tool: &str, _input: &Utf16JsonProjection) {
        let mut g = self.out.lock().await;
        let _ = writeln!(g, "[tool: {tool}]");
        let _ = g.flush();
    }
    async fn tool_result(&self, _tool: &str, _r: &Utf16JsonProjection) {
        if std::io::stderr().is_terminal() {
            let mut g = self.err.lock().await;
            let _ = write!(g, "\r\x1b[2K");
            let _ = g.flush();
        }
    }
    async fn tool_heartbeat(&self, _id: &str, tool: &str, elapsed_ms: u64) {
        if std::io::stderr().is_terminal() {
            let mut g = self.err.lock().await;
            let seconds = elapsed_ms / 1_000;
            let _ = write!(g, "\r\x1b[2K[tool: {tool} · {seconds}s]");
            let _ = g.flush();
        }
    }
    async fn command_output(&self, _name: &str, display: &str) {
        let mut g = self.out.lock().await;
        let _ = writeln!(g, "{display}");
        let _ = g.flush();
    }
    async fn mod_log(&self, plugin: &str, text: &Utf16JsonProjection) {
        let mut g = self.out.lock().await;
        let _ = writeln!(g, "{plugin}: {}", display_projection(text));
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
    pub fn new(session_id: lingxi_core::types::SessionId) -> Self {
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

    async fn emit_source<T: Serialize>(&self, frame: &T) {
        let Some(encoded) = serialize_source_frame(frame) else {
            return;
        };
        let mut g = self.out.lock().await;
        let _ = writeln!(g, "{encoded}");
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
    async fn source_text(&self, text: &Utf16JsonProjection) {
        self.emit_source(&TextFrame {
            event: "text",
            ts: Self::ts(),
            content: text,
        })
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
    async fn tool_call(&self, tool: &str, input: &Utf16JsonProjection) {
        self.emit_source(&ToolCallFrame {
            event: "tool_call",
            ts: Self::ts(),
            tool,
            input,
        })
        .await;
    }
    async fn tool_result(&self, tool: &str, result: &Utf16JsonProjection) {
        self.emit_source(&ToolResultFrame {
            event: "tool_result",
            ts: Self::ts(),
            tool,
            result,
        })
        .await;
    }
    async fn tool_heartbeat(&self, id: &str, tool: &str, elapsed_ms: u64) {
        self.emit(json!({
            "event": "tool_heartbeat",
            "ts": Self::ts(),
            "id": id,
            "tool": tool,
            "elapsed_ms": elapsed_ms,
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
    async fn command_output_source(&self, name: &str, display: &Utf16JsonProjection) {
        self.emit_source(&CommandOutputFrame {
            event: "command_output",
            ts: Self::ts(),
            name,
            display: client_text_from_source(display),
        })
        .await;
    }
    async fn mod_log(&self, plugin: &str, text: &Utf16JsonProjection) {
        self.emit_source(&UiLogFrame {
            event: "ui_log",
            ts: Self::ts(),
            plugin,
            text: client_text_from_source(text),
        })
        .await;
    }
    async fn mod_toast(&self, plugin: &str, text: &Utf16JsonProjection, timeout_ms: u64) {
        self.emit_source(&UiToastFrame {
            event: "ui_toast",
            ts: Self::ts(),
            plugin,
            text: client_text_from_source(text),
            timeout_ms,
        })
        .await;
    }
    async fn mod_status(&self, plugin: &str, text: Option<&Utf16JsonProjection>) {
        self.emit_source(&UiStatusFrame {
            event: "ui_status",
            ts: Self::ts(),
            plugin,
            text: text.map(client_text_from_source),
        })
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

#[derive(Serialize)]
struct UiLogFrame<'a> {
    event: &'static str,
    ts: String,
    plugin: &'a str,
    text: ClientTextDto,
}

#[derive(Serialize)]
struct UiToastFrame<'a> {
    event: &'static str,
    ts: String,
    plugin: &'a str,
    text: ClientTextDto,
    timeout_ms: u64,
}

#[derive(Serialize)]
struct UiStatusFrame<'a> {
    event: &'static str,
    ts: String,
    plugin: &'a str,
    text: Option<ClientTextDto>,
}

#[derive(Serialize)]
struct TextFrame<'a> {
    event: &'static str,
    ts: String,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    content: &'a Utf16JsonProjection,
}

#[derive(Serialize)]
struct ToolCallFrame<'a> {
    event: &'static str,
    ts: String,
    tool: &'a str,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    input: &'a Utf16JsonProjection,
}

#[derive(Serialize)]
struct ToolResultFrame<'a> {
    event: &'static str,
    ts: String,
    tool: &'a str,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    result: &'a Utf16JsonProjection,
}

#[derive(Serialize)]
struct CommandOutputFrame<'a> {
    event: &'static str,
    ts: String,
    name: &'a str,
    display: ClientTextDto,
}

fn client_text_from_source(text: &Utf16JsonProjection) -> ClientTextDto {
    ClientTextDto::from_code_units(
        text.string_units("")
            .expect("mod output text is a root-string projection"),
    )
}

fn display_projection(text: &Utf16JsonProjection) -> String {
    String::from_utf16_lossy(
        &text
            .string_units("")
            .expect("mod output text is a root-string projection"),
    )
}

fn serialize_source_frame<T: Serialize>(frame: &T) -> Option<String> {
    serde_json::to_string(frame).ok().map(|encoded| {
        encoded
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029")
    })
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

    #[test]
    fn json_mod_output_frame_round_trips_exact_source_text() {
        #[derive(serde::Deserialize)]
        struct CapturedFrame {
            text: client::protocol::text::ClientTextDto,
        }

        let units = vec![b'Q' as u16, 0xd800, b'!' as u16];
        let source =
            Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units.clone())
                .unwrap();
        let frame = UiLogFrame {
            event: "ui_log",
            ts: "2026-10-07T00:00:00Z".to_string(),
            plugin: "review",
            text: client_text_from_source(&source),
        };

        let encoded = serialize_source_frame(&frame).unwrap();
        assert!(encoded.contains(r#""text":"Q\ud800!""#));
        let captured: CapturedFrame = serde_json::from_str(&encoded).unwrap();
        assert_eq!(captured.text.code_units, units);
    }

    #[test]
    fn json_command_output_frame_round_trips_exact_source_text() {
        #[derive(serde::Deserialize)]
        struct CapturedFrame {
            display: ClientTextDto,
        }

        let units = vec![b'R' as u16, 0xd800, 0x03a9, 0xdc00];
        let source =
            Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units.clone())
                .unwrap();
        let frame = CommandOutputFrame {
            event: "command_output",
            ts: "2026-10-07T00:00:00Z".to_string(),
            name: "recap",
            display: client_text_from_source(&source),
        };

        let encoded = serialize_source_frame(&frame).unwrap();
        assert!(encoded.contains(r#""display":"R\ud800Ω\udc00""#));
        let captured: CapturedFrame = serde_json::from_str(&encoded).unwrap();
        assert_eq!(captured.display.code_units, units);
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
