# TUI Scrollback Usability Design

## Goal
Fix the main chat scrollback usability issues reported by the user:
1. The main interface cannot scroll.
2. Scrollback content cannot be copied with normal terminal selection.
3. Cursor feedback is missing: both the input cursor and scroll position are unclear.

Scope is the primary TUI chat screen only. Prompt focus must stay in the input box while scrolling.

## Current Context
The TUI already has a line-based scroll model:
- `tui/src/app.rs`: `scroll_with_viewport(st, dir, viewport_height)` updates `AppState.scroll_offset`.
- `tui/src/components/virtual_message_list.rs`: `VirtualMessageList` consumes `scroll_offset` and uses `HeightCache`/line measurement.
- `tui/tests/behavior_scroll.rs` and `behavior_virtual_window.rs`: existing scroll math coverage.

Copy support exists but is programmatic, not terminal-native:
- `/copy` sets `AppState.pending_copy_clipboard`.
- `root::pump_copy_clipboard` writes using native utilities (`pbcopy`, `clip`, `wl-copy`, `xclip`, `xsel`).
- No OSC-52 fallback is currently confirmed.

Cursor handling is split:
- `PromptInputProps.show_cursor` renders an in-component caret in `tui/src/components/prompt_input/mod.rs`.
- `tui/src/terminal.rs` restores terminal cursor on exit via `crossterm::cursor::Show`.
- No normal-runtime cursor show/hide path was found besides exit restore.

## Chosen Approach: Keep Input Focus, Fix Scrollback In Place
Adopt the lightweight approach selected during brainstorming:
- Keyboard and mouse wheel scroll the scrollback.
- Input focus stays in the prompt. Typing can continue without entering a separate scroll mode.
- Terminal-native text selection/copy must work by default.
- Add visible feedback for scroll position and prompt cursor.

## Interaction Design

### Keyboard Scrolling
Support these keys in the main chat screen when no modal/palette/completion/focus trap owns input:
- `PageUp` / `PageDown`: page scroll.
- `Home` / `End`: top/bottom.
- `Up` / `Down` and `j` / `k`: line scroll only when they are not consumed by the prompt editor or vim mode.

The implementation should route through the existing `scroll_with_viewport` function, not create a second scroll model.

### Mouse / Trackpad Scrolling
Mouse wheel events should update `AppState.scroll_offset` using the same `scroll_with_viewport` path.

Do not enable broad mouse capture if it prevents terminal text selection. If mouse capture is currently enabled globally, narrow it or disable it by default so terminal drag-select copy works. Wheel support should be implemented only if it can coexist with terminal-native selection.

### Terminal-Native Copy
Primary copy mechanism is terminal-native selection. The TUI should not steal drag selection in the main scrollback. `/copy` remains as a secondary command but is not the target of this fix.

If terminal-native selection and mouse wheel support conflict, prefer terminal-native copy and keep keyboard scrolling fully functional.

### Cursor Feedback
Two indicators are required:
1. Prompt cursor: ensure `PromptInputProps.show_cursor` is true while the prompt is focused and the rendered caret is visible in normal and vim insert modes.
2. Scroll position: show a lightweight scroll indicator when `scroll_offset > 0`, e.g. `Scrolled N lines` or a small right-edge marker. It disappears at bottom.

Do not rely on terminal hardware cursor positioning inside the iocraft render loop unless there is an existing safe abstraction. The prompt caret can remain component-rendered.

## Architecture

### Affected Modules
- `tui/src/root.rs`: event conversion, mouse event handling, pump scheduling, possible mouse capture configuration.
- `tui/src/app.rs`: dispatch and `scroll_with_viewport` call sites.
- `tui/src/state.rs`: existing `scroll_offset`, `prompt_cursor`, optional additional UI state for scroll indicator if needed.
- `tui/src/components/virtual_message_list.rs`: render scroll indicator if it belongs with the viewport.
- `tui/src/components/prompt_input/mod.rs`: ensure visible prompt caret.

### Data Flow
1. Terminal key or mouse wheel event arrives in `root.rs`.
2. If no overlay consumes it, convert to `ScrollDir` + current viewport height.
3. Call `scroll_with_viewport(&mut AppState, dir, viewport_height)`.
4. `AppState.scroll_offset` updates.
5. `VirtualMessageList` renders the correct line window.
6. Scroll indicator renders when offset is nonzero.

## Error Handling / Edge Cases
- Empty scrollback: scrolling is a no-op.
- Short scrollback smaller than viewport: offset clamps to 0.
- While palette/dialog/completion is open: those components keep priority; scrollback does not move.
- Vim normal mode: keep existing vim cursor semantics. Only route scroll keys that are not owned by vim.
- Terminal without mouse support: keyboard scroll still works.
- Terminal selection conflict: if mouse capture must be disabled to support selection, disable it and document that mouse wheel may be terminal-dependent.

## Testing Plan
- Extend existing scroll tests (`behavior_scroll.rs`, `behavior_virtual_window.rs`) for keyboard mappings if missing.
- Add root/event-level test for mouse wheel → `scroll_offset` when main screen focused.
- Add focus-trap regression: palette/dialog open means wheel/down/up does not scroll underlying chat.
- Add prompt cursor snapshot/behavior test proving caret renders when prompt focused.
- Add render snapshot for nonzero scroll offset showing the scroll indicator.
- Manual QA in a real terminal:
  1. Generate long scrollback.
  2. Scroll with PgUp/PgDown/Home/End.
  3. Scroll with mouse wheel/trackpad.
  4. Type while scrolled; prompt remains focused.
  5. Drag-select scrollback text and copy with terminal-native shortcut.
  6. Confirm prompt cursor visible.

## Non-Goals
- No separate scroll mode.
- No selectable message list UI in this iteration.
- No redesign of `/copy` or clipboard pump.
- No broad terminal cursor rewrite unless required to make the prompt caret visible.

## Open Implementation Questions
- Whether iocraft currently enables mouse capture globally. If yes, choose between: disable global capture by default, or handle only wheel without stealing drag selection if the backend supports that distinction.
- Exact viewport height source to use for mouse wheel events in `root.rs`; use the same value currently passed to keyboard scroll.
