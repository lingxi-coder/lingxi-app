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
/// Order is locked; the parity fixture `parity_tengu_events.json` asserts
/// byte-for-byte equality against this constant.
///
/// Populated in Tasks 2-9 (one task per category).
pub const ALL_EVENT_NAMES: &[&str] = &[];

// Audit-macro invocation: Task 12 replaces the stub body with the real walker.
lingxi_telemetry_macros::tengu_event_audit!();
