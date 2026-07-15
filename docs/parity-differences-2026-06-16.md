# LingXi-Next vs Claude Code — Differences Report

**Date:** 2026-06-16
**Reference:** `claude-code/` TypeScript source @ `6a25909` (2026-04-22) — 519k LOC TS/TSX
**Port:** `lingxi-code/` Rust workspace @ `main` (`db94598d`) — 354k LOC Rust, 86 crates
**Method:** 8 parallel deep-comparison agents (one per subsystem cluster), each citing `ts_file:line` ↔ `rust_file:line`. The 5 highest-impact claims were re-verified first-hand (see §0).

> **Bottom line.** The *single-process agent* — turn loop, tools, slash commands, hooks, compaction, MCP core, CLAUDE.md, cost math, migrations — is at **strong 1:1 parity**. The real differences cluster in four places: (A) **cloud/remote/web subsystems that were never ported**, (B) **three safety/scheduling subsystems that are fully built but inert by default**, (C) **a handful of model-capability gaps** (prompt caching, skill discovery, budget caps), and (D) **a thinner multi-agent coordinator surface**. Plus many small, mostly-cosmetic tool-fidelity divergences.

---

## §0. Verified-first-hand findings (highest impact)

| # | Claim | Evidence | Status |
|---|---|---|---|
| 1 | Permission enforcement is **opt-in**; default gate allows everything | `apps/engine-desktop/src/lib.rs:1597` gates the real `PolicyPermissionGate` behind `LINGXI_ENFORCE_PERMISSIONS`; `else { perms }` binds `NoOpPermissionGate` (`lib.rs:1508`) | ✔ confirmed |
| 2 | Desktop engine **never sandboxes** bash | `sandbox_available: false` (`lib.rs:1993`) + `SandboxRuntimeConfig::default()` (`lib.rs:1968`) → `should_use_sandbox` returns `NoSandbox` | ✔ confirmed |
| 3 | **Cron jobs never fire** | No `CronScheduler::new`/`.start()` callsite outside the `cron` crate's own tests | ✔ confirmed |
| 4 | Model **cannot discover skills** | No `skill_listing` system-reminder; `orchestrator/src/prompt/` has no skill module | ✔ confirmed |
| 5 | **No prompt-cache breakpoints** emitted → every request misses Anthropic's cache | Zero `cache_control`/`ephemeral` in `provider_adapter.rs` request builder | ✔ confirmed |

---

## §A. Whole subsystems ABSENT in the Rust port

These are the largest gaps — entire feature areas with no Rust substrate.

### Cloud / remote / web connectivity (4 subsystems, all missing)
- **Cloud CCR remote-session client** (`src/remote/`: `RemoteSessionManager.ts`, `SessionsWebSocket.ts`, `sdkMessageAdapter.ts`, `remotePermissionBridge.ts`) — attaching the local REPL to a cloud session over `wss://api.anthropic.com/v1/sessions/ws/{id}/subscribe`, control-request permission relay, SDK-message→REPL adapter. **No Rust equivalent.**
- **Direct-connect + local web/PTY terminal server** (`src/server/`: `directConnectManager.ts`, `createDirectConnectSession.ts`, `web/pty-server.ts`, browser xterm UI, session store) — self-hosted multi-session server with a browser terminal. **No Rust equivalent.** The Rust CLI has only `Print` and `StdioRepl` modes — no `connect` subcommand.
- **CCR remote-control "bridge" worker + daemon** (`src/bridge/` ~500 KB, `src/daemon/`) — `claude remote-control` registers the host as a cloud-drivable worker. **No `remote-control`/`daemon` subcommand exists in the Rust CLI.**
- **MCP OAuth for remote servers** (`services/mcp/auth.ts`, 2466 lines: dynamic client registration, PKCE, `.well-known` discovery, local callback server, token refresh, XAA cross-app-access) — Rust `mcp/src/oauth.rs` is a **state-enum skeleton** never constructed; SSE/HTTP connectors take only a static token.

> ⚠️ **Naming trap:** the Rust `bridge/` crate is **not** a port of `src/bridge/`. It's a homegrown *remote-drive* protocol (mobile/remote client → desktop engine over a custom WS envelope) that reuses only the IDE lockfile handshake (`X-Claude-Code-Ide-Authorization` + `mcp` subprotocol). It carries `client_protocol` DTOs, not MCP JSON-RPC, so it cannot serve a real MCP IDE client.

### Plugins (entire subsystem is an unwired orphan)
- No crate depends on `plugin/`; `PluginManager::new` is never called. TS loads plugins at bootstrap (`loadAllPlugins` + plugin commands/agents/hooks/MCP/LSP). Rust has **no startup discovery**.
- `MarketplaceManager` is an empty struct; `PluginManager::install` returns `Err("install impl in Plan 16")`.
- Plugin-provided commands/skills/hooks/output-styles/MCP are not materialized even via the unreachable load path (only LSP registration has real-but-dead logic).
- `/plugin` and `/reload-plugins` are interactive-only / host-bound stubs.
- *Present but dead:* the register/unregister plumbing, trust levels, blocklist, lifecycle state machine are all implemented — waiting on a `PluginManager` consumer.

### Skills discovery (inline exec works; discovery doesn't)
- **`skill_listing` system-reminder not injected** (verified §0.4) — TS tells the model "The following skills are available…" with a 1%-of-context budget. Rust never injects it, so **the model cannot autonomously discover or invoke skills**. `SkillTool::prompt` is a static one-liner vs TS's dynamic catalog.
- **Zero bundled skills** — TS ships 17 (`batch`, `debug`, `loop`, `remember`, `simplify`, `verify`, …); `skills/builtin` is `&[]`.
- **Forked-agent skill execution** (`context: fork`) runs inline instead of as a subagent.
- **MCP-skill + remote canonical-skill discovery** not ported.
- `skill-api::SkillFrontmatter` is a **divergent invented schema** (`when_to_use`/`auto_search`/`triggers`) that drops the real TS fields (`model`/`disable-model-invocation`/`user-invocable`/`argument-hint`/`effort`/`hooks`/`paths`). (The real fields *are* handled via a separate `SkillDescriptor`/`CommandRegistry` bridge — skills-as-commands work — but the `skill-api` type is a parallel listing-only struct.)

### UX features (3 missing)
- **Buddy companion sprite** (`src/buddy/`) — the ASCII "duck" virtual pet (18 species, idle animation, per-turn reactions, `/buddy` teaser). Gimmick; low importance. **Absent.**
- **Voice mode** (`src/voice/`, `/voice`) — hold-to-talk dictation (OAuth + SoX + streaming STT). Real capability; **absent.**
- **User-customizable keybindings** (`~/.claude/keybindings.json`) — loader/parser/chords/contexts/hot-reload. Rust keymap is a hardcoded `match`; `/keybindings` only writes the template and opens `$EDITOR`, never loads it (the Rust help screen documents this gap). **This is the one functionally significant UX gap.**
- *(`moreright` is internal-only even in the reference — a no-op stub — so there's nothing to port. `native-ts/yoga-layout` is N/A: Rust uses `iocraft`/`taffy`. `native-ts/color-diff` IS ported via `syntect`+`similar`.)*

---

## §B. Built-but-inert: safety & scheduling disabled by default

These engines are **faithfully ported and unit-tested**, but **not activated at the desktop composition root**. This is partly a deliberate posture decision (the parity log notes flipping `LINGXI_ENFORCE_PERMISSIONS` on is "a separate risky behavior change"), but it is a genuine behavioral difference: claude-code gates and sandboxes **by default**.

- **Permissions** (verified §0.1) — without `LINGXI_ENFORCE_PERMISSIONS`, every tool runs through `NoOpPermissionGate` (allow-all). Deny/ask rules, plan-mode backstop, and the bypass killswitch are all dormant. When the env var *is* set, the gate is faithful.
- **Sandbox** (verified §0.2) — `sandbox_available: false` + default config means the Bash tool spawns **unsandboxed on desktop, always**. The macOS SBPL generator, Linux bwrap builder, and the 1:1 `sandbox-runtime` port are all dead code on the live path.
- **Cron** (verified §0.3) — the three cron tools persist descriptors to disk, but the `CronScheduler` that would run them is never constructed/started, and never loads the persisted jobs. **A created cron job never executes.** (It also has a correctness bug — see §G.)

> **Combined effect:** by default the desktop engine runs tools with **neither a permission gate nor a sandbox**. This is the most security-relevant difference from claude-code.

---

## §C. Model-capability / turn-loop divergences

- **No prompt caching** (verified §0.5) — TS sets `cache_control: ephemeral` on system/tools/last-blocks; Rust sets none. Every request misses the Anthropic prompt cache — a cost & latency regression (not a correctness bug). *(Tracked in the parity log as the large "CACHE.1/2" item.)*
- **`maxBudgetUsd` cost cap absent** — a `--max-budget` headless run won't stop in Rust.
- **Structured-output retry limit absent** — no `MAX_STRUCTURED_OUTPUT_RETRIES` guard.
- **`maxTurns` is a hardcoded always-on cap of 30** (`error: MaxTurnsReached`), where TS leaves it unbounded in the interactive REPL — a legitimate >30-turn loop hard-errors in Rust.
- **PostSampling hooks** and **tool-use summaries** (Haiku side-summaries) not fired (both off-by-default / internal in TS — low impact).
- **API `task_budget` (output_config beta) param** not wired into requests (distinct from the ported `+500k` auto-continue feature, which *is* faithful).
- **Per-iteration `prependUserContext`/`appendSystemContext`** — Rust bakes the env/git/file-tree/memory block into the system prompt **once** at turn start; TS re-prepends a fresh env user-message every loop iteration (also a cache-segmentation difference).
- **Streaming-path recovery is thinner** than the batched path: max-tokens 8k→64k escalation and full 413/PTL truncate-loop recovery exist only on the batched path (documented intra-port divergence; the escalation itself is default-off).

---

## §D. Multi-agent coordinator surface is thinner

The task **todo-list** tools (TaskCreate/Get/List/Update/Stop) and standalone tools (RemoteTrigger, Brief) are at **strong parity**. The multi-agent *coordinator* is substantially reduced:

- **AgentTool is an explicitly-stubbed spawn surface** — advertises only `subagent_type`/`prompt`/`context_paths` (and `context_paths` isn't a TS field). Missing: the required `description`, `model` override, `run_in_background`, `name`/`team_name`/`mode`, `isolation`, `cwd`. No sync-vs-async-launch output. Subagent types are hardcoded to 6 built-ins — no user/project-loaded agents, no fork-subagent path. Prompt is static (model never sees the agent/MCP catalog).
- **SendMessage** — no broadcast (`*`), no shutdown-request/response, no plan-approval routing; no name-registry lookup, pending-message queueing, auto-resume, UDS, or `bridge:` cross-session delivery. The schema still advertises these, overstating the surface.
- **TeamCreate/TeamDelete** — drop the one-team-per-leader guard, unique-name-on-collision, disk team-file, active-member guard, and `tengu_team_*` telemetry; operate on an in-memory registry instead.
- **Coordinator mode** is a bare `AtomicBool` — no `getCoordinatorSystemPrompt`/`getCoordinatorUserContext`/`matchSessionMode` resume reconciliation.
- **Swarm-conditional TaskUpdate/TaskCreate side-effects** (auto-owner on `in_progress`, owner-change mailbox notification, TaskCreated/TaskCompleted blocking hooks) are unimplemented on the V2 tool path (explicitly marked `PARITY-GAP`). *(The background-task registry path does fire these hooks.)*

---

## §E. Tool-fidelity divergences (small, but real)

**Systemic:** every File tool's `description`/`prompt` is a short Rust stub (Grep is the lone byte-faithful exception); every File/Glob/Grep `check_permissions` is an allow-all `M4-01` stub (consistent with §B).

- **Bash** — param renamed `timeout`→`timeout_ms` (a model sending `timeout` is silently ignored); truncation message is `[Output truncated due to length]` (no line count); no image-output handling, no `interrupted` flag; `bashSecurity.ts` banned-command list + `sleep N≥2` block not on the live path (the deep injection battery `bashCommandIsSafe` *is* ported in `permission`, but only fires in enforce mode).
- **Glob** — `globset`+`walkdir` vs `rg --files`: `*.rs` matches root-only (no implied `**/`); `CLAUDE_CODE_GLOB_NO_IGNORE`/hidden-file env toggles unsupported.
- **Grep** — in-process `grep`/`ignore` crates vs `rg`: extra `GREP_PER_FILE_CAP=100` (drops matches/undercounts), `--max-columns 500` hard-truncates with no marker, no search timeout, file-read ignore-patterns not enforced.
- **WebFetch** — cross-host redirect detection not wired into the live path (the "REDIRECT DETECTED" message never fires at runtime); diverging size cap (5 MB body vs TS 100 KB markdown/10 MB transfer); `is_preapproved_domain` hardcoded `false` (always STRICT guidelines).
- **WebSearch** — single blocking POST instead of streaming (progress events lost); `isEnabled` provider gating not ported (always enabled).
- **FileWrite** — adds a non-reference `mkdir` flag and **rejects** a missing parent dir; TS auto-creates parents unconditionally.
- **FileRead** — different too-large message (omits offset/limit guidance); binary detection by **NUL-scan** vs TS **extension list**; no `MAX_LINES_TO_READ=2000` default cap; notebook byte-size cap + screenshot/ENOENT-suggestion paths not ported.
- **NotebookEdit** — no `.ipynb` extension validation; parse failures return a hard error vs TS's soft `error` field.

---

## §F. Memory / CLAUDE.md

- **`@import` engine is a near-exact port** (depth cap 5, cycle guard, external gate, splice order — all tested). ✔
- **Managed/enterprise CLAUDE.md tier not loaded** (intentionally skipped).
- **Conditional `paths:`-gated rules not loaded** (frontmatter discarded). Unconditional `.claude/rules/**.md` *is* ported.
- **Injection format diverges** — Rust `<memory># {path}` vs TS `MEMORY_INSTRUCTION_PROMPT` preamble + `Contents of {path} ({description}):`.
- **Invented 10 MB hard cap** (drops oversized files); **missing the 40 000-char warning**.
- **`memdir/` is a structurally different subsystem** — Rust = `~/.claude/memdir/` + numeric Jaccard×age ranker; TS = project `memory/` + `MEMORY.md` with a Sonnet selector. `claudeMdExcludes`, `--add-dir` CLAUDE.md, nested-worktree dedup not ported.

---

## §G. Session / cost / cron divergences

- **Session resume requires a strict linear parent chain** (`validate_chain`), and the reader requires `uuid`/`sessionId`/`timestamp`/`message` on *every* line → **Rust cannot load a real claude-code transcript** containing `summary`/`mode`/`custom-title` metadata lines. It round-trips its own files fine. TS does a branch-tolerant leaf→root DAG walk.
- **~17 of ~18 metadata `Entry` line types not ported** (only `agent-color`). No `summary`→leaf linking for the resume picker; no reactive AppState write-backs.
- **`gitBranch` always `None`** on write (also drops `entrypoint`/`slug`/`promptId`/`logicalParentUuid`).
- **Cost:** `tengu_cost_recorded` never fires from the live path (bus is `None`); `/cost` is a one-liner vs TS's per-model breakdown; no lines-added/removed accounting, no session-cost persist/restore-on-resume, no advisor recursive billing. *(Cost pricing math itself — per-model rates, cache tiers, Opus-4.6 fast tier — is faithful.)*
- **Cron correctness bug:** `decompose` uses fixed 30-day months, so day-of-month 31 can **never** match; month drifts off the real calendar; UTC-only; ANDs DOM/DOW where TS ORs them. Plus missing auto-expiry, missed-task catch-up, kill-switches, teammate scoping; persistence path (`~/.claude/cron/<id>.json`) differs from the path the result text claims.

---

## §H. Output styles

- **Builtins (`Explanatory`/`Learning`) + system-prompt injection + per-turn reminder are at parity.** ✔
- **Custom disk discovery** (`~/.claude/output-styles/`, `.claude/output-styles/`) **missing** — only the two compiled-in builtins can ever be active.
- **Plugin output styles** stubbed (no-op load path).
- **`keepCodingInstructions` gating inert** (field carried, never read).

---

## What IS at parity (so the picture is balanced)

- **Tool set:** all 40 reference tools have Rust modules (1:1 at the surface).
- **Slash commands:** all **99** runtime command names registered (35 fully implemented, 16 interactive-only with headless fallback, 23 correct-by-design stubs matching TS's own gated stubs, 25 host/remote-deferred). **Zero genuinely missing.**
- **Hooks:** 22/26 events fire with faithful matcher/decision-parsing/aggregation; all 4 executor transports (command/http/agent/prompt) + async backgrounding present. *(Gaps: `Setup`, `ElicitationResult`, `WorktreeRemove`, `PostSampling` have no firing site; `once` flag parsed-but-unhonored.)*
- **Compaction:** thresholds, buffers, layer order (snip→micro→auto), consecutive-failure circuit breaker, and the `+500k` token-budget state machine — all byte-faithful.
- **Migrations:** the runner + `CURRENT_MIGRATION_VERSION=11` + 9 live `migrateX` are at parity; the 2 unported ones are ant-only dead code (correct omission).
- **MCP core:** initialize `2025-11-25`, stdio/SSE/streamable-HTTP transports, tools/list+call, prompts/list+get, resources/list+read, progress notifications, `_meta` passthrough, reconnect/backoff, env-var expansion — faithful.
- **Permission engine (when enforced) & sandbox profile generation** — faithful in isolation (just inert by default).
- **TUI:** REPL loop, resume picker, vim core (h/j/k/l, w/b/e, operators, counts, registers — Rust even adds Visual mode), syntect+similar word-diffs, @-file completion, slash-command palette.

---

## Suggested priorities (if closing gaps)

**Functional, high-value, unblocked:**
1. **Prompt caching** (§C) — pure cost/latency win; set `cache_control` breakpoints in `build_request`.
2. **Skill listing reminder** (§A) — without it the model can't use skills; inject `getSkillToolCommands` equivalent.
3. **Flip permissions/sandbox on by default** (§B) — security posture; needs parity fixtures re-checked.
4. **Cron scheduler wiring + month/day matcher fix** (§B/§G) — currently jobs silently never run.
5. **User-customizable keybindings** (§A) — the one significant UX gap.

**Larger / needs product decision:**
6. Plugins subsystem activation (§A) — large; orphan crate ready.
7. Cloud/remote/web subsystems (§A) — only if remote-drive is a product goal.
8. Multi-agent coordinator depth (§D) — AgentTool params, SendMessage routing, coordinator prompt.

**Correctness worth fixing regardless:**
9. Session resume tolerance (§G) — can't load real claude-code transcripts today.
10. `maxTurns` hardcoded-30 hard-error (§C).
