# Workflow Re-audit Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the ~13 confirmed parity gaps from the 2026-06-23 Workflow re-audit vs the v2.1.186 binary (P0→P3), byte-faithful.

**Architecture:** LingXi is a Rust 1:1 byte-level port of claude-code. The Workflow subsystem spans crates `workflow` (QuickJS runtime + prelude), `tools/workflow` (the model-facing tool: description/schema/validate/result), `tasks` (`handlers/local_workflow.rs` = the agent()→subagent bridge + journaling/budget/concurrency), `agent` (`builtins.rs` agentdefs, `tool_resolver.rs` gating), and `apps/engine-desktop` (launcher wiring). Fixes are localized relocations/additions; no architectural change.

**Tech Stack:** Rust (workspace MSRV 1.82), rquickjs (embedded QuickJS), tokio.

## Global Constraints

- **The oracle is the v2.1.186 binary.** All ported strings must be BYTE-EXACT. The verbatim strings live in `docs/superpowers/oracle-facts/workflow/{agentdef-and-validation,result-and-gating,runtime-and-telemetry}.md` — these fact files are the source of truth; each task names the section to copy from. Em-dashes are U+2014 `—`.
- The leaked TS at `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src` is OLDER (v0.0.0-leaked) — structure hints only, NOT byte-canonical.
- **Build green at every task** with the guarded-ff env: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p <crate>`. Run from `/Users/luolingfeng/Projects/LingXi-Next/.worktrees/workflow-reaudit-fixes/lingxi-code`.
- **NEVER run `cargo fmt`** (repo is not fmt-clean → noise). Match surrounding style by hand.
- Existing workflow test baselines must stay green: `workflow` (35), `tasks` (121), `tool-workflow`/`tool-workflow` (7), `agent` (221).
- Work entirely in the worktree `/Users/luolingfeng/Projects/LingXi-Next/.worktrees/workflow-reaudit-fixes` (branch `worktree-workflow-reaudit-fixes`). A concurrent session is active on `main` — do NOT touch the main checkout.
- **REFUTED — do NOT "fix":** the chain-key hash algorithm (LingXi FNV-1a-64 vs binary SHA-256) is NOT a gap — the journal is LingXi's own same-session format, monotonicity is all that matters. The abort MECHANISM (QuickJS interrupt vs V8 Promise.race) is an inherent rehost difference; only surfacing the `"Workflow aborted"` error string is optional/cosmetic. The `"exceeds the maximum of 4096"` message is a LingXi-ism (the exact string is NOT in the binary) — leave the 4096 cap behavior, do not invent an oracle match.

---

## Task 1 (P0a): Add the `workflow-subagent` builtin agentdef

**Files:**
- Modify: `lingxi-code/agent/src/builtins.rs` (add the `workflow-subagent` builtin alongside `general-purpose` etc.)
- Test: same file's test module (or `agent` crate tests)

**Oracle facts:** `oracle-facts/workflow/agentdef-and-validation.md` §1 (kBp prompt), §2 (xBp schema-variant prompt), §6 (Oho agentdef fields), §5 (DBp = Oho + xBp). The em-dash in kBp/xBp is U+2014.

**Interfaces:**
- Produces: a builtin agentdef resolvable by `agent_type == "workflow-subagent"` with: `whenToUse = "Internal subagent for workflow script orchestration."`, `tools = ["*"]` (All policy), `disallowed_tools = ["SendUserMessage", "Agent", "Workflow"]`, `source = built-in`, system prompt = **kBp** (verbatim from §1). The schema-variant prompt **xBp** (§2, contains the `${Lp}` structured-output-tool-name interpolation — bind it to LingXi's StructuredOutput tool name) must be reachable for Task 2.

- [ ] **Step 1: Write the failing test** — assert a `workflow-subagent` builtin exists with the exact `whenToUse`, `disallowed_tools` set `{SendUserMessage, Agent, Workflow}`, and that its system prompt equals the kBp string (paste the kBp text from §1 as the expected literal).
- [ ] **Step 2: Run it, confirm it fails** (`cargo test -p agent workflow_subagent`).
- [ ] **Step 3: Implement** — add the builtin entry mirroring the existing `general-purpose` entry's shape; system prompt = kBp; the schema-variant prompt xBp as a sibling const for Task 2. Match how other builtins are registered (look at how `general-purpose` is keyed/listed).
- [ ] **Step 4: Run tests green.**
- [ ] **Step 5: Commit** `feat(agent): workflow-subagent builtin agentdef (kBp prompt + disallowedTools)`.

---

## Task 2 (P0b): Route workflow `agent()` to `workflow-subagent` + prompt/disallow logic

**Files:**
- Modify: `lingxi-code/tasks/src/handlers/local_workflow.rs` (`DEFAULT_WORKFLOW_SUBAGENT` @~129; the per-call spawn build in `make_request`/the spawn path)
- Test: same file's test module

**Oracle facts:** `agentdef-and-validation.md` §1 (kBp), §2 (xBp), §3 (HBp non-schema NOTE addendum), §4 (IBp schema NOTE addendum), §6 (disallowedTools union).

**The four cases (mirror the binary):**
1. **Bare `agent(prompt)`** (no `agentType`, no `schema`) → spawn `workflow-subagent` (kBp). (Change `DEFAULT_WORKFLOW_SUBAGENT` from `"general-purpose"` to `"workflow-subagent"`.)
2. **Bare `agent(prompt, {schema})`** (no `agentType`) → spawn the schema variant `DBp` (Oho + **xBp**), force the StructuredOutput tool (existing schema path).
3. **User `agent(prompt, {agentType})`** (no schema) → spawn that agentType, but APPEND the **HBp** NOTE addendum (§3) to its system prompt AND union `disallowed_tools` with `{SendUserMessage, Agent, Workflow}`.
4. **User `agent(prompt, {agentType, schema})`** → that agentType + **IBp** NOTE addendum (§4) + union disallowed + force StructuredOutput.

**Interfaces:**
- Consumes: the `workflow-subagent` builtin + `xBp` const from Task 1.

- [ ] **Step 1: Write failing tests** — (a) bare call → subagent_type `workflow-subagent`, prompt = kBp, SendUserMessage in disallowed; (b) `{schema}` bare → prompt = xBp; (c) `{agentType:"general-purpose"}` → prompt ends with the HBp addendum AND disallowed ⊇ {SendUserMessage, Agent, Workflow}.
- [ ] **Step 2: Run, confirm failures.**
- [ ] **Step 3: Implement** the 4-case selection in the spawn build. Keep `make_request`'s existing opts mapping (agentType/model/isolation/effort/schema/label) intact; add the prompt-selection + disallowed-union.
- [ ] **Step 4: Run `cargo test -p tasks` green.**
- [ ] **Step 5: Commit** `feat(workflow): route agent() to workflow-subagent (kBp/xBp/HBp/IBp + disallow union)`.

---

## Task 3 (P1a): `isEnabled` gate (`disableWorkflows` + env)

**Files:**
- Modify: `lingxi-code/tools/workflow/src/lib.rs` (`is_enabled`)
- Test: same file

**Oracle facts:** `result-and-gating.md` (pA() chain). LingXi can only faithfully evaluate the **deterministic, local** gates of `pA()`: (1) `!disableWorkflows` managed setting, (2) `!CLAUDE_CODE_DISABLE_WORKFLOWS` env. The org/launch (`allow_workflows`) + GrowthBook (`tengu_workflows_enabled`) + plan-availability gates have no LingXi backing → treat as permissive (enabled). Net: **enabled by default; hidden only when `disableWorkflows` managed setting OR `CLAUDE_CODE_DISABLE_WORKFLOWS` env is set.** (This matches the Max/Team/null-plan default; LingXi has no "pro plan" concept to suppress.)

**Interfaces:**
- Consumes: the managed-settings accessor on `ToolStaticContext` (find how other tools read managed settings, e.g. a `disable*` flag) + the env-var read pattern.

- [ ] **Step 1: Write failing tests** — default ctx → enabled; ctx with `disableWorkflows=true` → disabled; `CLAUDE_CODE_DISABLE_WORKFLOWS=1` env → disabled.
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** `is_enabled` reading the managed setting + env (mirror an existing managed-`disable*` tool gate if one exists).
- [ ] **Step 4: Tests green** (`cargo test -p tool-workflow` / the tool-workflow crate).
- [ ] **Step 5: Commit** `feat(workflow): isEnabled gates on disableWorkflows managed-setting + env`.

---

## Task 4 (P1b): `validate_input` error codes 1–7

**Files:**
- Modify: `lingxi-code/tools/workflow/src/lib.rs` (`validate_input`)
- Test: same file

**Oracle facts:** `agentdef-and-validation.md` §8 — all 7 errorCodes with byte-exact messages + triggers, and the §8.1 sub-errors (1a–1f). Match message bytes AND the `errorCode` numbers exactly.

Implement each gate in the binary's order (the existing `script||name||scriptPath` refine becomes sub-error 1a):
- **7 (abort):** if the abort signal is already aborted → `"Tool dispatch was retracted by a server fallback; the input may be truncated."`
- **5 (disableWorkflows managed setting):** `"Dynamic workflows are disabled by managed settings (\`disableWorkflows\`)."`
- **6 (pA gate):** `'Dynamic workflows are not enabled for this session (org policy, launch gate, or the "Dynamic workflows" setting in /config).'` (use the same gate as Task 3; with LingXi's permissive gates this fires only when disabled — keep it for byte-faithful message coverage even if rarely hit.)
- **1 (script resolution):** reuse LingXi's existing `resolve_script`; map its failure to the §8.1 sub-error strings (1a–1f) — `maxLength` const = 524288.
- **2 (parse/meta):** `"Invalid workflow script: ${error}"` when meta parse fails.
- **4 (determinism):** wire the EXISTING `workflow::check_determinism` (already byte-matches the message) into validate_input for inline `script` only → errorCode 4.
- **3 (still-running resume):** when `resumeFromRunId` matches a `local_workflow` task still `running` in the registry → the §8 errorCode-3 message (with the TaskStop tool name binding).

**Interfaces:**
- Consumes: `resolve_script` (tools/workflow), `workflow::check_determinism`, the task-registry lookup for a running `local_workflow` by run id (find the registry handle available to the tool).

- [ ] **Step 1: Write failing tests** — one per errorCode where LingXi has the infra to trigger it (at minimum: 1a presence, 2 parse, 4 determinism `Date.now()`, 5 disableWorkflows). Assert exact message + errorCode.
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** the gates in binary order. If errorCode 3's registry lookup or 7's abort signal isn't reachable from the tool's ctx, mark that sub-gate `⚠️ Cannot wire from ctx` in the report rather than faking it (controller adjudicates).
- [ ] **Step 4: Tests green.**
- [ ] **Step 5: Commit** `feat(workflow): validate_input error codes 1-7 (byte-exact messages)`.

---

## Task 5 (P2a): Result fields `summary` + `transcriptDir`

**Files:**
- Modify: `lingxi-code/tools/workflow/src/lib.rs` (`WorkflowLaunched` struct + the `call()` result JSON + the model-facing result text)
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` (`TaskRegistryWorkflowLauncher::launch` — populate summary + transcriptDir)
- Test: tool-workflow + (launcher) engine-desktop or a unit test on the result builder

**Oracle facts:** `result-and-gating.md` §1 (result text template), §2 (object shape), §3 (transcriptDir derivation). `summary = meta.description`; `transcriptDir = <sessionProjectDir>/<sessionId>/subagents/workflows/<runId>`. The launch result text is `"Workflow launched in background. Task ID: ${taskId}"` + conditional `"\nSummary: ${summary}"` + conditional `"\nTranscript dir: ${transcriptDir}"` + conditional script/runId lines + `"\n\nYou will be notified when it completes. Use /workflows to watch live progress."` — match byte-exact (verify the full template in §1 before writing).

**Interfaces:**
- Produces: `WorkflowLaunched` gains `summary: Option<String>`, `transcript_dir: Option<String>`; the result JSON inserts them when present; the result text renders the conditional lines.
- Consumes: `meta.description` (already extracted for `workflow_name`=meta.name — extract description alongside); session project dir + session id (find how the launcher knows the session dir).

- [ ] **Step 1: Write failing test** — a launch with `meta.description` set → result JSON has `summary` + the text contains `"\nSummary: ..."`; `transcriptDir` present + `"Transcript dir: "` line. Verify the FULL launch-text template byte-for-byte against §1.
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** — thread `summary`/`transcript_dir` through `WorkflowLaunched` + launcher; render the text template byte-exact.
- [ ] **Step 4: Tests green.**
- [ ] **Step 5: Commit** `feat(workflow): result summary + transcriptDir + byte-exact launch text`.

---

## Task 6 (P2b): Runtime message + constant drifts

**Files:**
- Modify: `lingxi-code/workflow/src/lib.rs` (prelude `parallel()` msg + 2nd throw; `workflow()` nesting msg)
- Modify: `lingxi-code/tools/workflow/src/lib.rs` (`max_result_size_chars` 16384 → 100000)
- Modify: `lingxi-code/tasks/src/handlers/local_workflow.rs` (concurrency floor)
- Test: each crate's tests

**Oracle facts:** `runtime-and-telemetry.md` §1 (parallel msgs), §2 (nesting msg), §3 (concurrency `Math.min(16, Math.max(2, cpus-2))`).

- [ ] **Step 1: Write failing tests** — (a) `parallel("x")` throw == `"parallel() expects an array of functions"`; (b) `parallel([promise])`-style non-function items throw == `"parallel() expects an array of functions, not promises. Wrap each call: () => agent(...)"`; (c) nested `workflow()` throw == the §2 message; (d) `max_result_size_chars()==100000`; (e) concurrency cap on 2 cores == 2 (floor 2).
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** — fix the prelude strings ("thunks"→"functions" + add the 2nd throw for non-function/promise items), the nesting message, `max_result_size_chars` → 100000, and the concurrency floor to `cores.saturating_sub(2).clamp(2, 16)` — BUT note: a 1-core machine yields `max(2,-1)`→2 in the binary, so use `(cores.max(3) - 2).clamp(2,16)` or equivalently `cores.saturating_sub(2).max(2).min(16)`. Verify the formula reproduces: 1→2, 2→2, 3→2, 4→2, 5→3, 18→16.
- [ ] **Step 4: Tests green** across the three crates.
- [ ] **Step 5: Commit** `fix(workflow): byte-exact parallel/nesting msgs + maxResultSizeChars 1e5 + concurrency floor 2`.

---

## Task 7 (P2c): Companion subagent-prompt note for filtered Workflow

**Files:**
- Modify: the subagent system-prompt injection layer — where LingXi filters disallowed tools for a subagent and (per the binary) appends the "not available inside subagents" note. Start from `lingxi-code/agent/src/tool_resolver.rs` (where `Workflow`/`Agent` are dropped for non-ant) and trace to the system-prompt assembly. (The bg-subagent/agent-parity work added related `<env>`/notice plumbing — reuse that injection point if present.)
- Test: the relevant crate

**Oracle facts:** `agentdef-and-validation.md` §7 — the `yyo` note: `". ${toolName} is not available inside subagents. Complete the task with the tools provided and return findings to the orchestrator."` (leading `. `, appended to an in-progress sentence). Applies to each tool in the subagent-disallowed set that was filtered.

- [ ] **Step 1: Write failing test** — a non-ant subagent whose pool dropped `Workflow` → its system prompt contains the §7 note with `Workflow` interpolated.
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** — append the note at the injection point for each filtered subagent-disallowed tool. If the note in the binary is gated to specific tools (`nke` set), match that set.
- [ ] **Step 4: Tests green.**
- [ ] **Step 5: Commit** `feat(agent): companion "not available inside subagents" note`.
- **NOTE:** if this proves entangled with the ant-gate or no clean injection point exists, report `⚠️ BLOCKED` with the specific obstacle — do not hack it in.

---

## Task 8 (P3a): Resume-cache key normalization (`ABp`)

**Files:**
- Modify: `lingxi-code/tasks/src/handlers/local_workflow.rs` (`chain_key` input)
- Test: same file

**Oracle facts:** `runtime-and-telemetry.md` §4 — `ABp` normalizes opts to ONLY `["schema","model","effort","isolation","agentType"]` (that order; skip undefined/function values) then `JSON.stringify` with a recursive key-sorter. Keep LingXi's FNV-1a-64 (REFUTED that the algo matters); only the INPUT must be the normalized subset, not the raw `opts_json`.

- [ ] **Step 1: Write failing test** — two agent() opts differing ONLY in `phase`/`label` produce the SAME chain key; differing in `model` produce DIFFERENT keys.
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** — before `chain_key`, project the opts object to the 5 keys (sorted, undefined/fn skipped), serialize, hash that.
- [ ] **Step 4: Tests green.**
- [ ] **Step 5: Commit** `fix(workflow): resume chain-key normalizes opts to [schema,model,effort,isolation,agentType]`.

---

## Task 9 (P3b): Telemetry — `tengu_workflow_*` events

**Files:**
- Modify: `lingxi-code/tasks/src/handlers/local_workflow.rs` + `lingxi-code/tools/workflow/src/lib.rs` (+ launcher for launch event)
- Test: capture emitted events in the relevant crate's test harness

**Oracle facts:** `runtime-and-telemetry.md` §6 — the 12 events + payload fields + emit conditions: `tengu_workflow_launched`, `tengu_workflow_completed`, `tengu_workflow_phase_completed`, `tengu_workflow_agent_cap_exceeded`, `tengu_workflow_budget_cap_exceeded`, `tengu_workflow_journal_started_hit_respawn`, `tengu_workflow_saved`, `tengu_workflows_enabled`, `tengu_workflow_keyword`, `tengu_workflow_keyword_dismissed`, `tengu_workflow_keyword_restored`, `tengu_workflow_usage_warning_accepted`. (The `keyword*` + `usage_warning_accepted` + `saved` events are tied to UI/keyword-trigger flows LingXi may not have — emit the ones whose trigger conditions LingXi reaches: launched/completed/phase_completed/agent_cap_exceeded/budget_cap_exceeded/journal_started_hit_respawn/workflows_enabled. For UI-only events with no LingXi trigger, record `⚠️ no trigger in LingXi` in the report rather than emitting dead events.)

**Interfaces:**
- Consumes: LingXi's telemetry emit API (find how other tools emit `tengu_*` — e.g. `telemetry::log_event` or the established macro).

- [ ] **Step 1: Write failing tests** — assert `tengu_workflow_launched` (with its payload keys) emits on launch; `tengu_workflow_completed` on completion; `tengu_workflow_agent_cap_exceeded` when the 1000 cap fires; `tengu_workflow_budget_cap_exceeded` on budget throw.
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** the reachable events with byte-exact names + payload field names from §6. Wire cap/budget events at the existing throw sites (`local_workflow.rs` agent-cap/budget checks).
- [ ] **Step 4: Tests green.**
- [ ] **Step 5: Commit** `feat(workflow): tengu_workflow_* telemetry (launched/completed/phase/cap/budget/respawn/enabled)`.

---

## Task 10 (P3c): Structured progress events (`workflow_agent`/`workflow_phase`/`workflow_log`)

**Files:**
- Modify: `lingxi-code/tasks/src/handlers/local_workflow.rs` (the progress emission — currently `format_progress` emits only text lines) + `lingxi-code/workflow/src/lib.rs` if the per-agent lifecycle needs new progress hooks
- Test: same

**Oracle facts:** `runtime-and-telemetry.md` §7 — `{type:"workflow_agent", index, label, phaseIndex, phaseTitle, agentId, model, state, startedAt, queuedAt, promptPreview, lastProgressAt}` with `state` ∈ {queued,start,done,error,cached} (confirm enum), `toolUseID = "workflow_agent_${index}_${agentId}"`; `{type:"workflow_phase", index, title, kind}` (`kind` always undefined → omit); `{type:"workflow_log", message}`.

**Interfaces:**
- Consumes: LingXi has NO `/workflows` TUI consumer — these events go to the task-output spool (same channel `format_progress` uses today). Emit them as structured progress entries so the data is faithful even without a TUI.

- [ ] **Step 1: Write failing test** — running a 1-agent script emits a `workflow_agent` progress entry with `index`/`label`/`state` transitions (queued→start→done) and the `toolUseID` format; a `phase()` emits `workflow_phase`; a `log()` emits `workflow_log`.
- [ ] **Step 2: Run, confirm fail.**
- [ ] **Step 3: Implement** — extend the progress channel to carry the structured `workflow_agent`/`workflow_phase`/`workflow_log` shapes (timestamps: per the no-`Date.now()` constraint in scripts, these are HOST-side stamps — use the host clock in Rust, fine). Keep the existing text spool for back-compat or replace per how the task output renders.
- [ ] **Step 4: Tests green.**
- [ ] **Step 5: Commit** `feat(workflow): structured workflow_agent/workflow_phase/workflow_log progress events`.
- **NOTE:** timestamps `startedAt`/`queuedAt`/`lastProgressAt` are host-stamped (NOT script-visible `Date.now()`), so they don't violate the determinism rule. If the progress channel can't carry structured data without a large refactor, report the constraint and propose the minimal faithful subset (index+label+state).

---

## Self-Review notes (controller)
- Tasks 1+2 are the P0 core (highest behavioral value). Task 4 (validate) and Tasks 9+10 (telemetry/progress) are the largest; their `⚠️`-escape clauses let the implementer flag genuinely-unreachable sub-parts for adjudication instead of faking.
- Cross-task type consistency: `WorkflowLaunched` gains `summary`+`transcript_dir` (Task 5); `workflow-subagent` builtin + `xBp` const (Task 1) are consumed by Task 2. No naming drift.
- Each task ends green + committed; final whole-branch review (opus) after Task 10, then finishing-a-development-branch.
