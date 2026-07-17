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
pub mod otel;
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

/// Emit the `tengu_cd_command` event (claude-code 2.1.207's `/cd` move:
/// `N("tengu_cd_command", { source: xe(t) })`). `source` is the pre-hash
/// trigger label (`"cd_command"` for the `/cd` slash path); the sink applies
/// any PII hashing on the wire, matching the reference's `xe(t)` = hash-strip.
/// The event name is intentionally the flat `tengu_cd_command` (NOT part of the
/// `tengu_command_<name>_<phase>` family in [`crate::tengu::command`]).
pub fn emit_cd_command(source: &str) {
    tracing::info!(event = crate::tengu::command::CD_COMMAND, source = %source);
}

/// Emit `tengu_workflow_saved` — a dynamic workflow was saved via the
/// `/workflows` "Save dynamic workflow" dialog (oracle `eya`:
/// `M("tengu_workflow_saved", { scope, overwrite, script_size_chars })`).
/// `scope` is the low-cardinality wire enum (`"project"` / `"user"`);
/// `script_size_chars` is the script's `.length` (UTF-16 code units).
pub fn emit_workflow_saved(scope: &str, overwrite: bool, script_size_chars: usize) {
    tracing::info!(
        event = crate::tengu::workflow::SAVED,
        scope = %scope,
        overwrite = overwrite,
        script_size_chars = script_size_chars,
    );
}

// -- EXPERIMENTAL_SKILL_SEARCH skill-discovery emit helper -------------------

/// Emit the skill-discovery-collected event with the `hidden_by_main_turn` field
/// (claude-code `query.ts:1617`: `true` when the per-iteration skill-discovery
/// prefetch resolved BEFORE collection — i.e. it hid under the main turn's
/// streaming + tool execution; expected >98%). The sole emitter of this field;
/// fired by the orchestrator's `skill_discovery_reminder_message` on the gated
/// (flag-ON) path only — inert (never reached) in the default-OFF build.
///
/// FAITHFULNESS: the FIELD `hidden_by_main_turn` is BYTE-FAITHFUL (canonical, from
/// `query.ts:1617`). The EVENT NAME below is `[RECONSTRUCTED]` — the real
/// `logEvent('…')` call lives in the DCE'd `services/skillSearch/prefetch.js`
/// body and is NOT recoverable from the 2.1.195 binary or the readable bundle
/// (0 hits for `tengu_skill_discovery_collected` in both); guessed via the
/// `tengu_*` convention. Zero observable bytes (feature OFF by default); swap the
/// literal if/when `services/skillSearch/` is recovered.
pub fn emit_skill_discovery_collected(hidden_by_main_turn: bool) {
    tracing::info!(
        // [RECONSTRUCTED — event name not in 2.1.195 / bundle; guessed by tengu_ convention]
        event = "tengu_skill_discovery_collected",
        hidden_by_main_turn = hidden_by_main_turn,
    );
}

/// Emit `tengu_retention_sweep` — the on-disk data-retention housekeeping event
/// (claude-code `fWu`). `skipped` with a `skip_reason` records a no-op run;
/// otherwise the counts describe what was removed. The port sweeps the
/// session-file dirs (`todos`/`statsig`/`logs`) only, so `transcripts_deleted`
/// is reported for field-parity but is `0`.
#[allow(clippy::too_many_arguments)]
pub fn emit_retention_sweep(
    skipped: bool,
    skip_reason: Option<&str>,
    transcripts_deleted: u64,
    session_files_deleted: u64,
    errors: u64,
    period_days: u64,
    used_default: bool,
) {
    tracing::info!(
        event = "tengu_retention_sweep",
        skipped = skipped,
        skip_reason = skip_reason.unwrap_or(""),
        transcripts_deleted = transcripts_deleted,
        session_files_deleted = session_files_deleted,
        errors = errors,
        period_days = period_days,
        used_default = used_default,
    );
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

/// Emit `tengu_loop_ended` with the raw `reason` literal and the optional
/// `via_keepalive` extra.
///
/// PARITY: binary `Vst(e,t){W("tengu_loop_ended",{reason:Le(e),...t})}` where
/// `Le()` is identity — so `reason` is the verbatim literal (`gate_off` |
/// `model_stopped` | `aged_out` | `user_abort`). The `...t` spread carries
/// per-reason extras: `gate_off` has none (`via_keepalive = None`), while
/// `model_stopped`/`aged_out` from the keepalive path carry `via_keepalive`.
pub fn emit_loop_ended(reason: &str, via_keepalive: Option<bool>) {
    match via_keepalive {
        Some(v) => tracing::info!(
            event = crate::tengu::kairos::LOOP_ENDED,
            reason = %reason,
            via_keepalive = v,
        ),
        None => tracing::info!(event = crate::tengu::kairos::LOOP_ENDED, reason = %reason),
    }
}

/// Emit `tengu_push_notification_send`.
///
/// PARITY: binary `PushNotification` tool `call` — `W("tengu_push_notification_send",
/// {message_length, push_sent, local_sent, is_remote, disabled_reason})`. The
/// `disabled_reason` is the raw literal (`config_off` | `user_present` |
/// `no_transport`) or `""` on the success path (binary `Mo(p)` is identity,
/// `undefined` for success).
pub fn emit_push_notification_send(
    message_length: usize,
    push_sent: bool,
    local_sent: bool,
    is_remote: bool,
    disabled_reason: &str,
) {
    tracing::info!(
        event = crate::tengu::kairos::PUSH_NOTIFICATION_SEND,
        message_length = message_length,
        push_sent = push_sent,
        local_sent = local_sent,
        is_remote = is_remote,
        disabled_reason = %disabled_reason,
    );
}

/// Emit `tengu_loop_keepalive_fired`.
///
/// PARITY: binary `cKi` keepalive branch —
/// `W("tengu_loop_keepalive_fired",{clamped_delay_seconds:d,
/// prompt_is_sentinel:Z4d.isLoopDefaultSentinel(t)})`.
pub fn emit_loop_keepalive_fired(clamped_delay_seconds: u64, prompt_is_sentinel: bool) {
    tracing::info!(
        event = crate::tengu::kairos::LOOP_KEEPALIVE_FIRED,
        clamped_delay_seconds = clamped_delay_seconds,
        prompt_is_sentinel = prompt_is_sentinel,
    );
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

/// Emit `tengu_uncompilable_ignore_pattern` with its `site`.
///
/// PARITY: binary helper `oeg(site, pattern)` fires
/// `N("tengu_uncompilable_ignore_pattern",{site:neg[site]})` after warn-logging
/// the un-compilable pattern. `site` is one of
/// [`crate::tengu::ignore_pattern::SITES`] (e.g.
/// [`crate::tengu::ignore_pattern::SITE_WORKTREEINCLUDE`]). The offending
/// pattern + compile error are surfaced by the caller's own `warn` log, not
/// this event's payload (matching CC, whose analytics payload is `{site}` only).
pub fn emit_uncompilable_ignore_pattern(site: &'static str) {
    tracing::info!(
        event = crate::tengu::ignore_pattern::UNCOMPILABLE_IGNORE_PATTERN,
        site = site,
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
