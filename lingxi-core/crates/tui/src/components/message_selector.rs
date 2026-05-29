//! MessageSelector (M7-14) — search the scrollback, jump back to a message,
//! and export the transcript.
//!
//! Searches [`crate::state::AppState::messages`] by substring, sets
//! `scroll_offset` (M7-03 line model) to jump to a selected message, and
//! exports a plain-text transcript to a default path with overwrite confirm.
