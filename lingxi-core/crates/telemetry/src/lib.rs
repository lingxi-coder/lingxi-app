//! Telemetry subsystem: analytics bus, sink trait, PII marker newtypes,
//! feature-flag client, and a global killswitch.
//!
//! See spec §26 (Telemetry & Feature Flags). This crate intentionally has no
//! provider-specific code — adapters for `BigQuery`, `Statsig`, OTLP, etc. live
//! in platform crates and plug in via [`AnalyticsSink`] / [`FeatureFlagsFetcher`].

#![forbid(unsafe_code)]

pub mod bus;
pub mod feature_flags;
pub mod killswitch;
pub mod pii;
pub mod sink;

pub use bus::AnalyticsBus;
pub use feature_flags::{FeatureFlagsClient, FeatureFlagsFetcher, FeatureValue};
pub use killswitch::Killswitch;
pub use pii::{strip_proto_fields, PiiTagged, Verified};
pub use sink::{AnalyticsSink, AnalyticsValue, LogEventMetadata};
