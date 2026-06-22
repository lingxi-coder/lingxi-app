//! Per-turn `task-notification` reminder — the fold-back of background
//! (terminal) tasks that finished since the last turn.
//!
//! Mirrors [`async_hook_response`](crate::prompt::async_hook_response): when a
//! background task (a `BashOutput`-style `local_bash`, a backgrounded
//! `local_agent`, an MCP `monitor`, …) reaches a terminal status, claude-code
//! enqueues exactly ONE `<task-notification>` so the model learns its async task
//! finished and can `Read` the output file. claude-code does this from a
//! per-task-type completion callback (`enqueueShellNotification` /
//! `enqueueAgentNotification` / …); this engine surfaces the same set at the
//! turn boundary by DRAINING the registry's terminal-not-notified tasks once per
//! turn (the registry marks each `notified` + evicts on drain, so a completion
//! is reported exactly once — the same `notified`-guard claude-code uses).
//!
//! Like the skill-/agent-listing and async-hook reminders, the message is
//! appended ONLY to the per-turn OUTGOING snapshot (never `session.history` /
//! JSONL), so it never accumulates. When no task finished since the last turn
//! the reminder is `None` — byte-identical to a build with no background tasks.
//!
//! ## Byte-faithful per-type formats
//!
//! claude-code does NOT use one generic format for completions — each task type
//! has its own. This module ports the formats that are actually produced for the
//! background-task types this engine runs:
//!
//! - `local_bash` (`enqueueShellNotification`, `LocalShellTask.tsx`): no
//!   `<task-type>` tag; summary `Background command "{desc}" completed (exit code
//!   N)` / `… failed with exit code N` / `… was stopped`. The summary is
//!   `escapeXml`-escaped.
//! - `monitor_mcp` (`enqueueShellNotification`, `monitor` kind): summary
//!   `Monitor "{desc}" stream ended` / `Monitor "{desc}" script failed (exit N)`
//!   / `Monitor "{desc}" stopped`. Also `escapeXml`-escaped.
//! - `local_agent` (`enqueueAgentNotification`, `LocalAgentTask.tsx`): summary
//!   `Agent "{desc}" completed` / `Agent "{desc}" failed: {error or 'Unknown
//!   error'}` / `Agent "{desc}" was stopped`. NOT escaped (claude-code
//!   interpolates the agent summary raw).
//! - any other type: the generic `framework.ts` `enqueueTaskNotification` format
//!   (the only one with a `<task-type>` tag): summary `Task "{desc}"
//!   {statusText}` where `statusText` is `completed successfully` / `failed` /
//!   `was stopped`.
//!
//! The streaming output-delta attachments (per-poll partial output) are a
//! separate, deferred surface — this module is completion-only.

use async_trait::async_trait;
use traits::task_registry::TaskNotification;

/// Supplies the terminal background tasks not yet surfaced to the model since
/// the previous call.
///
/// CONSUME-ONCE: each call DRAINS the registry's terminal-not-notified set — a
/// finished task surfaces in exactly one turn's reminder (the registry marks it
/// `notified` + evicts on drain). The production impl (desktop) is backed by the
/// `TaskRegistry`; tests inject a static fixture.
#[async_trait]
pub trait TaskNotificationProvider: Send + Sync {
    /// Drain + return a snapshot of each terminal task finished since the
    /// previous call, in registry-iteration order. Returns empty when nothing
    /// finished → no reminder this turn.
    async fn take_pending_task_notifications(&self) -> Vec<TaskNotification>;
}

/// `escapeXml` (claude-code `utils/xml.ts`): escape `&`, `<`, `>` for safe
/// interpolation into element text content. Order matters — `&` first so the
/// entities introduced by the later replaces are not double-escaped.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render ONE `<task-notification>` block for a drained task, dispatching on its
/// `task_type` to the byte-faithful per-type format.
fn render_one(n: &TaskNotification) -> String {
    // The `<output-file>` path: the real spool path when known, else the bare
    // `<taskId>.output` filename claude-code's `getTaskOutputPath` would join.
    let output_file = n
        .output_path
        .clone()
        .unwrap_or_else(|| format!("{}.output", n.task_id));
    // Optional `<tool-use-id>` line — a leading "\n" so it slots between the
    // `<task-id>` line and the next tag (claude-code `toolUseIdLine`).
    let tool_use_id_line = match &n.tool_use_id {
        Some(id) => format!("\n<tool-use-id>{id}</tool-use-id>"),
        None => String::new(),
    };

    match n.task_type.as_str() {
        "local_agent" => {
            // `enqueueAgentNotification` (v2.1.185 binary @202650700): the
            // "came to rest" summary, an always-present `<note>`, and optional
            // `<result>` / `<usage>` sections. The summary is escaped via `Np`
            // (== [`escape_xml`]: `&<>` only) — a change from the prior raw
            // interpolation, matching the v2.1.185 renderer (`<summary>${Np(g)}`).
            let summary = match n.status.as_str() {
                "completed" => format!("Agent \"{}\" came to rest", n.description),
                "failed" => {
                    let err = n.error.as_deref().unwrap_or("Unknown error");
                    format!("Agent \"{}\" came to rest with an error: {err}", n.description)
                }
                // `killed` (and any other terminal) → "(stopped by user)".
                _ => format!("Agent \"{}\" came to rest (stopped by user)", n.description),
            };
            // Hardcoded, always-present `<note>` (binary @202651760).
            const NOTE: &str = "A task-notification fires each time this agent comes to rest with no live background children of its own. The user can send it another message and resume it, so the same task-id may notify more than once.";
            // Optional `<result>` (escaped) — claude-code `s ? \n<result>${Np(s)}</result> : ''`.
            let result_section = match &n.result {
                Some(r) => format!("\n<result>{}</result>", escape_xml(r)),
                None => String::new(),
            };
            // Optional `<usage>` — claude-code `i ? \n<usage>…</usage> : ''`.
            let usage_section = match &n.usage {
                Some(u) => format!(
                    "\n<usage><subagent_tokens>{}</subagent_tokens><tool_uses>{}</tool_uses><duration_ms>{}</duration_ms></usage>",
                    u.subagent_tokens, u.tool_uses, u.duration_ms
                ),
                None => String::new(),
            };
            // The binary's trailing `${v}` (a teammate children/messages block,
            // `$mr` tag) is another OPTIONAL section, omitted here — LingXi's
            // local_agent carries no such data, so its absence is byte-faithful
            // (the same `data ? … : ''` shape as result/usage).
            format!(
                "<task-notification>\n<task-id>{}</task-id>{tool_use_id_line}\n<output-file>{output_file}</output-file>\n<status>{}</status>\n<summary>{}</summary>\n<note>{NOTE}</note>{result_section}{usage_section}\n</task-notification>",
                n.task_id,
                n.status,
                escape_xml(&summary)
            )
        }
        "local_bash" => {
            // `enqueueShellNotification` (bash kind) — no `<task-type>`; summary
            // escaped. The exit-code clause is omitted when `exit_code` is None.
            let exit = n.exit_code;
            let summary = match n.status.as_str() {
                "completed" => format!(
                    "Background command \"{}\" completed{}",
                    n.description,
                    exit.map_or(String::new(), |c| format!(" (exit code {c})"))
                ),
                "failed" => format!(
                    "Background command \"{}\" failed{}",
                    n.description,
                    exit.map_or(String::new(), |c| format!(" with exit code {c}"))
                ),
                _ => format!("Background command \"{}\" was stopped", n.description),
            };
            format!(
                "<task-notification>\n<task-id>{}</task-id>{tool_use_id_line}\n<output-file>{output_file}</output-file>\n<status>{}</status>\n<summary>{}</summary>\n</task-notification>",
                n.task_id,
                n.status,
                escape_xml(&summary)
            )
        }
        "monitor_mcp" => {
            // `enqueueShellNotification` (monitor kind) — no `<task-type>`;
            // summary escaped.
            let exit = n.exit_code;
            let summary = match n.status.as_str() {
                "completed" => format!("Monitor \"{}\" stream ended", n.description),
                "failed" => format!(
                    "Monitor \"{}\" script failed{}",
                    n.description,
                    exit.map_or(String::new(), |c| format!(" (exit {c})"))
                ),
                _ => format!("Monitor \"{}\" stopped", n.description),
            };
            format!(
                "<task-notification>\n<task-id>{}</task-id>{tool_use_id_line}\n<output-file>{output_file}</output-file>\n<status>{}</status>\n<summary>{}</summary>\n</task-notification>",
                n.task_id,
                n.status,
                escape_xml(&summary)
            )
        }
        other => {
            // Generic `framework.ts` `enqueueTaskNotification` — the only format
            // with a `<task-type>` tag. Summary `Task "{desc}" {statusText}`.
            let status_text = match n.status.as_str() {
                "completed" => "completed successfully",
                "failed" => "failed",
                "killed" => "was stopped",
                "running" => "is running",
                _ => "is pending",
            };
            format!(
                "<task-notification>\n<task-id>{}</task-id>{tool_use_id_line}\n<task-type>{other}</task-type>\n<output-file>{output_file}</output-file>\n<status>{}</status>\n<summary>Task \"{}\" {status_text}</summary>\n</task-notification>",
                n.task_id, n.status, n.description
            )
        }
    }
}

/// Render the `task-notification` `<system-reminder>` body from the drained
/// tasks, or `None` when there is nothing to surface.
///
/// Each task renders to its own `<task-notification>` block; all blocks are
/// joined with `\n` and wrapped in ONE `<system-reminder>` (the same batch-wrap
/// the async-hook reminder uses). Empty input → `None` → no reminder this turn.
#[must_use]
pub fn render_reminder(notifications: &[TaskNotification]) -> Option<String> {
    if notifications.is_empty() {
        return None;
    }
    let body = notifications
        .iter()
        .map(render_one)
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("<system-reminder>\n{body}\n</system-reminder>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(id: &str, ty: &str, status: &str, desc: &str) -> TaskNotification {
        TaskNotification {
            task_id: id.to_string(),
            task_type: ty.to_string(),
            status: status.to_string(),
            description: desc.to_string(),
            tool_use_id: None,
            output_path: Some(format!("/tmp/tasks/{id}.output")),
            exit_code: None,
            error: None,
            result: None,
            usage: None,
        }
    }

    #[test]
    fn empty_yields_no_reminder() {
        assert_eq!(render_reminder(&[]), None);
    }

    #[test]
    fn bash_completed_with_exit_code_is_byte_faithful() {
        let mut n = base("b12345678", "local_bash", "completed", "run tests");
        n.exit_code = Some(0);
        let out = render_reminder(std::slice::from_ref(&n)).expect("reminder");
        assert_eq!(
            out,
            "<system-reminder>\n\
<task-notification>\n\
<task-id>b12345678</task-id>\n\
<output-file>/tmp/tasks/b12345678.output</output-file>\n\
<status>completed</status>\n\
<summary>Background command \"run tests\" completed (exit code 0)</summary>\n\
</task-notification>\n\
</system-reminder>"
        );
    }

    #[test]
    fn bash_failed_with_exit_code_uses_with_exit_code_phrasing() {
        let mut n = base("b12345678", "local_bash", "failed", "build");
        n.exit_code = Some(2);
        let block = render_one(&n);
        assert!(
            block.contains("<summary>Background command \"build\" failed with exit code 2</summary>"),
            "got: {block}"
        );
    }

    #[test]
    fn bash_killed_is_was_stopped_with_no_exit_clause() {
        let n = base("b12345678", "local_bash", "killed", "sleeper");
        let block = render_one(&n);
        assert!(
            block.contains("<summary>Background command \"sleeper\" was stopped</summary>"),
            "got: {block}"
        );
    }

    #[test]
    fn agent_completed_is_byte_faithful_with_note_and_escaped_summary() {
        // v2.1.185 `enqueueAgentNotification`: "came to rest" summary, escaped
        // via `Np` (`<` in the description → `&lt;`), always-present `<note>`,
        // and NO `<result>`/`<usage>` when absent (the byte-faithful no-result case).
        let n = base("a12345678", "local_agent", "completed", "scan <repo>");
        let out = render_reminder(std::slice::from_ref(&n)).expect("reminder");
        assert_eq!(
            out,
            "<system-reminder>\n\
<task-notification>\n\
<task-id>a12345678</task-id>\n\
<output-file>/tmp/tasks/a12345678.output</output-file>\n\
<status>completed</status>\n\
<summary>Agent \"scan &lt;repo&gt;\" came to rest</summary>\n\
<note>A task-notification fires each time this agent comes to rest with no live background children of its own. The user can send it another message and resume it, so the same task-id may notify more than once.</note>\n\
</task-notification>\n\
</system-reminder>"
        );
    }

    #[test]
    fn agent_failed_uses_came_to_rest_with_error_or_unknown() {
        let mut n = base("a12345678", "local_agent", "failed", "research");
        n.error = Some("rate limited".to_string());
        assert!(
            render_one(&n).contains(
                "<summary>Agent \"research\" came to rest with an error: rate limited</summary>"
            ),
            "got: {}",
            render_one(&n)
        );
        n.error = None;
        assert!(render_one(&n).contains(
            "<summary>Agent \"research\" came to rest with an error: Unknown error</summary>"
        ));
    }

    #[test]
    fn agent_killed_came_to_rest_stopped_by_user() {
        let n = base("a12345678", "local_agent", "killed", "long job");
        assert!(
            render_one(&n)
                .contains("<summary>Agent \"long job\" came to rest (stopped by user)</summary>"),
            "got: {}",
            render_one(&n)
        );
    }

    /// The optional `<result>` (escaped) + `<usage>` sections, byte-faithful to
    /// `enqueueAgentNotification`'s `s ? <result> : ''` / `i ? <usage> : ''`,
    /// slotting after `<note>` in declaration order.
    #[test]
    fn agent_result_and_usage_sections_when_present() {
        let mut n = base("a12345678", "local_agent", "completed", "audit");
        n.result = Some("Found 2 bugs in <auth>".to_string());
        n.usage = Some(traits::task_registry::AgentRunUsage {
            subagent_tokens: 1234,
            tool_uses: 7,
            duration_ms: 4200,
        });
        assert_eq!(
            render_one(&n),
            "<task-notification>\n\
<task-id>a12345678</task-id>\n\
<output-file>/tmp/tasks/a12345678.output</output-file>\n\
<status>completed</status>\n\
<summary>Agent \"audit\" came to rest</summary>\n\
<note>A task-notification fires each time this agent comes to rest with no live background children of its own. The user can send it another message and resume it, so the same task-id may notify more than once.</note>\n\
<result>Found 2 bugs in &lt;auth&gt;</result>\n\
<usage><subagent_tokens>1234</subagent_tokens><tool_uses>7</tool_uses><duration_ms>4200</duration_ms></usage>\n\
</task-notification>"
        );
    }

    /// `<usage>` present but `<result>` absent: only the usage section rides.
    #[test]
    fn agent_usage_without_result() {
        let mut n = base("a12345678", "local_agent", "completed", "audit");
        n.usage = Some(traits::task_registry::AgentRunUsage {
            subagent_tokens: 5,
            tool_uses: 0,
            duration_ms: 1,
        });
        let block = render_one(&n);
        assert!(!block.contains("<result>"), "no result section; got: {block}");
        assert!(
            block.contains(
                "<usage><subagent_tokens>5</subagent_tokens><tool_uses>0</tool_uses><duration_ms>1</duration_ms></usage>"
            ),
            "got: {block}"
        );
    }

    #[test]
    fn tool_use_id_line_slots_after_task_id() {
        let mut n = base("b12345678", "local_bash", "completed", "x");
        n.tool_use_id = Some("toolu_42".to_string());
        let block = render_one(&n);
        assert!(
            block.contains("<task-id>b12345678</task-id>\n<tool-use-id>toolu_42</tool-use-id>\n<output-file>"),
            "got: {block}"
        );
    }

    #[test]
    fn bash_summary_is_xml_escaped() {
        // A `&` / `<` / `>` in the description is escaped in the SHELL summary.
        let n = base("b12345678", "local_bash", "completed", "a && b <c>");
        let block = render_one(&n);
        assert!(
            block.contains(
                "<summary>Background command \"a &amp;&amp; b &lt;c&gt;\" completed</summary>"
            ),
            "got: {block}"
        );
    }

    #[test]
    fn monitor_completed_says_stream_ended() {
        let n = base("m12345678", "monitor_mcp", "completed", "watch");
        let block = render_one(&n);
        assert!(
            block.contains("<summary>Monitor \"watch\" stream ended</summary>"),
            "got: {block}"
        );
        // Monitor uses the no-`<task-type>` shell format.
        assert!(!block.contains("<task-type>"), "got: {block}");
    }

    #[test]
    fn generic_type_uses_task_type_tag_and_status_text() {
        let n = base("w12345678", "local_workflow", "completed", "deploy");
        let block = render_one(&n);
        assert!(block.contains("<task-type>local_workflow</task-type>"), "got: {block}");
        assert!(
            block.contains("<summary>Task \"deploy\" completed successfully</summary>"),
            "got: {block}"
        );
    }

    #[test]
    fn multiple_tasks_join_into_one_reminder() {
        let a = base("b00000001", "local_bash", "completed", "one");
        let b = base("a00000002", "local_agent", "completed", "two");
        let out = render_reminder(&[a, b]).expect("reminder");
        assert_eq!(out.matches("<system-reminder>").count(), 1);
        assert_eq!(out.matches("<task-notification>").count(), 2);
    }
}
