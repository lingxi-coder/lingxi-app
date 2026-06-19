//! `CronListTool` — list every scheduled cron job by reading the persisted
//! `~/.claude/cron/*.json` descriptors.
//!
//! 1:1 parity port of claude-code `CronListTool.ts`. Returns a `jobs` array of
//! `{id, cron, humanSchedule, prompt, recurring?, durable?}` plus a flattened
//! result text for the model, one line per job.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Map, Value};
use telemetry::pii::Verified;
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{CRON_LIST_COMPLETED, CRON_LIST_FAILED, CRON_LIST_STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

use crate::schedule_cron::{config_home_dir, cron_to_human, home_dir_or_internal, CRON_SUBDIR};

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

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(CRON_LIST_FAILED, md).await;
}

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

/// Read every `~/.claude/cron/*.json` descriptor into the `jobs` shape, sorted
/// by id for determinism. Unreadable / unparseable files are skipped.
async fn read_all_jobs(home: &Path) -> Vec<Value> {
    let dir = config_home_dir(home).join(CRON_SUBDIR);
    let mut jobs: Vec<Value> = Vec::new();
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(_) => return jobs, // dir absent → no jobs
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let body = match tokio::fs::read_to_string(&path).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let desc: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = desc
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let cron = desc
            .get("cron")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let prompt = desc
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let recurring = desc.get("recurring").and_then(Value::as_bool);
        let durable = desc.get("durable").and_then(Value::as_bool);

        let mut obj = Map::new();
        obj.insert("id".into(), json!(id));
        obj.insert("cron".into(), json!(cron));
        obj.insert("humanSchedule".into(), json!(cron_to_human(&cron)));
        obj.insert("prompt".into(), json!(prompt));
        // Conditional spread (CronListTool.ts): `recurring` only when truthy,
        // `durable` only when it is explicitly false.
        if recurring == Some(true) {
            obj.insert("recurring".into(), json!(true));
        }
        if durable == Some(false) {
            obj.insert("durable".into(), json!(false));
        }
        jobs.push(Value::Object(obj));
    }
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
                reason: "CronList reads cron descriptors under ~/.claude/cron/".into(),
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

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_failed(&bus, "no_home", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };

        // PARITY-GAP: TS filters to the calling teammate's own crons
        // (`ctx ? allTasks.filter(t => t.agentId === ctx.agentId) : allTasks`).
        // There is no teammate context in this Rust seam, so every persisted
        // job is listed.
        let jobs = read_all_jobs(&home).await;
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
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule_cron::cron_path;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx, HOME_LOCK};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    async fn write_job(
        home: &Path,
        id: &str,
        cron: &str,
        prompt: &str,
        recurring: bool,
        durable: bool,
    ) {
        let path = cron_path(home, id);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        let descriptor = json!({
            "id": id,
            "cron": cron,
            "prompt": prompt,
            "recurring": recurring,
            "durable": durable,
            "created_at_unix_secs": 0,
            "next_fire_unix_secs": 60,
        });
        tokio::fs::write(&path, serde_json::to_vec_pretty(&descriptor).unwrap())
            .await
            .unwrap();
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
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());

        let tool = CronListTool::new(shell_test_ctx(dummy_out()));
        let out = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["jobs"], json!([]));
        assert_eq!(out.data["content"], json!("No scheduled jobs."));
    }

    #[tokio::test]
    async fn lists_created_jobs() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        // Insert out of order; the tool sorts by id.
        write_job(tmp.path(), "dbbbb1111", "0 9 * * *", "morning", true, true).await;
        write_job(tmp.path(), "daaaa0000", "*/5 * * * *", "ticker", true, false).await;

        let tool = CronListTool::new(shell_test_ctx(dummy_out()));
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
        // Conditional keys: recurring present (true) on both; durable:false key
        // only on the session-only job, absent on the durable one.
        assert_eq!(jobs[0]["recurring"], json!(true));
        assert_eq!(jobs[0]["durable"], json!(false));
        assert_eq!(jobs[1]["recurring"], json!(true));
        assert!(jobs[1].get("durable").is_none());
    }

    #[tokio::test]
    async fn result_text_format() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        // Recurring + session-only.
        write_job(tmp.path(), "daaaa0000", "*/5 * * * *", "ticker", true, false).await;
        // One-shot + durable (durable=true → no [session-only], no durable key).
        write_job(tmp.path(), "dbbbb1111", "30 14 28 2 *", "remind me", false, true).await;

        let tool = CronListTool::new(shell_test_ctx(dummy_out()));
        let out = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let content = out.data["content"].as_str().unwrap();
        let lines: Vec<&str> = content.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "daaaa0000 \u{2014} Every 5 minutes (recurring) [session-only]: ticker"
        );
        assert_eq!(
            lines[1],
            "dbbbb1111 \u{2014} February 28 at 2:30pm (one-shot): remind me"
        );
    }

    #[tokio::test]
    async fn result_text_truncates_long_prompt() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let long = "x".repeat(120);
        write_job(tmp.path(), "daaaa0000", "*/5 * * * *", &long, true, false).await;

        let tool = CronListTool::new(shell_test_ctx(dummy_out()));
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
