//! Test fixtures for the streaming path.
//!
//! - [`MockStreamingApiClient`] — implements [`crate::conversation::StreamingApiClient`]
//!   over a per-turn `Vec<StreamEvent>` script.
//! - [`scripted!`] — declarative macro for assembling event sequences
//!   with the high-level vocabulary `text`, `tool_use`, `end_turn`,
//!   `tool_use_stop`, etc.
//! - [`MockToolDispatchClock`] — records the wall-clock instant each
//!   tool dispatch begins, for the mid-stream dispatch test.
//!
//! Mirroring `test_support`, this module is compiled unconditionally
//! (not feature-gated) so integration tests can use the fixtures
//! without needing `--features test-support`. Production builds drop
//! the unused symbols at link-time.
#![forbid(unsafe_code)]

use crate::conversation::StreamingApiClient;
use api_client::types::{
    ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent, UsageApi,
};
use api_client::ApiError;
use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use protocol::{ConversationMessage, ToolUseId};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use traits::HttpError;

/// Captured arguments of one `StreamingApiClient::stream` call.
#[derive(Debug, Clone)]
pub struct CapturedStreamCall {
    /// Model name as requested by the orchestrator.
    pub model: String,
    /// Assembled system prompt (`None` if omitted).
    pub system: Option<String>,
    /// Conversation history snapshot at the time of the call.
    pub messages: Vec<ConversationMessage>,
    /// Wire tool definitions advertised on this call (`build_wire_tools`).
    pub tools: Vec<Value>,
}

/// Mock streaming client. Yields the next per-turn script of events each
/// time `stream` is called. If the queue is exhausted, returns
/// `ApiError::Http(HttpError::Connection("streaming script exhausted"))`.
pub struct MockStreamingApiClient {
    turns: Mutex<std::collections::VecDeque<Vec<Result<StreamEvent, ApiError>>>>,
    captured: Arc<Mutex<Vec<CapturedStreamCall>>>,
}

impl MockStreamingApiClient {
    /// Construct an empty (always-exhausted) mock.
    #[must_use]
    pub fn empty() -> Self {
        Self::with_turns(Vec::new())
    }

    /// Construct from a Vec where each inner Vec is the scripted event
    /// sequence for one turn.
    #[must_use]
    pub fn with_turns(turns: Vec<Vec<StreamEvent>>) -> Self {
        let mapped: Vec<Vec<Result<StreamEvent, ApiError>>> = turns
            .into_iter()
            .map(|t| t.into_iter().map(Ok).collect())
            .collect();
        Self {
            turns: Mutex::new(mapped.into()),
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Construct from already-fallible turns (used to inject an `Err`
    /// mid-stream for the error-propagation test).
    #[must_use]
    pub fn with_fallible_turns(turns: Vec<Vec<Result<StreamEvent, ApiError>>>) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Snapshot the captured `stream` call args (one entry per call).
    pub async fn captured_calls(&self) -> Vec<CapturedStreamCall> {
        self.captured.lock().await.clone()
    }
}

#[async_trait]
impl StreamingApiClient for MockStreamingApiClient {
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<Value>,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        self.captured.lock().await.push(CapturedStreamCall {
            model: model.to_string(),
            system: system.map(str::to_string),
            messages,
            tools,
        });
        let mut queue = self.turns.lock().await;
        let next = queue.pop_front().ok_or_else(|| {
            ApiError::Http(HttpError::Connection("streaming script exhausted".into()))
        })?;
        let s = stream::iter(next).boxed();
        Ok(s)
    }
}

/// Records wall-clock instants when tool dispatches begin. Used by the
/// mid-stream test to assert dispatch fires BEFORE `message_stop`.
#[derive(Debug, Default, Clone)]
pub struct MockToolDispatchClock {
    /// Each entry is `(tool name, instant the dispatch began)`.
    pub dispatch_instants: Arc<Mutex<Vec<(String, Instant)>>>,
}

impl MockToolDispatchClock {
    /// Record that a tool dispatch began at this instant.
    pub async fn record(&self, tool: &str) {
        self.dispatch_instants
            .lock()
            .await
            .push((tool.to_string(), Instant::now()));
    }
    /// Snapshot the dispatch instants.
    pub async fn snapshot(&self) -> Vec<(String, Instant)> {
        self.dispatch_instants.lock().await.clone()
    }
}

// ─── scripted! macro helpers ───────────────────────────────────────────

/// `message_start` event with the given id + model.
#[must_use]
pub fn message_start(id: &str, model: &str) -> StreamEvent {
    StreamEvent::MessageStart {
        message: MessageResponse {
            id: id.to_string(),
            model: model.to_string(),
            content: Vec::new(),
            stop_reason: None,
            usage: UsageApi::default(),
        },
    }
}

/// `content_block_start` for a `text` block at `index`.
#[must_use]
pub fn content_block_start_text(index: u32) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlockApi::Text {
            text: String::new(),
        },
    }
}

/// `content_block_start` for a `tool_use` block at `index`. The `id`
/// parameter accepts a raw `ToolUseId` so test scripts can correlate
/// dispatches with the eventual `ToolResult`.
#[must_use]
pub fn content_block_start_tool_use(index: u32, id: ToolUseId, name: &str) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlockApi::ToolUse {
            id,
            name: name.to_string(),
            input: Value::Object(serde_json::Map::new()),
        },
    }
}

/// `content_block_delta { delta: TextDelta { text } }`.
#[must_use]
pub fn text_delta(index: u32, text: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentDelta::TextDelta {
            text: text.to_string(),
        },
    }
}

/// `content_block_delta { delta: InputJsonDelta { partial_json } }`.
#[must_use]
pub fn input_json_delta(index: u32, partial: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentDelta::InputJsonDelta {
            partial_json: partial.to_string(),
        },
    }
}

/// `content_block_stop { index }`.
#[must_use]
pub fn content_block_stop(index: u32) -> StreamEvent {
    StreamEvent::ContentBlockStop { index }
}

/// `message_delta { delta: { stop_reason } }`.
#[must_use]
pub fn message_delta_stop(stop_reason: &str) -> StreamEvent {
    StreamEvent::MessageDelta {
        delta: MessageDeltaPayload {
            stop_reason: Some(stop_reason.to_string()),
        },
        usage: None,
    }
}

/// `message_stop`.
#[must_use]
pub fn message_stop() -> StreamEvent {
    StreamEvent::MessageStop
}

/// `ping` keepalive.
#[must_use]
pub fn ping() -> StreamEvent {
    StreamEvent::Ping
}

/// Declarative macro for assembling an event sequence. Pass any
/// expression that evaluates to a `StreamEvent`. Example:
/// ```ignore
/// let s = scripted![
///     message_start("msg_1", "claude-opus-4-7"),
///     content_block_start_text(0),
///     text_delta(0, "hello"),
///     content_block_stop(0),
///     message_delta_stop("end_turn"),
///     message_stop(),
/// ];
/// ```
#[macro_export]
macro_rules! scripted {
    [$($event:expr),* $(,)?] => {
        vec![$($event),*]
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_yields_first_turn_then_exhausts() {
        let mock = MockStreamingApiClient::with_turns(vec![scripted![
            message_start("m1", "claude-opus-4-7"),
            message_stop(),
        ]]);
        let s = mock
            .stream("claude-opus-4-7", None, Vec::new(), Vec::new())
            .await
            .expect("first turn");
        let collected: Vec<_> = s.collect().await;
        assert_eq!(collected.len(), 2);
        assert!(matches!(collected[0], Ok(StreamEvent::MessageStart { .. })));
        assert!(matches!(collected[1], Ok(StreamEvent::MessageStop)));

        let result = mock
            .stream("claude-opus-4-7", None, Vec::new(), Vec::new())
            .await;
        // `Result::expect_err` requires `Ok` to be `Debug`; `BoxStream`
        // is not. Match on the result instead.
        match result {
            Ok(_) => panic!("expected exhaustion error"),
            Err(e) => assert!(matches!(e, ApiError::Http(HttpError::Connection(_)))),
        }
    }

    #[tokio::test]
    async fn captured_calls_record_model_and_system() {
        let mock = MockStreamingApiClient::with_turns(vec![scripted![message_stop()]]);
        let result = mock
            .stream("claude-opus-4-7", Some("sys"), Vec::new(), Vec::new())
            .await;
        // `Result::expect` requires Ok = `BoxStream` to be Debug;
        // it is not. Discriminate via `is_ok` instead.
        assert!(result.is_ok(), "call should succeed");
        let calls = mock.captured_calls().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].model, "claude-opus-4-7");
        assert_eq!(calls[0].system.as_deref(), Some("sys"));
    }
}
