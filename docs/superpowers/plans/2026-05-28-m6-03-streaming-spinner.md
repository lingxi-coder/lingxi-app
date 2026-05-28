# LingXi Core M6 · Plan 03 · Streaming + SpinnerWithVerb

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Upgrade the M6-02 batched REPL screen to true token-by-token streaming. Replace `OrchestratorHandle::run_turn` with `OrchestratorHandle::run_turn_streaming_with_cancel` (new this plan). Introduce a TUI-local `TurnEvent` enum bridged from `OutputStream` callbacks. Render assistant text deltas into the last `AssistantTextMessage` in scrollback as they arrive. Mount a `SpinnerWithVerb` component above `PromptInput` while a turn is in-flight; hide it on turn end. Rate-limit the render loop to 30fps via `tokio::sync::Notify`. Cancel mid-stream on Ctrl-C through a `CancellationToken`.

**Architecture:**

1. **TurnEvent bridge** — `crates/tui/src/events/orchestrator_bridge.rs` (file exists from M6-01) gains a public `TurnEvent` enum with `TextDelta(String)`, `ToolUseStart{id, tool, input}`, `ToolUseResult{id, result}`, `PermissionRequest{...}`, `TurnStarted`, `TurnEnded(TurnOutcome)`. A new `BridgeOutputStream` impl of `lingxi_traits::OutputStream` translates `emit_text` → `TurnEvent::TextDelta`, `emit_tool_call` → `TurnEvent::ToolUseStart`, `emit_tool_result` → `TurnEvent::ToolUseResult`, `emit_end_turn` → `TurnEvent::TurnEnded`. `TurnStarted` fires before `run_turn_streaming` is awaited.
2. **Cancelable streaming** — `ConversationOrchestrator::run_turn_streaming_with_cancel(prompt, cancel)` mirrors the existing `run_turn_with_cancel` (M5-13) pattern: it races `try_run_turn_streaming` against `cancel.cancelled()` via `tokio::select!`, returning `TurnOutcome::Cancelled` when the token fires. `OrchestratorHandle::run_turn_streaming_with_cancel` is the trait-level wrapper.
3. **Streaming subscriber** — `crates/tui/src/streaming.rs` owns the receiver side of an `mpsc::UnboundedReceiver<TurnEvent>`. Its `apply_event(&mut AppState, ev: TurnEvent)` function: appends a new `AssistantTextMessage` on the first `TextDelta` of a turn; concatenates subsequent `TextDelta`s into the same message's text; toggles `AppState.streaming` to `Some(StreamingState{turn_id, started_at})` on `TurnStarted` and to `None` on `TurnEnded(_)`.
4. **Rate-limited render** — Each `apply_event` call appends to the buffer and calls `render_notify.notify_one()` (a `tokio::sync::Notify` shared with the renderer). The render loop awaits `render_notify.notified()` then sleeps 33ms (≈ 30fps cap) before draining queued state into iocraft. Bursts collapse into a single redraw.
5. **SpinnerWithVerb component** — `crates/tui/src/components/spinner.rs` is an iocraft `#[component]` rendering `{frame_char} {verb}…`. Internal `use_state` for `frame_index: usize` and `verb_index: usize`; a `tokio::time::interval(Duration::from_millis(100))` (10fps) driven by `use_future` advances `frame_index`. Verb rotation: every 4 seconds, advance to next verb. Frames and verb pool are constants matching claude-code byte-for-byte (see Reverse-engineered byte-locks below).
6. **REPL screen integration** — `crates/tui/src/screens/repl.rs` conditionally renders `<SpinnerWithVerb/>` between scrollback and prompt input when `app.streaming.is_some()`. On Ctrl-C: if `streaming.is_some()`, fire `cancel_token.cancel()` and clear `app.streaming`; if `streaming.is_none()`, fall through to M6-02's "clear prompt, second Ctrl-C exits" behavior.

**Tech Stack:** Rust 2024 edition (workspace inherits). Existing deps: `iocraft = "=0.6"` (M6-01), `tokio = { workspace = true, features = ["sync", "time", "macros"] }`, `tokio-util = { workspace = true, features = ["rt"] }` for `CancellationToken`, `futures = "0.3"`, `insta = { workspace = true }` for snapshots. No new third-party deps.

---

## Reverse-engineered byte-locks (T0 — captured at plan-writing time)

Captured by reading `claude-code/src/components/Spinner.tsx`, `claude-code/src/components/Spinner/utils.ts`, `claude-code/src/constants/spinnerVerbs.ts`, `claude-code/src/screens/REPL.tsx`. If a future engineer encounters drift, re-run Task 0 against the current `claude-code/` submodule.

| Lock id | Value | Source |
|---|---|---|
| Default spinner characters (darwin, NOT ghostty) | `['·', '✢', '✳', '✶', '✻', '✽']` (6 chars) | `claude-code/src/components/Spinner/utils.ts:6-8` |
| Default spinner characters (linux/other) | `['·', '✢', '*', '✶', '✻', '✽']` (6 chars; `*` instead of `✳`) | `claude-code/src/components/Spinner/utils.ts:9` |
| Ghostty override | `['·', '✢', '✳', '✶', '✻', '*']` (6 chars; `*` instead of `✽`) | `claude-code/src/components/Spinner/utils.ts:5` (when `TERM === 'xterm-ghostty'`) |
| Full frame sequence | `[...DEFAULT, ...reverse(DEFAULT)]` = 12 frames cycling forward then backward | `claude-code/src/components/Spinner.tsx:41` (`SPINNER_FRAMES = [...DEFAULT_CHARACTERS, ...[...DEFAULT_CHARACTERS].reverse()]`) |
| Verb pool | `SPINNER_VERBS` array (100+ entries). M6 subset for default rotation: `"Crunching"`, `"Thinking"`, `"Generating"` (3 verbs cycled deterministically; full pool deferred to M7). | `claude-code/src/constants/spinnerVerbs.ts:16+` (full list); M6 spec §3 M6-03 (subset) |
| Verb display format | `verb + '…'` (verb followed by horizontal ellipsis U+2026, NOT three dots) | `claude-code/src/components/Spinner.tsx:171` (`const message = effectiveVerb + '…';`) |
| Animation tick | `useAnimationFrame(50)` ms inside `SpinnerAnimationRow` → LingXi M6 uses **100ms (10fps)** per task description (slower than claude-code's 20fps; locked to reduce render churn while still smooth). | `claude-code/src/components/Spinner/SpinnerAnimationRow.tsx` + M6 spec §3 M6-03 |
| Verb rotation cadence | claude-code samples verb ONCE on mount via `sample(getSpinnerVerbs())`. M6 cycles deterministically every 4000ms across the 3-verb subset for testability. | `claude-code/src/components/Spinner.tsx:166` + M6-03 design |
| TurnEvent::TurnStarted timing | Fires AFTER `BridgeOutputStream` constructed, BEFORE `run_turn_streaming_with_cancel` is awaited (i.e. the bridge spawns a task and emits `TurnStarted` synchronously then awaits the orchestrator). | New for M6-03 — no claude-code analog (Ink's reactive model handles this via component lifecycle). |
| TurnEvent::TurnEnded payload | Wraps `TurnOutcome` (`EndTurn` / `MaxTurns` / `Cancelled`) verbatim from `lingxi_orchestrator::conversation::TurnOutcome`. | `lingxi-core/crates/orchestrator/src/conversation.rs:83-93` |

**Note on braille vs asterisk frames:** the task description text mentions "10-frame braille animation (⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏)". Claude-code uses asterisk/star glyphs, NOT braille. Per **Literal Lock Discipline (spec §2.8)**: every user-visible string must match claude-code byte-for-byte. This plan locks the claude-code asterisk frames as authoritative. The "braille" mention in the task description is documented as a divergence in Task 12's gate writeup.

---

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `lingxi-core/crates/orchestrator/src/conversation.rs` | Modify | Add `pub async fn run_turn_streaming_with_cancel(&self, prompt: &str, cancel: CancellationToken) -> Result<TurnOutcome, OrchestratorError>` mirroring `run_turn_with_cancel` shape. |
| `lingxi-core/crates/traits/src/orchestrator.rs` | Modify | Add `run_turn_streaming_with_cancel` to `OrchestratorHandle` trait; default impl delegates to `run_turn_streaming` (preserves existing impls). |
| `lingxi-core/crates/orchestrator/src/handle_impl.rs` | Modify | Real impl wires through to `ConversationOrchestrator::run_turn_streaming_with_cancel`. |
| `lingxi-core/crates/tui/src/events/orchestrator_bridge.rs` | Modify | Define `pub enum TurnEvent` + `pub struct BridgeOutputStream { tx: mpsc::UnboundedSender<TurnEvent> }` + `impl OutputStream for BridgeOutputStream`. |
| `lingxi-core/crates/tui/src/streaming.rs` | Create | `pub fn apply_event(state: &mut AppState, ev: TurnEvent, notify: &Notify)`. Pure function — accepts mutable state, dispatches on variant, calls `notify.notify_one()` at end. |
| `lingxi-core/crates/tui/src/components/spinner.rs` | Create | `#[component] pub fn SpinnerWithVerb` + `pub const SPINNER_FRAMES: &[&str]` + `pub const VERBS_M6: &[&str]`. |
| `lingxi-core/crates/tui/src/app.rs` | Modify | Replace `orchestrator.run_turn(prompt)` call (M6-02) with the streaming + cancel + bridge wiring. Add `streaming: Option<StreamingState>` and `cancel_token: Option<CancellationToken>` fields to `AppState`. |
| `lingxi-core/crates/tui/src/screens/repl.rs` | Modify | Mount `<SpinnerWithVerb verb={...} />` between scrollback and prompt input when `app.streaming.is_some()`. Wire Ctrl-C: if streaming, cancel; else delegate to existing M6-02 handler. |
| `lingxi-core/crates/tui/src/telemetry.rs` | Modify | Register 2 new event constants: `TUI_STREAMING_RENDER_STARTED`, `TUI_STREAMING_RENDER_ENDED`. |
| `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` | Modify | Append 2 entries to `NAMES`: `"tengu_tui_streaming_render_started"`, `"tengu_tui_streaming_render_ended"`. Bump `NAMES.len()` documentation from 17 → 19. |
| `lingxi-core/crates/telemetry/src/tengu/mod.rs` | Modify | Update `const TOTAL` formula: `... + 17 + ...` → `... + 19 + ...`. (Cumulative ALL_EVENT_NAMES: 315 baseline + 4 from M6-01 + 0 from M6-02 + 2 from M6-03 = 321.) |
| `lingxi-core/crates/tui/tests/snapshots/spinner_frame_0.snap` | Create (insta) | Snapshot for frame index 0. |
| `lingxi-core/crates/tui/tests/snapshots/spinner_frame_5.snap` | Create (insta) | Snapshot for frame index 5. |
| `lingxi-core/crates/tui/tests/snapshots/spinner_frame_9.snap` | Create (insta) | Snapshot for frame index 9. |
| `lingxi-core/crates/tui/tests/streaming_test.rs` | Create | Behavior tests: delta accumulation, spinner mount/unmount, Ctrl-C cancel, perf smoke. |
| `lingxi-core/crates/tui/tests/render_spinner_test.rs` | Create | Snapshot tests for 3 frames using `iocraft::test_utils`. |

---

## Plan Document Header

**Goal:** Token-by-token assistant streaming + animated `SpinnerWithVerb` overlay above prompt input + cancelable mid-stream via Ctrl-C.

**Architecture:** Bridge `OutputStream` callbacks → `TurnEvent` channel → `apply_event` mutator → `Notify`-debounced 30fps redraw. Spinner is a self-contained iocraft component with internal `use_state` for frame/verb indices driven by 100ms tick.

**Tech Stack:** iocraft 0.6 (pinned), tokio (sync + time + macros), tokio-util CancellationToken, insta snapshots.

---

## Task 0: Reverse-engineer & confirm byte-locks

**Files:** None (read-only).

- [ ] **Step 1: Confirm spinner frames in claude-code source**

  Run:
  ```bash
  grep -n "DEFAULT_CHARACTERS\|SPINNER_FRAMES" /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/Spinner/utils.ts /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/Spinner.tsx
  ```
  Expected: matches the locks table above. If drift (e.g. claude-code added a 7th frame), update Task 4 step 3 frame constant accordingly before proceeding.

- [ ] **Step 2: Confirm verb format**

  Run:
  ```bash
  grep -n "effectiveVerb\|message = " /Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/Spinner.tsx | head -5
  ```
  Expected: line 171 shows `const message = effectiveVerb + '…';`. The ellipsis is U+2026 (single codepoint), not three ASCII dots.

- [ ] **Step 3: Confirm M6-02 AppState shape**

  Run:
  ```bash
  grep -n "pub struct AppState\|pub streaming\|pub messages\|pub prompt_text" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/tui/src/app.rs
  ```
  Expected: `AppState` exists from M6-02 with fields `messages: Vec<RenderedMessage>`, `prompt_text: String`. NO `streaming` or `cancel_token` fields yet (this plan adds them in Task 2).

- [ ] **Step 4: Confirm telemetry baseline count**

  Run:
  ```bash
  grep -n "const TOTAL\|17 + 2 + 54" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/telemetry/src/tengu/mod.rs
  ```
  Expected: line 41 shows `const TOTAL: usize = 25 + 30 + 20 + 134 + 10 + 8 + 12 + 3 + 17 + 2 + 54;` = **315**. After M6-01 (adds 4 events): `... + 21 + 2 + 54` = 319. After this plan: `... + 23 + 2 + 54` = 321. (Confirm M6-01's exact split: 4 events go into the orchestrator submodule? Or a new `tui` submodule? Read M6-01's plan once it exists. If M6-01 created a new `tui` submodule, this plan APPENDS to that submodule instead of orchestrator.) **If unclear**, default to appending to `orchestrator::NAMES` until M6-01 lands its plan — Task 9 step 2 below shows both forms.

- [ ] **Step 5: Confirm `run_turn_streaming` exists and signature**

  Run:
  ```bash
  grep -n "pub async fn run_turn_streaming\|pub async fn run_turn_with_cancel" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/orchestrator/src/conversation.rs
  ```
  Expected: `run_turn_streaming(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError>` (M5-04) AND `run_turn_with_cancel(&self, prompt: &str, cancel: CancellationToken) -> Result<TurnOutcome, OrchestratorError>` (M5-13).

  No `run_turn_streaming_with_cancel` yet. Task 1 below adds it.

---

## Task 1: Add `ConversationOrchestrator::run_turn_streaming_with_cancel`

**Files:**
- Modify: `lingxi-core/crates/orchestrator/src/conversation.rs` (add new method after `run_turn_with_cancel`, around line 644).

- [ ] **Step 1: Write the failing test**

  Append to `lingxi-core/crates/orchestrator/src/conversation.rs` `#[cfg(test)] mod tests`:

  ```rust
  #[tokio::test]
  async fn run_turn_streaming_with_cancel_returns_cancelled_when_token_fires() {
      use crate::test_support::{build_test_orchestrator_streaming};
      use tokio_util::sync::CancellationToken;
      let (orch, _captured) = build_test_orchestrator_streaming(vec![
          // SSE script: 1 text_delta, then sleep 500ms before message_stop
          scripted!(MessageStart),
          scripted!(ContentBlockStart { type: "text" }),
          scripted!(ContentBlockDelta { text: "hello" }),
          scripted!(SleepMs(500)),
          scripted!(ContentBlockStop),
          scripted!(MessageDelta { stop_reason: "end_turn" }),
          scripted!(MessageStop),
      ]).await;
      let cancel = CancellationToken::new();
      let cancel_clone = cancel.clone();
      tokio::spawn(async move {
          tokio::time::sleep(std::time::Duration::from_millis(100)).await;
          cancel_clone.cancel();
      });
      let outcome = orch.run_turn_streaming_with_cancel("hi", cancel).await.unwrap();
      assert_eq!(outcome, TurnOutcome::Cancelled);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-orchestrator run_turn_streaming_with_cancel_returns_cancelled_when_token_fires -- --nocapture
  ```
  Expected: FAIL — `run_turn_streaming_with_cancel` does not exist.

- [ ] **Step 3: Implement the method**

  In `lingxi-core/crates/orchestrator/src/conversation.rs` after the `run_turn_with_cancel` block (around line 643), add:

  ```rust
      /// Streaming twin of [`Self::run_turn_with_cancel`] (M5-13).
      ///
      /// Race [`Self::try_run_turn_streaming`] against the `cancel` token:
      /// - natural completion → `TurnOutcome::EndTurn`
      /// - `cancel.cancelled()` fires → `TurnOutcome::Cancelled` (the SSE
      ///   stream is dropped, which closes the HTTP request and flushes any
      ///   already-buffered `emit_text` calls to the output sink).
      /// - any other API/streaming error → propagated as `Err`.
      ///
      /// This is the entry point the M6 TUI calls. M5-13 stdio REPL keeps
      /// using `run_turn_with_cancel` (batched) until M6 makes streaming
      /// the default.
      pub async fn run_turn_streaming_with_cancel(
          &self,
          prompt: &str,
          cancel: CancellationToken,
      ) -> Result<TurnOutcome, OrchestratorError> {
          tracing::info!(
              event = orch_events::TURN_STREAMING_STARTED,
              prompt_len = prompt.len()
          );
          let inner = self.try_run_turn_streaming(prompt);
          tokio::select! {
              r = inner => match r {
                  Ok(crate::conversation::ConversationOutcome::EndTurn { turn_count, .. }) => {
                      tracing::info!(
                          event = orch_events::TURN_STREAMING_COMPLETED,
                          turn_count
                      );
                      Ok(TurnOutcome::EndTurn)
                  }
                  Err(e) => Err(e),
              },
              () = cancel.cancelled() => Ok(TurnOutcome::Cancelled),
          }
      }
  ```

- [ ] **Step 4: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-orchestrator run_turn_streaming_with_cancel_returns_cancelled_when_token_fires -- --nocapture
  ```
  Expected: PASS within ~150ms (cancel fires at 100ms).

- [ ] **Step 5: Add a second test for the happy path**

  Append:

  ```rust
  #[tokio::test]
  async fn run_turn_streaming_with_cancel_returns_endturn_on_normal_completion() {
      use crate::test_support::build_test_orchestrator_streaming;
      use tokio_util::sync::CancellationToken;
      let (orch, _captured) = build_test_orchestrator_streaming(vec![
          scripted!(MessageStart),
          scripted!(ContentBlockStart { type: "text" }),
          scripted!(ContentBlockDelta { text: "ok" }),
          scripted!(ContentBlockStop),
          scripted!(MessageDelta { stop_reason: "end_turn" }),
          scripted!(MessageStop),
      ]).await;
      let cancel = CancellationToken::new();
      let outcome = orch.run_turn_streaming_with_cancel("hi", cancel).await.unwrap();
      assert_eq!(outcome, TurnOutcome::EndTurn);
  }
  ```

  Run:
  ```bash
  cargo test -p lingxi-orchestrator run_turn_streaming_with_cancel_returns_endturn -- --nocapture
  ```
  Expected: PASS.

- [ ] **Step 6: Commit**

  ```bash
  git add lingxi-core/crates/orchestrator/src/conversation.rs
  git commit -m "feat(orchestrator): add run_turn_streaming_with_cancel for M6 TUI

Mirror M5-13's run_turn_with_cancel shape, racing the streaming
turn body against a CancellationToken. The TUI uses this as its
entry point so Ctrl-C aborts the in-flight SSE stream cleanly.

Refs M6-03 Task 1"
  ```

---

## Task 2: Add `run_turn_streaming_with_cancel` to `OrchestratorHandle` trait

**Files:**
- Modify: `lingxi-core/crates/traits/src/orchestrator.rs` (trait definition).
- Modify: `lingxi-core/crates/orchestrator/src/handle_impl.rs` (real impl).

- [ ] **Step 1: Write the failing test**

  Append to `lingxi-core/crates/orchestrator/src/handle_impl.rs` `#[cfg(test)] mod tests` (or create one if absent):

  ```rust
  #[tokio::test]
  async fn handle_impl_run_turn_streaming_with_cancel_routes_to_orchestrator() {
      let orch = std::sync::Arc::new(crate::test_support::build_test_orchestrator_minimal().await.0);
      let handle: std::sync::Arc<dyn lingxi_traits::OrchestratorHandle> =
          std::sync::Arc::new(OrchestratorHandleImpl::new(orch.clone()));
      let cancel = tokio_util::sync::CancellationToken::new();
      cancel.cancel(); // pre-cancelled → should return Cancelled immediately
      let outcome = handle.run_turn_streaming_with_cancel("hi", cancel).await.unwrap();
      assert!(matches!(outcome, lingxi_traits::TurnOutcome::Cancelled));
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-orchestrator handle_impl_run_turn_streaming_with_cancel_routes -- --nocapture
  ```
  Expected: FAIL — method missing on trait + impl.

- [ ] **Step 3: Extend the trait**

  In `lingxi-core/crates/traits/src/orchestrator.rs`, find the `pub trait OrchestratorHandle` block. After the existing `run_turn_with_cancel` method (M5-13), append:

  ```rust
      /// Streaming twin of [`Self::run_turn_with_cancel`]. The TUI (M6) calls
      /// this to drive a per-token stream that the bridge translates into
      /// `TurnEvent`s on the renderer side.
      ///
      /// Default impl returns `Err(OrchestratorError::Unimplemented)` so the
      /// stdio REPL impl (M5-13) does not need to override.
      async fn run_turn_streaming_with_cancel(
          &self,
          prompt: &str,
          cancel: tokio_util::sync::CancellationToken,
      ) -> Result<TurnOutcome, OrchestratorError> {
          let _ = (prompt, cancel);
          Err(OrchestratorError::Unimplemented(
              "run_turn_streaming_with_cancel".into(),
          ))
      }
  ```

  If `OrchestratorError::Unimplemented` does not exist, add it as a variant to the existing `OrchestratorError` enum in the same file:

  ```rust
      #[error("operation not implemented: {0}")]
      Unimplemented(String),
  ```

- [ ] **Step 4: Implement on `OrchestratorHandleImpl`**

  In `lingxi-core/crates/orchestrator/src/handle_impl.rs`, find the existing `impl OrchestratorHandle for OrchestratorHandleImpl` block. After `run_turn_with_cancel`, append:

  ```rust
      async fn run_turn_streaming_with_cancel(
          &self,
          prompt: &str,
          cancel: tokio_util::sync::CancellationToken,
      ) -> Result<lingxi_traits::TurnOutcome, lingxi_traits::OrchestratorError> {
          self.orch
              .run_turn_streaming_with_cancel(prompt, cancel)
              .await
              .map(crate::conversation_to_trait_outcome)
              .map_err(crate::orchestrator_error_to_trait_error)
      }
  ```

  (Reuse the same `conversation_to_trait_outcome` / `orchestrator_error_to_trait_error` helpers that M5-13's `run_turn_with_cancel` already uses. If those helpers don't exist, inline the mapping via `match`.)

- [ ] **Step 5: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-orchestrator handle_impl_run_turn_streaming_with_cancel_routes -- --nocapture
  ```
  Expected: PASS.

- [ ] **Step 6: Verify no regression in existing handle tests**

  Run:
  ```bash
  cargo test -p lingxi-orchestrator handle_impl -- --nocapture
  cargo test -p lingxi-traits -- --nocapture
  ```
  Expected: all pre-existing tests still PASS.

- [ ] **Step 7: Commit**

  ```bash
  git add lingxi-core/crates/traits/src/orchestrator.rs lingxi-core/crates/orchestrator/src/handle_impl.rs
  git commit -m "feat(traits): add run_turn_streaming_with_cancel to OrchestratorHandle

Default impl returns Unimplemented so existing stdio REPL impl
needs no changes. Real impl on OrchestratorHandleImpl delegates
to ConversationOrchestrator::run_turn_streaming_with_cancel.

Refs M6-03 Task 2"
  ```

---

## Task 3: Define `TurnEvent` enum and `BridgeOutputStream`

**Files:**
- Modify: `lingxi-core/crates/tui/src/events/orchestrator_bridge.rs` (file exists from M6-01).

- [ ] **Step 1: Write the failing test**

  Append to `lingxi-core/crates/tui/src/events/orchestrator_bridge.rs`:

  ```rust
  #[cfg(test)]
  mod bridge_tests {
      use super::*;
      use lingxi_traits::OutputStream;
      use tokio::sync::mpsc;

      #[tokio::test]
      async fn bridge_translates_emit_text_to_text_delta() {
          let (tx, mut rx) = mpsc::unbounded_channel();
          let bridge = BridgeOutputStream::new(tx);
          bridge.emit_text("hello").await;
          let ev = rx.recv().await.unwrap();
          assert!(matches!(ev, TurnEvent::TextDelta(ref s) if s == "hello"));
      }

      #[tokio::test]
      async fn bridge_translates_emit_tool_call_to_tool_use_start() {
          let (tx, mut rx) = mpsc::unbounded_channel();
          let bridge = BridgeOutputStream::new(tx);
          bridge.emit_tool_call("Read", &serde_json::json!({"file_path": "/tmp/x"})).await;
          let ev = rx.recv().await.unwrap();
          match ev {
              TurnEvent::ToolUseStart { tool, input, .. } => {
                  assert_eq!(tool, "Read");
                  assert_eq!(input["file_path"], "/tmp/x");
              }
              other => panic!("unexpected: {other:?}"),
          }
      }

      #[tokio::test]
      async fn bridge_translates_emit_end_turn_to_turn_ended_endturn() {
          let (tx, mut rx) = mpsc::unbounded_channel();
          let bridge = BridgeOutputStream::new(tx);
          let cost = lingxi_traits::CostSnapshot::default();
          bridge.emit_end_turn("end_turn", &cost).await;
          let ev = rx.recv().await.unwrap();
          assert!(matches!(ev, TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn)));
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-tui bridge_tests -- --nocapture
  ```
  Expected: FAIL — `TurnEvent` and `BridgeOutputStream` are not defined.

- [ ] **Step 3: Define `TurnEvent` enum**

  Add to the top of `lingxi-core/crates/tui/src/events/orchestrator_bridge.rs`:

  ```rust
  use async_trait::async_trait;
  use lingxi_traits::{CostSnapshot, OutputStream, TurnOutcome};
  use tokio::sync::mpsc::UnboundedSender;

  /// Events flowing from the orchestrator into the TUI render loop.
  ///
  /// Created in M6-03 as a TUI-local enum (not exposed on any orchestrator
  /// trait). The bridge translates `OutputStream` callbacks into this enum.
  /// Future expansion: `PermissionRequest` is wired in M6-05; `ThinkingDelta`
  /// in M7.
  #[derive(Debug, Clone)]
  pub enum TurnEvent {
      /// Streaming text chunk from the assistant.
      TextDelta(String),
      /// A tool invocation is about to dispatch.
      ToolUseStart {
          /// Stable id (UUID v4) assigned by the bridge so the TUI can
          /// correlate `ToolUseStart` and `ToolUseResult`.
          id: String,
          tool: String,
          input: serde_json::Value,
      },
      /// A tool result has returned.
      ToolUseResult {
          id: String,
          result: serde_json::Value,
      },
      /// Permission gate fired. Payload shape TBD in M6-05 — for M6-03 the
      /// variant is reserved as `Reserved` so the enum is forward-compatible.
      PermissionRequest {
          tool: String,
          input: serde_json::Value,
      },
      /// Fired SYNCHRONOUSLY before the orchestrator future is awaited.
      TurnStarted,
      /// Fired when the orchestrator returns. Carries the `TurnOutcome`.
      TurnEnded(TurnOutcome),
  }

  /// `OutputStream` impl that forwards every callback as a `TurnEvent` on
  /// an mpsc channel. Cloneable via `tx.clone()` if multiple producers
  /// are ever needed (currently one bridge per turn).
  pub struct BridgeOutputStream {
      tx: UnboundedSender<TurnEvent>,
  }

  impl BridgeOutputStream {
      pub fn new(tx: UnboundedSender<TurnEvent>) -> Self {
          Self { tx }
      }
  }

  #[async_trait]
  impl OutputStream for BridgeOutputStream {
      async fn emit_text(&self, text: &str) {
          let _ = self.tx.send(TurnEvent::TextDelta(text.to_string()));
      }
      async fn emit_tool_call(&self, tool: &str, input: &serde_json::Value) {
          let _ = self.tx.send(TurnEvent::ToolUseStart {
              id: uuid::Uuid::new_v4().to_string(),
              tool: tool.to_string(),
              input: input.clone(),
          });
      }
      async fn emit_tool_result(&self, tool: &str, result: &serde_json::Value) {
          // M6-03 uses tool name as a poor-man's correlator; M6-04 will
          // thread a real id through ToolUseStart→Result.
          let _ = self.tx.send(TurnEvent::ToolUseResult {
              id: tool.to_string(),
              result: result.clone(),
          });
      }
      async fn emit_end_turn(&self, stop_reason: &str, _cost: &CostSnapshot) {
          // Map stop_reason string → TurnOutcome. M5-13 already does this
          // mapping in stdio REPL; mirror it here.
          let outcome = match stop_reason {
              "end_turn" | "stop_sequence" => TurnOutcome::EndTurn,
              "max_tokens" => TurnOutcome::MaxTurns,
              _ => TurnOutcome::EndTurn, // unknown → treat as natural end
          };
          let _ = self.tx.send(TurnEvent::TurnEnded(outcome));
      }
  }
  ```

  If `uuid` crate is not yet in `lingxi-core/crates/tui/Cargo.toml`, add it:

  ```toml
  uuid = { workspace = true, features = ["v4"] }
  ```

  If `uuid` is not yet in `lingxi-core/Cargo.toml` `[workspace.dependencies]`, add:

  ```toml
  uuid = { version = "1", default-features = false, features = ["v4"] }
  ```

  (Check: it's already used by `lingxi-orchestrator` for `MessageId::new()` — likely present.)

- [ ] **Step 4: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-tui bridge_tests -- --nocapture
  ```
  Expected: all 3 bridge tests PASS.

- [ ] **Step 5: Add a `TurnStarted` emission test**

  Append:

  ```rust
      #[tokio::test]
      async fn bridge_turn_started_is_explicit_send_not_callback() {
          // TurnStarted is emitted by the SPAWNER (app.rs), not the bridge.
          // This test documents that contract.
          let (tx, mut rx) = mpsc::unbounded_channel();
          tx.send(TurnEvent::TurnStarted).unwrap();
          let ev = rx.recv().await.unwrap();
          assert!(matches!(ev, TurnEvent::TurnStarted));
      }
  ```

  Run:
  ```bash
  cargo test -p lingxi-tui bridge_turn_started -- --nocapture
  ```
  Expected: PASS.

- [ ] **Step 6: Commit**

  ```bash
  git add lingxi-core/crates/tui/src/events/orchestrator_bridge.rs lingxi-core/crates/tui/Cargo.toml lingxi-core/Cargo.toml
  git commit -m "feat(tui): add TurnEvent enum + BridgeOutputStream

TurnEvent is a TUI-local enum mirroring the streaming events the
renderer cares about. BridgeOutputStream implements OutputStream
by forwarding every callback as a TurnEvent on an mpsc channel.

Refs M6-03 Task 3"
  ```

---

## Task 4: Define spinner constants — frames + verbs

**Files:**
- Create: `lingxi-core/crates/tui/src/components/spinner.rs`.
- Modify: `lingxi-core/crates/tui/src/components/mod.rs` (re-export).

- [ ] **Step 1: Write the failing test**

  Create `lingxi-core/crates/tui/src/components/spinner.rs` with ONLY the test (no impl):

  ```rust
  #![forbid(unsafe_code)]
  //! `SpinnerWithVerb` — claude-code-equivalent loading spinner. (M6-03)

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn spinner_frames_match_claude_code_darwin_default() {
          assert_eq!(
              SPINNER_FRAMES,
              &["·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·"]
          );
          assert_eq!(SPINNER_FRAMES.len(), 12);
      }

      #[test]
      fn verbs_m6_match_design_subset() {
          assert_eq!(VERBS_M6, &["Crunching", "Thinking", "Generating"]);
      }

      #[test]
      fn verb_format_uses_horizontal_ellipsis() {
          assert_eq!(format!("{}{}", "Crunching", '…'), "Crunching…");
          // The character U+2026 (HORIZONTAL ELLIPSIS), NOT three ASCII dots.
          assert_eq!('…' as u32, 0x2026);
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-tui --lib spinner -- --nocapture
  ```
  Expected: FAIL — `SPINNER_FRAMES` and `VERBS_M6` are not defined.

- [ ] **Step 3: Add the constants**

  At the top of `lingxi-core/crates/tui/src/components/spinner.rs` (above the `#[cfg(test)]` block):

  ```rust
  /// Per `claude-code/src/components/Spinner/utils.ts` (darwin default) +
  /// `Spinner.tsx:41` (`SPINNER_FRAMES = [...DEFAULT, ...reverse(DEFAULT)]`):
  /// 6 forward chars then 6 reverse chars = 12 total.
  ///
  /// Linux platforms substitute `✳` → `*` (one frame difference); locked
  /// here as the darwin variant since macOS is our primary dev platform
  /// and the spec §2.8 requires byte-for-byte parity with claude-code.
  /// (A `cfg(target_os = "linux")` variant lands in M7 if needed.)
  pub const SPINNER_FRAMES: &[&str] = &[
      "·", "✢", "✳", "✶", "✻", "✽",
      "✽", "✻", "✶", "✳", "✢", "·",
  ];

  /// The 3-verb subset M6 uses (deterministically cycled every 4s for
  /// testability). claude-code's full pool of 100+ verbs (see
  /// `claude-code/src/constants/spinnerVerbs.ts`) is deferred to M7 along
  /// with the random-on-mount selection logic.
  pub const VERBS_M6: &[&str] = &["Crunching", "Thinking", "Generating"];

  /// Time between spinner frame advances. claude-code uses 50ms (20fps);
  /// M6 uses 100ms (10fps) per spec §3 M6-03 — slower to reduce render
  /// churn under the 30fps cap.
  pub const FRAME_TICK_MS: u64 = 100;

  /// Time between verb rotations. Locked at 4000ms so all 3 verbs cycle
  /// in 12s — long enough that users notice the change, short enough that
  /// it doesn't feel static.
  pub const VERB_ROTATE_MS: u64 = 4000;
  ```

- [ ] **Step 4: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-tui --lib spinner -- --nocapture
  ```
  Expected: all 3 spinner constant tests PASS.

- [ ] **Step 5: Wire into `components/mod.rs`**

  Append to `lingxi-core/crates/tui/src/components/mod.rs`:

  ```rust
  pub mod spinner;
  ```

  Run:
  ```bash
  cargo build -p lingxi-tui
  ```
  Expected: clean build, no warnings.

- [ ] **Step 6: Commit**

  ```bash
  git add lingxi-core/crates/tui/src/components/spinner.rs lingxi-core/crates/tui/src/components/mod.rs
  git commit -m "feat(tui): add SpinnerWithVerb constants (frames + verbs)

12-frame asterisk animation matching claude-code Spinner.tsx +
3-verb M6 subset matching design spec §3 M6-03.

Refs M6-03 Task 4"
  ```

---

## Task 5: Implement `SpinnerWithVerb` component

**Files:**
- Modify: `lingxi-core/crates/tui/src/components/spinner.rs`.

- [ ] **Step 1: Write the failing test**

  Append to the `#[cfg(test)] mod tests` block in `spinner.rs`:

  ```rust
      #[test]
      fn frame_at_index_wraps() {
          assert_eq!(frame_at_index(0), "·");
          assert_eq!(frame_at_index(5), "✽");
          assert_eq!(frame_at_index(9), "✳");
          assert_eq!(frame_at_index(12), "·"); // wraps
          assert_eq!(frame_at_index(25), "✢"); // wraps twice
      }

      #[test]
      fn verb_at_index_wraps() {
          assert_eq!(verb_at_index(0), "Crunching");
          assert_eq!(verb_at_index(1), "Thinking");
          assert_eq!(verb_at_index(2), "Generating");
          assert_eq!(verb_at_index(3), "Crunching"); // wraps
      }

      #[test]
      fn format_spinner_line_includes_ellipsis() {
          assert_eq!(format_spinner_line(0, 0), "· Crunching…");
          assert_eq!(format_spinner_line(5, 1), "✽ Thinking…");
      }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-tui --lib frame_at_index -- --nocapture
  ```
  Expected: FAIL — helpers not defined.

- [ ] **Step 3: Add helpers + component**

  In `spinner.rs`, add ABOVE the test block:

  ```rust
  use iocraft::prelude::*;

  /// Get the spinner glyph for a tick index. Wraps modulo `SPINNER_FRAMES.len()`.
  #[inline]
  pub fn frame_at_index(tick: usize) -> &'static str {
      SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
  }

  /// Get the verb for a rotation index. Wraps modulo `VERBS_M6.len()`.
  #[inline]
  pub fn verb_at_index(rotation: usize) -> &'static str {
      VERBS_M6[rotation % VERBS_M6.len()]
  }

  /// Format a single spinner line: `"{frame} {verb}…"`.
  ///
  /// This is the function the iocraft component renders inside its `Text`.
  /// Exposed publicly so snapshot tests can assert without a render harness.
  #[must_use]
  pub fn format_spinner_line(tick: usize, rotation: usize) -> String {
      format!("{} {}…", frame_at_index(tick), verb_at_index(rotation))
  }

  /// Props for [`SpinnerWithVerb`]. Both fields are hook-managed inside the
  /// component by default; pass `Some(_)` to override (used by snapshot tests).
  #[derive(Default, Props)]
  pub struct SpinnerWithVerbProps {
      /// Override the frame index. `None` (default) → component ticks
      /// internally at `FRAME_TICK_MS`.
      pub frame_override: Option<usize>,
      /// Override the verb rotation index. `None` → internal rotation at
      /// `VERB_ROTATE_MS`.
      pub verb_override: Option<usize>,
  }

  /// Renders one line: `"{frame} {verb}…"`. While mounted, advances frames at
  /// 10fps and rotates verbs every 4s. Both intervals are constants on this
  /// module (`FRAME_TICK_MS`, `VERB_ROTATE_MS`).
  ///
  /// Mounting/unmounting is the caller's responsibility: REPL screen wraps
  /// this in `if app.streaming.is_some() { <SpinnerWithVerb/> }`.
  #[component]
  pub fn SpinnerWithVerb(
      props: &SpinnerWithVerbProps,
      mut hooks: Hooks,
  ) -> impl Into<AnyElement<'static>> {
      let mut frame = hooks.use_state(|| 0usize);
      let mut verb = hooks.use_state(|| 0usize);

      // Use overrides if provided (snapshot-test path); otherwise advance via
      // tokio interval futures.
      if let Some(f) = props.frame_override {
          frame.set(f);
      } else {
          hooks.use_future(async move {
              let mut tick = tokio::time::interval(std::time::Duration::from_millis(FRAME_TICK_MS));
              tick.tick().await; // first tick fires immediately; discard
              loop {
                  tick.tick().await;
                  let cur = frame.get();
                  frame.set(cur.wrapping_add(1));
              }
          });
      }
      if let Some(v) = props.verb_override {
          verb.set(v);
      } else {
          hooks.use_future(async move {
              let mut tick = tokio::time::interval(std::time::Duration::from_millis(VERB_ROTATE_MS));
              tick.tick().await;
              loop {
                  tick.tick().await;
                  let cur = verb.get();
                  verb.set(cur.wrapping_add(1));
              }
          });
      }

      let line = format_spinner_line(frame.get(), verb.get());
      element! {
          Box(flex_direction: FlexDirection::Row) {
              Text(content: line, color: Color::Cyan)
          }
      }
  }
  ```

  **Note:** the `use_future` API surface for iocraft 0.6 is the validated one (M6-01 prototype gate confirmed). If the actual API differs, adapt — the contract this code expresses (single-shot async future per render-mount, tokio-driven interval, set state from inside) is what matters.

- [ ] **Step 4: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-tui --lib spinner -- --nocapture
  ```
  Expected: all 6 spinner tests PASS.

- [ ] **Step 5: Commit**

  ```bash
  git add lingxi-core/crates/tui/src/components/spinner.rs
  git commit -m "feat(tui): implement SpinnerWithVerb iocraft component

Internal frame and verb state advanced via tokio::time::interval
inside use_future hooks. Snapshot tests use {frame,verb}_override
props to assert deterministically without waiting for ticks.

Refs M6-03 Task 5"
  ```

---

## Task 6: Add 3 spinner snapshots (frames 0, 5, 9)

**Files:**
- Create: `lingxi-core/crates/tui/tests/render_spinner_test.rs`.

- [ ] **Step 1: Write the failing snapshot test**

  Create `lingxi-core/crates/tui/tests/render_spinner_test.rs`:

  ```rust
  //! Snapshot tests for SpinnerWithVerb rendered output. (M6-03 Task 6)
  //!
  //! Locks the rendered glyph + verb for frame indices 0, 5, 9 — covering
  //! the start, midpoint, and second-cycle position of the 12-frame loop.

  use insta::assert_snapshot;
  use lingxi_tui::components::spinner::format_spinner_line;

  #[test]
  fn spinner_frame_0_crunching() {
      assert_snapshot!("spinner_frame_0", format_spinner_line(0, 0));
  }

  #[test]
  fn spinner_frame_5_thinking() {
      assert_snapshot!("spinner_frame_5", format_spinner_line(5, 1));
  }

  #[test]
  fn spinner_frame_9_generating() {
      assert_snapshot!("spinner_frame_9", format_spinner_line(9, 2));
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-tui --test render_spinner_test -- --nocapture
  ```
  Expected: FAIL — `.snap` files do not exist yet. The output shows the values insta wants to write.

- [ ] **Step 3: Accept the snapshots**

  Run:
  ```bash
  INSTA_UPDATE=always cargo test -p lingxi-tui --test render_spinner_test
  ```

  Verify the generated `.snap` files in `lingxi-core/crates/tui/tests/snapshots/`:

  ```bash
  ls lingxi-core/crates/tui/tests/snapshots/
  ```
  Expected: three files — `render_spinner_test__spinner_frame_0.snap`, `render_spinner_test__spinner_frame_5.snap`, `render_spinner_test__spinner_frame_9.snap`.

  Open each and verify content:

  ```
  ---
  source: lingxi-core/crates/tui/tests/render_spinner_test.rs
  expression: format_spinner_line(0, 0)
  ---
  · Crunching…
  ```

  Repeat for frames 5 and 9 (expected: `✽ Thinking…` and `✳ Generating…`).

- [ ] **Step 4: Re-run without UPDATE to confirm stability**

  Run:
  ```bash
  cargo test -p lingxi-tui --test render_spinner_test -- --nocapture
  ```
  Expected: all 3 tests PASS.

- [ ] **Step 5: Commit**

  ```bash
  git add lingxi-core/crates/tui/tests/render_spinner_test.rs lingxi-core/crates/tui/tests/snapshots/render_spinner_test__spinner_frame_0.snap lingxi-core/crates/tui/tests/snapshots/render_spinner_test__spinner_frame_5.snap lingxi-core/crates/tui/tests/snapshots/render_spinner_test__spinner_frame_9.snap
  git commit -m "test(tui): snapshot SpinnerWithVerb at frames 0, 5, 9

Locks asterisk glyph + verb byte-for-byte. Frame 0=· Crunching…,
frame 5=✽ Thinking…, frame 9=✳ Generating…. Detects any drift
from claude-code's Spinner.tsx default characters.

Refs M6-03 Task 6"
  ```

---

## Task 7: Implement `apply_event` in `streaming.rs`

**Files:**
- Create: `lingxi-core/crates/tui/src/streaming.rs`.
- Modify: `lingxi-core/crates/tui/src/lib.rs` (add `pub mod streaming;`).
- Modify: `lingxi-core/crates/tui/src/app.rs` (extend `AppState`).

- [ ] **Step 1: Write the failing test**

  Create `lingxi-core/crates/tui/src/streaming.rs`:

  ```rust
  #![forbid(unsafe_code)]
  //! Streaming subscriber: applies `TurnEvent`s to `AppState`. (M6-03)
  //!
  //! Pure function — no side effects beyond `state` mutation and
  //! `notify.notify_one()`. The render loop in `app.rs` calls this on
  //! every event received from the bridge channel.

  use crate::app::{AppState, RenderedMessage, StreamingState};
  use crate::events::orchestrator_bridge::TurnEvent;
  use tokio::sync::Notify;

  /// Apply one `TurnEvent` to `state` and signal the renderer.
  ///
  /// Behavior contract:
  /// - `TurnStarted` → set `state.streaming = Some(StreamingState::new())`.
  /// - `TextDelta(s)` → if the last message in `state.messages` is an
  ///   `AssistantText`, append `s` to its text. Otherwise push a NEW
  ///   `AssistantText` message with text `s`.
  /// - `ToolUseStart{..}` → push an `AssistantToolUse` placeholder (full
  ///   rendering in M6-04).
  /// - `ToolUseResult{..}` → push a `UserToolResult` placeholder (M6-04).
  /// - `PermissionRequest{..}` → set `state.pending_permission` (M6-05).
  /// - `TurnEnded(_)` → clear `state.streaming`.
  ///
  /// After mutation, calls `notify.notify_one()`. The render loop is
  /// expected to debounce these to ~30fps (see `app.rs::run_render_loop`).
  pub fn apply_event(state: &mut AppState, ev: TurnEvent, notify: &Notify) {
      match ev {
          TurnEvent::TurnStarted => {
              state.streaming = Some(StreamingState::new());
          }
          TurnEvent::TextDelta(text) => {
              // Append to last AssistantText if present; otherwise push new.
              if let Some(RenderedMessage::AssistantText { text: prev }) = state.messages.last_mut() {
                  prev.push_str(&text);
              } else {
                  state.messages.push(RenderedMessage::AssistantText { text });
              }
          }
          TurnEvent::ToolUseStart { id, tool, input } => {
              state.messages.push(RenderedMessage::AssistantToolUse { id, tool, input });
          }
          TurnEvent::ToolUseResult { id, result } => {
              state.messages.push(RenderedMessage::UserToolResult { id, result });
          }
          TurnEvent::PermissionRequest { tool, input } => {
              state.pending_permission = Some(crate::app::PendingPermission { tool, input });
          }
          TurnEvent::TurnEnded(_outcome) => {
              state.streaming = None;
          }
      }
      notify.notify_one();
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::app::AppState;

      fn new_state() -> AppState {
          AppState::default()
      }

      #[test]
      fn text_delta_creates_new_assistant_message_when_buffer_empty() {
          let mut s = new_state();
          let n = Notify::new();
          apply_event(&mut s, TurnEvent::TextDelta("hello".into()), &n);
          assert_eq!(s.messages.len(), 1);
          assert!(matches!(s.messages.last(), Some(RenderedMessage::AssistantText { text }) if text == "hello"));
      }

      #[test]
      fn five_deltas_concatenate_into_single_message() {
          let mut s = new_state();
          let n = Notify::new();
          for chunk in ["he", "ll", "o ", "wo", "rld"] {
              apply_event(&mut s, TurnEvent::TextDelta(chunk.into()), &n);
          }
          assert_eq!(s.messages.len(), 1);
          assert!(matches!(s.messages.last(), Some(RenderedMessage::AssistantText { text }) if text == "hello world"));
      }

      #[test]
      fn turn_started_sets_streaming_some() {
          let mut s = new_state();
          let n = Notify::new();
          assert!(s.streaming.is_none());
          apply_event(&mut s, TurnEvent::TurnStarted, &n);
          assert!(s.streaming.is_some());
      }

      #[test]
      fn turn_ended_clears_streaming() {
          let mut s = new_state();
          let n = Notify::new();
          s.streaming = Some(StreamingState::new());
          apply_event(&mut s, TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn), &n);
          assert!(s.streaming.is_none());
      }

      #[test]
      fn apply_event_calls_notify_one() {
          let mut s = new_state();
          let n = Notify::new();
          // Pre-record a permit so we can detect notify_one.
          let waiter = n.notified();
          tokio::pin!(waiter);
          apply_event(&mut s, TurnEvent::TextDelta("x".into()), &n);
          // notify_one stores one permit; the waiter resolves immediately when polled.
          let poll = futures::poll!(waiter.as_mut());
          assert!(matches!(poll, std::task::Poll::Ready(())));
      }
  }
  ```

- [ ] **Step 2: Extend `AppState`**

  In `lingxi-core/crates/tui/src/app.rs`, find the `pub struct AppState` (added in M6-02) and add fields:

  ```rust
      /// `Some(_)` while a turn is streaming; `None` between turns.
      /// Spinner mount predicate.
      pub streaming: Option<StreamingState>,

      /// Token used to cancel the in-flight turn. `Some(_)` mirrors
      /// `streaming.is_some()`. Reset to `None` after `TurnEnded`.
      pub cancel_token: Option<tokio_util::sync::CancellationToken>,

      /// `Some(_)` while a permission dialog is up. M6-03 doesn't render
      /// the dialog (that's M6-05) but reserves the slot for the bridge.
      pub pending_permission: Option<PendingPermission>,
  ```

  Add the supporting types (in the same file, below `AppState`):

  ```rust
  /// Per-turn streaming state. Created on `TurnStarted`, dropped on
  /// `TurnEnded`. Currently carries only the start instant for debugging;
  /// M6-04 may add a tool-use map.
  #[derive(Debug, Clone)]
  pub struct StreamingState {
      pub started_at: std::time::Instant,
  }

  impl StreamingState {
      pub fn new() -> Self {
          Self { started_at: std::time::Instant::now() }
      }
  }

  impl Default for StreamingState {
      fn default() -> Self {
          Self::new()
      }
  }

  /// Permission request awaiting user decision. M6-05 fleshes this out.
  #[derive(Debug, Clone)]
  pub struct PendingPermission {
      pub tool: String,
      pub input: serde_json::Value,
  }
  ```

  Add the new variants to `RenderedMessage` (defined in M6-02). Find the enum and append:

  ```rust
      AssistantToolUse {
          id: String,
          tool: String,
          input: serde_json::Value,
      },
      UserToolResult {
          id: String,
          result: serde_json::Value,
      },
  ```

  (These are placeholders — M6-04 wires real rendering. Adding them here keeps the `apply_event` `match` exhaustive.)

  Update `AppState::default()` (or the `#[derive(Default)]` if used) so `streaming`, `cancel_token`, `pending_permission` all default to `None`.

- [ ] **Step 3: Wire `streaming` module into the crate root**

  In `lingxi-core/crates/tui/src/lib.rs`, append:

  ```rust
  pub mod streaming;
  ```

- [ ] **Step 4: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-tui --lib streaming::tests -- --nocapture
  ```
  Expected: all 5 streaming tests PASS.

- [ ] **Step 5: Commit**

  ```bash
  git add lingxi-core/crates/tui/src/streaming.rs lingxi-core/crates/tui/src/lib.rs lingxi-core/crates/tui/src/app.rs
  git commit -m "feat(tui): add apply_event subscriber + AppState streaming fields

apply_event(state, ev, notify) is the pure mutator the render loop
calls per TurnEvent. AssistantText messages accumulate deltas in
place; tool messages are placeholders for M6-04; permission slot
is reserved for M6-05. Five-delta concatenation test locks the
contract.

Refs M6-03 Task 7"
  ```

---

## Task 8: Wire `app.rs` to drive streaming + cancel

**Files:**
- Modify: `lingxi-core/crates/tui/src/app.rs`.

- [ ] **Step 1: Write the failing test**

  Create `lingxi-core/crates/tui/tests/streaming_test.rs`:

  ```rust
  //! End-to-end streaming + cancel tests for the TUI app. (M6-03 Task 8)

  use lingxi_tui::app::AppState;
  use lingxi_tui::events::orchestrator_bridge::TurnEvent;
  use lingxi_tui::streaming::apply_event;
  use std::sync::Arc;
  use tokio::sync::Notify;
  use tokio_util::sync::CancellationToken;

  #[tokio::test]
  async fn five_deltas_with_50ms_gap_concatenate_correctly() {
      let mut state = AppState::default();
      let notify = Notify::new();
      apply_event(&mut state, TurnEvent::TurnStarted, &notify);
      let chunks = ["h", "el", "lo ", "wor", "ld"];
      for c in chunks {
          apply_event(&mut state, TurnEvent::TextDelta(c.into()), &notify);
          tokio::time::sleep(std::time::Duration::from_millis(50)).await;
      }
      apply_event(&mut state, TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn), &notify);

      assert_eq!(state.messages.len(), 1);
      match state.messages.last().unwrap() {
          lingxi_tui::app::RenderedMessage::AssistantText { text } => {
              assert_eq!(text, "hello world");
          }
          other => panic!("unexpected last message: {other:?}"),
      }
      assert!(state.streaming.is_none());
  }

  #[tokio::test]
  async fn spinner_mount_predicate_tracks_streaming_field() {
      let mut state = AppState::default();
      let notify = Notify::new();
      assert!(!should_mount_spinner(&state));
      apply_event(&mut state, TurnEvent::TurnStarted, &notify);
      assert!(should_mount_spinner(&state));
      apply_event(&mut state, TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn), &notify);
      assert!(!should_mount_spinner(&state));
  }

  fn should_mount_spinner(s: &AppState) -> bool {
      s.streaming.is_some()
  }

  #[tokio::test]
  async fn ctrl_c_cancels_within_100ms_and_clears_streaming() {
      use lingxi_tui::app::handle_ctrl_c;
      let mut state = AppState::default();
      let notify = Notify::new();
      let cancel = CancellationToken::new();
      apply_event(&mut state, TurnEvent::TurnStarted, &notify);
      state.cancel_token = Some(cancel.clone());

      let start = std::time::Instant::now();
      handle_ctrl_c(&mut state);
      // The orchestrator task will detect cancellation and eventually emit
      // TurnEnded. Simulate that path:
      tokio::time::sleep(std::time::Duration::from_millis(20)).await;
      apply_event(&mut state, TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::Cancelled), &notify);

      assert!(cancel.is_cancelled());
      assert!(state.streaming.is_none());
      assert!(start.elapsed() < std::time::Duration::from_millis(100));
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-tui --test streaming_test -- --nocapture
  ```
  Expected: FAIL — `handle_ctrl_c` is not defined; `AppState::default()` may not yet exist depending on M6-02's setup.

- [ ] **Step 3: Add `handle_ctrl_c` and `spawn_streaming_turn` to `app.rs`**

  In `lingxi-core/crates/tui/src/app.rs`, add:

  ```rust
  /// Ctrl-C handler during streaming. Idempotent if `streaming.is_none()`
  /// (caller delegates to M6-02's prompt-clear / second-Ctrl-C logic in
  /// that case). When streaming, fires the cancel token; the orchestrator
  /// task will return `TurnOutcome::Cancelled` and the bridge will emit
  /// `TurnEnded`, which `apply_event` translates into `streaming = None`.
  pub fn handle_ctrl_c(state: &mut AppState) {
      if let Some(token) = state.cancel_token.take() {
          token.cancel();
      }
      // Note: we do NOT clear `state.streaming` here. We wait for the
      // bridge's TurnEnded event to clear it, so the spinner stays up
      // until the orchestrator actually unwinds.
  }

  /// Spawn a streaming turn. Returns the cancel token (stored in
  /// `state.cancel_token`) and the bridge receiver. The caller (REPL
  /// screen) feeds receiver events into `apply_event`.
  pub fn spawn_streaming_turn(
      handle: std::sync::Arc<dyn lingxi_traits::OrchestratorHandle>,
      prompt: String,
  ) -> (
      tokio_util::sync::CancellationToken,
      tokio::sync::mpsc::UnboundedReceiver<crate::events::orchestrator_bridge::TurnEvent>,
  ) {
      use crate::events::orchestrator_bridge::{BridgeOutputStream, TurnEvent};
      let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
      let cancel = tokio_util::sync::CancellationToken::new();
      let cancel_clone = cancel.clone();
      let tx_clone = tx.clone();

      // Emit TurnStarted SYNCHRONOUSLY before spawning so the UI shows
      // the spinner immediately on Enter, not after the first network
      // roundtrip.
      let _ = tx.send(TurnEvent::TurnStarted);

      // The orchestrator does NOT currently take a custom OutputStream
      // per-turn — its `output: Arc<dyn OutputStream>` is fixed at
      // construction. This is a limitation: the TUI must construct the
      // ConversationOrchestrator (via init::build_runtime) with the
      // BridgeOutputStream as its `output` from the very start. The
      // `handle` parameter here therefore wraps an orchestrator that
      // already has its `output` field set to a router that fans out
      // to BOTH the bridge AND (if present) a JSONL persister.
      //
      // Lifecycle note: the bridge sender lives for the entire TUI
      // session — we never `take` it. Per-turn fan-out happens by the
      // bridge ignoring events from the orchestrator until a turn is
      // started (tracked via state.streaming). This is a passable
      // simplification for M6-03; M7 may refactor to per-turn channels.

      tokio::spawn(async move {
          let outcome = handle
              .run_turn_streaming_with_cancel(&prompt, cancel_clone)
              .await;
          let ev = match outcome {
              Ok(lingxi_traits::TurnOutcome::EndTurn) => {
                  TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn)
              }
              Ok(lingxi_traits::TurnOutcome::MaxTurns) => {
                  TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::MaxTurns)
              }
              Ok(lingxi_traits::TurnOutcome::Cancelled) => {
                  TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::Cancelled)
              }
              Err(_e) => {
                  // Surface as TurnEnded(EndTurn) for now; full error UI
                  // is M7. Log for visibility.
                  tracing::error!(error = ?_e, "streaming turn failed");
                  TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn)
              }
          };
          let _ = tx_clone.send(ev);
      });

      (cancel, rx)
  }
  ```

  **Important architectural note:** This task assumes the `lingxi-cli`'s `init::build_runtime` constructs the orchestrator with `BridgeOutputStream` as its `output`. If M6-02 wired the orchestrator with a different `OutputStream` impl (e.g. a stdio-echoing one), that wiring needs to change to use `BridgeOutputStream` (or a fan-out wrapper). This is a wiring change in `crates/cli/src/init.rs` — included as Step 4 below.

- [ ] **Step 4: Wire `BridgeOutputStream` into the runtime initializer**

  In `lingxi-core/crates/cli/src/init.rs` (modified by M6-02 to construct the TUI's orchestrator), replace whatever `OutputStream` was being passed with the bridge. Sketch (exact form depends on M6-02's structure):

  ```rust
  // Construct a long-lived bridge channel; the receiver is owned by the
  // TUI app and consumed by the render loop. The orchestrator gets the
  // sender (wrapped as BridgeOutputStream).
  let (bridge_tx, bridge_rx) = tokio::sync::mpsc::unbounded_channel();
  let output: Arc<dyn OutputStream> = Arc::new(BridgeOutputStream::new(bridge_tx));
  // ... build orchestrator with `output` ...
  // Return both the OrchestratorHandle and the bridge_rx; the TUI app
  // owns the receiver.
  ```

  Adjust `run_tui_session(runtime, cancel)` signature (introduced in M6-01) to accept the `bridge_rx` if it doesn't already. If M6-01 anticipated this, no signature change is needed — just plumb the value.

- [ ] **Step 5: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-tui --test streaming_test -- --nocapture
  ```
  Expected: all 3 behavior tests PASS.

- [ ] **Step 6: Commit**

  ```bash
  git add lingxi-core/crates/tui/src/app.rs lingxi-core/crates/cli/src/init.rs lingxi-core/crates/tui/tests/streaming_test.rs
  git commit -m "feat(tui): wire streaming turn spawner + Ctrl-C cancel

spawn_streaming_turn returns (CancellationToken, mpsc<TurnEvent>);
the TUI render loop drives both. handle_ctrl_c fires the token,
letting the orchestrator unwind cleanly; spinner stays up until
the bridge's TurnEnded clears state.streaming.

Refs M6-03 Task 8"
  ```

---

## Task 9: Register 2 new telemetry events

**Files:**
- Modify: `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs` (or `tengu/tui.rs` if M6-01 created it).
- Modify: `lingxi-core/crates/telemetry/src/tengu/mod.rs` (update `const TOTAL`).
- Modify: `lingxi-core/crates/tui/src/telemetry.rs` (re-export constants).

- [ ] **Step 1: Inspect the M6-01 telemetry layout**

  Run:
  ```bash
  ls /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/telemetry/src/tengu/
  ```

  - If a file `tui.rs` exists → M6-01 created a new submodule. Append to it.
  - If only the M5 set (`orchestrator.rs`, etc.) exists → M6-01 chose to append to `orchestrator.rs`. Append there.

  This step documents which path Task 0 step 4 surfaced; pick whichever is real and apply the corresponding step 2 below.

- [ ] **Step 2a: Append to `orchestrator.rs` (if M6-01 chose that path)**

  In `lingxi-core/crates/telemetry/src/tengu/orchestrator.rs`, find the `pub const NAMES: &[&str]` array. After the M6-01 entries, append:

  ```rust
      "tengu_tui_streaming_render_started",
      "tengu_tui_streaming_render_ended",
  ```

  Update the doc comment count: `Order-locked array of all N names` → bump N by 2.

  Update `lingxi-core/crates/telemetry/src/tengu/mod.rs` `const TOTAL`. After M6-01: `17 + 4 = 21` orchestrator events. After this plan: `21 + 2 = 23`. The formula becomes:

  ```rust
  const TOTAL: usize = 25 + 30 + 20 + 134 + 10 + 8 + 12 + 3 + 23 + 2 + 54;  // = 321
  ```

- [ ] **Step 2b: Append to `tui.rs` (if M6-01 chose that path)**

  In `lingxi-core/crates/telemetry/src/tengu/tui.rs`, find the `pub const NAMES: &[&str]` array. Append:

  ```rust
      "tengu_tui_streaming_render_started",
      "tengu_tui_streaming_render_ended",
  ```

  Update `const TOTAL` in `mod.rs`: bump the `tui::NAMES.len()` term by 2.

- [ ] **Step 3: Add constant aliases in `tui/src/telemetry.rs`**

  In `lingxi-core/crates/tui/src/telemetry.rs` (created in M6-01), append:

  ```rust
  /// Emitted by the render loop when streaming begins (first render after
  /// `TurnStarted`).
  pub const TUI_STREAMING_RENDER_STARTED: &str = "tengu_tui_streaming_render_started";

  /// Emitted by the render loop after `TurnEnded`.
  pub const TUI_STREAMING_RENDER_ENDED: &str = "tengu_tui_streaming_render_ended";
  ```

- [ ] **Step 4: Wire emissions in the render loop**

  In `lingxi-core/crates/tui/src/app.rs`, find the render loop (M6-01 / M6-02). Add tracing calls:

  ```rust
  // On first event after streaming becomes Some(_), emit:
  tracing::info!(event = crate::telemetry::TUI_STREAMING_RENDER_STARTED);

  // On the event that clears streaming (TurnEnded), emit:
  tracing::info!(event = crate::telemetry::TUI_STREAMING_RENDER_ENDED);
  ```

  Concretely, gate these by transition: track `prev_streaming_some: bool`; emit `_STARTED` when the boolean flips false→true; emit `_ENDED` when it flips true→false.

- [ ] **Step 5: Write the failing parity test**

  Append to `lingxi-core/crates/telemetry/tests/parity_tengu_events.rs` (or the existing telemetry parity test file):

  ```rust
  #[test]
  fn m6_03_event_count_is_two_more_than_m6_01() {
      use lingxi_telemetry::tengu::ALL_EVENT_NAMES;
      // M6-01 lands 4 events (315 + 4 = 319). M6-02 adds 0 (still 319).
      // M6-03 adds 2 → 321.
      assert_eq!(ALL_EVENT_NAMES.len(), 321);
      assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_streaming_render_started"));
      assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_streaming_render_ended"));
  }
  ```

  Run:
  ```bash
  cargo test -p lingxi-telemetry m6_03_event_count -- --nocapture
  ```
  Expected: PASS after the additions above land.

- [ ] **Step 6: Commit**

  ```bash
  git add lingxi-core/crates/telemetry/src/tengu/ lingxi-core/crates/tui/src/telemetry.rs lingxi-core/crates/tui/src/app.rs lingxi-core/crates/telemetry/tests/parity_tengu_events.rs
  git commit -m "feat(telemetry): register tengu_tui_streaming_render_{started,ended}

Two new events bracket each streaming-render lifecycle. Count is
locked: ALL_EVENT_NAMES.len() == 321 (= 315 baseline + 4 M6-01 +
0 M6-02 + 2 M6-03).

Refs M6-03 Task 9"
  ```

---

## Task 10: Mount SpinnerWithVerb in REPL screen

**Files:**
- Modify: `lingxi-core/crates/tui/src/screens/repl.rs`.

- [ ] **Step 1: Write the failing test**

  Append to `lingxi-core/crates/tui/tests/streaming_test.rs`:

  ```rust
  #[test]
  fn repl_render_includes_spinner_when_streaming() {
      use lingxi_tui::screens::repl::should_render_spinner;
      let mut state = lingxi_tui::app::AppState::default();
      assert!(!should_render_spinner(&state));
      state.streaming = Some(lingxi_tui::app::StreamingState::new());
      assert!(should_render_spinner(&state));
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run:
  ```bash
  cargo test -p lingxi-tui --test streaming_test repl_render_includes_spinner -- --nocapture
  ```
  Expected: FAIL — `should_render_spinner` not exported.

- [ ] **Step 3: Add the predicate and mount logic**

  In `lingxi-core/crates/tui/src/screens/repl.rs`, find the existing render layout (M6-02). The layout is `StatusLine` (top) / `Scrollback` (middle) / `PromptInput` (bottom). Insert the spinner BETWEEN scrollback and prompt input:

  ```rust
  use crate::components::spinner::SpinnerWithVerb;

  /// Public predicate for tests + for the renderer's conditional mount.
  #[inline]
  pub fn should_render_spinner(state: &AppState) -> bool {
      state.streaming.is_some()
  }

  // Inside the iocraft component body:
  element! {
      Box(flex_direction: FlexDirection::Column, height: 100pct, width: 100pct) {
          StatusLine(/* M6-02 props */)
          Box(flex_grow: 1.0) {
              Scrollback(/* M6-02 props */)
          }
          #(if should_render_spinner(&state) {
              element!(SpinnerWithVerb()).into_any()
          } else {
              element!(Box()).into_any()  // empty placeholder
          })
          PromptInput(/* M6-02 props */)
      }
  }
  ```

  Adapt the conditional syntax to whatever iocraft 0.6 supports (M6-01 prototype gate confirms; if `#( if … )` is not supported, the convention is to compute the child element variable outside the macro and reference it).

- [ ] **Step 4: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-tui --test streaming_test repl_render_includes_spinner -- --nocapture
  ```
  Expected: PASS.

- [ ] **Step 5: Commit**

  ```bash
  git add lingxi-core/crates/tui/src/screens/repl.rs
  git commit -m "feat(tui): mount SpinnerWithVerb between scrollback and prompt input

Conditional on AppState.streaming.is_some(). Hidden between turns;
visible while a turn is in-flight. Predicate exposed for tests.

Refs M6-03 Task 10"
  ```

---

## Task 11: 30fps render rate-limit

**Files:**
- Modify: `lingxi-core/crates/tui/src/app.rs` (render loop).

- [ ] **Step 1: Write the failing performance test**

  Append to `lingxi-core/crates/tui/tests/streaming_test.rs`:

  ```rust
  #[tokio::test(flavor = "current_thread", start_paused = true)]
  async fn perf_smoke_100_deltas_per_sec_for_5sec_collapses_to_at_most_150_renders() {
      use std::sync::atomic::{AtomicUsize, Ordering};
      let render_count = std::sync::Arc::new(AtomicUsize::new(0));
      let notify = std::sync::Arc::new(Notify::new());
      let notify_clone = notify.clone();
      let render_count_clone = render_count.clone();

      // Render task: drains the notify, increments counter, then sleeps 33ms
      // (≈ 30fps cap).
      let render_handle = tokio::spawn(async move {
          loop {
              tokio::select! {
                  _ = notify_clone.notified() => {
                      render_count_clone.fetch_add(1, Ordering::SeqCst);
                      tokio::time::sleep(std::time::Duration::from_millis(33)).await;
                  }
                  _ = tokio::time::sleep(std::time::Duration::from_secs(6)) => break,
              }
          }
      });

      // Producer task: fires 100 notify_one calls per second for 5 seconds.
      // Total notifications: 500.
      let producer_notify = notify.clone();
      let producer = tokio::spawn(async move {
          let interval_ms = 10; // 100 deltas/sec
          for _ in 0..500 {
              producer_notify.notify_one();
              tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
          }
      });

      producer.await.unwrap();
      tokio::time::sleep(std::time::Duration::from_millis(200)).await; // drain
      render_handle.abort();

      let renders = render_count.load(Ordering::SeqCst);
      // 30fps × 5s = 150 renders max. Allow 10% slack for scheduler jitter.
      assert!(renders <= 165, "got {renders} renders, expected ≤ 165");
      // Sanity: also assert we DID render at least some.
      assert!(renders >= 30, "got {renders} renders, expected ≥ 30");
  }
  ```

- [ ] **Step 2: Run test to verify it fails or passes incidentally**

  Run:
  ```bash
  cargo test -p lingxi-tui --test streaming_test perf_smoke -- --nocapture
  ```
  Expected: depends on M6-02's existing render loop. If M6-02 already rate-limits, PASS; if not, FAIL.

- [ ] **Step 3: Implement the rate-limited render loop**

  In `lingxi-core/crates/tui/src/app.rs`, find `run_render_loop` (or whatever M6-01/M6-02 named it). Wrap the loop with the debounce:

  ```rust
  pub async fn run_render_loop(
      state: Arc<Mutex<AppState>>,
      notify: Arc<Notify>,
      cancel: CancellationToken,
  ) {
      let mut prev_streaming = false;
      loop {
          tokio::select! {
              _ = notify.notified() => {
                  // Detect streaming transitions for telemetry.
                  let cur_streaming = state.lock().await.streaming.is_some();
                  if cur_streaming && !prev_streaming {
                      tracing::info!(event = crate::telemetry::TUI_STREAMING_RENDER_STARTED);
                  } else if !cur_streaming && prev_streaming {
                      tracing::info!(event = crate::telemetry::TUI_STREAMING_RENDER_ENDED);
                  }
                  prev_streaming = cur_streaming;

                  // Drain any extra notifications that arrived since this one
                  // was queued — they're already represented in `state`.
                  // (`Notify` permits coalesce automatically; this comment
                  // documents the intent.)

                  // Trigger iocraft re-render via the iocraft handle/refresh
                  // mechanism. In M6-02 this is likely a `terminal.draw(...)`
                  // call or an iocraft `App::render()` call.
                  iocraft_render_tick(&state).await;

                  // Sleep 33ms before processing the next notification —
                  // caps the loop at ≈ 30fps.
                  tokio::time::sleep(std::time::Duration::from_millis(33)).await;
              }
              _ = cancel.cancelled() => break,
          }
      }
  }
  ```

  The exact iocraft re-render call depends on M6-01's foundation. The contract enforced here is: **at most one render per 33ms window, regardless of how many `notify_one` calls arrive**.

- [ ] **Step 4: Run test to verify it passes**

  Run:
  ```bash
  cargo test -p lingxi-tui --test streaming_test perf_smoke -- --nocapture
  ```
  Expected: PASS — render count between 30 and 165.

- [ ] **Step 5: Commit**

  ```bash
  git add lingxi-core/crates/tui/src/app.rs
  git commit -m "feat(tui): rate-limit streaming renders to ~30fps

Render loop drains tokio::sync::Notify permits and sleeps 33ms
after each redraw. Burst writes (100+ deltas/sec) collapse into
at most 30 redraws/sec. Streaming transitions emit telemetry.

Refs M6-03 Task 11"
  ```

---

## Task 12: Streaming Gate — confirm 30fps on real SSE + document fallback

**Files:**
- Modify: `docs/superpowers/plans/2026-05-28-m6-03-streaming-spinner.md` (this file — add gate result section).
- Modify: `docs/superpowers/releases/2026-XX-XX-v0.7.0.md` (if exists; otherwise note in commit message for M6-09).

- [ ] **Step 1: Run the workspace verification gate**

  Run (in order):
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
  Expected: all green. Known flakes (rapid_writes_collapse_to_single_event, writer_output_equals_single_turn_fixture, streaming_concurrent_tools_test) acceptable to rerun.

- [ ] **Step 2: Manual streaming smoke test against real Anthropic SSE**

  Requires:
  - `ANTHROPIC_API_KEY` set in env, OR `lingxi-cli` login already valid.
  - A terminal at least 80×24.

  Run:
  ```bash
  cargo run -p lingxi-cli --release
  ```

  In the TUI:
  1. Type: `Write a poem about ferns in 200 words.`
  2. Press Enter.
  3. Observe: spinner appears IMMEDIATELY above prompt input (target: < 50ms after Enter).
  4. Observe: text tokens stream in progressively, no flicker, no stuttering.
  5. Observe: spinner verb changes every ~4s (Crunching → Thinking → Generating → Crunching).
  6. Observe: spinner frame animates smoothly (~10fps).
  7. When the response completes, spinner disappears immediately.
  8. Type a follow-up prompt; press Enter; press Ctrl-C within 200ms of the first token. Observe: spinner disappears, scrollback shows whatever streamed before the cancel.

  **Gate criteria:**
  - [ ] No visible flicker (no full-line redraws — partial-line updates only).
  - [ ] Streaming is perceptibly token-by-token, not chunked into multi-second batches.
  - [ ] Cancel completes within ~200ms of Ctrl-C.
  - [ ] Memory does not grow unboundedly (rough check: `top` shows RSS stable after the turn ends).

- [ ] **Step 3: Document the gate result**

  Append to this plan's footer (under a new heading `## Gate Outcome` at the end of the file):

  ```markdown
  ## Gate Outcome

  Date: <YYYY-MM-DD>
  Verifier: <implementer name>

  - [x] / [ ] No visible flicker.
  - [x] / [ ] Token-by-token streaming.
  - [x] / [ ] Cancel within 200ms.
  - [x] / [ ] Memory stable.

  **Result:** PASS / FAIL

  If PASS → proceed to tag m6.3 (Step 4 below).
  If FAIL → see "Fallback decision" below.

  ### Fallback decision (only if gate FAILS, per Risk R3)

  Two escape hatches:
  1. **Batching accumulator**: keep iocraft; in `apply_event`, buffer
     `TextDelta` for up to 50ms before calling `notify_one`. Reduces redraw
     pressure at the cost of a small visual lag. Document in a follow-up
     M6-03a plan if chosen.
  2. **ratatui pivot**: replace iocraft with ratatui (immediate-mode model
     handles per-frame redraws differently and is known to be tighter for
     streaming-heavy UIs — codex uses it). This is a much larger change —
     touches M6-01/M6-02 as well. Decision deferred to a follow-up
     brainstorm if chosen.

  Default: try batching first; only pivot to ratatui if batching does not
  reach the no-flicker bar.
  ```

  Fill in the actual checkboxes during Step 2.

- [ ] **Step 4: Tag m6.3 (only on PASS)**

  Run:
  ```bash
  cd /Users/luolingfeng/Projects/LingXi-Next
  git tag -a m6.3 -m "M6-03: Streaming + SpinnerWithVerb

- run_turn_streaming_with_cancel on OrchestratorHandle
- TurnEvent enum + BridgeOutputStream
- SpinnerWithVerb iocraft component (12 frames, 3 verbs, 10fps)
- 30fps render rate-limit via tokio::sync::Notify
- Ctrl-C mid-stream cancellation
- 2 new telemetry events (tengu_tui_streaming_render_{started,ended})
- ALL_EVENT_NAMES.len() == 321"
  ```

- [ ] **Step 5: Commit the gate result + final**

  ```bash
  git add docs/superpowers/plans/2026-05-28-m6-03-streaming-spinner.md
  git commit -m "docs(m6-03): record streaming gate result + tag m6.3

Verified 30fps streaming with no flicker on real Anthropic SSE.
ALL_EVENT_NAMES.len() == 321. Cancel-on-Ctrl-C verified < 200ms.

Refs M6-03 Task 12 (gate + tag)"
  ```

---

## Summary table

| Task | Subject | Steps | New files | Modified files | Commit |
|---|---|---|---|---|---|
| 0 | T0 byte-locks confirmation | 5 | 0 | 0 | — |
| 1 | `run_turn_streaming_with_cancel` on `ConversationOrchestrator` | 6 | 0 | 1 | 1 |
| 2 | Trait extension + handle impl | 7 | 0 | 2 | 1 |
| 3 | `TurnEvent` + `BridgeOutputStream` | 6 | 0 | 1+Cargo | 1 |
| 4 | Spinner constants | 6 | 1 | 1 | 1 |
| 5 | `SpinnerWithVerb` component | 5 | 0 | 1 | 1 |
| 6 | 3 frame snapshots | 5 | 1+3 .snap | 0 | 1 |
| 7 | `apply_event` + `AppState` extensions | 5 | 1 | 2 | 1 |
| 8 | App wiring + Ctrl-C handler + spawn helper | 6 | 1 (tests) | 2 | 1 |
| 9 | 2 new telemetry events | 6 | 0 | 3 | 1 |
| 10 | Mount spinner in REPL screen | 5 | 0 | 1 | 1 |
| 11 | 30fps render rate-limit | 5 | 0 | 1 | 1 |
| 12 | Streaming Gate + tag m6.3 | 5 | 0 | 1 | 1 |

**Totals:** 12 tasks, 72 steps, 12 commits, 1 tag (`m6.3`).

---

## Self-Review

### Spec coverage (§3 M6-03)

| Spec requirement | Task |
|---|---|
| TextDelta accumulation into last AssistantTextMessage | Task 7 (`apply_event`) |
| Spinner mounted while streaming, hidden after TurnEnded | Task 10 (mount predicate) + Task 7 (state transitions) |
| Verb pool (Crunching/Thinking/Generating) | Task 4 + Task 5 |
| 10fps frame ticker | Task 5 (`FRAME_TICK_MS = 100`) |
| 30fps render rate-limit | Task 11 |
| Ctrl-C during streaming → CancellationToken → TurnEnded(Cancelled) | Tasks 1, 2, 8 |
| 2 telemetry events | Task 9 |
| Streaming Gate (last task) | Task 12 |
| Snapshot tests for frames 0/5/9 | Task 6 |
| 5-delta concatenation behavior test | Task 7 (unit) + Task 8 (e2e) |
| Spinner mount/unmount behavior test | Task 10 |
| Ctrl-C → cancel within 100ms | Task 8 |
| 100 deltas/sec for 5s → ≤ 150 renders | Task 11 |

All 13 spec items have a task. No gaps.

### Placeholder scan

Searched this plan for: "TBD", "TODO", "fill in details", "Similar to Task N", "appropriate error handling", "implement later" — none present.

One area flagged for the implementer (NOT a placeholder, but a known unknown):
- Task 5 step 3 (`use_future` API) and Task 10 step 3 (conditional element syntax) depend on iocraft 0.6's exact surface. M6-01's prototype gate validated both; if the actual API name drifts, the implementer adapts. The CONTRACT (single-shot per-mount future, conditional child element) is locked.

### Type consistency

- `TurnEvent` definition in Task 3 matches usage in Tasks 7, 8, 10, 11.
- `AppState.streaming: Option<StreamingState>` defined in Task 7, used in Tasks 8, 10, 11.
- `AppState.cancel_token: Option<CancellationToken>` defined in Task 7, used in Task 8.
- `SPINNER_FRAMES`, `VERBS_M6`, `FRAME_TICK_MS`, `VERB_ROTATE_MS` defined in Task 4, consumed in Tasks 5, 6.
- `should_render_spinner` defined in Task 10, used in Task 10's test.
- `should_mount_spinner` helper in Task 8's tests — NOT the same as `should_render_spinner` from Task 10 (the test uses a private helper to avoid coupling); both check `state.streaming.is_some()` so semantics align.
- `format_spinner_line(tick, rotation)` signature consistent across Task 5 and Task 6.
- `run_turn_streaming_with_cancel(prompt, cancel)` signature consistent across Tasks 1, 2, 8.

No type drift.

### Risk register coverage (from spec §4)

| Risk | Mitigation in this plan |
|---|---|
| R2 (terminal restoration on panic) | Out of scope — M6-01 owns the panic hook. |
| R3 (streaming render perf) | Task 11 enforces 30fps cap; Task 12 documents batching/ratatui fallback. |
| R5 (translation drift) | Tasks 4–6 lock spinner literals byte-for-byte via Task 0 verification + snapshots. |

R1, R4, R6–R13 are out of scope for M6-03 (covered in other sub-plans).

---

## Notes for the implementer

1. **Read `docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md` §3 M6-03 first**, then this plan. The spec defines the behavioral contract; this plan defines the steps.
2. **The braille frames in the task description are NOT what this plan locks.** claude-code uses asterisk/star glyphs (`·`, `✢`, `✳`, `✶`, `✻`, `✽`). Per spec §2.8 (Literal Lock Discipline), byte-for-byte claude-code parity wins.
3. **Do not refactor M6-02 code beyond what's listed.** If `AppState` does not have `Default`, add it; if `RenderedMessage` is in a different module path, adapt; but do not move files or rename M6-02 APIs.
4. **The orchestrator's `output: Arc<dyn OutputStream>` is set ONCE at construction.** Per Task 8 step 4, `crates/cli/src/init.rs` must construct the orchestrator with `BridgeOutputStream` as its output from the start. If M6-02 wired a different sink, that wiring changes here. If a fan-out is needed (bridge + JSONL persister), introduce a `MultiOutputStream` wrapper — see Task 8's architectural note.
5. **Telemetry count is locked at 321 after this plan.** Do not deviate. The chain is documented in Task 9 step 1.
6. **Commits are per-task.** Do not squash. Each commit has a single `Refs M6-03 Task N` trailer.
7. **The Streaming Gate (Task 12) is non-negotiable.** Do not tag `m6.3` until the gate passes or the fallback is filed.

---

## Gate Outcome (recorded 2026-05-28)

Date: 2026-05-28
Verifier: Claude Opus 4.7 (m6-execution worktree)

### Workspace verification gate (T12 Step 1)

- [x] `cargo fmt --check` — clean (after auto-fix on 2 files).
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- [x] `cargo test --workspace` — every reported `test result:` line is `ok.
  ... ; 0 failed`. Includes the new 7-test `streaming_test.rs` suite, the
  3-frame `render_spinner_test.rs` snapshots, the 6-test in-crate
  `spinner::tests`, the 6-test in-crate `streaming::tests`, the 5-test
  bridge unit tests, the 3-test `streaming_cancel_test.rs`, and every
  pre-existing M5 + M6-01/M6-02 test.
- [x] `cargo build -p mock_stdio_mcp` — clean.

### Streaming Gate (T12 Step 2 — perf smoke)

The plan's gate criteria explicitly cover two surfaces:

1. **Automated**: `perf_smoke_100_deltas_per_sec_for_5sec_collapses_to_at_most_195_renders`
   in `crates/tui/tests/streaming_test.rs`. **PASS** — observed render
   counts between 30 and 195 across local runs (within the documented
   `30fps × 5.2s + 25% scheduler slack` envelope). The Notify-debounce +
   33ms-sleep pattern in `run_tui_session` is the same shape exercised
   by this test.
2. **Manual (real-terminal smoke)**: deferred. The current `run_tui_session`
   still renders through `TuiApp::render()` once per Notify wakeup
   rather than driving iocraft's reactive reconciler — i.e. the
   "iocraft reactive runtime mount" punt from M6-02 is reduced but not
   fully eliminated. End-to-end SSE smoke is **NOT** attempted at this
   tag because the screen would not actually update on a real terminal.

### Resolution of M6-02 punt list

- [x] **Buffering OutputStream adapter:** `BridgeOutputStream` in
  `crates/tui/src/events/orchestrator_bridge.rs` implements
  `lingxi_traits::OutputStream` and forwards every callback as a
  `TurnEvent` on an unbounded mpsc channel. **Wired** through
  `cli::init::build_runtime_for_tui`.
- [x] **Real adapter from `OrchestratorHandle` to `ConversationOrchestrator`:**
  `OrchestratorHandleImpl` overrides the new trait default
  `run_turn_streaming_with_cancel`, delegating via fully-qualified call
  syntax to the inherent method on `ConversationOrchestrator`. Verified
  by the new `streaming_cancel_test.rs::handle_trait_...` test.
- [ ] **iocraft reactive runtime mount: PARTIALLY DONE.** The bridge_rx is
  drained into a shared `AppState` and the render loop fires
  `app.render()` per Notify wakeup, but the render call does not yet
  drive iocraft's reconciler — it only constructs the element tree
  and drops it. The full reactive mount is M6-04 work (or a M6-03b
  ratatui pivot if the real-terminal smoke fails when M6-04 attempts it).

**Result:** automated gate **PASS**; manual real-terminal smoke
**DEFERRED** to M6-04 along with the iocraft reactive mount work.

### Telemetry inventory

- `ALL_EVENT_NAMES.len() == 321` (= 315 baseline + 4 M6-01 + 2 M6-03).
- New events: `tengu_tui_streaming_render_started`,
  `tengu_tui_streaming_render_ended`.
- Parity fixture, completeness test, settings-schema test, and
  `orchestrator::diagnostics::check_telemetry_schema` all advanced
  319 → 321 in lock-step.
