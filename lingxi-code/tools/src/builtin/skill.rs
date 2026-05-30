//! `SkillTool` — loads a skill descriptor via injected `SkillLoader`.
//!
//! Wire identifiers locked in spec §7:
//! - Descriptor cap: 1024 chars.
//!
//! Hermetic by default: the `EmptySkillLoader` always returns "skill not
//! found". Production hosts inject a loader backed by `lingxi_skills::registry`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{SKILL_COMPLETED, SKILL_FAILED, SKILL_STARTED};
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
pub const SKILL_TOOL_NAME: &str = "Skill";
/// Descriptor (description) char cap (spec §7).
pub const MAX_SKILL_DESCRIPTOR_LEN: usize = 1024;
/// Maximum skill name length (defensive cap; matches team name lock).
pub const MAX_SKILL_NAME_LEN: usize = 128;

/// Minimal skill descriptor returned to the model. Subset of
/// `lingxi_skills::model::Skill` — only the fields the model actually needs.
#[derive(Debug, Clone)]
pub struct SkillDescriptor {
    /// Canonical name (matches the input field).
    pub name: String,
    /// Short description (capped at [`MAX_SKILL_DESCRIPTOR_LEN`]).
    pub description: String,
    /// Body content (markdown sans frontmatter).
    pub body: String,
}

/// Loader trait — production wraps `lingxi_skills::registry::SkillRegistry`;
/// tests inject a fixed map.
#[async_trait]
pub trait SkillLoader: Send + Sync {
    /// Load a skill by name. Returns `None` if not registered.
    async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError>;
}

/// Default hermetic loader — always reports "not found".
pub struct EmptySkillLoader;

#[async_trait]
impl SkillLoader for EmptySkillLoader {
    async fn load(&self, _name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
        Ok(None)
    }
}

/// `SkillTool` — loads + validates a skill descriptor.
pub struct SkillTool {
    pub(crate) ctx: super::BuiltinToolContext,
    pub(crate) loader: Arc<dyn SkillLoader>,
}

impl SkillTool {
    /// Construct with the default `EmptySkillLoader`.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self {
            ctx,
            loader: Arc::new(EmptySkillLoader),
        }
    }

    /// Construct with a caller-supplied loader.
    #[must_use]
    pub fn with_loader(ctx: super::BuiltinToolContext, loader: Arc<dyn SkillLoader>) -> Self {
        Self { ctx, loader }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "minLength": 1, "maxLength": 128 }
        },
        "required": ["name"]
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
    bus.log_event(SKILL_FAILED, md).await;
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        SKILL_TOOL_NAME
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
                reason: "Skill loads a registered skill descriptor (read-only)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Load a skill by name; returns name, description (≤1024 chars), and body.".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Skill: load a registered skill's description + body.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("Skill: missing or non-string name".into()))?;
        if name.is_empty() {
            return Err(ValidationError("Skill: name is empty".into()));
        }
        if name.chars().count() > MAX_SKILL_NAME_LEN {
            return Err(ValidationError(format!(
                "Skill: name length {} exceeds max {}",
                name.chars().count(),
                MAX_SKILL_NAME_LEN
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

        let name = match input.get("name").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_name", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "Skill: missing or non-string name".into(),
                ));
            }
        };
        if name.is_empty() {
            emit_failed(&bus, "empty_name", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput("Skill: name is empty".into()));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("_PROTO_skill_name".into(), pii_str(&name));
        bus.log_event(SKILL_STARTED, md).await;

        let loaded = match self.loader.load(&name).await {
            Ok(o) => o,
            Err(e) => {
                emit_failed(&bus, "loader_error", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let mut desc = match loaded {
            Some(d) => d,
            None => {
                emit_failed(&bus, "not_found", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(format!(
                    "Skill: skill '{name}' is not registered"
                )));
            }
        };

        // Enforce descriptor cap byte-lock.
        let truncated = if desc.description.chars().count() > MAX_SKILL_DESCRIPTOR_LEN {
            let s: String = desc
                .description
                .chars()
                .take(MAX_SKILL_DESCRIPTOR_LEN)
                .collect();
            desc.description = s;
            true
        } else {
            false
        };

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert(
            "descriptor_len".into(),
            AnalyticsValue::Int(desc.description.chars().count() as i64),
        );
        md.insert(
            "descriptor_truncated".into(),
            AnalyticsValue::Bool(truncated),
        );
        bus.log_event(SKILL_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "name": desc.name,
                "description": desc.description,
                "body": desc.body,
                "descriptor_truncated": truncated,
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

    struct FixedLoader(Option<SkillDescriptor>);
    #[async_trait]
    impl SkillLoader for FixedLoader {
        async fn load(&self, _name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(SKILL_TOOL_NAME, "Skill");
        assert_eq!(MAX_SKILL_DESCRIPTOR_LEN, 1024);
    }

    #[tokio::test]
    async fn returns_not_found_with_empty_loader() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"name": "absent"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("not found");
        assert!(format!("{err}").contains("is not registered"));
    }

    #[tokio::test]
    async fn happy_path_returns_descriptor() {
        let desc = SkillDescriptor {
            name: "demo".into(),
            description: "short".into(),
            body: "body here".into(),
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"name": "demo"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["name"], json!("demo"));
        assert_eq!(out.data["description"], json!("short"));
        assert_eq!(out.data["body"], json!("body here"));
        assert_eq!(out.data["descriptor_truncated"], json!(false));
    }

    #[tokio::test]
    async fn descriptor_truncated_at_1024() {
        let desc = SkillDescriptor {
            name: "huge".into(),
            description: "x".repeat(MAX_SKILL_DESCRIPTOR_LEN + 100),
            body: String::new(),
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"name": "huge"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            out.data["description"].as_str().unwrap().chars().count(),
            MAX_SKILL_DESCRIPTOR_LEN
        );
        assert_eq!(out.data["descriptor_truncated"], json!(true));
    }

    #[tokio::test]
    async fn rejects_missing_name() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string name"));
    }
}
