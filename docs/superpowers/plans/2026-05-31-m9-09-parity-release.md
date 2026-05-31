# M9-09 — Parity Fixture + Release v0.10.0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cross-cutting validation + release: add the `parity_tui_multiagent` fixture, repair the `test-harness` parity suite for the new `Screen` variants, audit telemetry, bump crate versions 0.8.0 → 0.10.0 (reconciling M8's un-bumped `[0.9.0]`), write the CHANGELOG/README entries, tag `m9.9` + `v0.10.0`, run the final cross-state-seam review, and prepare the `m9-execution` merge.

**Architecture:** This sub-plan ships no new TUI feature — it validates + releases the M9-01…M9-08 surface. The parity fixture follows the existing `test-harness/src/parity/fixtures/*.json` + `test-harness/tests/parity_tui_*.rs` pattern, asserting the M9 renderers' high-value strings through their pure `render_*_to_string` functions (structure, not per-token color — §0 Q3).

**Tech Stack:** Rust 1.82.0; release mechanics (Cargo versions, CHANGELOG, tags). Run cargo from inside `lingxi-code/`.

**Latent fix (must do first):** adding `Screen::BackgroundTasks` (M9-05) + `Screen::Agents` (M9-08) made exhaustive `match` on `Screen` in `test-harness` tests non-exhaustive — NOT caught because the per-sub-plan gates ran `cargo test -p tui` (not `cargo test -p test-harness`). Specifically `test-harness/tests/parity_tui_screens.rs::active_screen_name` (~line 59). This sub-plan repairs it and runs the full parity suite.

**Version reality:** 67 `Cargo.toml` files declare `version = "0.8.0"` literally (not workspace-inherited). CHANGELOG head is `## [0.9.0] — M8 …`; crates were never bumped to 0.9.0. M9-09 bumps all to `0.10.0` in one step + adds a CHANGELOG note. **No remote push; no force-push/amend.**

---

## Task 1: Repair the test-harness parity suite for new Screen variants

**Files:** Modify `test-harness/tests/parity_tui_screens.rs` (+ any other `test-harness` file with an exhaustive `Screen` match).

- [ ] **Step 1: Surface the breakage** — from `lingxi-code/`: `cargo test -p test-harness --no-run 2>&1 | tail -30`. Expect a non-exhaustive-match error in `parity_tui_screens.rs` (`active_screen_name`) for `Screen::BackgroundTasks`/`Screen::Agents` (and possibly other files). Note every site.

- [ ] **Step 2: Fix `active_screen_name`** — in `test-harness/tests/parity_tui_screens.rs`, add the missing arms to the `match &st.active_screen` in `active_screen_name`:

```rust
        Some(Screen::BackgroundTasks(_)) => Some("background_tasks"),
        Some(Screen::Agents(_)) => Some("agents"),
```

(Place them with the other `Some(Screen::...)` arms, before `None`.) Fix any other `test-harness` exhaustive `Screen` match the compiler flags the same way.

- [ ] **Step 3: Verify** — `cargo test -p test-harness 2>&1 | tail -15`. The existing parity tests must compile + pass (allowed known flakes per spec §5.4 may be re-run). If a NON-flaky test fails, STOP and report.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add -A
git commit -m "fix(M9-09): repair test-harness parity suite for new Screen variants"
```

---

## Task 2: parity_tui_multiagent fixture + driver

Assert the M9 renderers' high-value strings through their pure functions (no parallel path).

**Files:** Create `test-harness/src/parity/fixtures/parity_tui_multiagent.json`, `test-harness/tests/parity_tui_multiagent.rs`. Confirm `test-harness/Cargo.toml` already dev-depends on `tui` (the other `parity_tui_*` tests do).

- [ ] **Step 1: Fixture** — create `test-harness/src/parity/fixtures/parity_tui_multiagent.json`:

```json
{
  "scenarios": [
    {
      "name": "task_footer",
      "tasks": [{"task_type": "local_bash", "status": "running", "description": "cargo build"}],
      "expected_contains": ["1 background task", "to view"]
    },
    {
      "name": "task_rows",
      "rows": [
        {"task_type": "local_bash", "status": "completed", "description": "ls", "expected": "ls (done)"},
        {"task_type": "remote_agent", "status": "running", "description": "deploy", "expected": "deploy"},
        {"task_type": "in_process_teammate", "status": "running", "description": "alice", "expected": "@alice"}
      ]
    },
    {
      "name": "coordinator_chrome",
      "team_footer_expected": "teammates",
      "teammate_header_expected": "Viewing @alice",
      "agent_progress_expected": ["Done", "tool"]
    },
    {
      "name": "worker_permission",
      "badge_expected": "@worker",
      "pending_expected": ["Waiting for team lead approval", "Tool:", "Action:"]
    },
    {
      "name": "agents_screen",
      "agents": [{"name": "explorer", "description": "find things"}],
      "list_expected": ["Agents", "explorer"],
      "detail_expected": ["Description", "Tools:"]
    }
  ]
}
```

- [ ] **Step 2: Driver** — create `test-harness/tests/parity_tui_multiagent.rs`:

```rust
//! Parity (M9-09): the multi-agent TUI surface's high-value strings asserted
//! through the LIVE pure renderers (no parallel path). Structure, not
//! per-token color (§0 Q3).

use serde_json::Value;
use tui::components::coordinator::agent_progress::{render_agent_progress_line, AgentProgressState};
use tui::components::coordinator::team_status::render_team_footer;
use tui::components::coordinator::teammate_view_header::render_teammate_view_header;
use tui::components::permissions::worker::{render_worker_badge, render_worker_pending_to_string};
use tui::components::tasks::render_task_row;
use tui::components::tasks::status_footer::render_task_footer;
use tui::multiagent::state::{TaskRow, WorkerRow};
use tui::screens::agents::{render_agents_to_string, AgentRow, AgentsDialogMode, AgentsScreenState};

const FIXTURE: &str = include_str!("../src/parity/fixtures/parity_tui_multiagent.json");

fn load() -> Value {
    serde_json::from_str(FIXTURE).expect("parity_tui_multiagent.json parses")
}

fn task_row(t: &str, s: &str, d: &str) -> TaskRow {
    TaskRow { task_id: "b1".into(), task_type: t.into(), status: s.into(), description: d.into() }
}

#[test]
fn task_footer_strings() {
    let f = load();
    let sc = f["scenarios"][0].clone();
    let tasks: Vec<TaskRow> = sc["tasks"].as_array().unwrap().iter()
        .map(|t| task_row(t["task_type"].as_str().unwrap(), t["status"].as_str().unwrap(), t["description"].as_str().unwrap()))
        .collect();
    let footer = render_task_footer(&tasks).expect("footer present");
    for needle in sc["expected_contains"].as_array().unwrap() {
        assert!(footer.contains(needle.as_str().unwrap()), "footer `{footer}` missing `{needle}`");
    }
}

#[test]
fn task_rows_strings() {
    let f = load();
    for row in f["scenarios"][1]["rows"].as_array().unwrap() {
        let r = task_row(row["task_type"].as_str().unwrap(), row["status"].as_str().unwrap(), row["description"].as_str().unwrap());
        let out = render_task_row(&r);
        let expected = row["expected"].as_str().unwrap();
        assert!(out.contains(expected), "row `{out}` missing `{expected}`");
    }
}

#[test]
fn coordinator_chrome_strings() {
    let f = load();
    let sc = f["scenarios"][2].clone();
    let workers = vec![WorkerRow { agent_id: "a".into(), name: "alice".into(), agent_type: "explorer".into(), status: "working".into() }];
    assert!(render_team_footer(&workers, true).unwrap().contains(sc["team_footer_expected"].as_str().unwrap()));
    assert!(render_teammate_view_header("alice", "task").contains(sc["teammate_header_expected"].as_str().unwrap()));
    let prog = render_agent_progress_line("alice", true, 2, 100, &AgentProgressState::Done);
    for needle in sc["agent_progress_expected"].as_array().unwrap() {
        assert!(prog.contains(needle.as_str().unwrap()), "progress `{prog}` missing `{needle}`");
    }
}

#[test]
fn worker_permission_strings() {
    let f = load();
    let sc = f["scenarios"][3].clone();
    assert!(render_worker_badge("worker").contains(sc["badge_expected"].as_str().unwrap()));
    let pending = render_worker_pending_to_string("Bash", "run ls", Some("worker"), Some("team"));
    for needle in sc["pending_expected"].as_array().unwrap() {
        assert!(pending.contains(needle.as_str().unwrap()), "pending `{pending}` missing `{needle}`");
    }
}

#[test]
fn agents_screen_strings() {
    let f = load();
    let sc = f["scenarios"][4].clone();
    let rows: Vec<AgentRow> = sc["agents"].as_array().unwrap().iter()
        .map(|a| AgentRow { name: a["name"].as_str().unwrap().into(), description: a["description"].as_str().unwrap().into(), tools: vec![], ..AgentRow::default() })
        .collect();
    let list = render_agents_to_string(&AgentsScreenState { rows: rows.clone(), selected: 0, mode: AgentsDialogMode::List });
    for needle in sc["list_expected"].as_array().unwrap() {
        assert!(list.contains(needle.as_str().unwrap()), "list `{list}` missing `{needle}`");
    }
    let detail = render_agents_to_string(&AgentsScreenState { rows, selected: 0, mode: AgentsDialogMode::Detail });
    for needle in sc["detail_expected"].as_array().unwrap() {
        assert!(detail.contains(needle.as_str().unwrap()), "detail `{detail}` missing `{needle}`");
    }
}
```

- [ ] **Step 2b: Verify the public paths** — confirm each `use` resolves (the functions are `pub` and the modules are re-exported). If any path differs (e.g. `render_task_footer` is under a different module), fix the `use` to the real path. `cargo test -p test-harness --test parity_tui_multiagent`. If an `expected` substring does not match the real render output, **fix the FIXTURE to the real output** (the renderers are the source of truth — they were byte-locked in their own sub-plans), not the renderer.

- [ ] **Step 3: Run** — `cargo test -p test-harness --test parity_tui_multiagent` → 5 PASS.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add -A
git commit -m "test(M9-09): parity_tui_multiagent fixture + driver"
```

---

## Task 3: Telemetry audit

**Files:** Modify `tui/src/telemetry.rs` (only if a name is missing an emit site or unregistered).

- [ ] **Step 1: Audit** — `grep -n "screen_opened\|tengu_tui\|lingxi_core_v" tui/src/telemetry.rs` and `grep -rn "screen_opened(" tui/src`. Verify: every screen name passed to `screen_opened(..)` (incl. the M9 `background_tasks` from M9-05's opener and `agents` from M9-08) is a real emit site, and the M9-05 Shift+Down opener actually calls `screen_opened("background_tasks")` (if it does not, add it for consistency with the other screen openers — the M6 "every registered name has an emit site" discipline). Report the final set.

- [ ] **Step 2: Release marker (if the pattern exists)** — if prior releases registered a once-guarded marker (e.g. `lingxi_core_v0_8_0_released`), add `lingxi_core_v0_10_0_released` following the exact pattern + emit site. If no such pattern exists, skip (do not invent one).

- [ ] **Step 3: Commit (if changed)**

```bash
cargo fmt -p tui
git add -A
git commit -m "chore(M9-09): telemetry audit — M9 screen events + v0.10.0 release marker"
```

> If the audit finds nothing to change, skip the commit and note "telemetry already complete" in the report.

---

## Task 4: Version bump 0.8.0 → 0.10.0 + CHANGELOG + README

**Files:** all `Cargo.toml` with `version = "0.8.0"`; `CHANGELOG.md`; `README.md`.

- [ ] **Step 1: Bump every crate version** — from `lingxi-code/`:

```bash
grep -rl 'version = "0.8.0"' --include=Cargo.toml . | while read -r f; do
  sed -i '' 's/version = "0.8.0"/version = "0.10.0"/' "$f"   # macOS sed; use `sed -i` on Linux
done
grep -rln 'version = "0.8.0"' --include=Cargo.toml . | wc -l   # expect 0
grep -rln 'version = "0.10.0"' --include=Cargo.toml . | wc -l  # expect 67
```

> The toolchain pins macOS `sed` (BSD: `sed -i ''`). If on Linux, use `sed -i`. Only replace the exact crate `version = "0.8.0"` line — do NOT touch dependency version specs that happen to say `0.8.0` (there should be none, since intra-workspace deps use `path = `; verify with `git diff --stat`).

- [ ] **Step 2: Cargo.lock** — `cargo build --workspace` to regenerate `Cargo.lock` with the new versions (or `cargo update -w` then build). Confirm it builds.

- [ ] **Step 3: CHANGELOG** — prepend a new entry ABOVE `## [0.9.0]` in `CHANGELOG.md`:

```markdown
## [0.10.0] — M9 Multi-Agent TUI Surface

Completes the multi-agent TUI: the message types, status chrome, dialogs, and
read-only agent discovery a user sees when subagents, in-process teammates, and
background tasks are active — built UI-first against a presentation adapter,
rendering real data on the live `TaskRegistryHandle` path and deterministic
fixtures where the execution pool is still stubbed.

Highlights:

- **Team message renderers (M9-03):** TaskAssignment, UserTeammate
  (task-completed + note), UserAgentNotification, UserChannel, plus team-memory
  collapse/saved parts — literal-locked to claude-code.
- **Background tasks (M9-04/05):** per-`TaskState` row renderers (7) +
  ShellProgress + live output tailing (`TaskRegistryHandle::output`); the
  `BackgroundTaskStatus` footer + `BackgroundTasksDialog` (list↔detail) routed
  via `active_screen`; the real `tasks::TaskRegistry` wired into the desktop
  composition root + a `MultiAgentEvent` render-loop pump.
- **Coordinator chrome (M9-06):** TeamStatus footer, TeammateViewHeader,
  AgentProgressLine (tree-char progress), CoordinatorAgentStatus panel +
  teammate-view mode.
- **Worker permissions (M9-07):** WorkerBadge + WorkerPendingPermission in the
  M6 permission focus-trap, with a cross-state-seam test.
- **Agent discovery (M9-08):** read-only `AgentsList` + `AgentDetail` via a
  `/agents` screen (catalog from `OrchestratorHandle::list_agents`).

**Versioning note:** M8 documented `[0.9.0]` in this changelog but never bumped
the crate versions (they stayed `0.8.0`). v0.10.0 bumps all workspace crates
`0.8.0` → `0.10.0` in one step; there is no `v0.9.0` tag.
```

- [ ] **Step 4: README** — if `README.md` has a version/feature line or a milestone list, add a one-line v0.10.0 multi-agent-TUI note consistent with the existing style. If there is no such section, skip (do not restructure the README).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "release(M9-09): bump crates 0.8.0 → 0.10.0 + CHANGELOG/README for v0.10.0"
```

---

## Task 5: Final gate + cross-state-seam review + tags

- [ ] **Step 1: Full gate** — from `lingxi-code/`:
  - `cargo fmt --check`
  - `cargo clippy -p tui --all-targets 2>&1 | grep -E "tui/src|tui/tests"` → zero tui-originated warnings
  - `cargo test -p tui` → all pass; `find tui/tests -name '*.snap.new'` empty
  - `cargo test -p test-harness` → all pass (the parity suite incl. the new fixture; allowed known flakes may be re-run)
  - `cargo build --workspace` → exit 0
  - `scripts/check-deps.sh` → OK

- [ ] **Step 2: Cross-state-seam review (named, per §5.6)** — confirm the three seams are covered by existing tests + read the routing once: (a) worker-permission while a screen is open → permission wins (priority 1) — `tui/tests/worker_permission.rs::seam`; (b) background-tasks dialog vs a pending permission — `tui/tests/cross_state_seam_test.rs` + the M9-05 `routing_seam`; (c) teammate-view esc vs the gates — `handle_live_key` ordering. State the verdict.

- [ ] **Step 3: Tag**

```bash
git tag -a m9.9 -m "M9-09: parity_tui_multiagent fixture + telemetry audit + version 0.10.0 + CHANGELOG/README"
git tag -a v0.10.0 -m "v0.10.0 — M9 Multi-Agent TUI Surface (M9-01…M9-09)"
git tag | grep -E '^m9|^v0\.10' | sort -V
```

Expect `m9.1`…`m9.9` + `v0.10.0`. **No remote push.**

---

## Task 6: Merge prep + handoff

- [ ] **Step 1: Summarize** — produce the final milestone summary: the 9 sub-plans, their tags, the test counts, the deferred-to-engine items (pool un-stub, live worker/mailbox feed, agent authoring), and the cross-state-seam verdict.
- [ ] **Step 2: Merge decision** — per design §6.5/§6.6, the `m9-execution` → integration fast-forward merge happens AFTER the cross-state-seam review, and §6.6 says "pause for review — no auto-progression." Present the completed, tagged `v0.10.0` for the user's final review + merge confirmation (the merge is local; `superpowers:finishing-a-development-branch` is the mechanism). Do NOT push to any remote.

---

## Notes

- This sub-plan touches `Cargo.toml` versions broadly — keep the diff to the `version =` line only (verify with `git diff --stat` before committing).
- Deferred-to-engine (documented across M9-04…08): execution-pool un-stub, live worker/mailbox feed, live worker-permission origination, the richer agent-detail fields, agent authoring. The UI surface + adapters are ready; each lights up when its engine support lands.

---

**End of plan.**
