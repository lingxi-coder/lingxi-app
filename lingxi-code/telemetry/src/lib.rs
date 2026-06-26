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
pub use feature_flags::{
    flag_bool, test_clear_flag, test_set_flag, FeatureFlagsClient, FeatureFlagsFetcher,
    FeatureValue,
};
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

// -- M5-10 emit helpers for the 6 batch-1 slash commands ---------------------
//
// Each command emits three lifecycle events: `_started`, `_completed`, and
// (on failure) `_failed`. The transport mirrors the M5-07 pattern: a
// `tracing::info!` line carrying the event name + structured fields. See
// `crate::tengu::command::*` for the locked event-name strings.

/// Emit a `tengu_command_<name>_started` event.
pub fn emit_command_started(event: &'static str) {
    tracing::info!(event = event);
}

/// Emit a `tengu_command_<name>_completed` event with optional structured
/// fields encoded as a single JSON string `details`.
pub fn emit_command_completed(event: &'static str, details: &str) {
    tracing::info!(event = event, details = %details);
}

/// Emit a `tengu_command_<name>_failed` event with the error string.
pub fn emit_command_failed(event: &'static str, error: &str) {
    tracing::error!(event = event, error = %error);
}

// -- Kairos (`/loop`) autonomous-loop emit helper ----------------------------

/// Emit `tengu_kairos_loop_persistent_activated` with the `variant` field.
///
/// PARITY: binary `pJr()` / `logAutonomousLoopActivation` (cc_all.txt:504950) —
/// `W("tengu_kairos_loop_persistent_activated",{variant:YIn()})`, where `variant`
/// is `isLoopPersistentPreambleEnabled()`. See [`crate::tengu::kairos`].
pub fn emit_loop_persistent_activated(variant: bool) {
    tracing::info!(
        event = crate::tengu::kairos::LOOP_PERSISTENT_ACTIVATED,
        variant = variant,
    );
}

/// Emit `tengu_loop_ended` with the raw `reason` literal.
///
/// PARITY: binary `Vst(e,t){W("tengu_loop_ended",{reason:Le(e),...t})}` where
/// `Le()` is identity — so `reason` is the verbatim literal (`gate_off` |
/// `model_stopped` | `aged_out` | `user_abort`). See [`crate::tengu::kairos`].
pub fn emit_loop_ended(reason: &str) {
    tracing::info!(event = crate::tengu::kairos::LOOP_ENDED, reason = %reason);
}

/// Emit `tengu_loop_dynamic_wakeup_scheduled`.
///
/// PARITY: binary `cKi` —
/// `{chosen_delay_seconds:Number.isFinite(e)?e:0, clamped_delay_seconds:d,
/// was_clamped:p, reason_length:o?.length??0, superseded_count:s}`.
/// `chosen_delay_seconds` is the RAW (possibly fractional) requested delay (not
/// rounded). `reason_length` is JS `String.length` = UTF-16 code units. The
/// port's single-shot `WakeupScheduler` has no multi-loop registry, so
/// `superseded_count` is always 0 (a floor, not faithful for reschedules).
pub fn emit_loop_dynamic_wakeup_scheduled(
    chosen_delay_seconds: f64,
    clamped_delay_seconds: u64,
    was_clamped: bool,
    reason_length: usize,
    superseded_count: u64,
) {
    tracing::info!(
        event = crate::tengu::kairos::LOOP_DYNAMIC_WAKEUP_SCHEDULED,
        chosen_delay_seconds = chosen_delay_seconds,
        clamped_delay_seconds = clamped_delay_seconds,
        was_clamped = was_clamped,
        reason_length = reason_length,
        superseded_count = superseded_count,
    );
}

// -- M5-14 Task 10: release-marker emit-once helpers -------------------------

/// Emit the release markers exactly once per process lifetime.
///
/// Guarded by `std::sync::Once` so re-constructing a `ConversationOrchestrator`
/// in long-running processes (e.g. tests, REPL) does not emit duplicate events.
/// Emits the v0.5.0 + v0.6.0 + v0.7.0 markers for upgrade-chain continuity and
/// the current v0.8.0 marker (all fire on the first
/// `ConversationOrchestrator::new*` after the binary starts).
pub fn emit_release_markers_once() {
    use std::sync::Once;

    static V0_5_0_ONCE: Once = Once::new();
    static V0_6_0_ONCE: Once = Once::new();
    static V0_7_0_ONCE: Once = Once::new();
    static V0_8_0_ONCE: Once = Once::new();

    V0_5_0_ONCE.call_once(|| {
        tracing::info!(
            event = crate::tengu::release::LINGXI_CORE_V0_5_0_RELEASED,
            version = "0.5.0",
        );
    });
    V0_6_0_ONCE.call_once(|| {
        tracing::info!(
            event = crate::tengu::release::LINGXI_CORE_V0_6_0_RELEASED,
            version = "0.6.0",
        );
    });
    V0_7_0_ONCE.call_once(|| {
        tracing::info!(
            event = crate::tengu::release::LINGXI_CORE_V0_7_0_RELEASED,
            version = "0.7.0",
        );
    });
    V0_8_0_ONCE.call_once(|| {
        tracing::info!(
            event = crate::tengu::release::LINGXI_CORE_V0_8_0_RELEASED,
            version = "0.8.0",
        );
    });
}

#[cfg(test)]
mod release_marker_tests {
    use super::*;

    #[test]
    fn emit_release_markers_once_is_idempotent_and_wires_v0_8_0() {
        // The helper is `Once`-guarded per release; calling it repeatedly must
        // not panic. The v0.8.0 marker is wired alongside v0.5.0/v0.6.0/v0.7.0.
        emit_release_markers_once();
        emit_release_markers_once();
        emit_release_markers_once();

        // The marker the helper emits is the registered, locked wire string.
        assert_eq!(
            crate::tengu::release::LINGXI_CORE_V0_8_0_RELEASED,
            "lingxi_core_v0_8_0_released"
        );
        assert!(
            crate::tengu::ALL_EVENT_NAMES
                .contains(&crate::tengu::release::LINGXI_CORE_V0_8_0_RELEASED),
            "the v0.8.0 marker the helper emits must be registered in ALL_EVENT_NAMES"
        );
    }
}
