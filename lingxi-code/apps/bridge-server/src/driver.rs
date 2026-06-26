//! S1 — the production [`TurnDriver`] over a real [`ConversationOrchestrator`].
//!
//! This is the engine entry the [`crate::server::BridgeConnection`] calls when an
//! inbound [`client_protocol::commands::ClientCommand::SendPrompt`] arrives. It is
//! the production counterpart of the test-only driver that lived inside the F2-06
//! e2e suite (`tests/e2e_permission_test.rs`): a thin wrapper that turns one
//! `run_turn(prompt)` call into one streaming turn on the orchestrator.
//!
//! ## How the events reach the client
//!
//! The driver does NOT hold the [`client_adapter::AdapterOutputStream`] directly —
//! the orchestrator already owns it as its [`traits::OutputStream`] (wired at
//! construction, by `engine_desktop::build` in production or by the test harness).
//! Because that output stream lowers every callback into a
//! [`client_protocol::events::ClientEvent`] and forwards it through the
//! connection-scoped [`client_adapter::ClientEventSink`]
//! ([`crate::server::BridgeConnection::event_sink`]), simply driving the streaming
//! turn is enough: `TextDelta`, `ToolUseStarted`/`Result`, the per-turn
//! `CostUpdate`, and the terminal `TurnEnded` all flow out as `Frame::Event`s as a
//! side effect. The driver's only job is to START the turn (on a spawned task, per
//! the [`TurnDriver`] contract) and to surface a turn-level FAILURE as a
//! [`ClientEvent::Error`] so the client is never left waiting silently.
//!
//! ## Cancellation
//!
//! Each turn gets a fresh [`CancellationToken`]; the driver uses the orchestrator's
//! cancelable streaming entry ([`ConversationOrchestrator::run_turn_streaming_with_cancel`])
//! so a future per-turn cancel command (an `AbortTurn`-style control) has a hook to
//! fire it. Today nothing cancels mid-driver, so the token is never tripped and the
//! turn runs to completion — behavior identical to the plain streaming entry.

use std::sync::Arc;

use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::commands::ImageRefDto;
use client_protocol::events::{ClientEvent, ErrorKindDto};
// `ImageSource` is re-exported from the orchestrator (the canonical, FROZEN
// `protocol` shape) so this library code can name it without taking a direct
// `protocol` dependency.
use orchestrator::conversation::ImageSource;
use orchestrator::{ConversationOrchestrator, OrchestratorError};
use tokio_util::sync::CancellationToken;

use crate::server::TurnDriver;

/// A msgqueue-backed [`orchestrator::prompt::mid_turn_input::MidTurnInputSource`].
///
/// Bridges the orchestrator's queue-agnostic mid-turn drain seam to the
/// connection's [`msgqueue::MessageQueueManager`]: each
/// [`MidTurnInputSource::take_mid_turn_input`] snapshots the `Next`-priority
/// MAIN-THREAD, NON-slash prompts, joins the consecutive ones via
/// [`msgqueue::join_prompt_values`], REMOVES the consumed commands from the
/// queue (so the between-turn drain doesn't re-run them), and returns the joined
/// text for injection as a meta user message. Returns `None` when nothing
/// batchable is queued.
///
/// `Now`-priority commands are deliberately EXCLUDED: a `Now` enqueue aborts the
/// in-flight turn (via the queue's now-abort hook) and must survive to the
/// between-turn drain so it runs as its own interrupting turn, rather than being
/// silently folded into the running turn as injected mid-turn text.
///
/// This is the composition-root half of the seam — it lives in the bridge (which
/// owns the per-connection queue) so the `orchestrator` crate keeps NO
/// dependency on `msgqueue`. Twin of claude-code's query.ts ~1570-1580
/// snapshot+`joinPromptValues`+inject path.
pub struct MsgQueueMidTurnInput {
    queue: Arc<msgqueue::MessageQueueManager>,
}

impl MsgQueueMidTurnInput {
    /// Build the adapter over the connection's queue.
    #[must_use]
    pub fn new(queue: Arc<msgqueue::MessageQueueManager>) -> Self {
        Self { queue }
    }
}

#[async_trait]
impl orchestrator::prompt::mid_turn_input::MidTurnInputSource for MsgQueueMidTurnInput {
    async fn take_mid_turn_input(&self) -> Option<String> {
        // Snapshot the highest-priority main-thread, non-slash prompts for
        // mid-turn injection, preserving priority+FIFO order. SCOPE TO `Next`
        // ONLY: a `Now`-priority command must NOT be consumed here — it has
        // already aborted the in-flight turn (via the queue's now-abort hook
        // firing the registered cancel token) and must remain in the queue so
        // the between-turn drain runs it as its own interrupting turn. The
        // `Next` threshold already excludes `Later`; the equality predicate
        // additionally excludes `Now` (since `Later < Next < Now`).
        let batch = self
            .queue
            .get_by_max_priority(msgqueue::QueuePriority::Next, |c| {
                c.is_main_thread()
                    && !c.is_slash_command()
                    && c.priority == msgqueue::QueuePriority::Next
            })
            .await;
        let (joined, consumed) = msgqueue::join_prompt_values(&batch)?;
        // Consume-once: remove the merged commands so the between-turn drain
        // never re-runs them.
        self.queue
            .remove(&consumed, "drained mid-turn into running turn")
            .await;
        Some(joined)
    }
}

/// A msgqueue-backed [`tool_cron::WakeupScheduler`] — the composition-root impl
/// of the `/loop` dynamic-mode one-shot self-wakeup seam (Phase 2).
///
/// Twin of [`MsgQueueMidTurnInput`]: it lives in the bridge (which owns the
/// per-connection queue + the [`traits::RuntimeSpawner`]) so the `tool-cron`
/// crate — and the orchestrator — keep NO knowledge of how a wakeup is delivered.
/// [`WakeupScheduler::schedule`] spawns ONE background task that
/// [`RuntimeSpawner::sleep`]s for `delay`, resolves the autonomous sentinel via
/// [`tool_cron::resolve_wakeup_prompt`], and ENQUEUEs the resolved prompt at
/// [`msgqueue::QueuePriority::Next`] so the between-turn / mid-turn drain folds
/// it into the session as its own follow-up turn.
///
/// `Next` (not `Now`) is deliberate: a self-wakeup should resume work between
/// turns, not abort an in-flight turn the user may be watching.
///
/// WIRING: attached at `boot::assemble`. The `ScheduleWakeupTool` is built deep
/// inside `engine_desktop::build` (via `tool_cron::register_all_with_auth`)
/// BEFORE the per-connection queue + spawner exist, so it holds an empty
/// set-once `WakeupSchedulerCell` surfaced on `DesktopRuntime`; `assemble` fills
/// that cell with this adapter once the queue + `runtime_spawner` are available.
/// Hosts without a per-connection queue (CLI / offline / mobile) leave the cell
/// empty → the tool is an honest no-op.
pub struct MsgQueueWakeupScheduler {
    queue: Arc<msgqueue::MessageQueueManager>,
    runtime: Arc<dyn traits::RuntimeSpawner>,
}

impl MsgQueueWakeupScheduler {
    /// Build the adapter over the connection's queue + the host runtime spawner.
    #[must_use]
    pub fn new(
        queue: Arc<msgqueue::MessageQueueManager>,
        runtime: Arc<dyn traits::RuntimeSpawner>,
    ) -> Self {
        Self { queue, runtime }
    }
}

#[async_trait]
impl tool_cron::WakeupScheduler for MsgQueueWakeupScheduler {
    async fn schedule(&self, delay: std::time::Duration, prompt: String, reason: String) {
        let queue = self.queue.clone();
        let runtime = self.runtime.clone();
        // Spawn a detached one-shot timer (engine code must not call
        // `tokio::spawn` directly — D17 — so go through the runtime seam).
        let _ = runtime
            .clone()
            .spawn(
                "loop-wakeup",
                Box::pin(async move {
                    runtime.sleep(delay).await;
                    // Resolve the `<<autonomous-loop-dynamic>>` sentinel at fire
                    // time (else passthrough).
                    let resolved = tool_cron::resolve_wakeup_prompt(&prompt);
                    queue
                        .enqueue(msgqueue::QueuedCommand {
                            uuid: format!("loop-wakeup-{}", uuid_like(&reason)),
                            content: msgqueue::QueuedCommandContent::UserInput { text: resolved },
                            priority: msgqueue::QueuePriority::Next,
                            queued_at: std::time::SystemTime::now(),
                            source: msgqueue::QueueSource::Cron,
                            agent_id: None,
                            // The resolved text is a /loop input meant for the
                            // model (it may begin with `/`); the dynamic-mode
                            // contract re-fires the same input, so route it as a
                            // slash command when applicable — leave the default
                            // (false) so a leading `/` IS treated as a slash
                            // command, matching how the user originally typed it.
                            skip_slash_commands: false,
                            is_meta: false,
                        })
                        .await;
                }),
            )
            .await;
    }
}

/// Cheap pseudo-unique suffix for the wakeup command uuid, derived from the
/// reason + the current nanos. Not cryptographic — only needs to disambiguate
/// concurrent wakeups for trace correlation.
fn uuid_like(reason: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}-{}", reason.len())
}

/// A production [`TurnDriver`] backed by a real [`ConversationOrchestrator`].
///
/// Wraps the orchestrator (whose [`client_adapter::AdapterOutputStream`] is already
/// wired to the connection's event sink) plus a clone of that same
/// [`ClientEventSink`] — held ONLY so a turn-level error (an `Err` out of the
/// streaming entry) can be surfaced as a [`ClientEvent::Error`]. Successful events
/// flow through the orchestrator's own output stream, not through this handle.
pub struct OrchestratorTurnDriver {
    orchestrator: Arc<ConversationOrchestrator>,
    /// The connection's event sink, shared with the orchestrator's output stream.
    /// Used solely to emit a terminal [`ClientEvent::Error`] on turn failure;
    /// `None` to drop errors silently (e.g. tests that only assert success).
    error_sink: Option<Arc<dyn ClientEventSink>>,
    /// The connection's message queue, wired so each turn's fresh
    /// [`CancellationToken`] is REGISTERED with the queue at turn start (so a
    /// `Now`-priority enqueue aborts the in-flight turn) and CLEARED at turn end.
    /// `None` ⇒ no registration; the turn runs uninterruptibly by the queue
    /// (the test drivers and any caller that builds the driver without a queue).
    queue: Option<Arc<msgqueue::MessageQueueManager>>,
    /// The abort-reason flag the orchestrator reads to tell a `Now`-command abort
    /// from a user interrupt. RESET to `UserInterrupt` at each turn start so a
    /// stale `QueueNowCommand` from a prior turn cannot mislabel this one. `None`
    /// when no queue is wired. The queue's now-abort hook sets it to
    /// `QueueNowCommand` right before firing the token.
    cancel_reason: Option<orchestrator::prompt::mid_turn_input::CancelReasonFlag>,
}

impl OrchestratorTurnDriver {
    /// Construct a driver that drops turn-level errors silently.
    ///
    /// Use this when the caller does not need failures surfaced as
    /// [`ClientEvent::Error`] (e.g. a test wired to a mock that never errors).
    /// The production server uses [`Self::with_error_sink`] so an error reaches
    /// the client.
    #[must_use]
    pub fn new(orchestrator: Arc<ConversationOrchestrator>) -> Self {
        Self {
            orchestrator,
            error_sink: None,
            queue: None,
            cancel_reason: None,
        }
    }

    /// Wire the connection's message queue + abort-reason flag so each turn's
    /// cancel token is registered with the queue (a `Now` enqueue aborts the
    /// in-flight turn) and the reason flag is reset at turn start. Additive over
    /// [`Self::new`] / [`Self::with_error_sink`]: a driver built without this
    /// behaves exactly as before (no queue-driven abort). The composition root
    /// (`boot::assemble`) calls this with the [`crate::server::BridgeConnection`]'s
    /// per-connection queue and the SAME flag wired into the orchestrator.
    #[must_use]
    pub fn with_queue(
        mut self,
        queue: Arc<msgqueue::MessageQueueManager>,
        cancel_reason: orchestrator::prompt::mid_turn_input::CancelReasonFlag,
    ) -> Self {
        self.queue = Some(queue);
        self.cancel_reason = Some(cancel_reason);
        self
    }

    /// Construct a driver that surfaces a turn-level failure as a
    /// [`ClientEvent::Error`] on `error_sink`.
    ///
    /// `error_sink` must be the SAME connection-scoped
    /// [`crate::server::BridgeConnection::event_sink`] the orchestrator's
    /// [`client_adapter::AdapterOutputStream`] was built from, so the error frame
    /// rides the one outbound channel in order behind any events the failed turn
    /// already streamed.
    #[must_use]
    pub fn with_error_sink(
        orchestrator: Arc<ConversationOrchestrator>,
        error_sink: Arc<dyn ClientEventSink>,
    ) -> Self {
        Self {
            orchestrator,
            error_sink: Some(error_sink),
            queue: None,
            cancel_reason: None,
        }
    }

    /// Map an [`OrchestratorError`] to a wire [`ClientEvent::Error`].
    ///
    /// The coarse [`ErrorKindDto`] mirrors the streaming output stream's own
    /// classification: API/transport failures are `Transport`, a stream-protocol
    /// violation is `Protocol`, a `max_turns` budget hit is `MaxTurns`, and any
    /// other internal failure is `Internal`.
    fn error_event(err: &OrchestratorError) -> ClientEvent {
        let kind = match err {
            // RateLimitRejected is an enriched ApiCall(RateLimited) — same
            // Transport class as the error it replaces (matches client-adapter).
            OrchestratorError::ApiCall(_)
            | OrchestratorError::Streaming(_)
            | OrchestratorError::RateLimitRejected { .. } => ErrorKindDto::Transport,
            OrchestratorError::StreamingProtocol(_)
            | OrchestratorError::StreamEndedWithoutStop => ErrorKindDto::Protocol,
            OrchestratorError::MaxTurnsReached { .. } => ErrorKindDto::MaxTurns,
            _ => ErrorKindDto::Internal,
        };
        ClientEvent::Error {
            kind,
            message: err.to_string(),
        }
    }

    /// Convert the wire [`ImageRefDto`]s (uniform inline `{media_type, base64}`,
    /// decision §0.8) into the canonical [`ImageSource::Base64`]. The media type
    /// and base64 bytes are taken STRAIGHT from the DTO — no content sniffing and
    /// no temp-file round-trip; the inline bytes ride directly onto the outgoing
    /// user message.
    fn to_image_sources(images: Vec<ImageRefDto>) -> Vec<ImageSource> {
        images
            .into_iter()
            .map(|dto| ImageSource::Base64 {
                media_type: dto.media_type,
                data: dto.base64,
            })
            .collect()
    }

    /// Drive ONE streaming turn with already-decoded image `sources`, surfacing a
    /// turn-level failure as a terminal [`ClientEvent::Error`] when an error sink
    /// is wired. Shared by [`TurnDriver::run_turn`] (no images) and
    /// [`TurnDriver::run_turn_with_images`]; an empty `sources` vector is
    /// byte-identical to the pre-MULTIMODAL.1 text-only turn.
    async fn drive_turn(&self, prompt: String, sources: Vec<ImageSource>) {
        // Each turn gets its own cancel token. A `Now`-priority enqueue fires it
        // (via the queue's registered active-turn token) to abort the in-flight
        // turn so the urgent command runs next; a future per-turn cancel command
        // can fire the same seam.
        let cancel = CancellationToken::new();
        // NOW-ABORT wiring: register this turn's token with the queue so a `Now`
        // enqueue aborts it, and RESET the abort-reason flag to `UserInterrupt`
        // so a stale `QueueNowCommand` from the previous turn can't mislabel this
        // one. Both no-ops when no queue is wired (the test drivers).
        if let Some(reason) = self.cancel_reason.as_ref() {
            reason.reset();
        }
        if let Some(queue) = self.queue.as_ref() {
            queue.register_active_turn(cancel.clone()).await;
        }
        let result = self
            .orchestrator
            .run_turn_streaming_with_cancel_image_sources(&prompt, sources, cancel)
            .await;
        // Clear the active-turn token at turn end (graceful OR error): a later
        // `Now` enqueue between turns then has nothing to abort and simply waits
        // for the between-turn drain. No-op when no queue is wired.
        if let Some(queue) = self.queue.as_ref() {
            queue.clear_active_turn().await;
        }
        match result {
            // Success / cancellation / max-turns all already produced their
            // terminal events through the orchestrator's output stream
            // (`TurnEnded`, etc.) — nothing more to push here.
            Ok(_) => {}
            // A hard failure never reached `emit_end_turn`, so surface it
            // explicitly as a terminal `Error` event (when an error sink is wired)
            // rather than letting the client hang.
            Err(err) => {
                if let Some(sink) = &self.error_sink {
                    sink.emit(Self::error_event(&err)).await;
                } else {
                    tracing::debug!(error = %err, "bridge-server: turn failed (no error sink)");
                }
            }
        }
    }
}

#[async_trait]
impl TurnDriver for OrchestratorTurnDriver {
    async fn run_turn(&self, prompt: String) {
        // No images: drive with an empty source set — identical to routing through
        // `run_turn_streaming_with_cancel` (which decodes `&[]` to an empty vec).
        self.drive_turn(prompt, Vec::new()).await;
    }

    /// MULTIMODAL.1: route pasted/attached images through to the model instead of
    /// dropping them. Each inline `ImageRefDto` becomes an
    /// [`ImageSource::Base64`], which the orchestrator appends to the outgoing
    /// user message via `ConversationMessage::user_with_images`.
    async fn run_turn_with_images(&self, prompt: String, images: Vec<ImageRefDto>) {
        let sources = Self::to_image_sources(images);
        self.drive_turn(prompt, sources).await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use client_adapter::{AdapterOutputStream, ClientEventSink, MockSink};
    use client_protocol::commands::ImageRefDto;
    use orchestrator::conversation::ImageSource;
    use orchestrator::test_support::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, noop_hook_executor, text_delta, MockApiClient, MockStreamingApiClient,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
    use permission::gate::PermissionGate;
    use protocol::{ContentBlock, ConversationMessage};

    use super::OrchestratorTurnDriver;
    use crate::server::TurnDriver;

    /// A mock streaming client scripting ONE minimal turn (assistant text, then
    /// `end_turn`) so a `run_turn*` call drives to completion and captures the
    /// outgoing request via `captured_calls()`.
    fn streaming_one_turn() -> Arc<MockStreamingApiClient> {
        Arc::new(MockStreamingApiClient::with_turns(vec![scripted![
            message_start("m1", "claude-sonnet-4-20250514"),
            content_block_start_text(0),
            text_delta(0, "ok"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]))
    }

    /// Build a production driver over an orchestrator wired to `streaming` (whose
    /// `captured_calls()` records the outgoing request messages). No network, no
    /// API key — the mock streaming client scripts the whole turn.
    fn build_driver(streaming: Arc<MockStreamingApiClient>) -> OrchestratorTurnDriver {
        let batched = Arc::new(MockApiClient::new(Vec::new()));
        let sink = MockSink::arc();
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(AdapterOutputStream::new(sink as Arc<dyn ClientEventSink>));
        let tools = Arc::new(tool_api::registry::ToolRegistry::new());
        let orchestrator = Arc::new(ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched,
            streaming,
            tools,
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate) as Arc<dyn PermissionGate>,
            output,
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        ));
        OrchestratorTurnDriver::new(orchestrator)
    }

    /// Extract the content blocks of the FIRST user message in a captured request.
    fn first_user_content(messages: &[ConversationMessage]) -> Vec<ContentBlock> {
        messages
            .iter()
            .find_map(|m| match m {
                // R-P1: skip the leading additional-context `<system-reminder>`
                // meta (is_meta: true); the real prompt + images ride on the
                // first NON-meta user message.
                ConversationMessage::User {
                    content,
                    is_meta: false,
                    ..
                } => Some(content.clone()),
                _ => None,
            })
            .expect("a non-meta user message must be in the outgoing request")
    }

    /// The pure DTO→source conversion preserves the media type and base64 bytes
    /// verbatim (no sniffing, order preserved) — the load-bearing MULTIMODAL.1 map.
    #[test]
    fn image_ref_dto_converts_to_base64_source_preserving_fields() {
        let dtos = vec![
            ImageRefDto {
                media_type: "image/png".to_string(),
                base64: "iVBORw0KGgoAAAA".to_string(),
            },
            ImageRefDto {
                media_type: "image/jpeg".to_string(),
                base64: "/9j/4AAQSkZJRg".to_string(),
            },
        ];

        let sources = OrchestratorTurnDriver::to_image_sources(dtos);

        assert_eq!(
            sources,
            vec![
                ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgoAAAA".to_string(),
                },
                ImageSource::Base64 {
                    media_type: "image/jpeg".to_string(),
                    data: "/9j/4AAQSkZJRg".to_string(),
                },
            ]
        );
    }

    /// End-to-end on the BRIDGE path: a turn driven with inline images lands those
    /// bytes on the OUTGOING user message as a `ContentBlock::Image` carrying the
    /// exact `ImageSource::Base64` — proving the image reaches the model instead of
    /// being silently dropped.
    #[tokio::test]
    async fn run_turn_with_images_reaches_outgoing_user_message() {
        let streaming = streaming_one_turn();
        let driver = build_driver(streaming.clone());

        let dto = ImageRefDto {
            media_type: "image/png".to_string(),
            base64: "iVBORw0KGgoAAAA".to_string(),
        };
        driver
            .run_turn_with_images("describe this".to_string(), vec![dto])
            .await;

        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1, "exactly one stream request");
        let content = first_user_content(&calls[0].messages);

        assert!(
            content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text == "describe this")),
            "the prompt text must precede the image: {content:?}"
        );
        let source = content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Image { source } => Some(source.clone()),
                _ => None,
            })
            .expect("an image block must reach the outgoing user message");
        assert_eq!(
            source,
            ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgoAAAA".to_string(),
            }
        );
    }

    /// With an empty image set, `run_turn_with_images` builds the SAME outgoing
    /// user message as the plain text-only `run_turn` — no image blocks, identical
    /// content — proving the additive path is byte-identical when there are no
    /// images.
    #[tokio::test]
    async fn empty_images_matches_text_only_run_turn() {
        let streaming_plain = streaming_one_turn();
        build_driver(streaming_plain.clone())
            .run_turn("hello".to_string())
            .await;

        let streaming_empty = streaming_one_turn();
        build_driver(streaming_empty.clone())
            .run_turn_with_images("hello".to_string(), Vec::new())
            .await;

        let plain = first_user_content(&streaming_plain.captured_calls().await[0].messages);
        let empty = first_user_content(&streaming_empty.captured_calls().await[0].messages);

        assert_eq!(
            plain, empty,
            "empty-images user content must equal the text-only path"
        );
        assert!(
            plain
                .iter()
                .all(|b| !matches!(b, ContentBlock::Image { .. })),
            "the text-only path carries no image blocks: {plain:?}"
        );
    }

    // ========================================================================
    // §27 mid-turn drain adapter (`MsgQueueMidTurnInput`) + Now-abort wiring.
    // ========================================================================

    use msgqueue::{
        MessageQueueManager, QueuePriority, QueueSource, QueuedCommand, QueuedCommandContent,
    };
    use orchestrator::prompt::mid_turn_input::MidTurnInputSource;
    use std::time::SystemTime;
    use tokio_util::sync::CancellationToken;

    fn user_cmd(uuid: &str, prio: QueuePriority, text: &str) -> QueuedCommand {
        QueuedCommand {
            uuid: uuid.to_string(),
            content: QueuedCommandContent::UserInput { text: text.to_string() },
            priority: prio,
            queued_at: SystemTime::now(),
            source: QueueSource::PromptInput,
            agent_id: None,
            skip_slash_commands: false,
            is_meta: false,
        }
    }

    /// The adapter joins consecutive `Next` main-thread prompts, returns the
    /// joined text, and REMOVES the consumed commands from the queue (consume-once
    /// so the between-turn drain never re-runs them).
    #[tokio::test]
    async fn mid_turn_adapter_joins_and_consumes_queued_prompts() {
        let queue = Arc::new(MessageQueueManager::new());
        queue.enqueue(user_cmd("a", QueuePriority::Next, "first")).await;
        queue.enqueue(user_cmd("b", QueuePriority::Next, "second")).await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());

        let joined = adapter.take_mid_turn_input().await;
        assert_eq!(joined.as_deref(), Some("first\nsecond"));
        // Consumed — the queue is now empty, so a second drain yields None.
        assert!(queue.is_empty().await);
        assert_eq!(adapter.take_mid_turn_input().await, None);
    }

    /// A `Now`-priority command is EXCLUDED from the mid-turn drain: it has
    /// already aborted the in-flight turn and must be PRESERVED in the queue for
    /// the between-turn drain to run as its own interrupting turn, not folded
    /// into the running turn as injected text. The drain still consumes the
    /// `Next` prompt that precedes it.
    #[tokio::test]
    async fn mid_turn_adapter_excludes_now_and_preserves_it() {
        let queue = Arc::new(MessageQueueManager::new());
        queue.enqueue(user_cmd("a", QueuePriority::Next, "first")).await;
        queue.enqueue(user_cmd("urgent", QueuePriority::Now, "do it now")).await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());

        // Only the `Next` prompt is drained mid-turn; the `Now` command is left.
        let joined = adapter.take_mid_turn_input().await;
        assert_eq!(joined.as_deref(), Some("first"));

        // The `Now` command survives for the between-turn drain.
        assert_eq!(queue.len().await, 1);
        let remaining = queue
            .get_by_max_priority(QueuePriority::Now, |_| true)
            .await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].uuid, "urgent");
        assert_eq!(remaining[0].priority, QueuePriority::Now);

        // A second mid-turn drain finds nothing batchable (Now stays excluded).
        assert_eq!(adapter.take_mid_turn_input().await, None);
        assert_eq!(queue.len().await, 1);
    }

    /// A slash command is EXCLUDED from the mid-turn drain (it is routed
    /// post-turn), so the adapter returns `None` when only a slash command waits.
    #[tokio::test]
    async fn mid_turn_adapter_excludes_slash_commands() {
        let queue = Arc::new(MessageQueueManager::new());
        queue.enqueue(user_cmd("s", QueuePriority::Next, "/clear")).await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());
        assert_eq!(adapter.take_mid_turn_input().await, None);
        // The slash command is left in the queue for the post-turn path.
        assert_eq!(queue.len().await, 1);
    }

    /// A subagent-scoped command (has an `agent_id`) is EXCLUDED by the
    /// main-thread filter, so it never leaks into the coordinator's mid-turn drain.
    #[tokio::test]
    async fn mid_turn_adapter_scopes_to_main_thread() {
        let queue = Arc::new(MessageQueueManager::new());
        let mut sub = user_cmd("sub", QueuePriority::Next, "subagent input");
        sub.agent_id = Some(protocol::AgentId::new());
        queue.enqueue(sub).await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());
        assert_eq!(adapter.take_mid_turn_input().await, None);
        assert_eq!(queue.len().await, 1);
    }

    /// A `Now`-priority enqueue, with the driver having registered the turn's
    /// cancel token via `with_queue`, fires the token AND sets the reason flag —
    /// proving the end-to-end Now-abort wiring (the driver registers, the queue
    /// hook records the reason, the token trips).
    #[tokio::test]
    async fn with_queue_registers_token_and_now_enqueue_aborts_with_reason() {
        use orchestrator::prompt::mid_turn_input::{CancelReason, CancelReasonFlag};

        let queue = Arc::new(MessageQueueManager::new());
        let reason = CancelReasonFlag::new();
        // Install the now-abort hook exactly as boot::assemble does.
        {
            let r = reason.clone();
            queue
                .set_now_abort_hook(Arc::new(move || r.set(CancelReason::QueueNowCommand)))
                .await;
        }

        // Register a turn token (what drive_turn does at turn start).
        let token = CancellationToken::new();
        queue.register_active_turn(token.clone()).await;
        assert!(!token.is_cancelled());
        assert_eq!(reason.get(), CancelReason::UserInterrupt);

        // A Now enqueue trips the token AND records the reason.
        queue.enqueue(user_cmd("urgent", QueuePriority::Now, "do it now")).await;
        assert!(token.is_cancelled(), "Now enqueue must abort the active turn");
        assert_eq!(
            reason.get(),
            CancelReason::QueueNowCommand,
            "the now-abort hook must record QueueNowCommand before firing"
        );

        // Build a driver wired with the queue + reason to prove the API compiles
        // and the seam is reachable (smoke).
        let _driver = build_driver(streaming_one_turn()).with_queue(queue, reason);
    }

    /// A minimal real-spawning `RuntimeSpawner` for the wakeup-adapter test: it
    /// spawns the future on the current tokio runtime and uses real `sleep`.
    struct TestRuntime;
    #[async_trait::async_trait]
    impl traits::RuntimeSpawner for TestRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::runtime::RuntimeError> {
            tokio::spawn(task);
            Ok(traits::BackgroundTaskHandle {
                task_name: name.to_string(),
                task_id: 0,
            })
        }
        async fn sleep(&self, duration: std::time::Duration) {
            tokio::time::sleep(duration).await;
        }
        async fn cancel(
            &self,
            _handle: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::runtime::RuntimeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn wakeup_scheduler_enqueues_resolved_prompt_after_delay() {
        use super::MsgQueueWakeupScheduler;
        use tool_cron::WakeupScheduler;

        // PARITY: the sentinel resolver gate `is_loop_default_prompt_enabled`
        // (binary `fJr`/`tengu_kairos_loop_prompt`) DEFAULTS OFF (FLAG-ONLY), so a
        // sentinel passes through verbatim unless the flag is on. Enable it via the
        // test-only flag override so this test exercises the resolution path, and
        // clear the shared delivery state so the FIRST-delivery branch fires.
        telemetry::test_set_flag("tengu_kairos_loop_prompt", true);
        tool_cron::reset_autonomous_loop_delivered();

        let queue = Arc::new(MessageQueueManager::new());
        let runtime: Arc<dyn traits::RuntimeSpawner> = Arc::new(TestRuntime);
        let sched = MsgQueueWakeupScheduler::new(queue.clone(), runtime);

        // Schedule a 0-delay wakeup carrying the autonomous sentinel — it must be
        // resolved to the instruction block before being enqueued.
        sched
            .schedule(
                std::time::Duration::from_millis(0),
                "<<autonomous-loop-dynamic>>".to_string(),
                "idle tick".to_string(),
            )
            .await;

        // Give the spawned timer a moment to fire + enqueue.
        for _ in 0..50 {
            if queue.len().await > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let cmd = queue
            .dequeue()
            .await
            .expect("wakeup must have enqueued one command");
        assert_eq!(cmd.priority, QueuePriority::Next);
        assert_eq!(cmd.source, msgqueue::QueueSource::Cron);
        let text = cmd.text().expect("user-input text");
        // The sentinel resolved to the autonomous-loop instruction block.
        assert_ne!(text, "<<autonomous-loop-dynamic>>");
        assert!(text.contains("autonomous"));
        assert!(text.contains("ScheduleWakeup"));

        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
    }

    #[tokio::test]
    async fn wakeup_scheduler_passthrough_prompt() {
        use super::MsgQueueWakeupScheduler;
        use tool_cron::WakeupScheduler;

        let queue = Arc::new(MessageQueueManager::new());
        let runtime: Arc<dyn traits::RuntimeSpawner> = Arc::new(TestRuntime);
        let sched = MsgQueueWakeupScheduler::new(queue.clone(), runtime);
        sched
            .schedule(
                std::time::Duration::from_millis(0),
                "5m /babysit-prs".to_string(),
                "repeat loop".to_string(),
            )
            .await;
        for _ in 0..50 {
            if queue.len().await > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let cmd = queue.dequeue().await.expect("enqueued");
        assert_eq!(cmd.text(), Some("5m /babysit-prs"));
    }
}
