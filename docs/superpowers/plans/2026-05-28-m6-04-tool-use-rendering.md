# M6-04 Tool Use Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render assistant `tool_use` blocks and the user `tool_result` blocks that follow them in the iocraft TUI, with collapse/expand keybindings, line/byte-bounded truncation for noisy outputs, and a minimal ANSI SGR parser so Bash output keeps its colors. After M6-04 a real `lingxi-cli` session that calls `Read` or `Bash` shows the call header, the JSON input, the output (truncated when huge), and the user can press `e` / `Enter` on the focused block to toggle the full body.

**Architecture:** Two new iocraft components live under `crates/tui/src/components/messages/` — `assistant_tool_use.rs` (header line with `●` marker, single-line JSON preview when collapsed, pretty-printed multi-line JSON when expanded) and `user_tool_result.rs` (`└ ` marker, dim-colored, 100-line / 4000-byte truncation budget with a footer when the budget bites). Bash output runs through a new sibling module `crates/tui/src/ansi.rs` — a deliberately tiny SGR parser that handles `reset` + `bold` + 8/16-color foreground + 8/16-color background and **skips every other CSI/OSC/control sequence without panicking**. A new dispatcher in `crates/tui/src/components/messages/mod.rs` routes each `ScrollbackEntry::AssistantToolCall { … }` and `ScrollbackEntry::UserToolResult { … }` to the right component. `AppState` (defined in M6-02 `crates/tui/src/app.rs`) grows two fields — `expanded: HashMap<ToolUseId, bool>` and `focused_tool_id: Option<ToolUseId>` — plus a key handler in `crates/tui/src/events/keymap.rs` that maps Up/Down (scrollback mode) to focus walking and `e` / `Enter` (when a tool is focused) to expanded-flag toggle. The orchestrator already emits `OutputEvent::ToolCall { tool, input }` and `OutputEvent::ToolResult { tool, result }` from M5-04; M6-04 plugs those events into `AppState::push_scrollback` via the existing orchestrator-bridge channel from M6-01.

**Tech stack:** Rust 2021, `iocraft = "=0.6"` (pinned from M6-01), `serde_json` (existing workspace dep, used for `to_string_pretty` + single-line preview), `lingxi-protocol::ToolUseId` (existing newtype from `lingxi_protocol::ContentBlock`), `lingxi-traits::OutputEvent::{ToolCall, ToolResult}` (M5-04 — these are the locked variant names; the M6 spec's "ToolUseStart / ToolUseResult" labels in §3 alias these), `unicode-width = "0.1"` (already in tui Cargo.toml from M6-02 for prompt input), `insta` (existing dev-dep for snapshot tests).

**Locked Types and Naming:**
- `OutputEvent::ToolCall { tool: String, input: serde_json::Value }` — orchestrator emits this just before dispatch (M5-04 §1701 of streaming_loop tests).
- `OutputEvent::ToolResult { tool: String, result: serde_json::Value }` — orchestrator emits this just after dispatch completes.
- `lingxi_protocol::ToolUseId` — newtype wrapper around the model-supplied `toolu_…` id; created in M2; `Display` impl prints the underlying string. **However**, `OutputEvent::ToolCall` does NOT carry the `ToolUseId` today — only `tool` (the name) and `input`. M6-04 Task 2 extends `OutputEvent::ToolCall` + `OutputEvent::ToolResult` with `id: ToolUseId` so the TUI can correlate calls↔results and key the `expanded` HashMap. This is a backward-compatible additive change (the variant is `#[non_exhaustive]`).
- Throughout this plan "ToolUseId" means `lingxi_protocol::ToolUseId`.

**Claude-code literal locks** (every literal copied byte-for-byte from `claude-code/src/components/messages/`):
- `●` (U+25CF BLACK CIRCLE) — `claude-code/src/constants/figures.ts` exports as `BLACK_CIRCLE`; used as the assistant tool-use marker. UTF-8 byte sequence: `0xE2 0x97 0x8F` (3 bytes). Lock in Task 3 step 1.
- `└ ` — left-corner glyph (U+2514) + ASCII space; the user-result indent marker. Two display columns wide. Lock in Task 5 step 1.
- `> ` — ASCII `>` + space; the focus marker prefix prepended to the focused tool block. Lock in Task 9 step 1. (Matches claude-code's `MessageSelector`'s `>` arrow.)
- `[output truncated, {N} more lines]` — exact wording from `UserToolResultMessage/UserToolSuccessMessage.tsx` collapse footer; `{N}` is the post-truncation overflow line count. Lock in Task 5 step 2.

---

## File Structure

**New files (3):**
- `crates/tui/src/components/messages/assistant_tool_use.rs` — `AssistantToolUseMessage` iocraft component + props + collapsed/expanded body builder. ~180 lines.
- `crates/tui/src/components/messages/user_tool_result.rs` — `UserToolResultMessage` iocraft component + truncation helper. ~200 lines.
- `crates/tui/src/ansi.rs` — `parse_ansi(input: &str) -> Vec<AnsiSpan>` + `AnsiSpan { style: AnsiStyle, text: String }` + `AnsiStyle { fg, bg, bold }`. ~220 lines including SGR table + tests.

**Modified files (4):**
- `crates/tui/src/components/messages/mod.rs` — extend `render_entry(entry, focused_tool_id, expanded)` to dispatch the two new entry variants. ~+40 lines.
- `crates/tui/src/app.rs` — add `expanded: HashMap<ToolUseId, bool>` + `focused_tool_id: Option<ToolUseId>` to `AppState`; handle `OutputEvent::ToolCall` / `OutputEvent::ToolResult` in `apply_output_event`; add `toggle_expanded(id)` + `focus_next_tool()` + `focus_prev_tool()` methods. ~+90 lines.
- `crates/tui/src/events/keymap.rs` — extend `match_key` to emit `KeyAction::FocusNextTool` / `FocusPrevTool` / `ToggleExpand` when in scrollback mode; route Up/Down/`e`/Enter. ~+40 lines.
- `crates/tui/src/components/messages/mod.rs` (already listed above; same file gets the `enum ScrollbackEntry` variant additions: `AssistantToolCall { id, tool, input }` and `UserToolResult { id, tool, result }`). ~+25 lines for the enum branches.

**Modified existing core crate (M5-04 surface — 1 file):**
- `lingxi-core/crates/traits/src/orchestrator.rs` — add `id: lingxi_protocol::ToolUseId` field to `OutputEvent::ToolCall` and `OutputEvent::ToolResult` variants; update `OutputStream::emit_tool_call` / `emit_tool_result` trait signatures to `(&self, id: &ToolUseId, tool: &str, …)`; bump all impls (`MockOutputStream`, `SinkAdapter` in `crates/cli/src/output_adapter.rs`, `lingxi-orchestrator::streaming_loop` call sites). ~+15 lines net; covered in Task 2.

**Test files (5):**
- `crates/tui/tests/render_assistant_tool_use.rs` — insta snapshot tests for collapsed + expanded `AssistantToolUseMessage`.
- `crates/tui/tests/render_user_tool_result.rs` — insta snapshot tests for short + truncated `UserToolResultMessage`.
- `crates/tui/tests/behavior_tool_focus.rs` — focus walking + expand/collapse keypress behavior.
- `crates/tui/tests/behavior_ansi_in_bash_result.rs` — end-to-end: feed a fake Bash result with `\x1b[31m` to the renderer; assert red span.
- `crates/tui/src/ansi.rs` — `#[cfg(test)] mod tests` with unit cases for the SGR parser (empty, single CSI, malformed CSI, unsupported CSI, mixed text).

---

## Task 1: Add `ToolUseId` to `OutputEvent` and reverse-engineer claude-code's tool-block visual

**Files:**
- Read: `claude-code/src/components/messages/AssistantToolUseMessage.tsx` (entire file, ~370 lines)
- Read: `claude-code/src/components/messages/UserToolResultMessage/UserToolResultMessage.tsx` + `UserToolSuccessMessage.tsx`
- Read: `claude-code/src/constants/figures.ts` (confirm `BLACK_CIRCLE = '●'`)
- Read: `claude-code/src/components/messages/CollapsedReadSearchContent.tsx` (collapse/expand discipline)
- Modify: `lingxi-core/crates/traits/src/orchestrator.rs:313-340` (`OutputEvent`)
- Modify: `lingxi-core/crates/traits/src/orchestrator.rs:347-362` (`OutputStream` trait)

- [ ] **Step 1: Read the four claude-code references and lock literals.**

  Open each file in `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/messages/` and extract:

  1. **Tool-use marker**: `BLACK_CIRCLE` constant from `figures.ts` — locked as `●` (U+25CF, 3-byte UTF-8 `0xE2 0x97 0x8F`).
  2. **Tool-result marker**: search `UserToolSuccessMessage.tsx` for the `└` glyph. Lock as `"└ "` (U+2514 + ASCII space, 4-byte UTF-8 `0xE2 0x94 0x94 0x20`).
  3. **Truncation footer template**: search `UserToolSuccessMessage.tsx` for the substring `truncated`. The text we lock is `[output truncated, {N} more lines]` — `{N}` is a decimal integer with no thousands separator.
  4. **Focus arrow**: claude-code uses `>` in `MessageSelector`. Lock as `"> "` (2 bytes).
  5. **Truncation limits**: `MAX_LINES_PRINTED_PER_TOOL_USE_RESULT = 100` and `MAX_CHARACTERS_PRINTED_PER_TOOL_USE_RESULT = 4000` from `claude-code/src/utils/messages.ts` (search for `MAX_LINES_PRINTED`). Both apply: whichever cap is reached first wins.

  Record the literals (with byte counts) in a comment block at the top of `crates/tui/src/components/messages/assistant_tool_use.rs` and `user_tool_result.rs` (added in Tasks 3 and 5).

- [ ] **Step 2: Write the failing test for `OutputEvent::ToolCall` carrying a `ToolUseId`.**

  Create `lingxi-core/crates/traits/src/orchestrator.rs` tests addition:

  ```rust
  #[test]
  fn output_event_tool_call_carries_tool_use_id() {
      use lingxi_protocol::ToolUseId;
      let ev = OutputEvent::ToolCall {
          id: ToolUseId::from("toolu_01ABC"),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x.rs"}),
      };
      let s = serde_json::to_string(&ev).unwrap();
      let back: OutputEvent = serde_json::from_str(&s).unwrap();
      assert_eq!(ev, back);
  }
  ```

- [ ] **Step 3: Run test to verify it fails.**

  Run: `cargo test -p lingxi-traits output_event_tool_call_carries_tool_use_id`
  Expected: FAIL — `OutputEvent::ToolCall` variant does not have an `id` field.

- [ ] **Step 4: Extend `OutputEvent::ToolCall` and `OutputEvent::ToolResult` with `id: ToolUseId`.**

  Edit `lingxi-core/crates/traits/src/orchestrator.rs` around lines 319-332:

  ```rust
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  #[non_exhaustive]
  pub enum OutputEvent {
      Text { text: String },
      ToolCall {
          id: lingxi_protocol::ToolUseId,
          tool: String,
          input: serde_json::Value,
      },
      ToolResult {
          id: lingxi_protocol::ToolUseId,
          tool: String,
          result: serde_json::Value,
      },
      EndTurn { stop_reason: String, cost: CostSnapshot },
  }
  ```

  Add `lingxi-protocol` to `lingxi-core/crates/traits/Cargo.toml` `[dependencies]` if not already there (it is — used for `ContentBlock` in the same file).

- [ ] **Step 5: Update the `OutputStream` trait signatures.**

  ```rust
  #[async_trait]
  pub trait OutputStream: Send + Sync {
      async fn emit_text(&self, text: &str);
      async fn emit_tool_call(
          &self,
          id: &lingxi_protocol::ToolUseId,
          tool: &str,
          input: &serde_json::Value,
      );
      async fn emit_tool_result(
          &self,
          id: &lingxi_protocol::ToolUseId,
          tool: &str,
          result: &serde_json::Value,
      );
      async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot);
  }
  ```

- [ ] **Step 6: Update all `OutputStream` impls and call sites.**

  Update these three files (compile errors will point you to the exact lines):

  1. `lingxi-core/crates/cli/src/output_adapter.rs:33-37` — accept the new `id` parameter and pass through to `OutputSink` (extend `OutputSink::tool_call` / `tool_result` in `crates/cli/src/output.rs` with the same `id: &ToolUseId` parameter; for the plain stdout sink ignore it; for the JSON sink emit `"tool_use_id": "<id>"` in the NDJSON line).
  2. `lingxi-core/crates/orchestrator/src/test_support.rs` — `MockOutputStream::emit_tool_call` / `emit_tool_result` accept the new parameter and record it.
  3. `lingxi-core/crates/orchestrator/src/streaming_loop.rs` and `turn_loop.rs` — at every call site that constructs `OutputEvent::ToolCall { tool, input }` or calls `output.emit_tool_call(tool, input)`, thread the `ObservedToolUse::id` through. The streaming loop already has `id` in scope (it's pushed into `turn.tool_uses` at line 84). The batched turn loop reads it from each `ContentBlock::ToolUse { id, .. }`.

- [ ] **Step 7: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-traits output_event_tool_call_carries_tool_use_id`
  Expected: PASS.

  Then run the whole workspace to confirm no regressions: `cargo test --workspace`. Three M5 streaming tests will now exercise the new `id` field via the `MockOutputStream` recording; assert they still pass.

- [ ] **Step 8: Commit.**

  ```bash
  git add lingxi-core/crates/traits/src/orchestrator.rs \
          lingxi-core/crates/cli/src/output.rs \
          lingxi-core/crates/cli/src/output_adapter.rs \
          lingxi-core/crates/orchestrator/src/test_support.rs \
          lingxi-core/crates/orchestrator/src/streaming_loop.rs \
          lingxi-core/crates/orchestrator/src/turn_loop.rs
  git commit -m "feat(traits): carry ToolUseId on OutputEvent::ToolCall/ToolResult

  Extends the M5-04 OutputEvent surface so the M6-04 TUI can correlate
  tool calls with their results and key the expanded-state HashMap by id.
  Backward-compatible additive change (variant is non_exhaustive)."
  ```

---

## Task 2: Add the `ScrollbackEntry` variants for tool blocks

**Files:**
- Modify: `crates/tui/src/components/scrollback.rs` (M6-02; the file that defines `enum ScrollbackEntry`)
- Modify: `crates/tui/src/app.rs` (where `apply_output_event` lives — added by M6-02)

- [ ] **Step 1: Write the failing test.**

  Add to `crates/tui/src/components/scrollback.rs`:

  ```rust
  #[test]
  fn scrollback_entry_carries_tool_call_and_result() {
      use lingxi_protocol::ToolUseId;
      let call = ScrollbackEntry::AssistantToolCall {
          id: ToolUseId::from("toolu_a"),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x.rs"}),
      };
      let result = ScrollbackEntry::UserToolResult {
          id: ToolUseId::from("toolu_a"),
          tool: "Read".into(),
          result: serde_json::json!({"content": "fn main() {}"}),
      };
      assert!(matches!(call, ScrollbackEntry::AssistantToolCall { .. }));
      assert!(matches!(result, ScrollbackEntry::UserToolResult { .. }));
  }
  ```

- [ ] **Step 2: Run test to verify it fails.**

  Run: `cargo test -p lingxi-tui scrollback_entry_carries_tool_call_and_result`
  Expected: FAIL — variants do not exist.

- [ ] **Step 3: Add the two `ScrollbackEntry` variants.**

  In `crates/tui/src/components/scrollback.rs`, extend the enum:

  ```rust
  #[derive(Debug, Clone)]
  pub enum ScrollbackEntry {
      UserText { text: String },                       // M6-02
      AssistantText { text: String },                  // M6-02
      AssistantToolCall {
          id: lingxi_protocol::ToolUseId,
          tool: String,
          input: serde_json::Value,
      },
      UserToolResult {
          id: lingxi_protocol::ToolUseId,
          tool: String,
          result: serde_json::Value,
      },
  }
  ```

  Add `lingxi-protocol` and `serde_json` to `crates/tui/Cargo.toml` if not yet present (both should be from M6-02).

- [ ] **Step 4: Wire `OutputEvent::ToolCall` / `ToolResult` into `apply_output_event`.**

  In `crates/tui/src/app.rs`, find `apply_output_event(&mut self, ev: OutputEvent)` (added in M6-02). Extend its match:

  ```rust
  match ev {
      OutputEvent::Text { text } => { /* M6-02 */ }
      OutputEvent::ToolCall { id, tool, input } => {
          self.scrollback.push(ScrollbackEntry::AssistantToolCall {
              id, tool, input,
          });
          self.cap_scrollback();
      }
      OutputEvent::ToolResult { id, tool, result } => {
          self.scrollback.push(ScrollbackEntry::UserToolResult {
              id, tool, result,
          });
          self.cap_scrollback();
      }
      OutputEvent::EndTurn { .. } => { /* M6-03 */ }
  }
  ```

- [ ] **Step 5: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui scrollback_entry_carries_tool_call_and_result`
  Expected: PASS.

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/tui/src/components/scrollback.rs crates/tui/src/app.rs
  git commit -m "feat(tui): scrollback variants for tool call + result"
  ```

---

## Task 3: `AssistantToolUseMessage` collapsed renderer

**Files:**
- Create: `crates/tui/src/components/messages/assistant_tool_use.rs`
- Create: `crates/tui/tests/render_assistant_tool_use.rs`

- [ ] **Step 1: Write the failing snapshot test (collapsed form).**

  Create `crates/tui/tests/render_assistant_tool_use.rs`:

  ```rust
  use insta::assert_snapshot;
  use lingxi_protocol::ToolUseId;
  use lingxi_tui::components::messages::assistant_tool_use::{
      AssistantToolUseProps, render_assistant_tool_use_to_string,
  };

  #[test]
  fn collapsed_read_with_file_path() {
      let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
          id: ToolUseId::from("toolu_01"),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x.rs"}),
          expanded: false,
          focused: false,
      });
      assert_snapshot!(s, @"● Read({\"file_path\": \"/tmp/x.rs\"})");
  }
  ```

- [ ] **Step 2: Run test to verify it fails.**

  Run: `cargo test -p lingxi-tui --test render_assistant_tool_use collapsed_read_with_file_path`
  Expected: FAIL — module / function do not exist.

- [ ] **Step 3: Implement `assistant_tool_use.rs` (collapsed branch only).**

  Create `crates/tui/src/components/messages/assistant_tool_use.rs`:

  ```rust
  //! AssistantToolUseMessage — header line `● ToolName(input_preview)`.
  //!
  //! Literal locks (byte-for-byte from claude-code):
  //!   - marker: `●` (U+25CF, 3-byte UTF-8 0xE2 0x97 0x8F)
  //!     source: claude-code/src/constants/figures.ts BLACK_CIRCLE
  //!   - focus prefix: `> ` (ASCII, 2 bytes)
  //!     source: claude-code/src/components/MessageSelector.tsx

  use iocraft::prelude::*;
  use lingxi_protocol::ToolUseId;

  /// Marker glyph. 3-byte UTF-8.
  pub const MARKER: &str = "●";
  /// Focus prefix prepended when this block is the focused one.
  pub const FOCUS_PREFIX: &str = "> ";

  #[derive(Debug, Clone, Default, Props)]
  pub struct AssistantToolUseProps {
      pub id: ToolUseId,
      pub tool: String,
      pub input: serde_json::Value,
      pub expanded: bool,
      pub focused: bool,
  }

  /// Pure-string renderer used by snapshot tests and the iocraft component
  /// alike. Returns the visible text (no ANSI styling — that's added by
  /// the iocraft `Text` element's `color` prop).
  #[must_use]
  pub fn render_assistant_tool_use_to_string(props: AssistantToolUseProps) -> String {
      let prefix = if props.focused { FOCUS_PREFIX } else { "" };
      let preview = single_line_json_preview(&props.input);
      let header = format!("{prefix}{MARKER} {tool}({preview})", tool = props.tool);
      if !props.expanded {
          return header;
      }
      let pretty = serde_json::to_string_pretty(&props.input)
          .unwrap_or_else(|_| props.input.to_string());
      format!("{header}\n{pretty}")
  }

  /// Single-line JSON preview. Renders the input as compact JSON
  /// (`to_string`, no pretty-printing), with newlines stripped. No
  /// truncation here — claude-code wraps via Ink `<Text wrap="truncate-end">`;
  /// our equivalent is the iocraft `Text` element's wrap mode set by
  /// the caller.
  fn single_line_json_preview(input: &serde_json::Value) -> String {
      let s = input.to_string(); // compact form: {"k":"v"}
      // Restore one space after each `:` and `,` for readability, mirroring
      // claude-code's `JSON.stringify(input, null, 0)` formatting hint.
      let mut out = String::with_capacity(s.len() + 16);
      let mut in_string = false;
      let mut prev = '\0';
      for ch in s.chars() {
          if ch == '"' && prev != '\\' {
              in_string = !in_string;
          }
          out.push(ch);
          if !in_string && (ch == ':' || ch == ',') {
              out.push(' ');
          }
          prev = ch;
      }
      out
  }

  /// iocraft component — wraps `render_assistant_tool_use_to_string` in
  /// `Text` elements with the cyan color (assistant theme).
  #[component]
  pub fn AssistantToolUseMessage(props: &AssistantToolUseProps) -> impl Into<AnyElement<'static>> {
      let body = render_assistant_tool_use_to_string(props.clone());
      element! {
          Box(flex_direction: FlexDirection::Column) {
              Text(content: body, color: Color::Cyan)
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn single_line_preview_has_space_after_colon_and_comma() {
          let v = serde_json::json!({"a": 1, "b": "x"});
          let s = single_line_json_preview(&v);
          assert_eq!(s, r#"{"a": 1, "b": "x"}"#);
      }

      #[test]
      fn marker_is_three_utf8_bytes() {
          assert_eq!(MARKER.as_bytes(), &[0xE2, 0x97, 0x8F]);
      }
  }
  ```

  Wire the module from `crates/tui/src/components/messages/mod.rs`:

  ```rust
  pub mod assistant_tool_use;
  ```

- [ ] **Step 4: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui --test render_assistant_tool_use collapsed_read_with_file_path`
  Expected: PASS. If insta complains the snapshot is missing, run `cargo insta review` to accept the inline snapshot in the test source.

- [ ] **Step 5: Run unit tests.**

  Run: `cargo test -p lingxi-tui assistant_tool_use::tests`
  Expected: PASS — both `single_line_preview_has_space_after_colon_and_comma` and `marker_is_three_utf8_bytes` pass.

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/tui/src/components/messages/assistant_tool_use.rs \
          crates/tui/src/components/messages/mod.rs \
          crates/tui/tests/render_assistant_tool_use.rs
  git commit -m "feat(tui): AssistantToolUseMessage collapsed renderer"
  ```

---

## Task 4: `AssistantToolUseMessage` expanded body (pretty-printed JSON)

**Files:**
- Modify: `crates/tui/tests/render_assistant_tool_use.rs` (add expanded test)
- Modify: `crates/tui/src/components/messages/assistant_tool_use.rs` (no impl change — it's already in Task 3's body; this task verifies)

- [ ] **Step 1: Write the failing snapshot test (expanded form).**

  Append to `crates/tui/tests/render_assistant_tool_use.rs`:

  ```rust
  #[test]
  fn expanded_read_shows_pretty_json() {
      let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
          id: ToolUseId::from("toolu_01"),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x.rs", "limit": 100}),
          expanded: true,
          focused: false,
      });
      assert_snapshot!(s, @r#"
      ● Read({"file_path": "/tmp/x.rs", "limit": 100})
      {
        "file_path": "/tmp/x.rs",
        "limit": 100
      }
      "#);
  }
  ```

- [ ] **Step 2: Run test to verify it passes (Task 3's impl already supports expanded).**

  Run: `cargo test -p lingxi-tui --test render_assistant_tool_use expanded_read_shows_pretty_json`
  Expected: PASS. Run `cargo insta review` if the snapshot needs initial acceptance.

- [ ] **Step 3: Add a focused-collapsed test.**

  Append:

  ```rust
  #[test]
  fn focused_collapsed_has_arrow_prefix() {
      let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
          id: ToolUseId::from("toolu_01"),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x.rs"}),
          expanded: false,
          focused: true,
      });
      assert_snapshot!(s, @"> ● Read({\"file_path\": \"/tmp/x.rs\"})");
  }
  ```

- [ ] **Step 4: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui --test render_assistant_tool_use`
  Expected: 3 tests PASS.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/tests/render_assistant_tool_use.rs
  git commit -m "test(tui): expanded and focused-collapsed snapshots for AssistantToolUseMessage"
  ```

---

## Task 5: `UserToolResultMessage` — short body (full render, no truncation)

**Files:**
- Create: `crates/tui/src/components/messages/user_tool_result.rs`
- Create: `crates/tui/tests/render_user_tool_result.rs`

- [ ] **Step 1: Write the failing snapshot test.**

  Create `crates/tui/tests/render_user_tool_result.rs`:

  ```rust
  use insta::assert_snapshot;
  use lingxi_protocol::ToolUseId;
  use lingxi_tui::components::messages::user_tool_result::{
      UserToolResultProps, render_user_tool_result_to_string,
  };

  #[test]
  fn short_5_line_result_renders_full() {
      let body = "line1\nline2\nline3\nline4\nline5";
      let s = render_user_tool_result_to_string(UserToolResultProps {
          id: ToolUseId::from("toolu_01"),
          tool: "Read".into(),
          result: serde_json::json!({"content": body}),
          expanded: true,
          focused: false,
      });
      assert_snapshot!(s, @r"
      └ line1
        line2
        line3
        line4
        line5
      ");
  }
  ```

- [ ] **Step 2: Run test to verify it fails.**

  Run: `cargo test -p lingxi-tui --test render_user_tool_result short_5_line_result_renders_full`
  Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement `user_tool_result.rs`.**

  Create `crates/tui/src/components/messages/user_tool_result.rs`:

  ```rust
  //! UserToolResultMessage — `└ ` indent, dim-colored, line/byte-bounded.
  //!
  //! Literal locks (byte-for-byte from claude-code):
  //!   - indent marker: `└ ` (U+2514 + ASCII space, 4-byte UTF-8 0xE2 0x94 0x94 0x20)
  //!     source: claude-code/src/components/messages/UserToolResultMessage/UserToolSuccessMessage.tsx
  //!   - truncation footer: `[output truncated, {N} more lines]`
  //!     source: claude-code/src/utils/messages.ts (search for `truncated`)
  //!   - MAX_LINES = 100, MAX_BYTES = 4000
  //!     source: claude-code/src/utils/messages.ts (MAX_LINES_PRINTED_PER_TOOL_USE_RESULT,
  //!     MAX_CHARACTERS_PRINTED_PER_TOOL_USE_RESULT)

  use iocraft::prelude::*;
  use lingxi_protocol::ToolUseId;

  pub const MARKER: &str = "└ ";
  pub const INDENT: &str = "  "; // 2 spaces, matches MARKER width
  pub const FOCUS_PREFIX: &str = "> ";
  pub const MAX_LINES: usize = 100;
  pub const MAX_BYTES: usize = 4000;

  #[derive(Debug, Clone, Default, Props)]
  pub struct UserToolResultProps {
      pub id: ToolUseId,
      pub tool: String,
      pub result: serde_json::Value,
      pub expanded: bool,
      pub focused: bool,
  }

  /// Extract the human-displayable body from a tool result JSON.
  /// Convention (locked in M5-04 turn_loop): result is one of
  ///   `{"content": "..."}` — string body (Read, Bash, Grep)
  ///   `{"content": [{"type":"text","text":"..."}]}` — block-array (some MCP tools)
  ///   any other shape — fall back to `serde_json::to_string_pretty`.
  fn body_text(result: &serde_json::Value) -> String {
      if let Some(s) = result.get("content").and_then(|c| c.as_str()) {
          return s.to_string();
      }
      if let Some(arr) = result.get("content").and_then(|c| c.as_array()) {
          let mut out = String::new();
          for block in arr {
              if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                  if !out.is_empty() { out.push('\n'); }
                  out.push_str(t);
              }
          }
          if !out.is_empty() {
              return out;
          }
      }
      serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
  }

  /// Apply both the line cap and the byte cap. Returns the truncated
  /// body plus a `truncated_lines: usize` count (0 when no truncation).
  pub fn truncate(body: &str) -> (String, usize) {
      // Byte cap first — cheaper.
      let byte_capped: &str = if body.len() > MAX_BYTES {
          // Walk back to a char boundary so we don't slice mid-codepoint.
          let mut idx = MAX_BYTES;
          while idx > 0 && !body.is_char_boundary(idx) {
              idx -= 1;
          }
          &body[..idx]
      } else {
          body
      };
      // Then line cap.
      let line_count = byte_capped.lines().count();
      if line_count <= MAX_LINES && byte_capped.len() == body.len() {
          return (body.to_string(), 0);
      }
      let lines: Vec<&str> = byte_capped.lines().take(MAX_LINES).collect();
      let kept = lines.join("\n");
      let total_lines = body.lines().count();
      let dropped = total_lines.saturating_sub(lines.len());
      (kept, dropped)
  }

  #[must_use]
  pub fn render_user_tool_result_to_string(props: UserToolResultProps) -> String {
      let prefix = if props.focused { FOCUS_PREFIX } else { "" };
      let body = body_text(&props.result);

      // Collapsed: just the 1-line summary.
      if !props.expanded {
          let first_line = body.lines().next().unwrap_or("");
          let total = body.lines().count();
          let suffix = if total > 1 {
              format!(" (+{} lines)", total - 1)
          } else {
              String::new()
          };
          return format!("{prefix}{MARKER}{first_line}{suffix}");
      }

      // Expanded: full body, line+byte capped.
      let (truncated, dropped) = truncate(&body);
      let mut out = String::new();
      for (i, line) in truncated.lines().enumerate() {
          if i == 0 {
              out.push_str(prefix);
              out.push_str(MARKER);
          } else {
              out.push('\n');
              out.push_str(INDENT);
          }
          out.push_str(line);
      }
      if dropped > 0 {
          out.push('\n');
          out.push_str(INDENT);
          out.push_str(&format!("[output truncated, {dropped} more lines]"));
      }
      out
  }

  #[component]
  pub fn UserToolResultMessage(props: &UserToolResultProps) -> impl Into<AnyElement<'static>> {
      let body = render_user_tool_result_to_string(props.clone());
      element! {
          Box(flex_direction: FlexDirection::Column) {
              Text(content: body, color: Color::DarkGrey)
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn truncate_short_body_returns_unchanged() {
          let (s, dropped) = truncate("a\nb\nc");
          assert_eq!(s, "a\nb\nc");
          assert_eq!(dropped, 0);
      }

      #[test]
      fn truncate_120_line_body_caps_at_100() {
          let body = (0..120).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
          let (s, dropped) = truncate(&body);
          assert_eq!(s.lines().count(), 100);
          assert_eq!(dropped, 20);
      }

      #[test]
      fn truncate_huge_body_respects_byte_cap() {
          let body = "x".repeat(5000); // > 4000 bytes, 1 line
          let (s, dropped) = truncate(&body);
          assert!(s.len() <= MAX_BYTES);
          assert_eq!(dropped, 0); // single line still — byte cap shrinks the line itself
      }

      #[test]
      fn body_text_extracts_string_content() {
          let v = serde_json::json!({"content": "hi"});
          assert_eq!(body_text(&v), "hi");
      }

      #[test]
      fn body_text_extracts_block_array() {
          let v = serde_json::json!({"content": [{"type":"text","text":"a"},{"type":"text","text":"b"}]});
          assert_eq!(body_text(&v), "a\nb");
      }
  }
  ```

  Add the module declaration to `crates/tui/src/components/messages/mod.rs`:

  ```rust
  pub mod user_tool_result;
  ```

- [ ] **Step 4: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui --test render_user_tool_result short_5_line_result_renders_full`
  Expected: PASS (accept the snapshot with `cargo insta review` if needed).

  Run: `cargo test -p lingxi-tui user_tool_result::tests`
  Expected: 5 unit tests PASS.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/components/messages/user_tool_result.rs \
          crates/tui/src/components/messages/mod.rs \
          crates/tui/tests/render_user_tool_result.rs
  git commit -m "feat(tui): UserToolResultMessage with line/byte truncation"
  ```

---

## Task 6: `UserToolResultMessage` — truncation footer visible

**Files:**
- Modify: `crates/tui/tests/render_user_tool_result.rs`

- [ ] **Step 1: Write the failing snapshot test for 200-line truncation.**

  Append to `crates/tui/tests/render_user_tool_result.rs`:

  ```rust
  #[test]
  fn long_200_line_result_shows_truncation_footer() {
      let body = (1..=200)
          .map(|i| format!("line{i}"))
          .collect::<Vec<_>>()
          .join("\n");
      let s = render_user_tool_result_to_string(UserToolResultProps {
          id: ToolUseId::from("toolu_01"),
          tool: "Bash".into(),
          result: serde_json::json!({"content": body}),
          expanded: true,
          focused: false,
      });
      // The first 100 lines render; line 101..=200 are dropped.
      // Footer reads `[output truncated, 100 more lines]`.
      assert!(s.starts_with("└ line1\n"));
      assert!(s.contains("line100\n"));
      assert!(!s.contains("line101\n"));
      assert!(s.ends_with("[output truncated, 100 more lines]"));
  }

  #[test]
  fn collapsed_long_result_shows_first_line_plus_lines_suffix() {
      let body = (1..=10).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
      let s = render_user_tool_result_to_string(UserToolResultProps {
          id: ToolUseId::from("toolu_01"),
          tool: "Bash".into(),
          result: serde_json::json!({"content": body}),
          expanded: false,
          focused: false,
      });
      assert_eq!(s, "└ line1 (+9 lines)");
  }
  ```

- [ ] **Step 2: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui --test render_user_tool_result`
  Expected: 3 tests PASS (including the 2 new ones — implementation already handles this from Task 5).

- [ ] **Step 3: Commit.**

  ```bash
  git add crates/tui/tests/render_user_tool_result.rs
  git commit -m "test(tui): truncation footer + collapsed summary for UserToolResultMessage"
  ```

---

## Task 7: ANSI SGR parser — happy path (8/16-color fg + reset)

**Files:**
- Create: `crates/tui/src/ansi.rs`

- [ ] **Step 1: Write the failing unit test.**

  Create `crates/tui/src/ansi.rs` with a test stub:

  ```rust
  //! Minimal ANSI SGR parser for Bash tool output.
  //!
  //! Scope (M6-04 — full parser deferred to M7):
  //!   - CSI `\x1b[Nm` where N ∈ {0 (reset), 1 (bold), 30..=37 (fg), 40..=47 (bg),
  //!     90..=97 (bright fg), 100..=107 (bright bg)}
  //!   - Multi-parameter forms `\x1b[1;31m` (bold + red) — semicolon-separated.
  //!   - Empty SGR `\x1b[m` is treated as reset (per spec).
  //!   - ALL other CSI / OSC / DCS / SOS / PM / APC sequences are SKIPPED
  //!     (text between `\x1b[`/`\x1b]` and the terminator is dropped; the
  //!     terminator itself is dropped).
  //!   - Malformed input (unterminated CSI, junk bytes) does NOT panic —
  //!     unterminated sequences are consumed up to EOF and ignored.

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
  pub enum AnsiColor {
      #[default]
      Default,
      Black,
      Red,
      Green,
      Yellow,
      Blue,
      Magenta,
      Cyan,
      White,
      BrightBlack,
      BrightRed,
      BrightGreen,
      BrightYellow,
      BrightBlue,
      BrightMagenta,
      BrightCyan,
      BrightWhite,
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
  pub struct AnsiStyle {
      pub fg: AnsiColor,
      pub bg: AnsiColor,
      pub bold: bool,
  }

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct AnsiSpan {
      pub style: AnsiStyle,
      pub text: String,
  }

  /// Parse `input` into a sequence of styled spans. Unsupported escape
  /// sequences are silently skipped. Never panics.
  #[must_use]
  pub fn parse_ansi(input: &str) -> Vec<AnsiSpan> {
      todo!()
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn red_err_then_reset() {
          let v = parse_ansi("\x1b[31mERR\x1b[0m");
          assert_eq!(v.len(), 1);
          assert_eq!(v[0].text, "ERR");
          assert_eq!(v[0].style.fg, AnsiColor::Red);
      }
  }
  ```

  Wire from `crates/tui/src/lib.rs`:

  ```rust
  pub mod ansi;
  ```

- [ ] **Step 2: Run test to verify it fails.**

  Run: `cargo test -p lingxi-tui ansi::tests::red_err_then_reset`
  Expected: FAIL — `todo!()` panic.

- [ ] **Step 3: Implement `parse_ansi`.**

  Replace the `todo!()` body in `crates/tui/src/ansi.rs`:

  ```rust
  pub fn parse_ansi(input: &str) -> Vec<AnsiSpan> {
      let mut spans: Vec<AnsiSpan> = Vec::new();
      let mut style = AnsiStyle::default();
      let mut buf = String::new();
      let bytes = input.as_bytes();
      let mut i = 0;
      while i < bytes.len() {
          let b = bytes[i];
          if b == 0x1b && i + 1 < bytes.len() {
              // Flush current buf as a span before processing the escape.
              if !buf.is_empty() {
                  spans.push(AnsiSpan { style, text: std::mem::take(&mut buf) });
              }
              let next = bytes[i + 1];
              if next == b'[' {
                  // CSI — read until a final byte in 0x40..=0x7E.
                  let mut j = i + 2;
                  let mut params = String::new();
                  while j < bytes.len() {
                      let c = bytes[j];
                      if (0x40..=0x7E).contains(&c) {
                          break;
                      }
                      params.push(c as char);
                      j += 1;
                  }
                  if j >= bytes.len() {
                      // Unterminated CSI — drop it and stop.
                      return spans;
                  }
                  let final_byte = bytes[j];
                  if final_byte == b'm' {
                      apply_sgr(&params, &mut style);
                  }
                  // Else: ignored CSI (e.g. cursor movement, mode set).
                  i = j + 1;
                  continue;
              } else if next == b']' {
                  // OSC — read until BEL (0x07) or ST (ESC \).
                  let mut j = i + 2;
                  while j < bytes.len() {
                      if bytes[j] == 0x07 {
                          j += 1;
                          break;
                      }
                      if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                          j += 2;
                          break;
                      }
                      j += 1;
                  }
                  i = j;
                  continue;
              } else {
                  // Other ESC-prefixed sequence (e.g. ESC c, ESC =). Skip 2 bytes.
                  i += 2;
                  continue;
              }
          }
          buf.push(b as char);
          i += 1;
      }
      if !buf.is_empty() {
          spans.push(AnsiSpan { style, text: buf });
      }
      spans
  }

  fn apply_sgr(params: &str, style: &mut AnsiStyle) {
      // Empty `\x1b[m` is reset.
      if params.is_empty() {
          *style = AnsiStyle::default();
          return;
      }
      for tok in params.split(';') {
          let n: u16 = tok.parse().unwrap_or(0);
          match n {
              0 => *style = AnsiStyle::default(),
              1 => style.bold = true,
              22 => style.bold = false,
              30 => style.fg = AnsiColor::Black,
              31 => style.fg = AnsiColor::Red,
              32 => style.fg = AnsiColor::Green,
              33 => style.fg = AnsiColor::Yellow,
              34 => style.fg = AnsiColor::Blue,
              35 => style.fg = AnsiColor::Magenta,
              36 => style.fg = AnsiColor::Cyan,
              37 => style.fg = AnsiColor::White,
              39 => style.fg = AnsiColor::Default,
              40 => style.bg = AnsiColor::Black,
              41 => style.bg = AnsiColor::Red,
              42 => style.bg = AnsiColor::Green,
              43 => style.bg = AnsiColor::Yellow,
              44 => style.bg = AnsiColor::Blue,
              45 => style.bg = AnsiColor::Magenta,
              46 => style.bg = AnsiColor::Cyan,
              47 => style.bg = AnsiColor::White,
              49 => style.bg = AnsiColor::Default,
              90 => style.fg = AnsiColor::BrightBlack,
              91 => style.fg = AnsiColor::BrightRed,
              92 => style.fg = AnsiColor::BrightGreen,
              93 => style.fg = AnsiColor::BrightYellow,
              94 => style.fg = AnsiColor::BrightBlue,
              95 => style.fg = AnsiColor::BrightMagenta,
              96 => style.fg = AnsiColor::BrightCyan,
              97 => style.fg = AnsiColor::BrightWhite,
              100 => style.bg = AnsiColor::BrightBlack,
              101 => style.bg = AnsiColor::BrightRed,
              102 => style.bg = AnsiColor::BrightGreen,
              103 => style.bg = AnsiColor::BrightYellow,
              104 => style.bg = AnsiColor::BrightBlue,
              105 => style.bg = AnsiColor::BrightMagenta,
              106 => style.bg = AnsiColor::BrightCyan,
              107 => style.bg = AnsiColor::BrightWhite,
              _ => { /* unsupported code — ignore */ }
          }
      }
  }
  ```

- [ ] **Step 4: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui ansi::tests::red_err_then_reset`
  Expected: PASS.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/ansi.rs crates/tui/src/lib.rs
  git commit -m "feat(tui): minimal ANSI SGR parser (8/16-color fg+bg+bold)"
  ```

---

## Task 8: ANSI parser — multi-span + bold + unsupported CSI skip + malformed input

**Files:**
- Modify: `crates/tui/src/ansi.rs` (add tests)

- [ ] **Step 1: Add the failing tests.**

  Append to the `#[cfg(test)] mod tests` block in `crates/tui/src/ansi.rs`:

  ```rust
  #[test]
  fn three_spans_red_bold_default() {
      let v = parse_ansi("\x1b[31mERR\x1b[0m\x1b[1mBOLD\x1b[0m tail");
      assert_eq!(v.len(), 3);
      assert_eq!(v[0].text, "ERR");
      assert_eq!(v[0].style.fg, AnsiColor::Red);
      assert!(!v[0].style.bold);
      assert_eq!(v[1].text, "BOLD");
      assert_eq!(v[1].style.fg, AnsiColor::Default);
      assert!(v[1].style.bold);
      assert_eq!(v[2].text, " tail");
      assert_eq!(v[2].style, AnsiStyle::default());
  }

  #[test]
  fn unsupported_csi_is_skipped_without_panic() {
      // Bracketed-paste enable — NOT an SGR.
      let v = parse_ansi("a\x1b[?2004hb");
      assert_eq!(v.len(), 1);
      assert_eq!(v[0].text, "ab");
  }

  #[test]
  fn cursor_movement_is_skipped() {
      let v = parse_ansi("x\x1b[2Jy"); // ED (erase display) — not SGR
      assert_eq!(v.len(), 1);
      assert_eq!(v[0].text, "xy");
  }

  #[test]
  fn osc_is_skipped() {
      // OSC 0; set title
      let v = parse_ansi("a\x1b]0;title\x07b");
      assert_eq!(v.len(), 1);
      assert_eq!(v[0].text, "ab");
  }

  #[test]
  fn empty_string_parses_to_empty_vec() {
      assert!(parse_ansi("").is_empty());
  }

  #[test]
  fn malformed_unterminated_csi_does_not_panic() {
      // ESC[31 with no final byte — must not panic.
      let _v = parse_ansi("\x1b[31");
  }

  #[test]
  fn malformed_lone_esc_does_not_panic() {
      let v = parse_ansi("a\x1bb");
      // Lone ESC followed by `b` is consumed as a 2-byte ESC-sequence and dropped.
      assert_eq!(v.len(), 1);
      assert_eq!(v[0].text, "a");
  }

  #[test]
  fn multi_param_sgr_bold_red() {
      let v = parse_ansi("\x1b[1;31mhi\x1b[0m");
      assert_eq!(v.len(), 1);
      assert_eq!(v[0].text, "hi");
      assert_eq!(v[0].style.fg, AnsiColor::Red);
      assert!(v[0].style.bold);
  }
  ```

- [ ] **Step 2: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui ansi::tests`
  Expected: 9 tests PASS (1 from Task 7 + 8 new). Task 7's implementation already handles each case.

- [ ] **Step 3: Commit.**

  ```bash
  git add crates/tui/src/ansi.rs
  git commit -m "test(tui): ANSI parser handles multi-span, OSC, malformed input"
  ```

---

## Task 9: Focus model in `AppState` — `focused_tool_id` field and walking helpers

**Files:**
- Modify: `crates/tui/src/app.rs`

- [ ] **Step 1: Write the failing test.**

  Add to `crates/tui/src/app.rs` `#[cfg(test)] mod tests`:

  ```rust
  #[test]
  fn focus_walks_through_tool_calls_in_order() {
      use lingxi_protocol::ToolUseId;
      let mut app = AppState::default();
      app.scrollback.push(ScrollbackEntry::UserText { text: "hi".into() });
      app.scrollback.push(ScrollbackEntry::AssistantToolCall {
          id: ToolUseId::from("toolu_a"),
          tool: "Read".into(),
          input: serde_json::json!({}),
      });
      app.scrollback.push(ScrollbackEntry::AssistantText { text: "ok".into() });
      app.scrollback.push(ScrollbackEntry::AssistantToolCall {
          id: ToolUseId::from("toolu_b"),
          tool: "Bash".into(),
          input: serde_json::json!({}),
      });
      assert_eq!(app.focused_tool_id, None);
      app.focus_next_tool();
      assert_eq!(app.focused_tool_id.as_deref(), Some("toolu_a"));
      app.focus_next_tool();
      assert_eq!(app.focused_tool_id.as_deref(), Some("toolu_b"));
      app.focus_next_tool(); // past end — stay on last
      assert_eq!(app.focused_tool_id.as_deref(), Some("toolu_b"));
      app.focus_prev_tool();
      assert_eq!(app.focused_tool_id.as_deref(), Some("toolu_a"));
      app.focus_prev_tool(); // past start — stay on first
      assert_eq!(app.focused_tool_id.as_deref(), Some("toolu_a"));
  }
  ```

- [ ] **Step 2: Run test to verify it fails.**

  Run: `cargo test -p lingxi-tui focus_walks_through_tool_calls_in_order`
  Expected: FAIL — `focused_tool_id` field + `focus_next_tool` / `focus_prev_tool` methods do not exist.

- [ ] **Step 3: Add the field + helpers.**

  In `crates/tui/src/app.rs`, extend `AppState`:

  ```rust
  use lingxi_protocol::ToolUseId;
  use std::collections::HashMap;

  pub struct AppState {
      // … existing M6-02/03 fields …
      pub focused_tool_id: Option<ToolUseId>,
      pub expanded: HashMap<ToolUseId, bool>,
  }

  impl Default for AppState {
      fn default() -> Self {
          Self {
              // … existing fields default-init …
              focused_tool_id: None,
              expanded: HashMap::new(),
          }
      }
  }

  impl AppState {
      /// All tool ids in scrollback order. Iterates `scrollback` once.
      fn tool_ids(&self) -> Vec<ToolUseId> {
          self.scrollback
              .iter()
              .filter_map(|e| match e {
                  ScrollbackEntry::AssistantToolCall { id, .. } => Some(id.clone()),
                  _ => None,
              })
              .collect()
      }

      pub fn focus_next_tool(&mut self) {
          let ids = self.tool_ids();
          if ids.is_empty() {
              return;
          }
          self.focused_tool_id = match &self.focused_tool_id {
              None => Some(ids[0].clone()),
              Some(cur) => {
                  let pos = ids.iter().position(|i| i == cur).unwrap_or(0);
                  let next = (pos + 1).min(ids.len() - 1);
                  Some(ids[next].clone())
              }
          };
      }

      pub fn focus_prev_tool(&mut self) {
          let ids = self.tool_ids();
          if ids.is_empty() {
              return;
          }
          self.focused_tool_id = match &self.focused_tool_id {
              None => Some(ids[0].clone()),
              Some(cur) => {
                  let pos = ids.iter().position(|i| i == cur).unwrap_or(0);
                  let prev = pos.saturating_sub(1);
                  Some(ids[prev].clone())
              }
          };
      }

      pub fn toggle_expanded(&mut self, id: &ToolUseId) {
          let entry = self.expanded.entry(id.clone()).or_insert(false);
          *entry = !*entry;
      }
  }
  ```

  Note: `ToolUseId` must impl `Hash + Eq + Clone` for the HashMap. Confirm in `lingxi-protocol::ToolUseId`; if not present (it's a newtype around `String` so should derive these from M2), add the missing derives in `lingxi-protocol/src/lib.rs` as part of this task.

  `Option<ToolUseId>::as_deref()` requires `ToolUseId: Deref<Target=str>`. If the protocol crate doesn't impl it, replace the test assertions with `.as_ref().map(|id| id.as_str()) == Some("toolu_a")` style.

- [ ] **Step 4: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui focus_walks_through_tool_calls_in_order`
  Expected: PASS.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/app.rs lingxi-core/crates/protocol/src/lib.rs
  git commit -m "feat(tui): AppState focus + expanded for tool blocks"
  ```

---

## Task 10: Keymap — Up/Down walks focus, `e`/Enter toggles expanded

**Files:**
- Modify: `crates/tui/src/events/keymap.rs`
- Modify: `crates/tui/src/app.rs` (extend `apply_key_action`)

- [ ] **Step 1: Write the failing behavior test.**

  Create `crates/tui/tests/behavior_tool_focus.rs`:

  ```rust
  use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
  use lingxi_protocol::ToolUseId;
  use lingxi_tui::app::{AppState, AppMode};
  use lingxi_tui::components::scrollback::ScrollbackEntry;
  use lingxi_tui::events::keymap::handle_key;

  fn seed_state_with_two_tools() -> AppState {
      let mut app = AppState::default();
      app.mode = AppMode::Scrollback; // M6-02 introduced AppMode::{Prompt, Scrollback}
      app.scrollback.push(ScrollbackEntry::AssistantToolCall {
          id: ToolUseId::from("toolu_a"),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x"}),
      });
      app.scrollback.push(ScrollbackEntry::AssistantToolCall {
          id: ToolUseId::from("toolu_b"),
          tool: "Bash".into(),
          input: serde_json::json!({"command": "ls"}),
      });
      app
  }

  #[test]
  fn down_arrow_focuses_first_tool_then_second() {
      let mut app = seed_state_with_two_tools();
      handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
      assert_eq!(app.focused_tool_id.as_ref().map(|i| i.as_str()), Some("toolu_a"));
      handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
      assert_eq!(app.focused_tool_id.as_ref().map(|i| i.as_str()), Some("toolu_b"));
  }

  #[test]
  fn e_keypress_toggles_expanded_for_focused_tool() {
      let mut app = seed_state_with_two_tools();
      handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
      assert_eq!(app.expanded.get(&ToolUseId::from("toolu_a")).copied().unwrap_or(false), false);
      handle_key(&mut app, KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
      assert_eq!(app.expanded.get(&ToolUseId::from("toolu_a")).copied(), Some(true));
      handle_key(&mut app, KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
      assert_eq!(app.expanded.get(&ToolUseId::from("toolu_a")).copied(), Some(false));
  }

  #[test]
  fn enter_keypress_toggles_expanded_for_focused_tool() {
      let mut app = seed_state_with_two_tools();
      handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
      handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
      assert_eq!(app.expanded.get(&ToolUseId::from("toolu_a")).copied(), Some(true));
  }
  ```

- [ ] **Step 2: Run tests to verify they fail.**

  Run: `cargo test -p lingxi-tui --test behavior_tool_focus`
  Expected: FAIL — Up/Down currently only scroll viewport (from M6-02), no `e` handler exists.

- [ ] **Step 3: Extend `handle_key` in `keymap.rs`.**

  In `crates/tui/src/events/keymap.rs`, find the existing `handle_key` (added in M6-02) and extend the `AppMode::Scrollback` arm:

  ```rust
  pub fn handle_key(app: &mut AppState, key: KeyEvent) {
      match app.mode {
          AppMode::Prompt => { /* M6-02: prompt editing */ }
          AppMode::Scrollback => match key.code {
              KeyCode::Down => app.focus_next_tool(),
              KeyCode::Up => app.focus_prev_tool(),
              KeyCode::Char('e') | KeyCode::Enter => {
                  if let Some(id) = app.focused_tool_id.clone() {
                      app.toggle_expanded(&id);
                  }
              }
              KeyCode::Char('j') => app.scroll_down(),  // M6-02 helper
              KeyCode::Char('k') => app.scroll_up(),    // M6-02 helper
              KeyCode::Char('g') => app.scroll_top(),   // M6-02 helper
              KeyCode::Char('G') => app.scroll_bottom(),// M6-02 helper
              KeyCode::Esc => app.mode = AppMode::Prompt,
              _ => {}
          },
      }
  }
  ```

  Note: in M6-02 Up/Down arrows were bound to history-prev/next in `AppMode::Prompt`. That binding stays. In `AppMode::Scrollback` (which the user enters via PgUp or some explicit key from M6-02), Up/Down now walks tool focus. If M6-02 wired Up/Down differently in Scrollback mode (e.g. to line scroll), this task supersedes that — line scroll moves to `j`/`k` only.

- [ ] **Step 4: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui --test behavior_tool_focus`
  Expected: 3 tests PASS.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/events/keymap.rs crates/tui/tests/behavior_tool_focus.rs
  git commit -m "feat(tui): Up/Down focuses tool blocks; e/Enter toggles expanded"
  ```

---

## Task 11: Message dispatcher routes the new variants

**Files:**
- Modify: `crates/tui/src/components/messages/mod.rs`

- [ ] **Step 1: Write the failing behavior test.**

  Append to `crates/tui/tests/behavior_tool_focus.rs`:

  ```rust
  use lingxi_tui::components::messages::render_entry_to_string;

  #[test]
  fn dispatcher_routes_tool_call_and_result() {
      let call = ScrollbackEntry::AssistantToolCall {
          id: ToolUseId::from("toolu_a"),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x"}),
      };
      let s = render_entry_to_string(&call, /*focused=*/ false, /*expanded=*/ false);
      assert!(s.contains("● Read"));
      let result = ScrollbackEntry::UserToolResult {
          id: ToolUseId::from("toolu_a"),
          tool: "Read".into(),
          result: serde_json::json!({"content": "hi"}),
      };
      let s2 = render_entry_to_string(&result, /*focused=*/ false, /*expanded=*/ false);
      assert!(s2.starts_with("└ hi"));
  }
  ```

- [ ] **Step 2: Run test to verify it fails.**

  Run: `cargo test -p lingxi-tui --test behavior_tool_focus dispatcher_routes_tool_call_and_result`
  Expected: FAIL — function not found or doesn't route the two new variants.

- [ ] **Step 3: Extend the dispatcher.**

  In `crates/tui/src/components/messages/mod.rs`:

  ```rust
  pub mod assistant_tool_use;
  pub mod user_tool_result;
  // … existing M6-02 modules: user_text, assistant_text …

  use crate::components::scrollback::ScrollbackEntry;
  use assistant_tool_use::{AssistantToolUseProps, render_assistant_tool_use_to_string};
  use user_tool_result::{UserToolResultProps, render_user_tool_result_to_string};

  /// String-form dispatcher used by snapshot tests. The iocraft-component
  /// dispatcher (returns `AnyElement`) lives next to this; both share the
  /// same routing rules.
  #[must_use]
  pub fn render_entry_to_string(
      entry: &ScrollbackEntry,
      focused: bool,
      expanded: bool,
  ) -> String {
      match entry {
          ScrollbackEntry::UserText { text } => format!("> {text}"),       // M6-02
          ScrollbackEntry::AssistantText { text } => text.clone(),         // M6-02
          ScrollbackEntry::AssistantToolCall { id, tool, input } => {
              render_assistant_tool_use_to_string(AssistantToolUseProps {
                  id: id.clone(),
                  tool: tool.clone(),
                  input: input.clone(),
                  expanded,
                  focused,
              })
          }
          ScrollbackEntry::UserToolResult { id, tool, result } => {
              render_user_tool_result_to_string(UserToolResultProps {
                  id: id.clone(),
                  tool: tool.clone(),
                  result: result.clone(),
                  expanded,
                  focused,
              })
          }
      }
  }
  ```

  Then in `crates/tui/src/components/scrollback.rs`, the iocraft `Scrollback` component (added in M6-02) iterates the `Vec<ScrollbackEntry>` and emits one child per entry. Extend its match to render the two new variants by invoking `AssistantToolUseMessage` / `UserToolResultMessage` with `expanded = app.expanded.get(id).copied().unwrap_or(false)` and `focused = app.focused_tool_id.as_ref() == Some(id)`.

- [ ] **Step 4: Run test to verify it passes.**

  Run: `cargo test -p lingxi-tui --test behavior_tool_focus dispatcher_routes_tool_call_and_result`
  Expected: PASS.

  Then run the full tui test suite: `cargo test -p lingxi-tui`. Expected: all green.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/components/messages/mod.rs crates/tui/src/components/scrollback.rs
  git commit -m "feat(tui): dispatcher routes AssistantToolCall + UserToolResult"
  ```

---

## Task 12: Bash output passes through `parse_ansi` and renders colored

**Files:**
- Modify: `crates/tui/src/components/messages/user_tool_result.rs` (route Bash bodies through parser)
- Create: `crates/tui/tests/behavior_ansi_in_bash_result.rs`

- [ ] **Step 1: Write the failing behavior test.**

  Create `crates/tui/tests/behavior_ansi_in_bash_result.rs`:

  ```rust
  use lingxi_protocol::ToolUseId;
  use lingxi_tui::ansi::{AnsiColor, parse_ansi};
  use lingxi_tui::components::messages::user_tool_result::{
      UserToolResultProps, render_user_tool_result_body_spans,
  };

  #[test]
  fn bash_result_with_red_err_yields_red_span() {
      let body = "ok\n\x1b[31mERR\x1b[0m\nrest";
      let spans = render_user_tool_result_body_spans(&UserToolResultProps {
          id: ToolUseId::from("toolu_a"),
          tool: "Bash".into(),
          result: serde_json::json!({"content": body}),
          expanded: true,
          focused: false,
      });
      // Find at least one span containing "ERR" with Red fg.
      let has_red_err = spans.iter().any(|s| s.text.contains("ERR") && s.style.fg == AnsiColor::Red);
      assert!(has_red_err, "expected a red ERR span, got {:?}", spans);
  }

  #[test]
  fn read_result_is_not_ansi_parsed() {
      // For non-Bash tools, ANSI sequences are kept as literal characters
      // (no parser applied). The TUI text element renders them verbatim;
      // the user sees the raw bytes. Matches claude-code which only runs
      // ansi-parser over Bash output.
      let body = "\x1b[31mERR\x1b[0m";
      let spans = render_user_tool_result_body_spans(&UserToolResultProps {
          id: ToolUseId::from("toolu_a"),
          tool: "Read".into(),
          result: serde_json::json!({"content": body}),
          expanded: true,
          focused: false,
      });
      // One span, raw literal text.
      assert_eq!(spans.len(), 1);
      assert!(spans[0].text.contains("\x1b[31m"));
      assert_eq!(spans[0].style.fg, AnsiColor::Default);
  }
  ```

- [ ] **Step 2: Run tests to verify they fail.**

  Run: `cargo test -p lingxi-tui --test behavior_ansi_in_bash_result`
  Expected: FAIL — `render_user_tool_result_body_spans` does not exist.

- [ ] **Step 3: Add the span-producing helper.**

  In `crates/tui/src/components/messages/user_tool_result.rs`, add at the end (before `#[cfg(test)]`):

  ```rust
  use crate::ansi::{AnsiSpan, parse_ansi};

  /// Produce the colored spans for the body — only Bash output runs through
  /// the ANSI parser; everything else is one default-styled span.
  pub fn render_user_tool_result_body_spans(props: &UserToolResultProps) -> Vec<AnsiSpan> {
      let body = body_text(&props.result);
      let (truncated, _dropped) = truncate(&body);
      if props.tool == "Bash" {
          parse_ansi(&truncated)
      } else {
          vec![AnsiSpan {
              style: Default::default(),
              text: truncated,
          }]
      }
  }
  ```

  Then update the iocraft `UserToolResultMessage` component to iterate the spans and emit one `Text` element per span with the color mapped from `AnsiColor` to `iocraft::Color`:

  ```rust
  fn ansi_to_iocraft_color(c: AnsiColor) -> Color {
      match c {
          AnsiColor::Default => Color::Reset,
          AnsiColor::Black => Color::Black,
          AnsiColor::Red => Color::DarkRed,
          AnsiColor::Green => Color::DarkGreen,
          AnsiColor::Yellow => Color::DarkYellow,
          AnsiColor::Blue => Color::DarkBlue,
          AnsiColor::Magenta => Color::DarkMagenta,
          AnsiColor::Cyan => Color::DarkCyan,
          AnsiColor::White => Color::Grey,
          AnsiColor::BrightBlack => Color::DarkGrey,
          AnsiColor::BrightRed => Color::Red,
          AnsiColor::BrightGreen => Color::Green,
          AnsiColor::BrightYellow => Color::Yellow,
          AnsiColor::BrightBlue => Color::Blue,
          AnsiColor::BrightMagenta => Color::Magenta,
          AnsiColor::BrightCyan => Color::Cyan,
          AnsiColor::BrightWhite => Color::White,
      }
  }
  ```

  Use this in the component body; for the non-Bash branch (single default-styled span) the result is identical to the M6-04 Task 5 rendering, so existing snapshot tests still pass.

- [ ] **Step 4: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui --test behavior_ansi_in_bash_result`
  Expected: 2 tests PASS.

  Run: `cargo test -p lingxi-tui` (full crate) — confirm no regressions in earlier snapshots.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/tui/src/components/messages/user_tool_result.rs \
          crates/tui/tests/behavior_ansi_in_bash_result.rs
  git commit -m "feat(tui): Bash tool output passes through ANSI SGR parser"
  ```

---

## Task 13: End-to-end smoke — `OutputEvent` → scrollback → render

**Files:**
- Create: `crates/tui/tests/behavior_e2e_tool_flow.rs`

- [ ] **Step 1: Write the failing behavior test.**

  Create `crates/tui/tests/behavior_e2e_tool_flow.rs`:

  ```rust
  use lingxi_protocol::ToolUseId;
  use lingxi_traits::OutputEvent;
  use lingxi_tui::app::AppState;
  use lingxi_tui::components::messages::render_entry_to_string;

  #[test]
  fn full_flow_call_then_result_renders_both_blocks() {
      let mut app = AppState::default();
      let id = ToolUseId::from("toolu_x");
      app.apply_output_event(OutputEvent::ToolCall {
          id: id.clone(),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x.rs"}),
      });
      app.apply_output_event(OutputEvent::ToolResult {
          id: id.clone(),
          tool: "Read".into(),
          result: serde_json::json!({"content": "fn main() {}"}),
      });
      assert_eq!(app.scrollback.len(), 2);
      let s0 = render_entry_to_string(&app.scrollback[0], false, false);
      assert!(s0.contains("● Read"));
      let s1 = render_entry_to_string(&app.scrollback[1], false, false);
      assert!(s1.starts_with("└ fn main() {}"));
  }

  #[test]
  fn expanded_state_flips_with_toggle() {
      let mut app = AppState::default();
      let id = ToolUseId::from("toolu_x");
      app.apply_output_event(OutputEvent::ToolCall {
          id: id.clone(),
          tool: "Read".into(),
          input: serde_json::json!({"file_path": "/tmp/x.rs"}),
      });
      app.focus_next_tool();
      app.toggle_expanded(&id);
      let expanded = app.expanded.get(&id).copied().unwrap_or(false);
      let s = render_entry_to_string(&app.scrollback[0], true, expanded);
      // Expanded form contains the pretty-printed JSON body.
      assert!(s.contains("{\n  \"file_path\""), "got: {s}");
  }
  ```

- [ ] **Step 2: Run tests to verify they pass.**

  Run: `cargo test -p lingxi-tui --test behavior_e2e_tool_flow`
  Expected: PASS — all the surface from Tasks 2, 9, 11 is now wired.

  Run the full workspace gate:

  ```bash
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

  Expected: all green. Three known flakes (per spec §5.4) may need a retry; same as M5/v0.6.0.

- [ ] **Step 3: Manual verification.**

  Run `cargo run -p lingxi-cli` against a real prompt that triggers a Read or Bash call (e.g. `cat .gitignore` proxied through Bash, or `read this file: /tmp/x`). Observe:
  - The `● Tool(input_preview)` header appears as soon as the tool is dispatched.
  - The `└ ` result block appears below it when the tool returns.
  - Long Bash output is truncated to 100 lines with the footer.
  - Press Esc to enter scrollback mode, Up/Down walks focus, `> ` arrow moves with focus.
  - Press `e` or Enter on the focused block; expanded body appears (pretty-printed JSON for tool-use, full body for result).
  - Press `e` again; body collapses.
  - For a Bash output containing color escapes, the colors appear.

- [ ] **Step 4: Commit.**

  ```bash
  git add crates/tui/tests/behavior_e2e_tool_flow.rs
  git commit -m "test(tui): e2e flow — ToolCall + ToolResult through AppState to render

  Closes M6-04. m6.4 ready for tag once gate is green."
  ```

---

## Task 14: Tag `m6.4` and update the M6 progress index

**Files:**
- Modify: `docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md` (mark M6-04 ✅ in §3 summary table)

- [ ] **Step 1: Workspace verification gate (final).**

  Run:

  ```bash
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  cargo check --workspace --target x86_64-unknown-linux-gnu
  cargo check --workspace --target x86_64-apple-darwin
  ```

  Expected: all green. (Windows + Android + iOS cross-compile checks deferred to M6-09.)

- [ ] **Step 2: Confirm the verification checklist from §5.5 row M6-04.**

  Manually run `lingxi-cli` against a project, prompt with something that calls `Read` and something that calls `Bash`, confirm: (a) call header appears, (b) result block appears below, (c) `e`/Enter expand/collapse works, (d) `> ` focus arrow moves with Up/Down in scrollback mode, (e) Bash color escapes render colored, (f) `\x1b[?2004h`-style unsupported sequences do not corrupt output (test by piping `printf '\x1b[?2004hok'` through Bash).

- [ ] **Step 3: Update the M6 spec progress table.**

  In `docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md` §3 summary table, mark the M6-04 row as shipped (add ✅ prefix to its `What lands` cell, matching the M5 series' convention).

- [ ] **Step 4: Commit the spec progress marker.**

  ```bash
  git add docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md
  git commit -m "docs(m6): mark M6-04 as shipped in spec progress table"
  ```

- [ ] **Step 5: Tag the milestone (locally only — no remote push).**

  ```bash
  git tag -a m6.4 -m "M6-04 Tool Use Rendering — AssistantToolUse + UserToolResult + ANSI parser"
  git tag -l m6.4 -n5
  ```

  Expected output includes the annotated tag with the message above.

- [ ] **Step 6: Final status check.**

  ```bash
  git status
  git log --oneline -10
  git tag --list 'm6.*'
  ```

  Expected: working tree clean; the last ~6 commits are M6-04's Task 1-13 commits; tag `m6.4` listed alongside `m6.1`, `m6.2`, `m6.3`.

---

## Self-Review

Spec coverage check against the parent task brief:

1. `assistant_tool_use.rs` with `●` marker, header `● ToolName(input_preview)`, JSON-pretty-printed expanded — Task 3 + Task 4. ✅
2. `user_tool_result.rs` with `└ ` marker, dim color, 100-line OR 4000-char truncation, footer `[output truncated, N more lines]`, Bash output through ANSI parser — Tasks 5, 6, 12. ✅
3. `crates/tui/src/ansi.rs` minimal SGR parser (reset, bold, 30-37, 40-47, 90-97, 100-107), skips unsupported, no panic — Tasks 7 + 8. ✅
4. Focus model: `focused_tool_id: Option<ToolUseId>`, Up/Down moves focus, `> ` left arrow shows focus — Tasks 9 + 10. ✅ Note: `> ` is rendered as a prefix inside `render_assistant_tool_use_to_string` / `render_user_tool_result_to_string`, not as a separate column, to keep the renderer pure.
5. Dispatcher in `messages/mod.rs` routes both variants — Task 11. ✅
6. `AppState` handles `OutputEvent::ToolCall` + `OutputEvent::ToolResult`, pushes to scrollback — Task 2 (event wiring) + Task 13 (e2e test). ✅
7. Locked types: `OutputEvent::ToolCall { id, tool, input }` — Task 1 (adds the `id` field; the brief used `ToolUseStart/ToolUseResult` as design-spec aliases for the actual `ToolCall/ToolResult` variants from M5-04). ✅
8. `expanded: HashMap<ToolUseId, bool>` default false — Task 9. ✅
9. Snapshot tests for collapsed + expanded `AssistantToolUseMessage` — Tasks 3 + 4. ✅
10. Snapshot tests for 5-line and 200-line `UserToolResultMessage` — Tasks 5 + 6. ✅
11. Behavior test: focus + `e` toggles expanded — Task 10. ✅
12. Behavior test: ANSI parser with `\x1b[31mERR\x1b[0m\x1b[1mBOLD\x1b[0m` → 3 spans — Task 8 (`three_spans_red_bold_default`). ✅
13. Behavior test: `\x1b[?2004h` skipped without panic — Task 8 (`unsupported_csi_is_skipped_without_panic`). ✅
14. Unit tests: empty string, single CSI, mixed, malformed — Task 8 (`empty_string_parses_to_empty_vec`, `malformed_unterminated_csi_does_not_panic`, `multi_param_sgr_bold_red`, `three_spans_red_bold_default`). ✅
15. Final task tags `m6.4` — Task 14. ✅

Decisions taken during plan authoring:
- **`ToolUseId` is added to `OutputEvent::ToolCall` / `OutputEvent::ToolResult` here, not in M5-04.** M5-04 carried the id internally (in `ObservedToolUse` and `ContentBlock::ToolUse`) but didn't surface it on the `OutputStream` boundary. M6-04 needs the id to key the `expanded` HashMap and to correlate calls↔results across the scrollback. Task 1 makes the additive change; the variant is `#[non_exhaustive]` so this is non-breaking.
- **Focus arrow `> ` is a string prefix, not a separate iocraft column.** Keeps the renderer pure-functional and snapshot-testable as a single string. The visual is identical to a column-based render at the cost of recomputing the prefix per frame — negligible.
- **Bash-only ANSI parsing.** Non-Bash tools (Read, Grep, Glob, etc.) keep their bytes literal — no parser. Matches claude-code's discipline.
- **Truncation limits 100 lines / 4000 bytes from claude-code constants** (`MAX_LINES_PRINTED_PER_TOOL_USE_RESULT`, `MAX_CHARACTERS_PRINTED_PER_TOOL_USE_RESULT`); both apply, whichever bites first wins.
- **Collapsed result format `└ first_line (+N lines)`** when there is more than one line — claude-code-equivalent compact summary so a closed tool result occupies exactly one row in the scrollback.
- **`render_entry_to_string` is a pure helper** alongside the iocraft component — snapshot tests run against the string form, the iocraft component delegates to it for the visible body. Avoids the cost of fully rendering iocraft trees in tests.

Type consistency check: `ToolUseId`, `AnsiColor`, `AnsiStyle`, `AnsiSpan`, `AssistantToolUseProps`, `UserToolResultProps`, `MARKER`, `FOCUS_PREFIX`, `MAX_LINES`, `MAX_BYTES`, `render_assistant_tool_use_to_string`, `render_user_tool_result_to_string`, `render_user_tool_result_body_spans`, `render_entry_to_string`, `focus_next_tool`, `focus_prev_tool`, `toggle_expanded`, `apply_output_event`, `handle_key`, `AppState`, `AppMode`, `ScrollbackEntry::{AssistantToolCall, UserToolResult}` — all names match across tasks. No drift.

Placeholder scan: none. Every step has its full code body or its exact verification command.

**Plan complete.** Estimate: 14 tasks, ~50 sub-steps, 2-3 calendar days at sustained M5 pace.
