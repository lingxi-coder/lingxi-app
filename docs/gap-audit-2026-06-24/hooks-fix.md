# hooks parity fix — report

**STATUS:** DONE  
**Commit:** `956c136b`  
**Tests:** 289 passed, 0 failed (was 286; added 3)

## Per-gap status

| Gap | Status | Notes |
|-----|--------|-------|
| [P0] `ElicitationResult` payload | DONE | `ElicitationResultPayload` struct added; binary-confirmed schema `{hook_event_name:"ElicitationResult", mcp_server_name, elicitation_id?, mode?, action, content?}` (BIN off ~201751493). Executor arm extracts `action`/`content` from `HookEvent::ElicitationResult.result` JSON blob; defaults `action` to `"cancel"` when absent. All 30 variants now serializable. |
| [P1] `session_title` in `UserPromptSubmit` input | DONE | Added `session_title: Option<String>` to `UserPromptSubmitPayload` (key `session_title`, confirmed BIN off 201745825). Threaded through `HookContext.session_title` → `BaseHookFields.session_title` → payload arm. |
| [P1] `session_title` in `SessionStart` input | DONE | Same as above for `SessionStartPayload`. |
| [P1] `UserPromptSubmit` hookSpecificOutput: `sessionTitle` | DONE — parse-and-wire | Key `sessionTitle` (camelCase, confirmed BIN off 201754804) parsed and stored in `HookResponse.session_title` / `AggregateHookResult.session_title`, last-wins. Seam exists (`session_title` field on aggregate); full apply (calling `updateSessionTitle`) is orchestrator work, not hooks-crate scope. |
| [P1] `UserPromptSubmit` hookSpecificOutput: `suppressOriginalPrompt` | DONE — parse-and-wire | Key `suppressOriginalPrompt` (boolean, confirmed BIN off 201754804) parsed into `HookResponse.suppress_original_prompt` / `AggregateHookResult.suppress_original_prompt`, OR-folded. **TODO**: wire at block-message render site — no clean seam inside the hooks crate itself. |
| [P1] `MessageDisplay` hookSpecificOutput: `displayContent` | DONE — parse-and-wire | Key `displayContent` (string, confirmed BIN off 201757586) parsed into `HookResponse.display_content` / `AggregateHookResult.display_content`, last-wins. **TODO**: wire display-override at message-display render site — no clean seam inside the hooks crate. |
| [P2] `events.rs` roster | NO FIX NEEDED | Confirmed all 30 variants present. Gap was TS-side only; Rust side was correct. |

## Fields parsed but not yet fully wired (reason)

- `suppress_original_prompt`: parsed and propagated on aggregate. The "omit original prompt from block message" display logic lives in the turn-loop/TUI render site outside the hooks crate — no clean wiring seam here. Field is on `AggregateHookResult` for the orchestrator to consume.
- `display_content`: parsed and propagated on aggregate. The on-screen delta substitution lives at the MessageDisplay render site in the TUI/turn-loop — again outside the hooks crate. Field is on `AggregateHookResult` for the orchestrator to consume.

## Binary key name verification

All key names binary-confirmed via `strings` + `dd` offset extraction:
- `"ElicitationResult"`, `"mcp_server_name"`, `"elicitation_id"`, `"action"`, `"content"` — BIN off ~201751493
- `"session_title"` — BIN off 201745825 (both input payloads)
- `"sessionTitle"`, `"suppressOriginalPrompt"` — BIN off 201754804 (UserPromptSubmit hsOut)
- `"displayContent"` — BIN off 201757586 (MessageDisplay hsOut)
