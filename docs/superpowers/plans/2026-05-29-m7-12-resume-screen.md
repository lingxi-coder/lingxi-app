# M7-12 — Resume Screen (iocraft) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an iocraft Resume screen that lists recent sessions, previews the selected one, resumes on Enter, cancels on Esc — and wire `--resume` (no id) in TTY mode to open it, while keeping the M5-08 stdio picker as the `--no-tui` fallback.

**Architecture:** A new full-page screen `screens/resume.rs` renders against the existing M5-08 session loader (`lingxi_session::jsonl::loader::{SessionMetadata, list_recent_sessions, load_session}`) — **zero engine change**. The screen is a modal route state added to `AppState.active_screen: Option<Screen>` (the enum M7-11 introduced) as a new `Screen::Resume` variant, routed through the priority-2 branch of the single `handle_live_key` dispatcher (spec §2.5). CLI dispatch (`lingxi-cli`) splits `--resume` with no id: in TTY mode it opens the iocraft screen; under `--no-tui` (or non-TTY) it falls through to the unchanged M5-08 stdio picker (`select_session_interactive`).

**Tech Stack:** Rust 1.82 (pinned via `rust-toolchain`; **run all cargo from inside `lingxi-core/`**), iocraft `=0.8.3` (`View`, not `Box`), `lingxi-session` loader (`uuid`, `chrono`, `SystemTime`), `insta` snapshots, `tempfile` for loader fixtures.

---

## Prerequisite — M7-11 infrastructure (read before starting)

M7-12 builds on the screen-overlay infrastructure that **M7-11 (Doctor screen)** establishes. Reference: `docs/superpowers/plans/2026-05-29-m7-11-doctor-screen.md` and `crates/tui/src/screens/doctor.rs`. M7-11 lands these three things in `crates/tui/src/`:

1. **`state.rs`:** a `Screen` enum and `AppState.active_screen: Option<Screen>`. After M7-11 the enum reads:
   ```rust
   /// Active full-page screen overlay. `None` = REPL. Set/cleared by the
   /// priority-2 branch of `handle_live_key`; rendered by `render_screen`.
   #[derive(Debug, Clone)]
   pub enum Screen {
       /// Diagnostics screen (M7-11).
       Doctor(crate::screens::doctor::DoctorState),
   }
   ```
   plus the field `pub active_screen: Option<Screen>,` on `AppState` (initialized `None` in `AppState::new`).

2. **`root.rs` `handle_live_key`:** a **priority-2 branch** placed *after* the permission focus-trap and *before* the default edit/scroll path:
   ```rust
   // === PRIORITY 2: an active screen owns all keys while open. ===
   if st.active_screen.is_some() {
       crate::screens::handle_screen_key(st, k);
       return;
   }
   // === end active-screen trap ===
   ```
   where `screens::handle_screen_key(st, k)` dispatches on the active `Screen` variant and clears `active_screen` (returns to REPL) on `Esc`.

3. **`app.rs` `render_screen`:** a branch placed *after* the `pending_permission` block and *before* the `ReplScreen` assembly:
   ```rust
   if let Some(screen) = &state.active_screen {
       return crate::screens::render_active_screen(screen);
   }
   ```
   plus `screens::render_active_screen(&Screen) -> AnyElement<'static>` and `screens::handle_screen_key(&mut AppState, &KeyEvent)` defined in `crate::screens` (`screens/mod.rs`).

**If M7-11 has NOT landed when you start:** do not invent it ad-hoc. Add a single precursor commit that introduces exactly the three items above with the `Doctor` variant omitted (i.e. `Screen` becomes an enum with only the `Resume` variant this plan adds, and `handle_screen_key`/`render_active_screen` match only `Resume`). Tasks 4–8 below assume the `Screen` enum, `handle_screen_key`, and `render_active_screen` exist; the only M7-12-specific change is **adding the `Resume` variant + its arms**. The literals, signatures, and routing in this plan are written so they compose either way.

---

## File Structure

| File | Responsibility | Create / Modify |
|---|---|---|
| `crates/tui/Cargo.toml` | Add `lingxi-session` path dep (screen reuses the loader types) | Modify |
| `crates/tui/src/screens/resume.rs` | The Resume screen: `ResumeRow` view-model, `ResumeState` (rows + selection), pure `handle_resume_key`, `ResumeScreen` iocraft component, empty-state, preview pane | Create |
| `crates/tui/src/screens/mod.rs` | `pub mod resume;`; extend `Screen` enum with `Resume(ResumeState)`; extend `handle_screen_key` + `render_active_screen` match arms | Modify |
| `crates/tui/src/state.rs` | (M7-11 owns `Screen`/`active_screen`) — only touched if adding the `Resume` variant here | Modify (variant only) |
| `crates/cli/src/run.rs` | `run_resume`: when arg is empty + TTY, build the loader rows and hand off to the TUI resume path; keep the concrete-id + `--no-tui` stdio paths | Modify |
| `crates/cli/src/lib.rs` | Split the `parsed.resume.is_some()` dispatch so the TTY-empty-arg case reaches the iocraft screen (via `run_resume`'s new branch) | Modify |
| `crates/tui/src/session.rs` | Add a TUI entry that seeds `active_screen = Some(Screen::Resume(..))` so the binary opens directly on the picker | Modify |
| `crates/tui/tests/render_resume_screen.rs` | Snapshot: 3 sessions + preview; empty-state | Create |
| `crates/tui/tests/behavior_resume_screen.rs` | Behavior: list renders N rows; arrow select; Enter → right uuid; Esc cancels; empty set → empty-state | Create |
| `crates/cli/src/mode.rs` | (read-only reference — `decide_mode` already exposes TTY decision) | Reference |

**View-model boundary (important):** the screen does **not** depend on iocraft for its logic. `ResumeRow` + `ResumeState` + `handle_resume_key` are plain data + pure functions (unit-testable without a terminal). The `ResumeScreen` component is a thin render over `ResumeState`. This mirrors the M6-05 permission-dialog split (`ExitPlanModeState` + pure `handle_key` + `ExitPlanMode` component).

---

## Literal lock (from `claude-code/src/screens/ResumeConversation.tsx` + `components/LogSelector.tsx`)

Copy these byte-for-byte (spec §2.8):

- Empty state (two lines, `LogSelector`):
  - `"No conversations found to resume."`
  - `"Press Ctrl+C to exit and start a new conversation."` (dim)
- Loading: `" Loading conversations…"` (leading space + ellipsis char `…`, U+2026) — used only if a future async-load lands; M7-12 loads synchronously before mount, so this is **not** rendered (documented, no dead literal).
- Resuming: `" Resuming conversation…"` — shown for one frame after Enter, before the screen closes (optional; if you don't render a transient resuming frame, omit it rather than leave it dead).

Columns in the list mirror `LogSelector` rows: title (truncated by the loader to ≤ 50 chars + ellipsis), then a bracketed `[modified]` timestamp, then a `(N messages)` count. M7-12 layout:

```
  1. <title>                       [2026-05-24T19:03:12Z]  (12 messages)
> 2. <title>                       [2026-05-24T18:55:01Z]  (3 messages)
  3. <title>                       [2026-05-24T18:40:00Z]  (1 message)
```

The selected row is prefixed `> ` (matching the permission-dialog focus convention `format!("> {label}")`); unselected rows `  ` (two spaces). Singular/plural: `(1 message)` vs `(N messages)`.

---

## Tasks

### Task 1: Add `lingxi-session` dependency to `lingxi-tui`

**Files:**
- Modify: `crates/tui/Cargo.toml`

- [ ] **Step 1: Add the path dependency**

In `crates/tui/Cargo.toml` under `[dependencies]`, after the `lingxi-commands` line, add:

```toml
lingxi-session = { path = "../session" }
```

- [ ] **Step 2: Verify it resolves**

Run (from inside `lingxi-core/`):

```bash
cargo check -p lingxi-tui
```

Expected: compiles (no new code yet). If a workspace cycle is reported, STOP — `lingxi-session` must not depend on `lingxi-tui`; it does not today (it depends only on `lingxi-traits`/`lingxi-protocol`), so this should be clean.

- [ ] **Step 3: Commit**

```bash
git add crates/tui/Cargo.toml
git commit -m "$(cat <<'EOF'
plan(M7-12 T1): add lingxi-session dep to lingxi-tui for resume loader reuse

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `ResumeRow` view-model + RFC-3339 formatter (pure)

**Files:**
- Create: `crates/tui/src/screens/resume.rs`
- Test: inline `#[cfg(test)] mod tests` in `resume.rs`

The screen renders rows derived from `lingxi_session::jsonl::loader::SessionMetadata` (fields: `uuid: Uuid`, `title: String`, `modified: SystemTime`, `message_count: usize`, `path: PathBuf`). We map that to a display row so the component never touches `SystemTime`/`PathBuf` directly.

- [ ] **Step 1: Write the failing test**

Create `crates/tui/src/screens/resume.rs` with only this test module first (the types it references are added in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};
    use uuid::Uuid;

    fn meta(title: &str, secs: u64, count: usize) -> SessionMetadata {
        SessionMetadata {
            uuid: Uuid::nil(),
            title: title.to_string(),
            modified: UNIX_EPOCH + Duration::from_secs(secs),
            message_count: count,
            path: std::path::PathBuf::from("/tmp/x.jsonl"),
        }
    }

    #[test]
    fn row_formats_timestamp_and_singular_plural() {
        // 1748113392 = 2025-05-24T20:23:12Z (an arbitrary fixed epoch second).
        let row_one = ResumeRow::from_meta(&meta("hello", 1_748_113_392, 1));
        assert_eq!(row_one.modified_label, "2025-05-24T20:23:12Z");
        assert_eq!(row_one.count_label, "(1 message)");

        let row_many = ResumeRow::from_meta(&meta("hi", 1_748_113_392, 12));
        assert_eq!(row_many.count_label, "(12 messages)");
    }

    #[test]
    fn row_keeps_uuid_and_title() {
        let m = meta("fix the bug", 0, 3);
        let row = ResumeRow::from_meta(&m);
        assert_eq!(row.title, "fix the bug");
        assert_eq!(row.uuid, m.uuid);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | head -20
```

Expected: FAIL to **compile** (`ResumeRow` / `SessionMetadata` not in scope).

- [ ] **Step 3: Write the minimal implementation**

Prepend this above the test module in `crates/tui/src/screens/resume.rs`:

```rust
//! Resume screen (M7-12) — an iocraft full-page view over the M5-08 session
//! loader (`lingxi_session::jsonl::loader`). Lists recent sessions, previews
//! the selected one, resumes on Enter, cancels on Esc.
//!
//! Logic/state (`ResumeRow`, `ResumeState`, `handle_resume_key`) are pure and
//! terminal-free; `ResumeScreen` is a thin render over `ResumeState`. Mirrors
//! the M6-05 permission-dialog split. Literals are byte-locked from
//! `claude-code/src/screens/ResumeConversation.tsx` + `components/LogSelector.tsx`.
#![forbid(unsafe_code)]

use std::time::{SystemTime, UNIX_EPOCH};

use iocraft::prelude::*;
use lingxi_session::jsonl::loader::SessionMetadata;
use uuid::Uuid;

/// One display row derived from a [`SessionMetadata`]. Terminal-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRow {
    /// Session UUID — the value Enter resolves to.
    pub uuid: Uuid,
    /// Title (already truncated to ≤ 50 chars + ellipsis by the loader).
    pub title: String,
    /// `[YYYY-MM-DDTHH:MM:SSZ]`-style timestamp body (without the brackets).
    pub modified_label: String,
    /// `(N message[s])` count label, singular for 1.
    pub count_label: String,
}

impl ResumeRow {
    /// Build a display row from a loader [`SessionMetadata`].
    #[must_use]
    pub fn from_meta(m: &SessionMetadata) -> Self {
        Self {
            uuid: m.uuid,
            title: m.title.clone(),
            modified_label: format_rfc3339_seconds(m.modified),
            count_label: if m.message_count == 1 {
                "(1 message)".to_string()
            } else {
                format!("({} messages)", m.message_count)
            },
        }
    }
}

/// RFC 3339, second precision, `Z` suffix — matches the M5-08 stdio picker's
/// `format_rfc3339_seconds` byte-for-byte so the two surfaces agree.
fn format_rfc3339_seconds(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    #[allow(clippy::cast_possible_wrap)]
    let secs_i64 = secs as i64;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs_i64, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}
```

- [ ] **Step 4: Run it to verify it passes**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | tail -10
```

Expected: PASS (2 tests). If `screens::resume` is "not found", add `pub mod resume;` to `crates/tui/src/screens/mod.rs` now (Task 5 formalizes the rest of `mod.rs`); the module must be declared for `--lib` tests to compile it.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/resume.rs crates/tui/src/screens/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T2): ResumeRow view-model + rfc3339 formatter over M5-08 loader

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `ResumeState` — rows + selection + preview source (pure)

**Files:**
- Modify: `crates/tui/src/screens/resume.rs`

`ResumeState` holds the rows and the selected index. The preview is the title + uuid + count of the selected row (claude-code shows the first/last message via LogSelector's expanded option; M7-12 previews from the metadata we already loaded — no extra file read, no engine change).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `resume.rs`:

```rust
#[test]
fn state_from_rows_selects_first() {
    let rows = vec![
        ResumeRow::from_meta(&meta("a", 0, 1)),
        ResumeRow::from_meta(&meta("b", 0, 2)),
    ];
    let st = ResumeState::new(rows);
    assert_eq!(st.selected, 0);
    assert_eq!(st.selected_uuid(), Some(Uuid::nil()));
    assert!(!st.is_empty());
}

#[test]
fn empty_state_has_no_selection() {
    let st = ResumeState::new(vec![]);
    assert!(st.is_empty());
    assert_eq!(st.selected_uuid(), None);
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | head -20
```

Expected: FAIL to compile (`ResumeState` not defined).

- [ ] **Step 3: Write the minimal implementation**

Add to `resume.rs` (above the test module):

```rust
/// Pure state for the Resume screen: the rows plus the selected index.
#[derive(Debug, Clone, Default)]
pub struct ResumeState {
    /// Display rows, newest-first (the loader already sorts mtime desc).
    pub rows: Vec<ResumeRow>,
    /// Index into `rows` of the highlighted row. Always `< rows.len()` when
    /// `rows` is non-empty; meaningless (0) when empty.
    pub selected: usize,
}

impl ResumeState {
    /// Build from display rows. Selects the first row.
    #[must_use]
    pub fn new(rows: Vec<ResumeRow>) -> Self {
        Self { rows, selected: 0 }
    }

    /// `true` when there are no sessions to resume (empty-state).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The currently selected row, if any.
    #[must_use]
    pub fn selected_row(&self) -> Option<&ResumeRow> {
        self.rows.get(self.selected)
    }

    /// The UUID of the selected row — the value Enter resolves to.
    #[must_use]
    pub fn selected_uuid(&self) -> Option<Uuid> {
        self.selected_row().map(|r| r.uuid)
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | tail -10
```

Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/resume.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T3): ResumeState rows + selection + selected_uuid

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `handle_resume_key` — arrow select, Enter, Esc (pure)

**Files:**
- Modify: `crates/tui/src/screens/resume.rs`

The pure key handler returns a `ResumeOutcome` so the caller (the screen-routing layer) decides what to do: stay open, resume a uuid, or cancel. This mirrors `DialogResolution` in the permission dialogs.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn down_moves_selection_clamped() {
    let mut st = ResumeState::new(vec![
        ResumeRow::from_meta(&meta("a", 0, 1)),
        ResumeRow::from_meta(&meta("b", 0, 1)),
    ]);
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Down)), ResumeOutcome::Stay);
    assert_eq!(st.selected, 1);
    // Past the end stays on the last row (no wrap).
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Down)), ResumeOutcome::Stay);
    assert_eq!(st.selected, 1);
}

#[test]
fn up_moves_selection_clamped() {
    let mut st = ResumeState::new(vec![
        ResumeRow::from_meta(&meta("a", 0, 1)),
        ResumeRow::from_meta(&meta("b", 0, 1)),
    ]);
    st.selected = 1;
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Up)), ResumeOutcome::Stay);
    assert_eq!(st.selected, 0);
    // Past the start stays on the first row.
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Up)), ResumeOutcome::Stay);
    assert_eq!(st.selected, 0);
}

#[test]
fn enter_resumes_selected_uuid() {
    let target = Uuid::from_u128(42);
    let mut second = meta("b", 0, 1);
    second.uuid = target;
    let mut st = ResumeState::new(vec![
        ResumeRow::from_meta(&meta("a", 0, 1)),
        ResumeRow::from_meta(&second),
    ]);
    handle_resume_key(&mut st, k(KeyCode::Down));
    assert_eq!(
        handle_resume_key(&mut st, k(KeyCode::Enter)),
        ResumeOutcome::Resume(target)
    );
}

#[test]
fn esc_cancels() {
    let mut st = ResumeState::new(vec![ResumeRow::from_meta(&meta("a", 0, 1))]);
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Esc)), ResumeOutcome::Cancel);
}

#[test]
fn enter_on_empty_cancels() {
    let mut st = ResumeState::new(vec![]);
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Enter)), ResumeOutcome::Cancel);
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | head -20
```

Expected: FAIL to compile (`handle_resume_key` / `ResumeOutcome` not defined).

- [ ] **Step 3: Write the minimal implementation**

Add to `resume.rs` (above the test module). Note: the workspace consumes crossterm-0.28 `KeyEvent` here (the same type the permission `handle_key`s take); `root.rs` already bridges iocraft 0.29 events to 0.28 via `iocraft_to_crossterm028_key`.

```rust
use crossterm::event::{KeyCode, KeyEvent};

/// What `handle_resume_key` tells the router to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Keep the screen open (selection moved, or an inert key).
    Stay,
    /// Resume the session with this UUID (Enter on a real row).
    Resume(Uuid),
    /// Close the screen, return to REPL (Esc, or Enter on empty-state).
    Cancel,
}

/// Pure key handler for the Resume screen.
///
/// - `Up`/`k` → move selection up (clamped at 0).
/// - `Down`/`j` → move selection down (clamped at `len-1`).
/// - `Enter` → resume the selected uuid; on empty-state → `Cancel`.
/// - `Esc` / `q` → cancel.
/// - anything else → `Stay`.
#[must_use]
pub fn handle_resume_key(state: &mut ResumeState, key: KeyEvent) -> ResumeOutcome {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.selected = state.selected.saturating_sub(1);
            ResumeOutcome::Stay
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !state.rows.is_empty() {
                state.selected = (state.selected + 1).min(state.rows.len() - 1);
            }
            ResumeOutcome::Stay
        }
        KeyCode::Enter => match state.selected_uuid() {
            Some(uuid) => ResumeOutcome::Resume(uuid),
            None => ResumeOutcome::Cancel,
        },
        KeyCode::Esc | KeyCode::Char('q') => ResumeOutcome::Cancel,
        _ => ResumeOutcome::Stay,
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | tail -10
```

Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/resume.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T4): pure handle_resume_key (arrow select / Enter / Esc)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `ResumeScreen` iocraft component (list + preview + empty-state)

**Files:**
- Modify: `crates/tui/src/screens/resume.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module (renders via `element.to_string()`, the convention used by `render_screen_smoke` in `app.rs`):

```rust
#[test]
fn component_renders_rows_and_selection_marker() {
    let st = ResumeState::new(vec![
        ResumeRow::from_meta(&meta("first session", 1_748_113_392, 3)),
        ResumeRow::from_meta(&meta("second session", 1_748_113_300, 1)),
    ]);
    let mut element = element! { ResumeScreen(state: st) };
    let frame = element.to_string();
    assert!(frame.contains("Resume which session?"), "got: {frame}");
    assert!(frame.contains("first session"), "got: {frame}");
    assert!(frame.contains("(3 messages)"), "got: {frame}");
    assert!(frame.contains("(1 message)"), "got: {frame}");
    // Selected (row 0) prefixed "> ", unselected "  ".
    assert!(frame.contains("> 1."), "got: {frame}");
    assert!(frame.contains("  2."), "got: {frame}");
}

#[test]
fn component_renders_empty_state() {
    let st = ResumeState::new(vec![]);
    let mut element = element! { ResumeScreen(state: st) };
    let frame = element.to_string();
    assert!(frame.contains("No conversations found to resume."), "got: {frame}");
    assert!(
        frame.contains("Press Ctrl+C to exit and start a new conversation."),
        "got: {frame}"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | head -20
```

Expected: FAIL to compile (`ResumeScreen` component not defined).

- [ ] **Step 3: Write the minimal implementation**

Add to `resume.rs` (above the test module). `ResumeState` is `Clone + Default`, so it can be a prop directly.

```rust
/// Props for [`ResumeScreen`].
#[derive(Default, Props)]
pub struct ResumeScreenProps {
    /// The screen state (rows + selection). Cloned from `active_screen`.
    pub state: ResumeState,
}

/// iocraft component: header, the session list (or empty-state), and a
/// preview pane for the selected row. Footer hint matches the key handler.
#[component]
pub fn ResumeScreen(props: &ResumeScreenProps) -> impl Into<AnyElement<'static>> {
    let state = props.state.clone();

    if state.is_empty() {
        return element! {
            View(flex_direction: FlexDirection::Column, padding: 1) {
                Text(content: "No conversations found to resume.".to_string())
                Text(
                    content: "Press Ctrl+C to exit and start a new conversation.".to_string(),
                    color: Color::DarkGrey,
                )
            }
        }
        .into_any();
    }

    let header = "Resume which session?".to_string();
    let selected = state.selected;
    // Build one Text per row: "> N. <title>  [<modified>]  (<count>)".
    let row_lines: Vec<String> = state
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let marker = if i == selected { "> " } else { "  " };
            format!(
                "{marker}{}. {}  [{}]  {}",
                i + 1,
                r.title,
                r.modified_label,
                r.count_label
            )
        })
        .collect();

    // Preview pane: title + uuid + count of the selected row (from metadata
    // already in hand — no extra file read, no engine change).
    let preview: Vec<String> = state
        .selected_row()
        .map(|r| {
            vec![
                format!("Title:    {}", r.title),
                format!("Session:  {}", r.uuid),
                format!("Messages: {}", r.count_label),
                format!("Modified: {}", r.modified_label),
            ]
        })
        .unwrap_or_default();

    let footer = "↑/↓ select   Enter resume   Esc cancel".to_string();

    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            Text(content: header)
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                #(row_lines.into_iter().map(|line| element! {
                    Text(content: line)
                }))
            }
            View(
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Round,
                padding: 1,
                margin_top: 1,
            ) {
                #(preview.into_iter().map(|line| element! {
                    Text(content: line)
                }))
            }
            View(margin_top: 1) {
                Text(content: footer, color: Color::DarkGrey)
            }
        }
    }
    .into_any()
}
```

> **iocraft note:** the `#(iter.map(...))` fragment syntax for repeated children is the same pattern M6 uses for scrollback rows. The footer string uses `↑`/`↓` (U+2191/U+2193) — these are decoration, not literal-locked, so plain ASCII `Up/Down` is also acceptable if a CI font lacks the glyphs; keep whichever the Doctor screen (M7-11) chose for footer consistency.

- [ ] **Step 4: Run it to verify it passes**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | tail -10
```

Expected: PASS (11 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/resume.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T5): ResumeScreen iocraft component (list + preview + empty-state)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Add `Screen::Resume` variant + route through `screens/mod.rs`

**Files:**
- Modify: `crates/tui/src/screens/mod.rs`
- Modify: `crates/tui/src/state.rs` (only the `Screen` enum variant, if `Screen` lives there per M7-11)

This wires the new screen into the M7-11 overlay infra: extend the `Screen` enum, the `handle_screen_key` dispatch, and the `render_active_screen` dispatch.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `resume.rs` (these exercise the routing helpers `screens::handle_screen_key` / `screens::render_active_screen` through a built `AppState`):

```rust
#[test]
fn screen_routing_enter_clears_active_screen_with_resume_request() {
    use crate::state::{AppState, Screen, StatusSnapshot};
    let target = Uuid::from_u128(7);
    let mut row = meta("only", 0, 1);
    row.uuid = target;
    let st_screen = ResumeState::new(vec![ResumeRow::from_meta(&row)]);

    let mut app = AppState::new(StatusSnapshot::default());
    app.active_screen = Some(Screen::Resume(st_screen));

    // Enter should request a resume and close the screen.
    crate::screens::handle_screen_key(&mut app, &iocraft_enter());
    assert!(app.active_screen.is_none(), "screen should close on Enter");
    assert_eq!(app.resume_request, Some(target));
}

#[test]
fn screen_routing_esc_clears_active_screen_no_request() {
    use crate::state::{AppState, Screen, StatusSnapshot};
    let st_screen = ResumeState::new(vec![ResumeRow::from_meta(&meta("only", 0, 1))]);
    let mut app = AppState::new(StatusSnapshot::default());
    app.active_screen = Some(Screen::Resume(st_screen));

    crate::screens::handle_screen_key(&mut app, &iocraft_esc());
    assert!(app.active_screen.is_none(), "screen should close on Esc");
    assert_eq!(app.resume_request, None);
}

// Build the iocraft (crossterm-0.29) KeyEvents the live path delivers.
fn iocraft_enter() -> iocraft::KeyEvent {
    iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Enter)
}
fn iocraft_esc() -> iocraft::KeyEvent {
    iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Esc)
}
```

> The router takes an **iocraft** `KeyEvent` (the live mount's type) and bridges to crossterm-0.28 internally via `root::iocraft_to_crossterm028_key` (already `pub`-usable within the crate, or expose a thin wrapper). If M7-11 already settled `handle_screen_key`'s signature on the crossterm-0.28 event instead, adapt these two builders to that type — the assertions are what matter.

This test also requires a new `AppState.resume_request: Option<Uuid>` field — the seam the CLI reads after the TUI exits to know which session to load. Added in Step 3.

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p lingxi-tui --lib screens::resume 2>&1 | head -25
```

Expected: FAIL to compile (`Screen::Resume`, `resume_request`, and/or `handle_screen_key` arms missing).

- [ ] **Step 3: Write the minimal implementation**

In `crates/tui/src/state.rs`:
- Add the variant to the `Screen` enum (M7-11 owns this enum; if M7-11 hasn't landed, define it now per the Prerequisite section, with only `Resume`):
  ```rust
  /// Resume picker (M7-12). Owns its own selection state.
  Resume(crate::screens::resume::ResumeState),
  ```
- Add the field to `AppState` (after `should_exit`):
  ```rust
  /// (M7-12) Set by the Resume screen on Enter: the session UUID the user
  /// chose. The CLI reads this after the TUI exits to load + resume it.
  pub resume_request: Option<uuid::Uuid>,
  ```
- Initialize `resume_request: None` in `AppState::new`.

In `crates/tui/src/screens/mod.rs`, extend the dispatch helpers (the bodies M7-11 introduced). After M7-12 the file reads:

```rust
//! Screens — single top-level views composed from the `components/` module.
//!
//! M6-02 shipped `repl`; M7-11 added `doctor` + the `active_screen` overlay
//! routing; M7-12 adds `resume`.

pub mod repl;
pub mod resume;
// pub mod doctor;  // present once M7-11 lands

use iocraft::prelude::*;

use crate::state::{AppState, Screen};

/// Render the active full-page screen. Called by `app::render_screen` when
/// `state.active_screen.is_some()`.
#[must_use]
pub fn render_active_screen(screen: &Screen) -> AnyElement<'static> {
    match screen {
        // Screen::Doctor(s) => element! { doctor::DoctorScreen(state: s.clone()) }.into_any(),
        Screen::Resume(s) => {
            element! { resume::ResumeScreen(state: s.clone()) }.into_any()
        }
    }
}

/// Route one live key into the active screen. Closes the screen (clears
/// `active_screen`) on the screen's cancel/accept; records side effects on
/// `AppState` (e.g. `resume_request`). The live mount delivers an iocraft
/// `KeyEvent`; we bridge to crossterm-0.28 for the pure per-screen handlers.
pub fn handle_screen_key(st: &mut AppState, k: &iocraft::KeyEvent) {
    let ct = crate::root::iocraft_to_crossterm028_key(k);
    // Take the screen out so we can mutate it, then put it back unless closed.
    let Some(screen) = st.active_screen.take() else {
        return;
    };
    match screen {
        Screen::Resume(mut state) => {
            match resume::handle_resume_key(&mut state, ct) {
                resume::ResumeOutcome::Stay => {
                    st.active_screen = Some(Screen::Resume(state));
                }
                resume::ResumeOutcome::Resume(uuid) => {
                    st.resume_request = Some(uuid);
                    st.should_exit = true; // hand control back to the CLI
                }
                resume::ResumeOutcome::Cancel => {
                    // active_screen already taken → screen closed (returns to REPL).
                }
            }
        } // Screen::Doctor(...) handled by M7-11's arm.
    }
}
```

> `iocraft_to_crossterm028_key` is currently a private `fn` in `root.rs`. Make it `pub(crate)` (one-word visibility change) so `screens::handle_screen_key` can call it. This does not change behavior — `root::handle_live_key` still uses it for the permission focus-trap.

**Resume-vs-stay design note:** the Resume screen sets `should_exit = true` on accept so the iocraft mount unwinds and returns to the CLI, which then loads the chosen session and starts a resumed REPL. M7-12 does NOT implement in-process re-entry into a resumed conversation (that depends on M8's resumed-session orchestrator wiring). The seam is `AppState.resume_request`; the CLI reads it (Task 8) and prints/loads accordingly. Esc clears `active_screen` and resumes the (empty) REPL.

- [ ] **Step 4: Run it to verify it passes**

```bash
cargo test -p lingxi-tui --lib 2>&1 | tail -15
```

Expected: PASS (13 tests in `screens::resume`, plus all existing lib tests green).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/state.rs crates/tui/src/screens/mod.rs crates/tui/src/root.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T6): Screen::Resume variant + screen-key routing + resume_request seam

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Behavior + snapshot test files (crate-external)

**Files:**
- Create: `crates/tui/tests/behavior_resume_screen.rs`
- Create: `crates/tui/tests/render_resume_screen.rs`

The inline `#[cfg(test)]` tests in Task 2–6 cover the pure logic + routing. These two crate-external files lock the spec-required behaviors and the snapshot via the public API (the way `snapshot_permission_dialogs.rs` does).

- [ ] **Step 1: Write the behavior tests**

Create `crates/tui/tests/behavior_resume_screen.rs`:

```rust
//! M7-12 behavior tests: list renders N rows from a fixture set of
//! SessionMetadata; arrow select moves; Enter selects the right uuid; Esc
//! cancels; empty set → empty-state. (Spec §3 M7-12 test row.)

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_session::jsonl::loader::SessionMetadata;
use lingxi_tui::screens::resume::{handle_resume_key, ResumeOutcome, ResumeRow, ResumeState};
use uuid::Uuid;

fn meta(title: &str, secs: u64, count: usize, uuid: Uuid) -> SessionMetadata {
    SessionMetadata {
        uuid,
        title: title.to_string(),
        modified: UNIX_EPOCH + Duration::from_secs(secs),
        message_count: count,
        path: PathBuf::from("/tmp/x.jsonl"),
    }
}

fn fixture_rows() -> Vec<ResumeRow> {
    vec![
        ResumeRow::from_meta(&meta("alpha", 300, 5, Uuid::from_u128(1))),
        ResumeRow::from_meta(&meta("beta", 200, 2, Uuid::from_u128(2))),
        ResumeRow::from_meta(&meta("gamma", 100, 1, Uuid::from_u128(3))),
    ]
}

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn list_renders_three_rows() {
    let st = ResumeState::new(fixture_rows());
    assert_eq!(st.rows.len(), 3);
    assert_eq!(st.selected, 0);
}

#[test]
fn arrow_select_moves_then_enter_picks_right_uuid() {
    let mut st = ResumeState::new(fixture_rows());
    // Down twice → row index 2 (gamma, uuid 3).
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Down)), ResumeOutcome::Stay);
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Down)), ResumeOutcome::Stay);
    assert_eq!(st.selected, 2);
    assert_eq!(
        handle_resume_key(&mut st, k(KeyCode::Enter)),
        ResumeOutcome::Resume(Uuid::from_u128(3))
    );
}

#[test]
fn esc_cancels_without_resume() {
    let mut st = ResumeState::new(fixture_rows());
    assert_eq!(handle_resume_key(&mut st, k(KeyCode::Esc)), ResumeOutcome::Cancel);
}

#[test]
fn empty_set_is_empty_state() {
    let st = ResumeState::new(vec![]);
    assert!(st.is_empty());
    assert_eq!(st.selected_uuid(), None);
}
```

- [ ] **Step 2: Write the snapshot test**

Create `crates/tui/tests/render_resume_screen.rs`:

```rust
//! M7-12 snapshot: Resume screen with 3 sessions + preview, and the
//! empty-state. Locks the rendered frame (insta) + substring asserts to
//! survive snapshot-file corruption (matches snapshot_permission_dialogs.rs).

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use iocraft::prelude::*;
use lingxi_session::jsonl::loader::SessionMetadata;
use lingxi_tui::screens::resume::{ResumeRow, ResumeScreen, ResumeState};
use uuid::Uuid;

fn meta(title: &str, secs: u64, count: usize) -> SessionMetadata {
    SessionMetadata {
        uuid: Uuid::nil(),
        title: title.to_string(),
        modified: UNIX_EPOCH + Duration::from_secs(secs),
        message_count: count,
        path: PathBuf::from("/tmp/x.jsonl"),
    }
}

#[test]
fn snapshot_resume_three_sessions_with_preview() {
    let st = ResumeState::new(vec![
        ResumeRow::from_meta(&meta("first session", 300, 5)),
        ResumeRow::from_meta(&meta("second session", 200, 2)),
        ResumeRow::from_meta(&meta("third session", 100, 1)),
    ]);
    let mut element = element! { ResumeScreen(state: st) };
    let frame = element.to_string();
    insta::assert_snapshot!("resume_three_sessions_with_preview", &frame);
    assert!(frame.contains("Resume which session?"), "got: {frame}");
    assert!(frame.contains("> 1. first session"), "got: {frame}");
    assert!(frame.contains("(5 messages)"), "got: {frame}");
    assert!(frame.contains("(1 message)"), "got: {frame}");
    // Preview pane shows the selected (first) row's title.
    assert!(frame.contains("Title:    first session"), "got: {frame}");
}

#[test]
fn snapshot_resume_empty_state() {
    let st = ResumeState::new(vec![]);
    let mut element = element! { ResumeScreen(state: st) };
    let frame = element.to_string();
    insta::assert_snapshot!("resume_empty_state", &frame);
    assert!(frame.contains("No conversations found to resume."), "got: {frame}");
}
```

- [ ] **Step 3: Run + accept snapshots**

```bash
cargo test -p lingxi-tui --test behavior_resume_screen 2>&1 | tail -10
cargo test -p lingxi-tui --test render_resume_screen 2>&1 | tail -20
```

The first run of the snapshot test reports two pending snapshots. Review + accept them:

```bash
cargo insta review   # accept resume_three_sessions_with_preview + resume_empty_state
```

Then re-run `cargo test -p lingxi-tui --test render_resume_screen` → PASS.

> If `ResumeScreen` / `screens::resume::*` are not `pub` from the crate, add `pub mod screens;`/`pub` re-exports as needed in `crates/tui/src/lib.rs` so the external test files can import them. (The crate already re-exports `state` and `session`; mirror that for `screens` if it isn't public yet — check `lib.rs` first.)

- [ ] **Step 4: Commit**

```bash
git add crates/tui/tests/behavior_resume_screen.rs crates/tui/tests/render_resume_screen.rs crates/tui/tests/snapshots/ crates/tui/src/lib.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T7): behavior + snapshot tests for the Resume screen

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Wire `--resume` (no id) → iocraft screen in TTY; keep stdio picker for `--no-tui`

**Files:**
- Modify: `crates/cli/src/run.rs`
- Modify: `crates/cli/src/lib.rs`
- Modify: `crates/tui/src/session.rs` (a TUI entry that opens on the Resume screen)
- Test: `crates/cli/tests/` (a new `resume_routing_test.rs`) + inline `mode`-style test

The current dispatch (`lib.rs`): `if parsed.resume.is_some() → run::run_resume`. `run_resume` treats empty arg as "not wired (M5-13)". We split on **(empty arg) AND (TTY, i.e. not `--no-tui` and stdin/stdout are terminals)**:
- Empty arg + TTY → open the iocraft Resume screen.
- Empty arg + `--no-tui`/non-TTY → the M5-08 stdio picker (`select_session_interactive`) — **the regression-free fallback**.
- Concrete id (any mode) → existing load-by-id path (unchanged).

- [ ] **Step 1: Write the failing routing test**

Create `crates/cli/tests/resume_routing_test.rs`. The decision is made by a new pure function `run::resume_route(argv, is_tty)` so it's testable without a real terminal:

```rust
//! M7-12: --resume routing. Empty arg + TTY → iocraft screen; empty arg +
//! no-tui/non-TTY → M5-08 stdio picker; concrete id → load-by-id.

use lingxi_cli::argv::Argv;
use lingxi_cli::run::{resume_route, ResumeRoute};

fn argv_resume(arg: &str, no_tui: bool) -> Argv {
    let mut a = Argv::from_iter(["lingxi-cli", "--resume", arg]).unwrap();
    a.no_tui = no_tui;
    a
}

#[test]
fn empty_arg_tty_routes_to_iocraft_screen() {
    let a = argv_resume("", false);
    assert_eq!(resume_route(&a, /* is_tty */ true), ResumeRoute::IocraftScreen);
}

#[test]
fn empty_arg_no_tui_routes_to_stdio_picker() {
    let a = argv_resume("", true);
    assert_eq!(resume_route(&a, true), ResumeRoute::StdioPicker);
}

#[test]
fn empty_arg_non_tty_routes_to_stdio_picker() {
    let a = argv_resume("", false);
    assert_eq!(resume_route(&a, /* is_tty */ false), ResumeRoute::StdioPicker);
}

#[test]
fn concrete_id_routes_to_load_by_id() {
    let id = "11111111-1111-1111-1111-111111111111";
    let a = argv_resume(id, false);
    assert_eq!(resume_route(&a, true), ResumeRoute::LoadById);
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p lingxi-cli --test resume_routing_test 2>&1 | head -20
```

Expected: FAIL to compile (`resume_route` / `ResumeRoute` not defined).

- [ ] **Step 3: Implement the route decision + the iocraft branch + the stdio branch**

In `crates/cli/src/run.rs`, add the pure decision and rewrite `run_resume` to branch on it:

```rust
/// Where a `--resume` invocation should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRoute {
    /// `--resume <uuid>` — load that concrete session.
    LoadById,
    /// `--resume` (no id), TTY, no `--no-tui` → iocraft Resume screen (M7-12).
    IocraftScreen,
    /// `--resume` (no id), `--no-tui` or non-TTY → M5-08 stdio picker.
    StdioPicker,
}

/// Decide how to handle a `--resume` invocation. Pure (TTY passed in).
#[must_use]
pub fn resume_route(argv: &Argv, is_tty: bool) -> ResumeRoute {
    let arg = argv.resume.as_deref().unwrap_or("");
    if !arg.is_empty() {
        return ResumeRoute::LoadById;
    }
    if argv.no_tui || !is_tty {
        ResumeRoute::StdioPicker
    } else {
        ResumeRoute::IocraftScreen
    }
}
```

Rewrite the body of `run_resume` to dispatch on `resume_route`. Use `crate::mode`'s TTY helper to keep one source of truth (expose `mode::is_full_tty()` as `pub(crate)`):

```rust
pub async fn run_resume(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    match resume_route(argv, crate::mode::is_full_tty()) {
        ResumeRoute::LoadById => run_resume_by_id(argv, runtime, sink).await,
        ResumeRoute::IocraftScreen => run_resume_iocraft(argv).await,
        ResumeRoute::StdioPicker => run_resume_stdio_picker(argv, sink).await,
    }
}
```

Where:
- `run_resume_by_id` is the existing concrete-id body (extracted verbatim from the current `run_resume` lines 71–93 — id resolve, surface, optional follow-up turn). No behavior change.
- `run_resume_stdio_picker` calls the **unchanged** M5-08 path: resolve `claude_home` + cwd, `list_recent_sessions(claude_home, &cwd, 5, fs)`, then `select_session_interactive(&rows, &mut BufReader::new(stdin), &mut stdout)`. On `Ok(Some(uuid))` surface "Resumed session {uuid}" (same text the id path uses) and run a follow-up turn if a prompt is present; on `Ok(None)` print "Cancelled." and return `SUCCESS`; on `Err(EmptyDirectory)` print the M5-08 "No conversations found to resume." and return `SUCCESS`. (This is the resume picker that M5-08 already implemented; M7-12 wires it as the explicit `--no-tui` fallback.)
- `run_resume_iocraft` builds the loader rows the same way, maps them to `ResumeState`, and launches the TUI seeded on the Resume screen via a new `lingxi_tui::session` entry (Step 4 of this task). After the TUI returns, read `resume_request`: `Some(uuid)` → surface "Resumed session {uuid}"; `None` → "Cancelled." Return `SUCCESS`.

> Loader inputs: `claude_home` comes from the same resolution M5-08 uses (check `crates/session` / `crates/cli` for the existing `claude_home()` helper; the loader's `project_dir_for_cwd` takes `claude_home` + `cwd`). The `fs: Arc<dyn FileSystem>` is the runtime's filesystem (the orchestrator already owns one — reuse it; do **not** construct a second). If the wiring for `claude_home`/`fs` from the CLI is more than ~15 lines, factor a private `load_resume_rows(argv, runtime) -> Result<Vec<SessionMetadata>, LoaderError>` helper shared by the stdio + iocraft branches (DRY).

In `crates/cli/src/lib.rs`, the existing dispatch already routes all `resume.is_some()` through `run_resume`; no change needed there beyond confirming the comment (lines 121–125) is updated to reflect M7-12:

```rust
    // --resume routes through run::run_resume, which itself splits:
    //   <uuid>            → load by id
    //   (none) + TTY      → iocraft Resume screen (M7-12)
    //   (none) + --no-tui → M5-08 stdio picker (unchanged fallback)
    if parsed.resume.is_some() {
        return run::run_resume(&parsed, &runtime, sink.as_ref()).await;
    }
```

- [ ] **Step 4: Add the TUI Resume-screen entry**

In `crates/tui/src/session.rs`, add a constructor/entry that builds a `Runtime` whose initial `AppState.active_screen = Some(Screen::Resume(ResumeState::new(rows)))`, runs the iocraft mount, and on exit exposes `resume_request`. Concretely add:

```rust
/// Launch the TUI directly on the Resume screen, seeded with `rows`. Returns
/// the session UUID the user chose (`None` if cancelled). Used by the CLI's
/// `--resume` (no id) TTY branch (M7-12).
pub async fn run_resume_picker(
    rows: Vec<lingxi_session::jsonl::loader::SessionMetadata>,
) -> std::io::Result<Option<uuid::Uuid>> {
    use crate::screens::resume::{ResumeRow, ResumeState};
    use crate::state::Screen;
    let display: Vec<ResumeRow> = rows.iter().map(ResumeRow::from_meta).collect();
    let state = /* build the shared Arc<Mutex<AppState>> as run_tui_session does */;
    {
        let mut st = state.lock().await;
        st.active_screen = Some(Screen::Resume(ResumeState::new(display)));
    }
    // Mount TuiRoot with this state (reuse the existing mount in run_tui_session);
    // on exit, read resume_request.
    let chosen = state.lock().await.resume_request;
    Ok(chosen)
}
```

> Implement `run_resume_picker` by reusing whatever `run_tui_session` already does to build the `Arc<Mutex<AppState>>` and mount `TuiRoot::fullscreen().await`. The ONLY differences from a normal session: (a) seed `active_screen` before mount, and (b) there is no orchestrator bridge to pump (pass an empty/`None` bridge_rx — the picker doesn't stream a turn). Keep it minimal; do not duplicate the streaming pump. If `run_tui_session`'s body isn't factored to allow a pre-seeded state, extract a private `mount(state, bridge_rx, cancel)` helper and call it from both. Then `run_resume_iocraft` (Task 8 Step 3) calls `lingxi_tui::session::run_resume_picker(rows)`.

- [ ] **Step 5: Run all the new + existing CLI tests**

```bash
cargo test -p lingxi-cli --test resume_routing_test 2>&1 | tail -10
cargo test -p lingxi-cli 2>&1 | tail -15
```

Expected: PASS — the 4 routing tests + all existing CLI tests (the `argv` `resume_without_value_enters_picker_mode` and `mode` tests are unaffected; the stdio picker path is reached identically under `--no-tui`).

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/run.rs crates/cli/src/lib.rs crates/cli/src/mode.rs crates/cli/tests/resume_routing_test.rs crates/tui/src/session.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T8): --resume (no id) → iocraft screen in TTY; M5-08 stdio picker for --no-tui

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Cross-state seam — Resume screen vs. permission focus-trap

**Files:**
- Test: `crates/tui/tests/behavior_resume_screen.rs` (extend)

Spec §2.5 / §5.6: the priority ladder is `permission (1) → screen (2) → input (3)`. Verify the Resume screen does NOT steal keys while a permission dialog is open (priority 1 wins), matching the M6 focus-trap discipline. This is the same seam the M7-16 final review probes.

- [ ] **Step 1: Write the failing test**

Append to `crates/tui/tests/behavior_resume_screen.rs`:

```rust
use lingxi_permission::gate::PermissionRequest;
use lingxi_tui::root::handle_live_key;
use lingxi_tui::state::{AppState, PendingPermission, Screen, StatusSnapshot};

fn iocraft_down() -> iocraft::KeyEvent {
    iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Down)
}

#[test]
fn permission_dialog_outranks_resume_screen() {
    let st_screen = ResumeState::new(fixture_rows());
    let mut app = AppState::new(StatusSnapshot::default());
    app.active_screen = Some(Screen::Resume(st_screen));
    // A permission dialog is also pending (priority 1).
    app.pending_permission = Some(PendingPermission {
        request: PermissionRequest::BypassPermissionsMode,
    });

    // Down arrow: priority 1 (permission) consumes it; the Resume screen's
    // selection must NOT move.
    handle_live_key(&mut app, &iocraft_down(), 24);

    if let Some(Screen::Resume(s)) = &app.active_screen {
        assert_eq!(s.selected, 0, "permission must outrank the screen");
    } else {
        panic!("resume screen should still be open");
    }
}
```

> `PendingPermission`, `Screen`, and `handle_live_key` must be `pub` from `lingxi_tui`. They already are (`root::handle_live_key` is `pub`, `state::PendingPermission` is `pub`); confirm `state::Screen` is `pub` (added in Task 6). The `BypassPermissionsMode` request needs no `resp_tx`, so the permission handler treats the key inertly — exactly what we want to assert (the screen didn't move).

- [ ] **Step 2: Run it to verify it fails or passes**

```bash
cargo test -p lingxi-tui --test behavior_resume_screen permission_dialog_outranks 2>&1 | tail -10
```

Expected: **PASS** if Task 6 placed the screen branch *after* the permission focus-trap (it did — the Prerequisite's priority-2 branch sits below the priority-1 block in `handle_live_key`). If it FAILS, the screen branch was mis-ordered: move it below the `pending_permission.is_some()` block in `root::handle_live_key`. This test is the regression lock for that ordering.

- [ ] **Step 3: Commit**

```bash
git add crates/tui/tests/behavior_resume_screen.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T9): seam test — permission dialog outranks the Resume screen

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Docs + literal-lock note

**Files:**
- Modify: `crates/cli/src/lib.rs` (module doc — resume section)
- Modify: `crates/tui/src/screens/resume.rs` (confirm the byte-locked literals are noted)

- [ ] **Step 1: Update the CLI module doc**

In `crates/cli/src/lib.rs`, the `# Resume mode` doc block (lines 14–19) currently says "interactive picker over 5 most-recent". Replace with the M7-12 reality:

```rust
//! # Resume mode
//!
//! ```text
//! $ lingxi-cli --resume <uuid>   # load that session
//! $ lingxi-cli --resume          # TTY: iocraft Resume screen (M7-12)
//! $ lingxi-cli --resume --no-tui # stdio picker over 5 most-recent (M5-08)
//! ```
```

- [ ] **Step 2: Confirm literal-lock note in `resume.rs`**

Ensure the module doc in `resume.rs` (Task 2 Step 3) names the claude-code source for the locked strings: `ResumeConversation.tsx` (`"No conversations found to resume."`, `"Press Ctrl+C to exit and start a new conversation."`) + `LogSelector.tsx`. This is what M7-16's literal-lock catalog audit will index. No new strings beyond those + the structural `"Resume which session?"` header (shared with the M5-08 stdio picker, which already uses it).

- [ ] **Step 3: Commit**

```bash
git add crates/cli/src/lib.rs crates/tui/src/screens/resume.rs
git commit -m "$(cat <<'EOF'
plan(M7-12 T10): doc resume modes (id / iocraft / stdio) + literal-lock note

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Workspace verification gate + tag `m7.12`

**Files:** none (gate only)

Spec §5.4: run the gate **from inside `lingxi-core/`** (toolchain pins rust 1.82.0; the repo root uses the host toolchain → spurious lint noise — this bit M6-08). No new telemetry events this sub-plan (baseline stays 326; screen-open event deferred to M7-16 audit per the M7-12 brief).

- [ ] **Step 1: Format + lint**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. Fix any `clippy` findings inline (the `#[allow(clippy::cast_possible_wrap)]` on the timestamp cast mirrors the M5-08 loader; keep it).

- [ ] **Step 2: Full test suite**

```bash
cargo test --workspace 2>&1 | tail -30
```

Expected: PASS. Known flakes (allowed rerun, do NOT "fix"): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests. If only these fail, rerun the specific test once to confirm green.

- [ ] **Step 3: Cross-platform compile gate (5 targets)**

```bash
for t in x86_64-unknown-linux-gnu x86_64-apple-darwin x86_64-pc-windows-gnu aarch64-linux-android aarch64-apple-ios; do
  echo "=== $t ===";
  cargo check --workspace --target "$t" 2>&1 | tail -3;
done
```

Expected: each target compiles (same posture as v0.6.0/v0.7.0). If a target's toolchain isn't installed, install it (`rustup target add <t>`) — do not skip.

- [ ] **Step 4: Confirm telemetry count unchanged**

```bash
cargo test --workspace event_names 2>&1 | tail -10
```

Expected: the `ALL_EVENT_NAMES.len()` assertion still equals **326** (M7-12 adds zero events; the screen-open event is M7-16). If a test hard-codes the count and it changed, you added an event you shouldn't have — revert it.

- [ ] **Step 5: Annotated tag**

Only after the gate is fully green:

```bash
git tag -a m7.12 -m "M7-12: Resume screen (iocraft) over M5-08 loader; --resume TTY→screen, --no-tui→stdio picker"
git tag -l m7.12
```

(Tag is **local only** — no remote push, per spec §6.4.)

- [ ] **Step 6: Final commit (if any gate fixes were needed)**

If Steps 1–4 required fixes:

```bash
git add -A
git commit -m "$(cat <<'EOF'
plan(M7-12 T11): workspace gate green (fmt/clippy/test/5-target check); tag m7.12

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

Otherwise the gate is satisfied by the prior commits and only the tag is created.

---

## Self-Review

**1. Spec coverage (§2.3, §2.5, §2.6, §3 M7-12, §5 test budget):**
- §3 M7-12 "lists recent sessions (reuses M5-08 loader — no engine change)" → Tasks 1–3 (dep + `ResumeRow` + `ResumeState` over `list_recent_sessions`/`SessionMetadata`). ✓
- §3 "preview" → Task 5 preview pane. ✓
- §3 "arrow-select, Enter resumes" → Task 4 (`handle_resume_key`) + Task 6 (`resume_request` seam). ✓
- §3 "`--resume` (no id) opens it. M5-08 stdio picker stays as `--no-tui` fallback" → Task 8 (`resume_route` 3-way split). ✓
- §2.3 "screens are modal route states; `AppState.active_screen: Option<Screen>` … Esc/q returns to REPL" → Task 6 `Screen::Resume` + Esc → `Cancel`. ✓
- §2.5 priority-2 routing (after permission, before input) → Task 6 places the branch per the M7-11 Prerequisite; Task 9 locks the ordering with a seam test. ✓
- §2.6 "M5-08 stdio resume picker stays as the `--no-tui` fallback; M7-12 adds an iocraft view over the same loader" → Task 8 `StdioPicker` route calls the unchanged `select_session_interactive`. ✓
- §5 test budget (screens: 1-2 snapshots + open/close + cross-state seam) → Task 7 (2 snapshots) + Task 4/6/9 behavior. ✓
- §2.7 telemetry additive-only; M7-12 adds zero → Task 11 Step 4 locks 326. ✓

**2. Placeholder scan:** No "TBD"/"implement later". Every code step shows the actual code. The two soft spots (`claude_home`/`fs` resolution in Task 8 Step 3, and reusing `run_tui_session`'s mount in Task 8 Step 4) are deliberately written as "reuse the existing X; if it isn't factored, extract helper Y" with the exact seam named — because the precise local helper name (`claude_home()`) lives in code the implementer will have open and must match, not invent. Flagged here so the reviewer confirms.

**3. Type consistency:** `SessionMetadata` is consistently `lingxi_session::jsonl::loader::SessionMetadata` (the loader row with `uuid/title/modified/message_count/path`), NOT `lingxi_session::metadata::SessionMetadata` (the on-disk header) nor `reader::SessionMetadata` (lite). `ResumeRow::from_meta`, `ResumeState::new/selected/selected_uuid/is_empty/selected_row`, `ResumeOutcome::{Stay,Resume,Cancel}`, `handle_resume_key`, `ResumeScreen`/`ResumeScreenProps`, `Screen::Resume`, `AppState.resume_request`, `resume_route`/`ResumeRoute::{LoadById,IocraftScreen,StdioPicker}`, `run_resume_picker` — names match across all tasks. The timestamp formatter is byte-aligned with M5-08's `format_rfc3339_seconds`.

**Prerequisite caveat (recorded, not a gap):** M7-12 depends on M7-11's `Screen` enum + `active_screen` + `handle_screen_key`/`render_active_screen` + the priority-2 branch. The Prerequisite section gives the exact shapes to add if M7-11 hasn't landed when this executes, so the plan is self-contained either way.
