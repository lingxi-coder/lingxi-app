# LingXi Core M3 — Engine Completion Design

> **Status**: DRAFT v2 (review fixes applied)
> **Date**: 2026-05-23
> **Target version**: v0.4.0
> **Predecessor**: M2 / v0.3.0 (claude-code parity for MCP / LSP / Sandbox / FS / Swarm / SecureStorage)
> **Successors**: M4 (Tools 全集) → M5 (Agent surface + Commands + Hooks) → M6 (Plugin marketplace + MCP server + /doctor + Cron + UniFFI polish)

**v2 changelog** — fixes from the M3 design review (see commit message
for the full inventory). High-level summary:

- **C1** Memory age: drop-vs-penalty contradiction across §4 / §7 resolved.
  Age is a relevance penalty (never drops); a separate 365-day hard-drop
  threshold handles scan-time hygiene.
- **C2** `trait OAuthRefreshHook` defined in M3-03 §3, implemented by
  M3-04. The "extends not modifies" rule now holds.
- **C3** `anthropic-beta` header value inlined verbatim from claude-code
  upstream commit `6a25909` (2026-05-23) — 16 beta constants locked in
  §7 with per-provider × per-endpoint applicability.
- **C4** New "Relationship to v3 engine spec" section after §1: explicit
  inheritance table (Secret&lt;T&gt;, SandboxedCommand, PKCE, single-flight,
  PII newtypes, F-series CI gates) and conflict-resolution notes.
- **C5** §6 test pyramid restored to v3 §32 parity: loom, cargo-fuzz,
  criterion, chaos, supply-chain layer all inherited explicitly.
- **D1** Memory ranking switched to fixed-point u64 basis points — no
  f64 in the scoring path (cross-platform deterministic).
- **D2** OAuth refresh shares `refresh_lock` per v3 §16.3 with
  double-check-after-acquire; loom test verifies single-flight.
- **D3** Retry backoff adds ±20% jitter (thundering-herd mitigation).
- **D4** `count_tokens` supported-model set + Vertex restrictions captured
  in §7 betas table.
- **D5** Batches discount field reserved in `tengu_cost_recorded`
  (`is_batch_request: bool`), always `false` in M3; real 50% discount
  deferred to M4 alongside the Batch endpoint.
- **D6** All `tengu_*` payload strings typed `VerifiedClean` / `PiiTagged`
  (v3 §26.2 inherit); `tengu_event_audit` proc-macro enforces at
  compile time.
- **D7** Proactive refresh task lifecycle: handle owned by `AuthState`,
  cancelable via `Engine::shutdown`, wake interval is
  `min(remaining/2, 5 min)` to handle short-lived tokens.
- **D8** Secret scanner reuses v3 §16.5 `lingxi-secret::scanner` 30+
  gitleaks rules — no duplicate rule set in M3-02.
- **P2** Settings 4-layer order rationale spelled out; cargo deps named
  (`schemars`, `figment`); `InMemorySink` added for test-side trip
  detection; `tengu_*` evolution policy (`#[non_exhaustive]`, append-only
  names, sibling-v2 for breaking adds); parallel DAG honest (~6 weeks
  wall-clock with 3-5 claws, not 3-4); single-dev rebaselined 5-7 → 8-10
  weeks at 400-800 LOC/week sustained-Rust pace.

---

## §1 Goal & non-goals

### Goal

Close the remaining **engine-layer** parity gaps versus claude-code TypeScript, producing v0.4.0:

1. **Full Anthropic API client** — non-streaming `messages.create`, `count_tokens`, retry/rate-limit/401-refresh middleware
2. **OAuth completion** — proactive token refresh (5-min lead or remaining/2, whichever is shorter), 401-driven reactive refresh, scope upgrade flow
3. **Memory system 1:1** — CLAUDE.md hierarchy, memdir scan + age penalty (not drop) + fixed-point ranking, team memory, secret scanning (sharing v3 §16.5 gitleaks rules)
4. **Settings 4-layer merge** — env > user > project > defaults with per-field merge rules (arrays concat-dedup, objects deep-merge, scalars override). Order rationale: env wins for ops override during emergencies; user > project because user has explicitly opted into a personal config that should follow them across projects (matches claude-code behavior); both override defaults.
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

### Relationship to the v3 engine spec (`2026-05-22-lingxi-core-rust-engine-design.md`)

The v3 engine spec describes the **full target engine**; M3 takes the
post-M2 codebase (a partial implementation of v3) and ships v0.4.0 by
closing the engine-layer parity gaps.

**Inherited from v3 (NOT re-specified here, but binding):**

| Area | v3 section | M3 obligation |
|------|-----------|---------------|
| `Secret<T>` semantics | v3 §16.2 | `secrecy::SecretBox` wrapper, Zeroize on Drop, no `impl Clone`, `Arc<Secret<T>>` for sharing |
| Sandbox enforcement | v3 §24 + §4.2 | `ProcessRunner::run(&SandboxedCommand)`; OAuth PKCE loopback listener (M3-04) does NOT bypass — wrapped via `Sandbox::bypass_with_audit(reason = BackendUnavailable)` on platforms where the listener is unsandboxable |
| OAuth flow | v3 §30 | PKCE S256 + 256-bit `state` + 127.0.0.1 loopback binding + host allowlist + 5-min flow deadline; M3-04 extends with refresh + scope upgrade only |
| OAuth single-flight | v3 §16.3 / §30.3 | `refresh_lock: Arc<tokio::sync::Mutex<()>>` with double-check-after-acquire; M3-04's reactive + proactive paths share this lock |
| PII discipline | v3 §26.2 | All `tengu_*` event payloads with user-derived strings use `VerifiedClean` or `PiiTagged` newtypes (NOT bare `String`); `strip_proto_fields` runs at every general-access sink |
| CI gates | v3 §32.4 + §32.6-8 | M3 inherits supply-chain (cargo-deny/audit/vet), loom for shared state, cargo-fuzz harnesses, criterion benchmarks, parity fixture protocol, Event/Effect stability tier |
| Cost arithmetic | v3 §17 | `saturating_add` everywhere; persistence sequenced through single-writer task with monotonic `sequence` field |
| Telemetry overflow | v3 §26.3 | `OverflowPolicy` is `DropNewest` by default; killswitch latched |

**M3-specific additions on top of inherited contracts:**

- §3 sub-plan modules (Settings / Memory / API client extensions / OAuth refresh / Cost events / Tengu schema)
- §7 wire-identifier table for claude-code byte alignment
- ~200 named `tengu_*` events with locked payload structs

**Conflict resolution (M3 vs v3):**

- v3 §30 mandates `Sandbox::prepare(...)` for any sub-process; M3-04 OAuth's PKCE loopback HTTP listener runs in-process (no sub-process spawn), so the SandboxedCommand discipline does not apply to the listener itself — it applies only when M3-04 needs to invoke an external `ApiKeyHelper` script (v3 §30.2). Documented here to prevent confusion.
- v3 §32.6 parity fixture protocol takes precedence over §6's plain "parity fixture" terminology in M3; M3 parity JSONs are written to the v3 schema (input + expected + canonicalizer reference).

---

## §2 Scope decomposition: M3 → M6 milestone outline

```
v0.3.0 (now)  ──►  M3 (v0.4.0)        ──►  M4 (v0.5.0)        ──►  M5 (v0.6.0)              ──►  M6 (v0.7.0)
                   Engine completion       Tools 全集               Agent surface +              Ecosystem
                                                                   Commands + Hooks
                   8-10 weeks              8-10 weeks               5-6 weeks                    4-5 weeks
```

M3 is **single-developer, ~8-10 weeks, six sub-plans** (rebaselined from
an initial 5-7 estimate; see §9 for the per-week breakdown):

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
M3-03 API client ───► uses Settings for baseUrl/proxy + retry policy thresholds;
       │              DEFINES `trait OAuthRefreshHook` (see below) so M3-04 can
       │              register without modifying api-client
       ▼
M3-04 OAuth ────────► IMPLEMENTS `OAuthRefreshHook`; uses keychain (M2-06)
       │
       ▼
M3-05 Cost events ──► sits on api-client's response middleware; emits via telemetry bus
       │
       ▼
M3-06 Telemetry ────► authoritative event schema; M3-05's events must conform
```

This ordering allows each plan to land as a clean commit with a working verification gate. No back-references; downstream plans extend (don't modify) upstream artifacts.

**Cross-plan trait contract — `OAuthRefreshHook`** (defined in M3-03,
implemented in M3-04 — explicit so the "extends not modifies" rule holds):

```rust
// lingxi-api-client/src/middleware/oauth_hook.rs (defined in M3-03)
#[async_trait::async_trait]
pub trait OAuthRefreshHook: Send + Sync + 'static {
    /// Invoked by api-client's middleware on HTTP 401. Returns either a
    /// fresh bearer token to retry with, or an error to surface to the
    /// caller. Must be single-flight: concurrent invocations during the
    /// same expiry window collapse to one refresh (see v3 §16.3 contract).
    async fn refresh(&self, prev_token_hash: TokenHash) -> Result<BearerToken, OAuthHookError>;

    /// Optional hook for proactive refresh; api-client calls this on a
    /// timer once `register_proactive` is consumed. Default: no-op so
    /// implementations that only handle reactive 401 don't need to opt in.
    async fn proactive_refresh(&self) -> Result<(), OAuthHookError> { Ok(()) }
}

#[derive(Debug, Clone)]
pub struct BearerToken(pub Secret<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash([u8; 32]);          // SHA-256 of the in-use token

/// Engine wiring: registered once at startup; api-client holds an
/// Arc<dyn OAuthRefreshHook>. NoOp default means tests that don't care
/// about auth don't have to plumb a real implementation.
pub fn register_oauth_hook(hook: Arc<dyn OAuthRefreshHook>) -> Result<(), MiddlewareError>;
```

M3-04 ships the concrete impl (`ClaudeAiOAuthClient: OAuthRefreshHook`). The
trait surface is therefore frozen in M3-03; M3-04 cannot retroactively
require new methods.

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
  │     │     # Loader keeps ALL entries regardless of age. Age is applied
  │     │     # later as a relevance penalty (see Flow C), never as a drop.
  │     │     # Entries older than MEMORY_AGE_HARD_DROP_DAYS (365) ARE dropped
  │     │     # at scan time as a hygiene measure (stale notes from years ago).
  │     ├─► memdir::age::annotate() — attach age_days to each entry; no drop
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
        ├─► spawn proactive refresh task — owned by AuthState; its handle is
        │     stored so Engine::shutdown() can cancel it cleanly. Wakes
        │     `min(remaining_lifetime / 2, 5 min)` before expiry to handle
        │     short-lived tokens (e.g. debug tokens issued with <5 min TTL).
        │     On successful refresh the task reschedules against the new
        │     expiry; on failure (transient) it backs off and retries; on
        │     hard failure (RefreshExpired) it emits tengu_oauth_refresh_failed
        │     and exits — the next API call will surface the auth error.
        └─► register `OAuthRefreshHook` impl into api-client middleware (the
              trait surface frozen in M3-03 §3); both proactive and reactive
              paths share `refresh_lock: Arc<Mutex<()>>` per v3 §16.3,
              with double-check-after-acquire so concurrent expirations
              collapse to one HTTP call.
        [returns AuthState { proactive_task: BackgroundTaskHandle, ... }]
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
      │     # Scoring is fixed-point u64 (numerator/denominator both ×10_000),
      │     # NOT f64 — keyword Jaccard + age decay + tier weight + team boost
      │     # all multiply into the same u64 accumulator. This guarantees
      │     # cross-platform determinism (x86 vs ARM vs CI runners) so parity
      │     # fixtures don't flake on float-rounding deltas. See M3-02 phase 6.
      │     ├─► keyword overlap: Jaccard(prompt_tokens, entry_tokens) × 10_000
      │     │   (integer-arithmetic Jaccard: intersection_count * 10_000 / union_count)
      │     ├─► age penalty: weight = max(MIN_WEIGHT_BPS, 10_000 / (1 + age_blocks))
      │     │   where age_blocks = age_days / MEMORY_AGE_PENALTY_DAYS (=30);
      │     │   MIN_WEIGHT_BPS = 1_000 (10%) so very-old entries are still
      │     │   reachable when relevant. Never drops, always penalizes.
      │     ├─► tier weight: session 10_000, project 8_000, user 6_000, team 7_000
      │     │   (basis points; session > project > team > user)
      │     └─► team boost: × 12_000 / 10_000 if entry tier is team AND
      │         settings.team_mode is opted-in
      ├─► sort desc by score (Ord on u64 is total + deterministic)
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

### Test pyramid (inherits M2 pattern + v3 §32 layered gates)

```
                   ╱╲
                  ╱  ╲     Parity fixtures (~6 new JSON files, v3 §32.6 format)
                 ╱────╲    wire-byte alignment w/ claude-code
                ╱      ╲
               ╱  Chaos ╲   Chaos / fault injection (~6 cases, inherits v3 §32.4 Layer 12)
              ╱──────────╲  SecureStorage panic, FS.watch drop, HTTP 5xx burst
             ╱            ╲
            ╱     Loom     ╲  Concurrency tests for shared-state hotspots
           ╱────────────────╲ (v3 §32.7 matrix; M3 adds: OAuthRefreshHook
          ╱                  ╲ single-flight, AnalyticsBus overflow, Settings tracer)
         ╱       Fuzz         ╲ cargo-fuzz harnesses (v3 §32.4 Layer 9)
        ╱──────────────────────╲ (settings JSON parser, anthropic-beta header
       ╱                        ╲ assembly, memdir scan path, tengu payload)
      ╱         Bench            ╲ criterion budgets (v3 §32.4 Layer 10)
     ╱────────────────────────────╲ regressions > 10% fail CI
    ╱                              ╲
   ╱            Integ                ╲ Integration tests (~25 new)
  ╱──────────────────────────────────╲ cross-crate end-to-end flows
 ╱                                    ╲
╱             Contract                  ╲ Contract suites (~6 parameterized)
────────────────────────────────────────  Settings / Memory / API / OAuth / Cost / Telemetry
                Unit                       ~150+ unit tests
                                           TDD red→green per task
```

The "M3 inherits v3 §32 CI gates" rule is binding (per the Relationship
section after §1): if a gate is in v3 §32 but missing from M3 CI, the
gate is still required — this pyramid only enumerates the gates M3 is
*authoritative* for. Supply-chain layer (`cargo deny / audit / vet`) is
fully inherited; M3 does not redocument it but the v0.4.0 release CI
must run all of v3 §32.4 layers 1-12.

### Per-sub-plan test targets

| Plan | Unit | Contract | Integration | Parity fixture |
|------|------|----------|-------------|----------------|
| M3-01 Settings | merger / loader / tracer / schema / env_parser | `run_settings_contract<L: SettingsLoader>` | 4-layer end-to-end with real tempfile dirs | `parity_settings_merge.json` |
| M3-02 Memory | claude_md / memdir / age (fixed-point) / find / secret_scan (gitleaks reuse) | `run_memory_contract<M: MemoryProvider>` | CLAUDE.md hierarchy with mocked `~/.claude/` layout | `parity_memory_loading.json`, `parity_memory_relevance.json` |
| M3-03 API client | retry+jitter / rate_limit / count_tokens supported-model decoder / OAuthRefreshHook trait | extend M2's existing `run_http_contract` | axum mock: 401 → refresh → retry roundtrip | `parity_messages_create.json`, `parity_betas.json` |
| M3-04 OAuth | refresh / scope_upgrade / single-flight | `run_oauth_contract<C: OAuthClient>` | proactive task + 401 hook with mock identity provider | `parity_oauth_pkce_refresh.json` |
| M3-05 Cost events | event emission / pricing extension | (uses existing cost contract; adds 2 events tests) | full API call → cost event flow through `InMemorySink` | `parity_cost_events.json` |
| M3-06 Telemetry schema | schema validation / PII typing / NoOp sink / InMemorySink / StatsigSink trait / `tengu_event_audit` macro | `run_telemetry_contract<S: TelemetrySink>` | end-to-end emit → `InMemorySink` capture for representative events | `parity_tengu_events.json` (200+ event names + payload samples) |

### Expected test counts at v0.4.0

- Existing M2 tests: 488 (all stay green)
- New unit tests: ~150
- New contract drivers: 12 (6 contracts × 2 drivers each on average)
- New integration tests: ~25
- New parity drivers: 6 (one per fixture file)
- **New loom tests**: ~8 (OAuth single-flight, AnalyticsBus overflow, Settings tracer concurrent reads, Memory loader concurrent claude_md hierarchy walk, plus the 4 v3 §32.7 hotspots that M3 code touches)
- **New fuzz harnesses**: 4 (settings JSON parser, anthropic-beta assembly, memdir path canonicalizer, tengu payload validator) — run as 5-minute quick CI on PRs, 24-hour nightly on a dedicated runner
- **New criterion benches**: ~6 (memory ranking N×K, settings 4-layer merge, message-create middleware overhead, tengu event encode, OAuth refresh under contention, full Engine::init() startup)
- **New chaos cases**: ~6 (SecureStorage refuses, FS.watch drops events, HTTP 5xx burst across retry window, OAuth token expires mid-request, telemetry sink rejects, partial-write settings.json)
- **Total at v0.4.0: ~700 functional tests + ~24 non-functional gates**

### Cross-platform CI

Continues M2 matrix + v3 §32.4 Layer 5 musl gate (inherited):

- `x86_64-apple-darwin` ✅ build + test
- `aarch64-apple-darwin` ✅ build + test
- `x86_64-unknown-linux-gnu` ✅ build + test
- `x86_64-unknown-linux-musl` ✅ cargo check (musl gate from v3 §32.4 Layer 5)
- `x86_64-pc-windows-msvc` ✅ build + test (Windows CI runner)
- `x86_64-pc-windows-gnu` ⚠️ `cargo check` only (local host mingw-w64 not installed)

### Test discipline

- Each fileful module: **≥1 happy path test, ≥1 error path test, byte-literal assertions for every locked wire identifier**.
- Coverage percentages not enforced; the "module ≥3 test types" rule is the gate.
- Test runtime targets: `cargo test -p <crate>` < 10s (raised from 5s because parity fixtures involve tempdir setup); full workspace < 90s; with `--ignored` < 180s. Loom / fuzz / criterion run on dedicated CI jobs, not the per-PR fast path.
- **Cross-platform float determinism**: any test that depends on score arithmetic uses the fixed-point u64 path (per §4 Flow C); no test may use `f64` for parity-sensitive comparisons.

---

## §7 Wire identifiers locked (byte-alignment table)

These literals must appear byte-for-byte in code AND in at least one test assertion. Drift breaks interop with claude-code, claude.ai OAuth, and Statsig analytics.

### Settings (M3-01)

| Item | Literal | Lock site |
|------|---------|-----------|
| User settings file | `~/.claude/settings.json` | `parity_settings_merge.json` |
| Project settings file | `<repo>/.claude/settings.json` | same |
| 4-layer priority | env > user > project > defaults | rationale in §1 goal #4 |
| Env var prefix priority | `LINGXI_*` → `CLAUDE_CODE_*` → `CLAUDE_*` | same |
| Array-merge fields | `trustedDirectories`, `additionalDirectories`, `enabledTools`, `additionalIncludes` | same |
| Object-merge fields | `sandbox`, `hooks`, `outputStyle` | same |
| Schema version | `"$schema"` field is **NOT** emitted by claude-code as of upstream commit 6a25909; M3-01 does not emit it either. If claude-code adds one in a future release, M3-01 adds a compat reader at that time. |

### Memory (M3-02)

| Item | Literal |
|------|---------|
| Project memory filename | `CLAUDE.md` (case-sensitive) |
| Local override filename | `CLAUDE.local.md` |
| Memdir directory | `~/.claude/memdir/` |
| Team memory directory | `~/.claude/team-mem/` |
| Per-file size cap | 10 MB (`MAX_MEMORY_FILE_SIZE = 10 * 1024 * 1024`) |
| Age penalty block | 30 days (`MEMORY_AGE_PENALTY_DAYS = 30`) — relevance penalty unit, NOT a drop threshold |
| Hard-drop threshold | 365 days (`MEMORY_AGE_HARD_DROP_DAYS = 365`) — scan-time hygiene drop |
| Minimum age weight | 1000 bps (`MEMORY_MIN_AGE_WEIGHT_BPS = 1_000`) — even very old entries stay reachable at 10% weight |
| Default relevance k | 5 (`DEFAULT_RELEVANT_MEMORIES = 5`) |
| Scoring arithmetic | fixed-point u64 (basis points), NOT f64 — cross-platform deterministic |
| Telemetry events | `tengu_agent_memory_loaded`, `tengu_memory_secret_redacted` |

### API client (M3-03)

| Item | Literal |
|------|---------|
| Base URL | `https://api.anthropic.com` |
| Version header | `anthropic-version: 2023-06-01` |
| User-Agent | `claude-cli/<CARGO_PKG_VERSION> (external, cli)` |
| Retry budget (default) | 3 (exponential backoff: 500ms / 1s / 2s ± 20% random jitter) |
| Retry budget (streaming) | 3, same backoff, jitter required for thundering-herd protection on shared retries |
| Default timeout (streaming) | 600s (10 min) |
| Default timeout (non-stream `messages.create`) | 120s (2 min — long enough for thinking models, short enough to surface stuck connections) |
| Default timeout (`count_tokens`) | 30s |
| Rate-limit headers | `Retry-After` (seconds), `anthropic-ratelimit-requests-reset` (ISO8601) |
| Rate-limit error string | `"Rate limited; retrying in {N}s"` |

**`anthropic-beta` header (LOCKED)** — extracted from
`claude-code/src/constants/betas.ts` at upstream commit `6a25909`
(last verified 2026-05-23). M3-03 ships a `BetaHeaderRegistry` that emits
**only the headers relevant to the current request kind** (`messages.create`
non-stream, `messages.create` streaming, `count_tokens`); not all betas
apply to every endpoint. The full known-good set:

```rust
// lingxi-api-client/src/anthropic/betas.rs (M3-03)
pub const CLAUDE_CODE_BETA:        &str = "claude-code-20250219";
pub const INTERLEAVED_THINKING:    &str = "interleaved-thinking-2025-05-14";
pub const CONTEXT_1M:              &str = "context-1m-2025-08-07";
pub const CONTEXT_MANAGEMENT:      &str = "context-management-2025-06-27";
pub const STRUCTURED_OUTPUTS:      &str = "structured-outputs-2025-12-15";
pub const WEB_SEARCH:              &str = "web-search-2025-03-05";
pub const ADVANCED_TOOL_USE_1P:    &str = "advanced-tool-use-2025-11-20";   // Claude API / Foundry
pub const TOOL_SEARCH_TOOL_3P:     &str = "tool-search-tool-2025-10-19";    // Vertex / Bedrock
pub const EFFORT:                  &str = "effort-2025-11-24";
pub const TASK_BUDGETS:            &str = "task-budgets-2026-03-13";
pub const PROMPT_CACHING_SCOPE:    &str = "prompt-caching-scope-2026-01-05";
pub const FAST_MODE:               &str = "fast-mode-2026-02-01";
pub const REDACT_THINKING:         &str = "redact-thinking-2026-02-12";
pub const TOKEN_EFFICIENT_TOOLS:   &str = "token-efficient-tools-2026-03-28";
pub const ADVISOR_TOOL:            &str = "advisor-tool-2026-03-01";
pub const OAUTH:                   &str = "oauth-2025-04-20";

// Vertex `count_tokens` allows only these three (per claude-code/src/constants/betas.ts):
pub const VERTEX_COUNT_TOKENS_ALLOWED: &[&str] =
    &[CLAUDE_CODE_BETA, INTERLEAVED_THINKING, CONTEXT_MANAGEMENT];

// Bedrock requires these to ride in extraBodyParams, NOT the header:
pub const BEDROCK_EXTRA_PARAMS_HEADERS: &[&str] =
    &[INTERLEAVED_THINKING, CONTEXT_1M, TOOL_SEARCH_TOOL_3P];
```

The parity fixture `parity_messages_create.json` asserts the exact
comma-joined string emitted per provider × endpoint combination. CI fails
if any of these constants drift unless `parity_betas.json` is updated in
the same commit (file-level co-change rule enforced by a pre-commit hook).

### OAuth (M3-04)

| Item | Literal |
|------|---------|
| Authorize endpoint | `https://claude.ai/oauth/authorize` (HTTPS-pinned; host allowlist from v3 §30.1) |
| Token endpoint | `https://console.anthropic.com/v1/oauth/token` (HTTPS-pinned) |
| OAuth beta header | `anthropic-beta: oauth-2025-04-20` (from `claude-code/src/constants/oauth.ts` @ commit 6a25909) |
| Refresh grant_type | `refresh_token` |
| PKCE method | `S256` (mandated by v3 §30; inherited, not re-specced) |
| State token entropy | 256 bits, CSPRNG (v3 §30.1 `PkceFlowState`) |
| Redirect URI template | `http://127.0.0.1:{port}/callback` (loopback only — v3 §30.3) |
| Login flow deadline | 5 min (v3 §30.1 `PkceFlowState.deadline`) |
| Scopes | `read:user`, `write:messages`, `read:projects` (final list from claude-code; M3-04 verifies against `claude-code/src/constants/oauth.ts`) |
| Proactive refresh lead | `min(remaining_lifetime / 2, 5 * 60)` seconds — handles short-lived tokens (debug tokens with TTL < 5 min) |
| Single-flight lock | `refresh_lock: Arc<tokio::sync::Mutex<()>>` (v3 §16.3); concurrent reactive + proactive collapse to one HTTP refresh |
| 401 retry policy | retry ONCE after refresh; no further retries (avoids tight loop on permanently-expired refresh_token) |
| Telemetry events | `tengu_oauth_refresh_started`, `tengu_oauth_refresh_succeeded`, `tengu_oauth_refresh_failed`, `tengu_oauth_scope_upgraded`, `tengu_oauth_proactive_canceled` (Engine::shutdown clean-cancel) |

### Cost events (M3-05)

All payload-typed strings use `VerifiedClean` (per v3 §26.2); raw `String`
is rejected by the schema. `model` and `error_kind` are `&'static str`
enumerated constants, not user input.

| Event name | Payload fields (byte-aligned) |
|------------|-------------------------------|
| `tengu_cost_recorded` | `model: VerifiedClean`, `input_tokens: u64`, `output_tokens: u64`, `cache_read_input_tokens: u64`, `cache_creation_input_tokens: u64`, `cost_usd: u64` (nano-USD per v3 §17), `session_id: VerifiedClean`, `is_batch_request: bool` (reserved for M4; always false in M3 since batches endpoint is M4) |
| `tengu_cost_budget_warning` | `limit_usd: u64`, `current_usd: u64`, `percent_bps: u32` (basis points; fixed-point per §4 Flow C float-determinism rule) |
| `tengu_cost_budget_exceeded` | `limit_usd: u64`, `current_usd: u64` |
| `tengu_api_request_started` | `model: VerifiedClean`, `request_id: VerifiedClean`, `stream: bool` |
| `tengu_api_request_succeeded` | `model: VerifiedClean`, `request_id: VerifiedClean`, `duration_ms: u64`, `status: u16` |
| `tengu_api_request_failed` | `model: VerifiedClean`, `request_id: VerifiedClean`, `error_kind: VerifiedClean`, `status_code: Option<u16>` |
| `tengu_api_rate_limited` | `model: VerifiedClean`, `retry_after_ms: u64` |

**Batches discount status**: the `tengu_cost_recorded` payload includes
`is_batch_request: bool` for forward compatibility with M4's Batch API
implementation. In M3, this field is **always `false`** because the
`/v1/messages/batches` endpoint is not yet wired. The cost pricing path
does NOT apply a 50% batch discount in M3; that arrives in M4 alongside
the endpoint itself. Parity fixture `parity_cost_events.json` asserts
the field is present and `false` for every M3-recorded API call.

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

Each event is a `#[serde(deny_unknown_fields)]` struct in
`telemetry::tengu::<category>`. Statsig-compatible serialization is
`{event_name, value, metadata}`.

**PII discipline (v3 §26.2 inheritance)**: every payload string with
user-derived content is typed `VerifiedClean` or `PiiTagged`; bare
`String` in a payload struct is rejected by the schema validation in
M3-06 phase 11. The `strip_proto_fields` filter runs at every
general-access sink before serialization, dropping `_PROTO_*`-prefixed
keys; `PiiTagged` values under non-proto keys hit a `debug_assert!` and
are silently dropped in release (per v3 §26.2 belt-and-braces rule).

**Event evolution policy** (forestalls `deny_unknown_fields` lock-in):

- Adding a new field to an *existing* event is a breaking change.
  Instead, define a sibling event `tengu_<name>_v2` with the new field
  set and deprecate `tengu_<name>` over 2 minor releases.
- All event payload enums are `#[non_exhaustive]`, so adding a new
  variant is backward-compatible.
- The `tengu_*` event name list is **append-only**: removing a name is
  a v0.5+ breaking change requiring a deprecation cycle entry in
  CHANGELOG.md.
- A proc-macro (`tengu_event_audit`) ships in M3-06 phase 11 to walk
  the `telemetry::tengu` module tree and fail compilation if a struct
  uses bare `String` or removes a field.

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
1. **Cargo deps** — pin `serde`, `serde_json`, `schemars` (for §3.1 SettingsJson Schema), `figment` (rejected: `config` does not support per-field merge strategies; `twelf` over-engineered for our needs); `thiserror` already in tree. The new direct deps are `schemars = "0.8"` and `figment = { version = "0.10", features = ["env", "json", "toml"] }` — but figment is only used as a source-loader pattern reference; we write our own merger because no crate supports our concat-dedup / deep-merge field metadata.
2. `schema.rs` — `SettingsJson` type + per-field merge metadata (`#[merge(strategy = "concat-dedup")]`) + schema validation (rejects unknown fields, type-checks scalars). New fields are added as `Option<T>` with explicit defaults to keep `serde(deny_unknown_fields)` backward-compatible (matches v3 Event/Effect stability policy).
3. `env_parser.rs` — walk `LINGXI_*` / `CLAUDE_CODE_*` / `CLAUDE_*` with priority
4. `loader.rs` — 4-layer source loader with fallback chain
5. `merger.rs` — per-field merge dispatcher (array / object / scalar)
6. `tracer.rs` — provenance recorder (passive observer)
7. Telemetry events emitted on settings load: `tengu_settings_loaded`, `tengu_settings_invalid_env`, `tengu_settings_parse_error`
8. Integration test — full 4-layer end-to-end with tempfile dirs
9. Parity fixture `parity_settings_merge.json` (v3 §32.6 format: input dirs + env vars + defaults snapshot, expected `EffectiveSettings` + provenance trace)
10. Workspace verification + commit

**Acceptance:** `cargo test -p lingxi-core --test settings_*` all pass; `effective_for(field)` returns provenance trace; clippy + fmt clean.

**Tag:** `m3.1`

### M3-02 — Memory (CLAUDE.md hierarchy + memdir + ranking + team + secret-scan)

**Phases** (estimated ~18 bite-sized TDD tasks when expanded by writing-plans):
1. `claude_md/hierarchy.rs` — dir-up walker (cwd → parents → user). Cross-platform case normalization: on macOS APFS / Windows NTFS the OS may return `claude.md`; the loader matches case-insensitively but emits a warning event (`tengu_memory_case_mismatch`) so users can spot misnamed files.
2. `claude_md/loader.rs` — file reader with 10MB size cap
3. `memdir/paths.rs` — path resolution (`~/.claude/memdir/`)
4. `memdir/scan.rs` — directory enumeration; drops entries older than `MEMORY_AGE_HARD_DROP_DAYS` (365) as scan-time hygiene
5. `memdir/age.rs` — fixed-point u64 age weight (no f64). Public API: `fn age_weight_bps(age_days: u64) -> u32`
6. `memdir/find.rs` — relevance ranking using **fixed-point u64 basis points** (see §4 Flow C). Float arithmetic is forbidden in this module; clippy denies `clippy::float_arithmetic`
7. `memdir/team_paths.rs` + `team_prompts.rs` — `settings.team_mode` triggers; team_mode is opted-in via `settings.team_memory.enabled` (a bool defaulting to `false`); auto-detection from filesystem presence is intentionally NOT used to avoid surprising the user
8. `secret_scan.rs` — replaces M2 stub. Inherits v3 §16.5 `SecretScanner` with the same 30+ gitleaks rule set (`rules/gitleaks/*.yaml` shipped in `lingxi-secret`); M3-02 calls into `lingxi-secret::scanner::scan_text(...)` instead of duplicating rules
9. Telemetry events: `tengu_agent_memory_loaded`, `tengu_memory_secret_redacted`, `tengu_memory_file_too_large`, `tengu_memory_case_mismatch`
10. Integration test — full hierarchy load with mocked dirs
11. Parity fixtures `parity_memory_loading.json` + `parity_memory_relevance.json` (v3 §32.6 format)
12. Workspace verification + commit

**Acceptance:** `cargo test -p lingxi-memory --test memdir_*` all pass; ranking is byte-identical across x86_64 and aarch64 CI runners (fixed-point arithmetic, no f64 in the scoring path); clippy + fmt clean.

**Tag:** `m3.2`

### M3-03 — API client (non-stream + retry + rate-limit + count_tokens)

**Phases** (estimated ~13 bite-sized TDD tasks when expanded by writing-plans):
1. **Define `trait OAuthRefreshHook`** (frozen in M3-03; implemented by M3-04). See §3 cross-plan trait contract block. Ships with `NoOpOAuthHook` default so M3-03 lands without M3-04 needing to exist yet.
2. `anthropic.rs` extension — `messages_create(model, msgs)` non-streaming
3. `anthropic.rs` extension — `count_tokens(model, msgs)` with supported-model check (see §7 wire-id table)
4. `retry.rs` — exp backoff middleware (3 retries: 500ms / 1s / 2s) with ±20% random jitter
5. `rate_limit.rs` — parse `Retry-After` + `anthropic-ratelimit-*` headers; sleep + retry
6. Beta-header registration (inlines `anthropic-beta` literal from §7)
7. User-Agent format (`claude-cli/<version> (external, cli)`)
8. Telemetry events: `tengu_api_request_started`, `tengu_api_request_succeeded`, `tengu_api_request_failed`, `tengu_api_rate_limited`
9. Integration test — axum mock with 401 + 429 + 5xx + 200 path coverage
10. Loom test — 401-hook + proactive-hook concurrent invocation collapses to one refresh call (verifies single-flight contract)
11. Parity fixture `parity_messages_create.json` (v3 §32.6 format)
12. Workspace verification + commit

**Acceptance:** Mock 401 → triggers OAuth hook → retries once → succeeds; 429 with Retry-After → sleeps → succeeds; 5xx × 3 → fails with `RetryExhausted`; clippy + fmt clean.

**Tag:** `m3.3`

### M3-04 — OAuth (reactive + proactive refresh + scope upgrade)

**Phases** (estimated ~12 bite-sized TDD tasks when expanded by writing-plans):
1. `refresh.rs` — implement `OAuthRefreshHook` (trait defined in M3-03 §3); reactive path returns `Future<Result<BearerToken>>`. Inherits v3 §16.3 `refresh_lock` single-flight pattern with double-check-after-acquire.
2. `refresh.rs` — proactive refresh task spawned via `RuntimeSpawner`; lifetime owned by `AuthState` (cancel handle stored); wake interval `min(remaining_lifetime / 2, 5 min)`. On token rotation, the task self-reschedules against the new expiry.
3. Token rotation: atomic SecureStorage update under `refresh_lock`. Reactive and proactive paths share the same lock; concurrent expirations collapse into one HTTP refresh.
4. `scope_upgrade.rs` — HTTP 403 with `required_scopes` body re-triggers PKCE preserving `refresh_token`. Sequence: keep current `refresh_token` in memory, run new PKCE flow (per v3 §30.3 — fresh `code_verifier` + `state`, loopback 127.0.0.1:0), on success replace `access_token` + new `refresh_token` atomically, fall back to old `refresh_token` on PKCE failure.
5. Engine::shutdown integration: `AuthState::shutdown()` cancels the proactive task and emits `tengu_oauth_proactive_canceled`. Idempotent.
6. Telemetry events (5 events listed in §7)
7. Integration test — proactive + reactive collision handling
8. **Loom test** — `OAuthRefreshHook::refresh` under concurrent invocation: exactly one HTTP call, all callers see the same new token (single-flight invariant from v3 §16.3 / §32.7)
9. Integration test — scope upgrade preserves session state if PKCE succeeds; falls back cleanly if PKCE fails
10. Parity fixture `parity_oauth_pkce_refresh.json` (v3 §32.6 format)
11. Workspace verification + commit

**Acceptance:** Refresh atomically updates keychain; concurrent 401 + proactive don't double-refresh (loom-verified); proactive task is cleanly canceled on shutdown; short-lived (<5 min) tokens trigger refresh at `remaining/2` not at fixed 5 min; clippy + fmt clean.

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

**Phases** (estimated ~14 bite-sized TDD tasks when expanded by writing-plans):
1. `tengu/api.rs` — ~25 events with byte-aligned payload structs; all string fields are `VerifiedClean` or `PiiTagged` (v3 §26.2 inherit)
2. `tengu/agent.rs` — ~30 events (skeletons; M5 fills payload values)
3. `tengu/session.rs` — ~15 events (skeletons)
4. `tengu/tool.rs` — ~40 events (skeletons; M4 fills payload values)
5. `tengu/cost.rs` — ~10 events (used by M3-05)
6. `tengu/oauth.rs` — ~8 events (used by M3-04)
7. `tengu/memory.rs` — ~12 events (used by M3-02)
8. `tengu/settings.rs` — ~5 events (used by M3-01)
9. `sinks/noop.rs` — default sink; logs via tracing, no network
10. `sinks/in_memory.rs` — `InMemorySink` for integration & contract tests (collects events into a `Vec<RecordedEvent>` so tests can assert "this event fired with this payload"). Without this, the "end-to-end emit → sink trip" tests in §6 cannot work — NoOp by definition doesn't trip
11. `sinks/statsig.rs` — `StatsigSink` trait; sample impl with placeholder HTTP client (no SDK key wired). Trait surface:
    ```rust
    #[async_trait]
    pub trait StatsigSink: AnalyticsSink {
        fn sdk_key(&self) -> &Secret<String>;
        async fn flush(&self) -> Result<(), TelemetryError>;
        async fn shutdown(&self) -> Result<(), TelemetryError>;
    }
    ```
    Consumer responsibility: implement `AnalyticsSink::log_event` + `StatsigSink` trait against their preferred HTTP client; M3-06 ships only a `MockStatsigSink` for tests.
12. **Proc-macro `tengu_event_audit`** — walks the `telemetry::tengu` module tree at compile time and rejects: bare `String` in payload structs, removed fields, missing `#[serde(deny_unknown_fields)]`, non-`#[non_exhaustive]` payload enums. Locks the evolution policy.
13. Parity fixture `parity_tengu_events.json` (full event name list + sample payloads, v3 §32.6 format)
14. Workspace verification + commit

**Acceptance:** All event structs serialize with `#[serde(deny_unknown_fields)]`; all string fields are `VerifiedClean`/`PiiTagged` (no bare `String`); NoOp sink is default; InMemorySink trips correctly in tests; StatsigSink trait compiles + has skeleton impl; `tengu_event_audit` proc-macro rejects schema regressions; clippy + fmt clean.

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

**Single-developer rebaseline: 8-10 weeks** (was 5-7 in the original draft;
adjusted for ~7600 total LOC at the 400-800 LOC/week sustained-Rust pace
flagged in the v3 self-review).

| Week | Sub-plan | Notes |
|------|---------|-------|
| 1 | M3-01 Settings | Pure data + merging; no I/O dependencies |
| 2 | M3-02 Memory (part 1) | claude_md hierarchy + memdir scan + fixed-point age weight |
| 3 | M3-02 Memory (part 2) | find_relevant + team + gitleaks scan reuse + parity fixtures |
| 4 | M3-03 API client (part 1) | OAuthRefreshHook trait + messages.create + count_tokens |
| 5 | M3-03 API client (part 2) | retry+jitter + rate_limit + loom test + parity fixture |
| 6 | M3-04 OAuth | Implements OAuthRefreshHook; scope upgrade; proactive lifecycle |
| 7 | M3-05 Cost events + M3-06 Telemetry (part 1) | Cost events emit; tengu schema + PII newtypes + audit proc-macro |
| 8 | M3-06 Telemetry (part 2) | NoOp / InMemorySink / StatsigSink + 200 events + parity fixture |
| 9 | M3-07 Release prep | Loom / fuzz / criterion / cargo-deny CI gate wiring; chaos tests |
| 10 | M3-07 Release | Verification + cross-compile + tag v0.4.0 |

Buffer: ±1 week per sub-plan inside the 10-week plan; if everything goes
right an 8-week ship is possible.

### Parallel DAG (only true independence shown)

The "3-4 weeks if parallel" claim from the prior draft was wishful. The
real DAG of independent work-units that can run concurrently:

```
                           ┌─ M3-01 Settings ─┐
                           │                  │
       ┌──── M3-06 Telemetry (api/agent/session/tool/cost/oauth/memory/settings
       │      schemas can be filled in parallel by 4 subagents) ─────────┐
       │                                                                 │
       ▼                                                                 ▼
M3-02 Memory (3 sub-modules can parallelize after `paths.rs` lands;
   `find_relevant` parallel with `secret_scan.rs` because they touch
   disjoint code).
                           │
                           ▼
           M3-03 API client (the OAuthRefreshHook trait phase is
           a hard serialization point; the rest of M3-03 can run
           after the trait lands)
                           │
                           ▼
           M3-04 OAuth (must follow M3-03 trait phase)
                           │
                           ▼
           M3-05 Cost events (must follow M3-03 middleware)
                           │
                           ▼
                 M3-07 Release verification
```

With 3-5 well-coordinated subagents (M2-style), the **earliest** v0.4.0
tag is ~6 weeks wall-clock, not 3-4. The schema/event work in M3-06 is
the most parallelizable; the API client + OAuth chain is the critical
path and cannot be parallelized below 4 weeks.

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
