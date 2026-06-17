# TUI Permission Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the existing `TuiPermissionGate` into the terminal TUI so an otherwise-unresolved mutating-tool `Ask` surfaces the already-built `ToolUseConfirm` dialog (Allow / Deny / Allow-Always), with a FIFO queue so concurrent streaming checks never lose a prompt.

**Architecture:** Connect two existing ends. `build_runtime_for_tui` creates an `mpsc<PermissionExchange>` channel + a `TuiPermissionGate`, injects the gate via `DesktopConfig.injected_permission_gate` (which `engine_desktop::build` already honors), and hands the receiver to the TUI. A new pump in `tui/src/root.rs` drains the receiver into a FIFO `AppState.permission_queue` and promotes one exchange at a time into the single active dialog slot; the existing `resolve_pending_permission` fires the `oneshot` back to the blocked `gate.check()` and promotes the next. No new dialog UI, no engine-core change.

**Tech Stack:** Rust workspace at `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code`. Crates: `tui` (state/root/session/streaming/keymap/permission_bridge), `cli` (init/mode/run — package name `cli`, binary `lingxi-cli`), `permission` (`PolicyPermissionGate`/`PermissionPolicy`), `engine-desktop` (`build`). iocraft TUI; tokio mpsc/oneshot.

**Design doc:** `docs/superpowers/specs/2026-06-17-tui-permission-prompt-design.md`

---

## Conventions for every task

- **Branch:** Work on `tui-permission-prompt`. Before editing, run `git -C /Users/luolingfeng/Projects/LingXi-Next rev-parse --abbrev-ref HEAD` and confirm it prints `tui-permission-prompt`. If not (harness placed you elsewhere / detached / worktree), `git -C /Users/luolingfeng/Projects/LingXi-Next checkout tui-permission-prompt`. Reconcile the tip with the previous task's commit (`git log --oneline -3`) before starting.
- **Build prefix:** Disk is near-full. Prefix EVERY cargo command with `CARGO_PROFILE_DEV_DEBUG=0` (and `CARGO_PROFILE_TEST_DEBUG=0` for `cargo test`).
- **Staging:** Stage ONLY the exact files named in each task with `git add <path> <path>`. NEVER `git add -A` / `git add .` — the working tree has unrelated untracked files (`session/*`, `.codegraph/`, `codex/`, `docs/parity-*`, `clients/*` WIP) that must never be swept in.
- **Never touch** any `session/*` working-tree file.
- **Line numbers are approximate** — they drift as edits land. Re-grep for the exact site before editing; match on the symbol/expression, not the line number.

---

## Task 1: `AppState` FIFO queue + shared dialog helpers

Add the FIFO queue and two helpers that BOTH the new pump and the refactored legacy branch will use. The helper sets `pending_permission_started_at` (fixing the `elapsed_ms = 0` telemetry gap).

**Files:**
- Modify: `tui/src/state.rs` (struct field `:534-660`, `impl AppState` ctor `:896`)
- Test: inline `#[cfg(test)] mod tests` in `tui/src/state.rs`

- [ ] **Step 1: Add the `permission_queue` field to `AppState`**

In `tui/src/state.rs`, near the `pending_permission` cluster (around `:540-654`), add a field:

```rust
    /// (TUI-PERM) FIFO of permission exchanges received from the
    /// `TuiPermissionGate` that have NOT yet been promoted into the single
    /// active dialog slot. The pump pushes here; `promote_next_permission`
    /// pops the front when `pending_permission` is free. Guarantees a second
    /// concurrent `gate.check()` never overwrites the active dialog's
    /// `resp_tx` (streaming dispatches tools concurrently).
    pub permission_queue: std::collections::VecDeque<crate::permission_bridge::PermissionExchange>,
```

- [ ] **Step 2: Initialize the field in every `AppState` constructor**

In `impl AppState`'s `new(status: StatusSnapshot)` (around `:896`), add to the struct literal:

```rust
            permission_queue: std::collections::VecDeque::new(),
```

Then `grep -n "AppState {" tui/src/state.rs` and add the same initializer to any OTHER place that constructs `AppState { ... }` with explicit fields (if the codebase uses `..Default::default()` or a single `new`, this is the only site). Build will tell you if one is missed.

- [ ] **Step 3: Write the failing tests for the helpers**

Add to the `#[cfg(test)] mod tests` block in `tui/src/state.rs` (re-use whatever imports the module already has; add `use tokio::sync::oneshot;` and `use crate::permission_bridge::PermissionExchange;` if absent):

```rust
    #[test]
    fn open_permission_dialog_sets_started_at_and_active_slot() {
        let mut st = AppState::new(StatusSnapshot::default());
        let (tx, _rx) = oneshot::channel();
        let req = permission::gate::PermissionRequest::ToolUseConfirm {
            tool_name: "Write".to_string(),
            tool_input: serde_json::json!({"file_path": "a.txt"}),
            default_decision: permission::tool_default("Write"),
        };
        open_permission_dialog(&mut st, req, Some(tx));
        assert!(st.pending_permission.is_some());
        assert!(st.pending_permission_resp_tx.is_some());
        assert!(st.pending_permission_started_at.is_some(), "started_at must be set for telemetry");
    }

    #[test]
    fn promote_next_permission_is_fifo_and_respects_active_slot() {
        let mut st = AppState::new(StatusSnapshot::default());
        let mk = |tool: &str| {
            let (tx, _rx) = oneshot::channel();
            PermissionExchange {
                request: permission::gate::PermissionRequest::ToolUseConfirm {
                    tool_name: tool.to_string(),
                    tool_input: serde_json::json!({}),
                    default_decision: permission::tool_default(tool),
                },
                resp_tx: tx,
            }
        };
        st.permission_queue.push_back(mk("Write"));
        st.permission_queue.push_back(mk("Edit"));

        // First promote opens "Write".
        promote_next_permission(&mut st);
        assert_eq!(st.pending_permission.as_ref().unwrap().tool(), "Write");
        assert_eq!(st.permission_queue.len(), 1);

        // A second promote is a NO-OP while a dialog is active.
        promote_next_permission(&mut st);
        assert_eq!(st.pending_permission.as_ref().unwrap().tool(), "Write");
        assert_eq!(st.permission_queue.len(), 1);

        // Clear the active slot, then promote opens "Edit".
        st.pending_permission = None;
        promote_next_permission(&mut st);
        assert_eq!(st.pending_permission.as_ref().unwrap().tool(), "Edit");
        assert!(st.permission_queue.is_empty());
    }
```

- [ ] **Step 4: Run the tests to confirm they fail to compile (helpers undefined)**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui open_permission_dialog promote_next_permission 2>&1 | tail -15`
Expected: compile error — `open_permission_dialog` / `promote_next_permission` not found.

- [ ] **Step 5: Implement the two helpers**

Add to `tui/src/state.rs` (free functions in the module, near `PendingPermission`; if the module prefers `impl AppState` methods, match that — but free fns taking `&mut AppState` mirror the existing `apply_*` seam style). Use the EXACT field names already on `AppState` (`pending_permission`, `pending_permission_resp_tx`, `pending_permission_started_at`, `tool_use_dialog_state`):

```rust
/// (TUI-PERM) Open the permission dialog for `request`, attaching `resp_tx`
/// (the oneshot back to the orchestrator's `TuiPermissionGate`; `None` for the
/// legacy bridge variant which resolves elsewhere). Sets `started_at` so the
/// resolved-telemetry `elapsed_ms` is non-zero, and resets the per-dialog
/// `ToolUseConfirm` state. Fires the `permission_dialog_shown` event.
pub fn open_permission_dialog(
    st: &mut AppState,
    request: PermissionRequest,
    resp_tx: Option<oneshot::Sender<PermissionResponse>>,
) {
    st.pending_permission = Some(PendingPermission { request, worker: None });
    st.pending_permission_resp_tx = resp_tx;
    st.pending_permission_started_at = Some(std::time::Instant::now());
    st.tool_use_dialog_state =
        crate::components::permissions::tool_use_confirm::ToolUseConfirmState::default();
    crate::telemetry::permission_dialog_shown("tool_use");
}

/// (TUI-PERM) If no dialog is active and the FIFO queue is non-empty, pop the
/// front exchange and open it. No-op when a dialog is already active (one
/// active dialog at a time) or the queue is empty.
pub fn promote_next_permission(st: &mut AppState) {
    if st.pending_permission.is_some() {
        return;
    }
    if let Some(exchange) = st.permission_queue.pop_front() {
        open_permission_dialog(st, exchange.request, Some(exchange.resp_tx));
    }
}
```

If `PermissionRequest` / `PermissionResponse` / `PendingPermission` / `oneshot` are not already imported at module scope, add `use permission::gate::{PermissionRequest, PermissionResponse};` and `use tokio::sync::oneshot;` (the module already imports `PermissionResponse` at `:22` and `oneshot` at `:25` — reuse those).

- [ ] **Step 6: Run the tests to confirm they pass**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui open_permission_dialog promote_next_permission 2>&1 | tail -15`
Expected: both pass.

- [ ] **Step 7: Build the crate clean**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p tui 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add tui/src/state.rs
git commit -m "feat(tui): AppState permission FIFO queue + shared dialog helpers

open_permission_dialog/promote_next_permission set started_at and enforce one
active dialog with a FIFO queue, so concurrent gate.check()s never lose a prompt.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Refactor the legacy `streaming.rs` branch to use the shared helper

DRY: the legacy `TurnEvent::PermissionRequest` branch already opens a dialog but duplicates the field-setting and (per the design) is the one that proves `started_at` is set consistently. Route it through `open_permission_dialog`.

**Files:**
- Modify: `tui/src/streaming.rs` (`TurnEvent::PermissionRequest` arm, `:74-92`)

- [ ] **Step 1: Read the current arm**

Run: `grep -n "TurnEvent::PermissionRequest" tui/src/streaming.rs` and read the ~18 lines of the arm. It currently sets `state.pending_permission`, `state.pending_permission_started_at`, `state.tool_use_dialog_state`, and calls `telemetry::permission_dialog_shown`.

- [ ] **Step 2: Replace the arm body with the helper call**

Replace the body of the `TurnEvent::PermissionRequest { tool, input } =>` arm with:

```rust
        TurnEvent::PermissionRequest { tool, input } => {
            // M6-03 bridge variant carries the legacy {tool, input} shape and
            // resolves over a separate channel, so NO resp_tx is attached here.
            // The richer in-process variant rides `TuiPermissionGate`'s
            // `mpsc<PermissionExchange>` (see `permission_bridge.rs` + the
            // root.rs permission pump). Shared helper keeps `started_at` + the
            // dialog-reset + telemetry identical across both paths.
            let default_decision = permission::tool_default(&tool);
            let request = permission::gate::PermissionRequest::ToolUseConfirm {
                tool_name: tool,
                tool_input: input,
                default_decision,
            };
            crate::state::open_permission_dialog(state, request, None);
        }
```

(`state` here is the `&mut AppState` the `apply_event` fn already holds — confirm the binding name by reading the fn signature; it is `state` in this module.)

- [ ] **Step 3: Build + run streaming tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui streaming 2>&1 | tail -15`
Expected: existing streaming tests pass (behavior unchanged; `started_at` now always set). If a test asserted the old inline behavior, it should still hold (same observable state).

- [ ] **Step 4: Commit**

```bash
git add tui/src/streaming.rs
git commit -m "refactor(tui): route legacy permission branch through open_permission_dialog

DRY with the new in-process path; guarantees started_at is set so resolved
telemetry reports a real elapsed_ms.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Promote the next queued exchange on dialog resolution

`resolve_pending_permission` is the single site that fires `resp_tx` and clears the active slot. Append a `promote_next_permission` call so a queued exchange opens immediately after one resolves.

**Files:**
- Modify: `tui/src/events/keymap.rs` (`resolve_pending_permission`, `:279-303`)
- Test: inline test in `tui/src/events/keymap.rs` (or extend an existing permission test there)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `tui/src/events/keymap.rs` (reuse existing imports; add `use crate::permission_bridge::PermissionExchange;` and `use tokio::sync::oneshot;` if absent). This drives the real resolution path and asserts the next queued exchange is promoted:

```rust
    #[test]
    fn resolving_a_dialog_promotes_the_next_queued_exchange() {
        let mut st = AppState::new(StatusSnapshot::default());
        // Active dialog for "Write" with a live oneshot.
        let (tx0, mut rx0) = oneshot::channel();
        crate::state::open_permission_dialog(
            &mut st,
            permission::gate::PermissionRequest::ToolUseConfirm {
                tool_name: "Write".to_string(),
                tool_input: serde_json::json!({}),
                default_decision: permission::tool_default("Write"),
            },
            Some(tx0),
        );
        // A second exchange waiting in the FIFO.
        let (tx1, _rx1) = oneshot::channel();
        st.permission_queue.push_back(PermissionExchange {
            request: permission::gate::PermissionRequest::ToolUseConfirm {
                tool_name: "Edit".to_string(),
                tool_input: serde_json::json!({}),
                default_decision: permission::tool_default("Edit"),
            },
            resp_tx: tx1,
        });

        // Resolve the active "Write" dialog with Deny.
        resolve_pending_permission(
            &mut st,
            DialogResolution { response: permission::gate::PermissionResponse::Deny },
        );

        // The first oneshot received the decision...
        assert_eq!(rx0.try_recv().unwrap(), permission::gate::PermissionResponse::Deny);
        // ...and the next queued exchange ("Edit") is now the active dialog.
        assert_eq!(st.pending_permission.as_ref().unwrap().tool(), "Edit");
        assert!(st.permission_queue.is_empty());
    }
```

NOTE: match the ACTUAL shape of `DialogResolution` (read its definition near `resolve_pending_permission`; the field may be named `response` of type `PermissionResponse` — adjust the literal to the real constructor). If `resolve_pending_permission` is private, the test is in the same module so it has access.

- [ ] **Step 2: Run to confirm it fails**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui resolving_a_dialog_promotes 2>&1 | tail -15`
Expected: FAIL — after resolution `pending_permission` is `None` (no promotion yet), so `.tool()` panics / assert fails.

- [ ] **Step 3: Append the promote call**

In `resolve_pending_permission` (`tui/src/events/keymap.rs:279`), AFTER the lines that fire `resp_tx` and set `state.pending_permission = None; state.pending_permission_started_at = None;` (around `:298-302`), add as the LAST statement of the function:

```rust
    // (TUI-PERM) Open the next queued exchange, if any, now that the active
    // slot is free. No-op when the queue is empty.
    crate::state::promote_next_permission(state);
```

- [ ] **Step 4: Run to confirm it passes**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui resolving_a_dialog_promotes 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tui/src/events/keymap.rs
git commit -m "feat(tui): promote next queued permission exchange on resolution

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `root.rs` permission pump + `RootProps` field + `PermissionRxSlot` alias

Add the third drain pump (mirroring the bridge pump `:2007` and multiagent pump `:2035`) that pushes each received `PermissionExchange` onto the FIFO queue and promotes.

**Files:**
- Modify: `tui/src/root.rs` (type aliases `:44-50`, `RootProps` `:53-94`, add a pump near `:2040`)

- [ ] **Step 1: Add the slot type alias**

In `tui/src/root.rs`, next to `BridgeRxSlot` (`:44`) and `MultiAgentRxSlot` (`:49`), add:

```rust
/// (TUI-PERM) Take-once slot for the `TuiPermissionGate` receiver, mirroring
/// [`BridgeRxSlot`]. The permission pump `take()`s it once on first render.
pub type PermissionRxSlot =
    Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<crate::permission_bridge::PermissionExchange>>>>;
```

- [ ] **Step 2: Add the `RootProps` field**

In `struct RootProps` (`:53`, derives `Default, Props`), add (Option defaults to `None`, so `Default` still holds):

```rust
    /// (TUI-PERM) Receiver for `TuiPermissionGate` exchanges. `None` for
    /// bridge-less mounts (resume picker / smoke gates) — the pump stays inert.
    pub permission_rx: Option<PermissionRxSlot>,
```

- [ ] **Step 3: Add the permission pump**

Immediately AFTER the multiagent pump block (the `use_future` that drains `props.multiagent_rx`, ending around `:2045`), add a third pump in the same idiom:

```rust
    // ---- Permission pump (TUI-PERM): drain PermissionExchange → FIFO --------
    // Mirrors the bridge/multiagent pumps. Each exchange is queued and the
    // front is promoted into the single active dialog when free, so concurrent
    // gate.check()s never overwrite an active dialog's resp_tx. Inert when
    // `permission_rx` is None (bridge-less mounts).
    {
        let state = state.clone();
        let rx_slot = props.permission_rx.clone();
        let mut tick_for_perm = tick;
        hooks.use_future(async move {
            let Some(slot) = rx_slot else {
                return;
            };
            let Some(mut rx) = slot.lock().expect("permission rx slot poisoned").take() else {
                return;
            };
            while let Some(exchange) = rx.recv().await {
                let mut st = state.lock().await;
                st.permission_queue.push_back(exchange);
                crate::state::promote_next_permission(&mut st);
                drop(st);
                tick_for_perm.set(tick_for_perm.get().wrapping_add(1));
            }
        });
    }
```

Confirm the surrounding bindings (`state`, `tick`, `hooks`) match the names used by the bridge/multiagent pumps in this same function — copy their exact form. (`tick` is a `State<...>` copy used via `.set(.get()...)`; reuse identically.)

- [ ] **Step 4: Build the crate clean**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p tui 2>&1 | tail -5`
Expected: clean. The pump is inert (no `permission_rx` source yet); no behavior change.

- [ ] **Step 5: Run the tui test suite (no regressions)**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui 2>&1 | tail -15`
Expected: pass / no new failures.

- [ ] **Step 6: Commit**

```bash
git add tui/src/root.rs
git commit -m "feat(tui): permission pump drains PermissionExchange into the FIFO queue

Third render-loop pump mirroring the bridge/multiagent pumps; promotes one
queued exchange at a time into the active dialog. Inert until a receiver is wired.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `session.rs` Runtime `permission_rx` + `with_permission_rx` + mount wiring

Thread the receiver from `Runtime` into the `TuiRoot` element via a take-once slot.

**Files:**
- Modify: `tui/src/session.rs` (`Runtime` struct `:42`, every `Runtime` constructor, a `with_permission_rx` builder, `mount`/`run_tui_session` `:355-388`)

- [ ] **Step 1: Add the `permission_rx` field to `Runtime`**

In `pub struct Runtime` (`tui/src/session.rs:42`), add (mirror `turn_tx`'s `Option` style):

```rust
    /// (TUI-PERM) Receiver for `TuiPermissionGate` exchanges, handed in by the
    /// CLI (`build_runtime_for_tui`). `None` (smoke gates / resume picker) keeps
    /// the permission pump inert. Moved into a take-once slot at mount.
    pub permission_rx:
        Option<tokio::sync::mpsc::Receiver<crate::permission_bridge::PermissionExchange>>,
```

- [ ] **Step 2: Initialize it in every constructor**

In `Runtime::new` (`:111`) and `Runtime::with_bridge` (`:135`) add `permission_rx: None,` to each struct literal. `grep -n "permission_rx\|turn_tx: None" tui/src/session.rs` after editing to confirm both constructors set it (anywhere `turn_tx: None` appears, add `permission_rx: None` alongside).

- [ ] **Step 3: Add the `with_permission_rx` builder**

Next to `with_turn_tx` (`:177`), add:

```rust
    /// (TUI-PERM) Attach the `TuiPermissionGate` receiver so the root's
    /// permission pump can drain it. Without it the interactive prompt never
    /// appears (the engine still auto-allows / denies per its gate selection).
    #[must_use]
    pub fn with_permission_rx(
        mut self,
        permission_rx: tokio::sync::mpsc::Receiver<crate::permission_bridge::PermissionExchange>,
    ) -> Self {
        self.permission_rx = Some(permission_rx);
        self
    }
```

- [ ] **Step 4: Build the take-once slot + pass it to `TuiRoot` in `mount`**

In the mount function (`run_tui_session`, around `:355-388`), after the `rx_slot` / multiagent slot construction, add:

```rust
    // (TUI-PERM) Move the permission receiver into a take-once slot for the
    // root's permission pump, mirroring `rx_slot`.
    let permission_rx_slot: Option<crate::root::PermissionRxSlot> = runtime
        .permission_rx
        .take()
        .map(|rx| Arc::new(std::sync::Mutex::new(Some(rx))));
```

Then in the `element! { TuiRoot( ... ) }` invocation (the one with `bridge_rx: Some(rx_slot)`, around `:376`), add the prop:

```rust
            permission_rx: permission_rx_slot,
```

NOTE: `runtime` must be `mut` for `.take()` — it already is (the existing `runtime.bridge.take()` at `:360` requires it). The resume-picker `TuiRoot` (`run_resume_picker`, around `:430`) does NOT get this prop, so it defaults to `None` — correct (the picker has no orchestrator/gate).

- [ ] **Step 5: Build clean**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p tui 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add tui/src/session.rs
git commit -m "feat(tui): thread permission receiver through Runtime into TuiRoot

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `init.rs` — single config resolve, gate construction, `TuiBuild.permission_rx`

Build the channel + `TuiPermissionGate`, inject it, and avoid the double config-resolution / lost-persist-paths trap by sharing one resolved `DesktopConfig`.

**Files:**
- Modify: `apps/cli/src/init.rs` (`TuiBuild` `:94`, `build_runtime` `:317`, `build_runtime_for_tui` `:353`, `resolve_desktop_config` comment `:280`)

- [ ] **Step 1: Add the `permission_rx` field to `TuiBuild`**

In `pub struct TuiBuild` (`apps/cli/src/init.rs:94`), add:

```rust
    /// (TUI-PERM) Receiver for the injected `TuiPermissionGate`'s exchanges.
    /// Threaded into `session::Runtime::with_permission_rx` so the TUI's
    /// permission pump drives the interactive dialog.
    pub permission_rx:
        tokio::sync::mpsc::Receiver<tui::permission_bridge::PermissionExchange>,
```

- [ ] **Step 2: Extract `build_runtime_from_config`**

Refactor so config resolution and the build are separable. Replace the body of `build_runtime` (`:317`) and add the shared helper:

```rust
/// Shared engine assembly: build the runtime from an already-resolved
/// [`DesktopConfig`] + output sink. Lets the TUI path inject a permission gate
/// derived from the SAME `cfg` without resolving config twice.
pub async fn build_runtime_from_config(
    cfg: DesktopConfig,
    output: Arc<dyn OutputStream>,
) -> Result<Runtime, InitError> {
    let permission_sink: Arc<dyn client_adapter::PermissionRequestSink> =
        Arc::new(NoopPermissionRequestSink);
    let rt = build(cfg, output, permission_sink).await?;
    Ok(Runtime {
        orchestrator: rt.orchestrator,
        dispatcher: rt.dispatcher,
        auth: rt.auth,
        task_registry: rt.task_registry,
        settings_watcher: rt.settings_watcher,
        file_changed_watcher: rt.file_changed_watcher,
        subscription: rt.subscription,
        provider_availability: rt.provider_availability,
        model_providers: rt.model_providers,
        provider_key_store: rt.credentials,
    })
}

pub async fn build_runtime(
    argv: &Argv,
    output: Arc<dyn OutputStream>,
    permission_mode: permission::PermissionMode,
) -> Result<Runtime, InitError> {
    let cfg = resolve_desktop_config(argv, permission_mode);
    build_runtime_from_config(cfg, output).await
}
```

(This keeps `build_runtime` byte-identical in behavior — same sink, same projection — just delegating the build half.)

- [ ] **Step 3: Construct the gate + channel in `build_runtime_for_tui`**

Replace the body of `build_runtime_for_tui` (`:353`) so it resolves config once, builds the gate from that `cfg`, injects it, and returns the receiver:

```rust
pub async fn build_runtime_for_tui(argv: &Argv) -> Result<TuiBuild, InitError> {
    let (bridge_tx, bridge_rx) = tokio::sync::mpsc::unbounded_channel();
    let turn_tx = bridge_tx.clone();
    let bridge: Arc<dyn OutputStream> = Arc::new(tui::BridgeOutputStream::new(bridge_tx));
    let (permission_mode, _notice) = crate::resolve_permission_mode(argv);

    // (TUI-PERM) Resolve config ONCE so the gate's persist paths come from the
    // SAME cfg the engine builds with (no double resolve, no lost paths).
    let mut cfg = resolve_desktop_config(argv, permission_mode);

    // Interactive permission gate: an unresolved mutating `Ask` surfaces the
    // TUI dialog over this channel instead of auto-allowing. AllowAlways
    // persists to <cwd>/.claude/settings.local.json (via `.with_persist`).
    let (perm_tx, perm_rx) =
        tokio::sync::mpsc::channel::<tui::permission_bridge::PermissionExchange>(16);
    let session_allow_rules = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let gate = std::sync::Arc::new(
        tui::permission_bridge::TuiPermissionGate::new(perm_tx, session_allow_rules).with_persist(
            permission::PermissionPaths {
                claude_home: cfg.claude_home.clone(),
                cwd: cfg.cwd.clone(),
            },
        ),
    );
    cfg.injected_permission_gate = Some(gate);

    let runtime = build_runtime_from_config(cfg, bridge).await?;
    Ok(TuiBuild {
        runtime,
        bridge_rx,
        turn_tx,
        permission_rx: perm_rx,
    })
}
```

NOTE: confirm `tui::permission_bridge::{PermissionExchange, TuiPermissionGate}` are `pub` (they are) and that `permission::PermissionPaths` is the type the gate's `with_persist` takes (it is — `PermissionPaths { claude_home, cwd }`). `cfg.injected_permission_gate` expects `Option<Arc<dyn PermissionGate>>`; `TuiPermissionGate: PermissionGate`, so the `Arc<TuiPermissionGate>` coerces — if the compiler needs an explicit cast, write `Some(gate as std::sync::Arc<dyn permission::gate::PermissionGate>)`.

- [ ] **Step 4: Remove the stale comment in `resolve_desktop_config`**

In `resolve_desktop_config` (`:280`), update the `injected_permission_gate: None` comment — it currently says "Interactive gate injected by `build_runtime_for_tui` ... not here". That is now accurate, so keep the "injected by build_runtime_for_tui" note but drop any "until wiring lands" phrasing if present. Also update the `deny_unresolved_ask` comment that says "until the TUI permission-prompt wiring lands" to reflect that interactive runs now prompt (the `-p` headless path still denies unresolved asks).

- [ ] **Step 5: Build clean**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p cli 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 6: Add a test that the TUI build wires the gate + receiver**

Add to the `#[cfg(test)] mod tests` in `apps/cli/src/init.rs`:

```rust
    #[tokio::test]
    async fn build_runtime_for_tui_wires_permission_channel() {
        let argv = Argv::default(); // non-print, default mode
        let build = build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        // The receiver exists and is open (the gate holds the sender).
        // try_recv on an empty-but-open channel returns Empty, not Disconnected.
        let mut rx = build.permission_rx;
        assert!(
            matches!(rx.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)),
            "permission_rx must be wired + open (gate holds the sender)"
        );
    }
```

NOTE: match `Argv`'s real default-construction idiom used by the other tests in this module (they build an `Argv`; reuse that exact pattern — e.g. the `base`/`absent` fixtures around `:440-466`). If `build_runtime_for_tui` reads the real FS/env and that's flaky in the harness, gate the assertion to the channel-open check only (above), which does not require a network/LLM.

- [ ] **Step 7: Run the test**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p cli build_runtime_for_tui_wires_permission_channel 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/cli/src/init.rs
git commit -m "feat(cli): inject TuiPermissionGate + return permission_rx from build_runtime_for_tui

Resolves DesktopConfig once and derives the gate's persist paths from it
(build_runtime_from_config), avoiding a double config resolve. The one-shot
path is unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `mode.rs` — thread `permission_rx` into the session Runtime

**Files:**
- Modify: `apps/cli/src/mode.rs` (`build_tui_runtime`, the `Runtime` builder chain `:219-223`)

- [ ] **Step 1: Add the `with_permission_rx` call**

In `build_tui_runtime` (`apps/cli/src/mode.rs:166`), the `Runtime` is built via a builder chain ending around `:219-223`:

```rust
    tui::session::Runtime::with_bridge(session_id, bridge, status)
        .with_orchestrator(orchestrator)
        .with_multiagent_feed(task_feed)
        .with_turn_tx(tui_build.turn_tx)
        .with_command_registry(command_registry)
```

Add `.with_permission_rx(tui_build.permission_rx)` to the chain (anywhere after `with_bridge`; place it next to `with_turn_tx` for readability). `tui_build` is consumed field-by-field here — confirm `permission_rx` is moved exactly once (it is `Receiver`, not `Clone`). If `tui_build` fields are read in a specific order that would conflict, move the `with_permission_rx` call to read `tui_build.permission_rx` last.

- [ ] **Step 2: Build clean**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p cli 2>&1 | tail -5`
Expected: clean — the full chain (init → mode → session → root pump) now compiles end-to-end.

- [ ] **Step 3: Run cli + tui suites**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p cli -p tui 2>&1 | tail -20`
Expected: pass / no new failures.

- [ ] **Step 4: Commit**

```bash
git add apps/cli/src/mode.rs
git commit -m "feat(cli): wire permission_rx into the TUI session runtime

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Build-seam integration test + engine-desktop doc refresh

The critical [P2] test: prove `PolicyPermissionGate` delegates an unresolved mutating `Ask` to the injected `TuiPermissionGate` (the real gate), and that read-only tools resolve WITHOUT a dialog. This lives in the `tui` crate (which depends on `permission`), exercising the real composition end-to-end without a full engine turn.

**Files:**
- Test: `tui/src/permission_bridge.rs` (extend its `#[cfg(test)] mod tests`)
- Modify (doc only): `apps/engine-desktop/src/lib.rs` (`:1958-1960` comment)

- [ ] **Step 1: Write the integration test**

Add to `#[cfg(test)] mod tests` in `tui/src/permission_bridge.rs` (it already has `use serde_json::json;` and the mpsc/oneshot imports):

```rust
    /// [P2] The real seam: PolicyPermissionGate (default mode, no rules) wraps
    /// the injected TuiPermissionGate. A mutating tool with no rule is an
    /// unresolved `Ask` → delegated to the inner gate → one exchange on the
    /// channel → the reply resolves the blocked check().
    #[tokio::test]
    async fn policy_gate_delegates_unresolved_ask_to_tui_gate() {
        use permission::gate::{PermissionDecision, PermissionGate, PermissionResponse};
        use permission::{PermissionMode, PermissionPolicy};
        use std::sync::Arc;

        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn PermissionGate> =
            Arc::new(TuiPermissionGate::new(event_tx, rules));
        let policy = Arc::new(PermissionPolicy::new(PermissionMode::Default));
        let gate = permission::PolicyPermissionGate::new(policy, inner);

        // A "TUI" that answers AllowOnce to the first exchange.
        let responder = tokio::spawn(async move {
            let ex = event_rx.recv().await.expect("one exchange expected");
            // It must be the Write tool we asked for.
            if let permission::gate::PermissionRequest::ToolUseConfirm { tool_name, .. } =
                &ex.request
            {
                assert_eq!(tool_name, "Write");
            } else {
                panic!("expected ToolUseConfirm");
            }
            ex.resp_tx.send(PermissionResponse::AllowOnce).unwrap();
        });

        let decision = gate.check("Write", &json!({"file_path": "a.txt"})).await;
        assert!(matches!(decision, PermissionDecision::Allow));
        responder.await.unwrap();
    }

    /// [P2] A Deny reply resolves the blocked check() to Deny.
    #[tokio::test]
    async fn policy_gate_relays_deny_from_tui_gate() {
        use permission::gate::{PermissionDecision, PermissionGate, PermissionResponse};
        use permission::{PermissionMode, PermissionPolicy};
        use std::sync::Arc;

        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn PermissionGate> = Arc::new(TuiPermissionGate::new(event_tx, rules));
        let policy = Arc::new(PermissionPolicy::new(PermissionMode::Default));
        let gate = permission::PolicyPermissionGate::new(policy, inner);

        let responder = tokio::spawn(async move {
            let ex = event_rx.recv().await.expect("one exchange");
            ex.resp_tx.send(PermissionResponse::Deny).unwrap();
        });

        let decision = gate.check("Write", &json!({})).await;
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        responder.await.unwrap();
    }

    /// [P2] A read-only tool (AllowByDefault) is resolved by the policy itself —
    /// the inner TuiPermissionGate is NEVER consulted (no exchange emitted).
    #[tokio::test]
    async fn policy_gate_auto_allows_readonly_without_dialog() {
        use permission::gate::{PermissionDecision, PermissionGate};
        use permission::{PermissionMode, PermissionPolicy};
        use std::sync::Arc;

        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn PermissionGate> = Arc::new(TuiPermissionGate::new(event_tx, rules));
        let policy = Arc::new(PermissionPolicy::new(PermissionMode::Default));
        let gate = permission::PolicyPermissionGate::new(policy, inner);

        let decision = gate.check("Read", &json!({"file_path": "a.txt"})).await;
        assert!(matches!(decision, PermissionDecision::Allow));
        // No exchange should have been emitted.
        assert!(
            matches!(event_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "read-only tool must not consult the interactive gate"
        );
    }
```

NOTE: verify the exact paths — `permission::PolicyPermissionGate`, `permission::PermissionPolicy`, `permission::PermissionMode`, `permission::gate::{PermissionDecision, PermissionGate, PermissionResponse, PermissionRequest}`. If any is re-exported at a different path, `grep -n "pub use\|pub struct PolicyPermissionGate\|pub enum PermissionMode" permission/src/lib.rs` and fix the import. If `PermissionPolicy::new(Default)` does NOT yield `Ask` for "Write" (e.g. default mode allows by rule), switch to whatever default produces an unresolved mutating ask (the assertion — one exchange for Write, none for Read — is the contract; adjust the policy construction to satisfy the premise, not the assertion).

- [ ] **Step 2: Run the integration tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui policy_gate_ 2>&1 | tail -20`
Expected: all three pass.

- [ ] **Step 3: Refresh the stale engine-desktop doc comment**

In `apps/engine-desktop/src/lib.rs`, the comment at `:1958-1960` says the CLI inner gate stays `NoOpPermissionGate` and "interactive prompting needs a TUI permission sink (a documented follow-up)". Update it to note that the interactive TUI now injects `TuiPermissionGate` via `cfg.injected_permission_gate` (the `if let Some(injected)` arm at `:1836`), so an unresolved mutating `Ask` in an interactive session now surfaces the dialog; the headless `-p` path still uses `NoOpPermissionGate`/`DenyOnAskGate`. Keep it concise and accurate; do not change code.

- [ ] **Step 4: Build engine-desktop (doc-only change compiles)**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add tui/src/permission_bridge.rs apps/engine-desktop/src/lib.rs
git commit -m "test(tui): integration test for PolicyPermissionGate→TuiPermissionGate delegation + doc refresh

Proves an unresolved mutating Ask delegates to the injected interactive gate
(AllowOnce→Allow, Deny→Deny) while read-only tools auto-allow with no dialog.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Workspace-relevant build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p tui -p cli -p engine-desktop 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 2: Full affected-crate test run**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui -p cli -p permission 2>&1 | tail -25`
Expected: pass / no new failures, including the new `open_permission_dialog`, `promote_next_permission`, `resolving_a_dialog_promotes_the_next_queued_exchange`, `build_runtime_for_tui_wires_permission_channel`, and `policy_gate_*` tests.

- [ ] **Step 3: Confirm the wiring is live (no stale "until wiring lands")**

Run: `grep -rn "until the TUI permission-prompt wiring lands\|until wiring lands" apps/cli/ apps/engine-desktop/ tui/`
Expected: ZERO hits (all stale markers refreshed).

- [ ] **Step 4: Report verification evidence**

Summarize: build status; the new test names + pass counts; confirmation the stale markers are gone. Environment caveat: this sandbox has no TTY/network, so a live interactive dialog cannot be keystroke-driven here — its correctness rests on the unit/integration tests (pump FIFO, resolution promotion, policy→gate delegation round-trip). No commit.

---

## Self-review (run by the plan author)

**Spec coverage** — every design element mapped:
- FIFO queue + one active dialog (P0) → Task 1 (`permission_queue` + `promote_next_permission`), Task 4 (pump push+promote), Task 3 (promote on resolve) ✅
- Shared `open_permission_dialog` setting `started_at` (P1 telemetry) → Task 1 + Task 2 (legacy branch refactor) ✅
- Single config resolve / `build_runtime_from_config` / persist paths (P1) → Task 6 ✅
- Gate construction + injection + `TuiBuild.permission_rx` → Task 6 ✅
- Receiver threading (Runtime → mount → RootProps → pump) → Tasks 5, 7, 4 ✅
- Build-seam integration test proving the gate is used, not NoOp (P2) → Task 8 ✅
- engine-desktop doc refresh → Task 8 ✅
- Headless `-p` unchanged → Task 6 (one-shot path uses `build_runtime` → `build_runtime_from_config`, no gate) ✅
- Out of scope (sandbox code, trust dialog, classifier, mobile/bridge) → untouched ✅

**Type consistency** — names used consistently across tasks: `permission_queue: VecDeque<PermissionExchange>`, `open_permission_dialog(st, request, resp_tx)`, `promote_next_permission(st)`, `PermissionRxSlot = Arc<Mutex<Option<mpsc::Receiver<PermissionExchange>>>>`, `Runtime::with_permission_rx`, `TuiBuild.permission_rx`, `build_runtime_from_config(cfg, output)`, `TuiPermissionGate::new(tx, rules).with_persist(PermissionPaths{claude_home, cwd})`, `cfg.injected_permission_gate: Option<Arc<dyn PermissionGate>>`. `PermissionResponse` = {AllowOnce, AllowAlways, Deny}; `PermissionRequest::ToolUseConfirm{tool_name, tool_input, default_decision}`.

**Placeholder scan** — no TBD/TODO; every code step shows code; ambiguous spots (exact `DialogResolution` shape, `Argv` default fixture, gate trait-object coercion, `PermissionPolicy` default producing an Ask) carry explicit "verify against the real symbol and adjust" notes rather than guesses, because those must be confirmed against the live code at edit time.

**Ordering** — Tasks 1→8 are sequential (later tasks depend on earlier helpers/fields); run strictly in order, one implementer at a time (no parallel edits to the shared `tui` crate).
