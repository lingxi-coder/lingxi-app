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
pub mod command;
pub mod cost;
pub mod memory;
pub mod oauth;
pub mod orchestrator;
pub mod release;
pub mod session;
pub mod settings;
pub mod tool;
pub mod tui;

/// Flat list of every `tengu_*` event name in registration order:
/// api → agent → session → tool → cost → oauth → memory → settings →
/// orchestrator → release.
///
/// Tasks 2-9 each append their category in registration order. The list is
/// re-exported at crate root as `lingxi_telemetry::ALL_EVENT_NAMES` (no — kept
/// inside the `tengu` module so the path `lingxi_telemetry::tengu::ALL_EVENT_NAMES`
/// remains the single source of truth; see `parity_tengu_events.rs`).
pub const ALL_EVENT_NAMES: &[&str] = {
    // post-M5-06: orchestrator block grew from 7 to 15 (+ 6 hook lifecycle
    // events + hook_http_skipped_ssrf + hook_timeout).
    // M5-07: session block grew 15 -> 18 (+3 session_appended/rotated/corrupted).
    // M5-08: session block grows 18 -> 20 (+2 session_resume_started/completed).
    // M5-10: + 18 command events (6 commands × 3 phases) -> 276 total.
    // M5-11: command block grows 18 -> 54 (+36 new events for 12 batch-2 commands)
    //        -> 312 total.
    // M5-13: orchestrator block grows 15 -> 17 (+2 REPL session started/ended)
    //        -> 314 total. (Note: baseline observed at M6-01 start was 315
    //        entries; the comments above understate by 1 — see audit log
    //        in the M6-01 plan execution.)
    // M6-01: TUI lifecycle events (+4 -> 319).
    // M6-03: +2 streaming render events (streaming_render_{started,ended})
    //        → tui block grows 4 → 6 → 321 total.
    // M6-05: +2 permission dialog events (permission_dialog_{shown,resolved})
    //        → tui block grows 6 → 8 → 323 total.
    const TOTAL: usize = 25 + 30 + 20 + 134 + 10 + 8 + 12 + 3 + 17 + 2 + 54 + 8;
    const fn concat_all() -> [&'static str; TOTAL] {
        let mut out: [&'static str; TOTAL] = [""; TOTAL];
        let mut idx = 0;
        let mut i = 0;
        while i < api::NAMES.len() {
            out[idx] = api::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < agent::NAMES.len() {
            out[idx] = agent::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < session::NAMES.len() {
            out[idx] = session::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < tool::NAMES.len() {
            out[idx] = tool::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < cost::NAMES.len() {
            out[idx] = cost::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < oauth::NAMES.len() {
            out[idx] = oauth::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < memory::NAMES.len() {
            out[idx] = memory::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < settings::NAMES.len() {
            out[idx] = settings::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < orchestrator::NAMES.len() {
            out[idx] = orchestrator::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < release::NAMES.len() {
            out[idx] = release::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < command::NAMES.len() {
            out[idx] = command::NAMES[i];
            idx += 1;
            i += 1;
        }
        let mut i = 0;
        while i < tui::NAMES.len() {
            out[idx] = tui::NAMES[i];
            idx += 1;
            i += 1;
        }
        out
    }
    &concat_all()
};

// Audit-macro invocation: Task 12 replaces the stub body with the real walker.
lingxi_telemetry_macros::tengu_event_audit!();
