# Plan: Align `tui-rata` UI Structure With Codex TUI

- **Status:** Planning artifact only. No implementation performed in this turn.
- **Date:** 2026-07-02
- **Scope:** `lingxi-code/tui-rata`, with necessary test-harness and cutover updates under `lingxi-code`.
- **Non-negotiable:** This plan has no deferred implementation buckets. Every `tui-rata` feature already represented by the current crate, `tui-core::message::RenderedMessage`, `SessionInfo`, `TurnEvent`, or `PermissionExchange` must end with a complete ratatui implementation, tests, and verification.

## Requirements Summary

1. Make `tui-rata` structurally match the Codex TUI shape: terminal runtime -> app loop -> chat widget -> transcript/history cells -> bottom pane -> bottom-pane views.
2. Keep LingXi's protocol/data model. Do not import Codex app-server, auth, plugin, skill, thread, rate-limit, or telemetry product types just to reuse UI code.
3. Reuse or port low-coupling Codex infrastructure patterns where appropriate: native scrollback, fixed bottom viewport, `HistoryCell`, `BottomPaneView`, renderable height/cursor contracts, and view-stack input routing.
4. Replace the current monolithic `RataApp` responsibilities with explicit ownership boundaries.
5. Remove scaffold-only behavior. No `RenderedMessage` variant may silently render `Vec::new()` unless the source variant is intentionally non-visible and documented with a test.
6. Replace the current simplified screen model with real interactive bottom-pane/full-screen views for all currently exposed slash surfaces.
7. Preserve the existing no-iocraft rule for `tui-rata`.
8. Add regression tests before structural movement where current behavior exists, then refactor while keeping tests green.
9. Add enough ratatui `TestBackend` or equivalent buffer tests to prove layout, cursor, overlays, active streaming, native scrollback commits, resize/reflow, and all message variants.
10. Complete cutover readiness: `tui-rata` must be able to replace the old `tui` crate for the CLI path with no hidden "later" work for existing LingXi TUI features.

## Evidence and Current State

- `tui-rata` is already a separate ratatui crate and explicitly must not depend on iocraft in `lingxi-code/tui-rata/src/lib.rs:1-14`.
- The crate currently exports `RataTerminal` as `ratatui::Terminal<CrosstermBackend<Stdout>>` and constructs `Viewport::Inline` in `lingxi-code/tui-rata/src/lib.rs:45-95`.
- A custom terminal that better matches Codex's bottom-anchored native scrollback model already exists but is not the public runtime terminal in `lingxi-code/tui-rata/src/custom_terminal.rs:1-13` and `lingxi-code/tui-rata/src/custom_terminal.rs:105-240`.
- `RataApp` is currently a monolith: messages, composer, theme, committed count, current turn, permission dialog, model picker, completion popup, verbose mode, vim state, spinner state, Ctrl-C state, and session snapshot all live in one struct at `lingxi-code/tui-rata/src/app.rs:66-100`.
- `RataApp::on_key` directly routes permission, picker, completion, vim, and composer input in `lingxi-code/tui-rata/src/app.rs:133-168`.
- `RataApp::submit_composer` directly records user messages, slash commands, current turn state, and submit callbacks in `lingxi-code/tui-rata/src/app.rs:171-190`.
- `RataApp::sync_completion` directly owns slash/file completion state in `lingxi-code/tui-rata/src/app.rs:570-586`.
- `RataApp::viewport_height`, `flush_scrollback`, spinner text, status rendering, composer rendering, cursor placement, and overlay rendering are all in `lingxi-code/tui-rata/src/app.rs:588-738`.
- The event loop directly mutates app state, polls terminal input, flushes history, resizes viewport, and renders in `lingxi-code/tui-rata/src/app.rs:777-840`.
- `message.rs` documents that some variants still remain the iocraft renderer responsibility and returns empty output for unhandled variants in `lingxi-code/tui-rata/src/message.rs:1-8` and `lingxi-code/tui-rata/src/message.rs:32-44`.
- `RenderedMessage` has a broad surface that must be covered: core user/assistant/system/tool variants in `lingxi-code/tui-core/src/message.rs:5-62`, thinking/system/rate-limit/team/advisor variants in `lingxi-code/tui-core/src/message.rs:63-180`, command/output/image/attachment variants in `lingxi-code/tui-core/src/message.rs:181-254`, and grouped/collapsed variants in `lingxi-code/tui-core/src/message.rs:255-288`.
- `screens.rs` still documents static/default-chord scaffold simplification in `lingxi-code/tui-rata/src/screens.rs:1-12`; `/help`, `/doctor`, `/mcp`, `/hooks`, and `/agents` currently print read-only text into scrollback rather than using the full interactive view model.
- Current completion and model picker navigation clamps at edges in `lingxi-code/tui-rata/src/palette.rs:91-101` and `lingxi-code/tui-rata/src/picker.rs:67-90`; this is acceptable only if tests lock it as a deliberate LingXi behavior, otherwise fix it during view-stack implementation.
- Codex uses a thin `App::render_chat_widget_frame` wrapper that asks the chat widget for desired height, render, cursor position, and cursor style in `codex/codex-rs/tui/src/app.rs:1349-1361`.
- Codex documents the intended chat-surface split: `ChatWidget` consumes protocol events, builds committed `HistoryCell`s, maintains an active in-flight cell, and drives overlays in `codex/codex-rs/tui/src/chatwidget.rs:1-28`.
- Codex `ChatWidget` owns `BottomPane` and transcript state as separate fields in `codex/codex-rs/tui/src/chatwidget.rs:516-533`.
- Codex commits active cells through a `HistoryCell` boundary in `codex/codex-rs/tui/src/chatwidget.rs:1207-1239`.
- Codex bottom pane owns composer plus a stack of transient `BottomPaneView`s and explicitly splits local input routing from parent-level interrupt/quit decisions in `codex/codex-rs/tui/src/bottom_pane/mod.rs:1-15` and `codex/codex-rs/tui/src/bottom_pane/mod.rs:211-243`.
- Codex bottom pane exposes render, desired height, cursor position, and cursor style through a renderable contract in `codex/codex-rs/tui/src/bottom_pane/mod.rs:1728-1840`.
- Codex `HistoryCell` is the unit of conversation history and defines `display_lines`, `raw_lines`, hyperlink lines, and desired height in `codex/codex-rs/tui/src/history_cell/mod.rs:1-11` and `codex/codex-rs/tui/src/history_cell/mod.rs:180-230`.
- The existing migration plan already chose a three-crate topology and a no-iocraft `tui-rata` target in `.omo/plans/2026-07-01-tui-iocraft-to-ratatui-migration.md:35-50`.
- The existing migration plan's goal includes fixed bottom composer/footer, independent scrollback, overlays, resize reflow, streaming control, and permission prompt control in `.omo/plans/2026-07-01-tui-iocraft-to-ratatui-migration.md:10-19`.

## Architectural Decision

Use Codex's UI architecture as the target shape, not Codex's product-specific implementation as the source of truth.

### Target module topology

```text
lingxi-code/tui-rata/src/
  lib.rs
  terminal.rs or custom_terminal.rs
  renderable.rs
  app.rs
  chat_widget.rs
  transcript.rs
  history_cell/
    mod.rs
    message.rs
    tool.rs
    system.rs
    team.rs
    attachments.rs
  bottom_pane/
    mod.rs
    view.rs
    composer_view.rs
    status.rs
    completion_view.rs
    model_picker_view.rs
    permission_view.rs
    screen_view.rs
    pending_input_preview.rs
  command.rs
  files.rs
  image_view.rs
  session.rs
  style_adapter.rs
  term_image.rs
  vim.rs
```

The final exact filenames can differ if local Rust module ergonomics demand it, but the ownership boundaries must not collapse back into one `RataApp`.

### Ownership boundaries

`Terminal`
- Owns raw terminal IO, bracketed paste, bottom viewport placement, buffer diffing, native scrollback insertion, cursor style, cursor position, resize tracking, restore-on-drop safeguards.
- Must not know about messages, composer, permissions, slash commands, or models.

`App`
- Owns the top-level event loop, channel draining, terminal height changes, redraw cadence, and callbacks to the embedding CLI/orchestrator.
- Must not render composer internals, permission dialogs, model pickers, or individual message variants.

`ChatWidget`
- Owns transcript, active streaming cell, bottom pane, session snapshot, current turn state, high-level key outcomes, slash command dispatch, and permission/model routing.
- Turns `TurnEvent`, `PermissionExchange`, paste events, and key events into UI state updates or caller callbacks.
- Exposes `desired_height(width)`, `render(area, buf)`, `cursor_pos(area)`, and `cursor_style(area)` to `App`.

`Transcript`
- Owns committed history cells, active cell, commit cursor, native scrollback flush state, live-tail rendering, verbose/expanded state, and raw/rich render mode.
- Converts `RenderedMessage` into `HistoryCell`.

`HistoryCell`
- Owns message-specific rendering and height measurement.
- Must support `display_lines(width)`, `raw_lines()`, `desired_height(width)`, and optional animation/live cache keys.
- Must cover every `RenderedMessage` variant from `tui-core::message`.

`BottomPane`
- Owns composer, status row, task-running indicator, pending/queued input preview, completion popup, and `view_stack`.
- Routes keys first to active view, then completion/composer/vim/history search.
- Returns local outcomes to `ChatWidget` without deciding process-level interrupt/quit policy by itself.

`BottomPaneView`
- Owns transient focused surfaces: permission prompt, model picker, help/doctor/mcp/hooks/agents/tasks/settings/theme/stats screens, file/command completion alternatives, and future form-like views.
- Implements render, desired height, cursor position where needed, key handling, and completion/cancellation outcome.

## Explicit Non-Deferred Scope

The following must be complete before the plan is considered implemented:

1. `tui-rata` uses the custom bottom-anchored terminal or an equivalent implementation; it must not rely on the scaffold `Viewport::Inline` path as the production runtime.
2. `RataApp` no longer owns direct fields for composer, permission dialog, model picker, completion popup, verbose state, vim state, streaming state, status text, and message vector. Those move behind `ChatWidget`, `Transcript`, and `BottomPane`.
3. `message.rs` no longer has a wildcard fallback that silently hides visible message variants.
4. All `RenderedMessage` variants have explicit render tests, including negative tests for intentionally non-visible states if any exist.
5. `/help`, `/doctor`, `/mcp`, `/hooks`, `/agents`, `/model`, `/vim`, `/clear`, `/exit`, `/image`, and currently advertised slash commands route through the new command/view system. No currently advertised command may be left as a placeholder.
6. Permission prompts use a bottom-pane view with proper keyboard ownership and response delivery.
7. Model selection uses a bottom-pane view with keyboard ownership and switch callback delivery.
8. Completion uses bottom-pane ownership and does not require `ChatWidget` or `App` to mutate completion structs directly.
9. Image messages preserve text fallback and real terminal image rendering when `source_path` is available.
10. Native scrollback commit behavior, active streaming live-tail behavior, resize reflow, and cursor placement are covered by automated tests.
11. CLI cutover is planned and testable in the same implementation branch; no "after migration" placeholder remains for existing LingXi TUI behavior.

## Explicit Non-Requirements

These are not deferred implementation. They are excluded because they are Codex product features, not LingXi UI structure requirements:

- Importing Codex `AppServerSession`, `ThreadId`, `codex_protocol`, ChatGPT account state, Codex backend auth state, Codex plugin capability summaries, Codex skills metadata, Codex rate-limit credit nudges, or Codex terminal title machinery.
- Exact byte-for-byte parity with Codex strings where LingXi has an existing product name or command set.
- Replacing LingXi's orchestrator, `TurnEvent`, `PermissionExchange`, `SessionInfo`, or `RenderedMessage` data model with Codex protocol types.

## Acceptance Criteria

1. `cargo tree -p tui-rata -i iocraft` returns no dependency path or exits with no reverse dependency.
2. `cargo tree -p tui-rata -i crossterm` shows exactly the workspace crossterm version used by `tui-rata`.
3. `lingxi-code/tui-rata/src/lib.rs` no longer exposes `RataTerminal = ratatui::Terminal<CrosstermBackend<Stdout>>` as the production terminal if the custom terminal remains the chosen implementation.
4. `RataApp` contains only event-loop/runtime state: terminal height, `ChatWidget`, channel/callback plumbing, redraw timing, and terminal restore handling.
5. `ChatWidget` has explicit fields for `Transcript` and `BottomPane`.
6. `ChatWidget::desired_height(width)`, `ChatWidget::render(area, buf)`, `ChatWidget::cursor_pos(area)`, and `ChatWidget::cursor_style(area)` exist and are used by `App`.
7. `BottomPane` owns a `view_stack: Vec<Box<dyn BottomPaneView>>` or equivalent.
8. `BottomPane` owns composer state; `App` and `ChatWidget` do not directly modify `Composer` internals.
9. `BottomPaneView` has a uniform completion/cancellation outcome model used by permission, picker, and screen views.
10. `HistoryCell` exists and every visible `RenderedMessage` variant maps to a concrete cell or an explicitly named cell adapter.
11. No `match RenderedMessage` in `tui-rata` ends with `_ => Vec::new()` for visible variants.
12. Streaming assistant text remains visible while the turn is active and is committed once finalized.
13. Tool activity updates the running status without forcing premature transcript commits.
14. Ctrl-C/Esc routing is layered: active view first, composer/history-search second, chat-widget interrupt/quit policy last.
15. Permission responses are sent exactly once and pending permissions serialize correctly when multiple requests arrive.
16. `/model` selection preserves `(request_model, profile)` callback behavior.
17. `/clear` clears committed transcript state and native scrollback commit counters coherently.
18. `/help`, `/doctor`, `/mcp`, `/hooks`, and `/agents` open focused views rather than dumping screen text into scrollback.
19. `SessionInfo` remains the read model for startup screen data until richer core data exists.
20. The composer still supports multiline input, history recall, CJK cursor width, paste handling, image-path paste, and vim mode.
21. Command and file completion are rendered above or within the bottom pane without requiring app-level overlay fields.
22. Native scrollback insertions do not overlap the bottom pane after viewport height changes.
23. Resize reflow keeps the bottom pane pinned and recomputes cursor position.
24. Tests cover 80x24, 120x40, and narrow-height layouts.
25. Tests cover permission view, model picker, completion popup, help screen, and active streaming.
26. All tests pass with `cargo test -p tui-rata`.
27. `cargo test -p tui-core` still passes.
28. `cargo clippy -p tui-rata --all-targets` passes without adding `allow` suppressions for new code.
29. `cargo fmt --check` passes.
30. `cargo build -p cli --bin lingxi-cli` passes after cutover wiring.

## Implementation Steps

### Phase 0: Lock behavior before moving code

1. Add current-behavior tests around `RataApp` submit, slash handling, permission prompt, model picker, completion popup, Ctrl-C, paste, and active streaming before extraction.
2. Add render tests for current `message::render_message` output for representative variants.
3. Add a failing coverage test that enumerates every `RenderedMessage` variant with fixture values and asserts it produces either visible output or a named non-visible reason. This should fail initially if the wildcard fallback hides variants.
4. Add layout tests using ratatui buffers or the custom terminal test backend to lock the current bottom viewport shape before replacing it.
5. Record the exact current verification baseline in the implementation PR notes. Do not proceed to structural edits until baseline tests compile and either pass or have documented known failures that the next phase resolves.

### Phase 1: Make the terminal substrate match the target

1. Replace the production `RataTerminal` alias in `lib.rs` with the custom terminal type from `custom_terminal.rs`, or move `custom_terminal.rs` to `terminal.rs` and expose it as the only runtime terminal boundary.
2. Delete or demote `setup_terminal`, `restore_terminal`, and `resize_inline_viewport` code that uses `ratatui::TerminalOptions { viewport: Viewport::Inline(...) }`.
3. Add a `TerminalSession` guard that enables raw mode and bracketed paste on construction and disables bracketed paste/raw mode on drop.
4. Add `set_bottom_viewport_height(height)` that computes an absolute bottom viewport rect from terminal size and cursor anchor.
5. Add `insert_history(height, render_fn)` or keep `insert_before` semantics, but make the public API name match the custom terminal's behavior.
6. Add tests for:
   - absolute viewport rect equals frame area,
   - viewport height change does not panic,
   - cursor is hidden when no cursor position is set,
   - cursor style resets on drop,
   - native scrollback insertions are above the viewport,
   - OSC escape width handling does not shift diff output.

### Phase 2: Introduce a shared renderable contract

1. Add `renderable.rs` with a local `Renderable` trait:
   - `fn render(&self, area: Rect, buf: &mut Buffer)`,
   - `fn desired_height(&self, width: u16) -> u16`,
   - `fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)>`,
   - `fn cursor_style(&self, area: Rect) -> crossterm::cursor::SetCursorStyle`.
2. Implement helpers for vertical stacking and optional cursor delegation.
3. Update composer, completion, picker, dialog, and screens to render through `Buffer`/`Rect` where possible instead of requiring `ratatui::Frame`.
4. Keep temporary `Frame` adapters only at terminal draw boundaries. Inner widgets should not need full-frame ownership.

### Phase 3: Introduce `HistoryCell` and `Transcript`

1. Add `history_cell/mod.rs` with the local trait modeled on Codex but using LingXi types:
   - `display_lines(width, theme, mode)`,
   - `raw_lines()`,
   - `desired_height(width, mode)`,
   - `is_visible(width)`,
   - optional `animation_key(now)` for active cells.
2. Add `Transcript`:
   - `committed: Vec<Box<dyn HistoryCell>>`,
   - `active: Option<Box<dyn HistoryCell>>`,
   - `committed_to_terminal: usize`,
   - `render_mode`,
   - `verbose/expanded state`.
3. Add methods:
   - `push_committed(cell)`,
   - `set_active(cell)`,
   - `mutate_active(f)`,
   - `flush_active()`,
   - `flush_to_native_scrollback(terminal, width)`,
   - `visible_live_tail(width)`,
   - `clear()`.
4. Migrate `RataApp.messages` and `RataApp.committed` into `Transcript`.
5. Add `MessageHistoryCell` as the first adapter over `RenderedMessage` to preserve existing render behavior while the cell split proceeds.
6. Add per-category cells for all message variants:
   - user and assistant text,
   - system rich/text/error/rate-limit,
   - tool use/result/bash/local command/grouped/collapsed read-search,
   - thinking/redacted thinking/advisor/plan approval/plan,
   - task assignment/agent notification/channel/teammate/shutdown,
   - image/attachment/resource/memory/compact boundary.
7. Delete the wildcard `RenderedMessage` fallback after every variant is covered.
8. Add tests for every variant and for grouped/collapsed verbose toggles.

### Phase 4: Introduce `BottomPaneView`

1. Add `bottom_pane/view.rs`:
   - `trait BottomPaneView: Renderable`,
   - `fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome`,
   - `fn handle_paste(&mut self, text: &str) -> ViewOutcome`,
   - `fn wants_status_line(&self) -> bool`,
   - `fn dismiss_after_child_accept(&self) -> bool`.
2. Add `ViewOutcome`:
   - `Pending`,
   - `Cancelled`,
   - `Accepted(ViewAction)`,
   - `SubmitPrompt(String)`,
   - `SwitchModel { request_model, profile }`,
   - `PermissionResponse(PermissionResponse)`,
   - `OpenView(Box<dyn BottomPaneView>)`,
   - `RunCommand(CommandAction)`.
3. Port `overlay::Dialog` into `PermissionView` and generic `DialogView`.
4. Port `ModelPicker` into `ModelPickerView`.
5. Port `CompletionPopup` into `CompletionView`, but keep completion state owned by `BottomPane`.
6. Port read-only `FullScreen` into `ScreenView` and ensure it is actually pushed to `view_stack`.
7. Add tests for view-stack push/pop, child-accept dismissal, cancellation, and key ownership.

### Phase 5: Introduce `BottomPane`

1. Add `bottom_pane/mod.rs`.
2. Move these fields out of `RataApp`:
   - `composer`,
   - `completion`,
   - `vim`,
   - `ctrl_c_at` if the visual hint is pane-local,
   - status hint text,
   - active view stack.
3. Keep process-level `current_turn`, `turn_started_at`, and `activity` in `ChatWidget`; expose a `BottomPaneStatus` input into `BottomPane`.
4. Implement:
   - `handle_key(key) -> BottomPaneOutcome`,
   - `handle_paste(text) -> BottomPaneOutcome`,
   - `show_view(view)`,
   - `show_permission(exchange)`,
   - `show_model_picker(rows)`,
   - `set_task_running(status)`,
   - `set_verbose(enabled)`,
   - `composer_is_empty()`,
   - `take_submission_state()`.
5. Implement render and height through `Renderable`.
6. Preserve composer behavior:
   - Enter submits,
   - modified Enter inserts newline,
   - Ctrl-A/E/W/U and word movement,
   - Up/Down history,
   - paste insertion,
   - image path paste,
   - vim normal/insert/visual mode.
7. Add `PendingInputPreview` if queued inputs exist in the current data path; if no queue exists yet, add the component with tests for empty state and wire it once the queue source appears in `ChatWidget`.
8. Add tests for CJK cursor width, multiline scrolling, history recall, completion replacement, `@file` completion, slash completion, and vim routing.

### Phase 6: Introduce `ChatWidget`

1. Add `chat_widget.rs`.
2. Move these fields out of `RataApp`:
   - `theme`,
   - `current_turn`,
   - `turn_started_at`,
   - `activity`,
   - `session`,
   - `verbose`,
   - pending permission queue state.
3. Add fields:
   - `transcript: Transcript`,
   - `bottom_pane: BottomPane`,
   - `session: SessionInfo`,
   - `theme: Theme`,
   - `current_turn: Option<CancellationToken>`,
   - `turn_started_at: Option<Instant>`,
   - `activity: Option<String>`,
   - `pending_permissions: VecDeque<PermissionExchange>`.
4. Implement `ChatWidget::new(messages, session)`.
5. Implement `apply_turn_event`:
   - `TurnStarted` creates or resets active assistant cell and sets task running,
   - `TextDelta` mutates active assistant cell,
   - `ToolUseStart` updates activity and may create/update tool active cell,
   - `ToolUseResult` records output and clears relevant activity,
   - `TurnEnded` flushes active cell, clears current turn, clears task running.
6. Implement `open_permission(exchange)` by pushing a `PermissionView`, not by storing `PendingPermission` in `App`.
7. Implement `handle_key(key) -> ChatOutcome`.
8. Implement `handle_paste(text) -> ChatOutcome`.
9. Implement `handle_slash(input) -> ChatOutcome` using a command registry instead of a hard-coded `match` in `RataApp`.
10. Implement `desired_height(width)`, `render(area, buf)`, `cursor_pos(area)`, and `cursor_style(area)`.
11. Implement `flush_scrollback(terminal)` by delegating to `Transcript`.
12. Add tests for event lifecycle, active streaming tail, turn cancellation, slash command routing, permission serialization, model switch callback, and clear/reset.

### Phase 7: Reduce `RataApp` to runtime orchestration

1. Replace `RataApp` state with:
   - `chat_widget`,
   - terminal height/cache,
   - redraw cadence,
   - event receivers,
   - callback closures or a small `AppCallbacks` struct.
2. Move `KeyOutcome` into `ChatOutcome` or `AppOutcome` with clear boundary meanings.
3. Move `PendingPermission` out of `app.rs`.
4. Delete `render_viewport`, `spinner_text`, `sync_completion`, `on_completion_key`, `on_picker_key`, `on_permission_key`, and `submit_composer` from `app.rs` after equivalent ownership exists.
5. Make the event loop:
   - drain turn events into `chat_widget`,
   - drain permission requests into `chat_widget`,
   - flush transcript to terminal,
   - compute desired height through `chat_widget`,
   - set terminal viewport,
   - draw `chat_widget`,
   - route input to `chat_widget`,
   - execute returned callbacks.
6. Add a terminal restore test or panic-safety smoke where feasible.

### Phase 8: Complete command and screen routing

1. Add `command.rs` with a single command registry. The registry must drive:
   - slash completion list,
   - help command list,
   - slash dispatch,
   - tests that registry and UI stay in sync.
2. Implement commands currently advertised by `palette.rs` and `screens.rs`.
3. Convert `/help`, `/doctor`, `/mcp`, `/hooks`, `/agents`, `/skills`, `/stats`, `/memory`, `/theme`, `/config`, `/status`, `/tasks`, `/export`, `/copy`, `/color`, and `/model` into `BottomPaneView` or full-frame views based on current `SessionInfo` and available core data.
4. If a command cannot perform a side effect because no core API exists, it must still provide a complete, non-placeholder UX: clear error/info message, no panic, and a test. Do not advertise commands that only show "not implemented".
5. Ensure `/clear`, `/exit`, `/quit`, `/vim`, and `/image` continue to perform their current behavior.
6. Add tests proving every registry command has completion metadata, help metadata, and a dispatch path.

### Phase 9: Complete message rendering

1. Split `message.rs` into history-cell modules.
2. For every `RenderedMessage` variant, create fixtures and expected line tests.
3. Preserve markdown rendering through `tui_core::render::markdown`.
4. Preserve ANSI rendering for bash/local output.
5. Preserve diff rendering for edit/write outputs when `old_string`, `new_string`, and `file_path` are present.
6. Implement verbose/expanded state per tool/group/thinking/advisor/plan variants.
7. Implement image text fallback and call `image_view`/`term_image` for real image rendering when source path exists and terminal capability supports it.
8. Replace any "iocraft renderer responsibility" comments with ratatui-owned implementations.

### Phase 10: Complete permission and approval UX

1. Permission prompt view must support all current `PermissionRequest` variants:
   - `ToolUseConfirm`,
   - `ExitPlanMode`,
   - `BypassPermissionsMode`.
2. The view must support:
   - arrow navigation,
   - number shortcut selection,
   - Enter confirm,
   - Esc deny/cancel,
   - clear response semantics,
   - one-shot response send.
3. Add tests for every request variant and every response branch.
4. Ensure pending permissions queue if one is open and another arrives.

### Phase 11: Complete model picker UX

1. `ModelPickerView` must consume `SessionInfo.models`.
2. It must start on the current model.
3. It must expose selected `(request_model, profile)`.
4. It must scroll for long model lists.
5. It must handle empty model lists with a non-placeholder user-facing message.
6. It must be covered by wide/narrow layout tests and key behavior tests.

### Phase 12: Complete completion and file mention UX

1. Command completion must source from the command registry.
2. File completion must remain in `files.rs` or move behind a `CompletionSource` trait, but `BottomPane` must own when it opens/closes.
3. Add tests for:
   - bare `/`,
   - prefix filtering,
   - unknown command,
   - `@file` fragment,
   - replacement in place,
   - empty results,
   - keyboard ownership when a view is stacked above completion.
4. Decide and test clamp vs wrap navigation as a product behavior. Do not leave it implicit.

### Phase 13: Complete layout and resize behavior

1. Add buffer tests for:
   - idle composer only,
   - running status + composer,
   - completion popup,
   - permission modal,
   - model picker,
   - full screen view,
   - active streaming live tail,
   - long finalized transcript flushed to native scrollback.
2. Test viewport heights:
   - minimum terminal height,
   - normal 80x24,
   - wide 120x40,
   - narrow width.
3. Test text does not overlap within bottom pane.
4. Test cursor stays within composer rect with CJK/wide characters.
5. Test viewport height changes when composer grows and shrinks.

### Phase 14: Cutover readiness

1. Ensure `tui-rata` public API supports the same embedding path that `apps/cli`, `apps/engine-desktop`, and `test-harness` need.
2. Update dependents only after all `tui-rata` tests pass.
3. Run CLI build and relevant parity tests.
4. Keep old `tui` crate untouched until `tui-rata` is green, then perform the dependency switch as a separate, reviewable commit.
5. After dependency switch, remove old iocraft-only code only when no dependent still imports it.

## Test Plan

### Unit tests

- `composer.rs`: cursor, history, multiline, wide chars, selection, vim interactions.
- `bottom_pane`: key routing, view stack, status rendering, desired height, cursor delegation.
- `chat_widget`: turn lifecycle, slash commands, permission queue, model picker outcomes, clear behavior.
- `transcript`: commit order, active cell mutation, flush active, clear, verbose expansion, native-scroll commit cursor.
- `history_cell`: every `RenderedMessage` variant.
- `terminal`: viewport rect, cursor placement, resize, diff flush, native scrollback insert.
- `command`: registry completeness and metadata consistency.

### Integration tests

- `run_app` with synthetic `TurnEvent` stream.
- Submit prompt -> user cell -> active assistant cell -> final commit.
- Permission arrives during active turn and owns keyboard until resolved.
- Model picker selection returns callback without mutating transcript incorrectly.
- `/help` and `/doctor` open views and close back to composer.
- `/clear` resets transcript and bottom pane state.
- Paste image path creates image message; paste normal text inserts into composer.

### Layout tests

- 80x24 idle.
- 80x24 running turn.
- 80x24 permission prompt.
- 80x24 model picker with >12 rows.
- 120x40 long markdown response.
- 40x12 narrow composer and completion.
- Very small terminal with graceful clipping.

### Manual QA

1. Start `lingxi-cli` in TUI mode.
2. Submit a prompt and watch streaming text.
3. Interrupt with Ctrl-C.
4. Verify Esc behavior when idle, when composer has text, when view is open, and during running turn.
5. Run `/help`, `/doctor`, `/mcp`, `/hooks`, `/agents`, `/model`, `/vim`, `/clear`.
6. Paste multiline text.
7. Paste an image path.
8. Trigger a permission prompt and test allow once, allow always, deny, Esc.
9. Resize terminal during streaming and during an open picker.
10. Scroll native terminal history and verify bottom pane remains pinned.

## Verification Commands

Run these from `lingxi-code/`:

```bash
cargo fmt --check
cargo test -p tui-core
cargo test -p tui-rata
cargo clippy -p tui-rata --all-targets
cargo tree -p tui-rata -i crossterm
cargo tree -p tui-rata -i iocraft
cargo build -p cli --bin lingxi-cli
cargo test -p test-harness parity_tui
```

If `cargo tree -p tui-rata -i iocraft` exits non-zero because no reverse dependency exists, record that as success.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Structural refactor regresses current key behavior | Add Phase 0 regression tests before moving fields. |
| Custom terminal behavior differs from ratatui inline viewport | Make terminal switch its own phase with focused tests before UI refactor. |
| `HistoryCell` split loses message formatting parity | Use fixture tests for every `RenderedMessage` variant before deleting the old renderer. |
| View stack steals keys incorrectly | Test active view, completion, composer, and chat-widget routing separately. |
| Permission response is sent twice or dropped | Wrap response sender in a state that consumes itself exactly once; test all branches. |
| Slash command metadata drifts from completion/help/dispatch | Use one registry and add a completeness test. |
| Native scrollback and active live tail double-render active text | Keep committed and active cells separate; test active turn before and after `TurnEnded`. |
| Small terminal layouts overlap | Add narrow and short terminal buffer tests. |
| New dependencies creep in | Use existing ratatui/crossterm/unicode-width/tokio stack; do not add dependencies unless separately approved. |
| Codex code copied too broadly imports product dependencies | Port patterns and small self-contained algorithms only; reject imports of Codex app-server/protocol/auth/product modules. |

## Implementation Commit Sequence

1. **Lock current `tui-rata` behavior with tests.**
2. **Switch runtime to custom terminal substrate.**
3. **Add renderable contract.**
4. **Add transcript/history-cell abstraction with adapter preserving current rendering.**
5. **Move bottom-pane ownership out of `RataApp`.**
6. **Add `BottomPaneView` stack and port permission/model/completion/screens.**
7. **Add `ChatWidget` and move turn/slash/session state into it.**
8. **Reduce `RataApp` to event-loop orchestration.**
9. **Complete all `RenderedMessage` cells and delete wildcard hidden fallback.**
10. **Complete command registry and screen routing.**
11. **Complete layout/resize/native-scroll tests.**
12. **Wire CLI/test-harness dependents to `tui-rata`.**
13. **Run full verification.**
14. **Only after green verification, remove obsolete iocraft paths in a separate cutover commit.**

## Definition of Done

The work is done only when:

- All acceptance criteria pass.
- All verification commands pass or have a documented external reason unrelated to the change.
- No current `tui-rata` behavior is implemented by a scaffold-only comment.
- No existing `RenderedMessage` variant is silently hidden.
- No advertised slash command opens a placeholder.
- `RataApp` is no longer a UI monolith.
- `tui-rata` is structurally aligned with Codex TUI while keeping LingXi protocol/data ownership.
- The final implementation report lists changed files, simplifications made, deleted/deprecated scaffolds, and remaining risks. Remaining risks must be operational or compatibility risks, not deferred implementation.
