# Telemetry Parity Audit

## Summary
| Metric | Count |
|---|---|
| Binary event names (unique) | 1450 |
| LingXi event names (unique) | 445 |
| Exact name matches | 65 |
| Missing (binary NOT in LingXi) | 1385 |
| Fabricated (LingXi NOT in binary) | 380 |

## Key Observation: Naming Divergence
LingXi uses structurally different event names for the most critical high-traffic events. These are NOT missing — they're renamed:

| Binary name | LingXi name | Severity |
|---|---|---|
| `tengu_api_query` | `tengu_api_request_started` | CRITICAL - naming divergence |
| `tengu_api_success` | `tengu_api_request_succeeded` | CRITICAL - naming divergence |
| `tengu_api_error` | `tengu_api_request_failed` | CRITICAL - naming divergence |
| `tengu_session_start` | `tengu_session_started` | HIGH - naming divergence |
| `tengu_agent_created` | `tengu_agent_started` | MEDIUM - naming divergence |

## Missing Events (by category, tied to reachable flows)

| Event | Flow | Binary evidence | LingXi has flow? | Severity |
|---|---|---|---|---|
| `tengu_api_query` | Every API call (payload: messagesLength, thinkingType, previousRequestId, buildAgeMins) | 2 string occurrences | YES (uses tengu_api_request_started) | CRITICAL — payload mismatch |
| `tengu_api_success` | Successful API response | 7 occurrences | YES (uses tengu_api_request_succeeded) | CRITICAL — different name |
| `tengu_api_error` | API error path | 5 occurrences | YES (uses tengu_api_request_failed) | CRITICAL — different name |
| `tengu_api_retry` | API retry logic | in binary | YES (uses tengu_api_retry_started) | HIGH — different name |
| `tengu_compact` | Compaction triggered | 20 occurrences, payload: summaryRequest, appState, droppedMessages | YES but no tengu_compact emit found | HIGH |
| `tengu_compact_failed` | Compaction failure | 7 occurrences, payload: prompt_too_long, no_streaming_response | YES (compaction crate exists) | HIGH |
| `tengu_session_start` | Session startup, payload: previous_session_id, dangerouslySkipPermissionsPassed | 2 occurrences | YES (emits tengu_session_started) | HIGH — different name |
| `tengu_hook_output_persisted` | Hook output saving, payload: truncatedFallback, json | present | YES (hooks implemented) | MEDIUM |
| `tengu_permission_request_option_selected` | Permission dialog resolution | 4 occurrences, payload: sandboxingEnabled | YES (TUI implemented) | MEDIUM |
| `tengu_permission_request_escape` | Permission dialog dismissed | present | YES | LOW |
| `tengu_mcp_server_connection_succeeded` | MCP server connects | present | YES | MEDIUM |
| `tengu_mcp_server_connection_failed` | MCP server fails | present | YES | MEDIUM |
| `tengu_mcp_tool_call_auth_error` | MCP auth failure | present | YES | MEDIUM |
| `tengu_mcp_degraded` | MCP degraded mode | present | partial (MCP exists) | LOW |

## Fabricated Events (LingXi only — NOT in binary)
High-signal fabricated events that represent intentional LingXi naming choices:

- `tengu_api_request_started` / `tengu_api_request_succeeded` / `tengu_api_request_failed` — LingXi's renamed versions of `tengu_api_query`/`tengu_api_success`/`tengu_api_error`
- `tengu_api_streaming_started` / `tengu_api_streaming_completed` / `tengu_api_streaming_failed` — LingXi additions not in binary
- `tengu_agent_started` / `tengu_agent_completed` / `tengu_agent_failed` — LingXi additions (binary uses `tengu_agent_created`, etc.)
- `tengu_orchestrator_turn_streaming_started` / `tengu_orchestrator_turn_streaming_completed` — LingXi additions
- `tengu_session_started` — LingXi's version of binary's `tengu_session_start`
- `tengu_command_*_{started,completed,failed}` (48 events) — LingXi structured command lifecycle; binary has simpler names
- `tengu_cost_*` (12 events) — LingXi extended cost tracking
- `tengu_memory_*` (11 events) — LingXi extended memory tracking

## Payload Mismatches Found

| Event pair | Binary fields | LingXi fields | Gap |
|---|---|---|---|
| `tengu_api_query` (bin) vs `tengu_api_request_started` (LingXi) | messagesLength, thinkingType, previousRequestId, buildAgeMins | model, request_id, stream | MISSING: messagesLength, thinkingType, previousRequestId, buildAgeMins |
| `tengu_compact` (bin) | summaryRequest, appState, stripNonEssential, onResponseLength, promptCacheSharingEnabled, ptlAttempts, droppedMessages | NOT EMITTED | Full event missing |
| `tengu_session_start` (bin) | previous_session_id, dangerouslySkipPermissionsPassed, modeIsBypass | LingXi emits tengu_session_started with different payload | Payload not audited |
| `tengu_auto_compact_succeeded` (both) | No major mismatch found | compactedMessageCount tracked | Likely OK |
