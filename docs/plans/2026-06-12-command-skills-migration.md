# Command Skills Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Register Claude-style file skills as executable slash commands and close local command parity gaps without broadening into host-bound or remote-only command families.

**Architecture:** `command-api` owns registry metadata and markdown/skills-dir command loading; `skill-api` owns shared file-skill listing; `tool-skill` owns prompt injection; `command-core` owns headless built-in handlers; `tui` consumes shared listing data; `test-harness` records parity status. Each production behavior is introduced with a focused failing test first.

**Tech Stack:** Rust 2021 workspace crates under `lingxi-code`, Tokio async tests, `serde`, existing command/skill/TUI crates, no new dependencies.

---

## Starting Context

- Existing uncommitted work already adds `command-core/src/skills.rs` and `skill-api/src/listing.rs` plus partial `/skills` registration.
- The repository root is dirty; preserve existing user/agent changes and avoid reverting unrelated edits.
- `rg` and `sg` are unavailable in the current shell, so use CodeGraph, direct file reads, and cargo test output for local discovery.

## Task 1: Command API skills-dir metadata

**Files:**
- Test: `lingxi-code/commands/command-api/src/markdown_loader.rs` or the current command loader test module
- Modify: `lingxi-code/commands/command-api/src/model.rs`
- Modify: `lingxi-code/commands/command-api/src/markdown_loader.rs`

**Step 1: Write the failing test**

Add a test that creates `.claude/skills/demo/SKILL.md`, loads commands from the repo root, and asserts:

- command name is `demo`
- kind is markdown
- `loaded_from == "skills"`
- `skill_root` is the `demo` directory
- `content_length` is the markdown length
- ordinary `.claude/commands/foo.md` has no `skill_root`

**Step 2: Verify RED**

Run:

```bash
cargo test -p command-api markdown_loader skill
```

Expected: FAIL because skills-dir commands are not registered or metadata is missing.

**Step 3: Minimal implementation**

Add optional metadata to `SlashCommand` with serde skip-if-empty behavior. Extend the markdown loader to scan `.claude/skills/<name>/SKILL.md` directory-format skills and populate metadata only for true skills.

**Step 4: Verify GREEN**

Run the same `cargo test -p command-api markdown_loader skill` command and expect PASS.

## Task 2: Shared listing and SkillTool prompt context

**Files:**
- Test: `lingxi-code/skill-api/src/listing.rs`
- Test: `lingxi-code/tool-skill/src/lib.rs` or existing Skill tool tests
- Modify: `lingxi-code/skill-api/src/listing.rs`
- Modify: `lingxi-code/tool-skill/src/lib.rs`

**Step 1: Write failing tests**

Add/extend tests that assert project/user listing order, directory-format-only behavior, and `Base directory for this skill: <dir>` prompt prefix for file-backed skills.

**Step 2: Verify RED**

Run:

```bash
cargo test -p skill-api listing
cargo test -p tool-skill skill
```

Expected: at least the prompt-prefix assertion fails before implementation.

**Step 3: Minimal implementation**

Keep `load_file_skill_sections` as the shared listing source and add prompt-prefix injection in the Skill tool when the resolved skill has a file base directory.

**Step 4: Verify GREEN**

Run both test commands again and expect PASS.

## Task 3: TUI/headless command parity

**Files:**
- Test: `lingxi-code/commands/core/src/register.rs`
- Test: `crates/tui/src/screens/skills.rs`
- Modify: `lingxi-code/commands/core/src/interactive_only.rs`
- Modify: `lingxi-code/commands/core/src/register.rs`
- Modify: `crates/tui/src/screens/skills.rs`

**Step 1: Write failing tests**

Add tests that target-implemented commands (`skills`, and the chosen P3/P4 names) resolve to real handlers and do not return the locked M5 stub in headless paths. Add TUI Skills screen tests proving it renders data from `skill-api::load_file_skill_sections` rather than a divergent loader.

**Step 2: Verify RED**

Run:

```bash
cargo test -p command-core
cargo test -p tui skills stats
```

Expected: FAIL for names still backed by the generic stub or divergent TUI loader.

**Step 3: Minimal implementation**

Introduce `InteractiveOnlyHandler` for commands that require TUI interaction in this batch, wire P3/P4 local commands to real text or interactive-only handlers, and switch TUI Skills screen loading to the shared `skill-api` listing source.

**Step 4: Verify GREEN**

Run both commands again and expect PASS.

## Task 4: Parity matrix upgrade

**Files:**
- Test/modify: `lingxi-code/test-harness/tests/parity_slash_commands.rs`
- Modify: any fixture file used by that test if one exists

**Step 1: Write failing matrix assertions**

Replace the old count-only expectation with assertions over `claude_type`, `rust_status`, `target_status`, `requires_tui`, and `defer_reason`. Assert target-implemented commands never dispatch to `not implemented in v0.6.0 (M5)`.

**Step 2: Verify RED**

Run:

```bash
cargo test -p test-harness --test parity_slash_commands
```

Expected: FAIL until the fixture and command registrations match.

**Step 3: Minimal implementation**

Update the fixture/matrix and command status derivation, keeping faithful stubs and host-bound gaps explicit but not using them as the whole ledger.

**Step 4: Verify GREEN**

Run the parity command again and expect PASS.

## Task 5: Full verification and manual surface QA

**Files:**
- All changed Rust files

**Step 1: Format and diagnostics**

Run:

```bash
cargo fmt -p command-api -p command-core -p skill-api -p tool-skill -p tui
```

Run `lsp_diagnostics` on changed Rust files and fix new diagnostics.

**Step 2: Targeted tests**

Run the full approved test set:

```bash
cargo test -p command-api markdown_loader skill
cargo test -p skill-api listing
cargo test -p tool-skill skill
cargo test -p command-core
cargo test -p tui skills stats
cargo test -p test-harness --test parity_slash_commands
```

**Step 3: Manual surface QA**

Drive a real command/Skill surface by creating a temporary `.claude/skills/foo/SKILL.md` in a throwaway directory or cargo integration test harness, running the CLI/headless command path if available, and confirming `/skills` lists it and Skill resolution injects the base directory.
