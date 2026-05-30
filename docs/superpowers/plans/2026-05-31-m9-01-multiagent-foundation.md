# M9-01 Multi-Agent Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the TUI-side multi-agent presentation foundation — `MultiAgentState`, a `MultiAgentEvent` mutation seam, and a swappable `MultiAgentFeed` adapter (fixture + real `TaskRegistryHandle` poller) — fully unit-tested, with zero engine wiring.

**Architecture:** A new `tui/src/multiagent/` module owns a pure presentation model (`MultiAgentState` on `AppState`), a single mutator `apply_multiagent_event` (mirrors the existing `streaming::apply_event`), and a `MultiAgentFeed` trait with two impls: `FixtureFeed` (deterministic, for tests) and `PollerFeed` (over `traits::task_registry::TaskRegistryHandle`, the genuinely-live task path). The `root.rs` pump integration and the desktop `TaskRegistry` construction are intentionally deferred to M9-05 (see "Scope boundary" below).

**Tech Stack:** Rust 1.82, `tokio` (mpsc/Notify), `async-trait`, the existing `tui` crate (iocraft), `traits::task_registry`.

---

## Scope boundary (deviation from spec §3-M9-01, recorded deliberately)

The M9 design's §3 listed two integration items under M9-01: "`MultiAgentEvent` wired into the existing render-loop drain" and "desktop constructs real `TaskRegistry` → `task_registry: Some(..)`". This plan **relocates both to M9-05**, where the live background-bash render is the success gate (design §1 success-criterion 2). Rationale:

1. **R4 (don't creep into engine work):** wiring a live `TaskRegistry` means constructing `PosixRuntime` + `TaskOutputManager` + registering the `local_bash` handler in the desktop root — engine-adjacent work that is only *exercised* once task rows render (M9-04/05). Doing it in M9-01 would ship a registry nothing reads.
2. **No fake data ships:** the real app mounts no feed until M9-05; `state.multiagent` stays empty and nothing renders it (renderers arrive M9-03+). The `FixtureFeed` is test-only.
3. **Matches the codebase's existing seam split:** `apply_event` (mutator) is unit-tested in `streaming.rs`; the `root.rs` `use_future` pump that drives it is integration-tested. M9-01 mirrors that — it ships and unit-tests the `apply_multiagent_event` mutator + the feeds; M9-05 adds the `root.rs` pump (a new `use_future` modeled on the existing bridge pump at `tui/src/root.rs:791-812`) + the desktop feed.

The "single mutation seam" invariant (design §2.3) is honored: every multi-agent state change funnels through the one `apply_multiagent_event` function under the shared `state` mutex, exactly as `apply_event` is the sole `TurnEvent` seam. No renderer subscribes to the engine directly.

**All commands run from `lingxi-code/`** (toolchain pins Rust 1.82.0; running from repo root uses the host toolchain).

---

## File Structure

**Create (all under `lingxi-code/tui/src/multiagent/`):**
- `mod.rs` — module root + re-exports. One responsibility: assemble the submodules.
- `state.rs` — `MultiAgentState`, `TaskRow`, `WorkerRow`. The pure presentation model.
- `event.rs` — `MultiAgentEvent`. The single output type both feeds produce.
- `apply.rs` — `apply_multiagent_event`. The single mutation seam.
- `adapter.rs` — `MultiAgentFeed` trait + `pump_once` producer helper.
- `fixture.rs` — `FixtureFeed` (deterministic scripted feed for tests).
- `poller.rs` — `PollerFeed` over `Arc<dyn TaskRegistryHandle>` (the live task path).

**Modify:**
- `lingxi-code/tui/src/lib.rs` — add `pub mod multiagent;`.
- `lingxi-code/tui/src/state.rs` — add `multiagent: MultiAgentState` field to `AppState` + init in `new`.

**No other files change in M9-01.** (`root.rs`, the desktop apps, and `tui/Cargo.toml` are untouched — the poller consumes the `traits::task_registry::TaskRegistryHandle` trait, which `tui` already reaches transitively via its existing `traits` dependency.)

---

### Task 1: `MultiAgentState` model + `AppState` field

**Files:**
- Create: `lingxi-code/tui/src/multiagent/mod.rs`
- Create: `lingxi-code/tui/src/multiagent/state.rs`
- Modify: `lingxi-code/tui/src/lib.rs` (add module declaration)
- Modify: `lingxi-code/tui/src/state.rs` (add field + init)

- [ ] **Step 1: Create the module file with the state types and a failing test**

Create `lingxi-code/tui/src/multiagent/state.rs`:

```rust
//! Multi-agent presentation model. (M9-01)
//!
//! Pure data held on `AppState`, mutated only by
//! [`crate::multiagent::apply::apply_multiagent_event`]. Renderers (M9-03+)
//! are pure functions of this state.

/// One background task as surfaced to the TUI. Mirrors the field shape of
/// `traits::task_registry::TaskRecord` (the live task path) so the poller
/// maps one-to-one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskRow {
    /// 9-char `[bartwmd][0-9a-z]{8}` task id.
    pub task_id: String,
    /// Task type wire string (e.g. `"local_bash"`).
    pub task_type: String,
    /// Status wire string (e.g. `"running"`).
    pub status: String,
    /// Human-readable description.
    pub description: String,
}

/// One teammate/worker row. Populated from the coordinator surface in M9-06;
/// in M9-01 it is filled only by fixtures/tests (no coordinator dependency).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerRow {
    /// Worker agent id (stringified).
    pub agent_id: String,
    /// Display name.
    pub name: String,
    /// Agent-type string (e.g. `"explorer"`).
    pub agent_type: String,
    /// Simplified status label.
    pub status: String,
}

/// Aggregate multi-agent presentation state owned by `AppState`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MultiAgentState {
    /// Background tasks, newest-first as the feed orders them.
    pub tasks: Vec<TaskRow>,
    /// Teammate/worker roster.
    pub workers: Vec<WorkerRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let s = MultiAgentState::default();
        assert!(s.tasks.is_empty());
        assert!(s.workers.is_empty());
    }
}
```

Create `lingxi-code/tui/src/multiagent/mod.rs`:

```rust
//! Multi-agent TUI surface — presentation model, mutation seam, and the
//! swappable feed adapter. (M9)

pub mod state;

pub use state::{MultiAgentState, TaskRow, WorkerRow};
```

- [ ] **Step 2: Declare the module and run the test to verify it fails to compile (module not yet wired)**

Add to `lingxi-code/tui/src/lib.rs` (alongside the other `pub mod` lines):

```rust
pub mod multiagent;
```

Run: `cd lingxi-code && cargo test -p tui multiagent::state`
Expected: PASS (the `default_is_empty` test). If the module wasn't declared, compilation would fail with "file not found for module `multiagent`".

- [ ] **Step 3: Add the `multiagent` field to `AppState`**

In `lingxi-code/tui/src/state.rs`, add the field to the `AppState` struct (after the `message_selector` field, before the closing brace at line ~589):

```rust
    /// (M9-01) Multi-agent presentation state (tasks + workers). Mutated only
    /// by `multiagent::apply::apply_multiagent_event`. Renderers (M9-03+) read
    /// it; empty until a feed is mounted (M9-05).
    pub multiagent: crate::multiagent::MultiAgentState,
```

And initialize it in `AppState::new` (after the `message_selector: ...default()` line at ~636):

```rust
            multiagent: crate::multiagent::MultiAgentState::default(),
```

- [ ] **Step 4: Write a test that `AppState` carries an empty `MultiAgentState`**

Add to the `tests` module in `lingxi-code/tui/src/state.rs`:

```rust
    #[test]
    fn app_state_carries_empty_multiagent_state() {
        let s = AppState::default_for_tests();
        assert!(s.multiagent.tasks.is_empty());
        assert!(s.multiagent.workers.is_empty());
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd lingxi-code && cargo test -p tui multiagent && cargo test -p tui app_state_carries_empty_multiagent_state`
Expected: PASS (both tests).

- [ ] **Step 6: Commit**

```bash
cd lingxi-code
git add tui/src/multiagent/mod.rs tui/src/multiagent/state.rs tui/src/lib.rs tui/src/state.rs
git commit -m "feat(M9-01): MultiAgentState presentation model + AppState field"
```

---

### Task 2: `MultiAgentEvent` enum

**Files:**
- Create: `lingxi-code/tui/src/multiagent/event.rs`
- Modify: `lingxi-code/tui/src/multiagent/mod.rs`

- [ ] **Step 1: Write the event enum with a failing test**

Create `lingxi-code/tui/src/multiagent/event.rs`:

```rust
//! `MultiAgentEvent` — the single output type both feeds produce and the
//! single input `apply_multiagent_event` consumes. (M9-01)

use crate::multiagent::state::{TaskRow, WorkerRow};

/// One multi-agent state update. Feeds emit full-snapshot refresh events
/// (deterministic + idempotent); the mutator replaces the corresponding
/// `MultiAgentState` slice wholesale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiAgentEvent {
    /// Replace the task list with this snapshot.
    TasksRefreshed(Vec<TaskRow>),
    /// Replace the worker roster with this snapshot.
    WorkersRefreshed(Vec<WorkerRow>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_construct_and_compare() {
        let a = MultiAgentEvent::TasksRefreshed(vec![TaskRow {
            task_id: "b12345678".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "build".into(),
        }]);
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(a, MultiAgentEvent::WorkersRefreshed(vec![]));
    }
}
```

- [ ] **Step 2: Add the module declaration and re-export**

In `lingxi-code/tui/src/multiagent/mod.rs`, add:

```rust
pub mod event;

pub use event::MultiAgentEvent;
```

(Place `pub mod event;` after `pub mod state;`, and the `pub use` after the existing state re-export.)

- [ ] **Step 3: Run the test to verify it passes**

Run: `cd lingxi-code && cargo test -p tui multiagent::event`
Expected: PASS (`variants_construct_and_compare`).

- [ ] **Step 4: Commit**

```bash
cd lingxi-code
git add tui/src/multiagent/event.rs tui/src/multiagent/mod.rs
git commit -m "feat(M9-01): MultiAgentEvent (the single feed output type)"
```

---

### Task 3: `apply_multiagent_event` mutator

**Files:**
- Create: `lingxi-code/tui/src/multiagent/apply.rs`
- Modify: `lingxi-code/tui/src/multiagent/mod.rs`

- [ ] **Step 1: Write the mutator with failing tests**

Create `lingxi-code/tui/src/multiagent/apply.rs`:

```rust
//! `apply_multiagent_event` — the single mutation seam for multi-agent state.
//! Mirrors `crate::streaming::apply_event`: a pure mutator whose only side
//! effects are `state` mutation and `notify.notify_one()`. (M9-01)

use crate::multiagent::event::MultiAgentEvent;
use crate::state::AppState;
use tokio::sync::Notify;

/// Apply one [`MultiAgentEvent`] to `state` and signal the renderer.
///
/// - `TasksRefreshed(v)` → replace `state.multiagent.tasks` with `v`.
/// - `WorkersRefreshed(v)` → replace `state.multiagent.workers` with `v`.
///
/// After mutation, calls `notify.notify_one()` (the render loop debounces).
pub fn apply_multiagent_event(state: &mut AppState, ev: MultiAgentEvent, notify: &Notify) {
    match ev {
        MultiAgentEvent::TasksRefreshed(tasks) => state.multiagent.tasks = tasks,
        MultiAgentEvent::WorkersRefreshed(workers) => state.multiagent.workers = workers,
    }
    notify.notify_one();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::state::TaskRow;
    use crate::state::AppState;

    fn row(id: &str) -> TaskRow {
        TaskRow {
            task_id: id.into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "x".into(),
        }
    }

    #[test]
    fn tasks_refreshed_replaces_the_slice() {
        let mut s = AppState::default_for_tests();
        let n = Notify::new();
        apply_multiagent_event(
            &mut s,
            MultiAgentEvent::TasksRefreshed(vec![row("b11111111"), row("b22222222")]),
            &n,
        );
        assert_eq!(s.multiagent.tasks.len(), 2);
        // A second refresh REPLACES (not appends).
        apply_multiagent_event(
            &mut s,
            MultiAgentEvent::TasksRefreshed(vec![row("b33333333")]),
            &n,
        );
        assert_eq!(s.multiagent.tasks.len(), 1);
        assert_eq!(s.multiagent.tasks[0].task_id, "b33333333");
    }

    #[tokio::test]
    async fn apply_calls_notify_one() {
        let mut s = AppState::default_for_tests();
        let n = Notify::new();
        let waiter = n.notified();
        tokio::pin!(waiter);
        apply_multiagent_event(&mut s, MultiAgentEvent::WorkersRefreshed(vec![]), &n);
        let poll = futures::poll!(waiter.as_mut());
        assert!(matches!(poll, std::task::Poll::Ready(())));
    }
}
```

- [ ] **Step 2: Add the module declaration and re-export**

In `lingxi-code/tui/src/multiagent/mod.rs`, add:

```rust
pub mod apply;

pub use apply::apply_multiagent_event;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cd lingxi-code && cargo test -p tui multiagent::apply`
Expected: PASS (`tasks_refreshed_replaces_the_slice`, `apply_calls_notify_one`).

Note: `futures::poll!` is already used this way in `tui/src/streaming.rs` tests, so the `futures` dev-dep is present.

- [ ] **Step 4: Commit**

```bash
cd lingxi-code
git add tui/src/multiagent/apply.rs tui/src/multiagent/mod.rs
git commit -m "feat(M9-01): apply_multiagent_event mutator (single mutation seam)"
```

---

### Task 4: `MultiAgentFeed` trait + `FixtureFeed`

**Files:**
- Create: `lingxi-code/tui/src/multiagent/adapter.rs`
- Create: `lingxi-code/tui/src/multiagent/fixture.rs`
- Modify: `lingxi-code/tui/src/multiagent/mod.rs`

- [ ] **Step 1: Write the trait with a failing doc-anchored test**

Create `lingxi-code/tui/src/multiagent/adapter.rs`:

```rust
//! `MultiAgentFeed` — the swappable source of [`MultiAgentEvent`]s. (M9-01)
//!
//! Two impls share one output type: [`crate::multiagent::fixture::FixtureFeed`]
//! (deterministic, tests) and [`crate::multiagent::poller::PollerFeed`] (live,
//! over `TaskRegistryHandle`). The single output type is the contract that lets
//! the UI be built against fixtures and light up against the real engine with
//! no UI changes.

use crate::multiagent::event::MultiAgentEvent;
use async_trait::async_trait;

/// A source of multi-agent updates. One `poll()` returns the events for one
/// tick (a poller read, or one scripted fixture step).
#[async_trait]
pub trait MultiAgentFeed: Send + Sync {
    /// Produce the events for the next tick. An empty `Vec` means "no change".
    async fn poll(&self) -> Vec<MultiAgentEvent>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn MultiAgentFeed>> = None;
    }
}
```

- [ ] **Step 2: Write the `FixtureFeed` with failing tests**

Create `lingxi-code/tui/src/multiagent/fixture.rs`:

```rust
//! `FixtureFeed` — a deterministic, scripted [`MultiAgentFeed`] for tests and
//! for any UI surface whose engine source is not yet live. (M9-01)

use crate::multiagent::adapter::MultiAgentFeed;
use crate::multiagent::event::MultiAgentEvent;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Replays a fixed script: each `poll()` pops one step (a `Vec` of events).
/// Once the script is exhausted, every further `poll()` returns empty.
pub struct FixtureFeed {
    steps: Mutex<VecDeque<Vec<MultiAgentEvent>>>,
}

impl FixtureFeed {
    /// Build a feed from an ordered list of per-tick event batches.
    #[must_use]
    pub fn new(steps: Vec<Vec<MultiAgentEvent>>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
        }
    }
}

#[async_trait]
impl MultiAgentFeed for FixtureFeed {
    async fn poll(&self) -> Vec<MultiAgentEvent> {
        self.steps.lock().expect("fixture poisoned").pop_front().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::state::TaskRow;

    fn task_step(n: usize) -> Vec<MultiAgentEvent> {
        vec![MultiAgentEvent::TasksRefreshed(
            (0..n)
                .map(|i| TaskRow {
                    task_id: format!("b{i:08}"),
                    task_type: "local_bash".into(),
                    status: "running".into(),
                    description: "x".into(),
                })
                .collect(),
        )]
    }

    #[tokio::test]
    async fn replays_steps_in_order_then_empties() {
        let feed = FixtureFeed::new(vec![task_step(1), task_step(2)]);
        // Step 1.
        match feed.poll().await.as_slice() {
            [MultiAgentEvent::TasksRefreshed(v)] => assert_eq!(v.len(), 1),
            other => panic!("unexpected: {other:?}"),
        }
        // Step 2.
        match feed.poll().await.as_slice() {
            [MultiAgentEvent::TasksRefreshed(v)] => assert_eq!(v.len(), 2),
            other => panic!("unexpected: {other:?}"),
        }
        // Exhausted → empty forever.
        assert!(feed.poll().await.is_empty());
        assert!(feed.poll().await.is_empty());
    }
}
```

- [ ] **Step 3: Add module declarations and re-exports**

In `lingxi-code/tui/src/multiagent/mod.rs`, add:

```rust
pub mod adapter;
pub mod fixture;

pub use adapter::MultiAgentFeed;
pub use fixture::FixtureFeed;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd lingxi-code && cargo test -p tui multiagent::adapter && cargo test -p tui multiagent::fixture`
Expected: PASS (`trait_is_object_safe`, `replays_steps_in_order_then_empties`).

- [ ] **Step 5: Commit**

```bash
cd lingxi-code
git add tui/src/multiagent/adapter.rs tui/src/multiagent/fixture.rs tui/src/multiagent/mod.rs
git commit -m "feat(M9-01): MultiAgentFeed trait + deterministic FixtureFeed"
```

---

### Task 5: `PollerFeed` over `TaskRegistryHandle`

**Files:**
- Create: `lingxi-code/tui/src/multiagent/poller.rs`
- Modify: `lingxi-code/tui/src/multiagent/mod.rs`

- [ ] **Step 1: Write the poller and a stub-backed failing test**

Create `lingxi-code/tui/src/multiagent/poller.rs`:

```rust
//! `PollerFeed` — the live [`MultiAgentFeed`], reading the production task
//! registry through the narrow `traits::task_registry::TaskRegistryHandle`
//! trait (no dependency on the concrete `tasks` crate). (M9-01)

use crate::multiagent::adapter::MultiAgentFeed;
use crate::multiagent::event::MultiAgentEvent;
use crate::multiagent::state::TaskRow;
use async_trait::async_trait;
use std::sync::Arc;
use traits::task_registry::{TaskListFilter, TaskRecord, TaskRegistryHandle};

/// Maps a `TaskRecord` (the trait's wire shape) onto a `TaskRow` (the TUI's
/// presentation shape). Total — every `TaskRecord` field has a `TaskRow` home.
#[must_use]
pub fn task_row_from_record(r: TaskRecord) -> TaskRow {
    TaskRow {
        task_id: r.task_id,
        task_type: r.task_type,
        status: r.status,
        description: r.description,
    }
}

/// Live feed: each `poll()` lists the registry and emits one `TasksRefreshed`.
pub struct PollerFeed {
    tasks: Arc<dyn TaskRegistryHandle>,
}

impl PollerFeed {
    /// Wrap a task-registry handle.
    #[must_use]
    pub fn new(tasks: Arc<dyn TaskRegistryHandle>) -> Self {
        Self { tasks }
    }
}

#[async_trait]
impl MultiAgentFeed for PollerFeed {
    async fn poll(&self) -> Vec<MultiAgentEvent> {
        // A failed list is surfaced as "no change" (empty) rather than a panic;
        // the registry error path is owned by the tools, not the read-only UI.
        let rows = self
            .tasks
            .list(TaskListFilter::default())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(task_row_from_record)
            .collect::<Vec<_>>();
        vec![MultiAgentEvent::TasksRefreshed(rows)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use traits::task_registry::{TaskCreateInput, TaskOutputChunk, TaskRegistryError, TaskUpdatePatch};

    /// Minimal stand-in `TaskRegistryHandle`: `list` returns a canned set; the
    /// other methods are unused by `PollerFeed::poll` and return trivially.
    struct StubTasks {
        rows: Vec<TaskRecord>,
    }

    #[async_trait]
    impl TaskRegistryHandle for StubTasks {
        async fn create(&self, _: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn get(&self, _: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }
        async fn list(&self, _: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(self.rows.clone())
        }
        async fn update(&self, _: &str, _: TaskUpdatePatch) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn set_status(&self, _: &str, _: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn kill(&self, _: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn output(&self, _: &str, _: Option<u64>) -> Result<TaskOutputChunk, TaskRegistryError> {
            Ok(TaskOutputChunk::default())
        }
    }

    #[tokio::test]
    async fn poll_maps_registry_records_to_task_rows() {
        let stub = Arc::new(StubTasks {
            rows: vec![
                TaskRecord {
                    task_id: "b00000001".into(),
                    task_type: "local_bash".into(),
                    status: "running".into(),
                    description: "cargo build".into(),
                },
                TaskRecord {
                    task_id: "a00000002".into(),
                    task_type: "local_agent".into(),
                    status: "completed".into(),
                    description: "explore".into(),
                },
            ],
        });
        let feed = PollerFeed::new(stub);
        match feed.poll().await.as_slice() {
            [MultiAgentEvent::TasksRefreshed(rows)] => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].task_id, "b00000001");
                assert_eq!(rows[0].task_type, "local_bash");
                assert_eq!(rows[1].status, "completed");
                assert_eq!(rows[1].description, "explore");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Add the module declaration and re-export**

In `lingxi-code/tui/src/multiagent/mod.rs`, add:

```rust
pub mod poller;

pub use poller::{task_row_from_record, PollerFeed};
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cd lingxi-code && cargo test -p tui multiagent::poller`
Expected: PASS (`poll_maps_registry_records_to_task_rows`).

- [ ] **Step 4: Commit**

```bash
cd lingxi-code
git add tui/src/multiagent/poller.rs tui/src/multiagent/mod.rs
git commit -m "feat(M9-01): PollerFeed over TaskRegistryHandle + record→row mapping"
```

---

### Task 6: Contract test — poller output shape == fixture schema

**Files:**
- Create: `lingxi-code/tui/tests/multiagent_contract_test.rs`

This is design §4 R1's mitigation: a test that proves the live poller's `TaskRow`s are structurally identical to what a fixture produces, so UI built against the fixture is correct against the real feed.

- [ ] **Step 1: Write the contract test**

Create `lingxi-code/tui/tests/multiagent_contract_test.rs`:

```rust
//! M9-01 contract test (design §4 R1): the live `PollerFeed` and the
//! `FixtureFeed` emit the SAME `MultiAgentEvent`/`TaskRow` shape, so the UI
//! built against fixtures lights up correctly against the real engine.

use tui::multiagent::{FixtureFeed, MultiAgentEvent, MultiAgentFeed, TaskRow};
use tui::multiagent::poller::task_row_from_record;
use traits::task_registry::TaskRecord;

/// The record→row mapping is TOTAL: every `TaskRecord` field lands on the
/// corresponding `TaskRow` field (no data dropped, no field invented).
#[test]
fn record_to_row_mapping_is_total() {
    let rec = TaskRecord {
        task_id: "b12345678".into(),
        task_type: "local_bash".into(),
        status: "running".into(),
        description: "build the workspace".into(),
    };
    let row = task_row_from_record(rec.clone());
    assert_eq!(row.task_id, rec.task_id);
    assert_eq!(row.task_type, rec.task_type);
    assert_eq!(row.status, rec.status);
    assert_eq!(row.description, rec.description);
}

/// A fixture can reproduce, byte-for-byte, the row a poller would emit from a
/// given record — so a snapshot taken against the fixture is valid for live data.
#[tokio::test]
async fn fixture_can_reproduce_a_poller_row() {
    let rec = TaskRecord {
        task_id: "a99999999".into(),
        task_type: "local_agent".into(),
        status: "completed".into(),
        description: "review".into(),
    };
    let poller_row: TaskRow = task_row_from_record(rec.clone());

    let fixture = FixtureFeed::new(vec![vec![MultiAgentEvent::TasksRefreshed(vec![TaskRow {
        task_id: "a99999999".into(),
        task_type: "local_agent".into(),
        status: "completed".into(),
        description: "review".into(),
    }])]]);

    match fixture.poll().await.as_slice() {
        [MultiAgentEvent::TasksRefreshed(rows)] => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0], poller_row, "fixture row must equal the poller row");
        }
        other => panic!("unexpected: {other:?}"),
    }
}
```

- [ ] **Step 2: Run the contract test to verify it passes**

Run: `cd lingxi-code && cargo test -p tui --test multiagent_contract_test`
Expected: PASS (`record_to_row_mapping_is_total`, `fixture_can_reproduce_a_poller_row`).

Note: this requires `tui::multiagent::poller` to be public (it is — `pub mod poller;` from Task 5) and `traits` to be a dev-dep of `tui`. If `cargo test` reports `unresolved import traits`, add `traits = { path = "../traits" }` under `[dev-dependencies]` in `lingxi-code/tui/Cargo.toml` (it is already a regular dependency, so this is normally unnecessary — regular deps are visible to integration tests).

- [ ] **Step 3: Commit**

```bash
cd lingxi-code
git add tui/tests/multiagent_contract_test.rs
git commit -m "test(M9-01): contract test — poller row shape == fixture schema"
```

---

### Task 7: `pump_once` producer helper

**Files:**
- Modify: `lingxi-code/tui/src/multiagent/adapter.rs`
- Modify: `lingxi-code/tui/src/multiagent/mod.rs`

`pump_once` is the unit-testable core of the tick loop that M9-05 will wrap in a `root.rs` `use_future`. It polls a feed once and forwards every event to a channel.

- [ ] **Step 1: Write `pump_once` with a failing test**

Append to `lingxi-code/tui/src/multiagent/adapter.rs` (after the trait definition, before the `#[cfg(test)]` block):

```rust
use tokio::sync::mpsc::UnboundedSender;

/// Poll `feed` once and forward every produced event to `tx`. Returns the
/// number of events sent. The M9-05 `root.rs` pump calls this on a tick; here
/// it is a standalone, fully-testable unit. A closed channel is treated as a
/// no-op (events are dropped) — the caller owns shutdown.
pub async fn pump_once(feed: &dyn MultiAgentFeed, tx: &UnboundedSender<MultiAgentEvent>) -> usize {
    let events = feed.poll().await;
    let mut sent = 0;
    for ev in events {
        if tx.send(ev).is_ok() {
            sent += 1;
        }
    }
    sent
}
```

Add this test inside the existing `#[cfg(test)] mod tests` block in `adapter.rs`:

```rust
    use crate::multiagent::fixture::FixtureFeed;
    use crate::multiagent::state::TaskRow;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn pump_once_forwards_fixture_events_to_channel() {
        let feed = FixtureFeed::new(vec![vec![MultiAgentEvent::TasksRefreshed(vec![TaskRow {
            task_id: "b00000001".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "x".into(),
        }])]]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sent = pump_once(&feed, &tx).await;
        assert_eq!(sent, 1);
        match rx.recv().await.unwrap() {
            MultiAgentEvent::TasksRefreshed(rows) => assert_eq!(rows.len(), 1),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pump_once_on_exhausted_feed_sends_nothing() {
        let feed = FixtureFeed::new(vec![]); // empty script
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sent = pump_once(&feed, &tx).await;
        assert_eq!(sent, 0);
        assert!(rx.try_recv().is_err());
    }
```

- [ ] **Step 2: Re-export `pump_once`**

In `lingxi-code/tui/src/multiagent/mod.rs`, update the adapter re-export line to:

```rust
pub use adapter::{pump_once, MultiAgentFeed};
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cd lingxi-code && cargo test -p tui multiagent::adapter`
Expected: PASS (`trait_is_object_safe`, `pump_once_forwards_fixture_events_to_channel`, `pump_once_on_exhausted_feed_sends_nothing`).

- [ ] **Step 4: Commit**

```bash
cd lingxi-code
git add tui/src/multiagent/adapter.rs tui/src/multiagent/mod.rs
git commit -m "feat(M9-01): pump_once producer (unit-testable core of the M9-05 tick pump)"
```

---

### Task 8: Workspace gate + tag `m9.1`

**Files:** none (verification + tag only)

- [ ] **Step 1: Format check**

Run: `cd lingxi-code && cargo fmt --check`
Expected: no output (clean). If it reports diffs, run `cargo fmt` and re-stage/commit with `style(M9-01): cargo fmt`.

- [ ] **Step 2: Clippy (all targets, deny warnings)**

Run: `cd lingxi-code && cargo clippy -p tui --all-targets -- -D warnings`
Expected: `Finished` with no warnings. (The new module forbids `unsafe` via the crate-level `#![forbid(unsafe_code)]` already in `tui/src/lib.rs`; `missing_docs` is satisfied — every `pub` item above has a doc comment.)

- [ ] **Step 3: Full crate test run**

Run: `cd lingxi-code && cargo test -p tui`
Expected: PASS, including the M7 suites (no regressions) and the new `multiagent::*` + `multiagent_contract_test` tests.

- [ ] **Step 4: Workspace build (catch any consumer breakage)**

Run: `cd lingxi-code && cargo build --workspace --all-targets --offline`
Expected: `Finished`. (M9-01 adds only an `AppState` field with a `Default` initializer, so no consumer construction sites break.)

- [ ] **Step 5: Annotated tag**

```bash
cd lingxi-code
git tag -a m9.1 -m "M9-01: multi-agent presentation foundation (state + event + mutator + feed adapter)"
git tag --list m9.1
```

Expected: `m9.1` listed. **Do not push** (design §6.4 — no remote push from Claude).

---

## Self-Review

**1. Spec coverage (against design §3-M9-01 deliverables):**
- `MultiAgentEvent` variants → Task 2. ✓
- `MultiAgentState` in `AppState` → Task 1. ✓
- adapter trait + fixture impl + real poller impl → Tasks 4, 5. ✓
- contract test (real-poller shape == fixture schema) → Task 6. ✓
- `apply_event` wiring (mutation seam) → Task 3 (`apply_multiagent_event`). ✓
- "`MultiAgentEvent` wired into the existing render-loop drain" → **relocated to M9-05** (recorded under "Scope boundary"; `pump_once` in Task 7 is the unit-testable core that the M9-05 `root.rs` pump wraps). ✓ (deliberate, documented)
- "desktop constructs real `TaskRegistry` → `task_registry: Some(..)`" → **relocated to M9-05** (recorded under "Scope boundary"; `PosixRuntime` impls `RuntimeSpawner` and the `local_bash` handler exists, so the relocation is purely about *where* it's exercised). ✓ (deliberate, documented)
- DAG check for a `tui→coordinator` edge → **N/A in M9-01** — the worker/mailbox path is fixture-only here (Task 1 `WorkerRow` note); the real coordinator drain + its DAG evaluation lands in M9-06. ✓

**2. Placeholder scan:** No "TBD"/"TODO"/"handle edge cases"/"similar to". Every code step shows complete, compiling code. The two relocations are explicit decisions with rationale, not deferred-detail placeholders. ✓

**3. Type consistency:**
- `TaskRow { task_id, task_type, status, description }` — identical fields in `state.rs` (Task 1), the poller mapping (Task 5), and the contract test (Task 6). ✓
- `MultiAgentEvent::{TasksRefreshed(Vec<TaskRow>), WorkersRefreshed(Vec<WorkerRow>)}` — same in `event.rs` (Task 2), `apply.rs` (Task 3), `fixture.rs` (Task 4), `poller.rs` (Task 5), `adapter.rs` (Task 7). ✓
- `MultiAgentFeed::poll(&self) -> Vec<MultiAgentEvent>` — same signature in the trait (Task 4) and both impls (Tasks 4, 5). ✓
- `apply_multiagent_event(&mut AppState, MultiAgentEvent, &Notify)` — mirrors `streaming::apply_event`'s real signature (verified against `tui/src/streaming.rs`). ✓
- `task_row_from_record(TaskRecord) -> TaskRow` — defined Task 5, used Tasks 5 & 6. ✓
- `traits::task_registry::TaskRegistryHandle` method set in the Task 5 stub (`create/get/list/update/set_status/kill/output`) matches the real trait (verified against `traits/src/task_registry.rs`). ✓

No gaps requiring new tasks. Plan is internally consistent and fully grounded in the current tree.

---

**End of plan.**
