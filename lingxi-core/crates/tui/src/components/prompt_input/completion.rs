//! `@` file-ref completion: a dropdown of cwd path entries. Opens when an `@`
//! token is being typed. The filter/select logic takes the candidate `Vec`
//! as input so it's testable without touching the filesystem; `read_cwd_entries`
//! is the only fs-touching fn and is exercised by a separate fs test.
//!
//! Literal lock (design §2.8): inserts `@<path> ` (trailing space), mirroring
//! claude-code QuickOpenDialog handleInsert. Empty-state strings copied below.

use iocraft::prelude::KeyCode;

use super::fuzzy::filtered_ranked;

/// Max dropdown rows (shared with the palette; claude-code `OVERLAY_MAX_ITEMS`).
pub const OVERLAY_MAX_ITEMS: usize = 5;

/// claude-code QuickOpenDialog empty-state literal when a query is present.
pub const EMPTY_WITH_QUERY: &str = "No matching files";
/// claude-code QuickOpenDialog empty-state literal for an empty query.
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
        match active_at_token(prompt, cursor) {
            Some((_, partial)) => {
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
            }
            None => {
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                self.candidates.clear();
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cands() -> Vec<String> {
        vec!["src/main.rs".into(), "src/lib.rs".into(), "README.md".into()]
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
}
