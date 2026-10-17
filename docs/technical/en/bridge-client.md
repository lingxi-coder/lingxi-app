# TypeScript Bridge Client

Connect a Node.js host to the local LingXi Bridge service, submit commands, consume events, and resolve permission requests.

<a id="integration-boundary"></a>

## Choose this integration

The Bridge client is the product repository’s private TypeScript package. It exports protocol DTOs, lockfile discovery, validation utilities, and a Node-side WebSocket client. Electron consumes its compiled dist exports through a local file dependency.

BridgeClient imports node:events and ws. Run it in Node.js or an Electron host process; browser applications need an appropriate host transport. This package is not a hosted LLM HTTP API, and there is no public npm installation contract for it.

### Build from the product checkout

```bash
cargo build --locked -p bridge-server --bin bridge-server
npm --prefix packages/bridge-client ci
npm --prefix packages/bridge-client run build
```

Verification method: API excerpt

### Local package dependency

```json
{
  "dependencies": {
    "@lingxi/bridge-client": "file:../../packages/bridge-client"
  }
}
```

Verification method: API excerpt

Commands run from the product repository root. The file dependency above is relative to apps/electron/package.json; adjust it for another local consumer. Start the Bridge service through the product host before connecting.

<a id="connect-and-stream"></a>

## Connect and receive a turn

connect() discovers a lockfile in ~/.lingxi/bridge unless lockfilePath or lockfileDir is supplied. It connects to ws://127.0.0.1:<port>/mcp using the mcp subprotocol and the lockfile token in X-LingXi-Ide-Authorization, then exchanges hello messages and checks protocol compatibility.

sendPrompt() submits a command without waiting for its reply. Text, tool activity, and the final turn outcome arrive through ClientEvent. Subscribe with events() before connecting or submitting: only subscribed iterators retain incoming events. The example resolves unattended permission, computer-access and questionnaire requests by denial/cancellation; a product host should present them and apply the user’s decision.

### Node.js / TypeScript consumer

```typescript
import { BridgeClient } from '@lingxi/bridge-client';

const client = new BridgeClient({ clientName: 'documentation-example' });
const events = client.events();
client.on('error', (error: Error) => console.error(error.message));
client.on('permission', (request) => {
  client.denyPermission(request.request_id);
});
client.on('computerAccess', (request) => {
  client.denyComputerAccess(request.request_id);
});

try {
  await client.connect();
  client.sendPrompt('Describe the repository structure without changing files.');

  for await (const event of events) {
    if (event.type === 'ask_user_question') client.cancelAskUserQuestion(event.request.request_id);
    if (event.type === 'text_delta') process.stdout.write(event.text);
    if (event.type === 'turn_ended') {
      console.log(event.outcome, event.cost);
      break;
    }
  }
} finally {
  await events.return?.();
  client.close();
}
```

Verification method: Compile check (bridge-command-events)

<a id="client-api"></a>

## Connection and command API

### BridgeClient

```typescript
new BridgeClient(opts: BridgeClientOptions = {})
```

Configure lockfilePath, lockfileDir, host, clientName, audioCapabilities, and handshakeTimeoutMs. The default handshake timeout is 10,000 ms.

### connect

```typescript
connect(): Promise<ServerHello>
```

Open the authenticated connection and finish the version handshake before sending commands.

### sendPrompt

```typescript
sendPrompt(text: string, opts: { images?: ImageRefDto[]; turnId?: number; visualizationContext?: VisualizationRefDto } = {}): void
```

Submit text, inline media_type/base64 images, an optional caller turn identifier, and visualization context.

### sendCommand

```typescript
sendCommand(command: ClientCommand): void
```

Send a typed protocol command. A disconnected client or oversized frame throws before the message is sent.

### cancel

```typescript
cancel(turnId?: number): void
```

Cancel the current turn or an explicitly selected turn.

### events

```typescript
events(): AsyncIterableIterator<ClientEvent>
```

Subscribe immediately and consume ordered events until close. Events before subscription are not replayed; all iterators share one queue and divide events. Returning the last iterator releases its buffer. openai_oauth_updated goes only to the host event listener.

### close

```typescript
close(): void
```

Close the underlying WebSocket. Retain the client for the full command and event lifecycle.

<a id="permissions"></a>

## Permissions and interactive requests

Permission and computer-access requests are separate listener events. Resolve each request with its request_id. The generic event feed does not replace the permission listener. Register listeners before submitting the first prompt.

### approvePermission / denyPermission

```typescript
approvePermission(requestId: number, response: PermissionResponseDto = { type: "allow_once" }): void
denyPermission(requestId: number): void
```

Resolve a parked tool permission request. Choose an explicit response when broader permission semantics are needed.

### approveComputerAccess / denyComputerAccess

```typescript
approveComputerAccess(requestId: number, response: ComputerAccessResponseDto): void
denyComputerAccess(requestId: number): void
```

Resolve computer tool access independently from ordinary tool permissions.

### answerAskUserQuestion / cancelAskUserQuestion

```typescript
answerAskUserQuestion(requestId: number, answers: Record<string, string>): void
cancelAskUserQuestion(requestId: number): void
```

Return a keyed answer set for an interactive questionnaire, or cancel the pending request.

<a id="wire-handshake"></a>

## Wire contract and version handshake

The current working-tree constants are BRIDGE_PROTOCOL_VERSION = 0.2.0 and CLIENT_PROTOCOL_VERSION = 25.0.0. Rust is the contract source; the TypeScript DTO file mirrors it. The envelope and DTO contract are checked independently, each requiring the same numeric major version. Malformed or mismatched versions refuse hello and bar later commands; server hello rejection uses wire error code -32600.

Frame is adjacent-tagged as type/payload; ClientCommand and ClientEvent are internally tagged with snake_case type values. Optional fields may be absent. Tool input_json/result_json payloads remain JSON strings, so clients must parse the contents deliberately and must not change their wire representation to objects. The mcp path/subprotocol names do not make product commands ordinary MCP tools/list or tools/call requests.

1. Host starts the local service and writes a protected <port>.lock containing authToken.
2. Client chooses an explicit lockfile for its session, sends the authorization header and requests the mcp WebSocket upgrade.
3. Client sends request/hello; server verifies both major versions and responds with ServerHello.
4. Only after connect resolves does the host submit commands and consume events/interactive requests.

![Electron uses authenticated WebSocket Bridge, native hosts use in-process UniFFI, both reach Harness.](../assets/platform-host-transports.svg)

Transport framing is separate from the shared ClientCommand / ClientEvent contract.

### Current hello request shape

```json
{
  "type": "request",
  "payload": {
    "id": 1,
    "method": "hello",
    "params": {
      "protocol_version": "0.2.0",
      "client_name": "documentation-example",
      "capabilities": {
        "supports_streaming": true,
        "supports_tools": true,
        "supports_skills": true,
        "supports_commands": true,
        "client_protocol_version": "25.0.0"
      }
    }
  }
}
```

Verification method: API excerpt

<a id="correlation-and-errors"></a>

## Correlation, errors and connection disposal

An outbound frame id correlates request/response envelopes. It is not a turn_id, session_id or broker request_id. sendCommand/sendPrompt are fire-and-forget: their request ids are not registered as awaited responses, and completion is a ClientEvent such as turn_ended. Event-replied product operations must correlate the payload request identifier together with session identity; SessionEventCorrelator rejects duplicate requests, times out missing replies and can reject a disposed session.

Keep three failure surfaces distinct: BridgeWireError is an envelope response with numeric code/message; streamed ClientEvent.error reports runtime failure during an accepted operation; ClientError is the native submit call-result error with transport/protocol/rejected/not_found/internal classes. A successful send or native submit does not establish a successful model turn. Wait for the terminal outcome and retain diagnostic context without logging auth tokens or provider credentials.

The endpoint reads at most 16 MiB per frame. BridgeClient checks UTF-8 serialized frame bytes before sending, protecting the whole connection from an oversized prompt, image or audio response. Socket close rejects pending envelope requests and ends subscribed iterators. Server connection teardown cancels owned turn/broker work; a host reconnect must restore persisted conversation state and clear old pending interactions, rather than automatically replaying a command whose result is uncertain.

| Identifier | Owner / use |
| --- | --- |
| Frame.payload.id | One connection request/response exchange. |
| session_id | Durable conversation and runtime ownership. |
| turn_id | One turn and targeted cancellation; fields may be optional. |
| permission / questionnaire request_id | Pending interaction broker request, answered once. |
| AudioOperationIdDto | Full id + generation + service_epoch identity. |

<a id="bridge-validation"></a>

## Integration verification and troubleshooting

The product package tests cover handshake refusal, frame bounds, malformed inbound DTOs, event iterator ownership, computer access and question correlation. Run them before changing a shared contract. The source index points to declarations; protocol fixtures and Rust round-trip tests establish serialization behavior. They do not establish a remote server service, physical audio device or real provider acceptance.

No lockfile means the intended service is not discoverable; an HTTP 401 means an upgrade-token mismatch; incompatible hello means a server/client build mismatch; a turn with no events may indicate a late iterator subscription or a parked permission/question. For multiple sessions use the host-provided exact lockfilePath instead of assuming the most recently modified file is the desired session. Never copy lockfile authToken into a webpage, example repository or diagnostic report.

### Bridge package and protocol checks

```bash
npm --prefix packages/bridge-client run build
npm --prefix packages/bridge-client test
cargo test --locked -p bridge --test protocol_roundtrip_test
```

Verification method: Environment validation

- [Desktop lifecycle](desktop-host.md)
- [Native callback integration](platform-integration.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app/tree/main/packages/bridge-client)

- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/packages/bridge-client/src/client.ts) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [lingxi-app/packages/bridge-client/src/protocol.ts](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/packages/bridge-client/src/protocol.ts) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [lingxi-app/packages/bridge-client/src/version.ts](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/packages/bridge-client/src/version.ts) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [lingxi-app/packages/bridge-client/src/lockfile.ts](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/packages/bridge-client/src/lockfile.ts) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [lingxi-app/packages/bridge-client/src/eventCorrelation.ts](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/packages/bridge-client/src/eventCorrelation.ts) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/apps/bridge-server/src/server.rs) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/apps/bridge-server/src/router.rs) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [harness-runtime/crates/bridge/src/wire.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/bridge/src/wire.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/bridge/src/mcp_endpoint.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/bridge/src/mcp_endpoint.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/client/src/protocol/commands.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/client/src/protocol/events.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/client/src/protocol/error.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/client/src/protocol/error.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)

- lingxi-app: HEAD c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b; Referenced input fingerprint sha256:9ac79cec993505292f531b296b57eb329900eb70aaaa7e34660a0857d25913b6
- harness-runtime: HEAD d2e3aed63ef232526967a8bf176a06d6a25ae8fa; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin d2e3aed63ef232526967a8bf176a06d6a25ae8fa

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
