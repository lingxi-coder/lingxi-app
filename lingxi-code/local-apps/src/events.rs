//! Typed domain events emitted by [`crate::service::AppService`].
//!
//! The engine maps these 1:1 onto client-protocol `App*` events (phase 1 T3);
//! this crate deliberately knows nothing about the client protocol. Operation
//! FAILURES are not events here — service methods return typed
//! [`crate::error::AppError`]s and the engine synthesizes
//! `AppOperationFailed { code, message }` from them. The one deliberate
//! exception is [`AppEvent::DesignConflict`], which the spec requires to be
//! emitted alongside the `revision_conflict` error on draft edits.

use crate::types::{
    AppCheckpoint, AppDesignPatch, AppGenerationProgress, AppRecord, AppRuntimeState,
    AppWorkflowState, DesignValue,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// One domain event. Mirrors the client-protocol `App*` event surface minus
/// `AppOperationFailed` (derived from errors by the engine).
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// The app list changed (create/delete). Carries the full record list.
    AppsChanged {
        /// Every app record, in stored order.
        apps: Vec<AppRecord>,
    },
    /// A designer gate opened; the client needs `interaction_id` to confirm.
    DesignerRequested {
        /// App whose designer opened.
        app_id: String,
        /// Pending interaction id (the confirm capability).
        interaction_id: String,
        /// Draft revision at open time.
        revision: u64,
    },
    /// The draft changed (user edit or applied suggestion).
    DesignDraftChanged {
        /// App whose draft changed.
        app_id: String,
        /// New draft revision.
        revision: u64,
        /// Full field map after the change.
        fields: BTreeMap<String, DesignValue>,
    },
    /// The agent stored a suggestion awaiting explicit application.
    DesignSuggestionAvailable {
        /// App the suggestion targets.
        app_id: String,
        /// Id to echo back in `ApplyAgentDesignSuggestion`.
        suggestion_id: String,
        /// Draft revision the suggestion was computed against.
        based_on_revision: u64,
        /// The proposed edit.
        patch: AppDesignPatch,
    },
    /// A draft edit raced a newer revision; the user value was kept.
    DesignConflict {
        /// App whose draft conflicted.
        app_id: String,
        /// Revision the caller expected.
        expected_revision: u64,
        /// Revision the draft actually holds.
        actual_revision: u64,
    },
    /// The workflow state machine advanced.
    WorkflowChanged {
        /// App whose workflow advanced.
        app_id: String,
        /// New workflow state.
        state: AppWorkflowState,
        /// Optional human-readable detail (e.g. failure summary).
        detail: Option<String>,
    },
    /// Generation progress report (phase 3 produces these).
    GenerationProgress(AppGenerationProgress),
    /// The runtime record changed.
    RuntimeChanged {
        /// App whose runtime changed.
        app_id: String,
        /// New runtime state.
        state: AppRuntimeState,
        /// Failure detail when `state` is `failed`.
        last_error: Option<String>,
    },
    /// A preview gate opened; `url` stays `None` until the phase-4 runtime.
    PreviewReady {
        /// App whose preview is ready.
        app_id: String,
        /// Pending interaction id (the confirm capability).
        interaction_id: String,
        /// Draft revision the preview was generated from.
        revision: u64,
        /// Where the preview is served (phase 4).
        url: Option<String>,
    },
    /// A checkpoint was recorded (git wiring is phase 5; no phase-1
    /// producer, but the wire layer already maps the variant).
    CheckpointCreated {
        /// App the checkpoint belongs to.
        app_id: String,
        /// The recorded checkpoint.
        checkpoint: AppCheckpoint,
    },
}

/// Observer callback for [`AppEvent`]s.
///
/// Implementations must NOT call back into the emitting `AppService`.
#[async_trait]
pub trait AppEventObserver: Send + Sync {
    /// Handle one domain event.
    async fn on_event(&self, event: AppEvent);
}

/// Observer that ignores every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopAppEventObserver;

#[async_trait]
impl AppEventObserver for NoopAppEventObserver {
    async fn on_event(&self, _event: AppEvent) {}
}

/// Recording observer for tests.
#[derive(Debug, Default)]
pub struct RecordingAppEventObserver {
    events: Mutex<Vec<AppEvent>>,
}

impl RecordingAppEventObserver {
    /// Fresh empty observer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every event observed so far, in order.
    ///
    /// PANICS on a poisoned lock (as do [`Self::take`] and the observer
    /// callback): swallowing poison would turn "a recording call panicked
    /// mid-push" into a silently EMPTY log, letting emptiness assertions in
    /// tests pass vacuously over a real failure.
    #[must_use]
    pub fn events(&self) -> Vec<AppEvent> {
        self.events
            .lock()
            .expect("recording observer lock poisoned")
            .clone()
    }

    /// Drain and return the observed events.
    #[must_use]
    pub fn take(&self) -> Vec<AppEvent> {
        std::mem::take(&mut *self.events.lock().expect("recording observer lock poisoned"))
    }
}

#[async_trait]
impl AppEventObserver for RecordingAppEventObserver {
    async fn on_event(&self, event: AppEvent) {
        self.events
            .lock()
            .expect("recording observer lock poisoned")
            .push(event);
    }
}
