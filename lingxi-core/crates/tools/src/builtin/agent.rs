//! `AgentTool` — spawns a subagent via the M1 agent pool.
//!
//! Spec §4 Flow D + §7 line 491-499. claude-code source:
//! `claude-code/src/tools/AgentTool/AgentTool.tsx` + `built-in/*.ts`.
//!
//! **Architectural note (M4-05):** the existing M1 dep graph has
//! `lingxi-tasks` and `lingxi-agent` already depending on `lingxi-tools`,
//! which prevents this crate from depending on either. Subagent spawn and
//! `BudgetEnforcer` inheritance therefore land here as a tool *surface*
//! (locked schemas + locked error strings + locked telemetry events); the
//! actual wiring through `StateMachinePool::allocate` + parent
//! `Arc<BudgetEnforcer>` happens in `lingxi-coordinator` post-M5 when the
//! cycle issue is resolved by extracting `ToolRegistry` into a leaf crate.
//! All byte-locks (tool name, 6 subagent types, M3-05 denial format,
//! 3 telemetry events) are preserved.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{AGENT_COMPLETED_M4_05, AGENT_FAILED, AGENT_STARTED};
use lingxi_telemetry::AnalyticsBus;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

/// `Agent` — canonical tool name (claude-code `AGENT_TOOL_NAME`).
pub const AGENT_TOOL_NAME: &str = "Agent";

/// `Task` — legacy alias the dispatcher must accept (claude-code
/// `LEGACY_AGENT_TOOL_NAME`).
pub const LEGACY_AGENT_TOOL_NAME: &str = "Task";

/// Six built-in subagent types — byte-aligned with upstream
/// `claude-code/src/tools/AgentTool/built-in/*.ts`.
pub const BUILTIN_SUBAGENT_TYPES: &[&str] = &[
    "general-purpose",
    "Plan",
    "Explore",
    "verification",
    "claude-code-guide",
    "statusline-setup",
];

/// Prefix locked by M3-05 (`cost/src/budget.rs` budget-exceeded test fixtures).
/// Production constructs the full string via
/// `format!("Budget exceeded (${:.2}); stopped.", dollars)`.
pub const SUBAGENT_BUDGET_DENIED_PREFIX: &str = "Budget exceeded ($";

/// Input shape accepted by `AgentTool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentToolInput {
    /// One of [`BUILTIN_SUBAGENT_TYPES`].
    pub subagent_type: String,
    /// Initial prompt seeded into the subagent's first turn.
    pub prompt: String,
    /// Optional context files (paths) injected as `system`-tagged messages.
    #[serde(default)]
    pub context_paths: Vec<PathBuf>,
}

static AGENT_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "subagent_type": { "type": "string", "minLength": 1 },
            "prompt":        { "type": "string", "minLength": 1 },
            "context_paths": { "type": "array", "items": { "type": "string" }, "default": [] }
        },
        "required": ["subagent_type", "prompt"]
    })
});

/// Format the M3-05 byte-locked budget-exceeded denial string.
///
/// Caller passes `current_nano_usd`; output is the literal
/// `"Budget exceeded (${dollars:.2}); stopped."` (see `cost/src/budget.rs`).
#[must_use]
pub fn format_budget_denied(current_nano_usd: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let dollars = current_nano_usd as f64 / 1_000_000_000.0;
    format!("Budget exceeded (${dollars:.2}); stopped.")
}

/// `AgentTool` — spawn a subagent (surface only; recursive dispatch lands
/// in `lingxi-coordinator` post-M5).
pub struct AgentTool {
    ctx: BuiltinToolContext,
}

impl AgentTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    fn fresh_invocation_id() -> String {
        crate::builtin::file_read::ulid_or_uuid()
    }

    async fn emit_started(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        subagent_type: &str,
        prompt_chars: usize,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "subagent_type".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(subagent_type.to_string()).into_inner(),
            ),
        );
        md.insert(
            "prompt_chars".into(),
            AnalyticsValue::Int(prompt_chars as i64),
        );
        bus.log_event(AGENT_STARTED, md).await;
    }

    async fn emit_completed(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        duration_ms: u64,
        subagent_type: &str,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "subagent_type".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(subagent_type.to_string()).into_inner(),
            ),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(AGENT_COMPLETED_M4_05, md).await;
    }

    async fn emit_failed(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        error_kind: &str,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "error_kind".into(),
            AnalyticsValue::String(Verified::assert_safe(error_kind.to_string()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(AGENT_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        AGENT_TOOL_NAME
    }
    fn aliases(&self) -> &[&str] {
        const ALIASES: &[&str] = &[LEGACY_AGENT_TOOL_NAME];
        ALIASES
    }
    fn input_schema(&self) -> &Value {
        &AGENT_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        true
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "Agent spawn delegates to host runtime; no direct side-effect".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Spawn a subagent of one of the built-in types".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use Agent to dispatch a focused subagent (general-purpose / Plan / \
         Explore / verification / claude-code-guide / statusline-setup) with \
         a seeded prompt. The subagent runs in its own state-machine slot \
         and inherits the parent's BudgetEnforcer."
            .into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = Self::fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        // 1. Parse input.
        let parsed: AgentToolInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "invalid_input",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "Agent: invalid input shape: {e}"
                )));
            }
        };

        // 2. Validate subagent_type.
        if !BUILTIN_SUBAGENT_TYPES.contains(&parsed.subagent_type.as_str()) {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "unknown_subagent_type",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "Agent: unknown subagent_type '{}' (known: {})",
                parsed.subagent_type,
                BUILTIN_SUBAGENT_TYPES.join(", ")
            )));
        }

        // 3. Validate prompt.
        if parsed.prompt.trim().is_empty() {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "empty_prompt",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput("Agent: prompt is empty".into()));
        }

        // 4. Emit started.
        Self::emit_started(
            &bus,
            &invocation_id,
            &parsed.subagent_type,
            parsed.prompt.chars().count(),
        )
        .await;

        // 5. Subagent dispatch surface (real recursion wired in post-M5).
        // For now: return a shape-correct stub so the tool composes cleanly
        // with the registry + UI. The byte-locked event triple still fires.
        let subagent_id = format!("agent-{invocation_id}");
        let final_result = json!({ "summary": "subagent surface (M4-05): dispatch deferred to post-M5 coordinator wiring" });

        let duration_ms = started.elapsed().as_millis() as u64;
        Self::emit_completed(&bus, &invocation_id, duration_ms, &parsed.subagent_type).await;

        Ok(ToolCallResult {
            data: json!({
                "subagent_id": subagent_id,
                "subagent_type": parsed.subagent_type,
                "result": final_result
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

    #[test]
    fn agent_tool_name_locked() {
        assert_eq!(AGENT_TOOL_NAME, "Agent");
        assert_eq!(LEGACY_AGENT_TOOL_NAME, "Task");
    }

    #[test]
    fn six_builtin_subagent_types_byte_aligned() {
        assert_eq!(
            BUILTIN_SUBAGENT_TYPES,
            &[
                "general-purpose",
                "Plan",
                "Explore",
                "verification",
                "claude-code-guide",
                "statusline-setup"
            ]
        );
        assert_eq!(BUILTIN_SUBAGENT_TYPES.len(), 6);
    }

    #[test]
    fn budget_exceeded_format_matches_m3_05_lock() {
        // M3-05 byte-locked string format.
        assert_eq!(
            format_budget_denied(150_750_000_000),
            "Budget exceeded ($150.75); stopped."
        );
        assert_eq!(
            format_budget_denied(1_500_000_000),
            "Budget exceeded ($1.50); stopped."
        );
    }

    #[test]
    fn budget_denied_prefix_locked() {
        assert_eq!(SUBAGENT_BUDGET_DENIED_PREFIX, "Budget exceeded ($");
        assert!(format_budget_denied(0).starts_with(SUBAGENT_BUDGET_DENIED_PREFIX));
    }

    #[test]
    fn agent_input_serde_roundtrip() {
        let v = json!({
            "subagent_type": "general-purpose",
            "prompt": "Explore the repo structure.",
            "context_paths": []
        });
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.subagent_type, "general-purpose");
        assert_eq!(parsed.prompt, "Explore the repo structure.");
        assert!(parsed.context_paths.is_empty());
    }

    #[test]
    fn agent_input_defaults_context_paths_to_empty() {
        let v = json!({"subagent_type": "Plan", "prompt": "Design."});
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert!(parsed.context_paths.is_empty());
    }
}
