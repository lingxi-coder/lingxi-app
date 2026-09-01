# Config-Migration Subsystem Design

**Date:** 2026-06-10
**Status:** Approved (user confirmed: full 9+1 set, default-on, remove `deny_unknown_fields`)
**Reference of truth:** `claude-code/` TS (leaked 2026-03-31 source) — `src/main.tsx` `runMigrations()`, `src/migrations/*.ts`, `src/utils/config.ts`, `src/utils/releaseNotes.ts`
**Branch target:** new feature branch off `main`, merged locally after gates (established parity pipeline)

## Goal

Port claude-code's startup config-migration subsystem to the Rust port: `runMigrations()` with
`CURRENT_MIGRATION_VERSION = 11`, the 9 external-build-reachable sync migrations, and the async
changelog migration. This closes the "config-migration subsystem" item from the parity remainder
list. SENSITIVE: it rewrites the real `~/.claude.json` and `~/.claude/settings.json` at startup —
safety invariants below are load-bearing.

## Scope triage (locked)

External-build-reachable = **9 sync + 1 async**. Excluded with reasons:

- `migrateFennecToOpus` — guarded by `if ("external" === 'ant')` → dead code in the external build. NOT ported.
- `resetAutoModeOptInForDefaultOffer` — guarded by `feature('TRANSCRIPT_CLASSIFIER')`, ant-only
  classifier (correctly stubbed in this port). NOT ported.

Ported, in TS execution order (`main.tsx:328-336`):

1. `migrateAutoUpdatesToSettings`
2. `migrateBypassPermissionsAcceptedToSettings`
3. `migrateEnableAllProjectMcpServersToSettings`
4. `resetProToOpusDefault`
5. `migrateSonnet1mToSonnet45`
6. `migrateLegacyOpusToCurrent`
7. `migrateSonnet45ToSonnet46`
8. `migrateOpusToOpus1m`
9. `migrateReplBridgeEnabledToRemoteControlAtStartup`

Plus async fire-and-forget: `migrateChangelogFromConfig` (`releaseNotes.ts:55`).

## Architecture

New top-level crate **`lingxi-code/migrations/`** (desktop-only; wired only in `apps/cli`;
NEVER in the engine-mobile dependency tree). Mirrors TS `src/migrations/` one-file-per-migration.

```
migrations/
├── Cargo.toml          # deps: serde_json, tokio (spawn/fs for changelog), anthropic-oauth
│                       # (SubscriptionType only), telemetry (events), tempfile (dev)
└── src/
    ├── lib.rs
    ├── global_config.rs    # ~/.claude.json substrate (NEW — no prior Rust substrate)
    ├── settings_update.rs  # updateSettingsForSource port (userSettings + localSettings)
    ├── context.rs          # MigrationContext (provider/subscription/env seams)
    ├── runner.rs           # run_migrations() + CURRENT_MIGRATION_VERSION = 11
    ├── changelog.rs        # async migrateChangelogFromConfig
    └── migrate_*.rs ×9     # one file per migration, 1:1 with TS
```

Why a new crate (not `engine`): the subscriber-gated migrations need
`anthropic_oauth::limits::SubscriptionType`, and `engine` is in engine-mobile's dependency tree —
a new leaf crate keeps the dep direction clean and mirrors the TS directory layout.

## Component 1: GlobalConfig substrate (`global_config.rs`)

The Rust port has NO `~/.claude.json` reader/writer today (confirmed: `tools/meta/src/config.rs:17`
"no substrate"); the `migrationVersion` guard itself lives there, so this is the prerequisite.

**Path resolution** (port of `utils/env.ts getGlobalClaudeFile` + `utils/envUtils.ts
getClaudeConfigHomeDir`):

- Claude config home = `$CLAUDE_CONFIG_DIR` if set, else `~/.claude` (NFC-normalized in TS;
  Rust uses the path as-is — NFC normalization is a documented no-op divergence on already-NFC
  macOS paths).
- Legacy fallback: if `<config-home>/.config.json` exists, use it.
- Else `(<$CLAUDE_CONFIG_DIR or $HOME>)/.claude.json`. The TS oauth suffix
  (`fileSuffixForOauthConfig()` → `-custom-oauth`/`-local-oauth`/`-staging-oauth`) applies only
  under custom OAuth env vars; the default build resolves to `""` → `.claude.json`. Port the
  suffix logic only if the Rust port already models `getOauthConfigType`; otherwise hardcode `""`
  with a doc comment (decided at plan time after checking `anthropic-oauth`).

**Read** (`get_global_config()`): whole file → `serde_json::Map<String, Value>` — unknown keys are
NEVER dropped (the real file carries `numStartups`, `projects`, `oauthAccount`, dozens more).
Typed accessors only for the keys the migrations touch:

| Key | Type | Used by |
|---|---|---|
| `migrationVersion` | u64 | runner guard |
| `autoUpdates`, `autoUpdatesProtectedForNative` | bool | autoUpdates |
| `bypassPermissionsModeAccepted` | bool | bypassPermissions |
| `opusProMigrationComplete`, `opusProMigrationTimestamp` | bool / i64 ms | resetProToOpus |
| `sonnet1m45MigrationComplete` | bool | sonnet1m→45 |
| `legacyOpusMigrationTimestamp` | i64 ms | legacyOpus |
| `sonnet45To46MigrationTimestamp` | i64 ms | sonnet45→46 |
| `numStartups` | u64 (default 0) | sonnet45→46 notification gate |
| `remoteControlAtStartup` / legacy `replBridgeEnabled` | bool | replBridge rename |
| `cachedChangelog` | String | changelog migration |
| `projects` (map keyed by normalized project path) | object | MCP migration |

**Write** (`save_global_config(mutator)`): mirrors TS `saveGlobalConfig(prev => next)` —
read-modify-write; if the mutator returns the input unchanged (Rust: value equality), ZERO write;
on write, port `removeProjectHistory` (strip legacy `history` key from each project entry — TS does
this on every save; the `needsCleaning` gate leaves untouched configs byte-identical); write via
tmp-file + atomic rename, `serde_json::to_string_pretty` + trailing newline.

**Documented simplifications** (recorded in module docs):
- No `proper-lockfile` cross-process lock and no in-memory mtime cache — migrations run once at
  startup before any concurrent writer in this process; the TS lock guards a multi-write runtime
  this port doesn't have yet. The auth-loss fallback guard (GH #3117) is therefore also N/A —
  we never write defaults over a failed read (see next line).
- Broken/unreadable JSON → `run_migrations` SKIPS this startup entirely (no write, no version
  bump). Strictly safer than TS (which falls back to defaults under a guard); divergence documented.

**Project config** (`get_current_project_config` / `save_current_project_config`): the
`projects[<key>]` sub-object where key = canonical git root of the original cwd, else
`resolve(cwd)`, forward-slash-normalized (`config.ts:1588 getProjectPathForConfig`). Reuses the
repo-root discovery already available to the CLI; memoized per process.

## Component 2: settings writer (`settings_update.rs`)

Port of `updateSettingsForSource` for the two sources the migrations write:

- `userSettings` → `<claude-config-home>/settings.json` (NOTE: honors `CLAUDE_CONFIG_DIR`, same as
  TS; `commands/core/effort.rs` hardcodes `$HOME/.claude` — pre-existing, not changed here, noted
  as a dedup follow-up)
- `localSettings` → `<project-dir>/.claude/settings.local.json`

Semantics (proven by the effort.rs precedent, extended): `create_dir_all` parent; missing/empty
file ⇒ empty object; **syntactically broken JSON ⇒ bail without overwriting**; merge: key present
⇒ set, explicit-delete marker ⇒ remove (TS `undefined`); `to_string_pretty` + newline. Operates on
raw `serde_json::Map` (never the typed `SettingsJson`) so unknown keys in the user's real
settings.json are preserved.

Existing duplicates (effort.rs, permission/persist.rs, tools/meta/config.rs) are NOT consolidated
in this batch — noted as follow-up.

## Component 3: MigrationContext (`context.rs`)

```rust
pub struct MigrationContext {
    /// getAPIProvider() == 'firstParty' — env-derived exactly like TS
    /// (CLAUDE_CODE_USE_BEDROCK / CLAUDE_CODE_USE_VERTEX / foundry env ⇒ not first-party).
    pub first_party: bool,
    /// Subscription tier. The Rust keychain token has NO subscriptionType field
    /// (secret/src/credential.rs OAuthTokens = scopes/email/org_id) ⇒ structurally
    /// None today. Seam typed as Option<SubscriptionType> so a future
    /// tier-persistence batch lights up the gated paths without API change.
    pub subscription_type: Option<anthropic_oauth::limits::SubscriptionType>,
}
```

Fail-closed semantics are FAITHFUL: TS itself fails closed on unknown tier
(`isOpus1mMergeEnabled`: "Fail closed when a subscriber's subscription type is unknown" —
`model.ts:322-330`). With `subscription_type == None`:

- `resetProToOpusDefault` → takes its real "not Pro" branch: marks `opusProMigrationComplete` +
  emits `tengu_reset_pro_to_opus_default {skipped:true}` (this IS TS behavior, not a stub).
- `migrateSonnet45ToSonnet46` → early-return, no write (= TS non-subscriber path).
- `migrateOpusToOpus1m` → `isOpus1mMergeEnabled()` false → early-return. The eligible body is
  still ported (with a local minimal `[1m]`-suffix model comparison standing in for
  `parseUserSpecifiedModel`, doc-commented) so it lights up when tier persistence lands.

The CLI does NOT read the keychain pre-boot just for this (avoids a second keychain prompt);
it passes `subscription_type: None` until a tier-persistence batch exists.

## Component 4: the nine migrations (fidelity contract)

Each ports its TS file 1:1 (same guards, same key reads/writes, same telemetry, same idempotency
mechanism). Fidelity notes:

| Migration | Fidelity |
|---|---|
| autoUpdates→settings env | Full. Includes `std::env::set_var("DISABLE_AUTOUPDATER", "1")` after the settings write; error path emits `tengu_migrate_autoupdates_error`. |
| bypassPermissionsAccepted→settings | Full at file level. `skipDangerousModePermissionPrompt` has no Rust consumer yet (parity-gap B) — writing it is still faithful + forward-compatible. `hasSkipDangerousModePermissionPrompt` check reads merged user settings. |
| MCP three fields → settings.local.json | Full. project-config → localSettings move with set-union merge of `enabledMcpjsonServers`/`disabledMcpjsonServers`, `enableAllProjectMcpServers` only-if-unset, field removal from project config in one save. |
| resetProToOpusDefault | Guards faithful; tier None ⇒ "not Pro" branch (see Component 3). Timestamp written as epoch-ms (`Date.now()` parity). |
| sonnet[1m]→sonnet-4-5[1m] | Full for the settings rewrite + `sonnet1m45MigrationComplete` flag. The TS in-memory `MainLoopModelOverride` sub-step is N/A (no such pre-boot in-memory state in Rust) — documented. |
| legacyOpus→opus | Full: firstParty gate + `CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP` opt-out (`isEnvTruthy` port) + the 4 legacy model-string matches + `legacyOpusMigrationTimestamp`. |
| sonnet45→46 | Guards faithful; tier None ⇒ early-return. Body ported: 4 model-string matches, `[1m]` preservation, `numStartups > 1` notification-timestamp gate. |
| opus→opus[1m] | Guards faithful (fail-closed); body ported with local model-comparison helper (doc-commented stand-in for `parseUserSpecifiedModel`/`getDefaultMainLoopModelSetting`, which have no Rust port). |
| replBridgeEnabled→remoteControlAtStartup | Full (pure rename: only when old key exists and new key absent). |

**Runner** (`runner.rs`): `CURRENT_MIGRATION_VERSION: u64 = 11`; guard is `!= 11` (re-runs on
downgrade too, like TS); runs the 9 in TS order; each migration's failure is caught + logged,
never aborts startup (TS per-migration try/catch parity where TS has it; runner-level catch-all
so a panic in one migration cannot brick the CLI — `catch_unwind` or per-fn Result, decided at
plan time); ends with the conditional `migrationVersion = 11` write
(`prev.migrationVersion === 11 ? prev : {...prev, migrationVersion: 11}`).

**Changelog migration** (`changelog.rs`): if `cachedChangelog` key exists → `create_dir_all` +
write `<claude-config-home>/cache/changelog.md` with create-new (`wx`) semantics (existing file
⇒ silent skip) → remove the key from global config. Spawned via `tokio::spawn`, errors silently
ignored (TS `.catch(() => {})`), retried next startup.

## Component 5: engine SettingsJson loosening (approved)

Remove `deny_unknown_fields` from `core/src/settings/schema.rs::SettingsJson`. Rationale:
claude-code's zod `SettingsSchema().safeParse` STRIPS unknown keys (zod default — non-strict);
the Rust strictness is itself a parity divergence, and it already bites: `/effort`'s persisted
`effortLevel` makes the engine settings load return `ParseError` → production callers `.ok()` →
the ENTIRE settings file is silently ignored (model/outputStyle/providers/routing all fall back).
Migration-written keys (`env`, `skipDangerousModePermissionPrompt`, `enableAllProjectMcpServers`,
`enabledMcpjsonServers`, `disabledMcpjsonServers`, `fastMode`) would trip the same mine.

Changes: drop the serde attr; update `loader.rs` test
`returns_schema_violation_for_unknown_field` → a tolerance test (unknown keys load fine, known
fields still parse); doc-comment the zod-strip parity rationale on the struct; `validate()` and
the merge/tracer tables are unaffected (they iterate known fields only).

## Component 6: telemetry events

New tengu event names (9): `tengu_migrate_autoupdates_to_settings`,
`tengu_migrate_autoupdates_error`, `tengu_migrate_bypass_permissions_accepted`,
`tengu_migrate_mcp_approval_fields_success`, `tengu_migrate_mcp_approval_fields_error`,
`tengu_reset_pro_to_opus_default`, `tengu_legacy_opus_migration`,
`tengu_sonnet45_to_46_migration`, `tengu_opus_to_opus1m_migration`.

Registry placement follows the W36 precedent (global-tail append to the events registry).
**W36/W38 lesson encoded as a plan gate:** after changing `ALL_EVENT_NAMES`, grep the WHOLE
workspace for the old count (`grep -rn "<oldcount>" --include=*.rs`) and RUN (not `--no-run`) the
dependent crates' tests: telemetry, orchestrator (diagnostics), tui (vim, behavior_palette),
test-harness.

Events are emitted through the existing `telemetry::AnalyticsBus` seam, `Option<&Arc<AnalyticsBus>>`
in the runner signature (None in unit tests).

## Component 7: CLI wiring

`apps/cli` `run_cli`, pre-REPL — the same point as the W41 model-deprecation warning (common to
Print/Tui/StdioRepl). Order: `run_migrations(&MigrationContext{...}, bus)` synchronously, then
`tokio::spawn(migrate_changelog_from_config())`, then proceed to `engine_desktop::build`. Default
ON (approved) — no env gate. `migrations` is added as a dependency of `apps/cli` only.

## Safety invariants (load-bearing)

1. **Preserve unknown keys** in both `~/.claude.json` and settings files — raw-map round-trip,
   never typed-struct re-serialization.
2. **Atomic writes** (tmp + rename, same directory).
3. **Zero writes when nothing changes** — mutator same-value short-circuit; on a machine where
   real claude-code already migrated to v11 (this machine), the version guard makes the whole
   subsystem a read-only no-op.
4. **Broken JSON is never overwritten** — global config: skip the run; settings: bail that write.
5. **Failures never abort startup** — log + continue; version bump still happens (TS parity:
   per-migration try/catch swallow).

## Testing strategy

All tests redirect `HOME`/`CLAUDE_CONFIG_DIR` to a tempdir (effort.rs precedent; env-mutating
tests serialized with the existing env-lock pattern).

- Per-migration unit tests: trigger-condition true/false, write effect, idempotency (run twice ⇒
  second run no-op), error paths.
- Runner: version != 11 runs + bumps; == 11 zero file writes (assert mtime/content unchanged);
  order-sensitive interplay (autoUpdates removes keys before version bump).
- GlobalConfig: unknown-key round-trip preservation; atomic write (no partial file on simulated
  failure); broken-JSON skip; legacy `.config.json` fallback; `removeProjectHistory` cleaning gate.
- settings_update: merge/delete/broken-JSON-bail; localSettings path shape.
- Schema loosening regression: settings.json with `effortLevel` + arbitrary unknown keys loads;
  known fields still parse; merge/tracer behavior unchanged.
- Gates: `cargo test -p migrations -p core -p telemetry` + the W36-lesson dependent-crate RUN
  set; `clippy -D warnings` on touched crates; whole-workspace `cargo test --workspace --no-run`
  struct-trap; both engines build; verify engine-mobile pulls no `migrations` crate
  (`cargo tree -p engine-mobile | grep -c migrations` == 0).

## Out of scope (documented follow-ups)

- Subscription-tier persistence in the keychain token (lights up the 3 gated migrations' eligible
  paths).
- Startup-notification-queue consuming the 3 `*MigrationTimestamp` keys (separate remainder item).
- Consolidating the 3 existing ad-hoc settings writers onto `settings_update.rs`.
- `numStartups` increment (the Rust port never writes it; we only read with default 0).
- The `--dangerously-skip-permissions` consumer of `skipDangerousModePermissionPrompt` (remainder
  item B, needs separate user confirmation).
