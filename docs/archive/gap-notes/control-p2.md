# Stream-JSON Control Protocol Phase 2 — `can_use_tool` decider

## What was done

The CLI-as-CLIENT permission-over-stdio round-trip (§3 of docs/control-proto/SPEC.md):
the ONE control subtype LingXi *originates*. Built as a shared control-plane +
an injected inner-transport permission gate.

### `control_plane.rs` (NEW)
- **`StdioControlPlane`** — shared bidirectional state created in the CLI BEFORE
  `build_runtime` (its outbound handle = `StreamJsonStream::outbound_tx()`):
  - `pending: Mutex<HashMap<request_id, PendingControlRequest>>` — the
    `pendingRequests` map (structuredIO.ts:469/362).
  - `send_request(request, tool_use_id)` — mints uuid, registers the oneshot
    BEFORE enqueueing the `control_request` frame on the shared single-writer
    outbound channel (FIFO, no overtaking data frames), returns the receiver.
  - `resolve_response(frame)` — looks up the inner `response.request_id`, removes
    the pending entry, resolves `Ok(response.response)` / `Err(response.error)`.
    Inner double-nesting is load-bearing. Absent `response` ⇒ `{}`. Orphan ⇒
    dropped inline (Phase 4 adds the resolved-tool-use dedup drop + log line).
  - `resolved_tool_use_ids` ring (cap 1000, oldest-evicted) — tracking wired;
    the duplicate-DROP check lands in Phase 4.
  - `active_turn_cancel` cell + `set_active_turn`/`cancel_active_turn` — for the
    `deny+interrupt` turn abort (§3.4).
- **`StdioControlPermissionGate`** — inner-transport `PermissionGate` impl:
  - `check`/`check_with_worker` → `decide`: emits `can_use_tool`
    `{tool_name, input, tool_use_id, agent_id?}` (§3.2), awaits the response (NO
    timeout, §3.1), maps `PermissionToolOutput` (§3.3):
    - `allow` → `PermissionDecision::Allow` (the `updatedInput` rewrite is a
      documented deferral — `PermissionDecision::Allow` can't carry a rewritten
      input; allow with ORIGINAL input).
    - `deny` → `Deny{reason: message}`; `deny+interrupt:true` also cancels the
      active turn token.
    - error / channel-closed → `Deny{reason:"Tool permission request failed: …"}`.

### `lib.rs` wiring
- For `is_stream_json_input()` ONLY: build `StdioControlPlane` over the stream's
  outbound handle, construct `StdioControlPermissionGate`, inject via
  `cfg.injected_permission_gate`, build via `build_runtime_from_config`. The
  `PolicyPermissionGate` (enforcement default on) wraps it as the OUTER local
  pre-check → only an unresolved `Ask` round-trips over stdio (§3.5). The
  output-only print path is UNCHANGED (headless deny-on-ask; no stdin reader to
  answer a `control_response`).

### `run.rs` wiring
- `run_stream_json_input_loop` gains a `control_plane: Arc<StdioControlPlane>`
  param. Destructures `StdinChannels`; spawns a dedicated **resolver task** that
  drains `control_resp_rx` → `control_plane.resolve_response` (concurrent with
  the turn loop, so a permission answer arrives mid-turn while the gate awaits).
  Each turn: `control_plane.set_active_turn(cancel.clone())` so `deny+interrupt`
  can abort. The resolver task is awaited during teardown.

## Tests (control_plane.rs, 9)
send_request→success; error-subtype reject; no-inner-payload→`{}`; orphan drop;
gate allow/deny/deny+interrupt(cancels token)/error→deny; check_with_worker sets
`agent_id`. **233 cli lib tests pass** (224 baseline + 9).

## Deferred (per spec §3.5, documented)
- The local PermissionRequest-hook race (`executePermissionRequestHooksForSDK`
  → hook-vs-SDK `Promise.race`, hook winner aborts the pending request).
- `updatedInput` rewrite + `updatedPermissions` persistence.
- Sandbox-network-ask piggyback (reuses `can_use_tool`).
- `LINGXI_ENFORCE_PERMISSIONS=0` path (no outer policy wrap): the stdio gate
  becomes the SOLE gate; today it only resolves the delegated `Ask` (the policy
  pre-check is assumed present, the CLI default).
