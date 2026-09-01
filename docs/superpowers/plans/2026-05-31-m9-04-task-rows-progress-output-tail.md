# M9-04 — Background-Task Rows + Progress + Output Tail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the background-task **row renderers** (one per `TaskState` variant — 7), the **ShellProgress** line, the shared **task-status-text** + **duration/exit** formatters, and a live **output-tail** (incremental `TaskRegistryHandle::output` reads), all literal-locked to claude-code.

**Architecture:** All row rendering lands as **pure `render_*_to_string` functions** (byte-locked, unit + insta-snapshot tested) in a new `tui/src/components/tasks/` module. The colored iocraft components + the dialog that *display* these rows are **M9-05** (build where the consumer is). The live `render_task_row(&TaskRow)` dispatch renders only the **wire-available subset** (`task_type`, `status`, `description`) per spec §2.4 ("where a claude-code field has no LingXi equivalent, the renderer omits it — never invents engine state"); the rich per-type fields (elapsed, exit, counts, `, unread`, agent activity, dream phase) are **renderer parameters** exercised by snapshots/fixtures and light up if/when a richer feed lands (spec Q3: build the full surface, fixtures where stubbed). The output-tail is genuinely live (the spool round-trip is proven).

**Tech Stack:** Rust 1.82.0 (pinned), `async-trait`, `tokio` (tests), `insta`. Run all cargo from inside `lingxi-code/`.

**Frozen-trait constraint (locked decision — do NOT change `traits/`):** `TaskRegistryHandle` surfaces only `TaskRecord { task_id, task_type, status, description }` and `TaskOutputChunk { task_id, content, total_lines, truncated }`. The M9-01 `TaskRow` carries exactly those 4 fields and the contract test (`tui/tests/multiagent_contract_test.rs`) enforces "no field invented" — **do not add fields to `TaskRow`.** Rich data is passed as *function parameters* to the renderers, not stored on `TaskRow`.

**Literal-lock reference:** `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/components/tasks/` — `BackgroundTask.tsx` (the 7-type row cases), `ShellProgress.tsx` / `taskStatusUtils.tsx` (status labels), `ShellDetailDialog.tsx` (exit-code), `src/utils/format.ts` (`formatDuration`).

**Locked literals (verified from source):**
- status → label (claude-code `ShellProgress`/`TaskStatusText`): `completed`→`done`, `failed`→`error`, `killed`→`stopped`, `running`→`running`, `pending`→`pending`. Wrapped `({label}{suffix})`. Suffix `, unread` when `completed && !notified`.
- remote diamonds: `◇` U+25C7 (running/pending), `◆` U+25C6 (terminal). Separator ` · ` = space + U+00B7 + space.
- teammate prefix `@`, separator ` : `.
- `formatDuration`: `<60s`→`"12s"`; `≥60s`→`"1m"`/`"1m 3s"`; `≥1h`→`"1h"`/`"1h 2m"`; `≥1d`→`"1d"`/`"1d 3h"`.
- exit-code (detail): `exit code: {code}`.

---

## File Structure

| File | Responsibility | Create/Modify |
|---|---|---|
| `tui/src/components/tasks/mod.rs` | module decls + `render_task_row(&TaskRow) -> String` dispatch (wire subset) | Create |
| `tui/src/components/tasks/format.rs` | `format_duration(ms)` + `format_exit_code(code)` | Create |
| `tui/src/components/tasks/status_text.rs` | `task_status_label`, `wrap_status_label`, `render_task_status_text` | Create |
| `tui/src/components/tasks/shell_progress.rs` | `render_shell_progress_to_string` (local_bash) | Create |
| `tui/src/components/tasks/rows.rs` | the other 6 type render fns + diamond/middot consts | Create |
| `tui/src/components/tasks/output_tail.rs` | `OutputTailState` + `tail_once` + `render_output_tail` | Create |
| `tui/src/components/mod.rs` | + `pub mod tasks;` | Modify |
| `tui/tests/render_task_rows.rs` | insta snapshots — one per task type (live-shape) + rich variants | Create |

No `RenderedMessage` variant, no dispatch-table/scrollback/virtual_message_list arms — task rows are **not** transcript messages (they render in the M9-05 dialog/footer). No iocraft components in this sub-plan (deferred to M9-05).

---

## Task 1: Duration + exit-code formatters

**Files:** Create `tui/src/components/tasks/format.rs`; Modify `tui/src/components/mod.rs`, `tui/src/components/tasks/mod.rs`.

- [ ] **Step 1: Wire the module skeleton** — in `tui/src/components/mod.rs` add `pub mod tasks;` (alphabetical — likely after `status_line`/`spinner`; place correctly among the existing `pub mod` lines). Create `tui/src/components/tasks/mod.rs` with just:

```rust
//! Background-task rendering (M9-04): per-type row renderers, shell progress,
//! status text, duration/exit formatters, and the live output tail. Pure
//! string renderers — the iocraft components + dialog that display them are
//! M9-05.

pub mod format;
```

- [ ] **Step 2: Write the failing test** — create `tui/src/components/tasks/format.rs` with the tests first:

```rust
//! Duration + exit-code formatting for task progress.
//!
//! Literal lock: claude-code `src/utils/format.ts` `formatDuration` and
//! `ShellDetailDialog.tsx` exit-code display.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(12_000), "12s");
        assert_eq!(format_duration(59_000), "59s");
    }

    #[test]
    fn duration_minutes() {
        assert_eq!(format_duration(60_000), "1m");
        assert_eq!(format_duration(63_000), "1m 3s");
        assert_eq!(format_duration(150_000), "2m 30s");
    }

    #[test]
    fn duration_hours_days() {
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(3_660_000), "1h 1m");
        assert_eq!(format_duration(86_400_000), "1d");
        assert_eq!(format_duration(97_200_000), "1d 3h");
    }

    #[test]
    fn exit_code() {
        assert_eq!(format_exit_code(0), "exit code: 0");
        assert_eq!(format_exit_code(1), "exit code: 1");
    }
}
```

- [ ] **Step 3: Run to verify it fails** — `cargo test -p tui --lib components::tasks::format` → FAIL (`cannot find function format_duration`).

- [ ] **Step 4: Implement** — prepend to `format.rs` (above the test module):

```rust
/// Format elapsed milliseconds (claude-code `formatDuration`): seconds under a
/// minute (`12s`), then `Nm`/`Nm Ss`, `Nh`/`Nh Mm`, `Nd`/`Nd Hh`.
#[must_use]
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        return format!("{secs}s");
    }
    let (mins, rem_s) = (secs / 60, secs % 60);
    if mins < 60 {
        return if rem_s == 0 { format!("{mins}m") } else { format!("{mins}m {rem_s}s") };
    }
    let (hours, rem_m) = (mins / 60, mins % 60);
    if hours < 24 {
        return if rem_m == 0 { format!("{hours}h") } else { format!("{hours}h {rem_m}m") };
    }
    let (days, rem_h) = (hours / 24, hours % 24);
    if rem_h == 0 { format!("{days}d") } else { format!("{days}d {rem_h}h") }
}

/// Format an exit code for the detail status line (claude-code `exit code: N`).
#[must_use]
pub fn format_exit_code(code: i32) -> String {
    format!("exit code: {code}")
}
```

- [ ] **Step 5: Run to verify pass** — `cargo test -p tui --lib components::tasks::format` → 4 PASS. Then `cargo build -p tui`.

- [ ] **Step 6: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-04): task duration + exit-code formatters"
```

---

## Task 2: Task status text (`(label)` indicator)

**Files:** Create `tui/src/components/tasks/status_text.rs`; Modify `tui/src/components/tasks/mod.rs` (add `pub mod status_text;`).

- [ ] **Step 1: Write the failing test** — create `tui/src/components/tasks/status_text.rs`:

```rust
//! Task status label + the `(label)` indicator.
//!
//! Literal lock: claude-code `ShellProgress.tsx` / `TaskStatusText`
//! (`({label}{suffix})`, dim, status-colored) and `taskStatusUtils.tsx`.
//! Color is applied by the M9-05 dialog via `multiagent::style::task_status_color`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(task_status_label("completed"), "done");
        assert_eq!(task_status_label("failed"), "error");
        assert_eq!(task_status_label("killed"), "stopped");
        assert_eq!(task_status_label("running"), "running");
        assert_eq!(task_status_label("pending"), "pending");
        assert_eq!(task_status_label("weird"), "pending");
    }

    #[test]
    fn status_text_no_suffix() {
        assert_eq!(render_task_status_text("completed", None), "(done)");
        assert_eq!(render_task_status_text("running", None), "(running)");
    }

    #[test]
    fn status_text_with_suffix() {
        assert_eq!(render_task_status_text("completed", Some(", unread")), "(done, unread)");
    }

    #[test]
    fn wrap_explicit_label() {
        assert_eq!(wrap_status_label("3 agents", None), "(3 agents)");
        assert_eq!(wrap_status_label("done", Some(", unread")), "(done, unread)");
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p tui --lib components::tasks::status_text` → FAIL.

- [ ] **Step 3: Implement** — prepend to `status_text.rs`:

```rust
/// Status wire string → display label (claude-code labels). Unknown → `pending`.
#[must_use]
pub fn task_status_label(status: &str) -> &'static str {
    match status {
        "completed" => "done",
        "failed" => "error",
        "killed" => "stopped",
        "running" => "running",
        _ => "pending",
    }
}

/// Wrap an explicit label as `({label}{suffix})`.
#[must_use]
pub fn wrap_status_label(label: &str, suffix: Option<&str>) -> String {
    match suffix {
        Some(s) => format!("({label}{s})"),
        None => format!("({label})"),
    }
}

/// `({label}{suffix})` for a status string (claude-code `TaskStatusText`).
#[must_use]
pub fn render_task_status_text(status: &str, suffix: Option<&str>) -> String {
    wrap_status_label(task_status_label(status), suffix)
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p tui --lib components::tasks::status_text` → 4 PASS. `cargo build -p tui`.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-04): task status-text (label) indicator"
```

---

## Task 3: ShellProgress (local_bash row)

claude-code `BackgroundTask.tsx` local_bash case = `{command} {ShellProgress}` where ShellProgress renders `({label})`. We add an optional trailing elapsed (` 12s`) for callers that have it.

**Files:** Create `tui/src/components/tasks/shell_progress.rs`; Modify `tui/src/components/tasks/mod.rs` (add `pub mod shell_progress;`).

- [ ] **Step 1: Write the failing test** — create `tui/src/components/tasks/shell_progress.rs`:

```rust
//! `ShellProgress` — the local_bash task row (claude-code `ShellProgress.tsx`
//! + `BackgroundTask.tsx` local_bash case): `{command} ({label})` with an
//! optional trailing elapsed (` 12s`) when known.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_no_elapsed() {
        assert_eq!(
            render_shell_progress_to_string("cargo build", "running", None),
            "cargo build (running)"
        );
    }

    #[test]
    fn completed_with_elapsed() {
        assert_eq!(
            render_shell_progress_to_string("ls", "completed", Some(12_000)),
            "ls (done) 12s"
        );
    }

    #[test]
    fn failed_no_elapsed() {
        assert_eq!(
            render_shell_progress_to_string("false", "failed", None),
            "false (error)"
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p tui --lib components::tasks::shell_progress` → FAIL.

- [ ] **Step 3: Implement** — prepend to `shell_progress.rs`:

```rust
use crate::components::tasks::format::format_duration;
use crate::components::tasks::status_text::render_task_status_text;

/// `{command} ({label})` + optional ` {elapsed}` (claude-code local_bash row).
#[must_use]
pub fn render_shell_progress_to_string(
    command: &str,
    status: &str,
    elapsed_ms: Option<u64>,
) -> String {
    let mut out = format!("{command} {}", render_task_status_text(status, None));
    if let Some(ms) = elapsed_ms {
        out.push(' ');
        out.push_str(&format_duration(ms));
    }
    out
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p tui --lib components::tasks::shell_progress` → 3 PASS. `cargo build -p tui`.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-04): ShellProgress local_bash row renderer"
```

---

## Task 4: The other 6 type row renderers

claude-code `BackgroundTask.tsx` cases: `local_agent`, `remote_agent`, `in_process_teammate`, `local_workflow`, `monitor_mcp`, `dream`. Each is a pure renderer taking the wire fields + the type's rich optionals.

**Files:** Create `tui/src/components/tasks/rows.rs`; Modify `tui/src/components/tasks/mod.rs` (add `pub mod rows;`).

- [ ] **Step 1: Read the reference** — read `BackgroundTask.tsx` to confirm each case's layout; the strings below are reconciled to it.

- [ ] **Step 2: Write the failing tests** — create `tui/src/components/tasks/rows.rs`:

```rust
//! Per-type background-task row renderers (claude-code `BackgroundTask.tsx`
//! cases). Pure string renderers; rich optionals (`notified`, counts,
//! `activity`, dream `phase`/`detail`) are passed by the caller — the live
//! `super::render_task_row` passes the wire subset (defaults), fixtures pass
//! full data. Per spec §2.4 the live row omits what the wire does not carry.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_agent_unread_only_when_completed_and_unnotified() {
        assert_eq!(render_local_agent_row("review", "running", false), "review (running)");
        assert_eq!(render_local_agent_row("review", "completed", false), "review (done, unread)");
        assert_eq!(render_local_agent_row("review", "completed", true), "review (done)");
    }

    #[test]
    fn remote_agent_diamond_and_progress() {
        // running → open diamond + "running…"; with counts → "d/t".
        assert_eq!(render_remote_agent_row("deploy", "running", None), "\u{25C7} deploy \u{00B7} running…");
        assert_eq!(render_remote_agent_row("deploy", "running", Some((3, 7))), "\u{25C7} deploy \u{00B7} 3/7");
        // completed → filled diamond + "done".
        assert_eq!(render_remote_agent_row("deploy", "completed", None), "\u{25C6} deploy \u{00B7} done");
    }

    #[test]
    fn in_process_teammate_name_and_activity() {
        assert_eq!(render_in_process_teammate_row("alice", "running", Some("editing")), "@alice : editing");
        assert_eq!(render_in_process_teammate_row("alice", "running", None), "@alice (running)");
    }

    #[test]
    fn local_workflow_agent_count_when_running() {
        assert_eq!(render_local_workflow_row("deploy", "running", Some(3), false), "deploy (3 agents)");
        assert_eq!(render_local_workflow_row("deploy", "running", Some(1), false), "deploy (1 agent)");
        assert_eq!(render_local_workflow_row("deploy", "completed", None, false), "deploy (done, unread)");
    }

    #[test]
    fn monitor_mcp_like_agent() {
        assert_eq!(render_monitor_mcp_row("watch fs", "running", false), "watch fs (running)");
        assert_eq!(render_monitor_mcp_row("watch fs", "completed", true), "watch fs (done)");
    }

    #[test]
    fn dream_phase_and_detail() {
        assert_eq!(
            render_dream_row("nightly", "running", Some("updating"), Some("5 files")),
            "nightly \u{00B7} updating \u{00B7} 5 files (running)"
        );
        assert_eq!(render_dream_row("nightly", "running", None, None), "nightly (running)");
    }
}
```

- [ ] **Step 3: Run to verify it fails** — `cargo test -p tui --lib components::tasks::rows` → FAIL.

- [ ] **Step 4: Implement** — prepend to `rows.rs`:

```rust
use crate::components::tasks::status_text::{render_task_status_text, wrap_status_label};

/// `◇` remote running/pending diamond (U+25C7).
pub const DIAMOND_OPEN: char = '\u{25C7}';
/// `◆` remote terminal diamond (U+25C6).
pub const DIAMOND_FILLED: char = '\u{25C6}';
/// ` · ` middot separator (space + U+00B7 + space).
pub const MIDDOT: &str = " \u{00B7} ";

fn unread_suffix(status: &str, notified: bool) -> Option<&'static str> {
    (status == "completed" && !notified).then_some(", unread")
}

/// local_agent: `{description} ({label}[, unread])`.
#[must_use]
pub fn render_local_agent_row(description: &str, status: &str, notified: bool) -> String {
    format!("{description} {}", render_task_status_text(status, unread_suffix(status, notified)))
}

/// remote_agent: `◇|◆ {title} · {progress}` where progress is `{done}/{total}`
/// when counts are known, else `done`/`error`/`stopped`/`{status}…`.
#[must_use]
pub fn render_remote_agent_row(title: &str, status: &str, progress: Option<(u64, u64)>) -> String {
    let diamond = if matches!(status, "running" | "pending") { DIAMOND_OPEN } else { DIAMOND_FILLED };
    let prog = match progress {
        Some((done, total)) => format!("{done}/{total}"),
        None => match status {
            "completed" => "done".to_string(),
            "failed" => "error".to_string(),
            "killed" => "stopped".to_string(),
            other => format!("{other}…"),
        },
    };
    format!("{diamond} {title}{MIDDOT}{prog}")
}

/// in_process_teammate: `@{name} : {activity}` when activity known, else
/// `@{name} ({label})`.
#[must_use]
pub fn render_in_process_teammate_row(name: &str, status: &str, activity: Option<&str>) -> String {
    match activity {
        Some(a) => format!("@{name} : {a}"),
        None => format!("@{name} {}", render_task_status_text(status, None)),
    }
}

/// local_workflow: `{name} ({n agents}|{label}[, unread])` — running shows the
/// agent count when known.
#[must_use]
pub fn render_local_workflow_row(name: &str, status: &str, agent_count: Option<u64>, notified: bool) -> String {
    let suffix = unread_suffix(status, notified);
    let text = match (status, agent_count) {
        ("running", Some(n)) => {
            let noun = if n == 1 { "agent" } else { "agents" };
            wrap_status_label(&format!("{n} {noun}"), suffix)
        }
        _ => render_task_status_text(status, suffix),
    };
    format!("{name} {text}")
}

/// monitor_mcp: `{description} ({label}[, unread])`.
#[must_use]
pub fn render_monitor_mcp_row(description: &str, status: &str, notified: bool) -> String {
    format!("{description} {}", render_task_status_text(status, unread_suffix(status, notified)))
}

/// dream: `{description}[ · {phase}][ · {detail}] ({label})`.
#[must_use]
pub fn render_dream_row(description: &str, status: &str, phase: Option<&str>, detail: Option<&str>) -> String {
    let mut out = description.to_string();
    if let Some(p) = phase {
        out.push_str(MIDDOT);
        out.push_str(p);
    }
    if let Some(d) = detail {
        out.push_str(MIDDOT);
        out.push_str(d);
    }
    format!("{out} {}", render_task_status_text(status, None))
}
```

- [ ] **Step 5: Run to verify pass** — `cargo test -p tui --lib components::tasks::rows` → 6 PASS. `cargo build -p tui`.

- [ ] **Step 6: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-04): 6 background-task type row renderers"
```

---

## Task 5: `render_task_row` dispatch + per-type snapshots

The live dispatch over a wire `TaskRow` (4 fields only) → the right renderer with rich fields defaulted (spec §2.4 omit). Plus insta snapshots fulfilling "snapshot per type".

**Files:** Modify `tui/src/components/tasks/mod.rs`; Create `tui/tests/render_task_rows.rs`.

- [ ] **Step 1: Implement the dispatch** — by now `tui/src/components/tasks/mod.rs` already declares `pub mod format;`, `status_text;`, `shell_progress;`, `rows;` (added by Tasks 1–4) and `output_tail;` (Task 6). **Do NOT re-declare them.** Just append the `use` + the dispatch fn below the existing `pub mod` lines:

```rust
use crate::multiagent::state::TaskRow;

/// Render a task list row from the wire-available `TaskRow` (claude-code
/// `BackgroundTask.tsx`). Only `task_type`, `status`, `description` are on the
/// wire, so rich per-type fields (elapsed, counts, `, unread`, activity, dream
/// phase) are passed as defaults here (spec §2.4 — omit, never invent); the
/// renderers accept them so fixtures/a richer feed light them up unchanged.
#[must_use]
pub fn render_task_row(row: &TaskRow) -> String {
    let d = row.description.as_str();
    let s = row.status.as_str();
    match row.task_type.as_str() {
        "local_bash" => shell_progress::render_shell_progress_to_string(d, s, None),
        "local_agent" => rows::render_local_agent_row(d, s, false),
        "remote_agent" => rows::render_remote_agent_row(d, s, None),
        "in_process_teammate" => rows::render_in_process_teammate_row(d, s, None),
        "local_workflow" => rows::render_local_workflow_row(d, s, None, false),
        "monitor_mcp" => rows::render_monitor_mcp_row(d, s, false),
        "dream" => rows::render_dream_row(d, s, None, None),
        _ => format!("{d} {}", status_text::render_task_status_text(s, None)),
    }
}
```

> Note: `render_task_row` does not reference `output_tail`, so this task is independent of Task 6. If `mod.rs` does not yet declare every submodule it should (because you are running tasks out of order), only add the `pub mod` line for a file that actually exists — never declare a module whose file is missing.

- [ ] **Step 2: Write per-type snapshots** — create `tui/tests/render_task_rows.rs`:

```rust
//! M9-04 — one snapshot per background-task type (live wire-shape rows) plus a
//! couple of rich-field variants proving the renderers handle full data.

use tui::components::tasks::render_task_row;
use tui::components::tasks::rows::{render_local_workflow_row, render_remote_agent_row};
use tui::multiagent::state::TaskRow;

fn row(task_type: &str, status: &str, description: &str) -> TaskRow {
    TaskRow {
        task_id: "b12345678".into(),
        task_type: task_type.into(),
        status: status.into(),
        description: description.into(),
    }
}

#[test]
fn task_row_local_bash() {
    insta::assert_snapshot!("task_row_local_bash", render_task_row(&row("local_bash", "running", "cargo build")));
}

#[test]
fn task_row_local_agent() {
    insta::assert_snapshot!("task_row_local_agent", render_task_row(&row("local_agent", "completed", "review")));
}

#[test]
fn task_row_remote_agent() {
    insta::assert_snapshot!("task_row_remote_agent", render_task_row(&row("remote_agent", "running", "deploy")));
}

#[test]
fn task_row_in_process_teammate() {
    insta::assert_snapshot!("task_row_in_process_teammate", render_task_row(&row("in_process_teammate", "running", "alice")));
}

#[test]
fn task_row_local_workflow() {
    insta::assert_snapshot!("task_row_local_workflow", render_task_row(&row("local_workflow", "running", "pipeline")));
}

#[test]
fn task_row_monitor_mcp() {
    insta::assert_snapshot!("task_row_monitor_mcp", render_task_row(&row("monitor_mcp", "running", "watch fs")));
}

#[test]
fn task_row_dream() {
    insta::assert_snapshot!("task_row_dream", render_task_row(&row("dream", "running", "nightly")));
}

#[test]
fn task_row_rich_variants() {
    insta::assert_snapshot!("task_row_remote_with_counts", render_remote_agent_row("deploy", "running", Some((3, 7))));
    insta::assert_snapshot!("task_row_workflow_with_agents", render_local_workflow_row("pipeline", "running", Some(4), false));
}
```

- [ ] **Step 3: Build + run + accept snapshots** — `cargo build -p tui`, `cargo test -p tui --test render_task_rows`, inspect the `.snap.new` files (confirm e.g. `task_row_remote_agent` = `◇ deploy · running…`), accept them, re-run green.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-04): render_task_row dispatch + per-type snapshots"
```

---

## Task 6: Output tail (live incremental reads)

A presentation buffer + an async tail that calls `TaskRegistryHandle::output(id, Some(offset))` and appends only the new bytes (spec §4 R6: fetch only new bytes via the existing offset).

**Files:** Create `tui/src/components/tasks/output_tail.rs`; Modify `tui/src/components/tasks/mod.rs` (ensure `pub mod output_tail;` is present — see Task 5 Step 1).

- [ ] **Step 1: Write the implementation + tests** — create `tui/src/components/tasks/output_tail.rs`:

```rust
//! Live output tail (M9-04): accumulate a task's spool by incrementally
//! reading `TaskRegistryHandle::output(id, Some(offset))` and appending only
//! the new bytes. `render_output_tail` shows the last N lines.

use platform_api::task_registry::{TaskRegistryError, TaskRegistryHandle};

/// Accumulated tail state for one task's spool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputTailState {
    /// All content seen so far.
    pub content: String,
    /// Byte offset of the next unread spool byte.
    pub offset: u64,
    /// Total line count reported by the spool.
    pub total_lines: u64,
    /// Whether the last chunk was truncated by a limit.
    pub truncated: bool,
}

/// Fetch the delta since `state.offset`, append it, and advance the offset by
/// the new content's byte length. Returns `Ok(true)` when new content arrived.
pub async fn tail_once(
    handle: &dyn TaskRegistryHandle,
    id: &str,
    state: &mut OutputTailState,
) -> Result<bool, TaskRegistryError> {
    let chunk = handle.output(id, Some(state.offset)).await?;
    let added = !chunk.content.is_empty();
    if added {
        state.offset += chunk.content.len() as u64;
        state.content.push_str(&chunk.content);
    }
    state.total_lines = chunk.total_lines;
    state.truncated = chunk.truncated;
    Ok(added)
}

/// The last `max_lines` lines of the accumulated content.
#[must_use]
pub fn render_output_tail(state: &OutputTailState, max_lines: usize) -> String {
    let lines: Vec<&str> = state.content.lines().collect();
    if lines.len() <= max_lines {
        return state.content.clone();
    }
    lines[lines.len() - max_lines..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use platform_api::task_registry::{
        TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskUpdatePatch,
    };

    /// Minimal stub: `output` returns the spool sliced at the byte offset.
    struct StubTasks {
        spool: Mutex<String>,
    }

    #[async_trait]
    impl TaskRegistryHandle for StubTasks {
        async fn create(&self, _i: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }
        async fn list(&self, _f: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(vec![])
        }
        async fn update(&self, _id: &str, _p: TaskUpdatePatch) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn set_status(&self, _id: &str, _s: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn output(&self, id: &str, offset: Option<u64>) -> Result<TaskOutputChunk, TaskRegistryError> {
            let spool = self.spool.lock().unwrap();
            let off = offset.unwrap_or(0) as usize;
            let content = spool.as_bytes().get(off..).map_or(String::new(), |b| {
                String::from_utf8_lossy(b).into_owned()
            });
            Ok(TaskOutputChunk {
                task_id: id.to_string(),
                content,
                total_lines: spool.lines().count() as u64,
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn tail_advances_on_new_spool_bytes() {
        let stub = StubTasks { spool: Mutex::new("hello\n".to_string()) };
        let mut state = OutputTailState::default();

        // First tail: reads "hello\n".
        let added = tail_once(&stub, "b12345678", &mut state).await.unwrap();
        assert!(added);
        assert_eq!(state.content, "hello\n");
        assert_eq!(state.offset, 6);
        assert_eq!(state.total_lines, 1);

        // No new bytes → no advance.
        let added = tail_once(&stub, "b12345678", &mut state).await.unwrap();
        assert!(!added);
        assert_eq!(state.content, "hello\n");
        assert_eq!(state.offset, 6);

        // Append to the spool, tail again → only the new bytes are added.
        *stub.spool.lock().unwrap() = "hello\nworld\n".to_string();
        let added = tail_once(&stub, "b12345678", &mut state).await.unwrap();
        assert!(added);
        assert_eq!(state.content, "hello\nworld\n");
        assert_eq!(state.offset, 12);
        assert_eq!(state.total_lines, 2);
    }

    #[test]
    fn render_tail_limits_lines() {
        let state = OutputTailState {
            content: "a\nb\nc\nd\ne".into(),
            offset: 9,
            total_lines: 5,
            truncated: false,
        };
        assert_eq!(render_output_tail(&state, 2), "d\ne");
        assert_eq!(render_output_tail(&state, 10), "a\nb\nc\nd\ne");
    }
}
```

- [ ] **Step 2: Run + build** — `cargo test -p tui --lib components::tasks::output_tail` → 2 PASS. `cargo build -p tui`.

- [ ] **Step 3: Commit**

```bash
cargo fmt -p tui
git add -A
git commit -m "feat(M9-04): live output-tail (incremental offset reads) + behavior test"
```

---

## Task 7: M9-04 gate + tag

**Files:** none (verification + tag only).

- [ ] **Step 1: Format** — from `lingxi-code/`: `cargo fmt --check`. If dirty: `cargo fmt -p tui` + commit `style(M9-04): cargo fmt`.

- [ ] **Step 2: Clippy (tui scope)** — `cargo clippy -p tui --all-targets 2>&1 | grep -E "tui/src|tui/tests"` — expect ZERO tui-originated warnings. (Pre-existing `tool-api` `-D warnings` debt is out of scope, per M9-01/02/03 precedent — do not fix it here.)

- [ ] **Step 3: Full tui tests** — `cargo test -p tui` → all pass; confirm no `.snap.new` left (`find tui/tests -name '*.snap.new'` empty).

- [ ] **Step 4: Workspace build** — `cargo build --workspace` → exit 0.

- [ ] **Step 5: Tag**

```bash
git tag -a m9.4 -m "M9-04: background-task rows (7) + ShellProgress + status-text + duration/exit formatters + live output-tail"
git tag | grep -E '^m9' | sort -V
```

Expect `m9.1`…`m9.4`. **No remote push.**

---

## Forward notes

- **M9-05** builds the iocraft components + `BackgroundTasksDialog` (list↔detail) that *display* these rows with theme colors (`task_status_color` on the `(label)`), the `BackgroundTaskStatus` footer, and the live pump that drives `tail_once` on a tick. The `format_exit_code` helper feeds `ShellDetailDialog`'s `{status} (exit code: N)` line.
- The rich per-type fields (counts, `, unread`, activity, dream phase, elapsed/exit) are renderer parameters today; they light up unchanged if the `TaskRegistryHandle` surface ever widens (a separate engine milestone — not M9).

---

**End of plan.**
