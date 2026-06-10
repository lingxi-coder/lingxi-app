//! Reusable LLM provider communication client.
//!
//! This crate owns provider-neutral request/response types, configuration,
//! route construction, authentication, retry classification, redaction, and
//! per-call usage/cost estimation.

#![forbid(unsafe_code)]

pub mod auth;
pub mod anthropic;
pub mod cost;
#[allow(missing_docs)]
pub mod client;
pub mod error;
pub mod config;
pub mod credentials;
pub mod protocol;
#[allow(missing_docs)]
pub mod providers;
pub mod registry;
pub mod redaction;
#[allow(missing_docs)]
pub mod route;
pub mod retry;
pub mod sse;
pub mod transport;
pub mod types;

pub use auth::{ApiKeyAuthenticator, Authenticator, BearerAuthenticator};
pub use anthropic::normalize_anthropic_usage;
pub use client::{DefaultLlmClient, PreparedLlmCall};
pub use providers::{AnthropicMessagesCodec, GeminiCodec, OpenAiChatCodec};
pub use config::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, ModelProfile, PricingConfig,
    ProtocolFamily, ProviderProfile,
};
pub use credentials::{Credential, CredentialProvider, CredentialScope, EnvCredentialProvider, StaticCredentialProvider};
pub use cost::{CostEstimator, PricingCatalog, PricingPolicy, TokenPricing};
pub use error::LlmError;
pub use protocol::{
    validate_capabilities, ContentBlock, ContentDelta, LlmEvent, LlmRequest, LlmResponse,
    Message, MessageDeltaPayload, NoopStreamDecoder, ProviderRequest, ProviderResponse,
    RawStreamFrame, ResponseFormat, StreamDecoder, ToolChoice, ToolDeclaration, WireCodec,
};
pub use registry::{ModelListing, ModelRegistry, ResolvedRoute};
pub use redaction::Redactor;
pub use route::Route;
pub use retry::{ResponseMetadata, RetryDecision, RetryPolicy};
pub use sse::SseFrameSplitter;
pub use transport::{BoxFuture, FrameStream, StreamingResponse, Transport};
pub use types::{CostEstimate, PricingModelRef, ProviderId, ServerToolUsage, TokenUsage, Usage};
