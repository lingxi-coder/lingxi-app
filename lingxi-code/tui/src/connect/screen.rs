//! `/connect <provider>` interactive credential setup (Plan 3c §6.3/§6.4).
//!
//! Two flows, one pure reducer: a masked API-key field (Enter submits), and the
//! Copilot OAuth device-flow (the host drives `CopilotLogin`; this screen renders
//! the code + spinner; typing is inert). Esc cancels either flow.

use crossterm::event::KeyCode;

/// Which credential flow this screen is driving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFlow {
    /// API-key providers: a masked key-input field for `provider_id`.
    ApiKey {
        /// Provider grouping key the key is stored under.
        provider_id: String,
        /// Human provider label for the header.
        label: String,
    },
    /// GitHub Copilot OAuth device-flow.
    Copilot,
    /// (T2b) First-party OAuth browser sign-in (Anthropic Pro/Max, OpenAI
    /// ChatGPT). The host drives the blocking `OAuthConnectDriver::login`; this
    /// screen shows progress via the shared [`CopilotPhase`] field (Starting →
    /// Done/Failed; no device-code Polling step). Typing is inert; terminal phases
    /// return on any key.
    OAuth {
        /// Provider grouping key being signed into.
        provider_id: String,
        /// Human provider label for the header.
        label: String,
    },
    /// (T2a) A login method whose flow isn't wired in this build. Honest terminal
    /// screen — never a dead key field. Any key returns to the REPL.
    Unavailable { label: String, reason: String },
}

/// Terminal/in-progress state of a Copilot device-flow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CopilotPhase {
    /// Requesting the device code.
    #[default]
    Starting,
    /// Code obtained; polling for authorization.
    Polling {
        /// Code the user types at GitHub.
        user_code: String,
        /// URL the user opens.
        verification_uri: String,
    },
    /// Authorization completed.
    Done,
    /// Device-flow failed.
    Failed {
        /// Server-reported error code.
        error: String,
    },
}

/// `/connect` screen state. Pure; the caller drives async work via [`ConnectAction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectScreenState {
    /// Which flow is active.
    pub flow: ConnectFlow,
    /// API-key entry buffer (rendered masked). Unused in the Copilot flow.
    pub key_buffer: String,
    /// Copilot device-flow phase. Unused in the API-key flow.
    pub copilot: CopilotPhase,
}

/// What the caller should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectAction {
    /// Stay open (field edited / inert key).
    None,
    /// Enter on a non-empty key field — store the key for `provider_id`.
    SubmitKey {
        /// Provider/keychain id to store under.
        provider_id: String,
        /// The entered secret.
        key: String,
    },
    /// Esc — cancel the flow, store nothing, close the screen.
    Cancel,
}

impl ConnectScreenState {
    /// Open an API-key field for `provider_id` (header uses `label`).
    #[must_use]
    pub fn api_key(provider_id: &str, label: &str) -> Self {
        Self {
            flow: ConnectFlow::ApiKey {
                provider_id: provider_id.to_string(),
                label: label.to_string(),
            },
            key_buffer: String::new(),
            copilot: CopilotPhase::Starting,
        }
    }

    /// Open an Unavailable screen for a login method not wired in this build.
    #[must_use]
    pub fn unavailable(label: &str, reason: &str) -> Self {
        Self {
            flow: ConnectFlow::Unavailable {
                label: label.to_string(),
                reason: reason.to_string(),
            },
            key_buffer: String::new(),
            copilot: CopilotPhase::Starting,
        }
    }

    /// Open a first-party OAuth browser sign-in screen for `provider_id`
    /// (header uses `label`). Starts in the `Starting` phase ("opening browser");
    /// the host's oauth-login task advances it to `Done`/`Failed`.
    #[must_use]
    pub fn oauth(provider_id: &str, label: &str) -> Self {
        Self {
            flow: ConnectFlow::OAuth {
                provider_id: provider_id.to_string(),
                label: label.to_string(),
            },
            key_buffer: String::new(),
            copilot: CopilotPhase::Starting,
        }
    }

    /// Open the Copilot device-flow in the `Starting` phase.
    #[must_use]
    pub fn copilot_pending() -> Self {
        Self {
            flow: ConnectFlow::Copilot,
            key_buffer: String::new(),
            copilot: CopilotPhase::Starting,
        }
    }

    /// Host setter: device code obtained → display + spinner.
    pub fn set_device_code(&mut self, user_code: &str, verification_uri: &str) {
        self.copilot = CopilotPhase::Polling {
            user_code: user_code.to_string(),
            verification_uri: verification_uri.to_string(),
        };
    }

    /// Host setter: authorization completed.
    pub fn set_done(&mut self) {
        self.copilot = CopilotPhase::Done;
    }

    /// Host setter: device-flow failed.
    pub fn set_failed(&mut self, error: &str) {
        self.copilot = CopilotPhase::Failed {
            error: error.to_string(),
        };
    }
}

/// Route one key into the `/connect` screen. API-key flow: chars edit the masked
/// buffer, Backspace deletes, Enter submits a non-empty key, Esc cancels. Copilot
/// flow: typing/Enter inert (the host drives the poll); Esc cancels.
#[must_use]
pub fn handle_connect_key(st: &mut ConnectScreenState, key: KeyCode) -> ConnectAction {
    if key == KeyCode::Esc {
        return ConnectAction::Cancel;
    }
    if matches!(st.flow, ConnectFlow::Unavailable { .. }) {
        return ConnectAction::Cancel;
    }
    // Copilot TERMINAL phases (Done/Failed): the flow is over, so ANY key returns
    // to the REPL (not just Esc). Without this the screen sat on "Authorized ✓"
    // and the only labelled action was "Esc to cancel" — which read like it would
    // UNDO the successful login.
    if matches!(st.flow, ConnectFlow::Copilot | ConnectFlow::OAuth { .. })
        && matches!(st.copilot, CopilotPhase::Done | CopilotPhase::Failed { .. })
    {
        return ConnectAction::Cancel;
    }
    match &st.flow {
        ConnectFlow::ApiKey { provider_id, .. } => match key {
            KeyCode::Char(c) => {
                st.key_buffer.push(c);
                ConnectAction::None
            }
            KeyCode::Backspace => {
                st.key_buffer.pop();
                ConnectAction::None
            }
            KeyCode::Enter => {
                if st.key_buffer.is_empty() {
                    ConnectAction::None
                } else {
                    ConnectAction::SubmitKey {
                        provider_id: provider_id.clone(),
                        key: st.key_buffer.clone(),
                    }
                }
            }
            _ => ConnectAction::None,
        },
        ConnectFlow::Copilot => ConnectAction::None,
        // OAuth in-progress (Starting): typing is inert; the host drives the
        // browser flow. Terminal phases already returned above; Esc at the top.
        ConnectFlow::OAuth { .. } => ConnectAction::None,
        ConnectFlow::Unavailable { .. } => ConnectAction::Cancel,
    }
}

/// Insert bracketed-paste `text` into the API-key field. Control characters
/// (a trailing newline from a copied key, tabs) are stripped so the stored
/// secret stays clean; the terminal delivers a paste as one event, so without
/// this the masked field ignores ⌘V entirely. Inert in the Copilot / OAuth /
/// Unavailable flows (no editable buffer). Never submits — Enter still does.
pub fn handle_connect_paste(st: &mut ConnectScreenState, text: &str) {
    if matches!(st.flow, ConnectFlow::ApiKey { .. }) {
        st.key_buffer
            .extend(text.chars().filter(|c| !c.is_control()));
    }
}

/// Render the `/connect` body (plain text; the iocraft layer wraps it).
#[must_use]
pub fn render_connect_to_string(st: &ConnectScreenState) -> String {
    let mut out = String::new();
    match &st.flow {
        ConnectFlow::ApiKey { label, .. } => {
            out.push_str(&format!("Connect {label}\n"));
            let mask: String = "\u{2022}".repeat(st.key_buffer.chars().count());
            out.push_str(&format!("Key: {mask}\n"));
            out.push_str("Paste your API key \u{00B7} Enter to save \u{00B7} Esc to cancel");
        }
        ConnectFlow::OAuth { label, .. } => {
            out.push_str(&format!("Connect {label}\n"));
            match &st.copilot {
                CopilotPhase::Starting | CopilotPhase::Polling { .. } => {
                    out.push_str("Opening your browser to sign in\u{2026}\n");
                    out.push_str("Complete the sign-in in your browser \u{00B7} Esc to cancel");
                }
                CopilotPhase::Done => {
                    out.push_str("Signed in \u{2713}\n");
                    out.push_str("Connected \u{2014} returning to the prompt\u{2026}");
                }
                CopilotPhase::Failed { error } => {
                    out.push_str(&format!("Sign-in failed: {error}\n"));
                    out.push_str(
                        "Connect with an API key instead \u{00B7} press any key to return",
                    );
                }
            }
        }
        ConnectFlow::Unavailable { label, reason } => {
            out.push_str(&format!("Connect {label}\n"));
            out.push_str(&format!("{reason}.\n"));
            out.push_str("Connect with an API key instead \u{00B7} press any key to go back");
        }
        ConnectFlow::Copilot => {
            out.push_str("Connect GitHub Copilot\n");
            match &st.copilot {
                CopilotPhase::Starting => out.push_str("Requesting device code\u{2026}\n"),
                CopilotPhase::Polling {
                    user_code,
                    verification_uri,
                } => {
                    // The host has already opened the browser + copied the code to
                    // the clipboard (the screen text is not mouse-selectable — the
                    // TUI captures the mouse — so we copy it FOR the user).
                    out.push_str(&format!(
                        "Enter code: {user_code}   (copied to clipboard)\n"
                    ));
                    out.push_str(&format!("at {verification_uri}\n"));
                    out.push_str(
                        "A browser was opened \u{2014} paste the code (\u{2318}V) to authorize.\n",
                    );
                    out.push_str("Waiting for authorization\u{2026}\n");
                }
                CopilotPhase::Done => out.push_str("Authorized \u{2713}\n"),
                CopilotPhase::Failed { error } => {
                    out.push_str(&format!("Authorization failed: {error}\n"))
                }
            }
            // Footer: terminal phases return to the REPL on ANY key; otherwise Esc
            // cancels the in-flight flow.
            match &st.copilot {
                CopilotPhase::Done => {
                    out.push_str("Connected \u{2014} returning to the prompt\u{2026}")
                }
                CopilotPhase::Failed { .. } => out.push_str("Press any key to return"),
                _ => out.push_str("Esc to cancel"),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn api_key_field_masks_and_submits() {
        let mut st = ConnectScreenState::api_key("deepseek", "DeepSeek");
        for c in "sk-secret".chars() {
            assert_eq!(
                handle_connect_key(&mut st, KeyCode::Char(c)),
                ConnectAction::None
            );
        }
        let out = render_connect_to_string(&st);
        assert!(out.contains("Connect DeepSeek"));
        assert!(out.contains(
            "Key: \u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"
        ));
        assert!(!out.contains("sk-secret"), "raw key must never render");
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Enter),
            ConnectAction::SubmitKey {
                provider_id: "deepseek".to_string(),
                key: "sk-secret".to_string()
            }
        );
    }

    #[test]
    fn paste_appends_into_the_key_field_stripping_control_chars() {
        let mut st = ConnectScreenState::api_key("openrouter", "OpenRouter");
        let _ = handle_connect_key(&mut st, KeyCode::Char('a'));
        // A copied key often carries a trailing newline; it must not enter the
        // buffer, and the paste must land after already-typed input.
        handle_connect_paste(&mut st, "sk-or-v1-xyz\n");
        assert_eq!(st.key_buffer, "ask-or-v1-xyz", "paste appends, newline stripped");
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Enter),
            ConnectAction::SubmitKey {
                provider_id: "openrouter".to_string(),
                key: "ask-or-v1-xyz".to_string(),
            }
        );
    }

    #[test]
    fn paste_is_inert_in_the_copilot_flow() {
        let mut st = ConnectScreenState::copilot_pending();
        handle_connect_paste(&mut st, "should-be-ignored");
        assert!(st.key_buffer.is_empty(), "no editable field in the Copilot flow");
    }

    #[test]
    fn api_key_backspace_and_empty_enter_inert() {
        let mut st = ConnectScreenState::api_key("openrouter", "OpenRouter");
        let _ = handle_connect_key(&mut st, KeyCode::Char('a'));
        let _ = handle_connect_key(&mut st, KeyCode::Backspace);
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Enter),
            ConnectAction::None
        );
    }

    #[test]
    fn esc_cancels() {
        let mut st = ConnectScreenState::api_key("deepseek", "DeepSeek");
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Esc),
            ConnectAction::Cancel
        );
    }

    #[test]
    fn copilot_renders_device_code_and_spinner() {
        let mut st = ConnectScreenState::copilot_pending();
        st.set_device_code("WDJB-MJHT", "https://github.com/login/device");
        let out = render_connect_to_string(&st);
        assert!(out.contains("Connect GitHub Copilot"));
        assert!(out.contains("Enter code: WDJB-MJHT"));
        assert!(out.contains("at https://github.com/login/device"));
        assert!(out.contains("Waiting for authorization"));
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Char('x')),
            ConnectAction::None
        );
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Enter),
            ConnectAction::None
        );
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Esc),
            ConnectAction::Cancel
        );
    }

    #[test]
    fn copilot_failure_renders_error() {
        let mut st = ConnectScreenState::copilot_pending();
        st.set_failed("access_denied");
        let out = render_connect_to_string(&st);
        assert!(out.contains("Authorization failed: access_denied"));
    }

    #[test]
    fn unavailable_renders_honest_message_and_any_key_closes() {
        let mut st = ConnectScreenState::unavailable(
            "OpenAI (ChatGPT)",
            "browser sign-in isn't available in this build yet",
        );
        let body = render_connect_to_string(&st);
        assert!(body.contains("OpenAI (ChatGPT)"));
        assert!(body.contains("isn't available in this build yet"));
        assert!(body.contains("API key")); // tells them what DOES work
                                           // Any key (and Esc) closes — never a dead field.
        assert_eq!(
            handle_connect_key(&mut st, crossterm::event::KeyCode::Enter),
            ConnectAction::Cancel
        );
        assert_eq!(
            handle_connect_key(&mut st, crossterm::event::KeyCode::Esc),
            ConnectAction::Cancel
        );
    }

    #[test]
    fn oauth_flow_renders_progress_and_terminal_phases_close_on_any_key() {
        // In-progress (Starting): "Opening your browser…"; typing inert, Esc cancels.
        let mut st = ConnectScreenState::oauth("anthropic", "Anthropic");
        let starting = render_connect_to_string(&st);
        assert!(starting.contains("Connect Anthropic"));
        assert!(starting.contains("Opening your browser to sign in"));
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Char('x')),
            ConnectAction::None
        );
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Esc),
            ConnectAction::Cancel
        );

        // Success: "Signed in ✓"; ANY key returns to the REPL (not just Esc).
        let mut done = ConnectScreenState::oauth("openai-chatgpt", "OpenAI (ChatGPT)");
        done.set_done();
        let out = render_connect_to_string(&done);
        assert!(out.contains("Signed in \u{2713}"));
        assert!(out.contains("returning to the prompt"), "{out}");
        assert_eq!(
            handle_connect_key(&mut done, KeyCode::Enter),
            ConnectAction::Cancel
        );
        assert_eq!(
            handle_connect_key(&mut done, KeyCode::Char('q')),
            ConnectAction::Cancel
        );

        // Failure: shows the error + offers the API-key alternative; any key returns.
        let mut failed = ConnectScreenState::oauth("anthropic", "Anthropic");
        failed.set_failed("user cancelled login");
        let f = render_connect_to_string(&failed);
        assert!(f.contains("Sign-in failed: user cancelled login"));
        assert!(f.contains("API key"));
        assert_eq!(
            handle_connect_key(&mut failed, KeyCode::Enter),
            ConnectAction::Cancel
        );
    }

    #[test]
    fn copilot_terminal_phases_close_on_any_key() {
        // Done: any key (not just Esc) returns to the REPL.
        let mut done = ConnectScreenState::copilot_pending();
        done.set_done();
        assert_eq!(
            handle_connect_key(&mut done, KeyCode::Enter),
            ConnectAction::Cancel
        );
        assert_eq!(
            handle_connect_key(&mut done, KeyCode::Char('x')),
            ConnectAction::Cancel
        );
        let out = render_connect_to_string(&done);
        assert!(out.contains("Authorized \u{2713}"));
        assert!(out.contains("returning to the prompt"), "{out}");

        // Failed: likewise closes on any key, with a "press any key" footer.
        let mut failed = ConnectScreenState::copilot_pending();
        failed.set_failed("access_denied");
        assert_eq!(
            handle_connect_key(&mut failed, KeyCode::Enter),
            ConnectAction::Cancel
        );
        assert!(render_connect_to_string(&failed).contains("Press any key to return"));

        // Polling (non-terminal): typing stays inert; only Esc cancels.
        let mut polling = ConnectScreenState::copilot_pending();
        polling.set_device_code("WDJB-MJHT", "https://github.com/login/device");
        assert_eq!(
            handle_connect_key(&mut polling, KeyCode::Char('x')),
            ConnectAction::None
        );
        assert!(render_connect_to_string(&polling).contains("copied to clipboard"));
    }
}
