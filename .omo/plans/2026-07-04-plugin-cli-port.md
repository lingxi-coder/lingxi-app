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

### 2. install / uninstall
`install <plugin[@market]>` → resolve marketplace, git-fetch to
`cache/{market}/{plugin}/{version}/`, `installed::record`, then enable
(`enabledPlugins[id]=true`) at `--scope` (default user). `--config key=value`
(repeatable) validated against manifest userConfig. `uninstall` → drop record +
`enabledPlugins[id]=false`, delete cache (unless `--keep-data` for data dir),
`--prune` orphaned deps (needs `-y` non-TTY). Reuse `PluginManager::install`
arms + `MarketplaceManager`.

### 3. marketplace add / list / remove / update
Settings key `extraKnownMarketplaces` (array of declarations). `add <source>`
(URL/path/GitHub) `--sparse <paths…>` `--scope`; `list [--json]`
(empty → `No marketplaces configured` / `[]`); `remove <name> [--scope]` (omit
scope → all scopes); `update [name]` re-fetch. Reuse `resolve_index_via_git`.

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
