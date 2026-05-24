//! `tengu_*` event schemas — the single authoritative source.
//!
//! Spec §7 line 752-792. Eight sub-modules, one per category. Every payload
//! struct uses `#[serde(deny_unknown_fields)]`; every payload enum uses
//! `#[non_exhaustive]`; every user-derived string field uses [`Verified`]
//! or [`PiiTagged`] (not bare `String`).
//!
//! [`Verified`]: crate::pii::Verified
//! [`PiiTagged`]: crate::pii::PiiTagged

pub mod agent;
pub mod api;
pub mod cost;
pub mod memory;
pub mod oauth;
pub mod session;
pub mod settings;
pub mod tool;

/// Flat list of every `tengu_*` event name in registration order:
/// api → agent → session → tool → cost → oauth → memory → settings.
///
/// Tasks 2-9 each append their category in registration order. The list is
/// re-exported at crate root as `lingxi_telemetry::ALL_EVENT_NAMES` (no — kept
/// inside the `tengu` module so the path `lingxi_telemetry::tengu::ALL_EVENT_NAMES`
/// remains the single source of truth; see `parity_tengu_events.rs`).
pub const ALL_EVENT_NAMES: &[&str] = {
    const fn concat_all() -> [&'static str; 25] {
        // const-fn concat is awkward; we use a manually-flattened array.
        // Each task adds 25 / 30 / 15 / 40 / 10 / 8 / 12 / 5 entries here.
        let mut out: [&'static str; 25] = [""; 25];
        let mut i = 0;
        while i < api::NAMES.len() {
            out[i] = api::NAMES[i];
            i += 1;
        }
        out
    }
    &concat_all()
};

// Audit-macro invocation: Task 12 replaces the stub body with the real walker.
lingxi_telemetry_macros::tengu_event_audit!();
