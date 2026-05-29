//! Re-export of the `tengu_tui_*` event-name constants from
//! `lingxi-telemetry`. Internal callers in `session.rs` use these
//! constants in `tracing::info!(event = ...)` lines.
//!
//! Inventory (10 events at M6-09):
//! - M6-01: `SESSION_STARTED`, `SESSION_ENDED`, `FIRST_RENDER`, `RESIZE`.
//! - M6-03: `STREAMING_RENDER_STARTED`, `STREAMING_RENDER_ENDED`.
//! - M6-05: `PERMISSION_DIALOG_SHOWN`, `PERMISSION_DIALOG_RESOLVED`.
//! - M6-09: `SCROLL_STARTED`, `SCROLL_ENDED`.
//! - M7-11: screen lifecycle events (`tengu_tui_screen_opened` /
//!   `_closed`) are a CANDIDATE but DEFERRED to the M7-16 telemetry audit.
//!   M7-11 adds 0 new events (baseline stays 326). Do not register a name
//!   here without a real emit site — that is the M6 "330 vs 326" lesson.

use lingxi_permission::gate::PermissionResponse;

pub use lingxi_telemetry::tengu::tui::{
    FIRST_RENDER, PERMISSION_DIALOG_RESOLVED, PERMISSION_DIALOG_SHOWN, RESIZE, SCROLL_ENDED,
    SCROLL_STARTED, SESSION_ENDED, SESSION_STARTED, STREAMING_RENDER_ENDED,
    STREAMING_RENDER_STARTED,
};

// M7-10 (history search + image paste) adds ZERO telemetry events. Baseline
// stays 326 (`registry_is_exactly_326_entries` holds). The
// `tengu_tui_search_opened` candidate (Ctrl-R open count) is DEFERRED to the
// M7-16 telemetry audit, which locks the real total — per the M6
// "330-vs-326, report the real number" lesson (parent spec §2.7). Do NOT mint
// it here.

/// (M6-05) Fire when a permission dialog transitions from `None` to
/// `Some(_)`. `kind` is one of `"tool_use" / "exit_plan_mode" /
/// "bypass_permissions"`.
pub fn permission_dialog_shown(kind: &str) {
    tracing::info!(
        target: "lingxi.tengu",
        event = PERMISSION_DIALOG_SHOWN,
        kind = kind,
    );
}

/// (M6-05) Fire when the user resolves a permission dialog.
pub fn permission_dialog_resolved(
    kind: &str,
    response: PermissionResponse,
    persist: bool,
    elapsed_ms: u64,
) {
    let decision = match response {
        PermissionResponse::AllowOnce => "allow_once",
        PermissionResponse::AllowAlways => "allow_always",
        PermissionResponse::Deny => "deny",
    };
    tracing::info!(
        target: "lingxi.tengu",
        event = PERMISSION_DIALOG_RESOLVED,
        kind = kind,
        decision = decision,
        persist = persist,
        elapsed_ms = elapsed_ms,
    );
}

/// (M6-09) Fire when the scrollback enters scroll mode (`scroll_offset`
/// transitions `0 → non-zero`). `offset` is the new non-zero offset.
pub fn scroll_started(offset: usize) {
    tracing::info!(
        target: "lingxi.tengu",
        event = SCROLL_STARTED,
        offset = offset,
    );
}

/// (M6-09) Fire when the scrollback exits scroll mode (`scroll_offset`
/// transitions `non-zero → 0` — back at the bottom).
pub fn scroll_ended() {
    tracing::info!(
        target: "lingxi.tengu",
        event = SCROLL_ENDED,
    );
}
