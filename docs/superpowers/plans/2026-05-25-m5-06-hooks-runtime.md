# LingXi Core M5 · Plan 06 · Hooks runtime — 4-arm executor + PreToolUse/PostToolUse wiring

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Promote `lingxi-hooks::executor::HookExecutorImpl` from "Builtin-only, three stubs" (M1.4) to a full 4-arm executor (`Builtin` / `Http` / `Command` / `Agent`) and wire the `ConversationOrchestrator::run_turn` tool-dispatch path to fire `PreToolUse` *before* every tool invocation and `PostToolUse` *after* every successful result. After this plan a registered HTTP hook can intercept any tool, a Bash/Python script hook can vet inputs via stdin/stdout, and an agent-spawn hook can produce structured responses by forking a subagent (reusing the M4-05 `SubagentSpawner` Arc).

This plan ships:

- A new module tree under `lingxi-core/crates/hooks/src/`:
  - `hook_payload.rs` — serde-locked `PreToolUsePayload` / `PostToolUsePayload` structs whose JSON keys match `claude-code/src/entrypoints/sdk/coreSchemas.ts:414-446` byte-for-byte (`hook_event_name`, `tool_name`, `tool_input`, `tool_response`, `tool_use_id`, `session_id`, `transcript_path`, `cwd`, `permission_mode?`, `agent_id?`, `agent_type?`). Plus the inbound `HookResponseBody` parser that lifts claude-code's `{ continue?, stopReason?, suppressOutput?, systemMessage?, decision?, permissionDecision?, hookSpecificOutput? }` shape into our `HookResponse`.
  - `http_executor.rs` — `HttpExecutor { http: Arc<dyn HttpTransport>, ssrf_guard: SsrfGuard }`. POSTs the `PreToolUsePayload` / `PostToolUsePayload` JSON to `hook.url` with `Content-Type: application/json`, enforces `HOOK_HTTP_TIMEOUT_MS = 600_000` (10 minutes — see T0) or per-hook override, parses the body via `hook_payload::parse_response`, returns a `HookResult`.
  - `command_executor.rs` — `CommandExecutor { runtime: Arc<dyn RuntimeSpawner> }`. Spawns `hook.command` with `hook.args` via the runtime spawner (D17 — no direct `tokio::process`), pipes the payload JSON to stdin, captures stdout/stderr, parses stdout via `hook_payload::parse_response`. Default timeout `HOOK_COMMAND_TIMEOUT_MS = 600_000` (10 minutes — same as TOOL_HOOK_EXECUTION_TIMEOUT_MS in claude-code).
  - `agent_executor.rs` — `AgentExecutor { spawner: Arc<dyn SubagentSpawner> }`. Builds a `SubagentSpawnRequest { subagent_type: hook.agent_type.clone(), prompt: hook.prompt_template + payload_json, context_paths: vec![] }`, awaits `SubagentResult::Completed { content, .. }`, treats `content` as `HookResponse` JSON (or empty success if absent). Default timeout `HOOK_AGENT_TIMEOUT_MS = 60_000` (60 seconds — claude-code's `execAgentHook.ts:75`).
- A modification to `lingxi-core/crates/hooks/src/executor.rs`:
  - `HookExecutorImpl` gains a 4th field `Option<Arc<dyn SubagentSpawner>>` (None means agent-arm hooks return `HookOutcome::Error` with `"agent executor not wired"`). The HTTP and Command arms always work — they reuse the already-stored `http` and `runtime` fields.
  - `HookExecutorImpl::new` keeps its current 3-arg signature; a new builder method `with_agent_spawner(spawner: Arc<dyn SubagentSpawner>)` attaches the agent arm. Production wiring (the orchestrator constructor) calls `with_agent_spawner(spawner.clone())`; tests that don't exercise agent hooks construct without it.
  - The match in `execute_single` replaces the three "stubbed" arms with delegating calls to the three new executor structs. The `Builtin` arm is untouched.
- A wiring change in `lingxi-core/crates/orchestrator/src/conversation.rs`:
  - `ConversationOrchestrator` grows a new public method `dispatch_tool_with_hooks(name, input, tool_use_id) -> Result<ToolCallResult, ToolError>` that:
    1. Constructs a `HookEvent::PreToolUse { tool_name, tool_input, tool_use_id }` and calls `self.hooks.execute(event, ctx).await`.
    2. If the aggregate `decision == Some(HookDecision::Block)`, returns `Err(ToolError::PermissionDenied(reason))`.
    3. If `modified_input.is_some()`, replaces `input` with the mutated copy.
    4. Calls the underlying tool (existing M5-02 path).
    5. On success, fires `HookEvent::PostToolUse { tool_name, tool_input: <post-mutation>, tool_output, tool_use_id }` and lets a hook optionally mutate the result via the same `modified_input` field (re-used as `modified_response` per spec §4.5 — see "Response-mutation aliasing" lock below).
    6. Returns the (possibly mutated) `ToolCallResult`.
  - `run_turn`'s existing tool-dispatch loop calls `dispatch_tool_with_hooks` instead of the direct `tools.invoke`.
- 8 new telemetry events on `lingxi-telemetry::tengu::orchestrator`:
  `hook_pre_started`, `hook_pre_completed`, `hook_pre_failed`, `hook_post_started`, `hook_post_completed`, `hook_post_failed`, `hook_http_skipped_ssrf`, `hook_timeout`. `ALL_EVENT_NAMES` grows from **245 → 253**.
- 6 integration test files under `crates/hooks/tests/` and `crates/orchestrator/tests/` covering: HTTP arm happy path, HTTP SSRF block + telemetry, HTTP timeout + telemetry, Command arm happy path + non-zero exit, Agent arm happy path via mock spawner, orchestrator end-to-end with scripted Pre/PostToolUse hooks.

**No regressions to M4-05 wiring.** The `Arc::ptr_eq` recursion-lock + budget-inheritance tests (`lingxi-tools/tests/agent_tool_recursion_lock_test.rs` and `agent_tool_budget_inheritance_test.rs`) MUST continue to pass byte-for-byte. Task 13 step 5 re-runs them explicitly after the executor change.

**Tech Stack:** Rust 2021. Reused workspace deps — `async-trait 0.1`, `serde 1` + `serde_json 1` (with `preserve_order`), `tokio 1`, `thiserror 2`, `url 2`. **Zero new third-party deps.** The Command arm uses the already-existing `RuntimeSpawner::spawn_with_stdin` method (M2-04). The HTTP arm uses the existing `HttpTransport::request_with_timeout` (M3-03). The Agent arm uses the already-existing `SubagentSpawner` from M4-05. **One workspace `Cargo.toml` change** — `lingxi-hooks` adds `lingxi-traits = { workspace = true, features = ["subagent"] }` if the `subagent` feature is gated (Task 1 step 2 reconciles by reading the current `traits/Cargo.toml`).

**References:**

- Spec: `docs/superpowers/specs/2026-05-25-m5-conversational-agent-loop-design.md` (committed at `1dbb9b8`).
  - §3 sub-plan row M5-06 (line 182) — "`Http`/`Command`/`Agent` arms 从 stub → 真实现 … 8 telemetry events … ~17 tasks".
  - §4.5 (lines 275-282) — Hook event JSON schema, hook timeout (60s default? → confirmed 600s in T0), SSRF block list (already locked in M1.7), error format `"Hook ${id} failed: ${reason}"`.
  - §4.9 telemetry row M5-06 (line 322) — 8 names listed.
  - §6.3 telemetry growth (line 439) — "M5-06 后 253 (+8)".
  - §7 OQ-4 (line 494) — "Hook event JSON schema". Resolved by T0 below.
- Predecessor M5-05 (committed at `297e3bc`):
  - `ConversationOrchestrator` now has 10 fields including `hooks: Arc<HookExecutorImpl>` (added in M5-02 Task 10) and `perms: Arc<dyn PermissionGate>` (M5-05 promoted to `lingxi-traits::permission_gate`).
  - `ALL_EVENT_NAMES.len() == 245`. `tengu::orchestrator::NAMES` has 7 entries (`conversation_started/completed/failed` + `turn_streaming_started/completed` + `permission_prompted/answered`).
  - `tengu::mod.rs:29` TOTAL formula reads `25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 7 + 1 = 245`. **Task 16 step 2 bumps the orchestrator's `7` to `15`** (orchestrator submodule grows from 7 → 15 entries), making `TOTAL = 253`.
- Predecessor M5-02 (committed at `653de44`):
  - `ConversationOrchestrator::run_turn` has its tool-dispatch loop at `conversation.rs:run_turn` calling `self.tools.invoke(name, input, ctx).await` directly. Task 14 of this plan inserts a `dispatch_tool_with_hooks` indirection between the loop and `tools.invoke`.
- Predecessor M5-01 (committed at `c477822`):
  - `RegistryToolInvoker` is the concrete `ToolInvoker` impl used by the orchestrator; nothing in this plan changes its surface.
- Predecessor M4-05 wiring follow-up (committed at `106 completed`):
  - `SubagentSpawner` trait is fully wired with `Arc::ptr_eq` recursion-lock + budget-inheritance invariants in `lingxi-tools/tests/agent_tool_*_test.rs`. T13/T14/T15 of this plan add an explicit "re-run M4-05 invariant tests" gate to catch any accidental Arc-clone introduced by the new code path.
- claude-code reference (Task 0 reverse-engineering — exact line numbers verified at plan-writing time):
  - `claude-code/src/utils/hooks.ts:166` — `const TOOL_HOOK_EXECUTION_TIMEOUT_MS = 10 * 60 * 1000` (600 000 ms). Used as default for command + http hooks.
  - `claude-code/src/utils/hooks/execHttpHook.ts:12` — `const DEFAULT_HTTP_HOOK_TIMEOUT_MS = 10 * 60 * 1000` (600 000 ms) — explicit comment "matches TOOL_HOOK_EXECUTION_TIMEOUT_MS".
  - `claude-code/src/utils/hooks/execAgentHook.ts:75` — `hookTimeoutMs = hook.timeout ? hook.timeout * 1000 : 60000` (60 000 ms default for agent hooks).
  - `claude-code/src/utils/hooks/execPromptHook.ts:55` — `hookTimeoutMs = hook.timeout ? hook.timeout * 1000 : 30000` (30 000 ms — prompt/LLM hook; we do NOT implement this arm — claude-code's "prompt" hook becomes our "Agent" hook because we route through `SubagentSpawner`).
  - `claude-code/src/entrypoints/sdk/coreSchemas.ts:414-423` — `PreToolUseHookInputSchema`: `BaseHookInput & { hook_event_name: "PreToolUse", tool_name: string, tool_input: unknown, tool_use_id: string }`.
  - `claude-code/src/entrypoints/sdk/coreSchemas.ts:436-446` — `PostToolUseHookInputSchema`: `BaseHookInput & { hook_event_name: "PostToolUse", tool_name: string, tool_input: unknown, tool_response: unknown, tool_use_id: string }`.
  - `claude-code/src/utils/hooks.ts:301-328` — `createBaseHookInput`: `{ session_id, transcript_path, cwd, permission_mode?, agent_id?, agent_type? }`.
  - `claude-code/src/utils/hooks.ts:336-360` — `HookResult` interface (TypeScript) defines `outcome: 'success' | 'blocking' | 'non_blocking_error' | 'cancelled'`, plus `systemMessage?, blockingError?, permissionBehavior? ('ask'|'deny'|'allow'|'passthrough'), additionalContext?, updatedInput?`.
  - `claude-code/src/utils/hooks.ts:540-680` (response-processing block) — `hookSpecificOutput.{ hookEventName, permissionDecision: "allow"|"deny"|"ask", permissionDecisionReason, updatedInput, additionalContext }`. The top-level `decision` field is the legacy `"block"|"approve"` taxonomy.

---

## Reverse-engineered byte-locks (T0 — captured at plan-writing time)

| Lock id | Value | Source |
|---|---|---|
| `HOOK_HTTP_TIMEOUT_MS` | `600_000` (10 minutes) | `claude-code/src/utils/hooks/execHttpHook.ts:12` (`DEFAULT_HTTP_HOOK_TIMEOUT_MS = 10 * 60 * 1000`) |
| `HOOK_COMMAND_TIMEOUT_MS` | `600_000` (10 minutes) | `claude-code/src/utils/hooks.ts:166` (`TOOL_HOOK_EXECUTION_TIMEOUT_MS = 10 * 60 * 1000`); fall-through default for command hooks in `executeHooks` at line 2195: `commandTimeoutMs = hook.timeout ? hook.timeout * 1000 : timeoutMs` where the caller passes `TOOL_HOOK_EXECUTION_TIMEOUT_MS`. |
| `HOOK_AGENT_TIMEOUT_MS` | `60_000` (60 seconds) | `claude-code/src/utils/hooks/execAgentHook.ts:75` (`hookTimeoutMs = hook.timeout ? hook.timeout * 1000 : 60000`) |
| `PreToolUsePayload` JSON keys (in registration order) | `hook_event_name` (literal `"PreToolUse"`), `session_id`, `transcript_path`, `cwd`, `permission_mode` (skip if `None`), `agent_id` (skip if `None`), `agent_type` (skip if `None`), `tool_name`, `tool_input`, `tool_use_id` | `coreSchemas.ts:414-423` |
| `PostToolUsePayload` JSON keys | identical to Pre + `tool_response` (after `tool_input`, before `tool_use_id`) | `coreSchemas.ts:436-446` |
| `HookResponseBody` JSON keys (canonical M5-06 mapping) | `continue` → ignored (passthrough hint, not blocking), `stopReason` → `HookResponse.reason` when `continue == false`, `suppressOutput` → `HookResponse.suppress_output`, `systemMessage` → `HookResponse.system_message`, `decision` ("block" → `HookDecision::Block`, "approve" → `HookDecision::Approve`, any other → ignore), `permissionDecision` ("allow" → `Approve`, "deny" → `Block`, "ask" → no decision change), `permissionDecisionReason` → `HookResponse.reason` (preferred over `stopReason` when both present), `hookSpecificOutput.hookEventName` (validated against expected, mismatch → parse error), `hookSpecificOutput.updatedInput` → `HookResponse.updated_input`, `hookSpecificOutput.additionalContext` → appended to `HookResponse.system_message` (newline-joined when both present) | `claude-code/src/utils/hooks.ts:540-680` |
| Response-mutation aliasing for PostToolUse | claude-code's PostToolUse response uses `hookSpecificOutput.additionalContext` rather than `updatedInput` (PostToolUse cannot mutate the prior input — it's already invoked). We map `additionalContext` from a Post hook to `HookResponse.system_message` and use it to **append** to the tool result text in `dispatch_tool_with_hooks`. The `updated_input` field, if returned by a Post hook, is ignored with a debug log (`"PostToolUse hook returned updatedInput — ignored (invalid for Post)"`). | `hooks.ts:625` |
| SSRF rules | RFC1918 + 127/8 + 169.254/16 (link-local) — already locked in `lingxi-hooks::ssrf_guard` M1.7. T5 of this plan adds the explicit 169.254/16 range to the existing block list (currently 10/8, 172.16/12, 192.168/16, 127/8 — link-local was deferred to M2 per `ssrf_guard.rs:62-64`, NOW added because M5-06 routes hooks to public endpoints where 169.254.169.254 is the cloud metadata service). | `ssrf_guard.rs:62-89` |
| Error format on executor failure | `"Hook {id} failed: {reason}"` (literal `"Hook "`, hook id in `Display` form, `" failed: "`, error reason) — emitted into `HookResult.stderr` for every non-Success outcome. The orchestrator does NOT surface this string to the user directly; it's a telemetry-only label. | `hooks.ts:1311` (`Error occurred while executing hook command: ${errorMsg}`) — adapted to our shape because claude-code uses a different prefix per arm; M5-06 normalises to one. |

### 8 new telemetry event names (T16 lock)

All emitted on the `lingxi-telemetry::tengu::orchestrator` submodule (the same submodule M5-02 / M5-04 / M5-05 already populated). Names are `tengu_orchestrator_<verb>_<noun>` per repo convention.

| Const name | Wire name | Payload fields |
|---|---|---|
| `HOOK_PRE_STARTED` | `tengu_orchestrator_hook_pre_started` | `tool_name: PiiTagged, hook_id: Verified, hook_kind: Verified ("builtin"\|"http"\|"command"\|"agent")` |
| `HOOK_PRE_COMPLETED` | `tengu_orchestrator_hook_pre_completed` | `tool_name: PiiTagged, hook_id: Verified, decision: Verified ("allow"\|"block"\|"approve"\|"continue"\|"none"), duration_ms: Verified` |
| `HOOK_PRE_FAILED` | `tengu_orchestrator_hook_pre_failed` | `tool_name: PiiTagged, hook_id: Verified, reason: PiiTagged` |
| `HOOK_POST_STARTED` | `tengu_orchestrator_hook_post_started` | `tool_name: PiiTagged, hook_id: Verified, hook_kind: Verified` |
| `HOOK_POST_COMPLETED` | `tengu_orchestrator_hook_post_completed` | `tool_name: PiiTagged, hook_id: Verified, duration_ms: Verified, mutated_response: Verified (bool)` |
| `HOOK_POST_FAILED` | `tengu_orchestrator_hook_post_failed` | `tool_name: PiiTagged, hook_id: Verified, reason: PiiTagged` |
| `HOOK_HTTP_SKIPPED_SSRF` | `tengu_orchestrator_hook_http_skipped_ssrf` | `hook_id: Verified, url: PiiTagged ("_PROTO_url" tag per spec §4.5), reason: Verified` |
| `HOOK_TIMEOUT` | `tengu_orchestrator_hook_timeout` | `tool_name: PiiTagged, hook_id: Verified, hook_kind: Verified, timeout_ms: Verified` |

**Telemetry chain re-assertion:** `25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 15 + 1 = 253` (the orchestrator submodule's count goes 7 → 15 — Task 16 step 2 updates the `TOTAL` formula and the `event_name_completeness_test.rs` assertion in lock-step).

---

## Design locks

- **`HookExecutorImpl` field layout (post-M5-06):**
  ```rust
  pub struct HookExecutorImpl {
      registry: Arc<RwLock<HookRegistry>>,
      http: Arc<dyn HttpTransport>,
      runtime: Arc<dyn RuntimeSpawner>,
      builtin_handlers: HashMap<String, Arc<dyn BuiltinHookHandler>>,
      ssrf_guard: SsrfGuard,
      agent_spawner: Option<Arc<dyn SubagentSpawner>>,   // NEW (T13)
      telemetry: Arc<dyn TelemetryEmitter>,              // NEW (T16) — already a workspace trait
  }
  ```
  The `runtime` field was `#[allow(dead_code)]` in M1.4; this plan removes that attribute when T8 actually uses it.

- **`HookEventEnvelope` (the JSON payload that goes over the wire / down stdin):**
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(untagged)]
  pub enum HookEventEnvelope {
      PreToolUse(PreToolUsePayload),
      PostToolUse(PostToolUsePayload),
  }
  ```
  `untagged` is correct because the discriminator `hook_event_name` already lives inside the variant. `serde_json::to_string(&envelope)` produces exactly the byte-locked shape claude-code expects.

- **`PreToolUsePayload` Rust shape:**
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct PreToolUsePayload {
      pub hook_event_name: HookEventNamePre,    // serializes as the literal "PreToolUse"
      pub session_id: String,
      pub transcript_path: String,
      pub cwd: String,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub permission_mode: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub agent_id: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub agent_type: Option<String>,
      pub tool_name: String,
      pub tool_input: serde_json::Value,
      pub tool_use_id: String,
  }
  ```
  `HookEventNamePre` is a unit struct with a custom `Serialize`/`Deserialize` that round-trips `"PreToolUse"` only. Same pattern for `PostToolUsePayload` with `tool_response: Value` inserted between `tool_input` and `tool_use_id`, and `HookEventNamePost` literal `"PostToolUse"`.

- **Response parser (`hook_payload::parse_response`):**
  ```rust
  pub fn parse_response(
      raw: &str,
      expected_event: &'static str,   // "PreToolUse" or "PostToolUse"
  ) -> Result<HookResponse, HookResponseParseError>
  ```
  Steps:
  1. `serde_json::from_str::<serde_json::Value>(raw)` — bubble `HookResponseParseError::Json(e)` on error.
  2. If top-level is not an object → `Err(HookResponseParseError::NotObject)`.
  3. Read `continue` (bool, default `true`); if `false` and `stopReason` is a string, copy `stopReason` into `reason`.
  4. Read `suppressOutput` (bool, default `false`) into `HookResponse.suppress_output`.
  5. Read `systemMessage` (string) into `HookResponse.system_message`.
  6. Read legacy `decision` (string): `"block"` → `Block`, `"approve"` → `Approve`, others ignored.
  7. Read `permissionDecision` (string): `"allow"` → `Approve` (overrides legacy `decision` only if it was `None`), `"deny"` → `Block`, `"ask"` → no change.
  8. Read `permissionDecisionReason` (string) into `HookResponse.reason` (overrides `stopReason`).
  9. Read `hookSpecificOutput` (object); validate `hookEventName == expected_event` → `Err(HookResponseParseError::EventNameMismatch { expected, got })` on mismatch; copy `updatedInput` into `HookResponse.updated_input`; concat `additionalContext` into `HookResponse.system_message` with newline separator if non-empty.
  10. Return.

- **`HookResponseParseError`:**
  ```rust
  #[derive(Debug, Clone, thiserror::Error)]
  pub enum HookResponseParseError {
      #[error("hook response is not valid JSON: {0}")]
      Json(String),
      #[error("hook response is not a JSON object")]
      NotObject,
      #[error("hook response hookEventName mismatch: expected '{expected}', got '{got}'")]
      EventNameMismatch { expected: &'static str, got: String },
  }
  ```

- **`HttpExecutor` shape:**
  ```rust
  pub(crate) struct HttpExecutor {
      http: Arc<dyn HttpTransport>,
      ssrf_guard: SsrfGuard,
      timeout: Duration,                      // defaults to HOOK_HTTP_TIMEOUT_MS
  }
  ```
  Single method `pub async fn execute(&self, hook: &HookDefinition, url: &str, headers: &HashMap<String, String>, body: &str, expected_event: &'static str) -> HookResult`. The body is the pre-serialized envelope JSON. On SSRF rejection returns `HookResult { outcome: HookOutcome::Error, stderr: format!("Hook {id} failed: SSRF guard rejected url"), .. }` AND emits `HOOK_HTTP_SKIPPED_SSRF`. On `tokio::time::timeout` elapsing returns `HookOutcome::Timeout` AND emits `HOOK_TIMEOUT`.

- **`CommandExecutor` shape:**
  ```rust
  pub(crate) struct CommandExecutor {
      runtime: Arc<dyn RuntimeSpawner>,
      timeout: Duration,                      // HOOK_COMMAND_TIMEOUT_MS
  }
  ```
  Single method `pub async fn execute(&self, hook: &HookDefinition, command: &str, args: &[String], env: &HashMap<String, String>, cwd: Option<&Path>, stdin_payload: &str, expected_event: &'static str) -> HookResult`. Uses `RuntimeSpawner::spawn_with_stdin(command, args, env, cwd, stdin_payload, self.timeout)` — that method already exists in `lingxi-traits::runtime::RuntimeSpawner` (M2-04 added it). On non-zero exit, returns `HookResult { outcome: HookOutcome::Error, exit_code: Some(code), .. }` but STILL attempts to parse stdout (claude-code's pattern: a hook can return JSON + non-zero exit to mean "non-fatal advisory").

- **`AgentExecutor` shape:**
  ```rust
  pub(crate) struct AgentExecutor {
      spawner: Option<Arc<dyn SubagentSpawner>>,
      timeout: Duration,                      // HOOK_AGENT_TIMEOUT_MS
  }
  ```
  Single method `pub async fn execute(&self, hook: &HookDefinition, agent_type: &str, prompt_template: &str, payload_json: &str, expected_event: &'static str, inherit: SubagentInheritance) -> HookResult`. Returns `HookOutcome::Error` with `stderr: "Hook {id} failed: agent executor not wired"` when `spawner.is_none()`. Otherwise builds `SubagentSpawnRequest { subagent_type: agent_type.into(), prompt: format!("{prompt_template}\n\n{payload_json}"), context_paths: vec![] }`, awaits `spawner.spawn(req, inherit)` with `tokio::time::timeout(self.timeout, ...)`. Maps `SubagentResult::Completed { content, .. }` → `parse_response(content.to_string(), expected_event)`; `Failed { reason }` → `HookOutcome::Error`; `Killed` → `HookOutcome::Cancelled`; timeout elapsed → `HookOutcome::Timeout`.

- **`AgentExecutor`'s `SubagentInheritance` source:** the orchestrator owns the canonical `Arc<dyn ToolInvoker>` (its own `RegistryToolInvoker` Arc) and the canonical `Arc<dyn BudgetEnforcerHandle>`. The orchestrator passes these to `HookExecutorImpl::execute` via a new field on `HookContext` (`HookContext` gains `inherit: Option<SubagentInheritance>` — Task 11 step 3 adds the field). `HookExecutorImpl::execute_single` then passes `ctx.inherit.clone().ok_or(...)?` into the agent arm. **No new Arc clones** — the orchestrator holds these Arcs already and only clones them once when building the context per turn.

- **Telemetry emission ownership:** the executor `HookExecutorImpl` owns the `telemetry: Arc<dyn TelemetryEmitter>` field and fires `HOOK_HTTP_SKIPPED_SSRF` + `HOOK_TIMEOUT` directly (since those are arm-level events). The orchestrator (`dispatch_tool_with_hooks`) fires the 6 `hook_pre_*` / `hook_post_*` events (since those are turn-level). Both use the same `lingxi-telemetry::tengu::orchestrator` constants. This split is intentional — it keeps the executor decoupled from the orchestrator's turn cadence.

- **`dispatch_tool_with_hooks` (orchestrator wiring):**
  ```rust
  async fn dispatch_tool_with_hooks(
      &self,
      tool_name: &str,
      mut tool_input: serde_json::Value,
      tool_use_id: ToolUseId,
  ) -> Result<ToolCallResult, ToolError> {
      // --- PreToolUse ---
      let pre_event = HookEvent::PreToolUse {
          tool_name: tool_name.to_string(),
          tool_input: tool_input.clone(),
          tool_use_id: tool_use_id.clone(),
      };
      let pre_ctx = self.build_hook_context();   // includes SubagentInheritance
      let started = std::time::Instant::now();
      emit_event(&self.telemetry, HOOK_PRE_STARTED, /* payload */ ..);
      let pre_agg = self.hooks.execute(pre_event, pre_ctx).await;
      let pre_dur = started.elapsed().as_millis() as u64;

      if matches!(pre_agg.decision, Some(HookDecision::Block)) {
          let reason = pre_agg.reason.unwrap_or_else(|| "blocked by hook".into());
          emit_event(&self.telemetry, HOOK_PRE_COMPLETED,
              /* decision: "block", duration_ms: pre_dur */ ..);
          return Err(ToolError::PermissionDenied(reason));
      }
      if let Some(updated) = pre_agg.modified_input {
          tool_input = updated;
      }
      emit_event(&self.telemetry, HOOK_PRE_COMPLETED, /* decision, duration_ms */ ..);

      // --- Tool call (existing M5-02 path) ---
      let mut result = self.tools.invoke(tool_name, tool_input.clone(), ..).await?;

      // --- PostToolUse ---
      let post_event = HookEvent::PostToolUse {
          tool_name: tool_name.to_string(),
          tool_input,
          tool_output: result.data.clone(),
          tool_use_id,
      };
      let post_ctx = self.build_hook_context();
      let post_started = std::time::Instant::now();
      emit_event(&self.telemetry, HOOK_POST_STARTED, ..);
      let post_agg = self.hooks.execute(post_event, post_ctx).await;
      let post_dur = post_started.elapsed().as_millis() as u64;

      let mutated = !post_agg.system_messages.is_empty();
      if mutated {
          // Append every Post hook's system_message to the result.data text
          // (or to a synthetic "additional_context" field for non-text results).
          append_post_messages(&mut result, &post_agg.system_messages);
      }
      emit_event(&self.telemetry, HOOK_POST_COMPLETED,
          /* mutated_response: mutated, duration_ms: post_dur */ ..);

      Ok(result)
  }
  ```
  `append_post_messages` (a free function in `conversation.rs`) concatenates each `system_message` to `result.data` using a newline separator. For `result.data` that is a JSON object containing a `"text"` field, it appends to that field; otherwise it wraps the existing value in `{ "data": <original>, "additional_context": <joined> }` (this is the byte-lock for our PostToolUse mutation — different from claude-code which uses `additionalContext` as a separate user-visible message, but functionally equivalent for the agent).

- **`HookContext` shape extension (T11):**
  ```rust
  pub struct HookContext {
      pub session_id: SessionId,
      pub cwd: PathBuf,
      pub transcript_path: PathBuf,                              // NEW (T11)
      pub permission_mode: Option<String>,                       // NEW (T11)
      pub agent_id: Option<AgentId>,                             // NEW (T11)
      pub agent_type: Option<String>,                            // NEW (T11)
      pub inherit: Option<SubagentInheritance>,                  // NEW (T11)
  }
  ```
  All new fields default to `None` so existing call sites continue compiling. The orchestrator's `build_hook_context()` populates them from its own state.

- **Repo conventions reaffirmed:**
  - Each new file under `lingxi-hooks/src/` includes `#![forbid(unsafe_code)]`.
  - Tests live in `#[cfg(test)] mod tests { ... }` adjacent to production code; integration tests under `crates/<crate>/tests/<name>_test.rs`.
  - Wire identifiers are byte-locked — assertions use `assert_eq!(json, r#"{...}"#)` against the canonical UTF-8 form.
  - Telemetry names follow `tengu_<category>_<verb>_<noun>`.
  - PII: `tool_name` → `PiiTagged`, `url` → `PiiTagged`, `hook_id` → `Verified` (it's an engine-assigned UUID), booleans → `Verified`.

---


## File structure

**Created:**
- `lingxi-core/crates/hooks/src/hook_payload.rs` — `PreToolUsePayload`, `PostToolUsePayload`, `HookEventEnvelope`, `parse_response`, `HookResponseParseError`. (~250 LoC inc. tests.)
- `lingxi-core/crates/hooks/src/http_executor.rs` — `HttpExecutor` struct + `execute` method. (~180 LoC inc. tests.)
- `lingxi-core/crates/hooks/src/command_executor.rs` — `CommandExecutor` struct + `execute` method. (~200 LoC inc. tests.)
- `lingxi-core/crates/hooks/src/agent_executor.rs` — `AgentExecutor` struct + `execute` method. (~180 LoC inc. tests.)
- `lingxi-core/crates/hooks/tests/http_executor_test.rs` — integration: SSRF block, timeout, happy path (mock `HttpTransport`).
- `lingxi-core/crates/hooks/tests/command_executor_test.rs` — integration: stdin payload, non-zero exit, timeout (mock `RuntimeSpawner`).
- `lingxi-core/crates/hooks/tests/agent_executor_test.rs` — integration: agent spawn happy path, `Failed`, `Killed` (mock `SubagentSpawner`).
- `lingxi-core/crates/orchestrator/tests/orchestrator_pre_post_hook_test.rs` — end-to-end with scripted Pre/PostToolUse hooks.
- `lingxi-core/crates/orchestrator/tests/orchestrator_hook_telemetry_test.rs` — captures the 8 new events in registration order.

**Modified:**
- `lingxi-core/crates/hooks/src/lib.rs` — add 4 new `pub mod` lines + corresponding `pub use` re-exports.
- `lingxi-core/crates/hooks/src/executor.rs` — fill three stub arms, add `agent_spawner` field, add `telemetry` field, add `with_agent_spawner` + `with_telemetry` builder methods.
- `lingxi-core/crates/hooks/src/registry.rs` — extend `HookContext` with the 5 new fields (T11).
- `lingxi-core/crates/hooks/src/ssrf_guard.rs` — add 169.254/16 link-local range to `with_defaults` (T5).
- `lingxi-core/crates/hooks/Cargo.toml` — add `lingxi-telemetry = { workspace = true }` (already a workspace member, but `hooks` does not yet depend on it).
- `lingxi-core/crates/orchestrator/src/conversation.rs` — add `dispatch_tool_with_hooks` method, route `run_turn`'s tool-dispatch loop through it.
- `lingxi-core/crates/orchestrator/Cargo.toml` — no change (already depends on `lingxi-hooks` and `lingxi-telemetry`).
- `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` — append 8 new constants + payload structs + extend `NAMES` slice to 15 entries.
- `lingxi-core/crates/telemetry/src/tengu/mod.rs:29` — bump TOTAL formula's orchestrator count from `7` to `15` → 253.
- `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs` — bump assertion to 253.
- `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` — insert 8 new names in registration order after `tengu_orchestrator_permission_answered`.

---

## Tasks

### Task 0: Reverse-engineer claude-code byte-locks

**Files:** none — diagnostic/reading task only.

- [ ] **Step 1: Verify `HOOK_HTTP_TIMEOUT_MS` source.**

  Run:
  ```bash
  grep -n "DEFAULT_HTTP_HOOK_TIMEOUT_MS\|TOOL_HOOK_EXECUTION_TIMEOUT_MS" \
      /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/hooks/execHttpHook.ts \
      /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/hooks.ts
  ```

  Expected output includes:
  ```
  claude-code/src/utils/hooks/execHttpHook.ts:12:const DEFAULT_HTTP_HOOK_TIMEOUT_MS = 10 * 60 * 1000
  claude-code/src/utils/hooks.ts:166:const TOOL_HOOK_EXECUTION_TIMEOUT_MS = 10 * 60 * 1000
  ```

  Record in your engineering log: `HOOK_HTTP_TIMEOUT_MS = 600_000`.

- [ ] **Step 2: Verify `HOOK_COMMAND_TIMEOUT_MS` source.**

  Run:
  ```bash
  grep -n "commandTimeoutMs\|TOOL_HOOK_EXECUTION_TIMEOUT_MS" \
      /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/hooks.ts | head -20
  ```

  Expected: `hooks.ts:2195: const commandTimeoutMs = hook.timeout ? hook.timeout * 1000 : timeoutMs` AND `hooks.ts:166: const TOOL_HOOK_EXECUTION_TIMEOUT_MS = 10 * 60 * 1000`. Caller at `hooks.ts:3401` passes `TOOL_HOOK_EXECUTION_TIMEOUT_MS` as the default. Record: `HOOK_COMMAND_TIMEOUT_MS = 600_000`.

- [ ] **Step 3: Verify `HOOK_AGENT_TIMEOUT_MS` source.**

  Run:
  ```bash
  grep -n "hookTimeoutMs" \
      /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/hooks/execAgentHook.ts
  ```

  Expected output:
  ```
  execAgentHook.ts:75:    const hookTimeoutMs = hook.timeout ? hook.timeout * 1000 : 60000
  ```

  Record: `HOOK_AGENT_TIMEOUT_MS = 60_000`.

- [ ] **Step 4: Verify `PreToolUsePayload` JSON schema.**

  Run:
  ```bash
  sed -n '414,423p' /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/entrypoints/sdk/coreSchemas.ts
  ```

  Confirm keys: `hook_event_name: 'PreToolUse'`, `tool_name: string`, `tool_input: unknown`, `tool_use_id: string`. Combined with `BaseHookInputSchema` (`session_id`, `transcript_path`, `cwd`, optional `permission_mode`/`agent_id`/`agent_type`).

- [ ] **Step 5: Verify `PostToolUsePayload` JSON schema.**

  Run:
  ```bash
  sed -n '436,446p' /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/entrypoints/sdk/coreSchemas.ts
  ```

  Confirm: same as Pre plus `tool_response: unknown`.

- [ ] **Step 6: Verify `HookResponseBody` shape.**

  Run:
  ```bash
  grep -n "hookSpecificOutput\|permissionDecision\|systemMessage\|stopReason" \
      /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/hooks.ts | head -40
  ```

  Confirm all the keys in the "HookResponseBody JSON keys" lock above appear.

- [ ] **Step 7: Verify SSRF block list.**

  Run:
  ```bash
  grep -n "169\.254\|link-local\|metadata\.\|169\\.254" \
      /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/hooks/src/ssrf_guard.rs
  ```

  Expected: zero matches (link-local was deferred to M2 per the M1.4 comment). T5 adds the range.

- [ ] **Step 8: Update this plan's "Reverse-engineered byte-locks" table** with the line numbers you just observed. If any number differs from what's recorded above, update the table inline before proceeding.

  This task produces no commit — it's a verification pass. If any value disagrees, STOP and ask the planner before continuing.

---

### Task 1: `hook_payload.rs` with serde-locked schemas

**Files:**
- Create: `lingxi-core/crates/hooks/src/hook_payload.rs`
- Modify: `lingxi-core/crates/hooks/src/lib.rs` (add `pub mod hook_payload;` + `pub use hook_payload::{...};`)

- [ ] **Step 1: Create the file with the full module body.**

  Write `lingxi-core/crates/hooks/src/hook_payload.rs`:
  ```rust
  //! Hook event payload (over-the-wire JSON) and response parser.
  //!
  //! Byte-locked against `claude-code/src/entrypoints/sdk/coreSchemas.ts:414-446`
  //! (PreToolUseHookInputSchema / PostToolUseHookInputSchema) and
  //! `claude-code/src/utils/hooks.ts:540-680` (response-processing block).

  #![forbid(unsafe_code)]

  use serde::de::{Deserializer, Error as DeError};
  use serde::ser::Serializer;
  use serde::{Deserialize, Serialize};
  use serde_json::Value;
  use thiserror::Error;

  use crate::response::{HookDecision, HookResponse};

  /// Marker unit struct that serializes/deserializes as the literal `"PreToolUse"`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct HookEventNamePre;

  impl Serialize for HookEventNamePre {
      fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
          s.serialize_str("PreToolUse")
      }
  }
  impl<'de> Deserialize<'de> for HookEventNamePre {
      fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
          let s = String::deserialize(d)?;
          if s == "PreToolUse" {
              Ok(Self)
          } else {
              Err(D::Error::custom(format!(
                  "expected 'PreToolUse', got {s:?}"
              )))
          }
      }
  }

  /// Marker unit struct that serializes/deserializes as the literal `"PostToolUse"`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct HookEventNamePost;

  impl Serialize for HookEventNamePost {
      fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
          s.serialize_str("PostToolUse")
      }
  }
  impl<'de> Deserialize<'de> for HookEventNamePost {
      fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
          let s = String::deserialize(d)?;
          if s == "PostToolUse" {
              Ok(Self)
          } else {
              Err(D::Error::custom(format!(
                  "expected 'PostToolUse', got {s:?}"
              )))
          }
      }
  }

  /// Wire-format `PreToolUse` payload (1:1 with `coreSchemas.ts:414-423`).
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct PreToolUsePayload {
      pub hook_event_name: HookEventNamePre,
      pub session_id: String,
      pub transcript_path: String,
      pub cwd: String,
      #[serde(skip_serializing_if = "Option::is_none", default)]
      pub permission_mode: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none", default)]
      pub agent_id: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none", default)]
      pub agent_type: Option<String>,
      pub tool_name: String,
      pub tool_input: Value,
      pub tool_use_id: String,
  }

  /// Wire-format `PostToolUse` payload (1:1 with `coreSchemas.ts:436-446`).
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct PostToolUsePayload {
      pub hook_event_name: HookEventNamePost,
      pub session_id: String,
      pub transcript_path: String,
      pub cwd: String,
      #[serde(skip_serializing_if = "Option::is_none", default)]
      pub permission_mode: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none", default)]
      pub agent_id: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none", default)]
      pub agent_type: Option<String>,
      pub tool_name: String,
      pub tool_input: Value,
      pub tool_response: Value,
      pub tool_use_id: String,
  }

  /// Envelope used to send one of either payload kind across the wire.
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(untagged)]
  pub enum HookEventEnvelope {
      Pre(PreToolUsePayload),
      Post(PostToolUsePayload),
  }

  /// Failure modes from [`parse_response`].
  #[derive(Debug, Clone, Error)]
  pub enum HookResponseParseError {
      #[error("hook response is not valid JSON: {0}")]
      Json(String),
      #[error("hook response is not a JSON object")]
      NotObject,
      #[error("hook response hookEventName mismatch: expected '{expected}', got '{got}'")]
      EventNameMismatch {
          expected: &'static str,
          got: String,
      },
  }

  /// Parse a hook's JSON reply into a [`HookResponse`].
  ///
  /// `expected_event` is `"PreToolUse"` or `"PostToolUse"`; used to validate the
  /// nested `hookSpecificOutput.hookEventName` field per `hooks.ts:585`.
  pub fn parse_response(
      raw: &str,
      expected_event: &'static str,
  ) -> Result<HookResponse, HookResponseParseError> {
      let v: Value = serde_json::from_str(raw)
          .map_err(|e| HookResponseParseError::Json(e.to_string()))?;
      let obj = v.as_object().ok_or(HookResponseParseError::NotObject)?;
      let mut resp = HookResponse::default();

      // continue / stopReason
      let cont = obj.get("continue").and_then(Value::as_bool).unwrap_or(true);
      if !cont {
          if let Some(reason) = obj.get("stopReason").and_then(Value::as_str) {
              resp.reason = Some(reason.to_string());
          }
          // continue=false without permissionDecision implies an advisory stop,
          // not a hard block — leave decision = None.
      }

      // suppressOutput
      if let Some(b) = obj.get("suppressOutput").and_then(Value::as_bool) {
          resp.suppress_output = b;
      }

      // systemMessage
      if let Some(s) = obj.get("systemMessage").and_then(Value::as_str) {
          resp.system_message = Some(s.to_string());
      }

      // legacy decision
      match obj.get("decision").and_then(Value::as_str) {
          Some("block") => resp.decision = Some(HookDecision::Block),
          Some("approve") => resp.decision = Some(HookDecision::Approve),
          _ => {}
      }

      // permissionDecision (preferred over legacy)
      match obj.get("permissionDecision").and_then(Value::as_str) {
          Some("allow") => {
              if resp.decision.is_none() {
                  resp.decision = Some(HookDecision::Approve);
              }
          }
          Some("deny") => resp.decision = Some(HookDecision::Block),
          Some("ask") => { /* leave alone */ }
          _ => {}
      }
      if let Some(r) = obj.get("permissionDecisionReason").and_then(Value::as_str) {
          resp.reason = Some(r.to_string());
      }

      // hookSpecificOutput
      if let Some(hs) = obj.get("hookSpecificOutput").and_then(Value::as_object) {
          if let Some(name) = hs.get("hookEventName").and_then(Value::as_str) {
              if name != expected_event {
                  return Err(HookResponseParseError::EventNameMismatch {
                      expected: expected_event,
                      got: name.to_string(),
                  });
              }
          }
          if let Some(upd) = hs.get("updatedInput") {
              resp.updated_input = Some(upd.clone());
          }
          if let Some(addl) = hs.get("additionalContext").and_then(Value::as_str) {
              let combined = match resp.system_message.take() {
                  Some(prev) => format!("{prev}\n{addl}"),
                  None => addl.to_string(),
              };
              resp.system_message = Some(combined);
          }
          // Nested permissionDecision inside hookSpecificOutput (PreToolUse only)
          match hs.get("permissionDecision").and_then(Value::as_str) {
              Some("allow") => {
                  if !matches!(resp.decision, Some(HookDecision::Block)) {
                      resp.decision = Some(HookDecision::Approve);
                  }
              }
              Some("deny") => resp.decision = Some(HookDecision::Block),
              _ => {}
          }
          if let Some(r) = hs.get("permissionDecisionReason").and_then(Value::as_str) {
              resp.reason = Some(r.to_string());
          }
      }

      Ok(resp)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use serde_json::json;

      #[test]
      fn pre_payload_serializes_byte_lock() {
          let p = PreToolUsePayload {
              hook_event_name: HookEventNamePre,
              session_id: "sess-1".into(),
              transcript_path: "/tmp/t.jsonl".into(),
              cwd: "/work".into(),
              permission_mode: None,
              agent_id: None,
              agent_type: None,
              tool_name: "Bash".into(),
              tool_input: json!({"command": "ls"}),
              tool_use_id: "tu-1".into(),
          };
          let s = serde_json::to_string(&p).unwrap();
          // Order: hook_event_name, session_id, transcript_path, cwd,
          //        tool_name, tool_input, tool_use_id (None fields skipped)
          assert!(s.starts_with(r#"{"hook_event_name":"PreToolUse","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","tool_name":"Bash","tool_input":{"command":"ls"},"tool_use_id":"tu-1"}"#));
      }

      #[test]
      fn post_payload_includes_tool_response() {
          let p = PostToolUsePayload {
              hook_event_name: HookEventNamePost,
              session_id: "s".into(),
              transcript_path: "/t".into(),
              cwd: "/w".into(),
              permission_mode: None,
              agent_id: None,
              agent_type: None,
              tool_name: "Read".into(),
              tool_input: json!({"path": "/x"}),
              tool_response: json!({"content": "data"}),
              tool_use_id: "tu-2".into(),
          };
          let s = serde_json::to_string(&p).unwrap();
          assert!(s.contains(r#""hook_event_name":"PostToolUse""#));
          assert!(s.contains(r#""tool_response":{"content":"data"}"#));
      }

      #[test]
      fn round_trip_pre_payload() {
          let p = PreToolUsePayload {
              hook_event_name: HookEventNamePre,
              session_id: "s".into(),
              transcript_path: "/t".into(),
              cwd: "/w".into(),
              permission_mode: Some("plan".into()),
              agent_id: Some("a-1".into()),
              agent_type: Some("general-purpose".into()),
              tool_name: "Edit".into(),
              tool_input: json!({"file_path": "/f"}),
              tool_use_id: "tu".into(),
          };
          let s = serde_json::to_string(&p).unwrap();
          let back: PreToolUsePayload = serde_json::from_str(&s).unwrap();
          assert_eq!(back.tool_name, "Edit");
          assert_eq!(back.agent_type.as_deref(), Some("general-purpose"));
      }

      #[test]
      fn parse_response_allow_via_permission_decision() {
          let r = parse_response(
              r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#,
              "PreToolUse",
          ).unwrap();
          assert_eq!(r.decision, Some(HookDecision::Approve));
      }

      #[test]
      fn parse_response_block_via_legacy_decision() {
          let r = parse_response(
              r#"{"decision":"block","stopReason":"because","continue":false}"#,
              "PreToolUse",
          ).unwrap();
          assert_eq!(r.decision, Some(HookDecision::Block));
          assert_eq!(r.reason.as_deref(), Some("because"));
      }

      #[test]
      fn parse_response_event_mismatch_errors() {
          let err = parse_response(
              r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse"}}"#,
              "PreToolUse",
          ).unwrap_err();
          assert!(matches!(err, HookResponseParseError::EventNameMismatch { .. }));
      }

      #[test]
      fn parse_response_additional_context_appends_to_system_message() {
          let r = parse_response(
              r#"{"systemMessage":"hello","hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"world"}}"#,
              "PreToolUse",
          ).unwrap();
          assert_eq!(r.system_message.as_deref(), Some("hello\nworld"));
      }
  }
  ```

- [ ] **Step 2: Wire the module in `lib.rs`.**

  In `lingxi-core/crates/hooks/src/lib.rs`, find the existing `pub mod` block (currently `async_registry, builtin, definition, events, executor, registry, response, ssrf_guard`) and add `pub mod hook_payload;` AFTER `events`. Then append to the `pub use` block:
  ```rust
  pub use hook_payload::{
      HookEventEnvelope, HookEventNamePost, HookEventNamePre, HookResponseParseError,
      PostToolUsePayload, PreToolUsePayload, parse_response,
  };
  ```

- [ ] **Step 3: Run the tests.**

  ```bash
  cargo test -p lingxi-hooks hook_payload::tests --quiet
  ```

  Expected: 6 tests pass. If any fail, fix the implementation (NOT the test — the test values are the byte-locks).

- [ ] **Step 4: Run clippy + fmt.**

  ```bash
  cargo clippy -p lingxi-hooks -- -D warnings
  cargo fmt -p lingxi-hooks --check
  ```

  Expected: clean.

- [ ] **Step 5: Commit.**

  ```bash
  cd /Users/luolingfeng/Projects/LingXi-Next
  git add lingxi-core/crates/hooks/src/hook_payload.rs lingxi-core/crates/hooks/src/lib.rs
  git commit -m "feat(M5-06 T1): hook_payload module — PreToolUse/PostToolUse serde-locked schemas + parse_response"
  ```

---

### Task 2: Timeout constants

**Files:**
- Modify: `lingxi-core/crates/hooks/src/executor.rs` (add three `pub const` declarations at the top).

- [ ] **Step 1: Add the constants.**

  Open `lingxi-core/crates/hooks/src/executor.rs`. Immediately after the file-level doc comment and before the `use` block, insert:
  ```rust
  /// Default HTTP hook timeout (10 minutes — matches
  /// `claude-code/src/utils/hooks/execHttpHook.ts:12` DEFAULT_HTTP_HOOK_TIMEOUT_MS).
  pub const HOOK_HTTP_TIMEOUT_MS: u64 = 600_000;

  /// Default command hook timeout (10 minutes — matches
  /// `claude-code/src/utils/hooks.ts:166` TOOL_HOOK_EXECUTION_TIMEOUT_MS).
  pub const HOOK_COMMAND_TIMEOUT_MS: u64 = 600_000;

  /// Default agent hook timeout (60 seconds — matches
  /// `claude-code/src/utils/hooks/execAgentHook.ts:75` fall-through default).
  pub const HOOK_AGENT_TIMEOUT_MS: u64 = 60_000;
  ```

- [ ] **Step 2: Add tests to the existing `#[cfg(test)] mod tests` block in `executor.rs`.**

  If no test block exists, add one at the bottom:
  ```rust
  #[cfg(test)]
  mod constants_tests {
      use super::*;

      #[test]
      fn http_timeout_is_10_minutes() {
          assert_eq!(HOOK_HTTP_TIMEOUT_MS, 600_000);
      }
      #[test]
      fn command_timeout_is_10_minutes() {
          assert_eq!(HOOK_COMMAND_TIMEOUT_MS, 600_000);
      }
      #[test]
      fn agent_timeout_is_60_seconds() {
          assert_eq!(HOOK_AGENT_TIMEOUT_MS, 60_000);
      }
  }
  ```

- [ ] **Step 3: Re-export from `lib.rs`.**

  Append to the existing `pub use executor::` line so it reads:
  ```rust
  pub use executor::{
      BuiltinHookHandler, HookExecutorImpl, HOOK_AGENT_TIMEOUT_MS,
      HOOK_COMMAND_TIMEOUT_MS, HOOK_HTTP_TIMEOUT_MS,
  };
  ```

- [ ] **Step 4: Run tests + clippy + fmt.**

  ```bash
  cargo test -p lingxi-hooks constants_tests --quiet
  cargo clippy -p lingxi-hooks -- -D warnings
  cargo fmt -p lingxi-hooks --check
  ```

- [ ] **Step 5: Commit.**

  ```bash
  git add lingxi-core/crates/hooks/src/executor.rs lingxi-core/crates/hooks/src/lib.rs
  git commit -m "feat(M5-06 T2): byte-lock hook timeout constants (600s http/command, 60s agent)"
  ```

---

### Task 3: HttpExecutor failing test (RED)

**Files:**
- Create: `lingxi-core/crates/hooks/src/http_executor.rs` — empty stub.
- Create: `lingxi-core/crates/hooks/tests/http_executor_test.rs`.
- Modify: `lingxi-core/crates/hooks/src/lib.rs` (`pub mod http_executor;` — keep private from public API).

- [ ] **Step 1: Create the empty stub.**

  Write `lingxi-core/crates/hooks/src/http_executor.rs`:
  ```rust
  //! HTTP hook executor — POSTs the event JSON, parses the body.
  //!
  //! Lands in M5-06 T4 (impl). T3 only sets up the failing test.

  #![forbid(unsafe_code)]

  use crate::definition::HookDefinition;
  use crate::response::HookResult;
  use crate::ssrf_guard::SsrfGuard;
  use lingxi_traits::HttpTransport;
  use std::collections::HashMap;
  use std::sync::Arc;
  use std::time::Duration;

  pub(crate) struct HttpExecutor {
      pub(crate) http: Arc<dyn HttpTransport>,
      pub(crate) ssrf_guard: SsrfGuard,
      pub(crate) timeout: Duration,
  }

  impl HttpExecutor {
      /// Execute one HTTP hook. Returns the raw [`HookResult`] for the caller
      /// to fold into an [`crate::AggregateHookResult`].
      ///
      /// Filled in T4. T3 leaves the method body empty so the integration test
      /// compiles but fails at runtime.
      #[allow(clippy::too_many_arguments)]
      pub(crate) async fn execute(
          &self,
          _hook: &HookDefinition,
          _url: &str,
          _headers: &HashMap<String, String>,
          _body: &str,
          _expected_event: &'static str,
      ) -> HookResult {
          HookResult {
              outcome: crate::response::HookOutcome::Error,
              stdout: String::new(),
              stderr: "http executor not yet implemented (M5-06 T3 stub)".into(),
              exit_code: None,
              response: None,
          }
      }
  }
  ```

- [ ] **Step 2: Wire the module privately in `lib.rs`.**

  Add `mod http_executor;` (NOT `pub mod` — it stays crate-private) immediately after `pub mod hook_payload;`.

- [ ] **Step 3: Create the failing integration test.**

  Write `lingxi-core/crates/hooks/tests/http_executor_test.rs`:
  ```rust
  //! Integration tests for the HTTP hook executor.
  //!
  //! Uses an in-process mock [`HttpTransport`] that records requests and
  //! returns scripted responses.

  use lingxi_hooks::{HOOK_HTTP_TIMEOUT_MS, HookOutcome, HookDecision};
  // The HttpExecutor type is crate-private; tests reach it via
  // a re-export under `#[cfg(test)] pub mod test_support;` added in T4.
  // For the RED test we only assert the integration shape via the
  // public HookExecutorImpl::execute path with a registered Http hook.

  use lingxi_hooks::events::{HookEvent, HookEventType};
  use lingxi_hooks::registry::{HookContext, HookRegistry};
  use lingxi_hooks::definition::{HookDefinition, HookExecutor, HookSource};
  use lingxi_hooks::executor::HookExecutorImpl;
  use lingxi_protocol::{HookId, SessionId, ToolUseId};
  use lingxi_traits::{HttpRequest, HttpResponse, HttpTransport};
  use serde_json::json;
  use std::collections::HashMap;
  use std::sync::{Arc, Mutex};
  use tokio::sync::RwLock;

  /// Mock HTTP transport that returns a single scripted 200 OK with the
  /// supplied JSON body. Records every request URL.
  struct MockHttp {
      recorded: Mutex<Vec<String>>,
      body: String,
  }
  #[async_trait::async_trait]
  impl HttpTransport for MockHttp {
      async fn request(&self, req: HttpRequest) -> Result<HttpResponse, lingxi_traits::HttpError> {
          self.recorded.lock().unwrap().push(req.url.clone());
          Ok(HttpResponse {
              status: 200,
              headers: HashMap::new(),
              body: self.body.clone().into_bytes(),
          })
      }
  }

  /// Minimal RuntimeSpawner mock — not exercised by these tests, but
  /// required to construct HookExecutorImpl.
  struct InertRuntime;
  #[async_trait::async_trait]
  impl lingxi_traits::RuntimeSpawner for InertRuntime {
      async fn spawn_with_stdin(
          &self,
          _command: &str,
          _args: &[String],
          _env: &HashMap<String, String>,
          _cwd: Option<&std::path::Path>,
          _stdin: &str,
          _timeout: std::time::Duration,
      ) -> Result<lingxi_traits::SpawnedProcessOutput, lingxi_traits::SpawnError> {
          Err(lingxi_traits::SpawnError::Internal("inert".into()))
      }
  }

  fn make_http_hook(url: &str) -> HookDefinition {
      HookDefinition {
          id: HookId::new(),
          name: "test-http".into(),
          events: vec![HookEventType::PreToolUse],
          if_condition: None,
          executor: HookExecutor::Http {
              url: url.into(),
              method: "POST".into(),
              headers: HashMap::new(),
              timeout: std::time::Duration::from_millis(HOOK_HTTP_TIMEOUT_MS),
          },
          source: HookSource::User,
          blocking: true,
          timeout: None,
          priority: 0,
      }
  }

  #[tokio::test]
  async fn http_arm_returns_allow_when_endpoint_responds_with_allow() {
      let http = Arc::new(MockHttp {
          recorded: Mutex::new(Vec::new()),
          body: r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#.into(),
      });
      let runtime = Arc::new(InertRuntime);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_http_hook("https://hook.example.com/pre"));
      let exec = HookExecutorImpl::new(registry, http.clone(), runtime);

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({"command": "ls"}),
          tool_use_id: ToolUseId::from("tu-1"),
      };
      let ctx = HookContext {
          session_id: SessionId::from("sess"),
          cwd: std::path::PathBuf::from("/work"),
          transcript_path: std::path::PathBuf::from("/tmp/t.jsonl"),
          permission_mode: None,
          agent_id: None,
          agent_type: None,
          inherit: None,
      };
      let agg = exec.execute(event, ctx).await;

      assert_eq!(agg.decision, Some(HookDecision::Approve));
      assert_eq!(http.recorded.lock().unwrap().len(), 1);
      assert_eq!(http.recorded.lock().unwrap()[0], "https://hook.example.com/pre");
      // None outcome in stub returns Error — verify HookResult shape:
      let first_result = &agg.all_results[0].1;
      assert!(matches!(first_result.outcome, HookOutcome::Success));
  }
  ```

  **Note:** this test uses `HookContext` with five new fields (`transcript_path`, `permission_mode`, `agent_id`, `agent_type`, `inherit`). These do not yet exist on `HookContext` (T11 adds them). T3 is the RED phase — we expect the test to fail to **compile**, not just fail at runtime. That's an acceptable RED for TDD because compile failure is a strict superset of test failure. T11 will make this compile, and T4 will make it pass.

- [ ] **Step 4: Run the test and observe failure.**

  ```bash
  cargo test -p lingxi-hooks --test http_executor_test --quiet 2>&1 | head -40
  ```

  Expected: compile error mentioning unknown fields on `HookContext` OR the runtime assertion failing on `agg.decision`. Either is acceptable RED.

- [ ] **Step 5: Commit (RED).**

  ```bash
  git add lingxi-core/crates/hooks/src/http_executor.rs \
          lingxi-core/crates/hooks/src/lib.rs \
          lingxi-core/crates/hooks/tests/http_executor_test.rs
  git commit -m "test(M5-06 T3): RED — HttpExecutor integration test against mock 200 OK"
  ```

---

### Task 4: HttpExecutor implementation (GREEN — happy path)

**Files:**
- Modify: `lingxi-core/crates/hooks/src/http_executor.rs` (fill `execute` body).
- Modify: `lingxi-core/crates/hooks/src/registry.rs` (add five new `HookContext` fields — actually completed in T11, but T4 needs the minimum for compile: add a `pub transcript_path: PathBuf` field with `Default` impl returning empty path, plus `pub inherit: Option<lingxi_traits::SubagentInheritance>` — the rest stay as `None`-typed `Option<String>`/`Option<AgentId>`.)
- Modify: `lingxi-core/crates/hooks/src/executor.rs` (replace the stubbed `HookExecutor::Http` arm with a delegating call to `HttpExecutor`).

- [ ] **Step 1: Extend `HookContext` with the five new fields (forward-compatible).**

  Open `lingxi-core/crates/hooks/src/registry.rs`. Find the current `HookContext` struct (M1.4 shape: probably `pub struct HookContext { pub session_id: SessionId, pub cwd: PathBuf }`). Add the five new fields and update the `Default` impl. The exact struct now reads:
  ```rust
  use lingxi_protocol::{AgentId, SessionId};
  use lingxi_traits::SubagentInheritance;
  use std::path::PathBuf;

  /// Per-event context passed alongside the hook event to every matched hook.
  #[derive(Clone, Default)]
  pub struct HookContext {
      pub session_id: SessionId,
      pub cwd: PathBuf,
      pub transcript_path: PathBuf,
      pub permission_mode: Option<String>,
      pub agent_id: Option<AgentId>,
      pub agent_type: Option<String>,
      pub inherit: Option<SubagentInheritance>,
  }
  ```
  If `HookContext` previously did NOT derive `Default`, add it now and audit existing call sites (`grep -rn "HookContext {" lingxi-core/crates/`) — every literal struct construction must explicitly include the new fields OR use `..Default::default()`.

  **`Cargo.toml` reconcile:** ensure `lingxi-hooks/Cargo.toml` has `lingxi-traits = { workspace = true }` (it should — confirm with `grep lingxi-traits lingxi-core/crates/hooks/Cargo.toml`).

- [ ] **Step 2: Fill the `HttpExecutor::execute` body.**

  Replace the file body of `http_executor.rs` with the production impl:
  ```rust
  //! HTTP hook executor — POSTs the event JSON, parses the body.

  #![forbid(unsafe_code)]

  use crate::definition::HookDefinition;
  use crate::hook_payload::parse_response;
  use crate::response::{HookOutcome, HookResponse, HookResult};
  use crate::ssrf_guard::SsrfGuard;
  use lingxi_traits::{HttpRequest, HttpTransport};
  use std::collections::HashMap;
  use std::sync::Arc;
  use std::time::Duration;

  pub(crate) struct HttpExecutor {
      pub(crate) http: Arc<dyn HttpTransport>,
      pub(crate) ssrf_guard: SsrfGuard,
      pub(crate) timeout: Duration,
  }

  /// Telemetry hint emitted by the executor's caller (executor.rs).
  pub(crate) enum HttpExecutionSignal {
      Ok,
      SsrfBlocked(String),
      TimedOut,
  }

  pub(crate) struct HttpExecutionOutcome {
      pub(crate) result: HookResult,
      pub(crate) signal: HttpExecutionSignal,
  }

  impl HttpExecutor {
      pub(crate) async fn execute(
          &self,
          hook: &HookDefinition,
          url: &str,
          headers: &HashMap<String, String>,
          body: &str,
          expected_event: &'static str,
      ) -> HttpExecutionOutcome {
          // SSRF check.
          if let Err(e) = self.ssrf_guard.check_url(url) {
              return HttpExecutionOutcome {
                  result: HookResult {
                      outcome: HookOutcome::Error,
                      stdout: String::new(),
                      stderr: format!("Hook {} failed: SSRF guard rejected url: {}", hook.id, e),
                      exit_code: None,
                      response: None,
                  },
                  signal: HttpExecutionSignal::SsrfBlocked(e.to_string()),
              };
          }

          // Per-hook timeout override.
          let effective_timeout = match &hook.executor {
              crate::definition::HookExecutor::Http { timeout, .. } if !timeout.is_zero() => *timeout,
              _ => self.timeout,
          };

          // Build request.
          let mut req_headers = headers.clone();
          req_headers
              .entry("Content-Type".into())
              .or_insert_with(|| "application/json".into());
          let req = HttpRequest {
              method: "POST".into(),
              url: url.to_string(),
              headers: req_headers,
              body: body.as_bytes().to_vec(),
          };

          // Issue with timeout.
          let send = self.http.request(req);
          let raw = match tokio::time::timeout(effective_timeout, send).await {
              Err(_) => {
                  return HttpExecutionOutcome {
                      result: HookResult {
                          outcome: HookOutcome::Timeout,
                          stdout: String::new(),
                          stderr: format!(
                              "Hook {} failed: timeout after {}ms",
                              hook.id,
                              effective_timeout.as_millis()
                          ),
                          exit_code: None,
                          response: None,
                      },
                      signal: HttpExecutionSignal::TimedOut,
                  };
              }
              Ok(Err(e)) => {
                  return HttpExecutionOutcome {
                      result: HookResult {
                          outcome: HookOutcome::Error,
                          stdout: String::new(),
                          stderr: format!("Hook {} failed: http error: {e}", hook.id),
                          exit_code: None,
                          response: None,
                      },
                      signal: HttpExecutionSignal::Ok,
                  };
              }
              Ok(Ok(r)) => r,
          };

          let body_str = String::from_utf8_lossy(&raw.body).into_owned();
          let success = (200..300).contains(&raw.status);

          let parsed: Option<HookResponse> = if body_str.is_empty() {
              None
          } else {
              match parse_response(&body_str, expected_event) {
                  Ok(r) => Some(r),
                  Err(e) => {
                      return HttpExecutionOutcome {
                          result: HookResult {
                              outcome: HookOutcome::Error,
                              stdout: body_str.clone(),
                              stderr: format!("Hook {} failed: parse: {e}", hook.id),
                              exit_code: Some(raw.status as i32),
                              response: None,
                          },
                          signal: HttpExecutionSignal::Ok,
                      };
                  }
              }
          };

          HttpExecutionOutcome {
              result: HookResult {
                  outcome: if success { HookOutcome::Success } else { HookOutcome::Error },
                  stdout: body_str,
                  stderr: String::new(),
                  exit_code: Some(raw.status as i32),
                  response: parsed,
              },
              signal: HttpExecutionSignal::Ok,
          }
      }
  }
  ```

- [ ] **Step 3: Wire the `HookExecutor::Http` arm in `executor.rs`.**

  In `lingxi-core/crates/hooks/src/executor.rs`, replace the `HookExecutor::Http { url, .. } => { … }` match arm with:
  ```rust
  HookExecutor::Http { url, headers, .. } => {
      let envelope_json = match self.serialize_event(event, ctx) {
          Ok(s) => s,
          Err(e) => {
              return HookResult {
                  outcome: HookOutcome::Error,
                  stdout: String::new(),
                  stderr: format!("Hook {} failed: serialize: {e}", hook.id),
                  exit_code: None,
                  response: None,
              };
          }
      };
      let expected = match event {
          HookEvent::PreToolUse { .. } => "PreToolUse",
          HookEvent::PostToolUse { .. } => "PostToolUse",
          _ => "Other",
      };
      let exec = crate::http_executor::HttpExecutor {
          http: self.http.clone(),
          ssrf_guard: self.ssrf_guard.clone(),
          timeout: std::time::Duration::from_millis(HOOK_HTTP_TIMEOUT_MS),
      };
      let outcome = exec.execute(hook, url, headers, &envelope_json, expected).await;
      if let crate::http_executor::HttpExecutionSignal::SsrfBlocked(reason) = &outcome.signal {
          self.emit_ssrf_skip(hook, url, reason).await;
      } else if matches!(outcome.signal, crate::http_executor::HttpExecutionSignal::TimedOut) {
          self.emit_timeout(hook, "http").await;
      }
      outcome.result
  }
  ```

- [ ] **Step 4: Add `serialize_event` + `emit_ssrf_skip` + `emit_timeout` helpers to `executor.rs`.**

  Inside `impl HookExecutorImpl`, append:
  ```rust
  fn serialize_event(
      &self,
      event: &HookEvent,
      ctx: &HookContext,
  ) -> Result<String, serde_json::Error> {
      use crate::hook_payload::*;
      match event {
          HookEvent::PreToolUse { tool_name, tool_input, tool_use_id } => {
              let p = PreToolUsePayload {
                  hook_event_name: HookEventNamePre,
                  session_id: ctx.session_id.to_string(),
                  transcript_path: ctx.transcript_path.display().to_string(),
                  cwd: ctx.cwd.display().to_string(),
                  permission_mode: ctx.permission_mode.clone(),
                  agent_id: ctx.agent_id.as_ref().map(|a| a.to_string()),
                  agent_type: ctx.agent_type.clone(),
                  tool_name: tool_name.clone(),
                  tool_input: tool_input.clone(),
                  tool_use_id: tool_use_id.to_string(),
              };
              serde_json::to_string(&p)
          }
          HookEvent::PostToolUse { tool_name, tool_input, tool_output, tool_use_id } => {
              let p = PostToolUsePayload {
                  hook_event_name: HookEventNamePost,
                  session_id: ctx.session_id.to_string(),
                  transcript_path: ctx.transcript_path.display().to_string(),
                  cwd: ctx.cwd.display().to_string(),
                  permission_mode: ctx.permission_mode.clone(),
                  agent_id: ctx.agent_id.as_ref().map(|a| a.to_string()),
                  agent_type: ctx.agent_type.clone(),
                  tool_name: tool_name.clone(),
                  tool_input: tool_input.clone(),
                  tool_response: tool_output.clone(),
                  tool_use_id: tool_use_id.to_string(),
              };
              serde_json::to_string(&p)
          }
          _ => serde_json::to_string(&serde_json::json!({"event": format!("{:?}", event.event_type())})),
      }
  }

  async fn emit_ssrf_skip(&self, hook: &HookDefinition, url: &str, reason: &str) {
      // Emission is wired in T16 — for now we just log via tracing.
      tracing::warn!(hook_id = ?hook.id, url, reason, "hook_http_skipped_ssrf");
  }

  async fn emit_timeout(&self, hook: &HookDefinition, kind: &'static str) {
      tracing::warn!(hook_id = ?hook.id, kind, "hook_timeout");
  }
  ```

  `SsrfGuard` must now be `Clone`. Open `ssrf_guard.rs` and add `#[derive(Clone)]` to the struct definition (the inner `HashSet`/`Vec`/`Option<HashSet>` already implement `Clone`).

- [ ] **Step 5: Run the T3 test and verify it passes.**

  ```bash
  cargo test -p lingxi-hooks --test http_executor_test --quiet
  ```

  Expected: 1 passed.

- [ ] **Step 6: Run clippy + fmt.**

  ```bash
  cargo clippy -p lingxi-hooks -- -D warnings
  cargo fmt -p lingxi-hooks --check
  ```

- [ ] **Step 7: Commit (GREEN).**

  ```bash
  git add lingxi-core/crates/hooks/src/http_executor.rs \
          lingxi-core/crates/hooks/src/executor.rs \
          lingxi-core/crates/hooks/src/registry.rs \
          lingxi-core/crates/hooks/src/ssrf_guard.rs
  git commit -m "feat(M5-06 T4): GREEN — HttpExecutor POSTs envelope + parses response + SSRF/timeout signals"
  ```

---

### Task 5: SSRF block test + telemetry signal

**Files:**
- Modify: `lingxi-core/crates/hooks/src/ssrf_guard.rs` (add 169.254.0.0–169.254.255.255 range to `with_defaults`).
- Modify: `lingxi-core/crates/hooks/tests/http_executor_test.rs` (add a `ssrf_blocks_link_local` test).

- [ ] **Step 1: Add the link-local range.**

  Open `lingxi-core/crates/hooks/src/ssrf_guard.rs`. Find the `blocked` vec inside `with_defaults`. Append:
  ```rust
  IpRange {
      start: "169.254.0.0".parse().unwrap(),
      end: "169.254.255.255".parse().unwrap(),
  },
  ```

  And update the comment to reflect that link-local is now included (delete the "IPv6 unique-local … added in M2" sentence; replace with: "IPv6 unique-local + link-local lands with the platform DNS resolver in M2; IPv4 link-local 169.254/16 is included here for the M5-06 cloud-metadata threat (169.254.169.254 AWS/Azure/GCP metadata service).").

- [ ] **Step 2: Add the SSRF test.**

  Append to `lingxi-core/crates/hooks/tests/http_executor_test.rs`:
  ```rust
  #[tokio::test]
  async fn ssrf_blocks_link_local_metadata_endpoint() {
      let http = Arc::new(MockHttp {
          recorded: Mutex::new(Vec::new()),
          body: "should-never-be-called".into(),
      });
      let runtime = Arc::new(InertRuntime);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_http_hook("http://169.254.169.254/latest/meta-data/"));
      let exec = HookExecutorImpl::new(registry, http.clone(), runtime);

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let ctx = HookContext::default();
      let agg = exec.execute(event, ctx).await;

      assert_eq!(http.recorded.lock().unwrap().len(), 0, "HTTP request must NOT be issued");
      let result = &agg.all_results[0].1;
      assert!(matches!(result.outcome, HookOutcome::Error));
      assert!(result.stderr.contains("SSRF"));
  }
  ```

- [ ] **Step 3: Add an in-crate SSRF unit test for the new range.**

  Append to the existing `#[cfg(test)] mod tests` in `ssrf_guard.rs`:
  ```rust
  #[test]
  fn blocks_link_local_ip() {
      let g = SsrfGuard::with_defaults();
      assert!(g.check_url("http://169.254.169.254/").is_err());
  }
  ```

- [ ] **Step 4: Run tests.**

  ```bash
  cargo test -p lingxi-hooks --quiet
  ```

  Expected: all hooks tests pass, including the new SSRF tests.

- [ ] **Step 5: Commit.**

  ```bash
  git add lingxi-core/crates/hooks/src/ssrf_guard.rs \
          lingxi-core/crates/hooks/tests/http_executor_test.rs
  git commit -m "feat(M5-06 T5): SSRF guard blocks 169.254/16 (cloud metadata) + integration test"
  ```

---

### Task 6: HTTP timeout test + telemetry signal

**Files:**
- Modify: `lingxi-core/crates/hooks/tests/http_executor_test.rs` (add a `never_resolving_endpoint_times_out` test).

- [ ] **Step 1: Add the timeout test.**

  Append to `lingxi-core/crates/hooks/tests/http_executor_test.rs`:
  ```rust
  /// HTTP transport that never resolves — used to trigger the executor timeout.
  struct PendingForeverHttp;
  #[async_trait::async_trait]
  impl HttpTransport for PendingForeverHttp {
      async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, lingxi_traits::HttpError> {
          std::future::pending::<()>().await;
          unreachable!()
      }
  }

  fn make_short_timeout_http_hook(url: &str) -> HookDefinition {
      let mut hook = make_http_hook(url);
      if let HookExecutor::Http { ref mut timeout, .. } = hook.executor {
          *timeout = std::time::Duration::from_millis(50);
      }
      hook
  }

  #[tokio::test(start_paused = true)]
  async fn http_executor_times_out_when_endpoint_never_responds() {
      let http = Arc::new(PendingForeverHttp);
      let runtime = Arc::new(InertRuntime);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_short_timeout_http_hook("https://stalled.example.com/"));
      let exec = HookExecutorImpl::new(registry, http, runtime);

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let ctx = HookContext::default();

      let task = tokio::spawn(async move { exec.execute(event, ctx).await });
      // Advance virtual time past the 50ms timeout.
      tokio::time::advance(std::time::Duration::from_millis(60)).await;
      let agg = task.await.unwrap();
      let result = &agg.all_results[0].1;
      assert!(matches!(result.outcome, HookOutcome::Timeout));
      assert!(result.stderr.contains("timeout"));
  }
  ```

- [ ] **Step 2: Run tests.**

  ```bash
  cargo test -p lingxi-hooks --test http_executor_test --quiet
  ```

  Expected: all 3 HTTP tests pass.

- [ ] **Step 3: Commit.**

  ```bash
  git add lingxi-core/crates/hooks/tests/http_executor_test.rs
  git commit -m "feat(M5-06 T6): HTTP timeout test with tokio::test(start_paused=true) — proves Timeout outcome path"
  ```

---

### Task 7: CommandExecutor failing test (RED)

**Files:**
- Create: `lingxi-core/crates/hooks/src/command_executor.rs` — empty stub.
- Create: `lingxi-core/crates/hooks/tests/command_executor_test.rs`.
- Modify: `lingxi-core/crates/hooks/src/lib.rs` (`mod command_executor;`).

- [ ] **Step 1: Create the empty stub.**

  Write `lingxi-core/crates/hooks/src/command_executor.rs`:
  ```rust
  //! Command (shell-exec) hook executor.
  //!
  //! T7: stub. T8: full impl.

  #![forbid(unsafe_code)]

  use crate::definition::HookDefinition;
  use crate::response::{HookOutcome, HookResult};
  use lingxi_traits::RuntimeSpawner;
  use std::collections::HashMap;
  use std::path::Path;
  use std::sync::Arc;
  use std::time::Duration;

  pub(crate) struct CommandExecutor {
      pub(crate) runtime: Arc<dyn RuntimeSpawner>,
      pub(crate) timeout: Duration,
  }

  pub(crate) enum CommandExecutionSignal {
      Ok,
      TimedOut,
  }

  pub(crate) struct CommandExecutionOutcome {
      pub(crate) result: HookResult,
      pub(crate) signal: CommandExecutionSignal,
  }

  impl CommandExecutor {
      #[allow(clippy::too_many_arguments)]
      pub(crate) async fn execute(
          &self,
          _hook: &HookDefinition,
          _command: &str,
          _args: &[String],
          _env: &HashMap<String, String>,
          _cwd: Option<&Path>,
          _stdin_payload: &str,
          _expected_event: &'static str,
      ) -> CommandExecutionOutcome {
          CommandExecutionOutcome {
              result: HookResult {
                  outcome: HookOutcome::Error,
                  stdout: String::new(),
                  stderr: "command executor not yet implemented (M5-06 T7 stub)".into(),
                  exit_code: None,
                  response: None,
              },
              signal: CommandExecutionSignal::Ok,
          }
      }
  }
  ```

- [ ] **Step 2: Wire in `lib.rs`.**

  Add `mod command_executor;` after `mod http_executor;`.

- [ ] **Step 3: Create the failing test.**

  Write `lingxi-core/crates/hooks/tests/command_executor_test.rs`:
  ```rust
  //! Integration tests for the Command hook executor.

  use lingxi_hooks::events::HookEvent;
  use lingxi_hooks::registry::{HookContext, HookRegistry};
  use lingxi_hooks::definition::{HookDefinition, HookExecutor, HookSource};
  use lingxi_hooks::executor::HookExecutorImpl;
  use lingxi_hooks::{HookDecision, HookEventType, HookOutcome};
  use lingxi_protocol::{HookId, SessionId, ToolUseId};
  use lingxi_traits::{
      HttpRequest, HttpResponse, HttpTransport, RuntimeSpawner, SpawnError,
      SpawnedProcessOutput,
  };
  use serde_json::json;
  use std::collections::HashMap;
  use std::path::Path;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;
  use tokio::sync::RwLock;

  struct InertHttp;
  #[async_trait::async_trait]
  impl HttpTransport for InertHttp {
      async fn request(&self, _r: HttpRequest) -> Result<HttpResponse, lingxi_traits::HttpError> {
          Err(lingxi_traits::HttpError::Transport("inert".into()))
      }
  }

  /// Captures the stdin string + returns a scripted stdout/exit_code/stderr.
  struct ScriptedRuntime {
      stdin_capture: Mutex<Vec<String>>,
      stdout: String,
      stderr: String,
      exit_code: i32,
  }

  #[async_trait::async_trait]
  impl RuntimeSpawner for ScriptedRuntime {
      async fn spawn_with_stdin(
          &self,
          _command: &str,
          _args: &[String],
          _env: &HashMap<String, String>,
          _cwd: Option<&Path>,
          stdin: &str,
          _timeout: Duration,
      ) -> Result<SpawnedProcessOutput, SpawnError> {
          self.stdin_capture.lock().unwrap().push(stdin.to_string());
          Ok(SpawnedProcessOutput {
              stdout: self.stdout.clone(),
              stderr: self.stderr.clone(),
              exit_code: self.exit_code,
              timed_out: false,
          })
      }
  }

  fn make_command_hook() -> HookDefinition {
      HookDefinition {
          id: HookId::new(),
          name: "test-cmd".into(),
          events: vec![HookEventType::PreToolUse],
          if_condition: None,
          executor: HookExecutor::Command {
              command: "/usr/bin/true".into(),
              args: vec![],
              env: HashMap::new(),
              cwd: None,
          },
          source: HookSource::User,
          blocking: true,
          timeout: None,
          priority: 0,
      }
  }

  #[tokio::test]
  async fn command_arm_pipes_payload_to_stdin_and_parses_deny_response() {
      let runtime = Arc::new(ScriptedRuntime {
          stdin_capture: Mutex::new(Vec::new()),
          stdout: r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"forbidden"}}"#.into(),
          stderr: String::new(),
          exit_code: 0,
      });
      let http = Arc::new(InertHttp);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_command_hook());
      let exec = HookExecutorImpl::new(registry, http, runtime.clone());

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({"command": "rm -rf /"}),
          tool_use_id: ToolUseId::from("tu-cmd"),
      };
      let ctx = HookContext {
          session_id: SessionId::from("sess"),
          cwd: std::path::PathBuf::from("/work"),
          ..HookContext::default()
      };
      let agg = exec.execute(event, ctx).await;

      assert_eq!(agg.decision, Some(HookDecision::Block));
      assert_eq!(agg.reason.as_deref(), Some("forbidden"));
      let captured = runtime.stdin_capture.lock().unwrap().clone();
      assert_eq!(captured.len(), 1);
      assert!(captured[0].contains(r#""hook_event_name":"PreToolUse""#));
      assert!(captured[0].contains(r#""tool_name":"Bash""#));
      assert!(captured[0].contains(r#""tool_input":{"command":"rm -rf /"}"#));
  }
  ```

- [ ] **Step 4: Run and observe failure.**

  ```bash
  cargo test -p lingxi-hooks --test command_executor_test --quiet 2>&1 | head -30
  ```

  Expected RED: the test fails because the stubbed `command_executor` returns `HookOutcome::Error` rather than parsing the deny response.

- [ ] **Step 5: Commit (RED).**

  ```bash
  git add lingxi-core/crates/hooks/src/command_executor.rs \
          lingxi-core/crates/hooks/src/lib.rs \
          lingxi-core/crates/hooks/tests/command_executor_test.rs
  git commit -m "test(M5-06 T7): RED — CommandExecutor pipes payload to stdin + parses deny response"
  ```

---

### Task 8: CommandExecutor implementation (GREEN)

**Files:**
- Modify: `lingxi-core/crates/hooks/src/command_executor.rs` (fill `execute` body).
- Modify: `lingxi-core/crates/hooks/src/executor.rs` (replace the stubbed `Command` arm).

- [ ] **Step 1: Fill the impl.**

  Replace `command_executor.rs`:
  ```rust
  //! Command (shell-exec) hook executor.

  #![forbid(unsafe_code)]

  use crate::definition::{HookDefinition, HookExecutor};
  use crate::hook_payload::parse_response;
  use crate::response::{HookOutcome, HookResponse, HookResult};
  use lingxi_traits::RuntimeSpawner;
  use std::collections::HashMap;
  use std::path::Path;
  use std::sync::Arc;
  use std::time::Duration;

  pub(crate) struct CommandExecutor {
      pub(crate) runtime: Arc<dyn RuntimeSpawner>,
      pub(crate) timeout: Duration,
  }

  pub(crate) enum CommandExecutionSignal {
      Ok,
      TimedOut,
  }

  pub(crate) struct CommandExecutionOutcome {
      pub(crate) result: HookResult,
      pub(crate) signal: CommandExecutionSignal,
  }

  impl CommandExecutor {
      #[allow(clippy::too_many_arguments)]
      pub(crate) async fn execute(
          &self,
          hook: &HookDefinition,
          command: &str,
          args: &[String],
          env: &HashMap<String, String>,
          cwd: Option<&Path>,
          stdin_payload: &str,
          expected_event: &'static str,
      ) -> CommandExecutionOutcome {
          // Per-hook timeout override (hook.timeout is at the definition level,
          // not on the Command variant — claude-code parity).
          let effective_timeout = hook.timeout.unwrap_or(self.timeout);

          let spawn = self.runtime.spawn_with_stdin(
              command,
              args,
              env,
              cwd,
              stdin_payload,
              effective_timeout,
          );

          let output = match spawn.await {
              Ok(o) => o,
              Err(e) => {
                  return CommandExecutionOutcome {
                      result: HookResult {
                          outcome: HookOutcome::Error,
                          stdout: String::new(),
                          stderr: format!("Hook {} failed: spawn: {e}", hook.id),
                          exit_code: None,
                          response: None,
                      },
                      signal: CommandExecutionSignal::Ok,
                  };
              }
          };

          if output.timed_out {
              return CommandExecutionOutcome {
                  result: HookResult {
                      outcome: HookOutcome::Timeout,
                      stdout: output.stdout,
                      stderr: format!(
                          "Hook {} failed: timeout after {}ms",
                          hook.id,
                          effective_timeout.as_millis()
                      ),
                      exit_code: Some(output.exit_code),
                      response: None,
                  },
                  signal: CommandExecutionSignal::TimedOut,
              };
          }

          // Try to parse stdout even when exit != 0 — claude-code's pattern is
          // that a hook can return non-zero with a parseable advisory response.
          let parsed: Option<HookResponse> = if output.stdout.trim().is_empty() {
              None
          } else {
              match parse_response(&output.stdout, expected_event) {
                  Ok(r) => Some(r),
                  Err(e) => {
                      // Parse failure on non-empty stdout: non-blocking error.
                      return CommandExecutionOutcome {
                          result: HookResult {
                              outcome: HookOutcome::Error,
                              stdout: output.stdout,
                              stderr: format!(
                                  "Hook {} failed: parse: {e} (stderr: {})",
                                  hook.id, output.stderr
                              ),
                              exit_code: Some(output.exit_code),
                              response: None,
                          },
                          signal: CommandExecutionSignal::Ok,
                      };
                  }
              }
          };

          let outcome = if output.exit_code == 0 {
              HookOutcome::Success
          } else {
              HookOutcome::Error
          };

          CommandExecutionOutcome {
              result: HookResult {
                  outcome,
                  stdout: output.stdout,
                  stderr: output.stderr,
                  exit_code: Some(output.exit_code),
                  response: parsed,
              },
              signal: CommandExecutionSignal::Ok,
          }
      }
  }

  // Silence unused-import lint when the executor variant is not yet matched
  // in tests that only exercise Builtin/Http arms.
  #[allow(dead_code)]
  fn _force_use_executor_variant(_: HookExecutor) {}
  ```

- [ ] **Step 2: Wire the `Command` arm in `executor.rs`.**

  Replace the previously-stubbed `HookExecutor::Command { .. } | HookExecutor::Agent { .. }` combined arm with two separate arms:
  ```rust
  HookExecutor::Command { command, args, env, cwd } => {
      let envelope_json = match self.serialize_event(event, ctx) {
          Ok(s) => s,
          Err(e) => {
              return HookResult {
                  outcome: HookOutcome::Error,
                  stdout: String::new(),
                  stderr: format!("Hook {} failed: serialize: {e}", hook.id),
                  exit_code: None,
                  response: None,
              };
          }
      };
      let expected = match event {
          HookEvent::PreToolUse { .. } => "PreToolUse",
          HookEvent::PostToolUse { .. } => "PostToolUse",
          _ => "Other",
      };
      let exec = crate::command_executor::CommandExecutor {
          runtime: self.runtime.clone(),
          timeout: std::time::Duration::from_millis(HOOK_COMMAND_TIMEOUT_MS),
      };
      let outcome = exec.execute(
          hook,
          command,
          args,
          env,
          cwd.as_deref(),
          &envelope_json,
          expected,
      ).await;
      if matches!(outcome.signal, crate::command_executor::CommandExecutionSignal::TimedOut) {
          self.emit_timeout(hook, "command").await;
      }
      outcome.result
  }
  HookExecutor::Agent { .. } => {
      // Filled in T12. Stub for now so the match compiles.
      HookResult {
          outcome: HookOutcome::Error,
          stdout: String::new(),
          stderr: format!("Hook {} failed: agent executor not wired (M5-06 T12)", hook.id),
          exit_code: None,
          response: None,
      }
  }
  ```

- [ ] **Step 3: Run tests.**

  ```bash
  cargo test -p lingxi-hooks --test command_executor_test --quiet
  ```

  Expected: 1 passed.

- [ ] **Step 4: Run clippy + fmt.**

  ```bash
  cargo clippy -p lingxi-hooks -- -D warnings
  cargo fmt -p lingxi-hooks --check
  ```

- [ ] **Step 5: Commit (GREEN).**

  ```bash
  git add lingxi-core/crates/hooks/src/command_executor.rs \
          lingxi-core/crates/hooks/src/executor.rs
  git commit -m "feat(M5-06 T8): GREEN — CommandExecutor spawns via RuntimeSpawner, pipes envelope JSON, parses stdout"
  ```

---

### Task 9: Command timeout test

**Files:**
- Modify: `lingxi-core/crates/hooks/tests/command_executor_test.rs` (add a timeout test using a `ScriptedRuntime` variant that reports `timed_out: true`).

- [ ] **Step 1: Add a timeout-marking runtime + test.**

  Append to `lingxi-core/crates/hooks/tests/command_executor_test.rs`:
  ```rust
  struct TimingOutRuntime;
  #[async_trait::async_trait]
  impl RuntimeSpawner for TimingOutRuntime {
      async fn spawn_with_stdin(
          &self,
          _command: &str,
          _args: &[String],
          _env: &HashMap<String, String>,
          _cwd: Option<&Path>,
          _stdin: &str,
          _timeout: Duration,
      ) -> Result<SpawnedProcessOutput, SpawnError> {
          Ok(SpawnedProcessOutput {
              stdout: String::new(),
              stderr: "exceeded timeout".into(),
              exit_code: -1,
              timed_out: true,
          })
      }
  }

  #[tokio::test]
  async fn command_arm_reports_timeout_when_runtime_signals_timed_out() {
      let runtime = Arc::new(TimingOutRuntime);
      let http = Arc::new(InertHttp);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_command_hook());
      let exec = HookExecutorImpl::new(registry, http, runtime);

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let ctx = HookContext::default();
      let agg = exec.execute(event, ctx).await;
      let result = &agg.all_results[0].1;
      assert!(matches!(result.outcome, HookOutcome::Timeout));
      assert!(result.stderr.contains("timeout"));
  }
  ```

- [ ] **Step 2: Run tests.**

  ```bash
  cargo test -p lingxi-hooks --test command_executor_test --quiet
  ```

  Expected: 2 passed.

- [ ] **Step 3: Commit.**

  ```bash
  git add lingxi-core/crates/hooks/tests/command_executor_test.rs
  git commit -m "feat(M5-06 T9): Command arm timeout test — RuntimeSpawner's timed_out flag → HookOutcome::Timeout"
  ```

---

### Task 10: Command non-zero exit captures stderr

**Files:**
- Modify: `lingxi-core/crates/hooks/tests/command_executor_test.rs` (add a non-zero-exit test).

- [ ] **Step 1: Add the test.**

  Append to `lingxi-core/crates/hooks/tests/command_executor_test.rs`:
  ```rust
  #[tokio::test]
  async fn command_arm_returns_error_outcome_with_stderr_when_exit_code_nonzero() {
      let runtime = Arc::new(ScriptedRuntime {
          stdin_capture: Mutex::new(Vec::new()),
          stdout: String::new(),
          stderr: "boom\nparse error at line 42".into(),
          exit_code: 1,
      });
      let http = Arc::new(InertHttp);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_command_hook());
      let exec = HookExecutorImpl::new(registry, http, runtime);

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let ctx = HookContext::default();
      let agg = exec.execute(event, ctx).await;
      let result = &agg.all_results[0].1;
      assert!(matches!(result.outcome, HookOutcome::Error));
      assert_eq!(result.exit_code, Some(1));
      assert!(result.stderr.contains("boom"));
      assert!(result.stderr.contains("line 42"));
      // No response parsed because stdout was empty.
      assert!(result.response.is_none());
      // Aggregate has no decision because no response.
      assert_eq!(agg.decision, None);
  }

  #[tokio::test]
  async fn command_arm_parses_advisory_response_on_nonzero_exit() {
      // claude-code pattern: hook returns JSON + non-zero exit = non-fatal advisory.
      let runtime = Arc::new(ScriptedRuntime {
          stdin_capture: Mutex::new(Vec::new()),
          stdout: r#"{"systemMessage":"please retry without sudo"}"#.into(),
          stderr: String::new(),
          exit_code: 2,
      });
      let http = Arc::new(InertHttp);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_command_hook());
      let exec = HookExecutorImpl::new(registry, http, runtime);

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let ctx = HookContext::default();
      let agg = exec.execute(event, ctx).await;
      let result = &agg.all_results[0].1;
      // Outcome is Error (non-zero exit) but the response is parsed.
      assert!(matches!(result.outcome, HookOutcome::Error));
      assert_eq!(result.exit_code, Some(2));
      assert!(result.response.is_some());
      assert_eq!(
          agg.system_messages,
          vec!["please retry without sudo".to_string()]
      );
  }
  ```

- [ ] **Step 2: Run tests.**

  ```bash
  cargo test -p lingxi-hooks --test command_executor_test --quiet
  ```

  Expected: 4 passed.

- [ ] **Step 3: Commit.**

  ```bash
  git add lingxi-core/crates/hooks/tests/command_executor_test.rs
  git commit -m "feat(M5-06 T10): Command arm non-zero exit captures stderr + parses advisory stdout"
  ```

---

### Task 11: AgentExecutor failing test (RED)

**Files:**
- Create: `lingxi-core/crates/hooks/src/agent_executor.rs` — stub.
- Create: `lingxi-core/crates/hooks/tests/agent_executor_test.rs`.
- Modify: `lingxi-core/crates/hooks/src/lib.rs` (`mod agent_executor;`).
- Modify: `lingxi-core/crates/hooks/src/executor.rs` (add `with_agent_spawner` builder + `agent_spawner` field — minimal, enough to compile).

- [ ] **Step 1: Create the stub.**

  Write `lingxi-core/crates/hooks/src/agent_executor.rs`:
  ```rust
  //! Agent (subagent-spawn) hook executor.

  #![forbid(unsafe_code)]

  use crate::definition::HookDefinition;
  use crate::response::{HookOutcome, HookResult};
  use lingxi_traits::{SubagentInheritance, SubagentSpawner};
  use std::sync::Arc;
  use std::time::Duration;

  pub(crate) struct AgentExecutor {
      pub(crate) spawner: Option<Arc<dyn SubagentSpawner>>,
      pub(crate) timeout: Duration,
  }

  pub(crate) enum AgentExecutionSignal {
      Ok,
      TimedOut,
      NotWired,
  }

  pub(crate) struct AgentExecutionOutcome {
      pub(crate) result: HookResult,
      pub(crate) signal: AgentExecutionSignal,
  }

  impl AgentExecutor {
      #[allow(clippy::too_many_arguments)]
      pub(crate) async fn execute(
          &self,
          hook: &HookDefinition,
          _agent_type: &str,
          _prompt_template: &str,
          _payload_json: &str,
          _expected_event: &'static str,
          _inherit: Option<SubagentInheritance>,
      ) -> AgentExecutionOutcome {
          AgentExecutionOutcome {
              result: HookResult {
                  outcome: HookOutcome::Error,
                  stdout: String::new(),
                  stderr: format!("Hook {} failed: agent executor not yet implemented (T11 stub)", hook.id),
                  exit_code: None,
                  response: None,
              },
              signal: AgentExecutionSignal::NotWired,
          }
      }
  }
  ```

- [ ] **Step 2: Add `agent_spawner` field + `with_agent_spawner` builder to `HookExecutorImpl`.**

  In `executor.rs`, add the field to the struct definition (next to `builtin_handlers`):
  ```rust
  agent_spawner: Option<Arc<dyn lingxi_traits::SubagentSpawner>>,
  ```
  Initialize it to `None` in `new()`. Add the builder method:
  ```rust
  /// Attach a [`SubagentSpawner`] so `HookExecutor::Agent` hooks can fork
  /// a subagent for their response. Without this, agent-arm hooks fail
  /// with a "not wired" error.
  #[must_use]
  pub fn with_agent_spawner(mut self, spawner: Arc<dyn lingxi_traits::SubagentSpawner>) -> Self {
      self.agent_spawner = Some(spawner);
      self
  }
  ```

- [ ] **Step 3: Wire `mod agent_executor;` in `lib.rs`.**

- [ ] **Step 4: Create the failing test.**

  Write `lingxi-core/crates/hooks/tests/agent_executor_test.rs`:
  ```rust
  //! Integration tests for the Agent hook executor.

  use lingxi_hooks::events::HookEvent;
  use lingxi_hooks::registry::{HookContext, HookRegistry};
  use lingxi_hooks::definition::{HookDefinition, HookExecutor, HookSource};
  use lingxi_hooks::executor::HookExecutorImpl;
  use lingxi_hooks::{HookDecision, HookEventType, HookOutcome};
  use lingxi_protocol::{HookId, SessionId, ToolUseId};
  use lingxi_traits::{
      BudgetEnforcerHandle, HttpRequest, HttpResponse, HttpTransport, RuntimeSpawner,
      SpawnError, SpawnedProcessOutput, SubagentInheritance, SubagentResult,
      SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner, SubagentUsage, ToolInvoker,
  };
  use serde_json::json;
  use std::collections::HashMap;
  use std::sync::{Arc, Mutex};
  use tokio::sync::RwLock;

  struct InertHttp;
  #[async_trait::async_trait]
  impl HttpTransport for InertHttp {
      async fn request(&self, _r: HttpRequest) -> Result<HttpResponse, lingxi_traits::HttpError> {
          Err(lingxi_traits::HttpError::Transport("inert".into()))
      }
  }
  struct InertRuntime;
  #[async_trait::async_trait]
  impl RuntimeSpawner for InertRuntime {
      async fn spawn_with_stdin(
          &self,
          _: &str, _: &[String], _: &HashMap<String, String>,
          _: Option<&std::path::Path>, _: &str, _: std::time::Duration,
      ) -> Result<SpawnedProcessOutput, SpawnError> {
          Err(SpawnError::Internal("inert".into()))
      }
  }
  struct InertTools;
  #[async_trait::async_trait]
  impl ToolInvoker for InertTools {
      async fn invoke(
          &self,
          _name: &str,
          _input: serde_json::Value,
          _ctx: lingxi_traits::SubagentInvocationContext,
      ) -> Result<serde_json::Value, lingxi_traits::ToolInvokerError> {
          Err(lingxi_traits::ToolInvokerError::ToolNotFound("inert".into()))
      }
  }
  struct InertBudget;
  #[async_trait::async_trait]
  impl BudgetEnforcerHandle for InertBudget {
      async fn charge(&self, _tokens: u64) -> Result<(), lingxi_traits::BudgetError> { Ok(()) }
      async fn remaining(&self) -> u64 { u64::MAX }
  }

  /// Mock spawner that captures its request and returns a scripted Completed.
  struct ScriptedSpawner {
      captured: Mutex<Vec<SubagentSpawnRequest>>,
      content: serde_json::Value,
  }
  #[async_trait::async_trait]
  impl SubagentSpawner for ScriptedSpawner {
      async fn spawn(
          &self,
          req: SubagentSpawnRequest,
          _inherit: SubagentInheritance,
      ) -> Result<SubagentResult, SubagentSpawnError> {
          self.captured.lock().unwrap().push(req);
          Ok(SubagentResult::Completed {
              content: self.content.clone(),
              usage: SubagentUsage::default(),
          })
      }
  }

  fn make_agent_hook() -> HookDefinition {
      HookDefinition {
          id: HookId::new(),
          name: "test-agent".into(),
          events: vec![HookEventType::PreToolUse],
          if_condition: None,
          executor: HookExecutor::Agent {
              agent_type: "guard".into(),
              prompt: "Review the following tool invocation.".into(),
          },
          source: HookSource::User,
          blocking: true,
          timeout: None,
          priority: 0,
      }
  }

  #[tokio::test]
  async fn agent_arm_spawns_subagent_and_parses_allow_response() {
      let spawner = Arc::new(ScriptedSpawner {
          captured: Mutex::new(Vec::new()),
          content: json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}),
      });
      let http = Arc::new(InertHttp);
      let runtime = Arc::new(InertRuntime);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_agent_hook());
      let exec = HookExecutorImpl::new(registry, http, runtime)
          .with_agent_spawner(spawner.clone());

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({"command": "ls"}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let inherit = SubagentInheritance {
          tool_invoker: Arc::new(InertTools),
          budget: Arc::new(InertBudget),
      };
      let ctx = HookContext {
          session_id: SessionId::from("s"),
          cwd: std::path::PathBuf::from("/w"),
          inherit: Some(inherit),
          ..HookContext::default()
      };
      let agg = exec.execute(event, ctx).await;

      assert_eq!(agg.decision, Some(HookDecision::Approve));
      let captured = spawner.captured.lock().unwrap().clone();
      assert_eq!(captured.len(), 1);
      assert_eq!(captured[0].subagent_type, "guard");
      assert!(captured[0].prompt.contains("Review the following tool invocation."));
      assert!(captured[0].prompt.contains(r#""tool_name":"Bash""#));
  }
  ```

- [ ] **Step 5: Run and observe failure.**

  ```bash
  cargo test -p lingxi-hooks --test agent_executor_test --quiet 2>&1 | head -30
  ```

  Expected RED: the Agent arm in `executor.rs` still returns the "not wired" stub, so the test fails at `assert_eq!(agg.decision, Some(HookDecision::Approve))`.

- [ ] **Step 6: Commit (RED).**

  ```bash
  git add lingxi-core/crates/hooks/src/agent_executor.rs \
          lingxi-core/crates/hooks/src/executor.rs \
          lingxi-core/crates/hooks/src/lib.rs \
          lingxi-core/crates/hooks/tests/agent_executor_test.rs
  git commit -m "test(M5-06 T11): RED — AgentExecutor spawns subagent + parses response (with_agent_spawner builder)"
  ```

---

### Task 12: AgentExecutor implementation (GREEN)

**Files:**
- Modify: `lingxi-core/crates/hooks/src/agent_executor.rs` (fill `execute`).
- Modify: `lingxi-core/crates/hooks/src/executor.rs` (replace stubbed `Agent` arm).

- [ ] **Step 1: Fill the impl.**

  Replace `agent_executor.rs`:
  ```rust
  //! Agent (subagent-spawn) hook executor.

  #![forbid(unsafe_code)]

  use crate::definition::HookDefinition;
  use crate::hook_payload::parse_response;
  use crate::response::{HookOutcome, HookResponse, HookResult};
  use lingxi_traits::{SubagentInheritance, SubagentResult, SubagentSpawner, SubagentSpawnRequest};
  use std::sync::Arc;
  use std::time::Duration;

  pub(crate) struct AgentExecutor {
      pub(crate) spawner: Option<Arc<dyn SubagentSpawner>>,
      pub(crate) timeout: Duration,
  }

  pub(crate) enum AgentExecutionSignal {
      Ok,
      TimedOut,
      NotWired,
  }

  pub(crate) struct AgentExecutionOutcome {
      pub(crate) result: HookResult,
      pub(crate) signal: AgentExecutionSignal,
  }

  impl AgentExecutor {
      #[allow(clippy::too_many_arguments)]
      pub(crate) async fn execute(
          &self,
          hook: &HookDefinition,
          agent_type: &str,
          prompt_template: &str,
          payload_json: &str,
          expected_event: &'static str,
          inherit: Option<SubagentInheritance>,
      ) -> AgentExecutionOutcome {
          let Some(spawner) = &self.spawner else {
              return AgentExecutionOutcome {
                  result: HookResult {
                      outcome: HookOutcome::Error,
                      stdout: String::new(),
                      stderr: format!("Hook {} failed: agent executor not wired", hook.id),
                      exit_code: None,
                      response: None,
                  },
                  signal: AgentExecutionSignal::NotWired,
              };
          };
          let Some(inherit) = inherit else {
              return AgentExecutionOutcome {
                  result: HookResult {
                      outcome: HookOutcome::Error,
                      stdout: String::new(),
                      stderr: format!(
                          "Hook {} failed: agent arm needs SubagentInheritance in HookContext",
                          hook.id
                      ),
                      exit_code: None,
                      response: None,
                  },
                  signal: AgentExecutionSignal::NotWired,
              };
          };

          let effective_timeout = hook.timeout.unwrap_or(self.timeout);

          let request = SubagentSpawnRequest {
              subagent_type: agent_type.to_string(),
              prompt: format!("{prompt_template}\n\n{payload_json}"),
              context_paths: vec![],
          };
          let fut = spawner.spawn(request, inherit);
          let spawned = match tokio::time::timeout(effective_timeout, fut).await {
              Err(_) => {
                  return AgentExecutionOutcome {
                      result: HookResult {
                          outcome: HookOutcome::Timeout,
                          stdout: String::new(),
                          stderr: format!(
                              "Hook {} failed: timeout after {}ms",
                              hook.id,
                              effective_timeout.as_millis()
                          ),
                          exit_code: None,
                          response: None,
                      },
                      signal: AgentExecutionSignal::TimedOut,
                  };
              }
              Ok(Err(e)) => {
                  return AgentExecutionOutcome {
                      result: HookResult {
                          outcome: HookOutcome::Error,
                          stdout: String::new(),
                          stderr: format!("Hook {} failed: spawn: {e}", hook.id),
                          exit_code: None,
                          response: None,
                      },
                      signal: AgentExecutionSignal::Ok,
                  };
              }
              Ok(Ok(r)) => r,
          };

          match spawned {
              SubagentResult::Completed { content, .. } => {
                  let content_str = match &content {
                      serde_json::Value::String(s) => s.clone(),
                      other => other.to_string(),
                  };
                  let parsed: Option<HookResponse> = if content_str.trim().is_empty() {
                      None
                  } else {
                      match parse_response(&content_str, expected_event) {
                          Ok(r) => Some(r),
                          Err(e) => {
                              return AgentExecutionOutcome {
                                  result: HookResult {
                                      outcome: HookOutcome::Error,
                                      stdout: content_str,
                                      stderr: format!("Hook {} failed: parse: {e}", hook.id),
                                      exit_code: None,
                                      response: None,
                                  },
                                  signal: AgentExecutionSignal::Ok,
                              };
                          }
                      }
                  };
                  AgentExecutionOutcome {
                      result: HookResult {
                          outcome: HookOutcome::Success,
                          stdout: content_str,
                          stderr: String::new(),
                          exit_code: None,
                          response: parsed,
                      },
                      signal: AgentExecutionSignal::Ok,
                  }
              }
              SubagentResult::Failed { reason } => AgentExecutionOutcome {
                  result: HookResult {
                      outcome: HookOutcome::Error,
                      stdout: String::new(),
                      stderr: format!("Hook {} failed: subagent failed: {reason}", hook.id),
                      exit_code: None,
                      response: None,
                  },
                  signal: AgentExecutionSignal::Ok,
              },
              SubagentResult::Killed => AgentExecutionOutcome {
                  result: HookResult {
                      outcome: HookOutcome::Cancelled,
                      stdout: String::new(),
                      stderr: format!("Hook {} failed: subagent killed", hook.id),
                      exit_code: None,
                      response: None,
                  },
                  signal: AgentExecutionSignal::Ok,
              },
          }
      }
  }
  ```

- [ ] **Step 2: Wire the `Agent` arm in `executor.rs`.**

  Replace the placeholder `HookExecutor::Agent { .. } => { … }` arm with:
  ```rust
  HookExecutor::Agent { agent_type, prompt } => {
      let envelope_json = match self.serialize_event(event, ctx) {
          Ok(s) => s,
          Err(e) => {
              return HookResult {
                  outcome: HookOutcome::Error,
                  stdout: String::new(),
                  stderr: format!("Hook {} failed: serialize: {e}", hook.id),
                  exit_code: None,
                  response: None,
              };
          }
      };
      let expected = match event {
          HookEvent::PreToolUse { .. } => "PreToolUse",
          HookEvent::PostToolUse { .. } => "PostToolUse",
          _ => "Other",
      };
      let exec = crate::agent_executor::AgentExecutor {
          spawner: self.agent_spawner.clone(),
          timeout: std::time::Duration::from_millis(HOOK_AGENT_TIMEOUT_MS),
      };
      let outcome = exec.execute(
          hook,
          agent_type,
          prompt,
          &envelope_json,
          expected,
          ctx.inherit.clone(),
      ).await;
      if matches!(outcome.signal, crate::agent_executor::AgentExecutionSignal::TimedOut) {
          self.emit_timeout(hook, "agent").await;
      }
      outcome.result
  }
  ```

- [ ] **Step 3: Run tests.**

  ```bash
  cargo test -p lingxi-hooks --test agent_executor_test --quiet
  ```

  Expected: 1 passed.

- [ ] **Step 4: Add a Failed/Killed test.**

  Append to `agent_executor_test.rs`:
  ```rust
  struct FailingSpawner;
  #[async_trait::async_trait]
  impl SubagentSpawner for FailingSpawner {
      async fn spawn(
          &self,
          _req: SubagentSpawnRequest,
          _inherit: SubagentInheritance,
      ) -> Result<SubagentResult, SubagentSpawnError> {
          Ok(SubagentResult::Failed { reason: "model went brrr".into() })
      }
  }

  #[tokio::test]
  async fn agent_arm_maps_failed_to_error_outcome() {
      let spawner = Arc::new(FailingSpawner);
      let http = Arc::new(InertHttp);
      let runtime = Arc::new(InertRuntime);
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(make_agent_hook());
      let exec = HookExecutorImpl::new(registry, http, runtime)
          .with_agent_spawner(spawner);
      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let ctx = HookContext {
          inherit: Some(SubagentInheritance {
              tool_invoker: Arc::new(InertTools),
              budget: Arc::new(InertBudget),
          }),
          ..HookContext::default()
      };
      let agg = exec.execute(event, ctx).await;
      let result = &agg.all_results[0].1;
      assert!(matches!(result.outcome, HookOutcome::Error));
      assert!(result.stderr.contains("model went brrr"));
  }
  ```

- [ ] **Step 5: Run tests + clippy + fmt.**

  ```bash
  cargo test -p lingxi-hooks --test agent_executor_test --quiet
  cargo clippy -p lingxi-hooks -- -D warnings
  cargo fmt -p lingxi-hooks --check
  ```

- [ ] **Step 6: Commit (GREEN).**

  ```bash
  git add lingxi-core/crates/hooks/src/agent_executor.rs \
          lingxi-core/crates/hooks/src/executor.rs \
          lingxi-core/crates/hooks/tests/agent_executor_test.rs
  git commit -m "feat(M5-06 T12): GREEN — AgentExecutor spawns via SubagentSpawner + maps Completed/Failed/Killed"
  ```

---

### Task 13: Full dispatch integration test + M4-05 regression gate

**Files:**
- Create: `lingxi-core/crates/hooks/tests/full_dispatch_test.rs`.

- [ ] **Step 1: Exercise all 4 arms in one registry.**

  Write `lingxi-core/crates/hooks/tests/full_dispatch_test.rs`:
  ```rust
  //! End-to-end: a single registry holds one hook of each kind; the executor
  //! routes each event to the correct arm.

  use async_trait::async_trait;
  use lingxi_hooks::events::HookEvent;
  use lingxi_hooks::registry::{HookContext, HookRegistry};
  use lingxi_hooks::definition::{HookDefinition, HookExecutor, HookSource};
  use lingxi_hooks::executor::{BuiltinHookHandler, HookExecutorImpl};
  use lingxi_hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
  use lingxi_hooks::HookEventType;
  use lingxi_protocol::{HookId, SessionId, ToolUseId};
  use lingxi_traits::{
      BudgetEnforcerHandle, HttpRequest, HttpResponse, HttpTransport, RuntimeSpawner,
      SpawnError, SpawnedProcessOutput, SubagentInheritance, SubagentResult,
      SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner, SubagentUsage, ToolInvoker,
  };
  use serde_json::json;
  use std::collections::HashMap;
  use std::sync::{Arc, Mutex};
  use tokio::sync::RwLock;

  struct EchoHttp { body: String }
  #[async_trait]
  impl HttpTransport for EchoHttp {
      async fn request(&self, _r: HttpRequest) -> Result<HttpResponse, lingxi_traits::HttpError> {
          Ok(HttpResponse { status: 200, headers: HashMap::new(), body: self.body.clone().into_bytes() })
      }
  }
  struct EchoRuntime { stdout: String }
  #[async_trait]
  impl RuntimeSpawner for EchoRuntime {
      async fn spawn_with_stdin(
          &self, _: &str, _: &[String], _: &HashMap<String, String>,
          _: Option<&std::path::Path>, _: &str, _: std::time::Duration,
      ) -> Result<SpawnedProcessOutput, SpawnError> {
          Ok(SpawnedProcessOutput {
              stdout: self.stdout.clone(),
              stderr: String::new(),
              exit_code: 0,
              timed_out: false,
          })
      }
  }
  struct EchoSpawner { content: serde_json::Value, parent_invoker_ptr: Mutex<Option<*const ()>>, parent_budget_ptr: Mutex<Option<*const ()>> }
  unsafe impl Send for EchoSpawner {}
  unsafe impl Sync for EchoSpawner {}
  #[async_trait]
  impl SubagentSpawner for EchoSpawner {
      async fn spawn(
          &self, _req: SubagentSpawnRequest, inherit: SubagentInheritance,
      ) -> Result<SubagentResult, SubagentSpawnError> {
          // Capture Arc raw pointers to assert Arc::ptr_eq from the test side.
          *self.parent_invoker_ptr.lock().unwrap() =
              Some(Arc::as_ptr(&inherit.tool_invoker) as *const ());
          *self.parent_budget_ptr.lock().unwrap() =
              Some(Arc::as_ptr(&inherit.budget) as *const ());
          Ok(SubagentResult::Completed {
              content: self.content.clone(),
              usage: SubagentUsage::default(),
          })
      }
  }
  struct InertTools;
  #[async_trait]
  impl ToolInvoker for InertTools {
      async fn invoke(&self, _: &str, _: serde_json::Value, _: lingxi_traits::SubagentInvocationContext)
          -> Result<serde_json::Value, lingxi_traits::ToolInvokerError> {
          Err(lingxi_traits::ToolInvokerError::ToolNotFound("inert".into()))
      }
  }
  struct InertBudget;
  #[async_trait]
  impl BudgetEnforcerHandle for InertBudget {
      async fn charge(&self, _: u64) -> Result<(), lingxi_traits::BudgetError> { Ok(()) }
      async fn remaining(&self) -> u64 { u64::MAX }
  }
  struct AllowHandler;
  #[async_trait]
  impl BuiltinHookHandler for AllowHandler {
      async fn handle(&self, _e: &HookEvent, _ctx: &HookContext) -> HookResult {
          HookResult {
              outcome: HookOutcome::Success,
              stdout: String::new(),
              stderr: String::new(),
              exit_code: None,
              response: Some(HookResponse {
                  decision: Some(HookDecision::Approve),
                  ..HookResponse::default()
              }),
          }
      }
      fn id(&self) -> &str { "allow-all" }
  }

  fn hook(executor: HookExecutor, priority: i32) -> HookDefinition {
      HookDefinition {
          id: HookId::new(),
          name: "h".into(),
          events: vec![HookEventType::PreToolUse],
          if_condition: None,
          executor,
          source: HookSource::User,
          blocking: true,
          timeout: None,
          priority,
      }
  }

  #[tokio::test]
  async fn full_registry_dispatches_all_four_arms_in_priority_order() {
      let http = Arc::new(EchoHttp {
          body: r#"{"systemMessage":"http-said-hi"}"#.into(),
      });
      let runtime = Arc::new(EchoRuntime {
          stdout: r#"{"systemMessage":"cmd-said-hi"}"#.into(),
      });
      let spawner = Arc::new(EchoSpawner {
          content: json!({"systemMessage":"agent-said-hi"}),
          parent_invoker_ptr: Mutex::new(None),
          parent_budget_ptr: Mutex::new(None),
      });

      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      {
          let mut r = registry.write().await;
          r.insert(hook(HookExecutor::Builtin { handler_id: "allow-all".into() }, 100));
          r.insert(hook(HookExecutor::Http {
              url: "https://example.com/h".into(),
              method: "POST".into(),
              headers: HashMap::new(),
              timeout: std::time::Duration::from_millis(0),
          }, 50));
          r.insert(hook(HookExecutor::Command {
              command: "/usr/bin/true".into(),
              args: vec![],
              env: HashMap::new(),
              cwd: None,
          }, 25));
          r.insert(hook(HookExecutor::Agent {
              agent_type: "g".into(),
              prompt: "check".into(),
          }, 10));
      }

      let invoker: Arc<dyn ToolInvoker> = Arc::new(InertTools);
      let budget: Arc<dyn BudgetEnforcerHandle> = Arc::new(InertBudget);
      let exec = HookExecutorImpl::new(registry, http, runtime)
          .with_builtin(Arc::new(AllowHandler))
          .with_agent_spawner(spawner.clone());

      let event = HookEvent::PreToolUse {
          tool_name: "Bash".into(),
          tool_input: json!({}),
          tool_use_id: ToolUseId::from("tu"),
      };
      let ctx = HookContext {
          session_id: SessionId::from("s"),
          cwd: std::path::PathBuf::from("/w"),
          inherit: Some(SubagentInheritance {
              tool_invoker: invoker.clone(),
              budget: budget.clone(),
          }),
          ..HookContext::default()
      };
      let agg = exec.execute(event, ctx).await;

      // First hook is Builtin AllowHandler → Approve → since Approve is not
      // Block, processing continues for all four hooks. We expect 4 results.
      assert_eq!(agg.all_results.len(), 4);
      assert_eq!(agg.system_messages, vec![
          "http-said-hi", "cmd-said-hi", "agent-said-hi",
      ]);

      // M4-05 regression gate: the spawner observed the parent's Arcs without
      // cloning the inner value.
      let captured_invoker_ptr = spawner.parent_invoker_ptr.lock().unwrap().unwrap();
      let captured_budget_ptr = spawner.parent_budget_ptr.lock().unwrap().unwrap();
      assert_eq!(captured_invoker_ptr, Arc::as_ptr(&invoker) as *const ());
      assert_eq!(captured_budget_ptr, Arc::as_ptr(&budget) as *const ());
  }
  ```

  Note: `HookExecutorImpl::with_builtin` does not exist yet — add a builder for parity with `with_agent_spawner`:
  ```rust
  /// Builder form of [`register_builtin`].
  #[must_use]
  pub fn with_builtin(mut self, h: Arc<dyn BuiltinHookHandler>) -> Self {
      self.builtin_handlers.insert(h.id().into(), h);
      self
  }
  ```

- [ ] **Step 2: Run the test.**

  ```bash
  cargo test -p lingxi-hooks --test full_dispatch_test --quiet
  ```

  Expected: 1 passed.

- [ ] **Step 3: Re-run M4-05 invariant tests.**

  ```bash
  cargo test -p lingxi-tools --test agent_tool_recursion_lock_test --quiet
  cargo test -p lingxi-tools --test agent_tool_budget_inheritance_test --quiet
  ```

  Expected: all green. If either fails, STOP — the new code path is somehow cloning an Arc that must remain identity-equal.

- [ ] **Step 4: Run clippy + fmt.**

  ```bash
  cargo clippy -p lingxi-hooks -- -D warnings
  cargo fmt -p lingxi-hooks --check
  ```

- [ ] **Step 5: Commit.**

  ```bash
  git add lingxi-core/crates/hooks/src/executor.rs \
          lingxi-core/crates/hooks/tests/full_dispatch_test.rs
  git commit -m "feat(M5-06 T13): full 4-arm dispatch test + M4-05 Arc::ptr_eq regression gate"
  ```

---

### Task 14: Orchestrator PreToolUse wiring

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs` (add `dispatch_tool_with_hooks` + reroute `run_turn`).
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_pre_post_hook_test.rs` (scripted hook denies Bash).

- [ ] **Step 1: Add `dispatch_tool_with_hooks` method body.**

  Open `lingxi-core/crates/orchestrator/src/conversation.rs`. Inside `impl ConversationOrchestrator`, append the method:
  ```rust
  /// Tool dispatch with `PreToolUse` + `PostToolUse` hooks (M5-06).
  ///
  /// Hooks may mutate `tool_input`, deny the call (returning
  /// `Err(ToolError::PermissionDenied)`), or append context to the result.
  async fn dispatch_tool_with_hooks(
      &self,
      tool_name: &str,
      mut tool_input: serde_json::Value,
      tool_use_id: lingxi_protocol::ToolUseId,
  ) -> Result<lingxi_tools::ToolCallResult, lingxi_tools::ToolError> {
      use lingxi_hooks::{HookDecision, HookEvent};

      // --- PreToolUse ---
      let pre_event = HookEvent::PreToolUse {
          tool_name: tool_name.to_string(),
          tool_input: tool_input.clone(),
          tool_use_id: tool_use_id.clone(),
      };
      let pre_ctx = self.build_hook_context();
      let pre_start = std::time::Instant::now();
      self.emit_hook_pre_started(tool_name, &pre_ctx).await;
      let pre_agg = self.hooks.execute(pre_event, pre_ctx).await;
      let pre_dur_ms = pre_start.elapsed().as_millis() as u64;

      if matches!(pre_agg.decision, Some(HookDecision::Block)) {
          let reason = pre_agg.reason.clone().unwrap_or_else(|| "blocked by hook".into());
          self.emit_hook_pre_completed(tool_name, "block", pre_dur_ms).await;
          return Err(lingxi_tools::ToolError::PermissionDenied(reason));
      }
      if let Some(updated) = pre_agg.modified_input {
          tool_input = updated;
      }
      let decision_tag = match pre_agg.decision {
          Some(HookDecision::Approve) => "approve",
          Some(HookDecision::Allow) => "allow",
          Some(HookDecision::Continue) => "continue",
          Some(HookDecision::Block) => "block", // unreachable here, but exhaustive
          None => "none",
      };
      self.emit_hook_pre_completed(tool_name, decision_tag, pre_dur_ms).await;

      // --- Tool call ---
      let mut result = self.invoke_tool(tool_name, tool_input.clone(), tool_use_id.clone()).await?;

      // --- PostToolUse ---
      let post_event = HookEvent::PostToolUse {
          tool_name: tool_name.to_string(),
          tool_input: tool_input.clone(),
          tool_output: result.data.clone(),
          tool_use_id,
      };
      let post_ctx = self.build_hook_context();
      let post_start = std::time::Instant::now();
      self.emit_hook_post_started(tool_name, &post_ctx).await;
      let post_agg = self.hooks.execute(post_event, post_ctx).await;
      let post_dur_ms = post_start.elapsed().as_millis() as u64;

      let mutated = !post_agg.system_messages.is_empty();
      if mutated {
          append_post_messages(&mut result, &post_agg.system_messages);
      }
      self.emit_hook_post_completed(tool_name, post_dur_ms, mutated).await;

      Ok(result)
  }

  /// Build the per-event [`HookContext`] from the orchestrator's state.
  fn build_hook_context(&self) -> lingxi_hooks::HookContext {
      lingxi_hooks::HookContext {
          session_id: self.session.id().clone(),
          cwd: self.cwd.clone(),
          transcript_path: self.session.transcript_path().to_path_buf(),
          permission_mode: None,
          agent_id: None,
          agent_type: None,
          inherit: Some(lingxi_traits::SubagentInheritance {
              tool_invoker: self.tools.invoker_arc(),
              budget: self.budget.clone(),
          }),
      }
  }
  ```

  **NOTE**: `self.invoke_tool` is the existing (M5-02 Task 10) private helper that wraps `self.tools.invoke(...)` — the M5-02 plan refers to it as the "tool dispatch loop's inner call". If your codebase's actual private method has a different name (e.g. `dispatch_tool`), match that name. If no private helper exists yet, inline `self.tools.invoke(...)` directly — but the rest of `dispatch_tool_with_hooks` stays as written.

  `self.tools.invoker_arc()` is a tiny accessor on `RegistryToolInvoker` returning `Arc<dyn ToolInvoker>` — add it in M5-01 if it doesn't yet exist:
  ```rust
  pub fn invoker_arc(self: &Arc<Self>) -> Arc<dyn ToolInvoker> {
      self.clone() as Arc<dyn ToolInvoker>
  }
  ```
  If the orchestrator doesn't already hold `self.tools` as `Arc<RegistryToolInvoker>`, change its type to `Arc<RegistryToolInvoker>` (already true per M5-01 close-out) so the upcast works without re-Arc-ing.

  `self.budget` is the orchestrator's `Arc<dyn BudgetEnforcerHandle>` (already a field added in M5-01).

- [ ] **Step 2: Add `append_post_messages` free function.**

  At the bottom of `conversation.rs` (outside any `impl`), add:
  ```rust
  fn append_post_messages(result: &mut lingxi_tools::ToolCallResult, msgs: &[String]) {
      if msgs.is_empty() {
          return;
      }
      let joined = msgs.join("\n");
      // If result.data is an object with a "text" field, append. Otherwise
      // wrap into { "data": <original>, "additional_context": <joined> }.
      match &mut result.data {
          serde_json::Value::Object(map) => {
              if let Some(text_val) = map.get_mut("text") {
                  if let Some(s) = text_val.as_str() {
                      *text_val = serde_json::Value::String(format!("{s}\n{joined}"));
                  } else {
                      map.insert("additional_context".into(), serde_json::Value::String(joined));
                  }
              } else {
                  map.insert("additional_context".into(), serde_json::Value::String(joined));
              }
          }
          other => {
              let orig = std::mem::replace(other, serde_json::Value::Null);
              *other = serde_json::json!({
                  "data": orig,
                  "additional_context": joined,
              });
          }
      }
  }
  ```

- [ ] **Step 3: Reroute `run_turn`'s tool-dispatch call site.**

  Find the line in `run_turn` (added by M5-02) that reads `let result = self.tools.invoke(name, input, ...).await?;` (or `self.invoke_tool(...)`). Replace it with:
  ```rust
  let result = self.dispatch_tool_with_hooks(name, input, tool_use_id.clone()).await?;
  ```

- [ ] **Step 4: Add the deny-hook integration test.**

  Write `lingxi-core/crates/orchestrator/tests/orchestrator_pre_post_hook_test.rs`:
  ```rust
  //! Orchestrator end-to-end with a scripted PreToolUse hook that denies Bash.

  use async_trait::async_trait;
  use lingxi_hooks::events::HookEvent;
  use lingxi_hooks::registry::{HookContext, HookRegistry};
  use lingxi_hooks::definition::{HookDefinition, HookExecutor, HookSource};
  use lingxi_hooks::executor::{BuiltinHookHandler, HookExecutorImpl};
  use lingxi_hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
  use lingxi_hooks::HookEventType;
  use lingxi_orchestrator::test_support::{build_test_orchestrator, scripted_assistant_turn_calling_bash};
  use lingxi_protocol::HookId;
  use std::sync::Arc;
  use tokio::sync::RwLock;

  struct DenyBashHandler;
  #[async_trait]
  impl BuiltinHookHandler for DenyBashHandler {
      async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
          match event {
              HookEvent::PreToolUse { tool_name, .. } if tool_name == "Bash" => HookResult {
                  outcome: HookOutcome::Success,
                  stdout: String::new(),
                  stderr: String::new(),
                  exit_code: None,
                  response: Some(HookResponse {
                      decision: Some(HookDecision::Block),
                      reason: Some("Bash is not allowed in this session".into()),
                      ..HookResponse::default()
                  }),
              },
              _ => HookResult {
                  outcome: HookOutcome::Success, stdout: String::new(), stderr: String::new(),
                  exit_code: None, response: None,
              },
          }
      }
      fn id(&self) -> &str { "deny-bash" }
  }

  #[tokio::test]
  async fn pre_tool_use_hook_blocks_bash_with_byte_locked_reason() {
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(HookDefinition {
          id: HookId::new(),
          name: "deny-bash".into(),
          events: vec![HookEventType::PreToolUse],
          if_condition: None,
          executor: HookExecutor::Builtin { handler_id: "deny-bash".into() },
          source: HookSource::User,
          blocking: true,
          timeout: None,
          priority: 0,
      });
      let orch = build_test_orchestrator()
          .with_hook_registry(registry)
          .with_builtin_hook(Arc::new(DenyBashHandler))
          .with_scripted_assistant(scripted_assistant_turn_calling_bash("rm -rf /"))
          .build();

      let outcome = orch.run_turn("hi".into()).await.expect("turn returns");
      // The tool result for the blocked Bash call must carry the deny reason
      // as `is_error: true` content.
      let last_tool_result = outcome
          .messages
          .iter()
          .filter_map(|m| m.tool_result_text())
          .last()
          .unwrap();
      assert!(last_tool_result.contains("Error: permission denied: Bash is not allowed"),
              "expected denied bash reason, got: {last_tool_result}");
  }
  ```

  **NOTE:** `build_test_orchestrator`, `scripted_assistant_turn_calling_bash`, `with_hook_registry`, `with_builtin_hook`, `with_scripted_assistant`, and `.build()` are existing test-support helpers introduced in M5-02 + M5-04. If `with_hook_registry`/`with_builtin_hook` don't yet exist, add them as straight-through setters on the orchestrator builder (Task 14 step 4a inside the `lingxi-orchestrator/src/test_support.rs` file — 6 lines, no logic, just `self.hook_registry = Some(r); self`). If you cannot find these helpers, `grep -n "build_test_orchestrator\|TestOrchestratorBuilder" lingxi-core/crates/orchestrator/src/test_support.rs` and adapt — the M5-04 plan installed the streaming variant of these helpers.

- [ ] **Step 5: Add three telemetry emit helpers (stubs for now — full bodies land in T16).**

  Append to `impl ConversationOrchestrator`:
  ```rust
  async fn emit_hook_pre_started(&self, _tool: &str, _ctx: &lingxi_hooks::HookContext) {
      // Wired in T16.
  }
  async fn emit_hook_pre_completed(&self, _tool: &str, _decision: &str, _dur_ms: u64) {
      // Wired in T16.
  }
  async fn emit_hook_post_started(&self, _tool: &str, _ctx: &lingxi_hooks::HookContext) {
      // Wired in T16.
  }
  async fn emit_hook_post_completed(&self, _tool: &str, _dur_ms: u64, _mutated: bool) {
      // Wired in T16.
  }
  ```

- [ ] **Step 6: Run tests.**

  ```bash
  cargo test -p lingxi-orchestrator --test orchestrator_pre_post_hook_test --quiet
  cargo test -p lingxi-orchestrator --quiet
  ```

  Expected: all green.

- [ ] **Step 7: Run M4-05 invariant tests again.**

  ```bash
  cargo test -p lingxi-tools --test agent_tool_recursion_lock_test --quiet
  cargo test -p lingxi-tools --test agent_tool_budget_inheritance_test --quiet
  ```

  Expected: green.

- [ ] **Step 8: Commit.**

  ```bash
  git add lingxi-core/crates/orchestrator/src/conversation.rs \
          lingxi-core/crates/orchestrator/tests/orchestrator_pre_post_hook_test.rs
  git commit -m "feat(M5-06 T14): wire PreToolUse hook into orchestrator dispatch — deny path returns ToolError::PermissionDenied"
  ```

---

### Task 15: Orchestrator PostToolUse wiring + response mutation test

**Files:**
- Modify: `lingxi-core/crates/orchestrator/tests/orchestrator_pre_post_hook_test.rs` (add a Post test).

- [ ] **Step 1: Add an append-context PostToolUse test.**

  Append to `orchestrator_pre_post_hook_test.rs`:
  ```rust
  struct AppendContextHandler;
  #[async_trait]
  impl BuiltinHookHandler for AppendContextHandler {
      async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
          match event {
              HookEvent::PostToolUse { tool_name, .. } if tool_name == "Bash" => HookResult {
                  outcome: HookOutcome::Success, stdout: String::new(), stderr: String::new(),
                  exit_code: None,
                  response: Some(HookResponse {
                      system_message: Some("post-hook annotation".into()),
                      ..HookResponse::default()
                  }),
              },
              _ => HookResult {
                  outcome: HookOutcome::Success, stdout: String::new(), stderr: String::new(),
                  exit_code: None, response: None,
              },
          }
      }
      fn id(&self) -> &str { "append-context" }
  }

  #[tokio::test]
  async fn post_tool_use_hook_appends_system_message_to_tool_result() {
      let registry = Arc::new(RwLock::new(HookRegistry::new()));
      registry.write().await.insert(HookDefinition {
          id: HookId::new(),
          name: "append".into(),
          events: vec![HookEventType::PostToolUse],
          if_condition: None,
          executor: HookExecutor::Builtin { handler_id: "append-context".into() },
          source: HookSource::User,
          blocking: true, timeout: None, priority: 0,
      });
      let orch = build_test_orchestrator()
          .with_hook_registry(registry)
          .with_builtin_hook(Arc::new(AppendContextHandler))
          .with_scripted_assistant(scripted_assistant_turn_calling_bash("echo hi"))
          .with_scripted_bash_output("hi\n")
          .build();

      let outcome = orch.run_turn("hi".into()).await.unwrap();
      let tool_result_text = outcome
          .messages
          .iter()
          .filter_map(|m| m.tool_result_text())
          .last()
          .unwrap();
      // `hi\n` + `\npost-hook annotation` joined into the text field OR
      // an additional_context key added — either is acceptable byte-lock.
      assert!(tool_result_text.contains("post-hook annotation"),
              "expected post-hook annotation in tool result, got: {tool_result_text}");
  }
  ```

  **NOTE:** `with_scripted_bash_output` is an existing M5-02 / M4-02 helper that pre-populates the `BashTool`'s stub output. If it does not exist with that exact name, use whatever the test-support module names the helper that lets you script `Bash` tool returns (e.g. `with_scripted_tool_output("Bash", "hi\n")`).

- [ ] **Step 2: Run tests.**

  ```bash
  cargo test -p lingxi-orchestrator --test orchestrator_pre_post_hook_test --quiet
  ```

  Expected: 2 passed.

- [ ] **Step 3: Commit.**

  ```bash
  git add lingxi-core/crates/orchestrator/tests/orchestrator_pre_post_hook_test.rs
  git commit -m "feat(M5-06 T15): wire PostToolUse hook into orchestrator — system_message appended to tool result"
  ```

---

### Task 16: Telemetry — 8 new event constants + parity bump (245 → 253)

**Files:**
- Modify: `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` (append 8 constants + payloads + extend `NAMES` to 15 entries).
- Modify: `lingxi-core/crates/telemetry/src/tengu/mod.rs:29` (bump orchestrator count `7` → `15`).
- Modify: `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs` (bump assertion 245 → 253).
- Modify: `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` (insert 8 names after `tengu_orchestrator_permission_answered`).
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs` (fill the 4 emit-helper bodies + add 2 emit helpers `emit_hook_http_skipped_ssrf` / `emit_hook_timeout`).
- Modify: `lingxi-core/crates/hooks/src/executor.rs` (replace `tracing::warn!` calls in `emit_ssrf_skip` / `emit_timeout` with real telemetry emission via the new `telemetry` field).
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_hook_telemetry_test.rs`.

- [ ] **Step 1: Append 8 constants + payloads to `tengu/orchestrator.rs`.**

  Open `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs`. After the existing 7 constants (`CONVERSATION_STARTED/COMPLETED/FAILED`, `TURN_STREAMING_STARTED/COMPLETED`, `PERMISSION_PROMPTED/ANSWERED`), append:
  ```rust
  /// `tengu_orchestrator_hook_pre_started` — fired before PreToolUse hook execution.
  pub const HOOK_PRE_STARTED: &str = "tengu_orchestrator_hook_pre_started";
  /// `tengu_orchestrator_hook_pre_completed` — fired after PreToolUse hook execution.
  pub const HOOK_PRE_COMPLETED: &str = "tengu_orchestrator_hook_pre_completed";
  /// `tengu_orchestrator_hook_pre_failed` — fired when a PreToolUse hook errored.
  pub const HOOK_PRE_FAILED: &str = "tengu_orchestrator_hook_pre_failed";
  /// `tengu_orchestrator_hook_post_started` — fired before PostToolUse hook execution.
  pub const HOOK_POST_STARTED: &str = "tengu_orchestrator_hook_post_started";
  /// `tengu_orchestrator_hook_post_completed` — fired after PostToolUse hook execution.
  pub const HOOK_POST_COMPLETED: &str = "tengu_orchestrator_hook_post_completed";
  /// `tengu_orchestrator_hook_post_failed` — fired when a PostToolUse hook errored.
  pub const HOOK_POST_FAILED: &str = "tengu_orchestrator_hook_post_failed";
  /// `tengu_orchestrator_hook_http_skipped_ssrf` — fired when SsrfGuard rejected an HTTP hook URL.
  pub const HOOK_HTTP_SKIPPED_SSRF: &str = "tengu_orchestrator_hook_http_skipped_ssrf";
  /// `tengu_orchestrator_hook_timeout` — fired when any hook arm hit its timeout.
  pub const HOOK_TIMEOUT: &str = "tengu_orchestrator_hook_timeout";
  ```

  Append corresponding payload structs (each is a tiny `#[derive(Serialize)]` mirroring the payload-fields column of the lock table):
  ```rust
  #[derive(serde::Serialize)]
  pub struct HookPreStartedPayload<'a> {
      pub tool_name: &'a str,        // PiiTagged
      pub hook_kind: &'a str,        // Verified
  }
  #[derive(serde::Serialize)]
  pub struct HookPreCompletedPayload<'a> {
      pub tool_name: &'a str,        // PiiTagged
      pub decision: &'a str,         // Verified
      pub duration_ms: u64,          // Verified
  }
  #[derive(serde::Serialize)]
  pub struct HookPreFailedPayload<'a> {
      pub tool_name: &'a str,
      pub hook_id: &'a str,
      pub reason: &'a str,
  }
  // (Same shape pattern for HookPostStartedPayload / HookPostCompletedPayload /
  //  HookPostFailedPayload / HookHttpSkippedSsrfPayload / HookTimeoutPayload.)
  ```

  Then update the `NAMES` slice. Find:
  ```rust
  pub const NAMES: &[&str] = &[
      CONVERSATION_STARTED, CONVERSATION_COMPLETED, CONVERSATION_FAILED,
      TURN_STREAMING_STARTED, TURN_STREAMING_COMPLETED,
      PERMISSION_PROMPTED, PERMISSION_ANSWERED,
  ];
  ```
  Extend to 15 entries:
  ```rust
  pub const NAMES: &[&str] = &[
      CONVERSATION_STARTED, CONVERSATION_COMPLETED, CONVERSATION_FAILED,
      TURN_STREAMING_STARTED, TURN_STREAMING_COMPLETED,
      PERMISSION_PROMPTED, PERMISSION_ANSWERED,
      HOOK_PRE_STARTED, HOOK_PRE_COMPLETED, HOOK_PRE_FAILED,
      HOOK_POST_STARTED, HOOK_POST_COMPLETED, HOOK_POST_FAILED,
      HOOK_HTTP_SKIPPED_SSRF, HOOK_TIMEOUT,
  ];
  ```

- [ ] **Step 2: Bump the orchestrator count in `tengu/mod.rs`.**

  Open `lingxi-core/crates/telemetry/src/tengu/mod.rs:29`. The current `TOTAL` formula reads:
  ```rust
  pub const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 7 + 1;
  ```
  Replace with:
  ```rust
  pub const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 15 + 1;
  ```
  Total: 253.

- [ ] **Step 3: Bump the completeness test.**

  Open `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs`. Find the `registry_is_exactly_245_entries` test added by M5-05. Rename to `registry_is_exactly_253_entries`, update the assertion `assert_eq!(lingxi_telemetry::tengu::ALL_EVENT_NAMES.len(), 253);`, update the comment to mention M5-06.

- [ ] **Step 4: Insert 8 names into the parity fixture.**

  Open `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`. Locate `"tengu_orchestrator_permission_answered"` (last entry from the M5-05 batch). Insert immediately after it (and before the next category):
  ```json
  "tengu_orchestrator_hook_pre_started",
  "tengu_orchestrator_hook_pre_completed",
  "tengu_orchestrator_hook_pre_failed",
  "tengu_orchestrator_hook_post_started",
  "tengu_orchestrator_hook_post_completed",
  "tengu_orchestrator_hook_post_failed",
  "tengu_orchestrator_hook_http_skipped_ssrf",
  "tengu_orchestrator_hook_timeout",
  ```

- [ ] **Step 5: Fill the orchestrator emit helpers.**

  Replace the 4 stubs in `conversation.rs` with real emissions, and add 2 more (for SSRF/timeout although those are also emitted at the executor level — the orchestrator emits them in case the SSRF/timeout happens inside `hooks.execute` and surfaces via the aggregate result's `all_results[i].outcome == Timeout`):
  ```rust
  use lingxi_telemetry::tengu::orchestrator as orch_events;
  use lingxi_telemetry::audit::{emit_event, PiiTagged, Verified};

  async fn emit_hook_pre_started(&self, tool: &str, _ctx: &lingxi_hooks::HookContext) {
      let payload = orch_events::HookPreStartedPayload {
          tool_name: tool,
          hook_kind: "any",
      };
      emit_event(&self.telemetry, orch_events::HOOK_PRE_STARTED, &payload).await;
  }
  async fn emit_hook_pre_completed(&self, tool: &str, decision: &str, dur_ms: u64) {
      let payload = orch_events::HookPreCompletedPayload {
          tool_name: tool,
          decision,
          duration_ms: dur_ms,
      };
      emit_event(&self.telemetry, orch_events::HOOK_PRE_COMPLETED, &payload).await;
  }
  async fn emit_hook_post_started(&self, tool: &str, _ctx: &lingxi_hooks::HookContext) {
      let payload = orch_events::HookPostStartedPayload {
          tool_name: tool,
          hook_kind: "any",
      };
      emit_event(&self.telemetry, orch_events::HOOK_POST_STARTED, &payload).await;
  }
  async fn emit_hook_post_completed(&self, tool: &str, dur_ms: u64, mutated: bool) {
      let payload = orch_events::HookPostCompletedPayload {
          tool_name: tool,
          duration_ms: dur_ms,
          mutated_response: mutated,
      };
      emit_event(&self.telemetry, orch_events::HOOK_POST_COMPLETED, &payload).await;
  }
  ```

  Replace the executor's `tracing::warn!` calls (`emit_ssrf_skip` + `emit_timeout`) with real emissions. First, add a `telemetry: Arc<dyn TelemetryEmitter>` field to `HookExecutorImpl` plus a `with_telemetry(telemetry: Arc<dyn TelemetryEmitter>) -> Self` builder method (initialise to a no-op emitter in `new()` via `Arc::new(NoopTelemetry)` from `lingxi-telemetry`). Then:
  ```rust
  async fn emit_ssrf_skip(&self, hook: &HookDefinition, url: &str, reason: &str) {
      let payload = lingxi_telemetry::tengu::orchestrator::HookHttpSkippedSsrfPayload {
          hook_id: &hook.id.to_string(),
          url,            // _PROTO_url PiiTagged
          reason,
      };
      lingxi_telemetry::audit::emit_event(
          &self.telemetry,
          lingxi_telemetry::tengu::orchestrator::HOOK_HTTP_SKIPPED_SSRF,
          &payload,
      ).await;
  }
  async fn emit_timeout(&self, hook: &HookDefinition, kind: &'static str) {
      let payload = lingxi_telemetry::tengu::orchestrator::HookTimeoutPayload {
          tool_name: "<n/a>",     // arm-level event, no tool yet
          hook_id: &hook.id.to_string(),
          hook_kind: kind,
          timeout_ms: 0,           // placeholder when caller doesn't pass it
      };
      lingxi_telemetry::audit::emit_event(
          &self.telemetry,
          lingxi_telemetry::tengu::orchestrator::HOOK_TIMEOUT,
          &payload,
      ).await;
  }
  ```

  The orchestrator's constructor now calls `.with_telemetry(self.telemetry.clone())` when building the `HookExecutorImpl`.

- [ ] **Step 6: Write the telemetry-capture integration test.**

  Write `lingxi-core/crates/orchestrator/tests/orchestrator_hook_telemetry_test.rs`:
  ```rust
  //! Capture the 8 new hook telemetry events on a deny-then-allow turn.

  use lingxi_orchestrator::test_support::{build_test_orchestrator, RecordingTelemetry, scripted_assistant_turn_calling_bash};
  use lingxi_telemetry::tengu::orchestrator as orch_events;
  use std::sync::Arc;

  #[tokio::test]
  async fn pre_post_hook_pair_emits_started_completed_in_order() {
      let recorder = Arc::new(RecordingTelemetry::default());
      let orch = build_test_orchestrator()
          .with_telemetry(recorder.clone())
          .with_scripted_assistant(scripted_assistant_turn_calling_bash("echo hi"))
          .with_scripted_bash_output("hi\n")
          .build();
      orch.run_turn("hi".into()).await.unwrap();
      let names = recorder.event_names();
      let pre_start = names.iter().position(|n| n == orch_events::HOOK_PRE_STARTED);
      let pre_done  = names.iter().position(|n| n == orch_events::HOOK_PRE_COMPLETED);
      let post_start = names.iter().position(|n| n == orch_events::HOOK_POST_STARTED);
      let post_done = names.iter().position(|n| n == orch_events::HOOK_POST_COMPLETED);
      assert!(pre_start < pre_done);
      assert!(pre_done < post_start);
      assert!(post_start < post_done);
  }
  ```

  `RecordingTelemetry` is the existing M5-02 / M5-04 test helper that records `(name, payload_json)` pairs. If it doesn't yet expose `event_names()`, add it as a thin accessor.

- [ ] **Step 7: Run all the bumped tests.**

  ```bash
  cargo test -p lingxi-telemetry --test event_name_completeness_test --quiet
  cargo test -p lingxi-orchestrator --test orchestrator_hook_telemetry_test --quiet
  cargo test --workspace --quiet
  ```

  Expected: all green. The parity tests (which check `tengu_events.json` against `ALL_EVENT_NAMES` exact-equality with order) verify the 8 new names landed in the correct slot.

- [ ] **Step 8: Run clippy + fmt workspace-wide.**

  ```bash
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --check
  ```

- [ ] **Step 9: Commit.**

  ```bash
  git add lingxi-core/crates/telemetry/src/tengu/orchestrator.rs \
          lingxi-core/crates/telemetry/src/tengu/mod.rs \
          lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs \
          lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json \
          lingxi-core/crates/orchestrator/src/conversation.rs \
          lingxi-core/crates/hooks/src/executor.rs \
          lingxi-core/crates/orchestrator/tests/orchestrator_hook_telemetry_test.rs
  git commit -m "feat(M5-06 T16): 8 hook telemetry events + parity fixture bump (245 → 253)"
  ```

---

### Task 17: Verification gate

**Files:** none — runs the full workspace.

- [ ] **Step 1: Workspace tests.**

  ```bash
  cargo test --workspace --quiet
  ```

  Expected: all green. If anything fails, fix the root cause; do NOT skip the test.

- [ ] **Step 2: Workspace clippy.**

  ```bash
  cargo clippy --workspace --all-targets -- -D warnings
  ```

  Expected: clean.

- [ ] **Step 3: Workspace fmt.**

  ```bash
  cargo fmt --check
  ```

  Expected: clean.

- [ ] **Step 4: Assert `ALL_EVENT_NAMES.len() == 253`.**

  ```bash
  cargo test -p lingxi-telemetry --test event_name_completeness_test --quiet
  ```

  Expected: 1 passed.

- [ ] **Step 5: Assert M4-05 invariants still hold.**

  ```bash
  cargo test -p lingxi-tools --test agent_tool_recursion_lock_test --quiet
  cargo test -p lingxi-tools --test agent_tool_budget_inheritance_test --quiet
  ```

  Expected: both green.

- [ ] **Step 6: Grep for forbidden placeholders.**

  ```bash
  grep -rn "unimplemented!\|todo!\|panic!.*stub\|TODO\|FIXME" \
      lingxi-core/crates/hooks/src/ \
      lingxi-core/crates/orchestrator/src/conversation.rs \
      | grep -v -E '(^|/)(tests?)\.rs:' | grep -v "Wired in T" | grep -v "// Filled in"
  ```

  Expected: zero matches. The `grep -v` guards remove the two acceptable artefacts (the T11→T12 progression markers in tests and the "Filled in TXX" inline notes — these are descriptive comments, not placeholders).

- [ ] **Step 7: Tag the release.**

  ```bash
  git tag -a m5.6 -m "M5-06 — hooks runtime 4-arm + PreToolUse/PostToolUse wiring + 8 telemetry events"
  ```

- [ ] **Step 8: No commit — verification gate is read-only.**

  This task produces no new artefacts beyond the tag.

---

## Self-review checklist

1. **Spec coverage:** §3 M5-06 row (4-arm + Pre/Post + 8 telemetry events + ~17 tasks) → T1-T16 cover all four arms (Builtin already shipped in M1.4, Http T3-T6, Command T7-T10, Agent T11-T13) and the orchestrator wiring (T14-T15). 8 telemetry events appear in T16. Task count = 17 (T0-T16). §4.5 byte-locks all captured in the "Reverse-engineered byte-locks" table. OQ-4 resolved by T0+T1 producing the canonical Rust schemas.
2. **Placeholder scan:** zero `unimplemented!()`/`todo!()` in production code. Comments like "Wired in T16" mark intentional progress steps where T14 ships stubs that T16 fills — each stub has a complete (no-op or tracing-only) body that compiles and passes its task's assertions. Verified by T17 step 6.
3. **Type consistency:** `HookResult`, `HookExecutorImpl`, `HookEvent`, `HookDefinition`, `AggregateHookResult`, `HookContext`, `SubagentInheritance`, `SubagentSpawner`, `RuntimeSpawner`, `HttpTransport` are all referenced with their canonical names from M1.4 / M4-05 / M3-03. New types — `HttpExecutor`, `CommandExecutor`, `AgentExecutor`, `PreToolUsePayload`, `PostToolUsePayload`, `HookEventEnvelope`, `HookEventNamePre`, `HookEventNamePost`, `HookResponseParseError`, `HttpExecutionSignal`, `CommandExecutionSignal`, `AgentExecutionSignal`, and their `…Outcome` siblings — are defined once and used consistently across tests + production calls.
4. **Telemetry chain:** 245 (post-M5-05) + 8 (M5-06) = 253. Reconciled in T16 step 2 (`TOTAL` formula) + T16 step 3 (`event_name_completeness_test.rs` assertion) + T16 step 4 (parity fixture). T17 step 4 re-asserts.
5. **No `lingxi-hooks` → `lingxi-orchestrator` cycle:** T14 places `dispatch_tool_with_hooks` inside the orchestrator crate, calling DOWN into `lingxi-hooks` via the existing `Arc<HookExecutorImpl>` field. `lingxi-hooks` continues to depend on `lingxi-traits` (for `SubagentSpawner`, `HttpTransport`, `RuntimeSpawner`) — never on the orchestrator. `cargo tree -p lingxi-hooks | grep orchestrator` must return zero matches; T17 implicitly validates by `cargo build --workspace`.
6. **M4-05 `Arc::ptr_eq` tests not regressed:** T13 step 3 + T14 step 7 + T17 step 5 explicitly re-run `agent_tool_recursion_lock_test` and `agent_tool_budget_inheritance_test`. The `EchoSpawner` in T13 directly asserts pointer equality with the orchestrator's parent `Arc`s.

---

## Final commit (plan document itself)

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add docs/superpowers/plans/2026-05-25-m5-06-hooks-runtime.md
git commit -m "$(cat <<'EOF'
plan(M5-06): hooks runtime 4-arm (Builtin/Http/Command/Agent) + orchestrator integration — 17 TDD tasks

EOF
)"
```
