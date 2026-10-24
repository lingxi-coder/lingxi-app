# Local App Agent on Desktop

Desktop Local Apps call an independent Agent through `window.localApp.agent`. The native Apps library owns the selected model, approved tools and Skills, task limits, and optional Agent background permission. The page never chooses provider credentials, a Harness session, a host path, or a permission mode.

An app opts in through its immutable release manifest. Tool and Skill declarations describe what the release may request; they do not grant access.

```json
{
  "schema": 2,
  "targets": [{ "platform": "macos", "environment": "web" }],
  "name": "Photo notes",
  "entry": { "type": "static", "path": "index.html" },
  "capabilities": ["agent"],
  "agent": {
    "tools": ["builtin:Read"],
    "skills": [{ "id": "nutrition", "path": "skills/nutrition/SKILL.md" }]
  }
}
```

The creating chat can call `LocalApp({ action: "agent_catalog" })` to discover model and tool IDs before authoring a manifest. Discovery grants nothing. The native host separately approves the model, exact tool grants, and Skill IDs in Apps settings. A Skill declaration must point to `skills/<id>/SKILL.md` in that release.

## Page API and files

The injected page SDK exposes:

```ts
localApp.agent.run(input): Promise<AgentReply>
localApp.agent.tasks.start(input): Promise<{ runId: string }>
localApp.agent.tasks.get(runId): Promise<Task>
localApp.agent.tasks.list(options?): Promise<Task[]>
localApp.agent.tasks.subscribe(runId, listener): Unsubscribe
localApp.agent.tasks.cancel(runId): Promise<void>

localApp.agent.chats.create({ title? }): Promise<ChatHandle>
localApp.agent.chats.open(chatId): Promise<ChatHandle>
localApp.agent.chats.list(options?): Promise<ChatSummary[]>
localApp.agent.chats.archive(chatId): Promise<void>
localApp.agent.chats.delete(chatId): Promise<void>

ChatHandle = {
  id: string,
  send(input): Promise<ChatReply>,
  history({ cursor?, limit? }): Promise<{ messages: ChatMessage[], nextCursor?: string }>,
  subscribe(listener): Unsubscribe,
  cancel(): Promise<void>
}
```

`agent.run()` waits for and returns the final reply. Use `tasks.start()` when the app needs the durable `runId` immediately; `get`, `list`, and subscriptions restore task state after a page reload. A JavaScript Promise itself lasts only for the current page. The SDK creates a `requestId` if one is absent. Retrying an identical request is idempotent; reusing the ID with different input is rejected.

```ts
AgentReply = { runId: string, text: string, output?: unknown, attachments: [], usage: Usage }
ChatReply = AgentReply & { chatId: string, messageId: string }
Usage = { inputTokens?: number, outputTokens?: number, costUsd?: number, costEstimated?: boolean, turns?: number }
```

`Task.result` contains `{ text, output?, attachments: [], chatId?, messageId? }`; usage appears once at `Task.usage`. Chat replies include the host-assigned message identity rather than requiring the app to infer the newest history row.

`AgentInput` is `{ requestId?, prompt, attachments?, outputSchema?, tools?, skills?, maxTurns?, timeoutMs? }`. `attachments` accepts standard browser `File` objects only. The app can supply Files from its own input control or use `localApp.files.pick({ multiple?, accept? })`, which returns page-world `File` objects; `accept` is a string or string array. The picker requires the app's separate `files` capability. Agent attachments do not expose a native path or a public base64 field.

The SDK is bundled with Desktop and injected into Local App HTML by the trusted host. Its narrow preload bridge exposes named operations only; it does not expose generic IPC. Because Electron's `contextBridge` does not preserve the page's `File`/`Blob` prototypes, the SDK reads each bounded File into an `ArrayBuffer` in the page's main world before crossing the bridge. Picker bytes cross in the other direction as `ArrayBuffer` and are rebuilt as `File` objects in the page world. The native host converts accepted bytes to the private attachment DTO for Rust; raw bytes and prompts are not stored in the task ledger.

The initial release accepts up to 8 files, 25 MiB per file and 50 MiB total. The runtime validates bytes and MIME content, independent of the File's advertised type. Supported inputs are UTF-8 `text/*`, JSON, XML, CSV and YAML; PDFs with a PDF signature; and JPEG, PNG, GIF and WebP images with matching signatures and successful image processing. Images are normalized to the shared model image budget. PDF and image input requires a vision-capable selected model. Unsupported archives, audio/video, arbitrary binary data, malformed UTF-8 and spoofed image/PDF signatures fail explicitly. Model-specific size or capability limits may be stricter. Generated reply attachments are not emitted in this release (`attachments: []`).

`TaskEvent` is either `{ type: "state", runId, sequence, time, task }` or `{ type: "connection", runId, error }`. Chat subscriptions emit `{ type: "state", chatId, chat }`, `{ type: "messages", chatId, messages, nextCursor? }`, or `{ type: "connection", chatId, error }`. Task state events carry increasing sequence numbers; Chat events are current state/history snapshots. Connection events report transport failures. Unsubscribe stops delivery but does not cancel work.

## Execution, authorization, and data

Every request is owned by the trusted host's active app, immutable release digest and invocation. The host revalidates that identity and the selected grants before execution and again before permission-sensitive operations. It fixes the model from native settings and does not silently fall back to a different model.

A one-shot request with no tools or Skills runs through Harness `SideQuery`. It is stateless: it uses only that request's prompt and attachments, has no creator conversation, app-chat history, personal/project memory, or inherited tools. Strict structured output is validated through the configured SideQuery path; the host does not scrape JSON from arbitrary response text. A request with tools or Skills runs in a private `bridge-server --app-agent` subprocess that constructs an isolated Desktop Harness runtime. Persistent App Chats always use that runtime, even when a turn requests no tools, because their context belongs to the App Chat.

Each App Chat has a host-assigned Harness session under `<config>/local-apps/v2/apps/<appId>/agent/chats/<chatId>`. Harness JSONL recording, cold resumption and normal compaction preserve its conversation across turns. The model context comes from that App-owned transcript; Desktop does not rebuild history by concatenating cached chat messages into a prompt. The session ID and directory stay private to the host. The separate App task/chat index is app-scoped; chat history returns visible user/assistant text and attachment metadata, while task records do not contain prompt text or raw file bytes. Deleting the creator chat does not delete an App Chat.

The runtime starts from a fresh App execution profile. It does not inherit the creating chat, its transcript, personal or project memory, user/project hooks, plugins, skills, or unselected MCP servers. Explicit managed policy and its trusted controls still apply. Built-in tools keep their normal host permission and filesystem/sandbox behavior; this profile does not add an operating-system sandbox.

For each turn, requested tools must be declared by the release and granted by native Apps settings. MCP authorization binds the exact server configuration ID and revision, original tool name, and definition digest. Search, lookup, approval, execution, reconnect, and later dispatch revalidate that grant. A changed MCP definition requires renewed approval. Delegation, extension installation and configuration-administration tools are excluded from the initial catalog. Tool approval and `AskUserQuestion` are handled by trusted Desktop UI; pages have no approval API.

Runtime Skills are declared in `manifest.agent.skills` and separately approved by the host. The request may select only Skills in both sets. The host copies selected Skill subtrees from the immutable release into the isolated task or Chat workspace after verifying each release file's path, size and digest. Skills add instructions and files, not permissions; scripts can run only through independently granted tools and existing task permissions.

Task states are `queued`, `running`, `succeeded`, `failed`, `cancelled`, and `interrupted`. Defaults are 20 model turns and 10 minutes. Native limits can be raised up to 100 turns and one hour; an app may only lower them. One Agent task may run per App at a time. Progress reports observed model-turn counts and the configured limit, not a percentage. A successful reply always contains the usage object reported by the executor; known fields (`inputTokens`, `outputTokens`, `costUsd`, `costEstimated`, `turns`) are included only when available. Unknown values are omitted, not replaced with zero. Usage is accounting information, not a hard spending guarantee.

The task ledger persists request identity before execution. After refresh, the app can reconnect with `tasks.get/list`; uncertain requests are not automatically resubmitted. A new attempt after terminal failure or interruption uses an explicit user action and a new request ID. Settings changes, grant revocation, Stop, deletion, restart and active-release replacement cancel affected runs. Closing the app window cancels Agent work unless the user separately enabled Agent background execution. That permission can keep the Agent task alive without keeping the app's ordinary processes alive. Desktop host exit marks unfinished tasks interrupted and never replays their side effects.

## Desktop authoring and publication

Each Desktop app uses a dedicated source Project selected in the Create Project flow; the default comes from Desktop's Local App project location, and the user may choose an existing directory. Its Code Project path and verified Code session working directory are the same canonical source path for this release. The native binding stores `workspaceId`, `sourceDirectory`, a Desktop `projectRef` path, the actual Code `authoringSessionId`, and the owning `appId`. The host resolves the selected session, verifies it is in Code mode, and checks its real workspace path before binding; page/model-supplied paths are not authoring authority.

Create establishes a Project and Code session for that source before composing the initial draft. Modify opens the same source Project and resumes its saved Code session when valid. If the session is missing or no longer a valid Code session, Desktop creates and binds a replacement Code session in that same source Project. If the Project is not loaded, the user must add the saved source path; an unrelated active Project is never selected implicitly. If the source directory is missing, Modify asks the user to locate or restore it and binds the confirmed source before drafting. Navigation cancellation or superseding navigation does not create or send a draft. Publishing uses the same app identity and expected revision; released source remains immutable.

Before showing a new draft, native Create durably registers the verified source Project and Code session as a pending authoring binding. First publication serializes the pending-binding check, Core publication, and binding consumption. Core saves the complete authoring reference with the App record. A failure before commit can be retried; after a committed publication or an uncertain response, another first-publish attempt reports the existing App identity instead of creating a duplicate. The tool's session locator is resolved through the trusted session catalog and never grants authority by itself. Ordinary publication outside native Create remains available without an authoring binding.

## Implementation map

- `apps/electron/src/preload/localAppSdk.ts` and `preload/localApp.ts`: bundled page SDK, File conversion and narrow preload surface.
- `apps/electron/src/main/local-apps/host.ts`: active-window ownership, native grants, file picker, subscriptions and authoring binding.
- `apps/electron/src/main/local-apps/agentBroker.ts`: durable App-scoped tasks/chats, idempotency, limits and lifecycle.
- `apps/electron/src/main/local-apps/agentExecutor.ts`: private subprocess, host credentials, model selection and task events.
- `apps/bridge-server/src/app_agent.rs` and Harness `desktop::app_invocation`: stateless SideQuery, isolated task execution and App-owned Chat recording/resumption.
- `apps/electron/src/renderer/bridge/localAppChat.ts`: safe Create/Modify navigation and Code-session drafts.
- `apps/electron/src/renderer/components/LocalAppAgentSettings.tsx`: native model, tool, Skill, limit and background controls.

This document describes the Desktop host. Mobile-specific browser, lifecycle and native permission behavior is documented separately in [Mobile Local App Agent](MOBILE_LOCAL_APP_AGENT.md).
