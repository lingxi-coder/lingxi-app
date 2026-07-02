//! Settings screen — Config / Settings / Status / Usage tab overlay (M7-13).
//!
//! A route-state screen (not a z-index overlay): when
//! `AppState.active_screen == Some(Screen::Settings(_))`, `render_screen`
//! returns this screen instead of the REPL, and `handle_live_key` routes
//! keys here at priority 2 (after the permission focus-trap, before input).
//! Interactive like the Resume picker (M7-12): `root::handle_screen_key`
//! bridges the live iocraft key to the pure [`apply_settings_key`] reducer.
//!
//! Reads REAL data from the M3 settings store (`engine::settings`) and
//! the orchestrator handle (`get_status_snapshot`, `snapshot_cost`). Writes
//! ONLY through the existing `OrchestratorHandle::edit_config_file` `$EDITOR`
//! handoff (§4 R7 — no new persistence/validation/schema logic; there is no
//! settings write API in `lingxi-*`, so the Config/Settings tabs are read-only
//! display surfaces). Usage shows FLAT cost (per-model breakdown is M8).
//!
//! # Open path & the `/config` `/status` command hook (DEFERRED to M7-16)
//!
//! The screen opens via [`crate::state::AppState::open_settings`] with a
//! pre-read [`SettingsData`] (built by the async [`SettingsData::snapshot`] in a
//! context that holds the `OrchestratorHandle` — the bridge/command path, NOT
//! the synchronous `app::dispatch`). M7-13 ships that seam (exercised by tests).
//!
//! The `/config`→Config-tab and `/status`→Status-tab command hooks are
//! **DEFERRED to M7-16**: unlike `/doctor` (whose `DoctorDiagnostics::capture`
//! is synchronous and handle-free, so it opens inline from the sync `dispatch`
//! `Submit` arm), `SettingsData::snapshot` needs an async `OrchestratorHandle`
//! call + a `Settings::load`, and the sync `dispatch` seam holds NEITHER the
//! handle nor async-color. Threading the handle into a new async submit seam is
//! a structural change beyond M7-13's surface-only scope, so the convenience
//! hook lands with the M7-16 screen-lifecycle/command cluster. The M5-11
//! `/config` `/status` handlers stay the `--no-tui` path, untouched.
#![forbid(unsafe_code)]

use std::sync::Arc;

use engine::settings::EffectiveSettings;
use iocraft::prelude::*;
use traits::{CostSnapshot, OrchestratorHandle, StatusSnapshot};

use crate::render_iocraft::StyleColorIocraftExt;
use crate::theme::TuiTheme;

pub mod config;
// (M7-13) The "Settings" tab module is intentionally named `settings` inside
// the `settings` screen module — the four sub-tabs are Config/Settings/Status/
// Usage and each is its own file. `module_inception` is the expected shape here.
#[allow(clippy::module_inception)]
pub mod settings;
pub mod status;
pub mod usage;

/// Which sub-screen is selected. Tab order is Config → Settings → Status →
/// Usage (`LingXi` order; see plan "Tab-order decision").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    /// Read-only effective config-file values + the `$EDITOR` handoff (`e`).
    Config,
    /// Read-only effective settings + per-field provenance.
    Settings,
    /// `/status` panel snapshot rows.
    Status,
    /// Flat cumulative session cost.
    Usage,
}

impl SettingsTab {
    /// All tabs in display order.
    #[must_use]
    pub fn all() -> [SettingsTab; 4] {
        // (settings-tab-order) claude-code order is Status → Config → Usage; the
        // Rust-only `Settings` provenance tab follows after Usage. The OPEN tab
        // is passed explicitly via `OpenSettings(tab)`, so `/status` and
        // `/config` still land on their tab regardless of this cycle order.
        [
            SettingsTab::Status,
            SettingsTab::Config,
            SettingsTab::Usage,
            SettingsTab::Settings,
        ]
    }

    /// Tab title literal (claude-code Settings.tsx tab titles).
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            SettingsTab::Config => "Config",
            SettingsTab::Settings => "Settings",
            SettingsTab::Status => "Status",
            SettingsTab::Usage => "Usage",
        }
    }

    /// Next tab with wrap-around (Right/`l`/Tab).
    #[must_use]
    pub fn next(self) -> SettingsTab {
        let all = SettingsTab::all();
        let i = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }

    /// Previous tab with wrap-around (Left/`h`/`BackTab`).
    #[must_use]
    pub fn prev(self) -> SettingsTab {
        let all = SettingsTab::all();
        let i = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(i + all.len() - 1) % all.len()]
    }
}

/// Immutable snapshot read once when the screen opens. Keeps the render
/// path synchronous + pure (no `.await` in iocraft's render callback). Built
/// by [`SettingsData::snapshot`] in the async open path.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsData {
    /// Effective merged settings + per-field provenance (M3 read API). Carries
    /// the whole [`EffectiveSettings`] so the Settings tab can surface
    /// `effective_for(field)` provenance without a second read.
    pub effective: EffectiveSettings,
    /// `/status` panel snapshot (the rich `traits::StatusSnapshot`).
    pub status: StatusSnapshot,
    /// Cumulative cost (FLAT — no per-model field; per-model is M8).
    pub cost: CostSnapshot,
}

impl SettingsData {
    /// Read every datum the screen displays. Called ONCE on open (and after
    /// the `$EDITOR` handoff) so the render path stays synchronous.
    ///
    /// `eff` is the already-loaded effective settings (the caller loads it via
    /// `engine::settings::Settings::load`, which is the ONLY M3 read API).
    /// Status + cost come from the handle.
    pub async fn snapshot(handle: &Arc<dyn OrchestratorHandle>, eff: EffectiveSettings) -> Self {
        let status = handle.get_status_snapshot().await;
        let cost = handle.snapshot_cost().await;
        SettingsData {
            effective: eff,
            status,
            cost,
        }
    }
}

/// UI state for the screen — owned inside `Screen::Settings`.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsState {
    /// Currently selected tab.
    pub tab: SettingsTab,
    /// The read-once data snapshot.
    pub data: SettingsData,
}

impl SettingsState {
    /// Open on a given tab with a pre-read data snapshot.
    #[must_use]
    pub fn new(tab: SettingsTab, data: SettingsData) -> Self {
        Self { tab, data }
    }
}

/// What [`apply_settings_key`] tells the router to do next. Mirrors the
/// Resume screen's `ResumeOutcome` (M7-12) — the screen owns its key
/// semantics; the router performs the `active_screen = None` on `Close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsOutcome {
    /// Keep the screen open (tab moved, or an inert key).
    Stay,
    /// Close the screen back to the REPL (`Esc`/`q`).
    Close,
    /// Open `$EDITOR` on `config.json` via `edit_config_file` (Config tab
    /// `e`/`Enter`), then re-snapshot. The async handoff runs on the bridge
    /// path — NEVER `.await` in the sync key callback (§4 R7; M6 rule).
    EditConfig,
}

/// Pure tab-navigation reducer. Mirrors the Resume screen's
/// `handle_resume_key` (M7-12): takes the bridged crossterm `KeyEvent` and
/// returns a [`SettingsOutcome`]; tab nav mutates `state.tab` in place.
///
/// - `Right`/`l`/`Tab` → next tab (wrap-around).
/// - `Left`/`h`/`BackTab` → previous tab (wrap-around).
/// - `Esc`/`q` → `Close`.
/// - `e`/`Enter` on the Config tab → `EditConfig` ($EDITOR handoff).
/// - anything else → `Stay`.
#[must_use]
pub fn apply_settings_key(
    state: &mut SettingsState,
    key: crossterm::event::KeyEvent,
) -> SettingsOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    match key.code {
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
            state.tab = state.tab.next();
            SettingsOutcome::Stay
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
            state.tab = state.tab.prev();
            SettingsOutcome::Stay
        }
        KeyCode::Esc | KeyCode::Char('q') => SettingsOutcome::Close,
        // The ONLY write path: the $EDITOR handoff on the Config tab (§4 R7).
        KeyCode::Char('e') | KeyCode::Enter
            if state.tab == SettingsTab::Config && key.modifiers == KeyModifiers::NONE =>
        {
            SettingsOutcome::EditConfig
        }
        _ => SettingsOutcome::Stay,
    }
}

/// Render the tab strip to a plain string (snapshot-testable). The selected
/// tab is bracketed, e.g. `[Config]  Settings  Status  Usage`.
#[must_use]
pub fn render_tab_strip(selected: SettingsTab) -> String {
    SettingsTab::all()
        .iter()
        .map(|t| {
            if *t == selected {
                format!("[{}]", t.title())
            } else {
                format!(" {} ", t.title())
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Props for the Settings screen container.
#[derive(Default, Props)]
pub struct SettingsScreenProps {
    /// The full screen state (tab + data snapshot). Cloned from `active_screen`.
    pub state: Option<SettingsState>,
}

/// Settings screen container — tab strip + the selected sub-screen.
#[component]
pub fn SettingsScreen(props: &SettingsScreenProps) -> impl Into<AnyElement<'static>> {
    let Some(state) = props.state.clone() else {
        return element! { View() }.into_any();
    };
    let strip = render_tab_strip(state.tab);
    let data = state.data.clone();
    let body = match state.tab {
        SettingsTab::Config => element! { config::ConfigTab(data: Some(data)) }.into_any(),
        SettingsTab::Settings => {
            element! { settings::SettingsTabView(data: Some(data)) }.into_any()
        }
        SettingsTab::Status => element! { status::StatusTab(data: Some(data)) }.into_any(),
        SettingsTab::Usage => element! { usage::UsageTab(data: Some(data)) }.into_any(),
    };
    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            Text(content: strip, color: TuiTheme::ASSISTANT.to_iocraft())
            View(margin_top: 1) {
                #(body)
            }
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use engine::settings::tracer::ProvenanceTrace;
    use engine::settings::SettingsJson;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn fixture_state(tab: SettingsTab) -> SettingsState {
        SettingsState {
            tab,
            data: SettingsData {
                effective: EffectiveSettings {
                    settings: SettingsJson::default(),
                    trace: ProvenanceTrace::default(),
                },
                status: StatusSnapshot::default(),
                cost: CostSnapshot::default(),
            },
        }
    }

    #[test]
    fn tab_cycles_with_wraparound() {
        // New order: Status → Config → Usage → Settings → Status.
        assert_eq!(SettingsTab::Status.next(), SettingsTab::Config);
        assert_eq!(SettingsTab::Config.next(), SettingsTab::Usage);
        assert_eq!(SettingsTab::Usage.next(), SettingsTab::Settings);
        assert_eq!(SettingsTab::Settings.next(), SettingsTab::Status);
        assert_eq!(SettingsTab::Config.prev(), SettingsTab::Status);
        assert_eq!(SettingsTab::Status.prev(), SettingsTab::Settings);
    }

    #[test]
    fn tab_titles_match_claude_code() {
        assert_eq!(SettingsTab::Config.title(), "Config");
        assert_eq!(SettingsTab::Settings.title(), "Settings");
        assert_eq!(SettingsTab::Status.title(), "Status");
        assert_eq!(SettingsTab::Usage.title(), "Usage");
    }

    #[test]
    fn tab_right_advances_with_wrap() {
        // Usage → Settings (wrap region) under the new order.
        let mut st = fixture_state(SettingsTab::Usage);
        let outcome = apply_settings_key(&mut st, k(KeyCode::Right));
        assert_eq!(st.tab, SettingsTab::Settings);
        assert_eq!(outcome, SettingsOutcome::Stay, "Tab nav must not close");
        // `l` and Tab alias Right: Config → Usage.
        let mut st2 = fixture_state(SettingsTab::Config);
        let _ = apply_settings_key(&mut st2, k(KeyCode::Char('l')));
        assert_eq!(st2.tab, SettingsTab::Usage);
        let mut st3 = fixture_state(SettingsTab::Config);
        let _ = apply_settings_key(&mut st3, k(KeyCode::Tab));
        assert_eq!(st3.tab, SettingsTab::Usage);
    }

    #[test]
    fn tab_left_retreats_with_wrap() {
        // Config → Status (prev) under the new order.
        let mut st = fixture_state(SettingsTab::Config);
        let _ = apply_settings_key(&mut st, k(KeyCode::Left));
        assert_eq!(st.tab, SettingsTab::Status);
        // `h` aliases Left: Settings → Usage.
        let mut st2 = fixture_state(SettingsTab::Settings);
        let _ = apply_settings_key(&mut st2, k(KeyCode::Char('h')));
        assert_eq!(st2.tab, SettingsTab::Usage);
    }

    #[test]
    fn esc_signals_close() {
        let mut st = fixture_state(SettingsTab::Status);
        let outcome = apply_settings_key(&mut st, k(KeyCode::Esc));
        assert_eq!(outcome, SettingsOutcome::Close);
        assert_eq!(st.tab, SettingsTab::Status, "tab unchanged on close");
        // `q` also closes.
        let mut st2 = fixture_state(SettingsTab::Status);
        assert_eq!(
            apply_settings_key(&mut st2, k(KeyCode::Char('q'))),
            SettingsOutcome::Close
        );
    }

    #[test]
    fn edit_config_only_on_config_tab() {
        // `e`/Enter on Config → EditConfig (the $EDITOR handoff, §4 R7).
        let mut st = fixture_state(SettingsTab::Config);
        assert_eq!(
            apply_settings_key(&mut st, k(KeyCode::Char('e'))),
            SettingsOutcome::EditConfig
        );
        assert_eq!(
            apply_settings_key(&mut st, k(KeyCode::Enter)),
            SettingsOutcome::EditConfig
        );
        // On other tabs `e`/Enter is inert (no edit, no nav).
        let mut st2 = fixture_state(SettingsTab::Status);
        assert_eq!(
            apply_settings_key(&mut st2, k(KeyCode::Char('e'))),
            SettingsOutcome::Stay
        );
    }

    #[test]
    fn tab_strip_brackets_selected_tab() {
        assert_eq!(
            render_tab_strip(SettingsTab::Config),
            " Status  [Config]  Usage   Settings "
        );
        assert_eq!(
            render_tab_strip(SettingsTab::Status),
            "[Status]  Config   Usage   Settings "
        );
    }
}
