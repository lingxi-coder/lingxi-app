//! `tool-computer-use` (M8-P11b) — the `computer` tool.
//!
//! Screen capture + mouse/keyboard automation, routed to `ctx.computer_control`
//! (`Arc<dyn ComputerControl>`). `None` unless a backend is wired — a desktop
//! automation impl, or a mobile `UniFFI` impl. Pure-Rust dispatch.
//!
//! Contract alignment (parity with claude-code `@anthropic-ai/computer-use-mcp`):
//! the input schema follows the Anthropic computer-use convention —
//! `coordinate: [x, y]` / `start_coordinate: [x, y]` tuples, `direction` +
//! `amount` for scroll, `duration` for `wait`/`hold_key`, `region` for `zoom`.
//! The legacy flat `x`/`y`/`dx`/`dy` form is kept as a fallback so existing
//! callers do not break. The screenshot action returns a compact descriptor
//! (the model-facing image-content-block egress for builtin tools is a future
//! seam — see the screenshot arm).
//!
//! Actions whose underlying capability is not yet on the [`ComputerControl`]
//! seam (drag, middle/triple click, mouse down/up, cursor position, hold key,
//! zoom, clipboard, application control) are accepted by the schema and routed,
//! but surface a clear "unsupported" error until the trait gains those methods
//! and a native backend is wired. See the module-level "deferred" notes.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use traits::computer_control::ComputerError;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "computer";

/// Cross-session lock-held error string (parity with `wrapper.tsx`
/// `formatLockHeld`). `holder` is truncated to the first 8 chars, matching the
/// upstream `holder.slice(0, 8)`. The lock itself needs engine wiring (session
/// id, lockfile path, shutdown registration) outside this crate, so the helper
/// lives here ready to use while the acquire path stays deferred.
#[must_use]
pub fn format_lock_held(holder: &str) -> String {
    let short: String = holder.chars().take(8).collect();
    format!(
        "Computer use is in use by another Claude session ({short}…). Wait for that session to finish or run /exit there."
    )
}

/// Enter-notification message when the Esc abort hotkey is registered
/// (`wrapper.tsx` `computer_use_enter`).
pub const NOTIFY_ENTER_ESC: &str = "Claude is using your computer · press Esc to stop";
/// Enter-notification message when only Ctrl+C is available.
pub const NOTIFY_ENTER_CTRL_C: &str = "Claude is using your computer · press Ctrl+C to stop";
/// Exit-notification message at turn end (`cleanup.ts` `computer_use_exit`).
pub const NOTIFY_EXIT: &str = "Claude is done using your computer";

/// `ComputerTool` — screenshot + mouse/keyboard automation.
#[derive(Clone)]
pub struct ComputerTool {
    ctx: BuiltinToolContext,
}

impl ComputerTool {
    /// Construct from the builtin tool context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

/// All actions exposed by the schema. Parity with the upstream MCP tool set
/// (`toolRendering.tsx` `renderToolUseMessage` / `RESULT_SUMMARY` switch cases).
const ACTIONS: &[&str] = &[
    "screenshot",
    "display_size",
    "cursor_position",
    "mouse_move",
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "triple_click",
    "left_click_drag",
    "left_mouse_down",
    "left_mouse_up",
    "type",
    "key",
    "hold_key",
    "scroll",
    "wait",
    "zoom",
    "read_clipboard",
    "write_clipboard",
    "open_application",
];

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ACTIONS
            },
            // Anthropic computer-use convention: [x, y] pixel tuple.
            "coordinate": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0 },
                "minItems": 2,
                "maxItems": 2
            },
            // Drag origin for `left_click_drag`.
            "start_coordinate": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0 },
                "minItems": 2,
                "maxItems": 2
            },
            // Zoom region: [x, y, w, h].
            "region": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0 },
                "minItems": 4,
                "maxItems": 4
            },
            // Scroll direction + tick count (replaces dx/dy).
            "direction": {
                "type": "string",
                "enum": ["up", "down", "left", "right"]
            },
            "amount": { "type": "integer", "minimum": 0 },
            // Seconds to wait / hold a key.
            "duration": { "type": "number", "minimum": 0 },
            // Text for `type` / key name for `key`/`hold_key` / clipboard write.
            "text": { "type": "string" },
            // Bundle id for `open_application`.
            "bundle_id": { "type": "string" },
            // Legacy flat fallbacks (kept so existing callers do not break).
            "x": { "type": "integer", "minimum": 0 },
            "y": { "type": "integer", "minimum": 0 },
            "dx": { "type": "integer" },
            "dy": { "type": "integer" }
        },
        "required": ["action"]
    })
});

/// One-line dim summary per action, mirroring `toolRendering.tsx`
/// `RESULT_SUMMARY`. Surfaced to the user, so wording is byte-identical.
fn result_summary(action: &str) -> Option<&'static str> {
    Some(match action {
        "screenshot" | "zoom" => "Captured",
        "request_access" => "Access updated",
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => {
            "Clicked"
        }
        "type" => "Typed",
        "key" | "hold_key" => "Pressed",
        "scroll" => "Scrolled",
        "left_click_drag" => "Dragged",
        "open_application" => "Opened",
        _ => return None,
    })
}

fn map_err(e: &ComputerError) -> ToolError {
    match e {
        ComputerError::PermissionDenied(m) => ToolError::PermissionDenied(m.clone()),
        other => ToolError::Internal(other.to_string()),
    }
}

/// Read a `[x, y]` tuple under `key`, falling back to the legacy flat `x`/`y`
/// scalars when reading the `"coordinate"` key. Returns `None` if absent.
fn coord(input: &Value, key: &str) -> Option<(u32, u32)> {
    #[allow(clippy::cast_possible_truncation)] // coordinate space never exceeds u32
    if let Some(arr) = input.get(key).and_then(Value::as_array) {
        if arr.len() == 2 {
            if let (Some(x), Some(y)) = (arr[0].as_u64(), arr[1].as_u64()) {
                return Some((x as u32, y as u32));
            }
        }
        return None;
    }
    // Legacy flat fallback only for the primary `coordinate` field.
    if key == "coordinate" {
        let x = input.get("x").and_then(Value::as_u64);
        let y = input.get("y").and_then(Value::as_u64);
        #[allow(clippy::cast_possible_truncation)] // coordinate space never exceeds u32
        if let (Some(x), Some(y)) = (x, y) {
            return Some((x as u32, y as u32));
        }
    }
    None
}

/// Coordinate or an `InvalidInput` error naming the expected fields.
fn require_coord(input: &Value, key: &str) -> Result<(u32, u32), ToolError> {
    coord(input, key).ok_or_else(|| {
        ToolError::InvalidInput(format!("this action requires `{key}` as `[x, y]`"))
    })
}

/// Translate a scroll `direction` + `amount` into `(dx, dy)` ticks, matching the
/// Anthropic convention. Falls back to the legacy flat `dx`/`dy` when no
/// `direction` is supplied. One tick ≈ `amount` (default 3).
fn scroll_delta(input: &Value) -> (i32, i32) {
    #[allow(clippy::cast_possible_truncation)] // scroll amount never exceeds i32
    if let Some(dir) = input.get("direction").and_then(Value::as_str) {
        let amount = input.get("amount").and_then(Value::as_i64).unwrap_or(3) as i32;
        return match dir {
            "up" => (0, -amount),
            "down" => (0, amount),
            "left" => (-amount, 0),
            "right" => (amount, 0),
            _ => (0, 0),
        };
    }
    #[allow(clippy::cast_possible_truncation)] // scroll delta never exceeds i32
    let dx = input.get("dx").and_then(Value::as_i64).unwrap_or(0) as i32;
    #[allow(clippy::cast_possible_truncation)] // scroll delta never exceeds i32
    let dy = input.get("dy").and_then(Value::as_i64).unwrap_or(0) as i32;
    (dx, dy)
}


#[async_trait]
impl Tool for ComputerTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4096
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, input: &Value) -> bool {
        matches!(
            input.get("action").and_then(Value::as_str),
            Some(
                "screenshot"
                    | "display_size"
                    | "cursor_position"
                    | "zoom"
                    | "read_clipboard"
                    | "wait"
            )
        )
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        // Parity note: claude-code does NOT permission-prompt per action — the
        // `mcp__computer-use__*` tools are pre-added to allowedTools, and a
        // dedicated `request_access` tool + per-app allowlist (clipboardRead /
        // clipboardWrite / systemKeyCombos grant flags) handles session-scoped
        // approval. LingXi has no per-app allowlist seam yet, so we mirror the
        // "allowed without prompt" decision and lean on the OS screen-recording
        // / accessibility prompts that gate the native backend. The per-app
        // allowlist + request_access tool are deferred (need an engine seam).
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "computer tool — OS screen-recording/accessibility prompt gates use".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _: &DescriptionOptions) -> String {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("screenshot");
        format!("Computer action: {action}")
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Capture the screen and drive the mouse/keyboard.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(Value::as_str) {
            Some(a) if ACTIONS.contains(&a) => Ok(()),
            Some(a) => Err(ValidationError(format!("unknown action: {a}"))),
            None => Err(ValidationError("`action` is required".into())),
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("screenshot")
            .to_string();

        // `wait` carries no host capability — it only pauses. The actual sleep
        // is intentionally deferred: this crate's `Cargo.toml` is minimal (no
        // async runtime/timer dependency) and is out of edit scope here, so we
        // acknowledge the requested duration without blocking. Wiring the real
        // pause needs a runtime-agnostic timer dep. Handled before the seam
        // check so it succeeds even when no ComputerControl is wired.
        if action == "wait" {
            let secs = input.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
            // Clamp the echoed value so a stray duration can't mislead callers.
            let secs = secs.clamp(0.0, 60.0);
            return Ok(finish(
                json!({ "ok": true, "requested_seconds": secs }),
                &action,
            ));
        }

        let cc = self.ctx.computer_control.as_ref().ok_or_else(|| {
            ToolError::Internal("computer-control not available on this platform".into())
        })?;

        let data = match action.as_str() {
            "screenshot" => {
                let s = cc.screenshot().await.map_err(|e| map_err(&e))?;
                // Compact descriptor — the base64 image is intentionally NOT
                // embedded in the tool result. A real image-content-block egress
                // for BUILTIN tools does not exist yet (only MCP results carry
                // `model_content_blocks`), and there is no native backend
                // producing bytes today. Embedding base64 in `content` would just
                // be JSON-stringified into a multi-MB text blob the model cannot
                // view (a context-bloat regression). Wire a real image block via
                // `model_content_blocks` once both that egress + a backend exist.
                json!({ "width": s.width, "height": s.height, "png_bytes_len": s.png_bytes.len() })
            }
            "display_size" => {
                let (w, h) = cc.display_size().await.map_err(|e| map_err(&e))?;
                json!({ "width": w, "height": h })
            }
            "mouse_move" => {
                let (x, y) = require_coord(&input, "coordinate")?;
                cc.mouse_move(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "left_click" => {
                let (x, y) = require_coord(&input, "coordinate")?;
                cc.left_click(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "right_click" => {
                let (x, y) = require_coord(&input, "coordinate")?;
                cc.right_click(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "double_click" => {
                let (x, y) = require_coord(&input, "coordinate")?;
                cc.double_click(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "type" => {
                let text = input
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("`type` requires `text`".into()))?;
                cc.type_text(text.to_string())
                    .await
                    .map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "key" => {
                let key = input.get("text").and_then(Value::as_str).ok_or_else(|| {
                    ToolError::InvalidInput("`key` requires `text` (the key name)".into())
                })?;
                cc.key(key.to_string()).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "scroll" => {
                let (x, y) = require_coord(&input, "coordinate")?;
                let (dx, dy) = scroll_delta(&input);
                cc.scroll(x, y, dx, dy).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            // ── Schema-accepted, seam-blocked actions ───────────────────────
            // These map onto ComputerControl methods that do not exist yet
            // (middle/triple click, drag, mouse down/up, cursor position, hold
            // key, zoom, clipboard, application control). The contract is in
            // place (schema + dispatch + summaries); wiring them needs new
            // `traits::computer_control::ComputerControl` methods (in the
            // `traits` crate) plus a native backend. Surface a clear
            // "unsupported" error until then so callers fail loudly, not
            // silently. See "deferred" in the task notes.
            "middle_click" | "triple_click" | "left_click_drag" | "left_mouse_down"
            | "left_mouse_up" | "cursor_position" | "hold_key" | "zoom" | "read_clipboard"
            | "write_clipboard" | "open_application" => {
                return Err(ToolError::Internal(format!(
                    "`{action}` is not yet supported by the ComputerControl backend"
                )));
            }
            other => return Err(ToolError::InvalidInput(format!("unknown action: {other}"))),
        };

        Ok(finish(data, &action))
    }
}

/// Attach the user-facing one-line summary (parity with `RESULT_SUMMARY`) to the
/// result payload and box it into a [`ToolCallResult`].
fn finish(mut data: Value, action: &str) -> ToolCallResult {
    if let Some(summary) = result_summary(action) {
        if let Some(obj) = data.as_object_mut() {
            obj.insert("summary".into(), json!(summary));
        }
    }
    ToolCallResult {
        data,
        model_content: None,
        new_messages: vec![],
        context_modifier: None,
        is_error: false,
        mcp_meta: None,
    }
}

/// Register the `computer` tool against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(ComputerTool::new(ctx)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_covers_full_action_set() {
        let schema = &*INPUT_SCHEMA;
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        for expect in [
            "screenshot",
            "display_size",
            "cursor_position",
            "mouse_move",
            "left_click",
            "right_click",
            "middle_click",
            "double_click",
            "triple_click",
            "left_click_drag",
            "left_mouse_down",
            "left_mouse_up",
            "type",
            "key",
            "hold_key",
            "scroll",
            "wait",
            "zoom",
            "read_clipboard",
            "write_clipboard",
            "open_application",
        ] {
            assert!(
                actions.iter().any(|v| v == expect),
                "missing action in schema: {expect}"
            );
        }
    }

    #[test]
    fn schema_has_tuple_and_legacy_coordinate_fields() {
        let props = &INPUT_SCHEMA["properties"];
        // Tuple convention.
        assert_eq!(props["coordinate"]["type"], "array");
        assert_eq!(props["start_coordinate"]["type"], "array");
        assert_eq!(props["region"]["minItems"], 4);
        assert_eq!(props["direction"]["enum"][0], "up");
        assert!(props["amount"].is_object());
        assert!(props["duration"].is_object());
        assert!(props["bundle_id"].is_object());
        // Legacy flat fallback retained.
        assert_eq!(props["x"]["type"], "integer");
        assert_eq!(props["y"]["type"], "integer");
        assert_eq!(props["dx"]["type"], "integer");
    }

    #[test]
    fn coord_reads_tuple_form() {
        let input = json!({ "coordinate": [12, 34] });
        assert_eq!(coord(&input, "coordinate"), Some((12, 34)));
        let start = json!({ "start_coordinate": [5, 6] });
        assert_eq!(coord(&start, "start_coordinate"), Some((5, 6)));
    }

    #[test]
    fn coord_falls_back_to_flat_xy_for_coordinate() {
        let input = json!({ "x": 7, "y": 8 });
        assert_eq!(coord(&input, "coordinate"), Some((7, 8)));
        // start_coordinate has no flat fallback.
        assert_eq!(coord(&input, "start_coordinate"), None);
    }

    #[test]
    fn coord_rejects_malformed_tuple() {
        assert_eq!(coord(&json!({ "coordinate": [1] }), "coordinate"), None);
        assert_eq!(
            coord(&json!({ "coordinate": [1, 2, 3] }), "coordinate"),
            None
        );
        assert_eq!(coord(&json!({}), "coordinate"), None);
    }

    #[test]
    fn scroll_direction_translates_to_delta() {
        assert_eq!(scroll_delta(&json!({ "direction": "up", "amount": 5 })), (0, -5));
        assert_eq!(
            scroll_delta(&json!({ "direction": "down", "amount": 2 })),
            (0, 2)
        );
        assert_eq!(
            scroll_delta(&json!({ "direction": "left", "amount": 4 })),
            (-4, 0)
        );
        assert_eq!(
            scroll_delta(&json!({ "direction": "right", "amount": 1 })),
            (1, 0)
        );
        // Default amount when omitted.
        assert_eq!(scroll_delta(&json!({ "direction": "up" })), (0, -3));
    }

    #[test]
    fn scroll_falls_back_to_flat_delta() {
        assert_eq!(scroll_delta(&json!({ "dx": 3, "dy": -2 })), (3, -2));
        assert_eq!(scroll_delta(&json!({})), (0, 0));
    }

    #[test]
    fn result_summary_matches_upstream_wording() {
        assert_eq!(result_summary("screenshot"), Some("Captured"));
        assert_eq!(result_summary("zoom"), Some("Captured"));
        assert_eq!(result_summary("left_click"), Some("Clicked"));
        assert_eq!(result_summary("middle_click"), Some("Clicked"));
        assert_eq!(result_summary("triple_click"), Some("Clicked"));
        assert_eq!(result_summary("type"), Some("Typed"));
        assert_eq!(result_summary("key"), Some("Pressed"));
        assert_eq!(result_summary("hold_key"), Some("Pressed"));
        assert_eq!(result_summary("scroll"), Some("Scrolled"));
        assert_eq!(result_summary("left_click_drag"), Some("Dragged"));
        assert_eq!(result_summary("open_application"), Some("Opened"));
        assert_eq!(result_summary("request_access"), Some("Access updated"));
        assert_eq!(result_summary("display_size"), None);
        assert_eq!(result_summary("wait"), None);
    }

    #[test]
    fn lock_held_string_is_byte_faithful() {
        assert_eq!(
            format_lock_held("abcdefghijklmnop"),
            "Computer use is in use by another Claude session (abcdefgh…). Wait for that session to finish or run /exit there."
        );
        // Short holder is not padded.
        assert_eq!(
            format_lock_held("ab"),
            "Computer use is in use by another Claude session (ab…). Wait for that session to finish or run /exit there."
        );
    }

    #[test]
    fn notification_strings_are_byte_faithful() {
        assert_eq!(NOTIFY_ENTER_ESC, "Claude is using your computer · press Esc to stop");
        assert_eq!(
            NOTIFY_ENTER_CTRL_C,
            "Claude is using your computer · press Ctrl+C to stop"
        );
        assert_eq!(NOTIFY_EXIT, "Claude is done using your computer");
    }

    #[test]
    fn finish_attaches_summary_only_when_known() {
        let r = finish(json!({ "ok": true }), "left_click");
        assert_eq!(r.data["summary"], "Clicked");
        let r2 = finish(json!({ "ok": true }), "display_size");
        assert!(r2.data.get("summary").is_none());
    }
}
