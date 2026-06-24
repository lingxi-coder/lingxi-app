# Control Frame Wire Envelope — stream-json stdio control protocol

Oracle binary: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude` (v2.1.x, minified single-file).
Readable (older, SUBSET) TS source: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`.

The control protocol is the side-channel that multiplexes over the SAME stream-json NDJSON stdio as data messages (`user`/`assistant`/`system`/`result`/`stream_event`). Each line of stdin/stdout is one JSON object; `type` discriminates data frames from control frames. There are exactly **three** control message `type` literals plus two adjacent stream-control literals (`keep_alive`, `update_environment_variables`).

Canonical Zod schema source: `entrypoints/sdk/controlSchemas.ts`.
Canonical reader/writer + pendingRequests map + ordering: `cli/structuredIO.ts` (class `StructuredIO`).
Canonical responder (the `control_request` switch on `subtype`): `cli/print.ts` (`runHeadlessStreaming`, builders `sendControlResponseSuccess` / `sendControlResponseError`).
Canonical key-normalizer: `utils/controlMessageCompat.ts` (`normalizeControlMessageKeys`).

---

## 1. The three control `type` literals (counts in binary)

```
"control_request"          29 occurrences
"control_response"         65 occurrences
"control_cancel_request"   10 occurrences
```
All three survive minification as string literals. `control_cancel_request` is its **OWN top-level `type`** — NOT a `control_request` carrying a cancel flag.

---

## 2. control_request — EXACT shape

Zod (`controlSchemas.ts` `SDKControlRequestSchema`):
```ts
z.object({
  type: z.literal('control_request'),
  request_id: z.string(),
  request: SDKControlRequestInnerSchema(),   // discriminated by request.subtype
})
```

Wire shape:
```json
{ "type": "control_request", "request_id": "<uuid-string>", "request": { "subtype": "<name>", ...subtype-specific fields } }
```

Binary byte evidence (build site in structuredIO.sendRequest, OFF≈210362272 region):
```
...request_id:r,request:e};if(this.inputClosed)throw Error("Stream closed");
if(n?.aborted)throw Error("Request aborted");
if(this.outbound.enqueue(o),e.subtype==="can_use_tool"&&this.onControlRequestSent)...
```
TS (`structuredIO.ts` sendRequest):
```ts
const message: SDKControlRequest = { type: 'control_request', request_id: requestId, request }
```

Bridge build site (OFF=202099435), showing prefixed request_id variants:
```
type:"control_request",request_id:`set-mode-${lut.randomUUID()}`,request:{subtype:"set_permission_mode",mode:...,ultraplan:...}
type:"control_request",request_id:`apply-flag-settings-${lut.randomUUID()}`,request:{subtype:"apply_flag_settings",...}
type:"control_request",request_id:lut.randomUUID(),request:{subtype:"interrupt"},uuid:lut.randomUUID()
```

### The `request.subtype` enumeration (full set from the binary control switch)

Confirmed present as `subtype` literals in the print.ts handler and controlSchemas:
```
initialize  interrupt  end_session  can_use_tool  set_permission_mode  set_model
set_max_thinking_tokens  mcp_status  get_context_usage  hook_callback  mcp_message
rewind_files  cancel_async_message  seed_read_state  mcp_set_servers  reload_plugins
mcp_reconnect  mcp_toggle  stop_task  apply_flag_settings  get_settings  elicitation
```
(`end_session` is handled in print.ts but is NOT in the controlSchemas union — older TS subset. `can_use_tool` is the SDK→host permission prompt direction; the rest are host←SDK control commands.)

Each subtype's payload (from `controlSchemas.ts`), notable ones:
- `initialize`: `{subtype, hooks?, sdkMcpServers?, jsonSchema?, systemPrompt?, appendSystemPrompt?, agents?, promptSuggestions?, agentProgressSummaries?}`
- `can_use_tool`: `{subtype, tool_name, input, permission_suggestions?, blocked_path?, decision_reason?, title?, display_name?, tool_use_id, agent_id?, description?}`
- `set_permission_mode`: `{subtype, mode, ultraplan?}`
- `set_model`: `{subtype, model?}`
- `set_max_thinking_tokens`: `{subtype, max_thinking_tokens: number|null}`
- `hook_callback`: `{subtype, callback_id, input, tool_use_id?}`
- `mcp_message`: `{subtype, server_name, message: JSONRPCMessage}`
- `rewind_files`: `{subtype, user_message_id, dry_run?}`
- `cancel_async_message`: `{subtype, message_uuid}`
- `seed_read_state`: `{subtype, path, mtime}`
- `elicitation`: `{subtype, mcp_server_name, message, mode?, url?, elicitation_id?, requested_schema?}`
- `interrupt`/`mcp_status`/`get_context_usage`/`reload_plugins`/`get_settings`: `{subtype}` only.

---

## 3. control_response — EXACT shape

Zod (`controlSchemas.ts` `SDKControlResponseSchema`):
```ts
z.object({
  type: z.literal('control_response'),
  response: z.union([ControlResponseSchema, ControlErrorResponseSchema]),
})
```
where:
```ts
ControlResponseSchema      = { subtype: z.literal('success'), request_id: z.string(), response?: z.record(...) }
ControlErrorResponseSchema = { subtype: z.literal('error'),   request_id: z.string(), error: z.string(),
                               pending_permission_requests?: SDKControlRequest[] }
```

**Note the double nesting**: the top-level object has `{type:"control_response", response:{...}}`, and the inner `response` object's OWN optional `response` field carries the success payload. So a success has `message.response.response` (payload), an error has `message.response.error`.

Wire shapes:
```json
{ "type": "control_response", "response": { "subtype": "success", "request_id": "<id>", "response": { ...payload } } }
{ "type": "control_response", "response": { "subtype": "error",   "request_id": "<id>", "error": "<message>" } }
```

Binary byte evidence — `sendControlResponseSuccess` / `sendControlResponseError` builders (print.ts, switch on subtype, OFF≈206177243+):
```
type:"control_response",response:{subtype:"success",request_id:e.request_id,response:{commands:[],agents:[],output_style:"normal",...}}   // initialize reply
type:"control_response",response:{subtype:"success",request_id:e.request_id}                                                            // payload-less ack
type:"control_response",response:{subtype:"success",request_id:e.request_id,response:{mcpServers:...}}                                   // mcp_status
{subtype:"error",request_id:e.request_id,error:$ym}                                                                                     // outbound-only reject
{subtype:"error",request_id:e.request_id,error:w.error}                                                                                 // generic error
{subtype:"error",request_id:e.request_id,error:"file_suggestions is not supported in this context"}
```
TS (`print.ts`):
```ts
const sendControlResponseSuccess = (message, response?) =>
  output.enqueue({ type:'control_response', response:{ subtype:'success', request_id: message.request_id, response } })
const sendControlResponseError = (message, errorMessage) =>
  output.enqueue({ type:'control_response', response:{ subtype:'error', request_id: message.request_id, error: errorMessage } })
```

`pending_permission_requests` (the error-response extra field, an array of re-deliverable `control_request`s): literal present at binary OFFs 128465008 / 192877654 / 206205036. The SessionsV2 client strips `response.pending_permission_requests` and `response.pending_user_dialog_requests` from re-delivered control responses (OFF 206205036).

---

## 4. control_cancel_request — EXACT shape (own type)

Zod (`controlSchemas.ts` `SDKControlCancelRequestSchema`):
```ts
z.object({
  type: z.literal('control_cancel_request'),
  request_id: z.string(),
}).describe('Cancels a currently open control request.')
```

Wire shape:
```json
{ "type": "control_cancel_request", "request_id": "<id>" }
```

Binary byte evidence (two emit sites):
- structuredIO.injectControlResponse / sendRequest abort path (OFF≈210357994, 210362272):
```
...this.write({type:"control_cancel_request",request_id:t})...                      // injectControlResponse: bridge won, cancel SDK callback
let s=()=>{this.outbound.enqueue({type:"control_cancel_request",request_id:r}); ... a.reject(new $c)}  // signal.abort handler
```
- RemoteSessionManager (OFF=207622342): `type:"control_cancel_request",request_id:Ee,session_id:ce` (carries session_id when going over the websocket).

TS (`structuredIO.ts`):
```ts
// on AbortSignal abort, or when the bridge resolves a permission the SDK still has open:
this.outbound.enqueue({ type:'control_cancel_request', request_id: requestId })
```
Semantics: cancels an OPEN `control_request` (typically a `can_use_tool` permission prompt) so the peer's pending `canUseTool` callback is aborted via its signal — otherwise the callback hangs. It does NOT itself get a response; the original request's promise is rejected immediately client-side (`new AbortError()` / `$c`) without waiting for an ack.

---

## 5. request_id format

Default generator: **`randomUUID()`** (Node `crypto.randomUUID()` → UUID v4 string), `request_id: string` in the schema. Byte evidence: `request_id:lut.randomUUID()` (interrupt), and the structuredIO `sendRequest(request, schema, signal?, requestId: string = randomUUID())` default param.

Prefixed variants exist on the bridge/CCR control-event path (still UUID-suffixed, still opaque strings):
- `` `set-mode-${randomUUID()}` `` (set_permission_mode)
- `` `apply-flag-settings-${randomUUID()}` `` (apply_flag_settings)

There is no format constraint beyond `z.string()` — the host echoes back whatever `request_id` it received, byte-for-byte, in the matching `control_response.response.request_id`. The protocol treats it as an opaque correlation token.

(Telemetry note: a `request_id_sha12` field and `request_id:xr(o.requestId)` exist for *API* request ids — distinct from control request ids; do not conflate.)

---

## 6. Pairing — the `pendingRequests` map keyed by request_id

`StructuredIO` (structuredIO.ts:137):
```ts
private readonly pendingRequests = new Map<string, PendingRequest<unknown>>()
type PendingRequest<T> = { resolve, reject, schema?, request: SDKControlRequest }
```

**Send path** (`sendRequest`, structuredIO.ts:469): build `{type:'control_request', request_id, request}`, `outbound.enqueue(message)`, then register a Promise and `pendingRequests.set(requestId, { request:{type:'control_request',request_id,request}, resolve, reject, schema })`. Binary (OFF 210362512):
```
this.pendingRequests.set(r,{request:{type:"control_request",request_id:r,request:e},resolve:(c)=>{a(c)},reject:l,schema:t})
```
`finally{ pendingRequests.delete(requestId) }` always runs.

**Receive path** (`processLine`, structuredIO.ts:362, the ONLY `control_response` sink):
1. `normalizeControlMessageKeys(jsonParse(line))` first.
2. If `message.type === 'control_response'`: `request = pendingRequests.get(message.response.request_id)`.
3. If `!request` → orphan/duplicate handling (see §8); else:
   - `trackResolvedToolUseId(request.request)`, then `pendingRequests.delete(message.response.request_id)`.
   - If `request.request.request.subtype === 'can_use_tool'` → fire `onControlRequestResolved(request_id)` (bridge cancels stale claude.ai prompt).
   - If `message.response.subtype === 'error'` → `request.reject(new Error(message.response.error))`.
   - Else payload `= message.response.response`; if `request.schema` → `request.resolve(schema.parse(payload))` (reject on parse failure); else `request.resolve({})`.

So **pairing is purely by `request_id` string equality** into the Map. The response's nested `response.request_id` is the join key. The schema attached at send-time validates/shapes the success payload.

`getPendingRequests()` iterates `pendingRequests.values()` (structuredIO.ts:255–264) — used to re-deliver pending permission requests on reconnect (→ `pending_permission_requests`).

Bridge/host side has a parallel map (`pendingControlRequests`, binary OFFs 187111392 / 206211705…) plus a separate `pendingRequests` (OFF 58260918 / 192920752 / 210354272 — the StructuredIO one). Same keyed-by-request_id discipline.

---

## 7. normalizeControlMessageKeys (requestId ↔ request_id)

Source `utils/controlMessageCompat.ts`. In-place mutation, applied to EVERY incoming line before type-dispatch (`processLine` calls it first). Rationale (from the docstring): older iOS app builds send camelCase `requestId` (missing Swift CodingKeys), which `isSDKControlRequest` (`'request_id' in value`) and structuredIO (`message.response.request_id`) would otherwise silently drop.

Rules (snake_case wins if both present):
- Top level: `if 'requestId' in record && !('request_id' in record) → record.request_id = record.requestId; delete record.requestId`.
- Nested `response`: if `record.response` is a non-null object, apply the same `requestId → request_id` rename inside `record.response`.

Binary byte evidence — function `IYn` (OFF 206174349):
```
function IYn(e){if(e===null||typeof e!=="object")return e;let t=e;
  if("requestId"in t&&!("request_id"in t))t.request_id=t.requestId,delete t.requestId;
  if("response"in t&&t.response!==null&&typeof t.response==="object"){let n=t.response;
    if("requestId"in n&&!("request_id"in n))n.request_id=n.requestId,delete n.requestId}
  return e}
```
Byte-faithful to the TS. It normalizes BOTH `control_request` (top-level `requestId`) AND `control_response` (`response.requestId`). It does NOT touch `control_cancel_request` payloads beyond the top-level rename (which is enough, since cancel only has top-level `request_id`).

Adjacent guards in the binary (same module):
- `Bym(e)` = isSDKControlResponse: `type==="control_response" && "response"in e`.
- `Uym(e)` = isSDKControlRequest: `type==="control_request" && "request_id"in e && "request"in e` (OFF 206175021).
- `z0o(e)` = has-string-type guard.
- `ZTe(e)` = sanitizes a string array (≤64 chars, ≤32 items) — used for sdkMcpServers in initialize.

---

## 8. Ordering rules — control frames vs data frames on stdout

**Single FIFO, single writer. Control plane NEVER overtakes data plane.**

The mechanism (structuredIO.ts:160–162):
```ts
// sendRequest() and print.ts both enqueue here; the drain loop is the
// only writer. Prevents control_request from overtaking queued stream_events.
readonly outbound = new Stream<StdoutMessage>()
```
`Stream` (`utils/stream.ts`) is a plain in-order queue (`private readonly queue: T[]`), iterated exactly once (`started` guard throws on second iteration). `enqueue` either resolves a waiting `next()` immediately or pushes to the tail — strict insertion order, no priority lanes.

`runHeadlessStreaming` (print.ts:976) does `const output = structuredIO.outbound` (print.ts:1022, comment: "Same queue sendRequest() enqueues to — one FIFO for everything") and `return output` at the end of the function. The top-level consumer loop `for await (const message of runHeadlessStreaming(...))` (print.ts:864) then `structuredIO.write()`s each frame to stdout in dequeue order.

Everything goes through this one queue:
- Data frames: `assistant`, `user`, `system` (incl. status/session_state_changed), `stream_event`, `result`, streamlined_* — enqueued by the query loop.
- Control frames: `control_request` (from `sendRequest` and `setPermissionModeChangedListener`-emitted `system/status`), `control_response` (from `sendControlResponseSuccess`/`Error`), `control_cancel_request` (from `injectControlResponse` and the abort handler).

Because all producers `outbound.enqueue(...)` into the same `Stream` and the single drain loop is the only thing that `writeToStdout`s, a `control_request` (e.g. a `can_use_tool` permission prompt) emitted while `stream_event`s are queued is appended AFTER those queued data frames — it cannot jump ahead. This is the explicit anti-overtake guarantee the comment names.

**Exception — orphan/late control_response handling does NOT go through the main loop:** `processLine` resolves/rejects pending requests and (for orphans) calls `unexpectedResponseCallback` inline; it only `yield`s the `control_response` back to the main loop when `replayUserMessages` is set. Comment (structuredIO.ts:362): "orphans don't yield to print.ts's main loop, so this is the only path that sees them."

**Duplicate-response dedup (ordering hazard guard):** `resolvedToolUseIds: Set<string>` (cap `MAX_RESOLVED_TOOL_USE_IDS = 1000`, oldest-evicted). When a `control_response` arrives for a `request_id` no longer in `pendingRequests`, but its payload's `toolUseID` is in `resolvedToolUseIds`, it's logged and dropped: `"Ignoring duplicate control_response for already-resolved toolUseID=… request_id=…"` (binary OFF 192947721 / 210359952). Prevents duplicate WebSocket-reconnect deliveries from pushing duplicate assistant messages (API 400 "tool_use ids must be unique").

On read, non-control unknown types are warn-logged and skipped; malformed JSON lines `console.error` + `process.exit(1)`. `keep_alive` and `update_environment_variables` lines are consumed silently (the latter mutates `process.env`).

---

## 9. Aggregate stdin/stdout message unions (controlSchemas.ts)

`StdoutMessageSchema` (host → SDK): union of SDKMessage, streamlined text/tool/post-turn, **SDKControlResponse, SDKControlRequest, SDKControlCancelRequest**, SDKKeepAlive.
`StdinMessageSchema` (SDK → host): union of SDKUserMessage, **SDKControlRequest, SDKControlResponse**, SDKKeepAlive, SDKUpdateEnvironmentVariables.

So `control_request` and `control_response` flow in BOTH directions over the one stdio stream (host asks `can_use_tool`; SDK asks `interrupt`/`set_model`/etc.). `control_cancel_request` is in the Stdout union (host→SDK cancel) and is also written by structuredIO on the host side. `keep_alive` is bidirectional and silently dropped on read.

---

## 10. Quick reference — the three envelopes

```json
// control_request (correlation token = request_id)
{ "type":"control_request", "request_id":"<uuid>", "request":{ "subtype":"<name>", ...fields } }

// control_response — SUCCESS  (note the inner response.response payload)
{ "type":"control_response", "response":{ "subtype":"success", "request_id":"<uuid>", "response":{...}? } }

// control_response — ERROR
{ "type":"control_response", "response":{ "subtype":"error", "request_id":"<uuid>", "error":"<msg>",
                                          "pending_permission_requests":[ <control_request>, ... ]? } }

// control_cancel_request (own top-level type; no response expected)
{ "type":"control_cancel_request", "request_id":"<uuid>" }
```
