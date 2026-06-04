//! `SkillTool` — resolves a slash-command skill via injected `SkillLoader`.
//!
//! Wire identifiers locked in spec §7:
//! - Tool name: `Skill`.
//! - Descriptor cap: 1024 chars.
//! - Telemetry: `SKILL_STARTED` / `SKILL_COMPLETED` / `SKILL_FAILED`.
//!
//! Contract (parity batch MISC.10, TS `tools/SkillTool/SkillTool.ts`):
//! - Input `{skill, args?}` (TS `:291-298`). `skill` is the slash-command name;
//!   `args` is accepted and echoed but **not** expanded into a prompt.
//! - `validateInput`/`call` trim `skill`, strip a single leading `/`
//!   (TS `:366-372`), then resolve by normalized name.
//! - The three rejection strings are byte-faithful with TS `:406`, `:412-416`,
//!   `:421-427`:
//!   - `Unknown skill: <name>`
//!   - `Skill <name> cannot be used with Skill tool due to disable-model-invocation`
//!   - `Skill <name> is not a prompt-based skill`
//! - Output (inline path) mirrors the TS inline output union (TS `:301-326`):
//!   `{success:true, commandName, allowedTools?, model?, status:"inline"}`.
//!
//! ## Biggest non-faithful surface
//! The entire **forked-agent execution** path (TS `executeForkedSkill` →
//! `runAgent`, `prepareForkedCommandContext`, progress streaming,
//! `createAgentId`), MCP-skill discovery (`getAllCommands` merging
//! `mcp.commands`), remote canonical skills (`EXPERIMENTAL_SKILL_SEARCH`), and
//! frontmatter parsing have **no Rust substrate**. This tool performs *inline
//! resolution + metadata surfacing only*: it loads a descriptor, enforces the
//! locked rejections, and echoes the skill's `model`/`allowedTools`. `args` is
//! accepted but never expanded into a prompt. The descriptor `body` rides along
//! as an extra field (Rust's pragmatic substitute for actually forking).
//!
//! Hermetic by default: the `EmptySkillLoader` always returns "skill not
//! found" (→ `Unknown skill:`). Production hosts inject a loader backed by the
//! real command/skill registry.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{SKILL_COMPLETED, SKILL_FAILED, SKILL_STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock.
pub const SKILL_TOOL_NAME: &str = "Skill";
/// Descriptor (description) char cap (spec §7).
pub const MAX_SKILL_DESCRIPTOR_LEN: usize = 1024;
/// Maximum skill name length (defensive cap; matches team name lock).
pub const MAX_SKILL_NAME_LEN: usize = 128;

/// Command kind for a resolved skill. Mirrors the TS `Command.type` discriminant
/// — only `prompt`-typed commands may be invoked via the Skill tool
/// (TS `:421-427`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillCommandType {
    /// A prompt-based slash command (the only model-invocable kind).
    Prompt,
    /// Any other command kind (local/jsx/etc.) — rejected with the locked
    /// "is not a prompt-based skill" error.
    Other,
}

/// Skill descriptor returned by a [`SkillLoader`]. Subset of the TS
/// `PromptCommand` shape — the fields the Skill tool actually surfaces.
#[derive(Debug, Clone)]
pub struct SkillDescriptor {
    /// Canonical name (matches the normalized input field).
    pub name: String,
    /// Short description (capped at [`MAX_SKILL_DESCRIPTOR_LEN`]).
    pub description: String,
    /// Body content (markdown sans frontmatter). Rides along as an extra
    /// output field — Rust's stand-in for forked execution.
    pub body: String,
    /// Whether model invocation is disabled (TS `disableModelInvocation`).
    /// `true` → rejected with the locked "disable-model-invocation" error.
    pub disable_model_invocation: bool,
    /// The command kind (TS `Command.type`). Only [`SkillCommandType::Prompt`]
    /// is model-invocable.
    pub command_type: SkillCommandType,
    /// Optional model override surfaced in the result (TS `command.model`).
    pub model: Option<String>,
    /// Tools this skill allows, surfaced in the result (TS `allowedTools`).
    pub allowed_tools: Vec<String>,
}

impl Default for SkillDescriptor {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            body: String::new(),
            disable_model_invocation: false,
            command_type: SkillCommandType::Prompt,
            model: None,
            allowed_tools: Vec::new(),
        }
    }
}

/// Loader trait — production wraps the real command/skill registry; tests inject
/// a fixed descriptor.
#[async_trait]
pub trait SkillLoader: Send + Sync {
    /// Load a skill by its normalized name (leading slash already stripped).
    /// Returns `None` if no such skill is registered → `Unknown skill:`.
    async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError>;
}

/// Default hermetic loader — always reports "not found" (→ `Unknown skill:`).
pub struct EmptySkillLoader;

#[async_trait]
impl SkillLoader for EmptySkillLoader {
    async fn load(&self, _name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
        Ok(None)
    }
}

/// `SkillTool` — resolves + validates a slash-command skill.
pub struct SkillTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
    pub(crate) loader: Arc<dyn SkillLoader>,
}

impl SkillTool {
    /// Construct with the default `EmptySkillLoader`.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self {
            ctx,
            loader: Arc::new(EmptySkillLoader),
        }
    }

    /// Construct with a caller-supplied loader.
    #[must_use]
    pub fn with_loader(ctx: tool_api::BuiltinToolContext, loader: Arc<dyn SkillLoader>) -> Self {
        Self { ctx, loader }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "skill": {
                "type": "string",
                "minLength": 1,
                "description": "The skill name. E.g., \"commit\", \"review-pr\", or \"pdf\""
            },
            "args": {
                "type": "string",
                "description": "Optional arguments for the skill"
            }
        },
        "required": ["skill"]
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

/// Trim `skill` and strip a single leading `/` (TS `:356`, `:366-372`).
/// Returns the normalized command name (may be empty if input was blank).
fn normalize_skill_name(skill: &str) -> String {
    let trimmed = skill.trim();
    trimmed.strip_prefix('/').unwrap_or(trimmed).to_string()
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
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
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

    async fn description(&self, input: &Value, _: &DescriptionOptions) -> String {
        // TS `:342`: `Execute skill: ${skill}`.
        match input.get("skill").and_then(Value::as_str) {
            Some(s) => format!("Execute skill: {s}"),
            None => "Execute a slash-command skill.".into(),
        }
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Skill: invoke a slash-command skill by name.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let skill = input
            .get("skill")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("Skill: missing or non-string skill".into()))?;

        // Trim; reject blank (TS `:356-363` → "Invalid skill format").
        let trimmed = skill.trim();
        if trimmed.is_empty() {
            return Err(ValidationError(format!("Invalid skill format: {skill}")));
        }

        // Strip a single leading slash (TS `:366-372`).
        let normalized = normalize_skill_name(skill);
        if normalized.chars().count() > MAX_SKILL_NAME_LEN {
            return Err(ValidationError(format!(
                "Skill: name length {} exceeds max {}",
                normalized.chars().count(),
                MAX_SKILL_NAME_LEN
            )));
        }

        // Resolve + apply the three locked rejections (TS `:399-427`).
        match self.loader.load(&normalized).await {
            Ok(Some(desc)) => {
                if desc.disable_model_invocation {
                    return Err(ValidationError(format!(
                        "Skill {normalized} cannot be used with {SKILL_TOOL_NAME} tool due to disable-model-invocation"
                    )));
                }
                if desc.command_type != SkillCommandType::Prompt {
                    return Err(ValidationError(format!(
                        "Skill {normalized} is not a prompt-based skill"
                    )));
                }
                Ok(())
            }
            Ok(None) => Err(ValidationError(format!("Unknown skill: {normalized}"))),
            // Loader I/O failure — surface verbatim; not one of the locked
            // contract strings.
            Err(e) => {
                let _ = ctx;
                Err(ValidationError(format!("{e}")))
            }
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        let skill = match input.get("skill").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_skill", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "Skill: missing or non-string skill".into(),
                ));
            }
        };

        // `args` is accepted and echoed, never expanded into a prompt
        // (substrate gap — see module doc).
        let args = input
            .get("args")
            .and_then(Value::as_str)
            .map(str::to_string);

        // Trim; reject blank (TS `:356-363`).
        if skill.trim().is_empty() {
            emit_failed(&bus, "empty_skill", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput(format!(
                "Invalid skill format: {skill}"
            )));
        }

        // Strip a single leading slash (TS `:597-598`).
        let command_name = normalize_skill_name(&skill);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("_PROTO_skill_name".into(), pii_str(&command_name));
        bus.log_event(SKILL_STARTED, md).await;

        let loaded = match self.loader.load(&command_name).await {
            Ok(o) => o,
            Err(e) => {
                emit_failed(&bus, "loader_error", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let mut desc = match loaded {
            Some(d) => d,
            None => {
                emit_failed(&bus, "unknown_skill", started.elapsed().as_millis() as u64).await;
                // Locked string (TS `:406`).
                return Err(ToolError::InvalidInput(format!(
                    "Unknown skill: {command_name}"
                )));
            }
        };

        // Locked rejection: disable-model-invocation (TS `:412-416`).
        if desc.disable_model_invocation {
            emit_failed(
                &bus,
                "disable_model_invocation",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "Skill {command_name} cannot be used with {SKILL_TOOL_NAME} tool due to disable-model-invocation"
            )));
        }

        // Locked rejection: non-prompt skill (TS `:421-427`).
        if desc.command_type != SkillCommandType::Prompt {
            emit_failed(&bus, "not_prompt", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput(format!(
                "Skill {command_name} is not a prompt-based skill"
            )));
        }

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

        // Inline output union (TS `:301-326`). `allowedTools` and `model` are
        // optional — omitted when empty/absent — matching the TS `.optional()`
        // surfacing. `commandName` is the normalized name. `body`/`args`/
        // `descriptor_truncated` ride along as extra fields (Rust substitute
        // for actually forking).
        let mut data = json!({
            "success": true,
            "commandName": command_name,
            "status": "inline",
            "body": desc.body,
            "descriptor_truncated": truncated,
        });
        let obj = data.as_object_mut().expect("json object");
        if !desc.allowed_tools.is_empty() {
            obj.insert(
                "allowedTools".into(),
                Value::Array(desc.allowed_tools.into_iter().map(Value::String).collect()),
            );
        }
        if let Some(model) = desc.model {
            obj.insert("model".into(), Value::String(model));
        }
        if let Some(args) = args {
            obj.insert("args".into(), Value::String(args));
        }

        Ok(ToolCallResult {
            data,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// A loader that returns a fixed descriptor for any name.
    struct FixedLoader(Option<SkillDescriptor>);
    #[async_trait]
    impl SkillLoader for FixedLoader {
        async fn load(&self, _name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
            Ok(self.0.clone())
        }
    }

    /// A loader that captures the (normalized) name it was asked to load.
    struct CapturingLoader {
        seen: std::sync::Mutex<Option<String>>,
        desc: Option<SkillDescriptor>,
    }
    #[async_trait]
    impl SkillLoader for CapturingLoader {
        async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
            *self.seen.lock().unwrap() = Some(name.to_string());
            Ok(self.desc.clone())
        }
    }

    fn prompt_desc(name: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.into(),
            description: "a skill".into(),
            body: "body here".into(),
            ..SkillDescriptor::default()
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(SKILL_TOOL_NAME, "Skill");
        assert_eq!(MAX_SKILL_DESCRIPTOR_LEN, 1024);
    }

    #[test]
    fn normalize_strips_single_leading_slash_and_trims() {
        assert_eq!(normalize_skill_name("/commit"), "commit");
        assert_eq!(normalize_skill_name("  /commit  "), "commit");
        assert_eq!(normalize_skill_name("commit"), "commit");
        // Only a single leading slash is stripped.
        assert_eq!(normalize_skill_name("//commit"), "/commit");
    }

    #[test]
    fn schema_uses_skill_and_optional_args() {
        let props = &SCHEMA["properties"];
        assert_eq!(props["skill"]["type"], json!("string"));
        assert_eq!(props["skill"]["minLength"], json!(1));
        assert_eq!(props["args"]["type"], json!("string"));
        assert_eq!(SCHEMA["required"], json!(["skill"]));
        // No legacy `name` field remains.
        assert!(props.get("name").is_none());
    }

    #[tokio::test]
    async fn slash_prefixed_skill_is_normalized_before_lookup() {
        let loader = Arc::new(CapturingLoader {
            seen: std::sync::Mutex::new(None),
            desc: Some(prompt_desc("commit")),
        });
        let tool = SkillTool::with_loader(shell_test_ctx(dummy_out()), loader.clone());
        let out = tool
            .call(json!({"skill": "/commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Loader saw the slash-stripped name.
        assert_eq!(loader.seen.lock().unwrap().as_deref(), Some("commit"));
        // commandName is the normalized name + inline status.
        assert_eq!(out.data["commandName"], json!("commit"));
        assert_eq!(out.data["status"], json!("inline"));
        assert_eq!(out.data["success"], json!(true));
    }

    #[tokio::test]
    async fn unknown_skill_locked_error() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"skill": "absent"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("unknown");
        assert!(format!("{err}").contains("Unknown skill: absent"));
    }

    #[tokio::test]
    async fn disable_model_invocation_locked_error() {
        let desc = SkillDescriptor {
            disable_model_invocation: true,
            ..prompt_desc("locked")
        };
        let tool =
            SkillTool::with_loader(shell_test_ctx(dummy_out()), Arc::new(FixedLoader(Some(desc))));
        let err = tool
            .call(json!({"skill": "locked"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("disabled");
        assert!(format!("{err}").contains(
            "Skill locked cannot be used with Skill tool due to disable-model-invocation"
        ));
    }

    #[tokio::test]
    async fn non_prompt_skill_locked_error() {
        let desc = SkillDescriptor {
            command_type: SkillCommandType::Other,
            ..prompt_desc("local-cmd")
        };
        let tool =
            SkillTool::with_loader(shell_test_ctx(dummy_out()), Arc::new(FixedLoader(Some(desc))));
        let err = tool
            .call(json!({"skill": "local-cmd"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("non-prompt");
        assert!(format!("{err}").contains("Skill local-cmd is not a prompt-based skill"));
    }

    #[tokio::test]
    async fn result_surfaces_model_and_allowed_tools_and_status_inline() {
        let desc = SkillDescriptor {
            model: Some("opus".into()),
            allowed_tools: vec!["Bash".into(), "Read".into()],
            ..prompt_desc("rich")
        };
        let tool =
            SkillTool::with_loader(shell_test_ctx(dummy_out()), Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "rich"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["status"], json!("inline"));
        assert_eq!(out.data["model"], json!("opus"));
        assert_eq!(out.data["allowedTools"], json!(["Bash", "Read"]));
        assert_eq!(out.data["commandName"], json!("rich"));
    }

    #[tokio::test]
    async fn model_and_allowed_tools_omitted_when_absent() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("plain")))),
        );
        let out = tool
            .call(json!({"skill": "plain"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Optional fields are omitted (matches TS `.optional()` surfacing).
        assert!(out.data.get("model").is_none());
        assert!(out.data.get("allowedTools").is_none());
        assert_eq!(out.data["body"], json!("body here"));
    }

    #[tokio::test]
    async fn args_accepted_optionally_and_echoed() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("commit")))),
        );
        // With args.
        let out = tool
            .call(
                json!({"skill": "commit", "args": "--amend"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["args"], json!("--amend"));
        // Without args — no `args` key.
        let out2 = tool
            .call(json!({"skill": "commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(out2.data.get("args").is_none());
    }

    #[tokio::test]
    async fn descriptor_truncated_at_1024() {
        let desc = SkillDescriptor {
            description: "x".repeat(MAX_SKILL_DESCRIPTOR_LEN + 100),
            ..prompt_desc("huge")
        };
        let tool =
            SkillTool::with_loader(shell_test_ctx(dummy_out()), Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "huge"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(out.data["body"].as_str().is_some(), "body present");
        assert_eq!(out.data["descriptor_truncated"], json!(true));
    }

    #[tokio::test]
    async fn blank_skill_rejected() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"skill": "   "}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("blank");
        assert!(format!("{err}").contains("Invalid skill format"));
    }

    #[tokio::test]
    async fn rejects_missing_skill() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string skill"));
    }

    #[tokio::test]
    async fn validate_input_applies_locked_rejections() {
        // Unknown via empty loader.
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(&json!({"skill": "/absent"}), &fresh_ctx())
            .await
            .expect_err("unknown");
        assert!(err.0.contains("Unknown skill: absent"));

        // disable-model-invocation.
        let desc = SkillDescriptor {
            disable_model_invocation: true,
            ..prompt_desc("x")
        };
        let tool =
            SkillTool::with_loader(shell_test_ctx(dummy_out()), Arc::new(FixedLoader(Some(desc))));
        let err = tool
            .validate_input(&json!({"skill": "x"}), &fresh_ctx())
            .await
            .expect_err("disabled");
        assert!(err
            .0
            .contains("cannot be used with Skill tool due to disable-model-invocation"));
    }
}
