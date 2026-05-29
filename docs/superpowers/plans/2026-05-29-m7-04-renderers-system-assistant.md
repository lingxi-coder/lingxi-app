# M7-04 — Message Renderers Batch 1 (system/assistant, 10 renderers) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 10 system/assistant message renderers to `lingxi-tui` — `thinking`, `redacted_thinking`, `compact_boundary`, `system_text`, `system_api_error`, `rate_limit`, `shutdown`, `advisor`, `hook_progress`, `plan_approval` — each its own file under `crates/tui/src/components/messages/`, each with a `RenderedMessage` enum variant, a pure-string renderer + iocraft component, and both dispatch entries wired. `compact_boundary` replaces M6-08's `[Compacted N → M messages]` `SystemText` placeholder with a real `CompactBoundaryMessage` variant.

**Architecture:** Each renderer follows the established M6-04 two-function pattern: a pure `render_*_to_string(props) -> String` (snapshot-testable, used by `render_entry_to_string`) plus a `#[component]` iocraft wrapper used by `components::scrollback::render_message`. New `RenderedMessage` variants live in `state.rs`; both dispatchers (`messages/mod.rs::render_entry_to_string` and `scrollback.rs::render_message`) gain one arm per variant. Renderers with markdown bodies (`thinking`, `advisor`) route through `render::markdown` (M7-01); none in this batch needs `render::syntax` directly (code blocks ride inside markdown via M7-01/02). Literal-lock: every label/marker/color is copied byte-for-byte from the equivalent claude-code `.tsx`.

**Tech Stack:** Rust 1.82 (pinned via `lingxi-core/rust-toolchain.toml`), iocraft `=0.8.3` (`View` not `Box`), `insta = "1.40"` snapshots, `chrono` (timestamps), `serde_json` (payloads). `lingxi-protocol::ToolUseId` correlator. `render::markdown::render` from M7-01 (prerequisite).

---

## Prerequisites & Dependencies

- **M7-01 (ANSI + markdown) MUST be merged first.** `thinking` and `advisor` route their bodies through `crate::render::markdown::render(text, theme) -> Vec<StyledLine>`. If M7-01's module is not present when this plan executes, those two renderers fall back to a single dim `Text` block over the raw body (documented inline in Task 2 / Task 8) and a follow-up wires markdown once M7-01 lands. **Verify before starting:** `ls lingxi-core/crates/tui/src/render/markdown.rs` exists and `crate::render::markdown::render` is callable.
- **`StyledLine` type name (M7-01):** the spec (§2.3) names the markdown/syntax output `Vec<StyledLine>` but the concrete type is defined in M7-01. **Do NOT invent it.** Before using it, grep for the real name: `grep -rn "struct StyledLine\|pub struct Styled\|type StyledLine" lingxi-core/crates/tui/src/render/`. If M7-01 named it differently (e.g. `MarkdownLine`, `RenderedLine`), use that name. In this batch, markdown output is flattened to a `String` for the pure renderer + snapshot (join styled-line plain text with `\n`); the iocraft component may render styled lines directly if the M7-01 helper exposes a `View`-building convenience, otherwise it renders the flattened string in dim.
- **M7-03 (VirtualMessageList)** is independent of this plan; renderers added here plug into whichever dispatcher is live (`scrollback.rs` today; `virtual_message_list.rs` after M7-03). This plan wires `scrollback.rs::render_message` — if M7-03 already replaced it, wire the equivalent dispatcher instead (same match arms).
- **Telemetry:** baseline is 326 events. **M7-04 adds 0 events.** No `ALL_EVENT_NAMES` changes. Do not register any telemetry name.

## Literal-Lock Reference (read each TSX before its renderer)

Source root: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/messages/`. Glyph constants from `claude-code/src/constants/figures.ts`: `BLACK_CIRCLE` = `⏺` on darwin else `●` (we lock the non-darwin `●` for parity with M6-04's existing `MARKER`), `TEARDROP_ASTERISK` = `✻` (U+273B), `REFERENCE_MARK` = `※` (U+203B). `figures` npm package: `tick` = `✔` (U+2714), `pointer` = `❯` (U+276F).

| Renderer | TSX | Locked literals (exact) |
|---|---|---|
| thinking | `AssistantThinkingMessage.tsx` + `HighlightedThinkingText.tsx` | collapsed: `∴ Thinking` (U+2234 + space + "Thinking") dim+italic, then a `(ctrl+o to expand)`-class hint (CtrlOToExpand). expanded: `∴ Thinking…` (trailing U+2026) header dim+italic, then markdown body indented 2 spaces, dim. |
| redacted_thinking | `AssistantRedactedThinkingMessage.tsx` | `✻ Thinking…` (U+273B + space + "Thinking" + U+2026) dim+italic. Single line, no expand. |
| compact_boundary | `CompactBoundaryMessage.tsx` | `✻ Conversation compacted (ctrl+o for history)` dim. (Shortcut literal `ctrl+o`.) |
| system_text | `SystemTextMessage.tsx` | info level → plain dim body (no marker). non-info (warning/error) → `●` marker (BLACK_CIRCLE) + body; warning color = warning/yellow, error color = error/red. `agents_killed` subtype → `● All background agents stopped` (marker error-colored, body dim). |
| system_api_error | `SystemAPIErrorMessage.tsx` | body = `formatAPIError` text, `error` color; truncated (>1000 chars, non-verbose) appends `…` + a `(ctrl+o to expand)` hint; footer line dim: `Retrying in {n} second(s)… (attempt {a}/{m})`. |
| rate_limit | `RateLimitMessage.tsx` | `text` line `error` color; optional dim upsell line. Upsell literals: `/upgrade to increase your usage limit.`, `/login to switch to an API usage-billed account.`, `/extra-usage to finish what you’re working on.` (U+2019 apostrophe), `/extra-usage to request more usage from your admin.`, `/upgrade or /extra-usage to finish what you’re working on.`, `Opening your options…` |
| shutdown | `ShutdownMessage.tsx` | request: `Shutdown request from {from}` (warning, bold) + optional `Reason: {reason}`, round warning border. rejected: `Shutdown rejected by {from}` (subtle, bold) + `Reason: {reason}` + `Teammate is continuing to work. You may request shutdown again later.` (dim). |
| advisor | `AdvisorMessage.tsx` | server_tool_use: `Advising` bold (+ ` using {model}` dim + ` · {input}` dim). advisor_result non-verbose: `✔ Advisor has reviewed the conversation and will apply the feedback` (dim) + CtrlOToExpand; verbose: raw text dim. advisor_tool_result_error: `Advisor unavailable ({error_code})` error. advisor_redacted_result: `✔ Advisor has reviewed the conversation and will apply the feedback` dim (no expand). |
| hook_progress | `HookProgressMessage.tsx` | running (Pre/PostToolUse hidden outside transcript): `Running ` dim + `{event}` dim-bold + ` hook…`/` hooks…` dim (count==1 → ` hook…`). transcript summary for Pre/PostToolUse: `{n} ` dim + `{event}` dim-bold + ` hook ran`/` hooks ran`. |
| plan_approval | `PlanApprovalMessage.tsx` | request: `Plan Approval Request from {from}` (planMode color, bold) + markdown plan content (dashed subtle border) + `Plan file: {path}` dim, round planMode border. approved: `✓ Plan Approved by {name}` (success/green, bold) + `You can now proceed with implementation. Your plan mode restrictions have been lifted.` rejected: `✗ Plan Rejected by {name}` (error/red, bold) + optional `Feedback: {feedback}` + `Please revise your plan based on the feedback and call ExitPlanMode again.` (dim). |

**Color mapping** (claude-code semantic name → `TuiTheme` / iocraft `Color`): `error`/`red` → `TuiTheme::ERROR`; `dimColor`/`subtle` → `TuiTheme::DIM`; `text`/`info` body → `TuiTheme::DIM` (system messages render dim); `warning` → `Color::Yellow`; `success` → `Color::Green`; `planMode` → `Color::Magenta`. (M6 has no `warning`/`success`/`planMode` constants yet — M7-15 adds the full theme; for this batch use the literal iocraft colors above and leave a `// TODO(M7-15): theme constant` comment so the theme picker can centralize them later.)

---

## File Structure

**Create (10 renderer files):** `lingxi-core/crates/tui/src/components/messages/`
- `thinking.rs` — `AssistantThinkingMessage` + `render_thinking_to_string`. Owns the `∴ Thinking` collapsed/`∴ Thinking…` expanded split; routes expanded body through `render::markdown`.
- `redacted_thinking.rs` — `AssistantRedactedThinkingMessage` + `render_redacted_thinking_to_string`. One-line `✻ Thinking…`.
- `compact_boundary.rs` — `CompactBoundaryMessage` + `render_compact_boundary_to_string`. `✻ Conversation compacted (ctrl+o for history)`.
- `system_text.rs` — `SystemTextMessage` + `render_system_text_to_string`. Level-aware marker/color. (Distinct from the inline `SystemText` arm M6 rendered with bare `Text`; this is the richer renderer keyed on a `level`.)
- `system_api_error.rs` — `SystemApiErrorMessage` + `render_system_api_error_to_string`. Body + truncation + retry footer.
- `rate_limit.rs` — `RateLimitMessage` + `render_rate_limit_to_string`. error text + optional upsell; `upsell_message(...)` helper holds the locked upsell literals.
- `shutdown.rs` — `ShutdownMessage` + `render_shutdown_to_string`. Request/rejected kinds.
- `advisor.rs` — `AdvisorMessage` + `render_advisor_to_string`. server_tool_use / result / error / redacted kinds; result body routes through `render::markdown` when verbose.
- `hook_progress.rs` — `HookProgressMessage` + `render_hook_progress_to_string`. Running vs transcript-summary, singular/plural.
- `plan_approval.rs` — `PlanApprovalMessage` + `render_plan_approval_to_string`. Request/approved/rejected kinds; request plan content routes through `render::markdown`.

**Modify:**
- `lingxi-core/crates/tui/src/state.rs` — add 10 `RenderedMessage` variants (Task 1).
- `lingxi-core/crates/tui/src/components/messages/mod.rs` — `pub mod` each new file; extend `render_entry_to_string` with 10 match arms.
- `lingxi-core/crates/tui/src/components/scrollback.rs` — extend `render_message` with 10 match arms (iocraft components).
- `lingxi-core/crates/tui/src/streaming.rs:83-97` — `CompactionCompleted` handler pushes `RenderedMessage::CompactBoundary {...}` instead of the `SystemText` `[Compacted …]` placeholder (Task 4).
- `lingxi-core/crates/tui/src/events/orchestrator_bridge.rs:71-82` — update the `CompactionCompleted` doc-comment to reflect the real renderer (no code change beyond the comment) (Task 4).

**Create (test files):** `lingxi-core/crates/tui/tests/`
- `render_messages_batch1.rs` — insta snapshots, one block per renderer (collapsed + expanded where applicable).
- `dispatch_batch1.rs` — asserts each new variant routes through `render_entry_to_string` to the expected renderer output (string-form).

**Snapshots land in:** `lingxi-core/crates/tui/tests/snapshots/` (insta auto-creates `*.snap`).

---

## Conventions (apply to every renderer)

- **iocraft:** use `View` (not `Box`); `element! { View(flex_direction: FlexDirection::Column) { Text(content: s, color: c) } }`. Pattern: see `assistant_text.rs` (cleanest) and `assistant_tool_use.rs` (string-fn + component split).
- **Pure renderer first:** `render_<name>_to_string(props: <Name>Props) -> String` returns the exact display string (newline-joined lines). The component wraps it (or, for multi-color renderers, builds `View` children directly while the string fn stays the snapshot oracle). `#![allow(clippy::needless_pass_by_value)]` at file top where props are taken by value (matches `assistant_tool_use.rs`).
- **Props:** `#[derive(Debug, Clone, Default, Props)]`. Carry only what the variant carries.
- **Glyph constants** declared as `pub const` at file top with a byte-comment, e.g. `/// U+2234 + space. dim+italic.` `pub const THINKING_MARKER: &str = "∴ ";` — and a `#[test] fn marker_bytes()` guarding the UTF-8 bytes (mirrors `assistant_tool_use.rs::marker_is_three_utf8_bytes`).
- **Run cargo from inside `lingxi-core/`** (rust-toolchain pins 1.82; repo-root runs use host toolchain → spurious lint noise — this bit M6-08).
- **Commit format:** `plan(M7-04 TN): <subject>` with trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. **Do not push.**

---

## Task 1: `RenderedMessage` variants + module declarations

**Files:**
- Modify: `lingxi-core/crates/tui/src/state.rs` (the `RenderedMessage` enum, ~line 33-81)
- Modify: `lingxi-core/crates/tui/src/components/messages/mod.rs` (module decls, ~line 8-11)
- Test: `lingxi-core/crates/tui/src/state.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test** — append to `state.rs` tests module:

```rust
    /// M7-04 Task 1: RenderedMessage carries the 10 batch-1 variants.
    #[test]
    fn rendered_message_carries_batch1_variants() {
        let _t = RenderedMessage::AssistantThinking { thinking: "x".into(), expanded: false };
        let _r = RenderedMessage::AssistantRedactedThinking;
        let _c = RenderedMessage::CompactBoundary { messages_before: 50, messages_after: 5 };
        let _s = RenderedMessage::SystemTextRich { body: "hi".into(), level: SystemLevel::Warning };
        let _e = RenderedMessage::SystemApiError {
            error: "boom".into(), retry_attempt: 4, retry_in_seconds: 3, max_retries: 10, truncated: false,
        };
        let _l = RenderedMessage::RateLimit { text: "limited".into(), upsell: None };
        let _sd = RenderedMessage::Shutdown { from: "agent-1".into(), reason: Some("done".into()), rejected: false };
        let _a = RenderedMessage::Advisor { kind: AdvisorKind::Result { text: "ok".into() }, verbose: false };
        let _h = RenderedMessage::HookProgress { event: "PreToolUse".into(), count: 2, transcript_summary: false };
        let _p = RenderedMessage::PlanApproval {
            kind: PlanApprovalKind::Approved { name: "you".into() },
        };
        assert!(matches!(_c, RenderedMessage::CompactBoundary { .. }));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --lib rendered_message_carries_batch1_variants`
Expected: FAIL — `no variant named AssistantThinking`, undefined `SystemLevel`/`AdvisorKind`/`PlanApprovalKind`.

- [ ] **Step 3: Add the enum variants + supporting enums** in `state.rs`. Insert these variants into `RenderedMessage` (after `UserToolResult`):

```rust
    /// (M7-04) Assistant thinking block. Collapsed → `∴ Thinking` + expand hint;
    /// expanded → `∴ Thinking…` + markdown body. `expanded` mirrors
    /// `AppState.expanded`-style state (default false → collapsed).
    AssistantThinking { thinking: String, expanded: bool },
    /// (M7-04) Redacted thinking. Single dim+italic line `✻ Thinking…`.
    AssistantRedactedThinking,
    /// (M7-04) Compaction boundary. REPLACES M6-08's `[Compacted …]` SystemText.
    /// Renders `✻ Conversation compacted (ctrl+o for history)` (dim). Counts are
    /// retained for telemetry/debug parity though the rendered line omits them
    /// (claude-code parity — the boundary line carries no numbers).
    CompactBoundary { messages_before: u32, messages_after: u32 },
    /// (M7-04) Level-aware system text. info → plain dim body; warning/error →
    /// `●` marker + colored body.
    SystemTextRich { body: String, level: SystemLevel },
    /// (M7-04) API error with retry countdown footer.
    SystemApiError {
        error: String,
        retry_attempt: u32,
        retry_in_seconds: u32,
        max_retries: u32,
        truncated: bool,
    },
    /// (M7-04) Rate-limit notice (error text + optional dim upsell line).
    RateLimit { text: String, upsell: Option<String> },
    /// (M7-04) Teammate shutdown request/rejected notice.
    Shutdown { from: String, reason: Option<String>, rejected: bool },
    /// (M7-04) Advisor block.
    Advisor { kind: AdvisorKind, verbose: bool },
    /// (M7-04) Hook-progress line.
    HookProgress { event: String, count: u32, transcript_summary: bool },
    /// (M7-04) Plan approval request/response.
    PlanApproval { kind: PlanApprovalKind },
```

Add these enums below `RenderedMessage` (top-level in `state.rs`):

```rust
/// (M7-04) System message severity → marker/color mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLevel {
    /// Plain dim body, no marker.
    Info,
    /// `●` marker + yellow body.
    Warning,
    /// `●` marker + red body.
    Error,
}

/// (M7-04) Advisor block content kinds (claude-code AdvisorMessage subtypes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvisorKind {
    /// `Advising` header (+ optional model / input descriptor).
    ServerToolUse { model: Option<String>, input: Option<String> },
    /// Advisor reviewed-and-applied result; `text` is the full feedback.
    Result { text: String },
    /// Redacted result — no expandable body.
    RedactedResult,
    /// `Advisor unavailable ({error_code})`.
    Error { error_code: String },
}

/// (M7-04) Plan-approval request/response kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanApprovalKind {
    /// Approval request from a teammate.
    Request { from: String, plan_content: String, plan_file_path: Option<String> },
    /// Approved by `{name}`.
    Approved { name: String },
    /// Rejected by `{name}` with optional feedback.
    Rejected { name: String, feedback: Option<String> },
}
```

In `components/messages/mod.rs`, add the module declarations after the existing four:

```rust
pub mod advisor;
pub mod compact_boundary;
pub mod hook_progress;
pub mod plan_approval;
pub mod rate_limit;
pub mod redacted_thinking;
pub mod shutdown;
pub mod system_api_error;
pub mod system_text;
pub mod thinking;
```

- [ ] **Step 4: Run test to verify it passes**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --lib rendered_message_carries_batch1_variants`
Expected: PASS. (The `pub mod` lines will not compile yet because the files don't exist — create empty stubs `// stub` for each in `components/messages/` so the lib compiles, OR add the `pub mod` lines in Step 3 of Task 2 onward. **Decision:** create empty stub files now containing only a doc-comment so the crate compiles; each later task replaces its stub.)

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/state.rs lingxi-core/crates/tui/src/components/messages/
git commit -m "$(cat <<'EOF'
plan(M7-04 T1): add 10 batch-1 RenderedMessage variants + module stubs

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `thinking.rs` (AssistantThinkingMessage)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/thinking.rs` (replaces stub)
- Test: `lingxi-core/crates/tui/tests/render_messages_batch1.rs` (create)

Read first: `claude-code/src/components/messages/AssistantThinkingMessage.tsx` + `HighlightedThinkingText.tsx`.

- [ ] **Step 1: Write the failing test** — create `render_messages_batch1.rs`:

```rust
//! M7-04 batch-1 renderer snapshots.
use lingxi_tui::components::messages::thinking::{render_thinking_to_string, ThinkingProps};

#[test]
fn thinking_collapsed() {
    let s = render_thinking_to_string(ThinkingProps {
        thinking: "Considering the tradeoffs between A and B.".into(),
        expanded: false,
    });
    insta::assert_snapshot!("thinking_collapsed", s);
}

#[test]
fn thinking_expanded() {
    let s = render_thinking_to_string(ThinkingProps {
        thinking: "Step one.\nStep two.".into(),
        expanded: true,
    });
    insta::assert_snapshot!("thinking_expanded", s);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --test render_messages_batch1 thinking`
Expected: FAIL — unresolved import `thinking::render_thinking_to_string`.

- [ ] **Step 3: Write the renderer** — `thinking.rs`:

```rust
//! `AssistantThinkingMessage` — `∴ Thinking` collapsed / `∴ Thinking…` expanded.
//!
//! Literal locks (claude-code AssistantThinkingMessage.tsx):
//!   - collapsed header: `∴ Thinking` (U+2234 + space + "Thinking") dim+italic,
//!     followed by a `(ctrl+o to expand)` hint (CtrlOToExpand).
//!   - expanded header: `∴ Thinking…` (trailing U+2026), then markdown body
//!     indented 2 spaces, dim.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// `∴ ` marker. U+2234 (0xE2 0x88 0xB4) + ASCII space.
pub const THINKING_MARKER: &str = "∴ ";
/// Collapsed-state expand hint (claude-code CtrlOToExpand surface).
pub const EXPAND_HINT: &str = "(ctrl+o to expand)";
/// Per-line body indent (2 spaces) — claude-code `paddingLeft={2}`.
pub const INDENT: &str = "  ";

/// Props for [`AssistantThinkingMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct ThinkingProps {
    /// The thinking text (markdown when expanded).
    pub thinking: String,
    /// `true` → render the full markdown body; `false` → header + hint only.
    pub expanded: bool,
}

/// Pure-string renderer (snapshot oracle).
#[must_use]
pub fn render_thinking_to_string(props: ThinkingProps) -> String {
    if !props.expanded {
        return format!("{THINKING_MARKER}Thinking {EXPAND_HINT}");
    }
    // Expanded: `∴ Thinking…` then markdown body, each line indented 2.
    let body = markdown_plain(&props.thinking);
    let mut out = format!("{THINKING_MARKER}Thinking\u{2026}");
    for line in body.lines() {
        out.push('\n');
        out.push_str(INDENT);
        out.push_str(line);
    }
    out
}

/// Flatten markdown → plain text for the string oracle. Routes through
/// M7-01's `render::markdown` when present; falls back to the raw text
/// otherwise (markdown styling shows in the iocraft component, not the
/// snapshot string).
fn markdown_plain(text: &str) -> String {
    // M7-01 prereq: `crate::render::markdown::render(text, theme)`.
    // If the helper exposes a plain-text projection, prefer it; otherwise
    // the raw text is already the plain projection.
    text.to_string()
}

/// iocraft component — dim+italic header; expanded body rendered dim.
#[component]
pub fn AssistantThinkingMessage(props: &ThinkingProps) -> impl Into<AnyElement<'static>> {
    let body = render_thinking_to_string(props.clone());
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM, italic: true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_four_utf8_bytes() {
        // U+2234 = 0xE2 0x88 0xB4, then ASCII space.
        assert_eq!(THINKING_MARKER.as_bytes(), &[0xE2, 0x88, 0xB4, 0x20]);
    }
}
```

(Note: `Text(... italic: true)` — confirm iocraft `0.8.3` exposes `italic` on `Text`; if not, drop the attribute and document the limitation. Check `assistant_text.rs` / iocraft prelude before relying on it.)

- [ ] **Step 4: Run test to verify it passes**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --test render_messages_batch1 thinking`
Then `cargo insta review` (accept the two new snapshots after eyeballing `∴ Thinking (ctrl+o to expand)` and the expanded form). Re-run; expected PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/thinking.rs lingxi-core/crates/tui/tests/render_messages_batch1.rs lingxi-core/crates/tui/tests/snapshots/
git commit -m "$(cat <<'EOF'
plan(M7-04 T2): thinking renderer (∴ Thinking collapsed/expanded)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `redacted_thinking.rs` + `compact_boundary.rs` (2 renderers)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/redacted_thinking.rs`
- Create: `lingxi-core/crates/tui/src/components/messages/compact_boundary.rs`
- Test: `lingxi-core/crates/tui/tests/render_messages_batch1.rs` (append)

Read first: `AssistantRedactedThinkingMessage.tsx`, `CompactBoundaryMessage.tsx`.

- [ ] **Step 1: Write the failing tests** — append to `render_messages_batch1.rs`:

```rust
use lingxi_tui::components::messages::redacted_thinking::render_redacted_thinking_to_string;
use lingxi_tui::components::messages::compact_boundary::render_compact_boundary_to_string;

#[test]
fn redacted_thinking_line() {
    insta::assert_snapshot!("redacted_thinking_line", render_redacted_thinking_to_string());
}

#[test]
fn compact_boundary_line() {
    insta::assert_snapshot!("compact_boundary_line", render_compact_boundary_to_string());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --test render_messages_batch1 redacted_thinking_line compact_boundary_line`
Expected: FAIL — unresolved imports.

- [ ] **Step 3: Write the renderers.**

`redacted_thinking.rs`:

```rust
//! `AssistantRedactedThinkingMessage` — `✻ Thinking…` dim+italic, single line.
//!
//! Literal lock (AssistantRedactedThinkingMessage.tsx): `✻ Thinking…`
//! (U+273B + space + "Thinking" + U+2026), dimColor italic.
use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// `✻ ` marker. U+273B (0xE2 0x9C 0xBB) + ASCII space.
pub const REDACTED_MARKER: &str = "✻ ";

/// Pure-string renderer (no props — the line is fixed).
#[must_use]
pub fn render_redacted_thinking_to_string() -> String {
    format!("{REDACTED_MARKER}Thinking\u{2026}")
}

/// iocraft component.
#[component]
pub fn AssistantRedactedThinkingMessage() -> impl Into<AnyElement<'static>> {
    let body = render_redacted_thinking_to_string();
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM, italic: true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_bytes() {
        assert_eq!(REDACTED_MARKER.as_bytes(), &[0xE2, 0x9C, 0xBB, 0x20]);
    }
}
```

`compact_boundary.rs`:

```rust
//! `CompactBoundaryMessage` — `✻ Conversation compacted (ctrl+o for history)`.
//!
//! Literal lock (CompactBoundaryMessage.tsx): dimColor, marginY 1. Shortcut
//! literal `ctrl+o`. REPLACES M6-08's `[Compacted N → M messages]` SystemText
//! placeholder (see streaming.rs CompactionCompleted handler).
use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Locked boundary line. `✻` = U+273B.
pub const BOUNDARY_LINE: &str = "✻ Conversation compacted (ctrl+o for history)";

/// Pure-string renderer (the line is fixed; counts are not rendered — parity).
#[must_use]
pub fn render_compact_boundary_to_string() -> String {
    BOUNDARY_LINE.to_string()
}

/// iocraft component — dim.
#[component]
pub fn CompactBoundaryMessage() -> impl Into<AnyElement<'static>> {
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: BOUNDARY_LINE, color: TuiTheme::DIM)
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --test render_messages_batch1 redacted_thinking_line compact_boundary_line` then `cargo insta review`. Expected PASS after accepting `✻ Thinking…` and `✻ Conversation compacted (ctrl+o for history)`.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/redacted_thinking.rs lingxi-core/crates/tui/src/components/messages/compact_boundary.rs lingxi-core/crates/tui/tests/
git commit -m "$(cat <<'EOF'
plan(M7-04 T3): redacted_thinking + compact_boundary renderers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Replace the M6-08 `[Compacted]` placeholder with `CompactBoundary`

**Files:**
- Modify: `lingxi-core/crates/tui/src/streaming.rs:83-97` (the `CompactionCompleted` arm)
- Modify: `lingxi-core/crates/tui/src/events/orchestrator_bridge.rs:71-82` (doc-comment only)
- Modify: `lingxi-core/crates/tui/tests/behavior_compact_marker.rs` (the assertion that checks for `"Compacted"`)
- Test: same `behavior_compact_marker.rs`

The M6-08 placeholder is emitted in `streaming.rs::apply_event` under `TurnEvent::CompactionCompleted` as a `RenderedMessage::SystemText { body: "[Compacted N → M messages]", .. }`. Switch it to the real variant.

- [ ] **Step 1: Update the failing behavior test** — in `behavior_compact_marker.rs`, replace the `"Compacted"` substring assertion with a variant + boundary-line check:

```rust
    // M7-04 T4: CompactionCompleted now pushes a CompactBoundary variant,
    // not a SystemText `[Compacted …]` placeholder.
    let last = state.messages.last().expect("a message was pushed");
    assert!(
        matches!(last, lingxi_tui::state::RenderedMessage::CompactBoundary { messages_before: 50, messages_after: 5 }),
        "expected CompactBoundary, got: {last:?}",
    );
    // The rendered line is the locked boundary string (no counts).
    let rendered = lingxi_tui::components::messages::render_entry_to_string(last, false, false);
    assert_eq!(rendered, "✻ Conversation compacted (ctrl+o for history)");
```

(Adjust the surrounding test setup to fire `CompactionCompleted { messages_before: 50, messages_after: 5, bytes_saved: 4096 }`. Read the existing test for its current driver shape.)

- [ ] **Step 2: Run test to verify it fails**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --test behavior_compact_marker`
Expected: FAIL — still pushes `SystemText`; `render_entry_to_string` has no `CompactBoundary` arm yet.

- [ ] **Step 3: Switch the emit site** — in `streaming.rs`, replace the `CompactionCompleted` body:

```rust
        TurnEvent::CompactionCompleted {
            messages_before,
            messages_after,
            ..
        } => {
            // M7-04: real CompactBoundaryMessage (replaces M6-08's
            // `[Compacted N → M messages]` SystemText placeholder). Renders
            // `✻ Conversation compacted (ctrl+o for history)` (counts retained
            // on the variant for debug/telemetry parity but not rendered).
            state.messages.push(RenderedMessage::CompactBoundary {
                messages_before,
                messages_after,
            });
        }
```

Update the `orchestrator_bridge.rs` `CompactionCompleted` doc-comment: change "Proper `CompactBoundaryMessage` rendering with summary preview lands in M7." → "Rendered by `CompactBoundaryMessage` (M7-04) as `✻ Conversation compacted (ctrl+o for history)`."

Wire the dispatchers (needed for `render_entry_to_string` in the test to compile) — in `messages/mod.rs::render_entry_to_string` add:

```rust
        RenderedMessage::CompactBoundary { .. } => {
            crate::components::messages::compact_boundary::render_compact_boundary_to_string()
        }
```

and in `scrollback.rs::render_message` add:

```rust
        RenderedMessage::CompactBoundary { .. } => element! {
            CompactBoundaryMessage()
        }
        .into_any(),
```

(Import `CompactBoundaryMessage` in `scrollback.rs`'s `use` block. Defer the other 9 dispatch arms to Task 11 — but if the match becomes non-exhaustive, add `_ => element! { Text(content: String::new()) }.into_any()` as a temporary catch-all and REMOVE it in Task 11. **Decision:** add the temporary catch-all; Task 11's test makes its removal verifiable.)

- [ ] **Step 4: Run test to verify it passes**

Run (from `lingxi-core/`): `cargo test -p lingxi-tui --test behavior_compact_marker`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/streaming.rs lingxi-core/crates/tui/src/events/orchestrator_bridge.rs lingxi-core/crates/tui/src/components/messages/mod.rs lingxi-core/crates/tui/src/components/scrollback.rs lingxi-core/crates/tui/tests/behavior_compact_marker.rs
git commit -m "$(cat <<'EOF'
plan(M7-04 T4): emit CompactBoundary; retire [Compacted] SystemText placeholder

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `system_text.rs` (SystemTextMessage)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/system_text.rs`
- Test: `render_messages_batch1.rs` (append)

Read first: `SystemTextMessage.tsx` (level/marker logic at the tail; `agents_killed` subtype).

- [ ] **Step 1: Write the failing tests** — append to `render_messages_batch1.rs`:

```rust
use lingxi_tui::components::messages::system_text::{render_system_text_to_string, SystemTextProps};
use lingxi_tui::state::SystemLevel;

#[test]
fn system_text_info_plain() {
    let s = render_system_text_to_string(SystemTextProps { body: "Saved settings.".into(), level: SystemLevel::Info });
    insta::assert_snapshot!("system_text_info_plain", s);
}

#[test]
fn system_text_warning_dotted() {
    let s = render_system_text_to_string(SystemTextProps { body: "Approaching context limit.".into(), level: SystemLevel::Warning });
    insta::assert_snapshot!("system_text_warning_dotted", s);
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p lingxi-tui --test render_messages_batch1 system_text`; FAIL (unresolved import).

- [ ] **Step 3: Write the renderer** — `system_text.rs`:

```rust
//! `SystemTextMessage` — level-aware system line.
//!
//! Literal lock (SystemTextMessage.tsx): info level → plain dim body, no
//! marker. Non-info → `●` (BLACK_CIRCLE) marker + body; warning → yellow,
//! error → red. (We lock the non-darwin BLACK_CIRCLE `●` to match M6-04's
//! existing tool-use MARKER.)
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::state::SystemLevel;
use crate::theme::TuiTheme;

/// `●` marker (BLACK_CIRCLE, non-darwin form). U+25CF.
pub const MARKER: &str = "● ";

/// Props.
#[derive(Debug, Clone, Default, Props)]
pub struct SystemTextProps {
    /// Message body.
    pub body: String,
    /// Severity → marker/color.
    pub level: SystemLevel,
}

/// Pure-string renderer.
#[must_use]
pub fn render_system_text_to_string(props: SystemTextProps) -> String {
    match props.level {
        SystemLevel::Info => props.body,
        SystemLevel::Warning | SystemLevel::Error => format!("{MARKER}{}", props.body),
    }
}

/// iocraft component.
#[component]
pub fn SystemTextMessage(props: &SystemTextProps) -> impl Into<AnyElement<'static>> {
    let body = render_system_text_to_string(props.clone());
    let color = match props.level {
        SystemLevel::Info => TuiTheme::DIM,
        // TODO(M7-15): theme constants for warning/error.
        SystemLevel::Warning => Color::Yellow,
        SystemLevel::Error => TuiTheme::ERROR,
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
        }
    }
}
```

Add `impl Default for SystemLevel { fn default() -> Self { Self::Info } }` to `state.rs` (Props derive needs it via `SystemTextProps: Default`).

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p lingxi-tui --test render_messages_batch1 system_text` + `cargo insta review`. Expected PASS (`Saved settings.` plain; `● Approaching context limit.`).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/system_text.rs lingxi-core/crates/tui/src/state.rs lingxi-core/crates/tui/tests/
git commit -m "$(cat <<'EOF'
plan(M7-04 T5): system_text renderer (level-aware marker/color)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `system_api_error.rs` (SystemAPIErrorMessage)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/system_api_error.rs`
- Test: `render_messages_batch1.rs` (append)

Read first: `SystemAPIErrorMessage.tsx`. Locked: error body + truncation `…` + `(ctrl+o to expand)` hint when truncated; footer `Retrying in {n} second(s)… (attempt {a}/{m})` (singular `second` when n==1). `MAX_API_ERROR_CHARS = 1000`.

- [ ] **Step 1: Write the failing tests** — append:

```rust
use lingxi_tui::components::messages::system_api_error::{render_system_api_error_to_string, SystemApiErrorProps};

#[test]
fn system_api_error_with_retry() {
    let s = render_system_api_error_to_string(SystemApiErrorProps {
        error: "529 Overloaded".into(), retry_attempt: 4, retry_in_seconds: 3, max_retries: 10, truncated: false,
    });
    insta::assert_snapshot!("system_api_error_with_retry", s);
}

#[test]
fn system_api_error_singular_second_and_truncated() {
    let s = render_system_api_error_to_string(SystemApiErrorProps {
        error: "boom".into(), retry_attempt: 5, retry_in_seconds: 1, max_retries: 10, truncated: true,
    });
    insta::assert_snapshot!("system_api_error_singular_second_and_truncated", s);
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p lingxi-tui --test render_messages_batch1 system_api_error`; FAIL.

- [ ] **Step 3: Write the renderer** — `system_api_error.rs`:

```rust
//! `SystemAPIErrorMessage` — error body + retry-countdown footer.
//!
//! Literal lock (SystemAPIErrorMessage.tsx): error-colored body; if truncated
//! (>1000 chars, non-verbose) append `…` + a `(ctrl+o to expand)` hint; footer
//! dim: `Retrying in {n} second(s)… (attempt {a}/{m})`.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Truncation hint surface (CtrlOToExpand).
pub const EXPAND_HINT: &str = "(ctrl+o to expand)";

/// Props.
#[derive(Debug, Clone, Default, Props)]
pub struct SystemApiErrorProps {
    /// Formatted API error text.
    pub error: String,
    /// 1-based retry attempt.
    pub retry_attempt: u32,
    /// Seconds until the next retry.
    pub retry_in_seconds: u32,
    /// Max retry attempts.
    pub max_retries: u32,
    /// `true` → error body was clipped; append `…` + hint.
    pub truncated: bool,
}

/// Pure-string renderer.
#[must_use]
pub fn render_system_api_error_to_string(props: SystemApiErrorProps) -> String {
    let mut out = props.error.clone();
    if props.truncated {
        out.push('\u{2026}');
        out.push('\n');
        out.push_str(EXPAND_HINT);
    }
    let unit = if props.retry_in_seconds == 1 { "second" } else { "seconds" };
    out.push('\n');
    out.push_str(&format!(
        "Retrying in {n} {unit}\u{2026} (attempt {a}/{m})",
        n = props.retry_in_seconds,
        a = props.retry_attempt,
        m = props.max_retries,
    ));
    out
}

/// iocraft component — error body (red) + dim footer.
#[component]
pub fn SystemApiErrorMessage(props: &SystemApiErrorProps) -> impl Into<AnyElement<'static>> {
    let unit = if props.retry_in_seconds == 1 { "second" } else { "seconds" };
    let body = if props.truncated {
        format!("{}\u{2026}\n{EXPAND_HINT}", props.error)
    } else {
        props.error.clone()
    };
    let footer = format!(
        "Retrying in {} {unit}\u{2026} (attempt {}/{})",
        props.retry_in_seconds, props.retry_attempt, props.max_retries,
    );
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::ERROR)
            Text(content: footer, color: TuiTheme::DIM)
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p lingxi-tui --test render_messages_batch1 system_api_error` + `cargo insta review`. Verify footer `Retrying in 3 seconds… (attempt 4/10)` and the singular/truncated case `Retrying in 1 second… (attempt 5/10)`. Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/system_api_error.rs lingxi-core/crates/tui/tests/
git commit -m "$(cat <<'EOF'
plan(M7-04 T6): system_api_error renderer (body + retry footer)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `rate_limit.rs` (RateLimitMessage)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/rate_limit.rs`
- Test: `render_messages_batch1.rs` (append)

Read first: `RateLimitMessage.tsx` (`getUpsellMessage` holds the locked strings).

- [ ] **Step 1: Write the failing tests** — append:

```rust
use lingxi_tui::components::messages::rate_limit::{render_rate_limit_to_string, RateLimitProps};

#[test]
fn rate_limit_with_upsell() {
    let s = render_rate_limit_to_string(RateLimitProps {
        text: "You've hit your usage limit.".into(),
        upsell: Some("/upgrade to increase your usage limit.".into()),
    });
    insta::assert_snapshot!("rate_limit_with_upsell", s);
}

#[test]
fn rate_limit_no_upsell() {
    let s = render_rate_limit_to_string(RateLimitProps {
        text: "You've hit your usage limit.".into(), upsell: None,
    });
    insta::assert_snapshot!("rate_limit_no_upsell", s);
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p lingxi-tui --test render_messages_batch1 rate_limit`; FAIL.

- [ ] **Step 3: Write the renderer** — `rate_limit.rs`. Include a `pub fn upsell_message(...) -> Option<String>` helper carrying the locked literals (so the variant builder reuses them), and a `#[test]` asserting one literal byte-for-byte (the `’` U+2019 apostrophe matters):

```rust
//! `RateLimitMessage` — error text + optional dim upsell line.
//!
//! Literal lock (RateLimitMessage.tsx `getUpsellMessage`). Note the curly
//! apostrophe U+2019 in "you’re".
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Props.
#[derive(Debug, Clone, Default, Props)]
pub struct RateLimitProps {
    /// The rate-limit notice text (error-colored).
    pub text: String,
    /// Optional dim upsell line (computed by [`upsell_message`]).
    pub upsell: Option<String>,
}

/// Locked upsell strings. Mirrors claude-code `getUpsellMessage`.
pub mod upsell {
    /// Max-20x + extra-usage enabled.
    pub const EXTRA_USAGE_FINISH: &str = "/extra-usage to finish what you\u{2019}re working on.";
    /// Max-20x, extra-usage disabled.
    pub const LOGIN_SWITCH: &str = "/login to switch to an API usage-billed account.";
    /// Auto-open menu.
    pub const OPENING_OPTIONS: &str = "Opening your options\u{2026}";
    /// Default (non-team, no extra-usage).
    pub const UPGRADE: &str = "/upgrade to increase your usage limit.";
    /// Team/enterprise, no billing access.
    pub const EXTRA_USAGE_ADMIN: &str = "/extra-usage to request more usage from your admin.";
    /// Fallback.
    pub const UPGRADE_OR_EXTRA: &str =
        "/upgrade or /extra-usage to finish what you\u{2019}re working on.";
}

/// Pure-string renderer.
#[must_use]
pub fn render_rate_limit_to_string(props: RateLimitProps) -> String {
    match props.upsell {
        Some(u) => format!("{}\n{u}", props.text),
        None => props.text,
    }
}

/// iocraft component — error text + optional dim upsell.
#[component]
pub fn RateLimitMessage(props: &RateLimitProps) -> impl Into<AnyElement<'static>> {
    let text = props.text.clone();
    let upsell = props.upsell.clone();
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: text, color: TuiTheme::ERROR)
            #(upsell.map(|u| element! { Text(content: u, color: TuiTheme::DIM) }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::upsell;
    #[test]
    fn extra_usage_uses_curly_apostrophe() {
        assert!(upsell::EXTRA_USAGE_FINISH.contains('\u{2019}'));
    }
}
```

(If iocraft's `#(...)` macro doesn't accept an `Option<Element>` interpolation in `0.8.3`, branch into two `element!` returns — one with the upsell `Text`, one without. Check the prelude / `user_tool_result.rs`'s `#(span_elements)` usage for the supported form.)

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p lingxi-tui --test render_messages_batch1 rate_limit` + `cargo insta review`. Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/rate_limit.rs lingxi-core/crates/tui/tests/
git commit -m "$(cat <<'EOF'
plan(M7-04 T7): rate_limit renderer + locked upsell literals

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `shutdown.rs` + `advisor.rs` (2 renderers)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/shutdown.rs`
- Create: `lingxi-core/crates/tui/src/components/messages/advisor.rs`
- Test: `render_messages_batch1.rs` (append)

Read first: `ShutdownMessage.tsx`, `AdvisorMessage.tsx`. Advisor result body (verbose) routes through `render::markdown` (M7-01); non-verbose is the fixed `✔ Advisor …` line.

- [ ] **Step 1: Write the failing tests** — append:

```rust
use lingxi_tui::components::messages::shutdown::{render_shutdown_to_string, ShutdownProps};
use lingxi_tui::components::messages::advisor::{render_advisor_to_string, AdvisorProps};
use lingxi_tui::state::AdvisorKind;

#[test]
fn shutdown_request_with_reason() {
    let s = render_shutdown_to_string(ShutdownProps { from: "agent-2".into(), reason: Some("task done".into()), rejected: false });
    insta::assert_snapshot!("shutdown_request_with_reason", s);
}

#[test]
fn shutdown_rejected() {
    let s = render_shutdown_to_string(ShutdownProps { from: "agent-2".into(), reason: Some("still working".into()), rejected: true });
    insta::assert_snapshot!("shutdown_rejected", s);
}

#[test]
fn advisor_result_collapsed() {
    let s = render_advisor_to_string(AdvisorProps { kind: AdvisorKind::Result { text: "Looks good.".into() }, verbose: false });
    insta::assert_snapshot!("advisor_result_collapsed", s);
}

#[test]
fn advisor_unavailable() {
    let s = render_advisor_to_string(AdvisorProps { kind: AdvisorKind::Error { error_code: "503".into() }, verbose: false });
    insta::assert_snapshot!("advisor_unavailable", s);
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p lingxi-tui --test render_messages_batch1 shutdown advisor`; FAIL.

- [ ] **Step 3: Write the renderers.**

`shutdown.rs`:

```rust
//! `ShutdownMessage` — teammate shutdown request/rejected notice.
//!
//! Literal lock (ShutdownMessage.tsx): request → `Shutdown request from {from}`
//! (warning, bold) + optional `Reason: {reason}`, round warning border.
//! rejected → `Shutdown rejected by {from}` (subtle, bold) + `Reason: {reason}`
//! + `Teammate is continuing to work. You may request shutdown again later.` (dim).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Locked tail line for rejected shutdowns.
pub const REJECTED_TAIL: &str =
    "Teammate is continuing to work. You may request shutdown again later.";

/// Props.
#[derive(Debug, Clone, Default, Props)]
pub struct ShutdownProps {
    /// Originating teammate id.
    pub from: String,
    /// Optional reason.
    pub reason: Option<String>,
    /// `true` → rejected response; `false` → request.
    pub rejected: bool,
}

/// Pure-string renderer.
#[must_use]
pub fn render_shutdown_to_string(props: ShutdownProps) -> String {
    let mut out = if props.rejected {
        format!("Shutdown rejected by {}", props.from)
    } else {
        format!("Shutdown request from {}", props.from)
    };
    if let Some(reason) = &props.reason {
        out.push('\n');
        out.push_str(&format!("Reason: {reason}"));
    }
    if props.rejected {
        out.push('\n');
        out.push_str(REJECTED_TAIL);
    }
    out
}

/// iocraft component. Warning border for requests, subtle for rejected.
#[component]
pub fn ShutdownMessage(props: &ShutdownProps) -> impl Into<AnyElement<'static>> {
    let body = render_shutdown_to_string(props.clone());
    // TODO(M7-15): warning/subtle border via theme; for now color the header.
    let color = if props.rejected { TuiTheme::DIM } else { Color::Yellow };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
        }
    }
}
```

`advisor.rs`:

```rust
//! `AdvisorMessage` — advisor block (server_tool_use / result / error / redacted).
//!
//! Literal lock (AdvisorMessage.tsx). `✔` = figures.tick (U+2714).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::state::AdvisorKind;
use crate::theme::TuiTheme;

/// figures.tick. U+2714.
pub const TICK: &str = "✔";
/// Locked review line (non-verbose result + redacted result).
pub const REVIEWED_LINE: &str =
    "Advisor has reviewed the conversation and will apply the feedback";

/// Props.
#[derive(Debug, Clone, Default, Props)]
pub struct AdvisorProps {
    /// Advisor block content.
    pub kind: AdvisorKind,
    /// `true` → render the full result text (markdown via render::markdown).
    pub verbose: bool,
}

/// Pure-string renderer.
#[must_use]
pub fn render_advisor_to_string(props: AdvisorProps) -> String {
    match &props.kind {
        AdvisorKind::ServerToolUse { model, input } => {
            let mut out = "Advising".to_string();
            if let Some(m) = model {
                out.push_str(&format!(" using {m}"));
            }
            if let Some(i) = input {
                out.push_str(&format!(" \u{00B7} {i}")); // ` · ` middot
            }
            out
        }
        AdvisorKind::Result { text } => {
            if props.verbose {
                text.clone() // markdown body (render::markdown styles it in the component)
            } else {
                format!("{TICK} {REVIEWED_LINE}")
            }
        }
        AdvisorKind::RedactedResult => format!("{TICK} {REVIEWED_LINE}"),
        AdvisorKind::Error { error_code } => format!("Advisor unavailable ({error_code})"),
    }
}

/// iocraft component.
#[component]
pub fn AdvisorMessage(props: &AdvisorProps) -> impl Into<AnyElement<'static>> {
    let body = render_advisor_to_string(props.clone());
    let color = match &props.kind {
        AdvisorKind::Error { .. } => TuiTheme::ERROR,
        AdvisorKind::ServerToolUse { .. } => TuiTheme::DIM, // "Advising" bold in claude-code; dim body parts
        _ => TuiTheme::DIM,
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
        }
    }
}
```

Add `Default for AdvisorKind` to `state.rs` (Props derive): `impl Default for AdvisorKind { fn default() -> Self { Self::RedactedResult } }`.

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p lingxi-tui --test render_messages_batch1 shutdown advisor` + `cargo insta review`. Verify `Shutdown request from agent-2` / `Shutdown rejected by agent-2` + tail, `✔ Advisor has reviewed the conversation and will apply the feedback`, `Advisor unavailable (503)`. Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/shutdown.rs lingxi-core/crates/tui/src/components/messages/advisor.rs lingxi-core/crates/tui/src/state.rs lingxi-core/crates/tui/tests/
git commit -m "$(cat <<'EOF'
plan(M7-04 T8): shutdown + advisor renderers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `hook_progress.rs` (HookProgressMessage)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/hook_progress.rs`
- Test: `render_messages_batch1.rs` (append)

Read first: `HookProgressMessage.tsx`. Running form: `Running ` + `{event}` (bold) + ` hook…`/` hooks…`. Transcript-summary form (Pre/PostToolUse): `{n} ` + `{event}` (bold) + ` hook ran`/` hooks ran`. Singular when count==1.

- [ ] **Step 1: Write the failing tests** — append:

```rust
use lingxi_tui::components::messages::hook_progress::{render_hook_progress_to_string, HookProgressProps};

#[test]
fn hook_progress_running_plural() {
    let s = render_hook_progress_to_string(HookProgressProps { event: "SessionStart".into(), count: 3, transcript_summary: false });
    insta::assert_snapshot!("hook_progress_running_plural", s);
}

#[test]
fn hook_progress_transcript_singular() {
    let s = render_hook_progress_to_string(HookProgressProps { event: "PreToolUse".into(), count: 1, transcript_summary: true });
    insta::assert_snapshot!("hook_progress_transcript_singular", s);
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p lingxi-tui --test render_messages_batch1 hook_progress`; FAIL.

- [ ] **Step 3: Write the renderer** — `hook_progress.rs`:

```rust
//! `HookProgressMessage` — running / transcript-summary hook line.
//!
//! Literal lock (HookProgressMessage.tsx): running → `Running {event} hook…`
//! / `Running {event} hooks…`; transcript summary → `{n} {event} hook ran`
//! / `{n} {event} hooks ran`. Singular when count == 1.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Props.
#[derive(Debug, Clone, Default, Props)]
pub struct HookProgressProps {
    /// Hook event name (e.g. `"PreToolUse"`).
    pub event: String,
    /// In-progress hook count for this event.
    pub count: u32,
    /// `true` → static transcript summary (`{n} … ran`); `false` → live (`Running … …`).
    pub transcript_summary: bool,
}

/// Pure-string renderer.
#[must_use]
pub fn render_hook_progress_to_string(props: HookProgressProps) -> String {
    let event = &props.event;
    if props.transcript_summary {
        let unit = if props.count == 1 { "hook" } else { "hooks" };
        format!("{n} {event} {unit} ran", n = props.count)
    } else {
        let unit = if props.count == 1 { "hook\u{2026}" } else { "hooks\u{2026}" };
        format!("Running {event} {unit}")
    }
}

/// iocraft component — all dim; `{event}` bold.
#[component]
pub fn HookProgressMessage(props: &HookProgressProps) -> impl Into<AnyElement<'static>> {
    // Single dim Text for the whole line (event-bold is a styling nicety the
    // string oracle ignores). If iocraft 0.8.3 allows inline weight runs via
    // nested Text in a Row, render `{event}` with Weight::Bold; otherwise the
    // whole line is dim (acceptable — equivalent look, parity caveat §0 Q3).
    let body = render_hook_progress_to_string(props.clone());
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p lingxi-tui --test render_messages_batch1 hook_progress` + `cargo insta review`. Verify `Running SessionStart hooks…` and `1 PreToolUse hook ran`. Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/hook_progress.rs lingxi-core/crates/tui/tests/
git commit -m "$(cat <<'EOF'
plan(M7-04 T9): hook_progress renderer (running/transcript, singular/plural)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `plan_approval.rs` (PlanApprovalMessage)

**Files:**
- Create: `lingxi-core/crates/tui/src/components/messages/plan_approval.rs`
- Test: `render_messages_batch1.rs` (append)

Read first: `PlanApprovalMessage.tsx`. Request body routes plan content through `render::markdown` (M7-01).

- [ ] **Step 1: Write the failing tests** — append:

```rust
use lingxi_tui::components::messages::plan_approval::{render_plan_approval_to_string, PlanApprovalProps};
use lingxi_tui::state::PlanApprovalKind;

#[test]
fn plan_approval_request() {
    let s = render_plan_approval_to_string(PlanApprovalProps {
        kind: PlanApprovalKind::Request {
            from: "agent-3".into(),
            plan_content: "1. Do X\n2. Do Y".into(),
            plan_file_path: Some("/tmp/plan.md".into()),
        },
    });
    insta::assert_snapshot!("plan_approval_request", s);
}

#[test]
fn plan_approval_approved() {
    let s = render_plan_approval_to_string(PlanApprovalProps {
        kind: PlanApprovalKind::Approved { name: "you".into() },
    });
    insta::assert_snapshot!("plan_approval_approved", s);
}

#[test]
fn plan_approval_rejected() {
    let s = render_plan_approval_to_string(PlanApprovalProps {
        kind: PlanApprovalKind::Rejected { name: "you".into(), feedback: Some("too risky".into()) },
    });
    insta::assert_snapshot!("plan_approval_rejected", s);
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p lingxi-tui --test render_messages_batch1 plan_approval`; FAIL.

- [ ] **Step 3: Write the renderer** — `plan_approval.rs`:

```rust
//! `PlanApprovalMessage` — plan approval request/response.
//!
//! Literal lock (PlanApprovalMessage.tsx): request → `Plan Approval Request
//! from {from}` (planMode, bold) + markdown plan content + `Plan file: {path}`
//! dim, round planMode border. approved → `✓ Plan Approved by {name}`
//! (success, bold) + `You can now proceed with implementation. Your plan mode
//! restrictions have been lifted.`. rejected → `✗ Plan Rejected by {name}`
//! (error, bold) + optional `Feedback: {feedback}` + `Please revise your plan
//! based on the feedback and call ExitPlanMode again.` (dim).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::state::PlanApprovalKind;
use crate::theme::TuiTheme;

/// U+2713 check mark (claude-code `✓`).
pub const CHECK: &str = "✓";
/// U+2717 ballot X (claude-code `✗`).
pub const CROSS: &str = "✗";
/// Locked approved tail.
pub const APPROVED_TAIL: &str =
    "You can now proceed with implementation. Your plan mode restrictions have been lifted.";
/// Locked rejected tail.
pub const REJECTED_TAIL: &str =
    "Please revise your plan based on the feedback and call ExitPlanMode again.";

/// Props.
#[derive(Debug, Clone, Default, Props)]
pub struct PlanApprovalProps {
    /// Request/approved/rejected content.
    pub kind: PlanApprovalKind,
}

/// Pure-string renderer.
#[must_use]
pub fn render_plan_approval_to_string(props: PlanApprovalProps) -> String {
    match &props.kind {
        PlanApprovalKind::Request { from, plan_content, plan_file_path } => {
            let mut out = format!("Plan Approval Request from {from}\n");
            // Plan content is markdown (render::markdown styles it in component).
            out.push_str(plan_content);
            if let Some(p) = plan_file_path {
                out.push('\n');
                out.push_str(&format!("Plan file: {p}"));
            }
            out
        }
        PlanApprovalKind::Approved { name } => {
            format!("{CHECK} Plan Approved by {name}\n{APPROVED_TAIL}")
        }
        PlanApprovalKind::Rejected { name, feedback } => {
            let mut out = format!("{CROSS} Plan Rejected by {name}");
            if let Some(f) = feedback {
                out.push('\n');
                out.push_str(&format!("Feedback: {f}"));
            }
            out.push('\n');
            out.push_str(REJECTED_TAIL);
            out
        }
    }
}

/// iocraft component.
#[component]
pub fn PlanApprovalMessage(props: &PlanApprovalProps) -> impl Into<AnyElement<'static>> {
    let body = render_plan_approval_to_string(props.clone());
    // TODO(M7-15): planMode/success/error borders via theme.
    let color = match &props.kind {
        PlanApprovalKind::Request { .. } => Color::Magenta,
        PlanApprovalKind::Approved { .. } => Color::Green,
        PlanApprovalKind::Rejected { .. } => TuiTheme::ERROR,
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn glyphs_are_check_and_cross() {
        assert_eq!(CHECK, "\u{2713}");
        assert_eq!(CROSS, "\u{2717}");
    }
}
```

Add `Default for PlanApprovalKind` to `state.rs`: `impl Default for PlanApprovalKind { fn default() -> Self { Self::Approved { name: String::new() } } }`.

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p lingxi-tui --test render_messages_batch1 plan_approval` + `cargo insta review`. Verify `Plan Approval Request from agent-3`, `✓ Plan Approved by you` + tail, `✗ Plan Rejected by you` + feedback + tail. Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/plan_approval.rs lingxi-core/crates/tui/src/state.rs lingxi-core/crates/tui/tests/
git commit -m "$(cat <<'EOF'
plan(M7-04 T10): plan_approval renderer (request/approved/rejected)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Wire all 10 dispatch arms + dispatch test

**Files:**
- Modify: `lingxi-core/crates/tui/src/components/messages/mod.rs` (`render_entry_to_string`)
- Modify: `lingxi-core/crates/tui/src/components/scrollback.rs` (`render_message` + `use` imports; remove the Task 4 temporary catch-all)
- Test: `lingxi-core/crates/tui/tests/dispatch_batch1.rs` (create)

- [ ] **Step 1: Write the failing test** — create `dispatch_batch1.rs`:

```rust
//! M7-04 Task 11: every batch-1 variant routes through render_entry_to_string
//! to its renderer's output. Guards the dispatch table.
use lingxi_tui::components::messages::render_entry_to_string;
use lingxi_tui::state::{AdvisorKind, PlanApprovalKind, RenderedMessage, SystemLevel};

#[test]
fn each_variant_routes_to_its_renderer() {
    let cases: Vec<(RenderedMessage, &str)> = vec![
        (RenderedMessage::AssistantThinking { thinking: "x".into(), expanded: false }, "∴ Thinking (ctrl+o to expand)"),
        (RenderedMessage::AssistantRedactedThinking, "✻ Thinking\u{2026}"),
        (RenderedMessage::CompactBoundary { messages_before: 50, messages_after: 5 }, "✻ Conversation compacted (ctrl+o for history)"),
        (RenderedMessage::SystemTextRich { body: "hi".into(), level: SystemLevel::Info }, "hi"),
        (RenderedMessage::RateLimit { text: "limited".into(), upsell: None }, "limited"),
        (RenderedMessage::Advisor { kind: AdvisorKind::Error { error_code: "503".into() }, verbose: false }, "Advisor unavailable (503)"),
        (RenderedMessage::HookProgress { event: "PreToolUse".into(), count: 1, transcript_summary: true }, "1 PreToolUse hook ran"),
        (RenderedMessage::PlanApproval { kind: PlanApprovalKind::Approved { name: "you".into() } },
            "✓ Plan Approved by you\nYou can now proceed with implementation. Your plan mode restrictions have been lifted."),
    ];
    for (msg, expected) in cases {
        assert_eq!(render_entry_to_string(&msg, false, false), expected, "variant: {msg:?}");
    }
}
```

(Also add one assertion each for `SystemApiError` and `Shutdown` matching their Task 6 / Task 8 string output. `AssistantThinking` collapsed uses the `expanded` field on the variant, NOT the dispatcher's `expanded` arg — see Step 3.)

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p lingxi-tui --test dispatch_batch1`; FAIL — `render_entry_to_string` lacks the new arms (and `scrollback.rs` still has the temporary catch-all hiding non-exhaustiveness).

- [ ] **Step 3: Add the match arms** in `messages/mod.rs::render_entry_to_string` (after the existing arms; import the `render_*_to_string` fns + props structs at the top):

```rust
        RenderedMessage::AssistantThinking { thinking, expanded } => {
            thinking::render_thinking_to_string(thinking::ThinkingProps {
                thinking: thinking.clone(),
                expanded: *expanded,
            })
        }
        RenderedMessage::AssistantRedactedThinking => {
            redacted_thinking::render_redacted_thinking_to_string()
        }
        RenderedMessage::CompactBoundary { .. } => {
            compact_boundary::render_compact_boundary_to_string()
        }
        RenderedMessage::SystemTextRich { body, level } => {
            system_text::render_system_text_to_string(system_text::SystemTextProps {
                body: body.clone(),
                level: *level,
            })
        }
        RenderedMessage::SystemApiError { error, retry_attempt, retry_in_seconds, max_retries, truncated } => {
            system_api_error::render_system_api_error_to_string(system_api_error::SystemApiErrorProps {
                error: error.clone(),
                retry_attempt: *retry_attempt,
                retry_in_seconds: *retry_in_seconds,
                max_retries: *max_retries,
                truncated: *truncated,
            })
        }
        RenderedMessage::RateLimit { text, upsell } => {
            rate_limit::render_rate_limit_to_string(rate_limit::RateLimitProps {
                text: text.clone(),
                upsell: upsell.clone(),
            })
        }
        RenderedMessage::Shutdown { from, reason, rejected } => {
            shutdown::render_shutdown_to_string(shutdown::ShutdownProps {
                from: from.clone(),
                reason: reason.clone(),
                rejected: *rejected,
            })
        }
        RenderedMessage::Advisor { kind, verbose } => {
            advisor::render_advisor_to_string(advisor::AdvisorProps {
                kind: kind.clone(),
                verbose: *verbose,
            })
        }
        RenderedMessage::HookProgress { event, count, transcript_summary } => {
            hook_progress::render_hook_progress_to_string(hook_progress::HookProgressProps {
                event: event.clone(),
                count: *count,
                transcript_summary: *transcript_summary,
            })
        }
        RenderedMessage::PlanApproval { kind } => {
            plan_approval::render_plan_approval_to_string(plan_approval::PlanApprovalProps {
                kind: kind.clone(),
            })
        }
```

In `scrollback.rs::render_message`, **remove the Task 4 temporary catch-all** and add the 10 iocraft-component arms (mirroring the existing `AssistantToolUse`/`UserToolResult` arms; add the component imports to the `use` block). Each builds the matching component, e.g.:

```rust
        RenderedMessage::AssistantThinking { thinking, expanded } => element! {
            AssistantThinkingMessage(thinking: thinking, expanded: expanded)
        }.into_any(),
        // ... 9 more, one per variant ...
```

Verify the match is now exhaustive with no `_` arm (the compiler enforces this once the catch-all is gone).

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p lingxi-tui --test dispatch_batch1`; expected PASS. Then `cargo build -p lingxi-tui` to confirm `scrollback.rs` compiles exhaustively.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/tui/src/components/messages/mod.rs lingxi-core/crates/tui/src/components/scrollback.rs lingxi-core/crates/tui/tests/dispatch_batch1.rs
git commit -m "$(cat <<'EOF'
plan(M7-04 T11): wire 10 dispatch arms (both dispatchers) + dispatch test

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Workspace verification gate + tag `m7.4`

**Files:** none (verification + tag only)

- [ ] **Step 1: Format + lint + test, from inside `lingxi-core/`** (toolchain pins 1.82; running from repo root uses host toolchain → spurious lint noise — this bit M6-08):

```bash
cd lingxi-core && cargo fmt --check
cd lingxi-core && cargo clippy --workspace --all-targets -- -D warnings
cd lingxi-core && cargo test --workspace
```

Expected: clean fmt, zero clippy warnings, all tests pass. **Known flakes (rerun the single test, don't treat as failure):** `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, and `lingxi-platform-posix` fs_watch FSEvents timing tests. If one trips, rerun it alone to confirm it's the known flake.

- [ ] **Step 2: Telemetry baseline unchanged** — confirm M7-04 added zero events:

```bash
cd lingxi-core && cargo test --workspace all_event_names 2>&1 | tail -5
```

Expected: the `ALL_EVENT_NAMES` count assertion still reads 326 (no change). If any event-count test fails, you accidentally registered a telemetry name — revert it.

- [ ] **Step 3: Cross-platform compile gate (5 targets)**:

```bash
cd lingxi-core && cargo check --workspace --target x86_64-unknown-linux-gnu
cd lingxi-core && cargo check --workspace --target x86_64-apple-darwin
cd lingxi-core && cargo check --workspace --target x86_64-pc-windows-gnu
cd lingxi-core && cargo check --workspace --target aarch64-linux-android
cd lingxi-core && cargo check --workspace --target aarch64-apple-ios
```

Expected: all green (same posture as v0.6.0/v0.7.0). If a target toolchain isn't installed, note it and run the ones available — the renderers are pure Rust + iocraft so no target-specific code is introduced.

- [ ] **Step 4: Tag the sub-plan** (annotated, local only — no push):

```bash
git tag -a m7.4 -m "M7-04: message renderers batch 1 (system/assistant, 10 renderers)"
git tag -l m7.4
```

Expected: `m7.4` listed.

- [ ] **Step 5: Final commit (if the gate produced any fmt/snapshot fixups)** — only if Step 1 changed files:

```bash
git add -A
git commit -m "$(cat <<'EOF'
plan(M7-04 T12): workspace gate green; tag m7.4

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**1. Spec coverage (§3 M7-04 entry):** 10 renderers each get a file + variant + renderer fn + dispatch entry (Tasks 2-3, 5-10 build them; Task 1 adds variants; Task 11 wires dispatch). `compact_boundary` replaces the `[Compacted]` SystemText placeholder (Task 4). Snapshots per renderer with collapsed/expanded where applicable (thinking T2; system_api_error truncated/not T6; hook_progress running/transcript T9; plan_approval request/approved/rejected T10) — 15+ snapshots total. Dispatch test (T11). Markdown routing noted for thinking/advisor/plan-approval (M7-01 prereq). Telemetry +0 (T12 verifies 326). Workspace gate from inside `lingxi-core/` + tag `m7.4` (T12). All covered.

**2. Placeholder scan:** No TBD/TODO-as-work. The `// TODO(M7-15)` comments are intentional forward-pointers for theme constants (warning/success/planMode colors centralize in M7-15), not plan gaps — each renderer ships a concrete literal iocraft color now.

**3. Type consistency:** `RenderedMessage` variant names (Task 1) match dispatch arms (Task 4, Task 11) and tests (Tasks 2-11): `AssistantThinking`, `AssistantRedactedThinking`, `CompactBoundary`, `SystemTextRich`, `SystemApiError`, `RateLimit`, `Shutdown`, `Advisor`, `HookProgress`, `PlanApproval`. Supporting enums `SystemLevel`/`AdvisorKind`/`PlanApprovalKind` defined in Task 1, used consistently. Props struct names (`ThinkingProps`, `SystemTextProps`, `SystemApiErrorProps`, `RateLimitProps`, `ShutdownProps`, `AdvisorProps`, `HookProgressProps`, `PlanApprovalProps`) match across renderer + dispatch + tests. `render_<name>_to_string` fn names consistent. `Default` impls added for the three enums (Tasks 1/5/8/10) since Props derives `Default`.

**Open items the implementer must confirm against the live tree (flagged, not gaps):**
- `StyledLine` real type name (M7-01) — grep before use (Prerequisites).
- iocraft `0.8.3` `Text` attributes: `italic`, `weight`, and `Option<Element>` interpolation in `#(...)` — check `assistant_text.rs`/`user_tool_result.rs` and the prelude; degrade gracefully (drop attribute / branch the element) if absent.
- The exact CtrlOToExpand surface string in this codebase — claude-code renders a `<CtrlOToExpand />` component; the literal `(ctrl+o to expand)` here is the spec-intent rendering. If M6 already established a different CtrlOToExpand literal, reuse it (grep `ctrl+o`).
