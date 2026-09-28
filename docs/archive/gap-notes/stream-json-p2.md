# stream-json P2 gap notes

## What was implemented

1. **CostSnapshot extended** (`traits/src/orchestrator.rs`): added `cache_read_tokens: u64` and `cache_creation_tokens: u64` fields with `#[serde(default)]`.

2. **snapshot_cost_real updated** (`orchestrator/src/conversation.rs`): sums `entry.cache_read_input_tokens` and `entry.cache_creation_input_tokens` from `per_model_usage` entries into the new CostSnapshot fields.

3. **StreamJsonStream expanded** (`apps/cli/src/stream_json.rs`):
   - Added `suppress_frames: bool` field; new constructors `new_json_mode_placeholder()` and `new_json_mode()`.
   - Added `last_result_text: Mutex<String>` field; `emit_message_boundary` captures text before reset; `get_last_result_text()` accessor.
   - Added `build_result_success_frame()` / `emit_result_success()`: builds exact 20-key golden order result frame.
   - Added `build_result_error_frame()` / `emit_result_error()`: error variant with `errors` at position 10.
   - suppress_frames=true: `emit_init`, `emit_status`, `emit_tool_call`, `emit_tool_result`, `emit_thinking`, `emit_message_boundary` all no-op (except still capturing text + resetting accumulator).

4. **run.rs updated** (`apps/cli/src/run.rs`):
   - `run_stream_json_print` now emits result frame after `run_turn` (success → `emit_result_success`; error → `emit_result_error`).
   - Added `run_json_print` which delegates to `run_stream_json_print` (suppress_frames is baked into the stream).

5. **lib.rs updated** (`apps/cli/src/lib.rs`):
   - New branch for `is_json_output() && non-slash-prompt`: routes through `StreamJsonStream::new_json_mode_placeholder()` + `run_json_print`.
   - Slash commands + `--json` still fall through to the old `JsonSink` path (preserves `{"event":"command_output"}` format).

## Gaps / deferred items

- **duration_api_ms / ttft_ms / ttft_stream_ms / time_to_request_ms**: all emit as `null`. LingXi does not track these API-timing metrics yet.
- **contextWindow / maxOutputTokens** in modelUsage: fixed at 200000/32000 (LingXi doesn't expose per-model context window from cost tracker). Oracle has 1000000/64000 for claude-opus-4-8[1m] — follow-up to wire from model catalog.
- **model suffix** (e.g., `[1m]` in oracle key `claude-opus-4-8[1m]`): not appended in LingXi. The model id comes from `session.model` without a tier suffix. Follow-up to include when the subscription tier is known.
- **init frame population**: slash_commands/agents/skills/plugins/mcp_servers remain `[]` (P1 TODOs).
- **`--output-format json --verbose` → JSON array**: deferred per spec.
- **stop_reason for error frame**: hardcoded `null`. Could surface the actual stop_reason from the turn outcome in a follow-up.
