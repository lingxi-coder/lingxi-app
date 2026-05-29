# M7-11 — Doctor Screen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the FIRST full-page TUI screen (Doctor — claude-code `Doctor.tsx` parity) and, in doing so, establish the reusable **screen-overlay infrastructure** that M7-12/13/14 will plug into: an `AppState.active_screen: Option<Screen>` route-state, a priority-2 branch in the single `handle_live_key` dispatcher, and a `render_screen` overlay path. `/doctor` opens the screen; `Esc` / `q` closes it back to the REPL. The M6 permission focus-trap stays priority 1 and is not regressed.

**Architecture:** Screens are **modal overlays / route states** (parent spec §2.3). A new `AppState.active_screen` field holds `None` (REPL is live) or `Some(Screen::Doctor)` (the Doctor screen owns the surface). Live keys route through the **single** `handle_live_key` dispatcher in `root.rs` in strict priority order (parent spec §2.5): permission (1) → screen (2) → input/scroll (existing). When a screen is active, `Esc`/`q` clears `active_screen` and **no key leaks to `PromptInput`**. `render_screen` (in `app.rs`) renders the active screen INSTEAD OF the REPL when `active_screen.is_some()` — same "render-instead-of, no z-index" discipline M6-05 used for permission dialogs. The Doctor screen is a pure iocraft component fed a plain `DoctorDiagnostics` value-struct; it does **no async and no engine calls itself** — the struct is built once when the screen opens, from data already on `AppState`/`StatusSnapshot` plus the MCP-server list the TUI already surfaces. M7 is TUI-only: zero engine changes.

**Tech Stack:** Rust 1.82 (pinned via `lingxi-core/rust-toolchain.toml` — run all cargo from **inside `lingxi-core/`**), iocraft 0.8.3 (`View` not `Box`; crossterm-0.29 re-exports inside iocraft, workspace pinned to crossterm-0.28 — `root.rs` already bridges the skew). Tests: `cargo test` unit + behavior, `insta` snapshots. No new dependencies. **0 new telemetry events** (screen-open/close events are a candidate but DEFERRED to the M7-16 audit — see "Telemetry" below).

**Prerequisite:** none beyond M6 (v0.7.0). This plan does NOT depend on M7-01..M7-10. It only touches `state.rs`, `app.rs`, `root.rs`, and adds `screens/doctor.rs` + `screens/mod.rs`. If those earlier sub-plans have already landed, this plan still applies cleanly (its edits are additive and localized to the screen seam).

---

## Background the engineer needs (read this first)

### The single live-key dispatcher (the §4 R4-critical seam)

There is exactly ONE place a live keystroke is routed: `root.rs::handle_live_key(st: &mut AppState, k: &KeyEvent, viewport: usize)`. The live `use_terminal_events` closure calls it and nothing else. M6-05's final review caught a ship-blocker — a parallel key path that bypassed the permission focus-trap — so the rule is: **never add a second key path; only add a branch inside `handle_live_key`, in priority order.**

Today `handle_live_key` is (abridged):

```rust
pub fn handle_live_key(st: &mut AppState, k: &KeyEvent, viewport: usize) {
    // === FOCUS TRAP: a permission dialog owns all keys while open. ===
    if st.pending_permission.is_some() {
        let ct_key = iocraft_to_crossterm028_key(k);
        let _ = crate::events::keymap::handle_key(st, ct_key);
        return;
    }
    // === end focus trap ===
    let prompt_empty = st.prompt_text.is_empty();
    let focus_active = /* ... */;
    if let Some(action) = map_iocraft_key(k, prompt_empty, focus_active) {
        if let KeyAction::ScrollStep(dir) = action {
            scroll_with_viewport(st, dir, viewport);
        } else {
            let _ = dispatch(action, st);
        }
    }
}
```

M7-11 inserts a **priority-2 branch** immediately after the focus-trap early-return and before the `map_iocraft_key` fall-through:

```
1. pending_permission.is_some()   → permission dialog (M6)    ← UNCHANGED, STAYS FIRST
2. active_screen.is_some()        → active screen (M7-11)      ← NEW
3. map_iocraft_key + dispatch     → input / scroll (M6)        ← unchanged fall-through
```

Because the permission check is still the first statement and still `return`s, opening a screen while a permission is pending cannot steal the key: the permission branch fires first and `return`s before the screen branch is even reached. That invariant is exactly what Task 6's behavior test locks.

### How `/doctor` opens the screen (the submit seam)

`/doctor` arrives as a submitted line. In the live iocraft mount, `Enter` maps to `KeyAction::Submit`, which `app::dispatch` handles. M5-11 shipped a `/doctor` slash command that renders a *text* report; in the TUI we intercept `/doctor` **before** that text dispatcher (exactly like `app.rs` already intercepts `/clear` and `/exit` locally inside `handle_submit_line`) and instead set `active_screen = Some(Screen::Doctor)`. The text `/doctor` report stays the `--no-tui` stdio fallback (untouched).

The diagnostics the screen shows come from a `DoctorDiagnostics` struct built at open time. We build it from data already present in the TUI without any new engine call:
- **versions** — `lingxi-cli` version from `env!("CARGO_PKG_VERSION")`; rust toolchain from `env!` build constants (see Task 2 for the exact source).
- **config paths** — claude home + cwd from `StatusSnapshot.cwd` and the standard config-dir helper already used elsewhere.
- **MCP servers** — configured count + connected count. `AppState` already carries the model/cwd/cost status; the MCP list is surfaced via the same `StatusSnapshot`-adjacent path M6-07 wired (`n_mcp_total` / `n_mcp_connected` exist on the orchestrator's `StatusSnapshot`). For v0.8.0, since MCP auto-connect is M8, the screen shows "configured, not connected" when total > 0 and connected == 0.
- **auth state** — stubbed/basic ("logged in" unknown until OAuth lands in M8) → render `"unknown"`.
- **terminal capabilities** — truecolor (from `$COLORTERM`) + size (`cols × rows`, already available in `root.rs` via `use_terminal_size`).

To keep the screen pure and testable, the engineer threads a `DoctorDiagnostics` value into `AppState` when the screen opens (set alongside `active_screen`). The screen component reads that struct. No `OrchestratorHandle` call happens on the render path.

### claude-code `Doctor.tsx` — the rows + labels (literal lock)

`claude-code/src/screens/Doctor.tsx` (575 lines, compiled-by-react-compiler form) renders a `Pane` with bold section headers and `└ `-prefixed detail rows. The sections we mirror (literal-lock the section titles + the `└ ` row prefix; LingXi-specific values fill the right side):

- **`Diagnostics`** (bold) — rows: `└ Currently running: {installationType} ({version})`, `└ Path: {installationPath}`, `└ Invoked: {invokedBinary}`, `└ Search: {OK|Not working} ({mode})`.
- **`Updates`** (bold) — rows: `└ Auto-updates: {…}`, `└ Auto-update channel: {channel}`.
- (Doctor.tsx also has SandboxDoctorSection, McpParsingWarnings, KeybindingWarnings, Environment Variables, Version Locks, Agent Parse Errors, Plugin Errors, Context Usage Warnings — these are claude-code-internal and **out of scope** for v0.8.0; the parent spec scopes M7-11 to "versions, config paths, MCP servers, auth state, terminal capabilities".)
- Dismiss affordance: claude-code uses `PressEnterToContinue`; we use the LingXi convention `Esc`/`q` (the parent spec mandates Esc/`q` close, consistent with the other M7 screens).

> **If `claude-code/` is absent at execution time:** use the LingXi label set encoded verbatim in Task 4's snapshot below. That snapshot IS the spec for the row strings — you do not need the submodule to execute this plan. The section headers `Diagnostics` and `Updates` are byte-locked from the source above.

### iocraft component conventions in this crate (copy these)

- Components are `#[component] pub fn Name(props: &NameProps) -> impl Into<AnyElement<'static>>`.
- Props are a `#[derive(Default, Props)]` struct (see `screens/repl.rs::ReplScreenProps`). Props with no natural `Default` need a manual `impl Default`.
- Layout: `View(flex_direction: FlexDirection::Column)`, `Text(content: ...)`. Bold via `Text(content: ..., weight: Weight::Bold)`; dim via a dim color. Match the existing `status_line.rs` / `repl.rs` usage exactly.
- Snapshot tests live in `crates/tui/tests/render_*.rs`, call `element.to_string()`, and `insta::assert_snapshot!`. New snapshots are accepted with `cargo insta accept` from inside `lingxi-core/`.

---

## File Structure

| Path | New/Modify | Responsibility |
|---|---|---|
| `lingxi-core/crates/tui/src/screens/doctor.rs` | **Create** | The Doctor screen: `DoctorDiagnostics` value-struct (the pure data the screen renders) + `DoctorScreen` iocraft component. Pure render — no async, no engine calls. Row/label literals live here. ~220 lines incl. tests. |
| `lingxi-core/crates/tui/src/screens/mod.rs` | **Modify** | `pub mod doctor;` and define the reusable `Screen` enum (`Doctor` variant now; `// Resume/Settings/Memory added by M7-12/13/14` placeholder comment). This is the NEW shared screen-routing type. |
| `lingxi-core/crates/tui/src/state.rs` | **Modify** | Add `active_screen: Option<crate::screens::Screen>` and `doctor_diagnostics: Option<crate::screens::doctor::DoctorDiagnostics>` to `AppState`; init both to `None` in `AppState::new`. Add `open_doctor(&mut self, diag)` / `close_screen(&mut self)` helpers. |
| `lingxi-core/crates/tui/src/root.rs` | **Modify** | Insert the priority-2 screen branch in `handle_live_key` (after the focus-trap `return`, before `map_iocraft_key`). Add a small `fn handle_screen_key(st, k) -> bool` that consumes the key for the active screen (Esc/`q` close → `true`; everything else → swallow → `true` so nothing leaks). |
| `lingxi-core/crates/tui/src/app.rs` | **Modify** | (a) In `render_screen`, after the permission overlay branch, add: `if active_screen.is_some() → render the active screen instead of ReplScreen`. (b) In `dispatch` (or a focused helper called from the Submit path), intercept a submitted `/doctor` line and set `active_screen = Doctor` + build `DoctorDiagnostics`. |
| `lingxi-core/crates/tui/tests/render_doctor_screen.rs` | **Create** | Snapshot: `DoctorScreen` at a fixed `DoctorDiagnostics`. |
| `lingxi-core/crates/tui/tests/behavior_doctor_screen.rs` | **Create** | Behavior: open/close routing; the §4 R4 permission-priority seam; the no-leak-to-PromptInput guard. Drives `handle_live_key` (live path) + the submit intercept. |

**Decomposition rationale:** The reusable infra (`Screen` enum + `active_screen` field + priority-2 routing + overlay render) is established in Tasks 1–3 in `state.rs`/`mod.rs`/`root.rs`/`app.rs` so M7-12/13/14 only add a `Screen` variant + a `render` arm + a key-routing arm — no re-plumbing. The Doctor-specific surface (`DoctorDiagnostics` + `DoctorScreen` + its open intercept) is Tasks 2/4/5. Tests (6) cover the seams the parent spec §4 R4 / §5.6 call out. Pure render logic is isolated in `doctor.rs` so it snapshot-tests without a terminal.

---

## Type contract (locked — used by every task below)

These names appear verbatim in the tasks. Later tasks reference exactly these.

```rust
// screens/mod.rs
/// Which full-page screen currently overlays the REPL. `None` ⇒ REPL is
/// live. Established by M7-11; M7-12/13/14 add `Resume`/`Settings`/`Memory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The diagnostic screen opened by `/doctor`.
    Doctor,
    // Resume,   // M7-12
    // Settings, // M7-13
    // Memory,   // M7-14
}
```

```rust
// screens/doctor.rs
/// Plain value-struct: everything the Doctor screen renders, captured once
/// when the screen opens. Pure data — no handles, no async. Built by
/// `DoctorDiagnostics::capture(...)` from AppState/status at open time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorDiagnostics {
    /// e.g. "lingxi-cli v0.8.0".
    pub cli_version: String,
    /// e.g. "1.82.0".
    pub rust_toolchain: String,
    /// Absolute claude config home (e.g. "~/.claude" expanded).
    pub claude_home: String,
    /// Working directory.
    pub cwd: String,
    /// MCP servers configured (any state).
    pub mcp_configured: u32,
    /// MCP servers currently connected.
    pub mcp_connected: u32,
    /// Auth state label ("unknown" until OAuth lands in M8).
    pub auth_state: String,
    /// Whether the terminal advertises truecolor ($COLORTERM=truecolor|24bit).
    pub truecolor: bool,
    /// Terminal size at open time (cols, rows).
    pub term_size: (u16, u16),
}
```

---

## Task 1: `Screen` enum + `active_screen` route-state (reusable infra)

**Files:**
- Modify: `lingxi-core/crates/tui/src/screens/mod.rs`
- Modify: `lingxi-core/crates/tui/src/state.rs`
- Test: `lingxi-core/crates/tui/src/state.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** (append to `state.rs` `mod tests`)

```rust
#[test]
fn active_screen_defaults_none_and_open_close_toggles() {
    use crate::screens::Screen;
    let mut st = AppState::default_for_tests();
    assert_eq!(st.active_screen, None, "REPL is live by default");
    st.active_screen = Some(Screen::Doctor);
    assert_eq!(st.active_screen, Some(Screen::Doctor));
    st.close_screen();
    assert_eq!(st.active_screen, None, "close_screen returns to REPL");
    assert!(st.doctor_diagnostics.is_none(), "close clears captured diagnostics");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run (from inside `lingxi-core/`): `cargo test -p lingxi-tui --lib active_screen_defaults_none -- --nocapture`
Expected: FAIL — `Screen` unresolved / `active_screen` field missing / `close_screen` not found.

- [ ] **Step 3: Define the `Screen` enum** in `screens/mod.rs`

```rust
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
```

- [ ] **Step 4: Add the fields + helpers to `AppState`** in `state.rs`

In the `AppState` struct (after the M6-05 dialog-state fields), add:

```rust
    /// (M7-11) Active full-page screen overlay. `None` ⇒ REPL is live.
    /// Routed at priority 2 in `handle_live_key` (after permission,
    /// before input). Reused by M7-12/13/14.
    pub active_screen: Option<crate::screens::Screen>,
    /// (M7-11) Diagnostics captured when the Doctor screen opens. `Some`
    /// only while `active_screen == Some(Screen::Doctor)`.
    pub doctor_diagnostics: Option<crate::screens::doctor::DoctorDiagnostics>,
```

In `AppState::new`, initialize both:

```rust
            active_screen: None,
            doctor_diagnostics: None,
```

Add the helpers in `impl AppState`:

```rust
    /// (M7-11) Open the Doctor screen with captured diagnostics.
    pub fn open_doctor(&mut self, diag: crate::screens::doctor::DoctorDiagnostics) {
        self.active_screen = Some(crate::screens::Screen::Doctor);
        self.doctor_diagnostics = Some(diag);
    }

    /// (M7-11) Close any active screen, returning to the REPL. Clears
    /// per-screen captured state. Reused by every M7 screen's close path.
    pub fn close_screen(&mut self) {
        self.active_screen = None;
        self.doctor_diagnostics = None;
    }
```

- [ ] **Step 5: Run the test to verify it passes** (it will still fail to *compile* until Task 2 defines `DoctorDiagnostics`)

Because `state.rs` now references `crate::screens::doctor::DoctorDiagnostics`, the crate won't compile until Task 2 lands. That is expected — **do Task 2 next**, then run both Task 1 and Task 2 tests together. (Do not commit Task 1 alone.)

---

## Task 2: `DoctorDiagnostics` value-struct + `capture`

**Files:**
- Create: `lingxi-core/crates/tui/src/screens/doctor.rs`
- Test: `lingxi-core/crates/tui/src/screens/doctor.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** (in a new `doctor.rs`, at the bottom)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_builds_expected_fields() {
        let diag = DoctorDiagnostics {
            cli_version: "lingxi-cli v0.8.0".into(),
            rust_toolchain: "1.82.0".into(),
            claude_home: "/home/u/.claude".into(),
            cwd: "/work/proj".into(),
            mcp_configured: 2,
            mcp_connected: 0,
            auth_state: "unknown".into(),
            truecolor: true,
            term_size: (120, 40),
        };
        // Round-trips as a plain value (Clone + Eq used by the open intercept).
        assert_eq!(diag.clone(), diag);
        assert_eq!(diag.mcp_configured, 2);
        assert_eq!(diag.mcp_connected, 0);
    }

    #[test]
    fn truecolor_detect_reads_colorterm() {
        assert!(truecolor_from_env(Some("truecolor")));
        assert!(truecolor_from_env(Some("24bit")));
        assert!(!truecolor_from_env(Some("256color")));
        assert!(!truecolor_from_env(None));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p lingxi-tui --lib truecolor_detect_reads_colorterm`
Expected: FAIL — `screens::doctor` module empty / `truecolor_from_env` not defined.

- [ ] **Step 3: Write the struct + capture helpers** at the top of `doctor.rs`

```rust
//! Doctor screen — the FIRST full-page TUI screen (M7-11).
//!
//! Mirrors claude-code `src/screens/Doctor.tsx`: bold section headers
//! (`Diagnostics`, `Updates`) with `└ `-prefixed detail rows. v0.8.0 shows
//! the LingXi-relevant subset (versions, config paths, MCP servers, auth,
//! terminal capabilities); claude-code-internal sections (sandbox, version
//! locks, plugin/agent parse errors, context warnings) are out of scope.
//!
//! The screen is a PURE iocraft component fed a `DoctorDiagnostics` value
//! captured once at open time. No async, no `OrchestratorHandle` call on the
//! render path — M7 is TUI-only.

use iocraft::prelude::*;

/// Plain value-struct: everything the Doctor screen renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorDiagnostics {
    /// e.g. "lingxi-cli v0.8.0".
    pub cli_version: String,
    /// e.g. "1.82.0".
    pub rust_toolchain: String,
    /// Absolute claude config home.
    pub claude_home: String,
    /// Working directory.
    pub cwd: String,
    /// MCP servers configured (any state).
    pub mcp_configured: u32,
    /// MCP servers currently connected.
    pub mcp_connected: u32,
    /// Auth state label ("unknown" until OAuth lands in M8).
    pub auth_state: String,
    /// Whether the terminal advertises truecolor.
    pub truecolor: bool,
    /// Terminal size at open time (cols, rows).
    pub term_size: (u16, u16),
}

/// `true` iff `$COLORTERM` is `truecolor` or `24bit`.
#[must_use]
pub fn truecolor_from_env(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some("truecolor") | Some("24bit"))
}

impl DoctorDiagnostics {
    /// Build the diagnostics from the live status snapshot + terminal info.
    /// Called once when `/doctor` opens the screen. `mcp_configured` /
    /// `mcp_connected` come from the orchestrator status the TUI already
    /// surfaces (M6-07); v0.8.0 connected is 0 until MCP auto-connect (M8).
    #[must_use]
    pub fn capture(
        cwd: &std::path::Path,
        mcp_configured: u32,
        mcp_connected: u32,
        term_size: (u16, u16),
    ) -> Self {
        Self {
            cli_version: format!("lingxi-cli v{}", env!("CARGO_PKG_VERSION")),
            rust_toolchain: rust_toolchain_version(),
            claude_home: claude_home_dir(),
            cwd: cwd.display().to_string(),
            mcp_configured,
            mcp_connected,
            auth_state: "unknown".to_string(),
            truecolor: truecolor_from_env(std::env::var("COLORTERM").ok().as_deref()),
            term_size,
        }
    }
}

/// Rust toolchain version. Sourced from the pinned `rust-toolchain.toml`
/// value (1.82.0); we surface a fixed string rather than shelling out to
/// `rustc -V` on the render path (no I/O in a screen). M8 may upgrade this
/// to a build-time `RUSTC_VERSION` constant.
fn rust_toolchain_version() -> String {
    "1.82.0".to_string()
}

/// Claude config home dir as a display string. Reuses the standard
/// config-dir resolution the rest of the workspace uses (`$CLAUDE_CONFIG_DIR`
/// → `~/.claude`). Falls back to "~/.claude" when the home dir is unknown.
fn claude_home_dir() -> String {
    if let Ok(explicit) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !explicit.is_empty() {
            return explicit;
        }
    }
    match dirs::home_dir() {
        Some(h) => h.join(".claude").display().to_string(),
        None => "~/.claude".to_string(),
    }
}
```

> **Note on `dirs`:** the workspace already depends on `dirs` for config-dir resolution (used by settings/memory). If `lingxi-tui`'s `Cargo.toml` does not yet list it, add `dirs` (matching the version already in the workspace lockfile — do NOT introduce a new major) under `[dependencies]`. This is the only dependency touch in this plan; if it is already a transitive workspace dep promoted to direct, no change is needed.

- [ ] **Step 4: Run both Task 1 + Task 2 tests**

Run: `cargo test -p lingxi-tui --lib active_screen_defaults_none capture_builds_expected_fields truecolor_detect_reads_colorterm`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/screens/mod.rs lingxi-core/crates/tui/src/screens/doctor.rs lingxi-core/crates/tui/src/state.rs lingxi-core/crates/tui/Cargo.toml
git commit -m "$(cat <<'EOF'
plan(M7-11 T1): active_screen route-state + DoctorDiagnostics value-struct

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: priority-2 screen routing in `handle_live_key`

**Files:**
- Modify: `lingxi-core/crates/tui/src/root.rs`
- Test: `lingxi-core/crates/tui/tests/behavior_doctor_screen.rs` (created here; expanded in Task 6)

- [ ] **Step 1: Write the failing test** (new file `tests/behavior_doctor_screen.rs`)

```rust
//! M7-11 behavior tests: screen-overlay routing through the SINGLE live-key
//! dispatcher. Drives `root::handle_live_key` — the exact function the live
//! `use_terminal_events` closure invokes.

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lingxi_tui::root::handle_live_key;
use lingxi_tui::screens::Screen;
use lingxi_tui::screens::doctor::DoctorDiagnostics;
use lingxi_tui::state::{AppState, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

fn diag() -> DoctorDiagnostics {
    DoctorDiagnostics::capture(std::path::Path::new("/work"), 0, 0, (80, 24))
}

#[test]
fn esc_closes_active_screen() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.open_doctor(diag());
    assert_eq!(st.active_screen, Some(Screen::Doctor));
    handle_live_key(&mut st, &key(KeyCode::Esc), 24);
    assert_eq!(st.active_screen, None, "Esc closes the screen → back to REPL");
}

#[test]
fn q_closes_active_screen() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.open_doctor(diag());
    handle_live_key(&mut st, &key(KeyCode::Char('q')), 24);
    assert_eq!(st.active_screen, None, "q closes the screen");
}

#[test]
fn text_key_does_not_leak_to_prompt_while_screen_open() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.prompt_text = "draft".to_string();
    st.prompt_cursor = 5;
    st.open_doctor(diag());
    handle_live_key(&mut st, &key(KeyCode::Char('h')), 24);
    assert_eq!(st.prompt_text, "draft", "text key must NOT reach PromptInput");
    assert_eq!(st.active_screen, Some(Screen::Doctor), "non-close key keeps screen open");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p lingxi-tui --test behavior_doctor_screen`
Expected: FAIL — `handle_live_key` does not branch on `active_screen`, so `Esc` falls through (no-op or wrong) and `h` mutates `prompt_text` to `"drafth"`.

- [ ] **Step 3: Add the priority-2 branch + `handle_screen_key`** in `root.rs`

Insert into `handle_live_key`, **immediately after** the focus-trap block's `// === end focus trap ===` comment and **before** `let prompt_empty = ...`:

```rust
    // === PRIORITY 2: a full-page screen owns all keys while open. ===
    // Priority order (parent spec §2.5): permission (1, above) → screen (2,
    // here) → input/scroll (below). The permission check above STILL fires
    // first and returns, so a screen can never steal a permission key.
    if st.active_screen.is_some() {
        handle_screen_key(st, k);
        return;
    }
    // === end screen routing ===
```

Add the helper near `handle_live_key`:

```rust
/// Route a key to the active full-page screen. Esc / `q` close the screen
/// (back to the REPL); every other key is swallowed so it cannot leak to
/// `PromptInput` (the M7-11 no-leak guarantee). Per-screen interactive keys
/// (arrow-select, Enter) are added by M7-12/13/14 as `match`-on-`Screen`
/// arms here.
fn handle_screen_key(st: &mut AppState, k: &KeyEvent) {
    match k.code {
        KeyCode::Esc => st.close_screen(),
        KeyCode::Char('q') if k.modifiers == KeyModifiers::NONE => st.close_screen(),
        // Doctor is read-only; other keys are inert. Future screens add arms.
        _ => {}
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p lingxi-tui --test behavior_doctor_screen`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/root.rs lingxi-core/crates/tui/tests/behavior_doctor_screen.rs
git commit -m "$(cat <<'EOF'
plan(M7-11 T2): priority-2 screen routing in handle_live_key (Esc/q close, no leak)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `DoctorScreen` component + snapshot

**Files:**
- Modify: `lingxi-core/crates/tui/src/screens/doctor.rs`
- Test: `lingxi-core/crates/tui/tests/render_doctor_screen.rs` (create)

- [ ] **Step 1: Write the failing snapshot test** (new file `tests/render_doctor_screen.rs`)

```rust
//! Snapshot test for `DoctorScreen` at a fixed diagnostic state.
//! Locks the section headers (`Diagnostics`, `Updates`) + `└ ` row layout.

use lingxi_tui::screens::doctor::{DoctorDiagnostics, DoctorScreen};

fn fixed_diag() -> DoctorDiagnostics {
    DoctorDiagnostics {
        cli_version: "lingxi-cli v0.8.0".into(),
        rust_toolchain: "1.82.0".into(),
        claude_home: "/home/u/.claude".into(),
        cwd: "/work/proj".into(),
        mcp_configured: 2,
        mcp_connected: 0,
        auth_state: "unknown".into(),
        truecolor: true,
        term_size: (120, 40),
    }
}

#[test]
fn doctor_screen_fixed_state() {
    let mut element = iocraft::prelude::element! {
        DoctorScreen(diag: fixed_diag())
    };
    let rendered = element.to_string();
    insta::assert_snapshot!("doctor_screen_fixed", rendered);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p lingxi-tui --test render_doctor_screen`
Expected: FAIL — `DoctorScreen` not defined.

- [ ] **Step 3: Write the component** (append to `doctor.rs`, before the `#[cfg(test)]` module)

```rust
/// Props for [`DoctorScreen`]: the captured diagnostics to render.
#[derive(Default, Props)]
pub struct DoctorScreenProps {
    /// Diagnostics captured at open time. `Default` renders an empty shell
    /// (only used by iocraft's prop defaulting; the live path always sets it).
    pub diag: Option<DoctorDiagnostics>,
}

/// The Doctor screen — a pure full-page diagnostic view (M7-11).
#[component]
pub fn DoctorScreen(props: &DoctorScreenProps) -> impl Into<AnyElement<'static>> {
    let d = props.diag.clone().unwrap_or(DoctorDiagnostics {
        cli_version: String::new(),
        rust_toolchain: String::new(),
        claude_home: String::new(),
        cwd: String::new(),
        mcp_configured: 0,
        mcp_connected: 0,
        auth_state: "unknown".into(),
        truecolor: false,
        term_size: (0, 0),
    });

    // Locked literals (claude-code Doctor.tsx parity):
    //   section headers: "Diagnostics", "Updates" (bold)
    //   detail prefix:   "└ "
    let mcp_status = if d.mcp_configured == 0 {
        "none configured".to_string()
    } else {
        format!(
            "{} configured, {} connected",
            d.mcp_configured, d.mcp_connected
        )
    };
    let truecolor = if d.truecolor { "yes" } else { "no" };
    let size = format!("{}x{}", d.term_size.0, d.term_size.1);

    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            Text(content: "Diagnostics", weight: Weight::Bold)
            Text(content: format!("└ Version: {}", d.cli_version))
            Text(content: format!("└ Rust toolchain: {}", d.rust_toolchain))
            Text(content: format!("└ Claude home: {}", d.claude_home))
            Text(content: format!("└ Working dir: {}", d.cwd))
            Text(content: format!("└ MCP servers: {mcp_status}"))
            Text(content: format!("└ Auth: {}", d.auth_state))
            Text(content: "")
            Text(content: "Terminal", weight: Weight::Bold)
            Text(content: format!("└ Truecolor: {truecolor}"))
            Text(content: format!("└ Size: {size}"))
            Text(content: "")
            Text(content: "Press Esc or q to return", color: Color::DarkGrey)
        }
    }
}
```

> Match the exact `Text` prop names used elsewhere in this crate (`weight: Weight::Bold`, `color: Color::DarkGrey`). If the local iocraft 0.8.3 uses a different dim mechanism than `status_line.rs`, copy that file's mechanism verbatim rather than guessing.

- [ ] **Step 4: Run + accept the snapshot**

Run: `cargo test -p lingxi-tui --test render_doctor_screen`
Then: `cargo insta review` (or `cargo insta accept`) from inside `lingxi-core/`.
Re-run the test. Expected: PASS. Confirm the snapshot contains `Diagnostics`, `Updates`-style `Terminal` header, `└ Version: lingxi-cli v0.8.0`, and `└ MCP servers: 2 configured, 0 connected`.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/screens/doctor.rs lingxi-core/crates/tui/tests/render_doctor_screen.rs lingxi-core/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-11 T3): DoctorScreen component + fixed-state snapshot

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: render the active screen + `/doctor` open intercept

**Files:**
- Modify: `lingxi-core/crates/tui/src/app.rs`
- Test: `lingxi-core/crates/tui/src/app.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests** (append to `app.rs` `dispatch_tests`)

```rust
    #[test]
    fn doctor_slash_opens_screen_via_dispatch() {
        use crate::screens::Screen;
        let mut st = s();
        // Submit a "/doctor" line through the same path the live Enter uses.
        st.prompt_text = "/doctor".to_string();
        st.prompt_cursor = "/doctor".len();
        let should_run = dispatch(KeyAction::Submit, &mut st);
        assert!(!should_run, "/doctor opens a screen, never runs a turn");
        assert_eq!(st.active_screen, Some(Screen::Doctor));
        assert!(st.doctor_diagnostics.is_some(), "diagnostics captured at open");
        assert!(st.prompt_text.is_empty(), "prompt cleared on submit");
        // No UserText pushed for the intercepted slash command.
        assert!(
            !matches!(st.messages.last(), Some(RenderedMessage::UserText { .. })),
            "/doctor must not echo as a user message"
        );
    }

    #[test]
    fn render_screen_renders_doctor_when_active() {
        use crate::screens::doctor::DoctorDiagnostics;
        let mut st = s();
        st.open_doctor(DoctorDiagnostics::capture(
            std::path::Path::new("/work"), 1, 0, (80, 24),
        ));
        let mut element = render_screen(&st, 20);
        let rendered = element.to_string();
        assert!(rendered.contains("Diagnostics"), "got: {rendered}");
        assert!(!rendered.contains("claude-sonnet-4.5"), "REPL status hidden while screen up");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p lingxi-tui --lib doctor_slash_opens_screen_via_dispatch render_screen_renders_doctor_when_active`
Expected: FAIL — `dispatch` doesn't intercept `/doctor`; `render_screen` doesn't branch on `active_screen`.

- [ ] **Step 3a: Intercept `/doctor` in the Submit branch** of `dispatch` (`app.rs`)

In `dispatch`, in the `KeyAction::Submit` arm, **after** the `if st.prompt_text.is_empty() { return false; }` guard and **before** `let line = std::mem::take(...)`, add:

```rust
            // (M7-11) `/doctor` opens the Doctor screen instead of echoing /
            // running a turn. Intercept here (the live submit path) the same
            // way `handle_submit_line` intercepts `/clear` / `/exit`. The
            // stdio `--no-tui` `/doctor` text report is unchanged.
            if st.prompt_text.trim() == "/doctor" {
                let diag = crate::screens::doctor::DoctorDiagnostics::capture(
                    &st.status.cwd,
                    st.status.mcp_configured,
                    st.status.mcp_connected,
                    st.status.term_size,
                );
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.open_doctor(diag);
                return false;
            }
```

> **`StatusSnapshot` fields needed:** the TUI `StatusSnapshot` (in `state.rs`) does not yet carry `mcp_configured`/`mcp_connected`/`term_size`. Add these three fields to `state.rs::StatusSnapshot` (defaulting to `0`/`0`/`(0,0)`), populated from the orchestrator status the bridge already receives (M6-07 wired MCP counts into the status path; if not yet surfaced to the TUI `StatusSnapshot`, default to `0` and document — the parent spec accepts "configured, not connected"/`0` for v0.8.0). Update `StatusSnapshot::Default` + the test constructors accordingly. This keeps the screen pure (no handle call on submit).

- [ ] **Step 3b: Render the active screen** in `render_screen` (`app.rs`)

Immediately **after** the `if let Some(pp) = &state.pending_permission { ... return ...; }` permission overlay block, add:

```rust
    // (M7-11) Screen overlay: a full-page screen renders INSTEAD OF the REPL
    // (same render-instead-of discipline as the permission overlay above — no
    // z-index primitive in iocraft 0.8). Reused by M7-12/13/14.
    if let Some(screen) = state.active_screen {
        use crate::screens::Screen;
        return match screen {
            Screen::Doctor => {
                use crate::screens::doctor::DoctorScreen;
                let diag = state.doctor_diagnostics.clone();
                element! { DoctorScreen(diag: diag) }.into_any()
            }
        };
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lingxi-tui --lib doctor_slash_opens_screen_via_dispatch render_screen_renders_doctor_when_active`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/app.rs lingxi-core/crates/tui/src/state.rs
git commit -m "$(cat <<'EOF'
plan(M7-11 T4): render active screen overlay + /doctor open intercept

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: the §4 R4 permission-priority seam test

**Files:**
- Modify: `lingxi-core/crates/tui/tests/behavior_doctor_screen.rs`

This is the parent spec's §4 R4 / §5.6 cross-state seam, and the single most important test in this plan: **a screen open while a permission is pending must yield to the permission (priority 1 > 2), and the permission key must resolve the dialog — not the screen.**

- [ ] **Step 1: Write the failing test** (append to `behavior_doctor_screen.rs`)

```rust
use lingxi_permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use lingxi_tui::state::PendingPermission;
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;

/// §4 R4 SEAM: Doctor screen open WHILE a permission is pending. The
/// permission (priority 1) wins; its key resolves the DIALOG, not the screen.
#[tokio::test]
async fn permission_wins_over_open_screen() {
    let mut st = AppState::new(StatusSnapshot::default());

    // Doctor screen is open...
    st.open_doctor(diag());
    assert_eq!(st.active_screen, Some(Screen::Doctor));

    // ...and a permission arrives on top of it.
    let (tx, rx) = oneshot::channel();
    st.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    st.pending_permission_resp_tx = Some(tx);
    st.pending_permission_started_at = Some(std::time::Instant::now());

    // Press `1` (ToolUseConfirm: AllowOnce). Priority 1 fires FIRST.
    handle_live_key(&mut st, &key(KeyCode::Char('1')), 24);

    // The dialog resolved...
    let resp = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("permission key must resolve the dialog (priority 1 > 2)")
        .expect("oneshot not dropped");
    assert_eq!(resp, PermissionResponse::AllowOnce);
    assert!(st.pending_permission.is_none(), "dialog cleared");

    // ...and the screen is UNTOUCHED — `1` did not close or alter it.
    assert_eq!(
        st.active_screen,
        Some(Screen::Doctor),
        "screen must be unaffected: the permission consumed the key"
    );
}

/// Once the permission is gone, the SAME dispatcher routes the next key to the
/// screen (priority 2): `q` now closes Doctor.
#[test]
fn screen_routing_resumes_after_permission_clears() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.open_doctor(diag());
    // No pending permission → priority 2 owns the key.
    handle_live_key(&mut st, &key(KeyCode::Char('q')), 24);
    assert_eq!(st.active_screen, None);
}
```

- [ ] **Step 2: Run it to verify it passes**

Run: `cargo test -p lingxi-tui --test behavior_doctor_screen permission_wins_over_open_screen screen_routing_resumes_after_permission_clears`
Expected: **PASS** — Task 3 already placed the screen branch *after* the permission `return`, so this should pass on first run. If it FAILS (the screen branch was placed before the permission check, or the permission check stopped `return`ing), that is the §4 R4 regression — STOP and fix the ordering in `handle_live_key` so the permission block stays the first statement and still `return`s. Do not weaken the test.

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/crates/tui/tests/behavior_doctor_screen.rs
git commit -m "$(cat <<'EOF'
plan(M7-11 T5): R4 seam test — permission (priority 1) wins over open screen

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: docs + telemetry note (0 new events)

**Files:**
- Modify: `lingxi-core/crates/tui/src/screens/mod.rs` (module doc — already done in Task 1; verify)
- Modify: `lingxi-core/crates/tui/src/telemetry.rs` (add a deferral note comment only)

- [ ] **Step 1: Add the telemetry deferral note** to `telemetry.rs`

Append to the inventory doc comment at the top of `telemetry.rs`:

```rust
//! - M7-11: screen lifecycle events (`tengu_tui_screen_opened` /
//!   `_closed`) are a CANDIDATE but DEFERRED to the M7-16 telemetry audit.
//!   M7-11 adds 0 new events (baseline stays 326). Do not register a name
//!   here without a real emit site — that is the M6 "330 vs 326" lesson.
```

- [ ] **Step 2: Verify no new event name was registered**

Run (from inside `lingxi-core/`):
```bash
cargo test -p lingxi-telemetry 2>&1 | tail -5
```
Expected: PASS — `ALL_EVENT_NAMES.len()` unchanged at 326 (no `tengu_tui_screen_*` constant was added). If the count changed, you registered an event — remove it (M7-11 is 0 new events).

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/crates/tui/src/telemetry.rs lingxi-core/crates/tui/src/screens/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-11 T6): telemetry note — screen events deferred to M7-16 (0 new events)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: workspace verification gate + tag `m7.11`

**Files:** none (verification + tag only).

- [ ] **Step 1: Run the full workspace gate FROM INSIDE `lingxi-core/`**

The toolchain pins rust 1.82.0; running from the repo root uses the host toolchain → spurious lint noise (this bit M6-08). Run **every** command below with the working directory inside `lingxi-core/`:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: `fmt` clean; `clippy` zero warnings; `cargo test` green.

**Known flakes (allowed a single rerun — NOT failures):** `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, and the `lingxi-platform-posix` fs_watch FSEvents timing tests. If only these fail, rerun once; if they pass on rerun, the gate is green.

- [ ] **Step 2: Cross-platform compile check (5 targets)**

```bash
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```
Expected: all green. (If a target's toolchain component isn't installed locally, document which and defer to the M7-16 cross-platform gate, matching the v0.7.0 posture.)

- [ ] **Step 3: Confirm the four required tests all pass together**

```bash
cargo test -p lingxi-tui --test behavior_doctor_screen
cargo test -p lingxi-tui --test render_doctor_screen
```
Expected: PASS. The four behaviors the parent prompt requires are all present:
1. Snapshot at fixed diagnostic state — `render_doctor_screen::doctor_screen_fixed_state`.
2. `/doctor` opens / `Esc`+`q` close — `doctor_slash_opens_screen_via_dispatch` + `esc_closes_active_screen` + `q_closes_active_screen`.
3. §4 R4 permission-priority seam — `permission_wins_over_open_screen`.
4. No text leak to `PromptInput` while open — `text_key_does_not_leak_to_prompt_while_screen_open`.

- [ ] **Step 4: Tag the sub-plan**

```bash
git tag -a m7.11 -m "M7-11: Doctor screen + screen-overlay infrastructure (active_screen, priority-2 routing)"
```
(Local annotated tag only — no remote push, no force-push, per parent spec §6.4.)

- [ ] **Step 5: Final self-check against the parent spec**

Confirm, by re-reading the diff:
- The permission focus-trap block in `handle_live_key` is still the FIRST statement and still `return`s (§4 R4 preserved).
- The screen branch is priority 2 (after permission, before input).
- `render_screen` renders Doctor instead of the REPL when `active_screen.is_some()`.
- `Screen` enum + `active_screen` field + `handle_screen_key` are generic enough that M7-12/13/14 add only a variant + a render arm + a key arm.
- 0 new telemetry events.

---

## Self-Review (run after writing all tasks)

**1. Spec coverage** (parent spec §3 "M7-11" entry + the WHAT-M7-11-SHIPS list):
- `AppState.active_screen: Option<Screen>` enum (Doctor variant; Resume/Settings/Memory commented for later) → Task 1. ✓
- Priority-2 branch in `handle_live_key`, Esc/`q` closes, focus-trap not broken → Tasks 3 + 6. ✓
- `screens/doctor.rs` diagnostic rows (versions, config paths, MCP configured/connected, auth, terminal caps) → Tasks 2 + 4. ✓
- `/doctor` opens it; REPL yields to Doctor → Task 5. ✓
- Reads diagnostics from data the handle already exposes; "not connected"/"unknown" otherwise → Task 2 (`capture`, `auth_state="unknown"`, connected=0). ✓
- 0 new telemetry events, note the deferral → Task 7. ✓
- Workspace gate from inside `lingxi-core/` + tag `m7.11` → Task 8. ✓
- Tests: snapshot, open/close, R4 seam, no-leak → Tasks 4/5/3/6/8. ✓

**2. Placeholder scan:** no TBD/TODO/"handle edge cases"; every code step shows the actual code; the snapshot literals are concrete.

**3. Type consistency:** `Screen`, `DoctorDiagnostics`, `DoctorScreen`, `DoctorScreenProps`, `active_screen`, `doctor_diagnostics`, `open_doctor`, `close_screen`, `handle_screen_key`, `capture`, `truecolor_from_env` are used identically across Tasks 1–8. `StatusSnapshot` gains `mcp_configured`/`mcp_connected`/`term_size` (Task 5 §3a) which `capture` consumes (Task 2). The TUI `StatusSnapshot` (state.rs) is distinct from the orchestrator's `StatusSnapshot` (traits) — Task 5 notes the field additions go on the TUI struct.

---

## Notes, gaps & decisions

- **Reusable infra (Tasks 1–3) is the load-bearing deliverable.** M7-12/13/14 will: add a `Screen` variant, add a `render_screen` match arm, add a `handle_screen_key` match arm (for their interactive keys), and a per-screen captured-state field on `AppState`. No re-plumbing of the dispatcher or the overlay seam.
- **Diagnostics are captured, not live.** The screen renders a frozen `DoctorDiagnostics` snapshot taken at open time. This keeps the render path pure (no async, no `OrchestratorHandle` call mid-render) and makes the snapshot test deterministic. claude-code's Doctor re-runs async checks; matching that is an M8 nicety, documented here.
- **MCP / auth values are intentionally degraded for v0.8.0.** MCP shows "configured, N connected" with connected typically 0 (auto-connect is M8); auth is `"unknown"` (OAuth is M8). The parent spec §0 Q1 / §3 explicitly accept this.
- **`StatusSnapshot` field additions (Task 5 §3a)** are the only state-shape change beyond `active_screen`/`doctor_diagnostics`. If M6-07 already surfaces MCP counts onto the TUI `StatusSnapshot` under different names, reuse those instead of adding new fields, and adjust `capture`'s call site — do not duplicate.
- **`rust_toolchain_version()` returns a fixed `"1.82.0"` string** rather than shelling out to `rustc -V` (no I/O on a screen). A build-time `RUSTC_VERSION` constant is an M8 polish item.
- **`dirs` dependency:** the only potential `Cargo.toml` touch. It is already in the workspace lockfile (settings/memory use it); promote it to a direct `lingxi-tui` dep if not already present, matching the existing version.
- **Out of scope (documented, not gaps):** claude-code Doctor's sandbox/version-lock/plugin/agent-parse/context-warning sections; live re-running of checks; screen-open/close telemetry (M7-16). These are deliberate per the parent spec's M7-11 scope.
```
