//! `/connect` LOGIN-METHOD choice step (shown for providers that offer more
//! than one method, e.g. Anthropic = Pro/Max OAuth + API key). Modeled on
//! `github_deploy.rs`: a pure Up/Down reducer + an outcome enum; the chosen
//! method opens the matching `ConnectFlow` in `root::handle_screen_key`.

use crate::screens::connect_picker::ConnectMethod;
use crossterm::event::KeyCode;

/// Method-choice screen state. `options` comes from `provider_methods()` and is
/// only opened (len >= 2) for multi-method providers (Anthropic today).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectMethodState {
    /// Engine provider id (e.g. `"anthropic"`).
    pub provider_id: String,
    /// Human label (e.g. `"Anthropic"`).
    pub label: String,
    /// The offered methods, in display order (`provider_methods()`).
    pub options: Vec<ConnectMethod>,
    /// Highlighted index into `options`.
    pub selected: usize,
}

impl ConnectMethodState {
    #[must_use]
    pub fn new(provider_id: String, label: String, options: Vec<ConnectMethod>) -> Self {
        Self {
            provider_id,
            label,
            options,
            selected: 0,
        }
    }
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodChoiceOutcome {
    /// Stay open (highlight moved / inert key).
    Stay,
    /// Enter on the highlighted option — open that method's flow.
    Pick {
        /// The chosen method.
        method: ConnectMethod,
    },
    /// Esc — cancel the whole `/connect` flow.
    Cancel,
}

/// Reduce a key. Up/Down move (clamped); Enter picks the highlighted method;
/// Esc cancels. Mirrors `github_deploy::handle_github_deploy_key`.
#[must_use]
pub fn handle_connect_method_key(st: &mut ConnectMethodState, key: KeyCode) -> MethodChoiceOutcome {
    let n = st.options.len();
    match key {
        KeyCode::Up => {
            st.selected = st.selected.saturating_sub(1);
            MethodChoiceOutcome::Stay
        }
        KeyCode::Down => {
            if n > 0 {
                st.selected = (st.selected + 1).min(n - 1);
            }
            MethodChoiceOutcome::Stay
        }
        KeyCode::Enter => match st.options.get(st.selected) {
            Some(&method) => MethodChoiceOutcome::Pick { method },
            None => MethodChoiceOutcome::Stay,
        },
        KeyCode::Esc => MethodChoiceOutcome::Cancel,
        _ => MethodChoiceOutcome::Stay,
    }
}

/// Render the body (plain text; the app.rs arm wraps it in the shared popup).
/// Kept for a snapshot test — the live render uses `picker_popup`.
#[must_use]
pub fn render_connect_method_to_string(st: &ConnectMethodState) -> String {
    let mut out = format!("Connect {}\n\n", st.label);
    for (i, opt) in st.options.iter().enumerate() {
        let mark = if i == st.selected { "\u{276F} " } else { "  " };
        out.push_str(mark);
        out.push_str(opt.choice_label());
        out.push('\n');
    }
    out.push_str("\n\u{2191}\u{2193} select \u{00B7} Enter \u{00B7} Esc to cancel");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anthropic() -> ConnectMethodState {
        ConnectMethodState::new(
            "anthropic".to_string(),
            "Anthropic".to_string(),
            vec![ConnectMethod::Oauth, ConnectMethod::ApiKey],
        )
    }

    #[test]
    fn nav_clamps_at_both_ends() {
        let mut st = anthropic();
        assert_eq!(st.selected, 0);
        // Up at top stays.
        assert_eq!(
            handle_connect_method_key(&mut st, KeyCode::Up),
            MethodChoiceOutcome::Stay
        );
        assert_eq!(st.selected, 0);
        // Down moves to 1.
        let _ = handle_connect_method_key(&mut st, KeyCode::Down);
        assert_eq!(st.selected, 1);
        // Down at bottom clamps.
        let _ = handle_connect_method_key(&mut st, KeyCode::Down);
        assert_eq!(st.selected, 1);
    }

    #[test]
    fn enter_picks_oauth_at_idx0_and_api_key_at_idx1() {
        let mut st = anthropic();
        assert_eq!(
            handle_connect_method_key(&mut st, KeyCode::Enter),
            MethodChoiceOutcome::Pick {
                method: ConnectMethod::Oauth
            }
        );
        let _ = handle_connect_method_key(&mut st, KeyCode::Down);
        assert_eq!(
            handle_connect_method_key(&mut st, KeyCode::Enter),
            MethodChoiceOutcome::Pick {
                method: ConnectMethod::ApiKey
            }
        );
    }

    #[test]
    fn esc_cancels() {
        let mut st = anthropic();
        assert_eq!(
            handle_connect_method_key(&mut st, KeyCode::Esc),
            MethodChoiceOutcome::Cancel
        );
    }

    #[test]
    fn render_shows_title_and_both_labels() {
        let out = render_connect_method_to_string(&anthropic());
        assert!(out.starts_with("Connect Anthropic"), "{out}");
        assert!(out.contains("Sign in with Claude Pro/Max"), "{out}");
        assert!(out.contains("Use an API key"), "{out}");
        // Highlighted (idx 0) row carries the ❯ marker.
        assert!(
            out.contains("\u{276F} Sign in with Claude Pro/Max"),
            "{out}"
        );
    }
}
