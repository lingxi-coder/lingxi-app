//! Immediate (non-screen) slash-command handlers for the TUI.
//!
//! Unlike the full-page overlays in [`crate::screens`], these commands produce
//! a `system` message + a side effect (and return immediately) rather than
//! mounting a screen. Each is a PURE arg-parser the dispatch intercept drives.

pub mod color;
