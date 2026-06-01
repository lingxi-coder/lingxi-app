//! Core router: model aliases, fallback chains, and retry/backoff, modeled as
//! `LlmProvider` decorators the registry composes.

use crate::capabilities::Capabilities;
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::BoxStream;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Retry policy for transient failures.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Max total attempts (clamped to >= 1).
    pub max_attempts: u32,
    /// Base backoff between attempts (multiplied by the attempt number).
    pub backoff_ms: u64,
}

/// Router configuration parsed from the settings `routing` object.
#[derive(Debug, Clone, Default)]
pub struct RoutingConfig {
    /// `alias -> "provider/model"`.
    pub aliases: BTreeMap<String, String>,
    /// `alias-or-"provider/model" -> ["provider/model", …]` fallback chain.
    pub fallback: BTreeMap<String, Vec<String>>,
    /// Optional retry policy applied to every resolved provider.
    pub retry: Option<RetryPolicy>,
}

/// Whether an error is worth retrying / failing over (transient).
#[must_use]
pub fn is_retryable(e: &ApiError) -> bool {
    matches!(e, ApiError::Server { status, .. } if *status == 429 || *status >= 500)
        || matches!(e, ApiError::RateLimited { .. } | ApiError::UnexpectedStreamEnd)
}

/// Retries `inner` on transient errors with linear backoff.
pub struct RetryingProvider {
    inner: Arc<dyn LlmProvider>,
    max_attempts: u32,
    backoff_ms: u64,
}

impl RetryingProvider {
    /// Wrap `inner` with a retry policy (`max_attempts` clamped to >= 1).
    #[must_use]
    pub fn new(inner: Arc<dyn LlmProvider>, max_attempts: u32, backoff_ms: u64) -> Self {
        Self { inner, max_attempts: max_attempts.max(1), backoff_ms }
    }
}

#[async_trait]
impl LlmProvider for RetryingProvider {
    fn id(&self) -> ProviderId { self.inner.id() }
    fn capabilities(&self) -> &Capabilities { self.inner.capabilities() }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.inner.complete(req.clone()).await {
                Ok(r) => return Ok(r),
                Err(e) if attempt < self.max_attempts && is_retryable(&e) => {
                    tokio::time::sleep(Duration::from_millis(self.backoff_ms * u64::from(attempt))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.inner.stream(req.clone()).await {
                Ok(s) => return Ok(s),
                Err(e) if attempt < self.max_attempts && is_retryable(&e) => {
                    tokio::time::sleep(Duration::from_millis(self.backoff_ms * u64::from(attempt))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Tries each `(provider, model)` in order, advancing on a transient error.
pub struct FallbackProvider {
    members: Vec<(Arc<dyn LlmProvider>, String)>,
}

impl FallbackProvider {
    /// Build from a non-empty member list (primary first); each carries its
    /// provider-local model id (rebound onto the request before each call).
    #[must_use]
    pub fn new(members: Vec<(Arc<dyn LlmProvider>, String)>) -> Self {
        Self { members }
    }
}

#[async_trait]
impl LlmProvider for FallbackProvider {
    fn id(&self) -> ProviderId { self.members[0].0.id() }
    fn capabilities(&self) -> &Capabilities { self.members[0].0.capabilities() }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let mut last = None;
        for (provider, model) in &self.members {
            let mut r = req.clone();
            r.model.clone_from(model);
            match provider.complete(r).await {
                Ok(resp) => return Ok(resp),
                Err(e) if is_retryable(&e) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| ApiError::MalformedStream("empty fallback chain".to_string())))
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut last = None;
        for (provider, model) in &self.members {
            let mut r = req.clone();
            r.model.clone_from(model);
            match provider.stream(r).await {
                Ok(s) => return Ok(s),
                Err(e) if is_retryable(&e) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| ApiError::MalformedStream("empty fallback chain".to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{MessageResponse, StreamEvent};
    use api_client::ApiError;
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use traits::HttpError;

    /// Mock provider that returns pre-seeded outcomes (complete only).
    struct MockProvider {
        id: ProviderId,
        caps: Capabilities,
        outcomes: Mutex<VecDeque<Result<MessageResponse, ApiError>>>,
    }

    impl MockProvider {
        fn new(id: ProviderId, outcomes: Vec<Result<MessageResponse, ApiError>>) -> Arc<Self> {
            Arc::new(Self {
                id,
                caps: Capabilities::openai(),
                outcomes: Mutex::new(outcomes.into()),
            })
        }

        fn ok_response() -> MessageResponse {
            MessageResponse {
                id: "id".to_string(),
                model: "m".to_string(),
                stop_reason: Some("end_turn".to_string()),
                content: vec![],
                usage: api_client::types::UsageApi::default(),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn id(&self) -> ProviderId { self.id.clone() }
        fn capabilities(&self) -> &Capabilities { &self.caps }

        async fn complete(&self, _req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
            self.outcomes
                .lock()
                .expect("mock mutex")
                .pop_front()
                .unwrap_or(Err(ApiError::MalformedStream("no more outcomes".to_string())))
        }

        async fn stream(&self, _req: CanonicalRequest) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
            let result = self.outcomes
                .lock()
                .expect("mock mutex")
                .pop_front()
                .unwrap_or(Err(ApiError::MalformedStream("no more outcomes".to_string())));
            // For stream tests: map Ok(MessageResponse) -> Ok(empty stream), Err -> Err
            result.map(|_| {
                let s: BoxStream<'static, Result<StreamEvent, ApiError>> =
                    Box::pin(futures::stream::empty());
                s
            })
        }
    }

    fn dummy_req() -> CanonicalRequest {
        CanonicalRequest {
            model: "test-model".to_string(),
            system: None,
            messages: vec![],
            max_tokens: 10,
            temperature: None,
            stream: false,
            tools: vec![],
            reasoning_effort: None,
            thinking_budget: None,
        }
    }

    fn server_err(status: u16) -> ApiError {
        ApiError::Server { status, body: "err".to_string() }
    }

    // ── is_retryable classification ──────────────────────────────────────────

    #[test]
    fn is_retryable_classifies() {
        // true cases
        assert!(is_retryable(&server_err(429)));
        assert!(is_retryable(&server_err(503)));
        assert!(is_retryable(&server_err(500)));
        assert!(is_retryable(&ApiError::RateLimited { retry_after_secs: 5 }));
        assert!(is_retryable(&ApiError::UnexpectedStreamEnd));

        // false cases
        assert!(!is_retryable(&server_err(400)));
        assert!(!is_retryable(&ApiError::Unauthorized("bad key".to_string())));
        assert!(!is_retryable(&ApiError::MalformedStream("x".to_string())));
        assert!(!is_retryable(&ApiError::Http(HttpError::InvalidRequest("x".to_string()))));
    }

    // ── RetryingProvider tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn retrying_succeeds_after_transient() {
        let mock = MockProvider::new(
            ProviderId::Anthropic,
            vec![Err(server_err(503)), Ok(MockProvider::ok_response())],
        );
        let provider = RetryingProvider::new(mock, 3, 1);
        let result = provider.complete(dummy_req()).await;
        assert!(result.is_ok(), "expected Ok but got: {:?}", result.err());
    }

    #[tokio::test]
    async fn retrying_gives_up_after_max() {
        let mock = MockProvider::new(
            ProviderId::Anthropic,
            vec![Err(server_err(503)), Err(server_err(503)), Err(server_err(503))],
        );
        let provider = RetryingProvider::new(mock, 2, 1);
        let result = provider.complete(dummy_req()).await;
        assert!(result.is_err(), "expected Err after max attempts");
    }

    #[tokio::test]
    async fn retrying_does_not_retry_terminal() {
        let mock = MockProvider::new(
            ProviderId::Anthropic,
            vec![Err(ApiError::Unauthorized("bad".to_string())), Ok(MockProvider::ok_response())],
        );
        // Arc so we can check remaining outcomes afterward
        let inner: Arc<dyn LlmProvider> = mock.clone();
        let provider = RetryingProvider::new(inner, 3, 1);
        let result = provider.complete(dummy_req()).await;
        assert!(result.is_err());
        // Second outcome should still be in the queue — only 1 call was made
        let remaining = mock.outcomes.lock().expect("mutex").len();
        assert_eq!(remaining, 1, "terminal error should not retry; expected 1 outcome remaining");
    }

    // ── FallbackProvider tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn fallback_advances_on_transient() {
        let mock_a = MockProvider::new(ProviderId::Anthropic, vec![Err(server_err(503))]);
        let mock_b = MockProvider::new(ProviderId::OpenAI, vec![Ok(MockProvider::ok_response())]);
        let members: Vec<(Arc<dyn LlmProvider>, String)> = vec![
            (mock_a, "model-a".to_string()),
            (mock_b, "model-b".to_string()),
        ];
        let provider = FallbackProvider::new(members);
        let result = provider.complete(dummy_req()).await;
        assert!(result.is_ok(), "expected fallback to succeed via mock_b");
    }

    #[tokio::test]
    async fn fallback_stops_on_terminal() {
        let mock_b = MockProvider::new(ProviderId::OpenAI, vec![Ok(MockProvider::ok_response())]);
        let mock_b_clone = mock_b.clone();
        let mock_a = MockProvider::new(ProviderId::Anthropic, vec![Err(ApiError::Unauthorized("bad".to_string()))]);
        let members: Vec<(Arc<dyn LlmProvider>, String)> = vec![
            (mock_a, "model-a".to_string()),
            (mock_b_clone, "model-b".to_string()),
        ];
        let provider = FallbackProvider::new(members);
        let result = provider.complete(dummy_req()).await;
        assert!(result.is_err(), "expected Err from terminal failure");
        // mock_b's outcome should be untouched
        let remaining = mock_b.outcomes.lock().expect("mutex").len();
        assert_eq!(remaining, 1, "mock_b should not have been called");
    }
}
