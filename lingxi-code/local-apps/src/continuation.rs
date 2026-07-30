//! Continuation delivery seam (spec §E).
//!
//! Human-gate outcomes (`design_confirmed`, `design_cancelled`,
//! `preview_confirmed`, `revision_requested`) are enqueued durably in the
//! per-app `interactions.json` and delivered through a [`ContinuationSink`].
//! `last_delivered_seq` advances only after a successful `deliver`, so
//! delivery is at-least-once across restarts and consumers must dedup by
//! `seq`.

use crate::error::AppError;
use crate::types::AppContinuation;
use async_trait::async_trait;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Where confirmed human-gate outcomes are delivered (e.g. injected into the
/// owning agent conversation by the engine).
///
/// Implementations must be idempotent per `(app_id, seq)`: redelivery after a
/// crash or a failed persist is expected, and a replayed seq must be a no-op
/// on the consumer side.
///
/// Deliveries may also RACE an app's deletion: a continuation picked up
/// before `delete_app` committed can arrive AFTER the deletion was announced
/// (delivery is lock-free in flight, and it can never be transactional with
/// an external conversation). Consumers must treat a continuation for an
/// unknown/deleted app as a no-op — the same tolerance the per-seq
/// idempotency rule already requires.
#[async_trait]
pub trait ContinuationSink: Send + Sync {
    /// Deliver one continuation. Returning `Err` leaves the continuation
    /// queued for a later redelivery attempt.
    async fn deliver(&self, app_id: &str, continuation: &AppContinuation) -> Result<(), AppError>;
}

/// Sink that drops every continuation on the floor (they stay queued only
/// until the successful no-op "delivery" marks them done). Used when no
/// conversation-injection seam is wired yet.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopContinuationSink;

#[async_trait]
impl ContinuationSink for NoopContinuationSink {
    async fn deliver(&self, _app_id: &str, _continuation: &AppContinuation) -> Result<(), AppError> {
        Ok(())
    }
}

/// Recording sink for tests: logs every `deliver` call, keeps a seq-deduped
/// `accepted` view (modelling a consumer that dedups by seq), and can be
/// toggled to fail (globally or for a single app) so redelivery paths are
/// exercisable.
#[derive(Debug, Default)]
pub struct RecordingContinuationSink {
    calls: Mutex<Vec<(String, AppContinuation)>>,
    accepted: Mutex<Vec<(String, AppContinuation)>>,
    seen: Mutex<BTreeSet<(String, u64)>>,
    fail: AtomicBool,
    fail_apps: Mutex<BTreeSet<String>>,
}

impl RecordingContinuationSink {
    /// Fresh sink that accepts deliveries.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// When `fail` is true every `deliver` returns an error (and records
    /// nothing), simulating an unavailable conversation.
    pub fn set_fail(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }

    /// Fail deliveries for `app_id` only (e.g. one app whose owning
    /// conversation is gone) while other apps keep delivering.
    ///
    /// All accessors PANIC on a poisoned lock (see
    /// `RecordingAppEventObserver`): swallowing poison would let emptiness
    /// assertions pass vacuously — or silently disarm a configured failure
    /// injection — over a real mid-test panic.
    pub fn set_fail_for(&self, app_id: &str, fail: bool) {
        let mut apps = self.fail_apps.lock().expect("recording sink lock poisoned");
        if fail {
            apps.insert(app_id.to_string());
        } else {
            apps.remove(app_id);
        }
    }

    /// Every successful `deliver` call, in order (including seq replays).
    #[must_use]
    pub fn calls(&self) -> Vec<(String, AppContinuation)> {
        self.calls.lock().expect("recording sink lock poisoned").clone()
    }

    /// Deliveries after consumer-side seq dedup: a replayed `(app_id, seq)`
    /// is a no-op and does not appear twice.
    #[must_use]
    pub fn accepted(&self) -> Vec<(String, AppContinuation)> {
        self.accepted
            .lock()
            .expect("recording sink lock poisoned")
            .clone()
    }
}

#[async_trait]
impl ContinuationSink for RecordingContinuationSink {
    async fn deliver(&self, app_id: &str, continuation: &AppContinuation) -> Result<(), AppError> {
        let app_fails = self
            .fail_apps
            .lock()
            .expect("recording sink lock poisoned")
            .contains(app_id);
        if self.fail.load(Ordering::SeqCst) || app_fails {
            return Err(AppError::Io(format!(
                "recording sink is set to fail (app {app_id}, seq {})",
                continuation.seq
            )));
        }
        self.calls
            .lock()
            .expect("recording sink lock poisoned")
            .push((app_id.to_string(), continuation.clone()));
        let fresh = self
            .seen
            .lock()
            .expect("recording sink lock poisoned")
            .insert((app_id.to_string(), continuation.seq));
        if fresh {
            self.accepted
                .lock()
                .expect("recording sink lock poisoned")
                .push((app_id.to_string(), continuation.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AppContinuationKind;

    fn continuation(seq: u64) -> AppContinuation {
        AppContinuation {
            seq,
            app_id: "abc123".into(),
            kind: AppContinuationKind::DesignConfirmed,
            payload: serde_json::json!({ "revision": 1 }),
            created_at_ms: 5,
        }
    }

    #[tokio::test]
    async fn replayed_seq_is_a_consumer_no_op() {
        let sink = RecordingContinuationSink::new();
        sink.deliver("abc123", &continuation(1)).await.unwrap();
        sink.deliver("abc123", &continuation(2)).await.unwrap();
        // At-least-once redelivery of seq 1.
        sink.deliver("abc123", &continuation(1)).await.unwrap();
        assert_eq!(sink.calls().len(), 3, "raw delivery log sees the replay");
        let accepted = sink.accepted();
        assert_eq!(
            accepted.iter().map(|(_, c)| c.seq).collect::<Vec<_>>(),
            vec![1, 2],
            "consumer dedups by seq, replay is a no-op"
        );
        // Same seq for a DIFFERENT app is not deduped.
        sink.deliver("other", &continuation(1)).await.unwrap();
        assert_eq!(sink.accepted().len(), 3);
    }

    #[tokio::test]
    async fn failing_sink_records_nothing() {
        let sink = RecordingContinuationSink::new();
        sink.set_fail(true);
        let err = sink.deliver("abc123", &continuation(1)).await.unwrap_err();
        assert_eq!(err.code(), crate::error::AppErrorCode::Io);
        assert!(sink.calls().is_empty());
        sink.set_fail(false);
        sink.deliver("abc123", &continuation(1)).await.unwrap();
        assert_eq!(sink.calls().len(), 1);
    }

    #[tokio::test]
    async fn noop_sink_always_succeeds() {
        let sink = NoopContinuationSink;
        sink.deliver("abc123", &continuation(9)).await.unwrap();
    }
}
