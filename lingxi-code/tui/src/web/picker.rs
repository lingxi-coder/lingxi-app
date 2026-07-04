use tool_web::web_search_config::WebSearchProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfigSnapshot {
    pub active: WebSearchProvider,
    pub tavily_key: bool,
    pub brave_key: bool,
    pub searxng_url: Option<String>,
    pub last_test: Option<WebTestSummary>,
}

impl Default for WebConfigSnapshot {
    fn default() -> Self {
        Self {
            active: WebSearchProvider::Auto,
            tavily_key: false,
            brave_key: false,
            searxng_url: None,
            last_test: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTestSummary {
    pub provider: WebSearchProvider,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebProviderRow {
    pub id: WebSearchProvider,
    pub label: &'static str,
    pub description: &'static str,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebPickerState {
    pub rows: Vec<WebProviderRow>,
    pub query: String,
    pub selected: usize,
    pub snapshot: WebConfigSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebPickerOutcome {
    Stay,
    Select(WebSearchProvider),
    Test(WebSearchProvider),
    Cancel,
}

impl WebPickerState {
    #[must_use]
    pub fn from_snapshot(snapshot: WebConfigSnapshot) -> Self {
        let rows = provider_order()
            .into_iter()
            .map(|id| WebProviderRow {
                id,
                label: provider_label(id),
                description: provider_description(id),
                active: id == snapshot.active,
            })
            .collect();
        Self {
            rows,
            query: String::new(),
            selected: 0,
            snapshot,
        }
    }

    #[must_use]
    pub fn visible_indices(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let q = self.query.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.label.to_lowercase().contains(&q)
                    || r.description.to_lowercase().contains(&q)
                    || r.id.as_str().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    #[must_use]
    pub fn highlighted_provider(&self) -> Option<WebSearchProvider> {
        let idx = *self.visible_indices().get(self.selected)?;
        Some(self.rows.get(idx)?.id)
    }
}

#[must_use]
pub fn provider_order() -> [WebSearchProvider; 5] {
    [
        WebSearchProvider::Auto,
        WebSearchProvider::DuckDuckGo,
        WebSearchProvider::Tavily,
        WebSearchProvider::Brave,
        WebSearchProvider::Searxng,
    ]
}

#[must_use]
pub fn provider_label(provider: WebSearchProvider) -> &'static str {
    match provider {
        WebSearchProvider::Auto => "Auto",
        WebSearchProvider::DuckDuckGo => "DuckDuckGo",
        WebSearchProvider::Tavily => "Tavily",
        WebSearchProvider::Brave => "Brave",
        WebSearchProvider::Searxng => "SearXNG",
    }
}

#[must_use]
pub fn provider_description(provider: WebSearchProvider) -> &'static str {
    match provider {
        WebSearchProvider::Auto => "Use best configured backend, then fallback",
        WebSearchProvider::DuckDuckGo => "Keyless web search fallback",
        WebSearchProvider::Tavily => "Agent-focused API search",
        WebSearchProvider::Brave => "Brave Search API",
        WebSearchProvider::Searxng => "Self-hosted metasearch URL",
    }
}

#[must_use]
pub fn web_provider_detail_lines(
    provider: WebSearchProvider,
    snapshot: &WebConfigSnapshot,
) -> Vec<String> {
    let mut out = Vec::new();
    let active = if provider == snapshot.active {
        "active"
    } else {
        "inactive"
    };
    out.push(format!("{} — {active}", provider_label(provider)));
    out.push(match provider {
        WebSearchProvider::Auto => "Status: uses Tavily → Brave → SearXNG → DuckDuckGo".to_string(),
        WebSearchProvider::DuckDuckGo => "Status: keyless (always available)".to_string(),
        WebSearchProvider::Tavily => if snapshot.tavily_key {
            "Status: Configured"
        } else {
            "Status: Missing API key"
        }
        .to_string(),
        WebSearchProvider::Brave => if snapshot.brave_key {
            "Status: Configured"
        } else {
            "Status: Missing API key"
        }
        .to_string(),
        WebSearchProvider::Searxng => snapshot.searxng_url.as_ref().map_or_else(
            || "Status: Missing SearXNG URL".to_string(),
            |url| format!("Status: Configured ({url})"),
        ),
    });
    out.push(format!("Backend: {}", provider_description(provider)));
    if let Some(test) = &snapshot.last_test {
        if test.provider == provider {
            let mark = if test.ok { "✓" } else { "✗" };
            out.push(format!("Last test: {mark} {}", test.message));
        }
    }
    out.push("Enter configure/select · t test · Esc close".to_string());
    out
}

#[must_use]
pub fn handle_web_picker_key(
    state: &mut WebPickerState,
    key: crossterm::event::KeyCode,
) -> WebPickerOutcome {
    use crossterm::event::KeyCode;
    let count = state.visible_indices().len();
    match key {
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            WebPickerOutcome::Stay
        }
        KeyCode::Down => {
            if count > 0 {
                state.selected = (state.selected + 1).min(count - 1);
            }
            WebPickerOutcome::Stay
        }
        KeyCode::Char('t') => state
            .highlighted_provider()
            .map_or(WebPickerOutcome::Stay, WebPickerOutcome::Test),
        KeyCode::Char(c) => {
            state.query.push(c);
            state.selected = 0;
            WebPickerOutcome::Stay
        }
        KeyCode::Backspace => {
            state.query.pop();
            state.selected = 0;
            WebPickerOutcome::Stay
        }
        KeyCode::Enter => state
            .highlighted_provider()
            .map_or(WebPickerOutcome::Stay, WebPickerOutcome::Select),
        KeyCode::Esc => WebPickerOutcome::Cancel,
        _ => WebPickerOutcome::Stay,
    }
}

#[must_use]
pub fn render_web_picker_to_string(state: &WebPickerState) -> String {
    let mut out = String::from("Configure web search\n");
    out.push_str(&format!("Search: {}\n", state.query));
    let visible = state.visible_indices();
    if visible.is_empty() {
        out.push_str("No web providers match.");
        return out;
    }
    for (pos, idx) in visible.into_iter().enumerate() {
        let row = &state.rows[idx];
        let marker = if pos == state.selected {
            "❯ "
        } else if row.active {
            "✓ "
        } else {
            "  "
        };
        out.push_str(marker);
        out.push_str(&format!("{:<12}{}\n", row.label, row.description));
    }
    out.push_str("type to search · Enter configure · t test · Esc close");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_rows_show_all_v1_providers_and_active_marker() {
        let snapshot = WebConfigSnapshot {
            active: WebSearchProvider::Auto,
            tavily_key: true,
            brave_key: false,
            searxng_url: Some("https://s.example".into()),
            last_test: None,
        };
        let st = WebPickerState::from_snapshot(snapshot);
        assert_eq!(
            st.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![
                WebSearchProvider::Auto,
                WebSearchProvider::DuckDuckGo,
                WebSearchProvider::Tavily,
                WebSearchProvider::Brave,
                WebSearchProvider::Searxng,
            ]
        );
        assert!(st.rows[0].active);
    }

    #[test]
    fn detail_lines_show_configured_and_missing_status() {
        let snapshot = WebConfigSnapshot {
            active: WebSearchProvider::Auto,
            tavily_key: true,
            brave_key: false,
            searxng_url: None,
            last_test: None,
        };
        let lines = web_provider_detail_lines(WebSearchProvider::Brave, &snapshot);
        assert!(lines.join("\n").contains("Missing API key"));
        let tavily = web_provider_detail_lines(WebSearchProvider::Tavily, &snapshot);
        assert!(tavily.join("\n").contains("Configured"));
    }

    #[test]
    fn reducer_down_enter_test_and_esc() {
        let mut st = WebPickerState::from_snapshot(WebConfigSnapshot::default());
        assert_eq!(
            handle_web_picker_key(&mut st, crossterm::event::KeyCode::Down),
            WebPickerOutcome::Stay
        );
        assert_eq!(
            st.highlighted_provider(),
            Some(WebSearchProvider::DuckDuckGo)
        );
        assert_eq!(
            handle_web_picker_key(&mut st, crossterm::event::KeyCode::Char('t')),
            WebPickerOutcome::Test(WebSearchProvider::DuckDuckGo)
        );
        assert_eq!(
            handle_web_picker_key(&mut st, crossterm::event::KeyCode::Enter),
            WebPickerOutcome::Select(WebSearchProvider::DuckDuckGo)
        );
        assert_eq!(
            handle_web_picker_key(&mut st, crossterm::event::KeyCode::Esc),
            WebPickerOutcome::Cancel
        );
    }
}
