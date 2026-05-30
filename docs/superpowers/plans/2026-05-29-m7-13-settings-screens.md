# LingXi Core M7 · Plan 13 · Settings screens (Config / Settings / Status / Usage)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard. Run all `cargo` commands **from inside `lingxi-code/`** (the toolchain pins rust 1.82.0 there; running from the repo root uses the host toolchain and produces spurious lint noise — this bit M6-08).

**Goal:** Ship a `Settings` full-page screen overlay (`crates/tui/src/screens/settings/`) with **four** tab sub-screens — `Config`, `Settings`, `Status`, `Usage` — mirroring claude-code's `components/Settings/{Config,Settings,Status,Usage}.tsx`. The container (`mod.rs`) owns a 4-tab strip and routes left/right/Tab between sub-screens; `Esc`/`q` closes the screen back to the REPL. **Config/Settings render REAL data from the M3 settings store and write ONLY through the existing M3/handle write path (`OrchestratorHandle::edit_config_file`) — no new persistence, validation, or schema logic (§4 R7).** Status renders `OrchestratorHandle::get_status_snapshot()`; Usage renders **flat total cost** from `snapshot_cost()` (per-model breakdown is M8 — documented). The screen plugs into M7-11's `active_screen` enum + priority-2 live-key routing. Telemetry adds **0** events (baseline stays 326; screen-lifecycle events are M7-16).

**Architecture:** The screen is a **route state**, not a z-index overlay (iocraft 0.8.3 has no portable overlay primitive — same posture as M6-05 permission dialogs and M7-11 Doctor). `AppState.active_screen: Option<Screen>` carries a new `Screen::Settings(SettingsState)` variant; when `Some`, `render_screen` returns the settings element INSTEAD OF the REPL 3-zone layout, and `handle_live_key` routes keys to the screen at priority 2 (after permission, before input). `SettingsState { tab: SettingsTab, ... }` is a small UI-state struct owned by `AppState`; tab navigation is a pure `(SettingsState, KeyAction) -> SettingsState` reducer (snapshot/behavior testable). Each sub-screen is a pure `render_*_to_string(&SettingsData) -> String` + a `#[component]` iocraft wrapper, matching the M6-04 / M7-04 two-function renderer pattern. **Settings data is read once when the screen opens** into a `SettingsData` snapshot (the merged `SettingsJson` + a `StatusSnapshot` + a `CostSnapshot`) so the render path stays synchronous and pure — no `.await` inside iocraft's render callback. Editing is **handoff, not inline mutation**: pressing the edit key on the Config tab calls `OrchestratorHandle::edit_config_file()` (the only settings write the engine exposes), which opens `$EDITOR` on `config.json`; on return the screen re-reads the snapshot. There is intentionally NO field-by-field setter — see "§4 R7 guard" below.

**Tech Stack:** Rust 1.82 (pinned via `lingxi-code/rust-toolchain.toml`), iocraft `=0.8.3` (`View` not `Box`; `Text`), `insta = "1.40"` (yaml) snapshots, `tokio` (async handle calls happen in the bridge/open path, NOT in render). `lingxi_core::settings::{Settings, LoadInputs, EffectiveSettings, SettingsJson}` (the M3 settings store — read-only loader). `lingxi_traits::{OrchestratorHandle, StatusSnapshot, CostSnapshot}`. No new workspace deps.

**References:**

- Parent design spec: `docs/superpowers/specs/2026-05-29-m7-tui-surface-design.md` — §1 (goal/non-goals: "Usage shows flat cost"), §2.3 (screens are modal overlays / route states; `AppState.active_screen: Option<Screen>`; `Esc`/`q` returns to REPL), §2.5 (live-key routing priority order — Settings is **priority 2**), §3 "M7-13" entry (lines 239-242), §4 R7 (Settings/Memory write-back scope-creep guard — "reads real data, writes through existing M3 stores only; engine-change needs → defer that piece; keep M7 surface-only"), §5.2 (test budget: 1-2 snapshots each + open/close + cross-state seam).
- **PREREQUISITE — M7-11 (Doctor screen)**, plan `docs/superpowers/plans/2026-05-29-m7-11-doctor-screen.md`. M7-11 is the sub-plan that FIRST introduces `AppState.active_screen`, the `Screen` enum, the priority-2 branch in `handle_live_key`, and the render-active-screen dispatch in `render_screen`. **M7-13 reuses that machinery verbatim — it does NOT re-invent it.** Because M7-11's plan/code may not be written yet when this plan is read, every task that touches shared machinery first GREPS for the real names M7-11 used and adapts (see "Prerequisites & name-discovery" below). If M7-11 is somehow not merged when M7-13 executes, Task 7 contains the minimal `Screen` enum + routing scaffold to add (clearly marked as the M7-11 fallback).
- claude-code byte-locks (verified by direct read at plan-writing time — literal-lock per §2.8):
  - `claude-code/src/components/Settings/Settings.tsx:106` — the tab container builds tabs in this exact order with these exact titles: `Status`, `Config`, `Usage` (the `Gates` tab is gated behind `"external" === 'ant'` → always `[]` in our build → **omit**). The on-close result string is `'Status dialog dismissed'`. **LingXi tab order (LOCKED):** `Config`, `Settings`, `Status`, `Usage` — see "Tab-order decision" below.
  - `claude-code/src/components/Settings/Status.tsx:23-35,49-52` — Status rows, in order: `Version`, `Session name`, `Session ID`, `cwd`, account/API-provider rows, then `Model`, IDE rows, MCP rows, sandbox rows, setting-source rows. Each row renders `<Text bold>{label}:</Text> {value}`. The footer is a dim `Esc` `cancel` hint. The "Session name" empty value renders the dim placeholder `/rename to add a name`. Diagnostics section header is `System Diagnostics`.
  - `claude-code/src/components/Settings/Usage.tsx:174-245` — claude-code's Usage tab shows **rate-limit utilization bars** (`Current session`, `Current week (all models)`, `Current week (Sonnet only)`) fetched from a `/usage` API, with loading literal `Loading usage data…` and error literal prefix `Failed to load usage data`. **LingXi diverges here (documented divergence, NOT a literal-lock match):** that API does not exist in LingXi (it is part of the deferred M8 engine-wiring cluster). M7-13's Usage tab instead shows the **flat session cost** from `snapshot_cost()` — see "Usage flat-cost decision".
  - `claude-code/src/components/Settings/Config.tsx` — a long scrollable list of toggle/select rows (e.g. `Auto-compact`, `Show tips`, `Theme`, `Model`, `Editor mode`, `Verbose output`, `Default permission mode`, …). claude-code lets the user toggle each row inline and persists via its own config store. **LingXi diverges (documented):** LingXi has no inline settings-mutation engine API (§4 R7), so the Config tab is **read-only display of the effective settings + an `e`/`Enter` handoff to `$EDITOR`** — see "§4 R7 guard".
- Predecessor code (read before touching):
  - `crates/tui/src/screens/doctor.rs` (M7-11) — the screen pattern to copy: pure `render_*_to_string`, a `#[component]`, an `active_screen` state variant, the close key, the open path. **If this file does not yet exist, M7-11 is not merged — see Task 7 fallback.**
  - `crates/tui/src/screens/mod.rs` — currently `pub mod repl;` only. Add `pub mod settings;`.
  - `crates/tui/src/root.rs` — `handle_live_key(st, k, viewport)` (THE single live-key dispatcher; §2.5 priority order). The permission focus-trap is priority 1 (`st.pending_permission.is_some()`). M7-11 adds the priority-2 `active_screen.is_some()` branch. M7-13 extends that branch's reducer for the `Settings` variant.
  - `crates/tui/src/app.rs:267` — `render_screen(state, viewport)`; the permission branch (lines 275-308) returns the dialog element instead of `ReplScreen`. M7-11 adds the `active_screen` branch just like it. `dispatch(action, st)` consumes `KeyAction` and mutates `AppState`.
  - `crates/tui/src/state.rs` — `AppState` struct + `StatusSnapshot` (TUI-local: model, cwd, cost String, context_pct, permission_mode). NOTE there are **two** `StatusSnapshot` types: the TUI-local one in `state.rs` (status-line) and `lingxi_traits::StatusSnapshot` (the rich `/status` panel). The Status tab uses the **traits** one (richer).
  - `crates/core/src/settings/mod.rs` — `Settings::load(LoadInputs{env, project_dir, defaults}) -> Result<EffectiveSettings, SettingsError>`; `EffectiveSettings { settings: SettingsJson, trace: ProvenanceTrace }`. **This is the ENTIRE M3 read API — there is no `save`/`set`/`write` method.** `SettingsJson` is `crates/core/src/settings/schema.rs` (camelCase JSON keys, all `Option<T>`).
  - `crates/traits/src/orchestrator.rs:208-239` — `StatusSnapshot { session_id, model, n_messages, total_cost_usd, input_tokens, output_tokens, n_mcp_connected, n_mcp_total, n_hooks, n_agents, started_at, cwd }`. `:27-49` — `CostSnapshot { total_usd, input_tokens, output_tokens, api_calls, session_duration, … }` (NO per-model field). Trait methods `get_status_snapshot()`, `snapshot_cost()`.
  - `crates/orchestrator/src/test_support.rs` — `MockOrchestratorHandle` (used by command tests; reuse for behavior tests that need a handle).
  - `crates/commands/src/builtin/config.rs`, `crates/commands/src/builtin/status.rs` — the existing `/config` and `/status` handlers (M5-11). `/config` calls `handle.edit_config_file()` and prints `Edited {path} (exit {code}).`. M7-13 leaves these handlers intact (they remain the `--no-tui` path) and optionally adds a TUI open-screen hook — see "Command wiring decision".
  - `crates/tui/src/theme.rs` — `TuiTheme::{ASSISTANT, USER, ERROR, DIM}` iocraft `Color` constants. M7-15 adds the full theme; until then use these + literal iocraft colors with a `// TODO(M7-15)` note where a semantic color is missing.

---

## Locked decisions

**Tab-order decision (LOCKED):** LingXi presents tabs in the order **`Config`, `Settings`, `Status`, `Usage`**. This intentionally diverges from claude-code's `Status, Config, Usage` order because the M7-13 brief names the four sub-screens in this order and adds an explicit `Settings` tab (claude-code folds "settings" into Config; LingXi splits the editable config-file handoff (`Config`) from the read-only effective-settings table (`Settings`) for clarity given the §4 R7 read-only constraint). The default selected tab on open is `Config`. Left/`h`/`BackTab` and Right/`l`/`Tab` cycle with **wrap-around** (claude-code's `Tabs` wraps); `Config → Settings → Status → Usage → Config`.

**§4 R7 guard — write-through approach (LOCKED, the central design call):** The M3 settings store (`lingxi_core::settings`) exposes ONLY a 4-layer read (`Settings::load`). There is **no `save`/`set`/`write` method anywhere in `lingxi-*`** (verified: `grep -rn "fn save\|fn set_\|fn write" crates/core/src/settings/` returns nothing that mutates settings). The ONLY settings *write* the engine exposes is `OrchestratorHandle::edit_config_file()`, which opens `$EDITOR` on `config.json` and returns the exit outcome. Therefore:
  - **Config + Settings tabs are read-only display surfaces.** They render the effective merged `SettingsJson` (and, on the Settings tab, provenance from `EffectiveSettings::trace`). No row is an inline toggle. Building an inline field setter would require a NEW engine persistence + validation + schema-write path — **exactly the scope creep §4 R7 forbids.** That work is explicitly **deferred** (to a future engine milestone) and rows render read-only.
  - **The one "write" is the `$EDITOR` handoff.** On the Config tab, the edit key (`e` or `Enter`, claude-code-adjacent) calls `edit_config_file()` through the handle — reusing the EXACT path the M5-11 `/config` command already uses. This is "write through the existing store API" per the brief: no new write logic, just the existing handoff. On editor return the screen re-snapshots so edits show up.
  - Any field that would need a new persist/validate path is rendered read-only and noted; nothing in M7-13 adds an engine write. This keeps M7-13 surface-only.

**Usage flat-cost decision (LOCKED):** claude-code's Usage tab is a rate-limit utilization view backed by a `/usage` API that is part of the deferred M8 engine-wiring cluster (per spec §1 non-goals: "Usage shows flat cost"). M7-13's Usage tab shows the **flat cumulative session cost** from `OrchestratorHandle::snapshot_cost()`: `total_usd` (rendered `$%.4f`), `input_tokens`, `output_tokens`, `api_calls`, and `session_duration`. It renders a single explicit dim line documenting the gap: `Per-model cost breakdown is not available yet (M8).` `CostSnapshot` has **no per-model field** — do not invent one. This is a documented divergence from claude-code's Usage layout, recorded in the M7-16 literal-lock catalog as a LingXi-specific surface.

**Command wiring decision (LOCKED):** The existing `/config` (M5-11) handler stays the `--no-tui` path (opens `$EDITOR`, prints the template). In the iocraft TUI, M7-13 wires a **new `KeyAction::OpenSettings(SettingsTab)`** that opens the Settings screen on the given tab. The `/status` slash command, when running under the iocraft TUI, opens the Settings screen on the `Status` tab; `/config` opens it on the `Config` tab. **Only do this if claude-code routes `/config`/`/status` to the same dialog** — it does (claude-code's `/config` and `/status` both mount the `Settings` dialog with `defaultTab` set). If wiring the slash-command → screen path turns out to entangle the M5-11 handler signatures, defer the command-to-screen hook to M7-16 and ship M7-13 with the screen openable via `KeyAction::OpenSettings` only (a keybinding); note the deferral. The screen open itself is what M7-13 must deliver; the command hook is a thin convenience.

---

## Prerequisites & name-discovery (run FIRST, before Task 1)

M7-11 (Doctor) owns the shared screen machinery. Names below are the EXPECTED names; **verify them against the merged M7-11 code and use whatever M7-11 actually shipped.**

- [ ] **P1: Confirm M7-11 is merged and discover the real names**

Run:
```bash
cd lingxi-core
ls crates/tui/src/screens/doctor.rs && echo "M7-11 PRESENT" || echo "M7-11 MISSING — use Task 7 fallback"
grep -rn "enum Screen\|active_screen\|Screen::Doctor\|fn open_screen\|fn close_screen" crates/tui/src/state.rs crates/tui/src/root.rs crates/tui/src/app.rs crates/tui/src/events/keymap.rs
```
Expected (if M7-11 merged): a `pub enum Screen { Doctor(...), ... }` in `state.rs`, a field `active_screen: Option<Screen>` on `AppState`, a priority-2 branch in `root.rs::handle_live_key` (`if let Some(screen) = &mut st.active_screen { ... return; }` placed AFTER the `pending_permission` branch and BEFORE the input fall-through), and an `active_screen` branch in `app.rs::render_screen`.
Record the EXACT names you find — they are the integration points for every later task. If `Screen` is named differently (e.g. `ScreenState`, `Overlay`), use that name everywhere below.

- [ ] **P2: Confirm the settings read API + status/cost handle methods**

Run:
```bash
cd lingxi-core
grep -rn "pub fn load\|pub struct EffectiveSettings\|pub struct SettingsJson\|pub use schema" crates/core/src/settings/mod.rs
grep -rn "fn get_status_snapshot\|fn snapshot_cost\|fn edit_config_file" crates/traits/src/orchestrator.rs
grep -rn "fn save\|fn set_\|fn store\|fn persist" crates/core/src/settings/ || echo "NO SETTINGS WRITE API — confirms §4 R7 read-only posture"
```
Expected: `Settings::load`, `EffectiveSettings`, `SettingsJson` present; the three handle methods present; **no settings write API** (the last grep prints the confirmation line). This grep result is the justification for the read-only Config/Settings tabs.

- **Telemetry:** baseline is 326 events. **M7-13 adds 0 events.** Do not register any `ALL_EVENT_NAMES` name; screen-lifecycle events are M7-16.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/tui/src/screens/settings/mod.rs` | The `Settings` container: `SettingsTab` enum (`Config`/`Settings`/`Status`/`Usage`), `SettingsState { tab }`, the pure tab-nav reducer `apply_settings_key`, the `SettingsData` read-snapshot struct + its async `snapshot(handle, eff)` constructor, the tab-strip renderer, and the `#[component] SettingsScreen`. Re-exports the four sub-modules. | Create (Tasks 1, 2, 7) |
| `crates/tui/src/screens/settings/config.rs` | `render_config_to_string(&SettingsData) -> String` + `#[component] ConfigTab`. Read-only effective-settings table + `$EDITOR` handoff hint. | Create (Task 3) |
| `crates/tui/src/screens/settings/settings.rs` | `render_settings_to_string(&SettingsData) -> String` + `#[component] SettingsTabView`. Read-only effective settings + per-field provenance (from `EffectiveSettings::trace`). | Create (Task 4) |
| `crates/tui/src/screens/settings/status.rs` | `render_status_to_string(&SettingsData) -> String` + `#[component] StatusTab`. Renders `lingxi_traits::StatusSnapshot` rows (claude-code Status.tsx order/labels). | Create (Task 5) |
| `crates/tui/src/screens/settings/usage.rs` | `render_usage_to_string(&SettingsData) -> String` + `#[component] UsageTab`. Flat cost from `CostSnapshot` + the M8-gap line. | Create (Task 6) |
| `crates/tui/src/screens/mod.rs` | Add `pub mod settings;`. | Modify (Task 1) |
| `crates/tui/src/state.rs` | Add `Screen::Settings(SettingsState)` variant (extend the M7-11 `Screen` enum); helper `AppState::open_settings(tab)` / reuse M7-11's open/close. | Modify (Task 7) |
| `crates/tui/src/events/keymap.rs` | Add `KeyAction::OpenSettings(SettingsTab)` and `KeyAction::CloseScreen` IF M7-11 didn't already add a generic close; map the open keybinding + Esc/q in both `map_key` and the screen reducer. | Modify (Task 7, 8) |
| `crates/tui/src/root.rs` | Extend the M7-11 priority-2 `active_screen` branch to route keys into `apply_settings_key` for the `Settings` variant; mirror `OpenSettings` in `map_iocraft_key`. | Modify (Task 8) |
| `crates/tui/src/app.rs` | Extend the M7-11 `active_screen` branch in `render_screen` to render `SettingsScreen` for the `Settings` variant; handle `OpenSettings`/`CloseScreen` in `dispatch`. | Modify (Task 7, 8) |

**Decomposition note:** `mod.rs` is the container + shared state + the read-snapshot (`SettingsData`); each tab is its own file because the four tabs change for different reasons (Config = handoff, Settings = provenance table, Status = engine status, Usage = cost) — split by responsibility per writing-plans. All five files stay small and pure-render-first.

---

## Task 1: Scaffold the `settings/` module + `SettingsData` read-snapshot

**Files:**
- Create: `crates/tui/src/screens/settings/mod.rs`
- Modify: `crates/tui/src/screens/mod.rs` (add `pub mod settings;`)

- [ ] **Step 1: Add the module declaration**

In `crates/tui/src/screens/mod.rs`, after `pub mod repl;`, add:

```rust
pub mod settings;
```

- [ ] **Step 2: Write the failing test for the read-snapshot shape (TDD)**

Create `crates/tui/src/screens/settings/mod.rs` with the test module first:

```rust
//! Settings screen — Config / Settings / Status / Usage tab overlay (M7-13).
//!
//! A route-state screen (not a z-index overlay): when
//! `AppState.active_screen == Some(Screen::Settings(_))`, `render_screen`
//! returns this screen instead of the REPL, and `handle_live_key` routes
//! keys here at priority 2 (after the permission focus-trap, before input).
//!
//! Reads REAL data from the M3 settings store (`lingxi_core::settings`) and
//! the orchestrator handle (`get_status_snapshot`, `snapshot_cost`). Writes
//! ONLY through the existing `OrchestratorHandle::edit_config_file` handoff
//! (§4 R7 — no new persistence/validation/schema logic). Usage shows FLAT
//! cost (per-model breakdown is M8).

use lingxi_core::settings::{EffectiveSettings, SettingsJson};
use lingxi_traits::{CostSnapshot, StatusSnapshot};

/// Which sub-screen is selected. Tab order is Config → Settings → Status →
/// Usage (LingXi order; see plan "Tab-order decision").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Config,
    Settings,
    Status,
    Usage,
}

impl SettingsTab {
    /// All tabs in display order.
    #[must_use]
    pub fn all() -> [SettingsTab; 4] {
        [SettingsTab::Config, SettingsTab::Settings, SettingsTab::Status, SettingsTab::Usage]
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
    /// Next tab with wrap-around (Right/l/Tab).
    #[must_use]
    pub fn next(self) -> SettingsTab {
        let all = SettingsTab::all();
        let i = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }
    /// Previous tab with wrap-around (Left/h/BackTab).
    #[must_use]
    pub fn prev(self) -> SettingsTab {
        let all = SettingsTab::all();
        let i = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(i + all.len() - 1) % all.len()]
    }
}

/// Immutable snapshot read once when the screen opens. Keeps the render
/// path synchronous + pure (no `.await` in iocraft's render callback).
#[derive(Debug, Clone)]
pub struct SettingsData {
    /// Effective merged settings + per-field provenance (M3 read API).
    pub effective: SettingsJson,
    /// `/status` panel snapshot.
    pub status: StatusSnapshot,
    /// Cumulative cost (FLAT — no per-model field; per-model is M8).
    pub cost: CostSnapshot,
}

/// UI state for the screen — owned by `AppState`.
#[derive(Debug, Clone)]
pub struct SettingsState {
    /// Currently selected tab.
    pub tab: SettingsTab,
    /// The read-once data snapshot.
    pub data: SettingsData,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_cycles_with_wraparound() {
        assert_eq!(SettingsTab::Config.next(), SettingsTab::Settings);
        assert_eq!(SettingsTab::Usage.next(), SettingsTab::Config);
        assert_eq!(SettingsTab::Config.prev(), SettingsTab::Usage);
        assert_eq!(SettingsTab::Settings.prev(), SettingsTab::Config);
    }

    #[test]
    fn tab_titles_match_claude_code() {
        assert_eq!(SettingsTab::Config.title(), "Config");
        assert_eq!(SettingsTab::Settings.title(), "Settings");
        assert_eq!(SettingsTab::Status.title(), "Status");
        assert_eq!(SettingsTab::Usage.title(), "Usage");
    }
}
```

- [ ] **Step 3: Run the tests to verify they pass (pure logic, no impl needed beyond the above)**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::tests`
Expected: PASS — `tab_cycles_with_wraparound`, `tab_titles_match_claude_code` (2 tests). (These exercise the enum methods written in Step 2; they fail to compile until the enum exists, then pass.)

- [ ] **Step 4: Build the crate to prove the module wires in**

Run: `cd lingxi-core && cargo check -p lingxi-tui --all-targets`
Expected: PASS. (A `dead_code` warning on `SettingsState`/`SettingsData`/`config`/etc. is acceptable until later tasks consume them — but suppress with `#[allow(dead_code)]` on the `SettingsState` struct ONLY if clippy's `-D warnings` in the gate would otherwise fail; remove the allow once Task 7 consumes it.)

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/screens/
git commit -m "$(cat <<'EOF'
plan(M7-13 T1): scaffold settings/ module + SettingsTab/SettingsData/SettingsState

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Tab-navigation reducer + the `SettingsData::snapshot` constructor

**Files:**
- Modify: `crates/tui/src/screens/settings/mod.rs`

- [ ] **Step 1: Write the failing test for the tab-nav reducer**

Add to the `tests` module in `mod.rs`:

```rust
    use crate::events::keymap::KeyAction;

    fn fixture_state(tab: SettingsTab) -> SettingsState {
        SettingsState {
            tab,
            data: SettingsData {
                effective: SettingsJson::default(),
                status: StatusSnapshot::default(),
                cost: CostSnapshot::default(),
            },
        }
    }

    #[test]
    fn tab_right_advances_with_wrap() {
        let mut st = fixture_state(SettingsTab::Usage);
        let closed = apply_settings_key(&mut st, &KeyAction::TabNext);
        assert_eq!(st.tab, SettingsTab::Config);
        assert!(!closed, "TabNext must not close the screen");
    }

    #[test]
    fn tab_left_retreats_with_wrap() {
        let mut st = fixture_state(SettingsTab::Config);
        apply_settings_key(&mut st, &KeyAction::TabPrev);
        assert_eq!(st.tab, SettingsTab::Usage);
    }

    #[test]
    fn esc_signals_close() {
        let mut st = fixture_state(SettingsTab::Status);
        let closed = apply_settings_key(&mut st, &KeyAction::CloseScreen);
        assert!(closed, "CloseScreen must request the screen close");
        assert_eq!(st.tab, SettingsTab::Status, "tab unchanged on close");
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::tests::esc_signals_close`
Expected: FAIL to compile — `apply_settings_key` not defined and `KeyAction::{TabNext,TabPrev,CloseScreen}` may not exist yet. (If `KeyAction::TabNext`/`TabPrev`/`CloseScreen` don't exist, add them in `crates/tui/src/events/keymap.rs` now — they are needed by Task 8 too. `CloseScreen` may already exist from M7-11; reuse it if so.)

- [ ] **Step 3: Implement the reducer + the snapshot constructor**

Add to `mod.rs` (top-level, outside `tests`):

```rust
use std::sync::Arc;
use lingxi_traits::OrchestratorHandle;

use crate::events::keymap::KeyAction;

/// Pure tab-navigation reducer. Returns `true` when the key requests the
/// screen close back to the REPL (`Esc`/`q`); the caller (root dispatcher)
/// performs the actual `active_screen = None`. Tab nav mutates `state.tab`.
#[must_use]
pub fn apply_settings_key(state: &mut SettingsState, action: &KeyAction) -> bool {
    match action {
        KeyAction::TabNext => {
            state.tab = state.tab.next();
            false
        }
        KeyAction::TabPrev => {
            state.tab = state.tab.prev();
            false
        }
        KeyAction::CloseScreen => true,
        _ => false,
    }
}

impl SettingsData {
    /// Read every datum the screen displays. Called ONCE on open (and after
    /// the `$EDITOR` handoff) so the render path stays synchronous.
    ///
    /// `eff` is the already-loaded effective settings (the caller loads it
    /// via `lingxi_core::settings::Settings::load`, which is the only M3
    /// read API). Status + cost come from the handle.
    pub async fn snapshot(handle: &Arc<dyn OrchestratorHandle>, eff: &EffectiveSettings) -> Self {
        let status = handle.get_status_snapshot().await;
        let cost = handle.snapshot_cost().await;
        SettingsData {
            effective: eff.settings.clone(),
            status,
            cost,
        }
    }
}
```

If you added `KeyAction` variants, add them to `crates/tui/src/events/keymap.rs`:

```rust
    /// Settings screen: advance to the next tab (Right/l/Tab), wrap-around.
    TabNext,
    /// Settings screen: retreat to the previous tab (Left/h/BackTab).
    TabPrev,
    /// Close the active full-page screen back to the REPL (Esc/q). (May
    /// already exist from M7-11 — do not duplicate; reuse it.)
    CloseScreen,
    /// Open the Settings screen on a given tab (keybinding + /config /status).
    OpenSettings(crate::screens::settings::SettingsTab),
```

- [ ] **Step 4: Run the reducer tests**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::tests`
Expected: PASS — `tab_right_advances_with_wrap`, `tab_left_retreats_with_wrap`, `esc_signals_close` + the two from Task 1 (5 tests).

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/screens/settings/mod.rs crates/tui/src/events/keymap.rs
git commit -m "$(cat <<'EOF'
plan(M7-13 T2): settings tab-nav reducer + SettingsData::snapshot read path

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Config tab — read-only effective config + `$EDITOR` handoff hint

**Files:**
- Create: `crates/tui/src/screens/settings/config.rs`
- Modify: `crates/tui/src/screens/settings/mod.rs` (add `pub mod config;`)

The Config tab is the editable-config-FILE surface. Per §4 R7 it is **read-only display** of the effective settings file values + a footer hint that the edit key opens `$EDITOR`. The actual `edit_config_file()` call is wired in Task 8 (the reducer/handle path); this task ships the pure renderer + component.

- [ ] **Step 1: Write the failing snapshot test**

Create `crates/tui/src/screens/settings/config.rs`:

```rust
//! Config tab — read-only view of the effective config file values + an
//! `$EDITOR` handoff (M7-13). Inline mutation is intentionally absent: the
//! engine exposes no settings-write API (§4 R7), only
//! `OrchestratorHandle::edit_config_file`, so editing is a handoff, not an
//! in-place toggle. Building inline setters would add new persistence +
//! validation + schema logic — exactly the scope creep §4 R7 forbids.

use crate::screens::settings::SettingsData;

/// Render the Config tab body to a plain string (snapshot-testable).
#[must_use]
pub fn render_config_to_string(data: &SettingsData) -> String {
    let s = &data.effective;
    let mut out = String::new();
    out.push_str("Config\n");
    // Effective values from the merged settings file. `None` → "(default)".
    out.push_str(&format!("Model: {}\n", opt(s.model.as_deref())));
    out.push_str(&format!(
        "Telemetry enabled: {}\n",
        s.telemetry_enabled.map_or("(default)".to_string(), |b| b.to_string())
    ));
    out.push_str(&format!(
        "Trusted directories: {}\n",
        list(s.trusted_directories.as_deref())
    ));
    // Footer: the only write path is the $EDITOR handoff.
    out.push_str("e to edit config in $EDITOR · Esc to close");
    out
}

fn opt(v: Option<&str>) -> String {
    v.map_or_else(|| "(default)".to_string(), str::to_string)
}

fn list(v: Option<&[String]>) -> String {
    match v {
        Some(xs) if !xs.is_empty() => xs.join(", "),
        _ => "(none)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::settings::{SettingsData, SettingsTab};
    use lingxi_core::settings::SettingsJson;
    use lingxi_traits::{CostSnapshot, StatusSnapshot};

    fn fixture() -> SettingsData {
        SettingsData {
            effective: SettingsJson {
                model: Some("claude-opus-4-8".to_string()),
                telemetry_enabled: Some(true),
                trusted_directories: Some(vec!["/home/u/proj".to_string()]),
                ..Default::default()
            },
            status: StatusSnapshot::default(),
            cost: CostSnapshot::default(),
        }
    }

    #[test]
    fn config_renders_effective_values_and_editor_hint() {
        let out = render_config_to_string(&fixture());
        insta::assert_snapshot!(out);
    }

    #[test]
    fn config_none_values_show_default_placeholder() {
        let mut f = fixture();
        f.effective = SettingsJson::default();
        let out = render_config_to_string(&f);
        assert!(out.contains("Model: (default)"));
        assert!(out.contains("Trusted directories: (none)"));
    }
}
```

> **IMPORTANT — verify `SettingsJson` field names before writing the above.** Run `grep -n "pub model\|pub telemetry_enabled\|pub trusted_directories\|pub additional_directories\|pub enabled_tools" crates/core/src/settings/schema.rs`. Use the EXACT field names + types found. If a field is not `Option<bool>`/`Option<String>`/`Option<Vec<String>>` as assumed, adjust the formatter. Pick 3-4 stable, present fields for the snapshot; do not enumerate the whole schema.

- [ ] **Step 2: Add `pub mod config;` to `mod.rs`**

In `crates/tui/src/screens/settings/mod.rs`, add near the top: `pub mod config;`

- [ ] **Step 3: Run the snapshot test (it will create the snapshot)**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::config && cargo insta review`
Expected: the first run creates `crates/tui/src/screens/settings/snapshots/...config_renders_effective_values_and_editor_hint.snap`. Review it: it MUST contain `Model: claude-opus-4-8`, `Telemetry enabled: true`, `Trusted directories: /home/u/proj`, and the footer `e to edit config in $EDITOR · Esc to close`. Accept the snapshot. `config_none_values_show_default_placeholder` passes without a snapshot.

- [ ] **Step 4: Add the iocraft component**

Append to `config.rs`:

```rust
use iocraft::prelude::*;
use crate::theme::TuiTheme;

/// Props for the Config tab component.
#[derive(Default, Props)]
pub struct ConfigTabProps {
    /// The read-once data snapshot (cloned into the prop).
    pub data: Option<SettingsData>,
}

/// Config tab — renders the read-only effective config + edit hint.
#[component]
pub fn ConfigTab(props: &ConfigTabProps) -> impl Into<AnyElement<'static>> {
    let body = props
        .data
        .as_ref()
        .map_or_else(String::new, render_config_to_string);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}
```

> Verify `FlexDirection`, `View`, and the `Text` `color:` prop against an existing screen/component (e.g. `screens/doctor.rs` or `screens/repl.rs`) — match the exact iocraft 0.8.3 surface used there. `TuiTheme::DIM` exists (M6).

- [ ] **Step 5: Build + re-run**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::config`
Expected: PASS (2 tests + snapshot).

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/screens/settings/config.rs crates/tui/src/screens/settings/mod.rs crates/tui/src/screens/settings/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-13 T3): Config tab — read-only effective config + $EDITOR handoff hint (§4 R7)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Settings tab — effective settings + per-field provenance (read-only)

**Files:**
- Create: `crates/tui/src/screens/settings/settings.rs`
- Modify: `crates/tui/src/screens/settings/mod.rs` (add `pub mod settings;`)

The Settings tab shows the same effective values as Config but framed as the **merged-settings table with provenance** (which layer set each field: defaults/project/user/env), reading `EffectiveSettings::trace`. This is the read-only counterpart to claude-code's setting-sources view. NOTE: `SettingsData` currently stores only `effective: SettingsJson`. To show provenance, **extend `SettingsData` to also hold the trace** (or the whole `EffectiveSettings`). Decide here.

- [ ] **Step 1: Extend `SettingsData` to carry provenance**

In `crates/tui/src/screens/settings/mod.rs`, change `SettingsData.effective: SettingsJson` to store the whole effective result so provenance is available:

```rust
    /// Effective merged settings + per-field provenance (M3 read API).
    pub effective: EffectiveSettings,
```

Update `SettingsData::snapshot` to take `eff: EffectiveSettings` (by value) and store it; update all `SettingsData { effective: SettingsJson::default(), .. }` fixtures to `EffectiveSettings { settings: SettingsJson::default(), trace: ProvenanceTrace::default() }`. Update `config.rs`'s `render_config_to_string` to read `&data.effective.settings`.

> Verify `ProvenanceTrace` is `Default` and public: `grep -n "pub struct ProvenanceTrace\|pub fn effective_for\|by_field\|FieldProvenance\|enum Source" crates/core/src/settings/tracer.rs`. Use `EffectiveSettings::effective_for(field) -> Option<&FieldProvenance>` (it exists, `mod.rs:98`) to look up the layer. Field names are camelCase wire keys (e.g. `"model"`, `"trustedDirectories"`).

- [ ] **Step 2: Write the failing snapshot test**

Create `crates/tui/src/screens/settings/settings.rs`:

```rust
//! Settings tab — read-only effective-settings table with per-field
//! provenance (which layer set each value: defaults/project/user/env),
//! reading `EffectiveSettings::trace` (M7-13). Read-only per §4 R7 — no
//! inline mutation.

use crate::screens::settings::SettingsData;

/// Render the Settings tab body to a plain string (snapshot-testable).
#[must_use]
pub fn render_settings_to_string(data: &SettingsData) -> String {
    let eff = &data.effective;
    let mut out = String::new();
    out.push_str("Settings\n");
    for field in ["model", "trustedDirectories", "telemetryEnabled"] {
        let prov = eff.effective_for(field).map_or("(default)", source_label);
        out.push_str(&format!("{field}: source={prov}\n"));
    }
    out.push_str("Read-only · edit via Config tab ($EDITOR) · Esc to close");
    out
}

// Map a FieldProvenance to a short source label. Adjust the inner match to
// the real `Source` enum variant names found in tracer.rs.
fn source_label(_prov: &lingxi_core::settings::tracer::FieldProvenance) -> &'static str {
    // PLACEHOLDER SHAPE — replace `_prov.source` access + match arms with the
    // real field/variants from tracer.rs (verified in Step 1).
    "set"
}
```

> The exact `FieldProvenance` shape + `Source` variants MUST come from `tracer.rs` (read it in Step 1). Replace `source_label` to return e.g. `"defaults"`/`"project"`/`"user"`/`"env"`. Do NOT ship the placeholder.

Add the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::settings::SettingsData;
    use lingxi_core::settings::{EffectiveSettings, SettingsJson};
    use lingxi_core::settings::tracer::ProvenanceTrace;
    use lingxi_traits::{CostSnapshot, StatusSnapshot};

    #[test]
    fn settings_renders_provenance_table() {
        let data = SettingsData {
            effective: EffectiveSettings {
                settings: SettingsJson { model: Some("opus".into()), ..Default::default() },
                trace: ProvenanceTrace::default(),
            },
            status: StatusSnapshot::default(),
            cost: CostSnapshot::default(),
        };
        let out = render_settings_to_string(&data);
        insta::assert_snapshot!(out);
    }
}
```

- [ ] **Step 3: Add `pub mod settings;` to `mod.rs`, run test, accept snapshot**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::settings && cargo insta review`
Expected: snapshot created (contains the three `field: source=…` lines + the read-only footer). Accept.

- [ ] **Step 4: Add the iocraft component** (mirror Task 3's `ConfigTab` — `SettingsTabView` with `render_settings_to_string`). Verify against `doctor.rs`.

- [ ] **Step 5: Run + commit**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings`
Expected: PASS (all settings-module tests so far).

```bash
cd lingxi-core && git add crates/tui/src/screens/settings/
git commit -m "$(cat <<'EOF'
plan(M7-13 T4): Settings tab — effective settings + per-field provenance (read-only)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Status tab — `lingxi_traits::StatusSnapshot` rows

**Files:**
- Create: `crates/tui/src/screens/settings/status.rs`
- Modify: `crates/tui/src/screens/settings/mod.rs` (add `pub mod status;`)

Rows follow claude-code `Status.tsx` order/labels: `Version`, `Session ID`, `cwd`, `Model`, then MCP/hooks/agents counts. (LingXi has no `getSessionTitle` / account / IDE rows yet — those need engine surfaces that don't exist; omit them. Note the omission.)

- [ ] **Step 1: Write the failing snapshot test**

Create `crates/tui/src/screens/settings/status.rs`:

```rust
//! Status tab — renders `lingxi_traits::StatusSnapshot` rows (M7-13),
//! matching claude-code Status.tsx row order/labels where the data exists.
//! Account/IDE/session-name rows are omitted (no engine surface yet).

use crate::screens::settings::SettingsData;

/// Render the Status tab body to a plain string (snapshot-testable).
#[must_use]
pub fn render_status_to_string(data: &SettingsData) -> String {
    let s = &data.status;
    let mut out = String::new();
    out.push_str(&format!("Session ID: {}\n", s.session_id));
    out.push_str(&format!("cwd: {}\n", s.cwd.display()));
    out.push_str(&format!("Model: {}\n", s.model));
    out.push_str(&format!("Messages: {}\n", s.n_messages));
    out.push_str(&format!("MCP servers: {} connected / {} configured\n", s.n_mcp_connected, s.n_mcp_total));
    out.push_str(&format!("Hooks: {}\n", s.n_hooks));
    out.push_str(&format!("Agents: {}\n", s.n_agents));
    out.push_str(&format!("Started: {}\n", s.started_at));
    out.push_str("Esc to close");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::settings::SettingsData;
    use lingxi_core::settings::{EffectiveSettings, SettingsJson};
    use lingxi_core::settings::tracer::ProvenanceTrace;
    use lingxi_traits::{CostSnapshot, StatusSnapshot};
    use std::path::PathBuf;

    fn fixture() -> SettingsData {
        SettingsData {
            effective: EffectiveSettings {
                settings: SettingsJson::default(),
                trace: ProvenanceTrace::default(),
            },
            status: StatusSnapshot {
                session_id: "sess-abc123".into(),
                model: "claude-opus-4-8".into(),
                n_messages: 12,
                total_cost_usd: 0.0421,
                input_tokens: 3400,
                output_tokens: 1200,
                n_mcp_connected: 1,
                n_mcp_total: 3,
                n_hooks: 2,
                n_agents: 4,
                started_at: "2026-05-29T10:00:00Z".into(),
                cwd: PathBuf::from("/home/u/proj"),
            },
            cost: CostSnapshot::default(),
        }
    }

    #[test]
    fn status_renders_real_snapshot_rows() {
        let out = render_status_to_string(&fixture());
        insta::assert_snapshot!(out);
        assert!(out.contains("Model: claude-opus-4-8"));
        assert!(out.contains("MCP servers: 1 connected / 3 configured"));
    }
}
```

> Verify `StatusSnapshot` field names against `crates/traits/src/orchestrator.rs:214-239` — they match the fixture above (confirmed at plan time). `StatusSnapshot` derives `Default`.

- [ ] **Step 2: Add `pub mod status;`, run, accept snapshot**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::status && cargo insta review`
Expected: snapshot created + the two `contains` asserts pass. Accept.

- [ ] **Step 3: Add the iocraft `StatusTab` component** (mirror Task 3). Verify against `doctor.rs`.

- [ ] **Step 4: Run + commit**

```bash
cd lingxi-core && git add crates/tui/src/screens/settings/
git commit -m "$(cat <<'EOF'
plan(M7-13 T5): Status tab — real StatusSnapshot rows (model/session/mcp/hooks/agents)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Usage tab — FLAT cost from `CostSnapshot` + documented M8 gap

**Files:**
- Create: `crates/tui/src/screens/settings/usage.rs`
- Modify: `crates/tui/src/screens/settings/mod.rs` (add `pub mod usage;`)

Per the Usage flat-cost decision: render the cumulative cost from `CostSnapshot` and a dim line stating per-model is M8. There is **no per-model field** on `CostSnapshot` — do not invent one.

- [ ] **Step 1: Write the failing snapshot + behavior test**

Create `crates/tui/src/screens/settings/usage.rs`:

```rust
//! Usage tab — FLAT cumulative session cost from
//! `OrchestratorHandle::snapshot_cost` (M7-13). claude-code's Usage tab is a
//! rate-limit utilization view backed by a `/usage` API that is part of the
//! deferred M8 engine-wiring cluster (spec §1 non-goals: "Usage shows flat
//! cost"). Per-model breakdown is M8 — `CostSnapshot` has no per-model field.

use crate::screens::settings::SettingsData;

/// Render the Usage tab body to a plain string (snapshot-testable).
#[must_use]
pub fn render_usage_to_string(data: &SettingsData) -> String {
    let c = &data.cost;
    let mut out = String::new();
    out.push_str("Usage\n");
    out.push_str(&format!("Total cost: ${:.4}\n", c.total_usd));
    out.push_str(&format!("Input tokens: {}\n", c.input_tokens));
    out.push_str(&format!("Output tokens: {}\n", c.output_tokens));
    out.push_str(&format!("API calls: {}\n", c.api_calls));
    out.push_str(&format!("Session duration: {}s\n", c.session_duration.as_secs()));
    // Documented M8 gap — keep this line; it is the parity divergence marker.
    out.push_str("Per-model cost breakdown is not available yet (M8).\n");
    out.push_str("Esc to close");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::settings::SettingsData;
    use lingxi_core::settings::{EffectiveSettings, SettingsJson};
    use lingxi_core::settings::tracer::ProvenanceTrace;
    use lingxi_traits::{CostSnapshot, StatusSnapshot};
    use std::time::Duration;

    fn fixture() -> SettingsData {
        SettingsData {
            effective: EffectiveSettings {
                settings: SettingsJson::default(),
                trace: ProvenanceTrace::default(),
            },
            status: StatusSnapshot::default(),
            cost: CostSnapshot {
                total_usd: 0.1234,
                input_tokens: 5000,
                output_tokens: 2000,
                api_calls: 7,
                session_duration: Duration::from_secs(125),
                ..Default::default()
            },
        }
    }

    #[test]
    fn usage_renders_flat_cost() {
        let out = render_usage_to_string(&fixture());
        insta::assert_snapshot!(out);
        assert!(out.contains("Total cost: $0.1234"));
    }

    #[test]
    fn usage_documents_m8_per_model_gap() {
        let out = render_usage_to_string(&fixture());
        assert!(
            out.contains("Per-model cost breakdown is not available yet (M8)."),
            "Usage MUST show the flat-cost / M8 per-model gap line"
        );
    }
}
```

> Verify `CostSnapshot` field names against `crates/traits/src/orchestrator.rs:27-49`: `total_usd: f64`, `input_tokens: u64`, `output_tokens: u64`, `api_calls: u32`, `session_duration: Duration` (confirmed at plan time). `..Default::default()` covers the legacy `total_nano_usd`/`total_tokens`/`session_id` fields.

- [ ] **Step 2: Add `pub mod usage;`, run, accept snapshot**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings::usage && cargo insta review`
Expected: snapshot created + both behavior asserts pass (`usage_renders_flat_cost`, `usage_documents_m8_per_model_gap`). Accept.

- [ ] **Step 3: Add the iocraft `UsageTab` component** (mirror Task 3).

- [ ] **Step 4: Run + commit**

```bash
cd lingxi-core && git add crates/tui/src/screens/settings/
git commit -m "$(cat <<'EOF'
plan(M7-13 T6): Usage tab — flat cost from CostSnapshot + documented M8 per-model gap

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Wire `Screen::Settings` into AppState + `render_screen` dispatch

**Files:**
- Modify: `crates/tui/src/state.rs` (extend the M7-11 `Screen` enum)
- Modify: `crates/tui/src/app.rs` (extend the M7-11 `render_screen` active-screen branch)
- Modify: `crates/tui/src/screens/settings/mod.rs` (the container component + tab-strip)

- [ ] **Step 1: Build the container component + tab strip in `mod.rs`**

Add to `crates/tui/src/screens/settings/mod.rs`:

```rust
use iocraft::prelude::*;
use crate::theme::TuiTheme;

/// Render the tab strip to a plain string (snapshot-testable). The selected
/// tab is bracketed, e.g. `[Config] Settings Status Usage`.
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
    /// The full screen state (tab + data snapshot).
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
        SettingsTab::Config => element! { config::ConfigTab(data: data) }.into_any(),
        SettingsTab::Settings => element! { settings::SettingsTabView(data: data) }.into_any(),
        SettingsTab::Status => element! { status::StatusTab(data: data) }.into_any(),
        SettingsTab::Usage => element! { usage::UsageTab(data: data) }.into_any(),
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: strip, color: TuiTheme::ASSISTANT)
            #(body)
        }
    }
    .into_any()
}
```

> Verify the `element! { Component(prop: value) }` syntax + child-element interpolation (`#(body)`) against `doctor.rs` / `repl.rs` — match iocraft 0.8.3 exactly. If the sub-tab components take `data: Option<SettingsData>`, pass `data: Some(data)`. Add a `render_tab_strip` snapshot test asserting `[Config]  Settings  Status  Usage` for the default tab.

- [ ] **Step 2: Extend the `Screen` enum in `state.rs`**

If M7-11 shipped `pub enum Screen { Doctor(...) }`, add:

```rust
    /// Settings screen (Config/Settings/Status/Usage tabs — M7-13).
    Settings(crate::screens::settings::SettingsState),
```

Add a helper (or reuse M7-11's open path):

```rust
impl AppState {
    /// Open the Settings screen on a given tab with a pre-read data snapshot.
    pub fn open_settings(&mut self, state: crate::screens::settings::SettingsState) {
        self.active_screen = Some(Screen::Settings(state));
    }
}
```

> **M7-11 FALLBACK (only if `Screen`/`active_screen` do NOT exist — P1 said MISSING):** add to `AppState`: `pub active_screen: Option<Screen>` (init `None` in `AppState::new`); add `pub enum Screen { Settings(crate::screens::settings::SettingsState) }`; in `render_screen` add the branch in Step 3; in `handle_live_key` add the priority-2 branch in Task 8 Step 1. Keep this minimal — it is the machinery M7-11 was supposed to own.

- [ ] **Step 3: Extend `render_screen` in `app.rs`**

After the `pending_permission` branch (the priority-1 focus-trap), before the REPL assembly, add (or extend M7-11's branch):

```rust
    if let Some(screen) = &state.active_screen {
        match screen {
            crate::state::Screen::Settings(ss) => {
                let ss = ss.clone();
                return element! {
                    crate::screens::settings::SettingsScreen(state: ss)
                }
                .into_any();
            }
            // other M7-11/12/14 screens fall through to their own arms
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
```

> If M7-11 already has a `match screen { Screen::Doctor(..) => ... }`, just ADD the `Screen::Settings(..)` arm — do not write a second `if let`.

- [ ] **Step 4: Snapshot the container render path (behavior)**

Add a test in `mod.rs` that builds a `SettingsState` fixture and asserts `render_tab_strip(SettingsTab::Config)` matches the bracketed-selected layout. (Component rendering is covered by the per-tab snapshots; the container test focuses on tab-strip + dispatch selection.)

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings && cargo check -p lingxi-tui --all-targets`
Expected: PASS, builds clean.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/state.rs crates/tui/src/app.rs crates/tui/src/screens/settings/mod.rs crates/tui/src/screens/settings/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-13 T7): wire Screen::Settings into AppState + render_screen; container + tab strip

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Priority-2 live-key routing + open/close + `$EDITOR` handoff

**Files:**
- Modify: `crates/tui/src/root.rs` (priority-2 branch → `apply_settings_key`; `OpenSettings` in `map_iocraft_key`)
- Modify: `crates/tui/src/events/keymap.rs` (map keys in `map_key`)
- Modify: `crates/tui/src/app.rs` (`dispatch` handles `OpenSettings`/`CloseScreen` + the `edit_config_file` handoff path)

- [ ] **Step 1: Route keys through the priority-2 branch in `handle_live_key`**

In `crates/tui/src/root.rs`, the M7-11 priority-2 branch (after `pending_permission`, before input fall-through) must route into the settings reducer for the `Settings` variant:

```rust
    // === PRIORITY 2: a full-page screen owns all keys while open. ===
    if let Some(screen) = &mut st.active_screen {
        match screen {
            crate::state::Screen::Settings(ss) => {
                // Map the raw key to a screen KeyAction (Tab/Left/Right/Esc/q).
                let action = map_screen_key(k);
                if let Some(action) = action {
                    let close = crate::screens::settings::apply_settings_key(ss, &action);
                    if close {
                        st.active_screen = None;
                    }
                }
                return;
            }
            #[allow(unreachable_patterns)]
            _ => return, // other screens handled by their own M7-11/12/14 routing
        }
    }
    // === end priority 2 ===
```

Add a small `map_screen_key(k: &KeyEvent) -> Option<KeyAction>` helper in `root.rs` mapping: `Tab`/`Right`/`l` → `TabNext`; `BackTab`/`Left`/`h` → `TabPrev`; `Esc`/`q` → `CloseScreen`; `Char('e')`/`Enter` on the Config tab → an `EditConfig` action (see Step 3). (Reuse M7-11's screen-key mapper if it added one; extend it for tabs.)

- [ ] **Step 2: Map `OpenSettings` in both keymaps + `dispatch`**

In `crates/tui/src/events/keymap.rs::map_key`, bind the open keybinding (claude-code uses no single global key for Settings — it opens via `/config` `/status`; for the keyboard path bind a sensible key, e.g. `Ctrl+,` if free, else leave open-only-via-command and document). In `app.rs::dispatch`, handle `KeyAction::OpenSettings(tab)` by setting `st.active_screen = Some(Screen::Settings(SettingsState { tab, data }))` — but `data` requires an async read, so the open must happen on the bridge/command path that CAN await, not the synchronous `dispatch`. **Decision:** the open is triggered by the slash-command handler (async) or by a bridge event; the synchronous `dispatch` stores a `pending_open: Option<SettingsTab>` flag that the bridge pump fills in by reading the snapshot. Simpler alternative for M7-13: open the screen with a data snapshot read lazily once and cached. **Pick the lazy-read approach:** store `Screen::Settings(SettingsState { tab, data })` where `data` was read by the command handler (which is async) before calling a sync `open_settings`. Wire `/config`/`/status` (Task 9) to do the async read then open.

- [ ] **Step 3: `$EDITOR` handoff on the Config tab**

When `map_screen_key` yields the edit action on the Config tab, the handler must call `handle.edit_config_file().await` (async) and then re-snapshot. Because `handle_live_key` is synchronous, route the edit like the open: set a `pending_config_edit: bool` flag on `AppState` that the bridge pump observes, awaits `edit_config_file()`, re-reads the snapshot into the live `Screen::Settings(..).data`, and clears the flag. Keep the actual `edit_config_file()` call on the async bridge path — **never `.await` in the sync key/render callbacks** (the M6 architecture rule). Document this in a comment. This is the ONLY write path (§4 R7).

- [ ] **Step 4: Behavior test — tab nav cycles + Esc closes**

Add `crates/tui/tests/settings_screen_test.rs`:

```rust
//! M7-13 behavior: settings screen routing through the live-key dispatcher.

use lingxi_tui::screens::settings::{apply_settings_key, SettingsState, SettingsTab, SettingsData};
// build a fixture SettingsState (Config tab, default data) and drive the reducer

#[test]
fn tab_nav_cycles_all_four_then_wraps() {
    let mut st = /* fixture at Config */;
    use lingxi_tui::events::keymap::KeyAction::TabNext;
    assert_eq!(st.tab, SettingsTab::Config);
    apply_settings_key(&mut st, &TabNext); assert_eq!(st.tab, SettingsTab::Settings);
    apply_settings_key(&mut st, &TabNext); assert_eq!(st.tab, SettingsTab::Status);
    apply_settings_key(&mut st, &TabNext); assert_eq!(st.tab, SettingsTab::Usage);
    apply_settings_key(&mut st, &TabNext); assert_eq!(st.tab, SettingsTab::Config); // wrap
}

#[test]
fn esc_requests_close() {
    let mut st = /* fixture at Status */;
    use lingxi_tui::events::keymap::KeyAction::CloseScreen;
    assert!(apply_settings_key(&mut st, &CloseScreen));
}
```

> Fill the fixture using `SettingsData` with `EffectiveSettings`/`StatusSnapshot`/`CostSnapshot` defaults (as in the unit tests). Ensure `apply_settings_key`, `SettingsState`, `SettingsTab`, `SettingsData`, and `KeyAction` are `pub` from `lingxi_tui` (add `pub use` re-exports in `lib.rs` if needed).

- [ ] **Step 5: Behavior test — full live-key path closes the screen**

Add a test that builds an `AppState` with `active_screen = Some(Screen::Settings(..))`, calls `handle_live_key(&mut st, &esc_key, viewport)`, and asserts `st.active_screen.is_none()`. This is the cross-state seam test — Esc through the REAL dispatcher (not just the reducer) must close. Mirror the existing `focus_trap_test.rs` pattern for constructing iocraft `KeyEvent`s.

- [ ] **Step 6: Behavior test — settings READ shows real values; edit goes through the handle**

Add a test using `MockOrchestratorHandle` (from `lingxi_orchestrator::test_support`): set a known model/cost on the mock, call `SettingsData::snapshot(&handle, &eff).await`, and assert `data.status.model` + `data.cost.total_usd` reflect the mock. For the write path: call the mock's `edit_config_file()` (it returns a fixed `Edited /tmp/mock/config.json (exit 0)` outcome) and assert the handoff returns `Ok`. This proves "settings read shows real values" + "edit writes through the existing M3/handle store API" with a mock store. (Use a `#[tokio::test]`.)

- [ ] **Step 7: Run all behavior tests**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test settings_screen_test`
Expected: PASS — `tab_nav_cycles_all_four_then_wraps`, `esc_requests_close`, the live-path close, the read-real-values + edit-through-handle tests.

- [ ] **Step 8: Commit**

```bash
cd lingxi-core && git add crates/tui/src/root.rs crates/tui/src/events/keymap.rs crates/tui/src/app.rs crates/tui/src/lib.rs crates/tui/tests/settings_screen_test.rs
git commit -m "$(cat <<'EOF'
plan(M7-13 T8): priority-2 routing + open/close + $EDITOR handoff; behavior tests

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Wire `/config` and `/status` slash commands to open the screen (TUI path)

**Files:**
- Modify: `crates/tui/src/...` (the command-dispatch → screen-open glue; exact file depends on how M5-13/M6 route slash commands in the TUI)

Per the command-wiring decision: `/config` opens the Settings screen on the `Config` tab; `/status` opens it on the `Status` tab — matching claude-code (both mount the Settings dialog with `defaultTab`). The async read happens in the command path (which can `.await`), then `open_settings` is called.

- [ ] **Step 1: Find the TUI slash-command dispatch seam**

Run: `cd lingxi-core && grep -rn "edit_config_file\|get_status_snapshot\|dispatch.*command\|run_slash\|CommandResult" crates/tui/src/`
Identify where the TUI routes a `/`-command result back into `AppState` (M5/M6 wired the slash surface; the REPL submits a `/`-line and applies the result). This is where `/config`/`/status` must additionally open the screen instead of (or in addition to) printing the `SystemText` result.

- [ ] **Step 2: On `/config`/`/status` in the TUI, read the snapshot + open the screen**

In that seam, for the two command names: load effective settings via `lingxi_core::settings::Settings::load(LoadInputs { env, project_dir: cwd, defaults })`, build `SettingsData::snapshot(&handle, &eff).await`, then `st.open_settings(SettingsState { tab, data })` (tab = `Config` for `/config`, `Status` for `/status`). Leave the existing M5-11 handlers as the `--no-tui` path untouched.

> **If this entangles the M5-11 handler signatures or the command dispatch is not reachable from a place that holds both the handle and `&mut AppState`:** DEFER the command hook to M7-16 and ship M7-13 with the screen openable via the `KeyAction::OpenSettings` keybinding only. Note the deferral in the commit message + the M7-16 plan. The screen itself (Tasks 1-8) is the M7-13 deliverable; the command convenience is secondary.

- [ ] **Step 3: Behavior test — `/status` opens the screen on the Status tab**

Add a test (or extend `settings_screen_test.rs`) that simulates the `/status` command path and asserts `st.active_screen` is `Some(Screen::Settings(SettingsState { tab: SettingsTab::Status, .. }))`. If deferred per Step 2, skip this test and document.

- [ ] **Step 4: Run + commit**

Run: `cd lingxi-core && cargo test -p lingxi-tui`
Expected: PASS.

```bash
cd lingxi-core && git add crates/tui/src/
git commit -m "$(cat <<'EOF'
plan(M7-13 T9): /config and /status open the Settings screen on the right tab (TUI path)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Container snapshot at fixed state + cross-state seam guard

**Files:**
- Modify: `crates/tui/src/screens/settings/mod.rs` (snapshot test)
- Modify: `crates/tui/tests/settings_screen_test.rs` (seam test)

- [ ] **Step 1: Snapshot the full body string per tab at a fixed `SettingsData`**

Add a `mod.rs` test that builds ONE fixed `SettingsData` (the union of the Task 3/5/6 fixtures) and snapshots the selected-tab body for each of the four tabs via the per-tab `render_*_to_string` fns, plus the tab strip. This is the "snapshot per sub-screen at fixed state" deliverable consolidated. (The per-tab unit snapshots from Tasks 3-6 already cover each; this is the combined-fixture lock.)

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib screens::settings && cargo insta review`
Expected: snapshots created/accepted.

- [ ] **Step 2: Cross-state seam — open the screen while a permission is pending (priority order holds)**

Per spec §5.6: build an `AppState` with BOTH `pending_permission = Some(..)` AND `active_screen = Some(Screen::Settings(..))`, send an Esc key through `handle_live_key`, and assert the **permission dialog** consumed it (priority 1 wins — the screen did NOT close, the permission resolved). This guards the M6 focus-trap lesson: the screen must NOT steal keys from a pending permission.

```rust
#[test]
fn permission_pending_wins_over_open_settings_screen() {
    // st with pending_permission AND active_screen = Settings
    // send Esc; assert pending_permission resolved (its handler ran),
    // and active_screen is unchanged (screen did not close).
}
```

> Construct `pending_permission` + its `resp_tx` like `focus_trap_test.rs` does. The key assertion is that priority 1 (permission) intercepts before priority 2 (screen).

- [ ] **Step 3: Run + commit**

Run: `cd lingxi-core && cargo test -p lingxi-tui`
Expected: PASS.

```bash
cd lingxi-core && git add crates/tui/src/screens/settings/ crates/tui/tests/settings_screen_test.rs
git commit -m "$(cat <<'EOF'
plan(M7-13 T10): container snapshot at fixed state + permission-over-screen seam guard

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Telemetry baseline check + docs of deferrals

**Files:**
- Verify: `crates/telemetry/src/...` (no new names)
- Modify: this plan / inline code comments (deferral notes)

- [ ] **Step 1: Confirm 0 new telemetry events**

Run: `cd lingxi-core && cargo test -p lingxi-telemetry 2>&1 | tail -20` (or the test that asserts `ALL_EVENT_NAMES.len()`).
Expected: the event-count test still asserts **326** (unchanged). M7-13 registered no new names.

- [ ] **Step 2: Confirm the §4 R7 read-only posture is documented in code**

Verify each of `config.rs`, `settings.rs` has a module-doc line stating editing is `$EDITOR`-handoff-only (no inline mutation) per §4 R7, and `usage.rs` documents the flat-cost / M8 per-model gap. (These were added in Tasks 3/4/6 — this step is the audit.)

Run: `cd lingxi-core && grep -rn "§4 R7\|R7\|M8\|flat cost\|read-only\|edit_config_file" crates/tui/src/screens/settings/`
Expected: hits in all four sub-screen files documenting the constraints.

- [ ] **Step 3: Commit (if any comment edits were needed)**

```bash
cd lingxi-core && git add crates/tui/src/screens/settings/
git commit -m "$(cat <<'EOF'
plan(M7-13 T11): audit telemetry baseline (326, +0) + §4 R7 / M8 deferral docs

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

(If no edits were needed, skip the commit and note "no-op" in the task log.)

---

## Task 12: Workspace verification gate + annotated tag `m7.13`

**Files:** none (verification + tag only)

This is the workspace-wide guard. Run **from inside `lingxi-code/`** (the toolchain pins rust 1.82.0 there).

- [ ] **Step 1: Format check**

Run: `cd lingxi-core && cargo fmt --check`
Expected: clean (no diff).

- [ ] **Step 2: Clippy (workspace, all targets, deny warnings)**

Run: `cd lingxi-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. (Remove any temporary `#[allow(dead_code)]` added in Task 1 now that all symbols are consumed.)

- [ ] **Step 3: Full workspace test**

Run: `cd lingxi-core && cargo test --workspace`
Expected: PASS. Known flakes (allowed rerun): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests. Re-run once if only those fail.

- [ ] **Step 4: Cross-platform compile gate (5 targets)**

Run:
```bash
cd lingxi-core && for t in x86_64-unknown-linux-gnu x86_64-apple-darwin x86_64-pc-windows-gnu aarch64-linux-android aarch64-apple-ios; do cargo check --workspace --target "$t" || echo "FAILED: $t"; done
```
Expected: all 5 green (same posture as v0.6.0/v0.7.0). Settings screens are pure-render + read-only — no platform-specific code, so this should pass without target-specific guards.

- [ ] **Step 5: Annotated tag**

Run:
```bash
cd lingxi-core && git tag -a m7.13 -m "M7-13: Settings screens (Config/Settings/Status/Usage) — read real M3 settings, write via edit_config_file handoff only (§4 R7), flat-cost Usage (M8 per-model gap), priority-2 screen routing"
```
Expected: tag `m7.13` created locally. **Do NOT push.** (Per spec §6.4 — no remote push from Claude.)

- [ ] **Step 6: Confirm the tag**

Run: `cd lingxi-core && git tag -l 'm7.13' && git log --oneline -1`
Expected: `m7.13` listed; HEAD is the Task 11 (or last) commit.

---

## Self-Review (run after writing all tasks)

**Spec coverage (§3 M7-13):**
- 4 sub-screens under `screens/settings/`: `mod.rs` (T1/2/7), `config.rs` (T3), `settings.rs` (T4), `status.rs` (T5), `usage.rs` (T6). ✔
- Tab navigation between sub-screens: `apply_settings_key` + `render_tab_strip` (T2, T7, T8). ✔
- Reads REAL settings (M3 store) + writes ONLY through existing store API: `Settings::load` read + `edit_config_file` handoff; no new write logic (T2, T3, T8, §4 R7 guard). ✔
- Status/Usage read from `get_status_snapshot` + `snapshot_cost`; Usage flat-cost + documented M8 gap (T5, T6). ✔
- `Screen::Settings` variant + reuse M7-11 active_screen + priority-2 routing (T7, T8). ✔
- `/config` `/status` open the right sub-screen (T9, with documented defer escape). ✔

**Tests required (brief):**
- Snapshot per sub-screen at fixed state: T3 (Config), T4 (Settings), T5 (Status), T6 (Usage), T10 (combined). ✔
- Tab nav cycles 4; Esc closes: T8 (reducer + live path), T10 (seam). ✔
- Settings read shows real values; edit writes through M3 store (mock/temp): T8 Step 6. ✔
- Usage flat cost + documented M8 per-model gap: T6 `usage_documents_m8_per_model_gap`. ✔

**Placeholder scan:** the `source_label` placeholder in T4 is explicitly flagged "do NOT ship the placeholder" with a grep to find real `Source` variants; the `SettingsJson` field names in T3 carry a verify-before-writing grep. No TBD/TODO-without-content. ✔

**Type consistency:** `SettingsTab`, `SettingsState`, `SettingsData` (carrying `EffectiveSettings` after T4's extension — note the T1→T4 migration of `effective` from `SettingsJson` to `EffectiveSettings` is called out in T4 Step 1), `apply_settings_key`, `KeyAction::{TabNext,TabPrev,CloseScreen,OpenSettings}`, `Screen::Settings`, `SettingsScreen`, `ConfigTab`/`SettingsTabView`/`StatusTab`/`UsageTab` used consistently across T1-T10. The `StatusSnapshot`/`CostSnapshot` field names match `crates/traits/src/orchestrator.rs` (verified). ✔

---

## Execution Handoff

Plan complete. Two execution options:
1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks.
2. **Inline Execution** — batch tasks in-session with checkpoints (superpowers:executing-plans).
