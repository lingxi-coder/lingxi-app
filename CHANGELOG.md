# Changelog

## [0.4.0] — M3 Engine Completion

Locks in claude-code's engine surface — Settings, Memory, real API client,
OAuth refresh, cost events, telemetry schema — at 1:1 byte-aligned parity
with claude-code upstream commit `6a25909` (2026-05-23). 8-10-week single-
developer sustained-Rust delivery per spec §9.

### Crates added

- `lingxi-telemetry-macros` — new sibling proc-macro crate. Ships the
  `tengu_event_audit!()` macro which walks `lingxi-telemetry::tengu/*.rs`
  at compile time and emits `compile_error!()` if any payload struct uses
  bare `String` (must be `Verified` or `PiiTagged`), omits
  `#[serde(deny_unknown_fields)]`, or any payload enum omits
  `#[non_exhaustive]`. Per spec §7 Event evolution policy (lines 781-789).

### Crates expanded

- `lingxi-core` — new `settings/` module tree: `schema.rs` (full
  `SettingsJson` with `deny_unknown_fields`), `env_parser.rs` (3-prefix
  priority `LINGXI_*` > `CLAUDE_CODE_*` > `CLAUDE_*`), `loader.rs`
  (4-layer: env > user > project > defaults), `merger.rs` (per-field
  array/object merge dispatcher), `tracer.rs` (provenance per field),
  and three `tengu_settings_*` events.
- `lingxi-memory` — new `claude_md/` and `memdir/` sub-modules. Walks the
  CLAUDE.md / CLAUDE.local.md hierarchy bottom-up with a 10 MB cap per
  file. Memdir scan applies a 365-day hard-drop threshold then ranks by
  the `score_bps: u64` product (jaccard × age weight × tier weight × team
  boost) — fixed-point `u64` basis points throughout, no `f64` in the
  scoring path. `secret_scan.rs` adapts the existing v3 §16.5
  `lingxi_secret::SecretScanner` (gitleaks rule reuse — no duplicate rule
  set). `tengu_agent_memory_loaded` + `tengu_memory_secret_redacted` emit
  through M3-06's schema.
- `lingxi-api-client` — non-streaming `messages.create` + `count_tokens`
  endpoints, retry middleware (3 attempts at 500ms / 1s / 2s ± 20% jitter
  for thundering-herd mitigation), `Retry-After` + `anthropic-ratelimit-
  requests-reset` aware rate-limit handling, frozen `OAuthRefreshHook`
  trait surface (M3-04 implements; this crate never re-modifies the
  trait), and `BetaHeaderRegistry` emitting only the headers relevant to
  the current request kind (16 locked `anthropic-beta` constants from
  claude-code @ 6a25909, per-provider × per-endpoint applicability).
  Bedrock extra-params route + Vertex `count_tokens` 3-constant allowlist
  captured verbatim.
- `lingxi-anthropic-oauth` — concrete `RefreshDriver` implementing
  M3-03's `OAuthRefreshHook`. Reactive 401 refresh and proactive task
  share a single `refresh_lock: Arc<tokio::sync::Mutex<()>>` with
  double-check-after-acquire; loom test in `refresh_single_flight_test.rs`
  (v3 §32.7 hotspot) verifies concurrent paths collapse to one HTTP
  refresh. Proactive wake interval `min(remaining/2, 5 min)` handles
  short-lived (< 5 min TTL) tokens. 403-with-`required_scopes` re-runs
  PKCE preserving the existing `refresh_token`. Endpoints HTTPS-pinned:
  authorize `https://claude.ai/oauth/authorize`, token
  `https://console.anthropic.com/v1/oauth/token`. Five tengu_oauth_*
  events including `_proactive_canceled` on `Engine::shutdown`.
- `lingxi-cost` — `events.rs` emits `tengu_cost_recorded` /
  `tengu_cost_budget_warning` / `tengu_cost_budget_exceeded` and the
  four `tengu_api_*` events (started / succeeded / failed / rate_limited).
  `is_batch_request: bool` reserved in the `tengu_cost_recorded` payload
  for forward compatibility with M4's Batch endpoint; always `false` in
  v0.4.0. No 50% batch discount in M3 (arrives in M4 alongside the
  endpoint).
- `lingxi-telemetry` — new `tengu/` module tree with 8 sub-modules
  (`api`, `agent`, `session`, `tool`, `cost`, `oauth`, `memory`,
  `settings`) declaring 143 events as the single authoritative source.
  Every payload struct `#[serde(deny_unknown_fields)]`, every payload
  enum `#[non_exhaustive]`, every user-derived string field
  `Verified` / `PiiTagged` (NOT bare `String`). Three new sinks:
  `NoOpSink` (default, no network), `InMemorySink` (test capture),
  `StatsigSink` trait + `MockStatsigSink` skeleton with statsig wire
  shape `{event_name, value, metadata}`.

### 1:1 parity guarantees locked (v0.4.0 additions on top of v0.3.0)

- **Settings**: file paths `~/.claude/settings.json` + `<repo>/.claude/
  settings.json`. 4-layer priority `env > user > project > defaults`. Env
  prefix priority `LINGXI_*` > `CLAUDE_CODE_*` > `CLAUDE_*`. Array-merge
  fields `trustedDirectories`, `additionalDirectories`, `enabledTools`,
  `additionalIncludes`. Object-merge fields `sandbox`, `hooks`,
  `outputStyle`. `"$schema"` not emitted (claude-code does not emit it
  either; reader tolerates for forward compat).
- **Memory**: `CLAUDE.md` (case-sensitive), `CLAUDE.local.md`,
  `~/.claude/memdir/`, `~/.claude/team-mem/`. `MAX_MEMORY_FILE_SIZE = 10 *
  1024 * 1024`. `MEMORY_AGE_PENALTY_DAYS = 30` (relevance penalty unit,
  NOT a drop threshold). `MEMORY_AGE_HARD_DROP_DAYS = 365` (scan-time
  hygiene drop). `MEMORY_MIN_AGE_WEIGHT_BPS = 1_000` (even very old
  entries stay reachable at 10% weight). `DEFAULT_RELEVANT_MEMORIES = 5`.
  Scoring is fixed-point u64 (basis points) — NOT `f64`, cross-platform
  deterministic per §4 Flow C.
- **API client**: base URL `https://api.anthropic.com`, version header
  `anthropic-version: 2023-06-01`, User-Agent
  `claude-cli/<CARGO_PKG_VERSION> (external, cli)`, retry budget 3 with
  exponential backoff 500ms / 1s / 2s ± 20% jitter, streaming timeout 600s,
  `messages.create` timeout 120s, `count_tokens` timeout 30s. Rate-limit
  error string `"Rate limited; retrying in {N}s"`. 16 `anthropic-beta`
  constants locked verbatim from claude-code @ 6a25909.
- **OAuth**: authorize endpoint `https://claude.ai/oauth/authorize`,
  token endpoint `https://console.anthropic.com/v1/oauth/token`, OAuth
  beta header value `oauth-2025-04-20`, refresh grant_type
  `refresh_token`, PKCE method `S256`, 256-bit CSPRNG state token,
  loopback redirect template `http://127.0.0.1:{port}/callback`, 5-minute
  login flow deadline, three scopes `read:user` / `write:messages` /
  `read:projects`. Proactive refresh lead `min(remaining/2, 5 * 60)`
  seconds. Single-flight via `refresh_lock: Arc<tokio::sync::Mutex<()>>`
  per v3 §16.3. 401 retry policy: retry ONCE after refresh.
- **Cost events**: `tengu_cost_recorded` payload fields
  `model: Verified`, `input_tokens: u64`, `output_tokens: u64`,
  `cache_read_input_tokens: u64`, `cache_creation_input_tokens: u64`,
  `cost_usd: u64` (nano-USD per v3 §17), `session_id: Verified`,
  `is_batch_request: bool` (reserved for M4; always false in M3).
  `tengu_cost_budget_warning` uses `percent_bps: u64` (basis points,
  fixed-point per §4 Flow C; M3-06's BudgetWarningPayload locks the type).
- **Telemetry**: ~200 event names organized into 8 modules with locked
  per-category counts: api=25, agent=30, session=15, tool=40, cost=10,
  oauth=8, memory=12, settings=3 (= 143 explicit; ~55 incremental from
  M2-touched subsystems). Statsig wire shape `{event_name, value,
  metadata}` per `claude-code/src/services/statsig.ts`. All payload
  strings `Verified` / `PiiTagged`; `strip_proto_fields` runs at
  every general-access sink. Event-name list is append-only; field
  additions to existing events use sibling-v2 names (`tengu_<name>_v2`)
  over a 2-minor-release deprecation cycle.

### Tests + verification

- Workspace test count: ~700 functional tests + ~24 non-functional gates
  (loom / fuzz / criterion / chaos) per spec §6. Up from 488 at v0.3.0
  (per master spec §6 line 590; M2 final test count); M3 adds ~200-300.
  Net add: ~150 unit + 12 contract drivers + ~25 integration + 6 parity
  + 8 loom + 4 fuzz + 6 criterion + 6 chaos.
- 16 parity drivers gate on every PR: 7 inherited from M2 (M2-07) plus 9
  new from M3 (`parity_settings_merge`, `parity_memory_loading`,
  `parity_memory_relevance`, `parity_messages_create`, `parity_betas`,
  `parity_oauth_pkce_refresh`, `parity_cost_events`, `parity_tengu_events`,
  `parity_full_v0_4_0_smoke`).
- New CI workflows: `ci-loom.yml` (weekly Monday 06:00 UTC),
  `ci-fuzz.yml` (daily 07:00 UTC, continue-on-error: true for v0.4.0),
  `ci-bench.yml` (weekly Monday 08:00 UTC, regression check
  continue-on-error initially), `ci-chaos.yml` (weekly Monday 09:00 UTC,
  hard gate).
- `ci.yml` gains `cross-compile-musl` (v3 §32.4 Layer 5 — `cargo check`
  against `x86_64-unknown-linux-musl`), `supply-chain` (`cargo deny` +
  `cargo audit` + `cargo vet` per v3 §32.4 Layers 1-3), and
  `parity-fixtures` (all 16 `parity_*` drivers).
- `cargo test --workspace` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.
- Existing `cross-compile-desktop` (x86_64-unknown-linux-gnu, aarch64-
  apple-darwin, x86_64-pc-windows-msvc) and `cross-compile-mobile`
  (aarch64-linux-android, aarch64-apple-ios — informational only) jobs
  preserved unchanged from M2-07.

### Known deferrals carried forward to M4+

- **`/v1/messages/batches` endpoint + 50% batch discount** — M4
  alongside the Batch API. The `is_batch_request: bool` field in
  `tengu_cost_recorded` is reserved for that work.
- **Cross-device token sync via claude.ai** — Out of M3. Would land in
  M6 if needed.
- **Statsig HTTP endpoint wiring** — `StatsigSink` trait + `MockStatsigSink`
  skeleton ship in M3-06; real HTTP client + retry remain consumer
  responsibility (M6 task if a real Statsig SDK key becomes available).
- **Embedding-based memory relevance** — M3-02 ships the keyword + age +
  tier heuristic. claude-code may use embeddings; if so, M3.5 or M4 can
  swap to embeddings without breaking the public `MemoryProvider` trait.
- **Anthropic SDK `files` / `models` / `organizations` endpoints** — Not
  in claude-code's usage; not in M3. Land in a separate plan if needed.
- **cargo-fuzz hard-gate** — Currently `continue-on-error: true` in
  `ci-fuzz.yml`. Flip to hard-gate at v0.5.0+.
- **cargo-bench regression baseline** — Currently informational. Baseline
  tooling + `benches/baselines/v0_4_0.json` audit data ship in v0.5.0.
- **cargo-vet supply-chain audit data** — `supply-chain/` audit directory
  is an M4 deliverable; v0.4.0 ships the workflow scaffold only.

### Migration from v0.3.0

The following surfaces changed in source-incompatible ways. Downstream
users of `lingxi-core` as a library MUST update accordingly:

- **`lingxi-telemetry::tengu`** is a new top-level module tree. Code that
  emits events through `AnalyticsBus::log_event` should now reference the
  typed event names from `lingxi_telemetry::tengu::<category>` instead of
  hand-rolled `&'static str`s. Existing string-based call sites still
  compile, but the audit proc-macro will flag any new payload that
  bypasses the typed schema.
- **`lingxi-api-client::OAuthRefreshHook`** is a new trait. Downstream
  consumers that want to participate in 401-driven refresh must implement
  this trait and register via `register_oauth_hook(...)`. The trait is
  frozen — M3-04's `RefreshDriver` is the canonical impl; future
  consumers should compose, not modify.
- **`lingxi-memory` API shape**: the public `MemoryProvider` trait gains
  `find_relevant_memories(query, k) -> Vec<MemoryEntry>` and
  `load_claude_md_hierarchy(repo_root) -> Vec<MemoryEntry>`. Existing
  callers of the M2 shape see `non_exhaustive` warnings.
- **`lingxi-core::settings`** is a new module. The 4-layer loader
  (`Settings::load(LoadInputs)`) replaces any ad-hoc settings reading.
  Downstream code that read settings via direct `serde_json::from_str`
  on `.claude/settings.json` should switch to the loader so it picks up
  the env-var and project-layer merges automatically.

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
