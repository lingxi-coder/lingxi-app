# M7-07 — Command Palette + `@` File-Ref Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/` slash-command autocomplete palette and an `@` file-ref completion dropdown to PromptInput, both rendered as overlays and both holding key focus (focus-trap) while open.

**Architecture:** Two new files in the `prompt_input/` submodule (created by M7-06): `palette.rs` (filters the 99 builtin command names) and `completion.rs` (filters cwd path entries). Each is a pure-logic core (`*State` struct + filter/select/accept functions, fully unit-testable) plus a small iocraft overlay component. New `AppState` fields hold each overlay's open-state, filter text, and selected index. The single `handle_live_key` dispatcher in `root.rs` gains a **priority-3 branch** (per design §2.5: below permission dialogs at priority 1 and screens at priority 2, above default input editing) so an open overlay owns every key until `Esc`/accept closes it.

**Tech Stack:** Rust 1.82 (pinned via `lingxi-code/rust-toolchain`), iocraft `=0.8.3` (`View`, not `Box`), crossterm (workspace 0.28 / iocraft 0.29 skew bridged in `root.rs`), `lingxi-commands` (`BUILTIN_COMMAND_NAMES` + `core_description`), `insta` snapshots. **No new dependency** — fuzzy filtering uses a hand-written subsequence matcher (see Decision D1).

---

## Context the implementer needs before starting

**Prerequisite:** M7-06 must have landed. It refactors the single file `crates/tui/src/components/prompt_input.rs` into a submodule `crates/tui/src/components/prompt_input/` with at least `mod.rs` (the editor core: `apply_insert`, `apply_backspace`, `apply_move`, `CursorMove`, `PromptInput` component) and `footer.rs`. **If `prompt_input/` is still a single `.rs` file, STOP and confirm M7-06 landed** — every path below assumes the submodule exists. All paths in this plan are relative to `lingxi-code/` unless absolute.

**The single live-key dispatcher** is `crate::root::handle_live_key(st: &mut AppState, k: &iocraft::KeyEvent, viewport: usize)` in `crates/tui/src/root.rs`. As of M6-05 it has exactly one priority branch:

```rust
pub fn handle_live_key(st: &mut AppState, k: &KeyEvent, viewport: usize) {
    // priority 1: permission focus-trap
    if st.pending_permission.is_some() {
        let ct_key = iocraft_to_crossterm028_key(k);
        let _ = crate::events::keymap::handle_key(st, ct_key);
        return;
    }
    // ... default: map_iocraft_key → dispatch / scroll_with_viewport
}
```

M7-11..14 will insert a priority-2 `active_screen` branch between permission and the default path. **M7-07 owns priority 3** — its branch goes *after* the permission branch (and after the priority-2 screen branch if M7-11+ already landed; if not, immediately after permission) and *before* the default `map_iocraft_key` path. Use the exact pattern from Task 9 below. Do **not** add a second key path — re-introducing a parallel path is the exact M6 ship-blocker bug (design §2.5).

**Why the overlay handles iocraft (crossterm-0.29) `KeyCode` directly:** `handle_live_key` receives `iocraft::KeyEvent`. The permission branch converts to crossterm-0.28 via `iocraft_to_crossterm028_key` because the dialog state machines predate iocraft. The M7-07 overlay handlers are **new** code, so they consume `iocraft::KeyCode` directly (no conversion shim) — simpler, and matches how `map_iocraft_key` already inspects `iocraft::KeyCode`. The overlay key handler signature is `fn handle_key(state: &mut PaletteState, code: iocraft::KeyCode) -> PaletteKeyOutcome`.

**Command data source:** `lingxi_commands::builtin::{BUILTIN_COMMAND_NAMES, core_description}` (re-exported; the `lingxi-commands` dep already exists in `crates/tui/Cargo.toml`). `BUILTIN_COMMAND_NAMES: &[&str; 99]` is ASCII-sorted, no leading `/`. `core_description(name) -> &'static str` returns a real description for the 18 core commands and `"(unimplemented in v0.6.0)"` otherwise. The palette shows `name` + description.

**Literal lock (design §2.8) — claude-code references** (`claude-code/src/components/`):
- `PromptInput/PromptInputFooterSuggestions.tsx`: file suggestion icon is `'+'`; a row with a description renders as `` `${icon} ${displayText} – ${truncatedDesc}` `` — note the **en-dash with surrounding spaces** `" – "` (U+2013), not a hyphen. `OVERLAY_MAX_ITEMS = 5`. Selected rows use theme color `"suggestion"`; non-selected are dim.
- `QuickOpenDialog.tsx`: `@`-mention insert is `` `@${p} ` `` (the path **plus a trailing space**); plain path insert is `` `${p} ` ``. Empty-state strings: `"No matching files"` (when a query is present) and `"Start typing to search…"` (U+2026 ellipsis, empty query). Title `"Quick Open"`, placeholder `"Type to search files…"`.

These literals are the byte-for-byte targets. Where M7-07's surface diverges structurally from claude-code (we render a compact inline dropdown, not the full FuzzyPicker pane), copy the **strings and the row format**, not the React layout. Snapshot tests lock them.

---

## Decisions (locked before tasks)

- **D1 — Fuzzy filter: hand-written subsequence matcher, NO new dependency.** No fuzzy-match crate (`fuzzy-matcher`, `nucleo`, `skim`, …) is in `Cargo.lock` today. Adding one for v0.8.0 violates YAGNI and the §2.2 exact-pin discipline. M7-07 ships a small `subsequence_match(needle, haystack) -> Option<score>` in `prompt_input/fuzzy.rs`: case-insensitive subsequence test; score = negative gap-count (contiguous matches rank first), tie-broken by shorter haystack then ASCII order for stable output. This is enough fidelity for v0.8.0 (palette over 99 short names; completion over a cwd listing). A real fuzzy crate can be swapped in behind the same `subsequence_match` signature in M8 if ranking quality matters — note this in the module doc-comment.
- **D2 — Telemetry: 0 new events in M7-07.** Design §2.7 lists `tengu_tui_command_palette_opened` as a *candidate*. Per the milestone decision, **defer the palette telemetry event to M7-16's count audit** to avoid count churn mid-milestone (the M6 "330→326" lesson). M7-07 registers and emits **zero** new event names; `ALL_EVENT_NAMES.len()` stays at **326**. A `// M7-16: candidate tengu_tui_command_palette_opened` comment marks the future emit site in `palette.rs`.
- **D3 — Completion path source is synchronous `std::fs::read_dir` of the cwd.** claude-code's `generateFileSuggestions` is async + indexed; M7-07 stays surface-only and reads the directory entries synchronously when the `@` overlay opens (and re-reads on directory-prefix change). No async, no engine wiring, no file watcher. The candidate list is computed once per directory and filtered in-memory as the user types. Hidden entries (dotfiles) are excluded to match the common-case listing. This keeps the overlay a pure-ish component testable by injecting a `Vec<String>` candidate list (see Task 6 — the filter/select logic takes the candidate vec as input, so tests never touch the filesystem).
- **D4 — Overlay render is "instead of", consistent with M6-05.** iocraft 0.8 has no portable z-index overlay primitive (documented in `app.rs::render_screen`). The palette/completion dropdown renders as extra rows *above* the PromptInput within the REPL screen's bottom zone (not a true floating layer). "Overlay" in this plan means "owns keys + draws a dropdown list above the prompt," not GPU compositing.

---

## File Structure

**Create:**
- `crates/tui/src/components/prompt_input/fuzzy.rs` — `subsequence_match` + shared filtering helper. One responsibility: ranking. Shared by palette + completion so the matcher isn't duplicated (DRY).
- `crates/tui/src/components/prompt_input/palette.rs` — `PaletteState`, palette open/filter/select/accept logic, `PaletteOverlay` iocraft component, `PaletteKeyOutcome`.
- `crates/tui/src/components/prompt_input/completion.rs` — `CompletionState`, completion open/filter/select/accept logic, `CompletionOverlay` iocraft component, `CompletionKeyOutcome`, plus `read_cwd_entries()` (the only fs-touching fn).
- `crates/tui/tests/behavior_palette.rs` — palette behavior tests driving `handle_live_key`.
- `crates/tui/tests/behavior_completion.rs` — completion behavior tests driving `handle_live_key`.
- `crates/tui/tests/snapshot_palette_completion.rs` — insta snapshots of both dropdowns.

**Modify:**
- `crates/tui/src/components/prompt_input/mod.rs` — `pub mod fuzzy; pub mod palette; pub mod completion;` + re-exports.
- `crates/tui/src/state.rs` — add `palette: PaletteState` and `completion: CompletionState` fields to `AppState` (+ init in `new`).
- `crates/tui/src/root.rs` — add the priority-3 branch in `handle_live_key`.
- `crates/tui/src/app.rs` — `render_screen` passes palette/completion state into the REPL screen so the dropdown draws above PromptInput.
- `crates/tui/src/screens/repl.rs` — `ReplScreen` renders the active overlay above the prompt row.

**Responsibility boundary:** `fuzzy.rs` knows nothing about commands or paths (pure string ranking). `palette.rs` knows commands, not paths. `completion.rs` knows paths, not commands. `root.rs` only *routes* keys to the right `*State::handle_key`; it never filters. This keeps each file small and independently reasoned about.

---

### Task 1: Fuzzy subsequence matcher (`fuzzy.rs`)

**Files:**
- Create: `crates/tui/src/components/prompt_input/fuzzy.rs`
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/tui/src/components/prompt_input/fuzzy.rs` with only the test module to start (the impl comes in Step 3):

```rust
//! Hand-written case-insensitive subsequence matcher + ranked filter.
//!
//! DECISION D1 (M7-07): no fuzzy-match crate is a workspace dependency, and
//! adding one for v0.8.0 would violate the exact-pin discipline (design §2.2)
//! and YAGNI. This module is a small subsequence matcher: every `needle` char
//! must appear in `haystack` in order (case-insensitive). The score rewards
//! contiguous matches (fewer gaps = higher score). Good enough for the 99
//! short command names and a cwd listing. A real fuzzy crate can replace the
//! body behind `subsequence_match`'s signature in M8 if ranking quality ever
//! matters.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_needle_matches_everything_with_zero_score() {
        assert_eq!(subsequence_match("", "anything"), Some(0));
    }

    #[test]
    fn exact_prefix_outranks_scattered() {
        let contig = subsequence_match("co", "compact").expect("contiguous matches");
        let scattered = subsequence_match("co", "context").expect("also matches");
        // Both match; "compact" has the run "co" adjacent at the very start,
        // "context" also starts "co" — tie on gaps, so ASCII order in the
        // caller decides. Here assert both are Some and contiguous beats gappy:
        let gappy = subsequence_match("ct", "context").expect("c..t matches");
        assert!(contig >= gappy, "contiguous run must score >= gappy run");
        let _ = scattered;
    }

    #[test]
    fn non_subsequence_returns_none() {
        assert_eq!(subsequence_match("xyz", "compact"), None);
    }

    #[test]
    fn case_insensitive() {
        assert!(subsequence_match("CMP", "compact").is_some());
    }

    #[test]
    fn filtered_ranked_orders_contiguous_first() {
        let cands = vec!["context".to_string(), "compact".to_string(), "copy".to_string()];
        let out = filtered_ranked("co", &cands);
        // All three contain "co"; "copy" and "compact" and "context" all start
        // with "co" (gap 0). Tie broken by shorter then ASCII → copy, compact,
        // context. Just assert ordering is deterministic and copy is first.
        assert_eq!(out, vec!["copy", "compact", "context"]);
    }

    #[test]
    fn filtered_ranked_drops_non_matches() {
        let cands = vec!["help".to_string(), "exit".to_string()];
        assert_eq!(filtered_ranked("zz", &cands), Vec::<&str>::new());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui fuzzy:: 2>&1 | head -30` (from inside `lingxi-code/`)
Expected: FAIL — `cannot find function subsequence_match` / `filtered_ranked`.

- [ ] **Step 3: Write minimal implementation**

Prepend to `fuzzy.rs` (above the test module):

```rust
/// Case-insensitive subsequence test. Returns `Some(score)` if every char of
/// `needle` appears in `haystack` in order, else `None`. Higher score is a
/// better match; score is `-(gap_count)` so a contiguous run scores `0` and
/// each skipped haystack char between matches subtracts one. An empty needle
/// matches everything with score `0`.
#[must_use]
pub fn subsequence_match(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().flat_map(char::to_lowercase).collect();
    let mut gaps: i32 = 0;
    let mut hi = 0usize;
    let mut last_matched: Option<usize> = None;
    for nc in needle.chars().flat_map(char::to_lowercase) {
        let mut found = false;
        while hi < hay.len() {
            if hay[hi] == nc {
                if let Some(prev) = last_matched {
                    gaps += i32::try_from(hi - prev - 1).unwrap_or(i32::MAX);
                }
                last_matched = Some(hi);
                hi += 1;
                found = true;
                break;
            }
            hi += 1;
        }
        if !found {
            return None;
        }
    }
    Some(-gaps)
}

/// Filter `candidates` to those matching `needle`, ranked best-first.
/// Sort key: score descending, then shorter candidate, then ASCII ascending —
/// deterministic so snapshots and behavior tests are stable.
#[must_use]
pub fn filtered_ranked<'a>(needle: &str, candidates: &'a [String]) -> Vec<&'a str> {
    let mut scored: Vec<(i32, &'a str)> = candidates
        .iter()
        .filter_map(|c| subsequence_match(needle, c).map(|s| (s, c.as_str())))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(b.1))
    });
    scored.into_iter().map(|(_, c)| c).collect()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui fuzzy:: 2>&1 | tail -20`
Expected: PASS (6 tests). If `filtered_ranked_orders_contiguous_first` fails on ordering, the tie-break is wrong — verify the sort is score desc → len asc → ASCII asc.

- [ ] **Step 5: Wire the module**

Add to `crates/tui/src/components/prompt_input/mod.rs`:

```rust
pub mod fuzzy;
```

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/components/prompt_input/fuzzy.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "plan(M7-07 T1): subsequence fuzzy matcher (no new dep)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `PaletteState` open + filter logic

**Files:**
- Create: `crates/tui/src/components/prompt_input/palette.rs`
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/tui/src/components/prompt_input/palette.rs`:

```rust
//! `/` slash-command palette: a live-filtered dropdown over the 99 builtin
//! command names. Opens when the prompt buffer starts with `/`. Pure logic
//! (`PaletteState` + open/filter/select/accept) is unit-tested without iocraft;
//! `PaletteOverlay` (Task 5) renders it.
//!
//! Literal lock (design §2.8): rows show `name` + ` – ` (en-dash, U+2013) +
//! description, mirroring claude-code PromptInputFooterSuggestions.tsx.

use lingxi_commands::builtin::{core_description, BUILTIN_COMMAND_NAMES};

use super::fuzzy::filtered_ranked;

/// Max dropdown rows shown at once (claude-code `OVERLAY_MAX_ITEMS`).
pub const OVERLAY_MAX_ITEMS: usize = 5;

/// One filtered palette row: command name + description (both `'static`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow {
    /// Command name without the leading `/`.
    pub name: &'static str,
    /// Description text (real for the 18 core commands, stub otherwise).
    pub description: &'static str,
}

/// Palette overlay state. `open == false` means the overlay is dismissed and
/// owns no keys.
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    /// Whether the dropdown is currently shown.
    pub open: bool,
    /// The filter text (the prompt slice after the leading `/`).
    pub filter: String,
    /// Index into the *filtered* rows, clamped to `[0, len)`.
    pub selected: usize,
}

impl PaletteState {
    /// Recompute open-state + filter from the current prompt buffer.
    /// Opens iff `prompt` starts with `/` and contains no space (a space
    /// means the user has moved past the command name into arguments).
    pub fn sync_from_prompt(&mut self, prompt: &str) {
        let is_command_token = prompt.starts_with('/') && !prompt[1..].contains(' ');
        if is_command_token {
            let new_filter = prompt[1..].to_string();
            if !self.open || new_filter != self.filter {
                self.selected = 0;
            }
            self.open = true;
            self.filter = new_filter;
            let max = self.rows().len();
            if max == 0 {
                self.selected = 0;
            } else if self.selected >= max {
                self.selected = max - 1;
            }
        } else {
            self.open = false;
            self.filter.clear();
            self.selected = 0;
        }
    }

    /// The filtered, ranked rows for the current filter.
    #[must_use]
    pub fn rows(&self) -> Vec<PaletteRow> {
        let names: Vec<String> = BUILTIN_COMMAND_NAMES.iter().map(|s| (*s).to_string()).collect();
        filtered_ranked(&self.filter, &names)
            .into_iter()
            .filter_map(|matched| {
                BUILTIN_COMMAND_NAMES
                    .iter()
                    .copied()
                    .find(|n| *n == matched)
                    .map(|name| PaletteRow {
                        name,
                        description: core_description(name),
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_when_prompt_starts_with_slash() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/co");
        assert!(p.open);
        assert_eq!(p.filter, "co");
    }

    #[test]
    fn stays_closed_without_slash() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("hello");
        assert!(!p.open);
    }

    #[test]
    fn closes_once_a_space_follows_the_command() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/compact");
        assert!(p.open);
        p.sync_from_prompt("/compact now");
        assert!(!p.open, "a space moves past the command name → close");
    }

    #[test]
    fn filter_narrows_rows() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/");
        let all = p.rows().len();
        assert_eq!(all, 99, "bare slash lists every command");
        p.sync_from_prompt("/comp");
        let narrowed = p.rows();
        assert!(narrowed.len() < all);
        assert!(narrowed.iter().any(|r| r.name == "compact"));
    }

    #[test]
    fn core_rows_carry_real_descriptions() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/help");
        let row = p.rows().into_iter().find(|r| r.name == "help").expect("help present");
        assert_eq!(row.description, "Show help and available commands");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui palette::tests 2>&1 | head -30`
Expected: FAIL — `module palette is not declared` (until Step 4) or compile errors referencing `PaletteState`.

- [ ] **Step 3: Implementation already written in Step 1.**

(The impl lives above the test module in the file created in Step 1 — no additional code.)

- [ ] **Step 4: Declare the module**

Add to `crates/tui/src/components/prompt_input/mod.rs`:

```rust
pub mod palette;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p lingxi-tui palette::tests 2>&1 | tail -20`
Expected: PASS (5 tests). The `lists every command` assertion locks the 99 count.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/components/prompt_input/palette.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "plan(M7-07 T2): PaletteState open + filter over 99 commands

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Palette key handling — select / accept / dismiss

**Files:**
- Modify: `crates/tui/src/components/prompt_input/palette.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `palette.rs`:

```rust
    use iocraft::prelude::KeyCode;

    #[test]
    fn down_up_move_selection_and_clamp() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/co"); // several rows
        assert_eq!(p.selected, 0);
        assert!(matches!(handle_key(&mut p, KeyCode::Down), PaletteKeyOutcome::Consumed));
        assert_eq!(p.selected, 1);
        assert!(matches!(handle_key(&mut p, KeyCode::Up), PaletteKeyOutcome::Consumed));
        assert_eq!(p.selected, 0);
        // Up at the top stays at 0 (no wrap).
        handle_key(&mut p, KeyCode::Up);
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn tab_accepts_selected_into_a_complete_command() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/comp");
        let sel = p.rows()[p.selected].name; // best match, e.g. "compact"
        let outcome = handle_key(&mut p, KeyCode::Tab);
        match outcome {
            PaletteKeyOutcome::Accept(text) => assert_eq!(text, format!("/{sel} ")),
            other => panic!("expected Accept, got {other:?}"),
        }
        assert!(!p.open, "accepting closes the overlay");
    }

    #[test]
    fn enter_also_accepts() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/help");
        assert!(matches!(handle_key(&mut p, KeyCode::Enter), PaletteKeyOutcome::Accept(_)));
    }

    #[test]
    fn esc_dismisses_and_releases_keys() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/co");
        assert!(matches!(handle_key(&mut p, KeyCode::Esc), PaletteKeyOutcome::Dismiss));
        assert!(!p.open);
    }

    #[test]
    fn accept_with_no_rows_is_passthrough() {
        let mut p = PaletteState::default();
        p.sync_from_prompt("/zzzznomatch");
        assert!(p.rows().is_empty());
        // Nothing to accept → Enter/Tab fall through to default input.
        assert!(matches!(handle_key(&mut p, KeyCode::Enter), PaletteKeyOutcome::PassThrough));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui palette::tests 2>&1 | head -30`
Expected: FAIL — `cannot find function handle_key` / `PaletteKeyOutcome`.

- [ ] **Step 3: Write minimal implementation**

Add to `palette.rs` (above the test module, after the `impl PaletteState` block):

```rust
use iocraft::prelude::KeyCode;

/// What the palette key handler decided. The dispatcher in `root.rs` acts on
/// this — `Accept` replaces the prompt buffer, `Dismiss`/`PassThrough` let the
/// key (or future keys) reach the default input path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteKeyOutcome {
    /// Selection/navigation handled; the key is swallowed.
    Consumed,
    /// Commit the selected command. Carries the full text to set as the new
    /// prompt buffer, e.g. `"/compact "` (leading `/`, trailing space).
    Accept(String),
    /// `Esc` — close the overlay; the key is swallowed (does not also type).
    Dismiss,
    /// The overlay has nothing actionable for this key — let it fall through
    /// to the default input editor (so the char still types, etc.).
    PassThrough,
}

impl PaletteState {
    /// Handle one key while the overlay is open. Only navigation/commit keys
    /// are consumed; printable chars return `PassThrough` so the default input
    /// path inserts them (which then re-runs `sync_from_prompt`).
    pub fn handle_key(&mut self, code: KeyCode) -> PaletteKeyOutcome {
        let rows = self.rows();
        match code {
            KeyCode::Down => {
                if !rows.is_empty() && self.selected + 1 < rows.len() {
                    self.selected += 1;
                }
                PaletteKeyOutcome::Consumed
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                PaletteKeyOutcome::Consumed
            }
            KeyCode::Tab | KeyCode::Enter => {
                if let Some(row) = rows.get(self.selected) {
                    let text = format!("/{} ", row.name);
                    self.open = false;
                    self.filter.clear();
                    self.selected = 0;
                    PaletteKeyOutcome::Accept(text)
                } else {
                    PaletteKeyOutcome::PassThrough
                }
            }
            KeyCode::Esc => {
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                PaletteKeyOutcome::Dismiss
            }
            _ => PaletteKeyOutcome::PassThrough,
        }
    }
}

/// Free-function shim so tests can call `handle_key(&mut state, code)`.
#[cfg(test)]
fn handle_key(state: &mut PaletteState, code: KeyCode) -> PaletteKeyOutcome {
    state.handle_key(code)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui palette::tests 2>&1 | tail -20`
Expected: PASS (10 tests total in the module). `// M7-16: candidate tengu_tui_command_palette_opened` — add this comment on the line above the `Accept` arm to mark the deferred telemetry site (Decision D2).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/components/prompt_input/palette.rs
git commit -m "plan(M7-07 T3): palette select/accept/dismiss key handling

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: `CompletionState` — `@` trigger, cwd candidates, filter

**Files:**
- Create: `crates/tui/src/components/prompt_input/completion.rs`
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/tui/src/components/prompt_input/completion.rs`:

```rust
//! `@` file-ref completion: a dropdown of cwd path entries. Opens when an `@`
//! token is being typed. The filter/select logic takes the candidate `Vec`
//! as input so it's testable without touching the filesystem; `read_cwd_entries`
//! is the only fs-touching fn and is exercised by a separate fs test.
//!
//! Literal lock (design §2.8): inserts `@<path> ` (trailing space), mirroring
//! claude-code QuickOpenDialog handleInsert. Empty-state strings copied below.

use iocraft::prelude::KeyCode;

use super::fuzzy::filtered_ranked;

/// Max dropdown rows (shared with the palette; claude-code `OVERLAY_MAX_ITEMS`).
pub const OVERLAY_MAX_ITEMS: usize = 5;

/// claude-code QuickOpenDialog empty-state literal when a query is present.
pub const EMPTY_WITH_QUERY: &str = "No matching files";
/// claude-code QuickOpenDialog empty-state literal for an empty query.
pub const EMPTY_NO_QUERY: &str = "Start typing to search…";

/// `@` completion overlay state.
#[derive(Debug, Clone, Default)]
pub struct CompletionState {
    /// Whether the dropdown is shown.
    pub open: bool,
    /// The text typed after the active `@` (the partial path).
    pub filter: String,
    /// Index into filtered candidates.
    pub selected: usize,
    /// Candidate paths (relative to cwd). Populated when the overlay opens.
    pub candidates: Vec<String>,
}

/// Find the active `@` token: the substring from the last `@` to the cursor,
/// iff that `@` is at the start or preceded by whitespace and the token has no
/// space. Returns `(at_byte_index, partial)` or `None`.
#[must_use]
pub fn active_at_token(prompt: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = cursor.min(prompt.len());
    let head = &prompt[..cursor];
    let at = head.rfind('@')?;
    let preceded_ok = at == 0 || head[..at].ends_with(|c: char| c.is_whitespace());
    let partial = &head[at + 1..];
    if preceded_ok && !partial.contains(char::is_whitespace) {
        Some((at, partial.to_string()))
    } else {
        None
    }
}

impl CompletionState {
    /// Recompute open-state + filter from the prompt + cursor, using a
    /// pre-supplied candidate list (so tests inject candidates; the live path
    /// calls `read_cwd_entries` first — see Task 9).
    pub fn sync(&mut self, prompt: &str, cursor: usize, candidates: &[String]) {
        match active_at_token(prompt, cursor) {
            Some((_, partial)) => {
                if !self.open || partial != self.filter {
                    self.selected = 0;
                }
                self.open = true;
                self.filter = partial;
                self.candidates = candidates.to_vec();
                let max = self.rows().len();
                if max == 0 {
                    self.selected = 0;
                } else if self.selected >= max {
                    self.selected = max - 1;
                }
            }
            None => {
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                self.candidates.clear();
            }
        }
    }

    /// Filtered, ranked candidate paths for the current filter.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        filtered_ranked(&self.filter, &self.candidates)
            .into_iter()
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands() -> Vec<String> {
        vec!["src/main.rs".into(), "src/lib.rs".into(), "README.md".into()]
    }

    #[test]
    fn active_token_at_start() {
        assert_eq!(active_at_token("@src", 4), Some((0, "src".into())));
    }

    #[test]
    fn active_token_after_whitespace() {
        assert_eq!(active_at_token("see @lib", 8), Some((4, "lib".into())));
    }

    #[test]
    fn no_token_without_at() {
        assert_eq!(active_at_token("plain text", 5), None);
    }

    #[test]
    fn at_mid_word_is_not_a_token() {
        // email-like — `@` not preceded by whitespace → not a completion token.
        assert_eq!(active_at_token("user@host", 9), None);
    }

    #[test]
    fn opens_and_filters_on_at() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(c.open);
        let rows = c.rows();
        assert!(rows.iter().all(|r| r.contains("src")));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn closes_when_token_gone() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(c.open);
        c.sync("plain", 5, &cands());
        assert!(!c.open);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui completion::tests 2>&1 | head -30`
Expected: FAIL — module not declared / undefined symbols.

- [ ] **Step 3: Declare the module**

Add to `crates/tui/src/components/prompt_input/mod.rs`:

```rust
pub mod completion;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui completion::tests 2>&1 | tail -20`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/components/prompt_input/completion.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "plan(M7-07 T4): CompletionState @ trigger + cwd filter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Completion key handling + `read_cwd_entries`

**Files:**
- Modify: `crates/tui/src/components/prompt_input/completion.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `completion.rs`:

```rust
    #[test]
    fn down_up_move_and_clamp() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands()); // 2 rows
        assert_eq!(c.selected, 0);
        c.handle_key(KeyCode::Down);
        assert_eq!(c.selected, 1);
        c.handle_key(KeyCode::Down); // clamp at last
        assert_eq!(c.selected, 1);
        c.handle_key(KeyCode::Up);
        assert_eq!(c.selected, 0);
    }

    #[test]
    fn tab_inserts_path_with_at_and_trailing_space() {
        let mut c = CompletionState::default();
        // prompt is "@s", cursor 2; selecting replaces the @token in place.
        c.sync("@s", 2, &cands());
        let sel = c.rows()[c.selected].clone();
        let outcome = c.handle_key_with_prompt(KeyCode::Tab, "@s", 2);
        match outcome {
            CompletionKeyOutcome::Accept { new_prompt, new_cursor } => {
                let expected = format!("@{sel} ");
                assert_eq!(new_prompt, expected);
                assert_eq!(new_cursor, expected.len());
            }
            other => panic!("expected Accept, got {other:?}"),
        }
        assert!(!c.open);
    }

    #[test]
    fn esc_dismisses() {
        let mut c = CompletionState::default();
        c.sync("@src", 4, &cands());
        assert!(matches!(c.handle_key(KeyCode::Esc), CompletionKeyOutcome::Dismiss));
        assert!(!c.open);
    }

    #[test]
    fn read_cwd_entries_excludes_dotfiles_and_is_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        let entries = read_cwd_entries(dir.path());
        assert_eq!(entries, vec!["a.txt".to_string(), "b.txt".to_string()]);
    }
```

Note: `tempfile` is already a dev-dependency of `lingxi-tui` (used by other behavior tests); if `cargo test` reports it missing, add `tempfile` to `[dev-dependencies]` in `crates/tui/Cargo.toml` matching the version used elsewhere in the workspace (`grep tempfile crates/*/Cargo.toml`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui completion::tests 2>&1 | head -30`
Expected: FAIL — `handle_key` / `handle_key_with_prompt` / `read_cwd_entries` / `CompletionKeyOutcome` undefined.

- [ ] **Step 3: Write minimal implementation**

Add to `completion.rs` (above the test module):

```rust
use std::path::Path;

/// Outcome of a completion key. `Accept` carries the rewritten prompt + cursor
/// because inserting a path edits the buffer in place (replacing the `@token`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKeyOutcome {
    /// Navigation handled; key swallowed.
    Consumed,
    /// Commit the selected path. The dispatcher sets `new_prompt`/`new_cursor`.
    Accept {
        /// The full prompt buffer after inserting `@<path> `.
        new_prompt: String,
        /// The new cursor byte index (end of the inserted token).
        new_cursor: usize,
    },
    /// `Esc` — close; key swallowed.
    Dismiss,
    /// Nothing actionable — fall through to default input.
    PassThrough,
}

impl CompletionState {
    /// Navigation-only handler (no prompt rewrite). Used for Up/Down/Esc.
    pub fn handle_key(&mut self, code: KeyCode) -> CompletionKeyOutcome {
        let len = self.rows().len();
        match code {
            KeyCode::Down => {
                if len > 0 && self.selected + 1 < len {
                    self.selected += 1;
                }
                CompletionKeyOutcome::Consumed
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                CompletionKeyOutcome::Consumed
            }
            KeyCode::Esc => {
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                self.candidates.clear();
                CompletionKeyOutcome::Dismiss
            }
            _ => CompletionKeyOutcome::PassThrough,
        }
    }

    /// Tab/Enter handler that rewrites the prompt in place: replaces the active
    /// `@token` (located via `active_at_token`) with `@<selected> `.
    pub fn handle_key_with_prompt(
        &mut self,
        code: KeyCode,
        prompt: &str,
        cursor: usize,
    ) -> CompletionKeyOutcome {
        match code {
            KeyCode::Tab | KeyCode::Enter => {
                let rows = self.rows();
                let Some(sel) = rows.get(self.selected).cloned() else {
                    return CompletionKeyOutcome::PassThrough;
                };
                let Some((at, _)) = active_at_token(prompt, cursor) else {
                    return CompletionKeyOutcome::PassThrough;
                };
                let cursor = cursor.min(prompt.len());
                let insert = format!("@{sel} ");
                let mut new_prompt = String::with_capacity(prompt.len() + insert.len());
                new_prompt.push_str(&prompt[..at]);
                new_prompt.push_str(&insert);
                let new_cursor = new_prompt.len();
                new_prompt.push_str(&prompt[cursor..]);
                self.open = false;
                self.filter.clear();
                self.selected = 0;
                self.candidates.clear();
                CompletionKeyOutcome::Accept { new_prompt, new_cursor }
            }
            _ => self.handle_key(code),
        }
    }
}

/// Read the immediate (non-recursive) entries of `dir`, excluding dotfiles,
/// returned as file names sorted ASCII-ascending. The only fs-touching fn in
/// this module. Errors → empty list (the overlay just shows the empty state).
#[must_use]
pub fn read_cwd_entries(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    out.sort_unstable();
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui completion::tests 2>&1 | tail -20`
Expected: PASS (10 tests total in the module).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/components/prompt_input/completion.rs crates/tui/Cargo.toml
git commit -m "plan(M7-07 T5): completion key handling + read_cwd_entries

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Add palette/completion fields to `AppState`

**Files:**
- Modify: `crates/tui/src/state.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/tui/src/state.rs`:

```rust
    #[test]
    fn new_state_has_closed_palette_and_completion() {
        let s = AppState::new(fake_status());
        assert!(!s.palette.open);
        assert!(s.palette.filter.is_empty());
        assert!(!s.completion.open);
        assert!(s.completion.filter.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui new_state_has_closed_palette 2>&1 | head -20`
Expected: FAIL — `no field palette on type AppState`.

- [ ] **Step 3: Write minimal implementation**

In `crates/tui/src/state.rs`, add the import near the top (with the other `crate::components` references):

```rust
use crate::components::prompt_input::completion::CompletionState;
use crate::components::prompt_input::palette::PaletteState;
```

Add two fields to the `AppState` struct (place them after `bypass_dialog_state`):

```rust
    /// (M7-07) `/` slash-command palette overlay state. `open == false`
    /// between uses; the live dispatcher routes keys here at priority 3.
    pub palette: PaletteState,
    /// (M7-07) `@` file-ref completion overlay state. Priority 3, same as
    /// the palette — only one can be open at a time (palette wins on `/`).
    pub completion: CompletionState,
```

Initialize them in `AppState::new` (after `bypass_dialog_state: ...`):

```rust
            palette: PaletteState::default(),
            completion: CompletionState::default(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui new_state_has_closed_palette 2>&1 | tail -10`
Expected: PASS. Also run `cargo test -p lingxi-tui 2>&1 | tail -15` to confirm no existing state test regressed.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/state.rs
git commit -m "plan(M7-07 T6): AppState palette + completion overlay fields

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: `PaletteOverlay` iocraft component

**Files:**
- Modify: `crates/tui/src/components/prompt_input/palette.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/tui/tests/snapshot_palette_completion.rs`:

```rust
//! Insta snapshots of the palette + completion dropdowns. The render harness
//! mirrors the other `render_*` tests in this crate (see render_status_line.rs).

use iocraft::prelude::*;
use lingxi_tui::components::prompt_input::palette::{PaletteOverlay, PaletteState};

fn render(el: impl Into<AnyElement<'static>>) -> String {
    let mut canvas = element! { View(width: 60u16) { #(el.into()) } };
    canvas.to_string()
}

#[test]
fn palette_dropdown_three_filtered_commands() {
    let mut state = PaletteState::default();
    state.sync_from_prompt("/co"); // compact / config / context / copy / cost ...
    // Keep snapshot stable: take the first 3 ranked rows.
    let rows: Vec<_> = state.rows().into_iter().take(3).collect();
    let out = render(element! { PaletteOverlay(rows: rows, selected: 0usize) });
    insta::assert_snapshot!(out);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test snapshot_palette_completion palette_dropdown 2>&1 | head -30`
Expected: FAIL — `cannot find PaletteOverlay`.

- [ ] **Step 3: Write minimal implementation**

Add to `palette.rs` (after the impls, before tests):

```rust
use iocraft::prelude::*;

/// Props for the palette dropdown overlay.
#[derive(Default, Props)]
pub struct PaletteOverlayProps {
    /// The filtered rows to display (caller truncates to `OVERLAY_MAX_ITEMS`).
    pub rows: Vec<PaletteRow>,
    /// Index of the highlighted row within `rows`.
    pub selected: usize,
}

/// Render the palette dropdown: up to `OVERLAY_MAX_ITEMS` rows, each
/// `name – description`. The selected row is highlighted; the rest are dim.
/// Literal lock: `" – "` is U+2013 with surrounding spaces (claude-code
/// PromptInputFooterSuggestions row format).
#[component]
pub fn PaletteOverlay(props: &PaletteOverlayProps) -> impl Into<AnyElement<'static>> {
    let rows: Vec<_> = props.rows.iter().take(OVERLAY_MAX_ITEMS).cloned().collect();
    let selected = props.selected;
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows.into_iter().enumerate().map(|(i, row)| {
                let line = format!("/{} \u{2013} {}", row.name, row.description);
                let color = if i == selected { Color::Cyan } else { Color::DarkGrey };
                element! {
                    View(height: 1) {
                        Text(content: line, color: color)
                    }
                }
            }))
        }
    }
}
```

If `Color::Cyan`/`Color::DarkGrey` don't match the project's theme convention, use the same color expression the existing `ReplScreen`/`status_line.rs` components use for selected vs dim text — grep `crates/tui/src/components/status_line.rs` for `Color::` and mirror it.

- [ ] **Step 4: Run + accept snapshot**

Run: `cargo test -p lingxi-tui --test snapshot_palette_completion palette_dropdown 2>&1 | tail -20`
Then review and accept: `cargo insta review` (accept if the dropdown shows 3 rows, the first highlighted, each line `/<name> – <desc>`). Confirm the en-dash renders, not a hyphen.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/components/prompt_input/palette.rs crates/tui/tests/snapshot_palette_completion.rs crates/tui/tests/snapshots/
git commit -m "plan(M7-07 T7): PaletteOverlay component + dropdown snapshot

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: `CompletionOverlay` iocraft component

**Files:**
- Modify: `crates/tui/src/components/prompt_input/completion.rs`
- Modify: `crates/tui/tests/snapshot_palette_completion.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/tui/tests/snapshot_palette_completion.rs`:

```rust
use lingxi_tui::components::prompt_input::completion::CompletionOverlay;

#[test]
fn completion_dropdown_three_paths() {
    let rows = vec![
        "src/lib.rs".to_string(),
        "src/main.rs".to_string(),
        "README.md".to_string(),
    ];
    let out = render(element! {
        CompletionOverlay(rows: rows, selected: 1usize, empty_query: false)
    });
    insta::assert_snapshot!(out);
}

#[test]
fn completion_empty_state_no_query() {
    let out = render(element! {
        CompletionOverlay(rows: Vec::<String>::new(), selected: 0usize, empty_query: true)
    });
    insta::assert_snapshot!(out);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test snapshot_palette_completion completion_ 2>&1 | head -30`
Expected: FAIL — `cannot find CompletionOverlay`.

- [ ] **Step 3: Write minimal implementation**

Add to `completion.rs` (above the test module):

```rust
use iocraft::prelude::*;

/// Props for the completion dropdown overlay.
#[derive(Default, Props)]
pub struct CompletionOverlayProps {
    /// Filtered candidate paths.
    pub rows: Vec<String>,
    /// Highlighted row index.
    pub selected: usize,
    /// Whether the active `@` token has no partial text yet (drives the
    /// empty-state literal).
    pub empty_query: bool,
}

/// Render the `@` completion dropdown. Each row is `+ <path>` (claude-code
/// file icon `+`). The empty state shows the QuickOpenDialog literal.
#[component]
pub fn CompletionOverlay(props: &CompletionOverlayProps) -> impl Into<AnyElement<'static>> {
    let selected = props.selected;
    if props.rows.is_empty() {
        let msg = if props.empty_query { EMPTY_NO_QUERY } else { EMPTY_WITH_QUERY };
        return element! {
            View(height: 1) { Text(content: msg.to_string(), color: Color::DarkGrey) }
        }
        .into_any();
    }
    let rows: Vec<_> = props.rows.iter().take(OVERLAY_MAX_ITEMS).cloned().collect();
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows.into_iter().enumerate().map(|(i, path)| {
                let line = format!("+ {path}");
                let color = if i == selected { Color::Cyan } else { Color::DarkGrey };
                element! {
                    View(height: 1) { Text(content: line, color: color) }
                }
            }))
        }
    }
    .into_any()
}
```

- [ ] **Step 4: Run + accept snapshots**

Run: `cargo test -p lingxi-tui --test snapshot_palette_completion completion_ 2>&1 | tail -20`
Then `cargo insta review`. Accept if: the 3-path snapshot shows `+ <path>` rows with row index 1 highlighted; the empty-state snapshot shows `Start typing to search…`.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/components/prompt_input/completion.rs crates/tui/tests/snapshot_palette_completion.rs crates/tui/tests/snapshots/
git commit -m "plan(M7-07 T8): CompletionOverlay component + dropdown snapshots

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 9: Wire the priority-3 branch into `handle_live_key`

**Files:**
- Modify: `crates/tui/src/root.rs`

This is the focus-trap integration — the load-bearing task. The branch must (a) sit *after* permission (priority 1) and any screen (priority 2) branch, *before* the default input path; (b) keep `palette`/`completion` open-state in sync after every default-path edit; (c) ensure only one overlay is open at a time (palette wins on `/`).

- [ ] **Step 1: Write the failing test (behavior, drives `handle_live_key`)**

Create `crates/tui/tests/behavior_palette.rs`:

```rust
//! M7-07 palette behavior + focus-trap, driving `root::handle_live_key` — THE
//! function the live `use_terminal_events` closure calls (mirrors
//! live_focus_trap_test.rs).

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lingxi_tui::root::handle_live_key;
use lingxi_tui::state::{AppState, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

#[test]
fn typing_slash_opens_palette_and_filters() {
    let mut st = AppState::new(StatusSnapshot::default());
    handle_live_key(&mut st, &key(KeyCode::Char('/')), 24);
    assert!(st.palette.open, "/ opens the palette");
    handle_live_key(&mut st, &key(KeyCode::Char('c')), 24);
    handle_live_key(&mut st, &key(KeyCode::Char('o')), 24);
    assert_eq!(st.prompt_text, "/co");
    assert!(st.palette.rows().iter().any(|r| r.name == "compact"));
}

#[test]
fn arrow_selects_then_tab_completes() {
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/comp".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    let sel = st.palette.rows()[st.palette.selected].name.to_string();
    handle_live_key(&mut st, &key(KeyCode::Tab), 24);
    assert_eq!(st.prompt_text, format!("/{sel} "));
    assert!(!st.palette.open, "completing closes the palette");
}

#[test]
fn esc_dismisses_palette_but_keeps_prompt() {
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/co".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    handle_live_key(&mut st, &key(KeyCode::Esc), 24);
    assert!(!st.palette.open);
    assert_eq!(st.prompt_text, "/co", "Esc dismisses overlay, not the text");
}

#[test]
fn focus_trap_down_key_does_not_scroll_scrollback() {
    // With the palette open, Down moves the palette selection — it must NOT
    // be interpreted as history/scroll on the underlying input surface.
    let mut st = AppState::new(StatusSnapshot::default());
    st.history.push("old line".into());
    for ch in "/co".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    let before = st.prompt_text.clone();
    handle_live_key(&mut st, &key(KeyCode::Down), 24);
    assert_eq!(st.palette.selected, 1, "Down moved palette selection");
    assert_eq!(st.prompt_text, before, "Down did not pull history into prompt");
}

#[test]
fn permission_pending_beats_open_palette() {
    // Cross-state seam (design §5.6): if a permission is pending AND the palette
    // is open, the permission focus-trap (priority 1) wins.
    use lingxi_permission::gate::{PermissionRequest, PromptDefault};
    use lingxi_tui::state::PendingPermission;
    use serde_json::json;
    use tokio::sync::oneshot;

    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/co".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    assert!(st.palette.open);
    let (tx, _rx) = oneshot::channel();
    st.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    st.pending_permission_resp_tx = Some(tx);
    st.pending_permission_started_at = Some(std::time::Instant::now());

    // A palette-navigation key must route to the permission dialog, not the
    // palette: the dialog stays open and the palette selection is untouched.
    handle_live_key(&mut st, &key(KeyCode::Down), 24);
    assert_eq!(st.palette.selected, 0, "permission owns keys; palette unchanged");
    assert!(st.pending_permission.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test behavior_palette 2>&1 | head -30`
Expected: FAIL — `/` currently just inserts a char; `st.palette` never opens (no priority-3 branch yet).

- [ ] **Step 3: Write minimal implementation**

In `crates/tui/src/root.rs`, add the imports near the top:

```rust
use crate::components::prompt_input::completion::{CompletionKeyOutcome};
use crate::components::prompt_input::palette::PaletteKeyOutcome;
```

Replace the body of `handle_live_key` (keep the permission branch first) so the priority-3 overlay branch sits between permission and the default path:

```rust
pub fn handle_live_key(st: &mut AppState, k: &KeyEvent, viewport: usize) {
    // === Priority 1: permission focus-trap (M6-05). ===
    if st.pending_permission.is_some() {
        let ct_key = iocraft_to_crossterm028_key(k);
        let _ = crate::events::keymap::handle_key(st, ct_key);
        return;
    }

    // === Priority 2: active screen (M7-11+). ===
    // NOTE: if a screen branch has already landed it lives here, before the
    // overlay branch. Leave it untouched.

    // === Priority 3: palette / completion overlay focus-trap (M7-07). ===
    if st.palette.open {
        match st.palette.handle_key(k.code) {
            PaletteKeyOutcome::Consumed | PaletteKeyOutcome::Dismiss => return,
            PaletteKeyOutcome::Accept(text) => {
                st.prompt_cursor = text.len();
                st.prompt_text = text;
                st.palette.sync_from_prompt(&st.prompt_text);
                return;
            }
            PaletteKeyOutcome::PassThrough => { /* fall through to default edit */ }
        }
    } else if st.completion.open {
        match st.completion.handle_key_with_prompt(k.code, &st.prompt_text, st.prompt_cursor) {
            CompletionKeyOutcome::Consumed | CompletionKeyOutcome::Dismiss => return,
            CompletionKeyOutcome::Accept { new_prompt, new_cursor } => {
                st.prompt_text = new_prompt;
                st.prompt_cursor = new_cursor;
                st.completion.sync(&st.prompt_text, st.prompt_cursor, &st.completion.candidates.clone());
                return;
            }
            CompletionKeyOutcome::PassThrough => { /* fall through */ }
        }
    }
    // === end priority 3 ===

    let prompt_empty = st.prompt_text.is_empty();
    let focus_active = prompt_empty
        && st
            .messages
            .iter()
            .any(|m| matches!(m, crate::state::RenderedMessage::AssistantToolUse { .. }));
    if let Some(action) = map_iocraft_key(k, prompt_empty, focus_active) {
        if let KeyAction::ScrollStep(dir) = action {
            scroll_with_viewport(st, dir, viewport);
        } else {
            let _ = dispatch(action, st);
        }
    }

    // === Re-sync overlays after a default-path edit. ===
    // Palette wins when the buffer is a `/command` token; otherwise check the
    // `@` token. Only one overlay is open at a time.
    st.palette.sync_from_prompt(&st.prompt_text);
    if st.palette.open {
        st.completion.open = false;
    } else {
        let cwd_entries = crate::components::prompt_input::completion::read_cwd_entries(
            &st.status.cwd,
        );
        st.completion.sync(&st.prompt_text, st.prompt_cursor, &cwd_entries);
    }
}
```

Note: `st.status.cwd` is the `PathBuf` already on `StatusSnapshot` (see `state.rs`). Using it keeps `read_cwd_entries` deterministic in behavior tests (tests construct `StatusSnapshot::default()` whose cwd is `"."` — a real dir, so the call is harmless; the focus-trap assertions don't depend on which files exist).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test behavior_palette 2>&1 | tail -20`
Expected: PASS (5 tests). If `permission_pending_beats_open_palette` fails, the priority-3 branch is above the permission branch — move it below.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/root.rs crates/tui/tests/behavior_palette.rs
git commit -m "plan(M7-07 T9): priority-3 palette/completion focus-trap in handle_live_key

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 10: Completion behavior + focus-trap tests through `handle_live_key`

**Files:**
- Create: `crates/tui/tests/behavior_completion.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/tui/tests/behavior_completion.rs`:

```rust
//! M7-07 `@` completion behavior + focus-trap via `root::handle_live_key`.
//! These run in a temp dir so the cwd listing is deterministic.

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lingxi_tui::root::handle_live_key;
use lingxi_tui::state::{AppState, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

/// Build a state whose status.cwd points at a temp dir with two known files.
fn state_with_files() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("alpha.rs"), "").unwrap();
    std::fs::write(dir.path().join("beta.rs"), "").unwrap();
    let mut status = StatusSnapshot::default();
    status.cwd = dir.path().to_path_buf();
    (AppState::new(status), dir)
}

#[test]
fn typing_at_opens_completion_with_cwd_entries() {
    let (mut st, _dir) = state_with_files();
    handle_live_key(&mut st, &key(KeyCode::Char('@')), 24);
    assert!(st.completion.open, "@ opens completion");
    let rows = st.completion.rows();
    assert!(rows.iter().any(|r| r == "alpha.rs"));
    assert!(rows.iter().any(|r| r == "beta.rs"));
}

#[test]
fn filter_narrows_completion() {
    let (mut st, _dir) = state_with_files();
    for ch in "@al".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    let rows = st.completion.rows();
    assert_eq!(rows, vec!["alpha.rs".to_string()]);
}

#[test]
fn tab_inserts_path_with_at_and_trailing_space() {
    let (mut st, _dir) = state_with_files();
    for ch in "@al".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    handle_live_key(&mut st, &key(KeyCode::Tab), 24);
    assert_eq!(st.prompt_text, "@alpha.rs ");
    assert!(!st.completion.open, "insert closes completion");
}

#[test]
fn esc_dismisses_completion_keeps_text() {
    let (mut st, _dir) = state_with_files();
    for ch in "@al".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    handle_live_key(&mut st, &key(KeyCode::Esc), 24);
    assert!(!st.completion.open);
    assert_eq!(st.prompt_text, "@al");
}

#[test]
fn focus_trap_down_moves_completion_not_history() {
    let (mut st, _dir) = state_with_files();
    st.history.push("old".into());
    handle_live_key(&mut st, &key(KeyCode::Char('@')), 24); // both files listed
    let before = st.prompt_text.clone();
    handle_live_key(&mut st, &key(KeyCode::Down), 24);
    assert_eq!(st.completion.selected, 1);
    assert_eq!(st.prompt_text, before, "Down did not recall history");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test behavior_completion 2>&1 | head -30`
Expected: FAIL initially only if a bug exists — but Task 9 already wired the path, so these may pass. If they pass on first run, that's acceptable (the tests still lock behavior); if any fails, fix in `completion.rs`/`root.rs` and re-run. The `focus_trap_down_moves_completion_not_history` case is the seam guard.

- [ ] **Step 3: Make them pass**

If `typing_at_opens_completion` fails because the re-sync in `handle_live_key` reads the process cwd instead of `st.status.cwd`, confirm Task 9 used `&st.status.cwd`. No other impl change expected.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test behavior_completion 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/tests/behavior_completion.rs
git commit -m "plan(M7-07 T10): completion behavior + focus-trap tests

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 11: Render the active overlay above PromptInput in `ReplScreen`

**Files:**
- Modify: `crates/tui/src/app.rs` (`render_screen`)
- Modify: `crates/tui/src/screens/repl.rs` (`ReplScreen`)

- [ ] **Step 1: Write the failing test (snapshot of the REPL screen with palette open)**

Append to `crates/tui/tests/snapshot_palette_completion.rs`:

```rust
use lingxi_tui::app::render_screen;
use lingxi_tui::state::{AppState, StatusSnapshot};

#[test]
fn repl_screen_shows_palette_above_prompt() {
    let mut st = AppState::new(StatusSnapshot::default());
    // Open the palette via the public sync the live path uses.
    st.prompt_text = "/co".into();
    st.prompt_cursor = 3;
    st.palette.sync_from_prompt(&st.prompt_text);
    let el = render_screen(&st, 10);
    let out = render(el);
    assert!(out.contains('\u{2013}'), "palette rows use the en-dash separator");
    insta::assert_snapshot!(out);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test snapshot_palette_completion repl_screen_shows_palette 2>&1 | head -30`
Expected: FAIL — the screen renders the prompt but not the dropdown (assertion on `\u{2013}` fails).

- [ ] **Step 3: Write minimal implementation**

In `crates/tui/src/screens/repl.rs`, add two optional props to `ReplScreenProps` (a pre-rendered overlay element is simplest — but iocraft props can't hold `AnyElement` cleanly, so pass the data and render inside): add fields `palette: Option<PaletteState>` and `completion: Option<CompletionState>` (clone-friendly), plus the necessary `use` imports. In the `ReplScreen` component body, immediately above the `PromptInput` row, conditionally render:

```rust
            #(palette.as_ref().filter(|p| p.open).map(|p| {
                let rows: Vec<_> = p.rows().into_iter().take(
                    crate::components::prompt_input::palette::OVERLAY_MAX_ITEMS
                ).collect();
                element! {
                    crate::components::prompt_input::palette::PaletteOverlay(
                        rows: rows, selected: p.selected,
                    )
                }
            }))
            #(completion.as_ref().filter(|c| c.open).map(|c| {
                let rows = c.rows();
                let empty_query = c.filter.is_empty();
                element! {
                    crate::components::prompt_input::completion::CompletionOverlay(
                        rows: rows, selected: c.selected, empty_query: empty_query,
                    )
                }
            }))
```

In `crates/tui/src/app.rs::render_screen`, pass the clones into the `ReplScreen` element (alongside the existing `status`, `messages`, … props):

```rust
        let palette = Some(state.palette.clone());
        let completion = Some(state.completion.clone());
```

and add `palette: palette, completion: completion,` to the `element! { ReplScreen(...) }` invocation.

If iocraft rejects `Option<PaletteState>` as a prop type (needs `Default`), `PaletteState`/`CompletionState` already derive `Default`, and `Option<T: Default>` defaults to `None` — fine. If the macro complains, fall back to passing `palette_open: bool` + the pre-filtered `Vec<PaletteRow>` + `palette_selected: usize` as separate primitive props (still clone-friendly).

- [ ] **Step 4: Run + accept snapshot**

Run: `cargo test -p lingxi-tui --test snapshot_palette_completion repl_screen_shows_palette 2>&1 | tail -20`
Then `cargo insta review`. Accept if the screen shows the 3-zone REPL with the palette dropdown rows rendered just above the `> /co` prompt line.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/app.rs crates/tui/src/screens/repl.rs crates/tui/tests/snapshot_palette_completion.rs crates/tui/tests/snapshots/
git commit -m "plan(M7-07 T11): render palette/completion overlay above PromptInput

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 12: Edge cases — Submit while overlay open; backspacing closes; `@` after `/`

**Files:**
- Modify: `crates/tui/tests/behavior_palette.rs`
- Modify: `crates/tui/src/root.rs` (only if a test exposes a gap)

- [ ] **Step 1: Write the failing tests**

Append to `crates/tui/tests/behavior_palette.rs`:

```rust
#[test]
fn enter_with_no_palette_match_submits_normally() {
    // "/zzzz" matches nothing → Enter must PassThrough to Submit (clears prompt).
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/zzzznomatch".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    assert!(st.palette.open);
    assert!(st.palette.rows().is_empty());
    handle_live_key(&mut st, &key(KeyCode::Enter), 24);
    assert!(st.prompt_text.is_empty(), "Enter submitted the line");
}

#[test]
fn backspacing_the_slash_closes_palette() {
    let mut st = AppState::new(StatusSnapshot::default());
    handle_live_key(&mut st, &key(KeyCode::Char('/')), 24);
    assert!(st.palette.open);
    handle_live_key(&mut st, &key(KeyCode::Backspace), 24);
    assert_eq!(st.prompt_text, "");
    assert!(!st.palette.open, "removing the / closes the palette");
}

#[test]
fn at_token_after_text_opens_completion_not_palette() {
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "look @".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    assert!(!st.palette.open, "no leading / → palette stays closed");
    assert!(st.completion.open, "@ after text opens completion");
}
```

- [ ] **Step 2: Run test to verify it fails (or passes)**

Run: `cargo test -p lingxi-tui --test behavior_palette 2>&1 | head -30`
Expected: these likely PASS already given Task 9's re-sync logic. `enter_with_no_palette_match_submits_normally` is the one to watch — it verifies `PaletteKeyOutcome::PassThrough` (no rows) lets Enter reach `dispatch(Submit)`. If it fails, the palette `handle_key` is returning `Accept`/`Consumed` for the no-rows Enter case — fix per Task 3 (`PassThrough` when `rows.get(selected)` is `None`).

- [ ] **Step 3: Fix if needed**

Apply the minimal fix in `palette.rs` so a no-rows `Enter`/`Tab` returns `PassThrough` (already specified in Task 3; this task just guards it).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test behavior_palette 2>&1 | tail -20`
Expected: PASS (8 tests total in the file).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/tests/behavior_palette.rs crates/tui/src/components/prompt_input/palette.rs
git commit -m "plan(M7-07 T12): edge cases — submit/backspace/@-after-text seams

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 13: Telemetry count guard (0 new events) + literal-lock note

**Files:**
- Modify: `crates/tui/tests/behavior_palette.rs` (or wherever the crate's event-count guard lives — grep first)

- [ ] **Step 1: Confirm where the count is asserted**

Run: `grep -rn "ALL_EVENT_NAMES\|326\|\.len()" crates/telemetry/src crates/tui/tests 2>&1 | grep -i event | head`
Find the canonical `ALL_EVENT_NAMES.len()` assertion (likely in `lingxi-telemetry`). Do **not** modify it — M7-07 adds zero events (Decision D2), so the count stays 326.

- [ ] **Step 2: Write a guard test asserting no palette event was registered**

If a telemetry registry test exists in `lingxi-telemetry`, add there; otherwise append to `crates/tui/tests/behavior_palette.rs`:

```rust
#[test]
fn m7_07_registers_no_new_telemetry_events() {
    // Decision D2: tengu_tui_command_palette_opened is DEFERRED to M7-16.
    // M7-07 must not register it. If this fails, an event leaked in early —
    // remove it (the count audit happens once, in M7-16).
    let names = lingxi_telemetry::ALL_EVENT_NAMES; // adjust path to the real const
    assert_eq!(names.len(), 326, "M7-07 adds zero events (D2)");
    assert!(
        !names.contains(&"tengu_tui_command_palette_opened"),
        "palette telemetry is deferred to M7-16"
    );
}
```

Adjust the import path to the actual `ALL_EVENT_NAMES` location found in Step 1. If the const isn't reachable from `lingxi-tui`'s test crate, place this test in the crate that owns it instead, keeping the same two assertions.

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test m7_07_registers_no_new_telemetry_events 2>&1 | tail -20`
Expected: PASS — `326` and the name is absent.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "plan(M7-07 T13): guard 0 new telemetry events (palette event deferred to M7-16)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 14: Workspace verification gate + tag `m7.7`

**Files:** none (verification + tag only)

- [ ] **Step 1: Format**

Run (from inside `lingxi-code/`): `cargo fmt --check`
Expected: clean. If not, run `cargo fmt` and amend the relevant prior commit's intent with a fresh fixup commit (do NOT `--amend` per §6.4; just `git commit -m "plan(M7-07 T14): cargo fmt"`).

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
Expected: zero warnings. Common M7-07 lints: `needless_pass_by_value` on overlay props (allow or take refs), `too_many_lines` on `handle_live_key` (add `#[allow(clippy::too_many_lines)]` — the fn already carries it). Fix any real lint; commit if changed.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace 2>&1 | tail -40`
Expected: PASS. Allowed-flake reruns (design §5.4): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, the `lingxi-platform-posix` fs_watch FSEvents timing tests. Re-run once if one of these flakes; a second failure of a non-flake test is a real failure.

- [ ] **Step 4: Cross-platform compile gate (5 targets)**

Run:
```bash
cargo check --workspace --target x86_64-unknown-linux-gnu 2>&1 | tail -5
cargo check --workspace --target x86_64-apple-darwin 2>&1 | tail -5
cargo check --workspace --target x86_64-pc-windows-gnu 2>&1 | tail -5
cargo check --workspace --target aarch64-linux-android 2>&1 | tail -5
cargo check --workspace --target aarch64-apple-ios 2>&1 | tail -5
```
Expected: all green. `read_cwd_entries` uses only `std::fs` + `std::path` (portable). If a target isn't installed, install via `rustup target add <t>` (same posture as v0.7.0).

- [ ] **Step 5: Tag**

```bash
git tag -a m7.7 -m "M7-07: command palette + @ file-ref completion (focus-trap priority 3)"
```

(Local tag only — no remote push, no force, per design §6.4.)

- [ ] **Step 6: Final commit (if Steps 1-2 produced fixes)**

```bash
git add -A
git commit -m "plan(M7-07 T14): workspace gate green; tag m7.7

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage (design §2.5 + §3 M7-07):**
- `/` palette autocomplete, live filter, arrow select, Tab/Enter complete, Esc dismiss, overlay above PromptInput → Tasks 2, 3, 7, 11; behavior in Task 9.
- `@` completion against cwd, fuzzy filter, arrow select, Tab/Enter insert, Esc dismiss, overlay → Tasks 4, 5, 8, 11; behavior in Task 10.
- Focus-trap at priority 3 via the single `handle_live_key` dispatcher (below permission + screen, above default input) → Task 9.
- Esc returns keys to the input → Tasks 3, 5, 9 (Esc closes overlay; the next key falls through).
- Cross-state seam (palette open + permission pending → permission wins) → Task 9 `permission_pending_beats_open_palette`.
- Snapshots: palette 3 filtered commands + completion 3 paths → Tasks 7, 8 (+ REPL-with-overlay snapshot in Task 11).
- Telemetry 0 new events, palette event deferred to M7-16 → Decision D2 + Task 13.
- Fuzzy filter without a new dependency → Decision D1 + Task 1.
- Workspace gate from inside `lingxi-code/` + tag `m7.7` → Task 14.

**Placeholder scan:** No TBD/TODO. Every code step shows complete code; commands show expected output. The one "may already pass" note (Tasks 10, 12) is deliberate — those tests lock behavior that Task 9 establishes, and TDD still requires writing them.

**Type consistency:** `PaletteState`/`CompletionState` field names (`open`, `filter`, `selected`, `candidates`) are identical across state.rs (Task 6), the overlay components (Tasks 7, 8), and the dispatcher (Task 9). `PaletteKeyOutcome` (`Consumed`/`Accept(String)`/`Dismiss`/`PassThrough`) and `CompletionKeyOutcome` (`Consumed`/`Accept{new_prompt,new_cursor}`/`Dismiss`/`PassThrough`) are used consistently. `subsequence_match`/`filtered_ranked` signatures match between Task 1 and their callers in Tasks 2 and 4. `OVERLAY_MAX_ITEMS = 5` is the same constant referenced in palette.rs, completion.rs, and repl.rs. `read_cwd_entries(&Path)` signature matches its caller in Task 9.

**Known integration risks the implementer should watch:**
1. M7-06 submodule must exist (guard at top). If `prompt_input/mod.rs` re-exports differ from assumed, adjust the `pub mod` lines and import paths only — logic is unaffected.
2. If M7-11+ already landed a priority-2 screen branch in `handle_live_key`, place the priority-3 branch *after* it. The plan's Task 9 snippet shows the slot.
3. iocraft prop typing for `Option<PaletteState>` — fallback to primitive props documented in Task 11 Step 3.
