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

// The "latest few" curation whitelist is shared with the mobile/CLI listings —
// it lives in `traits` so there is ONE source of truth (a stale copy here once
// keyed GLM on its slice filename instead of the profile name and hid the group).
use traits::is_curated_model;

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
    // Canonical labels — kept in sync with the orchestrator catalog's
    // `provider_label` (provider_adapter.rs) so a `provider/model` live row and a
    // catalog row for the same provider share one group header (visible_lines
    // groups by label).
    match prefix {
        "anthropic" => "Anthropic",
        "openrouter" => "OpenRouter",
        "deepseek" => "DeepSeek",
        "glm-coding" | "zhipuai-coding-plan" => "GLM (coding)",
        "zai" => "Z.AI",
        "openai" => "OpenAI",
        "openai-chatgpt" => "OpenAI (ChatGPT login)",
        "github-copilot" => "GitHub Copilot",
        "gemini" => "Google Gemini",
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

    // (multi-provider keying) The live list (`existing`) carries DISPLAY names
    // (`model.name`, e.g. "GPT-5.5"), whereas the catalog and `model_providers`
    // key by the WIRE id (`model.id`, e.g. "gpt-5.5"). A catalog provider's live
    // row therefore misses `model_providers` and would fall through to a mis-keyed
    // "Built-in"/available-true row — and because name != id it never dedups
    // against the correctly-keyed catalog row, leaving a silent duplicate (and a
    // row whose `provider_id` can't even be switched to). Pre-index the catalog's
    // display names so the Built-in fallback can defer to the catalog row instead.
    let catalog_display_names: std::collections::HashSet<String> =
        catalog.iter().map(|m| m.display_model.clone()).collect();

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
        } else if id.starts_with("claude-") {
            // Bare Claude ids (claude-sonnet-4-6, claude-opus-4-8, claude-fable-5…)
            // belong to Anthropic. Key them to the "anthropic" profile so the
            // group gates on the real Anthropic credential under the only-connected
            // curation, instead of the unkeyed "builtin" bucket (which is never in
            // `configured`, so once any provider was connected the whole Anthropic
            // group vanished).
            ModelRow {
                display_model: id.clone(),
                request_model: id.clone(),
                provider_id: "anthropic".to_string(),
                provider_label: "Anthropic".to_string(),
                available: avail("anthropic"),
                description,
            }
        } else {
            // A catalog provider's live row arrives as a DISPLAY name; the catalog
            // already carries the correctly-keyed row (real provider_id +
            // availability gating) for it, so defer to that rather than emit a
            // mis-keyed Built-in duplicate.
            if catalog_display_names.contains(&id) {
                continue;
            }
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

    // Catalog dedup: a live row already represents its model (dedup against `seen`
    // by request_model), but the SAME wire id can be legitimately offered by
    // MULTIPLE providers (a GitHub Copilot proxy of `gpt-5.5` vs first-party
    // OpenAI; `glm-*` shared by `zai` and `glm-coding`). Dedup catalog rows among
    // themselves by (provider_id, request_model) so each provider keeps its own
    // separately-gated row instead of the first-seen provider swallowing the rest.
    let mut seen_catalog: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    for m in catalog {
        if seen.contains(&m.request_model)
            || !seen_catalog.insert((m.provider_id.clone(), m.request_model.clone()))
        {
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
    /// (opencode-style curation) Provider ids the user has ACTUALLY configured
    /// (an explicit usable credential in `provider_availability`). When NON-EMPTY,
    /// [`Self::visible_lines`] curates the provider groups to ONLY these (the
    /// `Recent` group always shows) — hiding the big unconfigured catalog dump.
    /// EMPTY (the default; tests / headless) shows every group, i.e. the
    /// historical claude-code show-all-with-`[Connect]`-badges behavior.
    pub configured: std::collections::BTreeSet<String>,
    /// (curation) Whether to trim rows to the [`is_curated_model`] "latest few"
    /// even when `configured` is empty. The live `/model` open path sets this so a
    /// NO-AUTH session (nothing configured yet) still shows a curated short list
    /// per provider — badged `[Connect]` — instead of dumping the whole ~460-model
    /// assembled catalog. Left `false` on the test/headless path so the
    /// `build_model_entries` byte-parity show-all tests are preserved.
    pub curate: bool,
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
            configured: std::collections::BTreeSet::new(),
            curate: false,
        }
    }

    /// (curation) Set the configured-provider set (the live `/model` open path
    /// passes the providers with an explicit usable credential). See
    /// [`Self::configured`]. Empty leaves the show-all behavior.
    pub fn set_configured(&mut self, configured: std::collections::BTreeSet<String>) {
        self.configured = configured;
    }

    /// (curation) Enable the "latest few" trim regardless of `configured`. The
    /// live `/model` open path sets this; see [`Self::curate`].
    pub fn set_curate(&mut self, curate: bool) {
        self.curate = curate;
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

    /// Whether a provider-group row should be shown, given the "latest few"
    /// curation. claude-code curates the picker to a hand-picked short list
    /// (`modelOptions.ts`) rather than dumping the whole catalog (~460 models);
    /// we mirror that with [`is_curated_model`]. Curation runs on the live path —
    /// either because the user has configured providers (`configured` non-empty)
    /// OR because [`Self::curate`] is set (a no-auth live session, so the catalog
    /// isn't dumped). The headless/test path leaves both unset → show-all,
    /// preserving the `build_model_entries` byte-parity tests. The user's CURRENT
    /// and RECENT models are always kept so an off-list active model stays
    /// visible/selectable.
    #[must_use]
    fn is_shown_model(&self, r: &ModelRow) -> bool {
        if !self.curate && self.configured.is_empty() {
            return true;
        }
        is_curated_model(&r.provider_id, &r.request_model)
            || r.request_model == self.current
            || self
                .recent
                .iter()
                .any(|(p, m)| p == &r.provider_id && m == &r.request_model)
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
                .filter(|(_, r)| r.provider_label == label && self.matches(r) && self.is_shown_model(r))
                .map(|(i, _)| i)
                .collect();
            if items.is_empty() {
                continue;
            }
            // (opencode-style curation) When a configured-provider set is present,
            // hide groups whose provider the user hasn't configured (the big
            // unconfigured catalog dump). Recent already surfaced its rows above,
            // so a recent model still shows there. Empty set = show every group.
            if !self.configured.is_empty()
                && !items
                    .iter()
                    .any(|&i| self.configured.contains(&self.rows[i].provider_id))
            {
                continue;
            }
            out.push(VisibleLine::Header(label));
            out.extend(items.into_iter().map(VisibleLine::Item));
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
        // claude-* bare ids now group under Anthropic (gate on the anthropic credential).
        assert!(out.contains("\nAnthropic\n"));
        assert!(out.contains("\nDeepSeek\n"));
        assert!(out.contains("\u{276F} claude-opus-4-7  \u{00B7} Anthropic (current)\n"));
        assert!(out.contains("  DeepSeek Chat  \u{00B7} DeepSeek\n"));
        // (model-no-row-descriptions) The opus row carries a built-in blurb,
        // rendered as a 2-space-indented dimmed sub-line directly below its row.
        assert!(
            out.contains(
                "\u{276F} claude-opus-4-7  \u{00B7} Anthropic (current)\n  Most capable for complex work\n"
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
        assert!(out.contains("\u{276F} claude-opus-4-7  \u{00B7} Anthropic (current)\n"));
        assert!(out.contains("GPT-5.4 nano  \u{00B7} GitHub Copilot [Connect]\n"));
        assert!(!out.contains("GPT-5.4 nano  \u{00B7} GitHub Copilot (current)"));
    }

    fn shown_request_models(st: &ModelScreenState) -> Vec<String> {
        st.visible_lines()
            .iter()
            .filter_map(|l| match l {
                VisibleLine::Item(i) => Some(st.rows[*i].request_model.clone()),
                VisibleLine::Header(_) => None,
            })
            .collect()
    }

    #[test]
    fn live_no_auth_curates_but_keeps_all_provider_groups() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        // No-auth live session: nothing configured, but `curate` set → trim each
        // provider to its curated short list (no ~hundreds dump) while STILL
        // showing every provider group (badged [Connect] via empty availability).
        let catalog = vec![
            ModelListing { display_model: "GPT-5.5".into(), request_model: "gpt-5.5".into(), provider_id: "openai".into(), provider_label: "OpenAI".into(), description: None },
            ModelListing { display_model: "GPT-4o".into(), request_model: "gpt-4o".into(), provider_id: "openai".into(), provider_label: "OpenAI".into(), description: None },
            ModelListing { display_model: "Gemini 3.5 Flash".into(), request_model: "gemini-3.5-flash".into(), provider_id: "gemini".into(), provider_label: "Google Gemini".into(), description: None },
        ];
        let rows = build_model_entries(vec![], catalog, &BTreeMap::new(), &BTreeMap::new());
        let mut st = ModelScreenState::new(rows, vec![], "x".into());
        st.set_curate(true); // live path; configured stays empty (no auth)

        let shown = shown_request_models(&st);
        assert!(shown.contains(&"gpt-5.5".to_string()), "curated kept: {shown:?}");
        assert!(shown.contains(&"gemini-3.5-flash".to_string()), "curated kept");
        assert!(!shown.contains(&"gpt-4o".to_string()), "non-curated trimmed despite nothing configured");
        // Every provider group still renders (not hidden — nothing configured).
        let headers: Vec<String> = st.visible_lines().iter().filter_map(|l| match l {
            VisibleLine::Header(h) => Some(h.clone()),
            VisibleLine::Item(_) => None,
        }).collect();
        assert!(headers.iter().any(|h| h == "OpenAI"), "{headers:?}");
        assert!(headers.iter().any(|h| h == "Google Gemini"));
    }

    #[test]
    fn headless_show_all_preserved_when_not_curating() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        // Default (curate=false, nothing configured) keeps the show-all behavior
        // the build_model_entries byte-parity tests depend on — incl. non-curated.
        let catalog = vec![ModelListing { display_model: "GPT-4o".into(), request_model: "gpt-4o".into(), provider_id: "openai".into(), provider_label: "OpenAI".into(), description: None }];
        let rows = build_model_entries(vec![], catalog, &BTreeMap::new(), &BTreeMap::new());
        let st = ModelScreenState::new(rows, vec![], "x".into());
        assert!(shown_request_models(&st).contains(&"gpt-4o".to_string()), "show-all preserved");
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
        assert_eq!(opus.provider_label, "Anthropic");
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
    fn live_display_name_defers_to_catalog_no_builtin_duplicate() {
        use std::collections::BTreeMap;
        // Production reality (the multi-provider bug this guards): the live list
        // (`list_available_models`) carries DISPLAY names (model.name, e.g.
        // "GPT-5.5"), while the catalog keys by the wire id (model.id, e.g.
        // "gpt-5.5"). The two must NOT produce a mis-keyed "Built-in"/available
        // duplicate that never dedups (name != id) — the correctly-keyed catalog
        // row (real provider + [Connect] gating) represents the model.
        let existing = vec!["GPT-5.5".to_string(), "Gemini 3.5 Flash".to_string()];
        let catalog = vec![
            ModelListing {
                display_model: "GPT-5.5".to_string(),
                request_model: "gpt-5.5".to_string(),
                provider_id: "openai".to_string(),
                provider_label: "OpenAI".to_string(),
                description: None,
            },
            ModelListing {
                display_model: "Gemini 3.5 Flash".to_string(),
                request_model: "gemini-3.5-flash".to_string(),
                provider_id: "gemini".to_string(),
                provider_label: "Google Gemini".to_string(),
                description: None,
            },
        ];
        let mut avail = BTreeMap::new();
        avail.insert("openai".to_string(), true);
        avail.insert("gemini".to_string(), false);
        let rows = build_model_entries(existing, catalog, &avail, &BTreeMap::new());
        // No mis-keyed Built-in rows; exactly one correctly-keyed row per model.
        assert!(rows.iter().all(|r| r.provider_id != "builtin"), "no builtin dupes: {rows:?}");
        assert_eq!(rows.len(), 2, "one row per model, got {rows:?}");
        let gpt = rows.iter().find(|r| r.request_model == "gpt-5.5").unwrap();
        assert_eq!(gpt.provider_id, "openai");
        assert_eq!(gpt.display_model, "GPT-5.5");
        assert!(gpt.available, "configured openai → available");
        let gem = rows.iter().find(|r| r.request_model == "gemini-3.5-flash").unwrap();
        assert_eq!(gem.provider_id, "gemini");
        assert!(!gem.available, "unconfigured gemini → [Connect]");
    }

    #[test]
    fn catalog_same_wire_id_under_different_providers_keeps_both() {
        // A wire id offered by two providers (e.g. GitHub Copilot's proxy of
        // gpt-5.5 vs first-party OpenAI; glm-* shared by zai + glm-coding) must
        // keep BOTH provider rows — catalog dedup is per (provider_id,
        // request_model), not request_model alone, so the first-seen provider does
        // not silently swallow the rest.
        let catalog = vec![
            ModelListing {
                display_model: "GPT-5.5".to_string(),
                request_model: "gpt-5.5".to_string(),
                provider_id: "openai".to_string(),
                provider_label: "OpenAI".to_string(),
                description: None,
            },
            ModelListing {
                display_model: "GPT-5.5".to_string(),
                request_model: "gpt-5.5".to_string(),
                provider_id: "github-copilot".to_string(),
                provider_label: "GitHub Copilot".to_string(),
                description: None,
            },
        ];
        let rows = build_model_entries(
            vec![],
            catalog,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(
            rows.iter().filter(|r| r.request_model == "gpt-5.5").count(),
            2,
            "both provider variants kept: {rows:?}"
        );
        assert!(rows.iter().any(|r| r.provider_id == "openai"));
        assert!(rows.iter().any(|r| r.provider_id == "github-copilot"));
    }

    // NOTE: the curation whitelist (`is_curated_model`, incl. the glm-coding
    // profile-name regression) now lives in `traits` with its own tests
    // (`traits::curated_model_tests`) — one source of truth shared with mobile/CLI.

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
        let existing = vec!["mystery-model-9".to_string(), "llama-3.3-70b".to_string()];
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
        // A genuinely-unmapped non-claude id still groups under Built-in and stays available.
        let other = rows.iter().find(|r| r.request_model == "mystery-model-9").unwrap();
        assert_eq!(other.provider_id, "builtin");
        assert_eq!(other.provider_label, "Built-in");
        assert!(other.available);

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
        // A bare id with no mapping that is NOT a claude-* id is genuinely unknown
        // → Built-in/true (no regression). (claude-* ids now key to Anthropic.)
        let rows = build_model_entries(
            vec!["mystery-model-9".to_string()],
            vec![],
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let m = rows.iter().find(|r| r.request_model == "mystery-model-9").unwrap();
        assert_eq!(m.provider_id, "builtin");
        assert_eq!(m.provider_label, "Built-in");
        assert!(m.available);
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
