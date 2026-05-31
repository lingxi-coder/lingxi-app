# M9-06 — Coordinator Status Chrome Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the team/coordinator status chrome — `TeamStatus` footer, `TeammateViewHeader`, `AgentProgressLine` (tree-char per-agent progress), and the `CoordinatorAgentStatus` panel — plus the teammate-view mode scaffolding (`viewing_teammate` state + esc-to-return), all literal-locked to claude-code and rendered from the M9-01 presentation model (`WorkerRow`) + `agent_color`.

**Architecture:** All four surfaces land as **pure `render_*_to_string` functions** (byte-locked, unit + insta tested) in a new `tui/src/components/coordinator/` module, consuming `WorkerRow` + rich renderer parameters. Per-agent color uses `multiagent::style::agent_color_from_name` (M9-03). The `TeamStatus` footer integrates into the layout beside the M9-05 task footer; the `TeammateViewHeader` renders when `AppState.viewing_teammate` is set, with `Esc` clearing it (a new check in `handle_live_key`).

**Worker-data reality (R4/R5 — locked):** `traits/` has NO team/worker handle and must not change; the worker execution pool is stubbed, so the live worker roster is empty. Therefore **worker/teammate data is fixture-backed** (the `FixtureFeed` already emits `WorkersRefreshed`); the renderers take their fields as parameters and are snapshot-tested with fixtures (spec Q3: build the full surface, fixtures where stubbed). The **live mailbox drain + key-driven teammate entry + transcript filtering are deferred** (gated on the pool un-stub) and documented — M9-06 delivers the renderers + the view-mode header/exit scaffolding.

**Tech Stack:** Rust 1.82.0, `iocraft = "=0.8.3"`, `insta`. Run cargo from inside `lingxi-code/`.

**Literal-lock reference:** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/` — `TeamStatus.tsx` (`teams/`), `TeammateViewHeader.tsx`, `AgentProgressLine.tsx`, `CoordinatorAgentStatus.tsx`.

**Locked literals (verified):**
- TeamStatus: `{n} teammate[s]` + ` \u{00B7} Enter to view` (hide when 0; exclude name `team-lead`).
- TeammateViewHeader: `Viewing @{name} \u{00B7} esc to return` then a second line `{prompt}`.
- AgentProgressLine tree: `├─` (U+251C U+2500) mid, `└─` (U+2514 U+2500) last; continuation `│  ` (U+2502 + 2sp) mid / `   ` (3sp) last; status gutter `⎿  ` (U+23BF + 2sp); separator ` \u{00B7} ` (U+00B7); count `{n} tool use[s] \u{00B7} {tokens} tokens`; status states `Initializing…` (U+2026) / `Done` / `Running in the background`.
- middot ` · ` = U+00B7.

---

## File Structure

| File | Responsibility | C/M |
|---|---|---|
| `tui/src/components/coordinator/mod.rs` | module decls + (re-exports) | Create |
| `tui/src/components/coordinator/format_num.rs` | `format_token_count(u64) -> String` (e.g. `1.2k`) | Create |
| `tui/src/components/coordinator/team_status.rs` | `render_team_footer(&[WorkerRow]) -> Option<String>` | Create |
| `tui/src/components/coordinator/teammate_view_header.rs` | `render_teammate_view_header(name, prompt) -> String` | Create |
| `tui/src/components/coordinator/agent_progress.rs` | `AgentProgressState` + `render_agent_progress_line(..)` | Create |
| `tui/src/components/coordinator/coordinator_status.rs` | `render_coordinator_status(&[WorkerRow], selected) -> String` | Create |
| `tui/src/components/mod.rs` | + `pub mod coordinator;` | Modify |
| `tui/src/state.rs` | + `pub viewing_teammate: Option<String>` + open/close helpers | Modify |
| `tui/src/root.rs` | + esc-clears-teammate-view check; + TeamStatus footer & TeammateViewHeader in layout | Modify |
| `tui/tests/coordinator_chrome.rs` | insta snapshots + behavior tests | Create |

`WorkerRow` (M9-01) = `{ agent_id, name, agent_type, status }` (all `String`). It carries no tool/token counts (not on any wire) — those are renderer parameters, fixture-supplied.

---

## Task 1: format_token_count + TeamStatus footer

claude-code `TeamStatus.tsx`: `{n} teammate[s]` + (when selected) ` · Enter to view`; hidden when 0; excludes `team-lead`.

**Files:** Create `tui/src/components/coordinator/{mod.rs,format_num.rs,team_status.rs}`; Modify `tui/src/components/mod.rs`.

- [ ] **Step 1: Module skeleton** — add `pub mod coordinator;` to `tui/src/components/mod.rs` (alphabetical). Create `tui/src/components/coordinator/mod.rs`:

```rust
//! Coordinator / team status chrome (M9-06): TeamStatus footer,
//! TeammateViewHeader, AgentProgressLine, CoordinatorAgentStatus. Pure string
//! renderers over the M9-01 `WorkerRow` model + fixture-supplied counts.

pub mod format_num;
pub mod team_status;
```

(Tasks 2–4 each append their own `pub mod` line when they create their file — never declare a module whose file does not exist yet.)

- [ ] **Step 2: format_token_count (TDD)** — create `tui/src/components/coordinator/format_num.rs`:

```rust
//! Compact number formatting for token counts (claude-code `formatNumber`):
//! `< 1000` → as-is; `>= 1000` → `1.2k`; `>= 1_000_000` → `1.2M`.

/// Format a token count compactly. Drops a trailing `.0`.
#[must_use]
pub fn format_token_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        let v = (n as f64) / 1000.0;
        return trim_one_decimal(v, 'k');
    }
    let v = (n as f64) / 1_000_000.0;
    trim_one_decimal(v, 'M')
}

fn trim_one_decimal(v: f64, suffix: char) -> String {
    // One decimal, but drop `.0`.
    let s = format!("{v:.1}");
    let s = s.strip_suffix(".0").map_or(s.clone(), ToString::to_string);
    format!("{s}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(512), "512");
        assert_eq!(format_token_count(1000), "1k");
        assert_eq!(format_token_count(1200), "1.2k");
        assert_eq!(format_token_count(15_300), "15.3k");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(2_500_000), "2.5M");
    }
}
```

- [ ] **Step 3: TeamStatus footer (TDD)** — create `tui/src/components/coordinator/team_status.rs`:

```rust
//! `TeamStatus` footer (claude-code `teams/TeamStatus.tsx`): `{n} teammate[s]`
//! + optional ` · Enter to view`. Hidden when 0; excludes the `team-lead`.

use crate::multiagent::state::WorkerRow;

/// ` · Enter to view` hint (space + U+00B7 + space + text).
const VIEW_HINT: &str = " \u{00B7} Enter to view";

/// The team footer pill. `None` when there are no teammates (excluding the
/// `team-lead`). `with_hint` appends the Enter-to-view affordance.
#[must_use]
pub fn render_team_footer(workers: &[WorkerRow], with_hint: bool) -> Option<String> {
    let n = workers.iter().filter(|w| w.name != "team-lead").count();
    if n == 0 {
        return None;
    }
    let noun = if n == 1 { "teammate" } else { "teammates" };
    let mut s = format!("{n} {noun}");
    if with_hint {
        s.push_str(VIEW_HINT);
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(name: &str) -> WorkerRow {
        WorkerRow { agent_id: "a1".into(), name: name.into(), agent_type: "explorer".into(), status: "working".into() }
    }

    #[test]
    fn hidden_when_empty_or_only_lead() {
        assert_eq!(render_team_footer(&[], false), None);
        assert_eq!(render_team_footer(&[w("team-lead")], false), None);
    }

    #[test]
    fn count_and_hint() {
        assert_eq!(render_team_footer(&[w("alice")], false).as_deref(), Some("1 teammate"));
        assert_eq!(render_team_footer(&[w("alice"), w("bob")], false).as_deref(), Some("2 teammates"));
        assert_eq!(render_team_footer(&[w("alice")], true).as_deref(), Some("1 teammate \u{00B7} Enter to view"));
    }

    #[test]
    fn excludes_lead_from_count() {
        assert_eq!(render_team_footer(&[w("team-lead"), w("alice")], false).as_deref(), Some("1 teammate"));
    }
}
```

- [ ] **Step 4: Run + build** — `cargo test -p tui --lib components::coordinator::format_num`, `cargo test -p tui --lib components::coordinator::team_status` (both PASS); `cargo build -p tui`.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-06): format_token_count + TeamStatus footer"
```

---

## Task 2: TeammateViewHeader

claude-code `TeammateViewHeader.tsx`: `Viewing @{name} · esc to return` then a `{prompt}` line.

**Files:** Create `tui/src/components/coordinator/teammate_view_header.rs`; Modify `coordinator/mod.rs` (+ `pub mod teammate_view_header;`).

- [ ] **Step 1: Create with tests** — `teammate_view_header.rs`:

```rust
//! `TeammateViewHeader` (claude-code `TeammateViewHeader.tsx`):
//! `Viewing @{name} · esc to return` then a second line with the prompt.

/// ` · esc to return` (space + U+00B7 + space + text).
const RETURN_HINT: &str = " \u{00B7} esc to return";

/// Two-line header: `Viewing @{name} · esc to return` then `{prompt}`.
/// An empty prompt omits the second line.
#[must_use]
pub fn render_teammate_view_header(name: &str, prompt: &str) -> String {
    let head = format!("Viewing @{name}{RETURN_HINT}");
    if prompt.is_empty() {
        head
    } else {
        format!("{head}\n{prompt}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_prompt() {
        assert_eq!(
            render_teammate_view_header("alice", "Refactor the parser"),
            "Viewing @alice \u{00B7} esc to return\nRefactor the parser"
        );
    }

    #[test]
    fn without_prompt() {
        assert_eq!(render_teammate_view_header("bob", ""), "Viewing @bob \u{00B7} esc to return");
    }
}
```

- [ ] **Step 2: Run + wire + build** — add `pub mod teammate_view_header;` to `coordinator/mod.rs`; `cargo test -p tui --lib components::coordinator::teammate_view_header` (2 PASS); `cargo build -p tui`.

- [ ] **Step 3: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-06): TeammateViewHeader renderer"
```

---

## Task 3: AgentProgressLine

claude-code `AgentProgressLine.tsx`: a tree-prefixed agent line + (unless backgrounded) a `· {n} tool use[s] · {tokens} tokens` segment + a status line `{cont}⎿  {status}`.

**Files:** Create `tui/src/components/coordinator/agent_progress.rs`; Modify `coordinator/mod.rs` (+ `pub mod agent_progress;`).

- [ ] **Step 1: Read the reference** — read `AgentProgressLine.tsx`; confirm the tree chars, the count format, and the status strings. The code below is reconciled to the grounding; adjust literals if the source differs.

- [ ] **Step 2: Create with tests** — `agent_progress.rs`:

```rust
//! `AgentProgressLine` (claude-code `AgentProgressLine.tsx`): a tree-prefixed
//! agent line with tool/token counts and a status line. Backgrounded agents
//! show only the tree line (counts + status hidden).

use crate::components::coordinator::format_num::format_token_count;

/// Tree branch for a mid-list agent (`├─`).
pub const BRANCH_MID: &str = "\u{251C}\u{2500}";
/// Tree branch for the last agent (`└─`).
pub const BRANCH_LAST: &str = "\u{2514}\u{2500}";
/// Status gutter (`⎿  ` = U+23BF + 2 spaces).
pub const STATUS_GUTTER: &str = "\u{23BF}  ";

/// Progress state → status text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProgressState {
    /// Spawned, not yet producing output → `Initializing…`.
    Initializing,
    /// Running with a current activity label.
    Running(String),
    /// Finished (sync) → `Done`.
    Done,
    /// Async + resolved → tree line only (counts + status hidden).
    Backgrounded,
}

/// Render the multi-line progress block for one agent.
/// `is_last` selects the tree branch + continuation indent.
#[must_use]
pub fn render_agent_progress_line(
    name: &str,
    is_last: bool,
    tool_use_count: u32,
    token_count: u64,
    state: &AgentProgressState,
) -> String {
    let branch = if is_last { BRANCH_LAST } else { BRANCH_MID };
    let mut out = format!("{branch} {name}");
    if matches!(state, AgentProgressState::Backgrounded) {
        return out;
    }
    let tool = if tool_use_count == 1 { "tool use" } else { "tool uses" };
    out.push_str(&format!(
        " \u{00B7} {tool_use_count} {tool} \u{00B7} {} tokens",
        format_token_count(token_count)
    ));
    let status = match state {
        AgentProgressState::Initializing => "Initializing\u{2026}",
        AgentProgressState::Running(activity) => activity.as_str(),
        AgentProgressState::Done => "Done",
        AgentProgressState::Backgrounded => unreachable!(),
    };
    let cont = if is_last { "   " } else { "\u{2502}  " };
    out.push_str(&format!("\n{cont}{STATUS_GUTTER}{status}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_glyph_bytes() {
        assert_eq!(BRANCH_MID.as_bytes(), &[0xE2, 0x94, 0x9C, 0xE2, 0x94, 0x80]); // ├─
        assert_eq!(BRANCH_LAST.as_bytes(), &[0xE2, 0x94, 0x94, 0xE2, 0x94, 0x80]); // └─
    }

    #[test]
    fn mid_running() {
        let out = render_agent_progress_line("explorer", false, 2, 5100, &AgentProgressState::Running("Reading files".into()));
        assert_eq!(out, "\u{251C}\u{2500} explorer \u{00B7} 2 tool uses \u{00B7} 5.1k tokens\n\u{2502}  \u{23BF}  Reading files");
    }

    #[test]
    fn last_initializing_singular_tool() {
        let out = render_agent_progress_line("writer", true, 1, 512, &AgentProgressState::Initializing);
        assert_eq!(out, "\u{2514}\u{2500} writer \u{00B7} 1 tool use \u{00B7} 512 tokens\n   \u{23BF}  Initializing\u{2026}");
    }

    #[test]
    fn done() {
        let out = render_agent_progress_line("writer", true, 3, 2300, &AgentProgressState::Done);
        assert_eq!(out, "\u{2514}\u{2500} writer \u{00B7} 3 tool uses \u{00B7} 2.3k tokens\n   \u{23BF}  Done");
    }

    #[test]
    fn backgrounded_tree_only() {
        let out = render_agent_progress_line("bg", true, 9, 9999, &AgentProgressState::Backgrounded);
        assert_eq!(out, "\u{2514}\u{2500} bg");
    }
}
```

- [ ] **Step 3: Run + wire + build** — add `pub mod agent_progress;` to `coordinator/mod.rs`; `cargo test -p tui --lib components::coordinator::agent_progress` (5 PASS); `cargo build -p tui`.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-06): AgentProgressLine (tree + counts + status states)"
```

---

## Task 4: CoordinatorAgentStatus panel

claude-code `CoordinatorAgentStatus.tsx`: a main `main` line + one line per visible local-agent task, with a selection marker.

**Files:** Create `tui/src/components/coordinator/coordinator_status.rs`; Modify `coordinator/mod.rs` (+ `pub mod coordinator_status;`); Create `tui/tests/coordinator_chrome.rs`.

- [ ] **Step 1: Create with tests** — `coordinator_status.rs`:

```rust
//! `CoordinatorAgentStatus` panel (claude-code `CoordinatorAgentStatus.tsx`):
//! a `main` line + one line per teammate, with a `>`-marked selection. Names
//! are colored by the dialog (this renders the plain text + selection marker).

use crate::multiagent::state::WorkerRow;

/// Render the coordinator panel: a header, the `main` line, and one line per
/// worker. `selected` highlights a row (`0` == the `main` line).
#[must_use]
pub fn render_coordinator_status(workers: &[WorkerRow], selected: usize) -> String {
    let mut out = String::from("Agents\n");
    let marker = |i: usize| if i == selected { "> " } else { "  " };
    out.push_str(marker(0));
    out.push_str("main");
    for (i, w) in workers.iter().enumerate() {
        out.push('\n');
        out.push_str(marker(i + 1));
        out.push_str(&format!("@{}: {}", w.name, w.status));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(name: &str, status: &str) -> WorkerRow {
        WorkerRow { agent_id: "a".into(), name: name.into(), agent_type: "explorer".into(), status: status.into() }
    }

    #[test]
    fn main_selected() {
        let out = render_coordinator_status(&[w("alice", "working")], 0);
        assert_eq!(out, "Agents\n> main\n  @alice: working");
    }

    #[test]
    fn worker_selected() {
        let out = render_coordinator_status(&[w("alice", "working"), w("bob", "idle")], 2);
        assert_eq!(out, "Agents\n  main\n  @alice: working\n> @bob: idle");
    }
}
```

- [ ] **Step 2: Snapshots** — create `tui/tests/coordinator_chrome.rs`:

```rust
//! M9-06 — coordinator chrome snapshots + a fixture WorkersRefreshed check.

use tui::components::coordinator::agent_progress::{render_agent_progress_line, AgentProgressState};
use tui::components::coordinator::coordinator_status::render_coordinator_status;
use tui::components::coordinator::team_status::render_team_footer;
use tui::components::coordinator::teammate_view_header::render_teammate_view_header;
use tui::multiagent::state::WorkerRow;

fn workers() -> Vec<WorkerRow> {
    vec![
        WorkerRow { agent_id: "a1".into(), name: "alice".into(), agent_type: "explorer".into(), status: "working".into() },
        WorkerRow { agent_id: "a2".into(), name: "bob".into(), agent_type: "writer".into(), status: "idle".into() },
    ]
}

#[test]
fn coordinator_panel() {
    insta::assert_snapshot!("coord_panel", render_coordinator_status(&workers(), 1));
}

#[test]
fn team_footer() {
    insta::assert_snapshot!("coord_team_footer", render_team_footer(&workers(), true).unwrap());
}

#[test]
fn teammate_header() {
    insta::assert_snapshot!("coord_teammate_header", render_teammate_view_header("alice", "Explore the repo"));
}

#[test]
fn agent_progress_tree() {
    let a = render_agent_progress_line("alice", false, 2, 5100, &AgentProgressState::Running("Reading".into()));
    let b = render_agent_progress_line("bob", true, 1, 512, &AgentProgressState::Done);
    insta::assert_snapshot!("coord_agent_progress", format!("{a}\n{b}"));
}
```

- [ ] **Step 3: Run + accept** — add `pub mod coordinator_status;` to `coordinator/mod.rs`; `cargo test -p tui --lib components::coordinator::coordinator_status` (2 PASS); `cargo build -p tui`; `cargo test -p tui --test coordinator_chrome`, inspect + accept the 4 `.snap.new`, re-run green.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-06): CoordinatorAgentStatus panel + chrome snapshots"
```

---

## Task 5: Teammate-view mode (state + esc-to-return)

`AppState.viewing_teammate: Option<String>` + an `Esc`-clears check in `handle_live_key` (a lightweight mode above the editor, below screens/overlays). The header renders when set (Task 6). Key-driven ENTRY from the footer + transcript filtering are deferred (worker feed is fixture-only) — this delivers the mode + exit + a programmatic enter.

**Files:** Modify `tui/src/state.rs`, `tui/src/root.rs`; Modify `tui/tests/coordinator_chrome.rs`.

- [ ] **Step 1: State + helpers** — in `tui/src/state.rs`, add the field to `AppState` (near `active_screen`):

```rust
    /// (M9-06) When `Some`, the transcript is in teammate-view mode for this
    /// teammate name; `Esc` returns. Entry is programmatic until the worker
    /// feed is live (R4/R5).
    pub viewing_teammate: Option<String>,
```

Initialize it to `None` in `AppState::new` (find the struct initializer and add `viewing_teammate: None,`). Add helpers near `close_screen`:

```rust
    /// Enter teammate-view for `name`.
    pub fn enter_teammate_view(&mut self, name: String) {
        self.viewing_teammate = Some(name);
    }

    /// Leave teammate-view. Returns `true` if a view was active.
    pub fn leave_teammate_view(&mut self) -> bool {
        self.viewing_teammate.take().is_some()
    }
```

- [ ] **Step 2: Esc handling in handle_live_key** — in `tui/src/root.rs`, add a check that runs AFTER the permission(1) + active_screen(2) + overlay(3) gates but BEFORE normal editor input: if `viewing_teammate.is_some()` and the key is `Esc`, leave the view and consume the key:

```rust
    // (M9-06) Teammate-view mode: Esc returns to the normal transcript.
    if st.viewing_teammate.is_some() && k.code == KeyCode::Esc {
        st.leave_teammate_view();
        return; // match the crate's "key handled" control-flow (see neighbors)
    }
```

> Place + short-circuit it exactly as the neighboring global checks do (the grounding shows the ladder ~lines 426–695; put this near the other post-overlay global keys). Ensure it does NOT shadow the screen/permission gates above it.

- [ ] **Step 3: Behavior test** — append to `tui/tests/coordinator_chrome.rs` a state-level test (the public `AppState` helpers; if `AppState::new` needs args, use the crate's existing test constructor — search `tui/tests` for how other tests build an `AppState`):

```rust
#[test]
fn teammate_view_enter_leave() {
    // Uses the public AppState helpers (construct via the crate's test seam).
    let mut s = tui::state::AppState::new_for_test();
    assert!(s.viewing_teammate.is_none());
    s.enter_teammate_view("alice".into());
    assert_eq!(s.viewing_teammate.as_deref(), Some("alice"));
    assert!(s.leave_teammate_view());
    assert!(s.viewing_teammate.is_none());
    assert!(!s.leave_teammate_view());
}
```

> If no `new_for_test` exists, use whatever constructor the existing `tui/tests/*` use for `AppState` (search for `AppState::new`), or move this test into `state.rs`'s `#[cfg(test)] mod tests` where a state value is easy to build. Adjust to compile.

- [ ] **Step 4: Build + test** — `cargo build -p tui`; `cargo test -p tui --test coordinator_chrome` (+ the state test). Fix any constructor mismatch.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-06): teammate-view mode state + esc-to-return"
```

---

## Task 6: Layout integration (TeamStatus footer + TeammateViewHeader)

**Files:** Modify `tui/src/root.rs` (or the REPL screen component that owns the footer — the M9-05 task footer's host).

- [ ] **Step 1: Render the TeamStatus footer** — where the M9-05 task footer is rendered (the `ReplScreen`/root footer zone), also render the team footer when present:

```rust
            #(crate::components::coordinator::team_status::render_team_footer(&st.multiagent.workers, false).map(|line| element! {
                View(flex_direction: FlexDirection::Row) { Text(content: line, color: theme.dim) }
            }))
```

> Mirror exactly how the task footer (`render_task_footer`) was threaded in M9-05 (it used a `ReplScreen` prop `task_footer: Option<String>`). Add a sibling `team_footer: Option<String>` prop the same way, or render inline if the task footer is rendered inline. Match the established M9-05 approach so existing snapshots are unaffected (the prop defaults to `None`).

- [ ] **Step 2: Render the TeammateViewHeader** — when `st.viewing_teammate` is `Some(name)`, render the header above the transcript:

```rust
            #(st.viewing_teammate.as_ref().map(|name| {
                let header = crate::components::coordinator::teammate_view_header::render_teammate_view_header(name, "");
                element! { View(flex_direction: FlexDirection::Column) { Text(content: header, color: theme.claude) } }
            }))
```

> Place it where a transcript banner fits (above the scrollback / VirtualMessageList). Use the active theme variable in scope. Keep it behind the `Some` guard so it never renders in normal mode (existing snapshots unaffected).

- [ ] **Step 3: Build + verify no snapshot regressions** — `cargo build -p tui`; `cargo test -p tui` (all pass; no `.snap.new` — the new chrome is behind `Option`/`Some` guards, so existing REPL snapshots are unchanged). If any existing snapshot changed unexpectedly, STOP and report (the footer/header must be additive).

- [ ] **Step 4: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-06): render TeamStatus footer + TeammateViewHeader in the layout"
```

---

## Task 7: M9-06 gate + tag

- [ ] **Step 1: Format** — from `lingxi-code/`: `cargo fmt --check`.
- [ ] **Step 2: Clippy** — `cargo clippy -p tui --all-targets 2>&1 | grep -E "tui/src|tui/tests"` → zero tui-originated warnings (pre-existing `tool-api` debt out of scope).
- [ ] **Step 3: Tests** — `cargo test -p tui` (all pass; no `.snap.new`).
- [ ] **Step 4: Workspace build** — `cargo build --workspace` → exit 0.
- [ ] **Step 5: Tag**

```bash
git tag -a m9.6 -m "M9-06: coordinator chrome — TeamStatus footer, TeammateViewHeader, AgentProgressLine, CoordinatorAgentStatus + teammate-view mode scaffolding"
git tag | grep -E '^m9' | sort -V
```

Expect `m9.1`…`m9.6`. **No remote push.**

---

## Forward notes

- **Deferred (gated on the worker pool un-stub, a separate engine milestone):** the live mailbox drain (`TeammateMessage` → `WorkersRefreshed`/teammate `RenderedMessage`s), key-driven teammate entry from the footer, and transcript filtering by teammate. The renderers + the `viewing_teammate` mode are ready; a `WorkerFeed` (fixture today) lights them up unchanged.
- **M9-07** adds the worker permission chrome (`WorkerBadge`, `WorkerPendingPermission`) into the M6 permission focus-trap (priority 1) — the cross-state seam sub-plan.

---

**End of plan.**
