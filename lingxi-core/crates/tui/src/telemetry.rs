//! Re-export of the `tengu_tui_*` event-name constants from
//! `lingxi-telemetry`. Internal callers in `session.rs` use these
//! constants in `tracing::info!(event = ...)` lines.
//!
//! Inventory (6 events at M6-03):
//! - M6-01: `SESSION_STARTED`, `SESSION_ENDED`, `FIRST_RENDER`, `RESIZE`.
//! - M6-03: `STREAMING_RENDER_STARTED`, `STREAMING_RENDER_ENDED`.

pub use lingxi_telemetry::tengu::tui::{
    FIRST_RENDER, RESIZE, SESSION_ENDED, SESSION_STARTED, STREAMING_RENDER_ENDED,
    STREAMING_RENDER_STARTED,
};
