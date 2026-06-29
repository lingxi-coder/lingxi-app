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

/// The real login method for a provider, derived from the catalog auth tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMethod {
    ApiKey,
    CopilotDevice,
    OAuthSoon,
    /// (T2b) A LIVE first-party OAuth browser sign-in offered as an explicit
    /// menu choice (Anthropic Pro/Max). Distinct from `OAuthSoon` so the choice
    /// label is honest ("Sign in…", not "coming soon").
    Oauth,
}

impl ConnectMethod {
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "api_key" => Self::ApiKey,
            "copilot_device" => Self::CopilotDevice,
            _ => Self::OAuthSoon, // "oauth" + any unknown → honest "coming soon"
        }
    }

    /// Human suffix appended to the row description (so it reflects reality).
    fn suffix(self) -> &'static str {
        match self {
            Self::ApiKey => " \u{2014} API key",
            Self::CopilotDevice => " \u{2014} device sign-in",
            Self::OAuthSoon => " \u{2014} browser sign-in (coming soon)",
            Self::Oauth => " \u{2014} Pro/Max sign-in",
        }
    }

    /// Human menu label for the method-choice step (distinct from `suffix`,
    /// which is the picker-row trailer). Used only by the MethodChoice screen.
    #[must_use]
    pub fn choice_label(self) -> &'static str {
        match self {
            Self::Oauth => "Sign in with Claude Pro/Max",
            Self::ApiKey => "Use an API key",
            Self::CopilotDevice => "Sign in with GitHub",
            Self::OAuthSoon => "Browser sign-in (coming soon)",
        }
    }
}

/// The ordered set of login methods a provider offers. Single-method providers
/// yield one entry (their catalog tag); Anthropic is special-cased to BOTH a
/// live Pro/Max OAuth sign-in AND its API key (the engine tag stays "api_key";
/// the TUI knows the OAuth backend — `EngineOAuthConnect`'s "anthropic" arm — is
/// also live). OAuth is listed first (claude-code order: "Sign in" then "API key").
#[must_use]
pub fn provider_methods(profile_name: &str, tag: Option<&str>) -> Vec<ConnectMethod> {
    match profile_name {
        "anthropic" => vec![ConnectMethod::Oauth, ConnectMethod::ApiKey],
        _ => vec![tag.map(ConnectMethod::from_tag).unwrap_or(ConnectMethod::ApiKey)],
    }
}

/// Curated human label for a profile id (e.g. "anthropic" -> "Anthropic"),
/// falling back to title-case. Public so the MethodChoice title and the OAuth
/// success message can show a real label instead of the raw slug.
#[must_use]
pub fn provider_label(profile_name: &str) -> String {
    connect_display_meta(profile_name).label
}

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
    /// The real login method, derived from the catalog auth tag.
    pub method: ConnectMethod,
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

struct DisplayMeta {
    label: String,
    blurb: String,
    popular: bool,
}

fn connect_display_meta(profile_name: &str) -> DisplayMeta {
    // (id, label, blurb, popular) — blurb is "what it is", NOT the method (the
    // method suffix is appended separately so descriptions can't lie).
    let curated: &[(&str, &str, &str, bool)] = &[
        ("anthropic", "Anthropic", "Claude models", true),
        ("openai", "OpenAI", "GPT models", true),
        ("openai-chatgpt", "OpenAI (ChatGPT)", "ChatGPT Plus/Pro", true),
        ("github-copilot", "GitHub Copilot", "Your Copilot subscription", true),
        ("gemini", "Google Gemini", "Gemini models", true),
        ("deepseek", "DeepSeek", "Chat / Reasoner", true),
        ("openrouter", "OpenRouter", "Unified gateway to many models", false),
        ("zai", "Z.AI", "GLM models", false),
        ("glm-coding", "GLM Coding Plan", "Zhipu coding-plan subscription", false),
    ];
    if let Some((_, label, blurb, popular)) =
        curated.iter().find(|(id, ..)| *id == profile_name)
    {
        return DisplayMeta {
            label: (*label).to_string(),
            blurb: (*blurb).to_string(),
            popular: *popular,
        };
    }
    DisplayMeta { label: title_case(profile_name), blurb: "Provider".to_string(), popular: false }
}

/// "brand-new-provider" → "Brand New Provider" (split on '-'/'_').
fn title_case(id: &str) -> String {
    id.split(|c| c == '-' || c == '_')
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build picker rows from the REAL catalog (`auth_methods` = the provider set +
/// method) joined with availability (connected, keyed directly by profile_name).
/// An empty availability map ⇒ every row unconnected (no `✓`s).
#[must_use]
pub fn connect_rows_from(
    auth_methods: &BTreeMap<String, String>,
    availability: &BTreeMap<String, bool>,
) -> Vec<ConnectRow> {
    auth_methods
        .iter()
        .map(|(profile_name, tag)| {
            let method = ConnectMethod::from_tag(tag);
            let meta = connect_display_meta(profile_name);
            ConnectRow {
                provider_id: profile_name.clone(),
                label: meta.label,
                description: format!("{}{}", meta.blurb, method.suffix()),
                popular: meta.popular,
                connected: availability.get(profile_name).copied().unwrap_or(false),
                method,
            }
        })
        .collect()
}

/// Grouped `/connect` picker state. Pure; mirrors `ModelScreenState`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectPickerState {
    /// All selectable rows (from [`connect_rows_from`]).
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

    /// Build from `auth_methods` + `availability`; the primary data-driven constructor.
    #[must_use]
    pub fn from_connectable(
        auth_methods: &BTreeMap<String, String>,
        availability: &BTreeMap<String, bool>,
    ) -> Self {
        Self::new(connect_rows_from(auth_methods, availability))
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

impl ConnectPickerState {
    /// The provider id of the currently highlighted selectable row (None if empty).
    #[must_use]
    pub fn highlighted_provider_id(&self) -> Option<&str> {
        let idx = *self.selectable().get(self.selected)?;
        Some(self.rows.get(idx)?.provider_id.as_str())
    }
}

/// Detail lines for the highlighted provider: connected state, its models
/// (filtered from `model_providers` by profile name), and its login method(s).
/// Reuses the `/model` data; no engine change. Graceful on unknown/empty.
#[must_use]
pub fn provider_detail_lines(
    provider_id: &str,
    label: &str,
    connected: bool,
    methods: &[ConnectMethod],
    model_providers: &std::collections::BTreeMap<String, (String, String)>,
) -> Vec<String> {
    let mut out = Vec::new();
    out.push(if connected {
        format!("{label} \u{2014} \u{2713} connected")
    } else {
        format!("{label} \u{2014} not connected")
    });
    // its models (request-model ids whose profile == provider_id), capped + elided
    let mut models: Vec<&str> = model_providers
        .iter()
        .filter(|(_, (profile, _))| profile == provider_id)
        .map(|(m, _)| m.as_str())
        .collect();
    models.sort_unstable();
    if models.is_empty() {
        out.push("Models: (no models listed)".to_string());
    } else {
        let shown: Vec<&str> = models.iter().take(4).copied().collect();
        let more = models.len().saturating_sub(shown.len());
        let mut s = format!("Models: {}", shown.join(", "));
        if more > 0 {
            s.push_str(&format!(", +{more} more"));
        }
        out.push(s);
    }
    // login method(s)
    let m: Vec<&str> = methods
        .iter()
        .map(|x| match x {
            ConnectMethod::Oauth => "Pro/Max sign-in",
            ConnectMethod::ApiKey => "API key",
            ConnectMethod::CopilotDevice => "GitHub sign-in",
            ConnectMethod::OAuthSoon => "browser sign-in (coming soon)",
        })
        .collect();
    out.push(format!("Sign in: {}", m.join(" or ")));
    out
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
mod t3_tests {
    use super::*;

    #[test]
    fn method_from_tag_table() {
        assert_eq!(ConnectMethod::from_tag("api_key"), ConnectMethod::ApiKey);
        assert_eq!(ConnectMethod::from_tag("copilot_device"), ConnectMethod::CopilotDevice);
        assert_eq!(ConnectMethod::from_tag("oauth"), ConnectMethod::OAuthSoon);
        assert_eq!(ConnectMethod::from_tag("???"), ConnectMethod::OAuthSoon); // unknown → honest soon
    }

    #[test]
    fn provider_methods_anthropic_offers_oauth_then_api_key() {
        // Anthropic is the dual-method special case: live Pro/Max OAuth first,
        // API key second (engine tag stays "api_key").
        assert_eq!(
            provider_methods("anthropic", Some("api_key")),
            vec![ConnectMethod::Oauth, ConnectMethod::ApiKey]
        );
        // Single-method providers yield exactly their catalog tag's method.
        assert_eq!(provider_methods("openai", Some("api_key")), vec![ConnectMethod::ApiKey]);
        assert_eq!(
            provider_methods("openai-chatgpt", Some("oauth")),
            vec![ConnectMethod::OAuthSoon]
        );
        assert_eq!(
            provider_methods("github-copilot", Some("copilot_device")),
            vec![ConnectMethod::CopilotDevice]
        );
        // Typed-unknown (no tag) falls back to ApiKey, preserving today's behaviour.
        assert_eq!(provider_methods("typed-unknown", None), vec![ConnectMethod::ApiKey]);
    }

    #[test]
    fn choice_label_and_provider_label_values() {
        assert_eq!(ConnectMethod::Oauth.choice_label(), "Sign in with Claude Pro/Max");
        assert_eq!(ConnectMethod::ApiKey.choice_label(), "Use an API key");
        assert_eq!(ConnectMethod::CopilotDevice.choice_label(), "Sign in with GitHub");
        assert_eq!(provider_label("anthropic"), "Anthropic");
        assert_eq!(provider_label("brand-new-provider"), "Brand New Provider");
    }

    #[test]
    fn rows_are_data_driven_with_trustworthy_check_and_method() {
        let mut auth = std::collections::BTreeMap::new();
        auth.insert("anthropic".to_string(), "api_key".to_string());
        auth.insert("github-copilot".to_string(), "copilot_device".to_string());
        auth.insert("brand-new-provider".to_string(), "api_key".to_string()); // not in curated map
        let mut avail = std::collections::BTreeMap::new();
        avail.insert("anthropic".to_string(), true); // connected
        // github-copilot absent ⇒ not connected
        let rows = connect_rows_from(&auth, &avail);

        let a = rows.iter().find(|r| r.provider_id == "anthropic").unwrap();
        assert_eq!(a.label, "Anthropic"); // curated label
        assert!(a.connected); // ✓ keyed by profile_name
        assert_eq!(a.method, ConnectMethod::ApiKey);
        assert!(a.description.ends_with("API key")); // method-derived suffix, cannot lie

        let g = rows.iter().find(|r| r.provider_id == "github-copilot").unwrap();
        assert!(!g.connected);
        assert_eq!(g.method, ConnectMethod::CopilotDevice);

        let n = rows.iter().find(|r| r.provider_id == "brand-new-provider").unwrap();
        assert_eq!(n.label, "Brand New Provider"); // title-cased fallback — still renders
    }
}

#[cfg(test)]
mod reducer_tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn rows() -> Vec<ConnectRow> {
        let mut auth = BTreeMap::new();
        auth.insert("anthropic".to_string(), "api_key".to_string());
        auth.insert("openai".to_string(), "api_key".to_string());
        auth.insert("openai-chatgpt".to_string(), "copilot_device".to_string());
        auth.insert("github-copilot".to_string(), "copilot_device".to_string());
        auth.insert("gemini".to_string(), "api_key".to_string());
        auth.insert("deepseek".to_string(), "api_key".to_string());
        auth.insert("openrouter".to_string(), "api_key".to_string());
        auth.insert("zai".to_string(), "api_key".to_string());
        auth.insert("glm-coding".to_string(), "api_key".to_string());
        connect_rows_from(&auth, &BTreeMap::new())
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
mod detail_tests {
    use super::*;

    #[test]
    fn highlighted_provider_id_tracks_selection() {
        let mut auth = std::collections::BTreeMap::new();
        auth.insert("anthropic".to_string(), "api_key".to_string());
        auth.insert("openai".to_string(), "api_key".to_string());
        let st = ConnectPickerState::from_connectable(&auth, &std::collections::BTreeMap::new());
        // first selectable row highlighted by default
        assert!(st.highlighted_provider_id().is_some());
    }

    #[test]
    fn provider_detail_lines_shows_models_state_method() {
        let mut mp = std::collections::BTreeMap::new();
        mp.insert("claude-opus-4-8".to_string(), ("anthropic".to_string(), "Anthropic".to_string()));
        mp.insert("claude-sonnet-4-6".to_string(), ("anthropic".to_string(), "Anthropic".to_string()));
        mp.insert("gpt-5.5".to_string(), ("openai".to_string(), "OpenAI".to_string()));
        let lines = provider_detail_lines("anthropic", "Anthropic", true,
            &[ConnectMethod::Oauth, ConnectMethod::ApiKey], &mp);
        let joined = lines.join("\n");
        assert!(joined.contains("Anthropic"));
        assert!(joined.contains("connected"));         // connected state
        assert!(joined.contains("claude-opus-4-8"));    // its model
        assert!(!joined.contains("gpt-5.5"));           // NOT another provider's model
        assert!(joined.contains("Pro/Max") || joined.contains("API key")); // method(s)
        // unknown provider → graceful
        let empty = provider_detail_lines("nope", "Nope", false, &[ConnectMethod::ApiKey], &mp);
        assert!(empty.join("\n").to_lowercase().contains("no models"));
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    /// Minimal auth map covering both groups (Popular: anthropic, Providers: openrouter).
    fn two_provider_auth() -> BTreeMap<String, String> {
        [("anthropic", "api_key"), ("openrouter", "api_key")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn renders_groups_headers_and_footer() {
        let st = ConnectPickerState::from_connectable(&two_provider_auth(), &BTreeMap::new());
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
        let st = ConnectPickerState::from_connectable(&two_provider_auth(), &avail);
        let out = render_connect_picker_to_string(&st);
        // openrouter is not the highlighted row (Anthropic is), so its ✓ shows.
        assert!(out.contains("\u{2713} OpenRouter"), "{out}");
    }

    #[test]
    fn empty_query_shows_all_rows() {
        // Use explicit auth_methods; visible_lines must include every row (no
        // filter applied when query is empty).
        let auth: BTreeMap<String, String> = [
            ("anthropic", "api_key"),
            ("openai", "api_key"),
            ("openai-chatgpt", "copilot_device"),
            ("github-copilot", "copilot_device"),
            ("gemini", "api_key"),
            ("deepseek", "api_key"),
            ("openrouter", "api_key"),
            ("zai", "api_key"),
            ("glm-coding", "api_key"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let st = ConnectPickerState::from_connectable(&auth, &BTreeMap::new());
        let n = st.rows.len();
        assert_eq!(n, 9);
        let items = st
            .visible_lines()
            .into_iter()
            .filter(|l| matches!(l, VisibleLine::Item(_)))
            .count();
        assert_eq!(items, n);
    }
}
