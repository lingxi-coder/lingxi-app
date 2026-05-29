//! `@` file-ref completion: a dropdown of cwd path entries. Opens when an `@`
//! token is being typed. The filter/select logic takes the candidate `Vec`
//! as input so it's testable without touching the filesystem; `read_cwd_entries`
//! is the only fs-touching fn and is exercised by a separate fs test.
//!
//! Literal lock (design §2.8): inserts `@<path> ` (trailing space), mirroring
//! claude-code `QuickOpenDialog` handleInsert. Empty-state strings copied below.

use std::path::Path;

use iocraft::prelude::*;

use super::fuzzy::filtered_ranked;
use crate::theme::TuiTheme;

/// Max dropdown rows (shared with the palette; claude-code `OVERLAY_MAX_ITEMS`).
pub const OVERLAY_MAX_ITEMS: usize = 5;

/// claude-code `QuickOpenDialog` empty-state literal when a query is present.
pub const EMPTY_WITH_QUERY: &str = "No matching files";
/// claude-code `QuickOpenDialog` empty-state literal for an empty query.
pub const EMPTY_NO_QUERY: &str = "Start typing to search…";

/// `@` completion overlay state.
#[derive(Debug, Clone, Default)]
pub struct CompletionState {
    /// Whether the dropdown is shown.
    pub open: bool,
    /// The text typed after the active `@` (the partial path).
    pub filter: String,
    /// Index into filtered candidates.
    pub selected: usize,
    /// Candidate paths (relative to cwd). Populated when the overlay opens.
    pub candidates: Vec<String>,
}

/// Find the active `@` token: the substring from the last `@` to the cursor,
/// iff that `@` is at the start or preceded by whitespace and the token has no
/// space. Returns `(at_byte_index, partial)` or `None`.
#[must_use]
pub fn active_at_token(prompt: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = cursor.min(prompt.len());
    let head = &prompt[..cursor];
    let at = head.rfind('@')?;
    let preceded_ok = at == 0 || head[..at].ends_with(|c: char| c.is_whitespace());
    let partial = &head[at + 1..];
    if preceded_ok && !partial.contains(char::is_whitespace) {
        Some((at, partial.to_string()))
    } else {
        None
    }
}

impl CompletionState {
    /// Recompute open-state + filter from the prompt + cursor, using a
    /// pre-supplied candidate list (so tests inject candidates; the live path
    /// calls `read_cwd_entries` first — see Task 9).
    pub fn sync(&mut self, prompt: &str, cursor: usize, candidates: &[String]) {
        if let Some((_, partial)) = active_at_token(prompt, cursor) {
            if !self.open || partial != self.filter {
                self.selected = 0;
            }
            self.open = true;
            self.filter = partial;
            self.candidates = candidates.to_vec();
            let max = self.rows().len();
            if max == 0 {
                self.selected = 0;
            } else if self.selected >= max {
                self.selected = max - 1;
            }
        } else {
            self.open = false;
            self.filter.clear();
            self.selected = 0;
            self.candidates.clear();
        }
    }

    /// Filtered, ranked candidate paths for the current filter.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        filtered_ranked(&self.filter, &self.candidates)
            .into_iter()
            .map(str::to_string)
            .collect()
    }
}

/// Outcome of a completion key. `Accept` carries the rewritten prompt + cursor
/// because inserting a path edits the buffer in place (replacing the `@token`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKeyOutcome {
    /// Navigation handled; key swallowed.
    Consumed,
    /// Commit the selected path. The dispatcher sets `new_prompt`/`new_cursor`.
    Accept {
        /// The full prompt buffer after inserting `@<path> `.
        new_prompt: String,
        /// The new cursor byte index (end of the inserted token).
        new_cursor: usize,
    },
    /// `Esc` — close; key swallowed.
    Dismiss,
    /// Nothing actionable — fall through to default input.
    PassThrough,
}

impl CompletionState {
    /// Navigation-only handler (no prompt rewrite). Used for Up/Down/Esc.
    pub fn handle_key(&mut self, code: KeyCode) -> CompletionKeyOutcome {
        let len = self.rows().len();
        match code {
            KeyCode::Down => {
                if len > 0 && self.selected + 1 < len {
                    self.selected += 1;
                }
                CompletionKeyOutcome::Consumed
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                CompletionKeyOutcome::Consumed
            }
            KeyCode::Esc => {
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                self.candidates.clear();
                CompletionKeyOutcome::Dismiss
            }
            _ => CompletionKeyOutcome::PassThrough,
        }
    }

    /// Tab/Enter handler that rewrites the prompt in place: replaces the active
    /// `@token` (located via `active_at_token`) with `@<selected> `.
    pub fn handle_key_with_prompt(
        &mut self,
        code: KeyCode,
        prompt: &str,
        cursor: usize,
    ) -> CompletionKeyOutcome {
        match code {
            KeyCode::Tab | KeyCode::Enter => {
                let rows = self.rows();
                let Some(sel) = rows.get(self.selected).cloned() else {
                    return CompletionKeyOutcome::PassThrough;
                };
                let Some((at, _)) = active_at_token(prompt, cursor) else {
                    return CompletionKeyOutcome::PassThrough;
                };
                let cursor = cursor.min(prompt.len());
                let insert = format!("@{sel} ");
                let mut new_prompt = String::with_capacity(prompt.len() + insert.len());
                new_prompt.push_str(&prompt[..at]);
                new_prompt.push_str(&insert);
                let new_cursor = new_prompt.len();
                new_prompt.push_str(&prompt[cursor..]);
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                self.candidates.clear();
                CompletionKeyOutcome::Accept {
                    new_prompt,
                    new_cursor,
                }
            }
            _ => self.handle_key(code),
        }
    }
}

/// Read the immediate (non-recursive) entries of `dir`, excluding dotfiles,
/// returned as file names sorted ASCII-ascending. The only fs-touching fn in
/// this module. Errors → empty list (the overlay just shows the empty state).
#[must_use]
pub fn read_cwd_entries(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    out.sort_unstable();
    out
}

/// Props for the completion dropdown overlay.
#[derive(Default, Props)]
pub struct CompletionOverlayProps {
    /// Filtered candidate paths.
    pub rows: Vec<String>,
    /// Highlighted row index.
    pub selected: usize,
    /// Whether the active `@` token has no partial text yet (drives the
    /// empty-state literal).
    pub empty_query: bool,
}

/// Render the `@` completion dropdown. Each row is `+ <path>` (claude-code
/// file icon `+`). The empty state shows the QuickOpenDialog literal.
#[component]
pub fn CompletionOverlay(props: &CompletionOverlayProps) -> impl Into<AnyElement<'static>> {
    let selected = props.selected;
    if props.rows.is_empty() {
        let msg = if props.empty_query {
            EMPTY_NO_QUERY
        } else {
            EMPTY_WITH_QUERY
        };
        return element! {
            View(height: 1) { Text(content: msg.to_string(), color: TuiTheme::DIM) }
        }
        .into_any();
    }
    let rows: Vec<_> = props.rows.iter().take(OVERLAY_MAX_ITEMS).cloned().collect();
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows.into_iter().enumerate().map(|(i, path)| {
                let line = format!("+ {path}");
                // TODO(M7-15): theme picker adds a dedicated "suggestion" token;
                // until then the selected row reuses ASSISTANT and others DIM.
                let color = if i == selected { TuiTheme::ASSISTANT } else { TuiTheme::DIM };
                element! {
                    View(height: 1) { Text(content: line, color: color) }
                }
            }))
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands() -> Vec<String> {
        vec![
            "src/main.rs".into(),
            "src/lib.rs".into(),
            "README.md".into(),
        ]
    }

    #[test]
    fn active_token_at_start() {
        assert_eq!(active_at_token("@src", 4), Some((0, "src".into())));
    }

    #[test]
    fn active_token_after_whitespace() {
        assert_eq!(active_at_token("see @lib", 8), Some((4, "lib".into())));
    }

    #[test]
    fn no_token_without_at() {
        assert_eq!(active_at_token("plain text", 5), None);
    }

    #[test]
    fn at_mid_word_is_not_a_token() {
        // email-like — `@` not preceded by whitespace → not a completion token.
        assert_eq!(active_at_token("user@host", 9), None);
    }

    #[test]
    fn opens_and_filters_on_at() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(c.open);
        let rows = c.rows();
        assert!(rows.iter().all(|r| r.contains("src")));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn closes_when_token_gone() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(c.open);
        c.sync("plain", 5, &cands());
        assert!(!c.open);
    }

    #[test]
    fn down_up_move_and_clamp() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands()); // 2 rows
        assert_eq!(c.selected, 0);
        c.handle_key(KeyCode::Down);
        assert_eq!(c.selected, 1);
        c.handle_key(KeyCode::Down); // clamp at last
        assert_eq!(c.selected, 1);
        c.handle_key(KeyCode::Up);
        assert_eq!(c.selected, 0);
    }

    #[test]
    fn tab_inserts_path_with_at_and_trailing_space() {
        let mut c = CompletionState::default();
        // prompt is "@s", cursor 2; selecting replaces the @token in place.
        c.sync("@s", 2, &cands());
        let sel = c.rows()[c.selected].clone();
        let outcome = c.handle_key_with_prompt(KeyCode::Tab, "@s", 2);
        match outcome {
            CompletionKeyOutcome::Accept {
                new_prompt,
                new_cursor,
            } => {
                let expected = format!("@{sel} ");
                assert_eq!(new_prompt, expected);
                assert_eq!(new_cursor, expected.len());
            }
            other => panic!("expected Accept, got {other:?}"),
        }
        assert!(!c.open);
    }

    #[test]
    fn esc_dismisses() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(matches!(
            c.handle_key(KeyCode::Esc),
            CompletionKeyOutcome::Dismiss
        ));
        assert!(!c.open);
    }

    #[test]
    fn read_cwd_entries_excludes_dotfiles_and_is_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        let entries = read_cwd_entries(dir.path());
        assert_eq!(entries, vec!["a.txt".to_string(), "b.txt".to_string()]);
    }
}
