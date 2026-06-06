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
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{BRIEF_COMPLETED, BRIEF_FAILED, BRIEF_STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock. The model addresses this tool by its wire name
/// `SendUserMessage` (claude-code `BriefTool/prompt.ts` `BRIEF_TOOL_NAME`).
pub const BRIEF_TOOL_NAME: &str = "SendUserMessage";
/// Legacy wire name still accepted as an alias (claude-code
/// `LEGACY_BRIEF_TOOL_NAME = "Brief"`).
pub const LEGACY_BRIEF_TOOL_NAME: &str = "Brief";
/// Subdirectory under `~/.claude/`.
pub const BRIEF_SUBDIR: &str = "brief";
/// File extension.
pub const BRIEF_FILE_SUFFIX: &str = ".txt";
/// 9-char task-id prefix character (`b` for brief).
pub const BRIEF_TASK_ID_PREFIX: char = 'b';
/// Maximum message length (sane upper bound; 1 MB matches BashTool output cap).
/// Rust-side file-write guard only — the TS `message: z.string()` has no max.
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
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl BriefTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    // TS `z.strictObject({ message, attachments?, status })`
    // (`BriefTool.ts:20-39`). Byte-faithful field descriptions.
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "message": {
                "type": "string",
                "description": "The message for the user. Supports markdown formatting."
            },
            "attachments": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Optional file paths (absolute or relative to cwd) to attach. Use for photos, screenshots, diffs, logs, or any file the user should see alongside your message."
            },
            "status": {
                "type": "string",
                "enum": ["normal", "proactive"],
                "description": "Use 'proactive' when you're surfacing something the user hasn't asked for and needs to see now — task completion while they're away, a blocker you hit, an unsolicited status update. Use 'normal' when replying to something the user just said."
            }
        },
        "required": ["message", "status"]
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
    fn aliases(&self) -> &[&str] {
        const ALIASES: &[&str] = &[LEGACY_BRIEF_TOOL_NAME];
        ALIASES
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
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
        // TS schema: `message: z.string()` (no minLength). Require a string but
        // accept empty. `MAX_BRIEF_BODY_LEN` is a Rust-side file-write guard.
        let message = input
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("Brief: missing or non-string message".into()))?;
        if message.len() > MAX_BRIEF_BODY_LEN {
            return Err(ValidationError(format!(
                "Brief: message length {} exceeds max {}",
                message.len(),
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

        let message = match input.get("message").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    "missing_message",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "Brief: missing or non-string message".into(),
                ));
            }
        };
        if message.len() > MAX_BRIEF_BODY_LEN {
            emit_failed(
                &bus,
                "message_too_large",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "Brief: message length {} exceeds max {MAX_BRIEF_BODY_LEN}",
                message.len()
            )));
        }

        let preview: String = message.chars().take(80).collect();
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "message_len".into(),
            AnalyticsValue::Int(message.len() as i64),
        );
        md.insert("_PROTO_message_preview".into(), pii_str(&preview));
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
        if let Err(e) = tokio::fs::write(&path, message.as_bytes()).await {
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
            AnalyticsValue::Int(message.len() as i64),
        );
        md.insert("_PROTO_path".into(), pii_str(&path.display().to_string()));
        bus.log_event(BRIEF_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "task_id": task_id,
                "path": path.display().to_string(),
                "bytes_written": message.len(),
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

    #[test]
    fn constants_locked() {
        assert_eq!(BRIEF_TOOL_NAME, "SendUserMessage");
        assert_eq!(LEGACY_BRIEF_TOOL_NAME, "Brief");
        assert_eq!(BRIEF_SUBDIR, "brief");
        assert_eq!(BRIEF_FILE_SUFFIX, ".txt");
        assert_eq!(BRIEF_TASK_ID_PREFIX, 'b');
    }

    #[test]
    fn advertises_send_user_message_name_with_brief_alias() {
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        assert_eq!(tool.name(), "SendUserMessage");
        assert_eq!(tool.aliases(), &["Brief"]);
        // The legacy wire name resolves through the alias list.
        assert!(tool.aliases().contains(&"Brief"));
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

    #[test]
    fn schema_is_strict_object_with_message_and_status() {
        // TS `z.strictObject({ message, attachments?, status })`.
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        let schema = tool.input_schema();
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"], json!(["message", "status"]));
        assert_eq!(schema["properties"]["message"]["type"], json!("string"));
        assert_eq!(
            schema["properties"]["status"]["enum"],
            json!(["normal", "proactive"])
        );
        assert_eq!(
            schema["properties"]["attachments"]["items"]["type"],
            json!("string")
        );
        // minLength dropped from the message field.
        assert!(schema["properties"]["message"].get("minLength").is_none());
    }

    #[tokio::test]
    async fn writes_brief_under_home_dot_claude_brief() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        let message = "# Brief\n\nHello world.";
        let out = tool
            .call(
                json!({"message": message, "status": "normal"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        let path = out.data["path"].as_str().unwrap();
        assert!(
            path.starts_with(&format!("{}/.claude/brief/b", tmp.path().display())),
            "{path}"
        );
        assert!(path.to_lowercase().ends_with(".txt"));
        let written = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(written, message);
    }

    #[tokio::test]
    async fn rejects_missing_message() {
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"status": "normal"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string message"));
    }

    #[tokio::test]
    async fn accepts_empty_message() {
        // minLength dropped — an empty message now validates (TS `z.string()`).
        let tool = BriefTool::new(shell_test_ctx(dummy_out()));
        tool.validate_input(&json!({"message": "", "status": "normal"}), &fresh_ctx())
            .await
            .expect("empty message validates");
    }
}
