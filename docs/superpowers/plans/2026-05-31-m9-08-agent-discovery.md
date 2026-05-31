# M9-08 — Agent Discovery (read-only) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the read-only agent-discovery screen — `AgentsList` (browse the agent catalog) + `AgentDetail` (per-agent fields) — opened by a `/agents` command, routed via `active_screen`, following the `background_tasks.rs` list↔detail pattern.

**Architecture:** A new `Screen::Agents(AgentsScreenState)` with a pure reducer (selected index + List/Detail mode) and a `render_agents_to_string` (list + detail), in `tui/src/screens/agents.rs`. Rows are an `AgentRow` presentation struct. The **live** `/agents` path fetches via the already-reachable `OrchestratorHandle::list_agents() -> Vec<AgentInfo>` (name/description/tools_allowed); the richer detail fields (model/permission-mode/color/source/path) are **renderer parameters** — fixture-supplied for snapshots and `None` live, since the frozen `traits` `OrchestratorHandle` does not surface them (spec §2.4 omit; Q3 fixtures-where-stubbed). No `traits` change.

**Tech Stack:** Rust 1.82.0, `iocraft = "=0.8.3"`, `insta`, `tokio` (the async-open fetch). Run cargo from inside `lingxi-code/`.

**Literal-lock reference:** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/agents/AgentsList.tsx` + `AgentDetail.tsx` + `AgentNavigationFooter.tsx`.

**Locked literals (verified):**
- AgentsList: title `Agents`; footer hint `Press ↑↓ to navigate · Enter to select · Esc to go back`; selection marker `❯ ` (or `  `); model badge ` · {model}`.
- AgentDetail: top line = file path (dim, when present); `Description (tells Claude when to use this agent):` then indented `{description}`; `Tools: ` then `All tools` (empty allow-list = all) / joined `, `; optional `Model: {model}`, `Permission mode: {mode}`, `Color: {agent_type}`, footer `esc to go back`. Labels are bold; ` · ` = U+00B7.

**Catalog reality (locked):** `OrchestratorHandle::list_agents()` returns `AgentInfo { name, description, tools_allowed }` (3 fields). The full `AgentDefinition` (model/permission/color/source/path) is NOT on the frozen trait → those are fixture-only renderer params. The live list/detail shows name + description + tools.

---

## File Structure

| File | Responsibility | C/M |
|---|---|---|
| `tui/src/screens/agents.rs` | `AgentRow`, `AgentsScreenState`, `AgentsDialogMode`, `AgentsOutcome`, `handle_agents_key`, `render_agents_to_string` | Create |
| `tui/src/screens/mod.rs` | + `pub mod agents;` + `Screen::Agents(..)` variant | Modify |
| `tui/src/state.rs` | + `open_agents(rows)` helper | Modify |
| `tui/src/root.rs` | + `handle_screen_key` arm for `Screen::Agents` | Modify |
| `tui/src/app.rs` | + `/agents` dispatch intercept (fetch rows → open screen) | Modify |
| `tui/tests/agents_screen.rs` | snapshots + behavior tests | Create |

---

## Task 1: AgentsScreenState + reducer + render

**Files:** Create `tui/src/screens/agents.rs`; Modify `tui/src/screens/mod.rs`.

- [ ] **Step 1: Read** `tui/src/screens/background_tasks.rs` (the list↔detail reducer pattern to mirror) + the claude-code `AgentsList.tsx`/`AgentDetail.tsx` for literals.

- [ ] **Step 2: Create with tests** — `tui/src/screens/agents.rs`:

```rust
//! `/agents` discovery screen (claude-code `AgentsList.tsx` + `AgentDetail.tsx`):
//! a list of agent definitions with a per-agent detail view. Pure reducer over
//! a selected index + a mode (mirrors `background_tasks.rs`). The live path
//! fills `name`/`description`/`tools` from `OrchestratorHandle::list_agents`;
//! the richer detail fields are fixture/`None` (frozen trait, spec §2.4).

/// One agent row. `name`/`description`/`tools` come from the wire
/// (`AgentInfo`); the rest are optional detail fields (fixture-supplied).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentRow {
    /// Agent type / name.
    pub name: String,
    /// `when_to_use` description.
    pub description: String,
    /// Allowed tools (empty = all tools).
    pub tools: Vec<String>,
    /// Optional model display.
    pub model: Option<String>,
    /// Optional permission mode.
    pub permission_mode: Option<String>,
    /// Optional color name.
    pub color: Option<String>,
    /// Optional source/file path (shown dim atop the detail).
    pub path: Option<String>,
}

/// List vs. detail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentsDialogMode {
    /// Browsing the agent list.
    #[default]
    List,
    /// Viewing one agent's detail.
    Detail,
}

/// Screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentsScreenState {
    /// The agent catalog rows.
    pub rows: Vec<AgentRow>,
    /// Selected row index.
    pub selected: usize,
    /// List or detail.
    pub mode: AgentsDialogMode,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsOutcome {
    /// Stay open.
    Stay,
    /// Close the screen.
    Close,
}

/// Reduce a key (mirrors `handle_background_tasks_key`).
#[must_use]
pub fn handle_agents_key(
    state: &mut AgentsScreenState,
    key: crossterm::event::KeyCode,
) -> AgentsOutcome {
    use crossterm::event::KeyCode;
    match state.mode {
        AgentsDialogMode::List => match key {
            KeyCode::Up | KeyCode::Char('k') => {
                state.selected = state.selected.saturating_sub(1);
                AgentsOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !state.rows.is_empty() {
                    state.selected = (state.selected + 1).min(state.rows.len() - 1);
                }
                AgentsOutcome::Stay
            }
            KeyCode::Enter => {
                if !state.rows.is_empty() {
                    state.mode = AgentsDialogMode::Detail;
                }
                AgentsOutcome::Stay
            }
            KeyCode::Esc | KeyCode::Char('q') => AgentsOutcome::Close,
            _ => AgentsOutcome::Stay,
        },
        AgentsDialogMode::Detail => match key {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => {
                state.mode = AgentsDialogMode::List;
                AgentsOutcome::Stay
            }
            _ => AgentsOutcome::Stay,
        },
    }
}

/// Render the screen body (claude-code `AgentsList`/`AgentDetail`).
#[must_use]
pub fn render_agents_to_string(state: &AgentsScreenState) -> String {
    match state.mode {
        AgentsDialogMode::List => {
            let mut out = String::from("Agents\n");
            if state.rows.is_empty() {
                out.push_str("No agents found.");
                return out;
            }
            for (i, row) in state.rows.iter().enumerate() {
                let marker = if i == state.selected { "\u{276F} " } else { "  " };
                out.push_str(marker);
                out.push_str(&row.name);
                if let Some(m) = &row.model {
                    out.push_str(&format!(" \u{00B7} {m}"));
                }
                out.push('\n');
            }
            out.push_str("Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back");
            out
        }
        AgentsDialogMode::Detail => match state.rows.get(state.selected) {
            Some(row) => render_agent_detail(row),
            None => "Agents\n(agent no longer available)".to_string(),
        },
    }
}

/// The detail body for one agent.
fn render_agent_detail(row: &AgentRow) -> String {
    let mut out = String::new();
    if let Some(p) = &row.path {
        out.push_str(p);
        out.push('\n');
    }
    out.push_str("Description (tells Claude when to use this agent):\n  ");
    out.push_str(&row.description);
    out.push('\n');
    let tools = if row.tools.is_empty() {
        "All tools".to_string()
    } else {
        row.tools.join(", ")
    };
    out.push_str(&format!("Tools: {tools}"));
    if let Some(m) = &row.model {
        out.push_str(&format!("\nModel: {m}"));
    }
    if let Some(pm) = &row.permission_mode {
        out.push_str(&format!("\nPermission mode: {pm}"));
    }
    if let Some(c) = &row.color {
        out.push_str(&format!("\nColor: {c}"));
    }
    out.push_str("\nesc to go back");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn row(name: &str) -> AgentRow {
        AgentRow { name: name.into(), description: "does things".into(), tools: vec![], ..AgentRow::default() }
    }

    #[test]
    fn nav_and_enter_and_esc() {
        let mut s = AgentsScreenState { rows: vec![row("a"), row("b")], ..AgentsScreenState::default() };
        assert_eq!(handle_agents_key(&mut s, KeyCode::Down), AgentsOutcome::Stay);
        assert_eq!(s.selected, 1);
        assert_eq!(handle_agents_key(&mut s, KeyCode::Enter), AgentsOutcome::Stay);
        assert_eq!(s.mode, AgentsDialogMode::Detail);
        // Esc in detail → back to list.
        assert_eq!(handle_agents_key(&mut s, KeyCode::Esc), AgentsOutcome::Stay);
        assert_eq!(s.mode, AgentsDialogMode::List);
        // Esc in list → close.
        assert_eq!(handle_agents_key(&mut s, KeyCode::Esc), AgentsOutcome::Close);
    }

    #[test]
    fn list_render_marks_selection_and_model_badge() {
        let mut rows = vec![row("explorer"), row("writer")];
        rows[0].model = Some("opus".into());
        let s = AgentsScreenState { rows, selected: 0, mode: AgentsDialogMode::List };
        let out = render_agents_to_string(&s);
        assert!(out.starts_with("Agents\n\u{276F} explorer \u{00B7} opus\n  writer\n"));
        assert!(out.ends_with("Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back"));
    }

    #[test]
    fn detail_render_fields() {
        let r = AgentRow {
            name: "explorer".into(),
            description: "find things".into(),
            tools: vec!["Read".into(), "Grep".into()],
            model: Some("opus".into()),
            permission_mode: Some("plan".into()),
            color: Some("cyan".into()),
            path: Some(".lingxi/agents/explorer.md".into()),
        };
        let out = render_agent_detail(&r);
        assert_eq!(
            out,
            ".lingxi/agents/explorer.md\nDescription (tells Claude when to use this agent):\n  find things\nTools: Read, Grep\nModel: opus\nPermission mode: plan\nColor: cyan\nesc to go back"
        );
    }

    #[test]
    fn detail_empty_tools_is_all() {
        let out = render_agent_detail(&row("x"));
        assert!(out.contains("Tools: All tools"));
    }
}
```

- [ ] **Step 3: Add the Screen variant** — in `tui/src/screens/mod.rs`: add `pub mod agents;` and the variant `Agents(agents::AgentsScreenState)` to the `Screen` enum. `AgentsScreenState` derives `PartialEq, Eq` (all fields support it) so `Screen: PartialEq` holds.

- [ ] **Step 4: Run + build** — `cargo test -p tui --lib screens::agents` (4 PASS); `cargo build -p tui` (the new `Screen` variant will force a `render_screen` arm in `app.rs` and possibly a `handle_screen_key` arm — add minimal arms: render via `render_agents_to_string` like `BackgroundTasks` does; the key arm is Task 2 — for now a `Screen::Agents` arm in `handle_screen_key` that calls the reducer, see Task 2).

> If the build flags the missing `handle_screen_key`/`render_screen` arms before Task 2, add them now (they are small and mirror `BackgroundTasks`): the `render_screen` arm renders `render_agents_to_string(state)`; the `handle_screen_key` arm calls `handle_agents_key` and closes on `Close`.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-08): AgentsScreen state + reducer + list/detail render"
```

---

## Task 2: `/agents` command → screen + handle_screen_key arm

**Files:** Modify `tui/src/state.rs` (open helper), `tui/src/root.rs` (handle_screen_key arm if not added in Task 1), `tui/src/app.rs` (dispatch).

- [ ] **Step 1: open_agents helper** — in `tui/src/state.rs`, near `open_doctor`:

```rust
    /// Open the agents screen with the given catalog rows.
    pub fn open_agents(&mut self, rows: Vec<crate::screens::agents::AgentRow>) {
        self.active_screen = Some(crate::screens::Screen::Agents(
            crate::screens::agents::AgentsScreenState { rows, selected: 0, mode: crate::screens::agents::AgentsDialogMode::List },
        ));
        crate::telemetry::screen_opened("agents");
    }
```

> Confirm `telemetry::screen_opened` exists + takes a `&str` (the `open_doctor` helper uses it). If the telemetry name must be registered, add `"agents"` following the existing screen names (this feeds the M9-09 telemetry audit).

- [ ] **Step 2: handle_screen_key arm** — in `tui/src/root.rs` `handle_screen_key`, add (if not already added in Task 1):

```rust
        Some(Screen::Agents(state)) => {
            match crate::screens::agents::handle_agents_key(state, ct_key.code) {
                crate::screens::agents::AgentsOutcome::Close => st.close_screen(),
                crate::screens::agents::AgentsOutcome::Stay => {}
            }
        }
```

> Use the same crossterm-key binding the neighboring arms use (`ct_key` / `ct`). Add the `render_screen` arm in `app.rs` if not present: render `render_agents_to_string(state)` line-by-line in a column `View` (mirror the `BackgroundTasks` render arm exactly).

- [ ] **Step 3: `/agents` dispatch** — in `tui/src/app.rs`, where slash commands are intercepted (search for the `/doctor` intercept), add a `/agents` branch. It must fetch the catalog from the orchestrator (async). Mirror how `/config` or `/status` does its **async** open (the grounding notes those use `TuiSessionState.orchestrator`); if the dispatch site is sync, open with the rows fetched via the existing async-open mechanism. The fetch maps `AgentInfo` → `AgentRow`:

```rust
    // /agents → open the discovery screen (rows from the orchestrator catalog).
    if st.prompt_text.trim() == "/agents" {
        st.prompt_text.clear();
        st.prompt_cursor = 0;
        // Fetch via the orchestrator handle (mirror the async-open path used by
        // /config). Map AgentInfo → AgentRow (name/description/tools; rich
        // fields None — the frozen trait does not expose them).
        // let infos = orchestrator.list_agents().await;
        // let rows = infos.into_iter().map(|i| AgentRow {
        //     name: i.name, description: i.description, tools: i.tools_allowed, ..Default::default()
        // }).collect();
        // st.open_agents(rows);
        return false;
    }
```

> Implement the fetch using the EXACT async-open mechanism `/config`/`/status` use (read that code first — it likely sets a `pending_*` flag that an async pump resolves, OR awaits directly if the dispatch is async). If the simplest correct path is to open the screen with `rows = vec![]` synchronously and let an async pump fill them, do that and mirror the existing pattern. Do NOT block the render thread.

- [ ] **Step 4: Build + a behavior test** — `cargo build -p tui`. Add a behavior test in `tui/tests/agents_screen.rs` that drives the reducer through the public seam (or, if `/agents` dispatch is reachable in tests, asserts it opens `Screen::Agents`). At minimum, assert the reducer nav/enter/esc (already unit-tested) and that `open_agents` sets the screen.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-08): /agents command opens the discovery screen (catalog via orchestrator)"
```

---

## Task 3: Snapshots + gate + tag

**Files:** Create/extend `tui/tests/agents_screen.rs`.

- [ ] **Step 1: Snapshots** — in `tui/tests/agents_screen.rs`:

```rust
//! M9-08 — agents screen snapshots.

use tui::screens::agents::{render_agents_to_string, AgentRow, AgentsDialogMode, AgentsScreenState};

fn rows() -> Vec<AgentRow> {
    vec![
        AgentRow { name: "explorer".into(), description: "find things".into(), tools: vec!["Read".into(), "Grep".into()], model: Some("opus".into()), permission_mode: Some("plan".into()), color: Some("cyan".into()), path: Some(".lingxi/agents/explorer.md".into()) },
        AgentRow { name: "writer".into(), description: "writes code".into(), tools: vec![], ..AgentRow::default() },
    ]
}

#[test]
fn agents_list_snapshot() {
    let s = AgentsScreenState { rows: rows(), selected: 0, mode: AgentsDialogMode::List };
    insta::assert_snapshot!("agents_list", render_agents_to_string(&s));
}

#[test]
fn agents_detail_snapshot() {
    let s = AgentsScreenState { rows: rows(), selected: 0, mode: AgentsDialogMode::Detail };
    insta::assert_snapshot!("agents_detail", render_agents_to_string(&s));
}
```

Run `cargo test -p tui --test agents_screen`, inspect + accept the 2 `.snap.new`, re-run green.

- [ ] **Step 2: Gate** — from `lingxi-code/`: `cargo fmt --check`; `cargo clippy -p tui --all-targets 2>&1 | grep -E "tui/src|tui/tests"` (zero tui warnings); `cargo test -p tui` (all pass; no `.snap.new`); `cargo build --workspace` (exit 0).

- [ ] **Step 3: Tag**

```bash
git tag -a m9.8 -m "M9-08: read-only agent discovery — AgentsList + AgentDetail + /agents screen (catalog via OrchestratorHandle::list_agents)"
git tag | grep -E '^m9' | sort -V
```

Expect `m9.1`…`m9.8`. **No remote push.**

---

## Forward notes

- **Deferred (frozen trait):** the rich detail fields (model/permission-mode/color/source/path) + source-grouped list headers + model/memory/override badges render from fixtures today; they light up if `OrchestratorHandle` ever exposes full `AgentDefinition`s (or a `tui → agent` catalog dep is added — allowed by check-deps but out of scope here). Agent **authoring** (CreateAgentWizard/editor) is a separate milestone (design §1 non-goal).
- **M9-09** is the release: `parity_tui_multiagent` fixture + literal-lock catalog + telemetry audit + crate version 0.8.0 → 0.10.0 + CHANGELOG/README + final cross-state-seam review + `m9.9` + `v0.10.0`, then the fast-forward merge of `m9-execution`.

---

**End of plan.**
