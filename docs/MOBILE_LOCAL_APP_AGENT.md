# Mobile Local App Agent

Implementation status: source integration in progress. Android and iOS physical acceptance is required before release. This branch combines the Desktop Agent source with the schema-2 native foundation and Mobile port. The new combined packages still require acceptance.

## Application contract

A schema-2 manifest declares `"capabilities": ["agent"]` and `"agent": {"tools": []}`. Add camera/photos capabilities only when used. Model credentials, provider selection, tool grants and approvals belong to the native host.

```js
const { runId } = await localApp.agent.run({
  requestId: crypto.randomUUID(),
  prompt: 'Describe this image and return the requested data.',
  images: [{ media_type: 'image/jpeg', base64: imageBytes }],
  outputSchema: { type: 'object', properties: { description: {type: 'string'} },
    required: ['description'], additionalProperties: false },
  tools: []
});
const unsubscribe = localApp.agent.subscribe(runId, event => {
  if (event.run.state === 'succeeded') render(event.run.result.output);
});
// localApp.agent.get(runId), localApp.agent.cancel(runId)
```

Native camera/photo pickers return `{mimeType, base64}`. Convert the MIME field to `media_type` for the Agent request. Resize images before submitting: the native bridge limits the complete request to 24 MiB. The runtime additionally validates image signatures, decode budgets, and a maximum of four images.

iOS photo-library selections are normalized off the main thread to an oriented JPEG with a maximum dimension of 2048 pixels. This includes HEIC/PNG selections; the MIME is `image/jpeg`, never `image/*`. Inputs over 16 MiB or 64 megapixels are rejected. The file picker preserves original bytes. Cancelled photo callbacks cannot complete a later picker request.

Subscriptions poll the durable current task with monotonically increasing sequence numbers. Reconnecting reads the current snapshot. Unsubscribing suppresses late responses. The page has no generic Agent host, configuration, permission approval or question-answer API.

## Execution and authorization

- A dedicated process-owned native model host initializes without opening a conversation. Provider configuration changes revoke and reap old tasks before replacement. A normal chat changing only its default model does not rebuild the app model host or change the app-selected model.
- Every task receives a fresh in-memory orchestrator, task workspace, read state and session ID. It has no parent transcript, personal memory, skills, plugin discovery, personal hooks, delegation or automatic MCP connection. The bootstrap does not join ordinary chat cron/report delivery.
- Managed permission rules and global user deny/ask restrictions remain effective. Chat allow rules, bypass mode and additional directories are excluded.
- Native app details select the model, limits and exact tools. The model does not change the active conversation's selection or silently switch providers. MCP discovery requires a native action against an existing host configuration; apps cannot supply connection URLs or credentials.
- MCP grants bind the server configuration slot/revision, original tool name and definition digest. Current configuration is checked at tool discovery and RPC dispatch, including retries. A failed live guard irreversibly revokes the captured lease.
- Permission and question proposals appear in native host UI with the owning app ID. Permission approval is for one operation.
- Default limits are one active task per app, 20 turns and ten minutes. Cancelling holds admission until foreground/tool cleanup completes. Stops, revocation, deletion and version handoff invalidate the invocation before release of its resources.
- Mobile background Agent execution is currently unsupported and native configuration rejects it. Page close/background cancels tasks; native camera/photo picker transitions do not count as a page close. Platform background execution requires separate work and acceptance.

## Durable state and results

The host stores input hashes, request deduplication, grants, task state, results and usage. It does not store prompts or images in the task ledger. Identical retries reuse the task; conflicting input under one request ID fails. Restart marks unfinished work interrupted and does not replay external side effects. Results remain scoped to the same app/release; running tasks additionally require the original invocation.

Structured output uses the internal result tool with offline Draft-7 validation (`jsonschema 0.46.10`, formats disabled). Remote schema references and other dialects are rejected. Text resembling JSON is not a structured success. Usage comes from the runtime; it is not a hard monetary-budget guarantee.

## Acceptance fixture

`fixtures/local-apps/v2/calorie-agent` uses native camera/photos, image compression, a structured nutrition result, editable portions and explicit save confirmation. Nutrition estimates remain application behavior, outside the generic Agent interface.

The current source checks, device coordination, build constraints and outstanding acceptance cases are recorded in `/Users/luolingfeng/lingxi/acceptance/mobile-app-agent-20261023/report.html`.

The current Mobile implementation executes locally through its native host. Selecting a paired Desktop Agent and the dedicated cross-device app-task transport remain unimplemented; the ordinary Remote Control chat gateway is not exposed to Local Apps.
