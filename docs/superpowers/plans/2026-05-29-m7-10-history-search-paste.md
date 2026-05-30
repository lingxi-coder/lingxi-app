# LingXi Core M7 · Plan 10 · History search + image paste

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every implementation task ends with its own commit. The verification gate (final task) is the workspace-wide guard. Run all `cargo` commands **from inside `lingxi-code/`** (the toolchain pins rust 1.82.0 there; running from the repo root uses the host toolchain and produces spurious lint noise — this bit M6-08).

**Goal:** Add two new files to the `prompt_input/` submodule (created by M7-06): `history_search.rs` (Ctrl-R reverse incremental search over prompt history) and `image_paste.rs` (bracketed-paste coalescing + image-on-paste detection that inserts an `[Image #N]` placeholder ref). History search is an **input overlay** routed at priority 3 of the single `handle_live_key` dispatcher (§2.5): while active, every key goes to the search, not normal editing; Ctrl-R opens it; typing filters prompt history (most-recent match first); Ctrl-R again cycles to the next older match; Enter accepts the match into the prompt; Esc restores the pre-search prompt. Bracketed paste is captured as one unit — a multi-line paste inserts as a single block and does **not** trigger per-line submit. Image paste is **detection + ref insertion only** (NO inline terminal image display — that is M8, §4 R8): a paste payload that is an image file path (or the kitty/iTerm2 inline-image escape) records attachment metadata and inserts a `[Image #N]` placeholder into the prompt, incrementing the counter per image. Telemetry adds **0** events (baseline stays 326; the `tengu_tui_search_opened` candidate is deferred to the M7-16 audit — noted below).

**Architecture:** Both features are pure-function cores plus thin wiring, mirroring the M6/M7-06/M7-08 split (pure `(state, input) -> outcome` helpers, snapshot/behavior-tested, with the live mount calling the same functions the tests call). History search lives in `AppState` as `history_search: Option<HistorySearchState>` — `Some(_)` means the overlay owns keys. The pure core (`hs_open`, `hs_push_char`, `hs_backspace`, `hs_cycle`, `hs_accept`, `hs_cancel`, `recompute_match`) computes the current match index against `AppState.history` (the existing M6 store: `Vec<String>`, most-recent at end) without touching the prompt until accept. `handle_live_key` gains a priority-3 branch: when `history_search.is_some()`, route the key to `handle_history_search_key` and return early — exactly the focus-trap discipline M6-05 established for permission dialogs. Bracketed paste is the subtle part: **iocraft 0.8.3 has no paste event** (see "Critical iocraft fact" below), so a paste arrives as a rapid burst of individual `TerminalEvent::Key` events. M7-10 introduces a **paste coalescer** in `image_paste.rs`: a `PasteCoalescer` that buffers printable chars + newline keys when they arrive within a short window (a "burst"), flushing them as one block on a quiet tick or on a non-printable key — so multi-line paste inserts atomically and an embedded `Enter` does NOT submit. The coalescer's flush calls `process_paste(text, &mut state) -> PasteOutcome`, which classifies each line via `is_image_path` (claude-code `IMAGE_EXTENSION_REGEX`), records `Attachment` metadata, and builds the insertion string (`[Image #N]` for image lines, literal text otherwise). No engine wiring, no AppState shape change beyond the two new fields; the single `handle_live_key` dispatcher (§2.5) is preserved.

**Tech Stack:** Rust 2021, edition/rust-version from workspace (rust 1.82). `iocraft = "=0.8.3"` (`View`, not `Box`; `Text`; crossterm-0.29 re-exports). New deps: **none** — `image_paste.rs` matches extensions with a hand-written suffix check (no `regex` crate added; the claude-code pattern is a fixed alternation). `insta = "1.40"` (yaml) for the overlay snapshot, already a dev-dependency. The editor-core pure helpers (`apply_insert`, `apply_move`, `CursorMove`, `PromptInput`, `PromptInputProps`) from `prompt_input/mod.rs` (M7-06) are reused for the accept-into-prompt path. No `StyledLine` dependency.

**References:**

- Parent design spec: `docs/superpowers/specs/2026-05-29-m7-tui-surface-design.md` — §1 (goal/non-goals; "image paste does detection + ref insertion only"), §2.5 (live-key routing priority — history search is an input overlay at priority 3), §4 R8 (image-paste scoped to detection + ref insertion; inline display → M8), §3 "M7-10" entry (lines 224-227), §5.2 (test budget: 1 snapshot + 6+ behavior).
- Predecessor code (must stay green):
  - `crates/tui/src/components/prompt_input/mod.rs` — editor core after the M7-06 refactor. Pure helpers `apply_insert(text, cursor, ch) -> (String, usize)`, `apply_backspace`, `apply_move(text, cursor, CursorMove) -> usize`, enum `CursorMove {Left,Right,Home,End}`, the `PromptInput` component + `PromptInputProps {text, cursor}`. **Reused** for the accept path and for inserting a coalesced paste block.
  - `crates/tui/src/components/prompt_input/footer.rs` — `PromptInputFooter` (M7-06). Not modified by M7-10 except the optional overlay-line note (the search prompt renders as its own row above the input, mirroring claude-code's `HistorySearchInput` Box).
  - `crates/tui/src/events/keymap.rs` — `KeyAction` enum, `map_key(evt, prompt_empty, focus_active)`, `handle_key` (crossterm-0.28 dispatcher). M7-10 adds **no** new `KeyAction` (history search and paste are handled inside `handle_live_key`, not via `dispatch`, because they own keys / coalesce bursts rather than producing one action per key).
  - `crates/tui/src/root.rs` — `handle_live_key(st, k, viewport)` (THE single live-key dispatcher), `map_iocraft_key`, `iocraft_to_crossterm028_key`, `viewport_height(rows) = rows - 3`, and the `use_terminal_events` closure (`root.rs:301-315`). M7-10 adds the priority-3 history-search branch and the paste-coalescer plumbing here.
  - `crates/tui/src/state.rs` — `AppState.history: Vec<String>` (most-recent at end, `state.rs:194`), `history_cursor: Option<usize>`, `prompt_text: String`, `prompt_cursor: usize` (byte index, always at a char boundary). M7-10 adds `history_search: Option<HistorySearchState>` and `paste: PasteState` (attachment registry + next-id counter).
  - `crates/tui/src/app.rs` — `dispatch(action, st)`, `render_screen`. M7-10 does not add a `KeyAction`, so `dispatch` is unchanged; the submit path (`app.rs:119-120`, pushes to history, clears cursor) is the place a future audit could expand pasted-ref placeholders, but **M7-10 does not** — see "Scope: ref insertion only" below.
- claude-code byte-locks (verified by direct read at plan-writing time — literal-lock per §2.8):
  - `claude-code/src/components/PromptInput/HistorySearchInput.tsx:18` — the search prompt label is **`search prompts:`** normally and **`no matching prompt:`** when `historyFailedMatch` is true (`historyFailedMatch ? 'no matching prompt:' : 'search prompts:'`). The label is dim-colored and the query is rendered after it with a one-space gap (`<Box gap={1}>`). M7-10 ships these two literals exactly.
  - `claude-code/src/history.ts:58-59` — `formatImageRef(id) => `[Image #${id}]``. M7-10's `format_image_ref(n)` returns `"[Image #N]"` byte-for-byte (a leading `[Image #`, the integer, a trailing `]`).
  - `claude-code/src/history.ts:47-55` — `getPastedTextRefNumLines(text)` counts `\r\n|\r|\n` occurrences; `formatPastedTextRef(id, numLines)` returns `[Pasted text #${id}]` when 0 lines else `[Pasted text #${id} +${numLines} lines]`. **M7-10 does NOT implement the large-text-paste truncation ref** (`[Pasted text #N ...]`) — that path (`maybeTruncateInput`, 10k-char threshold) is out of scope; M7-10's paste inserts the literal pasted text inline and only swaps **image** lines for `[Image #N]`. The text-ref format is documented here only so M7-16/M8 can add truncation later without re-reading.
  - `claude-code/src/utils/imagePaste.ts:270` — `IMAGE_EXTENSION_REGEX = /\.(png|jpe?g|gif|webp)$/i`. M7-10's `is_image_path` matches a case-insensitive trailing `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp` after stripping surrounding single/double quotes and trimming.
  - `claude-code/src/utils/imagePaste.ts:277-344` — `removeOuterQuotes` (strip matching outer `'`/`"`), `isImageFilePath`/`asImageFilePath` (trim → unquote → ext test). M7-10 ports the trim + unquote + extension check; it does **not** port `stripBackslashEscapes` (shell-escape unescaping) — that is a refinement noted for M8, and the M7-10 detection treats a path with literal backslashes as a non-image text line (safe degrade, never a false positive).
  - `claude-code/src/hooks/usePasteHandler.ts:113-130,236-258` — paste chunks are joined, orphan focus sequences (`[I$`, `[O$`) stripped, then split on `/\n/` and on space-before-absolute-path (` /` or ` C:\`) to find image paths; `isFromPaste` (bracketed-paste flag) and the >800-char threshold (`PASTE_THRESHOLD`) gate the paste path. M7-10 reuses the **newline split + per-line image classification**; it relies on the **burst coalescer** (below) instead of node's `isPasted` flag because iocraft surfaces no such flag.

**Critical iocraft fact (verified — drives the whole paste design):** iocraft 0.8.3's `TerminalEvent` enum (`~/.cargo/registry/.../iocraft-0.8.3/src/terminal.rs:81`) has **only** `Key(KeyEvent)`, `FullscreenMouse(...)`, `Resize(u16,u16)` — there is **no `Paste` variant**. Its `event_stream` (`terminal.rs:363-383`) matches `Event::Key` / `Event::Mouse` / `Event::Resize` and drops everything else (`_ => None`), and it never sends `EnableBracketedPaste`. So even though the underlying crossterm 0.29 supports `Event::Paste`, iocraft **neither enables bracketed paste nor forwards a paste event**. Consequence: a pasted multi-line block reaches `handle_live_key` as a rapid burst of individual `TerminalEvent::Key` events — one per char, with `KeyCode::Enter` for each newline. M7-10 therefore detects paste **heuristically by burst coalescing** (buffer chars arriving inside a short inter-key window; flush as one block on a quiet tick or a non-printable key). This is implemented entirely in `lingxi-tui` — **no iocraft patch** — and the pure coalescer logic is unit-testable by feeding it a timestamped key sequence. **To verify at execution time:** re-read `terminal.rs:81` + `:363-383` to confirm the enum/`event_stream` are unchanged in the locally-resolved 0.8.3, and confirm `use_terminal_events`'s callback signature still hands you one `TerminalEvent` per call (`root.rs:301`).

**Scope: ref insertion only (LOCKED).** Per §4 R8, M7-10 ships **detection + reference insertion**: on an image paste it (1) records `Attachment { id, kind: Image, source }` in `AppState.paste.attachments` and (2) inserts the `[Image #N]` literal at the cursor. It does **NOT** read image bytes, base64-encode, resize, send to the API, or display the image inline. The recorded metadata is the source path (or `"clipboard"` for the empty-payload macOS case, detected but not read). Wiring attachments into the outgoing turn and inline display are M8. This keeps M7-10 a pure surface change with zero engine touch.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/tui/src/components/prompt_input/history_search.rs` | `HistorySearchState` + pure core (`hs_open`, `hs_push_char`, `hs_backspace`, `hs_cycle`, `hs_accept`, `hs_cancel`, `recompute_match`) + `HistorySearchOverlay` render component + the two literals. | Create (Tasks 1-4) |
| `crates/tui/src/components/prompt_input/image_paste.rs` | `is_image_path`, `format_image_ref`, `classify_paste_line`, `process_paste` (build insertion + attachment list), `PasteCoalescer` (burst buffer + flush decision), `Attachment`/`AttachmentKind`/`PasteState`/`PasteOutcome` types. | Create (Tasks 5-9) |
| `crates/tui/src/components/prompt_input/mod.rs` | Add `pub mod history_search;` + `pub mod image_paste;` and re-export the public types. | Modify (Task 1, Task 5) |
| `crates/tui/src/state.rs` | Add `history_search: Option<HistorySearchState>` and `paste: PasteState` fields + their `Default`/`new` init. | Modify (Task 2, Task 5) |
| `crates/tui/src/root.rs` | `handle_live_key`: priority-3 branch routing keys to `handle_history_search_key` when `history_search.is_some()`; Ctrl-R open binding; paste-coalescer plumbing in the `use_terminal_events` closure + a quiet-tick flush. | Modify (Tasks 3, 4, 10) |
| `crates/tui/src/screens/repl.rs` | Mount `HistorySearchOverlay` as a row above the input when `history_search.is_some()`. | Modify (Task 4) |
| `crates/tui/tests/history_search_test.rs` | Behavior tests: open/filter/cycle/accept/restore/empty/no-match/focus-capture. | Create (Task 4) |
| `crates/tui/tests/image_paste_test.rs` | Behavior tests: multi-line paste single block / no per-line submit; image ref + metadata; counter increments. | Create (Task 9, Task 10) |

**Decomposition note:** `history_search.rs` and `image_paste.rs` change for entirely different reasons (search state machine vs. paste classification/coalescing), so they are separate files per the writing-plans "split by responsibility" rule. Both have a pure core + a thin component/wiring layer, matching the established M7 pattern. Neither file exceeds ~250 lines.

---

## Task 1: Scaffold `history_search.rs` + `HistorySearchState`

**Files:**
- Create: `crates/tui/src/components/prompt_input/history_search.rs`
- Modify: `crates/tui/src/components/prompt_input/mod.rs` (add `pub mod history_search;` + re-exports)

The search state holds the query, the resolved match index into `AppState.history`, the `no-match` flag, and the prompt snapshot to restore on cancel. The match index is `None` when no history line contains the query.

- [ ] **Step 1: Write the failing test (pure state construction + restore snapshot)**

In `crates/tui/src/components/prompt_input/history_search.rs`:

```rust
//! Ctrl-R reverse incremental search over the prompt history.
//!
//! An *input overlay* (parent spec §2.5 priority 3): while `Some(_)` in
//! `AppState.history_search`, every key is captured by the search, not the
//! normal editor. Opening Ctrl-R snapshots the current prompt so Esc can
//! restore it; typing filters `AppState.history` (most-recent match first);
//! Ctrl-R again cycles to the next older match; Enter accepts the match into
//! the prompt; Esc cancels.
//!
//! Pure core: `hs_*` functions take `(&HistorySearchState, &[String], ...)`
//! and return a new state (or, for accept/cancel, the resulting prompt text).
//! The live mount in `root.rs` calls the same functions the tests call.

/// Search-overlay state. `Some(_)` in `AppState.history_search` means the
/// overlay owns all keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySearchState {
    /// The live query the user is typing.
    pub query: String,
    /// Index into `AppState.history` of the current match, or `None` when no
    /// history line contains `query` (drives the `no matching prompt:` label).
    pub match_index: Option<usize>,
    /// The prompt text at the moment search opened — restored on Esc.
    pub saved_prompt: String,
    /// The prompt cursor at the moment search opened — restored on Esc.
    pub saved_cursor: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_snapshots_prompt_and_starts_empty() {
        let st = hs_open("draft text", 4);
        assert_eq!(st.query, "");
        assert_eq!(st.match_index, None);
        assert_eq!(st.saved_prompt, "draft text");
        assert_eq!(st.saved_cursor, 4);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib history_search::tests::open_snapshots_prompt_and_starts_empty`
Expected: FAIL — `cannot find function `hs_open``.

- [ ] **Step 3: Implement `hs_open`**

Add to `history_search.rs`:

```rust
/// Open the overlay, snapshotting the current prompt for Esc-restore.
#[must_use]
pub fn hs_open(prompt: &str, cursor: usize) -> HistorySearchState {
    HistorySearchState {
        query: String::new(),
        match_index: None,
        saved_prompt: prompt.to_string(),
        saved_cursor: cursor,
    }
}
```

- [ ] **Step 4: Wire the module into the submodule**

In `crates/tui/src/components/prompt_input/mod.rs`, after the existing items, add:

```rust
pub mod history_search;
pub use history_search::{hs_open, HistorySearchState};
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib history_search::tests::open_snapshots_prompt_and_starts_empty`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/history_search.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T1): scaffold history_search.rs + HistorySearchState + hs_open

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `recompute_match` + `hs_push_char` + `hs_backspace` (most-recent-first filter)

**Files:**
- Modify: `crates/tui/src/components/prompt_input/history_search.rs`
- Modify: `crates/tui/src/state.rs` (add the `history_search` field)

Filtering walks `AppState.history` from the newest entry (end of the `Vec`) toward the oldest and returns the first index whose entry contains the query as a substring. Empty query → no match (`None`), matching claude-code's "search prompts:" idle state.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod in `history_search.rs`:

```rust
    fn hist() -> Vec<String> {
        // oldest .. newest (newest at the end, matching AppState.history).
        vec![
            "git status".to_string(),
            "cargo test".to_string(),
            "git commit -m wip".to_string(),
            "cargo build".to_string(),
        ]
    }

    #[test]
    fn recompute_finds_most_recent_match() {
        let h = hist();
        // "cargo" appears at idx 1 and idx 3; newest-first → idx 3.
        assert_eq!(recompute_match(&h, "cargo", None), Some(3));
        // "git" appears at idx 0 and idx 2; newest-first → idx 2.
        assert_eq!(recompute_match(&h, "git", None), Some(2));
        // empty query → no match.
        assert_eq!(recompute_match(&h, "", None), None);
        // no substring → no match.
        assert_eq!(recompute_match(&h, "zzz", None), None);
    }

    #[test]
    fn push_char_updates_query_and_match() {
        let h = hist();
        let mut st = hs_open("", 0);
        st = hs_push_char(st, 'g', &h);
        assert_eq!(st.query, "g");
        assert_eq!(st.match_index, Some(2)); // newest "git commit"
        st = hs_push_char(st, 'i', &h); // "gi"
        st = hs_push_char(st, 't', &h); // "git"
        assert_eq!(st.match_index, Some(2));
    }

    #[test]
    fn backspace_widens_match_and_clears_to_empty() {
        let h = hist();
        let mut st = hs_open("", 0);
        st = hs_push_char(st, 'z', &h); // no match
        assert_eq!(st.match_index, None);
        st = hs_backspace(st, &h); // query empty again
        assert_eq!(st.query, "");
        assert_eq!(st.match_index, None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib history_search::tests`
Expected: FAIL — `recompute_match`, `hs_push_char`, `hs_backspace` undefined.

- [ ] **Step 3: Implement the three functions**

Add to `history_search.rs`:

```rust
/// Find the newest (`from`-exclusive, walking toward older) history index
/// whose entry contains `query`. `start_below = Some(i)` restricts the search
/// to indices strictly below `i` (used by `hs_cycle` to step to an older
/// match); `None` searches all entries newest-first. Empty `query` → `None`.
#[must_use]
pub fn recompute_match(history: &[String], query: &str, start_below: Option<usize>) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    let upper = match start_below {
        Some(0) => return None,           // nothing older than index 0
        Some(i) => i,                     // search indices [0, i)
        None => history.len(),            // search all, newest-first
    };
    history[..upper]
        .iter()
        .enumerate()
        .rev() // newest-first within the window
        .find(|(_, entry)| entry.contains(query))
        .map(|(i, _)| i)
}

/// Append `ch` to the query and recompute the match from the newest entry.
#[must_use]
pub fn hs_push_char(mut st: HistorySearchState, ch: char, history: &[String]) -> HistorySearchState {
    st.query.push(ch);
    st.match_index = recompute_match(history, &st.query, None);
    st
}

/// Delete the last char of the query (saturating) and recompute.
#[must_use]
pub fn hs_backspace(mut st: HistorySearchState, history: &[String]) -> HistorySearchState {
    st.query.pop();
    st.match_index = recompute_match(history, &st.query, None);
    st
}
```

- [ ] **Step 4: Add the `AppState` field**

In `crates/tui/src/state.rs`, add to the `AppState` struct (near `history`/`history_cursor`, around `state.rs:196`):

```rust
    /// (M7-10) Active Ctrl-R history-search overlay. `Some(_)` means the
    /// overlay owns all live keys (parent spec §2.5 priority 3).
    pub history_search: Option<crate::components::prompt_input::HistorySearchState>,
```

And in `AppState::new` (around `state.rs:245`), add:

```rust
            history_search: None,
```

- [ ] **Step 5: Re-export the new functions**

In `crates/tui/src/components/prompt_input/mod.rs`, extend the re-export:

```rust
pub use history_search::{hs_backspace, hs_open, hs_push_char, recompute_match, HistorySearchState};
```

- [ ] **Step 6: Run to verify pass + crate builds**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib history_search::tests && cargo check -p lingxi-tui --all-targets`
Expected: PASS, no build errors.

- [ ] **Step 7: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/history_search.rs crates/tui/src/components/prompt_input/mod.rs crates/tui/src/state.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T2): history-search filter (recompute_match/push_char/backspace) + AppState field

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `hs_cycle` + `hs_accept` + `hs_cancel` + `handle_history_search_key` (the overlay key handler)

**Files:**
- Modify: `crates/tui/src/components/prompt_input/history_search.rs`

`hs_cycle` advances Ctrl-R to the next older match (wrapping to nothing-found ⇒ the search stays on the current match, mirroring claude-code's "no older match" no-op). `hs_accept` returns the matched history line (or the query verbatim if there is no match — claude-code accepts the typed text when there is no match). `hs_cancel` returns the saved prompt. `handle_history_search_key` is the single key router for the overlay, consuming a crossterm-0.28 `KeyEvent` (the same currency the focus-trap path uses via `iocraft_to_crossterm028_key`).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod:

```rust
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn cycle_steps_to_older_match() {
        let h = hist(); // "cargo" at idx 1 and 3
        let mut st = hs_open("", 0);
        st = hs_push_char(st, 'c', &h);
        st = hs_push_char(st, 'a', &h); // "ca" → newest match idx 3
        assert_eq!(st.match_index, Some(3));
        st = hs_cycle(st, &h); // older "cargo" → idx 1
        assert_eq!(st.match_index, Some(1));
        st = hs_cycle(st, &h); // no older match → stays at idx 1
        assert_eq!(st.match_index, Some(1));
    }

    #[test]
    fn accept_returns_match_else_query() {
        let h = hist();
        let mut st = hs_push_char(hs_open("", 0), 'g', &h); // idx 2
        assert_eq!(hs_accept(&st, &h), "git commit -m wip");
        st = hs_push_char(hs_open("", 0), 'z', &h); // no match
        assert_eq!(hs_accept(&st, &h), "z"); // accept typed query verbatim
    }

    #[test]
    fn cancel_returns_saved_prompt() {
        let st = hs_open("original draft", 8);
        assert_eq!((st.saved_prompt.clone(), st.saved_cursor), ("original draft".to_string(), 8));
    }

    #[test]
    fn key_handler_routes_printable_ctrl_r_enter_esc_backspace() {
        let h = hist();
        let mut st = hs_open("", 0);
        // printable → push
        let out = handle_history_search_key(st.clone(), &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE), &h);
        assert!(matches!(&out, HsKeyOutcome::Continue(s) if s.query == "c"));
        st = match out { HsKeyOutcome::Continue(s) => s, _ => unreachable!() };
        // Ctrl-R → cycle (still Continue)
        let out = handle_history_search_key(st.clone(), &KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL), &h);
        assert!(matches!(out, HsKeyOutcome::Continue(_)));
        // Enter → Accept(text)
        let out = handle_history_search_key(st.clone(), &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &h);
        assert!(matches!(out, HsKeyOutcome::Accept(_)));
        // Esc → Cancel(saved_prompt, saved_cursor)
        let out = handle_history_search_key(st.clone(), &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &h);
        assert!(matches!(out, HsKeyOutcome::Cancel(p, _) if p.is_empty()));
        // Backspace on empty query stays Continue with empty query
        let out = handle_history_search_key(st, &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), &h);
        assert!(matches!(&out, HsKeyOutcome::Continue(s) if s.query.is_empty()));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib history_search::tests`
Expected: FAIL — `hs_cycle`, `hs_accept`, `handle_history_search_key`, `HsKeyOutcome` undefined.

- [ ] **Step 3: Implement cycle/accept/cancel + the key router**

Add to `history_search.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Step to the next older match (Ctrl-R pressed again). If there is no older
/// match, keep the current match (no wrap — claude-code beeps/stays).
#[must_use]
pub fn hs_cycle(mut st: HistorySearchState, history: &[String]) -> HistorySearchState {
    if let Some(next) = recompute_match(history, &st.query, st.match_index) {
        st.match_index = Some(next);
    }
    st
}

/// The text to place in the prompt on Enter: the matched history line, or the
/// typed query verbatim when nothing matched.
#[must_use]
pub fn hs_accept(st: &HistorySearchState, history: &[String]) -> String {
    match st.match_index {
        Some(i) => history.get(i).cloned().unwrap_or_else(|| st.query.clone()),
        None => st.query.clone(),
    }
}

/// Result of feeding one key to the active overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HsKeyOutcome {
    /// Overlay stays open with this (possibly updated) state.
    Continue(HistorySearchState),
    /// Enter accepted: put this text in the prompt and close the overlay.
    Accept(String),
    /// Esc cancelled: restore this prompt text + cursor and close.
    Cancel(String, usize),
}

/// Route one key into the overlay. Consumes a crossterm-0.28 `KeyEvent` (the
/// same currency the permission focus-trap uses via `iocraft_to_crossterm028_key`).
#[must_use]
pub fn handle_history_search_key(
    st: HistorySearchState,
    key: &KeyEvent,
    history: &[String],
) -> HsKeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, _) => HsKeyOutcome::Accept(hs_accept(&st, history)),
        (KeyCode::Esc, _) => HsKeyOutcome::Cancel(st.saved_prompt.clone(), st.saved_cursor),
        (KeyCode::Char('r'), m) if m.contains(KeyModifiers::CONTROL) => {
            HsKeyOutcome::Continue(hs_cycle(st, history))
        }
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
            // Ctrl-C inside search behaves as cancel (restore prompt).
            HsKeyOutcome::Cancel(st.saved_prompt.clone(), st.saved_cursor)
        }
        (KeyCode::Backspace, _) => HsKeyOutcome::Continue(hs_backspace(st, history)),
        (KeyCode::Char(c), m) if (m == KeyModifiers::NONE || m == KeyModifiers::SHIFT) => {
            HsKeyOutcome::Continue(hs_push_char(st, c, history))
        }
        // Any other key (arrows, etc.) is captured but inert — the overlay owns
        // focus, so a stray key never leaks to the editor (parent spec §2.5).
        _ => HsKeyOutcome::Continue(st),
    }
}
```

- [ ] **Step 4: Re-export**

In `mod.rs`:

```rust
pub use history_search::{
    handle_history_search_key, hs_accept, hs_backspace, hs_cycle, hs_open, hs_push_char,
    recompute_match, HistorySearchState, HsKeyOutcome,
};
```

- [ ] **Step 5: Run to verify pass**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib history_search::tests`
Expected: PASS (all history_search unit tests).

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/history_search.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T3): history-search cycle/accept/cancel + handle_history_search_key router

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Wire history search into `handle_live_key` (priority 3) + overlay render + behavior tests

**Files:**
- Modify: `crates/tui/src/root.rs` (priority-3 branch + Ctrl-R open)
- Create: `crates/tui/src/components/prompt_input/history_search.rs` (the `HistorySearchOverlay` component — append to the existing file)
- Modify: `crates/tui/src/screens/repl.rs` (mount the overlay when active)
- Create: `crates/tui/tests/history_search_test.rs`

The dispatcher gains a priority-3 branch **below** the permission focus-trap (priority 1) and the active-screen check (priority 2, added by the screen sub-plans — if `active_screen` does not exist yet in `AppState`, place the history-search branch immediately after the permission trap and note it; the screen sub-plans slot above it). Ctrl-R opens the overlay only when no overlay/dialog is already active.

- [ ] **Step 1: Write the failing behavior test**

Create `crates/tui/tests/history_search_test.rs`:

```rust
//! M7-10 behavior: Ctrl-R history search owns keys while open, filters/cycles,
//! Enter accepts into the prompt, Esc restores the pre-search prompt.

use iocraft::prelude::*;
use lingxi_tui::root::handle_live_key;
use lingxi_tui::state::AppState;
use lingxi_tui::status_snapshot_for_test; // helper used across M6/M7 tui tests

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(KeyEventKind::Press, code)
}
fn ctrl(c: char) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(c));
    k.modifiers = KeyModifiers::CONTROL;
    k
}

fn state_with_history() -> AppState {
    let mut st = AppState::new(status_snapshot_for_test());
    st.history = vec![
        "git status".into(),
        "cargo test".into(),
        "git commit -m wip".into(),
        "cargo build".into(),
    ];
    st
}

#[test]
fn ctrl_r_opens_then_filters_then_accepts_into_prompt() {
    let mut st = state_with_history();
    handle_live_key(&mut st, &ctrl('r'), 20);
    assert!(st.history_search.is_some(), "Ctrl-R opens the overlay");

    // Typing 'g' filters to the newest "git" entry.
    handle_live_key(&mut st, &key(KeyCode::Char('g')), 20);
    let hs = st.history_search.as_ref().unwrap();
    assert_eq!(hs.query, "g");
    assert_eq!(hs.match_index, Some(2));

    // Enter accepts the match into the prompt and closes the overlay.
    handle_live_key(&mut st, &key(KeyCode::Enter), 20);
    assert!(st.history_search.is_none(), "Enter closes the overlay");
    assert_eq!(st.prompt_text, "git commit -m wip");
    assert_eq!(st.prompt_cursor, "git commit -m wip".len());
}

#[test]
fn ctrl_r_cycles_to_older_match() {
    let mut st = state_with_history();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('c')), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('a')), 20); // "ca" → idx 3
    assert_eq!(st.history_search.as_ref().unwrap().match_index, Some(3));
    handle_live_key(&mut st, &ctrl('r'), 20); // older "cargo" → idx 1
    assert_eq!(st.history_search.as_ref().unwrap().match_index, Some(1));
}

#[test]
fn esc_restores_pre_search_prompt() {
    let mut st = state_with_history();
    st.prompt_text = "half typed".into();
    st.prompt_cursor = "half typed".len();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('g')), 20);
    handle_live_key(&mut st, &key(KeyCode::Esc), 20);
    assert!(st.history_search.is_none());
    assert_eq!(st.prompt_text, "half typed", "Esc restores the original prompt");
    assert_eq!(st.prompt_cursor, "half typed".len());
}

#[test]
fn no_match_state_keeps_overlay_open_and_match_none() {
    let mut st = state_with_history();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('z')), 20);
    let hs = st.history_search.as_ref().unwrap();
    assert_eq!(hs.query, "z");
    assert_eq!(hs.match_index, None, "no history line contains 'z'");
}

#[test]
fn empty_history_open_has_no_match() {
    let mut st = AppState::new(status_snapshot_for_test()); // history empty
    handle_live_key(&mut st, &ctrl('r'), 20);
    assert!(st.history_search.is_some());
    handle_live_key(&mut st, &key(KeyCode::Char('x')), 20);
    assert_eq!(st.history_search.as_ref().unwrap().match_index, None);
}

#[test]
fn while_active_normal_edit_keys_are_captured_by_search() {
    // The prompt must NOT receive characters while the overlay is open.
    let mut st = state_with_history();
    st.prompt_text = String::new();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('a')), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('b')), 20);
    assert_eq!(st.prompt_text, "", "prompt untouched while search owns keys");
    assert_eq!(st.history_search.as_ref().unwrap().query, "ab");
}
```

> **Note on the test helper:** M6/M7 tui integration tests construct an `AppState` via a shared `status_snapshot_for_test()` (or equivalent). At execution time, confirm the exact helper name with `cd lingxi-core && grep -rn "AppState::new(" crates/tui/tests | head` and use whatever the existing tests use (e.g. `lingxi_tui::test_support::*` or an inline `StatusSnapshot::default()`); adjust the `use` lines accordingly. Do not invent a new helper.

- [ ] **Step 2: Run to verify failure**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test history_search_test`
Expected: FAIL — `history_search` field never set (Ctrl-R not wired), prompt not updated.

- [ ] **Step 3: Add the priority-3 branch + Ctrl-R open to `handle_live_key`**

In `crates/tui/src/root.rs`, inside `handle_live_key`, immediately **after** the permission focus-trap block (`root.rs:188-194`, the `if st.pending_permission.is_some() { ... return; }`), add:

```rust
    // === INPUT OVERLAY (priority 3): history search owns all keys. ===
    if st.history_search.is_some() {
        use crate::components::prompt_input::{handle_history_search_key, HsKeyOutcome};
        let ct_key = iocraft_to_crossterm028_key(k);
        let hs = st.history_search.take().expect("checked is_some");
        match handle_history_search_key(hs, &ct_key, &st.history) {
            HsKeyOutcome::Continue(next) => st.history_search = Some(next),
            HsKeyOutcome::Accept(text) => {
                st.prompt_cursor = text.len();
                st.prompt_text = text;
                st.history_cursor = None;
            }
            HsKeyOutcome::Cancel(prompt, cursor) => {
                st.prompt_text = prompt;
                st.prompt_cursor = cursor;
            }
        }
        return;
    }
    // === end input overlay ===
```

Then, in the **fall-through** key path (before/within the `map_iocraft_key` dispatch, `root.rs:196-210`), add the Ctrl-R open binding. Insert just before the `if let Some(action) = map_iocraft_key(...)` block:

```rust
    // Ctrl-R opens the history-search overlay (priority-3 input overlay). Only
    // when no overlay/dialog is active (we already returned above if one was).
    if matches!(k.code, KeyCode::Char('r')) && k.modifiers.contains(KeyModifiers::CONTROL) {
        st.history_search = Some(crate::components::prompt_input::hs_open(
            &st.prompt_text,
            st.prompt_cursor,
        ));
        return;
    }
```

> **Priority ordering:** if a later screen sub-plan (M7-11..14) has already added an `active_screen` check at priority 2, the history-search branch goes **below** it. If `active_screen` does not exist yet, the history-search branch sits directly after the permission trap — that is correct for M7-10 and the screen sub-plans will insert their check above it. Do not introduce a parallel key path (the M6 focus-trap bug was a parallel path — parent spec §2.5).

- [ ] **Step 4: Add the `HistorySearchOverlay` render component**

Append to `crates/tui/src/components/prompt_input/history_search.rs`:

```rust
use iocraft::prelude::*;

/// Props for the search overlay row.
#[derive(Default, Props)]
pub struct HistorySearchOverlayProps {
    /// The live query.
    pub query: String,
    /// True when no history line matches (drives the label literal).
    pub failed_match: bool,
}

/// One row rendered above the prompt while search is active. Mirrors
/// claude-code `HistorySearchInput.tsx`: a dim label (`search prompts:` /
/// `no matching prompt:`) and the query after a one-space gap.
#[component]
pub fn HistorySearchOverlay(props: &HistorySearchOverlayProps) -> impl Into<AnyElement<'static>> {
    let label = if props.failed_match {
        "no matching prompt:"
    } else {
        "search prompts:"
    };
    let line = format!("{label} {}", props.query);
    element! {
        View(flex_direction: FlexDirection::Row, height: 1) {
            Text(content: line, color: Color::DarkGrey)
        }
    }
}
```

> **Literal lock (parent spec §2.8):** the two labels are byte-for-byte from `HistorySearchInput.tsx:18` (`'no matching prompt:'` / `'search prompts:'`). The `Color::DarkGrey` stands in for claude-code's `dimColor`; if `theme::TuiTheme::DIM` exists (M7-06), prefer that constant for consistency with the footer.

- [ ] **Step 5: Mount the overlay in the REPL screen**

In `crates/tui/src/screens/repl.rs`, where the prompt zone is laid out, render the overlay row above the input when `state.history_search.is_some()`:

```rust
    // M7-10: history-search overlay row (above the prompt) when active.
    if let Some(hs) = state.history_search.as_ref() {
        element! {
            HistorySearchOverlay(
                query: hs.query.clone(),
                failed_match: !hs.query.is_empty() && hs.match_index.is_none(),
            )
        }
    }
```

Use the existing conditional-children idiom in `repl.rs` (M6 mounts the spinner row conditionally the same way — match that pattern; if `repl.rs` builds children via a `Vec<AnyElement>`, push the overlay element into it). Import `HistorySearchOverlay` from `crate::components::prompt_input`.

- [ ] **Step 6: Run the behavior tests to verify they pass**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test history_search_test`
Expected: PASS — all 6 history-search behavior tests.

- [ ] **Step 7: Run the focus-trap regression to prove no parallel key path leaked**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test focus_trap_test`
Expected: PASS — `dialog_open_prompt_input_not_mutated`, `no_dialog_open_prompt_input_accepts_keys` still green (history-search branch sits below the permission trap; a key during a permission dialog never reaches it).

- [ ] **Step 8: Commit**

```bash
cd lingxi-core && git add crates/tui/src/root.rs crates/tui/src/components/prompt_input/history_search.rs crates/tui/src/screens/repl.rs crates/tui/tests/history_search_test.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T4): wire Ctrl-R history search as priority-3 input overlay + overlay row + behavior tests

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Scaffold `image_paste.rs` — `format_image_ref` + `is_image_path` + types + state field

**Files:**
- Create: `crates/tui/src/components/prompt_input/image_paste.rs`
- Modify: `crates/tui/src/components/prompt_input/mod.rs` (add `pub mod image_paste;` + re-exports)
- Modify: `crates/tui/src/state.rs` (add the `paste: PasteState` field)

`format_image_ref` and `is_image_path` are the byte-locked primitives. `PasteState` holds the next image id (1-based) and the attachment registry.

- [ ] **Step 1: Write the failing tests**

Create `crates/tui/src/components/prompt_input/image_paste.rs`:

```rust
//! Bracketed-paste coalescing + image-on-paste detection.
//!
//! **Scope (parent spec §4 R8): detection + reference insertion only.** No
//! inline terminal image display, no byte reading / base64 / resize / API
//! wiring — those are M8. On an image paste we record `Attachment` metadata
//! and insert a `[Image #N]` placeholder; on a text paste we insert the text
//! verbatim as one block (multi-line paste does NOT submit per line).
//!
//! iocraft 0.8.3 surfaces no paste event (see `root.rs` notes), so paste is
//! detected by burst coalescing in the live key path; the pure classification
//! and ref-building logic lives here and is unit-tested directly.

/// claude-code `formatImageRef(id)` → `[Image #id]` (history.ts:58-59).
#[must_use]
pub fn format_image_ref(id: usize) -> String {
    format!("[Image #{id}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_ref_format_matches_claude_code() {
        assert_eq!(format_image_ref(1), "[Image #1]");
        assert_eq!(format_image_ref(42), "[Image #42]");
    }

    #[test]
    fn is_image_path_matches_supported_extensions() {
        assert!(is_image_path("/tmp/shot.png"));
        assert!(is_image_path("/tmp/a.JPG"));        // case-insensitive
        assert!(is_image_path("photo.jpeg"));
        assert!(is_image_path("anim.gif"));
        assert!(is_image_path("logo.webp"));
        assert!(is_image_path("'/Users/me/My Pic.png'")); // outer quotes stripped
        assert!(is_image_path("  /tmp/trailing.png  "));  // trimmed
        assert!(!is_image_path("/tmp/notes.txt"));
        assert!(!is_image_path("just some pasted text"));
        assert!(!is_image_path("/tmp/archive.png.zip")); // ext must be trailing
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests`
Expected: FAIL — `is_image_path` undefined.

- [ ] **Step 3: Implement `is_image_path` + the types + `format_image_ref` (already added)**

Add to `image_paste.rs`:

```rust
/// Supported image extensions, lowercase, including the leading dot. Mirrors
/// claude-code `IMAGE_EXTENSION_REGEX = /\.(png|jpe?g|gif|webp)$/i`
/// (imagePaste.ts:270).
const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// Strip a single pair of matching outer single/double quotes
/// (claude-code `removeOuterQuotes`).
fn remove_outer_quotes(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// True when `text` is a path/filename ending in a supported image extension,
/// after trimming whitespace and stripping outer quotes (claude-code
/// `isImageFilePath`). Backslash-escape unescaping (`stripBackslashEscapes`)
/// is NOT ported (M8); a path with literal `\` simply fails to match (safe
/// degrade — never a false positive).
#[must_use]
pub fn is_image_path(text: &str) -> bool {
    let cleaned = remove_outer_quotes(text.trim()).trim();
    let lower = cleaned.to_ascii_lowercase();
    IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// What kind of attachment a paste produced. M7-10 only mints `Image`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentKind {
    /// An image detected on paste. M7-10 records the source only (no bytes).
    Image,
}

/// Metadata recorded for one detected attachment. The `[Image #id]` ref in the
/// prompt points back to this by `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// 1-based id, matching the `[Image #id]` placeholder.
    pub id: usize,
    /// The kind (Image only in M7-10).
    pub kind: AttachmentKind,
    /// The source: the file path, or `"clipboard"` for an empty macOS paste.
    pub source: String,
}

/// Paste-related state on `AppState`. Holds the attachment registry and the
/// next id to mint. Defaults to id 1, empty registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteState {
    /// Next `[Image #N]` id to assign (1-based, increments per image).
    pub next_image_id: usize,
    /// Recorded attachments, in mint order.
    pub attachments: Vec<Attachment>,
}

impl Default for PasteState {
    fn default() -> Self {
        Self { next_image_id: 1, attachments: Vec::new() }
    }
}
```

- [ ] **Step 4: Add the `AppState` field + module wiring**

In `crates/tui/src/state.rs`, add to `AppState` (near `history_search`):

```rust
    /// (M7-10) Paste attachment registry + next `[Image #N]` id.
    pub paste: crate::components::prompt_input::PasteState,
```

And in `AppState::new`:

```rust
            paste: crate::components::prompt_input::PasteState::default(),
```

In `crates/tui/src/components/prompt_input/mod.rs`:

```rust
pub mod image_paste;
pub use image_paste::{
    format_image_ref, is_image_path, Attachment, AttachmentKind, PasteState,
};
```

- [ ] **Step 5: Run to verify pass + build**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests && cargo check -p lingxi-tui --all-targets`
Expected: PASS, no build errors.

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/image_paste.rs crates/tui/src/components/prompt_input/mod.rs crates/tui/src/state.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T5): scaffold image_paste.rs (format_image_ref/is_image_path/types) + PasteState field

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `process_paste` — classify lines, mint `[Image #N]` refs, build insertion + attachments

**Files:**
- Modify: `crates/tui/src/components/prompt_input/image_paste.rs`

`process_paste` takes a coalesced paste block + the current `PasteState`, splits on newlines (and on space-before-absolute-path, matching claude-code's `usePasteHandler` split), classifies each line, builds the text to insert (image lines → `[Image #N]`, others verbatim), and returns the new `PasteState` (with appended attachments + bumped id) plus the insertion string. **The block stays a single insertion** — never multiple submits.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod in `image_paste.rs`:

```rust
    fn fresh() -> PasteState {
        PasteState::default()
    }

    #[test]
    fn plain_multiline_text_inserts_verbatim_no_attachments() {
        let st = fresh();
        let out = process_paste("line one\nline two\nline three", st);
        assert_eq!(out.insertion, "line one\nline two\nline three");
        assert!(out.state.attachments.is_empty());
        assert_eq!(out.state.next_image_id, 1);
    }

    #[test]
    fn single_image_path_becomes_ref_and_records_attachment() {
        let st = fresh();
        let out = process_paste("/tmp/screenshot.png", st);
        assert_eq!(out.insertion, "[Image #1]");
        assert_eq!(out.state.attachments.len(), 1);
        assert_eq!(out.state.attachments[0].id, 1);
        assert_eq!(out.state.attachments[0].kind, AttachmentKind::Image);
        assert_eq!(out.state.attachments[0].source, "/tmp/screenshot.png");
        assert_eq!(out.state.next_image_id, 2);
    }

    #[test]
    fn counter_increments_across_multiple_images() {
        // newline-separated image paths → two refs, ids 1 and 2.
        let out = process_paste("/a/one.png\n/b/two.jpg", fresh());
        assert_eq!(out.insertion, "[Image #1]\n[Image #2]");
        assert_eq!(out.state.attachments.len(), 2);
        assert_eq!(out.state.next_image_id, 3);
        // a subsequent paste continues from where the counter left off.
        let out2 = process_paste("/c/three.gif", out.state);
        assert_eq!(out2.insertion, "[Image #3]");
        assert_eq!(out2.state.next_image_id, 4);
    }

    #[test]
    fn mixed_image_and_text_lines_keep_text_inline() {
        let out = process_paste("see this:\n/tmp/pic.png\nthanks", fresh());
        assert_eq!(out.insertion, "see this:\n[Image #1]\nthanks");
        assert_eq!(out.state.attachments.len(), 1);
    }

    #[test]
    fn space_separated_finder_paths_split_on_absolute_path_boundary() {
        // Finder drag pastes space-separated absolute paths.
        let out = process_paste("/tmp/a.png /tmp/b.png", fresh());
        assert_eq!(out.insertion, "[Image #1] [Image #2]");
        assert_eq!(out.state.attachments.len(), 2);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests`
Expected: FAIL — `process_paste`, `PasteOutcome` undefined.

- [ ] **Step 3: Implement `process_paste` + `PasteOutcome`**

Add to `image_paste.rs`:

```rust
/// The result of processing one coalesced paste block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteOutcome {
    /// The text to insert at the cursor as ONE block (image lines swapped for
    /// `[Image #N]`, everything else verbatim).
    pub insertion: String,
    /// The updated paste state (appended attachments + bumped id).
    pub state: PasteState,
}

/// Split a paste block the way claude-code's `usePasteHandler` does: first on
/// spaces that precede an absolute path (` /` on unix, ` C:\` on windows),
/// then on newlines. Returns segments paired with the separator that FOLLOWED
/// each segment in the original, so the insertion can be rebuilt exactly.
fn split_paste_segments(block: &str) -> Vec<(String, &'static str)> {
    // Token = a run of text; sep = "\n", " ", or "" (last token).
    // We tokenize char-by-char so we can preserve newlines and the
    // path-boundary spaces as separators while leaving in-path spaces intact.
    let mut out: Vec<(String, &'static str)> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = block.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            out.push((std::mem::take(&mut cur), "\n"));
            i += 1;
            continue;
        }
        if c == ' ' {
            // Split only when the space precedes an absolute path: ` /` or
            // ` X:\`. Otherwise the space stays inside the current token.
            let next = chars.get(i + 1).copied();
            let drive = matches!(next, Some(ch) if ch.is_ascii_alphabetic())
                && matches!(chars.get(i + 2).copied(), Some(':'))
                && matches!(chars.get(i + 3).copied(), Some('\\'));
            if next == Some('/') || drive {
                out.push((std::mem::take(&mut cur), " "));
                i += 1;
                continue;
            }
        }
        cur.push(c);
        i += 1;
    }
    out.push((cur, ""));
    out
}

/// Classify + rebuild a coalesced paste block into an insertion string and an
/// updated paste state. Image segments become `[Image #N]` and record an
/// `Attachment`; all other segments (and the separators) are preserved.
#[must_use]
pub fn process_paste(block: &str, mut state: PasteState) -> PasteOutcome {
    let mut insertion = String::with_capacity(block.len());
    for (seg, sep) in split_paste_segments(block) {
        if !seg.is_empty() && is_image_path(&seg) {
            let id = state.next_image_id;
            state.next_image_id += 1;
            state.attachments.push(Attachment {
                id,
                kind: AttachmentKind::Image,
                source: remove_outer_quotes(seg.trim()).trim().to_string(),
            });
            insertion.push_str(&format_image_ref(id));
        } else {
            insertion.push_str(&seg);
        }
        insertion.push_str(sep);
    }
    PasteOutcome { insertion, state }
}
```

> **Decision (LOCKED):** `process_paste` preserves the original separators (`\n` / path-boundary ` `) so a Finder space-separated drag rebuilds as `[Image #1] [Image #2]` (space kept) and a newline-separated drag rebuilds as `[Image #1]\n[Image #2]`. This matches the claude-code split semantics without inventing a normalization rule.

- [ ] **Step 4: Re-export**

In `mod.rs`:

```rust
pub use image_paste::{
    format_image_ref, is_image_path, process_paste, Attachment, AttachmentKind, PasteOutcome,
    PasteState,
};
```

- [ ] **Step 5: Run to verify pass**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests`
Expected: PASS (all `process_paste` tests).

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/image_paste.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T6): process_paste classifies lines, mints [Image #N] refs + records attachments

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `PasteCoalescer` — burst buffer + flush decision (the bracketed-paste substitute)

**Files:**
- Modify: `crates/tui/src/components/prompt_input/image_paste.rs`

Because iocraft surfaces no paste event, the coalescer turns a rapid burst of single-char `Key` events into one block. It records the timestamp of each pushed char; a char arriving within `BURST_WINDOW` of the previous one is part of the same paste. A non-printable key (or a quiet tick beyond the window) flushes the buffer. The coalescer is a pure state machine over `(char, Instant)` inputs so it is unit-testable with synthetic timestamps.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod:

```rust
    use std::time::{Duration, Instant};

    #[test]
    fn rapid_chars_buffer_then_flush_as_one_block() {
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        // Three chars 1ms apart → one burst.
        assert_eq!(c.push_char('a', base), None);
        assert_eq!(c.push_char('b', base + Duration::from_millis(1)), None);
        assert_eq!(c.push_char('\n', base + Duration::from_millis(2)), None);
        assert_eq!(c.push_char('c', base + Duration::from_millis(3)), None);
        // A quiet tick beyond the window flushes the whole block.
        assert_eq!(
            c.flush_if_idle(base + Duration::from_millis(60)),
            Some("ab\nc".to_string())
        );
        // Buffer is now empty.
        assert_eq!(c.flush_if_idle(base + Duration::from_millis(120)), None);
    }

    #[test]
    fn slow_typing_is_not_coalesced() {
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        // First char buffers; the SECOND char arrives after the window, so the
        // first char flushes as a 1-char "block" and the second starts anew.
        assert_eq!(c.push_char('x', base), None);
        let flushed = c.push_char('y', base + Duration::from_millis(80));
        assert_eq!(flushed, Some("x".to_string()));
        // 'y' is now the lone buffered char; an idle tick flushes it.
        assert_eq!(
            c.flush_if_idle(base + Duration::from_millis(200)),
            Some("y".to_string())
        );
    }

    #[test]
    fn non_printable_key_flushes_buffer() {
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        c.push_char('h', base);
        c.push_char('i', base + Duration::from_millis(1));
        // A non-printable key (e.g. Left arrow) forces an immediate flush.
        assert_eq!(c.flush_now(), Some("hi".to_string()));
        assert_eq!(c.flush_now(), None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests`
Expected: FAIL — `PasteCoalescer` undefined.

- [ ] **Step 3: Implement `PasteCoalescer`**

Add to `image_paste.rs`:

```rust
use std::time::{Duration, Instant};

/// Max gap between consecutive chars to count as the same paste burst. Paste
/// chars arrive sub-millisecond apart; human typing is tens of ms apart. 50ms
/// matches claude-code's `CLIPBOARD_CHECK_DEBOUNCE_MS` / paste-completion feel.
pub const BURST_WINDOW: Duration = Duration::from_millis(50);

/// Coalesces a rapid burst of single-char `Key` events (iocraft has no paste
/// event — see `root.rs`) into one block. Pure over `(char, Instant)`.
#[derive(Debug, Default)]
pub struct PasteCoalescer {
    buf: String,
    last: Option<Instant>,
}

impl PasteCoalescer {
    /// New, empty coalescer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one printable char (newline included) arriving at `now`. If `now`
    /// is more than `BURST_WINDOW` after the previous char, the existing buffer
    /// is flushed first and returned; `ch` then starts a fresh buffer.
    /// Returns `Some(block)` only when a flush happened.
    #[must_use]
    pub fn push_char(&mut self, ch: char, now: Instant) -> Option<String> {
        let flushed = match self.last {
            Some(prev) if now.duration_since(prev) > BURST_WINDOW => self.take(),
            _ => None,
        };
        self.buf.push(ch);
        self.last = Some(now);
        flushed
    }

    /// Flush if the buffer has gone quiet (called from a periodic tick). Flushes
    /// when `now` is more than `BURST_WINDOW` past the last char.
    #[must_use]
    pub fn flush_if_idle(&mut self, now: Instant) -> Option<String> {
        match self.last {
            Some(prev) if now.duration_since(prev) > BURST_WINDOW => self.take(),
            _ => None,
        }
    }

    /// Flush immediately (e.g. a non-printable key arrived, or before submit).
    #[must_use]
    pub fn flush_now(&mut self) -> Option<String> {
        self.take()
    }

    fn take(&mut self) -> Option<String> {
        self.last = None;
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}
```

- [ ] **Step 4: Re-export**

In `mod.rs`, add `BURST_WINDOW` and `PasteCoalescer` to the `image_paste` re-export.

- [ ] **Step 5: Run to verify pass**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests`
Expected: PASS (coalescer tests).

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/image_paste.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T7): PasteCoalescer burst buffer + flush decision (iocraft-no-paste-event substitute)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `apply_paste_block` — single-block insert into the prompt (no per-line submit)

**Files:**
- Modify: `crates/tui/src/components/prompt_input/image_paste.rs`

This is the one function the live path calls when a block flushes: it runs `process_paste`, inserts the result at the prompt cursor (reusing the editor-core string slice, since the insertion is a whole string not one char), advances the cursor past it, and writes back the attachments. Pure over `(prompt, cursor, block, PasteState)`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod:

```rust
    #[test]
    fn apply_block_inserts_text_at_cursor_and_advances() {
        let st = fresh();
        // prompt "ab|cd" (cursor at byte 2), paste "XY".
        let r = apply_paste_block("abcd", 2, "XY", st);
        assert_eq!(r.prompt, "abXYcd");
        assert_eq!(r.cursor, 4);
        assert!(r.state.attachments.is_empty());
    }

    #[test]
    fn apply_block_inserts_image_ref_and_records_attachment() {
        let st = fresh();
        let r = apply_paste_block("", 0, "/tmp/shot.png", st);
        assert_eq!(r.prompt, "[Image #1]");
        assert_eq!(r.cursor, "[Image #1]".len());
        assert_eq!(r.state.attachments.len(), 1);
        assert_eq!(r.state.attachments[0].source, "/tmp/shot.png");
    }

    #[test]
    fn apply_block_multiline_is_a_single_insertion() {
        // The whole multi-line block lands at once — no submit happens here.
        let r = apply_paste_block("> ", 2, "first\nsecond", fresh());
        assert_eq!(r.prompt, "> first\nsecond");
        assert_eq!(r.cursor, "> first\nsecond".len());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests`
Expected: FAIL — `apply_paste_block`, `PasteApply` undefined.

- [ ] **Step 3: Implement `apply_paste_block`**

Add to `image_paste.rs`:

```rust
/// Result of applying a coalesced block to the prompt buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteApply {
    /// New prompt text.
    pub prompt: String,
    /// New cursor byte-index (just past the inserted block).
    pub cursor: usize,
    /// Updated paste state.
    pub state: PasteState,
}

/// Run `process_paste` on `block`, splice the insertion into `prompt` at
/// `cursor` (a char boundary), and advance the cursor past it. The block is
/// inserted as ONE unit — embedded newlines do not trigger submit (the live
/// path only submits on a bare Enter that the coalescer has already flushed
/// past). `cursor` is clamped into `prompt` defensively.
#[must_use]
pub fn apply_paste_block(prompt: &str, cursor: usize, block: &str, state: PasteState) -> PasteApply {
    let PasteOutcome { insertion, state } = process_paste(block, state);
    let at = clamp_to_char_boundary(prompt, cursor);
    let mut out = String::with_capacity(prompt.len() + insertion.len());
    out.push_str(&prompt[..at]);
    out.push_str(&insertion);
    out.push_str(&prompt[at..]);
    let new_cursor = at + insertion.len();
    PasteApply { prompt: out, cursor: new_cursor, state }
}

/// Clamp `cursor` down to the nearest char boundary at or below it
/// (mirrors `prompt_input::mod`'s private helper; duplicated here to keep
/// `apply_paste_block` self-contained and pure).
fn clamp_to_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut c = cursor;
    while c > 0 && !text.is_char_boundary(c) {
        c -= 1;
    }
    c
}
```

- [ ] **Step 4: Re-export**

In `mod.rs`, add `apply_paste_block` and `PasteApply` to the re-export.

- [ ] **Step 5: Run to verify pass**

Run: `cd lingxi-core && cargo test -p lingxi-tui --lib image_paste::tests`
Expected: PASS (all `image_paste` unit tests, including `apply_paste_block`).

- [ ] **Step 6: Commit**

```bash
cd lingxi-core && git add crates/tui/src/components/prompt_input/image_paste.rs crates/tui/src/components/prompt_input/mod.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T8): apply_paste_block single-block prompt insertion (no per-line submit)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Wire the coalescer into `handle_live_key` + the quiet-tick flush

**Files:**
- Modify: `crates/tui/src/root.rs`
- Modify: `crates/tui/src/state.rs` (the coalescer lives on the live mount, not `AppState` — see note)

The coalescer is per-mount mutable state, not serializable session state, so it lives in `root.rs` alongside the `use_terminal_events` closure (a `Rc<RefCell<PasteCoalescer>>` captured by the key closure and the ticker). On each printable `Key`, push to the coalescer; if it returns a flushed block, apply it via `apply_paste_block`. On a non-printable key, flush-now first (so a buffered burst lands before the key acts), then proceed with normal dispatch. A new branch of the existing 100ms ticker calls `flush_if_idle` so a paste that ends without a trailing keystroke still lands.

- [ ] **Step 1: Add the coalescer flush into the key closure**

In `crates/tui/src/root.rs`, in the `TuiRoot` component, create the shared coalescer before the `use_terminal_events` block:

```rust
    // ---- Paste coalescer: iocraft has no paste event, so a paste arrives as
    // a rapid burst of Key events. We buffer printable chars and flush the
    // block as one insertion (multi-line paste never submits per line). ----
    let paste_coalescer = hooks.use_state(|| {
        std::rc::Rc::new(std::cell::RefCell::new(
            crate::components::prompt_input::PasteCoalescer::new(),
        ))
    });
```

Then in the `use_terminal_events` closure (`root.rs:301-315`), replace the simple `handle_live_key` call with coalescer-aware routing. The closure already has `state.try_lock()`; inside the `TerminalEvent::Key` arm:

```rust
            TerminalEvent::Key(k) if k.kind != KeyEventKind::Release => {
                let Ok(mut st) = state.try_lock() else { return; };
                let now = std::time::Instant::now();
                let coalescer = paste_coalescer.get();
                // Don't coalesce while an overlay/dialog owns keys, or while a
                // history search is active — those consume keys directly.
                let overlay_active =
                    st.pending_permission.is_some() || st.history_search.is_some();
                let is_printable = matches!(k.code, KeyCode::Char(_))
                    && !k.modifiers.contains(KeyModifiers::CONTROL)
                    && !k.modifiers.contains(KeyModifiers::ALT);
                let is_newline_key = matches!(k.code, KeyCode::Enter);

                if !overlay_active && is_printable {
                    // Buffer the char; a returned block means the burst ended.
                    if let Some(KeyCode::Char(c)) = Some(k.code) {
                        if let Some(block) =
                            coalescer.borrow_mut().push_char(c, now)
                        {
                            apply_block(&mut st, &block);
                        }
                        // A single char that did NOT flush stays buffered; the
                        // ticker / next non-printable key flushes it. This is
                        // correct: a lone keystroke flushes within 50ms and the
                        // user sees it land essentially immediately.
                        drop(st);
                        tick_for_keys.set(tick_for_keys.get().wrapping_add(1));
                        return;
                    }
                }

                // Non-printable key (Enter, arrows, Ctrl-*, …) or overlay
                // active: flush any buffered burst FIRST so it lands before the
                // key acts, then route the key normally.
                if let Some(block) = coalescer.borrow_mut().flush_now() {
                    apply_block(&mut st, &block);
                }
                let _ = is_newline_key; // (documents intent; Enter handled by handle_live_key)
                handle_live_key(&mut st, &k, viewport);
                drop(st);
                tick_for_keys.set(tick_for_keys.get().wrapping_add(1));
            }
```

Add a small free helper near `handle_live_key` in `root.rs`:

```rust
/// Apply a coalesced paste block to the prompt buffer in-place.
fn apply_block(st: &mut AppState, block: &str) {
    use crate::components::prompt_input::apply_paste_block;
    let r = apply_paste_block(&st.prompt_text, st.prompt_cursor, block, st.paste.clone());
    st.prompt_text = r.prompt;
    st.prompt_cursor = r.cursor;
    st.paste = r.state;
}
```

> **Capture note:** `KeyCode`, `KeyModifiers`, `KeyEventKind` are already in scope via `iocraft::prelude::*`. `paste_coalescer.get()` returns the `Rc` clone (cheap). The `Rc<RefCell<…>>` is single-threaded — fine, because the iocraft event callback runs on the render thread. If `use_state` rejects the non-`Copy` `Rc` value, store it via `use_ref`/`hooks.use_state(|| …)` per whichever the resolved iocraft 0.8.3 supports (confirm with `grep -n "use_state\|use_ref" root.rs` and match the existing pattern; M6 used `use_state` for `Copy` values, so a `use_ref`-style hook may be needed for the `Rc` — adapt at execution time).

- [ ] **Step 2: Add the idle flush to the ticker**

In the 100ms ticker `use_future` (`root.rs:269-279`), add a coalescer idle-flush. Because the ticker is async and the coalescer `Rc` is not `Send`, instead add the flush to the **render path** or a dedicated short interval that locks state. Simplest correct approach: in the existing ticker loop, after the streaming check, also bump `tick` so the next render runs; then in the render snapshot (`root.rs:348`), call `coalescer.borrow_mut().flush_if_idle(Instant::now())` and apply any block before building the element. Add inside the `state.try_lock().map(|st| { ... })` closure, at the top:

```rust
        // M7-10: flush a paste burst that ended without a trailing keystroke.
        {
            let mut st = st; // shadow: we already hold the lock guard here
            if let Some(block) = paste_coalescer.get().borrow_mut().flush_if_idle(Instant::now()) {
                apply_block(&mut st, &block);
            }
            // continue building the element from `st` …
        }
```

> **Execution note:** the exact placement depends on how the M6 render snapshot is structured (it takes `st` by value into a closure). Keep it simple: ensure `flush_if_idle` runs on a regular cadence while the buffer is non-empty. The behavior test in Task 10 drives `process_paste`/`apply_paste_block` directly (not the live ticker), so this wiring is exercised by the manual smoke + the `apply_paste_block` unit tests; the ticker hook just guarantees a trailing-less paste eventually lands. If the render-path placement is awkward, an acceptable alternative is to flush-now on the **next** key of any kind (already done in Step 1) and accept that a paste with no following keystroke lands on the next render tick — document whichever is chosen.

- [ ] **Step 3: Build the crate**

Run: `cd lingxi-core && cargo check -p lingxi-tui --all-targets`
Expected: PASS, no errors.

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/src/root.rs crates/tui/src/state.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T9): wire PasteCoalescer into handle_live_key + idle-flush tick

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Paste behavior tests (single-block, no per-line submit, image ref + metadata, counter)

**Files:**
- Create: `crates/tui/tests/image_paste_test.rs`

These tests drive the pure paste pipeline (`apply_paste_block` + a small coalescer-feed helper) the same way the live mount does, asserting the spec's required behaviors without a PTY. The "no per-line submit" guarantee is proven structurally: a multi-line block is one `apply_paste_block` call that mutates `prompt_text` and never touches `Submit`.

- [ ] **Step 1: Write the failing tests**

Create `crates/tui/tests/image_paste_test.rs`:

```rust
//! M7-10 behavior: paste coalescing + image-ref insertion.
//! - A multi-line paste inserts as ONE block (no per-line submit).
//! - An image payload → `[Image #N]` ref + recorded attachment metadata.
//! - The counter increments across multiple images.

use std::time::{Duration, Instant};

use lingxi_tui::components::prompt_input::{
    apply_paste_block, AttachmentKind, PasteCoalescer, PasteState,
};

/// Feed a string into a coalescer as a tight 1ms-apart burst, then flush.
/// Returns the single coalesced block (proving multi-line stays one unit).
fn coalesce_burst(text: &str) -> String {
    let base = Instant::now();
    let mut c = PasteCoalescer::new();
    for (i, ch) in text.chars().enumerate() {
        let now = base + Duration::from_millis(i as u64); // 1ms apart → one burst
        assert_eq!(c.push_char(ch, now), None, "burst should not flush mid-stream");
    }
    c.flush_now().expect("non-empty burst flushes")
}

#[test]
fn multiline_paste_coalesces_to_single_block() {
    let block = coalesce_burst("line1\nline2\nline3");
    assert_eq!(block, "line1\nline2\nline3", "the whole paste is one block");
    // Inserting it is a single mutation — there is no submit in this path.
    let r = apply_paste_block("", 0, &block, PasteState::default());
    assert_eq!(r.prompt, "line1\nline2\nline3");
    assert_eq!(r.cursor, "line1\nline2\nline3".len());
}

#[test]
fn image_payload_inserts_ref_and_records_metadata() {
    let block = coalesce_burst("/tmp/screenshot.png");
    let r = apply_paste_block("here: ", 6, &block, PasteState::default());
    assert_eq!(r.prompt, "here: [Image #1]");
    assert_eq!(r.state.attachments.len(), 1);
    assert_eq!(r.state.attachments[0].id, 1);
    assert_eq!(r.state.attachments[0].kind, AttachmentKind::Image);
    assert_eq!(r.state.attachments[0].source, "/tmp/screenshot.png");
}

#[test]
fn counter_increments_for_multiple_images() {
    let mut state = PasteState::default();
    let r1 = apply_paste_block("", 0, &coalesce_burst("/a/one.png"), state);
    assert_eq!(r1.prompt, "[Image #1]");
    state = r1.state;
    let r2 = apply_paste_block(&r1.prompt, r1.cursor, &coalesce_burst("/b/two.jpg"), state);
    assert_eq!(r2.prompt, "[Image #1][Image #2]");
    assert_eq!(r2.state.attachments.len(), 2);
    assert_eq!(r2.state.next_image_id, 3);
}
```

> **Public-path note:** these tests import from `lingxi_tui::components::prompt_input::*`. Confirm `components` and `prompt_input` are `pub` in the crate (M7-06 made the submodule public; `lib.rs` re-exports `pub mod components`). If the test crate cannot reach them, expose a thin `pub use` in `lib.rs` matching how M7-08's `vim_behavior` test reaches `vim::*`.

- [ ] **Step 2: Run to verify pass**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test image_paste_test`
Expected: PASS — all 3 paste behavior tests.

- [ ] **Step 3: Run the full tui test set to confirm no regression**

Run: `cd lingxi-core && cargo test -p lingxi-tui`
Expected: PASS — history-search + image-paste + all M6/M7-06/M7-08 tui tests green. Re-run any known flake (parent spec §5.4) once if it trips.

- [ ] **Step 4: Commit**

```bash
cd lingxi-core && git add crates/tui/tests/image_paste_test.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T10): paste behavior tests — single-block, no per-line submit, image ref + counter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Overlay snapshot + telemetry note (0 events)

**Files:**
- Create: `crates/tui/tests/render_history_search.rs` (insta snapshot of the overlay row)
- Modify: `crates/tui/src/telemetry.rs` (doc note only — no new event)

The snapshot locks the two byte-locked labels + the gap layout. The telemetry note records that M7-10 adds 0 events and that the `tengu_tui_search_opened` candidate is deferred to the M7-16 audit (so the count stays 326 and the audit lesson — "report the real number" — is honored).

- [ ] **Step 1: Write the snapshot test**

Create `crates/tui/tests/render_history_search.rs`:

```rust
//! M7-10 snapshot: the Ctrl-R search overlay row in its three states
//! (idle/search, active query, no-match) — locks the byte-for-byte labels
//! from claude-code HistorySearchInput.tsx.

use iocraft::prelude::*;
use lingxi_tui::components::prompt_input::HistorySearchOverlay;

fn render_row(query: &str, failed: bool) -> String {
    let mut el = element! {
        HistorySearchOverlay(query: query.to_string(), failed_match: failed)
    };
    el.to_string() // iocraft Element renders to a String for snapshotting (M6 pattern)
}

#[test]
fn overlay_idle_label() {
    insta::assert_snapshot!("history_search_idle", render_row("", false));
}

#[test]
fn overlay_active_query() {
    insta::assert_snapshot!("history_search_query", render_row("cargo", false));
}

#[test]
fn overlay_no_match() {
    insta::assert_snapshot!("history_search_no_match", render_row("zzz", true));
}
```

> **Render-to-string note:** match the exact M6/M7 snapshot idiom for rendering an iocraft element to text (M6 snapshots use the project's established helper — `grep -rn "assert_snapshot\|to_string\|render_to_string" crates/tui/tests | head` and copy whatever `render_placeholder.rs`/M7-04 use; do not invent `el.to_string()` if the project uses a different helper such as `iocraft`'s canvas render or a `lingxi_tui::test_support::render(...)`).

- [ ] **Step 2: Run to generate + accept the snapshots**

Run: `cd lingxi-core && cargo test -p lingxi-tui --test render_history_search`
Expected: FAIL first run (new snapshots). Review the `.snap.new` files: `history_search_idle` must contain `search prompts:`, `history_search_query` must contain `search prompts: cargo`, `history_search_no_match` must contain `no matching prompt: zzz`. Then accept:

```bash
cd lingxi-core && cargo insta accept
```

Re-run: `cargo test -p lingxi-tui --test render_history_search` → PASS.

- [ ] **Step 3: Add the telemetry doc note (no new event)**

In `crates/tui/src/telemetry.rs`, add a comment near the event constants:

```rust
// M7-10 (history search + image paste) adds ZERO telemetry events. Baseline
// stays 326. The `tengu_tui_search_opened` candidate (Ctrl-R open count) is
// DEFERRED to the M7-16 telemetry audit, which locks the real total — per the
// M6 "330-vs-326, report the real number" lesson (parent spec §2.7).
```

- [ ] **Step 4: Confirm the event count is unchanged**

Run: `cd lingxi-core && cargo test --workspace event 2>&1 | grep -i "event\|326" | head` (or the established event-count test). Confirm `ALL_EVENT_NAMES.len()` is still 326 — M7-10 registered no new names.
Expected: the event-name count test (wherever it lives) still passes at 326.

- [ ] **Step 5: Commit**

```bash
cd lingxi-core && git add crates/tui/tests/render_history_search.rs crates/tui/tests/snapshots/ crates/tui/src/telemetry.rs
git commit -m "$(cat <<'EOF'
plan(M7-10 T11): history-search overlay snapshot (3 states) + telemetry 0-event note

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Workspace gate + tag `m7.10`

**Files:**
- None (verification + tag only).

This is the final task. It runs the full workspace gate **from inside `lingxi-code/`** (the toolchain pins 1.82.0; running from repo root uses the host toolchain → spurious lint noise — this bit M6-08, parent spec §5.4), then cuts the annotated tag.

- [ ] **Step 1: fmt + clippy**

Run (all from inside `lingxi-code/`):

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. Likely lints to fix inline here: `needless_return` on the early-return overlay branches (allow or restructure), `module_name_repetitions` on `HistorySearchState`/`HistorySearchOverlay` (allow at the item), `must_use_candidate` (already annotated). Re-commit any fmt/clippy fixes as `plan(M7-10 T12): fmt + clippy`.

- [ ] **Step 2: Full test suite**

```bash
cargo test --workspace
```

Expected: PASS. Known flakes (allowed rerun, parent spec §5.4): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests. If only these fail, rerun the specific test; do not treat as a gate failure.

- [ ] **Step 3: Targeted M7-10 suite confirmation**

```bash
cargo test -p lingxi-tui --lib history_search::
cargo test -p lingxi-tui --lib image_paste::
cargo test -p lingxi-tui --test history_search_test --test image_paste_test --test render_history_search
cargo test -p lingxi-tui --test focus_trap_test
```

Expected: all history_search + image_paste unit mods, all three M7-10 integration tests, and the focus-trap regression PASS.

- [ ] **Step 4: Cross-platform compile gate (5 targets)**

```bash
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```

Expected: green for all 5 (same posture as v0.7.0). `history_search.rs` + `image_paste.rs` are pure std + crossterm/iocraft — no platform-specific code, so this should pass cleanly. If a target's toolchain isn't installed, note it and skip per the established v0.7.0 convention.

- [ ] **Step 5: Tag `m7.10`**

```bash
git tag -a m7.10 -m "M7-10 — Ctrl-R history search (priority-3 input overlay) + bracketed-paste coalescing + image-ref detection ([Image #N], detection only)"
```

(NO push to remote. Local tag only — parent spec §6.4.)

- [ ] **Step 6: Verify the tag exists locally**

```bash
git tag -l 'm7.10'
git show --stat m7.10 | head -20
```

Expected: `m7.10` listed; `git show` points at the T11 (or T12 fmt/clippy) commit.

---

## Self-Review

**1. Spec coverage** — mapping the prompt's "WHAT M7-10 SHIPS" + "TESTS REQUIRED" to tasks:

| Prompt requirement | Task |
|---|---|
| `history_search.rs` — Ctrl-R reverse incremental search; open enters search mode; typing filters (most-recent first); Ctrl-R cycles older; Enter accepts into prompt; Esc restores pre-search prompt; overlay indicator (match + query) | T1 (state+open), T2 (filter), T3 (cycle/accept/cancel+router), T4 (wire+overlay row+behavior) |
| `image_paste.rs` — bracketed paste captured as one unit (multi-line no per-line submit); image detect (path/escape) → `[Image #N]` + attachment metadata; NO inline display | T5 (primitives+types), T6 (process_paste+refs+metadata), T7 (coalescer), T8 (apply block), T9 (wire), T10 (behavior) |
| Live-key routing: history-search priority 3/4 (overlay owns keys) via single `handle_live_key` | T4 (priority-3 branch below permission trap; no parallel path) |
| iocraft/crossterm 0.29 paste surfacing investigated; root.rs Paste arm | T9 + "Critical iocraft fact" (iocraft has NO Paste event → burst coalescer instead of a Paste arm; documented) |
| Telemetry baseline 326; 0 events; search-event candidate deferred to M7-16 | T11 (doc note + count confirm) |
| Test: history search open/filter/cycle/accept/restore | T4 (`ctrl_r_opens_then_filters_then_accepts_into_prompt`, `ctrl_r_cycles_to_older_match`, `esc_restores_pre_search_prompt`) |
| Test: history search empty / no-match | T4 (`empty_history_open_has_no_match`, `no_match_state_keeps_overlay_open_and_match_none`) |
| Test: bracketed paste multi-line = one block, no per-line submit | T10 (`multiline_paste_coalesces_to_single_block`) + T7 coalescer unit tests |
| Test: image payload → `[Image #N]` + metadata; counter increments | T10 (`image_payload_inserts_ref_and_records_metadata`, `counter_increments_for_multiple_images`) + T6 unit tests |
| Test: while history-search active, normal edit keys captured by search | T4 (`while_active_normal_edit_keys_are_captured_by_search`) |
| Workspace gate (cd lingxi-core) + tag `m7.10` | T12 |
| claude-code literal lock (HistorySearchInput / inputPaste / PromptInput paste wiring) | T3/T4 (`search prompts:` / `no matching prompt:`), T5/T6 (`[Image #N]`, `IMAGE_EXTENSION_REGEX`), T6 (usePasteHandler split) |

**2. Placeholder scan:** searched for `TBD`, `TODO`, `implement later`, `fill in details`, `add appropriate`, `handle edge cases`, `similar to Task` — none present. Every code step shows complete code. The two execution-time "confirm the helper name" notes (test `AppState` constructor, render-to-string idiom, `use_state` vs `use_ref` for the `Rc`) are not placeholders — they are explicit instructions to match an existing, already-shipped project idiom, with the grep command to find it; the surrounding code is complete.

**3. Type consistency:** `HistorySearchState`, `HsKeyOutcome`, `hs_open/hs_push_char/hs_backspace/hs_cycle/hs_accept`, `recompute_match`, `handle_history_search_key`, `HistorySearchOverlay`/`HistorySearchOverlayProps` are defined once and referenced consistently (T1-T4, wired in T4). `is_image_path`, `format_image_ref`, `Attachment`/`AttachmentKind`/`PasteState`/`PasteOutcome`/`PasteApply`, `process_paste`, `PasteCoalescer`/`BURST_WINDOW`, `apply_paste_block` are defined once (T5-T8) and used in T9/T10. The `handle_history_search_key(HistorySearchState, &KeyEvent, &[String]) -> HsKeyOutcome` signature is stable T3→T4. `apply_paste_block(&str, usize, &str, PasteState) -> PasteApply` is stable T8→T9→T10. `recompute_match(&[String], &str, Option<usize>)` is consistent T2 (None) and T3 (cycle passes `match_index`). `PasteState { next_image_id, attachments }` defaults to id 1 (T5) and the counter assertions (T6/T10) match.

**Notes / decisions for the executor:**
- **No paste event in iocraft 0.8.3 — coalescer, not a Paste arm.** Verified against the locally-resolved 0.8.3 (`terminal.rs:81` enum = Key/FullscreenMouse/Resize only; `:363-383` event_stream drops non-Key/Mouse/Resize; no `EnableBracketedPaste`). The burst coalescer (50ms `BURST_WINDOW`) is the chosen substitute. This is the single most important fact for the executor — re-verify it (the spec's "check how iocraft surfaces paste" was the explicit ask). If a future iocraft bump adds `TerminalEvent::Paste`, the coalescer becomes a fallback and the new arm calls `apply_block` directly — a clean upgrade path.
- **History store reuse.** Search reads `AppState.history: Vec<String>` (most-recent at end) — the exact M6 store that Up/Down already use (`app.rs:157-173`). No new history store; `recompute_match` walks it `.rev()` for newest-first.
- **Accept-with-no-match.** Following claude-code's `HistorySearchInput` (the `TextInput` value is what's committed), Enter with no match commits the typed query verbatim, not an empty string. Locked in `hs_accept`.
- **Ref insertion only (R8).** Attachments record the source path/`"clipboard"` only — no byte reads, base64, resize, or API wiring. Inline display + turn-wiring are M8. The empty-payload macOS clipboard-image case is *detected* (a flushed empty block with the macOS path heuristic) but M7-10 only records `source: "clipboard"` if a non-path image signal is present; with no clipboard read in scope, an empty paste simply inserts nothing — documented, not a bug.
- **Large-text-paste truncation ref deferred.** `[Pasted text #N +M lines]` (claude-code `maybeTruncateInput`, 10k-char threshold) is NOT implemented; M7-10 inserts pasted text verbatim and only swaps image lines. The format is documented in References so M8 can add it.
- **`stripBackslashEscapes` not ported.** Shell-escape unescaping of dragged paths is M8; a backslash-bearing path fails `is_image_path` (safe degrade).

**Gaps surfaced (raise before execution if blocking):**
1. **Hard prerequisite:** M7-06 must have landed the `prompt_input/` submodule (`mod.rs` editor core + `footer.rs`) and made `components::prompt_input` public. If `prompt_input.rs` is still a single file (not a dir), this plan cannot start — the new files have nowhere to live. (At plan-writing time the submodule does NOT yet exist; `prompt_input.rs` is still the M6 single file.)
2. **Test helper + render-to-string idiom + `use_state` non-Copy:** three execution-time confirmations (flagged inline with the exact grep). All are "match the existing idiom," not new design.
3. **`active_screen` priority-2 interaction:** if a screen sub-plan (M7-11..14) has already added `active_screen` to `AppState`/`handle_live_key`, the history-search branch goes below it; otherwise directly after the permission trap. The M7-16 final review (parent spec §5.6) explicitly probes the "paste while a screen is open" seam — the coalescer's `overlay_active` guard (extended to `active_screen` when it exists) is the hook for that.

**End of M7-10 plan.**
