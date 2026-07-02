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
