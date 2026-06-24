# stream-json P3 Gap Notes

**Branch:** stream-json (worktree: `.worktrees/stream-json/`)
**Date:** 2026-06-24
**Commit:** feat(cli): stream-json P3 — --input-format stream-json (stdin turn-loop + dedup + replay)

---

## What was implemented

### 1. Validation chain (`argv.rs` — `validate_stream_json_input_args`)

Exact error strings from §4.1 SPEC-inferred.md (binary order):

1. `--input-format=stream-json requires output-format=stream-json.`
2. `--input-format=stream-json requires --print.`
3. `--replay-user-messages requires both --input-format=stream-json and --output-format=stream-json.`

Called in `lib.rs` before the stream-json branch. Returns `Err(String)` → emitted as `Error: <msg>` + exit 1.

### 2. stdin NDJSON reader (`apps/cli/src/stream_json_input.rs`)

- `process_line(line, &mut seen_uuids)` — parse JSON, normalize camelCase keys (`requestId`→`request_id`, `response.requestId`→`response.request_id`), dispatch by `type`.
- `read_input_turns(reader, replay, session_id)` — BufRead wrapper that drives `process_line` over lines, collects `UserTurn`s, emits dup-acks.
- `content_to_prompt(value)` — flatten string or content-block array into a plain string for `run_turn`.
- `emit_replay_ack(uuid, session_id)` — emit `{type:"user", isReplay:true, ...}` to stdout.

Frame dispatch:
- `keep_alive` → silently consumed
- `update_environment_variables` → partial: only `CLAUDE_CODE_OAUTH_TOKEN` env var applied (OD-9)
- `user` → role-checked (`"user"` only), uuid-deduped, extracted as `UserTurn`
- `control_request` → `request` field required; accepted (logs P5-deferred warning)
- `control_response` → silently consumed
- `assistant`/`system` → silently consumed (history seeding deferred)
- `bash_command` → silently consumed with stderr log (P3-optional, deferred)
- unknown → `Ignoring unknown message type: <type>` to stderr, consumed

Fatal errors (print to stderr, return `Err`):
- Malformed JSON: `Error parsing streaming input line: <line>: <err>`
- Bad role: `Error: Expected message role 'user', got '<role>'`
- Missing `control_request.request`: `Error: Missing request on control_request`

### 3. Multi-turn loop (`run.rs` — `run_stream_json_input_loop`)

- Reads all stdin turns via `spawn_blocking` (non-blocking stdin read)
- Populates init frame (same logic as `run_stream_json_print`)
- Emits `system/init` + `system/status` before turns
- Iterates turns sequentially through `orchestrator.run_turn`
- Under `--replay-user-messages`: emits a `isReplay:true` ack before each turn
- Emits `result/success` or `result/error_during_execution` after all turns

### 4. Wiring (`lib.rs`)

- `validate_stream_json_input_args()` called before stream-json gate
- `is_stream_json_input()` check inside stream-json branch routes to `run_stream_json_input_loop`

---

## Deferred gaps (P5 / not-in-scope)

| Gap | Description | Why deferred |
|-----|-------------|--------------|
| `bash_command` frame | Binary-only in 2.1.187; exact tag literals not extracted (OD-10) | P3-optional per spec |
| Full `update_environment_variables` allowlist | Only `CLAUDE_CODE_OAUTH_TOKEN` confirmed (OD-9) | Allowlist not extracted from binary |
| `control_request` full protocol | ~50-subtype control switch, `pendingRequests` map | P5 — explicitly deferred |
| `control_response` routing | Resolve pending CLI-originated requests | P5 — explicitly deferred |
| `control_response` + `assistant`/`system` history seeding | Seed `SessionState.history` from inbound frames | P3-optional; needs orchestrator API |
| Inbound `assistant` replay re-emit | Under `--replay-user-messages`, re-emit matched inbound `assistant` verbatim | P3-partial |
| `update_environment_variables` ack | `control_response` success ack when `request_id` present | P5 |
| `--sdk-url` validation | `--sdk-url requires both --input-format=stream-json and --output-format=stream-json.` | P5 |
| `--prompt-suggestions` validation | requires `--print` and `--output-format=stream-json` | P4 |
| `--include-partial-messages` validation | Hard error only if `--include-hook-events` also set | P4 |
| `shouldQuery`/`client_platform`/`inbound_origin` frame fields | Loop-level opportunistic reads on `user` frame | OD-11 — not in strict schema |

---

## Test coverage added

### `stream_json_input.rs` (23 unit tests)
- `normalize_*` (3 tests)
- `keep_alive_is_consumed`
- `unknown_type_is_consumed_with_warning`
- `malformed_json_is_fatal`
- `user_frame_with_string_content_is_extracted`
- `user_frame_with_content_block_array_is_extracted`
- `user_frame_bad_role_is_fatal`
- `user_frame_bad_role_error_string`
- `duplicate_uuid_returns_duplicate_action`
- `different_uuids_both_accepted`
- `control_request_without_request_field_is_fatal`
- `control_request_with_request_field_is_consumed`
- `missing_request_error_string`
- `content_to_prompt_*` (4 tests)
- `read_input_turns_*` (5 tests incl. multi-line order, empty lines, dedup, malformed, bad role)
- `update_env_vars_is_consumed`

### `argv.rs` (9 new tests)
- All 3 validation error strings asserted byte-for-byte
- Valid combinations pass
- `is_stream_json_input` detection

---

## Architecture notes

The stdin read is done synchronously via `tokio::task::spawn_blocking` — the entire stdin is consumed before the first turn starts. This differs from the TS runtime (which streams stdin concurrently with output) but is simpler and correct for the common SDK usage pattern where all turns are pre-buffered. True concurrent stdin streaming would require an async MPSC channel wiring stdin frames into the turn loop while turns run — a follow-up if needed.

The per-turn `emit_replay_ack` emits a NEW uuid for the ack frame rather than reusing the inbound uuid. The inbound uuid is embedded in the debug log `Sending acknowledgment for duplicate user message: <uuid>`. The ack frame shape is `{type:"user", message:{role:"user",content:""}, session_id, parent_tool_use_id:null, uuid:<new>, timestamp:<iso>, isReplay:true}` — the minimal shape per §2.4 GROUND-TRUTH.
