# LingXi Namespace Rebrand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Claude-branded namespace (on-disk paths, env vars, well-known filenames, user-facing branding, internal symbols) with LingXi equivalents as a clean break, leaving the Anthropic protocol layer byte-identical.

**Architecture:** Approach C (hybrid). First establish a single source of truth — a `branding` constants module + one consolidated `config_home()` — routing the ≥8 duplicated helpers and scattered `.claude` literals through it (Commit A, values unchanged), then flip the constants to LingXi values (Commit B). After the seam exists, do guarded, staged sweeps for the cosmetic categories (branding strings, internal symbols, comments), the plugin dir, and the client cross-process contracts. Reconcile parity fixtures/snapshots last.

**Tech Stack:** Rust (cargo workspace `lingxi-code/`), TypeScript/Swift/Kotlin clients (`clients/`), `insta` snapshot tests, the `test-harness` byte-locked parity suite.

**Spec:** `docs/superpowers/specs/2026-06-26-lingxi-namespace-rebrand-design.md` (read it first).

## Global Constraints

- **Scope trees only:** `lingxi-code/` and `clients/`. NEVER edit vendored trees: `claude-code/`, `claw-code*/`, `codex/`, `opencode/`, `liter-llm/`, `third_party/`, or any `target/`/`node_modules/`.
- **Brand name:** `LingXi`. CLI binary `lingxi`. System-prompt identity "You are LingXi".
- **Path namespace:** dir `.lingxi`; global config `~/.lingxi.json`; memory files `LINGXI.md` / `LINGXI.local.md`; plugin manifest dir `.lingxi-plugin`.
- **Env namespace:** collapse both `CLAUDE_` and `CLAUDE_CODE_` prefixes to `LINGXI_` (e.g. `CLAUDE_CONFIG_DIR`→`LINGXI_CONFIG_DIR`, `CLAUDE_CODE_ENABLE_TASKS`→`LINGXI_ENABLE_TASKS`). Where a `LINGXI_*` twin already exists (`LINGXI_ENABLE_XAA`, `LINGXI_MODEL`, `LINGXI_API_BASE_URL`), keep the existing name and remove the duplicate `CLAUDE_*` read.
- **Clean break:** never read `.claude` / `CLAUDE_*` / `CLAUDE.md`. No migration code. Re-auth and loss of resume history are accepted.
- **DO-NOT-TOUCH (protocol/referential):** model IDs (`claude-opus-*`/`claude-sonnet-*`/`claude-haiku-*`) + the `anthropic`/`claude-` substring parsing in `agent/src/model_resolution.rs`; `api.anthropic.com`; the `anthropic` provider id; all `tengu_*` (also Statsig flag keys); `anthropic-beta` constants; `claude-cli/<ver>` UA + `anthropic-version` + `x-api-key`; OAuth endpoints/scopes + `client_id="lingxi-core"`; all `ANTHROPIC_*` env; protocol env vars (`CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_AGENT_SDK_*`, `CLAUDE_CODE_EXTRA_METADATA`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_USE_BEDROCK/VERTEX/FOUNDRY`, `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`, `CLAUDE_CODE_VERSION`); `AI_AGENT` stamp; `rate_limit_tier` values (`default_claude_*`); `BedrockClaude*`/`VertexClaude*`/`ClaudeAiOAuth*` types; `AddFromClaudeDesktop` + the Claude Desktop config path; `AGENTS.md`; all `claude-code/...:line` oracle-citation comments; the `claude-code-guide` subagent body; embedded URLs (`github.com/anthropics/claude-code`, `claude.ai/code`, `claude.com/claude-code`, `support.anthropic.com`).
- **Build:** `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_RELEASE_DEBUG=0` on all cargo invocations.
- **Per-crate safety:** after any change touching shared structs/strings, run `cargo test --workspace --no-run` (catches test-only literal breaks a normal run misses). After any render/string change, run BOTH `cargo test -p tui` AND `cargo test -p test-harness`.
- **Commits:** end every commit message with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 0: Pick the host crate for the `branding` constants

**Files:**
- Investigate: `lingxi-code/Cargo.toml` (workspace members), `lingxi-code/platform-api/Cargo.toml`

**Interfaces:**
- Produces: the crate path `BRANDING_CRATE` (a `::branding` module) that every other task imports constants from. Default target: a new leaf crate `lingxi-code/branding/`.

- [ ] **Step 1: Confirm no existing single seam**

Run: `grep -rn 'fn config_home\|claude_home_dir\|claude_config_home\|config_home_dir' lingxi-code --include='*.rs' | grep -v test`
Expected: ≥8 distinct definitions across engine/migrations/apps-cli/tui/commands/memory/tools — confirms consolidation is needed.

- [ ] **Step 2: Decide the host crate**

Create a new leaf crate `lingxi-code/branding/` (zero deps). This avoids dependency-direction cycles (every crate can depend on a leaf) and keeps the source of truth single-purpose. Record this decision; all later tasks `use branding::...`.

- [ ] **Step 3: Commit the decision note (no code yet)**

```bash
git commit --allow-empty -m "chore: branding constants will live in new leaf crate lingxi-code/branding

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 1: Create the `branding` crate with constants (values still Claude — Commit A part 1)

**Files:**
- Create: `lingxi-code/branding/Cargo.toml`, `lingxi-code/branding/src/lib.rs`
- Modify: `lingxi-code/Cargo.toml` (add `branding` to `[workspace] members`)

**Interfaces:**
- Produces:
  - `branding::DOT_DIR: &str`
  - `branding::GLOBAL_CONFIG_FILE: &str`
  - `branding::LEGACY_GLOBAL_CONFIG_FILE: &str`
  - `branding::CONFIG_DIR_ENV: &str`
  - `branding::MEMORY_FILE: &str`
  - `branding::MEMORY_LOCAL_FILE: &str`
  - `branding::PLUGIN_MANIFEST_DIR: &str`
  - `branding::PRODUCT_NAME: &str`
  - `branding::ENV_PREFIX: &str` (`"CLAUDE_"` now; `"LINGXI_"` after flip)
  - `branding::config_home(home: &Path, env: Option<OsString>) -> PathBuf` (pure)

- [ ] **Step 1: Write the failing test**

`lingxi-code/branding/src/lib.rs` (test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn config_home_defaults_to_home_join_dot_dir() {
        let got = config_home(Path::new("/home/u"), None);
        assert_eq!(got, Path::new("/home/u").join(DOT_DIR));
    }

    #[test]
    fn config_home_honors_env_verbatim_including_empty() {
        // `??` semantics: a SET value wins verbatim, even when empty.
        let got = config_home(Path::new("/home/u"), Some("/custom".into()));
        assert_eq!(got, Path::new("/custom"));
        let empty = config_home(Path::new("/home/u"), Some("".into()));
        assert_eq!(empty, Path::new(""));
    }

    #[test]
    fn values_are_still_claude_pre_flip() {
        assert_eq!(DOT_DIR, ".claude");
        assert_eq!(MEMORY_FILE, "CLAUDE.md");
        assert_eq!(CONFIG_DIR_ENV, "CLAUDE_CONFIG_DIR");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p branding`
Expected: FAIL — crate/functions not defined.

- [ ] **Step 3: Implement the crate (values unchanged from Claude)**

`lingxi-code/branding/Cargo.toml`:

```toml
[package]
name = "branding"
version = "0.1.0"
edition = "2021"

[dependencies]
```

`lingxi-code/branding/src/lib.rs`:

```rust
//! Single source of truth for the product namespace (paths, filenames, env
//! prefix, brand name). Commit A keeps the Claude values so the consolidation
//! is a pure refactor; Commit B flips them to LingXi.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const DOT_DIR: &str = ".claude";
pub const GLOBAL_CONFIG_FILE: &str = ".claude.json";
pub const LEGACY_GLOBAL_CONFIG_FILE: &str = ".config.json";
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
pub const MEMORY_FILE: &str = "CLAUDE.md";
pub const MEMORY_LOCAL_FILE: &str = "CLAUDE.local.md";
pub const PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";
pub const PRODUCT_NAME: &str = "Claude Code";
pub const ENV_PREFIX: &str = "CLAUDE_";

/// `$CONFIG_DIR_ENV ?? home.join(DOT_DIR)` — a SET env value wins verbatim
/// (including empty); only an UNSET var falls back to the home default.
#[must_use]
pub fn config_home(home: &Path, config_dir_env: Option<OsString>) -> PathBuf {
    match config_dir_env {
        Some(dir) => PathBuf::from(dir),
        None => home.join(DOT_DIR),
    }
}
```

Add `"branding"` to `lingxi-code/Cargo.toml` `[workspace] members`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p branding`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/branding lingxi-code/Cargo.toml
git commit -m "feat(branding): add namespace constants crate (Claude values, pre-flip)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Route all config-home helpers + scattered path literals through `branding` (Commit A part 2 — pure refactor)

**Files (each adds `branding` as a dep + replaces its literal/helper):**
- Modify (config-home helpers): `core/src/settings/loader.rs:57-67`, `migrations/src/global_config.rs:81-105`, `apps/cli/src/run.rs:1446-1450`, `tui/src/root.rs:2157`, `tui/src/screens/doctor.rs:90`, `commands/core/src/skills.rs:127-131`, `memory/src/session_memory.rs:254`, `tools/task/src/todo_store.rs:103-115`, `tools/meta/src/config.rs:116,405`
- Modify (inline copies): `bridge/src/lockfile.rs:108,144,150`, `platforms/posix/src/secure_storage/factory.rs:103-105`, `memory/src/memdir/team_paths.rs:15`
- Modify (project-level + memory consts): `memory/src/claude_md/hierarchy.rs:6-8,21-34,99,514,681`, `core/src/settings/loader.rs:67`, `migrations/src/settings_update.rs:30,118`, `skill-api/src/listing.rs:60,117`, `commands/core/src/custom_commands.rs:197,235,366`, `memory/src/memdir/{paths,scan}.rs`, `tools/worktree/src/worktree.rs:43`, `platforms/windows/src/worktree.rs:76,123`, `cron/src/tasks_file.rs:28`, `tools/cron/src/schedule_cron.rs:308`, `outputstyles/src/disk.rs`, `hooks/src/definition.rs`, `agent/src/catalog.rs`, `apps/cli/src/commands/auto_mode.rs:72` (path list in the self-mod guard prompt)
- Test: `lingxi-code/core/tests/settings_4layer_test.rs` (existing), plus a new `lingxi-code/test-harness/tests/no_inline_dotclaude_guard.rs`

**Interfaces:**
- Consumes: `branding::{DOT_DIR, GLOBAL_CONFIG_FILE, CONFIG_DIR_ENV, config_home, MEMORY_FILE, MEMORY_LOCAL_FILE}`.
- Produces: every config-home computation calls `branding::config_home(home, std::env::var_os(branding::CONFIG_DIR_ENV))`; no remaining inline `home.join(".claude")` outside `branding`.

- [ ] **Step 1: Write the guard test (fails until refactor done)**

`lingxi-code/test-harness/tests/no_inline_dotclaude_guard.rs`:

```rust
//! After consolidation, the ONLY place a `.claude`/`.claude.json`/`CLAUDE.md`
//! string literal may appear in *runtime* code is the `branding` crate.
//! Oracle-citation comments and test/fixture files are allowed.
use std::process::Command;

#[test]
fn no_inline_namespace_literals_outside_branding() {
    // ripgrep: string literals only, exclude branding crate, tests, fixtures, comments.
    let out = Command::new("rg")
        .args([
            "-n", "--no-heading",
            r#""\.claude"|"\.claude\.json"|"CLAUDE\.md"|"CLAUDE\.local\.md""#,
            "-g", "*.rs",
            "-g", "!**/branding/**",
            "-g", "!**/tests/**",
            "-g", "!**/test-harness/**",
            "-g", "!**/*_test.rs",
            "lingxi-code",
        ])
        .output()
        .expect("rg available");
    let hits = String::from_utf8_lossy(&out.stdout);
    // Filter out comment lines (// or *).
    let runtime: Vec<&str> = hits
        .lines()
        .filter(|l| {
            let code = l.splitn(3, ':').nth(2).unwrap_or("");
            let t = code.trim_start();
            !t.starts_with("//") && !t.starts_with('*') && !t.starts_with("///")
        })
        .collect();
    assert!(runtime.is_empty(), "inline namespace literals remain:\n{}", runtime.join("\n"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p test-harness --test no_inline_dotclaude_guard`
Expected: FAIL — lists the scattered inline literals.

- [ ] **Step 3: Refactor each helper + literal (values unchanged)**

For each file above: add `branding = { path = "../branding" }` (relative as appropriate) to its `Cargo.toml`; replace the inline `home.join(".claude")` / `dirs::home_dir()...join(".claude")` with `branding::config_home(&home, std::env::var_os(branding::CONFIG_DIR_ENV))`; replace project-level `project_dir.join(".claude")` with `project_dir.join(branding::DOT_DIR)`; replace `FILE_NAME`/`LOCAL_OVERRIDE_NAME` and `.claude.json` literals with the `branding` constants. Re-export the old `memory::claude_md::hierarchy` consts from `branding` to keep call sites compiling, or update call sites. Keep behavior identical (values are still Claude).

- [ ] **Step 4: Run the guard + workspace build + settings tests**

Run: `cargo test -p test-harness --test no_inline_dotclaude_guard` → PASS.
Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-run` → builds.
Run: `cargo test -p core settings` and `cargo test -p memory` → PASS (behavior unchanged).

- [ ] **Step 5: Commit (Commit A complete)**

```bash
git add -A
git commit -m "refactor(namespace): route config-home + path literals through branding crate (no behavior change)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Flip namespace constants to LingXi (Commit B — the behavior change)

**Files:**
- Modify: `lingxi-code/branding/src/lib.rs`
- Modify (managed dirs, two sites): `memory/src/claude_md/hierarchy.rs:57-69`, `apps/engine-desktop/src/settings_watch.rs:97-101`
- Modify (memory exclude/discovery globs): `orchestrator/src/prompt/memory_block.rs:241`, `memory/src/claude_md/excludes.rs:136-139`, settings `additionalIncludes` default site
- Test: `lingxi-code/memory/tests/claude_md_hierarchy_test.rs`, `lingxi-code/branding/src/lib.rs`

**Interfaces:**
- Produces: `DOT_DIR=".lingxi"`, `GLOBAL_CONFIG_FILE=".lingxi.json"`, `CONFIG_DIR_ENV="LINGXI_CONFIG_DIR"`, `MEMORY_FILE="LINGXI.md"`, `MEMORY_LOCAL_FILE="LINGXI.local.md"`, `PLUGIN_MANIFEST_DIR=".lingxi-plugin"`, `PRODUCT_NAME="LingXi"`, `ENV_PREFIX="LINGXI_"`.

- [ ] **Step 1: Write the failing tests (LingXi behavior + CLAUDE rejected)**

Add to `lingxi-code/branding/src/lib.rs` tests:

```rust
    #[test]
    fn values_are_lingxi_post_flip() {
        assert_eq!(DOT_DIR, ".lingxi");
        assert_eq!(MEMORY_FILE, "LINGXI.md");
        assert_eq!(CONFIG_DIR_ENV, "LINGXI_CONFIG_DIR");
        assert_eq!(PLUGIN_MANIFEST_DIR, ".lingxi-plugin");
        assert_eq!(PRODUCT_NAME, "LingXi");
    }
```

In `lingxi-code/memory/tests/claude_md_hierarchy_test.rs` add a hermetic case asserting `LINGXI.md` in a temp project IS discovered and a sibling `CLAUDE.md` is NOT (clean break). Mirror the existing temp-dir test pattern in that file.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p branding values_are_lingxi_post_flip` and `cargo test -p memory hierarchy`
Expected: FAIL (still `.claude`) and the old `values_are_still_claude_pre_flip` test now fails — delete that obsolete test.

- [ ] **Step 3: Flip the constants + managed dirs + globs**

Edit `branding/src/lib.rs` to the LingXi values. Update managed dirs (macOS `…/ClaudeCode`→`…/LingXi`, Windows `…\ClaudeCode`→`…\LingXi`, other `/etc/claude-code`→`/etc/lingxi`) at BOTH sites. Update the exclude glob `**/CLAUDE.md`→`**/LINGXI.md` and the `additionalIncludes` discovery default. Remove the now-obsolete pre-flip assertion test.

- [ ] **Step 4: Verify**

Run: `cargo test -p branding` → PASS.
Run: `cargo test -p memory` → PASS (incl. new LINGXI.md case).
Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-run` → builds.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(namespace): flip on-disk namespace to .lingxi / LINGXI.md / .lingxi.json

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Rename local environment variables to `LINGXI_*`

**Files:**
- Create: `lingxi-code/branding/src/env.rs` (a `rename_env(claude_name) -> &'static str` table is unnecessary; instead each call site uses the literal `LINGXI_*` name — but add a single test asserting the keep-list is untouched).
- Modify (representative; full list in spec §5.2): `orchestrator/src/conversation.rs`, `tools/agent/src/agent.rs`, `platform-api/src/subagent_spawn.rs`, `tools/task/src/task.rs`, `tools/workflow/src/lib.rs`, `mcp/src/registry.rs:520`, `compaction/src/{thresholds,orchestrator,threshold_calc}.rs`, `llm-client/src/model/{retry,context_window}.rs`, `commands/core/src/effort.rs`, `tools/shell/src/prompt.rs`, `platforms/posix/src/process/runner.rs` (child-env injection), `hooks/src/{executor,hook_payload}.rs`
- Test: `lingxi-code/test-harness/tests/env_namespace_test.rs` (new), `platforms/posix/tests/process_spawn_env_test.rs` (update)

**Interfaces:**
- Consumes: `branding::ENV_PREFIX`.
- Produces: all local flags read `LINGXI_*`; child-env injection sets `LINGXI_PROJECT_DIR`, `LINGXI_SESSION_ID`, `LINGXI_SKILL_DIR`, `LINGXI_PLUGIN_ROOT`, `LINGXI_EFFORT`.

- [ ] **Step 1: Write the failing test**

`lingxi-code/test-harness/tests/env_namespace_test.rs`:

```rust
//! Local feature flags must read LINGXI_*; protocol env vars must remain.
use std::process::Command;

fn rg(pattern: &str, extra: &[&str]) -> String {
    let mut args = vec!["-n", "--no-heading", pattern, "-g", "*.rs",
        "-g", "!**/tests/**", "-g", "!**/*_test.rs", "lingxi-code"];
    args.extend_from_slice(extra);
    let out = Command::new("rg").args(&args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn local_flags_renamed_to_lingxi() {
    // These specific local flags must no longer be read under CLAUDE_CODE_.
    for v in ["CLAUDE_CODE_ENABLE_TASKS", "CLAUDE_CODE_DISABLE_WORKFLOWS",
              "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "CLAUDE_CODE_MAX_RETRIES"] {
        let hits = rg(&format!(r#"var(_os)?\("{v}"#), &[]);
        assert!(hits.is_empty(), "{v} still read from env:\n{hits}");
    }
}

#[test]
fn protocol_env_vars_preserved() {
    for v in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN",
              "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_USE_BEDROCK"] {
        let hits = rg(&format!(r#""{v}""#), &[]);
        assert!(!hits.is_empty(), "protocol var {v} must remain in source");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p test-harness --test env_namespace_test`
Expected: FAIL — local flags still read `CLAUDE_CODE_*`.

- [ ] **Step 3: Rename the local env reads**

Enumerate candidates: `rg -n 'var(_os)?\("CLAUDE' lingxi-code --include='*.rs' | grep -v test`. For each, decide via spec §5.2: if it is in the keep-list, leave it; otherwise rename `CLAUDE_CODE_X`/`CLAUDE_X` → `LINGXI_X`. Where a `LINGXI_*` twin already exists (e.g. `mcp/registry.rs:520` reads both `CLAUDE_CODE_ENABLE_XAA` and `LINGXI_ENABLE_XAA`), drop the `CLAUDE_*` read. Update child-env injection in `platforms/posix/src/process/runner.rs` and hook payload builders to emit `LINGXI_*` names. Update settings env-parser prefix scan to `branding::ENV_PREFIX`.

- [ ] **Step 4: Verify**

Run: `cargo test -p test-harness --test env_namespace_test` → PASS.
Run: `cargo test -p platforms-posix process_spawn_env` → update assertions to `LINGXI_*`, PASS.
Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-run` → builds.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(env): rename local CLAUDE_/CLAUDE_CODE_ flags to LINGXI_ (protocol vars kept)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Rebrand identity strings (keychain, MCP clientInfo, system prompt, user-facing text)

**Files:**
- Modify (keychain): `platforms/posix/src/secure_storage/{macos,linux}.rs` (service base `"Claude Code"`→`branding::PRODUCT_NAME`)
- Modify (MCP clientInfo): the MCP client init site (`mcp/` — `name`/`title` → `branding::PRODUCT_NAME` / `"lingxi"`)
- Modify (system prompt identity): `orchestrator/src/prompt/env_block.rs:41` (remove the "kept as-is" note; "You are Claude Code"→"You are LingXi")
- Modify (user-facing text): TUI titles/help, CLI help/errors, init templates — found via guarded grep
- Test: `lingxi-code/test-harness/tests/branding_strings_test.rs` (new); update `orchestrator/tests/prompt_*` asserts

**Interfaces:**
- Consumes: `branding::PRODUCT_NAME`.

- [ ] **Step 1: Write the failing test**

`lingxi-code/test-harness/tests/branding_strings_test.rs`:

```rust
use std::process::Command;

#[test]
fn no_user_facing_claude_code_outside_denylist() {
    // User-facing "Claude Code" must be gone from prompt/tui/cli source,
    // EXCEPT denylisted referential sites (claude-code-guide subagent,
    // URLs, oracle citations, attribution payload).
    let out = Command::new("rg").args([
        "-n", "--no-heading", "Claude Code",
        "-g", "*.rs", "-g", "!**/*_test.rs", "-g", "!**/tests/**",
        "lingxi-code/orchestrator/src/prompt",
        "lingxi-code/tui/src",
        "lingxi-code/apps/cli/src",
    ]).output().unwrap();
    let hits = String::from_utf8_lossy(&out.stdout);
    let offenders: Vec<&str> = hits.lines().filter(|l| {
        let code = l.splitn(3, ':').nth(2).unwrap_or("");
        let t = code.trim_start();
        // allow comments, URLs, the attribution payload, and builtins guide
        !(t.starts_with("//") || t.contains("claude.com/claude-code")
          || t.contains("claude.ai/code") || t.contains("anthropics/claude-code")
          || t.contains("Generated with"))
    }).collect();
    assert!(offenders.is_empty(), "user-facing Claude Code remains:\n{}", offenders.join("\n"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p test-harness --test branding_strings_test`
Expected: FAIL.

- [ ] **Step 3: Rebrand the strings**

Replace user-facing `"Claude Code"`→`"LingXi"` in prompt/tui/cli (prefer `branding::PRODUCT_NAME` for non-`const`-context strings; literal `"LingXi"` where a `const` is needed). Update the keychain service base and MCP clientInfo. Edit `env_block.rs:41`. Do NOT touch the `claude-code-guide` subagent body, URLs, or the `🤖 Generated with [Claude Code]` attribution (handled separately in Task 8 decision).

- [ ] **Step 4: Verify (two byte-locked layers)**

Run: `cargo test -p test-harness --test branding_strings_test` → PASS.
Run: `cargo test -p orchestrator prompt` → update prompt asserts to LingXi, PASS.
Run: `cargo test -p tui` AND `cargo test -p test-harness` → fix/regen affected snapshots (Task 9 covers `.snap`).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(branding): rebrand identity strings (keychain, MCP clientInfo, system prompt, UI) to LingXi

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Rename `.claude-plugin/` → `.lingxi-plugin/`

**Files:**
- Modify: `plugin/src/marketplace.rs:17,89`, `plugin/src/discovery.rs`
- Test: `lingxi-code/plugin/tests/discovery_bootstrap.rs` (or `materialize.rs`)

**Interfaces:**
- Consumes: `branding::PLUGIN_MANIFEST_DIR`.

- [ ] **Step 1: Write/extend the failing test**

In a plugin test, create a temp plugin dir containing `.lingxi-plugin/plugin.json` and assert discovery finds it; assert a `.claude-plugin/` sibling is ignored.

- [ ] **Step 2: Run to verify failure** — `cargo test -p plugin discovery` → FAIL.

- [ ] **Step 3: Replace the two literals with `branding::PLUGIN_MANIFEST_DIR`.**

- [ ] **Step 4: Verify** — `cargo test -p plugin` → PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(plugin): use .lingxi-plugin manifest dir

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Rename internal Rust symbols (per-crate, staged)

**Files (staged, one crate per commit):**
- `memory`: `claude_md` module → `lingxi_md`; `ClaudeMdTier`→`LingxiMdTier`; `ClaudeMdExcluder`→`LingxiMdExcluder`; functions `claude_config_home`/`claude_home_dir`→`lingxi_*`
- Each crate with `claude_home` local var/param (≈507 total) → `lingxi_home`
- KEEP per Global Constraints: `BedrockClaude*`/`VertexClaude*`/`ClaudeAiOAuth*`, `AddFromClaudeDesktop`, `rate_limit_tier` values, TS-citation camelCase in comments, `tengu_*` names.

**Interfaces:** purely internal; no value changes.

- [ ] **Step 1: Enumerate the rename set**

Run: `grep -rn 'claude_md\|ClaudeMd\|claude_home\|claude_config_home\|claude_temp_dir' lingxi-code --include='*.rs' | grep -v 'tengu_\|//' | wc -l` — record the count as the regression baseline.

- [ ] **Step 2: Rename module + types in `memory` first**

Use `cargo`-aware rename: rename the `claude_md` directory/module, update `mod`/`use` paths, rename `ClaudeMdTier`/`ClaudeMdExcluder`. Leave the telemetry event name `tengu_memory_claude_md_hierarchy_walked` unchanged.

- [ ] **Step 3: Verify per crate**

Run after each crate: `CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-run` (catches test-only literal breaks). Then `cargo test -p <crate>`.

- [ ] **Step 4: Rename `claude_home` local vars per crate (do this crate-by-crate last)**

Disambiguate free functions named `claude_home_dir` from local vars `claude_home`. Sequence: session → engine → tui → commands → tools. Build after each.

- [ ] **Step 5: Commit per crate**

```bash
git add -A && git commit -m "refactor(<crate>): rename claude_* internal symbols to lingxi_*

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Reword brand-prose comments + decide attribution line

**Files:**
- Modify: ~30–40 generic-brand-prose comment lines in `apps/cli/src/commands/*` and `commands/core/*` (e.g. "Manage Claude Code plugins"→"Manage LingXi plugins")
- Decision: `tui/src/screens/repl.rs`/`shell/src/prompt.rs:564`/`commit_push_pr.rs:17` — the `🤖 Generated with [Claude Code](https://claude.com/claude-code)` attribution

**Interfaces:** none (comments/output text only).

- [ ] **Step 1: List the candidate comment lines**

Run: `rg -n 'Claude Code' lingxi-code --include='*.rs' | rg '^\S+:\d+:\s*//' | rg -v 'claude-code/|\.ts:|claude\.com|claude\.ai|anthropics/'`
Manually confirm each is generic prose describing *our* product (not an oracle citation, not a kept literal).

- [ ] **Step 2: Reword only those lines** to "LingXi". Leave every oracle citation and embedded literal.

- [ ] **Step 3: Decide the commit-attribution line**

Default: rebrand the rendered text to `🤖 Generated with LingXi` and drop the `claude.com/claude-code` URL (LingXi has no public URL yet). Update the matching golden/parity assertion if one exists. If the user prefers to leave it, skip — record which.

- [ ] **Step 4: Verify** — `cargo test -p test-harness --no-run` builds; `cargo test -p tui` passes (regen snapshots in Task 9 if affected).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "docs(branding): reword product-prose comments; rebrand commit attribution

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Client cross-process lockstep + parity reconciliation + final verification

**Files (clients — change with engine atomically):**
- Modify: `clients/shared/src/lockfile.ts:35`, `clients/electron/src/main/bridge.ts:33`, `clients/shared/src/client.ts:50,147`, `clients/shared/scripts/e2e.mjs` (bridge dir `~/.claude/bridge`→`~/.lingxi/bridge`; header `X-Claude-Code-Ide-Authorization`→`X-LingXi-Ide-Authorization`)
- Modify (engine side of those contracts): `bridge/src/lockfile.rs`, `bridge/src/mcp_endpoint.rs` (`AUTH_HEADER_NAME`), `apps/bridge-server/src/main.rs`
- Modify (Android mock + asserts): the mock chat title `"重装 Claude Code"` and `AppFlowUiTest.kt`, `SessionStateTest.kt`, `DrawerSearchTest.kt` (update the lowercase-`claude` search query too)
- Modify (parity): `test-harness/src/parity/fixtures/*.json` (~9 UI/path), inline `.rs` literals in `parity_mcp_initialize.rs`, `parity_tui_permission_dialogs.rs`, `parity_full_v0_4_0_smoke.rs`, `parity_system_tools.rs`, `parity_team_tools.rs`, `parity_settings_merge.rs`; `parity_init_template.json` (re-derive `sha256`+`byte_length`+substrings); `tui/tests/snapshots/*.snap`

**Interfaces:** the header name and bridge dir must match exactly on both sides.

- [ ] **Step 1: Change the bridge dir + auth header on BOTH sides**

Edit the Rust `AUTH_HEADER_NAME` and bridge-dir computation and the four TS sites together. Keep the `mcp` WS subprotocol and the `ideName`/`authToken` lockfile schema unchanged (not Claude-branded).

- [ ] **Step 2: Update Android mock title + 3 test assertions** to "重装 LingXi" (and the DrawerSearch query).

- [ ] **Step 3: Reconcile parity fixtures + inline `.rs` literals**

Update the ~9 UI/path fixtures and the duplicated inline literals to LingXi values. For `parity_init_template.json`: regenerate the `/init` prompt, then re-derive `sha256`, `byte_length`, and the asserted substrings/first_sentence together. Leave pure-protocol fixtures (betas, model IDs, oauth) untouched.

- [ ] **Step 4: Regenerate insta snapshots**

Run: `INSTA_UPDATE=always cargo test -p tui` then review each `.snap` diff (bypass-permission ×2, doctor "LingXi home", memory selector LINGXI.md, agents-screen description). Accept only brand/path diffs.

- [ ] **Step 5: Full verification gate**

Run, expecting all green:
```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_RELEASE_DEBUG=0 cargo test --workspace
cargo test -p tui
cargo test -p test-harness
```
Run the no-split-brain guard (must be empty for runtime sites, denylist excluded):
```bash
rg -n '"\.claude"|"CLAUDE_CODE_|var(_os)?\("CLAUDE_(?!CODE_(ENTRYPOINT|OAUTH_TOKEN|USE_|EXTRA_METADATA|DISABLE_EXPERIMENTAL|VERSION)|AGENT_SDK)' \
  lingxi-code clients -g '*.rs' -g '*.ts' -g '!**/branding/**' -g '!**/tests/**' -g '!**/*_test.rs' | rg -v '//|claude-code/|\.ts:'
```
Manual smoke (built binary): fresh `~/.lingxi` is created; `LINGXI.md` is loaded; `/login` re-auth works; desktop/Electron discovers the engine via `~/.lingxi/bridge`; `/doctor` shows LingXi paths.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(clients+parity): lockstep bridge dir + IDE auth header; reconcile parity fixtures/snapshots

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review notes (author)

- **Spec coverage:** §4 arch → Tasks 1–2; §4.3 two-commit flip → Tasks 2 (A) + 3 (B); §5 table → Tasks 3–7; §5.2 env keep-list → Task 4; §6 clean break (no-migration, LINGXI.md rejects CLAUDE.md) → Task 3 tests; §7 lockstep → Task 9; §8 denylist → Global Constraints + per-task "do NOT touch"; §9 comments → Task 8; §10 parity → Task 9; §11 sequencing → Task order; §12 verification → Task 9 Step 5.
- **Placeholders:** none — each task carries real test code, exact seams, and commands. Items the spec marks "verify during implementation" (`CLAUDE_TOKEN`, inert `BRIEF_*`) are resolved inside Task 4 Step 3 / Task 2 enumeration.
- **Type consistency:** `branding::{DOT_DIR, MEMORY_FILE, CONFIG_DIR_ENV, PLUGIN_MANIFEST_DIR, PRODUCT_NAME, ENV_PREFIX, config_home}` used consistently across Tasks 1–6; `config_home(home, env)` signature stable.
