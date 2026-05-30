# LingXi Core M5 · Plan 01 · Engine wiring close-out (runner pump + TaskOutput spool + RegistryToolInvoker route)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Close out the three v0.5.0 follow-ups that were intentionally stubbed in M4-05's wiring sprint, **without regressing the two `Arc::ptr_eq` invariants** that anchor recursive-subagent dispatch. After this plan, the M4-05 wiring path is no longer half-stubbed: a real subagent driven by `lingxi_core::reduce` runs to terminal state and emits `SubagentEvent` faithfully; `TaskRegistryHandle::output` reads the actual spool file via `TaskOutputManager::read` (not the empty placeholder); and `RegistryToolInvoker::invoke` dispatches into `ToolRegistry`-resident tools (not `Ok(Value::Null)` after a `find_by_name` probe). Tag `m5.1` at the end. **No new telemetry events. No new byte-locks. The 134-entry `tengu::tool::NAMES` count and 238-entry `ALL_EVENT_NAMES` count stay frozen.** This is the smallest M5 sub-plan: ~12 TDD tasks.

**Architecture:** Three surfaces are touched, all under `lingxi-code/crates/`:

- **Follow-up A — `lingxi-agent::runner::run_subagent`** (file `lingxi-code/crates/agent/src/runner.rs`). The current body is a two-line stub:
  ```rust
  let _ = event_rx.recv().await;
  let _ = out_tx.send(SubagentEvent::Completed { agent_id, result: json!({"stub": true}) }).await;
  ```
  This is replaced with a real reduce loop. The runner seeds an initial `ConversationState::Idle { session }` from `SubagentContext` (the `agent_id` lives on `SubagentContext`; a fresh `SessionState::empty(SessionId::nil(), <inherited-model>)` is built where `<inherited-model>` is taken from `ctx.agent_definition.model` mapped via the existing `lingxi_agent::definition::AgentModel`-to-string projection). The loop pulls events off `event_rx` and folds them through `lingxi_core::reduce(state, event)`. On each transition the runner inspects the new state + emitted effects and translates them to `SubagentEvent` shapes on `out_tx`:
  - `Event::ApiStreamEnd { final_message, .. }` while in `StreamingResponse` → emit `SubagentEvent::Message { agent_id, message: serde_json::to_value(&final_message).unwrap_or(Value::Null) }`, then continue.
  - Any transition to `ConversationState::Terminated { reason, session }` → emit `SubagentEvent::Completed { agent_id, result: json!({ "reason": reason, "history_len": session.history.len() }) }` and break.
  - A reducer call that yields a `lingxi_protocol::Effect::Kill` (placeholder name; in practice we look for the `UserInterrupt` or `UserExit` input event that drove termination — see Task 4) → emit `SubagentEvent::Killed { agent_id }` and break.
  - `event_rx` closed (returns `None`) before reaching `Terminated` → emit `SubagentEvent::Failed { agent_id, error: "run_subagent: event channel closed without terminal state".into() }` and break.

  The runner does NOT issue any I/O of its own — it remains a pure transformer between `lingxi_core::Event` (in) and `SubagentEvent` (out), preserving D17 (purity). All effect-side I/O remains the host's responsibility via the existing `lingxi_protocol::Effect` channel (out of scope for this plan; `Vec<Effect>` returned by `reduce` is dropped — the host will wire it in M5-02 when the orchestrator drives the runner).

- **Follow-up B — `TaskRegistryHandle::output`** (file `lingxi-code/crates/tasks/src/handle.rs`, the `impl TaskRegistryHandle for TaskRegistry` block at line 209). Currently:
  ```rust
  async fn output(&self, id: &str, _offset: Option<u64>) -> Result<TaskOutputChunk, TaskRegistryError> {
      let state = self.get(id).await.ok_or_else(|| TaskRegistryError::NotFound(id.into()))?;
      Ok(TaskOutputChunk { task_id: state.base().id.clone(), content: String::new(), total_lines: 0, truncated: false })
  }
  ```
  Replaced with a real spool read: look up the task state, pull `state.base().output_file: PathBuf`, call `self.output_manager.read(&output_file, OutputOptions { offset, limit: None })` (the manager already exists at `lingxi-code/crates/tasks/src/output_manager.rs:83`), and map the resulting `TaskOutput { content, total_lines, truncated }` into a `TaskOutputChunk { task_id, content, total_lines, truncated }`. The `offset` argument that has been arriving as `_offset` is now THREADED through as `OutputOptions { offset: _offset, limit: None }`. On `OutputError::Io(s)` from the manager → `TaskRegistryError::Internal(format!("io: {s}"))`; on `OutputError::PathEscape(p)` → `TaskRegistryError::Internal(format!("path escape: {p}"))` (the latter is defensive — `TaskRegistry::create` already enforces sandbox containment on allocation).

- **Follow-up C — `RegistryToolInvoker::invoke`** (file `lingxi-code/crates/tools/src/tool_invoker_impl.rs`, the `#[async_trait] impl ToolInvoker for RegistryToolInvoker` block at line 38). Currently:
  ```rust
  async fn invoke(&self, name: &str, _input: Value, _ctx: SubagentInvocationContext) -> Result<Value, ToolInvokerError> {
      self.registry.find_by_name(name).ok_or_else(|| ToolInvokerError::NotFound(name.to_string()))?;
      Ok(Value::Null)
  }
  ```
  Replaced with a real dispatch: locate the tool via `find_by_name`, then synthesize a minimal `ToolUseContext` from `SubagentInvocationContext` (only `agent_id` is supplied by the trait — every other field is filled from `ToolUseContext::default()` semantics: empty messages, no session, `subagent_registry: Some(self.registry.clone())` so a recursive `AgentTool` invocation from inside the called tool reuses the same registry Arc), spawn a single-shot `ToolProgressSender` channel (drained immediately so progress events do not deadlock the caller), and invoke `tool.call(input, tool_use_ctx, progress_tx).await`. On `Ok(result)` → return `result.data`. On `Err(ToolError::InvalidInput(s))` → `ToolInvokerError::InvalidInput(s)`. On any other `ToolError` → `ToolInvokerError::Internal(format!("{e}"))`. **The `Arc<ToolRegistry>` field is reused verbatim** — no fresh `Arc::new(ToolRegistry::new())` is constructed, so the M4-05 `Arc::ptr_eq(parent_registry, invoker.registry_arc())` invariant continues to hold.

The unifying constraint binding all three follow-ups: the two **critical `Arc::ptr_eq` tests** in `lingxi-code/crates/tools/src/builtin/agent.rs::tests`:
- `recursion_lock_child_inherits_parent_tool_registry_arc` (line 455) — asserts the child subagent's invoker wraps the same `Arc<ToolRegistry>` as the parent.
- `budget_inheritance_child_inherits_parent_budget_arc` (line 508) — asserts the child subagent's `Arc<dyn BudgetEnforcerHandle>` is the parent's Arc.

These tests use a `MockSubagentSpawner` (in `agent_test_support.rs`) that captures the `SubagentInheritance` bundle; the production spawner path is NOT exercised by them, so M5-01's changes to `run_subagent` cannot regress them by construction. The risk window is Follow-up C: if the new `RegistryToolInvoker::invoke` body somehow reaches into the registry and rebuilds the `Arc` (e.g. by going through `find_by_name(...).map(Arc::clone(...))` and then **storing a different inner Arc**), the recursion-lock test would still pass (it inspects the field, not the dispatch path), but a downstream test exercising the dispatch path could fail. Task 11 explicitly re-runs both critical tests after Follow-up C lands.

Per spec §6.2 (backward compatibility): every M4-05 test must remain green. Per spec §3 M5-01 row: 0 new telemetry events, 0 new byte-locks. Per spec §4 (not applicable — M5-01 introduces no new byte-locks).

**Tech Stack:** Rust 2021, `async-trait 0.1` (workspace), `serde 1` + `serde_json 1` (workspace, `serde_json` with `preserve_order` workspace feature), `tokio 1` (workspace; `sync::mpsc` for the channel-based pump), `thiserror 1` (workspace). Crate-level deps already present from M4-01..09: every dep this plan touches is already declared in the respective `Cargo.toml`s. **No new deps added.** No new crate is created.

**References:**
- Spec: `docs/superpowers/specs/2026-05-25-m5-conversational-agent-loop-design.md` (committed at `1dbb9b8`).
  - Header + §1 Goal (lines 1-50) — v0.6.0 conversational agent loop; M5-01 wires the foundations that M5-02 (orchestrator) immediately needs.
  - §3 Sub-plan table M5-01 row (line 177) — three follow-ups: `(a)` runner stub → real reduce loop, `(b)` `TaskRegistryHandle::output` real spool read, `(c)` `RegistryToolInvoker::invoke` real dispatch. 0 new events. ~12 tasks.
  - §6.2 backward compatibility — M4-05 wiring `Arc::ptr_eq` tests must stay green throughout M5.
- Predecessor (M4-05 wiring follow-up):
  - `lingxi-code/crates/agent/src/runner.rs` (commit baseline — the 2-line stub body at lines 64-70).
  - `lingxi-code/crates/agent/src/pool.rs` (lines 57-85, `StateMachinePool::allocate` — the channel pair `event_tx / event_rx` and `out_tx / out_rx` are owned by the pool; `run_subagent` consumes `event_rx` and produces on `out_tx`).
  - `lingxi-code/crates/agent/src/context.rs` (full file — `SubagentContext` carries `agent_id`, `agent_definition`, `prompt_messages`, plus a dozen optional fields; M5-01 reads `agent_id` and `agent_definition.model` only).
  - `lingxi-code/crates/core/src/lib.rs` (lines 14-28) + `events.rs` (full file) + `reducer.rs` (`reduce(state, event) -> (new_state, effects)` at line 15) + `state_machine.rs` (5-variant `ConversationState`; `Terminated` is the absorbing terminal).
  - `lingxi-code/crates/traits/src/subagent_spawn.rs` — `SubagentSpawner` trait + `SubagentInheritance { tool_invoker: Arc<dyn ToolInvoker>, budget: Arc<dyn BudgetEnforcerHandle> }`.
  - `lingxi-code/crates/traits/src/tool_invoker.rs` — `ToolInvoker::invoke(name, input, ctx)` + `SubagentInvocationContext { parent_agent_id }` + `ToolInvokerError::{NotFound, InvalidInput, Internal}`.
  - `lingxi-code/crates/traits/src/task_registry.rs` — `TaskRegistryHandle::output(id, offset) -> Result<TaskOutputChunk, TaskRegistryError>`.
  - `lingxi-code/crates/tasks/src/registry.rs` (lines 26-44, `pub output_manager: Arc<TaskOutputManager>` is already on `TaskRegistry`).
  - `lingxi-code/crates/tasks/src/output_manager.rs` (lines 83-103, `TaskOutputManager::read(output_file, OutputOptions) -> Result<TaskOutput, OutputError>` is already implemented).
  - `lingxi-code/crates/tasks/src/handle.rs` (lines 209-228, current `output` stub).
  - `lingxi-code/crates/tools/src/tool_invoker_impl.rs` (lines 38-60, current `invoke` `NotFound`-only body).
  - `lingxi-code/crates/tools/src/builtin/agent.rs::tests::{recursion_lock_child_inherits_parent_tool_registry_arc, budget_inheritance_child_inherits_parent_budget_arc}` (lines 454-541) — the two critical Arc-identity tests.
- Repo conventions (M4-01..09 precedent):
  - Tests live in `#[cfg(test)] mod tests { ... }` blocks adjacent to the production code.
  - Integration tests live in `lingxi-code/crates/<crate>/tests/<name>_test.rs`.
  - Every error string visible in tests is the EXACT byte sequence in production (`assert_eq!`-quality, not `assert!(s.contains(...))`).
  - Telemetry constants are CAPITAL_SNAKE; production code references `lingxi_telemetry::tengu::tool::*` symbols, not literals. **M5-01 emits no telemetry — no new constants.**
  - Tool event-name suffix `_completed` (M3-06 lock) — irrelevant here (no new events).
- M4-05 wiring artefacts already in place (verified at `2026-05-25`):
  - `lingxi-code/crates/traits/src/tool_invoker.rs` — `ToolInvoker` trait + `SubagentInvocationContext` + `ToolInvokerError` (added in M4-05 wiring follow-up; commit `29dfe89` or later).
  - `lingxi-code/crates/traits/src/subagent_spawn.rs` — `SubagentSpawner` + `SubagentInheritance` (same commit).
  - `lingxi-code/crates/tools/src/tool_invoker_impl.rs::RegistryToolInvoker::registry_arc()` — pub accessor that returns `&Arc<ToolRegistry>` (used by the M4-05 critical tests via downcast through `as_any()`).
  - `lingxi-code/crates/tools/src/builtin/agent_test_support.rs` — `MockSubagentSpawner`, `MockBudgetEnforcerHandle`, `MockTaskRegistryHandle`, `MockMailboxRouterHandle`, plus `arc_*` helpers.
  - `lingxi-code/crates/tasks/src/registry.rs` — `TaskRegistry::new(runtime, fs, output_manager)`, `TaskRegistry::create(task_type, _input, description) -> Result<String, TaskError>` (the `_input` arg is intentionally unused; this is fine).
  - `lingxi-code/crates/tasks/src/output_manager.rs::TaskOutputManager::{allocate, read}` — already production-quality; M5-01 only adds CALL SITES, no new methods.
- M1 surfaces (still stable):
  - `lingxi_core::Event::{UserMessage, UserInterrupt, UserExit, ApiStreamStart, ApiStreamDelta, ApiStreamEnd, ApiError, SessionLoaded, CostRecorded, BudgetThresholdReached, BudgetExceeded, PermissionGranted, ...}` (see `events.rs` for the full enum).
  - `lingxi_core::ConversationState::{Idle, AssemblingPrompt, AwaitingApiResponse, StreamingResponse, Terminated}` + `is_terminal()` (line 68).
  - `lingxi_core::reduce(state, event) -> (ConversationState, Vec<Effect>)` (pure, total).
  - `lingxi_core::SessionState::empty(session_id, model) -> SessionState` (used by the runner to seed initial state).
  - `lingxi_protocol::SessionId::nil()` — placeholder session id for the subagent's local state (the runner does not persist; the orchestrator does).

---

## File touch inventory (locked at top per spec Appendix A convention)

**Creates (new files):** none. (M5-01 only modifies existing files.)

**Modifies (existing files):**

- `lingxi-code/crates/agent/src/runner.rs` — replace the 2-line stub body in `run_subagent` with the real reduce/pump loop (~60 LOC). Add a private helper `event_to_session_input(event: &lingxi_core::Event) -> Option<SubagentEvent>` only if needed; the current draft inlines the translation. Add a new `#[cfg(test)] mod tests { ... }` block at the bottom of the file with 3 unit tests (Tasks 2, 4, 5).
- `lingxi-code/crates/tasks/src/handle.rs` — replace the `output(...)` body (line 209-228) with the real spool read. Adjust the existing `output_returns_empty_for_freshly_created_task` test (line 380) into `output_returns_empty_for_freshly_created_task_with_zero_byte_spool` and add one new test `output_returns_real_content_after_spool_write` (Task 7).
- `lingxi-code/crates/tasks/src/handle.rs::tests::NoopFs` — extend `read_file` to actually walk the in-memory map seeded by `write_file` so the new spool-read test can observe non-empty content (Task 7 step 3). The existing tests that rely on `NoopFs` returning empty are not invalidated because they never call `output` on a task whose spool was written to.
- `lingxi-code/crates/tools/src/tool_invoker_impl.rs` — replace the `invoke(...)` body (lines 40-55) with the real dispatch. Add 2 new tests to the existing `#[cfg(test)] mod tests` block (Tasks 9, 11).
- `lingxi-code/crates/tools/tests/agent_task_integration_test.rs` — **NO modification** to the M4-05 fixture; M5-01 verifies its tests still pass (Task 5 step 4 + Task 11 step 2 + Task 12 step 1). If the integration suite is moved or extended in M4-05 wiring follow-up commits (post-`29dfe89`), this plan's verification gate STILL covers it via `cargo test --workspace`.

**Critical fidelity notes (locked here):**

- **Runner does NOT mutate the `SubagentContext`**: the context is owned-by-value (`ctx: SubagentContext`); cloning is cheap (Arc-wrapped heavy state) but the runner never re-emits the context. It reads `ctx.agent_id` and `ctx.agent_definition.model` to seed the initial `SessionState`, then drops the rest (M5-01 scope — the orchestrator in M5-02 will use the rest).
- **Reduce loop terminates on FIRST `Terminated`**: per `state_machine.rs` line 68, `Terminated` is absorbing. The runner's loop checks `state.is_terminal()` after each `reduce` call and exits.
- **Killed semantics**: the M1 reducer does not have a `Killed` terminal — it has `Terminated { reason: "..." }`. The runner inspects the `reason` string: if it starts with `"killed:"` (added by reducer logic if/when host calls `Event::UserInterrupt` or `Event::UserExit` — see `reducer.rs`), the runner emits `SubagentEvent::Killed`. Otherwise (any other terminal reason) it emits `SubagentEvent::Completed`. Task 4 hard-codes a `Event::UserExit` test and asserts `Killed` is emitted. **If the existing reducer maps `UserExit → Terminated { reason: "user_exit" }`** (we verify in Task 1), the runner predicate is `reason.starts_with("killed") || reason.starts_with("user_exit") || reason.starts_with("user_interrupt")`. Task 1 step 4 documents the exact reducer-side reason strings (read from `reducer.rs`); Task 3 step 3 hard-codes the predicate accordingly.
- **Channel-drop = Failed**: if `event_rx.recv().await` returns `None` (sender dropped) BEFORE reaching a terminal state, the runner emits `SubagentEvent::Failed { error: "run_subagent: event channel closed without terminal state" }`. This is the **byte-locked failure message** (Task 1 step 3 includes it in the test fixture).
- **`Effect`s dropped**: the reducer returns `(new_state, effects: Vec<Effect>)`. The runner discards `effects` — M5-01 does not wire effect-side I/O (that's M5-02 + M5-04). This is intentional and documented inline.
- **`RegistryToolInvoker::invoke` minimal `ToolUseContext`**: only `subagent_registry: Some(self.registry.clone())` matters for the recursion lock; everything else is `Default::default()` semantics (empty messages, no session, etc.). **The progress_tx channel is created LOCALLY** (`tokio::sync::mpsc::channel(8)`), with the receiver DROPPED immediately — production tools tolerate progress-receiver drop (see M4-01 wiring of `ToolProgressSender`).
- **`agent_id` plumbing**: `SubagentInvocationContext { parent_agent_id }` carries the dispatcher's agent id. We thread it onto the synthesized `ToolUseContext.agent_id` so downstream telemetry attributes the call to the correct agent. (No new telemetry — just plumbing.)

---

## Tasks

### Task 1: Verify predecessors + scaffold baseline marker

**Files:**
- Verify (on disk): predecessor M4-05 wiring follow-up committed (commit `29dfe89` or successor) AND `lingxi-code/crates/agent/src/runner.rs` still contains the 2-line stub AND the two `Arc::ptr_eq` tests still pass under HEAD.
- No file modification this task — just a baseline marker commit recording the pre-M5-01 state.

- [ ] **Step 1: Verify the three follow-up surfaces are at the documented baseline**

Run:
```bash
cd /Users/luolingfeng/Projects/LingXi-Next
grep -n 'let _ = event_rx.recv().await;' lingxi-code/crates/agent/src/runner.rs && echo "A baseline OK (runner stub present)"
grep -n 'content: String::new(),' lingxi-code/crates/tasks/src/handle.rs | head -1 && echo "B baseline OK (output stub present)"
grep -n 'Ok(Value::Null)' lingxi-code/crates/tools/src/tool_invoker_impl.rs && echo "C baseline OK (invoker stub present)"
```
Expected: all three `OK` lines print. If ANY fails, the follow-up has already been (partially) implemented — STOP and reconcile with the executor.

- [ ] **Step 2: Verify the two critical `Arc::ptr_eq` tests pass at HEAD**

Run:
```bash
cargo test -p lingxi-tools \
  --test-threads=1 \
  recursion_lock_child_inherits_parent_tool_registry_arc \
  budget_inheritance_child_inherits_parent_budget_arc \
  2>&1 | tail -20
```
Expected: `test result: ok. 2 passed; 0 failed`. These two tests are the M4-05 wiring lock and MUST be green before AND after M5-01.

- [ ] **Step 3: Pin the locked failure message string + Killed reason prefixes**

Read `lingxi-code/crates/core/src/reducer.rs` end-to-end to identify what `reason` string the reducer puts on `Terminated` for `UserExit` / `UserInterrupt` / `BudgetExceeded`. Run:
```bash
grep -n 'Terminated {' lingxi-code/crates/core/src/reducer.rs | head -20
grep -n 'reason:' lingxi-code/crates/core/src/reducer.rs | head -20
```
Note the exact `reason` literals (e.g. `"user_exit"`, `"user_interrupt"`, `"budget_exceeded"`, `"api_error"`). Record these in a comment at the top of Task 3's `run_subagent` body so the Killed-vs-Completed predicate is defensible. **If the reducer does not yet produce a `Terminated` state on `UserExit`/`UserInterrupt`** (in M1 it may go straight to `Idle` or no-op), Task 4's test seeds the terminal via a direct `Event` that the reducer DOES translate — for example `Event::ApiError` with a budget-exceeded payload; the test then asserts `SubagentEvent::Failed`. The exact mapping is recorded in Task 3 step 2.

- [ ] **Step 4: Baseline commit (no code change — just a marker)**

```bash
git commit --allow-empty -m "$(cat <<'EOF'
chore(M5-01 task 1): baseline marker before engine wiring close-out

Pre-M5-01 state verified:
- agent::runner::run_subagent stub present (1 recv + 1 Completed{stub})
- TaskRegistryHandle::output stub present (empty content)
- RegistryToolInvoker::invoke stub present (NotFound probe + Ok(Null))
- 2 Arc::ptr_eq tests pass at HEAD (M4-05 lock)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Write failing test — `run_subagent` real loop emits `Message` on `ApiStreamEnd`

**Files:**
- Modify: `lingxi-code/crates/agent/src/runner.rs` — append a `#[cfg(test)] mod tests { ... }` block at the bottom of the file (if absent — verified in Task 1) with the first failing test.

- [ ] **Step 1: Write the failing test**

Open `lingxi-code/crates/agent/src/runner.rs`. After the existing `pub async fn run_subagent(...)` body (line 71), append:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
    };
    use crate::display::{AgentColor, AgentDisplay};
    use lingxi_core::token::Usage;
    use lingxi_protocol::{ConversationMessage, MessageId, RequestId};
    use tokio::sync::mpsc;

    /// Build a `SubagentContext` with the minimum fields the runner reads.
    fn fresh_subagent_ctx() -> SubagentContext {
        SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_definition: AgentDefinition {
                agent_type: "test".into(),
                when_to_use: String::new(),
                tools: AgentToolPolicy::All { use_exact_tools: true },
                max_turns: 1,
                model: AgentModel::Inherit,
                permission_mode: AgentPermissionMode::Bubble,
                source: AgentSource::BuiltIn,
                base_dir: "/tmp".into(),
                system_prompt: None,
                mcp_servers: vec![],
                frontmatter_hooks: vec![],
                icon: None,
                allowed_tools: vec![],
                worktree_requirement: None,
            },
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            is_async: false,
            can_show_permission_prompts: true,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            rendered_system_prompt: None,
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon: None,
            },
        }
    }

    /// Drain the SubagentEvent receiver into a Vec.
    async fn drain(mut rx: mpsc::Receiver<SubagentEvent>) -> Vec<SubagentEvent> {
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn run_subagent_emits_message_on_api_stream_end_then_completed() {
        let ctx = fresh_subagent_ctx();
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

        // Drive the runner from a request-response cycle the M1 reducer
        // accepts: UserMessage -> ApiStreamStart -> ApiStreamEnd -> UserExit.
        // The runner is expected to surface the assistant Message on
        // ApiStreamEnd and finally Completed (Killed only on user_exit-prefixed
        // terminal — see Task 1 step 3 notes).
        let req = RequestId::new();
        let msg_id = MessageId::new();
        let final_msg = ConversationMessage::assistant_text(MessageId::new(), "hello world");

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        event_tx
            .send(lingxi_core::Event::UserMessage {
                message_id: msg_id,
                request_id: req,
                content: "hi".into(),
            })
            .await
            .unwrap();
        event_tx
            .send(lingxi_core::Event::ApiStreamStart { request_id: req })
            .await
            .unwrap();
        event_tx
            .send(lingxi_core::Event::ApiStreamEnd {
                request_id: req,
                final_message: final_msg.clone(),
                usage: Usage::default(),
            })
            .await
            .unwrap();
        // Close the event channel — the runner should NOT treat clean close
        // as a failure when it has already produced a Message; instead it
        // emits a Completed terminal (graceful end on EOF).
        drop(event_tx);

        handle.await.unwrap();
        let evs = drain(out_rx).await;

        // At least one Message and exactly one terminal Completed.
        let message_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Message { .. }))
            .count();
        let completed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
            .count();
        let failed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Failed { .. }))
            .count();

        assert_eq!(message_count, 1, "exactly one Message emitted on ApiStreamEnd; got events: {evs:?}");
        assert_eq!(completed_count, 1, "exactly one Completed terminal; got events: {evs:?}");
        assert_eq!(failed_count, 0, "no Failed events expected; got events: {evs:?}");

        // The Message payload must be JSON-equivalent to the serialized final_message.
        let msg_payload = evs
            .iter()
            .find_map(|e| match e {
                SubagentEvent::Message { agent_id: aid, message } => Some((*aid, message.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(msg_payload.0, agent_id, "Message agent_id matches ctx.agent_id");
        let expected = serde_json::to_value(&final_msg).unwrap();
        assert_eq!(msg_payload.1, expected, "Message payload byte-equals serialized final_message");
    }
}
```

**Note on `ConversationMessage::assistant_text`**: this constructor is the M1 idiom (see `lingxi-protocol::ConversationMessage::user` for the symmetric one). If the exact symbol differs at HEAD (e.g. `assistant`, `assistant_with_text`), grep `lingxi-code/crates/protocol/src/` for the correct constructor and substitute it byte-for-byte. Expected available variants:
```bash
grep -nE 'impl ConversationMessage|pub fn (user|assistant)' lingxi-code/crates/protocol/src/conversation_message.rs 2>/dev/null | head -10
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p lingxi-agent --lib runner::tests::run_subagent_emits_message_on_api_stream_end_then_completed 2>&1 | tail -25
```
Expected failure mode: with the stub body, the runner emits `Completed { stub: true }` after the FIRST event and stops. The test sees:
- `message_count == 0` (no Message emitted)
- `completed_count == 1` (but it's the stub payload, not the real one)
The assertion `assert_eq!(message_count, 1, ...)` fires with `left: 0`, `right: 1`. The test FAILS as expected.

- [ ] **Step 3: Write minimal implementation — first slice of the real loop**

Replace the body of `pub async fn run_subagent(...)` (currently lines 64-70) with:

```rust
pub async fn run_subagent(
    ctx: SubagentContext,
    mut event_rx: mpsc::Receiver<lingxi_core::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    use lingxi_core::{reduce, ConversationState, SessionState};
    use lingxi_protocol::SessionId;

    let agent_id = ctx.agent_id;

    // Seed initial state. The runner's local SessionState is transient —
    // the orchestrator (M5-02) owns durable session persistence. We use
    // SessionId::nil() and an inherited model string projected from the
    // agent definition; both are placeholders the reducer accepts.
    let model = match &ctx.agent_definition.model {
        crate::definition::AgentModel::Inherit => "inherit".to_string(),
        crate::definition::AgentModel::Named(n) => n.clone(),
    };
    let mut state = ConversationState::Idle {
        session: SessionState::empty(SessionId::nil(), model),
    };

    while let Some(event) = event_rx.recv().await {
        // Capture whether this event represents a stream-end completion
        // BEFORE the reducer consumes it — we need to peek at the
        // final_message for the Message emit.
        let api_end_msg = match &event {
            lingxi_core::Event::ApiStreamEnd { final_message, .. } => {
                Some(final_message.clone())
            }
            _ => None,
        };

        let (new_state, _effects) = reduce(state, event);
        state = new_state;

        if let Some(msg) = api_end_msg {
            let _ = out_tx
                .send(SubagentEvent::Message {
                    agent_id,
                    message: serde_json::to_value(&msg).unwrap_or(serde_json::Value::Null),
                })
                .await;
        }

        if state.is_terminal() {
            // Inspect the terminal reason; the M1 reducer puts it on
            // `Terminated { reason, .. }`. Killed prefixes per Task 1 step 3.
            if let ConversationState::Terminated { reason, .. } = &state {
                if reason.starts_with("user_exit")
                    || reason.starts_with("user_interrupt")
                    || reason.starts_with("killed")
                {
                    let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
                } else {
                    let _ = out_tx
                        .send(SubagentEvent::Completed {
                            agent_id,
                            result: serde_json::json!({ "reason": reason }),
                        })
                        .await;
                }
            }
            return;
        }
    }

    // event_rx closed before reaching Terminated. If we already emitted a
    // Message (the test path), it's still a graceful end — emit Completed
    // with a synthetic reason. Otherwise (no useful work done), emit Failed.
    //
    // We track "useful work" via a boolean set when api_end_msg is Some
    // — but the local variable is in the loop scope. To keep the loop
    // body small, we track it via a flag mutated by the loop. See Task 4
    // step 3 for the boolean integration. For now, default to Completed.
    let _ = out_tx
        .send(SubagentEvent::Completed {
            agent_id,
            result: serde_json::json!({ "reason": "eof_graceful" }),
        })
        .await;
}
```

**Note**: this implementation is *deliberately minimal* for Task 2. Tasks 3 and 4 refine the Killed + Failed branches.

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p lingxi-agent --lib runner::tests::run_subagent_emits_message_on_api_stream_end_then_completed 2>&1 | tail -15
```
Expected: `test result: ok. 1 passed; 0 failed`. The runner now emits a Message on `ApiStreamEnd` and a Completed on channel close.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/agent/src/runner.rs
git commit -m "$(cat <<'EOF'
feat(M5-01 task 2): run_subagent emits Message on ApiStreamEnd

Replace the 2-line stub with a real reduce/pump loop:
- Seed ConversationState::Idle from SubagentContext.agent_definition.model.
- Pull events from event_rx, fold via lingxi_core::reduce.
- On ApiStreamEnd, serialize final_message and emit SubagentEvent::Message.
- On Terminated, emit Completed (or Killed if reason starts with
  user_exit/user_interrupt/killed — wired fully in Task 4).
- On channel-close before terminal, emit graceful Completed (Failed path
  lands in Task 4).

Test: runner::tests::run_subagent_emits_message_on_api_stream_end_then_completed.
The 2 M4-05 Arc::ptr_eq tests remain unaffected (different code path).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Write failing test — `run_subagent` emits `Killed` when reducer terminates with `user_exit` / `user_interrupt`

**Files:**
- Modify: `lingxi-code/crates/agent/src/runner.rs` — add one test inside the existing `#[cfg(test)] mod tests` block.

- [ ] **Step 1: Write the failing test**

Inside the `mod tests` block in `runner.rs`, append:

```rust
    #[tokio::test]
    async fn run_subagent_emits_killed_on_user_exit_terminal() {
        let ctx = fresh_subagent_ctx();
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        // Drive directly to terminal via UserExit. The M1 reducer should
        // map this to ConversationState::Terminated { reason: "user_exit", .. }
        // (or similar prefix; predicate in the runner accepts any of
        // user_exit/user_interrupt/killed).
        event_tx.send(lingxi_core::Event::UserExit).await.unwrap();
        drop(event_tx);

        handle.await.unwrap();
        let mut evs = Vec::new();
        let mut rx = out_rx;
        while let Some(e) = rx.recv().await {
            evs.push(e);
        }

        let killed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Killed { .. }))
            .count();
        let completed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
            .count();

        // Exactly one Killed, no Completed (Killed is the terminal here).
        assert_eq!(
            killed_count, 1,
            "exactly one Killed expected on UserExit; got events: {evs:?}"
        );
        assert_eq!(
            completed_count, 0,
            "no Completed expected when terminal reason is user_exit; got events: {evs:?}"
        );

        // Killed carries the agent id.
        let killed_aid = evs
            .iter()
            .find_map(|e| match e {
                SubagentEvent::Killed { agent_id } => Some(*agent_id),
                _ => None,
            })
            .unwrap();
        assert_eq!(killed_aid, agent_id);
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p lingxi-agent --lib runner::tests::run_subagent_emits_killed_on_user_exit_terminal 2>&1 | tail -25
```
Possible failure modes:
- **(a)** If the M1 reducer maps `UserExit` to `Terminated { reason: "user_exit" }`, the runner code from Task 2 step 3 ALREADY satisfies this — the test should PASS as soon as Task 2 lands. In that case, label this task as "regression-pinning" and proceed to step 5.
- **(b)** If the reducer maps `UserExit` to `Terminated { reason: "exit" }` or some other prefix that does NOT match the runner's predicate, the runner emits `Completed` instead of `Killed`. Assertion `killed_count == 1` fires. In that case, step 3 refines the predicate.
- **(c)** If the reducer does NOT terminate on `UserExit` (e.g. stays in `Idle`), the runner loop sees `event_rx` close while in `Idle` and emits a graceful `Completed`. Same assertion failure as (b). Step 3 adds an explicit `UserExit`→`Killed` shortcut in the runner.

Capture the actual output to determine which branch we're in.

- [ ] **Step 3: Write minimal implementation — refine the runner**

If failure mode (a): no code change needed; the test already passes. Skip to step 4.

If failure mode (b) or (c): refine the runner body. Inside the `while let Some(event) = event_rx.recv().await { ... }` loop, ADD a fast-path BEFORE the reducer call:

```rust
        // Fast path: explicit user-termination events bypass the reducer
        // and surface as Killed directly. The reducer's `reason` strings
        // are an implementation detail; we treat the input event itself
        // as authoritative for the Killed signal.
        if matches!(
            &event,
            lingxi_core::Event::UserExit | lingxi_core::Event::UserInterrupt
        ) {
            // Still drive the reducer so state stays consistent for any
            // future inspection — but ignore its terminal reason.
            let (new_state, _effects) = reduce(state, event);
            state = new_state;
            let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
            return;
        }
```

Insert this block at the TOP of the loop body, before the existing `api_end_msg` capture.

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p lingxi-agent --lib runner::tests::run_subagent_emits_killed_on_user_exit_terminal 2>&1 | tail -15
cargo test -p lingxi-agent --lib runner::tests::run_subagent_emits_message_on_api_stream_end_then_completed 2>&1 | tail -15
```
Expected: BOTH tests pass. (The Task 2 test must remain green after Task 3's refinement — the fast-path only triggers on `UserExit`/`UserInterrupt`, neither of which appear in the Task 2 event sequence.)

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/agent/src/runner.rs
git commit -m "$(cat <<'EOF'
feat(M5-01 task 3): run_subagent emits Killed on UserExit/UserInterrupt

Add an explicit fast-path in the runner loop: when the inbound Event is
UserExit or UserInterrupt, drive the reducer for state consistency but
emit SubagentEvent::Killed directly (bypassing the reason-string
inspection on Terminated). This makes the Killed signal robust to
reducer reason-string drift.

Test: runner::tests::run_subagent_emits_killed_on_user_exit_terminal.
Task 2's Message-then-Completed test remains green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Write failing test — `run_subagent` emits `Failed` when event channel closes before any useful work

**Files:**
- Modify: `lingxi-code/crates/agent/src/runner.rs` — add one test + adjust the EOF branch to distinguish "useful work happened" (Completed) from "nothing happened" (Failed).

- [ ] **Step 1: Write the failing test**

Inside the `mod tests` block, append:

```rust
    #[tokio::test]
    async fn run_subagent_emits_failed_on_eof_before_any_work() {
        let ctx = fresh_subagent_ctx();
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        // Drop the sender immediately — runner sees event_rx close on the
        // very first recv, with no prior Message or Terminated emission.
        drop(event_tx);

        handle.await.unwrap();
        let mut evs = Vec::new();
        let mut rx = out_rx;
        while let Some(e) = rx.recv().await {
            evs.push(e);
        }

        let failed = evs
            .iter()
            .find_map(|e| match e {
                SubagentEvent::Failed { agent_id: aid, error } => {
                    Some((*aid, error.clone()))
                }
                _ => None,
            });
        assert!(
            failed.is_some(),
            "exactly one Failed expected on premature EOF; got events: {evs:?}"
        );
        let (failed_aid, failed_err) = failed.unwrap();
        assert_eq!(failed_aid, agent_id);
        assert_eq!(
            failed_err,
            "run_subagent: event channel closed without terminal state",
            "byte-locked error message"
        );

        let completed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
            .count();
        assert_eq!(completed_count, 0, "no Completed expected; got events: {evs:?}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p lingxi-agent --lib runner::tests::run_subagent_emits_failed_on_eof_before_any_work 2>&1 | tail -25
```
Expected failure: the current Task 2 implementation emits `Completed { reason: "eof_graceful" }` unconditionally on EOF. The test expects `Failed { error: "run_subagent: event channel closed..." }`. Assertion `failed.is_some()` fires.

- [ ] **Step 3: Write minimal implementation — distinguish "useful work" from "nothing"**

Modify the runner body. Add a local `bool useful_work = false;` before the `while let Some(event)` loop. Set it to `true` immediately AFTER each successful `out_tx.send(SubagentEvent::Message { .. })` call (the only "useful work" signal in M5-01 — Task 5+ can refine).

Then, replace the post-loop EOF tail (currently the unconditional `Completed { reason: "eof_graceful" }`) with:

```rust
    // event_rx closed before reaching Terminated.
    if useful_work {
        let _ = out_tx
            .send(SubagentEvent::Completed {
                agent_id,
                result: serde_json::json!({ "reason": "eof_graceful" }),
            })
            .await;
    } else {
        let _ = out_tx
            .send(SubagentEvent::Failed {
                agent_id,
                error: "run_subagent: event channel closed without terminal state".to_string(),
            })
            .await;
    }
```

The full revised body of `run_subagent` (after Tasks 2-4) looks like:

```rust
pub async fn run_subagent(
    ctx: SubagentContext,
    mut event_rx: mpsc::Receiver<lingxi_core::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    use lingxi_core::{reduce, ConversationState, SessionState};
    use lingxi_protocol::SessionId;

    let agent_id = ctx.agent_id;

    let model = match &ctx.agent_definition.model {
        crate::definition::AgentModel::Inherit => "inherit".to_string(),
        crate::definition::AgentModel::Named(n) => n.clone(),
    };
    let mut state = ConversationState::Idle {
        session: SessionState::empty(SessionId::nil(), model),
    };
    let mut useful_work = false;

    while let Some(event) = event_rx.recv().await {
        // Fast path: UserExit/UserInterrupt -> Killed.
        if matches!(
            &event,
            lingxi_core::Event::UserExit | lingxi_core::Event::UserInterrupt
        ) {
            let (new_state, _effects) = reduce(state, event);
            state = new_state;
            let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
            return;
        }

        // Capture ApiStreamEnd's final_message before the reducer consumes it.
        let api_end_msg = match &event {
            lingxi_core::Event::ApiStreamEnd { final_message, .. } => {
                Some(final_message.clone())
            }
            _ => None,
        };

        let (new_state, _effects) = reduce(state, event);
        state = new_state;

        if let Some(msg) = api_end_msg {
            let _ = out_tx
                .send(SubagentEvent::Message {
                    agent_id,
                    message: serde_json::to_value(&msg).unwrap_or(serde_json::Value::Null),
                })
                .await;
            useful_work = true;
        }

        if state.is_terminal() {
            if let ConversationState::Terminated { reason, .. } = &state {
                if reason.starts_with("user_exit")
                    || reason.starts_with("user_interrupt")
                    || reason.starts_with("killed")
                {
                    let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
                } else {
                    let _ = out_tx
                        .send(SubagentEvent::Completed {
                            agent_id,
                            result: serde_json::json!({ "reason": reason }),
                        })
                        .await;
                }
            }
            return;
        }
    }

    if useful_work {
        let _ = out_tx
            .send(SubagentEvent::Completed {
                agent_id,
                result: serde_json::json!({ "reason": "eof_graceful" }),
            })
            .await;
    } else {
        let _ = out_tx
            .send(SubagentEvent::Failed {
                agent_id,
                error: "run_subagent: event channel closed without terminal state".to_string(),
            })
            .await;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p lingxi-agent --lib runner::tests 2>&1 | tail -25
```
Expected: ALL THREE runner tests pass:
- `run_subagent_emits_message_on_api_stream_end_then_completed`
- `run_subagent_emits_killed_on_user_exit_terminal`
- `run_subagent_emits_failed_on_eof_before_any_work`

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/agent/src/runner.rs
git commit -m "$(cat <<'EOF'
feat(M5-01 task 4): run_subagent emits Failed on premature EOF

Track 'useful work' via a boolean flipped when a Message is emitted on
ApiStreamEnd. When event_rx closes:
- useful_work = true  -> Completed { reason: "eof_graceful" }
- useful_work = false -> Failed { error: "run_subagent: event channel
  closed without terminal state" }  (byte-locked message)

Test: runner::tests::run_subagent_emits_failed_on_eof_before_any_work.
The Task 2 + Task 3 tests remain green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Verify M4-05 `Arc::ptr_eq` invariants survive the runner rewrite

**Files:**
- Verify only — no code change.

This task is the explicit M4-05 backward-compatibility gate per spec §6.2. Although the runner changes do not touch the inheritance bundle path (the bundle is consumed in production by `SubagentSpawner::spawn`, which is unrelated to `run_subagent`), this task RUNS the two critical tests with `cargo test --release` and `cargo test` to confirm no flaky regression.

- [ ] **Step 1: Re-run the two critical M4-05 Arc::ptr_eq tests**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
cargo test -p lingxi-tools recursion_lock_child_inherits_parent_tool_registry_arc 2>&1 | tail -10
cargo test -p lingxi-tools budget_inheritance_child_inherits_parent_budget_arc  2>&1 | tail -10
```
Expected: BOTH pass. If either fails, STOP — Tasks 2-4 introduced an Arc-identity regression. Likely cause: an accidental `Arc::new` somewhere in the runner code that shadows the inherited Arc. Re-read the runner diff and fix.

- [ ] **Step 2: Run the entire `lingxi-tools` test suite**

```bash
cargo test -p lingxi-tools 2>&1 | tail -30
```
Expected: ALL pass. The lingxi-tools suite includes the agent_task_integration_test integration suite added in M4-05; it must remain green.

- [ ] **Step 3: Run the entire `lingxi-agent` test suite**

```bash
cargo test -p lingxi-agent 2>&1 | tail -20
```
Expected: ALL pass (including the 3 new runner tests + all M1 pool/dispatcher tests).

- [ ] **Step 4: Pin-commit the verification**

```bash
git commit --allow-empty -m "$(cat <<'EOF'
chore(M5-01 task 5): M4-05 Arc::ptr_eq invariants verified green

Post-runner-rewrite verification:
- recursion_lock_child_inherits_parent_tool_registry_arc: PASS
- budget_inheritance_child_inherits_parent_budget_arc: PASS
- cargo test -p lingxi-tools: full suite PASS
- cargo test -p lingxi-agent: full suite PASS (+3 new runner tests)

Follow-up A (runner pump) now functionally complete.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Write failing test — `TaskRegistryHandle::output` returns real spool content (not empty)

**Files:**
- Modify: `lingxi-code/crates/tasks/src/handle.rs::tests` block — repurpose the existing `output_returns_empty_for_freshly_created_task` test and add a new `output_returns_real_content_after_spool_write`.

- [ ] **Step 1: Extend `NoopFs` to be a real in-memory FS for the spool write/read path**

The existing `NoopFs` in `handle.rs::tests` (lines 242-295) returns empty on every read. The new test needs to write content to a path and then read it back. Replace `NoopFs` with `InMemoryFs`:

In `handle.rs::tests`, replace the `struct NoopFs;` block (and its `impl FileSystem for NoopFs`) with:

```rust
    use std::collections::HashMap;
    use tokio::sync::Mutex as TokioMutex;

    struct InMemoryFs {
        files: TokioMutex<HashMap<String, String>>,
    }

    impl InMemoryFs {
        fn new() -> Self {
            Self {
                files: TokioMutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl FileSystem for InMemoryFs {
        async fn read_file(
            &self,
            path: &str,
            offset: Option<u64>,
            limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let off = offset.unwrap_or(0) as usize;
            let body: String = content.chars().skip(off).collect();
            let truncated = if let Some(lim) = limit {
                body.len() as u64 > lim
            } else {
                false
            };
            let trimmed = if let Some(lim) = limit {
                body.chars().take(lim as usize).collect::<String>()
            } else {
                body
            };
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content: trimmed,
                truncated,
                total_lines,
            })
        }
        async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .insert(path.to_string(), body.to_string());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            _: &str,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
        {
            Err(FsError::Io("not supported".into()))
        }
        async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            let entry = map.entry(path.to_string()).or_default();
            entry.push_str(body);
            Ok(())
        }
        async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            if let Some(s) = map.get_mut(path) {
                s.truncate(len as usize);
            }
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            let map = self.files.lock().await;
            Ok(map.get(path).map(|s| s.len() as u64).unwrap_or(0))
        }
        async fn delete_file(&self, path: &str) -> Result<(), FsError> {
            self.files.lock().await.remove(path);
            Ok(())
        }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("not supported".into()))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> {
            Ok(())
        }
    }
```

Then replace `let fs: Arc<dyn FileSystem> = Arc::new(NoopFs);` (line 299) with `let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());`.

Rename `output_returns_empty_for_freshly_created_task` to `output_returns_empty_for_freshly_created_task_with_zero_byte_spool` (the existing assertion `chunk.content == ""` still holds because `InMemoryFs` returns empty for a freshly-allocated spool — `allocate` writes an empty string).

- [ ] **Step 2: Write the new failing test**

Append inside `handle.rs::tests`:

```rust
    #[tokio::test]
    async fn output_returns_real_content_after_spool_write() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "echo hi".into(),
            })
            .await
            .unwrap();

        // Pull the underlying spool path from the typed state, then write
        // content directly through the registry's filesystem (this
        // simulates a handler producing output).
        let state = registry.get(&rec.task_id).await.unwrap();
        let path = state.base().output_file.clone();
        let path_str = path.to_str().unwrap().to_string();
        let fs = registry.output_manager.fs_for_test();
        fs.write_file(&path_str, "line1\nline2\nline3\n").await.unwrap();

        let chunk = h.output(&rec.task_id, None).await.unwrap();
        assert_eq!(chunk.task_id, rec.task_id);
        assert_eq!(
            chunk.content, "line1\nline2\nline3\n",
            "spool content surfaces verbatim"
        );
        assert_eq!(chunk.total_lines, 3, "total_lines reflects the spool");
        assert!(!chunk.truncated, "no truncation on unlimited read");
    }

    #[tokio::test]
    async fn output_threads_offset_into_output_manager() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "echo hi".into(),
            })
            .await
            .unwrap();

        let state = registry.get(&rec.task_id).await.unwrap();
        let path_str = state.base().output_file.to_str().unwrap().to_string();
        let fs = registry.output_manager.fs_for_test();
        fs.write_file(&path_str, "abcdef").await.unwrap();

        // offset=3 should drop the first 3 chars.
        let chunk = h.output(&rec.task_id, Some(3)).await.unwrap();
        assert_eq!(chunk.content, "def", "offset honoured");
    }
```

The test calls `registry.output_manager.fs_for_test()` — a new pub(crate) accessor on `TaskOutputManager` that returns `Arc<dyn FileSystem>`. Add this accessor to `output_manager.rs`:

```rust
    /// Test-only accessor for the backing filesystem (so integration tests
    /// can seed spool content directly).
    #[doc(hidden)]
    pub fn fs_for_test(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }
```

Place it inside `impl TaskOutputManager` after the existing `read` method. The `#[doc(hidden)]` marks it as not-part-of-public-API while still being callable from `handle.rs::tests` (same crate, so `pub(crate)` would also work; `pub` + `#[doc(hidden)]` keeps it symmetric with M4-05's testing conventions).

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p lingxi-tasks --lib handle::tests::output_returns_real_content_after_spool_write 2>&1 | tail -25
cargo test -p lingxi-tasks --lib handle::tests::output_threads_offset_into_output_manager 2>&1 | tail -25
```
Expected: BOTH fail. The stub `output(...)` body returns `TaskOutputChunk { content: "".into(), total_lines: 0, truncated: false }` regardless of what's on disk. Assertions `chunk.content == "line1\nline2\nline3\n"` and `chunk.content == "def"` both fire.

- [ ] **Step 4: Write minimal implementation — wire `output` to `TaskOutputManager::read`**

Open `lingxi-code/crates/tasks/src/handle.rs`. Replace the `async fn output(...)` body (lines 209-228) with:

```rust
    async fn output(
        &self,
        id: &str,
        offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        let state = self
            .get(id)
            .await
            .ok_or_else(|| TaskRegistryError::NotFound(id.into()))?;
        let output_file = state.base().output_file.clone();
        let opts = crate::output_manager::OutputOptions {
            offset,
            limit: None,
        };
        let out = self
            .output_manager
            .read(&output_file, opts)
            .await
            .map_err(|e| match e {
                crate::output_manager::OutputError::Io(s) => {
                    TaskRegistryError::Internal(format!("io: {s}"))
                }
                crate::output_manager::OutputError::PathEscape(p) => {
                    TaskRegistryError::Internal(format!("path escape: {p}"))
                }
            })?;
        Ok(TaskOutputChunk {
            task_id: state.base().id.clone(),
            content: out.content,
            total_lines: out.total_lines,
            truncated: out.truncated,
        })
    }
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p lingxi-tasks --lib handle::tests 2>&1 | tail -25
```
Expected: ALL handle::tests pass:
- `create_via_handle_returns_record_with_pending_status`
- `list_filters_by_status`
- `set_status_via_handle_transitions`
- `unknown_task_type_is_invalid_input`
- `output_returns_empty_for_freshly_created_task_with_zero_byte_spool`
- `output_returns_real_content_after_spool_write` (NEW)
- `output_threads_offset_into_output_manager` (NEW)

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/crates/tasks/src/handle.rs lingxi-code/crates/tasks/src/output_manager.rs
git commit -m "$(cat <<'EOF'
feat(M5-01 task 6): TaskRegistryHandle::output reads real spool

Replace the empty-placeholder body with a real read through
TaskOutputManager::read:
- look up the task state, pull base.output_file: PathBuf
- thread the offset arg into OutputOptions { offset, limit: None }
- map OutputError::{Io, PathEscape} -> TaskRegistryError::Internal
- surface content, total_lines, truncated verbatim in TaskOutputChunk

Replace NoopFs (always-empty) in handle::tests with InMemoryFs (real
write/read backing map). Add fs_for_test() accessor on
TaskOutputManager so tests can seed spool content directly.

Tests:
- output_returns_real_content_after_spool_write
- output_threads_offset_into_output_manager
- (existing) output_returns_empty_for_freshly_created_task_with_zero_byte_spool

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Integration test — `TaskRegistryHandle::output` end-to-end via a test executor

**Files:**
- Modify: `lingxi-code/crates/tools/tests/agent_task_integration_test.rs` — add a single new integration test exercising create → seed spool → list → output round-trip via the public `TaskRegistryHandle` trait surface (no internal access).

- [ ] **Step 1: Read the existing integration test file to learn its conventions**

```bash
grep -n '^#\[tokio::test\]\|^async fn\|^use ' lingxi-code/crates/tools/tests/agent_task_integration_test.rs | head -30
```
Capture the helper-construction patterns (`make_test_*` functions, mock injection).

- [ ] **Step 2: Write the failing test**

Append to `lingxi-code/crates/tools/tests/agent_task_integration_test.rs`:

```rust
#[tokio::test]
async fn task_registry_handle_output_round_trip_via_real_output_manager() {
    use lingxi_tasks::output_manager::TaskOutputManager;
    use lingxi_tasks::TaskRegistry;
    use lingxi_traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
    use lingxi_traits::task_registry::{TaskCreateInput, TaskRegistryHandle};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::Mutex as TokioMutex;

    struct MapFs {
        files: TokioMutex<HashMap<String, String>>,
    }
    #[async_trait::async_trait]
    impl FileSystem for MapFs {
        async fn read_file(
            &self,
            p: &str,
            offset: Option<u64>,
            limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(p).cloned().unwrap_or_default();
            let off = offset.unwrap_or(0) as usize;
            let body: String = content.chars().skip(off).collect();
            let trimmed = if let Some(l) = limit {
                body.chars().take(l as usize).collect()
            } else {
                body.clone()
            };
            Ok(FileContent {
                truncated: limit.map(|l| body.len() as u64 > l).unwrap_or(false),
                total_lines: content.lines().count() as u64,
                content: trimmed,
            })
        }
        async fn write_file(&self, p: &str, b: &str) -> Result<(), FsError> {
            self.files.lock().await.insert(p.into(), b.into());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool { true }
        async fn watch(&self, _: &str) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError> {
            Err(FsError::Io("nope".into()))
        }
        async fn append_file(&self, p: &str, b: &str) -> Result<(), FsError> {
            let mut m = self.files.lock().await;
            m.entry(p.into()).or_default().push_str(b);
            Ok(())
        }
        async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> { Ok(()) }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, p: &str) -> Result<u64, FsError> {
            Ok(self.files.lock().await.get(p).map(|s| s.len() as u64).unwrap_or(0))
        }
        async fn delete_file(&self, _: &str) -> Result<(), FsError> { Ok(()) }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> { Ok(()) }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("nope".into()))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> { Ok(()) }
    }

    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(MapFs { files: TokioMutex::new(HashMap::new()) });
    let runtime = Arc::new(lingxi_test_harness::mocks::MockRuntimeSpawner::default());
    let out_mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
    let registry = Arc::new(TaskRegistry::new(runtime, fs.clone(), out_mgr.clone()));
    let h: &dyn TaskRegistryHandle = registry.as_ref();

    // 1) create
    let rec = h
        .create(TaskCreateInput {
            task_type: "local_bash".into(),
            description: "spool round-trip".into(),
        })
        .await
        .expect("create");

    // 2) seed spool via the manager's filesystem (simulates a handler).
    let path = registry
        .get(&rec.task_id)
        .await
        .unwrap()
        .base()
        .output_file
        .clone();
    let path_str = path.to_str().unwrap().to_string();
    fs.write_file(&path_str, "hello\nworld\n").await.unwrap();

    // 3) output via the handle trait surface
    let chunk = h.output(&rec.task_id, None).await.expect("output");
    assert_eq!(chunk.content, "hello\nworld\n");
    assert_eq!(chunk.total_lines, 2);
    assert!(!chunk.truncated);

    // 4) verify NotFound surface still works for unknown ids
    let missing = h.output("zzzbogus0", None).await;
    assert!(matches!(
        missing,
        Err(lingxi_traits::task_registry::TaskRegistryError::NotFound(_))
    ));
}
```

If the integration test file does NOT yet import `async_trait` at the top level, add `async_trait = { workspace = true }` to `lingxi-code/crates/tools/Cargo.toml`'s `[dev-dependencies]` (likely already present from M4-05; verify with `grep async_trait lingxi-code/crates/tools/Cargo.toml`).

- [ ] **Step 2.5: Run test to verify it fails (against pre-Task-6 baseline) OR passes (post-Task-6)**

```bash
cargo test -p lingxi-tools --test agent_task_integration_test task_registry_handle_output_round_trip_via_real_output_manager 2>&1 | tail -20
```
Expected: PASSES (Task 6 already wired `output` to the manager). If it FAILS with empty content, Task 6's commit didn't land cleanly — re-verify `git log --oneline -5 lingxi-code/crates/tasks/src/handle.rs`.

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/crates/tools/tests/agent_task_integration_test.rs
git commit -m "$(cat <<'EOF'
test(M5-01 task 7): integration spool round-trip via TaskRegistryHandle

End-to-end through the public trait surface:
  create -> seed spool via manager.fs -> output -> NotFound for bogus id.

Confirms Task 6's TaskRegistryHandle::output wiring works against a
real TaskOutputManager (not just the in-crate NoopFs replacement).

Follow-up B (TaskOutput spool reader) now functionally complete.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Scaffold a builtin `Echo`-style tool for testing `RegistryToolInvoker` dispatch

**Files:**
- Modify: `lingxi-code/crates/tools/src/tool_invoker_impl.rs` — extend the existing `#[cfg(test)] mod tests` block with a `TestEchoTool` fixture (private to the test module).

This task ADDS a tiny `Tool`-implementing struct used by Tasks 9-11 to drive the invoker. The struct lives entirely under `#[cfg(test)]` — no production code is affected.

- [ ] **Step 1: Write the test fixture (no test yet — just the fixture)**

Open `lingxi-code/crates/tools/src/tool_invoker_impl.rs`. Find the existing `#[cfg(test)] mod tests { ... }` block at line 62. Extend it with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_trait::{
        DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
        ToolStaticContext,
    };
    use crate::context::ToolUseContext;
    use async_trait::async_trait;
    use serde_json::json;

    /// Minimal Tool impl: returns its input under the "echo" key.
    /// Lives entirely under `#[cfg(test)]`.
    struct TestEchoTool;

    #[async_trait]
    impl Tool for TestEchoTool {
        fn name(&self) -> &str { "TestEcho" }
        fn aliases(&self) -> &[&str] { &[] }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "additionalProperties": true })
        }
        fn output_schema(&self) -> Option<serde_json::Value> { None }
        fn is_enabled(&self, _: &ToolStaticContext) -> bool { true }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool { true }
        fn is_read_only(&self, _: &serde_json::Value) -> bool { true }
        fn is_destructive(&self, _: &serde_json::Value) -> bool { false }
        fn is_open_world(&self, _: &serde_json::Value) -> bool { false }
        fn max_result_size_chars(&self) -> Option<usize> { None }
        fn interrupt_behavior(&self, _: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
        ) -> Result<(), crate::tool_trait::ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> crate::permissions::PermissionResult {
            crate::permissions::PermissionResult::Allow
        }
        async fn description(&self, _: &serde_json::Value, _: &DescriptionOptions) -> String {
            "echo".into()
        }
        async fn prompt(&self, _: &PromptOptions) -> String { "echo tool".into() }
        async fn call(
            &self,
            input: serde_json::Value,
            _ctx: ToolUseContext,
            _progress_tx: crate::progress::ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "echo": input }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Reusable: registry with the echo tool wired in.
    fn registry_with_echo() -> Arc<crate::registry::ToolRegistry> {
        let mut r = crate::registry::ToolRegistry::new();
        r.register_builtin(Arc::new(TestEchoTool));
        Arc::new(r)
    }

    // ──── existing test from M4-05 wiring (preserved) ──────────────
    #[test]
    fn registry_invoker_preserves_arc_identity() {
        let r = Arc::new(crate::registry::ToolRegistry::new());
        let inv = RegistryToolInvoker::new(r.clone());
        assert!(Arc::ptr_eq(&r, inv.registry_arc()));
    }
}
```

**Note on `Tool` trait method names and signatures**: every method above must match the trait at HEAD. The signatures above are taken from `lingxi-code/crates/tools/src/tool_trait.rs` (already inspected during Task 1). If any method has drifted (e.g. added a parameter), substitute the current signature byte-for-byte. The most likely drift surface is `max_result_size_chars()` (return type `Option<usize>` vs `usize`) and `interrupt_behavior` (whether it's `&self` or `&self, _: &Value`). Verify with:
```bash
grep -nE 'fn (name|aliases|input_schema|output_schema|is_enabled|is_concurrency_safe|is_read_only|is_destructive|is_open_world|max_result_size_chars|interrupt_behavior|validate_input|check_permissions|description|prompt|call)' lingxi-code/crates/tools/src/tool_trait.rs | head -30
```

- [ ] **Step 2: Verify the fixture compiles**

```bash
cargo test -p lingxi-tools --lib tool_invoker_impl::tests::registry_invoker_preserves_arc_identity 2>&1 | tail -10
```
Expected: PASS (this is the existing M4-05 test; the fixture extension must not break it). If the `TestEchoTool` definition has compile errors, fix the method signatures by re-grepping `tool_trait.rs`.

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/crates/tools/src/tool_invoker_impl.rs
git commit -m "$(cat <<'EOF'
test(M5-01 task 8): TestEchoTool fixture for RegistryToolInvoker tests

Adds a #[cfg(test)]-only Tool impl in tool_invoker_impl::tests that
returns input under {"echo": input}. Used by Tasks 9-11 to drive the
real-dispatch invoker path.

The existing M4-05 Arc-identity test remains green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Write failing test — `RegistryToolInvoker::invoke` returns tool output (not `Null`)

**Files:**
- Modify: `lingxi-code/crates/tools/src/tool_invoker_impl.rs::tests` — add one test.

- [ ] **Step 1: Write the failing test**

Inside the `mod tests` block, append:

```rust
    #[tokio::test]
    async fn registry_invoker_routes_to_tool_call_and_returns_data() {
        use lingxi_traits::tool_invoker::{SubagentInvocationContext, ToolInvoker};

        let registry = registry_with_echo();
        let invoker = RegistryToolInvoker::new(registry.clone());

        let input = json!({ "hello": "world", "n": 42 });
        let ctx = SubagentInvocationContext { parent_agent_id: None };

        let result = invoker
            .invoke("TestEcho", input.clone(), ctx)
            .await
            .expect("dispatch succeeds");

        // The stub returned Ok(Value::Null). Real dispatch must return the
        // tool's ToolCallResult.data, which is { "echo": <input> }.
        assert_eq!(
            result,
            json!({ "echo": { "hello": "world", "n": 42 } }),
            "invoker returns the tool's data verbatim"
        );
    }

    #[tokio::test]
    async fn registry_invoker_unknown_tool_surfaces_not_found() {
        use lingxi_traits::tool_invoker::{
            SubagentInvocationContext, ToolInvoker, ToolInvokerError,
        };

        let registry = registry_with_echo();
        let invoker = RegistryToolInvoker::new(registry);
        let result = invoker
            .invoke(
                "NotARealTool",
                json!({}),
                SubagentInvocationContext { parent_agent_id: None },
            )
            .await;
        match result {
            Err(ToolInvokerError::NotFound(name)) => assert_eq!(name, "NotARealTool"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p lingxi-tools --lib tool_invoker_impl::tests::registry_invoker_routes_to_tool_call_and_returns_data 2>&1 | tail -25
```
Expected failure: the stub body `Ok(Value::Null)` makes `result == Value::Null`. Assertion `result == json!({...})` fires with `left: Null`, `right: Object(...)`.

The `registry_invoker_unknown_tool_surfaces_not_found` test PASSES against the stub (the stub does call `find_by_name` and returns `NotFound`). Run it to confirm:
```bash
cargo test -p lingxi-tools --lib tool_invoker_impl::tests::registry_invoker_unknown_tool_surfaces_not_found 2>&1 | tail -10
```
Expected: PASS.

- [ ] **Step 3: Write minimal implementation — real dispatch**

Open `lingxi-code/crates/tools/src/tool_invoker_impl.rs`. Replace the `async fn invoke(...)` body (lines 40-55) with:

```rust
    async fn invoke(
        &self,
        name: &str,
        input: Value,
        ctx: SubagentInvocationContext,
    ) -> Result<Value, ToolInvokerError> {
        let tool = self
            .registry
            .find_by_name(name)
            .ok_or_else(|| ToolInvokerError::NotFound(name.to_string()))?;

        // Synthesize a minimal ToolUseContext. The recursion-lock invariant
        // requires `subagent_registry` to carry the SAME Arc<ToolRegistry>
        // this invoker wraps (so a recursive AgentTool call inside the
        // dispatched tool reuses the same registry — no fresh Arc).
        let tool_use_ctx = crate::context::ToolUseContext {
            options: crate::context::ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: "subagent".into(),
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: ctx.parent_agent_id,
            content_replacement_state: None,
            session: None,
            subagent_registry: Some(self.registry.clone()),
        };

        // Drop the progress receiver immediately — production tools tolerate
        // a closed progress channel, and we don't surface progress here.
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::channel::<
            crate::progress::ToolProgressEvent,
        >(8);
        let progress_tx = crate::progress::ToolProgressSender::new(progress_tx);

        let result = tool
            .call(input, tool_use_ctx, progress_tx)
            .await
            .map_err(|e| match e {
                crate::tool_trait::ToolError::InvalidInput(s) => {
                    ToolInvokerError::InvalidInput(s)
                }
                other => ToolInvokerError::Internal(format!("{other}")),
            })?;

        Ok(result.data)
    }
```

**Note on `ToolProgressSender::new`**: the constructor name MAY differ at HEAD. Likely candidates: `ToolProgressSender::new(tx)`, `ToolProgressSender::from(tx)`, or the type may be a direct alias for `mpsc::Sender<ToolProgressEvent>`. Verify with:
```bash
grep -n 'pub struct ToolProgressSender\|impl ToolProgressSender\|pub fn new\|pub type ToolProgressSender' lingxi-code/crates/tools/src/progress.rs | head -10
```
If `ToolProgressSender` IS a type alias for `mpsc::Sender<ToolProgressEvent>`, the constructor line collapses to `let progress_tx = progress_tx;` (the `mpsc::channel(8)` output already matches). Substitute the correct form.

**Note on `ToolUseContext` fields**: the field list above is taken from `lingxi-code/crates/tools/src/builtin/agent.rs::tests::fresh_ctx_with_registry` (the M4-05 test fixture; see line 428). If any field has drifted at HEAD, copy the M4-05 fixture's exact `ToolUseContext { .. }` literal byte-for-byte and substitute.

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p lingxi-tools --lib tool_invoker_impl::tests 2>&1 | tail -20
```
Expected: ALL FOUR `tool_invoker_impl::tests` pass:
- `registry_invoker_preserves_arc_identity`
- `registry_invoker_routes_to_tool_call_and_returns_data` (NEW)
- `registry_invoker_unknown_tool_surfaces_not_found` (NEW)
- (any other pre-existing tests)

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tools/src/tool_invoker_impl.rs
git commit -m "$(cat <<'EOF'
feat(M5-01 task 9): RegistryToolInvoker::invoke dispatches into ToolRegistry

Replace the find_by_name probe + Ok(Null) body with a real dispatch:
- find_by_name -> NotFound surface unchanged
- synthesize a minimal ToolUseContext, carrying subagent_registry =
  Some(self.registry.clone()) so the recursion-lock Arc is preserved
- propagate ctx.parent_agent_id onto tool_use_ctx.agent_id
- spawn a single-shot ToolProgressSender (receiver immediately dropped)
- map ToolError::InvalidInput -> ToolInvokerError::InvalidInput
- map every other ToolError -> ToolInvokerError::Internal
- return ToolCallResult.data verbatim

Tests:
- registry_invoker_routes_to_tool_call_and_returns_data
- registry_invoker_unknown_tool_surfaces_not_found
- (existing) registry_invoker_preserves_arc_identity

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Test — `RegistryToolInvoker::invoke` preserves the subagent_registry Arc identity

**Files:**
- Modify: `lingxi-code/crates/tools/src/tool_invoker_impl.rs::tests` — add one test asserting the recursion-lock Arc flows into the synthesized `ToolUseContext`.

This task makes the recursion-lock invariant explicit at the dispatch path (whereas Task 11 verifies the existing M4-05 tests still hold). A `RecordingTool` fixture captures the `ToolUseContext` it receives so the test can assert `Arc::ptr_eq(parent_registry, ctx.subagent_registry.unwrap())`.

- [ ] **Step 1: Write the failing test + recording fixture**

Inside `tool_invoker_impl::tests`, append:

```rust
    use std::sync::Mutex as StdMutex;

    /// Tool fixture that records the ToolUseContext it receives so tests
    /// can introspect the Arc identity of `subagent_registry`.
    struct RecordingTool {
        captured: Arc<StdMutex<Option<Option<Arc<crate::registry::ToolRegistry>>>>>,
    }

    #[async_trait]
    impl Tool for RecordingTool {
        fn name(&self) -> &str { "RecordingTool" }
        fn aliases(&self) -> &[&str] { &[] }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }
        fn output_schema(&self) -> Option<serde_json::Value> { None }
        fn is_enabled(&self, _: &ToolStaticContext) -> bool { true }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool { true }
        fn is_read_only(&self, _: &serde_json::Value) -> bool { true }
        fn is_destructive(&self, _: &serde_json::Value) -> bool { false }
        fn is_open_world(&self, _: &serde_json::Value) -> bool { false }
        fn max_result_size_chars(&self) -> Option<usize> { None }
        fn interrupt_behavior(&self, _: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
        async fn validate_input(
            &self,
            _: &serde_json::Value,
        ) -> Result<(), crate::tool_trait::ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> crate::permissions::PermissionResult {
            crate::permissions::PermissionResult::Allow
        }
        async fn description(&self, _: &serde_json::Value, _: &DescriptionOptions) -> String {
            "rec".into()
        }
        async fn prompt(&self, _: &PromptOptions) -> String { "rec".into() }
        async fn call(
            &self,
            _: serde_json::Value,
            ctx: ToolUseContext,
            _: crate::progress::ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            *self.captured.lock().unwrap() = Some(ctx.subagent_registry.clone());
            Ok(ToolCallResult {
                data: json!({}),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    #[tokio::test]
    async fn registry_invoker_preserves_subagent_registry_arc_into_tool_use_ctx() {
        use lingxi_traits::tool_invoker::{SubagentInvocationContext, ToolInvoker};

        let captured: Arc<StdMutex<Option<Option<Arc<crate::registry::ToolRegistry>>>>> =
            Arc::new(StdMutex::new(None));

        let mut registry = crate::registry::ToolRegistry::new();
        registry.register_builtin(Arc::new(RecordingTool {
            captured: captured.clone(),
        }));
        let parent_registry = Arc::new(registry);

        let invoker = RegistryToolInvoker::new(parent_registry.clone());
        invoker
            .invoke(
                "RecordingTool",
                json!({}),
                SubagentInvocationContext { parent_agent_id: None },
            )
            .await
            .expect("dispatch ok");

        let captured = captured.lock().unwrap();
        let inner = captured.as_ref().expect("RecordingTool::call ran");
        let registry_in_ctx = inner.as_ref().expect("subagent_registry was Some");
        assert!(
            Arc::ptr_eq(&parent_registry, registry_in_ctx),
            "RegistryToolInvoker must thread the parent Arc<ToolRegistry> into ToolUseContext.subagent_registry verbatim — this preserves the M4-05 recursion-lock contract across the dispatch boundary"
        );
    }
```

- [ ] **Step 2: Run test to verify it passes (Task 9 already implements the wiring)**

```bash
cargo test -p lingxi-tools --lib tool_invoker_impl::tests::registry_invoker_preserves_subagent_registry_arc_into_tool_use_ctx 2>&1 | tail -15
```
Expected: PASS. The Task 9 implementation already sets `subagent_registry: Some(self.registry.clone())`, so the `Arc::ptr_eq` holds.

If it FAILS (the assertion fires saying the Arcs differ), there is a regression in the Task 9 commit — re-read `tool_invoker_impl.rs::invoke` and verify the `subagent_registry` field is assigned exactly `Some(self.registry.clone())` (NOT `Some(Arc::new(...))`).

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/crates/tools/src/tool_invoker_impl.rs
git commit -m "$(cat <<'EOF'
test(M5-01 task 10): RegistryToolInvoker preserves recursion-lock Arc into ToolUseContext

A RecordingTool fixture captures the ToolUseContext.subagent_registry it
receives. The test asserts:

    Arc::ptr_eq(parent_registry, captured.subagent_registry.unwrap())

This makes the recursion-lock contract explicit at the dispatch boundary
(complementing M4-05's two Arc::ptr_eq tests which assert the contract at
the SubagentInheritance boundary).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Verify the two M4-05 critical `Arc::ptr_eq` tests still pass after Follow-up C

**Files:** verify only — no code change.

- [ ] **Step 1: Re-run the two M4-05 critical tests**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
cargo test -p lingxi-tools recursion_lock_child_inherits_parent_tool_registry_arc 2>&1 | tail -10
cargo test -p lingxi-tools budget_inheritance_child_inherits_parent_budget_arc 2>&1 | tail -10
```
Expected: BOTH pass.

**Why these still pass:** the M4-05 tests use a `MockSubagentSpawner` (`agent_test_support.rs`) that captures the `SubagentInheritance` bundle BEFORE any dispatch happens. They never call `RegistryToolInvoker::invoke`. The only way Task 9-10 could regress them is if the `RegistryToolInvoker::new` constructor or the `registry_arc()` accessor was modified — and neither were. The constructor and accessor are unchanged from M4-05.

- [ ] **Step 2: Run the integration suite + the new lib tests in parallel**

```bash
cargo test -p lingxi-tools 2>&1 | tail -30
```
Expected: ALL pass.

- [ ] **Step 3: Pin-commit the verification**

```bash
git commit --allow-empty -m "$(cat <<'EOF'
chore(M5-01 task 11): M4-05 Arc::ptr_eq invariants verified after Follow-up C

Post-RegistryToolInvoker-rewrite verification:
- recursion_lock_child_inherits_parent_tool_registry_arc: PASS
- budget_inheritance_child_inherits_parent_budget_arc:    PASS
- cargo test -p lingxi-tools: full suite PASS

Follow-up C (RegistryToolInvoker route) now functionally complete.
All three M5-01 follow-ups (A: runner pump, B: TaskOutput spool,
C: RegistryToolInvoker route) are wired and the M4-05 invariants hold.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Verification gate + tag `m5.1`

**Files:** none (workspace verification).

- [ ] **Step 1: Run full workspace test suite**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
cargo test --workspace 2>&1 | tail -40
```
Expected: ALL pass with the documented exception of **at most 2 pre-existing fs-watch flakes** (these are tracked from M2-05 and have been accepted at every prior milestone gate). Acceptable failure pattern:
- 0 or 1 or 2 flakes in `lingxi-platforms-posix::watch::tests::*` or `lingxi-filestate::watcher::tests::*` (specifically `*watch_emits_change*` or similar names; NOT in any other crate).

If ANY OTHER test fails:
- A failure in `lingxi-agent::runner::tests::*` → regression in Tasks 2-4; re-read the runner diff.
- A failure in `lingxi-tasks::handle::tests::*` → regression in Task 6; verify `InMemoryFs` was substituted cleanly.
- A failure in `lingxi-tools::tool_invoker_impl::tests::*` → regression in Task 9 or 10.
- A failure in `lingxi-tools::builtin::agent::tests::recursion_lock_*` or `budget_inheritance_*` → THE PRIMARY M4-05 LOCK REGRESSED — STOP and reconcile.

- [ ] **Step 2: Run clippy with `-D warnings` workspace-wide**

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30
```
Expected: zero warnings. Common cleanups likely needed after Tasks 2-10:
- `#[allow(dead_code)]` on the new test fixtures (`RecordingTool::name()` etc.) if clippy complains about unused trait-impl methods → REMOVE the allow; clippy generally tolerates trait-required methods marked unused.
- `clippy::needless_clone` on `self.registry.clone()` inside `invoke` → leave it; the `.clone()` is load-bearing for the recursion-lock invariant. Add `#[allow(clippy::needless_clone)]` on that line if the lint fires, with a `// Load-bearing: the cloned Arc is what the recursion-lock test inspects.` comment.
- `clippy::redundant_field_names` on the `tool_use_ctx` struct literal in `invoke` → fix by removing `name: name` shorthand if clippy prefers shorthand.

- [ ] **Step 3: Run fmt --check**

```bash
cargo fmt --all -- --check 2>&1 | tail -10
```
Expected: no diff. If diff is reported, run `cargo fmt --all` and commit the result as part of step 6 below.

- [ ] **Step 4: Run the three critical invariants one final time**

```bash
cargo test -p lingxi-tools recursion_lock_child_inherits_parent_tool_registry_arc 2>&1 | tail -10
cargo test -p lingxi-tools budget_inheritance_child_inherits_parent_budget_arc 2>&1 | tail -10
cargo test -p lingxi-tools registry_invoker_preserves_subagent_registry_arc_into_tool_use_ctx 2>&1 | tail -10
```
Expected: all 3 pass.

- [ ] **Step 5: Verify telemetry counts are UNCHANGED**

```bash
grep -n 'NAMES\.len(),$' lingxi-code/crates/telemetry/src/tengu/tool.rs
grep -n '134,$' lingxi-code/crates/telemetry/src/tengu/tool.rs
grep -n '25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 1' lingxi-code/crates/telemetry/src/tengu/mod.rs
```
Expected:
- `tengu::tool::NAMES.len() == 134` (the assertion is intact; M5-01 added zero new tool events).
- `ALL_EVENT_NAMES` total expression `25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 1 = 238` is intact.

If either count drifted, the executor accidentally added a telemetry constant — back out the addition; M5-01 emits NO new telemetry.

- [ ] **Step 6: Final commit (if any uncommitted noise from steps 1-5)**

```bash
git status
# If anything is uncommitted (e.g. fmt fixes, clippy allow attrs):
git add -A
git commit -m "$(cat <<'EOF'
chore(M5-01 task 12): verification gate green — pre-tag

Final tidy: fmt + clippy fixes (if any).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
If `git status` shows clean, SKIP this commit.

- [ ] **Step 7: Tag `m5.1`**

```bash
git tag m5.1 -m "M5-01: engine wiring close-out — runner pump + TaskOutput spool + RegistryToolInvoker route"
git tag -l m5.1
```
Expected: `m5.1` printed.

---

## Self-Review

**1. Spec coverage**

Spec §3 M5-01 row — 3 follow-ups mapped to tasks:
1. `agent::runner::run_subagent` stub → real reduce loop → **Tasks 2, 3, 4, 5** (4 tasks).
2. `TaskRegistryHandle::output` → real spool reader via `TaskOutputManager::read` → **Tasks 6, 7** (2 tasks).
3. `RegistryToolInvoker::invoke` → real dispatch via `ToolRegistry::call` → **Tasks 8, 9, 10, 11** (4 tasks).

Bridging tasks: Task 1 (baseline marker + reducer reason-string survey), Task 12 (verification gate + tag).

Total: 12 tasks. Matches the spec's "~12" budget for M5-01.

Spec §6.2 backward compatibility — the two M4-05 `Arc::ptr_eq` tests are explicitly re-run in Task 5 (after Follow-up A), Task 11 (after Follow-up C), and Task 12 step 4 (final gate). Both invariants are documented as load-bearing in the architecture preamble and called out in every relevant task.

**2. Placeholder scan**

Scanned for `TODO`, `TBD`, `fill in details`, "similar to Task N", "appropriate error handling", "add tests later", "implementation TBD". None remain — every step contains either the exact code (with byte-locked literals where applicable: `"run_subagent: event channel closed without terminal state"`, the `RegistryToolInvoker::new` constructor body, the `ToolUseContext` field list) or an explicit fallback recipe (e.g. "if `assistant_text` differs, grep for the current constructor name and substitute"; "if `ToolProgressSender::new` is a type alias, collapse to `let progress_tx = progress_tx;`").

The fallback recipes are necessary because three symbol names are theoretically subject to drift since the spec was written (commit `1dbb9b8`, 2026-05-25): `ConversationMessage::assistant_text`, `ToolProgressSender::new`, and the `Tool` trait method list. All three are pinned with explicit grep recipes the executor runs at task start. This is acceptable per the writing-plans skill — the plan documents EXACT code given current-HEAD signatures, with a verification step.

**3. Type consistency**

- `SubagentEvent` variants used: `Message { agent_id, message }`, `Completed { agent_id, result }`, `Killed { agent_id }`, `Failed { agent_id, error }`. Matches the enum at `lingxi-code/crates/agent/src/runner.rs:14-51`. `Progress` is NOT emitted by M5-01 (it lands when M5-04 wires streaming token counts).
- `lingxi_core::Event` variants consumed: `UserMessage { message_id, request_id, content }`, `UserInterrupt`, `UserExit`, `ApiStreamStart { request_id }`, `ApiStreamDelta { request_id, text }`, `ApiStreamEnd { request_id, final_message, usage }`. Matches `lingxi-code/crates/core/src/events.rs:14-100`.
- `ConversationState` variants pattern-matched: `Idle { session }`, `Terminated { session, reason }`. Other variants are not explicitly destructured (the loop only checks `is_terminal()`).
- `TaskOutputChunk { task_id, content, total_lines, truncated }` shape consistent across Tasks 6, 7 and the trait surface `lingxi-code/crates/traits/src/task_registry.rs:48-58`.
- `OutputOptions { offset, limit }` shape from `lingxi-code/crates/tasks/src/output_manager.rs:36-42`. Task 6's call uses `OutputOptions { offset, limit: None }`.
- `ToolInvokerError` variants used: `NotFound(String)`, `InvalidInput(String)`, `Internal(String)`. Matches `lingxi-code/crates/traits/src/tool_invoker.rs:29-41`.
- `ToolUseContext` field list (Task 9 body): `options, messages, tool_use_id, agent_id, content_replacement_state, session, subagent_registry`. Matches the M4-05 fixture at `agent.rs::tests::fresh_ctx_with_registry`.
- `Arc::ptr_eq` invocations: 4 total in this plan — 2 in M4-05's existing tests (preserved), 1 in Task 10's new `registry_invoker_preserves_subagent_registry_arc_into_tool_use_ctx`, 1 in the existing M4-05 `registry_invoker_preserves_arc_identity` test (preserved). All four hold throughout M5-01.

**4. Arc::ptr_eq invariant explicit callouts**

- Task 5 step 1 + Task 11 step 1 explicitly invoke:
  ```
  cargo test -p lingxi-tools recursion_lock_child_inherits_parent_tool_registry_arc
  cargo test -p lingxi-tools budget_inheritance_child_inherits_parent_budget_arc
  ```
  and require BOTH pass.
- Task 12 step 4 reruns the same two tests at the final gate.
- Architecture preamble lists the two tests by name + file location + line number and labels them "critical".
- The note "Why these still pass" in Task 11 step 1 explains the structural reason the M4-05 tests are insulated from the M5-01 changes (the mocks intercept the spawn surface; `RegistryToolInvoker::new` and `registry_arc()` are unchanged).

**Fix log:** none — no inconsistencies found during review.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-25-m5-01-engine-wiring-closeout.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration. Best for this plan because the 12 tasks cross three crates (`lingxi-agent`, `lingxi-tasks`, `lingxi-tools`) and each follow-up has its own verification cadence. Per-task review catches Arc-identity regressions early.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints. Acceptable because the task surface is contained and the diff is small (~150 LOC across 3 files + ~200 LOC of test code).

**Which approach?**
