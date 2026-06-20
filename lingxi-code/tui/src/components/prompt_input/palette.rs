//! `/` slash-command palette: a live-filtered dropdown over the visible builtin
//! command names. Opens when the prompt buffer starts with `/`. Pure logic
//! (`PaletteState` + open/filter/select/accept) is unit-tested without iocraft;
//! `PaletteOverlay` (Task 5) renders it.
//!
//! The 26 hidden/disabled commands (`is_palette_hidden`) are filtered out, plus
//! any command whose `DISABLE_*_COMMAND` env gate (`is_command_env_disabled`)
//! is tripped, so the palette lists only the visible commands (68 with no env
//! gate set), matching claude-code's
//! `commands.filter(c => !c.isHidden && !$te(c))` palette filter.
//!
//! Literal lock (design §2.8): rows show `name` + ` – ` (en-dash, U+2013) +
//! description, mirroring claude-code PromptInputFooterSuggestions.tsx.

use command_api::builtin_support::names::{
    core_description, is_command_env_disabled, is_palette_hidden, BUILTIN_COMMAND_NAMES,
};
use iocraft::prelude::*;

use super::fuzzy::filtered_ranked;
use crate::theme::Theme;

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
    ///
    /// Hidden / disabled commands (per `is_palette_hidden`) are excluded up
    /// front so they never appear in the dropdown, exactly as claude-code drops
    /// any command where `isHidden || isEnabled()===off` from the palette.
    #[must_use]
    pub fn rows(&self) -> Vec<PaletteRow> {
        let names: Vec<String> = BUILTIN_COMMAND_NAMES
            .iter()
            .filter(|n| !is_palette_hidden(n) && !is_command_env_disabled(n))
            .map(|s| (*s).to_string())
            .collect();
        filtered_ranked(&self.filter, &names)
            .into_iter()
            .filter_map(|matched| {
                BUILTIN_COMMAND_NAMES
                    .iter()
                    .copied()
                    .find(|n| *n == matched && !is_palette_hidden(n) && !is_command_env_disabled(n))
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
    /// (M7-15) Active palette — the selected-row `suggestion` accent + dim
    /// rest are centralized here.
    pub theme: Theme,
}

/// Render the palette dropdown: up to `OVERLAY_MAX_ITEMS` rows, each
/// `name – description`. The selected row is highlighted; the rest are dim.
/// Literal lock: `" – "` is U+2013 with surrounding spaces (claude-code
/// PromptInputFooterSuggestions row format).
#[component]
pub fn PaletteOverlay(props: &PaletteOverlayProps) -> impl Into<AnyElement<'static>> {
    let rows: Vec<_> = props.rows.iter().take(OVERLAY_MAX_ITEMS).cloned().collect();
    let selected = props.selected;
    let theme = props.theme;
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows.into_iter().enumerate().map(|(i, row)| {
                let line = format!("/{} \u{2013} {}", row.name, row.description);
                // (M7-15) Centralized: selected row uses the theme's `suggestion`
                // accent (claude-code's completion highlight), the rest dim.
                let color = if i == selected { theme.suggestion } else { theme.dim };
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
        // 94 builtins minus the 26 hidden/disabled commands = 68 visible.
        assert_eq!(all, 68, "bare slash lists every VISIBLE command");
        p.sync_from_prompt("/comp");
        let narrowed = p.rows();
        assert!(narrowed.len() < all);
        assert!(narrowed.iter().any(|r| r.name == "compact"));
    }

    #[test]
    fn hidden_and_disabled_commands_never_appear() {
        use command_api::builtin_support::names::{
            CORRECT_BY_DESIGN_STUBS, HIDDEN_PALETTE_COMMANDS,
        };
        let mut p = PaletteState::default();
        p.sync_from_prompt("/");
        let rows = p.rows();
        for name in HIDDEN_PALETTE_COMMANDS
            .iter()
            .copied()
            .chain(CORRECT_BY_DESIGN_STUBS.iter().map(|(n, _)| *n))
        {
            assert!(
                !rows.iter().any(|r| r.name == name),
                "/{name} is hidden/disabled and must not appear in the palette"
            );
        }
        // And not even when typed as an exact prefix.
        p.sync_from_prompt("/heapdump");
        assert!(
            p.rows().iter().all(|r| r.name != "heapdump"),
            "/heapdump is isHidden:!0 and must never surface"
        );
    }

    #[test]
    fn visible_host_bound_commands_still_appear() {
        // claude-code SHOWS these (no isHidden/isEnabled gate) — keep them.
        let mut p = PaletteState::default();
        p.sync_from_prompt("/");
        let rows = p.rows();
        for name in ["btw", "x402", "reload-plugins", "install-slack-app", "mobile", "desktop"] {
            assert!(
                rows.iter().any(|r| r.name == name),
                "/{name} is visible in claude-code and must appear in the palette"
            );
        }
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
