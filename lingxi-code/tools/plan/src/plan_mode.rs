//! `EnterPlanModeTool` + `ExitPlanModeTool` — flip `SessionState.plan_mode`
//! and emit the byte-locked markers `[PLAN MODE]` / `[EXIT PLAN MODE]`
//! (spec §7 line 487).

use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use telemetry::pii::Verified;
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    ENTER_PLAN_MODE_COMPLETED, ENTER_PLAN_MODE_FAILED, ENTER_PLAN_MODE_STARTED,
    EXIT_PLAN_MODE_COMPLETED, EXIT_PLAN_MODE_FAILED, EXIT_PLAN_MODE_STARTED,
};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Byte-locked marker emitted when entering plan mode (spec §7 line 487).
pub const PLAN_MODE_ENTER_MARKER: &str = "[PLAN MODE]";
/// Byte-locked marker emitted when exiting plan mode (spec §7 line 487).
pub const PLAN_MODE_EXIT_MARKER: &str = "[EXIT PLAN MODE]";

/// Canonical tool name in the registry for `EnterPlanModeTool`.
pub const ENTER_TOOL_NAME: &str = "EnterPlanMode";
/// Canonical tool name in the registry for `ExitPlanModeTool`.
pub const EXIT_TOOL_NAME: &str = "ExitPlanMode";

static EMPTY_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
});

fn fresh_invocation_id() -> String {
    tool_api::util::ids::ulid_or_uuid()
}

fn verified(s: impl Into<String>) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.into()).into_inner())
}

/// `EnterPlanModeTool` — flips `session.plan_mode` from `false` → `true`.
/// Emits `[PLAN MODE]` in the result.
pub struct EnterPlanModeTool {
    ctx: BuiltinToolContext,
}

impl EnterPlanModeTool {
    /// Construct a new tool wired to the shared builtin context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    async fn emit_started(&self, invocation_id: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(ENTER_TOOL_NAME));
        self.ctx.bus.log_event(ENTER_PLAN_MODE_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(ENTER_TOOL_NAME));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(ENTER_PLAN_MODE_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, error_kind: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(ENTER_TOOL_NAME));
        md.insert("error_kind".into(), verified(error_kind));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(ENTER_PLAN_MODE_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str {
        ENTER_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &EMPTY_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
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
                reason: "EnterPlanMode toggles a session flag only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Enter plan mode".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "EnterPlanMode flips the session into plan mode and emits the literal `[PLAN MODE]`.".into()
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = fresh_invocation_id();
        let started_at = Instant::now();
        self.emit_started(&invocation_id).await;

        let session = ctx.session.as_ref().ok_or_else(|| {
            ToolError::Internal(
                "EnterPlanMode: session not wired into ToolUseContext (M4-04 contract)".into(),
            )
        })?;
        {
            let mut guard = session.lock().await;
            if guard.plan_mode {
                drop(guard);
                let dur = started_at.elapsed().as_millis() as u64;
                self.emit_failed(&invocation_id, "already_in_plan_mode", dur)
                    .await;
                return Err(ToolError::InvalidInput(
                    "EnterPlanMode: session is already in plan mode".into(),
                ));
            }
            guard.plan_mode = true;
        }
        let duration_ms = started_at.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, duration_ms).await;
        Ok(ToolCallResult {
            data: json!({
                "marker": PLAN_MODE_ENTER_MARKER,
                "plan_mode": true,
            }),
            new_messages: Vec::new(),
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

/// `ExitPlanModeTool` — flips `session.plan_mode` from `true` → `false`.
/// Emits `[EXIT PLAN MODE]` in the result.
pub struct ExitPlanModeTool {
    ctx: BuiltinToolContext,
}

impl ExitPlanModeTool {
    /// Construct a new tool wired to the shared builtin context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    async fn emit_started(&self, invocation_id: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(EXIT_TOOL_NAME));
        self.ctx.bus.log_event(EXIT_PLAN_MODE_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(EXIT_TOOL_NAME));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(EXIT_PLAN_MODE_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, error_kind: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(EXIT_TOOL_NAME));
        md.insert("error_kind".into(), verified(error_kind));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(EXIT_PLAN_MODE_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        EXIT_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &EMPTY_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
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
                reason: "ExitPlanMode toggles a session flag only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Exit plan mode".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "ExitPlanMode flips the session out of plan mode and emits the literal `[EXIT PLAN MODE]`."
            .into()
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = fresh_invocation_id();
        let started_at = Instant::now();
        self.emit_started(&invocation_id).await;

        let session = ctx.session.as_ref().ok_or_else(|| {
            ToolError::Internal(
                "ExitPlanMode: session not wired into ToolUseContext (M4-04 contract)".into(),
            )
        })?;
        {
            let mut guard = session.lock().await;
            if !guard.plan_mode {
                drop(guard);
                let dur = started_at.elapsed().as_millis() as u64;
                self.emit_failed(&invocation_id, "not_in_plan_mode", dur)
                    .await;
                return Err(ToolError::InvalidInput(
                    "ExitPlanMode: session is not in plan mode".into(),
                ));
            }
            guard.plan_mode = false;
        }
        let duration_ms = started_at.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, duration_ms).await;
        Ok(ToolCallResult {
            data: json!({
                "marker": PLAN_MODE_EXIT_MARKER,
                "plan_mode": false,
            }),
            new_messages: Vec::new(),
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::SessionState;
    use protocol::SessionId;
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};
    use tokio::sync::Mutex;
    use tool_api::test_support::{ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs};

    fn make_ctx() -> (
        BuiltinToolContext,
        Arc<InMemorySink>,
        Arc<Mutex<SessionState>>,
        tool_api::context::ToolUseContext,
    ) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        let bctx = ctx_for_file_tools(make_dummy_fs(), bus.clone(), vec![std::env::temp_dir()]);
        let session = Arc::new(Mutex::new(SessionState::empty(
            SessionId::nil(),
            "claude-opus-4-7".into(),
        )));
        let mut use_ctx = fresh_ctx();
        use_ctx.session = Some(session.clone());
        (bctx, sink, session, use_ctx)
    }

    #[test]
    fn enter_marker_matches_spec() {
        assert_eq!(PLAN_MODE_ENTER_MARKER, "[PLAN MODE]");
    }

    #[test]
    fn exit_marker_matches_spec() {
        assert_eq!(PLAN_MODE_EXIT_MARKER, "[EXIT PLAN MODE]");
    }

    #[test]
    fn enter_and_exit_markers_differ() {
        assert_ne!(PLAN_MODE_ENTER_MARKER, PLAN_MODE_EXIT_MARKER);
    }

    #[tokio::test]
    async fn enter_flips_flag_and_returns_marker() {
        let (bctx, sink, session, use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = EnterPlanModeTool::new(bctx);
        assert!(!session.lock().await.plan_mode);
        let res = tool
            .call(json!({}), use_ctx, fresh_tx())
            .await
            .expect("enter must succeed on fresh session");
        assert_eq!(res.data["marker"], "[PLAN MODE]");
        assert_eq!(res.data["plan_mode"], true);
        assert!(session.lock().await.plan_mode);
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_PLAN_MODE_STARTED.to_string()));
        assert!(names.contains(&ENTER_PLAN_MODE_COMPLETED.to_string()));
    }

    #[tokio::test]
    async fn enter_twice_rejects_with_locked_string() {
        let (bctx, sink, _session, use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = EnterPlanModeTool::new(bctx);
        tool.call(json!({}), use_ctx.clone(), fresh_tx())
            .await
            .expect("first enter must succeed");
        let err = tool
            .call(json!({}), use_ctx, fresh_tx())
            .await
            .expect_err("second enter must fail");
        assert_eq!(
            format!("{err}"),
            "invalid input: EnterPlanMode: session is already in plan mode"
        );
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_PLAN_MODE_FAILED.to_string()));
    }

    #[tokio::test]
    async fn exit_flips_flag_and_returns_marker() {
        let (bctx, sink, session, use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        session.lock().await.plan_mode = true; // pre-arm
        let tool = ExitPlanModeTool::new(bctx);
        let res = tool
            .call(json!({}), use_ctx, fresh_tx())
            .await
            .expect("exit must succeed when in plan mode");
        assert_eq!(res.data["marker"], "[EXIT PLAN MODE]");
        assert_eq!(res.data["plan_mode"], false);
        assert!(!session.lock().await.plan_mode);
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&EXIT_PLAN_MODE_COMPLETED.to_string()));
    }

    #[tokio::test]
    async fn exit_without_enter_rejects_with_locked_string() {
        let (bctx, sink, _session, use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = ExitPlanModeTool::new(bctx);
        let err = tool
            .call(json!({}), use_ctx, fresh_tx())
            .await
            .expect_err("exit on fresh session must fail");
        assert_eq!(
            format!("{err}"),
            "invalid input: ExitPlanMode: session is not in plan mode"
        );
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&EXIT_PLAN_MODE_FAILED.to_string()));
    }
}
