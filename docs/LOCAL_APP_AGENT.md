# Local App Agent on Desktop

Local App Agent uses the official AG-UI 1.0 Schema and SDK 1.0.3. The
[shared protocol contract](LOCAL_APP_AGENT_PROTOCOL.md) defines the input,
event and client boundary. Source checks and signed-package acceptance are
recorded separately in the acceptance report.

Only the current AG-UI Agent API and persistence format are supported. Source
import and publication require a schema 2 manifest; the library has no legacy
archive or old-package conversion interface.

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

Use `localApp.agent.createClient()` to obtain the bundled official AG-UI
`AbstractAgent` client. Add official messages with `addMessage`, then call
`runAgent`; its official `RunAgentResult` contains `result` and `newMessages`.
For selected browser files, `await localApp.agent.file(file)` returns one official
`ContentPart`. This keeps photo/document handling simple without introducing a
second message or reply format. See the executable recipe in the
[shared protocol contract](LOCAL_APP_AGENT_PROTOCOL.md#application-sdk).

The thin `agent.run`, `agent.tasks.start` and `Chat.send` methods all take official
`RunAgentInput`: `threadId`, `runId`, `messages` and supported standard options.
Product options use only
`forwardedProps["com.lingxi.localApp"] = {version: 1, outputSchema?, tools?, skills?, maxTurns?, timeoutMs?}`.
Standard top-level frontend tool declarations cannot grant native host tools.
Model credentials, native paths and permission modes remain private.

`tasks.get/list/cancel` and `chats.create/open/list/archive/delete` manage
host-owned resources. `tasks.events({runId, afterSequence?})` returns ordered
standard events with a separate replay sequence; `subscribe` observes the same
AG-UI events. `Chat.history` returns official messages, and the stable handle ID
is its `threadId`. A fresh `runId` identifies each turn. An identical run retry
is idempotent; conflicting input under that ID is rejected.

A JavaScript Promise lasts only for its page. `connectAgent({runId})` observes an
existing run without starting a model request. Unsubscribe/detach stops observing;
explicit cancellation stops work. A transport failure or missing terminal event
must not become a successful response. Cancellation is the standard
`RUN_FINISHED` with `outcome.type = "cancelled"` and no result.

The shared SDK is bundled and injected by the trusted host. Electron's narrow
preload bridge exposes named operations rather than generic IPC. File content is
represented by official parts at the public boundary and converted explicitly
into private execution DTOs; no native filesystem references reach the page.

The host accepts at most eight files, 25 MiB per file and 50 MiB per new user
message. It verifies bytes independently of advertised MIME type. Supported
inputs are valid UTF-8 text/JSON/XML/CSV/YAML, signed PDFs, and successfully decoded
JPEG/PNG/GIF/WebP images. Images use the shared image budget. Model capabilities
and size limits can be stricter; unsupported audio/video/archive/binary parts
fail explicitly. The AG-UI content type does not bypass host file validation.
Generated output file parts are not supported in this release.

## Execution, authorization, and data

Every request is owned by the trusted host's active app, immutable release digest and invocation. The host revalidates that identity and the selected grants before execution and again before permission-sensitive operations. It fixes the model from native settings and does not silently fall back to a different model.

A one-shot request with no tools or Skills runs through Harness `SideQuery`. It is stateless: it uses only that request's prompt and attachments, has no creator conversation, app-chat history, personal/project memory, or inherited tools. Strict structured output is validated through the configured SideQuery path; the host does not scrape JSON from arbitrary response text. A request with tools or Skills runs in a private `bridge-server --app-agent` subprocess that constructs an isolated Desktop Harness runtime. Persistent App Chats always use that runtime, even when a turn requests no tools, because their context belongs to the App Chat.

Each App Chat has a host-assigned Harness session under
`<config>/local-apps/v2/apps/<appId>/agent/agui-v1/chats/<chatId>`.
Every turn writes a fresh generation from the last successfully committed
checkpoint. Failed or cancelled generations never become the next turn's
history. Tool files use the separate persistent `workspaces/<chatId>` directory;
the transcript directory is private host storage.

The host retains the original conversation and inline attachments. When its
model context approaches the selected model's limit, a controlled SideQuery
summarizes complete earlier successful rounds for outgoing requests only. It
does not rewrite the original transcript, re-execute earlier tools or include
the current input in that summary. Tool calls and results stay within their
user round. The App reads standard Chat history and sends its new user message;
it does not implement its own compression or concatenate cached history into
a prompt. The host still enforces finite storage, input and execution limits.

Canonical protocol messages are separate from the Harness model transcript;
managed Chat assets retain standard content parts for history and cold resume.
Host session IDs, transcript locations and managed asset paths stay private;
public task metadata remains separate from message content. Deleting the
creator chat does not delete an App Chat.

The runtime starts from a fresh App execution profile. It does not inherit the creating chat, its transcript, personal or project memory, user/project hooks, plugins, skills, or unselected MCP servers. Explicit managed policy and its trusted controls still apply. Built-in tools keep their normal host permission and filesystem/sandbox behavior; this profile does not add an operating-system sandbox.

For each turn, requested tools must be declared by the release and granted by native Apps settings. MCP authorization binds the exact server configuration ID and revision, original tool name, and definition digest. Search, lookup, approval, execution, reconnect, and later dispatch revalidate that grant. A changed MCP definition requires renewed approval. Delegation, extension installation and configuration-administration tools are excluded from the initial catalog. Tool approval and `AskUserQuestion` are handled by trusted Desktop UI; pages have no approval API.

Persistent App Chat uses only tools that explicitly support a durable, self-contained history. Tool outputs stay inline within a 2 MiB result limit; shell output has a 256 KiB limit. Shell work remains in the foreground without output spools or detached tasks. Unsupported tools fail before the first model request; opaque external result handles fail verification instead of becoming a restorable Chat result. These constraints do not change ordinary user chats.

Runtime Skills are declared in `manifest.agent.skills` and separately approved by the host. The request may select only Skills in both sets. The host copies selected Skill subtrees from the immutable release into the isolated task or Chat workspace after verifying each release file's path, size and digest. Skills add instructions and files, not permissions; scripts can run only through independently granted tools and existing task permissions.

Each run receives a fresh, host-pinned Skill copy. The host removes it only after tool shutdown and drain are confirmed. An unfinished drain retains both the Skill scope and the Chat concurrency fence; a process exit alone does not prove that cleanup completed.

Task states are `queued`, `running`, `succeeded`, `failed`, `cancelled`, and `interrupted`. Defaults are 20 model turns and 10 minutes. Native limits can be raised up to 100 turns and one hour; an app may only lower them. One Agent task may run per App at a time. Progress reports observed model-turn counts and the configured limit, not a percentage. Usage reported by the executor is recorded by the host and mapped only to supported standard usage fields or an explicitly named host resource; unknown fields are not invented as standard AG-UI fields. Unknown values are omitted, not replaced with zero. Usage is accounting information, not a hard spending guarantee.

The task ledger persists request identity before execution. After refresh, the app can reconnect with resource reads and the official client read-only reconnect; uncertain requests are not automatically resubmitted. A new attempt after terminal failure or interruption uses an explicit user action and a new `runId`. Settings changes, grant revocation, Stop, deletion, restart and active-release replacement cancel affected runs. Closing the app window cancels Agent work unless the user separately enabled Agent background execution. That permission can keep the Agent task alive without keeping the app's ordinary processes alive. Desktop host exit marks unfinished tasks interrupted and never replays their side effects.

## Desktop authoring and publication

Each Desktop app uses a dedicated source Project selected in the Create Project flow; the default comes from Desktop's Local App project location, and the user may choose an existing directory. Its Code Project path and verified Code session working directory are the same canonical source path for this release. The native binding stores `workspaceId`, `sourceDirectory`, a Desktop `projectRef` path, the actual Code `authoringSessionId`, and the owning `appId`. The host resolves the selected session, verifies it is in Code mode, and checks its real workspace path before binding; page/model-supplied paths are not authoring authority.

Create establishes a Project and Code session for that source before composing the initial draft. Modify opens the same source Project and resumes its saved Code session when valid. If the session is missing or no longer a valid Code session, Desktop creates and binds a replacement Code session in that same source Project. If the Project is not loaded, the user must add the saved source path; an unrelated active Project is never selected implicitly. If the source directory is missing, Modify asks the user to locate or restore it and binds the confirmed source before drafting. Navigation cancellation or superseding navigation does not create or send a draft. Publishing uses the same app identity and expected revision; released source remains immutable.

Before showing a new draft, native Create durably registers the verified source Project and Code session as a pending authoring binding. First publication serializes the pending-binding check, Core publication, and binding consumption. Core saves the complete authoring reference with the App record. A failure before commit can be retried; after a committed publication or an uncertain response, another first-publish attempt reports the existing App identity instead of creating a duplicate. The tool's session locator is resolved through the trusted session catalog and never grants authority by itself. Ordinary publication outside native Create remains available without an authoring binding.

Before its first prompt, a native Code draft may not have a persisted catalog row yet. In that case only, the host can verify its exact current, connected runtime: the active session, native draft, runtime session and Project must match, its explicit mode must be Code, and the engine launch cwd must resolve to the source Project. An existing catalog row always takes precedence; a saved Chat session or mismatched workspace cannot use the draft path to bypass validation.

## Implementation map

- `apps/electron/src/preload/localAppSdk.ts` and `preload/localApp.ts`: bundled page SDK, File conversion and narrow preload surface.
- `apps/electron/src/main/local-apps/host.ts`: active-window ownership, native grants, file picker, subscriptions and authoring binding.
- `apps/electron/src/main/local-apps/agentBroker.ts`: durable App-scoped tasks/chats, idempotency, limits and lifecycle.
- `apps/electron/src/main/local-apps/agentExecutor.ts`: private subprocess, host credentials, model selection and task events.
- `apps/bridge-server/src/app_agent.rs` and Harness `desktop::app_invocation`: stateless SideQuery, isolated task execution and App-owned Chat recording/resumption.
- `apps/electron/src/renderer/bridge/localAppChat.ts`: safe Create/Modify navigation and Code-session drafts.
- `apps/electron/src/renderer/components/LocalAppAgentSettings.tsx`: native model, tool, Skill, limit and background controls.

This document describes the Desktop host. Mobile-specific browser, lifecycle and native permission behavior is documented separately in [Mobile Local App Agent](MOBILE_LOCAL_APP_AGENT.md).
