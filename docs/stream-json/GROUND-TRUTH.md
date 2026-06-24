# stream-json GROUND-TRUTH (live-captured from claude v2.1.187, 2026-06-24)

**These key orders + shapes are CAPTURED FROM THE REAL BINARY** (`/tmp` → `captures/*.ndjson`), not inferred. They OVERRIDE `SPEC-inferred.md` wherever they differ. The 4 capture files in `captures/` are the golden oracle — implement to byte-match them (after masking volatile fields: `uuid`, `session_id`, `*_ms`, `resetsAt`, `costUSD`, token counts, `timestamp`). Re-capture to refresh: `claude -p "..." --output-format stream-json --verbose [--include-partial-messages|--include-hook-events]`.

## Frame inventory observed in a plain `-p --output-format stream-json --verbose` run (in order)
`system/init` → `system/status`(status:"requesting") → `rate_limit_event` → `system/hook_started`×N + `system/hook_response`×N (SessionStart/Setup whitelist, always) → [`stream_event`×N only if `--include-partial-messages`] → `assistant` → `result/success`.
**`rate_limit_event` and `system/status` are ALWAYS emitted** (the inferred spec wrongly marked them out-of-scope). Hooks: the whitelist (SessionStart/Setup) streams WITHOUT `--include-hook-events`; the flag adds the rest.

## EXACT key orders (declare serde struct fields in THIS order; `serde_json` preserves declaration order)

### system/init (20 keys)
`type`("system"), `subtype`("init"), `cwd`, `session_id`, `tools`(string[]), `mcp_servers`([{name,status}]), `model`, `permissionMode`, `slash_commands`(string[]), `apiKeySource`, `claude_code_version`, `output_style`, `agents`(string[]), `skills`(string[]), `plugins`([{name,path,source}]), `analytics_disabled`(bool), `product_feedback_disabled`(bool), `uuid`, `memory_paths`, `fast_mode_state`.
- **NO `betas` key** (inferred spec was wrong — absent in capture; if it's conditional-on-active-betas, omit when none).
- `plugin_errors`/`plugin_warnings` ABSENT when empty (conditional → `Option`+skip_if_none).
- `fast_mode_state` is a **STRING** (e.g. `"off"`), NOT an object/null.
- `output_style` default = `"default"` (not `"normal"`). `apiKeySource` observed `"none"`. `permissionMode` observed `"bypassPermissions"`.

### assistant
outer: `type`("assistant"), `message`, `parent_tool_use_id`(null), `session_id`, `uuid`, `request_id`.
`message` keys (order): `model`, `id`, `type`("message"), `role`("assistant"), `content`, `stop_reason`, `stop_sequence`, `stop_details`, `usage`, `diagnostics`, `context_management`.
- `message` carries `stop_details` + `diagnostics` (not in inferred spec). content blocks: `{type:"text",text}` | `{type:"thinking",thinking,signature}` | `{type:"tool_use",id,name,input}`.
- `message.usage` keys: `input_tokens, cache_creation_input_tokens, cache_read_input_tokens, cache_creation, output_tokens, service_tier, inference_geo` (a DIFFERENT subset than the result-frame usage — no server_tool_use/iterations/speed here).

### user (tool_result echo) / user REPLAY
`type`("user"), `message`, `session_id`, `parent_tool_use_id`(null), `uuid`, `timestamp`(ISO), [`isReplay`:true — replay variant only], [`isSynthetic`], [`tool_use_result`].

### result/success (20 keys)
`type`("result"), `subtype`("success"), `is_error`(bool), `api_error_status`, `duration_ms`, `duration_api_ms`, `ttft_ms`, `ttft_stream_ms`, `time_to_request_ms`, `num_turns`, `result`(string), `stop_reason`, `session_id`, `total_cost_usd`, `usage`, `modelUsage`, `permission_denials`, `terminal_reason`, `fast_mode_state`, `uuid`.
- **The telemetry fields ARE present** (`api_error_status`, `ttft_ms`, `ttft_stream_ms`, `time_to_request_ms`, `terminal_reason`) — OD-6 was WRONG, do NOT omit. Values are likely `null` (no error) or numbers; emit them (LingXi: `null`/`0` where untracked, or thread real timing).
- `structured_output` appears (after `permission_denials`) only with `--json-schema`.
- error variant: `subtype`∈{error_during_execution,error_max_turns,error_max_budget_usd,error_max_structured_output_retries}; `errors`(string[]) replaces `result`.

### usage (top-level result, snake_case) — exact keys + zero-fill
```json
{"input_tokens":N,"cache_creation_input_tokens":N,"cache_read_input_tokens":N,"output_tokens":N,
 "server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},
 "service_tier":"standard",
 "cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":0},
 "inference_geo":<str|"">,"iterations":[],"speed":"standard"}
```
`service_tier`/`speed` = `"standard"` (CONFIRMED, OD-7 resolved). LingXi tracks input/output/cache_read/cache_creation; emit the rest at these literals.

### modelUsage (camelCase), keyed by model-id WITH context suffix (observed `"claude-opus-4-8[1m]"`)
```json
{"<modelId>":{"inputTokens":N,"outputTokens":N,"cacheReadInputTokens":N,"cacheCreationInputTokens":N,
  "webSearchRequests":0,"costUSD":F,"contextWindow":200000,"maxOutputTokens":32000}}
```
`{}` when empty. The model-id key includes the `[1m]` context-window suffix when the 1M beta is active.

### stream_event (`--include-partial-messages`) — 6 keys
`type`("stream_event"), `event`(verbatim Anthropic SSE event), `session_id`, `parent_tool_use_id`(null), `uuid`, `ttft_ms`. **`ttft_ms` IS present** (OD-12 wrong). `event.type`∈{message_start,content_block_start,content_block_delta,content_block_stop,message_delta,message_stop}.

### rate_limit_event (ALWAYS emitted)
`type`("rate_limit_event"), `rate_limit_info`:{`status`,`resetsAt`(int),`rateLimitType`,`utilization`(float),`isUsingOverage`(bool),`surpassedThreshold`(float)}, `uuid`, `session_id`.

### system/status (ALWAYS emitted, pre-API)
`type`("system"), `subtype`("status"), `status`("requesting"), `uuid`, `session_id`.

### system/hook_started, system/hook_response (`--include-hook-events`, + whitelist always)
started: `type`,`subtype`("hook_started"),`hook_id`,`hook_name`,`hook_event`,`uuid`,`session_id`.
response: `type`,`subtype`("hook_response"),`hook_id`,`hook_name`,`hook_event`,`output`,`stdout`,`stderr`,`exit_code`,`outcome`("success"|"error"|"cancelled"),`uuid`,`session_id`.

## WIRE FORMAT (unchanged from spec §1)
Compact `serde_json::to_string` + U+2028→` `/U+2029→` ` escape + single `\n`; `Option`+`skip_serializing_if="Option::is_none"` for conditional keys; lock stdout per-line (atomic). Every frame has `session_id`+`uuid`. `--output-format json` (no `--verbose`) = the single `result` object only; gating: `--print`+`stream-json` requires `--verbose` else `Error: When using --print, --output-format=stream-json requires --verbose`.
