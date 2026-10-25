# Local App Agent protocol standardization

Status: AG-UI migration in progress, 2026-10-24. The previous
`agent.run/tasks/chats` functional prototype passed its recorded builds and smoke
tests; those results do not establish AG-UI conformance. The shared adapter,
Desktop, Mobile, generator and example migration requires fresh validation.
See the acceptance report for actual completed checks and device evidence.

## Main protocol

Use **AG-UI 1.0** for Local App-to-host Agent input and output. Local Apps are
user-facing applications. The Agent remains Harness; no second Agent framework
is introduced. Use the official schema and SDK types rather than maintaining a
LingXi-specific substitute for messages, multimodal parts, or run events.

Convenience JavaScript methods may wrap the official client. Method names such
as `run`, `tasks` or `chats` do not constitute a protocol: their Agent message
payloads and emitted events must use the official contract. Application authors
should not need to reproduce that contract or provider-specific request shapes.

Normative sources:

- [AG-UI 1.0 specification](https://docs.ag-ui.com/spec/1.0)
- [Messages and content parts](https://docs.ag-ui.com/concepts/messages)
- [Run and message events](https://docs.ag-ui.com/concepts/events)
- [Official architecture and client](https://docs.ag-ui.com/concepts/architecture)

The official `@ag-ui/core` and `@ag-ui/client` packages are pinned to **1.0.3**
at upstream commit `a1131c90709d51d562202b7b6cc0a2a1b48b05fc`. The canonical
1.0 JSON Schema is vendored byte-for-byte in Harness under
`packages/local-app-agui/spec/ag-ui-1.0.schema.json` (94,359 bytes; SHA-256
`4b5c93226838a0e72d88e6c5df20633c686c49fcb75d9be2815c6cbf9e48e71a`).
Keep package-lock integrity, source provenance and the upstream license with the
shared package. The official schema governs field structure; the behavioural specification governs
ordering, completion, interruption and cancellation. Do not treat example JSON
or hand-written TypeScript interfaces as the normative contract.

## Input and output

- Input uses `RunAgentInput` and standard `Message` / `ContentPart` values.
- Text, images and documents use the official content-part and source types.
  A browser `File` helper may read a selected file and construct that standard
  part; application authors do not need to handle base64 or native paths.
- Output uses the standard run, message, tool, state and error events. Use the
  official SDK's assembled messages/result; do not introduce another
  `AgentReply` envelope as the cross-platform communication standard.
- A structured result is exposed through the standard result facility only
  after host schema validation. Provider text that resembles JSON is not enough.
- JSON Schema constraints, execution limits and other product options that
  AG-UI does not define must be documented as a small, versioned, namespaced
  host extension at `RunAgentInput.forwardedProps['com.lingxi.localApp']`.
  Version 1 is `{ version: 1, outputSchema?, tools?, skills?, maxTurns?, timeoutMs? }`.
  `tools` here means exact host-granted tool IDs. Standard top-level AG-UI `tools`
  describes frontend tools and never grants native tools. Unsupported frontend
  tools, state, context or resume semantics fail explicitly; resume cannot approve
  a native permission. Do not add a new top-level `metadata` to `RunAgentInput`.
  These options must not redefine ordinary message, file or lifecycle semantics.

Public types come from the official contract. The internal Harness execution
DTO is private; an explicit host adapter converts validated standard input into
that DTO. It is not a fallback public API or a legacy-data loader. Check private
bridge types against authoritative Rust definitions, including exact JSON casing
and absent-field semantics; renaming fields alone does not establish protocol
conformance.

## Application SDK

Use the bundled official client with a custom authenticated host transport:

```ts
const agent = localApp.agent.createClient();
const content = [{ type: "text", text: "Describe this file." }];
if (selectedFile) content.push(await localApp.agent.file(selectedFile));
agent.addMessage({ id: crypto.randomUUID(), role: "user", content });
const { result, newMessages } = await agent.runAgent({
  runId: crypto.randomUUID(),
  forwardedProps: {
    "com.lingxi.localApp": { version: 1, outputSchema: applicationSchema }
  }
});
```

`createClient` returns an official `AbstractAgent` subclass; message accumulation
and `RunAgentResult` (`result`, `newMessages`) come from the SDK. A `File` helper
constructs the official image/document `ContentPart`, including its `DataSource`;
application authors do not encode bytes or supply native paths. Supported media
is a host capability, not a promise to support every part in the AG-UI schema.

Thin `agent.run(RunAgentInput)`, `tasks.start(RunAgentInput)` and persistent
`Chat.send(RunAgentInput)` consume the same official input. Tasks and Chats CRUD
remain host resources. `tasks.events({runId, afterSequence?})` returns a host
replay batch `{events: BaseEvent[], sequence}`; sequence is outside the official
event objects. Subscriptions deliver the official events. The official client
assembles their messages and result. Resource state alone is not a successful
Agent response.

## Sessions, tasks and lifecycle

An independent invocation uses a fresh run context and the existing SideQuery
path when no tools or Skills are requested. A persistent App conversation uses
a stable AG-UI `threadId` mapped privately to its own Harness Session, with a new
`runId` for every turn. Creator conversation history remains isolated.

The public `runId` is supplied by the standard client and remains unchanged in
run events and resource operations. It also keys idempotency: an identical retry
returns the same admission, and changed input with the same ID is rejected.
Private execution/storage IDs may differ only behind a scoped, tested mapping.

The host owns durable run records, event replay, request deduplication and
conversation storage. Task listing, conversation creation/archive/deletion,
Project/Session authoring bindings and OS background leases are host resources;
AG-UI does not standardize their complete CRUD or storage policy. Keep those
resource interfaces separate from the standard Agent message/event contract.

Reconnection observes recorded state without starting another model request.
A stream ending without a terminal event is truncated; it must not be presented
as successful or synthesized into a clean cancellation. Explicit host
cancellation stops execution and produces the standard cancelled outcome.
Native interruption/approval and protocol resume must retain their different
authority: a page cannot approve a host tool request through a forged resume.

Android's foreground service and iOS 26 continued-processing / iOS 27 Intent
mechanisms retain their existing ownership and finite lifetime. A protocol
standard does not make an OS lease indefinite or guarantee background wakeup.

## Model, tool and Skill boundaries

- Model traffic stays in `llm-client` and its provider adapters. OpenAI uses the
  supported OpenAI API route, including Responses where applicable; the Local
  App does not depend on a vendor-specific model API or receive credentials.
- Connected tools use [MCP](https://modelcontextprotocol.io/specification/2026-07-28/server/tools).
  Tool declarations and AG-UI tool events do not grant permission. Native App
  settings, release declarations, exact MCP provenance and managed policy
  continue to constrain execution.
- Runtime Skill packages follow the [Agent Skills specification](https://agentskills.io/specification),
  including `SKILL.md` metadata and directory rules. Skill loading does not grant
  tools, scripts or installation permission.
- A2A is an interoperability option for a later external-Agent endpoint. Do not
  put a second public Agent protocol into the initial Local App boundary merely
  to mirror the same request twice.

## Delivery and verification

First implement one shared protocol adapter and schema contract, then use it in
Desktop and the Mobile browser/host bridge. Update the application generator and
examples in the same change. This first-release API does not need obsolete
aliases, fallback decoders or migration branches that preserve the prototype's
custom message format. Initialize a fresh Agent persistence version and remove
known obsolete task/chat/pending data in the host-owned Agent scope. Do not
convert legacy messages. Application business data, source Projects, immutable
releases and protocol-independent authoring bindings are outside that cleanup.

Add protocol conformance cases for official schema validation, event ordering,
terminal success/error/cancel/interrupt semantics, reconnect without replay,
multimodal parts, structured output and cross-App isolation. Retain the existing
functional regressions for authoring bindings, permissions and cold Chat resume.

Acceptance remains one flavor per platform: Desktop Flare development, Android
PlayDebug and iOS FullDebug. A protocol change requires fresh affected builds
and acceptance; prior prototype screenshots and tests are historical evidence.
