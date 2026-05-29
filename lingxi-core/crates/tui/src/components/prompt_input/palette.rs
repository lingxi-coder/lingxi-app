//! `/` slash-command palette: a live-filtered dropdown over the 99 builtin
//! command names. Opens when the prompt buffer starts with `/`. Pure logic
//! (`PaletteState` + open/filter/select/accept) is unit-tested without iocraft;
//! `PaletteOverlay` (Task 5) renders it.
//!
//! Literal lock (design §2.8): rows show `name` + ` – ` (en-dash, U+2013) +
//! description, mirroring claude-code PromptInputFooterSuggestions.tsx.

use iocraft::prelude::*;
use lingxi_commands::builtin::{core_description, BUILTIN_COMMAND_NAMES};

use super::fuzzy::filtered_ranked;
use crate::theme::TuiTheme;

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
        let names: Vec<String> = BUILTIN_COMMAND_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect();
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

/// What the palette key handler decided. The dispatcher in `root.rs` acts on
/// this — `Accept` replaces the prompt buffer, `Dismiss`/`PassThrough` let the
/// key (or future keys) reach the default input path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteKeyOutcome {
    /// Selection/navigation handled; the key is swallowed.
    Consumed,
    /// Commit the selected command. Carries the full text to set as the new
    /// prompt buffer, e.g. `"/compact "` (leading `/`, trailing space).
    Accept(String),
    /// `Esc` — close the overlay; the key is swallowed (does not also type).
    Dismiss,
    /// The overlay has nothing actionable for this key — let it fall through
    /// to the default input editor (so the char still types, etc.).
    PassThrough,
}

impl PaletteState {
    /// Handle one key while the overlay is open. Only navigation/commit keys
    /// are consumed; printable chars return `PassThrough` so the default input
    /// path inserts them (which then re-runs `sync_from_prompt`).
    pub fn handle_key(&mut self, code: KeyCode) -> PaletteKeyOutcome {
        let rows = self.rows();
        match code {
            KeyCode::Down => {
                if !rows.is_empty() && self.selected + 1 < rows.len() {
                    self.selected += 1;
                }
                PaletteKeyOutcome::Consumed
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                PaletteKeyOutcome::Consumed
            }
            KeyCode::Tab | KeyCode::Enter => {
                if let Some(row) = rows.get(self.selected) {
                    let text = format!("/{} ", row.name);
                    self.open = false;
                    self.filter.clear();
                    self.selected = 0;
                    // M7-16: candidate tengu_tui_command_palette_opened
                    PaletteKeyOutcome::Accept(text)
                } else {
                    PaletteKeyOutcome::PassThrough
                }
            }
            KeyCode::Esc => {
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                PaletteKeyOutcome::Dismiss
            }
            _ => PaletteKeyOutcome::PassThrough,
        }
    }
}

/// Free-function shim so tests can call `handle_key(&mut state, code)`.
#[cfg(test)]
fn handle_key(state: &mut PaletteState, code: KeyCode) -> PaletteKeyOutcome {
    state.handle_key(code)
}

/// Props for the palette dropdown overlay.
#[derive(Default, Props)]
pub struct PaletteOverlayProps {
    /// The filtered rows to display (caller truncates to `OVERLAY_MAX_ITEMS`).
    pub rows: Vec<PaletteRow>,
    /// Index of the highlighted row within `rows`.
    pub selected: usize,
}

/// Render the palette dropdown: up to `OVERLAY_MAX_ITEMS` rows, each
/// `name – description`. The selected row is highlighted; the rest are dim.
/// Literal lock: `" – "` is U+2013 with surrounding spaces (claude-code
/// PromptInputFooterSuggestions row format).
#[component]
pub fn PaletteOverlay(props: &PaletteOverlayProps) -> impl Into<AnyElement<'static>> {
    let rows: Vec<_> = props.rows.iter().take(OVERLAY_MAX_ITEMS).cloned().collect();
    let selected = props.selected;
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows.into_iter().enumerate().map(|(i, row)| {
                let line = format!("/{} \u{2013} {}", row.name, row.description);
                // TODO(M7-15): theme picker adds a dedicated "suggestion" token;
                // until then the selected row reuses ASSISTANT and others DIM.
                let color = if i == selected { TuiTheme::ASSISTANT } else { TuiTheme::DIM };
                element! {
                    View(height: 1) {
                        Text(content: line, color: color)
                    }
                }
            }))
        }
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
        let row = p
            .rows()
            .into_iter()
            .find(|r| r.name == "help")
            .expect("help present");
        assert_eq!(row.description, "Show help and available commands");
    }

    #[test]
    fn down_up_move_selection_and_clamp() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/co"); // several rows
        assert_eq!(p.selected, 0);
        assert!(matches!(
            handle_key(&mut p, KeyCode::Down),
            PaletteKeyOutcome::Consumed
        ));
        assert_eq!(p.selected, 1);
        assert!(matches!(
            handle_key(&mut p, KeyCode::Up),
            PaletteKeyOutcome::Consumed
        ));
        assert_eq!(p.selected, 0);
        // Up at the top stays at 0 (no wrap).
        handle_key(&mut p, KeyCode::Up);
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn tab_accepts_selected_into_a_complete_command() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/comp");
        let sel = p.rows()[p.selected].name; // best match, e.g. "compact"
        let outcome = handle_key(&mut p, KeyCode::Tab);
        match outcome {
            PaletteKeyOutcome::Accept(text) => assert_eq!(text, format!("/{sel} ")),
            other => panic!("expected Accept, got {other:?}"),
        }
        assert!(!p.open, "accepting closes the overlay");
    }

    #[test]
    fn enter_also_accepts() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/help");
        assert!(matches!(
            handle_key(&mut p, KeyCode::Enter),
            PaletteKeyOutcome::Accept(_)
        ));
    }

    #[test]
    fn esc_dismisses_and_releases_keys() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/co");
        assert!(matches!(
            handle_key(&mut p, KeyCode::Esc),
            PaletteKeyOutcome::Dismiss
        ));
        assert!(!p.open);
    }

    #[test]
    fn accept_with_no_rows_is_passthrough() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/zzzznomatch");
        assert!(p.rows().is_empty());
        // Nothing to accept → Enter/Tab fall through to default input.
        assert!(matches!(
            handle_key(&mut p, KeyCode::Enter),
            PaletteKeyOutcome::PassThrough
        ));
    }
}
