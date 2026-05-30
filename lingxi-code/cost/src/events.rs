//! Cost-event emission. M3-05 hooks the api-client response success path here
//! to fire `tengu_cost_recorded` with a byte-aligned payload.
//!
//! See spec §1 goal #5, §4 Flow B (lines 360-380), §7 cost-events table
//! (lines 730-745), and §8 M3-05 phase list (lines 890-905).
//!
//! **Field-name notice**: the payload key `cost_usd` is the spec-locked name
//! but the value is **nano-USD per v3 §17** (cost storage is `u64` nano-USD
//! everywhere — no `f64` in the accumulating path). Downstream sinks (`BigQuery`,
//! Statsig) must divide by 10^9 to render dollars.

#![forbid(unsafe_code)]

use protocol::SessionId;
use std::sync::Arc;
use telemetry::{AnalyticsBus, AnalyticsValue, LogEventMetadata, Verified};

/// Event name (locked byte-for-byte; matches spec §7 line 734 and claude-code @ 6a25909).
pub const EVENT_NAME_COST_RECORDED: &str = "tengu_cost_recorded";

/// Emit a `tengu_cost_recorded` event for one successful API call.
///
/// Called from `lingxi-api-client::anthropic::messages_create_non_stream` at
/// the end of the 200-response success path (after `tengu_api_request_succeeded`
/// per spec §4 Flow B lines 364-376).
///
/// All eight payload keys are spec-locked (see spec §7 line 734). The field
/// order on the wire is `HashMap` insertion order via `LogEventMetadata`; the
/// presence + value of each key is asserted in `events_emission_test.rs` and
/// the parity fixture `cost_events.json` (Task 9) locks the byte-aligned JSON.
///
/// # Field reference
///
/// - `model` — wrapped in `Verified::assert_safe` (caller-validated model name; non-PII).
/// - `input_tokens` / `output_tokens` — Anthropic Messages API `UsageApi`.
/// - `cache_read_input_tokens` / `cache_creation_input_tokens` — Anthropic prompt-caching counters.
/// - `cost_usd` — **nano-USD** (per v3 §17); the key name says "usd" for spec parity, the value is nano-USD.
/// - `session_id` — `SessionId::to_string()` (`UUIDv4`; non-PII); wrapped in `Verified::assert_safe`.
/// - `is_batch_request` — ALWAYS `false` in M3. M4 sets `true` when the `/v1/messages/batches` endpoint fires.
#[allow(clippy::too_many_arguments)]
pub async fn emit_cost_recorded(
    bus: &Arc<AnalyticsBus>,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    cost_nano_usd: u64,
    session_id: &SessionId,
    is_batch_request: bool,
) {
    let mut metadata = LogEventMetadata::new();
    metadata.insert(
        "model".into(),
        AnalyticsValue::String(Verified::assert_safe(model.to_string()).into_inner()),
    );
    metadata.insert(
        "input_tokens".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(input_tokens)),
    );
    metadata.insert(
        "output_tokens".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(output_tokens)),
    );
    metadata.insert(
        "cache_read_input_tokens".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(cache_read_input_tokens)),
    );
    metadata.insert(
        "cache_creation_input_tokens".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(cache_creation_input_tokens)),
    );
    metadata.insert(
        "cost_usd".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(cost_nano_usd)),
    );
    metadata.insert(
        "session_id".into(),
        AnalyticsValue::String(Verified::assert_safe(session_id.to_string()).into_inner()),
    );
    metadata.insert(
        "is_batch_request".into(),
        AnalyticsValue::Bool(is_batch_request),
    );

    bus.log_event(EVENT_NAME_COST_RECORDED, metadata).await;
}

/// Saturate a `u64` into the `i64` value range used by `AnalyticsValue::Int`.
///
/// In practice token counts and nano-USD totals never exceed `i64::MAX`
/// (~9.2e18), but the saturation guards against the pathological `u64::MAX`
/// case so `as i64` would not produce a confusing negative number.
#[inline]
#[allow(clippy::cast_possible_wrap)]
const fn i64_from_u64_saturating(v: u64) -> i64 {
    if v > i64::MAX as u64 {
        i64::MAX
    } else {
        v as i64
    }
}
