# LingXi Core M3 — Engine Completion Design

> **Status**: DRAFT (awaiting user review)
> **Date**: 2026-05-23
> **Target version**: v0.4.0
> **Predecessor**: M2 / v0.3.0 (claude-code parity for MCP / LSP / Sandbox / FS / Swarm / SecureStorage)
> **Successors**: M4 (Tools 全集) → M5 (Agent surface + Commands + Hooks) → M6 (Plugin marketplace + MCP server + /doctor + Cron + UniFFI polish)

---

## §1 Goal & non-goals

### Goal

Close the remaining **engine-layer** parity gaps versus claude-code TypeScript, producing v0.4.0:

1. **Full Anthropic API client** — non-streaming `messages.create`, `count_tokens`, retry/rate-limit/401-refresh middleware
2. **OAuth completion** — proactive token refresh (5-min lead), 401-driven reactive refresh, scope upgrade flow
3. **Memory system 1:1** — CLAUDE.md hierarchy, memdir scan + age decay + relevance ranking, team memory, secret scanning
4. **Settings 4-layer merge** — env > user > project > defaults with per-field merge rules (arrays concat-dedup, objects deep-merge, scalars override)
5. **Cost event emission** — emit `tengu_cost_*` and `tengu_api_*` events on every API call, byte-aligned payload
6. **Telemetry schema lock** — define and lock ~200 `tengu_*` event schemas; default NoOp sink; `StatsigSink` trait as extension point

### Non-goals (deferred to later milestones)

- **Tool implementations** (40 concrete tools — Bash/Edit/Grep/WebFetch/Agent/Task/Skill/Notebook/...) → **M4**
- **Slash commands** (`/clear`, `/compact`, `/memory`, `/init`, `/resume`, `/cost`, ...) → **M5** (M3 provides `/cost` data-only stub)
- **Hooks lifecycle** (PreToolUse / PostToolUse / UserPromptSubmit / Stop / SubagentStop / Notification / SessionStart / PreCompact) → **M5**
- **Agent prompt templates** byte-aligned (system message, persona, sub-agent prompts) → **M5**
- **Plugin marketplace** (discover / install / update / manifest validation) → **M6**
- **MCP server side** (expose LingXi as an MCP server to other clients) → **M6**
- **`/doctor` diagnostic command** → **M6** (M3-01 provides `effective_for(field)` API that `/doctor` will consume)
- **Cron user-level workflow** (agent-scheduled long tasks) → **M6**
- **UniFFI bridge API stabilization** → **M6**
- **UI / Terminal rendering** (out of scope entirely — there is no Rust ink equivalent in this design)
- **Mobile real-device binding** (only cross-compile gates remain)
- **Statsig HTTP endpoint** (M3 ships `StatsigSink` trait and NoOp default; real HTTP integration is consumer-chosen extension)

### What changed since M2

M2 v0.3.0 landed all wire-protocol-level parity (MCP / LSP / IDE bridge / sandbox / worktree / keychain / FS watch / swarm / process tree-kill). The remaining gap is **engine semantics**: how settings combine, how memory loads, how the API client behaves under failure, what events flow through telemetry. M3 closes those.

---

## §2 Scope decomposition: M3 → M6 milestone outline

```
v0.3.0 (now)  ──►  M3 (v0.4.0)        ──►  M4 (v0.5.0)        ──►  M5 (v0.6.0)              ──►  M6 (v0.7.0)
                   Engine completion       Tools 全集               Agent surface +              Ecosystem
                                                                   Commands + Hooks
                   5-7 weeks               8-10 weeks               5-6 weeks                    4-5 weeks
```

M3 is **single-developer, ~5-7 weeks, six sub-plans**:

```
M3-01 Settings  ──►  M3-02 Memory  ──►  M3-03 API client  ──►  M3-04 OAuth  ──►  M3-05 Cost events  ──►  M3-06 Telemetry
4-layer merge        CLAUDE.md +        non-stream +              refresh +         emit tengu_cost_*       schema lock +
+ per-field rules    memdir + ranking    batches + retry          scope upgrade     + /cost data API        NoOp sink +
                     + team + secret     + count_tokens                                                     StatsigSink trait
```

Each sub-plan is independently committable, independently verifiable, and produces its own milestone tag (`m3.1` through `m3.6`). The final v0.4.0 release tag annotates the entire chain.

---

## §3 Architecture & components

### Existing crates touched (no new crates created)

| Plan | Target crate(s) | Approx new LOC | Approx test LOC |
|------|----------------|----------------|-----------------|
| M3-01 Settings | `lingxi-core` (new `settings/` module), `lingxi-protocol` (schema types) | 800 | 600 |
| M3-02 Memory | `lingxi-memory` (new `memdir/` + `claude_md/` modules) | 1200 | 900 |
| M3-03 API client | `lingxi-api-client` (extend `anthropic.rs`, new `retry.rs` + `rate_limit.rs`) | 600 | 500 |
| M3-04 OAuth | `lingxi-anthropic-oauth` (new `refresh.rs` + `scope_upgrade.rs`) | 500 | 400 |
| M3-05 Cost events | `lingxi-cost` (new `events.rs`), `lingxi-telemetry` (uses bus) | 400 | 350 |
| M3-06 Telemetry schema | `lingxi-telemetry` (new `tengu/` module + `sinks/`) | 700 | 650 |
| **Total** | | **~4200** | **~3400** |

### Module structure increments

```
lingxi-core/crates/core/
  settings/                              [NEW M3-01]
    mod.rs
    loader.rs                            # 4-layer source priority loader
    merger.rs                            # per-field merge rules
    tracer.rs                            # effective-value provenance for /doctor
    schema.rs                            # SettingsJson type + validation
    env_parser.rs                        # LINGXI_*/CLAUDE_*/CLAUDE_CODE_* env walker

lingxi-core/crates/memory/
  memdir/                                [NEW M3-02; mirrors claude-code/src/memdir/]
    mod.rs
    scan.rs                              # memoryScan.ts equivalent
    age.rs                               # memoryAge.ts equivalent (30-day decay)
    find.rs                              # findRelevantMemories.ts equivalent (ranked retrieval)
    paths.rs                             # paths.ts equivalent (~/.claude/memdir/)
    team_paths.rs                        # teamMemPaths.ts equivalent (~/.claude/team-mem/)
    team_prompts.rs                      # teamMemPrompts.ts equivalent
    secret_scan.rs                       # replaces M2 stub
  claude_md/                             [NEW M3-02]
    mod.rs
    hierarchy.rs                         # dir-up walk + user CLAUDE.md
    loader.rs                            # file reader + size cap

lingxi-core/crates/api-client/
  anthropic.rs                           [EXTEND M3-03; +200 LOC]
                                         # add non-stream messages.create + count_tokens
  retry.rs                               [NEW M3-03]
                                         # exponential-backoff middleware (3 retries: 500ms/1s/2s)
  rate_limit.rs                          [NEW M3-03]
                                         # parse Retry-After / anthropic-ratelimit-* headers

lingxi-core/crates/anthropic-oauth/
  refresh.rs                             [NEW M3-04]
                                         # reactive (401-driven) + proactive (5-min lead) refresh
  scope_upgrade.rs                       [NEW M3-04]
                                         # scope upgrade flow when API requires new scope

lingxi-core/crates/cost/
  events.rs                              [NEW M3-05]
                                         # emit tengu_cost_* on every API call
  tracker.rs                             [EXTEND M3-05]
                                         # hook into events; add cache hit/miss + batches discount

lingxi-core/crates/telemetry/
  tengu/                                 [NEW M3-06]
    mod.rs
    api.rs                               # tengu_api_*       (~25 events)
    agent.rs                             # tengu_agent_*     (~30 events)
    session.rs                           # tengu_session_*   (~15 events)
    tool.rs                              # tengu_tool_*      (~40 events; M4 will populate)
    cost.rs                              # tengu_cost_*      (~10 events; M3-05 emits)
    oauth.rs                             # tengu_oauth_*     (~8 events;  M3-04 emits)
    memory.rs                            # tengu_memory_*    (~12 events; M3-02 emits)
    settings.rs                          # tengu_settings_*  (~5 events;  M3-01 emits)
  sinks/                                 [NEW M3-06]
    noop.rs                              # default; logs only, no network
    statsig.rs                           # trait + skeleton impl, no SDK key
```

### Dependency chain (foundational → consumer)

```
M3-01 Settings ─────► consumed by every other M3 sub-plan
       │
       ▼
M3-02 Memory  ──────► uses Settings for memdir/team paths + secret-scan rules
       │
       ▼
M3-03 API client ───► uses Settings for baseUrl/proxy + retry policy thresholds
       │
       ▼
M3-04 OAuth ────────► wires into api-client (401 hook); uses keychain (M2-06)
       │
       ▼
M3-05 Cost events ──► sits on api-client's response middleware; emits via telemetry bus
       │
       ▼
M3-06 Telemetry ────► authoritative event schema; M3-05's events must conform
```

This ordering allows each plan to land as a clean commit with a working verification gate. No back-references; downstream plans extend (don't modify) upstream artifacts.

---

## §4 Data flow

### Flow A — Startup sequence (settings → memory → OAuth)

```
Engine::init():
  ├─► settings::Loader::load() [M3-01]
  │     ├─► read env vars (LINGXI_*, CLAUDE_CODE_*, CLAUDE_*)
  │     ├─► read user file (~/.claude/settings.json)
  │     ├─► read project file (<repo>/.claude/settings.json)
  │     ├─► load defaults (compile-time const)
  │     ├─► merger::merge() with per-field rules
  │     └─► tracer records source-of-truth per field
  │     [returns EffectiveSettings]
  │
  ├─► memory::Loader::load(settings.workspace_root) [M3-02]
  │     ├─► claude_md::hierarchy::collect()
  │     │     ├─► CLAUDE.md (cwd)
  │     │     ├─► CLAUDE.md (parents up to repo root)
  │     │     ├─► .claude/CLAUDE.md (per-dir override)
  │     │     ├─► ~/.claude/CLAUDE.md
  │     │     └─► ~/.claude/CLAUDE.local.md
  │     ├─► memdir::scan::scan(memdir_root)
  │     ├─► memdir::age::filter_stale() — drop entries older than 30 days
  │     ├─► memdir::team_paths::collect() (only if settings.team_mode)
  │     └─► memdir::secret_scan::redact()
  │     [returns MemorySnapshot]
  │     [emits tengu_agent_memory_loaded]
  │
  └─► oauth::Resolver::resolve(settings.auth_strategy) [M3-04]
        ├─► keychain.retrieve("claude-code-credentials") → token
        ├─► if token expired or null:
        │     ├─► trigger PKCE flow if no refresh_token
        │     └─► reactive refresh if refresh_token exists
        ├─► spawn proactive refresh task (wakes 5 min before expiry)
        └─► register 401 hook into api-client middleware
        [returns AuthState]
        [emits tengu_oauth_refresh_*]
```

### Flow B — API messages.create with retry + refresh + cost (the core path)

```
caller: api_client.messages_create(model, msgs)
      │
      ▼
api_client::middleware [M3-03]
      │
      ├─► telemetry::emit("tengu_api_request_started", { model, request_id, stream })
      │
      ├─► rate_limit::check() ─────► sleep if at limit
      │
      ├─► retry::with_backoff(max=3):
      │     ▼
      │   anthropic.rs::do_request(POST /v1/messages)
      │     ▼ HTTP response
      │     │
      │     ├─► 200 OK ─────────────────► continue
      │     │
      │     ├─► 401 Unauthorized ──────► oauth::refresh_hook.trigger() [M3-04]
      │     │      ├─► POST /v1/oauth/token (grant_type=refresh_token)
      │     │      ├─► update keychain
      │     │      ├─► emit tengu_oauth_refresh_succeeded
      │     │      └─► retry ONCE with new token (no further retries if it 401s again)
      │     │
      │     ├─► 429 Rate-limited ─────► parse Retry-After → sleep → retry
      │     │
      │     ├─► 5xx ────────────────────► exponential backoff (0.5s / 1s / 2s) → retry
      │     │
      │     └─► other 4xx ──────────────► propagate as ApiError::Client (no retry)
      │
      ▼ on 200
      cost::tracker::record(model, usage) [M3-05]
      │
      ├─► calculate cost via pricing table (with cache hit/miss + batches discount)
      ├─► update budget; if over: emit tengu_cost_budget_exceeded
      ├─► if 80%+: emit tengu_cost_budget_warning
      └─► emit tengu_cost_recorded { model, input_tokens, output_tokens, cache_read,
                                     cache_write, cost_usd, session_id }
      │
      ▼
      telemetry::emit("tengu_api_request_succeeded", { model, request_id, duration_ms, status })
      │
      ▼
      return MessagesResponse to caller
```

### Flow C — Memory retrieval (findRelevantMemories)

```
agent_loop step:
  needs_memory(prompt: &str, k: usize) → Vec<MemoryEntry>
      │
      ▼
  memory::find::find_relevant(prompt, k): [M3-02]
      │
      ├─► scan all memory entries (from snapshot)
      ├─► for each entry: score = relevance_score(prompt, entry)
      │     ├─► keyword overlap (Jaccard between prompt tokens and entry tokens)
      │     ├─► age decay (entries > 30d: score *= 0.5 per 30d block)
      │     ├─► section weight (recent session 1.0 > project 0.8 > user 0.6)
      │     └─► team boost (team memory entries × 1.2 if user opted in)
      ├─► sort desc by score
      ├─► take top k (default k = 5; configurable via settings)
      ├─► emit telemetry "tengu_agent_memory_loaded" { count, sources }
      └─► return Vec<MemoryEntry>
```

### Flow D — Settings effective-value lookup (used by `/doctor`)

```
/doctor cmd: settings::tracer::effective_for("trustedDirectories")
      │
      ▼ returns:
      {
        value: ["~/projects", "/opt/claude"],
        sources: [
          { layer: "user",    file: "~/.claude/settings.json", value: ["~/projects"] },
          { layer: "project", file: ".claude/settings.json",   value: ["/opt/claude"] },
        ],
        merge_rule: "concat-dedup",
      }
```

The tracer is a passive observer attached to the merger. Every merge operation records which source contributed which value. `/doctor` (M6) reads this map.

---

## §5 Error handling

### Per-subsystem error types

Each subsystem keeps its own `Error` enum (no unified `LingxiError`); top-level `Engine` aggregates as needed.

```rust
// M3-01 Settings
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings file not found: {0}")]
    Missing(PathBuf),
    #[error("settings file malformed at {path}: {source}")]
    ParseError { path: PathBuf, source: serde_json::Error },
    #[error("env var {var} has invalid value {value:?}")]
    InvalidEnv { var: String, value: String },
    #[error("schema validation failed: {0}")]
    SchemaViolation(String),
    #[error("io error reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

// M3-02 Memory
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("CLAUDE.md exceeds 10MB at {path}")]
    FileTooLarge { path: PathBuf, size: u64 },
    #[error("memdir scan failed at {path}: {source}")]
    ScanFailed { path: PathBuf, source: std::io::Error },
    #[error("secret detected in {path}: {kind}")]
    SecretLeak { path: PathBuf, kind: String },
    #[error("team memory unavailable: {0}")]
    TeamUnavailable(String),
}

// M3-03 API client (EXTEND existing ApiError)
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /* existing variants from M2 ... */
    #[error("Rate limited; retrying in {seconds}s")]
    RateLimited { seconds: u64 },
    #[error("API authentication failed.")]
    AuthExhausted,
    #[error("retry budget exhausted after {attempts} attempts")]
    RetryExhausted { attempts: u32 },
    #[error("count_tokens estimate failed: {0}")]
    CountTokens(String),
}

// M3-04 OAuth (EXTEND existing OAuthError)
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /* existing variants ... */
    #[error("Session expired. Re-authenticate?")]
    RefreshExpired,
    #[error("Scope upgrade denied by provider")]
    ScopeRejected { required: Vec<String>, granted: Vec<String> },
    #[error("proactive refresh failed: {source}")]
    ProactiveFailed { source: Box<OAuthError> },
}

// M3-05 Cost
#[derive(Debug, thiserror::Error)]
pub enum CostError {
    /* existing variants ... */
    #[error("Cost tracking unavailable for {model}")]
    UnknownModel { model: String },
    #[error("Budget exceeded (${current:.2}); stopped.")]
    BudgetExceeded { limit: f64, current: f64 },
}

// M3-06 Telemetry
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /* existing variants ... */
    #[error("unknown event name {name} (not in tengu_* schema)")]
    UnknownEvent { name: String },
    #[error("event payload validation failed for {event}: {detail}")]
    PayloadInvalid { event: String, detail: String },
}
```

### Recovery strategy

| Error | Auto-recover? | User-facing string (byte-aligned w/ claude-code) |
|-------|--------------|--------------------------------------------------|
| `ApiError::RateLimited` | sleep + retry | `"Rate limited; retrying in {N}s"` |
| `ApiError::AuthExhausted` | ❌ terminate | `"API authentication failed."` |
| `ApiError::5xx (transient)` | exp backoff × 3 | (silent — log only) |
| `OAuthError::RefreshExpired` | trigger PKCE flow | `"Session expired. Re-authenticate?"` |
| `OAuthError::ScopeRejected` | ❌ terminate | `"Scope upgrade denied by provider"` |
| `SettingsError::Missing` | fallback to defaults | (silent — log only) |
| `SettingsError::ParseError` | fallback to defaults | `"Settings file {path} ignored: {error}"` |
| `MemoryError::FileTooLarge` | skip file | `"CLAUDE.md skipped: file exceeds 10MB"` |
| `MemoryError::SecretLeak` | redact + warn | `"Secrets redacted from memory ({n} entries)"` |
| `MemoryError::TeamUnavailable` | fallback to local | (silent unless team mode required) |
| `CostError::BudgetExceeded` | ❌ terminate (default) | `"Budget exceeded (${X.XX}); stopped."` |
| `CostError::UnknownModel` | skip billing | `"Cost tracking unavailable for {model}"` |

The user-facing strings are locked in `parity_error_strings.json` and asserted byte-for-byte against claude-code source.

### Panic policy

- Business logic: **never panic**. All fallible operations return `Result`.
- Programmer errors: `debug_assert!` for invariants; release builds fall through silently (no `unwrap()` outside tests).
- Startup-time invariants (e.g., schema validation): panic with a clear message is acceptable; this catches misconfiguration before serving any requests.

---

## §6 Testing strategy

### Test pyramid (inherits M2 pattern)

```
                  ╱╲
                 ╱  ╲    Parity fixtures (~6 new JSON files)
                ╱────╲   wire-byte alignment w/ claude-code
               ╱      ╲
              ╱  Integ ╲  Integration tests (~25 new)
             ╱──────────╲ cross-crate end-to-end flows
            ╱            ╲
           ╱   Contract   ╲  Contract suites (~6 new parameterized harnesses)
          ╱────────────────╲ Settings/Memory/API/OAuth/Cost/Telemetry
         ╱                  ╲
        ╱     Unit tests     ╲ ~150+ unit tests
       ╱______________________╲ TDD red→green per task
```

### Per-sub-plan test targets

| Plan | Unit | Contract | Integration | Parity fixture |
|------|------|----------|-------------|----------------|
| M3-01 Settings | merger / loader / tracer / schema / env_parser | `run_settings_contract<L: SettingsLoader>` | 4-layer end-to-end with real tempfile dirs | `parity_settings_merge.json` |
| M3-02 Memory | claude_md / memdir / age / find / secret_scan | `run_memory_contract<M: MemoryProvider>` | CLAUDE.md hierarchy with mocked `~/.claude/` layout | `parity_memory_loading.json`, `parity_memory_relevance.json` |
| M3-03 API client | retry / rate_limit / count_tokens decoder | extend M2's existing `run_http_contract` | axum mock: 401 → refresh → retry roundtrip | `parity_messages_create.json` |
| M3-04 OAuth | refresh / scope_upgrade | `run_oauth_contract<C: OAuthClient>` | proactive task + 401 hook with mock identity provider | `parity_oauth_pkce_refresh.json` |
| M3-05 Cost events | event emission / pricing extension | (uses existing cost contract; adds 2 events tests) | full API call → cost event flow through telemetry bus | `parity_cost_events.json` |
| M3-06 Telemetry schema | schema validation / NoOp sink / StatsigSink trait | `run_telemetry_contract<S: TelemetrySink>` | end-to-end emit → sink trip on representative events | `parity_tengu_events.json` (200+ event names + payload samples) |

### Expected test counts at v0.4.0

- Existing M2 tests: 488 (all stay green)
- New unit tests: ~150
- New contract drivers: 12 (6 contracts × 2 drivers each on average)
- New integration tests: ~25
- New parity drivers: 6 (one per fixture file)
- **Total at v0.4.0: ~700 tests**

### Cross-platform CI

Continues M2 matrix; M3 adds parity fixtures to the gating set:

- `x86_64-apple-darwin` ✅ build + test
- `aarch64-apple-darwin` ✅ build + test
- `x86_64-unknown-linux-gnu` ✅ build + test
- `x86_64-pc-windows-msvc` ✅ build + test (Windows CI runner)
- `x86_64-pc-windows-gnu` ⚠️ `cargo check` only (local host mingw-w64 not installed)

### Test discipline

- Each fileful module: **≥1 happy path test, ≥1 error path test, byte-literal assertions for every locked wire identifier**.
- Coverage percentages not enforced; the "module ≥3 test types" rule is the gate.
- Test runtime targets: `cargo test -p <crate>` < 5s; full workspace < 60s; with `--ignored` < 120s.

---

## §7 Wire identifiers locked (byte-alignment table)

These literals must appear byte-for-byte in code AND in at least one test assertion. Drift breaks interop with claude-code, claude.ai OAuth, and Statsig analytics.

### Settings (M3-01)

| Item | Literal | Lock site |
|------|---------|-----------|
| User settings file | `~/.claude/settings.json` | `parity_settings_merge.json` |
| Project settings file | `<repo>/.claude/settings.json` | same |
| Env var prefix priority | `LINGXI_*` → `CLAUDE_CODE_*` → `CLAUDE_*` | same |
| Array-merge fields | `trustedDirectories`, `additionalDirectories`, `enabledTools`, `additionalIncludes` | same |
| Object-merge fields | `sandbox`, `hooks`, `outputStyle` | same |
| Schema version | `"$schema": "claude-code-settings-v1"` (if claude-code emits) | same |

### Memory (M3-02)

| Item | Literal |
|------|---------|
| Project memory filename | `CLAUDE.md` (case-sensitive) |
| Local override filename | `CLAUDE.local.md` |
| Memdir directory | `~/.claude/memdir/` |
| Team memory directory | `~/.claude/team-mem/` |
| Per-file size cap | 10 MB (`MAX_MEMORY_FILE_SIZE = 10 * 1024 * 1024`) |
| Age decay threshold | 30 days (`MEMORY_AGE_PENALTY_DAYS = 30`) |
| Default relevance k | 5 (`DEFAULT_RELEVANT_MEMORIES = 5`) |
| Telemetry events | `tengu_agent_memory_loaded`, `tengu_memory_secret_redacted` |

### API client (M3-03)

| Item | Literal |
|------|---------|
| Base URL | `https://api.anthropic.com` |
| Version header | `anthropic-version: 2023-06-01` |
| Beta header | `anthropic-beta: <comma-list from claude-code/src/services/api/claude.ts>` |
| User-Agent | `claude-cli/<CARGO_PKG_VERSION> (external, cli)` |
| Retry budget | 3 (exponential backoff: 500ms / 1s / 2s) |
| Default timeout | 600s (10 min — distinct from MCP's 60s) |
| Rate-limit headers | `Retry-After` (seconds), `anthropic-ratelimit-requests-reset` (ISO8601) |
| Rate-limit error string | `"Rate limited; retrying in {N}s"` |

### OAuth (M3-04)

| Item | Literal |
|------|---------|
| Authorize endpoint | `https://claude.ai/oauth/authorize` |
| Token endpoint | `https://console.anthropic.com/v1/oauth/token` |
| Refresh grant_type | `refresh_token` |
| Scopes | `read:user`, `write:messages`, `read:projects` (final list from claude-code) |
| Proactive refresh lead | 5 minutes (`PROACTIVE_REFRESH_LEAD = 5 * 60`) |
| 401 retry policy | retry ONCE after refresh; no further retries |
| Telemetry events | `tengu_oauth_refresh_started`, `tengu_oauth_refresh_succeeded`, `tengu_oauth_refresh_failed`, `tengu_oauth_scope_upgraded` |

### Cost events (M3-05)

| Event name | Payload fields (byte-aligned) |
|------------|-------------------------------|
| `tengu_cost_recorded` | `model`, `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`, `cost_usd`, `session_id` |
| `tengu_cost_budget_warning` | `limit_usd`, `current_usd`, `percent` |
| `tengu_cost_budget_exceeded` | `limit_usd`, `current_usd` |
| `tengu_api_request_started` | `model`, `request_id`, `stream` |
| `tengu_api_request_succeeded` | `model`, `request_id`, `duration_ms`, `status` |
| `tengu_api_request_failed` | `model`, `request_id`, `error_kind`, `status_code` |

### Telemetry schema (M3-06)

~200 `tengu_*` event names organized into 8 modules:

```
tengu_api_*         ~25   (M3-05 emits)
tengu_agent_*       ~30   (M5 will emit; schema M3-locked)
tengu_session_*     ~15   (M5)
tengu_tool_*        ~40   (M4 emits; schema M3-locked)
tengu_cost_*        ~10   (M3-05 emits)
tengu_oauth_*       ~8    (M3-04 emits)
tengu_memory_*      ~12   (M3-02 emits)
tengu_settings_*    ~5    (M3-01 emits)
                    ─────
                    ~145 explicit
                    ~55  from M2-touched subsystems (skill/swarm/bridge/etc) added incrementally
```

Each event is a `#[serde(deny_unknown_fields)]` struct in `telemetry::tengu::<category>`. Statsig-compatible serialization is `{event_name, value, metadata}`.

### File paths

| Purpose | Path | Source |
|---------|------|--------|
| User settings | `~/.claude/settings.json` | claude-code |
| Project settings | `<repo>/.claude/settings.json` | claude-code |
| Memdir | `~/.claude/memdir/` | claude-code |
| Team memory | `~/.claude/team-mem/` | claude-code |
| User CLAUDE.md | `~/.claude/CLAUDE.md` | claude-code |
| User local CLAUDE.md | `~/.claude/CLAUDE.local.md` | claude-code |
| Legacy credentials | `~/.claude/credentials.json` | claude-code (legacy) |
| Keychain service | `Claude Code` | M2-06 lock |
| IDE bridge lockfile | `~/.claude/ide/<port>.lock` | M2-02d lock |
| Worktree path | `<repo>/.claude/worktrees/<flatten>` | M2-01 lock |

---

## §8 Sub-plan outline (M3-01 through M3-06)

Each sub-plan is a separate document under `docs/superpowers/plans/2026-05-23-m3-XX-*.md`, drafted via the `superpowers:writing-plans` skill after this spec is approved.

### M3-01 — Settings (4-layer merge + per-field rules)

**Phases** (estimated ~14 bite-sized TDD tasks when expanded by writing-plans):
1. Cargo deps + `settings/` module skeleton
2. `schema.rs` — `SettingsJson` type + per-field merge metadata (`#[merge(strategy = "concat-dedup")]`) + schema validation (rejects unknown fields, type-checks scalars)
3. `env_parser.rs` — walk `LINGXI_*` / `CLAUDE_CODE_*` / `CLAUDE_*` with priority
4. `loader.rs` — 4-layer source loader with fallback chain
5. `merger.rs` — per-field merge dispatcher (array / object / scalar)
6. `tracer.rs` — provenance recorder (passive observer)
7. Telemetry events emitted on settings load: `tengu_settings_loaded`, `tengu_settings_invalid_env`, `tengu_settings_parse_error`
8. Integration test — full 4-layer end-to-end with tempfile dirs
9. Parity fixture `parity_settings_merge.json`
10. Workspace verification + commit

**Acceptance:** `cargo test -p lingxi-core --test settings_*` all pass; `effective_for(field)` returns provenance trace; clippy + fmt clean.

**Tag:** `m3.1`

### M3-02 — Memory (CLAUDE.md hierarchy + memdir + ranking + team + secret-scan)

**Phases** (estimated ~18 bite-sized TDD tasks when expanded by writing-plans):
1. `claude_md/hierarchy.rs` — dir-up walker (cwd → parents → user)
2. `claude_md/loader.rs` — file reader with 10MB size cap
3. `memdir/paths.rs` — path resolution (`~/.claude/memdir/`)
4. `memdir/scan.rs` — directory enumeration
5. `memdir/age.rs` — 30-day decay scoring
6. `memdir/find.rs` — relevance ranking (keyword + age + tier + team)
7. `memdir/team_paths.rs` + `team_prompts.rs`
8. `secret_scan.rs` — replaces M2 stub; redacts common secret patterns
9. Telemetry events: `tengu_agent_memory_loaded`, `tengu_memory_secret_redacted`, `tengu_memory_file_too_large`
10. Integration test — full hierarchy load with mocked dirs
11. Parity fixtures `parity_memory_loading.json` + `parity_memory_relevance.json`
12. Workspace verification + commit

**Acceptance:** `cargo test -p lingxi-memory --test memdir_*` all pass; ranking is deterministic for fixed prompt + memory set; clippy + fmt clean.

**Tag:** `m3.2`

### M3-03 — API client (non-stream + retry + rate-limit + count_tokens)

**Phases** (estimated ~12 bite-sized TDD tasks when expanded by writing-plans):
1. `anthropic.rs` extension — `messages_create(model, msgs)` non-streaming
2. `anthropic.rs` extension — `count_tokens(model, msgs)`
3. `retry.rs` — exp backoff middleware (3 retries: 500ms/1s/2s)
4. `rate_limit.rs` — parse `Retry-After` + `anthropic-ratelimit-*` headers; sleep + retry
5. Beta-header registration (locks `anthropic-beta` literal from claude-code)
6. User-Agent format (`claude-cli/<version> (external, cli)`)
7. Telemetry events: `tengu_api_request_started`, `tengu_api_request_succeeded`, `tengu_api_request_failed`, `tengu_api_rate_limited`
8. Integration test — axum mock with 401 + 429 + 5xx + 200 path coverage
9. Parity fixture `parity_messages_create.json`
10. Workspace verification + commit

**Acceptance:** Mock 401 → triggers OAuth hook → retries once → succeeds; 429 with Retry-After → sleeps → succeeds; 5xx × 3 → fails with `RetryExhausted`; clippy + fmt clean.

**Tag:** `m3.3`

### M3-04 — OAuth (reactive + proactive refresh + scope upgrade)

**Phases** (estimated ~10 bite-sized TDD tasks when expanded by writing-plans):
1. `refresh.rs` — reactive refresh path (401 hook returns `Future<Result<NewToken>>`)
2. `refresh.rs` — proactive refresh task (tokio spawned, wakes 5 min before expiry)
3. Token rotation atomic update to keychain (no race between reactive + proactive)
4. `scope_upgrade.rs` — when API requires new scope (HTTP 403 with `required_scopes` body), re-trigger PKCE preserving refresh_token
5. Telemetry events (4 events listed in §7)
6. Integration test — proactive + reactive collision handling
7. Parity fixture `parity_oauth_pkce_refresh.json`
8. Workspace verification + commit

**Acceptance:** Refresh atomically updates keychain; concurrent 401 + proactive don't double-refresh; clippy + fmt clean.

**Tag:** `m3.4`

### M3-05 — Cost events (emit + budget alarms + /cost data API)

**Phases** (estimated ~8 bite-sized TDD tasks when expanded by writing-plans):
1. `events.rs` — `tengu_cost_recorded` emitter (called from api-client response middleware)
2. `tracker.rs` extension — cache hit/miss tracking (cache_read_input_tokens / cache_creation_input_tokens)
3. `tracker.rs` extension — batches discount (50% off, claude-code parity)
4. Budget alarm emission (warning at 80%, exceeded at 100%)
5. `/cost` data API — `CostTracker::summary() -> CostSummary { session, day, month, models }` (M5 will wire this to the slash command)
6. Integration test — full API call → cost event → budget hit flow
7. Parity fixture `parity_cost_events.json`
8. Workspace verification + commit

**Acceptance:** Cost event fires on every API success; budget alarm fires at correct thresholds; clippy + fmt clean.

**Tag:** `m3.5`

### M3-06 — Telemetry schema lock (200 events + NoOp sink + StatsigSink trait)

**Phases** (estimated ~12 bite-sized TDD tasks when expanded by writing-plans):
1. `tengu/api.rs` — ~25 events with byte-aligned payload structs
2. `tengu/agent.rs` — ~30 events (skeletons; M5 fills payload values)
3. `tengu/session.rs` — ~15 events (skeletons)
4. `tengu/tool.rs` — ~40 events (skeletons; M4 fills payload values)
5. `tengu/cost.rs` — ~10 events (used by M3-05)
6. `tengu/oauth.rs` — ~8 events (used by M3-04)
7. `tengu/memory.rs` — ~12 events (used by M3-02)
8. `tengu/settings.rs` — ~5 events (used by M3-01)
9. `sinks/noop.rs` — default sink; logs via tracing, no network
10. `sinks/statsig.rs` — `StatsigSink` trait; sample impl with placeholder HTTP client (no SDK key wired)
11. Parity fixture `parity_tengu_events.json` (full event name list + sample payloads)
12. Workspace verification + commit

**Acceptance:** All event structs serialize with `#[serde(deny_unknown_fields)]`; NoOp sink is default; StatsigSink trait compiles + has skeleton impl; clippy + fmt clean.

**Tag:** `m3.6`

### Release: M3-07 (verification + v0.4.0 tag)

After all six sub-plans land, a final verification + release commit:
- `cargo test --workspace` (~700 tests)
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all --check`
- Cross-target build matrix (4 targets)
- Update CHANGELOG.md / ARCHITECTURE.md / PLATFORMS.md
- Tag `v0.4.0` annotated

---

## §9 Release plan & timeline

Single-developer, 5-7 weeks estimated:

| Week | Sub-plan | Notes |
|------|---------|-------|
| 1 | M3-01 Settings | Pure data + merging; no I/O dependencies |
| 2 | M3-02 Memory (part 1) | claude_md + memdir scan + age |
| 3 | M3-02 Memory (part 2) | find + team + secret_scan + parity fixtures |
| 4 | M3-03 API client | Extends existing anthropic.rs |
| 5 | M3-04 OAuth | Depends on M3-03's 401 hook surface |
| 6 | M3-05 Cost events + M3-06 Telemetry (part 1) | Cost events tag in week 6; telemetry schema cont. |
| 7 | M3-06 Telemetry (part 2) + M3-07 Release | NoOp/StatsigSink + verification + v0.4.0 |

Buffer: ±1 week per sub-plan. If a single-developer pace, expect 6-7 weeks. If parallel via subagent-driven-development (as M2 was), 3-4 weeks possible.

---

## §10 Open questions & deferred items

Items that came up during brainstorming but are explicitly deferred:

1. **Cross-device token sync via claude.ai** — Mentioned in Q2; user did NOT select. Stays out of M3. If needed later, would be M6 scope.
2. **Statsig HTTP endpoint wiring** — `StatsigSink` trait shipped; actual HTTP client + retry remain consumer responsibility. Could be a M6 task if a real Statsig SDK key becomes available.
3. **Embedding-based memory relevance** — Current `find_relevant` uses keyword + age + tier heuristic. claude-code may use embeddings; if so, M3-02 ships the heuristic, M3.5 or M4 can swap to embeddings.
4. **Anthropic SDK `files` / `models` / `organizations` endpoints** — Not in claude-code's usage; not in M3. If a future LingXi SDK consumer (mobile app, third-party) needs them, add in a separate plan.

These do NOT block v0.4.0 release.

---

## §11 References

- **claude-code source (read-only):** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/`
  - API client: `src/services/api/claude.ts`, `src/services/tokenEstimation.ts`
  - OAuth: `src/services/oauth/{auth-code-listener.ts, client.ts, crypto.ts, getOauthProfile.ts, index.ts}`
  - Memory: `src/memdir/{findRelevantMemories.ts, memdir.ts, memoryAge.ts, memoryScan.ts, memoryTypes.ts, paths.ts, teamMemPaths.ts, teamMemPrompts.ts}`
  - Settings: `src/schemas/hooks.ts` + scattered type definitions
  - Cost: `src/cost-tracker.ts`, `src/costHook.ts`
  - Telemetry: `tengu_*` calls throughout `src/`

- **M2 spec:** `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md`
- **M2 plans:** `docs/superpowers/plans/2026-05-23-m2-{01..07}*.md`
- **Existing LingXi crates:**
  - `lingxi-core/crates/api-client/` (M2 SSE foundation)
  - `lingxi-core/crates/anthropic-oauth/` (M1/M2 PKCE foundation)
  - `lingxi-core/crates/memory/` (M1 skeleton)
  - `lingxi-core/crates/cost/` (M1 budget/calculator/pricing/tracker/usage)
  - `lingxi-core/crates/telemetry/` (M1 bus/sink/pii/killswitch)

- **JSON-RPC spec:** https://www.jsonrpc.org/specification (M2 already locked)
- **Anthropic API docs:** https://docs.anthropic.com/api (canonical wire format)

---

## §12 Out of scope (explicit)

To prevent scope creep:

- **UI / Terminal rendering** — No Rust ink equivalent. claude-code's React UI is not being ported.
- **Mobile real-device binding** — UniFFI compile-only gates stay; real Android / iOS app testing is not part of M3.
- **Tool implementations** — All 40 concrete tools defer to M4 (BashTool, FileEditTool, GrepTool, WebFetch, AgentTool, etc).
- **Slash commands** — `/clear`, `/compact`, `/memory`, `/init`, `/resume`, `/cost` runtime defer to M5 (M3-05 provides data API for `/cost`).
- **Hooks lifecycle** — 8 event lifecycle hooks (PreToolUse, PostToolUse, UserPromptSubmit, Stop, SubagentStop, Notification, SessionStart, PreCompact) defer to M5.
- **Plugin marketplace** — discover / install / update defer to M6.
- **MCP server side** — exposing LingXi as MCP server defer to M6.
- **`/doctor` command** — M3-01 ships the data API (`effective_for`); the user-facing command defers to M6.
- **Cron user-level workflow** — agent-driven task scheduling defers to M6.
- **UniFFI API stabilization** — defers to M6.
- **Statsig HTTP endpoint** — trait shipped; HTTP integration is consumer-chosen.

---

**End of M3 design.**
