//! The runtime provider contract. Implemented by `AnthropicLlmProvider` and
//! (via a codec) `GenericClient`.

use crate::capabilities::Capabilities;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::BoxStream;

/// A model backend that can complete and stream the canonical request shape.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// The provider's identity (for cost attribution / telemetry).
    fn id(&self) -> ProviderId;

    /// What this provider can do.
    fn capabilities(&self) -> &Capabilities;

    /// Non-streaming completion.
    ///
    /// # Errors
    /// Returns [`ApiError`] on transport, auth, or decode failure.
    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError>;

    /// Streaming completion: yields canonical stream events until end-of-stream.
    ///
    /// # Errors
    /// Returns [`ApiError`] if the connection cannot be opened; per-event
    /// failures surface as `Err` items inside the stream.
    async fn stream(
        &self,
        req: CanonicalRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError>;
}
