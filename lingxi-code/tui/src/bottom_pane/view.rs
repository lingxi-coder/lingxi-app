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
use std::time::Instant;

use crossterm::event::KeyEvent;
use permission::gate::PermissionResponse;
use permission::{PermissionBehavior, PermissionUpdateDestination};

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
    /// The view asks the app to run a `/permissions` effect (persist an
    /// added/removed allow/ask/deny rule to a settings file) on its behalf.
    /// Like [`Self::RunWebAction`]'s test path, the editor stays OPEN so the
    /// user can make several edits; the async persist result is reported back
    /// through `TurnEvent::SystemNotice`.
    RunPermissionAction(PermissionAction),
    /// The `/tasks` picker asks the app to STOP a running background task. Like
    /// [`Self::RunPermissionAction`], the picker stays OPEN so the user can stop
    /// several tasks; the async `TaskRegistryHandle::kill` result is reported
    /// through `TurnEvent::SystemNotice`.
    RunTaskAction(TaskAction),
    /// A `/plugin` view asked the app to toggle the on-disk
    /// `settings.enabledPlugins` allowlist. The manager stays OPEN (like the
    /// `/permissions` editor); the async settings write is reported back
    /// through `TurnEvent::SystemNotice` and reflected on the next open.
    RunPluginAction(PluginAction),
    /// The `/cd` confirm view was accepted: move the session's working
    /// directory to this (already-resolved, absolute) path. Unlike
    /// [`Self::RunPermissionAction`] (which keeps the editor open), this
    /// completes the confirm view — it hits `ViewStack::apply`'s accepting
    /// catch-all and pops. The owner performs the actual `SessionCwd` swap +
    /// `tengu_cd_command` + result notice off-loop (it carries no live
    /// `SessionCwd`/telemetry handle itself).
    ChangeDirectory(std::path::PathBuf),
    /// The `/resume` picker resolved to this session uuid. Unlike the off-loop
    /// effect variants above, this UNWINDS the app loop: the owner
    /// (`RataApp::run` → `run_app`) returns an `AppExit::SwitchSession(uuid)` so
    /// the runtime is re-mounted in-process against that session (the JSONL
    /// writer is retargeted) — NEVER an in-place `resume_session` swap.
    SwitchSession(uuid::Uuid),
    /// The `/rewind` picker resolved to `message` with restore `scope`. Like
    /// [`Self::SwitchSession`] this UNWINDS the app loop (the owner returns
    /// `AppExit::Rewind`) so the code is rewound and/or the conversation is
    /// truncated + re-mounted in-process.
    Rewind {
        /// The target user-message uuid (the checkpoint key).
        message: uuid::Uuid,
        /// Which parts to restore.
        scope: RewindScope,
    },
    /// The user delivered a held cross-session message. The owner should
    /// start a skip-append turn (`run_async_hook_rewake`) so the inbox
    /// drain can inject the released body.
    RewakePeer,
}

/// Which parts of the session a `/rewind` restore should touch (claude-code
/// `RestoreOption`, first-cut = the three concrete scopes; `summarize` /
/// `summarize_up_to` are deferred).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindScope {
    /// Restore BOTH the working-tree files and the conversation (claude `both`).
    CodeAndConversation,
    /// Restore only the working-tree files to the checkpoint (claude `code`).
    CodeOnly,
    /// Restore only the conversation position (claude `conversation`).
    ConversationOnly,
}

/// An app-level `/permissions` effect a view can request via
/// [`ViewOutcome::RunPermissionAction`]. The owner runs these asynchronously
/// (settings-file merge via [`permission::persist_permission_update`] /
/// [`permission::remove_permission_update`], plus an in-memory
/// `session_allow_rules` push for an added allow rule so it takes effect this
/// session) and reports the result back through `TurnEvent::SystemNotice`.
///
/// Rule strings are the `"Tool"` / `"Tool(content)"` wire form and are NOT
/// secret, so the derived `Debug` is fine (unlike [`WebAction`]/[`ConnectAction`]).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionAction {
    /// Add `rule` to `permissions.{allow|ask|deny}` (keyed by `behavior`) in
    /// the `dest` settings file.
    Add {
        /// The rule string (`"Bash(npm:*)"`), as typed.
        rule: String,
        /// Which permission bucket the rule goes into.
        behavior: PermissionBehavior,
        /// Which settings file to persist to (User/Project/Local).
        dest: PermissionUpdateDestination,
    },
    /// Remove `rule` from `permissions.{allow|ask|deny}` (keyed by `behavior`)
    /// in the `dest` settings file it came from.
    Remove {
        /// The rule string to remove.
        rule: String,
        /// Which permission bucket the rule lives in.
        behavior: PermissionBehavior,
        /// Which settings file the rule came from (User/Project/Local).
        dest: PermissionUpdateDestination,
    },
    /// Add `path` to `permissions.additionalDirectories` in the `dest` settings
    /// file (the `/add-dir` command). Reuses the `/permissions` off-loop effect
    /// channel; the write goes through `permission::persist_workspace_directory`.
    AddDirectory {
        /// Absolute, normalized directory path to add.
        path: String,
        /// Which settings file to persist to.
        dest: PermissionUpdateDestination,
    },
    /// Move the session's working directory to `path` (the `/cd` command,
    /// parity 2.1.207). Reuses the `/permissions` off-loop effect channel (like
    /// [`Self::AddDirectory`]) rather than growing a dedicated app callback: the
    /// owner swaps the shared `tool_api::SessionCwd` cell to `(path, [path])` —
    /// the same cell `EnterWorktree`/`ExitWorktree` swap — emits
    /// `tengu_cd_command`, and prints the byte-exact `command_api::cd`
    /// result message via `TurnEvent::SystemNotice`.
    ChangeDirectory {
        /// Absolute, normalized target directory (validated to exist + be a
        /// directory by `crate::add_dir::resolve_and_validate` before confirm).
        path: String,
    },
}

/// An app-level `/tasks` effect a view can request via
/// [`ViewOutcome::RunTaskAction`]. The owner aborts the task OFF-LOOP via
/// [`platform_api::task_registry::TaskRegistryHandle::kill`] on the live runtime and
/// reports the result back through `TurnEvent::SystemNotice`. The 9-char task
/// id is not secret, so the derived `Debug` is fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAction {
    /// Stop the running background task with this 9-char id.
    Kill {
        /// The `[bartwmdks][0-9a-z]{8}` task id to stop.
        task_id: String,
    },
}

/// An app-level `/plugin` effect a view can request via
/// [`ViewOutcome::RunPluginAction`]. The owner runs these asynchronously
/// (an on-disk `settings.enabledPlugins` read-modify-write via the CLI
/// `plugin_settings::run_enable`/`run_disable` seam) and reports the result
/// back through `TurnEvent::SystemNotice`. The plugin id is a non-secret
/// `plugin` / `plugin@marketplace` string, so the derived `Debug` is fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAction {
    /// Set `enabledPlugins[id] = true` (auto-scope: user).
    Enable {
        /// The plugin id (bare name or `name@marketplace`).
        id: String,
    },
    /// Set `enabledPlugins[id] = false` at its holding scope.
    Disable {
        /// The plugin id (bare name or `name@marketplace`).
        id: String,
    },
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
            Self::TestSearch {
                provider,
                typed_key,
            } => f
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

    /// Advance time-based view state during the app's regular render tick.
    /// Most views are event-driven and keep the default no-op; countdown or
    /// animation-backed views can resolve themselves without synthesizing a
    /// keyboard event.
    fn handle_tick(&mut self, _now: Instant) -> ViewOutcome {
        ViewOutcome::Pending
    }

    /// Fold a pushed multi-agent lifecycle update into this view. Most views
    /// are unrelated and keep the default no-op; workflow list/detail views
    /// override it so an already-open `/workflows` screen stays live.
    fn apply_multiagent_event(&mut self, _event: &tui_core::multiagent::MultiAgentEvent) {}

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
        assert!(
            !rendered.contains("sk-super-secret-value"),
            "leaked: {rendered}"
        );
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
