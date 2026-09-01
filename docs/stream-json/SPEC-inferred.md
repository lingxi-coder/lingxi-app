# Implementation Spec — claude-code `stream-json` (SDK streaming) for LingXi

Target parity oracle: `@anthropic-ai/claude-code` **v2.1.187** (GIT_SHA `6a53320fad5541a68d79e4b6c53677df77b98e33`, BUILD_TIME `2026-06-23T16:59:46Z`).
LingXi integration target: `main` (the parity HEAD). The on-disk working tree (`fix/android-desktop-divergence`) predates `715adc4e` and lacks `--output-format`; **this work must land on `main` or a branch rebased onto it.** All LingXi citations below are `git show main:<path>` line numbers.

This spec is derived from the five extraction files in `/tmp/stream-json/` (byte-faithful from the oracle binary's minified builders + Zod schemas, cross-checked against leaked TS). Where the binary and leaked TS disagree, **the binary wins** (it is newer and has more keys).

---

## 0. Scope & priority

| Layer | What | Priority | Phase |
|---|---|---|---|
| **OUTPUT (text→frames)** | `system/init` → `user`(replay/tool_result) → `assistant` → `result` NDJSON on stdout | **HIGH-VALUE CORE** | P1–P2 |
| RESULT envelope | the shared terminal frame (also = `--output-format json`) | HIGH (folds into P2) | P2 |
| INPUT (stdin NDJSON) | `--input-format stream-json` user-turn reader + dedup + replay | MEDIUM | P3 |
| PARTIAL / HOOKS | `--include-partial-messages` (`stream_event`), `--include-hook-events` (`system/hook_*`) | MEDIUM-LOW | P4 |
| CONTROL protocol | bidirectional `control_request`/`control_response`/`can_use_tool` permission-over-stdio | **DEEP OPTIONAL — out of scope for first pass** | P5 (deferred) |

The OUTPUT path is the high-value core: it makes `claude-code -p --output-format stream-json --verbose "…"` and `--output-format json` byte-faithful, which is what SDK consumers and `lingxi-code` callers actually parse. The INPUT side and the full SDK control protocol (`can_use_tool` permission prompts over stdio, `initialize` handshake, the ~50-subtype control switch) are a deeper layer; only the user-turn reader + dedup/replay (P3) is in scope for a first pass. The full control protocol (P5) is explicitly deferred.

---

## 1. WIRE FORMAT (applies to every frame)

- **NDJSON, COMPACT**: exactly one `JSON.stringify(obj)` (no `space` arg → no indentation) followed by a single `\n`. LF only.
- **U+2028/U+2029 escaping**: after stringify, replace ` `→literal ` `, ` `→literal ` ` (so a JS line-splitter can't cut a string mid-line). All other JSON standard. Binary `SYm`: `e.replace(/[  ]/g, t => t===" " ? "\\u2028" : "\\u2029")`.
- **`undefined`-valued keys are omitted** by `JSON.stringify` — so source object literals carry keys (`error`, `structured_output`, `tool_use_result`, `isSynthetic`) that vanish from the wire when unset. Rust analog: model these as `Option<T>` with `#[serde(skip_serializing_if = "Option::is_none")]`.
- **Key emission ORDER follows the binary object-literal insertion order** (documented per-frame below). `serde_json` preserves struct field declaration order, so declare struct fields in exactly the documented order. Conditional spreads (`...cond && {k:v}`) only serialize when truthy → `Option` + skip-if-none, declared at the spread's position.
- **Every data frame carries `session_id` (string) and `uuid` (random UUID v4).** init/assistant/user/result/stream_event/system-* all do.
- **`parent_tool_use_id`** is present (`null` or a tool_use-id string) on `assistant`, `user`, `stream_event`, `tool_progress`. `null` = main thread; a tool_use id = subagent output. LingXi P1–P4 emit `null` everywhere (no subagent-nested stdout streaming in headless one-shot).
- Rust writer: a single `write_frame(&self, obj)` that does `serde_json::to_string(obj)` → escape U+2028/29 → `stdout.write_all(line)` + `write_all(b"\n")` + flush. Must serialize to a `Vec<u8>`/`String` and emit atomically per line (lock stdout) so concurrent frames never interleave.

### Gating to reach the OUTPUT path
- `--print`/`-p` **AND** `--output-format stream-json` **AND** `--verbose` REQUIRED. Without `--verbose`: hard error `Error: When using --print, --output-format=stream-json requires --verbose`.
- `--output-format json` (no `--verbose`): single result object (§3). `--output-format json --verbose`: a JSON **array** of all retained messages.

---

## 2. OUTPUT FRAMES — exact JSON shape, in emission order over one turn

Emission order for a plain `-p --output-format stream-json --verbose "hello"` run:
**`system/init`** → (**`user`** replay of the prompt, only under `--replay-user-messages`) → **`assistant`** (1+) → (**`user`** tool_result echoes + more `assistant` turns) → **`result`**.

Total **distinct OUTPUT frame types the P1–P4 implementation must emit: 8** —
`system/init`, `assistant`, `user`, `result`, `stream_event`, `system/hook_started`, `system/hook_progress`, `system/hook_response`.
(The 9th type the INPUT/replay path re-emits is `user` with `isReplay:true` — same `type`, extra key.) Frames beyond these 8 — `system/compact_boundary`, `system/status`, `system/api_retry`, `rate_limit_event`, `tool_progress`, `prompt_suggestion`, `auth_status`, task_*, `session_state_changed`, control-plane — are **out of scope for the first pass** but catalogued in §8 for completeness; LingXi already has the `emit_*` hooks (compaction, rate_limit) to light several of them up later.

### 2.1 `system` / `init` — FIRST frame (binary builder `QKt`, offset 207531483)

Key order EXACTLY (declare struct fields in this order):

```
type:"system", subtype:"init", cwd, session_id, tools[],
mcp_servers[{name,status}], model, permissionMode, slash_commands[],
apiKeySource, betas[], claude_code_version, output_style, agents[], skills[],
plugins[{name,path,source}],
...plugin_errors?[],          // only when pluginErrors.length>0
...plugin_warnings?[],        // only when pluginWarnings.length>0
analytics_disabled, product_feedback_disabled,
uuid,
memory_paths?,                // only when memoryPaths set
fast_mode_state               // set unconditionally AFTER build
// startup_timing?, messaging_socket_path?  — binary/ant-only telemetry; OMIT for LingXi
```

| key | always? | LingXi source |
|---|---|---|
| `type`/`subtype` | YES | literals `"system"`/`"init"` |
| `cwd` | YES | process cwd |
| `session_id` | YES | `orchestrator.current_session_id()` (NOT a fresh mint — fixes G2) |
| `tools` | YES `string[]` | tool names from dispatcher/tool pool; **`Agent`→`Task` rename** (`sdkCompatToolName`) |
| `mcp_servers` | YES `{name,status}[]` | desktop runtime MCP clients; `status` = connection type (`connected`/`pending`/`failed`) |
| `model` | YES | resolved main-loop model id |
| `permissionMode` | YES | `default`/`acceptEdits`/`bypassPermissions`/`plan`/`dontAsk`/`auto` |
| `slash_commands` | YES `string[]` | dispatcher commands with `userInvocable!==false` |
| `apiKeySource` | YES | `getAnthropicApiKeyWithSource().source` — values incl. `none`, `ANTHROPIC_API_KEY`, `/login managed key`, `apiKeyHelper` (**enum set not exhaustively extracted — see Open Decision OD-3**) |
| `betas` | YES `string[]` | active SDK betas (LingXi has the beta-header gate already) |
| `claude_code_version` | YES | the version LingXi reports as claude-code (NOT LingXi's own version — must mirror `"2.1.x"`) |
| `output_style` | YES | settings outputStyle or `"normal"` |
| `agents` | YES `string[]` | agentType names |
| `skills` | YES `string[]` | user-invocable skill names |
| `plugins` | YES `{name,path,source}[]` | source = `name@marketplace`/`name@inline`/`name@builtin` |
| `plugin_errors`/`plugin_warnings` | COND | only if non-empty (LingXi: emit `[]`→omit) |
| `analytics_disabled` | YES bool | (see OD-4: treat as always-present plain property) |
| `product_feedback_disabled` | YES bool | |
| `memory_paths` | COND object | only if memory paths resolved (LingXi has claudeMd/memory paths) |
| `uuid` | YES | random UUID v4 |
| `fast_mode_state` | YES object/null | `getFastModeState(model, fastMode)`; LingXi: `null` unless fast-mode wired |

> The raw material (tools, model, mcp, slash_commands) is scattered on `Runtime`/orchestrator but NOT pre-assembled (G3). New work: a `build_init_frame(&Runtime, session_id)` collector. The SECOND `account`-bearing init block (`control_response` `initialize` handshake) is a DIFFERENT thing (§7) — do not conflate.

### 2.2 `assistant` — the model message (binary `normalizeMessage`, offset 203757936)

```
type:"assistant",
message,                       // the FULL Anthropic Messages-API assistant message object
parent_tool_use_id,            // null (main thread)
session_id, uuid,
error?,                        // SDKAssistantMessageError enum, omitted when undefined
...request_id?,                // only if message.requestId!==undefined
...supersedes?,                // UUID[]; only if prior partial set exists
...tool_use_meta?              // array; only if non-empty
// subagent frames also add subagent_type?/task_description? — out of scope (main thread only)
```

`message` is the full API assistant message: `{id, type:"message", role:"assistant", model, content:[…blocks…], stop_reason, stop_sequence, usage, container, context_management}`. `content` blocks: `{type:"text",text}` | `{type:"thinking",thinking,signature}` | `{type:"tool_use",id,name,input}` (+ redacted/server_tool_use passthrough).

`error` enum (one of): `authentication_failed`, `billing_error`, `rate_limit`, `invalid_request`, `server_error`, `unknown`, `max_output_tokens`.

**CRITICAL ACCUMULATION CONSTRAINT (the hardest part).** LingXi's `OutputStream` is delta-granular: `emit_text` fires **per SSE TextDelta token** (event_router.rs:154), `emit_thinking` per ThinkingDelta (event_router.rs:166), `emit_tool_call` once per tool block (turn_loop.rs:1821), `emit_usage` per MessageDelta/Start. claude-code batches text+thinking+tool_use into **ONE** `assistant` frame per API message. The trait has **no message-boundary / `message_stop` callback** to flush on. So the stream-json sink must:
- accumulate per-message: text blocks (concatenate consecutive `emit_text`), thinking blocks (concatenate `emit_thinking`, attach signature when it arrives — LingXi only has live signature `None`), tool_use blocks (`emit_tool_call`), and the latest `emit_usage` snapshot;
- **flush a single `assistant` frame at the message boundary**. The boundary signal options are (OD-1): (a) the next `emit_tool_result` (a tool_result implies the preceding assistant message is complete), (b) `emit_end_turn` (final flush), or (c) add a new `emit_message_boundary()` trait method fed from event_router's `MessageStop`. **Recommended: add a minimal `emit_message_boundary()` default-no-op trait method** wired from event_router (it already sees `LlmEvent::MessageStop`); cleaner and parity-faithful (one frame per API message). Without it, you can only flush on tool_result/end_turn, which collapses multi-message turns incorrectly.
- The accumulated `message.usage` comes from the last `emit_usage`; `id`/`model`/`stop_reason` need threading from the orchestrator's response accumulator (the `acc` in event_router already holds per-index blocks — surfacing its assembled message is the cleanest source).

### 2.3 `user` — tool_result echo + replays (binary `normalizeMessage` user case)

```
type:"user",
message,                       // {role:"user", content: string | ContentBlockParam[]}  — carries tool_result blocks
parent_tool_use_id,            // null
session_id, uuid, timestamp,   // timestamp = ISO string
isSynthetic,                   // = isMeta||isVisibleInTranscriptOnly; omitted when undefined
tool_use_result?,              // full structured tool output; with mcpMeta: {content:<out>, ...mcpMeta}; omitted when undefined
...origin?                     // only if origin truthy (out of scope)
```

LingXi maps `emit_tool_result(id, tool, result)` → a `user` frame whose `message.content` is a single `tool_result` block: `{type:"tool_result", tool_use_id:id, content:<result>, is_error:<derived>}`. `is_error` derives from `result` being a `{"error":…}` shape (same heuristic as `AdapterOutputStream`). `tool_use_result` is the structured output (the raw `result` value). `timestamp` = now (ISO). NOTE: `emit_tool_result` doesn't currently carry the originating `tool_use_id` cleanly through `SinkAdapter` (it drops the id); the stream-json sink reads the `id` arg directly (the trait DOES pass it).

### 2.4 `user` REPLAY variant (`--replay-user-messages` and prompt ack)

Same as `user` plus literal **`isReplay:true`**:
```
{type:"user", message, session_id, parent_tool_use_id:null, uuid, timestamp, isReplay:true [, isSynthetic][, file_attachments]}
```
(`SDKUserMessageReplay` = `SDKUserMessageContent` + `{uuid, session_id, isReplay:literal(true)}`.) The normal (non-duplicate) user turn is NOT replayed — only the initial-prompt ack and duplicate-uuid acks are. See §5.

### 2.5 `result` — terminal frame → §3 (the shared envelope)

---

## 3. RESULT ENVELOPE (the shared terminal frame; also = `--output-format json`)

ONE envelope, two shapes discriminated by `subtype`. In stream-json it is the FINAL NDJSON line; in `--output-format json` (no `--verbose`) it is the WHOLE stdout (byte-identical single object — throws `No messages returned` if last msg isn't a result). Process exit code = `is_error ? 1 : 0`.

### 3.1 SUCCESS (binary `Ajm`) — emitted key order

```
type:"result", subtype:"success",
is_error, duration_ms, duration_api_ms, num_turns,
result,                        // final assistant text ('' if none)
stop_reason,                   // string|null
session_id, total_cost_usd,
usage,                         // NonNullableUsage — §3.3
modelUsage,                    // record<modelId, ModelUsage> — §3.3 (camelCase!)
permission_denials,            // [{tool_name,tool_use_id,tool_input}]
structured_output?,            // --json-schema value; omitted when undefined
fast_mode_state,               // object/null
uuid
// schema-optional binary telemetry — OMIT for local headless (OD-6):
// ttft_ms? ttft_stream_ms? time_to_request_ms? time_to_request_from_spawn_ms?
// warm_spare_claimed? time_origin_ms? api_error_status? deferred_tool_use?
// terminal_reason? origin?
```

> Note the producer's actual emit order puts `is_error`/`duration_ms`/`duration_api_ms`/`num_turns` right after `subtype`; the Zod schema lists `duration_ms` first. **Follow the PRODUCER order above** (it's what serializes). `session_id` appears once in the producer block (the schema also lists it last); emit it once.

### 3.2 ERROR (binary `Rjm`)

```
type:"result", subtype:<error enum>,
duration_ms, duration_api_ms, is_error:true, num_turns,
stop_reason, total_cost_usd, usage, modelUsage, permission_denials,
errors,                        // string[]  — REPLACES result; no structured_output/telemetry
fast_mode_state, uuid, session_id
```

`subtype` enum: `error_during_execution`, `error_max_turns`, `error_max_budget_usd`, `error_max_structured_output_retries`. `errors[]` content:
- `error_max_turns`: `["Reached maximum number of turns (<N>)"]`
- `error_max_budget_usd`: `["Reached maximum budget ($<N>)"]`
- `error_max_structured_output_retries`: `["Failed to provide valid structured output after <N> attempts"]`
- `error_during_execution`: `[<ede_diagnostic prefix>, …in-memory error log slice]`; load/catch path uses `duration_ms:0, duration_api_ms:0, num_turns:0, total_cost_usd:0, usage:EMPTY_USAGE, modelUsage:{}, permission_denials:[], stop_reason:null`.

### 3.3 `usage` and `modelUsage` exact key sets

**`usage`** (top-level, **snake_case**) — the runtime Anthropic `Usage`. EMPTY_USAGE literal (binary `BE`):
```json
{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0,
 "server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},
 "service_tier":"standard",
 "cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":0},
 "inference_geo":"","iterations":[],"speed":"standard"}
```
(messages.ts default `O6l` uses `service_tier:null, inference_geo:null, iterations:null, speed:null`.) **LingXi only tracks input/output/cache_read/cache_creation** (via `emit_usage`/`CostSnapshot`); the other keys must be emitted at their zero/default literals. `service_tier`/`speed` = `"standard"` (EMPTY_USAGE path) vs `null` (messages.ts default) depends on which default is hit and the live API response (OD-7) — **use `"standard"` to match EMPTY_USAGE** unless the live API value is threaded through.

**`modelUsage`** (`record<modelId, ModelUsage>`, **camelCase** — DIFFERENT casing), binary `krr`:
```json
{"inputTokens":0,"outputTokens":0,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,
 "webSearchRequests":0,"costUSD":0,"contextWindow":200000,"maxOutputTokens":32000}
```
`{}` when empty. LingXi: key by the active model id; `costUSD` from `CostSnapshot.total_usd`, `contextWindow`/`maxOutputTokens` from the model's `context_window` caps (llm-client has these).

**`permission_denials`** (binary `Dgc`): `[{tool_name:string, tool_use_id:string, tool_input:record}]`, `[]` when none. LingXi has permission-denial tracking; thread it through.

### 3.4 LingXi field mapping for `result`

| frame field | LingXi source |
|---|---|
| `duration_ms` | `CostSnapshot.session_duration` (ms) |
| `duration_api_ms` | sum of API durations (orchestrator must surface; approximate from per-call timing or `session_duration` if unavailable) |
| `num_turns` | `CostSnapshot.api_calls` (≈turn count) |
| `result` | accumulated final assistant text (from the `assistant`-frame accumulator) |
| `stop_reason` | last `emit_end_turn(stop_reason, …)` |
| `total_cost_usd` | `CostSnapshot.total_usd` |
| `usage` | `emit_usage` cumulative → snake_case shape (zero-fill rest) |
| `modelUsage` | per-model from cost subsystem (camelCase) |
| `structured_output` | the `--json-schema` validated value (run.rs `run_structured_output`) — success path only |
| subtype selection | success unless max-turns/budget/structured-retries/terminal-error → corresponding error subtype |

---

## 4. INPUT FRAMES + `--replay-user-messages` (P3)

### 4.1 Cross-validation chain (exact error strings, in order)
Replicate in argv validation (binary order):
1. `Error: Invalid input format "<P>".` (value_parser covers).
2. `--input-format=stream-json` requires `--output-format=stream-json` → `Error: --input-format=stream-json requires output-format=stream-json.` (note: no leading `--` on right side).
3. `--input-format=stream-json` requires `--print` → `Error: --input-format=stream-json requires --print.`
4. `--sdk-url requires both --input-format=stream-json and --output-format=stream-json.` (P5 — defer).
5. `--replay-user-messages requires both --input-format=stream-json and --output-format=stream-json.`
6. `--prompt-suggestions requires --print and --output-format=stream-json (prompt_suggestion messages are only surfaced in stream-json output).`
7. `--include-partial-messages requires --print and --output-format=stream-json.` (**asymmetric**: hard error ONLY if `--include-hook-events` also set; else silently disable `includePartialMessages=false`).
8. `Error: When using --print, --output-format=stream-json requires --verbose`.

### 4.2 stdin NDJSON reader (binary `StructuredIO.read`/`processLine`)
- Read stdin as async byte/string chunks, buffer, split on `\n`, each non-empty line → `serde_json::from_str`. Trailing line w/o newline still processed at EOF. Empty lines skipped.
- **Malformed JSON line is FATAL**: `console.error("Error parsing streaming input line: <line>: <err>")` then exit 1.
- Per line, `normalizeControlMessageKeys`: top-level `requestId`→`request_id`, `response.requestId`→`response.request_id` (iOS camelCase compat; snake_case wins).
- Dispatch by `type`:
  - `keep_alive` → silently ignored.
  - `update_environment_variables` → apply allow-listed keys to env; ack with `control_response` success if `request_id` present (allowlist set not fully extracted — OD-9; at minimum `CLAUDE_CODE_OAUTH_TOKEN` triggers auth reload).
  - `user` → the primary turn; hard-check `message.role==="user"` else exit `Error: Expected message role 'user', got '<role>'`.
  - `control_request` → require `request` present else exit `Error: Missing request on control_request` (P5 handles the switch; P3 may reject-unsupported).
  - `control_response` → resolve pending CLI-originated request (P5).
  - `assistant`/`system` → seed transcript history (P3-optional).
  - `bash_command` (binary 2.1.187 only) → run headless, emit two `user` frames isReplay:true (P3-optional; tag literals `<bash-input>` not byte-confirmed — OD-10).
  - unknown `type` → `Ignoring unknown message type: <type>` (warn, drop).

### 4.3 `user` input frame shape (the turn)
```jsonc
{"type":"user","message":{"role":"user","content":<string|ContentBlock[]>},
 "parent_tool_use_id":<string|null>,   // required, nullable
 "isSynthetic"?:bool, "tool_use_result"?:unknown,
 "priority"?:"now"|"next"|"later", "timestamp"?:ISO,
 "uuid"?:UUID, "session_id"?:string}
```
Loop-level opportunistic reads (binary, newer than schema — OD-11): `shouldQuery` (gates auto-title/enqueue), `client_platform`, `inbound_origin`. Minimal valid line:
`{"type":"user","message":{"role":"user","content":"hello"},"parent_tool_use_id":null}`.

### 4.4 Multi-turn loop + dedup + replay
- Feed each `user` line's content into a sequential `run_turn` loop (orchestrator already supports sequential turns). `wt=hasReceivedInput`.
- **Dedup**: if `gt.uuid` already in session OR runtime-dup-set → skip the turn. If `--replay-user-messages` on, emit the duplicate-ack `user` frame (§2.4 with `isReplay:true`, same `uuid`); debug `Sending acknowledgment for duplicate user message: <uuid>`.
- **Replay re-emits** (only under `--replay-user-messages`): matched `control_response` (verbatim), inbound `assistant` (verbatim), duplicate-user ack. The bash_command echo/output frames are always `isReplay:true` regardless of the flag.

---

## 5. `--include-partial-messages` + `--include-hook-events` (P4)

### 5.1 `stream_event` (partial assistant) — `--include-partial-messages`
```jsonc
{"type":"stream_event","event":<RawMessageStreamEvent>,
 "parent_tool_use_id":null,"uuid":"<uuid>","session_id":"<uuid>"}
```
- `event` = the verbatim Anthropic SSE event: `message_start` | `content_block_start` | `content_block_delta` | `content_block_stop` | `message_delta` | `message_stop` — one frame per raw SSE event, in arrival order, interleaved with the digested `assistant`/`user` frames. `stream_request_start` internal events are dropped.
- The binary's internal `iUm` mapper carries an internal `ttftMs` on some paths, but the SDK schema does NOT declare it and the headless print path emits the plain 5-key form (OD-12) → **emit the plain form, no `ttftMs`**. `parent_tool_use_id` hardcoded `null` (OD-13).
- **LingXi gap G5**: `event_router` DIGESTS SSE (`emit_text`/`emit_thinking`) and discards the raw envelope. Implementing this needs a **raw-SSE tap**: a new side-channel/callback (e.g. `emit_stream_event(raw_event_json)`) fed from `event_router` BEFORE the digest, forwarding each `LlmEvent`/`RawMessageStreamEvent` as JSON. This is the only OUTPUT frame requiring orchestrator-internal changes; gate it behind the flag (default off).

### 5.2 hook lifecycle — `--include-hook-events` (NOT a `hook_event` frame!)
Three `system`-subtype frames (binary `mGn`/`RKp`/`EH`, offset 204373374). `hook_event` is a FIELD (value = event NAME like `PreToolUse`), never a `type`.
```jsonc
{"type":"system","subtype":"hook_started","hook_id","hook_name","hook_event","uuid","session_id"}
{"type":"system","subtype":"hook_progress","hook_id","hook_name","hook_event","stdout","stderr","output","uuid","session_id"}
{"type":"system","subtype":"hook_response","hook_id","hook_name","hook_event","output","stdout","stderr","exit_code"?,"outcome","uuid","session_id"}
```
- `outcome` enum: `success`|`error`|`cancelled`. `exit_code` conditionally spread (omit when undefined).
- `hook_progress`: interval `e.intervalMs??1000` (1000ms), emits only when `output` changed since last tick. Exact producing hooks (async/long-running) inferred, not exhaustively traced (OD-14).
- **Gate `pGn`**: `SessionStart` and `Setup` hook events ALWAYS stream (whitelist `AKp`); ALL others stream ONLY when `--include-hook-events`. Requires `--verbose` in print+stream-json.
- LingXi: hook subsystem exists; wire the three frames at hook start/progress/finish, gated. `hook_event_name` full set: `PreToolUse, PermissionRequest, PostToolUse, PostToolUseFailure, PermissionDenied, Notification, UserPromptSubmit, SessionStart, Setup, Stop, StopFailure, SubagentStart, SubagentStop, PreCompact, PostCompact, TeammateIdle, TaskCreated, TaskCompleted, Elicitation, ElicitationResult, ConfigChange, InstructionsLoaded, WorktreeCreate, WorktreeRemove, CwdChanged, FileChanged, SessionEnd`.

### 5.3 `prompt_suggestion` — `--prompt-suggestions` (optional, P4)
```jsonc
{"type":"prompt_suggestion","suggestion":"<string>","uuid":"<uuid>","session_id":"<uuid>"}
```
After each turn (after `result` if bg agents delay). Suppressible via `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION` falsy. Low priority.

---

## 6. CONTROL PROTOCOL (P5 — DEEP OPTIONAL, DEFERRED)

The full bidirectional SDK control protocol — `control_request`/`control_response`/`control_cancel_request`, the ~50-subtype CLI-as-server switch (`initialize`, `set_model`, `set_permission_mode`, `mcp_*`, `interrupt`, `rewind_*`, …), and the CLI-as-client `can_use_tool` permission-prompt-over-stdio (with `Promise.race(localHook, sdkRequest)`) — is a **deep, multi-file subsystem out of scope for the first pass**. It requires: a stdout control-plane writer that never overtakes data frames, a `pendingRequests` map keyed by `request_id`, the `initialize` response builder (opaque helper `iJm`; bridge stub = `{commands,agents,output_style,available_output_styles,models,account,pid,fast_mode_state?}` — OD-8), and permission integration with LingXi's permission gate. **Defer entirely.** If/when implemented, the leaked `controlSchemas.ts` is OLDER than the binary (subtype union is a subset — OD-15); the binary `print.ts` switch is authoritative.

Out-of-scope frames also include: `system/compact_boundary`, `system/status`, `system/api_retry`, `rate_limit_event`, `auth_status`, `tool_progress`, `tool_use_summary`, task_* lifecycle, `session_state_changed` (its `waiting_on_user`/`turn_starting` are CCR-tee-only and likely absent in plain `--print` — OD-5), `post_turn_summary`, `streamlined_*`, `files_persisted`, `elicitation_complete`. LingXi's `emit_compaction_completed`/`emit_rate_limit` make `compact_boundary`/`rate_limit_event` cheap follow-ups.

---

## 7. LINGXI INTEGRATION PLAN

### 7.1 The seam (reuse, don't invent)
LingXi already has the exact abstraction: the push-based `platform_api::OutputStream` trait (orchestrator.rs:761) that the orchestrator emits every turn event to. **stream-json = a fourth `OutputStream` impl** (alongside CLI `SinkAdapter`, bridge `AdapterOutputStream`, TUI `BridgeOutputStream`).

- **Install point — `lib.rs:129`**: today
  ```rust
  let sink = if parsed.is_json_output() { JsonSink::new(SessionId::new()) } else { PlainSink::new() };
  let adapter = SinkAdapter::new(sink);
  ```
  Branch here: when a NEW `parsed.is_stream_json()` is true, construct `StreamJsonStream` (a direct `OutputStream` impl, **bypassing** the `OutputSink`/`SinkAdapter` layer — that layer is lossy: it drops thinking/usage and tool_use_id) instead of `SinkAdapter(JsonSink)`. Keep `--output-format json` on the existing `JsonSink` path (LingXi's own schema) — **wait, NO**: `--output-format json` must emit the claude-code `result` envelope (§3), not LingXi's `{"event":…}` schema. So `--output-format json` (non-stream) must ALSO route to a result-envelope writer (buffer all frames, print the single `result` object). Recommended: `StreamJsonStream` handles both — in `json` mode it suppresses per-line writes and emits only the final `result`; in `stream-json` mode it writes every frame.

- **argv (`argv.rs:342`)**: split `is_json_output()` apart. Add `is_stream_json()` (true iff `output_format == Some("stream-json")`). `--output-format json` stays json-envelope. Enforce the §4.1 validation chain (esp. the `--verbose` gate). The on-disk `is_json_output()` currently collapses `stream-json`→json (the TODO) — this is the line to fix.

### 7.2 session_id / usage / cost threading
- **session_id (fix G2)**: read `runtime.orchestrator.current_session_id()` AFTER `build_runtime`, thread it into `StreamJsonStream` (constructor arg, or a one-shot setter before the turn). Stamp on EVERY frame. (Today `JsonSink` mints a fresh `SessionId::new()` — wrong for stream-json.)
- **cost**: `emit_end_turn(stop_reason, &CostSnapshot)` delivers `total_usd`/`input_tokens`/`output_tokens`/`api_calls`/`session_duration` — everything the `result` frame needs. Also `orchestrator.snapshot_cost()` for a final pull. Map per §3.4.
- **usage (per-call)**: override `emit_usage` (today `SinkAdapter` doesn't → dropped in `--json`). Accumulate into the current `assistant.message.usage` and the cumulative `result.usage`.
- **thinking**: override `emit_thinking` (today dropped) → accumulate into the assistant frame's `thinking` block.
- **init payload (G3)**: new `build_init_frame(&Runtime, session_id)` collector — tools from dispatcher/tool pool (Agent→Task rename), model + mcp from desktop runtime, slash_commands from dispatcher, version/output_style/agents/skills/plugins/betas/apiKeySource from runtime+auth.

### 7.3 Print-path changes (`run.rs`)
- `run_oneshot` (run.rs:25) currently brackets the turn with `sink.turn_start()` and lets text stream through the installed `output`. For stream-json: emit `system/init` BEFORE `run_turn`, let `StreamJsonStream` accumulate+flush `assistant`/`user` frames DURING `run_turn`, then emit the terminal `result` frame AFTER `run_turn` returns (assembled from `snapshot_cost()` + accumulated final text + stop_reason→subtype). The `--json-schema` path (`run_structured_output`, run.rs:68) maps its validated value into `result.structured_output`.
- **structured guard**: claude-code runs `installStreamJsonStdoutGuard()` first (diverts stray stdout→stderr so only frames hit stdout). LingXi should similarly ensure no stray `println!` pollutes the NDJSON stream (route logs to stderr).

### 7.4 What does NOT exist / new code
- New `StreamJsonStream` `OutputStream` impl (in `apps/cli` or a small `stream-json` crate).
- New frame structs (do NOT reuse `client_protocol::ClientEvent` — tags differ: `text_delta` vs `assistant`, no `system/init`/`result`; it's the structural template only).
- New `build_init_frame` collector.
- New stdin NDJSON reader + multi-turn loop (P3).
- New raw-SSE tap in `event_router` (P4, behind flag).
- New `emit_message_boundary()` trait method (recommended, P1 — see OD-1).

---

## 8. PHASED IMPLEMENTATION PLAN (each phase independently testable)

**P1 — OUTPUT core (init + assistant + user).** Add `is_stream_json()` + `--verbose` gate + the `When using --print … requires --verbose` error. New `StreamJsonStream` impl: NDJSON writer (compact + U+2028/29 escape + LF), real session_id, `system/init` collector, assistant-frame accumulator (text/thinking/tool_use blocks) with message-boundary flush (add `emit_message_boundary()`), tool_result→`user` frame. NO result frame yet. Test: golden NDJSON of init+assistant+user lines for a canned turn (mock LLM), byte-compared field-by-field.

**P2 — RESULT envelope (+ `--output-format json`).** Assemble success/error `result` from `CostSnapshot` + accumulated text + stop_reason→subtype; `usage`/`modelUsage` exact key sets; `permission_denials`; `structured_output` from `--json-schema`. Wire exit code = `is_error?1:0`. Route `--output-format json` (non-stream) to emit only the single `result` object (and `--verbose` → array). Test: golden success + each of 4 error subtypes; assert `--output-format json` byte-equals the final stream-json `result` line.

**P3 — INPUT (`--input-format stream-json`).** Validation chain (§4.1). stdin line reader (split/parse/normalizeControlMessageKeys, fatal-on-malformed, role check). Multi-turn `run_turn` loop with uuid dedup. `--replay-user-messages` ack frames (`isReplay:true`). `keep_alive`/`update_environment_variables`/unknown-type handling. (control_request → reject-unsupported for now.) Test: feed a 3-line NDJSON stdin script → assert turns run in order + dedup skips + replay acks; assert exact error strings on malformed/bad-role.

**P4 — PARTIAL + HOOKS.** `--include-partial-messages`: raw-SSE tap in event_router (new gated `emit_stream_event`) → `stream_event` frames (plain 5-key, interleaved). `--include-hook-events`: three `system/hook_*` frames + `pGn` gate (SessionStart/Setup always; rest flag-gated). Optional `--prompt-suggestion`. Test: golden interleaved stream with partials on; hook frames with/without the flag; assert SessionStart streams without flag, PreToolUse only with flag.

**P5 — CONTROL protocol (DEFERRED, optional).** Full bidirectional control plane + `can_use_tool` permission-over-stdio + `initialize` handshake. Out of scope for the first byte-faithful pass.

---

## 9. BYTE-FAITHFULNESS TEST STRATEGY

1. **Golden NDJSON fixtures.** For each frame type, a checked-in expected line. Compare structurally (parse both, assert key-set + key-ORDER + values) AND, for order-sensitive correctness, assert the serialized string's key sequence matches (serde struct field order = declared order). Mask volatile fields (`uuid`, `session_id`, `duration_ms`, timestamps) before compare.
2. **Live oracle capture (resolves most ODs).** The extraction could NOT run the binary (no creds/sandbox), so several shapes (conditional-spread presence, optional-telemetry serialization, `service_tier` literal, init `apiKeySource` enum) are from minified builders, not a captured transcript. **Before/early in P1**, run the real binary once: `claude -p --output-format stream-json --verbose "hi"` (and `--include-partial-messages`, `--include-hook-events`, `--input-format stream-json` variants) and diff LingXi's output against the captured NDJSON. This is the single highest-value verification step — it converts the OD list from "inferred" to "confirmed."
3. **Cross-format invariant.** Assert `--output-format json` stdout (no `--verbose`) byte-equals (after uuid/timing masking) the final `result` line of a `--output-format stream-json --verbose` run on the same input. The binary guarantees this (same `lastMessage`).
4. **Round-trip / schema validation.** Validate every emitted frame against the binary Zod schema shapes (port the key schemas as Rust assertions, or validate the captured oracle output AND the LingXi output against the same JSON Schema).
5. **Input fuzz/error-string tests.** Feed malformed lines, bad `message.role`, missing `control_request.request`, unknown types → assert EXACT byte-for-byte error strings + exit codes from §4.1/§4.2.
6. **No-stray-stdout test.** Run a turn that internally `println!`s/logs and assert stdout contains ONLY valid NDJSON frames (the stdout guard).

---

## 10. OPEN DECISIONS / UNCERTAINTIES (carry forward; most resolved by §9.2 live capture)

- **OD-1 (design fork)**: assistant-frame flush boundary — add `emit_message_boundary()` trait method (recommended, parity-faithful) vs flush only on tool_result/end_turn (lossy for multi-message turns). **Needs a human call** (touches the shared trait).
- **OD-2 (design fork)**: does `--output-format json` route through `StreamJsonStream` (suppress lines, emit final result) or a separate buffering writer? Recommended: same impl, mode flag.
- OD-3: exact `apiKeySource` enum value set (not exhaustively extracted) — capture from binary.
- OD-4: are `analytics_disabled`/`product_feedback_disabled` strictly always-present (binary shows plain non-spread props → yes) vs conditional — confirm across callers.
- OD-5: `session_state_changed.waiting_on_user`/`turn_starting` are CCR-tee-only; likely ABSENT in plain `--print` — confirm (out of scope regardless).
- OD-6: optional result telemetry (`ttft_ms`, `time_to_request_ms`, `warm_spare_claimed`, `api_error_status`, `deferred_tool_use`, `terminal_reason`, `origin`) — schema-optional, producer doesn't emit in headless; **omit** unless live capture shows otherwise.
- OD-7: `usage.service_tier`/`speed` = `"standard"` (EMPTY_USAGE) vs `null` (messages.ts default) — live API value is truth; **default to `"standard"`**.
- OD-8: full `initialize` control-response payload builder (`iJm`) not extracted (P5 only).
- OD-9: `update_environment_variables` allowlist set (only `CLAUDE_CODE_OAUTH_TOKEN` confirmed) — extract for P3.
- OD-10: bash_command echo wrapper tag literals (`<bash-input>` vs `<command-name>`) not byte-resolved (P3-optional).
- OD-11: `shouldQuery`/`client_platform`/`inbound_origin` are loop-level opportunistic reads, not in the strict schema — treat as optional reads.
- OD-12: `stream_event.ttftMs` internal-only; headless emits plain 5-key form — **omit ttftMs**.
- OD-13: `stream_event.parent_tool_use_id` hardcoded `null` in headless — emit `null`.
- OD-14: exact set of hooks producing `hook_progress` ticks not exhaustively traced.
- OD-15: leaked `controlSchemas.ts` older than binary; binary `print.ts` switch is authoritative (P5).
- **OD-16 (blocking-ish)**: this lands on `main` (which has `715adc4e`/`--output-format`), NOT the on-disk `fix/android-desktop-divergence` branch. Confirm the target branch and rebase onto `main` first, or re-add the output-format argv flags. Assumed the orchestrator/`OutputStream` surface is identical on both (it predates the divergence) — verify.
