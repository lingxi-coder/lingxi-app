//! Multi-provider LLM backend.
//!
//! Defines a provider abstraction (`LlmProvider`) whose translation core is a
//! pure `WireCodec` (encode request / decode response) plus a stateful
//! `SseDecoder`. Each provider normalizes to the canonical Anthropic-shaped
//! types in `api_client::types`, so the rest of the engine is unchanged.
//!
//! See `docs/superpowers/specs/2026-06-01-llm-providers-design.md`.
#![forbid(unsafe_code)]

pub mod auth;
pub mod capabilities;
pub mod error;
pub mod request;

pub use auth::Auth;
pub use capabilities::{Capabilities, ReasoningSupport, SystemStyle};
pub use error::CodecError;
pub use request::{CanonicalRequest, DEFAULT_MAX_TOKENS};
