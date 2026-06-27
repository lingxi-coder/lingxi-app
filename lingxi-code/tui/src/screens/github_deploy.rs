//! GitHub Copilot DEPLOYMENT-TYPE sub-flow (`/connect` → GitHub Copilot),
//! modeled on opencode's `authorize` prompt: first pick **GitHub.com Public** or
//! **GitHub Enterprise**; for Enterprise, enter the host (e.g. `company.ghe.com`).
//! The resolved domain parameterizes the device-flow URLs
//! (`https://<domain>/login/device/code` + `/login/oauth/access_token`).
//!
//! Pure reducer + a `-> String` render oracle (the app.rs arm renders it inside
//! the shared popup). Two phases: [`DeployPhase::Choose`] and
//! [`DeployPhase::Host`].

use crossterm::event::KeyCode;

/// Which step of the deployment-type flow is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployPhase {
    /// Pick Public (0) or Enterprise (1).
    Choose {
        /// Highlighted option: 0 = Public, 1 = Enterprise.
        selected: usize,
    },
    /// Enter the GitHub Enterprise host.
    Host {
        /// The typed host buffer (e.g. `company.ghe.com`).
        buffer: String,
    },
}

impl Default for DeployPhase {
    fn default() -> Self {
        Self::Choose { selected: 0 }
    }
}

/// Deployment-type screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GithubDeploymentState {
    /// Current phase.
    pub phase: DeployPhase,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployOutcome {
    /// Stay open (highlight moved / typing / inert key).
    Stay,
    /// Proceed with the PUBLIC `github.com` device flow.
    Public,
    /// Proceed with a GitHub ENTERPRISE device flow against `domain`.
    Enterprise {
        /// Normalized host (scheme + trailing slash stripped).
        domain: String,
    },
    /// Esc from the Choose phase — cancel the whole flow.
    Cancel,
}

/// Normalize a user-entered URL/host to a bare domain (opencode `normalizeDomain`):
/// strip the scheme and any trailing slash. Returns an empty string for blank input.
#[must_use]
pub fn normalize_domain(input: &str) -> String {
    let s = input.trim();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    s.trim_end_matches('/').trim().to_string()
}

/// Reduce a key. Choose: Up/Down move, Enter picks (Public → `Public`,
/// Enterprise → switch to the Host phase), Esc cancels. Host: chars/Backspace
/// edit the host, Enter yields `Enterprise{domain}` (inert if blank), Esc returns
/// to Choose.
#[must_use]
pub fn handle_github_deploy_key(state: &mut GithubDeploymentState, key: KeyCode) -> DeployOutcome {
    match &mut state.phase {
        DeployPhase::Choose { selected } => match key {
            KeyCode::Up => {
                *selected = 0;
                DeployOutcome::Stay
            }
            KeyCode::Down => {
                *selected = 1;
                DeployOutcome::Stay
            }
            KeyCode::Enter => {
                if *selected == 0 {
                    DeployOutcome::Public
                } else {
                    state.phase = DeployPhase::Host { buffer: String::new() };
                    DeployOutcome::Stay
                }
            }
            KeyCode::Esc => DeployOutcome::Cancel,
            _ => DeployOutcome::Stay,
        },
        DeployPhase::Host { buffer } => match key {
            KeyCode::Char(c) => {
                buffer.push(c);
                DeployOutcome::Stay
            }
            KeyCode::Backspace => {
                buffer.pop();
                DeployOutcome::Stay
            }
            KeyCode::Enter => {
                let domain = normalize_domain(buffer);
                if domain.is_empty() {
                    DeployOutcome::Stay
                } else {
                    DeployOutcome::Enterprise { domain }
                }
            }
            // Esc steps BACK to the choice (not a full cancel).
            KeyCode::Esc => {
                state.phase = DeployPhase::Choose { selected: 1 };
                DeployOutcome::Stay
            }
            _ => DeployOutcome::Stay,
        },
    }
}

/// Render the body (plain text; the app.rs arm wraps it in the shared popup).
/// Choose phase: a two-row menu; Host phase: a labelled input line.
#[must_use]
pub fn render_github_deploy_to_string(state: &GithubDeploymentState) -> String {
    match &state.phase {
        DeployPhase::Choose { selected } => {
            let mark = |i: usize| if *selected == i { "\u{276F} " } else { "  " };
            format!(
                "Select GitHub deployment type\n\n{}GitHub.com Public\n{}GitHub Enterprise  (Data residency or self-hosted)\n\ntype \u{2191}\u{2193}, Enter to select \u{00B7} Esc to cancel",
                mark(0),
                mark(1),
            )
        }
        DeployPhase::Host { buffer } => format!(
            "GitHub Enterprise host\n\nHost: {buffer}\n\ne.g. company.ghe.com \u{00B7} Enter to continue \u{00B7} Esc to go back",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_nav_and_public() {
        let mut st = GithubDeploymentState::default();
        assert_eq!(st.phase, DeployPhase::Choose { selected: 0 });
        assert_eq!(handle_github_deploy_key(&mut st, KeyCode::Down), DeployOutcome::Stay);
        assert_eq!(st.phase, DeployPhase::Choose { selected: 1 });
        let _ = handle_github_deploy_key(&mut st, KeyCode::Up);
        assert_eq!(handle_github_deploy_key(&mut st, KeyCode::Enter), DeployOutcome::Public);
    }

    #[test]
    fn enterprise_collects_host() {
        let mut st = GithubDeploymentState::default();
        let _ = handle_github_deploy_key(&mut st, KeyCode::Down); // select Enterprise
        let _ = handle_github_deploy_key(&mut st, KeyCode::Enter); // → Host phase
        assert!(matches!(st.phase, DeployPhase::Host { .. }));
        for c in "https://company.ghe.com/".chars() {
            let _ = handle_github_deploy_key(&mut st, KeyCode::Char(c));
        }
        assert_eq!(
            handle_github_deploy_key(&mut st, KeyCode::Enter),
            DeployOutcome::Enterprise { domain: "company.ghe.com".to_string() }
        );
    }

    #[test]
    fn host_blank_enter_is_inert_and_esc_goes_back() {
        let mut st = GithubDeploymentState { phase: DeployPhase::Host { buffer: String::new() } };
        assert_eq!(handle_github_deploy_key(&mut st, KeyCode::Enter), DeployOutcome::Stay);
        assert_eq!(handle_github_deploy_key(&mut st, KeyCode::Esc), DeployOutcome::Stay);
        assert_eq!(st.phase, DeployPhase::Choose { selected: 1 });
    }

    #[test]
    fn choose_esc_cancels() {
        let mut st = GithubDeploymentState::default();
        assert_eq!(handle_github_deploy_key(&mut st, KeyCode::Esc), DeployOutcome::Cancel);
    }

    #[test]
    fn render_shows_both_phases() {
        let st = GithubDeploymentState::default();
        let out = render_github_deploy_to_string(&st);
        assert!(out.starts_with("Select GitHub deployment type"));
        assert!(out.contains("GitHub.com Public"));
        assert!(out.contains("GitHub Enterprise"));
        let host = GithubDeploymentState { phase: DeployPhase::Host { buffer: "x".into() } };
        assert!(render_github_deploy_to_string(&host).contains("Host: x"));
    }
}
