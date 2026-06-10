//! Claude-code-parity model-layer policy modules.
//!
//! These modules port the pure decision and parsing helpers from `api-client`
//! onto `llm-client`'s provider-neutral types, keeping the retry driver
//! synchronous and unit-testable without a Tokio runtime.

pub mod overflow;
pub mod retry;
