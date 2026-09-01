# M6-08 Engine Wiring 3 — `force_compact` Real Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `OrchestratorHandle::force_compact` no-op stub with a real call into `lingxi_compaction::CompactionOrchestrator`, so that `/compact` in both the v0.6.0 stdio REPL and the M6-02+ TUI reports real `messages_before → messages_after` numbers and the orchestrator's history is actually compacted in place.

**Architecture:** `ConversationOrchestrator` grows an `Arc<CompactionOrchestrator>` field (defaults via `CompactionOrchestrator::new(autocompact_threshold)` — same threshold M3 uses). `force_compact()` snapshots the current `session.history`, runs `compactor.process_iteration(history, 0).await`, swaps the result back under the same `session` lock, appends a `SystemTextMessage("[Compacted N → M messages]")` to the history (so the next turn carries the boundary marker), and returns a `CompactionSummary { messages_before, messages_after, bytes_saved }`. Cancellation is racing the `process_iteration` future against a `CancellationToken` via `tokio::select!`. The CLI `build_runtime` constructs the `CompactionOrchestrator` once (same default config M3 ships) and shares it with the orchestrator via a new `with_compaction` builder. The TUI's `app.rs` translates the `CompactionCompleted` orchestrator event into a `SystemTextMessage` push onto the scrollback buffer — proper `CompactBoundaryMessage` lands in M7.

**Tech Stack:** Rust 2021, `lingxi-compaction = { path = "../compaction" }` (M3-05), `tokio-util = "0.7"` (CancellationToken — already in workspace), `async-trait = "0.1"`, `lingxi-platform_api::CompactionSummary` (locked since M5-02).

---

## File Structure

**Modify:**
- `lingxi-code/crates/orchestrator/src/conversation.rs` — add `Arc<CompactionOrchestrator>` field + `with_compaction` builder; emit `CompactionCompleted` event via `OutputStream`.
- `lingxi-code/crates/orchestrator/src/handle_impl.rs` — `force_compact` real body: snapshot, run, swap, append marker, return summary.
- `lingxi-code/crates/orchestrator/src/error.rs` — extend `OrchestratorError` with `Compaction(lingxi_compaction::CompactionError)` and `CompactionCancelled` variants (keeps the trait surface `HandleError::ActionFailed(String)` unchanged — projects errors to a string).
- `lingxi-code/crates/orchestrator/src/test_support.rs` — `MockOrchestratorHandle` setter `set_compact_history_delta(before, after, bytes_saved)` already exists from M5-10; verify the production handle now produces non-zero deltas in the integration test.
- `lingxi-code/crates/cli/src/init.rs` — construct `CompactionOrchestrator` once; pass via `.with_compaction(compactor)` builder.
- `lingxi-code/crates/commands/src/builtin/compact.rs` — no behavioural change; verify the existing `"Compacted: {before} → {after} messages ({bytes} bytes saved)."` template renders real numbers (handler tests already cover the template; add a smoke test that exercises the real orchestrator).
- `lingxi-code/crates/tui/src/app.rs` — handle `CompactionCompleted` event: push `SystemTextMessage` `"[Compacted N → M messages]"` to scrollback.
- `lingxi-code/crates/tui/src/components/messages/mod.rs` — add `SystemText` variant + minimal renderer.
- `lingxi-code/crates/test-harness/src/parity/fixtures/parity_orchestrator_turn_loop.json` — extend with `compact_scenario`.
- `lingxi-code/crates/test-harness/tests/parity_orchestrator.rs` — drive the new compact scenario.

**Create:**
- `lingxi-code/crates/orchestrator/tests/force_compact_real.rs` — behavior tests for real wiring (50 msg → reduced, failure unchanged, cancel unchanged, stress 5×).
- `lingxi-code/crates/tui/src/components/messages/system_text.rs` — minimal system-text renderer (dim gray).

**Key types (locked):**
- Trait surface: `CompactionSummary { messages_before: u32, messages_after: u32, bytes_saved: u64 }` — unchanged from M5-02; **no `summary_id`** in v0.7.0 (deferred to M7 when `CompactBoundaryMessage` ships with a real summary content reference).
- New `OrchestratorError::Compaction(lingxi_compaction::CompactionError)` and `OrchestratorError::CompactionCancelled` — projected to `HandleError::ActionFailed("compaction failed: <reason>")` / `"compaction cancelled"`.

**Deviation from prompt's "LOCKED TYPES" line:** The prompt listed `CompactionOutcome { messages_before, messages_after, summary_id: SummaryId }` and `OrchestratorHandle::force_compact -> Result<CompactionOutcome, CompactError>`. The **actual** trait surface in `lingxi-platform_api::orchestrator::OrchestratorHandle` (locked since M5-02 and shipped in v0.6.0) is `force_compact() -> Result<CompactionSummary, HandleError>` where `CompactionSummary { messages_before: u32, messages_after: u32, bytes_saved: u64 }` — no `summary_id`. This plan KEEPS the v0.6.0 trait surface unchanged (zero parity-fixture regression) and surfaces real numbers via the existing fields. A real summary identifier is M7's concern when `CompactBoundaryMessage` arrives.

---

## Task 0: Reverse-engineer the M3 Compactor API and lock the integration contract

**Files:**
- Read: `lingxi-code/crates/compaction/src/lib.rs`
- Read: `lingxi-code/crates/compaction/src/orchestrator.rs`
- Read: `lingxi-code/crates/compaction/src/autocompact.rs`
- Read: `lingxi-code/crates/orchestrator/src/handle_impl.rs:52-66`
- Read: `claude-code/src/commands/compact.ts` (or `claude-code/src/utils/compaction*`) — confirm the user-visible "Compacted N → M" template

- [ ] **Step 1: Confirm the M3 Compactor entry-point shape.**

Run:
```bash
rg -n "pub fn process_iteration|pub async fn compact" lingxi-code/crates/compaction/src/
rg -n "pub fn new" lingxi-code/crates/compaction/src/orchestrator.rs
```

Expected (locked here so subsequent tasks compile against the real signatures):

| Symbol | Signature |
|---|---|
| `CompactionOrchestrator::new` | `pub fn new(autocompact_threshold: u64) -> Self` |
| `CompactionOrchestrator::process_iteration` | `pub async fn process_iteration(&self, messages: Vec<ConversationMessage>, snip_tokens_freed_already: u64) -> Result<IterationCompactionResult, CompactionError>` |
| `IterationCompactionResult` | `{ messages: Vec<ConversationMessage>, layers_applied: Vec<CompactionLayer>, total_tokens_freed: u64 }` |
| `CompactionError` | enum `Api(ApiError) / MaxRetriesExceeded / NotApplicable / Internal(String)` |

Notes:
- `process_iteration` does NOT take a `CancellationToken`. Cancellation in `force_compact` is implemented by racing the future against the token inside `tokio::select!` at the orchestrator boundary; on cancel we drop the future and leave history unchanged.
- The default `Autocompactor` (no `with_forked_runner`) returns a **stub summary string** — `[stub-summary attempt=0; messages=N]` — but still collapses the messages vec to 1. Real LLM-driven summarization only fires when M7+ wires a `ForkedAgentRunner` + `CacheSafeParamsSlot` via `Autocompactor::with_forked_runner`. M6-08 ships with the default (stub) autocompact; the **history-reduction behaviour** is real and that's what the trait promises.

- [ ] **Step 2: Lock the user-visible literal.**

The existing handler in `crates/commands/src/builtin/compact.rs:42` renders:
```
"Compacted: {messages_before} → {messages_after} messages ({bytes_saved} bytes saved)."
```

(Note the colon after `Compacted` and the trailing period — locked from M5-10.) The prompt's "Compacted N → M messages" is the **claude-code** form; the **LingXi** lock is the variant above (extra "({bytes} bytes saved)." suffix). This plan keeps the LingXi form — diverging from the prompt by design (M5-10 locked it; M6-08 does not change command output literals).

- [ ] **Step 3: Lock the bytes_saved estimator.**

`bytes_saved` (locked since M5-02 as a UX estimate) is computed as:
```rust
let before_bytes: u64 = before.iter().map(|m| m.text_byte_size()).sum();
let after_bytes:  u64 = after.iter() .map(|m| m.text_byte_size()).sum();
let bytes_saved = before_bytes.saturating_sub(after_bytes);
```
where `text_byte_size()` is the helper added in Task 1 (sums all text-content-block bytes, ignores tool_use input JSON byte size to match the "useful payload saved" semantics). Tool-use/tool-result are sized as their serialized JSON bytes.

- [ ] **Step 4: Lock the post-compact history shape.**

After `process_iteration` returns, the orchestrator's `session.history` is REPLACED with the IterationCompactionResult's `messages` vec, then a `SystemTextMessage` is APPENDED carrying the literal `"[Compacted N → M messages]"`. So a 50-message history compacts to typically `{summary_msg + boundary_marker}` = 2 messages. The next turn's system-prompt assembly sees the summary as part of `s.history`; no separate "summary context" injection path is needed (the summary IS in history).

- [ ] **Step 5: Decide cancellation semantics.**

If the supplied `CancellationToken` fires before `process_iteration` resolves:
1. Drop the future (it has no side effects on `session.history` until we explicitly write back).
2. Return `Err(HandleError::ActionFailed("compaction cancelled".into()))`.
3. `session.history` is unchanged.
4. `/compact` renders `"Could not compact: handle action failed: compaction cancelled"` (matches the existing failure template).

- [ ] **Step 6: Commit Task 0 notes inline (no code yet).**

```bash
git add docs/superpowers/plans/2026-05-28-m6-08-wire-force-compact.md
git commit -m "docs(m6-08): land force_compact integration contract notes"
```

---

## Task 1: Helper — `ConversationMessage::text_byte_size`

**Files:**
- Create: `lingxi-code/crates/protocol/src/message_size.rs`
- Modify: `lingxi-code/crates/protocol/src/lib.rs` (re-export trait)
- Test: `lingxi-code/crates/protocol/src/message_size.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test.**

Create `crates/protocol/src/message_size.rs`:

```rust
//! Tiny extension helper: estimate the "useful payload" byte size of a
//! [`ConversationMessage`]. Used by M6-08 to compute the `bytes_saved`
//! field of `CompactionSummary` (UX estimate only — the exact cost
//! accounting lives in `lingxi-cost`).

use crate::{ContentBlock, ConversationMessage};

/// Returns the sum, in bytes, of every text payload carried by `msg`.
///
/// Tool-use input JSON and tool-result content are sized as the
/// serialized JSON length (best-effort; falls back to 0 on serializer
/// error).
#[must_use]
pub fn text_byte_size(msg: &ConversationMessage) -> u64 {
    match msg {
        ConversationMessage::User { content, .. }
        | ConversationMessage::Assistant { content, .. } => {
            content.iter().map(content_block_size).sum()
        }
        ConversationMessage::System { content, .. } => content.len() as u64,
    }
}

fn content_block_size(b: &ContentBlock) -> u64 {
    match b {
        ContentBlock::Text { text, .. } => text.len() as u64,
        ContentBlock::ToolUse { input, .. } => serde_json::to_string(input)
            .map(|s| s.len() as u64)
            .unwrap_or(0),
        ContentBlock::ToolResult { content, .. } => serde_json::to_string(content)
            .map(|s| s.len() as u64)
            .unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentBlock, ConversationMessage, MessageId};

    #[test]
    fn user_text_message_returns_string_byte_length() {
        let m = ConversationMessage::user(MessageId::new(), "hello".into());
        assert_eq!(text_byte_size(&m), 5);
    }

    #[test]
    fn system_message_returns_content_length() {
        let m = ConversationMessage::System {
            id: MessageId::new(),
            content: "abc".into(),
        };
        assert_eq!(text_byte_size(&m), 3);
    }

    #[test]
    fn tool_use_block_sized_as_json() {
        let m = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "Read".into(),
                input: serde_json::json!({"path": "/a"}),
            }],
            stop_reason: Some("tool_use".into()),
        };
        // {"path":"/a"} → 12 bytes
        assert_eq!(text_byte_size(&m), 12);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails (compile error: module not in lib.rs).**

```bash
cargo test -p lingxi-protocol message_size:: --no-run
```
Expected: COMPILE FAIL — `unresolved module`.

- [ ] **Step 3: Re-export from `lib.rs`.**

In `crates/protocol/src/lib.rs`, find the existing module list and add:
```rust
pub mod message_size;
pub use message_size::text_byte_size;
```

- [ ] **Step 4: Run tests; expect pass.**

```bash
cargo test -p lingxi-protocol message_size::
```
Expected: 3 passed.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/protocol/src/message_size.rs lingxi-code/crates/protocol/src/lib.rs
git commit -m "feat(protocol): add text_byte_size helper for M6-08 bytes_saved estimate"
```

---

## Task 2: Extend `OrchestratorError` with compaction variants

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/error.rs`
- Test: same file (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

Append to `crates/orchestrator/src/error.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn compaction_variant_projects_to_string() {
    let api_err = lingxi_api_client::ApiError::Http(lingxi_platform_api::HttpError::Connection("nope".into()));
    let compact_err = lingxi_compaction::CompactionError::Api(api_err);
    let e = OrchestratorError::Compaction(compact_err);
    let s = e.to_string();
    assert!(s.contains("compaction"), "got: {s}");
}

#[test]
fn compaction_cancelled_renders() {
    let e = OrchestratorError::CompactionCancelled;
    assert_eq!(e.to_string(), "compaction cancelled");
}
```

- [ ] **Step 2: Run; expect FAIL (variants missing).**

```bash
cargo test -p lingxi-orchestrator error::tests::compaction_variant_projects_to_string error::tests::compaction_cancelled_renders --no-run
```
Expected: COMPILE FAIL.

- [ ] **Step 3: Add the variants.**

In `crates/orchestrator/src/error.rs` `#[derive(thiserror::Error, Debug)] pub enum OrchestratorError`, add:

```rust
    /// Compaction layer surfaced an error.
    #[error("compaction failed: {0}")]
    Compaction(#[from] lingxi_compaction::CompactionError),

    /// `force_compact` was cancelled mid-run by the supplied
    /// `CancellationToken`.
    #[error("compaction cancelled")]
    CompactionCancelled,
```

Ensure the `Cargo.toml` of `crates/orchestrator/` has `lingxi-compaction = { path = "../compaction" }` (add if missing — likely missing today; M3 was a sibling).

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator error::tests::
```
Expected: 2 new passing tests; no regressions in the rest of the module.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/orchestrator/src/error.rs lingxi-code/crates/orchestrator/Cargo.toml
git commit -m "feat(orchestrator): add Compaction + CompactionCancelled error variants"
```

---

## Task 3: Add `compaction: Option<Arc<CompactionOrchestrator>>` field + `with_compaction` builder

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/conversation.rs`
- Test: `lingxi-code/crates/orchestrator/src/conversation.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test.**

In `crates/orchestrator/src/conversation.rs` test module (or `crates/orchestrator/tests/conversation_compaction_field.rs` if there's no existing inline test mod):

```rust
#[tokio::test]
async fn with_compaction_builder_stores_compactor() {
    use crate::test_support::{noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider};
    use crate::config::OrchestratorConfig;
    use std::sync::Arc;
    use lingxi_compaction::CompactionOrchestrator;

    let api = Arc::new(MockApiClient::new());
    let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(), api, tools, hooks, perms, output, memory,
        std::env::temp_dir(),
    ).with_compaction(Arc::new(CompactionOrchestrator::new(150_000)));

    assert!(orch.compaction.is_some(), "compaction builder did not store the compactor");
}
```

- [ ] **Step 2: Run; expect FAIL (no field, no builder).**

```bash
cargo test -p lingxi-orchestrator with_compaction_builder --no-run
```
Expected: COMPILE FAIL.

- [ ] **Step 3: Add the field + builder.**

In `crates/orchestrator/src/conversation.rs`, add to the struct (after `should_exit`):

```rust
    /// Compaction engine (M3-05) wired by `with_compaction`. `None` when
    /// not configured — `force_compact` then falls back to the legacy
    /// no-op semantics. The CLI binary (M6-08 init.rs) always populates
    /// this.
    pub(crate) compaction: Option<Arc<lingxi_compaction::CompactionOrchestrator>>,
```

In both `new_with_streaming` and `new` constructors, add `compaction: None,` to the struct literal.

Add the builder method on the `impl ConversationOrchestrator` block (after `with_jsonl_writer`):

```rust
    /// Attach a [`lingxi_compaction::CompactionOrchestrator`] so
    /// `force_compact` performs real history compaction. Without this,
    /// `force_compact` retains the M5-10 no-op shape.
    #[must_use]
    pub fn with_compaction(
        mut self,
        compactor: Arc<lingxi_compaction::CompactionOrchestrator>,
    ) -> Self {
        self.compaction = Some(compactor);
        self
    }
```

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator with_compaction_builder
```
Expected: 1 passed.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/orchestrator/src/conversation.rs
git commit -m "feat(orchestrator): add Compaction field + with_compaction builder"
```

---

## Task 4: Add a `CompactionCompleted` `OutputEvent` variant

**Files:**
- Modify: `lingxi-code/crates/platform-api/src/output_stream.rs` (variant lives wherever `OutputEvent` is defined — check `crates/platform-api/src/`)
- Test: `lingxi-code/crates/orchestrator/src/test_support.rs` (`MockOutputStream` already records events)

- [ ] **Step 1: Locate the `OutputEvent` enum.**

```bash
rg -n "pub enum OutputEvent" lingxi-code/crates/platform-api/src/
```

Expected: one definition. Read the file and identify existing variant style (likely `Text { ... }`, `ToolCall { ... }`, `TurnEnd { ... }`).

- [ ] **Step 2: Write the failing test (a `MockOutputStream` emit helper).**

In `crates/orchestrator/src/test_support.rs` `#[cfg(test)] mod tests` (or wherever `MockOutputStream` tests live):

```rust
#[tokio::test]
async fn mock_output_records_compaction_completed() {
    let m = MockOutputStream::new();
    m.emit_compaction_completed(42, 7, 1234).await;
    let events = m.events().await;
    let last = events.last().expect("at least one event");
    assert!(matches!(last, lingxi_platform_api::OutputEvent::CompactionCompleted { messages_before: 42, messages_after: 7, bytes_saved: 1234 }), "got: {last:?}");
}
```

- [ ] **Step 3: Run; expect FAIL.**

```bash
cargo test -p lingxi-orchestrator mock_output_records_compaction_completed --no-run
```

- [ ] **Step 4: Add the variant and helper.**

In `crates/platform-api/src/output_stream.rs` (or wherever `OutputEvent` lives), add:

```rust
    /// Emitted once a successful `force_compact` finishes. M6-08.
    CompactionCompleted {
        /// Message count BEFORE compaction.
        messages_before: u32,
        /// Message count AFTER compaction (including the appended
        /// `[Compacted]` boundary marker).
        messages_after: u32,
        /// UX estimate of bytes freed.
        bytes_saved: u64,
    },
```

In `crates/platform-api/src/output_stream.rs` (the `OutputStream` trait), add an emitter method (default impl = no-op so existing impls compile):

```rust
    /// Emit a compaction-completed event. Default no-op for adapters
    /// that don't care (e.g. NDJSON sink may flush a one-line marker).
    async fn emit_compaction_completed(
        &self,
        _messages_before: u32,
        _messages_after: u32,
        _bytes_saved: u64,
    ) {}
```

In `crates/orchestrator/src/test_support.rs` `MockOutputStream`, override the method to push the event onto its internal `Vec<OutputEvent>`.

- [ ] **Step 5: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator mock_output_records_compaction_completed
```

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/crates/platform-api/src/output_stream.rs lingxi-code/crates/orchestrator/src/test_support.rs
git commit -m "feat(traits): add OutputEvent::CompactionCompleted variant + default emitter"
```

---

## Task 5: Implement the real `force_compact` body (happy path, no cancel yet)

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/handle_impl.rs:52-66`
- Test: `lingxi-code/crates/orchestrator/tests/force_compact_real.rs` (new file)

- [ ] **Step 1: Write the failing test (50 messages → reduced).**

Create `crates/orchestrator/tests/force_compact_real.rs`:

```rust
//! M6-08 — real `force_compact` wiring. Verifies that a 50-message
//! history compacts to fewer than 50 messages and the trait surface
//! reports real numbers.

use lingxi_compaction::CompactionOrchestrator;
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_protocol::{ConversationMessage, MessageId};
use lingxi_platform_api::OrchestratorHandle;
use std::sync::Arc;

fn make_orch() -> Arc<ConversationOrchestrator> {
    let api = Arc::new(MockApiClient::new());
    let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());
    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output,
        memory,
        std::env::temp_dir(),
    )
    .with_compaction(Arc::new(CompactionOrchestrator::new(1_000)));
    Arc::new(orch)
}

async fn seed_history(orch: &ConversationOrchestrator, n: usize) {
    let mut s = orch.session().lock().await;
    for i in 0..n {
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            format!("turn-{i} body padded with filler text to push token count up beyond autocompact threshold"),
        ));
    }
}

#[tokio::test]
async fn compacts_50_message_history() {
    let orch = make_orch();
    seed_history(&orch, 50).await;

    let summary = orch.force_compact().await.expect("force_compact ok");

    assert_eq!(summary.messages_before, 50);
    assert!(
        summary.messages_after < 50,
        "messages_after={} must be <50 to count as compacted",
        summary.messages_after
    );
}
```

- [ ] **Step 2: Run; expect FAIL.**

```bash
cargo test -p lingxi-orchestrator --test force_compact_real compacts_50_message_history --no-run
```
Expected: COMPILE OK, RUNTIME FAIL — `messages_after == 50` from the stub.

- [ ] **Step 3: Replace the `force_compact` body.**

In `crates/orchestrator/src/handle_impl.rs`, replace lines 52-66 with:

```rust
    async fn force_compact(&self) -> Result<CompactionSummary, HandleError> {
        use lingxi_protocol::{ConversationMessage, MessageId};

        let Some(compactor) = self.compaction.clone() else {
            // No compactor wired — fall back to the M5-10 no-op shape so
            // pre-M6-08 callers do not break.
            let s = self.session.lock().await;
            let count = u32::try_from(s.history.len()).unwrap_or(u32::MAX);
            return Ok(CompactionSummary {
                messages_before: count,
                messages_after: count,
                bytes_saved: 0,
            });
        };

        // Snapshot history (clone — we don't hold the lock across the
        // network call inside `process_iteration`).
        let history_before = {
            let s = self.session.lock().await;
            s.history.clone()
        };
        let messages_before = u32::try_from(history_before.len()).unwrap_or(u32::MAX);
        let bytes_before: u64 = history_before
            .iter()
            .map(lingxi_protocol::text_byte_size)
            .sum();

        // Run the 5-layer compactor.
        let result = compactor
            .process_iteration(history_before, 0)
            .await
            .map_err(|e| HandleError::ActionFailed(format!("compaction failed: {e}")))?;

        let mut history_after = result.messages;
        // Append the boundary marker so the TUI scrollback and the next
        // turn's system-prompt assembly see the compaction transition.
        let n_after_summary = history_after.len();
        let marker = ConversationMessage::System {
            id: MessageId::new(),
            content: format!(
                "[Compacted {messages_before} → {n} messages]",
                n = n_after_summary
            ),
        };
        history_after.push(marker);

        let messages_after = u32::try_from(history_after.len()).unwrap_or(u32::MAX);
        let bytes_after: u64 = history_after
            .iter()
            .map(lingxi_protocol::text_byte_size)
            .sum();
        let bytes_saved = bytes_before.saturating_sub(bytes_after);

        // Swap history under the same lock.
        {
            let mut s = self.session.lock().await;
            s.history = history_after;
        }

        // Best-effort emit so the TUI hears about it.
        self.output
            .emit_compaction_completed(messages_before, messages_after, bytes_saved)
            .await;

        Ok(CompactionSummary {
            messages_before,
            messages_after,
            bytes_saved,
        })
    }
```

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator --test force_compact_real compacts_50_message_history
```
Expected: 1 passed.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/orchestrator/src/handle_impl.rs lingxi-code/crates/orchestrator/tests/force_compact_real.rs
git commit -m "feat(orchestrator): wire real force_compact via CompactionOrchestrator"
```

---

## Task 6: Compaction failure path — history must stay unchanged

**Files:**
- Modify: `lingxi-code/crates/orchestrator/tests/force_compact_real.rs`
- (No production code changes — Task 5 already returns `Err(HandleError::ActionFailed)` without touching `session.history`.)

- [ ] **Step 1: Write the failing test.**

Append to `crates/orchestrator/tests/force_compact_real.rs`:

```rust
/// A custom `CompactionOrchestrator` that always errors via an
/// `Autocompactor` swapped for one whose `with_forked_runner` slot is
/// empty (produces `CompactionError::Internal`).
fn errored_compactor() -> Arc<CompactionOrchestrator> {
    use lingxi_compaction::autocompact::Autocompactor;
    use lingxi_compaction::microcompact::{Microcompactor, TimeBasedMCConfig};
    use lingxi_compaction::snip::SnipCompactor;
    use lingxi_sidequery::{CacheSafeParamsSlot, ForkedAgentRunner, SubagentSlotProvider};
    use std::sync::Arc;

    // A SlotProvider that panics if used — but the slot is empty so
    // `compact` will Internal-error before reaching the pool.
    struct NeverProvider;
    #[async_trait::async_trait]
    impl SubagentSlotProvider for NeverProvider {
        async fn acquire(&self) -> Result<lingxi_sidequery::SubagentSlotGuard, lingxi_sidequery::SubagentPoolError> {
            unreachable!("slot pool must not be touched in this test")
        }
    }

    let runner = Arc::new(ForkedAgentRunner::new(Arc::new(NeverProvider)));
    let slot = Arc::new(CacheSafeParamsSlot::new());
    // Threshold is intentionally tiny (1 token) so autocompact ALWAYS
    // fires, then errors on the empty slot.
    let mut orch = CompactionOrchestrator {
        snip: SnipCompactor,
        micro: Microcompactor { config: TimeBasedMCConfig::default() },
        auto: Autocompactor::with_forked_runner(runner, slot),
        autocompact_threshold: 1,
    };
    let _ = &mut orch;
    Arc::new(orch)
}

#[tokio::test]
async fn failure_leaves_history_unchanged() {
    let api = Arc::new(MockApiClient::new());
    let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api, tools, hooks, perms, output, memory,
        std::env::temp_dir(),
    ).with_compaction(errored_compactor());

    // Seed 5 messages.
    {
        let mut s = orch.session().lock().await;
        for i in 0..5 {
            s.history.push(ConversationMessage::user(MessageId::new(), format!("m{i}")));
        }
    }
    let len_before = orch.session().lock().await.history.len();

    let err = orch.force_compact().await.expect_err("must fail");
    let s = err.to_string();
    assert!(s.contains("compaction failed"), "got: {s}");

    let len_after = orch.session().lock().await.history.len();
    assert_eq!(len_before, len_after, "history must be untouched on failure");
}
```

- [ ] **Step 2: Run.**

```bash
cargo test -p lingxi-orchestrator --test force_compact_real failure_leaves_history_unchanged
```
Expected: PASS (Task 5's body already returns `Err` without writing back).

If the test compile fails because `SubagentSlotProvider`'s exact signature differs, run `rg -n "trait SubagentSlotProvider" lingxi-code/crates/sidequery/src/` and adjust the impl signature.

- [ ] **Step 3: Commit.**

```bash
git add lingxi-code/crates/orchestrator/tests/force_compact_real.rs
git commit -m "test(orchestrator): force_compact failure leaves history unchanged"
```

---

## Task 7: Cancellation — race `process_iteration` against a `CancellationToken`

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/handle_impl.rs::force_compact`
- Add: `force_compact_cancelable` helper method on `ConversationOrchestrator` (so the REPL/TUI can supply a token; the trait method calls into it with `CancellationToken::new()`)
- Test: `lingxi-code/crates/orchestrator/tests/force_compact_real.rs`

- [ ] **Step 1: Write the failing test.**

Append:

```rust
#[tokio::test]
async fn cancel_during_compaction_leaves_history_unchanged() {
    use tokio_util::sync::CancellationToken;

    let orch = make_orch();
    seed_history(&orch, 30).await;
    let len_before = orch.session().lock().await.history.len();

    let token = CancellationToken::new();
    token.cancel(); // already cancelled — first poll exits

    let err = orch
        .force_compact_with_cancel(token)
        .await
        .expect_err("cancelled must error");
    assert!(err.to_string().contains("cancelled"), "got: {err}");

    let len_after = orch.session().lock().await.history.len();
    assert_eq!(len_before, len_after);
}
```

- [ ] **Step 2: Run; expect FAIL (`force_compact_with_cancel` does not exist).**

```bash
cargo test -p lingxi-orchestrator --test force_compact_real cancel_during_compaction_leaves_history_unchanged --no-run
```

- [ ] **Step 3: Add `force_compact_with_cancel` on `ConversationOrchestrator` and refactor the trait method to call it with a fresh token.**

In `crates/orchestrator/src/handle_impl.rs`, replace the trait method body with a thin dispatcher and add the new pub method on the inherent impl. Add a `use tokio_util::sync::CancellationToken;` import.

Move the Task 5 body into a new `pub async fn force_compact_with_cancel(&self, cancel: CancellationToken) -> Result<CompactionSummary, HandleError>` on `impl ConversationOrchestrator { ... }` (NOT the trait impl), and surround the `compactor.process_iteration(...)` call with:

```rust
        let result = tokio::select! {
            r = compactor.process_iteration(history_before.clone(), 0) => r
                .map_err(|e| HandleError::ActionFailed(format!("compaction failed: {e}")))?,
            () = cancel.cancelled() => {
                return Err(HandleError::ActionFailed("compaction cancelled".into()));
            }
        };
```

(Move the `bytes_before` snapshot to BEFORE the select so we don't compute it on the cancel path.)

Then the trait method becomes:

```rust
    async fn force_compact(&self) -> Result<CompactionSummary, HandleError> {
        self.force_compact_with_cancel(tokio_util::sync::CancellationToken::new()).await
    }
```

- [ ] **Step 4: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator --test force_compact_real
```
Expected: 3 passed (compacts_50, failure_leaves, cancel_leaves).

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/orchestrator/src/handle_impl.rs lingxi-code/crates/orchestrator/tests/force_compact_real.rs
git commit -m "feat(orchestrator): force_compact_with_cancel — Ctrl-C-safe compaction"
```

---

## Task 8: Verify post-compact summary is part of next turn's context

**Files:**
- Test: `lingxi-code/crates/orchestrator/tests/force_compact_real.rs`

- [ ] **Step 1: Write the failing test.**

Append:

```rust
#[tokio::test]
async fn post_compact_summary_is_visible_to_next_turn() {
    let orch = make_orch();
    seed_history(&orch, 20).await;

    orch.force_compact().await.unwrap();

    // Inspect history: must contain the [Compacted ...] marker as the
    // last message, AND at least one preceding message that is the
    // summary (length-checked: compactor produces ≥1 message + our
    // marker).
    let s = orch.session().lock().await;
    let last = s.history.last().expect("history non-empty after compact");
    let lingxi_protocol::ConversationMessage::System { content, .. } = last else {
        panic!("expected System message; got {last:?}");
    };
    assert!(
        content.starts_with("[Compacted "),
        "marker missing; got: {content}"
    );

    // The summary message produced by the compactor sits before the
    // marker. With the M3 stub Autocompactor this is `[stub-summary
    // attempt=0; messages=20]`.
    assert!(
        s.history.len() >= 2,
        "expected ≥2 messages (summary + marker), got {}",
        s.history.len()
    );
}
```

- [ ] **Step 2: Run.**

```bash
cargo test -p lingxi-orchestrator --test force_compact_real post_compact_summary_is_visible_to_next_turn
```
Expected: PASS (Task 5's body already builds the marker correctly).

- [ ] **Step 3: Commit.**

```bash
git add lingxi-code/crates/orchestrator/tests/force_compact_real.rs
git commit -m "test(orchestrator): post-compact summary visible to next turn"
```

---

## Task 9: Stress smoke — 5 consecutive `force_compact` calls

**Files:**
- Test: `lingxi-code/crates/orchestrator/tests/force_compact_real.rs`

- [ ] **Step 1: Write the test.**

Append:

```rust
#[tokio::test]
async fn five_consecutive_force_compact_calls_do_not_explode() {
    let orch = make_orch();
    seed_history(&orch, 30).await;

    for i in 0..5 {
        let r = orch.force_compact().await;
        assert!(r.is_ok(), "iter {i} failed: {:?}", r.err());
    }

    // No assertion on final length — the stub autocompact collapses to
    // 1 + marker on each pass; we just confirm no panics / no leaks
    // (validated implicitly by `cargo test` finishing).
}
```

- [ ] **Step 2: Run; expect PASS.**

```bash
cargo test -p lingxi-orchestrator --test force_compact_real five_consecutive_force_compact_calls_do_not_explode
```

- [ ] **Step 3: Commit.**

```bash
git add lingxi-code/crates/orchestrator/tests/force_compact_real.rs
git commit -m "test(orchestrator): 5× force_compact stress smoke"
```

---

## Task 10: Wire `CompactionOrchestrator` in `lingxi-cli::init`

**Files:**
- Modify: `lingxi-code/crates/cli/src/init.rs`
- Modify: `lingxi-code/crates/cli/Cargo.toml` (add `lingxi-compaction = { path = "../compaction" }`)
- Test: existing `build_runtime_with_defaults` (verify post-wire the orchestrator carries the compactor)

- [ ] **Step 1: Extend the existing test.**

In `crates/cli/src/init.rs` `#[cfg(test)] mod tests`, replace the `build_runtime_with_defaults` body's tail:

```rust
        let r = build_runtime(&argv, output).await;
        let r = r.unwrap();
        // M6-08: compactor must be wired.
        assert!(
            r.orchestrator.compaction.is_some(),
            "build_runtime did not wire CompactionOrchestrator"
        );
```

(`compaction` was made `pub(crate)` in Task 3 — bump to `pub` so the integration test in `crates/cli` can see it, OR add a `pub fn has_compaction(&self) -> bool { self.compaction.is_some() }` accessor. Prefer the accessor — keeps visibility tight.)

- [ ] **Step 2: Run; expect FAIL.**

```bash
cargo test -p lingxi-cli build_runtime_with_defaults
```

- [ ] **Step 3: Add the `has_compaction` accessor.**

In `crates/orchestrator/src/conversation.rs`:

```rust
    /// Whether a [`lingxi_compaction::CompactionOrchestrator`] has been
    /// wired via [`Self::with_compaction`].
    #[must_use]
    pub fn has_compaction(&self) -> bool {
        self.compaction.is_some()
    }
```

Update the test in `crates/cli/src/init.rs` to use the accessor.

- [ ] **Step 4: Wire in `build_runtime`.**

In `crates/cli/src/init.rs`, after the existing orchestrator construction (line 145):

```rust
    // M6-08: Real compaction. Threshold defaults to 150_000 tokens —
    // matches M3's design lock for the Anthropic prod context window.
    // Default Autocompactor (no `with_forked_runner`) returns a stub
    // summary string; real LLM summarization lands in M7 when the
    // ForkedAgentRunner pool is wired.
    let compactor = Arc::new(lingxi_compaction::CompactionOrchestrator::new(150_000));
    let orch = Arc::new(
        ConversationOrchestrator::new(
            cfg, api_client, tools, hooks, perms, output, memory, cwd,
        )
        .with_compaction(compactor),
    );
```

(Replaces the existing `let orch = Arc::new(ConversationOrchestrator::new(...))` block.)

Add to `crates/cli/Cargo.toml` `[dependencies]`:
```toml
lingxi-compaction = { path = "../compaction" }
```

- [ ] **Step 5: Run; expect PASS.**

```bash
cargo test -p lingxi-cli build_runtime_with_defaults
```
Expected: PASS, `has_compaction == true`.

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/crates/cli/src/init.rs lingxi-code/crates/cli/Cargo.toml lingxi-code/crates/orchestrator/src/conversation.rs
git commit -m "feat(cli): wire CompactionOrchestrator into build_runtime"
```

---

## Task 11: TUI — render `[Compacted]` boundary in scrollback

**Files:**
- Create: `lingxi-code/crates/tui/src/components/messages/system_text.rs`
- Modify: `lingxi-code/crates/tui/src/components/messages/mod.rs`
- Modify: `lingxi-code/crates/tui/src/app.rs` (handle `CompactionCompleted` event → push SystemText)
- Test: `lingxi-code/crates/tui/tests/behavior_compact_marker.rs`

> **Prerequisite check:** if M6-01 / M6-02 are still in flight when this task starts and `crates/tui/src/components/messages/` does not yet exist, defer Task 11 to immediately after M6-02 lands (recorded in the dependency graph below). The handler and engine wiring (Tasks 1-10, 12) are independent and ship first.

- [ ] **Step 1: Write the failing behavior test.**

Create `crates/tui/tests/behavior_compact_marker.rs`:

```rust
//! M6-08 — TUI handles CompactionCompleted by appending a
//! `[Compacted N → M messages]` SystemTextMessage to scrollback.

use lingxi_platform_api::OutputEvent;
use lingxi_tui::test_support::TuiHarness;

#[tokio::test]
async fn compaction_completed_event_appends_marker_to_scrollback() {
    let mut h = TuiHarness::new();
    h.feed_orchestrator_event(OutputEvent::CompactionCompleted {
        messages_before: 50,
        messages_after: 2,
        bytes_saved: 12_345,
    }).await;

    let last = h.scrollback_last().expect("scrollback non-empty");
    let text = last.to_plain_text();
    assert!(text.contains("Compacted"), "got: {text}");
    assert!(text.contains("50"), "got: {text}");
    assert!(text.contains("2"), "got: {text}");
}
```

- [ ] **Step 2: Run; expect FAIL.**

```bash
cargo test -p lingxi-tui --test behavior_compact_marker --no-run
```

- [ ] **Step 3: Add the `SystemText` renderer.**

Create `crates/tui/src/components/messages/system_text.rs`:

```rust
//! Minimal system-text renderer. Used in M6 to render the
//! `[Compacted N → M messages]` boundary marker. Proper
//! `CompactBoundaryMessage` with a summary preview lands in M7.

use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct SystemTextProps {
    /// The text payload (already containing the literal brackets).
    pub text: String,
}

#[component]
pub fn SystemText(props: &SystemTextProps) -> impl Into<AnyElement<'static>> {
    element! {
        Box(padding_left: 1) {
            Text(content: props.text.clone(), color: Color::DarkGrey)
        }
    }
}
```

In `crates/tui/src/components/messages/mod.rs`, add:

```rust
pub mod system_text;
pub use system_text::{SystemText, SystemTextProps};

/// One rendered message in the scrollback.
pub enum RenderedMessage {
    UserText(String),
    AssistantText(String),
    AssistantToolUse(/* … existing fields … */),
    UserToolResult(/* … existing fields … */),
    SystemText(String),   // ← M6-08
}

impl RenderedMessage {
    pub fn to_plain_text(&self) -> String {
        match self {
            Self::UserText(s) | Self::AssistantText(s) | Self::SystemText(s) => s.clone(),
            Self::AssistantToolUse(_) => "<tool-use>".into(),
            Self::UserToolResult(_) => "<tool-result>".into(),
        }
    }
}
```

(If `RenderedMessage` already exists with different variants, append `SystemText(String)` and update the match arms in any existing `to_plain_text` / `render` dispatchers.)

- [ ] **Step 4: Handle `CompactionCompleted` in `app.rs`.**

In `crates/tui/src/app.rs` (the `match` over orchestrator events — M6-02 establishes this loop), add:

```rust
    OutputEvent::CompactionCompleted { messages_before, messages_after, .. } => {
        let marker = format!("[Compacted {messages_before} → {messages_after} messages]");
        app_state.scrollback.push(RenderedMessage::SystemText(marker));
    }
```

- [ ] **Step 5: Run; expect PASS.**

```bash
cargo test -p lingxi-tui --test behavior_compact_marker
```

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/crates/tui/src/components/messages/system_text.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/app.rs lingxi-code/crates/tui/tests/behavior_compact_marker.rs
git commit -m "feat(tui): render [Compacted] boundary marker in scrollback"
```

---

## Task 12: Verify `/compact` command surface (smoke + regression)

**Files:**
- Test: `lingxi-code/crates/commands/src/builtin/compact.rs` (extend existing `#[cfg(test)]`)

- [ ] **Step 1: Write the integration smoke.**

Append to `crates/commands/src/builtin/compact.rs` `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn real_orchestrator_renders_non_zero_delta() {
        use lingxi_compaction::CompactionOrchestrator;
        use lingxi_orchestrator::test_support::{
            noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
            StaticMemoryProvider,
        };
        use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
        use lingxi_protocol::{ConversationMessage, MessageId};
        use std::sync::Arc;

        let api = Arc::new(MockApiClient::new());
        let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());
        let hooks = noop_hook_executor();
        let perms = Arc::new(NoOpPermissionGate);
        let output = Arc::new(MockOutputStream::new());
        let memory = Arc::new(StaticMemoryProvider::empty());
        let orch = Arc::new(
            ConversationOrchestrator::new(
                OrchestratorConfig::default(),
                api, tools, hooks, perms, output, memory,
                std::env::temp_dir(),
            ).with_compaction(Arc::new(CompactionOrchestrator::new(1_000))),
        );
        {
            let mut s = orch.session().lock().await;
            for i in 0..40 {
                s.history.push(ConversationMessage::user(MessageId::new(), format!("msg-{i} body padding")));
            }
        }

        let handle: Arc<dyn lingxi_platform_api::OrchestratorHandle> = orch.clone();
        let h = CompactHandler::new(handle);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Compacted: 40 → "), "got: {s}");
                assert!(!s.contains("Compacted: 40 → 40 "), "no-op detected: {s}");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run.**

```bash
cargo test -p lingxi-commands compact:: real_orchestrator_renders_non_zero_delta
```
Expected: PASS — the `40 → N` where N < 40 confirms real wiring end-to-end through the trait, handler, and template.

- [ ] **Step 3: Commit.**

```bash
git add lingxi-code/crates/commands/src/builtin/compact.rs
git commit -m "test(commands): /compact renders non-zero delta against real orchestrator"
```

---

## Task 13: Extend `parity_orchestrator_turn_loop.json` with a compact scenario

**Files:**
- Modify: `lingxi-code/crates/test-harness/src/parity/fixtures/parity_orchestrator_turn_loop.json`
- Modify: `lingxi-code/crates/test-harness/tests/parity_orchestrator.rs`

- [ ] **Step 1: Append the scenario to the fixture.**

Edit `parity_orchestrator_turn_loop.json` `scenarios` array — add (between the existing entries):

```json
    {
      "name": "force_compact_50_messages",
      "seed_history_size": 50,
      "expected_messages_before": 50,
      "expected_messages_after_max": 49,
      "expected_marker_present": true,
      "expected_marker_prefix": "[Compacted 50 → "
    }
```

And add to `telemetry_invariants`:

```json
    "events_fired_on_force_compact": [
      "tengu_orchestrator_conversation_started"
    ]
```

(No new telemetry events are introduced in M6-08 — `CompactionCompleted` is an `OutputEvent`, not a `tengu_*` event. M6-09 may add `tengu_tui_compaction_*` if needed.)

- [ ] **Step 2: Write the driver assertion.**

Edit `crates/test-harness/tests/parity_orchestrator.rs`. Add a new test:

```rust
#[tokio::test]
async fn parity_force_compact_50_messages() {
    use lingxi_compaction::CompactionOrchestrator;
    use lingxi_orchestrator::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
    use lingxi_protocol::{ConversationMessage, MessageId};
    use lingxi_platform_api::OrchestratorHandle;
    use std::sync::Arc;

    let orch = Arc::new(
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new()),
            Arc::new(lingxi_tools::registry::ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_compaction(Arc::new(CompactionOrchestrator::new(1_000))),
    );

    // Seed 50 messages.
    {
        let mut s = orch.session().lock().await;
        for i in 0..50 {
            s.history.push(ConversationMessage::user(
                MessageId::new(),
                format!("turn-{i} padding"),
            ));
        }
    }
    let summary = orch.force_compact().await.unwrap();
    assert_eq!(summary.messages_before, 50);
    assert!(summary.messages_after < 50);

    let s = orch.session().lock().await;
    let last = s.history.last().unwrap();
    match last {
        ConversationMessage::System { content, .. } => {
            assert!(content.starts_with("[Compacted 50 → "), "got: {content}");
        }
        other => panic!("expected System marker; got {other:?}"),
    }
}
```

- [ ] **Step 3: Run.**

```bash
cargo test -p lingxi-test-harness --test parity_orchestrator parity_force_compact_50_messages
```
Expected: PASS.

- [ ] **Step 4: Run all v0.6.0 parity fixtures to confirm no regression.**

```bash
cargo test -p lingxi-test-harness
```
Expected: all green. If `parity_orchestrator_turn_loop` was previously passing under MockApiClient, it still does (the new scenario is additive).

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/crates/test-harness/src/parity/fixtures/parity_orchestrator_turn_loop.json lingxi-code/crates/test-harness/tests/parity_orchestrator.rs
git commit -m "test(parity): force_compact 50-message scenario in parity_orchestrator_turn_loop"
```

---

## Task 14: Compaction Gate + tag `m6.8`

**Files:**
- All files modified in Tasks 1-13
- Tag: `m6.8`

This is the **Compaction Gate** (Risk R4 in §4 of the parent spec). Last task before tag.

- [ ] **Step 1: Run the full Compaction Gate test set.**

```bash
cargo test -p lingxi-protocol message_size::
cargo test -p lingxi-orchestrator --test force_compact_real
cargo test -p lingxi-orchestrator error::tests::
cargo test -p lingxi-cli build_runtime_with_defaults
cargo test -p lingxi-commands compact::
cargo test -p lingxi-tui --test behavior_compact_marker
cargo test -p lingxi-test-harness --test parity_orchestrator
```

**Pass criteria:**
- `force_compact_real::compacts_50_message_history` → PASS, `messages_after < 50`
- `force_compact_real::failure_leaves_history_unchanged` → PASS
- `force_compact_real::cancel_during_compaction_leaves_history_unchanged` → PASS
- `force_compact_real::post_compact_summary_is_visible_to_next_turn` → PASS
- `force_compact_real::five_consecutive_force_compact_calls_do_not_explode` → PASS
- `commands::compact::real_orchestrator_renders_non_zero_delta` → PASS
- `test-harness::parity_force_compact_50_messages` → PASS
- ALL v0.6.0 parity fixtures (`parity_betas`, `parity_cost_events`, `parity_doctor_report`, `parity_file_tools`, `parity_help_render`, `parity_hooks_runtime`, `parity_init_template`, `parity_orchestrator`, `parity_session_jsonl`, `parity_slash_commands_102`) → all green

- [ ] **Step 2: Run the workspace verification gate.**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```

Known-allowed flakes (allowed to rerun once each):
- `rapid_writes_collapse_to_single_event`
- `writer_output_equals_single_turn_fixture`
- `streaming_concurrent_tools_test`

- [ ] **Step 3: Manual smoke (per spec §5.5 M6-08 row).**

Build the CLI binary:
```bash
cargo build -p lingxi-cli --release
```

Run a forced-history-load + compact sequence in the v0.6.0 stdio REPL (M6-02 TUI integration is verified by the behavior test in Task 11):

```bash
ANTHROPIC_API_KEY="" target/release/lingxi-cli --no-tui <<EOF
/clear
$(yes 'tell me a story' | head -50 | tr '\n' '\0' | xargs -0 -I{} echo "{}")
/compact
/exit
EOF
```

Expected: the `/compact` line prints `Compacted: 50 → N messages (M bytes saved).` with `N < 50` and `M > 0`. No panic. Terminal restored.

(API key empty is fine — `MockApiClient` is not in production binary, but `/compact` does NOT call out to the network; it only runs the local compactor. The user prompts that fail with 401 do still push the user messages onto history, which is what we want.)

- [ ] **Step 4: GATE DECISION.**

**If all of Steps 1, 2, 3 pass:** proceed to Step 5 (tag).

**If ANY of them fails AND it is rooted in M3 compaction code (autocompact panic, microcompact data corruption, history corruption, etc.):**
1. Revert all M6-08 commits: `git revert <task-1-sha>..<task-13-sha>` (or interactively if cleaner).
2. Restore the M5-10 no-op `force_compact` stub.
3. Document the gap in `docs/superpowers/releases/2026-XX-XX-v0.7.0.md` under a new "Deferred to v0.8.0 (M7)" section: *"M6-08 (real `force_compact`) failed the Compaction Gate due to <root cause>. `force_compact` remains the M5-10 no-op stub in v0.7.0. Real wiring tracked for M7 with prerequisite fix of <root cause>."*
4. Skip Step 5; M6-08 tag is **NOT** cut. Proceed to M6-09 with the gap documented.

**If the failure is rooted in M6-08 code itself (handler bug, builder bug, app.rs event handling):**
1. Fix in-line. Do NOT defer.
2. Re-run Steps 1-3 until green.
3. Then proceed to Step 5.

- [ ] **Step 5: Tag `m6.8`.**

```bash
git tag -a m6.8 -m "M6-08 — real force_compact via CompactionOrchestrator + TUI [Compacted] marker"
```

(NO push to remote. Same policy as v0.5.0 / v0.6.0 — see parent spec §6.4.)

- [ ] **Step 6: Verify the tag exists locally.**

```bash
git tag -l 'm6.8'
git show --stat m6.8 | head -20
```

- [ ] **Step 7: Final commit (only if Step 4 deferred — otherwise skip).**

If the gate failed and we reverted, commit the release-doc gap note:
```bash
git add docs/superpowers/releases/2026-XX-XX-v0.7.0.md
git commit -m "docs(v0.7.0): record M6-08 deferral — force_compact gap"
```

---

## Dependency Notes / Cross-task ordering

- **Tasks 1-10 + 12-13** are independent of the TUI crate and can be executed before M6-01/M6-02 if the executor decides to interleave M6-08 with earlier sub-plans. (The spec sequences M6-06 → M6-07 → M6-08 strictly, but the engine-wiring work itself does not depend on M6-01-05 — only Task 11 does.)
- **Task 11** depends on `crates/tui/src/app.rs` and `crates/tui/src/components/messages/` existing. Both come from M6-02. If M6-02 has not landed when M6-08 begins, defer Task 11 to immediately after M6-02 tags `m6.2`.
- **Task 14** is gated on Tasks 1-13 all passing AND the Compaction Gate (Step 4). If the gate fails, Task 14's tag step is skipped.

## Self-Review

**1. Spec coverage** (mapping the prompt's "WHAT M6-08 SHIPS" list to tasks):
| # | Prompt requirement | Task |
|---|---|---|
| 1 | `force_compact` stops being a no-op; calls into `Compactor::compact` | Tasks 5, 7 (process_iteration is the real entry; see Task 0 Step 1 for the API gap note) |
| 2 | `ConversationOrchestrator` constructs Compactor sharing ApiClient + CostTracker | Tasks 3, 10. NOTE: Compactor sharing of ApiClient/CostTracker is M7 work — the default Autocompactor does NOT take an ApiClient handle at construction. See "Unresolved" below. |
| 3 | `/compact` displays real numbers | Task 12 |
| 4 | Scrollback marker SystemTextMessage | Task 11 |
| 5 | Risk discipline / TDD | Tasks 6, 7, 8, 9 (failure, cancel, summary visible, 5× stress) |
| 6 | Compaction Gate (last task) | Task 14 |

**2. Placeholder scan:** searched for `TBD`, `TODO`, `implement later`, `fill in details`, `add appropriate`, `similar to Task` — none present.

**3. Type consistency:** all references use the **existing** `CompactionSummary` trait surface (`u32 / u32 / u64`). No `CompactionOutcome` or `summary_id` references appear in production code — these were called out as a deviation from the prompt in the "File Structure" preamble.

## Unresolved questions / surfaced M3 gaps

These should be raised to the user before execution begins (or during execution if blocking):

1. **Compactor sharing of ApiClient + CostTracker.** The prompt asks the `ConversationOrchestrator` to "construct Compactor with the same ApiClient + CostTracker (Compaction performs API calls for summarization)". The **actual** M3 `Autocompactor` does not accept an `ApiClient` or `CostTracker` at construction — it accepts a `ForkedAgentRunner` + `CacheSafeParamsSlot` via `with_forked_runner`. Until the `ForkedAgentRunner` graduates from its M1.14 stub (currently returns `"[forked-agent-stub]"`), real LLM-driven summarization cannot fire. M6-08 ships with the default Autocompactor (stub summary text, but **real** message-vec collapse — which is what the trait surface actually promises). M7 should wire `ForkedAgentRunner` + `CacheSafeParamsSlot` once the runner is real. **Recommendation:** explicitly document this in the v0.7.0 release notes — `/compact` reduces history but uses a stub summary body.
2. **`CompactionOutcome { summary_id }`.** The prompt's LOCKED TYPES include a `summary_id: SummaryId` field; none exists in the codebase, no `SummaryId` newtype is defined, and `CompactionResult` from `lingxi-compaction::autocompact` carries no id either. M6-08 plan keeps the v0.6.0 `CompactionSummary` shape unchanged. If `summary_id` is needed for M7's `CompactBoundaryMessage`, it should be added there alongside the new message renderer (not in M6-08, where it would force a trait surface change and break the v0.6.0 parity fixtures).
3. **Cancellation propagation depth.** `CompactionOrchestrator::process_iteration` does not accept a `CancellationToken`. M6-08 cancels by dropping the future at the orchestrator boundary (`tokio::select!`). Any side effects inside the dropped future (currently none in the stub path; potentially network calls in the M7 forked-agent path) are abandoned without cleanup. If M7 wires forked-agent compaction, `CompactionOrchestrator::process_iteration` should grow a `cancel: CancellationToken` parameter — and `force_compact_with_cancel` should pass it through rather than racing.
4. **`bytes_saved` semantics.** Plan locks this as the byte delta of `text_byte_size` over history. The exact M3 cost-accounting `total_tokens_freed` field (on `IterationCompactionResult`) is NOT used because it's a token count, not a byte count, and `CompactionSummary.bytes_saved` is documented as `u64 bytes`. If the user prefers token count there, the field semantics + name need a one-line edit in `lingxi-traits`.
5. **`HandleError` variant.** This plan keeps `HandleError::ActionFailed(String)` and projects compaction errors via `format!`. A dedicated `HandleError::Compaction { kind: CompactionErrorKind, reason: String }` variant would be cleaner but is a trait surface change — out of scope for M6-08 unless requested.

**End of M6-08 plan.**
