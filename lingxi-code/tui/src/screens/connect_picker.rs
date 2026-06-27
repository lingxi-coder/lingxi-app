//! `/connect` provider PICKER (opencode-style): a grouped, searchable list of
//! the connectable LLM providers. Selecting a provider routes into the EXISTING
//! key-entry `/connect` screen (`connect.rs`) via `AppState.pending_connect`.
//!
//! Modeled EXACTLY on the `/model` picker (`model.rs`): a pure highlight-only
//! reducer + a pure `-> String` render oracle (the app.rs arm colors it
//! line-by-line). Up/Down move over the SELECTABLE rows, printable chars edit
//! the search query, Backspace deletes, Enter yields
//! [`ConnectPickerOutcome::Select`] (the caller raises `pending_connect` →
//! `root::pump_open_connect` opens the key-entry screen), Esc cancels.
//!
//! GROUPING: rows split into a `Popular` group (`ConnectRow.popular`) and a
//! `Providers` group (everything else), first-seen order. A row whose provider
//! has a usable credential (joined from `AppState.provider_availability` at
//! build time) is marked connected (a leading `✓`). An empty availability map
//! (the default, before the engine populates it) simply yields no `✓`s.

use std::collections::BTreeMap;

/// One selectable provider row in the grouped picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRow {
    /// Value handed to `pending_connect` (the SAME id `/connect <provider>`
    /// accepts — the profile_name).
    pub provider_id: String,
    /// Human label shown in the row (e.g. `"Anthropic"`).
    pub label: String,
    /// One-line description, rendered after the label column.
    pub description: String,
    /// Whether this row belongs to the `Popular` group (else `Providers`).
    pub popular: bool,
    /// Whether this provider already has a usable credential (leading `✓`).
    pub connected: bool,
}

/// A rendered line: a non-selectable group header, or a selectable row
/// (carrying its index into `ConnectPickerState::rows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibleLine {
    /// Group header (e.g. "Popular", "Providers").
    Header(String),
    /// Selectable row: index into `ConnectPickerState::rows`.
    Item(usize),
}

/// (connect-picker) claude-code-style dim sub-header line, plain text.
pub const SUB_HEADER: &str =
    "Add an API key or sign in to a provider. Applies to this session and future LingXi sessions.";

/// Column width the provider label is left-justified into before the
/// description (so the descriptions align into a second column).
const NAME_COL: usize = 22;

/// Whether ANY of `keys` resolves to a usable credential in `availability`.
/// A missing key ⇒ not connected (best-effort; an empty map yields no `✓`s).
fn any_connected(availability: &BTreeMap<String, bool>, keys: &[&str]) -> bool {
    keys.iter().any(|k| availability.get(*k).copied().unwrap_or(false))
}

/// Build the static LingXi connectable-provider catalog, joining each row's
/// `connected` flag from `availability` (best-effort key lookup; anthropic also
/// tries the `anthropic-api-key`/`anthropic-oauth` keychain ids). An empty map
/// ⇒ every row unconnected (no `✓`).
#[must_use]
pub fn default_connect_rows(availability: &BTreeMap<String, bool>) -> Vec<ConnectRow> {
    let mk = |provider_id: &str, label: &str, description: &str, popular: bool, keys: &[&str]| {
        ConnectRow {
            provider_id: provider_id.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            popular,
            connected: any_connected(availability, keys),
        }
    };
    vec![
        // POPULAR
        mk(
            "anthropic",
            "Anthropic",
            "Claude models \u{2014} API key or Pro/Max sign-in",
            true,
            &["anthropic", "anthropic-api-key", "anthropic-oauth"],
        ),
        mk("openai", "OpenAI", "GPT models \u{2014} API key", true, &["openai"]),
        mk(
            "openai-chatgpt",
            "OpenAI (ChatGPT)",
            "Sign in with ChatGPT Plus/Pro",
            true,
            &["openai-chatgpt"],
        ),
        mk(
            "github-copilot",
            "GitHub Copilot",
            "Use your GitHub Copilot subscription",
            true,
            &["github-copilot"],
        ),
        mk("gemini", "Google Gemini", "Gemini models \u{2014} API key", true, &["gemini"]),
        mk(
            "deepseek",
            "DeepSeek",
            "DeepSeek Chat / Reasoner \u{2014} API key",
            true,
            &["deepseek"],
        ),
        // PROVIDERS
        mk(
            "openrouter",
            "OpenRouter",
            "Unified gateway to many models \u{2014} API key",
            false,
            &["openrouter"],
        ),
        mk("zai", "Z.AI", "GLM models \u{2014} API key", false, &["zai"]),
        mk(
            "glm-coding",
            "GLM Coding Plan",
            "Zhipu coding-plan subscription \u{2014} API key",
            false,
            &["glm-coding"],
        ),
    ]
}

/// Grouped `/connect` picker state. Pure; mirrors `ModelScreenState`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectPickerState {
    /// All selectable rows (from [`default_connect_rows`]).
    pub rows: Vec<ConnectRow>,
    /// Search query (printable chars typed in the picker).
    pub query: String,
    /// Highlighted index into the flat list of selectable items (not headers).
    pub selected: usize,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectPickerOutcome {
    /// Stay open (highlight moved / query edited / inert key).
    Stay,
    /// Enter — route into the key-entry `/connect` screen for `provider_id`.
    Select {
        /// Provider id of the chosen row (== `ConnectRow.provider_id`).
        provider_id: String,
    },
    /// Esc — cancel with no change.
    Cancel,
}

impl ConnectPickerState {
    /// Build from a pre-built row list.
    #[must_use]
    pub fn new(rows: Vec<ConnectRow>) -> Self {
        Self { rows, query: String::new(), selected: 0 }
    }

    /// Build the default catalog joined against the App's `provider_availability`.
    #[must_use]
    pub fn from_availability(availability: &BTreeMap<String, bool>) -> Self {
        Self::new(default_connect_rows(availability))
    }

    /// Whether a row matches the query (case-insensitive substring over label,
    /// description, and provider id). Empty query matches all.
    fn matches(&self, row: &ConnectRow) -> bool {
        if self.query.is_empty() {
            return true;
        }
        let q = self.query.to_lowercase();
        row.label.to_lowercase().contains(&q)
            || row.description.to_lowercase().contains(&q)
            || row.provider_id.to_lowercase().contains(&q)
    }

    /// Ordered visible lines: a `Popular` group then a `Providers` group (each
    /// in row order). Only query-matching rows appear; empty groups are omitted.
    #[must_use]
    pub fn visible_lines(&self) -> Vec<VisibleLine> {
        let mut out = Vec::new();
        for (header, popular) in [("Popular", true), ("Providers", false)] {
            let items: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.popular == popular && self.matches(r))
                .map(|(i, _)| i)
                .collect();
            if !items.is_empty() {
                out.push(VisibleLine::Header(header.to_string()));
                out.extend(items.into_iter().map(VisibleLine::Item));
            }
        }
        out
    }

    /// `rows` indices of the selectable items, in visible order.
    fn selectable(&self) -> Vec<usize> {
        self.visible_lines()
            .into_iter()
            .filter_map(|l| match l {
                VisibleLine::Item(i) => Some(i),
                VisibleLine::Header(_) => None,
            })
            .collect()
    }
}

/// Reduce a key. `Up`/`Down` move over selectable items; printable chars edit
/// the search query; Backspace deletes; Enter selects the highlighted row; Esc
/// cancels. Mirrors `model::handle_model_key`.
#[must_use]
pub fn handle_connect_picker_key(
    state: &mut ConnectPickerState,
    key: crossterm::event::KeyCode,
) -> ConnectPickerOutcome {
    use crossterm::event::KeyCode;
    let count = state.selectable().len();
    match key {
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            ConnectPickerOutcome::Stay
        }
        KeyCode::Down => {
            if count > 0 {
                state.selected = (state.selected + 1).min(count - 1);
            }
            ConnectPickerOutcome::Stay
        }
        KeyCode::Char(c) => {
            state.query.push(c);
            state.selected = 0;
            ConnectPickerOutcome::Stay
        }
        KeyCode::Backspace => {
            state.query.pop();
            state.selected = 0;
            ConnectPickerOutcome::Stay
        }
        KeyCode::Enter => match state.selectable().get(state.selected) {
            Some(&idx) => ConnectPickerOutcome::Select {
                provider_id: state.rows[idx].provider_id.clone(),
            },
            None => ConnectPickerOutcome::Stay,
        },
        KeyCode::Esc => ConnectPickerOutcome::Cancel,
        _ => ConnectPickerOutcome::Stay,
    }
}

/// Render the grouped picker body (plain text; the iocraft layer wraps it and
/// colors it line-by-line — line 0 title bold/accent, line 1 dim, rest default,
/// mirroring the Model arm). A connected row gets a leading `✓`; the highlighted
/// row a leading `❯` (the highlight wins over the `✓` marker on the same row).
#[must_use]
pub fn render_connect_picker_to_string(state: &ConnectPickerState) -> String {
    let mut out = String::from("Connect a provider\n");
    out.push_str(SUB_HEADER);
    out.push('\n');
    out.push_str(&format!("Search: {}\n", state.query));
    let lines = state.visible_lines();
    if lines.is_empty() {
        out.push_str("No providers available.");
        return out;
    }
    let mut item_pos = 0usize;
    for line in &lines {
        match line {
            VisibleLine::Header(label) => {
                out.push('\n');
                out.push_str(label);
                out.push('\n');
            }
            VisibleLine::Item(idx) => {
                let row = &state.rows[*idx];
                let marker = if item_pos == state.selected {
                    "\u{276F} "
                } else if row.connected {
                    "\u{2713} "
                } else {
                    "  "
                };
                out.push_str(marker);
                out.push_str(&format!("{:<NAME_COL$}", row.label));
                out.push_str(&row.description);
                out.push('\n');
                item_pos += 1;
            }
        }
    }
    out.push_str("type to search \u{00B7} Enter to select \u{00B7} Esc to cancel");
    out
}

#[cfg(test)]
mod reducer_tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn rows() -> Vec<ConnectRow> {
        default_connect_rows(&BTreeMap::new())
    }

    #[test]
    fn nav_moves_highlight_over_selectables() {
        let mut st = ConnectPickerState::new(rows());
        assert_eq!(st.selected, 0);
        assert_eq!(handle_connect_picker_key(&mut st, KeyCode::Down), ConnectPickerOutcome::Stay);
        assert_eq!(st.selected, 1);
        let _ = handle_connect_picker_key(&mut st, KeyCode::Up);
        assert_eq!(st.selected, 0);
        // Up clamps at 0.
        let _ = handle_connect_picker_key(&mut st, KeyCode::Up);
        assert_eq!(st.selected, 0);
    }

    #[test]
    fn char_filters_query_and_resets_selection() {
        let mut st = ConnectPickerState::new(rows());
        let _ = handle_connect_picker_key(&mut st, KeyCode::Down);
        for c in "openrouter".chars() {
            assert_eq!(
                handle_connect_picker_key(&mut st, KeyCode::Char(c)),
                ConnectPickerOutcome::Stay
            );
        }
        assert_eq!(st.selected, 0);
        let sel = st.selectable();
        assert_eq!(sel.len(), 1);
        assert_eq!(st.rows[sel[0]].provider_id, "openrouter");
        let _ = handle_connect_picker_key(&mut st, KeyCode::Backspace);
        assert!(st.selectable().len() >= 1);
    }

    #[test]
    fn enter_selects_provider_id() {
        let mut st = ConnectPickerState::new(rows());
        for c in "openrouter".chars() {
            let _ = handle_connect_picker_key(&mut st, KeyCode::Char(c));
        }
        assert_eq!(
            handle_connect_picker_key(&mut st, KeyCode::Enter),
            ConnectPickerOutcome::Select { provider_id: "openrouter".to_string() }
        );
    }

    #[test]
    fn esc_cancels() {
        let mut st = ConnectPickerState::new(rows());
        assert_eq!(handle_connect_picker_key(&mut st, KeyCode::Esc), ConnectPickerOutcome::Cancel);
    }

    #[test]
    fn empty_is_inert() {
        let mut st = ConnectPickerState::default();
        assert_eq!(handle_connect_picker_key(&mut st, KeyCode::Enter), ConnectPickerOutcome::Stay);
        assert_eq!(handle_connect_picker_key(&mut st, KeyCode::Down), ConnectPickerOutcome::Stay);
        assert_eq!(st.selected, 0);
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn renders_groups_headers_and_footer() {
        let st = ConnectPickerState::from_availability(&BTreeMap::new());
        let out = render_connect_picker_to_string(&st);
        assert!(out.starts_with(&format!("Connect a provider\n{SUB_HEADER}\nSearch: \n")), "{out}");
        assert!(out.contains("\nPopular\n"), "{out}");
        assert!(out.contains("\nProviders\n"), "{out}");
        // First row is highlighted with the ❯ marker.
        assert!(out.contains("\u{276F} Anthropic"), "{out}");
        // A non-selected, unconnected row gets the two-space marker.
        assert!(out.contains("  OpenRouter"), "{out}");
        assert!(out.ends_with("type to search \u{00B7} Enter to select \u{00B7} Esc to cancel"));
    }

    #[test]
    fn connected_provider_gets_check_marker() {
        let mut avail = BTreeMap::new();
        avail.insert("openrouter".to_string(), true);
        let st = ConnectPickerState::from_availability(&avail);
        let out = render_connect_picker_to_string(&st);
        // openrouter is not the highlighted row (Anthropic is), so its ✓ shows.
        assert!(out.contains("\u{2713} OpenRouter"), "{out}");
    }

    #[test]
    fn anthropic_oauth_key_counts_as_connected() {
        let mut avail = BTreeMap::new();
        avail.insert("anthropic-oauth".to_string(), true);
        let rows = default_connect_rows(&avail);
        let anthropic = rows.iter().find(|r| r.provider_id == "anthropic").unwrap();
        assert!(anthropic.connected);
    }

    #[test]
    fn empty_query_shows_all_nine() {
        let st = ConnectPickerState::from_availability(&BTreeMap::new());
        assert_eq!(st.rows.len(), 9);
        let items = st
            .visible_lines()
            .into_iter()
            .filter(|l| matches!(l, VisibleLine::Item(_)))
            .count();
        assert_eq!(items, 9);
    }
}
