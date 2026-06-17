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
  1. let (perm_tx, perm_rx) = mpsc::channel::<PermissionExchange>(8)  // small bounded; see Error handling
  2. let gate = Arc::new(
         TuiPermissionGate::new(perm_tx, session_allow_rules)
            .with_persist(PermissionPaths { claude_home, cwd }))
  3. build_runtime(argv, output, mode, injected_permission_gate = Some(gate))
  4. return TuiBuild { runtime, bridge_rx, turn_tx, permission_rx: perm_rx }
        │
apps/cli/src/run.rs
  session::Runtime…with_permission_rx(tui_build.permission_rx)
        │
tui/src/session.rs  (mount → RootProps)
  RootProps { …, permission_rx: Some(Arc<Mutex<Option<Receiver>>>) }
        │
tui/src/root.rs  (new pump — third use_future drain loop)
  while let Some(PermissionExchange{request, resp_tx}) = rx.recv().await {
     let mut st = state.lock().await;
     st.pending_permission         = Some(PendingPermission{ request, worker: None });
     st.pending_permission_resp_tx = Some(resp_tx);
     st.tool_use_dialog_state      = ToolUseConfirmState::default();
     drop(st);
     telemetry::permission_dialog_shown("tool_use");
     tick();
  }
        │
existing dialog render + keymap resolution → fires resp_tx → gate.check() returns the decision
```

The orchestrator side is unchanged: when a tool's `Ask` is not resolved by `PolicyPermissionGate` (no matching deny/allow rule, not read-only auto-allow, not sandbox-auto-allow), it delegates to the injected `TuiPermissionGate`, which blocks on the `oneshot` until the pump-fed dialog resolves.

## Components / file structure

1. **`apps/engine-desktop/src/lib.rs`** — NO core change; `build()` already honors `cfg.injected_permission_gate`. Optional: refresh the now-outdated doc note at `:1958-1960` ("interactive prompting needs a TUI permission sink (a documented follow-up)") to reflect that the TUI now injects the gate.
2. **`apps/cli/src/init.rs`** —
   - `build_runtime` gains a parameter `injected_permission_gate: Option<Arc<dyn PermissionGate>>`. It sets `cfg.injected_permission_gate = injected_permission_gate` after `resolve_desktop_config`. All existing callers (one-shot path, tests) pass `None` → byte-identical behavior.
   - `build_runtime_for_tui` builds the `mpsc::channel::<PermissionExchange>()`, constructs the `TuiPermissionGate` (fresh `session_allow_rules`, `.with_persist`), passes `Some(gate)` to `build_runtime`, and returns the receiver on `TuiBuild`.
   - `TuiBuild` gains `permission_rx: tokio::sync::mpsc::Receiver<PermissionExchange>`.
   - Remove the stale `injected_permission_gate: None` "until wiring lands" comment in `resolve_desktop_config`.
3. **`tui/src/session.rs`** — `Runtime` gains a `permission_rx` slot + `with_permission_rx(rx)` builder (mirroring `with_turn_tx`/`bridge_rx`), threaded into `RootProps` as `Some(Arc<Mutex<Option<Receiver>>>)`.
4. **`tui/src/root.rs`** — add the permission pump: a third `use_future` drain loop in the same idiom as the bridge pump (`:2007`) and multiagent pump (`:2035`), draining `PermissionExchange` into the `AppState` slots above.
5. **`apps/cli/src/run.rs`** — thread `tui_build.permission_rx` into `session::Runtime::with_permission_rx`.

## Error handling

- **Dropped `resp_tx`** (dialog closed / cancelled without a choice): the gate already maps the dropped oneshot to `Deny`. The pump's only obligation is to not leave stale `pending_permission` state — resolution (including cancel) clears it via the existing keymap path.
- **`mpsc` send failure / full channel** inside the gate's `check`: handled by the gate (existing behavior); the pump only consumes.
- **`AllowAlways`**: gate appends to `session_allow_rules` (future same-tool calls short-circuit) and persists to `settings.local.json` via `.with_persist` — already implemented.
- **Headless one-shot (`-p`)**: `build_runtime` receives `None`, keeps the `NoOpPermissionGate` / `DenyOnAskGate` selection — no prompt path (no TTY). Unchanged.
- **Channel capacity:** use a small bounded channel (e.g. 8). Permission checks are inherently serialized by the user (one dialog at a time), so a small buffer is sufficient; the gate awaits its reply before the next check anyway.

## Testing (TDD)

- **`init.rs`**: `build_runtime_for_tui` returns a `TuiBuild` whose injected gate is `Some` (assert via a build-path observable, e.g. that the channel is wired / `permission_rx` is present), while the one-shot `build_runtime(..., None)` leaves the base gate as `NoOp`/`Deny`. Existing `engine-desktop` boot tests stay green (one-shot path unchanged).
- **`root.rs` pump**: feeding a `PermissionExchange` into the receiver sets `pending_permission` + `pending_permission_resp_tx` + resets `tool_use_dialog_state`; resolving the dialog (Allow/Deny/AllowAlways) fires the oneshot with the matching `PermissionResponse`.
- **Gate round-trip** (extend existing `permission_bridge.rs` tests): `check` on a tool with no session rule emits one `PermissionExchange` and blocks until the oneshot is filled; an `AllowAlways` reply appends a session rule so the next same-tool `check` short-circuits to `Allow` without emitting.
- **Regression**: an explicit deny rule still denies BEFORE the gate emits (PolicyPermissionGate resolves first); a read-only tool auto-allows without a dialog.

## Out of scope

- Sandbox code changes (already correct; opt-in parity).
- `project_trust` trust-dialog wiring and the safety classifier (separate sandbox-parity follow-ups).
- Mobile / bridge-server permission flow (already uses `AdapterPermissionGate` + WS sink → client dialog) — unchanged.
- The `ExitPlanMode` / `BypassPermissionsMode` permission variants beyond what already works — this change is about the `ToolUseConfirm` path; the other variants ride the same `pending_permission` state and are not regressed.
