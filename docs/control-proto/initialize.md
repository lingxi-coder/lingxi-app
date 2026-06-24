# Control Protocol: `initialize` Handshake

Oracle binary: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`
Method: byte-extraction via `tail -c +OFF | head -c N | tr -d '\0'`. Minified symbol names (`zyc`, `k3E`, `sjt`, …) are local and not stable across builds; the JSON **string-literal key sets + Zod `.describe()` text** below are the load-bearing evidence.

---

## 0. TL;DR — the wire shapes

**SDK → CLI** (`control_request`):
```jsonc
{ "type":"control_request", "request_id":"<id>",
  "request": { "subtype":"initialize", /* hooks?, sdkMcpServers?, jsonSchema?, systemPrompt?, … */ } }
```

**CLI → SDK** (`control_response`, success):
```jsonc
{ "type":"control_response",
  "response": { "subtype":"success", "request_id":"<id>",
    "response": { /* the initialize payload — see §2 */ } } }
```

The inner `response.response` object is built by runtime helper `zyc()` (the "`iJm`"/binary-helper the task refers to) and validated by Zod schema `k3E` (=`iJm`-around-`iJm`). Two paths produce it:

1. **Main SDK control handler** → `zyc(...)` (real arrays). This is the canonical payload.
2. **REPL bridge fallback** (Remote-Control web/mobile, `initReplBridge`) → emits empty arrays `commands:[],agents:[],…` and **spreads `...s?.()`** where `s = getInitializeState()` injects `current_model` + `current_permission_mode`.

> Key correction: `slash_commands`, `mcp_servers`, `roots`, `model`, `permissionMode`, `plugins` are **NOT** keys of the initialize control-response. They belong to a *different* message — the `type:"system", subtype:"init"` system message schema (see §6). The task's candidate list conflated the two. The control-response initialize payload key set is exactly §2.

---

## 1. The REQUEST (SDK-declared)

### 1.1 Runtime construction (SDK side) — `initialize()` method
Byte offset `206303422` (`subtype:"initialize"`):

```js
async initialize(){
  let e;                                  // hooks → callback-id form
  if(this.hooks){ e={};
    for(let[o,s] of Object.entries(this.hooks)) if(s.length>0)
      e[o]=s.map((i)=>{ let a=[];
        for(let l of i.hooks){ let c=`hook_${this.nextCallbackId++}`;
          this.hookCallbacks.set(c,l); a.push(c); }
        return {matcher:i.matcher, hookCallbackIds:a, timeout:i.timeout}; }); }
  let t=this.sdkMcpTransports.size>0 ? Array.from(this.sdkMcpTransports.keys()) : void 0,
    n={ subtype:"initialize",
        hooks:e,
        sdkMcpServers:t,
        jsonSchema:this.jsonSchema,
        systemPrompt: typeof this.initConfig?.systemPrompt==="string"
                       ? [this.initConfig.systemPrompt] : this.initConfig?.systemPrompt,
        appendSystemPrompt:this.initConfig?.appendSystemPrompt,
        planModeInstructions:this.initConfig?.planModeInstructions,
        appendSubagentSystemPrompt:this.initConfig?.appendSubagentSystemPrompt,
        toolAliases:this.initConfig?.toolAliases,
        excludeDynamicSections:this.initConfig?.excludeDynamicSections,
        agents:this.initConfig?.agents,
        title:this.initConfig?.title,
        skills:Array.isArray(this.initConfig?.skills)?this.initConfig.skills:void 0,
        webSearchIsolationExemptMcpServers:this.initConfig?.webSearchIsolationExemptMcpServers,
        promptSuggestions:this.initConfig?.promptSuggestions,
        agentProgressSummaries:this.initConfig?.agentProgressSummaries,
        forwardSubagentText:this.initConfig?.forwardSubagentText,
        supportedDialogKinds:this.initConfig?.supportedDialogKinds };
  return (await this.request(n)).response; }
```

NOTE on hooks transform: client-side `hooks: Record<event, [{matcher?, hooks:[fn], timeout?}]>` is rewritten on the wire to `Record<event, [{matcher?, hookCallbackIds:[string], timeout?}]>` — the callbacks are replaced with opaque ids `hook_<n>` and stored in `this.hookCallbacks` for later `hook_callback` control-requests. Entries with empty `hooks` arrays are dropped (`if(s.length>0)`).
NOTE on `sdkMcpServers`: it's **just the list of names/keys** of in-process SDK MCP transports (`Array.from(this.sdkMcpTransports.keys())`), NOT server configs. `undefined` when none.

### 1.2 Request Zod schema
Byte offset `210318607` (`subtype:C.literal("initialize")`) … ends at description offset `210321664`:

```
C.object({
  subtype: C.literal("initialize"),
  hooks: C.record(Cgc(), C.array(oYm())).optional(),
  sdkMcpServers: C.array(C.string()).optional(),
  jsonSchema: C.record(C.string(), C.unknown()).optional(),
  systemPrompt: C.array(C.string()).optional(),
  appendSystemPrompt: C.string().optional(),
  planModeInstructions: C.string().optional()
     .describe("Custom workflow body for the plan-mode system reminder. Replaces the default
                code-implementation phases; the CLI still wraps it with the read-only enforcement
                preamble and the ExitPlanMode protocol footer."),
  appendSubagentSystemPrompt: C.string().optional()
     .describe("@internal Additional system prompt appended to every Task-tool subagent (and
                propagated to nested subagents). Gated by CLAUDE_CODE_ENABLE_APPEND_SUBAGENT_PROMPT."),
  toolAliases: C.record(C.string(), C.string()).optional()
     .describe("Map of tool-name aliases applied before name resolution. … Single-hop (no chains).
                See Options.toolAliases."),
  excludeDynamicSections: C.boolean().optional()
     .describe("When true, omit per-user dynamic sections (working directory, auto-memory path)
                from the cached system prompt and re-inject them as the first user message. Lets
                cross-user prompt caching hit on a static system prompt prefix. …"),
  agents: C.record(C.string(), kgc()).optional(),
  title: C.string().optional()
     .describe("Custom session title. … Has no effect on the persisted title when resuming …"),
  skills: C.array(C.string()).optional()
     .describe("When provided, only skills whose names match an entry are loaded into the main
                session system prompt … Applies to the main session only; subagents use
                AgentDefinition.skills."),
  webSearchIsolationExemptMcpServers: C.array(C.string()).optional()
     .describe("@internal Additional MCP server names exempt from the web search / connector
                isolation latch. Unioned with the built-in infra-server list."),
  promptSuggestions: C.boolean().optional(),
  agentProgressSummaries: C.boolean().optional(),
  forwardSubagentText: C.boolean().optional(),
  supportedDialogKinds: C.array(C.string()).optional()
     .describe("Dialog kinds (request_user_dialog `dialog_kind` values) this consumer's onUserDialog
                can actually render. The CLI treats ABSENCE as 'cannot display' and fails closed …
                First-attached-client-wins on multi-client sessions; later initializes do not change it.")
}).describe("Initializes the SDK session with hooks, MCP servers, and agent configuration.")
```

### 1.3 Request sub-schemas
**Hook event-name enum** `Cgc = C.enum(czm)` where `czm` (offset `210244686`):
```
["PreToolUse","PostToolUse","PostToolUseFailure","PostToolBatch","Notification",
 "UserPromptSubmit","UserPromptExpansion","SessionStart","SessionEnd","Stop","StopFailure",
 "SubagentStart","SubagentStop","PreCompact","PostCompact","PermissionRequest","PermissionDenied",
 "Setup","TeammateIdle","TaskCreated","TaskCompleted","Elicitation","ElicitationResult",
 "ConfigChange","WorktreeCreate","WorktreeRemove","InstructionsLoaded","C…"]   (list continues)
```

**Hook matcher entry** `oYm` (offset `210318xxx`):
```
C.object({ matcher:C.string().optional(),
           hookCallbackIds:C.array(C.string()),
           timeout:C.number().optional() })
  .describe("Configuration for matching and routing hook callbacks…")
```

**Agent definition** `kgc` (offset `210264xxx`, the value type of request `agents`):
```
C.object({
  description: C.string().describe("Natural language description of when to use this agent"),
  tools: C.array(C.string()).optional()
     .describe("Array of allowed tool names. If omitted, inherits all tools from parent.
                Note: passing 'Skill' here is deprecated — use the `skills` field instead."),
  disallowedTools: C.array(C.string()).optional().describe("Array of tool names to explicitly disallow…"),
  … (continues)
})
```

---

## 2. The RESPONSE payload — KEY SET (authoritative)

### 2.1 Runtime builder `zyc()` (the "iJm" helper)
Byte offset of `available_output_styles:` at `210509330`; builder body:

```js
async function zyc(e,t,n,r,o,s){
  let a = $o()?.outputStyle || x1,        // output_style: current, else default x1 ("normal")
      l = await Wft(Lt()),                // l = available output-style records
      c = $Be();                          // c = account info
  let u = {
    commands: e.filter((d)=>d.userInvocable!==!1)
               .map((d)=>({ name:mu(d), description:gne(d),
                            argumentHint:d.argumentHint||"",
                            aliases:d.aliases?.length?d.aliases:void 0 })),
    agents: t.map((d)=>({ name:d.agentType, description:d.whenToUse,
                          model:d.model==="inherit"?void 0:d.model })),
    output_style: a,
    available_output_styles: Object.keys(l),
    models: n,
    ...(r.length>0 && { unavailable_models: r }),   // only present when non-empty
    account: { email:c?.email, organization:c?.organization,
               subscriptionType:c?.subscription, tokenSource:c?.tokenSource,
               apiKeySource:c?.apiKeySource, apiProvider:Rr() },
    pid: process.pid,
    feedback_survey_config: pyc()          // present per host gate (see §4)
  };
  if($l() && vk()){ let d=o(); u.fast_mode_state = lF(s??null, d.fastMode); }  // conditional
  return u;
}
```

### 2.2 RESPONSE key set (exact)
| key | type | presence | source |
|---|---|---|---|
| `commands` | array of `{name,description,argumentHint,aliases?}` | always | slash-command list, `userInvocable!==false` filtered |
| `agents` | array of `{name,description,model?}` | always | subagents; `model` omitted when `"inherit"` |
| `output_style` | string | always | active style name, fallback `x1`="normal" |
| `available_output_styles` | array<string> | always | `Object.keys(<style records>)` |
| `models` | array of model objects (see §3) | always | selectable models |
| `unavailable_models` | array of model objects | **only when non-empty** | spread `...(r.length>0 && {…})` |
| `account` | object (see §3) | always | `{email,organization,subscriptionType,tokenSource,apiKeySource,apiProvider}` |
| `pid` | number | always | `process.pid` |
| `feedback_survey_config` | object (see §4) | host-gated (can be undefined) | `pyc()` |
| `fast_mode_state` | enum `"off"\|"cooldown"\|"on"` | **only if** `$l()&&vk()` | `lF(...)` |
| `current_model` | string | **bridge path only** (`...getInitializeState()`) | §5 |
| `current_permission_mode` | enum (permission mode) | **bridge path only** | §5 |

### 2.3 RESPONSE Zod schema `k3E` (offset `210322558`)
```
C.object({
  commands: C.array(sjt()),
  agents:   C.array(D$o()),
  output_style: C.string(),
  available_output_styles: C.array(C.string()),
  models: C.array(P$o()),
  unavailable_models: C.array(P$o()).optional()
     .describe("@internal Models the account can see but not select (disabled:true, reason folded
                into description … e.g. a model the org's Zero Data Retention setting excludes).
                Disjoint from `models` … Populated only for allowlisted 1P hosts that render these
                rows (currently the VS Code extension — UNAVAILABLE_MODELS_HOST_ENTRYPOINTS); empty
                for every other consumer. Omitted when empty."),
  account: wgc(),
  current_model: C.string().optional()
     .describe("@internal The CLI's active model at connect time. Remote Control clients (web/mobile)
                sync their model dropdown TO this value on connect instead of sending set_model with
                their own default — without it, connecting from a phone silently switches the
                terminal's model (CC-2659)."),
  current_permission_mode: jSe().optional()
     .describe("@internal The CLI's active permission mode at connect time, for the same connect-time
                sync as current_model."),
  pid: C.number().optional().describe("@internal CLI process PID for tmux socket isolation"),
  fast_mode_state: ijt().optional(),
  feedback_survey_config: sYm().optional()
     .describe("@internal Present only when the feedback-survey surface is enabled for this host
                (GrowthBook gate, privacy level, and org policy all allow it). Absent means the host
                must not show the survey.")
}).describe("Response from session initialization with available commands, models, and account info.")
```
NOTE: in the schema, `current_model`/`current_permission_mode`/`pid`/`fast_mode_state`/`feedback_survey_config`/`unavailable_models` are all `.optional()`. The runtime `zyc()` always sets `pid` and (conditionally) the rest; `current_model`/`current_permission_mode` are injected ONLY by the bridge `getInitializeState()` spread (§5).

---

## 3. Response sub-schemas (byte evidence)

### 3.1 `commands` element — `sjt` (offset `210261974`)
```
C.object({
  name: C.string().describe("Skill name (without the leading slash)"),
  description: C.string().describe("Description of what the skill does"),
  argumentHint: C.string().describe('Hint for skill arguments (e.g., "<file>")'),
  aliases: C.array(C.string()).optional()
     .describe("Alternate names that resolve to this command (e.g., /cost and /stats both resolve to /usage)")
}).describe("Information about an available skill (invoked via /command syntax).")
```
(Runtime: `name:mu(d)`, `description:gne(d)`, `argumentHint:d.argumentHint||""`, `aliases` omitted when empty.)

### 3.2 `agents` element — `D$o` (offset `210262437`)
```
C.object({
  name: C.string().describe('Agent type identifier (e.g., "Explore")'),
  description: C.string().describe("Description of when to use this agent"),
  model: C.string().optional().describe("Model alias this agent uses. If omitted, inherits the parent's model")
}).describe("Information about an available subagent that can be invoked via the Task tool.")
```

### 3.3 `models` / `unavailable_models` element — `P$o` (offset `210262802`)
```
C.object({
  value: C.string().describe("Model identifier to use in API calls"),
  displayName: C.string().describe("Human-readable display name"),
  description: C.string().describe("Description of the model's capabilities"),
  supportsEffort: C.boolean().optional().describe("Whether this model supports effort levels"),
  supportedEffortLevels: C.array(C.enum(["low","medium","high","xhigh","max"])).optional()
     .describe("Available effort levels for this model"),
  supportsAdaptiveThinking: C.boolean().optional()
     .describe("Whether this model supports adaptive thinking (Claude decides when and how much to think)"),
  supportsFastMode: C.boolean().optional().describe("Whether this model supports fast mode"),
  supportsAutoMode: C.boolean().optional().describe("Whether this model supports auto mode"),
  disabled: C.boolean().optional()
     .describe("@internal Model is visible but not selectable (e.g. a model the org's Zero Data
                Retention setting excludes). The human-readable reason is folded into `description`;
                a structured disabledReason field is the extension point …")
}).describe("Information about an available model.")
```

### 3.4 `account` — `wgc` (offset `210263955`)
```
C.object({
  email: C.string().optional(),
  organization: C.string().optional(),
  subscriptionType: C.string().optional(),
  tokenSource: C.string().optional(),
  apiKeySource: C.string().optional(),
  apiProvider: C.enum(["firstParty","bedrock","vertex","foundry","anthropicAws","mantle","gateway"]).optional()
     .describe('Active API backend. Anthropic OAuth login only applies when "firstParty"; for 3P
                providers the other fields are absent and auth is external (AWS creds, gcloud ADC, etc.).
                "gateway" means the CLI is authenticated against an enterprise gateway.')
}).describe("Information about the logged in user's account.")
```
(Runtime maps `subscriptionType` ← `c?.subscription`, and `apiProvider` ← `Rr()`.)

### 3.5 `fast_mode_state` — `ijt` (offset `210317881`)
```
C.enum(["off","cooldown","on"]).describe("Fast mode state: off, in cooldown after rate limit, or actively enabled.")
```

### 3.6 permission-mode enum — `jSe` (offset `210244123`) — used by `current_permission_mode`
```
C.enum(["default","acceptEdits","bypassPermissions","plan","dontAsk","auto"])
  .describe("Permission mode … 'default' - Standard behavior, prompts for dangerous operations.
             'acceptEdits' - Auto-accept file edit operations. 'bypassPermissions' - Bypass all
             permission checks (requires allowDangerouslySkipPermissions). 'plan' - Planning mode,
             no actual tool execution. 'dontAsk' - Don't prompt for permissions, deny if not
             pre-approved. 'auto' - Use a model classifier to approve/deny permission prompts.")
```

---

## 4. `feedback_survey_config` — `sYm` (offset just before `k3E`, ~`210321xxx`)
```
C.object({
  minTimeBeforeFeedbackMs: C.number(),
  minTimeBetweenFeedbackMs: C.number(),
  minTimeBetweenGlobalFeedbackMs: C.number(),
  minUserTurnsBeforeFeedback: C.number(),
  minUserTurnsBetweenFeedback: C.number(),
  hideThanksAfterMs: C.number(),
  onForModels: C.array(C.string()),
  probability: C.number(),
  lastSurveyShownTime: C.number().nullable()
}).describe("@internal Session feedback-survey configuration for host UIs (VS Code webview, Claude
            Desktop) that run the survey trigger logic themselves: the same GrowthBook-driven
            pacing/probability values the terminal survey uses, plus the cross-surface last-shown
            time the host can't read. Survey responses are proxied back as
            tengu_feedback_survey_event log_event notifications.")
```
Built by `pyc()`; per-host gated (absent ⇒ host must not show survey).

---

## 5. `current_model` / `current_permission_mode` — injected via the REPL bridge spread
Two initialize handlers; the **bridge** one (Remote-Control web/mobile, `initReplBridge`) is the only place these two keys enter.

### 5.1 Bridge fallback handler (offset `206176765`)
```js
case "initialize": {
  try { let w=ZTe(e.request.supportedDialogKinds); if(w.length>0) a?.(w) }
  catch(w){ A(`[bridge:repl] dialog-kind capture failed; acking initialize anyway: ${Ce(w)}`) }
  E = { type:"control_response",
        response:{ subtype:"success", request_id:e.request_id,
          response:{ commands:[], agents:[], output_style:"normal",
                     available_output_styles:["normal"], models:[],
                     account:{}, pid:process.pid,
                     ...s?.() } } };          // ← s = getInitializeState
  break;
}
```
Outbound-only guard: if the bridge is outbound-only AND subtype≠"initialize", the request is rejected with `{subtype:"error", error:$ym}`. `initialize` is always allowed.

### 5.2 `s = getInitializeState()` (offset `207640946`, inside `initReplBridge` params)
```js
getInitializeState(){
  return { current_model: k.current,
           current_permission_mode: zP(D.getState().toolPermissionContext.mode) };
}
```
So on the bridge path the wire payload = the bridge's static skeleton overlaid by `{current_model, current_permission_mode}`. The terminal/SDK main path (`zyc`) does NOT add these two.

### 5.3 Main control handler that calls `zyc()` (offset ~`210508xxx`)
```js
if(e.hooks){ let h={};
  for(let[g,_] of Object.entries(e.hooks))
    h[g]=_.map((T)=>{ let y=T.hookCallbackIds.map((S)=>a.createHookCallback(S,T.timeout));
                      return {matcher:T.matcher, hooks:y}; });
  jde(h); }                              // re-hydrate hookCallbackIds → live callbacks
if(e.jsonSchema) Lar(e.jsonSchema);
r.enqueue({ type:"control_response",
            response:{ subtype:"success", request_id:t,
                       response: await zyc(o,u,s,i,d,c.userSpecifiedModel) } });
// telemetry
W("tengu_sdk_init_handshake",{ uptime_ms:…, mcp_client_count:f.clients.length,
   mcp_pending_count:…, mcpNonBlocking:Hbe(), session_mirror:!!c.sessionMirror });
```
This confirms the CLI re-inflates the request's `hookCallbackIds` into live callbacks (`a.createHookCallback`) and applies `jsonSchema` before building the response.

---

## 6. IMPORTANT distinction — keys that are NOT in this payload
The task's candidate key list included `mcp_servers`, `slash_commands`, `roots`, `permission_mode`. Byte evidence shows these are keys of the **`type:"system", subtype:"init"` system message** schema (offset `210281066`), a *separate* message, NOT the control-response initialize payload:
```
{ api_version:C.string(), cwd:C.string(), tools:C.array(C.string()),
  mcp_servers:C.array(C.object({name:C.string(), status:C.string()})),
  model:C.string(), permissionMode:jSe(), slash_commands:C.array(C.string()),
  output_style:C.string(), skills:C.array(C.string()),
  plugins:C.array(C.object({name:C.string(), path:C.string(), source:C.string().optional() …})), … }
```
- `mcp_servers` (here: `[{name,status}]`) and `slash_commands` (here: `string[]`) live in `system/init`, not the control initialize response.
- `roots:` (offsets `194097392`, `200065645`, …) appears in MCP/workspace contexts, not in the control initialize response.
- `permission_mode` as a key (vs the `current_permission_mode` field) appears in hook-input schema `IT` (`{session_id,transcript_path,cwd,permission_mode?,agent_id?,agent_type?}`) and `system/init` as `permissionMode`, not the initialize control-response.

---

## 7. Byte-offset index (for re-verification)
| symbol / anchor | offset | meaning |
|---|---|---|
| `subtype:C.literal("initialize")` | `210318607` | REQUEST Zod schema start |
| `Initializes the SDK session with hooks` | `210321664` | REQUEST schema `.describe()` |
| `subtype:"initialize"` (construction) | `206303422` | SDK `initialize()` builder |
| `czm=` (hook event enum) | `210244686` | request hooks event names |
| `oYm=` (hook matcher entry) | `210318xxx` | `{matcher?,hookCallbackIds,timeout?}` |
| `kgc=` (agent def, request) | `210264xxx` | request `agents` value type |
| `k3E` schema (`available_output_styles:`) | `210322558` | RESPONSE Zod schema |
| `zyc()` runtime builder (`available_output_styles:`) | `210509330` | RESPONSE payload construction |
| `sjt=` (commands element) | `210261974` | `{name,description,argumentHint,aliases?}` |
| `D$o=` (agents element) | `210262437` | `{name,description,model?}` |
| `P$o=` (model element) | `210262802` | model object |
| `wgc=` (account) | `210263955` | account object |
| `ijt=` (fast_mode_state) | `210317881` | `enum off/cooldown/on` |
| `jSe=` (permission mode) | `210244123` | enum default/acceptEdits/… |
| `sYm=` (feedback_survey_config) | `~210321xxx` | survey config object |
| bridge `case "initialize"` | `206176765` | empty-skeleton + `...s?.()` |
| `getInitializeState()` | `207640946` | `{current_model,current_permission_mode}` |
| `system/init` schema (`slash_commands:`) | `210281066` | SEPARATE message (not this payload) |

Build note: `tengu_sdk_init_handshake` telemetry fires immediately after the success control_response is enqueued, carrying `uptime_ms, mcp_client_count, mcp_pending_count, mcpNonBlocking, session_mirror`.
