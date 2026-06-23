# Workflow Runtime & Telemetry — Byte-Exact Oracle Facts

Binary: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`
Version: v2.1.186
Extracted: 2026-06-23

---

## 1. Tengu Workflow Event Names (Complete List)

Extracted via: `LC_ALL=C strings "$BIN" | grep -oE "tengu_workflow[a-z_]*" | sort -u`

```
tengu_workflow_agent_cap_exceeded
tengu_workflow_budget_cap_exceeded
tengu_workflow_completed
tengu_workflow_journal_started_hit_respawn
tengu_workflow_keyword
tengu_workflow_keyword_dismissed
tengu_workflow_keyword_restored
tengu_workflow_launched
tengu_workflow_phase_completed
tengu_workflow_saved
tengu_workflow_usage_warning_accepted
tengu_workflows_enabled
```

Also confirmed present (checked separately):
- `tengu_workflows_enabled` — confirmed (also in event list above; used as feature flag: `it("tengu_workflows_enabled", true)`)
- `tengu_review_workflow_routing` — confirmed present as a feature flag: `it("tengu_review_workflow_routing", false)`

Total: **12 event names** (10 `tengu_workflow_*` events + `tengu_workflows_enabled` + `tengu_review_workflow_routing`).

---

## 2. `parallel()` Validation Throws (Both Messages)

Two distinct `TypeError` throws. Source region at offset 202945078:

```js
if (!Array.isArray(K))
  throw TypeError("parallel() expects an array of functions");

// …then per-element check:
for (let ne of Y)
  if (typeof ne !== "function")
    throw TypeError("parallel() expects an array of functions, not promises. Wrap each call: () => agent(...)");
```

**Throw 1** — triggered when the argument is not an array at all:
```
parallel() expects an array of functions
```

**Throw 2** — triggered when an element of the array is not a function (e.g. a resolved promise was passed instead of a thunk):
```
parallel() expects an array of functions, not promises. Wrap each call: () => agent(...)
```

Byte offsets of message strings:
- `"parallel() expects an array of functions"` → 202945078
- `"parallel() expects an array of functions, not promises. Wrap each call: () => agent(...)"` → 202945230 (same region, second throw)

---

## 3. `workflow()` Nesting Throw (Full Verbatim Message)

Source region at offset 202924088. The `workflow` key in the child-context API object is replaced with a stub that always rejects:

```js
workflow: () =>
  Promise.reject(
    Error(
      "workflow() cannot be called from within a child workflow — nesting is limited to one level. Inline the inner script or call its agents directly."
    )
  )
```

Decoded (em-dash `—` → `—`):

```
workflow() cannot be called from within a child workflow — nesting is limited to one level. Inline the inner script or call its agents directly.
```

Byte offset of anchor `"nesting is limited to one level"`: **202924088**

---

## 4. Concurrency Cap Formula (Exact Expression)

### `CBp` function definition (offset 202928798):

```js
function CBp(e) {
  return Math.min(16, Math.max(2, e - 2));
}
```

### Call site at module init (offset 202948315):

```js
VKa = require("os");
RBp = CBp(VKa.cpus().length);
```

So: `RBp = Math.min(16, Math.max(2, os.cpus().length - 2))`

**Meaning:** parallel concurrency cap = `Math.min(16, Math.max(2, cpuCount - 2))`. Minimum 2, maximum 16, default = CPU count minus 2.

`RBp` is passed to `Nit(RBp, F)` which creates the semaphore wrapping the `agent()` implementation function `F`.

### Pipeline concurrency cap:

`vBp = 50` (offset 202947046) — used as `Nit(vBp, G)` for the pipeline semaphore, independently capped at 50 concurrent pipeline slots.

---

## 5. `agent()` Opts Key Normalization (`ABp` function)

Source region at offset 202927143:

```js
function ABp(e) {
  if (!e) return "{}";
  let t = {};
  let n = ["schema", "model", "effort", "isolation", "agentType"];
  for (let o of n) {
    let s = e[o];
    if (s === undefined || typeof s === "function") continue;
    t[o] = s;
  }
  let r = (o) => {
    if (typeof o === "function") return;
    if (Array.isArray(o)) {
      let s = [], i = o.length, a = Number.isSafeInteger(i) ? i : 0;
      for (let l = 0; l < a; l++) s[l] = r(o[l]);
      return s;
    }
    if (o && typeof o === "object") {
      let s = {};
      for (let i of Object.keys(o).sort()) {
        if (i === "__proto__") continue;
        s[i] = r(o[i]);
      }
      return s;
    }
    return o;
  };
  return JSON.stringify(r(t));
}
```

**Key set (exact, in extraction order):**
```
["schema", "model", "effort", "isolation", "agentType"]
```

**Normalization method:**
- Extract only those 5 keys (skip `undefined` and function values)
- Recursively serialize: arrays by index, objects with keys **sorted alphabetically** (skipping `__proto__`), functions stripped
- Serialize via `JSON.stringify`

This key string is used as the journal cache key for agent deduplication/resumption.

---

## 6. Lifetime Cap, Agent Cap, and Budget Messages

### Constants (offset 202947046–202947053):

```
vBp = 50        // pipeline parallel cap
zKa = 1000      // agent() lifetime call cap
WKa = 400       // resultPreview/promptPreview truncation length (n5e)
PBp = 180000    // agent stallMs default (180 seconds)
GKa = 5         // stall retry limit
OBp = 5         // StructuredOutput retry cap default (MAX_STRUCTURED_OUTPUT_RETRIES)
```

### Agent cap exceeded — `WorkflowAgentCapError` (offset 202948380):

```js
wBp = `Workflow agent() call cap reached (${zKa}). This usually means a loop using budget.remaining() never terminates because ` +
  "no token budget was set — remaining() returns Infinity when budget.total is null. " +
  "Add a hard iteration cap to the loop, or pass a token budget.";

class WorkflowAgentCapError extends Error {
  constructor() { super(wBp); this.name = "WorkflowAgentCapError"; }
}
```

Decoded verbatim message:
```
Workflow agent() call cap reached (1000). This usually means a loop using budget.remaining() never terminates because no token budget was set — remaining() returns Infinity when budget.total is null. Add a hard iteration cap to the loop, or pass a token budget.
```

### Budget cap exceeded — `WorkflowBudgetExceededError` (offset 202948769):

```js
class WorkflowBudgetExceededError extends Error {
  constructor(e, t) {
    super(`Workflow token budget exceeded (${e.toLocaleString()} / ${t.toLocaleString()} output tokens). Stopping further agent() calls. In-flight agents will complete; their results are preserved.`);
    this.name = "WorkflowBudgetExceededError";
  }
}
```

Verbatim template:
```
Workflow token budget exceeded ({spent} / {budget} output tokens). Stopping further agent() calls. In-flight agents will complete; their results are preserved.
```

### parallel/pipeline "slots dropped — token budget exceeded" messages (offset 202945746, 202946701):

```js
// parallel:
`parallel: ${ee} ${An(ee, "slot")} dropped — token budget exceeded`

// pipeline:
`pipeline: ${ne} ${An(ne, "slot")} dropped — token budget exceeded`
```

Decoded pattern (em-dash):
```
parallel: N slot(s) dropped — token budget exceeded
pipeline: N slot(s) dropped — token budget exceeded
```

### StructuredOutput retry cap (offset 202936048):

```js
throw new la(
  `agent({schema}): StructuredOutput retry cap (${Yr}) exceeded — ` +
  `${kn} failed ${An(kn, "call")} with no valid output`,
  "Workflow agent({schema}) StructuredOutput retry cap exceeded"
);
```

Where `Yr = Fe.MAX_STRUCTURED_OUTPUT_RETRIES ?? OBp` (i.e., `MAX_STRUCTURED_OUTPUT_RETRIES` or 5).

Decoded:
```
agent({schema}): StructuredOutput retry cap (5) exceeded — N failed call(s) with no valid output
```

### Stall retry message template (offset 202941236):

```js
`[${Q}] throttled response (no stop_reason, ${Be.outputTokens ?? "?"} output tokens in ${Math.round(Be.durationMs / 1000)}s) — sleeping 45s before retry`

`[stall] agent "${Q}" ${zt} after ${Math.round(Be.durationMs / 1000)}s${Ot} — retrying (${ct}/${GKa})`
```

---

## 7. Telemetry Events — Payload Fields per Event

### `tengu_workflow_launched` (offset 203007344)

Emit condition: every time a workflow is invoked (inline script, named, or `scriptPath`).

Payload fields:
```js
{
  invocation_mode: Le(e.scriptPath ? "scriptPath" : e.name ? "named" : "inline"),
  workflow_source:  Le(_),          // "scriptPath" | name | "inline"
  workflow_name:    T,              // iUp(meta.name, sourceName)
  workflow_description: y,          // lUp(meta.description, sourceName)
  phase_count:      c.meta.phases?.length ?? 0,
  launched_from_subagent: t.agentId != null,
  has_args:         e.args != null,
  is_resume:        e.resumeFromRunId != null,
  script_size_chars: i.length,
}
```

### `tengu_workflow_completed` (offset 202958982)

Emit condition: after the workflow run finishes (success, failure, or abort).

Payload fields:
```js
{
  workflow_run_id:       n,
  workflow_source:       Le(p.source),
  workflow_name:         p.name,
  workflow_description:  p.description,
  status:                Le(P),   // "completed" | "failed" | "killed"
  agent_count:           k.agentCount,
  total_tokens:          O,
  total_tool_calls:      L,
  duration_ms:           k.durationMs,
}
```

Status derivation:
```js
const P = g.abortController?.signal.aborted ? "killed"
         : k.error ? "failed"
         : "completed";
```

### `tengu_workflow_phase_completed` (offset 202959726)

Emit condition: for each phase of a "built-in" workflow source only, after completion.

Payload fields (one event per phase):
```js
{
  workflow_run_id:          n,
  workflow_source:          Le(p.source),
  workflow_name:            p.name,
  phase_index:              U,           // numeric index
  phase_title:              F.title,
  phase_tokens:             F.tokens,
  phase_tool_calls:         F.toolCalls,
  phase_agent_duration_ms:  F.durationMs,
  phase_agent_count:        F.agentCount,
  phase_error_count:        F.errorCount,
  phase_skip_count:         F.skipCount,
}
```

### `tengu_workflow_saved` (offset 205547167)

Emit condition: when `/workflow save` persists a workflow script.

Payload fields:
```js
{
  scope:              Le(e.scope),
  overwrite:          e.overwrite,
  script_size_chars:  e.script.length,
}
```

### `tengu_workflow_agent_cap_exceeded` (offset 202929433)

Emit condition: when `c >= zKa` (1000 agent() calls reached), fires once (`if (!h) h = true`).

Payload fields:
```js
{ agentCount: c }
```

### `tengu_workflow_budget_cap_exceeded` (offset 202929630)

Emit condition: when `i.getTurnSpent() >= i.total` (output token budget exhausted), fires once (`if (!g) g = true`).

Payload fields:
```js
{ spent: K, budget: i.total, agentCount: c }
```

### `tengu_workflow_journal_started_hit_respawn` (offset 202931208)

Emit condition: when a journal entry for this agent key already has prior `started` records (i.e., the agent ran before and is being retried from a resumed run).

Payload fields:
```js
{ attempts: he.length }
```

### `tengu_workflow_keyword` (offset 204110859)

Emit condition: when the ultracode keyword trigger is matched in user input (`ell(e)` check), emitted from `X7p()`.

Payload fields:
```js
{}   // empty payload
```

### `tengu_workflow_keyword_dismissed` (offset 207563516)

Emit condition: when user toggles keyword off (UI checkbox). Payload: `{}` (empty).

### `tengu_workflow_keyword_restored` (offset 207563714)

Emit condition: when user re-enables keyword (toggle back on). Payload: `{}` (empty).

### `tengu_workflow_usage_warning_accepted` (offset 203014181)

Emit condition: when user accepts the usage warning dialog and it is persisted successfully.

Payload fields: `{}` (empty).

### `tengu_workflows_enabled` (feature flag, not an emitted event)

Used as: `it("tengu_workflows_enabled", true)` — returns feature flag value. Also: `Ie("task_local_workflow_resume")` is called when resuming (separate infra event).

---

## 8. Progress Event Shapes

All progress events are emitted as:
```js
n({ type: "progress", toolUseID: <string>, data: { type: <kind>, ...fields } })
```

### `workflow_agent` progress event

**toolUseID format**: `workflow_agent_${index}_${suffix}`
- Suffix variants:
  - `workflow_agent_${ne}_queued` — for initial "start" and "error" states (queued agent)
  - `workflow_agent_${K}_${ct}` — for in-flight agent (where `ct` is the agentId UUID)
  - `workflow_agent_${ne}_cached` — for journal cache hit

**State values** (the `state` field):
- `"start"` — agent queued and running (initial emit)
- `"progress"` — intermediate progress update during agent execution
- `"done"` — agent completed successfully
- `"error"` — agent failed (stalled, aborted, or threw)
- `"cached"` — agent result served from journal cache (not re-run)

**Fields present on ALL `workflow_agent` events:**
```js
{
  type:           "workflow_agent",
  index:          <number>,         // monotonically incrementing agent counter
  label:          <string>,         // agent label (prompt truncated to 60 chars, or opts.label)
  phaseIndex:     <number|undefined>, // phase index if agent has a phase
  phaseTitle:     <string|undefined>, // phase title string
  model:          <string>,         // opts.model ?? mainLoopModel
  state:          <string>,         // see states above
  lastProgressAt: <Date.now()>,
}
```

**Additional fields by state:**

`"start"` state (queued emit):
```js
{
  agentType:   <string|undefined>,  // String(te.agentType) if set
  isolation:   "worktree"|undefined, // if te.isolation === "worktree" or "remote"
  queuedAt:    <Date.now()>,
  promptPreview: <string|undefined>, // n5e(prompt), max 400 chars
}
```

`"start"` state (in-flight, from cn()):
```js
{
  agentId:       <uuid>,
  agentType:     <string|undefined>,
  isolation:     "worktree"|undefined,
  fallbackModel: <string|undefined>,
  startedAt:     <Date.now()>,
  queuedAt:      <number>,
  attempt:       <number>,
  lastAttemptReason: <string|undefined>,
  promptPreview: <string|undefined>,
  tokens:        <number|undefined>,   // from cached resume
  toolCalls:     <number|undefined>,   // from cached resume
}
```

`"progress"` state:
```js
{
  tokens:        <number>,   // cumulative tokens spent (inherited + current turn)
  toolCalls:     <number>,   // cumulative tool calls
  lastToolName:  <string|undefined>,
  lastToolSummary: <string|undefined>,
}
```

`"done"` state:
```js
{
  tokens:        <number>,
  toolCalls:     <number>,
  durationMs:    <number>,
  resultPreview: <string|undefined>,  // n5e(result), max 400 chars
}
```

`"error"` state:
```js
{
  error:     <string>,   // error message; special cases: "skipped by user", "stalled — no progress for Nms"
  tokens:    <number>,
  toolCalls: <number>,
  durationMs: <number>,
  skipped:   <boolean|undefined>,   // true when "skipped by user"
}
```

`"cached"` state:
```js
{
  agentId:       <uuid>,
  cached:        true,
  startedAt:     <Date.now()>,
  resultPreview: <string|undefined>,
  promptPreview: <string|undefined>,
}
```

### `workflow_phase` progress event

**toolUseID format**: `` `workflow_phase_${Q}` `` where `Q` is the phase's numeric index (1-based, auto-incremented).

**Fields:**
```js
{
  type:    "workflow_phase",
  index:   <number>,    // 1-based sequential phase index
  title:   <string>,    // phase title string
  kind:    <undefined>, // ALWAYS undefined — D(K) called without kind arg
}
```

Note: `kind` field is present in the type but is never populated in practice. The `D(K, Y)` function receives `Y` only from pre-declared phases loop (`for (let K of s??[]) D(K)`) and from `O = phase()` API (`D(H)`) — neither passes a kind argument. The field defaults to `undefined` in the emitted event.

### `workflow_log` progress event

**toolUseID**: always the string literal `"workflow_log"` (not indexed).

**Fields:**
```js
{
  type:    "workflow_log",
  message: <string>,
}
```

Log trimming: when total progress items exceed `IKa * 2` (IKa = 500, so threshold = 1000), the oldest `workflow_log` entries are trimmed from the front until `IKa` (500) log items remain. Non-log items are never trimmed.

---

## 9. Subagent System Prompts

### `kBp` — plain subagent (no schema) (offset 202947083):

```
You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.

CRITICAL: Your final text response is returned **verbatim** as a string to the calling script — it is your return value, not a message to a human.
- Output the literal result (data, JSON, text). Do NOT output confirmations like "Done." or "Sent."
- If asked for JSON, return ONLY the raw JSON — no code fences, no prose, no markdown.
- Do NOT use SendUserMessage to deliver your answer. Put your answer in your final text response.
- Be concise. The script will parse your output.
```

### `IBp` — NOTE suffix appended to existing system prompt when agent has schema (offset 202948025 region):

```
---

NOTE: You are running inside a workflow script. You MUST return your final answer by calling the {StructuredOutput} tool exactly once — the tool's input schema defines the required shape. Do your work, then call {StructuredOutput}; do NOT put your answer in a text response (the script reads ONLY the tool call). If validation fails, read the error and call {StructuredOutput} again with a corrected shape.
```

### `xBp` — standalone schema subagent prompt (offset 202948410 region):

```
You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.

CRITICAL: You MUST call the {StructuredOutput} tool exactly once to return your final answer. The tool's input schema defines the required shape.
- Do your work (Read files, run commands, etc.), then call {StructuredOutput} with your answer.
- Do NOT put your answer in a text response. The script reads ONLY the {StructuredOutput} tool call.
- If the schema validation fails, read the error and call {StructuredOutput} again with a corrected shape.
```

### `HBp` — inline NOTE appended for workflow-context sessions (offset near 202947083):

```
---

NOTE: You are running inside a workflow script. Your final text response is returned verbatim as a string to the calling script — it is your return value, not a message to a human. Output the literal result; do not output confirmations like "Done." Be concise — the script will parse your output.
```

### `Oho` — workflow subagent agent definition (offset 202949968):

```js
{
  agentType: "workflow-subagent",
  whenToUse: "Internal subagent for workflow script orchestration.",
  tools: ["*"],
  disallowedTools: [i1, ns, SI],  // includes SendUserMessage (SI)
  source: "built-in",
  baseDir: "built-in",
  getSystemPrompt: () => kBp,
}
```

`DBp` = same as `Oho` but with `getSystemPrompt: () => xBp` (schema variant).

---

## 10. Summary of Confirmed vs. Uncertain Items

| Item | Status |
|------|--------|
| All 12 tengu_workflow* event names | **CONFIRMED** |
| `tengu_workflows_enabled` feature flag | **CONFIRMED** |
| `tengu_review_workflow_routing` feature flag | **CONFIRMED** |
| `parallel()` throw 1 (not array) | **CONFIRMED** |
| `parallel()` throw 2 (not function/promise) | **CONFIRMED** |
| `workflow()` nesting throw | **CONFIRMED** |
| Concurrency formula `Math.min(16, Math.max(2, cpus-2))` | **CONFIRMED** |
| Pipeline cap = 50 | **CONFIRMED** |
| `ABp` key set `["schema","model","effort","isolation","agentType"]` | **CONFIRMED** |
| `ABp` serialization = sorted keys + `JSON.stringify` | **CONFIRMED** |
| Agent lifetime cap = 1000 (`zKa`) | **CONFIRMED** |
| Stall retry limit = 5 (`GKa`) | **CONFIRMED** |
| StructuredOutput retry cap = 5 (`OBp`) | **CONFIRMED** |
| Stall timeout default = 180000ms (`PBp`) | **CONFIRMED** |
| Progress log trim threshold = 500 (`IKa`), keep 500 | **CONFIRMED** |
| Budget exceeded error message template | **CONFIRMED** |
| All telemetry event payloads | **CONFIRMED** |
| `workflow_phase` kind field always undefined | **CONFIRMED** |
| `workflow_agent` state enum values | **CONFIRMED** |
| 4096 array length cap message | **0-HIT** — no such message found in binary; likely not a real limit at this layer |
