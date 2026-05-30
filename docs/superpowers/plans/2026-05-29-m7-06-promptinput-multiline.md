# LingXi Core M7 · Plan 06 · PromptInput multi-line + footer

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard. Run all `cargo` commands **from inside `lingxi-code/`** (the toolchain pins rust 1.82.0 there; running from the repo root uses the host toolchain and produces spurious lint noise — this bit M6-08).

**Goal:** Refactor the single-file M6 editor `crates/tui/src/components/prompt_input.rs` into a `prompt_input/` submodule (`mod.rs` = editor core, `footer.rs` = footer surface), then grow the editor from single-line to multi-line: newline insertion (Shift+Enter, with `\`+Enter fallback), cursor up/down across lines, line/visual-line height that grows the `PromptInput` zone from 1 to N rows, and a footer surface (placeholder text, help-menu hint, mode-indicator, suggestions area). The refactor is move-don't-rewrite: every M6 behavior (line editing, history nav, submit, backspace, Home/End, the live-key dispatcher, slash routing) stays green. Enter still submits; the new newline key adds a line. Telemetry adds **0** events (baseline stays 326).

**Architecture:** `components/prompt_input.rs` becomes `components/prompt_input/mod.rs` verbatim (move only) in Task 1 so all `crate::components::prompt_input::{apply_insert, apply_backspace, apply_move, CursorMove, PromptInput, PromptInputProps}` import paths in `app.rs`, `root.rs`, `screens/repl.rs`, and the keymap stay byte-identical — no call-site edits. Multi-line is then layered onto `mod.rs`: the existing `prompt_text: String` keeps holding the whole buffer, now with embedded `\n`; new pure helpers (`line_starts`, `cursor_line_col`, `apply_move_vertical`, `apply_newline`, `visual_row_count`) compute line geometry and vertical cursor motion as pure functions of `(text, cursor, width)`, mirroring the M6 pure-function-then-component split. A new `KeyAction::InsertNewline` variant (emitted by Shift+Enter / `\`+Enter in both keymaps) routes through `app::dispatch`. The `PromptInput` component renders one `Text` per visual row and the REPL screen sizes the zone from `visual_row_count`. A sibling `footer.rs` holds the `PromptInputFooter` component (placeholder / help hint / mode-indicator / suggestions placeholder), pure and snapshot-tested. No AppState shape change beyond the existing `prompt_text`/`prompt_cursor`; no engine wiring; the single `handle_live_key` dispatcher (§2.5 priority order) is preserved — `InsertNewline` slots into priority 4 (input widget) exactly like `InsertChar`.

**Tech Stack:** Rust 2021, edition/rust-version from workspace (rust 1.82). `iocraft = "=0.8.3"` (`View`, not `Box`; `Text`). New deps added to `crates/tui/Cargo.toml`: `unicode-segmentation = "=1.12.0"` and `unicode-width = "=0.1.14"` (exact versions already resolved in `lingxi-code/Cargo.lock`; the workspace already builds them transitively, so this is a no-network pin). `insta = "1.40"` (yaml) for footer + multi-line snapshots, already a dev-dependency. No `StyledLine` type exists yet (it lands in M7-01); M7-06 does **not** depend on it — the footer renders with plain `Text` + `Color` from `theme::TuiTheme`, matching M6.

**References:**

- Parent design spec: `docs/superpowers/specs/2026-05-29-m7-tui-surface-design.md` — §1 (goal/non-goals), §2.1 (the `prompt_input/` submodule refactor table, lines 93-100), §2.5 (live-key routing priority order), §3 "M7-06" entry (lines 204-207), §5.2 (test budget: 2 snapshots + 6+ behavior).
- Predecessor (M6, the code being refactored):
  - `crates/tui/src/components/prompt_input.rs` — the ENTIRE current file. Pure helpers `apply_insert`, `apply_backspace`, `apply_move`, enum `CursorMove {Left,Right,Home,End}`, the `PromptInput` component + `PromptInputProps {text, cursor}`. Six existing `#[cfg(test)]` unit tests (`insert_at_end`, `insert_at_middle`, `backspace_at_zero_is_noop`, `backspace_removes_prev_char`, `move_home_end`, `move_left_right_utf8`). **All must move verbatim and stay green.**
  - `crates/tui/src/events/keymap.rs` — `KeyAction` enum, `map_key(evt, prompt_empty, focus_active)`, `handle_key` (crossterm-0.28 dispatcher). M6-02 tests in `mod m6_02_tests`.
  - `crates/tui/src/root.rs` — `handle_live_key(st, k, viewport)` (THE single live-key dispatcher), `map_iocraft_key` (crossterm-0.29 mirror of `map_key`), `viewport_height(rows) = rows - 3`.
  - `crates/tui/src/app.rs` — `dispatch(action, st)` (consumes `KeyAction`, mutates `AppState`), `render_screen`.
  - `crates/tui/src/screens/repl.rs` — `ReplScreen` 3-zone layout; mounts `PromptInput(text, cursor)` at fixed `height: 1`.
  - `crates/tui/src/state.rs` — `AppState.prompt_text: String`, `prompt_cursor: usize` (byte index, always at a char boundary), `history`, `history_cursor`.
  - `crates/tui/src/theme.rs` — `TuiTheme::{ASSISTANT, USER, ERROR, DIM}` (iocraft `Color` constants). The footer uses `DIM` for hints.
- claude-code byte-locks (verified by direct read at plan-writing time — literal-lock per §2.8):
  - `claude-code/src/components/PromptInput/PromptInputModeIndicator.tsx:44-92` — prompt-mode glyph is `figures.pointer` = `❯` followed by a space (`PromptChar` renders `<Text>{figures.pointer} </Text>`); bash-mode glyph is `! ` (`<Text color="bashBorder">! </Text>`). M7-06 ships the prompt glyph `❯ `; the `bash`/teammate variants are M7-07+/M8 — the indicator takes a `mode` arg defaulting to prompt.
  - `claude-code/src/components/PromptInput/PromptInputHelpMenu.tsx:143-175` — help-hint literals (each on its own line): `! for bash mode`, `/ for commands`, `@ for file paths`, `& for background`, `/btw for side question`. M7-06 ships the **collapsed one-line hint** shown when the menu is closed — see "Help hint literal" below — and the full menu is M7-07's palette work.
  - `claude-code/src/components/PromptInput/utils.ts:17-32` — `getNewlineInstructions()` returns `'shift + ⏎ for newline'` (when Shift+Enter binding is installed / Apple Terminal) else `'backslash (\\) + return (⏎) for newline'` (or `'\\⏎ for newline'` once used). M7-06's footer renders the literal **`shift + ⏎ for newline`** (LingXi assumes the Shift+Enter binding; the backslash fallback path is the keymap escape hatch, see "Newline-vs-submit decision").
  - `claude-code/src/components/PromptInput/PromptInput.tsx:1312-1313,1662` — newline is the `chat:newline` keybinding bound to `handleNewline` (insert `\n` at cursor); `onSubmit` is the default Enter binding (`PromptInput.tsx:164,1100`). I.e. **Enter submits, the newline key inserts a line** — never the reverse.
  - `claude-code/src/components/PromptInput/PromptInputFooterSuggestions.tsx:18,24-29` — `OVERLAY_MAX_ITEMS = 5`; suggestion icons `+` (file), `◇` (mcp-resource), `*` (agent). M7-06 lays out the **suggestions area as an empty placeholder** (the live filtering/selection is M7-07); the snapshot locks the empty-state layout only.

**Newline-vs-submit decision (LOCKED):** Following claude-code, **Enter submits** the prompt (unchanged from M6 `KeyAction::Submit`) and the **newline key inserts `\n` at the cursor**. The primary newline key is **Shift+Enter** (crossterm: `KeyCode::Enter` with `KeyModifiers::SHIFT`). Because many terminals do not distinguish Shift+Enter from Enter, M7-06 ALSO accepts a **backslash-return fallback**: a literal `\` immediately followed by Enter inserts a newline and removes the trailing `\` (matching claude-code's `hasUsedBackslashReturn` path). The fallback is implemented in the keymap, not the editor, so the editor's `apply_newline` stays a pure `(text, cursor) -> (text, cursor)` helper. Plain Enter with no pending `\` always submits.

**Literal lock — footer strings (copy byte-for-byte):**
- Newline hint: `shift + ⏎ for newline`
- Collapsed help hint (menu closed): `? for shortcuts`  *(claude-code shows this when `!suppressHint`; `PromptInputFooterLeftSide` renders `? for shortcuts` via the KeyboardShortcutHint — verified the surrounding `suppressHint` gate at `PromptInputFooter.tsx:122`).*  If the exact `? for shortcuts` literal cannot be confirmed at implementation time, fall back to the help-menu line literals (`/ for commands`, `@ for file paths`, `! for bash mode`) and note the divergence in the M7-16 literal-lock catalog.
- Mode-indicator prompt glyph: `❯ ` (U+276F + space)

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/tui/src/components/prompt_input.rs` | (M6 single file) | **Deleted** in Task 1 (moved) |
| `crates/tui/src/components/prompt_input/mod.rs` | Editor core: M6 pure helpers (moved verbatim) + new multi-line geometry helpers + the `PromptInput` component (now multi-row). Re-exports `footer`. | Create (Task 1), grow (Tasks 3-9) |
| `crates/tui/src/components/prompt_input/footer.rs` | `PromptInputFooter` component + `FooterMode` enum + `PromptInputFooterProps`. Renders mode-indicator, placeholder, help hint, suggestions placeholder. Pure render, snapshot-tested. | Create (Task 10) |
| `crates/tui/src/events/keymap.rs` | Add `KeyAction::InsertNewline`; map Shift+Enter + `\`-return fallback in `map_key`. | Modify (Task 5) |
| `crates/tui/src/root.rs` | Mirror the newline mapping in `map_iocraft_key`; size the prompt zone is done in repl.rs, but `handle_live_key` must route `InsertNewline`. | Modify (Task 6) |
| `crates/tui/src/app.rs` | Handle `KeyAction::InsertNewline` in `dispatch` (calls `apply_newline`). | Modify (Task 5) |
| `crates/tui/src/screens/repl.rs` | Pass `prompt_width` to `PromptInput`; mount `PromptInputFooter`; the zone height becomes content-driven (no fixed `height: 1`). | Modify (Tasks 7, 11) |
| `crates/tui/src/state.rs` | (No shape change.) Optionally add `prompt_pending_backslash: bool` IF the keymap fallback needs state — decided in Task 5 to keep it stateless via lookbehind on `prompt_text`. | Possibly modify (Task 5) |

**Decomposition note:** `mod.rs` stays the editor (geometry + component); `footer.rs` is the chrome (mode/hint/placeholder/suggestions). They change for different reasons, so they live in separate files per the writing-plans "split by responsibility" rule. Both stay small.

---

## Task 1: Refactor — move `prompt_input.rs` → `prompt_input/mod.rs` (keep tests green)

**Files:**
- Delete: `crates/tui/src/components/prompt_input.rs`
- Create: `crates/tui/src/components/prompt_input/mod.rs` (verbatim copy of the deleted file)
- Verify (no edit): `crates/tui/src/components/mod.rs:8` (`pub mod prompt_input;` — unchanged, resolves to the new dir's `mod.rs`)

This is a pure structural move. Rust resolves `pub mod prompt_input;` to either `prompt_input.rs` OR `prompt_input/mod.rs`; switching forms requires **zero** changes to `components/mod.rs` or any import path (`crate::components::prompt_input::apply_insert` etc. all still resolve). The six existing unit tests move with the file.

- [ ] **Step 1: Confirm the current import surface (read-only, so you know what must keep resolving)**

Run: `cd lingxi-core && grep -rn "components::prompt_input\|prompt_input::" crates/tui/src crates/tui/tests`
Expected: hits in `app.rs` (`use crate::components::prompt_input::{apply_backspace, apply_insert, apply_move, CursorMove as PiCursor}`), `screens/repl.rs` (`use crate::components::prompt_input::PromptInput`). These paths must NOT change.

- [ ] **Step 2: Move the file (git mv into the new dir)**

```bash
cd lingxi-code/crates/tui/src/components
mkdir prompt_input
git mv prompt_input.rs prompt_input/mod.rs
```

- [ ] **Step 3: Update the module doc-comment header in `mod.rs` to reflect M7-06 (text only, no code change)**

In `crates/tui/src/components/prompt_input/mod.rs`, replace the M6 header lines:

```rust
//! `PromptInput` — the 1-3 row bottom zone.
//!
//! M6-02 supports a single-line editor with:
```

with:

```rust
//! `PromptInput` — the content-driven multi-line bottom zone.
//!
//! M6-02 shipped a single-line editor; M7-06 refactors this file into the
//! `prompt_input/` submodule (`mod.rs` = editor core, `footer.rs` = footer
//! surface) and adds multi-line editing. The single-line behaviour is
//! preserved exactly when the buffer has no `\n`.
//!
//! M6-02 single-line editor supported:
```

- [ ] **Step 4: Run the moved unit tests — they must still pass unchanged**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input`
Expected: PASS — `insert_at_end`, `insert_at_middle`, `backspace_at_zero_is_noop`, `backspace_removes_prev_char`, `move_home_end`, `move_left_right_utf8` (6 tests).

- [ ] **Step 5: Build the whole crate to prove no import broke**

Run: `cd lingxi-core && cargo check -p lingxi-tui --all-targets`
Expected: PASS, no errors. `app.rs` and `repl.rs` still resolve `crate::components::prompt_input::*`.

- [ ] **Step 6: Run the two integration tests that touch the prompt**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test focus_trap_test --test render_placeholder`
Expected: PASS — `dialog_open_prompt_input_not_mutated`, `no_dialog_open_prompt_input_accepts_keys`, the render_placeholder snapshot.

- [ ] **Step 7: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/
git commit -m "$(cat <<'EOF'
plan(M7-06 T1): refactor prompt_input.rs into prompt_input/ submodule (move, tests green)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add unicode deps + line-geometry helper `line_starts`

**Files:**
- Modify: `crates/tui/Cargo.toml` (`[dependencies]`)
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

`mod.rs` will need grapheme-correct and width-aware geometry. The two crates resolve to the exact versions already in `lingxi-code/Cargo.lock` (`unicode-segmentation 1.12.0`, `unicode-width 0.1.14`), so adding them as direct deps does not change the lockfile or hit the network.

- [ ] **Step 1: Add the deps to `crates/tui/Cargo.toml`**

In `[dependencies]`, after the `uuid` line, add:

```toml
unicode-segmentation = "=1.12.0"
unicode-width = "=0.1.14"
```

- [ ] **Step 2: Confirm the lockfile is unchanged (deps were already transitive)**

Run: `cd lingxi-core && cargo check -p lingxi-tui && git diff --stat Cargo.lock`
Expected: `cargo check` PASS; `git diff --stat Cargo.lock` prints nothing (no new lockfile entries — versions already resolved).

- [ ] **Step 3: Write the failing test for `line_starts`**

In `crates/tui/src/components/prompt_input/mod.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn line_starts_single_line() {
    assert_eq!(line_starts("hello"), vec![0]);
    assert_eq!(line_starts(""), vec![0]);
}

#[test]
fn line_starts_multi_line() {
    // "ab\ncd\ne" — line starts at byte 0, 3, 6.
    assert_eq!(line_starts("ab\ncd\ne"), vec![0, 3, 6]);
}

#[test]
fn line_starts_trailing_newline() {
    // A trailing "\n" opens an empty final line.
    assert_eq!(line_starts("ab\n"), vec![0, 3]);
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::line_starts`
Expected: FAIL — `cannot find function line_starts`.

- [ ] **Step 4: Implement `line_starts`**

In `mod.rs` (module body, after `clamp_to_char_boundary`):

```rust
/// Byte indices at which each logical line begins. Always non-empty:
/// `line_starts("")` is `[0]`. A trailing `\n` opens an empty final line.
#[must_use]
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}
```

- [ ] **Step 5: Run the test — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::line_starts`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/Cargo.toml crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T2): add unicode deps + line_starts geometry helper

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `cursor_line_col` — map a byte cursor to (line, column)

**Files:**
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

Vertical motion needs the cursor's logical line index and its column (in graphemes) within that line.

- [ ] **Step 1: Write the failing test**

In `mod tests`:

```rust
#[test]
fn cursor_line_col_basics() {
    // "ab\ncd" — byte 0..2 on line 0; byte 3.. on line 1.
    assert_eq!(cursor_line_col("ab\ncd", 0), (0, 0));
    assert_eq!(cursor_line_col("ab\ncd", 2), (0, 2)); // end of line 0
    assert_eq!(cursor_line_col("ab\ncd", 3), (1, 0)); // start of line 1
    assert_eq!(cursor_line_col("ab\ncd", 5), (1, 2)); // end of line 1
}

#[test]
fn cursor_line_col_grapheme_column() {
    // "é" is 2 bytes but column 1. "éb\nx": byte 3 is after "éb" → col 2.
    assert_eq!(cursor_line_col("éb\nx", 3), (0, 2));
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::cursor_line_col`
Expected: FAIL — `cannot find function cursor_line_col`.

- [ ] **Step 2: Implement `cursor_line_col`**

```rust
use unicode_segmentation::UnicodeSegmentation;

/// Map a byte `cursor` into `(line_index, grapheme_column)`.
/// Column counts grapheme clusters from the line start (not bytes).
#[must_use]
pub fn cursor_line_col(text: &str, cursor: usize) -> (usize, usize) {
    let cursor = clamp_to_char_boundary(text, cursor);
    let starts = line_starts(text);
    // Largest line start <= cursor.
    let line = starts.iter().rposition(|&s| s <= cursor).unwrap_or(0);
    let line_start = starts[line];
    let col = text[line_start..cursor].graphemes(true).count();
    (line, col)
}
```

- [ ] **Step 3: Run the test — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::cursor_line_col`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T3): cursor_line_col byte-to-(line,col) mapping

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `apply_newline` — insert `\n` at the cursor

**Files:**
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

- [ ] **Step 1: Write the failing test**

In `mod tests`:

```rust
#[test]
fn newline_at_end() {
    let (t, c) = apply_newline("hi", 2);
    assert_eq!(t, "hi\n");
    assert_eq!(c, 3);
}

#[test]
fn newline_in_middle_splits_line() {
    let (t, c) = apply_newline("abcd", 2);
    assert_eq!(t, "ab\ncd");
    assert_eq!(c, 3); // cursor after the inserted '\n'
}

#[test]
fn newline_respects_char_boundary() {
    // "é" is 2 bytes; a mid-codepoint cursor saturates left to byte 0.
    let (t, c) = apply_newline("é", 1);
    assert_eq!(t, "\né");
    assert_eq!(c, 1);
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::newline`
Expected: FAIL — `cannot find function apply_newline`.

- [ ] **Step 2: Implement `apply_newline`**

```rust
/// Insert a `\n` at byte index `cursor`, returning `(new_text, new_cursor)`.
/// Mirrors `apply_insert` but for the newline char.
#[must_use]
pub fn apply_newline(text: &str, cursor: usize) -> (String, usize) {
    apply_insert(text, cursor, '\n')
}
```

- [ ] **Step 3: Run the test — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::newline`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T4): apply_newline cursor insert helper

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Wire `KeyAction::InsertNewline` (Shift+Enter + `\`-return) through keymap + dispatch

**Files:**
- Modify: `crates/tui/src/events/keymap.rs`
- Modify: `crates/tui/src/app.rs`

The newline key must produce a new `KeyAction` that `dispatch` turns into `apply_newline`. Shift+Enter is the primary key. The `\`-return fallback is **stateless**: when Enter arrives and `prompt_text` ends with a single `\`, the keymap emits `InsertNewline` AND the dispatcher strips the trailing `\` before inserting `\n`. This avoids new AppState.

- [ ] **Step 1: Write the failing keymap test**

In `crates/tui/src/events/keymap.rs`, in `mod m6_02_tests`, add:

```rust
#[test]
fn shift_enter_maps_to_newline() {
    let evt = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    assert!(matches!(
        map_key(evt, false, false),
        Some(KeyAction::InsertNewline)
    ));
}

#[test]
fn plain_enter_still_submits() {
    assert!(matches!(
        map_key(k(KeyCode::Enter), false, false),
        Some(KeyAction::Submit)
    ));
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib events::keymap::m6_02_tests::shift_enter_maps_to_newline`
Expected: FAIL — no `InsertNewline` variant.

- [ ] **Step 2: Add the `InsertNewline` variant**

In `keymap.rs`, in `enum KeyAction`, after `Submit`:

```rust
    /// Shift+Enter (or the backslash-return fallback) — insert a `\n` at the
    /// prompt cursor instead of submitting. Enter alone always submits.
    InsertNewline,
```

- [ ] **Step 3: Map Shift+Enter in `map_key` (BEFORE the plain-Enter arm)**

In `map_key`, the FIRST arm of the final `match (evt.code, evt.modifiers)` block. Replace:

```rust
        (KeyCode::Enter, _) => Some(Submit),
```

with:

```rust
        (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) => Some(InsertNewline),
        (KeyCode::Enter, _) => Some(Submit),
```

Also add `InsertNewline` to the `use KeyAction::{...}` import list at the top of `map_key`.

- [ ] **Step 4: Run the keymap tests — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib events::keymap`
Expected: PASS — both new tests plus all existing `m6_02_tests` unchanged.

- [ ] **Step 5: Write the failing dispatch test (covers InsertNewline + the `\`-return strip)**

In `crates/tui/src/app.rs`, in `mod dispatch_tests`, add:

```rust
#[test]
fn insert_newline_adds_line() {
    let mut st = s();
    dispatch(KeyAction::InsertChar('a'), &mut st);
    dispatch(KeyAction::InsertNewline, &mut st);
    dispatch(KeyAction::InsertChar('b'), &mut st);
    assert_eq!(st.prompt_text, "a\nb");
    assert_eq!(st.prompt_cursor, 3);
}

#[test]
fn backslash_return_strips_backslash_and_adds_newline() {
    let mut st = s();
    dispatch(KeyAction::InsertChar('a'), &mut st);
    dispatch(KeyAction::InsertChar('\\'), &mut st);
    // Enter with a trailing backslash → keymap emits InsertNewline.
    dispatch(KeyAction::InsertNewline, &mut st);
    assert_eq!(st.prompt_text, "a\n"); // trailing '\' stripped, '\n' inserted
    assert_eq!(st.prompt_cursor, 2);
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib app::dispatch_tests::insert_newline`
Expected: FAIL — `no variant InsertNewline` handled by `dispatch` (non-exhaustive match).

- [ ] **Step 6: Handle `InsertNewline` in `dispatch`**

In `app.rs` `dispatch`, after the `KeyAction::Backspace` arm, add:

```rust
        KeyAction::InsertNewline => {
            // Backslash-return fallback: if the char immediately before the
            // cursor is a lone '\', strip it before inserting the newline so
            // the literal '\' the user typed doesn't linger.
            if st.prompt_cursor > 0
                && st.prompt_text[..st.prompt_cursor].ends_with('\\')
            {
                let (t, cur) = apply_backspace(&st.prompt_text, st.prompt_cursor);
                st.prompt_text = t;
                st.prompt_cursor = cur;
            }
            let (t, cur) = apply_newline(&st.prompt_text, st.prompt_cursor);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
```

Add `apply_newline` to the `use crate::components::prompt_input::{...}` import at the top of `app.rs`.

- [ ] **Step 7: Add the keymap `\`-return fallback (emit InsertNewline on Enter when prompt ends with `\`)**

`map_key`/`map_iocraft_key` don't see `prompt_text`. Rather than thread state, gate the fallback at the dispatcher in `root.rs` and the `keymap::handle_key` path. In `keymap.rs` `handle_key`, the existing fall-through is:

```rust
    if let Some(action) = map_key(
        key,
        state.prompt_text.is_empty(),
        state.focused_tool_id.is_some(),
    ) {
        crate::app::dispatch(action, state)
    } else {
        false
    }
```

Replace the `if let Some(action) = ...` line's body to upgrade a plain-Enter Submit into InsertNewline when a trailing `\` is present:

```rust
    if let Some(mut action) = map_key(
        key,
        state.prompt_text.is_empty(),
        state.focused_tool_id.is_some(),
    ) {
        if matches!(action, KeyAction::Submit)
            && state.prompt_cursor > 0
            && state.prompt_text[..state.prompt_cursor].ends_with('\\')
        {
            action = KeyAction::InsertNewline;
        }
        crate::app::dispatch(action, state)
    } else {
        false
    }
```

- [ ] **Step 8: Run dispatch tests — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib app::dispatch_tests`
Expected: PASS — both new tests plus all existing dispatch tests (`insert_chars_then_backspace`, `submit_clears_prompt_and_pushes_user_message`, etc.) unchanged.

- [ ] **Step 9: Commit**

```bash
cd lingxi-core && git add crates/tui/src/events/keymap.rs crates/tui/src/app.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T5): KeyAction::InsertNewline — Shift+Enter + backslash-return fallback

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Mirror the newline mapping in the live `map_iocraft_key` dispatcher

**Files:**
- Modify: `crates/tui/src/root.rs`

`root.rs` keeps a crossterm-0.29 mirror of `map_key` (the comment at lines 61-66 says it's kept in sync byte-for-byte). The live path must also emit `InsertNewline` for Shift+Enter and apply the `\`-return upgrade so the real binary behaves like the unit-tested keymap.

- [ ] **Step 1: Write the failing root test**

In `crates/tui/src/root.rs`, in `mod tests`, add:

```rust
#[test]
fn map_iocraft_key_shift_enter_inserts_newline() {
    let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
    k.modifiers = KeyModifiers::SHIFT;
    assert!(matches!(
        map_iocraft_key(&k, false, false),
        Some(KeyAction::InsertNewline)
    ));
}

#[test]
fn map_iocraft_key_plain_enter_submits() {
    let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
    assert!(matches!(
        map_iocraft_key(&k, false, false),
        Some(KeyAction::Submit)
    ));
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib root::tests::map_iocraft_key_shift_enter`
Expected: FAIL — Shift+Enter currently maps to `Submit`.

- [ ] **Step 2: Mirror the Shift+Enter arm in `map_iocraft_key`**

In `root.rs` `map_iocraft_key`, replace:

```rust
        (KeyCode::Enter, _) => Some(Submit),
```

with:

```rust
        (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) => Some(InsertNewline),
        (KeyCode::Enter, _) => Some(Submit),
```

Add `InsertNewline` to the `use KeyAction::{...}` import at the top of `map_iocraft_key`.

- [ ] **Step 3: Apply the `\`-return upgrade in `handle_live_key` (non-permission branch)**

In `root.rs` `handle_live_key`, after computing `focus_active` and before the `if let Some(action) = ...`, change:

```rust
    if let Some(action) = map_iocraft_key(k, prompt_empty, focus_active) {
        if let KeyAction::ScrollStep(dir) = action {
            scroll_with_viewport(st, dir, viewport);
        } else {
            let _ = dispatch(action, st);
        }
    }
```

to:

```rust
    if let Some(mut action) = map_iocraft_key(k, prompt_empty, focus_active) {
        if matches!(action, KeyAction::Submit)
            && st.prompt_cursor > 0
            && st.prompt_text[..st.prompt_cursor].ends_with('\\')
        {
            action = KeyAction::InsertNewline;
        }
        if let KeyAction::ScrollStep(dir) = action {
            scroll_with_viewport(st, dir, viewport);
        } else {
            let _ = dispatch(action, st);
        }
    }
```

- [ ] **Step 4: Run root tests — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib root::tests`
Expected: PASS — both new tests plus all existing `root::tests` unchanged. The focus-trap path (priority 1) is untouched: the newline upgrade only runs after the permission branch returns, so §2.5 ordering is preserved.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/src/root.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T6): mirror Shift+Enter newline mapping in live map_iocraft_key + handle_live_key

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `visual_row_count` — content-driven height (with wrap awareness)

**Files:**
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

The prompt zone height is now driven by the number of **visual** rows: logical lines, each possibly wrapped to the available width. The `"> "` (2-col) prompt marker reduces the usable width for the first line; M7-06 keeps a uniform usable width for simplicity and documents the 2-col marker as part of the wrap budget.

- [ ] **Step 1: Write the failing test**

In `mod tests`:

```rust
#[test]
fn visual_rows_single_short_line() {
    // "hi" with the "> " marker, width 80 → 1 row.
    assert_eq!(visual_row_count("hi", 80), 1);
    assert_eq!(visual_row_count("", 80), 1); // empty buffer still 1 row
}

#[test]
fn visual_rows_three_logical_lines() {
    assert_eq!(visual_row_count("a\nb\nc", 80), 3);
}

#[test]
fn visual_rows_wraps_long_line() {
    // 10 graphemes, usable width 4 (after the 2-col "> " marker) → ceil(10/4)=3.
    // width arg is the TOTAL column count; usable = width - 2.
    assert_eq!(visual_row_count("0123456789", 6), 3);
}

#[test]
fn visual_rows_wrap_plus_newline() {
    // "0123456789" wraps to 3 (width 6 → usable 4), then "x" is 1 → 4 total.
    assert_eq!(visual_row_count("0123456789\nx", 6), 4);
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::visual_rows`
Expected: FAIL — `cannot find function visual_row_count`.

- [ ] **Step 2: Implement `visual_row_count`**

```rust
use unicode_width::UnicodeWidthStr;

/// Number of terminal rows the buffer occupies at the given total column
/// `width`, accounting for the 2-column `"> "` marker and soft-wrapping each
/// logical line. Always at least 1 (an empty buffer shows one row).
#[must_use]
pub fn visual_row_count(text: &str, width: usize) -> usize {
    let usable = width.saturating_sub(2).max(1);
    let starts = line_starts(text);
    let mut rows = 0usize;
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(text.len(), |&s| s - 1); // drop the '\n'
        let line = &text[start..end];
        let w = UnicodeWidthStr::width(line);
        // ceil(w / usable), but an empty line is still 1 row.
        rows += if w == 0 { 1 } else { w.div_ceil(usable) };
    }
    rows.max(1)
}
```

Note: `div_ceil` is stable on rust 1.82.

- [ ] **Step 3: Run the test — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::visual_rows`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T7): visual_row_count content-driven height with wrap awareness

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `apply_move_vertical` — cursor up/down across lines

**Files:**
- Modify: `crates/tui/src/components/prompt_input/mod.rs`

Up/Down move the cursor between logical lines, preserving the target column (clamped to the destination line's length), matching standard editor behavior. In M6 the keymap routes Up/Down to history; **M7-06 keeps that** — vertical motion is only meaningful in a multi-line buffer. The decision (locked here): when the buffer has **more than one logical line**, Up/Down move the cursor vertically; when it is single-line, Up/Down keep their M6 history-step behavior. This is gated in Task 9 (the editor helper here is pure and unconditional).

- [ ] **Step 1: Write the failing test**

In `mod tests`:

```rust
#[test]
fn move_down_preserves_column() {
    // "abc\ndef", cursor at line0 col2 (byte 2) → down → line1 col2 (byte 6).
    assert_eq!(apply_move_vertical("abc\ndef", 2, 1), 6);
}

#[test]
fn move_up_preserves_column() {
    // line1 col1 (byte 5) → up → line0 col1 (byte 1).
    assert_eq!(apply_move_vertical("abc\ndef", 5, -1), 1);
}

#[test]
fn move_down_clamps_to_shorter_line() {
    // line0 col3 (byte 3, end of "abc") → down → "de" only has col 0..2 → byte 6 (col2).
    assert_eq!(apply_move_vertical("abc\nde", 3, 1), 6);
}

#[test]
fn move_up_at_top_is_noop_to_target_col_on_line0() {
    // Already on line 0 → up clamps to line 0 (same line), column preserved.
    assert_eq!(apply_move_vertical("abc\ndef", 1, -1), 1);
}

#[test]
fn move_down_at_bottom_is_noop() {
    // Already on last line → down stays (column preserved on same line).
    assert_eq!(apply_move_vertical("abc\ndef", 5, 1), 5);
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::move_down components::prompt_input::tests::move_up`
Expected: FAIL — `cannot find function apply_move_vertical`.

- [ ] **Step 2: Implement `apply_move_vertical`**

```rust
/// Move the cursor `delta` logical lines (`-1` up, `+1` down), preserving the
/// grapheme column (clamped to the destination line). Returns the new byte
/// cursor. At the top/bottom edge the cursor stays on its current line.
#[must_use]
pub fn apply_move_vertical(text: &str, cursor: usize, delta: i32) -> usize {
    let starts = line_starts(text);
    let (line, col) = cursor_line_col(text, cursor);
    let target_line = (line as i32 + delta).clamp(0, starts.len() as i32 - 1) as usize;
    let line_start = starts[target_line];
    let line_end = starts
        .get(target_line + 1)
        .map_or(text.len(), |&s| s - 1); // drop the trailing '\n'
    // Walk `col` graphemes into the destination line, clamping at its end.
    let mut byte = line_start;
    let dest = &text[line_start..line_end];
    for (i, (off, g)) in dest.grapheme_indices(true).enumerate() {
        if i == col {
            byte = line_start + off;
            break;
        }
        byte = line_start + off + g.len();
    }
    byte
}
```

(`grapheme_indices` comes from `UnicodeSegmentation`, already imported in Task 3.)

- [ ] **Step 3: Run the test — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::move_down components::prompt_input::tests::move_up`
Expected: PASS (5 tests).

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T8): apply_move_vertical cursor up/down across lines

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Route Up/Down to vertical motion in a multi-line buffer

**Files:**
- Modify: `crates/tui/src/events/keymap.rs` (`KeyAction` + `map_key`)
- Modify: `crates/tui/src/root.rs` (`map_iocraft_key`)
- Modify: `crates/tui/src/app.rs` (`dispatch`)

Up/Down currently emit `HistoryStep(-1|+1)`. When the buffer spans multiple logical lines, the same keys should move the cursor vertically. We thread the "buffer is multi-line" decision through the existing `prompt_empty` parameter pattern by adding a new `multiline` flag to the mappers (consistent with how `focus_active` is threaded).

- [ ] **Step 1: Write the failing keymap test**

In `keymap.rs` `mod m6_02_tests`:

```rust
#[test]
fn up_down_step_history_in_single_line() {
    // multiline=false → M6 behaviour preserved.
    assert!(matches!(
        map_key_ml(k(KeyCode::Up), false, false, false),
        Some(KeyAction::HistoryStep(-1))
    ));
}

#[test]
fn up_down_move_cursor_in_multiline() {
    assert!(matches!(
        map_key_ml(k(KeyCode::Up), false, false, true),
        Some(KeyAction::MoveCursorVertical(-1))
    ));
    assert!(matches!(
        map_key_ml(k(KeyCode::Down), false, false, true),
        Some(KeyAction::MoveCursorVertical(1))
    ));
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib events::keymap::m6_02_tests::up_down`
Expected: FAIL — `MoveCursorVertical` and `map_key_ml` don't exist.

- [ ] **Step 2: Add the variant + a multiline-aware `map_key_ml`, keep `map_key` as a thin wrapper**

In `keymap.rs`, add to `enum KeyAction` (after `MoveCursor`):

```rust
    /// Move the cursor one logical line up (`-1`) or down (`+1`) in a
    /// multi-line buffer. Single-line buffers route Up/Down to history.
    MoveCursorVertical(i8),
```

Rename the body of `map_key` into a new `map_key_ml(evt, prompt_empty, focus_active, multiline)` and make `map_key` delegate with `multiline = false` so EVERY existing M6 caller and test keeps working unchanged:

```rust
#[must_use]
pub fn map_key(evt: KeyEvent, prompt_empty: bool, focus_active: bool) -> Option<KeyAction> {
    map_key_ml(evt, prompt_empty, focus_active, false)
}
```

Inside `map_key_ml`, before the existing `(KeyCode::Up, _) => Some(HistoryStep(-1))` arms, add:

```rust
        (KeyCode::Up, _) if multiline => Some(MoveCursorVertical(-1)),
        (KeyCode::Down, _) if multiline => Some(MoveCursorVertical(1)),
```

Add `MoveCursorVertical` to the `use KeyAction::{...}` import in `map_key_ml`.

- [ ] **Step 3: Run keymap tests — expect PASS**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib events::keymap`
Expected: PASS — new tests plus ALL existing `m6_02_tests` (they call `map_key`, which now delegates with `multiline=false`).

- [ ] **Step 4: Handle `MoveCursorVertical` in `dispatch` + add a dispatch test**

In `app.rs` `mod dispatch_tests`:

```rust
#[test]
fn move_cursor_vertical_down() {
    let mut st = s();
    st.prompt_text = "abc\ndef".into();
    st.prompt_cursor = 2;
    dispatch(KeyAction::MoveCursorVertical(1), &mut st);
    assert_eq!(st.prompt_cursor, 6);
}
```

In `app.rs` `dispatch`, after the `KeyAction::MoveCursor(m)` arm:

```rust
        KeyAction::MoveCursorVertical(delta) => {
            st.prompt_cursor =
                crate::components::prompt_input::apply_move_vertical(
                    &st.prompt_text,
                    st.prompt_cursor,
                    i32::from(delta),
                );
            false
        }
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib app::dispatch_tests::move_cursor_vertical_down`
Expected: PASS.

- [ ] **Step 5: Thread `multiline` through the live path in `root.rs`**

In `root.rs`, give `map_iocraft_key` the same `multiline` parameter (mirror Step 2's arms) and compute it in `handle_live_key`:

In `handle_live_key`, after `let prompt_empty = st.prompt_text.is_empty();` add:

```rust
    let multiline = st.prompt_text.contains('\n');
```

and change the `map_iocraft_key(k, prompt_empty, focus_active)` call to `map_iocraft_key(k, prompt_empty, focus_active, multiline)`. Update `map_iocraft_key`'s signature + add the two `multiline`-gated Up/Down arms (mirroring Step 2). Handle `MoveCursorVertical` is already routed via `dispatch` (it's not a `ScrollStep`, so it falls into the `dispatch(action, st)` branch — no extra root code needed).

Add a root test:

```rust
#[test]
fn map_iocraft_key_up_is_vertical_when_multiline() {
    let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Up);
    assert!(matches!(
        map_iocraft_key(&k, false, false, true),
        Some(KeyAction::MoveCursorVertical(-1))
    ));
}
```

Update the three existing `map_iocraft_key(...)` calls in `root::tests` to pass a fourth `false` argument (they assert single-line behavior). Run:
`cd lingxi-core && cargo test -p lingxi-tui --lib root::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/events/keymap.rs crates/tui/src/root.rs crates/tui/src/app.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T9): route Up/Down to vertical cursor motion in multi-line buffers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `footer.rs` — footer surface (mode-indicator, placeholder, help hint, suggestions placeholder)

**Files:**
- Create: `crates/tui/src/components/prompt_input/footer.rs`
- Modify: `crates/tui/src/components/prompt_input/mod.rs` (add `pub mod footer; pub use footer::*;`)

Pure render component. Literal-locked strings per the References block. The suggestions area is an empty placeholder (live filtering is M7-07). Snapshot-tested.

- [ ] **Step 1: Create `footer.rs` with the component + props**

```rust
//! `PromptInputFooter` — the chrome below the editor: mode-indicator,
//! placeholder, the collapsed help hint, the newline hint, and a (M7-06
//! empty) suggestions area. Live suggestion filtering arrives in M7-07.
//!
//! Literal lock (claude-code PromptInput*): the prompt glyph is `❯ `
//! (figures.pointer + space), the newline hint is `shift + ⏎ for newline`,
//! and the help hint is `? for shortcuts`.

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Which leading glyph the mode-indicator shows. M7-06 ships `Prompt`; the
/// `Bash` / vim-mode variants land in M7-07/M7-08.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FooterMode {
    /// Normal prompt mode — glyph `❯ `.
    #[default]
    Prompt,
    /// Bash mode — glyph `! ` (placeholder for M7-07; not yet keyboard-wired).
    Bash,
}

impl FooterMode {
    /// The leading glyph (claude-code PromptInputModeIndicator.tsx).
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            FooterMode::Prompt => "❯ ",
            FooterMode::Bash => "! ",
        }
    }
}

/// Props for [`PromptInputFooter`].
#[derive(Default, Props)]
pub struct PromptInputFooterProps {
    /// Mode-indicator glyph selector.
    pub mode: FooterMode,
    /// Placeholder shown when the buffer is empty (None = no placeholder).
    pub placeholder: Option<String>,
    /// Whether the buffer is currently empty (drives placeholder visibility).
    pub is_empty: bool,
}

/// Render the footer: `[glyph][placeholder?]` row + a dim hint row
/// (`? for shortcuts     shift + ⏎ for newline`).
#[component]
pub fn PromptInputFooter(props: &PromptInputFooterProps) -> impl Into<AnyElement<'static>> {
    let glyph = props.mode.glyph().to_string();
    let placeholder = if props.is_empty {
        props.placeholder.clone().unwrap_or_default()
    } else {
        String::new()
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            View(flex_direction: FlexDirection::Row) {
                Text(content: glyph, color: TuiTheme::DIM)
                Text(content: placeholder, color: TuiTheme::DIM)
            }
            View(flex_direction: FlexDirection::Row, gap: 5) {
                Text(content: "? for shortcuts".to_string(), color: TuiTheme::DIM)
                Text(content: "shift + ⏎ for newline".to_string(), color: TuiTheme::DIM)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_prompt_is_pointer() {
        assert_eq!(FooterMode::Prompt.glyph(), "❯ ");
    }

    #[test]
    fn glyph_bash_is_bang() {
        assert_eq!(FooterMode::Bash.glyph(), "! ");
    }
}
```

- [ ] **Step 2: Wire the module in `mod.rs`**

At the top of `crates/tui/src/components/prompt_input/mod.rs` (after the doc-comment, before the helpers), add:

```rust
pub mod footer;
pub use footer::{FooterMode, PromptInputFooter, PromptInputFooterProps};
```

- [ ] **Step 3: Run the footer unit tests + build**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::footer`
Expected: PASS (2 tests).
Run: `cd lingxi-core && cargo check -p lingxi-tui --all-targets`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/
git commit -m "$(cat <<'EOF'
plan(M7-06 T10): footer.rs — mode-indicator, placeholder, help + newline hints

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Make `PromptInput` render N rows + mount footer in the REPL screen

**Files:**
- Modify: `crates/tui/src/components/prompt_input/mod.rs` (the `PromptInput` component + `PromptInputProps`)
- Modify: `crates/tui/src/screens/repl.rs` (mount footer; pass width; content-driven height)

The component currently emits one `View(height: 1)` with `"> {text}"`. Now it renders one `Text` per logical line (with the `"> "` marker only on the first), and its outer `View` height grows with `visual_row_count`. The REPL screen passes the live `prompt_width` (= `cols`) and mounts `PromptInputFooter` below the editor.

- [ ] **Step 1: Extend `PromptInputProps` with `width`**

In `mod.rs`, change `PromptInputProps`:

```rust
#[derive(Default, Props)]
pub struct PromptInputProps {
    /// Current text in the prompt buffer (may contain `\n`).
    pub text: String,
    /// Byte-index cursor position (always at a char boundary).
    pub cursor: usize,
    /// Total terminal column width (drives wrap + height). 0 → treat as 80.
    pub width: usize,
}
```

- [ ] **Step 2: Rewrite the `PromptInput` component to render per-line rows**

Replace the M6 `PromptInput` body:

```rust
#[component]
pub fn PromptInput(props: &PromptInputProps) -> impl Into<AnyElement<'static>> {
    let _ = props.cursor; // cursor glyph rendering remains an M7-08 (vim) enhancement
    let width = if props.width == 0 { 80 } else { props.width };
    let starts = line_starts(&props.text);
    let height = visual_row_count(&props.text, width);
    // One Text per logical line; first line carries the "> " marker, the rest
    // are indented by 2 columns to align under it.
    let lines: Vec<(usize, String)> = starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).map_or(props.text.len(), |&s| s - 1);
            (i, props.text[start..end].to_string())
        })
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column, height: height as u16) {
            #(lines.into_iter().map(|(i, content)| {
                let display = if i == 0 {
                    format!("> {content}")
                } else {
                    format!("  {content}")
                };
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: display)
                    }
                }
            }))
        }
    }
}
```

- [ ] **Step 3: Write a render smoke test for multi-line (3 logical lines → 3 rows)**

In `mod tests`:

```rust
#[test]
fn prompt_input_renders_three_lines() {
    let mut el = element! { PromptInput(text: "a\nb\nc".to_string(), cursor: 0, width: 80) };
    let out = el.to_string();
    assert!(out.contains("> a"), "got: {out}");
    assert!(out.contains("  b"), "got: {out}");
    assert!(out.contains("  c"), "got: {out}");
}

#[test]
fn prompt_input_single_line_unchanged() {
    let mut el = element! { PromptInput(text: "hi".to_string(), cursor: 0, width: 80) };
    let out = el.to_string();
    assert!(out.contains("> hi"), "got: {out}");
}
```

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib components::prompt_input::tests::prompt_input`
Expected: FAIL first (component signature changed — `width` arg required), then PASS after Steps 1-2 compile.

- [ ] **Step 4: Mount the footer + pass width in `repl.rs`**

In `crates/tui/src/screens/repl.rs`:
- Add to `ReplScreenProps`: `pub prompt_width: usize,`.
- Update the `use` line to also import the footer:
  `use crate::components::prompt_input::{PromptInput, PromptInputFooter};`
- In the `ReplScreen` body, bind `let prompt_width = props.prompt_width;` and `let prompt_is_empty = props.prompt_text.is_empty();`.
- Replace the `PromptInput(text: prompt_text, cursor: prompt_cursor,)` mount with:

```rust
            PromptInput(
                text: prompt_text,
                cursor: prompt_cursor,
                width: prompt_width,
            )
            PromptInputFooter(
                mode: crate::components::prompt_input::FooterMode::Prompt,
                placeholder: None,
                is_empty: prompt_is_empty,
            )
```

- In `app.rs::render_screen`, pass `prompt_width: <cols>`. `render_screen` currently takes only `viewport_height`; add a `prompt_width: usize` parameter and thread `cols` from `root.rs`'s render path (where `(cols, rows) = hooks.use_terminal_size()` is already available). Update the `render_screen` call in `root.rs` to `crate::app::render_screen(&st, viewport, usize::from(cols))` and the signature `pub fn render_screen(state: &AppState, viewport_height: usize, prompt_width: usize)`. Update the `render_screen_smoke` test in `app.rs` to pass a third arg (e.g. `render_screen(&st, 5, 80)`).

- [ ] **Step 5: Run the affected tests + full crate build**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib`
Expected: PASS — multi-line render tests, `render_screen_smoke` (now `render_screen(&st, 5, 80)`), all dispatch/keymap/root tests.
Run: `cd lingxi-core && cargo test -p lingxi-tui --test focus_trap_test --test render_placeholder`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/mod.rs crates/tui/src/screens/repl.rs crates/tui/src/app.rs crates/tui/src/root.rs
git commit -m "$(cat <<'EOF'
plan(M7-06 T11): content-driven multi-row PromptInput + mounted footer in REPL screen

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Snapshots (footer + 3-line input) + workspace gate + tag `m7.6`

**Files:**
- Create: `crates/tui/tests/prompt_input_footer_snapshot.rs`
- Create snapshots: `crates/tui/tests/snapshots/*.snap` (insta-generated, accepted)

Two snapshots per §5.2: the footer (mode-indicator + help hint + newline hint, with a placeholder), and a 3-line multi-line input. Then the full workspace verification gate (run from inside `lingxi-code/`) and the annotated tag.

- [ ] **Step 1: Write the footer snapshot test**

`crates/tui/tests/prompt_input_footer_snapshot.rs`:

```rust
//! M7-06 snapshots: the PromptInput footer surface and a 3-line input.
use iocraft::prelude::*;
use lingxi_tui::components::prompt_input::{
    FooterMode, PromptInput, PromptInputFooter,
};

#[test]
fn footer_prompt_mode_with_placeholder() {
    let mut el = element! {
        PromptInputFooter(
            mode: FooterMode::Prompt,
            placeholder: Some("Try \"edit <filepath> to...\"".to_string()),
            is_empty: true,
        )
    };
    insta::assert_snapshot!("footer_prompt_mode_with_placeholder", el.to_string());
}

#[test]
fn input_three_lines() {
    let mut el = element! {
        PromptInput(text: "first\nsecond\nthird".to_string(), cursor: 0, width: 40)
    };
    insta::assert_snapshot!("input_three_lines", el.to_string());
}
```

Confirm `lingxi_tui::components::prompt_input` re-exports `PromptInput`, `PromptInputFooter`, `FooterMode` (Task 10 Step 2 added the `pub use`). If `components` is not `pub` at the crate root, it is (`lib.rs:14 pub mod components;`).

- [ ] **Step 2: Run to generate snapshots, then accept them**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test prompt_input_footer_snapshot`
Expected: FAIL (new snapshots pending).
Run: `cd lingxi-core && cargo insta accept` (or review with `cargo insta review` then accept).
Re-run: `cd lingxi-core && cargo test -p lingxi-tui --test prompt_input_footer_snapshot`
Expected: PASS. Inspect the two `.snap` files: footer shows `❯`, `? for shortcuts`, `shift + ⏎ for newline`, and the placeholder; the input shows `> first` / `  second` / `  third`.

- [ ] **Step 3: Format + clippy (from inside lingxi-core)**

Run: `cd lingxi-core && cargo fmt --check`
Expected: PASS (no diff).
Run: `cd lingxi-core && cargo clippy -p lingxi-tui --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 4: Full workspace test gate**

Run: `cd lingxi-core && cargo test --workspace`
Expected: PASS. Known flakes are allowed a rerun: `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, and `lingxi-platform-posix` fs_watch FSEvents timing tests. Re-run only the flaky test if one trips: `cargo test -p <crate> <test_name>`.

- [ ] **Step 5: Telemetry baseline check — M7-06 adds 0 events**

Run: `cd lingxi-core && cargo test --workspace event 2>&1 | rg -i "326|event_names|ALL_EVENT" | head`
Expected: the event-count assertion test still passes at **326** (M7-06 registers no new telemetry names). If a count test exists (e.g. `all_event_names_count`), it must read 326 unchanged.

- [ ] **Step 6: Cross-platform compile gate (5 targets, same posture as v0.7.0)**

Run:
```bash
cd lingxi-core && for t in x86_64-unknown-linux-gnu x86_64-apple-darwin x86_64-pc-windows-gnu aarch64-linux-android aarch64-apple-ios; do
  cargo check --workspace --target "$t" || echo "FAILED: $t";
done
```
Expected: each target checks clean (toolchains assumed installed per the v0.7.0 gate). If a target's toolchain is unavailable in this environment, note it and proceed (the gate is enforced at M7-16 release; per-sub-plan it is best-effort, matching M6).

- [ ] **Step 7: Commit the snapshots**

```bash
cd lingxi-core && git add crates/tui/tests/prompt_input_footer_snapshot.rs crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-06 T12): footer + 3-line input snapshots; workspace gate green

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 8: Tag `m7.6` (annotated, local only — no push)**

```bash
cd lingxi-core && git tag -a m7.6 -m "M7-06: PromptInput multi-line + footer"
git tag -l m7.6
```
Expected: `m7.6` listed. Do NOT push.

---

## Self-Review (run after writing the plan)

**Spec coverage (against §3 "M7-06" + §2.1 + §2.5):**
- Refactor `prompt_input.rs` → `prompt_input/` submodule → Task 1 (move) + Task 10 (footer.rs sibling). ✓
- Multi-line buffer: newline insert → Tasks 4-6; cursor up/down across lines → Tasks 3, 8, 9; wrap awareness + height 1→N → Tasks 7, 11. ✓
- `footer.rs` (suggestions / help-menu hint / mode-indicator / placeholder) → Task 10 (suggestions = empty placeholder, documented as M7-07 territory). ✓
- Single-line behavior preserved → Tasks 1 (verbatim move + green tests), 5 (Enter still submits), 9 (`map_key` delegates `multiline=false`), 11 (`> hi` single-line render test). ✓
- §2.5 live-key priority unchanged → Task 6/9 keep the focus-trap branch first; newline + vertical motion slot into priority 4 (input widget). ✓
- Telemetry +0 (baseline 326) → Task 12 Step 5. ✓
- Tests: §5.2 budget = 2 snapshots + 6+ behavior. Behavior: line_starts(3) + cursor_line_col(2) + newline(3) + keymap shift-enter(2) + dispatch newline(2) + root mirror(2) + visual_rows(4) + vertical(5) + keymap up/down(2) + dispatch vertical(1) + footer glyph(2) + render(2) = well over 6. Snapshots: footer + 3-line input = 2. ✓

**Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N" — every code step shows full code. The `? for shortcuts` literal carries an explicit fallback note (verify-at-impl, with documented alternative). ✓

**Type consistency:** `KeyAction::InsertNewline` (T5) + `KeyAction::MoveCursorVertical(i8)` (T9) used consistently in keymap/root/dispatch. `apply_newline` (T4) called in T5 dispatch. `apply_move_vertical(text, cursor, i32)` (T8) called in T9 dispatch with `i32::from(delta)`. `visual_row_count(text, width)` (T7) used in T11. `line_starts` (T2) used by T3/T7/T8/T11. `FooterMode`/`PromptInputFooter`/`PromptInputFooterProps` (T10) used in T11/T12. `render_screen` gains a third `prompt_width` arg (T11) — all call sites updated (root.rs live path, app.rs `render_screen_smoke` test). ✓

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-29-m7-06-promptinput-multiline.md`. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
