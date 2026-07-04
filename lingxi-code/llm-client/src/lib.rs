//! Reusable LLM provider communication client.
//!
//! This crate owns provider-neutral request/response types, configuration,
//! route construction, authentication, retry classification, redaction, and
//! per-call usage/cost estimation.

#![forbid(unsafe_code)]

pub mod anthropic;
pub mod auth;
pub mod aws_auth;
pub mod catalog;
#[allow(missing_docs)]
pub mod client;
pub mod config;
pub mod convert;
pub mod copilot;
pub mod cost;
pub mod credentials;
pub mod error;
pub mod eventstream;
pub mod model;
pub mod oauth;
pub mod prompt_format;
pub mod protocol;
pub mod provider_settings;
#[allow(missing_docs)]
pub mod providers;
pub mod redaction;
pub mod registry;
pub mod retry;
#[allow(missing_docs)]
pub mod route;
#[allow(missing_docs)]
pub mod service;
pub mod sigv4;
pub mod sse;
pub mod ssl;
pub mod transport;
pub mod transport_bridge;
pub mod types;

pub use anthropic::normalize_anthropic_usage;
pub use auth::{ApiKeyAuthenticator, Authenticator, BearerAuthenticator, ChatGptAuthenticator};
pub use aws_auth::{
    AwsAuthProcess, AwsAuthRefresh, AwsAuthRefresher, AwsAuthSettings, AwsExportedCredentials,
    ShellAwsAuthProcess,
};
pub use catalog::{builtin_presets, BuiltinCatalog};
pub use client::{
    DefaultLlmClient, FileActivationPoll, LlmEventStream, PreparedLlmCall,
    ResponsesWebSocketRequestSnapshot, ResponsesWebSocketSession,
};
pub use config::{
    AuthStrategy, AzureConfig, Capabilities, ClientConfig, CredentialConfig, ModelProfile,
    PricingConfig, ProtocolFamily, ProviderProfile, SigningConfig,
};
pub use copilot::{
    CopilotAuthenticator, CopilotHttp, CopilotLogin, CopilotSecret, DeviceCodeResponse, PollOutcome,
};
pub use cost::{CostEstimator, PricingCatalog, PricingPolicy, TokenPricing};
pub use credentials::{
    CopilotExchangeCredentialProvider, Credential, CredentialProvider, CredentialScope,
    EnvCredentialProvider, StaticCredentialProvider,
};
pub use error::LlmError;
pub use eventstream::{crc32, EventStreamMessage, EventStreamSplitter};
pub use protocol::{
    stream_provider_metadata_from_headers, validate_capabilities, CacheControl, CacheEdit,
    CacheScope, ContentBlock, ContentDelta, LlmEvent, LlmRequest, LlmResponse, Message,
    MessageDeltaPayload, NoopStreamDecoder, OpenAiResponsesRequestOptions, ProviderRequest,
    ProviderResponse, ProviderStreamTransport, RawStreamFrame, ReasoningConfig, RequestMetadata,
    ResponseFormat, StopDetails, StreamDecoder, StreamFraming, SystemBlock, ToolChoice,
    ToolDeclaration, WireCodec,
};
pub use provider_settings::{
    anthropic_model_profiles, anthropic_provider_profile, parse_provider_profiles_lenient,
    parse_provider_profiles_strict, pricing_provider_id_for_profile, split_profile_model,
    ParsedUserProvider, ProviderCredentialMode, ProviderKind, ProviderParseOptions,
};
pub use providers::{
    AnthropicMessagesCodec, AzureOpenAiCodec, BedrockClaudeCodec, GeminiCodec, GeminiFile,
    OpenAiChatCodec, OpenAiResponsesCodec, VertexClaudeCodec, VertexGeminiCodec,
};
pub use redaction::Redactor;
pub use registry::{ModelListing, ModelRegistry, ResolvedRoute};
pub use retry::{ResponseMetadata, RetryDecision, RetryPolicy};
pub use route::Route;
pub use service::{ApiService, SubscriberState};
pub use sse::SseFrameSplitter;
pub use ssl::{detect_ssl_code, is_ssl_code, ssl_hint};
pub use transport::{
    BoxFuture, FrameStream, ResponsesWebSocketTransportSession, StreamingResponse, Transport,
};
pub use transport_bridge::{from_http, LlmTransportBridge};
pub use types::{CostEstimate, PricingModelRef, ProviderId, ServerToolUsage, TokenUsage, Usage};
