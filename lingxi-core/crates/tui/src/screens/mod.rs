//! Screens — single top-level views composed from the `components/` module.
//!
//! M6-02 shipped only `repl::ReplScreen`. M7-11 establishes the screen-overlay
//! route-state (`Screen` + `AppState.active_screen`); later sub-plans add
//! `Resume` (M7-12), `Settings` (M7-13), `Memory` (M7-14).

pub mod doctor;
pub mod repl;
pub mod resume;

/// Which full-page screen currently overlays the REPL. `None` ⇒ REPL is live.
/// Established by M7-11; M7-12/13/14 add `Resume`/`Settings`/`Memory`.
///
/// (M7-11 review) Each variant CARRIES its own per-screen state inline (the
/// data lives in the variant, not in a parallel `AppState` field). This makes
/// the foundation a clean "add a variant carrying its state + a render arm +
/// (optional) a key arm" for M7-12/13/14, and lets `AppState::close_screen`
/// stay a single generic `active_screen = None` with no per-screen clear.
/// Carrying the (non-`Copy`) `DoctorDiagnostics` drops `Screen: Copy`; the
/// ≤2 live match sites borrow the variant (`match &st.active_screen`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// The diagnostic screen opened by `/doctor`, carrying its captured
    /// diagnostics.
    Doctor(doctor::DoctorDiagnostics),
    /// (M7-12) The Resume picker, carrying its own rows + selection state.
    /// The FIRST screen with interactive keys (Up/Down select, Enter resume):
    /// `root::handle_screen_key` dispatches per-variant so this arm runs the
    /// pure `resume::handle_resume_key` while Doctor stays read-only.
    Resume(resume::ResumeState),
    // Settings(SettingsState), // M7-13
    // Memory(MemoryState),     // M7-14
}
