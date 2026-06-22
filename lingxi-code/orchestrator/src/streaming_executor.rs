//! Faithful port of claude-code's `StreamingToolExecutor`
//! (`services/tools/StreamingToolExecutor.ts`). Schedules tool execution as
//! `tool_use` blocks stream in, under concurrency control, buffering results
//! for emission in *received* order. Single-task: all tool futures borrow
//! `&ConversationOrchestrator` and are polled on one `FuturesUnordered`, so no
//! `'static`/spawn is required.

use crate::conversation::ConversationOrchestrator;
use futures::{stream::FuturesUnordered, StreamExt};
use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use tool_api::ContextModifier;

/// claude-code `REJECT_MESSAGE` (utils/messages.ts:212). The user-interrupted
/// synthetic result is this BARE text (NOT `<tool_use_error>`-wrapped, unlike
/// sibling_error/streaming_fallback). The optional memoryCorrectionHint is gated
/// off by default in claude-code, so it is not appended.
const REJECT_MESSAGE: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";

/// Result of one `dispatch_tool_uses_tracked` call routed through the executor:
/// the single result block + the tool's injected messages + context modifiers.
type DispatchOutcome = Result<
    (
        ContentBlock,
        Vec<(ConversationMessage, ToolUseId)>,
        Vec<ContextModifier>,
    ),
    crate::error::OrchestratorError,
>;

/// Why a tracked tool is being cancelled (TS `getAbortReason`). v2.1.183's
/// `getAbortReason` returns exactly `"streaming_fallback"` (discarded) or
/// `"user_interrupted"` — there is NO sibling/parallel-error abort reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbortReason {
    /// Emitted when the user interrupts (ESC / new message) an in-flight or
    /// queued tool whose `interrupt_behavior()==Cancel` (TS `getAbortReason`
    /// 'user_interrupted'). The synthetic result is the bare `REJECT_MESSAGE`.
    UserInterrupted,
    StreamingFallback,
}

/// Build the synthetic `tool_result` for a cancelled tool (TS
/// `createSyntheticErrorMessage`). `provider_tool_use_id` is left `None` —
/// the caller copies the tracked tool's `provider_id` in before persisting.
pub(crate) fn synthetic_error_block(tool_use_id: ToolUseId, reason: AbortReason) -> ContentBlock {
    let content = match reason {
        AbortReason::StreamingFallback => {
            "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>"
                .to_string()
        }
        // claude-code (StreamingToolExecutor.ts:160-172) uses the BARE REJECT_MESSAGE
        // here — NOT `<tool_use_error>`-wrapped — with is_error: true. This is the
        // faithful text; the `UserInterrupted` reason itself is only produced once
        // the user-ESC / per-tool cancellation path is wired in a later sub-task.
        AbortReason::UserInterrupted => REJECT_MESSAGE.to_string(),
    };
    ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error: true,
        provider_tool_use_id: None,
        content_blocks: None,
    }
}

/// Lifecycle of one tracked tool, mirroring TS `ToolStatus`.
#[allow(dead_code)] // variants wired in Tasks 5-11
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolStatus {
    Queued,
    Executing,
    Completed,
    Yielded,
}

/// One `tool_use` block under management. `assistant_id` is the id of the
/// assistant message that requested this call — it becomes the JSONL
/// `parentUuid` of the result (TS `sourceToolAssistantUUID`).
#[allow(dead_code)] // fields wired in Tasks 5-11
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

// ============================================================================
// Task 7: concurrency gate + ordered process_queue
// ============================================================================

/// TS `canExecuteTool` (line 129-135): a tool may start if nothing is
/// executing, OR if both the candidate and every executing tool are
/// concurrency-safe.
///
/// `executing_safe_flags` is a slice of the `is_concurrency_safe` flags for
/// every tool currently in the `Executing` state.
pub(crate) fn can_execute(executing_safe_flags: &[bool], candidate_safe: bool) -> bool {
    executing_safe_flags.is_empty() || (candidate_safe && executing_safe_flags.iter().all(|&s| s))
}

/// Default maximum number of concurrency-safe tools to execute simultaneously.
/// Byte-locked to the v2.1.183 binary's `r1p` getter:
/// ```js
/// function r1p() {
///   let e = parseInt(process.env.CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY || "", 10);
///   return e > 0 ? e : 10;
/// }
/// ```
pub(crate) const DEFAULT_MAX_TOOL_USE_CONCURRENCY: usize = 10;

/// Resolve the maximum number of concurrency-safe tools to run at once,
/// mirroring the binary's `r1p()`. Reads `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY`
/// and uses it only when it parses to a value `> 0`; otherwise the default of
/// [`DEFAULT_MAX_TOOL_USE_CONCURRENCY`] (10).
///
/// Injectable form for tests: [`max_tool_use_concurrency_from`].
pub(crate) fn max_tool_use_concurrency() -> usize {
    max_tool_use_concurrency_from(
        std::env::var("CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY")
            .ok()
            .as_deref(),
    )
}

/// Pure resolver for [`max_tool_use_concurrency`] — `parseInt(v, 10) > 0 ? v : 10`.
/// `parseInt` semantics: leading numeric prefix is parsed (e.g. `"5x"` → 5),
/// non-numeric / absent / `<= 0` → the default.
pub(crate) fn max_tool_use_concurrency_from(raw: Option<&str>) -> usize {
    // Mirror JS `parseInt(s, 10)`: take the leading (optionally signed) integer
    // prefix. Anything else (NaN) falls through to the default.
    let parsed: Option<i64> = raw.and_then(|s| {
        let t = s.trim_start();
        let bytes = t.as_bytes();
        let mut end = 0;
        if matches!(bytes.first(), Some(b'+' | b'-')) {
            end = 1;
        }
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        // Need at least one digit after the optional sign.
        let has_digit = bytes[..end].iter().any(u8::is_ascii_digit);
        if has_digit {
            t[..end].parse::<i64>().ok()
        } else {
            None
        }
    });
    match parsed {
        Some(n) if n > 0 => n as usize,
        _ => DEFAULT_MAX_TOOL_USE_CONCURRENCY,
    }
}

// ============================================================================
// StreamingToolExecutor — Task 6: struct + new() + add_tool()
// ============================================================================

/// Manages the lifecycle of all `tool_use` blocks from one streaming assistant
/// turn. Mirrors TS `StreamingToolExecutor` (`StreamingToolExecutor.ts:76-124`
/// for `addTool`).
///
/// All fields are now read by the Task 8 dispatch/abort logic.
pub(crate) struct StreamingToolExecutor<'a> {
    orch: &'a ConversationOrchestrator,
    pub(crate) tools: Vec<TrackedTool>,
    /// Set when the turn is discarded (streaming fallback); all queued tools
    /// are cancelled with `AbortReason::StreamingFallback` (Task 8).
    discarded: bool,
    /// In-flight tool futures keyed by their index in `tools`. Polled on the
    /// current task (no spawn); each borrows `&'a orch`. `+ Send` so the whole
    /// executor (held across `.await` in the live streaming turn) stays `Send`,
    /// matching the `Send` turn future required by the handle traits.
    inflight: FuturesUnordered<
        std::pin::Pin<Box<dyn std::future::Future<Output = (usize, DispatchOutcome)> + Send + 'a>>,
    >,
    /// Parent cancellation token carrying cancellation into in-flight tools.
    /// Each dispatched tool receives a `child_token()` of this, threaded into
    /// its `ToolUseContext::cancel`. It fires when the turn is **discarded**
    /// (streaming fallback) and — being a child of `user_cancel` when one is
    /// supplied — when the **user interrupts**. On either, an in-flight tool
    /// observing the token returns early (a Bash SIGKILLs its subprocess via
    /// `kill_on_drop` and returns `Aborted`), whose real outcome `drain_one`
    /// then substitutes with the synthetic abort block.
    tool_abort: tokio_util::sync::CancellationToken,
    /// DEFERRED-3: the turn's USER-interrupt token (ESC / new message), mirroring
    /// claude-code's `toolUseContext.abortController` with reason 'interrupt'.
    /// `None` outside the live streaming turn (executor unit tests + the test-only
    /// `run_to_completion` path), so those are byte-identical to before. When
    /// `Some` and fired, `abort_reason_for` substitutes `UserInterrupted` for
    /// every tool whose `interrupt_behavior()==Cancel` (queued via
    /// `apply_abort_to_pending`, in-flight via `drain_one`). To also deliver the
    /// token into each in-flight tool's `ctx.cancel` (so a Cancel-behavior tool
    /// observes it and returns early), `tool_abort` is parented to this token
    /// in `new_with_user_cancel` — a child token fires when its parent fires.
    user_cancel: Option<tokio_util::sync::CancellationToken>,
}

impl<'a> StreamingToolExecutor<'a> {
    /// Construct a fresh executor borrowing the given orchestrator for the
    /// duration of the streaming turn.
    pub(crate) fn new(orch: &'a ConversationOrchestrator) -> Self {
        Self {
            orch,
            tools: Vec::new(),
            discarded: false,
            inflight: FuturesUnordered::new(),
            tool_abort: tokio_util::sync::CancellationToken::new(),
            user_cancel: None,
        }
    }

    /// DEFERRED-3: construct with the turn's USER-interrupt token (ESC / new
    /// message). `tool_abort` is made a CHILD of `user_cancel`, so firing the
    /// user token also cancels every per-tool child (an in-flight Cancel-behavior
    /// tool observes its `ctx.cancel` and returns early); `abort_reason_for` then
    /// substitutes the bare `REJECT_MESSAGE` for Cancel-behavior tools.
    /// Mirrors claude-code's `createChildAbortController` where the user abort
    /// lives on the parent `toolUseContext.abortController`.
    pub(crate) fn new_with_user_cancel(
        orch: &'a ConversationOrchestrator,
        user_cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        let tool_abort = user_cancel.child_token();
        Self {
            orch,
            tools: Vec::new(),
            discarded: false,
            inflight: FuturesUnordered::new(),
            tool_abort,
            user_cancel: Some(user_cancel),
        }
    }

    /// Register one `tool_use` block received from the stream.
    ///
    /// - If the tool is **not in the registry**, a `TrackedTool` already
    ///   `Completed` is pushed with an unknown-tool error block (short-circuit,
    ///   mirrors TS `addTool` lines 76-85).
    /// - If the tool **is known**, classify `is_concurrency_safe` via the tool's
    ///   own method and push a `Queued` entry (mirrors TS lines 86-124).
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
                let block = synthetic_unknown_tool(id.clone(), &name, provider_id.clone());
                self.tools.push(TrackedTool {
                    id,
                    name,
                    input,
                    provider_id,
                    assistant_id,
                    status: ToolStatus::Completed,
                    is_concurrency_safe: true,
                    result: Some(block),
                    injected: Vec::new(),
                    modifiers: Vec::new(),
                });
            }
            Some(tool) => {
                // NOTE divergence: claude-code parses input against the schema
                // first and marks unparseable input `isConcurrencySafe=false`.
                // LingXi's is_concurrency_safe takes raw &Value and returns the
                // tool's conservative default on malformed input — acceptable
                // for Phase 1.
                let safe = tool.is_concurrency_safe(&input);
                self.tools.push(TrackedTool {
                    id,
                    name,
                    input,
                    provider_id,
                    assistant_id,
                    status: ToolStatus::Queued,
                    is_concurrency_safe: safe,
                    result: None,
                    injected: Vec::new(),
                    modifiers: Vec::new(),
                });
            }
        }
    }

    /// TS `processQueue` (line 140-151): walk the queue IN ORDER; start each
    /// queued tool whose concurrency conditions are met; STOP at the first
    /// queued non-concurrency-safe tool that cannot start yet (preserves
    /// exclusive-tool ordering). After each start the executing set changes, so
    /// we re-evaluate from scratch.
    ///
    /// ## Concurrency cap (parity binary `r1p` / `i1p`)
    ///
    /// The v2.1.183 binary runs a contiguous concurrency-safe group through
    /// `i1p`, which merges the per-tool generators with a bounded window of
    /// `r1p()` (`CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY`, default 10). So no more
    /// than N concurrency-safe tools execute simultaneously; the rest of the
    /// group waits for a slot. We enforce the same bound here: a queued safe
    /// tool may only start when the number of currently-`Executing`
    /// concurrency-safe tools is below the cap. When the cap is reached no
    /// further safe tool starts this pass (and an unsafe tool is barriered by
    /// the executing safe tools), so the queue stalls until a completion frees
    /// a slot — at which point the streaming loop re-invokes `process_queue`.
    // Index loop + per-pass rebuild are forced by the borrow checker: `start_tool`
    // takes `&mut self`, so we can't hold an iterator borrow over `self.tools`
    // across a start. N is small (tools per turn), so the rebuild is negligible.
    #[allow(clippy::needless_range_loop)]
    pub(crate) fn process_queue(&mut self) {
        // Resolve the cap once per call (env-driven; `r1p()`).
        let max_safe = max_tool_use_concurrency();
        loop {
            let executing_flags: Vec<bool> = self
                .tools
                .iter()
                .filter(|t| t.status == ToolStatus::Executing)
                .map(|t| t.is_concurrency_safe)
                .collect();
            // Count concurrency-safe tools already in flight (all executing
            // tools are safe whenever a safe candidate could start, but count
            // explicitly so the bound is correct regardless).
            let executing_safe_count = executing_flags.iter().filter(|&&s| s).count();

            let mut started_any = false;
            for i in 0..self.tools.len() {
                if self.tools[i].status != ToolStatus::Queued {
                    continue;
                }
                let safe = self.tools[i].is_concurrency_safe;
                // Concurrency cap: do not start an (N+1)th simultaneous safe
                // tool — keep it queued. An unsafe tool is unbounded (it runs
                // alone behind the barrier), so the cap applies only to safe.
                if safe && executing_safe_count >= max_safe {
                    // At the safe-concurrency ceiling: this safe tool waits.
                    // Keep scanning in case a later unsafe tool barriers, but it
                    // can't start either while safe tools execute — so the pass
                    // ends without starting anything once we hit the cap.
                    continue;
                }
                if can_execute(&executing_flags, safe) {
                    self.start_tool(i);
                    started_any = true;
                    break; // re-evaluate the executing set after each start
                } else if !safe {
                    // An exclusive (non-safe) tool can't start yet → barrier.
                    return;
                }
                // A safe tool that can't start (unsafe tool executing) —
                // keep scanning; TS continues the loop in this case.
            }
            if !started_any {
                return;
            }
        }
    }

    /// Dispatch tool `i` as an in-flight future on `self.inflight`. The future
    /// borrows `&'a orch` and is polled on the current task (no spawn). It
    /// routes through the existing per-tool pipeline
    /// [`crate::turn_loop::dispatch_tool_uses_tracked`] (one tool at a time) so
    /// the hook→permission→registry ordering stays byte-locked.
    fn start_tool(&mut self, i: usize) {
        self.tools[i].status = ToolStatus::Executing;
        let id = self.tools[i].id.clone();
        let name = self.tools[i].name.clone();
        let input = self.tools[i].input.clone();
        let provider_id = self.tools[i].provider_id.clone();
        let orch = self.orch;
        // Hand this tool a child of the executor's `tool_abort` token. When the
        // turn is discarded (or the user interrupts, via the parented
        // `user_cancel`), `tool_abort` fires and this child fires too — an
        // in-flight Bash kills its subprocess.
        let child = self.tool_abort.child_token();
        let fut =
            async move {
                let single = vec![(id, name, input, provider_id)];
                let outcome: DispatchOutcome =
                    match crate::turn_loop::dispatch_tool_uses_tracked(orch, &single, Some(child))
                        .await
                    {
                        // `single` has one element, so `pop()` == the only result.
                        Ok((mut blocks, _prevent, injected, modifiers)) => blocks
                            .pop()
                            .map(|b| (b, injected, modifiers))
                            .ok_or_else(|| {
                                crate::error::OrchestratorError::StreamingProtocol(format!(
                                    "dispatch returned empty for tool index {i}"
                                ))
                            }),
                        Err(e) => Err(e),
                    };
                (i, outcome)
            };
        self.inflight.push(Box::pin(fut));
    }

    /// `true` when no tool futures are currently in flight. Accessor for the
    /// live loop (which cannot reach the private `inflight` field across module
    /// boundaries).
    pub(crate) fn inflight_is_empty(&self) -> bool {
        self.inflight.is_empty()
    }

    /// TS `getAbortReason` (StreamingToolExecutor.ts:210-231): why tool `i` should
    /// be cancelled, computed from PRIOR state. Precedence is faithful:
    /// `discarded` → `StreamingFallback`, then a fired USER-interrupt token →
    /// `UserInterrupted` ONLY when this tool's `interrupt_behavior()==Cancel`
    /// (Block-behavior tools are NOT interrupted; TS returns `null` for them).
    /// Unlike `discarded` — which applies to the whole batch — the user-interrupt
    /// branch is PER-TOOL, so it must be evaluated by index here rather than
    /// precomputed once.
    fn abort_reason_for(&self, i: usize) -> Option<AbortReason> {
        if self.discarded {
            return Some(AbortReason::StreamingFallback);
        }
        if let Some(token) = &self.user_cancel {
            if token.is_cancelled() {
                // TS `getToolInterruptBehavior`: default Block when the tool is not
                // in the registry; only `Cancel` tools are user-interrupted.
                let is_cancel = matches!(
                    self.orch
                        .tools
                        .find_by_name(&self.tools[i].name)
                        .map(|t| t.interrupt_behavior(&self.tools[i].input)),
                    Some(tool_api::tool_trait::InterruptBehavior::Cancel)
                );
                if is_cancel {
                    return Some(AbortReason::UserInterrupted);
                }
            }
        }
        None
    }

    /// Await one in-flight tool future and record its result. Returns the
    /// completed tool index, or `None` if no futures are in flight.
    /// (TS `executeTool`/`collectResults` completion path.)
    pub(crate) async fn drain_one(&mut self) -> Option<usize> {
        let (i, outcome) = self.inflight.next().await?;
        // Compute the abort reason from PRIOR state (discard / user-interrupt). A
        // cancelled in-flight tool's real outcome is discarded for the synthetic.
        let abort_reason = self.abort_reason_for(i);
        // Cancelled in-flight tool: discard its real outcome for the synthetic.
        if let Some(reason) = abort_reason {
            let mut block = synthetic_error_block(self.tools[i].id.clone(), reason);
            set_provider_id(&mut block, self.tools[i].provider_id.clone());
            self.tools[i].result = Some(block);
            // A cancelled tool yields ONLY the synthetic — its injected msgs/modifiers are dropped.
            self.tools[i].status = ToolStatus::Completed;
            return Some(i);
        }
        // Otherwise record the real outcome (existing handling).
        match outcome {
            Ok((mut block, injected, modifiers)) => {
                // Copy the provider id onto the result for egress replay.
                set_provider_id(&mut block, self.tools[i].provider_id.clone());
                self.tools[i].result = Some(block);
                self.tools[i].injected = injected;
                self.tools[i].modifiers = modifiers;
                self.tools[i].status = ToolStatus::Completed;
            }
            Err(e) => {
                // A hard orchestrator error → surface as an errored result,
                // matching claude-code's outer plumbing catch
                // (toolExecution.ts:471-480): `Error calling tool (<name>): <msg>`
                // wrapped in `<tool_use_error>`.
                let name = &self.tools[i].name;
                self.tools[i].result = Some(ContentBlock::ToolResult {
                    tool_use_id: self.tools[i].id.clone(),
                    content: format!(
                        "<tool_use_error>Error calling tool ({name}): {e}</tool_use_error>"
                    ),
                    is_error: true,
                    provider_tool_use_id: self.tools[i].provider_id.clone(),
                    content_blocks: None,
                });
                self.tools[i].status = ToolStatus::Completed;
            }
        }
        Some(i)
    }

    /// Convert still-`Queued` tools to a synthetic-cancel result once
    /// `discarded` is set / the user interrupts (TS `getAbortReason` on next
    /// poll). This handles ONLY the `Queued` siblings; in-flight (`Executing`)
    /// siblings are substituted with the synthetic in [`Self::drain_one`] on
    /// their next completion (mirroring `collectResults` 335-345). The two are
    /// complementary: queued tools never enter `inflight`, so `drain_one` never
    /// sees them, and an executing tool is never `Queued` here.
    pub(crate) fn apply_abort_to_pending(&mut self) {
        for i in 0..self.tools.len() {
            if !matches!(self.tools[i].status, ToolStatus::Queued) || self.tools[i].result.is_some()
            {
                continue;
            }
            // Per-tool: the user-interrupt branch in `abort_reason_for` gates on
            // `interrupt_behavior()`, so a Queued Block-behavior tool under a pure
            // user-interrupt gets `None` here and still runs (faithful: Block tools
            // are not interrupted). `discarded` applies to all.
            let Some(reason) = self.abort_reason_for(i) else {
                continue;
            };
            let mut block = synthetic_error_block(self.tools[i].id.clone(), reason);
            set_provider_id(&mut block, self.tools[i].provider_id.clone());
            self.tools[i].result = Some(block);
            self.tools[i].status = ToolStatus::Completed;
        }
    }

    /// Mark the turn discarded (streaming fallback). Pending tools get a
    /// `StreamingFallback` synthetic result on the next `apply_abort_to_pending`.
    // Still only exercised by the Task-11 fallback test path / Phase 2; the live
    // loop does not yet discard.
    #[allow(dead_code)]
    fn discard(&mut self) {
        self.discarded = true;
        // Streaming-fallback also aborts in-flight work — fire `tool_abort` so
        // any in-flight Bash kills its subprocess.
        self.tool_abort.cancel();
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
        Ok(self
            .tools
            .iter()
            .map(|t| t.result.clone().expect("completed"))
            .collect())
    }
}

/// Short `Name(arg…)` description for a tool (TS `getToolDescription`,
/// StreamingToolExecutor.ts:243-252). No longer consumed in production now that
/// the sibling-error cascade is gone, but kept as the faithful API twin.
#[allow(dead_code)]
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
            summary.to_owned()
        };
        format!("{}({})", t.name, truncated)
    }
}

/// Copy a provider-issued tool-call id onto a `ToolResult` block's
/// `provider_tool_use_id` for egress replay; no-op for non-`ToolResult` blocks.
fn set_provider_id(block: &mut ContentBlock, provider_id: Option<String>) {
    if let ContentBlock::ToolResult {
        provider_tool_use_id,
        ..
    } = block
    {
        *provider_tool_use_id = provider_id;
    }
}

// ============================================================================
// Task 9: ordered result drain (TS getCompletedResults / hasUnfinishedTools)
// ============================================================================

/// One drained result ready for the live loop to persist, carrying everything
/// needed to build the per-result user message (TS `getCompletedResults` yields
/// one message per result). The live loop parents every result to the single
/// per-turn assistant via that assistant's captured JSONL uuid (TS
/// `sourceToolAssistantUUID`), so no per-result assistant id is carried here.
pub(crate) struct DrainedResult {
    pub(crate) block: ContentBlock,
    pub(crate) injected: Vec<(ConversationMessage, ToolUseId)>,
    pub(crate) modifiers: Vec<ContextModifier>,
}

impl<'a> StreamingToolExecutor<'a> {
    /// TS `getCompletedResults`: walk tools in order, yield each newly-`Completed`
    /// tool's result (marking it `Yielded`), and STOP at an `Executing`
    /// non-concurrency-safe tool (don't emit past an unfinished exclusive
    /// barrier). Returns results in RECEIVED order.
    pub(crate) fn take_newly_completed(&mut self) -> Vec<DrainedResult> {
        let mut out = Vec::new();
        for t in &mut self.tools {
            match t.status {
                ToolStatus::Completed => {
                    t.status = ToolStatus::Yielded;
                    out.push(DrainedResult {
                        block: t.result.clone().expect("completed tool has result"),
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

    /// TS `hasUnfinishedTools`: any tool not yet `Yielded`. The live loop drives
    /// on `inflight_is_empty` instead (guaranteed-progress shape), so this is
    /// exercised by the executor's tests; kept as the faithful API twin.
    #[allow(dead_code)]
    pub(crate) fn has_unfinished(&self) -> bool {
        self.tools.iter().any(|t| t.status != ToolStatus::Yielded)
    }
}

/// Build the synthetic `tool_result` for an unknown tool (claude-code `addTool`
/// line 78-84 / `toolExecution.ts:401`). Shared by the streaming executor and
/// the batched dispatch (`turn_loop`) so this parity-critical string lives in
/// exactly one place.
pub(crate) fn synthetic_unknown_tool(
    id: ToolUseId,
    name: &str,
    provider_id: Option<String>,
) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id,
        content: format!("<tool_use_error>Error: No such tool available: {name}</tool_use_error>"),
        is_error: true,
        provider_tool_use_id: provider_id,
        content_blocks: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TOOL_CONCURRENCY_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn tool_concurrency_env_guard() -> std::sync::MutexGuard<'static, ()> {
        TOOL_CONCURRENCY_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn status_enum_roundtrips() {
        assert_eq!(ToolStatus::Queued, ToolStatus::Queued);
        assert_ne!(ToolStatus::Queued, ToolStatus::Yielded);
    }

    #[test]
    fn streaming_fallback_synthetic() {
        let block = synthetic_error_block(ToolUseId::new(), AbortReason::StreamingFallback);
        let ContentBlock::ToolResult { content, .. } = block else {
            panic!()
        };
        assert_eq!(
            content,
            "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>"
        );
    }

    // ============================================================================
    // Task 6: StreamingToolExecutor::add_tool tests
    // ============================================================================

    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::MessageId;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// Build an orchestrator with an EMPTY tool registry. Used to exercise the
    /// unknown-tool short-circuit path.
    fn orch_empty() -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// A minimal concurrency-safe tool for the "known tool" path test.
    struct SafeTool;

    #[async_trait]
    impl Tool for SafeTool {
        fn name(&self) -> &str {
            "SafeTool"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "safe-tool".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "content": "ok" }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator whose registry contains a single `SafeTool`.
    fn orch_with_safe_tool() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(SafeTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    #[tokio::test]
    async fn add_unknown_tool_completes_immediately_with_wrapper() {
        let orch = orch_empty();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(
            ToolUseId::new(),
            "Nope".into(),
            json!({}),
            None,
            MessageId::new(),
        );
        let t = &exec.tools[0];
        assert_eq!(t.status, ToolStatus::Completed);
        assert!(t.is_concurrency_safe);
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = t.result.as_ref().unwrap()
        else {
            panic!("expected ToolResult block")
        };
        assert!(*is_error);
        assert_eq!(
            content,
            "<tool_use_error>Error: No such tool available: Nope</tool_use_error>"
        );
    }

    #[tokio::test]
    async fn add_known_concurrency_safe_tool_is_queued() {
        let orch = orch_with_safe_tool();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(
            ToolUseId::new(),
            "SafeTool".into(),
            json!({}),
            None,
            MessageId::new(),
        );
        let t = &exec.tools[0];
        assert_eq!(t.status, ToolStatus::Queued);
        assert!(t.is_concurrency_safe);
        assert!(t.result.is_none());
    }

    // ============================================================================
    // Task 7: can_execute + process_queue tests
    // ============================================================================

    use super::can_execute;

    #[test]
    fn can_execute_respects_concurrency_safety() {
        assert!(can_execute(&[], true));
        assert!(can_execute(&[], false)); // nothing running → ok
        assert!(can_execute(&[true, true], true)); // all safe + candidate safe → ok
        assert!(!can_execute(&[true], false)); // candidate unsafe, something running → no
        assert!(!can_execute(&[false], true)); // an unsafe tool running → no
        assert!(!can_execute(&[true, true], false)); // many safe running, unsafe candidate → no
    }

    /// A minimal concurrency-UNSAFE tool for ordering/barrier tests.
    struct UnsafeTool;

    #[async_trait]
    impl Tool for UnsafeTool {
        fn name(&self) -> &str {
            "UnsafeTool"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            false
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            false
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "unsafe-tool".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "content": "ok" }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator with both SafeTool and UnsafeTool.
    fn orch_with_both_tools() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(SafeTool) as Arc<dyn Tool>);
        registry.register_builtin(Arc::new(UnsafeTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// [safe, unsafe, safe]: first safe tool starts; the unsafe is a barrier;
    /// the third safe tool must NOT start out of order.
    #[tokio::test]
    async fn process_queue_starts_safe_tool_and_barriers_on_unsafe() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        // First safe starts; unsafe is a barrier (can't run while safe executes);
        // the third safe sits behind the barrier and must NOT start out of order.
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Queued);
        assert_eq!(exec.tools[2].status, ToolStatus::Queued);
    }

    /// [safe, safe]: both concurrent-safe tools start.
    #[tokio::test]
    async fn process_queue_starts_all_safe_tools_concurrently() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Executing);
    }

    /// [unsafe, safe]: the unsafe tool starts first (nothing executing), then
    /// the following safe tool stays Queued (exclusive holds the barrier).
    #[tokio::test]
    async fn process_queue_unsafe_first_blocks_following_safe() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Queued);
    }

    /// [safe, safe, unsafe, safe]: both leading safe tools start; the unsafe is
    /// a barrier; the trailing safe stays Queued behind it.
    #[tokio::test]
    async fn process_queue_two_safe_then_unsafe_barrier() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Executing);
        assert_eq!(exec.tools[2].status, ToolStatus::Queued);
        assert_eq!(exec.tools[3].status, ToolStatus::Queued);
    }

    /// A SAFE queued tool blocked by an already-Executing UNSAFE tool does NOT
    /// barrier (only an unsafe queued tool barriers) — process_queue scans past
    /// it and starts nothing. Exercises the "keep scanning" continuation branch
    /// that requires pre-existing Executing state.
    #[tokio::test]
    async fn process_queue_safe_blocked_by_executing_unsafe_keeps_scanning() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        // Simulate the unsafe tool already running (as if a prior process_queue
        // started it and it has not completed yet).
        exec.tools[0].status = ToolStatus::Executing;
        exec.process_queue();
        // Both safe tools are blocked by the executing unsafe tool; neither
        // starts, and the safe ones do NOT barrier (scan continues past them).
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Queued);
        assert_eq!(exec.tools[2].status, ToolStatus::Queued);
    }

    // ============================================================================
    // B6: concurrency cap (binary r1p / i1p) tests
    // ============================================================================

    use super::{
        max_tool_use_concurrency, max_tool_use_concurrency_from, DEFAULT_MAX_TOOL_USE_CONCURRENCY,
    };

    /// Binary `r1p`: `parseInt(env, 10) > 0 ? env : 10`.
    #[test]
    fn max_tool_use_concurrency_parse_matches_binary_r1p() {
        // Absent / empty / non-numeric → default 10.
        assert_eq!(max_tool_use_concurrency_from(None), 10);
        assert_eq!(max_tool_use_concurrency_from(Some("")), 10);
        assert_eq!(max_tool_use_concurrency_from(Some("abc")), 10);
        // Zero and negative → default (binary uses `e > 0`).
        assert_eq!(max_tool_use_concurrency_from(Some("0")), 10);
        assert_eq!(max_tool_use_concurrency_from(Some("-3")), 10);
        // Positive → that value.
        assert_eq!(max_tool_use_concurrency_from(Some("1")), 1);
        assert_eq!(max_tool_use_concurrency_from(Some("5")), 5);
        assert_eq!(max_tool_use_concurrency_from(Some("25")), 25);
        // parseInt leading-prefix semantics: "5x" → 5, "  7 " → 7.
        assert_eq!(max_tool_use_concurrency_from(Some("5x")), 5);
        assert_eq!(max_tool_use_concurrency_from(Some("  7 ")), 7);
        // Default constant is 10.
        assert_eq!(DEFAULT_MAX_TOOL_USE_CONCURRENCY, 10);
    }

    /// With 30 concurrency-safe tools, `process_queue` must start at most the
    /// default cap (10) simultaneously; the remaining 20 stay Queued. Mirrors
    /// the binary's `i1p` bounded merge (window = `r1p()` = 10).
    /// (Mutates env to clear any override → `--test-threads=1`.)
    #[tokio::test]
    async fn process_queue_caps_safe_tools_at_default_ten() {
        let _guard = tool_concurrency_env_guard();
        // Ensure no env override leaks in from the environment.
        std::env::remove_var("CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY");
        assert_eq!(max_tool_use_concurrency(), DEFAULT_MAX_TOOL_USE_CONCURRENCY);

        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        for _ in 0..30 {
            exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        }
        exec.process_queue();

        let executing = exec
            .tools
            .iter()
            .filter(|t| t.status == ToolStatus::Executing)
            .count();
        let queued = exec
            .tools
            .iter()
            .filter(|t| t.status == ToolStatus::Queued)
            .count();
        assert_eq!(
            executing, DEFAULT_MAX_TOOL_USE_CONCURRENCY,
            "no more than {DEFAULT_MAX_TOOL_USE_CONCURRENCY} safe tools may run at once"
        );
        assert_eq!(
            queued,
            30 - DEFAULT_MAX_TOOL_USE_CONCURRENCY,
            "the rest stay Queued"
        );
        // The first N (in received order) are the ones started.
        for i in 0..DEFAULT_MAX_TOOL_USE_CONCURRENCY {
            assert_eq!(
                exec.tools[i].status,
                ToolStatus::Executing,
                "tool {i} should run"
            );
        }
        for i in DEFAULT_MAX_TOOL_USE_CONCURRENCY..30 {
            assert_eq!(
                exec.tools[i].status,
                ToolStatus::Queued,
                "tool {i} should wait"
            );
        }
    }

    /// `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY` overrides the cap. With the env
    /// set to 3 and 10 safe tools queued, exactly 3 start.
    /// (Mutates env → `--test-threads=1`.)
    #[tokio::test]
    async fn process_queue_respects_env_concurrency_override() {
        let _guard = tool_concurrency_env_guard();
        std::env::set_var("CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY", "3");
        // Guard so a panic/assert failure still clears the env for sibling tests.
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                std::env::remove_var("CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY");
            }
        }
        let _clear = Clear;

        assert_eq!(max_tool_use_concurrency(), 3);

        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        for _ in 0..10 {
            exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        }
        exec.process_queue();

        let executing = exec
            .tools
            .iter()
            .filter(|t| t.status == ToolStatus::Executing)
            .count();
        assert_eq!(executing, 3, "env override caps safe concurrency at 3");
        assert_eq!(
            exec.tools
                .iter()
                .filter(|t| t.status == ToolStatus::Queued)
                .count(),
            7,
            "remaining 7 stay Queued under the override"
        );
    }

    /// Releasing one in-flight safe tool (mark it Completed) frees a slot so the
    /// next queued safe tool starts on the following `process_queue` — the
    /// sliding-window behaviour of the binary's `i1p` merge.
    /// (Mutates env → `--test-threads=1`.)
    #[tokio::test]
    async fn process_queue_starts_next_safe_when_slot_frees() {
        let _guard = tool_concurrency_env_guard();
        std::env::set_var("CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY", "2");
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                std::env::remove_var("CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY");
            }
        }
        let _clear = Clear;

        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        for _ in 0..4 {
            exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        }
        exec.process_queue();
        // Cap=2 → first two run, last two wait.
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Executing);
        assert_eq!(exec.tools[2].status, ToolStatus::Queued);
        assert_eq!(exec.tools[3].status, ToolStatus::Queued);

        // Simulate tool[0] completing → frees one slot.
        exec.tools[0].status = ToolStatus::Completed;
        exec.process_queue();
        // Now exactly one more (tool[2]) starts; tool[3] still waits (slot full again).
        assert_eq!(
            exec.tools[2].status,
            ToolStatus::Executing,
            "freed slot starts next"
        );
        assert_eq!(
            exec.tools[3].status,
            ToolStatus::Queued,
            "still capped at 2 in-flight"
        );
        let executing = exec
            .tools
            .iter()
            .filter(|t| t.status == ToolStatus::Executing)
            .count();
        assert_eq!(executing, 2, "never more than 2 safe tools in flight");
    }

    #[tokio::test]
    async fn add_unknown_tool_sets_provider_id_on_result() {
        let orch = orch_empty();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(
            ToolUseId::new(),
            "Ghost".into(),
            json!({}),
            Some("prov-abc-123".into()),
            MessageId::new(),
        );
        let t = &exec.tools[0];
        let ContentBlock::ToolResult {
            provider_tool_use_id,
            ..
        } = t.result.as_ref().unwrap()
        else {
            panic!()
        };
        assert_eq!(provider_tool_use_id.as_deref(), Some("prov-abc-123"));
    }

    /// Part B: the UserInterrupted synthetic uses the BARE REJECT_MESSAGE with
    /// is_error: true, and is NOT `<tool_use_error>`-wrapped.
    #[test]
    fn user_interrupted_synthetic_is_bare_reject_message() {
        let block = synthetic_error_block(ToolUseId::new(), AbortReason::UserInterrupted);
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = block
        else {
            panic!()
        };
        assert!(is_error, "user-interrupted result must be an error");
        assert_eq!(content, REJECT_MESSAGE);
        assert!(
            !content.contains("<tool_use_error>"),
            "REJECT_MESSAGE must be bare (not tool_use_error-wrapped): {content}"
        );
    }

    #[test]
    fn tool_description_priority_and_truncation() {
        // command field, > 40 chars → truncate to 40 + ellipsis.
        let long = "x".repeat(45);
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "Bash".into(),
            input: json!({ "command": long }),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: false,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        let desc = tool_description(&t);
        assert_eq!(desc, format!("Bash({}\u{2026})", "x".repeat(40)));

        // file_path fallback (no command), short → no truncation.
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "Read".into(),
            input: json!({ "file_path": "/tmp/a.txt" }),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: true,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        assert_eq!(tool_description(&t), "Read(/tmp/a.txt)");

        // pattern fallback.
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "Grep".into(),
            input: json!({ "pattern": "foo" }),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: true,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        assert_eq!(tool_description(&t), "Grep(foo)");

        // empty input → bare name.
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: true,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        assert_eq!(tool_description(&t), "SafeTool");
    }

    // ============================================================================
    // Task 9: take_newly_completed + has_unfinished tests
    // ============================================================================

    /// Test 1: Two SafeTools driven to completion → take_newly_completed returns
    /// both in received order; a second call returns empty; statuses become Yielded.
    #[tokio::test]
    async fn take_newly_completed_returns_results_in_received_order() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        while !exec.inflight.is_empty() {
            exec.drain_one().await;
        }
        // Both should be Completed now.
        assert_eq!(exec.tools[0].status, ToolStatus::Completed);
        assert_eq!(exec.tools[1].status, ToolStatus::Completed);

        let results = exec.take_newly_completed();
        assert_eq!(results.len(), 2, "expected both tools drained");
        // Statuses should now be Yielded.
        assert_eq!(exec.tools[0].status, ToolStatus::Yielded);
        assert_eq!(exec.tools[1].status, ToolStatus::Yielded);

        // Second call returns empty (all already Yielded).
        let results2 = exec.take_newly_completed();
        assert!(results2.is_empty(), "second call must return empty");
    }

    /// Test 2: Unknown tool (already Completed at add_tool time) is immediately
    /// yielded by take_newly_completed without calling process_queue.
    #[tokio::test]
    async fn take_newly_completed_yields_unknown_tool_immediately() {
        let orch = orch_empty();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(
            ToolUseId::new(),
            "NoSuchTool".into(),
            json!({}),
            None,
            MessageId::new(),
        );
        // No process_queue, no drain_one — it's already Completed.
        assert_eq!(exec.tools[0].status, ToolStatus::Completed);

        let results = exec.take_newly_completed();
        assert_eq!(results.len(), 1, "unknown tool result should be drained");
        assert_eq!(exec.tools[0].status, ToolStatus::Yielded);

        let ContentBlock::ToolResult { is_error, .. } = &results[0].block else {
            panic!("expected ToolResult block")
        };
        assert!(*is_error, "unknown-tool block must be an error");
    }

    /// Test 3: Exclusive-barrier stop.
    /// tool[0] = Completed (safe), tool[1] = Executing+unsafe, tool[2] = Completed (safe).
    /// take_newly_completed must yield ONLY tool[0] and stop at tool[1].
    #[tokio::test]
    async fn take_newly_completed_stops_at_executing_exclusive_tool() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);

        // We build a contrived state by directly constructing TrackedTools.
        let id0 = ToolUseId::new();
        let id1 = ToolUseId::new();
        let id2 = ToolUseId::new();
        let result_block = ContentBlock::ToolResult {
            tool_use_id: id0.clone(),
            content: "done".into(),
            is_error: false,
            provider_tool_use_id: None,
            content_blocks: None,
        };
        let result_block2 = ContentBlock::ToolResult {
            tool_use_id: id2.clone(),
            content: "also done".into(),
            is_error: false,
            provider_tool_use_id: None,
            content_blocks: None,
        };

        exec.tools.push(TrackedTool {
            id: id0,
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Completed,
            is_concurrency_safe: true,
            result: Some(result_block),
            injected: Vec::new(),
            modifiers: Vec::new(),
        });
        exec.tools.push(TrackedTool {
            id: id1,
            name: "UnsafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Executing,
            is_concurrency_safe: false, // exclusive barrier
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        });
        exec.tools.push(TrackedTool {
            id: id2,
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Completed,
            is_concurrency_safe: true,
            result: Some(result_block2),
            injected: Vec::new(),
            modifiers: Vec::new(),
        });

        let results = exec.take_newly_completed();
        // Only tool[0] should be emitted; tool[1] is the barrier; tool[2] is skipped.
        assert_eq!(
            results.len(),
            1,
            "only the pre-barrier completed tool should be drained"
        );
        assert_eq!(exec.tools[0].status, ToolStatus::Yielded);
        // tool[1] still Executing (we don't touch it).
        assert_eq!(exec.tools[1].status, ToolStatus::Executing);
        // tool[2] still Completed (was NOT emitted past the barrier).
        assert_eq!(exec.tools[2].status, ToolStatus::Completed);
    }

    /// Complement of the barrier test: an Executing+SAFE tool does NOT stop the
    /// drain — a `Completed` tool AFTER it is still emitted (TS: only
    /// `executing && !isConcurrencySafe` breaks; a safe executing tool falls
    /// through and scanning continues).
    #[tokio::test]
    async fn take_newly_completed_skips_executing_safe_and_emits_later_completed() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        let id0 = ToolUseId::new();
        let id1 = ToolUseId::new();
        exec.tools.push(TrackedTool {
            id: id0,
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Executing,
            is_concurrency_safe: true, // safe → NOT a barrier
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        });
        exec.tools.push(TrackedTool {
            id: id1.clone(),
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Completed,
            is_concurrency_safe: true,
            result: Some(ContentBlock::ToolResult {
                tool_use_id: id1,
                content: "done".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }),
            injected: Vec::new(),
            modifiers: Vec::new(),
        });

        let results = exec.take_newly_completed();
        // tool[0] (Executing+safe) is skipped but not a barrier; tool[1] emits.
        assert_eq!(results.len(), 1);
        assert_eq!(exec.tools[0].status, ToolStatus::Executing); // untouched
        assert_eq!(exec.tools[1].status, ToolStatus::Yielded);
    }

    // ============================================================================
    // DEFERRED-3: user-ESC granular interrupt (abort_reason_for + per-tool gating)
    // ============================================================================

    /// A concurrency-SAFE tool whose `interrupt_behavior()==Cancel`. Sleeps,
    /// racing its `ctx.cancel`; on cancel returns `Aborted` so the executor
    /// substitutes the synthetic. Mirrors a WebFetch/Agent-style Cancel tool.
    struct CancelBehaviorTool;

    #[async_trait]
    impl Tool for CancelBehaviorTool {
        fn name(&self) -> &str {
            "CancelTool"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn interrupt_behavior(
            &self,
            _input: &serde_json::Value,
        ) -> tool_api::tool_trait::InterruptBehavior {
            tool_api::tool_trait::InterruptBehavior::Cancel
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "cancel-tool".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            let token = ctx.cancel.clone();
            tokio::select! {
                () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                    Ok(ToolCallResult {
                        data: json!({ "content": "cancel-tool-ran-to-end" }),
                        new_messages: vec![],
                        context_modifier: None,
                        mcp_meta: None,
                    })
                }
                () = async { match token { Some(t) => t.cancelled().await, None => std::future::pending().await } } => {
                    Err(ToolError::Aborted)
                }
            }
        }
    }

    fn orch_with_cancel_and_block_tools() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(CancelBehaviorTool) as Arc<dyn Tool>);
        // SafeTool defaults to Block (no interrupt_behavior override).
        registry.register_builtin(Arc::new(SafeTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// `abort_reason_for`: a fired user-cancel token yields `UserInterrupted` for
    /// a Cancel-behavior tool and `None` for a Block-behavior tool (gating).
    #[tokio::test]
    async fn abort_reason_for_gates_on_interrupt_behavior() {
        let orch = orch_with_cancel_and_block_tools();
        let user_cancel = tokio_util::sync::CancellationToken::new();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new_with_user_cancel(&orch, user_cancel.clone());
        exec.add_tool(ToolUseId::new(), "CancelTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        // Not fired yet → no abort for either.
        assert_eq!(exec.abort_reason_for(0), None);
        assert_eq!(exec.abort_reason_for(1), None);
        user_cancel.cancel();
        assert_eq!(exec.abort_reason_for(0), Some(AbortReason::UserInterrupted));
        assert_eq!(
            exec.abort_reason_for(1),
            None,
            "Block tool is NOT interrupted"
        );
    }

    /// Queued Cancel-behavior tool under a fired user-cancel gets the bare
    /// REJECT_MESSAGE via `apply_abort_to_pending`; a queued Block tool runs.
    #[tokio::test]
    async fn apply_abort_to_pending_user_interrupt_rejects_cancel_tool_only() {
        let orch = orch_with_cancel_and_block_tools();
        let user_cancel = tokio_util::sync::CancellationToken::new();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new_with_user_cancel(&orch, user_cancel.clone());
        exec.add_tool(ToolUseId::new(), "CancelTool".into(), json!({}), None, a);
        user_cancel.cancel();
        exec.apply_abort_to_pending();
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = exec.tools[0].result.as_ref().unwrap()
        else {
            panic!()
        };
        assert!(*is_error);
        assert_eq!(content, REJECT_MESSAGE);
    }

    /// End-to-end through `run_to_completion`: an in-flight Cancel-behavior tool
    /// observes its `ctx.cancel` (parented to the user token) firing mid-flight,
    /// returns early, and `drain_one` substitutes the bare REJECT_MESSAGE.
    #[tokio::test]
    async fn in_flight_cancel_tool_user_interrupted_gets_reject_message() {
        let orch = orch_with_cancel_and_block_tools();
        let user_cancel = tokio_util::sync::CancellationToken::new();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new_with_user_cancel(&orch, user_cancel.clone());
        exec.add_tool(ToolUseId::new(), "CancelTool".into(), json!({}), None, a);
        // Start it, then fire the user cancel while it is in flight.
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        user_cancel.cancel();
        let results = exec.run_to_completion().await.unwrap();
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = &results[0]
        else {
            panic!()
        };
        assert!(*is_error);
        assert_eq!(
            content, REJECT_MESSAGE,
            "in-flight Cancel tool must get the bare REJECT_MESSAGE on user interrupt"
        );
        assert!(!content.contains("cancel-tool-ran-to-end"));
    }

    /// discard → StreamingFallback propagation: `discard()` fires `tool_abort`,
    /// whose child reaches an in-flight tool's `ctx.cancel`. The tool returns
    /// early and `drain_one` substitutes the streaming-fallback synthetic onto
    /// its real outcome (the surviving non-user abort path, formerly exercised by
    /// the removed Bash sibling-error cascade tests).
    #[tokio::test]
    async fn discard_substitutes_streaming_fallback_on_in_flight_tool() {
        let orch = orch_with_cancel_and_block_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "CancelTool".into(), json!({}), None, a);
        // Start it, then discard the turn while it is in flight.
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        exec.discard();
        let results = exec.run_to_completion().await.unwrap();
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = &results[0]
        else {
            panic!()
        };
        assert!(*is_error);
        assert_eq!(
            content,
            "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>",
            "in-flight tool must get the streaming-fallback synthetic on discard"
        );
        assert!(!content.contains("cancel-tool-ran-to-end"));
    }

    /// Test 4: has_unfinished returns true when there are Queued/Executing tools,
    /// and false once all tools are Yielded.
    #[tokio::test]
    async fn has_unfinished_tracks_non_yielded_tools() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        // No tools at all → nothing unfinished.
        assert!(
            !exec.has_unfinished(),
            "empty executor must have no unfinished tools"
        );

        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        assert!(exec.has_unfinished(), "Queued tool means unfinished");

        exec.process_queue();
        assert!(exec.has_unfinished(), "Executing tool still unfinished");

        while !exec.inflight.is_empty() {
            exec.drain_one().await;
        }
        assert!(
            exec.has_unfinished(),
            "Completed but not yet Yielded still unfinished"
        );

        exec.take_newly_completed();
        assert!(!exec.has_unfinished(), "all Yielded → no unfinished tools");
    }
}
