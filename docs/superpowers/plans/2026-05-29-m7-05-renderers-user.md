# M7-05 — Message Renderers Batch 2 (user, 12 renderers) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the 12 user-side message renderers (bash-input, bash-output, command, local-command-output, memory-input, plan, prompt, resource-update, image, attachment, grouped-tool-use, collapsed-read-search) to the TUI scrollback, each a focused file under `crates/tui/src/components/messages/`, wired through both dispatchers, with markdown/code/ANSI bodies routed through the M7-01/02 `render/` primitives.

**Architecture:** Each renderer follows the M6-04 pattern: a `RenderedMessage` enum variant in `state.rs`, a pure string-form renderer fn + an iocraft `#[component]` in its own file, and a dispatch arm in **both** `components/messages/mod.rs::render_entry_to_string` (string form, used by snapshot/parity tests) and `components/scrollback.rs::render_message` (iocraft element form, used by the live mount). Markdown bodies (plan, local-command-output) go through `render::markdown::render`; bash output goes through the full ANSI parser (`render::ansi` from M7-01, the expanded successor to today's `crate::ansi`); the two folding renderers (grouped-tool-use, collapsed-read-search) carry a list of child entries and collapse/expand on the per-id `AppState.expanded` map already established in M6-04. Image is a `[Image #N]`/`[Image]` placeholder + metadata only — terminal image protocols defer to M8.

**Tech Stack:** Rust (workspace MSRV pinned to 1.82 via `lingxi-code/rust-toolchain.toml`), iocraft `=0.8.3` (`View`, not `Box`; `Text`, `element!`, `#[component]`, `Props`), `insta` snapshots, `serde_json::Value` for tool input/result payloads, `lingxi_protocol::ToolUseId` correlators.

---

## Context the implementer needs (read before starting)

**This is a TUI-surface-only milestone. Zero engine changes.** All renderers are pure functions of their inputs; no async, no orchestrator calls.

**Run cargo from inside `lingxi-code/`** — the repo root uses the host toolchain and produces spurious clippy noise (this bit M6-08). Every build/test/clippy command in this plan assumes cwd `lingxi-code/`.

**The crate lives at `lingxi-code/crates/tui/`** (NOT `crates/tui/` at repo root). All paths below are relative to repo root `/Users/luolingfeng/Projects/LingXi-Next`.

**Two dispatchers, always edited together** (this is the single most common mistake to avoid):
- `lingxi-code/crates/tui/src/components/messages/mod.rs` → `render_entry_to_string(entry, focused, expanded) -> String`. Match arm per variant. Used by snapshot tests + parity fixtures.
- `lingxi-code/crates/tui/src/components/scrollback.rs` → `render_message(m, expanded, focused_tool_id) -> AnyElement<'static>`. Match arm per variant. Used by the live iocraft mount.

A new `RenderedMessage` variant that is added to `state.rs` but missing from either dispatcher's `match` will FAIL TO COMPILE (non-exhaustive match) — that is the safety net; rely on it.

**Existing reference renderers to copy the shape from:**
- `lingxi-code/crates/tui/src/components/messages/user_text.rs` — minimal: `Props` struct, one `#[component]`, `format!("> {}", body)`, `TuiTheme::USER`.
- `lingxi-code/crates/tui/src/components/messages/user_tool_result.rs` — the full pattern: pure string-form renderer fn (`render_user_tool_result_to_string`), collapsed-vs-expanded branching on `props.expanded`, ANSI span pipeline (`render_user_tool_result_body_spans` → `parse_ansi`), and the `#[component]` that wraps it. **Copy its ANSI-span → iocraft-color mapping helper `ansi_to_iocraft_color` for bash-output and local-command-output.**
- `lingxi-code/crates/tui/src/components/messages/assistant_text.rs` — `TuiTheme::ASSISTANT` cyan + `● ` prefix.

**Theme constants available today** (`lingxi-code/crates/tui/src/theme.rs`): `TuiTheme::ASSISTANT` (Cyan), `TuiTheme::USER` (Reset), `TuiTheme::ERROR` (Red), `TuiTheme::DIM` (DarkGrey). claude-code uses named theme colors (`bashBorder`, `subtle`, `planMode`, `remember`, `success`, `suggestion`) that do NOT exist in `theme.rs` yet and are NOT in scope for M7-05. **Map claude-code's named colors to the closest existing `TuiTheme` constant** (see the per-task literal tables) — do NOT add new theme constants here (that is M7-15's job). Record the chosen mapping in a `//` doc comment so M7-15 can revisit.

**ANSI / markdown primitives:** M7-01 lands `render::ansi` (full 256/truecolor parser) and `render::markdown::render(text, theme) -> Vec<StyledLine>`; M7-02 lands `render::syntax::highlight`. The styled-line type produced by these is the M7-01 `render::StyledLine` (a `Vec` of styled spans per line). **If `render/` is not yet present when you start** (M7-01/02 are prerequisites — verify with `ls lingxi-code/crates/tui/src/render/`), STOP and flag the missing prerequisite; do not stub the primitives. For bash output specifically, the existing `crate::ansi::parse_ansi` (8/16-color) is an acceptable fallback only if `render::ansi` is genuinely absent — note it in the file's doc comment as a downgrade and add a `// TODO(M7-01): switch to render::ansi` marker.

**Telemetry:** M7-05 adds **0** events. Baseline stays at 326. Do not register any event names. If a test asserts `ALL_EVENT_NAMES.len()`, it must still read 326 after this sub-plan.

**Literal lock (spec §2.8):** Every user-visible string matches claude-code byte-for-byte. The per-task tables below carry the exact literals pulled from the claude-code `.tsx` sources; the implementer must reproduce them exactly. Each renderer file gets a `//! Literal locks:` doc-comment block citing the `.tsx` source path, mirroring `user_tool_result.rs`.

**Folding renderers — scope decisions (locked):**
- **GroupedToolUseContent** (claude-code `GroupedToolUseContent.tsx`): groups consecutive tool-use blocks **of the same tool name** and delegates to the tool's `renderGroupedToolUse`. M7-05 reproduces the *folding structure* (one `GroupedToolUse` entry carries `tool: String` + `Vec<(input, result)>`), NOT the per-tool custom renderers (those are the M7-04/06+ tool-renderer surface). Collapsed → `● {tool} (×{N})`; expanded → header line + each child rendered via the existing `render_assistant_tool_use_to_string` / `render_user_tool_result_to_string` shape. Uses the per-id `AppState.expanded` map (key = the group's first `ToolUseId`).
- **CollapsedReadSearchContent** (claude-code `CollapsedReadSearchContent.tsx`): folds Read/Search/List tool runs into one count-summary line. M7-05 scope = the read/search/list counts (the solo-user core): collapsed summary line `  ⎿  {Verb} {N} {noun}[, …]` with comma-joined parts, first-part capitalized, present-tense when active / past-tense when finalized; expanded → one `  ⎿  ` row per folded tool use. **Out of scope for M7-05** (defer to M8, document in the file): git/commit/PR/push/branch parts, bash-command counts (fullscreen-only), MCP-query parts, auto-memory parts, team-memory parts, and the live `⤿` progress hint / min-display-time debounce. Keep the verb/noun vocabulary exactly: `Searched for`/`Searching for` + `pattern`/`patterns`; `Read`/`Reading` + `file`/`files`; `Listed`/`Listing` + `directory`/`directories`.

---

## File Structure

**New files (12 renderers + 1 fold helper):**

| File | Responsibility |
|---|---|
| `lingxi-code/crates/tui/src/components/messages/bash_input.rs` | `UserBashInputMessage` — `! ` prefix + command text |
| `lingxi-code/crates/tui/src/components/messages/bash_output.rs` | `UserBashOutputMessage` — stdout/stderr through ANSI parser |
| `lingxi-code/crates/tui/src/components/messages/command.rs` | `UserCommandMessage` — `❯ /cmd args` or `❯ Skill(name)` |
| `lingxi-code/crates/tui/src/components/messages/local_command_output.rs` | `UserLocalCommandOutputMessage` — `  ⎿  ` gutter + markdown body |
| `lingxi-code/crates/tui/src/components/messages/memory_input.rs` | `UserMemoryInputMessage` — `# {input}` + saving line |
| `lingxi-code/crates/tui/src/components/messages/plan.rs` | `UserPlanMessage` — bordered "Plan to implement" + markdown |
| `lingxi-code/crates/tui/src/components/messages/prompt.rs` | `UserPromptMessage` — prompt text with head+tail truncation |
| `lingxi-code/crates/tui/src/components/messages/resource_update.rs` | `UserResourceUpdateMessage` — `↻ server: target · reason` lines |
| `lingxi-code/crates/tui/src/components/messages/image.rs` | `UserImageMessage` — `[Image #N]` / `[Image]` placeholder + metadata |
| `lingxi-code/crates/tui/src/components/messages/attachment.rs` | `AttachmentMessage` — solo-user attachment `Line` summaries |
| `lingxi-code/crates/tui/src/components/messages/grouped_tool_use.rs` | `GroupedToolUseContent` — same-tool folding |
| `lingxi-code/crates/tui/src/components/messages/collapsed_read_search.rs` | `CollapsedReadSearchContent` — Read/Search/List count folding |

**Modified files:**

| File | Change |
|---|---|
| `lingxi-code/crates/tui/src/state.rs` | +12 `RenderedMessage` variants |
| `lingxi-code/crates/tui/src/components/messages/mod.rs` | +12 `pub mod` declarations; +12 arms in `render_entry_to_string` |
| `lingxi-code/crates/tui/src/components/scrollback.rs` | +12 arms in `render_message` |

**New test files (one per renderer group, mirroring `tests/render_messages.rs`):**

| File | Covers |
|---|---|
| `lingxi-code/crates/tui/tests/render_user_bash.rs` | bash_input, bash_output (incl. ANSI passthrough) |
| `lingxi-code/crates/tui/tests/render_user_command.rs` | command, local_command_output |
| `lingxi-code/crates/tui/tests/render_user_misc.rs` | memory_input, plan, prompt, resource_update, image |
| `lingxi-code/crates/tui/tests/render_user_attachment.rs` | attachment |
| `lingxi-code/crates/tui/tests/render_folding.rs` | grouped_tool_use, collapsed_read_search (collapsed + expanded) |
| `lingxi-code/crates/tui/tests/dispatch_user_renderers.rs` | one dispatch test per variant (both dispatchers compile + route) |

---

## Task 1: `UserBashInputMessage` renderer + variant + dispatch

**Files:**
- Modify: `lingxi-code/crates/tui/src/state.rs` (add variant)
- Create: `lingxi-code/crates/tui/src/components/messages/bash_input.rs`
- Modify: `lingxi-code/crates/tui/src/components/messages/mod.rs`
- Modify: `lingxi-code/crates/tui/src/components/scrollback.rs`
- Test: `lingxi-code/crates/tui/tests/render_user_bash.rs`

**Literal lock** (claude-code `UserBashInputMessage.tsx`): prefix `"! "` (color `bashBorder` → map to `TuiTheme::DIM`); command text in `text` color (→ `TuiTheme::USER`). The input is the inner text of a `<bash-input>…</bash-input>` tag in the engine payload; M7-05 receives it already extracted as a plain `String`. Empty input → render nothing.

- [ ] **Step 1: Write the failing snapshot test**

In `lingxi-code/crates/tui/tests/render_user_bash.rs`:
```rust
use iocraft::prelude::*;
use lingxi_tui::components::messages::bash_input::UserBashInputMessage;

#[test]
fn bash_input_renders_bang_prefix() {
    let mut element = element! {
        UserBashInputMessage(command: "ls -la".to_string())
    };
    insta::assert_snapshot!("bash_input_basic", element.to_string());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run (cwd `lingxi-code/`): `cargo test -p lingxi-tui --test render_user_bash bash_input_renders_bang_prefix`
Expected: FAIL — `unresolved import lingxi_tui::components::messages::bash_input`.

- [ ] **Step 3: Add the `RenderedMessage` variant**

In `lingxi-code/crates/tui/src/state.rs`, inside `enum RenderedMessage`:
```rust
    /// (M7-05) User bash-mode command line (`!` prefix). Body is the
    /// command text already extracted from the `<bash-input>` engine tag.
    UserBashInput {
        /// The command line the user typed in `!` bash mode.
        command: String,
    },
```

- [ ] **Step 4: Write the renderer**

Create `lingxi-code/crates/tui/src/components/messages/bash_input.rs`:
```rust
//! `UserBashInputMessage` — `! ` prefix + command text.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - prefix: `! ` (color `bashBorder` → mapped to TuiTheme::DIM here;
//!     M7-15 may introduce a dedicated bash-border color)
//!   - command text color: `text` → TuiTheme::USER
//!   source: claude-code/src/components/messages/UserBashInputMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// `! ` prefix glyph + space (color `bashBorder` in claude-code).
pub const PREFIX: &str = "! ";

/// Props for [`UserBashInputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserBashInputProps {
    /// Command line text (already extracted from `<bash-input>`).
    pub command: String,
}

/// Pure string-form renderer: `"! {command}"`. Empty command → empty string.
#[must_use]
pub fn render_bash_input_to_string(command: &str) -> String {
    if command.is_empty() {
        return String::new();
    }
    format!("{PREFIX}{command}")
}

/// iocraft component.
#[component]
pub fn UserBashInputMessage(props: &UserBashInputProps) -> impl Into<AnyElement<'static>> {
    let command = props.command.clone();
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: PREFIX, color: TuiTheme::DIM)
            Text(content: command, color: TuiTheme::USER)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_command_renders_empty() {
        assert_eq!(render_bash_input_to_string(""), "");
    }

    #[test]
    fn command_gets_bang_prefix() {
        assert_eq!(render_bash_input_to_string("ls"), "! ls");
    }
}
```

- [ ] **Step 5: Register the module + both dispatch arms**

In `lingxi-code/crates/tui/src/components/messages/mod.rs`, add module + dispatch arm:
```rust
pub mod bash_input;
```
and in `render_entry_to_string`, add:
```rust
        RenderedMessage::UserBashInput { command } => {
            bash_input::render_bash_input_to_string(command)
        }
```

In `lingxi-code/crates/tui/src/components/scrollback.rs`, add to `render_message`:
```rust
        RenderedMessage::UserBashInput { command } => element! {
            UserBashInputMessage(command: command)
        }
        .into_any(),
```
and add the import at the top: `use crate::components::messages::bash_input::UserBashInputMessage;`

- [ ] **Step 6: Run the snapshot test, accept the snapshot**

Run (cwd `lingxi-code/`): `cargo test -p lingxi-tui --test render_user_bash bash_input_renders_bang_prefix`
Expected: FAIL on first run with a pending snapshot. Review the `.snap.new` shows `! ls -la`, then accept: `cargo insta accept`. Re-run → PASS.

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/crates/tui/src/state.rs \
        lingxi-code/crates/tui/src/components/messages/bash_input.rs \
        lingxi-code/crates/tui/src/components/messages/mod.rs \
        lingxi-code/crates/tui/src/components/scrollback.rs \
        lingxi-code/crates/tui/tests/render_user_bash.rs \
        lingxi-code/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-05 T1): UserBashInputMessage renderer + variant + dispatch

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `UserBashOutputMessage` renderer (ANSI passthrough)

**Files:**
- Modify: `lingxi-code/crates/tui/src/state.rs`
- Create: `lingxi-code/crates/tui/src/components/messages/bash_output.rs`
- Modify: `lingxi-code/crates/tui/src/components/messages/mod.rs`
- Modify: `lingxi-code/crates/tui/src/components/scrollback.rs`
- Test: `lingxi-code/crates/tui/tests/render_user_bash.rs`

**Literal lock** (claude-code `UserBashOutputMessage.tsx` → `BashToolResultMessage`): the body is `<bash-stdout>` (unwrapping an inner `<persisted-output>` if present) and `<bash-stderr>`; M7-05 receives `stdout: String` and `stderr: String` already extracted. Bash output carries ANSI SGR codes — it MUST route through the ANSI parser, NOT be rendered as a plain string. Reuse `user_tool_result.rs`'s `ansi_to_iocraft_color` mapping (copy it into a shared helper or duplicate with a doc comment; do not refactor `user_tool_result.rs` in this task).

- [ ] **Step 1: Write the failing ANSI-passthrough test**

In `lingxi-code/crates/tui/tests/render_user_bash.rs` append:
```rust
use lingxi_tui::components::messages::bash_output::{render_bash_output_spans, UserBashOutputMessage};

#[test]
fn bash_output_parses_ansi_into_spans() {
    // SGR 31 (red) "err" reset, then plain "ok".
    let spans = render_bash_output_spans("\x1b[31merr\x1b[0mok", "");
    // The parser must split the colored prefix from the plain suffix.
    assert!(spans.len() >= 2, "expected ANSI split into >=2 spans, got {}", spans.len());
    assert_eq!(spans.iter().map(|s| s.text.as_str()).collect::<String>(), "errok");
}

#[test]
fn bash_output_snapshot() {
    let mut element = element! {
        UserBashOutputMessage(stdout: "hello\nworld".to_string(), stderr: "".to_string())
    };
    insta::assert_snapshot!("bash_output_plain", element.to_string());
}
```

- [ ] **Step 2: Run to verify failure**

Run (cwd `lingxi-code/`): `cargo test -p lingxi-tui --test render_user_bash bash_output`
Expected: FAIL — unresolved `bash_output` module.

- [ ] **Step 3: Add the variant**

In `state.rs`:
```rust
    /// (M7-05) Bash tool output. stdout/stderr already extracted from the
    /// `<bash-stdout>`/`<bash-stderr>` engine tags. Body carries ANSI SGR
    /// codes and is parsed through the ANSI parser at render time.
    UserBashOutput {
        /// Standard output (ANSI-coded).
        stdout: String,
        /// Standard error (ANSI-coded).
        stderr: String,
    },
```

- [ ] **Step 4: Write the renderer**

Create `lingxi-code/crates/tui/src/components/messages/bash_output.rs`:
```rust
//! `UserBashOutputMessage` — stdout/stderr rendered through the ANSI parser.
//!
//! Literal locks / behavior:
//!   - body = `<bash-stdout>` (inner `<persisted-output>` unwrapped) + `<bash-stderr>`
//!   - bash output carries SGR codes → parsed via the ANSI parser, NOT plain text
//!   source: claude-code/src/components/messages/UserBashOutputMessage.tsx
//!           + tools/BashTool/BashToolResultMessage.tsx
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

// TODO(M7-01): switch to `crate::render::ansi` (256/truecolor) once present.
use crate::ansi::{parse_ansi, AnsiColor, AnsiSpan, AnsiStyle};
use crate::theme::TuiTheme;

/// Props for [`UserBashOutputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserBashOutputProps {
    /// Standard output (ANSI-coded).
    pub stdout: String,
    /// Standard error (ANSI-coded).
    pub stderr: String,
}

/// Join stdout + stderr (stderr after stdout, separated by a newline when both
/// non-empty) and parse the whole into ANSI spans.
#[must_use]
pub fn render_bash_output_spans(stdout: &str, stderr: &str) -> Vec<AnsiSpan> {
    let mut body = String::new();
    if !stdout.is_empty() {
        body.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(stderr);
    }
    parse_ansi(&body)
}

/// Map an [`AnsiColor`] to an iocraft [`Color`] (same mapping as
/// `user_tool_result::ansi_to_iocraft_color`).
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

/// iocraft component — each ANSI span becomes a styled `Text` child.
#[component]
pub fn UserBashOutputMessage(props: &UserBashOutputProps) -> impl Into<AnyElement<'static>> {
    let spans = render_bash_output_spans(&props.stdout, &props.stderr);
    let children: Vec<AnyElement<'static>> = spans
        .into_iter()
        .map(|s| {
            let color = ansi_to_iocraft_color(s.style.fg);
            let weight = if s.style.bold { Weight::Bold } else { Weight::Normal };
            element! { Text(content: s.text, color: color, weight: weight) }.into_any()
        })
        .collect();
    let _ = AnsiStyle::default(); // keep import meaningful if spans empty
    element! {
        View(flex_direction: FlexDirection::Row) {
            #(children)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_stdout_and_stderr() {
        let spans = render_bash_output_spans("out", "err");
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "out\nerr");
    }
}
```
Note: `TuiTheme` import may be unused — if clippy flags it, drop the `use crate::theme::TuiTheme;` line.

- [ ] **Step 5: Register module + both dispatch arms**

`messages/mod.rs`: add `pub mod bash_output;` and:
```rust
        RenderedMessage::UserBashOutput { stdout, stderr } => {
            // String form strips ANSI; the parser drops escape codes, so
            // join the span texts.
            bash_output::render_bash_output_spans(stdout, stderr)
                .into_iter()
                .map(|s| s.text)
                .collect::<String>()
        }
```
`scrollback.rs`: import `use crate::components::messages::bash_output::UserBashOutputMessage;` and:
```rust
        RenderedMessage::UserBashOutput { stdout, stderr } => element! {
            UserBashOutputMessage(stdout: stdout, stderr: stderr)
        }
        .into_any(),
```

- [ ] **Step 6: Run tests, accept snapshot**

Run (cwd `lingxi-code/`): `cargo test -p lingxi-tui --test render_user_bash bash_output`
Expected: `bash_output_parses_ansi_into_spans` PASS; snapshot pending → `cargo insta accept` → re-run PASS.

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/crates/tui/src/state.rs \
        lingxi-code/crates/tui/src/components/messages/bash_output.rs \
        lingxi-code/crates/tui/src/components/messages/mod.rs \
        lingxi-code/crates/tui/src/components/scrollback.rs \
        lingxi-code/crates/tui/tests/render_user_bash.rs \
        lingxi-code/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-05 T2): UserBashOutputMessage renderer with ANSI passthrough

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `UserCommandMessage` + `UserLocalCommandOutputMessage`

**Files:**
- Modify: `state.rs`, `messages/mod.rs`, `scrollback.rs`
- Create: `messages/command.rs`, `messages/local_command_output.rs`
- Test: `lingxi-code/crates/tui/tests/render_user_command.rs`

**Literal locks:**
- `UserCommandMessage.tsx`: prefix `figures.pointer` = `❯ ` (U+276F + space, color `subtle` → `TuiTheme::DIM`). Slash form content = `/{command}` joined with `args` by a space → `/{command} {args}` (args omitted when empty). Skill form (`<skill-format>true</skill-format>`) → `Skill({command})`. Body text color `text` → `TuiTheme::USER`.
- `UserLocalCommandOutputMessage.tsx`: stdout from `<local-command-stdout>`, stderr from `<local-command-stderr>` (M7-05 receives both extracted). Indent gutter `"  ⎿  "` (two spaces + U+23BF + two spaces, dim). Body rendered as **markdown** (`render::markdown`). When both stdout+stderr empty → render `NO_CONTENT_MESSAGE` dim (look up the exact literal in `claude-code/src/constants/messages.ts`; it is `"(no content)"` — verify and copy byte-for-byte). M7-05 scope: render the `IndentedContent` path only (gutter + markdown body); the `CloudLaunchContent` diamond branch (`◇`/`◆` prefixed lines) defers to M8 — document.

- [ ] **Step 1: Write failing tests**

In `lingxi-code/crates/tui/tests/render_user_command.rs`:
```rust
use iocraft::prelude::*;
use lingxi_tui::components::messages::command::{render_command_to_string, UserCommandMessage};
use lingxi_tui::components::messages::local_command_output::{
    render_local_output_to_string, UserLocalCommandOutputMessage,
};

#[test]
fn command_slash_form() {
    assert_eq!(render_command_to_string("clear", "", false), "❯ /clear");
    assert_eq!(render_command_to_string("model", "sonnet", false), "❯ /model sonnet");
}

#[test]
fn command_skill_form() {
    assert_eq!(render_command_to_string("brainstorm", "", true), "❯ Skill(brainstorm)");
}

#[test]
fn command_snapshot() {
    let mut e = element! { UserCommandMessage(command: "help".to_string(), args: "".to_string(), is_skill: false) };
    insta::assert_snapshot!("command_slash", e.to_string());
}

#[test]
fn local_output_no_content() {
    assert_eq!(render_local_output_to_string("", ""), "(no content)");
}

#[test]
fn local_output_snapshot() {
    let mut e = element! { UserLocalCommandOutputMessage(stdout: "done".to_string(), stderr: "".to_string()) };
    insta::assert_snapshot!("local_output_done", e.to_string());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-tui --test render_user_command`
Expected: FAIL — unresolved modules.

- [ ] **Step 3: Add variants**

In `state.rs`:
```rust
    /// (M7-05) Slash-command echo. `❯ /{command} {args}` or `❯ Skill(name)`.
    UserCommand {
        /// Command name (without leading slash).
        command: String,
        /// Argument string (may be empty).
        args: String,
        /// `true` → render `Skill(name)` form instead of `/name args`.
        is_skill: bool,
    },
    /// (M7-05) Output of a local (slash) command. stdout/stderr already
    /// extracted; body rendered as markdown under a `  ⎿  ` gutter.
    UserLocalCommandOutput {
        /// Local-command stdout.
        stdout: String,
        /// Local-command stderr.
        stderr: String,
    },
```

- [ ] **Step 4: Write `command.rs`**

Create `lingxi-code/crates/tui/src/components/messages/command.rs`:
```rust
//! `UserCommandMessage` — `❯ /cmd args` or `❯ Skill(name)`.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - prefix: `❯ ` (figures.pointer U+276F + space, color `subtle` → DIM)
//!   - slash form: `/{command} {args}` (args omitted when empty)
//!   - skill form: `Skill({command})`
//!   source: claude-code/src/components/messages/UserCommandMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// `❯ ` pointer prefix + space (color `subtle` in claude-code).
pub const PREFIX: &str = "❯ ";

/// Props for [`UserCommandMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserCommandProps {
    /// Command name without the leading slash.
    pub command: String,
    /// Argument string (may be empty).
    pub args: String,
    /// Render the `Skill(name)` form.
    pub is_skill: bool,
}

/// Pure string renderer. Returns `""` when `command` is empty.
#[must_use]
pub fn render_command_to_string(command: &str, args: &str, is_skill: bool) -> String {
    if command.is_empty() {
        return String::new();
    }
    if is_skill {
        return format!("{PREFIX}Skill({command})");
    }
    let body = if args.is_empty() {
        format!("/{command}")
    } else {
        format!("/{command} {args}")
    };
    format!("{PREFIX}{body}")
}

/// iocraft component.
#[component]
pub fn UserCommandMessage(props: &UserCommandProps) -> impl Into<AnyElement<'static>> {
    let content = render_command_to_string(&props.command, &props.args, props.is_skill);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: content, color: TuiTheme::USER)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_command_empty() {
        assert_eq!(render_command_to_string("", "x", false), "");
    }
}
```

- [ ] **Step 5: Write `local_command_output.rs`**

Create `lingxi-code/crates/tui/src/components/messages/local_command_output.rs`:
```rust
//! `UserLocalCommandOutputMessage` — `  ⎿  ` gutter + markdown body.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - gutter: `  ⎿  ` (2 spaces + U+23BF + 2 spaces, dim)
//!   - empty stdout+stderr → NO_CONTENT_MESSAGE = `(no content)`
//!   - body rendered as markdown
//!   - SCOPE: IndentedContent path only; CloudLaunchContent (◇/◆) defers to M8
//!   source: claude-code/src/components/messages/UserLocalCommandOutputMessage.tsx
//!           + constants/messages.ts (NO_CONTENT_MESSAGE)

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Dim gutter prepended to each indented content block.
pub const GUTTER: &str = "  ⎿  ";
/// Rendered when both stdout and stderr are empty (verify against
/// `constants/messages.ts`).
pub const NO_CONTENT_MESSAGE: &str = "(no content)";

/// Props for [`UserLocalCommandOutputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserLocalCommandOutputProps {
    /// Local-command stdout.
    pub stdout: String,
    /// Local-command stderr.
    pub stderr: String,
}

/// Pure string renderer. Trims stdout/stderr; joins the non-empty parts under
/// the gutter; empty → `NO_CONTENT_MESSAGE`.
#[must_use]
pub fn render_local_output_to_string(stdout: &str, stderr: &str) -> String {
    let out = stdout.trim();
    let err = stderr.trim();
    if out.is_empty() && err.is_empty() {
        return NO_CONTENT_MESSAGE.to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if !out.is_empty() {
        parts.push(format!("{GUTTER}{out}"));
    }
    if !err.is_empty() {
        parts.push(format!("{GUTTER}{err}"));
    }
    parts.join("\n")
}

/// iocraft component. The body would route through `render::markdown` once
/// M7-01 lands; until then the string form is rendered verbatim.
#[component]
pub fn UserLocalCommandOutputMessage(
    props: &UserLocalCommandOutputProps,
) -> impl Into<AnyElement<'static>> {
    let body = render_local_output_to_string(&props.stdout, &props.stderr);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_content_when_empty() {
        assert_eq!(render_local_output_to_string("  ", "\n"), "(no content)");
    }

    #[test]
    fn gutter_prepended() {
        assert_eq!(render_local_output_to_string("ok", ""), "  ⎿  ok");
    }
}
```
**Markdown note:** if `render::markdown` is present (M7-01 done), replace the component body with a `Column` of `Text` lines built from `render::markdown::render(&body, theme)`, mapping each `StyledLine` to a `Text` row. The string-form renderer stays as-is (markdown structure does not change the byte-string the parity test locks). Add a `// TODO(M7-01)` if deferring.

- [ ] **Step 6: Register modules + both dispatch arms**

`messages/mod.rs`: `pub mod command;` `pub mod local_command_output;` and arms:
```rust
        RenderedMessage::UserCommand { command, args, is_skill } => {
            command::render_command_to_string(command, args, *is_skill)
        }
        RenderedMessage::UserLocalCommandOutput { stdout, stderr } => {
            local_command_output::render_local_output_to_string(stdout, stderr)
        }
```
`scrollback.rs`: imports + arms:
```rust
        RenderedMessage::UserCommand { command, args, is_skill } => element! {
            UserCommandMessage(command: command, args: args, is_skill: is_skill)
        }
        .into_any(),
        RenderedMessage::UserLocalCommandOutput { stdout, stderr } => element! {
            UserLocalCommandOutputMessage(stdout: stdout, stderr: stderr)
        }
        .into_any(),
```

- [ ] **Step 7: Run tests, accept snapshots, commit**

Run: `cargo test -p lingxi-tui --test render_user_command` → fix until unit asserts PASS; `cargo insta accept`; re-run PASS.
```bash
git add lingxi-code/crates/tui/src/state.rs lingxi-code/crates/tui/src/components/messages/command.rs lingxi-code/crates/tui/src/components/messages/local_command_output.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/components/scrollback.rs lingxi-code/crates/tui/tests/render_user_command.rs lingxi-code/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-05 T3): UserCommand + UserLocalCommandOutput renderers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `UserMemoryInputMessage` + `UserPlanMessage`

**Files:**
- Modify: `state.rs`, `messages/mod.rs`, `scrollback.rs`
- Create: `messages/memory_input.rs`, `messages/plan.rs`
- Test: `lingxi-code/crates/tui/tests/render_user_misc.rs`

**Literal locks:**
- `UserMemoryInputMessage.tsx`: `#` glyph (color `remember`, bg `memoryBackgroundColor`) then `" {input} "` (color `text`, same bg). Below it a dim saving line — claude-code picks randomly from `['Got it.', 'Good to know.', 'Noted.']`. **Determinism:** snapshots must be stable, so M7-05 fixes the saving line to the first literal `"Got it."` (document the divergence: claude-code randomizes for variety; we pin for snapshot stability — record in the literal-lock catalog at M7-16). Input extracted from `<user-memory-input>`. Map `remember` → `TuiTheme::USER`, gutter `# ` rendered as a `# ` prefix.
- `UserPlanMessage.tsx`: a `round`-bordered box (`borderColor planMode`), header line `Plan to implement` (bold, color `planMode`), then the plan body rendered as **markdown**. Map `planMode` → `TuiTheme::ASSISTANT`. Header literal is exactly `Plan to implement`.

- [ ] **Step 1: Failing tests**

In `lingxi-code/crates/tui/tests/render_user_misc.rs`:
```rust
use iocraft::prelude::*;
use lingxi_tui::components::messages::memory_input::{render_memory_to_string, UserMemoryInputMessage};
use lingxi_tui::components::messages::plan::{render_plan_to_string, UserPlanMessage};

#[test]
fn memory_input_form() {
    assert_eq!(render_memory_to_string("prefer tabs"), "# prefer tabs\nGot it.");
}

#[test]
fn memory_snapshot() {
    let mut e = element! { UserMemoryInputMessage(input: "use rg".to_string()) };
    insta::assert_snapshot!("memory_use_rg", e.to_string());
}

#[test]
fn plan_header_literal() {
    let s = render_plan_to_string("- step one");
    assert!(s.starts_with("Plan to implement"), "got: {s}");
    assert!(s.contains("- step one"));
}

#[test]
fn plan_snapshot() {
    let mut e = element! { UserPlanMessage(plan_content: "1. do it".to_string()) };
    insta::assert_snapshot!("plan_basic", e.to_string());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-tui --test render_user_misc memory plan`
Expected: FAIL — unresolved modules.

- [ ] **Step 3: Add variants**

In `state.rs`:
```rust
    /// (M7-05) Memory write (`# {input}`) + a saving acknowledgement line.
    UserMemoryInput {
        /// Text the user added to memory (from `<user-memory-input>`).
        input: String,
    },
    /// (M7-05) Plan-mode plan body, rendered as bordered markdown under a
    /// "Plan to implement" header.
    UserPlan {
        /// Markdown plan content.
        plan_content: String,
    },
```

- [ ] **Step 4: Write `memory_input.rs`**
```rust
//! `UserMemoryInputMessage` — `# {input}` + a saving acknowledgement line.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - prefix glyph: `#` (color `remember` → USER)
//!   - saving line: claude-code samples ['Got it.', 'Good to know.', 'Noted.']
//!     at random; M7-05 PINS the first ("Got it.") for snapshot determinism
//!     (documented divergence — recorded in the M7-16 literal-lock catalog).
//!   source: claude-code/src/components/messages/UserMemoryInputMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Pinned saving-acknowledgement line (first of claude-code's sample set).
pub const SAVING_MESSAGE: &str = "Got it.";

/// Props for [`UserMemoryInputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserMemoryInputProps {
    /// Memory text (from `<user-memory-input>`).
    pub input: String,
}

/// Pure string renderer: `"# {input}\n{SAVING_MESSAGE}"`. Empty input → `""`.
#[must_use]
pub fn render_memory_to_string(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    format!("# {input}\n{SAVING_MESSAGE}")
}

/// iocraft component.
#[component]
pub fn UserMemoryInputMessage(props: &UserMemoryInputProps) -> impl Into<AnyElement<'static>> {
    let head = format!("# {}", props.input);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: head, color: TuiTheme::USER)
            Text(content: SAVING_MESSAGE, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_empty() {
        assert_eq!(render_memory_to_string(""), "");
    }
}
```

- [ ] **Step 5: Write `plan.rs`**
```rust
//! `UserPlanMessage` — bordered "Plan to implement" + markdown body.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - header: `Plan to implement` (bold, color `planMode` → ASSISTANT)
//!   - body: markdown (round-bordered box, borderColor `planMode`)
//!   source: claude-code/src/components/messages/UserPlanMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Exact header literal.
pub const HEADER: &str = "Plan to implement";

/// Props for [`UserPlanMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserPlanProps {
    /// Markdown plan content.
    pub plan_content: String,
}

/// Pure string renderer: header + blank line + plan body.
#[must_use]
pub fn render_plan_to_string(plan_content: &str) -> String {
    format!("{HEADER}\n{plan_content}")
}

/// iocraft component. Body routes through `render::markdown` once M7-01 lands
/// (see TODO); border is a `round`-style View.
#[component]
pub fn UserPlanMessage(props: &UserPlanProps) -> impl Into<AnyElement<'static>> {
    let body = props.plan_content.clone();
    // TODO(M7-01): render `body` via render::markdown into styled Text rows.
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: TuiTheme::ASSISTANT,
        ) {
            Text(content: HEADER, color: TuiTheme::ASSISTANT, weight: Weight::Bold)
            Text(content: body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_present() {
        assert!(render_plan_to_string("x").starts_with("Plan to implement"));
    }
}
```
Note: confirm iocraft 0.8.3 `View` accepts `border_style`/`border_color` (it does — the M6 permission dialogs use bordered views; cross-check `components/permissions/` if uncertain). If the attribute names differ, match what the permissions dialogs use.

- [ ] **Step 6: Register modules + both dispatch arms**

`messages/mod.rs`: `pub mod memory_input;` `pub mod plan;` and arms:
```rust
        RenderedMessage::UserMemoryInput { input } => {
            memory_input::render_memory_to_string(input)
        }
        RenderedMessage::UserPlan { plan_content } => {
            plan::render_plan_to_string(plan_content)
        }
```
`scrollback.rs`: imports + arms:
```rust
        RenderedMessage::UserMemoryInput { input } => element! {
            UserMemoryInputMessage(input: input)
        }
        .into_any(),
        RenderedMessage::UserPlan { plan_content } => element! {
            UserPlanMessage(plan_content: plan_content)
        }
        .into_any(),
```

- [ ] **Step 7: Run tests, accept snapshots, commit**

Run: `cargo test -p lingxi-tui --test render_user_misc memory plan` → PASS unit asserts; `cargo insta accept`; re-run PASS.
```bash
git add lingxi-code/crates/tui/src/state.rs lingxi-code/crates/tui/src/components/messages/memory_input.rs lingxi-code/crates/tui/src/components/messages/plan.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/components/scrollback.rs lingxi-code/crates/tui/tests/render_user_misc.rs lingxi-code/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-05 T4): UserMemoryInput + UserPlan renderers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `UserPromptMessage` (head+tail truncation)

**Files:**
- Modify: `state.rs`, `messages/mod.rs`, `scrollback.rs`
- Create: `messages/prompt.rs`
- Test: `lingxi-code/crates/tui/tests/render_user_misc.rs`

**Literal lock** (claude-code `UserPromptMessage.tsx`): renders the user prompt text (color `text`, bg `userMessageBackground`). The KAIROS/brief-layout branches are M8 team-mode scope — M7-05 implements the plain path only. The truncation constants are load-bearing: `MAX_DISPLAY_CHARS = 10_000`, `TRUNCATE_HEAD_CHARS = 2_500`, `TRUNCATE_TAIL_CHARS = 2_500`. When `text.len() > MAX_DISPLAY_CHARS`, display `"{head}\n… +{hiddenLines} lines …\n{tail}"` where head = first 2500 chars, tail = last 2500 chars, and `hiddenLines = countNewlines(text up to head boundary) - countNewlines(tail)`. The ellipsis marker is exactly `… +{N} lines …` (U+2026 ellipsis, surrounding spaces). Empty text → render nothing.

- [ ] **Step 1: Failing test**

Append to `render_user_misc.rs`:
```rust
use lingxi_tui::components::messages::prompt::{render_prompt_to_string, MAX_DISPLAY_CHARS};

#[test]
fn short_prompt_unchanged() {
    assert_eq!(render_prompt_to_string("hello"), "hello");
}

#[test]
fn long_prompt_truncates_head_tail() {
    let body = "x".repeat(MAX_DISPLAY_CHARS + 100);
    let s = render_prompt_to_string(&body);
    assert!(s.contains("… +"), "expected ellipsis marker, got len {}", s.len());
    assert!(s.contains(" lines …"));
    assert!(s.len() < body.len());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-tui --test render_user_misc prompt`
Expected: FAIL — unresolved `prompt` module.

- [ ] **Step 3: Add variant**

In `state.rs`:
```rust
    /// (M7-05) A user prompt echoed into scrollback. Long bodies are
    /// head+tail truncated (claude-code parity, 10k char cap).
    UserPrompt {
        /// The prompt body text.
        text: String,
    },
```

- [ ] **Step 4: Write `prompt.rs`**
```rust
//! `UserPromptMessage` — prompt text with head+tail truncation.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - MAX_DISPLAY_CHARS = 10_000, TRUNCATE_HEAD_CHARS = TRUNCATE_TAIL_CHARS = 2_500
//!   - truncated form: `{head}\n… +{hiddenLines} lines …\n{tail}`
//!   - SCOPE: plain path only; KAIROS/brief-layout defers to M8.
//!   source: claude-code/src/components/messages/UserPromptMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Hard cap on displayed prompt text.
pub const MAX_DISPLAY_CHARS: usize = 10_000;
/// Head slice kept on truncation.
pub const TRUNCATE_HEAD_CHARS: usize = 2_500;
/// Tail slice kept on truncation.
pub const TRUNCATE_TAIL_CHARS: usize = 2_500;

/// Props for [`UserPromptMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserPromptProps {
    /// Prompt body text.
    pub text: String,
}

/// Count occurrences of `\n` in the first `up_to` chars of `s`.
fn count_newlines_in_prefix(s: &str, up_to: usize) -> usize {
    s.chars().take(up_to).filter(|&c| c == '\n').count()
}

/// Pure string renderer with head+tail truncation. Operates on `char`
/// boundaries to stay UTF-8 safe.
#[must_use]
pub fn render_prompt_to_string(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= MAX_DISPLAY_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(TRUNCATE_HEAD_CHARS).collect();
    let tail: String = text
        .chars()
        .skip(char_count.saturating_sub(TRUNCATE_TAIL_CHARS))
        .collect();
    let hidden_lines = count_newlines_in_prefix(text, TRUNCATE_HEAD_CHARS)
        .saturating_sub(tail.matches('\n').count());
    format!("{head}\n… +{hidden_lines} lines …\n{tail}")
}

/// iocraft component. Empty text → empty view.
#[component]
pub fn UserPromptMessage(props: &UserPromptProps) -> impl Into<AnyElement<'static>> {
    let content = render_prompt_to_string(&props.text);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: content, color: TuiTheme::USER)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_empty() {
        assert_eq!(render_prompt_to_string(""), "");
    }
}
```

- [ ] **Step 5: Register module + both dispatch arms**

`messages/mod.rs`: `pub mod prompt;` and:
```rust
        RenderedMessage::UserPrompt { text } => prompt::render_prompt_to_string(text),
```
`scrollback.rs`: import + arm:
```rust
        RenderedMessage::UserPrompt { text } => element! {
            UserPromptMessage(text: text)
        }
        .into_any(),
```

- [ ] **Step 6: Run, accept snapshot (none new here), commit**

Run: `cargo test -p lingxi-tui --test render_user_misc prompt` → PASS.
```bash
git add lingxi-code/crates/tui/src/state.rs lingxi-code/crates/tui/src/components/messages/prompt.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/components/scrollback.rs lingxi-code/crates/tui/tests/render_user_misc.rs
git commit -m "$(cat <<'EOF'
plan(M7-05 T5): UserPromptMessage renderer with head+tail truncation

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `UserResourceUpdateMessage` + `UserImageMessage`

**Files:**
- Modify: `state.rs`, `messages/mod.rs`, `scrollback.rs`
- Create: `messages/resource_update.rs`, `messages/image.rs`
- Test: `lingxi-code/crates/tui/tests/render_user_misc.rs`

**Literal locks:**
- `UserResourceUpdateMessage.tsx`: each update line = `↻` (REFRESH_ARROW U+21BB, color `success` → `TuiTheme::ASSISTANT`) + ` ` + `{server}:` (dim) + ` ` + `{target}` (color `suggestion` → `TuiTheme::USER`) + optional ` · {reason}` (dim). For resource kind, `target` is a formatted URI: `file://` URIs show the basename; other URIs longer than 40 chars are truncated to 39 chars + `…` (U+2026). For polling kind, `target` is the tool name verbatim. M7-05 receives updates already parsed into `(server, target, reason)` triples — implement `format_uri` but the engine-side XML parse is out of scope (the data arrives structured). The separator between server:target and reason is ` · ` (space, U+00B7 middle dot, space).
- `UserImageMessage.tsx`: label = `[Image #{imageId}]` when an id is present, else `[Image]`. M7-05 renders the **placeholder label + optional metadata** only (e.g. dims/name if available as a `metadata: Option<String>` suffix). NO inline image display, NO hyperlink (terminal image protocols + hyperlink support are M8). Document the deferral.

- [ ] **Step 1: Failing tests**

Append to `render_user_misc.rs`:
```rust
use lingxi_tui::components::messages::image::{render_image_label, UserImageMessage};
use lingxi_tui::components::messages::resource_update::{
    format_uri, render_resource_update_to_string, ResourceUpdate, UserResourceUpdateMessage,
};

#[test]
fn image_label_with_and_without_id() {
    assert_eq!(render_image_label(Some(3), None), "[Image #3]");
    assert_eq!(render_image_label(None, None), "[Image]");
    assert_eq!(render_image_label(Some(1), Some("800x600")), "[Image #1] (800x600)");
}

#[test]
fn format_uri_file_shows_basename() {
    assert_eq!(format_uri("file:///a/b/c.rs"), "c.rs");
}

#[test]
fn format_uri_long_truncates() {
    let long = format!("https://{}", "x".repeat(60));
    let out = format_uri(&long);
    assert!(out.ends_with('…'));
    assert_eq!(out.chars().count(), 40);
}

#[test]
fn resource_update_line() {
    let u = ResourceUpdate { server: "fs".into(), target: "x.rs".into(), reason: Some("changed".into()) };
    assert_eq!(render_resource_update_to_string(&[u]), "↻ fs: x.rs · changed");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-tui --test render_user_misc image resource`
Expected: FAIL.

- [ ] **Step 3: Add variants**

In `state.rs`:
```rust
    /// (M7-05) MCP resource/polling update lines (`↻ server: target · reason`).
    UserResourceUpdate {
        /// Parsed update triples (server, target, optional reason).
        updates: Vec<(String, String, Option<String>)>,
    },
    /// (M7-05) Image attachment placeholder. Terminal image protocols are M8;
    /// this renders `[Image #N]`/`[Image]` + optional metadata only.
    UserImage {
        /// Stored image id, if any (drives `#N` suffix).
        image_id: Option<u64>,
        /// Optional metadata suffix (dims/name) shown in parens.
        metadata: Option<String>,
    },
```

- [ ] **Step 4: Write `resource_update.rs`**
```rust
//! `UserResourceUpdateMessage` — `↻ server: target · reason` lines.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - glyph: `↻` (REFRESH_ARROW U+21BB, color `success` → ASSISTANT)
//!   - line: `↻ {server}: {target}[ · {reason}]`
//!   - file:// URIs → basename; long URIs → 39 chars + `…`
//!   source: claude-code/src/components/messages/UserResourceUpdateMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Refresh-arrow glyph (U+21BB).
pub const REFRESH_ARROW: &str = "↻";

/// One parsed update.
#[derive(Debug, Clone)]
pub struct ResourceUpdate {
    /// MCP server name.
    pub server: String,
    /// Resource URI (resource kind) or tool name (polling kind).
    pub target: String,
    /// Optional human-readable reason.
    pub reason: Option<String>,
}

/// Format a URI for display: `file://` → basename; len>40 → 39 chars + `…`.
#[must_use]
pub fn format_uri(uri: &str) -> String {
    if let Some(path) = uri.strip_prefix("file://") {
        return path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(path)
            .to_string();
    }
    if uri.chars().count() > 40 {
        let head: String = uri.chars().take(39).collect();
        return format!("{head}…");
    }
    uri.to_string()
}

/// Pure string renderer: one line per update, joined by `\n`.
#[must_use]
pub fn render_resource_update_to_string(updates: &[ResourceUpdate]) -> String {
    updates
        .iter()
        .map(|u| {
            let mut line = format!("{REFRESH_ARROW} {}: {}", u.server, u.target);
            if let Some(r) = &u.reason {
                line.push_str(&format!(" · {r}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Props for [`UserResourceUpdateMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserResourceUpdateProps {
    /// `(server, target, reason)` triples.
    pub updates: Vec<(String, String, Option<String>)>,
}

/// iocraft component.
#[component]
pub fn UserResourceUpdateMessage(
    props: &UserResourceUpdateProps,
) -> impl Into<AnyElement<'static>> {
    let updates: Vec<ResourceUpdate> = props
        .updates
        .iter()
        .map(|(s, t, r)| ResourceUpdate {
            server: s.clone(),
            target: t.clone(),
            reason: r.clone(),
        })
        .collect();
    let body = render_resource_update_to_string(&updates);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reason_no_dot() {
        let u = ResourceUpdate { server: "s".into(), target: "t".into(), reason: None };
        assert_eq!(render_resource_update_to_string(&[u]), "↻ s: t");
    }
}
```

- [ ] **Step 5: Write `image.rs`**
```rust
//! `UserImageMessage` — `[Image #N]` / `[Image]` placeholder + metadata.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - label: `[Image #{imageId}]` with id, else `[Image]`
//!   - SCOPE: placeholder only — inline terminal image display + hyperlinks
//!     are M8 (terminal-protocol cluster). Metadata suffix is a LingXi
//!     addition for the headless placeholder, shown in parens.
//!   source: claude-code/src/components/messages/UserImageMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Build the placeholder label (+ optional metadata suffix).
#[must_use]
pub fn render_image_label(image_id: Option<u64>, metadata: Option<&str>) -> String {
    let base = match image_id {
        Some(id) => format!("[Image #{id}]"),
        None => "[Image]".to_string(),
    };
    match metadata {
        Some(m) if !m.is_empty() => format!("{base} ({m})"),
        _ => base,
    }
}

/// Props for [`UserImageMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserImageProps {
    /// Stored image id (drives `#N`).
    pub image_id: Option<u64>,
    /// Optional metadata suffix.
    pub metadata: Option<String>,
}

/// iocraft component.
#[component]
pub fn UserImageMessage(props: &UserImageProps) -> impl Into<AnyElement<'static>> {
    let label = render_image_label(props.image_id, props.metadata.as_deref());
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: label, color: TuiTheme::USER)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_label() {
        assert_eq!(render_image_label(None, None), "[Image]");
    }
}
```

- [ ] **Step 6: Register modules + both dispatch arms**

`messages/mod.rs`: `pub mod resource_update;` `pub mod image;` and arms:
```rust
        RenderedMessage::UserResourceUpdate { updates } => {
            let parsed: Vec<resource_update::ResourceUpdate> = updates
                .iter()
                .map(|(s, t, r)| resource_update::ResourceUpdate {
                    server: s.clone(),
                    target: t.clone(),
                    reason: r.clone(),
                })
                .collect();
            resource_update::render_resource_update_to_string(&parsed)
        }
        RenderedMessage::UserImage { image_id, metadata } => {
            image::render_image_label(*image_id, metadata.as_deref())
        }
```
`scrollback.rs`: imports + arms:
```rust
        RenderedMessage::UserResourceUpdate { updates } => element! {
            UserResourceUpdateMessage(updates: updates)
        }
        .into_any(),
        RenderedMessage::UserImage { image_id, metadata } => element! {
            UserImageMessage(image_id: image_id, metadata: metadata)
        }
        .into_any(),
```

- [ ] **Step 7: Run tests, accept snapshots, commit**

Run: `cargo test -p lingxi-tui --test render_user_misc image resource` → PASS; add `#[test]` snapshot for each component (`image_label`, `resource_lines`) and `cargo insta accept`.
```bash
git add lingxi-code/crates/tui/src/state.rs lingxi-code/crates/tui/src/components/messages/resource_update.rs lingxi-code/crates/tui/src/components/messages/image.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/components/scrollback.rs lingxi-code/crates/tui/tests/render_user_misc.rs lingxi-code/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-05 T6): UserResourceUpdate + UserImage (placeholder) renderers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `AttachmentMessage` (solo-user `Line` summaries)

**Files:**
- Modify: `state.rs`, `messages/mod.rs`, `scrollback.rs`
- Create: `messages/attachment.rs`
- Test: `lingxi-code/crates/tui/tests/render_user_attachment.rs`

**Literal locks** (claude-code `AttachmentMessage.tsx` — the `Line` helper is a dim 2-space-gutter row; bold spans wrap the path/count). M7-05 implements the solo-user attachment kinds via a single `Attachment` enum; team/swarm/hook kinds defer to M8 (document). Exact literals per kind:

| Kind | Rendered line |
|---|---|
| `Directory { display_path }` | `Listed directory {display_path}{sep}` (sep = `/`; path is bold) |
| `File { display_path, num_lines, truncated }` | `Read {display_path} ({num_lines}{+} lines)` (`+` only when truncated) |
| `CompactFileReference { display_path }` | `Referenced file {display_path}` |
| `PdfReference { display_path, page_count }` | `Referenced PDF {display_path} ({page_count} pages)` |
| `SelectedLines { count, display_path, ide_name }` | `⧉ Selected {count} lines from {display_path} in {ide_name}` |
| `NestedMemory { display_path }` | `Loaded {display_path}` |
| `McpResource { name, server }` | `Read MCP resource {name} from {server}` |
| `PlanFileReference { plan_file_path }` | `Plan file referenced ({plan_file_path})` |
| `InvokedSkills { skill_names }` | `Skills restored ({comma_joined_names})` |

Glyph `⧉` = U+29C9. M7-05 renders the full `Line` as dim text (bold spans collapse to plain in the string form; the iocraft component may bold the path). All kinds render under a `Line` (dim, 2-space gutter — but claude-code's `Line` here has no leading gutter glyph; it is just dim text, optionally `color="error"`/`"warning"`). Keep the string form gutter-free to match `Line`.

- [ ] **Step 1: Failing tests**

In `lingxi-code/crates/tui/tests/render_user_attachment.rs`:
```rust
use iocraft::prelude::*;
use lingxi_tui::components::messages::attachment::{render_attachment_to_string, Attachment, AttachmentMessage};

#[test]
fn directory_line() {
    let a = Attachment::Directory { display_path: "src".into() };
    assert_eq!(render_attachment_to_string(&a), "Listed directory src/");
}

#[test]
fn file_line_truncated() {
    let a = Attachment::File { display_path: "a.rs".into(), num_lines: 120, truncated: true };
    assert_eq!(render_attachment_to_string(&a), "Read a.rs (120+ lines)");
}

#[test]
fn file_line_untruncated() {
    let a = Attachment::File { display_path: "a.rs".into(), num_lines: 10, truncated: false };
    assert_eq!(render_attachment_to_string(&a), "Read a.rs (10 lines)");
}

#[test]
fn selected_lines() {
    let a = Attachment::SelectedLines { count: 3, display_path: "x.rs".into(), ide_name: "VSCode".into() };
    assert_eq!(render_attachment_to_string(&a), "⧉ Selected 3 lines from x.rs in VSCode");
}

#[test]
fn attachment_snapshot() {
    let a = Attachment::PdfReference { display_path: "doc.pdf".into(), page_count: 5 };
    let mut e = element! { AttachmentMessage(attachment: a) };
    insta::assert_snapshot!("attachment_pdf", e.to_string());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-tui --test render_user_attachment`
Expected: FAIL.

- [ ] **Step 3: Add variant**

In `state.rs`:
```rust
    /// (M7-05) A non-tool attachment summary line (directory listing, file
    /// read, PDF/resource reference, etc.). Carries the parsed attachment
    /// kind; team/swarm/hook kinds defer to M8.
    Attachment {
        /// The parsed attachment kind.
        attachment: crate::components::messages::attachment::Attachment,
    },
```
(Define `Attachment` in the renderer file and re-export it; the variant references it by path.)

- [ ] **Step 4: Write `attachment.rs`**
```rust
//! `AttachmentMessage` — solo-user attachment summary lines.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - "Listed directory {path}/", "Read {path} ({n}[+] lines)",
//!     "Referenced file {path}", "Referenced PDF {path} ({n} pages)",
//!     "⧉ Selected {n} lines from {path} in {ide}", "Loaded {path}",
//!     "Read MCP resource {name} from {server}",
//!     "Plan file referenced ({path})", "Skills restored ({names})"
//!   - SCOPE: solo-user kinds only; team/swarm/hook/diagnostics kinds → M8.
//!   source: claude-code/src/components/messages/AttachmentMessage.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// `⧉` selected-lines glyph (U+29C9).
pub const SELECTED_GLYPH: &str = "⧉";

/// A solo-user attachment kind.
#[derive(Debug, Clone)]
pub enum Attachment {
    /// Directory listing.
    Directory { display_path: String },
    /// File read.
    File { display_path: String, num_lines: u64, truncated: bool },
    /// Compact file reference.
    CompactFileReference { display_path: String },
    /// PDF reference.
    PdfReference { display_path: String, page_count: u64 },
    /// IDE-selected lines.
    SelectedLines { count: u64, display_path: String, ide_name: String },
    /// Nested memory file loaded.
    NestedMemory { display_path: String },
    /// MCP resource read.
    McpResource { name: String, server: String },
    /// Plan file referenced.
    PlanFileReference { plan_file_path: String },
    /// Skills restored.
    InvokedSkills { skill_names: Vec<String> },
}

/// Pure string renderer for one attachment line.
#[must_use]
pub fn render_attachment_to_string(a: &Attachment) -> String {
    match a {
        Attachment::Directory { display_path } => format!("Listed directory {display_path}/"),
        Attachment::File { display_path, num_lines, truncated } => {
            let plus = if *truncated { "+" } else { "" };
            format!("Read {display_path} ({num_lines}{plus} lines)")
        }
        Attachment::CompactFileReference { display_path } => {
            format!("Referenced file {display_path}")
        }
        Attachment::PdfReference { display_path, page_count } => {
            format!("Referenced PDF {display_path} ({page_count} pages)")
        }
        Attachment::SelectedLines { count, display_path, ide_name } => {
            format!("{SELECTED_GLYPH} Selected {count} lines from {display_path} in {ide_name}")
        }
        Attachment::NestedMemory { display_path } => format!("Loaded {display_path}"),
        Attachment::McpResource { name, server } => {
            format!("Read MCP resource {name} from {server}")
        }
        Attachment::PlanFileReference { plan_file_path } => {
            format!("Plan file referenced ({plan_file_path})")
        }
        Attachment::InvokedSkills { skill_names } => {
            format!("Skills restored ({})", skill_names.join(", "))
        }
    }
}

/// Props for [`AttachmentMessage`].
#[derive(Debug, Clone, Props)]
pub struct AttachmentProps {
    /// The attachment to render.
    pub attachment: Attachment,
}

impl Default for AttachmentProps {
    fn default() -> Self {
        Self { attachment: Attachment::Directory { display_path: String::new() } }
    }
}

/// iocraft component (dim `Line`).
#[component]
pub fn AttachmentMessage(props: &AttachmentProps) -> impl Into<AnyElement<'static>> {
    let line = render_attachment_to_string(&props.attachment);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: line, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoked_skills_join() {
        let a = Attachment::InvokedSkills { skill_names: vec!["a".into(), "b".into()] };
        assert_eq!(render_attachment_to_string(&a), "Skills restored (a, b)");
    }
}
```

- [ ] **Step 5: Register module + both dispatch arms**

`messages/mod.rs`: `pub mod attachment;` and:
```rust
        RenderedMessage::Attachment { attachment } => {
            attachment::render_attachment_to_string(attachment)
        }
```
`scrollback.rs`: import `use crate::components::messages::attachment::AttachmentMessage;` and:
```rust
        RenderedMessage::Attachment { attachment } => element! {
            AttachmentMessage(attachment: attachment)
        }
        .into_any(),
```
Note: `Attachment` has no `Copy`/`Default`-friendly auto-derive issue for the `Props` macro — `AttachmentProps` derives `Clone` + a manual `Default` (above) because iocraft `Props` requires `Default`. Verify the `Props` derive compiles; if it requires `Default` on the field type too, the manual impl covers it.

- [ ] **Step 6: Run tests, accept snapshot, commit**

Run: `cargo test -p lingxi-tui --test render_user_attachment` → PASS; `cargo insta accept`.
```bash
git add lingxi-code/crates/tui/src/state.rs lingxi-code/crates/tui/src/components/messages/attachment.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/components/scrollback.rs lingxi-code/crates/tui/tests/render_user_attachment.rs lingxi-code/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-05 T7): AttachmentMessage solo-user summary renderer

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `GroupedToolUseContent` (same-tool folding)

**Files:**
- Modify: `state.rs`, `messages/mod.rs`, `scrollback.rs`
- Create: `messages/grouped_tool_use.rs`
- Test: `lingxi-code/crates/tui/tests/render_folding.rs`

**Folding behavior** (claude-code `GroupedToolUseContent.tsx`): groups consecutive tool-use blocks of the **same tool name** and renders them as a unit. M7-05 reproduces the fold structure:
- The variant carries `tool: String`, `group_id: ToolUseId` (the first child's id, used as the `expanded` map key), and `entries: Vec<(serde_json::Value, serde_json::Value)>` — `(input, result)` pairs.
- **Collapsed** (`expanded == false`): one summary line `● {tool} (×{N})` (use `TuiTheme::ASSISTANT`, dot marker `● ` matching `assistant_text.rs`). When `N == 1`, drop the `(×1)` and render as a normal single tool line `● {tool}`.
- **Expanded** (`expanded == true`): the summary header line, then each entry rendered as a child pair using the existing string shapes — call `render_assistant_tool_use_to_string` for the input line and `render_user_tool_result_to_string` for the result line (import both from their modules). Indent children by two spaces.
- The `focused`/`expanded` state comes from `AppState.expanded.get(&group_id)` (same mechanism as M6-04 tool blocks).

- [ ] **Step 1: Failing folding tests**

In `lingxi-code/crates/tui/tests/render_folding.rs`:
```rust
use lingxi_protocol::ToolUseId;
use lingxi_tui::components::messages::grouped_tool_use::render_grouped_to_string;

fn pair(p: &str) -> (serde_json::Value, serde_json::Value) {
    (serde_json::json!({"file_path": p}), serde_json::json!({"content": "ok"}))
}

#[test]
fn grouped_collapsed_shows_count() {
    let entries = vec![pair("a.rs"), pair("b.rs"), pair("c.rs")];
    let s = render_grouped_to_string("Read", &entries, /*expanded*/ false);
    assert_eq!(s, "● Read (×3)");
}

#[test]
fn grouped_single_drops_count() {
    let entries = vec![pair("a.rs")];
    let s = render_grouped_to_string("Read", &entries, false);
    assert_eq!(s, "● Read");
}

#[test]
fn grouped_expanded_lists_children() {
    let entries = vec![pair("a.rs"), pair("b.rs")];
    let s = render_grouped_to_string("Read", &entries, true);
    assert!(s.starts_with("● Read (×2)"), "header missing: {s}");
    assert!(s.matches('\n').count() >= 2, "expected child lines: {s}");
    let _ = ToolUseId::new(); // id type is in scope for the variant
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-tui --test render_folding grouped`
Expected: FAIL.

- [ ] **Step 3: Add variant**

In `state.rs`:
```rust
    /// (M7-05) A fold of consecutive same-tool tool-use blocks. Collapsed →
    /// `● {tool} (×N)`; expanded → header + each child input/result pair.
    /// `group_id` (the first child's id) keys `AppState.expanded`.
    GroupedToolUse {
        /// Shared tool name for the group.
        tool: String,
        /// First child's id — the per-group expanded-map key.
        group_id: lingxi_protocol::ToolUseId,
        /// `(input, result)` pairs in group order.
        entries: Vec<(serde_json::Value, serde_json::Value)>,
    },
```

- [ ] **Step 4: Write `grouped_tool_use.rs`**
```rust
//! `GroupedToolUseContent` — folds consecutive same-tool tool-use blocks.
//!
//! Folding (from claude-code GroupedToolUseContent.tsx):
//!   - collapsed: `● {tool} (×N)`  (drops `(×1)` for a single entry)
//!   - expanded: header + each child input line + result line, indented 2sp
//!   source: claude-code/src/components/messages/GroupedToolUseContent.tsx
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;
use lingxi_protocol::ToolUseId;

use crate::components::messages::assistant_tool_use::{
    render_assistant_tool_use_to_string, AssistantToolUseProps,
};
use crate::components::messages::user_tool_result::{
    render_user_tool_result_to_string, UserToolResultProps,
};
use crate::theme::TuiTheme;

/// Dot marker prefix (matches `assistant_text.rs`).
pub const MARKER: &str = "● ";
/// Child indent (2 spaces).
pub const INDENT: &str = "  ";

/// Pure string renderer.
#[must_use]
pub fn render_grouped_to_string(
    tool: &str,
    entries: &[(serde_json::Value, serde_json::Value)],
    expanded: bool,
) -> String {
    let n = entries.len();
    let header = if n <= 1 {
        format!("{MARKER}{tool}")
    } else {
        format!("{MARKER}{tool} (×{n})")
    };
    if !expanded {
        return header;
    }
    let mut out = header;
    for (input, result) in entries {
        let in_line = render_assistant_tool_use_to_string(AssistantToolUseProps {
            id: ToolUseId::new(),
            tool: tool.to_string(),
            input: input.clone(),
            expanded: false,
            focused: false,
        });
        let res_line = render_user_tool_result_to_string(UserToolResultProps {
            id: ToolUseId::new(),
            tool: tool.to_string(),
            result: result.clone(),
            expanded: false,
            focused: false,
        });
        for line in in_line.lines().chain(res_line.lines()) {
            out.push('\n');
            out.push_str(INDENT);
            out.push_str(line);
        }
    }
    out
}

/// Props for [`GroupedToolUseContent`].
#[derive(Debug, Clone, Default, Props)]
pub struct GroupedToolUseProps {
    /// Shared tool name.
    pub tool: String,
    /// `(input, result)` pairs.
    pub entries: Vec<(serde_json::Value, serde_json::Value)>,
    /// Expanded state (from `AppState.expanded`).
    pub expanded: bool,
}

/// iocraft component.
#[component]
pub fn GroupedToolUseContent(props: &GroupedToolUseProps) -> impl Into<AnyElement<'static>> {
    let body = render_grouped_to_string(&props.tool, &props.entries, props.expanded);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::ASSISTANT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_group_no_count() {
        assert_eq!(render_grouped_to_string("Bash", &[], false), "● Bash");
    }
}
```

- [ ] **Step 5: Register module + both dispatch arms**

`messages/mod.rs`: `pub mod grouped_tool_use;` and:
```rust
        RenderedMessage::GroupedToolUse { tool, entries, .. } => {
            grouped_tool_use::render_grouped_to_string(tool, entries, expanded)
        }
```
(Note: this dispatcher already receives `expanded: bool` as a parameter — reuse it.)
`scrollback.rs`: import `use crate::components::messages::grouped_tool_use::GroupedToolUseContent;` and:
```rust
        RenderedMessage::GroupedToolUse { tool, group_id, entries } => {
            let is_expanded = expanded.get(&group_id).copied().unwrap_or(false);
            element! {
                GroupedToolUseContent(tool: tool, entries: entries, expanded: is_expanded)
            }
            .into_any()
        }
```

- [ ] **Step 6: Run tests, commit**

Run: `cargo test -p lingxi-tui --test render_folding grouped` → PASS.
```bash
git add lingxi-code/crates/tui/src/state.rs lingxi-code/crates/tui/src/components/messages/grouped_tool_use.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/components/scrollback.rs lingxi-code/crates/tui/tests/render_folding.rs
git commit -m "$(cat <<'EOF'
plan(M7-05 T8): GroupedToolUseContent same-tool folding renderer

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `CollapsedReadSearchContent` (Read/Search/List count folding)

**Files:**
- Modify: `state.rs`, `messages/mod.rs`, `scrollback.rs`
- Create: `messages/collapsed_read_search.rs`
- Test: `lingxi-code/crates/tui/tests/render_folding.rs`

**Folding behavior** (claude-code `CollapsedReadSearchContent.tsx`, non-verbose path). M7-05 scope = read/search/list counts (the solo-user core; git/PR/bash/mcp/memory/team parts defer to M8). Build a comma-joined summary line under the `  ⎿  ` gutter:
- Verbs (active vs finalized) and nouns — exact literals:
  - search: active `Searching for` / finalized `Searched for`, noun `pattern`/`patterns`
  - read: active `Reading` / finalized `Read`, noun `file`/`files`
  - list: active `Listing` / finalized `Listed`, noun `directory`/`directories`
- Order: search, then read, then list (matches claude-code's nonMemParts order for the M7-05 subset).
- First part is **capitalized** (first letter upper); subsequent parts lowercase, comma+space joined.
- Collapsed → just the summary line. Expanded → summary line + one `  ⎿  ` row per folded tool-use entry (entry display strings carried on the variant).
- If all counts are 0, render nothing (defensive — claude-code returns null).

The variant carries the three counts, an `is_active` flag (active = group still streaming), `group_id` (expanded key), and `entries: Vec<String>` (one display line per folded tool use, used in expanded mode).

- [ ] **Step 1: Failing folding tests (collapsed + expanded + pluralization)**

Append to `render_folding.rs`:
```rust
use lingxi_tui::components::messages::collapsed_read_search::{
    render_collapsed_to_string, CollapsedCounts,
};

#[test]
fn collapsed_finalized_summary() {
    let c = CollapsedCounts { search: 2, read: 1, list: 0, is_active: false };
    let s = render_collapsed_to_string(&c, &[], /*expanded*/ false);
    assert_eq!(s, "  ⎿  Searched for 2 patterns, read 1 file");
}

#[test]
fn collapsed_active_present_tense() {
    let c = CollapsedCounts { search: 0, read: 3, list: 0, is_active: true };
    let s = render_collapsed_to_string(&c, &[], false);
    assert_eq!(s, "  ⎿  Reading 3 files");
}

#[test]
fn collapsed_list_directories_plural() {
    let c = CollapsedCounts { search: 0, read: 0, list: 2, is_active: false };
    assert_eq!(render_collapsed_to_string(&c, &[], false), "  ⎿  Listed 2 directories");
}

#[test]
fn collapsed_zero_counts_empty() {
    let c = CollapsedCounts { search: 0, read: 0, list: 0, is_active: false };
    assert_eq!(render_collapsed_to_string(&c, &[], false), "");
}

#[test]
fn collapsed_expanded_lists_entries() {
    let c = CollapsedCounts { search: 0, read: 2, list: 0, is_active: false };
    let entries = vec!["a.rs".to_string(), "b.rs".to_string()];
    let s = render_collapsed_to_string(&c, &entries, true);
    assert!(s.starts_with("  ⎿  Read 2 files"), "header: {s}");
    assert!(s.contains("a.rs") && s.contains("b.rs"), "entries: {s}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-tui --test render_folding collapsed`
Expected: FAIL.

- [ ] **Step 3: Add variant**

In `state.rs`:
```rust
    /// (M7-05) A fold of Read/Search/List tool runs into one count summary.
    /// Scope: read/search/list counts (git/PR/bash/mcp/memory parts → M8).
    CollapsedReadSearch {
        /// Number of search (Grep/Glob) tool uses.
        search_count: u64,
        /// Number of file reads.
        read_count: u64,
        /// Number of directory listings.
        list_count: u64,
        /// `true` while the group is still streaming (present-tense verbs).
        is_active: bool,
        /// Expanded-map key (first child's id).
        group_id: lingxi_protocol::ToolUseId,
        /// Per-entry display lines, shown when expanded.
        entries: Vec<String>,
    },
```

- [ ] **Step 4: Write `collapsed_read_search.rs`**
```rust
//! `CollapsedReadSearchContent` — folds Read/Search/List runs into a count line.
//!
//! Folding (from claude-code CollapsedReadSearchContent.tsx, non-verbose):
//!   - gutter `  ⎿  ` + comma-joined parts; first part capitalized
//!   - verbs: Searching for/Searched for, Reading/Read, Listing/Listed
//!   - nouns: pattern(s), file(s), directory/directories
//!   - all counts 0 → render nothing
//!   - SCOPE: read/search/list only; git/PR/bash/mcp/memory/team → M8;
//!     live `⤿` progress hint + min-display debounce → M8.
//!   source: claude-code/src/components/messages/CollapsedReadSearchContent.tsx

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Dim gutter for the summary + each expanded entry row.
pub const GUTTER: &str = "  ⎿  ";

/// The three M7-05 counts + active flag.
#[derive(Debug, Clone, Default)]
pub struct CollapsedCounts {
    /// Search (Grep/Glob) tool uses.
    pub search: u64,
    /// File reads.
    pub read: u64,
    /// Directory listings.
    pub list: u64,
    /// `true` → present-tense verbs.
    pub is_active: bool,
}

fn cap_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Build the comma-joined summary (without the gutter). Empty when all 0.
#[must_use]
pub fn render_summary(c: &CollapsedCounts) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.search > 0 {
        let verb = if c.is_active { "searching for" } else { "searched for" };
        let noun = if c.search == 1 { "pattern" } else { "patterns" };
        parts.push(format!("{verb} {} {noun}", c.search));
    }
    if c.read > 0 {
        let verb = if c.is_active { "reading" } else { "read" };
        let noun = if c.read == 1 { "file" } else { "files" };
        parts.push(format!("{verb} {} {noun}", c.read));
    }
    if c.list > 0 {
        let verb = if c.is_active { "listing" } else { "listed" };
        let noun = if c.list == 1 { "directory" } else { "directories" };
        parts.push(format!("{verb} {} {noun}", c.list));
    }
    if parts.is_empty() {
        return String::new();
    }
    // Capitalize the first part only.
    let first = cap_first(&parts[0]);
    let mut joined = first;
    for p in &parts[1..] {
        joined.push_str(", ");
        joined.push_str(p);
    }
    joined
}

/// Pure string renderer: gutter + summary; expanded → + indented entry rows.
#[must_use]
pub fn render_collapsed_to_string(
    c: &CollapsedCounts,
    entries: &[String],
    expanded: bool,
) -> String {
    let summary = render_summary(c);
    if summary.is_empty() {
        return String::new();
    }
    let mut out = format!("{GUTTER}{summary}");
    if expanded {
        for e in entries {
            out.push('\n');
            out.push_str(GUTTER);
            out.push_str(e);
        }
    }
    out
}

/// Props for [`CollapsedReadSearchContent`].
#[derive(Debug, Clone, Default, Props)]
pub struct CollapsedReadSearchProps {
    /// search/read/list counts + active flag.
    pub counts: CollapsedCounts,
    /// Per-entry display lines (expanded mode).
    pub entries: Vec<String>,
    /// Expanded state (from `AppState.expanded`).
    pub expanded: bool,
}

/// iocraft component.
#[component]
pub fn CollapsedReadSearchContent(
    props: &CollapsedReadSearchProps,
) -> impl Into<AnyElement<'static>> {
    let body = render_collapsed_to_string(&props.counts, &props.entries, props.expanded);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pattern_singular() {
        let c = CollapsedCounts { search: 1, read: 0, list: 0, is_active: false };
        assert_eq!(render_collapsed_to_string(&c, &[], false), "  ⎿  Searched for 1 pattern");
    }
}
```

- [ ] **Step 5: Register module + both dispatch arms**

`messages/mod.rs`: `pub mod collapsed_read_search;` and:
```rust
        RenderedMessage::CollapsedReadSearch {
            search_count, read_count, list_count, is_active, entries, ..
        } => {
            let counts = collapsed_read_search::CollapsedCounts {
                search: *search_count,
                read: *read_count,
                list: *list_count,
                is_active: *is_active,
            };
            collapsed_read_search::render_collapsed_to_string(&counts, entries, expanded)
        }
```
`scrollback.rs`: import `use crate::components::messages::collapsed_read_search::{CollapsedCounts, CollapsedReadSearchContent};` and:
```rust
        RenderedMessage::CollapsedReadSearch {
            search_count, read_count, list_count, is_active, group_id, entries,
        } => {
            let is_expanded = expanded.get(&group_id).copied().unwrap_or(false);
            let counts = CollapsedCounts {
                search: search_count,
                read: read_count,
                list: list_count,
                is_active,
            };
            element! {
                CollapsedReadSearchContent(counts: counts, entries: entries, expanded: is_expanded)
            }
            .into_any()
        }
```

- [ ] **Step 6: Run tests, commit**

Run: `cargo test -p lingxi-tui --test render_folding collapsed` → PASS.
```bash
git add lingxi-code/crates/tui/src/state.rs lingxi-code/crates/tui/src/components/messages/collapsed_read_search.rs lingxi-code/crates/tui/src/components/messages/mod.rs lingxi-code/crates/tui/src/components/scrollback.rs lingxi-code/crates/tui/tests/render_folding.rs
git commit -m "$(cat <<'EOF'
plan(M7-05 T9): CollapsedReadSearchContent Read/Search/List folding

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Dispatch coverage tests for all 12 variants

**Files:**
- Test: `lingxi-code/crates/tui/tests/dispatch_user_renderers.rs`

**Why:** the two `match` arms compile-check exhaustiveness, but a dispatch test asserts each new variant actually *routes* through `render_entry_to_string` (string form) and produces the expected leading literal. This catches a wrong-arm copy-paste (e.g. `UserCommand` routed to `bash_input`'s renderer) that the compiler can't.

- [ ] **Step 1: Write the dispatch test (one assertion per variant)**

In `lingxi-code/crates/tui/tests/dispatch_user_renderers.rs`:
```rust
use lingxi_protocol::ToolUseId;
use lingxi_tui::components::messages::render_entry_to_string;
use lingxi_tui::components::messages::attachment::Attachment;
use lingxi_tui::state::RenderedMessage;

fn s(m: &RenderedMessage) -> String {
    render_entry_to_string(m, /*focused*/ false, /*expanded*/ false)
}

#[test]
fn dispatch_bash_input() {
    assert_eq!(s(&RenderedMessage::UserBashInput { command: "ls".into() }), "! ls");
}

#[test]
fn dispatch_bash_output() {
    assert_eq!(s(&RenderedMessage::UserBashOutput { stdout: "ok".into(), stderr: "".into() }), "ok");
}

#[test]
fn dispatch_command() {
    assert_eq!(s(&RenderedMessage::UserCommand { command: "help".into(), args: "".into(), is_skill: false }), "❯ /help");
}

#[test]
fn dispatch_local_command_output() {
    assert_eq!(s(&RenderedMessage::UserLocalCommandOutput { stdout: "".into(), stderr: "".into() }), "(no content)");
}

#[test]
fn dispatch_memory_input() {
    assert_eq!(s(&RenderedMessage::UserMemoryInput { input: "x".into() }), "# x\nGot it.");
}

#[test]
fn dispatch_plan() {
    assert!(s(&RenderedMessage::UserPlan { plan_content: "p".into() }).starts_with("Plan to implement"));
}

#[test]
fn dispatch_prompt() {
    assert_eq!(s(&RenderedMessage::UserPrompt { text: "hi".into() }), "hi");
}

#[test]
fn dispatch_resource_update() {
    let m = RenderedMessage::UserResourceUpdate {
        updates: vec![("fs".into(), "x.rs".into(), None)],
    };
    assert_eq!(s(&m), "↻ fs: x.rs");
}

#[test]
fn dispatch_image() {
    assert_eq!(s(&RenderedMessage::UserImage { image_id: Some(2), metadata: None }), "[Image #2]");
}

#[test]
fn dispatch_attachment() {
    let m = RenderedMessage::Attachment { attachment: Attachment::NestedMemory { display_path: "M.md".into() } };
    assert_eq!(s(&m), "Loaded M.md");
}

#[test]
fn dispatch_grouped_tool_use() {
    let m = RenderedMessage::GroupedToolUse {
        tool: "Read".into(),
        group_id: ToolUseId::new(),
        entries: vec![
            (serde_json::json!({}), serde_json::json!({"content": "a"})),
            (serde_json::json!({}), serde_json::json!({"content": "b"})),
        ],
    };
    assert_eq!(s(&m), "● Read (×2)");
}

#[test]
fn dispatch_collapsed_read_search() {
    let m = RenderedMessage::CollapsedReadSearch {
        search_count: 0, read_count: 1, list_count: 0, is_active: false,
        group_id: ToolUseId::new(), entries: vec![],
    };
    assert_eq!(s(&m), "  ⎿  Read 1 file");
}
```

- [ ] **Step 2: Run the dispatch tests**

Run (cwd `lingxi-code/`): `cargo test -p lingxi-tui --test dispatch_user_renderers`
Expected: PASS (all 12). If any FAILs, the dispatch arm for that variant routes to the wrong renderer or has a literal typo — fix the arm in `messages/mod.rs`.

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/crates/tui/tests/dispatch_user_renderers.rs
git commit -m "$(cat <<'EOF'
plan(M7-05 T10): dispatch coverage tests for all 12 user renderers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Workspace verification gate + tag `m7.5`

**Files:** none modified (verification + tag only)

**Run every command from inside `lingxi-code/`** (toolchain pins 1.82; running from repo root produces spurious lint noise — this bit M6-08).

- [ ] **Step 1: Format check**

Run (cwd `lingxi-code/`): `cargo fmt --check`
Expected: clean. If it reports diffs, run `cargo fmt` and re-check.

- [ ] **Step 2: Clippy (deny warnings)**

Run (cwd `lingxi-code/`): `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. Common fixes in this sub-plan: unused `TuiTheme` imports in `bash_output.rs`, `needless_pass_by_value` on `Value`-taking fns (the `#![allow(...)]` at the top of those files covers it — add it if clippy flags).

- [ ] **Step 3: Full test suite**

Run (cwd `lingxi-code/`): `cargo test --workspace`
Expected: PASS. Known flakes (allowed to rerun, NOT failures): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, and `lingxi-platform-posix` fs_watch FSEvents timing tests. Rerun the specific flaky test if it trips; do not treat as a gate failure.

- [ ] **Step 4: Telemetry baseline unchanged**

Run (cwd `lingxi-code/`): `cargo test --workspace -- all_event_names 2>/dev/null; echo "verify the count test still reads 326"`
Expected: the `ALL_EVENT_NAMES.len()` assertion (wherever it lives) still reads **326**. M7-05 registered no new events. If the count changed, you accidentally registered an event — revert it.

- [ ] **Step 5: Cross-platform compile gate (5 targets)**

Run (cwd `lingxi-code/`), each target:
```bash
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```
Expected: all green (same posture as v0.6.0/v0.7.0). The renderers are pure Rust with no platform-specific code, so failures here indicate a cross-target issue in a dependency, not in M7-05 code.

- [ ] **Step 6: Annotated tag `m7.5`**

```bash
git tag -a m7.5 -m "M7-05: message renderers batch 2 (user) — 12 renderers"
```
(Local tag only. No remote push. No `v0.8.0` here — that is M7-16.)

- [ ] **Step 7: Final commit if any fmt/clippy fixes were applied**

If Steps 1-2 produced fixes:
```bash
git add -A
git commit -m "$(cat <<'EOF'
plan(M7-05 T11): workspace gate — fmt/clippy fixes + tag m7.5

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**1. Spec coverage (spec §3 "M7-05" entry):**
- 12 renderer files, each its own file under `messages/` — Tasks 1-9. ✓
- `RenderedMessage` enum variant per renderer (state.rs) — Tasks 1-9, 12 variants total. ✓
- Renderer fn routing markdown/code bodies through `render/markdown`+`render/syntax` — plan.rs + local_command_output.rs note the markdown route (TODO(M7-01) where the primitive isn't yet present); no code body needs syntax in this batch (syntax highlighting attaches to fenced blocks inside markdown, handled by `render::markdown`). ✓
- Dispatch entry in `messages/mod.rs` — every task wires `render_entry_to_string`; plan also wires the second dispatcher (`scrollback.rs`) which the spec's "dispatch" implies. ✓
- bash_output + local_command_output route through ANSI / markdown — Task 2 (ANSI), Task 3 (markdown). ✓
- image = placeholder/metadata only — Task 6. ✓
- grouped_tool_use + collapsed_read_search folding — Tasks 8-9, with collapsed/expanded behavior tests. ✓
- Telemetry adds 0 events; baseline 326 — Task 11 Step 4. ✓
- Tests: 1-2 insta snapshots per renderer (Tasks 1-7), folding behavior tests (Tasks 8-9), ANSI-passthrough test (Task 2), dispatch test per variant (Task 10). ✓
- Workspace gate from inside `lingxi-code/` + tag `m7.5` — Task 11. ✓

**2. Placeholder scan:** No "TBD"/"implement later"/"add error handling" — every step carries real code or an exact command. The only deferred items are explicitly-scoped M8 features (CloudLaunchContent diamonds, KAIROS brief layout, git/PR/bash/mcp/memory fold parts, inline image protocols) each with a documented `// SCOPE`/`TODO(M7-01)` marker and a reason. The `NO_CONTENT_MESSAGE` literal is flagged for byte-verification against `constants/messages.ts` (verify step in Task 3). ✓

**3. Type consistency:** `render_entry_to_string(entry, focused, expanded)` signature matches the existing fn (confirmed from `messages/mod.rs`). `render_message(m, expanded, focused_tool_id)` matches `scrollback.rs`. `AssistantToolUseProps`/`UserToolResultProps` field names (`id`, `tool`, `input`/`result`, `expanded`, `focused`) match the real structs. `ToolUseId::new()` is the real constructor (used in existing tests). `parse_ansi`/`AnsiSpan`/`AnsiColor`/`AnsiStyle` match `crate::ansi`. `TuiTheme::{ASSISTANT,USER,ERROR,DIM}` are the four real constants. The `expanded` map key for folding variants is `group_id` (consistent across state.rs variant, dispatcher, and component). ✓

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-29-m7-05-renderers-user.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
