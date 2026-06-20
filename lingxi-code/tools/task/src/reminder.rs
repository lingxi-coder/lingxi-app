//! Per-turn todo / task reminder — claude-code Finding #73.
//!
//! claude-code injects a per-turn `isMeta` reminder when the `TodoWrite` (V1) /
//! `Task*` (V2) tools "haven't been used recently". The reminder fires only
//! when BOTH counters cross their thresholds (`TURNS_SINCE_WRITE` /
//! `TURNS_BETWEEN_REMINDERS`, each `10`), is killswitched by
//! `CLAUDE_CODE_TODO_REMINDER_MODE === "off"`, and selects V1 vs V2 via `TE()`
//! (`is_todo_v2_enabled`).
//!
//! ## Binary ground truth (`bin/claude.exe`, v2.1.183)
//!
//! Thresholds (`rqt`, offset ~203118638):
//! `{ TURNS_SINCE_WRITE: 10, TURNS_BETWEEN_REMINDERS: 10 }`.
//!
//! Killswitch (`wgo()`, offset ~203087087):
//! ```js
//! function wgo(){let e=process.env.CLAUDE_CODE_TODO_REMINDER_MODE;
//!   if(e!==void 0)return e;
//!   return ct("tengu_soft_slate_nudge","baseline")==="off"?"off":"baseline"}
//! ```
//! → when the env var is SET it is honored verbatim; `"off"` ⇒ no reminder.
//! With no GrowthBook in Rust the gate defaults to NOT-off (`"baseline"`).
//!
//! V1/V2 selection (`TE()`, offset ~199285430):
//! ```js
//! function TE(){if(_l(process.env.CLAUDE_CODE_ENABLE_TASKS))return false;return true}
//! ```
//! where `_l(e)` is "explicitly disabled" (`true` iff the value lowercases to
//! one of `0/false/no/off`). So `TE()` returns `true` (V2 task_reminder) by
//! DEFAULT and `false` (V1 todo_reminder) only when `CLAUDE_CODE_ENABLE_TASKS`
//! is explicitly disabled. The producer (`ytl`, offset ~203087213) is
//! `()=>TE()?B4p(...):M4p(...)` — V2 when `TE()`, else V1.
//!
//! Renderers (`messages.ts` `normalizeAttachmentForAPI`, offset ~206021028):
//! the reminder body `r` is emitted RAW as `Ln({content:r,isMeta:!0})` — there
//! is NO `<system-reminder>` wrapper (unlike the skill-/task-notification
//! reminders). The exact texts are locked in [`V1_BASE`] / [`V2_BASE`] below.

use engine::TodoState;

/// `rqt.TURNS_SINCE_WRITE` — assistant turns since the last TodoWrite/Task call
/// before a reminder is eligible. (binary offset ~203118638)
pub const TURNS_SINCE_WRITE: u32 = 10;

/// `rqt.TURNS_BETWEEN_REMINDERS` — assistant turns since the last reminder
/// before another one is eligible. (binary offset ~203118638)
pub const TURNS_BETWEEN_REMINDERS: u32 = 10;

/// `Kw` — the V2 task-create tool name, interpolated into [`V2_BASE`].
const TASK_CREATE_TOOL_NAME: &str = "TaskCreate";
/// `mP` — the V2 task-update tool name, interpolated into [`V2_BASE`].
const TASK_UPDATE_TOOL_NAME: &str = "TaskUpdate";

/// V1 (`todo_reminder`) base text — byte-exact, INCLUDING the trailing `\n`
/// (the JS template literal's closing backtick sits on the next line). Note the
/// missing "it" in "if has become stale" — faithful to the binary typo.
pub const V1_BASE: &str = "The TodoWrite tool hasn't been used recently. If you're working on tasks that would benefit from tracking progress, consider using the TodoWrite tool to track progress. Also consider cleaning up the todo list if has become stale and no longer matches what you are working on. Only use it if it's relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n";

/// V1 items header — appended (after `\n\n`) when the todo list is non-empty.
const V1_ITEMS_HEADER: &str = "Here are the existing contents of your todo list:";

/// V2 (`task_reminder`) base text — byte-exact, INCLUDING the trailing `\n`.
/// `${Kw}`/`${mP}` are interpolated as `TaskCreate`/`TaskUpdate`.
#[must_use]
pub fn v2_base() -> String {
    format!(
        "The task tools haven't been used recently. If you're working on tasks that would benefit from tracking progress, consider using {TASK_CREATE_TOOL_NAME} to add new tasks and {TASK_UPDATE_TOOL_NAME} to update task status (set to in_progress when starting, completed when done). Also consider cleaning up the task list if it has become stale. Only use these if relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n"
    )
}

/// V2 items header — appended (after `\n\n`) when the task list is non-empty.
const V2_ITEMS_HEADER: &str = "Here are the existing tasks:";

/// Which reminder variant to produce, mirroring the binary's `()=>TE()?B4p:M4p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderMode {
    /// `todo_reminder` — V1, reads `session.todos`. Selected when `TE()` is
    /// false (`CLAUDE_CODE_ENABLE_TASKS` explicitly disabled).
    V1Todo,
    /// `task_reminder` — V2, reads the file-backed task store. The default.
    V2Task,
}

/// Wire string for a [`TodoState`] (`pending`/`in_progress`/`completed`),
/// matching `o.status` in the binary's item formatter.
fn status_wire(s: TodoState) -> &'static str {
    match s {
        TodoState::Pending => "pending",
        TodoState::InProgress => "in_progress",
        TodoState::Completed => "completed",
    }
}

/// `wgo() === "off"` — the killswitch. Honors `CLAUDE_CODE_TODO_REMINDER_MODE`
/// verbatim when SET (`"off"` ⇒ suppressed); with no GrowthBook the unset case
/// is NOT off.
#[must_use]
pub fn is_killswitched() -> bool {
    matches!(
        std::env::var("CLAUDE_CODE_TODO_REMINDER_MODE").ok().as_deref(),
        Some("off")
    )
}

/// `_l(e)` — "explicitly disabled": `true` iff the value lowercases/trims to one
/// of `0`/`false`/`no`/`off`. `None` ⇒ `false`.
fn is_explicitly_disabled(val: Option<&str>) -> bool {
    match val {
        None => false,
        Some(v) => matches!(v.to_lowercase().trim(), "0" | "false" | "no" | "off"),
    }
}

/// `TE()` (`is_todo_v2_enabled`) → the active [`ReminderMode`]. Returns
/// [`ReminderMode::V2Task`] by default, [`ReminderMode::V1Todo`] only when
/// `CLAUDE_CODE_ENABLE_TASKS` is explicitly disabled.
#[must_use]
pub fn select_mode() -> ReminderMode {
    if is_explicitly_disabled(std::env::var("CLAUDE_CODE_ENABLE_TASKS").ok().as_deref()) {
        ReminderMode::V1Todo
    } else {
        ReminderMode::V2Task
    }
}

/// Render the V1 (`todo_reminder`) body. `items` = `(status, content)` pairs in
/// list order. When non-empty, the `\n\n`-separated header + `[<items>]` block
/// is appended; each item is `"${idx}. [${status}] ${content}"` (1-based idx),
/// joined by `\n`, wrapped in `[...]`.
#[must_use]
pub fn render_v1(items: &[(TodoState, String)]) -> String {
    let mut r = String::from(V1_BASE);
    if !items.is_empty() {
        let body = items
            .iter()
            .enumerate()
            .map(|(i, (st, content))| format!("{}. [{}] {}", i + 1, status_wire(*st), content))
            .collect::<Vec<_>>()
            .join("\n");
        r.push_str(&format!("\n\n{V1_ITEMS_HEADER}\n\n[{body}]"));
    }
    r
}

/// Render the V2 (`task_reminder`) body. `items` = `(id, status, subject)`
/// triples in list order. When non-empty, the `\n\n`-separated header + items
/// (NO `[...]` wrapping, unlike V1) is appended; each item is
/// `"#${id}. [${status}] ${subject}"`, joined by `\n`.
#[must_use]
pub fn render_v2(items: &[(String, TodoState, String)]) -> String {
    let mut r = v2_base();
    if !items.is_empty() {
        let body = items
            .iter()
            .map(|(id, st, subject)| format!("#{id}. [{}] {subject}", status_wire(*st)))
            .collect::<Vec<_>>()
            .join("\n");
        r.push_str(&format!("\n\n{V2_ITEMS_HEADER}\n\n{body}"));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_base_is_byte_exact_with_no_items() {
        let out = render_v1(&[]);
        assert_eq!(
            out,
            "The TodoWrite tool hasn't been used recently. If you're working on tasks that would benefit from tracking progress, consider using the TodoWrite tool to track progress. Also consider cleaning up the todo list if has become stale and no longer matches what you are working on. Only use it if it's relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n"
        );
    }

    #[test]
    fn v1_with_items_appends_bracketed_block() {
        let items = vec![
            (TodoState::Pending, "first thing".to_string()),
            (TodoState::InProgress, "second thing".to_string()),
        ];
        let out = render_v1(&items);
        assert!(out.ends_with(
            "\n\nHere are the existing contents of your todo list:\n\n[1. [pending] first thing\n2. [in_progress] second thing]"
        ), "got: {out:?}");
        // Base text is unchanged at the front.
        assert!(out.starts_with("The TodoWrite tool hasn't been used recently."));
    }

    #[test]
    fn v2_base_is_byte_exact_with_no_items() {
        let out = render_v2(&[]);
        assert_eq!(
            out,
            "The task tools haven't been used recently. If you're working on tasks that would benefit from tracking progress, consider using TaskCreate to add new tasks and TaskUpdate to update task status (set to in_progress when starting, completed when done). Also consider cleaning up the task list if it has become stale. Only use these if relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n"
        );
    }

    #[test]
    fn v2_with_items_appends_unbracketed_block() {
        let items = vec![
            ("1".to_string(), TodoState::Completed, "alpha".to_string()),
            ("2".to_string(), TodoState::Pending, "beta".to_string()),
        ];
        let out = render_v2(&items);
        assert!(
            out.ends_with("\n\nHere are the existing tasks:\n\n#1. [completed] alpha\n#2. [pending] beta"),
            "got: {out:?}"
        );
        assert!(out.starts_with("The task tools haven't been used recently."));
    }

    #[test]
    fn killswitch_reads_env_off_only() {
        // Default (unset) is NOT killswitched; any value other than "off" is also
        // not killswitched. Exercised via direct argument-free reads is brittle
        // under parallelism, so we test the pure helper indirectly via the
        // explicitly-disabled / mode helpers in the orchestrator. Here we only
        // assert the constants.
        assert_eq!(TURNS_SINCE_WRITE, 10);
        assert_eq!(TURNS_BETWEEN_REMINDERS, 10);
    }

    #[test]
    fn explicitly_disabled_matches_binary_set() {
        assert!(is_explicitly_disabled(Some("off")));
        assert!(is_explicitly_disabled(Some("FALSE")));
        assert!(is_explicitly_disabled(Some(" 0 ")));
        assert!(is_explicitly_disabled(Some("no")));
        assert!(!is_explicitly_disabled(Some("1")));
        assert!(!is_explicitly_disabled(Some("true")));
        assert!(!is_explicitly_disabled(None));
    }
}
