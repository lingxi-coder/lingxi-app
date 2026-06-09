//! Reusable LLM provider communication client.
//!
//! This crate owns provider-neutral request/response types, configuration,
//! route construction, authentication seams, transport abstractions, retry
//! classification, redaction, and per-call usage/cost estimation.

#![forbid(unsafe_code)]

pub mod auth;
pub mod anthropic;
pub mod cost;
pub mod error;
pub mod config;
pub mod credentials;
pub mod protocol;
pub mod registry;
pub mod redaction;
pub mod transport;
pub mod retry;
pub mod types;

pub use auth::{ApiKeyAuthenticator, Authenticator, BearerAuthenticator};
pub use anthropic::normalize_anthropic_usage;
pub use config::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, ModelProfile, PricingConfig,
    ProtocolFamily, ProviderProfile,
};
pub use credentials::{Credential, CredentialProvider, CredentialScope, EnvCredentialProvider, StaticCredentialProvider};
pub use cost::{CostEstimator, PricingCatalog, PricingPolicy, TokenPricing};
pub use error::LlmError;
pub use protocol::{
    validate_capabilities, ContentBlock, LlmEvent, LlmRequest, LlmResponse, Message, PreparedBody,
    Protocol, RawResponse, RawStreamFrame, ResponseFormat, StreamDecoder, ToolChoice,
    ToolDeclaration,
};
pub use registry::{ModelListing, ModelRegistry, ResolvedRoute};
pub use redaction::Redactor;
pub use retry::{ResponseMetadata, RetryDecision, RetryPolicy};
pub use transport::PreparedRequest;
pub use types::{CostEstimate, PricingModelRef, ProviderId, ServerToolUsage, TokenUsage, Usage};
