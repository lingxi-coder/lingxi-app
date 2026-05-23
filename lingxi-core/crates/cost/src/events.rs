//! Cost-event emission. M3-05 hooks the api-client response success path here
//! to fire `tengu_cost_recorded` with a byte-aligned payload.
//!
//! See spec §1 goal #5, §4 Flow B (lines 360-380), §7 cost-events table
//! (lines 730-745), and §8 M3-05 phase list (lines 890-905).

#![forbid(unsafe_code)]

/// Stub: Task 2 will replace with the real signature + body.
#[allow(clippy::too_many_arguments, clippy::missing_const_for_fn)]
pub fn emit_cost_recorded() {}
