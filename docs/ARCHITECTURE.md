# Architecture

LingXi Core is an event-sourced conversation engine split across 30 crates.
This document is a navigation aid; full design lives in
`docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`.

## Crate map

- `protocol` — shared DTOs, IDs, Effect/Event envelopes
- `core` — state machine, reducer, prompt assembly, session model
- `traits` — 13 platform abstraction traits
- `api-client` — Anthropic/OpenAI-compatible API + SSE
- `permission/secret/cost` — security & cost foundations (Plan 02)
- `tools/hooks` — execution + extension (Plan 03)
- `memory/mcp` — retrieval + tool surface (Plan 04)
- `jsonrpc` — JSON-RPC 2.0 framing shared by MCP and LSP. Content-Length and
  line-delimited framing, outbound request router with timeout + drop-cancel,
  inbound request router, notification broker (added in M2).
- `compaction` — 5-layer compactor (Plan 05)
- `agent` — subagent runtime (Plan 06)
- `tasks/coordinator` — background work + multi-agent (Plan 07)
- `sidequery` — side LLM + forked agent infra (Plan 08)
- `skills/commands/outputstyles` — user-facing surface (Plan 09)
- `session/filestate/msgqueue` — persistence + caching (Plan 10)
- `cron` — scheduled tasks (Plan 11)
- `sandbox/lsp` — execution support (Plan 12; M2 expands sandbox to full
  `SandboxRuntimeConfig` + dispatcher, LSP to real client over `jsonrpc`)
- `telemetry/anthropic-oauth` — infra + main auth (Plan 13)
- `bridge` — lockfile-based local IDE bridge (MCP-over-WebSocket). M2 swap
  removed the pre-claude-code pairing/JWT machinery.
- `plugin` — manifest + 8-registry materialization (Plan 15)
- `uniffi-bridge` — FFI façade (Plan 16)
- `test-harness` — contracts + properties + parity (Plan 17)
- `platforms/posix-minimal` — M1 desktop demo host
- `platforms/posix` — M2 production host for Linux + macOS + WSL2. Ships real
  impls for `Sandbox`, `McpTransport` (stdio + sse + http + ws), `LspTransport`,
  `SwarmBackend` (tmux + iTerm + InProcess), `FileSystem::watch` (notify +
  debounce), `HttpTransport::stream_sse`, `ProcessRunner::spawn_background` +
  `kill_tree`, `SecureStorage` (macOS Keychain via `security` CLI, plaintext
  fallback on Linux).
- `platforms/windows` — M2 production host for Windows 10 22H2+. `Sandbox` and
  `SwarmBackend` return `Unsupported` (matches claude-code refusal logic);
  `FileSystem::watch` uses `notify`'s `ReadDirectoryChangesW`. `SecureStorage`
  remains plaintext (Credential Vault deferred).
- `examples/cli-demo` — M1 end-to-end demo

## Key flows

### Single-turn conversation
User input → `reduce(Idle, UserMessage)` → `Effect::SendApiRequest` →
HttpTransport → SSE stream → `Event::ApiStream*` → `Effect::RenderStreamDelta`
→ `Event::ApiStreamEnd` → `Idle`.

### Tool dispatch
Assistant tool_use block → `ToolUseReceived` → permission check (rules + classifier)
→ PreToolUse hook → tool.call() → PostToolUse hook → `Effect::ExecuteTool` result
→ tool_result message → next API turn.

### Compaction
Token estimate > threshold → orchestrator → micro/cached-micro/collapse →
autocompact via ForkedAgentRunner → PostCompactBuilder → boundary message in transcript.

### Subagent
AgentTool → SubagentContext built (Tools/MCP/Hooks/Memory/Permission inherited)
→ StateMachinePool::allocate → sibling slot runs its own reducer → SubagentEvent
to parent → tool_result.

## Cross-cutting concerns

- **No tokio::spawn outside runtime trait** — all background work via `RuntimeSpawner`.
- **No tokio::fs/std::fs in engine crates** — all I/O via `FileSystem` trait.
- **Secrets never logged** — `Secret<T>` debug is always `<redacted>`.
- **Sandbox is type-enforced** — `ProcessRunner::run` only accepts `SandboxedCommand`.

## Platform crates (M2)

- `platforms/posix-minimal/` — M1 demo host. Stubs for most traits; used by `examples/cli-demo`.
- `platforms/posix/` — M2 production host for Linux + macOS + WSL2. Real impls
  for all 13 traits including `Sandbox` (macOS `sandbox-exec`, Linux/WSL2
  `bwrap+socat`), `McpTransport` (stdio + sse + http + ws), `LspTransport`,
  `SwarmBackend` (tmux + iTerm + InProcess fallback), `FileSystem::watch`
  (FSEvents/inotify via `notify` + debounce), `HttpTransport::stream_sse`,
  `ProcessRunner::spawn_background` + `kill_tree`, `SecureStorage` (macOS
  Keychain via `security` CLI with 30s TTL cache + in-flight dedupe; plaintext
  fallback on Linux).
- `platforms/windows/` — M2 production host for Windows. Mirrors posix; `Sandbox`
  and `SwarmBackend` explicitly return `Unsupported` (matches claude-code's
  Windows refusal). `FileSystem::watch` uses `notify`'s `ReadDirectoryChangesW`.

Pre-M3 (mobile platforms) is documented in spec §35.

## claude-code parity guarantees (locked in v0.3.0)

The following identifiers, error strings, and file paths are part of the
behavioral contract with claude-code. Changing any of them is a breaking
change for managed-policy customers and for users restoring cross-version
state. They are covered by parity fixtures in
`crates/test-harness/src/parity/fixtures/`.

### Wire identifiers
| Identifier | Value | Rationale |
|---|---|---|
| MCP client name | `"claude-code"` | Servers identify allowed clients by this string; mismatched clients trigger reject-or-ignore logic in some MCP implementations. |
| MCP client title | `"Claude Code"` | Human-readable handshake field surfaced in MCP server logs. |
| MCP client websiteUrl | `"https://claude.com/claude-code"` | Stable identity / support link. |
| MCP capabilities.roots | `{}` (empty object) | Empty object signals "supported, no parameters." `null` or missing signals "not supported" — Java MCP servers reject other shapes. |
| MCP capabilities.elicitation | `{}` (empty object) | Same as above. |
| IDE WebSocket auth header | `X-Claude-Code-Ide-Authorization` | Matches the IDE extension; using `Authorization: Bearer` would not be recognized. |
| MCP tool full-name format | `mcp__<server>__<tool>` | Tool-name partitioning in the assistant's tool router relies on the double-underscore separator. |

### File paths
| Path | Rationale |
|---|---|
| `<repo_root>/.claude/worktrees/<flattened-slug>` | Cross-version worktree discovery. Slug flattening `/` → `+` so file names stay flat while branch names retain hierarchy. |
| `~/.claude/ide/<port>.lock` | IDE lockfile shape (workspaceFolders, pid, ideName, transport, runningInWindows, authToken). VS Code / JetBrains extensions write this; the bridge reads it. |
| Keychain service name | `Claude Code{oauth_suffix}-credentials{dir_hash}` where `dir_hash` is `sha256(config_dir).hex()[..8]` for non-default config dirs. Allows multiple installations to coexist. |

### Branch and slug rules
- Desktop / local worktree branch: `worktree-<flattened-slug>`. Not `lingxi/...`, not `claude/...`.
- Cloud / remote git outcome uses `claude/<branch>` — that's a separate code
  path (and out of v0.3.0 scope); do not confuse the two.
- Slug regex (per segment): `^[A-Za-z0-9_\-.]+$`. Maximum 64 chars total
  across all segments. `/` separates segments and is flattened to `+` on disk.

### Error strings (byte-for-byte)
- Sandbox WSL1: `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`.
- Sandbox unsupported platform: `"sandbox.enabled is set but ${platform} is not supported (requires macOS, Linux, or WSL2)"`.
- Sandbox disabled-platforms: `"sandbox.enabled is set but ${platform} is not in sandbox.enabledPlatforms"`.
- Sandbox missing deps: `"sandbox.enabled is set but dependencies are missing: ${deps.join(', ')} · ${platform_hint}"`.
- Worktree source-is-worktree: `"Already in a worktree session"`.
- Windows tmux: `"--tmux is not supported on Windows"`.
- MCP tool timeout: `"MCP server \"<server>\" tool \"<tool>\" timed out after Ns"`.
- Keychain plaintext fallback warning: `"Warning: Storing credentials in plaintext."`.

### Numeric constants
| Constant | Value | Locus |
|---|---|---|
| `MAX_LSP_FILE_SIZE_BYTES` | `10_000_000` (10 MB) | LSP file-backed operations reject inputs above this. |
| `STDERR_BUFFER_CAP` | 64 MB | MCP stdio transport per-connection stderr ring buffer. |
| `MAX_MCP_DESCRIPTION_LENGTH` | claude-code constant; respected with `"… [truncated]"` suffix | Tool description truncation. |
| `KEYCHAIN_CACHE_TTL_MS` | `30_000` (30s) | macOS Keychain prefetch cache TTL. |
| `DEFAULT_TIMEOUT` | 30 minutes | `ProcessRunner::run` default timeout when caller omits. |
| `PANE_SHELL_INIT_DELAY_MS` | 200 ms | tmux pane creation post-split delay. |

### Capability matrix (must match claude-code refusal logic)
| Subsystem | macOS | Linux | WSL2 | WSL1 | Windows |
|---|---|---|---|---|---|
| Sandbox | yes (`sandbox-exec`) | yes (`bwrap+socat`) | yes (same as Linux) | **refused** | **refused** |
| Swarm / tmux | yes | yes | yes | n/a | **refused** |
| LSP | yes | yes | yes | yes | yes |
| MCP stdio | yes | yes | yes | yes | yes |
| MCP WebSocket | yes | yes | yes | yes | yes |
| SecureStorage encrypted | yes (Keychain) | no (plaintext) | no (plaintext) | no (plaintext) | no (plaintext) |
| FS watch | yes (FSEvents via notify) | yes (inotify via notify) | yes | yes | yes (RDC via notify) |
