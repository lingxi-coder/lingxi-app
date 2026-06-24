# CLI-AS-SERVER control_request subtype switch — byte-exact enumeration

**Oracle binary:** `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`
**Version:** 2.1.187 (`VERSION:"2.1.187"`, `BUILD_TIME:"2026-06-23T16:59:46Z"`, `GIT_SHA:"6a53320fad5541a68d79e4b6c53677df77b98e33"` — extracted from the `get_binary_version` handler itself).

This is the switch in the binary's `print.ts` message loop that handles `control_request` frames **sent BY the SDK** (the CLI acting as the server). It is reached when `gt.type==="control_request"` inside the structured-input `for await` loop. The chain is a long `if / else if` on `gt.request.subtype`, ending in a fallthrough error.

Switch body byte location: starts at offset **210472932** (`request.subtype==="interrupt"`), last branch `remote_control` at **210497289**, fallthrough error right after.

## Response / error helpers (byte-exact)

Defined immediately before the loop (offset ~210466000):

```js
let rn=function(wt,xn){                       // sendControlResponseSuccess
  I.enqueue({type:"control_response",
    response:{subtype:"success",request_id:wt.request_id,response:xn}})};
let Dn=function(wt,xn){                        // sendControlResponseError
  I.enqueue({type:"control_response",
    response:{subtype:"error",request_id:wt.request_id,error:xn}})};
```

So EVERY success response frame is:
```json
{"type":"control_response","response":{"subtype":"success","request_id":"<id>","response":<payload-or-undefined>}}
```
and EVERY error response frame is:
```json
{"type":"control_response","response":{"subtype":"error","request_id":"<id>","error":"<string>"}}
```
When a handler calls `rn(gt)` with no second arg, `response` is `undefined` (omitted by JSON serializer). Several handlers fire-and-forget async work, replying later.

**Fallthrough (unknown subtype):**
```js
else Dn(gt,`Unsupported control request subtype: ${gt.request.subtype}`);
continue;
```
(string literal `"Unsupported control request subtype: "` present in binary.)

## Frame envelope (incoming, from SDK)

```json
{"type":"control_request","request_id":"<rand>","request":{"subtype":"<X>", ...fields}}
```
The SDK generates `request_id` as `Math.random().toString(36).substring(2,15)` (Query.request) or `crypto.randomUUID()` (for mcp_message). The CLI validates only that `request` exists; StructuredIO emits `"Error: Missing request on control_request"` otherwise (offsets 192948119 / 210360932).

The CLI can cancel a pending request it is awaiting by sending `{type:"control_cancel_request",request_id}`; `handleControlCancelRequest` aborts the matching AbortController. Each in-flight server request gets its own `AbortController` keyed by `request_id` (duplicate delivery of an in-flight id is skipped with a debug log).

---

# THE 51 SWITCH BRANCHES (in binary order)

Note: branches 1–3 (`can_use_tool`, `request_user_dialog`) are *also* checked here as guards but are CLIENT→ SERVER permission/dialog frames handled on the StructuredIO pending-request path, NOT in this server switch. The server `if/else if` chain proper begins at `interrupt`. The 48 server-handled subtypes are:

### 1. `interrupt`
- **Request:** `{subtype:"interrupt"}` (no fields).
- **Action:** aborts the active turn `abortController` (`H.abort($Mt("remote-cancel"))`), clears task registry escapes, aborts + nulls suggestion state.
- **Response:** `rn(gt)` → `{response: undefined}` (success, empty).
- **Errors:** none.

### 2. `end_session`
- **Request:** `{subtype:"end_session", reason?}`.
- **Action:** If `eTc(reason, CLAUDE_CODE_WORKER_EPOCH)` → stale 'archived' on epoch>1 is ignored (logs `[print.ts] stale 'archived' end_session ignored on epoch>1`, replies success, `continue`). Otherwise logs `[print.ts] end_session received, reason=${reason??"unspecified"}`, aborts turn + suggestion state, replies success, **`break`s the loop** (drains then exits).
- **Response:** `rn(gt)` empty success.
- **Errors:** none.

### 3. `initialize`
- **Request:** `{subtype:"initialize", title?, sdkMcpServers?:string[], webSearchIsolationExemptMcpServers?, systemPrompt?:string|string[], appendSystemPrompt?, planModeInstructions?, appendSubagentSystemPrompt?, toolAliases?, excludeDynamicSections?, agents?, skills?, promptSuggestions?, agentProgressSummaries?, forwardSubagentText?, hooks?, jsonSchema?, supportedDialogKinds?}`.
- **Action:** delegates to `iJm(...)` = handleInitializeRequest (offset 210506291). Registers SDK MCP server placeholders (`{type:"sdk",name}`), sets session title, applies systemPrompt/appendSystemPrompt/planModeInstructions/appendSubagentSystemPrompt/toolAliases/excludeDynamicSections/promptSuggestions/forwardSubagentText/skills/agents/hooks/jsonSchema/supportedDialogKinds, restricts agent model (returns `restrictedAgentModel`), enables prompt suggestions + agent progress summaries.
- **Response payload** (built by `zyc()`):
  ```js
  {
    commands: [{name, description, argumentHint, aliases?}],   // userInvocable!==false
    agents:   [{name, description, model?}],                   // model omitted if "inherit"
    output_style: <string>,
    available_output_styles: <string[]>,
    models: <ModelInfo[]>,
    unavailable_models?: <ModelInfo[]>,                        // only if length>0
    account: {email, organization, subscriptionType, tokenSource, apiKeySource, apiProvider},
    pid: process.pid,
    feedback_survey_config: <obj>,
    fast_mode_state?: <obj>                                    // only when $l()&&vk()
  }
  ```
- **Re-initialize path** (when already initialized `n===true`): fires `tengu_reinit_pending_redelivery`, and the success response additionally carries **siblings** `pending_permission_requests` and `pending_user_dialog_requests` (arrays of in-flight `can_use_tool` / `request_user_dialog` frames) so a re-joining client can re-arm dialogs:
  ```json
  {"subtype":"success","request_id":"...","response":{...zyc...},
   "pending_permission_requests":[...],"pending_user_dialog_requests":[...]}
  ```
- After the success enqueue it may also enqueue a separate `{type:"auth_status",...}` message (when `enableAuthStatus`).
- **Errors:** none in the switch itself (handler always replies success).

### 4. `set_permission_mode`
- **Request:** `{subtype:"set_permission_mode", mode, ultraplan?}`. `mode` ∈ default|plan|acceptEdits|bypassPermissions|auto.
- **Action:** `aJm(request, request_id, toolPermissionContext, transport)`. Updates `toolPermissionContext` and `isUltraplanMode`.
- **Response:** success `{mode}`.
- **Errors (via Dn inside aJm):**
  - mode `bypassPermissions` when disabled by settings (`N2()`): `"Cannot set permission mode to bypassPermissions because it is disabled by settings or configuration"`.
  - mode `bypassPermissions` when session not launched with the flag: `"Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions"`.
  - mode `auto` when not allowed (`!cv()`): `"Cannot set permission mode to auto"` or `"Cannot set permission mode to auto: ${PJ(...)}"`.

### 5. `set_model`
- **Request:** `{subtype:"set_model", model?}`. `model` defaults to `"default"`; `"default"` (case-insensitive) resolves to `Kg()`.
- **Action:** validates model (`XT`/`dF`/`$a`). If unresolvable → error. Else sets `mainLoopModelForSession`, notifies metadata.
- **Response:** success (empty, `rn(gt)`).
- **Errors:** `Dn(gt, fte(model, fallback))` — model-not-available message when the requested model is unknown and not a fallback.

### 6. `set_max_thinking_tokens`
- **Request:** `{subtype:"set_max_thinking_tokens", max_thinking_tokens, thinking_display?}`.
- **Action:** sets `se = thinking_display`, `re = jyc(max_thinking_tokens, se)`.
- **Response:** success empty.
- **Errors:** none.

### 7. `mcp_status`
- **Request:** `{subtype:"mcp_status"}`.
- **Action:** none beyond read.
- **Response:** success `{mcpServers: nn()}` (`nn()` = current MCP server status list).
- **Errors:** none.

### 8. `get_binary_version`   *(NEW in 2.1.187 vs leaked TS)*
- **Request:** `{subtype:"get_binary_version"}`.
- **Response:** success `{version:"2.1.187"+IU(), buildTime:"2026-06-23T16:59:46Z"}` (VERSION + BUILD_TIME literals are inlined in the handler).
- **Errors:** none.

### 9. `get_context_usage`
- **Request:** `{subtype:"get_context_usage"}`.
- **Action:** `J8t({messages, getAppState, options:{mainLoopModel, tools, agentDefinitions, customSystemPrompt, appendSystemPrompt, excludeDynamicSections}})`.
- **Response:** success `{...contextUsage}` (token-budget breakdown).
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 10. `get_session_cost`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"get_session_cost"}`.
- **Response:** success `{text: cc(F0e())}` (formatted cost string).
- **Errors:** none.

### 11. `get_usage`   *(NEW in 2.1.187; SDK method is usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET)*
- **Request:** `{subtype:"get_usage"}`.
- **Action:** `W8t()`.
- **Response:** success `{...usage}`.
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 12. `mcp_message`
- **Request:** `{subtype:"mcp_message", server_name, message}` (JSON-RPC message for an SDK MCP server).
- **Action:** finds connected server by name; forwards `message` to `client.transport.onmessage`.
- **Response:** success empty (`rn(gt)`). (Note: the SERVER side here just forwards; it does not return mcp_response. The mcp_response wrapping is on the SDK-CLIENT `processControlRequest` side.)
- **Errors:** none in switch (silently no-ops if server not found/connected).

### 13. `rewind_files`
- **Request:** `{subtype:"rewind_files", user_message_id, dry_run?}`.
- **Action:** `cTc(user_message_id, appState, dry_run??false)`.
- **Response:** success = the rewind result `{canRewind, filesChanged?, insertions?, deletions?}` (when `canRewind` or `dry_run`).
- **Errors:** `Dn(gt, result.error ?? "Unexpected error")`. Handler-internal error strings: `"File rewinding is not enabled."`, `"No file checkpoint found for this message."`, `"Failed to rewind: ${...}"`.

### 14. `cancel_async_message`
- **Request:** `{subtype:"cancel_async_message", message_uuid}`.
- **Action:** finds queued messages with that uuid; if none, `F8i(uuid)`.
- **Response:** success `{cancelled: <bool>}`.
- **Errors:** none.

### 15. `rewind_conversation`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"rewind_conversation", target_message_uuid}`.
- **Action:** rewinds message history to a user message; finds preceding assistant uuid; truncates `B.splice(idx)`.
- **Response (always success-shaped, embeds outcome):**
  - failure: `{rewound:false, prefillText:null, precedingAssistantUuid:null, error:"<reason>"}` where reason ∈ `"turn running"` | `"target not found"` | `"no preceding assistant"` | `"failed to persist rewind anchor"` | `"state changed"`. Also `"commands queued"` (via `error:kn?"turn running":"commands queued"`).
  - success: `{rewound:true, targetMessageUuid, prefillText, precedingAssistantUuid}`.
- **Errors:** uses success frames with `error` field embedded (does NOT use `Dn`).

### 16. `read_file`
- **Request:** `{subtype:"read_file", path, max_bytes?, encoding?}`.
- **Action:** lazy-imports `readFileForRemote`; reads via `toolPermissionContext`.
- **Response:** success = the readFileForRemote result.
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 17. `stage_file`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"stage_file", ...}` (mount_path-based; passed straight to `stageFile`).
- **Action:** async; sends `{type:"keep_alive"}` every 30s while staging; lazy-imports `stageFile`.
- **Response:** success = the stage result `On` (when `On.ok`).
- **Errors:** `Dn(gt, On.error)` when not ok, `Dn(gt, Ce(err))` on throw.

### 18. `register_repo_root`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"register_repo_root", directory, reload_claude_md?, reload_skills?, reload_plugins?}`.
- **Action:** handler `Io`. `realpath`s cwd and `directory`; requires `directory` be a subdirectory of cwd; adds it to addDirectories tool-permission context + KH() dir list; optionally reloads CLAUDE.md / skills / plugins.
- **Response:** success `{directory: <realpath>}`.
- **Errors:** `Dn(gt, Ce(err))`. Handler throws `register_repo_root: ${directory} is not a subdirectory of cwd` when not a subdir.

### 19. `add_directory`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"add_directory", mount_path}`.
- **Action:** handler `Er` (async, keep_alive every 30s). Resolves `mount_path` → host path (`destFromMountPath`); requires env `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`; stages the file (force) and adds parent dir.
- **Response:** success `{staged_path, directory}`.
- **Errors:** `Dn(gt, Ce(err))` for bad mount path; `Dn(gt, "add_directory requires CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD to be set in the container environment")`; `Dn(gt, Nr.error)` when staging fails.

### 20. `file_suggestions`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"file_suggestions", query}`.
- **Action:** `generateFileSuggestions(globalFileIndexCache, query, true)`.
- **Response:** success `{suggestions: [{path}]}` (path = `displayText`).
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 21. `seed_read_state`
- **Request:** `{subtype:"seed_read_state", path, mtime}`.
- **Action:** stats file; if file mtime ≤ provided mtime, reads it (strips BOM, normalizes CRLF→LF) and seeds the read-state cache `z`.
- **Response:** success empty (`rn(gt)`).
- **Errors:** none (errors swallowed by inner try/catch).

### 22. `mcp_set_servers`
- **Request:** `{subtype:"mcp_set_servers", servers}` (map of server name → config).
- **Action:** `Nt(servers, {authoritative:true, caller:"mcp_set_servers"})`; refreshes commands if sdkServersChanged.
- **Response:** success = `Xt` (the diff/apply response).
- **Errors:** none in switch.

### 23. `reload_plugins`
- **Request:** `{subtype:"reload_plugins"}`.
- **Action:** reloads plugins, agents (preserves flagSettings agents), commands, applies plugin MCP diff.
- **Response:** success `{commands:[{name,description,argumentHint,aliases?}], agents:[{name,description,model?}], plugins:[{name,path,source}], mcpServers, error_count}`.
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 24. `reload_skills`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"reload_skills"}`.
- **Action:** reloads skills + commands.
- **Response:** success `{skills:[{name,description,argumentHint,aliases?}]}`.
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 25. `mcp_reconnect`
- **Request:** `{subtype:"mcp_reconnect", serverName}`.
- **Action:** resolves server config across all client lists; reconnects via `Ej`; rebuilds tools/commands/resources.
- **Response:** success empty (`rn(gt)`) when client connects.
- **Errors:** `Dn(gt, "Server not found: ${serverName}")`; `Dn(gt, "MCP server ${serverName} is blocked by enterprise managed policy")`; `Dn(gt, "Connection failed" | "Server status: ${type}")` when not connected.

### 26. `mcp_call`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"mcp_call", tool, arguments?}`. `tool` = fully-qualified MCP tool name.
- **Action:** parses FQN; finds connected (non-SDK) server; invokes the tool (`pZr`). Honors turn abort signal.
- **Response:** success `{content, structuredContent, _meta}`.
- **Errors (via Dn):**
  - `"Not a fully-qualified MCP tool name: ${tool}"`
  - `"MCP server not connected: ${serverName}"`
  - `"mcp_call does not support SDK MCP servers. SDK servers are caller-provided — invoke ${serverName} directly."`
  - `"URL elicitation required (open URL, then retry mcp_call): ${url}..."`
  - `"MCP session expired for ${serverName} — send mcp_reconnect and retry mcp_call: ${msg}"`
  - generic `Ce(err)`.

### 27. `mcp_toggle`
- **Request:** `{subtype:"mcp_toggle", serverName, enabled}`.
- **Action:** disables (disconnect + mark `disabled`) or enables (reconnect) a server.
- **Response:** success empty (`rn(gt)`).
- **Errors:** `Dn(gt, "Server not found: ${serverName}")`; `Dn(gt, "MCP server ${serverName} is blocked by enterprise managed policy")` (on enable); `Dn(gt, "Connection failed" | "Server status: ${type}")` when enable fails.

### 28. `set_mcp_permission_mode_override`   *(NEW in 2.1.187; SDK method setMcpPermissionModeOverride)*
- **Request:** `{subtype:"set_mcp_permission_mode_override", serverName, mode}`. `mode` is tighten-only: `'default'`, `'auto'`, or `null`.
- **Action:** validates via `Pla(mode)` (tighten-only); applies `mcpPermissionModeOverrides[serverName]`.
- **Response:** success — `undefined` when server is known; otherwise success with a `{warning}` payload:
  - `{warning:"MCP server '${serverName}' is not known; no override was present to clear."}`
  - `{warning:"MCP server '${serverName}' is not yet known; override stored but will not apply until a server with that exact name connects."}`
- **Errors (via Dn):**
  - `"Permission mode override over the control channel is tighten-only ('default', 'auto', or null); rejected '${rejected}'"`
  - `"Cannot pin MCP server '${serverName}' to auto"` / `"Cannot pin MCP server '${serverName}' to auto: ${...}"`.

### 29. `channel_enable`
- **Request:** `{subtype:"channel_enable", serverName}`.
- **Action:** handler `lJm`. Requires a connected, plugin-sourced (marketplace) server; registers channel notification handler.
- **Response:** success `{response: undefined}` (`rn` via direct enqueue with `response:void 0`).
- **Errors (via inline Dn-equivalent):** `"server ${serverName} is not connected"`; `"server ${serverName} is not plugin-sourced; channel_enable requires a marketplace plugin"`; plus a `skip` reason from `kpt(...)`.

### 30. `mcp_authenticate`
- **Request:** `{subtype:"mcp_authenticate", serverName, redirectUri?}`.
- **Action:** builds OAuth flow. For claude.ai proxy connector → returns auth URL; for localhost/custom redirect → starts PKCE flow, races for auth URL.
- **Response (success):**
  - claude.ai connector: `{authUrl, requiresUserAction:true, callbackExpected:false}`.
  - OAuth flow with URL: `{authUrl, requiresUserAction:true, callbackExpected:true, redirectScheme, state, callbackPort?}` (`callbackPort` only when scheme==="localhost").
  - already-authed/no URL: `{requiresUserAction:false, callbackExpected:false}`.
- **Errors (via Dn):** `"Server not found: ${serverName}"`; `"Unable to build claude.ai connector auth URL (missing org or server id)"`; `'Server type "${transport}" does not support OAuth authentication'`; anthropic-hosted message; generic `Ce(err)`.

### 31. `mcp_oauth_callback_url`
- **Request:** `{subtype:"mcp_oauth_callback_url", serverName, callbackUrl}`.
- **Action:** delivers the OAuth redirect URL to the waiting flow; awaits token exchange.
- **Response:** success empty (`rn(gt)`).
- **Errors (via Dn):** `"Invalid callback URL: missing authorization code. Please paste the full redirect URL including the code parameter."`; `"No active OAuth flow for server: ${serverName}"`; OAuth-failure message.

### 32. `claude_authenticate`
- **Request:** `{subtype:"claude_authenticate", loginWithClaudeAi?}`.
- **Action:** single-slot Anthropic OAuth flow (`QW`). Cleans up any prior flow; starts PKCE + localhost listener; produces manual + automatic URLs.
- **Response:** success `{manualUrl, automaticUrl}`.
- **Errors:** `Dn(gt, Ce(err))`.

### 33. `claude_oauth_callback`  (shares branch with #34)
- **Request:** `{subtype:"claude_oauth_callback", authorizationCode, state}`.
- **Action:** feeds manual auth code (`handleManualAuthCodeInput`) into the active flow; awaits completion.
- **Response:** success `{account:{email, organization, subscriptionType, tokenSource, apiKeySource, apiProvider}}`.
- **Errors:** `Dn(gt, "No active claude_authenticate flow")`; flow-rejection `Ce(err)`.

### 34. `claude_oauth_wait_for_completion`  (shares branch with #33)
- **Request:** `{subtype:"claude_oauth_wait_for_completion"}`.
- **Action:** awaits the in-flight Anthropic OAuth flow without feeding a code.
- **Response:** success `{account:{...}}` (same shape as #33).
- **Errors:** `Dn(gt, "No active claude_authenticate flow")`; `Ce(err)`.

### 35. `mcp_clear_auth`
- **Request:** `{subtype:"mcp_clear_auth", serverName}`.
- **Action:** clears stored OAuth creds (`Nge`) then reconnects; only valid for `sse`/`http` server types.
- **Response:** success `{}` (`rn(gt,{})`).
- **Errors (via Dn):** `"Server not found: ${serverName}"`; `'Cannot clear auth for server type "${type}"'`; `"MCP server ${serverName} is blocked by enterprise managed policy"`.

### 36. `apply_flag_settings`
- **Request:** `{subtype:"apply_flag_settings", settings}`. `settings` may contain `agent`, `model`, `effortLevel`, `ultracode`, `viewMode`, etc. `null` values delete the key.
- **Action:** applies flag settings: agent switch (via `gyc`, may set systemPrompt), model change, effort level, ultracode (`effortValue:"xhigh"`), viewMode.
- **Response:** success empty (`rn(gt)`).
- **Errors:** `Dn(gt, agentResult.error)` when agent switch fails (then `continue`).

### 37. `get_settings`
- **Request:** `{subtype:"get_settings"}`.
- **Action:** reads effective settings (`pbr()`), applied model/effort/ultracode, config errors (non-warning severity).
- **Response:** success `{...settings, applied:{model, effort, ultracode}, errors?:[{file,path,message}]}`.
- **Errors:** none.

### 38. `stop_task`
- **Request:** `{subtype:"stop_task", task_id}`.
- **Action:** `Aqt(task_id, {taskRegistry, setAppState})`.
- **Response:** success `{}`. Also returns `{}` when the task is already `not_found`/`not_running` (treated as success).
- **Errors:** `Dn(gt, Ce(err))` for other errors.

### 39. `background_tasks`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"background_tasks", tool_use_id?}`.
- **Action:** if `tool_use_id` → background that specific tool (`qTo`); else background all (`$5e`).
- **Response:** success `{backgrounded: <bool>}` (with tool_use_id) or `{}` (all).
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 40. `generate_session_title`
- **Request:** `{subtype:"generate_session_title", description, persist?}`.
- **Action:** async; if `persist` sets `xn=true`; generates title via `Aue`; saves if persist.
- **Response:** success `{title}`.
- **Errors:** `Dn(gt, Ce(err))`.

### 41. `rename_session`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"rename_session", title}`.
- **Action:** trims title; persists rename (remote-origin) or updates in-memory title.
- **Response:** success empty (`rn(gt)`).
- **Errors:** `Dn(gt, "title must be non-empty")` for empty; `Dn(gt, Ce(err))` on throw.

### 42. `submit_feedback`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"submit_feedback", description, surface?}`. `surface` defaults to `"sdk"`.
- **Action:** async; submits feedback (`n8t`).
- **Response (success):**
  - unavailable: `{feedback_id:null, unavailable_reason:<reason>}`.
  - success: `{feedback_id, ccshare_url?}`.
  - failure: `{feedback_id:null, is_zdr_org, failure_reason, status_code}`.
- **Errors:** `Dn(gt, Ce(err))` on throw.

### 43. `side_question`
- **Request:** `{subtype:"side_question", question}`.
- **Action:** async; asks a side question against current cache-safe params (`oVn`).
- **Response:** success `{response, synthetic}`.
- **Errors:** `Dn(gt, Ce(err))`.

### 44. `ultrareview_launch`   *(NEW in 2.1.187)*
- **Request:** `{subtype:"ultrareview_launch", args?, confirm?}`. `args` default `""`, `confirm` default `false`.
- **Action:** async; launches ultrareview (`Ajn`); on `status==="launched"` injects `<command-name>/ultrareview ...</command-name>` user messages.
- **Response:** success = the launch result `Gn` (`{status, message, ...}`).
- **Errors:** `Dn(gt, Ce(err))`.

### 45. `message_rated`
- **Request:** `{subtype:"message_rated", messageUuid, sentiment, surface?, cleared?}`. `surface` default `"tool_use"`, `cleared` default `false`.
- **Action:** if `allow_product_feedback` gate set, fires `tengu_message_rated` telemetry.
- **Response:** success `{}` (`rn(gt,{})`).
- **Errors:** none.

### 46. `remote_control`
- **Request:** `{subtype:"remote_control", enabled, name?}`.
- **Action (enabled=true):** initializes the REPL bridge (`initReplBridge`) with tool/permission/model/interrupt callbacks; wires `onControlRequestSent`/`onControlRequestResolved`. If already running returns existing session URLs.
- **Response (enabled, success):** `{session_url, connect_url, environment_id}`.
- **Action (enabled=false):** tears down the bridge (`reason:"remote_control_disabled"`).
- **Response (disable):** success empty (`rn(gt)`).
- **Errors:** `Dn(gt, error??"Remote Control initialization failed")`; `Dn(gt, Ce(err))`.

---

# SDK-CLIENT SIDE (the inverse switch) — for completeness

The SDK's `Query.processControlRequest` (offset ~206301000) handles requests the **CLI sends to the SDK** (server→client). These are NOT in the print.ts server switch but complete the protocol picture. Final fallthrough: `throw Error("Unsupported control request subtype: "+subtype)`.

- `can_use_tool` → `{...await canUseTool(tool_name, input, {signal, suggestions:permission_suggestions, blockedPath:blocked_path, decisionReason:decision_reason, title, displayName:display_name, description, toolUseID:tool_use_id, agentID:agent_id}), toolUseID:tool_use_id}`. Throws `"canUseTool callback is not provided."`.
- `hook_callback` → `handleHookCallbacks(callback_id, input, tool_use_id, signal)`. Throws `"No hook callback found for ID: ${id}"`.
- `mcp_message` → `{mcp_response: <jsonrpc>}` (server→client direction; routes to the SDK's in-process MCP server `sdkMcpTransports.get(server_name)`). Throws `"SDK MCP server not found: ${server_name}"`.
- `elicitation` → `onElicitation({serverName:mcp_server_name, message, mode, url, elicitationId:elicitation_id, requestedSchema:requested_schema, title, displayName:display_name, description}, {signal})`; default `{action:"decline"}`.
- `request_user_dialog` → `onUserDialog({dialogKind:dialog_kind, payload, toolUseID:tool_use_id}, {signal})`; if no handler, stays silent (`fFl` sentinel) and fires `tengu_request_user_dialog_response_ignored`.
- `oauth_token_refresh` → `{accessToken: await getOAuthToken({signal}) ?? null}`. Throws `"getOAuthToken callback is not provided."`.
- `host_auth_token_refresh` → `{authToken: await getHostAuthToken({signal}) ?? null}`. Throws `"getHostAuthToken callback is not provided."`.

SDK→CLI request methods (what the SDK sends, mirror of the server switch): `initialize`, `interrupt`, `set_permission_mode`, `set_mcp_permission_mode_override`, `set_model`, `set_max_thinking_tokens`, `apply_flag_settings`, `get_settings`, `rewind_files`, `cancel_async_message`, `seed_read_state`, `remote_control`, `submit_feedback`, `generate_session_title`, `side_question`, `ultrareview_launch`, `message_rated`, `mcp_reconnect`, `mcp_toggle`, `channel_enable`, `mcp_authenticate`, `mcp_clear_auth`, `mcp_oauth_callback_url`, `claude_authenticate`, `claude_oauth_callback`, `claude_oauth_wait_for_completion`, `mcp_status`, `get_context_usage`, `get_usage`, `read_file`, `reload_plugins`, `reload_skills`, `mcp_set_servers`.

---

# WHICH ARE 2.1.187-ONLY (vs the leaked OLDER TS)

The leaked TS (`cli/print.ts`, `entrypoints/sdk/controlSchemas.ts`) is a SUBSET (older). Comparing the leaked `request.subtype ===` chain (29 subtypes) against the 2.1.187 binary's switch (48 server subtypes) — these **22 subtypes are present in the binary but ABSENT from the leaked TS**, i.e. added after the leak (most plausibly 2.1.187 or a recent prior release):

`get_binary_version`, `get_session_cost`, `get_usage`, `rewind_conversation`, `read_file`, `stage_file`, `register_repo_root`, `add_directory`, `file_suggestions`, `reload_skills`, `mcp_call`, `set_mcp_permission_mode_override`, `rename_session`, `background_tasks`, `submit_feedback`, `ultrareview_launch`, `message_rated`.

(That is 17 confirmed NOT-in-leaked-print.ts. Caveat: a few — e.g. `read_file`, `file_suggestions` — appear in the binary's REPL/bridge handlers and the SDK `Query` class, so they may predate 2.1.187 even though the leaked print.ts switch lacks them. The leaked controlSchemas only declares 23 schema literals and is even more out of date.)

**Subtypes shared (present in BOTH leaked TS and binary, NOT 187-only):** `interrupt`, `end_session`, `initialize`, `set_permission_mode`, `set_model`, `set_max_thinking_tokens`, `mcp_status`, `get_context_usage`, `mcp_message`, `rewind_files`, `cancel_async_message`, `seed_read_state`, `mcp_set_servers`, `reload_plugins`, `mcp_reconnect`, `mcp_toggle`, `channel_enable`, `mcp_authenticate`, `mcp_oauth_callback_url`, `claude_authenticate`, `claude_oauth_callback`, `claude_oauth_wait_for_completion`, `mcp_clear_auth`, `apply_flag_settings`, `get_settings`, `stop_task`, `generate_session_title`, `side_question`, `remote_control`.

## CANNOT precisely date which release added each new subtype
The leak is an unknown-but-older snapshot, not specifically 2.1.186. So "2.1.187-only" cannot be asserted at single-version granularity from these two artifacts alone — only "newer than the leak." A diff against the 2.1.186 binary would be required to pin exactly the 187-delta. The binary IS authoritative for the 2.1.187 set (the 48 server subtypes above are exhaustive and byte-verified from the switch body).
