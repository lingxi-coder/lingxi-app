# M9-03 — Team Message Renderers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the 6 multi-agent transcript renderers from claude-code's `components/messages/` (TaskAssignment, UserTeammate, UserAgentNotification, UserChannel, teamMemCollapsed, teamMemSaved) to the LingXi TUI, literal-locked to claude-code's output.

**Architecture:** Each renderer follows the existing `tui/src/components/messages/` pattern: a pure `render_*_to_string(props) -> String` (byte-locked, unit-tested) + a `#[component]` that delegates to it and applies iocraft colors. New `RenderedMessage` variants carry **structured** fields (mirroring claude-code's *parsed* shapes); the dispatch arm in `messages/mod.rs::render_entry_to_string` formats them. Two of the six (teamMemCollapsed, teamMemSaved) extend existing surfaces (`collapsed_read_search.rs`, a new system-line segment helper) rather than adding new message variants — faithful to claude-code, where those are *parts* not standalone messages.

**Scope boundary (important):** M9-03 is the **rendering** sub-plan. The *classification* of a raw `coordinator::mailbox::TeammateMessage.content` string into the right variant (plan-approval / shutdown / task-assignment / task-completed / idle-suppression) is a **drain** concern deferred to **M9-06** (mailbox drain). M9-03 provides the structured variants + renderers; snapshot/unit tests construct the variants directly. Two teammate sub-types reuse renderers that **already exist**: plan-approval → `plan_approval.rs` (`RenderedMessage::PlanApproval`), shutdown → `shutdown.rs` (`RenderedMessage::Shutdown`). `idle_notification` renders nothing (suppressed at the drain). So the genuinely new teammate rendering here is **task-completed** + **plain-note**.

**Tech Stack:** Rust 1.82.0 (pinned), `iocraft = "=0.8.3"`, `insta` snapshots. Run all cargo from inside `lingxi-code/`.

**Literal-lock reference source:** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/messages/` — every implementer reads the matching `.tsx` first and copies exact literals. Glyph constants are in `claude-code/src/constants/figures.ts`.

**Locked glyph constants (verified from `figures.ts`):**
- `BLACK_CIRCLE` = `⏺` (darwin) / `●` (else). **We lock the non-darwin `●` = U+25CF**, matching the existing `system_text::MARKER` precedent.
- `CHANNEL_ARROW` = `←` = U+2190.
- `figures.pointer` = `❯` = U+276F (the `@name❯` teammate header).
- task-completed check = `✓` = U+2713 (same as the existing `plan_approval::CHECK`).
- middot separator (channel) = `·` = U+00B7.

---

## File Structure

| File | Responsibility | Create/Modify |
|---|---|---|
| `tui/src/multiagent/style.rs` | + `agent_color_from_name(&str) -> Color` (name→AgentColor→Color, cyan fallback) | Modify |
| `tui/src/components/messages/task_assignment.rs` | TaskAssignment renderer (round cyan border) | Create |
| `tui/src/components/messages/user_agent_notification.rs` | UserAgentNotification renderer (`● summary`, status color) | Create |
| `tui/src/components/messages/user_channel.rs` | UserChannel renderer (`← server · user: content`) | Create |
| `tui/src/components/messages/user_teammate.rs` | UserTeammate renderer (TaskCompleted + Note sub-types) | Create |
| `tui/src/components/messages/team_mem_saved.rs` | `team_mem_saved_segment(u64) -> Option<String>` helper | Create |
| `tui/src/components/messages/collapsed_read_search.rs` | + team-memory counts/parts (teamMemCollapsed) | Modify |
| `tui/src/components/messages/mod.rs` | + `pub mod` decls + dispatch arms | Modify |
| `tui/src/state.rs` | + `TaskAssignment`/`AgentNotification`/`ChannelMessage`/`UserTeammate` variants + `UserTeammateKind` enum + `CollapsedReadSearch` mem fields | Modify |
| `tui/tests/render_messages.rs` | + insta component snapshots | Modify |

**Variant-derive note:** `RenderedMessage` carries a `serde_json::Value` (in `AssistantToolUse`), so it does **not** derive `Eq`/`PartialEq`. New variant fields are all `String`/`Option<String>`/`bool`/`u64` — they don't constrain its derives. The new `UserTeammateKind` enum derives `Debug, Clone` only (match the parent).

**Non-exhaustive-match note:** Adding a `RenderedMessage` variant forces every exhaustive `match` on it to gain an arm. The dispatch in `messages/mod.rs` is the main one. If `cargo build` flags others (e.g. in `virtual_message_list.rs` or a height estimator), add a minimal arm following the existing neighboring pattern — do not restructure.

---

## Task 1: `agent_color_from_name` styling primitive

**Files:**
- Modify: `tui/src/multiagent/style.rs` (append a function + test)

- [ ] **Step 1: Write the failing test** — append to the `#[cfg(test)] mod tests` block in `style.rs`:

```rust
    #[test]
    fn agent_color_from_name_maps_known_and_falls_back_to_cyan() {
        // Known names resolve to the same Color as the enum path.
        assert_eq!(agent_color_from_name("magenta"), agent_color(AgentColor::Magenta));
        assert_eq!(agent_color_from_name("Orange"), agent_color(AgentColor::Orange));
        assert_eq!(agent_color_from_name("teal"), agent_color(AgentColor::Teal));
        // Unknown / empty → cyan fallback (claude-code cyan_FOR_SUBAGENTS_ONLY).
        assert_eq!(agent_color_from_name("chartreuse"), agent_color(AgentColor::Cyan));
        assert_eq!(agent_color_from_name(""), agent_color(AgentColor::Cyan));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tui --lib multiagent::style::tests::agent_color_from_name`
Expected: FAIL — `cannot find function agent_color_from_name`.

- [ ] **Step 3: Write minimal implementation** — add to `style.rs` (after `agent_color`):

```rust
/// Map a claude-code agent **color name** (e.g. `"cyan"`, `"orange"`) to a
/// render [`Color`]. Case-insensitive. Unknown / empty names fall back to
/// cyan — claude-code's `cyan_FOR_SUBAGENTS_ONLY` default (`toInkColor`).
#[must_use]
pub fn agent_color_from_name(name: &str) -> Color {
    let c = match name.to_ascii_lowercase().as_str() {
        "magenta" => AgentColor::Magenta,
        "yellow" => AgentColor::Yellow,
        "green" => AgentColor::Green,
        "blue" => AgentColor::Blue,
        "red" => AgentColor::Red,
        "orange" => AgentColor::Orange,
        "purple" => AgentColor::Purple,
        "pink" => AgentColor::Pink,
        "teal" => AgentColor::Teal,
        // "cyan" and anything unknown → cyan.
        _ => AgentColor::Cyan,
    };
    agent_color(c)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p tui --lib multiagent::style::tests::agent_color_from_name`
Expected: PASS.

- [ ] **Step 5: Re-export** — in `tui/src/multiagent/mod.rs`, add `agent_color_from_name` to the existing `pub use style::{...}` re-export list (keep alphabetical within the list).

- [ ] **Step 6: Commit**

```bash
cargo fmt -p tui
git add lingxi-code/tui/src/multiagent/style.rs lingxi-code/tui/src/multiagent/mod.rs
git commit -m "feat(M9-03): agent_color_from_name color-name → Color primitive"
```

---

## Task 2: TaskAssignment renderer

claude-code: `components/messages/TaskAssignmentMessage.tsx` (`TaskAssignmentDisplay`). Round border in subagent-cyan; bold header `Task #{taskId} assigned by {assignedBy}`; bold subject line; optional dim description line.

**Files:**
- Create: `tui/src/components/messages/task_assignment.rs`
- Modify: `tui/src/components/messages/mod.rs`, `tui/src/state.rs`, `tui/tests/render_messages.rs`

- [ ] **Step 1: Read the reference** — read `claude-code/src/components/messages/TaskAssignmentMessage.tsx` and confirm the exact header/subject/description literals and ordering.

- [ ] **Step 2: Add the variant** — in `tui/src/state.rs`, add to the `RenderedMessage` enum (after the `Shutdown { .. }` variant, keeping the multi-agent group together):

```rust
    /// (M9-03) Task assignment notice — claude-code `TaskAssignmentMessage`.
    TaskAssignment {
        /// Task id, rendered as `#{task_id}`.
        task_id: String,
        /// Assigning agent's name.
        assigned_by: String,
        /// Task subject / title.
        subject: String,
        /// Optional task description.
        description: Option<String>,
    },
```

- [ ] **Step 3: Write the renderer file** — create `tui/src/components/messages/task_assignment.rs`:

```rust
//! `TaskAssignmentMessage` — task assignment notice.
//!
//! Literal lock (claude-code `TaskAssignmentMessage.tsx`): round
//! subagent-cyan border; bold header `Task #{task_id} assigned by
//! {assigned_by}`; bold subject line; optional dim description line.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::multiagent::style::{agent_color, AgentColor};
use crate::theme::Theme;

/// Props for [`TaskAssignmentMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct TaskAssignmentProps {
    /// Task id (rendered `#{task_id}`).
    pub task_id: String,
    /// Assigning agent name.
    pub assigned_by: String,
    /// Task subject / title.
    pub subject: String,
    /// Optional description.
    pub description: Option<String>,
    /// Active palette.
    pub theme: Theme,
}

/// Pure-string renderer (border applied by the component). Lines: header,
/// subject, optional description.
#[must_use]
pub fn render_task_assignment_to_string(props: TaskAssignmentProps) -> String {
    let mut out = format!(
        "Task #{} assigned by {}\n{}",
        props.task_id, props.assigned_by, props.subject
    );
    if let Some(desc) = &props.description {
        out.push('\n');
        out.push_str(desc);
    }
    out
}

/// iocraft component. Round cyan border; bold header + subject; dim
/// description.
#[component]
pub fn TaskAssignmentMessage(props: &TaskAssignmentProps) -> impl Into<AnyElement<'static>> {
    let cyan = agent_color(AgentColor::Cyan);
    let header = format!(
        "Task #{} assigned by {}",
        props.task_id, props.assigned_by
    );
    let subject = props.subject.clone();
    let description = props.description.clone();
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: cyan,
        ) {
            Text(content: header, color: cyan, weight: Weight::Bold)
            Text(content: subject, weight: Weight::Bold)
            #(description.map(|d| element! {
                Text(content: d, color: props.theme.dim)
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_subject_only() {
        let out = render_task_assignment_to_string(TaskAssignmentProps {
            task_id: "123".into(),
            assigned_by: "alice".into(),
            subject: "Set up DB".into(),
            description: None,
            theme: Theme::dark(),
        });
        assert_eq!(out, "Task #123 assigned by alice\nSet up DB");
    }

    #[test]
    fn with_description() {
        let out = render_task_assignment_to_string(TaskAssignmentProps {
            task_id: "7".into(),
            assigned_by: "lead".into(),
            subject: "Migrate".into(),
            description: Some("Move tables".into()),
            theme: Theme::dark(),
        });
        assert_eq!(out, "Task #7 assigned by lead\nMigrate\nMove tables");
    }
}
```

- [ ] **Step 4: Wire the module + dispatch** — in `tui/src/components/messages/mod.rs`:
  1. Add `pub mod task_assignment;` (alphabetical: after `system_text;`? no — alphabetical order; place between `system_text` and `thinking` is wrong — `task_assignment` sorts after `system_text` and before `thinking`. Insert accordingly.)
  2. Add a dispatch arm in `render_entry_to_string`:

```rust
        RenderedMessage::TaskAssignment {
            task_id,
            assigned_by,
            subject,
            description,
        } => task_assignment::render_task_assignment_to_string(
            task_assignment::TaskAssignmentProps {
                task_id: task_id.clone(),
                assigned_by: assigned_by.clone(),
                subject: subject.clone(),
                description: description.clone(),
                theme: crate::theme::Theme::dark(),
            },
        ),
```

- [ ] **Step 5: Run unit + build**

Run: `cargo test -p tui --lib components::messages::task_assignment`
Expected: PASS. Then `cargo build -p tui` — if other non-exhaustive matches on `RenderedMessage` surface, add minimal arms (see header note) and rebuild.

- [ ] **Step 6: Add component snapshots** — append to `tui/tests/render_messages.rs`:

```rust
#[test]
fn task_assignment_no_description() {
    let mut element = element! {
        tui::components::messages::task_assignment::TaskAssignmentMessage(
            task_id: "123".to_string(),
            assigned_by: "alice".to_string(),
            subject: "Set up DB".to_string(),
            description: None,
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("task_assignment_no_description", element.to_string());
}

#[test]
fn task_assignment_with_description() {
    let mut element = element! {
        tui::components::messages::task_assignment::TaskAssignmentMessage(
            task_id: "7".to_string(),
            assigned_by: "lead".to_string(),
            subject: "Migrate".to_string(),
            description: Some("Move tables".to_string()),
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("task_assignment_with_description", element.to_string());
}
```

Run: `cargo test -p tui --test render_messages task_assignment` then review the new `.snap` files with `cargo insta review` (or inspect `tests/snapshots/*.snap.new`) and accept once correct.

- [ ] **Step 7: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-03): TaskAssignment message renderer + variant + dispatch"
```

---

## Task 3: UserAgentNotification renderer

claude-code: `components/messages/UserAgentNotificationMessage.tsx`. Renders `{BLACK_CIRCLE} {summary}`; the circle is colored by status (`completed`→success, `failed`→error, `killed`→warning, else→text). Empty summary → render nothing.

**Files:**
- Create: `tui/src/components/messages/user_agent_notification.rs`
- Modify: `tui/src/components/messages/mod.rs`, `tui/src/state.rs`, `tui/tests/render_messages.rs`

- [ ] **Step 1: Read the reference** — read `UserAgentNotificationMessage.tsx`; confirm the marker glyph, the status→color map, and the null-on-empty-summary behavior.

- [ ] **Step 2: Add the variant** — in `state.rs`, after `TaskAssignment`:

```rust
    /// (M9-03) Background-agent notification — claude-code
    /// `UserAgentNotificationMessage`. `status`: completed/failed/killed/other
    /// → marker color.
    AgentNotification {
        /// Summary line (empty → renders nothing).
        summary: String,
        /// Optional status string.
        status: Option<String>,
    },
```

- [ ] **Step 3: Write the renderer file** — create `tui/src/components/messages/user_agent_notification.rs`:

```rust
//! `UserAgentNotificationMessage` — background-agent status notification.
//!
//! Literal lock (claude-code `UserAgentNotificationMessage.tsx`):
//! `{BLACK_CIRCLE} {summary}` where the circle's color is the status color
//! (completed→success, failed→error, killed→warning, else→text). Empty
//! summary renders nothing (claude-code returns null). We lock the non-darwin
//! `BLACK_CIRCLE` = `●` (U+25CF), matching `system_text::MARKER`.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::Theme;

/// `● ` marker (`BLACK_CIRCLE` non-darwin form, U+25CF + space).
pub const MARKER: &str = "\u{25CF} ";

/// Props for [`UserAgentNotificationMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserAgentNotificationProps {
    /// Summary line.
    pub summary: String,
    /// Optional status string.
    pub status: Option<String>,
    /// Active palette.
    pub theme: Theme,
}

/// Marker color for a status string.
#[must_use]
pub fn status_color(status: Option<&str>, theme: &Theme) -> Color {
    match status {
        Some("completed") => theme.success,
        Some("failed") => theme.error,
        Some("killed") => theme.warning,
        _ => theme.text,
    }
}

/// Pure-string renderer. Empty summary → empty string.
#[must_use]
pub fn render_user_agent_notification_to_string(props: UserAgentNotificationProps) -> String {
    if props.summary.is_empty() {
        return String::new();
    }
    format!("{MARKER}{}", props.summary)
}

/// iocraft component. Marker colored by status; summary in default text.
#[component]
pub fn UserAgentNotificationMessage(
    props: &UserAgentNotificationProps,
) -> impl Into<AnyElement<'static>> {
    if props.summary.is_empty() {
        return element! { View {} }.into_any();
    }
    let marker_color = status_color(props.status.as_deref(), &props.theme);
    let summary = props.summary.clone();
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: MARKER, color: marker_color)
            Text(content: summary, color: props.theme.text)
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bytes() {
        // ● = U+25CF = 0xE2 0x97 0x8F, then ASCII space.
        assert_eq!(MARKER.as_bytes(), &[0xE2, 0x97, 0x8F, 0x20]);
    }

    #[test]
    fn renders_marker_and_summary() {
        let out = render_user_agent_notification_to_string(UserAgentNotificationProps {
            summary: "Task done".into(),
            status: Some("completed".into()),
            theme: Theme::dark(),
        });
        assert_eq!(out, "\u{25CF} Task done");
    }

    #[test]
    fn empty_summary_is_empty() {
        let out = render_user_agent_notification_to_string(UserAgentNotificationProps {
            summary: String::new(),
            status: None,
            theme: Theme::dark(),
        });
        assert_eq!(out, "");
    }

    #[test]
    fn status_color_map() {
        let t = Theme::dark();
        assert_eq!(status_color(Some("completed"), &t), t.success);
        assert_eq!(status_color(Some("failed"), &t), t.error);
        assert_eq!(status_color(Some("killed"), &t), t.warning);
        assert_eq!(status_color(Some("other"), &t), t.text);
        assert_eq!(status_color(None, &t), t.text);
    }
}
```

- [ ] **Step 4: Wire the module + dispatch** — in `messages/mod.rs`: add `pub mod user_agent_notification;` (alphabetical) and a dispatch arm. The dispatch passes a theme; use the active theme the dispatcher already has access to, or `Theme::dark()` if the dispatcher is theme-less (check how the existing `SystemText` arm obtains its theme — match that). If `render_entry_to_string` has no theme in scope, use `Theme::dark()` for the string form (color is irrelevant to the string output):

```rust
        RenderedMessage::AgentNotification { summary, status } => {
            user_agent_notification::render_user_agent_notification_to_string(
                user_agent_notification::UserAgentNotificationProps {
                    summary: summary.clone(),
                    status: status.clone(),
                    theme: crate::theme::Theme::dark(),
                },
            )
        }
```

- [ ] **Step 5: Run unit + build**

Run: `cargo test -p tui --lib components::messages::user_agent_notification` then `cargo build -p tui`.
Expected: PASS / clean build.

- [ ] **Step 6: Add component snapshot** — append to `tui/tests/render_messages.rs`:

```rust
#[test]
fn user_agent_notification_completed() {
    let mut element = element! {
        tui::components::messages::user_agent_notification::UserAgentNotificationMessage(
            summary: "Background task finished".to_string(),
            status: Some("completed".to_string()),
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("user_agent_notification_completed", element.to_string());
}
```

Run the test, review + accept the snapshot.

- [ ] **Step 7: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-03): UserAgentNotification message renderer + variant + dispatch"
```

---

## Task 4: UserChannel renderer

claude-code: `components/messages/UserChannelMessage.tsx`. Renders `{CHANNEL_ARROW} {serverLeaf}[ · {user}]: {content}`; arrow in `suggestion` color, server/user dim, content default; content whitespace-collapsed and truncated to 60. `serverLeaf` = substring after the last `:` of the source. Regex-mismatch → render nothing.

**Files:**
- Create: `tui/src/components/messages/user_channel.rs`
- Modify: `tui/src/components/messages/mod.rs`, `tui/src/state.rs`, `tui/tests/render_messages.rs`

- [ ] **Step 1: Read the reference** — read `UserChannelMessage.tsx`; confirm arrow glyph, the `· ` separator (U+00B7), the `:` placement, `displayServerName` (leaf after last `:`), and `truncateToWidth(body, 60)`.

- [ ] **Step 2: Add the variant** — in `state.rs`, after `AgentNotification`:

```rust
    /// (M9-03) Inbound channel message — claude-code `UserChannelMessage`.
    ChannelMessage {
        /// Source server (raw; renderer takes the leaf after the last `:`).
        server: String,
        /// Optional sender user.
        user: Option<String>,
        /// Message content (renderer collapses whitespace + truncates to 60).
        content: String,
    },
```

- [ ] **Step 3: Write the renderer file** — create `tui/src/components/messages/user_channel.rs`:

```rust
//! `UserChannelMessage` — inbound channel message.
//!
//! Literal lock (claude-code `UserChannelMessage.tsx`):
//! `{CHANNEL_ARROW} {serverLeaf}[ · {user}]: {content}` — arrow in
//! `suggestion`, server/user dim, content default. `serverLeaf` is the
//! substring after the last `:` of the source (e.g. `plugin:slack:slack` →
//! `slack`). Content has whitespace collapsed and is truncated to 60 with `…`.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::Theme;

/// `← ` inbound-channel arrow (U+2190 + space).
pub const ARROW: &str = "\u{2190} ";
/// ` · ` user separator (space + U+00B7 + space).
pub const MIDDOT: &str = " \u{00B7} ";

/// Leaf server name: substring after the last `:` (or the whole string).
#[must_use]
pub fn display_server_name(source: &str) -> &str {
    match source.rfind(':') {
        Some(i) => &source[i + 1..],
        None => source,
    }
}

/// Collapse runs of whitespace to a single space and trim.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to `max` chars, appending `…` when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// Props for [`UserChannelMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserChannelProps {
    /// Source server (raw).
    pub server: String,
    /// Optional sender user.
    pub user: Option<String>,
    /// Message content.
    pub content: String,
    /// Active palette.
    pub theme: Theme,
}

/// The dim middle segment: `serverLeaf[ · user]: `.
fn middle_segment(server: &str, user: Option<&str>) -> String {
    let leaf = display_server_name(server);
    match user {
        Some(u) => format!("{leaf}{MIDDOT}{u}: "),
        None => format!("{leaf}: "),
    }
}

/// Pure-string renderer.
#[must_use]
pub fn render_user_channel_to_string(props: UserChannelProps) -> String {
    let mid = middle_segment(&props.server, props.user.as_deref());
    let body = truncate_chars(&collapse_ws(&props.content), 60);
    format!("{ARROW}{mid}{body}")
}

/// iocraft component. Arrow (suggestion) + dim middle + default content.
#[component]
pub fn UserChannelMessage(props: &UserChannelProps) -> impl Into<AnyElement<'static>> {
    let mid = middle_segment(&props.server, props.user.as_deref());
    let body = truncate_chars(&collapse_ws(&props.content), 60);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: ARROW, color: props.theme.suggestion)
            Text(content: mid, color: props.theme.dim)
            Text(content: body, color: props.theme.text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_and_middot_bytes() {
        assert_eq!(ARROW.as_bytes(), &[0xE2, 0x86, 0x90, 0x20]); // ← + space
        assert_eq!(MIDDOT.as_bytes(), &[0x20, 0xC2, 0xB7, 0x20]); // sp · sp
    }

    #[test]
    fn server_leaf() {
        assert_eq!(display_server_name("plugin:slack:slack"), "slack");
        assert_eq!(display_server_name("slack"), "slack");
    }

    #[test]
    fn with_user() {
        let out = render_user_channel_to_string(UserChannelProps {
            server: "plugin:slack:slack".into(),
            user: Some("bob".into()),
            content: "hello   there".into(),
            theme: Theme::dark(),
        });
        assert_eq!(out, "\u{2190} slack \u{00B7} bob: hello there");
    }

    #[test]
    fn without_user_and_truncation() {
        let long = "x".repeat(80);
        let out = render_user_channel_to_string(UserChannelProps {
            server: "irc".into(),
            user: None,
            content: long,
            theme: Theme::dark(),
        });
        // 59 x's + ellipsis.
        assert_eq!(out, format!("\u{2190} irc: {}\u{2026}", "x".repeat(59)));
    }
}
```

- [ ] **Step 4: Wire the module + dispatch** — `messages/mod.rs`: add `pub mod user_channel;` (alphabetical) and a dispatch arm (theme via `Theme::dark()` for the string form, same as Task 3):

```rust
        RenderedMessage::ChannelMessage { server, user, content } => {
            user_channel::render_user_channel_to_string(user_channel::UserChannelProps {
                server: server.clone(),
                user: user.clone(),
                content: content.clone(),
                theme: crate::theme::Theme::dark(),
            })
        }
```

- [ ] **Step 5: Run unit + build**

Run: `cargo test -p tui --lib components::messages::user_channel` then `cargo build -p tui`.

- [ ] **Step 6: Add component snapshot** — append to `render_messages.rs`:

```rust
#[test]
fn user_channel_with_user() {
    let mut element = element! {
        tui::components::messages::user_channel::UserChannelMessage(
            server: "plugin:slack:slack".to_string(),
            user: Some("bob".to_string()),
            content: "deploy is green".to_string(),
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("user_channel_with_user", element.to_string());
}
```

Run, review, accept.

- [ ] **Step 7: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-03): UserChannel message renderer + variant + dispatch"
```

---

## Task 5: UserTeammate renderer (TaskCompleted + Note)

claude-code: `components/messages/UserTeammateMessage.tsx`. The `@{displayName}❯` header is colored by the teammate's agent color. **TaskCompleted:** `@name❯ ✓ Completed task #{taskId}` + optional ` ({taskSubject})` (dim). **Note (default plaintext):** `@name❯` + optional ` {summary}`; when transcript mode, the full content follows, each line indented 2. (plan-approval/shutdown sub-types reuse existing renderers via the M9-06 drain; `idle_notification` is suppressed — out of scope here.)

**Files:**
- Create: `tui/src/components/messages/user_teammate.rs`
- Modify: `tui/src/components/messages/mod.rs`, `tui/src/state.rs`, `tui/tests/render_messages.rs`

- [ ] **Step 1: Read the reference** — read `UserTeammateMessage.tsx` (the task-completed branch ~lines 118–136 and `TeammateMessageContent` ~lines 150+). The layout below is already reconciled to the source: **task-completed is TWO lines** — the `@name❯` header on its own line (the outer `Box` is `flexDirection="column"`), then a `MessageResponse`-wrapped line. `MessageResponse` (claude-code `MessageResponse.tsx`) prepends the gutter `  ⎿  ` (2 spaces + U+23BF + 2 spaces), so line 2 is `  ⎿  ✓ Completed task #{task_id}` + optional ` ({task_subject})` (dim). **Note** is one line `@name❯ {summary}` (inner `Box` is row; summary `<Text>` has a leading space) with no gutter; transcript mode adds the content in a `paddingLeft={2}` box. Confirm these against the source; the code below already encodes them.

- [ ] **Step 2: Add the variant + kind enum** — in `state.rs`, add the enum near the other message-payload enums (e.g. next to `PlanApprovalKind`):

```rust
/// (M9-03) `UserTeammate` sub-type payloads (claude-code
/// `UserTeammateMessage`). plan-approval/shutdown reuse the existing
/// `PlanApproval`/`Shutdown` variants; `idle_notification` is suppressed at
/// the drain (M9-06), so it has no payload here.
#[derive(Debug, Clone)]
pub enum UserTeammateKind {
    /// `✓ Completed task #{task_id}` + optional ` ({task_subject})`.
    TaskCompleted {
        /// Completed task id.
        task_id: String,
        /// Optional task subject.
        task_subject: Option<String>,
    },
    /// Plain teammate note: optional summary + optional full content
    /// (shown indented when `is_transcript_mode`).
    Note {
        /// Optional one-line summary.
        summary: Option<String>,
        /// Optional full content.
        content: Option<String>,
        /// `true` → show full content (transcript mode).
        is_transcript_mode: bool,
    },
}
```

And add to `RenderedMessage` (after `ChannelMessage`):

```rust
    /// (M9-03) Teammate message — claude-code `UserTeammateMessage`
    /// (task-completed + plain-note sub-types).
    UserTeammate {
        /// Display name (`leader` or teammate id).
        display_name: String,
        /// Optional agent color name (→ `agent_color_from_name`).
        color: Option<String>,
        /// Sub-type payload.
        kind: UserTeammateKind,
    },
```

- [ ] **Step 3: Write the renderer file** — create `tui/src/components/messages/user_teammate.rs`:

```rust
//! `UserTeammateMessage` — teammate transcript message.
//!
//! Literal lock (claude-code `UserTeammateMessage.tsx`): `@{display_name}❯`
//! header in the teammate's agent color. TaskCompleted → TWO lines: the header
//! line, then a `MessageResponse`-guttered line
//! `  ⎿  ✓ Completed task #{task_id}` + optional ` ({task_subject})` (dim).
//! Note → `@name❯` + optional ` {summary}` (same line); transcript mode
//! appends the full content, each line indented 2. (plan-approval/shutdown
//! sub-types reuse the existing renderers; `idle_notification` is suppressed
//! upstream.)
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::multiagent::style::agent_color_from_name;
use crate::state::UserTeammateKind;
use crate::theme::Theme;

/// `❯` teammate-header pointer (U+276F).
pub const POINTER: &str = "\u{276F}";
/// `✓` completed check (U+2713).
pub const CHECK: &str = "\u{2713}";
/// `  ⎿  ` MessageResponse gutter (2 spaces + U+23BF + 2 spaces) — prefixes
/// the task-completed line (claude-code `MessageResponse.tsx`).
pub const GUTTER: &str = "  \u{23BF}  ";

/// Props for [`UserTeammateMessage`].
#[derive(Debug, Clone, Props)]
pub struct UserTeammateProps {
    /// Display name.
    pub display_name: String,
    /// Optional agent color name.
    pub color: Option<String>,
    /// Sub-type payload.
    pub kind: UserTeammateKind,
    /// Active palette.
    pub theme: Theme,
}

impl Default for UserTeammateProps {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            color: None,
            kind: UserTeammateKind::Note {
                summary: None,
                content: None,
                is_transcript_mode: false,
            },
            theme: Theme::dark(),
        }
    }
}

/// Pure-string renderer.
#[must_use]
pub fn render_user_teammate_to_string(props: UserTeammateProps) -> String {
    let header = format!("@{}{POINTER}", props.display_name);
    match &props.kind {
        UserTeammateKind::TaskCompleted {
            task_id,
            task_subject,
        } => {
            // Two lines: header, then the MessageResponse-guttered completed line.
            let mut line2 = format!("{GUTTER}{CHECK} Completed task #{task_id}");
            if let Some(s) = task_subject {
                line2.push_str(&format!(" ({s})"));
            }
            format!("{header}\n{line2}")
        }
        UserTeammateKind::Note {
            summary,
            content,
            is_transcript_mode,
        } => {
            let mut out = header;
            if let Some(s) = summary {
                out.push(' ');
                out.push_str(s);
            }
            if *is_transcript_mode {
                if let Some(c) = content {
                    for line in c.lines() {
                        out.push('\n');
                        out.push_str("  ");
                        out.push_str(line);
                    }
                }
            }
            out
        }
    }
}

/// iocraft component.
#[component]
pub fn UserTeammateMessage(props: &UserTeammateProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme;
    let accent = match &props.color {
        Some(name) => agent_color_from_name(name),
        None => agent_color_from_name(""), // cyan fallback
    };
    let header = format!("@{}{POINTER}", props.display_name);
    match &props.kind {
        UserTeammateKind::TaskCompleted {
            task_id,
            task_subject,
        } => {
            let completed = format!(" Completed task #{task_id}");
            let subject = task_subject.as_ref().map(|s| format!(" ({s})"));
            element! {
                View(flex_direction: FlexDirection::Column) {
                    Text(content: header.clone(), color: accent)
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: GUTTER, color: theme.dim)
                        Text(content: CHECK, color: theme.success)
                        Text(content: completed, color: theme.text)
                        #(subject.map(|s| element! {
                            Text(content: s, color: theme.dim)
                        }))
                    }
                }
            }
            .into_any()
        }
        UserTeammateKind::Note {
            summary,
            content,
            is_transcript_mode,
        } => {
            let head = match summary {
                Some(s) => format!("{header} {s}"),
                None => header.clone(),
            };
            let body = if *is_transcript_mode {
                content.as_ref().map(|c| {
                    c.lines()
                        .map(|l| format!("  {l}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            } else {
                None
            };
            element! {
                View(flex_direction: FlexDirection::Column) {
                    Text(content: head, color: accent)
                    #(body.map(|b| element! {
                        Text(content: b, color: theme.text)
                    }))
                }
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_bytes() {
        assert_eq!(POINTER, "\u{276F}");
        assert_eq!(CHECK, "\u{2713}");
        // ⎿ = U+23BF; gutter = 2 spaces + ⎿ + 2 spaces.
        assert_eq!(GUTTER.as_bytes(), &[0x20, 0x20, 0xE2, 0x8E, 0xBF, 0x20, 0x20]);
    }

    #[test]
    fn task_completed_with_subject() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "alice".into(),
            color: Some("magenta".into()),
            kind: UserTeammateKind::TaskCompleted {
                task_id: "456".into(),
                task_subject: Some("Setup DB".into()),
            },
            theme: Theme::dark(),
        });
        assert_eq!(
            out,
            "@alice\u{276F}\n  \u{23BF}  \u{2713} Completed task #456 (Setup DB)"
        );
    }

    #[test]
    fn task_completed_no_subject() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "lead".into(),
            color: None,
            kind: UserTeammateKind::TaskCompleted {
                task_id: "1".into(),
                task_subject: None,
            },
            theme: Theme::dark(),
        });
        assert_eq!(out, "@lead\u{276F}\n  \u{23BF}  \u{2713} Completed task #1");
    }

    #[test]
    fn note_summary_only() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "bob".into(),
            color: None,
            kind: UserTeammateKind::Note {
                summary: Some("on it".into()),
                content: Some("full body".into()),
                is_transcript_mode: false,
            },
            theme: Theme::dark(),
        });
        assert_eq!(out, "@bob\u{276F} on it");
    }

    #[test]
    fn note_transcript_indents_content() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "bob".into(),
            color: None,
            kind: UserTeammateKind::Note {
                summary: Some("done".into()),
                content: Some("line1\nline2".into()),
                is_transcript_mode: true,
            },
            theme: Theme::dark(),
        });
        assert_eq!(out, "@bob\u{276F} done\n  line1\n  line2");
    }
}
```

- [ ] **Step 4: Wire the module + dispatch** — `messages/mod.rs`: add `pub mod user_teammate;` (alphabetical) and a dispatch arm:

```rust
        RenderedMessage::UserTeammate { display_name, color, kind } => {
            user_teammate::render_user_teammate_to_string(user_teammate::UserTeammateProps {
                display_name: display_name.clone(),
                color: color.clone(),
                kind: kind.clone(),
                theme: crate::theme::Theme::dark(),
            })
        }
```

- [ ] **Step 5: Run unit + build**

Run: `cargo test -p tui --lib components::messages::user_teammate` then `cargo build -p tui`.

- [ ] **Step 6: Add component snapshots** — append to `render_messages.rs` (import the kind enum at the top if convenient, or fully-qualify):

```rust
#[test]
fn user_teammate_task_completed() {
    let mut element = element! {
        tui::components::messages::user_teammate::UserTeammateMessage(
            display_name: "alice".to_string(),
            color: Some("magenta".to_string()),
            kind: tui::state::UserTeammateKind::TaskCompleted {
                task_id: "456".to_string(),
                task_subject: Some("Setup DB".to_string()),
            },
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("user_teammate_task_completed", element.to_string());
}

#[test]
fn user_teammate_note_transcript() {
    let mut element = element! {
        tui::components::messages::user_teammate::UserTeammateMessage(
            display_name: "bob".to_string(),
            color: None,
            kind: tui::state::UserTeammateKind::Note {
                summary: Some("done".to_string()),
                content: Some("line1\nline2".to_string()),
                is_transcript_mode: true,
            },
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("user_teammate_note_transcript", element.to_string());
}
```

Run, review, accept the snapshots.

- [ ] **Step 7: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-03): UserTeammate renderer (task-completed + note) + variant + dispatch"
```

---

## Task 6: teamMemCollapsed — team-memory parts in `CollapsedReadSearch`

claude-code: `components/messages/teamMemCollapsed.tsx` (`TeamMemCountParts`). Adds comma-joined team-memory parts to the collapsed group: recall/search/write, verb-conjugated by active vs. completed, with bold counts and singular/plural `team memory|memories`. The existing `collapsed_read_search.rs` already documents these as deferred ("team-memory parts → M8"). Extend it.

**Files:**
- Modify: `tui/src/components/messages/collapsed_read_search.rs`
- Modify: `tui/src/state.rs` (the `CollapsedReadSearch` variant), `tui/src/components/messages/mod.rs` (dispatch arm)

- [ ] **Step 1: Read the reference** — read `teamMemCollapsed.tsx`. Lock the exact verbs (recall/search/write), the active vs. completed tense forms, whether each part shows its count, and how capitalization works (it should follow the existing first-part-capitalized rule via `cap_first`). The notes below reflect the extraction; **verify and correct against the TSX before coding.**

- [ ] **Step 2: Extend `CollapsedCounts` + write the failing test** — in `collapsed_read_search.rs`, add three fields to `CollapsedCounts`:

```rust
    /// Team memories recalled.
    pub mem_read: u64,
    /// Team-memory searches.
    pub mem_search: u64,
    /// Team memories written.
    pub mem_write: u64,
```

Then add a test asserting the team-mem parts append after read/search/list, lower-cased when not first (verify literals against the TSX in Step 1):

```rust
    #[test]
    fn team_mem_parts_append() {
        let c = CollapsedCounts {
            search: 0,
            read: 1,
            list: 0,
            is_active: false,
            mem_read: 2,
            mem_search: 0,
            mem_write: 1,
        };
        // "Read 1 file" then ", recalled 2 team memories, wrote 1 team memory"
        assert_eq!(
            render_collapsed_to_string(&c, &[], false),
            "  \u{23BF}  Read 1 file, recalled 2 team memories, wrote 1 team memory"
        );
    }

    #[test]
    fn team_mem_only_capitalizes_first() {
        let c = CollapsedCounts {
            mem_read: 3,
            ..CollapsedCounts::default()
        };
        assert_eq!(
            render_collapsed_to_string(&c, &[], false),
            "  \u{23BF}  Recalled 3 team memories"
        );
    }
```

> Update the 5 **existing** tests in this file that construct `CollapsedCounts { search, read, list, is_active }` with explicit fields: append `mem_read: 0, mem_search: 0, mem_write: 0,` to each (or switch them to `..CollapsedCounts::default()`). They must still compile and pass unchanged in output.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p tui --lib components::messages::collapsed_read_search`
Expected: the new tests FAIL (parts not emitted); existing tests PASS after the field additions.

- [ ] **Step 4: Extend `render_summary`** — in `render_summary`, after the `list` block and before the `parts.is_empty()` check, append the team-mem parts (correct the verbs/forms to match the TSX from Step 1):

```rust
    if c.mem_read > 0 {
        let verb = if c.is_active { "recalling" } else { "recalled" };
        let noun = if c.mem_read == 1 { "team memory" } else { "team memories" };
        parts.push(format!("{verb} {} {noun}", c.mem_read));
    }
    if c.mem_search > 0 {
        let verb = if c.is_active { "searching" } else { "searched" };
        let noun = if c.mem_search == 1 { "team memory" } else { "team memories" };
        parts.push(format!("{verb} {} {noun}", c.mem_search));
    }
    if c.mem_write > 0 {
        let verb = if c.is_active { "writing" } else { "wrote" };
        let noun = if c.mem_write == 1 { "team memory" } else { "team memories" };
        parts.push(format!("{verb} {} {noun}", c.mem_write));
    }
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p tui --lib components::messages::collapsed_read_search`
Expected: all PASS.

- [ ] **Step 6: Thread the new counts through state + dispatch** — in `state.rs`, add `mem_read: u64, mem_search: u64, mem_write: u64` to the `RenderedMessage::CollapsedReadSearch` variant. In `messages/mod.rs`, the `CollapsedReadSearch` dispatch arm constructs `CollapsedCounts` — add the three new fields there (mapping from the variant's new fields). Build: `cargo build -p tui`; fix any other constructors of the variant the compiler flags (e.g. in `streaming.rs` if it builds one — default the new counts to 0).

- [ ] **Step 7: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-03): teamMemCollapsed — team-memory parts in CollapsedReadSearch"
```

---

## Task 7: teamMemSaved — system-line segment helper

claude-code: `components/messages/teamMemSaved.ts` (`teamMemSavedPart`). A pure helper returning the `{count} team {memory|memories}` segment used inside a memory-saved system line; returns null when count is 0. There is **no** memory-saved message variant/data source in the TUI yet, so this lands as a standalone literal-locked helper (UI-first; a consumer wires it when the engine reports memory-saved events). Faithful to claude-code, where this is a helper, not a renderer.

**Files:**
- Create: `tui/src/components/messages/team_mem_saved.rs`
- Modify: `tui/src/components/messages/mod.rs` (add `pub mod team_mem_saved;`)

- [ ] **Step 1: Read the reference** — read `teamMemSaved.ts`; confirm the exact segment string and the singular/plural rule.

- [ ] **Step 2: Write the failing test** — create `team_mem_saved.rs` with only the test first:

```rust
//! `teamMemSaved` — team-memory-saved system-line segment.
//!
//! Literal lock (claude-code `teamMemSaved.ts` `teamMemSavedPart`): returns
//! `Some("{count} team {memory|memories}")`; `None` when count is 0. A pure
//! segment helper consumed by a memory-saved system line (no TUI consumer yet
//! — UI-first).

/// Build the team-memory-saved segment. `None` when `team_count == 0`.
#[must_use]
pub fn team_mem_saved_segment(team_count: u64) -> Option<String> {
    if team_count == 0 {
        return None;
    }
    let noun = if team_count == 1 { "memory" } else { "memories" };
    Some(format!("{team_count} team {noun}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_none() {
        assert_eq!(team_mem_saved_segment(0), None);
    }

    #[test]
    fn singular_and_plural() {
        assert_eq!(team_mem_saved_segment(1).as_deref(), Some("1 team memory"));
        assert_eq!(team_mem_saved_segment(5).as_deref(), Some("5 team memories"));
    }
}
```

- [ ] **Step 3: Wire the module** — `messages/mod.rs`: add `pub mod team_mem_saved;` (alphabetical: after `system_text`/before `task_assignment`? `team_mem_saved` sorts after `task_assignment` and before `thinking` — place correctly).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p tui --lib components::messages::team_mem_saved`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-03): teamMemSaved system-line segment helper"
```

---

## Task 8: M9-03 gate + tag

**Files:** none (verification + tag only).

- [ ] **Step 1: Format check**

Run (from `lingxi-code/`): `cargo fmt --check`
Expected: clean. If not, `cargo fmt -p tui` and commit `style(M9-03): cargo fmt`.

- [ ] **Step 2: Clippy (multiagent + messages scope)**

Run: `cargo clippy -p tui --all-targets -- -D warnings`
Expected: zero warnings in the new/modified files. (Pre-existing workspace debt outside `tui` is out of scope — do not fix it here; if clippy fails only on pre-existing non-`tui` debt, note it and proceed, matching the M9-01/M9-02 precedent.)

- [ ] **Step 3: Full tui test run**

Run: `cargo test -p tui`
Expected: exit 0. Confirm every new snapshot is accepted (no `.snap.new` left). Allowed known flakes per spec §5.4 may be re-run.

- [ ] **Step 4: Workspace build**

Run: `cargo build --workspace`
Expected: exit 0.

- [ ] **Step 5: Tag**

```bash
git tag -a m9.3 -m "M9-03: team message renderers (6) — TaskAssignment, UserTeammate, UserAgentNotification, UserChannel, teamMemCollapsed, teamMemSaved"
git tag | grep m9
```

Expected: `m9.1`, `m9.2`, `m9.3` present. **No remote push.**

---

## Forward notes (for later sub-plans)

- **M9-06 (mailbox drain):** classify a `TeammateMessage.content` into → `PlanApproval` / `Shutdown` (existing variants) / `TaskAssignment` / `UserTeammate{TaskCompleted|Note}`; suppress `idle_notification`. Reads claude-code `utils/teammateMailbox.ts` + the `tryRender*` helpers. The structured variants from this sub-plan are the classifier's targets — keep them in sync (R1).
- **M9-09 (parity fixture + literal catalog):** add the 6 renderers' high-value strings + the teammate sub-type markers (`@name❯`, `✓ Completed task #`, `← `, `● `) to `parity_tui_multiagent.json` and the literal-lock catalog.

---

**End of plan.**
