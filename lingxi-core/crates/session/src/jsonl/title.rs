//! First-user-message title extraction — 1:1 port of
//! `claude-code/src/utils/sessionStorage.ts::enrichLogs::firstPrompt` derivation.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-08-resume.md` Task 0 step 3 for byte-locks.

/// Maximum number of `char`s in the displayed title before truncation + ellipsis.
pub const TITLE_MAX_CHARS: usize = 50;

/// The single-codepoint Unicode ellipsis (U+2026) appended when truncating.
pub const TITLE_ELLIPSIS: char = '…';
