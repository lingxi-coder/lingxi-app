# M9-07 — Worker Permission Chrome Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the worker-permission chrome — `WorkerBadge` (colored `● @name`) and `WorkerPendingPermission` (the "waiting for team lead approval" view with tool/action lines) — and surface a worker badge in the existing permission dialog when a request is worker-originated, all inside the M6 permission focus-trap (priority 1), with a cross-state-seam test.

**Architecture:** Two pure `render_*_to_string` renderers in the existing `tui/src/components/permissions/` module (byte-locked, snapshot-tested). Worker identity rides on the **TUI-side** `PendingPermission` struct via a new `worker: Option<WorkerPermissionInfo>` field (the `PermissionRequest` enum is in frozen `traits/` and must not change). The `ToolUseConfirm` dialog prepends the `WorkerBadge` when `worker` is present (additive, guarded — existing permission snapshots unaffected). The priority-1 focus-trap routing is unchanged (a worker permission is still a `pending_permission`). Live worker-permission origination is stubbed (the worker pool is an M1.14 stub), so the `worker` field is fixture/test-set; the cross-state-seam test exercises the routing precedence.

**Tech Stack:** Rust 1.82.0, `iocraft = "=0.8.3"`, `insta`, `tokio` (tests). Run cargo from inside `lingxi-code/`.

**Literal-lock reference:** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/permissions/WorkerBadge.tsx` + `WorkerPendingPermission.tsx`.

**Locked literals (verified):**
- WorkerBadge: `{BLACK_CIRCLE} @{name}` — we lock the non-darwin `BLACK_CIRCLE` = `●` (U+25CF, matching `system_text::MARKER`); circle colored by the worker color, `@name` bold.
- WorkerPendingPermission: header `Waiting for team lead approval` (warning, bold); the worker badge line; `Tool: {tool}`; `Action: {description}` (labels dim); optional `Permission request sent to team "{team}" leader` (dim); round warning border.

---

## File Structure

| File | Responsibility | C/M |
|---|---|---|
| `tui/src/components/permissions/worker.rs` | `WorkerPermissionInfo`, `render_worker_badge`, `render_worker_pending_to_string`, the two components | Create |
| `tui/src/components/permissions/mod.rs` | + `pub mod worker;` | Modify |
| `tui/src/state.rs` | + `worker: Option<WorkerPermissionInfo>` on `PendingPermission` | Modify |
| `tui/src/streaming.rs` | set `worker: None` where `PendingPermission` is built | Modify |
| `tui/src/components/permissions/tool_use_confirm.rs` | prepend the `WorkerBadge` when worker info present (guarded) | Modify |
| `tui/tests/worker_permission.rs` | snapshots + cross-state-seam test | Create |

`PendingPermission` is TUI-side (`tui/src/state.rs`); adding a field there does NOT touch `traits/`. `agent_color_from_name` (M9-03) maps the worker color name → render `Color`.

---

## Task 1: WorkerBadge + WorkerPendingPermission renderers

**Files:** Create `tui/src/components/permissions/worker.rs`; Modify `tui/src/components/permissions/mod.rs`.

- [ ] **Step 1: Read the reference** — read `WorkerBadge.tsx` + `WorkerPendingPermission.tsx`; the literals below are reconciled to them.

- [ ] **Step 2: Create with tests** — `tui/src/components/permissions/worker.rs`:

```rust
//! Worker-permission chrome (claude-code `WorkerBadge.tsx` +
//! `WorkerPendingPermission.tsx`): a colored `● @name` badge and the
//! "waiting for team lead approval" view. We lock the non-darwin
//! `BLACK_CIRCLE` = `●` (U+25CF).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::multiagent::style::agent_color_from_name;
use crate::theme::Theme;

/// `● ` worker badge circle (`BLACK_CIRCLE` non-darwin, U+25CF + space).
pub const BADGE_CIRCLE: &str = "\u{25CF} ";

/// Worker identity carried on a pending permission (TUI-side; not on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkerPermissionInfo {
    /// Worker display name (rendered `@name`).
    pub name: String,
    /// Worker color name (→ `agent_color_from_name`).
    pub color: String,
    /// Optional team name (for the "sent to team … leader" line).
    pub team: Option<String>,
}

/// `● @{name}` (claude-code `WorkerBadge`). Color applied by the component.
#[must_use]
pub fn render_worker_badge(name: &str) -> String {
    format!("{BADGE_CIRCLE}@{name}")
}

/// The worker-pending body (claude-code `WorkerPendingPermission`).
#[must_use]
pub fn render_worker_pending_to_string(
    tool: &str,
    description: &str,
    worker_name: Option<&str>,
    team: Option<&str>,
) -> String {
    let mut out = String::from("Waiting for team lead approval");
    if let Some(name) = worker_name {
        out.push('\n');
        out.push_str(&render_worker_badge(name));
    }
    out.push_str(&format!("\nTool: {tool}\nAction: {description}"));
    if let Some(t) = team {
        out.push_str(&format!("\nPermission request sent to team \"{t}\" leader"));
    }
    out
}

/// Props for [`WorkerBadge`].
#[derive(Debug, Clone, Default, Props)]
pub struct WorkerBadgeProps {
    /// Worker display name.
    pub name: String,
    /// Worker color name.
    pub color: String,
}

/// iocraft component: colored circle + bold `@name`.
#[component]
pub fn WorkerBadge(props: &WorkerBadgeProps) -> impl Into<AnyElement<'static>> {
    let circle_color = agent_color_from_name(&props.color);
    let name = format!("@{}", props.name);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: BADGE_CIRCLE, color: circle_color)
            Text(content: name, weight: Weight::Bold)
        }
    }
}

/// Props for [`WorkerPendingPermission`].
#[derive(Debug, Clone, Default, Props)]
pub struct WorkerPendingProps {
    /// Requested tool.
    pub tool: String,
    /// Action description.
    pub description: String,
    /// Worker identity (badge shown when present).
    pub worker: Option<WorkerPermissionInfo>,
    /// Active palette.
    pub theme: Theme,
}

/// iocraft component: round warning border, "Waiting…" header (bold warning),
/// worker badge, Tool/Action lines, optional team line.
#[component]
pub fn WorkerPendingPermission(props: &WorkerPendingProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme;
    let (worker_name, team) = props
        .worker
        .as_ref()
        .map_or((None, None), |w| (Some(w.name.clone()), w.team.clone()));
    let body = render_worker_pending_to_string(
        &props.tool,
        &props.description,
        worker_name.as_deref(),
        team.as_deref(),
    );
    let mut lines = body.lines();
    let header = lines.next().unwrap_or("").to_string();
    let rest = lines.collect::<Vec<_>>().join("\n");
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: theme.warning,
        ) {
            Text(content: header, color: theme.warning, weight: Weight::Bold)
            Text(content: rest, color: theme.text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_bytes_and_format() {
        assert_eq!(BADGE_CIRCLE.as_bytes(), &[0xE2, 0x97, 0x8F, 0x20]); // ● + space
        assert_eq!(render_worker_badge("alice"), "\u{25CF} @alice");
    }

    #[test]
    fn pending_minimal() {
        assert_eq!(
            render_worker_pending_to_string("Bash", "run ls", None, None),
            "Waiting for team lead approval\nTool: Bash\nAction: run ls"
        );
    }

    #[test]
    fn pending_full() {
        assert_eq!(
            render_worker_pending_to_string("Bash", "run ls", Some("alice"), Some("my-team")),
            "Waiting for team lead approval\n\u{25CF} @alice\nTool: Bash\nAction: run ls\nPermission request sent to team \"my-team\" leader"
        );
    }
}
```

- [ ] **Step 3: Wire + run** — add `pub mod worker;` to `tui/src/components/permissions/mod.rs`; `cargo test -p tui --lib components::permissions::worker` (3 PASS); `cargo build -p tui`.

- [ ] **Step 4: Snapshots** — create `tui/tests/worker_permission.rs`:

```rust
//! M9-07 — worker-permission chrome snapshots + cross-state-seam test.

use tui::components::permissions::worker::{
    render_worker_badge, render_worker_pending_to_string,
};

#[test]
fn worker_badge_snapshot() {
    insta::assert_snapshot!("worker_badge", render_worker_badge("alice"));
}

#[test]
fn worker_pending_snapshot() {
    insta::assert_snapshot!(
        "worker_pending_full",
        render_worker_pending_to_string("Bash", "run mkdir /tmp/x", Some("alice"), Some("my-team"))
    );
}
```

Run `cargo test -p tui --test worker_permission`, inspect + accept the 2 `.snap.new`, re-run green.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-07): WorkerBadge + WorkerPendingPermission renderers"
```

---

## Task 2: PendingPermission worker field + dialog badge

**Files:** Modify `tui/src/state.rs`, `tui/src/streaming.rs`, `tui/src/components/permissions/tool_use_confirm.rs`.

- [ ] **Step 1: Add the field** — in `tui/src/state.rs`, add to the `PendingPermission` struct:

```rust
    /// (M9-07) Worker identity when this request is worker-originated (TUI-side;
    /// the `PermissionRequest` enum is in frozen `traits/`). Fixture/test-set
    /// until the worker pool is live.
    pub worker: Option<crate::components::permissions::worker::WorkerPermissionInfo>,
```

- [ ] **Step 2: Update constructions** — `grep -rn "PendingPermission {" tui/src` and add `worker: None,` to every literal construction (notably `tui/src/streaming.rs`'s `TurnEvent::PermissionRequest` arm). `cargo build -p tui` until clean (fix every flagged construction site).

- [ ] **Step 3: Prepend the badge in the dialog** — in `tui/src/components/permissions/tool_use_confirm.rs`, add an optional `worker_badge: Option<String>` to `ToolUseConfirmProps`, and when `Some`, render it as a first line (e.g. a `Text` above the "Claude needs your permission…" line). Where `ToolUseConfirmProps` is constructed from `AppState` (search the render path — `app.rs`/`root.rs`), populate `worker_badge` from `state.pending_permission.as_ref().and_then(|p| p.worker.as_ref()).map(|w| render_worker_badge(&w.name))`. Keep it `None` by default so existing permission snapshots are unchanged (additive + guarded).

> If threading a new prop through the dialog render is non-trivial, the minimal acceptable alternative is to render the badge line inside the existing `ToolUseConfirm` component by reading a new `worker_badge` prop defaulting to `None`. Do NOT change behavior when `worker_badge` is `None`.

- [ ] **Step 4: Build + verify additivity** — `cargo build -p tui`; `cargo test -p tui` — existing permission/dialog snapshots MUST be unchanged (worker is `None` everywhere live). If any changed, STOP and report.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-07): carry worker identity on PendingPermission + badge in the dialog"
```

---

## Task 3: Cross-state-seam test

Model on `tui/tests/cross_state_seam_test.rs` (`permission_wins_over_open_screen`). Verify: a worker-tagged permission still wins priority 1 over an open screen, and resolves correctly.

**Files:** Modify `tui/tests/worker_permission.rs`.

- [ ] **Step 1: Read the model** — read `tui/tests/cross_state_seam_test.rs` + `tui/tests/behavior_permission_dialogs.rs` to copy the exact `AppState` construction + `handle_live_key`/`handle_key` driving + the permission-arming helper.

- [ ] **Step 2: Add the seam test** — append to `tui/tests/worker_permission.rs` (adapt the helper names/imports to match the existing tests you just read):

```rust
mod seam {
    use tui::components::permissions::worker::WorkerPermissionInfo;
    use tui::state::{AppState, PendingPermission};
    // import the same items the existing seam/permission tests use:
    //   StatusSnapshot, PermissionRequest, PromptDefault, handle_live_key, key(..), Screen, etc.

    fn arm_worker_permission(st: &mut AppState) {
        // Build a ToolUseConfirm PendingPermission exactly as behavior_permission_dialogs.rs does,
        // then tag it with a worker:
        // st.pending_permission = Some(PendingPermission { request: <ToolUseConfirm ...>, worker: Some(WorkerPermissionInfo { name: "alice".into(), color: "magenta".into(), team: Some("my-team".into()) }) });
    }

    #[test]
    fn worker_permission_wins_over_open_screen() {
        // 1. fresh AppState (use the existing tests' constructor)
        // 2. open a screen (e.g. st.open_doctor(..) like cross_state_seam_test.rs)
        // 3. arm_worker_permission(&mut st)
        // 4. handle_live_key(&mut st, &key(KeyCode::Char('q')), 24)  // 'q' would close a screen
        // 5. assert st.pending_permission.is_some()  // permission still owns keys (priority 1)
        // 6. assert the screen is still open (NOT closed)
        // 7. assert st.pending_permission.as_ref().unwrap().worker.is_some()  // worker tag preserved
    }

    #[test]
    fn worker_permission_resolves_and_clears() {
        // arm_worker_permission; drive the deny/allow key the dialog uses (e.g. '1' = allow once);
        // assert pending_permission is cleared (resolution sent), exactly like behavior_permission_dialogs.rs.
    }
}
```

Fill in the bodies using the EXACT constructors/imports from the existing tests (do not invent APIs). Both tests must compile and pass.

- [ ] **Step 3: Run** — `cargo test -p tui --test worker_permission` → all pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "test(M9-07): cross-state seam — worker permission wins priority 1 + resolves"
```

---

## Task 4: M9-07 gate + tag

- [ ] **Step 1: Format** — `cargo fmt --check`.
- [ ] **Step 2: Clippy** — `cargo clippy -p tui --all-targets 2>&1 | grep -E "tui/src|tui/tests"` → zero tui-originated warnings.
- [ ] **Step 3: Tests** — `cargo test -p tui` (all pass; no `.snap.new`).
- [ ] **Step 4: Workspace build** — `cargo build --workspace` → exit 0.
- [ ] **Step 5: Tag**

```bash
git tag -a m9.7 -m "M9-07: worker permission chrome — WorkerBadge + WorkerPendingPermission + badge in the permission dialog + cross-state seam test"
git tag | grep -E '^m9' | sort -V
```

Expect `m9.1`…`m9.7`. **No remote push.**

---

## Forward notes

- **Deferred (gated on the worker pool un-stub):** live worker-permission origination (a worker actually requesting a tool → the lead's dialog showing the badge live) and the separate worker-side "waiting" screen. The renderers + the `worker` field are ready; a live producer sets `pending_permission.worker` unchanged.
- **M9-08** adds read-only agent discovery (`AgentsList` + `AgentDetail` over the `AgentDefinition` catalog) + a `/agents` command/screen.

---

**End of plan.**
