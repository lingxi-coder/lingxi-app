# Workflow Agentdef & ValidateInput — Binary Oracle Facts (v2.1.186)

Binary: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`

---

## 1. kBp — `workflow-subagent` System Prompt (default / no schema)

Byte offset: **202947082** (`kBp=\`` starts here; prompt text at 202947087)

This is the system prompt used when NO `schema` is provided to `agent()`. It is returned by `Oho.getSystemPrompt`.

```text
You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.

CRITICAL: Your final text response is returned **verbatim** as a string to the calling script — it is your return value, not a message to a human.
- Output the literal result (data, JSON, text). Do NOT output confirmations like "Done." or "Sent."
- If asked for JSON, return ONLY the raw JSON — no code fences, no prose, no markdown.
- Do NOT use SendUserMessage to deliver your answer. Put your answer in your final text response.
- Be concise. The script will parse your output.
```

Note: `—` in the binary decodes to `—` (em dash).

---

## 2. xBp — Schema-Variant System Prompt (with `schema`)

Byte offset: **202949377** (`xBp=\`` starts here)

This is the system prompt used when a `schema` IS provided. It is returned by `DBp.getSystemPrompt`. The `${Lp}` interpolation is the structured-output tool name (a dynamic binding — the actual tool name depends on runtime context).

```text
You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.

CRITICAL: You MUST call the ${Lp} tool exactly once to return your final answer. The tool's input schema defines the required shape.
- Do your work (Read files, run commands, etc.), then call ${Lp} with your answer.
- Do NOT put your answer in a text response. The script reads ONLY the ${Lp} tool call.
- If the schema validation fails, read the error and call ${Lp} again with a corrected shape.
- After calling ${Lp} successfully, end your turn. No acknowledgment needed.
```

---

## 3. HBp — Non-Schema Subagent NOTE Addendum

Byte offset: **202947689** (`HBp=\`` starts here)

This is the trailing addendum appended to a subagent's system prompt for non-schema runs (appended to the user-specified or default system prompt).

```text

---

NOTE: You are running inside a workflow script. Your final text response is returned verbatim as a string to the calling script — it is your return value, not a message to a human. Output the literal result; do not output confirmations like "Done." Be concise — the script will parse your output.
```

Note: `—` decodes to `—`.

---

## 4. IBp — Schema Subagent NOTE Addendum

Byte offset: **202948991** (`IBp=\`` starts here)

This is the trailing addendum appended when a schema IS provided (appended to the user-specified system prompt, then `xBp` becomes the base prompt for the subagent).

```text

---

NOTE: You are running inside a workflow script. You MUST return your final answer by calling the ${Lp} tool exactly once — the tool's input schema defines the required shape. Do your work, then call ${Lp}; do NOT put your answer in a text response (the script reads ONLY the tool call). If validation fails, read the error and call ${Lp} again with a corrected shape.
```

---

## 5. DBp — Structured-Output Agentdef Object

Byte offset: **202950169** (`DBp=` starts here)

`DBp` is the agentdef used when `schema` is provided in a workflow `agent()` call. It is a spread of `Oho` with `getSystemPrompt` replaced:

```javascript
DBp = { ...Oho, getSystemPrompt: () => xBp }
```

So all fields are identical to `Oho` except `getSystemPrompt` returns `xBp` (the schema-variant prompt above).

**LingXi target:** `lingxi-code/tasks/src/handlers/local_workflow.rs:129 DEFAULT_WORKFLOW_SUBAGENT` — `DBp` is the schema-enabled variant.

---

## 6. Oho — `workflow-subagent` Agentdef Object Fields

Byte offset: **202949968** (`Oho=` starts here)

Exact verbatim field values from binary:

```javascript
Oho = {
  agentType: "workflow-subagent",
  whenToUse: "Internal subagent for workflow script orchestration.",
  tools: ["*"],
  disallowedTools: [i1, ns, SI],
  source: "built-in",
  baseDir: "built-in",
  getSystemPrompt: () => kBp
}
```

### Resolved `disallowedTools` values

The three variables in `disallowedTools` resolve to (confirmed from binary):

| Variable | Resolved value | Offset |
|----------|---------------|--------|
| `i1` | `"SendUserMessage"` | 198046976 |
| `ns` | `"Agent"` | 196484310 |
| `SI` | `"Workflow"` | 198059228 |

So `disallowedTools = ["SendUserMessage", "Agent", "Workflow"]`.

**LingXi target:** `lingxi-code/agent/src/builtins.rs` — the `workflow-subagent` builtin entry.

---

## 7. Companion Subagent Note (yyo function)

Byte offset: **203453254** (anchor `not available inside subagents`)

The function `yyo(e, t, n, r)` appends a note to the system prompt when a disallowed tool is called from within a subagent. The relevant branch:

```javascript
if (n && o && nke.has(o.name))
  return `. ${e} is not available inside subagents. Complete the task with the tools provided and return findings to the orchestrator.`;
```

Where:
- `e` = the tool name string (e.g., `"SendUserMessage"`)
- `n` = boolean flag indicating we're inside a subagent
- `o` = resolved tool object (`rl(Xq(), e)`)
- `nke` = set of tools whose names are checked (external tool set)

**Full template (verbatim):**

```text
. ${toolName} is not available inside subagents. Complete the task with the tools provided and return findings to the orchestrator.
```

Note: the string starts with `. ` (period + space) — it is appended directly to a sentence already in progress.

---

## 8. ValidateInput Error Codes for the Workflow Tool

Source context: byte offset **203004507** (inside `validateInput` function body of `cUp = Ks({name: SI, ...})`).

### Error Code 7 — Abort / Input Truncated

Byte offset: **203003931** (`P7a=` starts here)

**Trigger:** `yke(t.abortController.signal)` is true (request was aborted/retracted by server fallback) at either of the two abort checks.

```javascript
{
  result: false,
  message: "Tool dispatch was retracted by a server fallback; the input may be truncated.",
  errorCode: 7
}
```

### Error Code 5 — Disabled by Managed Settings

**Trigger:** `fbn()` returns true (org policy has set `disableWorkflows`).

```javascript
{
  result: false,
  message: "Dynamic workflows are disabled by managed settings (`disableWorkflows`).",
  errorCode: 5
}
```

### Error Code 6 — Not Enabled for Session

**Trigger:** `!pA()` (org policy, launch gate, or `/config` "Dynamic workflows" setting not enabled).

```javascript
{
  result: false,
  message: 'Dynamic workflows are not enabled for this session (org policy, launch gate, or the "Dynamic workflows" setting in /config).',
  errorCode: 6
}
```

### Error Code 1 — Script Resolution Error

**Trigger:** `D7a(e)` returns `{ error: string }` — script/name/scriptPath resolution failed. The `message` is `n.error` where `n` is the D7a result. See sub-errors below.

```javascript
{ result: false, message: n.error, errorCode: 1 }
```

**D7a sub-errors (passed through as the message):**

#### 1a — Must provide script, name, or scriptPath

Byte offset: **203002326**

```text
Must provide script, name, or scriptPath
```

Trigger: none of `e.scriptPath`, `e.name`, or `e.script` is set.

#### 1b — Workflow name not found

Byte offset: **203000051**

```text
Workflow "${e.name}" not found. Available: ${n || "(none)"}
```

Where `n` = comma-joined list of available workflow names. Trigger: `e.name` is set but `K4t(e.name, Mt())` returns null.

#### 1c — UNC path not allowed

Byte offset: **196572915**

```text
UNC paths are not allowed for workflow scriptPath: ${e}
```

Trigger: `au(e)` detects a UNC path in `e.scriptPath`.

#### 1d — Workflow script file not found

Byte offset: **196572915** (same function body)

```text
Workflow script file not found: ${t}
```

Where `t = path.resolve(cwd, e.scriptPath)`. Trigger: `readFileBytes` throws ENOENT.

#### 1e — Failed to read workflow script file

```text
Failed to read workflow script file ${t}: ${n}
```

Trigger: `readFileBytes` throws for a reason other than ENOENT.

#### 1f — Workflow script file exceeds size limit

```text
Workflow script file ${t} exceeds ${P2} bytes
```

Where `P2 = 524288` (512 KB). Trigger: script file is too large.

### Error Code 2 — Invalid Workflow Script (parse/meta error)

Byte offset: **203004930**

**Trigger:** `Bw(n.script)` returns `{ error: string }` (script failed to parse meta block).

```javascript
{ result: false, message: `Invalid workflow script: ${r.error}`, errorCode: 2 }
```

### Error Code 4 — Determinism Violation

Byte offset: **203005037**

**Trigger:** `e.script` is set AND `HKa(r.scriptBody)` returns true (script body uses `Date.now()`, `Math.random()`, or `new Date()`).

```javascript
{
  result: false,
  message: "Workflow scripts must be deterministic: Date.now()/Math.random()/new Date() are unavailable (breaks resume). Stamp results after the workflow returns, or pass timestamps via args.",
  errorCode: 4
}
```

### Error Code 3 — Still-Running Resume Target

Byte offset: **203005037** (same block, after determinism check)

**Trigger:** `e.resumeFromRunId` is set AND a `local_workflow` task with matching `workflowRunId` is still in `status === "running"` in the task registry.

```javascript
{
  result: false,
  message: `Workflow ${e.resumeFromRunId} is still running (task ${o}). Stop it first with ${ED}({taskId: "${o}"}) before resuming.`,
  errorCode: 3
}
```

Where `ED` is the internal name of the TaskStop/StopTask tool (dynamic binding).

---

## LingXi Implementation Targets

| Binary symbol | LingXi location |
|--------------|-----------------|
| `Oho` agentdef | `lingxi-code/agent/src/builtins.rs` |
| `DBp` (schema variant) | same file |
| `DEFAULT_WORKFLOW_SUBAGENT` | `lingxi-code/tasks/src/handlers/local_workflow.rs:129` |
| `validateInput` errors | `lingxi-code/tools/workflow/src/lib.rs` |
| companion subagent note (`yyo`) | tool system-prompt injection layer |
