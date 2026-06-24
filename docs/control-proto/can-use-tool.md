# CLI-as-Client `can_use_tool` permission-over-stdio flow

**Oracle binary:** `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude` (v2.x, 215,994,048 bytes)
**Leaked TS (older subset, structure hints only):** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
**Date:** 2026-06-24

---

## 0. TL;DR — the flow

When a tool needs permission AND the SDK is driving (`--input-format stream-json` + `--permission-prompt-tool stdio` / the `query()` SDK consumer), the CLI does **not** show an interactive terminal prompt. Instead `getCanUseToolFn('stdio', …)` returns `StructuredIO.createCanUseTool(...)`, which:

1. Runs the **local** permission machinery first (`hasPermissionsToUseTool`). If that already resolves `allow` or `deny`, it returns immediately — **no control_request is sent**.
2. Only when the local result is `behavior: 'ask'` does it go over stdio. It then races **two** things:
   - **localHook** = `executePermissionRequestHooksForSDK(...)` — runs local `PermissionRequest` hooks in the background.
   - **sdkRequest** = `this.sendRequest({subtype:'can_use_tool', ...})` — emits a `control_request` to **stdout** and awaits the matching `control_response` from **stdin**.
3. `Promise.race([hookPromise, sdkPromise])` — **whoever resolves first wins**; the loser is aborted/ignored. (See §4 for the exact tie-break.)
4. The winning result is normalized by `permissionPromptToolResultToPermissionDecision(...)` into `{behavior, updatedInput?, updatedPermissions?, message?, decisionReason}`, with `deny + interrupt` aborting the whole turn.

There is **no timeout** on the SDK request inside `createCanUseTool`. It blocks until (a) a `control_response` arrives, (b) the local hook decides first (aborts the SDK request), or (c) the parent `toolUseContext.abortController` aborts (Ctrl-C / interrupt control_request) → `control_cancel_request` is sent and the promise rejects with `AbortError`.

---

## 1. Entry point — `getCanUseToolFn` selects the stdio path

`src/cli/print.ts` ~L4267:
```js
export function getCanUseToolFn(permissionPromptToolName, structuredIO, getMcpTools, onPermissionPrompt) {
  if (permissionPromptToolName === 'stdio') {
    return structuredIO.createCanUseTool(onPermissionPrompt)   // <-- SDK-driving path
  }
  if (!permissionPromptToolName) { /* terminal: local hasPermissionsToUseTool only */ }
  // else: an MCP tool named by --permission-prompt-tool → createCanUseToolWithPermissionPrompt
}
```
- `'stdio'` is the sentinel that means "the SDK host on the other end of stdout/stdin makes the decision."
- A non-`stdio` `--permission-prompt-tool` name resolves to an MCP tool and goes through `createCanUseToolWithPermissionPrompt` (a different code path; it calls `permissionPromptTool.call(...)` and races only against the abort signal — no hook race).

---

## 2. The `can_use_tool` control_request SHAPE (CLI → SDK, over stdout)

### 2a. Construction site (`StructuredIO.createCanUseTool` → `sendRequest`)
`src/cli/structuredIO.ts` ~L590:
```js
const sdkPromise = this.sendRequest(
  {
    subtype: 'can_use_tool',
    tool_name: tool.name,
    input,
    permission_suggestions: mainPermissionResult.suggestions,
    blocked_path: mainPermissionResult.blockedPath,
    decision_reason: serializeDecisionReason(mainPermissionResult.decisionReason),
    tool_use_id: toolUseID,
    agent_id: toolUseContext.agentId,
  },
  permissionToolOutputSchema(),
  hookAbortController.signal,      // <-- aborting cancels the SDK request
  requestId,                       // <-- same id passed to onPermissionPrompt
).then(result => ({ source: 'sdk', result }))
```

### 2b. Envelope (`sendRequest`, structuredIO.ts ~L469)
```js
const message = { type: 'control_request', request_id: requestId, request /* the object above */ }
this.outbound.enqueue(message)             // serialized as NDJSON to stdout
if (request.subtype === 'can_use_tool' && this.onControlRequestSent)
  this.onControlRequestSent(message)        // bridge hook → forward to claude.ai
```
So the wire frame is:
```json
{ "type": "control_request",
  "request_id": "<uuid>",
  "request": { "subtype": "can_use_tool", "tool_name": "...", "input": {...}, ... } }
```

### 2c. AUTHORITATIVE request field set — Zod schema `Bgc` from the binary
Byte offset 210324747 (`"can_use_tool"`), extracted via tail/head/tr:
```js
Bgc = C.object({
  subtype: C.literal("can_use_tool"),
  tool_name: C.string(),
  input: C.record(C.string(), C.unknown()),
  permission_suggestions: C.array(ojt()).optional(),       // PermissionUpdate[]
  blocked_path: C.string().optional(),
  decision_reason: C.string().optional(),                  // serialized text
  decision_reason_type: C.enum(TSr).optional()             // structured discriminator…
      .describe('… Lets SDK hosts make policy (e.g. auto-deny safetyCheck) without
                 parsing decision_reason text. For compound bash commands this is
                 "subcommandResults" even when a safetyCheck is nested inside…'),
  classifier_approvable: C.boolean().optional()            // safetyCheck present anywhere…
      .describe('… false = at least one safety check requires manual approval (Windows
                 path bypass, dangerous rm); true = all safety checks MAY be
                 classifier-approved. Absent when no safetyCheck is involved.'),
  title: C.string().optional(),                            // from MCP _meta anthropic/permissionDisplay
  display_name: C.string().optional(),                     // short tool/server label
  tool_use_id: C.string(),                                 // REQUIRED (not optional)
  agent_id: C.string().optional(),                         // subagent routing
  description: C.string().optional(),
}).describe("Requests permission to use a tool with the given input.")
```

**Field notes:**
- `tool_use_id` is the ONLY required field beyond `subtype/tool_name/input`. `permission_suggestions`, `blocked_path`, `decision_reason`, `decision_reason_type`, `classifier_approvable`, `title`, `display_name`, `agent_id`, `description` are all `.optional()`.
- `title` / `display_name` / `description` are sourced from the MCP server's `_meta['anthropic/permissionDisplay']` (`title` / `.displayName` / `.description`). They let SDK hosts render structured permission headers instead of parsing `message`. (Binary `describe` strings confirm: "Mirrors can_use_tool.title …".)
- `decision_reason` is a free-text serialization; `decision_reason_type` is the machine-readable discriminator for auto-mode escalation policy.
- The CLI-side builder in `createCanUseTool` (2a) does NOT populate `title`/`display_name`/`description`/`decision_reason_type`/`classifier_approvable` — those are populated on other emit paths (the bridge builder `sendControlRequest` adds `display_name: Fce(r)` and `description`; the elicitation/MCP-permissionDisplay path supplies title/display_name). The schema is the superset the SDK consumer must accept.

### 2d. The SDK-CONSUMER side (the `query()` host that RECEIVES it) — binary @ 206300637
```js
async processControlRequest(e, t) {
  if (e.request.subtype === "can_use_tool") {
    if (!this.canUseTool) throw Error("canUseTool callback is not provided.");
    return { ...await this.canUseTool(
        e.request.tool_name, e.request.input,
        { signal: t,
          suggestions:    e.request.permission_suggestions,
          blockedPath:    e.request.blocked_path,
          decisionReason: e.request.decision_reason,
          title:          e.request.title,
          displayName:    e.request.display_name,
          description:    e.request.description,
          toolUseID:      e.request.tool_use_id,
          agentID:        e.request.agent_id }),
      toolUseID: e.request.tool_use_id };
  } else if (e.request.subtype === "hook_callback") { ... }
  else if (e.request.subtype === "mcp_message") { ... }
}
```
This confirms the exact field names the consumer reads off the wire and how it maps them into its `canUseTool({...})` options bag.

---

## 3. The `control_response` SHAPE (SDK → CLI, over stdin)

### 3a. Response ENVELOPE — binary @ 210349524 (`control_response` literal)
```js
cYm = C.object({                                  // success
  subtype: C.literal("success"),
  request_id: C.string(),
  response: C.record(C.string(), C.unknown()).optional(),   // <-- the decision payload (§3b)
  pending_permission_requests: R_c(),
  pending_user_dialog_requests: v_c(),
})
uYm = C.object({                                  // error
  subtype: C.literal("error"),
  request_id: C.string(),
  error: C.string(),
  pending_permission_requests: R_c(),
  pending_user_dialog_requests: v_c(),
})
w_c = C.object({ type: C.literal("control_response"), response: C.union([cYm(), uYm()]) })
```
Wire frame:
```json
{ "type": "control_response",
  "response": { "subtype": "success", "request_id": "<uuid>", "response": { /* PermissionToolOutput */ } } }
```
On `subtype: "error"`, the pending request is rejected: `request.reject(new Error(message.response.error))`.

### 3b. The DECISION PAYLOAD (`response.response`) — `PermissionToolOutput` / `permissionToolOutputSchema`
Validated against the union below. Binary `hYm`/`gYm`/`ljt` @ ~210351900; TS `src/utils/permissions/PermissionPromptToolResultSchema.ts`:

**ALLOW** (`hYm`):
```js
{
  behavior: "allow",                              // literal
  updatedInput: C.record(C.string(), C.unknown()),   // REQUIRED. {} means "use original input" (§3c)
  updatedPermissions: C.array(permissionUpdate).optional()
      .catch(() => { log("Malformed updatedPermissions from SDK host ignored: …"); return undefined }),
  toolUseID: C.string().optional(),
  decisionClassification: C.enum(["user_temporary","user_permanent","user_reject"]).optional().catch(undefined),
}
```
**DENY** (`gYm`):
```js
{
  behavior: "deny",                               // literal
  message: C.string(),                            // REQUIRED — rejection reason returned to the model
  interrupt: C.boolean().optional(),              // true = abort the whole conversation turn (§3c)
  toolUseID: C.string().optional(),
  decisionClassification: C.enum(["user_temporary","user_permanent","user_reject"]).optional().catch(undefined),
}
output = C.union([allow, deny])  // ljt
```

**Critical field semantics:**
- `behavior`: `"allow" | "deny"` — the only two values. There is **no "ask"** on the response (ask only exists on the local pre-check; an ask escalates to the SDK request).
- `updatedInput` (allow only, REQUIRED by schema): the (possibly rewritten) tool input the SDK host approved. An empty `{}` is treated as "use original input" (mobile push-notification clients send `{}` because they lack the original input).
- `updatedPermissions` (allow only, optional): `PermissionUpdate[]` to persist ("always allow X"). Malformed entries are caught + dropped (logged) rather than rejecting the whole decision — anthropics/claude-code#29440.
- `message` (deny only, REQUIRED): the rejection text surfaced to the model in the `tool_result`.
- `interrupt` (deny only, optional): when `true`, the CLI calls `toolUseContext.abortController.abort()` — i.e. deny **and stop the turn**, not just deny this one tool. (§3c)
- `decisionClassification` (`user_temporary`/`user_permanent`/`user_reject`, optional, `.catch(undefined)`): mirrors `PermissionDecisionClassificationSchema`; a bad string falls through to `undefined` instead of rejecting.
- `toolUseID` (optional both branches): used by the orphan/duplicate-response handler to dedupe late responses (e.g. WebSocket reconnect re-deliveries).

### 3c. Response → PermissionDecision normalization
Binary `Jyt` @ 210350900 / TS `permissionPromptToolResultToPermissionDecision`:
```js
function Jyt(result, tool, input, ctx) {
  const decisionReason = { type: "permissionPromptTool", permissionPromptToolName: tool.name, toolResult: result };
  if (result.behavior === "allow") {
    if (result.updatedPermissions) {
      ctx.setToolPermissionContext(a => applyPermissionUpdates(a, result.updatedPermissions));
      persistPermissionUpdates(result.updatedPermissions);          // TW(s)
    }
    const updatedInput = Object.keys(result.updatedInput).length > 0 ? result.updatedInput : input;  // {} → original
    return { ...result, updatedInput, decisionReason };
  } else if (result.behavior === "deny" && result.interrupt) {
    log(`SDK permission prompt deny+interrupt: tool=${tool.name} message=${result.message}`);
    ctx.abortController.abort();                                     // <-- interrupt the turn
  }
  return { ...result, decisionReason, decideLocation: "ask-path" };  // NB: binary adds decideLocation:"ask-path"
}
```
> Byte-diff vs leaked TS: the **binary** appends `decideLocation: "ask-path"` to the returned decision (the older leaked TS does not). Otherwise identical.

---

## 4. The `Promise.race(localHook, sdkRequest)` logic — WHO WINS

`src/cli/structuredIO.ts` `createCanUseTool` ~L575-638. Verified against binary (same structure).

```js
// Pre-gate: local permission machinery runs FIRST.
const mainPermissionResult = forceDecision ?? await hasPermissionsToUseTool(...);
if (mainPermissionResult.behavior === 'allow' || mainPermissionResult.behavior === 'deny')
  return mainPermissionResult;          // short-circuit — NO control_request sent at all

// Only 'ask' reaches here.
const hookAbortController = new AbortController();
const parentSignal = toolUseContext.abortController.signal;       // turn-level abort
parentSignal.addEventListener('abort', () => hookAbortController.abort(), { once: true });

// localHook: PermissionRequest hooks (background). NEVER rejects; resolves undefined if no decision.
const hookPromise = executePermissionRequestHooksForSDK(tool.name, toolUseID, input, ctx, suggestions)
                      .then(decision => ({ source: 'hook', decision }));

// sdkRequest: emit control_request immediately (don't wait for hooks).
const requestId = randomUUID();
onPermissionPrompt?.(buildRequiresActionDetails(tool, input, toolUseID, requestId));   // UI/requires_action signal
const sdkPromise = this.sendRequest({subtype:'can_use_tool', ...}, schema, hookAbortController.signal, requestId)
                      .then(result => ({ source: 'sdk', result }));

const winner = await Promise.race([hookPromise, sdkPromise]);

if (winner.source === 'hook') {
  if (winner.decision) {                       // hook MADE a decision → hook wins
    sdkPromise.catch(() => {});                 // suppress the abort rejection
    hookAbortController.abort();                // cancel the in-flight SDK request → control_cancel_request
    return winner.decision;
  }
  // hook resolved with NO decision (passthrough) → fall back to waiting on the SDK
  const sdkResult = await sdkPromise;
  return permissionPromptToolResultToPermissionDecision(sdkResult.result, tool, input, ctx);
}
// SDK responded first → SDK wins; the still-running hook's result is ignored.
return permissionPromptToolResultToPermissionDecision(winner.result, tool, input, ctx);
```

**Tie-break / precedence rules (exhaustive):**
1. **Local `hasPermissionsToUseTool` allow/deny pre-empts everything** — if the static permission system already decides, no race happens and the SDK is never asked.
2. The SDK request is emitted **immediately** and in parallel with the hooks (so a slow hook, e.g. `--delay 20`, doesn't block the host's permission dialog). Comment in source: "the SDK host (VS Code, etc.) shows its permission dialog immediately while hooks run in the background."
3. **A hook that returns a concrete decision WINS over the SDK** — it aborts the pending SDK request (`hookAbortController.abort()` → `control_cancel_request` frame to the host) and returns the hook decision. So: **local PermissionRequest hook beats the SDK when it resolves first AND actually decides.**
4. A hook that resolves with **no decision** (passthrough) does NOT win — control falls through to `await sdkPromise` (the host's answer).
5. If the **SDK responds first**, the SDK wins and the still-running hook's eventual result is discarded.
6. **Abort/interrupt** (`toolUseContext.abortController`, e.g. Ctrl-C or an `interrupt` control_request) forwards to `hookAbortController` → the SDK request's signal aborts → `sendRequest` enqueues `control_cancel_request` and rejects with `AbortError`. The surrounding `try/catch` then maps that to `{behavior:'deny', message:'Tool permission request failed: …'}`.

**TIMEOUT:** There is **no per-request timeout** inside `createCanUseTool` for the SDK request — it waits indefinitely for a `control_response` (or until aborted). (Contrast: the bridge-side `mcp_status` poll and the suggestion-state path use `Promise.race([…, sleep(5000)])` elsewhere in print.ts, and `request_user_dialog` has `CLAUDE_CODE_USER_DIALOG_TIMEOUT_MS ?? 300000` ms — but `can_use_tool` itself has none.) The hook generator does receive `toolUseContext.abortController.signal`.

### 4b. `sendRequest` cancellation mechanics (structuredIO.ts ~L469)
```js
const aborted = () => {
  this.outbound.enqueue({ type: 'control_cancel_request', request_id: requestId });   // tell host to cancel
  const req = this.pendingRequests.get(requestId);
  if (req) { this.trackResolvedToolUseId(req.request); req.reject(new AbortError()); } // reject locally NOW
};
if (signal) signal.addEventListener('abort', aborted, { once: true });
// the promise is registered in this.pendingRequests.set(requestId, {resolve,reject,schema}) and
// resolved when the matching control_response arrives in processLine().
```
- `trackResolvedToolUseId` marks the tool_use as resolved so a **late** host response is ignored by the orphan handler (dedupes WebSocket reconnect re-deliveries).

---

## 5. SENT vs RECEIVED — control_request subtype inventory

Two binary Sets (P_c module @ ~210350760) drive this split:

```js
mYm = new Set(["interrupt","set_permission_mode","set_model","set_max_thinking_tokens",
               "set_color","mcp_toggle","message_rated"]);          // x_c(): buffered/inbound subtypes
fYm = new Set(["can_use_tool","request_user_dialog","elicitation"]); // D_c(): subtypes the CLI ORIGINATES (sends)
```
- `D_c(req) = fYm.has(req.request.subtype)` is checked when a **control_response** arrives, on a request the CLI had **sent** — i.e. `fYm` is the set of subtypes **the agent loop originates and needs a reply to** (it calls `$$o()` to notify session-state on resolve). **These are the CLI→SDK requests.**
- `x_c(msg) = msg.type==='control_request' && mYm.has(msg.request.subtype)` classifies **inbound** (SDK→CLI) control_requests that the loop should treat specially.

### 5a. Subtypes the CLI **SENDS** (CLI → SDK, awaits control_response)  — `fYm`
| subtype | sender | request fields (key ones) | reply payload |
|---|---|---|---|
| **`can_use_tool`** | `createCanUseTool` + `createSandboxAskCallback` (synthetic tool) + bridge `sendRequest` | tool_name, input, permission_suggestions?, blocked_path?, decision_reason?, decision_reason_type?, classifier_approvable?, title?, display_name?, description?, tool_use_id, agent_id? | `{behavior, updatedInput?/updatedPermissions? \| message?/interrupt?, decisionClassification?, toolUseID?}` |
| **`request_user_dialog`** | tool-driven blocking dialogs (`L_c` builds `dialog:<kind>` synthetic tool_name) | dialog_kind (open string union), payload (opaque per-kind), tool_use_id? | host renders dialog; unknown kinds must answer `{behavior:"cancelled"}`. Timeout `CLAUDE_CODE_USER_DIALOG_TIMEOUT_MS ?? 300000` |
| **`elicitation`** | `handleElicitation` (MCP elicitation) | mcp_server_name, message, mode?(`form`/`url`), url?, elicitation_id?, requested_schema?, title?, display_name?, description? | `ElicitResult` (`{action:'accept'/'decline'/'cancel', content?}`); on failure → `{action:'cancel'}` |
| `hook_callback` | `createHookCallback` | callback_id, input, tool_use_id? | `HookJSONOutput` (NB: not in fYm Set but still a CLI-originated sendRequest) |
| `mcp_message` | `sendMcpMessage` | server_name, message (JSONRPC) | `{mcp_response: JSONRPCMessage}` (CLI-originated sendRequest) |

> Note: `hook_callback` and `mcp_message` are also CLI-originated `sendRequest`s but are **not** in `fYm` (they don't trigger the session-state `$$o()` notify). The strict "CLI sends + session-state-relevant" set is `{can_use_tool, request_user_dialog, elicitation}`.

### 5b. Two SENT-related "synthetic tool" piggybacks on `can_use_tool`
- **Sandbox network ask** (`createSandboxAskCallback`, structuredIO.ts ~L731): sends a `can_use_tool` with `tool_name: SANDBOX_NETWORK_ACCESS_TOOL_NAME`, `input:{host}`, `description: "Allow network connection to <host>?"`, `tool_use_id: randomUUID()`. Returns `result.behavior === 'allow'`. Reuses the permission protocol instead of adding a new subtype.

### 5c. Subtypes the CLI **RECEIVES / HANDLES** (SDK → CLI) — full union literals
From the SDKControlRequest schema region (binary 210318000–210360000), `literal("...")`:
```
initialize, can_use_tool*, interrupt, set_permission_mode, set_model,
set_max_thinking_tokens, set_color, rename_session, mcp_toggle, mcp_status,
mcp_message, mcp_reconnect, mcp_set_servers, mcp_call, set_mcp_permission_mode_override,
hook_callback, elicitation*, request_user_dialog*, message_rated, submit_feedback,
get_settings, get_usage, get_session_cost, get_context_usage, get_binary_version,
read_file, rewind_files, seed_read_state, register_repo_root, reload_plugins,
reload_skills, file_suggestions, background_tasks, stop_task, cancel_async_message,
apply_flag_settings, update_environment_variables, host_auth_token_refresh,
oauth_token_refresh, keep_alive
```
(`*` = also appear as SENT — the schemas are shared; `can_use_tool`/`elicitation`/`request_user_dialog` are defined once and used in both the inbound union the CLI validates and the outbound frames the CLI emits. Direction is determined by sender, not by a separate schema.)

`mYm` (the inbound subtypes the agent-loop main switch treats as buffer-eligible alongside `user`/`bash_command`): `interrupt, set_permission_mode, set_model, set_max_thinking_tokens, set_color, mcp_toggle, message_rated`.

---

## 6. Bridge wiring (forward to claude.ai) — onControlRequestSent/Resolved

`src/cli/print.ts` ~L3987 (Remote Control / bridge enable):
```js
structuredIO.setOnControlRequestSent(request => { handle.sendControlRequest(request); });   // forward can_use_tool → claude.ai
structuredIO.setOnControlRequestResolved(requestId => { handle.sendControlCancelRequest(requestId); }); // cancel stale prompt when SDK consumer wins
// teardown:
structuredIO.setOnControlRequestSent(undefined);
structuredIO.setOnControlRequestResolved(undefined);
```
- `onControlRequestSent` fires inside `sendRequest` **only** when `request.subtype === 'can_use_tool'` (structuredIO.ts L487). So the bridge forwards permission prompts to claude.ai in parallel with the local SDK stdout frame.
- `onControlRequestResolved` fires in `processLine` when a `control_response` resolves a `can_use_tool` request — so the bridge can **cancel the stale claude.ai prompt** when the local SDK consumer answers first. This is a second race layer (local SDK consumer vs claude.ai mobile), distinct from the §4 hook-vs-SDK race.

---

## 7. Telemetry / observability around the deny path

- `tengu_tool_use_can_use_tool_allowed`, `tengu_tool_use_can_use_tool_rejected` — events emitted on the canUseTool result. (rejected example arg: `"Execution stopped by PreToolUse hook"`.)
- **Auto-deny short-circuit event** (binary @ 192683586, a structured SDK event, not the control_request): *"Emitted when a tool call is auto-denied without an interactive permission prompt (e.g. auto-mode classifier, dontAsk mode, headless-agent auto-deny, or a deny rule). The 'ask' path surfaces via a can_use_tool control_request; this event covers the 'deny' short-circuit in canUseTool so SDK hosts can render the denial instead of only seeing an is_error tool_result. PreToolUse hook denies bypass canUseTool and are not covered here."* — confirms: **only the `ask` branch sends `can_use_tool`; `deny`/`allow` short-circuits do not.**

---

## 8. Byte-evidence index (offsets in the oracle binary)

| What | Offset | Anchor |
|---|---|---|
| SDK-consumer `processControlRequest` (reads can_use_tool fields) | 206300637 | `subtype==="can_use_tool"` |
| Bridge `sendRequest` builder (emits can_use_tool) | 207651700 | `subtype:"can_use_tool",tool_name:r,display_name:Fce(r)` |
| React/UI permission handler `f` (receives, builds dialog) | 207905400 | `g.request.subtype!=="can_use_tool"` |
| `can_use_tool` request schema `Bgc` (AUTHORITATIVE fields) | 210324747 | `Bgc=ve(()=>C.object({subtype:C.literal("can_use_tool")` |
| `elicitation` request schema `h_c` | 210342145 | `subtype:C.literal("elicitation"),mcp_server_name` |
| `request_user_dialog` schema `__c` | 210343288 | `subtype:C.literal("request_user_dialog"),dialog_kind` |
| `control_response` envelope `cYm`/`uYm`/`w_c` | 210349524 | `subtype:C.literal("success"),request_id…pending_permission_requests` |
| `mYm`/`fYm` Sets (SENT vs inbound split) | ~210350760 | `mYm=new Set([...]);fYm=new Set(["can_use_tool","request_user_dialog","elicitation"])` |
| `Jyt` result→decision mapper (+`decideLocation:"ask-path"`) | 210350900 | `behavior==="allow"…else if…deny&&e.interrupt…abort()` |
| ALLOW/DENY result schemas `hYm`/`gYm`/`ljt` | ~210351900 | `behavior:za.literal("allow"),updatedInput…interrupt:za.boolean().optional()` |
| `D_c` usage on response resolve | 210360428 | `if(D_c(r.request))$$o()` |
| auto-deny short-circuit event describe | 192683586 | `this event covers the 'deny' short-circuit in canUseTool` |
| `tengu_tool_use_can_use_tool_rejected` | 94331616 | telemetry event |

## 9. Authoritative TS source files (older subset, structure confirmed against bytes)
- `src/cli/print.ts` — `getCanUseToolFn` (L4267), `createCanUseToolWithPermissionPrompt` (L4149), bridge wiring (L3987), control_request/response dispatch loop.
- `src/cli/structuredIO.ts` — `createCanUseTool` (L533), `sendRequest` (L469), `processLine`/control_response handling (L362), `createSandboxAskCallback` (L731), `handleElicitation` (L694), `createHookCallback` (L661), `setOnControlRequestSent/Resolved` (L316/L327), `executePermissionRequestHooksForSDK` (L787).
- `src/utils/permissions/PermissionPromptToolResultSchema.ts` — request `inputSchema` + response `outputSchema` (allow/deny union) + `permissionPromptToolResultToPermissionDecision`.
- `src/entrypoints/sdk/controlSchemas.ts`, `src/entrypoints/agentSdkTypes.ts` — SDK control schema/type surface.
