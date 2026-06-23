# Workflow Tool — Result Format & Gating: Binary Oracle Facts

Binary: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude` (v2.1.186)

---

## 1. Tool-Result Text the Model Sees (`mapToolResultToToolResultBlockParam`)

**Binary offset**: 203008520 (`Summary: ` anchor), 203008835 (`Transcript dir` anchor)

Verbatim JS from binary (extracted at offset ~203007200):

```js
mapToolResultToToolResultBlockParam(e, t) {
  // Case 1: compile/syntax error (no launch)
  if (e.error)
    return {
      tool_use_id: t,
      type: "tool_result",
      content: `Workflow script has a syntax error and was not launched:\n${e.error}`,
      is_error: true
    };

  // Case 2: remote CCR launch
  if (e.status === "remote_launched")
    return {
      tool_use_id: t,
      type: "tool_result",
      content:
        `Workflow launched in a remote CCR session. Task ID: ${e.taskId}\n` +
        `Session: ${e.sessionUrl}\n` +
        (e.summary ? `Summary: ${e.summary}\n` : "") +
        (e.warning ? `Warning: ${e.warning}\n` : "") +
        `\nThe workflow runs against a fresh clone of the pushed branch; phase progress is visible at the session URL, not in /workflows. You will be notified when it completes.`,
      is_error: false
    };

  // Case 3: async_launched (local background)
  let n = e.summary   ? `\nSummary: ${e.summary}`    : "";
  let r = e.transcriptDir ? `\nTranscript dir: ${e.transcriptDir}` : "";
  let o = e.scriptPath
    ? `\nScript file: ${e.scriptPath}\n(Edit this file with Write/Edit and re-invoke Workflow with {scriptPath: "${e.scriptPath}"} to iterate without resending the script.)`
    : "";
  let s = e.scriptPath && e.runId
    ? `\nRun ID: ${e.runId}\nTo resume after editing the script: Workflow({scriptPath: "${e.scriptPath}", resumeFromRunId: "${e.runId}"}) — completed agents return cached results.`
    : "";
  let i =
    `Workflow launched in background. Task ID: ${e.taskId}${n}${r}${o}${s}` +
    `\n\nYou will be notified when it completes. Use /workflows to watch live progress.`;
  return { tool_use_id: t, type: "tool_result", content: i, is_error: false };
}
```

### Template lines verbatim (async_launched case)

| Field | Line template | Conditional? |
|---|---|---|
| `taskId` | `Workflow launched in background. Task ID: ${e.taskId}` | Always present |
| `summary` | `\nSummary: ${e.summary}` | Only if `e.summary` truthy |
| `transcriptDir` | `\nTranscript dir: ${e.transcriptDir}` | Only if `e.transcriptDir` truthy |
| `scriptPath` | `\nScript file: ${e.scriptPath}\n(Edit this file with Write/Edit and re-invoke Workflow with {scriptPath: "${e.scriptPath}"} to iterate without resending the script.)` | Only if `e.scriptPath` truthy |
| `runId` | `\nRun ID: ${e.runId}\nTo resume after editing the script: Workflow({scriptPath: "${e.scriptPath}", resumeFromRunId: "${e.runId}"}) — completed agents return cached results.` | Only if BOTH `e.scriptPath && e.runId` truthy |
| footer | `\n\nYou will be notified when it completes. Use /workflows to watch live progress.` | Always |

**Error case template**: `Workflow script has a syntax error and was not launched:\n${e.error}` — `is_error: true`

**Remote case template** (verbatim):
```
Workflow launched in a remote CCR session. Task ID: ${e.taskId}
Session: ${e.sessionUrl}
[Summary: ${e.summary}  ← conditional]
[Warning: ${e.warning}  ← conditional]

The workflow runs against a fresh clone of the pushed branch; phase progress is visible at the session URL, not in /workflows. You will be notified when it completes.
```

---

## 2. Result Object Shape (call() return value)

**Binary offset**: 203006800 (the `call()` return statement)

Verbatim JS from binary:

```js
// Happy path (compile succeeded):
return {
  data: {
    status: "async_launched",
    taskId: d,          // d = k$("local_workflow") — new task ID
    taskType: "local_workflow",
    workflowName: m,    // m = c.meta.name (from parsed script)
    runId: u,           // u = e.resumeFromRunId ?? `wf_${crypto.randomUUID().slice(0,12)}`
    summary: p,         // p = c.meta.description
    transcriptDir: h,   // h = Nte(u)
    scriptPath: g       // g = l ?? Kbi(m, u, i)  (l = resolvedScriptPath from input)
  }
};

// Compile failed (before launch):
return {
  data: {
    status: "async_launched",
    taskId: d,
    taskType: "local_workflow",
    workflowName: m,
    runId: u,
    summary: p,
    error: f.error      // compile error text
    // transcriptDir and scriptPath are ABSENT on compile failure
  }
};
```

### outputSchema (sUp()) — Zod definition (verbatim from binary, offset ~203002000)

```js
sUp = lazy(() => z.object({
  status:        z.enum(["async_launched", "remote_launched"]),
  taskId:        z.string(),
  taskType:      z.enum(["local_workflow", "remote_agent"]).optional()
                   .describe("TaskType of the registered background task — 'local_workflow' for in-process runs, 'remote_agent' when remote:true dispatches to CCR. Set on all new writes; absent only on transcripts written before this field existed."),
  workflowName:  z.string().optional()
                   .describe("meta.name from the workflow script — same value as task_started.workflow_name. Set on all new writes; absent only on transcripts written before this field existed."),
  runId:         z.string().optional()
                   .describe("Local workflow run identifier for resumeFromRunId. Absent for remote_launched (the CCR session URL is the resume handle there) and on transcripts written before this field existed."),
  summary:       z.string().optional(),
  transcriptDir: z.string().optional()
                   .describe("Directory where subagent transcripts are written during execution"),
  scriptPath:    z.string().optional()
                   .describe("Path to the persisted workflow script for this invocation. Editable via Write/Edit; pass back as `scriptPath` to re-run without resending the script."),
  sessionUrl:    z.string().optional()
                   .describe("CCR session URL when status is remote_launched"),
  warning:       z.string().optional()
                   .describe("Non-blocking heads-up (e.g. local git state diverges from the pushed branch the cloud session will clone)"),
  error:         z.string().optional()
                   .describe("Set if syntax check failed")
}))
```

**Key derivations**:
- `summary = c.meta.description` (from parsed workflow script's meta block)
- `runId = e.resumeFromRunId ?? "wf_" + crypto.randomUUID().slice(0, 12)`
- `scriptPath = resolvedScriptPath ?? Kbi(workflowName, runId, script)` (Kbi builds a saved script path)

---

## 3. `transcriptDir` Path Derivation (`Nte` function)

**Binary offset**: 202925481

Verbatim JS:

```js
function Nte(e) {  // e = runId
  let t = CU() ?? _g(gr());
  return path.join(t, xt(), "subagents", "workflows", e)
}
```

Where:
- `CU()` = `sessionProjectDir` from state (the `.claude/projects/<hash>` directory for this session)
  - `function CU() { let e = qH(); return e ? e.sessionProjectDir : Ft.sessionProjectDir }`
- `_g(gr())` = fallback: `getProjectDir(originalCwd)` — builds project dir from the original cwd
  - `gr()` returns `originalCwd`
- `xt()` = current `sessionId` (UUID)
  - `function xt() { return qH()?.sessionId ?? Ft.sessionId }`

**Resulting path format**:
```
<sessionProjectDir>/<sessionId>/subagents/workflows/<runId>
```

Example: `~/.claude/projects/-Users-foo-myproject/<sessionId>/subagents/workflows/wf_abc123def456`

This matches the TS source comment at `src/utils/sessionStorage.ts`:
```
// transcripts (e.g. workflow runs write to subagents/workflows/<runId>/).
```

---

## 4. `maxResultSizeChars` Confirmation

**Binary offset**: 203004100 (in the Ks({…}) tool definition object)

Verbatim JS:

```js
cUp = Ks({
  name: SI,                      // SI = "Workflow" (the tool name constant)
  aliases: ["RunWorkflow"],
  searchHint: "orchestrate subagents with deterministic JavaScript workflow",
  maxResultSizeChars: 1e5,       // ← 100,000 characters
  isEnabled: () => pA(),
  // ...
})
```

**Confirmed**: `maxResultSizeChars: 1e5` (100000) for the Workflow tool.

---

## 5. `isEnabled` Gate — `pA()` Full Logic

**Binary offset**: 196461282 (containing `pA`, `fbn`, `TSi`, `Did`, `xid` function definitions)

### Verbatim JS (complete, from binary at offset ~196461282):

```js
// Managed setting / env disable check
function fbn() {
  return ot(process.env.CLAUDE_CODE_DISABLE_WORKFLOWS) || $H()?.settings.disableWorkflows === true;
}

// Main isEnabled gate (what isEnabled: () => pA() calls)
function pA() {
  if (fbn()) return false;           // Step 1: hard-disabled check
  if (!TSi()) return false;          // Step 2: org/launch gate
  let { available: e, defaultOn: t } = s$r();
  if (!e) return false;              // Step 3: feature availability
  return xid() ?? t;                 // Step 4: user setting (with defaultOn fallback)
}

// Org gate: checks "allow_workflows" in org policy
function TSi() {
  return Xs("allow_workflows");
}

// User setting: settings.enableWorkflows (undefined = use defaultOn)
function xid() {
  return $H()?.settings.enableWorkflows;
}

// Feature availability + defaultOn (cached)
function s$r() {
  if (hbn !== undefined) return hbn;
  return hbn = Did(), hbn;
}

function Did() {
  // Env force-enable: CLAUDE_CODE_WORKFLOWS=1/true/yes
  if (ot(process.env.CLAUDE_CODE_WORKFLOWS)) {
    let t = it("tengu_workflows_enabled", true);
    return { available: t, defaultOn: t };
  }
  // Env force-disable: CLAUDE_CODE_WORKFLOWS=0/false/no
  if (el(process.env.CLAUDE_CODE_WORKFLOWS))
    return { available: false, defaultOn: false };
  // Feature flag gate
  if (!it("tengu_workflows_enabled", true))
    return { available: false, defaultOn: false };
  // Default: available=true, defaultOn depends on plan
  return { available: true, defaultOn: Oi() !== "pro" };
}
```

### `Xs("allow_workflows")` — org policy check (verbatim, offset 196441163):

```js
function Xs(e) {
  let t = cSi();  // gets org policy object
  if (!t) {
    // No org policy: check hardcoded deny list
    if (hid.has(e)) {
      if (v2()) return false;      // API key mode: deny
      if (gid.has(e) && Ki() && !(e === "allow_product_feedback" && bfe())) return false;
    }
    return true;  // no policy → ALLOW
  }
  let n = t[e];
  if (n) return n.allowed;
  // Check compliance taints
  let r = J7()?.compliance_taints ?? [];
  for (let [o, s] of fid)
    if (s === e && r.includes(o)) return false;
  return true;
}
```

### `it("tengu_workflows_enabled", true)` — GrowthBook feature flag (verbatim, offset 196421580):

```js
function it(e, t) {
  let n = Mxt();  // local override map
  if (n && e in n) return n[e];
  let r = Nxt();  // cached settings override
  if (r && e in r) return r[e];
  if (!U3()) return t;  // not initialized → return default
  if (MRe.has(e)) XSn(e);
  else Oxt.add(e);
  if (C8.has(e)) return C8.get(e);
  try {
    let o = kt().cachedGrowthBookFeatures?.[e];
    return o !== undefined ? o : t;
  } catch { return t; }
}
```

### `Did()` plan check: `Oi() !== "pro"`

```js
function Oi() {
  if (SHr()) return THr();    // override in tests
  if (!nT()) return null;     // not logged in
  let e = qs();               // session/account info
  if (!e) return null;
  return e.subscriptionType ?? null;
}
```

Returns `"pro"`, `"max"`, `"team"`, or `null`.

### Gate Summary Table

| Gate | Key/Env | Source | If fails |
|---|---|---|---|
| 1. Managed disable | `CLAUDE_CODE_DISABLE_WORKFLOWS` env (truthy) | `fbn()` | `false` — tool hidden |
| 1. Managed disable | `settings.disableWorkflows === true` | `fbn()` / `$H()` | `false` — tool hidden |
| 2. Org/launch gate | `"allow_workflows"` org policy key | `Xs("allow_workflows")` | `false` — tool hidden |
| 3. Feature flag | `"tengu_workflows_enabled"` GrowthBook flag (default: `true`) | `it("tengu_workflows_enabled", true)` | `available=false` → hidden |
| 3b. Env force-on | `CLAUDE_CODE_WORKFLOWS` truthy | `ot(process.env.CLAUDE_CODE_WORKFLOWS)` | available+defaultOn = flag result |
| 3c. Env force-off | `CLAUDE_CODE_WORKFLOWS` falsy string | `el(process.env.CLAUDE_CODE_WORKFLOWS)` | `{available: false, defaultOn: false}` |
| 4. User setting | `settings.enableWorkflows` | `$H()?.settings.enableWorkflows` | Falls back to `defaultOn` |
| 4b. defaultOn | plan != `"pro"` | `Oi() !== "pro"` | Pro plan: `defaultOn=false`; others: `true` |

### `validateInput` error codes

| errorCode | Condition | Message |
|---|---|---|
| 5 | `fbn()` = true (managed disable) | `"Dynamic workflows are disabled by managed settings (\`disableWorkflows\`)."` |
| 6 | `!pA()` (any gate fails) | `'Dynamic workflows are not enabled for this session (org policy, launch gate, or the "Dynamic workflows" setting in /config).'` |
| 1 | Script resolution failed | `e.error` (the resolve error) |
| 2 | Invalid workflow script (parse error) | `"Invalid workflow script: ${r.error}"` |
| 4 | Determinism violation (Date.now/Math.random/new Date) | `"Workflow scripts must be deterministic: Date.now()/Math.random()/new Date() are unavailable (breaks resume). Stamp results after the workflow returns, or pass timestamps via args."` |
| 3 | Resume of still-running workflow | `"Workflow ${runId} is still running (task ${taskId}). Stop it first with ${TaskStopToolName}({taskId: \"${taskId}\"}) before resuming."` |
| 7 | Aborted (signal fired) | `"Tool dispatch was retracted by a server fallback; the input may be truncated."` (errorCode not user-visible) |

### Default result for a normal user

**`pA()` returns `true` by default** for non-pro users when:
- No `CLAUDE_CODE_DISABLE_WORKFLOWS` set
- No `disableWorkflows` managed setting
- `Xs("allow_workflows")` returns `true` (no org policy, or policy allows)
- `tengu_workflows_enabled` GrowthBook flag is `true` (default)
- `settings.enableWorkflows` is `undefined` → falls back to `defaultOn = (plan !== "pro")`

**Pro plan**: `defaultOn = false` → Workflow tool is **hidden by default** for Pro users unless they explicitly enable it in `/config` (`settings.enableWorkflows = true`) or org policy overrides.

**Max/Team/null plan**: `defaultOn = true` → Workflow tool is **enabled by default**.

---

## 6. Tool Config Object Keys (around `maxResultSizeChars`)

Verbatim from binary offset ~203004050:

```js
cUp = Ks({
  name:                SI,             // "Workflow"
  aliases:             ["RunWorkflow"],
  searchHint:          "orchestrate subagents with deterministic JavaScript workflow",
  maxResultSizeChars:  1e5,            // 100_000
  isEnabled:           () => pA(),
  async prompt()       { return $ho },
  async description()  { return $ho },
  get inputSchema()    { return oUp() },
  get outputSchema()   { return sUp() },
  toAutoClassifierInput(e) { return e.script ?? e.name ?? "" },
  async validateInput(e, t) { … },
  async checkPermissions(e, t) { … },
  userFacingName()     { return "Workflow" },
  getToolUseSummary(e) { … },
  async call(e, t, n, r, o) { … },
  renderToolUseMessage:          w7a,
  renderToolUseProgressMessage:  k7a,
  renderToolResultMessage:       H7a,
  renderToolUseRejectedMessage:  I7a,
  mapToolResultToToolResultBlockParam(e, t) { … }
})
```
