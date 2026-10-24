# Mobile Local Apps: Agent, authoring, and lifetime contract

This document describes the current Local App contract shared by Android, iOS, Desktop, the browser SDK, and Harness. It is a source/design contract, not evidence that a build or a device acceptance run passed. Physical Android and iOS verification remains a separate release gate; evidence and open cases are recorded in the task acceptance report at `acceptance/mobile-app-agent-20261023/report.html`.

## One-off tasks and persistent chats

The public API has three distinct paths:

- `localApp.agent.run(input)` starts one independent request and resolves to its final `AgentReply`.
- `localApp.agent.tasks.start(input)` returns a durable `runId` as soon as the host admits the request. `get`, `list`, `subscribe`, and `cancel` inspect, restore, observe, and stop tasks. Use this path when the app needs a run ID before completion or must recover task state after a page reload.
- `localApp.agent.chats.create({title?})` and `.open(chatId)` return an App-owned Chat handle. A handle can `send`, page through `history`, `subscribe`, or `cancel`; the Chats collection can `list`, `archive`, and `delete`. Chats keep their own conversation history and Harness session across page reloads; they do not inherit the conversation that created the Local App.

`run` and `Chat.send` resolve to the final result. A JavaScript Promise belongs to the current page and does not survive reload. Durable recovery uses task listing/state or Chat history. A Chat send is admitted as a durable task before the SDK waits for its final reply. The host permits one active Chat turn per Chat and, by default, one active task per App.

A Task keeps its `runId`, `requestId`, state (`queued`, `running`, `succeeded`, `failed`, `cancelled`, or `interrupted`), monotonic sequence, observed phase/progress, timestamps, model/limits, result, usage, and error. Phase and progress are omitted unless observed by the host. Progress is `{completedModelTurns, turnLimit}`; the count comes from completed Harness model turns, and the limit is not a forecast or percentage. Task listing is scoped to the owning App.

Inputs have this public shape:

```ts
type AgentInput = {
  requestId?: string;
  prompt: string;
  attachments?: File[];
  outputSchema?: unknown;
  tools?: string[];
  skills?: string[];
  maxTurns?: number;
  timeoutMs?: number;
};
```

Applications use standard browser `File` objects in `attachments`. `localApp.files.pick({multiple?, accept?})` returns browser `File` objects; `accept` may be a MIME/type string or an array of them. The SDK creates a `requestId` when omitted. It accepts up to eight files, 25 MiB per file, and 50 MiB total; model or host limits may be lower. Unsupported media or model capabilities fail explicitly.

Pages cannot submit filesystem paths, native file references, provider IDs, `images`, or base64 payload fields as Agent input. The browser sends file metadata and bytes together through one bounded, run-scoped upload. The trusted host converts bytes to its private execution DTO. Raw file contents and host model credentials are not written to the task or Chat ledger. Public Chat history includes attachment name, media type, and size only. Public result attachments are currently empty.

Successful `AgentReply` values always include a `usage` object. Its known metrics—`inputTokens`, `outputTokens`, `costUsd`, `costEstimated`, and `turns`—are optional and appear only when the runtime reported them. Missing metrics are not zero. `Task.result` stores the reply text, optional structured output, empty result attachments, and Chat IDs where applicable; `Task.usage` is the authoritative usage object. Structured output is validated by the host. Text that merely resembles JSON is not a structured result.

```ts
type AgentReply = { runId: string; text: string; output?: unknown; attachments: []; usage: Usage };
type ChatReply = AgentReply & { chatId: string; messageId: string };
type Usage = {
  inputTokens?: number;
  outputTokens?: number;
  costUsd?: number;
  costEstimated?: boolean;
  turns?: number;
};
```

## Isolation, tools, and Skills

Plain prompt and file understanding without requested tools or executable Skills uses an independent SideQuery, with no creator transcript, memory, or session history. A request that needs tools or executable Skills runs in an isolated Harness Agent workspace under the App's task directory. App tasks do not inherit the creator's model selection, chat grants, elevated permission mode, personal hooks, plugins, or MCP connections; managed rules and global deny/ask restrictions still apply. Each persistent App Chat owns a separate Harness Session with transcript recording, resume, and compaction, and deleting the creating chat does not delete it.

The model route and limits come from native App settings and do not silently switch providers. Default execution limits are one active task per App, 20 model turns, and ten minutes; the host can configure limits and an App can request lower limits. `timeoutMs` bounds execution after admission. The browser's finite wake/receive window is separate from this execution timeout. No JS Promise or native background lease is indefinite.

Effective tool authority is the intersection of release-declared tools, native host grants, the request, platform capability, and enterprise policy. Native UI owns permission and question approval; those controls are never exposed to page JavaScript. MCP access is bound to the selected configuration ID and revision, raw tool name, and definition digest. The host checks it at discovery and execution, including after approval or reconnect, and revocation blocks later calls. Apps cannot provide MCP URLs or credentials.

An immutable release may declare runtime Skills as `manifest.agent.skills: {id, path}[]`; each path is exactly `skills/<id>/SKILL.md` within that release. Native settings approve a subset of those IDs, and a request may use only the intersection. A Skill supplies instructions; it does not grant tools or permission to install dependencies or run scripts outside existing task permissions.

The host derives and revalidates the App, release, and invocation owner from the active run. Page-supplied owner, `appId`, release digest, native path, or session identifiers cannot select another App or invoke host administration. The host stores request fingerprints for idempotency, not prompts or file bytes. An identical retry under the same request ID resolves to the existing admission; conflicting input under that ID fails. Startup marks unfinished tasks and Chats interrupted and never replays side effects. Refresh restores state through task APIs and Chat history. Cancelling, stopping, revoking, deleting, or replacing a release cancels affected work after its tool cleanup; upgrade preserves Chat history, cancels active old-release tasks, and rechecks grants before new turns.

## Create and Modify authoring

Local App source is separate from the immutable published release. A native authoring binding is:

```ts
type ProjectRef =
  | { kind: "desktopPath"; path: string }
  | { kind: "mobileProject"; projectId: string };

type AuthoringRef = {
  workspaceId: string;
  sourceDirectory: string;
  projectRef: ProjectRef;
  authoringSessionId: string;
  appId?: string;
};
```

Create allocates a UUID workspace and dedicated source Project and Code session. Before binding or resuming, the native adapter verifies that the selected Code session's real engine working directory is the source directory. Modify reuses the same valid source Project and Code session and persists the corrected binding. If the binding or session is missing or mismatched, the host creates a replacement only after resolving the source; missing source requires an explicit locate or restore action. It never falls back to a global project or unrelated chat. Published release files are immutable. Publishing a modification keeps the App ID and supplies the expected revision; Remix creates a new identity. Modify does not auto-send a prompt or publish.

## Brand-owned mobile paths

Mobile runtime paths come from the host's `BrandRuntimeProfile { brandId, linuxUser, configDirName }`; Android and iOS use the returned profile and directory values instead of spelling a Linux user or config folder in product code. Derived guest paths are:

| Location | Profile-derived path |
| --- | --- |
| Guest home | `/home/<linuxUser>` |
| Config root | `/home/<linuxUser>/<configDirName>` |
| Brand workspace root | `/home/<linuxUser>/<brandId>` |
| Projects | `/home/<linuxUser>/<brandId>/projects` |
| Local App authoring sources | `<config root>/.apps/<slug>-<uuid8>` |
| Native persistent backing | `<managedRoot>/persistent/<linuxUser>` |

The source-directory `uuid8` suffix is generated independently from the Project's `workspaceId`; the directory name is not an encoding of the workspace identity.

The App library and agent data live below the config root under `local-apps/v2/apps/<appId>/{releases/<revision>/source,data,runs,agent/{chats/<chatId>,tasks/<runId>,assets}}`; immutable release source, user data, task records, Chat records, and attachments have separate directories. Linux user identity is a runtime profile and is independent of the Firebase account. No files or paths are migrated from `/root` or another brand profile.

## External browser and process lifetime

Android and iOS open the app page in the operating system's resolved external HTTP browser handler. The OS uses the user's actual default browser or presents its resolver when needed. This includes OEM and third-party browsers; product code must not pin a browser brand, package, or application ID. Mobile Local App content is not rendered in Android WebView or iOS WKWebView. Browser camera and file selection use standard browser flows; the browser returns selected content as `File` objects through the SDK.

On Android, a user-started foreground service owns the Local App runtime before the app hands off to the browser. Its persistent notification provides stop and view actions. Task records can be inspected after process death, but work is not replayed. Stop and cancellation target the App or named run and wait for cleanup before releasing admission.

On iOS 26, browser Agent work starts with an explicit native foreground handoff. The app requests an immediate `BGContinuedProcessingTask` and waits up to 12 seconds for the actual OS task to attach before opening the browser. If the lease does not attach in that window, native UI reports failure and does not send the user into a waiting browser page. After attachment, native reserves the exact `(appId, revision, requestId)` with the host. The default receive lease is 60 seconds and the host caps it at 120 seconds; iOS requests 120 seconds. This window covers arrival and admission of that browser request only. The admitted task then has its own `timeoutMs`/turn limits and is monitored under the finite OS lease. Expiration or cancellation stops only the linked request, clears that reservation, and does not promise unattended or indefinite execution.

The browser wake link is narrowly scoped to `lingxi://apps/<appId>?review=agent&requestId=<opaque>&revision=<revision>`. It carries no prompt, attachment, path, credential, or tool payload. Native validates the App, canonical positive revision, and opaque request ID (at most 128 UTF-8 bytes, no control characters); duplicate, unexpected, or malformed route/query values are rejected. The host allows one live pending or consumed reservation per App: the same tuple is idempotent, while a different request is busy. After the exact reservation is ready, the browser returns to the app page and the SDK submits the complete request and files together under the same request ID. `wake.clear` cancels only the run linked to that exact request. Retries with the same durable input can recover the admitted task. A request that misses the bounded receive deadline loses that reservation and must use a new foreground handoff; native does not hold a lease or UI waiting indefinitely.

On iOS 27 and later, availability-guarded `LongRunningIntent`/`performBackgroundTask` support uses the same native broker for a user-invoked intent that already has its complete bounded input. The intent can return a result and run reference. It does not wait for a browser upload while the browser waits for intent completion, and it does not authorize arbitrary browser-triggered intent execution. iOS 26 remains the foreground browser-handoff path.

## Verification boundaries

The mobile browser SDK source and its served-route contract are owned by Harness; `crates/runtime/src/mobile/unified_local_apps/tests.rs::external_browser_sdk_and_agent_route_bind_caller_to_the_current_epoch` checks the served SDK and run-scoped route. Android source contains instrumented acceptance cases for the actual implicit system-browser handoff, browser file/photo selection, Agent attachment upload, task policy, and process lifetime. Existing iOS unit tests check wake-context scoping, permanent host errors, observed progress, and bounded intent inputs; they do not establish that iOS scheduled a continued-processing lease or ran an iOS 27 intent. These source and unit/contract checks are distinct from builds and physical acceptance.

Release acceptance requires actual Android devices and installed browsers, including the device's selected/default OEM or third-party browser, plus photo/file return, refresh/recovery, Agent tools, cancellation, service/process death, and stop notification. iOS 26 and iOS 27 behavior requires signed builds on those OS versions and evidence of the actual lease/intent UI. A historical iPhone 11 on iOS 18 does not verify either requirement. Build success, source inspection, simulator runs, or older-device results must not be reported as physical acceptance. The linked acceptance report records the exact artifacts, devices, browser, OS, logs, screenshots, and remaining cases.
