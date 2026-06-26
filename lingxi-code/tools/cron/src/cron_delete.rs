//! `CronDeleteTool` — cancel a scheduled cron job by id, removing it from the
//! single project-relative `<root>/.lingxi/scheduled_tasks.json` file.
//!
//! 1:1 parity port of claude-code `CronDeleteTool.ts`. The model supplies the
//! job `id` returned by `CronCreate`; the tool read-modify-writes the tasks
//! file (dropping the matching task) and reports `Cancelled job <id>.`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::Verified;
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{CRON_DELETE_COMPLETED, CRON_DELETE_FAILED, CRON_DELETE_STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

use std::path::Path;

use crate::schedule_cron::cron_file_path;

/// Tool name byte-lock.
pub const CRON_DELETE_TOOL_NAME: &str = "CronDelete";

/// Model-facing description (CronDeleteTool.ts description()).
///
/// PARITY-GAP: TS `CRON_DELETE_DESCRIPTION` is the terse `'Cancel a scheduled
/// cron job by ID'`; the durability-aware `buildCronDeletePrompt` is the longer
/// "Cancel a cron job previously scheduled with CronCreate. …" form. This seam
/// has no durability feature gate, so we pin the description to the stable
/// "Cancel a cron job previously scheduled with CronCreate." wording.
const CRON_DELETE_DESCRIPTION: &str = "Cancel a cron job previously scheduled with CronCreate.";

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": {
                "type": "string",
                "description": "Job ID returned by CronCreate."
            }
        },
        "required": ["id"]
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
    bus.log_event(CRON_DELETE_FAILED, md).await;
}

/// Does a persisted task with `id` exist in the project's single tasks file?
async fn job_exists(project_root: &Path, id: &str) -> bool {
    let path = cron_file_path(project_root);
    match tokio::fs::read_to_string(&path).await {
        Ok(body) => cron::tasks_file::parse_tasks(&body)
            .tasks
            .iter()
            .any(|t| t.id == id),
        Err(_) => false,
    }
}

/// `CronDeleteTool` — cancel a scheduled cron job by id.
pub struct CronDeleteTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl CronDeleteTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &str {
        CRON_DELETE_TOOL_NAME
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
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        // CronDeleteTool.ts does not flag destructive; deleting a job descriptor
        // is treated as non-destructive (it is the user's own scheduled job).
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
                reason: "CronDelete removes a cron job from .lingxi/scheduled_tasks.json".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        CRON_DELETE_DESCRIPTION.into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "CronDelete: cancel a previously scheduled cron job by its id.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let id = input
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("CronDelete: missing or non-string id".into()))?;

        if !job_exists(&self.ctx.workspace, id).await {
            return Err(ValidationError(format!("No scheduled job with id '{id}'")));
        }
        // PARITY-GAP: TS validateInput also rejects deleting a cron owned by a
        // different teammate (`ctx && task.agentId !== ctx.agentId`). There is
        // no teammate context in this Rust seam, so the ownership check is
        // omitted.
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        let id = match input.get("id").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_id", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "CronDelete: missing or non-string id".into(),
                ));
            }
        };

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("id".into(), verified_str(&id));
        bus.log_event(CRON_DELETE_STARTED, md).await;

        // Read-modify-write the single `{ "tasks": [...] }` file: drop the task
        // with the matching id and write the rest back. A missing file / missing
        // id surfaces the byte-exact "No scheduled job with id '<id>'" error.
        let path = cron_file_path(&self.ctx.workspace);
        let body = match tokio::fs::read_to_string(&path).await {
            Ok(b) => b,
            Err(_) => {
                emit_failed(&bus, "not_found", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(format!(
                    "No scheduled job with id '{id}'"
                )));
            }
        };
        let mut doc = cron::tasks_file::parse_tasks(&body);
        let before = doc.tasks.len();
        doc.tasks.retain(|t| t.id != id);
        if doc.tasks.len() == before {
            emit_failed(&bus, "not_found", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput(format!(
                "No scheduled job with id '{id}'"
            )));
        }

        let updated = cron::tasks_file::serialize_tasks(&doc);
        if let Err(e) = tokio::fs::write(&path, updated.as_bytes()).await {
            emit_failed(&bus, "io_remove", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::Io(format!(
                "CronDelete: io error at {}: {e}",
                path.display()
            )));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        bus.log_event(CRON_DELETE_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "id": id,
                "content": format!("Cancelled job {id}."),
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

    /// Seed `<root>/.lingxi/scheduled_tasks.json` with the given task ids.
    async fn seed_ids(root: &Path, ids: &[&str]) {
        let path = cron_file_path(root);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        let doc = cron::tasks_file::ScheduledTasks {
            tasks: ids
                .iter()
                .map(|id| cron::tasks_file::CronTask {
                    id: (*id).into(),
                    cron: "*/5 * * * *".into(),
                    prompt: "echo hi".into(),
                    created_at: 0,
                    last_fired_at: None,
                    recurring: Some(true),
                    permanent: None,
                })
                .collect(),
        };
        tokio::fs::write(&path, cron::tasks_file::serialize_tasks(&doc))
            .await
            .unwrap();
    }

    async fn read_ids(root: &Path) -> Vec<String> {
        let path = cron_file_path(root);
        let body = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        cron::tasks_file::parse_tasks(&body)
            .tasks
            .into_iter()
            .map(|t| t.id)
            .collect()
    }

    #[test]
    fn constants_locked() {
        assert_eq!(CRON_DELETE_TOOL_NAME, "CronDelete");
    }

    #[tokio::test]
    async fn delete_existing_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        // Two jobs; deleting one leaves the other in the same file.
        seed_ids(tmp.path(), &["d12345678", "dkeep0000"]).await;

        let tool = CronDeleteTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(json!({"id": "d12345678"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["id"], json!("d12345678"));
        assert_eq!(out.data["content"], json!("Cancelled job d12345678."));

        // Only the targeted job is gone; the file (and the other job) remains.
        assert_eq!(read_ids(tmp.path()).await, vec!["dkeep0000".to_string()]);
    }

    #[tokio::test]
    async fn delete_missing_errors_with_exact_message() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronDeleteTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let err = tool
            .call(json!({"id": "dnope0000"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        match err {
            ToolError::InvalidInput(msg) => {
                assert_eq!(msg, "No scheduled job with id 'dnope0000'");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_input_rejects_missing_job() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronDeleteTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let err = tool
            .validate_input(&json!({"id": "dmissing1"}), &fresh_ctx())
            .await
            .expect_err("missing");
        assert_eq!(err.0, "No scheduled job with id 'dmissing1'");
    }
}
