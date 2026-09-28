# Stream-JSON Control Protocol Phase 1

## What was done

### Task 1: `ControlPlaneWriter` (stream_json_input.rs)
- Added `ControlPlaneWriter` struct wrapping `Arc<OutboundTx>`.
- `reply_success(request_id, payload)` — builds success envelope + serializes + sends.
- `reply_error(request_id, msg)` — builds error envelope + serializes + sends.

### Task 2: 46-arm dispatcher (run.rs)
- Added `dispatch_control_request` free function (sync, called from the async ctrl task).
- Replaced Phase-0 stub `ctrl_req_task` with `ControlPlaneWriter`-based loop.
- All unknown subtypes still return the byte-exact fallthrough error via `_ =>` arm.

### Task 3: `initialize` handler
- Pre-collects `init_commands`, `init_agents`, `init_models`, `init_account` before spawning the ctrl task (no `.await` inside the dispatcher).
- `init_commands`: from `registry.list_all()`, filtered on `user_invocable != Some(false)`, sorted by name; description uses `format_description_with_source`.
- `init_agents`: from `orchestrator.list_agents()`, name + description.
- `init_models`: from `orchestrator.list_model_listings()`; capability flags mapped in `model_capabilities()` (opus→effort+adaptive, sonnet→effort+fast+auto, haiku/default→all false).
- `init_account`: default `{"email":"","organization":"","subscriptionType":"Claude Max","apiProvider":"firstParty"}`.
- `pid`: `std::process::id()`.
- `feedback_survey_config`: hard-coded golden defaults.

### Task 4: `interrupt` handler + cancel wiring
- `tokio::sync::watch::channel(false)` shared between ctrl task and turn loop.
- `interrupt` arm fires `cancel_tx.send(true)` then replies success.
- Turn loop creates `CancellationToken` per turn + watcher task bridges watch→token.
- Calls `orchestrator.run_turn_streaming_with_cancel(prompt, cancel)` (concrete `ConversationOrchestrator` method, returns `OrchestratorError`).
- On error resets cancel to `false` before breaking.

### Task 5: Unit tests (stream_json_input.rs)
- `control_plane_writer_reply_success_envelope_shape` — verifies shape including payload.
- `control_plane_writer_reply_error_envelope_shape` — verifies error envelope.
- `control_plane_writer_reply_success_no_payload_omits_response_key` — None → absent key.

## Gaps / deferred
- `init_account`: email/org/subscriptionType are always default; full auth integration is Phase 3+.
- `init_models` capability flags are heuristic-based (request_model substring match); a proper model catalog with flags would be more robust.
- The 44 unsupported subtypes all return the fallthrough error; each is a future Phase task.
- `control_response` frames (phase 2+) are still dropped (same as Phase 0).
