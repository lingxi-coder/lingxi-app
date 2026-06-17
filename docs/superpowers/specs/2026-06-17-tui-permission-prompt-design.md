# Wire the interactive TUI permission prompt (design)

**Date:** 2026-06-17
**Status:** approved for planning
**Origin:** continuing to close the "built-but-inert-by-default" cluster after Plan 17. An audit of the desktop permission/sandbox stack found the sandbox is already correctly wired (opt-in parity), but the terminal TUI never surfaces claude-code's signature "Do you want to allow this tool? (y / n / always)" prompt — an unresolved `Ask` on a mutating tool **auto-allows** instead of prompting. Every part needed to prompt already exists (`TuiPermissionGate`, the `ToolUseConfirm` dialog, the `AppState` slots, the keymap resolution); it is simply not connected.

## Goal

Connect the existing `TuiPermissionGate` into the terminal TUI runtime so an otherwise-unresolved mutating-tool `Ask` surfaces the already-built `ToolUseConfirm` dialog, and the user's choice (Allow / Deny / Allow-Always) flows back to the orchestrator's blocked `check()`. No new UI component, no engine-core change — wire two existing ends together.

## Audit context (why this is the only gap)

- **Permission policy enforcement is already default-ON** for the CLI/desktop (`PERM.1`, `apps/engine-desktop/src/lib.rs:1942`): `PolicyPermissionGate` wraps the base gate unless explicitly opted out (`LINGXI_ENFORCE_PERMISSIONS` falsey, or `BypassPermissions` mode). Deny/allow rules + permission modes + sandbox-auto-allow already resolve.
- **The sandbox is already real and correctly opt-in** (`SANDBOX.1` + Plan 17): `sandbox_available = sandbox.enabled (settings) AND host deps present` (`lib.rs:2475`); the live `SandboxRuntimeRunner` is injected on desktop (`lib.rs:2520`); `sandbox-auto-allow` is wired (`lib.rs:2044`). Default-off matches claude-code (`sandbox.enabled` opt-in). **No sandbox code change in this project.**
  - Noted parity follow-ups (separate, out of scope): `project_trust` is hardcoded `ProjectTrustLevel::Trusted` (no trust-dialog wiring) and the safety classifier verdict is always `None`. Neither changes default safety.
- **The remaining inert piece is the interactive prompt.** The CLI TUI build hardwires `injected_permission_gate: None` (`apps/cli/src/init.rs:282`, comment: *"until the TUI permission-prompt wiring lands"*), and no pump drains the gate's `mpsc<PermissionExchange>` into the dialog. So the orchestrator's base gate stays `NoOpPermissionGate`, which auto-allows an unresolved `Ask`.

## What already exists (reused as-is)

- `tui/src/permission_bridge.rs`: `TuiPermissionGate` (a `PermissionGate` that sends a `PermissionExchange{request, resp_tx}` over an `mpsc::Sender` and awaits the `oneshot` reply), session-rule short-circuit, `AllowAlways` append + `.with_persist(PermissionPaths)` to `settings.local.json`. A dropped `resp_tx` is already mapped to `Deny{reason:"TUI permission response dropped"}`.
- `tui/src/components/permissions/tool_use_confirm.rs`: the `ToolUseConfirm` dialog component + `ToolUseConfirmState`.
- `tui/src/state.rs`: `AppState.pending_permission: Option<PendingPermission>` (`:478`), `AppState.pending_permission_resp_tx: Option<oneshot::Sender<PermissionResponse>>` (`:651`), `tool_use_dialog_state`.
- `tui/src/root.rs`: the focus-trap that routes keys to the dialog state machine and fires `resp_tx` on resolution.
- `apps/engine-desktop/src/lib.rs:1836`: `build()` already consumes `cfg.injected_permission_gate` — when `Some`, it is used as the base gate (still wrapped by `PolicyPermissionGate` when enforcement is on), so an unresolved `Ask` delegates to it.

## Architecture / data flow

```
build_runtime_for_tui (apps/cli/src/init.rs)
  1. let cfg = resolve_desktop_config(argv, mode)            // resolve ONCE
  2. let (perm_tx, perm_rx) = mpsc::channel::<PermissionExchange>(16)
  3. let gate = Arc::new(
         TuiPermissionGate::new(perm_tx, session_allow_rules)
            .with_persist(PermissionPaths {
                claude_home: cfg.claude_home.clone(),
                cwd: cfg.cwd.clone(),
            }))
  4. cfg.injected_permission_gate = Some(gate)
  5. let runtime = build_runtime_from_config(cfg, output)    // shared helper (no re-resolve)
  6. return TuiBuild { runtime, bridge_rx, turn_tx, permission_rx: perm_rx }
        │
apps/cli/src/run.rs
  session::Runtime…with_permission_rx(tui_build.permission_rx)
        │
tui/src/session.rs  (mount → RootProps)
  RootProps { …, permission_rx: Some(Arc<Mutex<Option<Receiver>>>) }
        │
tui/src/root.rs  (new pump — third use_future drain loop)
  while let Some(exchange) = rx.recv().await {           // PermissionExchange{request, resp_tx}
     let mut st = state.lock().await;
     st.permission_queue.push_back(exchange);            // FIFO — NEVER overwrite an active dialog
     if st.pending_permission.is_none() {
         promote_next_permission(&mut st);               // shared helper, see below
     }
     drop(st);
     tick();
  }
        │
existing dialog render + keymap resolution → fires resp_tx → clears pending_permission
   → promote_next_permission(&mut st)  (drain the next queued exchange, if any)
        │
gate.check() returns the resolved decision
```

The orchestrator side is unchanged: when a tool's `Ask` is not resolved by `PolicyPermissionGate` (no matching deny/allow rule, not read-only auto-allow, not sandbox-auto-allow), it delegates to the injected `TuiPermissionGate`, which blocks on the `oneshot` until the pump-fed dialog resolves.

### Concurrency (one active dialog, FIFO queue)

The streaming executor dispatches concurrency-safe tools on a `FuturesUnordered` (`orchestrator/src/streaming_executor.rs`), and `perms.check()` runs inside each tool's pipeline (`orchestrator/src/turn_loop.rs:1340`). So **two `check()` calls — hence two `PermissionExchange`s — can be in flight at once.** `AppState` has only ONE active permission slot (`pending_permission` + `pending_permission_resp_tx`). In practice mutating tools (Write/Edit/Bash) are exclusive (`is_concurrency_safe == false`) and `process_queue` serializes them, which usually prevents overlap — but the pump MUST NOT depend on that, or a second exchange could overwrite the first's `resp_tx` and silently drop a prompt (the dropped oneshot maps to `Deny` → an unintended denial).

**Decision: a FIFO UI queue (least invasive — `AppState` keeps one active dialog).** The pump always `push_back`s the received exchange onto `AppState.permission_queue` and only promotes the front into the active slot when the slot is free. On resolution (Allow/Deny/AllowAlways, including a dropped/cancelled dialog), after the existing path clears `pending_permission`, it promotes the next queued exchange. This guarantees no `resp_tx` is ever overwritten or lost, regardless of how many checks the orchestrator issues concurrently.

## Components / file structure

1. **`apps/engine-desktop/src/lib.rs`** — NO core change; `build()` already honors `cfg.injected_permission_gate`. Optional: refresh the now-outdated doc note at `:1958-1960` ("interactive prompting needs a TUI permission sink (a documented follow-up)") to reflect that the TUI now injects the gate.
2. **`apps/cli/src/init.rs`** —
   - **Avoid double config resolution / lost persist paths.** Extract a shared `build_runtime_from_config(cfg: DesktopConfig, output) -> Result<Runtime, InitError>` that performs the `engine_desktop::build(cfg, output, sink)` call. The existing `build_runtime` becomes `resolve_desktop_config(argv, mode)` → `build_runtime_from_config(cfg, output)` (behavior-identical for the one-shot path). `build_runtime_for_tui` resolves `DesktopConfig` ONCE, reads `cfg.claude_home`/`cfg.cwd` to build the gate's `PermissionPaths`, sets `cfg.injected_permission_gate = Some(gate)`, then calls `build_runtime_from_config(cfg, output)` directly — no second `resolve_desktop_config`.
   - `build_runtime_for_tui` creates the `mpsc::channel::<PermissionExchange>(16)`, constructs the `TuiPermissionGate` (fresh `session_allow_rules`, `.with_persist`), and returns the receiver on `TuiBuild`.
   - `TuiBuild` gains `permission_rx: tokio::sync::mpsc::Receiver<PermissionExchange>`.
   - Remove the stale `injected_permission_gate: None` "until wiring lands" comment in `resolve_desktop_config`.
3. **`tui/src/state.rs`** —
   - Add `AppState.permission_queue: std::collections::VecDeque<PermissionExchange>` (the FIFO of not-yet-shown exchanges; default empty).
   - Add a shared helper `open_permission_dialog(st, request, resp_tx: Option<oneshot::Sender<PermissionResponse>>)` that sets `pending_permission`, `pending_permission_resp_tx`, `pending_permission_started_at = Some(Instant::now())`, `tool_use_dialog_state = Default::default()`, and fires `telemetry::permission_dialog_shown(...)`. **Refactor the existing legacy `TurnEvent::PermissionRequest` branch (`tui/src/streaming.rs:74`) to call this helper** (passing `resp_tx: None`, preserving its behavior) so BOTH paths set `pending_permission_started_at` — fixing the resolved-telemetry `elapsed_ms = 0` gap.
   - Add `promote_next_permission(st)`: when `pending_permission.is_none()` and `permission_queue` is non-empty, `pop_front()` and `open_permission_dialog(st, ex.request, Some(ex.resp_tx))`.
4. **`tui/src/root.rs`** — add the permission pump: a third `use_future` drain loop in the same idiom as the bridge pump (`:2007`) and multiagent pump (`:2035`). Each received exchange is `push_back`ed onto `permission_queue`, then `promote_next_permission` runs (no-op if a dialog is already active).
5. **`tui/src/events/keymap.rs`** — `resolve_pending_permission` (`:279`) is the single site that fires `resp_tx` and clears `pending_permission`/`started_at`. Append a `promote_next_permission(state)` call at its END (after `pending_permission = None`) so the next queued exchange opens immediately. One site → no missed arm.
6. **`tui/src/session.rs`** — `Runtime` gains a `permission_rx` slot + `with_permission_rx(rx)` builder (mirroring `with_turn_tx`/`bridge_rx`), threaded into `RootProps` as `Some(Arc<Mutex<Option<Receiver>>>)`.
7. **`apps/cli/src/run.rs`** — thread `tui_build.permission_rx` into `session::Runtime::with_permission_rx`.

## Error handling

- **Concurrent exchanges**: handled by the FIFO `permission_queue` (see Concurrency) — a second in-flight exchange is queued, never overwrites the active dialog's `resp_tx`.
- **Dropped `resp_tx`** (dialog closed / cancelled without a choice): the gate already maps the dropped oneshot to `Deny`. Resolution (including cancel) clears `pending_permission` and then runs `promote_next_permission`, so a queued exchange is not stranded.
- **`mpsc` send failure / full channel** inside the gate's `check`: handled by the gate (existing behavior); the pump only consumes.
- **`AllowAlways`**: gate appends to `session_allow_rules` (future same-tool calls short-circuit) and persists to `settings.local.json` via `.with_persist` — already implemented.
- **Headless one-shot (`-p`)**: takes the `resolve_desktop_config` → `build_runtime_from_config` path with no injected gate, keeping the `NoOpPermissionGate` / `DenyOnAskGate` selection — no prompt path (no TTY). Unchanged.
- **Channel capacity:** bounded at 16. Combined with the FIFO queue this comfortably absorbs the realistic concurrent-check count (exclusive mutating tools already serialize); the gate awaits its own reply before its next check, so back-pressure is benign.

## Testing (TDD)

- **Build-seam integration test (the critical one — proves the gate is actually used, not `NoOp`)**: build the engine via `engine_desktop::build` with `injected_permission_gate = Some(TuiPermissionGate)` AND enforcement on, then trigger a mutating, otherwise-unresolved `Ask` (a tool with no matching deny/allow rule). Observe exactly ONE `PermissionExchange` arrive on the receiver; reply `AllowOnce` → the blocked `check()` resolves `Allow`; in a second run reply `Deny` → it resolves `Deny`. This exercises the real seam: injected gate wins at `lib.rs:1836`, `PolicyPermissionGate` delegates the unresolved `Ask` to the inner gate (`permission/src/policy_gate.rs:79`). Asserting only that `permission_rx` exists is NOT sufficient.
- **`init.rs`**: `build_runtime_for_tui` returns a `TuiBuild` with a wired `permission_rx` and an injected gate; the one-shot path (`build_runtime` → `build_runtime_from_config` with no gate) leaves the base gate as `NoOp`/`Deny`. Existing `engine-desktop` boot tests stay green (one-shot path unchanged).
- **`root.rs` pump + FIFO concurrency**: feeding ONE `PermissionExchange` opens the dialog (sets `pending_permission` + `pending_permission_resp_tx` + `pending_permission_started_at` + resets `tool_use_dialog_state`). Feeding a SECOND while the first is unresolved leaves the first active and the second queued (`permission_queue.len() == 1`, first `resp_tx` intact); resolving the first fires its oneshot, then `promote_next_permission` opens the second. Resolving each (Allow/Deny/AllowAlways) fires the matching `PermissionResponse`.
- **Telemetry (`started_at`)**: after `open_permission_dialog`, `pending_permission_started_at` is `Some`, so the resolver's `elapsed_ms` is non-zero — covered for BOTH the new pump and the refactored legacy `streaming.rs` branch.
- **Gate round-trip** (extend existing `permission_bridge.rs` tests): `check` on a tool with no session rule emits one `PermissionExchange` and blocks until the oneshot is filled; an `AllowAlways` reply appends a session rule so the next same-tool `check` short-circuits to `Allow` without emitting.
- **Regression**: an explicit deny rule still denies BEFORE the gate emits (PolicyPermissionGate resolves first); a read-only tool auto-allows without a dialog.

## Out of scope

- Sandbox code changes (already correct; opt-in parity).
- `project_trust` trust-dialog wiring and the safety classifier (separate sandbox-parity follow-ups).
- Mobile / bridge-server permission flow (already uses `AdapterPermissionGate` + WS sink → client dialog) — unchanged.
- The `ExitPlanMode` / `BypassPermissionsMode` permission variants beyond what already works — this change is about the `ToolUseConfirm` path; the other variants ride the same `pending_permission` state and are not regressed.
