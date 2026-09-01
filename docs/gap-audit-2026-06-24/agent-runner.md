# Agent Runner + Orchestrator Turn-Loop Parity Audit
## LingXi vs claude-code v2.1.186

**Date:** 2026-06-24  
**Binary:** v2.1.186 `/opt/homebrew/.../claude-code-darwin-arm64/claude`  
**Total CONFIRMED gaps: 2 P0 | 3 P1 | 3 P2**  
**UNCERTAIN (feature-gated/needs deeper read): 3**

---

## CONFIRMED GAPS

| # | Area | Item | Oracle (evidence) | LingXi (file:line) | Severity | Note |
|---|------|------|-------------------|--------------------|----------|------|
| 1 | Synthetic messages / interruptions | `CANCEL_MESSAGE` emitted when `abortController.signal.aborted` is already true at `runToolUse` entry (pre-dispatch abort path) | TS `toolExecution.ts:415-452`: if signal already aborted, yields `{type:"tool_result", content:withMemoryCorrectionHint(CANCEL_MESSAGE), is_error:true}`. `CANCEL_MESSAGE="The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed."` Binary @64290753 `[Request interrupted by user]` exists but is a DIFFERENT string at REPL level. | LingXi `turn_loop.rs`: `dispatch_tool_uses_tracked` has no pre-cancellation guard. `streaming_executor.rs:17` has `REJECT_MESSAGE` (different text) for mid-stream cancel; `CANCEL_MESSAGE` text is entirely absent from LingXi. | P0 | LingXi's batched path never emits CANCEL_MESSAGE. The streaming path emits REJECT_MESSAGE for user-cancel but it's a different code path. |
| 2 | Synthetic messages / interruptions | `INTERRUPT_MESSAGE='[Request interrupted by user]'` and `INTERRUPT_MESSAGE_FOR_TOOL_USE='[Request interrupted by user for tool use]'` produced as conversation messages on REPL-level cancel | TS `messages.ts:207-209`: two distinct strings. Binary @64290753 confirms `[Request interrupted by user]` verbatim. These are injected into the transcript as user-turn content when the whole turn is cancelled at the REPL layer. | LingXi `session/src/jsonl/title.rs:54`: uses these strings as a PATTERN for title-strip regex — proving awareness — but LingXi's turn drivers (`conversation.rs`, `turn_loop.rs`) do NOT produce `INTERRUPT_MESSAGE` or `INTERRUPT_MESSAGE_FOR_TOOL_USE` as injected messages. Turn cancellation returns `TurnOutcome::Cancelled` enum; no string injected. | P0 | Entire REPL-cancel message injection is absent. Claude-code injects `[Request interrupted by user]` as a user message into conversation history on cancel; LingXi does not. |
| 3 | Tool dispatch | `sibling_error` abort reason absent — when one concurrent tool errors, siblings do NOT get `<tool_use_error>Cancelled: parallel tool call … errored</tool_use_error>` | TS `StreamingToolExecutor.ts:153-205`: three-branch `createSyntheticErrorMessage` — `user_interrupted`, `streaming_fallback`, `sibling_error`. Sibling-error produces `Cancelled: parallel tool call ${desc} errored` wrapped in `<tool_use_error>`. `this.hasErrored` flag drives the cascade. | LingXi `streaming_executor.rs:30-40`: `AbortReason` enum has only `UserInterrupted` and `StreamingFallback`. Line 554-558: `tool_description()` is `#[allow(dead_code)]` with explicit comment "No longer consumed in production now that the sibling-error cascade is gone." | P1 | INTENTIONALLY dropped per LingXi comment. When a parallel tool fails, its siblings run to natural completion in LingXi instead of getting synthetic cancel results. |
| 4 | Turn-loop control | `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY` environment variable cap (default 10) for concurrent tool execution | TS `toolOrchestration.ts:8-12`: `getMaxToolUseConcurrency() = parseInt(env.CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY) || 10`. The concurrent-batch runner is bounded by this count. | LingXi `streaming_executor.rs`: `FuturesUnordered` with no numeric count gate. `process_queue` uses `is_concurrency_safe` + block-tool serialization but no ceiling. | P1 | Unlikely to matter in practice (models rarely emit >10 tool_uses per turn), but env-override capability is absent. |
| 5 | Tool dispatch | Batched (non-streaming) path uses `partitionToolCalls` — concurrent-safe groups run as atomic batches; next serial tool waits for ALL concurrent siblings to complete | TS `toolOrchestration.ts:19-82`: `runTools` iterates over `partitionToolCalls(…)` groups; for `isConcurrencySafe` groups, runs `runToolsConcurrently` for the whole group then yields; for non-safe, runs serially. This is a GROUP-then-next model. | LingXi batched path (`turn_loop.rs`→ `dispatch_tool_uses_tracked` → `StreamingToolExecutor`): the executor starts safe tools one-by-one as `canExecuteTool` passes, not group-atomic. A serial tool may begin while safe siblings are still in-flight if `inflight_is_empty()` passes. | P1 | Subtle execution order difference. For the streaming path the difference is smaller (tools start during the stream anyway). For the batched path the dispatch order of tool results may differ when safe + unsafe tools are interspersed. |
| 6 | Streaming / SSE edges | `thinking_delta` event handling in content_block_delta | Binary @193628080 `content_block_delta` block, TS shows `case "thinking_delta"` arm in the SDK's `content_block_delta` switch. LingXi `sse/accumulator.rs` (inferred) accumulates thinking blocks. | Need to verify `sse/event_router.rs` handles `thinking_delta` — not read in this audit. | P2 (UNCERTAIN) | See UNCERTAIN section. |
| 7 | Tool-result envelope | `withMemoryCorrectionHint` applied to `REJECT_MESSAGE` on user-interrupted synthetic | TS `StreamingToolExecutor.ts:165`: `content: withMemoryCorrectionHint(REJECT_MESSAGE)` — appends hint when `isAutoMemoryEnabled() && tengu_amber_prism` flag. | LingXi `streaming_executor.rs:55`: `REJECT_MESSAGE.to_string()` — no hint appended. | P2 | Flag-gated (`tengu_amber_prism`, default false). Dormant in standard builds. |
| 8 | Tool dispatch | `capacity_off_switch` telemetry event absent | Binary @80350304: telemetry event string `capacity_off_switch` emitted when Anthropic capacity limits hit a special gate. TS code around binary @198668109. | LingXi `service.rs:979`: maps `Overloaded` to telemetry tag `"overloaded"` — does not produce `capacity_off_switch` event string. | P2 | Telemetry-only divergence (not a behavioral gap in API handling). |

---

## UNCERTAIN GAPS (deeper file reads needed)

### U1: `redacted_thinking` ContentBlock preservation
- **Binary:** @79427792 `thinking redacted_thinking` — both types present in content block handling.
- **TS:** `redacted_thinking` blocks accumulated and passed through as opaque assistant content.
- **LingXi:** `streaming_loop.rs:237` collects `AppendAssistantBlock(block)` from `event_router`. Does `event_router.rs` emit a `RedactedThinking` variant? Does `protocol::ContentBlock` have it? Not read.
- **Risk:** If absent, redacted thinking blocks are silently dropped, causing the next API call to lack them in the context (Anthropic requires them to be passed back).

### U2: `server_tool_use` block passthrough
- **Binary:** @68560208 — `server_tool_use` blocks appear in content-block tracking alongside regular `tool_use`.
- **TS:** `server_tool_use` blocks are preserved as opaque assistant content and echoed back.
- **LingXi:** Not confirmed whether `ContentBlock` has a `ServerToolUse` variant.
- **Risk:** If absent, web_search and similar server-side tools break.

### U3: `No response requested.` injection for empty non-streaming turns  
- **Binary:** @64116016 `No response requested.` — matches `NO_RESPONSE_REQUESTED = 'No response requested.'` (`messages.ts:240`).
- **LingXi:** No production site found in turn drivers.  
- **Context:** In TS, this is injected when the REPL receives a response with no content and no pending tool results (specific non-interactive flow). May be UI-only, not core loop.

---

## VERIFIED-OK (no gap)

| Area | Item | Evidence |
|------|------|----------|
| Tool-result envelope | `<tool_use_error>` wrapper on unknown tool | PRESENT `streaming_executor.rs:650` |
| Tool-result envelope | `is_error:true` on all error paths | PRESENT all error arms |
| Tool-result envelope | `InputValidationError:` prefix in `<tool_use_error>` | PRESENT `turn_loop.rs:1866` |
| Tool-result envelope | `<tool_use_error>Error calling tool (${name}): …` | PRESENT `streaming_executor.rs:483` |
| Tool-result envelope | `<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>` | PRESENT `streaming_executor.rs:48` |
| Tool-result envelope | `[Tool result missing due to internal error]` (ensureToolResultPairing) | PRESENT `llm-client/src/convert.rs:215` |
| Turn-loop control | Max output-tokens recovery limit = 3 (`MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`) | PRESENT `turn_loop.rs:255` |
| Turn-loop control | `max_tokens` nudge text (em-dash "Resume directly —") | PRESENT `turn_loop.rs:272-275` |
| Turn-loop control | `max_tokens` escalation (8k→64k) `ESCALATED_MAX_TOKENS=64000` | PRESENT `turn_loop.rs:266` |
| Turn-loop control | Malformed tool_use retry nudge + SECOND-failure string | PRESENT `turn_loop.rs:308,315` |
| Turn-loop control | Thinking-only nudge `[Your previous response had no visible output…]` | PRESENT `turn_loop.rs:323` |
| Turn-loop control | `end_turn` / `tool_use` / `max_tokens` stop_reason dispatch | PRESENT in `execute_one_turn_with_recovery_tracked` |
| Turn-loop control | `hook_stopped` stop_reason on PreToolUse `continue:false` | PRESENT `turn_loop.rs:844` |
| Streaming/SSE | `message_delta` stop_reason + usage handling | PRESENT `streaming_loop.rs:271-295` |
| Streaming/SSE | Stream-ended-without-stop error | PRESENT `streaming_loop.rs:313` |
| Streaming/SSE | Mid-stream tool dispatch via `pump_stream_with_executor` | PRESENT `streaming_loop.rs:185-191` |
| Synthetic messages | `<system-reminder>\n…\n</system-reminder>` wrap format | PRESENT at multiple sites |
| Synthetic messages | `system-reminder` per-turn injection (memory, skill listing, etc.) | PRESENT `turn_loop.rs:595-601` |
| API retry/error | MAX_529_RETRIES = 3, `repeated_529` tracking | PRESENT `retry.rs:94-95` |
| API retry/error | `Overloaded { repeated }` error type | PRESENT `error.rs:46-56` |
| API retry/error | `\n\nRequest ID: …` suffix on refusal errors | PRESENT `turn_loop.rs:1448` |
| API retry/error | `invalid_request` stop_reason end-turn | PRESENT `turn_loop.rs:652` |
| Tool dispatch | Tool sort order (localeCompare at schema-registration layer) | N/A — tool sort happens at schema registration, not dispatch. LingXi `tool_api` handles registration order. |
| Tool dispatch | `isConcurrencySafe` per-tool check | PRESENT all tools implement `fn is_concurrency_safe` |
| Tool dispatch | `AbortReason::UserInterrupted` → bare `REJECT_MESSAGE` (not `<tool_use_error>`-wrapped) | PRESENT `streaming_executor.rs:55,1258-1261` (test asserts bare) |

---

## Confirmed Gaps

| # | Area | Severity | Binary evidence | LingXi behavior | Description |
|---|------|----------|----------------|-----------------|-------------|
| 1 | MCP tool concurrency safety | P1 | `killShellTasksForAgent` at binary offset 89153872 + 202032451; `isConcurrencySafe() { return tool.annotations?.readOnlyHint ?? false }` at `claude-code/src/services/mcp/client.ts:1795-1796` | `tools/mcp/src/mcp_tool.rs:454` hardcodes `is_concurrency_safe() -> bool { true }` for all MCP tools; `platform-api/src/mcp.rs:178` `McpToolDto` has no `read_only_hint`/`annotations` field | TS maps MCP `tool.annotations?.readOnlyHint` → `isConcurrencySafe()`, defaulting **false** when not set. LingXi hardcodes **true** for every MCP tool. Effect: all MCP tools become safe for concurrent dispatch in LingXi, meaning multiple MCP tools can run in parallel when they should serialize. Any MCP server with no `readOnlyHint` annotation gets concurrency incorrectly enabled. |
| 2 | Shell task kill on subagent exit | P1 | Binary offset 89153872 `killShellTasksForAgent` (dead-code-elimination survived → it is called at runtime, not stripped) | `agent/src/runner.rs:238-239` `finally` block only calls `he.clear_agent_hooks(agent_id)`. No per-agent shell-task kill. `LocalBashHandler::drain_pending_kills` exists but is never called from subagent-exit code paths (`orchestrator/` and `agent/` grep → 0 hits). | TS `runAgent.ts:844-847`: "Kill any background bash tasks this agent spawned. Without this, a `run_in_background` shell loop outlives the agent as a PPID=1 zombie." LingXi subagent runner does NOT kill bash tasks spawned by the subagent on exit. Background bash loops (`run_in_background: true`) from within a subagent will orphan after the subagent completes or fails. |
| 3 | PostSampling hooks not fired | P2 | `query.ts:1001` `void executePostSamplingHooks(...)` after every model response; used by `sessionMemory.ts:374`, `skillImprovement.ts:180`, `magicDocs.ts:252` to register hooks | `orchestrator/src/turn_loop.rs` — grep for `post_sampling` → 0 hits. No `PostSampling` hook concept in LingXi hooks enum or execution path. | TS fires `executePostSamplingHooks` (non-blocking `void`) after every model response in `query.ts` main loop. This drives session-memory extraction, skill improvement, and MagicDocs updates. LingXi has no equivalent PostSampling hook slot. Note: all three consumers are internal claude-code features that LingXi does not implement — so there are currently no registered PostSampling hooks to miss. This becomes a gap if/when LingXi adds features that need to observe each model response. |

---

## Uncertain Findings

### U1 — ESCALATED_MAX_TOKENS retry not wired (feature-gated)
- **TS**: `query.ts` wires `maxOutputTokensOverride: ESCALATED_MAX_TOKENS` (64,000) on `max_output_tokens` recovery behind `tengu_otk_slot_v1` GrowthBook gate.
- **Binary**: GrowthBook `tengu_otk_slot_v1` defaults false in the binary (no OVERRIDE_GROWTHBOOK env → gate is always false → escalation path never runs in stock binary).
- **LingXi**: `turn_loop.rs` has `DEFERRED (A1)` comment explicitly noting the escalated retry is not wired.
- **Verdict**: Not a real production gap because the gate is default-off. If Anthropic enables `tengu_otk_slot_v1` server-side, LingXi will diverge. Track as follow-up.

### U2 — `cleanupAgentTracking` (prompt-cache break detection) on subagent exit
- **TS**: `runAgent.ts:824-825` calls `cleanupAgentTracking(agentId)` in finally block, behind `feature('PROMPT_CACHE_BREAK_DETECTION')`.
- **Binary**: `PROMPT_CACHE_BREAK_DETECTION` does not appear in the binary (grep → 0 hits) — feature is compiled out / stripped.
- **LingXi**: No equivalent.
- **Verdict**: Feature is not active in v2.1.186 binary. Not a functional gap today.

---

## Structural Matches Confirmed

The following areas were audited and found to be parity-correct:

- **StreamingToolExecutor concurrency gate**: `streaming_executor.rs` correctly implements `can_execute(executing_safe_flags, candidate_safe)` matching TS's `isConcurrencySafe` gate. `DEFAULT_MAX_TOOL_USE_CONCURRENCY=10` and `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY` env-var reading with JS `parseInt` leading-prefix semantics both correct.
- **Structured output retry cap**: `runner.rs` `structured_output_retry_cap()` reads `MAX_STRUCTURED_OUTPUT_RETRIES` env (default 5), nudge limit 2, matches TS exactly.
- **NKE set (companion note for disallowed tools)**: `companion_note_for_disallowed_tool()` covers TaskOutput, ExitPlanMode, EnterPlanMode, AskUserQuestion, ConnectGitHub, WaitForMcpServers, ScheduleWakeup, (Workflow for non-ant) — matches binary-grounded TS set.
- **Frontmatter hooks with `isAgent=true`**: `runner.rs:103-241` registers agent hooks scoped to child `agent_id`, fires `SubagentStop` (agent-scoped, not double-fire), clears hooks in finally — matching `runAgent.ts:557-575 + clearSessionHooks`.
- **`isNonInteractiveSession` for async agents**: `runner.rs:1025-1026` passes `is_async: ctx.is_async` to `SubagentInvocationContext`, which maps to `is_non_interactive_session=true` for async agents — matching `runAgent.ts:668-672`.
- **Tool-result error format**: `runner.rs:1057` `format!("Error: {}", e.model_facing_message())` matches turn-loop convention.
- **RecoveryState**: `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT=3`, `ESCALATED_MAX_TOKENS=64_000`, `MALFORMED_TOOL_USE_RETRY_NUDGE`, `THINKING_ONLY_NUDGE` all present in `turn_loop.rs`.
- **Abort handling**: After abort, `streamingToolExecutor.getRemainingResults()` → synthetic tool_results pattern implemented in `conversation.rs`. `createUserInterruptionMessage({toolUse:true})` on abort (skipped for `'interrupt'` reason) correctly implemented.
- **`build_preload_messages` (G4+G5)**: SubagentStart `additionalContext` injection as `<system-reminder>` message and skill preloading both implemented in `runner.rs:403+`.
- **`translate_response_blocks()`**: Preserves RedactedThinking, ServerToolUse, ConnectorText, AdvisorToolResult — all correctly handled.

---

## Methodology Notes

Binary oracle used for evidence: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`

Key binray greps used:
- `grep -aboF 'killShellTasksForAgent' binary` → offsets 89153872, 202032451 (survived DCE → called at runtime)
- `grep -aboF 'PROMPT_CACHE_BREAK_DETECTION' binary` → 0 hits (compiled out)
- `grep -aboF 'MONITOR_TOOL' binary` → 0 hits (compiled out)
- `grep -aboF 'run_in_background' binary` → multiple hits confirming the feature is active

TS source cross-referenced: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
LingXi source cross-referenced: `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/`
