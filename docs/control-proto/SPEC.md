# IMPLEMENTATION SPEC — stream-json bidirectional CONTROL protocol (P5) in LingXi

**Status:** P1–P4 (output frames, input frames, key-normalization, `request`-field
validation) shipped. This is **P5**: the bidirectional control plane.

**Oracle binary:** `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude` (v2.1.187, GIT_SHA `6a53320fad…`).
**Readable TS (OLDER subset — structure-only, never authoritative on byte detail):** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/` (`cli/print.ts`, `cli/structuredIO.ts`, `entrypoints/sdk/controlSchemas.ts`).
**LingXi target:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/apps/cli/` + `engine-desktop` + `orchestrator` + `traits`.

**Authority rule (carry through the whole port):** the binary is canonical on every
field set, literal, and tag (e.g. `decideLocation:"ask-path"`, the `mYm`/`fYm` Sets,
the `Unsupported control request subtype:` error string). The leaked TS lacks
`end_session` and 17 of the binary's 48 server subtypes — it is an *unknown-older*
snapshot, NOT 2.1.186, so "187-only" cannot be asserted at single-version
granularity. Treat all 48 binary server subtypes as the target set; treat the leaked
TS only as a structural map.

---

## 1. Wire envelope, pendingRequests pairing, stdout ordering

### 1.1 The three control `type` literals (+ two adjacent stream-control literals)

```json
// control_request — correlation token = request_id; request discriminated by request.subtype
{ "type":"control_request", "request_id":"<uuid>", "request":{ "subtype":"<name>", ...fields } }

// control_response — SUCCESS (note the DOUBLE nesting: outer response, inner response payload)
{ "type":"control_response", "response":{ "subtype":"success", "request_id":"<uuid>", "response":{...}? } }

// control_response — ERROR
{ "type":"control_response", "response":{ "subtype":"error", "request_id":"<uuid>", "error":"<msg>",
   "pending_permission_requests":[<control_request>...]?, "pending_user_dialog_requests":[...]? } }

// control_cancel_request — OWN top-level type, no response expected
{ "type":"control_cancel_request", "request_id":"<uuid>" }
```
Adjacent stream-control literals on the same NDJSON stream: `keep_alive` (bidirectional,
silently dropped on read; the CLI EMITS it every 30s during long `stage_file`/`add_directory`),
`update_environment_variables` (mutates env on read). These already have arms in
`stream_json_input.rs:141-157`.

**Double-nesting is load-bearing.** Success payload is at `message.response.response`;
error string at `message.response.error`. The Rust resolver MUST read the inner
`response.request_id` as the join key and the inner `response.response` as the payload.

**Builders (byte-exact, print.ts switch prelude, OFF~210466000):**
```js
rn = (wt,xn) => I.enqueue({type:"control_response",response:{subtype:"success",request_id:wt.request_id,response:xn}});
Dn = (wt,xn) => I.enqueue({type:"control_response",response:{subtype:"error",  request_id:wt.request_id,error:xn}});
```
`rn(gt)` with no `xn` ⇒ `response:undefined` (omitted by JSON serializer). Rust: emit
`response` key absent when payload is `None` (use `serde_json` skip-if-none, NOT `null`).

### 1.2 `normalizeControlMessageKeys` — ALREADY DONE (reuse)

`stream_json_input.rs:74-89` `normalize_control_message_keys` byte-mirrors the binary
`IYn` (OFF 206174349): rename top-level `requestId`→`request_id` and nested
`response.requestId`→`response.request_id`, snake_case wins. Applied first on every line.
No change needed.

### 1.3 pendingRequests pairing (the OUTBOUND-request side — net-new in LingXi)

Mirror `StructuredIO.pendingRequests: Map<request_id, {resolve,reject,schema,request}>`.

**Send path (`sendRequest`, structuredIO.ts:469, binary OFF 210362272):** build
`{type:"control_request",request_id,request}`, `outbound.enqueue(message)`, then
register the pending entry and return a future. `finally { pendingRequests.delete(id) }`
always runs. When `request.subtype==="can_use_tool"` AND `onControlRequestSent` is set,
fire it (bridge forward — out of scope for stdio-local port; leave a no-op hook seam).

**Receive path (`processLine`, structuredIO.ts:362 — the ONLY `control_response` sink):**
1. `normalize` (done).
2. `req = pendingRequests.get(message.response.request_id)`.
3. `!req` ⇒ orphan/duplicate handling (§1.5).
4. else: `trackResolvedToolUseId(req.request)`, `pendingRequests.delete(id)`.
   - if `req.request.request.subtype==="can_use_tool"` ⇒ fire `onControlRequestResolved(id)` (bridge cancel; no-op seam locally).
   - if `message.response.subtype==="error"` ⇒ `req.reject(Error(message.response.error))`.
   - else payload `= message.response.response`; if `req.schema` ⇒ resolve `schema.parse(payload)` (reject on parse failure); else resolve `{}`.

Pairing is **pure `request_id` string equality**. `request_id` is an opaque
`crypto.randomUUID()` (v4) string; no format constraint; echoed byte-for-byte.

**Rust shape:**
```rust
struct PendingControlRequest { responder: oneshot::Sender<Result<Value, String>>, request: Value /* the control_request, for tracking */ }
// PendingControlRequests = Arc<Mutex<HashMap<String, PendingControlRequest>>>
```
`sendRequest`: mint uuid, push frame on the outbound queue, insert the oneshot, await it.
`control_response` arm: look up + remove by inner `request_id`, send the result, drop the
guard. Schema validation: since LingXi only ORIGINATES `can_use_tool` (and later
`elicitation`/`hook_callback`/`mcp_message`), the "schema" is per-subtype Rust
deserialization of the payload at the call site (decode `PermissionToolOutput`), not a
generic zod port.

### 1.4 control_cancel_request semantics (own type)

Two roles, do NOT conflate with `interrupt`:
- **OUTBOUND-emit (LingXi → host):** when LingXi's own pending request is aborted (turn
  cancelled / superseded), enqueue `{type:"control_cancel_request",request_id}` AND
  locally reject the pending entry immediately with an abort error (no ack awaited).
  Binary: `sendRequest` abort handler (OFF 210357994/210362272) and
  `injectControlResponse`.
- **INBOUND-receive (host → LingXi):** the host can cancel a `control_request` IT sent
  (rare for stdio data direction; the print.ts server gives each in-flight server
  request its own `AbortController` keyed by `request_id`, and a
  `handleControlCancelRequest` aborts the matching one — duplicate-id delivery skipped
  with a debug log). For the first-pass port the inbound server switch is mostly
  synchronous; wire a `request_id→AbortHandle/CancellationToken` map only when an async
  handler (e.g. `mcp_call`, `stage_file`) is implemented.

`control_cancel_request` carries `session_id` ONLY over the websocket/remote transport
(RemoteSessionManager, OFF 207622342) — irrelevant to the stdio-local port.

### 1.5 stdout ordering — single FIFO, single writer (the design lock)

**Control plane NEVER overtakes the data plane.** TS lock (structuredIO.ts:160-162):
one `outbound = Stream<StdoutMessage>`, the drain loop is the **only** writer; every
producer (turn output callbacks + `sendRequest` + `sendControlResponseSuccess/Error` +
`control_cancel_request`) `enqueue`s into the SAME queue; a single consumer serializes
to stdout in strict insertion order. No priority lanes.

**LingXi gap (integration map §a):** `StreamJsonStream` owns `out: Arc<Mutex<Stdout>>`
(`stream_json.rs:180`). Each *line* is atomic, but two tasks racing the lock can
interleave at line granularity → a control frame CAN overtake a queued data frame.
**Required refactor:** replace `Arc<Mutex<Stdout>>` with a single writer task fed by an
`mpsc` channel (the `outbound` queue). `StreamJsonStream` and a new
`ControlPlaneWriter` both become producers that push `Value`s onto that channel; one
drain task calls `emit_line` (the existing `escape_line_terminators` + `\n` framing,
`stream_json.rs:33-44`, stays verbatim). This is the 1:1 of `Stream<StdoutMessage>` +
single drain loop.

**Caveats to encode in comments (NOT to "fix"):**
- Strict no-overtake is a **stdio-local property only**. Over websocket/remote
  transports control frames carry `session_id` and CAN be reordered by
  transport/reconnect; the duplicate-response dedup exists precisely because reconnects
  redeliver out of band. The stdio port gets strict FIFO for free from the single
  channel; do not attempt end-to-end remote ordering.
- **Orphan/late `control_response` does NOT go through the main loop** — it is
  resolved/dropped inline in the receive path; it only `yield`s to print.ts's loop when
  `replayUserMessages` is set.
- **Duplicate-response dedup:** `resolvedToolUseIds: Set<string>` (cap
  `MAX_RESOLVED_TOOL_USE_IDS=1000`, oldest-evicted). When a `control_response` arrives
  for a `request_id` not in `pendingRequests` but whose payload `toolUseID` is in
  `resolvedToolUseIds`, log+drop:
  `"Ignoring duplicate control_response for already-resolved toolUseID=… request_id=…"`
  (OFF 192947721/210359952). Prevents websocket-reconnect double-deliveries from
  pushing duplicate assistant messages (API 400 on non-unique tool_use ids). Port this
  Set + the log line; for stdio it is mostly inert but is byte-faithful and cheap.

---

## 2. CLI-AS-SERVER control_request subtype switch — COMPLETE (48 server subtypes)

The switch fires when an inbound frame is `type:"control_request"` and dispatches on
`gt.request.subtype` via a long `if/else if` chain (binary switch body OFF
210472932…210497289), ending in the fallthrough:
```js
else Dn(gt, `Unsupported control request subtype: ${gt.request.subtype}`); continue;
```
Port the literal `"Unsupported control request subtype: "` exactly — this REPLACES the
current reject stub at `stream_json_input.rs:159-168`
(`"control_request received but full control protocol is not yet implemented (P5 deferred)"`).

`request`-field-missing error (`"Error: Missing request on control_request"`,
`stream_json_input.rs:161-163`) already matches the binary — keep it.

### 2.1 Tractability legend

- **[T] tractable-first-pass** — maps onto an existing LingXi seam or is a small
  pure/registry handler. Ship in Phase 2–4.
- **[D] deep** — needs a subsystem LingXi doesn't have (MCP lifecycle, OAuth flows,
  remote-control bridge, file staging, ultrareview). Reply `Unsupported control request
  subtype` (the binary fallthrough) until that subsystem lands — this is BYTE-FAITHFUL
  fallback behavior for a host that asks for an absent capability, not a stub hack.

### 2.2 The full enumeration (binary order; req → resp shape; tractability)

| # | subtype | request fields | success response | tract |
|---|---|---|---|---|
| 1 | `interrupt` | `{}` | empty | **T** — `cancel.cancel()` on the per-turn token, then `rn(gt)`. Maps to `run_turn_streaming_with_cancel` (§5.4). |
| 2 | `end_session` | `{reason?}` | empty | **T** — log `[print.ts] end_session received, reason=…`, abort turn, reply success, then **break the loop** (drain+exit). Stale-archived-on-epoch>1 path: ignore+ack+continue (`eTc(reason, CLAUDE_CODE_WORKER_EPOCH)`); epoch gate may be stubbed to "not stale" first pass. |
| 3 | `initialize` | see §4.1 | §4 payload | **T (core) / D (merge sub-protocol)** — build the §4 registry payload from LingXi sources; the hooks/agents/systemPrompt MERGE-from-stdin (createHookCallback per matcher) is **deep** — first pass ACK the registry payload and ignore inbound hooks/agents (record them but don't re-arm callbacks). |
| 4 | `set_permission_mode` | `{mode, ultraplan?}` mode∈default/plan/acceptEdits/bypassPermissions/auto | `{mode}` | **T (needs net-new mutation seam §5.6)** — errors: bypassPermissions disabled-by-settings / not-launched-with-flag; auto-not-allowed. |
| 5 | `set_model` | `{model?}` ("default"→`Kg()`) | empty | **T** — `orchestrator.switch_model(model,None)` (`handle_impl.rs:106`); resolve `"default"` to LingXi default first. Error: model-unavailable `fte(model,fallback)`. |
| 6 | `set_max_thinking_tokens` | `{max_thinking_tokens, thinking_display?}` | empty | **T** — set session max-thinking + display flag. |
| 7 | `mcp_status` | `{}` | `{mcpServers: nn()}` | **T** — `orchestrator.list_mcp_servers()` (`handle_impl.rs:140`). |
| 8 | `get_binary_version` | `{}` | `{version, buildTime}` | **T** — inline LingXi version + build time literals. |
| 9 | `get_context_usage` | `{}` | `{...contextUsage}` | **T** — token-budget breakdown; error `Ce(err)`. |
| 10 | `get_session_cost` | `{}` | `{text}` | **T** — `snapshot_cost()` → formatted string. |
| 11 | `get_usage` | `{}` | `{...usage}` | **T** — usage snapshot; error on throw. |
| 12 | `mcp_message` | `{server_name, message}` | empty | **D** — forwards JSONRPC to a connected SDK-MCP server's `transport.onmessage`. Needs SDK-MCP transport registry. |
| 13 | `rewind_files` | `{user_message_id, dry_run?}` | `{canRewind, filesChanged?, insertions?, deletions?}` | **D** — file-checkpoint subsystem; error strings: "File rewinding is not enabled.", "No file checkpoint found…", "Failed to rewind: …". |
| 14 | `cancel_async_message` | `{message_uuid}` | `{cancelled}` | **T (partial)** — needs a queued-message registry; first pass `{cancelled:false}`. |
| 15 | `rewind_conversation` | `{target_message_uuid}` | success-shaped `{rewound,prefillText,precedingAssistantUuid,error?}` | **D** — history splice; embeds outcome in SUCCESS frame (never uses `Dn`). errors∈"turn running"/"target not found"/"no preceding assistant"/"failed to persist rewind anchor"/"state changed"/"commands queued". |
| 16 | `read_file` | `{path, max_bytes?, encoding?}` | readFileForRemote result | **T (partial)** — read via tool-permission context; error `Ce(err)`. |
| 17 | `stage_file` | `{...}` (mount_path-based) | stage result `{ok,...}` | **D** — lazy `stageFile`; keep_alive every 30s; full success field list NOT byte-dumped beyond `{ok,error}`. |
| 18 | `register_repo_root` | `{directory, reload_claude_md?, reload_skills?, reload_plugins?}` | `{directory: <realpath>}` | **D** — subdir-of-cwd check; addDirectories context; throws `register_repo_root: ${dir} is not a subdirectory of cwd`. |
| 19 | `add_directory` | `{mount_path}` | `{staged_path, directory}` | **D** — keep_alive 30s; requires env `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`; error strings recorded. |
| 20 | `file_suggestions` | `{query}` | `{suggestions:[{path}]}` | **T (partial)** — file-index search; if no index, reply error `"file_suggestions is not supported in this context"`. |
| 21 | `seed_read_state` | `{path, mtime}` | empty | **T** — stat; if mtime≤given, read (strip BOM, CRLF→LF), seed read-state cache. Errors swallowed. |
| 22 | `mcp_set_servers` | `{servers}` | apply/diff response `Xt` | **D** — MCP config apply (`authoritative:true`). |
| 23 | `reload_plugins` | `{}` | `{commands,agents,plugins,mcpServers,error_count}` | **D** — plugin reload subsystem. |
| 24 | `reload_skills` | `{}` | `{skills:[{name,description,argumentHint,aliases?}]}` | **T (partial)** — reload skills+commands from dispatcher registry. |
| 25 | `mcp_reconnect` | `{serverName}` | empty | **D** — reconnect; errors "Server not found", managed-policy-blocked, connection-failed. |
| 26 | `mcp_call` | `{tool, arguments?}` | `{content, structuredContent, _meta}` | **D** — FQN parse + invoke non-SDK server; 5 distinct error strings recorded (§server-switch #26). |
| 27 | `mcp_toggle` | `{serverName, enabled}` | empty | **D** — disconnect/reconnect; same error family as mcp_reconnect. |
| 28 | `set_mcp_permission_mode_override` | `{serverName, mode}` (tighten-only: default/auto/null) | empty OR `{warning}` | **D** — `mcpPermissionModeOverrides`; tighten-only validation; warnings for unknown server. |
| 29 | `channel_enable` | `{serverName}` | `{response: undefined}` | **D** — marketplace-plugin server channel handler. |
| 30 | `mcp_authenticate` | `{serverName, redirectUri?}` | `{authUrl?, requiresUserAction, callbackExpected, redirectScheme?, state?, callbackPort?}` | **D** — OAuth/PKCE flow. |
| 31 | `mcp_oauth_callback_url` | `{serverName, callbackUrl}` | empty | **D** — delivers redirect URL to waiting flow. |
| 32 | `claude_authenticate` | `{loginWithClaudeAi?}` | `{manualUrl, automaticUrl}` | **D** — single-slot Anthropic OAuth. |
| 33 | `claude_oauth_callback` | `{authorizationCode, state}` | `{account:{...}}` | **D** — feeds manual code (shares branch w/ #34). |
| 34 | `claude_oauth_wait_for_completion` | `{}` | `{account:{...}}` | **D** — awaits in-flight flow. |
| 35 | `mcp_clear_auth` | `{serverName}` | `{}` | **D** — clears creds + reconnect (sse/http only). |
| 36 | `apply_flag_settings` | `{settings}` (agent/model/effortLevel/ultracode/viewMode; null deletes) | empty | **T (partial)** — model/effort tractable via session; agent-switch + systemPrompt is **D**. error `agentResult.error`. |
| 37 | `get_settings` | `{}` | `{...settings, applied:{model,effort,ultracode}, errors?}` | **T (partial)** — effective settings snapshot. |
| 38 | `stop_task` | `{task_id}` | `{}` | **T** — `Aqt(task_id, taskRegistry)`; not_found/not_running treated as success `{}`. |
| 39 | `background_tasks` | `{tool_use_id?}` | `{backgrounded}` or `{}` | **T (partial)** — needs background-task seam. |
| 40 | `generate_session_title` | `{description, persist?}` | `{title}` | **T (partial)** — async title gen; first pass can synthesize. |
| 41 | `rename_session` | `{title}` | empty | **T** — trim+persist/in-memory rename; error `"title must be non-empty"`. |
| 42 | `submit_feedback` | `{description, surface?}` ("sdk" default) | `{feedback_id…}` variants | **D** — feedback submission backend. |
| 43 | `side_question` | `{question}` | `{response, synthetic}` | **D** — async side-question against cache-safe params. |
| 44 | `ultrareview_launch` | `{args?, confirm?}` | launch result `{status,message,...}` | **D** — injects `<command-name>/ultrareview…` user messages. |
| 45 | `message_rated` | `{messageUuid, sentiment, surface?, cleared?}` | `{}` | **T** — telemetry-only (gated); reply `{}`. |
| 46 | `remote_control` | `{enabled, name?}` | enable: `{session_url,connect_url,environment_id}`; disable: empty | **D** — REPL bridge (`initReplBridge`). Out of single-process stdio scope. |

(That is the 46 server-handled subtypes in the binary `if/else if`; `can_use_tool` and
`request_user_dialog` appear as guards at the top of the chain but are the
CLIENT→SERVER permission/dialog frames resolved on the pending-request path, NOT in this
server switch — they bring the total literal count the doc cites to ~48.)

**Subtype-count for this spec's switch:** **46** server-handled `control_request`
subtypes (the count the Rust dispatcher's match arms must cover, including the deep ones
that initially fall through to `Unsupported control request subtype`).

**Payload-field caveat (carry forward):** field lists for the 17 newer subtypes were
read from how the handlers *destructure* `gt.request` (authoritative for names) but NOT
from a standalone zod schema — optional/required typing for
`stage_file`/`add_directory`/`mcp_call` bodies is inferred. `register_repo_root` returns
`{directory}` and `add_directory` returns `{staged_path,directory}` (read from the
Io/Er handler bodies); the full `stageFile` success object beyond `{ok,error}` was NOT
byte-dumped. When implementing a [D] subtype, re-extract its exact body from the binary
before relying on this table.

---

## 3. can_use_tool — CLI-as-CLIENT permission-over-stdio + LingXi gate integration

### 3.1 The flow (CLI ORIGINATES the request; host answers)

`can_use_tool` is the ONE permission subtype LingXi must EMIT. The reference is
`StructuredIO.createCanUseTool` (structuredIO.ts:533). The crucial property: the
**local permission machinery runs FIRST**; only an `ask` escalates over stdio.

```
local hasPermissionsToUseTool(...) → allow/deny ⇒ RETURN immediately. NO control_request sent.
                                   → 'ask'      ⇒ race:
  localHook = executePermissionRequestHooksForSDK(...)   // background; never rejects; resolves undefined if no decision
  sdkReq    = sendRequest({subtype:'can_use_tool', ...}, schema, hookAbort.signal, requestId)
  winner = Promise.race([localHook, sdkReq])
    hook decided   ⇒ abort sdkReq (→ control_cancel_request), return hook decision
    hook no-decision ⇒ await sdkReq, map result
    sdk first      ⇒ return mapped sdk result; running hook ignored
  abort/interrupt (turn token) ⇒ hookAbort → sdkReq cancels (control_cancel_request) → reject AbortError
                               ⇒ caught → {behavior:'deny', message:'Tool permission request failed: …'}
```
There is **NO timeout** on the `can_use_tool` SDK request (contrast: `request_user_dialog`
has `CLAUDE_CODE_USER_DIALOG_TIMEOUT_MS ?? 300000`). It blocks until a `control_response`,
a hook decision, or a turn abort.

### 3.2 Outbound request shape (AUTHORITATIVE — binary `Bgc`, OFF 210324747)

```jsonc
{ "type":"control_request", "request_id":"<uuid>",
  "request":{
    "subtype":"can_use_tool",
    "tool_name": "<string>",                 // required
    "input": { ... },                        // required (record)
    "tool_use_id": "<string>",               // REQUIRED (not optional)
    "permission_suggestions": [PermissionUpdate]?,  // .optional()
    "blocked_path": "<string>"?,             // .optional()
    "decision_reason": "<serialized text>"?, // .optional()
    "decision_reason_type": "<enum TSr>"?,   // .optional() — machine-readable discriminator
    "classifier_approvable": <bool>?,        // .optional()
    "title": "<string>"?,                    // from MCP _meta anthropic/permissionDisplay
    "display_name": "<string>"?,
    "description": "<string>"?,
    "agent_id": "<string>"?                  // subagent routing
  } }
```
The CLI-side `createCanUseTool` builder populates `tool_name, input,
permission_suggestions, blocked_path, decision_reason, tool_use_id, agent_id`. The
`title/display_name/description/decision_reason_type/classifier_approvable` come from
*other* emit paths (bridge / MCP permissionDisplay); the schema is the superset the host
accepts. **For LingXi's first pass emit `tool_name, input, tool_use_id` (+ `agent_id`
when a worker is present); leave the rest absent.** Exact JSON key set of the outbound
request was confirmed from `Bgc`; key set was NOT independently byte-dumped from the
LingXi-relevant builder beyond the structuredIO:590 site — fields above are the schema
superset.

### 3.3 Inbound response payload — `PermissionToolOutput` (binary `hYm`/`gYm`/`ljt`, OFF ~210351900)

```jsonc
// ALLOW
{ "behavior":"allow",
  "updatedInput": { ... },                 // REQUIRED; {} ⇒ "use original input"
  "updatedPermissions": [PermissionUpdate]?, // optional; malformed → dropped+logged, not rejected
  "toolUseID": "<string>"?,                 // for orphan/dup dedup
  "decisionClassification": "user_temporary"|"user_permanent"|"user_reject"? }  // .catch(undefined)

// DENY
{ "behavior":"deny",
  "message": "<rejection text>",            // REQUIRED — surfaced to the model in tool_result
  "interrupt": <bool>?,                     // true ⇒ abort the WHOLE turn (not just this tool)
  "toolUseID": "<string>"?,
  "decisionClassification": ...? }
```
`behavior` ∈ `allow|deny` ONLY (no `ask` on the response). Wire field names confirmed
from the schema; the exact JSON key set the decider must parse was NOT independently
byte-dumped beyond the structuredIO:590/737 + print.ts:4152-4176/4240 usage sites — the
schema (above) is authoritative for names.

### 3.4 Response → decision normalization (binary `Jyt`, OFF 210350900)

```js
decisionReason = {type:"permissionPromptTool", permissionPromptToolName: tool.name, toolResult: result};
if behavior==="allow":
   if updatedPermissions: ctx.setToolPermissionContext(applyUpdates); persistPermissionUpdates(updatedPermissions);
   updatedInput = Object.keys(result.updatedInput).length>0 ? result.updatedInput : input;  // {} → original
   return {...result, updatedInput, decisionReason};
else if behavior==="deny" && interrupt:
   log(`SDK permission prompt deny+interrupt: tool=… message=…`); ctx.abortController.abort();  // interrupt turn
return {...result, decisionReason, decideLocation:"ask-path"};   // NB: binary ADDS decideLocation:"ask-path"
```
Port `decideLocation:"ask-path"` (binary-only; leaked TS lacks it). `deny+interrupt`
MUST cancel the turn token (§5.4).

### 3.5 LingXi gate integration — Promise.race(local-vs-SDK) onto the gate trait

**Seam (integration map §b):** add a NEW inner-transport `PermissionGate` impl —
`StdioControlPermissionGate` — substituted for
`NoOpPermissionGate`/`DenyOnAskGate`/`AdapterPermissionGate` at
`engine-desktop/src/lib.rs:2539-2550`. Keep `PolicyPermissionGate` as the OUTER wrapper:
it resolves rule/mode allow+deny LOCALLY first (= TS `hasPermissionsToUseTool`), and only
an unresolved `Ask` delegates to the inner transport — exactly the §3.1 short-circuit.
This makes the OUTER policy gate the "local pre-gate" and the inner stdio decider the
"SDK request"; the structure already matches the TS by construction.

The trait surface to implement (`traits/src/permission_gate.rs:116`):
`check` / `check_with_worker` (threads `PromptWorker` → maps to `agent_id`) /
`resolve_detailed` / `check_in_plan_mode` / `check_after_hook_allow`. The turn loop
already calls `resolve_detailed` (turn_loop.rs:2329) and `check` (2427).
`StdioControlPermissionGate::check`:
1. mint `request_id` + `tool_use_id`; build the §3.2 request.
2. `control_plane.send_request(req)` → register pending → await the oneshot.
3. on response: decode `PermissionToolOutput`; `allow ⇒ PermissionDecision::Allow`
   (apply `updatedInput` if non-empty — needs a decision variant that can rewrite input;
   if LingXi's `PermissionDecision` cannot carry `updatedInput`, first pass = allow with
   ORIGINAL input and note the gap); `deny ⇒ Deny{reason:message}`; `deny+interrupt ⇒`
   also cancel the turn token.
4. on error/abort: `Deny{reason:"Tool permission request failed: …"}`.

**The local PermissionRequest-hook race (`executePermissionRequestHooksForSDK`)** is the
`Promise.race` second arm. LingXi's `resolve_detailed` already fires
`PermissionRequest`/`PermissionDenied` source-gated hooks. First pass: run the stdio
request as the sole inner-transport decider (the OUTER policy gate already did the
allow/deny pre-check). The hook-VS-SDK race (hook abort → `control_cancel_request`) is a
**second-tier refinement** — implement after the basic round-trip works; it requires
spawning the hook future + the sdk future and racing, with the hook winner aborting the
pending control request. Mark deep.

**LINGXI_ENFORCE_PERMISSIONS uncertainty:** default-on per the code comment
(engine-desktop:2784-2808 wraps the inner transport in `PolicyPermissionGate` when
enforcement is on). If a run uses `use_noop_permission_gate` WITHOUT the policy wrap, the
`can_use_tool` decider becomes the SOLE gate (it must then also handle plain allow/deny,
not just ask). Confirm the live cfg for the stream-json path before assuming the outer
policy pre-check is always present.

### 3.6 sandbox-ask piggyback (do NOT add a new subtype)

`createSandboxAskCallback` reuses `can_use_tool` with `tool_name:
SANDBOX_NETWORK_ACCESS_TOOL_NAME`, `input:{host}`, `description:"Allow network
connection to <host>?"`, fresh `tool_use_id`; returns `result.behavior==='allow'`. If
LingXi has a sandbox-network-ask seam, route it through the same `send_request` path.

---

## 4. initialize handshake response payload

### 4.1 Request (SDK → CLI) — `{subtype:"initialize", …}` (binary schema OFF 210318607)

Fields: `hooks?` (Record<event,[{matcher?,hookCallbackIds:[string],timeout?}]>),
`sdkMcpServers?:string[]` (names only), `jsonSchema?`, `systemPrompt?:string[]`,
`appendSystemPrompt?`, `planModeInstructions?`, `appendSubagentSystemPrompt?`,
`toolAliases?`, `excludeDynamicSections?`, `agents?:Record<name,AgentDefinition>`,
`title?`, `skills?:string[]`, `webSearchIsolationExemptMcpServers?`,
`promptSuggestions?`, `agentProgressSummaries?`, `forwardSubagentText?`,
`supportedDialogKinds?`. The CLI re-inflates `hookCallbackIds`→live callbacks
(`createHookCallback`) and applies `jsonSchema` BEFORE building the response — this MERGE
sub-protocol is **deep**; first pass record-and-ignore (ACK the registry payload).
(Hook-event enum `czm` captured through `InstructionsLoaded` (28 events) + a truncated
tail — re-extract the full list before porting hook-event validation. Request-side
`AgentDefinition` `kgc` tail (skills/model/prompt) is truncated — response-side `D$o`
agent element IS fully captured.)

### 4.2 Response payload (CLI → SDK) — built by `zyc()`, validated by `k3E` (OFF 210322558)

```jsonc
{ "type":"control_response", "response":{ "subtype":"success", "request_id":"<id>",
  "response":{
    "commands": [ {"name","description","argumentHint","aliases"?} ],   // always; userInvocable!==false filtered
    "agents":   [ {"name","description","model"?} ],                     // always; model omitted when "inherit"
    "output_style": "<string>",                                         // always; fallback "normal"
    "available_output_styles": ["normal", ...],                         // always; Object.keys(styleRecords)
    "models": [ ModelInfo ],                                            // always (see §4.3)
    "unavailable_models": [ ModelInfo ]?,                               // ONLY when length>0
    "account": { "email"?,"organization"?,"subscriptionType"?,"tokenSource"?,"apiKeySource"?,"apiProvider"? },
    "pid": <process.pid>,                                               // always
    "feedback_survey_config": {...}?,                                   // host-gated; absent ⇒ host must not show
    "fast_mode_state": "off"|"cooldown"|"on"?,                          // ONLY if $l()&&vk()
    "current_model": "<string>"?,                                      // BRIDGE PATH ONLY (getInitializeState spread)
    "current_permission_mode": "<mode>"?                                // BRIDGE PATH ONLY
  } } }
```

**ModelInfo (`P$o`):** `{value, displayName, description, supportsEffort?,
supportedEffortLevels?(low/medium/high/xhigh/max), supportsAdaptiveThinking?,
supportsFastMode?, supportsAutoMode?, disabled?}`.

**account `apiProvider`** ∈ `firstParty|bedrock|vertex|foundry|anthropicAws|mantle|gateway`.

**Critical:** `slash_commands`, `mcp_servers`, `roots`, `model`, `permissionMode`,
`plugins` are NOT keys here — they belong to the SEPARATE `type:"system",subtype:"init"`
message. Do not conflate. `current_model`/`current_permission_mode` enter ONLY via the
REPL-bridge `getInitializeState()` spread; the main/stdio path (`zyc`) does NOT add them.
First-pass stdio port: OMIT both (the main-path behavior). They are schema-`.optional()`
so a consumer COULD set them, but the stdio handler must not.

**Re-initialize path:** when already initialized, the success response additionally
carries siblings `pending_permission_requests` + `pending_user_dialog_requests` (in-flight
`can_use_tool`/`request_user_dialog` frames) so a re-joining client can re-arm dialogs.
The leaked-TS-era LingXi note (integration map §e) says re-init returns
`subtype:"error", error:"Already initialized"` — the **binary** instead replies SUCCESS
with the two pending-array siblings (server-switch §3 re-init path). Follow the binary:
re-init = success + sibling arrays, NOT an error. First pass: success with empty sibling
arrays.

### 4.3 LingXi sources for the payload (integration map §e — all reachable)

| field | LingXi source |
|---|---|
| `commands` (name/description/argumentHint/aliases) | `runtime.dispatcher.registry().list_all()`; filter `userInvocable!==false`; `description = formatDescriptionWithSource(cmd)` |
| `agents` (name/description/model) | `orchestrator.list_agents()` → `AgentInfo`; omit model when "inherit" |
| `models` / `unavailable_models` | `orchestrator.list_model_listings()` / `list_available_models()` |
| `output_style`/`available_output_styles` | currently hardcoded `"default"` (GAP — produce real style list, fallback `"normal"`) |
| `account` | `detect_api_key_source()` (env-based); fill email/org/subscriptionType/tokenSource where available |
| `pid` | `std::process::id()` |
| `feedback_survey_config` | host-gated (`pyc()`); first pass OMIT (absent = host won't show) |
| `fast_mode_state` | first pass OMIT (only when `$l()&&vk()`) |

The richer `{name,description,argumentHint}` command triple is in the registry's
`Command` records (just not yet surfaced for the system/init frame at run.rs:124-128).
Telemetry `tengu_sdk_init_handshake` fires right after enqueue — optional.

---

## 5. LingXi integration plan (the seams, file:line precise)

### 5.1 Concurrent streaming stdin reader (PREREQUISITE — everything depends on this)

`apps/cli/src/stream_json_input.rs:287-320` `read_input_turns` is a **synchronous
drain-to-EOF** `for line in reader.lines()` returning `Vec<UserTurn>`; driven from
`run.rs:322-326` via `spawn_blocking`, turns run sequentially (run.rs:354-376). No
inbound frame can arrive mid-turn → no control protocol is possible.

**Required:** convert to a streaming async reader task that pushes frames onto channels:
- `user` → a turn queue (mpsc) consumed by the turn driver.
- `control_request` → the control dispatcher (§5.5).
- `control_response` → the pending-request resolver (§1.3).
- `keep_alive`/`update_environment_variables` → handled inline (as today).

TS model: `structuredIO.read()` is an `AsyncGenerator`; print.ts:2816 `for await`
handles `control_request` INLINE while `user` feeds the turn `run()`; `control_response`
is resolved against `pendingRequests` and NOT surfaced to the loop. Mirror that: a
single inbound task owns frame routing; `process_line` becomes a router that returns a
richer `FrameAction` (add `ControlRequest(Value)` + `ControlResponse(Value)` variants).
Keep stdin reading off the async runtime (a `spawn_blocking` lines loop forwarding into a
tokio mpsc, OR `tokio::io::stdin` + `BufReader.lines()`).

### 5.2 Single-writer outbound queue (PREREQUISITE — no-overtake)

Refactor `StreamJsonStream.out: Arc<Mutex<Stdout>>` (`stream_json.rs:180`) →
`mpsc::Sender<Value>` feeding ONE drain task that calls `emit_line`. `StreamJsonStream`
emit_* methods push onto the channel instead of locking stdout. The SAME
`Arc<StreamJsonStream>` already reaches both the OutputStream and the loop
(`lib.rs:146-170`), so the control writer needs no new plumbing — it shares the channel.
`escape_line_terminators` + `\n` framing (stream_json.rs:33-44) unchanged.

### 5.3 Control-plane writer + pendingRequests + resolver (§1.3 / §3)

New `ControlPlaneWriter` over the shared outbound channel:
- `send_control_response_success(req_id, payload: Option<Value>)` → `rn`.
- `send_control_response_error(req_id, msg)` → `Dn`.
- `send_control_cancel_request(req_id)`.
- `send_request(request: Value) -> oneshot::Receiver<Result<Value,String>>` — mints
  uuid, enqueues `control_request`, registers the pending entry (§1.3), returns the
  receiver. `control_response` arm of the inbound router resolves it.
- `resolvedToolUseIds` Set + dedup log (§1.5).

### 5.4 Turn-loop interrupt seam (`interrupt` + `deny+interrupt`)

The print/stream-json path calls non-cancellable `run_turn(&prompt)` (run.rs:367,
conversation.rs:2947→try_run_turn, no token). **Switch to
`run_turn_streaming_with_cancel`** (conversation.rs:5080; handle method
handle_impl.rs:297) holding a per-turn `CancellationToken` (mint from
`tokio_util::sync::CancellationToken`). Share the token (Arc/clone) with the control
dispatcher. On inbound `control_request{subtype:"interrupt"}`: `cancel.cancel()` then
`send_control_response_success`. On `can_use_tool` `deny+interrupt` (§3.4): cancel the
same token. The streaming-with-cancel driver already injects
`INTERRUPT_MESSAGE`/`"[Request interrupted by user]"` and ends the turn gracefully
(`aborted_streaming`).

### 5.5 Control dispatcher — replace the reject stub with the real switch

Replace `stream_json_input.rs:159-168` (`"…full control protocol is not yet implemented
(P5 deferred)"`) with the §2 switch: a `match subtype { … }` covering all 46 server
subtypes, [T] ones wired to seams, [D] ones falling through to the binary's
`format!("Unsupported control request subtype: {subtype}")` error via
`send_control_response_error`. Keep the `request`-missing error (line 161-163,
byte-matches binary). The dispatcher runs in the inbound task (or a spawned per-request
task for async [D] handlers, keyed by `request_id` for `control_cancel_request`).

### 5.6 session/model/permission-mode mutation seams

- **set_model:** `orchestrator.switch_model(model, None)` (handle_impl.rs:106) — EXISTS.
  Resolve `"default"` to LingXi's configured default first. Verify the live router
  re-reads `session.model` per turn (it builds each request from session model, so it
  should take effect next turn — confirm the router doesn't cache the boot model).
- **set_max_thinking_tokens / rename_session / message_rated / get_session_cost /
  get_context_usage / get_usage / mcp_status / get_binary_version / stop_task / seed_read_state:**
  small handlers over existing orchestrator/session/registry accessors.
- **set_permission_mode — NET-NEW MUTATION SURFACE (the real gap).** The mode is baked
  into `PolicyPermissionGate` at boot behind an immutable `Arc`
  (engine-desktop:2787-2808); only `plan_mode` is live (via `SessionState.plan_mode` +
  `check_in_plan_mode`). Required: give `PolicyPermissionGate` an interior
  `ArcSwap<PermissionMode>` (or `Mutex`) read per `authorize_with_mode`, a setter, and a
  handle method `set_permission_mode(mode)` (most faithful to TS live-read). The
  mode→string mapping already exists (`stream_json.rs:1000-1010` `permission_mode_str`).
  `apply_flag_settings` is the same class of gap (model/effort tractable via session;
  agent-switch deep).

---

## 6. SMALLEST-VIABLE-FIRST phased plan (each phase independently testable)

**Phase 0 — Frame plumbing refactor (enables everything).**
- 0a: single-writer outbound channel (§5.2). Test: existing stream-json output golden
  tests still pass (no behavior change, only the writer's internal path).
- 0b: streaming async stdin reader (§5.1) replacing drain-to-Vec; `process_line` returns
  `ControlRequest`/`ControlResponse` variants routed onto channels. Test: existing
  user-turn + replay-ack tests pass; a `control_request` no longer prints the deferred
  stderr line.

**Phase 1 — initialize + interrupt + the switch skeleton.**
- ControlPlaneWriter (`rn`/`Dn`/cancel) + the §2 dispatcher skeleton with EVERY subtype
  falling through to `Unsupported control request subtype` (byte-faithful baseline).
- `initialize` handler: build the §4 registry payload, reply success. (record-and-ignore
  inbound hooks/agents/systemPrompt.)
- `interrupt`: switch the loop to `run_turn_streaming_with_cancel` (§5.4), cancel+ack.
- Test: feed `{control_request, subtype:initialize}` → assert the §4 success envelope
  (double-nested, `pid`, `commands`/`agents`/`models` present). Feed `interrupt` mid-turn
  → assert turn cancels + empty success. Feed unknown subtype → assert
  `Unsupported control request subtype: <x>` error frame.

**Phase 2 — can_use_tool decider (the SDK-permission integration).**
- `StdioControlPermissionGate` (§3.5) at engine-desktop:2539; `send_request` round-trip;
  decode `PermissionToolOutput`; map allow/deny/(deny+interrupt). Keep `PolicyPermissionGate`
  outer (local pre-check). DEFER the hook-vs-SDK race + `updatedInput` rewrite + sandbox-ask.
- Test: drive a tool that needs permission → assert an outbound
  `control_request{subtype:can_use_tool, tool_name, input, tool_use_id}` on stdout; inject
  a matching `control_response{allow, updatedInput:{}}` → tool proceeds; inject `{deny,
  message}` → tool denied with that message; `{deny, interrupt:true}` → turn aborts.

**Phase 3 — the tractable [T] switch arms, incrementally.**
- Batch by seam: `set_model`/`set_max_thinking_tokens`/`get_session_cost`/
  `get_context_usage`/`get_usage`/`mcp_status`/`get_binary_version`/`rename_session`/
  `message_rated`/`stop_task`/`seed_read_state`/`end_session`. Each: one handler + one
  round-trip test (request → expected success/error frame).
- `set_permission_mode`: the net-new mutation surface (§5.6) — gate `ArcSwap<mode>` +
  setter + handle method + mode-validation errors. Test mode transitions + the
  bypassPermissions/auto error strings.

**Phase 4 — control_cancel_request + dedup hardening.**
- Outbound cancel on turn abort (§1.4); inbound cancel → abort the keyed async handler.
- `resolvedToolUseIds` dedup + the duplicate-response log line (§1.5).
- Test: abort a pending `can_use_tool` → assert `control_cancel_request` emitted + the
  pending future rejects; redeliver a resolved `control_response` → assert it is
  dropped+logged, no double-resolve.

**Phase 5+ (DEEP, as subsystems land):** MCP family (mcp_message/mcp_call/mcp_reconnect/
mcp_toggle/mcp_set_servers/set_mcp_permission_mode_override/channel_enable/mcp_*auth*),
file family (read_file/stage_file/register_repo_root/add_directory/seed beyond basic/
file_suggestions/rewind_files/rewind_conversation), OAuth (claude_*), feedback/side_question/
ultrareview/remote_control, and the `initialize` hooks/agents MERGE sub-protocol +
hook_callback/elicitation outbound. Each stays `Unsupported control request subtype`
until its subsystem exists — that fallback is correct, not a stub.

---

## 7. Test strategy

**Unit (pure, no orchestrator):**
- Envelope round-trip: build success/error/cancel frames → assert exact JSON
  (double-nesting; `response` key ABSENT when payload `None`, never `null`).
- `normalize_control_message_keys` (already present) — regression keep.
- pendingRequests pairing: send_request → resolve by inner `request_id` → future
  resolves with inner `response.response`; error subtype → future rejects with
  `error` string; orphan id → dropped.
- dedup: resolvedToolUseIds eviction at 1000; duplicate drop + log.

**Integration (driven through the streaming reader + a fake host on stdin/stdout):**
- A test harness that writes control frames to the reader's stdin channel and asserts
  frames on the outbound channel, in order (verify no-overtake: queue a data frame +
  a control frame, assert FIFO).
- initialize → §4 payload assertion against the registry fixtures.
- interrupt mid-turn → turn cancellation + `[Request interrupted by user]`.
- can_use_tool: the full Phase-2 matrix (allow / allow+updatedInput / deny / deny+interrupt /
  host-never-answers-then-turn-aborts → AbortError-mapped deny).
- each [T] subtype: request → expected success/error frame (table-driven over §2.2).
- fallthrough: a random subtype → `Unsupported control request subtype: <x>`.

**Oracle/byte parity:** for any frame LingXi EMITS (initialize response, can_use_tool
request, every success/error envelope), diff the serialized JSON key set against the
binary schema literals in the extraction docs. Re-extract the binary handler body before
implementing any [D] subtype (the payload-field caveats in §2.2 / §3.2 / §3.3 / §4.1 flag
exactly which fields were inferred-by-destructuring vs schema-confirmed).

**Not coverable statically (call out, don't fake):** concurrent multi-tool permission
prompts (the pending-permission bookkeeping + "transition back to running only when
getPendingPermissionRequests().length===0" finally-block) — describe from source,
exercise with a live multi-tool stream-json session when available. No live session was
run during extraction; all facts are static (binary byte + TS structure).
