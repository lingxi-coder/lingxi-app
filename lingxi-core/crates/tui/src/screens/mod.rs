//! Screens — single top-level views composed from the `components/` module.
//!
//! M6-02 shipped only `repl::ReplScreen`. M7-11 establishes the screen-overlay
//! route-state (`Screen` + `AppState.active_screen`); later sub-plans add
//! `Resume` (M7-12), `Settings` (M7-13), `Memory` (M7-14).

pub mod doctor;
pub mod repl;

/// Which full-page screen currently overlays the REPL. `None` ⇒ REPL is live.
/// Established by M7-11; M7-12/13/14 add `Resume`/`Settings`/`Memory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The diagnostic screen opened by `/doctor`.
    Doctor,
    // Resume,   // M7-12
    // Settings, // M7-13
    // Memory,   // M7-14
}
