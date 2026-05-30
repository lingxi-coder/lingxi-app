//! `RemoteTriggerTool` — local stub for remote workflow triggers.
//!
//! Wire identifiers locked in spec §7:
//! - Legacy credentials path `~/.claude/.credentials.json`.
//! - Local stub — **NO network**.
//!
//! Reads `~/.claude/.credentials.json`, validates it parses as JSON with at
//! least an `oauth.access_token` string, and returns
//! `{ stub: true, credentials_path, would_trigger: <input> }`. Emits
//! `remote_trigger_started` and `remote_trigger_completed`. Production
//! hosts wrap this with the real HTTP trigger (out of M4-08 scope).

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
use telemetry::tengu::tool::{
    REMOTE_TRIGGER_COMPLETED, REMOTE_TRIGGER_FAILED, REMOTE_TRIGGER_STARTED,
};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock.
pub const REMOTE_TRIGGER_TOOL_NAME: &str = "RemoteTrigger";
/// Legacy credentials file (spec §7).
pub const REMOTE_TRIGGER_CREDENTIALS_FILE: &str = ".credentials.json";
/// `~/.claude/` subdirectory housing the credentials file.
pub const REMOTE_TRIGGER_SUBDIR: &str = ".claude";

fn home_dir_or_internal() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Internal("RemoteTrigger: HOME directory not available".into()))
}

#[must_use]
pub(crate) fn credentials_path(home: &Path) -> PathBuf {
    home.join(REMOTE_TRIGGER_SUBDIR)
        .join(REMOTE_TRIGGER_CREDENTIALS_FILE)
}

/// `RemoteTriggerTool` — local stub. Validates credentials file exists +
/// has `oauth.access_token` string; never hits the network.
pub struct RemoteTriggerTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl RemoteTriggerTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "trigger_name": { "type": "string", "minLength": 1 },
            "payload":      {}
        },
        "required": ["trigger_name"]
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
    bus.log_event(REMOTE_TRIGGER_FAILED, md).await;
}

#[async_trait]
impl Tool for RemoteTriggerTool {
    fn name(&self) -> &str {
        REMOTE_TRIGGER_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4_096
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
        false // local stub — explicitly NOT open-world.
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "RemoteTrigger is a local stub — no network access".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Local stub for remote workflow triggers. Validates ~/.claude/.credentials.json; no network."
            .into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "RemoteTrigger: stub for triggering a remote workflow (M4-08 ships local-only).".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let name = input
            .get("trigger_name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ValidationError("RemoteTrigger: missing or non-string trigger_name".into())
            })?;
        if name.is_empty() {
            return Err(ValidationError(
                "RemoteTrigger: trigger_name is empty".into(),
            ));
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

        let trigger_name = match input.get("trigger_name").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    "missing_trigger_name",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "RemoteTrigger: missing or non-string trigger_name".into(),
                ));
            }
        };
        let payload = input.get("payload").cloned().unwrap_or(Value::Null);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("_PROTO_trigger_name".into(), pii_str(&trigger_name));
        bus.log_event(REMOTE_TRIGGER_STARTED, md).await;

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_failed(&bus, "no_home", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let path = credentials_path(&home);

        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            emit_failed(
                &bus,
                "credentials_missing",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "RemoteTrigger: credentials file not found at {}",
                path.display()
            )));
        }
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                emit_failed(&bus, "io_read", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Io(format!(
                    "RemoteTrigger: io error at {}: {e}",
                    path.display()
                )));
            }
        };
        let creds: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                emit_failed(&bus, "invalid_json", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Io(format!(
                    "RemoteTrigger: credentials file at {} is invalid JSON: {e}",
                    path.display()
                )));
            }
        };
        let has_access_token = creds
            .get("oauth")
            .and_then(|o| o.get("access_token"))
            .and_then(Value::as_str)
            .is_some();
        if !has_access_token {
            emit_failed(
                &bus,
                "missing_access_token",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "RemoteTrigger: credentials file at {} is missing oauth.access_token",
                path.display()
            )));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert(
            "credentials_bytes".into(),
            AnalyticsValue::Int(bytes.len() as i64),
        );
        bus.log_event(REMOTE_TRIGGER_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "stub": true,
                "credentials_path": path.display().to_string(),
                "trigger_name": trigger_name,
                "would_trigger": {
                    "trigger_name": trigger_name,
                    "payload": payload,
                },
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
        assert_eq!(REMOTE_TRIGGER_TOOL_NAME, "RemoteTrigger");
        assert_eq!(REMOTE_TRIGGER_CREDENTIALS_FILE, ".credentials.json");
        assert_eq!(REMOTE_TRIGGER_SUBDIR, ".claude");
    }

    #[tokio::test]
    async fn happy_path_with_valid_credentials() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let path = credentials_path(tmp.path());
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &path,
            br#"{"oauth": {"access_token": "tok-abc", "refresh_token": "ref-xyz"}}"#,
        )
        .await
        .unwrap();
        let tool = RemoteTriggerTool::new(shell_test_ctx(dummy_out()));
        let out = tool
            .call(
                json!({"trigger_name": "deploy", "payload": {"env": "prod"}}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["stub"], json!(true));
        assert_eq!(out.data["trigger_name"], json!("deploy"));
        assert_eq!(out.data["would_trigger"]["payload"]["env"], json!("prod"));
    }

    #[tokio::test]
    async fn rejects_missing_credentials_file() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = RemoteTriggerTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"trigger_name": "deploy"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing creds");
        assert!(format!("{err}").contains("credentials file not found"));
    }

    #[tokio::test]
    async fn rejects_credentials_without_access_token() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let path = credentials_path(tmp.path());
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, br#"{"oauth": {"refresh_token": "ref-xyz"}}"#)
            .await
            .unwrap();
        let tool = RemoteTriggerTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"trigger_name": "deploy"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("no access_token");
        assert!(format!("{err}").contains("missing oauth.access_token"));
    }

    #[tokio::test]
    async fn rejects_missing_trigger_name() {
        let tool = RemoteTriggerTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string trigger_name"));
    }
}
