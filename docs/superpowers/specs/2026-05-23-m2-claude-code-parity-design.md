# M2 · claude-code Behavioral Parity — Design Spec

**Date**: 2026-05-23
**Status**: Draft
**Author**: luolingfeng + Claude Sonnet 4.7
**Supersedes**: v0.2.0 "production desktop platforms" (which shipped pragmatic scaffolds)

---

## 0. Reading order

1. §1 **Goal** — what M2 must deliver.
2. §2 **v0.2.0 baseline** — what already shipped, what's wrong, what to keep.
3. §3 **"1:1" framing** — what behavioral parity means when porting TS to Rust.
4. §4 **TS-dep → Rust strategy** — explicit mapping table.
5. §5 **Out of scope** — Cloud Remote Control bridge and why it's M3+.
6. §6 **The seven plans** (M2-01 through M2-07) — bulk of the spec.
7. §7 **Cross-plan concerns** — version pinning, capability matrix, testing.
8. §8 **Verification & release**.

If you only have 5 minutes, read §1, §3, the §6 summary table, and §8.

---

## 1. Goal

Deliver `v0.3.0` of LingXi Core with **observable behavioral parity** against the
2026-03-31 claude-code TypeScript reference for the desktop platform crates
(`platforms/posix/`, `platforms/windows/`).

Parity means: same settings schema, same error strings, same OS support matrix,
same file paths, same protocol identifiers. It does **not** mean same library
(Rust cannot consume TypeScript packages) and does **not** mean same internal
control flow.

In numbers: ~4000-5500 lines of new Rust, modifications to ~10 existing crates,
1 new shared crate (`lingxi-jsonrpc`), 1 release tag (`v0.3.0`).

---

## 2. v0.2.0 baseline

### What v0.2.0 shipped (correct direction, keep)

- `platforms/posix/` crate scaffold with real impls for `Clock`, `FileSystem`
  (Linux inotify), `HttpTransport` (reqwest, sans SSE), `ProcessRunner`
  (`tokio::process`, foreground only), `RuntimeSpawner`, `SecureStorage`
  (PlainTextFile fallback), `WorktreeManager` (git CLI).
- `platforms/windows/` crate scaffold mirroring posix with tokio fallbacks.
- 35-crate workspace; 104/104 tests passing; clippy clean; tagged `v0.2.0`.

### What v0.2.0 got wrong (must correct)

These divergences are documented in §6.1 (Plan M2-01) — listed here in summary:

1. **Windows sandbox file exists** but claude-code does not support sandbox on
   Windows at all. Whole file should return `Unsupported` with the exact error
   string.
2. **`lingxi-bridge` crate invented an 8-char pairing protocol** plus
   project-scoped JWT plus 9 `BridgeMessage` variants. claude-code has nothing
   of the kind. The local IDE bridge is **MCP-over-WebSocket via lockfiles**
   (`~/.claude/ide/<port>.lock`); the cloud Remote Control bridge is a separate
   ~14k-line subsystem (out of scope, see §5).
3. **`WorktreeManager` uses branch prefix `lingxi/`** and a configurable
   `worktree_base`. claude-code mandates `claude/<slug>` branch names and
   `<repo>/.claude/worktrees/<flattened-slug>` paths with strict slug validation.
4. **Windows swarm file exists** but claude-code does not support tmux/swarm
   on Windows. Should return `Unsupported`.

These three crates need surgical edits in Plan M2-01 before subsequent plans
can build cleanly on top.

### What v0.2.0 stubbed (will be closed by M2-02..M2-06)

- All MCP `list_tools`/`call_tool`/etc. return `Internal("M2 follow-up")`.
- All LSP `request`/`notify` return `Transport("M2 follow-up")`.
- `Sandbox::prepare` is a policy-validating no-op (no OS isolation).
- `FileSystem::watch` on macOS returns empty stream (no FSEvents).
- `FileSystem::watch` on Windows returns empty stream (no ReadDirectoryChangesW).
- `SwarmBackend` on posix returns `Tmux("M2 follow-up")` for all methods.
- `BridgeTransport` returns `Unsupported` for everything.
- `SecureStorage` is plaintext on all platforms (no macOS Keychain shell-out).
- `HttpTransport::stream_sse` returns `InvalidRequest`.
- `ProcessRunner::spawn_background` returns `Unsupported`.
- `WorktreeManager::cleanup_stale` calls `git worktree prune` but discards
  the output.

---

## 3. The "1:1" framing

When porting TypeScript to Rust, "1:1" has to be defined precisely. We use the
following discriminators:

### Behavioral parity (in scope for "1:1")

- **Settings schema** — every field name, default value, validation rule, and
  JSON shape that appears in `~/.claude/settings.json` or a managed-settings
  drop-in MUST match. Managed-policy compatibility hinges on this.
- **Error strings** — strings shown to the user (via CLI output, `/doctor`,
  `/sandbox`, tool failure messages) MUST match byte-for-byte. Examples:
  `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`,
  `"Already in a worktree session"`, `"MCP server "<name>" tool "<tool>" timed
  out after Ns"`.
- **File paths** — `~/.claude/worktrees/<slug>`, `~/.claude/ide/<port>.lock`,
  `~/.claude/.credentials.json`, keychain service names. Cross-version state
  compatibility depends on these.
- **Wire identifiers** — MCP client `name: "claude-code"`, capabilities
  `{roots:{}, elicitation:{}}` (note the empty object — Java SDK rejects
  `{form:{}, url:{}}`), git branch prefix `claude/`, MCP tool prefix
  `mcp__<server>__<tool>`.
- **OS support matrix** — sandbox: macOS + Linux + WSL2; reject WSL1 and
  Windows. tmux/swarm: macOS + Linux; reject Windows. LSP: from plugins only
  (not user/project settings).
- **Public function names / module layout** to the extent reasonable for code
  archeology by future contributors who know claude-code first.

### Implementation freedom (not "1:1")

- **Library choice** — `notify` instead of `chokidar`; `tokio-tungstenite`
  instead of `ws`; `reqwest` instead of `axios`/`@anthropic-ai/sdk`. Rust ports
  inevitably swap libraries.
- **Control flow idioms** — `Result<T, E>` and `?` instead of try/catch;
  `enum` discriminated unions instead of string-tagged objects; `Arc<RwLock>`
  instead of singleton modules.
- **Concurrency primitives** — tokio tasks instead of Node event loop; this
  changes how cancellation propagates but not what the user observes.
- **Internal type names** — `Sandbox` (Rust trait) vs `BaseSandboxManager` (TS
  class); only the public-facing surface needs to align.

---

## 4. TS dep → Rust strategy

For each TypeScript dependency in claude-code, we picked the Rust equivalent
strategy. Categories:

- **(A) Drop-in crate** — `npm` and `crates.io` ecosystems both have it.
- **(B) Shell-out** — claude-code shells out to an external CLI; we do the same.
- **(C) Reimplement** — no Rust equivalent on crates.io; we own the
  implementation.
- **(D) Net-new work** — small utility absent from v0.2.0.

| TS dep | Used in claude-code for | Rust strategy | Category |
|---|---|---|---|
| `@anthropic-ai/sandbox-runtime` | sandbox config + dispatch | `crates/sandbox/` + `platforms/*/sandbox.rs` — we reimplement | C |
| `@modelcontextprotocol/sdk` | MCP client (stdio/http/sse/ws/in-process) | `crates/mcp/` + new `crates/jsonrpc/` — we reimplement | C |
| `@anthropic-ai/sdk` | Anthropic API client (SSE) | `crates/api-client/` (already exists) + `reqwest::Response::bytes_stream` — we reimplement | C |
| `vscode-jsonrpc/node` | LSP JSON-RPC client | `lingxi-jsonrpc` (shared with MCP) | C |
| `vscode-languageserver-protocol/types` | LSP message types | `lsp-types` crate | A |
| `chokidar` 4 | FS watcher | `notify` crate + `notify-debouncer-mini` for `awaitWriteFinish` | A |
| `ws` 8 | WebSocket transport | `tokio-tungstenite` | A |
| `axios` | HTTP client | `reqwest` (already in v0.2.0) | A |
| `execa` | subprocess (utilities) | `tokio::process::Command` (already in v0.2.0) | A |
| `node-pty` | PTY (web pty-server only) | `portable-pty` crate (deferred to M3+) | A |
| `tree-kill` | kill process tree | `nix::sys::signal::killpg(-pgid, SIGTERM)` (Unix) / `taskkill /T /F /PID` (Windows) | D |
| `proper-lockfile` | file locking | `fs2::FileExt::lock_exclusive` (already in v0.2.0) | A |
| `child_process.spawn` | bash spawn | `tokio::process::Command` (already in v0.2.0) | A |
| `crypto` | hashing | `sha2`, `blake3` crates (already in v0.2.0) | A |
| `bubblewrap` / `socat` (CLI) | Linux sandbox primitive | shell-out (matches claude-code's `@anthropic-ai/sandbox-runtime` internals) | B |
| `sandbox-exec` (CLI) | macOS sandbox primitive | shell-out (same) | B |
| `security` (CLI) | macOS Keychain | shell-out (matches claude-code) | B |
| `tmux` (CLI) | swarm tmux backend | shell-out | B |
| `osascript` (CLI) | swarm iTerm backend | shell-out | B |
| `git worktree` (CLI) | worktree management | shell-out (already in v0.2.0) | B |

### Net-new utility work (category D)

These are small but absent from v0.2.0 and required for parity:

- **tree-kill equivalent** — Unix `nix::sys::signal::killpg`, Windows `taskkill`.
- **macOS Keychain prefetch** — startup async warm of the 30s cache.
- **Process pgid tracking** — needed for clean tree-kill.
- **`pwd -P` cwd tracking** — append `&& pwd -P >| $cwdfile` to bash commands;
  read cwd after execution; claude-code does this synchronously via
  `readFileSync` to avoid microtask races.

---

## 5. Out of scope: Cloud Remote Control bridge

claude-code's `src/bridge/` directory (24 files, ~14k lines) implements the
"Remote Control" feature: a `claude remote-control` mode polls Anthropic's
environments API for "work" jobs, decodes a `WorkSecret` (JWT prefixed
`sk-ant-si-`), and drives a worker session via WebSocket transport to
session-ingress. Authentication flows through OAuth-derived trusted-device
tokens.

This subsystem is tightly coupled to `claude.ai` subscription billing and
session-ingress infrastructure. For a desktop CLI port that does not need to
connect to `claude.ai` worker pools, it is dead weight.

**Decision**: defer to M3 or a separate milestone. The `crates/bridge/`
rewrite in Plan M2-01 covers the local IDE bridge (lockfile-based MCP-over-WS)
but does **not** include cloud Remote Control. If/when business requires
claude.ai integration, a dedicated milestone owns it.

Other deferrals:
- **`node-pty` web pty-server** — claude-code has a separate web UI that
  exposes a PTY over WebSocket. Not part of the CLI path. Deferred to M4 (UI
  Layer per spec §35).
- **Computer-use / Chrome MCP servers** — claude-code has in-process MCP
  servers for browser automation. The Rust port can support `InProcess` MCP
  transport in M2-02, but the actual computer-use server is its own project.

---

## 6. The seven plans

### Summary table

| Plan | Goal | New code (LOC est.) | Touched crates | Sequential? |
|---|---|---|---|---|
| M2-01 | v0.2.0 corrections + scaffold prep | ~400 added, ~800 deleted | bridge (delete only), platforms/posix, platforms/windows, secret | Required first |
| M2-02 | lingxi-jsonrpc + MCP client + bridge wiring | ~1500 | jsonrpc (new), mcp, platforms/posix/mcp, platforms/windows/mcp, bridge | Depends on M2-01 |
| M2-03 | LSP client + plugin-only registration | ~700 | lsp, plugin, platforms/posix/lsp, platforms/windows/lsp | Depends on M2-02 |
| M2-04 | Sandbox runtime (biggest single plan) | ~1200 | sandbox, platforms/posix/sandbox, platforms/windows/sandbox | Depends on M2-01 |
| M2-05 | FS watch (notify) + Swarm backends (tmux/iTerm/InProcess) | ~800 | platforms/posix/fs+swarm, platforms/windows/fs+swarm | Depends on M2-01 |
| M2-06 | SecureStorage macOS + HTTP SSE + Process spawn polish | ~800 | secret, api-client, platforms/posix/{secure_storage,http,process}, platforms/windows/{secure_storage,http,process} | Depends on M2-01 |
| M2-07 | Test infra + docs + tag v0.3.0 | ~600 | test-harness, docs | Depends on M2-01..M2-06 |

M2-02 and M2-03 must be sequential (LSP shares `lingxi-jsonrpc` infra from
M2-02). M2-04..M2-06 can be parallelized after M2-01. M2-07 runs last.

### 6.1 Plan M2-01: v0.2.0 corrections

**Goal**: surgical edits to bring v0.2.0 into the right shape before any new
feature work. Mostly deletions and renames.

**Changes**:

- `platforms/windows/src/sandbox.rs` — rewrite as a struct that returns
  `SandboxError::Unsupported` from every method. `is_available()` returns
  `false`. `probe_capability()` returns capability with
  `reason: Some("claude-code does not support sandbox on Windows".into())`.
- `platforms/windows/src/swarm.rs` — same pattern. Rename
  `WindowsSwarmBackend` to keep the type, but `is_available()` is `false`
  and all methods return `SwarmError::Unsupported`.
- `crates/bridge/` — **deletion phase only** (rewrite continues in M2-02
  once WebSocket MCP transport lands):
  - Delete: `codes.rs` (8-char pairing), `jwt.rs` (JwtVerifier),
    `rate_limiter.rs`, `pairing.rs`, the 9-variant `BridgeMessage` enum.
  - Reduce `transport.rs` (`IdeBridge`) to a stub returning
    `BridgeError::Unsupported` from every method, with module doc noting
    "Real impl wired in M2-02 after MCP WebSocket transport lands."
  - `state.rs` — keep, but simplify to just `connected: bool` +
    `current_file: Option<PathBuf>`.
  - Remove `crates/bridge/Cargo.toml` deps: `rand`, `sha2`, `hmac`, `base64`
    (no longer needed without JWT). Keep `tokio`, `serde`, `thiserror`.
  - Lockfile reader and MCP-over-WS wiring deferred to M2-02 §6.2.
- `platforms/posix/src/worktree.rs` and `platforms/windows/src/worktree.rs`:
  - Branch name: `format!("claude/{slug}")` (was `lingxi/{slug}`).
  - Path: parameter renamed from `worktree_base` to `repo_root`; actual path
    becomes `repo_root.join(".claude").join("worktrees").join(flatten_slug(slug))`.
  - Add `fn validate_worktree_slug(slug: &str) -> Result<(), WorktreeError>`:
    each `/`-separated segment must be alphanumeric + `_-.`, max 64 chars
    total. Reject empty segments. Implementation copies claude-code's regex.
  - Add `fn flatten_slug(slug: &str) -> String` — replace `/` with `_` to
    keep file names flat.
  - Add `copy_worktree_includes` parameter handling in `create_worktree`:
    iterate `copy_includes: &[PathBuf]`, copy each into the new worktree if
    it exists in the source.
  - `cleanup_stale` — actually parse `git worktree prune -v` stdout to
    return pruned paths.
- `crates/secret/src/keychain_prefetch.rs` — already has `KeychainPrefetch`
  type; M2-01 leaves it untouched (real prefetch wiring lands in M2-06).

**1:1 fidelity items to lock in**:
- `"Already in a worktree session"` error string in worktree create when
  source is itself a worktree (claude-code chdir's to canonical git root).
- Branch prefix exactly `claude/` (used by claude-code's `teleport.tsx` for
  title generation, must match).
- Worktree paths under `<gitRoot>/.claude/worktrees/` exactly.
- IDE lockfile shape (the JSON keys above) byte-for-byte from
  `src/utils/ide.ts`.

**Tests**:
- Update existing `e2e_single_turn.rs` to use new worktree paths (no
  functional change).
- New `crates/bridge/tests/lockfile_test.rs` — write a fake lockfile to
  tmpdir, discover, parse, verify selection of most recent by mtime.

**Dependencies**: none (operates on v0.2.0).

**Estimated complexity**: medium. Most edits are deletions or renames. The
lockfile reader is ~80 lines, worktree slug validation is ~50 lines,
worktree path/branch refactor is ~150 lines.

**Commit**: single commit `refactor(M2-01): correct v0.2.0 divergences from claude-code`.

---

### 6.2 Plan M2-02: lingxi-jsonrpc + MCP client

**Goal**: build a shared JSON-RPC framing crate for MCP and LSP; replace the
MCP stub with a real client matching claude-code's protocol behavior.

**New crate `crates/jsonrpc/`**:

- `Cargo.toml` — deps: tokio, tokio-util (for `Framed` + `Decoder`/`Encoder`),
  serde, serde_json, thiserror, async-trait, tracing, futures.
- `src/codec.rs` — `JsonRpcCodec` implementing `tokio_util::codec::Decoder`
  and `Encoder`. Supports two framing modes:
  - **Content-Length-prefixed** (LSP, modern MCP): `Content-Length: N\r\n\r\n<N bytes>`.
  - **Line-delimited** (older MCP servers): one JSON object per `\n`-terminated line.
  - Auto-detect: first byte is `C` (Content-Length) → header mode; otherwise line mode. Codec keeps an internal `mode: Mode` field after the first frame is decoded.
- `src/messages.rs` — `JsonRpcMessage` enum (Request / Response / Notification),
  `RequestId` newtype, `JsonRpcError` shape matching the JSON-RPC 2.0 spec.
- `src/router.rs` — `RequestRouter` holding `HashMap<RequestId, oneshot::Sender<Result<Value, JsonRpcError>>>`. Public API: `send_request(method, params) -> Future<Result<...>>`. Internal: writer task drains an outbound channel, reader task dispatches responses to pending senders. Cancellation: `Drop` on the future removes the entry from the map and cancels the oneshot.
- `src/broker.rs` — `NotificationBroker` using `tokio::sync::broadcast`. Public API: `subscribe(method: &str) -> BroadcastReceiver<Notification>`. Reader task fans out notifications by method name.
- `src/connection.rs` — `Connection` glues codec + router + broker. Constructor: `Connection::new(read: AsyncRead, write: AsyncWrite, mode: Mode) -> Connection`. Spawns reader/writer background tasks via the platform's `RuntimeSpawner` (passed in). Exposes `send_request`, `send_notification`, `subscribe_notifications`, `close`.
- Stderr capture: a separate concern — owned by the `crates/mcp/` consumer, not by `lingxi-jsonrpc`. The connection only handles stdin/stdout.

**Modifications to `crates/mcp/`**:

- `src/client.rs` (new file, ~400 lines): `McpClient` type holding a `lingxi_jsonrpc::Connection`. Methods:
  - `async fn initialize(&self) -> Result<ServerCapabilitiesDto, McpError>`
  - `async fn list_tools(&self) -> Result<Vec<McpToolDto>, McpError>`
  - `async fn list_resources(&self) -> Result<Vec<McpResourceDto>, McpError>`
  - `async fn list_prompts(&self) -> Result<Vec<McpPromptDto>, McpError>`
  - `async fn call_tool(&self, tool_name: &str, input: Value, timeout: Duration) -> Result<McpToolResultDto, McpError>` — `tokio::select!` between `send_request` and `tokio::time::sleep(timeout)`; on timeout emit error string `"MCP server \"{server_name}\" tool \"{tool_name}\" timed out after {timeout_secs}s"`.
  - `async fn read_resource(&self, uri: &str) -> Result<McpResourceContentDto, McpError>`
  - `async fn ping(&self) -> Result<(), McpError>`
  - `fn notifications_stream(&self) -> impl Stream<Item = McpNotificationDto>`
- `src/identity.rs` — constants: `MCP_CLIENT_NAME = "claude-code"`, `MCP_CLIENT_TITLE = "Claude Code"`, `MCP_CLIENT_VERSION = env!("CARGO_PKG_VERSION")`, `MCP_WEBSITE_URL = "https://claude.com/claude-code"`. Capability JSON: literal `{"roots":{}, "elicitation":{}}` constructed via `serde_json::json!`.
- `src/initialize_params.rs` — assembles the `initialize` JSON-RPC params per MCP spec with the identity constants above.

**Modifications to `platforms/posix/src/mcp.rs` and `platforms/windows/src/mcp.rs`**:

- Each `connect` impl branches on `McpTransportSpec`:
  - `Stdio` — spawns the child via `tokio::process::Command`, takes stdin/stdout/stderr handles, constructs `McpClient` over a `lingxi_jsonrpc::Connection::new_stdio(stdin, stdout)`, stores the client in the connection map.
  - `WebSocket { url, headers }` — opens `tokio-tungstenite::connect_async(url)` with the headers, splits the WS stream, adapts to `AsyncRead + AsyncWrite` via a small Sink/Stream-to-pipe shim, constructs `McpClient` over `lingxi_jsonrpc::Connection::new_websocket(ws_stream)`. This unblocks the IDE bridge work (`crates/bridge/`).
  - `Http`, `Sse`, `InProcess`, `SseIde`, `SdkControl` — return `Err(McpError::UnsupportedTransport(...))` for M2; M2.next or M3 can wire them.
- `initialize` delegates to `McpClient::initialize`.
- `list_tools`, `call_tool`, etc. delegate accordingly.
- `notifications` returns `McpClient::notifications_stream`.
- Stderr capture: 64MB ring buffer per connection (claude-code's
  `STDERR_BUFFER_CAP`). On connection failure, the buffer is included in
  the error message for diagnostics. WebSocket transport has no stderr —
  errors come from the WS error stream.

**Bridge wiring** (completes the rewrite started in M2-01):

- `crates/bridge/src/lockfile.rs` (new): discover `~/.claude/ide/*.lock`
  files, sort by mtime descending, parse JSON
  `{workspaceFolders, pid, ideName, transport, runningInWindows, authToken}`,
  filename pattern is `<port>.lock`. Returns the most recent valid entry.
- `crates/bridge/src/transport.rs` (rewrite, replacing M2-01's stub):
  `IdeBridge::connect()` calls `lockfile::discover_latest()`, then builds an
  `McpTransportSpec::WebSocket { url: format!("ws://localhost:{port}"),
  headers: HashMap::from([("Authorization", format!("Bearer {authToken}"))]) }`,
  hands off to `lingxi-mcp::McpRegistry::connect_with_spec()`. The bridge
  itself owns no JSON-RPC framing — it's a thin lockfile-discovery +
  WebSocket transport spec builder.
- `crates/bridge/Cargo.toml` (modify): add `lingxi-mcp = { path = "../mcp" }`.

**1:1 fidelity items to lock in**:
- MCP client `name: "claude-code"` literal — interoperability bugs hinge here.
- Capability shape literally `{"roots":{}, "elicitation":{}}` — empty elicitation
  object is required (Java MCP servers reject `{form:{}, url:{}}`).
- Tool full-name format `mcp__<server>__<tool>` — already covered in
  `crates/mcp` / `crates/tools` but verify after wiring.
- Tool metadata fields: `tool._meta?.['anthropic/searchHint']` (whitespace-collapsed)
  and `tool._meta?.['anthropic/alwaysLoad']` (`=== true`) — `McpToolDto` already
  has fields for these in v0.1.0; verify the deserialization path lands them
  correctly from the SDK response.
- Tool description max length: respect `MAX_MCP_DESCRIPTION_LENGTH` constant
  with truncation suffix `"… [truncated]"`.
- Stderr buffer 64MB cap (claude-code calls this out as a deliberate limit
  to avoid memory explosion on chatty servers).
- Timeout error format exact string (above).

**Tests** (in `crates/jsonrpc/tests/` and `crates/mcp/tests/`):
- `codec_test.rs` — encode/decode roundtrip for Content-Length and line modes,
  partial frames, oversize frames.
- `router_test.rs` — request/response pairing, timeout, drop-on-future-cancel.
- `broker_test.rs` — notification fanout, late subscribers.
- `mcp_client_test.rs` — using a mock JSON-RPC server in-process, verify
  `initialize` sends the literal identity / capabilities; `call_tool` enforces
  timeout with exact error string.

**Dependencies**: M2-01.

**Estimated complexity**: large. `lingxi-jsonrpc` is ~600 lines of foundational
async plumbing; MCP client is ~400 lines on top; platform-side glue (stdio +
WebSocket) is ~300 lines each (×2 platforms); bridge rewrite is ~200 lines.
Total ~1500 lines new code + ~300 deleted (stub returns).

**Commits**: 4 commits — `feat(jsonrpc): codec + router + broker`,
`feat(mcp): real client over lingxi-jsonrpc`,
`feat(platforms): wire MCP client + WebSocket transport into posix + windows`,
`feat(bridge): lockfile discovery + MCP-over-WS wiring`.

---

### 6.3 Plan M2-03: LSP client + plugin-only registration

**Goal**: real LSP client speaking JSON-RPC; lock LSP server registration to
the plugin code path.

**Modifications to `crates/lsp/`**:

- `Cargo.toml` — add `lsp-types = "0.95"`, `lingxi-jsonrpc = { path = "../jsonrpc" }`.
- `src/client.rs` (new, ~300 lines): `LspClient` over `lingxi_jsonrpc::Connection`. Methods:
  - `async fn initialize(&self, root_uri: &str) -> Result<LspServerCapabilities, LspError>`
  - `async fn request<P: Serialize, R: DeserializeOwned>(&self, method: &str, params: P) -> Result<R, LspError>`
  - `async fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), LspError>`
  - `async fn shutdown(&self) -> Result<(), LspError>`
- `src/passive_feedback.rs` (new, ~150 lines): subscribe to
  `textDocument/publishDiagnostics` notifications, accumulate into
  `LspDiagnosticRegistry`. Diagnostic record keyed by file URI.
- `src/diagnostic_registry.rs` — `LspDiagnosticRegistry` with
  `RwLock<HashMap<Url, Vec<Diagnostic>>>`. Public API: `get(url)`, `clear(url)`,
  `all_diagnostics()`.
- `src/tool_operations.rs` (new, ~250 lines): the 9 operations claude-code
  exposes as the `LSPTool`:
  1. `goToDefinition(file, line, character)`
  2. `findReferences(file, line, character, includeDeclaration)`
  3. `hover(file, line, character)`
  4. `documentSymbol(file)`
  5. `workspaceSymbol(query)`
  6. `goToImplementation(file, line, character)`
  7. `prepareCallHierarchy(file, line, character)`
  8. `incomingCalls(item)`
  9. `outgoingCalls(item)`

  Each operation:
  - Takes 1-based line/character (claude-code convention).
  - Internally converts to 0-based for LSP wire protocol.
  - File size cap: `MAX_LSP_FILE_SIZE_BYTES = 10_000_000` (10 MB). Reject
    files exceeding this.
  - Returns claude-code-shaped result (struct with file/range/preview).

- `src/registry.rs` — modify `register_config`:
  - Change visibility: `pub(crate) fn register_config(&self, config: LspServerConfig)`.
  - Add `pub fn register_plugin_servers(&self, plugin_id: PluginId, configs: Vec<LspServerConfig>)`.
  - Document: "Only plugin-loaded LSP servers are supported. User and project
    settings cannot register LSP servers." Matches claude-code's
    `config.ts::getAllLspServers()` which only consults `getPluginLspServers()`.

**Modifications to `crates/plugin/src/manager.rs`**:

- `load_plugin` LSP section — currently calls
  `lsp_registry.register_plugin_servers(...)` which is correct. Verify after
  M2-03 lands that no other code path calls `register_config`. Add a doc
  comment noting the constraint.

**Modifications to `platforms/posix/src/lsp.rs` and `platforms/windows/src/lsp.rs`**:

- `start_server` spawns child, constructs `LspClient`, stores in connection
  map. Handshake: await `spawn` event before `listen()` (claude-code calls
  this out — without it ENOENT propagates as unhandled rejection).
- `initialize` delegates to `LspClient::initialize`.
- `request` and `notify` delegate to the client.
- `shutdown` sends `shutdown` request + `exit` notification per LSP spec,
  then kills the child.

**1:1 fidelity items to lock in**:
- 1-based line/character at the tool boundary (LSP itself is 0-based).
- 10 MB file size cap with exact `MAX_LSP_FILE_SIZE_BYTES` value.
- Spawn options: `stdio: ['pipe','pipe','pipe']`, `windowsHide: true`.
- ENOENT race: await spawn before listen.
- LSP servers from plugins only (no user/project settings path).
- `textDocument/publishDiagnostics` accumulation into a registry the
  assistant can query.

**Tests** (`crates/lsp/tests/`):
- `lsp_client_test.rs` — mock LSP server (echoes initialize, returns canned
  hover), verify 1-based↔0-based conversion.
- `lsp_diagnostic_registry_test.rs` — publish diagnostics, verify accumulation.
- `lsp_plugin_only_test.rs` — verify `register_config` is not callable from
  outside `crates/lsp` (compile-fail test if practical, otherwise doc test
  asserting visibility).

**Dependencies**: M2-02 (uses `lingxi-jsonrpc`).

**Estimated complexity**: medium. LSP client thinner than MCP (no notifications
beyond diagnostics in M2 scope). ~700 lines new code total.

**Commits**: 2 commits — `feat(lsp): real client over lingxi-jsonrpc + 9 tool
operations`, `feat(platforms): wire LSP client into posix + windows`.

---

### 6.4 Plan M2-04: Sandbox runtime (biggest single plan)

**Goal**: reimplement `@anthropic-ai/sandbox-runtime` in Rust — config schema,
dependency check, dispatch to `bwrap+socat` (Linux) / `sandbox-exec` (macOS) /
unsupported (Windows, WSL1).

**New modules in `crates/sandbox/`**:

- `src/runtime_config.rs` (~250 lines): `SandboxRuntimeConfig` struct matching
  the zod schema. Fields (exact names from claude-code's
  `entrypoints/sandboxTypes.ts`):
  - `enabled: bool`
  - `failIfUnavailable: bool`
  - `enabledPlatforms: Option<Vec<Platform>>` where `Platform = Mac | Linux | Wsl`
  - `autoAllowBashIfSandboxed: bool`
  - `allowUnsandboxedCommands: Vec<String>`
  - `network: NetworkRestrictionConfig` with subfields `allowedDomains`,
    `allowManagedDomainsOnly`, `allowUnixSockets`, `allowAllUnixSockets`,
    `allowLocalBinding`, `httpProxyPort`, `socksProxyPort`
  - `filesystem: FilesystemRestrictionConfig` with `allowWrite`, `denyWrite`,
    `denyRead`, `allowRead`, `allowManagedReadPathsOnly`
  - `ignoreViolations: HashMap<String, Vec<String>>`
  - `enableWeakerNestedSandbox: bool`
  - `enableWeakerNetworkIsolation: bool`
  - `excludedCommands: Vec<String>`
  - `ripgrep: RipgrepConfig` with `command`, `args`
  - All struct fields use `#[serde(default)]` and `passthrough` semantics
    (unknown fields preserved when possible).
- `src/policy_convert.rs` (~200 lines): `convert_settings_to_runtime_config(
  settings: &SettingsJson) -> SandboxRuntimeConfig` — port the logic from
  claude-code's `sandbox-adapter.ts::convertToSandboxRuntimeConfig`:
  - Walk `permissions.allow` / `permissions.deny` for `Edit(...)`, `Read(...)`,
    `Bash(...)`, `WebFetch(domain:...)` rules.
  - Extract path patterns and apply `resolvePathPatternForSandbox` (handles
    `//` prefix, `/` prefix relative to settings file dir, `~/` passthrough).
  - Merge into `filesystem.{allowWrite,denyWrite,denyRead,allowRead}` arrays.
  - Extract domain patterns for `network.allowedDomains`.
- `src/dependency_check.rs` (~150 lines): `check_dependencies(platform: Platform)
  -> SandboxDependencyCheck` — probe for required tools:
  - macOS: check `sandbox-exec` exists (typically `/usr/bin/sandbox-exec`).
  - Linux/WSL2: check `bwrap` and `socat` in `$PATH`.
  - Windows: always returns "unsupported".
  - WSL1: detect via `/proc/version` (no "WSL2" / "microsoft-standard"
    substring) and refuse.
  - Returns `{ errors: Vec<String>, warnings: Vec<String> }`.
- `src/violation_store.rs` (~100 lines): `SandboxViolationStore` —
  `RwLock<VecDeque<SandboxViolationEvent>>` with bounded size (claude-code
  uses a circular buffer). `SandboxViolationEvent` shape: `{ timestamp,
  command, violation_type, message }`. UI consumer (e.g., `/sandbox doctor`)
  drains via `snapshot()`.
- `src/wrap.rs` (~250 lines): `wrap_with_sandbox(command: &str, policy:
  &SandboxRuntimeConfig, platform: Platform) -> Result<String, SandboxError>` —
  produces the actual shell-runnable command:
  - macOS path: generate an SBPL profile string (claude-code calls this
    `getSandboxProfile()`), write to a temp file, return
    `format!("sandbox-exec -f {profile_path} {command}")`.
  - Linux path: produce a `bwrap` invocation with appropriate `--ro-bind`,
    `--bind`, `--proc`, `--dev`, `--unshare-net` / `--share-net`, etc. flags
    derived from the policy. If `network` requires proxying, also start a
    `socat` companion process (managed lifecycle in `wrap.rs`).
  - WSL2 path: same as Linux (bwrap works).
  - Windows / WSL1: return `Err(SandboxError::Unsupported(reason))`.
- `src/decision.rs` (modify, ~150 lines added): expand the existing M1
  `should_use_sandbox` function with claude-code's
  `BashTool/shouldUseSandbox.ts` decision logic:
  - Split compound commands on `&&`, `;`.
  - For each subcommand, strip BINARY_HIJACK_VARS env-vars (`PATH=`, etc.)
    and safe wrappers (`sudo -E -- ...`).
  - Match against `excludedCommands` patterns and the GrowthBook gate
    `tengu_sandbox_disabled_commands` (M2 ships static list; GrowthBook
    integration is a separate feature).
  - Return `SandboxDecision` (existing enum from M1).
- `src/lib.rs` — re-exports new modules.

**Modifications to `platforms/posix/src/sandbox.rs`**:

- Real `Sandbox` trait impl:
  - `is_available()` — call `dependency_check::check_dependencies(platform)`,
    return `errors.is_empty()`.
  - `backend()` — return `SandboxBackend::LinuxNamespaces` /
    `SandboxBackend::MacOsSandboxExec` based on `Platform::detect()`.
  - `prepare(cmd, policy)` — call `wrap::wrap_with_sandbox(cmd_string, policy,
    platform)` to get the wrapped command, then construct a `SandboxedCommand`
    via the `__new_sandboxed` constructor with a shell wrapper:
    `ProcessCommand { command: "/bin/sh", args: ["-c", wrapped_string], ... }`.
  - `bypass_with_audit(cmd, reason)` — log the audit event, return
    `SandboxedCommand::__new_sandboxed(cmd, SandboxedTag::BypassAuditedWithReason{reason})`.
  - `probe_capability()` — return real `SandboxCapability` with
    `available: is_available()`, `reason: dependency_check.errors.first()`,
    `features` populated per platform.

**Modifications to `platforms/windows/src/sandbox.rs`**:

- Stays as Unsupported (Plan M2-01 already corrected this).

**1:1 fidelity items to lock in** (exact strings from claude-code source):

- WSL1 refusal: `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`
- Unsupported platform: `"sandbox.enabled is set but ${platform} is not supported (requires macOS, Linux, or WSL2)"`
- Disabled platforms list: `"sandbox.enabled is set but ${platform} is not in sandbox.enabledPlatforms"`
- Missing deps: `"sandbox.enabled is set but dependencies are missing: ${deps.join(', ')} · ${platform_hint}"`
  where platform_hint is `"run /sandbox or /doctor for details"` (macOS) or
  `"install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details"` (Linux/WSL).
- bubblewrap glob warning: `getLinuxGlobPatternWarnings()` scans `permissions.allow/deny`
  for `Edit/Read` rules whose path contains `* ? [ ]` (excluding trailing `/**`)
  and surfaces them.
- SBPL profile must be a valid macOS sandbox profile (S-expression DSL). For M2,
  ship a minimal template covering `filesystem.{allowWrite,denyWrite,denyRead,allowRead}` + `network` rules. Full template fidelity (covering all macOS quirks) is a separate research task.
- Compound command splitting (`&&`/`;`) and iterative env-var + safe-wrapper stripping
  must be fixed-point.

**Tests** (`crates/sandbox/tests/`):
- `runtime_config_test.rs` — JSON roundtrip for full config; verify field names byte-for-byte.
- `policy_convert_test.rs` — convert sample `SettingsJson` (multiple
  `permissions.allow/deny` entries) to `SandboxRuntimeConfig`, verify
  resulting `filesystem.*` arrays.
- `dependency_check_test.rs` — mock subprocess: simulate `bwrap`/`socat`
  presence/absence, verify error messages.
- `wsl_detection_test.rs` — mock `/proc/version` reads for WSL1/WSL2/native.
- `should_use_sandbox_test.rs` — compound commands, env-var stripping fixed-point.
- `wrap_linux_test.rs` — verify `bwrap` args derived correctly from a sample policy.
- `wrap_macos_test.rs` — verify SBPL profile syntax (basic regex on generated string).

**Dependencies**: M2-01.

**Estimated complexity**: large. ~1200 lines new Rust. Macos SBPL generation
is trickiest part — claude-code uses a template; we port the template.

**Commits**: 4 commits — `feat(sandbox): RuntimeConfig schema`,
`feat(sandbox): dependency check + violation store`,
`feat(sandbox): wrap_with_sandbox dispatch`, `feat(platforms/posix): real Sandbox impl`.

---

### 6.5 Plan M2-05: FS watch (notify) + Swarm backends

**Goal**: cross-platform native file watching (FSEvents/inotify/RDC handled
by `notify`); three swarm backends (Tmux, iTerm, InProcess).

**Modifications to `platforms/posix/src/fs.rs` and `platforms/windows/src/fs.rs`**:

- Replace platform-specific `watch` implementations with a common helper:
  - Use `notify` crate's `RecommendedWatcher` (picks `FSEventWatcher` /
    `INotifyWatcher` / `ReadDirectoryChangesWatcher` automatically).
  - Wrap with `notify-debouncer-mini` to implement `awaitWriteFinish`
    semantics: stabilityThreshold (default 500ms) + pollInterval (default
    200ms). Match chokidar 4's defaults.
  - Apply ignore filter: skip paths containing `/.git/` segment. Skip
    directories that match common transient patterns (`.swp`, `~$` editor
    files).
  - Convert `notify::Event` → `FileEvent { path, kind }` where kind
    discriminates Created / Modified / Deleted.

**New code in `platforms/posix/src/swarm/`** (refactor `swarm.rs` → directory):

- `mod.rs` — exports `make_backend(detect_env)` -> `Box<dyn SwarmBackend>`.
- `detection.rs` (~80 lines): probe terminal environment:
  - `which tmux` exists → tmux available.
  - `$TERM_PROGRAM == "iTerm.app"` → iTerm available.
  - else InProcess fallback.
- `tmux.rs` (~400 lines): real tmux backend. Methods:
  - `start_swarm(layout: SwarmLayout)` — `tmux -L <socket> new-session -d -s claude-swarm`
    if outside-tmux mode; or `tmux split-window` if inside-tmux. Socket name
    via `getSwarmSocketName()` (claude-code uses per-pid socket).
  - `create_teammate_pane(agent_id, position)` — `tmux split-window -h/-v -t <target>`,
    then `tmux send-keys -t <pane> "<spawn-claude-cmd>" Enter`, then `tmux
    select-pane -t <pane> -P bg=default,fg=<color>`, plus
    `tmux set-option -p -t <pane> pane-border-style "fg=<color>"`
    and `pane-border-format "<title>"`. Apply `PANE_SHELL_INIT_DELAY_MS = 200ms` sleep.
  - Color mapping (literal from claude-code):
    `red`→`red`, `blue`→`blue`, `green`→`green`, `yellow`→`yellow`,
    `cyan`→`cyan`, `purple`→`magenta`, `orange`→`colour208`,
    `pink`→`colour205`.
  - `destroy_swarm(handle)` — `tmux kill-session -t <name>` (or break-pane
    for inside-tmux mode).
  - `is_available()` — `which tmux` returns 0 AND tmux version ≥ 3.2.
  - `is_running_inside()` — `$TMUX` env var set.
- `iterm.rs` (~200 lines): iTerm backend using `osascript`. Methods build
  AppleScript snippets like `tell application "iTerm" to create window with default profile command ...`.
- `inprocess.rs` (~50 lines): no-pane fallback. `is_available()` returns
  `true`. Methods log "swarm running in-process; no pane visualization".
- `registry.rs` (~80 lines): `SwarmRegistry` picks backend at construction
  time via `detection`. Public API: `current_backend() -> &dyn SwarmBackend`.

**Modifications to `platforms/windows/src/swarm.rs`**:

- Stays as Unsupported (Plan M2-01 already corrected this). claude-code
  refuses tmux on Windows; we match.

**1:1 fidelity items to lock in**:

- `awaitWriteFinish` defaults: stabilityThreshold 500ms, pollInterval 200ms.
- `.git` directory always excluded from watches.
- `--tmux is not supported on Windows` error string.
- Pane creation lock — global mutex preventing concurrent splits (claude-code
  uses `paneCreationLock`).
- 200ms shell-init delay after pane creation.
- Color mapping literals (above).
- Requires tmux 3.2+ (`pane-border-style -p` is per-pane only in 3.2+).
- iTerm: must use AppleScript via `osascript`, not iTerm's Python API
  (claude-code path).

**Tests** (`platforms/posix/tests/`):
- `fs_watch_debounce_test.rs` — write file rapidly, verify single
  Modified event after stabilityThreshold elapses.
- `fs_watch_git_filter_test.rs` — create `.git/foo`, verify no event emitted.
- `swarm_detection_test.rs` — mock `$TMUX`/`$TERM_PROGRAM`, verify
  backend selection.
- (Real tmux tests are integration tests — `#[ignore]` by default, gated
  on `TMUX_AVAILABLE` env var for CI matrix.)

**Dependencies**: M2-01.

**Estimated complexity**: medium. ~800 lines new code. The tmux backend is
shell-out heavy but mostly mechanical.

**Commits**: 3 commits — `feat(platforms): notify-based FS watch with debounce`,
`feat(platforms/posix): tmux + iTerm + InProcess swarm backends`,
`refactor(platforms/windows): swarm explicitly Unsupported`.

---

### 6.6 Plan M2-06: SecureStorage macOS + HTTP SSE + Process spawn polish

**Goal**: three medium-sized features bundled together because each is
~150-300 lines and they don't share infra.

**SecureStorage — macOS Keychain via `security` CLI**:

Modify `platforms/posix/src/secure_storage.rs`:

- Add `MacOsKeychainStorage` struct alongside `PlainTextSecureStorage`.
- Constructor takes `user: String` and `config_dir: PathBuf`.
- `store(service, account, data)`:
  - Service name format: `format!("Claude Code{oauth_suffix}-credentials{dir_hash}", oauth_suffix = ..., dir_hash = sha256(config_dir).hex()[..8] if non-default else "")`.
  - Spawn `security add-generic-password -U -a <user> -s <full_service_name>` with `-i` flag (stdin mode) to write data without exposing it in argv.
  - Stdin payload: JSON-serialized `SecureStorageData`, written through pipe.
  - Fallback when payload > `SECURITY_STDIN_LINE_LIMIT = 4096 - 64`: hex-encode and use `-X <hex>` flag.
- `retrieve(service, account)`:
  - 30-second TTL cache (`KEYCHAIN_CACHE_TTL_MS = 30_000`).
  - Generation counter prevents stale subprocess writes from overwriting fresh updates.
  - In-flight dedupe: concurrent `retrieve` calls share a single `Future`.
  - Spawn `security find-generic-password -a <user> -w -s <full_service_name>`, parse stdout, deserialize.
- `delete(service, account)`: `security delete-generic-password -a <user> -s <full_service_name>`.
- `list(service)`: not directly supported by `security` CLI for prefix queries — defer to plaintext fallback.
- `is_encrypted()` returns `true`.
- `backend()` returns `SecureStorageBackend::MacOsKeychain`.

Modify `platforms/posix/src/lib.rs` to expose both `MacOsKeychainStorage` and `PlainTextSecureStorage`, plus a `SecureStorage` factory:

```rust
pub async fn secure_storage_for_platform() -> Result<Box<dyn SecureStorage>, SecureStorageError> {
    #[cfg(target_os = "macos")]
    {
        // Try keychain first; fall back to plaintext on init error
        match MacOsKeychainStorage::new(user, config_dir).await {
            Ok(s) => Ok(Box::new(s)),
            Err(e) => {
                tracing::warn!("Warning: Storing credentials in plaintext.");
                Ok(Box::new(PlainTextSecureStorage::new(plaintext_path()).await?))
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux: TODO add libsecret support; currently plaintext only (claude-code comment)
        Ok(Box::new(PlainTextSecureStorage::new(plaintext_path()).await?))
    }
}
```

**Modify `crates/secret/src/keychain_prefetch.rs`**: real implementation.
At startup, asynchronously call `MacOsKeychainStorage::retrieve(service, account)` for the Anthropic credentials entry, populating the 30s cache before the user's first request. Claude-code does this to overlap with the ~65ms module-init.

**HTTP SSE streaming** — modify `platforms/posix/src/http.rs` and `platforms/windows/src/http.rs`:

- `stream_sse(req: HttpRequest) -> Result<SseStream, HttpError>`:
  - Build `reqwest::RequestBuilder` same as `request`.
  - Call `.send().await?.bytes_stream()` to get a `Stream<Item = Result<Bytes, reqwest::Error>>`.
  - Wrap with a transform that accumulates bytes until `\n\n` (event boundary), emits parsed events.
  - Reuse `lingxi_api_client::sse::parse_sse_chunks` for the actual parsing — already implemented in M1.

**Modify `crates/api-client/src/types.rs::StreamEvent`** — expand to cover all claude-code event types:

- `ContentBlockDelta` already has `Text` and `InputJsonDelta` variants — add `Thinking { thinking: String }`, `SignatureDelta { signature: String }`, `CitationsDelta { citation: Value }`, `ConnectorTextDelta { text: String }`.
- `ContentBlock` add variants: `Thinking { thinking: String, signature: Option<String> }`, `ServerToolUse { ... }`, `ConnectorText { ... }`, `AdvisorToolResult { ... }`.
- Make all of these `#[serde(default)]` to handle absence gracefully.

**Process spawn — polish on `platforms/posix/src/process.rs` and `platforms/windows/src/process.rs`**:

- Add `kill_tree(pid: u32)`:
  - Unix: `nix::sys::signal::killpg(unistd::Pid::from_raw(-(pid as i32)), Signal::SIGTERM)`. Wait 5s. If still alive, SIGKILL.
  - Windows: spawn `taskkill /T /F /PID <pid>`.
- Implement `spawn_background` for real:
  - Spawn child with `tokio::process::Command`.
  - Detached: `pre_exec` calls `setsid()` (Unix).
  - Return `ProcessHandle { pid, task_id }` with the PID for later `kill_tree`.
  - Background task pipes stdout/stderr to a file at `getTaskOutputPath(task_id)` so the assistant can `tailFile()` it.
- Implement `kill(handle)` using `kill_tree(handle.pid)`.
- cwd tracking: when wrapping a bash command for the bash tool, append `&& pwd -P >| $cwdfile` to the command string. After execution, read the file synchronously (claude-code uses `readFileSync` to avoid microtask races). NFC-normalize before comparing.
- Shell snapshot loading: simplified version. Source a snapshot file before the user command if one exists; otherwise run with the user's default shell init. Full snapshot machinery (claude-code's `ShellSnapshot.ts`) is M2.next.
- Extended glob disable per shell:
  - bash: prepend `shopt -u extglob 2>/dev/null || true;` to command.
  - zsh: prepend `setopt NO_EXTENDED_GLOB 2>/dev/null || true;` to command.
- Spawn env additions: `CLAUDECODE=1`, `GIT_EDITOR=true`, `SHELL=<bin>`, `CLAUDE_CODE_SESSION_ID=<session-id>` when present in caller context.
- Spawn options: `detached: true`, `windowsHide: true`, file-mode stdio uses `O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW` on Unix and `'w'` on Windows (MSYS2 FILE_WRITE_DATA quirk per claude-code comment).
- 30-minute default timeout: `DEFAULT_TIMEOUT = Duration::from_secs(30 * 60)`.

**1:1 fidelity items to lock in** (each subsystem):

- **Keychain**: service name format with `dir_hash` derivation, 30s TTL,
  generation counter, in-flight dedupe, `-i` stdin path, hex-encode argv fallback
  threshold. Plaintext fallback warning string: `"Warning: Storing credentials in plaintext."`.
- **SSE**: event names from Messages API contract literally.
- **Process**: env vars `CLAUDECODE=1`, `GIT_EDITOR=true`; 30-minute timeout;
  `pwd -P` cwd tracking; extglob disable; detached + windowsHide; tree-kill
  on the full process group.

**Tests**:
- `secure_storage_macos_test.rs` (gated `#[cfg(target_os = "macos")]`): write,
  read, delete, verify cache TTL.
- `http_stream_sse_test.rs`: mock SSE server, verify event boundary parsing.
- `process_kill_tree_test.rs`: spawn shell that forks two children, kill the
  parent, verify all descendants gone.
- `process_cwd_tracking_test.rs`: spawn `cd /tmp; ls`, verify reported cwd is
  `/tmp`.

**Dependencies**: M2-01.

**Estimated complexity**: medium-large. SecureStorage macOS ~300 lines, SSE
~150 lines, Process polish ~400 lines. Total ~800 lines.

**Commits**: 3 commits — `feat(secure_storage): macOS Keychain via security CLI`,
`feat(http): real SSE streaming + claude-code event type parity`,
`feat(process): tree-kill + cwd tracking + spawn_background`.

---

### 6.7 Plan M2-07: Test infra + docs + tag v0.3.0

**Goal**: lock in M2's behavior with tests, update docs, ship v0.3.0.

**Test infrastructure** (in `crates/test-harness/`):

- `src/contracts/` — add suites for the 12 traits still missing from M1's
  filesystem contract. Each is small (3-7 contract methods), ~50-100 lines.
  Required traits:
  - `clock_contract`
  - `runtime_spawner_contract`
  - `http_transport_contract`
  - `process_runner_contract`
  - `mcp_transport_contract`
  - `lsp_transport_contract`
  - `sandbox_contract`
  - `worktree_manager_contract`
  - `secure_storage_contract`
  - `swarm_backend_contract`
  - `bridge_transport_contract`
  - `effect_handler_contract`
- `tests/contract_*.rs` — drivers for each, running platform impls (posix and
  windows where applicable) through the contract suite.
- `src/parity/` — fixtures for 6 high-value claude-code behavior parity tests:
  - `sandbox_config_conversion.json` — SettingsJson → SandboxRuntimeConfig roundtrip.
  - `mcp_initialize_request.json` — verify literal `name: "claude-code"`, `capabilities: {roots:{}, elicitation:{}}`.
  - `lsp_plugin_only.json` — verify that `register_config` is not callable from
    user/project settings paths.
  - `worktree_branch_naming.json` — verify branch is `claude/<slug>`,
    path is `<root>/.claude/worktrees/<flattened-slug>`.
  - `secure_storage_macos_service_name.json` — verify service name format.
  - `tmux_windows_refusal.json` — verify Windows returns Unsupported with
    matching error string.

**Documentation updates**:

- `CHANGELOG.md` — new `## [0.3.0]` section listing all M2-01..M2-07 changes
  organized by subsystem.
- `docs/ARCHITECTURE.md` — update the platform crate descriptions to reflect
  real implementations. Add a section "claude-code parity guarantees" listing
  the wire identifiers, error strings, and file paths that are locked.
- `docs/PLATFORMS.md` (new) — per-OS support matrix. Mirror claude-code's:
  - macOS: full support (FS watch via FSEvents, sandbox via sandbox-exec, swarm via tmux/iTerm).
  - Linux: full support (FS watch via inotify, sandbox via bwrap+socat, swarm via tmux).
  - WSL2: same as Linux.
  - Windows: limited support (sandbox unsupported, swarm unsupported, FS watch via RDC, keychain via plaintext fallback only).
  - WSL1: refused.
  - Android/iOS: not in scope (M3).
- `README.md` — update platform support callout and quickstart.

**Release verification**:

- `cargo test --workspace` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.
- `cargo check --no-default-features` clean.
- Cross-compile matrix in CI (existing `.github/workflows/ci.yml`) passes for
  5 targets: `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`, `aarch64-linux-android`, `aarch64-apple-ios`.
- All contract tests pass for posix-minimal, posix, windows where applicable.
- All 6 parity fixtures verified.

**Tag**: from repo root, `git tag -a v0.3.0 -m "M2 v0.3.0 — claude-code
behavioral parity"`.

**Dependencies**: M2-01..M2-06 complete.

**Estimated complexity**: medium. Contract suites are repetitive templates;
parity fixtures are new but bounded.

**Commits**: 4 commits — `test(contracts): 12 trait contract suites`,
`test(parity): 6 claude-code behavior fixtures`,
`docs: CHANGELOG + ARCHITECTURE + PLATFORMS for v0.3.0`,
`release: v0.3.0 tag`.

---

## 7. Cross-plan concerns

### 7.1 Version pinning

The workspace toolchain is pinned at Rust 1.82.0 (workspace `rust-toolchain.toml`).
M1 and v0.2.0 encountered several deps that pull edition2024 transitively
and break the build. Apply the same `cargo update --precise` pattern when
hit:

- `uuid` pinned to 1.10 (M1)
- `proptest` pinned to 1.5 (M1)
- `tempfile` pinned to 3.13 (M1)
- `idna_adapter` pinned to 1.1.0 (M1)
- `hyper-rustls` pinned to 0.27.5 (v0.2.0)
- `indexmap` pinned to 2.7.1 (M1)
- `clap` pinned to `=4.5.20` (M1)

M2 new deps to watch:
- `notify` 6.x — verify Rust 1.82 compatibility.
- `notify-debouncer-mini` — same.
- `lsp-types` 0.95 — usually fine.
- `tokio-tungstenite` 0.21 — verify.
- `nix` (for `killpg` on Unix) — verify 0.27 works.
- `rmcp` — not used (we own MCP); listed only for reference.

If a pin needs to slip post-Rust-1.85 adoption, document in M2-07 release notes.

### 7.2 Capability flag wiring (M3 prep)

Spec §35 says "Mobile gap closure principle: All G1-G12 fixes are confined to
the M3 platform crates plus capability-flag wiring in the existing trait
surface. Zero engine-crate changes are expected."

M2 must keep this invariant. Each platform-specific limitation (Windows sandbox
unsupported, etc.) must be communicated to the engine via the `PlatformCapabilities`
struct rather than special-cased in engine code. The engine consults capabilities
when deciding whether to advertise tools like `Bash` (which depends on `process`
+ `sandbox`), `LSPTool` (depends on plugin LSP availability), etc.

Per-plan capability flag impact:

- M2-01: corrects `BridgeTransport` advertisement (cloud bridge not implemented; local IDE bridge is the only path).
- M2-04: `PlatformCapabilities.sandbox = false` on Windows, WSL1.
- M2-05: `PlatformCapabilities.swarm = false` on Windows.

M2-07's parity fixtures verify these flags are set correctly per platform.

### 7.3 Test runner OS matrix

CI runs on `ubuntu-latest`. Some M2 tests require specific OS features:

- Sandbox real-impl tests: only Linux + macOS. Gate with `#[cfg(target_os = ...)]`.
- macOS Keychain tests: only macOS. Gate similarly.
- tmux backend integration tests: gate on `TMUX_AVAILABLE=1` env var (CI sets this when applicable).
- ReadDirectoryChangesW: only Windows. Gate.

CI workflow update (M2-07 covers this): add a macOS job and a Windows job to
the matrix, each running the relevant gated tests.

### 7.4 `posix-minimal` disposition

`platforms/posix-minimal/` remains the demo host that backs `examples/cli-demo`.
It is **not** upgraded in M2; cli-demo continues to drive posix-minimal as
the simplest path to "Rust binary that runs."

Production use should depend on `platforms/posix/` (real impls). Future work
might unify these, but for v0.3.0 they coexist.

---

## 8. Verification & release

### 8.1 Pre-release checklist (M2-07)

Run from `lingxi-core/`:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo check -p lingxi-platform-posix --no-default-features
cargo check -p lingxi-platform-windows --no-default-features
cargo test --workspace --features 10k-iterations  # if available
cargo test -p lingxi-test-harness --test 'contract_*'
cargo test -p lingxi-test-harness --test 'parity_*'
```

All must pass. Contract coverage gate (M1's deferred plan): ratio of
unexercised trait methods ≤ 5%. M2-07 implements the registry / checker.

### 8.2 Tag

From repo root (not lingxi-core):
```bash
git tag -a v0.3.0 -m "M2 v0.3.0 — claude-code behavioral parity"
git push origin v0.3.0  # only if pushing
```

### 8.3 What v0.3.0 unlocks

After v0.3.0:
- `examples/cli-demo` (still on posix-minimal) can be upgraded to `posix` and
  drive a real Anthropic conversation end-to-end with SSE streaming.
- Real MCP servers (e.g., `npm install -g @modelcontextprotocol/server-filesystem`)
  can be configured via settings.json and used as tools.
- Real LSP servers (rust-analyzer, gopls, etc.) can be loaded via plugins
  and consulted by `LSPTool`.
- macOS Keychain stores credentials securely.
- tmux swarm splits work on Linux + macOS.

### 8.4 What still doesn't work after v0.3.0

- Cloud Remote Control bridge (claude.ai worker integration) — deferred to a
  separate milestone, not M3.
- Android / iOS mobile platforms — M3.
- Plugin marketplace / mcpb bundle install — covered by `lingxi-plugin` crate
  but no marketplace UI; that's M4+.
- Web pty-server — separate UI project (M4).

---

## 9. Self-review

(Spec author: complete this section after writing the spec, fix issues inline.)

- **Placeholder scan**: no "TBD", "TODO", "implement later" outside of the
  intentional `TODO(M2-followup)` markers in source code; the spec itself
  has no placeholders. ✓
- **Internal consistency**: cross-checked plan dependencies (M2-02 → M2-03,
  M2-01 → all others). ✓
- **Scope check**: 7 plans is appropriate decomposition; each is bounded
  to a single subsystem cluster. ✓
- **Ambiguity check**: all "1:1 fidelity items" are explicit strings or
  numbers, not paraphrased. ✓

---

## 10. Out-of-spec items deliberately not addressed

These came up during brainstorming and were explicitly excluded:

- **Cloud Remote Control bridge** (claude-code `src/bridge/` 14k lines) —
  deferred (see §5).
- **Native Linux SecureStorage (libsecret)** — claude-code uses plaintext
  fallback on Linux with a TODO; we match.
- **Windows native FS watcher (ReadDirectoryChangesW)** — `notify` handles
  this internally; we don't need to call the API ourselves.
- **node-pty web pty-server** — separate UI feature (M4).
- **Sandbox real OS-level isolation written from scratch** — claude-code
  delegates to `bwrap+socat` / `sandbox-exec`; we delegate too. Net-new
  Rust namespace code is not in scope.
- **Java MCP SDK quirks beyond capabilities** — we lock the capabilities
  shape but don't otherwise diverge to accommodate non-spec-compliant
  servers.

---

## Appendix A: file-touch inventory

(Rough estimate per plan; helps reviewers locate work.)

### M2-01 (corrections)
- `platforms/windows/src/sandbox.rs` (rewrite)
- `platforms/windows/src/swarm.rs` (rewrite)
- `crates/bridge/src/{codes.rs,jwt.rs,rate_limiter.rs,pairing.rs}` (delete)
- `crates/bridge/src/{message.rs,state.rs,transport.rs,lib.rs}` (rewrite)
- `crates/bridge/src/lockfile.rs` (new)
- `platforms/posix/src/worktree.rs` (modify)
- `platforms/windows/src/worktree.rs` (modify)
- `crates/bridge/Cargo.toml` (modify)

### M2-02 (jsonrpc + MCP + bridge)
- `crates/jsonrpc/` (new crate, ~600 lines)
- `crates/mcp/src/{client.rs,identity.rs,initialize_params.rs}` (new)
- `crates/mcp/src/{lib.rs,connection.rs,registry.rs}` (modify)
- `platforms/posix/src/mcp.rs` (rewrite — add stdio + WebSocket transports)
- `platforms/windows/src/mcp.rs` (rewrite — same)
- `crates/bridge/src/lockfile.rs` (new, ~80 lines)
- `crates/bridge/src/transport.rs` (rewrite — lockfile + MCP-over-WS wiring)
- `crates/bridge/Cargo.toml` (modify — add `lingxi-mcp` dep)

### M2-03 (LSP)
- `crates/lsp/src/{client.rs,passive_feedback.rs,diagnostic_registry.rs,tool_operations.rs}` (new)
- `crates/lsp/src/{lib.rs,registry.rs,connection.rs}` (modify)
- `crates/plugin/src/manager.rs` (modify — doc + verify)
- `platforms/posix/src/lsp.rs` (rewrite)
- `platforms/windows/src/lsp.rs` (rewrite)
- `crates/lsp/Cargo.toml` (modify — add `lsp-types`, `lingxi-jsonrpc`)

### M2-04 (sandbox)
- `crates/sandbox/src/{runtime_config.rs,policy_convert.rs,dependency_check.rs,violation_store.rs,wrap.rs,should_use_sandbox.rs}` (new)
- `crates/sandbox/src/{lib.rs,decision.rs,policy.rs}` (modify)
- `platforms/posix/src/sandbox.rs` (rewrite)
- `platforms/windows/src/sandbox.rs` (no change — M2-01 already done)
- `crates/sandbox/Cargo.toml` (modify)

### M2-05 (watch + swarm)
- `platforms/posix/src/swarm/` (new directory replacing `swarm.rs`)
- `platforms/posix/src/swarm/{mod.rs,detection.rs,tmux.rs,iterm.rs,inprocess.rs,registry.rs}` (new)
- `platforms/posix/src/fs.rs` (modify — replace watch)
- `platforms/windows/src/fs.rs` (modify — replace watch)
- Cargo.toml updates for `notify` + `notify-debouncer-mini`.

### M2-06 (secure storage + SSE + process)
- `platforms/posix/src/secure_storage.rs` (modify — add MacOsKeychainStorage)
- `platforms/windows/src/secure_storage.rs` (no change)
- `platforms/posix/src/http.rs` (modify — real stream_sse)
- `platforms/windows/src/http.rs` (modify — real stream_sse)
- `platforms/posix/src/process.rs` (modify — kill_tree, cwd, spawn_background, etc.)
- `platforms/windows/src/process.rs` (modify — same)
- `crates/api-client/src/types.rs` (modify — StreamEvent variants)
- `crates/secret/src/keychain_prefetch.rs` (modify — real impl)

### M2-07 (test + docs + release)
- `crates/test-harness/src/contracts/` (new files per trait)
- `crates/test-harness/tests/contract_*.rs` (new drivers)
- `crates/test-harness/src/parity/` (new fixtures)
- `crates/test-harness/tests/parity_*.rs` (new drivers)
- `CHANGELOG.md` (modify)
- `docs/ARCHITECTURE.md` (modify)
- `docs/PLATFORMS.md` (new)
- `README.md` (modify)

Total file count: ~90 files touched, ~25 net-new files.

---

## Appendix B: glossary

- **1:1 parity** — observable behavior (settings, errors, paths, identifiers) matches claude-code. Implementation details do not.
- **claude-code** — the 2026-03-31 leaked TypeScript source at `/Users/luolingfeng/Projects/LingXi-Next/claude-code/`. Reference implementation.
- **CLI** — `claude` (TypeScript) and `lingxi-demo` (our Rust port).
- **Lockfile** — `~/.claude/ide/<port>.lock`, JSON file written by VS Code/JetBrains plugins announcing the IDE's WebSocket port + auth token.
- **MCP** — Model Context Protocol. Tool / resource / prompt protocol over stdio/HTTP/SSE/WS/InProcess.
- **Plan** — one of M2-01..M2-07; a unit of work with its own commit(s) and implementation plan doc.
- **v0.2.0** — current release tag (M2 pragmatic scaffolds).
- **v0.3.0** — target release tag (M2 behavioral parity, end of M2-07).

---

## End of spec

Next steps:
1. Author commits this spec to git.
2. User reviews.
3. On approval, invoke `superpowers:writing-plans` skill to generate 7
   implementation plans (one per M2-NN), each in
   `docs/superpowers/plans/2026-MM-DD-m2-NN-<topic>.md`.
4. Execute plans via `superpowers:subagent-driven-development` (same pattern
   as M1).
