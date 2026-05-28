//! Telemetry subsystem: analytics bus, sink trait, PII marker newtypes,
//! feature-flag client, killswitch, the `tengu_*` event schema tree, and
//! the standard sink implementations (`NoOp` / `InMemory` / Statsig).
//!
//! See spec §26 (Telemetry & Feature Flags) + §7 (Telemetry schema, M3-06).
//! This crate is the **single authoritative source** for `tengu_*` event
//! wire shapes; provider-specific adapters (`BigQuery`, real Statsig SDK,
//! OTLP) live in platform crates and plug in via [`AnalyticsSink`] /
//! [`FeatureFlagsFetcher`].

#![forbid(unsafe_code)]

pub mod bus;
pub mod error;
pub mod feature_flags;
pub mod killswitch;
pub mod pii;
pub mod sink;
pub mod sinks;
pub mod tengu;

pub use bus::{AnalyticsBus, OverflowPolicy};
pub use error::TelemetryError;
pub use feature_flags::{FeatureFlagsClient, FeatureFlagsFetcher, FeatureValue};
pub use killswitch::Killswitch;
pub use pii::{strip_proto_fields, PiiTagged, Verified};
pub use sink::{AnalyticsSink, AnalyticsValue, LogEventMetadata};
pub use sinks::{InMemorySink, MockStatsigSink, NoOpSink, RecordedEvent, StatsigSink};

// -- M5-07 emit helpers (`tracing::info!` transport, matching the M5-06 pattern) --

/// Convenience for [`crate::tengu::session::APPENDED`].
///
/// Emits a `tracing::info!` event named `tengu_session_appended` with the
/// structured fields used by the M5-06 hook telemetry path. Used by the
/// orchestrator's M5-07 JSONL append site.
pub fn emit_session_appended(session_id: &str, message_uuid: &str) {
    tracing::info!(
        event = crate::tengu::session::APPENDED,
        session_id = %session_id,
        message_uuid = %message_uuid,
    );
}

/// Convenience for [`crate::tengu::session::ROTATED`]. Reserved for M5-08 (resume).
pub fn emit_session_rotated(session_id: &str, bytes_before_rotation: u64) {
    tracing::info!(
        event = crate::tengu::session::ROTATED,
        session_id = %session_id,
        bytes_before_rotation = bytes_before_rotation,
    );
}

/// Convenience for [`crate::tengu::session::CORRUPTED`].
pub fn emit_session_corrupted(session_id: &str, error: &str) {
    tracing::error!(
        event = crate::tengu::session::CORRUPTED,
        session_id = %session_id,
        error = %error,
    );
}
