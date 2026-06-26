# LingXi Namespace Rebrand — Design Spec

- **Date:** 2026-06-26
- **Status:** Approved (design); pending spec review → implementation plan
- **Scope tree:** `lingxi-code/` (Rust workspace) + `clients/` (android/ios/electron/shared). Vendored reference/oracle trees (`claude-code/`, `claw-code*/`, `codex/`, `opencode/`, `liter-llm/`, `third_party/`) are **out of scope**.

## 1. Context & goal

LingXi-Next is a from-scratch reimplementation that has been kept at byte-for-byte
parity with Claude Code. Because LingXi is a distinct product, the Claude-branded
**namespace** must become LingXi's own so it never collides with — or reads from —
a real Claude Code install on the same machine.

**Goal.** Replace the Claude-branded namespace with LingXi equivalents as a
**clean break**:

- on-disk paths (`~/.claude`, project `.claude/`, `~/.claude.json`, worktrees, transcripts, …),
- environment variables (`CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_*`, `CLAUDE_*` → `LINGXI_*`),
- well-known filenames (`CLAUDE.md`/`CLAUDE.local.md` → `LINGXI.md`/`LINGXI.local.md`),
- user-facing branding strings (`Claude Code` → `LingXi`, including system-prompt self-identity),
- internal Rust symbols (`claude_md`, `DOT_CLAUDE`, `ClaudeMdTier`, `claude_home*`, …).

**Non-goal.** The Anthropic **protocol layer** stays byte-identical — renaming it
would break backend communication. See §5 (denylist).

**No data migration.** Clean break means existing `.claude` data is ignored; users
re-authenticate and lose resume history (see §6).

## 2. Decisions (locked)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Rename scope | Paths + env vars + branding strings (NOT protocol layer) |
| 2 | Backward compatibility | **Clean break** — only read `.lingxi` / `LINGXI_*`; ignore old config |
| 3 | Brand name | **LingXi** (CLI `lingxi`, system prompt "You are LingXi") |
| 4 | Memory filename | **`LINGXI.md`** / `LINGXI.local.md`; do **not** read `CLAUDE.md` |
| 5 | Parity strategy | Keep harness; **behavior alignment + namespace rewrite** via *selective fixture updates* (not a global normalizer) |
| 6 | Internal symbols | **Rename**, but preserve oracle-citation comments (refinement) |
| 7 | Execution approach | **Hybrid (C)** — centralize behavior-critical values + config-home; guarded staged sweep for cosmetic categories |
| 8 | `.claude-plugin/` | **Rename → `.lingxi-plugin/`** (own plugin format) |
| 9 | MCP clientInfo identity | **Rename → LingXi** (sent to MCP servers) |

## 3. Key inventory findings (rationale for the architecture)

1. **No single seam.** Config-home (`$CLAUDE_CONFIG_DIR ?? ~/.claude`) is
   reimplemented in **≥8 helpers**: `engine config_home_dir` (settings/loader.rs:57),
   `migrations claude_config_home` (global_config.rs:81), `apps/cli claude_home_dir`
   (run.rs:1446), `tui claude_home_dir` (root.rs:2157 + doctor.rs:90),
   `commands/core skills.rs:127`, `memory config_home_dir` (session_memory.rs:254),
   `tools/task claude_config_home_dir` (todo_store.rs:103), `tools/meta config_home_dir`,
   plus inline copies in `bridge/lockfile.rs:108/144` and
   `platforms/posix secure_storage/factory.rs:103`, and again in the TS clients
   (`clients/shared/src/lockfile.ts:35`). **Missing one ⇒ split-brain.**
2. **Comments: ~6,753 mentions, ~99% are oracle citations** (`claude-code/src/...:line`).
   Only ~30–40 lines are generic brand prose safe to reword. Scrubbing the rest
   would falsify the parity provenance map (this is the Q6 refinement).
3. **Parity has no normalization layer; a blanket one is the wrong fix.** Protocol
   tokens (model IDs, `anthropic-beta`, MCP clientInfo, OAuth) are interleaved with
   branding tokens inside the same fixtures, so a global rewrite would mask real
   wire regressions. Use selective fixture/snapshot updates.
4. **Clients are already ~90% rebranded** (`com.lingxi.code`, `lingxi-code-desktop`,
   `lingxi:*` IPC, iOS `LingxiCode/`). What remains are deliberate cross-process
   contracts (§7).
5. **Protocol guard is well-isolated** (§5). Note: the OAuth `client_id` is *already*
   `lingxi-core` and is wire-registered — do **not** touch it.

## 4. Architecture — establish the seam (Approach C)

### 4.1 Namespace/branding module (single source of truth)
Introduce one module (low in the dep graph — e.g. a small new `branding` crate or
`traits`) exposing the runtime-affecting constants:

```
DOT_DIR            = ".lingxi"          // was ".claude"
GLOBAL_CONFIG      = ".lingxi.json"     // was ".claude.json"  (sibling in $HOME)
CONFIG_DIR_ENV     = "LINGXI_CONFIG_DIR"// was "CLAUDE_CONFIG_DIR"
MEMORY_FILE        = "LINGXI.md"        // was "CLAUDE.md"
MEMORY_LOCAL_FILE  = "LINGXI.local.md"  // was "CLAUDE.local.md"
PLUGIN_MANIFEST_DIR= ".lingxi-plugin"   // was ".claude-plugin"
PRODUCT_NAME       = "LingXi"           // was "Claude Code"
MANAGED_DIR_*      = LingXi managed-policy paths (see §4.4)
// + an env-prefix mapping helper for the local LINGXI_* family
```

### 4.2 Consolidate config-home
Replace the ≥8 duplicated helpers + inline copies with one
`config_home() = env(CONFIG_DIR_ENV) ?? home.join(DOT_DIR)`, preserving the existing
`??` semantics (a set-but-empty value is honored verbatim; see
`memory/src/claude_md/hierarchy.rs:21-34`). All `.join(".claude")` /
`.join("settings.json")`-style chains route through helpers built on `DOT_DIR`.

### 4.3 Two-commit flip (de-risk the consolidation)
- **Commit A — pure refactor:** introduce the module and route every helper +
  scattered literal through it, **values still `.claude`/`CLAUDE_*`/`CLAUDE.md`**.
  Workspace stays green ⇒ proves no behavior change. Reviewable in isolation.
- **Commit B — value flip:** change the constants to the LingXi values. This is the
  single behavior-changing commit and is where parity fixtures get reconciled.

### 4.4 Managed-policy dirs (two definition sites — keep in sync)
`memory/src/claude_md/hierarchy.rs:57-69` and
`apps/engine-desktop/src/settings_watch.rs:97-101`:
- macOS `/Library/Application Support/ClaudeCode` → `…/LingXi`
- Windows `C:\Program Files\ClaudeCode` → `…\LingXi`
- other `/etc/claude-code` → `/etc/lingxi`

## 5. Canonical rename table

| Category | From → To | Seam / notes |
|---|---|---|
| Config-home | `~/.claude` → `~/.lingxi` | §4.2; sub-leaves (`projects/`, `plugins/`, `tasks/`, `memdir/`, `team-mem/`, `agents/session-memory/`, `ide/`, `bridge/`, `.credentials.json`, `keybindings.json`, `.config.json` legacy) keep their names — only the `.claude` parent renames |
| Global config | `~/.claude.json` → `~/.lingxi.json` | `migrations/global_config.rs:105`, `tools/meta/config.rs:116,405` |
| Project dir | `<repo>/.claude/` → `<repo>/.lingxi/` | all subs: settings(.local).json, rules, commands, skills, agents, hooks, output-styles, workflows, scheduled_tasks.json, worktrees, loop.md, routines. Heavily scattered (`engine/settings/loader.rs:67`, `skill-api/listing.rs`, `commands/core/*`, `memory/claude_md/hierarchy.rs:514,681`, `apps/cli/commands/auto_mode.rs:72`, …) |
| Memory files | `CLAUDE.md`/`CLAUDE.local.md` → `LINGXI.md`/`LINGXI.local.md` | consts `FILE_NAME`/`LOCAL_OVERRIDE_NAME` (`memory/claude_md/hierarchy.rs:6-8`) **and** discovery globs (settings `additionalIncludes` default) + exclude globs `**/CLAUDE.md` (`orchestrator/prompt/memory_block.rs:241`, `memory/claude_md/excludes.rs:136-139`) |
| Managed dirs | see §4.4 | two definition sites |
| Local env vars | `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_*`, `CLAUDE_*` → `LINGXI_*` | collapse both prefixes to `LINGXI_`; dedupe where a `LINGXI_*` twin already exists (`LINGXI_ENABLE_XAA`, `LINGXI_MODEL`, `LINGXI_API_BASE_URL`). See §5.2 for the keep-list |
| Plugin manifest | `.claude-plugin/` → `.lingxi-plugin/` | `plugin/marketplace.rs:17,89`, `plugin/discovery.rs` |
| Keychain service | `"Claude Code"` → `"LingXi"` | `platforms/posix/secure_storage/{macos,linux}.rs` — orphans creds → re-auth |
| MCP clientInfo | `claude-code`/`Claude Code` → LingXi | client identity sent to MCP servers; update matching parity fixture |
| System prompt identity | `"You are Claude Code"` → `"You are LingXi"` | **reverses** the deliberate keep at `orchestrator/prompt/env_block.rs:41` |
| User-facing text | `Claude Code` → `LingXi` | TUI titles, CLI help/errors, init templates, banners |
| Internal symbols | `claude_md`→`lingxi_md`, `DOT_CLAUDE`→`DOT_LINGXI`, `ClaudeMdTier`→`LingxiMdTier` (~110), `ClaudeMdExcluder`, `claude_config_home`/`claude_home`/`claude_temp_dir`→`lingxi_*`, `claude_home` local var (~507) | per-crate, staged; `claude_home` var last (purely local, safest, highest count); disambiguate fn-vs-var name collisions |

### 5.2 Env-var keep-list (protocol — do NOT rename)
- **SDK auth/provider:** `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`,
  `ANTHROPIC_CUSTOM_HEADERS`, `ANTHROPIC_FOUNDRY_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`,
  `CLAUDE_CODE_USE_BEDROCK` / `…_USE_VERTEX` / `…_USE_FOUNDRY`.
- **User-Agent feeders:** `CLAUDE_CODE_ENTRYPOINT` (value → UA header, `model/user_agent.rs:73`),
  `CLAUDE_AGENT_SDK_VERSION`, `CLAUDE_AGENT_SDK_CLIENT_APP`, `CLAUDE_CODE_VERSION` (verify per call site).
- **Request body / header feeders:** `CLAUDE_CODE_EXTRA_METADATA` (→ `metadata.user_id`),
  `ANTHROPIC_BETAS`, `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`,
  `ANTHROPIC_DEFAULT_OPUS/SONNET/HAIKU_MODEL`, `ANTHROPIC_SMALL_FAST_MODEL` (values become the model field).
- **Not env vars** (regex/const/comment artifacts — leave as symbols, do not treat as env):
  `ANTHROPIC_VERSION`, `CLAUDE_CODE_BETA`, `CLAUDE_CODE_OAUTH_SCOPES`, `CLAUDE_AI_*_SCOPE`,
  `CLAUDE_OPUS_4_*_CONFIG`, trailing-underscore prefix tokens.
- **Script-facing child-env vars → RENAME to `LINGXI_*`** (clean break): `CLAUDE_PROJECT_DIR`,
  `CLAUDE_SESSION_ID`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_SKILL_DIR`, `CLAUDE_PLUGIN_ROOT`,
  `CLAUDE_EFFORT`. Consequence: user-authored hooks/skills referencing the old names break (accepted).

## 6. Clean-break consequences (accepted under Decision 2)
- **Re-authentication required:** keychain service name change **and** the
  `sha256(config_dir)[..8]` discriminator (`platforms/posix/secure_storage/helpers.rs:45`)
  both change ⇒ stored OAuth creds orphaned.
- **Resume history invisible:** prior transcripts under `~/.claude/projects/...` are
  not read (new store is `~/.lingxi/projects/...`).
- **No migration code** is written.

## 7. Cross-process lockstep (clients ↔ engine — change atomically)
Renaming one side alone breaks discovery (path) or causes HTTP 401 before WS upgrade (header):
- `~/.claude/bridge` → `~/.lingxi/bridge`: Rust `bridge/lockfile.rs` **and** TS
  `clients/shared/src/lockfile.ts:35`, `clients/electron/src/main/bridge.ts:33`,
  `clients/shared/src/client.ts:147`, `clients/shared/scripts/e2e.mjs`.
- `X-Claude-Code-Ide-Authorization` → `X-LingXi-Ide-Authorization`:
  `bridge/src/mcp_endpoint.rs` (`AUTH_HEADER_NAME`), `apps/bridge-server/src/main.rs`,
  `clients/shared/src/client.ts:50`. Internal LingXi-only header; safe in lockstep.
- Mock title `"重装 Claude Code"` + assertions in `AppFlowUiTest.kt`,
  `SessionStateTest.kt`, `DrawerSearchTest.kt` (the last also matches lowercase
  `claude` for case-insensitive search — update the query too).
- Local-storage keys named `anthropic.*` / `anthropic_api_key` and the provider key
  `anthropic` **stay** (they name the provider, not the brand).

## 8. DO-NOT-TOUCH denylist (protocol + referential)
- Model IDs (`claude-opus-*`, `claude-sonnet-*`, `claude-haiku-*`) **and** the
  `anthropic`/`claude-` substring parsing in `agent/src/model_resolution.rs`.
- `api.anthropic.com` + `/v1/messages`, `/v1/messages/count_tokens`, `/v1/code/triggers`.
- The `anthropic` provider id.
- **All ~463 `tengu_*`** names (`telemetry/src/tengu/`) — they double as Statsig
  feature-flag keys; renaming disables features, not just metrics. Atomic keep-block.
- `anthropic-beta` constants (`claude-code-20250219`, `interleaved-thinking-…`,
  `context-1m-…`, `oauth-2025-04-20`, …).
- `claude-cli/<ver>` UA (Anthropic route), `anthropic-version: 2023-06-01`,
  `x-api-key` / `Authorization: Bearer`. (Non-Anthropic route already uses neutral
  `LingXi-Code/<ver>` — leave it.)
- OAuth endpoints/scopes; `client_id = "lingxi-core"` (already non-Claude, wire-registered).
- All `ANTHROPIC_*` env vars (§5.2).
- `AI_AGENT` child-env stamp (`claude-code_<ver>_agent`).
- `rate_limit_tier` string values (`default_claude_max_20x`, `default_claude_pro`, …).
- `BedrockClaude*` / `VertexClaude*` / `ClaudeAiOAuth*` Rust types (name the real
  model family / Claude.ai OAuth provider).
- `AddFromClaudeDesktop` subcommand + the Claude Desktop config path it imports
  (interop with the real Claude Desktop app).
- `AGENTS.md` (cross-tool convention, not Anthropic branding).
- **All ~6,700 oracle-citation comments** (`claude-code/src/...:line`).
- The `claude-code-guide` subagent body (`agent/src/builtins.rs:395/487`) — it is
  *about* Claude products by design.
- Embedded URLs: `github.com/anthropics/claude-code/issues`, `claude.ai/code`,
  `claude.com/claude-code`, `support.anthropic.com` (exclude from any text sweep).
- The `🤖 Generated with [Claude Code](https://claude.com/claude-code)` commit
  attribution and `Co-Authored-By` lines: treated as a separate, explicit decision in
  the plan (rebrand text + drop/replace URL, or leave) — not swept blindly.

## 9. Comments policy (Decision 6)
Reword **only** the ~30–40 generic brand-prose lines that describe *our* product
(e.g. "Manage Claude Code plugins" → "Manage LingXi plugins"), and only where they
do not embed a keep-literal. Preserve every oracle citation. Mixed comments
(e.g. `register.rs:289` "Claude Code implements /skills … LingXi's …") are edited
per-clause, not whole-line.

## 10. Parity strategy (Decision 5, refined)
No global normalizer. Selective updates only:
- ~9 UI/path fixtures + their golden text → LingXi values.
- 5 `.snap` snapshots (bypass-permission dialogs ×2, doctor "Claude home", memory
  selector `CLAUDE.md` paths, agents-screen description) → regenerate via
  `cargo insta accept` (never hand-edit).
- Inline `.rs` literals duplicated in parity drivers updated alongside fixtures
  (`parity_mcp_initialize.rs`, `parity_tui_permission_dialogs.rs`,
  `parity_full_v0_4_0_smoke.rs`, `parity_system_tools.rs`, `parity_team_tools.rs`,
  `parity_settings_merge.rs`).
- `parity_init_template.json`: re-derive `sha256` **and** `byte_length` **and**
  `first_sentence`/substrings together.
- Fixtures for deliberately-rebranded wire values (MCP clientInfo, keychain) updated
  to the new values; pure-protocol fixtures (betas, model IDs, oauth) untouched.
- Going forward: behavior/structure parity is preserved; the brand namespace is a
  documented permanent divergence; oracle citations remain the provenance map.

## 11. Sequencing (each stage independently testable)
1. Namespace module + config-home consolidation — **Commit A (refactor)** then **Commit B (value flip)** (§4.3).
2. Local env-var rename (`LINGXI_*`) + child-env injection.
3. Filenames: `LINGXI.md` + globs/excludes; managed dirs; keychain; MCP clientInfo.
4. Branding-string sweep (guarded; system-prompt identity).
5. Internal symbols, per-crate (`claude_home` var last).
6. Comment rewording (~30–40 lines).
7. `.lingxi-plugin/`.
8. Clients cross-process lockstep (bridge dir, IDE auth header, mock titles).
9. Parity fixtures/snapshots reconciled + full verification.

## 12. Verification
- `cargo test --workspace --no-run` after each crate (catches test-only literal
  breaks that a normal run misses).
- After any render/string change run **both** `cargo test -p tui` **and**
  `cargo test -p test-harness` (two byte-locked layers).
- Build with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_RELEASE_DEBUG=0` (repo convention).
- Manual smoke: fresh `~/.lingxi` creation; `LINGXI.md` load; login/re-auth;
  desktop/Electron bridge discovery; `/doctor`.
- Final guard sweep: `grep -rn '\.claude\|CLAUDE_'` over `lingxi-code/` + `clients/`
  (excluding vendored trees, oracle-citation comments, and the §8 denylist) returns
  no **runtime** read/write sites — proves no split-brain.

## 13. Open risks & flagged items
- **Env-var prefix convention** is `LINGXI_*` (both `CLAUDE_` and `CLAUDE_CODE_`
  collapse to `LINGXI_`). Where a `LINGXI_*` twin already exists, the existing name
  wins; remove the duplicate read to avoid precedence bugs.
- **`CLAUDE_TOKEN`** (`platforms/common/llm_config.rs`) was not fully traced — if it
  is an auth credential it is keep-protocol; resolve during implementation.
- **`.lingxi-plugin/` drops the existing Claude plugin ecosystem** (Decision 8,
  accepted) — existing third-party Claude plugins won't load.
- **MCP clientInfo → LingXi** may not be recognized by MCP servers that whitelist
  `claude-code` (Decision 9, accepted, low risk).
- **Inert constants** (e.g. `BRIEF_SUBDIR`/`BRIEF_FILE_SUFFIX` in `tools/ui/brief.rs`)
  may not be joined to a live path — verify before treating as runtime paths.
- **Two `agents` shapes:** `<config_home>/agents/session-memory` vs
  `<repo>/.lingxi/agents` — same leaf, different parent; verify each call site.

## 14. Appendix — primary seams (for the implementation plan)
- Config-home helpers (≥8): listed in §3.1.
- Memory consts/globs: `memory/src/claude_md/hierarchy.rs`,
  `memory/src/claude_md/excludes.rs`, `orchestrator/src/prompt/memory_block.rs`.
- Managed dirs: `memory/src/claude_md/hierarchy.rs:57-69`,
  `apps/engine-desktop/src/settings_watch.rs:97-101`.
- UA / protocol env: `llm-client/src/model/user_agent.rs`, `…/model/betas.rs`,
  `…/service.rs`, `…/protocol.rs`.
- Secure storage: `platforms/posix/src/secure_storage/{factory,helpers,macos,linux}.rs`.
- Bridge/IPC: `bridge/src/lockfile.rs`, `bridge/src/mcp_endpoint.rs`,
  `apps/bridge-server/src/main.rs`, `clients/shared/src/{lockfile,client}.ts`,
  `clients/electron/src/main/bridge.ts`.
- Parity: `test-harness/src/parity/fixtures/*.json`, `test-harness/tests/parity_*.rs`,
  `tui/tests/snapshots/*.snap`.
