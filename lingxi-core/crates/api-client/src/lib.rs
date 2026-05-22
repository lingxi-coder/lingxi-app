//! Anthropic / OpenAI-compatible API client.
//!
//! All network I/O routes through `lingxi_traits::HttpTransport`. The client
//! itself is purely about request shape + SSE parsing + retry policy.

#![forbid(unsafe_code)]

pub mod anthropic;
pub mod error;
pub mod sse;
pub mod types;

// Re-exports below are uncommented in Tasks 14-15 once the modules ship real types.
// pub use anthropic::AnthropicProvider;
// pub use error::ApiError;
// pub use types::{ContentDelta, MessageRequest, MessageResponse, StreamEvent};
