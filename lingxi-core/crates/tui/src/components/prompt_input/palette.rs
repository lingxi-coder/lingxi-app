//! `/` slash-command palette: a live-filtered dropdown over the 99 builtin
//! command names. Opens when the prompt buffer starts with `/`. Pure logic
//! (`PaletteState` + open/filter/select/accept) is unit-tested without iocraft;
//! `PaletteOverlay` (Task 5) renders it.
//!
//! Literal lock (design §2.8): rows show `name` + ` – ` (en-dash, U+2013) +
//! description, mirroring claude-code PromptInputFooterSuggestions.tsx.

use lingxi_commands::builtin::{core_description, BUILTIN_COMMAND_NAMES};

use super::fuzzy::filtered_ranked;

/// Max dropdown rows shown at once (claude-code `OVERLAY_MAX_ITEMS`).
pub const OVERLAY_MAX_ITEMS: usize = 5;

/// One filtered palette row: command name + description (both `'static`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow {
    /// Command name without the leading `/`.
    pub name: &'static str,
    /// Description text (real for the 18 core commands, stub otherwise).
    pub description: &'static str,
}

/// Palette overlay state. `open == false` means the overlay is dismissed and
/// owns no keys.
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    /// Whether the dropdown is currently shown.
    pub open: bool,
    /// The filter text (the prompt slice after the leading `/`).
    pub filter: String,
    /// Index into the *filtered* rows, clamped to `[0, len)`.
    pub selected: usize,
}

impl PaletteState {
    /// Recompute open-state + filter from the current prompt buffer.
    /// Opens iff `prompt` starts with `/` and contains no space (a space
    /// means the user has moved past the command name into arguments).
    pub fn sync_from_prompt(&mut self, prompt: &str) {
        let is_command_token = prompt.starts_with('/') && !prompt[1..].contains(' ');
        if is_command_token {
            let new_filter = prompt[1..].to_string();
            if !self.open || new_filter != self.filter {
                self.selected = 0;
            }
            self.open = true;
            self.filter = new_filter;
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
        }
    }

    /// The filtered, ranked rows for the current filter.
    #[must_use]
    pub fn rows(&self) -> Vec<PaletteRow> {
        let names: Vec<String> = BUILTIN_COMMAND_NAMES.iter().map(|s| (*s).to_string()).collect();
        filtered_ranked(&self.filter, &names)
            .into_iter()
            .filter_map(|matched| {
                BUILTIN_COMMAND_NAMES
                    .iter()
                    .copied()
                    .find(|n| *n == matched)
                    .map(|name| PaletteRow {
                        name,
                        description: core_description(name),
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_when_prompt_starts_with_slash() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/co");
        assert!(p.open);
        assert_eq!(p.filter, "co");
    }

    #[test]
    fn stays_closed_without_slash() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("hello");
        assert!(!p.open);
    }

    #[test]
    fn closes_once_a_space_follows_the_command() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/compact");
        assert!(p.open);
        p.sync_from_prompt("/compact now");
        assert!(!p.open, "a space moves past the command name → close");
    }

    #[test]
    fn filter_narrows_rows() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/");
        let all = p.rows().len();
        assert_eq!(all, 99, "bare slash lists every command");
        p.sync_from_prompt("/comp");
        let narrowed = p.rows();
        assert!(narrowed.len() < all);
        assert!(narrowed.iter().any(|r| r.name == "compact"));
    }

    #[test]
    fn core_rows_carry_real_descriptions() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/help");
        let row = p.rows().into_iter().find(|r| r.name == "help").expect("help present");
        assert_eq!(row.description, "Show help and available commands");
    }
}
