# Architecture

LingXi Code is an event-sourced conversation engine split across 30 crates.
This document is a navigation aid; full design lives in
`docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`.

## Crate map

- `protocol` — shared DTOs, IDs, Effect/Event envelopes
- `core` — state machine, reducer, prompt assembly, session model. M3 adds
  the `settings/` module tree: `schema.rs` (full `SettingsJson` shape),
  `env_parser.rs` (3-prefix priority `LINGXI_*` > `CLAUDE_CODE_*` >
  `CLAUDE_*`), `loader.rs` (4-layer env > user > project > defaults),
  `merger.rs` (per-field dispatcher), `tracer.rs` (provenance).
- `traits` — 13 platform abstraction traits
- `api-client` — Anthropic/OpenAI-compatible API + SSE. M3 adds
  `anthropic/messages_create.rs` + `anthropic/count_tokens.rs`,
  `oauth_hook.rs` (frozen `OAuthRefreshHook` trait), `retry/` middleware
  (3 attempts at 500ms/1s/2s ± 20% jitter), `rate_limit/` (Retry-After
  + `anthropic-ratelimit-requests-reset` aware), `betas.rs` (16 locked
  `anthropic-beta` constants + per-provider × per-endpoint applicability).
- `permission/secret/cost` — security & cost foundations (Plan 02). M3
  extends `cost/events.rs` with `tengu_cost_recorded` (incl. reserved
  `is_batch_request: bool` for M4) / `_budget_warning` / `_budget_exceeded`
  and the four `tengu_api_*` events.
- `tools/hooks` — execution + extension (Plan 03)
- `memory/mcp` — retrieval + tool surface (Plan 04). M3 expands `memory`
  with `claude_md/` (hierarchy walk + 10 MB cap), `memdir/` (memdir +
  team-mem scan + fixed-point u64 ranking), `find.rs`
  (`#![deny(clippy::float_arithmetic)]` integer-only scoring path),
  `secret_scan.rs` (adapter over v3 §16.5 `lingxi_secret::SecretScanner`).
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
- `telemetry` — analytics bus + sinks + PII discipline. M3 adds the
  `tengu/` module tree (8 sub-modules: `api`, `agent`, `session`, `tool`,
  `cost`, `oauth`, `memory`, `settings`; 143 event names; every payload
  struct `#[serde(deny_unknown_fields)]`, every payload enum
  `#[non_exhaustive]`, every user-derived string `Verified` / `PiiTagged`)
  and `sinks/` (NoOpSink default, InMemorySink test capture, StatsigSink
  trait + MockStatsigSink skeleton).
- `telemetry-macros` — sibling proc-macro crate added in M3. Ships
  `tengu_event_audit!()` which walks `telemetry::tengu/*.rs` at compile
  time and emits `compile_error!()` on bare `String`, missing
  `deny_unknown_fields`, or missing `non_exhaustive`.
- `anthropic-oauth` — main auth (Plan 13). M3 adds `refresh/RefreshDriver`
  implementing `OAuthRefreshHook` (single-flight via
  `refresh_lock: Arc<Mutex<()>>` per v3 §16.3; loom-verified hotspot),
  `scope_upgrade.rs` (403-with-`required_scopes` re-runs PKCE preserving
  refresh_token), proactive task with lifecycle owned by `AuthState` and
  cancelable via `Engine::shutdown`.
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

## claude-code parity guarantees (v0.4.0 additions)

M3 locks the following identifiers/paths/numerics on top of v0.3.0.
Coverage in `crates/test-harness/src/parity/fixtures/`: `settings_merge.json`
(M3-01), `memory_loading.json` + `memory_relevance.json` (M3-02),
`messages_create.json` + `betas.json` (M3-03), `oauth_pkce_refresh.json`
(M3-04), `cost_events.json` (M3-05), `tengu_events.json` (M3-06),
`full_v0_4_0_smoke.json` (M3-07 cross-check).

### Settings (M3-01)
| Item | Value | Rationale |
|---|---|---|
| User settings file | `~/.claude/settings.json` | Mirrors claude-code's location. |
| Project settings file | `<repo>/.claude/settings.json` | Same. |
| 4-layer priority | `env > user > project > defaults` | Higher specificity wins. |
| Env var prefix priority | `LINGXI_*` > `CLAUDE_CODE_*` > `CLAUDE_*` | Allows policy override across both LingXi and inherited claude-code env vars. |
| Array-merge fields | `trustedDirectories`, `additionalDirectories`, `enabledTools`, `additionalIncludes` | Per spec §7 line 633. |
| Object-merge fields | `sandbox`, `hooks`, `outputStyle` | Per spec §7 line 634. |
| `$schema` emission | NOT emitted (reader tolerates) | claude-code does not emit it. |

### Memory (M3-02)
| Item | Value |
|---|---|
| Project memory filename | `CLAUDE.md` (case-sensitive) |
| Local override filename | `CLAUDE.local.md` |
| Memdir directory | `~/.claude/memdir/` |
| Team memory directory | `~/.claude/team-mem/` |
| Per-file size cap | 10 MB (`MAX_MEMORY_FILE_SIZE = 10 * 1024 * 1024`) |
| Age penalty block | 30 days (`MEMORY_AGE_PENALTY_DAYS = 30`) — relevance penalty, NOT a drop |
| Hard-drop threshold | 365 days (`MEMORY_AGE_HARD_DROP_DAYS = 365`) — scan-time hygiene |
| Minimum age weight | 1000 bps (`MEMORY_MIN_AGE_WEIGHT_BPS = 1_000`) — even very old entries reachable at 10% |
| Default relevance k | 5 (`DEFAULT_RELEVANT_MEMORIES = 5`) |
| Scoring arithmetic | fixed-point `u64` (basis points), NOT `f64` — cross-platform deterministic per §4 Flow C |

### API client (M3-03)
| Item | Value |
|---|---|
| Base URL | `https://api.anthropic.com` |
| Version header | `anthropic-version: 2023-06-01` |
| User-Agent | `claude-cli/<CARGO_PKG_VERSION> (external, cli)` |
| Retry budget (default) | 3 (exponential backoff 500ms / 1s / 2s ± 20% jitter) |
| Streaming timeout | 600s (10 min) |
| `messages.create` timeout | 120s |
| `count_tokens` timeout | 30s |
| Rate-limit error string | `"Rate limited; retrying in {N}s"` |

The 16 locked `anthropic-beta` constants are stored in
`lingxi-api-client/src/anthropic/betas.rs` and asserted byte-for-byte
in `parity_betas.json`. Vertex `count_tokens` is restricted to the
three-constant `VERTEX_COUNT_TOKENS_ALLOWED` allowlist; Bedrock routes
`INTERLEAVED_THINKING`, `CONTEXT_1M`, and `TOOL_SEARCH_TOOL_3P` via
`extraBodyParams` rather than the header. The full list:
`claude-code-20250219`, `interleaved-thinking-2025-05-14`,
`context-1m-2025-08-07`, `context-management-2025-06-27`,
`structured-outputs-2025-12-15`, `web-search-2025-03-05`,
`advanced-tool-use-2025-11-20`, `tool-search-tool-2025-10-19`,
`effort-2025-11-24`, `task-budgets-2026-03-13`,
`prompt-caching-scope-2026-01-05`, `fast-mode-2026-02-01`,
`redact-thinking-2026-02-12`, `token-efficient-tools-2026-03-28`,
`advisor-tool-2026-03-01`, `oauth-2025-04-20`.

### OAuth (M3-04)
| Item | Value |
|---|---|
| Authorize endpoint | `https://claude.ai/oauth/authorize` (HTTPS-pinned) |
| Token endpoint | `https://console.anthropic.com/v1/oauth/token` (HTTPS-pinned) |
| OAuth beta header value | `oauth-2025-04-20` |
| Refresh grant_type | `refresh_token` |
| PKCE method | `S256` (v3 §30 mandate) |
| State token entropy | 256 bits CSPRNG (v3 §30.1 `PkceFlowState`) |
| Redirect URI template | `http://127.0.0.1:{port}/callback` (loopback only) |
| Login flow deadline | 5 min (300s) |
| Scopes | `read:user`, `write:messages`, `read:projects` |
| Proactive refresh lead | `min(remaining_lifetime / 2, 5 * 60)` seconds |
| Single-flight lock | `refresh_lock: Arc<tokio::sync::Mutex<()>>` (v3 §16.3) |
| 401 retry policy | retry ONCE after refresh |

### Cost events (M3-05)
- `tengu_cost_recorded` payload fields: `model: Verified`,
  `input_tokens: u64`, `output_tokens: u64`, `cache_read_input_tokens: u64`,
  `cache_creation_input_tokens: u64`, `cost_usd: u64` (nano-USD),
  `session_id: Verified`, `is_batch_request: bool` (reserved for M4,
  always `false` in v0.4.0).
- `tengu_cost_budget_warning` uses `percent_bps: u64` (basis points;
  fixed-point per §4 Flow C — no `f64` in payload).
- Four `tengu_api_*` events: `_request_started`, `_request_succeeded`,
  `_request_failed`, `_rate_limited`. All payload strings `Verified`.

### Telemetry schema (M3-06)
- ~200 `tengu_*` event names in 8 sub-modules. Per-category counts:
  api=25, agent=30, session=15, tool=40, cost=10, oauth=8, memory=12,
  settings=3 (= 143 explicit; ~55 incremental from M2-touched subsystems).
- Statsig wire shape: `{event_name, value, metadata}` per
  `claude-code/src/services/statsig.ts::logStatsigEvent`.
- All payload strings `Verified` / `PiiTagged`; `strip_proto_fields`
  runs at every general-access sink before serialization.
- Event evolution: append-only event names; adding a field to an
  existing event is breaking — define sibling `tengu_<name>_v2` and
  deprecate `tengu_<name>` over 2 minor releases. Enforced by the
  `tengu_event_audit!()` proc-macro in `lingxi-telemetry-macros`.

### CI gates added in v0.4.0
| Gate | Trigger | Hard? |
|---|---|---|
| `cross-compile-musl` (`x86_64-unknown-linux-musl` cargo check) | per-PR | Yes (v3 §32.4 Layer 5) |
| `supply-chain` (cargo-deny + cargo-audit + cargo-vet) | per-PR | Yes for deny + audit; warn for vet until M4 |
| `parity-fixtures` (all 16 `parity_*` drivers) | per-PR | Yes |
| `ci-loom.yml` (M3-04 OAuth refresh single-flight) | weekly Monday 06:00 UTC + manual | Yes (warning fallback for not-yet-added tests) |
| `ci-fuzz.yml` (4 cargo-fuzz harnesses) | daily 07:00 UTC + manual | No (continue-on-error: true for v0.4.0; mandatory at v0.5.0+) |
| `ci-bench.yml` (6 criterion benches + baseline check) | weekly Monday 08:00 UTC + manual | No (baseline tooling lands in v0.5.0) |
| `ci-chaos.yml` (6 fault-injection scenarios) | weekly Monday 09:00 UTC + manual | Yes |
