# Mobile Local Apps: Agent, authoring, and lifetime contract

Protocol status: migration to the official AG-UI 1.0 Schema and SDK 1.0.3 is in
progress. Desktop and Mobile share the [protocol contract](LOCAL_APP_AGENT_PROTOCOL.md).
Existing prototype builds/tests do not establish current standard conformance;
actual package and device results are recorded separately.

This document describes the current Local App contract shared by Android, iOS, Desktop, the browser SDK, and Harness. It is a source/design contract, not evidence that a build or a device acceptance run passed. Physical Android and iOS verification remains a separate release gate; evidence and open cases are recorded in the task acceptance report at `acceptance/mobile-app-agent-20261023/report.html`.

## One-off tasks and persistent chats

Local Apps use the bundled official AG-UI client:
`localApp.agent.createClient({threadId?})` returns an `AbstractAgent` subclass.
Applications add official `Message` values and call `runAgent`; the official SDK
returns `RunAgentResult` with `result` and `newMessages`. A selected browser file
becomes an official image/document part through `await localApp.agent.file(file)`.
The [shared contract](LOCAL_APP_AGENT_PROTOCOL.md#application-sdk) has the common
Desktop/Mobile recipe.

All thin run/send/start interfaces accept official `RunAgentInput`. Required
`threadId`, `runId` and `messages` retain their standard meaning. Host options use
`forwardedProps["com.lingxi.localApp"] = {version: 1, outputSchema?, tools?, skills?, maxTurns?, timeoutMs?}`.
They cannot change native ownership, credentials, models or permission mode.
An identical `runId` retry resolves to the same admission; changed input conflicts.
There is no separate public prompt/attachments/requestId message format.

Plain one-off runs use fresh thread context. `agent.chats.create/open` manages a
persistent App-owned thread mapped privately to a Harness Session. A handle ID
is its stable `threadId`; each turn gets a fresh `runId`. Chat history uses
official messages, with host-owned canonical parts and private assets kept
separate from the Harness model transcript. Supplied historical messages must
match canonical history; pages cannot forge assistant/tool history or approval.

Tasks/Chats CRUD and native task monitoring are host resources. Their metadata
states do not replace AG-UI completion semantics. `tasks.events` returns ordered
standard events plus a separate replay sequence. Subscribers receive standard
`BaseEvent` values; the official SDK assembles messages/results. Read-only
`connectAgent` never submits a new model request. A Promise does not survive
reload; durable run/thread IDs and host replay restore observation.

Supported file limits remain eight files, 25 MiB/file and 50 MiB/new user message.
Bytes and MIME content are validated on the host; unsupported media or model
capabilities fail explicitly. File helpers hide serialization from App authors.
Public standard parts never contain native paths or credentials. Structured
results are emitted through the standard result field after host Schema
validation. A stream truncated before its terminal event is not success;
cancellation uses the standard cancelled outcome, not a synthetic result.

## Isolation, tools, and Skills

Plain prompt and file understanding without requested tools or executable Skills uses an independent SideQuery, with no creator transcript, memory, or session history. A request that needs tools or executable Skills runs in an isolated Harness Agent workspace under the App's task directory. App tasks do not inherit the creator's model selection, chat grants, elevated permission mode, personal hooks, plugins, or MCP connections; managed rules and global deny/ask restrictions still apply. Each persistent App Chat owns a separate Harness Session with transcript recording, resume, and compaction, and deleting the creating chat does not delete it.

The model route and limits come from native App settings and do not silently switch providers. Default execution limits are one active task per App, 20 model turns, and ten minutes; the host can configure limits and an App can request lower limits. `timeoutMs` bounds execution after admission. The browser's finite wake/receive window is separate from this execution timeout. No JS Promise or native background lease is indefinite.

Effective tool authority is the intersection of release-declared tools, native host grants, the request, platform capability, and enterprise policy. Native UI owns permission and question approval; those controls are never exposed to page JavaScript. MCP access is bound to the selected configuration ID and revision, raw tool name, and definition digest. The host checks it at discovery and execution, including after approval or reconnect, and revocation blocks later calls. Apps cannot provide MCP URLs or credentials.

An immutable release may declare runtime Skills as `manifest.agent.skills: {id, path}[]`; each path is exactly `skills/<id>/SKILL.md` within that release. Native settings approve a subset of those IDs, and a request may use only the intersection. A Skill supplies instructions; it does not grant tools or permission to install dependencies or run scripts outside existing task permissions.

The host derives and revalidates the App, release, and invocation owner from the active run. Page-supplied owner, `appId`, release digest, native path, or session identifiers cannot select another App or invoke host administration. The host stores run fingerprints for idempotency; task metadata remains separate from canonical Chat messages and their bounded private file assets. An identical retry under the same run ID resolves to the existing admission; conflicting input under that ID fails. Startup marks unfinished tasks and Chats interrupted and never replays side effects. Refresh restores state through task APIs and Chat history. Cancelling, stopping, revoking, deleting, or replacing a release cancels affected work after its tool cleanup; upgrade preserves Chat history, cancels active old-release tasks, and rechecks grants before new turns.

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

On iOS 26, browser Agent work starts with an explicit native foreground handoff. The app requests an immediate `BGContinuedProcessingTask` and waits up to 12 seconds for the actual OS task to attach before opening the browser. If the lease does not attach in that window, native UI reports failure and does not send the user into a waiting browser page. After attachment, native reserves the exact `(appId, revision, runId)` with the host. The default receive lease is 60 seconds and the host caps it at 120 seconds; iOS requests 120 seconds. This window covers arrival and admission of that browser request only. The admitted task then has its own `timeoutMs`/turn limits and is monitored under the finite OS lease. Expiration or cancellation stops only the linked request, clears that reservation, and does not promise unattended or indefinite execution.

An ordinary user-invoked `openURL` has no `BGContinuedProcessingTask`; immediately before routing, the app requests a
UIKit assertion scoped to `(appId, revision)`. Before ending the continued-processing task for a deliverable terminal
result, it requests a delivery assertion in the same scope. An active assertion is reused without extending its deadline.
Each assertion is capped locally at 15 seconds and may expire earlier at the OS's request. An accepted `openURL` is routing feedback, not acknowledgement that the browser
loaded the page or saved the result. Cancellation, expiry, stop/revoke, and release replacement end matching assertions;
no later reload or indefinite background delivery is promised.
See Apple's [background-execution guidance](https://developer.apple.com/documentation/uikit/extending-your-app-s-background-execution-time) and
[`UIApplication.beginBackgroundTask`](https://developer.apple.com/documentation/uikit/uiapplication/beginbackgroundtask%28withname%3Aexpirationhandler%3A%29).

The browser wake link is narrowly scoped to `lingxi://apps/<appId>?review=agent&runId=<opaque>&revision=<revision>`. It carries no prompt, attachment, path, credential, or tool payload. Native validates the App, canonical positive revision, and opaque run ID (at most 128 UTF-8 bytes, no control characters); duplicate, unexpected, or malformed route/query values are rejected. The host allows one live pending or consumed reservation per App: the same tuple is idempotent, while a different request is busy. After the exact reservation is ready, the browser returns to the app page and the SDK submits the complete request and files together under the same run ID. `wake.clear` cancels only the run linked to that exact request. Retries with the same durable input can recover the admitted task. A request that misses the bounded receive deadline loses that reservation and must use a new foreground handoff; native does not hold a lease or UI waiting indefinitely.

On iOS 27 and later, availability-guarded `LongRunningIntent`/`performBackgroundTask` support uses the same native broker for a user-invoked intent that already has its complete bounded input. The intent returns the actual terminal AG-UI event JSON after host validation; it does not invent a separate Agent reply envelope. It does not wait for a browser upload while the browser waits for intent completion, and it does not authorize arbitrary browser-triggered intent execution. Its work remains bounded by configured task limits and the OS-managed Intent run; it is not an indefinite browser-delivery lease. iOS 26 remains the foreground browser-handoff path.

## Verification boundaries

The mobile browser SDK source and its served-route contract are owned by Harness; `crates/runtime/src/mobile/unified_local_apps/tests.rs::external_browser_sdk_and_agent_route_bind_caller_to_the_current_epoch` checks the served SDK and run-scoped route. Android source contains instrumented acceptance cases for the actual implicit system-browser handoff, browser file/photo selection, Agent attachment upload, task policy, and process lifetime. Existing iOS unit tests check wake-context scoping, permanent host errors, observed progress, and bounded intent inputs; they do not establish that iOS scheduled a continued-processing lease or ran an iOS 27 intent. These source and unit/contract checks are distinct from builds and physical acceptance.

Release acceptance requires actual Android devices and installed browsers, including the device's selected/default OEM or third-party browser, plus photo/file return, refresh/recovery, Agent tools, cancellation, service/process death, and stop notification. iOS 26 and iOS 27 behavior requires signed builds on those OS versions and evidence of the actual lease/intent UI. A historical iPhone 11 on iOS 18 does not verify either requirement. Build success, source inspection, simulator runs, or older-device results must not be reported as physical acceptance. The linked acceptance report records the exact artifacts, devices, browser, OS, logs, screenshots, and remaining cases.
