//! `AnalyticsBus` — facade between event emitters and pluggable sinks.
//!
//! See spec §26.3. Engine code calls [`AnalyticsBus::log_event`] on a single
//! process-wide bus. Events submitted before a sink is attached are buffered
//! in a bounded queue and drained when the sink shows up.

use crate::killswitch::Killswitch;
use crate::sink::{AnalyticsSink, LogEventMetadata};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Single-sink event bus with a bounded pre-attach buffer.
///
/// Construction is cheap (`new` / `default`); the heavy work happens in
/// [`Self::attach_sink`], which drains the buffer in order.
pub struct AnalyticsBus {
    sink: RwLock<Option<Arc<dyn AnalyticsSink>>>,
    pending: Mutex<VecDeque<QueuedEvent>>,
    max_pending: usize,
    killswitch: Killswitch,
}

#[derive(Debug, Clone)]
struct QueuedEvent {
    name: String,
    metadata: LogEventMetadata,
    #[allow(dead_code)]
    is_async: bool,
}

impl AnalyticsBus {
    /// Construct an empty bus with no sink attached and a 1000-event buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sink: RwLock::new(None),
            pending: Mutex::new(VecDeque::new()),
            max_pending: 1000,
            killswitch: Killswitch::new(),
        }
    }

    /// Synchronous log. If no sink is attached, the event is buffered (oldest
    /// evicted at capacity). When the killswitch is active the event is
    /// silently dropped.
    pub async fn log_event(&self, name: &str, metadata: LogEventMetadata) {
        if self.killswitch.is_active() {
            return;
        }
        if let Some(sink) = self.sink.read().await.as_ref() {
            sink.log_event(name, metadata).await;
        } else {
            self.buffer(QueuedEvent {
                name: name.into(),
                metadata,
                is_async: false,
            })
            .await;
        }
    }

    /// Attach a sink and drain pending events through it in FIFO order.
    pub async fn attach_sink(&self, sink: Arc<dyn AnalyticsSink>) {
        *self.sink.write().await = Some(sink.clone());
        // Drain pending.
        let mut pending = self.pending.lock().await;
        while let Some(e) = pending.pop_front() {
            sink.log_event(&e.name, e.metadata).await;
        }
    }

    /// Borrow the killswitch so callers can activate it from outside.
    #[must_use]
    pub fn killswitch(&self) -> &Killswitch {
        &self.killswitch
    }

    async fn buffer(&self, e: QueuedEvent) {
        let mut p = self.pending.lock().await;
        if p.len() >= self.max_pending {
            p.pop_front();
        }
        p.push_back(e);
    }
}

impl Default for AnalyticsBus {
    fn default() -> Self {
        Self::new()
    }
}
