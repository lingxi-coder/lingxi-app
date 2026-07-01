//! Ctrl-R reverse incremental search over the prompt history.
//!
//! An *input overlay* (parent spec §2.5 priority 3): while `Some(_)` in
//! `AppState.history_search`, every key is captured by the search, not the
//! normal editor. Opening Ctrl-R snapshots the current prompt so Esc can
//! restore it; typing filters `AppState.history` (most-recent match first);
//! Ctrl-R again cycles to the next older match; Enter accepts the match into
//! the prompt; Esc cancels.
//!
//! Pure core: `hs_*` functions take `(&HistorySearchState, &[String], ...)`
//! and return a new state (or, for accept/cancel, the resulting prompt text).
//! The live mount in `root.rs` calls the same functions the tests call.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use iocraft::prelude::*;
use crate::render_iocraft::StyleColorIocraftExt;

/// Search-overlay state. `Some(_)` in `AppState.history_search` means the
/// overlay owns all keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySearchState {
    /// The live query the user is typing.
    pub query: String,
    /// Index into `AppState.history` of the current match, or `None` when no
    /// history line contains `query` (drives the `no matching prompt:` label).
    pub match_index: Option<usize>,
    /// The prompt text at the moment search opened — restored on Esc.
    pub saved_prompt: String,
    /// The prompt cursor at the moment search opened — restored on Esc.
    pub saved_cursor: usize,
}

/// Open the overlay, snapshotting the current prompt for Esc-restore.
#[must_use]
pub fn hs_open(prompt: &str, cursor: usize) -> HistorySearchState {
    HistorySearchState {
        query: String::new(),
        match_index: None,
        saved_prompt: prompt.to_string(),
        saved_cursor: cursor,
    }
}

/// Find the newest (`from`-exclusive, walking toward older) history index
/// whose entry contains `query`. `start_below = Some(i)` restricts the search
/// to indices strictly below `i` (used by `hs_cycle` to step to an older
/// match); `None` searches all entries newest-first. Empty `query` → `None`.
#[must_use]
pub fn recompute_match(
    history: &[String],
    query: &str,
    start_below: Option<usize>,
) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    let upper = match start_below {
        Some(0) => return None, // nothing older than index 0
        Some(i) => i,           // search indices [0, i)
        None => history.len(),  // search all, newest-first
    };
    history[..upper]
        .iter()
        .enumerate()
        .rev() // newest-first within the window
        .find(|(_, entry)| entry.contains(query))
        .map(|(i, _)| i)
}

/// Append `ch` to the query and recompute the match from the newest entry.
#[must_use]
pub fn hs_push_char(
    mut st: HistorySearchState,
    ch: char,
    history: &[String],
) -> HistorySearchState {
    st.query.push(ch);
    st.match_index = recompute_match(history, &st.query, None);
    st
}

/// Delete the last char of the query (saturating) and recompute.
#[must_use]
pub fn hs_backspace(mut st: HistorySearchState, history: &[String]) -> HistorySearchState {
    st.query.pop();
    st.match_index = recompute_match(history, &st.query, None);
    st
}

/// Step to the next older match (Ctrl-R pressed again). If there is no older
/// match, keep the current match (no wrap — claude-code beeps/stays).
#[must_use]
pub fn hs_cycle(mut st: HistorySearchState, history: &[String]) -> HistorySearchState {
    if let Some(next) = recompute_match(history, &st.query, st.match_index) {
        st.match_index = Some(next);
    }
    st
}

/// The text to place in the prompt on Enter: the matched history line, or the
/// typed query verbatim when nothing matched.
#[must_use]
pub fn hs_accept(st: &HistorySearchState, history: &[String]) -> String {
    match st.match_index {
        Some(i) => history.get(i).cloned().unwrap_or_else(|| st.query.clone()),
        None => st.query.clone(),
    }
}

/// Result of feeding one key to the active overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HsKeyOutcome {
    /// Overlay stays open with this (possibly updated) state.
    Continue(HistorySearchState),
    /// Enter accepted: put this text in the prompt and close the overlay.
    Accept(String),
    /// Esc cancelled: restore this prompt text + cursor and close.
    Cancel(String, usize),
}

/// Route one key into the overlay. Consumes a crossterm-0.28 `KeyEvent` (the
/// same currency the permission focus-trap uses via `iocraft_to_crossterm028_key`).
#[must_use]
pub fn handle_history_search_key(
    st: HistorySearchState,
    key: &KeyEvent,
    history: &[String],
) -> HsKeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, _) => HsKeyOutcome::Accept(hs_accept(&st, history)),
        (KeyCode::Esc, _) => HsKeyOutcome::Cancel(st.saved_prompt.clone(), st.saved_cursor),
        (KeyCode::Char('r'), m) if m.contains(KeyModifiers::CONTROL) => {
            HsKeyOutcome::Continue(hs_cycle(st, history))
        }
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
            // Ctrl-C inside search behaves as cancel (restore prompt).
            HsKeyOutcome::Cancel(st.saved_prompt.clone(), st.saved_cursor)
        }
        (KeyCode::Backspace, _) => HsKeyOutcome::Continue(hs_backspace(st, history)),
        (KeyCode::Char(c), m) if (m == KeyModifiers::NONE || m == KeyModifiers::SHIFT) => {
            HsKeyOutcome::Continue(hs_push_char(st, c, history))
        }
        // Any other key (arrows, etc.) is captured but inert — the overlay owns
        // focus, so a stray key never leaks to the editor (parent spec §2.5).
        _ => HsKeyOutcome::Continue(st),
    }
}

/// Props for the search overlay row.
#[derive(Default, Props)]
pub struct HistorySearchOverlayProps {
    /// The live query.
    pub query: String,
    /// True when no history line matches (drives the label literal).
    pub failed_match: bool,
}

/// One row rendered above the prompt while search is active. Mirrors
/// claude-code `HistorySearchInput.tsx`: a dim label (`search prompts:` /
/// `no matching prompt:`) and the query after a one-space gap. (PIC-14) The
/// query carries a block cursor at its end (claude-code's `TextInput
/// showCursor cursorOffset={value.length}`), matching `PromptInput`'s own
/// cursor-chunk rendering (no reverse-video primitive in iocraft, so this
/// swaps fg/bg the same way).
#[component]
pub fn HistorySearchOverlay(props: &HistorySearchOverlayProps) -> impl Into<AnyElement<'static>> {
    use crate::components::prompt_input::render_line_with_cursor;
    use unicode_width::UnicodeWidthStr;
    // Literal lock (parent spec §2.8): the two labels are byte-for-byte from
    // claude-code `HistorySearchInput.tsx:18`.
    let label = if props.failed_match {
        "no matching prompt:"
    } else {
        "search prompts:"
    };
    let cursor_col = UnicodeWidthStr::width(props.query.as_str());
    let chunks = render_line_with_cursor(&props.query, Some(cursor_col));
    element! {
        View(flex_direction: FlexDirection::Row, height: 1) {
            Text(content: format!("{label} "), color: crate::theme::TuiTheme::DIM.to_iocraft())
            #(chunks.into_iter().map(|(seg, is_cursor)| {
                if is_cursor {
                    element! {
                        View(background_color: Color::White) {
                            Text(content: seg, color: Color::Black)
                        }
                    }.into_any()
                } else {
                    element! { Text(content: seg, color: crate::theme::TuiTheme::DIM.to_iocraft()) }.into_any()
                }
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn overlay_cursor_split_preserves_text() {
        // (PIC-14) The query's trailing cursor block (color-swapped View/
        // Text) is emitted as a separate span, but iocraft's plain
        // `to_string()` strips styling, so all visible characters still
        // appear in order. The per-chunk split itself is pinned by
        // `render_line_with_cursor`'s own unit tests.
        let mut el = element! {
            HistorySearchOverlay(query: "cargo".to_string(), failed_match: false)
        };
        let out = el.to_string();
        assert!(out.contains("search prompts: cargo"), "got: {out}");
    }

    #[test]
    fn overlay_empty_query_still_renders_label() {
        // Cursor-at-EOL on an empty query renders a synthetic space chunk
        // (render_line_with_cursor's EOL branch) — must not panic or drop
        // the label.
        let mut el = element! {
            HistorySearchOverlay(query: String::new(), failed_match: false)
        };
        let out = el.to_string();
        assert!(out.contains("search prompts:"), "got: {out}");
    }

    #[test]
    fn open_snapshots_prompt_and_starts_empty() {
        let st = hs_open("draft text", 4);
        assert_eq!(st.query, "");
        assert_eq!(st.match_index, None);
        assert_eq!(st.saved_prompt, "draft text");
        assert_eq!(st.saved_cursor, 4);
    }

    fn hist() -> Vec<String> {
        // oldest .. newest (newest at the end, matching AppState.history).
        vec![
            "git status".to_string(),
            "cargo test".to_string(),
            "git commit -m wip".to_string(),
            "cargo build".to_string(),
        ]
    }

    #[test]
    fn recompute_finds_most_recent_match() {
        let h = hist();
        // "cargo" appears at idx 1 and idx 3; newest-first → idx 3.
        assert_eq!(recompute_match(&h, "cargo", None), Some(3));
        // "git" appears at idx 0 and idx 2; newest-first → idx 2.
        assert_eq!(recompute_match(&h, "git", None), Some(2));
        // empty query → no match.
        assert_eq!(recompute_match(&h, "", None), None);
        // no substring → no match.
        assert_eq!(recompute_match(&h, "zzz", None), None);
    }

    #[test]
    fn push_char_updates_query_and_match() {
        let h = hist();
        let mut st = hs_open("", 0);
        st = hs_push_char(st, 'g', &h);
        assert_eq!(st.query, "g");
        // 'g' also matches "car`g`o build" (idx 3) — newest-first wins, so the
        // bare 'g' query resolves to idx 3, NOT idx 2 (the plan's draft comment
        // assumed only git lines contained 'g'; "cargo" does too).
        assert_eq!(st.match_index, Some(3));
        st = hs_push_char(st, 'i', &h); // "gi"
        st = hs_push_char(st, 't', &h); // "git" — now only the git lines match
        assert_eq!(st.match_index, Some(2)); // newest "git commit"
    }

    #[test]
    fn backspace_widens_match_and_clears_to_empty() {
        let h = hist();
        let mut st = hs_open("", 0);
        st = hs_push_char(st, 'z', &h); // no match
        assert_eq!(st.match_index, None);
        st = hs_backspace(st, &h); // query empty again
        assert_eq!(st.query, "");
        assert_eq!(st.match_index, None);
    }

    #[test]
    fn cycle_steps_to_older_match() {
        let h = hist(); // "cargo" at idx 1 and 3
        let mut st = hs_open("", 0);
        st = hs_push_char(st, 'c', &h);
        st = hs_push_char(st, 'a', &h); // "ca" → newest match idx 3
        assert_eq!(st.match_index, Some(3));
        st = hs_cycle(st, &h); // older "cargo" → idx 1
        assert_eq!(st.match_index, Some(1));
        st = hs_cycle(st, &h); // no older match → stays at idx 1
        assert_eq!(st.match_index, Some(1));
    }

    #[test]
    fn accept_returns_match_else_query() {
        let h = hist();
        // "git" matches only the two git lines; newest-first → idx 2.
        let mut st = hs_push_char(hs_open("", 0), 'g', &h);
        st = hs_push_char(st, 'i', &h);
        st = hs_push_char(st, 't', &h); // "git" → idx 2
        assert_eq!(hs_accept(&st, &h), "git commit -m wip");
        st = hs_push_char(hs_open("", 0), 'z', &h); // no match
        assert_eq!(hs_accept(&st, &h), "z"); // accept typed query verbatim
    }

    #[test]
    fn cancel_returns_saved_prompt() {
        let st = hs_open("original draft", 8);
        assert_eq!(
            (st.saved_prompt.clone(), st.saved_cursor),
            ("original draft".to_string(), 8)
        );
    }

    #[test]
    fn key_handler_routes_printable_ctrl_r_enter_esc_backspace() {
        let h = hist();
        let mut st = hs_open("", 0);
        // printable → push
        let out = handle_history_search_key(
            st.clone(),
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &h,
        );
        assert!(matches!(&out, HsKeyOutcome::Continue(s) if s.query == "c"));
        st = match out {
            HsKeyOutcome::Continue(s) => s,
            _ => unreachable!(),
        };
        // Ctrl-R → cycle (still Continue)
        let out = handle_history_search_key(
            st.clone(),
            &KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &h,
        );
        assert!(matches!(out, HsKeyOutcome::Continue(_)));
        // Enter → Accept(text)
        let out = handle_history_search_key(
            st.clone(),
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &h,
        );
        assert!(matches!(out, HsKeyOutcome::Accept(_)));
        // Esc → Cancel(saved_prompt, saved_cursor)
        let out = handle_history_search_key(
            st.clone(),
            &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &h,
        );
        assert!(matches!(out, HsKeyOutcome::Cancel(p, _) if p.is_empty()));
        // Backspace on empty query stays Continue with empty query
        let out = handle_history_search_key(
            st,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &h,
        );
        assert!(matches!(&out, HsKeyOutcome::Continue(s) if s.query.is_empty()));
    }
}
