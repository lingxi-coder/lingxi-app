//! 把一次结构化模型调用的实时输出，转成客户端能渲染的进度事件。
//!
//! Turn a structured call's live output into progress events the client can
//! render.
//!
//! The three local-app stages each spend tens of seconds inside a single model
//! call. Before this, that call was a black box: the client showed a spinner
//! and a stage name, and a user could not tell a slow generation from a wedged
//! one. [`GenerationTranscript`] bridges
//! [`crate::local_apps_llm::GenerationDeltaSink`] — called inline on the model
//! stream, so it must never block — to
//! `LocalAppsService::report_generation_progress`, which is async.
//!
//! Two properties the bridge must have, and how each is obtained:
//!
//! - **Never slow the model down.** The sink does a bounded `try_send` and
//!   DROPS on a full channel. A transcript is a view of the work, not the work
//!   itself; losing a chunk costs a flicker, while blocking the stream costs
//!   real generation time.
//! - **Never flood the event bus.** Chunks arrive token-by-token. The drain
//!   task coalesces everything that piles up within [`FLUSH_WINDOW`] into one
//!   event, so the event rate is bounded by time rather than by how fast the
//!   provider streams.

use crate::local_apps_llm::{GenerationDeltaKind, GenerationDeltaSink};
use local_apps::AppService;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Longest a chunk waits before it is published. Coalescing within this window
/// keeps the event rate bounded no matter how fast the provider streams.
const FLUSH_WINDOW: std::time::Duration = std::time::Duration::from_millis(200);

/// Chunks buffered before the sink starts dropping. Large enough that a normal
/// flush window never overflows, small enough that a stalled consumer cannot
/// grow unbounded.
const CHANNEL_DEPTH: usize = 256;

/// Longest single `detail` payload, in CHARS (not bytes — a byte cut would
/// split a multi-byte character, and every stage's output is Chinese).
///
/// `report_generation_progress` caps `detail` by bytes and REJECTS an
/// over-long one, so a burst that outran the flush window must be trimmed
/// here rather than lost at the service boundary.
const MAX_CHUNK_CHARS: usize = 1_000;

/// `stage` value marking a live thinking chunk.
pub const STAGE_THINKING: &str = "llm_thinking";
/// `stage` value marking a live text chunk.
pub const STAGE_TEXT: &str = "llm_text";

/// A live transcript for one app's in-flight model call.
///
/// Hold it for as long as the call runs; DROPPING it closes the channel, which
/// ends the drain task after it publishes whatever is still buffered.
pub struct GenerationTranscript {
    /// Kept so the drain task ends when the caller drops the transcript.
    _tx: mpsc::Sender<(GenerationDeltaKind, String)>,
    drain: tokio::task::JoinHandle<()>,
}

impl GenerationTranscript {
    /// Start publishing live deltas for `app_id`, returning the sink to hand to
    /// [`crate::local_apps_llm::LocalAppsLlm`] and the transcript to hold.
    #[must_use]
    pub fn spawn(service: Arc<AppService>, app_id: String) -> (Arc<dyn GenerationDeltaSink>, Self) {
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let drain = crate::local_apps_profile::worker_runtime()
            .spawn(drain_into_progress(service, app_id, rx));
        let sink: Arc<dyn GenerationDeltaSink> = Arc::new(ChannelSink { tx: tx.clone() });
        (sink, Self { _tx: tx, drain })
    }
}

impl Drop for GenerationTranscript {
    fn drop(&mut self) {
        // The channel closing is what ends the drain task; this only stops a
        // task whose final flush is still in flight from outliving the call.
        // Aborting is safe: every chunk it could still publish is a view of
        // work that has already finished.
        self.drain.abort();
    }
}

/// The sink handed to the model. Deliberately trivial: everything expensive
/// (batching, awaiting the service) happens in the drain task.
struct ChannelSink {
    tx: mpsc::Sender<(GenerationDeltaKind, String)>,
}

impl GenerationDeltaSink for ChannelSink {
    fn on_delta(&self, kind: GenerationDeltaKind, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        // `try_send`, never `send`: this runs inline on the model stream.
        // A full channel means the UI is behind, and a dropped chunk is
        // strictly better than a stalled generation.
        let _ = self.tx.try_send((kind, chunk.to_string()));
    }
}

/// Coalesce buffered chunks and publish them as progress events until the
/// sender is dropped.
async fn drain_into_progress(
    service: Arc<AppService>,
    app_id: String,
    mut rx: mpsc::Receiver<(GenerationDeltaKind, String)>,
) {
    let mut pending: Option<(GenerationDeltaKind, String)> = None;
    let mut deadline = tokio::time::Instant::now() + FLUSH_WINDOW;

    loop {
        let timeout = tokio::time::sleep_until(deadline);
        tokio::select! {
            received = rx.recv() => {
                match received {
                    Some((kind, chunk)) => match pending.take() {
                        // Same kind: keep appending into the open buffer.
                        Some((open, mut buf)) if open == kind => {
                            buf.push_str(&chunk);
                            pending = Some((open, buf));
                        }
                        // Kind CHANGED (thinking -> text): publish what is open
                        // before starting the new one, or the two would merge
                        // into a single mislabelled block downstream.
                        Some((open, buf)) => {
                            publish(&service, &app_id, open, buf).await;
                            pending = Some((kind, chunk));
                            deadline = tokio::time::Instant::now() + FLUSH_WINDOW;
                        }
                        None => {
                            pending = Some((kind, chunk));
                            deadline = tokio::time::Instant::now() + FLUSH_WINDOW;
                        }
                    },
                    // Sender dropped: the call is over. Publish the tail so the
                    // last words of a generation are not the ones lost.
                    None => {
                        if let Some((kind, buf)) = pending.take() {
                            publish(&service, &app_id, kind, buf).await;
                        }
                        return;
                    }
                }
            }
            () = timeout => {
                if let Some((kind, buf)) = pending.take() {
                    publish(&service, &app_id, kind, buf).await;
                }
                deadline = tokio::time::Instant::now() + FLUSH_WINDOW;
            }
        }
    }
}

/// Emit one coalesced chunk. Failures are swallowed on purpose: a transcript
/// that cannot be delivered must never fail the generation it is describing.
async fn publish(service: &AppService, app_id: &str, kind: GenerationDeltaKind, chunk: String) {
    let chunk = trim_to_chars(chunk, MAX_CHUNK_CHARS);
    if chunk.is_empty() {
        return;
    }
    let stage = match kind {
        GenerationDeltaKind::Thinking => STAGE_THINKING,
        GenerationDeltaKind::Text => STAGE_TEXT,
    };
    let _ = service
        .report_generation_progress(local_apps::AppGenerationProgress {
            app_id: app_id.to_string(),
            stage: stage.to_string(),
            // Deliberately absent: a live chunk says nothing about how far
            // along the PIPELINE is, and a percent here would fight the
            // stage events that own that number.
            percent: None,
            detail: Some(chunk),
        })
        .await;
}

/// Keep the LAST `max` chars. The tail is what a live transcript is for — a
/// burst that outran the flush window should show its most recent output, not
/// its oldest.
fn trim_to_chars(chunk: String, max: usize) -> String {
    if chunk.chars().count() <= max {
        return chunk;
    }
    let skip = chunk.chars().count() - max;
    chunk.chars().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trim_keeps_the_tail_and_never_splits_a_character() {
        // Every char here is 3 bytes: a byte-wise cut would produce invalid
        // UTF-8, which is exactly what this function exists to prevent.
        let chunk = "一二三四五".to_string();
        assert_eq!(trim_to_chars(chunk.clone(), 3), "三四五");
        assert_eq!(trim_to_chars(chunk.clone(), 5), chunk);
        assert_eq!(trim_to_chars(chunk, 99), "一二三四五");
    }

    #[test]
    fn a_full_channel_drops_rather_than_blocking() {
        // The sink runs inline on the model stream. Proving it returns while
        // the channel is full is the whole point: a blocking sink would make
        // a slow UI slow the model.
        let (tx, _rx) = mpsc::channel(1);
        let sink = ChannelSink { tx };
        for _ in 0..100 {
            sink.on_delta(GenerationDeltaKind::Text, "x");
        }
    }

    #[test]
    fn an_empty_chunk_is_not_forwarded() {
        let (tx, mut rx) = mpsc::channel(4);
        let sink = ChannelSink { tx };
        sink.on_delta(GenerationDeltaKind::Text, "");
        assert!(
            rx.try_recv().is_err(),
            "an empty chunk must not become an event"
        );
    }
}
