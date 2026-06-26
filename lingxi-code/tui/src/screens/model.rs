//! `/model` picker (claude-code `model/` `ModelPicker`): select the main-loop
//! model. A grouped, searchable single-select list (the active model is marked
//! `(current)`; unconfigured providers badge `[Connect]`).
//!
//! Unlike `theme.rs` — which live-previews and commits SYNCHRONOUSLY — switching
//! the model is an ASYNC write (`OrchestratorHandle::switch_model`) the sync key
//! path can't `.await`. So this reducer is pure highlight-only (mirroring
//! `agents.rs`): Enter on an AVAILABLE row yields [`ModelOutcome::Commit`]
//! carrying the chosen `(provider_id, request_model)`; the caller raises
//! `AppState.pending_switch_model` and the async `root::pump_switch_model`
//! performs the write + refreshes the status line. Enter on an UNAVAILABLE row
//! yields [`ModelOutcome::Connect`] (the row's provider has no usable
//! credential); the caller routes to `/connect` (2d) — for now a placeholder that
//! closes the picker. Esc cancels with no change.
//!
//! GROUPING: rows merge `OrchestratorHandle::list_available_models` (routable ids)
//! and `list_model_listings` (the llm-client provider catalog). Each row resolves
//! its provider via the engine-threaded `model_providers`
//! (`request_model -> (profile, label)`) + `provider_availability`
//! (`profile -> bool`) maps; rows group under their provider label, and an
//! unconfigured provider badges `[Connect]` (spec §8). Empty maps (the default,
//! before the engine populates them) keep every row available + grouped under
//! their static label — byte-identical to the historical behavior.

/// One selectable model row in the grouped picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRow {
    /// Human label shown in the row (e.g. `"DeepSeek Chat"` or `"gpt-4o"`).
    pub display_model: String,
    /// Wire id passed to `switch_model` (and the dedup/recents key).
    pub request_model: String,
    /// Provider grouping key (e.g. "deepseek", "anthropic", "builtin").
    pub provider_id: String,
    /// Human provider header (e.g. `"DeepSeek"`, `"Anthropic"`, `"Built-in"`).
    pub provider_label: String,
    /// Whether this row's provider has a usable credential. Joined from the
    /// sibling availability map at build time (spec §8); a provider absent from
    /// that map defaults to `true`. Drives the Connect badge + select-launches-`/connect`.
    pub available: bool,
    /// Optional one-line description, rendered as a dimmed line beneath the row
    /// (claude-code `ListItem`: `paddingLeft={2}` + `color="inactive"`). For
    /// catalog rows it's the listing's `description`; for routable/built-in rows
    /// it falls back to [`model_description`]. `None` ⇒ no sub-line.
    pub description: Option<String>,
}

/// Known one-line description for a built-in model wire id, mirroring the
/// claude-code `/model` picker blurbs. Matched by a case-insensitive family
/// substring so dated ids (`claude-opus-4-7`) and aliases (`opus`) both resolve.
/// `None` for unrecognized ids (the row then renders with no sub-line).
#[must_use]
fn model_description(request_model: &str) -> Option<String> {
    let id = request_model.to_ascii_lowercase();
    if id.contains("opus") {
        Some("Most capable for complex work".to_string())
    } else if id.contains("haiku") {
        Some("Fastest for quick answers".to_string())
    } else if id.contains("sonnet") {
        Some("Best for everyday tasks".to_string())
    } else {
        None
    }
}

/// Human header for an EXISTING (`list_available_models`) provider prefix.
fn existing_provider_label(prefix: &str) -> String {
    match prefix {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "gemini" => "Gemini",
        other => other,
    }
    .to_string()
}

/// Merge the routable model ids (`list_available_models`) and the catalog
/// listings (`list_model_listings`) into a uniform, de-duplicated row list.
/// Existing (routable) models come first, then catalog providers; a wire id
/// seen twice keeps its first (routable) occurrence.
///
/// `model_providers` (I1/I2 fix) is the engine-threaded authoritative map
/// `request_model -> (profile_name, provider_label)` assembled from the LIVE
/// multi-provider config (`ClientConfig.providers`). A bare available-model id
/// (no `/`, no catalog listing) is looked up here so USER-defined providers
/// (e.g. a `groq` profile serving `llama-3.3-70b`) resolve to their OWN profile
/// and label, gating on availability — instead of mis-falling into
/// `"builtin"`/`true`, which suppressed the `[Connect]` badge and let an
/// unconfigured provider's row route directly (→ 401). Built-in CATALOG rows are
/// unaffected: they still come through `catalog` (`list_model_listings`).
#[must_use]
pub fn build_model_entries(
    existing: Vec<String>,
    catalog: Vec<traits::orchestrator::ModelListing>,
    availability: &std::collections::BTreeMap<String, bool>,
    model_providers: &std::collections::BTreeMap<String, (String, String)>,
) -> Vec<ModelRow> {
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let avail = |pid: &str| availability.get(pid).copied().unwrap_or(true);

    for id in existing {
        if !seen.insert(id.clone()) {
            continue;
        }
        let description = model_description(&id);
        let row = if let Some(rest) = id.strip_prefix('@') {
            ModelRow {
                display_model: format!("@{rest}"),
                request_model: id.clone(),
                provider_id: "alias".to_string(),
                provider_label: "Aliases".to_string(),
                available: true,
                description,
            }
        } else if let Some((p, m)) = id.split_once('/') {
            ModelRow {
                display_model: m.to_string(),
                request_model: id.clone(),
                provider_id: p.to_string(),
                provider_label: existing_provider_label(p),
                available: avail(p),
                description,
            }
        } else if let Some((profile, label)) = model_providers.get(&id) {
            // Authoritative engine mapping (typically a USER-defined provider):
            // key the row to its real profile + label and gate on availability,
            // so an unconfigured provider badges `[Connect]` instead of routing.
            ModelRow {
                display_model: id.clone(),
                request_model: id.clone(),
                provider_id: profile.clone(),
                provider_label: label.clone(),
                available: avail(profile),
                description,
            }
        } else {
            // Genuinely unknown bare id (no mapping): default to Built-in/true.
            ModelRow {
                display_model: id.clone(),
                request_model: id.clone(),
                provider_id: "builtin".to_string(),
                provider_label: "Built-in".to_string(),
                available: true,
                description,
            }
        };
        rows.push(row);
    }

    for m in catalog {
        if !seen.insert(m.request_model.clone()) {
            continue;
        }
        let available = avail(&m.provider_id);
        // Prefer the catalog-carried description; fall back to the built-in
        // family blurb keyed on the wire id.
        let description = m.description.or_else(|| model_description(&m.request_model));
        rows.push(ModelRow {
            display_model: m.display_model,
            request_model: m.request_model,
            provider_id: m.provider_id,
            provider_label: m.provider_label,
            available,
            description,
        });
    }
    rows
}

/// A rendered line: a non-selectable group header, or a selectable row
/// (carrying its index into `ModelScreenState::rows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibleLine {
    /// Group header (e.g. "Recent", "`DeepSeek`").
    Header(String),
    /// Selectable row: index into `ModelScreenState::rows`.
    Item(usize),
}

/// Grouped `/model` picker state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelScreenState {
    /// All selectable rows (from `build_model_entries`).
    pub rows: Vec<ModelRow>,
    /// Recent selections, most-recent-first, as `(provider_id, request_model)`.
    pub recent: Vec<(String, String)>,
    /// Active model wire id (rendered with a `(current)` badge).
    pub current: String,
    /// Search query (printable chars typed in the picker).
    pub query: String,
    /// Highlighted index into the flat list of selectable items (not headers).
    pub selected: usize,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOutcome {
    /// Stay open (highlight moved / query edited / inert key).
    Stay,
    /// Enter — commit the highlighted row's `(provider_id, request_model)`.
    Commit {
        /// Provider grouping key of the chosen row.
        provider_id: String,
        /// Wire model id of the chosen row.
        request_model: String,
    },
    /// Enter on an UNCONFIGURED provider's row — launch `/connect <provider>`
    /// instead of switching (spec §6.4). For now the caller closes the picker
    /// (the `/connect` screen lands in 2d).
    Connect {
        /// Provider grouping key to connect (== `ModelRow.provider_id`).
        provider_id: String,
    },
    /// Esc — cancel with no change.
    Cancel,
}

impl ModelScreenState {
    /// Build from merged rows + recent keys + the active model.
    #[must_use]
    pub fn new(rows: Vec<ModelRow>, recent: Vec<(String, String)>, current: String) -> Self {
        Self {
            rows,
            recent,
            current,
            query: String::new(),
            selected: 0,
        }
    }

    /// Whether a row matches the query (case-insensitive substring over display
    /// name, provider label, and wire id). Empty query matches all.
    fn matches(&self, row: &ModelRow) -> bool {
        if self.query.is_empty() {
            return true;
        }
        let q = self.query.to_lowercase();
        row.display_model.to_lowercase().contains(&q)
            || row.provider_label.to_lowercase().contains(&q)
            || row.request_model.to_lowercase().contains(&q)
    }

    /// Ordered visible lines: a `Recent` group (rows whose
    /// `(provider_id, request_model)` is in `recent`, recent order), then one
    /// group per provider (first-seen order). Only query-matching rows appear;
    /// empty groups are omitted. A row may appear in both Recent and its group.
    #[must_use]
    pub fn visible_lines(&self) -> Vec<VisibleLine> {
        let mut out = Vec::new();

        let mut recent_items: Vec<usize> = Vec::new();
        for (pid, rm) in &self.recent {
            if let Some(idx) = self
                .rows
                .iter()
                .position(|r| &r.provider_id == pid && &r.request_model == rm)
            {
                if self.matches(&self.rows[idx]) && !recent_items.contains(&idx) {
                    recent_items.push(idx);
                }
            }
        }
        if !recent_items.is_empty() {
            out.push(VisibleLine::Header("Recent".to_string()));
            out.extend(recent_items.into_iter().map(VisibleLine::Item));
        }

        let mut seen_labels: Vec<String> = Vec::new();
        for r in &self.rows {
            if !seen_labels.contains(&r.provider_label) {
                seen_labels.push(r.provider_label.clone());
            }
        }
        for label in seen_labels {
            let items: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.provider_label == label && self.matches(r))
                .map(|(i, _)| i)
                .collect();
            if !items.is_empty() {
                out.push(VisibleLine::Header(label));
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
/// the search query (vim `j`/`k` nav is intentionally dropped so typing works);
/// Backspace deletes; Enter commits the highlighted row (or yields `Connect` for
/// an unavailable row); Esc cancels.
#[must_use]
pub fn handle_model_key(
    state: &mut ModelScreenState,
    key: crossterm::event::KeyCode,
) -> ModelOutcome {
    use crossterm::event::KeyCode;
    let count = state.selectable().len();
    match key {
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            ModelOutcome::Stay
        }
        KeyCode::Down => {
            if count > 0 {
                state.selected = (state.selected + 1).min(count - 1);
            }
            ModelOutcome::Stay
        }
        KeyCode::Char(c) => {
            state.query.push(c);
            state.selected = 0;
            ModelOutcome::Stay
        }
        KeyCode::Backspace => {
            state.query.pop();
            state.selected = 0;
            ModelOutcome::Stay
        }
        KeyCode::Enter => match state.selectable().get(state.selected) {
            Some(&idx) => {
                let row = &state.rows[idx];
                if row.available {
                    ModelOutcome::Commit {
                        provider_id: row.provider_id.clone(),
                        request_model: row.request_model.clone(),
                    }
                } else {
                    ModelOutcome::Connect {
                        provider_id: row.provider_id.clone(),
                    }
                }
            }
            None => ModelOutcome::Stay,
        },
        KeyCode::Esc => ModelOutcome::Cancel,
        _ => ModelOutcome::Stay,
    }
}

/// (model-header-not-bold-no-subheader) claude-code `ModelPicker`'s dim
/// sub-header line, verbatim.
pub const SUB_HEADER: &str = "Switch between Claude models. Applies to this session and future LingXi sessions. For other/previous model names, specify with --model.";

/// Render the grouped picker body (plain text; the iocraft layer wraps it).
///
/// (model-header-not-bold-no-subheader) The title is bold/accent-colored at
/// the component layer (this oracle can't carry color); the dim
/// [`SUB_HEADER`] line is plain text content, so it's included here.
#[must_use]
pub fn render_model_to_string(state: &ModelScreenState) -> String {
    let mut out = String::from("Select model\n");
    out.push_str(SUB_HEADER);
    out.push('\n');
    out.push_str(&format!("Search: {}\n", state.query));
    let lines = state.visible_lines();
    if lines.is_empty() {
        out.push_str("No models available.");
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
                } else {
                    "  "
                };
                out.push_str(marker);
                out.push_str(&row.display_model);
                out.push_str(&format!("  \u{00B7} {}", row.provider_label));
                if !row.available {
                    // Unconfigured provider: badge it; Enter launches `/connect`
                    // (spec §6.4). An unconfigured row is never the active model,
                    // so Connect + (current) are mutually exclusive.
                    out.push_str(" [Connect]");
                } else if row.request_model == state.current {
                    out.push_str(" (current)");
                }
                out.push('\n');
                // (model-no-row-descriptions) A dimmed one-line description below
                // the row, indented by 2 (claude-code `ListItem`:
                // `<Box paddingLeft={2}><Text color="inactive">{description}</Text>`).
                // The component layer dims it; this oracle carries the 2-space
                // indent + text. Empty/absent descriptions emit no extra line.
                // A catalog-sourced description could carry a newline; clamp to
                // its first line so a multi-line value can't break the 2-space
                // indent / column alignment of the following rows.
                if let Some(desc) = row.description.as_deref() {
                    let desc = desc.lines().next().unwrap_or("");
                    if !desc.is_empty() {
                        out.push_str("  ");
                        out.push_str(desc);
                        out.push('\n');
                    }
                }
                item_pos += 1;
            }
        }
    }
    // (model-footer-static-vs-byline) claude-code's ModelPicker footer is a
    // dim Byline of shortcut hints (Enter/confirm + Esc/exit), not a full
    // sentence. LingXi keeps the search feature, so `type to search` stays;
    // the `Press`/`to navigate`/`go back` wording is dropped.
    out.push_str(
        "type to search \u{00B7} Enter to select \u{00B7} Esc to cancel",
    );
    out
}

#[cfg(test)]
mod reducer_tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn rows() -> Vec<ModelRow> {
        build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![
                traits::orchestrator::ModelListing {
                    display_model: "DeepSeek Chat".to_string(),
                    request_model: "deepseek-chat".to_string(),
                    provider_id: "deepseek".to_string(),
                    provider_label: "DeepSeek".to_string(),
                    description: None,
                },
                traits::orchestrator::ModelListing {
                    display_model: "GPT-5.4 nano".to_string(),
                    request_model: "gpt-5.4-nano".to_string(),
                    provider_id: "github-copilot".to_string(),
                    provider_label: "GitHub Copilot".to_string(),
                    description: None,
                },
            ],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        )
    }

    #[test]
    fn recent_group_appears_first_and_resolves() {
        let st = ModelScreenState::new(
            rows(),
            vec![("github-copilot".to_string(), "gpt-5.4-nano".to_string())],
            "claude-opus-4-7".to_string(),
        );
        let lines = st.visible_lines();
        assert_eq!(lines[0], VisibleLine::Header("Recent".to_string()));
        let first = st.selectable()[0];
        assert_eq!(st.rows[first].request_model, "gpt-5.4-nano");
    }

    #[test]
    fn search_filters_and_resets_selection() {
        let mut st = ModelScreenState::new(rows(), vec![], "x".to_string());
        for c in "deep".chars() {
            assert_eq!(handle_model_key(&mut st, KeyCode::Char(c)), ModelOutcome::Stay);
        }
        let sel = st.selectable();
        assert_eq!(sel.len(), 1);
        assert_eq!(st.rows[sel[0]].request_model, "deepseek-chat");
        let _ = handle_model_key(&mut st, KeyCode::Backspace);
        assert!(!st.selectable().is_empty());
    }

    #[test]
    fn enter_commits_provider_and_model() {
        let mut st = ModelScreenState::new(rows(), vec![], "x".to_string());
        for c in "deepseek-chat".chars() {
            let _ = handle_model_key(&mut st, KeyCode::Char(c));
        }
        assert_eq!(
            handle_model_key(&mut st, KeyCode::Enter),
            ModelOutcome::Commit {
                provider_id: "deepseek".to_string(),
                request_model: "deepseek-chat".to_string()
            }
        );
        assert_eq!(handle_model_key(&mut st, KeyCode::Esc), ModelOutcome::Cancel);
    }

    #[test]
    fn nav_clamps_and_empty_is_inert() {
        let mut st = ModelScreenState::default();
        assert_eq!(handle_model_key(&mut st, KeyCode::Enter), ModelOutcome::Stay);
        assert_eq!(handle_model_key(&mut st, KeyCode::Down), ModelOutcome::Stay);
        assert_eq!(st.selected, 0);
    }

    #[test]
    fn enter_on_unconfigured_row_yields_connect() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let mut avail = BTreeMap::new();
        avail.insert("github-copilot".to_string(), false);
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![ModelListing {
                display_model: "GPT-5.4 nano".to_string(),
                request_model: "gpt-5.4-nano".to_string(),
                provider_id: "github-copilot".to_string(),
                provider_label: "GitHub Copilot".to_string(),
                description: None,
            }],
            &avail,
            &std::collections::BTreeMap::new(),
        );
        let mut st = ModelScreenState::new(rows, vec![], "claude-opus-4-7".to_string());
        for c in "gpt-5.4-nano".chars() {
            let _ = handle_model_key(&mut st, KeyCode::Char(c));
        }
        assert_eq!(
            handle_model_key(&mut st, KeyCode::Enter),
            ModelOutcome::Connect {
                provider_id: "github-copilot".to_string()
            }
        );
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    fn st() -> ModelScreenState {
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![traits::orchestrator::ModelListing {
                display_model: "DeepSeek Chat".to_string(),
                request_model: "deepseek-chat".to_string(),
                provider_id: "deepseek".to_string(),
                provider_label: "DeepSeek".to_string(),
                description: None,
            }],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        ModelScreenState::new(rows, vec![], "claude-opus-4-7".to_string())
    }

    #[test]
    fn renders_groups_headers_and_current_badge() {
        let out = render_model_to_string(&st());
        assert!(out.starts_with(&format!("Select model\n{SUB_HEADER}\nSearch: \n")), "{out}");
        assert!(out.contains("\nBuilt-in\n"));
        assert!(out.contains("\nDeepSeek\n"));
        assert!(out.contains("\u{276F} claude-opus-4-7  \u{00B7} Built-in (current)\n"));
        assert!(out.contains("  DeepSeek Chat  \u{00B7} DeepSeek\n"));
        // (model-no-row-descriptions) The opus row carries a built-in blurb,
        // rendered as a 2-space-indented dimmed sub-line directly below its row.
        assert!(
            out.contains(
                "\u{276F} claude-opus-4-7  \u{00B7} Built-in (current)\n  Most capable for complex work\n"
            ),
            "{out}"
        );
        // (model-footer-static-vs-byline) Byline-style footer.
        assert!(out.ends_with("type to search \u{00B7} Enter to select \u{00B7} Esc to cancel"));
    }

    #[test]
    fn catalog_description_renders_and_overrides_fallback() {
        // A catalog row carrying an explicit description renders it verbatim as a
        // 2-space-indented sub-line; an absent description on a known family id
        // falls back to the built-in blurb; an unknown id with no description
        // renders no sub-line.
        let rows = build_model_entries(
            vec![],
            vec![
                traits::orchestrator::ModelListing {
                    display_model: "DeepSeek Chat".to_string(),
                    request_model: "deepseek-chat".to_string(),
                    provider_id: "deepseek".to_string(),
                    provider_label: "DeepSeek".to_string(),
                    description: Some("Fast and cheap".to_string()),
                },
                traits::orchestrator::ModelListing {
                    display_model: "Sonnet".to_string(),
                    request_model: "claude-sonnet-4-6".to_string(),
                    provider_id: "anthropic".to_string(),
                    provider_label: "Anthropic".to_string(),
                    description: None,
                },
                traits::orchestrator::ModelListing {
                    display_model: "Mystery".to_string(),
                    request_model: "mystery-1".to_string(),
                    provider_id: "deepseek".to_string(),
                    provider_label: "DeepSeek".to_string(),
                    description: None,
                },
            ],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        let out = render_model_to_string(&ModelScreenState::new(rows, vec![], "x".to_string()));
        // Explicit catalog description (the first row is highlighted, so it carries
        // the `❯ ` marker; the description sub-line follows directly below).
        assert!(out.contains("DeepSeek Chat  \u{00B7} DeepSeek\n  Fast and cheap\n"), "{out}");
        // Fallback blurb for a sonnet id with no catalog description.
        assert!(out.contains("  Sonnet  \u{00B7} Anthropic\n  Best for everyday tasks\n"), "{out}");
        // Unknown id, no description ⇒ no sub-line (the line after the row is NOT a
        // 2-space-indented description; it's the next row/header/blank line).
        assert!(out.contains("  Mystery  \u{00B7} DeepSeek\n"), "{out}");
        assert!(!out.contains("  Mystery  \u{00B7} DeepSeek\n  "), "{out}");
    }

    #[test]
    fn multiline_catalog_description_clamps_to_first_line() {
        // (review) A catalog description carrying a newline must not break the
        // 2-space-indent contract — only its first line is rendered.
        let rows = build_model_entries(
            vec![],
            vec![traits::orchestrator::ModelListing {
                display_model: "Multi".to_string(),
                request_model: "multi-1".to_string(),
                provider_id: "deepseek".to_string(),
                provider_label: "DeepSeek".to_string(),
                description: Some("First line\nSecond line".to_string()),
            }],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        let out = render_model_to_string(&ModelScreenState::new(rows, vec![], "x".to_string()));
        assert!(out.contains("\n  First line\n"), "first line only, got: {out}");
        assert!(!out.contains("Second line"), "second line dropped, got: {out}");
    }

    #[test]
    fn empty_shows_locked_state() {
        let out = render_model_to_string(&ModelScreenState::default());
        assert_eq!(
            out,
            format!("Select model\n{SUB_HEADER}\nSearch: \nNo models available.")
        );
    }

    fn st_with_unconfigured() -> ModelScreenState {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let mut avail = BTreeMap::new();
        avail.insert("github-copilot".to_string(), false);
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![ModelListing {
                display_model: "GPT-5.4 nano".to_string(),
                request_model: "gpt-5.4-nano".to_string(),
                provider_id: "github-copilot".to_string(),
                provider_label: "GitHub Copilot".to_string(),
                description: None,
            }],
            &avail,
            &std::collections::BTreeMap::new(),
        );
        ModelScreenState::new(rows, vec![], "claude-opus-4-7".to_string())
    }

    #[test]
    fn renders_connect_badge_on_unconfigured_provider() {
        let out = render_model_to_string(&st_with_unconfigured());
        assert!(out.contains("\u{276F} claude-opus-4-7  \u{00B7} Built-in (current)\n"));
        assert!(out.contains("GPT-5.4 nano  \u{00B7} GitHub Copilot [Connect]\n"));
        assert!(!out.contains("GPT-5.4 nano  \u{00B7} GitHub Copilot (current)"));
    }
}

#[cfg(test)]
mod entries_tests {
    use super::*;
    use traits::orchestrator::ModelListing;

    #[test]
    fn merges_existing_and_catalog_with_groups() {
        let existing = vec![
            "claude-opus-4-7".to_string(),
            "openai/gpt-4o".to_string(),
            "@fast".to_string(),
        ];
        let catalog = vec![ModelListing {
            display_model: "DeepSeek Chat".to_string(),
            request_model: "deepseek-chat".to_string(),
            provider_id: "deepseek".to_string(),
            provider_label: "DeepSeek".to_string(),
            description: None,
        }];
        let rows = build_model_entries(
            existing,
            catalog,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );

        let opus = rows.iter().find(|r| r.request_model == "claude-opus-4-7").unwrap();
        assert_eq!(opus.provider_label, "Built-in");
        let gpt = rows.iter().find(|r| r.request_model == "openai/gpt-4o").unwrap();
        assert_eq!(gpt.display_model, "gpt-4o");
        assert_eq!(gpt.provider_label, "OpenAI");
        let alias = rows.iter().find(|r| r.request_model == "@fast").unwrap();
        assert_eq!(alias.provider_label, "Aliases");
        let ds = rows.iter().find(|r| r.request_model == "deepseek-chat").unwrap();
        assert_eq!(ds.display_model, "DeepSeek Chat");
        assert_eq!(ds.provider_label, "DeepSeek");
    }

    #[test]
    fn dedups_by_request_model_existing_wins() {
        let rows = build_model_entries(
            vec!["deepseek-chat".to_string()],
            vec![ModelListing {
                display_model: "DeepSeek Chat".to_string(),
                request_model: "deepseek-chat".to_string(),
                provider_id: "deepseek".to_string(),
                provider_label: "DeepSeek".to_string(),
                description: None,
            }],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows.iter().filter(|r| r.request_model == "deepseek-chat").count(), 1);
        assert_eq!(rows[0].provider_label, "Built-in");
    }

    #[test]
    fn availability_joins_by_provider_id_default_true() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let existing = vec!["claude-opus-4-7".to_string()];
        let catalog = vec![
            ModelListing {
                display_model: "DeepSeek Chat".to_string(),
                request_model: "deepseek-chat".to_string(),
                provider_id: "deepseek".to_string(),
                provider_label: "DeepSeek".to_string(),
                description: None,
            },
            ModelListing {
                display_model: "GPT-5.4 nano".to_string(),
                request_model: "gpt-5.4-nano".to_string(),
                provider_id: "github-copilot".to_string(),
                provider_label: "GitHub Copilot".to_string(),
                description: None,
            },
        ];
        let mut avail = BTreeMap::new();
        avail.insert("deepseek".to_string(), true);
        avail.insert("github-copilot".to_string(), false);
        let rows = build_model_entries(existing, catalog, &avail, &std::collections::BTreeMap::new());
        assert!(
            rows.iter().find(|r| r.request_model == "claude-opus-4-7").unwrap().available,
            "absent provider defaults available"
        );
        assert!(rows.iter().find(|r| r.request_model == "deepseek-chat").unwrap().available);
        assert!(!rows.iter().find(|r| r.request_model == "gpt-5.4-nano").unwrap().available);
    }

    #[test]
    fn empty_availability_map_keeps_all_rows_available() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string(), "openai/gpt-4o".to_string()],
            vec![ModelListing {
                display_model: "DeepSeek Chat".to_string(),
                request_model: "deepseek-chat".to_string(),
                provider_id: "deepseek".to_string(),
                provider_label: "DeepSeek".to_string(),
                description: None,
            }],
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(rows.iter().all(|r| r.available), "empty map → all available");
    }

    #[test]
    fn user_provider_model_maps_to_its_profile_and_gates_on_availability() {
        use std::collections::BTreeMap;
        // I1/I2 regression: a bare available-model id from a USER provider (no
        // catalog listing, no `/` in the id) must resolve to its own profile +
        // label and gate on availability — not silently fall into "Built-in"/true.
        let existing = vec!["claude-opus-4-7".to_string(), "llama-3.3-70b".to_string()];
        let catalog: Vec<traits::orchestrator::ModelListing> = vec![];
        let mut model_providers: BTreeMap<String, (String, String)> = BTreeMap::new();
        model_providers
            .insert("llama-3.3-70b".to_string(), ("groq".to_string(), "Groq".to_string()));

        // Unconfigured: `groq:false` → row keyed to `groq`, available == false
        // (so it badges + selecting routes to ModelOutcome::Connect).
        let mut avail = BTreeMap::new();
        avail.insert("groq".to_string(), false);
        let rows = build_model_entries(existing.clone(), catalog.clone(), &avail, &model_providers);
        let llama = rows.iter().find(|r| r.request_model == "llama-3.3-70b").unwrap();
        assert_eq!(llama.provider_id, "groq", "user provider id, not 'builtin'");
        assert_eq!(llama.provider_label, "Groq");
        assert!(!llama.available, "unconfigured groq → not available (badges + Connect)");
        // The true built-in (no mapping) still groups under Built-in and stays available.
        let opus = rows.iter().find(|r| r.request_model == "claude-opus-4-7").unwrap();
        assert_eq!(opus.provider_id, "builtin");
        assert_eq!(opus.provider_label, "Built-in");
        assert!(opus.available);

        // Configured: `groq:true` → available == true (routes directly).
        let mut avail_ok = BTreeMap::new();
        avail_ok.insert("groq".to_string(), true);
        let rows_ok = build_model_entries(existing, catalog, &avail_ok, &model_providers);
        let llama_ok = rows_ok.iter().find(|r| r.request_model == "llama-3.3-70b").unwrap();
        assert_eq!(llama_ok.provider_id, "groq");
        assert!(llama_ok.available, "configured groq → available");
    }

    #[test]
    fn unmapped_bare_id_still_falls_back_to_builtin() {
        use std::collections::BTreeMap;
        // A bare id with no mapping is genuinely unknown → Built-in/true (no regression).
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![],
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let opus = rows.iter().find(|r| r.request_model == "claude-opus-4-7").unwrap();
        assert_eq!(opus.provider_id, "builtin");
        assert_eq!(opus.provider_label, "Built-in");
        assert!(opus.available);
    }

    #[test]
    fn descriptions_populate_from_catalog_and_family_fallback() {
        // Existing (routable) rows pick up the built-in family blurb by id family;
        // catalog rows prefer their own description, else fall back to the family
        // blurb; an unrecognized id with no description stays `None`.
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string(), "openai/gpt-4o".to_string()],
            vec![
                ModelListing {
                    display_model: "DeepSeek Chat".to_string(),
                    request_model: "deepseek-chat".to_string(),
                    provider_id: "deepseek".to_string(),
                    provider_label: "DeepSeek".to_string(),
                    description: Some("Fast and cheap".to_string()),
                },
                ModelListing {
                    display_model: "Haiku".to_string(),
                    request_model: "claude-haiku-4-5".to_string(),
                    provider_id: "anthropic".to_string(),
                    provider_label: "Anthropic".to_string(),
                    description: None,
                },
            ],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        // Existing opus → family fallback.
        let opus = rows.iter().find(|r| r.request_model == "claude-opus-4-7").unwrap();
        assert_eq!(opus.description.as_deref(), Some("Most capable for complex work"));
        // Non-family existing id → no description.
        let gpt = rows.iter().find(|r| r.request_model == "openai/gpt-4o").unwrap();
        assert_eq!(gpt.description, None);
        // Catalog row with explicit description wins.
        let ds = rows.iter().find(|r| r.request_model == "deepseek-chat").unwrap();
        assert_eq!(ds.description.as_deref(), Some("Fast and cheap"));
        // Catalog row, no description, haiku family → fallback blurb.
        let haiku = rows.iter().find(|r| r.request_model == "claude-haiku-4-5").unwrap();
        assert_eq!(haiku.description.as_deref(), Some("Fastest for quick answers"));
    }
}
