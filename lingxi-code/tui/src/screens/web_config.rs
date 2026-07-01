//! Pure `/web` provider configuration screen state/reducer.
//!
//! Owns text-entry and test-status display for one selected web search provider.
//! I/O (secure-store writes, settings writes, test network calls) is performed by
//! root pumps in later tasks; this module only emits outcomes.

use crate::screens::web_picker::WebConfigSnapshot;
use tool_web::web_search_config::WebSearchProvider;

/// Last/active test status shown in the provider config screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebTestStatus {
    /// No test has been requested in this screen state.
    Idle,
    /// A root pump is currently running the test search.
    Running,
    /// Test completed successfully.
    Success {
        provider: WebSearchProvider,
        count: usize,
        top_title: String,
        top_url: String,
    },
    /// Test/save validation failed with a user-facing message.
    Failed(String),
}

/// Pure state for the selected provider's config screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfigState {
    /// Provider being configured.
    pub provider: WebSearchProvider,
    /// Snapshot used to render current configured/missing status.
    pub snapshot: WebConfigSnapshot,
    /// Text buffer for API key or SearXNG URL.
    pub input: String,
    /// Test/validation status displayed under the input.
    pub test_status: WebTestStatus,
}

/// Outcome emitted by [`handle_web_config_key`] for the root router/pumps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebConfigOutcome {
    /// Keep screen open without side effects.
    Stay,
    /// Close back to the REPL.
    Close,
    /// Persist a secret key through the secure credential store.
    SaveSecret {
        provider: WebSearchProvider,
        secret: String,
    },
    /// Persist non-secret settings (`provider`, optional SearXNG URL).
    SaveSettings {
        provider: WebSearchProvider,
        searxng_url: Option<String>,
    },
    /// Run a test search for the selected provider.
    Test(WebSearchProvider),
}

impl WebConfigState {
    /// Build initial state for `provider` from the current `/web` snapshot.
    #[must_use]
    pub fn new(provider: WebSearchProvider, snapshot: WebConfigSnapshot) -> Self {
        let input = match provider {
            WebSearchProvider::Searxng => snapshot.searxng_url.clone().unwrap_or_default(),
            _ => String::new(),
        };
        Self {
            provider,
            snapshot,
            input,
            test_status: WebTestStatus::Idle,
        }
    }

    /// Current user-facing status line for render/tests.
    #[must_use]
    pub fn status_text(&self) -> String {
        match &self.test_status {
            WebTestStatus::Idle => status_for_provider(self.provider, &self.snapshot),
            WebTestStatus::Running => "Testing…".to_string(),
            WebTestStatus::Success {
                provider,
                count,
                top_title,
                top_url,
            } => format!(
                "{} test passed: {count} results · {top_title} ({top_url})",
                crate::screens::web_picker::provider_label(*provider)
            ),
            WebTestStatus::Failed(msg) => msg.clone(),
        }
    }
}

/// Human status line for `provider` in the current snapshot.
#[must_use]
pub fn status_for_provider(provider: WebSearchProvider, snapshot: &WebConfigSnapshot) -> String {
    match provider {
        WebSearchProvider::Auto => {
            "Auto fallback: Tavily → Brave → SearXNG → DuckDuckGo".to_string()
        }
        WebSearchProvider::DuckDuckGo => "DuckDuckGo is keyless and always available".to_string(),
        WebSearchProvider::Tavily => if snapshot.tavily_key {
            "Tavily API key configured"
        } else {
            "Paste Tavily API key"
        }
        .to_string(),
        WebSearchProvider::Brave => if snapshot.brave_key {
            "Brave API key configured"
        } else {
            "Paste Brave API key"
        }
        .to_string(),
        WebSearchProvider::Searxng => snapshot.searxng_url.as_ref().map_or_else(
            || "Enter SearXNG URL".to_string(),
            |url| format!("SearXNG URL: {url}"),
        ),
    }
}

/// Reduce one key press for the pure config screen.
#[must_use]
pub fn handle_web_config_key(
    state: &mut WebConfigState,
    key: crossterm::event::KeyCode,
) -> WebConfigOutcome {
    use crossterm::event::KeyCode;
    match key {
        KeyCode::Esc => WebConfigOutcome::Close,
        KeyCode::Backspace => {
            state.input.pop();
            WebConfigOutcome::Stay
        }
        KeyCode::Char(c) => {
            if requires_input(state.provider) {
                state.input.push(c);
                WebConfigOutcome::Stay
            } else if c == 't' {
                WebConfigOutcome::Test(state.provider)
            } else {
                WebConfigOutcome::Stay
            }
        }
        KeyCode::Enter => save_outcome(state),
        _ => WebConfigOutcome::Stay,
    }
}

fn requires_input(provider: WebSearchProvider) -> bool {
    matches!(
        provider,
        WebSearchProvider::Tavily | WebSearchProvider::Brave | WebSearchProvider::Searxng
    )
}

fn save_outcome(state: &mut WebConfigState) -> WebConfigOutcome {
    match state.provider {
        WebSearchProvider::Tavily | WebSearchProvider::Brave => {
            let secret = state.input.trim().to_string();
            if secret.is_empty() {
                state.test_status = WebTestStatus::Failed("API key cannot be empty".to_string());
                WebConfigOutcome::Stay
            } else {
                WebConfigOutcome::SaveSecret {
                    provider: state.provider,
                    secret,
                }
            }
        }
        WebSearchProvider::Searxng => {
            let url = state.input.trim().trim_end_matches('/').to_string();
            if !is_http_url(&url) {
                state.test_status = WebTestStatus::Failed("Invalid URL".to_string());
                WebConfigOutcome::Stay
            } else {
                WebConfigOutcome::SaveSettings {
                    provider: state.provider,
                    searxng_url: Some(url),
                }
            }
        }
        WebSearchProvider::Auto | WebSearchProvider::DuckDuckGo => WebConfigOutcome::SaveSettings {
            provider: state.provider,
            searxng_url: state.snapshot.searxng_url.clone(),
        },
    }
}

fn is_http_url(s: &str) -> bool {
    url::Url::parse(s)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// Render a plain-text oracle for snapshot/unit tests.
#[must_use]
pub fn render_web_config_to_string(state: &WebConfigState) -> String {
    let label = crate::screens::web_picker::provider_label(state.provider);
    let mut out = format!("Configure {label}\n{}\n", state.status_text());
    if requires_input(state.provider) {
        let shown = if matches!(
            state.provider,
            WebSearchProvider::Tavily | WebSearchProvider::Brave
        ) {
            "*".repeat(state.input.chars().count())
        } else {
            state.input.clone()
        };
        out.push_str(&format!("Input: {shown}\n"));
    }
    if requires_input(state.provider) {
        out.push_str("Enter save · Esc cancel");
    } else {
        out.push_str("Enter save · t test · Esc cancel");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn key_char(c: char) -> KeyCode {
        KeyCode::Char(c)
    }
    fn key_enter() -> KeyCode {
        KeyCode::Enter
    }

    #[test]
    fn tavily_key_buffer_edits_and_save_returns_secret_action() {
        let mut st = WebConfigState::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
        for ch in "tvly-secret".chars() {
            assert_eq!(
                handle_web_config_key(&mut st, key_char(ch)),
                WebConfigOutcome::Stay
            );
        }
        assert_eq!(
            handle_web_config_key(&mut st, key_enter()),
            WebConfigOutcome::SaveSecret {
                provider: WebSearchProvider::Tavily,
                secret: "tvly-secret".into()
            }
        );
    }

    #[test]
    fn tavily_t_is_text_not_test_shortcut() {
        let mut st = WebConfigState::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
        assert_eq!(
            handle_web_config_key(&mut st, key_char('t')),
            WebConfigOutcome::Stay
        );
        assert_eq!(st.input, "t");
    }

    #[test]
    fn keyless_provider_keeps_t_test_shortcut() {
        let mut st =
            WebConfigState::new(WebSearchProvider::DuckDuckGo, WebConfigSnapshot::default());
        assert_eq!(
            handle_web_config_key(&mut st, key_char('t')),
            WebConfigOutcome::Test(WebSearchProvider::DuckDuckGo)
        );
    }

    #[test]
    fn searxng_url_validates_before_save() {
        let mut st = WebConfigState::new(WebSearchProvider::Searxng, WebConfigSnapshot::default());
        for ch in "not a url".chars() {
            let _ = handle_web_config_key(&mut st, key_char(ch));
        }
        assert_eq!(
            handle_web_config_key(&mut st, key_enter()),
            WebConfigOutcome::Stay
        );
        assert!(st.status_text().contains("Invalid URL"));
    }
}
