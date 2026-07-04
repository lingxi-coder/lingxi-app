//! The [`BottomPaneView`] contract: a transient, keyboard-owning surface
//! stacked over the composer (plan Phase 4).
//!
//! Ported from codex-rs `tui/src/bottom_pane/bottom_pane_view.rs` (UI
//! architecture pattern only — no codex product types). Where codex signals
//! completion through `is_complete()`/`completion()` side-state, the views
//! here return a [`ViewOutcome`] directly from [`BottomPaneView::handle_key`]
//! so the owning stack can pop them and the app can act on the result in one
//! pass.

use std::any::Any;

use crossterm::event::KeyEvent;
use permission::gate::PermissionResponse;

use crate::renderable::Renderable;

/// The result of routing one key (or paste) into the active view.
pub enum ViewOutcome {
    /// The view consumed the input and stays open.
    Pending,
    /// The view dismissed itself without an app-level effect (`Esc`).
    Cancelled,
    /// The view completed with a view-level action for its owner.
    Accepted(ViewAction),
    /// The view asks the app to submit `String` as a user prompt turn.
    SubmitPrompt(String),
    /// The user picked a model — the exact `(request_model, profile)` args
    /// `OrchestratorHandle::switch_model` accepts.
    SwitchModel {
        /// The wire model id to switch to.
        request_model: String,
        /// The provider profile the model routes through, when qualified.
        profile: Option<String>,
    },
    /// A permission prompt resolved with this response. The view has already
    /// delivered it through its one-shot channel; the variant tells the owner
    /// WHAT was answered (and that the view is done).
    PermissionResponse(PermissionResponse),
    /// Push a child view on top of this one (this view stays open beneath).
    OpenView(Box<dyn BottomPaneView>),
    /// The view asks the app to run a command effect on its behalf.
    RunCommand(CommandAction),
    /// The view asks the app to run a `/web` effect (secret/settings save or
    /// a test search) on its behalf. The view stays open — the async result
    /// (and any later close) is a later task's concern.
    RunWebAction(WebAction),
    /// The view asks the app to run a `/connect` effect (store an API key, or
    /// kick off a Copilot/OAuth sign-in) on its behalf. Unlike
    /// [`Self::RunWebAction`], the WHOLE `/connect` view stack (picker →
    /// method choice → key entry) is cleared when this fires — the flow is
    /// over and the result is reported into the transcript, not back into a
    /// still-open screen.
    RunConnectAction(ConnectAction),
}

/// An app-level `/connect` effect a view can request via
/// [`ViewOutcome::RunConnectAction`]. The owner runs these asynchronously
/// (secure-store writes, OAuth/Copilot device-flow sign-in) and reports
/// results back through the transcript (`TurnEvent::SystemNotice`).
#[derive(Clone, PartialEq, Eq)]
pub enum ConnectAction {
    /// Persist an API key through the secure credential store.
    StoreKey {
        /// Provider/keychain id to store the key under.
        provider_id: String,
        /// The entered secret.
        key: String,
    },
    /// Kick off the GitHub Copilot OAuth device-flow for `provider_id`.
    Copilot {
        /// Provider id being connected (normally `"github-copilot"`).
        provider_id: String,
    },
    /// Kick off first-party OAuth browser sign-in for `provider_id`.
    OAuth {
        /// Provider id being connected (e.g. `"anthropic"`).
        provider_id: String,
    },
}

// Hand-written `Debug` that REDACTS the secret `key` — the derived impl would
// print it verbatim, so any future `debug!`/panic on a `ConnectAction` would
// leak the credential. Everything else is shown for diagnostics.
impl std::fmt::Debug for ConnectAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreKey { provider_id, .. } => f
                .debug_struct("StoreKey")
                .field("provider_id", provider_id)
                .field("key", &"<redacted>")
                .finish(),
            Self::Copilot { provider_id } => f
                .debug_struct("Copilot")
                .field("provider_id", provider_id)
                .finish(),
            Self::OAuth { provider_id } => f
                .debug_struct("OAuth")
                .field("provider_id", provider_id)
                .finish(),
        }
    }
}

/// An app-level `/web` effect a view can request via
/// [`ViewOutcome::RunWebAction`]. The owner runs these asynchronously
/// (secure-store writes, settings writes, test network calls) and reports
/// results back through `TurnEvent::SystemNotice`.
#[derive(Clone, PartialEq, Eq)]
pub enum WebAction {
    /// Persist a secret key through the secure credential store.
    SaveSecret {
        provider: tool_web::web_search_config::WebSearchProvider,
        secret: String,
    },
    /// Persist non-secret settings (`provider`, optional SearXNG URL).
    SaveSettings {
        provider: tool_web::web_search_config::WebSearchProvider,
        searxng_url: Option<String>,
    },
    /// Run a test search for `provider`, optionally using a not-yet-saved
    /// `typed_key` (the config screen's in-progress input buffer) instead of
    /// the persisted credential.
    TestSearch {
        provider: tool_web::web_search_config::WebSearchProvider,
        typed_key: Option<String>,
    },
}

// Hand-written `Debug` that REDACTS the secret `secret`/`typed_key` — the
// derived impl would print them verbatim, leaking the credential through any
// future `debug!`/panic on a `WebAction`.
impl std::fmt::Debug for WebAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SaveSecret { provider, .. } => f
                .debug_struct("SaveSecret")
                .field("provider", provider)
                .field("secret", &"<redacted>")
                .finish(),
            Self::SaveSettings {
                provider,
                searxng_url,
            } => f
                .debug_struct("SaveSettings")
                .field("provider", provider)
                .field("searxng_url", searxng_url)
                .finish(),
            Self::TestSearch { provider, typed_key } => f
                .debug_struct("TestSearch")
                .field("provider", provider)
                .field("typed_key", &typed_key.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

/// The view-level payload of [`ViewOutcome::Accepted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewAction {
    /// A generic dialog confirmed the option at this index.
    Selected(usize),
}

/// An app-level command effect a view can request via
/// [`ViewOutcome::RunCommand`]. Mirrors the effects `RataApp::handle_slash`
/// executes directly today; the full slash-command registry (plan Phase 8)
/// will grow this into the single dispatch currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    /// Clear the conversation transcript (`/clear`).
    ClearTranscript,
    /// Exit the app (`/exit`, `/quit`).
    Quit,
    /// Apply (and persist) this theme setting (`/theme` picker commit).
    SetTheme(tui_core::theme::ThemeSetting),
}

/// A transient focused surface shown in the bottom pane: permission prompt,
/// model picker, read-only screens, and future form-like views.
///
/// While a view is on the stack it owns the keyboard (and the paste stream);
/// the stack routes input to the TOP view only and pops views according to
/// the [`ViewOutcome`] they return.
pub trait BottomPaneView: Renderable {
    /// Route a key pressed while this view is the active (top) view.
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome;

    /// Route a bracketed paste while this view is active. Modal views own the
    /// paste stream, so the default swallows it without effect.
    fn handle_paste(&mut self, _text: &str) -> ViewOutcome {
        ViewOutcome::Pending
    }

    /// Whether the status line (and the composer beneath) should still render
    /// while this view is active. Full-frame views that own the whole
    /// viewport return `false`.
    fn wants_status_line(&self) -> bool {
        true
    }

    /// Whether this view should also be dismissed when a child view stacked
    /// above it completes acceptingly (codex parity: parent views that exist
    /// only to spawn a child flow).
    fn dismiss_after_child_accept(&self) -> bool {
        false
    }

    /// Downcasting support (MSRV 1.82 has no `dyn` trait upcasting):
    /// implementations return `self`.
    fn as_any(&self) -> &dyn Any;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_web::web_search_config::WebSearchProvider;

    /// The hand-written `Debug` for `ConnectAction`/`WebAction` must NEVER
    /// print the secret — a derived impl would, leaking credentials into any
    /// future `debug!`/panic message.
    #[test]
    fn connect_action_debug_redacts_the_key() {
        let action = ConnectAction::StoreKey {
            provider_id: "anthropic".to_string(),
            key: "sk-super-secret-value".to_string(),
        };
        let rendered = format!("{action:?}");
        assert!(!rendered.contains("sk-super-secret-value"), "leaked: {rendered}");
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("anthropic"));
    }

    #[test]
    fn web_action_debug_redacts_secret_and_typed_key() {
        let save = WebAction::SaveSecret {
            provider: WebSearchProvider::Tavily,
            secret: "tvly-super-secret".to_string(),
        };
        assert!(!format!("{save:?}").contains("tvly-super-secret"));
        let test = WebAction::TestSearch {
            provider: WebSearchProvider::Brave,
            typed_key: Some("brave-typed-secret".to_string()),
        };
        assert!(!format!("{test:?}").contains("brave-typed-secret"));
    }
}
