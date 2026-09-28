# Stream-JSON Control Protocol Phase 3a — tractable inbound arms

`dispatch_control_request` made **async** + threaded `Arc<ConversationOrchestrator>`
+ `Arc<TaskRegistry>` + an `end_session` `Notify` into the ctrl-dispatcher task.
The turn loop now `select!`s on `end_notify` so `end_session` breaks it.

## Arms wired (run.rs dispatcher)
- **set_model** (#5) — `switch_model(m, None)`; `"default"`/absent ⇒ no-op success;
  Err ⇒ error frame.
- **mcp_status** (#7) — `{mcpServers:[{name,status}]}` from `list_mcp_servers`.
- **get_context_usage** (#9) — `{usedTokens,maxTokens}` from
  `context_window_usage()` (shape inferred — not byte-dumped).
- **get_session_cost** (#10) — `{text:"Total cost: $x.xxxx"}` from `snapshot_cost`
  (format inferred).
- **get_usage** (#11) — `{input_tokens,output_tokens,cache_*,total_tokens}` from
  `snapshot_cost` (shape inferred).
- **stop_task** (#38) — best-effort `task_registry.kill`; not_found/not_running ⇒
  success `{}`.
- **end_session** (#2) — cancel turn + ack + `end_notify.notify_one()` ⇒ loop breaks.

## Pure arms (`pure_control_response`, unit-tested without an orchestrator)
- **set_max_thinking_tokens** (#6) — ack, no payload (no storage seam).
- **get_binary_version** (#8) — `{version: CLAUDE_CODE_VERSION, buildTime:""}`.
- **rename_session** (#41) — empty title ⇒ error `"title must be non-empty"`;
  valid ⇒ ack (persistence deferred, no title field).
- **message_rated** (#45) — ack `{}` (telemetry-only).
- **seed_read_state** (#21) — ack, no payload (no read-state cache seam).
- **fallthrough `_`** — byte-exact `Unsupported control request subtype: <x>`.

## Tests — 6 (run.rs) + carry: **239 cli lib tests pass** (233 + 6).
fallthrough byte-exact; get_binary_version shape; rename empty→error / valid→ack;
message_rated→`{}`; set_max_thinking/seed_read_state→no-payload.

## Deferred to 3b
- **set_permission_mode** (#4) — the net-new runtime mode-mutation surface
  (gate mode baked at boot). Needs an interior-mutable mode cell on
  `PolicyPermissionGate` + a `PermissionGate::set_permission_mode` trait method +
  an orchestrator handle method.

## Notes (faithful approximations, per spec §2.2 `[T (partial)]`)
- Inferred payload shapes (`get_context_usage`/`get_session_cost`/`get_usage`)
  were NOT byte-dumped in the spec; re-extract from the binary if exact bytes
  are later required.
- `set_max_thinking_tokens` / `seed_read_state` / `rename_session` persistence
  have no storage seam yet — accepted + acked with the correct wire shape.
