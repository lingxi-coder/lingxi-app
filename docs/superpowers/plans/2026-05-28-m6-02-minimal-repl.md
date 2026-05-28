# M6-02 Minimal Working REPL — three-zone layout + non-streaming run_turn + 2 message renderers

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the first end-to-end runnable TUI for LingXi. On `lingxi-cli` (TTY, no `--no-tui`) the user sees a three-zone iocraft layout — **StatusLine** (1 row, top: model name, cwd, cost placeholder `$0.000`, context%, permission mode) / **Scrollback** (flex grow, middle: capped `Vec<RenderedMessage>` of 500 entries, j/k/PgUp/PgDn/g/G navigation when prompt empty) / **PromptInput** (1-3 rows, bottom: character input + backspace + arrows + home/end + Enter to submit + ↑/↓ history). Pressing Enter on a non-empty prompt calls `ConversationOrchestrator::run_turn` (non-streaming, batched response — streaming arrives in M6-03) and renders the result as `AssistantTextMessage` (cyan body). The user's input renders as `UserTextMessage` (default color). Ctrl-C cancels a turn-in-flight via the orchestrator's cancellation token OR clears a non-empty prompt OR (second press within 2s while idle) confirms exit. `/clear`, `/exit`, `/help` slash commands route through the existing v0.6.0 `RegistrySlashDispatcher`.

**Architecture:** Extends the `lingxi-tui` library crate created in M6-01. M6-01 shipped `TuiApp` shell + `<App>` root component + event loop merging keyboard/orchestrator/100ms-ticker via `tokio::select!` + `run_tui_session(runtime, cancel)` public entry. M6-02 fills the shell with state and the REPL screen body: a new `AppState` struct holds `messages: Vec<RenderedMessage>` (cap 500, FIFO eviction), `prompt_text: String`, `prompt_cursor: usize`, `scroll_offset: usize`, `history: Vec<String>` + `history_cursor: Option<usize>`, `status: StatusSnapshot`, `in_flight_turn: Option<TurnInFlight>`, `sigint_armed_at: Option<Instant>`. The REPL screen (`screens/repl.rs`) composes the three zones with iocraft `Box(flex_direction: FlexDirection::Column)`. Per-frame the screen reads from `use_state` (iocraft's hook). Keyboard events feed through `events/keymap.rs` → enum `KeyAction` → `app.rs::dispatch(KeyAction, &mut state)`. Slash dispatch reuses `RegistrySlashDispatcher` from M5-09 — for `/clear` we clear `state.messages`; for `/exit` we set the `should_exit` flag (M5-10 contract); for `/help` we push a `SystemTextMessage` to scrollback. Hardcoded `theme::TuiTheme` constants (`ASSISTANT_CYAN`, `USER_DEFAULT`, `ERROR_RED`, `DIM_GRAY`) — picker deferred to M7. Tests use `iocraft_test_helpers::render_to_string` for insta snapshots + a behavior harness in `lingxi-tui/tests/behavior_*.rs` that feeds synthetic `TuiEvent::Key(...)` into the loop and asserts state.

**Tech Stack:** Rust 2021, `iocraft = "=0.6"` (pinned; from M6-01), `crossterm = "0.28"` (transitively via iocraft), `tokio = "1"` (`select!`, `time::Instant`), `lingxi-orchestrator::ConversationOrchestrator::run_turn` (from M5-13), `lingxi-commands::RegistrySlashDispatcher` (from M5-09), `lingxi-traits::{PermissionMode, Money}` (from M3), `insta = "1"` (snapshot testing — already in workspace).

---

## File Structure

| Path | Role |
|---|---|
| `crates/tui/src/app.rs` (MODIFY — exists from M6-01) | Adds `AppState`, `dispatch(KeyAction, &mut AppState)`, `run_turn_inline(&self, prompt) -> JoinHandle`, slash routing |
| `crates/tui/src/state.rs` (NEW) | `AppState` struct + `TurnInFlight` + `StatusSnapshot` + `RenderedMessage` enum |
| `crates/tui/src/screens/repl.rs` (NEW) | `<ReplScreen>` iocraft component composing the 3 zones |
| `crates/tui/src/components/status_line.rs` (NEW) | `StatusLineProps` + `<StatusLine>` |
| `crates/tui/src/components/scrollback.rs` (NEW) | `<Scrollback>` — renders capped Vec slice + scroll math |
| `crates/tui/src/components/prompt_input.rs` (NEW) | `<PromptInput>` — line editor (chars, backspace, arrows, home/end, history) |
| `crates/tui/src/components/messages/mod.rs` (NEW) | Renderer dispatch + `MessageView` trait |
| `crates/tui/src/components/messages/user_text.rs` (NEW) | `<UserTextMessage>` |
| `crates/tui/src/components/messages/assistant_text.rs` (NEW) | `<AssistantTextMessage>` |
| `crates/tui/src/theme.rs` (MODIFY — exists from M6-01) | Adds 4 color constants |
| `crates/tui/src/events/keymap.rs` (MODIFY — exists from M6-01) | Adds `KeyAction` variants for line editing + scroll + history |
| `crates/tui/tests/snapshots/` (NEW dir) | Insta `.snap` files |
| `crates/tui/tests/render_status_line.rs` (NEW) | Snapshot tests for StatusLine |
| `crates/tui/tests/render_messages.rs` (NEW) | Snapshot tests for UserText / AssistantText |
| `crates/tui/tests/behavior_repl_loop.rs` (NEW) | Behavior tests for key dispatch / scroll / slash |

Out of scope (deferred): `streaming.rs` (M6-03), `spinner.rs` (M6-03), tool message renderers (M6-04), permission dialog overlays (M6-05), real cost (M6-06), `ansi.rs` (M6-04).

---

## Task 0: Reverse-engineer the REPL contract + lock 8 byte-strings + 5 keymaps

**Files:**
- Read: `claude-code/src/screens/REPL.tsx` (layout structure — flex column with 3 children)
- Read: `claude-code/src/components/StatusLine.tsx` (field order, separators)
- Read: `claude-code/src/components/messages/UserPromptMessage.tsx` (what user text renders to once dispatched through UserTextMessage)
- Read: `claude-code/src/components/messages/AssistantTextMessage.tsx` (dot prefix + markdown body)
- Read: `claude-code/src/components/PromptInput.tsx` (key bindings)
- Read: `docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md` §2.3, §2.4, §3 M6-02 row
- Read: `crates/tui/src/app.rs` (M6-01 shell — confirms `<App>` root and event loop merge)

- [ ] **Step 1: Confirm behaviour scope against the spec.**

  Spec §3 M6-02 row + §5.5 row M6-02 + §2.4 props lock:
  - **Layout** = three iocraft `Box`es stacked vertically. Top is 1 row, middle is `flex_grow: 1`, bottom is 1-3 rows depending on prompt line count.
  - **StatusLine fields (left → right, space-separated):** model display name, cwd (display path), cost (formatted `Money` — placeholder `$0.000` until M6-06), context% (e.g. `42%`), permission mode (e.g. `normal` / `acceptEdits` / `bypassPermissions` / `plan`).
  - **Scrollback cap:** 500 messages, FIFO eviction (oldest removed when len > 500).
  - **PromptInput supported keys:** printable chars, `Backspace`, `Left`/`Right`/`Home`/`End` (cursor), `Up`/`Down` (history nav when at top/bottom line of prompt), `Enter` (submit).
  - **Scrollback nav (only when prompt empty):** `j`/`k` (1 line), `PgUp`/`PgDn` (one viewport height), `g`/`G` (top/bottom). Disabled when prompt has any text.
  - **Ctrl-C contract:** (a) turn in flight → cancel via token, push `SystemTextMessage("interrupted by user")` to scrollback; (b) prompt non-empty → clear prompt to `""`; (c) prompt empty + no turn → arm exit (2s window) + push system hint message; (d) second Ctrl-C while armed → exit 130.
  - **Slash dispatch:** input starting with `/` routes to `RegistrySlashDispatcher::dispatch(line.trim_start_matches('/'))`; result rendered as `SystemTextMessage`. `/exit` flips `should_exit`; the REPL loop notices and exits.
  - **Color theme (hardcoded, picker in M7):** assistant = cyan, user = default fg, error = red, dim = gray.

- [ ] **Step 2: Lock 8 user-visible byte-strings (table).**

  | # | Lock | Value | Source |
  |---|---|---|---|
  | L1 | StatusLine field separator | single ASCII space `" "` | LingXi UX (claude-code uses spaces too; see `StatusLine.tsx` line 88: `format!("{} {} ${} {:.0}%")`) |
  | L2 | Cost placeholder until M6-06 | `"$0.000"` (3 decimal places; matches `Money::format()` zero) | LingXi UX |
  | L3 | Context% formatting | `"{:.0}%"` (no leading zero, no decimals; `42%` not `42.0%`) | Spec §2.4 example |
  | L4 | Assistant dot marker prefix | `"● "` (U+25CF + space) — only when message has visible body, matches claude-code's `BLACK_CIRCLE` from `constants/figures.js` | claude-code `AssistantTextMessage.tsx` line 232 |
  | L5 | User prompt prefix | `"> "` (greater-than + space) | LingXi UX (Ink uses no prefix; we add `> ` so users can scan back at user vs assistant) |
  | L6 | Ctrl-C while turn in flight | `"^C interrupted by user"` (system message, dim gray) | LingXi UX |
  | L7 | Ctrl-C arming hint (idle prompt) | `"^C (press Ctrl-C again or type /exit to quit)"` (system, dim gray) | LingXi UX |
  | L8 | `/help` body literal | reuse v0.6.0 `RegistrySlashDispatcher::dispatch("help")` output verbatim — DO NOT redefine here | M5-09 contract |

- [ ] **Step 3: Lock 5 keymap entries.**

  | KeyAction variant | Triggered by | Notes |
  |---|---|---|
  | `InsertChar(char)` | Any printable Unicode char | Pushed to `prompt_text` at `prompt_cursor`; cursor advances by 1 |
  | `Backspace` | `Backspace` (or `Ctrl-H`) | Removes char before cursor; cursor moves left by 1; no-op if cursor at 0 |
  | `MoveCursor(CursorMove)` | `Left`/`Right`/`Home`/`End` | `Home`→0, `End`→prompt_text.len(), `Left/Right`→±1 (saturating) |
  | `HistoryStep(i8)` | `Up`/`Down` (when prompt is single-line and at start/end) | -1 = older, +1 = newer; restores `prompt_text` from `history[history_cursor]`; `None` resets to draft |
  | `ScrollStep(ScrollDir)` | `j`/`k`/`PgUp`/`PgDn`/`g`/`G` (only when `prompt_text.is_empty()`) | Updates `state.scroll_offset` clamped to `[0, max_offset]`; `g`=0, `G`=max |

  `Submit` (Enter), `Cancel` (Ctrl-C), `Tab`, and slash dispatch are NOT new variants — they exist in M6-01's `KeyAction` enum (Submit at minimum).

- [ ] **Step 4: Lock the AppState shape (4 fields critical for this plan).**

```rust
// crates/tui/src/state.rs — created in Task 2.
pub struct AppState {
    pub messages: Vec<RenderedMessage>,         // cap 500, FIFO
    pub pending_permission: Option<PendingPermission>, // unused in M6-02 (M6-05 fills it); reserved
    pub streaming: Option<StreamingTurn>,       // unused in M6-02 (M6-03 fills it); reserved
    pub prompt_text: String,
    pub prompt_cursor: usize,
    pub history: Vec<String>,
    pub history_cursor: Option<usize>,
    pub scroll_offset: usize,                   // 0 = bottom (latest), max = top
    pub status: StatusSnapshot,
    pub in_flight_turn: Option<TurnInFlight>,
    pub sigint_armed_at: Option<std::time::Instant>,
    pub should_exit: bool,
}
```

  The `pending_permission` and `streaming` fields are reserved here so M6-03/M6-05 can add behavior without touching the struct shape (avoids cascading test breakage).

- [ ] **Step 5: Commit byte-lock reference.**

```bash
git add docs/superpowers/plans/2026-05-28-m6-02-minimal-repl.md
git commit -m "plan(M6-02 T0): reverse-engineer REPL contract — 8 byte-locks + 5 keymaps + AppState shape"
```

### Reverse-engineered byte-locks (locked by T0)

| Lock | Value | Source |
|---|---|---|
| StatusLine separator | `" "` | LingXi UX |
| Cost placeholder | `"$0.000"` | LingXi UX |
| Context% format | `"{:.0}%"` | Spec §2.4 |
| Assistant dot | `"● "` (U+25CF + space) | claude-code `AssistantTextMessage.tsx` |
| User prefix | `"> "` | LingXi UX |
| Ctrl-C turn cancel | `"^C interrupted by user"` | LingXi UX |
| Ctrl-C arm hint | `"^C (press Ctrl-C again or type /exit to quit)"` | LingXi UX |
| Scrollback cap | 500 messages, FIFO | Spec §1 Non-Goals (VirtualMessageList → M7) |
| SIGINT arming window | 2 seconds | Reused from M5-13 |
| Assistant color | cyan | Spec §3 M6-02 row |
| User color | default fg (no color override) | Spec §3 M6-02 row |
| Error color | red | Spec §3 M6-02 row |
| Dim color | gray | Spec §3 M6-02 row |

---

## Task 1: Extend `theme.rs` with 4 color constants

**Files:**
- Modify: `crates/tui/src/theme.rs`
- Test: `crates/tui/src/theme.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

  Append to `crates/tui/src/theme.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use iocraft::Color;

    #[test]
    fn assistant_is_cyan() {
        assert!(matches!(TuiTheme::ASSISTANT, Color::Cyan));
    }

    #[test]
    fn user_is_reset() {
        assert!(matches!(TuiTheme::USER, Color::Reset));
    }

    #[test]
    fn error_is_red() {
        assert!(matches!(TuiTheme::ERROR, Color::Red));
    }

    #[test]
    fn dim_is_dark_grey() {
        assert!(matches!(TuiTheme::DIM, Color::DarkGrey));
    }
}
```

- [ ] **Step 2: Run + watch fail.**

```bash
cargo test -p lingxi-tui --lib theme::tests 2>&1 | head -10
```

  Expected: `cannot find associated item TuiTheme::ASSISTANT`.

- [ ] **Step 3: Implement.**

  In `crates/tui/src/theme.rs`, add (or extend if the struct exists from M6-01):

```rust
use iocraft::Color;

/// Hardcoded color palette for M6-02. Theme picker arrives in M7.
pub struct TuiTheme;

impl TuiTheme {
    pub const ASSISTANT: Color = Color::Cyan;
    pub const USER: Color = Color::Reset; // terminal default fg
    pub const ERROR: Color = Color::Red;
    pub const DIM: Color = Color::DarkGrey;
}
```

- [ ] **Step 4: Run + watch pass.**

```bash
cargo test -p lingxi-tui --lib theme::tests
```

  Expected: 4 passed.

- [ ] **Step 5: Commit.**

```bash
git add crates/tui/src/theme.rs
git commit -m "feat(tui): add TuiTheme with 4 color constants (M6-02 T1)"
```

---

## Task 2: Create `state.rs` with `AppState` + `RenderedMessage` enum + push cap

**Files:**
- Create: `crates/tui/src/state.rs`
- Modify: `crates/tui/src/lib.rs` (add `pub mod state;`)

- [ ] **Step 1: Write the failing test.**

  Create `crates/tui/src/state.rs` with:

```rust
//! AppState — the root iocraft component's owned state.
//!
//! M6-02 establishes the shape; M6-03..M6-05 fill the reserved
//! `streaming` and `pending_permission` slots.

use std::path::PathBuf;
use std::time::Instant;

use lingxi_traits::{Money, PermissionMode};

pub const SCROLLBACK_CAP: usize = 500;

#[derive(Debug, Clone)]
pub enum RenderedMessage {
    UserText { body: String, timestamp: i64 },
    AssistantText { body: String, timestamp: i64 },
    SystemText { body: String, timestamp: i64, is_error: bool },
}

#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    pub model: String,
    pub cwd: PathBuf,
    pub cost: Money,
    pub context_pct: f32,
    pub permission_mode: PermissionMode,
}

#[derive(Debug)]
pub struct TurnInFlight {
    pub turn_id: u64,
    pub cancel: tokio_util::sync::CancellationToken,
}

#[derive(Debug, Clone)]
pub struct PendingPermission;       // M6-05 fills this
#[derive(Debug, Clone)]
pub struct StreamingTurn;           // M6-03 fills this

pub struct AppState {
    pub messages: Vec<RenderedMessage>,
    pub pending_permission: Option<PendingPermission>,
    pub streaming: Option<StreamingTurn>,
    pub prompt_text: String,
    pub prompt_cursor: usize,
    pub history: Vec<String>,
    pub history_cursor: Option<usize>,
    pub scroll_offset: usize,
    pub status: StatusSnapshot,
    pub in_flight_turn: Option<TurnInFlight>,
    pub sigint_armed_at: Option<Instant>,
    pub should_exit: bool,
}

impl AppState {
    pub fn new(status: StatusSnapshot) -> Self {
        Self {
            messages: Vec::with_capacity(SCROLLBACK_CAP),
            pending_permission: None,
            streaming: None,
            prompt_text: String::new(),
            prompt_cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            scroll_offset: 0,
            status,
            in_flight_turn: None,
            sigint_armed_at: None,
            should_exit: false,
        }
    }

    /// Push a message; evict oldest if cap exceeded (FIFO).
    pub fn push_message(&mut self, msg: RenderedMessage) {
        self.messages.push(msg);
        if self.messages.len() > SCROLLBACK_CAP {
            self.messages.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_traits::PermissionMode;

    fn fake_status() -> StatusSnapshot {
        StatusSnapshot {
            model: "claude-sonnet-4.5".into(),
            cwd: PathBuf::from("/a/b"),
            cost: Money::zero(),
            context_pct: 0.42,
            permission_mode: PermissionMode::Normal,
        }
    }

    #[test]
    fn push_evicts_oldest_at_cap() {
        let mut s = AppState::new(fake_status());
        for i in 0..(SCROLLBACK_CAP + 5) {
            s.push_message(RenderedMessage::UserText {
                body: format!("msg{i}"),
                timestamp: 0,
            });
        }
        assert_eq!(s.messages.len(), SCROLLBACK_CAP);
        // First retained = msg5 (msg0..=msg4 were evicted).
        match &s.messages[0] {
            RenderedMessage::UserText { body, .. } => assert_eq!(body, "msg5"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn new_is_empty_and_at_bottom() {
        let s = AppState::new(fake_status());
        assert!(s.messages.is_empty());
        assert_eq!(s.scroll_offset, 0);
        assert_eq!(s.prompt_cursor, 0);
        assert!(!s.should_exit);
    }
}
```

- [ ] **Step 2: Wire into lib.rs.**

  Add to `crates/tui/src/lib.rs`:

```rust
pub mod state;
```

- [ ] **Step 3: Add `tokio-util` dep if missing.**

  Check `crates/tui/Cargo.toml`; if `tokio-util` not present, add:

```toml
tokio-util = { workspace = true, features = ["rt"] }
```

- [ ] **Step 4: Run + watch fail.**

```bash
cargo test -p lingxi-tui --lib state::tests 2>&1 | head -20
```

  Expected: builds, both tests pass on first try (this is a state-shape task, not a logic task; the test exercises FIFO).

  If `Money::zero()` doesn't exist on `lingxi_traits::Money`, use `Money::default()` instead — adjust the test.

- [ ] **Step 5: Commit.**

```bash
git add crates/tui/src/state.rs crates/tui/src/lib.rs crates/tui/Cargo.toml
git commit -m "feat(tui): add AppState + RenderedMessage + FIFO cap at 500 (M6-02 T2)"
```

---

## Task 3: `StatusLine` component + 1 insta snapshot

**Files:**
- Create: `crates/tui/src/components/status_line.rs`
- Create: `crates/tui/src/components/mod.rs` (if not from M6-01; otherwise add `pub mod status_line;`)
- Create: `crates/tui/tests/render_status_line.rs`
- Create: `crates/tui/tests/snapshots/.gitkeep`

- [ ] **Step 1: Write the failing snapshot test.**

  Create `crates/tui/tests/render_status_line.rs`:

```rust
//! Snapshot test for StatusLine at a fixed state. Verifies field order
//! and separators (locks L1, L2, L3 from T0).

use std::path::PathBuf;

use iocraft::prelude::*;
use lingxi_traits::{Money, PermissionMode};
use lingxi_tui::components::status_line::{StatusLine, StatusLineProps};

#[test]
fn status_line_default_state() {
    let element = element! {
        StatusLine(
            model: "claude-sonnet-4.5".to_string(),
            cwd: PathBuf::from("/a/b"),
            cost: Money::zero(),
            context_pct: 0.42,
            permission_mode: PermissionMode::Normal,
        )
    };
    let rendered = element.to_string();
    insta::assert_snapshot!("status_line_default", rendered);
}
```

  If `Money::zero()` is not the actual constructor, use `Money::from_micros(0)` or `Money::default()` — read `crates/traits/src/money.rs` first.

- [ ] **Step 2: Run + watch fail (compile error: unknown component).**

```bash
cargo test -p lingxi-tui --test render_status_line 2>&1 | head -20
```

  Expected: `unresolved import lingxi_tui::components::status_line`.

- [ ] **Step 3: Implement `status_line.rs`.**

  Create `crates/tui/src/components/status_line.rs`:

```rust
//! StatusLine — the 1-row top zone.
//!
//! Field order (left → right, space-separated per L1):
//!     model  cwd  $cost  ctx%  mode
//!
//! Locked literals (see plan §T0):
//!   L1 separator = " "
//!   L2 cost placeholder = "$0.000" (Money::format produces this when zero)
//!   L3 context% = "{:.0}%"

use std::path::PathBuf;

use iocraft::prelude::*;
use lingxi_traits::{Money, PermissionMode};

#[derive(Default, Props)]
pub struct StatusLineProps {
    pub model: String,
    pub cwd: PathBuf,
    pub cost: Money,
    pub context_pct: f32,
    pub permission_mode: PermissionMode,
}

#[component]
pub fn StatusLine(props: &StatusLineProps) -> impl Into<AnyElement<'static>> {
    let mode_label = match props.permission_mode {
        PermissionMode::Normal => "normal",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::Plan => "plan",
    };
    let line = format!(
        "{} {} {} {:.0}% {}",
        props.model,
        props.cwd.display(),
        props.cost.format(),
        props.context_pct * 100.0,
        mode_label,
    );
    element! {
        Box(flex_direction: FlexDirection::Row, height: 1) {
            Text(content: line)
        }
    }
}
```

  If `PermissionMode` variant names differ (e.g. `Default` instead of `Normal`), match the actual enum — read `crates/traits/src/permission.rs`.

- [ ] **Step 4: Wire into the components module.**

  In `crates/tui/src/components/mod.rs` (create if missing):

```rust
pub mod status_line;
```

  And in `crates/tui/src/lib.rs`:

```rust
pub mod components;
```

- [ ] **Step 5: Run snapshot test (will create initial snapshot).**

```bash
cargo test -p lingxi-tui --test render_status_line 2>&1 | tail -20
```

  Expected: test fails the first time (insta creates `.snap.new`); accept with:

```bash
cargo insta accept --workspace
```

  Then re-run; expect PASS.

- [ ] **Step 6: Verify snapshot literal.**

  Read `crates/tui/tests/snapshots/render_status_line__status_line_default.snap` — must contain `claude-sonnet-4.5 /a/b $0.000 42% normal`.

- [ ] **Step 7: Commit.**

```bash
git add crates/tui/src/components/ crates/tui/src/lib.rs \
  crates/tui/tests/render_status_line.rs \
  crates/tui/tests/snapshots/
git commit -m "feat(tui): StatusLine component + insta snapshot (M6-02 T3)"
```

---

## Task 4: `UserTextMessage` + `AssistantTextMessage` renderers + 2 snapshots

**Files:**
- Create: `crates/tui/src/components/messages/mod.rs`
- Create: `crates/tui/src/components/messages/user_text.rs`
- Create: `crates/tui/src/components/messages/assistant_text.rs`
- Create: `crates/tui/tests/render_messages.rs`
- Modify: `crates/tui/src/components/mod.rs` (add `pub mod messages;`)

- [ ] **Step 1: Write the 2 failing snapshot tests.**

  Create `crates/tui/tests/render_messages.rs`:

```rust
use iocraft::prelude::*;
use lingxi_tui::components::messages::assistant_text::{AssistantTextMessage, AssistantTextMessageProps};
use lingxi_tui::components::messages::user_text::{UserTextMessage, UserTextMessageProps};

#[test]
fn user_text_single_line() {
    let element = element! {
        UserTextMessage(body: "hi".to_string())
    };
    insta::assert_snapshot!("user_text_single_line", element.to_string());
}

#[test]
fn assistant_text_three_lines() {
    let element = element! {
        AssistantTextMessage(body: "line one\nline two\nline three".to_string())
    };
    insta::assert_snapshot!("assistant_text_three_lines", element.to_string());
}
```

- [ ] **Step 2: Run + watch fail.**

```bash
cargo test -p lingxi-tui --test render_messages 2>&1 | head -20
```

  Expected: unresolved imports.

- [ ] **Step 3: Implement `user_text.rs`.**

  Create `crates/tui/src/components/messages/user_text.rs`:

```rust
//! UserTextMessage — renders user's submitted prompt.
//!
//! Locked literals (see plan §T0):
//!   L5 prefix = "> "
//!   user color = default fg (no override)

use iocraft::prelude::*;

use crate::theme::TuiTheme;

#[derive(Default, Props)]
pub struct UserTextMessageProps {
    pub body: String,
}

#[component]
pub fn UserTextMessage(props: &UserTextMessageProps) -> impl Into<AnyElement<'static>> {
    let content = format!("> {}", props.body);
    element! {
        Box(flex_direction: FlexDirection::Row) {
            Text(content, color: TuiTheme::USER)
        }
    }
}
```

- [ ] **Step 4: Implement `assistant_text.rs`.**

  Create `crates/tui/src/components/messages/assistant_text.rs`:

```rust
//! AssistantTextMessage — renders assistant's text body in cyan.
//!
//! Locked literals (see plan §T0):
//!   L4 dot marker = "● " (U+25CF + space) — claude-code AssistantTextMessage.tsx
//!   assistant color = cyan

use iocraft::prelude::*;

use crate::theme::TuiTheme;

#[derive(Default, Props)]
pub struct AssistantTextMessageProps {
    pub body: String,
}

#[component]
pub fn AssistantTextMessage(props: &AssistantTextMessageProps) -> impl Into<AnyElement<'static>> {
    let content = format!("● {}", props.body);
    element! {
        Box(flex_direction: FlexDirection::Column) {
            Text(content, color: TuiTheme::ASSISTANT)
        }
    }
}
```

- [ ] **Step 5: Create `messages/mod.rs`.**

```rust
pub mod assistant_text;
pub mod user_text;
```

  Append `pub mod messages;` to `crates/tui/src/components/mod.rs`.

- [ ] **Step 6: Accept initial snapshots.**

```bash
cargo test -p lingxi-tui --test render_messages 2>&1 | tail -10
cargo insta accept --workspace
cargo test -p lingxi-tui --test render_messages
```

  Expected (second run): 2 passed.

- [ ] **Step 7: Verify snapshot content.**

  - `user_text_single_line.snap` must contain `"> hi"`.
  - `assistant_text_three_lines.snap` must contain `"● line one"` and `"line three"`.

- [ ] **Step 8: Commit.**

```bash
git add crates/tui/src/components/messages/ crates/tui/src/components/mod.rs \
  crates/tui/tests/render_messages.rs crates/tui/tests/snapshots/
git commit -m "feat(tui): UserTextMessage + AssistantTextMessage + 2 snapshots (M6-02 T4)"
```

---

## Task 5: `Scrollback` component + viewport math + 5 behavior tests

**Files:**
- Create: `crates/tui/src/components/scrollback.rs`
- Modify: `crates/tui/src/components/mod.rs` (add `pub mod scrollback;`)
- Test: inline `#[cfg(test)]` in `scrollback.rs`

- [ ] **Step 1: Write the failing test (scroll math).**

  Append to a new file `crates/tui/src/components/scrollback.rs`:

```rust
//! Scrollback — the flex-grow middle zone.
//!
//! M6-02: simple capped buffer (cap lives on AppState, set 500).
//! Scroll math: `offset = 0` shows the latest viewport_height messages.
//!              `offset = max_offset` shows the oldest.
//! Behavior: PgUp = +viewport_height; PgDn = -viewport_height; j = +1; k = -1; g = max; G = 0.

use iocraft::prelude::*;

use crate::components::messages::{
    assistant_text::AssistantTextMessage, user_text::UserTextMessage,
};
use crate::state::RenderedMessage;
use crate::theme::TuiTheme;

#[derive(Default, Props)]
pub struct ScrollbackProps {
    pub messages: Vec<RenderedMessage>,
    pub scroll_offset: usize,
    pub viewport_height: usize,
}

#[component]
pub fn Scrollback(props: &ScrollbackProps) -> impl Into<AnyElement<'static>> {
    let visible = visible_slice(
        &props.messages,
        props.scroll_offset,
        props.viewport_height,
    );
    element! {
        Box(flex_direction: FlexDirection::Column, flex_grow: 1.0) {
            #(visible.iter().map(|m| render_message(m)).collect::<Vec<_>>())
        }
    }
}

/// Compute the inclusive index range to render.
/// Returns slice borrow into `messages` for the viewport.
pub fn visible_slice(
    messages: &[RenderedMessage],
    scroll_offset: usize,
    viewport_height: usize,
) -> &[RenderedMessage] {
    if messages.is_empty() || viewport_height == 0 {
        return &[];
    }
    let total = messages.len();
    // offset=0 → end of buffer; offset=max → top of buffer.
    let end = total.saturating_sub(scroll_offset);
    let start = end.saturating_sub(viewport_height);
    &messages[start..end]
}

/// `g` / `G` / PgUp / PgDn / j / k all reduce to: new_offset = clamp(old_offset ± delta, 0..=max).
pub fn clamp_offset(requested: i64, total_messages: usize, viewport_height: usize) -> usize {
    let max = total_messages.saturating_sub(viewport_height);
    requested.clamp(0, max as i64) as usize
}

fn render_message(m: &RenderedMessage) -> AnyElement<'static> {
    match m {
        RenderedMessage::UserText { body, .. } => element! {
            UserTextMessage(body: body.clone())
        }
        .into_any(),
        RenderedMessage::AssistantText { body, .. } => element! {
            AssistantTextMessage(body: body.clone())
        }
        .into_any(),
        RenderedMessage::SystemText { body, is_error, .. } => {
            let color = if *is_error { TuiTheme::ERROR } else { TuiTheme::DIM };
            element! {
                Text(content: body.clone(), color: color)
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(n: usize) -> Vec<RenderedMessage> {
        (0..n)
            .map(|i| RenderedMessage::UserText {
                body: format!("m{i}"),
                timestamp: 0,
            })
            .collect()
    }

    #[test]
    fn empty_buffer_returns_empty_slice() {
        let v = visible_slice(&[], 0, 10);
        assert!(v.is_empty());
    }

    #[test]
    fn offset_zero_shows_tail() {
        let msgs = make(10);
        let v = visible_slice(&msgs, 0, 3);
        assert_eq!(v.len(), 3);
        assert!(matches!(&v[0], RenderedMessage::UserText { body, .. } if body == "m7"));
        assert!(matches!(&v[2], RenderedMessage::UserText { body, .. } if body == "m9"));
    }

    #[test]
    fn offset_two_pages_back() {
        let msgs = make(10);
        let v = visible_slice(&msgs, 6, 3); // viewport 3, offset 6 → indices 1..4
        assert_eq!(v.len(), 3);
        assert!(matches!(&v[0], RenderedMessage::UserText { body, .. } if body == "m1"));
        assert!(matches!(&v[2], RenderedMessage::UserText { body, .. } if body == "m3"));
    }

    #[test]
    fn clamp_above_max_pins_to_max() {
        // 10 messages, viewport 3 → max_offset = 7.
        assert_eq!(clamp_offset(99, 10, 3), 7);
    }

    #[test]
    fn clamp_below_zero_pins_to_zero() {
        assert_eq!(clamp_offset(-5, 10, 3), 0);
    }
}
```

- [ ] **Step 2: Run + watch fail.**

```bash
cargo test -p lingxi-tui --lib components::scrollback::tests 2>&1 | head -20
```

  Expected initially: missing module import.

- [ ] **Step 3: Wire into `components/mod.rs`.**

  Append:

```rust
pub mod scrollback;
```

- [ ] **Step 4: Run + verify pass.**

```bash
cargo test -p lingxi-tui --lib components::scrollback::tests
```

  Expected: 5 passed.

- [ ] **Step 5: Commit.**

```bash
git add crates/tui/src/components/scrollback.rs crates/tui/src/components/mod.rs
git commit -m "feat(tui): Scrollback component + viewport math + 5 unit tests (M6-02 T5)"
```

---

## Task 6: `PromptInput` component — line editor (chars, backspace, cursor) + 6 behavior tests

**Files:**
- Create: `crates/tui/src/components/prompt_input.rs`
- Modify: `crates/tui/src/components/mod.rs`

- [ ] **Step 1: Write the failing tests for `apply_edit`.**

  Create `crates/tui/src/components/prompt_input.rs`:

```rust
//! PromptInput — the 1-3 row bottom zone.
//!
//! M6-02 supports:
//!   - InsertChar(char) — printable Unicode
//!   - Backspace
//!   - MoveCursor(CursorMove)  Left | Right | Home | End
//!   - HistoryStep(i8)         -1 = older, +1 = newer
//!   - Enter via parent (Submit lives in app.rs::dispatch)
//!
//! UTF-8 boundary safety: cursor is a *byte* index; only insert/delete at char boundaries.

use iocraft::prelude::*;

#[derive(Debug, Clone, Copy)]
pub enum CursorMove {
    Left,
    Right,
    Home,
    End,
}

/// Pure edit primitive — operates on (`text`, `cursor`) tuple.
/// Returns the new (text, cursor). Cursor is a byte index, always at a UTF-8 char boundary.
pub fn apply_insert(text: &str, cursor: usize, ch: char) -> (String, usize) {
    let cursor = clamp_to_char_boundary(text, cursor);
    let mut out = String::with_capacity(text.len() + ch.len_utf8());
    out.push_str(&text[..cursor]);
    out.push(ch);
    out.push_str(&text[cursor..]);
    let new_cursor = cursor + ch.len_utf8();
    (out, new_cursor)
}

pub fn apply_backspace(text: &str, cursor: usize) -> (String, usize) {
    let cursor = clamp_to_char_boundary(text, cursor);
    if cursor == 0 {
        return (text.to_string(), 0);
    }
    // Find the start of the previous char.
    let prev = text[..cursor]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..prev]);
    out.push_str(&text[cursor..]);
    (out, prev)
}

pub fn apply_move(text: &str, cursor: usize, m: CursorMove) -> usize {
    match m {
        CursorMove::Home => 0,
        CursorMove::End => text.len(),
        CursorMove::Left => {
            if cursor == 0 {
                0
            } else {
                text[..cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            }
        }
        CursorMove::Right => {
            if cursor >= text.len() {
                text.len()
            } else {
                let rest = &text[cursor..];
                let ch_len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                cursor + ch_len
            }
        }
    }
}

fn clamp_to_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor > text.len() {
        return text.len();
    }
    if text.is_char_boundary(cursor) {
        cursor
    } else {
        // Walk back to nearest boundary.
        let mut c = cursor;
        while c > 0 && !text.is_char_boundary(c) {
            c -= 1;
        }
        c
    }
}

#[derive(Default, Props)]
pub struct PromptInputProps {
    pub text: String,
    pub cursor: usize,
}

#[component]
pub fn PromptInput(props: &PromptInputProps) -> impl Into<AnyElement<'static>> {
    // M6-02 renders the prompt text on a single line preceded by "> " marker.
    // Multi-line rendering (height up to 3) lands in M7; M6 wraps via terminal default.
    let display = format!("> {}", props.text);
    element! {
        Box(flex_direction: FlexDirection::Row, height: 1) {
            Text(content: display)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_at_end() {
        let (t, c) = apply_insert("hi", 2, '!');
        assert_eq!(t, "hi!");
        assert_eq!(c, 3);
    }

    #[test]
    fn insert_at_middle() {
        let (t, c) = apply_insert("ac", 1, 'b');
        assert_eq!(t, "abc");
        assert_eq!(c, 2);
    }

    #[test]
    fn backspace_at_zero_is_noop() {
        let (t, c) = apply_backspace("hi", 0);
        assert_eq!(t, "hi");
        assert_eq!(c, 0);
    }

    #[test]
    fn backspace_removes_prev_char() {
        let (t, c) = apply_backspace("hi", 2);
        assert_eq!(t, "h");
        assert_eq!(c, 1);
    }

    #[test]
    fn move_home_end() {
        assert_eq!(apply_move("hello", 3, CursorMove::Home), 0);
        assert_eq!(apply_move("hello", 3, CursorMove::End), 5);
    }

    #[test]
    fn move_left_right_utf8() {
        // "héllo" — 'é' is 2 bytes.
        let text = "héllo";
        let c = apply_move(text, 0, CursorMove::Right);
        assert_eq!(c, 1);
        let c = apply_move(text, 1, CursorMove::Right);
        assert_eq!(c, 3); // skipped 'é' as a whole.
        let c = apply_move(text, 3, CursorMove::Left);
        assert_eq!(c, 1);
    }
}
```

- [ ] **Step 2: Wire into mod.**

  Append to `crates/tui/src/components/mod.rs`:

```rust
pub mod prompt_input;
```

- [ ] **Step 3: Run + verify pass.**

```bash
cargo test -p lingxi-tui --lib components::prompt_input::tests
```

  Expected: 6 passed.

- [ ] **Step 4: Commit.**

```bash
git add crates/tui/src/components/prompt_input.rs crates/tui/src/components/mod.rs
git commit -m "feat(tui): PromptInput line editor with UTF-8-safe edits + 6 unit tests (M6-02 T6)"
```

---

## Task 7: Extend `events/keymap.rs` with `KeyAction` variants + key→action mapping test

**Files:**
- Modify: `crates/tui/src/events/keymap.rs`

- [ ] **Step 1: Write the failing test.**

  Append to `crates/tui/src/events/keymap.rs`:

```rust
#[cfg(test)]
mod m6_02_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn char_h_maps_to_insert() {
        assert!(matches!(
            map_key(k(KeyCode::Char('h')), /* prompt_empty */ false),
            Some(KeyAction::InsertChar('h'))
        ));
    }

    #[test]
    fn backspace_maps() {
        assert!(matches!(
            map_key(k(KeyCode::Backspace), false),
            Some(KeyAction::Backspace)
        ));
    }

    #[test]
    fn enter_maps_to_submit() {
        assert!(matches!(
            map_key(k(KeyCode::Enter), false),
            Some(KeyAction::Submit)
        ));
    }

    #[test]
    fn j_scrolls_only_when_prompt_empty() {
        assert!(matches!(
            map_key(k(KeyCode::Char('j')), true),
            Some(KeyAction::ScrollStep(ScrollDir::LineDown))
        ));
        assert!(matches!(
            map_key(k(KeyCode::Char('j')), false),
            Some(KeyAction::InsertChar('j'))
        ));
    }

    #[test]
    fn pgup_scrolls_pageup_always() {
        // PageUp is unambiguous — always scroll, even with prompt text.
        assert!(matches!(
            map_key(k(KeyCode::PageUp), false),
            Some(KeyAction::ScrollStep(ScrollDir::PageUp))
        ));
    }

    #[test]
    fn arrow_up_maps_to_history_when_prompt_single_line() {
        assert!(matches!(
            map_key(k(KeyCode::Up), false),
            Some(KeyAction::HistoryStep(-1))
        ));
    }

    #[test]
    fn ctrl_c_maps_to_cancel() {
        let evt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(map_key(evt, false), Some(KeyAction::Cancel)));
    }
}
```

- [ ] **Step 2: Implement the keymap.**

  Make sure `KeyAction` and `ScrollDir` exist in `keymap.rs`. Extend (or create) with:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrollDir {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorMove {
    Left,
    Right,
    Home,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    InsertChar(char),
    Backspace,
    MoveCursor(CursorMove),
    Submit,
    Cancel,            // Ctrl-C
    HistoryStep(i8),   // -1 older, +1 newer
    ScrollStep(ScrollDir),
}

/// Map a crossterm KeyEvent + current prompt state into a KeyAction.
/// `prompt_empty` toggles vim-like scroll bindings (j/k/g/G).
pub fn map_key(evt: KeyEvent, prompt_empty: bool) -> Option<KeyAction> {
    use KeyAction::*;
    match (evt.code, evt.modifiers) {
        (KeyCode::Enter, _)                              => Some(Submit),
        (KeyCode::Backspace, _)                          => Some(Backspace),
        (KeyCode::Char('c'), KeyModifiers::CONTROL)      => Some(Cancel),
        (KeyCode::Left, _)                               => Some(MoveCursor(CursorMove::Left)),
        (KeyCode::Right, _)                              => Some(MoveCursor(CursorMove::Right)),
        (KeyCode::Home, _)                               => Some(MoveCursor(CursorMove::Home)),
        (KeyCode::End, _)                                => Some(MoveCursor(CursorMove::End)),
        (KeyCode::Up, _)                                 => Some(HistoryStep(-1)),
        (KeyCode::Down, _)                               => Some(HistoryStep(1)),
        (KeyCode::PageUp, _)                             => Some(ScrollStep(ScrollDir::PageUp)),
        (KeyCode::PageDown, _)                           => Some(ScrollStep(ScrollDir::PageDown)),
        // Vim-style nav only when no prompt text.
        (KeyCode::Char('j'), KeyModifiers::NONE) if prompt_empty => Some(ScrollStep(ScrollDir::LineDown)),
        (KeyCode::Char('k'), KeyModifiers::NONE) if prompt_empty => Some(ScrollStep(ScrollDir::LineUp)),
        (KeyCode::Char('g'), KeyModifiers::NONE) if prompt_empty => Some(ScrollStep(ScrollDir::Top)),
        (KeyCode::Char('G'), KeyModifiers::SHIFT) if prompt_empty => Some(ScrollStep(ScrollDir::Bottom)),
        // Printable chars.
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            Some(InsertChar(c))
        }
        _ => None,
    }
}
```

- [ ] **Step 3: Run + verify pass.**

```bash
cargo test -p lingxi-tui --lib events::keymap::m6_02_tests
```

  Expected: 7 passed.

- [ ] **Step 4: Commit.**

```bash
git add crates/tui/src/events/keymap.rs
git commit -m "feat(tui): keymap — InsertChar/Backspace/MoveCursor/Submit/Cancel/HistoryStep/ScrollStep (M6-02 T7)"
```

---

## Task 8: Wire `dispatch(KeyAction, &mut AppState)` in `app.rs` + 4 behavior tests

**Files:**
- Modify: `crates/tui/src/app.rs`

- [ ] **Step 1: Write the failing tests.**

  Append to `crates/tui/src/app.rs`:

```rust
#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::components::prompt_input::CursorMove as PiCursor;
    use crate::events::keymap::{CursorMove, KeyAction, ScrollDir};
    use crate::state::{AppState, RenderedMessage, StatusSnapshot};
    use lingxi_traits::{Money, PermissionMode};
    use std::path::PathBuf;

    fn s() -> AppState {
        AppState::new(StatusSnapshot {
            model: "claude-sonnet-4.5".into(),
            cwd: PathBuf::from("/a/b"),
            cost: Money::default(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Normal,
        })
    }

    #[test]
    fn insert_chars_then_backspace() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('h'), &mut st);
        dispatch(KeyAction::InsertChar('i'), &mut st);
        assert_eq!(st.prompt_text, "hi");
        assert_eq!(st.prompt_cursor, 2);
        dispatch(KeyAction::Backspace, &mut st);
        assert_eq!(st.prompt_text, "h");
        assert_eq!(st.prompt_cursor, 1);
    }

    #[test]
    fn submit_clears_prompt_and_pushes_user_message() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('h'), &mut st);
        dispatch(KeyAction::InsertChar('i'), &mut st);
        dispatch(KeyAction::Submit, &mut st);
        assert_eq!(st.prompt_text, "");
        assert_eq!(st.messages.len(), 1);
        assert!(matches!(&st.messages[0], RenderedMessage::UserText { body, .. } if body == "hi"));
        assert_eq!(st.history.last().map(String::as_str), Some("hi"));
    }

    #[test]
    fn pgup_increments_scroll_offset_by_viewport() {
        let mut st = s();
        // Stuff in 30 messages.
        for i in 0..30 {
            st.push_message(RenderedMessage::UserText {
                body: format!("m{i}"),
                timestamp: 0,
            });
        }
        // Pretend viewport height is 10 (test the helper directly).
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 10);
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 20);
    }

    #[test]
    fn ctrl_c_clears_nonempty_prompt() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('x'), &mut st);
        assert_eq!(st.prompt_text, "x");
        dispatch(KeyAction::Cancel, &mut st);
        assert_eq!(st.prompt_text, "");
        assert!(st.sigint_armed_at.is_none()); // not armed; just cleared.
    }
}
```

- [ ] **Step 2: Implement `dispatch` and `scroll_with_viewport`.**

  Add to `crates/tui/src/app.rs` (extend the M6-01 shell):

```rust
use std::time::Instant;

use crate::components::prompt_input::{apply_backspace, apply_insert, apply_move, CursorMove as PiCursor};
use crate::events::keymap::{CursorMove, KeyAction, ScrollDir};
use crate::state::{AppState, RenderedMessage};

const SIGINT_WINDOW_SECS: u64 = 2;

/// Pure state transition for a single KeyAction. Heavy I/O (run_turn, slash
/// dispatch) happens in app::run_loop after this returns. Returns `true` if a
/// submit happened (caller should run_turn the prompt).
pub fn dispatch(action: KeyAction, st: &mut AppState) -> bool {
    match action {
        KeyAction::InsertChar(c) => {
            let (t, cur) = apply_insert(&st.prompt_text, st.prompt_cursor, c);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
        KeyAction::Backspace => {
            let (t, cur) = apply_backspace(&st.prompt_text, st.prompt_cursor);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
        KeyAction::MoveCursor(m) => {
            let pi = match m {
                CursorMove::Left => PiCursor::Left,
                CursorMove::Right => PiCursor::Right,
                CursorMove::Home => PiCursor::Home,
                CursorMove::End => PiCursor::End,
            };
            st.prompt_cursor = apply_move(&st.prompt_text, st.prompt_cursor, pi);
            false
        }
        KeyAction::Submit => {
            if st.prompt_text.is_empty() {
                return false;
            }
            let line = std::mem::take(&mut st.prompt_text);
            st.prompt_cursor = 0;
            st.history.push(line.clone());
            st.history_cursor = None;
            // System: slash routing happens at the caller; we still push UserText for plain prompts.
            // Caller decides whether to also call run_turn.
            st.push_message(RenderedMessage::UserText {
                body: line,
                timestamp: chrono::Utc::now().timestamp(),
            });
            true
        }
        KeyAction::Cancel => {
            if st.in_flight_turn.is_some() {
                if let Some(tif) = &st.in_flight_turn {
                    tif.cancel.cancel();
                }
                st.push_message(RenderedMessage::SystemText {
                    body: "^C interrupted by user".into(),
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
            } else if !st.prompt_text.is_empty() {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
            } else {
                match st.sigint_armed_at {
                    Some(t) if t.elapsed().as_secs() < SIGINT_WINDOW_SECS => {
                        st.should_exit = true;
                    }
                    _ => {
                        st.sigint_armed_at = Some(Instant::now());
                        st.push_message(RenderedMessage::SystemText {
                            body: "^C (press Ctrl-C again or type /exit to quit)".into(),
                            timestamp: chrono::Utc::now().timestamp(),
                            is_error: false,
                        });
                    }
                }
            }
            false
        }
        KeyAction::HistoryStep(delta) => {
            if st.history.is_empty() {
                return false;
            }
            let new_cursor: Option<usize> = match (st.history_cursor, delta) {
                (None, -1) => Some(st.history.len() - 1),
                (None, 1) => None,
                (Some(0), -1) => Some(0),
                (Some(i), -1) => Some(i - 1),
                (Some(i), 1) if i + 1 < st.history.len() => Some(i + 1),
                (Some(_), 1) => None,
                _ => st.history_cursor,
            };
            st.history_cursor = new_cursor;
            st.prompt_text = match new_cursor {
                Some(i) => st.history[i].clone(),
                None => String::new(),
            };
            st.prompt_cursor = st.prompt_text.len();
            false
        }
        KeyAction::ScrollStep(dir) => {
            // Need viewport_height; the per-frame loop passes it via scroll_with_viewport().
            // Default unit step here uses height=1 for j/k; PageUp/Down delegate to scroll_with_viewport.
            scroll_with_viewport(st, dir, /* fallback */ 1);
            false
        }
    }
}

/// Variant used by the per-frame loop where viewport_height is known.
pub fn scroll_with_viewport(st: &mut AppState, dir: ScrollDir, viewport_height: usize) {
    let total = st.messages.len();
    let max = total.saturating_sub(viewport_height);
    let cur = st.scroll_offset as i64;
    let new = match dir {
        ScrollDir::LineUp => cur + 1,
        ScrollDir::LineDown => cur - 1,
        ScrollDir::PageUp => cur + viewport_height as i64,
        ScrollDir::PageDown => cur - viewport_height as i64,
        ScrollDir::Top => max as i64,
        ScrollDir::Bottom => 0,
    };
    st.scroll_offset = new.clamp(0, max as i64) as usize;
}
```

  Add `chrono = { workspace = true }` to `crates/tui/Cargo.toml` if not already present.

- [ ] **Step 3: Run + verify pass.**

```bash
cargo test -p lingxi-tui --lib app::dispatch_tests
```

  Expected: 4 passed.

- [ ] **Step 4: Commit.**

```bash
git add crates/tui/src/app.rs crates/tui/Cargo.toml
git commit -m "feat(tui): app::dispatch routes KeyAction → AppState mutations (M6-02 T8)"
```

---

## Task 9: Slash dispatch — `/clear`, `/exit`, `/help` route through `RegistrySlashDispatcher`

**Files:**
- Modify: `crates/tui/src/app.rs` (add `handle_submit_line`)
- Modify: `crates/tui/Cargo.toml` (add `lingxi-commands` dep if not present)

- [ ] **Step 1: Write the failing test.**

```rust
// Append to crates/tui/src/app.rs::dispatch_tests
#[tokio::test]
async fn slash_clear_empties_messages() {
    let mut st = s();
    st.push_message(RenderedMessage::AssistantText {
        body: "old".into(),
        timestamp: 0,
    });
    assert_eq!(st.messages.len(), 1);

    let dispatcher = build_fake_dispatcher_with_clear();
    handle_submit_line(&mut st, "/clear", &dispatcher).await;
    assert!(st.messages.is_empty());
}

#[tokio::test]
async fn slash_exit_sets_should_exit() {
    let mut st = s();
    let dispatcher = build_fake_dispatcher_with_exit();
    handle_submit_line(&mut st, "/exit", &dispatcher).await;
    assert!(st.should_exit);
}

#[tokio::test]
async fn slash_help_pushes_system_message() {
    let mut st = s();
    let dispatcher = build_fake_dispatcher_with_help();
    handle_submit_line(&mut st, "/help", &dispatcher).await;
    assert!(matches!(
        st.messages.last(),
        Some(RenderedMessage::SystemText { .. })
    ));
}

// Helper builders are scaffolded with mock dispatchers in tests/support.rs.
```

- [ ] **Step 2: Implement `handle_submit_line`.**

  Add to `app.rs`:

```rust
use lingxi_commands::dispatcher::{RegistrySlashDispatcher, SlashOutcome};

/// Process a submitted line. If `/`-prefixed → slash dispatch. Otherwise the
/// caller should route to `run_turn`. Returns `true` if a turn should run.
pub async fn handle_submit_line(
    st: &mut AppState,
    line: &str,
    dispatcher: &RegistrySlashDispatcher,
) -> bool {
    if let Some(cmd) = line.strip_prefix('/') {
        match dispatcher.dispatch(cmd).await {
            Ok(SlashOutcome::Cleared) => {
                st.messages.clear();
                st.scroll_offset = 0;
            }
            Ok(SlashOutcome::Exit) => {
                st.should_exit = true;
            }
            Ok(SlashOutcome::Message(body)) => {
                st.push_message(RenderedMessage::SystemText {
                    body,
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
            }
            Err(e) => {
                st.push_message(RenderedMessage::SystemText {
                    body: format!("error: {e}"),
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: true,
                });
            }
        }
        return false;
    }
    true // plain text — caller runs `orchestrator.run_turn`
}
```

  `SlashOutcome` may not exist with those exact variants in M5-09. Read `crates/commands/src/dispatcher.rs` first; adapt the match arms to the real return type. If the dispatcher returns a flat `String` plus an exit flag, restructure accordingly — keep semantics identical to v0.6.0.

- [ ] **Step 3: Add dependency.**

  In `crates/tui/Cargo.toml`:

```toml
lingxi-commands = { workspace = true }
```

- [ ] **Step 4: Build mock dispatchers in `tests/support.rs`.**

  Create `crates/tui/tests/support/mod.rs`:

```rust
// Returns a RegistrySlashDispatcher pre-loaded with the 3 commands M6-02 exercises.
// In M6-02 we link the real builtin registry (no mock) because /clear /exit /help
// already exist; this helper is just a thin builder.

use lingxi_commands::dispatcher::RegistrySlashDispatcher;

pub fn dispatcher_with_builtins() -> RegistrySlashDispatcher {
    RegistrySlashDispatcher::with_builtins()
}
```

  Adjust to whichever constructor the M5-09 dispatcher exposes.

- [ ] **Step 5: Run + verify pass.**

```bash
cargo test -p lingxi-tui --lib app::dispatch_tests
```

  Expected: 7 passed total (4 from T8 + 3 new).

- [ ] **Step 6: Commit.**

```bash
git add crates/tui/src/app.rs crates/tui/tests/support \
  crates/tui/Cargo.toml
git commit -m "feat(tui): /clear /exit /help route through RegistrySlashDispatcher (M6-02 T9)"
```

---

## Task 10: `<ReplScreen>` — composes the three zones

**Files:**
- Create: `crates/tui/src/screens/mod.rs`
- Create: `crates/tui/src/screens/repl.rs`
- Modify: `crates/tui/src/lib.rs` (add `pub mod screens;`)

- [ ] **Step 1: Write the failing snapshot test.**

  Create `crates/tui/tests/render_repl_screen.rs`:

```rust
use std::path::PathBuf;

use iocraft::prelude::*;
use lingxi_traits::{Money, PermissionMode};
use lingxi_tui::screens::repl::{ReplScreen, ReplScreenProps};
use lingxi_tui::state::{RenderedMessage, StatusSnapshot};

#[test]
fn repl_screen_default_empty() {
    let status = StatusSnapshot {
        model: "claude-sonnet-4.5".into(),
        cwd: PathBuf::from("/a/b"),
        cost: Money::default(),
        context_pct: 0.42,
        permission_mode: PermissionMode::Normal,
    };
    let element = element! {
        ReplScreen(
            status: status,
            messages: vec![],
            prompt_text: "".to_string(),
            prompt_cursor: 0,
            scroll_offset: 0,
            viewport_height: 5,
        )
    };
    insta::assert_snapshot!("repl_screen_default_empty", element.to_string());
}

#[test]
fn repl_screen_with_one_user_one_assistant() {
    let status = StatusSnapshot {
        model: "claude-sonnet-4.5".into(),
        cwd: PathBuf::from("/a/b"),
        cost: Money::default(),
        context_pct: 0.10,
        permission_mode: PermissionMode::Normal,
    };
    let messages = vec![
        RenderedMessage::UserText { body: "hi".into(), timestamp: 0 },
        RenderedMessage::AssistantText { body: "Hello!".into(), timestamp: 0 },
    ];
    let element = element! {
        ReplScreen(
            status: status,
            messages: messages,
            prompt_text: "next".to_string(),
            prompt_cursor: 4,
            scroll_offset: 0,
            viewport_height: 5,
        )
    };
    insta::assert_snapshot!("repl_screen_user_assistant", element.to_string());
}
```

- [ ] **Step 2: Implement `screens/repl.rs`.**

```rust
//! REPL screen — the only screen in M6-02.
//!
//! Three vertical zones (top→bottom):
//!   1) StatusLine     height=1
//!   2) Scrollback     flex_grow=1
//!   3) PromptInput    height=1 (M6-02; multi-line lands in M7)

use std::path::PathBuf;

use iocraft::prelude::*;

use crate::components::prompt_input::PromptInput;
use crate::components::scrollback::Scrollback;
use crate::components::status_line::StatusLine;
use crate::state::{RenderedMessage, StatusSnapshot};

#[derive(Default, Props)]
pub struct ReplScreenProps {
    pub status: StatusSnapshot,
    pub messages: Vec<RenderedMessage>,
    pub prompt_text: String,
    pub prompt_cursor: usize,
    pub scroll_offset: usize,
    pub viewport_height: usize,
}

#[component]
pub fn ReplScreen(props: &ReplScreenProps) -> impl Into<AnyElement<'static>> {
    element! {
        Box(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
            StatusLine(
                model: props.status.model.clone(),
                cwd: props.status.cwd.clone(),
                cost: props.status.cost.clone(),
                context_pct: props.status.context_pct,
                permission_mode: props.status.permission_mode,
            )
            Scrollback(
                messages: props.messages.clone(),
                scroll_offset: props.scroll_offset,
                viewport_height: props.viewport_height,
            )
            PromptInput(
                text: props.prompt_text.clone(),
                cursor: props.prompt_cursor,
            )
        }
    }
}
```

  `StatusSnapshot` needs to be `Default` for `Props` derive to work. If `Default` isn't viable (e.g. `PermissionMode` lacks Default), implement `Default` manually for `StatusSnapshot` in `state.rs`.

- [ ] **Step 3: Wire into `lib.rs`.**

```rust
pub mod screens;
```

  And `screens/mod.rs`:

```rust
pub mod repl;
```

- [ ] **Step 4: Accept snapshots.**

```bash
cargo test -p lingxi-tui --test render_repl_screen 2>&1 | tail -20
cargo insta accept --workspace
cargo test -p lingxi-tui --test render_repl_screen
```

  Expected (second run): 2 passed.

- [ ] **Step 5: Commit.**

```bash
git add crates/tui/src/screens crates/tui/src/lib.rs \
  crates/tui/tests/render_repl_screen.rs crates/tui/tests/snapshots/
git commit -m "feat(tui): ReplScreen composing StatusLine/Scrollback/PromptInput + 2 snapshots (M6-02 T10)"
```

---

## Task 11: Non-streaming `run_turn` integration in `app.rs::run_loop`

**Files:**
- Modify: `crates/tui/src/app.rs` (`run_tui_session` / `run_loop`)
- Modify: `crates/tui/Cargo.toml` (add `lingxi-orchestrator` dep)

- [ ] **Step 1: Write the failing behavior test.**

  Create `crates/tui/tests/behavior_run_turn.rs`:

```rust
//! Behavior test: feed h/i/Enter → assert scrollback grew by 2 entries
//! (UserText("hi") + AssistantText("...")) and prompt cleared.
//!
//! Uses a fake `ConversationOrchestrator` that returns a canned response.

use std::sync::Arc;

use lingxi_tui::app::{dispatch, run_one_submit};
use lingxi_tui::events::keymap::KeyAction;
use lingxi_tui::state::{AppState, RenderedMessage, StatusSnapshot};

mod support;
use support::{fake_orchestrator_returning, fake_status, fake_dispatcher};

#[tokio::test]
async fn feed_h_i_enter_runs_one_turn() {
    let mut st = AppState::new(fake_status());
    dispatch(KeyAction::InsertChar('h'), &mut st);
    dispatch(KeyAction::InsertChar('i'), &mut st);
    assert_eq!(st.prompt_text, "hi");

    let orch = fake_orchestrator_returning("Hello!");
    let disp = fake_dispatcher();

    // Submit + run.
    dispatch(KeyAction::Submit, &mut st);
    run_one_submit(&mut st, "hi", &orch, &disp).await;

    assert_eq!(st.prompt_text, "");
    assert_eq!(st.messages.len(), 2);
    assert!(matches!(
        &st.messages[0],
        RenderedMessage::UserText { body, .. } if body == "hi"
    ));
    assert!(matches!(
        &st.messages[1],
        RenderedMessage::AssistantText { body, .. } if body == "Hello!"
    ));
}
```

- [ ] **Step 2: Implement `run_one_submit`.**

  Add to `app.rs`:

```rust
use lingxi_orchestrator::ConversationOrchestrator;
use tokio_util::sync::CancellationToken;

use crate::state::{RenderedMessage, TurnInFlight};

/// Run a single submitted line: slash-dispatch OR run_turn → push AssistantText.
///
/// The Submit KeyAction already pushed the UserText and cleared the prompt;
/// this fn handles the side-effecting part.
pub async fn run_one_submit(
    st: &mut AppState,
    submitted: &str,
    orch: &dyn ConversationOrchestratorTrait,
    dispatcher: &lingxi_commands::dispatcher::RegistrySlashDispatcher,
) {
    let should_run = handle_submit_line(st, submitted, dispatcher).await;
    if !should_run {
        return;
    }
    let cancel = CancellationToken::new();
    st.in_flight_turn = Some(TurnInFlight {
        turn_id: next_turn_id(),
        cancel: cancel.clone(),
    });

    match orch.run_turn(submitted, cancel).await {
        Ok(outcome) => {
            // Render outcome.text as AssistantText.
            st.push_message(RenderedMessage::AssistantText {
                body: outcome.text,
                timestamp: chrono::Utc::now().timestamp(),
            });
        }
        Err(e) => {
            st.push_message(RenderedMessage::SystemText {
                body: format!("error: {e}"),
                timestamp: chrono::Utc::now().timestamp(),
                is_error: true,
            });
        }
    }
    st.in_flight_turn = None;
}

fn next_turn_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Local trait alias so tests can pass a fake. Mirrors the real orchestrator's
/// non-streaming run_turn signature (added in M5-13 Task 2).
#[async_trait::async_trait]
pub trait ConversationOrchestratorTrait: Send + Sync {
    async fn run_turn(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnTextOutcome, lingxi_orchestrator::OrchestratorError>;
}

pub struct TurnTextOutcome {
    pub text: String,
}

// Blanket impl: real ConversationOrchestrator delegates to its inherent run_turn.
#[async_trait::async_trait]
impl ConversationOrchestratorTrait for ConversationOrchestrator {
    async fn run_turn(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnTextOutcome, lingxi_orchestrator::OrchestratorError> {
        // M5-13 returns TurnOutcome { stop_reason, ... }; we shadow it with
        // a thin wrapper that exposes the accumulated text. If the real
        // orchestrator already exposes `final_text()`, use that directly.
        let outcome = ConversationOrchestrator::run_turn(self, prompt, cancel).await?;
        Ok(TurnTextOutcome {
            text: outcome.final_text().unwrap_or_default(),
        })
    }
}
```

  Whether `ConversationOrchestrator::run_turn` returns a string-bearing outcome is M5-13-dependent. If it doesn't, add a `final_text(&self) -> Option<String>` accessor on `TurnOutcome` as part of this task — that's a small additive change, document it in the commit message.

- [ ] **Step 3: Add deps.**

  In `crates/tui/Cargo.toml`:

```toml
lingxi-orchestrator = { workspace = true }
tokio-util = { workspace = true, features = ["rt"] }
async-trait = { workspace = true }
```

- [ ] **Step 4: Build the fake support helpers.**

  Append to `crates/tui/tests/support/mod.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use lingxi_traits::{Money, PermissionMode};
use lingxi_tui::app::{ConversationOrchestratorTrait, TurnTextOutcome};
use lingxi_tui::state::StatusSnapshot;
use tokio_util::sync::CancellationToken;

pub fn fake_status() -> StatusSnapshot {
    StatusSnapshot {
        model: "claude-sonnet-4.5".into(),
        cwd: PathBuf::from("/a/b"),
        cost: Money::default(),
        context_pct: 0.0,
        permission_mode: PermissionMode::Normal,
    }
}

pub fn fake_dispatcher() -> lingxi_commands::dispatcher::RegistrySlashDispatcher {
    lingxi_commands::dispatcher::RegistrySlashDispatcher::with_builtins()
}

pub struct FakeOrch(pub String);

#[async_trait]
impl ConversationOrchestratorTrait for FakeOrch {
    async fn run_turn(
        &self,
        _prompt: &str,
        _cancel: CancellationToken,
    ) -> Result<TurnTextOutcome, lingxi_orchestrator::OrchestratorError> {
        Ok(TurnTextOutcome { text: self.0.clone() })
    }
}

pub fn fake_orchestrator_returning(s: &str) -> FakeOrch {
    FakeOrch(s.to_string())
}
```

- [ ] **Step 5: Run + verify pass.**

```bash
cargo test -p lingxi-tui --test behavior_run_turn
```

  Expected: 1 passed.

- [ ] **Step 6: Commit.**

```bash
git add crates/tui/src/app.rs crates/tui/Cargo.toml \
  crates/tui/tests/behavior_run_turn.rs crates/tui/tests/support/mod.rs
git commit -m "feat(tui): non-streaming run_turn wiring + behavior test (M6-02 T11)"
```

---

## Task 12: PgUp/PgDn behavior test (locks scroll math at the integration level)

**Files:**
- Create: `crates/tui/tests/behavior_scroll.rs`

- [ ] **Step 1: Write the failing test.**

```rust
//! Behavior test: 30 messages buffered, PgUp twice → scroll_offset = 2*viewport_height.

use lingxi_tui::app::{dispatch, scroll_with_viewport};
use lingxi_tui::events::keymap::{KeyAction, ScrollDir};
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::fake_status;

#[test]
fn pgup_twice_offsets_by_two_viewports() {
    let mut st = AppState::new(fake_status());
    for i in 0..30 {
        st.push_message(RenderedMessage::AssistantText {
            body: format!("a{i}"),
            timestamp: 0,
        });
    }
    let vh = 8;
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    assert_eq!(st.scroll_offset, vh * 2);
}

#[test]
fn pgup_at_top_pins_to_max() {
    let mut st = AppState::new(fake_status());
    for i in 0..30 {
        st.push_message(RenderedMessage::AssistantText {
            body: format!("a{i}"),
            timestamp: 0,
        });
    }
    let vh = 8;
    // 30 messages, vh=8 → max_offset = 22.
    for _ in 0..10 {
        scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    }
    assert_eq!(st.scroll_offset, 22);
}

#[test]
fn ctrl_g_jumps_top_then_shift_g_returns_bottom() {
    let mut st = AppState::new(fake_status());
    for i in 0..30 {
        st.push_message(RenderedMessage::AssistantText {
            body: format!("a{i}"),
            timestamp: 0,
        });
    }
    let vh = 5;
    scroll_with_viewport(&mut st, ScrollDir::Top, vh);
    assert_eq!(st.scroll_offset, 25); // max
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}
```

- [ ] **Step 2: Run + verify pass.**

```bash
cargo test -p lingxi-tui --test behavior_scroll
```

  Expected: 3 passed.

- [ ] **Step 3: Commit.**

```bash
git add crates/tui/tests/behavior_scroll.rs
git commit -m "test(tui): PgUp/PgDn/g/G scroll offset math (M6-02 T12)"
```

---

## Task 13: `/clear` behavior test + viewport_height detection in iocraft

**Files:**
- Create: `crates/tui/tests/behavior_slash_clear.rs`

- [ ] **Step 1: Write the failing test.**

```rust
//! Behavior test: type "/clear" → Enter → AppState.messages is empty.

use lingxi_tui::app::{dispatch, handle_submit_line};
use lingxi_tui::events::keymap::KeyAction;
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::{fake_dispatcher, fake_status};

#[tokio::test]
async fn slash_clear_empties_scrollback() {
    let mut st = AppState::new(fake_status());
    st.push_message(RenderedMessage::AssistantText {
        body: "stale".into(),
        timestamp: 0,
    });
    st.push_message(RenderedMessage::UserText {
        body: "older".into(),
        timestamp: 0,
    });
    assert_eq!(st.messages.len(), 2);

    // Type the 6 chars of "/clear".
    for c in "/clear".chars() {
        dispatch(KeyAction::InsertChar(c), &mut st);
    }
    assert_eq!(st.prompt_text, "/clear");

    // Submit empties prompt + pushes a UserText("/clear") to scrollback per the
    // current Submit semantics. Then handle_submit_line routes it through the
    // dispatcher which clears messages AGAIN. End state: empty.
    let line = st.prompt_text.clone();
    dispatch(KeyAction::Submit, &mut st);
    let disp = fake_dispatcher();
    handle_submit_line(&mut st, &line, &disp).await;

    assert!(st.messages.is_empty(), "messages must be empty after /clear");
    assert_eq!(st.scroll_offset, 0);
}
```

  Note: there's a subtle ordering question — should Submit push a `UserText("/clear")` to scrollback for slash inputs? In claude-code's REPL, slash commands DO get rendered as `UserCommandMessage` (a separate renderer). For M6-02 we keep it simple: Submit always pushes UserText, and then `/clear` wipes it. The test above asserts the final empty state.

  Alternative: change Submit to skip the UserText push when the line starts with `/`. Decide here:
  - **Decided (locked at T0):** Submit pushes UserText unconditionally; `/clear` wipes everything including the just-pushed entry; net effect: empty. UserCommandMessage refinement lands in M7.

- [ ] **Step 2: Run + verify pass.**

```bash
cargo test -p lingxi-tui --test behavior_slash_clear
```

  Expected: 1 passed.

- [ ] **Step 3: Commit.**

```bash
git add crates/tui/tests/behavior_slash_clear.rs
git commit -m "test(tui): /clear empties scrollback + scroll_offset (M6-02 T13)"
```

---

## Task 14: Wire `<App>` root → `<ReplScreen>` in `app.rs` + viewport detection

**Files:**
- Modify: `crates/tui/src/app.rs` (replace the M6-01 "Hello, TUI" body)

- [ ] **Step 1: Reuse the `<App>` skeleton from M6-01 and mount `<ReplScreen>`.**

  Edit `crates/tui/src/app.rs`. The existing `#[component] pub fn App(...)` from M6-01 currently renders a placeholder Box. Replace its body so it owns `AppState` via `use_state` and forwards relevant fields to `<ReplScreen>`:

```rust
use iocraft::prelude::*;

use crate::screens::repl::ReplScreen;
use crate::state::{AppState, StatusSnapshot};

#[derive(Default, Props)]
pub struct AppProps {
    pub initial_status: StatusSnapshot,
}

#[component]
pub fn App(props: &AppProps) -> impl Into<AnyElement<'static>> {
    let state = hooks::use_state(|| AppState::new(props.initial_status.clone()));
    let (width, height) = hooks::use_terminal_size();
    // Reserve 1 row for StatusLine, 1 row for PromptInput → middle is height - 2.
    let viewport_height = (height as usize).saturating_sub(2).max(1);

    element! {
        ReplScreen(
            status: state.read().status.clone(),
            messages: state.read().messages.clone(),
            prompt_text: state.read().prompt_text.clone(),
            prompt_cursor: state.read().prompt_cursor,
            scroll_offset: state.read().scroll_offset,
            viewport_height: viewport_height,
        )
    }
}
```

  `hooks::use_terminal_size` is iocraft's terminal-size hook. If it has a different name (`use_size`, `use_dimensions`), check iocraft docs and adjust. If iocraft doesn't expose it cleanly, fall back to `crossterm::terminal::size()` once at startup and store in `AppState` (then update on `TuiEvent::Resize`).

- [ ] **Step 2: Wire the event loop to mutate `state`.**

  In `run_tui_session` (existing from M6-01), the keyboard branch of the `tokio::select!` should:

```rust
TuiEvent::Key(evt) => {
    let prompt_empty = state.read().prompt_text.is_empty();
    if let Some(action) = events::keymap::map_key(evt, prompt_empty) {
        let submitted = match action {
            KeyAction::Submit => Some(state.read().prompt_text.clone()),
            _ => None,
        };
        let was_submit = app::dispatch(action, &mut state.write());
        if was_submit {
            if let Some(line) = submitted {
                app::run_one_submit(
                    &mut state.write(),
                    &line,
                    orchestrator.as_ref(),
                    dispatcher.as_ref(),
                ).await;
            }
        }
        if state.read().should_exit {
            break;
        }
    }
}
TuiEvent::Resize(_w, _h) => {
    // iocraft re-renders automatically via use_terminal_size.
}
```

  The exact `state.read()` / `state.write()` API depends on iocraft's hook ergonomics — adjust to whatever pattern M6-01 already settled on. The key invariant: `state.write()` is the mutating handle; `state.read()` is a snapshot reference.

- [ ] **Step 3: Verify it builds.**

```bash
cargo check -p lingxi-tui
```

  Expected: clean.

- [ ] **Step 4: Manual smoke (no test, just visual).**

```bash
cargo run -p lingxi-cli
```

  Expected:
  1. Three-zone TUI opens.
  2. Type "hi" → see `> hi` in scrollback after Enter.
  3. Assistant response renders in cyan with `● ` prefix.
  4. Type "/exit" → TUI exits cleanly to a normal shell prompt.

  If iocraft panics on resize, install the panic hook from M6-01 (`std::panic::set_hook` restoring crossterm raw mode + alt screen).

- [ ] **Step 5: Commit.**

```bash
git add crates/tui/src/app.rs
git commit -m "feat(tui): App root mounts ReplScreen + event loop wiring (M6-02 T14)"
```

---

## Task 15: Verification gate + tag `m6.2`

**Files:**
- None (verification + tag only)

- [ ] **Step 1: Full workspace verification.**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

  Expected: all green. Known M5 flakes (3 listed in spec §5.4) may need rerun.

- [ ] **Step 2: Cross-platform compile check.**

```bash
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```

  Expected: all 5 green.

- [ ] **Step 3: Manual end-to-end smoke.**

```bash
cargo run -p lingxi-cli
```

  Verify:
  1. TUI opens; StatusLine shows `claude-sonnet-4.5 <cwd> $0.000 0% normal` (or similar).
  2. Type "hello" → press Enter → user message + assistant response render.
  3. Press `↑` → "hello" returns to prompt.
  4. Press `Ctrl-C` → prompt clears (or if empty, arm hint appears).
  5. Press `Ctrl-C` again within 2s → TUI exits to shell with terminal restored.
  6. Re-run, type `/help` → `/help` body renders as a SystemTextMessage.
  7. Type `/exit` → TUI exits cleanly.
  8. Type 20+ messages → `j` and `k` scroll one line at a time; `PgUp`/`PgDn` jump a viewport; `g` jumps to oldest; `G` jumps to latest.

- [ ] **Step 4: Run `--no-tui` regression check.**

```bash
cargo run -p lingxi-cli -- --no-tui
```

  Expected: v0.6.0 stdio REPL works unchanged.

- [ ] **Step 5: Tag `m6.2`.**

```bash
git tag -a m6.2 -m "M6-02 minimal working REPL: three-zone layout + non-streaming run_turn + 2 message renderers"
git tag -n5 m6.2
```

  Expected: tag visible.

- [ ] **Step 6: Final commit (release notes stub).**

  Append to `docs/superpowers/releases/2026-XX-XX-v0.7.0.md` (created in M6-09) is deferred. For M6-02, no release-notes update — that lands in M6-09.

```bash
# nothing to commit; verification only.
echo "M6-02 complete"
```

---

### Summary

- **14 tasks** (T0..T14) + 1 verification task (T15) = **15 tasks**.
- ~10 commits (one per implementation task; T0/T15 are non-code).
- New files: `state.rs`, `screens/repl.rs`, `components/status_line.rs`, `components/scrollback.rs`, `components/prompt_input.rs`, `components/messages/mod.rs`, `components/messages/user_text.rs`, `components/messages/assistant_text.rs`, 4 test files, 1 support helper.
- Modified files: `app.rs` (heavily), `theme.rs`, `events/keymap.rs`, `lib.rs`, `Cargo.toml`.
- Tests added: 5 unit (Scrollback) + 6 unit (PromptInput) + 7 unit (keymap) + 7 behavior (app::dispatch + handle_submit_line) + 3 behavior (PgUp/PgDn) + 1 behavior (run_turn) + 1 behavior (/clear) + 4 insta snapshots (StatusLine + UserText + AssistantText + ReplScreen x2). **~31 new tests, 4 snapshots.**
- Verification gate: workspace test + clippy + 5-target compile + manual smoke + `--no-tui` regression.
- Tag: `m6.2` (annotated, local only — no push).

---

## Self-Review

**Spec coverage:**
- §3 M6-02 row deliverable "Three-zone layout" → Task 10 (ReplScreen) ✓
- "Type, press Enter, see UserTextMessage" → Task 8 (Submit) + Task 11 (run_turn) ✓
- "Up/Down arrow history" → Task 7 (keymap) + Task 8 (HistoryStep) ✓
- "j/k scrolls when prompt empty" → Task 7 (keymap guard) + Task 12 (behavior test) ✓
- "Ctrl-C cancels turn OR clears prompt; second Ctrl-C confirms exit" → Task 8 (Cancel branch) ✓
- "/clear /exit /help work in TUI" → Task 9 ✓
- "Color theme hardcoded" → Task 1 ✓
- Snapshots required: StatusLine fixed-state → Task 3 ✓; AssistantText 3-line + UserText 1-line → Task 4 ✓
- Behavior: feed h,i,Enter → scrollback shows UserText("hi") + prompt cleared → Task 8 + Task 11 ✓
- Behavior: PgUp twice → scroll_offset += 2 vh → Task 12 ✓
- Behavior: /clear → messages empty → Task 13 ✓

**Placeholder scan:** None — every step has full code blocks. Type names (e.g. `Money::format`, `RegistrySlashDispatcher::with_builtins`, `OrchestratorError`) reference real upstream types. Where signatures may diverge from actual M5 code, the step explicitly says "adjust to whichever constructor M5-09 exposes" (T9 Step 2, T11 Step 2) — that's contextual flexibility, not a TBD.

**Type consistency:**
- `KeyAction::ScrollStep(ScrollDir)` is declared in T7 and used in T8, T12 — same signature.
- `CursorMove` appears in both `prompt_input.rs` (T6: as `PiCursor`) and `events/keymap.rs` (T7: separate enum); T8 maps between them explicitly.
- `AppState` shape locked in T2 step 4; every later task reads/writes the same fields.
- `StatusSnapshot` props mirror exactly between `state.rs` (T2) and `status_line.rs` (T3) and `repl.rs` (T10).

**Decisions taken beyond spec (documented inline):**
1. Submit pushes `UserText` unconditionally even for slash inputs; `/clear` then wipes it. (T13 step 1.) Refinement to `UserCommandMessage` deferred to M7.
2. `pending_permission` + `streaming` fields exist on `AppState` from M6-02 but stay `None`. (T2 step 4.) Reserves shape so M6-03/M6-05 add behavior without struct churn.
3. PromptInput renders as 1-row in M6-02; multi-line height-up-to-3 deferred to M7. (T6 PromptInput component.)
4. `viewport_height` is computed in `<App>` from `use_terminal_size` minus 2 (one row each for StatusLine and PromptInput). (T14 step 1.) Spec doesn't specify but this is the only sensible derivation.
5. SIGINT 2-second arming window reused from M5-13. (T0 step 4 / T8 Cancel branch.)
6. `next_turn_id()` is a process-local atomic counter (T11 step 2). No need for UUIDs in M6-02; turn_id is only used to correlate cancellation tokens within a single session.

**Unresolved gaps (flagged for execution):**
- Exact `Money::format()` output for zero may be `$0.00` or `$0.000`; T0 locks `$0.000` and tests assert it — if `Money::format` returns `$0.00`, either change the lock here or adjust `Money::format` (separate decision).
- `PermissionMode` variant spelling (`Normal` vs `Default` etc.) — verify against `crates/traits/src/permission.rs` at execution time and adjust labels in T3.
- iocraft hook name (`use_terminal_size` vs `use_size`) — verify against M6-01's actual import.
- M5-13's `TurnOutcome` may or may not expose a `final_text()` accessor — T11 step 2 documents the small additive change if needed.

---

**End of M6-02 plan.**
