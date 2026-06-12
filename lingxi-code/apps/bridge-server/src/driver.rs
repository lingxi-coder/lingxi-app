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
        }
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
        // Each turn gets its own cancel token. Nothing trips it today; it is the
        // seam a future per-turn cancel command fires.
        let cancel = CancellationToken::new();
        match self
            .orchestrator
            .run_turn_streaming_with_cancel_image_sources(&prompt, sources, cancel)
            .await
        {
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
                ConversationMessage::User { content, .. } => Some(content.clone()),
                _ => None,
            })
            .expect("a user message must be in the outgoing request")
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
}
