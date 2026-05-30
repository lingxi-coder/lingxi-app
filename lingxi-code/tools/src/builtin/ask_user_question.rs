//! `AskUserQuestionTool` — prompts the user with up to 4 labeled options.
//!
//! Wire identifiers locked in spec §7:
//! - Max 4 options.
//! - Each label ≤ 60 chars.
//! - Question ≤ 200 chars.
//! - Returns `{ selected_index, selected_label }`.
//!
//! Hermetic by default: the question is not actually presented; the resolver
//! `ctx.options.ask_user_question_selected_index` (advisory) returns 0 when
//! absent. Production hosts override `AskUserQuestionTool` constructor input
//! to inject a real terminal/UI prompt.
//!
//! no-truncation: returns `{ selected_index: u32, selected_label: String }`.
//! Selected label is bounded to ≤60 chars (input contract).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{
    ASK_USER_QUESTION_COMPLETED, ASK_USER_QUESTION_FAILED, ASK_USER_QUESTION_STARTED,
};
use lingxi_telemetry::AnalyticsBus;
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

// -- Wire identifier locks (spec §7) -----------------------------------------

/// Tool name byte-lock.
pub const ASK_USER_QUESTION_TOOL_NAME: &str = "AskUserQuestion";
/// Maximum number of options (spec §7).
pub const MAX_ASK_OPTIONS: usize = 4;
/// Maximum label length per option (spec §7).
pub const MAX_ASK_LABEL_LEN: usize = 60;
/// Maximum question length (spec §7).
pub const MAX_ASK_QUESTION_LEN: usize = 200;

/// Resolver trait — production wraps a terminal/UI prompt; the M4-08 default
/// is a "first option" stub so dispatcher integration stays hermetic.
#[async_trait]
pub trait AskUserQuestionResolver: Send + Sync {
    /// Return the selected option index, given the question and options list.
    ///
    /// # Errors
    /// Implementations may surface `ToolError` if the prompt fails.
    async fn resolve(&self, question: &str, options: &[String]) -> Result<usize, ToolError>;
}

/// Default hermetic resolver — always returns index 0 (first option).
pub struct FirstOptionResolver;

#[async_trait]
impl AskUserQuestionResolver for FirstOptionResolver {
    async fn resolve(&self, _question: &str, _options: &[String]) -> Result<usize, ToolError> {
        Ok(0)
    }
}

/// `AskUserQuestionTool` — prompts the user with up to 4 labeled options.
pub struct AskUserQuestionTool {
    pub(crate) ctx: super::BuiltinToolContext,
    pub(crate) resolver: Arc<dyn AskUserQuestionResolver>,
}

impl AskUserQuestionTool {
    /// Construct with the default `FirstOptionResolver`.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self {
            ctx,
            resolver: Arc::new(FirstOptionResolver),
        }
    }

    /// Construct with a caller-supplied resolver (production use).
    #[must_use]
    pub fn with_resolver(
        ctx: super::BuiltinToolContext,
        resolver: Arc<dyn AskUserQuestionResolver>,
    ) -> Self {
        Self { ctx, resolver }
    }
}

pub(crate) fn validate_question(q: &str) -> Result<(), ToolError> {
    if q.is_empty() {
        return Err(ToolError::InvalidInput(
            "AskUserQuestion: question is empty".into(),
        ));
    }
    if q.chars().count() > MAX_ASK_QUESTION_LEN {
        return Err(ToolError::InvalidInput(format!(
            "AskUserQuestion: question length {} exceeds max {}",
            q.chars().count(),
            MAX_ASK_QUESTION_LEN
        )));
    }
    Ok(())
}

pub(crate) fn validate_options(opts: &[String]) -> Result<(), ToolError> {
    if opts.is_empty() {
        return Err(ToolError::InvalidInput(
            "AskUserQuestion: at least one option is required".into(),
        ));
    }
    if opts.len() > MAX_ASK_OPTIONS {
        return Err(ToolError::InvalidInput(format!(
            "AskUserQuestion: option count {} exceeds max {}",
            opts.len(),
            MAX_ASK_OPTIONS
        )));
    }
    for (i, opt) in opts.iter().enumerate() {
        if opt.is_empty() {
            return Err(ToolError::InvalidInput(format!(
                "AskUserQuestion: option[{i}] is empty"
            )));
        }
        if opt.chars().count() > MAX_ASK_LABEL_LEN {
            return Err(ToolError::InvalidInput(format!(
                "AskUserQuestion: option[{i}] length {} exceeds max {}",
                opt.chars().count(),
                MAX_ASK_LABEL_LEN
            )));
        }
    }
    Ok(())
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "question": { "type": "string", "minLength": 1, "maxLength": 200 },
            "options": {
                "type": "array",
                "minItems": 1,
                "maxItems": 4,
                "items": { "type": "string", "minLength": 1, "maxLength": 60 }
            }
        },
        "required": ["question", "options"]
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
    bus.log_event(ASK_USER_QUESTION_FAILED, md).await;
}

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        ASK_USER_QUESTION_TOOL_NAME
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
        false
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
    fn requires_user_interaction(&self) -> bool {
        true
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "AskUserQuestion is a user-prompt UI action (always allowed)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Ask the user a multiple-choice question (max 4 options, 60-char labels).".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "AskUserQuestion: prompts the user with up to 4 labeled options.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let q = input
            .get("question")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ValidationError("AskUserQuestion: missing or non-string question".into())
            })?;
        validate_question(q).map_err(|e| ValidationError(format!("{e}")))?;
        let opts_v = input
            .get("options")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ValidationError("AskUserQuestion: missing or non-array options".into())
            })?;
        let opts: Vec<String> = opts_v
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_string())
            .collect();
        validate_options(&opts).map_err(|e| ValidationError(format!("{e}")))?;
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

        let q = match input.get("question").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    "missing_question",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "AskUserQuestion: missing or non-string question".into(),
                ));
            }
        };
        if let Err(e) = validate_question(&q) {
            emit_failed(
                &bus,
                "invalid_question",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(e);
        }

        let opts_v = match input.get("options").and_then(Value::as_array) {
            Some(a) => a.clone(),
            None => {
                emit_failed(
                    &bus,
                    "missing_options",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "AskUserQuestion: missing or non-array options".into(),
                ));
            }
        };
        let opts: Vec<String> = opts_v
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_string())
            .collect();
        if let Err(e) = validate_options(&opts) {
            emit_failed(
                &bus,
                "invalid_options",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(e);
        }

        // Started event.
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "question_len".into(),
            AnalyticsValue::Int(q.chars().count() as i64),
        );
        md.insert(
            "option_count".into(),
            AnalyticsValue::Int(opts.len() as i64),
        );
        md.insert("_PROTO_question".into(), pii_str(&q));
        bus.log_event(ASK_USER_QUESTION_STARTED, md).await;

        let idx = match self.resolver.resolve(&q, &opts).await {
            Ok(i) => i,
            Err(e) => {
                emit_failed(&bus, "resolver_error", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };

        if idx >= opts.len() {
            emit_failed(
                &bus,
                "resolver_index_out_of_range",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "AskUserQuestion: resolver returned index {idx} but only {} options were provided",
                opts.len()
            )));
        }

        let label = opts[idx].clone();
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert("selected_index".into(), AnalyticsValue::Int(idx as i64));
        bus.log_event(ASK_USER_QUESTION_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({ "selected_index": idx, "selected_label": label }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
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
        assert_eq!(ASK_USER_QUESTION_TOOL_NAME, "AskUserQuestion");
        assert_eq!(MAX_ASK_OPTIONS, 4);
        assert_eq!(MAX_ASK_LABEL_LEN, 60);
        assert_eq!(MAX_ASK_QUESTION_LEN, 200);
    }

    #[test]
    fn validate_question_at_limit_ok() {
        let q = "x".repeat(MAX_ASK_QUESTION_LEN);
        validate_question(&q).expect("ok at limit");
    }

    #[test]
    fn validate_question_over_limit_rejects() {
        let q = "x".repeat(MAX_ASK_QUESTION_LEN + 1);
        let err = validate_question(&q).unwrap_err();
        assert!(format!("{err}").contains("exceeds max 200"));
    }

    #[test]
    fn validate_options_count_capped_at_4() {
        let opts: Vec<String> = (0..5).map(|i| format!("opt{i}")).collect();
        let err = validate_options(&opts).unwrap_err();
        assert!(format!("{err}").contains("exceeds max 4"));
    }

    #[test]
    fn validate_options_label_capped_at_60() {
        let label = "x".repeat(MAX_ASK_LABEL_LEN + 1);
        let err = validate_options(&[label]).unwrap_err();
        assert!(format!("{err}").contains("exceeds max 60"));
    }

    #[test]
    fn validate_options_empty_rejected() {
        let err = validate_options(&[]).unwrap_err();
        assert!(format!("{err}").contains("at least one option"));
    }

    #[tokio::test]
    async fn happy_path_returns_selected_label() {
        let tool = AskUserQuestionTool::new(shell_test_ctx(dummy_out()));
        let input = json!({
            "question": "Pick one",
            "options": ["Alpha", "Beta", "Gamma"]
        });
        let out = tool.call(input, fresh_ctx(), fresh_tx()).await.expect("ok");
        assert_eq!(out.data["selected_index"], json!(0));
        assert_eq!(out.data["selected_label"], json!("Alpha"));
    }

    #[tokio::test]
    async fn rejects_missing_question() {
        let tool = AskUserQuestionTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"options": ["A"]}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing question");
        assert!(format!("{err}").contains("missing or non-string question"));
    }

    #[tokio::test]
    async fn rejects_too_many_options() {
        let tool = AskUserQuestionTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(
                json!({"question": "Q", "options": ["a", "b", "c", "d", "e"]}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("too many");
        assert!(format!("{err}").contains("exceeds max 4"));
    }

    struct FixedResolver(usize);
    #[async_trait]
    impl AskUserQuestionResolver for FixedResolver {
        async fn resolve(&self, _: &str, _: &[String]) -> Result<usize, ToolError> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn rejects_resolver_index_out_of_range() {
        let tool = AskUserQuestionTool::with_resolver(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedResolver(5)),
        );
        let err = tool
            .call(
                json!({"question": "Pick", "options": ["A", "B"]}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("out of range");
        assert!(format!("{err}").contains("returned index 5"));
    }
}
