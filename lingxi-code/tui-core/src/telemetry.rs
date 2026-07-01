//! Re-export of the `tengu_tui_*` event-name constants from
//! `lingxi-telemetry`. Internal callers in `session.rs` use these
//! constants in `tracing::info!(event = ...)` lines.
//!
//! Inventory (13 events at M7-16):
//! - M6-01: `SESSION_STARTED`, `SESSION_ENDED`, `FIRST_RENDER`, `RESIZE`.
//! - M6-03: `STREAMING_RENDER_STARTED`, `STREAMING_RENDER_ENDED`.
//! - M6-05: `PERMISSION_DIALOG_SHOWN`, `PERMISSION_DIALOG_RESOLVED`.
//! - M6-09: `SCROLL_STARTED`, `SCROLL_ENDED`.
//! - M7-16: `SCREEN_OPENED`, `SCREEN_CLOSED`, `SEARCH_OPENED` — the §2.7
//!   candidates that gained REAL emit sites this milestone (screen open/close
//!   on `active_screen None↔Some`; `MessageSelector` open). The remaining
//!   candidates (`command_palette_opened`, `vim_mode_entered`, `key_pressed`)
//!   stay DEFERRED — no clean/aggregated emit site, so registering them would
//!   mint dead names (the M6 "330 vs 326" lesson).

use permission::gate::PermissionResponse;

pub use telemetry::tengu::tui::{
    FIRST_RENDER, PERMISSION_DIALOG_RESOLVED, PERMISSION_DIALOG_SHOWN, RESIZE, SCREEN_CLOSED,
    SCREEN_OPENED, SCROLL_ENDED, SCROLL_STARTED, SEARCH_OPENED, SESSION_ENDED, SESSION_STARTED,
    STREAMING_RENDER_ENDED, STREAMING_RENDER_STARTED,
};

// M7-16: the `tengu_tui_search_opened` candidate is now REGISTERED + emitted
// (`search_opened` below, called from `MessageSelectorState::open` /
// `open_export` — the Ctrl-T / `/export` overlay open). Note: the search this
// event names is the MessageSelector message-search overlay, NOT the M7-10
// Ctrl-R history search (which remains a 0-event input overlay). The total is
// locked at 330 in the M7-16 audit — per the M6 "report the real number"
// lesson (parent spec §2.7).

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

/// (M7-16) Fire when a full-page screen opens (`AppState.active_screen`
/// transitions `None → Some(_)`). `screen` is one of `"doctor"`, `"resume"`,
/// `"settings"`, `"memory"`, `"theme"`, `"background_tasks"` (M9-05),
/// `"agents"` (M9-08). Called from each `AppState::open_*` helper and the
/// M9-05 background-tasks opener in `root`.
pub fn screen_opened(screen: &str) {
    tracing::info!(
        target: "lingxi.tengu",
        event = SCREEN_OPENED,
        screen = screen,
    );
}

/// (M7-16) Fire when the active screen closes back to the REPL
/// (`AppState.active_screen` transitions `Some(_) → None`). Called from
/// `AppState::close_screen`, guarded so it fires only when a screen was open.
pub fn screen_closed() {
    tracing::info!(
        target: "lingxi.tengu",
        event = SCREEN_CLOSED,
    );
}

/// (M7-16) Fire when the `MessageSelector` search/jump/export overlay opens
/// (Ctrl-T or `/export`). `mode` is one of `"search"`, `"export"`. Called from
/// `MessageSelectorState::open` / `open_export`.
pub fn search_opened(mode: &str) {
    tracing::info!(
        target: "lingxi.tengu",
        event = SEARCH_OPENED,
        mode = mode,
    );
}
