//! Ctrl+O transcript toggle (claude-code `app:toggleTranscript` parity): a
//! read-only, scrollable verbose dump of the full message log.
//!
//! In claude-code the transcript mode re-renders the SAME message log in its
//! "verbose" form (every block expanded, nothing folded) over an alternate
//! screen; Ctrl+O / Esc returns to the live REPL. This makes the
//! compact-boundary's `✻ Conversation compacted (ctrl+o for history)` hint
//! functional — the user presses Ctrl+O to read the pre-compaction history.
//!
//! Four-part split mirroring `help.rs`/`skills.rs`/`stats.rs`: a
//! [`TranscriptScreenState`] (a captured snapshot of the scrollback lines + an
//! embedded [`crate::screens::scroll::ScrollState`]), a [`TranscriptOutcome`]
//! enum, a pure [`handle_transcript_key`] reducer (scroll keys delegated to the
//! shared `ScrollState`; Ctrl+O / Esc close), and a pure
//! [`render_transcript_to_string`] oracle.
//!
//! The verbose dump reuses the SAME `render_transcript` formatter `/export`
//! writes (`message_selector::render_transcript`), so the on-screen transcript
//! and the exported `.txt` are byte-identical. Unlike the live REPL scrollback
//! (which folds/collapses and caps how many messages render), the transcript
//! shows EVERY message — the whole retained log — scrollable line-by-line.
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::components::message_selector::render_transcript;
use crate::screens::scroll::{scroll_indicator, visible_slice, ScrollState};
use crate::state::RenderedMessage;

/// Fixed viewport height (body rows shown before scrolling kicks in). A modest
/// constant keeps the pure oracle deterministic and unit-testable; the live
/// render is line-by-line. Mirrors `help.rs`/`skills.rs`.
const VIEWPORT: usize = 16;

/// Locked footer hint (claude-code transcript dismiss). Default keymap → Ctrl+O
/// or Esc both close.
pub const FOOTER: &str = "ctrl+o or esc to close";

/// Screen state: the captured verbose-dump lines + the scroll window over them.
///
/// The lines are SNAPSHOT at open time (the transcript is a read-only view; the
/// live log keeps streaming behind it, but the transcript shows the log as it
/// was when opened — claude-code re-derives on each render, but a snapshot is
/// observably equivalent for a modal view the user dismisses to get back to the
/// live REPL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptScreenState {
    /// The flattened verbose-dump lines (one per physical line of the dump).
    pub lines: Vec<String>,
    /// Scroll window over [`Self::lines`].
    pub scroll: ScrollState,
}

impl TranscriptScreenState {
    /// Build the transcript screen from the live scrollback, capturing the
    /// verbose dump and anchoring the scroll window at the BOTTOM (the most
    /// recent message — where the live REPL was), like claude-code's transcript
    /// which opens scrolled to the tail.
    #[must_use]
    pub fn new(messages: &[RenderedMessage]) -> Self {
        Self::with_viewport(messages, VIEWPORT)
    }

    /// As [`Self::new`] but with an explicit viewport height (the live caller
    /// passes the real scrollback viewport so scrolling matches the screen;
    /// tests pass a small height to exercise the window deterministically).
    #[must_use]
    pub fn with_viewport(messages: &[RenderedMessage], viewport: usize) -> Self {
        let lines = transcript_lines(messages);
        let mut scroll = ScrollState::new(lines.len(), viewport.max(1));
        // Open at the tail (most recent), mirroring the live REPL position.
        scroll.set_offset(scroll.max_offset());
        Self { lines, scroll }
    }
}

/// Flatten the scrollback to the verbose-dump body lines. Reuses the exact
/// `render_transcript` formatter `/export` writes, then splits into physical
/// lines so the embedded [`ScrollState`] can window over them.
#[must_use]
pub fn transcript_lines(messages: &[RenderedMessage]) -> Vec<String> {
    let dump = render_transcript(messages);
    // `render_transcript` ends every message with a trailing `\n`; splitting on
    // `\n` then yields a final empty element we drop so the line count matches
    // the rendered content (no phantom blank tail row).
    let mut lines: Vec<String> = dump.split('\n').map(str::to_string).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Controller outcome after a key (mirrors `HelpOutcome`/`SkillsOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptOutcome {
    /// Stay open (scrolled or inert key).
    Stay,
    /// Close the screen (Ctrl+O toggle-off / Esc).
    Close,
}

/// Reduce one key. Scroll keys (Up/Down/PageUp/PageDown/Home/End) are handled by
/// the embedded [`ScrollState`]; Ctrl+O (the toggle), Esc, and bare `q` all
/// close (`q` mirrors the sibling read-only viewers `help.rs`/`skills.rs`).
/// Everything else is inert. Pure — the caller owns closing + telemetry.
#[must_use]
pub fn handle_transcript_key(state: &mut TranscriptScreenState, key: KeyEvent) -> TranscriptOutcome {
    // Ctrl+O toggles the transcript OFF (the same chord that opened it).
    if key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return TranscriptOutcome::Close;
    }
    // Scroll keys (claude-code modal pager parity); `handle_scroll_key` returns
    // `true` when it consumed the key.
    if state.scroll.handle_scroll_key(key) {
        return TranscriptOutcome::Stay;
    }
    match key.code {
        KeyCode::Esc => TranscriptOutcome::Close,
        // (review) bare `q` also closes, matching help.rs/skills.rs. Only the
        // unmodified key dismisses — a modified `q` falls through.
        KeyCode::Char('q') if key.modifiers == KeyModifiers::NONE => TranscriptOutcome::Close,
        _ => TranscriptOutcome::Stay,
    }
}

/// Pure render oracle: the full screen body as text — the visible window of the
/// captured verbose-dump lines, then (when scrolled) a scroll indicator, then
/// the footer hint. Mirrors `render_skills_to_string`/`render_help_to_string`.
#[must_use]
pub fn render_transcript_to_string(state: &TranscriptScreenState) -> String {
    let mut out = String::new();
    for line in visible_slice(&state.lines, &state.scroll) {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(ind) = scroll_indicator(&state.scroll) {
        out.push_str(&ind);
        out.push('\n');
    }
    out.push_str(FOOTER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn k_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn user(body: &str) -> RenderedMessage {
        RenderedMessage::UserText {
            body: body.to_string(),
            timestamp: 0,
        }
    }

    fn assistant(body: &str) -> RenderedMessage {
        RenderedMessage::AssistantText {
            body: body.to_string(),
            timestamp: 0,
        }
    }

    #[test]
    fn close_on_esc() {
        let mut s = TranscriptScreenState::new(&[user("hi")]);
        assert_eq!(
            handle_transcript_key(&mut s, k(KeyCode::Esc)),
            TranscriptOutcome::Close
        );
    }

    #[test]
    fn close_on_ctrl_o() {
        // The toggle chord that opened the transcript also closes it.
        let mut s = TranscriptScreenState::new(&[user("hi")]);
        assert_eq!(
            handle_transcript_key(&mut s, k_ctrl(KeyCode::Char('o'))),
            TranscriptOutcome::Close
        );
    }

    #[test]
    fn bare_o_does_not_close() {
        // A plain `o` (no Ctrl) is inert — only the chord toggles.
        let mut s = TranscriptScreenState::new(&[user("hi")]);
        assert_eq!(
            handle_transcript_key(&mut s, k(KeyCode::Char('o'))),
            TranscriptOutcome::Stay
        );
    }

    #[test]
    fn close_on_bare_q() {
        // (review) bare `q` closes, matching help.rs/skills.rs; a modified `q`
        // (Ctrl+Q) is inert.
        let mut s = TranscriptScreenState::new(&[user("hi")]);
        assert_eq!(
            handle_transcript_key(&mut s, k(KeyCode::Char('q'))),
            TranscriptOutcome::Close
        );
        assert_eq!(
            handle_transcript_key(&mut s, k_ctrl(KeyCode::Char('q'))),
            TranscriptOutcome::Stay
        );
    }

    #[test]
    fn scroll_keys_move_window_and_stay() {
        // A tall log so scrolling is live (viewport 4 over many lines).
        let msgs: Vec<RenderedMessage> = (0..30).map(|i| user(&format!("m{i}"))).collect();
        let mut s = TranscriptScreenState::with_viewport(&msgs, 4);
        assert!(s.scroll.is_scrollable());
        // Opens anchored at the bottom (tail), like the live REPL.
        assert_eq!(s.scroll.offset(), s.scroll.max_offset());
        // PageUp/Up move the window up; still Stay.
        assert_eq!(
            handle_transcript_key(&mut s, k(KeyCode::PageUp)),
            TranscriptOutcome::Stay
        );
        assert!(s.scroll.offset() < s.scroll.max_offset());
        // Home jumps to the top.
        assert_eq!(
            handle_transcript_key(&mut s, k(KeyCode::Home)),
            TranscriptOutcome::Stay
        );
        assert_eq!(s.scroll.offset(), 0);
        // End jumps back to the bottom.
        assert_eq!(
            handle_transcript_key(&mut s, k(KeyCode::End)),
            TranscriptOutcome::Stay
        );
        assert_eq!(s.scroll.offset(), s.scroll.max_offset());
        // Down past the bottom clamps (still Stay).
        assert_eq!(
            handle_transcript_key(&mut s, k(KeyCode::Down)),
            TranscriptOutcome::Stay
        );
        assert_eq!(s.scroll.offset(), s.scroll.max_offset());
    }

    #[test]
    fn transcript_lines_match_export_formatter() {
        // The on-screen lines join back to the exact `/export` body (single
        // source of truth: `render_transcript`).
        let msgs = vec![user("question"), assistant("answer")];
        let lines = transcript_lines(&msgs);
        assert_eq!(lines, vec!["> question".to_string(), "answer".to_string()]);
    }

    #[test]
    fn render_shows_all_messages_when_scrolled() {
        // Distinct from the live REPL: every message is reachable. The first
        // message is at the top (scroll Home), the last at the bottom.
        let msgs: Vec<RenderedMessage> = (0..30).map(|i| user(&format!("msg{i}"))).collect();
        let mut s = TranscriptScreenState::with_viewport(&msgs, 4);
        // At the tail: the last message is visible.
        let bottom = render_transcript_to_string(&s);
        assert!(bottom.contains("msg29"), "tail visible, got: {bottom}");
        assert!(bottom.ends_with(FOOTER), "footer present");
        // Scroll to the top: the first message is visible.
        let _ = handle_transcript_key(&mut s, k(KeyCode::Home));
        let top = render_transcript_to_string(&s);
        assert!(top.contains("> msg0"), "head visible, got: {top}");
    }

    #[test]
    fn empty_log_renders_just_the_footer() {
        let s = TranscriptScreenState::new(&[]);
        assert!(s.lines.is_empty());
        assert_eq!(render_transcript_to_string(&s), FOOTER);
    }

    #[test]
    fn scroll_indicator_present_when_taller_than_window() {
        let msgs: Vec<RenderedMessage> = (0..30).map(|i| user(&format!("m{i}"))).collect();
        let mut s = TranscriptScreenState::with_viewport(&msgs, 4);
        // At the tail there is hidden content above → an `↑ N more` indicator.
        let out = render_transcript_to_string(&s);
        assert!(out.contains("more"), "scroll indicator present, got: {out}");
        // Scroll to the very top → hidden content below.
        let _ = handle_transcript_key(&mut s, k(KeyCode::Home));
        let top = render_transcript_to_string(&s);
        assert!(top.contains("more"), "indicator at top, got: {top}");
    }
}
