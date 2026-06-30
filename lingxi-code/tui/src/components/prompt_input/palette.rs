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
//! (cp-08) Rows mirror claude-code `PromptInputFooterSuggestions`'s
//! non-unified ("command") layout: a fixed-width padded name column (sized
//! from the widest name in the filtered list, clamped to 40% of terminal
//! width) followed by a separately width-truncated description — see
//! `format_palette_row`.

use command_api::builtin_support::names::{
    command_aliases, core_description, is_command_env_disabled, is_palette_hidden,
    BUILTIN_COMMAND_NAMES,
};
use iocraft::prelude::*;

use super::fuzzy::filtered_ranked;
use crate::render::truncate_to_width_ellipsis;
use crate::theme::Theme;

/// Max dropdown rows shown at once (claude-code `OVERLAY_MAX_ITEMS`).
pub const OVERLAY_MAX_ITEMS: usize = 5;

/// One filtered palette row: command name + description (both `'static`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow {
    /// Command name without the leading `/`. Always the canonical command —
    /// the accept text commits `/<name> ` even when matched via an alias.
    pub name: &'static str,
    /// Description text (real for the 18 core commands, stub otherwise).
    pub description: &'static str,
    /// (cp-03) The typed alias this row matched through, if any — rendered as
    /// ` (<alias>)` after the name (`findMatchedAlias` / `createCommandSuggestionItem`).
    pub matched_alias: Option<&'static str>,
}

/// LingXi-only palette commands that are NOT in claude-code's byte-locked
/// [`BUILTIN_COMMAND_NAMES`] (the parity list stays pure — we never mutate the
/// `94` count). `/connect` is a LingXi addition (opencode-style provider
/// sign-in / API-key entry) and is surfaced in the `/` palette like any builtin
/// so it is discoverable. `(name, description)`.
const LINGXI_EXTRA_COMMANDS: &[(&str, &str)] = &[(
    "connect",
    "Connect a provider \u{2014} sign in or add an API key",
)];

/// Description for a palette row: LingXi extras first, else the byte-locked
/// `core_description`.
fn description_for(name: &str) -> &'static str {
    LINGXI_EXTRA_COMMANDS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or_else(|| core_description(name), |(_, d)| *d)
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
        let visible = |n: &str| !is_palette_hidden(n) && !is_command_env_disabled(n);
        let names: Vec<&'static str> = BUILTIN_COMMAND_NAMES
            .iter()
            .copied()
            // LingXi extras (e.g. `/connect`) appended after the parity builtins
            // so they rank alphabetically alongside them but never alter the
            // byte-locked `BUILTIN_COMMAND_NAMES` list.
            .chain(LINGXI_EXTRA_COMMANDS.iter().map(|(n, _)| *n))
            .filter(|n| visible(n))
            .collect();

        // Bare `/` (empty filter): all commands by name (cp-02 alphabetical),
        // no aliases — aliases only surface once the user types a filter.
        if self.filter.is_empty() {
            let cands: Vec<String> = names.iter().map(|s| (*s).to_string()).collect();
            return filtered_ranked(&self.filter, &cands)
                .into_iter()
                .filter_map(|m| names.iter().copied().find(|n| *n == m))
                .map(|name| PaletteRow {
                    name,
                    description: description_for(name),
                    matched_alias: None,
                })
                .collect();
        }

        // (cp-03) Non-empty filter: rank NAME matches first (claude-code's Fuse
        // `commandName` weight 3), then ALIAS matches (`aliasKey` weight 2) for
        // commands not already shown — so `/co` lists the co* commands before
        // surfacing `/usage` via its `cost` alias. `matched_alias` (the displayed
        // ` (<alias>)`) is `findMatchedAlias`, computed for EVERY row regardless
        // of which pass matched it, exactly as claude-code does.
        let row = |name: &'static str| PaletteRow {
            name,
            description: description_for(name),
            matched_alias: find_matched_alias(&self.filter, name),
        };
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<PaletteRow> = Vec::new();

        // Pass 1 — name matches.
        let name_cands: Vec<String> = names.iter().map(|s| (*s).to_string()).collect();
        for token in filtered_ranked(&self.filter, &name_cands) {
            if let Some(name) = names.iter().copied().find(|n| *n == token) {
                if seen.insert(name) {
                    out.push(row(name));
                }
            }
        }

        // Pass 2 — alias matches (canonical command not already shown).
        let mut alias_cands: Vec<String> = Vec::new();
        let mut alias_owner: std::collections::HashMap<String, &'static str> =
            std::collections::HashMap::new();
        for &name in &names {
            for &alias in command_aliases(name) {
                alias_cands.push(alias.to_string());
                alias_owner.entry(alias.to_string()).or_insert(name);
            }
        }
        for token in filtered_ranked(&self.filter, &alias_cands) {
            if let Some(&name) = alias_owner.get(token) {
                if seen.insert(name) {
                    out.push(row(name));
                }
            }
        }
        out
    }
}

/// claude-code `findMatchedAlias`: the first alias of `name` that the
/// (lowercased, already-trimmed) `query` is a prefix of, or `None`.
fn find_matched_alias(query: &str, name: &'static str) -> Option<&'static str> {
    if query.is_empty() {
        return None;
    }
    let q = query.to_lowercase();
    command_aliases(name)
        .iter()
        .copied()
        .find(|alias| alias.to_lowercase().starts_with(&q))
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
                // (cp-04) wrap: last → first.
                if !rows.is_empty() {
                    self.selected = (self.selected + 1) % rows.len();
                }
                PaletteKeyOutcome::Consumed
            }
            KeyCode::Up => {
                // (cp-04) wrap: first → last.
                if !rows.is_empty() {
                    self.selected = (self.selected + rows.len() - 1) % rows.len();
                }
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
    /// The FULL filtered rows (not just the visible window) — needed to
    /// compute the shared name-column width (cp-08) before slicing to
    /// `OVERLAY_MAX_ITEMS` for rendering.
    pub rows: Vec<PaletteRow>,
    /// Index of the highlighted row within `rows`.
    pub selected: usize,
    /// (M7-15) Active palette — the selected-row `suggestion` accent + dim
    /// rest are centralized here.
    pub theme: Theme,
    /// (cp-08) Live terminal column width, driving the name-column width
    /// (`floor(width * 0.4)`) and description truncation budget.
    pub width: usize,
}

/// The command name as it's displayed (with a leading `/` and, when matched
/// via a typed alias, ` (<alias>)` — cp-03).
fn display_text_for(row: &PaletteRow) -> String {
    let alias = row
        .matched_alias
        .map(|a| format!(" ({a})"))
        .unwrap_or_default();
    format!("/{}{}", row.name, alias)
}

/// (cp-08) claude-code `SuggestionItemRow`'s non-unified ("command") row
/// layout: a fixed-width padded name column (shared across every row, sized
/// from the WIDEST name in the full filtered list, clamped to 40% of the
/// terminal width) plus a separately width-truncated, whitespace-collapsed
/// description — no `–` separator (the column padding alone provides the
/// gap). Mirrors `PromptInputFooterSuggestions`'s `maxColumnWidth` (computed
/// once over every row, not just the visible window) and per-row
/// `descriptionWidth` math.
fn format_palette_row(row: &PaletteRow, all_rows: &[PaletteRow], columns: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let columns = columns.max(1);
    let max_name_width = (columns * 2) / 5; // floor(columns * 0.4), exact in integer math
    let widest = all_rows
        .iter()
        .map(|r| UnicodeWidthStr::width(display_text_for(r).as_str()))
        .max()
        .unwrap_or(0);
    let max_column_width = widest + 5;
    let display_text_width = max_column_width.min(max_name_width);
    let mut name = display_text_for(row);
    if UnicodeWidthStr::width(name.as_str()) > display_text_width.saturating_sub(2) {
        name = truncate_to_width_ellipsis(&name, display_text_width.saturating_sub(2));
    }
    let pad = display_text_width.saturating_sub(UnicodeWidthStr::width(name.as_str()));
    let padded_name = format!("{name}{}", " ".repeat(pad));
    let description_width = columns.saturating_sub(display_text_width).saturating_sub(4);
    let collapsed_desc = row
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let desc = truncate_to_width_ellipsis(&collapsed_desc, description_width);
    format!("{padded_name}{desc}")
}

/// Render the palette dropdown: up to `OVERLAY_MAX_ITEMS` rows, name-column
/// padded + description-truncated per `format_palette_row` (cp-08). The
/// selected row is highlighted; the rest are dim.
#[component]
pub fn PaletteOverlay(props: &PaletteOverlayProps) -> impl Into<AnyElement<'static>> {
    let all_rows = &props.rows;
    let visible: Vec<_> = props.rows.iter().take(OVERLAY_MAX_ITEMS).cloned().collect();
    let selected = props.selected;
    let theme = props.theme;
    let columns = props.width;
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(visible.into_iter().enumerate().map(|(i, row)| {
                let line = format_palette_row(&row, all_rows, columns);
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
        // 94 builtins minus the 26 hidden/disabled commands = 68 visible, plus
        // the 1 LingXi extra (`/connect`) = 69.
        assert_eq!(
            all, 69,
            "bare slash lists every VISIBLE command + LingXi /connect"
        );
        assert!(
            p.rows().iter().any(|r| r.name == "connect"),
            "/connect must appear in the palette"
        );
        p.sync_from_prompt("/comp");
        let narrowed = p.rows();
        assert!(narrowed.len() < all);
        assert!(narrowed.iter().any(|r| r.name == "compact"));
    }

    #[test]
    fn alias_matching_surfaces_canonical_with_alias_label() {
        // (cp-03) Typing an alias surfaces its CANONICAL command, tagged with
        // the matched alias; the accept text still commits the canonical name.
        let mut p = PaletteState::default();
        p.sync_from_prompt("/cost");
        let usage = p
            .rows()
            .into_iter()
            .find(|r| r.name == "usage")
            .expect("/cost should surface /usage");
        assert_eq!(usage.matched_alias, Some("cost"));
        assert_eq!(usage.name, "usage"); // accept commits `/usage `, not `/cost `

        p.sync_from_prompt("/settings");
        let config = p
            .rows()
            .into_iter()
            .find(|r| r.name == "config")
            .expect("/settings should surface /config");
        assert_eq!(config.matched_alias, Some("settings"));

        // A name match (no alias prefix) carries no alias label.
        p.sync_from_prompt("/config");
        let config = p.rows().into_iter().find(|r| r.name == "config").unwrap();
        assert_eq!(config.matched_alias, None);

        // Bare `/` never folds aliases — all rows are plain.
        p.sync_from_prompt("/");
        assert!(p.rows().iter().all(|r| r.matched_alias.is_none()));
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
        for name in [
            "btw",
            "x402",
            "reload-plugins",
            "install-slack-app",
            "mobile",
            "desktop",
        ] {
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
        // (cp-04) Up at the top wraps to the last row.
        handle_key(&mut p, KeyCode::Up);
        assert_eq!(p.selected, p.rows().len() - 1);
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
