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
