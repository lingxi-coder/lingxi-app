//! Multi-provider LLM backend.
//!
//! Defines a provider abstraction (`LlmProvider`) whose translation core is a
//! pure `WireCodec` (encode request / decode response) plus a stateful
//! `SseDecoder`. Each provider normalizes to the canonical Anthropic-shaped
//! types in `api_client::types`, so the rest of the engine is unchanged.
//!
//! See `docs/superpowers/specs/2026-06-01-llm-providers-design.md`.
#![forbid(unsafe_code)]

pub mod anthropic;
pub mod auth;
pub mod capabilities;
pub mod client;
pub mod codec;
pub mod error;
pub mod gemini;
pub mod model_spec;
/// `OpenAI` / OpenAI-compatible chat-completions codec.
pub mod openai;
pub mod profile;
pub mod provider;
pub mod registry;
pub mod request;

#[cfg(test)]
mod testutil;

pub use anthropic::AnthropicLlmProvider;
pub use auth::Auth;
pub use capabilities::{Capabilities, ReasoningSupport, SystemStyle};
pub use client::GenericClient;
pub use codec::{SseDecoder, WireCodec};
pub use error::CodecError;
pub use model_spec::{ModelSpec, DEFAULT_PROFILE};
pub use openai::OpenAiCodec;
pub use profile::{builtin_profiles, parse_profiles, ProviderKind, ProviderProfile};
pub use provider::LlmProvider;
pub use registry::{ModelRouter, ProviderRegistry, Resolved};
pub use request::{CanonicalRequest, DEFAULT_MAX_TOKENS};
