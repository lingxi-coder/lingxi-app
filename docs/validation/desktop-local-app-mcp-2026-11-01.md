# Desktop Local App MCP — 2026-11-01

Desktop Local Apps now declare standard MCP servers in schema-2 `mcpServers`.
The Electron host uses the official `@modelcontextprotocol/sdk` (1.32.2), with
managed stdio children or Streamable HTTP endpoints on declared backend processes.
Native Desktop sessions and the App Agent catalog discover the private host
endpoint without modifying user/project MCP configuration.

## Contract

- `mcp` and exact-release process consent are required; managed MCP declarations
  support Desktop native targets. The publication entry remains static/server.
- Stdio is owned by the app run, including its process group and parent-death
  guardian. HTTP binds a named app process and plain endpoint path; the host
  injects a per-run `LOCAL_APP_MCP_TOKEN` for server-side authentication.
- Preparation initializes servers before activation. Failed preparation keeps
  the previous serving release. Tools register only for active authorized runs.
- Reconnect restores discovery, including changes missed while the notification
  stream was disconnected. Calls are never automatically replayed. Failed child
  cleanup blocks replacement spawning until cleanup succeeds.
- Public tool names are stable; release and definition fingerprints prevent stale
  grants or discovery snapshots from authorizing changed operations.
- Stop/revoke closes startup admission before awaiting cleanup. Revocation persists
  even when process cleanup needs retry. Uninstall clears browser storage/cache
  before removing the durable app record, leaving a retry path on failure.
- HTTP sessions use bounded DELETE cleanup. Interrupted cleanup waits retain their
  completion receipt. Native catalog success and error paths drain the runtime.
- The gateway exports ordinary tools only. Resources, prompts, MCP task execution,
  sampling and elicitation are not forwarded.

Desktop Local App WebMCP injection, its served script, vendored polyfill and sync
script were removed. No legacy data decoder or migration adapter was added.
Mobile WebMCP and the general browser integration are separate features.

`local-app` owns app design/publication/lifecycle guidance; `lingxi-api` owns host
API/configuration guidance. Both prompts document the platform split, separate
App-to-Agent permissions, managed MCP lifecycle, and independent-service routing.

## Validation

- Local App Rust library: 158 tests passed, including manifest/consent and skill tests.
- Rust private discovery: 2 tests passed.
- Rust HTTP transport: 12 tests passed, including missed-notification recovery
  and retrying an interrupted DELETE cleanup wait on real loopback HTTP/SSE servers.
- Desktop lifecycle/host/package/guardian/catalog regressions: 153 tests passed.
- Official SDK integration: all 12 tests passed without skips, covering stdio/HTTP,
  discovery changes, crash/no replay,
  failed replacement, revocation, cancellation during initialization, cleanup retry,
  stale definitions, authentication, and uninstall.
- Native model-route projection unit test passed for static and account routes;
  unavailable accounts remain unselectable.
- Real Rust store + native App Agent catalog: passed discovery before/after app
  start/stop and verified the catalog process terminates its HTTP session.
- Real Electron Local Apps fixture: passed library, independent windows, backend,
  permissions, HTML/AG-UI injection, and durable updates after WebMCP removal.
- Electron Node/Web TypeScript checks and skill validation passed. The generic
  skill validator was run on a temporary copy without LingXi's supported
  `session-modes` extension; the real skill retains that field.

Native catalog integration also exposed per-model reconstruction of the account
model directory. A single catalog query now shares a model snapshot; account-backed
routes use the account-aware validator without making unavailable models selectable.

No signed package, Windows/Linux host, real-provider inference, or long-duration
soak acceptance is claimed. No deployment was performed.
