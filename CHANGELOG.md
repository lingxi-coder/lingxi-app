# Changelog

## [0.3.0] — M2 claude-code Behavioral Parity

### Crates added
- `lingxi-jsonrpc` — JSON-RPC 2.0 framing shared by MCP and LSP. Supports
  Content-Length-prefixed (LSP / modern MCP) and line-delimited (older MCP)
  framing with auto-detect, outbound request router with timeout + drop-cancel,
  inbound request router (for `roots/list`, `elicitation/create`), and a
  notification broker.

### Crates expanded
- `lingxi-sandbox` — full `SandboxRuntimeConfig` schema (matches claude-code
  `entrypoints/sandboxTypes.ts` field-for-field), `convert_settings_to_runtime_config`,
  `dependency_check`, `violation_store`, and `wrap_with_sandbox` dispatcher
  (macOS `sandbox-exec` SBPL profile, Linux `bwrap+socat`, Windows/WSL1 Unsupported).
- `lingxi-mcp` — real client over `lingxi-jsonrpc` covering `initialize`, `list_tools`,
  `call_tool` (with timeout error string `"MCP server \"...\" tool \"...\" timed out
  after Ns"`), `list_resources`, `list_prompts`, `read_resource`, `ping`, plus
  inbound `roots/list` and `elicitation/create` handlers. Identity locked:
  `name="claude-code"`, `title="Claude Code"`, capabilities `{"roots":{}, "elicitation":{}}`.
- `lingxi-lsp` — real client over `lingxi-jsonrpc` with 9 tool operations
  (`goToDefinition`, `findReferences`, `hover`, `documentSymbol`, `workspaceSymbol`,
  `goToImplementation`, `prepareCallHierarchy`, `incomingCalls`, `outgoingCalls`),
  1-based ↔ 0-based line/character translation, `textDocument/didOpen` registry,
  `textDocument/publishDiagnostics` accumulation, 10 MB `MAX_LSP_FILE_SIZE_BYTES`
  cap, and plugin-only `register_config` (visibility narrowed to `pub(crate)`).
- `lingxi-bridge` — lockfile-based local IDE bridge. Reads `~/.claude/ide/<port>.lock`,
  builds an MCP-over-WebSocket transport spec with header
  `X-Claude-Code-Ide-Authorization`. The 8-char pairing protocol and
  project-scoped JWT machinery from v0.2.0 were removed (they had no claude-code
  counterpart). Cloud Remote Control bridge remains out of scope.
- `lingxi-platform-posix` — real impls land for `Sandbox` (macOS + Linux + WSL2),
  `McpTransport` (stdio + sse + http + ws), `LspTransport`, `SwarmBackend`
  (tmux + iTerm + InProcess fallback), `FileSystem::watch` (notify + debounce),
  `HttpTransport::stream_sse`, `ProcessRunner::spawn_background` + `kill_tree`
  + `pwd -P` cwd tracking, `SecureStorage` (macOS Keychain via `security` CLI
  + plaintext fallback).
- `lingxi-platform-windows` — `Sandbox` and `SwarmBackend` explicitly return
  `Unsupported` (claude-code does not support sandbox or tmux on Windows).
  `FileSystem::watch` switches to `notify`'s `ReadDirectoryChangesW` path.
  `SecureStorage` remains plaintext (Windows Credential Vault deferred).

### 1:1 parity guarantees locked
- Worktree branch prefix: `worktree-` (was `lingxi/` in v0.2.0). Slug flattening
  `/` → `+`. Path: `<repo_root>/.claude/worktrees/<flattened-slug>`.
- MCP client identity: `name="claude-code"`, `title="Claude Code"`,
  `websiteUrl="https://claude.com/claude-code"`, capabilities
  `{"roots":{}, "elicitation":{}}` (empty objects, not null).
- IDE WebSocket auth header: `X-Claude-Code-Ide-Authorization` (literal).
- macOS Keychain service name format: `Claude Code{oauth_suffix}-credentials{dir_hash}`.
- LSP file size cap: `MAX_LSP_FILE_SIZE_BYTES = 10_000_000` (10 MB).
- Sandbox WSL1 refusal: `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`.
- Sandbox unsupported-platform: `"sandbox.enabled is set but ${platform} is not supported (requires macOS, Linux, or WSL2)"`.
- Windows tmux refusal: `"--tmux is not supported on Windows"`.
- LSP `register_config` is `pub(crate)` — only plugins can register LSP servers.
- MCP tool name format: `mcp__<server>__<tool>`.

### Known deferrals carried forward to M3+
- Cloud Remote Control bridge (claude.ai worker integration, ~14k TS lines).
- In-process MCP transports (computer-use, Chrome) — depend on a separate
  computer-use server crate.
- Linux SecureStorage native backend (libsecret) — plaintext fallback only,
  matching claude-code's TODO.
- Android / iOS platform crates — M3 milestone.
- Plugin marketplace UI and `.mcpb` bundle installer — M4 UI Layer.
- Web pty-server — M4 UI Layer.
- macOS SBPL profile fidelity beyond the M2 template — separate research task.

### Migration from v0.2.0

The following surfaces changed in source-incompatible ways. Downstream users
of `lingxi-core` as a library MUST update accordingly:

- **Worktree branch prefix:** existing v0.2.0 worktrees with `lingxi/<slug>`
  branches are not recognized by v0.3.0 cleanup. Run `git worktree remove`
  manually for any orphan v0.2.0 worktree before upgrading.
- **Worktree path:** `WorktreeManager::create_worktree` parameter renamed from
  `worktree_base` to `repo_root`. The path is now hardcoded to
  `<repo_root>/.claude/worktrees/<flattened-slug>`.
- **`lingxi-bridge` API:** the 9-variant `BridgeMessage` enum, `BridgeCode`,
  `JwtVerifier`, `RateLimiter`, and `PairingManager` types were removed.
  `IdeBridge` now exposes only `connect()` + `disconnect()`; transport details
  live in `lingxi-mcp`.
- **`crates/bridge` dependencies:** `rand`, `sha2`, `hmac`, `base64` removed
  from `Cargo.toml`. Add `lingxi-mcp` dependency.
- **`lingxi-lsp::LspRegistry::register_config`** is now `pub(crate)`. External
  callers must register LSP servers through `crates/plugin`'s
  `register_plugin_servers` path.
- **`ProcessRunner::spawn_background`** previously returned `Unsupported` on all
  platforms; now returns a real `ProcessHandle` on posix/windows.
- **`api-client::types::StreamEvent`** gained new variants (`Thinking`,
  `SignatureDelta`, `CitationsDelta`, `ConnectorTextDelta`, etc.). Existing
  match arms over `StreamEvent` will hit `non_exhaustive` warnings — add a
  catch-all or update arms.

### Tests + verification
- Workspace test count: ~145 (v0.2.0 baseline 104 + 12 contract suites + 7
  parity fixtures + per-plan tests from M2-01..M2-06).
- `cargo test --workspace` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.
- `cargo check -p lingxi-platform-posix --no-default-features` clean.
- `cargo check -p lingxi-platform-windows --no-default-features` clean.
- Desktop cross-compile matrix (`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`) green. Android/iOS targets are informational only
  for v0.3.0 (M3 scope).

## [0.2.0] — M2 Production Desktop Platforms

### Crates shipped
- `lingxi-platform-posix` (Linux + macOS) — real impls for Clock, FileSystem (with inotify watch on Linux), HTTP (reqwest), Process (tokio::process), Runtime (tokio::spawn), SecureStorage (plain-text file fallback), Worktree (git CLI). Sandbox, MCP stdio, LSP, Swarm, Bridge ship as type-correct stubs.
- `lingxi-platform-windows` — mirrors posix with Windows-friendly fallbacks. Same stubbed surfaces; uses fs2 cross-platform locking instead of LockFileEx directly; symlink uses `tokio::fs::symlink_file` under cfg(windows).

### Platform support
- Linux/macOS/Windows: production-ready for most engine workloads; demo cli-demo still wires posix-minimal.
- Android/iOS: unchanged from M1 — cross-compile only, M3 work.

### Known deferred (M2-followup TODOs in code)
- Real OS sandbox isolation (Linux user namespaces, macOS sandbox-exec, Windows Job Objects).
- Full JSON-RPC framing for MCP stdio and LSP (request id tracking, Content-Length headers, notification streaming).
- FSEvents (macOS) and ReadDirectoryChangesW (Windows) for filesystem watch.
- tmux/Windows Terminal CLI driver for SwarmBackend.
- WebSocket BridgeTransport.
- Native SecureStorage backends (libsecret, macOS Keychain, Windows Credential Vault) — current PlainTextFile fallback is functional but not encrypted.
- `http.stream_sse` — non-trivial but needed for live API streaming; currently returns InvalidRequest.

## [0.1.0] — M1 Foundation Release

### Crates shipped
- protocol, core, traits, api-client (Plan 01)
- permission, secret, cost (Plan 02)
- tools, hooks (Plan 03)
- memory, mcp (Plan 04)
- compaction (Plan 05)
- agent (Plan 06)
- tasks, coordinator (Plan 07)
- sidequery (Plan 08)
- skills, commands, outputstyles (Plan 09)
- session, filestate, msgqueue (Plan 10)
- cron (Plan 11)
- sandbox, lsp (Plan 12)
- telemetry, anthropic-oauth (Plan 13)
- bridge (Plan 14)
- plugin (Plan 15)
- uniffi-bridge, platforms/posix-minimal, examples/cli-demo (Plan 16)
- test-harness (Plan 17)

### Platform support
- Linux/macOS/Windows: runnable via cli-demo + posix-minimal
- Android/iOS: cross-compile gate only; production platform crates land in M3

### Tests + verification (Plan 17)
- 104 tests pass under `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo fmt --all --check` clean
- Filesystem trait contract suite + posix-minimal driver
- `10k-iterations` Cargo feature flag plumbed on test-harness (CI wiring in M2)

### Deferred to M2
- Contract suites for the remaining 12 traits (process, http, mcp, worktree,
  swarm, secure_storage, sandbox, lsp, bridge, runtime, clock,
  hook_broadcaster) — pattern seeded in Plan 17, replication across traits
  is mechanical.
- Property tests at 10K iterations across all 11 property domains — feature
  flag is in place, individual suites read it during the M2 expansion.
- Parity fixture recordings (12 scenarios from claude-code reference) and
  per-scenario driver tests — namespace scaffold lands here.
- Contract coverage CLI + CI gate (`ratio ≤ 0.05`) — written into the
  M2 plan; trait method registry is small enough to maintain by hand
  until then.

### Tag
- `v0.1.0` — M1 v0.1.0, desktop-runnable, mobile cross-compile only.
