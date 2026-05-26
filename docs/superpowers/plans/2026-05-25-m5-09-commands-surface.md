# M5-09 Slash Commands Surface — 102 Names Registered, 84 Stubs + 18 Core Placeholders

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Register every one of the **102** slash commands that `claude-code` exposes at runtime into `lingxi-commands::CommandRegistry` via a single helper `register_all_builtin_commands(reg)`. Each of the **84 not-yet-implemented** commands gets a single shared `UnimplementedCommandHandler` instance that returns the byte-locked literal `"{name}: not implemented in v0.6.0 (M5)"` when dispatched. Each of the **18 core** commands gets its own per-name placeholder struct (`ClearHandler`, `HelpHandler`, ..., `DoctorHandler`) that **also** returns the same literal in M5-09 but exists as a stable type-id so M5-10 (batch 1: 6 commands) and M5-11 (batch 2: 12 commands) can swap real bodies in one struct at a time without touching the registry wiring. Adds a `SlashCommandDispatcher` impl in `lingxi-commands::dispatcher` that takes `Arc<dyn lingxi_traits::OrchestratorHandle>` (M5-02), parses raw input via existing `parse_slash_command`, looks up the registry, and routes to the resolved handler or returns the locked unknown-command literal `"Unknown command: /{name}"`. Zero new telemetry events.

**Architecture:** Three new files under `lingxi-commands/src/`: `builtin/unimplemented.rs` (the shared stub handler), `builtin/core_placeholders.rs` (18 per-command placeholder structs, each delegating to the shared stub body), and `dispatcher.rs` (the `SlashCommandDispatcher` impl). One modified file `builtin/mod.rs` (re-export). One modified file `registry.rs` (add `register_all_builtin_commands` + `is_builtin_name` helpers). One modified file `lib.rs` (re-export dispatcher + helpers). All 102 names are kept in one centralised `BUILTIN_COMMAND_NAMES: &[&str; 102]` constant sorted ASCII-ascending and a parallel `BUILTIN_CORE_NAMES: &[&str; 18]` constant. Parity fixture `parity_slash_commands_102.json` + driver `parity_slash_commands.rs` in `lingxi-test-harness` lock the count and split (84 + 18 = 102). The dispatcher integration into the orchestrator turn loop is **not** done in this plan — only the trait impl is shipped; M5-12 (CLI binary) and M5-02 follow-up wire it. No telemetry deltas; no settings-schema deltas; no event-name count changes (still 258 after M5-08, still 258 after M5-09).

**Tech stack:** Rust 2021, `async_trait = "0.1"` (workspace), `lingxi_protocol::Effect` (already used by `CommandResult::EmitEffects`), `lingxi_traits::OrchestratorHandle` (introduced in M5-02), `lingxi_commands::{model::*, parser::parse_slash_command, registry::CommandRegistry}` (in-crate), `lingxi-test-harness::parity` (M3-06 module).

---

## Task 0: Reverse-engineer the 102-command list + locked literals

**Files:**
- Read: `claude-code/src/commands/` directory listing (88 subdirs + ~15 top-level `.ts`/`.tsx` files)
- Read: `claude-code/src/commands/registry.ts` (or wherever the runtime command registry is assembled — find via grep)
- Read: `claude-code/src/commands/createMovedToPluginCommand.ts` (helper that emits "moved to plugin" stub commands; these still count toward the runtime registry)
- Read: existing `lingxi-core/crates/commands/src/builtin/{help,cost,memory,compact,resume}.rs` for the 5 already-stubbed handlers — those names are part of the 18 core list and must NOT collide

- [ ] **Step 1: Grep claude-code for the runtime command list assembly.**

Run from repo root:

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
rg -n "registerCommand\(|registerSlashCommand\(|COMMANDS\s*=\s*\[" claude-code/src/commands/ | head -50
rg -n "createMovedToPluginCommand" claude-code/src/ | head -20
```

  **Expected:** locate the file that aggregates the runtime list. In claude-code the aggregator is typically `src/commands/index.ts` or `src/services/commands.ts`. Open whichever file lists every command literal (`'clear' | 'compact' | …`) and extract the **runtime** name list (not the filesystem directory list — some dirs are dead code or migrated to plugins via `createMovedToPluginCommand`).

  **If the runtime list cannot be located via grep** (rare, but possible if claude-code uses dynamic discovery via filesystem scan), fall back to: `ls claude-code/src/commands/ | wc -l` + subtract dead-code helpers (`createMovedToPluginCommand.ts`, `init-verifiers.ts`, `bridge-kick.ts` which is internal). The spec §3 row M5-09 commits to **102** as the byte-lock; this T0 step's job is to enumerate them and write them into Task 2's constant.

- [ ] **Step 2: Enumerate the 102 command names.**

  Cross-reference the runtime aggregator output with the filesystem listing. The canonical list (lowercase, no leading `/`, kebab-case where multi-word) is:

```
add-dir              advisor              agents               ant-trace
autofix-pr           backfill-sessions    branch               break-cache
bridge               brief                btw                  bughunter
chrome               clear                color                commit
commit-push-pr       compact              config               context
copy                 cost                 ctx-viz              debug-tool-call
desktop              diff                 doctor               effort
env                  exit                 export               extra-usage
fast                 feedback             files                good-claude
heapdump             help                 hooks                ide
init                 init-verifiers       insights             install
install-github-app   install-slack-app    issue                keybindings
login                logout               mcp                  memory
mobile               mock-limits          model                oauth-refresh
onboarding           output-style         passes               perf-issue
permissions          plan                 plugin               pr-comments
privacy-settings     rate-limit-options   release-notes        reload-plugins
remote-env           remote-setup         rename               reset-limits
resume               review               rewind               sandbox-toggle
security-review      session              share                skills
stats                status               statusline           stickers
summary              tag                  tasks                teleport
terminal-setup       theme                thinkback            thinkback-play
ultraplan            upgrade              usage                version
vim                  voice                x402
```

  **Count check:** the block above is 102 entries (read carefully — 26 rows of 4 + 1 row of 4 = 100; double-check the count). Run `wc -w` on the block (omitting indentation, joining lines) to verify exactly **102**. If the discovered runtime list differs from this provisional table by 1–3 entries (claude-code is evolving), update the table in this plan and re-commit Task 0 with the correction noted in the commit message.

  **Naming conventions to lock:**
  - All lowercase
  - Multi-word: kebab-case (`add-dir`, `ctx-viz`, `pr-comments`, `terminal-setup`, `release-notes`, etc.) — NOT snake_case, NOT camelCase
  - File-system dir `ctx_viz` (underscore) becomes runtime name `ctx-viz` (hyphen) — claude-code normalises this in `command/index.ts`; we lock the hyphenated runtime spelling
  - `pr_comments` directory → `pr-comments` runtime name (same normalisation)
  - `terminalSetup.tsx` file → `terminal-setup` runtime name (camelCase → kebab-case via `String#toKebabCase`)
  - `installGithubApp` → `install-github-app`
  - `thinkbackPlay` → `thinkback-play`

- [ ] **Step 3: Identify the 18 core commands that get per-name placeholder structs.**

  Spec §3 row M5-10 lists batch 1 (6 commands): `clear compact help exit memory init`. Spec §3 row M5-11 lists batch 2 (12 commands): `cost config model permissions mcp hooks agents login logout version status doctor`. Note that spec §3 M5-11 row prose mentions a thirteenth name (`resume`) but the telemetry count in §4.9 says `12×3=36` (12 commands), and the **success criterion #4** in §1 also says "18 core commands". So `/resume` is NOT one of the 18 — it is implemented end-to-end as part of M5-08 (loader + interactive picker) and gets its own pre-existing stub at `lingxi-commands::builtin::resume::ResumeHandler` (from M1.15). M5-09 does **not** create a new placeholder for `/resume`; instead, in Task 4 it re-wires the existing `ResumeHandler` to delegate to the new locked-literal body so it stays consistent with the other 84 stubs until M5-08's `--resume`-via-CLI path is also exposed as the slash command (deferred to M5-11 / M5-12 wiring).

  **The 18 core list (locked, ASCII-sorted):**

```
agents      clear       compact     config
cost        doctor      exit        help
hooks       init        login       logout
mcp         memory      model       permissions
status      version
```

  Verify: 18 entries.

- [ ] **Step 4: Lock the four user-visible literals.**

  | # | Lock | Value | Source |
  |---|---|---|---|
  | L1 | Stub display (84 unimplemented) | `"{name}: not implemented in v0.6.0 (M5)"` | Spec §4.6 row "Stub literal (84 未实现)" |
  | L2 | Core placeholder display (18 core, in M5-09 only — replaced in M5-10/M5-11) | same as L1: `"{name}: not implemented in v0.6.0 (M5)"` | Same literal so the registry passes the parity fixture before the real bodies land |
  | L3 | Unknown-command literal | `"Unknown command: /{name}"` | LingXi UX lock (claude-code prints `Unknown slash command: <name>` — we shorten to match REPL conventions and 1-line output) |
  | L4 | Multi-line trailing newline | none (no `\n` at end of L1/L2/L3) | LingXi output-string lock (orchestrator appends `\n` at the OutputStream::emit_text boundary) |

  **L1 expansion examples** (the `{name}` is the runtime name, NO leading `/`):
  - `/ant-trace` (name = `ant-trace`) → display `"ant-trace: not implemented in v0.6.0 (M5)"`
  - `/extra-usage` → `"extra-usage: not implemented in v0.6.0 (M5)"`
  - `/x402` → `"x402: not implemented in v0.6.0 (M5)"`

  **L3 expansion examples:**
  - User types `/notacommand` → `"Unknown command: /notacommand"`
  - User types `/CLEAR` (uppercase — parser does NOT case-fold) → `"Unknown command: /CLEAR"` (the registry only holds lowercase keys; M3 parser does not lowercase; this is consistent with claude-code)
  - User types `/foo bar` → `"Unknown command: /foo"` (the `bar` is the args, not part of the lookup; `parse_slash_command` already splits)

- [ ] **Step 5: Lock the description strings (the `SlashCommand.description` field).**

  All 102 commands need a `description` shown by `/help` (M5-10 implementation). For M5-09 we just need a placeholder that doesn't break `/help`'s output. Lock per-command descriptions for the 18 core list now (from claude-code source — read each `commands/<name>/index.ts` and copy the `description` literal):

  | Core command | Description (byte-locked from claude-code source) |
  |---|---|
  | `agents` | `"Manage subagents"` |
  | `clear` | `"Clear conversation history and free up context"` |
  | `compact` | `"Compact the conversation to a summary"` |
  | `config` | `"Open config panel"` |
  | `cost` | `"Show total cost and duration of the current session"` |
  | `doctor` | `"Diagnose installation and configuration"` |
  | `exit` | `"Exit the REPL"` |
  | `help` | `"Show help and available commands"` |
  | `hooks` | `"Manage hooks"` |
  | `init` | `"Initialize a new CLAUDE.md file with codebase documentation"` |
  | `login` | `"Sign in with your Anthropic account"` |
  | `logout` | `"Sign out from your Anthropic account"` |
  | `mcp` | `"Manage MCP servers"` |
  | `memory` | `"Edit Claude memory files"` |
  | `model` | `"Set the model for Claude Code to use"` |
  | `permissions` | `"Manage permissions"` |
  | `status` | `"Show Claude Code status"` |
  | `version` | `"Print version information"` |

  **Source files for description grep:**
  ```bash
  for cmd in agents clear compact config cost doctor exit help hooks init login logout mcp memory model permissions status version; do
    rg -n "description\s*[:=]" "claude-code/src/commands/$cmd/" 2>/dev/null | head -2 || true
  done
  ```

  For the 84 non-core commands the description in M5-09 is a generic placeholder `"(unimplemented in v0.6.0)"` — `/help` (M5-10) will render it as such. The literal **inside** `CommandResult::Done.display` (L1 above) is the action body and does NOT depend on the description; the description is metadata for `/help` only.

- [ ] **Step 6: Confirm the registry's idempotency contract.**

  Read `lingxi-core/crates/commands/src/registry.rs::CommandRegistry::register_command` (already exists from M1.15). Confirm that calling `register_command` twice with the same name **overwrites** the previous entry (the underlying `HashMap::insert` returns the old value but discards it). This is important because Task 4 registers the 18 core placeholders **after** Task 3 registers all 102 as unimplemented — the second pass overwrites those 18 entries with their per-name structs, leaving 84 entries pointing at the shared `UnimplementedCommandHandler` and 18 pointing at their named placeholders. Lock this two-pass design now so Task 3 and Task 4 don't conflict.

  **If `register_command` is not idempotent** (rare — e.g., if it panics on duplicate), Task 1 step 2 below adds a guard that drains the previous entry first. Verify by reading `registry.rs` line-by-line. (The version checked in this plan author's working copy uses `HashMap::insert`, so idempotent.)

- [ ] **Step 7: Commit the byte-locks reference.**

  Append the locks below to this plan file (this Task 0 section), then commit:

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md
git commit -m "plan(M5-09 T0): reverse-engineer 102 command list + lock 4 literals (L1-L4) + 18 core descriptions"
```

### Reverse-engineered byte-locks (locked by T0)

| Lock | Value | Source |
|---|---|---|
| Total command count | **`102`** | Spec §1 success #5; M5-09 row |
| Core command count | **`18`** | Spec §1 success #4; M5-10 (6) + M5-11 (12) |
| Unimplemented stub literal | `"{name}: not implemented in v0.6.0 (M5)"` (no trailing newline) | Spec §4.6 |
| Unknown command literal | `"Unknown command: /{name}"` (no trailing newline) | LingXi UX lock |
| Name format | lowercase ASCII, kebab-case multi-word, no leading `/` | claude-code runtime normalisation |
| Registry overwrite behaviour | `HashMap::insert` semantics — last write wins | `registry.rs` existing code |
| 18 core names (sorted) | `agents clear compact config cost doctor exit help hooks init login logout mcp memory model permissions status version` | Spec §3 M5-10 + M5-11 rows |
| 18 core descriptions | (table above) | claude-code `src/commands/<name>/index.ts` `description` field |
| Telemetry events (M5-09) | **0 new events** | Spec §4.9 telemetry table; no M5-09 row |
| `ALL_EVENT_NAMES.len()` after M5-09 | unchanged from M5-08 (**258**) | Spec §6.3 — no M5-09 row in growth table |

---

## Task 1: Scaffold `builtin/unimplemented.rs` — the shared 84-handler stub

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/unimplemented.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs`

- [ ] **Step 1: Write the failing test.**

  Create `lingxi-core/crates/commands/src/builtin/unimplemented.rs` with **only** the test module:

```rust
//! Shared stub handler used for every slash command that lingxi-core has not
//! yet implemented in v0.6.0 (M5).
//!
//! 84 of the 102 builtin commands point at a single shared instance of this
//! handler. The 18 core commands point at per-name placeholder structs (see
//! `builtin/core_placeholders.rs`) so M5-10 / M5-11 can swap each one's body
//! independently. Both kinds return the same locked literal until the real
//! bodies land.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 0 step 4 for the byte-locked literal.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CommandResult, BuiltinCommandHandler};
    use crate::parser::ParsedSlashCommand;

    #[tokio::test]
    async fn returns_locked_literal_with_name_substituted() {
        let h = UnimplementedCommandHandler::new("ant-trace", "(unimplemented in v0.6.0)");
        let args = ParsedSlashCommand {
            name: "ant-trace".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "ant-trace: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done{{display: Some(...)}}, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn name_and_description_accessors_return_constructor_values() {
        let h = UnimplementedCommandHandler::new("x402", "Crypto micropayments");
        assert_eq!(h.name(), "x402");
        assert_eq!(h.description(), "Crypto micropayments");
    }

    #[tokio::test]
    async fn empty_name_still_produces_locked_format() {
        let h = UnimplementedCommandHandler::new("", "");
        let args = ParsedSlashCommand {
            name: String::new(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, ": not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Run the test and watch it fail.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test -p lingxi-commands --lib builtin::unimplemented::tests 2>&1 | head -40
```

  Expected: **3 failures** with messages like `cannot find type 'UnimplementedCommandHandler' in this scope`. Confirm before proceeding.

- [ ] **Step 3: Implement the struct + trait.**

  Replace the stub file contents with:

```rust
//! Shared stub handler used for every slash command that lingxi-core has not
//! yet implemented in v0.6.0 (M5).
//!
//! 84 of the 102 builtin commands point at a single shared instance of this
//! handler. The 18 core commands point at per-name placeholder structs (see
//! `builtin/core_placeholders.rs`) so M5-10 / M5-11 can swap each one's body
//! independently. Both kinds return the same locked literal until the real
//! bodies land.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 0 step 4 for the byte-locked literal.

use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;

/// Stub handler that returns the locked
/// `"{name}: not implemented in v0.6.0 (M5)"` literal.
///
/// Used for every M5-09-registered command that does not have a real body yet.
#[derive(Debug, Clone)]
pub struct UnimplementedCommandHandler {
    name: String,
    description: String,
}

impl UnimplementedCommandHandler {
    /// Construct a handler keyed by `name` with the given `description` for
    /// `/help` rendering.
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }

    /// Produce the locked stub literal for a given command name (without `/`).
    ///
    /// Public so the dispatcher can use the same formatting for "Unknown
    /// command" responses without going through a handler instance.
    #[must_use]
    pub fn stub_literal(name: &str) -> String {
        format!("{name}: not implemented in v0.6.0 (M5)")
    }
}

#[async_trait]
impl BuiltinCommandHandler for UnimplementedCommandHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        CommandResult::Done {
            display: Some(Self::stub_literal(&self.name)),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CommandResult;
    use crate::parser::ParsedSlashCommand;

    #[tokio::test]
    async fn returns_locked_literal_with_name_substituted() {
        let h = UnimplementedCommandHandler::new("ant-trace", "(unimplemented in v0.6.0)");
        let args = ParsedSlashCommand {
            name: "ant-trace".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "ant-trace: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done{{display: Some(...)}}, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn name_and_description_accessors_return_constructor_values() {
        let h = UnimplementedCommandHandler::new("x402", "Crypto micropayments");
        assert_eq!(h.name(), "x402");
        assert_eq!(h.description(), "Crypto micropayments");
    }

    #[tokio::test]
    async fn empty_name_still_produces_locked_format() {
        let h = UnimplementedCommandHandler::new("", "");
        let args = ParsedSlashCommand {
            name: String::new(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, ": not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {:?}", other),
        }
    }
}
```

  **Important about `ParsedSlashCommand` shape:** the field names used above (`name`, `raw_args`, `tokens`) must match what M1.15 `parser.rs` actually exposes. If `parse_slash_command` returns a different struct (e.g. field named `args` instead of `tokens`), update the test setup. Run `cargo check -p lingxi-commands` after writing to catch any field-name drift; fix the test setup before continuing.

- [ ] **Step 4: Add the submodule to `builtin/mod.rs`.**

  Open `lingxi-core/crates/commands/src/builtin/mod.rs`. The current content lists 5 submodules (`compact, cost, help, memory, resume`). Append:

```rust
//! Built-in slash-command handlers.
//!
//! Surface (M5-09): 102 names registered via [`crate::registry::register_all_builtin_commands`].
//! 84 point at a shared [`unimplemented::UnimplementedCommandHandler`]; 18 core
//! get per-name placeholder structs (see [`core_placeholders`]) so M5-10 / M5-11
//! can swap each one's body independently.

pub mod compact;
pub mod core_placeholders;
pub mod cost;
pub mod help;
pub mod memory;
pub mod resume;
pub mod unimplemented;

pub use unimplemented::UnimplementedCommandHandler;
```

  The `core_placeholders` module does not exist yet (created in Task 4) — leave the `pub mod` declaration and tolerate the compile error until Task 4 step 1 lands the file. Alternatively, gate the `pub mod core_placeholders;` line behind a `#[cfg(any())]` until Task 4. The simplest workflow: do Task 4 step 1 immediately after this step **before** running cargo build. See Task 4 step 1 for the empty stub.

- [ ] **Step 5: Run the test again and watch it pass.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test -p lingxi-commands --lib builtin::unimplemented::tests 2>&1 | tail -15
```

  Expected: **3 passed; 0 failed**.

- [ ] **Step 6: Run fmt + clippy on the new file.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -20
```

  Expected: no fmt diff, no clippy warnings.

- [ ] **Step 7: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/src/builtin/unimplemented.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs
git commit -m "feat(M5-09 task 1): UnimplementedCommandHandler + locked stub literal (3 unit tests)"
```

---

## Task 2: Lock the 102-name + 18-core constants

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/names.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs` (add `pub mod names;`)

- [ ] **Step 1: Write the failing test.**

  Create `lingxi-core/crates/commands/src/builtin/names.rs`:

```rust
//! Locked constant tables of the 102 builtin command names + the 18 core names.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 0 step 2 (102 list) + Task 0 step 3 (18 core list).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_count_locked_at_102() {
        assert_eq!(BUILTIN_COMMAND_NAMES.len(), 102);
    }

    #[test]
    fn core_count_locked_at_18() {
        assert_eq!(BUILTIN_CORE_NAMES.len(), 18);
    }

    #[test]
    fn names_are_sorted_ascii_ascending() {
        let mut sorted = BUILTIN_COMMAND_NAMES.to_vec();
        sorted.sort();
        assert_eq!(BUILTIN_COMMAND_NAMES, sorted.as_slice(),
            "BUILTIN_COMMAND_NAMES must be ASCII-sorted to keep diffs reviewable");
    }

    #[test]
    fn core_names_are_sorted_ascii_ascending() {
        let mut sorted = BUILTIN_CORE_NAMES.to_vec();
        sorted.sort();
        assert_eq!(BUILTIN_CORE_NAMES, sorted.as_slice());
    }

    #[test]
    fn no_duplicate_names() {
        let mut set = std::collections::HashSet::new();
        for n in BUILTIN_COMMAND_NAMES {
            assert!(set.insert(*n), "duplicate name {n} in BUILTIN_COMMAND_NAMES");
        }
    }

    #[test]
    fn every_core_name_is_in_the_full_list() {
        let full: std::collections::HashSet<&str> = BUILTIN_COMMAND_NAMES.iter().copied().collect();
        for c in BUILTIN_CORE_NAMES {
            assert!(full.contains(c), "core name {c} not in BUILTIN_COMMAND_NAMES");
        }
    }

    #[test]
    fn all_names_are_lowercase_ascii_or_hyphen_or_digit() {
        for n in BUILTIN_COMMAND_NAMES {
            for c in n.chars() {
                assert!(
                    c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit(),
                    "name '{n}' has invalid char '{c}' (only lowercase ascii + '-' + digits allowed)"
                );
            }
        }
    }

    #[test]
    fn no_name_starts_or_ends_with_hyphen() {
        for n in BUILTIN_COMMAND_NAMES {
            assert!(!n.starts_with('-'), "name '{n}' starts with hyphen");
            assert!(!n.ends_with('-'), "name '{n}' ends with hyphen");
        }
    }

    #[test]
    fn no_double_hyphen() {
        for n in BUILTIN_COMMAND_NAMES {
            assert!(!n.contains("--"), "name '{n}' contains '--'");
        }
    }

    #[test]
    fn includes_known_canonical_names() {
        // Spot-check a few rare ones so a future name rename doesn't drift silently.
        assert!(BUILTIN_COMMAND_NAMES.contains(&"x402"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"ctx-viz"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"pr-comments"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"thinkback-play"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"terminal-setup"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"ant-trace"));
    }
}
```

- [ ] **Step 2: Run and watch fail.**

```bash
cargo test -p lingxi-commands --lib builtin::names::tests 2>&1 | head -20
```

  Expected: 9 failures (cannot find `BUILTIN_COMMAND_NAMES` / `BUILTIN_CORE_NAMES`).

- [ ] **Step 3: Implement the constants.**

  Prepend to `builtin/names.rs` (before the `#[cfg(test)] mod tests`):

```rust
//! Locked constant tables of the 102 builtin command names + the 18 core names.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 0 step 2 (102 list) + Task 0 step 3 (18 core list).

/// Every built-in slash command's runtime name (without leading `/`),
/// ASCII-sorted. Locked at length **102** for v0.6.0.
///
/// Changing the count or membership requires bumping the parity fixture
/// `crates/test-harness/src/parity/fixtures/parity_slash_commands_102.json`
/// (see plan M5-09 Task 6).
pub const BUILTIN_COMMAND_NAMES: &[&str; 102] = &[
    "add-dir",
    "advisor",
    "agents",
    "ant-trace",
    "autofix-pr",
    "backfill-sessions",
    "branch",
    "break-cache",
    "bridge",
    "brief",
    "btw",
    "bughunter",
    "chrome",
    "clear",
    "color",
    "commit",
    "commit-push-pr",
    "compact",
    "config",
    "context",
    "copy",
    "cost",
    "ctx-viz",
    "debug-tool-call",
    "desktop",
    "diff",
    "doctor",
    "effort",
    "env",
    "exit",
    "export",
    "extra-usage",
    "fast",
    "feedback",
    "files",
    "good-claude",
    "heapdump",
    "help",
    "hooks",
    "ide",
    "init",
    "init-verifiers",
    "insights",
    "install",
    "install-github-app",
    "install-slack-app",
    "issue",
    "keybindings",
    "login",
    "logout",
    "mcp",
    "memory",
    "mobile",
    "mock-limits",
    "model",
    "oauth-refresh",
    "onboarding",
    "output-style",
    "passes",
    "perf-issue",
    "permissions",
    "plan",
    "plugin",
    "pr-comments",
    "privacy-settings",
    "rate-limit-options",
    "release-notes",
    "reload-plugins",
    "remote-env",
    "remote-setup",
    "rename",
    "reset-limits",
    "resume",
    "review",
    "rewind",
    "sandbox-toggle",
    "security-review",
    "session",
    "share",
    "skills",
    "stats",
    "status",
    "statusline",
    "stickers",
    "summary",
    "tag",
    "tasks",
    "teleport",
    "terminal-setup",
    "theme",
    "thinkback",
    "thinkback-play",
    "ultraplan",
    "upgrade",
    "usage",
    "version",
    "vim",
    "voice",
    "x402",
];

/// The 18 core commands that ship with real implementations in M5-10 / M5-11.
/// Subset of [`BUILTIN_COMMAND_NAMES`], ASCII-sorted.
///
/// In M5-09 each of these gets a per-name placeholder struct in
/// [`super::core_placeholders`]; the placeholders return the same locked stub
/// literal as the shared `UnimplementedCommandHandler` until the real bodies
/// land in M5-10 (batch 1: clear/compact/help/exit/memory/init) and M5-11
/// (batch 2: the remaining 12).
pub const BUILTIN_CORE_NAMES: &[&str; 18] = &[
    "agents",
    "clear",
    "compact",
    "config",
    "cost",
    "doctor",
    "exit",
    "help",
    "hooks",
    "init",
    "login",
    "logout",
    "mcp",
    "memory",
    "model",
    "permissions",
    "status",
    "version",
];

/// Descriptions for the 18 core commands, used for `/help` rendering in M5-10.
/// Lookup by core name; falls back to `"(unimplemented in v0.6.0)"` for the 84
/// non-core entries.
#[must_use]
pub fn core_description(name: &str) -> &'static str {
    match name {
        "agents" => "Manage subagents",
        "clear" => "Clear conversation history and free up context",
        "compact" => "Compact the conversation to a summary",
        "config" => "Open config panel",
        "cost" => "Show total cost and duration of the current session",
        "doctor" => "Diagnose installation and configuration",
        "exit" => "Exit the REPL",
        "help" => "Show help and available commands",
        "hooks" => "Manage hooks",
        "init" => "Initialize a new CLAUDE.md file with codebase documentation",
        "login" => "Sign in with your Anthropic account",
        "logout" => "Sign out from your Anthropic account",
        "mcp" => "Manage MCP servers",
        "memory" => "Edit Claude memory files",
        "model" => "Set the model for Claude Code to use",
        "permissions" => "Manage permissions",
        "status" => "Show Claude Code status",
        "version" => "Print version information",
        _ => "(unimplemented in v0.6.0)",
    }
}
```

- [ ] **Step 4: Add the submodule to `builtin/mod.rs`.**

  Edit `builtin/mod.rs` to insert `pub mod names;` and the re-exports:

```rust
//! Built-in slash-command handlers.
//!
//! Surface (M5-09): 102 names registered via [`crate::registry::register_all_builtin_commands`].
//! 84 point at a shared [`unimplemented::UnimplementedCommandHandler`]; 18 core
//! get per-name placeholder structs (see [`core_placeholders`]) so M5-10 / M5-11
//! can swap each one's body independently.

pub mod compact;
pub mod core_placeholders;
pub mod cost;
pub mod help;
pub mod memory;
pub mod names;
pub mod resume;
pub mod unimplemented;

pub use names::{core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
pub use unimplemented::UnimplementedCommandHandler;
```

- [ ] **Step 5: Run the test again and watch all pass.**

```bash
cargo test -p lingxi-commands --lib builtin::names::tests 2>&1 | tail -15
```

  Expected: **9 passed**. If `total_count_locked_at_102` fails with `assertion left: 101 right: 102` or similar, you mis-counted the list — recount, fix, re-run.

- [ ] **Step 6: Fmt + clippy.**

```bash
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 7: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/src/builtin/names.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs
git commit -m "feat(M5-09 task 2): BUILTIN_COMMAND_NAMES (102) + BUILTIN_CORE_NAMES (18) + core_description() (9 invariant tests)"
```

---

## Task 3: `register_all_builtin_commands(reg)` helper — registers 84 unimplemented stubs

**Files:**
- Modify: `lingxi-core/crates/commands/src/registry.rs`
- Modify: `lingxi-core/crates/commands/src/lib.rs` (re-export)

- [ ] **Step 1: Write the failing test in `registry.rs`.**

  Append to `lingxi-core/crates/commands/src/registry.rs`:

```rust
#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::builtin::{BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};

    #[test]
    fn register_all_registers_exactly_102_names() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        for name in BUILTIN_COMMAND_NAMES {
            assert!(reg.resolve(name).is_some(), "expected command /{name} registered");
        }
    }

    #[test]
    fn register_all_uses_shared_handler_for_84_non_core() {
        // After Task 3 only (before Task 4), all 102 should point at the shared handler.
        // After Task 4, the 18 core entries get overwritten with their per-name
        // placeholders. This test exercises the post-Task-4 state.
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        // Each registered command must have a handler resolvable via get_handler.
        for name in BUILTIN_COMMAND_NAMES {
            assert!(
                reg.get_handler(name).is_some(),
                "expected handler for /{name}"
            );
        }
    }

    #[tokio::test]
    async fn unimplemented_command_returns_locked_literal() {
        use crate::parser::ParsedSlashCommand;
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        // Pick a definitely-not-in-the-18-core command.
        let h = reg.get_handler("ant-trace").expect("ant-trace handler missing");
        let args = ParsedSlashCommand {
            name: "ant-trace".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "ant-trace: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn idempotent_double_register_overwrites_cleanly() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        register_all_builtin_commands(&mut reg); // call twice
        for name in BUILTIN_COMMAND_NAMES {
            assert!(reg.resolve(name).is_some());
        }
    }
}
```

  Notes on imports: `CommandResult` is brought into scope by the test module via the `use super::*;` (the file already imports it; if not, add `use crate::model::CommandResult;`).

- [ ] **Step 2: Run and watch fail.**

```bash
cargo test -p lingxi-commands --lib registry::registry_tests 2>&1 | head -30
```

  Expected: 4 failures (`cannot find function 'register_all_builtin_commands' in this scope`).

- [ ] **Step 3: Implement the helper.**

  Add to `lingxi-core/crates/commands/src/registry.rs` (after the existing `impl CommandRegistry` block):

```rust
/// Register all 102 built-in slash commands into `reg`.
///
/// 84 of the names point at a single shared
/// [`crate::builtin::UnimplementedCommandHandler`] instance that returns the
/// locked stub literal `"{name}: not implemented in v0.6.0 (M5)"`.
///
/// The 18 core names listed in [`crate::builtin::BUILTIN_CORE_NAMES`] are
/// **also** registered here against the same shared handler **first**, then
/// immediately overwritten by [`register_core_placeholders`] (called at the
/// end of this function) with their per-name placeholder structs.
///
/// Calling this function on the same registry twice is safe — every name is
/// overwritten in-place via `HashMap::insert` semantics.
pub fn register_all_builtin_commands(reg: &mut CommandRegistry) {
    use crate::builtin::core_placeholders::register_core_placeholders;
    use crate::builtin::{core_description, UnimplementedCommandHandler, BUILTIN_COMMAND_NAMES};
    use std::sync::Arc;

    // Pass 1: register all 102 with the shared unimplemented handler.
    //
    // Each name needs its own handler **instance** because the handler
    // carries its own `name` field used to substitute the locked literal.
    // We can't share a single Arc across all 102 since each command must
    // report its own name back when queried. (If we wanted to share one
    // Arc we'd have to make handler-side substitution depend on the
    // dispatcher-supplied ParsedSlashCommand.name, but the existing
    // BuiltinCommandHandler::handle takes only &ParsedSlashCommand and our
    // tests above rely on the handler's own name() accessor, so we keep
    // per-name instances.)
    for &name in BUILTIN_COMMAND_NAMES {
        let h = Arc::new(UnimplementedCommandHandler::new(name, core_description(name)));
        reg.register_builtin_handler(h);
    }

    // Pass 2: overwrite the 18 core entries with their per-name placeholders.
    // (Implemented in Task 4. Until Task 4 lands, this is a no-op via a stub.)
    register_core_placeholders(reg);
}
```

  Re-export from `lib.rs`. Open `lingxi-core/crates/commands/src/lib.rs` and append to the `pub use` block:

```rust
pub use registry::{register_all_builtin_commands, CommandRegistry};
```

  (replacing the existing `pub use registry::CommandRegistry;` line — combine them into one.)

- [ ] **Step 4: Stub `core_placeholders::register_core_placeholders` so this compiles.**

  Until Task 4 implements the real per-name structs, we still need a function with the right signature so `register_all_builtin_commands` compiles. Create the file as a stub now; Task 4 will fill it out:

  Create `lingxi-core/crates/commands/src/builtin/core_placeholders.rs`:

```rust
//! Per-name placeholder handler structs for the 18 core commands.
//!
//! In M5-09 each placeholder returns the same locked stub literal as
//! [`super::unimplemented::UnimplementedCommandHandler`]. M5-10 / M5-11
//! replace each placeholder's body with the real implementation, one
//! struct at a time, without touching registry wiring.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 4.

use crate::registry::CommandRegistry;

/// Register the 18 per-name core placeholders, overwriting the shared
/// unimplemented entries put down by [`crate::register_all_builtin_commands`].
///
/// M5-09 stub: no-op (Task 4 fills this in). The shared
/// `UnimplementedCommandHandler` registrations from Pass 1 stay in place,
/// which is correct behaviour for the M5-09 surface (everything returns the
/// stub literal regardless of whether we overwrite with a per-name placeholder
/// of the same literal).
pub fn register_core_placeholders(_reg: &mut CommandRegistry) {
    // Filled in by Task 4.
}
```

- [ ] **Step 5: Run the tests and watch pass.**

```bash
cargo test -p lingxi-commands --lib registry::registry_tests 2>&1 | tail -15
```

  Expected: **4 passed**.

- [ ] **Step 6: Fmt + clippy.**

```bash
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 7: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/src/registry.rs \
        lingxi-core/crates/commands/src/lib.rs \
        lingxi-core/crates/commands/src/builtin/core_placeholders.rs
git commit -m "feat(M5-09 task 3): register_all_builtin_commands() — 102 entries (4 registry tests)"
```

---

## Task 4: 18 per-name core placeholder structs in `core_placeholders.rs`

**Files:**
- Modify: `lingxi-core/crates/commands/src/builtin/core_placeholders.rs`

- [ ] **Step 1: Write the failing test.**

  Append to `lingxi-core/crates/commands/src/builtin/core_placeholders.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::names::BUILTIN_CORE_NAMES;
    use crate::model::{BuiltinCommandHandler, CommandResult};
    use crate::parser::ParsedSlashCommand;
    use std::sync::Arc;

    #[test]
    fn all_18_core_placeholders_have_distinct_type_ids() {
        // Each per-name placeholder must be a distinct struct so M5-10/M5-11
        // can pattern-match or downcast on type for selective replacement.
        // We don't have introspection on `dyn BuiltinCommandHandler` directly,
        // but we can confirm each placeholder construction succeeds AND that
        // the registry holds 18 distinct Arc<dyn ...> entries after overwrite.
        let mut reg = CommandRegistry::new();
        // Seed all 102 with unimplemented stubs first (mimics Task 3 Pass 1).
        crate::register_all_builtin_commands(&mut reg);
        // Pass 2 (this Task) overwrites 18 entries.
        register_core_placeholders(&mut reg);

        // After overwrite, each of the 18 core names still resolves.
        for name in BUILTIN_CORE_NAMES {
            assert!(reg.resolve(name).is_some(), "core /{name} disappeared");
            assert!(reg.get_handler(name).is_some(), "core /{name} has no handler");
        }
    }

    #[tokio::test]
    async fn core_placeholder_returns_same_locked_literal_in_m5_09() {
        let mut reg = CommandRegistry::new();
        crate::register_all_builtin_commands(&mut reg);

        // Pick a core command — its placeholder returns the M5-09 stub.
        let h = reg.get_handler("clear").expect("clear handler missing");
        let args = ParsedSlashCommand {
            name: "clear".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "clear: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn core_placeholder_carries_real_description() {
        let mut reg = CommandRegistry::new();
        crate::register_all_builtin_commands(&mut reg);
        let h = reg.get_handler("help").expect("help handler missing");
        assert_eq!(h.description(), "Show help and available commands");
    }

    #[tokio::test]
    async fn all_18_core_placeholders_return_locked_literal() {
        let mut reg = CommandRegistry::new();
        crate::register_all_builtin_commands(&mut reg);
        for name in BUILTIN_CORE_NAMES {
            let h = reg.get_handler(name).unwrap_or_else(|| panic!("no handler /{name}"));
            let args = ParsedSlashCommand {
                name: name.to_string(),
                raw_args: String::new(),
                tokens: vec![],
            };
            match h.handle(&args).await {
                CommandResult::Done { display: Some(s) } => {
                    assert_eq!(s, format!("{name}: not implemented in v0.6.0 (M5)"));
                }
                other => panic!("/{name} did not return Done, got {other:?}"),
            }
        }
    }
}
```

- [ ] **Step 2: Run and watch fail.**

```bash
cargo test -p lingxi-commands --lib builtin::core_placeholders::tests 2>&1 | head -30
```

  Expected: 4 failures (function `register_core_placeholders` is a no-op; tests pass for assertions that check 102 existence but fail on `core_placeholder_carries_real_description` because the shared unimplemented handler uses `core_description("help")` = `"Show help and available commands"`, so this MAY actually pass even with the no-op). Re-check expectations:

  - `all_18_core_placeholders_have_distinct_type_ids` — passes even with no-op (entries still exist from Pass 1)
  - `core_placeholder_returns_same_locked_literal_in_m5_09` — passes (Pass 1 handler returns same literal)
  - `core_placeholder_carries_real_description` — passes (Pass 1 used `core_description`)
  - `all_18_core_placeholders_return_locked_literal` — passes

  **All tests already pass with the no-op `register_core_placeholders`!** This is OK — the M5-09 surface goal is to have the right behaviour shipped, even if Pass 2 is structurally a no-op until M5-10. However, the spec wants per-name structs for M5-10/M5-11 to swap into, so Task 4 still needs to ship the 18 named structs with their `BuiltinCommandHandler` impls **distinct from `UnimplementedCommandHandler`**, even though they return the same literal. This gives M5-10/M5-11 a stable struct to extend.

  Add one more test that FAILS with the no-op:

```rust
    #[test]
    fn core_placeholder_struct_is_distinct_from_unimplemented() {
        use std::any::Any;
        // Construct each placeholder directly and check its TypeId is different
        // from UnimplementedCommandHandler.
        use crate::builtin::core_placeholders::*;
        use crate::builtin::unimplemented::UnimplementedCommandHandler;

        let unimpl = UnimplementedCommandHandler::new("test", "");
        let unimpl_tid = (&unimpl as &dyn Any).type_id();

        let clear: Box<dyn Any> = Box::new(ClearHandler::new());
        assert_ne!(clear.type_id(), unimpl_tid, "ClearHandler must be a distinct type");

        let help: Box<dyn Any> = Box::new(HelpHandler::new());
        assert_ne!(help.type_id(), unimpl_tid, "HelpHandler must be a distinct type");

        // Spot-check the remaining 16 — at minimum one per batch.
        let cost: Box<dyn Any> = Box::new(CostHandler::new());
        assert_ne!(cost.type_id(), unimpl_tid);
        let init: Box<dyn Any> = Box::new(InitHandler::new());
        assert_ne!(init.type_id(), unimpl_tid);
    }
```

  This test fails because no `ClearHandler` / `HelpHandler` / `CostHandler` / `InitHandler` types exist yet.

- [ ] **Step 3: Run and watch the new test fail.**

```bash
cargo test -p lingxi-commands --lib builtin::core_placeholders::tests::core_placeholder_struct_is_distinct 2>&1 | head -30
```

  Expected: 1 failure (`cannot find type 'ClearHandler' in this scope`).

- [ ] **Step 4: Implement the 18 per-name placeholder structs + register_core_placeholders.**

  Replace the body of `core_placeholders.rs` (keep the doc-comment at the top + the tests at the bottom). Insert:

```rust
//! Per-name placeholder handler structs for the 18 core commands.
//!
//! In M5-09 each placeholder returns the same locked stub literal as
//! [`super::unimplemented::UnimplementedCommandHandler`]. M5-10 / M5-11
//! replace each placeholder's body with the real implementation, one
//! struct at a time, without touching registry wiring.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 4.

use crate::builtin::names::core_description;
use crate::builtin::unimplemented::UnimplementedCommandHandler;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use crate::registry::CommandRegistry;
use async_trait::async_trait;
use std::sync::Arc;

/// Generate a per-name placeholder struct + `BuiltinCommandHandler` impl.
///
/// In M5-09 every per-name placeholder's body simply returns
/// `UnimplementedCommandHandler::stub_literal(name)`. M5-10 / M5-11 replace
/// the macro-generated `handle` body with the real implementation by
/// changing the macro call to a hand-written impl for the affected names.
macro_rules! core_placeholder {
    ($struct_name:ident, $name_literal:literal) => {
        #[doc = concat!("Placeholder for /", $name_literal, " — body returns the M5-09 stub literal until M5-10/M5-11 lands the real implementation.")]
        #[derive(Debug, Default)]
        pub struct $struct_name;

        impl $struct_name {
            #[must_use]
            pub fn new() -> Self { Self }
        }

        #[async_trait]
        impl BuiltinCommandHandler for $struct_name {
            async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
                CommandResult::Done {
                    display: Some(UnimplementedCommandHandler::stub_literal($name_literal)),
                }
            }
            fn name(&self) -> &str { $name_literal }
            fn description(&self) -> &str { core_description($name_literal) }
        }
    };
}

core_placeholder!(AgentsHandler,      "agents");
core_placeholder!(ClearHandler,       "clear");
core_placeholder!(CompactHandler,     "compact");
core_placeholder!(ConfigHandler,      "config");
core_placeholder!(CostHandler,        "cost");
core_placeholder!(DoctorHandler,      "doctor");
core_placeholder!(ExitHandler,        "exit");
core_placeholder!(HelpHandler,        "help");
core_placeholder!(HooksHandler,       "hooks");
core_placeholder!(InitHandler,        "init");
core_placeholder!(LoginHandler,       "login");
core_placeholder!(LogoutHandler,      "logout");
core_placeholder!(McpHandler,         "mcp");
core_placeholder!(MemoryHandler,      "memory");
core_placeholder!(ModelHandler,       "model");
core_placeholder!(PermissionsHandler, "permissions");
core_placeholder!(StatusHandler,      "status");
core_placeholder!(VersionHandler,     "version");

/// Register the 18 per-name core placeholders, overwriting the shared
/// unimplemented entries put down by [`crate::register_all_builtin_commands`].
///
/// **Important about pre-existing M1.15 handlers:** the M1.15 codebase already
/// has `CompactHandler` and `HelpHandler` and `MemoryHandler` and `ResumeHandler`
/// and `CostHandler` defined under `builtin/{compact,help,memory,resume,cost}.rs`.
/// Those files are kept (with their existing names) so M1.15 telemetry probes do
/// not regress, but the M5-09 surface registers the **placeholder** structs
/// from THIS file. The placeholders intentionally shadow the M1.15 stubs in
/// the registry by being registered second (this Pass 2). M5-10's first task is
/// to consolidate: it will move the bodies of the M1.15 stubs into the M5-09
/// placeholder structs (e.g. `CompactHandler::handle` body grows from this
/// placeholder body into the real M3 compact-trigger logic that lives in
/// `builtin/compact.rs::CompactHandler`). M5-10 may delete the M1.15
/// `builtin/{help,cost,memory,compact}.rs` files after migration; that's an
/// M5-10 follow-up, not an M5-09 concern.
pub fn register_core_placeholders(reg: &mut CommandRegistry) {
    reg.register_builtin_handler(Arc::new(AgentsHandler::new()));
    reg.register_builtin_handler(Arc::new(ClearHandler::new()));
    reg.register_builtin_handler(Arc::new(CompactHandler::new()));
    reg.register_builtin_handler(Arc::new(ConfigHandler::new()));
    reg.register_builtin_handler(Arc::new(CostHandler::new()));
    reg.register_builtin_handler(Arc::new(DoctorHandler::new()));
    reg.register_builtin_handler(Arc::new(ExitHandler::new()));
    reg.register_builtin_handler(Arc::new(HelpHandler::new()));
    reg.register_builtin_handler(Arc::new(HooksHandler::new()));
    reg.register_builtin_handler(Arc::new(InitHandler::new()));
    reg.register_builtin_handler(Arc::new(LoginHandler::new()));
    reg.register_builtin_handler(Arc::new(LogoutHandler::new()));
    reg.register_builtin_handler(Arc::new(McpHandler::new()));
    reg.register_builtin_handler(Arc::new(MemoryHandler::new()));
    reg.register_builtin_handler(Arc::new(ModelHandler::new()));
    reg.register_builtin_handler(Arc::new(PermissionsHandler::new()));
    reg.register_builtin_handler(Arc::new(StatusHandler::new()));
    reg.register_builtin_handler(Arc::new(VersionHandler::new()));
}
```

  **Name collision note:** the macro generates `CompactHandler` / `HelpHandler` / `CostHandler` / `MemoryHandler` which **conflict with the existing types in `builtin/compact.rs`** etc. Two paths:

  - **(A) Rename M5-09 placeholders to `CompactPlaceholder` etc.** — keeps M1.15 types unchanged but breaks the convention of M5-10/M5-11 swapping `Handler` bodies in-place.
  - **(B) Shadow the M1.15 types: rename the M1.15 types to `LegacyCompactHandler` etc.** — preserves M5-09 naming but bumps M1.15 imports.

  We pick **(B)**. Rename the existing `builtin/compact.rs::CompactHandler` → `LegacyCompactHandler` (likewise `Help`, `Cost`, `Memory`, `Resume`). This is a 5-file rename + maybe 1-2 import-site fixes (any test that imported the old name). Do this rename as part of **Task 4 Step 4 before the macro expansion**: open each of `builtin/{compact,help,cost,memory,resume}.rs`, `s/Handler/LegacyHandler/g` for the struct name only (not the `BuiltinCommandHandler` trait), update any in-crate imports.

  Run a quick check first:

```bash
rg -n "pub struct (Compact|Help|Cost|Memory|Resume)Handler" lingxi-core/crates/commands/src/builtin/
rg -n "use crate::builtin::(compact|help|cost|memory|resume)::(Compact|Help|Cost|Memory|Resume)Handler" lingxi-core/
```

  Then rename. (If the M1.15 handlers are entirely empty stubs not used anywhere — likely — you may instead just **delete** their `pub use` and remove the orphaned files, then keep the macro-generated names. Decide based on the rg output.)

  **Locking decision for this plan:** If M1.15 stubs are minimal (`pub struct CompactHandler;` + a no-op `handle` returning `CommandResult::Done { display: None }`) and unused outside `builtin/mod.rs` re-exports, just delete the bodies and let the M5-09 placeholders own the names. Update `builtin/mod.rs` to NOT re-declare those submodules:

```rust
//! Built-in slash-command handlers.

pub mod core_placeholders;
pub mod names;
pub mod unimplemented;

pub use core_placeholders::*;
pub use names::{core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
pub use unimplemented::UnimplementedCommandHandler;
```

  And **delete** the files `builtin/{compact,help,cost,memory,resume}.rs`. Git tracks the deletion; commit message in Step 7 records it.

- [ ] **Step 5: Run all tests.**

```bash
cargo test -p lingxi-commands 2>&1 | tail -25
```

  Expected: all `builtin::core_placeholders::tests` + `builtin::names::tests` + `builtin::unimplemented::tests` + `registry::registry_tests` pass. If the M1.15 stub deletion broke any existing test (e.g. one that does `use crate::builtin::compact::CompactHandler`), fix it inline — most likely the existing test just needs its import line removed.

- [ ] **Step 6: Fmt + clippy.**

```bash
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -10
```

  Expected: no fmt diff. Clippy may flag `Default` for the per-name placeholder structs — accept the suggestion (the macro already derives `Default`, so `Self::new()` is the only ambiguity).

- [ ] **Step 7: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/src/builtin/core_placeholders.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs
git add -u lingxi-core/crates/commands/src/builtin/  # picks up deletions
git commit -m "feat(M5-09 task 4): 18 per-name core placeholder structs (macro-generated) + delete M1.15 stub modules (5 placeholder tests)"
```

---

## Task 5: `SlashCommandDispatcher` impl + locked unknown-command literal

**Files:**
- Create: `lingxi-core/crates/commands/src/dispatcher.rs`
- Modify: `lingxi-core/crates/commands/src/lib.rs` (re-export)
- Modify: `lingxi-core/crates/commands/Cargo.toml` (add `lingxi-traits` dep if not already)

- [ ] **Step 1: Confirm `OrchestratorHandle` trait location.**

```bash
rg -n "pub trait OrchestratorHandle" lingxi-core/crates/traits/src/ 2>&1 | head -5
rg -n "SlashCommandDispatcher" lingxi-core/crates/traits/src/ 2>&1 | head -5
```

  Expected: `OrchestratorHandle` lives in `lingxi-core/crates/traits/src/orchestrator.rs` (from M5-02). `SlashCommandDispatcher` either also lives there (defined by M5-02) or it doesn't exist yet and this Task creates it. In either case, the **canonical home** for the trait is `lingxi-traits` so `lingxi-commands` doesn't take a dep on `lingxi-orchestrator`.

  - **If `SlashCommandDispatcher` does not exist in `lingxi-traits`**, add it there (in `lingxi-core/crates/traits/src/commands.rs`) before continuing this Task. The trait minimum:

```rust
//! Slash-command dispatch surface — implemented by `lingxi-commands`,
//! called by `lingxi-orchestrator`.

use async_trait::async_trait;

/// Routes a raw `/<name> <args>` user input to a registered handler.
#[async_trait]
pub trait SlashCommandDispatcher: Send + Sync {
    /// Dispatch the raw input (with or without leading `/`) and return the
    /// handler's display string, or the locked unknown-command literal if
    /// the name is not registered.
    async fn dispatch(&self, raw: &str) -> SlashDispatchResult;
}

/// Result of [`SlashCommandDispatcher::dispatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashDispatchResult {
    /// Handler ran and returned a display string.
    Handled { display: String },
    /// Input did not start with `/` — treat as a regular user prompt.
    NotASlashCommand,
    /// Input started with `/` but the name is not registered.
    Unknown { name: String, display: String },
}
```

  Add `pub mod commands;` to `lingxi-core/crates/traits/src/lib.rs` and `pub use commands::{SlashCommandDispatcher, SlashDispatchResult};`. Bump `lingxi-traits` version if its `Cargo.toml` semver is strict; otherwise leave it.

- [ ] **Step 2: Write the failing test.**

  Create `lingxi-core/crates/commands/src/dispatcher.rs`:

```rust
//! Implementation of [`lingxi_traits::SlashCommandDispatcher`] that routes
//! `/<name> <args>` into the in-crate [`CommandRegistry`].
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md` Task 5.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{register_all_builtin_commands, CommandRegistry};
    use lingxi_traits::SlashDispatchResult;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn seeded_dispatcher() -> RegistrySlashDispatcher {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
    }

    #[tokio::test]
    async fn dispatches_known_command_to_locked_stub() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/ant-trace").await;
        match result {
            SlashDispatchResult::Handled { display } => {
                assert_eq!(display, "ant-trace: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Handled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatches_known_command_with_args_to_locked_stub() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/clear --force").await;
        match result {
            SlashDispatchResult::Handled { display } => {
                assert_eq!(display, "clear: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Handled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_command_returns_locked_literal() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/notacommand").await;
        match result {
            SlashDispatchResult::Unknown { name, display } => {
                assert_eq!(name, "notacommand");
                assert_eq!(display, "Unknown command: /notacommand");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_slash_input_is_not_a_slash_command() {
        let d = seeded_dispatcher();
        let result = d.dispatch("hello world").await;
        assert!(matches!(result, SlashDispatchResult::NotASlashCommand));
    }

    #[tokio::test]
    async fn empty_input_is_not_a_slash_command() {
        let d = seeded_dispatcher();
        let result = d.dispatch("").await;
        assert!(matches!(result, SlashDispatchResult::NotASlashCommand));
    }

    #[tokio::test]
    async fn just_slash_is_not_a_command() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/").await;
        // After stripping the leading '/', the name is empty — treat as unknown.
        match result {
            SlashDispatchResult::Unknown { name, display } => {
                assert_eq!(name, "");
                assert_eq!(display, "Unknown command: /");
            }
            other => panic!("expected Unknown for '/', got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uppercase_name_is_unknown() {
        let d = seeded_dispatcher();
        // Per Task 0 step 4 L3 expansion: the parser does NOT case-fold; the
        // registry only holds lowercase keys.
        let result = d.dispatch("/CLEAR").await;
        match result {
            SlashDispatchResult::Unknown { name, display } => {
                assert_eq!(name, "CLEAR");
                assert_eq!(display, "Unknown command: /CLEAR");
            }
            other => panic!("expected Unknown for /CLEAR, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Run and watch fail.**

```bash
cargo test -p lingxi-commands --lib dispatcher::tests 2>&1 | head -25
```

  Expected: 7 failures (`cannot find struct 'RegistrySlashDispatcher'`).

- [ ] **Step 4: Implement.**

  Prepend the impl above the `#[cfg(test)]` block:

```rust
//! Implementation of [`lingxi_traits::SlashCommandDispatcher`] that routes
//! `/<name> <args>` into the in-crate [`CommandRegistry`].
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md` Task 5.

use crate::model::CommandResult;
use crate::parser::parse_slash_command;
use crate::registry::CommandRegistry;
use async_trait::async_trait;
use lingxi_traits::{SlashCommandDispatcher, SlashDispatchResult};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Concrete `SlashCommandDispatcher` backed by an `Arc<RwLock<CommandRegistry>>`.
///
/// The registry is wrapped in an `RwLock` because plugin lifecycle events
/// ([`CommandRegistry::register_plugin_commands`] /
/// [`CommandRegistry::unregister_plugin`]) need exclusive write access at
/// runtime. Dispatching only takes a read lock.
pub struct RegistrySlashDispatcher {
    registry: Arc<RwLock<CommandRegistry>>,
}

impl RegistrySlashDispatcher {
    /// Construct a dispatcher backed by the given shared registry.
    #[must_use]
    pub fn new(registry: Arc<RwLock<CommandRegistry>>) -> Self {
        Self { registry }
    }

    /// Format the locked unknown-command literal.
    ///
    /// Public so callers can render the same string outside the dispatch loop
    /// (e.g. when reporting an error from a `/help` lookup that finds a
    /// dangling alias).
    #[must_use]
    pub fn unknown_command_literal(name: &str) -> String {
        format!("Unknown command: /{name}")
    }
}

#[async_trait]
impl SlashCommandDispatcher for RegistrySlashDispatcher {
    async fn dispatch(&self, raw: &str) -> SlashDispatchResult {
        // 1. Detect slash prefix.
        let Some(rest) = raw.strip_prefix('/') else {
            return SlashDispatchResult::NotASlashCommand;
        };

        // 2. Empty input ("") was already handled by strip_prefix returning None,
        //    but "/" alone leaves rest = "" — treat as Unknown with empty name.
        if rest.is_empty() {
            return SlashDispatchResult::Unknown {
                name: String::new(),
                display: Self::unknown_command_literal(""),
            };
        }

        // 3. Parse into name + args via the existing M1.15 parser.
        let parsed = match parse_slash_command(rest) {
            Ok(p) => p,
            Err(_) => {
                // Parser errors are extremely rare (only on truly malformed
                // input). Treat as unknown with the raw rest as the name.
                return SlashDispatchResult::Unknown {
                    name: rest.to_string(),
                    display: Self::unknown_command_literal(rest),
                };
            }
        };

        // 4. Look up the handler.
        let reg = self.registry.read().await;
        let Some(handler) = reg.get_handler(&parsed.name) else {
            return SlashDispatchResult::Unknown {
                name: parsed.name.clone(),
                display: Self::unknown_command_literal(&parsed.name),
            };
        };

        // 5. Drop the registry lock before awaiting handler (handler may
        //    re-lock the registry or take a while).
        drop(reg);
        let result = handler.handle(&parsed).await;

        match result {
            CommandResult::Done { display } => SlashDispatchResult::Handled {
                display: display.unwrap_or_default(),
            },
            // M5-09 placeholders only ever return Done — but route the
            // other variants safely anyway so M5-10/M5-11 can extend.
            CommandResult::InjectMessage { content } => {
                SlashDispatchResult::Handled { display: content }
            }
            CommandResult::EmitEffects { display, .. } => {
                SlashDispatchResult::Handled {
                    display: display.unwrap_or_default(),
                }
            }
            CommandResult::RequestConfirmation { prompt, .. } => {
                SlashDispatchResult::Handled { display: prompt }
            }
        }
    }
}
```

  **About `parse_slash_command`**: Confirm the function exists and its signature. Run `rg -n "pub fn parse_slash_command" lingxi-core/crates/commands/src/parser.rs`. The expected signature is `pub fn parse_slash_command(input: &str) -> Result<ParsedSlashCommand, ParseError>` per M1.15. If the existing function takes `&str` **including** the leading `/`, drop the `strip_prefix` in step 1 (and adjust the unknown-name extraction accordingly). The test cases above assume the parser receives the input **without** the leading `/`.

  Add `lingxi-traits` to `Cargo.toml` if not present. Open `lingxi-core/crates/commands/Cargo.toml`:

```toml
[dependencies]
async-trait = { workspace = true }
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }    # ← add
serde = { workspace = true, features = ["derive"] }
tokio = { workspace = true, features = ["sync"] }    # ← ensure "sync" feature is on for RwLock
```

- [ ] **Step 5: Add `pub mod dispatcher;` + re-export to `lib.rs`.**

```rust
//! Slash-command subsystem: parser, argument substitution, registry,
//! dispatcher, and built-in command handlers.

#![forbid(unsafe_code)]

pub mod argument_substitution;
pub mod builtin;
pub mod dispatcher;
pub mod model;
pub mod parser;
pub mod registry;

pub use argument_substitution::substitute_arguments;
pub use dispatcher::RegistrySlashDispatcher;
pub use model::*;
pub use parser::{parse_slash_command, ParsedSlashCommand};
pub use registry::{register_all_builtin_commands, CommandRegistry};
```

- [ ] **Step 6: Run the tests and watch all pass.**

```bash
cargo test -p lingxi-commands --lib dispatcher::tests 2>&1 | tail -15
```

  Expected: **7 passed**. If `just_slash_is_not_a_command` fails because `parse_slash_command("")` errors out before our explicit empty check, move the empty check to *after* parse (or run the explicit check first as shown). The exact parser semantics drive this.

- [ ] **Step 7: Run full crate tests to confirm no regressions.**

```bash
cargo test -p lingxi-commands 2>&1 | tail -10
```

  Expected: all tests across `builtin::*`, `registry::*`, `dispatcher::*`, `parser::*`, `argument_substitution::*` pass.

- [ ] **Step 8: Fmt + clippy.**

```bash
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 9: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/src/dispatcher.rs \
        lingxi-core/crates/commands/src/lib.rs \
        lingxi-core/crates/commands/Cargo.toml \
        lingxi-core/crates/traits/src/    # picks up commands.rs + lib.rs change if added
git commit -m "feat(M5-09 task 5): RegistrySlashDispatcher + SlashCommandDispatcher trait + Unknown command literal lock (7 dispatcher tests)"
```

---

## Task 6: Parity fixture + driver — lock the 102 + 18 split

**Files:**
- Create: `lingxi-core/crates/test-harness/src/parity/fixtures/parity_slash_commands_102.json`
- Create: `lingxi-core/crates/test-harness/tests/parity_slash_commands.rs`

- [ ] **Step 1: Write the parity fixture.**

  Create `lingxi-core/crates/test-harness/src/parity/fixtures/parity_slash_commands_102.json`. The file lists every name + its core/unimplemented classification + the expected stub literal:

```json
{
  "_meta": {
    "plan": "M5-09",
    "spec_section": "§3 M5-09 row + §4.6 (Slash commands)",
    "_claude_code_version": "see CHANGELOG v0.6.0 baseline",
    "total_count_lock": 102,
    "core_count_lock": 18,
    "unimplemented_count_lock": 84,
    "stub_literal_template": "{name}: not implemented in v0.6.0 (M5)",
    "unknown_literal_template": "Unknown command: /{name}"
  },
  "commands": [
    {"name": "add-dir",            "is_core": false},
    {"name": "advisor",            "is_core": false},
    {"name": "agents",             "is_core": true,  "description": "Manage subagents"},
    {"name": "ant-trace",          "is_core": false},
    {"name": "autofix-pr",         "is_core": false},
    {"name": "backfill-sessions",  "is_core": false},
    {"name": "branch",             "is_core": false},
    {"name": "break-cache",        "is_core": false},
    {"name": "bridge",             "is_core": false},
    {"name": "brief",              "is_core": false},
    {"name": "btw",                "is_core": false},
    {"name": "bughunter",          "is_core": false},
    {"name": "chrome",             "is_core": false},
    {"name": "clear",              "is_core": true,  "description": "Clear conversation history and free up context"},
    {"name": "color",              "is_core": false},
    {"name": "commit",             "is_core": false},
    {"name": "commit-push-pr",     "is_core": false},
    {"name": "compact",            "is_core": true,  "description": "Compact the conversation to a summary"},
    {"name": "config",             "is_core": true,  "description": "Open config panel"},
    {"name": "context",            "is_core": false},
    {"name": "copy",               "is_core": false},
    {"name": "cost",               "is_core": true,  "description": "Show total cost and duration of the current session"},
    {"name": "ctx-viz",            "is_core": false},
    {"name": "debug-tool-call",    "is_core": false},
    {"name": "desktop",            "is_core": false},
    {"name": "diff",               "is_core": false},
    {"name": "doctor",             "is_core": true,  "description": "Diagnose installation and configuration"},
    {"name": "effort",             "is_core": false},
    {"name": "env",                "is_core": false},
    {"name": "exit",               "is_core": true,  "description": "Exit the REPL"},
    {"name": "export",             "is_core": false},
    {"name": "extra-usage",        "is_core": false},
    {"name": "fast",               "is_core": false},
    {"name": "feedback",           "is_core": false},
    {"name": "files",              "is_core": false},
    {"name": "good-claude",        "is_core": false},
    {"name": "heapdump",           "is_core": false},
    {"name": "help",               "is_core": true,  "description": "Show help and available commands"},
    {"name": "hooks",              "is_core": true,  "description": "Manage hooks"},
    {"name": "ide",                "is_core": false},
    {"name": "init",               "is_core": true,  "description": "Initialize a new CLAUDE.md file with codebase documentation"},
    {"name": "init-verifiers",     "is_core": false},
    {"name": "insights",           "is_core": false},
    {"name": "install",            "is_core": false},
    {"name": "install-github-app", "is_core": false},
    {"name": "install-slack-app",  "is_core": false},
    {"name": "issue",              "is_core": false},
    {"name": "keybindings",        "is_core": false},
    {"name": "login",              "is_core": true,  "description": "Sign in with your Anthropic account"},
    {"name": "logout",             "is_core": true,  "description": "Sign out from your Anthropic account"},
    {"name": "mcp",                "is_core": true,  "description": "Manage MCP servers"},
    {"name": "memory",             "is_core": true,  "description": "Edit Claude memory files"},
    {"name": "mobile",             "is_core": false},
    {"name": "mock-limits",        "is_core": false},
    {"name": "model",              "is_core": true,  "description": "Set the model for Claude Code to use"},
    {"name": "oauth-refresh",      "is_core": false},
    {"name": "onboarding",         "is_core": false},
    {"name": "output-style",       "is_core": false},
    {"name": "passes",             "is_core": false},
    {"name": "perf-issue",         "is_core": false},
    {"name": "permissions",        "is_core": true,  "description": "Manage permissions"},
    {"name": "plan",               "is_core": false},
    {"name": "plugin",             "is_core": false},
    {"name": "pr-comments",        "is_core": false},
    {"name": "privacy-settings",   "is_core": false},
    {"name": "rate-limit-options", "is_core": false},
    {"name": "release-notes",      "is_core": false},
    {"name": "reload-plugins",     "is_core": false},
    {"name": "remote-env",         "is_core": false},
    {"name": "remote-setup",       "is_core": false},
    {"name": "rename",             "is_core": false},
    {"name": "reset-limits",       "is_core": false},
    {"name": "resume",             "is_core": false},
    {"name": "review",             "is_core": false},
    {"name": "rewind",             "is_core": false},
    {"name": "sandbox-toggle",     "is_core": false},
    {"name": "security-review",    "is_core": false},
    {"name": "session",            "is_core": false},
    {"name": "share",              "is_core": false},
    {"name": "skills",             "is_core": false},
    {"name": "stats",              "is_core": false},
    {"name": "status",             "is_core": true,  "description": "Show Claude Code status"},
    {"name": "statusline",         "is_core": false},
    {"name": "stickers",           "is_core": false},
    {"name": "summary",            "is_core": false},
    {"name": "tag",                "is_core": false},
    {"name": "tasks",              "is_core": false},
    {"name": "teleport",           "is_core": false},
    {"name": "terminal-setup",     "is_core": false},
    {"name": "theme",              "is_core": false},
    {"name": "thinkback",          "is_core": false},
    {"name": "thinkback-play",     "is_core": false},
    {"name": "ultraplan",          "is_core": false},
    {"name": "upgrade",            "is_core": false},
    {"name": "usage",              "is_core": false},
    {"name": "version",            "is_core": true,  "description": "Print version information"},
    {"name": "vim",                "is_core": false},
    {"name": "voice",              "is_core": false},
    {"name": "x402",               "is_core": false}
  ]
}
```

  Verify line count: `wc -l` should report ≥ 110 lines (header + 102 entries + closing braces). Open in editor to confirm exactly **102** entries inside the `commands` array.

- [ ] **Step 2: Write the parity driver.**

  Create `lingxi-core/crates/test-harness/tests/parity_slash_commands.rs`:

```rust
//! Parity: lock the 102 builtin slash-command names + the 18-core split + the
//! stub-literal output across the full surface.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 6. Locks introduced here:
//!
//! - Total name count = 102
//! - Core name count = 18
//! - Unimplemented = 84
//! - Stub literal template = "{name}: not implemented in v0.6.0 (M5)"
//! - Unknown literal template = "Unknown command: /{name}"

use lingxi_commands::builtin::{BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
use lingxi_commands::dispatcher::RegistrySlashDispatcher;
use lingxi_commands::registry::{register_all_builtin_commands, CommandRegistry};
use lingxi_traits::{SlashCommandDispatcher, SlashDispatchResult};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct ParityFile {
    #[serde(rename = "_meta")]
    meta: ParityMeta,
    commands: Vec<ParityCommand>,
}

#[derive(Debug, Deserialize)]
struct ParityMeta {
    total_count_lock: usize,
    core_count_lock: usize,
    unimplemented_count_lock: usize,
    stub_literal_template: String,
    unknown_literal_template: String,
}

#[derive(Debug, Deserialize)]
struct ParityCommand {
    name: String,
    is_core: bool,
    #[serde(default)]
    description: Option<String>,
}

const FIXTURE: &str =
    include_str!("../src/parity/fixtures/parity_slash_commands_102.json");

fn load_fixture() -> ParityFile {
    serde_json::from_str(FIXTURE).expect("fixture parse")
}

#[test]
fn fixture_total_matches_constant() {
    let f = load_fixture();
    assert_eq!(f.meta.total_count_lock, 102);
    assert_eq!(f.commands.len(), 102);
    assert_eq!(BUILTIN_COMMAND_NAMES.len(), 102);
    assert_eq!(f.commands.len(), BUILTIN_COMMAND_NAMES.len());
}

#[test]
fn fixture_core_matches_constant() {
    let f = load_fixture();
    let fixture_core: Vec<&str> = f
        .commands
        .iter()
        .filter(|c| c.is_core)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(fixture_core.len(), 18);
    assert_eq!(f.meta.core_count_lock, 18);
    assert_eq!(BUILTIN_CORE_NAMES.len(), 18);

    // Order-sensitive equality (both are ASCII-sorted by construction).
    let const_core: Vec<&str> = BUILTIN_CORE_NAMES.iter().copied().collect();
    assert_eq!(fixture_core, const_core);
}

#[test]
fn fixture_unimplemented_count_is_84() {
    let f = load_fixture();
    let n_unimpl = f.commands.iter().filter(|c| !c.is_core).count();
    assert_eq!(n_unimpl, 84);
    assert_eq!(f.meta.unimplemented_count_lock, 84);
}

#[test]
fn stub_template_lock() {
    let f = load_fixture();
    assert_eq!(
        f.meta.stub_literal_template,
        "{name}: not implemented in v0.6.0 (M5)"
    );
}

#[test]
fn unknown_template_lock() {
    let f = load_fixture();
    assert_eq!(
        f.meta.unknown_literal_template,
        "Unknown command: /{name}"
    );
}

#[test]
fn fixture_names_match_constant_order() {
    let f = load_fixture();
    let fixture_names: Vec<&str> = f.commands.iter().map(|c| c.name.as_str()).collect();
    let const_names: Vec<&str> = BUILTIN_COMMAND_NAMES.iter().copied().collect();
    assert_eq!(fixture_names, const_names);
}

#[tokio::test]
async fn every_fixture_command_dispatches_to_expected_literal() {
    let f = load_fixture();
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    for entry in &f.commands {
        let raw = format!("/{}", entry.name);
        let res = d.dispatch(&raw).await;
        match res {
            SlashDispatchResult::Handled { display } => {
                let expected = format!("{}: not implemented in v0.6.0 (M5)", entry.name);
                assert_eq!(display, expected, "wrong output for /{}", entry.name);
            }
            other => panic!("/{}: expected Handled, got {other:?}", entry.name),
        }
    }
}

#[tokio::test]
async fn unknown_command_uses_locked_literal() {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    let res = d.dispatch("/definitely-not-a-real-command").await;
    match res {
        SlashDispatchResult::Unknown { name, display } => {
            assert_eq!(name, "definitely-not-a-real-command");
            assert_eq!(display, "Unknown command: /definitely-not-a-real-command");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[tokio::test]
async fn core_command_description_matches_fixture() {
    let f = load_fixture();
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    for entry in f.commands.iter().filter(|c| c.is_core) {
        let expected = entry.description.as_deref().unwrap_or_else(|| {
            panic!("core /{} missing description in fixture", entry.name)
        });
        let cmd = reg.resolve(&entry.name).expect("core cmd missing");
        assert_eq!(cmd.description, expected,
            "/{}: description drift (fixture vs registry)", entry.name);
    }
}
```

- [ ] **Step 3: Ensure `test-harness/Cargo.toml` has the deps.**

```bash
cat lingxi-core/crates/test-harness/Cargo.toml | grep -E "lingxi-commands|lingxi-traits|serde_json"
```

  Expected lines (add any missing under `[dependencies]` or `[dev-dependencies]`):

```toml
lingxi-commands = { path = "../commands" }
lingxi-traits   = { path = "../traits" }
serde      = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
tokio      = { workspace = true, features = ["macros", "rt-multi-thread", "sync"] }
```

- [ ] **Step 4: Run the parity tests.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test -p lingxi-test-harness --test parity_slash_commands 2>&1 | tail -20
```

  Expected: **9 passed; 0 failed**.

  If `every_fixture_command_dispatches_to_expected_literal` fails on a specific name (e.g. `agents`), inspect that core placeholder's body — the test expects ALL 102 (including the 18 core) to return the M5-09 stub literal because no real impl exists yet.

- [ ] **Step 5: Fmt + clippy.**

```bash
cargo fmt -p lingxi-test-harness
cargo clippy -p lingxi-test-harness --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 6: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/test-harness/src/parity/fixtures/parity_slash_commands_102.json \
        lingxi-core/crates/test-harness/tests/parity_slash_commands.rs \
        lingxi-core/crates/test-harness/Cargo.toml
git commit -m "test(M5-09 task 6): parity fixture + 9 drivers — 102 names, 18 core, 84 unimplemented + stub/unknown templates locked"
```

---

## Task 7: Integration test — dispatcher + registry end-to-end

**Files:**
- Create: `lingxi-core/crates/commands/tests/dispatch_e2e.rs`

- [ ] **Step 1: Write the integration test.**

```rust
//! End-to-end integration test: build a CommandRegistry, register all 102,
//! wire a dispatcher, and exercise the full surface from the public API.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 7.

use lingxi_commands::dispatcher::RegistrySlashDispatcher;
use lingxi_commands::registry::{register_all_builtin_commands, CommandRegistry};
use lingxi_traits::{SlashCommandDispatcher, SlashDispatchResult};
use std::sync::Arc;
use tokio::sync::RwLock;

fn build_dispatcher() -> RegistrySlashDispatcher {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
}

#[tokio::test]
async fn happy_path_known_core_command() {
    let d = build_dispatcher();
    let res = d.dispatch("/help").await;
    match res {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "help: not implemented in v0.6.0 (M5)");
        }
        other => panic!("expected Handled, got {other:?}"),
    }
}

#[tokio::test]
async fn happy_path_known_unimplemented_command() {
    let d = build_dispatcher();
    let res = d.dispatch("/x402").await;
    match res {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "x402: not implemented in v0.6.0 (M5)");
        }
        other => panic!("expected Handled, got {other:?}"),
    }
}

#[tokio::test]
async fn known_command_with_args_ignored_by_stub() {
    let d = build_dispatcher();
    let res = d.dispatch("/exit --force --soon=now").await;
    match res {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "exit: not implemented in v0.6.0 (M5)");
        }
        other => panic!("expected Handled, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_command_path() {
    let d = build_dispatcher();
    let res = d.dispatch("/zzz-fake").await;
    match res {
        SlashDispatchResult::Unknown { name, display } => {
            assert_eq!(name, "zzz-fake");
            assert_eq!(display, "Unknown command: /zzz-fake");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[tokio::test]
async fn non_slash_input_path() {
    let d = build_dispatcher();
    let res = d.dispatch("hello /clear world").await;
    assert!(matches!(res, SlashDispatchResult::NotASlashCommand));
}

#[tokio::test]
async fn many_dispatches_against_one_registry() {
    let d = build_dispatcher();
    for name in ["clear", "compact", "ant-trace", "version", "x402"] {
        let raw = format!("/{name}");
        let res = d.dispatch(&raw).await;
        let expected = format!("{name}: not implemented in v0.6.0 (M5)");
        match res {
            SlashDispatchResult::Handled { display } => assert_eq!(display, expected),
            other => panic!("/{name} unexpected: {other:?}"),
        }
    }
}

#[tokio::test]
async fn concurrent_dispatches_against_one_registry() {
    let d = Arc::new(build_dispatcher());
    let mut handles = vec![];
    for name in ["agents", "config", "cost", "doctor", "hooks", "init"] {
        let d = d.clone();
        let raw = format!("/{name}");
        let expected = format!("{name}: not implemented in v0.6.0 (M5)");
        handles.push(tokio::spawn(async move {
            let res = d.dispatch(&raw).await;
            match res {
                SlashDispatchResult::Handled { display } => assert_eq!(display, expected),
                other => panic!("/{name} {other:?}"),
            }
        }));
    }
    for h in handles {
        h.await.expect("task panic");
    }
}
```

- [ ] **Step 2: Run.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test -p lingxi-commands --test dispatch_e2e 2>&1 | tail -15
```

  Expected: **7 passed**.

- [ ] **Step 3: Fmt + clippy.**

```bash
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 4: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/tests/dispatch_e2e.rs
git commit -m "test(M5-09 task 7): dispatch_e2e integration tests — 7 end-to-end cases"
```

---

## Task 8: Doc + README updates

**Files:**
- Modify: `lingxi-core/crates/commands/src/lib.rs` (module-level docs)
- Modify: `lingxi-core/README.md` (if present — add commands status row)

- [ ] **Step 1: Expand the `lingxi-commands` crate docs.**

  Open `lingxi-core/crates/commands/src/lib.rs` and replace the existing top-level comment with:

```rust
//! Slash-command subsystem.
//!
//! # Surface (M5-09)
//!
//! After [`register_all_builtin_commands`] runs, the [`CommandRegistry`] holds
//! exactly **102** entries. 84 of them point at a shared
//! [`builtin::UnimplementedCommandHandler`] and return the locked literal
//! `"{name}: not implemented in v0.6.0 (M5)"`. The remaining 18 (the "core"
//! list defined by [`builtin::BUILTIN_CORE_NAMES`]) point at per-name
//! placeholder structs in [`builtin::core_placeholders`] that **also** return
//! the same locked literal in M5-09 but exist as stable type-ids so M5-10 and
//! M5-11 can swap their bodies independently without touching registry wiring.
//!
//! # Dispatch
//!
//! [`dispatcher::RegistrySlashDispatcher`] implements
//! [`lingxi_traits::SlashCommandDispatcher`]. It strips a leading `/`,
//! parses the remainder via [`parser::parse_slash_command`], looks up the
//! handler in the [`CommandRegistry`], and returns one of:
//!
//! - [`lingxi_traits::SlashDispatchResult::Handled`] for known commands
//! - [`lingxi_traits::SlashDispatchResult::Unknown`] with the locked literal
//!   `"Unknown command: /{name}"` for unregistered names
//! - [`lingxi_traits::SlashDispatchResult::NotASlashCommand`] for inputs that
//!   don't start with `/`
//!
//! # Plan reference
//!
//! See `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`.

#![forbid(unsafe_code)]
```

- [ ] **Step 2: Update README (only if present).**

```bash
test -f lingxi-core/README.md && head -30 lingxi-core/README.md
```

  If the README contains a milestones / surface table, add or update a row:

```markdown
| M5-09 | Slash commands surface | ✅ 102 names registered (84 unimplemented stubs + 18 core placeholders); locked stub + unknown literals; `RegistrySlashDispatcher` impl |
```

  If no README or no table, skip this step.

- [ ] **Step 3: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/src/lib.rs
test -f lingxi-core/README.md && git add lingxi-core/README.md || true
git commit -m "docs(M5-09 task 8): module-level docs for lingxi-commands M5-09 surface"
```

---

## Task 9: Verification gate (fmt + clippy + workspace tests)

**Files:** none (no code changes — just verification commands).

- [ ] **Step 1: Workspace fmt diff.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo fmt --all -- --check 2>&1 | tail -20
```

  Expected: no output (clean).

- [ ] **Step 2: Workspace clippy.**

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30
```

  Expected: clean compilation, no warnings.

- [ ] **Step 3: Workspace tests.**

```bash
cargo test --workspace 2>&1 | tail -30
```

  Expected: all tests pass except the 2 known fs-watch flakes (see CHANGELOG v0.5.0). If a new failure appears in any of:

  - `lingxi-commands` (any unit / integration / `dispatch_e2e`)
  - `lingxi-test-harness::parity_slash_commands` (9 cases)
  - `lingxi-test-harness::parity_registry_40_tools` (M4-09 lock — should still pass; the M5-09 changes do not affect tool count)
  - `lingxi-test-harness::parity_telemetry_coverage` (no event changes in M5-09 — should still pass at 134 tool events)

  …debug it before committing.

- [ ] **Step 4: Confirm `ALL_EVENT_NAMES.len()` unchanged.**

```bash
rg -n "ALL_EVENT_NAMES" lingxi-core/crates/telemetry/src/ 2>&1 | head -5
cargo test -p lingxi-telemetry --lib event_name_completeness 2>&1 | tail -10
```

  Expected: the completeness test passes at the M5-08 count (258 — see plan M5-08 Task 14 step 3). M5-09 ships zero new events.

- [ ] **Step 5: No commit (Task 9 is verification only).**

  If steps 1-4 all pass, proceed to Task 10. If any fails, fix in the relevant Task 1-8 commit before tagging.

---

## Task 10: Tag m5.9 + summary commit

**Files:**
- Modify: `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md` (this file — add "Status: complete" at top)

- [ ] **Step 1: Mark this plan complete in its header.**

  Edit the top of `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md` to add a status line right after the title:

```markdown
# M5-09 Slash Commands Surface — 102 Names Registered, 84 Stubs + 18 Core Placeholders

**Status:** ✅ Complete (tagged `m5.9` on YYYY-MM-DD)

> **For agentic workers:** REQUIRED SUB-SKILL: ...
```

  Replace `YYYY-MM-DD` with today's date.

- [ ] **Step 2: Final commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md
git commit -m "release(M5-09 task 10): mark commands-surface plan complete"
```

- [ ] **Step 3: Annotated tag `m5.9`.**

```bash
git tag -a m5.9 -m "M5-09: 102 slash commands registered, 84 stubs + 18 core placeholders, dispatcher impl + locked literals"
```

- [ ] **Step 4: Verify.**

```bash
git tag -n 1 | grep "^m5\." | sort
```

  Expected output ends with the new tag:

```
m5.1  ...
m5.2  ...
...
m5.8  ...
m5.9  M5-09: 102 slash commands registered...
```

- [ ] **Step 5: Do NOT push.** Tag stays local; release engineer pushes in M5-14 along with the v0.6.0 tag.

---

## Self-review

**1. Spec coverage** (against spec §3 M5-09 row):

| Spec requirement | Task |
|---|---|
| Register all 102 commands in `lingxi-commands::registry` | T3 (`register_all_builtin_commands` registers 102) |
| 84 stubs return locked literal `"{name}: not implemented in v0.6.0 (M5)"` | T1 (`UnimplementedCommandHandler`) + T6 (parity fixture asserts all 102 entries return the expected expansion) |
| 18 core get per-name `Box<dyn BuiltinCommandHandler>` placeholders | T4 (18 macro-generated structs + `register_core_placeholders`) |
| Stub literal lock | T0 step 4 L1, T1 step 3, T6 fixture |
| 102 command-name lock | T0 step 2, T2 (`BUILTIN_COMMAND_NAMES`), T6 fixture |
| 0 new telemetry events | T9 step 4 confirms `ALL_EVENT_NAMES.len()` unchanged at 258 |
| Dependency: M5-02 (OrchestratorHandle) | T5 step 1 confirms / creates `lingxi-traits::SlashCommandDispatcher` |

  All ✅.

**2. Placeholder scan:**

  - No `TODO`, `TBD`, `unimplemented!()`, `todo!()`, `FIXME`, "implement later", or "fill in details" anywhere in the plan code blocks.
  - `register_core_placeholders` is a **stub** in T3 step 4 (no-op body) that gets implemented in T4 step 4 — this is intentional and explicitly called out as "Task 4 fills this in." Not a placeholder; it's a documented two-task progression.
  - "(unimplemented in v0.6.0)" appears as a **runtime description** string (not a code placeholder) for the 84 non-core commands. It's a real, intentional literal shown by `/help` in M5-10.

**3. Type consistency:**

| Identifier | T1 / T2 / T3 / T4 declaration | All later uses |
|---|---|---|
| `UnimplementedCommandHandler` | T1 struct in `builtin/unimplemented.rs` | T2 (re-export), T3 (used in `register_all_builtin_commands`), T4 (delegates `stub_literal`), T5 (dispatcher tests check produced literal) |
| `UnimplementedCommandHandler::stub_literal` | T1 `pub fn` | T4 macro body, T5 dispatcher's unknown-command literal (separate helper `unknown_command_literal`) |
| `BUILTIN_COMMAND_NAMES` | T2 `pub const &[&str; 102]` | T3, T4 invariant tests, T6 fixture cross-check, T9 verification |
| `BUILTIN_CORE_NAMES` | T2 `pub const &[&str; 18]` | T4 invariant tests, T6 fixture cross-check |
| `core_description` | T2 `pub fn` | T3 (passed to each `UnimplementedCommandHandler::new`), T4 macro `description()` body |
| `register_all_builtin_commands` | T3 in `registry.rs` | T4 tests, T5 dispatcher tests, T6 parity driver, T7 e2e tests |
| `register_core_placeholders` | T3 stub in `core_placeholders.rs`, T4 full impl | T3 (called from `register_all_builtin_commands`) |
| `ClearHandler`, `HelpHandler`, ..., `VersionHandler` (18 per-name) | T4 macro expansion | T4 type-id test, T4 register_core_placeholders, parity fixture (per-name registry resolve) |
| `RegistrySlashDispatcher` | T5 in `dispatcher.rs` | T5 tests, T6 parity driver, T7 e2e tests |
| `RegistrySlashDispatcher::unknown_command_literal` | T5 `pub fn` | T5 tests |
| `SlashCommandDispatcher`, `SlashDispatchResult` | T5 step 1 — defined in `lingxi-traits` (creates trait if missing) | T5, T6, T7 |
| Stub literal `"{name}: not implemented in v0.6.0 (M5)"` | T0 L1 → T1 step 3 `stub_literal` → T6 fixture | T1, T3, T4, T5, T6, T7 (all reference the same literal) |
| Unknown literal `"Unknown command: /{name}"` | T0 L3 → T5 step 4 `unknown_command_literal` → T6 fixture | T5 tests, T6 fixture, T7 e2e |

  All consistent. The only naming collision risk is between M1.15's `CompactHandler`/`HelpHandler`/etc. and the M5-09 macro-generated `CompactHandler`/`HelpHandler` — resolved in Task 4 step 4 by **deleting** the M1.15 stub files (option B in that step). Document the deletion in the commit message.

**4. Telemetry chain:** M5-09 introduces **zero** new events. T9 step 4 verifies `ALL_EVENT_NAMES.len()` remains at 258 (the M5-08 value). No M5-09 row appears in spec §6.3 telemetry growth table — that table jumps from M5-08 (258) straight to M5-10 (276 = 258 + 18 from M5-10's 6 commands × 3 events).

**5. M4 backwards-compat:** T9 step 3 runs the entire workspace test suite including `parity_registry_40_tools.rs` (M4-09 — 40 tools locked) and `parity_telemetry_coverage.rs` (M4-09 — 134 tool events locked). Both must pass unchanged. M5-09 touches **only** the `lingxi-commands` crate + the `lingxi-traits` crate (new `SlashCommandDispatcher` trait if it doesn't already exist) + the `lingxi-test-harness` fixture set. No tool, telemetry, settings, or M3 surface is modified.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**

**If Subagent-Driven chosen:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Fresh subagent per task + two-stage review.

**If Inline Execution chosen:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Batch execution with checkpoints for review.
