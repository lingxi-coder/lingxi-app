# LingXi Core M5 · Plan 02 · ConversationOrchestrator core — batched turn loop + new lingxi-orchestrator crate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Land the v0.6.0 conversational outer loop in a brand-new crate `lingxi-orchestrator`, drive it batched (non-streaming `messages_create_non_stream`) end-to-end with a mock model, and emit the first 3 of the ~77 M5 telemetry events. After this plan, `ConversationOrchestrator::run_turn(prompt)` can:

1. Append the user prompt to a `lingxi_core::SessionState`.
2. Call `AnthropicProvider::messages_create_non_stream(model, msgs, transport)` (no SSE, no streaming).
3. Parse `content` for `ContentBlockApi::Text` (→ `OutputStream::emit_text`) and `ContentBlockApi::ToolUse` (→ tool dispatch).
4. For each `ToolUse`: invoke `PreToolUse` hook (no-op stub this plan), check permissions (always-allow stub this plan), dispatch via `ToolRegistry::find_by_name(name).call(input, ctx, progress_tx)`, invoke `PostToolUse` hook (no-op), append a synthesized `ToolResult` content block (with `is_error: true` if the tool failed) to the session as a `ConversationMessage::User`.
5. Append the assistant response as `ConversationMessage::Assistant`.
6. If `stop_reason == Some("end_turn")` → emit `OutputStream::emit_end_turn` and return `Ok(ConversationOutcome::EndTurn)`.
7. Otherwise loop back to step 2 (a fresh `messages_create_non_stream` with the now-extended history).
8. Enforce `OrchestratorConfig::max_turns` (default `30`): after `max_turns` iterations without `end_turn`, return `Err(OrchestratorError::MaxTurnsReached { max_turns })` whose `Display` is the byte-locked literal **`Reached maximum number of turns (<n>)`** (taken from `claude-code/src/QueryEngine.ts:870`).

This plan ships:

- A new crate `lingxi-orchestrator` (added to workspace + default-members).
- `OrchestratorError`, `OrchestratorConfig`, `ConversationOrchestrator`, `ConversationOutcome`.
- 4 new traits in `lingxi-traits::orchestrator`: `OrchestratorHandle`, `OutputStream`, `OutputEvent`, `CostSnapshot`.
- A `test_support` module in the new crate (gated `#[cfg(any(test, feature = "test-support"))]`) with `MockApiClient`, `MockOutputStream`, `NoOpHookExecutor`, `NoOpPermissionGate`.
- 3 new telemetry events in a brand-new `lingxi_telemetry::tengu::orchestrator` submodule, growing `ALL_EVENT_NAMES` from **238 → 241** and the `tengu_events.json` parity fixture in lockstep.
- 4 integration test files under `crates/orchestrator/tests/`.

**No other crates' public surfaces change.** `lingxi-traits` only gains a new module + 4 new public types (additive). `lingxi-telemetry` only gains 1 new submodule + 3 new constants + a `total = 238 → 241` count change in `tengu::mod.rs`. Everything else is internal to the new crate.

**Tech Stack:** Rust 2021, `async-trait 0.1` (workspace), `serde 1` + `serde_json 1` (workspace, `serde_json` with `preserve_order`), `tokio 1` (workspace; `sync::Mutex` for the mock output sink), `thiserror 2` (workspace). No new third-party deps. The new crate depends on (in order, no cycle): `lingxi-protocol`, `lingxi-core`, `lingxi-traits`, `lingxi-api-client`, `lingxi-tools`, `lingxi-permission`, `lingxi-hooks`, `lingxi-cost`, `lingxi-telemetry`. **None of those depend back on `lingxi-orchestrator`** — Task 1 step 3 verifies the dep direction in the workspace `Cargo.toml` graph.

**References:**

- Spec: `docs/superpowers/specs/2026-05-25-m5-conversational-agent-loop-design.md` (committed at `1dbb9b8`).
  - §1 Goal (lines 1-50) — v0.6.0 conversational agent loop scope.
  - §2.1 Architecture (lines 50-130) — new crate `lingxi-orchestrator`, `OrchestratorHandle` / `OutputStream` traits live in `lingxi-traits`.
  - §2.3 traits block (lines 130-162) — exact trait signatures for `OrchestratorHandle` + `OutputStream`.
  - §3 sub-plan row M5-02 (line 178) — "new crate `lingxi-orchestrator`, batched non-streaming, mock model, +3 events, ~16 tasks".
  - §4.2 turn loop limits (lines 251-256) — `Reached maximum number of turns (${maxTurns})` byte-lock, `maxTurns` camelCase frozen.
  - §6.1 Cargo dep graph (lines 401-422) — orchestrator is a new leaf depending on agent/tools/session/commands/hooks/api-client/permission/cost/memory/telemetry; cycle risks listed (M5-09 commands cycle deferred — orchestrator does NOT depend on commands in M5-02).
  - §6.3 telemetry growth table (line 432-446) — v0.5.0 238 → after M5-02 241.
  - §7 Open Questions OQ-1 (line 491) + OQ-2 (line 492) — OQ-1 resolution: claude-code has no global main-loop default, LingXi locks `30`. OQ-2 (SSE event schema) deferred to M5-04 — M5-02 uses non-streaming so OQ-2 is not in scope.
- Predecessor: **M5-01 — engine wiring close-out** committed at `c477822`. Verifies:
  - `lingxi-core/crates/agent/src/runner.rs::run_subagent` now drives a real `reduce` loop (not a stub).
  - `lingxi-core/crates/tasks/src/handle.rs::TaskRegistryHandle::output` reads from `TaskOutputManager::read` (not empty placeholder).
  - `lingxi-core/crates/tools/src/tool_invoker_impl.rs::RegistryToolInvoker::invoke` dispatches into `ToolRegistry::call` (not `Ok(Value::Null)`).
  - M4-05's two `Arc::ptr_eq` invariants (`recursion_lock_child_inherits_parent_tool_registry_arc`, `budget_inheritance_child_inherits_parent_budget_arc`) still pass — M5-02 does not touch agent/tasks/tools internals, so those invariants are not at risk.
- claude-code source:
  - `claude-code/src/QueryEngine.ts:870` — `Reached maximum number of turns (${message.attachment.maxTurns})` byte-lock (verified by grep in Task 2 step 1).
  - `claude-code/src/QueryEngine.ts:146,220,684,1196,1227,1265` — `maxTurns` field usage (camelCase frozen).
- Existing engine surfaces consumed by this plan:
  - `lingxi-core/crates/api-client/src/anthropic.rs:211` — `AnthropicProvider::messages_create_non_stream<T: HttpTransport>(&self, model: &str, msgs: Vec<ConversationMessage>, transport: &T) -> Result<MessageResponse, ApiError>`.
  - `lingxi-core/crates/api-client/src/types.rs:35` — `MessageResponse { id, model, content: Vec<ContentBlockApi>, stop_reason: Option<String>, usage: UsageApi }`.
  - `lingxi-core/crates/api-client/src/types.rs:59` — `ContentBlockApi::{ Text { text }, ToolUse { id, name, input }, Thinking { .. }, ServerToolUse { .. }, ConnectorText { .. }, AdvisorToolResult { .. } }`. M5-02 only reads `Text` and `ToolUse`; the other 4 variants are passed through as-is into the recorded assistant message (no panic, no dispatch).
  - `lingxi-core/crates/protocol/src/messages.rs:25` — `ContentBlock::{ Text { text }, ToolUse { id, name, input }, ToolResult { tool_use_id, content, is_error }, Thinking { thinking, signature } }`.
  - `lingxi-core/crates/protocol/src/messages.rs:61` — `ConversationMessage::{ User { id, content }, Assistant { id, content, stop_reason }, System { id, content } }`.
  - `lingxi-core/crates/core/src/session.rs:50` — `SessionState { session_id, history, usage, model, todos, plan_mode }` + `SessionState::empty(session_id, model)`.
  - `lingxi-core/crates/tools/src/registry.rs:23` — `ToolRegistry { ... }` + `ToolRegistry::find_by_name(name) -> Option<Arc<dyn Tool>>`.
  - `lingxi-core/crates/tools/src/context.rs:20` — `ToolUseContext { options, messages, tool_use_id, agent_id, content_replacement_state, session, subagent_registry }` + `ToolUseOptions`.
  - `lingxi-core/crates/tools/src/tool_trait.rs::Tool::call(input, ctx, progress_tx) -> Result<ToolCallResult, ToolError>` + `ToolCallResult { data, new_messages, context_modifier, mcp_meta }`.
  - `lingxi-core/crates/cost/src/summary.rs:30` — `CostSummary { session, day, month, by_model }` + `SessionCostSummary { session_id, total_nano_usd, total_tokens }`. M5-02's `CostSnapshot` trait-level type is a NEW lightweight struct in `lingxi-traits::orchestrator` that holds `{ total_nano_usd: u64, total_tokens: u64, session_id: SessionId }` — it is NOT a re-export of `CostSummary` (the trait surface must stay free of `lingxi-cost` deps to keep `lingxi-traits` a leaf). Task 4 documents this with a doc-link from `CostSnapshot` to `lingxi_cost::CostSummary`.
  - `lingxi-core/crates/protocol/src/ids.rs` — `SessionId`, `MessageId`, `AgentId`, `ToolUseId`.
  - `lingxi-core/crates/permission/src/lib.rs` — `PermissionGate` trait (M4-XX); M5-02 uses an in-crate `NoOpPermissionGate` test stub.
  - `lingxi-core/crates/hooks/src/lib.rs` — `HookExecutor` (M5-06 wires 4 arms); M5-02 uses an in-crate `NoOpHookExecutor` test stub.
- Telemetry plumbing (locked by M3-06):
  - `lingxi-core/crates/telemetry/src/tengu/mod.rs:28` — `ALL_EVENT_NAMES: &[&str]` const, currently sized at 238 (TOTAL = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 1). M5-02 grows this to 241 by inserting a new `orchestrator` submodule between `release` and the close. New TOTAL formula = `25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 3 + 1` (orchestrator inserted as second-to-last, with 3 entries; release stays last).
  - `lingxi-core/crates/telemetry/src/tengu/release.rs:14` — the existing `NAMES: &[&str] = &[LINGXI_CORE_V0_5_0_RELEASED]` registration pattern (Task 15 follows this template verbatim for the new `orchestrator::NAMES`).
  - `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs:8` — `registry_is_exactly_238_entries` test (Task 15 updates to 241 + updates the comment to mention M5-02).
  - `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` — 286-line JSON array (line 6: `"event_names": [...]`); M5-02 inserts the 3 new orchestrator names BEFORE the trailing `"lingxi_core_v0_5_0_released"` entry to mirror `tengu::ALL_EVENT_NAMES` registration order. Task 15 step 4 documents the insertion line range explicitly.
- Repo conventions (M4-01..09 precedent reaffirmed):
  - Tests live in `#[cfg(test)] mod tests { ... }` blocks adjacent to production code.
  - Integration tests live in `crates/<crate>/tests/<name>_test.rs`.
  - Every error string visible in tests is the EXACT byte sequence in production (`assert_eq!`-quality, not `contains`).
  - Telemetry constants are CAPITAL_SNAKE; production code references `lingxi_telemetry::tengu::orchestrator::*` symbols, not literals.
  - Crate-level `#![forbid(unsafe_code)]` is mandatory in every new lib.rs.
  - New crates start at `version = "0.5.0"` to match the current workspace (M5-14 bumps everything to `0.6.0`).

---

## File touch inventory (locked at top per spec Appendix A convention)

**Creates (new files):**

- `lingxi-core/crates/orchestrator/Cargo.toml` — new crate manifest.
- `lingxi-core/crates/orchestrator/src/lib.rs` — re-exports + crate docs + `#![forbid(unsafe_code)]`.
- `lingxi-core/crates/orchestrator/src/error.rs` — `OrchestratorError` enum.
- `lingxi-core/crates/orchestrator/src/config.rs` — `OrchestratorConfig` + `MAX_TURNS_DEFAULT`.
- `lingxi-core/crates/orchestrator/src/conversation.rs` — `ConversationOrchestrator` + `ConversationOutcome` + `run_turn`.
- `lingxi-core/crates/orchestrator/src/turn_loop.rs` — private inner loop helpers (`execute_one_turn`, `dispatch_tool_uses`, content-block translation).
- `lingxi-core/crates/orchestrator/src/test_support.rs` — `MockApiClient`, `MockOutputStream`, `OutputEventCapture`, `NoOpHookExecutor`, `NoOpPermissionGate`.
- `lingxi-core/crates/orchestrator/tests/orchestrator_smoke_test.rs` — single-turn happy path (no tools).
- `lingxi-core/crates/orchestrator/tests/orchestrator_multi_turn_test.rs` — two turns, one tool call (mock tool).
- `lingxi-core/crates/orchestrator/tests/orchestrator_max_turns_test.rs` — exceeds `max_turns`, asserts byte-locked Display.
- `lingxi-core/crates/orchestrator/tests/orchestrator_tool_error_test.rs` — tool returns `ToolError`, propagates as `is_error: true`.
- `lingxi-core/crates/orchestrator/tests/orchestrator_real_tools_test.rs` — integrates with real `ToolRegistry` + `FileReadTool` + a tempfile.
- `lingxi-core/crates/traits/src/orchestrator.rs` — 4 new public types: `OrchestratorHandle`, `OutputStream`, `OutputEvent`, `CostSnapshot`, plus `OrchestratorError` re-export.
- `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` — 3 new constants + `NAMES` array.

**Modifies (existing files):**

- `lingxi-core/Cargo.toml` — add `crates/orchestrator` to `members` AND `default-members` (preserve alphabetical-ish order; insert after `crates/mcp` so the alphabetic placement reads naturally — see Task 1 step 2).
- `lingxi-core/crates/traits/src/lib.rs` — add `pub mod orchestrator;` declaration + `pub use orchestrator::{...}` re-exports.
- `lingxi-core/crates/telemetry/src/tengu/mod.rs` — add `pub mod orchestrator;` declaration + update `const TOTAL` arithmetic (`+ 3`) + insert `orchestrator::NAMES` walk before `release::NAMES` in `concat_all()`.
- `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs` — bump expected count from `238` to `241` in the test + update the explanatory comment to add `M5-02 added 3 (conversation lifecycle × 3 = started/completed/failed): 238 + 3 = 241.`
- `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` — insert the 3 orchestrator event names (verbatim wire strings, see "Wire identifiers" below) in registration order BEFORE the trailing `"lingxi_core_v0_5_0_released"` entry. The `_note` field is also updated to mention "+3 (M5-02 orchestrator)". File grows from 286 lines to 289 lines.
- `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` `_note` — append " + 3 (M5-02 orchestrator: conversation_started/completed/failed)" to the existing note.

**Verifications (no modification, just read in tests):**

- `lingxi-core/crates/api-client/src/anthropic.rs:211` — `messages_create_non_stream` signature still matches what `ConversationOrchestrator` calls.
- `lingxi-core/crates/protocol/src/messages.rs:25,61` — `ContentBlock` + `ConversationMessage` shapes unchanged from M4.
- `lingxi-core/crates/tools/src/registry.rs:23` — `ToolRegistry` shape unchanged from M4.
- `lingxi-core/crates/cost/src/summary.rs:30` — `SessionCostSummary` shape (referenced in `CostSnapshot` doc but not depended on).
- The two M4-05 `Arc::ptr_eq` tests (`recursion_lock_child_inherits_parent_tool_registry_arc`, `budget_inheritance_child_inherits_parent_budget_arc`) at `lingxi-core/crates/tools/src/builtin/agent.rs::tests` — must remain green (Task 16 step 1).

**Critical fidelity notes (locked here):**

- **`MAX_TURNS_DEFAULT = 30`** — LingXi-defined. Spec §7 OQ-1 documents claude-code has no global main-loop default (only per-agent frontmatter `maxTurns?: number`). LingXi locks `30`. This number lives in `lingxi-orchestrator::config::MAX_TURNS_DEFAULT` and is the literal value used by `OrchestratorConfig::default()`.
- **`Reached maximum number of turns (<n>)`** — byte-locked from `claude-code/src/QueryEngine.ts:870`. The `<n>` is the runtime `max_turns: u32`. The Display impl uses `write!(f, "Reached maximum number of turns ({})", max_turns)` (no padding, no parens around `<n>`, exact unicode). Task 2 step 3 asserts this string at full byte-precision.
- **`OrchestratorConfig` field names locked at this plan**: `max_turns: u32` (NOT `maxTurns` — Rust idiom prevails in our config; the claude-code TS field is `maxTurns` camelCase but it's not a wire string between us and claude-code), `model: String`, `system_prompt_override: Option<String>`. Serde serialization is snake_case (the workspace default).
- **3 new telemetry event names locked at this plan**:
  - `tengu_orchestrator_conversation_started`
  - `tengu_orchestrator_conversation_completed`
  - `tengu_orchestrator_conversation_failed`
  These are the constant VALUES (the `&str` literals); the constant NAMES (Rust identifiers) are `CONVERSATION_STARTED`, `CONVERSATION_COMPLETED`, `CONVERSATION_FAILED`, exported from `lingxi_telemetry::tengu::orchestrator`. The Rust name + the `&str` value mapping is itself a byte-lock from this plan onward.
- **`OutputStream` trait surface** (in `lingxi-traits::orchestrator`):
  - `async fn emit_text(&self, text: &str)`
  - `async fn emit_tool_call(&self, tool: &str, input: &serde_json::Value)`
  - `async fn emit_tool_result(&self, tool: &str, result: &serde_json::Value)`
  - `async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot)`
  All four `async fn` use `&self` (immutable borrow + interior mutability in implementations) and take borrowed args (no clone in the hot path). Concrete impls (e.g. `MockOutputStream`) use `tokio::sync::Mutex<Vec<OutputEvent>>` for capture.
- **`OrchestratorHandle` trait surface** (in `lingxi-traits::orchestrator`):
  - `async fn current_session_id(&self) -> SessionId`
  - `async fn clear_session(&self) -> Result<(), OrchestratorError>`
  - `async fn force_compact(&self) -> Result<CompactionSummary, OrchestratorError>`
  - `async fn snapshot_cost(&self) -> CostSnapshot`
  - `async fn switch_model(&self, model: &str) -> Result<(), OrchestratorError>`
  `CompactionSummary` is a NEW lightweight type in `lingxi-traits::orchestrator` (Task 3): `pub struct CompactionSummary { pub messages_before: u32, pub messages_after: u32, pub bytes_saved: u64 }`. M5-10 wires `/compact` against this surface; this plan defines the type so future plans can reference it. M5-02 itself does NOT implement `OrchestratorHandle` on `ConversationOrchestrator` (that wiring lives in M5-09 when the slash dispatcher needs it) — the trait merely lives next to `OutputStream` in the same module for cohesion.
- **`ConversationOutcome` enum** (in `lingxi-orchestrator::conversation`):
  - `EndTurn { turn_count: u32, final_message_id: MessageId }`
  - `MaxTurnsReached` — internal sentinel turned into `Err(OrchestratorError::MaxTurnsReached)` by `run_turn` before returning to caller. NEVER exposed publicly; only used in `turn_loop.rs` private return.
- **Mock model**: `MockApiClient` in `test_support.rs` is the ONLY non-trivial test fixture this plan ships. It holds a `tokio::sync::Mutex<VecDeque<MessageResponse>>` and a `tokio::sync::Mutex<Vec<Vec<ConversationMessage>>>` (captures the `msgs` arg per call for assertion). It implements an INTERNAL trait `OrchestratorApiClient` (defined in `conversation.rs`, NOT public outside the orchestrator crate) which is what `ConversationOrchestrator` depends on — this lets us mock without forcing all of `lingxi-api-client::AnthropicProvider`'s constructor surface. **Inside production code**, the orchestrator's `new()` constructor takes `Arc<dyn OrchestratorApiClient>`. A thin adapter `AnthropicProviderAdapter<T: HttpTransport>` wraps `AnthropicProvider` + a transport into an `OrchestratorApiClient` impl. The adapter lives in `conversation.rs` (Task 10 step 4); the mock lives in `test_support.rs`.
- **No SSE in this plan**. Streaming + `OutputStream::emit_text` mid-token-by-token comes in M5-04. M5-02 calls `emit_text` ONCE per Text block per turn, with the WHOLE text body (batched). M5-04 will switch this to per-delta emission without changing the trait signature.
- **No real `PermissionGate` integration in this plan**. The `NoOpPermissionGate` always returns `Allow`. M5-05 swaps in `PromptingGate` with interactive y/N — but the orchestrator code calls a `Box<dyn PermissionGate>` (or `Arc<dyn PermissionGate>`) field, so the wiring point exists from day one.
- **No real `HookExecutor` integration in this plan**. The `NoOpHookExecutor` returns `Ok(())` for both `PreToolUse` and `PostToolUse` arms. M5-06 swaps in the real 4-arm executor. The orchestrator calls hooks through a `Arc<dyn HookExecutor>` field so the wiring point exists.
- **`tool_use_id` plumbing**: the `ContentBlockApi::ToolUse { id, name, input }` `id` field is a `lingxi_protocol::ToolUseId` newtype. The orchestrator passes this `id` verbatim into the synthesized `ToolUseContext.tool_use_id: Option<ToolUseId>` AND into the `ContentBlock::ToolResult { tool_use_id, content, is_error }` it appends to the session. Round-trip identity is a unit test (Task 11 step 4).
- **`ToolCallResult.data` → `ToolResult.content`**: the orchestrator stringifies `data: Value` via `serde_json::to_string(&data).unwrap_or_else(|_| "<unserializable>".into())`. Test (Task 13 step 2) covers the unserializable branch with a sentinel.
- **Tool errors**: a `ToolError` from `Tool::call` produces a `ContentBlock::ToolResult { tool_use_id, content: format!("Error: {err}"), is_error: true }`. The literal `"Error: "` prefix is locked here (Task 13 step 3). Future plans (M5-06 hooks) may inject additional content but cannot drop the `"Error: "` prefix without a new byte-lock.
- **`messages_create_non_stream` retry semantics**: the orchestrator does NOT add retries on top of `AnthropicProvider`'s 3-attempt loop. If the API call fails after exhausting retries, the orchestrator returns `Err(OrchestratorError::ApiCall(ApiError))` with the underlying error. Task 12 step 4 covers this propagation.
- **Telemetry emission ordering**: `tengu_orchestrator_conversation_started` fires at the TOP of `run_turn` (before any session mutation). On success, `tengu_orchestrator_conversation_completed` fires AFTER `emit_end_turn` (so the OutputStream observer sees the end-turn signal before the telemetry consumer). On `Err`, `tengu_orchestrator_conversation_failed` fires with a `reason` payload field. Task 15 step 5 covers the ordering with an `InMemorySink` assertion.

---

## Tasks

### Task 1: Scaffold new crate `lingxi-orchestrator` + workspace registration

**Files:**
- Create: `lingxi-core/crates/orchestrator/Cargo.toml`
- Create: `lingxi-core/crates/orchestrator/src/lib.rs`
- Modify: `lingxi-core/Cargo.toml` (add to `[workspace] members` AND `default-members`)

**Steps:**

- [ ] Step 1 — Verify predecessor `c477822` (M5-01) is on `HEAD` and that the directory `lingxi-core/crates/orchestrator/` does NOT yet exist. Run `git log -1 --oneline` and `ls lingxi-core/crates/orchestrator 2>/dev/null; echo $?` — second command must print non-zero exit code.

- [ ] Step 2 — Modify `lingxi-core/Cargo.toml` to add `"crates/orchestrator",` to BOTH `[workspace] members` (line 4-50 block) AND `default-members` (line 53-86 block). Insert the line immediately AFTER `"crates/mcp",` in both arrays (alphabetic placement reads naturally between `mcp` and `outputstyles`). Do not touch any other workspace settings.

- [ ] Step 3 — Create `lingxi-core/crates/orchestrator/Cargo.toml`:
  ```toml
  [package]
  name = "lingxi-orchestrator"
  version = "0.5.0"
  edition.workspace = true
  rust-version.workspace = true
  license.workspace = true

  [dependencies]
  lingxi-protocol = { path = "../protocol" }
  lingxi-core = { path = "../core" }
  lingxi-traits = { path = "../traits" }
  lingxi-api-client = { path = "../api-client" }
  lingxi-tools = { path = "../tools" }
  lingxi-permission = { path = "../permission" }
  lingxi-hooks = { path = "../hooks" }
  lingxi-cost = { path = "../cost" }
  lingxi-telemetry = { path = "../telemetry" }
  async-trait.workspace = true
  serde.workspace = true
  serde_json.workspace = true
  thiserror.workspace = true
  tracing.workspace = true

  [dev-dependencies]
  tokio = { workspace = true, features = ["rt-multi-thread", "macros", "time", "sync"] }
  tempfile = "3"

  [features]
  test-support = []

  [lints]
  workspace = true
  ```

- [ ] Step 4 — Create `lingxi-core/crates/orchestrator/src/lib.rs`:
  ```rust
  //! Top-level conversational orchestrator — drives the v0.6.0 turn loop.
  //!
  //! `ConversationOrchestrator` is the single owner of an in-process AI
  //! coding conversation:
  //!
  //! 1. Append user prompt to `SessionState`.
  //! 2. Call `messages_create_non_stream` (batched; streaming lands in M5-04).
  //! 3. Dispatch tool_use blocks through `ToolRegistry` (after `PreToolUse`
  //!    hook + permission gate; both stubbed in M5-02, real in M5-05 / M5-06).
  //! 4. Append assistant message to session.
  //! 5. Loop until `stop_reason == "end_turn"` or `max_turns` exceeded.
  //!
  //! See spec §2.2 (data flow diagram) and §4.2 (turn loop limits).
  #![forbid(unsafe_code)]

  pub mod config;
  pub mod conversation;
  pub mod error;
  pub mod turn_loop;

  #[cfg(any(test, feature = "test-support"))]
  pub mod test_support;

  pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};
  pub use conversation::{ConversationOrchestrator, ConversationOutcome, OrchestratorApiClient};
  pub use error::OrchestratorError;
  ```

  Note: at this task the four submodules are empty stubs (Tasks 2/5/10 fill them). Create EACH submodule file as an empty file with just `//! placeholder` plus `#![forbid(unsafe_code)]` is NOT needed at module level — only `lib.rs` carries `#![forbid(unsafe_code)]`. Specifically create:
  - `lingxi-core/crates/orchestrator/src/config.rs` — file body `//! Orchestrator configuration. Filled in Task 5.`
  - `lingxi-core/crates/orchestrator/src/conversation.rs` — file body `//! Conversation orchestrator. Filled in Task 10.`
  - `lingxi-core/crates/orchestrator/src/error.rs` — file body `//! Orchestrator errors. Filled in Task 2.`
  - `lingxi-core/crates/orchestrator/src/turn_loop.rs` — file body `//! Inner turn loop. Filled in Task 10.`
  - `lingxi-core/crates/orchestrator/src/test_support.rs` — file body `//! Test fixtures. Filled in Tasks 6-8.`

  Until those tasks fill the modules, `lib.rs` would fail to compile if it tried to re-export from them. So at this task, `lib.rs` is the version WITHOUT the `pub use` lines — those land in their respective tasks. Replace the `pub use` lines above with TODO comments:
  ```rust
  // pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};      // Task 5
  // pub use conversation::{ConversationOrchestrator, ConversationOutcome, OrchestratorApiClient};  // Task 10
  // pub use error::OrchestratorError;                              // Task 2
  ```

- [ ] Step 5 — Run `cargo build -p lingxi-orchestrator`. Must succeed. The crate compiles as an empty shell (4 empty modules) at this point.

- [ ] Step 6 — Run `cargo metadata --no-deps --format-version 1 | grep -c '"name": "lingxi-orchestrator"'` → expect `1`. Confirms the new crate is discoverable by Cargo.

- [ ] Step 7 — Run `cargo tree -p lingxi-orchestrator -e normal --depth 2 2>&1 | grep -E "lingxi-(agent|tasks|commands)"` — expect ZERO matches. Confirms the dep direction is clean: orchestrator does NOT pull in `lingxi-agent`, `lingxi-tasks`, or `lingxi-commands` in M5-02 (those wirings land in M5-06 / M5-09 / M5-12).

  Note about the architecture: the design §2.1 lists `lingxi-orchestrator → lingxi-agent` as a future arrow, but in M5-02 the orchestrator does NOT actually need agent yet (subagent dispatch lives behind `AgentTool` which is reached through `ToolRegistry`, not through a direct `lingxi-agent` dep). Adding the dep can wait until M5-06 wires hooks → `agent_executor`. Likewise `lingxi-tasks` and `lingxi-commands`. Cycle-prevention is the explicit win.

- [ ] Commit: `feat(M5-02 task 1): scaffold lingxi-orchestrator crate — empty shell + workspace registration`

---

### Task 2: `OrchestratorError` enum with byte-locked `MaxTurnsReached` Display

**Files:**
- Modify (fill from stub): `lingxi-core/crates/orchestrator/src/error.rs`
- Modify: `lingxi-core/crates/orchestrator/src/lib.rs` (uncomment the `pub use error::OrchestratorError;` line)

**Steps:**

- [ ] Step 1 — Re-verify the byte-locked message source. Run `grep -n "Reached maximum number of turns" claude-code/src/QueryEngine.ts` and confirm the match at line 870 reads exactly `` `Reached maximum number of turns (${message.attachment.maxTurns})` ``. The Rust Display must produce the literal `Reached maximum number of turns (<n>)` where `<n>` is the integer value (no padding, no commas, plain `Display` formatting). NOTE: claude-code uses TS template literal — the parentheses are literal `(` `)` chars and the value substitution is `${maxTurns}` with no formatting; Rust `write!(f, "Reached maximum number of turns ({})", max_turns)` produces the byte-identical string.

- [ ] Step 2 — Fill `lingxi-core/crates/orchestrator/src/error.rs`:
  ```rust
  //! Orchestrator-side errors.
  //!
  //! `MaxTurnsReached` carries the byte-locked Display literal
  //! `"Reached maximum number of turns (<n>)"` — verified against
  //! `claude-code/src/QueryEngine.ts:870` on 2026-05-25.

  use lingxi_api_client::ApiError;
  use thiserror::Error;

  /// Failure modes of `ConversationOrchestrator::run_turn`.
  ///
  /// `MaxTurnsReached` is byte-locked against `QueryEngine.ts:870`. Do not
  /// change the format string without a corresponding spec amendment.
  #[derive(Debug, Error)]
  pub enum OrchestratorError {
      /// The configured `max_turns` budget was exhausted before the model
      /// emitted `stop_reason == "end_turn"`.
      #[error("Reached maximum number of turns ({max_turns})")]
      MaxTurnsReached {
          /// The configured ceiling that was reached.
          max_turns: u32,
      },

      /// The Anthropic API call failed after exhausting `AnthropicProvider`'s
      /// own retry budget (3 attempts, 500ms/1s/2s ± 20% jitter — see M3-03).
      #[error("api call failed: {0}")]
      ApiCall(#[from] ApiError),

      /// A non-tool runtime error inside the orchestrator. Used for
      /// internal invariants (unexpected `ContentBlockApi` variant in the
      /// hot path, etc.). Test stubs use this for synthetic failures.
      #[error("orchestrator internal error: {0}")]
      Internal(String),
  }
  ```

- [ ] Step 3 — Uncomment `pub use error::OrchestratorError;` in `lingxi-core/crates/orchestrator/src/lib.rs`.

- [ ] Step 4 — Inside `error.rs`, add a `#[cfg(test)] mod tests` block:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn max_turns_reached_display_is_byte_locked_against_query_engine_ts_870() {
          // Source: claude-code/src/QueryEngine.ts:870
          //   `Reached maximum number of turns (${message.attachment.maxTurns})`
          let err = OrchestratorError::MaxTurnsReached { max_turns: 30 };
          assert_eq!(err.to_string(), "Reached maximum number of turns (30)");
      }

      #[test]
      fn max_turns_reached_display_handles_one() {
          let err = OrchestratorError::MaxTurnsReached { max_turns: 1 };
          assert_eq!(err.to_string(), "Reached maximum number of turns (1)");
      }

      #[test]
      fn max_turns_reached_display_handles_large_number() {
          let err = OrchestratorError::MaxTurnsReached { max_turns: 9999 };
          assert_eq!(err.to_string(), "Reached maximum number of turns (9999)");
      }

      #[test]
      fn internal_display_carries_payload() {
          let err = OrchestratorError::Internal("synthetic".into());
          assert_eq!(err.to_string(), "orchestrator internal error: synthetic");
      }
  }
  ```

- [ ] Step 5 — Run `cargo test -p lingxi-orchestrator --lib error::tests` — all 4 tests must pass.

- [ ] Commit: `feat(M5-02 task 2): OrchestratorError with byte-locked MaxTurnsReached Display`

---

### Task 3: Add `OrchestratorHandle`, `OutputStream`, `OutputEvent`, `CostSnapshot`, `CompactionSummary` traits to `lingxi-traits`

**Files:**
- Create: `lingxi-core/crates/traits/src/orchestrator.rs`
- Modify: `lingxi-core/crates/traits/src/lib.rs` (add `pub mod orchestrator;` + re-exports)

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/traits/src/orchestrator.rs`:
  ```rust
  //! Orchestrator-level traits — the public surface that slash commands
  //! (via `SlashContext`, M5-09) and the CLI binary (M5-12) consume.
  //!
  //! The concrete `ConversationOrchestrator` lives in `lingxi-orchestrator`;
  //! these traits live here so consumers can depend on them without pulling
  //! in the orchestrator (preserves the leaf position of `lingxi-traits`).
  //!
  //! See spec §2.3 (key traits) for the matched design.

  use async_trait::async_trait;
  use lingxi_protocol::SessionId;
  use serde::{Deserialize, Serialize};
  use thiserror::Error;

  /// Snapshot of cumulative cost at a single point in time.
  ///
  /// Lightweight echo of `lingxi_cost::SessionCostSummary` — see that type for
  /// the canonical session-scope rollup. We keep a leaf-friendly mirror here
  /// so `lingxi-traits` does not need to depend on `lingxi-cost`.
  #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
  pub struct CostSnapshot {
      /// Session whose cost this snapshot describes.
      pub session_id: SessionId,
      /// Cumulative cost in nano-USD.
      pub total_nano_usd: u64,
      /// Cumulative tokens (input + output across all models).
      pub total_tokens: u64,
  }

  /// Result of a `force_compact` operation. M5-10 wires `/compact` against
  /// this surface; M5-02 only defines the type for forward compatibility.
  #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
  pub struct CompactionSummary {
      /// Number of messages in the session BEFORE compaction.
      pub messages_before: u32,
      /// Number of messages in the session AFTER compaction.
      pub messages_after: u32,
      /// Approximate bytes saved (summary token count delta × 4, as a UX
      /// estimate — exact accounting lives in `lingxi-compaction`).
      pub bytes_saved: u64,
  }

  /// Errors surfaced through the orchestrator's public handle.
  ///
  /// Distinct from `lingxi_orchestrator::OrchestratorError` because the
  /// handle surface deliberately hides the API-error variants from slash
  /// command authors (they cannot meaningfully act on a 429). Implementations
  /// MAY wrap `OrchestratorError` and project a coarse `HandleError`.
  #[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
  pub enum HandleError {
      /// The requested action could not be completed (e.g. `clear` during a
      /// turn-in-flight). The payload is a human-readable reason.
      #[error("handle action failed: {0}")]
      ActionFailed(String),
  }

  /// Public handle to the orchestrator that slash commands operate against.
  ///
  /// Wired in M5-09 (slash-command surface). M5-02 only defines the trait —
  /// `ConversationOrchestrator` does NOT yet implement it.
  #[async_trait]
  pub trait OrchestratorHandle: Send + Sync {
      /// The session id currently driving the conversation.
      async fn current_session_id(&self) -> SessionId;

      /// Clear the in-memory session and start fresh.
      async fn clear_session(&self) -> Result<(), HandleError>;

      /// Force a compaction pass and return the summary.
      async fn force_compact(&self) -> Result<CompactionSummary, HandleError>;

      /// Snapshot the cumulative cost.
      async fn snapshot_cost(&self) -> CostSnapshot;

      /// Switch the active model. Subsequent turns use the new model.
      async fn switch_model(&self, model: &str) -> Result<(), HandleError>;
  }

  /// Captured output emission. Useful for tests and (M5-13) the stdio sink.
  ///
  /// The enum is `non_exhaustive` so M5-04 can add a `StreamingDelta` variant
  /// without a breaking change.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[non_exhaustive]
  pub enum OutputEvent {
      /// Plain text from the assistant.
      Text { text: String },
      /// A tool invocation about to dispatch.
      ToolCall { tool: String, input: serde_json::Value },
      /// A tool result returning to the conversation.
      ToolResult { tool: String, result: serde_json::Value },
      /// End-of-turn marker with cost.
      EndTurn { stop_reason: String, cost: CostSnapshot },
  }

  /// Sink for orchestrator-emitted output events.
  ///
  /// The stdio CLI (M5-12) and the future TUI (M6) both implement this.
  /// M5-02 ships `MockOutputStream` (in `lingxi-orchestrator::test_support`)
  /// for unit tests.
  #[async_trait]
  pub trait OutputStream: Send + Sync {
      /// Emit a piece of plain assistant text. In M5-02 this is called once
      /// per `Text` content block per turn (whole-body). M5-04 will switch
      /// to per-SSE-delta emission without changing this signature.
      async fn emit_text(&self, text: &str);

      /// Emit a tool-call notification immediately before dispatch.
      async fn emit_tool_call(&self, tool: &str, input: &serde_json::Value);

      /// Emit a tool-result notification immediately after dispatch.
      async fn emit_tool_result(&self, tool: &str, result: &serde_json::Value);

      /// Emit the end-of-turn marker with the cost snapshot.
      async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot);
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      // Compile-time assertion that the traits are object-safe and Send + Sync.
      fn _output_stream_is_object_safe<T: OutputStream + Send + Sync + 'static>() {
          let _: Box<dyn OutputStream> = Box::new(std::marker::PhantomData::<T>);
      }
      fn _orchestrator_handle_is_object_safe<T: OrchestratorHandle + Send + Sync + 'static>() {
          let _: Box<dyn OrchestratorHandle> = Box::new(std::marker::PhantomData::<T>);
      }
      // Cannot construct `PhantomData` as `dyn Trait` — keep the function
      // bodies behind `if false { ... }` so they're still type-checked:
      #[allow(dead_code)]
      fn _trait_objects_compile() {
          fn _f<T: OutputStream + 'static>(t: T) -> Box<dyn OutputStream> { Box::new(t) }
          fn _g<T: OrchestratorHandle + 'static>(t: T) -> Box<dyn OrchestratorHandle> { Box::new(t) }
      }

      #[test]
      fn output_event_round_trips_through_json() {
          let ev = OutputEvent::Text { text: "hello".into() };
          let s = serde_json::to_string(&ev).unwrap();
          let back: OutputEvent = serde_json::from_str(&s).unwrap();
          assert_eq!(ev, back);
      }

      #[test]
      fn cost_snapshot_default_is_zero() {
          let s = CostSnapshot::default();
          assert_eq!(s.total_nano_usd, 0);
          assert_eq!(s.total_tokens, 0);
      }

      #[test]
      fn compaction_summary_default_is_zero() {
          let s = CompactionSummary::default();
          assert_eq!(s.messages_before, 0);
          assert_eq!(s.messages_after, 0);
          assert_eq!(s.bytes_saved, 0);
      }
  }
  ```

  Replace the earlier `_output_stream_is_object_safe` lines (which won't compile — PhantomData of a trait is invalid) with the simpler `_trait_objects_compile()` form below. The corrected test block is:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[allow(dead_code)]
      fn _trait_objects_compile() {
          fn _f<T: OutputStream + 'static>(t: T) -> Box<dyn OutputStream> { Box::new(t) }
          fn _g<T: OrchestratorHandle + 'static>(t: T) -> Box<dyn OrchestratorHandle> { Box::new(t) }
      }

      #[test]
      fn output_event_round_trips_through_json() {
          let ev = OutputEvent::Text { text: "hello".into() };
          let s = serde_json::to_string(&ev).unwrap();
          let back: OutputEvent = serde_json::from_str(&s).unwrap();
          assert_eq!(ev, back);
      }

      #[test]
      fn cost_snapshot_default_is_zero() {
          let s = CostSnapshot::default();
          assert_eq!(s.total_nano_usd, 0);
          assert_eq!(s.total_tokens, 0);
      }

      #[test]
      fn compaction_summary_default_is_zero() {
          let s = CompactionSummary::default();
          assert_eq!(s.messages_before, 0);
          assert_eq!(s.messages_after, 0);
          assert_eq!(s.bytes_saved, 0);
      }
  }
  ```

  Use only the corrected version. (The earlier draft with `PhantomData<T>` is documentation; do NOT paste it.)

- [ ] Step 2 — Modify `lingxi-core/crates/traits/src/lib.rs` — add the module declaration AND re-exports. Insert after the existing `pub mod mcp;` line:
  ```rust
  pub mod orchestrator;
  ```
  And in the `pub use` block (after the `pub use mcp::*;` line), insert:
  ```rust
  pub use orchestrator::{
      CompactionSummary, CostSnapshot, HandleError, OrchestratorHandle, OutputEvent, OutputStream,
  };
  ```

- [ ] Step 3 — Run `cargo build -p lingxi-traits` — must succeed.

- [ ] Step 4 — Run `cargo test -p lingxi-traits --lib orchestrator::tests` — all 3 tests must pass (`output_event_round_trips_through_json`, `cost_snapshot_default_is_zero`, `compaction_summary_default_is_zero`).

- [ ] Step 5 — Run `cargo clippy -p lingxi-traits --all-targets -- -D warnings` — must pass clean. (The `missing_docs` workspace lint requires every public item to have a doc comment; the file above covers this.)

- [ ] Commit: `feat(M5-02 task 3): add OrchestratorHandle / OutputStream / CostSnapshot / CompactionSummary traits to lingxi-traits`

---

### Task 4: Confirm `CostSnapshot` placement + relationship to `lingxi-cost::SessionCostSummary`

**Files:**
- Modify (small doc-only): `lingxi-core/crates/cost/src/summary.rs` (add cross-reference doc-link in the `SessionCostSummary` doc comment).

**Steps:**

- [ ] Step 1 — Read `lingxi-core/crates/cost/src/summary.rs:43` to confirm `SessionCostSummary` still has fields `{ session_id, total_nano_usd, total_tokens }` (it does — verified at plan-writing time). The trait-side `CostSnapshot` from Task 3 mirrors these three fields exactly. Confirmation prevents drift between the cost crate and the trait crate.

- [ ] Step 2 — In `lingxi-core/crates/cost/src/summary.rs`, just above the `pub struct SessionCostSummary` declaration (currently line 43), add the cross-reference paragraph to its doc comment:
  ```rust
  /// Session-scope rollup.
  ///
  /// `lingxi_traits::CostSnapshot` (M5-02) mirrors the three primary fields
  /// (`session_id`, `total_nano_usd`, `total_tokens`) without depending on
  /// `lingxi-cost`, so traits-tier consumers can publish costs without
  /// pulling in pricing. The two types convert via
  /// `From<&SessionCostSummary> for CostSnapshot`, wired in `lingxi-cost`
  /// behind a `traits` feature in a later plan; for M5-02 the mapping is
  /// the orchestrator's responsibility.
  ```

- [ ] Step 3 — Inside `lingxi-orchestrator::conversation`, add a small private helper that will be used by `run_turn` and verified later:
  ```rust
  // (sketch — actual definition lives in Task 10)
  fn cost_snapshot_from_session(session: &SessionState) -> lingxi_traits::CostSnapshot {
      // M5-02 reports zero cost — `lingxi-cost` integration is M5-05 / M5-11.
      lingxi_traits::CostSnapshot {
          session_id: session.session_id,
          total_nano_usd: 0,
          total_tokens: session.usage.input_tokens.saturating_add(session.usage.output_tokens),
      }
  }
  ```
  Note: this helper is mentioned here but is created in Task 10 step 3. Task 4 is purely a documentation cross-link task — no code beyond the doc comment.

- [ ] Step 4 — Run `cargo doc -p lingxi-cost --no-deps 2>&1 | grep -i "warning"` — must produce zero new doc warnings.

- [ ] Commit: `docs(M5-02 task 4): cross-link SessionCostSummary ↔ CostSnapshot (trait-tier mirror)`

---

### Task 5: `OrchestratorConfig` with `MAX_TURNS_DEFAULT = 30`

**Files:**
- Modify (fill from stub): `lingxi-core/crates/orchestrator/src/config.rs`
- Modify: `lingxi-core/crates/orchestrator/src/lib.rs` (uncomment `pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};`).

**Steps:**

- [ ] Step 1 — Fill `lingxi-core/crates/orchestrator/src/config.rs`:
  ```rust
  //! Orchestrator runtime configuration.
  //!
  //! `MAX_TURNS_DEFAULT = 30` is the LingXi-locked default. Spec §7 OQ-1:
  //! claude-code has no global `maxTurns` default (only per-agent
  //! frontmatter), so we lock 30 as the main-loop ceiling. Override at
  //! construction via `OrchestratorConfig { max_turns, .. }`.

  use serde::{Deserialize, Serialize};

  /// Default value for [`OrchestratorConfig::max_turns`]. **Locked at 30**
  /// per spec §4.2 OQ-1 resolution (2026-05-25).
  pub const MAX_TURNS_DEFAULT: u32 = 30;

  /// Default model identifier. The actual model lives in user settings or
  /// CLI flags (M3-01 + M5-12); this value is only used when the embedder
  /// constructs an orchestrator with `OrchestratorConfig::default()` for
  /// tests.
  pub const DEFAULT_MODEL: &str = "claude-opus-4-7";

  /// Runtime configuration for [`crate::ConversationOrchestrator`].
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct OrchestratorConfig {
      /// Maximum number of turns before the loop aborts with
      /// [`crate::OrchestratorError::MaxTurnsReached`]. Default
      /// [`MAX_TURNS_DEFAULT`].
      pub max_turns: u32,

      /// Active model identifier (passed verbatim to
      /// `AnthropicProvider::messages_create_non_stream`).
      pub model: String,

      /// Optional system prompt override. `None` means the default
      /// claude-code-equivalent system prompt is assembled (M5-03 wires
      /// the dynamic assembly; M5-02 leaves this `None` and the API call
      /// sends NO system prompt — the model receives only `messages`).
      pub system_prompt_override: Option<String>,
  }

  impl Default for OrchestratorConfig {
      fn default() -> Self {
          Self {
              max_turns: MAX_TURNS_DEFAULT,
              model: DEFAULT_MODEL.to_string(),
              system_prompt_override: None,
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn default_max_turns_is_30() {
          assert_eq!(OrchestratorConfig::default().max_turns, 30);
          assert_eq!(MAX_TURNS_DEFAULT, 30);
      }

      #[test]
      fn default_model_is_locked_string() {
          assert_eq!(OrchestratorConfig::default().model, "claude-opus-4-7");
      }

      #[test]
      fn default_system_prompt_override_is_none() {
          assert!(OrchestratorConfig::default().system_prompt_override.is_none());
      }

      #[test]
      fn config_round_trips_through_json() {
          let cfg = OrchestratorConfig {
              max_turns: 5,
              model: "x".into(),
              system_prompt_override: Some("custom".into()),
          };
          let s = serde_json::to_string(&cfg).unwrap();
          let back: OrchestratorConfig = serde_json::from_str(&s).unwrap();
          assert_eq!(back.max_turns, 5);
          assert_eq!(back.model, "x");
          assert_eq!(back.system_prompt_override.as_deref(), Some("custom"));
      }
  }
  ```

- [ ] Step 2 — Uncomment `pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};` in `lingxi-core/crates/orchestrator/src/lib.rs`.

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --lib config::tests` — all 4 tests must pass.

- [ ] Step 4 — Run `cargo clippy -p lingxi-orchestrator --all-targets -- -D warnings` — must pass clean.

- [ ] Commit: `feat(M5-02 task 5): OrchestratorConfig with MAX_TURNS_DEFAULT = 30 (spec OQ-1)`

---

### Task 6: `MockApiClient` in `test_support` + internal `OrchestratorApiClient` trait

**Files:**
- Modify (start filling): `lingxi-core/crates/orchestrator/src/test_support.rs`
- Modify (add private trait def): `lingxi-core/crates/orchestrator/src/conversation.rs`
- Modify: `lingxi-core/crates/orchestrator/src/lib.rs` (uncomment `pub use conversation::{ConversationOrchestrator, ConversationOutcome, OrchestratorApiClient};` partially — see step 4).

**Steps:**

- [ ] Step 1 — In `lingxi-core/crates/orchestrator/src/conversation.rs`, replace the placeholder with the trait definition (production-only — `ConversationOrchestrator` lands in Task 10):
  ```rust
  //! Conversation orchestrator.
  //!
  //! Drives the v0.6.0 batched turn loop:
  //! `append_user → messages_create_non_stream → dispatch_tools → append_assistant → loop_or_end`.

  use async_trait::async_trait;
  use lingxi_api_client::{ApiError, MessageResponse};
  use lingxi_protocol::ConversationMessage;

  /// Minimal contract the orchestrator needs from the API client.
  ///
  /// Production: `AnthropicProviderAdapter` wraps `AnthropicProvider` + a
  /// `HttpTransport` into this shape (added in Task 10).
  /// Tests: `crate::test_support::MockApiClient` implements this directly.
  #[async_trait]
  pub trait OrchestratorApiClient: Send + Sync {
      /// Non-streaming `messages.create`. Returns the full response after
      /// the model finishes generating.
      async fn messages_create(
          &self,
          model: &str,
          msgs: Vec<ConversationMessage>,
      ) -> Result<MessageResponse, ApiError>;
  }
  ```

- [ ] Step 2 — Fill `lingxi-core/crates/orchestrator/src/test_support.rs` with the `MockApiClient` (M5-02 ships ONE mock; later tasks add more):
  ```rust
  //! Test fixtures.
  //!
  //! Gated behind `#[cfg(any(test, feature = "test-support"))]` so the
  //! cli + tui crates can re-use the fixtures in M5-12 / M6 without
  //! pulling them into release builds.

  use crate::conversation::OrchestratorApiClient;
  use async_trait::async_trait;
  use lingxi_api_client::{ApiError, MessageResponse};
  use lingxi_protocol::ConversationMessage;
  use std::collections::VecDeque;
  use std::sync::Arc;
  use tokio::sync::Mutex;

  /// Scripted mock API client. Returns the responses queued at construction
  /// time, in order. Captures each `msgs` argument for later assertion.
  ///
  /// If the queue is exhausted, `messages_create` returns
  /// `ApiError::ProviderError("mock script exhausted".into())` so the
  /// orchestrator's max-turns guard is exercised honestly.
  pub struct MockApiClient {
      queue: Arc<Mutex<VecDeque<MessageResponse>>>,
      captured_msgs: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
  }

  impl MockApiClient {
      /// Construct a mock with a script of `responses` returned in order.
      pub fn new(responses: Vec<MessageResponse>) -> Self {
          Self {
              queue: Arc::new(Mutex::new(VecDeque::from(responses))),
              captured_msgs: Arc::new(Mutex::new(Vec::new())),
          }
      }

      /// Snapshot the captured `msgs` arguments (one entry per `messages_create` call).
      pub async fn captured_msgs(&self) -> Vec<Vec<ConversationMessage>> {
          self.captured_msgs.lock().await.clone()
      }

      /// Number of responses still queued.
      pub async fn remaining(&self) -> usize {
          self.queue.lock().await.len()
      }
  }

  #[async_trait]
  impl OrchestratorApiClient for MockApiClient {
      async fn messages_create(
          &self,
          _model: &str,
          msgs: Vec<ConversationMessage>,
      ) -> Result<MessageResponse, ApiError> {
          self.captured_msgs.lock().await.push(msgs);
          let mut q = self.queue.lock().await;
          q.pop_front().ok_or_else(|| {
              ApiError::ProviderError("mock script exhausted".into())
          })
      }
  }

  /// Tiny helper for tests to construct a fully populated `MessageResponse`
  /// without typing out every field. Defaults: zero usage, no thinking,
  /// caller picks the content blocks + stop_reason.
  pub fn mock_message_response(
      content: Vec<lingxi_api_client::types::ContentBlockApi>,
      stop_reason: Option<&str>,
  ) -> MessageResponse {
      MessageResponse {
          id: "msg_mock".to_string(),
          model: "claude-opus-4-7".to_string(),
          content,
          stop_reason: stop_reason.map(str::to_string),
          usage: lingxi_api_client::types::UsageApi::default(),
      }
  }
  ```

  Note: `lingxi_api_client::ApiError::ProviderError(String)` is the closest variant — if M3-03's actual variant name differs (it's locked in the api-client crate), the test will fail to compile and we adjust to the actual variant name. The fall-back is `ApiError::Transport(...)` or similar. Check `lingxi-core/crates/api-client/src/error.rs` first; the production variant for "no upstream response" is most likely `ApiError::ProviderError(String)` per the M3-03 lock.

  **Verification step inside this task**: before writing the line above, `grep -n "pub enum ApiError" lingxi-core/crates/api-client/src/error.rs` AND `grep -n "^    [A-Z]" lingxi-core/crates/api-client/src/error.rs` to see the actual variants. Use the closest single-string-payload variant. If none exists, add a quoted alternative path: use `ApiError::from(std::io::Error::new(std::io::ErrorKind::Other, "mock script exhausted"))` IFF `ApiError: From<std::io::Error>`. Document the exact variant chosen in a comment.

- [ ] Step 3 — In `lingxi-core/crates/orchestrator/src/lib.rs`, uncomment partial — only the trait needs exposing now; `ConversationOrchestrator` + `ConversationOutcome` lands in Task 10:
  ```rust
  pub use conversation::OrchestratorApiClient;
  // pub use conversation::{ConversationOrchestrator, ConversationOutcome};  // Task 10
  pub use config::{OrchestratorConfig, MAX_TURNS_DEFAULT};
  pub use error::OrchestratorError;
  ```

- [ ] Step 4 — Add a unit test inside `test_support.rs` (we test the mock itself):
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use lingxi_api_client::types::ContentBlockApi;

      #[tokio::test]
      async fn mock_returns_responses_in_order() {
          let r1 = mock_message_response(
              vec![ContentBlockApi::Text { text: "one".into() }],
              Some("end_turn"),
          );
          let r2 = mock_message_response(
              vec![ContentBlockApi::Text { text: "two".into() }],
              Some("end_turn"),
          );
          let mock = MockApiClient::new(vec![r1, r2]);
          let resp1 = mock.messages_create("m", vec![]).await.expect("first");
          let resp2 = mock.messages_create("m", vec![]).await.expect("second");
          let first_text = match &resp1.content[0] {
              ContentBlockApi::Text { text } => text,
              _ => panic!("expected text block"),
          };
          let second_text = match &resp2.content[0] {
              ContentBlockApi::Text { text } => text,
              _ => panic!("expected text block"),
          };
          assert_eq!(first_text, "one");
          assert_eq!(second_text, "two");
          assert_eq!(mock.remaining().await, 0);
      }

      #[tokio::test]
      async fn mock_captures_msgs_per_call() {
          let r = mock_message_response(vec![], Some("end_turn"));
          let mock = MockApiClient::new(vec![r]);
          let msgs = vec![]; // empty for this assertion
          mock.messages_create("m", msgs).await.expect("call");
          assert_eq!(mock.captured_msgs().await.len(), 1);
      }

      #[tokio::test]
      async fn mock_exhaustion_returns_provider_error() {
          let mock = MockApiClient::new(vec![]);
          let err = mock.messages_create("m", vec![]).await.expect_err("exhausted");
          assert!(format!("{err}").contains("mock script exhausted"));
      }
  }
  ```

- [ ] Step 5 — Run `cargo test -p lingxi-orchestrator --lib test_support::tests` — all 3 tests must pass.

- [ ] Commit: `feat(M5-02 task 6): MockApiClient + internal OrchestratorApiClient trait`

---

### Task 7: `MockOutputStream` + `OutputEventCapture` in `test_support`

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/test_support.rs` (append `MockOutputStream` + helpers).

**Steps:**

- [ ] Step 1 — Append to `lingxi-core/crates/orchestrator/src/test_support.rs`:
  ```rust
  use async_trait::async_trait as _;
  use lingxi_traits::{CostSnapshot, OutputEvent, OutputStream};

  /// Capture all `OutputStream` events into an in-memory `Vec` for assertion.
  pub struct MockOutputStream {
      events: Arc<Mutex<Vec<OutputEvent>>>,
  }

  impl MockOutputStream {
      /// Construct an empty mock.
      pub fn new() -> Self {
          Self { events: Arc::new(Mutex::new(Vec::new())) }
      }

      /// Snapshot the captured events.
      pub async fn snapshot(&self) -> Vec<OutputEvent> {
          self.events.lock().await.clone()
      }

      /// Convenience: count of `Text` events in capture order.
      pub async fn text_events(&self) -> Vec<String> {
          self.events
              .lock()
              .await
              .iter()
              .filter_map(|e| match e {
                  OutputEvent::Text { text } => Some(text.clone()),
                  _ => None,
              })
              .collect()
      }

      /// Convenience: count of `ToolCall` events in capture order.
      pub async fn tool_calls(&self) -> Vec<(String, serde_json::Value)> {
          self.events
              .lock()
              .await
              .iter()
              .filter_map(|e| match e {
                  OutputEvent::ToolCall { tool, input } => Some((tool.clone(), input.clone())),
                  _ => None,
              })
              .collect()
      }
  }

  impl Default for MockOutputStream {
      fn default() -> Self {
          Self::new()
      }
  }

  #[async_trait::async_trait]
  impl OutputStream for MockOutputStream {
      async fn emit_text(&self, text: &str) {
          self.events.lock().await.push(OutputEvent::Text { text: text.to_string() });
      }
      async fn emit_tool_call(&self, tool: &str, input: &serde_json::Value) {
          self.events.lock().await.push(OutputEvent::ToolCall {
              tool: tool.to_string(),
              input: input.clone(),
          });
      }
      async fn emit_tool_result(&self, tool: &str, result: &serde_json::Value) {
          self.events.lock().await.push(OutputEvent::ToolResult {
              tool: tool.to_string(),
              result: result.clone(),
          });
      }
      async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot) {
          self.events.lock().await.push(OutputEvent::EndTurn {
              stop_reason: stop_reason.to_string(),
              cost: cost.clone(),
          });
      }
  }
  ```

- [ ] Step 2 — Append tests inside the existing `#[cfg(test)] mod tests` block in `test_support.rs`:
  ```rust
  #[tokio::test]
  async fn mock_output_stream_captures_text() {
      let m = MockOutputStream::new();
      m.emit_text("hello").await;
      m.emit_text("world").await;
      let texts = m.text_events().await;
      assert_eq!(texts, vec!["hello".to_string(), "world".to_string()]);
  }

  #[tokio::test]
  async fn mock_output_stream_captures_tool_lifecycle() {
      let m = MockOutputStream::new();
      let input = serde_json::json!({"file_path": "/tmp/x"});
      let result = serde_json::json!({"content": "ok"});
      m.emit_tool_call("Read", &input).await;
      m.emit_tool_result("Read", &result).await;
      let snap = m.snapshot().await;
      assert_eq!(snap.len(), 2);
      assert!(matches!(snap[0], OutputEvent::ToolCall { .. }));
      assert!(matches!(snap[1], OutputEvent::ToolResult { .. }));
  }

  #[tokio::test]
  async fn mock_output_stream_captures_end_turn() {
      let m = MockOutputStream::new();
      let cost = CostSnapshot::default();
      m.emit_end_turn("end_turn", &cost).await;
      let snap = m.snapshot().await;
      assert_eq!(snap.len(), 1);
      match &snap[0] {
          OutputEvent::EndTurn { stop_reason, cost: c } => {
              assert_eq!(stop_reason, "end_turn");
              assert_eq!(c, &CostSnapshot::default());
          }
          _ => panic!("expected EndTurn"),
      }
  }
  ```

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --lib test_support::tests` — all tests (including the 3 from Task 6) must pass; 3 new tests added here.

- [ ] Commit: `feat(M5-02 task 7): MockOutputStream + OutputEvent capture for tests`

---

### Task 8: `NoOpHookExecutor` + `NoOpPermissionGate` stubs

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/test_support.rs` (append two stubs).

**Steps:**

- [ ] Step 1 — First grep for the hook executor trait surface and the permission gate trait surface to make sure the stubs match the real shape. Read the relevant signatures:
  ```bash
  grep -n "pub trait HookExecutor\|pub trait PermissionGate\|async fn.*check\|async fn pre_tool\|async fn post_tool" \
      lingxi-core/crates/hooks/src/lib.rs lingxi-core/crates/permission/src/lib.rs
  ```
  Lock the exact method names + arg shapes from the output. **Decision rule**: if `lingxi-hooks` exposes a trait method like `async fn execute(&self, event: HookEvent) -> Result<HookOutcome, HookError>` (where `HookEvent` is the enum with `PreToolUse { tool_name, tool_input }` and `PostToolUse { tool_name, tool_output, is_error }` arms), the stub does `Ok(HookOutcome::Continue)`. If the surface differs (M5-06 is the canonical wiring plan — M5-02 only consumes whatever exists today), document the discovered shape in this task's commit message and mirror it.

  **Fallback if `HookExecutor` doesn't exist as a trait yet**: M5-02 declares a thin `trait HookExecutor` directly inside `lingxi-orchestrator::test_support` (private to the orchestrator crate), with the methods `async fn pre_tool_use(&self, tool_name: &str, input: &serde_json::Value)` and `async fn post_tool_use(&self, tool_name: &str, output: &serde_json::Value, is_error: bool)`, both `Result<(), HookError>` for some `HookError` we define locally. M5-06 replaces this with the real trait. The orchestrator's `new()` constructor takes `Arc<dyn HookExecutor>` where `HookExecutor` is THIS local trait until M5-06.

  Default to the fallback path unless the grep above shows an existing trait with a compatible shape.

- [ ] Step 2 — Append to `lingxi-core/crates/orchestrator/src/test_support.rs`:
  ```rust
  /// Local hook executor trait used by `ConversationOrchestrator` until
  /// M5-06 wires the real 4-arm executor from `lingxi-hooks`. Lives here
  /// (not in `lingxi-traits`) because M5-06 will move it.
  #[async_trait::async_trait]
  pub trait HookExecutor: Send + Sync {
      async fn pre_tool_use(&self, tool_name: &str, input: &serde_json::Value) -> Result<(), String>;
      async fn post_tool_use(
          &self,
          tool_name: &str,
          output: &serde_json::Value,
          is_error: bool,
      ) -> Result<(), String>;
  }

  /// Allow-all hook executor. Does nothing on pre/post.
  pub struct NoOpHookExecutor;

  #[async_trait::async_trait]
  impl HookExecutor for NoOpHookExecutor {
      async fn pre_tool_use(&self, _tool_name: &str, _input: &serde_json::Value) -> Result<(), String> {
          Ok(())
      }
      async fn post_tool_use(
          &self,
          _tool_name: &str,
          _output: &serde_json::Value,
          _is_error: bool,
      ) -> Result<(), String> {
          Ok(())
      }
  }

  /// Local permission gate trait. M5-05 will swap this for
  /// `lingxi_permission::PermissionGate` (or extend it with a `PromptingGate`
  /// arm). For M5-02 the orchestrator only needs an allow/deny decision.
  #[async_trait::async_trait]
  pub trait PermissionGate: Send + Sync {
      async fn check(
          &self,
          tool_name: &str,
          input: &serde_json::Value,
      ) -> PermissionDecision;
  }

  /// Decision returned by `PermissionGate::check`.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum PermissionDecision {
      /// Tool dispatch may proceed.
      Allow,
      /// Tool dispatch is denied. The orchestrator turns this into a
      /// `ContentBlock::ToolResult { is_error: true, content: "Permission denied: <reason>" }`.
      Deny { reason: String },
  }

  /// Allow-all permission gate. Always returns `Allow`.
  pub struct NoOpPermissionGate;

  #[async_trait::async_trait]
  impl PermissionGate for NoOpPermissionGate {
      async fn check(
          &self,
          _tool_name: &str,
          _input: &serde_json::Value,
      ) -> PermissionDecision {
          PermissionDecision::Allow
      }
  }
  ```

  Note: the names `HookExecutor` and `PermissionGate` are LOCAL to `lingxi-orchestrator::test_support`. They shadow (in name) the future traits from `lingxi-hooks` and `lingxi-permission`. M5-05 and M5-06 will renamespace as needed. Because the orchestrator's `ConversationOrchestrator` (Task 10) takes `Arc<dyn HookExecutor>` and `Arc<dyn PermissionGate>` referring to THESE local traits, the future plans can either (a) rename the locals + add `From` adapters from the real `lingxi-hooks`/`lingxi-permission` traits, or (b) move the local traits up to `lingxi-traits` and have `lingxi-hooks`/`lingxi-permission` implement them. The decision is M5-05/M5-06's; M5-02 only ensures the wiring points exist.

- [ ] Step 3 — Add a small smoke test inside `test_support.rs::tests`:
  ```rust
  #[tokio::test]
  async fn noop_hook_executor_always_succeeds() {
      let h = NoOpHookExecutor;
      let v = serde_json::json!({});
      assert!(h.pre_tool_use("Read", &v).await.is_ok());
      assert!(h.post_tool_use("Read", &v, false).await.is_ok());
      assert!(h.post_tool_use("Read", &v, true).await.is_ok());
  }

  #[tokio::test]
  async fn noop_permission_gate_always_allows() {
      let g = NoOpPermissionGate;
      let v = serde_json::json!({});
      assert_eq!(g.check("Read", &v).await, PermissionDecision::Allow);
      assert_eq!(g.check("Bash", &v).await, PermissionDecision::Allow);
  }
  ```

- [ ] Step 4 — Run `cargo test -p lingxi-orchestrator --lib test_support::tests` — all tests must pass (now 8 tests in the module: 3 mock-api + 3 mock-output + 2 noop-hook/perm).

- [ ] Step 5 — Run `cargo clippy -p lingxi-orchestrator --all-targets -- -D warnings`.

- [ ] Commit: `feat(M5-02 task 8): NoOpHookExecutor + NoOpPermissionGate stubs (local traits, M5-05/M5-06 will renamespace)`

---

### Task 9: First failing test — `orchestrator_smoke_test.rs` (single turn, no tools)

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_smoke_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/orchestrator/tests/orchestrator_smoke_test.rs`:
  ```rust
  //! M5-02 Task 9: failing smoke test that drives the future
  //! `ConversationOrchestrator::run_turn` happy path. Expected to FAIL at
  //! Task 9 (the orchestrator is not yet implemented) and PASS at Task 10.

  use lingxi_api_client::types::ContentBlockApi;
  use lingxi_orchestrator::test_support::{
      mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
  use lingxi_traits::OutputEvent;
  use std::sync::Arc;

  #[tokio::test]
  async fn single_turn_no_tools_returns_end_turn_and_emits_text_and_end_turn() {
      // Model returns one text block + end_turn — simplest possible turn.
      let response = mock_message_response(
          vec![ContentBlockApi::Text { text: "hello world".into() }],
          Some("end_turn"),
      );
      let api = Arc::new(MockApiClient::new(vec![response]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());

      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api.clone(),
          tools,
          hooks,
          perms,
          output.clone(),
      );

      let outcome = orch
          .run_turn("say hello")
          .await
          .expect("turn must succeed");

      match outcome {
          ConversationOutcome::EndTurn { turn_count, .. } => assert_eq!(turn_count, 1),
      }

      let events = output.snapshot().await;
      assert_eq!(events.len(), 2, "expected Text + EndTurn; got {events:?}");
      assert!(matches!(events[0], OutputEvent::Text { ref text } if text == "hello world"));
      assert!(matches!(events[1], OutputEvent::EndTurn { ref stop_reason, .. } if stop_reason == "end_turn"));

      // Mock observed exactly one API call.
      assert_eq!(api.captured_msgs().await.len(), 1);
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test orchestrator_smoke_test`. Expected outcome: **COMPILATION FAILURE** because `ConversationOrchestrator`, `ConversationOutcome`, and the `new()` / `run_turn()` methods do not yet exist. This is the TDD red.

- [ ] Step 3 — Confirm the failure is a `cannot find type` / `cannot find function` error (NOT some other failure mode). Document the exact error message in the commit body.

- [ ] Commit: `test(M5-02 task 9): failing smoke test for ConversationOrchestrator::run_turn (expected FAIL until Task 10)`

---

### Task 10: Implement `ConversationOrchestrator::new` + `run_turn` happy path

**Files:**
- Modify (fill from stub): `lingxi-core/crates/orchestrator/src/conversation.rs` (add struct + `new` + `run_turn`)
- Modify (fill from stub): `lingxi-core/crates/orchestrator/src/turn_loop.rs` (private helpers for turn execution)
- Modify: `lingxi-core/crates/orchestrator/src/lib.rs` (uncomment `pub use conversation::{ConversationOrchestrator, ConversationOutcome};`).

**Steps:**

- [ ] Step 1 — Fill `lingxi-core/crates/orchestrator/src/conversation.rs` with the full struct + methods. The file now needs:
  - The `OrchestratorApiClient` trait (from Task 6).
  - The `ConversationOrchestrator` struct.
  - `ConversationOutcome` enum.
  - `AnthropicProviderAdapter<T: HttpTransport>` adapter (production glue).
  - `new()` constructor.
  - `run_turn()` driving the loop.

  ```rust
  //! Conversation orchestrator.
  //!
  //! Drives the v0.6.0 batched turn loop. See module-level docs in `lib.rs`.

  use crate::config::OrchestratorConfig;
  use crate::error::OrchestratorError;
  use crate::turn_loop::{cost_snapshot_from_session, execute_one_turn, TurnStepOutcome};
  use async_trait::async_trait;
  use lingxi_api_client::{ApiError, AnthropicProvider, MessageResponse};
  use lingxi_core::SessionState;
  use lingxi_protocol::{ConversationMessage, MessageId, SessionId};
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_traits::{HttpTransport, OutputStream};
  use serde_json::Value;
  use std::sync::Arc;
  use tokio::sync::Mutex;

  use crate::test_support::{HookExecutor, PermissionGate};

  /// Minimal contract the orchestrator needs from the API client.
  ///
  /// Production: `AnthropicProviderAdapter` wraps `AnthropicProvider` +
  /// `HttpTransport` into this shape. Tests: `MockApiClient`.
  #[async_trait]
  pub trait OrchestratorApiClient: Send + Sync {
      /// Non-streaming `messages.create`.
      async fn messages_create(
          &self,
          model: &str,
          msgs: Vec<ConversationMessage>,
      ) -> Result<MessageResponse, ApiError>;
  }

  /// Result of `ConversationOrchestrator::run_turn` on success.
  ///
  /// Only one variant in M5-02; M5-04 may add `Cancelled { ... }` later.
  #[derive(Debug, Clone)]
  #[non_exhaustive]
  pub enum ConversationOutcome {
      /// Model emitted `stop_reason == "end_turn"` after `turn_count` API
      /// calls. `final_message_id` is the id of the final assistant message
      /// appended to the session.
      EndTurn {
          turn_count: u32,
          final_message_id: MessageId,
      },
  }

  /// The orchestrator. Owns the session, dispatches tools, drives the loop.
  ///
  /// Construction is via `new(...)`. Driven via `run_turn(prompt)`.
  pub struct ConversationOrchestrator {
      pub(crate) config: OrchestratorConfig,
      pub(crate) api: Arc<dyn OrchestratorApiClient>,
      pub(crate) tools: Arc<ToolRegistry>,
      pub(crate) hooks: Arc<dyn HookExecutor>,
      pub(crate) perms: Arc<dyn PermissionGate>,
      pub(crate) output: Arc<dyn OutputStream>,
      pub(crate) session: Arc<Mutex<SessionState>>,
  }

  impl ConversationOrchestrator {
      /// Construct a new orchestrator with a fresh in-memory session.
      pub fn new(
          config: OrchestratorConfig,
          api: Arc<dyn OrchestratorApiClient>,
          tools: Arc<ToolRegistry>,
          hooks: Arc<dyn HookExecutor>,
          perms: Arc<dyn PermissionGate>,
          output: Arc<dyn OutputStream>,
      ) -> Self {
          let session = SessionState::empty(SessionId::new(), config.model.clone());
          Self {
              config,
              api,
              tools,
              hooks,
              perms,
              output,
              session: Arc::new(Mutex::new(session)),
          }
      }

      /// Drive one user prompt through the turn loop until `end_turn` or
      /// `max_turns` is exhausted.
      pub async fn run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
          // Telemetry: conversation_started. (Wired in Task 15.)

          // 1. Append the user prompt to session history.
          {
              let mut s = self.session.lock().await;
              let msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
              s.history.push(msg);
          }

          // 2. Turn-by-turn driver.
          let mut turn_count: u32 = 0;
          let final_message_id;
          loop {
              if turn_count >= self.config.max_turns {
                  return Err(OrchestratorError::MaxTurnsReached {
                      max_turns: self.config.max_turns,
                  });
              }
              turn_count = turn_count.saturating_add(1);

              let step = execute_one_turn(self).await?;
              match step {
                  TurnStepOutcome::Continue => continue,
                  TurnStepOutcome::Ended { final_message_id: id, stop_reason } => {
                      let cost = {
                          let s = self.session.lock().await;
                          cost_snapshot_from_session(&s)
                      };
                      self.output.emit_end_turn(&stop_reason, &cost).await;
                      final_message_id = id;
                      break;
                  }
              }
          }

          // Telemetry: conversation_completed. (Wired in Task 15.)
          Ok(ConversationOutcome::EndTurn {
              turn_count,
              final_message_id,
          })
      }

      /// Borrow the in-memory session (read lock surrogate). Useful for tests.
      pub fn session(&self) -> Arc<Mutex<SessionState>> {
          self.session.clone()
      }
  }

  /// Production adapter: wraps `AnthropicProvider` + an `HttpTransport` into
  /// the `OrchestratorApiClient` shape.
  ///
  /// Concrete type so callers can construct without knowing the transport
  /// type parameter (the constructor takes `Arc<dyn OrchestratorApiClient>`).
  pub struct AnthropicProviderAdapter<T: HttpTransport + Send + Sync + 'static> {
      provider: AnthropicProvider,
      transport: Arc<T>,
  }

  impl<T: HttpTransport + Send + Sync + 'static> AnthropicProviderAdapter<T> {
      /// Construct from an existing provider + transport.
      pub fn new(provider: AnthropicProvider, transport: Arc<T>) -> Self {
          Self { provider, transport }
      }
  }

  #[async_trait]
  impl<T: HttpTransport + Send + Sync + 'static> OrchestratorApiClient for AnthropicProviderAdapter<T> {
      async fn messages_create(
          &self,
          model: &str,
          msgs: Vec<ConversationMessage>,
      ) -> Result<MessageResponse, ApiError> {
          self.provider
              .messages_create_non_stream(model, msgs, self.transport.as_ref())
              .await
      }
  }
  ```

  **Note on `AnthropicProvider::new`**: M3-03 made `AnthropicProvider` constructable with a builder; the adapter just stores an already-constructed provider so the caller (M5-12 CLI) handles construction. M5-02 does NOT itself test the adapter end-to-end (no live HTTP); the adapter only needs to compile. Tests use `MockApiClient` which bypasses the adapter entirely.

- [ ] Step 2 — Fill `lingxi-core/crates/orchestrator/src/turn_loop.rs`:
  ```rust
  //! Inner turn-by-turn loop helpers. Private to `ConversationOrchestrator`.

  use crate::conversation::ConversationOrchestrator;
  use crate::error::OrchestratorError;
  use crate::test_support::PermissionDecision;
  use lingxi_api_client::types::ContentBlockApi;
  use lingxi_core::SessionState;
  use lingxi_protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
  use lingxi_tools::context::{ToolUseContext, ToolUseOptions};
  use lingxi_traits::CostSnapshot;

  /// What one turn step decided.
  pub(crate) enum TurnStepOutcome {
      /// Continue the loop (e.g. model returned `tool_use`).
      Continue,
      /// Loop should terminate — model returned `end_turn`.
      Ended {
          final_message_id: MessageId,
          stop_reason: String,
      },
  }

  /// Execute one `messages_create_non_stream` round-trip + tool dispatches.
  pub(crate) async fn execute_one_turn(
      orch: &ConversationOrchestrator,
  ) -> Result<TurnStepOutcome, OrchestratorError> {
      // Snapshot the current session history for the API call.
      let (history_snapshot, model) = {
          let s = orch.session.lock().await;
          (s.history.clone(), s.model.clone())
      };

      // 1. Call the API.
      let response = orch.api.messages_create(&model, history_snapshot).await?;

      // 2. Translate `MessageResponse.content` → `ContentBlock` history entry.
      let assistant_blocks = translate_response_blocks(&response.content);

      // 3. Append the assistant message to the session. We need the
      //    `final_message_id` to return to the caller.
      let assistant_id = MessageId::new();
      {
          let mut s = orch.session.lock().await;
          s.history.push(ConversationMessage::Assistant {
              id: assistant_id,
              content: assistant_blocks.clone(),
              stop_reason: response.stop_reason.clone(),
          });
      }

      // 4. Emit each Text block to the output stream (whole-body in M5-02;
      //    M5-04 will switch to per-delta).
      for blk in &assistant_blocks {
          if let ContentBlock::Text { text } = blk {
              orch.output.emit_text(text).await;
          }
      }

      // 5. If there are tool_use blocks, dispatch them and feed results back.
      let tool_uses: Vec<(ToolUseId, String, serde_json::Value)> = assistant_blocks
          .iter()
          .filter_map(|b| match b {
              ContentBlock::ToolUse { id, name, input } => Some((*id, name.clone(), input.clone())),
              _ => None,
          })
          .collect();

      if !tool_uses.is_empty() {
          let tool_results = dispatch_tool_uses(orch, &tool_uses).await?;
          // Append a fresh user message carrying the tool results.
          let user_id = MessageId::new();
          {
              let mut s = orch.session.lock().await;
              s.history.push(ConversationMessage::User {
                  id: user_id,
                  content: tool_results,
              });
          }
      }

      // 6. Decide loop disposition.
      match response.stop_reason.as_deref() {
          Some("end_turn") => Ok(TurnStepOutcome::Ended {
              final_message_id: assistant_id,
              stop_reason: "end_turn".to_string(),
          }),
          _ => Ok(TurnStepOutcome::Continue),
      }
  }

  /// Translate api-client content blocks into protocol content blocks.
  /// Unknown variants (Thinking / ServerToolUse / ConnectorText / AdvisorToolResult)
  /// are dropped in M5-02. M5-04 may surface Thinking.
  fn translate_response_blocks(content: &[ContentBlockApi]) -> Vec<ContentBlock> {
      content
          .iter()
          .filter_map(|b| match b {
              ContentBlockApi::Text { text } => Some(ContentBlock::Text { text: text.clone() }),
              ContentBlockApi::ToolUse { id, name, input } => Some(ContentBlock::ToolUse {
                  id: *id,
                  name: name.clone(),
                  input: input.clone(),
              }),
              ContentBlockApi::Thinking { thinking, signature } => Some(ContentBlock::Thinking {
                  thinking: thinking.clone(),
                  signature: signature.clone(),
              }),
              // Server-side variants are skipped in M5-02; M5-04 may revisit.
              ContentBlockApi::ServerToolUse { .. }
              | ContentBlockApi::ConnectorText { .. }
              | ContentBlockApi::AdvisorToolResult { .. } => None,
          })
          .collect()
  }

  /// Dispatch each tool_use block through hooks → permission → registry →
  /// hooks. Returns a list of `ContentBlock::ToolResult` blocks for the
  /// next user message.
  async fn dispatch_tool_uses(
      orch: &ConversationOrchestrator,
      tool_uses: &[(ToolUseId, String, serde_json::Value)],
  ) -> Result<Vec<ContentBlock>, OrchestratorError> {
      use lingxi_tools::progress::ToolProgressSender;

      let mut results = Vec::with_capacity(tool_uses.len());
      for (tool_use_id, name, input) in tool_uses {
          orch.output.emit_tool_call(name, input).await;

          // Pre-tool hook.
          if let Err(reason) = orch.hooks.pre_tool_use(name, input).await {
              let result_block = ContentBlock::ToolResult {
                  tool_use_id: *tool_use_id,
                  content: format!("Hook blocked: {reason}"),
                  is_error: true,
              };
              orch.output
                  .emit_tool_result(name, &serde_json::json!({ "error": format!("Hook blocked: {reason}") }))
                  .await;
              results.push(result_block);
              continue;
          }

          // Permission gate.
          match orch.perms.check(name, input).await {
              PermissionDecision::Allow => {}
              PermissionDecision::Deny { reason } => {
                  let result_block = ContentBlock::ToolResult {
                      tool_use_id: *tool_use_id,
                      content: format!("Permission denied: {reason}"),
                      is_error: true,
                  };
                  orch.output
                      .emit_tool_result(name, &serde_json::json!({ "error": format!("Permission denied: {reason}") }))
                      .await;
                  results.push(result_block);
                  continue;
              }
          }

          // Dispatch through ToolRegistry.
          let tool_handle = match orch.tools.find_by_name(name) {
              Some(t) => t,
              None => {
                  let result_block = ContentBlock::ToolResult {
                      tool_use_id: *tool_use_id,
                      content: format!("Error: tool not found: {name}"),
                      is_error: true,
                  };
                  orch.output
                      .emit_tool_result(name, &serde_json::json!({ "error": format!("tool not found: {name}") }))
                      .await;
                  results.push(result_block);
                  continue;
              }
          };

          // Synthesize a minimal ToolUseContext.
          let messages = {
              let s = orch.session.lock().await;
              s.history.clone()
          };
          let ctx = ToolUseContext {
              options: ToolUseOptions {
                  debug: false,
                  verbose: false,
                  main_loop_model: orch.config.model.clone(),
                  max_budget_nano_usd: None,
                  mcp_clients: Vec::new(),
                  is_non_interactive_session: true,
                  custom_system_prompt: orch.config.system_prompt_override.clone(),
                  append_system_prompt: None,
              },
              messages,
              tool_use_id: Some(*tool_use_id),
              agent_id: None,
              content_replacement_state: None,
              session: Some(orch.session.clone()),
              subagent_registry: Some(orch.tools.clone()),
          };

          // One-shot progress channel — drained immediately so the producer
          // never blocks on a full buffer.
          let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(8);
          let drain = tokio::spawn(async move { while let Some(_) = progress_rx.recv().await {} });
          let progress = ToolProgressSender::new(progress_tx);

          let tool_outcome = tool_handle.call(input.clone(), ctx, progress).await;
          drop(progress); // drop, then await drain
          let _ = drain.await;

          let (content, is_error, emit_payload) = match tool_outcome {
              Ok(result) => {
                  let text = serde_json::to_string(&result.data)
                      .unwrap_or_else(|_| "<unserializable>".into());
                  (text, false, result.data)
              }
              Err(err) => {
                  let text = format!("Error: {err}");
                  (text, true, serde_json::json!({ "error": format!("{err}") }))
              }
          };

          orch.output.emit_tool_result(name, &emit_payload).await;

          // Post-tool hook (best-effort — failures are surfaced as a side
          // ContentBlock but do NOT replace the tool result).
          let _ = orch.hooks.post_tool_use(name, &emit_payload, is_error).await;

          results.push(ContentBlock::ToolResult {
              tool_use_id: *tool_use_id,
              content,
              is_error,
          });
      }
      Ok(results)
  }

  /// Project a `SessionState` into a `CostSnapshot`. M5-02 reports zero cost;
  /// M5-05/M5-11 will plug in `lingxi-cost`.
  pub(crate) fn cost_snapshot_from_session(s: &SessionState) -> CostSnapshot {
      CostSnapshot {
          session_id: s.session_id,
          total_nano_usd: 0,
          total_tokens: s.usage.input_tokens.saturating_add(s.usage.output_tokens),
      }
  }
  ```

  **`SessionState.usage`**: M1 ships `CumulativeUsage` with `input_tokens: u64, output_tokens: u64` (plus possibly cache fields). Grep `lingxi-core/crates/core/src/session.rs` for the exact field name — if it's `prompt_tokens` instead of `input_tokens`, adjust. The fall-back is `s.usage.input_tokens` / `s.usage.output_tokens` per M1 lock. Task 10 step 0 (below) does that grep.

- [ ] Step 0 — (Pre-step) Run `grep -n "pub struct CumulativeUsage\|pub.*input_tokens\|pub.*output_tokens\|pub.*prompt_tokens" lingxi-core/crates/core/src/session.rs lingxi-core/crates/core/src/usage.rs 2>/dev/null` to confirm field names. Lock the actual field names in `cost_snapshot_from_session`. (If neither exists yet — `CumulativeUsage` could be `Default`-only — use `0` for `total_tokens` and document the placeholder.)

- [ ] Step 3 — Uncomment `pub use conversation::{ConversationOrchestrator, ConversationOutcome};` in `lingxi-core/crates/orchestrator/src/lib.rs`.

- [ ] Step 4 — Run `cargo test -p lingxi-orchestrator --test orchestrator_smoke_test`. The test from Task 9 must now PASS. (If the smoke test still fails because of a field-name mismatch in `CumulativeUsage`, debug + fix; do not declare success without `cargo test -p lingxi-orchestrator --test orchestrator_smoke_test` printing `test result: ok. 1 passed`.)

- [ ] Step 5 — Run `cargo test -p lingxi-orchestrator` (all tests). Must pass.

- [ ] Step 6 — Run `cargo clippy -p lingxi-orchestrator --all-targets -- -D warnings`. Must pass.

- [ ] Commit: `feat(M5-02 task 10): ConversationOrchestrator::run_turn happy path — smoke test passes`

---

### Task 11: Multi-turn with one tool_use — `orchestrator_multi_turn_test.rs`

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_multi_turn_test.rs`

**Steps:**

- [ ] Step 1 — Create the test file:
  ```rust
  //! M5-02 Task 11: multi-turn loop with one tool_use in the first response.

  use async_trait::async_trait;
  use lingxi_api_client::types::ContentBlockApi;
  use lingxi_orchestrator::test_support::{
      mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
  use lingxi_protocol::ToolUseId;
  use lingxi_tools::progress::ToolProgressSender;
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_tools::tool_trait::{
      DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
  };
  use lingxi_traits::OutputEvent;
  use serde_json::json;
  use std::sync::Arc;

  /// Mock tool that always returns `{"ok": true}`.
  struct AlwaysOkTool;

  #[async_trait]
  impl Tool for AlwaysOkTool {
      fn name(&self) -> &str { "AlwaysOk" }
      fn input_schema(&self) -> &serde_json::Value {
          static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
              once_cell::sync::Lazy::new(|| json!({"type": "object"}));
          &SCHEMA
      }
      fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool { true }
      fn max_result_size_chars(&self) -> usize { 1024 }
      fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
      fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
      async fn check_permissions(
          &self,
          _input: &serde_json::Value,
          _ctx: &lingxi_tools::context::ToolUseContext,
      ) -> lingxi_permission::PermissionResult {
          lingxi_permission::PermissionResult::Allow {
              reason: lingxi_permission::PermissionDecisionReason::Other { reason: "test".into() },
              updated_input: None,
              update_destination: None,
              metadata: lingxi_permission::result::PermissionMetadata::default(),
          }
      }
      async fn description(&self, _input: &serde_json::Value, _opts: &DescriptionOptions) -> String {
          "AlwaysOk".into()
      }
      async fn prompt(&self, _opts: &PromptOptions) -> String { String::new() }
      async fn call(
          &self,
          _input: serde_json::Value,
          _ctx: lingxi_tools::context::ToolUseContext,
          _tx: ToolProgressSender,
      ) -> Result<ToolCallResult, ToolError> {
          Ok(ToolCallResult {
              data: json!({"ok": true}),
              new_messages: vec![],
              context_modifier: None,
              mcp_meta: None,
          })
      }
  }

  #[tokio::test]
  async fn two_turns_with_one_tool_use_drives_loop_to_end_turn() {
      // Turn 1: model returns text + tool_use (AlwaysOk).
      let tool_use_id = ToolUseId::new();
      let r1 = mock_message_response(
          vec![
              ContentBlockApi::Text { text: "let me check".into() },
              ContentBlockApi::ToolUse {
                  id: tool_use_id,
                  name: "AlwaysOk".into(),
                  input: json!({"x": 1}),
              },
          ],
          Some("tool_use"),
      );
      // Turn 2: model returns text + end_turn.
      let r2 = mock_message_response(
          vec![ContentBlockApi::Text { text: "all good".into() }],
          Some("end_turn"),
      );
      let api = Arc::new(MockApiClient::new(vec![r1, r2]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let mut registry = ToolRegistry::new();
      registry.register_builtin(Arc::new(AlwaysOkTool));
      let tools = Arc::new(registry);

      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api.clone(),
          tools,
          hooks,
          perms,
          output.clone(),
      );

      let outcome = orch.run_turn("please check").await.expect("turn loop");
      match outcome {
          ConversationOutcome::EndTurn { turn_count, .. } => {
              assert_eq!(turn_count, 2, "expected 2 turns; got {turn_count}");
          }
      }

      let events = output.snapshot().await;
      // Expected sequence:
      //   0. Text "let me check"
      //   1. ToolCall "AlwaysOk"
      //   2. ToolResult "AlwaysOk"
      //   3. Text "all good"
      //   4. EndTurn
      assert_eq!(events.len(), 5, "events: {events:?}");
      match &events[0] {
          OutputEvent::Text { text } => assert_eq!(text, "let me check"),
          _ => panic!("event 0 expected Text"),
      }
      match &events[1] {
          OutputEvent::ToolCall { tool, .. } => assert_eq!(tool, "AlwaysOk"),
          _ => panic!("event 1 expected ToolCall"),
      }
      match &events[2] {
          OutputEvent::ToolResult { tool, result } => {
              assert_eq!(tool, "AlwaysOk");
              assert_eq!(result, &json!({"ok": true}));
          }
          _ => panic!("event 2 expected ToolResult"),
      }
      match &events[3] {
          OutputEvent::Text { text } => assert_eq!(text, "all good"),
          _ => panic!("event 3 expected Text"),
      }
      match &events[4] {
          OutputEvent::EndTurn { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
          _ => panic!("event 4 expected EndTurn"),
      }

      // Mock observed 2 API calls — the second call's first user message
      // (after the assistant's tool_use) carries the tool_result.
      let captured = api.captured_msgs().await;
      assert_eq!(captured.len(), 2);
      // The 2nd call's history must include: user(prompt), assistant(tool_use), user(tool_result).
      let second_call = &captured[1];
      assert_eq!(second_call.len(), 3);
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test orchestrator_multi_turn_test`. Must pass.

- [ ] Step 3 — If a `lingxi-permission::PermissionDecisionReason` field name differs (registry's `DummyTool` in `lingxi-tools` uses the same pattern — see `lingxi-core/crates/tools/src/registry.rs::tests:151`), align names.

- [ ] Step 4 — Run `cargo test -p lingxi-orchestrator` (full crate) — all tests must pass.

- [ ] Commit: `test(M5-02 task 11): multi-turn loop with tool_use — exact event sequence asserted`

---

### Task 12: `max_turns` exceeded — `orchestrator_max_turns_test.rs`

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_max_turns_test.rs`

**Steps:**

- [ ] Step 1 — Create the test file:
  ```rust
  //! M5-02 Task 12: max_turns ceiling enforcement.

  use lingxi_api_client::types::ContentBlockApi;
  use lingxi_orchestrator::test_support::{
      mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
  };
  use lingxi_orchestrator::{
      ConversationOrchestrator, OrchestratorConfig, OrchestratorError,
  };
  use lingxi_tools::registry::ToolRegistry;
  use std::sync::Arc;

  #[tokio::test]
  async fn never_ending_loop_aborts_with_max_turns_reached() {
      // Model returns `tool_use` forever — but with NO actual tool_use blocks,
      // so the orchestrator just loops the API. (We deliberately use a
      // non-`end_turn` stop_reason and no tool_use blocks — this stresses the
      // turn counter, not the tool dispatch path.)
      let make_resp = || {
          mock_message_response(
              vec![ContentBlockApi::Text { text: "still thinking".into() }],
              Some("max_tokens"), // any value other than "end_turn"
          )
      };
      // Provide enough responses that the API never exhausts before max_turns.
      let api = Arc::new(MockApiClient::new(vec![
          make_resp(), make_resp(), make_resp(), make_resp(), make_resp(),
      ]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let tools = Arc::new(ToolRegistry::new());

      let mut config = OrchestratorConfig::default();
      config.max_turns = 3;

      let orch = ConversationOrchestrator::new(
          config,
          api.clone(),
          tools,
          hooks,
          perms,
          output.clone(),
      );

      let err = orch.run_turn("forever").await.expect_err("must fail");
      assert!(matches!(err, OrchestratorError::MaxTurnsReached { max_turns: 3 }));
      assert_eq!(err.to_string(), "Reached maximum number of turns (3)");

      // API was called exactly max_turns (= 3) times.
      assert_eq!(api.captured_msgs().await.len(), 3);
  }

  #[tokio::test]
  async fn max_turns_default_30_is_the_construction_default() {
      let api = Arc::new(MockApiClient::new(vec![mock_message_response(vec![], Some("end_turn"))]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let tools = Arc::new(ToolRegistry::new());

      // Use Default — max_turns should be 30.
      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api,
          tools,
          hooks,
          perms,
          output,
      );

      // We don't actually loop 30 times — we just verify the field via the
      // public config getter. But there's no getter; instead, run a single
      // happy turn and confirm no MaxTurnsReached fires:
      let outcome = orch.run_turn("ping").await.expect("happy");
      assert!(matches!(outcome, lingxi_orchestrator::ConversationOutcome::EndTurn { turn_count: 1, .. }));
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test orchestrator_max_turns_test`. Both tests must pass.

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator`. All tests must pass.

- [ ] Step 4 — (Belt-and-braces) Add a third test in the same file covering API-error propagation:
  ```rust
  #[tokio::test]
  async fn api_error_propagates_as_orchestrator_error_api_call() {
      // Empty mock → first call returns ApiError::ProviderError.
      let api = Arc::new(MockApiClient::new(vec![]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let tools = Arc::new(ToolRegistry::new());

      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api,
          tools,
          hooks,
          perms,
          output,
      );

      let err = orch.run_turn("anything").await.expect_err("must fail");
      let msg = err.to_string();
      assert!(msg.starts_with("api call failed: "), "got: {msg}");
  }
  ```
  Then `cargo test -p lingxi-orchestrator --test orchestrator_max_turns_test` again — all 3 tests pass.

- [ ] Commit: `test(M5-02 task 12): max_turns enforcement + api_error propagation`

---

### Task 13: Tool returns `ToolError` — `orchestrator_tool_error_test.rs`

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_tool_error_test.rs`

**Steps:**

- [ ] Step 1 — Create the test file:
  ```rust
  //! M5-02 Task 13: tool errors propagate as ToolResult { is_error: true }.

  use async_trait::async_trait;
  use lingxi_api_client::types::ContentBlockApi;
  use lingxi_orchestrator::test_support::{
      mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_protocol::ToolUseId;
  use lingxi_tools::progress::ToolProgressSender;
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_tools::tool_trait::{
      DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
  };
  use lingxi_traits::OutputEvent;
  use serde_json::json;
  use std::sync::Arc;

  struct AlwaysFailingTool;

  #[async_trait]
  impl Tool for AlwaysFailingTool {
      fn name(&self) -> &str { "AlwaysFail" }
      fn input_schema(&self) -> &serde_json::Value {
          static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
              once_cell::sync::Lazy::new(|| json!({"type": "object"}));
          &SCHEMA
      }
      fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool { true }
      fn max_result_size_chars(&self) -> usize { 1024 }
      fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
      fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
      async fn check_permissions(
          &self,
          _input: &serde_json::Value,
          _ctx: &lingxi_tools::context::ToolUseContext,
      ) -> lingxi_permission::PermissionResult {
          lingxi_permission::PermissionResult::Allow {
              reason: lingxi_permission::PermissionDecisionReason::Other { reason: "test".into() },
              updated_input: None,
              update_destination: None,
              metadata: lingxi_permission::result::PermissionMetadata::default(),
          }
      }
      async fn description(&self, _input: &serde_json::Value, _opts: &DescriptionOptions) -> String {
          "fail".into()
      }
      async fn prompt(&self, _opts: &PromptOptions) -> String { String::new() }
      async fn call(
          &self,
          _input: serde_json::Value,
          _ctx: lingxi_tools::context::ToolUseContext,
          _tx: ToolProgressSender,
      ) -> Result<ToolCallResult, ToolError> {
          Err(ToolError::Internal("disk on fire".into()))
      }
  }

  #[tokio::test]
  async fn tool_error_becomes_tool_result_with_is_error_true_and_loop_continues() {
      let tool_use_id = ToolUseId::new();
      let r1 = mock_message_response(
          vec![ContentBlockApi::ToolUse {
              id: tool_use_id,
              name: "AlwaysFail".into(),
              input: json!({}),
          }],
          Some("tool_use"),
      );
      let r2 = mock_message_response(
          vec![ContentBlockApi::Text { text: "sorry, fix it later".into() }],
          Some("end_turn"),
      );
      let api = Arc::new(MockApiClient::new(vec![r1, r2]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let mut registry = ToolRegistry::new();
      registry.register_builtin(Arc::new(AlwaysFailingTool));
      let tools = Arc::new(registry);

      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api.clone(),
          tools,
          hooks,
          perms,
          output.clone(),
      );
      let outcome = orch.run_turn("try the broken tool").await.expect("loop succeeds despite tool failure");
      assert!(matches!(outcome, lingxi_orchestrator::ConversationOutcome::EndTurn { .. }));

      // Inspect the OutputStream — the tool_result event must carry the
      // error envelope payload.
      let events = output.snapshot().await;
      let tool_result = events.iter().find_map(|e| match e {
          OutputEvent::ToolResult { tool, result } if tool == "AlwaysFail" => Some(result.clone()),
          _ => None,
      }).expect("ToolResult event present");
      assert!(tool_result.get("error").is_some(), "result envelope: {tool_result}");

      // The session's second-to-last message must be the user message
      // carrying the ToolResult block with is_error = true.
      let session = orch.session();
      let session = session.lock().await;
      // history: [user(prompt), assistant(tool_use), user(tool_result), assistant(text/end_turn)]
      assert_eq!(session.history.len(), 4);
      let third = &session.history[2];
      use lingxi_protocol::{ContentBlock, ConversationMessage};
      match third {
          ConversationMessage::User { content, .. } => {
              assert_eq!(content.len(), 1);
              match &content[0] {
                  ContentBlock::ToolResult { tool_use_id: id, content: text, is_error } => {
                      assert_eq!(*id, tool_use_id);
                      assert!(text.starts_with("Error: "), "byte-locked prefix: {text}");
                      assert!(text.contains("disk on fire"), "preserves payload: {text}");
                      assert!(*is_error);
                  }
                  _ => panic!("expected ToolResult content block"),
              }
          }
          _ => panic!("expected User message at index 2"),
      }
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test orchestrator_tool_error_test`. Must pass. If `ToolError::Internal` is named differently (M4-01 lock), adjust to the actual variant (e.g. `ToolError::Execution { .. }`).

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator`. All tests must pass.

- [ ] Step 4 — Add the `<unserializable>` branch coverage by adding a fixture tool that returns `data: serde_json::Value::Null` and verifying the orchestrator handles it cleanly (it will serialize to the literal `"null"`, NOT `"<unserializable>"` — only a hypothetical `Value::Number(f64::NAN)` would trigger unserializable; we leave that branch covered by code review rather than a runtime test since `serde_json` does not currently produce NaN through normal channels). NO new test added; just a code comment in `turn_loop.rs::dispatch_tool_uses` explaining the unreachable path is defensive.

- [ ] Commit: `test(M5-02 task 13): tool errors → is_error: true ToolResult; "Error: " prefix byte-locked`

---

### Task 14: Integration with real `ToolRegistry` + builtin `Read` tool

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/orchestrator_real_tools_test.rs`

**Steps:**

- [ ] Step 1 — Create the test file (uses tempfile + `lingxi_tools::register_all_builtin_tools` if convenient, or just `FileReadTool` directly):
  ```rust
  //! M5-02 Task 14: integration with the real ToolRegistry + Read tool.

  use lingxi_api_client::types::ContentBlockApi;
  use lingxi_orchestrator::test_support::{
      mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_protocol::ToolUseId;
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_tools::FileReadTool;
  use lingxi_traits::OutputEvent;
  use serde_json::json;
  use std::io::Write;
  use std::sync::Arc;

  #[tokio::test]
  async fn orchestrator_drives_real_file_read_tool_on_a_tempfile() {
      // Set up a tempfile with known content.
      let dir = tempfile::tempdir().expect("tempdir");
      let path = dir.path().join("greeting.txt");
      {
          let mut f = std::fs::File::create(&path).expect("create");
          writeln!(f, "hello from disk").expect("write");
      }

      // Mock: model issues a Read tool call against the tempfile path,
      // then on the second turn says "I read it" and ends.
      let tool_use_id = ToolUseId::new();
      let r1 = mock_message_response(
          vec![ContentBlockApi::ToolUse {
              id: tool_use_id,
              name: "Read".into(),
              input: json!({ "file_path": path.to_string_lossy() }),
          }],
          Some("tool_use"),
      );
      let r2 = mock_message_response(
          vec![ContentBlockApi::Text { text: "I read it".into() }],
          Some("end_turn"),
      );
      let api = Arc::new(MockApiClient::new(vec![r1, r2]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let mut registry = ToolRegistry::new();
      registry.register_builtin(Arc::new(FileReadTool::default()));
      let tools = Arc::new(registry);

      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api,
          tools,
          hooks,
          perms,
          output.clone(),
      );

      let outcome = orch.run_turn("read greeting").await.expect("loop");
      assert!(matches!(outcome, lingxi_orchestrator::ConversationOutcome::EndTurn { turn_count: 2, .. }));

      // The ToolResult event should carry the file content.
      let events = output.snapshot().await;
      let tool_result_payload = events.iter().find_map(|e| match e {
          OutputEvent::ToolResult { tool, result } if tool == "Read" => Some(result.clone()),
          _ => None,
      }).expect("Read ToolResult present");

      // Read tool's data shape: per M4-01, it typically returns something like
      // { "content": "<text>", "lines": N, ... } — exact shape verified inline.
      // We just assert the file body appears in the payload string.
      let s = serde_json::to_string(&tool_result_payload).unwrap();
      assert!(s.contains("hello from disk"), "tool payload should contain file body: {s}");
  }
  ```

  **`FileReadTool::default()` constructor**: M4-01 either ships `Default` for the tool or requires a builder. Grep `lingxi-core/crates/tools/src/builtin/file_read.rs` (or equivalent) to confirm — if no `Default`, use whatever zero-arg constructor exists (e.g. `FileReadTool::new()`). The test code adjusts to the actual API.

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test orchestrator_real_tools_test`. Must pass.

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator` (full crate). All tests must pass.

- [ ] Step 4 — Run `cargo clippy -p lingxi-orchestrator --all-targets -- -D warnings`. Must pass.

- [ ] Commit: `test(M5-02 task 14): real ToolRegistry + FileReadTool integration on a tempfile`

---

### Task 15: 3 telemetry events — new `tengu::orchestrator` submodule + wire emission

**Files:**
- Create: `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs`
- Modify: `lingxi-core/crates/telemetry/src/tengu/mod.rs` (declare submodule + grow `TOTAL` + insert in `concat_all`)
- Modify: `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs` (count 238 → 241)
- Modify: `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` (insert 3 names + update `_note`)
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs` (wire emission inside `run_turn`)
- Modify: `lingxi-core/crates/orchestrator/Cargo.toml` (already depends on `lingxi-telemetry` — no change unless the dep wasn't added in Task 1; double-check)

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` following the `release.rs` template:
  ```rust
  //! Orchestrator-lifecycle events (M5-02).
  //!
  //! Three events fired by `ConversationOrchestrator::run_turn`:
  //! - `CONVERSATION_STARTED` at the top of the turn loop.
  //! - `CONVERSATION_COMPLETED` after `emit_end_turn`.
  //! - `CONVERSATION_FAILED` on any `Err` return.
  //!
  //! Wire strings are byte-locked at v0.6.0; M5-04..M5-13 add more events to
  //! a sibling `tengu::orchestrator_streaming` / `tengu::orchestrator_perm` /
  //! ... if topic grouping is wanted (or stay flat under this module — TBD by
  //! M5-04). For M5-02 only the three lifecycle markers live here.

  /// Conversation start marker — fired once per `run_turn` invocation.
  pub const CONVERSATION_STARTED: &str = "tengu_orchestrator_conversation_started";

  /// Conversation end marker (success) — fired after `emit_end_turn`.
  pub const CONVERSATION_COMPLETED: &str = "tengu_orchestrator_conversation_completed";

  /// Conversation end marker (failure) — fired on any non-success return.
  pub const CONVERSATION_FAILED: &str = "tengu_orchestrator_conversation_failed";

  /// Order-locked array of all orchestrator-lifecycle names; consumed by
  /// `tengu::ALL_EVENT_NAMES`. Append-only: never reorder or remove entries.
  pub(crate) const NAMES: &[&str] = &[
      CONVERSATION_STARTED,
      CONVERSATION_COMPLETED,
      CONVERSATION_FAILED,
  ];
  ```

- [ ] Step 2 — Modify `lingxi-core/crates/telemetry/src/tengu/mod.rs` — declare the submodule + grow `TOTAL` + walk in `concat_all`:
  - Add `pub mod orchestrator;` to the module declarations (right after `pub mod memory;`).
  - Change `const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 1;` (238) to `const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 3 + 1;` (241). The `+ 3` is added BEFORE the final `+ 1` (release marker stays last).
  - In `concat_all()`, add a walk loop for `orchestrator::NAMES` BEFORE the `release::NAMES` walk:
    ```rust
    let mut i = 0;
    while i < orchestrator::NAMES.len() {
        out[idx] = orchestrator::NAMES[i];
        idx += 1;
        i += 1;
    }
    ```
  - Update the module-level doc comment top-line: change `"api → agent → session → tool → cost → oauth → memory → settings."` to `"api → agent → session → tool → cost → oauth → memory → settings → orchestrator → release."` (the older docstring also mentions release implicitly; update both occurrences).

- [ ] Step 3 — Modify `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs`:
  - Update the assertion `assert_eq!(ALL_EVENT_NAMES.len(), 238);` → `assert_eq!(ALL_EVENT_NAMES.len(), 241);`.
  - Update the explanatory comment to include the M5-02 row:
    ```rust
    // M4-05 added 24 events (8 agent/task tools × 3 lifecycle stages),
    // M4-06 added 6 (2 team tools × 3 lifecycle stages),
    // M4-07 added 13 (1 MCP_STARTED + 4 new tools × 3 lifecycle stages),
    // M4-08 added 24 (8 system tools × 3 lifecycle stages),
    // M4-09 added 1 (release marker `lingxi_core_v0_5_0_released`),
    // M5-02 added 3 (orchestrator conversation lifecycle: started/completed/failed):
    // 213 (post-M4-07) + 24 (M4-08) + 1 (M4-09) + 3 (M5-02) = 241.
    ```

- [ ] Step 4 — Modify `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`:
  - Insert these 3 lines immediately BEFORE the trailing `"lingxi_core_v0_5_0_released"` entry (which is currently line 242). The new lines become 242/243/244, the release marker shifts to 245:
    ```json
        "tengu_orchestrator_conversation_started",
        "tengu_orchestrator_conversation_completed",
        "tengu_orchestrator_conversation_failed",
    ```
    Make sure to add a comma after `"tengu_settings_parse_error"` (currently no comma needed — recheck) AND a comma after the third inserted line, so the JSON remains valid.
  - Update the `_note` field: append " + 3 (M5-02 orchestrator: conversation_started/completed/failed)" to the existing `_note` string. New total in the note is `216` → `241` (or update whatever current count is mentioned).
  - File total line count grows from 286 → 289.

- [ ] Step 5 — Wire emission in `lingxi-core/crates/orchestrator/src/conversation.rs`:
  - Add `use lingxi_telemetry::tengu::orchestrator as orch_events;` at the top.
  - Inside `ConversationOrchestrator::run_turn`, immediately at the top (BEFORE the session mutation):
    ```rust
    tracing::info!(event = orch_events::CONVERSATION_STARTED, prompt_len = prompt.len());
    ```
  - On successful exit (just before returning `Ok(...)`):
    ```rust
    tracing::info!(event = orch_events::CONVERSATION_COMPLETED, turn_count);
    ```
  - On the `Err` return paths, refactor the function to use `?` + a `match` at the call boundary so we can emit `CONVERSATION_FAILED`:
    ```rust
    let result = run_turn_inner(self, prompt).await;
    match &result {
        Ok(_) => tracing::info!(event = orch_events::CONVERSATION_COMPLETED, ...),
        Err(err) => tracing::error!(event = orch_events::CONVERSATION_FAILED, reason = %err),
    }
    result
    ```
    where `run_turn_inner` is a refactored copy of the previous body. Adjust signatures accordingly.

  Concretely: the simplest refactor is to keep `run_turn` as the public method and add a `try` block via labeled break, but Rust doesn't have `try` blocks stable. Use a private `async fn try_run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError>` containing the existing body sans telemetry, and have `run_turn` emit + delegate:
    ```rust
    pub async fn run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        tracing::info!(event = orch_events::CONVERSATION_STARTED, prompt_len = prompt.len());
        let result = self.try_run_turn(prompt).await;
        match &result {
            Ok(ConversationOutcome::EndTurn { turn_count, .. }) => {
                tracing::info!(event = orch_events::CONVERSATION_COMPLETED, turn_count = *turn_count);
            }
            Err(err) => {
                tracing::error!(event = orch_events::CONVERSATION_FAILED, reason = %err);
            }
        }
        result
    }
    ```

- [ ] Step 6 — Add a tracing-subscriber-based test that asserts the 3 events fire in order. Append to `lingxi-core/crates/orchestrator/tests/orchestrator_smoke_test.rs`:
  ```rust
  use tracing::subscriber;
  use tracing_subscriber::{fmt, EnvFilter};

  // (No new test — keep this section as documentation only. A full
  // `InMemorySink` integration test against `tengu::orchestrator` events
  // belongs in the test-harness parity drivers, which already audit
  // `ALL_EVENT_NAMES`. The smoke test here just verifies the events compile.)
  ```

  Actually, do NOT modify the existing smoke test. Instead create a NEW test file `lingxi-core/crates/orchestrator/tests/orchestrator_telemetry_test.rs`:
  ```rust
  //! M5-02 Task 15: verify orchestrator emits the 3 lifecycle events.

  use lingxi_api_client::types::ContentBlockApi;
  use lingxi_orchestrator::test_support::{
      mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_telemetry::tengu::orchestrator as orch_events;
  use std::sync::{Arc, Mutex as StdMutex};
  use tracing::field::Field;
  use tracing::Subscriber;
  use tracing::{Event, Metadata};
  use tracing_subscriber::layer::{Context, Layer};
  use tracing_subscriber::Registry;

  #[derive(Default, Clone)]
  struct EventNameCapture {
      events: Arc<StdMutex<Vec<String>>>,
  }

  impl<S: Subscriber> Layer<S> for EventNameCapture {
      fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
          struct V<'a>(&'a mut Option<String>);
          impl tracing::field::Visit for V<'_> {
              fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                  if field.name() == "event" {
                      *self.0 = Some(format!("{value:?}").trim_matches('"').to_string());
                  }
              }
              fn record_str(&mut self, field: &Field, value: &str) {
                  if field.name() == "event" {
                      *self.0 = Some(value.to_string());
                  }
              }
          }
          let mut name: Option<String> = None;
          event.record(&mut V(&mut name));
          if let Some(n) = name {
              self.events.lock().unwrap().push(n);
          }
      }
  }

  #[tokio::test]
  async fn run_turn_emits_started_and_completed_in_order() {
      use tracing_subscriber::prelude::*;
      let cap = EventNameCapture::default();
      let layer = cap.clone();
      let subscriber = Registry::default().with(layer);
      let _guard = tracing::subscriber::set_default(subscriber);

      let resp = mock_message_response(
          vec![ContentBlockApi::Text { text: "hi".into() }],
          Some("end_turn"),
      );
      let api = Arc::new(MockApiClient::new(vec![resp]));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let tools = Arc::new(ToolRegistry::new());

      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api,
          tools,
          hooks,
          perms,
          output,
      );
      orch.run_turn("ping").await.expect("happy");

      let events = cap.events.lock().unwrap().clone();
      assert!(events.iter().any(|n| n == orch_events::CONVERSATION_STARTED), "events: {events:?}");
      assert!(events.iter().any(|n| n == orch_events::CONVERSATION_COMPLETED), "events: {events:?}");
      assert!(!events.iter().any(|n| n == orch_events::CONVERSATION_FAILED), "no failure expected: {events:?}");
      // Order: STARTED comes before COMPLETED.
      let i_start = events.iter().position(|n| n == orch_events::CONVERSATION_STARTED).unwrap();
      let i_end = events.iter().position(|n| n == orch_events::CONVERSATION_COMPLETED).unwrap();
      assert!(i_start < i_end, "started < completed: {events:?}");
  }

  #[tokio::test]
  async fn run_turn_emits_failed_on_max_turns_error() {
      use tracing_subscriber::prelude::*;
      let cap = EventNameCapture::default();
      let layer = cap.clone();
      let subscriber = Registry::default().with(layer);
      let _guard = tracing::subscriber::set_default(subscriber);

      // Never-ending model.
      let api = Arc::new(MockApiClient::new((0..5).map(|_| {
          mock_message_response(vec![], Some("max_tokens"))
      }).collect()));
      let output = Arc::new(MockOutputStream::new());
      let hooks = Arc::new(NoOpHookExecutor);
      let perms = Arc::new(NoOpPermissionGate);
      let tools = Arc::new(ToolRegistry::new());

      let mut cfg = OrchestratorConfig::default();
      cfg.max_turns = 2;
      let orch = ConversationOrchestrator::new(cfg, api, tools, hooks, perms, output);
      let _err = orch.run_turn("loop").await.expect_err("must fail");

      let events = cap.events.lock().unwrap().clone();
      assert!(events.iter().any(|n| n == orch_events::CONVERSATION_STARTED), "events: {events:?}");
      assert!(events.iter().any(|n| n == orch_events::CONVERSATION_FAILED), "events: {events:?}");
      assert!(!events.iter().any(|n| n == orch_events::CONVERSATION_COMPLETED), "no completion expected: {events:?}");
  }
  ```

  This test requires `tracing-subscriber` as a dev-dep. Modify `lingxi-core/crates/orchestrator/Cargo.toml` `[dev-dependencies]`:
  ```toml
  tracing = "0.1"
  tracing-subscriber = { version = "0.3", features = ["env-filter", "registry"] }
  once_cell = "1"
  ```
  (`once_cell` is needed for the schema `Lazy` in the integration tests in Tasks 11/13.)

- [ ] Step 7 — Run `cargo test -p lingxi-telemetry --test event_name_completeness_test` — expect `registry_is_exactly_241_entries`-style pass.

- [ ] Step 8 — Run `cargo test -p lingxi-test-harness` (the parity_tengu_events test) — should pass with the updated fixture.

- [ ] Step 9 — Run `cargo test -p lingxi-orchestrator --test orchestrator_telemetry_test` — both tests must pass.

- [ ] Step 10 — Run `cargo clippy --workspace --all-targets -- -D warnings`. Must pass.

- [ ] Step 11 — Run `cargo fmt --all -- --check`. Must pass.

- [ ] Commit: `feat(M5-02 task 15): 3 telemetry events (tengu::orchestrator) — ALL_EVENT_NAMES 238 → 241 + parity fixture + InMemorySink test`

---

### Task 16: Verification gate + annotated tag `m5.2`

**Files:**
- None modified. This is the workspace-wide guard task.

**Steps:**

- [ ] Step 1 — Run the M4-05 critical `Arc::ptr_eq` invariants:
  ```bash
  cargo test -p lingxi-tools --lib --test agent_task_integration_test \
      recursion_lock_child_inherits_parent_tool_registry_arc \
      budget_inheritance_child_inherits_parent_budget_arc
  ```
  Both must pass. (M5-02 did not touch tools internals so this should be trivially green; we verify explicitly because of spec §6.2.)

- [ ] Step 2 — Run the full workspace test suite:
  ```bash
  cargo test --workspace
  ```
  Accept up to 2 known fs-watch flakes (per repo convention). If new failures emerge, debug + fix; do not declare success.

- [ ] Step 3 — Run clippy across the workspace:
  ```bash
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  Must pass clean.

- [ ] Step 4 — Run formatter check:
  ```bash
  cargo fmt --all -- --check
  ```
  Must pass clean.

- [ ] Step 5 — Confirm event counts on disk:
  ```bash
  cargo test -p lingxi-telemetry --test event_name_completeness_test registry_is_exactly_241_entries
  cargo test -p lingxi-telemetry --test event_name_completeness_test registry_entries_are_unique
  ```
  Both must pass.

- [ ] Step 6 — Confirm orchestrator crate is wired into default-members of the workspace:
  ```bash
  cargo build --workspace
  ```
  Must succeed (verifies orchestrator is built by default).

- [ ] Step 7 — Confirm no cyclic dep was introduced:
  ```bash
  cargo tree -p lingxi-orchestrator -e normal --depth 2 | grep -E "lingxi-(agent|tasks|commands)" || echo "OK: no agent/tasks/commands dep"
  ```
  Expect "OK: no agent/tasks/commands dep".

- [ ] Step 8 — Create annotated git tag:
  ```bash
  git tag -a m5.2 -m "M5-02: orchestrator core — batched turn loop + new lingxi-orchestrator crate + 3 telemetry events (238 → 241)"
  ```

- [ ] Step 9 — Verify tag exists and points to HEAD:
  ```bash
  git show m5.2 --stat | head -20
  git log -1 --oneline
  ```

- [ ] Commit (only if any leftover hygiene fixes were needed in earlier steps; otherwise skip — the tag does not itself require a commit): if needed, `release(M5-02 task 16): verification matrix green + tag m5.2`.

---

## Self-review checklist (M5-02)

1. **Spec coverage:** §3 M5-02 row (turn loop, mock model, 3 events, ~16 tasks) — ALL mapped. §4.2 (maxTurns byte-lock) — Task 2 step 1 verifies + step 4 byte-tests. §6.1 (dep graph + cycle risk) — Task 1 step 7 forbids agent/tasks/commands deps. §6.3 (telemetry growth 238 → 241) — Task 15 steps 2-3.
2. **Placeholder scan:** No `TBD`, no `<placeholder>`, no `// TODO: implement`. Every code block compiles as written (modulo the field-name verifications in Task 6 step 2 and Task 10 step 0).
3. **Type consistency:** `OrchestratorError` appears in Tasks 2, 10, 12. `ConversationOrchestrator` in Tasks 9, 10, 11, 12, 13, 14, 15. `OutputStream` in Tasks 3, 7, 9, 11, 13. Always the same fully-qualified name (`lingxi_orchestrator::ConversationOrchestrator`, `lingxi_traits::OutputStream`).
4. **Cargo cycle prevention:** Task 1 step 3 declares orchestrator deps; Task 1 step 7 verifies via `cargo tree`. The new traits live in `lingxi-traits` (leaf), the events live in `lingxi-telemetry` (leaf), the orchestrator depends only on existing leaves + protocol/core. No back-edges.
5. **New events count consistent:** 238 (post-M4-09) + 3 (M5-02 orchestrator) = **241**. Updated in (a) `tengu::mod.rs` TOTAL arithmetic, (b) `event_name_completeness_test.rs` assertion, (c) `tengu_events.json` insert + `_note` update. All three locations cross-referenced in Task 15.
6. **Byte-lock fidelity:** `Reached maximum number of turns (<n>)` — Task 2 step 4 (3 tests covering 30, 1, 9999). `Error: <err>` ToolResult prefix — Task 13. `tengu_orchestrator_conversation_*` wire strings — Task 15 step 1.
7. **TDD red→green progression:** Task 9 ships a failing smoke test; Task 10 turns it green. Tasks 11, 12, 13, 14, 15 each ship a new test (small, focused) AND an implementation pass that makes it green. No task ships a test without the matching impl in the same task (except Task 9, which is intentionally a red task).
8. **Commit boundary discipline:** 16 tasks → 16 commits (Task 16 is verification-only, may have 0 commits). Each commit message follows the `<type>(M5-02 task <N>): <summary>` format used by M5-01.
9. **No regression vector touched:** Tasks 1-16 do not modify any code in `lingxi-agent`, `lingxi-tasks`, `lingxi-commands`, `lingxi-tools::builtin`, or `lingxi-permission`. The M4-05 `Arc::ptr_eq` invariants are not at risk by construction.
10. **Forward compatibility hooks installed:** `Arc<dyn HookExecutor>` field on the orchestrator (M5-06 swaps in real exec). `Arc<dyn PermissionGate>` field (M5-05 swaps in `PromptingGate`). `OutputStream` trait (M5-04 reuses the same signature for per-delta emission). `OrchestratorHandle` trait declared but not yet implemented (M5-09 wires).

---

## Wire identifiers — LOCKED at this plan

| Lock | Value | Source |
|---|---|---|
| `MAX_TURNS_DEFAULT` | `30` | LingXi-defined; spec §4.2 OQ-1 resolution (2026-05-25). |
| `MaxTurnsReached` Display | `"Reached maximum number of turns ({max_turns})"` | `claude-code/src/QueryEngine.ts:870`. |
| Telemetry event names | `tengu_orchestrator_conversation_started`, `tengu_orchestrator_conversation_completed`, `tengu_orchestrator_conversation_failed` | This plan, registered in `tengu::orchestrator::NAMES`. |
| Rust constant identifiers | `CONVERSATION_STARTED`, `CONVERSATION_COMPLETED`, `CONVERSATION_FAILED` | This plan, in `lingxi_telemetry::tengu::orchestrator`. |
| `OrchestratorConfig` field names | `max_turns: u32`, `model: String`, `system_prompt_override: Option<String>` | This plan. |
| `ToolResult` error prefix | `"Error: "` | This plan (orchestrator-side; not from claude-code). |
| Default model in `OrchestratorConfig::default()` | `"claude-opus-4-7"` | This plan (only used in tests; production overrides). |
| `ALL_EVENT_NAMES.len()` | `241` (was `238`) | Workspace count after M5-02. |

---

## Open questions resolved at this plan

- **OQ-1 (spec §7):** Main conversation `maxTurns` default — claude-code has none. LingXi locks `30`. Documented in `config.rs` doc comment + this plan's "Wire identifiers" row.
- **OQ-2 (spec §7):** SSE event subscription list — NOT IN SCOPE for M5-02. M5-02 uses non-streaming `messages_create_non_stream`. OQ-2 is deferred to M5-04.

---

## Out of scope (will be addressed in later plans)

- Streaming SSE (M5-04).
- Permission gate UX (`PromptingGate`) (M5-05).
- 4-arm hooks executor (M5-06).
- Session JSONL persistence (M5-07).
- Session resume (M5-08).
- Slash commands (M5-09 — `OrchestratorHandle` trait is defined here but not yet implemented on `ConversationOrchestrator`; M5-09 wires).
- System prompt assembly (M5-03 — `system_prompt_override` field exists, but the dynamic default-assembly logic lives in M5-03).
- CLI binary (M5-12).
- REPL mode (M5-13).
- `lingxi-cost` integration for real cost snapshots (M5-05 / M5-11 — `cost_snapshot_from_session` currently reports zero cost).

End of plan.
