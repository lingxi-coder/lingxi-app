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
pub mod migration;
pub mod oauth;
pub mod orchestrator;
pub mod permission;
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
/// re-exported at crate root as `telemetry::ALL_EVENT_NAMES` (no — kept
/// inside the `tengu` module so the path `telemetry::tengu::ALL_EVENT_NAMES`
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
    // M6-09: release block grows 2 → 3 (+lingxi_core_v0_7_0_released);
    //        tui block grows 8 → 10 (+scroll_started/scroll_ended). The
    //        specced tengu_tui_key_pressed is deferred to M7 (no emit site),
    //        so M6-09 adds 3 (not the spec's stale §2.6 estimate of ~15/330).
    //        → 326 total.
    // M7-16: release block grows 3 → 4 (+lingxi_core_v0_8_0_released); tui
    //        block grows 10 → 13 (+screen_opened/screen_closed/search_opened —
    //        the §2.7 candidates with REAL emit sites). M7-01..M7-15 added 0
    //        events (every candidate was deferred to this audit). The deferred
    //        command_palette_opened / vim_mode_entered / key_pressed stay OUT
    //        (no clean/aggregated emit site → no dead names — the M6 lesson).
    //        → 330 total. Cumulative across releases:
    //          v0.4.0 (M3-06): 196 · v0.5.0 (M4-09): 238 ·
    //          v0.6.0 (M5-14): 315 · v0.7.0 (M6-09): 326 · v0.8.0 (M7-16): 330.
    // CronDelete/CronList: tool block grows 134 → 140 (+6, 2 tools × 3 stages)
    //        → 336 total.
    // FileReadTool analytics: +3 appended at the GLOBAL TAIL (tengu_file_read_dedup,
    //        tengu_session_file_read, tengu_file_read_limits_override) — NOT in the
    //        tool concat block (they are not tengu_tool_*) → 339 total. The trailing
    //        `+ 3` is `tool::FILE_READ_ANALYTICS_NAMES.len()`.
    // Config migrations: +9 (migration::NAMES, runMigrations port) → 348 total.
    // Bypass-permissions dialog: +1 (permission::NAMES) → 349 total.
    const TOTAL: usize = 25 + 30 + 20 + 140 + 10 + 8 + 12 + 3 + 17 + 4 + 54 + 13 + 3 + 9 + 1;
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
        // FileReadTool analytics names (`tengu_file_read_*` / `tengu_session_file_read`)
        // are appended at the GLOBAL TAIL — they are not `tengu_tool_*`, so keeping
        // them out of the tool concat block preserves every per-block prefix slice
        // in event_name_completeness_test::category_ordering_preserved. Positions
        // 336/337/338 (tengu_events.json fixture).
        let mut i = 0;
        while i < tool::FILE_READ_ANALYTICS_NAMES.len() {
            out[idx] = tool::FILE_READ_ANALYTICS_NAMES[i];
            idx += 1;
            i += 1;
        }
        // Config-migration block (tengu_migrate_* / model-migration markers) —
        // appended after the FileRead global tail. Positions 339..348.
        let mut i = 0;
        while i < migration::NAMES.len() {
            out[idx] = migration::NAMES[i];
            idx += 1;
            i += 1;
        }
        // Permission-flow block (bypass dialog accept) — appended after the
        // config-migration block. Position 348.
        let mut i = 0;
        while i < permission::NAMES.len() {
            out[idx] = permission::NAMES[i];
            idx += 1;
            i += 1;
        }
        out
    }
    &concat_all()
};

// Audit-macro invocation: Task 12 replaces the stub body with the real walker.
telemetry_macros::tengu_event_audit!();
