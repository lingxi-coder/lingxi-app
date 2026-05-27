//! Streaming turn loop. Filled in Tasks 9-13.
//!
//! ## `StreamingError` → `OrchestratorError` mapping
//!
//! Each [`crate::sse::StreamingError`] variant is converted to
//! [`crate::error::OrchestratorError::StreamingProtocol`] via its
//! `Display` impl. The Display strings (locked at Task 1 step 3) are
//! the public-facing reason carried in the orchestrator error and
//! visible in telemetry payloads.
#![forbid(unsafe_code)]
