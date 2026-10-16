# Desktop · Electron host

Session-owned Bridge sidecars, renderer IPC, event replay, native services and signed packaging.

<a id="desktop-ownership"></a>

## Composition and ownership

Electron main is the desktop composition host. SessionRuntimeManager owns per-session SessionRuntime objects, each with project context, Bridge connection, optional child-process ownership, lifecycle generation and pending interactions. Renderer components consume the preload API and shared DTOs; they do not start the Rust sidecar or read its lockfile token directly.

The Rust bridge-server resolves product launch state and routes ClientCommand into Harness desktop assembly. Provider communication remains in LLM Client; native device audio, account/credential services, filesystem selection and windows belong to the host. A browser docs page is a separate application and does not share this in-process Electron transport.

![Renderer IPC reaches Electron main, which authenticates its Bridge sidecar; mobile uses UniFFI.](../assets/platform-host-transports.svg)

<a id="desktop-flow"></a>

## Startup and renderer attachment

Packaged builds resolve bridge-server only from process.resourcesPath/bin. Development can use an explicit serverBin or LINGXI_BRIDGE_SERVER_BIN, then search workspace target/debug and target/release. This avoids a packaged application silently adopting a development executable. Reusable processes are checked for live process identity, private owned lockfiles and compatible launch identity before adoption.

The preload keeps a sequenced runtime replay buffer for renderer attachment. Conversation hydration and live delivery must share source generation and session identity, so older resume results cannot overwrite the active conversation. Disconnect/restart rejects outstanding correlated operations and clears the previous interactions; the host distinguishes its owned process from an adopted external process when disposing.

1. Resolve public settings, credential references, active project and session.
2. Resolve or start the sidecar using the host launch identity and exact discovery file.
3. Connect Bridge, verify hello and install permission, question, event and audio handlers.
4. Hydrate persisted transcript, attach sequenced replay and render subsequent live events.

<a id="desktop-ipc"></a>

## IPC, native services and credentials

BrowserWindow enables sandbox and contextIsolation. Preload exposes explicit command and subscription methods; main validates the sender, session reference, command variant and payload before forwarding. A new runtime command requires matching Rust/TypeScript DTOs, main admission, preload typing and renderer ownership. A typed renderer value alone is not admission evidence.

Main resolves provider credentials at launch through the secure host services and sends only what the selected engine needs. Diagnostics use sanitizeDiagnostic and bounded buffers. openai_oauth_updated is handled as a credential event and is excluded from Bridge iterator replay. On macOS the signed Credential Broker verifies packaged identity, so unpackaged npm dev is unsuitable for proving secure credential persistence.

| Boundary | Owned behavior |
| --- | --- |
| Renderer → preload/main | Typed IPC, trusted sender and session validation. |
| Main → Bridge | Lockfile authorization, hello, lifecycle and broker responses. |
| Main → native services | Audio leases, OS dialogs, credential storage and desktop windows. |
| Bridge → Harness | Session/turn execution, permission gates, tools and persisted history. |

<a id="desktop-development"></a>

## Development, packaging and failure diagnosis

Build the private Bridge client before Electron so file dependencies contain matching dist exports. A source/typecheck success establishes JavaScript integration; sidecar and package verification additionally check resource layout, signing and actual process launch. The macOS release path is package:mac:flare, which preserves source path remapping, inside-out signing and package verification with the configured Flare team.

Diagnose by boundary: missing packaged binary → resources; HTTP 401 → discovery token; hello refusal → protocol/build pairing; pending question/permission → broker state; stale UI → replay generation; rejected macOS credentials → signed application/broker identity. Focused renderer tests do not validate operating-system grants, audio devices or Windows/Linux packaging.

### Desktop source checks

```bash
cargo build --locked -p bridge-server --bin bridge-server
npm --prefix packages/bridge-client ci
npm --prefix packages/bridge-client run build
npm --prefix apps/electron ci
npm --prefix apps/electron run typecheck
npm --prefix apps/electron test
npm --prefix apps/electron run build
```

Verification method: Environment validation

### macOS signed package workflow

```bash
cd apps/electron
npm run package:mac:flare -- --check
npm run package:mac:flare
npm run verify:package
```

Verification method: Environment validation

- [Bridge wire details](bridge-client.md)
- [Build matrix](platform-builds.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app/tree/main/apps/electron/src/main/bridge.ts)

- [lingxi-app/apps/electron/src/main/index.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/main/index.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/src/main/bridge.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/main/bridge.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/src/main/bridgeDiscovery.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/main/bridgeDiscovery.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/src/main/validation.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/main/validation.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/src/main/host-utils.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/main/host-utils.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/src/preload/index.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/preload/index.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/src/preload/event-replay.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/preload/event-replay.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/src/renderer/bridge/useBridge.ts](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/src/renderer/bridge/useBridge.ts) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/scripts/package-macos-flare.sh](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/scripts/package-macos-flare.sh) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)
- [lingxi-app/apps/electron/package.json](https://github.com/lingxi-coder/lingxi-app/blob/86b0c95bccdf6182701d5c0a63c8cede93f2fd6a/apps/electron/package.json) — Committed source (86b0c95bccdf6182701d5c0a63c8cede93f2fd6a)

- lingxi-app: HEAD 86b0c95bccdf6182701d5c0a63c8cede93f2fd6a; Referenced input fingerprint sha256:a43c7b53f8204a3396d3fd5d52d7942bac36d65bd885d68ea36c4987bf75758c

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
