# Streaming Tool Executor Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace LingXi's pump-to-completion-then-`join_all` streaming path with a faithful port of claude-code's `StreamingToolExecutor`, so tools execute mid-stream under concurrency control, Bash errors abort siblings, results carry `<tool_use_error>` wrappers, and the JSONL transcript parents each tool result to its originating assistant message — closing the P0 streaming + topology byte-parity gaps.

**Architecture:** A single-task `StreamingToolExecutor` struct in `orchestrator` owns a `Vec<TrackedTool>` state machine (`queued`/`executing`/`completed`/`yielded`) plus a `FuturesUnordered` of in-flight tool futures, all borrowing `&ConversationOrchestrator` (no `'static`/spawn). The live streaming loop drives the SSE stream and the executor on one task: a `RouterAction::DispatchToolUse` immediately calls `executor.add_tool(...)`; between stream events the loop drains completed results in *received* order and persists each as its own assistant-parented user message; after `EndOfStream` it drains the remainder. The non-streaming 529 fallback keeps its existing batch dispatch. Per-tool dispatch still reuses `dispatch_tool_uses_tracked` so the hook → permission → registry → hook ordering stays byte-locked.

**Tech Stack:** Rust, `tokio`, `futures::stream::FuturesUnordered`, `tokio_util::sync::CancellationToken`, `async_trait`. Crates touched: `orchestrator` (executor + streaming loop + persistence), `tool-api` (cancel field on `ToolUseContext`), `protocol`/`traits` (unchanged in Phase 1).

---

## Campaign Roadmap (context for this plan)

This plan is **Phase 1** of a sequenced parity campaign. Each phase is independently testable and shippable. Later phases are separate plan documents written after the prior phase merges.

- **Phase 1 (THIS PLAN) — Streaming tool executor + assistant-parented topology.** Mid-stream execution, concurrency control, Bash sibling abort (executor-level synthetic results), `<tool_use_error>` wrappers, unknown-tool wrapper, per-result user messages parented to the originating assistant UUID.
- **Phase 2 — Subprocess-level cancellation.** Thread the per-tool `CancellationToken` from `ToolUseContext` into the Bash/subprocess tools so an aborted sibling's OS process actually dies (Phase 1 substitutes the synthetic result but lets the in-flight future resolve). Removes the residual side-effect-timing divergence.
- **Phase 3 — Tool-id canonicalization (user-selected end state).** Replace the UUID-newtype `ToolUseId` + sidecar `provider_id`/`provider_tool_use_id` with the verbatim provider id as the single canonical transcript id, so JSONL/VCR/resume bytes match claude-code natively. Touches `protocol::ContentBlock`, resume/dedup, and convert.rs.
- **Phase 4 — Residual byte-fixes.** `isEnvTruthy` strict gate (conversation.rs:3564), tool-schema input-order (drop the name sort in `tool-api/src/wire.rs:10`), `normalizeMessagesForAPI` / tool-pairing-repair parity (provider_adapter.rs:401), low-frequency block preservation (redacted_thinking/server_tool_use/connector).

Phase 1 deliberately does **not** change the `ToolUseId` type (Phase 3) and does **not** kill subprocesses on abort (Phase 2). It produces correct transcript *bytes* for the common path now; Phase 2/3 remove the remaining timing/id divergences.

---

## Reference Semantics (claude-code, the parity target)

From `claude-code/src/services/tools/StreamingToolExecutor.ts` and `query.ts:826-862`:

1. **Mid-stream add.** As each assistant message streams in, every `tool_use` block is fed to `executor.addTool(block, assistantMessage)`. (LingXi: the router already emits `RouterAction::DispatchToolUse` at `ContentBlockStop`; `pump_stream` currently buffers them instead of dispatching.)
2. **Concurrency gate** (`canExecuteTool`): a tool may start if no tool is executing, OR (it is concurrency-safe AND every executing tool is concurrency-safe). `processQueue` walks the queue *in order*; it `break`s at the first non-concurrency-safe tool that cannot start yet (preserves order for exclusive tools).
3. **Bash sibling abort.** When an executing tool yields an `is_error` result AND it is the Bash tool, set `hasErrored = true`, record the tool description, and abort the sibling controller. Other not-yet-finished tools get a synthetic `<tool_use_error>Cancelled: parallel tool call <desc> errored</tool_use_error>` (or the desc-less form). Non-Bash errors do **not** cascade.
4. **Result order.** Results are buffered and emitted in the order tools were *received* (stream order), even though execution/completion is concurrent. `getCompletedResults` walks tools in order, yields completed ones, marks them `yielded`, and `break`s at an executing non-concurrency-safe tool.
5. **Unknown tool.** Completed immediately with `<tool_use_error>Error: No such tool available: <name></tool_use_error>` (`is_error: true`, `toolUseResult: "Error: No such tool available: <name>"`).
6. **Topology.** Each result is its own `createUserMessage({ content: [one tool_result], sourceToolAssistantUUID: assistantMessage.uuid })`. The `sourceToolAssistantUUID` becomes the JSONL `parentUuid` — i.e. each tool result is parented to the assistant message that *requested* it, not chained off the previous line.
7. **Discard.** On streaming fallback, `discard()` abandons pending/in-flight results; in-flight tools get a `streaming_fallback` synthetic error.

---

## File Structure

- **Create** `orchestrator/src/streaming_executor.rs` — the `StreamingToolExecutor` struct, `TrackedTool`, `ToolStatus`, concurrency gate, synthetic-message builders, ordered drain. One responsibility: scheduling + buffering tool execution for the streaming path.
- **Modify** `orchestrator/src/streaming_loop.rs` — change `pump_stream` to accept a `&mut StreamingToolExecutor` and dispatch/drain mid-stream instead of buffering into `PumpedTurn.tool_uses`. Delete `dispatch_tool_uses_concurrent` (superseded) after the loop is migrated.
- **Modify** `orchestrator/src/conversation.rs` — the live streaming turn body (~2540-2724): construct the executor, drive it, persist results per-completion with an explicit assistant parent UUID. Add a `persist_message_to_jsonl_with_parent` variant.
- **Modify** `orchestrator/src/turn_loop.rs` — change the unknown-tool and error-path `tool_result` content strings in `dispatch_tool_uses_tracked` to the `<tool_use_error>…</tool_use_error>` wrapper (shared by batched + streaming paths).
- **Modify** `orchestrator/src/lib.rs` — `mod streaming_executor;`.
- **Modify** `tool-api/src/context.rs` — add `pub cancel: Option<tokio_util::sync::CancellationToken>` to `ToolUseContext` (Phase 1 wires it into the ctx; Phase 2 makes Bash observe it).

---

## Task 1: `<tool_use_error>` wrapper for unknown tool (shared path)

This is the smallest faithful byte-fix and unblocks the executor's unknown-tool case sharing one code path.

**Files:**
- Modify: `orchestrator/src/turn_loop.rs:1273-1280` (the `find_by_name` `None` arm)
- Test: `orchestrator/src/turn_loop.rs` (in-file `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Add to the existing turn_loop tests module:

```rust
#[tokio::test]
async fn unknown_tool_emits_tool_use_error_wrapper() {
    let orch = crate::test_support::orch_with_empty_registry().await;
    let id = protocol::ToolUseId::new();
    let calls = vec![(id, "NoSuchTool".to_string(), serde_json::json!({}), None)];
    let (blocks, _prevent, _injected, _mods) =
        dispatch_tool_uses_tracked(&orch, &calls).await.unwrap();
    let ContentBlock::ToolResult { content, is_error, .. } = &blocks[0] else {
        panic!("expected ToolResult");
    };
    assert!(*is_error);
    assert_eq!(
        content,
        "<tool_use_error>Error: No such tool available: NoSuchTool</tool_use_error>"
    );
}
```

If `test_support::orch_with_empty_registry` does not exist, reuse the nearest existing constructor used by sibling tests in this module (grep `dispatch_tool_uses_tracked` test callers) and register zero tools.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p orchestrator unknown_tool_emits_tool_use_error_wrapper`
Expected: FAIL — content is `Error: tool not found: NoSuchTool`, not the wrapper.

- [ ] **Step 3: Implement the wrapper**

In `turn_loop.rs`, the `find_by_name` `None` arm (currently `format!("Error: tool not found: {name}")`):

```rust
let Some(tool_handle) = orch.tools.find_by_name(name) else {
    let result_block = ContentBlock::ToolResult {
        tool_use_id: *tool_use_id,
        content: fold_pre_context(format!(
            "<tool_use_error>Error: No such tool available: {name}</tool_use_error>"
        )),
        is_error: true,
        provider_tool_use_id: provider_id.clone(),
    };
    // ...existing emit_tool_result + results.push + continue unchanged...
```

Match claude-code's exact string: `Error: No such tool available: <name>` (NOT "tool not found").

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p orchestrator unknown_tool_emits_tool_use_error_wrapper`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator/src/turn_loop.rs
git commit -m "feat(turn-loop): wrap unknown-tool error in <tool_use_error> (claude-code parity)"
```

---

## Task 2: `<tool_use_error>` wrapper for tool-execution errors (shared path)

claude-code wraps a tool's own error result in `<tool_use_error>` too (via each tool's `mapToolResultToToolResultBlockParam`). LingXi currently emits bare `Error: <msg>`.

**Files:**
- Modify: `orchestrator/src/turn_loop.rs:1384` (the `Err(ToolError)` → `ToolResult` arm) and `:1689` (the generic error fallback). Grep `is_error: true` in this file to find every `ToolResult` the dispatch path can emit for a *tool failure* (hook-block and permission-deny are separate UX strings — see Step 3).
- Test: `orchestrator/src/turn_loop.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn tool_runtime_error_emits_tool_use_error_wrapper() {
    let orch = crate::test_support::orch_with_failing_tool("Boom", "kaboom").await;
    let id = protocol::ToolUseId::new();
    let calls = vec![(id, "Boom".to_string(), serde_json::json!({}), None)];
    let (blocks, ..) = dispatch_tool_uses_tracked(&orch, &calls).await.unwrap();
    let ContentBlock::ToolResult { content, is_error, .. } = &blocks[0] else {
        panic!("expected ToolResult");
    };
    assert!(*is_error);
    assert!(
        content.starts_with("<tool_use_error>") && content.ends_with("</tool_use_error>"),
        "got: {content}"
    );
    assert!(content.contains("kaboom"));
}
```

Use the existing in-file failing-tool stub (grep this module for a `Tool` impl whose `call` returns `Err(ToolError::Internal(...))`; several exist around `turn_loop.rs:1786/2627/2700`). Add a `test_support` helper if none is reusable.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p orchestrator tool_runtime_error_emits_tool_use_error_wrapper`
Expected: FAIL — content is `Error: kaboom` with no wrapper.

- [ ] **Step 3: Implement a single wrapping helper and apply it**

Add near the top of the dispatch function body:

```rust
/// Wrap a tool-failure message in claude-code's `<tool_use_error>` envelope.
/// Hook-block and permission-deny strings are intentionally NOT wrapped here —
/// claude-code surfaces those via separate UX paths; only genuine tool errors
/// (unknown tool, validation, ToolError from `call`) get the envelope.
fn tool_use_error(msg: &str) -> String {
    format!("<tool_use_error>{msg}</tool_use_error>")
}
```

At the `Err(e)` arm of the `tool_handle.call(...)` result and the generic fallback (`turn_loop.rs:1384`/`:1689`), change the content from `format!("Error: {e}")` to `fold_pre_context(tool_use_error(&format!("Error: {e}")))`. Keep the pre-context fold OUTSIDE the wrapper exactly as claude-code keeps hint text outside (verify against `toolExecution.ts:664`; if claude-code wraps first then appends hint, swap the order to match — the test in Step 1 only asserts the wrapper, so add an assertion matching whichever order claude-code uses before finalizing).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p orchestrator tool_runtime_error_emits_tool_use_error_wrapper`
Expected: PASS

- [ ] **Step 5: Run the full turn-loop suite to catch fixture drift**

Run: `cargo test -p orchestrator turn_loop`
Expected: PASS. If locked VCR/byte fixtures assert the old bare-`Error:` strings, update them in the same commit — the wrapper is the new correct byte shape.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/orchestrator/src/turn_loop.rs
git commit -m "feat(turn-loop): wrap tool-execution errors in <tool_use_error> (claude-code parity)"
```

---

## Task 3: Add cancel slot to `ToolUseContext`

Phase 1 only *carries* the token; Phase 2 makes Bash observe it. Adding it now lets the executor construct per-tool child tokens without a later signature churn.

**Files:**
- Modify: `tool-api/src/context.rs:20-43` (struct) and `:55-74` (`model_seed`)
- Test: `tool-api/src/context.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn tool_use_context_carries_optional_cancel_token() {
    let mut ctx = ToolUseContext::model_seed("opus".into());
    assert!(ctx.cancel.is_none());
    ctx.cancel = Some(tokio_util::sync::CancellationToken::new());
    assert!(ctx.cancel.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tool-api tool_use_context_carries_optional_cancel_token`
Expected: FAIL — no field `cancel` on `ToolUseContext`.

- [ ] **Step 3: Add the field**

In the struct (after `subagent_registry`):

```rust
    /// Per-call cancellation token. Fired by the streaming executor when a
    /// sibling Bash tool errors (or the user interrupts). Phase 1 carries it;
    /// the Bash/subprocess tools observe it in Phase 2 to kill in-flight work.
    /// `None` for batched/legacy call sites.
    pub cancel: Option<tokio_util::sync::CancellationToken>,
```

In `model_seed`, add `cancel: None,` to the struct literal. Add `tokio_util` to `tool-api/Cargo.toml` if not already a dependency (it is used elsewhere in the workspace; confirm with `cargo tree -p tool-api | grep tokio-util`).

Then fix every `ToolUseContext { ... }` literal the compiler flags by adding `cancel: None,` (grep `ToolUseContext {` across the workspace — `test_support.rs` builders and any orchestrator construction sites).

- [ ] **Step 4: Run test + workspace build**

Run: `cargo test -p tool-api tool_use_context_carries_optional_cancel_token && cargo build --workspace`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/tool-api/src/context.rs lingxi-code/tool-api/Cargo.toml
git commit -m "feat(tool-api): add optional cancel token to ToolUseContext (executor groundwork)"
```

---

## Task 4: `StreamingToolExecutor` skeleton + `TrackedTool` state machine

**Files:**
- Create: `orchestrator/src/streaming_executor.rs`
- Modify: `orchestrator/src/lib.rs` (add `mod streaming_executor;`)
- Test: `orchestrator/src/streaming_executor.rs` (in-file)

- [ ] **Step 1: Write the module skeleton + a status-transition unit test**

Create `orchestrator/src/streaming_executor.rs`:

```rust
//! Faithful port of claude-code's `StreamingToolExecutor`
//! (`services/tools/StreamingToolExecutor.ts`). Schedules tool execution as
//! `tool_use` blocks stream in, under concurrency control, buffering results
//! for emission in *received* order. Single-task: all tool futures borrow
//! `&ConversationOrchestrator` and are polled on one `FuturesUnordered`, so no
//! `'static`/spawn is required.

use crate::conversation::ConversationOrchestrator;
use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use tool_api::ContextModifier;

/// Lifecycle of one tracked tool, mirroring TS `ToolStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolStatus {
    Queued,
    Executing,
    Completed,
    Yielded,
}

/// One `tool_use` block under management. `assistant_id` is the UUID of the
/// assistant message that requested this call — it becomes the JSONL
/// `parentUuid` of the result (TS `sourceToolAssistantUUID`).
pub(crate) struct TrackedTool {
    pub(crate) id: ToolUseId,
    pub(crate) name: String,
    pub(crate) input: serde_json::Value,
    pub(crate) provider_id: Option<String>,
    pub(crate) assistant_id: MessageId,
    pub(crate) status: ToolStatus,
    pub(crate) is_concurrency_safe: bool,
    /// The result block once `Completed` (the unknown-tool case fills it
    /// synchronously at `add_tool` time).
    pub(crate) result: Option<ContentBlock>,
    /// Tool-injected follow-up messages (SKILLEXEC.3) + context modifiers,
    /// threaded through unchanged from `dispatch_tool_uses_tracked`.
    pub(crate) injected: Vec<(ConversationMessage, ToolUseId)>,
    pub(crate) modifiers: Vec<ContextModifier>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_enum_roundtrips() {
        assert_eq!(ToolStatus::Queued, ToolStatus::Queued);
        assert_ne!(ToolStatus::Queued, ToolStatus::Yielded);
    }
}
```

Add `mod streaming_executor;` to `orchestrator/src/lib.rs` (next to `mod streaming_loop;`).

- [ ] **Step 2: Run it**

Run: `cargo test -p orchestrator streaming_executor::tests::status_enum_roundtrips`
Expected: PASS (compiles + trivially passes).

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/orchestrator/src/streaming_executor.rs lingxi-code/orchestrator/src/lib.rs
git commit -m "feat(orchestrator): StreamingToolExecutor skeleton + TrackedTool state machine"
```

---

## Task 5: Synthetic error-message builders

Pure functions, fully unit-testable, no orchestrator needed.

**Files:**
- Modify: `orchestrator/src/streaming_executor.rs`
- Test: same file

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn sibling_error_synthetic_with_description() {
    let block = synthetic_error_block(
        ToolUseId::new(),
        AbortReason::SiblingError,
        Some("Bash(rm -rf /tmp/x)"),
    );
    let ContentBlock::ToolResult { content, is_error, .. } = block else {
        panic!()
    };
    assert!(is_error);
    assert_eq!(
        content,
        "<tool_use_error>Cancelled: parallel tool call Bash(rm -rf /tmp/x) errored</tool_use_error>"
    );
}

#[test]
fn sibling_error_synthetic_without_description() {
    let block = synthetic_error_block(ToolUseId::new(), AbortReason::SiblingError, None);
    let ContentBlock::ToolResult { content, .. } = block else { panic!() };
    assert_eq!(
        content,
        "<tool_use_error>Cancelled: parallel tool call errored</tool_use_error>"
    );
}

#[test]
fn streaming_fallback_synthetic() {
    let block = synthetic_error_block(ToolUseId::new(), AbortReason::StreamingFallback, None);
    let ContentBlock::ToolResult { content, .. } = block else { panic!() };
    assert_eq!(
        content,
        "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>"
    );
}
```

(The `user_interrupted` case uses `REJECT_MESSAGE` + `withMemoryCorrectionHint`; port those constants/helpers in Phase 2 when interrupt is wired — Phase 1 covers sibling_error + streaming_fallback, which are reachable without the interrupt path. Add `AbortReason::UserInterrupted` to the enum now but leave its mapping `todo!()`-free by returning the desc-less sibling text as a placeholder ONLY if reached; gate it so no Phase-1 test exercises it.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p orchestrator streaming_executor`
Expected: FAIL — `synthetic_error_block`/`AbortReason` undefined.

- [ ] **Step 3: Implement**

```rust
/// Why a tracked tool is being cancelled (TS `getAbortReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbortReason {
    SiblingError,
    UserInterrupted,
    StreamingFallback,
}

/// Build the synthetic `tool_result` for a cancelled tool (TS
/// `createSyntheticErrorMessage`). `provider_tool_use_id` is left `None` —
/// the caller copies the tracked tool's `provider_id` in before persisting.
pub(crate) fn synthetic_error_block(
    tool_use_id: ToolUseId,
    reason: AbortReason,
    errored_desc: Option<&str>,
) -> ContentBlock {
    let content = match reason {
        AbortReason::StreamingFallback => {
            "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>"
                .to_string()
        }
        AbortReason::UserInterrupted => {
            // Phase 2 swaps in REJECT_MESSAGE + memory-correction hint.
            "<tool_use_error>User rejected tool use</tool_use_error>".to_string()
        }
        AbortReason::SiblingError => match errored_desc {
            Some(desc) => format!(
                "<tool_use_error>Cancelled: parallel tool call {desc} errored</tool_use_error>"
            ),
            None => "<tool_use_error>Cancelled: parallel tool call errored</tool_use_error>"
                .to_string(),
        },
    };
    ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error: true,
        provider_tool_use_id: None,
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator streaming_executor`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator/src/streaming_executor.rs
git commit -m "feat(orchestrator): synthetic tool-cancel message builders (claude-code parity)"
```

---

## Task 6: `add_tool` — classification + unknown-tool short-circuit

**Files:**
- Modify: `orchestrator/src/streaming_executor.rs`
- Test: same file

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn add_unknown_tool_completes_immediately_with_wrapper() {
    let orch = crate::test_support::orch_with_empty_registry().await;
    let mut exec = StreamingToolExecutor::new(&orch);
    let assistant = MessageId::new();
    exec.add_tool(ToolUseId::new(), "Nope".into(), serde_json::json!({}), None, assistant);
    let t = &exec.tools[0];
    assert_eq!(t.status, ToolStatus::Completed);
    assert!(t.is_concurrency_safe); // TS marks unknown tool concurrency-safe
    let ContentBlock::ToolResult { content, is_error, .. } = t.result.as_ref().unwrap() else {
        panic!()
    };
    assert!(*is_error);
    assert_eq!(
        content,
        "<tool_use_error>Error: No such tool available: Nope</tool_use_error>"
    );
}

#[tokio::test]
async fn add_known_tool_is_queued_with_classification() {
    let orch = crate::test_support::orch_with_readonly_tool("Read").await;
    let mut exec = StreamingToolExecutor::new(&orch);
    exec.add_tool(ToolUseId::new(), "Read".into(), serde_json::json!({"file_path":"/x"}), None, MessageId::new());
    let t = &exec.tools[0];
    assert_eq!(t.status, ToolStatus::Queued);
    assert!(t.is_concurrency_safe);
}
```

Reuse/add `test_support::orch_with_readonly_tool` returning an orchestrator whose registry has one tool with `is_concurrency_safe(_) -> true`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p orchestrator streaming_executor`
Expected: FAIL — `StreamingToolExecutor::new`/`add_tool` undefined.

- [ ] **Step 3: Implement `new` + `add_tool`**

```rust
use futures::stream::FuturesUnordered;

pub(crate) struct StreamingToolExecutor<'a> {
    orch: &'a ConversationOrchestrator,
    pub(crate) tools: Vec<TrackedTool>,
    /// In-flight tool futures keyed by their index in `tools`.
    inflight: FuturesUnordered<
        std::pin::Pin<Box<dyn std::future::Future<Output = (usize, DispatchOutcome)> + 'a>>,
    >,
    has_errored: bool,
    errored_desc: Option<String>,
    discarded: bool,
}

/// Result of one `dispatch_tool_uses_tracked` call routed through the executor.
type DispatchOutcome = Result<
    (ContentBlock, Vec<(ConversationMessage, ToolUseId)>, Vec<ContextModifier>),
    crate::error::OrchestratorError,
>;

impl<'a> StreamingToolExecutor<'a> {
    pub(crate) fn new(orch: &'a ConversationOrchestrator) -> Self {
        Self {
            orch,
            tools: Vec::new(),
            inflight: FuturesUnordered::new(),
            has_errored: false,
            errored_desc: None,
            discarded: false,
        }
    }

    pub(crate) fn add_tool(
        &mut self,
        id: ToolUseId,
        name: String,
        input: serde_json::Value,
        provider_id: Option<String>,
        assistant_id: MessageId,
    ) {
        match self.orch.tools.find_by_name(&name) {
            None => {
                let mut block = synthetic_unknown_tool(id, &name);
                if let ContentBlock::ToolResult { provider_tool_use_id, .. } = &mut block {
                    *provider_tool_use_id = provider_id.clone();
                }
                self.tools.push(TrackedTool {
                    id, name, input, provider_id, assistant_id,
                    status: ToolStatus::Completed,
                    is_concurrency_safe: true,
                    result: Some(block),
                    injected: Vec::new(),
                    modifiers: Vec::new(),
                });
            }
            Some(tool) => {
                let safe = tool.is_concurrency_safe(&input);
                self.tools.push(TrackedTool {
                    id, name, input, provider_id, assistant_id,
                    status: ToolStatus::Queued,
                    is_concurrency_safe: safe,
                    result: None,
                    injected: Vec::new(),
                    modifiers: Vec::new(),
                });
            }
        }
    }
}

fn synthetic_unknown_tool(id: ToolUseId, name: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id,
        content: format!("<tool_use_error>Error: No such tool available: {name}</tool_use_error>"),
        is_error: true,
        provider_tool_use_id: None,
    }
}
```

Note: TS parses the input against the schema before classifying; on parse failure `isConcurrencySafe = false`. LingXi's `is_concurrency_safe` takes raw `&Value`, so schema-parse failure isn't separately modeled — acceptable for Phase 1 (the tool's own `is_concurrency_safe` returns its conservative default on malformed input). Document this divergence in a code comment.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator streaming_executor`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator/src/streaming_executor.rs
git commit -m "feat(orchestrator): executor add_tool with classification + unknown-tool short-circuit"
```

---

## Task 7: Concurrency gate + `process_queue` (start eligible tools)

**Files:**
- Modify: `orchestrator/src/streaming_executor.rs`
- Test: same file

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn can_execute_respects_concurrency_safety() {
    // pure-logic test on a helper that takes (executing_safe_flags, candidate_safe)
    assert!(can_execute(&[], true));              // nothing running → ok
    assert!(can_execute(&[], false));             // nothing running → ok
    assert!(can_execute(&[true, true], true));    // all safe + candidate safe → ok
    assert!(!can_execute(&[true], false));        // candidate unsafe, something running → no
    assert!(!can_execute(&[false], true));        // an unsafe tool running → no
}
```

```rust
#[tokio::test]
async fn process_queue_starts_safe_tools_and_barriers_on_unsafe() {
    let orch = crate::test_support::orch_with_mixed_tools().await; // "Read"=safe, "Bash"=unsafe
    let mut exec = StreamingToolExecutor::new(&orch);
    let a = MessageId::new();
    exec.add_tool(ToolUseId::new(), "Read".into(), serde_json::json!({"file_path":"/a"}), None, a);
    exec.add_tool(ToolUseId::new(), "Bash".into(), serde_json::json!({"command":"true"}), None, a);
    exec.add_tool(ToolUseId::new(), "Read".into(), serde_json::json!({"file_path":"/b"}), None, a);
    exec.process_queue();
    // First Read starts; Bash is a barrier (can't start while Read executing);
    // the second Read is BEHIND the barrier and must NOT start out of order.
    assert_eq!(exec.tools[0].status, ToolStatus::Executing);
    assert_eq!(exec.tools[1].status, ToolStatus::Queued);
    assert_eq!(exec.tools[2].status, ToolStatus::Queued);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p orchestrator streaming_executor`
Expected: FAIL — `can_execute`/`process_queue` undefined.

- [ ] **Step 3: Implement**

```rust
/// TS `canExecuteTool`: no tool executing, OR candidate safe AND all executing safe.
pub(crate) fn can_execute(executing_safe_flags: &[bool], candidate_safe: bool) -> bool {
    executing_safe_flags.is_empty()
        || (candidate_safe && executing_safe_flags.iter().all(|&s| s))
}

impl<'a> StreamingToolExecutor<'a> {
    /// TS `processQueue`: walk the queue in order, start eligible tools, and
    /// `break` at the first non-concurrency-safe tool that cannot start yet
    /// (preserve exclusive-tool ordering). Pushes started tools' futures onto
    /// `inflight`.
    pub(crate) fn process_queue(&mut self) {
        loop {
            let executing_flags: Vec<bool> = self
                .tools
                .iter()
                .filter(|t| t.status == ToolStatus::Executing)
                .map(|t| t.is_concurrency_safe)
                .collect();

            // Find the next queued tool to consider, in order.
            let Some(idx) = self.tools.iter().position(|t| t.status == ToolStatus::Queued) else {
                return;
            };
            // Walk in order: if an earlier queued *unsafe* tool can't run, stop.
            let mut started_any = false;
            for i in 0..self.tools.len() {
                if self.tools[i].status != ToolStatus::Queued {
                    continue;
                }
                let safe = self.tools[i].is_concurrency_safe;
                if can_execute(&executing_flags, safe) {
                    self.start_tool(i);
                    started_any = true;
                    break; // re-evaluate executing set after each start
                } else if !safe {
                    return; // exclusive barrier — preserve order
                }
            }
            let _ = idx;
            if !started_any {
                return;
            }
        }
    }
}
```

`start_tool(i)` is implemented in Task 8; for this task stub it to just set status `Executing` so the ordering test passes without real dispatch:

```rust
fn start_tool(&mut self, i: usize) {
    self.tools[i].status = ToolStatus::Executing;
    // Real future push lands in Task 8.
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator streaming_executor`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator/src/streaming_executor.rs
git commit -m "feat(orchestrator): executor concurrency gate + ordered process_queue"
```

---

## Task 8: `start_tool` — real dispatch future with sibling-abort race

**Files:**
- Modify: `orchestrator/src/streaming_executor.rs`
- Test: same file (integration-style with a fake slow tool)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn bash_error_cancels_a_queued_sibling() {
    // Registry: "Bash" returns an is_error result; "Read" is concurrency-safe.
    let orch = crate::test_support::orch_with_bash_error_and_read().await;
    let a = MessageId::new();
    let mut exec = StreamingToolExecutor::new(&orch);
    // Bash first (unsafe → exclusive), Read second (queued behind the barrier).
    exec.add_tool(ToolUseId::new(), "Bash".into(), serde_json::json!({"command":"false"}), None, a);
    exec.add_tool(ToolUseId::new(), "Read".into(), serde_json::json!({"file_path":"/x"}), None, a);
    let results = exec.run_to_completion().await.unwrap();
    // Bash result is its real error; Read got the synthetic sibling-cancel.
    let read = &results[1];
    let ContentBlock::ToolResult { content, is_error, .. } = read else { panic!() };
    assert!(*is_error);
    assert!(content.contains("Cancelled: parallel tool call"), "got: {content}");
}
```

`run_to_completion` is a Phase-1 test convenience that loops `process_queue` + drains `inflight` until all tools are `Completed`, returning results in tool order. (The production loop in Task 9/10 interleaves with the SSE stream; this test exercises the executor in isolation.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p orchestrator bash_error_cancels_a_queued_sibling`
Expected: FAIL — `start_tool` doesn't dispatch; `run_to_completion` undefined.

- [ ] **Step 3: Implement real dispatch + abort accounting**

Replace the stub `start_tool` with one that pushes a borrowed future calling the existing per-tool pipeline, and add the completion-handling that sets `has_errored` for Bash errors:

```rust
use crate::turn_loop::dispatch_tool_uses_tracked;

const BASH_TOOL_NAME: &str = "Bash";

impl<'a> StreamingToolExecutor<'a> {
    fn start_tool(&mut self, i: usize) {
        self.tools[i].status = ToolStatus::Executing;
        let id = self.tools[i].id;
        let name = self.tools[i].name.clone();
        let input = self.tools[i].input.clone();
        let provider_id = self.tools[i].provider_id.clone();
        let orch = self.orch;
        let fut = async move {
            let single = vec![(id, name, input, provider_id)];
            let outcome: DispatchOutcome = match dispatch_tool_uses_tracked(orch, &single).await {
                Ok((mut blocks, _prevent, injected, modifiers)) => blocks
                    .pop()
                    .map(|b| (b, injected, modifiers))
                    .ok_or_else(|| crate::error::OrchestratorError::StreamingProtocol(
                        format!("empty dispatch for tool {i}"),
                    )),
                Err(e) => Err(e),
            };
            (i, outcome)
        };
        self.inflight.push(Box::pin(fut));
    }

    /// Drain one completed in-flight future, recording its result + Bash-error
    /// cascade. Returns the completed tool index, or `None` if `inflight` empty.
    async fn drain_one(&mut self) -> Option<usize> {
        use futures::StreamExt;
        let (i, outcome) = self.inflight.next().await?;
        match outcome {
            Ok((mut block, injected, modifiers)) => {
                let is_err = matches!(&block, ContentBlock::ToolResult { is_error, .. } if *is_error);
                if is_err && self.tools[i].name == BASH_TOOL_NAME {
                    self.has_errored = true;
                    self.errored_desc = Some(tool_description(&self.tools[i]));
                }
                // Copy provider id onto the result for egress replay.
                if let ContentBlock::ToolResult { provider_tool_use_id, .. } = &mut block {
                    *provider_tool_use_id = self.tools[i].provider_id.clone();
                }
                self.tools[i].result = Some(block);
                self.tools[i].injected = injected;
                self.tools[i].modifiers = modifiers;
                self.tools[i].status = ToolStatus::Completed;
            }
            Err(e) => {
                // A hard orchestrator error fails the whole turn (matches the
                // batched path's fail-fast). Surface it by stashing + returning.
                self.tools[i].status = ToolStatus::Completed;
                self.tools[i].result = Some(ContentBlock::ToolResult {
                    tool_use_id: self.tools[i].id,
                    content: format!("<tool_use_error>Error: {e}</tool_use_error>"),
                    is_error: true,
                    provider_tool_use_id: self.tools[i].provider_id.clone(),
                });
            }
        }
        Some(i)
    }

    /// Convert any still-`Queued`/`Executing` tool to a synthetic-cancel result
    /// once `has_errored`/`discarded` is set (TS `getAbortReason` on next poll).
    fn apply_abort_to_pending(&mut self) {
        let reason = if self.discarded {
            Some(AbortReason::StreamingFallback)
        } else if self.has_errored {
            Some(AbortReason::SiblingError)
        } else {
            None
        };
        let Some(reason) = reason else { return };
        let desc = self.errored_desc.clone();
        for t in &mut self.tools {
            if matches!(t.status, ToolStatus::Queued) && t.result.is_none() {
                let mut block = synthetic_error_block(t.id, reason, desc.as_deref());
                if let ContentBlock::ToolResult { provider_tool_use_id, .. } = &mut block {
                    *provider_tool_use_id = t.provider_id.clone();
                }
                t.result = Some(block);
                t.status = ToolStatus::Completed;
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn run_to_completion(
        &mut self,
    ) -> Result<Vec<ContentBlock>, crate::error::OrchestratorError> {
        loop {
            self.apply_abort_to_pending();
            self.process_queue();
            if self.inflight.is_empty() {
                break;
            }
            self.drain_one().await;
        }
        Ok(self.tools.iter().map(|t| t.result.clone().unwrap()).collect())
    }
}

fn tool_description(t: &TrackedTool) -> String {
    let summary = t
        .input
        .get("command")
        .or_else(|| t.input.get("file_path"))
        .or_else(|| t.input.get("pattern"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if summary.is_empty() {
        t.name.clone()
    } else {
        let truncated = if summary.chars().count() > 40 {
            format!("{}\u{2026}", summary.chars().take(40).collect::<String>())
        } else {
            summary.to_string()
        };
        format!("{}({})", t.name, truncated)
    }
}
```

Note on Phase-1 fidelity: an *in-flight* (already `Executing`) sibling is not interrupted here — it runs to completion and keeps its real result. claude-code interrupts it via the sibling controller on the generator's next yield. Phase 2 adds the per-tool `CancellationToken` race so an in-flight sibling also yields the synthetic error. The `bash_error_cancels_a_queued_sibling` test uses a *queued* sibling (behind the exclusive Bash barrier), which Phase 1 handles correctly. Add a `// PHASE-2:` comment marking the in-flight gap.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator streaming_executor`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator/src/streaming_executor.rs
git commit -m "feat(orchestrator): executor real dispatch + Bash sibling-error cascade (queued siblings)"
```

---

## Task 9: Ordered drain API for the live loop (`take_newly_completed`)

The production loop needs to drain *completed* results in received order between stream events, persisting each immediately. This mirrors TS `getCompletedResults`.

**Files:**
- Modify: `orchestrator/src/streaming_executor.rs`
- Test: same file

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn take_newly_completed_yields_in_received_order_and_marks_yielded() {
    let orch = crate::test_support::orch_with_readonly_tool("Read").await;
    let a = MessageId::new();
    let mut exec = StreamingToolExecutor::new(&orch);
    exec.add_tool(ToolUseId::new(), "Read".into(), serde_json::json!({"file_path":"/a"}), None, a);
    exec.add_tool(ToolUseId::new(), "Read".into(), serde_json::json!({"file_path":"/b"}), None, a);
    // Drive both to completion.
    exec.process_queue();
    while !exec.inflight_is_empty() { exec.drain_one_pub().await; }
    let first = exec.take_newly_completed(); // both completed, in order
    assert_eq!(first.len(), 2);
    let second = exec.take_newly_completed(); // nothing left
    assert!(second.is_empty());
}
```

Expose `inflight_is_empty`/`drain_one_pub` as `#[cfg(test)]` thin wrappers if `inflight`/`drain_one` are private.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p orchestrator take_newly_completed`
Expected: FAIL — `take_newly_completed` undefined.

- [ ] **Step 3: Implement**

```rust
/// One drained result ready to persist, carrying everything the live loop
/// needs to build + parent the user message.
pub(crate) struct DrainedResult {
    pub(crate) block: ContentBlock,
    pub(crate) assistant_id: MessageId,
    pub(crate) injected: Vec<(ConversationMessage, ToolUseId)>,
    pub(crate) modifiers: Vec<ContextModifier>,
}

impl<'a> StreamingToolExecutor<'a> {
    /// TS `getCompletedResults`: walk tools in order, yield each `Completed`
    /// (not yet `Yielded`) tool's result, mark it `Yielded`, and STOP at an
    /// `Executing` non-concurrency-safe tool (don't emit past an exclusive
    /// barrier that hasn't finished). Returns results in received order.
    pub(crate) fn take_newly_completed(&mut self) -> Vec<DrainedResult> {
        let mut out = Vec::new();
        for t in &mut self.tools {
            match t.status {
                ToolStatus::Completed => {
                    t.status = ToolStatus::Yielded;
                    out.push(DrainedResult {
                        block: t.result.clone().expect("completed tool has result"),
                        assistant_id: t.assistant_id,
                        injected: std::mem::take(&mut t.injected),
                        modifiers: std::mem::take(&mut t.modifiers),
                    });
                }
                ToolStatus::Yielded => continue,
                ToolStatus::Executing if !t.is_concurrency_safe => break,
                _ => {}
            }
        }
        out
    }

    pub(crate) fn has_unfinished(&self) -> bool {
        self.tools.iter().any(|t| t.status != ToolStatus::Yielded)
    }
    pub(crate) fn inflight_is_empty(&self) -> bool { self.inflight.is_empty() }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator streaming_executor`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator/src/streaming_executor.rs
git commit -m "feat(orchestrator): ordered take_newly_completed drain for the live loop"
```

---

## Task 10: Assistant-parented persistence (`persist_message_to_jsonl_with_parent`)

Each tool result must parent to its originating assistant UUID, not the linear `last_jsonl_uuid` chain.

**Files:**
- Modify: `orchestrator/src/conversation.rs:1569-1602`
- Test: `orchestrator/src/conversation.rs` (in-file, against a fake/in-memory JSONL writer)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn tool_result_parents_to_originating_assistant_uuid() {
    let (orch, writer_spy) = crate::test_support::orch_with_jsonl_spy().await;
    let assistant_uuid = "assistant-uuid-1".to_string();
    let user = ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: ToolUseId::new(),
            content: "ok".into(),
            is_error: false,
            provider_tool_use_id: None,
        }],
    };
    orch.persist_message_to_jsonl_with_parent(&user, Some(assistant_uuid.clone())).await;
    let last = writer_spy.last_appended().await;
    assert_eq!(last.parent_uuid.as_deref(), Some("assistant-uuid-1"));
}
```

The spy captures the `JsonlMessage` passed to `writer.append`. If a JSONL spy writer doesn't exist, add one in `test_support` implementing the same `append` trait the real writer uses.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p orchestrator tool_result_parents_to_originating_assistant_uuid`
Expected: FAIL — method undefined.

- [ ] **Step 3: Implement the parent-override variant**

Refactor `persist_message_to_jsonl` to delegate to a new method that accepts an explicit parent override. The override, when `Some`, is used as `parent_uuid` INSTEAD of `last_jsonl_uuid`, but the `last_jsonl_uuid` cache is STILL advanced to this message's uuid (so any subsequent non-overridden line chains correctly):

```rust
pub(crate) async fn persist_message_to_jsonl(&self, msg: &ConversationMessage) {
    self.persist_message_to_jsonl_with_parent(msg, None).await;
}

/// Persist with an optional explicit `parentUuid` override. The streaming
/// executor passes the originating assistant message's UUID so each tool
/// result parents to the assistant that requested it (TS
/// `sourceToolAssistantUUID`), rather than the linear `last_jsonl_uuid`
/// chain. The `last_jsonl_uuid` cache is still advanced to this line's UUID.
pub(crate) async fn persist_message_to_jsonl_with_parent(
    &self,
    msg: &ConversationMessage,
    parent_override: Option<String>,
) {
    let Some(writer) = self.jsonl_writer.as_ref() else { return };
    let (session_id_str, parent_uuid) = {
        let session_id = self.session.lock().await.session_id;
        let parent = match parent_override {
            Some(p) => Some(p),
            None => self.last_jsonl_uuid.lock().await.clone(),
        };
        (session_id.to_string(), parent)
    };
    let git_branch = self.resolve_git_branch().await;
    let entrypoint = Some(entrypoint_value());
    let prompt_id = self.prompt_id_for_message(msg).await;
    let jmsg = self.to_jsonl_message(
        msg, &session_id_str, parent_uuid, git_branch, entrypoint, prompt_id,
    );
    let uuid_for_chain = jmsg.uuid.clone();
    match writer.append(&jmsg).await {
        Ok(()) => {
            *self.last_jsonl_uuid.lock().await = Some(uuid_for_chain.clone());
            telemetry::emit_session_appended(&session_id_str, &uuid_for_chain);
        }
        Err(e) => {
            tracing::error!(error = %e, "jsonl writer append failed");
            telemetry::emit_session_corrupted(&session_id_str, &e.to_string());
        }
    }
}
```

Decision to verify against claude-code before finalizing: whether `last_jsonl_uuid` should advance to the assistant-parented result, or whether the NEXT assistant line parents off the last *result*. In claude-code each result parents off the assistant; the subsequent assistant turn parents off the last persisted line. Keeping the cache advancing to each result preserves that. Add a test asserting a second result in the same batch parents to the SAME assistant uuid (not to the first result) — see Task 11's integration test.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator tool_result_parents_to_originating_assistant_uuid`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator/src/conversation.rs
git commit -m "feat(orchestrator): persist_message_to_jsonl_with_parent for assistant-parented results"
```

---

## Task 11: Wire the executor into the live streaming loop

This is the integration task: replace `pump_stream`-buffers-then-`dispatch_tool_uses_concurrent` with mid-stream dispatch + ordered per-result persistence. The non-streaming 529 fallback keeps batch dispatch.

**Files:**
- Modify: `orchestrator/src/streaming_loop.rs` (`pump_stream` signature) and `orchestrator/src/conversation.rs:2540-2724` (the live streaming body)
- Test: `orchestrator/src/conversation.rs` (streaming integration test with a scripted SSE stream)

- [ ] **Step 1: Write the failing integration test**

Add a streaming integration test that scripts an SSE stream with two `tool_use` blocks (one safe Read, one Bash) and asserts: (a) results land in stream order in `session.history`'s tool_result user message(s); (b) each result's JSONL parent is the assistant uuid; (c) emit_tool_result fired for both. Model it on the nearest existing `run_turn_streaming` integration test (grep `run_turn_streaming` in conversation.rs tests). Assert separate user messages, one per result, each parented to the assistant.

```rust
#[tokio::test]
async fn streaming_dispatches_tools_midstream_and_parents_results() {
    let (orch, spy) = crate::test_support::streaming_orch_two_tools().await;
    orch.run_turn_streaming_with_cancel("hi".into(), CancellationToken::new()).await.unwrap();
    let appended = spy.all_appended().await;
    let results: Vec<_> = appended.iter()
        .filter(|m| m.is_tool_result_user())
        .collect();
    assert_eq!(results.len(), 2, "one user message per tool result");
    let assistant = appended.iter().find(|m| m.is_assistant()).unwrap();
    for r in results {
        assert_eq!(r.parent_uuid.as_deref(), Some(assistant.uuid.as_str()));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p orchestrator streaming_dispatches_tools_midstream_and_parents_results`
Expected: FAIL — current path emits a single batched user message chained off `last_jsonl_uuid`.

- [ ] **Step 3: Change `pump_stream` to drive the executor**

Change `pump_stream` to accept `&mut StreamingToolExecutor` and a callback (or return-channel) so that on `RouterAction::DispatchToolUse` it calls `executor.add_tool(...)` with the CURRENT assistant message id, and after each event calls `executor.process_queue()` + drains `take_newly_completed()` via a caller-supplied async sink. The cleanest faithful shape: inline the stream-drive into the streaming body rather than keeping `pump_stream` standalone. Concretely, in `conversation.rs` replace the `let pumped = ... pump_stream(...)` block with:

```rust
let assistant_id = MessageId::new();
// Persist the assistant message shell-first is NOT how claude-code works;
// the assistant message is finalized when the stream ends. But tool results
// must parent to assistant_id, so mint it up front and use it for BOTH the
// assistant line (persisted after stream end) and each result's parent.
let mut exec = StreamingToolExecutor::new(self);
let mut acc = BlockAccumulator::new();
let mut turn = PumpedTurn::default();
let mut pending: Vec<ConversationMessage> = Vec::new(); // results to persist after assistant

while let Some(item) = stream.next().await {
    let event = item.map_err(OrchestratorError::Streaming)?;
    // (capture MessageStart usage as today)
    let action = dispatch_event(event, &mut acc, &self.output).await
        .map_err(|e| OrchestratorError::StreamingProtocol(e.to_string()))?;
    match action {
        RouterAction::DispatchToolUse { id, name, input, provider_id } => {
            turn.tool_uses.push(ObservedToolUse { id, name: name.clone(), input: input.clone(), provider_id: provider_id.clone() });
            exec.add_tool(id, name, input, provider_id, assistant_id);
            exec.process_queue();
        }
        RouterAction::AppendAssistantBlock(b) => turn.assistant_blocks.push(b),
        RouterAction::RecordStopReason { .. } | RouterAction::RecordUsage { .. } => { /* fold usage as today */ }
        RouterAction::EndOfStream => break,
        RouterAction::Continue => {}
    }
    // Drive any tools that finished while we were reading the next event.
    drive_executor_once(&mut exec).await; // process_queue + drain_one if ready (non-blocking try)
}
```

Because `take_newly_completed` must persist results with the assistant parent, and the assistant message must be persisted BEFORE its child results, the persistence order is: finalize+persist the assistant message (assembled from `turn.assistant_blocks` + `turn.tool_uses`), THEN drain the executor to completion, persisting each result (and its injected messages) parented to `assistant_id`. So after the stream loop:

```rust
// Assemble + persist the assistant message (as today, using assistant_id).
let assistant_msg = /* same assembly as conversation.rs:2654-2668, with id = assistant_id */;
{ let mut s = self.session.lock().await; s.history.push(assistant_msg.clone()); }
self.persist_message_to_jsonl(&assistant_msg).await;
let assistant_uuid = self.last_jsonl_uuid.lock().await.clone();

// Drain remaining tools to completion, persisting each result in received order.
let mut all_modifiers: Vec<ContextModifier> = Vec::new();
while exec.has_unfinished() {
    exec.apply_abort_to_pending();
    exec.process_queue();
    if !exec.inflight_is_empty() {
        exec.drain_one().await;
    }
    for drained in exec.take_newly_completed() {
        let user_msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![drained.block],
        };
        { let mut s = self.session.lock().await; s.history.push(user_msg.clone()); }
        self.persist_message_to_jsonl_with_parent(&user_msg, assistant_uuid.clone()).await;
        for (m, tuid) in drained.injected {
            { let mut s = self.session.lock().await; s.history.push(m.clone()); s.injected_message_sources.insert(m.id(), tuid); }
            self.persist_message_to_jsonl(&m).await;
        }
        all_modifiers.extend(drained.modifiers);
    }
}
crate::turn_loop::apply_model_context_modifiers(self, all_modifiers).await;
```

Key faithfulness points to preserve from the current code:
- The assistant message assembly (text/thinking blocks + tool_use blocks) is unchanged (conversation.rs:2654-2668).
- `emit_tool_result` still fires in completion order — it already fires inside `dispatch_tool_uses_tracked`, so it's emitted when `drain_one` resolves a future, i.e. completion order. ✓
- `injected_message_sources` side-table + `apply_model_context_modifiers` semantics are preserved (just moved to the drain loop).
- The 529 non-streaming fallback path (conversation.rs:2565-2601) is UNCHANGED — it builds `pumped_from_fallback` and must still go through the OLD batched dispatch (it has no SSE stream / executor). Keep a `dispatch_tool_uses_concurrent`-equivalent for that branch, OR feed the fallback's `tool_uses` into a fresh executor and `run_to_completion` (preferred — one code path). If fed to the executor, mint a single assistant message and parent results to it identically.

- [ ] **Step 4: Run the integration test + full streaming suite**

Run: `cargo test -p orchestrator streaming`
Expected: PASS. Fix any locked streaming VCR fixtures that asserted the single-batched-user-message shape — the new per-result shape is the correct parity bytes; regenerate/adjust them in this commit.

- [ ] **Step 5: Delete the superseded `dispatch_tool_uses_concurrent`**

If the fallback now uses the executor, remove `streaming_loop::dispatch_tool_uses_concurrent` and its `IndexedDispatchResult` alias. Run `cargo build -p orchestrator` to confirm no callers remain.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/orchestrator/src/conversation.rs lingxi-code/orchestrator/src/streaming_loop.rs
git commit -m "feat(orchestrator): drive StreamingToolExecutor mid-stream with assistant-parented results"
```

---

## Task 12: Full-workspace regression + parity fixture sweep

**Files:**
- Test: workspace-wide

- [ ] **Step 1: Run the full test suite**

Run: `CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace`
Expected: PASS (the memory note flags this volume builds near-full RAM — keep debug symbols off).

- [ ] **Step 2: Diff a recorded streaming conversation against claude-code shape**

Capture a streaming session JSONL (use an existing harness fixture under `lingxi-code/test-harness/`) with ≥2 parallel tools and one Bash error, and manually verify against the reference shape:
- each `tool_result` line's `parentUuid` == the assistant line's `uuid`
- one user line per tool result (not one batched line)
- error results carry `<tool_use_error>…</tool_use_error>`
- a Bash error produced `Cancelled: parallel tool call …` for the queued sibling

- [ ] **Step 3: Commit any fixture updates**

```bash
git add -A
git commit -m "test(orchestrator): refresh streaming parity fixtures for executor topology"
```

---

## Self-Review Notes (carried for the executor)

- **Spec coverage:** Mid-stream dispatch (Task 11), concurrency gate + ordering (Task 7), Bash sibling abort for queued siblings (Task 8), `<tool_use_error>` wrappers (Tasks 1/2/5/6), unknown-tool wrapper (Tasks 1/6), assistant-parented topology (Tasks 10/11), discard/fallback synthetic (Task 5 + Task 11 fallback note). **Deferred by design:** in-flight sibling subprocess kill (Phase 2), `user_interrupted`/REJECT_MESSAGE exact text + memory-correction hint (Phase 2), tool-id canonicalization (Phase 3).
- **Type consistency:** `StreamingToolExecutor<'a>`, `TrackedTool`, `ToolStatus`, `AbortReason`, `DrainedResult`, `synthetic_error_block`, `can_execute`, `take_newly_completed`, `process_queue`, `drain_one`, `has_unfinished`, `apply_abort_to_pending`, `persist_message_to_jsonl_with_parent` — names used consistently across Tasks 4-11.
- **Known divergences documented in code:** schema-parse-failure → concurrency-safe classification (Task 6); in-flight sibling not interrupted (Task 8 PHASE-2 comment).
