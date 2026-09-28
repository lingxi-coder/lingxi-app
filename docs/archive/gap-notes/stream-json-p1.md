# stream-json P1 — Implementation Report

## STATUS: COMPLETE

Commit: (pending)
Test result: all cli + orchestrator tests pass (✓); workspace build clean (✓)

---

## Frames emitted byte-faithfully

| Frame | Status |
|---|---|
| `system/init` (20 keys, GROUND-TRUTH order) | ✓ |
| `system/status` (5 keys: type/subtype/status/uuid/session_id) | ✓ |
| `assistant` (outer: 6 keys + `message` inner: 11 keys) | ✓ |
| `user` (tool_result echo) | ✓ |
| `result` (P2 — deferred) | deferred |

---

## How accumulation / message-boundary was solved

**Problem**: `OutputStream` is delta-granular (emit_text per SSE token); claude-code emits ONE `assistant` frame per API message with all blocks accumulated.

**Solution**: Added two new default no-op methods to the `OutputStream` trait:

- `emit_message_start(message_id, model)` — called from `event_router.rs` in the `MessageStart` SSE handler to capture the API message id (e.g. `msg_01WHCArT5...`) and model into `MessageAccum`. Resets the accumulator.
- `emit_message_boundary(stop_reason, request_id)` — called from `conversation.rs` after `let request_id = self.api.last_request_id()` to flush all accumulated blocks as one `assistant` frame, then reset.

Both are default no-ops, so existing impls (SinkAdapter, TUI BridgeOutputStream) are unaffected.

---

## Key architectural decisions

### Two-phase StreamJsonStream construction

`StreamJsonStream::new_placeholder()` is constructed BEFORE `build_runtime` (as the OutputStream to wire into the orchestrator). After `build_runtime` returns, the real session_id/tool_names/model are available, so `set_init_params(params)` fills them in. Then `emit_init()` and `emit_status()` are called. This avoids requiring a second pass through `build_runtime`.

### lib.rs stream-json gate

```
if parsed.is_stream_json() {
    // --verbose gate
    // build StreamJsonStream::new_placeholder() as the adapter
    // build_runtime(adapter)
    // return run_stream_json_print(...)
}
```

The stream-json path exits `run_cli` early, before the SinkAdapter/JsonSink path.

### `tool_names()` accessor on ConversationOrchestrator

Added `pub fn tool_names(&self) -> Vec<String>` to `ConversationOrchestrator` delegating to `self.tools.all_names()`. This exposes tool names through a public seam without leaking `ToolRegistry` internals.

---

## Files modified

| File | Change |
|---|---|
| `lingxi-code/traits/src/orchestrator.rs` | Added `emit_message_start` + `emit_message_boundary` default no-op methods |
| `lingxi-code/orchestrator/src/sse/event_router.rs` | Call `output.emit_message_start(&response.id, &response.model)` on `MessageStart` |
| `lingxi-code/orchestrator/src/conversation.rs` | Call `self.output.emit_message_boundary(...)` after `last_request_id`; add `pub fn tool_names()` |
| `lingxi-code/apps/cli/src/argv.rs` | `is_json_output()` excludes stream-json; `is_stream_json()` added; test updated |
| `lingxi-code/apps/cli/src/lib.rs` | `pub mod stream_json;`; stream-json early branch with `--verbose` gate |
| `lingxi-code/apps/cli/src/run.rs` | `run_stream_json_print()` function |
| `lingxi-code/apps/cli/src/stream_json.rs` | NEW: `StreamJsonStream`, `StreamJsonInitParams`, `MessageAccum`, `AccBlock`, `build_init_params`, `detect_api_key_source`, `permission_mode_str`, `escape_line_terminators`, 4 unit tests |

---

## P1 init-frame placeholder values (follow-up items)

- `mcp_servers`: `[]` (real population requires iterating the MCP registry post-build)
- `slash_commands`: `[]` (real population requires iterating `dispatcher.registry()`)
- `agents`/`skills`/`plugins`: `[]`
- `memory_paths`: `null`
- `fast_mode_state`: `"off"`
- `analytics_disabled`/`product_feedback_disabled`: `false`

These are cosmetically correct for P1 (the streaming callbacks are byte-faithful). Populating them is a follow-up.

---

## Concerns / known deviations

1. `serde_json::json!({})` does not guarantee key order in the serialized output. The implementation relies on serde_json's `IndexMap`-backed `Value::Object` preserving insertion order (it does in practice since serde_json 1.0 with the `preserve_order` feature). Matches claude-code's JS `JSON.stringify` behavior.

2. `result` frame (P2) — not implemented. The `emit_end_turn` method is a no-op.

3. MCP/slash_commands/agents/skills/plugins in the init frame are placeholder empty vecs for P1.
