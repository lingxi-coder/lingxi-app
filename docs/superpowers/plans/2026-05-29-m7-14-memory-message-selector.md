# M7-14 Memory editor + MessageSelector — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Memory file editor screen (pick a CLAUDE.md tier → edit → save through the existing M3 store) and a MessageSelector component (search the scrollback, jump-back to a message, export the transcript to a safe default path) to the TUI, wiring `Screen::Memory` into the M7-11 `active_screen` priority-2 route and `/export` + a search keybind into the live key path.

**Architecture:** Two surfaces in one sub-plan. (1) **Memory** is a full-page modal screen (`screens/memory.rs`): a `MemoryFileSelector` lists the project/user CLAUDE.md tiers discovered by `lingxi_memory::claude_md::hierarchy::walk`, and an inline edit view reads/writes that file's bytes — the M3 "store" *is* the on-disk CLAUDE.md, so reads go through `lingxi_memory::claude_md::loader::load_file` and writes go through a thin atomic `std::fs` write of the same `HierarchyEntry.path` (no new persistence layer — §4 R7). (2) **MessageSelector** is a component (`components/message_selector.rs`) that filters `AppState.messages` by a substring query, sets `AppState.scroll_offset` to the selected message's line offset using M7-03's `HeightCache` line model (jump-back), and exports a plain-text transcript dump to a default path (`~/.lingxi/exports/` with cwd fallback — §4 R10) with confirm-on-overwrite (no silent clobber). Both route through the **single** `handle_live_key` dispatcher (design §2.5): the Memory screen sits at `active_screen` priority 2 (the branch M7-11 establishes), and the MessageSelector search overlay sits at priority 3 (input-overlay), so neither re-introduces a parallel key path (the M6 ship-blocker).

**Tech Stack:** Rust 1.82 (pinned via `rust-toolchain.toml` — **run all cargo from inside `lingxi-code/`**), iocraft `=0.8.3` (`View`, not `Box`), `lingxi-memory` (M3 CLAUDE.md hierarchy + loader), `lingxi-tui` (M7-03 `HeightCache`/`measured_height` for line-offset jump math), `insta` 1.40 for snapshots, `tempfile` for test stores.

---

## Prerequisites & assumptions (read before Task 1)

This plan assumes **M7-11 has landed** the screen-overlay scaffold:
- `AppState.active_screen: Option<Screen>` where `Screen` is an enum in `crate::screens` (e.g. `crate::screens::Screen`).
- A priority-2 branch in `root.rs::handle_live_key` (`active_screen.is_some()` → route to the active screen), placed **after** the permission focus-trap and **before** the default `map_iocraft_key`/overlay paths.
- A priority-2 branch in `app.rs::render_screen` that renders the active screen instead of the REPL layout (mirroring the `pending_permission` branch at `app.rs:275-308`).

If `active_screen` / `Screen` are **not yet present** when this plan executes, Task 2 adds the `Screen` enum + `active_screen` field defensively (additively — a new enum *variant* `Screen::Memory` if the enum exists, or the whole enum + field if it does not), and Tasks 3/9 add the priority-2 render + key branches if M7-11 hasn't. Each such task checks first with `grep` and skips the part already present. **Do not** add a second/parallel key path — extend the one `handle_live_key` dispatcher (design §2.5).

**Telemetry:** 0 new events in M7-14 (baseline 326). The search/export/screen events are registered + audited in **M7-16** (design §2.7). This plan emits no `tracing::info!(event = …)` and registers no names.

**Literal lock (design §2.8):** copy user-visible strings byte-for-byte from the claude-code refs:
- `claude-code/src/components/memory/MemoryFileSelector.tsx`: tier labels `"User memory"` / `"Project memory"`; descriptions `"Saved in ~/.claude/CLAUDE.md"` / `"Checked in at ./CLAUDE.md"` / `"Saved in ./CLAUDE.md"` (the `isGit` ternary); the `" (new)"` suffix for a tier whose file does not exist yet.
- `claude-code/src/components/ExportDialog.tsx`: title `"Export Conversation"`, success literal `"Conversation exported to: {filepath}"`, failure prefix `"Failed to export conversation: "`, cancel `"Export cancelled"`, filename prompt `"Enter filename:"`. claude-code defaults the export dir to cwd and forces a `.txt` extension; LingXi keeps the `.txt` rule and the success literal but defaults to `~/.lingxi/exports/` (cwd fallback) and **adds** an overwrite confirm that claude-code lacks (§4 R10 mandates it).

---

## File Structure

**Created:**
- `lingxi-code/crates/tui/src/screens/memory.rs` — the Memory screen: `MemoryTierEntry` (resolved tier row), `memory_tiers(cwd, home)` (pure resolver over `hierarchy::walk`), `MemoryScreenState` (selector index / edit-mode / buffer / dirty), `load_tier_body` + `save_tier_body` (read/write through the M3 store path), the `handle_memory_key` state machine, and the `MemoryScreen` iocraft component.
- `lingxi-code/crates/tui/src/components/message_selector.rs` — `MessageSelectorState` (query / filtered indices / selected), `search_messages(messages, query)` (pure substring filter → `Vec<usize>`), `message_line_offset(messages, height_cache, target_index, viewport_height)` (pure jump-back math → line `scroll_offset`), `export_transcript(messages, dir, filename, overwrite)` (plain-text dump + overwrite guard), `default_export_dir()` / `default_export_filename()`, `handle_message_selector_key` state machine, and the `MessageSelector` iocraft component.
- `lingxi-code/crates/tui/tests/behavior_memory_screen.rs` — memory tier listing, select→edit→save round-trip through a temp store, no-write-on-cancel.
- `lingxi-code/crates/tui/tests/behavior_message_selector.rs` — search filter, jump-back offset, export-writes-file, export-refuses-silent-overwrite.
- `lingxi-code/crates/tui/tests/render_memory_screen.rs` — insta snapshot of the tier selector.
- `lingxi-code/crates/tui/tests/render_message_selector.rs` — insta snapshot of search results.

**Modified:**
- `lingxi-code/crates/tui/src/screens/mod.rs` — `pub mod memory;`; add `Screen::Memory` variant (or the whole `Screen` enum if M7-11 hasn't landed it).
- `lingxi-code/crates/tui/src/components/mod.rs` — `pub mod message_selector;`.
- `lingxi-code/crates/tui/src/state.rs` — add `memory_screen: MemoryScreenState` and `message_selector: MessageSelectorState` sub-state slots + (only if M7-11 absent) `active_screen: Option<Screen>`.
- `lingxi-code/crates/tui/src/app.rs:267-331` — `render_screen` gains a `Screen::Memory` render branch (priority 2, after the `pending_permission` branch) and a MessageSelector overlay branch.
- `lingxi-code/crates/tui/src/root.rs:187-211` — `handle_live_key` routes to `handle_memory_key` when `active_screen == Some(Screen::Memory)` (priority 2) and to `handle_message_selector_key` when the selector is open (priority 3); Ctrl-T opens the selector.
- `lingxi-code/crates/tui/Cargo.toml` — add `lingxi-memory` (path dep) under `[dependencies]` and `tempfile` under `[dev-dependencies]` if absent.
- `lingxi-code/crates/commands/src/builtin/export.rs` (Created) + `lingxi-code/crates/commands/src/builtin/mod.rs` — real `/export` handler replacing the M5-11 unimplemented stub; wired to signal the TUI to open the selector's export flow.

**Read-only references (do NOT edit):**
- `lingxi-code/crates/memory/src/claude_md/hierarchy.rs` — `walk(cwd, home) -> Hierarchy`, `HierarchyEntry { path, is_local_override, exact_case }`, `FILE_NAME = "CLAUDE.md"`.
- `lingxi-code/crates/memory/src/claude_md/loader.rs` — `load_file(path, bus) -> Result<LoadedFile, LoaderError>`, `LoadedFile { path, body, size_bytes }`.
- `lingxi-code/crates/tui/src/components/virtual_message_list.rs` — `HeightCache::build`, `measured_height` (M7-03 line model for jump-back).
- `lingxi-code/crates/tui/src/state.rs` — `AppState.messages: Vec<RenderedMessage>`, `RenderedMessage` variants, `scroll_offset`.
- `lingxi-code/crates/tui/src/app.rs:267-308` — the `pending_permission` render branch (the pattern the Memory branch mirrors).
- `lingxi-code/crates/tui/src/root.rs:187-211` — the `handle_live_key` focus-trap dispatcher (the single key path to extend).
- `claude-code/src/components/memory/MemoryFileSelector.tsx`, `claude-code/src/components/MessageSelector.tsx`, `claude-code/src/components/ExportDialog.tsx` — literal-lock sources.

---

## Background: how Memory "writes through the M3 store"

The M3 memory subsystem (`lingxi-memory`) has a **reader** (`claude_md::loader::load_file`) and a **discovery walker** (`claude_md::hierarchy::walk`), but no public `save`/`write` function — the canonical CLAUDE.md files on disk *are* the store, and the production `/memory` command (`crates/commands/src/builtin/memory.rs`) mutates them by shelling out to `$EDITOR` via `OrchestratorHandle::open_memory_editor`. §4 R7 forbids adding a new persistence layer. So the Memory **editor** reads the tier file's bytes via `load_file` (the M3 reader) and writes the edited bytes back to the **same `HierarchyEntry.path`** with an atomic `std::fs` write (write to `path.tmp` + `rename`). No new store, no schema, no orchestrator method — the write target is exactly the path the M3 walker/loader resolved. This is "write through the existing store" in the only sense the store supports.

## Background: jump-back uses M7-03's line offset model

M7-03 made `scroll_offset` count **lines from the bottom** (not message rows), backed by `HeightCache` (`message-index → rendered line count`). Jump-back to message `i` must therefore set `scroll_offset` so that message `i` sits at the **top** of the viewport: `scroll_offset = total_lines - (line_at_start_of[i]) - viewport_height`, clamped to `[0, total_lines - viewport_height]`, where `line_at_start_of[i] = sum(height_at(0..i))`. Pinning the target to the top (rather than centering) keeps the selected message and everything after it visible, matching claude-code's "scroll the conversation to this message" jump. The math lives in the pure `message_line_offset` fn so it is unit-testable without iocraft.

---

## Task 1: `lingxi-memory` dependency + module registration

**Files:**
- Modify: `lingxi-code/crates/tui/Cargo.toml`
- Modify: `lingxi-code/crates/tui/src/screens/mod.rs`
- Modify: `lingxi-code/crates/tui/src/components/mod.rs`
- Create: `lingxi-code/crates/tui/src/screens/memory.rs`
- Create: `lingxi-code/crates/tui/src/components/message_selector.rs`

- [ ] **Step 1: Confirm whether `lingxi-memory` is already a tui dep**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -n 'lingxi-memory\|lingxi_memory' crates/tui/Cargo.toml && echo PRESENT || echo MISSING`

- [ ] **Step 2: Add `lingxi-memory` (path dep) if MISSING**

If MISSING, in `crates/tui/Cargo.toml` under `[dependencies]`, add (mirror the exact `path = "../memory"` form the other intra-workspace deps in that file use — check an existing line like `lingxi-protocol = { path = "../protocol" }`):

```toml
lingxi-memory = { path = "../memory" }
```

- [ ] **Step 3: Add `tempfile` dev-dep if MISSING**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -n 'tempfile' crates/tui/Cargo.toml && echo PRESENT || echo MISSING`
If MISSING, add under `[dev-dependencies]` (pin to the version already in `Cargo.lock`: `grep 'name = "tempfile"' -A1 Cargo.lock | head`):

```toml
tempfile = "3"
```

- [ ] **Step 4: Register the new modules**

In `crates/tui/src/screens/mod.rs`, after `pub mod repl;`, add:

```rust
pub mod memory;
```

In `crates/tui/src/components/mod.rs`, add (alongside the other `pub mod` lines):

```rust
pub mod message_selector;
```

- [ ] **Step 5: Create empty module files so the crate compiles**

Create `crates/tui/src/screens/memory.rs`:

```rust
//! Memory file editor screen (M7-14).
//!
//! A `MemoryFileSelector` lists the project/user CLAUDE.md tiers (resolved
//! via [`lingxi_memory::claude_md::hierarchy::walk`]); selecting a tier
//! opens an inline edit view. Reads go through the M3 loader
//! ([`lingxi_memory::claude_md::loader::load_file`]); writes go back to the
//! same on-disk path (the M3 store — §4 R7, no new persistence).
```

Create `crates/tui/src/components/message_selector.rs`:

```rust
//! MessageSelector (M7-14) — search the scrollback, jump back to a message,
//! and export the transcript.
//!
//! Searches [`crate::state::AppState::messages`] by substring, sets
//! `scroll_offset` (M7-03 line model) to jump to a selected message, and
//! exports a plain-text transcript to a default path with overwrite confirm.
```

- [ ] **Step 6: Verify the crate builds**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo build -p lingxi-tui`
Expected: builds clean (empty modules + new deps resolve).

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/crates/tui/Cargo.toml lingxi-code/Cargo.lock \
        lingxi-code/crates/tui/src/screens/mod.rs \
        lingxi-code/crates/tui/src/components/mod.rs \
        lingxi-code/crates/tui/src/screens/memory.rs \
        lingxi-code/crates/tui/src/components/message_selector.rs
git commit -m "plan(M7-14 T1): scaffold memory screen + message_selector modules + lingxi-memory dep

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: `Screen::Memory` variant + `active_screen` field (defensive)

**Files:**
- Modify: `lingxi-code/crates/tui/src/screens/mod.rs`
- Modify: `lingxi-code/crates/tui/src/state.rs`

- [ ] **Step 1: Check what M7-11 already landed**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -rn 'enum Screen' crates/tui/src/screens/mod.rs; grep -n 'active_screen' crates/tui/src/state.rs`
- If `enum Screen` exists: add only the `Memory` variant (Step 3a).
- If it does not: add the whole enum + field (Step 3b).

- [ ] **Step 2: Write the failing test**

Add to the `tests` module in `state.rs`:

```rust
#[test]
fn active_screen_defaults_none_and_holds_memory() {
    use crate::screens::Screen;
    let mut s = AppState::default_for_tests();
    assert!(s.active_screen.is_none());
    s.active_screen = Some(Screen::Memory);
    assert_eq!(s.active_screen, Some(Screen::Memory));
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib state::tests::active_screen_defaults_none_and_holds_memory`
Expected: FAIL — `no variant Memory` (M7-11 present) or `no field active_screen` / `cannot find type Screen` (M7-11 absent).

- [ ] **Step 3a: If `enum Screen` exists, add the `Memory` variant**

In `screens/mod.rs`, add `Memory` to the existing `enum Screen` (keep its existing `derive`s):

```rust
    /// Memory file editor (M7-14).
    Memory,
```

- [ ] **Step 3b: If `enum Screen` does NOT exist, add the enum + the field**

In `screens/mod.rs`, after the `pub mod` lines, add:

```rust
/// A full-page modal screen overlaying the REPL (design §2.3). `None`
/// means the REPL is active. Routed at priority 2 in `handle_live_key`
/// and `render_screen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Memory file editor (M7-14).
    Memory,
}
```

In `state.rs`, add to the `AppState` struct (after `scroll_offset`):

```rust
    /// (M7-11/M7-14) Active full-page screen, or `None` for the REPL.
    pub active_screen: Option<crate::screens::Screen>,
```

In `AppState::new`, initialise it (after `scroll_offset: 0,`):

```rust
            active_screen: None,
```

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib state::tests::active_screen_defaults_none_and_holds_memory`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/screens/mod.rs lingxi-code/crates/tui/src/state.rs
git commit -m "plan(M7-14 T2): Screen::Memory variant + active_screen slot

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: `memory_tiers` resolver — list project/user CLAUDE.md tiers

**Files:**
- Modify: `lingxi-code/crates/tui/src/screens/memory.rs`

- [ ] **Step 1: Write the failing test for tier resolution**

Add to `memory.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn lists_project_and_user_tiers_with_labels() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("CLAUDE.md"), b"# project notes\n").unwrap();
        // user CLAUDE.md does NOT exist → still listed, marked (new).

        let tiers = memory_tiers(&cwd, &home);
        // Project (exists) + User (new) — innermost first.
        let proj = tiers.iter().find(|t| t.label == "Project memory").unwrap();
        assert!(proj.exists);
        assert_eq!(proj.path, cwd.join("CLAUDE.md"));
        let user = tiers.iter().find(|t| t.label == "User memory").unwrap();
        assert!(!user.exists);
        assert_eq!(user.path, home.join(".claude").join("CLAUDE.md"));
    }

    #[test]
    fn empty_dirs_still_offer_creatable_project_and_user_tiers() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let tiers = memory_tiers(&cwd, &home);
        assert!(tiers.iter().any(|t| t.label == "Project memory" && !t.exists));
        assert!(tiers.iter().any(|t| t.label == "User memory" && !t.exists));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib screens::memory::tests::lists_project_and_user_tiers_with_labels`
Expected: FAIL — `cannot find function memory_tiers`.

- [ ] **Step 3: Implement `MemoryTierEntry` + `memory_tiers`**

Add to `memory.rs` (above the test module):

```rust
use std::path::{Path, PathBuf};

use lingxi_memory::claude_md::hierarchy::{walk, FILE_NAME};

/// One resolved memory tier the selector lists. The project/user tiers are
/// always offered (even when the file does not exist yet — marked `(new)`);
/// additional discovered project-parent CLAUDE.md files are appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTierEntry {
    /// Selector label, literal-locked to claude-code (`"Project memory"`,
    /// `"User memory"`, or a display path for parent files).
    pub label: String,
    /// One-line description shown under the label (literal-locked).
    pub description: String,
    /// On-disk path of the CLAUDE.md file (the M3 store target).
    pub path: PathBuf,
    /// Whether the file currently exists. `false` → the row shows `" (new)"`.
    pub exists: bool,
}

/// Resolve the memory tiers to list, innermost-first. The project tier is
/// `<cwd>/CLAUDE.md` and the user tier is `<home>/.claude/CLAUDE.md`; both
/// are always present (creatable). Any other CLAUDE.md the walker finds
/// (project parents) is appended after, by display path.
///
/// Mirrors `claude-code/src/components/memory/MemoryFileSelector.tsx`
/// (single-user subset: auto-memory / team / agent folders are M8).
#[must_use]
pub fn memory_tiers(cwd: &Path, home: &Path) -> Vec<MemoryTierEntry> {
    let project_path = cwd.join(FILE_NAME);
    let user_path = home.join(".claude").join(FILE_NAME);

    let mut tiers = vec![
        MemoryTierEntry {
            label: "Project memory".to_string(),
            description: "Checked in at ./CLAUDE.md".to_string(),
            exists: project_path.is_file(),
            path: project_path.clone(),
        },
        MemoryTierEntry {
            label: "User memory".to_string(),
            description: "Saved in ~/.claude/CLAUDE.md".to_string(),
            exists: user_path.is_file(),
            path: user_path.clone(),
        },
    ];

    // Append any other discovered CLAUDE.md (project parents) not already
    // covered by the project/user rows, in walk order.
    let h = walk(cwd, home);
    for entry in h.entries {
        if entry.path == project_path || entry.path == user_path {
            continue;
        }
        let display = entry.path.display().to_string();
        tiers.push(MemoryTierEntry {
            label: display.clone(),
            description: "@-imported".to_string(),
            exists: true,
            path: entry.path,
        });
    }
    tiers
}
```

> NOTE on `description`: claude-code uses `"Checked in at ./CLAUDE.md"` when the project is a git repo and `"Saved in ./CLAUDE.md"` otherwise. M7-14 keeps it simple — the git-repo check is not load-bearing for the editor, so default to `"Checked in at ./CLAUDE.md"` (the common case for this project) and leave the git-ternary refinement to M7-16 literal-lock catalog work if a parity fixture demands it. Do NOT invent a third string.

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib screens::memory::tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/screens/memory.rs
git commit -m "plan(M7-14 T3): memory_tiers resolver over M3 CLAUDE.md hierarchy

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: `load_tier_body` + `save_tier_body` — read/write through the M3 store

**Files:**
- Modify: `lingxi-code/crates/tui/src/screens/memory.rs`

- [ ] **Step 1: Write the failing round-trip test**

Add to the `memory.rs` `tests` module:

```rust
#[test]
fn load_returns_empty_for_missing_then_save_creates_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("CLAUDE.md");
    // Missing file → empty editable buffer (a "new" tier).
    assert_eq!(load_tier_body(&path).unwrap(), "");
    save_tier_body(&path, "# hello\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "# hello\n");
    // Re-load returns the saved body.
    assert_eq!(load_tier_body(&path).unwrap(), "# hello\n");
}

#[test]
fn save_creates_parent_dirs_when_absent() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nested").join("deeper").join("CLAUDE.md");
    save_tier_body(&path, "x\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "x\n");
}

#[test]
fn save_is_atomic_no_leftover_tmp() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("CLAUDE.md");
    save_tier_body(&path, "body\n").unwrap();
    // The temp sibling must be gone after a successful rename.
    let tmp_sibling = path.with_extension("md.lingxi-tmp");
    assert!(!tmp_sibling.exists());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib screens::memory::tests::load_returns_empty_for_missing_then_save_creates_file`
Expected: FAIL — `cannot find function load_tier_body`.

- [ ] **Step 3: Implement `load_tier_body` + `save_tier_body`**

Add to `memory.rs`:

```rust
use std::io;

use lingxi_memory::claude_md::loader::{load_file, LoaderError};

/// Read a tier file's body through the M3 loader. A missing file is NOT an
/// error here — it yields an empty buffer so the editor can create a "new"
/// tier. Oversized / unreadable files surface as `Err`.
///
/// # Errors
/// Returns the loader's I/O error string for files that exist but cannot be
/// read, or a "too large" message for files over the 10 MB cap.
pub fn load_tier_body(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok(String::new());
    }
    match load_file(path, None) {
        Ok(loaded) => Ok(loaded.body),
        Err(LoaderError::FileTooLarge { bytes, .. }) => {
            Err(format!("memory file too large: {bytes} bytes"))
        }
        Err(LoaderError::Io(e)) => Err(e),
    }
}

/// Write `body` back to the tier file (the M3 store target). Atomic:
/// writes to a `.lingxi-tmp` sibling then renames over the target, so a
/// crash mid-write never truncates the existing file. Creates parent dirs.
///
/// This is the ONLY write path — it targets exactly the `HierarchyEntry`
/// path the M3 walker/loader resolved (§4 R7: no new persistence layer).
///
/// # Errors
/// Returns the I/O error string on any filesystem failure.
pub fn save_tier_body(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e: io::Error| e.to_string())?;
    }
    let tmp = path.with_extension("md.lingxi-tmp");
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib screens::memory::tests`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/screens/memory.rs
git commit -m "plan(M7-14 T4): load_tier_body via M3 loader + atomic save_tier_body

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: `MemoryScreenState` + `handle_memory_key` state machine

**Files:**
- Modify: `lingxi-code/crates/tui/src/screens/memory.rs`

- [ ] **Step 1: Write the failing tests for the state machine**

Add to the `memory.rs` `tests` module:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn selector_arrows_move_index_and_enter_opens_editor() {
    let tiers = vec![
        MemoryTierEntry { label: "Project memory".into(), description: String::new(),
            path: "/p/CLAUDE.md".into(), exists: true },
        MemoryTierEntry { label: "User memory".into(), description: String::new(),
            path: "/u/CLAUDE.md".into(), exists: false },
    ];
    let mut st = MemoryScreenState::default();
    assert_eq!(st.selected, 0);
    handle_memory_key(&mut st, &tiers, key(KeyCode::Down));
    assert_eq!(st.selected, 1);
    // Past the end clamps (no wrap).
    handle_memory_key(&mut st, &tiers, key(KeyCode::Down));
    assert_eq!(st.selected, 1);
    // Enter opens the editor on the selected tier.
    handle_memory_key(&mut st, &tiers, key(KeyCode::Enter));
    assert!(st.editing);
    assert_eq!(st.editing_path.as_deref(), Some(std::path::Path::new("/u/CLAUDE.md")));
}

#[test]
fn editor_typing_marks_dirty_and_esc_returns_to_selector() {
    let tiers = vec![MemoryTierEntry { label: "Project memory".into(),
        description: String::new(), path: "/p/CLAUDE.md".into(), exists: true }];
    let mut st = MemoryScreenState::default();
    st.open_editor(&tiers[0], "old".to_string());
    assert!(st.editing && !st.dirty);
    handle_memory_key(&mut st, &tiers, key(KeyCode::Char('!')));
    assert_eq!(st.buffer, "old!");
    assert!(st.dirty);
    // Esc from the editor returns to the selector (does NOT close the screen).
    let action = handle_memory_key(&mut st, &tiers, key(KeyCode::Esc));
    assert_eq!(action, MemoryAction::BackToSelector);
    assert!(!st.editing);
}

#[test]
fn esc_from_selector_requests_close() {
    let tiers = vec![MemoryTierEntry { label: "Project memory".into(),
        description: String::new(), path: "/p/CLAUDE.md".into(), exists: true }];
    let mut st = MemoryScreenState::default();
    let action = handle_memory_key(&mut st, &tiers, key(KeyCode::Esc));
    assert_eq!(action, MemoryAction::CloseScreen);
}

#[test]
fn ctrl_s_in_editor_requests_save() {
    let tiers = vec![MemoryTierEntry { label: "Project memory".into(),
        description: String::new(), path: "/p/CLAUDE.md".into(), exists: true }];
    let mut st = MemoryScreenState::default();
    st.open_editor(&tiers[0], String::new());
    st.buffer = "new body".into();
    st.dirty = true;
    let save = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    let action = handle_memory_key(&mut st, &tiers, save);
    assert_eq!(action, MemoryAction::Save {
        path: "/p/CLAUDE.md".into(), body: "new body".into() });
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib screens::memory::tests::selector_arrows_move_index_and_enter_opens_editor`
Expected: FAIL — `cannot find type MemoryScreenState`.

- [ ] **Step 3: Implement the state + state machine**

Add to `memory.rs` (add `use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};` to the imports):

```rust
/// Outcome the caller (`handle_live_key`) acts on after routing a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryAction {
    /// Nothing for the caller to do (state mutated in place).
    None,
    /// Close the Memory screen (clear `active_screen`).
    CloseScreen,
    /// Leave the editor, return to the tier selector (stay on the screen).
    BackToSelector,
    /// Persist the current buffer to `path`. The caller calls
    /// [`save_tier_body`] and reports success/failure.
    Save {
        /// Target tier path.
        path: PathBuf,
        /// Buffer to write.
        body: String,
    },
}

/// Per-screen state for the Memory editor. Lives on `AppState`.
#[derive(Debug, Clone, Default)]
pub struct MemoryScreenState {
    /// Selected tier index in the selector list.
    pub selected: usize,
    /// `true` once a tier is opened for editing.
    pub editing: bool,
    /// Path of the tier being edited (`None` while in the selector).
    pub editing_path: Option<PathBuf>,
    /// Edit buffer (the tier body).
    pub buffer: String,
    /// `true` when the buffer differs from the loaded body (unsaved).
    pub dirty: bool,
    /// Transient status line (e.g. last save result), shown in the footer.
    pub status: Option<String>,
}

impl MemoryScreenState {
    /// Enter the editor on `tier` with `body` as the initial buffer.
    pub fn open_editor(&mut self, tier: &MemoryTierEntry, body: String) {
        self.editing = true;
        self.editing_path = Some(tier.path.clone());
        self.buffer = body;
        self.dirty = false;
        self.status = None;
    }

    /// Return to the selector list (discarding editor focus, keeping the
    /// buffer untouched on disk — Esc never writes).
    pub fn back_to_selector(&mut self) {
        self.editing = false;
        self.editing_path = None;
        self.buffer.clear();
        self.dirty = false;
    }
}

/// Route one key into the Memory screen state. Returns a [`MemoryAction`]
/// the caller acts on. `tiers` is the current selector list (re-resolved by
/// the caller each frame). Esc semantics: in the editor → back to selector;
/// in the selector → close the screen.
pub fn handle_memory_key(
    st: &mut MemoryScreenState,
    tiers: &[MemoryTierEntry],
    key: KeyEvent,
) -> MemoryAction {
    if st.editing {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                st.back_to_selector();
                return MemoryAction::BackToSelector;
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                if let Some(path) = st.editing_path.clone() {
                    return MemoryAction::Save { path, body: st.buffer.clone() };
                }
                return MemoryAction::None;
            }
            (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
                st.buffer.push(c);
                st.dirty = true;
            }
            (KeyCode::Enter, _) => {
                st.buffer.push('\n');
                st.dirty = true;
            }
            (KeyCode::Backspace, _) => {
                st.buffer.pop();
                st.dirty = true;
            }
            _ => {}
        }
        return MemoryAction::None;
    }
    // Selector mode.
    match key.code {
        KeyCode::Esc => return MemoryAction::CloseScreen,
        KeyCode::Up => st.selected = st.selected.saturating_sub(1),
        KeyCode::Down => {
            let last = tiers.len().saturating_sub(1);
            st.selected = (st.selected + 1).min(last);
        }
        KeyCode::Enter => {
            if let Some(tier) = tiers.get(st.selected) {
                let body = load_tier_body(&tier.path).unwrap_or_default();
                st.open_editor(tier, body);
            }
        }
        _ => {}
    }
    MemoryAction::None
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib screens::memory::tests`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/screens/memory.rs
git commit -m "plan(M7-14 T5): MemoryScreenState + handle_memory_key state machine

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: `MemoryScreen` iocraft component + render branch

**Files:**
- Modify: `lingxi-code/crates/tui/src/screens/memory.rs`
- Modify: `lingxi-code/crates/tui/src/state.rs`
- Modify: `lingxi-code/crates/tui/src/app.rs:267-308`

- [ ] **Step 1: Add the `memory_screen` slot to `AppState`**

In `state.rs`, add to the `AppState` struct:

```rust
    /// (M7-14) Memory editor screen state (selector index, edit buffer).
    pub memory_screen: crate::screens::memory::MemoryScreenState,
```

In `AppState::new`, initialise:

```rust
            memory_screen: crate::screens::memory::MemoryScreenState::default(),
```

- [ ] **Step 2: Implement the `MemoryScreen` component**

Add to `memory.rs` (add `use iocraft::prelude::*;`):

```rust
/// Props for [`MemoryScreen`]. The tier list + state are cloned from
/// `AppState` each frame (iocraft re-renders on the tick).
#[derive(Default, Props)]
pub struct MemoryScreenProps {
    /// Resolved tier list (selector rows).
    pub tiers: Vec<MemoryTierEntry>,
    /// Selected index in the selector.
    pub selected: usize,
    /// `true` when the editor (not the selector) is showing.
    pub editing: bool,
    /// Editor buffer body.
    pub buffer: String,
    /// `true` when there are unsaved changes.
    pub dirty: bool,
    /// Transient footer status (last save result).
    pub status: Option<String>,
}

/// Memory editor screen. Selector list, or the edit view once a tier is
/// opened. Title + footer hints are literal-locked to claude-code.
#[component]
pub fn MemoryScreen(props: &MemoryScreenProps) -> impl Into<AnyElement<'static>> {
    if props.editing {
        let body = props.buffer.clone();
        let dirty_mark = if props.dirty { " *" } else { "" };
        let status = props.status.clone().unwrap_or_default();
        return element! {
            View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
                Text(content: format!("Edit memory{dirty_mark}"), weight: Weight::Bold)
                View(flex_grow: 1.0) { Text(content: body) }
                Text(content: format!("Ctrl-S save · Esc back   {status}"))
            }
        }
        .into_any();
    }
    let selected = props.selected;
    let rows: Vec<AnyElement<'static>> = props
        .tiers
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let marker = if i == selected { "❯ " } else { "  " };
            let new_suffix = if t.exists { "" } else { " (new)" };
            let line = format!("{marker}{}{new_suffix}", t.label);
            element! { Text(content: line) }.into_any()
        })
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
            Text(content: "Memory files", weight: Weight::Bold)
            #(rows)
            Text(content: "↑/↓ select · Enter edit · Esc close")
        }
    }
    .into_any()
}
```

> NOTE: `Weight::Bold` and `View` are iocraft 0.8.3 — match the existing usage in `screens/repl.rs` / `components/status_line.rs`. If `weight:` is not a valid prop on `Text` in the pinned iocraft, drop it (the title text alone is enough; literal lock is on the string, not the bold attribute).

- [ ] **Step 3: Add the render branch in `render_screen`**

In `app.rs`, inside `render_screen`, **after** the `pending_permission` branch closes (after line 308, before `let status = state.status.clone();`), add the Memory screen branch (priority 2):

```rust
    // (M7-14) Memory screen owns the frame at priority 2 (after permission).
    if state.active_screen == Some(crate::screens::Screen::Memory) {
        use crate::screens::memory::{memory_tiers, MemoryScreen};
        let cwd = state.status.cwd.clone();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let tiers = memory_tiers(&cwd, &home);
        let ms = &state.memory_screen;
        return element! {
            MemoryScreen(
                tiers: tiers,
                selected: ms.selected,
                editing: ms.editing,
                buffer: ms.buffer.clone(),
                dirty: ms.dirty,
                status: ms.status.clone(),
            )
        }
        .into_any();
    }
```

> NOTE: if `dirs` is not yet a tui dependency, add `dirs = "5"` (pin to the lockfile version) under `[dependencies]` in `crates/tui/Cargo.toml` — `crates/orchestrator` already uses `dirs::config_dir()`, so the workspace locks it.

- [ ] **Step 4: Write the failing snapshot test**

Create `crates/tui/tests/render_memory_screen.rs`:

```rust
//! M7-14 snapshot: the memory tier selector.

use iocraft::prelude::*;
use lingxi_tui::screens::memory::{MemoryScreen, MemoryTierEntry};

#[test]
fn snapshot_memory_selector() {
    let tiers = vec![
        MemoryTierEntry { label: "Project memory".into(),
            description: "Checked in at ./CLAUDE.md".into(),
            path: "/repo/CLAUDE.md".into(), exists: true },
        MemoryTierEntry { label: "User memory".into(),
            description: "Saved in ~/.claude/CLAUDE.md".into(),
            path: "/home/.claude/CLAUDE.md".into(), exists: false },
    ];
    let mut el = element! {
        MemoryScreen(tiers: tiers, selected: 0, editing: false,
            buffer: String::new(), dirty: false, status: None)
    };
    let out = el.to_string();
    insta::assert_snapshot!(out);
}
```

- [ ] **Step 5: Run to generate + accept the snapshot**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test render_memory_screen`
Then `cargo insta accept` (or review the `.snap.new` and rename). Expected: snapshot shows `Memory files`, `❯ Project memory`, `  User memory (new)`, and the footer hint.

> NOTE: confirm the exact render-to-string helper M6 snapshots use (`element.to_string()` vs a custom harness) by reading `crates/tui/tests/render_*.rs` — match it. If those tests use a fixed-width canvas, set the same width here so the snapshot is deterministic.

- [ ] **Step 6: Run the full crate to confirm wiring compiles**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo build -p lingxi-tui && cargo test -p lingxi-tui --lib screens::memory::tests`
Expected: builds; lib tests PASS.

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/crates/tui/src/screens/memory.rs \
        lingxi-code/crates/tui/src/state.rs \
        lingxi-code/crates/tui/src/app.rs \
        lingxi-code/crates/tui/Cargo.toml lingxi-code/Cargo.lock \
        lingxi-code/crates/tui/tests/render_memory_screen.rs \
        lingxi-code/crates/tui/tests/snapshots/
git commit -m "plan(M7-14 T6): MemoryScreen component + priority-2 render branch + snapshot

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: `search_messages` pure filter

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/message_selector.rs`

- [ ] **Step 1: Write the failing test**

Add to `message_selector.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RenderedMessage;

    fn user(body: &str) -> RenderedMessage {
        RenderedMessage::UserText { body: body.to_string(), timestamp: 0 }
    }
    fn asst(body: &str) -> RenderedMessage {
        RenderedMessage::AssistantText { body: body.to_string(), timestamp: 0 }
    }

    #[test]
    fn empty_query_returns_all_indices() {
        let msgs = vec![user("hello"), asst("world")];
        assert_eq!(search_messages(&msgs, ""), vec![0, 1]);
    }

    #[test]
    fn substring_filter_is_case_insensitive() {
        let msgs = vec![user("Hello World"), asst("goodbye"), user("WORLDLY")];
        assert_eq!(search_messages(&msgs, "world"), vec![0, 2]);
    }

    #[test]
    fn no_match_returns_empty() {
        let msgs = vec![user("a"), asst("b")];
        assert!(search_messages(&msgs, "zzz").is_empty());
    }

    #[test]
    fn searches_tool_use_and_result_text() {
        let id = lingxi_protocol::ToolUseId::new();
        let msgs = vec![
            RenderedMessage::AssistantToolUse { id, tool: "Bash".into(),
                input: serde_json::json!({"command": "ls -la"}) },
        ];
        // matches the tool name
        assert_eq!(search_messages(&msgs, "bash"), vec![0]);
        // matches inside the json input
        assert_eq!(search_messages(&msgs, "ls -la"), vec![0]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests::substring_filter_is_case_insensitive`
Expected: FAIL — `cannot find function search_messages`.

- [ ] **Step 3: Implement `search_messages` + a `searchable_text` helper**

Add to `message_selector.rs`:

```rust
use crate::state::RenderedMessage;

/// Project a message to the plain text the search query matches against.
fn searchable_text(msg: &RenderedMessage) -> String {
    match msg {
        RenderedMessage::UserText { body, .. }
        | RenderedMessage::AssistantText { body, .. }
        | RenderedMessage::SystemText { body, .. } => body.clone(),
        RenderedMessage::AssistantToolUse { tool, input, .. } => {
            format!("{tool} {input}")
        }
        RenderedMessage::UserToolResult { tool, result, .. } => {
            format!("{tool} {result}")
        }
    }
}

/// Filter `messages` by a case-insensitive substring `query`, returning the
/// matching message **indices** (into `messages`) in order. An empty query
/// matches everything (the selector shows the full list).
#[must_use]
pub fn search_messages(messages: &[RenderedMessage], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..messages.len()).collect();
    }
    let q = query.to_lowercase();
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| searchable_text(m).to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/message_selector.rs
git commit -m "plan(M7-14 T7): search_messages case-insensitive substring filter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: `message_line_offset` — jump-back via M7-03 line model

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/message_selector.rs`

- [ ] **Step 1: Write the failing jump-back tests**

Add to the `message_selector.rs` `tests` module:

```rust
use crate::components::virtual_message_list::HeightCache;

#[test]
fn jump_to_last_message_is_offset_zero() {
    // 4 one-line msgs (total 4 lines), viewport 10 → bottom anchor.
    let msgs = vec![user("m0"), user("m1"), user("m2"), user("m3")];
    let cache = HeightCache::build(&msgs, 80);
    // Jumping to the newest message keeps us at the bottom (offset 0).
    assert_eq!(message_line_offset(&msgs, &cache, 3, 10), 0);
}

#[test]
fn jump_to_first_pins_it_to_viewport_top() {
    // Heights: m0=1, m1=50, m2=1 → total 52. viewport 10.
    let msgs = vec![
        user("m0"),
        user(&vec!["x"; 50].join("\n")),
        user("m2"),
    ];
    let cache = HeightCache::build(&msgs, 80);
    assert_eq!(cache.total_lines(), 52);
    // line_at_start_of[0] = 0. offset = total(52) - 0 - viewport(10) = 42.
    assert_eq!(message_line_offset(&msgs, &cache, 0, 10), 42);
}

#[test]
fn jump_to_middle_tall_message_pins_its_top() {
    let msgs = vec![
        user("m0"),
        user(&vec!["x"; 50].join("\n")),
        user("m2"),
    ];
    let cache = HeightCache::build(&msgs, 80);
    // line_at_start_of[1] = 1. offset = 52 - 1 - 10 = 41.
    assert_eq!(message_line_offset(&msgs, &cache, 1, 10), 41);
}

#[test]
fn jump_offset_never_exceeds_max() {
    let msgs = vec![user("only")];
    let cache = HeightCache::build(&msgs, 80);
    // total 1, viewport 10 → max_offset 0; clamps to 0.
    assert_eq!(message_line_offset(&msgs, &cache, 0, 10), 0);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests::jump_to_first_pins_it_to_viewport_top`
Expected: FAIL — `cannot find function message_line_offset`.

- [ ] **Step 3: Implement `message_line_offset`**

Add to `message_selector.rs`:

```rust
use crate::components::virtual_message_list::HeightCache;

/// Compute the line-based `scroll_offset` (M7-03 model: lines from the
/// bottom) that pins message `target_index` to the **top** of the viewport.
///
/// `line_at_start = sum(height_at(0..target_index))`. Offset so the target's
/// first line is the viewport top:
/// `offset = total_lines - line_at_start - viewport_height`, clamped to
/// `[0, total_lines - viewport_height]`. Out-of-range `target_index`
/// clamps to the last message.
#[must_use]
pub fn message_line_offset(
    messages: &[RenderedMessage],
    cache: &HeightCache,
    target_index: usize,
    viewport_height: usize,
) -> usize {
    let total = cache.total_lines();
    let max_offset = total.saturating_sub(viewport_height);
    if messages.is_empty() {
        return 0;
    }
    let idx = target_index.min(messages.len() - 1);
    let line_at_start: usize = (0..idx).map(|i| cache.height_at(i)).sum();
    // Desired offset from the bottom that puts line_at_start at the top.
    let from_bottom = total.saturating_sub(line_at_start);
    let offset = from_bottom.saturating_sub(viewport_height);
    offset.min(max_offset)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/message_selector.rs
git commit -m "plan(M7-14 T8): message_line_offset jump-back using M7-03 HeightCache

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: `export_transcript` + default path + overwrite confirm

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/message_selector.rs`

- [ ] **Step 1: Write the failing export tests**

Add to the `message_selector.rs` `tests` module:

```rust
use std::fs;
use tempfile::TempDir;

#[test]
fn export_writes_plaintext_transcript_to_dir() {
    let tmp = TempDir::new().unwrap();
    let msgs = vec![user("hello"), asst("hi there")];
    let path = export_transcript(&msgs, tmp.path(), "session.txt", false).unwrap();
    assert_eq!(path, tmp.path().join("session.txt"));
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("hello"));
    assert!(body.contains("hi there"));
}

#[test]
fn export_forces_txt_extension() {
    let tmp = TempDir::new().unwrap();
    let msgs = vec![user("x")];
    // No extension → .txt appended.
    let p1 = export_transcript(&msgs, tmp.path(), "notes", false).unwrap();
    assert_eq!(p1.extension().unwrap(), "txt");
    // Wrong extension → replaced with .txt.
    let p2 = export_transcript(&msgs, tmp.path(), "notes.md", false).unwrap();
    assert_eq!(p2.file_name().unwrap(), "notes.txt");
}

#[test]
fn export_refuses_silent_overwrite() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("dup.txt");
    fs::write(&target, b"original").unwrap();
    let msgs = vec![user("new content")];
    // overwrite=false on an existing file → ExportError::Exists, original kept.
    match export_transcript(&msgs, tmp.path(), "dup.txt", false) {
        Err(ExportError::Exists(p)) => assert_eq!(p, target),
        other => panic!("expected Exists, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(&target).unwrap(), "original");
    // overwrite=true → clobbers (after the user confirmed).
    export_transcript(&msgs, tmp.path(), "dup.txt", true).unwrap();
    assert!(fs::read_to_string(&target).unwrap().contains("new content"));
}

#[test]
fn default_filename_ends_in_txt() {
    assert!(default_export_filename().ends_with(".txt"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests::export_refuses_silent_overwrite`
Expected: FAIL — `cannot find function export_transcript`.

- [ ] **Step 3: Implement export**

Add to `message_selector.rs` (add `use std::path::{Path, PathBuf};`):

```rust
/// Export failure modes.
#[derive(Debug)]
pub enum ExportError {
    /// Target file exists and `overwrite` was `false` (confirm required).
    Exists(PathBuf),
    /// Filesystem error (with the OS message).
    Io(String),
}

/// Default export directory: `~/.lingxi/exports/`, falling back to the
/// current working directory when the home dir is unavailable (§4 R10).
#[must_use]
pub fn default_export_dir() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(".lingxi").join("exports"),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Default export filename, `lingxi-transcript-<unix_secs>.txt`. The
/// timestamp keeps successive exports from colliding (so the overwrite
/// confirm is the exception, not the rule).
#[must_use]
pub fn default_export_filename() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("lingxi-transcript-{secs}.txt")
}

/// Render the scrollback to a plain-text transcript.
fn render_transcript(messages: &[RenderedMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        match m {
            RenderedMessage::UserText { body, .. } => {
                out.push_str("> ");
                out.push_str(body);
            }
            RenderedMessage::AssistantText { body, .. } => out.push_str(body),
            RenderedMessage::SystemText { body, .. } => out.push_str(body),
            RenderedMessage::AssistantToolUse { tool, input, .. } => {
                out.push_str(&format!("● {tool}({input})"));
            }
            RenderedMessage::UserToolResult { tool, result, .. } => {
                out.push_str(&format!("└ {tool}: {result}"));
            }
        }
        out.push('\n');
    }
    out
}

/// Export the transcript to `<dir>/<filename>`, forcing a `.txt` extension.
/// When the target exists and `overwrite` is `false`, returns
/// [`ExportError::Exists`] WITHOUT touching the file — the caller prompts
/// for confirmation and retries with `overwrite = true` (§4 R10: no silent
/// clobber). Creates `dir` if absent.
///
/// # Errors
/// [`ExportError::Exists`] on an unconfirmed overwrite; [`ExportError::Io`]
/// on any filesystem failure.
pub fn export_transcript(
    messages: &[RenderedMessage],
    dir: &Path,
    filename: &str,
    overwrite: bool,
) -> Result<PathBuf, ExportError> {
    // Force the .txt extension (claude-code ExportDialog parity).
    let stem = filename.rsplit_once('.').map_or(filename, |(s, _)| s);
    let final_name = format!("{stem}.txt");
    let target = dir.join(final_name);
    if target.exists() && !overwrite {
        return Err(ExportError::Exists(target));
    }
    std::fs::create_dir_all(dir).map_err(|e| ExportError::Io(e.to_string()))?;
    let body = render_transcript(messages);
    std::fs::write(&target, body.as_bytes()).map_err(|e| ExportError::Io(e.to_string()))?;
    Ok(target)
}
```

> NOTE: the success message the caller renders is literal-locked to claude-code: `format!("Conversation exported to: {}", target.display())`. The failure prefix is `"Failed to export conversation: "`. These strings are produced by the caller (Task 11 / the `/export` handler), not by `export_transcript` itself.

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests`
Expected: PASS (12 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/message_selector.rs
git commit -m "plan(M7-14 T9): export_transcript with default path + overwrite confirm

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 10: `MessageSelectorState` + `handle_message_selector_key` state machine

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/message_selector.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `message_selector.rs` `tests` module:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn typing_builds_query_and_filters_selection() {
    let msgs = vec![user("apple"), asst("banana"), user("apricot")];
    let mut st = MessageSelectorState::default();
    st.open();
    assert!(st.open);
    handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('a')));
    handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('p')));
    assert_eq!(st.query, "ap");
    // "ap" matches apple(0) + apricot(2).
    assert_eq!(st.filtered, vec![0, 2]);
    // Selection clamps within the filtered set.
    assert!(st.selected_filtered < st.filtered.len());
}

#[test]
fn up_down_move_within_filtered_results() {
    let msgs = vec![user("x1"), user("x2"), user("x3")];
    let mut st = MessageSelectorState::default();
    st.open();
    handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('x'))); // all match
    assert_eq!(st.filtered, vec![0, 1, 2]);
    handle_message_selector_key(&mut st, &msgs, k(KeyCode::Down));
    assert_eq!(st.selected_filtered, 1);
    handle_message_selector_key(&mut st, &msgs, k(KeyCode::Up));
    assert_eq!(st.selected_filtered, 0);
}

#[test]
fn enter_returns_jump_to_underlying_message_index() {
    let msgs = vec![user("alpha"), asst("beta"), user("alpaca")];
    let mut st = MessageSelectorState::default();
    st.open();
    handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('a'))); // matches 0,1?,2
    // "a" matches alpha(0), beta(1), alpaca(2) → pick the 3rd.
    st.selected_filtered = 2;
    let action = handle_message_selector_key(&mut st, &msgs, k(KeyCode::Enter));
    assert_eq!(action, SelectorAction::Jump { message_index: 2 });
    assert!(!st.open); // selecting closes the overlay
}

#[test]
fn esc_closes_without_jump() {
    let msgs = vec![user("a")];
    let mut st = MessageSelectorState::default();
    st.open();
    let action = handle_message_selector_key(&mut st, &msgs, k(KeyCode::Esc));
    assert_eq!(action, SelectorAction::Close);
    assert!(!st.open);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests::typing_builds_query_and_filters_selection`
Expected: FAIL — `cannot find type MessageSelectorState`.

- [ ] **Step 3: Implement the state + state machine**

Add to `message_selector.rs`:

```rust
/// Outcome the caller acts on after routing a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorAction {
    /// Nothing to do (state mutated in place).
    None,
    /// Close the overlay (Esc, no jump).
    Close,
    /// Jump the scrollback to `message_index` (caller sets `scroll_offset`
    /// via [`message_line_offset`]) and close the overlay.
    Jump {
        /// Index into `AppState.messages`.
        message_index: usize,
    },
}

/// Overlay state for the message search / jump selector. Lives on
/// `AppState`. `filtered` holds indices into `AppState.messages`;
/// `selected_filtered` indexes into `filtered`.
#[derive(Debug, Clone, Default)]
pub struct MessageSelectorState {
    /// `true` while the search overlay is shown (priority-3 focus).
    pub open: bool,
    /// Live search query.
    pub query: String,
    /// Matching message indices (into `AppState.messages`).
    pub filtered: Vec<usize>,
    /// Cursor into `filtered`.
    pub selected_filtered: usize,
}

impl MessageSelectorState {
    /// Open the overlay with an empty query (matches all).
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.filtered.clear();
        self.selected_filtered = 0;
    }

    /// Close the overlay and reset.
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.filtered.clear();
        self.selected_filtered = 0;
    }

    /// Re-run the filter against `messages` and clamp the selection.
    fn refilter(&mut self, messages: &[RenderedMessage]) {
        self.filtered = search_messages(messages, &self.query);
        if self.filtered.is_empty() {
            self.selected_filtered = 0;
        } else {
            self.selected_filtered = self.selected_filtered.min(self.filtered.len() - 1);
        }
    }
}

/// Route one key into the selector overlay. Returns a [`SelectorAction`].
pub fn handle_message_selector_key(
    st: &mut MessageSelectorState,
    messages: &[RenderedMessage],
    key: KeyEvent,
) -> SelectorAction {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            st.close();
            SelectorAction::Close
        }
        (KeyCode::Enter, _) => {
            if let Some(&idx) = st.filtered.get(st.selected_filtered) {
                st.close();
                SelectorAction::Jump { message_index: idx }
            } else {
                SelectorAction::None
            }
        }
        (KeyCode::Up, _) => {
            st.selected_filtered = st.selected_filtered.saturating_sub(1);
            SelectorAction::None
        }
        (KeyCode::Down, _) => {
            if !st.filtered.is_empty() {
                st.selected_filtered =
                    (st.selected_filtered + 1).min(st.filtered.len() - 1);
            }
            SelectorAction::None
        }
        (KeyCode::Backspace, _) => {
            st.query.pop();
            st.refilter(messages);
            SelectorAction::None
        }
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            st.query.push(c);
            st.refilter(messages);
            SelectorAction::None
        }
        _ => SelectorAction::None,
    }
}
```

> NOTE: `st.open()` starts with an empty query but does NOT pre-populate `filtered`. The first keystroke refilters. If a snapshot/behavior test needs the full list shown immediately on open, call `st.refilter(messages)` inside `open()` — add it only if Task 12's snapshot needs it (it does; see Task 12 Step 3).

- [ ] **Step 4: Run to verify pass**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --lib message_selector::tests`
Expected: PASS (16 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/message_selector.rs
git commit -m "plan(M7-14 T10): MessageSelectorState + handle_message_selector_key

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 11: Wire MessageSelector into `handle_live_key` (priority 3) + Memory routing (priority 2)

**Files:**
- Modify: `lingxi-code/crates/tui/src/state.rs`
- Modify: `lingxi-code/crates/tui/src/root.rs:187-211`

- [ ] **Step 1: Add the `message_selector` slot to `AppState`**

In `state.rs`, add to the `AppState` struct:

```rust
    /// (M7-14) Message search/jump overlay state.
    pub message_selector: crate::components::message_selector::MessageSelectorState,
```

In `AppState::new`:

```rust
            message_selector:
                crate::components::message_selector::MessageSelectorState::default(),
```

- [ ] **Step 2: Write the failing integration test for routing precedence**

Create `crates/tui/tests/behavior_message_selector.rs`:

```rust
//! M7-14 behavior: search overlay routing, jump-back, export safety.

use lingxi_tui::components::message_selector::{
    export_transcript, message_line_offset, search_messages, ExportError,
    MessageSelectorState, SelectorAction, handle_message_selector_key,
};
use lingxi_tui::components::virtual_message_list::HeightCache;
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::fake_status;

fn push(st: &mut AppState, body: &str) {
    st.push_message(RenderedMessage::UserText { body: body.into(), timestamp: 0 });
}

#[test]
fn selecting_a_match_sets_scroll_offset_to_that_message() {
    let mut st = AppState::new(fake_status());
    for i in 0..40 {
        push(&mut st, &format!("line {i}"));
    }
    st.refresh_height_cache(80); // M7-03: populate the line cache
    let vh = 10;

    st.message_selector.open();
    // Type "line 5" → matches "line 5".
    for c in "line 5".chars() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        handle_message_selector_key(
            &mut st.message_selector,
            &st.messages,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    // The first filtered hit is message index 5.
    let target = st.message_selector.filtered[st.message_selector.selected_filtered];
    assert_eq!(target, 5);

    // Jump: caller sets scroll_offset from message_line_offset.
    let cache = HeightCache::build(&st.messages, 80);
    let offset = message_line_offset(&st.messages, &cache, target, vh);
    st.scroll_offset = offset;
    // 40 one-line msgs: line_at_start[5]=5, total 40 → offset = 40-5-10 = 25.
    assert_eq!(st.scroll_offset, 25);
}
```

- [ ] **Step 3: Run to verify it compiles + the jump math is right**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test behavior_message_selector selecting_a_match_sets_scroll_offset_to_that_message`
Expected: PASS (this exercises the pure fns already built; it pins the integration contract).

- [ ] **Step 4: Extend `handle_live_key` — Memory (priority 2) + selector (priority 3)**

In `root.rs`, inside `handle_live_key`, **after** the permission focus-trap block (after line 194 `// === end focus trap ===`) and **before** `let prompt_empty = …`, add:

```rust
    // === PRIORITY 2: active screen owns all keys while open (M7-11/14). ===
    if let Some(screen) = st.active_screen {
        let ct_key = iocraft_to_crossterm028_key(k);
        match screen {
            crate::screens::Screen::Memory => {
                use crate::screens::memory::{handle_memory_key, memory_tiers, save_tier_body, MemoryAction};
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
                let tiers = memory_tiers(&st.status.cwd, &home);
                match handle_memory_key(&mut st.memory_screen, &tiers, ct_key) {
                    MemoryAction::CloseScreen => st.active_screen = None,
                    MemoryAction::Save { path, body } => match save_tier_body(&path, &body) {
                        Ok(()) => {
                            st.memory_screen.dirty = false;
                            st.memory_screen.status =
                                Some(format!("Saved {}", path.display()));
                        }
                        Err(e) => {
                            st.memory_screen.status =
                                Some(format!("Could not save memory: {e}"));
                        }
                    },
                    MemoryAction::BackToSelector | MemoryAction::None => {}
                }
            }
        }
        return;
    }
    // === end priority 2 ===

    // === PRIORITY 3: message-selector search overlay (M7-14). ===
    if st.message_selector.open {
        use crate::components::message_selector::{
            handle_message_selector_key, message_line_offset, SelectorAction,
        };
        let ct_key = iocraft_to_crossterm028_key(k);
        let messages = st.messages.clone();
        match handle_message_selector_key(&mut st.message_selector, &messages, ct_key) {
            SelectorAction::Jump { message_index } => {
                st.refresh_height_cache(st.viewport_width.max(1));
                let cache = st.height_cache.clone();
                st.scroll_offset = message_line_offset(&messages, &cache, message_index, viewport);
            }
            SelectorAction::Close | SelectorAction::None => {}
        }
        return;
    }
    // === end priority 3 ===
```

Then add the **open** binding for the selector in the default path. After the `focus_active` computation (line 199-203), before the `if let Some(action) = map_iocraft_key(...)` block, add:

```rust
    // Ctrl-T opens the message search/jump overlay (M7-14).
    if matches!(k.code, KeyCode::Char('t')) && k.modifiers.contains(KeyModifiers::CONTROL) {
        st.message_selector.open();
        st.message_selector.refilter_all(&st.messages);
        return;
    }
```

> NOTE: `refilter_all` is a thin public wrapper that runs the filter with the current (empty) query so the overlay shows the full list on open. Add it to `MessageSelectorState`:
> ```rust
> /// Populate `filtered` from the current query (used right after `open`).
> pub fn refilter_all(&mut self, messages: &[RenderedMessage]) {
>     self.refilter(messages);
> }
> ```
> and change `refilter`'s visibility to `pub(crate)` or call it through this wrapper. `viewport` is the `handle_live_key` parameter (scrollback viewport height). `st.viewport_width` comes from M7-03.

- [ ] **Step 5: Confirm `dirs` is a tui dep (added in Task 6 Step 3)**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -n '^dirs' crates/tui/Cargo.toml || echo MISSING`
If MISSING, add `dirs = "5"` (lockfile version) under `[dependencies]`.

- [ ] **Step 6: Run the full crate test + build**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo build -p lingxi-tui && cargo test -p lingxi-tui --lib && cargo test -p lingxi-tui --test behavior_message_selector`
Expected: builds; all PASS.

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/crates/tui/src/state.rs \
        lingxi-code/crates/tui/src/root.rs \
        lingxi-code/crates/tui/src/components/message_selector.rs \
        lingxi-code/crates/tui/Cargo.toml lingxi-code/Cargo.lock \
        lingxi-code/crates/tui/tests/behavior_message_selector.rs
git commit -m "plan(M7-14 T11): route Memory (prio-2) + MessageSelector (prio-3) through handle_live_key

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 12: `MessageSelector` component + render overlay branch + snapshot

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/message_selector.rs`
- Modify: `lingxi-code/crates/tui/src/app.rs:267-331`
- Create: `lingxi-code/crates/tui/tests/render_message_selector.rs`

- [ ] **Step 1: Implement the `MessageSelector` component**

Add to `message_selector.rs` (add `use iocraft::prelude::*;`):

```rust
/// Props for [`MessageSelector`]. Cloned from `AppState` each frame.
#[derive(Default, Props)]
pub struct MessageSelectorProps {
    /// Live query string.
    pub query: String,
    /// Result labels (one per filtered match), in `filtered` order. The
    /// caller projects each matched message to a one-line preview.
    pub result_labels: Vec<String>,
    /// Cursor into `result_labels`.
    pub selected: usize,
}

/// One-line preview of a message for the result list (≤ 60 cols).
#[must_use]
pub fn preview_label(msg: &RenderedMessage) -> String {
    let text = searchable_text(msg);
    let first = text.lines().next().unwrap_or("");
    let trimmed: String = first.chars().take(60).collect();
    trimmed
}

/// The search/jump overlay. Renders the query line and up to the visible
/// window of results with the selected one marked.
#[component]
pub fn MessageSelector(props: &MessageSelectorProps) -> impl Into<AnyElement<'static>> {
    let selected = props.selected;
    let rows: Vec<AnyElement<'static>> = props
        .result_labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let marker = if i == selected { "❯ " } else { "  " };
            element! { Text(content: format!("{marker}{label}")) }.into_any()
        })
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct) {
            Text(content: format!("Search: {}", props.query))
            #(rows)
            Text(content: "↑/↓ select · Enter jump · Esc cancel")
        }
    }
    .into_any()
}
```

- [ ] **Step 2: Add the render overlay branch in `render_screen`**

In `app.rs`, in `render_screen`, **after** the Memory screen branch (Task 6 Step 3) and **before** the REPL composition, add:

```rust
    // (M7-14) Message search overlay renders over the REPL at priority 3.
    if state.message_selector.open {
        use crate::components::message_selector::{preview_label, MessageSelector};
        let sel = &state.message_selector;
        let result_labels: Vec<String> = sel
            .filtered
            .iter()
            .filter_map(|&i| state.messages.get(i))
            .map(preview_label)
            .collect();
        return element! {
            MessageSelector(
                query: sel.query.clone(),
                result_labels: result_labels,
                selected: sel.selected_filtered,
            )
        }
        .into_any();
    }
```

- [ ] **Step 3: Ensure the overlay shows the full list on open**

Confirm `MessageSelectorState::open` followed by `refilter_all` (Task 11 Step 4) populates `filtered`. The snapshot below opens then refilters so results render.

- [ ] **Step 4: Write the failing snapshot test**

Create `crates/tui/tests/render_message_selector.rs`:

```rust
//! M7-14 snapshot: message search results.

use iocraft::prelude::*;
use lingxi_tui::components::message_selector::MessageSelector;

#[test]
fn snapshot_message_selector_results() {
    let mut el = element! {
        MessageSelector(
            query: "world".to_string(),
            result_labels: vec![
                "hello world".to_string(),
                "worldly affairs".to_string(),
            ],
            selected: 0,
        )
    };
    let out = el.to_string();
    insta::assert_snapshot!(out);
}
```

- [ ] **Step 5: Run + accept the snapshot**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test render_message_selector`
Then `cargo insta accept`. Expected: `Search: world`, `❯ hello world`, `  worldly affairs`, footer hint. Match the canvas-width convention used by existing `render_*.rs` tests.

- [ ] **Step 6: Build + full lib test**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo build -p lingxi-tui && cargo test -p lingxi-tui --lib`
Expected: builds; PASS.

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/crates/tui/src/components/message_selector.rs \
        lingxi-code/crates/tui/src/app.rs \
        lingxi-code/crates/tui/tests/render_message_selector.rs \
        lingxi-code/crates/tui/tests/snapshots/
git commit -m "plan(M7-14 T12): MessageSelector component + render overlay + snapshot

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 13: Wire `/memory` → open the Memory screen + real `/export` handler

**Files:**
- Modify: `lingxi-code/crates/tui/src/root.rs` (or `app.rs` command dispatch) — `/memory` opens `Screen::Memory`
- Create: `lingxi-code/crates/commands/src/builtin/export.rs`
- Modify: `lingxi-code/crates/commands/src/builtin/mod.rs`

- [ ] **Step 1: Find how slash-command output reaches the TUI**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -rn 'fn dispatch\|CommandResult\|slash\|"/"\|run_command\|handle_slash' crates/tui/src/app.rs crates/tui/src/streaming.rs | head -20`
Determine the seam where a submitted `/memory` line is routed. M5-13/M6 run slash commands through the orchestrator; the TUI receives a `CommandResult::Done { display }`. The Memory screen is a TUI-local view — so `/memory` in the **TUI** must intercept before/around the orchestrator call and set `active_screen = Some(Screen::Memory)` instead of (or in addition to) the `$EDITOR` shell-out path.

- [ ] **Step 2: Write the failing test for `/memory` opening the screen**

Add to `crates/tui/tests/behavior_memory_screen.rs` (create it):

```rust
//! M7-14 behavior: memory screen open + tier round-trip.

use lingxi_tui::screens::memory::{
    handle_memory_key, load_tier_body, memory_tiers, save_tier_body, MemoryAction,
    MemoryScreenState, MemoryTierEntry,
};
use lingxi_tui::screens::Screen;
use lingxi_tui::state::AppState;

mod support;
use support::fake_status;

#[test]
fn memory_command_sets_active_screen() {
    let mut st = AppState::new(fake_status());
    // Simulate the /memory intercept (Task 13 Step 4 sets this).
    lingxi_tui::app::open_memory_screen(&mut st);
    assert_eq!(st.active_screen, Some(Screen::Memory));
}

#[test]
fn select_edit_save_round_trip_through_store() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("CLAUDE.md");
    let tiers = vec![MemoryTierEntry {
        label: "Project memory".into(), description: String::new(),
        path: path.clone(), exists: false,
    }];
    let mut ms = MemoryScreenState::default();
    // Enter → open editor (empty body, new file).
    handle_memory_key(&mut ms, &tiers, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(ms.editing);
    // Type "hi".
    for c in "hi".chars() {
        handle_memory_key(&mut ms, &tiers,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // Ctrl-S → Save action; persist via the store fn.
    let action = handle_memory_key(&mut ms, &tiers,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    match action {
        MemoryAction::Save { path: p, body } => save_tier_body(&p, &body).unwrap(),
        other => panic!("expected Save, got {other:?}"),
    }
    // The store now holds the edited body.
    assert_eq!(load_tier_body(&path).unwrap(), "hi");
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test -p lingxi-tui --test behavior_memory_screen memory_command_sets_active_screen`
Expected: FAIL — `cannot find function open_memory_screen`.

- [ ] **Step 4: Add `open_memory_screen` + intercept `/memory` in the TUI**

In `app.rs`, add a small public helper:

```rust
/// (M7-14) Open the Memory editor screen (priority-2 overlay). The TUI
/// intercepts `/memory` to open this in-process view instead of shelling
/// out to `$EDITOR` (the `--no-tui` REPL keeps the `$EDITOR` path).
pub fn open_memory_screen(st: &mut AppState) {
    st.memory_screen = crate::screens::memory::MemoryScreenState::default();
    st.active_screen = Some(crate::screens::Screen::Memory);
}
```

At the slash-command seam found in Step 1 (where a submitted line beginning `/memory` is detected), call `open_memory_screen(st)` and skip the orchestrator dispatch for that command. Match the exact dispatch shape in `app.rs` — if commands run through `OrchestratorHandle`, add the intercept in the TUI's submit handler before the handle call:

```rust
    if trimmed == "/memory" {
        crate::app::open_memory_screen(st);
        return; // handled in-TUI; do not dispatch to the orchestrator
    }
```

> NOTE: keep the `/memory` interception minimal and TUI-local. Do NOT modify the `MemoryHandler` in `crates/commands` (it remains the `--no-tui` / non-iocraft path that shells out to `$EDITOR`). This is the same split M7-12 uses for `--resume` (iocraft view over the same loader; stdio picker stays as the fallback).

- [ ] **Step 5: Add the real `/export` handler**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && grep -rn 'export' crates/commands/src/builtin/mod.rs crates/commands/src/registry.rs | head`
`/export` is currently the M5-11 `UnimplementedCommandHandler` stub. The export *write* lives in the TUI (`export_transcript`), but the slash-command surface must light up `/export`. Wire `/export` in the TUI submit handler the same way as `/memory`: open the selector's export flow. Add to the TUI submit intercept:

```rust
    if trimmed == "/export" {
        // Open the message selector; the export action is reachable from it
        // (M7-14 scopes /export to opening the search/export overlay).
        st.message_selector.open();
        st.message_selector.refilter_all(&st.messages);
        return;
    }
```

> NOTE: M7-14 keeps `/export` as "open the overlay where export lives" to avoid a second modal. A dedicated ExportDialog screen (claude-code's separate dialog) is optional polish; if a parity fixture in M7-16 requires the standalone dialog, add it then. The behavior test below exercises `export_transcript` directly (the load-bearing safety surface).

- [ ] **Step 6: Add the export-safety behavior test**

Add to `crates/tui/tests/behavior_message_selector.rs`:

```rust
#[test]
fn export_default_path_and_overwrite_confirm() {
    use tempfile::TempDir;
    use std::fs;
    let tmp = TempDir::new().unwrap();
    let msgs = vec![
        RenderedMessage::UserText { body: "q".into(), timestamp: 0 },
        RenderedMessage::AssistantText { body: "a".into(), timestamp: 0 },
    ];
    // First export writes the file.
    let p = export_transcript(&msgs, tmp.path(), "out.txt", false).unwrap();
    assert!(p.exists());
    // Second export to the same name without overwrite → refused, file kept.
    let original = fs::read_to_string(&p).unwrap();
    match export_transcript(&msgs, tmp.path(), "out.txt", false) {
        Err(ExportError::Exists(_)) => {}
        other => panic!("expected Exists, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(&p).unwrap(), original);
}
```

- [ ] **Step 7: Run all M7-14 tests + build**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo build -p lingxi-tui && cargo test -p lingxi-tui --test behavior_memory_screen --test behavior_message_selector && cargo test -p lingxi-tui --lib`
Expected: all PASS.

- [ ] **Step 8: Commit**

```bash
git add lingxi-code/crates/tui/src/app.rs \
        lingxi-code/crates/tui/tests/behavior_memory_screen.rs \
        lingxi-code/crates/tui/tests/behavior_message_selector.rs \
        lingxi-code/crates/commands/src/builtin/
git commit -m "plan(M7-14 T13): /memory opens Memory screen; /export opens search/export overlay

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 14: Workspace verification gate + tag `m7.14`

**Files:** none (verification only)

- [ ] **Step 1: Format check**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo fmt --check`
Expected: clean. If not, run `cargo fmt` and re-stage.

- [ ] **Step 2: Clippy (workspace, all targets, deny warnings)**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. Fix any lints in the M7-14 files (the gate runs from inside `lingxi-code/` so the pinned 1.82 toolchain is used — running from repo root gives spurious lint noise; this bit M6-08).

- [ ] **Step 3: Full workspace test**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && cargo test --workspace`
Expected: PASS. Known flakes are allowed a rerun: `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, and `lingxi-platform-posix` fs_watch FSEvents timing tests. Re-run any of those once if they flake; a second failure is a real regression.

- [ ] **Step 4: Cross-platform compile gate (5 targets)**

Run:
```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && \
cargo check --workspace --target x86_64-unknown-linux-gnu && \
cargo check --workspace --target x86_64-apple-darwin && \
cargo check --workspace --target x86_64-pc-windows-gnu && \
cargo check --workspace --target aarch64-linux-android && \
cargo check --workspace --target aarch64-apple-ios
```
Expected: all green. (`std::fs` + `dirs` are cross-platform; no platform-specific code added.)

- [ ] **Step 5: Confirm no new telemetry events leaked in**

Run: `cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core && git diff m7.13..HEAD -- crates/ | grep -nE 'event = |ALL_EVENT_NAMES|tengu_tui_' || echo "NO NEW EVENTS — good"`
Expected: `NO NEW EVENTS — good` (the count stays at the 326 baseline; search/export/screen events are M7-16). If any appear, remove them — M7-14 registers zero events.

> NOTE: `m7.13` is the previous sub-plan's tag. If M7-13 has not been tagged in this execution context, diff against the M7-14 starting commit instead.

- [ ] **Step 6: Annotated tag**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next && \
git tag -a m7.14 -m "M7-14: Memory editor + MessageSelector (search/jump/export)"
```

> Tag is **local only** — no remote push (design §6.4). No force-push / skip-hooks / amends.

- [ ] **Step 7: Final commit (if any fmt/clippy fixes were staged)**

```bash
git add -A
git commit -m "plan(M7-14 T14): workspace gate green + tag m7.14

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

(If Steps 1-5 produced no changes, skip the commit — the tag points at Task 13's commit.)

---

## Self-Review

**Spec coverage (design §3 M7-14 + §4 R7/R10 + §2.3/§2.5):**
- "MemoryFileSelector (pick tier) + edit view, reads+writes through M3 store ONLY (R7)" → Tasks 3 (tiers via `hierarchy::walk`), 4 (load via M3 `loader::load_file` + atomic write to the same path), 5/6 (selector + editor). ✓
- "Add Screen::Memory variant (reuse M7-11 active_screen + priority-2 routing)" → Tasks 2 (variant/field, defensive if M7-11 absent), 6 (priority-2 render branch), 11 (priority-2 key branch). ✓
- "MessageSelector: search current scrollback, jump-back, export to sane default path (R10), no arbitrary overwrite without confirm" → Tasks 7 (search), 8 (jump via M7-03 offset), 9 (export default path + overwrite confirm), 10 (state machine). ✓
- "Wire /export + a search keybind" → Task 11 (Ctrl-T opens selector), 13 (`/export` intercept + `/memory` opens screen). ✓
- "single handle_live_key dispatcher, no parallel path (§2.5)" → Task 11 extends the one dispatcher at priorities 2 and 3. ✓
- Tests required (behavior: tier list/edit/save through temp store; search filter + jump offset; export writes + confirm-on-overwrite; snapshots: memory selector + search results) → Tasks 3-13 cover all four behavior families + both snapshots. ✓
- Telemetry baseline 326, 0 new events → Task 14 Step 5 gate. ✓

**Placeholder scan:** no TBD/TODO; every code step shows the full code; commands have expected output. The only deferred items are explicitly scoped out (git-repo description ternary → M7-16 literal catalog; standalone ExportDialog screen → optional M7-16 polish; telemetry events → M7-16) with a documented reason, matching the spec's deferrals.

**Type consistency:** `MemoryTierEntry { label, description, path, exists }`, `MemoryScreenState`, `MemoryAction::{None,CloseScreen,BackToSelector,Save}`, `handle_memory_key`, `load_tier_body`/`save_tier_body`, `memory_tiers`, `open_memory_screen` are used consistently across Tasks 3-6, 11, 13. `MessageSelectorState { open, query, filtered, selected_filtered }`, `SelectorAction::{None,Close,Jump}`, `search_messages`, `message_line_offset`, `export_transcript`, `ExportError::{Exists,Io}`, `default_export_dir`/`default_export_filename`, `preview_label`, `refilter_all` are used consistently across Tasks 7-13. `Screen::Memory`, `active_screen` consistent Tasks 2/6/11/13. `HeightCache`/`refresh_height_cache`/`scroll_offset`/`viewport_width` reused exactly as M7-03 defined them.

---

**End of plan.**
