# Commands + Skills 1:1 Migration Design

Date: 2026-06-12
Status: Approved for implementation

## Summary

This migration closes the local slash-command and file-skill parity gap for
`lingxi-code`. The scope is deliberately limited to LingXi's Rust command
runtime, command utilities, skill listing/loading, the Skill tool prompt seam,
and the TUI/headless commands that have a local implementation seam. It does not
cover `claw-code`, `claude-code/web`, `src/server`, remote-control flows, host
application integrations, billing, browser extensions, or remote services.

Success means external Claude Code style users can discover and run local
commands/skills through LingXi without the commands that have a local seam
falling back to the generic `not implemented in v0.6.0 (M5)` stub.

## Goals

1. Register `.claude/skills/<name>/SKILL.md` directory-format skills as
   markdown slash commands, with `loaded_from = "skills"` and the command name
   derived from the directory name.
2. Expose enough optional slash-command metadata for skill-aware callers:
   `skill_root`, `user_invocable`, `content_length`, and room for future
   optional parity fields without forcing existing commands to populate them.
3. Make `skill-api::load_file_skill_sections` the shared source for headless
   `/skills`, the TUI Skills screen, and listing tests.
4. Ensure `CommandRegistrySkillLoader` treats only directory-format skills, or
   commands explicitly loaded from `skills`, as skill roots. Ordinary
   `.claude/commands/foo.md` files must not become skill base directories.
5. Prefix Skill tool prompt injections with
   `Base directory for this skill: <dir>` for file-backed skills.
6. Replace target local/TUI-only command stubs with real text behavior or a
   reusable interactive-only handler where a full TUI screen is out of scope.
7. Replace the old parity fixture accounting with a matrix that records Claude
   type, Rust status, target status, TUI requirement, and defer reason.

## Non-goals

1. Do not migrate `claw-code` or Claude web/server/remote-control commands.
2. Do not implement commands whose behavior depends on remote billing, host
   applications, browser extensions, desktop app IPC, or unavailable remote
   services in this batch.
3. Do not duplicate TUI UI logic in headless command handlers. Use shared pure
   reducers or return an explicit interactive-only message.
4. Do not introduce new dependencies.

## Architecture

### Command registration

`command-api` owns the command registry data model and markdown command loader.
The loader will scan project and user `.claude/skills/<name>/SKILL.md`
directories, parse them as markdown slash commands, and register them as
`SlashCommandKind::Markdown` with `loaded_from = "skills"`. The user-visible
command name is the directory name. Optional metadata defaults to empty and is
serialized only when present.

`command-core` continues to register the 99 built-in names first, then
overwrites implemented command names with real handlers. The migration adds a
reusable `InteractiveOnlyHandler` for Claude `supportsNonInteractive: false`
local-jsx commands whose TUI screen is not implemented in this batch.

### Skill listing and execution

`skill-api::load_file_skill_sections` is the single listing source. It loads
directory-format skills from project `.claude/skills/` directories from the
current directory up to the nearest git root, then user `~/.claude/skills/`,
deduplicating by canonical `SKILL.md` path and sorting rows by directory name.

The TUI Skills screen and headless `/skills` handler consume this shared data.
Execution still resolves through `CommandRegistry`, so listing and execution
stay consistent once the skills-dir loader registers those commands.

`tool-skill` injects the existing skill content prompt plus a base-directory
prefix for file-backed skills. The command registry skill loader must only set a
`skill_root` for true skills (`loaded_from = "skills"` or directory-format
`SKILL.md`); ordinary markdown commands retain no skill root.

### Parity matrix

The parity test harness moves from the old count-based fixture to per-command
status records:

- `claude_type`
- `rust_status`
- `target_status`
- `requires_tui`
- `defer_reason`

`CORRECT_BY_DESIGN_STUBS` and `HOST_BOUND_DEFERRED_GAPS` remain as named
categories, but no longer serve as the whole state ledger.

## Command batches

### P0: Current diff convergence

Keep the existing `/skills` and `skill-api` listing work, remove unrelated
formatting noise, verify whether `Cargo.lock` is necessary, and switch the TUI
Skills screen to the shared loader.

### P1: Parity matrix upgrade

Replace the old implemented/unimplemented count with the richer per-command
matrix and assert target-implemented commands never return the locked M5 stub.

### P2: Skills registration chain

Implement the `.claude/skills` loader, register it at the command registry
composition root, add Skill tool base-directory prompt context, and lock
`${CLAUDE_SKILL_DIR}`, `${CLAUDE_SESSION_ID}`, args, and shell expansion order.

### P3: Existing/TUI-only commands

Update matrix and handler behavior for `skills`, `stats`, `tasks`, `theme`,
`vim`, `color`, and `copy`. TUI paths keep interactive behavior; headless paths
return equivalent text or the explicit interactive-only result.

### P4: Local migratable commands

Implement or degrade to interactive-only for `add-dir`, `branch`, `diff`,
`rename`, `rewind`, `plan`, `plugin`, `privacy-settings`, `terminal-setup`, and
`usage`, reusing existing TUI or pure reducer seams where available.

### P5: Deferred commands

Faithful stubs stay for Claude-disabled/internal/gated commands. Host-bound
gaps such as `btw`, `x402`, and `reload-plugins` remain deferred. Remote,
billing, browser, host-app, install, upgrade, feedback, and extra-usage command
families enter the matrix only.

## Verification

Run the focused verification set:

```bash
cargo fmt -p command-api -p command-core -p skill-api -p tool-skill -p tui
cargo test -p command-api markdown_loader skill
cargo test -p skill-api listing
cargo test -p tool-skill skill
cargo test -p command-core
cargo test -p tui skills stats
cargo test -p test-harness --test parity_slash_commands
```

Acceptance checks:

- Target-implemented commands do not return
  `not implemented in v0.6.0 (M5)`.
- `.claude/skills/foo/SKILL.md` registers as command `foo`.
- `/skills` lists `foo` through the shared listing source.
- The Skill tool resolves `foo` and injects the base-directory prompt prefix.
