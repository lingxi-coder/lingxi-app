# Hooks & Permission Parity Audit — LingXi vs v2.1.186

**Date:** 2026-06-24  
**Oracle binary:** `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`  
**LingXi hooks:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/hooks/src/`  
**LingXi permission:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/permission/src/`

---

## Summary

**Total confirmed gaps: 6**  
**Uncertain (no binary evidence; TS-only): 0**  
All 6 are grounded in binary evidence (literal strings extracted from oracle binary at cited offsets).

---

## Gap Table

| # | Subsystem | Item | Oracle Evidence (binary offset) | LingXi (file:line) | Severity | Note |
|---|---|---|---|---|---|---|
| 1 | Hooks / payload | `ElicitationResult` wire payload missing — hooks for this event fire `None` in `build_lifecycle_envelope_body`, so the hook process receives no JSON | Binary: `executeElicitationResultHooks` at 67628403; schema at BIN off ~201751493: `{hook_event_name:"ElicitationResult", mcp_server_name:string, elicitation_id?:string, mode?:enum, action:enum, content?:record}` | `hooks/src/executor.rs:1243-1244` (comment: "not-yet-ported variant") and `_ => None` at `:1790` | **P0** | The `HookEvent::ElicitationResult` variant exists in `events.rs` and `HookEventNameElicitationResult` marker is defined in `hook_payload.rs:117-123`, but no `ElicitationResultPayload` struct or `executor.rs` arm exists. The event falls to `_ => None` so the hook subprocess receives an empty string instead of the JSON input. |
| 2 | Hooks / payload | `UserPromptSubmit` input payload missing `session_title` optional field | Binary off 201745825 (schema source): `{hook_event_name:"UserPromptSubmit", prompt:string, session_title?:string}`; `session_title` confirmed at offsets 105992153, 113805392, 201745825 | `hooks/src/hook_payload.rs:338-356` (`UserPromptSubmitPayload` has no `session_title`) | **P1** | The `session_title` optional field is present in the binary's `UserPromptSubmitHookInputSchema`. LingXi's `UserPromptSubmitPayload` lacks it — hooks receive the payload without the session title that may be available when fired. |
| 3 | Hooks / payload | `SessionStart` input payload missing `session_title` optional field | Binary off 201745825 (schema source chain): `{hook_event_name:"SessionStart", source:enum, agent_type?:string, model?:string, session_title?:string}` | `hooks/src/hook_payload.rs:362-381` (`SessionStartPayload` has `model` but no `session_title`) | **P1** | LingXi's `SessionStartPayload` already carries `model` (good) but is missing `session_title`. The binary schema at BIN off ~201746000 adds this optional field to the `SessionStart` shape. |
| 4 | Hooks / output | `UserPromptSubmit` hookSpecificOutput missing `sessionTitle` and `suppressOriginalPrompt` fields | Binary off 201754804: `{hookEventName:"UserPromptSubmit", additionalContext?:string, sessionTitle?:string, suppressOriginalPrompt?:boolean}` where `suppressOriginalPrompt` description is "When decision is 'block', omit the original prompt from the block message"; confirmed at offsets 113952656, 201754804, 206570907 | `hooks/src/hook_payload.rs:959-980` (`parse_response` only reads `additionalContext` for `UserPromptSubmit`); `hooks/src/response.rs` has no `suppress_original_prompt` field | **P1** | The binary's `UserPromptSubmitHookSpecificOutputSchema` has two extra optional output fields that LingXi's parser silently ignores. `sessionTitle` lets hooks rename the session; `suppressOriginalPrompt` controls blocking message display. Both are confirmed in the oracle binary. |
| 5 | Hooks / output | `MessageDisplay` hookSpecificOutput missing `displayContent` field | Binary off 201757586 (schema): `{hookEventName:"MessageDisplay", displayContent?:string}` with description "Text displayed in place of the delta. Omit (or return the delta unchanged) to display the original."; `displayContent` confirmed at offsets 113952704, 187071351, 201757586, 205705836 | `hooks/src/response.rs` has no `display_content` field; `hooks/src/hook_payload.rs` `parse_response` has no arm for `MessageDisplay` hookSpecificOutput | **P1** | The `MessageDisplay` event's output schema includes a `displayContent` string that replaces the assistant delta on-screen. LingXi currently ignores this field entirely — a hook that tries to modify displayed text has no effect. |
| 6 | Hooks / events (HOOK_EVENTS roster) | `PostToolBatch`, `UserPromptExpansion`, `MessageDisplay` are NOT in the TS `HOOK_EVENTS` constant (coreTypes.ts:25-53) but ARE in the binary and fire real hooks; LingXi's `events.rs` has them correctly but they are ABSENT from the TS-derived settings-validation roster | Binary: all three strings confirmed at offsets 67662391, 67628403, 67662631 (and many more); TS `HOOK_EVENTS` at `/claude-code/src/entrypoints/sdk/coreTypes.ts:25-53` lacks them | `lingxi-code/hooks/src/events.rs:77-83` correctly defines all three variants | **P2** | LingXi `events.rs` is correct (all 30 event types present). The gap is that the TS reference copy of `HOOK_EVENTS` (26 entries) is stale relative to the binary (30 events). This may affect hook-settings validation if LingXi uses the TS constant as an authoritative list anywhere. The Rust side is correct. |

---

## Evidence Notes

### Hook Event Roster (binary-confirmed)

The binary exposes these hook events as live strings (not just schema references):
- **PreToolUse** (BIN off 64905352), **PostToolUse** (64905408), **Notification** (55938871), **UserPromptSubmit** (64905779), **Stop** (64905582), **SubagentStop** (209791804), **PreCompact** (64307958), **SessionStart** (64905824), **PostToolBatch** (67662391), **UserPromptExpansion** (67628403), **MessageDisplay** (67662631), **ElicitationResult** (67662903)

The TS `HOOK_EVENTS` constant at `claude-code/src/entrypoints/sdk/coreTypes.ts:25-53` lists 26 events (missing PostToolBatch, UserPromptExpansion, MessageDisplay, ElicitationResult vs binary).

### Permission Subsystem

No gaps found. LingXi's permission crate (`/lingxi-code/permission/src/`) is well-aligned with the oracle:
- `PermissionBehavior` (Allow/Deny/Ask) matches binary-confirmed `permission_mode` usage
- `PermissionRuleSource` priority ordering matches claude-code's `Szn` walk (already corrected, #35 note in `rule.rs:190`)
- `PermissionResult` variants (Allow/Deny/Ask) match the binary
- `HookDecision::Defer` is present in `response.rs` matching `permissionDecision:"defer"` at BIN off 205722868
- `HookDecision::Ask` is present matching BIN off ~205721920

### Already-Closed Gaps (not re-reported)

Per audit scope, the following are ALREADY FIXED and excluded: ask-rule, AllowAlways-narrow, allowManaged*Only, bridge+mobile policy gate, mobile runner guard, http allowedEnvVars+agent model.
