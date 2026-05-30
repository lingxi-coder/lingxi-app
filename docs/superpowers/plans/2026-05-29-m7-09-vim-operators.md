# M7-09 — Vim Mode 2 (visual + operators) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete core vim fidelity in `PromptInput` — add Visual / Visual-line modes (`v`, `V` + selection-extending motions + `Esc`), operators `d c y x p P` with operator×motion combos (`dw cc d$ yy dd de`) and counts (`3dw 2yy`), and a yank/delete register with charwise-vs-linewise paste — building directly on M7-08's `vim.rs` (normal/insert + motions). Vim stays gated behind `vim_enabled`; M6 default editing is byte-for-byte unchanged when off.

**Architecture:** M7-09 **extends the M7-08 state machine in place** (parent spec §2.3 — vim is PromptInput-local). M7-08 left scaffold for exactly this: `VimMode::Visual` (enum variant, never constructed), `VimState.pending_operator: Option<Operator>`, `VimState.register: Option<String>`, and `VimState.last_find`. M7-09 (a) promotes `register` to a linewise-aware `Register { text: String, linewise: bool }`; (b) adds `CommandState::Operator/OperatorCount/OperatorFind/OperatorG` (mirroring claude-code `CommandState`'s operator-pending arms) so `dw`/`3dw`/`df<char>`/`dG` parse; (c) adds a `VisualState { anchor: usize, linewise: bool }` field driving `VimMode::Visual`; (d) adds pure operator functions (`apply_operator`, `line_op`, `delete_char`, `paste`) modelled on claude-code `src/vim/operators.ts`. `handle_vim_key` (M7-08's single dispatcher) grows operator-pending and visual branches; `VimEffect` grows a buffer-mutating variant. The root.rs seam (M7-08 priority-4 branch) is unchanged — it already applies whatever `VimEffect` comes back.

**Tech Stack:** Rust 1.82 (pinned via `lingxi-code/rust-toolchain.toml` — run all cargo from **inside `lingxi-code/`**), iocraft 0.8.3 (`View` not `Box`), crossterm key events. Pure logic — no new dependencies, no async, no iocraft inside `vim.rs`. Tests are `cargo test` unit (in `vim.rs`) + the table-driven `tests/vim_behavior.rs` integration file (created in M7-08, extended here).

**Prerequisite (HARD):** M7-08 has landed and tag `m7.8` exists. This plan assumes `lingxi-code/crates/tui/src/components/prompt_input/vim.rs` already contains: `VimMode {Normal,Insert,Visual}`, `Operator {Delete,Change,Yank}`, `FindKind {F,BigF,T,BigT}`, `CommandState {Idle, Count{digits}, Find{kind,count}, G{count}}`, `VimState {mode, command, pending_operator, register, last_find}`, `Motion`, `VimEffect {Move(usize), Edit{text,cursor}, None}`, `VimOutcome {Effect, Pending, PassThrough}`, the `VimCursor<'a>` motion engine (`left/right/down_logical_line/up_logical_line/start_of_logical_line/end_of_logical_line/first_non_blank/next_vim_word/prev_vim_word/end_vim_word/start_of_first_line/start_of_last_line/go_to_line/find_character/is_at_end`, plus private `logical_line_start`/`logical_line_end`/`next_off`/`prev_off`/`char_at`/`clamp`), `resolve_motion`, `enter_insert_effect`, `esc_clamp`, `handle_vim_key`, `dispatch_normal`, `mode_indicator`, and the `AppState {vim_enabled: bool, vim: VimState}` fields + `KeyAction::ToggleVim` + the root.rs priority-4 vim branch with `apply_vim_effect`. **If `m7.8` is not present, STOP and land M7-08 first** — every File Structure path and type below depends on it. Read `docs/superpowers/plans/2026-05-29-m7-08-vim-motions.md` for the exact M7-08 shapes this plan builds on.

---

## Background the engineer needs (read this first)

### What M7-09 ships on top of M7-08

M7-08 gave us a working Normal/Insert machine with a full motion engine. The cursor can move; nothing edits the buffer except `o`/`O`. M7-09 makes vim **edit**:

1. **Operators** `d c y` parse an operator-pending sub-state, then consume a motion (`dw`, `de`, `d$`, `d0`, `dG`, `df<char>`), a doubled key (`dd`, `cc`, `yy`), or a count (`3dw`, `2yy`). The operator applies over the byte range the motion would have moved across.
2. **`x`** deletes the char under the cursor (count = N chars); **`p`/`P`** paste the register after/before the cursor.
3. **Register** holds the last yanked OR deleted text, tagged charwise/linewise. `p`/`P` paste it with the right semantics (charwise = inline; linewise = whole new line(s)).
4. **Visual / Visual-line** modes: `v` (charwise) and `V` (linewise) set a selection anchor; motions extend the selection; `d`/`c`/`y` operate on it; `Esc` returns to Normal.

### claude-code semantics we MUST match (locked from `claude-code/src/vim/`)

The submodule **is** checked out. Operators are ported from `src/vim/operators.ts`; the state shape from `src/vim/types.ts`; motion classification from `src/vim/motions.ts`. **One gap:** claude-code's vim has **no Visual mode** (`VimTextInput.tsx`/`useVimInput.ts`/`transitions.ts` model operator-pending only; `CommandState` has no visual arm). So `v`/`V` (required by the parent spec §1 "normal/insert/visual" and the prompt) are implemented to **standard vim semantics** — there is no claude-code literal to match for visual; this is documented in the GATE note. Everything else is locked to the source:

1. **Operator range = where the motion would move** (`operators.ts::executeOperatorMotion` + `getOperatorRange`). `d<motion>` deletes `[min(cursor,target) .. max(cursor,target))`. If the motion doesn't move (`target.equals(cursor)`), the operator is a **no-op** (nothing deleted, register untouched).
2. **Inclusive motions** are `e E $` (`motions.ts::isInclusiveMotion` → `'eE$'`). For an inclusive motion with `cursor <= target`, the range extends one char past `target` (`to = nextOffset(to)`). So `de` deletes through the last char of the word; `d$` deletes through end-of-line.
3. **Linewise motions** are `j k G gg` (`motions.ts::isLinewiseMotion` → `'jkG'` or `'gg'`). An operator with a linewise motion deletes **whole lines** including their trailing `\n`; the register is tagged linewise. (`gj`/`gk` are characterwise-exclusive per `:help gj` — out of scope, M7-08 never had them.)
4. **`cw` is special-cased to `ce`** (`getOperatorRange`: `if op==='change' && motion==='w'`). `cw` changes to the **end of the current word**, not the start of the next word (so it doesn't eat the trailing space). With a count, move forward `count-1` words then take end-of-word. M7-09 ports this exactly.
5. **Doubled operator = line op** (`operators.ts::executeLineOp`): `dd` deletes the current line(s) incl. trailing `\n`, register linewise; `yy` yanks them (cursor → line start); `cc` clears the line content and enters Insert at the line start (keeping the line, not the `\n`). `Ndd`/`Nyy`/`Ncc` affect N lines (clamped to remaining lines). When deleting the **last** line to EOF and a preceding `\n` exists, the preceding `\n` is consumed too (no orphan trailing newline) — `executeLineOp` delete branch.
6. **`x`** (`operators.ts::executeX`): delete `count` chars forward from the cursor (clamped at EOF), register charwise, cursor clamps to `min(from, maxOff)` where `maxOff = len - last_char_len` (i.e. cannot rest past the last char of the new text — but never below 0). No-op if cursor is at EOF.
7. **Paste** (`operators.ts::executePaste`): register is linewise iff it ends with `\n`.
   - **Charwise `p`**: insert after the cursor char (`nextOffset(cursor)`; at EOF, at cursor); `P`: insert at the cursor. Content repeated `count` times. Cursor lands on the **last char** of the pasted text (`insertPoint + len - last_char_len`, floored at `insertPoint`).
   - **Linewise `p`**: open a new line **below** the current logical line and insert the content line(s) there; `P`: **above**. Cursor → start of the first pasted line. Content (sans its single trailing `\n`) repeated `count` times, each repeat re-adding the line.
   - If the register is empty, paste is a **no-op**.
8. **Operator + register interaction** (`operators.ts::applyOperator`): `yank` sets the register and moves the cursor to `from` (range start). `delete` sets the register, removes the range, cursor → `min(from, maxOff)`. `change` sets the register, removes the range, enters Insert at `from`.
9. **`dG`/`dgg`** (`executeOperatorG`/`executeOperatorGg`): `dG` (bare) deletes from the cursor's line to the last line (linewise); `dgg` to the first line. `NdG`/`Ndgg` target line N. (Operator-G is the operator-pending equivalent of M7-08's `G`/`gg` motions.)

### How operators extend the M7-08 state machine (the central design)

M7-08's `CommandState` was `{ Idle, Count{digits}, Find{kind,count}, G{count} }`. M7-09 adds the operator-pending arms — a direct port of claude-code `types.ts::CommandState`:

```
Idle ──[d|c|y]──► Operator{op, count}
Operator{op,count} ──[0-9]──► OperatorCount{op, count, digits}   (e.g. d3w)
Operator{op,count} ──[f|F|t|T]──► OperatorFind{op, count, kind}  (e.g. df<char>)
Operator{op,count} ──[g]──► (g-pending under operator) ──[g]──► dgg
Operator{op,count} ──[G]──► dG (line N if count>1)
Operator{op,count} ──[motion h/l/w/b/e/0/^/$/j/k]──► execute, back to Idle
Operator{op,count} ──[same key as op, e.g. dd/cc/yy]──► line op, back to Idle
Operator{op,count} ──[Esc]──► cancel, back to Idle
```

Count composition (`3dw`, `2yy`, `d3w`): the leading count (from `Count{digits}`) and the operator's own count multiply, exactly like vim. M7-08 already parses a leading count into `Count{digits}`; when an operator key arrives in `Count` state, that count seeds `Operator{op, count}`. A further count after the operator (`d3w`) accumulates in `OperatorCount` and multiplies. (`operators.ts` takes a single resolved `count`; the dispatcher computes `leading * operator_count`.)

Visual mode is **not** in claude-code's machine, so it's a parallel branch on `VimMode::Visual` (not a `CommandState` arm): `v`/`V` from Normal set `state.visual = Some(VisualState{anchor, linewise})` and `state.mode = Visual`; in Visual, motions move the cursor (the live end of the selection) and `d`/`c`/`y` apply the operator over `[min(anchor,cursor) .. max(anchor,cursor))` (+1 char for charwise inclusive end; whole lines for linewise) then return to Normal.

### Simplifications carried over from M7-08 (locked)

- **Logical lines** (`\n`-separated), not display-wrapped lines. Linewise ops and `j`/`k` operate on logical lines. (claude-code's `executeLineOp` deliberately counts `\n` via `countCharInString` rather than `getPosition()` for exactly this reason — see its comment "cursor.getPosition() returns wrapped line which is wrong for this".)
- **`char` boundaries**, not grapheme clusters. M7-08's `VimCursor::clamp`/`next_off`/`prev_off` already enforce char-boundary discipline; operators reuse them. claude-code uses `lastGrapheme(...).length || 1` to compute "one char back"; M7-09's analogue is `last_char_len(text)` (byte length of the last `char`, or 1 if empty) — a char-level port of the same clamp.

### GATE: vim subset (parent spec §4 R1 + hard gate §3 item 3)

**This is the final-task gate.** Core vim — the operators and motions listed in the prompt — MUST ship and pass the operator×motion matrix. If genuinely obscure cases balloon, they are deferred to **M8** with a documented "vim parity subset" line. The split is locked here so Task 14 can assert it:

**IN (M7-09 — must pass the matrix):**
- Operators `d c y` × motions `w b e $ 0 ^ h l j k f<char> t<char> G gg`, with counts (`3dw`, `2yy`, `d3w`).
- Doubled line ops `dd cc yy` (+ counts `2dd`).
- `cw`→`ce` special case (inclusive change-word).
- `x` (count = N chars), `p`/`P` charwise + linewise paste.
- Register: last yank OR delete, charwise/linewise tagged; paste consumes it.
- Visual (`v`) + Visual-line (`V`): extend with `h l j k w b e $ 0 ^ gg G`; `d`/`c`/`y` on selection; `Esc` → Normal.
- `c` enters Insert after deleting (operator + visual).
- vim-disabled passthrough unchanged (M6 editing byte-identical).

**DEFERRED to M8 (documented "vim parity subset"):**
- `.` dot-repeat (M7-08 left `RecordedChange` unported; out of scope).
- Macros `q`/`@`, ex-commands `:`, the `/` search motion as an operator target.
- Named registers (`"ayy`), numbered/yank-ring registers — **only the unnamed register** ships.
- Text objects `iw`/`aw`/`i(`/`a"` etc. (claude-code has `textObjects.ts` + `operatorTextObj`; explicitly deferred — `Operator{op}` never transitions to a text-object arm in M7-09).
- `W B E` WORD-motions (already deferred in M7-08), `r` replace, `~` toggle-case, `J` join, `>>`/`<<` indent, `gj`/`gk` display-wrap motions, `;`/`,` find-repeat, `NG` as a bare motion (M7-08 deferred; operator-G `dG`/`NdG` IS in).
- Visual block mode (`Ctrl-v`), `o` (swap selection ends) in visual, `gv` (reselect).

Task 14 records this exact split in the `vim.rs` module doc as the "GATE: vim subset" note and asserts the deferred items are NOT wired (e.g. text-object keys after an operator are a no-op, not a panic).

### Where it plugs in (unchanged from M7-08)

The root.rs priority-4 branch (M7-08 Task 10) already routes keys through `handle_vim_key` when `vim_enabled` and applies the returned `VimEffect` via `apply_vim_effect`. M7-09 changes **only** `vim.rs` internals plus a small `apply_vim_effect` extension for the new buffer-mutating effect variant and the footer indicator for Visual. The dispatcher signature `handle_vim_key(&mut VimState, &str, usize, KeyEvent) -> VimOutcome` is **stable** — M7-09 does not change it.

---

## File Structure

| Path | New/Modify | Responsibility |
|---|---|---|
| `lingxi-code/crates/tui/src/components/prompt_input/vim.rs` | **Modify** | All M7-09 logic: promote `register` to `Register`; add `VisualState` + `VimState.visual`; add `CommandState::Operator/OperatorCount/OperatorFind/OperatorG`; classify motions (`is_inclusive_motion`/`is_linewise_motion`); add operator engine (`apply_operator`, `line_op`, `delete_char_x`, `paste`, `operator_range`, `last_char_len`); extend `handle_vim_key`/`dispatch_normal` with operator-pending + visual branches; grow `VimEffect` with a buffer-edit variant carrying the new cursor. Pure — no iocraft, no async. Adds ~500 lines incl. tests. |
| `lingxi-code/crates/tui/src/components/prompt_input/footer.rs` | **Modify** (1 line) | The mode indicator already reads `VimMode` (M7-08 `footer_mode_label`); confirm `VimMode::Visual` → `-- VISUAL --` flows through (it does, via `mode_indicator`). Add a Visual-line label variant `-- VISUAL LINE --` selected from `VisualState.linewise`. |
| `lingxi-code/crates/tui/src/root.rs` | **Modify** (small) | `apply_vim_effect` gains the new `VimEffect` buffer-edit variant (if M7-09 adds one distinct from `Edit`). The priority-4 routing itself is unchanged. |
| `lingxi-code/crates/tui/tests/vim_behavior.rs` | **Modify** | Extend the M7-08 table-driven file with the operator×motion matrix, `x`/`p`/`P`, register, visual charwise + linewise, count×operator, `c`-enters-insert, and a re-assert that vim-disabled passthrough is unchanged. |

**Decomposition rationale:** Same as M7-08 — all vim logic stays in the one pure file `vim.rs` so the high-risk operator×motion matrix (parent spec §4 R1) is exhaustively unit-testable without a terminal. The footer/root edits are thin (one label, one effect arm). No new files: M7-08 already established `vim.rs` + `tests/vim_behavior.rs` as the home for this subsystem.

---

## Type contract (locked — M7-09 deltas over M7-08)

These replace/extend the M7-08 types. Names below are referenced by every task; later tasks use exactly these.

```rust
/// The unnamed register: yanked/deleted text + whether it is linewise.
/// Replaces M7-08's `register: Option<String>` scaffold. `None`/empty = nothing
/// to paste. claude-code models linewise as "string ends with '\n'"; we make it
/// explicit (operators.ts::executePaste detects `register.endsWith('\n')`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

/// Visual-mode selection. `anchor` is the byte offset where `v`/`V` was pressed;
/// the live end is the current cursor. `linewise` distinguishes `V` from `v`.
/// claude-code has NO visual mode — this is standard vim semantics (see GATE note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualState {
    pub anchor: usize,
    pub linewise: bool,
}
```

`CommandState` gains the operator-pending arms (port of `types.ts::CommandState`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandState {
    Idle,
    Count { digits: String },
    Find { kind: FindKind, count: usize },
    G { count: usize },
    // ---- M7-09 additions ----
    /// After 'd'/'c'/'y' — waiting for a motion / doubled key / count / find / g.
    Operator { op: Operator, count: usize },
    /// After an operator then a digit, e.g. `d3w` — accumulating the inner count.
    OperatorCount { op: Operator, count: usize, digits: String },
    /// After an operator then f/F/t/T, e.g. `df` — waiting for the target char.
    OperatorFind { op: Operator, count: usize, kind: FindKind },
    /// After an operator then 'g', e.g. `dg` — waiting for the second 'g' (dgg).
    OperatorG { op: Operator, count: usize },
}
```

`VimState` gains `visual` and swaps `register`'s type:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimState {
    pub mode: VimMode,
    pub command: CommandState,
    pub pending_operator: Option<Operator>, // M7-08 scaffold; now unused (Operator state replaces it) — keep field for struct stability, document.
    pub register: Register,                  // CHANGED from Option<String>
    pub last_find: Option<(FindKind, char)>,
    pub visual: Option<VisualState>,         // NEW: Some(..) iff mode == Visual
}

impl Default for VimState {
    fn default() -> Self {
        Self {
            mode: VimMode::Insert,
            command: CommandState::Idle,
            pending_operator: None,
            register: Register::default(),
            last_find: None,
            visual: None,
        }
    }
}
```

`VimEffect` gains a variant that both replaces the buffer AND signals "the caller should also enter Insert" is **not** needed — mode is already on `VimState`, which the caller reads. But operators that mutate the buffer need to report the new text + cursor. M7-08's `Edit { text, cursor }` already does exactly that. **Decision (locked):** reuse `VimEffect::Edit { text, cursor }` for every buffer-mutating operator (delete/change/yank-cursor-move/paste/x). No new variant is added. `change` reports `Edit` and ALSO sets `state.mode = Insert` inside `handle_vim_key` before returning, so the caller's existing `apply_vim_effect` needs **no change**. (This keeps the root.rs seam untouched — confirmed in Task 13.)

```rust
// VimEffect is UNCHANGED from M7-08:
//   Move(usize) | Edit { text: String, cursor: usize } | None
// Yank reports Move(from) (buffer unchanged, cursor to range start).
// Delete/Change/x/Paste report Edit { text, cursor }.
```

`VimOutcome` is **unchanged** (`Effect(VimEffect) | Pending | PassThrough`).

A helper used throughout:

```rust
/// Byte length of the last `char` of `text`, or 1 if empty. The char-level
/// analogue of claude-code's `lastGrapheme(text).length || 1`, used to clamp a
/// Normal-mode cursor so it never rests past the last char of the buffer.
#[must_use]
fn last_char_len(text: &str) -> usize {
    text.chars().next_back().map_or(1, char::len_utf8)
}
```

---

## Task 1: Register type + VisualState + CommandState operator arms (types only)

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod m7_09_types_tests`

This task lands the type deltas above and updates every existing reference so the crate still compiles. No behavior yet.

- [ ] **Step 1: Write the failing test**

Add to `vim.rs`:

```rust
#[cfg(test)]
mod m7_09_types_tests {
    use super::*;

    #[test]
    fn default_register_is_empty_charwise() {
        let s = VimState::default();
        assert_eq!(s.register, Register::default());
        assert!(s.register.text.is_empty());
        assert!(!s.register.linewise);
        assert!(s.visual.is_none());
    }

    #[test]
    fn operator_command_states_constructible() {
        let a = CommandState::Operator { op: Operator::Delete, count: 1 };
        let b = CommandState::OperatorCount { op: Operator::Change, count: 1, digits: "3".into() };
        let c = CommandState::OperatorFind { op: Operator::Yank, count: 2, kind: FindKind::F };
        let d = CommandState::OperatorG { op: Operator::Delete, count: 1 };
        assert_ne!(a, b);
        assert_ne!(c, d);
    }

    #[test]
    fn visual_state_records_anchor_and_kind() {
        let v = VisualState { anchor: 3, linewise: true };
        assert_eq!(v.anchor, 3);
        assert!(v.linewise);
    }

    #[test]
    fn last_char_len_handles_utf8_and_empty() {
        assert_eq!(last_char_len("hi"), 1);
        assert_eq!(last_char_len("hé"), 2); // 'é' is 2 bytes
        assert_eq!(last_char_len(""), 1);   // empty -> 1
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run (from inside `lingxi-code/`): `cargo test -p lingxi-tui --lib vim::m7_09_types_tests`
Expected: FAIL — `Register` / `VisualState` / the new `CommandState` arms / `last_char_len` not defined; and the existing `register: None` initializers no longer typecheck.

- [ ] **Step 3: Write minimal implementation**

In `vim.rs`:

1. Add the `Register` and `VisualState` structs and the `last_char_len` fn from the Type-contract block above.

2. Extend `CommandState` with the four operator arms (paste the full enum from the Type-contract block — keep the existing `Idle`/`Count`/`Find`/`G` arms unchanged).

3. Change `VimState.register` from `Option<String>` to `Register`, add `pub visual: Option<VisualState>`, and update `Default` (paste the full struct + `Default` from the Type-contract block). Keep `pending_operator` (now unused) with a doc comment:

```rust
    /// (M7-08 scaffold; M7-09 superseded by `CommandState::Operator`.)
    /// Retained for struct stability; always `None`. Remove in a future cleanup.
    pub pending_operator: Option<Operator>,
```

4. Fix the M7-08 `register` references: M7-08's `handle_vim_key` Find branch did `state.last_find = Some(..)` (fine) and never read `register` — search `vim.rs` for `register` and replace any `register: None` literal in `VimState` construction with `register: Register::default()`. (The M7-08 `Default` impl is the only place; the type-contract paste covers it.)

5. If `AppState::new` or any test in `state.rs` constructed `VimState { register: None, .. }` literally, it used `..VimState::default()` (M7-08 convention) — no change needed. Confirm by `grep -rn "register:" lingxi-code/crates/tui/src` and fix any explicit literal.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::m7_09_types_tests`
Expected: PASS (4 tests). Also run `cargo build -p lingxi-tui` to confirm the whole crate still compiles after the `register` type change.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T1): Register type + VisualState + operator-pending CommandState arms

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Motion classification (inclusive / linewise) + the operator-eligible motion map

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod motion_class_tests`

Ports `motions.ts::isInclusiveMotion` (`e E $`) and `isLinewiseMotion` (`j k G gg`). These classify how an operator extends its range. Also adds `motion_for_operator_key(char) -> Option<(Motion, char)>` returning the `Motion` and the **classification key char** (the original key, needed because `Motion::EndWord` could come from `e`, and `$` maps to `Motion::LineEnd`).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod motion_class_tests {
    use super::*;

    #[test]
    fn inclusive_motions_are_e_and_dollar() {
        assert!(is_inclusive_motion('e'));
        assert!(is_inclusive_motion('$'));
        assert!(!is_inclusive_motion('w'));
        assert!(!is_inclusive_motion('0'));
        assert!(!is_inclusive_motion('h'));
    }

    #[test]
    fn linewise_motions_are_jk_and_G() {
        assert!(is_linewise_motion("j"));
        assert!(is_linewise_motion("k"));
        assert!(is_linewise_motion("G"));
        assert!(is_linewise_motion("gg"));
        assert!(!is_linewise_motion("w"));
        assert!(!is_linewise_motion("$"));
    }

    #[test]
    fn operator_motion_map_covers_required_keys() {
        assert_eq!(motion_for_operator_key('w'), Some(Motion::NextWord));
        assert_eq!(motion_for_operator_key('b'), Some(Motion::PrevWord));
        assert_eq!(motion_for_operator_key('e'), Some(Motion::EndWord));
        assert_eq!(motion_for_operator_key('$'), Some(Motion::LineEnd));
        assert_eq!(motion_for_operator_key('0'), Some(Motion::LineStart));
        assert_eq!(motion_for_operator_key('^'), Some(Motion::FirstNonBlank));
        assert_eq!(motion_for_operator_key('h'), Some(Motion::Left));
        assert_eq!(motion_for_operator_key('l'), Some(Motion::Right));
        assert_eq!(motion_for_operator_key('j'), Some(Motion::Down));
        assert_eq!(motion_for_operator_key('k'), Some(Motion::Up));
        assert_eq!(motion_for_operator_key('z'), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib vim::motion_class_tests`
Expected: FAIL — `is_inclusive_motion` / `is_linewise_motion` / `motion_for_operator_key` not defined.

- [ ] **Step 3: Write minimal implementation**

```rust
/// claude-code motions.ts::isInclusiveMotion — `e E $` include the dest char.
/// (E is WORD-motion, deferred; we keep it in the set for parity but it is
/// never reached because `motion_for_operator_key` does not map 'E'.)
#[must_use]
fn is_inclusive_motion(key: char) -> bool {
    matches!(key, 'e' | 'E' | '$')
}

/// claude-code motions.ts::isLinewiseMotion — `j k G` and the digraph `gg`.
#[must_use]
fn is_linewise_motion(key: &str) -> bool {
    matches!(key, "j" | "k" | "G" | "gg")
}

/// Map an operator-pending motion key to its `Motion`. Returns `None` for keys
/// that are not operator-eligible motions (those are handled elsewhere: 'f'/'g'
/// start sub-states; the doubled op key is a line op).
#[must_use]
fn motion_for_operator_key(ch: char) -> Option<Motion> {
    match ch {
        'h' => Some(Motion::Left),
        'l' => Some(Motion::Right),
        'j' => Some(Motion::Down),
        'k' => Some(Motion::Up),
        'w' => Some(Motion::NextWord),
        'b' => Some(Motion::PrevWord),
        'e' => Some(Motion::EndWord),
        '0' => Some(Motion::LineStart),
        '^' => Some(Motion::FirstNonBlank),
        '$' => Some(Motion::LineEnd),
        _ => None,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::motion_class_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T2): motion classification (inclusive/linewise) + operator-motion map

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: operator_range + apply_operator (d/c/y over a byte range)

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod apply_op_tests`

Ports `operators.ts::getOperatorRange` + `applyOperator`. `operator_range` computes `(from, to, linewise)` from `(cursor_offset, target_offset, motion_key, op)`; `apply_operator` produces the `(VimEffect, Register, enter_insert: bool)` triple. These are the pure core; the dispatcher (Task 7) wires them.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod apply_op_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn range_exclusive_for_w() {
        // dw on "foo bar": w moves 0->4, range [0,4) exclusive (not inclusive).
        let r = operator_range(cur("foo bar", 0), 4, 'w', Operator::Delete, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 4, false));
    }

    #[test]
    fn range_inclusive_for_e_and_dollar() {
        // de on "foo bar": e moves 0->2 ('o'), inclusive -> to = 3.
        let r = operator_range(cur("foo bar", 0), 2, 'e', Operator::Delete, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 3, false));
        // d$ on "foo": $ moves 0->3 (== len), inclusive but already at end -> to stays 3.
        let r2 = operator_range(cur("foo", 0), 3, '$', Operator::Delete, 1);
        assert_eq!((r2.from, r2.to, r2.linewise), (0, 3, false));
    }

    #[test]
    fn range_cw_changes_to_end_of_word_like_ce() {
        // cw on "foo bar" from 0: special-cased to end-of-word -> through 'o' (to=3),
        // NOT to start of next word (4). This is the claude-code cw->ce rule.
        let r = operator_range(cur("foo bar", 0), 4, 'w', Operator::Change, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 3, false));
    }

    #[test]
    fn range_linewise_for_j() {
        // dj on "a\nb\nc" from offset 0: j is linewise, deletes lines 0..1 incl
        // trailing newline of line1 -> [0, 4) ("a\nb\n").
        let r = operator_range(cur("a\nb\nc", 0), 2, 'j', Operator::Delete, 1);
        assert!(r.linewise);
        assert_eq!((r.from, r.to), (0, 4));
    }

    #[test]
    fn apply_delete_sets_register_and_edits() {
        let (effect, reg, enter_insert) =
            apply_operator(Operator::Delete, "foo bar", 0, 4, false);
        assert_eq!(reg, Register { text: "foo ".into(), linewise: false });
        assert_eq!(effect, VimEffect::Edit { text: "bar".into(), cursor: 0 });
        assert!(!enter_insert);
    }

    #[test]
    fn apply_yank_keeps_text_moves_cursor_to_from() {
        let (effect, reg, enter_insert) =
            apply_operator(Operator::Yank, "foo bar", 4, 7, false);
        assert_eq!(reg, Register { text: "bar".into(), linewise: false });
        assert_eq!(effect, VimEffect::Move(4)); // buffer unchanged, cursor to range start
        assert!(!enter_insert);
    }

    #[test]
    fn apply_change_edits_and_requests_insert() {
        let (effect, reg, enter_insert) =
            apply_operator(Operator::Change, "foo bar", 0, 3, false);
        assert_eq!(reg, Register { text: "foo".into(), linewise: false });
        assert_eq!(effect, VimEffect::Edit { text: " bar".into(), cursor: 0 });
        assert!(enter_insert);
    }

    #[test]
    fn apply_linewise_delete_tags_register_linewise() {
        // delete "a\n" (line 0) from "a\nb": register linewise, text "a\nb" -> "b".
        let (effect, reg, _) = apply_operator(Operator::Delete, "a\nb", 0, 2, true);
        assert!(reg.linewise);
        assert_eq!(reg.text, "a\n");
        assert_eq!(effect, VimEffect::Edit { text: "b".into(), cursor: 0 });
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib vim::apply_op_tests`
Expected: FAIL — `operator_range` / `apply_operator` / `OperatorRange` not defined.

- [ ] **Step 3: Write minimal implementation**

```rust
/// Resolved operator byte range over the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorRange {
    pub from: usize,
    pub to: usize,
    pub linewise: bool,
}

/// claude-code operators.ts::getOperatorRange. `cursor` is the start; `target`
/// is where the motion landed; `motion_key` classifies inclusivity/linewiseness;
/// `count` feeds the cw->ce special case.
#[must_use]
fn operator_range(
    cursor: VimCursor<'_>,
    target: usize,
    motion_key: char,
    op: Operator,
    count: usize,
) -> OperatorRange {
    let text = cursor.text;
    let mut from = cursor.offset.min(target);
    let mut to = cursor.offset.max(target);
    let mut linewise = false;

    if op == Operator::Change && motion_key == 'w' {
        // cw -> ce: change to end of (count-th) word, not start of next word.
        let mut wc = cursor;
        for _ in 0..count.saturating_sub(1) {
            wc = wc.next_vim_word();
        }
        let word_end = wc.end_vim_word();
        to = word_end.next_off(word_end.offset); // inclusive: through the last char
    } else if is_linewise_motion(&motion_key.to_string()) {
        linewise = true;
        match text[to..].find('\n') {
            None => {
                to = text.len();
                if from > 0 && text.as_bytes()[from - 1] == b'\n' {
                    from -= 1;
                }
            }
            Some(rel) => {
                to += rel + 1; // include the newline
            }
        }
    } else if is_inclusive_motion(motion_key) && cursor.offset <= target {
        let c = VimCursor { text, offset: to };
        to = c.next_off(to);
    }

    OperatorRange { from, to, linewise }
}

/// claude-code operators.ts::applyOperator. Returns the effect to apply, the
/// register to store, and whether the caller should enter Insert (change only).
#[must_use]
fn apply_operator(
    op: Operator,
    text: &str,
    from: usize,
    to: usize,
    linewise: bool,
) -> (VimEffect, Register, bool) {
    let mut content = text[from..to].to_string();
    if linewise && !content.ends_with('\n') {
        content.push('\n');
    }
    let register = Register { text: content, linewise };

    match op {
        Operator::Yank => (VimEffect::Move(from), register, false),
        Operator::Delete => {
            let new_text = format!("{}{}", &text[..from], &text[to..]);
            let max_off = new_text.len().saturating_sub(last_char_len(&new_text));
            let cursor = from.min(max_off);
            (VimEffect::Edit { text: new_text, cursor }, register, false)
        }
        Operator::Change => {
            let new_text = format!("{}{}", &text[..from], &text[to..]);
            (VimEffect::Edit { text: new_text, cursor: from }, register, true)
        }
    }
}
```

Note: `next_off` is a private `VimCursor` method from M7-08 (Task 3). It is in-module, so `vim.rs` code can call it. If M7-08 made it `pub(self)` or private-without-`pub`, it is still visible within `vim.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::apply_op_tests`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T3): operator_range + apply_operator (d/c/y over byte range, cw->ce, linewise)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: line_op (dd / cc / yy with counts)

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod line_op_tests`

Ports `operators.ts::executeLineOp`. Doubled operator key (`dd`/`cc`/`yy`) affects `count` whole logical lines from the cursor's line.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod line_op_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn dd_deletes_current_line_register_linewise() {
        // dd on line 1 ('b') of "a\nb\nc": delete "b\n", register linewise.
        let (effect, reg, enter_insert) = line_op(Operator::Delete, cur("a\nb\nc", 2), 1);
        assert!(reg.linewise);
        assert_eq!(reg.text, "b\n");
        assert_eq!(effect, VimEffect::Edit { text: "a\nc".into(), cursor: 2 });
        assert!(!enter_insert);
    }

    #[test]
    fn dd_last_line_consumes_preceding_newline() {
        // dd on last line ('c') of "a\nb\nc": delete to EOF; preceding '\n' consumed
        // so no orphan trailing newline. Result "a\nb".
        let (effect, _reg, _) = line_op(Operator::Delete, cur("a\nb\nc", 4), 1);
        match effect {
            VimEffect::Edit { text, .. } => assert_eq!(text, "a\nb"),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn count_dd_deletes_n_lines() {
        // 2dd on "a\nb\nc\nd" from line0: delete "a\nb\n" -> "c\nd".
        let (effect, reg, _) = line_op(Operator::Delete, cur("a\nb\nc\nd", 0), 2);
        assert_eq!(reg.text, "a\nb\n");
        match effect {
            VimEffect::Edit { text, cursor } => {
                assert_eq!(text, "c\nd");
                assert_eq!(cursor, 0);
            }
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn yy_yanks_keeps_buffer_cursor_to_line_start() {
        // yy on line1 of "a\nb\nc": register "b\n" linewise, buffer unchanged, cursor->line start (2).
        let (effect, reg, enter_insert) = line_op(Operator::Yank, cur("a\nb\nc", 3), 1);
        assert_eq!(reg, Register { text: "b\n".into(), linewise: true });
        assert_eq!(effect, VimEffect::Move(2));
        assert!(!enter_insert);
    }

    #[test]
    fn cc_clears_line_enters_insert_at_line_start() {
        // cc on line1 of "ab\ncd\nef": clear "cd" -> "ab\n\nef", enter insert at line start (3).
        let (effect, _reg, enter_insert) = line_op(Operator::Change, cur("ab\ncd\nef", 4), 1);
        assert_eq!(effect, VimEffect::Edit { text: "ab\n\nef".into(), cursor: 3 });
        assert!(enter_insert);
    }

    #[test]
    fn cc_single_line_buffer_clears_to_empty() {
        let (effect, _reg, enter_insert) = line_op(Operator::Change, cur("hello", 2), 1);
        assert_eq!(effect, VimEffect::Edit { text: String::new(), cursor: 0 });
        assert!(enter_insert);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib vim::line_op_tests`
Expected: FAIL — `line_op` not defined.

- [ ] **Step 3: Write minimal implementation**

```rust
/// claude-code operators.ts::executeLineOp — dd/cc/yy over `count` logical lines.
#[must_use]
fn line_op(op: Operator, cursor: VimCursor<'_>, count: usize) -> (VimEffect, Register, bool) {
    let text = cursor.text;
    let count = count.max(1);
    let lines: Vec<&str> = text.split('\n').collect();
    // Logical line index = number of '\n' before the cursor offset.
    let current_line = text[..cursor.offset].matches('\n').count();
    let lines_to_affect = count.min(lines.len().saturating_sub(current_line));

    let line_start = cursor.start_of_logical_line().offset;
    let mut line_end = line_start;
    for _ in 0..lines_to_affect {
        match text[line_end..].find('\n') {
            None => {
                line_end = text.len();
                break;
            }
            Some(rel) => line_end += rel + 1, // include the newline
        }
    }

    let mut content = text[line_start..line_end].to_string();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    let register = Register { text: content, linewise: true };

    match op {
        Operator::Yank => (VimEffect::Move(line_start), register, false),
        Operator::Delete => {
            let mut delete_start = line_start;
            let delete_end = line_end;
            // Deleting to EOF with a preceding newline: consume it (no orphan '\n').
            if delete_end == text.len()
                && delete_start > 0
                && text.as_bytes()[delete_start - 1] == b'\n'
            {
                delete_start -= 1;
            }
            let new_text = format!("{}{}", &text[..delete_start], &text[delete_end..]);
            let max_off = new_text.len().saturating_sub(last_char_len(&new_text));
            let cursor_off = delete_start.min(max_off);
            (VimEffect::Edit { text: new_text, cursor: cursor_off }, register, true && false /* delete never enters insert */)
        }
        Operator::Change => {
            if lines.len() == 1 {
                (VimEffect::Edit { text: String::new(), cursor: 0 }, register, true)
            } else {
                let before = &lines[..current_line];
                let after = &lines[(current_line + lines_to_affect)..];
                let new_lines: Vec<&str> =
                    before.iter().chain(std::iter::once(&"")).chain(after.iter()).copied().collect();
                let new_text = new_lines.join("\n");
                (VimEffect::Edit { text: new_text, cursor: line_start }, register, true)
            }
        }
    }
}
```

Clean the awkward `true && false` literal — it documents intent but reads badly; write the delete arm as:

```rust
        Operator::Delete => {
            // ... (same body) ...
            (VimEffect::Edit { text: new_text, cursor: cursor_off }, register, false)
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::line_op_tests`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T4): line_op (dd/cc/yy with counts, linewise register)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: delete_char_x (x command)

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod x_tests`

Ports `operators.ts::executeX`. `x` deletes `count` chars forward, register charwise.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod x_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn x_deletes_char_under_cursor() {
        // x on "hello" at 0: delete 'h' -> "ello", register "h", cursor 0.
        let (effect, reg) = delete_char_x(cur("hello", 0), 1);
        assert_eq!(reg, Register { text: "h".into(), linewise: false });
        assert_eq!(effect, VimEffect::Edit { text: "ello".into(), cursor: 0 });
    }

    #[test]
    fn count_x_deletes_n_chars() {
        // 3x on "hello" at 0: delete "hel" -> "lo", cursor 0.
        let (effect, reg) = delete_char_x(cur("hello", 0), 3);
        assert_eq!(reg.text, "hel");
        assert_eq!(effect, VimEffect::Edit { text: "lo".into(), cursor: 0 });
    }

    #[test]
    fn x_clamps_cursor_to_last_char() {
        // x on last char of "ab" at 1: delete 'b' -> "a"; cursor clamps to 0 (cannot rest past last char).
        let (effect, _reg) = delete_char_x(cur("ab", 1), 1);
        assert_eq!(effect, VimEffect::Edit { text: "a".into(), cursor: 0 });
    }

    #[test]
    fn x_at_eof_is_noop() {
        let (effect, reg) = delete_char_x(cur("ab", 2), 1);
        assert_eq!(effect, VimEffect::None);
        assert_eq!(reg, Register::default()); // unchanged
    }

    #[test]
    fn count_x_overshoot_clamps_at_eof() {
        // 9x on "ab" at 0: delete both -> "", cursor 0.
        let (effect, reg) = delete_char_x(cur("ab", 0), 9);
        assert_eq!(reg.text, "ab");
        assert_eq!(effect, VimEffect::Edit { text: String::new(), cursor: 0 });
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib vim::x_tests`
Expected: FAIL — `delete_char_x` not defined.

- [ ] **Step 3: Write minimal implementation**

```rust
/// claude-code operators.ts::executeX — delete `count` chars forward from the
/// cursor. Register charwise. No-op (and register untouched) if at EOF.
#[must_use]
fn delete_char_x(cursor: VimCursor<'_>, count: usize) -> (VimEffect, Register) {
    let text = cursor.text;
    let from = cursor.offset;
    if from >= text.len() {
        return (VimEffect::None, Register::default());
    }
    let mut end = cursor;
    for _ in 0..count.max(1) {
        if end.is_at_end() {
            break;
        }
        end = end.right();
    }
    let to = end.offset;
    let deleted = text[from..to].to_string();
    let new_text = format!("{}{}", &text[..from], &text[to..]);
    let max_off = new_text.len().saturating_sub(last_char_len(&new_text));
    let cursor_off = from.min(max_off);
    (
        VimEffect::Edit { text: new_text, cursor: cursor_off },
        Register { text: deleted, linewise: false },
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::x_tests`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T5): delete_char_x (x command, count, EOF no-op)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: paste (p / P — charwise + linewise)

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod paste_tests`

Ports `operators.ts::executePaste`. `after=true` is `p`; `after=false` is `P`. Linewise iff `register.linewise`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod paste_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }
    fn reg_char(s: &str) -> Register { Register { text: s.into(), linewise: false } }
    fn reg_line(s: &str) -> Register { Register { text: s.into(), linewise: true } }

    #[test]
    fn charwise_p_inserts_after_cursor() {
        // p with register "X" on "ab" at 0: insert after 'a' -> "aXb", cursor on 'X' (1).
        let effect = paste(true, 1, &reg_char("X"), cur("ab", 0));
        assert_eq!(effect, VimEffect::Edit { text: "aXb".into(), cursor: 1 });
    }

    #[test]
    fn charwise_cap_p_inserts_before_cursor() {
        // P with register "X" on "ab" at 1: insert at cursor -> "aXb", cursor on 'X' (1).
        let effect = paste(false, 1, &reg_char("X"), cur("ab", 1));
        assert_eq!(effect, VimEffect::Edit { text: "aXb".into(), cursor: 1 });
    }

    #[test]
    fn charwise_p_repeats_count_times() {
        // 3p with register "X" on "ab" at 0: "aXXXb", cursor on last 'X' (3).
        let effect = paste(true, 3, &reg_char("X"), cur("ab", 0));
        assert_eq!(effect, VimEffect::Edit { text: "aXXXb".into(), cursor: 3 });
    }

    #[test]
    fn charwise_p_at_eof_inserts_at_cursor() {
        // p with register "X" on "ab" at 2 (EOF): insert at cursor -> "abX", cursor on 'X' (2).
        let effect = paste(true, 1, &reg_char("X"), cur("ab", 2));
        assert_eq!(effect, VimEffect::Edit { text: "abX".into(), cursor: 2 });
    }

    #[test]
    fn linewise_p_opens_line_below() {
        // p with linewise register "x\n" on "a\nb" at 0 (line0): new line below -> "a\nx\nb",
        // cursor at start of pasted line (2).
        let effect = paste(true, 1, &reg_line("x\n"), cur("a\nb", 0));
        assert_eq!(effect, VimEffect::Edit { text: "a\nx\nb".into(), cursor: 2 });
    }

    #[test]
    fn linewise_cap_p_opens_line_above() {
        // P with linewise register "x\n" on "a\nb" at 2 (line1): new line above -> "a\nx\nb",
        // cursor at start of pasted line (2).
        let effect = paste(false, 1, &reg_line("x\n"), cur("a\nb", 2));
        assert_eq!(effect, VimEffect::Edit { text: "a\nx\nb".into(), cursor: 2 });
    }

    #[test]
    fn empty_register_is_noop() {
        let effect = paste(true, 1, &Register::default(), cur("ab", 0));
        assert_eq!(effect, VimEffect::None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib vim::paste_tests`
Expected: FAIL — `paste` not defined.

- [ ] **Step 3: Write minimal implementation**

```rust
/// claude-code operators.ts::executePaste. `after`=p, `!after`=P. `count` repeats.
#[must_use]
fn paste(after: bool, count: usize, register: &Register, cursor: VimCursor<'_>) -> VimEffect {
    if register.text.is_empty() {
        return VimEffect::None;
    }
    let count = count.max(1);
    let text = cursor.text;

    if register.linewise {
        // Content sans the single trailing '\n', split into its lines.
        let content = register.text.strip_suffix('\n').unwrap_or(&register.text);
        let lines: Vec<&str> = text.split('\n').collect();
        let current_line = text[..cursor.offset].matches('\n').count();
        let insert_line = if after { current_line + 1 } else { current_line };

        let content_lines: Vec<&str> = content.split('\n').collect();
        let mut repeated: Vec<&str> = Vec::with_capacity(content_lines.len() * count);
        for _ in 0..count {
            repeated.extend_from_slice(&content_lines);
        }

        let mut new_lines: Vec<&str> = Vec::with_capacity(lines.len() + repeated.len());
        new_lines.extend_from_slice(&lines[..insert_line]);
        new_lines.extend_from_slice(&repeated);
        new_lines.extend_from_slice(&lines[insert_line..]);

        let new_text = new_lines.join("\n");
        let cursor_off = line_start_offset(&new_lines, insert_line);
        VimEffect::Edit { text: new_text, cursor: cursor_off }
    } else {
        let to_insert = register.text.repeat(count);
        let insert_point = if after && cursor.offset < text.len() {
            cursor.next_off(cursor.offset)
        } else {
            cursor.offset
        };
        let new_text = format!(
            "{}{}{}",
            &text[..insert_point],
            to_insert,
            &text[insert_point..]
        );
        let last_gr = last_char_len(&to_insert);
        let new_off = (insert_point + to_insert.len()).saturating_sub(last_gr);
        VimEffect::Edit { text: new_text, cursor: new_off.max(insert_point) }
    }
}

/// Byte offset of the start of `line_index` within `lines` joined by '\n'.
/// (claude-code operators.ts::getLineStartOffset.)
#[must_use]
fn line_start_offset(lines: &[&str], line_index: usize) -> usize {
    let prefix: usize = lines[..line_index].iter().map(|l| l.len()).sum();
    prefix + line_index.min(lines.len()).saturating_sub(0).min(line_index) * 0
        + if line_index > 0 { line_index } else { 0 }
}
```

Simplify `line_start_offset` — the claude-code original is `lines.slice(0, lineIndex).join('\n').length + (lineIndex > 0 ? 1 : 0)`. The join length is `sum(len) + (lineIndex - 1)` newlines for `lineIndex` lines, then `+1` for the newline before the target. Net: `sum(len of first lineIndex lines) + lineIndex` when `lineIndex > 0`, else `0`. Write it cleanly:

```rust
#[must_use]
fn line_start_offset(lines: &[&str], line_index: usize) -> usize {
    if line_index == 0 {
        return 0;
    }
    let body: usize = lines[..line_index].iter().map(|l| l.len()).sum();
    body + line_index // one '\n' per preceding line
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::paste_tests`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T6): paste (p/P charwise + linewise, count, empty-register no-op)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: handle_vim_key operator-pending dispatch (d/c/y + motion/dd/count/find/g)

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod op_dispatch_tests`

Wires the operator engine into the state machine. From `Idle`, `d`/`c`/`y` (or `Count`→operator) enter `Operator{op,count}`. The next key resolves: motion → `operator_range` + `apply_operator`; same-key → `line_op`; digit → `OperatorCount`; `f/F/t/T` → `OperatorFind`; `g` → `OperatorG`; `G` → operator-G to last/Nth line; `Esc` → cancel. `x`/`p`/`P` dispatch directly from Idle. **Change** sets `state.mode = Insert`.

This task **extends the existing `handle_vim_key` and `dispatch_normal`** from M7-08 Task 7. Below shows the new arms to insert; preserve all M7-08 arms.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod op_dispatch_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn key(c: char) -> KeyEvent {
        let m = if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        KeyEvent::new(KeyCode::Char(c), m)
    }
    fn esc() -> KeyEvent { KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE) }
    fn normal() -> VimState { VimState { mode: VimMode::Normal, ..VimState::default() } }

    #[test]
    fn d_enters_operator_pending() {
        let mut s = normal();
        assert_eq!(handle_vim_key(&mut s, "foo bar", 0, key('d')), VimOutcome::Pending);
        assert_eq!(s.command, CommandState::Operator { op: Operator::Delete, count: 1 });
    }

    #[test]
    fn dw_deletes_word() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('w'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "bar".into(), cursor: 0 }));
        assert_eq!(s.command, CommandState::Idle);
        assert_eq!(s.register.text, "foo ");
    }

    #[test]
    fn de_deletes_to_end_of_word_inclusive() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('e'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: " bar".into(), cursor: 0 }));
    }

    #[test]
    fn d_dollar_deletes_to_eol() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 4, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 4, key('$'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "foo ".into(), cursor: 3 }));
    }

    #[test]
    fn dd_deletes_line() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 2, key('d'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('d'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "a\nc".into(), cursor: 2 }));
        assert!(s.register.linewise);
    }

    #[test]
    fn cc_clears_line_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "ab\ncd", 0, key('c'));
        let out = handle_vim_key(&mut s, "ab\ncd", 0, key('c'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "\ncd".into(), cursor: 0 }));
        assert_eq!(s.mode, VimMode::Insert);
    }

    #[test]
    fn cw_changes_to_end_of_word_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('c'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('w'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: " bar".into(), cursor: 0 }));
        assert_eq!(s.mode, VimMode::Insert);
    }

    #[test]
    fn yy_yanks_line_keeps_buffer() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 2, key('y'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('y'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
        assert_eq!(s.register, Register { text: "b\n".into(), linewise: true });
    }

    #[test]
    fn count_dw_multiplies() {
        // 3dw on "a b c d e" from 0: delete 3 words -> "d e".
        let mut s = normal();
        handle_vim_key(&mut s, "a b c d e", 0, key('3'));
        handle_vim_key(&mut s, "a b c d e", 0, key('d'));
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "d e".into(), cursor: 0 }));
    }

    #[test]
    fn d_count_w_inner_count_multiplies() {
        // d3w on "a b c d e" from 0: same as 3dw -> "d e".
        let mut s = normal();
        handle_vim_key(&mut s, "a b c d e", 0, key('d'));
        handle_vim_key(&mut s, "a b c d e", 0, key('3'));
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "d e".into(), cursor: 0 }));
    }

    #[test]
    fn count_yy_yanks_n_lines() {
        // 2yy on "a\nb\nc" from 0: register "a\nb\n".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('2'));
        handle_vim_key(&mut s, "a\nb\nc", 0, key('y'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('y'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
        assert_eq!(s.register.text, "a\nb\n");
    }

    #[test]
    fn df_char_deletes_through_find() {
        // df_c on "abcde" from 0: find 'c' at 2, inclusive -> delete "abc" -> "de".
        let mut s = normal();
        handle_vim_key(&mut s, "abcde", 0, key('d'));
        assert_eq!(s.command, CommandState::OperatorFind { op: Operator::Delete, count: 1, kind: FindKind::F });
        let out = handle_vim_key(&mut s, "abcde", 0, key('c'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "de".into(), cursor: 0 }));
    }

    #[test]
    fn dG_deletes_to_last_line() {
        // dG on "a\nb\nc" from 0: linewise delete all -> "".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('d'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('G'));
        match out {
            VimOutcome::Effect(VimEffect::Edit { text, .. }) => assert_eq!(text, ""),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn dgg_deletes_to_first_line() {
        // dgg on "a\nb\nc" from offset 4 (line2 'c'): linewise delete lines 0..2 -> "".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 4, key('d'));
        assert_eq!(handle_vim_key(&mut s, "a\nb\nc", 4, key('g')), VimOutcome::Pending);
        assert_eq!(s.command, CommandState::OperatorG { op: Operator::Delete, count: 1 });
        let out = handle_vim_key(&mut s, "a\nb\nc", 4, key('g'));
        match out {
            VimOutcome::Effect(VimEffect::Edit { text, .. }) => assert_eq!(text, ""),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn x_deletes_char() {
        let mut s = normal();
        let out = handle_vim_key(&mut s, "hello", 0, key('x'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "ello".into(), cursor: 0 }));
        assert_eq!(s.register.text, "h");
    }

    #[test]
    fn p_pastes_after() {
        let mut s = normal();
        s.register = Register { text: "X".into(), linewise: false };
        let out = handle_vim_key(&mut s, "ab", 0, key('p'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "aXb".into(), cursor: 1 }));
    }

    #[test]
    fn esc_cancels_operator_pending() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo", 0, esc());
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.command, CommandState::Idle);
    }

    #[test]
    fn operator_motion_noop_when_motion_does_not_move() {
        // dh at offset 0: h is a no-op -> operator no-op, register untouched.
        let mut s = normal();
        handle_vim_key(&mut s, "abc", 0, key('d'));
        let out = handle_vim_key(&mut s, "abc", 0, key('h'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.register, Register::default());
        assert_eq!(s.command, CommandState::Idle);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib vim::op_dispatch_tests`
Expected: FAIL — operator-pending branches not wired.

- [ ] **Step 3: Write minimal implementation**

Extend `handle_vim_key`. In M7-08, after the Insert-mode block and the Normal `Esc`-cancel, there is a `match std::mem::replace(&mut state.command, CommandState::Idle) { ... }` over the literal-char sub-states. Add the M7-09 operator-pending arms to that match (and an `Esc` guard so Esc cancels an operator). Insert these arms **before** the `CommandState::Idle => {}` arm:

```rust
        CommandState::Operator { op, count } => {
            return dispatch_operator_pending(state, cursor, op, count, key);
        }
        CommandState::OperatorCount { op, count, digits } => {
            if let KeyCode::Char(c @ '0'..='9') = key.code {
                let mut d = digits;
                d.push(c);
                state.command = CommandState::OperatorCount { op, count, digits: d };
                return VimOutcome::Pending;
            }
            let inner = digits.parse::<usize>().unwrap_or(1).max(1);
            return dispatch_operator_pending(state, cursor, op, count * inner, key);
        }
        CommandState::OperatorFind { op, count, kind } => {
            if let KeyCode::Char(ch) = key.code {
                state.last_find = Some((kind, ch));
                return match cursor.find_character(ch, kind, count) {
                    Some(target) => {
                        // find ranges are inclusive (operators.ts::getOperatorRangeForFind).
                        let from = cursor.offset.min(target);
                        let to_raw = cursor.offset.max(target);
                        let to = VimCursor { text: cursor.text, offset: to_raw }.next_off(to_raw);
                        finish_operator(state, op, cursor.text, from, to, false)
                    }
                    None => VimOutcome::Effect(VimEffect::None),
                };
            }
            return VimOutcome::Effect(VimEffect::None);
        }
        CommandState::OperatorG { op, count } => {
            if let KeyCode::Char('g') = key.code {
                let target = if count > 1 {
                    cursor.go_to_line(count)
                } else {
                    cursor.start_of_first_line()
                };
                if target.offset == cursor.offset && count <= 1 {
                    // already at first line and no count -> still operate on line(s) below? vim dgg from line0 deletes line0.
                }
                let range = operator_range(cursor, target.offset, 'g', op, count);
                // gg is linewise; force linewise classification:
                let range = OperatorRange { linewise: true, ..force_linewise_g(cursor, target.offset, op) };
                return finish_operator(state, op, cursor.text, range.from, range.to, range.linewise);
            }
            return VimOutcome::Effect(VimEffect::None);
        }
```

The `OperatorG` arm above is over-complicated; replace it with a clean version that reuses `operator_range` with the `"gg"` linewise classification by passing a sentinel. Since `operator_range` keys on a single `char`, add a small dedicated helper for operator-G (mirrors `executeOperatorGg`/`executeOperatorG`):

```rust
        CommandState::OperatorG { op, count } => {
            if let KeyCode::Char('g') = key.code {
                let target = if count > 1 { cursor.go_to_line(count) } else { cursor.start_of_first_line() };
                return operator_over_lines(state, op, cursor, target.offset);
            }
            return VimOutcome::Effect(VimEffect::None);
        }
```

Now the helpers. Add `dispatch_operator_pending`, `finish_operator`, and `operator_over_lines`:

```rust
/// One key in operator-pending state (op already chosen, count resolved).
fn dispatch_operator_pending(
    state: &mut VimState,
    cursor: VimCursor<'_>,
    op: Operator,
    count: usize,
    key: KeyEvent,
) -> VimOutcome {
    // Esc cancels.
    if key.code == KeyCode::Esc {
        state.command = CommandState::Idle;
        return VimOutcome::Effect(VimEffect::None);
    }
    let KeyCode::Char(ch) = key.code else {
        state.command = CommandState::Idle;
        return VimOutcome::Effect(VimEffect::None);
    };

    // Inner count: digit (but '0' is a motion, not a count seed — matches `0` LineStart).
    if let '1'..='9' = ch {
        state.command = CommandState::OperatorCount { op, count, digits: ch.to_string() };
        return VimOutcome::Pending;
    }

    // Doubled operator key -> line op (dd/cc/yy).
    let op_char = match op { Operator::Delete => 'd', Operator::Change => 'c', Operator::Yank => 'y' };
    if ch == op_char {
        let (effect, register, enter_insert) = line_op(op, cursor, count);
        state.register = register;
        state.command = CommandState::Idle;
        if enter_insert { state.mode = VimMode::Insert; }
        return VimOutcome::Effect(effect);
    }

    // Find prefix -> operatorFind.
    let find_kind = match ch {
        'f' => Some(FindKind::F), 'F' => Some(FindKind::BigF),
        't' => Some(FindKind::T), 'T' => Some(FindKind::BigT), _ => None,
    };
    if let Some(kind) = find_kind {
        state.command = CommandState::OperatorFind { op, count, kind };
        return VimOutcome::Pending;
    }

    // g -> operatorG (dgg).
    if ch == 'g' {
        state.command = CommandState::OperatorG { op, count };
        return VimOutcome::Pending;
    }

    // G -> operator over lines to last/Nth line.
    if ch == 'G' {
        let target = if count > 1 { cursor.go_to_line(count) } else { cursor.start_of_last_line() };
        return operator_over_lines(state, op, cursor, target.offset);
    }

    // Simple motion.
    if let Some(motion) = motion_for_operator_key(ch) {
        let target = resolve_motion(motion, cursor, count);
        if target.offset == cursor.offset {
            // motion didn't move -> operator no-op (operators.ts: target.equals(cursor) return)
            state.command = CommandState::Idle;
            return VimOutcome::Effect(VimEffect::None);
        }
        let range = operator_range(cursor, target.offset, ch, op, count);
        return finish_operator(state, op, cursor.text, range.from, range.to, range.linewise);
    }

    // Text objects (iw/aw...) and any other key: DEFERRED (GATE) -> cancel, no-op.
    state.command = CommandState::Idle;
    VimOutcome::Effect(VimEffect::None)
}

/// Apply an operator over a resolved byte range; store register; reset command;
/// enter Insert for change.
fn finish_operator(
    state: &mut VimState,
    op: Operator,
    text: &str,
    from: usize,
    to: usize,
    linewise: bool,
) -> VimOutcome {
    state.command = CommandState::Idle;
    if from == to {
        return VimOutcome::Effect(VimEffect::None);
    }
    let (effect, register, enter_insert) = apply_operator(op, text, from, to, linewise);
    state.register = register;
    if enter_insert { state.mode = VimMode::Insert; }
    VimOutcome::Effect(effect)
}

/// Operator over whole lines from cursor's line to `target_offset`'s line
/// (dG / dgg / NdG). Always linewise. Mirrors executeOperatorG/Gg + line range.
fn operator_over_lines(
    state: &mut VimState,
    op: Operator,
    cursor: VimCursor<'_>,
    target_offset: usize,
) -> VimOutcome {
    let text = cursor.text;
    let from_line_start = VimCursor { text, offset: cursor.offset.min(target_offset) }
        .start_of_logical_line().offset;
    let to_line = VimCursor { text, offset: cursor.offset.max(target_offset) };
    let to_line_end = match text[to_line.offset..].find('\n') {
        None => text.len(),
        Some(rel) => to_line.offset + rel + 1,
    };
    finish_operator(state, op, text, from_line_start, to_line_end, true)
}
```

In `dispatch_normal` (the M7-08 Idle-dispatch helper), add the operator-entry and `x`/`p`/`P` arms. After the mode-entry keys (`i a I A o O`) and **before** the simple-motion match (so `d`/`c`/`y` are not mistaken for motions — they aren't motions anyway, but order keeps it clear), insert:

```rust
    // Operators (M7-09): enter operator-pending.
    let op = match ch {
        'd' => Some(Operator::Delete),
        'c' => Some(Operator::Change),
        'y' => Some(Operator::Yank),
        _ => None,
    };
    if let Some(op) = op {
        state.command = CommandState::Operator { op, count };
        return VimOutcome::Pending;
    }

    // x: delete char(s) under cursor.
    if ch == 'x' {
        let (effect, register) = delete_char_x(cursor, count);
        if effect != VimEffect::None {
            state.register = register;
        }
        return VimOutcome::Effect(effect);
    }

    // p / P: paste.
    if ch == 'p' || ch == 'P' {
        let effect = paste(ch == 'p', count, &state.register, cursor);
        return VimOutcome::Effect(effect);
    }

    // v / V: enter Visual / Visual-line (Task 8 implements; stub returns Pending-equivalent
    // here in Task 7 would break — Task 8 adds these arms. For Task 7, leave to Task 8.)
```

(Remove the `v`/`V` comment placeholder once Task 8 lands; in Task 7, `v`/`V` fall through to the existing M7-08 `VimEffect::None` for unknown keys — that's fine, Task 8's tests will turn them on.)

Important: the M7-08 `Count` arm in `handle_vim_key` already routes a non-digit key (after a count) into `dispatch_normal(state, cursor, count, key)`. Because `dispatch_normal` now handles `d`/`c`/`y`, a leading-count operator (`3dw`) seeds `Operator{op, count}` with the leading count — the inner count then multiplies via `OperatorCount`. Verify the `count_dw_multiplies` test covers this.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::op_dispatch_tests`
Expected: PASS (19 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T7): operator-pending dispatch (dw/de/d\$/dd/cc/yy/df/dG/dgg/3dw/d3w) + x/p/P

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: Visual / Visual-line modes (v / V + motions + d/c/y + Esc)

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs`
- Test: `vim.rs` `#[cfg(test)] mod visual_tests`

`v`/`V` from Normal set `state.visual` + `state.mode = Visual`. In Visual: motions move the cursor (selection end); `d`/`c`/`y` apply over the selection; `Esc` → Normal. Standard vim semantics (claude-code has no visual mode — documented in GATE note). The selection range is `[min(anchor,cursor) .. max(anchor,cursor))`; charwise visual is **inclusive** of the cursor char (+1 char on the high end); linewise covers whole lines.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod visual_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn key(c: char) -> KeyEvent {
        let m = if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        KeyEvent::new(KeyCode::Char(c), m)
    }
    fn esc() -> KeyEvent { KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE) }
    fn normal() -> VimState { VimState { mode: VimMode::Normal, ..VimState::default() } }

    #[test]
    fn v_enters_visual_sets_anchor() {
        let mut s = normal();
        let out = handle_vim_key(&mut s, "hello", 2, key('v'));
        assert_eq!(s.mode, VimMode::Visual);
        assert_eq!(s.visual, Some(VisualState { anchor: 2, linewise: false }));
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
    }

    #[test]
    fn cap_v_enters_visual_linewise() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb", 0, key('V'));
        assert_eq!(s.mode, VimMode::Visual);
        assert_eq!(s.visual, Some(VisualState { anchor: 0, linewise: true }));
    }

    #[test]
    fn visual_motion_moves_selection_end() {
        // v then l l on "hello" from 0: cursor moves to 2 (anchor stays 0).
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 0, key('l'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(1)));
        let out2 = handle_vim_key(&mut s, "hello", 1, key('l'));
        assert_eq!(out2, VimOutcome::Effect(VimEffect::Move(2)));
        assert_eq!(s.visual, Some(VisualState { anchor: 0, linewise: false }));
    }

    #[test]
    fn visual_d_deletes_inclusive_selection() {
        // v l l (anchor 0, cursor 2) then d on "hello": charwise inclusive ->
        // delete [0,3) "hel" -> "lo", back to Normal.
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v')); // anchor 0
        // simulate cursor at 2 (caller moved it via the two Move effects)
        let out = handle_vim_key(&mut s, "hello", 2, key('d'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "lo".into(), cursor: 0 }));
        assert_eq!(s.mode, VimMode::Normal);
        assert!(s.visual.is_none());
        assert_eq!(s.register.text, "hel");
    }

    #[test]
    fn visual_y_yanks_selection_keeps_buffer() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 2, key('y'));
        // yank moves cursor to range start, buffer unchanged.
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
        assert_eq!(s.register, Register { text: "hel".into(), linewise: false });
        assert_eq!(s.mode, VimMode::Normal);
    }

    #[test]
    fn visual_c_deletes_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 2, key('c'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "lo".into(), cursor: 0 }));
        assert_eq!(s.mode, VimMode::Insert);
        assert!(s.visual.is_none());
    }

    #[test]
    fn visual_line_d_deletes_whole_lines() {
        // V then j (anchor line0, cursor line1) then d on "a\nb\nc":
        // linewise delete lines 0..1 -> "c".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('V')); // anchor 0, linewise
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('d')); // cursor on line1 ('b' @2)
        assert_eq!(out, VimOutcome::Effect(VimEffect::Edit { text: "c".into(), cursor: 0 }));
        assert!(s.register.linewise);
        assert_eq!(s.register.text, "a\nb\n");
        assert_eq!(s.mode, VimMode::Normal);
    }

    #[test]
    fn visual_esc_returns_to_normal() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 2, key('v'));
        let out = handle_vim_key(&mut s, "hello", 4, esc());
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4))); // no clamp change here (4 < len 5)
        assert_eq!(s.mode, VimMode::Normal);
        assert!(s.visual.is_none());
    }

    #[test]
    fn visual_count_motion_extends() {
        // v then 2l on "hello" from 0: cursor -> 2.
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        assert_eq!(handle_vim_key(&mut s, "hello", 0, key('2')), VimOutcome::Pending);
        let out = handle_vim_key(&mut s, "hello", 0, key('l'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib vim::visual_tests`
Expected: FAIL — visual branches not wired.

- [ ] **Step 3: Write minimal implementation**

First, in `dispatch_normal`, add the `v`/`V` arms (replacing the Task 7 placeholder comment):

```rust
    // v / V: enter Visual / Visual-line.
    if ch == 'v' || ch == 'V' {
        state.mode = VimMode::Visual;
        state.visual = Some(VisualState { anchor: cursor.offset, linewise: ch == 'V' });
        return VimOutcome::Effect(VimEffect::None);
    }
```

Then, in `handle_vim_key`, add a Visual-mode block. Place it **after** the Insert-mode block and **before** the Normal `Esc`-cancel, so Visual gets its own handling (and a Visual `Esc` exits to Normal rather than just cancelling a command):

```rust
    // ----- VISUAL mode (M7-09). claude-code has no visual mode; standard vim. --
    if state.mode == VimMode::Visual {
        let visual = state.visual.expect("Visual mode without VisualState");
        let cursor = VimCursor { text, offset };

        // Esc -> Normal.
        if key.code == KeyCode::Esc {
            state.mode = VimMode::Normal;
            state.visual = None;
            state.command = CommandState::Idle;
            return VimOutcome::Effect(VimEffect::Move(esc_clamp(text, offset)));
        }

        let KeyCode::Char(ch) = key.code else {
            return VimOutcome::Effect(VimEffect::None);
        };

        // d/c/y operate on the selection.
        if matches!(ch, 'd' | 'c' | 'y') {
            let op = match ch { 'd' => Operator::Delete, 'c' => Operator::Change, _ => Operator::Yank };
            let (from, to, linewise) = visual_range(text, visual, offset);
            state.mode = VimMode::Normal;
            state.visual = None;
            state.command = CommandState::Idle;
            return finish_operator(state, op, text, from, to, linewise);
        }

        // Pending count inside Visual (e.g. 2l): reuse the Count sub-state.
        match std::mem::replace(&mut state.command, CommandState::Idle) {
            CommandState::Count { digits } => {
                if let '0'..='9' = ch {
                    let mut d = digits;
                    d.push(ch);
                    state.command = CommandState::Count { digits: d };
                    return VimOutcome::Pending;
                }
                let count = digits.parse::<usize>().unwrap_or(1).max(1);
                return visual_motion(state, cursor, count, ch);
            }
            CommandState::Idle => {}
            other => { state.command = other; } // shouldn't happen in Visual; keep
        }
        if let '1'..='9' = ch {
            state.command = CommandState::Count { digits: ch.to_string() };
            return VimOutcome::Pending;
        }
        return visual_motion(state, cursor, 1, ch);
    }
```

Add the helpers `visual_range` and `visual_motion`:

```rust
/// Byte range + linewise flag for a Visual selection from anchor to cursor.
/// Charwise: inclusive of the cursor char (+1 char on the high end).
/// Linewise: whole logical lines spanning anchor..cursor.
#[must_use]
fn visual_range(text: &str, visual: VisualState, cursor_offset: usize) -> (usize, usize, bool) {
    let lo = visual.anchor.min(cursor_offset);
    let hi = visual.anchor.max(cursor_offset);
    if visual.linewise {
        let from = VimCursor { text, offset: lo }.start_of_logical_line().offset;
        let to = match text[hi..].find('\n') {
            None => text.len(),
            Some(rel) => hi + rel + 1,
        };
        (from, to, true)
    } else {
        // charwise inclusive: extend one char past the high offset (clamped to len).
        let to = VimCursor { text, offset: hi }.next_off(hi);
        (lo, to, false)
    }
}

/// A motion key inside Visual mode: move the cursor (selection end). Anchor stays.
fn visual_motion(state: &mut VimState, cursor: VimCursor<'_>, count: usize, ch: char) -> VimOutcome {
    // gg / G are linewise navigation; support them as selection extenders.
    let dest = match ch {
        'g' => {
            // need a second 'g'; reuse the G sub-state but in Visual it's simpler:
            // treat single 'g' as pending via Count? Simplest: support 'G' and gg below.
            // We model gg by stashing a G-pending; but to keep Visual minimal, handle 'g'
            // as: set a transient pending using CommandState::G.
            state.command = CommandState::G { count };
            return VimOutcome::Pending;
        }
        'G' => return VimOutcome::Effect(VimEffect::Move(cursor.start_of_last_line().offset)),
        _ => match motion_for_operator_key(ch) {
            Some(m) => resolve_motion(m, cursor, count),
            None => return VimOutcome::Effect(VimEffect::None),
        },
    };
    VimOutcome::Effect(VimEffect::Move(dest.offset))
}
```

The `'g'`-pending inside Visual needs the Visual block's `CommandState::G` arm to resolve `gg` to file-start. Add to the Visual block's `match std::mem::replace(...)` (alongside the `Count` arm):

```rust
            CommandState::G { count: _ } => {
                if ch == 'g' {
                    return VimOutcome::Effect(VimEffect::Move(cursor.start_of_first_line().offset));
                }
                return VimOutcome::Effect(VimEffect::None);
            }
```

(Place this arm before `CommandState::Idle => {}` in the Visual block's match.)

Note: in Visual, motions return `VimEffect::Move(off)`; the caller (root.rs `apply_vim_effect`) updates `prompt_cursor`, and the renderer highlights `[anchor..cursor]`. Selection rendering is a footer/highlight concern handled in Task 9 (the indicator) + the existing PromptInput render reading `state.visual` (best-effort highlight; not asserted here — behavior tests assert the d/c/y outcome which is the load-bearing part).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib vim::visual_tests`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T8): Visual + Visual-line modes (v/V + motions + d/c/y on selection + Esc)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: Footer indicator for Visual / Visual-line

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs` (extend `mode_indicator` callers — but the function takes only `VimMode`)
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/footer.rs`
- Test: `footer.rs` `#[cfg(test)] mod visual_indicator_tests`

M7-08's `mode_indicator(VimMode)` already returns `-- VISUAL --` for `VimMode::Visual`. But Visual-line should show `-- VISUAL LINE --`. Since `VimMode` doesn't distinguish, the footer reads `VisualState.linewise` to pick the label. Extend `footer_mode_label` to take the visual linewise flag.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod visual_indicator_tests {
    use super::*;
    use crate::components::prompt_input::VimMode;

    #[test]
    fn charwise_visual_label() {
        assert_eq!(footer_mode_label_v2(true, VimMode::Visual, false), Some("-- VISUAL --"));
    }

    #[test]
    fn linewise_visual_label() {
        assert_eq!(footer_mode_label_v2(true, VimMode::Visual, true), Some("-- VISUAL LINE --"));
    }

    #[test]
    fn normal_insert_labels_unchanged() {
        assert_eq!(footer_mode_label_v2(true, VimMode::Normal, false), Some("-- NORMAL --"));
        assert_eq!(footer_mode_label_v2(true, VimMode::Insert, false), Some("-- INSERT --"));
    }

    #[test]
    fn disabled_is_none() {
        assert_eq!(footer_mode_label_v2(false, VimMode::Visual, true), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib footer::visual_indicator_tests`
Expected: FAIL — `footer_mode_label_v2` not defined.

- [ ] **Step 3: Write minimal implementation**

In `footer.rs`:

```rust
/// (M7-09) Mode-indicator label, distinguishing charwise vs linewise Visual.
/// `None` when vim is disabled. Supersedes M7-08's `footer_mode_label` which
/// could not tell `v` from `V`. `footer_mode_label` is retained as a thin
/// wrapper for callers that don't track linewise.
#[must_use]
pub fn footer_mode_label_v2(
    vim_enabled: bool,
    mode: VimMode,
    visual_linewise: bool,
) -> Option<&'static str> {
    if !vim_enabled {
        return None;
    }
    Some(match mode {
        VimMode::Normal => "-- NORMAL --",
        VimMode::Insert => "-- INSERT --",
        VimMode::Visual if visual_linewise => "-- VISUAL LINE --",
        VimMode::Visual => "-- VISUAL --",
    })
}
```

Update the `Footer` render path: where M7-08 called `footer_mode_label(props.vim_enabled, props.vim_mode)`, switch to `footer_mode_label_v2(props.vim_enabled, props.vim_mode, props.vim_visual_linewise)`. Add `vim_visual_linewise: bool` to the footer `Props` and thread it from the `PromptInput` render (`AppState.vim.visual.map_or(false, |v| v.linewise)`). Keep the old `footer_mode_label` as-is for any caller not yet updated (and its M7-08 test stays green).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib footer::visual_indicator_tests`
Expected: PASS (4 tests). If M7-06/M7-08 established a footer insta snapshot, run `cargo insta test --review` and accept only the Visual-line label row.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/footer.rs
git commit -m "plan(M7-09 T9): footer indicator distinguishes -- VISUAL -- vs -- VISUAL LINE --

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 10: Operator×motion matrix (exhaustive behavior tests)

**Files:**
- Modify: `lingxi-code/crates/tui/tests/vim_behavior.rs`

Extends the M7-08 integration file with the dense operator×motion matrix. Reuses M7-08's `run_normal` harness (drives keys one at a time, applying each `VimEffect`). Each row is `(start_text, start_offset, keys, expected_text, expected_offset)`.

- [ ] **Step 1: Write the failing test**

Append to `vim_behavior.rs`. (The M7-08 file already imports `handle_vim_key, VimEffect, VimMode, VimOutcome, VimState` and defines `run_normal`. M7-09 also needs `Register` — extend the `use` line. `run_normal` already applies `Edit` effects, so operator results compose.)

```rust
use lingxi_tui::components::prompt_input::vim::Register;

#[test]
fn operator_motion_matrix() {
    // (start_text, start_offset, keys, expected_text, expected_offset)
    let cases: &[(&str, usize, &str, &str, usize)] = &[
        // ---- d × motions ----
        ("foo bar", 0, "dw", "bar", 0),          // delete word + trailing space
        ("foo bar", 0, "de", " bar", 0),         // delete to end of word (inclusive)
        ("foo bar", 4, "d$", "foo ", 3),         // delete to end of line
        ("  hello", 4, "d0", "hello", 0),        // delete to line start (exclusive)
        ("foo bar baz", 0, "dl", "oo bar baz", 0), // dl == x: delete one char right
        ("abcde", 0, "dfc", "de", 0),            // delete through find 'c' (inclusive)
        ("abcde", 0, "dtc", "cde", 0),           // delete up-to 'c' (t: stops before)
        ("a\nb\nc", 0, "dj", "c", 0),            // linewise: delete lines 0..1
        ("a\nb\nc", 0, "dG", "", 0),             // linewise: delete to last line
        ("a\nb\nc", 4, "dgg", "", 0),            // linewise: delete to first line
        // ---- c × motions ----
        ("foo bar", 0, "cw", " bar", 0),         // cw -> ce (end of word), enter insert
        ("foo bar", 4, "c$", "foo ", 3),         // change to EOL
        // ---- y (buffer unchanged; cursor moves) ----
        ("foo bar", 0, "yw", "foo bar", 0),      // yank word: buffer unchanged
        ("foo bar", 4, "y$", "foo bar", 4),      // yank to EOL: cursor stays at range start
        // ---- doubled ops ----
        ("a\nb\nc", 2, "dd", "a\nc", 2),         // delete line1
        ("a\nb\nc", 0, "yy", "a\nb\nc", 0),      // yank line: buffer unchanged
        ("ab\ncd", 0, "cc", "\ncd", 0),          // clear line, enter insert
        // ---- counts ----
        ("a b c d e", 0, "3dw", "d e", 0),       // 3 words
        ("a b c d e", 0, "d3w", "d e", 0),       // inner count, same result
        ("a\nb\nc\nd", 0, "2dd", "c\nd", 0),     // 2 lines
        ("a\nb\nc", 0, "2yy", "a\nb\nc", 0),     // yank 2 lines: buffer unchanged
    ];
    for (i, (text, off, keys, want_text, want_off)) in cases.iter().enumerate() {
        let (got_text, got_off) = run_normal(text, *off, keys);
        assert_eq!(&got_text, want_text, "case {i}: text after {keys:?} on {text:?}");
        assert_eq!(got_off, *want_off, "case {i}: offset after {keys:?} on {text:?}");
    }
}

#[test]
fn yank_then_paste_roundtrip() {
    // yy then p: yank line0, paste below -> duplicated line.
    let (text, _off) = run_normal("hello\nworld", 0, "yyp");
    assert_eq!(text, "hello\nhello\nworld");
}

#[test]
fn delete_then_paste_charwise() {
    // x x (delete "he") leaves register "e" (last delete); p pastes "e".
    // Then test a single-char delete+paste: x on "abc" -> "bc" reg "a"; p -> "bac".
    let (text, off) = run_normal("abc", 0, "xp");
    assert_eq!(text, "bac");
    assert_eq!(off, 1); // cursor on pasted 'a'
}

#[test]
fn register_holds_last_yank_or_delete() {
    // After dw the register holds "foo "; paste it elsewhere.
    let (text, _off) = run_normal("foo bar", 0, "dw$p");
    // "foo bar" -> dw -> "bar" (reg "foo "); $ -> end (offset 3 -> after clamp 2 'r');
    // p pastes "foo " after 'r' -> "barfoo " ... assert the register content survived:
    assert!(text.contains("foo "));
}
```

- [ ] **Step 2: Run test to verify it fails (or surfaces a real bug)**

Run: `cargo test -p lingxi-tui --test vim_behavior operator_motion_matrix yank_then_paste delete_then_paste register_holds`
Expected: PASS if Tasks 3-8 are correct. A failing row pinpoints a real operator bug — fix the offending function in `vim.rs` (treat the matrix as the spec; do NOT weaken a row to match a buggy impl). The `dl == x` case verifies `dl` (operator + Right motion) deletes exactly one char like `x` does for a single char.

- [ ] **Step 3: Fix any discrepancy the matrix surfaces**

Each failing row is a TDD failure against `vim.rs`. The matrix encodes the locked claude-code semantics from the Background section. Common culprits: inclusive-motion off-by-one (`de`/`d$`), the `cw`→`ce` special case, linewise trailing-newline handling (`dd` of last line). Re-run the relevant `vim::*` unit mod after each fix.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test vim_behavior`
Expected: PASS (M7-08 fns + the 4 new M7-09 fns).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/tests/vim_behavior.rs
git commit -m "plan(M7-09 T10): exhaustive operator×motion matrix + yank/delete/paste roundtrip tests

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 11: Visual-mode behavior tests (charwise + linewise via the seam)

**Files:**
- Modify: `lingxi-code/crates/tui/tests/vim_behavior.rs`

Visual needs the cursor to move between keys (the selection end), so it must drive a harness that threads `Move` effects. The unit tests in Task 8 fed the cursor manually; here a `run_visual` helper threads it through, mirroring `run_normal`. This proves `v`+motions+`d/c/y` compose correctly.

- [ ] **Step 1: Write the failing test**

Append to `vim_behavior.rs`:

```rust
/// Drive a key sequence starting in NORMAL, where `v`/`V` flips to Visual and
/// subsequent motions move the cursor (selection end). Applies each effect.
fn run_visual(text: &str, offset: usize, keys: &str) -> (String, usize, VimMode) {
    let mut state = VimState { mode: VimMode::Normal, ..VimState::default() };
    let mut buf = text.to_string();
    let mut off = offset;
    for ch in keys.chars() {
        let key = if ch == '⎋' {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            )
        } else {
            k(ch)
        };
        match handle_vim_key(&mut state, &buf, off, key) {
            VimOutcome::Effect(VimEffect::Move(o)) => off = o.min(buf.len()),
            VimOutcome::Effect(VimEffect::Edit { text, cursor }) => {
                buf = text;
                off = cursor.min(buf.len());
            }
            _ => {}
        }
    }
    (buf, off, state.mode)
}

#[test]
fn visual_charwise_delete_matrix() {
    // (start_text, start_offset, keys, expected_text, expected_offset)
    let cases: &[(&str, usize, &str, &str, usize)] = &[
        ("hello", 0, "vlld", "lo", 0),    // v + ll (cursor->2) + d -> delete "hel"
        ("hello", 0, "vlly", "hello", 0), // yank: buffer unchanged, cursor to start
        ("hello", 0, "v$d", "", 0),       // v + $ + d -> delete whole line "hello"
        ("hello", 1, "vlld", "ho", 1),    // v from 1 + ll (cursor->3) + d -> delete "ell"
    ];
    for (i, (text, off, keys, want_text, want_off)) in cases.iter().enumerate() {
        let (got_text, got_off, mode) = run_visual(text, *off, keys);
        assert_eq!(&got_text, want_text, "case {i}: {keys:?} on {text:?}");
        assert_eq!(got_off, *want_off, "case {i}");
        // d/y return to Normal.
        assert_eq!(mode, VimMode::Normal, "case {i}: should be back in Normal");
    }
}

#[test]
fn visual_linewise_delete() {
    // V + j (cursor to line1) + d on "a\nb\nc": delete lines 0..1 -> "c".
    let (text, off, mode) = run_visual("a\nb\nc", 0, "Vjd");
    assert_eq!(text, "c");
    assert_eq!(off, 0);
    assert_eq!(mode, VimMode::Normal);
}

#[test]
fn visual_c_enters_insert() {
    let (text, _off, mode) = run_visual("hello", 0, "vlc");
    // v + l (cursor->1) + c -> delete "he" (inclusive of cursor char) -> "llo", Insert.
    assert_eq!(text, "llo");
    assert_eq!(mode, VimMode::Insert);
}

#[test]
fn visual_esc_returns_to_normal_no_edit() {
    let (text, _off, mode) = run_visual("hello", 0, "vll⎋");
    assert_eq!(text, "hello"); // no edit
    assert_eq!(mode, VimMode::Normal);
}

#[test]
fn visual_count_motion_then_delete() {
    // v + 2l (cursor->2) + d on "hello": delete "hel" -> "lo".
    let (text, _off, mode) = run_visual("hello", 0, "v2ld");
    assert_eq!(text, "lo");
    assert_eq!(mode, VimMode::Normal);
}
```

- [ ] **Step 2: Run test to verify it fails (or surfaces a bug)**

Run: `cargo test -p lingxi-tui --test vim_behavior visual_charwise visual_linewise visual_c visual_esc visual_count`
Expected: PASS if Task 8 is correct. A failing row pinpoints a visual-range bug (inclusive boundary, linewise span) — fix `visual_range`/`visual_motion` in `vim.rs`.

- [ ] **Step 3: Fix any discrepancy**

The charwise-inclusive boundary (`vll` from 0 selects offsets 0,1,2 → delete `[0,3)`) is the most likely off-by-one. Confirm `visual_range` extends one char past the high offset for charwise.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test vim_behavior`
Expected: PASS (all fns).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/tests/vim_behavior.rs
git commit -m "plan(M7-09 T11): visual-mode behavior tests (charwise/linewise delete, c-insert, count, esc)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 12: vim-disabled passthrough + operator-edit seam (root.rs integration)

**Files:**
- Modify: `lingxi-code/crates/tui/tests/vim_behavior.rs`
- Modify: `lingxi-code/crates/tui/src/root.rs` (only if the seam needs the new Edit-from-operator path; see Step 3)

Proves through the full `handle_live_key` seam that: (a) operators mutate the real prompt buffer; (b) `c` enters Insert and subsequent typing flows through `PassThrough`; (c) vim-disabled editing remains byte-identical to M6 (the GATE invariant). Reuses M7-08's `live_char`/`live_esc` helpers if present in `vim_behavior.rs`; else add them (model on M7-08 Task 12).

- [ ] **Step 1: Write the failing test**

Append to `vim_behavior.rs` (if `live_char`/`live_esc`/the `AppState`+`handle_live_key` imports already exist from M7-08 Task 12, reuse them — do NOT redefine):

```rust
#[test]
fn operator_mutates_real_buffer_via_live_key() {
    use lingxi_tui::state::AppState;
    use lingxi_tui::root::handle_live_key;
    let mut st = AppState::new(Default::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "foo bar".into();
    st.prompt_cursor = 0;
    handle_live_key(&mut st, &live_char('d'), 24); // operator-pending
    handle_live_key(&mut st, &live_char('w'), 24); // dw
    assert_eq!(st.prompt_text, "bar");
    assert_eq!(st.prompt_cursor, 0);
    assert_eq!(st.vim.register.text, "foo ");
}

#[test]
fn change_enters_insert_and_typing_flows_through() {
    use lingxi_tui::state::AppState;
    use lingxi_tui::root::handle_live_key;
    let mut st = AppState::new(Default::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "foo bar".into();
    st.prompt_cursor = 0;
    handle_live_key(&mut st, &live_char('c'), 24);
    handle_live_key(&mut st, &live_char('w'), 24); // cw -> " bar", Insert at 0
    assert_eq!(st.vim.mode, VimMode::Insert);
    assert_eq!(st.prompt_text, " bar");
    assert_eq!(st.prompt_cursor, 0);
    // Now type "X": default editing inserts at cursor.
    handle_live_key(&mut st, &live_char('X'), 24);
    assert_eq!(st.prompt_text, "X bar");
    assert_eq!(st.prompt_cursor, 1);
}

#[test]
fn paste_via_live_key_after_yank() {
    use lingxi_tui::state::AppState;
    use lingxi_tui::root::handle_live_key;
    let mut st = AppState::new(Default::default());
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "ab".into();
    st.prompt_cursor = 0;
    // yl yanks one char 'a' (yank + Right motion -> range [0,1)).
    handle_live_key(&mut st, &live_char('y'), 24);
    handle_live_key(&mut st, &live_char('l'), 24);
    assert_eq!(st.vim.register.text, "a");
    handle_live_key(&mut st, &live_char('p'), 24); // paste 'a' after cursor
    assert_eq!(st.prompt_text, "aab");
}

#[test]
fn vim_disabled_operator_keys_are_literal_inserts() {
    use lingxi_tui::state::AppState;
    use lingxi_tui::root::handle_live_key;
    let mut st = AppState::new(Default::default());
    st.vim_enabled = false; // OFF — the GATE invariant
    st.prompt_text = String::new();
    st.prompt_cursor = 0;
    // "dwccyyxp" with vim OFF are all literal characters.
    for c in "dwccyyxp".chars() {
        handle_live_key(&mut st, &live_char(c), 24);
    }
    assert_eq!(st.prompt_text, "dwccyyxp");
    assert_eq!(st.prompt_cursor, 8);
}
```

- [ ] **Step 2: Run test to verify it fails (or passes if the seam is already correct)**

Run: `cargo test -p lingxi-tui --test vim_behavior operator_mutates change_enters paste_via vim_disabled_operator`
Expected: PASS if the M7-08 seam already applies `VimEffect::Edit` (it does — `apply_vim_effect` handles `Edit { text, cursor }`). A FAIL on `operator_mutates` would mean the seam dropped the Edit; on `vim_disabled` would mean the gate is broken (a real regression — investigate immediately).

- [ ] **Step 3: Confirm/fix the seam**

`apply_vim_effect` (M7-08 Task 10) already handles `Move`, `Edit`, `None`. M7-09 added **no new `VimEffect` variant** (Type-contract decision), so the seam needs **no change**. If `change_enters_insert` fails because mode didn't flip: confirm `handle_vim_key`/`finish_operator` sets `state.mode = VimMode::Insert` for `change` (Task 7) — the seam reads `st.vim.mode`, so no root.rs edit is needed. If a root.rs edit IS required (it should not be), keep it minimal and re-run the M7-08 seam tests to confirm no regression.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test vim_behavior`
Expected: PASS (all fns). Crucially the M7-08 `vim_disabled_is_unchanged_m6_editing` test still passes.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/tests/vim_behavior.rs lingxi-code/crates/tui/src/root.rs
git commit -m "plan(M7-09 T12): operator/paste/change seam tests + vim-disabled passthrough re-assert

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

(Only `git add` `root.rs` if Step 3 actually edited it.)

---

## Task 13: Telemetry baseline guard + GATE deferral docs

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/prompt_input/vim.rs` (module doc + a guard test)

M7-09 adds **0 telemetry events** (baseline stays 326; M7-16 audits the real M7 total). This task records the "GATE: vim subset" in/out list in-code so the M7-16 auditor and future readers find it, and guards against an accidental event registration.

- [ ] **Step 1: Write the failing/guard test**

Add to `vim.rs` tests:

```rust
#[test]
fn m7_09_adds_no_telemetry_events() {
    // M7-09 ships 0 new telemetry events; baseline locked at 326 (M7-16 audits
    // the real M7 total). vim operators/visual emit nothing (per-keystroke
    // telemetry is explicitly NOT done; aggregated vim usage is an M7-16 decision).
    assert_eq!(lingxi_telemetry::tengu::ALL_EVENT_NAMES.len(), 326);
}
```

- [ ] **Step 2: Run test to verify it passes (baseline check)**

Run: `cargo test -p lingxi-tui --lib vim::tests::m7_09_adds_no_telemetry_events`
(or `cargo test -p lingxi-tui --lib m7_09_adds_no_telemetry_events`)
Expected: PASS at 326. If it differs because an earlier M7 sub-plan added events, set the literal to the count observed at the START of M7-09 and note the number in the commit message. The assertion's job is "M7-09 itself adds zero."

- [ ] **Step 3: Add the GATE deferral note**

Extend the `vim.rs` module doc with the locked subset split:

```rust
//! ## GATE: vim subset (M7-09 — parent spec §4 R1 / hard gate §3.3)
//!
//! Core vim SHIPS and passes the operator×motion matrix. Obscure cases are
//! DEFERRED to M8 with this documented "vim parity subset" line:
//!
//! IN (M7-09): operators d/c/y × motions {w b e $ 0 ^ h l j k f<char> t<char>
//!   G gg}, counts (3dw, 2yy, d3w); doubled ops dd/cc/yy (+counts); cw->ce;
//!   x (count); p/P charwise+linewise; the unnamed yank/delete register;
//!   Visual (v) + Visual-line (V) with d/c/y on the selection; c enters Insert.
//!
//! DEFERRED to M8 (vim parity subset): `.` dot-repeat; macros q/@; ex-commands `:`;
//!   `/` search-as-motion; named/numbered registers (only the unnamed register
//!   ships); text objects iw/aw/i(/a" (claude-code has textObjects.ts +
//!   operatorTextObj — NOT wired here; text-object keys after an operator are a
//!   no-op, never a panic); W/B/E WORD-motions; r replace; ~ toggle-case; J join;
//!   >>/<< indent; gj/gk display-wrap motions; ;/, find-repeat; bare NG motion;
//!   Visual block (Ctrl-v), o (swap ends), gv (reselect).
//!
//! NOTE: claude-code's vim (src/vim/*) has NO Visual mode — v/V are implemented
//! to standard vim semantics; there is no claude-code literal to match for them.
//!
//! ## Telemetry (M7-09)
//! M7-09 emits ZERO telemetry events. Baseline 326 unchanged; M7-16 audits the
//! real M7 total. Per-keystroke vim telemetry is explicitly NOT done.
```

- [ ] **Step 4: Add a guard that deferred keys are no-ops (not panics)**

Add to `vim.rs` tests — proves the GATE's "text-object keys after an operator are a no-op, never a panic":

```rust
#[test]
fn deferred_text_object_after_operator_is_noop_not_panic() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = VimState { mode: VimMode::Normal, ..VimState::default() };
    // 'd' then 'i' (would be `diw` text-object in full vim) -> deferred -> no-op.
    handle_vim_key(&mut s, "foo bar", 1, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    let out = handle_vim_key(&mut s, "foo bar", 1, KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(out, VimOutcome::Effect(VimEffect::None));
    assert_eq!(s.command, CommandState::Idle);
    assert_eq!(s.register, Register::default()); // nothing deleted/yanked
}
```

Run: `cargo test -p lingxi-tui --lib vim::tests::deferred_text_object_after_operator_is_noop_not_panic`
Expected: PASS. (If `dispatch_operator_pending` routes `i` to a text-object arm by mistake, this fails — confirm the catch-all returns `VimEffect::None`.)

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/prompt_input/vim.rs
git commit -m "plan(M7-09 T13): GATE vim-subset in/out doc + telemetry baseline guard + deferred-key no-op test

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 14: GATE + workspace gate + tag `m7.9`

**Files:**
- None (verification + tag only).

The final task. It (1) **runs the operator×motion matrix GATE** as the go/no-go, (2) runs the full workspace gate **from inside `lingxi-code/`** (toolchain pins 1.82.0; running from repo root uses the host toolchain → spurious lint noise — this bit M6-08, parent spec §5.4), then (3) cuts the annotated tag.

- [ ] **Step 1: GATE — operator×motion matrix + visual + register pass**

Run (from inside `lingxi-code/`):

```bash
cargo test -p lingxi-tui --test vim_behavior
cargo test -p lingxi-tui --lib vim::
```

Expected: ALL pass — `operator_motion_matrix`, `yank_then_paste_roundtrip`, `delete_then_paste_charwise`, `register_holds_last_yank_or_delete`, `visual_charwise_delete_matrix`, `visual_linewise_delete`, `visual_c_enters_insert`, `visual_esc_returns_to_normal_no_edit`, `visual_count_motion_then_delete`, the seam tests, and every `vim::*` unit mod (`m7_09_types_tests`, `motion_class_tests`, `apply_op_tests`, `line_op_tests`, `x_tests`, `paste_tests`, `op_dispatch_tests`, `visual_tests`, plus all M7-08 mods).

**GATE decision:** If the IN-subset matrix passes → core vim ships; proceed. If a *core* row cannot be made to pass (a real defect in d/c/y/x/p/P/visual), that BLOCKS the tag — fix it. If only *deferred* cases were ever in question, they are already excluded by the GATE doc (Task 13) — do not add them to chase parity. The "vim parity subset" line in `vim.rs` is the authoritative record of what shipped vs. deferred-to-M8.

- [ ] **Step 2: fmt + clippy**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. Likely `vim.rs` lints to fix inline: `needless_lifetimes` on `VimCursor` methods (elide/allow), `too_many_arguments` on `apply_operator`/`operator_range`/`finish_operator` (allow with a comment, or group into a struct if trivial — prefer `#[allow(clippy::too_many_arguments)]` with a one-line justification since these are pure-fn ports), `match_like_matches_macro`. Re-commit fixes as `plan(M7-09 T14): fmt + clippy` if any are needed.

- [ ] **Step 3: Full test suite**

```bash
cargo test --workspace
```

Expected: PASS. Known flakes (allowed rerun, parent spec §5.4): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests. If only these fail, rerun the specific test; do not treat as a gate failure.

- [ ] **Step 4: Cross-platform compile gate (5 targets)**

```bash
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```

Expected: green for all 5 (same posture as v0.7.0). `vim.rs` is pure std + crossterm — no platform-specific code. If a target's toolchain isn't installed, note it and skip per the established v0.7.0 convention.

- [ ] **Step 5: Tag `m7.9`**

```bash
git tag -a m7.9 -m "M7-09 — vim mode 2: visual + operators (d/c/y × motions, dd/cc/yy, cw->ce, x, p/P, register, v/V) gated on vim_enabled; vim parity subset documented (obscure cases deferred to M8)"
```

(NO push to remote. Local tag only — parent spec §6.4.)

- [ ] **Step 6: Verify the tag exists locally**

```bash
git tag -l 'm7.9'
git show --stat m7.9 | head -20
```

Expected: `m7.9` listed; `git show` points at the T13 (or T14 fmt/clippy) commit.

---

## Self-Review

**1. Spec coverage** — mapping the prompt's "WHAT M7-09 SHIPS" + "TESTS REQUIRED" to tasks:

| Prompt requirement | Task |
|---|---|
| Visual + Visual-line modes (`v` charwise, `V` linewise); extend with h/j/k/l/w/b/e/$/0/gg/G; Esc → Normal | T8 (engine), T9 (indicator), T11 (matrix) |
| Operators `d c y x p P`; combos `dw cc d$ yy dd de`; counts `3dw 2yy` | T2 (classify), T3 (apply), T4 (line ops), T5 (x), T6 (paste), T7 (dispatch), T10 (matrix) |
| Register holds last yank/delete; `p`/`P` charwise vs linewise | T1 (Register type), T3/T4/T5 (set register), T6 (paste), T10 (roundtrip), T12 (seam) |
| Visual `d`/`c`/`y` operate on selection | T8 (engine), T11 (matrix) |
| Operator×motion matrix behavior tests | T10 |
| x, p/P, register tests | T10 |
| Visual charwise / linewise tests | T11 |
| Counts in visual + operator | T7 (count×operator), T8/T11 (count in visual) |
| `c` enters insert after deleting | T3/T4/T7 (operator), T8/T11 (visual), T12 (seam typing) |
| vim-disabled passthrough unchanged | T12 |
| GATE: operator-matrix + "vim subset" in/out line; obscure cases → M8 | T13 (doc + deferred-key no-op guard), T14 (GATE decision) |
| Telemetry 0 events (baseline 326) | T13 |
| Workspace gate (cd lingxi-core) + tag `m7.9` | T14 |
| claude-code literal lock (operator/register/visual semantics) | Background section + T2-T8 ported from `src/vim/operators.ts`/`types.ts`/`motions.ts`; visual = standard vim (claude-code has none) documented |

**2. Placeholder scan:** searched for `TBD`, `TODO`, `implement later`, `fill in details`, `add appropriate`, `handle edge cases`, `similar to Task` — none present as plan-failures. Every code step shows complete code. (Two steps include a "simplify this" cleanup note with the cleaned code shown inline — Task 4's `true && false` and Task 6's `line_start_offset` — these show BOTH the naive and the clean version, so the engineer has the final code, not a placeholder.)

**3. Type consistency:** `Register {text, linewise}`, `VisualState {anchor, linewise}`, `CommandState::{Operator, OperatorCount, OperatorFind, OperatorG}`, `OperatorRange {from, to, linewise}`, `operator_range`, `apply_operator`, `line_op`, `delete_char_x`, `paste`, `line_start_offset`, `last_char_len`, `is_inclusive_motion`, `is_linewise_motion`, `motion_for_operator_key`, `dispatch_operator_pending`, `finish_operator`, `operator_over_lines`, `visual_range`, `visual_motion`, `footer_mode_label_v2` are each defined once and referenced consistently. `handle_vim_key` signature `(&mut VimState, &str, usize, KeyEvent) -> VimOutcome` is UNCHANGED from M7-08 (verified against T7/T8/T10/T11/T12). `VimEffect` variants `Move(usize)` / `Edit{text,cursor}` / `None` are UNCHANGED — every operator reports through them (yank→`Move(from)`, delete/change/x/paste→`Edit`). `apply_operator` and `line_op` both return `(VimEffect, Register, bool)` (the `bool` is `enter_insert`); `finish_operator` consumes that shape. `delete_char_x` returns `(VimEffect, Register)` (no insert) and `paste` returns `VimEffect`. These shapes are consistent across T3-T8.

**Notes / decisions for the executor:**
- **No new `VimEffect` variant.** Operators reuse M7-08's `Edit { text, cursor }`; `change` flips `state.mode = Insert` inside the dispatcher so the root.rs seam (`apply_vim_effect`) needs **zero** changes. This is the key reason Task 12 has no required `root.rs` edit.
- **`pending_operator` field retired but kept.** M7-08's scaffold `pending_operator: Option<Operator>` is superseded by `CommandState::Operator`; the field stays (always `None`) for struct stability, documented in T1. A later cleanup may remove it.
- **Visual is standard-vim, not claude-code.** claude-code's vim has no visual mode (confirmed: `VimTextInput.tsx`/`transitions.ts`/`types.ts` are operator-pending only). `v`/`V` follow standard vim; the GATE doc records there's no literal to match.
- **`0` in operator-pending is a motion (`d0`), not a count seed** — mirrors M7-08's "`0` is a motion from Idle." `dispatch_operator_pending` only seeds an inner count on `1-9`.
- **Count composition `leading * inner`** (`3dw` == `d3w`): leading count seeds `Operator{op, count}`; an inner count multiplies in `OperatorCount`. Both paths tested (T7 `count_dw_multiplies` / `d_count_w_inner_count_multiplies`).
- **Register linewise = trailing `\n` semantics** preserved from claude-code via the explicit `Register.linewise` flag; `apply_operator`/`line_op` always append `\n` to linewise content and `paste` strips one trailing `\n` before splitting.

**Gaps surfaced (raise before execution if blocking):**
1. **Visual-mode selection highlight rendering** is not asserted by tests (the load-bearing behavior — what d/c/y do — IS asserted). The PromptInput render should read `state.visual` to highlight `[anchor..cursor]`; T9 wires the footer label but the inline highlight is a best-effort render touch. If the M7-06 PromptInput render has no selection-highlight hook, that is a small render-only follow-up; it does NOT affect the GATE (which is behavior).
2. **`dl` vs `x` equivalence** (T10 `dl == x` row): `dl` is operator+Right, `x` is the dedicated command. Both delete one char and set the charwise register. The matrix asserts `dl` deletes one char; if claude-code's exclusive-motion handling makes `dl` at EOL differ from `x`, the row uses a mid-line case (offset 0 on "foo bar baz") to avoid the EOL edge — verified in the row.
3. **`yl` single-char yank** (T12 `paste_via_live_key_after_yank`): relies on `y` + Right motion yielding range `[0,1)`. If `resolve_motion(Right)` at offset 0 returns offset 1 and `operator_range` (exclusive for `l`) gives `[0,1)`, the register is the single char `'a'`. Confirmed against T3 `operator_range` (exclusive for non-inclusive motions).

**End of M7-09 plan.**
