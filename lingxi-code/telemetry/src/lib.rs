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
    let attrs = std::iter::IntoIterator::into_iter([
        (
            "session_id".to_string(),
            crate::otel::AttrValue::from(session_id.to_string()),
        ),
        (
            "message_uuid".to_string(),
            crate::otel::AttrValue::from(message_uuid.to_string()),
        ),
    ])
    .collect();
    crate::otel::emit_named_log_event(crate::tengu::session::APPENDED, &attrs);
}

/// Convenience for [`crate::tengu::session::ROTATED`]. Reserved for M5-08 (resume).
pub fn emit_session_rotated(session_id: &str, bytes_before_rotation: u64) {
    tracing::info!(
        event = crate::tengu::session::ROTATED,
        session_id = %session_id,
        bytes_before_rotation = bytes_before_rotation,
    );
    let attrs = std::iter::IntoIterator::into_iter([
        (
            "session_id".to_string(),
            crate::otel::AttrValue::from(session_id.to_string()),
        ),
        (
            "bytes_before_rotation".to_string(),
            crate::otel::AttrValue::from(bytes_before_rotation as i64),
        ),
    ])
    .collect();
    crate::otel::emit_named_log_event(crate::tengu::session::ROTATED, &attrs);
}

/// Convenience for [`crate::tengu::session::CORRUPTED`].
pub fn emit_session_corrupted(session_id: &str, error: &str) {
    tracing::error!(
        event = crate::tengu::session::CORRUPTED,
        session_id = %session_id,
        error = %error,
    );
    let attrs = std::iter::IntoIterator::into_iter([
        (
            "session_id".to_string(),
            crate::otel::AttrValue::from(session_id.to_string()),
        ),
        (
            "error".to_string(),
            crate::otel::AttrValue::from(error.to_string()),
        ),
    ])
    .collect();
    crate::otel::emit_named_log_event(crate::tengu::session::CORRUPTED, &attrs);
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

/// Emit `tengu_workflow_size_warning_shown` — the `/workflows` UI displayed the
/// large-workflow warning for a run.
pub fn emit_workflow_size_warning_shown(
    axis: &str,
    scheduled_agents: u64,
    total_tokens: u64,
    projected_tokens: u64,
    agent_cap: f64,
    token_cap: f64,
    cap_from_guideline: bool,
) {
    tracing::info!(
        event = crate::tengu::workflow::SIZE_WARNING_SHOWN,
        axis = %axis,
        scheduled_agents = scheduled_agents,
        total_tokens = total_tokens,
        projected_tokens = projected_tokens,
        agent_cap = agent_cap,
        token_cap = token_cap,
        cap_from_guideline = cap_from_guideline,
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

/// Emit `tengu_auto_mode_denial_limit_exceeded` — claude-code's `dSm` trip
/// telemetry, fired when the auto-mode classifier denial breaker trips
/// (consecutive or total limit). `mode` is `"headless"`/`"cli"`
/// (`DenialBreakerTrip::mode_tag`); the counts and blocking tool round out the
/// tags. Byte-1:1 event name; the permission gate calls this on a trip.
pub fn emit_auto_mode_denial_limit_exceeded(
    mode: &str,
    consecutive_denials: u32,
    total_denials: u32,
    tool_name: &str,
) {
    tracing::info!(
        event = crate::tengu::agent::AUTO_MODE_DENIAL_LIMIT_EXCEEDED,
        mode = mode,
        consecutive_denials = consecutive_denials,
        total_denials = total_denials,
        tool_name = tool_name,
    );
}

/// Emit `auto_mode_setup_write` — the WIZARD-06 non-interactive
/// `auto-mode-setup --apply-file` write attempt (oracle `pe("auto_mode_setup_write",
/// {code})`). `code` is the outcome (`usage` / `bad_flag_grammar` / `bad_path` /
/// `read_denied` / `read_failed` / `too_large` / `missing_hash_arg` /
/// `bad_hash_arg` / `hash_mismatch` / `parse_failed` / `scope_mismatch` /
/// `write_failed`). NOTE the event name has NO `tengu_` prefix (byte-exact).
pub fn emit_auto_mode_setup_write(code: &str) {
    tracing::info!(
        event = crate::tengu::agent::AUTO_MODE_SETUP_WRITE,
        code = %code,
    );
}

/// Emit `auto_mode_setup_propose` — the WIZARD-06 propose run's outcome
/// (oracle `pe("auto_mode_setup_propose", {code})`). `code` is the failure code
/// (`recon_failed` / `api_failed` / `truncated` / `refused` / `unexpected_stop` /
/// `parse_failed` / `invalid_proposal` / `unknown_removal`) or the qualified
/// success code (`parse_repaired` / `unsafe_allow_dropped`).
///
/// `aborted` is deliberately NOT recorded: the oracle skips the emit when the
/// user cancelled, so a cancellation never shows up as a failure rate.
/// NOTE the event name has NO `tengu_` prefix (byte-exact).
pub fn emit_auto_mode_setup_propose(code: &str) {
    tracing::info!(
        event = crate::tengu::agent::AUTO_MODE_SETUP_PROPOSE,
        code = %code,
    );
}

/// Emit `auto_mode_pregather` — a WIZARD-06 recon producer degraded (oracle
/// `Ne("auto_mode_pregather", {code})`). `code` names what fell short
/// (`visibility_gh_failed` / `org_list_gh_parse_failed` / …).
///
/// A MISSING or unauthenticated `gh` is deliberately not reported through
/// here: the caller filters those out first, so this measures real breakage
/// rather than how many users lack the tool.
/// NOTE the event name has NO `tengu_` prefix (byte-exact).
pub fn emit_auto_mode_pregather(code: &str) {
    tracing::info!(
        event = crate::tengu::agent::AUTO_MODE_PREGATHER,
        code = %code,
    );
}

/// Emit `tengu_auto_mode_setup_wizard_resolved` — the `/auto-mode-setup` wizard
/// finished. `choice` records how it ended (`applied` / `cancelled` / …).
pub fn emit_auto_mode_setup_wizard_resolved(choice: &str) {
    tracing::info!(
        event = crate::tengu::agent::AUTO_MODE_SETUP_WIZARD_RESOLVED,
        choice = %choice,
    );
}

/// Emit `tengu_agent_hooks_origin_untrusted` (cc 2.1.218 `hvo`) — an agent
/// definition's frontmatter `hooks:` were skipped because the folder the
/// definition came from has not been trusted. `source` is the claude
/// `SettingSource` string, `surface` is `"subagent"` or `"mainThread"`, and
/// `from_additional_directory` reports whether the definition came from an
/// `--add-dir` directory (serialized as the string `"true"`/`"false"`, matching
/// the binary's `me(... ? "true" : "false")`).
pub fn emit_agent_hooks_origin_untrusted(
    source: &str,
    surface: &str,
    from_additional_directory: bool,
) {
    tracing::info!(
        event = crate::tengu::agent::AGENT_HOOKS_ORIGIN_UNTRUSTED,
        source = source,
        surface = surface,
        fromAdditionalDirectory = if from_additional_directory {
            "true"
        } else {
            "false"
        },
    );
}

/// Emit `tengu_repair_double_escaped_unicode` (cc 2.1.218 `jYd`) — a tool_use
/// input had literal `\uXXXX` text repaired into real characters, and/or was
/// left verbatim because it looked like a Windows path.
pub fn emit_repair_double_escaped_unicode(repaired_strings: u32, windows_path_skips: u32) {
    tracing::info!(
        event = crate::tengu::tool::REPAIR_DOUBLE_ESCAPED_UNICODE,
        repaired_strings = repaired_strings,
        windows_path_skips = windows_path_skips,
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
