# Conversation spaces and context ownership

Detailed design and current handoff: [Chat / Code context design](conversation-spaces-context-design.md), [2026-11-01 integration handoff](../plans/2026-11-01-conversation-spaces-handoff.md). The handoff distinguishes the intended protocol-32 contract from the current dependency and generated-binding state.

Chat and Code are independent conversation spaces over one Harness and one transcript store. A session receives its mode when it is created; opening it, changing its directory or selecting another space never changes that mode. The top-level selector changes navigation, not an executor's prompt profile.

## Product contract

- The ordinary New Chat action creates a Global conversation with no logical project, even after visiting a project. Project-specific creation is explicit.
- Each space remembers its own project, selected session and drafts. Projects share their physical files and have persistent membership in one or both spaces. Empty projects remain visible in their space.
- A session's immutable owner is its UUID, logical scope and mode. Its workspace/cwd can move independently. A Global conversation whose cwd is inside a project remains Global.
- All ordinary history surfaces use the selected space: project lists, recent chats, search, pinned and archived conversations. Filtering occurs before result limits.
- CLI exposes only Code. Public list/search/resume/fork reject Chat ownership before publishing a background job, claiming a writer or running hooks. Authenticated recovery of an already-admitted background Chat preserves that exact UUID and admitted context.

## Runtime ownership and navigation

Desktop retains UUID-owned runtimes. Native SessionHost retains the source, listener, reducer, permission/question queues, composer state and runtime owner independently of the visible screen. Navigation reuses an existing owner and does not submit Cancel, resume a second executor or detach its interaction channels. Late connection, first-submit and handoff completions are fenced by their captured owner and navigation operation.

Cold mobile mounts capture the actual transcript, fixed mode and saved workspace before startup services or hooks. Native admission prevents two independent owners mounting the same config-home/UUID. Claims survive active turns, detached session transitions and background cleanup. Idle settings reconnection retires the old source, waits for a native release receipt, then mounts the replacement. A failed or timed-out replacement keeps the draft and an explicit retry state.

A scheduled invocation may borrow an idle mounted owner only through the private scheduler path. The UUID gate and reader transition barriers exclude ordinary, queued and realtime admission until the child, detached transitions and background producers drain. Cancellation waits for that cleanup before publishing a terminal scheduled result.

Background notifications and recovery checkpoints carry the originating session/turn identity. Workspace or mode navigation cannot redirect Stop, permission replies, audio cleanup or recovery to the currently selected conversation. Native process-level Computer Use callbacks capture the executor that initiated the request.

## Fixed metadata and recovery

Protocol 32 requires `new_session.mode` and `resume_session.expected_mode`; `list_sessions.mode` is optional. There is no `set_session_mode` command or `session_mode_changed` event. `session_started` and `session_resumed` identify the fixed owner; `get_context_manifest` reports `session: { session_id, mode }` without changing it.

Every transcript starts with exactly one `fixed-session-mode` record containing `sessionId`, `sessionMode` and `instructionEpoch`, before an empty-session anchor or startup hook. Missing, duplicated, malformed or old mutable-mode records are rejected. There is no old-data migration or default-Code inference. Projects and per-space indexes require explicit mode membership in the current storage format. Older preferences and indexes are not read or overwritten.

The instruction epoch is independent of the immutable mode. An independent fork starts a new instruction epoch so inherited host activations remain audit evidence; authenticated recovery of an already-admitted background task preserves its admitted epoch. New scheduled TaskSession targets persist an empty-session anchor under their writer authority before binding the target UUID to the task.

Cold recovery accepts a torn final JSONL fragment, validates the committed prefix and all native preserved-membership invariants, then compares the original bytes under pinned transcript authority before repairing only that suffix. Complete/interior corruption remains an error. Recovery restores the original UUID, cwd, durable turn journal and admitted instruction epoch before hooks or deferred work.

## Context and capabilities

Chat supports research, writing, learning and requested tool work; Code uses the engineering workflow. Mode is independent of model, provider account, filesystem trust and execution permissions.

Both modes use the host's eligible MCP, plugin and Skill catalogs. Discovery loads catalog metadata first. Full Skill instructions, tool schemas and external resources are loaded when needed, with the session's own authorization. Navigation does not execute Skill bodies, setup commands or hooks. Provider normalization such as Tool Search is held by the owning API service rather than a process-global switch.

Host instructions remain distinct from evidence. Documents, tool output, MCP prompt expansions and old conversation text do not acquire instruction authority through headings or role delimiters. Host-issued Skill activations carry identity, content digests and instruction epochs. Projection excludes retired instructions without deleting the original transcript or same-text user evidence. Compaction and resume preserve model-exclusion markers, exact retained memberships and native-computer receipt boundaries.

## Cross-space handoff

Continue in another space prepares a linked, editable Global draft. It does not fork the source transcript or start the target Agent.

1. Capture a committed transcript cutoff under the source writer's short transaction, without acquiring its turn gate.
2. Reconstruct included evidence, respecting model-context exclusions. Do not inherit system instructions, Skill activations, approvals, pending tools, provider continuation or opaque reasoning.
3. Run one isolated, tool-denied summary request. The source turn may continue while it runs.
4. Keep the source UUID and cutoff with the target draft. The user edits and sends it to begin target execution. A late result offers the draft without stealing a newer navigation.

Prepare is idempotent for its request ID. Explicit Retry reuses the failed operation's fixed cutoff; Cancel affects only the summary. Failed captures have bounded retention and expiry; terminal event replay is bounded separately, so repeated handoffs cannot permanently exhaust an owner or retain unlimited history.

## Realtime audio

Provider response completion and Agent input completion are different events. Local, typed Agent activity events are produced only after the admitted input's tools and final continuation settle. The audio adapter serializes these separately from provider events, with operation/session/local-input identity. Clients retain the original audio lane through permission waits and playback acknowledgements; navigation does not abort it.

The connection owner selects the completion contract from trusted provider/model capabilities. OpenAI response IDs and Gemini's raw codec interaction status are checked at the provider boundary. An intermediate Gemini `IN_PROGRESS` completion does not finish an Agent input. Unsupported or ambiguous completion evidence fails explicitly rather than guessing. Current Agent audio uses the pinned providers' manual input commit configuration.

## Verification boundary

Source, protocol, host regression, native binding/typecheck, packaged application and physical-device/live-provider acceptance are separate evidence. Earlier profile-switch test counts and historical dependency pins do not describe this implementation. Validation must use the current immutable Harness revision and freshly generated protocol/FFI bindings; parser-only or stub-backed tests do not establish a native application build.
