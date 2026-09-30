# TUI Scrollback Usability Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the main TUI chat scrollback scrollable with keyboard and mouse/trackpad where compatible with terminal-native selection, while keeping prompt focus and visible cursor/scroll feedback.

**Architecture:** Use the existing line-based scroll model (`AppState.scroll_offset` + `scroll_with_viewport` + `VirtualMessageList`) instead of adding a second mode. Keep the prompt focused; route scroll inputs to scrollback only when overlays do not own input. Preserve terminal-native text selection by avoiding broad mouse capture unless wheel-only handling is proven safe.

**Tech Stack:** Rust, iocraft TUI, crossterm key/mouse event mapping, existing `tui` behavior/snapshot tests.

---

## Task 1: Lock current keyboard scroll behavior with viewport-height tests

**Files:**
- Modify: `apps/cli/tui/tests/behavior_scroll.rs`
- Modify if needed: `apps/cli/tui/tests/behavior_virtual_window.rs`

**Step 1: Add/confirm failing keyboard mapping tests**

Add tests for the user-facing keyboard contract if missing:

```rust
#[test]
fn page_home_end_scroll_the_main_scrollback() {
    let mut st = long_scrollback_state(120);
    let vh = 10;

    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    assert_eq!(st.scroll_offset, 8);

    scroll_with_viewport(&mut st, ScrollDir::Top, vh);
    assert!(st.scroll_offset > 0);

    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}
```

If helper names differ, use the existing fixture helpers in `behavior_scroll.rs` / `behavior_virtual_window.rs` rather than creating a new state model.

**Step 2: Run test to verify baseline**

Run:

```bash
cd lingxi-code
cargo test -p tui --test behavior_scroll -- --nocapture
cargo test -p tui --test behavior_virtual_window -- --nocapture
```

Expected: existing scroll math should already pass. If not, capture the failure before code changes.

**Step 3: Commit**

```bash
git add apps/cli/tui/tests/behavior_scroll.rs apps/cli/tui/tests/behavior_virtual_window.rs
git commit -m "test(tui): lock scrollback keyboard scrolling"
```

---

## Task 2: Route live keyboard scroll events through the real viewport height

**Files:**
- Modify: `apps/cli/tui/src/root.rs:1450-1485` (current `scroll_with_viewport(st, dir, viewport)` call site)
- Inspect: `apps/cli/tui/src/events/keymap.rs:160-180` and `tui/src/app.rs:600-612`
- Test: `apps/cli/tui/tests/cross_state_seam_test.rs` or add focused behavior test if an event-level helper exists

**Step 1: Write failing event-level test**

Create or extend a root/app dispatch test proving that when the main screen is focused and no palette/dialog/completion is open, `KeyAction::ScrollStep(ScrollDir::PageUp)` uses the live viewport height, not the fallback `1` used by plain `app::dispatch`.

Pseudo-shape:

```rust
#[test]
fn root_scroll_step_uses_live_viewport_height() {
    let mut st = long_scrollback_state(120);
    let viewport = 20;
    apply_root_scroll_step(&mut st, ScrollDir::PageUp, viewport);
    assert_eq!(st.scroll_offset, 18);
}
```

Use the existing root test seam if present. If no seam exists, extract a tiny pure helper from `root.rs`:

```rust
pub(crate) fn apply_scrollback_scroll(st: &mut AppState, dir: ScrollDir, viewport: usize) {
    scroll_with_viewport(st, dir, viewport);
}
```

**Step 2: Run failing test**

```bash
cd lingxi-code
cargo test -p tui root_scroll_step_uses_live_viewport_height -- --nocapture
```

Expected: fail if root path uses incorrect viewport or no test seam exists.

**Step 3: Minimal implementation**

Ensure the real root event path calls `scroll_with_viewport(st, dir, viewport)` with the current scrollback viewport height. Do not change `app::dispatch` fallback unless required; `dispatch` may keep its unit fallback for tests/non-root callers.

**Step 4: Run verification**

```bash
cargo test -p tui root_scroll_step_uses_live_viewport_height -- --nocapture
cargo test -p tui --test behavior_scroll -- --nocapture
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/cli/tui/src/root.rs apps/cli/tui/tests/cross_state_seam_test.rs
git commit -m "fix(tui): scroll chat with live viewport height"
```

---

## Task 3: Add mouse/trackpad wheel scrolling without stealing terminal selection

**Files:**
- Modify: `apps/cli/tui/src/root.rs` (iocraft/crossterm mouse event conversion)
- Inspect: `apps/cli/tui/src/terminal.rs:21-51` (only disables mouse capture on restore)
- Test: add/extend a root event test under `apps/cli/tui/tests/`

**Step 1: Confirm mouse capture state**

Search in code before editing:

```bash
grep -RIn "EnableMouseCapture\|DisableMouseCapture\|MouseEvent\|FullscreenMouse" apps/cli/tui/src
```

Expected: no broad `EnableMouseCapture` in app code; `DisableMouseCapture` only on restore. If broad capture exists, disable/narrow it before adding wheel handling.

**Step 2: Write failing wheel test**

Add a test for wheel up/down in main chat focus:

```rust
#[test]
fn mouse_wheel_scrolls_scrollback_without_prompt_focus_change() {
    let mut st = long_scrollback_state(120);
    st.prompt_text = "typing".into();
    st.prompt_cursor = st.prompt_text.len();

    apply_mouse_wheel(&mut st, MouseWheel::Up, 10);

    assert!(st.scroll_offset > 0);
    assert_eq!(st.prompt_text, "typing");
    assert_eq!(st.prompt_cursor, "typing".len());
}
```

Use existing event conversion types in `root.rs`; if no test seam exists, extract a small pure helper for wheel→`ScrollDir` mapping.

**Step 3: Implement wheel mapping**

Map wheel up/down to `ScrollDir::LineUp` / `LineDown` or page increments if current UX favors trackpad acceleration. Start with line-based increments to avoid surprising jumps.

Rules:
- Do nothing when palette/dialog/completion/focus-trap owns input.
- Keep prompt focus and cursor unchanged.
- Do not enable broad mouse capture if terminal drag selection would break. If wheel events require capture in iocraft, gate it behind a narrow runtime choice and document the tradeoff in code comments.

**Step 4: Run tests**

```bash
cargo test -p tui mouse_wheel_scrolls_scrollback_without_prompt_focus_change -- --nocapture
cargo test -p tui --test focus_trap_test -- --nocapture
```

Expected: pass.

**Step 5: Manual QA**

In a real terminal:
1. Generate long scrollback.
2. Use trackpad/wheel over chat.
3. Drag-select chat text and copy with terminal shortcut.
4. Confirm prompt text remains editable.

**Step 6: Commit**

```bash
git add apps/cli/tui/src/root.rs apps/cli/tui/tests/<new-or-existing-test>.rs
git commit -m "fix(tui): scroll chat with mouse wheel"
```

---

## Task 4: Render scroll position feedback when not at bottom

**Files:**
- Modify: `apps/cli/tui/src/components/virtual_message_list.rs`
- Possibly modify: `apps/cli/tui/src/screens/repl.rs` if viewport chrome belongs there
- Test: add snapshot or behavior test in `apps/cli/tui/tests/behavior_virtual_window.rs` or snapshot tests

**Step 1: Write failing test**

Add a render/snapshot test proving nonzero `scroll_offset` shows an indicator and bottom state hides it.

Expected UI text can be simple and stable:
- Nonzero: `Scrolled 42 lines`
- Zero: no indicator

Example assertion:

```rust
#[test]
fn scroll_indicator_appears_only_when_scrolled_up() {
    let mut st = long_scrollback_state(120);
    st.scroll_offset = 42;
    let rendered = render_repl_to_text(&st);
    assert!(rendered.contains("Scrolled 42 lines"));

    st.scroll_offset = 0;
    let rendered = render_repl_to_text(&st);
    assert!(!rendered.contains("Scrolled"));
}
```

Use existing render test helpers; do not create a separate renderer.

**Step 2: Run failing test**

```bash
cargo test -p tui scroll_indicator_appears_only_when_scrolled_up -- --nocapture
```

Expected: fail until indicator is rendered.

**Step 3: Implement indicator**

Preferred location: `VirtualMessageList` footer/overlay if it already knows `scroll_offset` and viewport height. If the component lacks enough context, render the indicator in `screens/repl.rs` near the scrollback container.

Keep it low-noise:
- `scroll_offset == 0`: render nothing.
- `scroll_offset > 0`: dim text, e.g. `Scrolled {scroll_offset} lines`.

**Step 4: Run tests**

```bash
cargo test -p tui scroll_indicator_appears_only_when_scrolled_up -- --nocapture
cargo test -p tui --test behavior_virtual_window -- --nocapture
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/cli/tui/src/components/virtual_message_list.rs apps/cli/tui/src/screens/repl.rs apps/cli/tui/tests/<test-file>.rs
git commit -m "feat(tui): show scrollback position"
```

---

## Task 5: Ensure prompt cursor is visible while prompt is focused

**Files:**
- Inspect/modify: `apps/cli/tui/src/screens/repl.rs:330-360` (current `show_cursor: true`)
- Inspect/modify: `apps/cli/tui/src/components/prompt_input/mod.rs:387-432`
- Test: `apps/cli/tui/tests/prompt_input_footer_snapshot.rs` or `render_repl_screen.rs`

**Step 1: Write failing cursor visibility test**

Add a render test proving the prompt caret appears in the primary chat screen when prompt focused. Existing component tests prove `PromptInput(show_cursor=true)` renders a cursor; this test should prove the screen passes `show_cursor: true` and the caret survives screen composition.

Example shape:

```rust
#[test]
fn repl_screen_renders_prompt_cursor_when_focused() {
    let mut st = AppState::default();
    st.prompt_text = "abc".into();
    st.prompt_cursor = 1;
    let rendered = render_repl_to_text_or_chunks(&st);
    assert_contains_cursor_marker(rendered);
}
```

If text snapshots cannot capture inverse-video chunks, assert through the component tree/test helper that `PromptInputProps.show_cursor == true`.

**Step 2: Run failing test**

```bash
cargo test -p tui repl_screen_renders_prompt_cursor_when_focused -- --nocapture
```

Expected: fail if cursor not visible or no assertion seam exists.

**Step 3: Minimal implementation**

Likely options:
- Ensure all primary REPL screen `PromptInput` constructions pass `show_cursor: true` while no modal owns focus.
- If component caret exists but is visually too subtle, adjust `render_line_with_cursor` style to use an obvious inverse-video/block glyph without changing cursor index semantics.

Do not add terminal hardware cursor positioning unless component caret cannot solve it.

**Step 4: Run tests**

```bash
cargo test -p tui repl_screen_renders_prompt_cursor_when_focused -- --nocapture
cargo test -p tui prompt_input -- --nocapture
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/cli/tui/src/screens/repl.rs apps/cli/tui/src/components/prompt_input/mod.rs apps/cli/tui/tests/<test-file>.rs
git commit -m "fix(tui): keep prompt cursor visible"
```

---

## Task 6: Preserve terminal-native copy behavior

**Files:**
- Inspect/modify: `apps/cli/tui/src/root.rs`
- Inspect: `apps/cli/tui/src/terminal.rs`
- Test/manual QA required

**Step 1: Verify no broad mouse capture**

Run:

```bash
grep -RIn "EnableMouseCapture\|DisableMouseCapture" apps/cli/tui/src
```

Expected acceptable state:
- `DisableMouseCapture` on restore is fine.
- No unconditional `EnableMouseCapture` for the main chat.

**Step 2: If broad capture exists, write a failing test or narrow it**

If unconditional mouse capture exists, refactor it so normal terminal drag selection is not captured in the main chat. Prefer keyboard scroll + no mouse capture over breaking terminal-native copy.

**Step 3: Manual QA**

Because terminal-native selection is emulator behavior, lock this with manual verification:

```text
1. Run LingXi TUI in a normal terminal.
2. Produce several assistant/user messages.
3. Drag-select text in the scrollback.
4. Copy using terminal shortcut/menu.
5. Paste elsewhere; selected text should be present.
6. Scroll with keyboard. If mouse wheel is enabled, verify wheel still works without breaking drag selection.
```

**Step 4: Document any terminal limitation**

If the backend cannot support both wheel capture and drag selection, update the code comment and spec implementation notes: keyboard scrolling is guaranteed; mouse wheel is best-effort and must not break terminal-native selection.

**Step 5: Commit**

```bash
git add apps/cli/tui/src/root.rs apps/cli/tui/src/terminal.rs docs/superpowers/specs/2026-06-30-tui-scrollback-usability-design.md
git commit -m "fix(tui): preserve terminal-native scrollback copy"
```

---

## Task 7: Full regression and manual smoke test

**Files:**
- No source edits expected.

**Step 1: Run TUI tests**

```bash
cd lingxi-code
cargo test -p tui --test behavior_scroll -- --nocapture
cargo test -p tui --test behavior_virtual_window -- --nocapture
cargo test -p tui --test focus_trap_test -- --nocapture
cargo test -p tui --test render_repl_screen -- --nocapture
cargo test -p tui
```

Expected: all pass.

**Step 2: Run workspace-relevant build**

```bash
cargo build -p tui -p cli
```

Expected: build succeeds.

**Step 3: Manual smoke**

Run the CLI TUI and verify:
- PgUp/PgDown/Home/End scroll history.
- Mouse wheel scrolls if supported without breaking drag selection.
- Prompt remains focused while scrolled.
- Prompt cursor visible while typing.
- Scroll indicator appears only when `scroll_offset > 0`.
- Terminal-native drag selection copies chat text.

**Step 4: Final commit if needed**

If any test snapshots or docs update in this task:

```bash
git add <changed-files>
git commit -m "test(tui): verify scrollback usability"
```

---

## Implementation Order
1. Task 1 — lock existing scroll math.
2. Task 2 — live keyboard scroll path.
3. Task 3 — mouse wheel path, only if compatible with terminal-native selection.
4. Task 4 — scroll position indicator.
5. Task 5 — prompt cursor visibility.
6. Task 6 — terminal-native copy preservation.
7. Task 7 — regression + manual smoke.

## Notes for Executor
- Do not create a separate scroll mode.
- Keep prompt focus during scrolling.
- Reuse `scroll_with_viewport`; do not add another scroll offset model.
- Treat terminal-native selection as more important than mouse wheel if the terminal backend forces a tradeoff.
- Prefer focused helper extraction over large refactors.
