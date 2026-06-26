//! `CronListTool` — list every scheduled cron job by reading the single
//! project-relative `<root>/.claude/scheduled_tasks.json` file.
//!
//! 1:1 parity port of claude-code `CronListTool.ts`. Returns a `jobs` array of
//! `{id, cron, humanSchedule, prompt, recurring?, durable?}` plus a flattened
//! result text for the model, one line per job.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Map, Value};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{CRON_LIST_COMPLETED, CRON_LIST_STARTED};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

use crate::schedule_cron::cron_to_human;

/// Tool name byte-lock.
pub const CRON_LIST_TOOL_NAME: &str = "CronList";

/// Model-facing description (CronListTool.ts description()).
const CRON_LIST_DESCRIPTION: &str = "List all cron jobs scheduled via CronCreate.";

/// Empty `strictObject({})` schema — CronList takes no input.
static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {},
        "required": []
    })
});

/// Truncate `s` to `max_width` columns, appending `…` if it was longer.
///
/// Port of claude-code `utils/truncate.ts` `truncateToWidth`.
///
/// PARITY-GAP: TS uses `stringWidth` (terminal cell width — CJK glyphs count
/// as 2) plus a grapheme segmenter; this seam has no ink/`stringWidth`
/// dependency, so we count Unicode scalar values (chars). Equivalent for the
/// ASCII prompts cron jobs carry in practice.
fn truncate_to_width(s: &str, max_width: usize) -> String {
    if s.chars().count() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return "\u{2026}".to_string();
    }
    // TS walks graphemes accumulating `stringWidth`, breaking before exceeding
    // `maxWidth - 1`. Counting each char as width 1 collapses that to taking
    // the first `max_width - 1` chars, then appending the ellipsis.
    let mut result: String = s.chars().take(max_width - 1).collect();
    result.push('\u{2026}');
    result
}

/// Port of claude-code `utils/truncate.ts` `truncate(str, maxWidth, true)`
/// (single-line mode): cut at the first newline, else width-truncate.
fn truncate_single_line(s: &str, max_width: usize) -> String {
    if let Some(idx) = s.find('\n') {
        let head = &s[..idx];
        if head.chars().count() + 1 > max_width {
            return truncate_to_width(head, max_width);
        }
        return format!("{head}\u{2026}");
    }
    if s.chars().count() <= max_width {
        return s.to_string();
    }
    truncate_to_width(s, max_width)
}

/// Read the single `<root>/.claude/scheduled_tasks.json` file into the `jobs`
/// shape, sorted by id for determinism. A missing / unparseable file yields no
/// jobs. Every persisted task is durable by definition, so the `durable:false`
/// key (CronListTool.ts' `durable === false` spread, which only applies to the
/// separate in-memory session tasks) is never emitted here.
async fn read_all_jobs(project_root: &Path) -> Vec<Value> {
    let path = cron::tasks_file::scheduled_tasks_path(project_root);
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(b) => b,
        Err(_) => return Vec::new(), // file absent → no jobs
    };
    let doc = cron::tasks_file::parse_tasks(&body);

    let mut jobs: Vec<Value> = doc
        .tasks
        .into_iter()
        .map(|t| {
            let mut obj = Map::new();
            obj.insert("id".into(), json!(t.id));
            obj.insert("cron".into(), json!(t.cron));
            obj.insert("humanSchedule".into(), json!(cron_to_human(&t.cron)));
            obj.insert("prompt".into(), json!(t.prompt));
            // Conditional spread (CronListTool.ts): `recurring` only when truthy.
            if t.recurring == Some(true) {
                obj.insert("recurring".into(), json!(true));
            }
            Value::Object(obj)
        })
        .collect();
    jobs.sort_by(|a, b| {
        a.get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("id").and_then(Value::as_str).unwrap_or(""))
    });
    jobs
}

/// Flatten the `jobs` array to the model-facing result text
/// (CronListTool.ts `mapToolResultToToolResultBlockParam`).
fn render_result(jobs: &[Value]) -> String {
    if jobs.is_empty() {
        return "No scheduled jobs.".to_string();
    }
    jobs.iter()
        .map(|j| {
            let id = j.get("id").and_then(Value::as_str).unwrap_or("");
            let human = j.get("humanSchedule").and_then(Value::as_str).unwrap_or("");
            let prompt = j.get("prompt").and_then(Value::as_str).unwrap_or("");
            let recurring_label = if j.get("recurring").and_then(Value::as_bool) == Some(true) {
                " (recurring)"
            } else {
                " (one-shot)"
            };
            let durable_label = if j.get("durable").and_then(Value::as_bool) == Some(false) {
                " [session-only]"
            } else {
                ""
            };
            // U+2014 EM DASH separator, matching the TS template literal " — ".
            format!(
                "{id} \u{2014} {human}{recurring_label}{durable_label}: {}",
                truncate_single_line(prompt, 80)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `CronListTool` — list scheduled cron jobs.
pub struct CronListTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl CronListTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str {
        CRON_LIST_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "CronList reads .claude/scheduled_tasks.json".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        CRON_LIST_DESCRIPTION.into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "CronList: list every cron job scheduled via CronCreate.".into()
    }

    async fn validate_input(
        &self,
        _input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        bus.log_event(CRON_LIST_STARTED, HashMap::new()).await;

        // PARITY-GAP: TS filters to the calling teammate's own crons
        // (`ctx ? allTasks.filter(t => t.agentId === ctx.agentId) : allTasks`).
        // There is no teammate context in this Rust seam, so every persisted
        // job is listed.
        let jobs = read_all_jobs(&self.ctx.workspace).await;
        let content = render_result(&jobs);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert("job_count".into(), AnalyticsValue::Int(jobs.len() as i64));
        bus.log_event(CRON_LIST_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "jobs": jobs,
                "content": content,
            }),
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx_in};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// Seed the single `<root>/.claude/scheduled_tasks.json` file with the given
    /// tasks (everything on disk is durable by definition — there is no on-disk
    /// `durable` field).
    async fn seed_tasks(root: &Path, tasks: Vec<cron::tasks_file::CronTask>) {
        let path = cron::tasks_file::scheduled_tasks_path(root);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        let doc = cron::tasks_file::ScheduledTasks { tasks };
        tokio::fs::write(&path, cron::tasks_file::serialize_tasks(&doc))
            .await
            .unwrap();
    }

    /// Build one recurring `CronTask` for seeding.
    fn task(id: &str, cron: &str, prompt: &str, recurring: bool) -> cron::tasks_file::CronTask {
        cron::tasks_file::CronTask {
            id: id.into(),
            cron: cron.into(),
            prompt: prompt.into(),
            created_at: 0,
            last_fired_at: None,
            recurring: Some(recurring),
            permanent: None,
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(CRON_LIST_TOOL_NAME, "CronList");
    }

    #[test]
    fn truncate_single_line_behaviour() {
        // Short prompt is returned verbatim.
        assert_eq!(truncate_single_line("echo hi", 80), "echo hi");
        // Over 80 chars gets cut to 79 chars + ellipsis (= 80 total).
        let long = "a".repeat(100);
        let out = truncate_single_line(&long, 80);
        assert_eq!(out.chars().count(), 80);
        assert!(out.ends_with('\u{2026}'));
        assert_eq!(out.chars().filter(|c| *c == 'a').count(), 79);
        // Newline forces single-line truncation with a trailing ellipsis.
        assert_eq!(truncate_single_line("line one\nline two", 80), "line one\u{2026}");
    }

    #[tokio::test]
    async fn empty_lists_no_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronListTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["jobs"], json!([]));
        assert_eq!(out.data["content"], json!("No scheduled jobs."));
    }

    #[tokio::test]
    async fn lists_created_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        // Insert out of order; the tool sorts by id. Both are durable (on disk).
        seed_tasks(
            tmp.path(),
            vec![
                task("dbbbb1111", "0 9 * * *", "morning", true),
                task("daaaa0000", "*/5 * * * *", "ticker", true),
            ],
        )
        .await;

        let tool = CronListTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let jobs = out.data["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 2);
        // Sorted by id.
        assert_eq!(jobs[0]["id"], json!("daaaa0000"));
        assert_eq!(jobs[1]["id"], json!("dbbbb1111"));
        // humanSchedule rendered.
        assert_eq!(jobs[0]["humanSchedule"], json!("Every 5 minutes"));
        assert_eq!(jobs[1]["humanSchedule"], json!("Every day at 9:00am"));
        // recurring present (true) on both. Every persisted task is durable, so
        // the `durable` key is never emitted.
        assert_eq!(jobs[0]["recurring"], json!(true));
        assert!(jobs[0].get("durable").is_none());
        assert_eq!(jobs[1]["recurring"], json!(true));
        assert!(jobs[1].get("durable").is_none());
    }

    #[tokio::test]
    async fn result_text_format() {
        let tmp = tempfile::tempdir().unwrap();
        // Recurring + one-shot, both durable (no [session-only] label on disk jobs).
        seed_tasks(
            tmp.path(),
            vec![
                task("daaaa0000", "*/5 * * * *", "ticker", true),
                task("dbbbb1111", "30 14 28 2 *", "remind me", false),
            ],
        )
        .await;

        let tool = CronListTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let content = out.data["content"].as_str().unwrap();
        let lines: Vec<&str> = content.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "daaaa0000 \u{2014} Every 5 minutes (recurring): ticker"
        );
        assert_eq!(
            lines[1],
            "dbbbb1111 \u{2014} February 28 at 2:30pm (one-shot): remind me"
        );
    }

    #[tokio::test]
    async fn result_text_truncates_long_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let long = "x".repeat(120);
        seed_tasks(tmp.path(), vec![task("daaaa0000", "*/5 * * * *", &long, true)]).await;

        let tool = CronListTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let content = out.data["content"].as_str().unwrap();
        // 79 'x' + ellipsis.
        assert!(content.ends_with(&format!("{}\u{2026}", "x".repeat(79))));
        assert!(!content.contains(&"x".repeat(80)));
    }
}
