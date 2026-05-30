//! Error type for the TUI session lifecycle.

use std::io;
use thiserror::Error;

/// Errors surfaced by [`crate::run_tui_session`].
#[derive(Debug, Error)]
pub enum TuiError {
    /// Terminal IO error (raw-mode toggle, alt-screen enter/leave, write).
    #[error("terminal io: {0}")]
    Terminal(#[from] io::Error),

    /// An internal mpsc/oneshot channel was closed unexpectedly.
    #[error("internal channel closed")]
    Channel,

    /// The cancellation token tripped before the session finished cleanly.
    #[error("cancelled")]
    Cancelled,

    /// iocraft runtime surfaced an error (string-flattened to avoid leaking
    /// iocraft's internal error type across the public surface).
    #[error("iocraft: {0}")]
    Iocraft(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_error_displays() {
        let e = TuiError::Terminal(io::Error::other("boom"));
        assert!(e.to_string().starts_with("terminal io:"));
    }

    #[test]
    fn channel_error_is_unit() {
        assert_eq!(TuiError::Channel.to_string(), "internal channel closed");
    }

    #[test]
    fn cancelled_error_displays() {
        assert_eq!(TuiError::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn iocraft_error_carries_message() {
        let e = TuiError::Iocraft("render reconcile failed".into());
        assert!(e.to_string().contains("render reconcile failed"));
    }
}
