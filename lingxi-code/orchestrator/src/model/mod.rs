//! Claude-code-parity model-layer policy modules.
//!
//! These modules port the pure decision and parsing helpers from `api-client`
//! onto `llm-client`'s provider-neutral types, keeping the retry driver
//! synchronous and unit-testable without a Tokio runtime.

pub mod betas;
pub mod count_tokens;
pub mod fallback;
pub mod overflow;
pub mod prompt_too_long;
pub mod rate_limit;
pub mod retry;
pub mod telemetry;
pub mod user_agent;
