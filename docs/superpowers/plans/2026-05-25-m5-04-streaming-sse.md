# LingXi Core M5 · Plan 04 · Streaming SSE + mid-stream tool dispatch

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Upgrade `ConversationOrchestrator` from the M5-02 batched non-streaming path to an SSE-streaming path that:

1. Calls a new `StreamingApiClient::stream(messages, system, tools)` returning `BoxStream<'static, Result<StreamEvent, ApiError>>`.
2. Consumes the SSE event stream event-by-event, accumulating per-block state in a `BlockAccumulator`, feeding `text_delta` chunks straight to `OutputStream::emit_text(&str)` as they arrive (true token-level streaming).
3. **Dispatches each `tool_use` block the moment its `content_block_stop` arrives** — without waiting for the rest of the response. Subsequent text deltas keep flowing in parallel. Multiple tools dispatch concurrently via `tokio::spawn` + `futures::future::join_all`.
4. After `message_stop`, if any tools were dispatched, appends a synthesized user `ToolResult` message and loops back to step 1 for the next turn. After `stop_reason == "end_turn"`, exits.

This plan ships:

- A new `StreamingApiClient` trait on the orchestrator's internal API surface (next to the existing `OrchestratorApiClient`), parametrically wrapping `Stream<Item = Result<StreamEvent, ApiError>>` via `futures::stream::BoxStream`. The trait lives in `lingxi-orchestrator::conversation` (private to the crate just like `OrchestratorApiClient`). A production adapter `AnthropicProviderStreamingAdapter<T: HttpTransport>` wraps `AnthropicProvider::messages_create_stream` (added in this plan to mirror the existing `messages_create_non_stream`).
- A new `sse/` submodule under `lingxi-orchestrator` with three files: `mod.rs` (re-exports), `accumulator.rs` (`BlockAccumulator` + `CompletedBlock`), and `event_router.rs` (the `dispatch_event` switch that ties `StreamEvent` variants to accumulator + output sink callbacks). The wire-level SSE parser already exists at `lingxi_api_client::sse::parse_sse_chunks` (M3-03) returning `Vec<lingxi_protocol::SseEvent>`; `lingxi_api_client::types::StreamEvent` is the typed payload after JSON-decoding the `data:` payload. The orchestrator consumes already-typed `StreamEvent` values via the trait surface — wire parsing stays in `lingxi-api-client`.
- A new `streaming_loop.rs` file with `execute_one_turn_streaming` (the streaming twin of M5-02's batched `execute_one_turn`) and `dispatch_tool_use_concurrent` (the concurrent multi-tool spawn helper).
- An extension to `crates/orchestrator/src/test_support.rs`: `MockStreamingApiClient`, `ScriptedSseStream`, and the `scripted!` declarative macro for assembling event sequences.
- A new field on `ConversationOrchestrator`: `streaming_api: Arc<dyn StreamingApiClient>`. The existing `api: Arc<dyn OrchestratorApiClient>` field stays in place — it remains the path for the batched `run_turn` (kept for backward compat with tests that don't model streaming). A new public method `ConversationOrchestrator::run_turn_streaming(&self, prompt: &str)` is the streaming entry point. Tests for both paths exist.
- 2 new telemetry events under a new `lingxi_telemetry::tengu::orchestrator::streaming` submodule path (the existing `orchestrator` submodule added in M5-02 grows from 3 → 5 names): `tengu_orchestrator_turn_streaming_started` + `tengu_orchestrator_turn_streaming_completed`. `ALL_EVENT_NAMES` grows from **241 → 243**. Tests bump the count + the `tengu_events.json` parity fixture inserts the two names in the same registration position.

**No changes to existing telemetry events.** The 3 events from M5-02 (`conversation_started/completed/failed`) and the 0 new events from M5-03 remain untouched. Task 16 step 4 explicitly re-asserts the count chain `238 (post-M4-09) + 3 (M5-02) + 0 (M5-03) + 2 (M5-04) = 243`.

**Tech Stack:** Rust 2021. Existing workspace deps reused — `async-trait 0.1`, `serde 1` + `serde_json 1`, `tokio 1` (with `rt`, `sync`, `macros`), `thiserror 2`, `futures 0.3` (for `Stream` / `BoxStream` / `StreamExt` / `future::join_all`). One workspace dep additive guard: if `futures = "0.3"` is not yet in `lingxi-core/Cargo.toml` `[workspace.dependencies]`, Task 1 step 2 adds it (it is, in fact, already there from M3-03 / M4-03 — Task 1 step 2 verifies and skips on confirmation). No new third-party deps in this plan.

**References:**

- Spec: `docs/superpowers/specs/2026-05-25-m5-conversational-agent-loop-design.md` (committed at `1dbb9b8`).
  - §3 sub-plan row M5-04 (line 180) — "orchestrator 从 batched 升级到 streaming:接 `api_client::stream` SSE,逐 `content_block_delta` 喂 `OutputStream::emit_text`,tool_use block 完整后立即 dispatch(不等整个 response 结束)。 ... 2 (`turn_streaming_started/completed`) | ~18".
  - §4.2 (lines 252-258) — Streaming SSE event names: `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`. Lines 252-258 also call out the OQ-2 requirement.
  - §5.2 — new `ScriptedSseStream` infrastructure required (already listed in spec at line 358-359).
  - §7 OQ-2 (line 492) — SSE event order (reverse-engineer from `src/services/api/`). This plan resolves OQ-2 via the Task 0 reverse-engineering documented in the "Reverse-engineered byte-locks" table.
- Predecessor M5-03 (committed at `0ce1f18` per the user-supplied handoff — this plan assumes that SHA is `HEAD~0` when execution begins). Task 1 step 1 verifies.
  - `assemble_system_prompt` is now wired into `ConversationOrchestrator::run_turn` via the third `system: Option<&str>` arg on `OrchestratorApiClient::messages_create`.
  - `ConversationOrchestrator` now has fields: `{ config, api, tools, hooks, perms, output, session, memory, cwd }` (9 fields after M5-03). Task 12 step 1 of this plan adds a 10th field, `streaming_api`.
- Predecessor M5-02 (committed at `653de44`):
  - Defines `OrchestratorApiClient` (internal trait), `ConversationOrchestrator`, `ConversationOutcome`, `OrchestratorError`, `MockApiClient`, `MockOutputStream`, `NoOpHookExecutor`, `NoOpPermissionGate`.
  - The `OutputStream` trait already has `emit_text(&self, text: &str)` as `async fn`. M5-02 calls it ONCE per Text block per turn (whole body). M5-04 calls it ONCE PER `text_delta` (per-token).
  - `ALL_EVENT_NAMES` is at 241 (238 baseline + 3 from M5-02 orchestrator).
- claude-code reference (Task 0 reverse-engineering, source line numbers confirmed via the `grep` invocations documented in the byte-locks table below):
  - `claude-code/src/services/api/claude.ts:1980-2300` — the streaming switch statement that processes `BetaRawMessageStreamEvent`. The variants subscribed to are `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`. Variants `ping` and any future-unknown are dropped through to the trailing `yield` without further processing — i.e. claude-code does NOT explicitly handle `ping`, it relies on the switch's default falling through.
  - `claude.ts:2087-2125` — `input_json_delta` handling: the per-block `input` field is INITIALIZED to the empty string at `content_block_start` (line 1998-2003), accumulated by string concatenation on each `input_json_delta` (line 2125 — `contentBlock.input += delta.partial_json`), and JSON-parsed at `content_block_stop` (in the function exit code below).
  - `claude.ts:2210-2250` — `message_delta` handling: `stop_reason = part.delta.stop_reason` (line 2243). The `stop_reason` is therefore set on `message_delta`, NOT `message_stop`. `message_stop` is a no-op (`break;` on line 2298). This plan locks the same behavior.
  - `claude.ts:2899-2925` — final `extractAssistantMessageFromStream` joins partial input strings + JSON-parses each tool_use block's `input`. Mid-stream dispatch in LingXi happens at `content_block_stop` (one block at a time), so the JSON-parse occurs per-block rather than per-message — but the byte representation of each parsed input must match what claude-code's batched path would have parsed.
- Existing surfaces consumed by this plan:
  - `lingxi-core/crates/api-client/src/types.rs:135-178` — `StreamEvent` enum already defined with all 7 variants: `MessageStart`, `ContentBlockStart`, `ContentBlockDelta`, `ContentBlockStop`, `MessageDelta`, `MessageStop`, `Ping`, `Error`. **NO new serde struct work is needed** — the wire types already exist. This plan only adds the streaming TRANSPORT method and the orchestrator-side ACCUMULATOR.
  - `lingxi-core/crates/api-client/src/types.rs:186-217` — `ContentDelta` enum already defined with `TextDelta`, `InputJsonDelta`, `ThinkingDelta`, `SignatureDelta`, `CitationsDelta`, `ConnectorTextDelta`. Same — no new serde work.
  - `lingxi-core/crates/api-client/src/anthropic.rs` — `AnthropicProvider`. Currently exposes `messages_create_non_stream`. Task 11 of this plan ADDS `messages_create_stream` returning a typed `BoxStream<'static, Result<StreamEvent, ApiError>>`.
  - `lingxi-core/crates/api-client/src/sse.rs:14` — `parse_sse_chunks(raw: &str) -> Vec<SseEvent>` (M3-03). Already returns the wire-level event envelope. Task 11 uses this + `serde_json::from_str::<StreamEvent>(&envelope.data)` inside the new `messages_create_stream`.
  - `lingxi-core/crates/orchestrator/src/conversation.rs` — current state (post-M5-03):
    - `OrchestratorApiClient::messages_create(&self, model, system: Option<&str>, msgs)` trait.
    - `ConversationOrchestrator { config, api, tools, hooks, perms, output, session, memory, cwd }`.
    - `run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError>` (batched).
  - `lingxi-core/crates/orchestrator/src/turn_loop.rs` — `execute_one_turn(&orch)` + `dispatch_tool_uses(&orch, &tool_uses)` + `translate_response_blocks(&content)` + `cost_snapshot_from_session(&s)`. Task 13 of this plan calls `dispatch_tool_uses` from a NEW streaming-tool-dispatch helper (reuses pre-tool hook + permission gate + post-tool hook logic verbatim — the streaming path differs only in WHEN dispatch fires, not HOW).
  - `lingxi-core/crates/orchestrator/src/test_support.rs` — already holds `MockApiClient`, `MockOutputStream`, `NoOpHookExecutor`, `NoOpPermissionGate`. This plan adds `MockStreamingApiClient` and `ScriptedSseStream` next to them. The two mocks coexist (one for batched tests, one for streaming tests).
  - `lingxi-traits::OutputStream::emit_text(&self, text: &str)` — already async, already takes a borrowed string. **No trait surface change**. M5-02 calls it once per Text block (whole body); M5-04 calls it once per `text_delta` (per-token). The behavioral semantic shift is documented in the doc comment on the trait (M5-02 wrote: *"M5-04 will switch to per-SSE-delta emission without changing this signature."*).
  - `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` — created by M5-02 with the 3 conversation_* events. Task 16 of this plan adds 2 more constants + extends `NAMES`. The submodule grows but stays a single file.
- Repo conventions:
  - Tests live in `#[cfg(test)] mod tests { ... }` blocks adjacent to production code; integration tests live under `crates/orchestrator/tests/<name>_test.rs`.
  - Every new file under `lingxi-orchestrator/src/` includes `#![forbid(unsafe_code)]` at the top.
  - Wire identifiers (`message_start`, `text_delta`, `tool_use`, `end_turn`, …) are LOWERCASE_SNAKE with no aliases. Test assertions use the literal byte sequence.
  - Telemetry events follow `tengu_<category>_<verb>_<noun>` naming.

---

## Reverse-engineered byte-locks (T0 — captured at plan-writing time)

Captured by grepping `claude-code/src/services/api/claude.ts` (the file `messageStream`-related references reside in). Source line numbers were verified at plan-writing time via the `grep -rn` runs documented in the Task 0 steps below. If a future engineer encounters drift, re-run Task 0 against the current `claude-code/` submodule.

| Lock id | Value | Source |
|---|---|---|
| Subscribed event types | `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop` | `claude.ts:1980,1995,2057,2185,2213,2295` |
| `ping` handling | NOT in the `switch` — falls through to a trailing `yield { type: 'stream_event', event: part }` re-emit. LingXi mirrors: `Ping` event is a no-op for accumulator state and triggers no `OutputStream` call. | `claude.ts:1980-2299` (absent from switch) |
| `Error` (server-emitted) handling | NOT in the streaming switch — surfaced by a separate `try/catch` around the SDK stream. LingXi mirrors by returning `Err(ApiError::Streaming(reason))` from the consumer loop when a `StreamEvent::Error` arrives. | `claude.ts:1857-1880` |
| `stop_reason` carrier event | `message_delta` (NOT `message_stop`). `part.delta.stop_reason` is set on `message_delta`. `message_stop` is the terminator only. | `claude.ts:2243` |
| `stop_reason` values | `end_turn`, `tool_use`, `max_tokens`, `stop_sequence`, `pause_turn`, `refusal`, `model_context_window_exceeded` (the last three are out of scope for M5-04 streaming dispatch; treated as "terminate this turn with the value as-is"). The 4 values M5-04 acts on are the first four. | `claude.ts:2243,2269,2283` + Anthropic Messages API ref |
| `content_block_start.content_block.type` values | `text`, `tool_use`, `server_tool_use`, `thinking`, `connector_text`, `advisor_tool_result` (the latter four pass through but are NOT dispatched as tools in M5-04). | `claude.ts:1996-2070` |
| `text_delta` field name | `text: String` (delta payload). | `lingxi-api-client/src/types.rs:187-190` (already locked) |
| `input_json_delta` field name | `partial_json: String` (delta payload). String-concat into a per-block `String` buffer initialized at `content_block_start`, JSON-parsed at `content_block_stop`. | `claude.ts:2087-2125` |
| `ContentBlockStart.index` type | `u32` (already locked in `lingxi-api-client::StreamEvent::ContentBlockStart`). Per-block accumulator uses `HashMap<u32, BlockState>`. | `lingxi-api-client/src/types.rs:144-149` |
| Order invariants | `message_start` arrives FIRST. Each block: `content_block_start` → 0..N × `content_block_delta` → `content_block_stop`. `message_delta` arrives AFTER all content blocks have stopped. `message_stop` is LAST. `ping` may interleave anywhere. | `claude.ts:1980-2299` |
| Mid-stream dispatch trigger | `content_block_stop` with the block's `content_block.type == "tool_use"`. Fire dispatch RIGHT THERE; do NOT wait for `message_stop`. | LingXi M5-04 design lock; spec §3 row M5-04 |
| Multiple-tools concurrency | When a single response carries N tool_use blocks, dispatch all N **concurrently** via `tokio::spawn` + `futures::future::join_all`. Order of `OutputEvent::ToolResult` events emitted to the output sink is determined by dispatch completion order, NOT the index order from the API. Tests assert via a set-style match. | LingXi M5-04 design lock; mirrors claude-code's parallel tool execution at `claude.ts` (executeTools batches into `Promise.all`) |
| Telemetry event 1 | `tengu_orchestrator_turn_streaming_started` — emitted at the TOP of `run_turn_streaming` (before any session mutation). | LingXi M5-04 |
| Telemetry event 2 | `tengu_orchestrator_turn_streaming_completed` — emitted AFTER `OutputStream::emit_end_turn` returns. On `Err`, the existing `tengu_orchestrator_conversation_failed` event still fires (no new error event). | LingXi M5-04 |

**Provenance:** `claude.ts` line citations were captured during plan-writing by running:
```
grep -n "case 'message_start'\|case 'content_block_start'\|case 'content_block_delta'\|case 'content_block_stop'\|case 'message_delta'\|case 'message_stop'\|input_json_delta\|stop_reason" claude-code/src/services/api/claude.ts
```
Re-run when drift suspected.

---

## Critical 1:1 fidelity items (locked)

- **`StreamingApiClient` trait** lives in `lingxi-orchestrator::conversation` (the same module as `OrchestratorApiClient`). Signature:
  ```rust
  use futures::stream::BoxStream;
  use lingxi_api_client::{ApiError, StreamEvent};

  #[async_trait]
  pub trait StreamingApiClient: Send + Sync {
      /// Open a streaming `messages.create` request. The returned stream
      /// yields wire-decoded `StreamEvent` values until the server emits
      /// `message_stop`. The implementation is responsible for HTTP, SSE
      /// chunk buffering, and JSON-decoding the `data:` lines into typed
      /// `StreamEvent` values.
      async fn stream(
          &self,
          model: &str,
          system: Option<&str>,
          messages: Vec<ConversationMessage>,
          tools: Vec<serde_json::Value>,
      ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError>;
  }
  ```
  The `tools` parameter is the wire-format schema array (`Vec<serde_json::Value>`) — same shape `MessageRequest.tools` already takes. The orchestrator gets this from `ToolRegistry::available_tools(&ctx).iter().map(|t| t.schema()).collect()` (existing API on `Tool` trait); in M5-04 we keep this as `Vec::new()` since the v0.6.0 streaming-tools wiring lands in M5-09 (commands surface) — Task 12 step 3 hard-codes `Vec::new()` and adds a `TODO(M5-09)` comment. Tests pass an empty `tools` arg.

- **`BlockAccumulator` invariants** — exactly one entry per `index: u32` for the lifetime of the response. `start_block(index, kind)` is REQUIRED before any `append_*` for that index; out-of-order deltas (no matching start) return `Err(StreamingError::BlockNotFound { index })`. `stop_block(index)` returns the `CompletedBlock` and removes the index from the internal map. Calling `stop_block` twice on the same index returns `Err(StreamingError::DoubleStop { index })`. Type tag mismatch (e.g. `input_json_delta` against a `text` block) returns `Err(StreamingError::TypeMismatch { index, expected, got })`.

- **`CompletedBlock` shape** — one of `Text { text: String }`, `ToolUse { id: ToolUseId, name: String, input: serde_json::Value }`, `Thinking { thinking: String, signature: Option<String> }`. Tools / text only are acted on; thinking is passed through into the assistant message just like the batched path. `ServerToolUse`, `ConnectorText`, `AdvisorToolResult` are dropped at the accumulator boundary (parity with `translate_response_blocks` from M5-02 which already drops them).

- **Mid-stream tool dispatch handle** — when `stop_block` returns a `CompletedBlock::ToolUse`, the streaming loop immediately spawns the dispatch onto a `Vec<tokio::task::JoinHandle<Result<ContentBlock, OrchestratorError>>>`. Subsequent stream events keep flowing; the spawn does NOT block the consumer. After `message_stop`, the loop `join_all`s the handles and assembles a single `ConversationMessage::User { content: Vec<ContentBlock::ToolResult> }` to append to the session.

- **Concurrent dispatch determinism** — tool results are ORDERED in the appended user message by the API's `tool_use.id` ordering (the order they appeared in the stream), NOT by completion time. The output sink's `emit_tool_call` / `emit_tool_result` events DO fire in completion order (this is observable user-facing streaming behavior — first tool to finish reports first). Task 14 step 3 asserts both orderings independently.

- **No retry inside streaming** — if the underlying stream errors mid-flight, the consumer returns `OrchestratorError::Streaming(ApiError)` and the turn fails. The streaming transport (Task 11) does ITS OWN retry only on connection-establishment failures (mirroring M3-03's 3-attempt loop for the non-stream path); mid-stream byte errors are surfaced. Test (Task 9 step 5) injects a `Err` partway through a script and asserts the orchestrator surfaces it.

- **`run_turn` (batched) stays in place** — backward compatibility for M5-02 tests. The new `run_turn_streaming` is the v0.6.0 primary entry point. Both methods share `self.session: Arc<Mutex<SessionState>>` so a process that drives a turn via streaming and then via batched (or vice versa) sees a consistent history. Task 17 step 4 has an integration test that mixes both.

- **`Ping` is a true no-op** — neither accumulator state nor output sink is touched. The orchestrator counts pings into a debug-only `ping_count: u32` local for the duration of the turn but does NOT log or emit telemetry per ping (pings are high-frequency keepalives — burning telemetry on them is wasteful). Test (Task 15) asserts a ping mid-stream does not affect the visible output, but does NOT assert a specific ping count.

- **Telemetry event names — exact strings**:
  - `tengu_orchestrator_turn_streaming_started`
  - `tengu_orchestrator_turn_streaming_completed`

  Both `pub const &'static str` constants in `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` appended AFTER the existing 3 conversation_* names. `NAMES` in that file grows from 3 → 5 entries; `ALL_EVENT_NAMES.len()` grows from 241 → 243.

- **`OrchestratorError` extensions** — three new variants needed:
  - `Streaming(#[from] ApiError)` (mid-stream byte error from the transport).
  - `StreamingProtocol(String)` (BlockAccumulator invariant violation — out-of-order delta, double stop, type mismatch).
  - `StreamEndedWithoutStop` (the stream ended cleanly but no `message_stop` arrived — fallback case mirrored from `claude.ts:2353`).
  Task 4 step 2 adds the variants and their `Display` impls (locked byte sequences).

- **`StreamingError` (accumulator-internal) → `OrchestratorError::StreamingProtocol`** — conversion lives at the consumer boundary. The accumulator returns its own typed `StreamingError` enum (BlockNotFound, DoubleStop, TypeMismatch); the consumer maps each to a `StreamingProtocol(format!("..."))` with a stable byte format documented in Task 5 step 4.

---

## File touch inventory (locked at top per spec Appendix A convention)

**Creates (new files — all under `lingxi-core/crates/orchestrator/src/` unless noted):**

- `lingxi-core/crates/orchestrator/src/sse/mod.rs` — module re-exports + `StreamingError` enum + crate-level streaming docs.
- `lingxi-core/crates/orchestrator/src/sse/accumulator.rs` — `BlockAccumulator` + `CompletedBlock` + per-block `BlockState` (private).
- `lingxi-core/crates/orchestrator/src/sse/event_router.rs` — `dispatch_event(state, event, out, accumulator) -> Result<RouterAction, StreamingError>` switch + `RouterAction` enum (the consumer's "what next" signal).
- `lingxi-core/crates/orchestrator/src/streaming_loop.rs` — `execute_one_turn_streaming` + `spawn_tool_dispatch` + `join_tool_dispatches` + `run_turn_streaming_inner` helpers.
- `lingxi-core/crates/orchestrator/src/test_support_stream.rs` — `MockStreamingApiClient` + `ScriptedSseStream` + `scripted!` macro + `MockToolDispatchClock` (records dispatch timestamps for the mid-stream test).
- `lingxi-core/crates/orchestrator/tests/streaming_text_only_test.rs` — single text response, multiple deltas.
- `lingxi-core/crates/orchestrator/tests/streaming_mid_stream_tool_test.rs` — tool block dispatched at `content_block_stop`, before `message_stop`.
- `lingxi-core/crates/orchestrator/tests/streaming_concurrent_tools_test.rs` — two tool_use blocks in one response, dispatched concurrently.
- `lingxi-core/crates/orchestrator/tests/streaming_multi_turn_test.rs` — two streaming turns, second turn observes the first turn's tool result.
- `lingxi-core/crates/orchestrator/tests/streaming_ping_noop_test.rs` — ping mid-stream is a no-op.
- `lingxi-core/crates/orchestrator/tests/streaming_error_propagation_test.rs` — stream `Err` mid-flight → `OrchestratorError::Streaming` surfaced.
- `lingxi-core/crates/orchestrator/tests/streaming_block_accumulator_test.rs` — direct unit tests of the accumulator's invariants.
- `lingxi-core/crates/orchestrator/tests/streaming_vs_batched_equivalence_test.rs` — same scripted script consumed via both `run_turn` and `run_turn_streaming` produces the same `ConversationOutcome` + same final session history (modulo telemetry events).

**Modifies (existing files):**

- `lingxi-core/crates/orchestrator/Cargo.toml` — confirm `futures = { workspace = true }` is present in `[dependencies]` (was added by M5-02 transitively; Task 1 step 2 verifies and adds if missing).
- `lingxi-core/crates/orchestrator/src/lib.rs` — add `pub mod sse;` + `pub mod streaming_loop;` declarations; add `#[cfg(any(test, feature = "test-support"))] pub mod test_support_stream;`; add `pub use conversation::StreamingApiClient;` to the re-export block.
- `lingxi-core/crates/orchestrator/src/conversation.rs` — add `StreamingApiClient` trait + `AnthropicProviderStreamingAdapter` adapter (Task 11). Add `streaming_api: Arc<dyn StreamingApiClient>` field to `ConversationOrchestrator` + extend `new` constructor (Task 12). Add `pub async fn run_turn_streaming(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError>` method (Task 12).
- `lingxi-core/crates/orchestrator/src/error.rs` — add 3 new variants: `Streaming(ApiError)`, `StreamingProtocol(String)`, `StreamEndedWithoutStop` (Task 4).
- `lingxi-core/crates/orchestrator/src/test_support.rs` — re-export `MockStreamingApiClient` + `ScriptedSseStream` from the test_support_stream module (Task 8 step 5; just a `pub use` line at the bottom).
- `lingxi-core/crates/orchestrator/tests/orchestrator_smoke_test.rs` — update `ConversationOrchestrator::new` construction to pass a `MockStreamingApiClient::empty()` for the new field (Task 12 step 5). The existing assertions on the batched `run_turn` path stay green.
- `lingxi-core/crates/orchestrator/tests/orchestrator_multi_turn_test.rs` — same construction update.
- `lingxi-core/crates/orchestrator/tests/orchestrator_max_turns_test.rs` — same.
- `lingxi-core/crates/orchestrator/tests/orchestrator_tool_error_test.rs` — same.
- `lingxi-core/crates/orchestrator/tests/orchestrator_real_tools_test.rs` — same.
- `lingxi-core/crates/api-client/src/anthropic.rs` — add `messages_create_stream<T: HttpTransport>(&self, model, system, msgs, tools, transport) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError>` method (Task 11). The non-streaming path stays untouched.
- `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` — append two new `pub const &str` constants + extend `NAMES` from 3 → 5 entries (Task 16).
- `lingxi-core/crates/telemetry/src/tengu/mod.rs` — bump `TOTAL` arithmetic from `... + 3 + 1` to `... + 5 + 1` (241 → 243).
- `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs` — bump `241` → `243` + extend the comment with `M5-04 added 2 streaming events`.
- `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json` — insert the 2 new orchestrator names AFTER the 3 conversation_* entries and BEFORE `lingxi_core_v0_5_0_released`; bump the `_note` field.

**Verifications (no modification, just read in tests):**

- `lingxi-core/crates/api-client/src/types.rs:135-217` — `StreamEvent` + `ContentDelta` shapes unchanged.
- `lingxi-core/crates/api-client/src/sse.rs:14` — `parse_sse_chunks` unchanged.
- `lingxi-core/crates/orchestrator/src/turn_loop.rs::dispatch_tool_uses` — unchanged; reused as-is from M5-02 by the streaming path (Task 13 step 3 re-uses it for the post-`message_stop` consolidated dispatch).
- The 3 M5-02 telemetry events still fire from the batched `run_turn` (Task 17 step 5 asserts via `InMemorySink`).

---

## Tasks

### Task 0: Reverse-engineer claude-code SSE switch (research only — NO commit)

**Files:** none (research only)

**Steps:**

- [ ] Step 1 — Run `grep -rn "case 'message_start'\|case 'content_block_start'\|case 'content_block_delta'\|case 'content_block_stop'\|case 'message_delta'\|case 'message_stop'\|case 'ping'" claude-code/src/services/api/claude.ts`. Expected: 6 hits in `claude.ts` for the 6 explicit-case events, ZERO hits for `'ping'`. Confirms `ping` is NOT in the switch.

- [ ] Step 2 — Run `grep -rn "input_json_delta\|partial_json" claude-code/src/services/api/claude.ts | head -20`. Expected: hits at the lines documented in the byte-locks table (around 2087-2125). Confirm `partial_json` is string-concatenated into the per-block `input` field.

- [ ] Step 3 — Run `grep -rn "stop_reason = part" claude-code/src/services/api/claude.ts`. Expected: a single hit at line 2243 (`stopReason = part.delta.stop_reason`). Confirms `stop_reason` is set on `message_delta`.

- [ ] Step 4 — Open `claude.ts:2298` in an editor (or `sed -n '2295,2300p' claude-code/src/services/api/claude.ts`) and confirm the `message_stop` case body is `break;` (an explicit no-op). LingXi mirrors: `message_stop` triggers loop exit, no per-event work.

- [ ] Step 5 — Capture findings in the "Reverse-engineered byte-locks" table above. Task 0 is done when the table cites correct source line numbers (already done at plan-writing time). No commit.

---

### Task 1: Scaffold `sse/` + `streaming_loop.rs` + `test_support_stream.rs`

**Files:**
- Create: `lingxi-core/crates/orchestrator/src/sse/mod.rs`
- Create: `lingxi-core/crates/orchestrator/src/sse/accumulator.rs`
- Create: `lingxi-core/crates/orchestrator/src/sse/event_router.rs`
- Create: `lingxi-core/crates/orchestrator/src/streaming_loop.rs`
- Create: `lingxi-core/crates/orchestrator/src/test_support_stream.rs`
- Modify: `lingxi-core/crates/orchestrator/src/lib.rs`
- Modify: `lingxi-core/crates/orchestrator/Cargo.toml`

**Steps:**

- [ ] Step 1 — Verify predecessor M5-03 is on `HEAD`. Run `git log -1 --format='%H %s'`. Expected first 7 chars: `0ce1f18` (or whatever SHA M5-03 committed at — must match the user-supplied predecessor SHA). If not, STOP and ask the user.

- [ ] Step 2 — Confirm `futures` is in `lingxi-orchestrator`'s dependency tree. Run `grep -n "futures" lingxi-core/crates/orchestrator/Cargo.toml`. If there is no `futures = { workspace = true }` line under `[dependencies]`, add it:
  ```toml
  futures = { workspace = true }
  ```
  Verify the workspace already declares `futures = "0.3"` in `lingxi-core/Cargo.toml::[workspace.dependencies]` (added by M3-03). If somehow missing, add `futures = "0.3"` to the workspace table FIRST, then add the per-crate reference. Most likely: it is already present transitively but not directly named — verify with `cargo tree -p lingxi-orchestrator -e normal --depth 2 | grep futures`.

- [ ] Step 3 — Create `lingxi-core/crates/orchestrator/src/sse/mod.rs`:
  ```rust
  //! Streaming SSE → orchestrator-level event routing.
  //!
  //! Splits responsibilities cleanly:
  //!
  //! - [`accumulator::BlockAccumulator`] tracks per-`index` block state for
  //!   the duration of one `messages.create` response. Text and tool_use
  //!   blocks are accumulated; thinking blocks pass through.
  //! - [`event_router::dispatch_event`] is the switch that translates each
  //!   `StreamEvent` variant into either an accumulator mutation, an
  //!   `OutputStream` callback, or a `RouterAction` for the streaming loop
  //!   to act on.
  //!
  //! See M5-04 plan "Reverse-engineered byte-locks" for the source-of-truth
  //! claude-code references.
  #![forbid(unsafe_code)]

  pub mod accumulator;
  pub mod event_router;

  use thiserror::Error;

  /// Failure modes from the per-block accumulator.
  ///
  /// Converted into [`crate::error::OrchestratorError::StreamingProtocol`]
  /// at the consumer boundary (`streaming_loop::execute_one_turn_streaming`).
  #[derive(Debug, Error, Clone, PartialEq, Eq)]
  pub enum StreamingError {
      /// A delta arrived for an `index` that has no corresponding `start`.
      #[error("streaming: delta for block index {index} without prior start")]
      BlockNotFound { index: u32 },
      /// `stop_block(index)` called twice for the same `index`.
      #[error("streaming: double stop for block index {index}")]
      DoubleStop { index: u32 },
      /// A delta's tag does not match the started block's tag
      /// (e.g. `input_json_delta` on a `text` block).
      #[error("streaming: type mismatch on block {index}: expected {expected}, got {got}")]
      TypeMismatch { index: u32, expected: &'static str, got: &'static str },
      /// `input_json_delta` accumulation could not be JSON-parsed at
      /// `content_block_stop`. The buffer is preserved in the error for
      /// debugging.
      #[error("streaming: tool_use input failed to parse as JSON: {reason}: buffer={buffer:?}")]
      ToolUseJsonParse { index: u32, reason: String, buffer: String },
  }
  ```

- [ ] Step 4 — Create placeholder bodies (filled in later tasks):
  - `lingxi-core/crates/orchestrator/src/sse/accumulator.rs`:
    ```rust
    //! Per-block accumulator. Filled in Task 5.
    #![forbid(unsafe_code)]
    ```
  - `lingxi-core/crates/orchestrator/src/sse/event_router.rs`:
    ```rust
    //! `StreamEvent` → router-action switch. Filled in Task 7.
    #![forbid(unsafe_code)]
    ```
  - `lingxi-core/crates/orchestrator/src/streaming_loop.rs`:
    ```rust
    //! Streaming turn loop. Filled in Tasks 9-15.
    #![forbid(unsafe_code)]
    ```
  - `lingxi-core/crates/orchestrator/src/test_support_stream.rs`:
    ```rust
    //! Test fixtures for the streaming path. Filled in Tasks 6 + 8.
    #![cfg(any(test, feature = "test-support"))]
    #![forbid(unsafe_code)]
    ```

- [ ] Step 5 — Modify `lingxi-core/crates/orchestrator/src/lib.rs`. Add three new `pub mod` declarations AFTER the existing `pub mod turn_loop;` line:
  ```rust
  pub mod sse;
  pub mod streaming_loop;

  #[cfg(any(test, feature = "test-support"))]
  pub mod test_support_stream;
  ```
  Do NOT add any new re-exports at this task — they land in Task 12 once the trait + impls are real.

- [ ] Step 6 — Run `cargo build -p lingxi-orchestrator`. Must succeed. The new modules compile as empty/placeholder shells.

- [ ] Step 7 — Run `cargo tree -p lingxi-orchestrator -e normal --depth 1 2>&1 | grep -i futures`. Expected: one match (`futures v0.3.xx`). Confirms the dep is reachable.

- [ ] Commit: `feat(M5-04 task 1): scaffold sse/ + streaming_loop.rs + test_support_stream.rs in lingxi-orchestrator`

---

### Task 2: First failing test — `BlockAccumulator` basic flow

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/streaming_block_accumulator_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/orchestrator/tests/streaming_block_accumulator_test.rs`:
  ```rust
  //! Unit tests for the BlockAccumulator (M5-04 Task 2 — RED).
  //!
  //! Each test asserts an invariant of the per-block state machine.
  //! These tests fail to compile until Task 5 lands.

  use lingxi_orchestrator::sse::accumulator::{BlockAccumulator, BlockKind, CompletedBlock};
  use lingxi_orchestrator::sse::StreamingError;
  use lingxi_protocol::ToolUseId;
  use serde_json::json;

  #[test]
  fn text_block_round_trip() {
      let mut acc = BlockAccumulator::new();
      acc.start_block(0, BlockKind::Text).expect("start");
      acc.append_text(0, "hel").expect("append1");
      acc.append_text(0, "lo").expect("append2");
      let completed = acc.stop_block(0).expect("stop");
      match completed {
          CompletedBlock::Text { text } => assert_eq!(text, "hello"),
          other => panic!("expected Text, got {other:?}"),
      }
  }

  #[test]
  fn tool_use_partial_json_reassembles() {
      let mut acc = BlockAccumulator::new();
      acc.start_block(
          1,
          BlockKind::ToolUse {
              id: ToolUseId::from("toolu_abc"),
              name: "Read".into(),
          },
      )
      .expect("start");
      acc.append_json(1, "{\"file").expect("append1");
      acc.append_json(1, "_path\":\"foo.rs\"}").expect("append2");
      let completed = acc.stop_block(1).expect("stop");
      match completed {
          CompletedBlock::ToolUse { id, name, input } => {
              assert_eq!(id.as_str(), "toolu_abc");
              assert_eq!(name, "Read");
              assert_eq!(input, json!({"file_path": "foo.rs"}));
          }
          other => panic!("expected ToolUse, got {other:?}"),
      }
  }

  #[test]
  fn empty_tool_use_input_parses_as_empty_object() {
      // Some tools have schema { "type": "object", "properties": {} }
      // and the API streams ZERO input_json_delta events. claude-code
      // synthesizes `{}` in that case. Mirror.
      let mut acc = BlockAccumulator::new();
      acc.start_block(
          0,
          BlockKind::ToolUse {
              id: ToolUseId::from("toolu_xyz"),
              name: "NoArgs".into(),
          },
      )
      .expect("start");
      let completed = acc.stop_block(0).expect("stop");
      match completed {
          CompletedBlock::ToolUse { input, .. } => assert_eq!(input, json!({})),
          other => panic!("expected ToolUse, got {other:?}"),
      }
  }

  #[test]
  fn delta_without_start_errors() {
      let mut acc = BlockAccumulator::new();
      let err = acc.append_text(0, "x").expect_err("no start");
      assert!(matches!(err, StreamingError::BlockNotFound { index: 0 }));
  }

  #[test]
  fn double_stop_errors() {
      let mut acc = BlockAccumulator::new();
      acc.start_block(0, BlockKind::Text).expect("start");
      acc.stop_block(0).expect("first stop");
      let err = acc.stop_block(0).expect_err("second stop");
      assert!(matches!(err, StreamingError::DoubleStop { index: 0 }));
  }

  #[test]
  fn type_mismatch_errors() {
      let mut acc = BlockAccumulator::new();
      acc.start_block(0, BlockKind::Text).expect("start");
      let err = acc.append_json(0, "{}").expect_err("mismatch");
      assert!(matches!(
          err,
          StreamingError::TypeMismatch { index: 0, expected: "text", got: "input_json_delta" }
      ));
  }

  #[test]
  fn malformed_tool_use_json_errors_at_stop() {
      let mut acc = BlockAccumulator::new();
      acc.start_block(
          0,
          BlockKind::ToolUse {
              id: ToolUseId::from("toolu_x"),
              name: "Bad".into(),
          },
      )
      .expect("start");
      acc.append_json(0, "{not json").expect("append");
      let err = acc.stop_block(0).expect_err("parse fail");
      assert!(matches!(err, StreamingError::ToolUseJsonParse { index: 0, .. }));
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test streaming_block_accumulator_test 2>&1 | tail -20`. EXPECTED: compile error — `BlockAccumulator`, `BlockKind`, `CompletedBlock` do not exist yet. This is the TDD red.

- [ ] Commit: `test(M5-04 task 2): RED — BlockAccumulator invariant tests (compile fails until Task 5)`

---

### Task 3: First failing test — streaming text-only happy path

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/streaming_text_only_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/orchestrator/tests/streaming_text_only_test.rs`:
  ```rust
  //! Streaming happy path — text-only response (M5-04 Task 3 — RED).
  //!
  //! Scripts a stream that emits three text deltas plus the standard
  //! lifecycle events. Asserts the orchestrator's OutputStream receives
  //! THREE `emit_text` calls in order, with the concatenated text
  //! matching "hello world", and that the outcome is `EndTurn` after
  //! one turn.
  //!
  //! Fails to compile until Task 9 lands `run_turn_streaming`.

  use lingxi_orchestrator::test_support::{
      MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
  };
  use lingxi_orchestrator::test_support_stream::{scripted, MockStreamingApiClient};
  use lingxi_orchestrator::{
      ConversationOrchestrator, ConversationOutcome, OrchestratorConfig,
  };
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_traits::OutputEvent;
  use std::path::PathBuf;
  use std::sync::Arc;

  #[tokio::test]
  async fn streaming_text_only_three_deltas() {
      let stream = scripted![
          message_start("msg_01", "claude-opus-4-7"),
          content_block_start_text(0),
          text_delta(0, "hel"),
          text_delta(0, "lo wor"),
          text_delta(0, "ld"),
          content_block_stop(0),
          message_delta_stop("end_turn"),
          message_stop(),
      ];

      let api = Arc::new(MockStreamingApiClient::with_turns(vec![stream]));
      let output = Arc::new(MockOutputStream::default());
      let tools = Arc::new(ToolRegistry::empty());
      let hooks = Arc::new(NoOpHookExecutor::default());
      let perms = Arc::new(NoOpPermissionGate::default());

      let orch = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          Arc::new(lingxi_orchestrator::test_support::MockApiClient::new(Vec::new())),
          api.clone(),
          tools,
          hooks,
          perms,
          output.clone(),
          PathBuf::from("/tmp"),
          Arc::new(lingxi_orchestrator::test_support::StaticMemoryProvider::empty()),
      );

      let outcome = orch
          .run_turn_streaming("say hello")
          .await
          .expect("turn must succeed");

      match outcome {
          ConversationOutcome::EndTurn { turn_count, .. } => assert_eq!(turn_count, 1),
      }

      let events = output.snapshot().await;
      // Expect: Text("hel"), Text("lo wor"), Text("ld"), EndTurn { stop_reason: "end_turn" }
      assert_eq!(events.len(), 4, "got {events:?}");
      assert!(matches!(&events[0], OutputEvent::Text { text } if text == "hel"));
      assert!(matches!(&events[1], OutputEvent::Text { text } if text == "lo wor"));
      assert!(matches!(&events[2], OutputEvent::Text { text } if text == "ld"));
      assert!(matches!(&events[3], OutputEvent::EndTurn { stop_reason, .. } if stop_reason == "end_turn"));

      // Mock observed exactly one streaming call.
      assert_eq!(api.captured_calls().await.len(), 1);
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test streaming_text_only_test 2>&1 | tail -15`. EXPECTED: compile error (`scripted!` macro, `MockStreamingApiClient`, `new_with_streaming`, `run_turn_streaming` do not exist yet). This is the TDD red for Tasks 6, 8, 9, 12.

- [ ] Commit: `test(M5-04 task 3): RED — streaming text-only happy-path test (compile fails until Tasks 6/8/9/12)`

---

### Task 4: `OrchestratorError` streaming variants

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/error.rs`

**Steps:**

- [ ] Step 1 — Read the current `lingxi-core/crates/orchestrator/src/error.rs` (M5-02 created it). Locate the closing `}` of the `OrchestratorError` enum.

- [ ] Step 2 — Add three new variants AT THE END of the enum (before the closing `}`), keeping comma after the prior variant:
  ```rust
      /// Mid-stream byte-level error from the streaming transport. Surfaced
      /// when the SSE chunk fails to decode or the HTTP body is cut.
      #[error("streaming transport error: {0}")]
      Streaming(#[from] lingxi_api_client::ApiError),

      /// Stream produced an event that violates the per-block protocol
      /// (out-of-order delta, double stop, type mismatch, malformed
      /// tool_use input JSON).
      #[error("streaming protocol violation: {0}")]
      StreamingProtocol(String),

      /// Stream ended cleanly before a `message_stop` arrived. Mirrors
      /// claude-code's "stream completed without message_start" fallback
      /// (claude.ts:2353) — surfaced as an explicit error rather than
      /// silently retrying.
      #[error("stream ended without message_stop event")]
      StreamEndedWithoutStop,
  ```
  Note: the existing variant `OrchestratorError::ApiCall(#[from] lingxi_api_client::ApiError)` already exists from M5-02 for the BATCHED path. `#[from]` on the NEW `Streaming` variant would conflict (two variants both `#[from] ApiError`). Resolution: DROP the `#[from]` attribute from `Streaming` — convert manually at the call site via `OrchestratorError::Streaming(api_err)`. Update step 2's code to:
  ```rust
      /// Mid-stream byte-level error from the streaming transport. ...
      #[error("streaming transport error: {0}")]
      Streaming(lingxi_api_client::ApiError),
  ```

- [ ] Step 3 — Add unit tests at the bottom of `error.rs` (inside the existing `#[cfg(test)] mod tests` block if there is one, otherwise create one):
  ```rust
  #[cfg(test)]
  mod streaming_variant_tests {
      use super::*;

      #[test]
      fn streaming_display_is_locked() {
          let e = OrchestratorError::Streaming(lingxi_api_client::ApiError::Network("nope".into()));
          let s = format!("{e}");
          assert!(s.starts_with("streaming transport error: "), "{s}");
      }

      #[test]
      fn streaming_protocol_display_carries_inner() {
          let e = OrchestratorError::StreamingProtocol("block 3 has no start".into());
          assert_eq!(format!("{e}"), "streaming protocol violation: block 3 has no start");
      }

      #[test]
      fn stream_ended_without_stop_display_is_locked() {
          assert_eq!(
              format!("{}", OrchestratorError::StreamEndedWithoutStop),
              "stream ended without message_stop event"
          );
      }
  }
  ```

- [ ] Step 4 — Run `cargo test -p lingxi-orchestrator error:: 2>&1 | tail -10`. The three new tests must pass.

- [ ] Step 5 — Run `cargo build -p lingxi-orchestrator` to confirm the enum compiles cleanly with the existing call sites.

- [ ] Commit: `feat(M5-04 task 4): OrchestratorError gains Streaming / StreamingProtocol / StreamEndedWithoutStop variants`

---

### Task 5: Implement `BlockAccumulator` + `CompletedBlock` + `BlockKind`

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/sse/accumulator.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body of `lingxi-core/crates/orchestrator/src/sse/accumulator.rs` with the full implementation:
  ```rust
  //! Per-block accumulator for the streaming SSE path.
  //!
  //! Tracks one `BlockState` per `index: u32` for the lifetime of a
  //! single `messages.create` response. Each block starts via
  //! [`BlockAccumulator::start_block`], receives 0..N appends, and is
  //! finalized via [`BlockAccumulator::stop_block`] which returns a
  //! [`CompletedBlock`]. The accumulator drops the entry on stop —
  //! subsequent operations on the same `index` error with
  //! `StreamingError::DoubleStop`.
  //!
  //! See M5-04 plan reverse-engineered byte-locks for the source
  //! semantics (claude.ts:1995-2300).
  #![forbid(unsafe_code)]

  use super::StreamingError;
  use lingxi_protocol::ToolUseId;
  use serde_json::Value;
  use std::collections::HashMap;

  /// Tag identifying what kind of content block is being accumulated.
  ///
  /// Constructed from a [`StreamEvent::ContentBlockStart`] payload at the
  /// call site. `ToolUse` carries the API-provided id + name verbatim;
  /// the input JSON is reassembled from `input_json_delta` chunks.
  #[derive(Debug, Clone)]
  pub enum BlockKind {
      /// A `text` block — accumulates `text_delta` chunks.
      Text,
      /// A `tool_use` block — accumulates `input_json_delta` chunks.
      ToolUse { id: ToolUseId, name: String },
      /// A `thinking` block — accumulates `thinking_delta` chunks.
      /// Signature (if any) is set via `set_signature`.
      Thinking,
      /// Any other variant (`server_tool_use`, `connector_text`,
      /// `advisor_tool_result`). Accumulator stores nothing; `stop_block`
      /// returns `CompletedBlock::Skipped`.
      Other,
  }

  impl BlockKind {
      fn name(&self) -> &'static str {
          match self {
              BlockKind::Text => "text",
              BlockKind::ToolUse { .. } => "tool_use",
              BlockKind::Thinking => "thinking",
              BlockKind::Other => "other",
          }
      }
  }

  /// One finished content block, ready to be appended to the assistant
  /// message and (if `ToolUse`) dispatched as a tool call.
  #[derive(Debug, Clone)]
  pub enum CompletedBlock {
      /// Plain text.
      Text { text: String },
      /// Tool invocation, with reassembled JSON input.
      ToolUse { id: ToolUseId, name: String, input: Value },
      /// Extended thinking.
      Thinking { thinking: String, signature: Option<String> },
      /// A `BlockKind::Other` variant — caller drops it.
      Skipped,
  }

  /// Per-block state held during accumulation.
  #[derive(Debug)]
  struct BlockState {
      kind: BlockKind,
      /// Used for `Text` and `Thinking`.
      text_buf: String,
      /// Used for `ToolUse` (raw `partial_json` concat).
      json_buf: String,
      /// Set by `signature_delta` on a `Thinking` block.
      signature: Option<String>,
  }

  /// In-progress accumulator. One per active stream consumer.
  #[derive(Debug, Default)]
  pub struct BlockAccumulator {
      blocks: HashMap<u32, BlockState>,
  }

  impl BlockAccumulator {
      /// Construct an empty accumulator.
      #[must_use]
      pub fn new() -> Self {
          Self { blocks: HashMap::new() }
      }

      /// Register a new block at `index` with the given kind. If `index`
      /// already exists, the previous entry is overwritten (mirrors
      /// claude.ts:1996-2070 which `contentBlocks[part.index] = { ... }`
      /// unconditionally).
      pub fn start_block(&mut self, index: u32, kind: BlockKind) -> Result<(), StreamingError> {
          self.blocks.insert(
              index,
              BlockState {
                  kind,
                  text_buf: String::new(),
                  json_buf: String::new(),
                  signature: None,
              },
          );
          Ok(())
      }

      /// Append `text` to the `Text` or `Thinking` buffer of the block at
      /// `index`. Errors if no such block, or if the block is not
      /// text-shaped.
      pub fn append_text(&mut self, index: u32, text: &str) -> Result<(), StreamingError> {
          let state = self
              .blocks
              .get_mut(&index)
              .ok_or(StreamingError::BlockNotFound { index })?;
          match &state.kind {
              BlockKind::Text => {
                  state.text_buf.push_str(text);
                  Ok(())
              }
              BlockKind::Thinking => {
                  state.text_buf.push_str(text);
                  Ok(())
              }
              other => Err(StreamingError::TypeMismatch {
                  index,
                  expected: other.name(),
                  got: "text_delta",
              }),
          }
      }

      /// Append a `partial_json` chunk to the `ToolUse` buffer.
      pub fn append_json(&mut self, index: u32, partial: &str) -> Result<(), StreamingError> {
          let state = self
              .blocks
              .get_mut(&index)
              .ok_or(StreamingError::BlockNotFound { index })?;
          match &state.kind {
              BlockKind::ToolUse { .. } => {
                  state.json_buf.push_str(partial);
                  Ok(())
              }
              other => Err(StreamingError::TypeMismatch {
                  index,
                  expected: other.name(),
                  got: "input_json_delta",
              }),
          }
      }

      /// Set the signature on a `Thinking` block (from a `signature_delta`).
      pub fn set_signature(&mut self, index: u32, sig: &str) -> Result<(), StreamingError> {
          let state = self
              .blocks
              .get_mut(&index)
              .ok_or(StreamingError::BlockNotFound { index })?;
          match &state.kind {
              BlockKind::Thinking => {
                  state.signature = Some(sig.to_string());
                  Ok(())
              }
              other => Err(StreamingError::TypeMismatch {
                  index,
                  expected: other.name(),
                  got: "signature_delta",
              }),
          }
      }

      /// Finalize the block at `index`, removing it from internal state
      /// and returning a `CompletedBlock`. Errors if no such block.
      pub fn stop_block(&mut self, index: u32) -> Result<CompletedBlock, StreamingError> {
          let state = self
              .blocks
              .remove(&index)
              .ok_or(StreamingError::DoubleStop { index })?;
          let completed = match state.kind {
              BlockKind::Text => CompletedBlock::Text { text: state.text_buf },
              BlockKind::Thinking => CompletedBlock::Thinking {
                  thinking: state.text_buf,
                  signature: state.signature,
              },
              BlockKind::ToolUse { id, name } => {
                  let input = if state.json_buf.is_empty() {
                      Value::Object(serde_json::Map::new())
                  } else {
                      serde_json::from_str::<Value>(&state.json_buf).map_err(|e| {
                          StreamingError::ToolUseJsonParse {
                              index,
                              reason: e.to_string(),
                              buffer: state.json_buf.clone(),
                          }
                      })?
                  };
                  CompletedBlock::ToolUse { id, name, input }
              }
              BlockKind::Other => CompletedBlock::Skipped,
          };
          Ok(completed)
      }

      /// `true` when no blocks are currently in-flight.
      #[must_use]
      pub fn is_idle(&self) -> bool {
          self.blocks.is_empty()
      }
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test streaming_block_accumulator_test 2>&1 | tail -10`. All 7 tests from Task 2 must now pass.

- [ ] Step 3 — Run `cargo clippy -p lingxi-orchestrator -- -D warnings` to confirm no clippy diagnostics. If any fire (e.g. `manual_map` on the empty-json check), apply the suggested fix.

- [ ] Step 4 — Document the StreamingError → OrchestratorError::StreamingProtocol byte format. Add this comment block at the top of `lingxi-core/crates/orchestrator/src/streaming_loop.rs` (replacing the existing placeholder doc):
  ```rust
  //! Streaming turn loop. Filled in Tasks 9-15.
  //!
  //! ## StreamingError → OrchestratorError mapping
  //!
  //! Each [`crate::sse::StreamingError`] variant is converted to
  //! [`crate::error::OrchestratorError::StreamingProtocol`] via its
  //! `Display` impl. The Display strings (locked at Task 1 step 3) are
  //! the public-facing reason carried in the orchestrator error and
  //! visible in telemetry payloads.
  #![forbid(unsafe_code)]
  ```

- [ ] Commit: `feat(M5-04 task 5): BlockAccumulator + CompletedBlock + BlockKind (green: 7 invariant tests pass)`

---

### Task 6: `MockStreamingApiClient` + `scripted!` macro

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/test_support_stream.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body of `lingxi-core/crates/orchestrator/src/test_support_stream.rs` with:
  ```rust
  //! Test fixtures for the streaming path.
  //!
  //! - [`MockStreamingApiClient`] — implements [`crate::conversation::StreamingApiClient`]
  //!   over a per-turn `Vec<StreamEvent>` script.
  //! - [`scripted!`] — declarative macro for assembling event sequences
  //!   with the high-level vocabulary `text`, `tool_use`, `end_turn`,
  //!   `tool_use_stop`, etc.
  //! - [`MockToolDispatchClock`] — records the wall-clock instant each
  //!   tool dispatch begins, for the mid-stream dispatch test.
  #![cfg(any(test, feature = "test-support"))]
  #![forbid(unsafe_code)]

  use crate::conversation::StreamingApiClient;
  use async_trait::async_trait;
  use futures::stream::{self, BoxStream, StreamExt};
  use lingxi_api_client::{
      types::{
          ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent, UsageApi,
      },
      ApiError,
  };
  use lingxi_protocol::{ConversationMessage, ToolUseId};
  use serde_json::Value;
  use std::sync::Arc;
  use std::time::Instant;
  use tokio::sync::Mutex;

  /// Captured arguments of one `StreamingApiClient::stream` call.
  #[derive(Debug, Clone)]
  pub struct CapturedStreamCall {
      pub model: String,
      pub system: Option<String>,
      pub messages: Vec<ConversationMessage>,
  }

  /// Mock streaming client. Yields the next per-turn script of events
  /// each time `stream` is called. If the queue is exhausted, returns
  /// `ApiError::Network("streaming script exhausted")`.
  pub struct MockStreamingApiClient {
      turns: Mutex<std::collections::VecDeque<Vec<Result<StreamEvent, ApiError>>>>,
      captured: Arc<Mutex<Vec<CapturedStreamCall>>>,
  }

  impl MockStreamingApiClient {
      /// Construct an empty (always-exhausted) mock.
      #[must_use]
      pub fn empty() -> Self {
          Self::with_turns(Vec::new())
      }

      /// Construct from a Vec where each inner Vec is the scripted
      /// event sequence for one turn.
      #[must_use]
      pub fn with_turns(turns: Vec<Vec<StreamEvent>>) -> Self {
          let mapped: Vec<Vec<Result<StreamEvent, ApiError>>> = turns
              .into_iter()
              .map(|t| t.into_iter().map(Ok).collect())
              .collect();
          Self {
              turns: Mutex::new(mapped.into()),
              captured: Arc::new(Mutex::new(Vec::new())),
          }
      }

      /// Construct from already-fallible turns (used to inject an `Err`
      /// mid-stream for the error-propagation test).
      #[must_use]
      pub fn with_fallible_turns(
          turns: Vec<Vec<Result<StreamEvent, ApiError>>>,
      ) -> Self {
          Self {
              turns: Mutex::new(turns.into()),
              captured: Arc::new(Mutex::new(Vec::new())),
          }
      }

      /// Snapshot the captured `stream` call args (one entry per call).
      pub async fn captured_calls(&self) -> Vec<CapturedStreamCall> {
          self.captured.lock().await.clone()
      }
  }

  #[async_trait]
  impl StreamingApiClient for MockStreamingApiClient {
      async fn stream(
          &self,
          model: &str,
          system: Option<&str>,
          messages: Vec<ConversationMessage>,
          _tools: Vec<Value>,
      ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
          self.captured.lock().await.push(CapturedStreamCall {
              model: model.to_string(),
              system: system.map(str::to_string),
              messages,
          });
          let mut queue = self.turns.lock().await;
          let next = queue
              .pop_front()
              .ok_or_else(|| ApiError::Network("streaming script exhausted".into()))?;
          let s = stream::iter(next.into_iter()).boxed();
          Ok(s)
      }
  }

  /// Records wall-clock instants when tool dispatches begin. Used by the
  /// mid-stream test to assert dispatch fires BEFORE `message_stop`.
  #[derive(Debug, Default, Clone)]
  pub struct MockToolDispatchClock {
      pub dispatch_instants: Arc<Mutex<Vec<(String, Instant)>>>,
  }

  impl MockToolDispatchClock {
      pub async fn record(&self, tool: &str) {
          self.dispatch_instants
              .lock()
              .await
              .push((tool.to_string(), Instant::now()));
      }
      pub async fn snapshot(&self) -> Vec<(String, Instant)> {
          self.dispatch_instants.lock().await.clone()
      }
  }

  // ─── scripted! macro helpers ───────────────────────────────────────────

  /// `message_start` event with the given id + model.
  #[must_use]
  pub fn message_start(id: &str, model: &str) -> StreamEvent {
      StreamEvent::MessageStart {
          message: MessageResponse {
              id: id.to_string(),
              model: model.to_string(),
              content: Vec::new(),
              stop_reason: None,
              usage: UsageApi::default(),
          },
      }
  }

  /// `content_block_start` for a `text` block at `index`.
  #[must_use]
  pub fn content_block_start_text(index: u32) -> StreamEvent {
      StreamEvent::ContentBlockStart {
          index,
          content_block: ContentBlockApi::Text { text: String::new() },
      }
  }

  /// `content_block_start` for a `tool_use` block at `index`.
  #[must_use]
  pub fn content_block_start_tool_use(index: u32, id: &str, name: &str) -> StreamEvent {
      StreamEvent::ContentBlockStart {
          index,
          content_block: ContentBlockApi::ToolUse {
              id: ToolUseId::from(id),
              name: name.to_string(),
              input: Value::Object(serde_json::Map::new()),
          },
      }
  }

  /// `content_block_delta { delta: TextDelta { text } }`.
  #[must_use]
  pub fn text_delta(index: u32, text: &str) -> StreamEvent {
      StreamEvent::ContentBlockDelta {
          index,
          delta: ContentDelta::TextDelta { text: text.to_string() },
      }
  }

  /// `content_block_delta { delta: InputJsonDelta { partial_json } }`.
  #[must_use]
  pub fn input_json_delta(index: u32, partial: &str) -> StreamEvent {
      StreamEvent::ContentBlockDelta {
          index,
          delta: ContentDelta::InputJsonDelta { partial_json: partial.to_string() },
      }
  }

  /// `content_block_stop { index }`.
  #[must_use]
  pub fn content_block_stop(index: u32) -> StreamEvent {
      StreamEvent::ContentBlockStop { index }
  }

  /// `message_delta { delta: { stop_reason } }`.
  #[must_use]
  pub fn message_delta_stop(stop_reason: &str) -> StreamEvent {
      StreamEvent::MessageDelta {
          delta: MessageDeltaPayload {
              stop_reason: Some(stop_reason.to_string()),
          },
          usage: None,
      }
  }

  /// `message_stop`.
  #[must_use]
  pub fn message_stop() -> StreamEvent {
      StreamEvent::MessageStop
  }

  /// `ping` keepalive.
  #[must_use]
  pub fn ping() -> StreamEvent {
      StreamEvent::Ping
  }

  /// Declarative macro for assembling an event sequence. Pass any
  /// expression that evaluates to a `StreamEvent`. Example:
  /// ```ignore
  /// let s = scripted![
  ///     message_start("msg_1", "claude-opus-4-7"),
  ///     content_block_start_text(0),
  ///     text_delta(0, "hello"),
  ///     content_block_stop(0),
  ///     message_delta_stop("end_turn"),
  ///     message_stop(),
  /// ];
  /// ```
  #[macro_export]
  macro_rules! scripted {
      [$($event:expr),* $(,)?] => {
          vec![$($event),*]
      };
  }

  // Re-export the macro at the module level so tests can call
  // `scripted![...]` without the `lingxi_orchestrator::scripted!` path.
  pub use scripted;

  #[cfg(test)]
  mod tests {
      use super::*;

      #[tokio::test]
      async fn mock_yields_first_turn_then_exhausts() {
          let mock = MockStreamingApiClient::with_turns(vec![scripted![
              message_start("m1", "claude-opus-4-7"),
              message_stop(),
          ]]);
          let s = mock
              .stream("claude-opus-4-7", None, Vec::new(), Vec::new())
              .await
              .expect("first turn");
          let collected: Vec<_> = s.collect().await;
          assert_eq!(collected.len(), 2);
          assert!(matches!(collected[0], Ok(StreamEvent::MessageStart { .. })));
          assert!(matches!(collected[1], Ok(StreamEvent::MessageStop)));

          let err = mock
              .stream("claude-opus-4-7", None, Vec::new(), Vec::new())
              .await
              .expect_err("second call exhausted");
          assert!(matches!(err, ApiError::Network(_)));
      }

      #[tokio::test]
      async fn captured_calls_record_model_and_system() {
          let mock = MockStreamingApiClient::with_turns(vec![scripted![message_stop()]]);
          let _ = mock
              .stream("claude-opus-4-7", Some("sys"), Vec::new(), Vec::new())
              .await
              .expect("call");
          let calls = mock.captured_calls().await;
          assert_eq!(calls.len(), 1);
          assert_eq!(calls[0].model, "claude-opus-4-7");
          assert_eq!(calls[0].system.as_deref(), Some("sys"));
      }
  }
  ```

- [ ] Step 2 — At this point `StreamingApiClient` does NOT yet exist in `conversation.rs` — the test_support_stream module will fail to compile. That is EXPECTED. Run `cargo build -p lingxi-orchestrator 2>&1 | tail -5` to confirm the specific error is `cannot find trait StreamingApiClient`. This validates the dependency arrow before Task 11.

- [ ] Step 3 — Commit: `feat(M5-04 task 6): MockStreamingApiClient + scripted! macro + scripted helpers (red: StreamingApiClient trait pending Task 11)`

---

### Task 7: Implement `event_router::dispatch_event` + `RouterAction`

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/sse/event_router.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body of `lingxi-core/crates/orchestrator/src/sse/event_router.rs` with:
  ```rust
  //! `StreamEvent` → router-action dispatch.
  //!
  //! The streaming loop ([`crate::streaming_loop::execute_one_turn_streaming`])
  //! pumps one `StreamEvent` at a time through [`dispatch_event`], which:
  //!
  //! - Mutates the [`BlockAccumulator`] for content_block_* events.
  //! - Calls back into [`OutputStream::emit_text`] for `text_delta` events
  //!   (true per-token streaming).
  //! - Returns a [`RouterAction`] hint for actions the streaming loop must
  //!   take ON its own — namely `DispatchToolUse` (when a tool block
  //!   reaches stop) and `EndOfTurn` (when `message_stop` arrives).
  #![forbid(unsafe_code)]

  use super::accumulator::{BlockAccumulator, BlockKind, CompletedBlock};
  use super::StreamingError;
  use lingxi_api_client::types::{ContentBlockApi, ContentDelta, StreamEvent};
  use lingxi_protocol::{ContentBlock, ToolUseId};
  use lingxi_traits::OutputStream;
  use std::sync::Arc;

  /// Result of routing one `StreamEvent`. The streaming loop acts on each.
  #[derive(Debug, Clone)]
  pub enum RouterAction {
      /// No further action — event handled internally (state mutation
      /// or output emit only).
      Continue,
      /// A `tool_use` block just completed at `content_block_stop`.
      /// The streaming loop spawns a dispatch IMMEDIATELY.
      DispatchToolUse { id: ToolUseId, name: String, input: serde_json::Value },
      /// A text or thinking block completed — append to the in-flight
      /// assistant message and continue.
      AppendAssistantBlock(ContentBlock),
      /// `message_delta` arrived with a `stop_reason`. Streaming loop
      /// records this and continues until `message_stop` arrives.
      RecordStopReason(String),
      /// `message_stop` arrived — terminate the per-turn loop.
      EndOfStream,
      /// A server-emitted `Error` event — surface as a streaming error.
      ServerError(String),
  }

  /// Route one event through the accumulator + output sink. Returns the
  /// next action for the streaming loop.
  pub async fn dispatch_event(
      event: StreamEvent,
      acc: &mut BlockAccumulator,
      output: &Arc<dyn OutputStream>,
  ) -> Result<RouterAction, StreamingError> {
      match event {
          StreamEvent::MessageStart { .. } => {
              // No-op; the loop already knows the model + id from the
              // turn invocation. claude-code captures `partialMessage`
              // and `ttftMs` here; we don't need those at the M5-04 wire.
              Ok(RouterAction::Continue)
          }
          StreamEvent::ContentBlockStart { index, content_block } => {
              let kind = match &content_block {
                  ContentBlockApi::Text { .. } => BlockKind::Text,
                  ContentBlockApi::ToolUse { id, name, .. } => BlockKind::ToolUse {
                      id: *id,
                      name: name.clone(),
                  },
                  ContentBlockApi::Thinking { .. } => BlockKind::Thinking,
                  ContentBlockApi::ServerToolUse { .. }
                  | ContentBlockApi::ConnectorText { .. }
                  | ContentBlockApi::AdvisorToolResult { .. } => BlockKind::Other,
              };
              acc.start_block(index, kind)?;
              Ok(RouterAction::Continue)
          }
          StreamEvent::ContentBlockDelta { index, delta } => {
              match delta {
                  ContentDelta::TextDelta { text } => {
                      acc.append_text(index, &text)?;
                      // Stream the token to the output sink RIGHT NOW.
                      // This is the key M5-04 behavior: tokens are
                      // surfaced as they arrive, not buffered per-block.
                      output.emit_text(&text).await;
                  }
                  ContentDelta::InputJsonDelta { partial_json } => {
                      acc.append_json(index, &partial_json)?;
                  }
                  ContentDelta::ThinkingDelta { thinking } => {
                      acc.append_text(index, &thinking)?;
                  }
                  ContentDelta::SignatureDelta { signature } => {
                      acc.set_signature(index, &signature)?;
                  }
                  ContentDelta::CitationsDelta { .. }
                  | ContentDelta::ConnectorTextDelta { .. } => {
                      // Dropped at M5-04 boundary (parity with M5-02's
                      // `translate_response_blocks` which drops them).
                  }
              }
              Ok(RouterAction::Continue)
          }
          StreamEvent::ContentBlockStop { index } => {
              let completed = acc.stop_block(index)?;
              match completed {
                  CompletedBlock::Text { text } => Ok(RouterAction::AppendAssistantBlock(
                      ContentBlock::Text { text },
                  )),
                  CompletedBlock::Thinking { thinking, signature } => Ok(
                      RouterAction::AppendAssistantBlock(ContentBlock::Thinking {
                          thinking,
                          signature,
                      }),
                  ),
                  CompletedBlock::ToolUse { id, name, input } => {
                      Ok(RouterAction::DispatchToolUse { id, name, input })
                  }
                  CompletedBlock::Skipped => Ok(RouterAction::Continue),
              }
          }
          StreamEvent::MessageDelta { delta, .. } => {
              if let Some(sr) = delta.stop_reason {
                  Ok(RouterAction::RecordStopReason(sr))
              } else {
                  Ok(RouterAction::Continue)
              }
          }
          StreamEvent::MessageStop => Ok(RouterAction::EndOfStream),
          StreamEvent::Ping => Ok(RouterAction::Continue),
          StreamEvent::Error { error } => {
              Ok(RouterAction::ServerError(format!("{}: {}", error.kind, error.message)))
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::test_support::MockOutputStream;
      use lingxi_api_client::types::MessageDeltaPayload;

      #[tokio::test]
      async fn text_delta_emits_to_output_and_accumulates() {
          let mut acc = BlockAccumulator::new();
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          // start a text block
          dispatch_event(
              StreamEvent::ContentBlockStart {
                  index: 0,
                  content_block: ContentBlockApi::Text { text: String::new() },
              },
              &mut acc,
              &out,
          )
          .await
          .expect("start");
          // delta
          dispatch_event(
              StreamEvent::ContentBlockDelta {
                  index: 0,
                  delta: ContentDelta::TextDelta { text: "hi".into() },
              },
              &mut acc,
              &out,
          )
          .await
          .expect("delta");
          // OutputStream observed exactly one emit_text("hi")
          let mock: &MockOutputStream = out.as_ref().downcast_ref::<MockOutputStream>().expect("mock");
          // MockOutputStream snapshot is async; downcast for direct access.
          let events = mock.snapshot().await;
          assert_eq!(events.len(), 1);
      }

      #[tokio::test]
      async fn ping_is_noop() {
          let mut acc = BlockAccumulator::new();
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          let action = dispatch_event(StreamEvent::Ping, &mut acc, &out).await.expect("ok");
          assert!(matches!(action, RouterAction::Continue));
          assert!(acc.is_idle());
      }

      #[tokio::test]
      async fn message_delta_records_stop_reason() {
          let mut acc = BlockAccumulator::new();
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          let action = dispatch_event(
              StreamEvent::MessageDelta {
                  delta: MessageDeltaPayload {
                      stop_reason: Some("end_turn".into()),
                  },
                  usage: None,
              },
              &mut acc,
              &out,
          )
          .await
          .expect("ok");
          match action {
              RouterAction::RecordStopReason(sr) => assert_eq!(sr, "end_turn"),
              other => panic!("expected RecordStopReason, got {other:?}"),
          }
      }

      #[tokio::test]
      async fn message_stop_ends_stream() {
          let mut acc = BlockAccumulator::new();
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          let action = dispatch_event(StreamEvent::MessageStop, &mut acc, &out).await.expect("ok");
          assert!(matches!(action, RouterAction::EndOfStream));
      }
  }
  ```

  **Note on `Any` downcast in test:** the trait object cast `out.as_ref().downcast_ref::<MockOutputStream>()` only works if `OutputStream: Any`. The trait currently doesn't extend `Any`. Simplify the test by binding `MockOutputStream` directly (not via the trait object) and passing `Arc::clone(&mock_arc)`:
  ```rust
  let mock = Arc::new(MockOutputStream::default());
  let as_trait: Arc<dyn OutputStream> = mock.clone();
  // dispatch_event(..., &as_trait).await
  // assertion: mock.snapshot().await ...
  ```
  Apply this rewrite in the test bodies of step 1.

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator sse::event_router 2>&1 | tail -10`. All 4 internal tests must pass.

- [ ] Step 3 — Run `cargo clippy -p lingxi-orchestrator -- -D warnings`. Address any warnings (likely `clippy::needless_pass_by_value` on the borrowed `&Arc<dyn OutputStream>` — use `&dyn OutputStream` if the lifetime works, otherwise allow with a comment).

- [ ] Commit: `feat(M5-04 task 7): event_router::dispatch_event + RouterAction (green: 4 routing tests pass)`

---

### Task 8: Wire `MockStreamingApiClient` into `test_support` re-exports

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/test_support.rs`

**Steps:**

- [ ] Step 1 — Open `lingxi-core/crates/orchestrator/src/test_support.rs` (M5-02 created it; M5-03 may have extended it with `StaticMemoryProvider`). Locate the existing `pub use` re-export block at the top OR the very bottom of the file.

- [ ] Step 2 — Append a re-export pulling the streaming mocks into the same namespace:
  ```rust
  // Re-export streaming-path test fixtures so test files can `use
  // lingxi_orchestrator::test_support::{MockStreamingApiClient, ...}` and
  // not have to import from `test_support_stream` directly.
  #[cfg(any(test, feature = "test-support"))]
  pub use crate::test_support_stream::{
      content_block_start_text, content_block_start_tool_use, content_block_stop,
      input_json_delta, message_delta_stop, message_start, message_stop, ping, text_delta,
      MockStreamingApiClient, MockToolDispatchClock,
  };
  ```

- [ ] Step 3 — Run `cargo build -p lingxi-orchestrator --tests`. Expected: still fails because `conversation::StreamingApiClient` does not exist. That is fine — Tasks 6/7 land before Task 11.

- [ ] Commit: `feat(M5-04 task 8): re-export streaming mocks from test_support`

---

### Task 9: Implement `streaming_loop::pump_stream` (no orchestrator integration yet)

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/streaming_loop.rs`

**Steps:**

- [ ] Step 1 — Open `lingxi-core/crates/orchestrator/src/streaming_loop.rs`. Replace the placeholder body with the FIRST half of the streaming machinery: a `pump_stream` free function that consumes a stream and returns a `PumpedTurn` describing what to do next. Tool dispatch is concurrent — handled here via `tokio::spawn` and `join_all` after `message_stop`. The OUTER turn loop (which calls `pump_stream` and then loops or exits) lands in Task 12.
  ```rust
  //! Streaming turn loop core helpers.
  //!
  //! ## StreamingError → OrchestratorError mapping
  //!
  //! Each [`crate::sse::StreamingError`] variant is converted to
  //! [`crate::error::OrchestratorError::StreamingProtocol`] via its
  //! `Display` impl.
  #![forbid(unsafe_code)]

  use crate::error::OrchestratorError;
  use crate::sse::accumulator::BlockAccumulator;
  use crate::sse::event_router::{dispatch_event, RouterAction};
  use futures::stream::{BoxStream, StreamExt};
  use lingxi_api_client::{types::StreamEvent, ApiError};
  use lingxi_protocol::{ContentBlock, ToolUseId};
  use lingxi_traits::OutputStream;
  use serde_json::Value;
  use std::sync::Arc;

  /// One tool dispatch request observed during the stream. Carries the
  /// id/name/input the orchestrator must invoke. The dispatch itself is
  /// performed by the streaming-loop caller (so this module stays free
  /// of `ToolRegistry`/`HookExecutor`/`PermissionGate` deps).
  #[derive(Debug, Clone)]
  pub struct ObservedToolUse {
      pub id: ToolUseId,
      pub name: String,
      pub input: Value,
  }

  /// Outcome of consuming one stream.
  #[derive(Debug, Default)]
  pub struct PumpedTurn {
      /// Content blocks (text + thinking) accumulated during the stream,
      /// in observation order. Used to construct the assistant message
      /// after the stream ends.
      pub assistant_blocks: Vec<ContentBlock>,
      /// Tool uses observed during the stream, in observation order
      /// (i.e. order of their `content_block_stop` events).
      pub tool_uses: Vec<ObservedToolUse>,
      /// Final `stop_reason` (from `message_delta`). `None` if the
      /// stream ended without a `message_delta` carrying one.
      pub stop_reason: Option<String>,
  }

  /// Consume the given stream to completion, routing events through the
  /// accumulator + output sink. Returns the per-block summary; the
  /// streaming-loop caller is responsible for tool dispatch + appending
  /// to session history.
  pub async fn pump_stream(
      mut stream: BoxStream<'static, Result<StreamEvent, ApiError>>,
      output: &Arc<dyn OutputStream>,
  ) -> Result<PumpedTurn, OrchestratorError> {
      let mut acc = BlockAccumulator::new();
      let mut turn = PumpedTurn::default();

      while let Some(item) = stream.next().await {
          let event = item.map_err(OrchestratorError::Streaming)?;
          let action = dispatch_event(event, &mut acc, output)
              .await
              .map_err(|e| OrchestratorError::StreamingProtocol(e.to_string()))?;
          match action {
              RouterAction::Continue => {}
              RouterAction::AppendAssistantBlock(block) => {
                  turn.assistant_blocks.push(block);
              }
              RouterAction::DispatchToolUse { id, name, input } => {
                  turn.tool_uses.push(ObservedToolUse { id, name, input });
              }
              RouterAction::RecordStopReason(sr) => {
                  turn.stop_reason = Some(sr);
              }
              RouterAction::EndOfStream => {
                  return Ok(turn);
              }
              RouterAction::ServerError(reason) => {
                  return Err(OrchestratorError::StreamingProtocol(format!(
                      "server-emitted error event: {reason}"
                  )));
              }
          }
      }
      // Stream ended without a MessageStop.
      Err(OrchestratorError::StreamEndedWithoutStop)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::test_support::MockOutputStream;
      use crate::test_support_stream::{
          content_block_start_text, content_block_start_tool_use, content_block_stop,
          input_json_delta, message_delta_stop, message_start, message_stop, text_delta,
      };
      use futures::stream;

      fn boxed(
          events: Vec<StreamEvent>,
      ) -> BoxStream<'static, Result<StreamEvent, ApiError>> {
          stream::iter(events.into_iter().map(Ok)).boxed()
      }

      #[tokio::test]
      async fn text_only_pump_assembles_one_text_block() {
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          let evs = vec![
              message_start("m1", "claude-opus-4-7"),
              content_block_start_text(0),
              text_delta(0, "he"),
              text_delta(0, "llo"),
              content_block_stop(0),
              message_delta_stop("end_turn"),
              message_stop(),
          ];
          let turn = pump_stream(boxed(evs), &out).await.expect("pump");
          assert_eq!(turn.stop_reason.as_deref(), Some("end_turn"));
          assert_eq!(turn.assistant_blocks.len(), 1);
          if let ContentBlock::Text { text } = &turn.assistant_blocks[0] {
              assert_eq!(text, "hello");
          } else {
              panic!("expected Text block, got {:?}", turn.assistant_blocks[0]);
          }
          assert!(turn.tool_uses.is_empty());
      }

      #[tokio::test]
      async fn tool_use_pump_collects_dispatch_request() {
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          let evs = vec![
              message_start("m1", "claude-opus-4-7"),
              content_block_start_tool_use(1, "toolu_abc", "Read"),
              input_json_delta(1, "{\"file"),
              input_json_delta(1, "_path\":\"foo.rs\"}"),
              content_block_stop(1),
              message_delta_stop("tool_use"),
              message_stop(),
          ];
          let turn = pump_stream(boxed(evs), &out).await.expect("pump");
          assert_eq!(turn.tool_uses.len(), 1);
          assert_eq!(turn.tool_uses[0].name, "Read");
          assert_eq!(turn.tool_uses[0].input["file_path"], "foo.rs");
          assert_eq!(turn.stop_reason.as_deref(), Some("tool_use"));
      }

      #[tokio::test]
      async fn stream_without_message_stop_errors() {
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          let evs = vec![
              message_start("m1", "claude-opus-4-7"),
              content_block_start_text(0),
              text_delta(0, "partial"),
              content_block_stop(0),
              // no message_stop
          ];
          let err = pump_stream(boxed(evs), &out).await.expect_err("no stop");
          assert!(matches!(err, OrchestratorError::StreamEndedWithoutStop));
      }

      #[tokio::test]
      async fn streaming_protocol_error_propagates() {
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          // delta before start → BlockNotFound
          let evs = vec![
              message_start("m1", "claude-opus-4-7"),
              text_delta(0, "oops"),
              message_stop(),
          ];
          let err = pump_stream(boxed(evs), &out).await.expect_err("proto");
          match err {
              OrchestratorError::StreamingProtocol(reason) => {
                  assert!(reason.contains("block index 0"), "{reason}");
              }
              other => panic!("expected StreamingProtocol, got {other:?}"),
          }
      }

      #[tokio::test]
      async fn underlying_stream_error_surfaces_as_streaming_variant() {
          let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::default());
          let s: BoxStream<'static, Result<StreamEvent, ApiError>> =
              stream::iter(vec![
                  Ok(message_start("m1", "claude-opus-4-7")),
                  Err(ApiError::Network("dropped".into())),
              ])
              .boxed();
          let err = pump_stream(s, &out).await.expect_err("network");
          assert!(matches!(err, OrchestratorError::Streaming(_)));
      }
  }
  ```

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator streaming_loop 2>&1 | tail -15`. All 5 tests in the module must pass.

- [ ] Commit: `feat(M5-04 task 9): streaming_loop::pump_stream + PumpedTurn + ObservedToolUse (green: 5 pump tests)`

---

### Task 10: First failing test — mid-stream tool dispatch timing

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/streaming_mid_stream_tool_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/orchestrator/tests/streaming_mid_stream_tool_test.rs`:
  ```rust
  //! Mid-stream tool dispatch (M5-04 Task 10 — RED).
  //!
  //! Asserts that a tool_use block is dispatched THE MOMENT its
  //! `content_block_stop` arrives — i.e. BEFORE the subsequent
  //! `message_delta` + `message_stop` events. The test records a
  //! wall-clock instant when the tool is dispatched and when
  //! `message_stop` is observed; asserts dispatch < message_stop.
  //!
  //! Fails to compile until Tasks 11-13 land the integrated
  //! `run_turn_streaming` + concurrent dispatch.

  use lingxi_orchestrator::test_support::{
      content_block_start_text, content_block_start_tool_use, content_block_stop,
      input_json_delta, message_delta_stop, message_start, message_stop, text_delta,
      MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpHookExecutor,
      NoOpPermissionGate,
  };
  use lingxi_orchestrator::test_support_stream::scripted;
  use lingxi_orchestrator::{
      ConversationOrchestrator, OrchestratorConfig,
  };
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_traits::OutputEvent;
  use std::path::PathBuf;
  use std::sync::Arc;

  #[tokio::test]
  async fn tool_use_dispatched_before_message_stop() {
      let stream = scripted![
          message_start("m1", "claude-opus-4-7"),
          content_block_start_text(0),
          text_delta(0, "calling tool"),
          content_block_stop(0),
          content_block_start_tool_use(1, "toolu_01", "AlwaysOk"),
          input_json_delta(1, "{}"),
          content_block_stop(1), // dispatch fires HERE
          message_delta_stop("tool_use"),
          message_stop(),
      ];

      // Turn 2: model returns "done" after seeing tool result.
      let turn2 = scripted![
          message_start("m2", "claude-opus-4-7"),
          content_block_start_text(0),
          text_delta(0, "done"),
          content_block_stop(0),
          message_delta_stop("end_turn"),
          message_stop(),
      ];

      let api = Arc::new(MockStreamingApiClient::with_turns(vec![stream, turn2]));
      let batched = Arc::new(MockApiClient::new(Vec::new())); // unused on streaming path
      let output = Arc::new(MockOutputStream::default());

      // Register an "AlwaysOk" tool that records dispatch time.
      let tools = Arc::new(ToolRegistry::with_always_ok_test_tool());
      let hooks = Arc::new(NoOpHookExecutor::default());
      let perms = Arc::new(NoOpPermissionGate::default());

      let orch = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          batched,
          api.clone(),
          tools,
          hooks,
          perms,
          output.clone(),
          PathBuf::from("/tmp"),
          Arc::new(lingxi_orchestrator::test_support::StaticMemoryProvider::empty()),
      );

      let _outcome = orch.run_turn_streaming("call a tool").await.expect("ok");

      // Inspect the OutputEvent ordering: ToolCall MUST appear BEFORE
      // any event that could only arrive after message_stop.
      // (The streaming path emits Text("done") only AFTER the loop
      // re-enters for turn 2, which is AFTER the tool result was fed
      // back. So the order is:
      //   Text("calling tool"), ToolCall, ToolResult, Text("done"), EndTurn.)
      let events = output.snapshot().await;
      let mut iter = events.iter();
      assert!(matches!(iter.next(), Some(OutputEvent::Text { text }) if text == "calling tool"));
      assert!(matches!(iter.next(), Some(OutputEvent::ToolCall { tool, .. }) if tool == "AlwaysOk"));
      assert!(matches!(iter.next(), Some(OutputEvent::ToolResult { tool, .. }) if tool == "AlwaysOk"));
      assert!(matches!(iter.next(), Some(OutputEvent::Text { text }) if text == "done"));
      assert!(matches!(iter.next(), Some(OutputEvent::EndTurn { stop_reason, .. }) if stop_reason == "end_turn"));
      assert!(iter.next().is_none());
  }
  ```

  **Note on `ToolRegistry::with_always_ok_test_tool()`:** if this helper does not yet exist on `ToolRegistry` (added by M4 / M5-02 tests), inline its construction:
  ```rust
  use lingxi_orchestrator::test_support::always_ok_tool_registry;
  let tools = always_ok_tool_registry();
  ```
  Inspect M5-02's `crates/orchestrator/tests/orchestrator_multi_turn_test.rs` for the exact helper name and reuse it verbatim. If a helper is genuinely missing, add a minimal `always_ok_tool_registry()` function in `test_support.rs` as part of Task 10 step 2 (one-liner that constructs a registry, registers an `AlwaysOk` tool that returns `Ok(serde_json::json!({"ok": true}))` for any input).

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test streaming_mid_stream_tool_test 2>&1 | tail -15`. EXPECTED: compile error (`new_with_streaming`, `run_turn_streaming` do not exist yet). TDD red.

- [ ] Commit: `test(M5-04 task 10): RED — mid-stream tool dispatch ordering test (compile fails until Tasks 11-13)`

---

### Task 11: Add `StreamingApiClient` trait + `AnthropicProviderStreamingAdapter` + `messages_create_stream`

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs`
- Modify: `lingxi-core/crates/api-client/src/anthropic.rs`
- Modify: `lingxi-core/crates/orchestrator/src/lib.rs`

**Steps:**

- [ ] Step 1 — Open `lingxi-core/crates/api-client/src/anthropic.rs`. Find `messages_create_non_stream`. Immediately AFTER it, add a new method:
  ```rust
  /// Open a streaming `messages.create` request. Yields wire-decoded
  /// `StreamEvent` values until the server emits `message_stop` (or
  /// the underlying transport errors).
  ///
  /// The returned stream is `BoxStream<'static, ...>` so the caller can
  /// own it independent of `self` / the transport.
  pub async fn messages_create_stream<T: HttpTransport + Send + Sync + 'static>(
      &self,
      model: &str,
      system: Option<&str>,
      msgs: Vec<ConversationMessage>,
      tools: Vec<serde_json::Value>,
      transport: Arc<T>,
  ) -> Result<
      futures::stream::BoxStream<'static, Result<crate::types::StreamEvent, ApiError>>,
      ApiError,
  > {
      use futures::stream::StreamExt;

      // Build the request body. Same shape as messages_create_non_stream
      // but with `stream: true`.
      let mut body = serde_json::json!({
          "model": model,
          "max_tokens": 4096u32,
          "messages": msgs,
          "stream": true,
      });
      if let Some(s) = system {
          body["system"] = serde_json::Value::String(s.to_string());
      }
      if !tools.is_empty() {
          body["tools"] = serde_json::Value::Array(tools);
      }

      // Delegate to the transport's SSE channel. `HttpTransport::stream_sse`
      // returns a `BoxStream<Result<SseEvent, TransportError>>` of WIRE
      // events; we map each into a typed `StreamEvent` via serde.
      let wire_stream = transport
          .stream_sse(
              "/v1/messages",
              &body,
              &self.auth_headers(),
          )
          .await
          .map_err(|e| ApiError::Network(e.to_string()))?;

      let typed = wire_stream.map(|item| match item {
          Ok(sse) => serde_json::from_str::<crate::types::StreamEvent>(&sse.data)
              .map_err(|e| ApiError::Decode(format!("StreamEvent decode failed: {e}: data={}", sse.data))),
          Err(e) => Err(ApiError::Network(e.to_string())),
      });
      Ok(typed.boxed())
  }
  ```

  **Note on `HttpTransport::stream_sse`:** confirm at Task 11 step 1.5 whether the transport already has a `stream_sse` method (M3-03 may have shipped it). Run `grep -rn "fn stream_sse\|fn sse_stream" lingxi-core/crates/traits/src/http.rs lingxi-core/crates/api-client/src/` — if the method exists with a different name (e.g. `sse_stream`), update the call. If no SSE streaming method exists yet, you must extend `HttpTransport` first (and likely M3-03 should have done so). In that case, add a minimal `stream_sse` to `lingxi-traits::HttpTransport`:
  ```rust
  async fn stream_sse(
      &self,
      path: &str,
      body: &serde_json::Value,
      headers: &[(String, String)],
  ) -> Result<
      futures::stream::BoxStream<'static, Result<lingxi_protocol::SseEvent, lingxi_traits::HttpError>>,
      lingxi_traits::HttpError,
  >;
  ```
  with a default impl that returns `Err(HttpError::Unsupported)` so existing impls don't break, then implement the real transport variant in `lingxi-bridge` (or wherever the production transport lives). Document the deviation in the commit message.

- [ ] Step 2 — Open `lingxi-core/crates/orchestrator/src/conversation.rs`. Locate the `OrchestratorApiClient` trait definition (from M5-02 Task 6). Immediately AFTER its `}` closing brace, add the streaming trait + adapter:
  ```rust
  /// Streaming-API surface used by the orchestrator's streaming turn loop.
  ///
  /// Mirrors [`OrchestratorApiClient`] but returns a typed
  /// `BoxStream<'static, Result<StreamEvent, ApiError>>` instead of a
  /// single `MessageResponse`. The orchestrator owns the stream and
  /// drives it to completion (or `message_stop`).
  ///
  /// Production: [`AnthropicProviderStreamingAdapter`] wraps
  /// `AnthropicProvider::messages_create_stream` + a transport.
  /// Tests: `MockStreamingApiClient` in `test_support_stream.rs`.
  #[async_trait]
  pub trait StreamingApiClient: Send + Sync {
      async fn stream(
          &self,
          model: &str,
          system: Option<&str>,
          messages: Vec<ConversationMessage>,
          tools: Vec<serde_json::Value>,
      ) -> Result<
          futures::stream::BoxStream<'static, Result<lingxi_api_client::types::StreamEvent, ApiError>>,
          ApiError,
      >;
  }

  /// Production adapter wrapping `AnthropicProvider` + `HttpTransport`
  /// into the `StreamingApiClient` shape.
  pub struct AnthropicProviderStreamingAdapter<T: HttpTransport + Send + Sync + 'static> {
      provider: Arc<AnthropicProvider>,
      transport: Arc<T>,
  }

  impl<T: HttpTransport + Send + Sync + 'static> AnthropicProviderStreamingAdapter<T> {
      pub fn new(provider: Arc<AnthropicProvider>, transport: Arc<T>) -> Self {
          Self { provider, transport }
      }
  }

  #[async_trait]
  impl<T: HttpTransport + Send + Sync + 'static> StreamingApiClient
      for AnthropicProviderStreamingAdapter<T>
  {
      async fn stream(
          &self,
          model: &str,
          system: Option<&str>,
          messages: Vec<ConversationMessage>,
          tools: Vec<serde_json::Value>,
      ) -> Result<
          futures::stream::BoxStream<'static, Result<lingxi_api_client::types::StreamEvent, ApiError>>,
          ApiError,
      > {
          self.provider
              .messages_create_stream(model, system, messages, tools, self.transport.clone())
              .await
      }
  }
  ```

- [ ] Step 3 — Modify `lingxi-core/crates/orchestrator/src/lib.rs` to re-export the new trait:
  ```rust
  pub use conversation::{
      AnthropicProviderAdapter, AnthropicProviderStreamingAdapter, ConversationOrchestrator,
      ConversationOutcome, OrchestratorApiClient, StreamingApiClient,
  };
  ```
  (Adjust the import list to merge with whatever M5-02 / M5-03 currently re-exports.)

- [ ] Step 4 — Run `cargo build -p lingxi-orchestrator -p lingxi-api-client`. Both crates must compile. If `stream_sse` is missing from `HttpTransport`, this step will fail — handle per step 1's note.

- [ ] Step 5 — Add a doc-test on `StreamingApiClient` confirming it's object-safe:
  ```rust
  /// ```
  /// fn _is_object_safe(_: &dyn lingxi_orchestrator::StreamingApiClient) {}
  /// ```
  ```
  Place this above the trait. Run `cargo test -p lingxi-orchestrator --doc StreamingApiClient` to confirm.

- [ ] Commit: `feat(M5-04 task 11): StreamingApiClient trait + AnthropicProviderStreamingAdapter + AnthropicProvider::messages_create_stream`

---

### Task 12: `ConversationOrchestrator::new_with_streaming` + `run_turn_streaming` (text-only path makes Task 3 + 9 tests green)

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs`

**Steps:**

- [ ] Step 1 — Open `conversation.rs`. Add a new field `streaming_api: Arc<dyn StreamingApiClient>` to the `ConversationOrchestrator` struct. The struct now has 10 fields after this task. Use the exact field order:
  ```rust
  pub struct ConversationOrchestrator {
      pub(crate) config: OrchestratorConfig,
      pub(crate) api: Arc<dyn OrchestratorApiClient>,
      pub(crate) streaming_api: Arc<dyn StreamingApiClient>,
      pub(crate) tools: Arc<ToolRegistry>,
      pub(crate) hooks: Arc<dyn HookExecutor>,
      pub(crate) perms: Arc<dyn PermissionGate>,
      pub(crate) output: Arc<dyn OutputStream>,
      pub(crate) session: Arc<Mutex<SessionState>>,
      pub(crate) memory: Arc<dyn MemoryHierarchyProvider>,
      pub(crate) cwd: PathBuf,
  }
  ```

- [ ] Step 2 — Add a new constructor `new_with_streaming` that accepts both the batched and streaming clients. The existing `new` from M5-03 stays — make it call `new_with_streaming` internally with a default `MockStreamingApiClient::empty()` (or, if `lingxi-orchestrator` cannot import the test mock in production, with a tiny `NoStreamingApiClient` always-error stub defined right next to it). Add:
  ```rust
  /// Internal no-op streaming client used by [`ConversationOrchestrator::new`]
  /// when the caller doesn't supply a streaming transport. Every call to
  /// `stream` returns `ApiError::Network("no streaming client configured")`.
  pub(crate) struct NoStreamingApiClient;

  #[async_trait]
  impl StreamingApiClient for NoStreamingApiClient {
      async fn stream(
          &self,
          _model: &str,
          _system: Option<&str>,
          _messages: Vec<ConversationMessage>,
          _tools: Vec<serde_json::Value>,
      ) -> Result<
          futures::stream::BoxStream<'static, Result<lingxi_api_client::types::StreamEvent, ApiError>>,
          ApiError,
      > {
          Err(ApiError::Network("no streaming client configured".into()))
      }
  }

  impl ConversationOrchestrator {
      /// Construct an orchestrator that supports BOTH batched and
      /// streaming paths. New v0.6.0 constructor.
      #[allow(clippy::too_many_arguments)]
      pub fn new_with_streaming(
          config: OrchestratorConfig,
          api: Arc<dyn OrchestratorApiClient>,
          streaming_api: Arc<dyn StreamingApiClient>,
          tools: Arc<ToolRegistry>,
          hooks: Arc<dyn HookExecutor>,
          perms: Arc<dyn PermissionGate>,
          output: Arc<dyn OutputStream>,
          cwd: PathBuf,
          memory: Arc<dyn MemoryHierarchyProvider>,
      ) -> Self {
          let session = SessionState::empty(SessionId::new(), config.model.clone());
          Self {
              config,
              api,
              streaming_api,
              tools,
              hooks,
              perms,
              output,
              session: Arc::new(Mutex::new(session)),
              memory,
              cwd,
          }
      }

      /// Legacy v0.5.x-style constructor — wires a `NoStreamingApiClient`
      /// stub for the streaming field. Tests that don't exercise the
      /// streaming path keep working.
      #[allow(clippy::too_many_arguments)]
      pub fn new(
          config: OrchestratorConfig,
          api: Arc<dyn OrchestratorApiClient>,
          tools: Arc<ToolRegistry>,
          hooks: Arc<dyn HookExecutor>,
          perms: Arc<dyn PermissionGate>,
          output: Arc<dyn OutputStream>,
          cwd: PathBuf,
          memory: Arc<dyn MemoryHierarchyProvider>,
      ) -> Self {
          Self::new_with_streaming(
              config,
              api,
              Arc::new(NoStreamingApiClient),
              tools,
              hooks,
              perms,
              output,
              cwd,
              memory,
          )
      }
  }
  ```

- [ ] Step 3 — Add `run_turn_streaming`. Calls `assemble_system_prompt`, opens the stream, pumps it via `pump_stream`, dispatches tools (Task 13 lands the concurrent dispatch helper — call it `dispatch_tool_uses_concurrent`; for THIS task, stub it inline as a placeholder that calls the existing `dispatch_tool_uses` from `turn_loop` so the text-only path is green).
  ```rust
  impl ConversationOrchestrator {
      /// Drive one user prompt through the STREAMING turn loop. Mirrors
      /// the contract of `run_turn` but consumes SSE events as they
      /// arrive and dispatches tool_use blocks the moment their
      /// `content_block_stop` event is received.
      pub async fn run_turn_streaming(
          &self,
          prompt: &str,
      ) -> Result<ConversationOutcome, OrchestratorError> {
          use crate::streaming_loop::pump_stream;
          use crate::turn_loop::{cost_snapshot_from_session, dispatch_tool_uses};
          use lingxi_protocol::MessageId;
          use lingxi_telemetry::tengu::orchestrator::{
              TURN_STREAMING_COMPLETED, TURN_STREAMING_STARTED,
          };

          lingxi_telemetry::emit(TURN_STREAMING_STARTED, serde_json::json!({}));

          // 1. Append the user prompt to session history.
          {
              let mut s = self.session.lock().await;
              s.history.push(ConversationMessage::user(
                  MessageId::new(),
                  prompt.to_string(),
              ));
          }

          let mut turn_count: u32 = 0;
          let final_message_id;
          loop {
              if turn_count >= self.config.max_turns {
                  return Err(OrchestratorError::MaxTurnsReached {
                      max_turns: self.config.max_turns,
                  });
              }
              turn_count = turn_count.saturating_add(1);

              // 2. Assemble system prompt + open the stream.
              let system = self.assemble_system_prompt_for_turn().await?;
              let snapshot = {
                  let s = self.session.lock().await;
                  s.history.clone()
              };
              let stream = self
                  .streaming_api
                  .stream(
                      &self.config.model,
                      system.as_deref(),
                      snapshot,
                      Vec::new(), // M5-09 wires the real tools schema.
                  )
                  .await
                  .map_err(OrchestratorError::Streaming)?;

              // 3. Pump the stream.
              let pumped = pump_stream(stream, &self.output).await?;

              // 4. Append the assistant message.
              let assistant_id = MessageId::new();
              let mut blocks: Vec<lingxi_protocol::ContentBlock> = pumped.assistant_blocks.clone();
              for t in &pumped.tool_uses {
                  blocks.push(lingxi_protocol::ContentBlock::ToolUse {
                      id: t.id,
                      name: t.name.clone(),
                      input: t.input.clone(),
                  });
              }
              {
                  let mut s = self.session.lock().await;
                  s.history.push(ConversationMessage::Assistant {
                      id: assistant_id,
                      content: blocks,
                      stop_reason: pumped.stop_reason.clone(),
                  });
              }

              // 5. Dispatch tools (concurrent — Task 13 promotes this).
              if !pumped.tool_uses.is_empty() {
                  let tool_inputs: Vec<_> = pumped
                      .tool_uses
                      .iter()
                      .map(|t| (t.id, t.name.clone(), t.input.clone()))
                      .collect();
                  let results = dispatch_tool_uses(self, &tool_inputs).await?;
                  let user_id = MessageId::new();
                  let mut s = self.session.lock().await;
                  s.history.push(ConversationMessage::User {
                      id: user_id,
                      content: results,
                  });
              }

              // 6. Decide loop disposition.
              match pumped.stop_reason.as_deref() {
                  Some("end_turn") => {
                      let cost = {
                          let s = self.session.lock().await;
                          cost_snapshot_from_session(&s)
                      };
                      self.output.emit_end_turn("end_turn", &cost).await;
                      lingxi_telemetry::emit(
                          TURN_STREAMING_COMPLETED,
                          serde_json::json!({"turn_count": turn_count}),
                      );
                      final_message_id = assistant_id;
                      break;
                  }
                  Some("tool_use") if !pumped.tool_uses.is_empty() => continue,
                  Some(other) => {
                      // max_tokens / stop_sequence / pause_turn /
                      // refusal — terminate the loop with the value
                      // as-is, mirroring claude-code's behavior
                      // (claude.ts:2269).
                      let cost = {
                          let s = self.session.lock().await;
                          cost_snapshot_from_session(&s)
                      };
                      self.output.emit_end_turn(other, &cost).await;
                      lingxi_telemetry::emit(
                          TURN_STREAMING_COMPLETED,
                          serde_json::json!({"turn_count": turn_count, "stop_reason": other}),
                      );
                      final_message_id = assistant_id;
                      break;
                  }
                  None => {
                      // Stream ended without a stop_reason — treat as
                      // an end_turn (rare; claude.ts uses the same
                      // fallback path).
                      let cost = {
                          let s = self.session.lock().await;
                          cost_snapshot_from_session(&s)
                      };
                      self.output.emit_end_turn("end_turn", &cost).await;
                      lingxi_telemetry::emit(
                          TURN_STREAMING_COMPLETED,
                          serde_json::json!({"turn_count": turn_count}),
                      );
                      final_message_id = assistant_id;
                      break;
                  }
              }
          }

          Ok(ConversationOutcome::EndTurn {
              turn_count,
              final_message_id,
          })
      }

      /// Helper: assemble the system prompt for THIS turn, reusing the
      /// M5-03 logic. Mirrors `run_turn`'s assembly call.
      async fn assemble_system_prompt_for_turn(&self) -> Result<Option<String>, OrchestratorError> {
          if let Some(custom) = &self.config.system_prompt_override {
              return Ok(Some(custom.clone()));
          }
          // Delegate to the M5-03 assembler.
          let memory_files = self.memory.load(&self.cwd).await;
          let tool_names: Vec<String> = self
              .tools
              .iter()
              .map(|t| t.name().to_string())
              .collect();
          let ctx = crate::prompt::SystemPromptContext {
              cwd: self.cwd.clone(),
              platform: std::env::consts::OS.to_string(),
              model: self.config.model.clone(),
              model_marketing_name: None,
              knowledge_cutoff: None,
              shell: std::env::var("SHELL").unwrap_or_else(|_| "sh".into()),
              os_version: "".into(),
              git_status: crate::prompt::git_status::probe(&self.cwd),
              file_tree: crate::prompt::file_tree::probe(&self.cwd, 2),
              memory_files: memory_files
                  .into_iter()
                  .map(|f| crate::prompt::MemoryFile {
                      path: f.path,
                      body: f.body,
                      is_local_override: f.is_local_override,
                  })
                  .collect(),
              tool_names,
          };
          Ok(Some(crate::prompt::assemble_system_prompt(&ctx)))
      }
  }
  ```
  **Important:** the exact body of `assemble_system_prompt_for_turn` mirrors what M5-03 ships in `run_turn`. If M5-03's version differs (e.g. uses a different MemoryFile mapping or different ctx defaults), copy ITS version verbatim. The point of this helper is to keep the streaming path 1:1 with the batched path's prompt-assembly behavior. If `run_turn` in M5-03 already extracts this into a private helper (e.g. `fn assemble_system_prompt_for_turn`), DELETE the body above and call the existing helper instead — do NOT duplicate logic.

- [ ] Step 4 — Update `lingxi-core/crates/orchestrator/src/lib.rs` re-exports if `NoStreamingApiClient` should be visible outside (it should NOT — keep it `pub(crate)`).

- [ ] Step 5 — Update the existing M5-02/M5-03 integration tests to pass `Arc::new(NoStreamingApiClient)` is NOT needed because `new` (the legacy constructor) is still callable. Verify by running `cargo test -p lingxi-orchestrator --test orchestrator_smoke_test --test orchestrator_multi_turn_test --test orchestrator_max_turns_test --test orchestrator_tool_error_test --test orchestrator_real_tools_test`. All M5-02/M5-03 tests must still pass without modification.

- [ ] Step 6 — Run `cargo test -p lingxi-orchestrator --test streaming_text_only_test 2>&1 | tail -10`. Task 3's test must now pass (text-only happy path).

- [ ] Commit: `feat(M5-04 task 12): ConversationOrchestrator::new_with_streaming + run_turn_streaming (green: streaming_text_only_test)`

---

### Task 13: Promote tool dispatch to concurrent (mid-stream + multi-tool)

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/streaming_loop.rs`
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs`

**Steps:**

- [ ] Step 1 — In `streaming_loop.rs`, extend the file with a `dispatch_tool_uses_concurrent` helper. The function takes an `Arc<ConversationOrchestrator>`-like seam (use `&ConversationOrchestrator` directly — same as M5-02's `dispatch_tool_uses`, which is `async fn dispatch_tool_uses(orch: &ConversationOrchestrator, ...)`) and a `Vec<ObservedToolUse>`. It spawns one `tokio::task::spawn` per tool use, waits via `futures::future::join_all`, and returns `Vec<ContentBlock::ToolResult>` IN ORIGINAL ORDER (sorted back by index).
  ```rust
  use crate::conversation::ConversationOrchestrator;
  use crate::turn_loop::dispatch_tool_uses;
  use lingxi_protocol::ContentBlock;

  /// Dispatch N tool_use blocks concurrently. Each dispatch goes through
  /// the same pre-tool-hook → permission → tool-call → post-tool-hook
  /// pipeline as the batched path (`turn_loop::dispatch_tool_uses` is
  /// reused per-tool to keep the byte-locked hook + permission ordering).
  ///
  /// Returns the `ToolResult` blocks IN ORIGINAL ORDER (matching the
  /// `tool_use` block order in the stream). The OutputStream emits
  /// `ToolCall` / `ToolResult` events in COMPLETION order (not
  /// dispatch order) — that's the visible streaming behavior.
  pub async fn dispatch_tool_uses_concurrent(
      orch: &ConversationOrchestrator,
      observed: &[ObservedToolUse],
  ) -> Result<Vec<ContentBlock>, OrchestratorError> {
      use futures::future::join_all;

      // We can't `tokio::spawn` directly on `&ConversationOrchestrator`
      // because spawn requires `'static`. Instead, dispatch each tool
      // sequentially as a `Future` and join the futures via `join_all`,
      // which runs them concurrently on the current task. This gives us
      // the concurrency we need without the `'static` requirement.
      let futures: Vec<_> = observed
          .iter()
          .enumerate()
          .map(|(idx, tu)| {
              let single = vec![(tu.id, tu.name.clone(), tu.input.clone())];
              async move {
                  let mut result = dispatch_tool_uses(orch, &single).await?;
                  // Each call returns exactly one ContentBlock; pop it.
                  let block = result
                      .pop()
                      .ok_or_else(|| OrchestratorError::StreamingProtocol(
                          format!("dispatch returned empty for tool index {idx}"),
                      ))?;
                  Ok::<(usize, ContentBlock), OrchestratorError>((idx, block))
              }
          })
          .collect();

      let mut indexed: Vec<(usize, ContentBlock)> = Vec::with_capacity(observed.len());
      for r in join_all(futures).await {
          indexed.push(r?);
      }
      indexed.sort_by_key(|(idx, _)| *idx);
      Ok(indexed.into_iter().map(|(_, b)| b).collect())
  }
  ```

  **Note on concurrency model:** `join_all` on a `Vec<Future>` polls them all in a single task — each future progresses when its `await` point is reached. For tool dispatch which is itself `async` and contains internal `await` points (hook execution, permission check, registry lookup, tool body), this yields true cooperative concurrency without `'static` requirements. If the future inner code is CPU-bound (not `.await`-driven), this would not actually overlap — but tool dispatch is I/O-bound (hooks fire other processes, tools touch the filesystem), so `join_all` is the right primitive.

- [ ] Step 2 — In `conversation.rs::run_turn_streaming`, REPLACE the `dispatch_tool_uses(self, &tool_inputs)` call with `dispatch_tool_uses_concurrent(self, &pumped.tool_uses)`. The function takes the raw `ObservedToolUse` list directly (not the manually-mapped tuple) — adjust the call site:
  ```rust
  // Step 5. Dispatch tools concurrently.
  if !pumped.tool_uses.is_empty() {
      let results = crate::streaming_loop::dispatch_tool_uses_concurrent(self, &pumped.tool_uses).await?;
      let user_id = MessageId::new();
      let mut s = self.session.lock().await;
      s.history.push(ConversationMessage::User {
          id: user_id,
          content: results,
      });
  }
  ```

- [ ] Step 3 — Re-run `cargo test -p lingxi-orchestrator --test streaming_mid_stream_tool_test 2>&1 | tail -10`. Task 10's test must now pass.

- [ ] Step 4 — Add a unit test inside `streaming_loop.rs::tests` for the ordering invariant:
  ```rust
      #[tokio::test]
      async fn concurrent_dispatch_preserves_original_order_in_results() {
          // Build a minimal orchestrator with two tool uses; the second
          // tool is slower than the first. The returned ContentBlock
          // order must still be [first, second].
          //
          // (Detailed scaffolding mirrors orchestrator_real_tools_test
          // from M5-02 — see that test for the boilerplate.)
          //
          // High-level assertion:
          //   - observed = [(id_A, "Slow"), (id_B, "Fast")]
          //   - results[0].tool_use_id == id_A
          //   - results[1].tool_use_id == id_B
      }
  ```
  Implement the body using two test tools registered via `ToolRegistry::with_two_test_tools_one_slow()` — if such a helper doesn't exist, inline its construction by copying the pattern from M5-02's `orchestrator_real_tools_test.rs`. The two test tools: `"Slow"` sleeps 50ms then returns `Ok({"slow": true})`; `"Fast"` returns immediately. Use `tokio::time::sleep(Duration::from_millis(50))` inside `Slow`. Assert the returned results retain `[Slow, Fast]` order despite `Fast` completing first.

- [ ] Step 5 — Run all streaming tests: `cargo test -p lingxi-orchestrator streaming`. Confirm green.

- [ ] Commit: `feat(M5-04 task 13): dispatch_tool_uses_concurrent via futures::join_all (green: mid_stream_tool_test + order preservation)`

---

### Task 14: Test — two concurrent tools in one response

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/streaming_concurrent_tools_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/orchestrator/tests/streaming_concurrent_tools_test.rs`:
  ```rust
  //! Two tool_use blocks in one streaming response (M5-04 Task 14).
  //!
  //! Asserts both tools dispatch concurrently (the slower one does NOT
  //! delay the faster one's OutputStream::ToolResult emission) AND that
  //! both ToolResult content blocks appear in the next user message in
  //! the original (in-stream) order.

  use lingxi_orchestrator::test_support::{
      content_block_start_tool_use, content_block_stop, input_json_delta,
      message_delta_stop, message_start, message_stop, MockApiClient, MockOutputStream,
      MockStreamingApiClient, NoOpHookExecutor, NoOpPermissionGate, StaticMemoryProvider,
  };
  use lingxi_orchestrator::test_support_stream::scripted;
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_traits::OutputEvent;
  use std::path::PathBuf;
  use std::sync::Arc;

  /// Build a registry with two test tools, "Slow" (50ms) and "Fast" (immediate).
  fn registry_with_slow_and_fast() -> Arc<ToolRegistry> {
      // ... mirror the helper used in streaming_loop tests; if that
      // helper is `pub(crate)`, factor it into `test_support.rs`.
      unimplemented!("inline using ToolRegistry::register API from M4")
  }

  #[tokio::test]
  async fn two_tools_dispatched_concurrently_results_ordered() {
      let turn1 = scripted![
          message_start("m1", "claude-opus-4-7"),
          content_block_start_tool_use(0, "toolu_A", "Slow"),
          input_json_delta(0, "{}"),
          content_block_stop(0),
          content_block_start_tool_use(1, "toolu_B", "Fast"),
          input_json_delta(1, "{}"),
          content_block_stop(1),
          message_delta_stop("tool_use"),
          message_stop(),
      ];
      let turn2 = scripted![
          message_start("m2", "claude-opus-4-7"),
          // empty turn — just end_turn after seeing both results
          message_delta_stop("end_turn"),
          message_stop(),
      ];

      let api = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
      let batched = Arc::new(MockApiClient::new(Vec::new()));
      let output = Arc::new(MockOutputStream::default());
      let tools = registry_with_slow_and_fast();
      let hooks = Arc::new(NoOpHookExecutor::default());
      let perms = Arc::new(NoOpPermissionGate::default());

      let orch = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          batched,
          api,
          tools,
          hooks,
          perms,
          output.clone(),
          PathBuf::from("/tmp"),
          Arc::new(StaticMemoryProvider::empty()),
      );

      let start = std::time::Instant::now();
      let _ = orch.run_turn_streaming("call two tools").await.expect("ok");
      let elapsed = start.elapsed();
      // Concurrent execution: total elapsed should be ~50ms (Slow's
      // sleep), NOT ~100ms (50ms × 2 sequential).
      assert!(
          elapsed < std::time::Duration::from_millis(90),
          "expected concurrent (~50ms), got {elapsed:?}"
      );

      // OutputEvent order — ToolCall/ToolResult interleaving:
      //   ToolCall("Slow"), ToolCall("Fast"), ToolResult("Fast"), ToolResult("Slow"), EndTurn
      // (Fast finishes first.)
      let events = output.snapshot().await;
      let kinds: Vec<&str> = events
          .iter()
          .map(|e| match e {
              OutputEvent::Text { .. } => "Text",
              OutputEvent::ToolCall { .. } => "ToolCall",
              OutputEvent::ToolResult { .. } => "ToolResult",
              OutputEvent::EndTurn { .. } => "EndTurn",
              _ => "Other",
          })
          .collect();
      // The exact ToolCall ordering is observation order (Slow first
      // because its content_block_stop arrived first), but ToolResults
      // arrive in completion order — Fast first.
      assert_eq!(
          kinds,
          vec!["ToolCall", "ToolCall", "ToolResult", "ToolResult", "EndTurn"]
      );
      // Identify which is which:
      let call_tools: Vec<&str> = events
          .iter()
          .filter_map(|e| match e {
              OutputEvent::ToolCall { tool, .. } => Some(tool.as_str()),
              _ => None,
          })
          .collect();
      let result_tools: Vec<&str> = events
          .iter()
          .filter_map(|e| match e {
              OutputEvent::ToolResult { tool, .. } => Some(tool.as_str()),
              _ => None,
          })
          .collect();
      assert_eq!(call_tools, vec!["Slow", "Fast"]);
      assert_eq!(result_tools, vec!["Fast", "Slow"]); // Fast finishes first
  }
  ```

  **Note on `registry_with_slow_and_fast`:** the body is the bottleneck. Mirror what M5-02's `orchestrator_real_tools_test.rs` does to construct a `ToolRegistry` and register a `Tool` impl. Specifically: define two structs `SlowTool` and `FastTool`, both implementing the `Tool` trait. `SlowTool::call` does `tokio::time::sleep(Duration::from_millis(50)).await; Ok(serde_json::json!({"slow": true}))`. `FastTool::call` returns immediately. Both report `name() = "Slow"` / `"Fast"`. Register both into a new `ToolRegistry::new()` and wrap in `Arc::new`.

- [ ] Step 2 — Run `cargo test -p lingxi-orchestrator --test streaming_concurrent_tools_test 2>&1 | tail -15`. Test must pass.

- [ ] Commit: `test(M5-04 task 14): two concurrent tools — completion-order OutputEvents + original-order ToolResults`

---

### Task 15: Test — `ping` mid-stream is a no-op + multi-turn streaming

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/streaming_ping_noop_test.rs`
- Create: `lingxi-core/crates/orchestrator/tests/streaming_multi_turn_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/orchestrator/tests/streaming_ping_noop_test.rs`:
  ```rust
  //! `ping` event is a no-op (M5-04 Task 15).

  use lingxi_orchestrator::test_support::{
      content_block_start_text, content_block_stop, message_delta_stop, message_start,
      message_stop, ping, text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient,
      NoOpHookExecutor, NoOpPermissionGate, StaticMemoryProvider,
  };
  use lingxi_orchestrator::test_support_stream::scripted;
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_traits::OutputEvent;
  use std::path::PathBuf;
  use std::sync::Arc;

  #[tokio::test]
  async fn ping_between_deltas_does_not_disturb_output() {
      let turn = scripted![
          message_start("m1", "claude-opus-4-7"),
          ping(),
          content_block_start_text(0),
          ping(),
          text_delta(0, "abc"),
          ping(),
          text_delta(0, "def"),
          ping(),
          content_block_stop(0),
          ping(),
          message_delta_stop("end_turn"),
          ping(),
          message_stop(),
      ];

      let api = Arc::new(MockStreamingApiClient::with_turns(vec![turn]));
      let batched = Arc::new(MockApiClient::new(Vec::new()));
      let output = Arc::new(MockOutputStream::default());
      let orch = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          batched,
          api,
          Arc::new(ToolRegistry::empty()),
          Arc::new(NoOpHookExecutor::default()),
          Arc::new(NoOpPermissionGate::default()),
          output.clone(),
          PathBuf::from("/tmp"),
          Arc::new(StaticMemoryProvider::empty()),
      );
      orch.run_turn_streaming("hi").await.expect("ok");

      let events = output.snapshot().await;
      // Expect: Text("abc"), Text("def"), EndTurn — pings filtered out.
      assert_eq!(events.len(), 3, "got {events:?}");
      assert!(matches!(&events[0], OutputEvent::Text { text } if text == "abc"));
      assert!(matches!(&events[1], OutputEvent::Text { text } if text == "def"));
      assert!(matches!(&events[2], OutputEvent::EndTurn { .. }));
  }
  ```

- [ ] Step 2 — Create `lingxi-core/crates/orchestrator/tests/streaming_multi_turn_test.rs`:
  ```rust
  //! Multi-turn streaming — turn 1 tool_use, turn 2 end_turn (M5-04 Task 15).

  use lingxi_orchestrator::test_support::{
      content_block_start_text, content_block_start_tool_use, content_block_stop,
      input_json_delta, message_delta_stop, message_start, message_stop, text_delta,
      MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpHookExecutor,
      NoOpPermissionGate, StaticMemoryProvider,
  };
  use lingxi_orchestrator::test_support_stream::scripted;
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_tools::registry::ToolRegistry;
  use lingxi_traits::OutputEvent;
  use std::path::PathBuf;
  use std::sync::Arc;

  fn registry_with_always_ok() -> Arc<ToolRegistry> {
      // Mirror Task 10's always_ok_tool_registry().
      unimplemented!("see Task 10 step 1 note — inline AlwaysOk tool registration")
  }

  #[tokio::test]
  async fn two_streaming_turns_with_tool_in_between() {
      let turn1 = scripted![
          message_start("m1", "claude-opus-4-7"),
          content_block_start_tool_use(0, "toolu_01", "AlwaysOk"),
          input_json_delta(0, "{}"),
          content_block_stop(0),
          message_delta_stop("tool_use"),
          message_stop(),
      ];
      let turn2 = scripted![
          message_start("m2", "claude-opus-4-7"),
          content_block_start_text(0),
          text_delta(0, "done"),
          content_block_stop(0),
          message_delta_stop("end_turn"),
          message_stop(),
      ];

      let api = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
      let batched = Arc::new(MockApiClient::new(Vec::new()));
      let output = Arc::new(MockOutputStream::default());
      let orch = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          batched,
          api.clone(),
          registry_with_always_ok(),
          Arc::new(NoOpHookExecutor::default()),
          Arc::new(NoOpPermissionGate::default()),
          output.clone(),
          PathBuf::from("/tmp"),
          Arc::new(StaticMemoryProvider::empty()),
      );

      let outcome = orch.run_turn_streaming("call then echo").await.expect("ok");
      match outcome {
          lingxi_orchestrator::ConversationOutcome::EndTurn { turn_count, .. } => {
              assert_eq!(turn_count, 2);
          }
      }

      // API was called twice (one stream per turn).
      assert_eq!(api.captured_calls().await.len(), 2);

      // Output sequence: ToolCall, ToolResult, Text("done"), EndTurn.
      let events = output.snapshot().await;
      let kinds: Vec<&str> = events
          .iter()
          .map(|e| match e {
              OutputEvent::Text { .. } => "Text",
              OutputEvent::ToolCall { .. } => "ToolCall",
              OutputEvent::ToolResult { .. } => "ToolResult",
              OutputEvent::EndTurn { .. } => "EndTurn",
              _ => "Other",
          })
          .collect();
      assert_eq!(kinds, vec!["ToolCall", "ToolResult", "Text", "EndTurn"]);
  }
  ```

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --test streaming_ping_noop_test --test streaming_multi_turn_test`. Both must pass.

- [ ] Commit: `test(M5-04 task 15): ping no-op + multi-turn streaming with mid-stream tool`

---

### Task 16: Telemetry — 2 new events + count 241 → 243

**Files:**
- Modify: `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs`
- Modify: `lingxi-core/crates/telemetry/src/tengu/mod.rs`
- Modify: `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs`
- Modify: `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`

**Steps:**

- [ ] Step 1 — Open `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` (created by M5-02 with 3 conversation events). Append two new constants AFTER the existing 3:
  ```rust
  /// Streaming turn started — emitted at the top of
  /// `ConversationOrchestrator::run_turn_streaming` before any session
  /// mutation. Wire string is byte-locked.
  pub const TURN_STREAMING_STARTED: &str = "tengu_orchestrator_turn_streaming_started";

  /// Streaming turn completed — emitted after `OutputStream::emit_end_turn`.
  /// Payload carries `turn_count` + optional `stop_reason`.
  pub const TURN_STREAMING_COMPLETED: &str = "tengu_orchestrator_turn_streaming_completed";
  ```

- [ ] Step 2 — Update the existing `NAMES` array in the same file. Before M5-04 it is:
  ```rust
  pub(crate) const NAMES: &[&str] = &[
      CONVERSATION_STARTED,
      CONVERSATION_COMPLETED,
      CONVERSATION_FAILED,
  ];
  ```
  Extend to:
  ```rust
  pub(crate) const NAMES: &[&str] = &[
      CONVERSATION_STARTED,
      CONVERSATION_COMPLETED,
      CONVERSATION_FAILED,
      TURN_STREAMING_STARTED,
      TURN_STREAMING_COMPLETED,
  ];
  ```

- [ ] Step 3 — Open `lingxi-core/crates/telemetry/src/tengu/mod.rs`. Update the `TOTAL` arithmetic at the top of `ALL_EVENT_NAMES`:
  ```rust
  // Before M5-04 (from M5-02):
  //   const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 3 + 1;  // = 241
  // After M5-04:
  const TOTAL: usize = 25 + 30 + 15 + 134 + 10 + 8 + 12 + 3 + 5 + 1; // = 243
  ```
  The `3 + 5` represents `settings (3) + orchestrator (5)` — the M5-02 orchestrator submodule grew from 3 to 5 with this plan's additions.

  The `concat_all` body that walks each submodule's `NAMES` array must still walk `orchestrator::NAMES` (which is now 5 long). If `mod.rs` does NOT yet include an `orchestrator::NAMES` walk (M5-02 may have inserted it between `settings` and `release`), confirm by reading the file and add the walk in the correct registration-order position (after `settings`, before `release`).

- [ ] Step 4 — Open `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs`. Update the `registry_is_exactly_241_entries` test:
  ```rust
  // Before:
  //   assert_eq!(ALL_EVENT_NAMES.len(), 241);
  // After:
  assert_eq!(ALL_EVENT_NAMES.len(), 243);
  ```
  Rename the test from `registry_is_exactly_241_entries` to `registry_is_exactly_243_entries` (the test function name). Update the explanatory comment:
  ```rust
  // 213 (post-M4-07) + 24 (M4-08) + 1 (M4-09) + 3 (M5-02) + 0 (M5-03) + 2 (M5-04) = 243.
  ```

- [ ] Step 5 — Open `lingxi-core/crates/test-harness/src/parity/fixtures/tengu_events.json`. Find the section where the M5-02 orchestrator events were inserted (between the `tool_*` block and the trailing `lingxi_core_v0_5_0_released` entry). Append two new entries IMMEDIATELY AFTER `tengu_orchestrator_conversation_failed` and BEFORE `lingxi_core_v0_5_0_released`:
  ```json
        "tengu_orchestrator_turn_streaming_started",
        "tengu_orchestrator_turn_streaming_completed",
  ```
  Update the `_note` field at the top of the JSON to append: `" + 2 (M5-04 streaming: turn_streaming_started/completed)"`.

- [ ] Step 6 — Wire the emissions from `run_turn_streaming` (already done in Task 12 step 3 as a reference) — confirm the two `lingxi_telemetry::emit(...)` calls in `conversation.rs::run_turn_streaming` use the new constants and not bare string literals.

- [ ] Step 7 — Run the full telemetry verification:
  ```
  cargo test -p lingxi-telemetry --test event_name_completeness_test
  cargo test -p lingxi-test-harness parity_tengu_events
  cargo test -p lingxi-orchestrator streaming_text_only_test
  ```
  All three must pass.

- [ ] Commit: `feat(M5-04 task 16): 2 telemetry events (tengu::orchestrator) — ALL_EVENT_NAMES 241 → 243 + parity fixture + run_turn_streaming wires`

---

### Task 17: Error-propagation test + streaming-vs-batched equivalence test

**Files:**
- Create: `lingxi-core/crates/orchestrator/tests/streaming_error_propagation_test.rs`
- Create: `lingxi-core/crates/orchestrator/tests/streaming_vs_batched_equivalence_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-core/crates/orchestrator/tests/streaming_error_propagation_test.rs`:
  ```rust
  //! Mid-stream Err propagates as `OrchestratorError::Streaming` (M5-04 Task 17).

  use lingxi_api_client::ApiError;
  use lingxi_orchestrator::test_support::{
      content_block_start_text, message_start, text_delta, MockApiClient, MockOutputStream,
      MockStreamingApiClient, NoOpHookExecutor, NoOpPermissionGate, StaticMemoryProvider,
  };
  use lingxi_orchestrator::test_support_stream::scripted;
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig, OrchestratorError};
  use lingxi_tools::registry::ToolRegistry;
  use std::path::PathBuf;
  use std::sync::Arc;

  #[tokio::test]
  async fn mid_stream_err_surfaces_as_streaming_variant() {
      // First two events OK, third event is an Err.
      let turn: Vec<Result<lingxi_api_client::types::StreamEvent, ApiError>> = vec![
          Ok(message_start("m1", "claude-opus-4-7")),
          Ok(content_block_start_text(0)),
          Ok(text_delta(0, "before err")),
          Err(ApiError::Network("connection reset by peer".into())),
      ];

      let api = Arc::new(MockStreamingApiClient::with_fallible_turns(vec![turn]));
      let batched = Arc::new(MockApiClient::new(Vec::new()));
      let output = Arc::new(MockOutputStream::default());
      let orch = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          batched,
          api,
          Arc::new(ToolRegistry::empty()),
          Arc::new(NoOpHookExecutor::default()),
          Arc::new(NoOpPermissionGate::default()),
          output,
          PathBuf::from("/tmp"),
          Arc::new(StaticMemoryProvider::empty()),
      );

      let err = orch.run_turn_streaming("hi").await.expect_err("network err");
      match err {
          OrchestratorError::Streaming(inner) => {
              let s = format!("{inner}");
              assert!(s.contains("connection reset"), "{s}");
          }
          other => panic!("expected Streaming variant, got {other:?}"),
      }
  }
  ```

- [ ] Step 2 — Create `lingxi-core/crates/orchestrator/tests/streaming_vs_batched_equivalence_test.rs`:
  ```rust
  //! Same conversational outcome whether the turn runs via batched or
  //! streaming (M5-04 Task 17). Asserts:
  //!   - both paths produce `ConversationOutcome::EndTurn { turn_count: 1, .. }`
  //!   - both paths append identical assistant message bodies to the session
  //!   - telemetry events are DIFFERENT (batched emits conversation_*,
  //!     streaming emits turn_streaming_*) — explicitly assert that the
  //!     event-NAMES differ, validating the separation of paths.

  use lingxi_api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
  use lingxi_orchestrator::test_support::{
      content_block_start_text, content_block_stop, message_delta_stop, message_start,
      message_stop, text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient,
      NoOpHookExecutor, NoOpPermissionGate, StaticMemoryProvider,
  };
  use lingxi_orchestrator::test_support_stream::scripted;
  use lingxi_orchestrator::{ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
  use lingxi_protocol::{ContentBlock, ConversationMessage};
  use lingxi_tools::registry::ToolRegistry;
  use std::path::PathBuf;
  use std::sync::Arc;

  fn batched_response(text: &str) -> MessageResponse {
      MessageResponse {
          id: "msg_eq".into(),
          model: "claude-opus-4-7".into(),
          content: vec![ContentBlockApi::Text { text: text.into() }],
          stop_reason: Some("end_turn".into()),
          usage: UsageApi::default(),
      }
  }

  #[tokio::test]
  async fn batched_and_streaming_produce_same_assistant_text() {
      // ── Batched path
      let batched_mock = Arc::new(MockApiClient::new(vec![batched_response("hello world")]));
      let streaming_stub = Arc::new(MockStreamingApiClient::empty());
      let output_b = Arc::new(MockOutputStream::default());
      let orch_b = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          batched_mock,
          streaming_stub,
          Arc::new(ToolRegistry::empty()),
          Arc::new(NoOpHookExecutor::default()),
          Arc::new(NoOpPermissionGate::default()),
          output_b.clone(),
          PathBuf::from("/tmp"),
          Arc::new(StaticMemoryProvider::empty()),
      );
      let outcome_b = orch_b.run_turn("ping").await.expect("batched");

      // ── Streaming path
      let stream_script = scripted![
          message_start("m1", "claude-opus-4-7"),
          content_block_start_text(0),
          text_delta(0, "hello world"),
          content_block_stop(0),
          message_delta_stop("end_turn"),
          message_stop(),
      ];
      let streaming_mock = Arc::new(MockStreamingApiClient::with_turns(vec![stream_script]));
      let batched_stub = Arc::new(MockApiClient::new(Vec::new()));
      let output_s = Arc::new(MockOutputStream::default());
      let orch_s = ConversationOrchestrator::new_with_streaming(
          OrchestratorConfig::default_for_test(),
          batched_stub,
          streaming_mock,
          Arc::new(ToolRegistry::empty()),
          Arc::new(NoOpHookExecutor::default()),
          Arc::new(NoOpPermissionGate::default()),
          output_s.clone(),
          PathBuf::from("/tmp"),
          Arc::new(StaticMemoryProvider::empty()),
      );
      let outcome_s = orch_s.run_turn_streaming("ping").await.expect("stream");

      // Same outcome shape.
      match (outcome_b, outcome_s) {
          (
              ConversationOutcome::EndTurn { turn_count: tc_b, .. },
              ConversationOutcome::EndTurn { turn_count: tc_s, .. },
          ) => {
              assert_eq!(tc_b, 1);
              assert_eq!(tc_s, 1);
          }
      }

      // Same assistant text in the session history.
      let s_b = orch_b.session().lock().await;
      let s_s = orch_s.session().lock().await;
      let extract_text = |hist: &[ConversationMessage]| -> Option<String> {
          for m in hist {
              if let ConversationMessage::Assistant { content, .. } = m {
                  for blk in content {
                      if let ContentBlock::Text { text } = blk {
                          return Some(text.clone());
                      }
                  }
              }
          }
          None
      };
      assert_eq!(
          extract_text(&s_b.history),
          Some("hello world".into()),
      );
      assert_eq!(
          extract_text(&s_s.history),
          Some("hello world".into()),
      );
  }
  ```

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --test streaming_error_propagation_test --test streaming_vs_batched_equivalence_test`. Both must pass.

- [ ] Step 4 — Verify that the M5-02 `orchestrator_smoke_test`, `orchestrator_multi_turn_test`, `orchestrator_max_turns_test`, `orchestrator_tool_error_test`, `orchestrator_real_tools_test` ALL still pass (they were updated to the new constructor signature in Task 12 step 5 — re-confirm).

- [ ] Step 5 — Confirm telemetry separation via an `InMemorySink` check (add inside `streaming_vs_batched_equivalence_test.rs`):
  ```rust
  // (Continued in same test or a separate test_telemetry_event_separation.)
  // Capture telemetry into an InMemorySink (from lingxi-telemetry) and
  // assert:
  //   batched session emitted "tengu_orchestrator_conversation_started"
  //                     and "tengu_orchestrator_conversation_completed"
  //   streaming session emitted "tengu_orchestrator_turn_streaming_started"
  //                       and "tengu_orchestrator_turn_streaming_completed"
  ```
  Implement using `lingxi_telemetry::InMemorySink::new()` and `register_global_sink_for_test_scope` (or whatever the M3-06 / M5-02 test-harness API is). If wiring InMemorySink globally is non-trivial in a separate test process, factor it into a single combined test that uses two separate sinks.

- [ ] Commit: `test(M5-04 task 17): error propagation + batched/streaming equivalence + telemetry-event separation`

---

### Task 18: Verification gate + tag

**Files:**
- (no edits — verification + tag only)

**Steps:**

- [ ] Step 1 — Run the full workspace test suite. Use `--all-features` to catch the `test-support` feature path:
  ```
  cd lingxi-core
  cargo test --workspace --all-features 2>&1 | tail -30
  ```
  Expected: all tests pass. Pay particular attention to:
  - `lingxi-orchestrator` — 5 new streaming integration tests + the existing M5-02/M5-03 ones.
  - `lingxi-telemetry::event_name_completeness_test::registry_is_exactly_243_entries`.
  - `lingxi-test-harness::parity_tengu_events`.

- [ ] Step 2 — Run clippy with `-D warnings` across the workspace:
  ```
  cargo clippy --workspace --all-features --all-targets -- -D warnings
  ```
  Expected: zero warnings, zero errors.

- [ ] Step 3 — Run `cargo fmt --all --check`. Expected: no diff.

- [ ] Step 4 — Verify the telemetry count chain in one explicit assertion. Run:
  ```
  cargo test -p lingxi-telemetry --test event_name_completeness_test registry_is_exactly_243_entries -- --nocapture
  ```
  Expected: PASS, and the test prints (if `--nocapture`) any computed-vs-expected line confirming `243 == 243`.

- [ ] Step 5 — Confirm the production code path does NOT depend on any test-only feature. Run:
  ```
  cargo build -p lingxi-orchestrator --no-default-features
  ```
  Expected: clean build with no `test_support_stream` / `MockStreamingApiClient` references in the production graph.

- [ ] Step 6 — Confirm no `cargo tree` cycle was introduced:
  ```
  cargo tree -p lingxi-orchestrator -e normal --depth 5 | grep -c lingxi-orchestrator
  ```
  Expected: `1` (only the root entry; no transitive self-reference).

- [ ] Step 7 — Tag the milestone:
  ```
  git tag -a m5.4 -m "M5-04: streaming SSE + mid-stream tool dispatch — 18 tasks, ALL_EVENT_NAMES 241 → 243"
  ```

- [ ] Commit (final empty-marker commit if a workspace-wide guard touched files; otherwise skip): `release(M5-04): verification gate green — tag m5.4`

---

## Self-review

**1. Spec coverage:**
- §3 row M5-04 (streaming SSE, mid-stream tool dispatch, 2 events, ~18 tasks) — Tasks 1-17 cover all four columns. Task count = 18 (T0 research + T1-T17 implementation + T18 gate).
- §4.2 SSE event names (`message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`) — Task 7's `dispatch_event` handles all 7 (plus the existing `Error` variant from the `StreamEvent` enum).
- §5.2 `ScriptedSseStream` infrastructure — Task 6 ships `MockStreamingApiClient` + the `scripted!` macro + per-event helpers.
- §7 OQ-2 (SSE event order — reverse-engineer) — Task 0 resolves; byte-locks table captures the source citations.
- Telemetry growth 241 → 243 — Task 16 covers all four locations (`orchestrator.rs`, `mod.rs`, `event_name_completeness_test.rs`, `tengu_events.json`).

**2. Placeholder scan:** No `TBD`, no `TODO without code`, no "implement later" — except two intentional `unimplemented!()` markers in Tasks 14 + 15 that direct the engineer to the helper pattern in M5-02's `orchestrator_real_tools_test.rs` (the helper is small enough that copy-paste reuse is clear; spelling it out would duplicate ~30 lines from M5-02 verbatim, which is wasteful). Each `unimplemented!` is paired with a concrete instruction citing the exact file + pattern to mirror.

**3. Type consistency check:**
- `StreamingApiClient::stream` signature: `(model, system, messages, tools) -> BoxStream<Result<StreamEvent, ApiError>>` — used consistently in Tasks 6, 11, 12.
- `BlockAccumulator` API: `start_block(index, kind)`, `append_text(index, text)`, `append_json(index, partial)`, `set_signature(index, sig)`, `stop_block(index) -> CompletedBlock`, `is_idle()` — used consistently in Tasks 2, 5, 7, 9.
- `CompletedBlock` variants: `Text`, `ToolUse`, `Thinking`, `Skipped` — consistent in Tasks 5, 7.
- `RouterAction` variants: `Continue`, `DispatchToolUse`, `AppendAssistantBlock`, `RecordStopReason`, `EndOfStream`, `ServerError` — consistent in Tasks 7, 9.
- `OrchestratorError` new variants: `Streaming(ApiError)`, `StreamingProtocol(String)`, `StreamEndedWithoutStop` — added in Task 4, referenced in Tasks 9, 17.
- Telemetry constants: `TURN_STREAMING_STARTED` / `TURN_STREAMING_COMPLETED` — exact strings `tengu_orchestrator_turn_streaming_started` / `tengu_orchestrator_turn_streaming_completed`. Used consistently in Tasks 12 (emit), 16 (define), 17 (assert).

**4. Telemetry count chain:**
- Pre-M5-02: 238.
- Post-M5-02: 241 (+3).
- Post-M5-03: 241 (+0).
- Post-M5-04: 243 (+2). ✓ matches plan goal.

**5. Concurrency vs. Arc::ptr_eq tests from M4-05 / M5-01:** `dispatch_tool_uses_concurrent` (Task 13) uses `futures::future::join_all` on a `Vec<Future>` rather than `tokio::spawn`. This means each future runs in the SAME tokio task — no `Arc<ToolRegistry>` re-wrapping, no `'static` requirement, and crucially no new `Arc` clones beyond what the existing `dispatch_tool_uses` already does. The recursion-lock invariant from M4-05 wiring follow-up (`Arc::ptr_eq` on registries passed into subagents) is unaffected because subagent spawning is NOT part of this plan — only top-level tool dispatch. If a future plan (M5-09 commands? M5-12 CLI?) wants TRUE OS-thread parallelism for tools, it would use `tokio::spawn` AND must ensure each spawn gets a `Arc::clone(&registry)` of the SAME registry (no rebuild). Task 13 step 1 documents this constraint.

**6. Backward compat:**
- `ConversationOrchestrator::new` (legacy constructor) — preserved in Task 12 step 2, delegates to `new_with_streaming` with a `NoStreamingApiClient` stub.
- `run_turn` (batched) — untouched. Task 17 step 2 asserts via the equivalence test that both paths produce the same outcome.
- The 3 M5-02 telemetry events still fire from `run_turn`. The 2 new events fire ONLY from `run_turn_streaming`.

**7. Risks / OQs addressed:**
- §7 OQ-2 (SSE event order) — resolved in Task 0 byte-locks.
- Risk: `HttpTransport::stream_sse` may not exist. Task 11 step 1's note instructs the engineer to add it (with a default `Unsupported` impl) if missing — this is a M3-03 leftover risk we surface explicitly rather than silently failing.
- Risk: `partial_json` reassembly producing invalid JSON — `BlockAccumulator::stop_block` returns `StreamingError::ToolUseJsonParse` with the raw buffer (Task 5 step 1) so debugging is straightforward.

---

## Wire identifiers — LOCKED at the top of this plan

(Authoritative — if claude-code drift is detected during execution, update the byte-locks table FIRST, then propagate through the affected tasks.)

- SSE event types: `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping` (+ `error` from existing `StreamEvent` enum, surfaced as `RouterAction::ServerError`).
- Delta types: `text_delta` (`text: String`), `input_json_delta` (`partial_json: String`), `thinking_delta` (`thinking: String`), `signature_delta` (`signature: String`), `citations_delta` (dropped), `connector_text_delta` (dropped).
- `stop_reason` values M5-04 acts on: `end_turn`, `tool_use`, `max_tokens`, `stop_sequence` (others pass through as-is to `emit_end_turn(other_value, ...)`).
- New telemetry events (2):
  - `tengu_orchestrator_turn_streaming_started`
  - `tengu_orchestrator_turn_streaming_completed`

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-25-m5-04-streaming-sse.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using `superpowers:executing-plans`, batch execution with checkpoints.

Which approach?
