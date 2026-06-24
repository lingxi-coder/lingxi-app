# Stream-JSON Control Protocol Phase 4 — control_cancel_request + dedup

## A. Outbound cancel on turn abort (§1.4 / §3.1)
`send_request` now returns `(request_id, rx)`. The gate's `decide` selects the
response receiver against the active turn token: on a turn abort (interrupt /
Ctrl-C) it calls `plane.cancel_request(request_id)` — which removes the pending
entry, tracks its tool_use_id (so a late response dedupes), and enqueues
`{type:"control_cancel_request", request_id}` to the host — then denies with
`"Tool permission request failed: aborted"`. When no turn token is registered
(non-turn check), it falls back to a plain `rx.await`.

## B. Inbound control_cancel_request (§1.4)
Explicit `control_cancel_request` arm in `process_line` → `Consumed`. The
CLI-as-server inbound handlers (initialize/interrupt/set_*/get_*) are synchronous
and resolve before a cancel could arrive, so there is no in-flight async handler
to abort — byte-faithful no-op for the stdio-local path. A request_id→AbortHandle
map is only needed once an async [D] handler (mcp_call/stage_file) lands.

## C. Duplicate-response dedup (§1.5)
`resolved_tool_use_ids` ring (cap 1000, oldest-evicted) is populated on every
resolve/cancel. `resolve_response`: when no pending entry matches, it reads the
payload `toolUseID`; if that id is already in the ring (a websocket-reconnect
double-delivery) it logs the byte-faithful
`Ignoring duplicate control_response for already-resolved toolUseID=… request_id=…`
and drops — preventing a double-resolve that would 400 on a non-unique tool_use id.

## Tests — 2 new (control_plane): **241 cli lib tests pass** (239 + 2)
`turn_abort_emits_control_cancel_request_and_denies`,
`duplicate_control_response_for_resolved_tool_use_is_dropped`; the 3 existing
send_request tests updated for the `(request_id, rx)` tuple.

## P5 COMPLETE — Phase 5+ deep families correctly deferred
Every remaining subtype (MCP family, file family, OAuth `claude_*`, feedback /
side_question / ultrareview / remote_control, and the `initialize` hooks/agents
MERGE sub-protocol + hook_callback/elicitation outbound) falls through to the
byte-exact `Unsupported control request subtype: <x>` — the CORRECT response for
a host requesting a capability whose subsystem does not yet exist, NOT a stub.
