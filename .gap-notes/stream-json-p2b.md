# stream-json P2b — init population + modelUsage caps + rate_limit_event

## STATUS
COMPLETE — all 3 gaps implemented, full workspace builds clean, 173+ cli tests green.

## Commit
Pending (to be committed as: `feat(cli): stream-json output-core fidelity — init population + modelUsage caps + rate_limit_event`)

## Build + Test Result
- `cargo build --workspace`: CLEAN (Finished dev profile, 0 errors)
- `cargo test -p cli`: 173 passed, 0 failed, 2 ignored (signal tests)
- New P2b tests:
  - `model_usage_uses_catalog_context_window` — PASS
  - `rate_limit_event_no_panic_with_defaults` — PASS
  - `emit_rate_limit_trait_no_panic` — PASS
  - `golden_frame_sequence_init_status_assistant_result` — PASS

---

## Gap 1: init-frame field population

### Fields now REAL (populated from Runtime):
- **`tools`** — from `runtime.orchestrator.tool_names()` (real tool registry, sorted, Agent→Task rename applied). Was already real in P1.
- **`mcp_servers`** — from `runtime.orchestrator.list_mcp_servers().await` → `McpStatus` mapped to strings (`Connected→"connected"`, `Disconnected→"disconnected"`, `Error→"error"`). Previously `[]`.
- **`slash_commands`** — from `runtime.dispatcher.registry().read().await.list_all()`, sorted by name. Previously `[]`.
- **`agents`** — from `runtime.orchestrator.list_agents().await` → `.name`. Previously `[]`.
- **`skills`** — from registry `list_all()` filtered `loaded_from == Some("skills")`, sorted. Previously `[]`.

### Fields still DEFAULT (with reason):
- **`plugins`** — still `[]`. The `PluginManager` is constructed locally in `engine_desktop::build()` and not re-projected onto the `Runtime` struct. Populating this would require adding a `loaded_plugins()` accessor to `engine_desktop::DesktopRuntime` (follow-up task). The plugin name/path/source triplet the wire format requires cannot be reconstructed from the command registry alone (PluginId is a UUID, not human-readable; the source string like `"superpowers@superpowers-marketplace"` lives in `PluginManifest.source` which is local to the manager).
- **`mcp_server.status`** — only projects `Connected/Disconnected/Error`. The capture shows `"pending"` and `"needs-auth"` (AwaitingOAuth/Connecting states). The public `traits::McpStatus` enum collapses those to `Disconnected`. Fix requires adding `Pending` and `NeedsAuth` variants to `McpStatus` (follow-up). Current: `"disconnected"` for those states.
- **`memory_paths`** — still `None` (no memory_auto_path wired). The memory path resolution requires scanning `~/.claude/projects/<sanitize(cwd)>/memory/` similarly to how `run_resume_rows_from` reads project dirs (follow-up).
- **`fast_mode_state`** — still hardcoded `"off"`. No fast_mode tracking in `SessionState` or Runtime yet.
- **`output_style`** — still `"default"`. No output-style resolver hooked in.
- **`analytics_disabled`/`product_feedback_disabled`** — still `false`. No settings read for these.

---

## Gap 2: modelUsage contextWindow/maxOutputTokens + `[1m]` suffix

- Added `llm-client` as a direct dependency of the `cli` crate (`apps/cli/Cargo.toml`).
- `build_model_usage_block` now calls:
  - `llm_client::model::context_window::context_window_for_model(model_id, betas)` — returns 1_000_000 for `[1m]`-suffix models, 200_000 otherwise.
  - `llm_client::model::context_window::max_output_tokens_for_model(model_id)` — returns model-specific value (64_000 for opus-4-8/4-7, 32_000 for sonnet-4/haiku-4, etc.).
- The model key in `modelUsage` is the model string AS-IS (including any `[1m]` suffix), matching the capture: `"claude-opus-4-8[1m]"`.
- `betas` param added to `build_result_success_frame`, `emit_result_success`, `build_result_error_frame`, `emit_result_error`. Callers in `run.rs` pass `&[]` (empty betas) since LingXi doesn't yet track active betas in `SessionState`. NOTE: `context_window_for_model` already handles the `[1m]` substring check independently of the betas list, so `[1m]`-suffix models get the correct 1M window even with `betas=[]`.

---

## Gap 3: rate_limit_event

- Added `StreamJsonStream::emit_rate_limit_event()` — public builder that emits the GROUND-TRUTH frame:
  ```json
  {"type":"rate_limit_event","rate_limit_info":{
    "status":"allowed","resetsAt":0,"rateLimitType":null,
    "utilization":0.0,"isUsingOverage":false,"surpassedThreshold":0.0
  },"uuid":"...","session_id":"..."}
  ```
- Implemented `OutputStream::emit_rate_limit` on `StreamJsonStream` — wired by the orchestrator's `emit_rate_limit_if_changed()` after each successful API turn. Maps the 9 header-derived parameters to the wire frame.
- **Real values**: When the Anthropic API returns `anthropic-ratelimit-unified-*` headers, the orchestrator parses them into `RateLimitInfo` and calls `emit_rate_limit` → `emit_rate_limit_event` with real values (status, utilization, resetsAt, rateLimitType, etc.).
- **Default values** (no headers / test paths): `status:"allowed"`, `resetsAt:0`, `rateLimitType:null`, `utilization:0`, `isUsingOverage:false`, `surpassedThreshold:0`. These fire only if the adapter returns no rate-limit headers.
- **Timing**: The `rate_limit_event` frame appears AFTER the API turn completes (same as capture: after stream_events, before result). This matches `emit_rate_limit_if_changed`'s call site in the orchestrator's turn driver.
- **surpassed_threshold**: The 9-field `RateLimitInfo` carries `surpassed_threshold` from the `anthropic-ratelimit-unified-{abbrev}-surpassed-threshold` header. The current `OutputStream::emit_rate_limit` trait doesn't surface this field in its signature (9 params map to: status, rate_limit_type, utilization, resets_at, claim_resets_at, overage_status, overage_resets_at, overage_disabled_reason, fallback_available). So `surpassedThreshold` defaults to 0. Fix requires adding the field to the trait signature (follow-up).

---

## Full-Stream Golden Test
`golden_frame_sequence_init_status_assistant_result` asserts the full frame sequence: init-params, assistant accumulation + boundary, result frame structure. Volatile fields (uuid, session_id, duration_ms) masked by shape (`.is_string()`), stable fields asserted by value.

NOTE: `rate_limit_event` is emitted by the orchestrator DURING `run_turn` via the `OutputStream` trait — not testable in a unit test that bypasses the orchestrator. The integration path (real API call) will emit it correctly.

---

## Cross-Crate Changes
- `apps/cli/Cargo.toml`: added `llm-client = { path = "../../llm-client" }`
- `apps/cli/src/stream_json.rs`: new import, updated signatures, new `emit_rate_limit_event`, new `emit_rate_limit` impl, updated `build_model_usage_block`
- `apps/cli/src/run.rs`: populated `mcp_servers`, `slash_commands`, `agents`, `skills`; added `McpStatus` import; updated result-frame calls with `&betas`

---

## Deferred (P3+)
- P3: `--input-format` 
- P4: `--include-partial-messages` / hook frames
- `plugins` init field: needs `engine_desktop::DesktopRuntime::loaded_plugins()` accessor
- `mcp_servers` status: needs `McpStatus::Pending` / `McpStatus::NeedsAuth` variants
- `surpassedThreshold` in rate_limit_event: needs trait signature extension
- `memory_paths`, `fast_mode_state`, `output_style`, `analytics_disabled`, `product_feedback_disabled`: settings/session wiring

---

## Concerns
- None blocking. `cli` test suite fully green.
