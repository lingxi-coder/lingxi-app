//! Screens — single top-level views composed from the `components/` module.
//!
//! M6-02 shipped only `repl::ReplScreen`. M7-11 establishes the screen-overlay
//! route-state (`Screen` + `AppState.active_screen`); later sub-plans add
//! `Resume` (M7-12), `Settings` (M7-13), `Memory` (M7-14).

pub mod background_tasks;
pub mod doctor;
pub mod memory;
pub mod repl;
pub mod resume;
pub mod settings;
pub mod theme;

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
///
/// (M7-13) `Eq` is dropped (kept `PartialEq`): the `Settings` variant carries a
/// `SettingsData` snapshot whose `StatusSnapshot`/`CostSnapshot` hold `f64`
/// cost fields, which do not implement `Eq`. No code uses `Screen` as a
/// `HashSet`/`HashMap` key, so `Eq` is unused; the existing `assert_eq!` /
/// `matches!` sites need only `PartialEq` + `Debug`.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// The diagnostic screen opened by `/doctor`, carrying its captured
    /// diagnostics.
    Doctor(doctor::DoctorDiagnostics),
    /// (M7-12) The Resume picker, carrying its own rows + selection state.
    /// The FIRST screen with interactive keys (Up/Down select, Enter resume):
    /// `root::handle_screen_key` dispatches per-variant so this arm runs the
    /// pure `resume::handle_resume_key` while Doctor stays read-only.
    Resume(resume::ResumeState),
    /// (M7-13) The Settings screen — a Config/Settings/Status/Usage tab
    /// overlay carrying its tab + read-once data snapshot. Interactive like
    /// Resume: `root::handle_screen_key` runs the pure `settings::apply_settings_key`
    /// (Left/Right/Tab cycle, Esc/`q` close). Surface-only — reads real M3
    /// settings + status/cost, writes ONLY via the `edit_config_file` handoff
    /// (§4 R7 — no inline mutation).
    Settings(settings::SettingsState),
    /// (M7-14) The Memory file editor — pick a CLAUDE.md tier, edit it
    /// inline, save through the M3 store. Carries its own selector/edit
    /// state. Interactive like Resume/Settings: `root::handle_screen_key`
    /// runs the pure `memory::handle_memory_key` (↑/↓ select, Enter edit,
    /// Ctrl-S save, Esc back/close). Reads via the M3 loader; writes back
    /// to the SAME `HierarchyEntry.path` atomically (§4 R7 — no new
    /// persistence layer).
    Memory(memory::MemoryScreenState),
    /// (M7-15) The theme picker — claude-code `ThemePicker.tsx`. Carries its
    /// own highlight + restore-on-cancel state. Interactive like
    /// Resume/Settings/Memory: `root::handle_screen_key` runs the pure
    /// `theme::theme_picker_handle_key` (↑/↓ live-preview, Enter commit + persist,
    /// Esc/`q` cancel-restore). Up/Down LIVE-PREVIEW the highlighted theme by
    /// writing `AppState.theme` directly (the whole UI re-renders); Enter
    /// commits via `set_theme` + best-effort persist; Esc restores the prior
    /// setting. Persists through the existing `~/.claude/settings.json` `theme`
    /// field (§4 R7 — best-effort, session-only on failure).
    Theme(theme::ThemePickerState),
    /// (M9-05) The background-tasks dialog — claude-code
    /// `BackgroundTasksDialog.tsx`. Carries its own list↔detail state
    /// (selection + mode + the open task's output tail). Interactive like
    /// Resume/Settings/Memory/Theme: `root::handle_screen_key` runs the pure
    /// `background_tasks::handle_background_tasks_key` (↑/↓ move, Enter open
    /// detail, Esc/`q` close; in detail Esc/`←` returns to the list). Opened
    /// from normal editing by Shift+Down; the task list it browses lives in
    /// `AppState.multiagent.tasks` (driven by the M9-05 MultiAgent pump).
    BackgroundTasks(background_tasks::BackgroundTasksState),
}
