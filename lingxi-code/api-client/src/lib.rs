//! Anthropic / OpenAI-compatible API client.
//!
//! All network I/O routes through `traits::HttpTransport`. The client
//! itself is purely about request shape + SSE parsing + retry policy. M3-03
//! extends with non-streaming `messages.create` + `count_tokens`, retry
//! middleware (3 attempts: 500ms / 1s / 2s ± 20% jitter), rate-limit
//! awareness, and a frozen `OAuthRefreshHook` trait surface that M3-04
//! implements without modifying this crate.

#![forbid(unsafe_code)]

pub mod anthropic;
pub mod betas;
pub mod error;
pub mod oauth_hook;
pub mod opus;
pub mod overflow;
pub mod rate_limit;
pub mod retry;
pub mod sse;
pub mod types;

pub use anthropic::AnthropicProvider;
pub use error::ApiError;
pub use opus::is_non_custom_opus;
pub use retry::{with_retry_ctl, RetryControl, MAX_529_RETRIES};
pub use oauth_hook::{
    register_oauth_hook, BearerToken, MiddlewareError, NoOpOAuthHook, OAuthHookError,
    OAuthRefreshHook, TokenHash,
};
pub use types::{ContentDelta, MessageRequest, MessageResponse, StreamEvent};
