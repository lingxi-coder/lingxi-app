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
            flow: ConnectFlow::ApiKey { provider_id: provider_id.to_string(), label: label.to_string() },
            key_buffer: String::new(),
            copilot: CopilotPhase::Starting,
        }
    }

    /// Open the Copilot device-flow in the `Starting` phase.
    #[must_use]
    pub fn copilot_pending() -> Self {
        Self { flow: ConnectFlow::Copilot, key_buffer: String::new(), copilot: CopilotPhase::Starting }
    }

    /// Host setter: device code obtained → display + spinner.
    pub fn set_device_code(&mut self, user_code: &str, verification_uri: &str) {
        self.copilot = CopilotPhase::Polling { user_code: user_code.to_string(), verification_uri: verification_uri.to_string() };
    }

    /// Host setter: authorization completed.
    pub fn set_done(&mut self) {
        self.copilot = CopilotPhase::Done;
    }

    /// Host setter: device-flow failed.
    pub fn set_failed(&mut self, error: &str) {
        self.copilot = CopilotPhase::Failed { error: error.to_string() };
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
                    ConnectAction::SubmitKey { provider_id: provider_id.clone(), key: st.key_buffer.clone() }
                }
            }
            _ => ConnectAction::None,
        },
        ConnectFlow::Copilot => ConnectAction::None,
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
        ConnectFlow::Copilot => {
            out.push_str("Connect GitHub Copilot\n");
            match &st.copilot {
                CopilotPhase::Starting => out.push_str("Requesting device code\u{2026}\n"),
                CopilotPhase::Polling { user_code, verification_uri } => {
                    out.push_str(&format!("Enter code: {user_code}\n"));
                    out.push_str(&format!("at {verification_uri}\n"));
                    out.push_str("Waiting for authorization\u{2026}\n");
                }
                CopilotPhase::Done => out.push_str("Authorized \u{2713}\n"),
                CopilotPhase::Failed { error } => out.push_str(&format!("Authorization failed: {error}\n")),
            }
            out.push_str("Esc to cancel");
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
            assert_eq!(handle_connect_key(&mut st, KeyCode::Char(c)), ConnectAction::None);
        }
        let out = render_connect_to_string(&st);
        assert!(out.contains("Connect DeepSeek"));
        assert!(out.contains("Key: \u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"));
        assert!(!out.contains("sk-secret"), "raw key must never render");
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Enter),
            ConnectAction::SubmitKey { provider_id: "deepseek".to_string(), key: "sk-secret".to_string() }
        );
    }

    #[test]
    fn api_key_backspace_and_empty_enter_inert() {
        let mut st = ConnectScreenState::api_key("openrouter", "OpenRouter");
        let _ = handle_connect_key(&mut st, KeyCode::Char('a'));
        let _ = handle_connect_key(&mut st, KeyCode::Backspace);
        assert_eq!(handle_connect_key(&mut st, KeyCode::Enter), ConnectAction::None);
    }

    #[test]
    fn esc_cancels() {
        let mut st = ConnectScreenState::api_key("deepseek", "DeepSeek");
        assert_eq!(handle_connect_key(&mut st, KeyCode::Esc), ConnectAction::Cancel);
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
        assert_eq!(handle_connect_key(&mut st, KeyCode::Char('x')), ConnectAction::None);
        assert_eq!(handle_connect_key(&mut st, KeyCode::Enter), ConnectAction::None);
        assert_eq!(handle_connect_key(&mut st, KeyCode::Esc), ConnectAction::Cancel);
    }

    #[test]
    fn copilot_failure_renders_error() {
        let mut st = ConnectScreenState::copilot_pending();
        st.set_failed("access_denied");
        let out = render_connect_to_string(&st);
        assert!(out.contains("Authorization failed: access_denied"));
    }
}
