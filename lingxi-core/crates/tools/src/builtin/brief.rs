//! `BriefTool` — writes a brief markdown to `~/.claude/brief/<task_id>.txt`.
//!
//! Wire identifiers locked in spec §7:
//! - Brief subdir literal `brief`.
//! - File name uses M1 task-id format `[b][0-9a-z]{8}` (NOT uuid v4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{BRIEF_COMPLETED, BRIEF_FAILED, BRIEF_STARTED};
use lingxi_telemetry::AnalyticsBus;
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock.
pub const BRIEF_TOOL_NAME: &str = "Brief";
/// Subdirectory under `~/.claude/`.
pub const BRIEF_SUBDIR: &str = "brief";
/// File extension.
pub const BRIEF_FILE_SUFFIX: &str = ".txt";
/// 9-char task-id prefix character (`b` for brief).
pub const BRIEF_TASK_ID_PREFIX: char = 'b';
/// Maximum body length (sane upper bound; 1 MB matches BashTool output cap).
pub const MAX_BRIEF_BODY_LEN: usize = 1_048_576;

/// Generate a 9-char `[b][0-9a-z]{8}` task id matching M1 `TaskId` format.
#[must_use]
pub(crate) fn generate_brief_task_id() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let mut s = String::with_capacity(9);
    s.push(BRIEF_TASK_ID_PREFIX);
    for _ in 0..8 {
        let idx = rng.random_range(0..ALPHABET.len());
        s.push(ALPHABET[idx] as char);
    }
    s
}

/// Resolve `<home>/.claude/brief/<task_id>.txt`.
#[must_use]
pub(crate) fn brief_path(home: &Path, task_id: &str) -> PathBuf {
    home.join(".claude")
        .join(BRIEF_SUBDIR)
        .join(format!("{task_id}{BRIEF_FILE_SUFFIX}"))
}

pub(crate) fn home_dir_or_internal() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Internal("Brief: HOME directory not available".into()))
}

/// `BriefTool` — writes a brief markdown to `~/.claude/brief/<task_id>.txt`.
pub struct BriefTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

impl BriefTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "body": { "type": "string", "minLength": 1 }
        },
        "required": ["body"]
    })
});

fn pii_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}

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
    bus.log_event(BRIEF_FAILED, md).await;
}

#[async_trait]
impl Tool for BriefTool {
    fn name(&self) -> &str {
        BRIEF_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
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
                reason: "Brief writes to ~/.claude/brief/ (local stub)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Write a brief markdown to ~/.claude/brief/<task_id>.txt.".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Brief: write a brief to a local file under ~/.claude/brief/.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let body = input
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("Brief: missing or non-string body".into()))?;
        if body.is_empty() {
            return Err(ValidationError("Brief: body is empty".into()));
        }
        if body.len() > MAX_BRIEF_BODY_LEN {
            return Err(ValidationError(format!(
                "Brief: body length {} exceeds max {}",
                body.len(),
                MAX_BRIEF_BODY_LEN
            )));
        }
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

        let body = match input.get("body").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_body", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "Brief: missing or non-string body".into(),
                ));
            }
        };
        if body.is_empty() {
            emit_failed(&bus, "empty_body", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput("Brief: body is empty".into()));
        }
        if body.len() > MAX_BRIEF_BODY_LEN {
            emit_failed(&bus, "body_too_large", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput(format!(
                "Brief: body length {} exceeds max {MAX_BRIEF_BODY_LEN}",
                body.len()
            )));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "body_len".into(),
            AnalyticsValue::Int(body.len() as i64),
        );
        md.insert("_PROTO_body_preview".into(), pii_str(&body[..body.len().min(80)]));
        bus.log_event(BRIEF_STARTED, md).await;

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_failed(&bus, "no_home", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let task_id = generate_brief_task_id();
        let path = brief_path(&home, &task_id);

        if let Some(dir) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                emit_failed(&bus, "io_create_dir", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Io(format!(
                    "Brief: io error at {}: {e}",
                    dir.display()
                )));
            }
        }
        if let Err(e) = tokio::fs::write(&path, body.as_bytes()).await {
            emit_failed(&bus, "io_write", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::Io(format!(
                "Brief: io error at {}: {e}",
                path.display()
            )));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert(
            "bytes_written".into(),
            AnalyticsValue::Int(body.len() as i64),
        );
        md.insert("_PROTO_path".into(), pii_str(&path.display().to_string()));
        bus.log_event(BRIEF_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "task_id": task_id,
                "path": path.display().to_string(),
                "bytes_written": body.len(),
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
    use crate::builtin::test_support::{fresh_ctx, fresh_tx, shell_test_ctx, HOME_LOCK};
    use lingxi_traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(BRIEF_TOOL_NAME, "Brief");
        assert_eq!(BRIEF_SUBDIR, "brief");
        assert_eq!(BRIEF_FILE_SUFFIX, ".txt");
        assert_eq!(BRIEF_TASK_ID_PREFIX, 'b');
    }

    #[test]
    fn task_id_is_9_chars_with_b_prefix() {
        let id = generate_brief_task_id();
        assert_eq!(id.len(), 9, "got {id:?}");
        assert_eq!(id.chars().next().unwrap(), 'b');
        assert!(id
            .chars()
            .skip(1)
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    #[test]
    fn brief_path_layout() {
        let p = brief_path(Path::new("/tmp/h"), "babcd1234");
        assert_eq!(p, PathBuf::from("/tmp/h/.claude/brief/babcd1234.txt"));
    }

    #[tokio::test]
    async fn writes_brief_under_home_dot_claude_brief() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        let body = "# Brief\n\nHello world.";
        let out = tool
            .call(json!({"body": body}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let path = out.data["path"].as_str().unwrap();
        assert!(
            path.starts_with(&format!("{}/.claude/brief/b", tmp.path().display())),
            "{path}"
        );
        assert!(path.ends_with(".txt"));
        let written = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(written, body);
    }

    #[tokio::test]
    async fn rejects_missing_body() {
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string body"));
    }

    #[tokio::test]
    async fn rejects_empty_body() {
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"body": ""}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("empty");
        assert!(format!("{err}").contains("body is empty"));
    }
}
