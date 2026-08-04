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
    AppCheckpoint, AppDesignPatch, AppGenerationProgress, AppRecord, AppRuntimeRecord,
    AppWorkflowState, DesignValue,
};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

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
        /// Complete persisted runtime snapshot.
        runtime: AppRuntimeRecord,
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

/// Opaque handle returned by [`AppEventFanout::subscribe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AppEventSubscription(u64);

/// Multi-subscriber observer for a profile-global [`crate::AppService`].
///
/// Subscribers are held weakly so closing a client connection never keeps
/// its event bridge alive. Delivery snapshots upgraded subscribers under the
/// lock, then invokes callbacks after releasing it.
#[derive(Default)]
pub struct AppEventFanout {
    next_id: AtomicU64,
    observers: Mutex<HashMap<u64, Weak<dyn AppEventObserver>>>,
}

impl AppEventFanout {
    /// Create an empty fanout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            observers: Mutex::new(HashMap::new()),
        }
    }

    /// Attach an observer. The caller retains its strong `Arc` for the
    /// lifetime of the subscription.
    #[allow(clippy::needless_pass_by_value)] // ergonomic Arc-to-trait coercion for host callers
    pub fn subscribe(&self, observer: Arc<dyn AppEventObserver>) -> AppEventSubscription {
        let mut id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            id = self.next_id.fetch_add(1, Ordering::Relaxed);
        }
        self.observers
            .lock()
            .expect("app event fanout lock poisoned")
            .insert(id, Arc::downgrade(&observer));
        AppEventSubscription(id)
    }

    /// Detach a subscription. Returns whether it was registered.
    pub fn unsubscribe(&self, subscription: AppEventSubscription) -> bool {
        self.observers
            .lock()
            .expect("app event fanout lock poisoned")
            .remove(&subscription.0)
            .is_some()
    }

    /// Count live subscribers and prune dropped weak registrations.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        let mut observers = self
            .observers
            .lock()
            .expect("app event fanout lock poisoned");
        observers.retain(|_, observer| observer.strong_count() > 0);
        observers.len()
    }
}

#[async_trait]
impl AppEventObserver for AppEventFanout {
    async fn on_event(&self, event: AppEvent) {
        let observers: Vec<Arc<dyn AppEventObserver>> = {
            let mut registrations = self
                .observers
                .lock()
                .expect("app event fanout lock poisoned");
            let mut live = Vec::with_capacity(registrations.len());
            registrations.retain(|_, observer| match observer.upgrade() {
                Some(observer) => {
                    live.push(observer);
                    true
                }
                None => false,
            });
            live
        };
        for observer in observers {
            observer.on_event(event.clone()).await;
        }
    }
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
        std::mem::take(
            &mut *self
                .events
                .lock()
                .expect("recording observer lock poisoned"),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> AppEvent {
        AppEvent::AppsChanged { apps: Vec::new() }
    }

    #[tokio::test]
    async fn fanout_delivers_to_live_subscribers_and_unsubscribes() {
        let fanout = AppEventFanout::new();
        let first = Arc::new(RecordingAppEventObserver::new());
        let second = Arc::new(RecordingAppEventObserver::new());
        let first_token = fanout.subscribe(first.clone());
        fanout.subscribe(second.clone());
        fanout.on_event(event()).await;
        assert_eq!(first.events(), vec![event()]);
        assert_eq!(second.events(), vec![event()]);
        assert!(fanout.unsubscribe(first_token));
        fanout.on_event(event()).await;
        assert_eq!(first.events().len(), 1);
        assert_eq!(second.events().len(), 2);
    }

    #[tokio::test]
    async fn fanout_prunes_dropped_subscribers() {
        let fanout = AppEventFanout::new();
        let observer = Arc::new(RecordingAppEventObserver::new());
        fanout.subscribe(observer.clone());
        assert_eq!(fanout.subscriber_count(), 1);
        drop(observer);
        assert_eq!(fanout.subscriber_count(), 0);
        fanout.on_event(event()).await;
    }
}
