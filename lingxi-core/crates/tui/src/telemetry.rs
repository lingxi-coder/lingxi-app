//! Re-export of the 4 `tengu_tui_*` event-name constants from
//! `lingxi-telemetry`. Internal callers in `session.rs` use these
//! constants in `tracing::info!(event = ...)` lines.

pub use lingxi_telemetry::tengu::tui::{FIRST_RENDER, RESIZE, SESSION_ENDED, SESSION_STARTED};
