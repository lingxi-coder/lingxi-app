# Plugin CLI parity port (Gap 5 vs claude-code 2.1.201)

Branch: `plugin-cli-port` (worktree `.worktrees/plugin-cli-port`).
Oracle: `/Users/luolingfeng/.local/share/claude/versions/2.1.201` (run `claude plugin … --help` / probe in isolated `$HOME`+`$CLAUDE_CONFIG_DIR`).

## Goal
Wire the already-byte-faithful `lingxi-cli plugin` clap surface
(`apps/cli/src/commands/plugin.rs`) to real behavior, replacing every
`notice("…") → NOT_IMPLEMENTED` arm with a 1:1 port of 2.1.201. `list`,
`details`, `validate` are already real.

## Backing that already exists (reuse, don't rebuild)
- `migrations::settings_update` — JSON-preserving settings read-modify-write
  (`read_settings_map` / `update_settings`, top-level replace). Has `User` +
  `Local` scopes; **needs a `Project` scope added** (`<cwd>/.lingxi/settings.json`).
- `plugin::installed` — `installed_plugins.json` load/`record`; **needs `remove`**.
- `plugin::discovery::discover_recorded_plugins` — resolve installed name→marketplace.
- `plugin::MarketplaceManager` — `resolve_index_via_git`, `plugin_dir_in_clone`.
- `plugin::PluginManager::{install,enable,disable}` — heavy engine-registry
  materialization (NOT the CLI seam; CLI toggles on-disk settings instead).

## Scope model (all subcommands)
`user` = `~/.lingxi/settings.json`; `project` = `<cwd>/.lingxi/settings.json`;
`local` = `<cwd>/.lingxi/settings.local.json`. Invalid scope →
`Invalid scope "<x>". Valid scopes: user, project, local` (update also allows
`managed`). Default: `user` for install/uninstall/update/prune; **auto-detect**
for enable/disable (find the scope already holding the id; else `user`).

## Increments (each: TDD, `cargo test -p <crate>` green, one commit)

### 1. enable / disable  ← START HERE
On-disk `settings.enabledPlugins` toggle. `enable` sets id→`true`, `disable`
sets id→`false` (NOT delete). Enables/disables even non-installed ids (pure
allowlist). Exact I/O (probed):
- `enable a@b` → write `{"enabledPlugins":{"a@b":true}}`; stdout
  `✔ Successfully enabled plugin: a (scope: user)` (name = pre-`@`).
- bare name: resolve to an existing `name@*` key across editable scopes; none →
  `✘ Failed to enable plugin "solo": Plugin "solo" not found in any editable settings scope. Use plugin@marketplace format.`
- already `true` → `✘ Failed to enable plugin "a@b": Plugin "a@b" is already enabled`.
- `disable` absent/`false` → `… is already disabled`.
- `disable --all` → set every currently-`true` entry to `false`; `✔ Disabled N plugins`.
- New module `apps/cli/src/commands/plugin_settings.rs` (scope resolve + RMW + id resolve).

### DEPENDENCY NOTE (discovered 2026-07-04 by probing real binary)
install's happy path REQUIRES a configured marketplace, so **do marketplace
(3) BEFORE install/uninstall (2)**. Also two pre-existing local schema drifts
must be fixed as part of this work:
- `plugin/src/installed.rs` `InstalledPlugins` is an OLD shape
  (`plugins[marketplace][plugin] = {version, added}`). **Real 2.1.201 v2 schema:**
  `{"version":2,"plugins":{"<plugin>@<market>":[{"scope","installPath","version","installedAt"(ISO8601),"lastUpdated"(ISO8601)}]}}`
  (keyed by full id → ARRAY of per-scope records). `discover_recorded_plugins`
  (used by the already-"real" `list`/`details`) reads the old shape → update both.
- `plugin/src/marketplace.rs` `MarketplaceIndex` is missing the **required
  `owner` object** — real `marketplace.json` schema rejects a catalog lacking
  `owner:{name}` (`Invalid schema … owner: expected object, received undefined`).

### 3. marketplace add / list / remove / update  ← DO FIRST
Two on-disk stores, written together by `add`:
1. settings `extraKnownMarketplaces` (per-scope DECLARATION), shape
   `{"<name>":{"source":<S>}}` where `<S>` is one of:
   `{"source":"directory","path":<abs>}` / `{"source":"git","url":<u>,"ref"?:<r>}`
   / `{"source":"github","repo":<owner/repo>,"ref"?:<r>}` / `{"source":"url","url":<u>}`.
2. `<plugins>/known_marketplaces.json` (resolved REGISTRY — the source of truth
   `list` renders; a settings-only decl without this shows nothing).
- name = the marketplace.json `name` field (NOT the arg/dir).
- `add <source>`: `Adding marketplace…✔ Successfully added marketplace: <name> (declared in <scope> settings)`.
  Sources: local dir, URL, GitHub `owner/repo`. `--sparse <paths…>` (git
  sparse-checkout), `--scope` (default user). Reuse `resolve_index_via_git`.
- `list`: empty → `No marketplaces configured`. Human:
  `Configured marketplaces:\n\n  ❯ <name>\n    Source: Directory (<path>) | Git (<url>[ @ref]) | GitHub (<repo>[ @ref]) | URL (<url>)`.
  `--json` → `[{"name","source","path"|"url"|"repo","installLocation"}]`; empty → `[]`.
- `remove <name> [--scope]`: not-configured →
  `✘ Failed to remove marketplace: Marketplace '<name>' not found`. success →
  `Removed marketplace '<name>' declaration from <scope>[; still declared in <scopes>]`.
  Omit `--scope` → remove from every scope.
- `update [name]` re-fetch (all if no name).

### 2. install / uninstall (after marketplace)
`install <plugin[@market]>` → resolve entry from a configured marketplace,
git/dir-fetch to `cache/{market}/{plugin}/{version}/`, write installed record
(NEW schema above), set `enabledPlugins[id]=true` at `--scope` (default user).
Exact I/O:
- progress prefix `Installing plugin "<arg>"...` then ✔/✘ on the SAME line.
- success `✔ Successfully installed plugin: <id> (scope: <scope>)`.
- no marketplace → `✘ Failed to install plugin "<arg>": Plugin "<name>" not found in any configured marketplace`.
- unknown market `foo@bar` → `✘ Failed to install plugin "foo@bar": Plugin "foo" not found in marketplace "bar". Your local copy may be out of date — try ` + "`claude plugin marketplace update bar`" + `.`
- invalid scope (DIFFERENT from enable/disable!): `Invalid scope: <x>. Must be one of: user, project, local.`
`--config key=value` (repeatable) validated against manifest userConfig.
`uninstall <plugin>`: not-installed → `✘ Failed to uninstall plugin "<arg>": Plugin "<name>" not found in installed plugins`. On success drop record +
`enabledPlugins[id]=false`, delete cache (keep data dir unless… `--keep-data`
preserves `plugins/data/{id}/`), `--prune` orphaned deps (needs `-y` non-TTY).

### 4. update
`update <plugin>` → re-fetch latest into cache, re-record, "restart required to
apply". Scope incl. `managed`.

### 5. prune / autoremove
Dep-graph orphan removal (`installedBy`/auto-installed marker — needs modeling
in `installed.rs`). `--dry-run`, `-y`, `--scope`.

### 6. init / tag
`init <name>` scaffold `~/.lingxi/skills/<name>/` (+`--with` components,
`--author`/`--author-email`/`--description`, `-f`). `tag [path]` create
`{name}--v{version}` git tag validating plugin.json vs marketplace entry;
`--dry-run`/`-f`/`-m`/`--push`/`--remote`.

## Residuals / notes
- `list --available` still needs marketplace resolution (increment 3 unlocks it).
- `details` projected token cost still not fabricated (unchanged).
- Symbols `✔`/`✘` mirror the oracle; brand tokens `claude`→`lingxi`, `.claude`→`.lingxi`.
