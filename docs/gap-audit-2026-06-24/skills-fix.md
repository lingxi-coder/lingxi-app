# skills-plugin parity fix

**Commit**: `27034a95`
**Date**: 2026-06-24
**Test exit codes**: skill-api 22/22 OK, tool-skill 46/46 OK, outputstyles 16/16 OK, plugin 4/4 OK
**Workspace build**: `Finished dev profile` — CLEAN (11.17s). Pre-existing `bridge-server` e2e test failure (missing `DesktopConfig` fields) is unrelated and pre-dates this patch.

---

## Per-gap status

| # | Gap | Severity | Status |
|---|-----|----------|--------|
| 1 | SkillTool `prompt()` stub → full binary text | P1 | DONE |
| 2 | `disable-model-invocation` YAML field | P1 | DONE |
| 3 | `user-invocable` field | P1 | DONE |
| 4 | `disallowed-tools` field (+ camelCase alias) | P1 | DONE |
| 5 | `paths` field (conditional skill activation) | P1 | DONE (field parsed; dynamic discovery at FileWrite deferred — deep cross-tool change) |
| 6 | `argument-hint` / `arguments` fields | P1 | DONE |
| 7 | `effort`, `version`, `shell` fields | P1 | DONE |
| 8 | `context` / `agent` fields | P1 | DONE (fields parsed; fork execution already documented as known-deferred in skill.rs module doc) |
| 9 | `hooks` field | P1 | DONE (field parsed; per-skill hook dispatch deferred) |
| 10 | `hide-from-slash-command-tool` field | P2 | DONE |
| 11 | `created_by` / `improved_by` fields | P2 | DONE |
| 12 | `model` in SkillFrontmatter | P1 | DONE (was missing from struct, now present) |
| 13 | Dynamic skill discovery at FileWrite | P1 | DEFERRED — requires hooking into every file-write tool's execution path; separate cross-crate task |
| 14 | OutputStyle `forceForPlugin` | P2 | DONE (field parsed + stored; plugin-activation enforcement deferred) |
| 15 | Plugin `RawManifest` component fields | P2 | DONE (skills, commands, agents, outputStyles, mcpServers, lspServers, hooks, channels, dependencies, keywords, license, repository all parsed; override of `detect_components()` deferred) |

---

## Files changed

- `lingxi-code/tools/skill/src/skill.rs` — `prompt()` replacement + 9-assertion prompt test
- `lingxi-code/skill-api/src/model.rs` — 14 new `SkillFrontmatter` fields
- `lingxi-code/skill-api/src/frontmatter.rs` — 22 frontmatter parsing tests
- `lingxi-code/skill-api/Cargo.toml` — `serde_json` dependency added
- `lingxi-code/outputstyles/src/disk.rs` — `force_for_plugin` field + 2 tests
- `lingxi-code/plugin/src/discovery.rs` — 12 new `RawManifest` fields
- `lingxi-code/Cargo.lock` — `serde_json` lockfile update

---

## Cross-crate breakage

- `SkillFrontmatter` struct is built with `..Default::default()` spread in `mcp_builders.rs` and `registry.rs` tests — both compile cleanly with new fields because all new fields have `Option`/`bool`/`Vec` defaults via `#[serde(default)]` + `Default` derive.
- `DiskOutputStyle` literal construction in disk.rs tests fixed by adding `force_for_plugin: None`.

---

## Concerns / deferred

1. **Dynamic skill discovery** (activateConditionalSkillsForPaths): requires a file-write hook seam. The `paths:` field is now parsed and stored in `SkillFrontmatter`, but nothing consumes it at runtime. This is a separate feature-sized task.
2. **`hooks` enforcement**: `SkillFrontmatter::hooks` stores raw `serde_json::Value`; parsing it against the full `HooksSchema` and firing the hook lifecycle is deferred.
3. **`context: fork` execution**: `SkillFrontmatter::context`/`agent` are now parsed. Actual forked-agent execution remains the documented non-faithful surface in `skill.rs:21-33`.
4. **Plugin manifest component override**: `RawManifest` now holds the component fields; wiring them to override/augment `detect_components()` results is deferred.
5. **`forceForPlugin` enforcement**: field parsed, but the plugin-activation layer that auto-applies styles when a plugin is enabled has not been changed.
