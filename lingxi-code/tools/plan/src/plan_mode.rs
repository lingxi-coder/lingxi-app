//! `EnterPlanModeTool` + `ExitPlanModeTool` — flip `SessionState.plan_mode`
//! and emit the byte-locked markers `[PLAN MODE]` / `[EXIT PLAN MODE]`
//! (spec §7 line 487).
//!
//! ## Divergence from TS (parity Batch MISC.11 — close-parity, flag prominently)
//!
//! The TS reference (`EnterPlanModeTool.ts`, `ExitPlanModeV2Tool.ts`) implements
//! plan mode on top of a real permission-mode substrate: it sets
//! `toolPermissionContext.mode = 'plan'` via `applyPermissionUpdate`, runs the
//! classifier activation in `prepareContextForPlanMode`, reads the plan from disk
//! (`getPlan`/`getPlanFilePath`), and performs a plan-approval handoff
//! (`setAwaitingPlanApproval`, teammate mailbox). LingXi's `SessionState` has only
//! a `plan_mode: bool`, **not** a permission-mode enum — none of that machinery
//! has a Rust home. This batch lands only the guardable, headless-meaningful
//! pieces faithful to TS:
//!   * the agent-context guard (`EnterPlanMode` rejects when `ctx.agent_id` is set;
//!     TS throws at `EnterPlanModeTool.ts:78-80`),
//!   * the `Entered plan mode...` instruction block reaching the model
//!     (port of `mapToolResultToToolResultBlockParam` `:103-125`),
//!   * the `ExitPlanMode` `{allowedPrompts?:[{tool:"Bash", prompt}]}` input schema
//!     (`ExitPlanModeV2Tool.ts:64-89`) and the `{plan, isAgent, allowedPrompts}`
//!     output passthrough (`:110-120`). Rust has no on-disk plan store, so `plan`
//!     is whatever the model passed (or `null`).
//!
//! TS uses prose, not literal markers; LingXi keeps the byte-locked
//! `[PLAN MODE]` / `[EXIT PLAN MODE]` markers (LingXi fixture lock) **and** adds
//! the TS instruction prose alongside them.

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

/// Instruction block surfaced to the model on entering plan mode. Byte-faithful
/// port of the non-interview-phase branch of `EnterPlanModeTool.ts`
/// `mapToolResultToToolResultBlockParam` (`:108-118`). LingXi has no
/// `isPlanModeInterviewPhaseEnabled` substrate, so the close-parity default
/// (the numbered exploration steps) is always emitted.
pub const ENTER_PLAN_MODE_INSTRUCTIONS: &str = "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach.

In plan mode, you should:
1. Thoroughly explore the codebase to understand existing patterns
2. Identify similar features and architectural approaches
3. Consider multiple approaches and their trade-offs
4. Use AskUserQuestion if you need to clarify the approach
5. Design a concrete implementation strategy
6. When ready, use ExitPlanMode to present your plan for approval

Remember: DO NOT write or edit any files yet. This is a read-only exploration and planning phase.";

/// Locked rejection string for using `EnterPlanMode` inside an agent context.
/// Byte-faithful to the TS throw at `EnterPlanModeTool.ts:79`.
const ENTER_PLAN_MODE_AGENT_GUARD_MSG: &str =
    "EnterPlanMode tool cannot be used in agent contexts";

/// Full model-facing tool prompt for `ExitPlanMode` — byte-faithful port of
/// `EXIT_PLAN_MODE_V2_TOOL_PROMPT` (`ExitPlanModeTool/prompt.ts:6-29`), returned
/// by `ExitPlanModeV2Tool.ts:154-156`. The TS template interpolates
/// `${ASK_USER_QUESTION_TOOL_NAME}` (= `"AskUserQuestion"`, prompt.ts:4); that
/// substitution is inlined here so the text is verbatim what the model sees.
pub const EXIT_PLAN_MODE_V2_TOOL_PROMPT: &str = r#"Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.

## How This Tool Works
- You should have already written your plan to the plan file specified in the plan mode system message
- This tool does NOT take the plan content as a parameter - it will read the plan from the file you wrote
- This tool simply signals that you're done planning and ready for the user to review and approve
- The user will see the contents of your plan file when they review it

## When to Use This Tool
IMPORTANT: Only use this tool when the task requires planning the implementation steps of a task that requires writing code. For research tasks where you're gathering information, searching files, reading files or in general trying to understand the codebase - do NOT use this tool.

## Before Using This Tool
Ensure your plan is complete and unambiguous:
- If you have unresolved questions about requirements or approach, use AskUserQuestion first (in earlier phases)
- Once your plan is finalized, use THIS tool to request approval

**Important:** Do NOT use AskUserQuestion to ask "Is this plan okay?" or "Should I proceed?" - that's exactly what THIS tool does. ExitPlanMode inherently requests user approval of your plan.

## Examples

1. Initial task: "Search for and understand the implementation of vim mode in the codebase" - Do not use the exit plan mode tool because you are not planning the implementation steps of a task.
2. Initial task: "Help me implement yank mode for vim" - Use the exit plan mode tool after you have finished planning the implementation steps of the task.
3. Initial task: "Add a new feature to handle user authentication" - If unsure about auth method (OAuth, JWT, etc.), use AskUserQuestion first, then use exit plan mode tool after clarifying the approach.
"#;

/// Model-facing approval text emitted by `ExitPlanMode` in an agent context —
/// byte-faithful port of the `isAgent` branch of
/// `mapToolResultToToolResultBlockParam` (`ExitPlanModeV2Tool.ts:452-459`).
const EXIT_PLAN_APPROVED_AGENT_MSG: &str =
    "User has approved the plan. There is nothing else needed from you now. Please respond with \"ok\"";

/// Model-facing approval text when the plan is empty — byte-faithful port of the
/// empty-plan branch (`ExitPlanModeV2Tool.ts:461-468`).
const EXIT_PLAN_APPROVED_EMPTY_MSG: &str =
    "User has approved exiting plan mode. You can now proceed.";

/// Opening line of the model-facing approval text when a plan is present — port
/// of the approved branch (`ExitPlanModeV2Tool.ts:481-491`). The TS text also
/// emits a "Your plan has been saved to: ${filePath}" line plus a refer-back
/// line; LingXi has no on-disk plan store (`getPlanFilePath` has no Rust home),
/// so those file-path-dependent lines are omitted rather than inventing a path.
/// The "## Approved Plan:\n<plan>" section is preserved.
const EXIT_PLAN_APPROVED_PREFIX: &str =
    "User has approved your plan. You can now start coding. Start with updating your todo list if applicable";

/// Locked rejection string for calling `ExitPlanMode` outside plan mode —
/// byte-faithful to `validateInput` (`ExitPlanModeV2Tool.ts:212-216`).
const EXIT_PLAN_MODE_NOT_IN_PLAN_MODE_MSG: &str =
    "You are not in plan mode. To enter plan mode, call the EnterPlanMode tool first. If your plan was already approved, continue with implementation.";

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

/// `ExitPlanMode` input schema — port of `ExitPlanModeV2Tool.ts:64-89`. Accepts
/// an optional `allowedPrompts` array of prompt-based permission requests, each
/// `{tool:"Bash", prompt:string}`. The schema is a `passthrough()` in TS (extra
/// keys allowed so `normalizeToolInput` can inject `plan`/`planFilePath`), hence
/// `additionalProperties: true` here.
// Mirrors the pre-existing `EMPTY_INPUT_SCHEMA` `once_cell::Lazy` style above;
// `allow` keeps this batch from adding a net-new pedantic warning while staying
// consistent with the surrounding code (a `LazyLock` migration is out of scope).
// `unknown_lints` keeps the pinned 1.82 toolchain (whose clippy predates the
// `non_std_lazy_statics` lint) from erroring on the allow below under `-D warnings`,
// while newer clippy still honors the allow.
#[allow(unknown_lints, clippy::non_std_lazy_statics)]
static EXIT_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "allowedPrompts": {
                "type": "array",
                "description": "Prompt-based permissions needed to implement the plan. These describe categories of actions rather than specific commands.",
                "items": {
                    "type": "object",
                    "properties": {
                        "tool": {
                            "type": "string",
                            "enum": ["Bash"],
                            "description": "The tool this prompt applies to"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Semantic description of the action, e.g. \"run tests\", \"install dependencies\""
                        }
                    },
                    "required": ["tool", "prompt"],
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": true
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

        // Agent-context guard — TS throws here (`EnterPlanModeTool.ts:78-80`):
        // plan mode is a user-interactive control that has no meaning inside a
        // spawned agent. Reject before touching session state.
        if ctx.agent_id.is_some() {
            let dur = started_at.elapsed().as_millis() as u64;
            self.emit_failed(&invocation_id, "agent_context", dur).await;
            return Err(ToolError::InvalidInput(
                ENTER_PLAN_MODE_AGENT_GUARD_MSG.into(),
            ));
        }

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
                // Instruction block ported from TS `mapToolResultToToolResultBlockParam`
                // (`EnterPlanModeTool.ts:103-125`) so the exploration guidance reaches
                // the model (TS embeds it as the `tool_result` content).
                "instructions": ENTER_PLAN_MODE_INSTRUCTIONS,
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
        &EXIT_INPUT_SCHEMA
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
        // Full multi-section tool prompt (TS `ExitPlanModeV2Tool.ts:154-156`
        // returning `EXIT_PLAN_MODE_V2_TOOL_PROMPT`), not a one-line stub.
        EXIT_PLAN_MODE_V2_TOOL_PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
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
                    EXIT_PLAN_MODE_NOT_IN_PLAN_MODE_MSG.into(),
                ));
            }
            guard.plan_mode = false;
        }
        let duration_ms = started_at.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, duration_ms).await;

        // Output passthrough ported from `ExitPlanModeV2Tool.ts:110-120` (the
        // `{plan, isAgent, filePath?}` output shape). Rust has no on-disk plan
        // store (`getPlan`/`getPlanFilePath`), so `plan` is whatever the model
        // injected via `input.plan` or `null`; `filePath` is intentionally
        // omitted. `allowedPrompts` is echoed back from the input schema.
        let is_agent = ctx.agent_id.is_some();
        let plan = input.get("plan").cloned().unwrap_or(Value::Null);
        let allowed_prompts = input.get("allowedPrompts").cloned().unwrap_or(Value::Null);

        // Model-facing prose, mirroring TS `mapToolResultToToolResultBlockParam`
        // (`ExitPlanModeV2Tool.ts:419-492`). Without this the orchestrator
        // (`turn_loop.rs` `tool_result_to_model_text`) would JSON-dump the whole
        // `data` object to the model. Branch order matches TS: agent context →
        // empty plan → approved plan. (TS' `awaitingLeaderApproval` teammate
        // branch has no Rust substrate and is not represented.)
        let model_content = if is_agent {
            EXIT_PLAN_APPROVED_AGENT_MSG.to_string()
        } else {
            let plan_text = plan.as_str().unwrap_or("");
            if plan_text.trim().is_empty() {
                EXIT_PLAN_APPROVED_EMPTY_MSG.to_string()
            } else {
                format!("{EXIT_PLAN_APPROVED_PREFIX}\n\n## Approved Plan:\n{plan_text}")
            }
        };

        Ok(ToolCallResult {
            data: json!({
                "marker": PLAN_MODE_EXIT_MARKER,
                "plan_mode": false,
                "plan": plan,
                "isAgent": is_agent,
                "allowedPrompts": allowed_prompts,
                // Model-facing string preferred by the orchestrator over a JSON
                // dump of `data` (TS prose parity, EXITPLAN.1).
                "model_content": model_content,
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
    use protocol::{AgentId, SessionId};
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
        // Instruction block (TS `mapToolResultToToolResultBlockParam`) reaches the model.
        assert_eq!(res.data["instructions"], ENTER_PLAN_MODE_INSTRUCTIONS);
        let instructions = res.data["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("Entered plan mode."));
        assert!(instructions.contains("6. When ready, use ExitPlanMode to present your plan for approval"));
        assert!(instructions.contains("DO NOT write or edit any files yet"));
        assert!(session.lock().await.plan_mode);
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_PLAN_MODE_STARTED.to_string()));
        assert!(names.contains(&ENTER_PLAN_MODE_COMPLETED.to_string()));
    }

    #[tokio::test]
    async fn enter_in_agent_context_rejects_with_locked_string() {
        let (bctx, sink, session, mut use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        use_ctx.agent_id = Some(AgentId::new());
        let tool = EnterPlanModeTool::new(bctx);
        let err = tool
            .call(json!({}), use_ctx, fresh_tx())
            .await
            .expect_err("enter in agent context must fail");
        assert_eq!(
            format!("{err}"),
            "invalid input: EnterPlanMode tool cannot be used in agent contexts"
        );
        // The guard runs before any session mutation.
        assert!(!session.lock().await.plan_mode);
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_PLAN_MODE_STARTED.to_string()));
        assert!(names.contains(&ENTER_PLAN_MODE_FAILED.to_string()));
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
        // No model-supplied plan / agent context → null plan, isAgent false.
        assert_eq!(res.data["plan"], Value::Null);
        assert_eq!(res.data["isAgent"], false);
        // Empty-plan branch (TS `ExitPlanModeV2Tool.ts:461-468`).
        assert_eq!(res.data["model_content"], EXIT_PLAN_APPROVED_EMPTY_MSG);
        assert_eq!(
            res.data["model_content"],
            "User has approved exiting plan mode. You can now proceed."
        );
        assert!(!session.lock().await.plan_mode);
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&EXIT_PLAN_MODE_COMPLETED.to_string()));
    }

    #[test]
    fn exit_input_schema_accepts_allowed_prompts() {
        let schema = &*EXIT_INPUT_SCHEMA;
        let props = &schema["properties"]["allowedPrompts"];
        assert_eq!(props["type"], "array");
        let item = &props["items"];
        assert_eq!(item["properties"]["tool"]["enum"], json!(["Bash"]));
        assert_eq!(item["properties"]["prompt"]["type"], "string");
        assert_eq!(item["required"], json!(["tool", "prompt"]));
        // passthrough() in TS → extra keys (plan/planFilePath) allowed.
        assert_eq!(schema["additionalProperties"], true);
    }

    #[tokio::test]
    async fn exit_accepts_allowed_prompts_and_passes_through() {
        let (bctx, sink, session, use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        session.lock().await.plan_mode = true; // pre-arm
        let tool = ExitPlanModeTool::new(bctx);
        let input = json!({
            "allowedPrompts": [{ "tool": "Bash", "prompt": "run tests" }],
            "plan": "Step 1. Do the thing.",
        });
        let res = tool
            .call(input, use_ctx, fresh_tx())
            .await
            .expect("exit must succeed with allowedPrompts");
        assert_eq!(res.data["marker"], "[EXIT PLAN MODE]");
        assert_eq!(res.data["plan_mode"], false);
        // Model-supplied plan is echoed back (no on-disk store in Rust).
        assert_eq!(res.data["plan"], "Step 1. Do the thing.");
        assert_eq!(res.data["isAgent"], false);
        assert_eq!(
            res.data["allowedPrompts"],
            json!([{ "tool": "Bash", "prompt": "run tests" }])
        );
        // Approved-plan branch (TS `ExitPlanModeV2Tool.ts:481-491`): prefix line
        // + "## Approved Plan:" + the plan text. File-path lines are omitted
        // (no on-disk plan store in Rust; no path invented).
        assert_eq!(
            res.data["model_content"],
            "User has approved your plan. You can now start coding. Start with updating your todo list if applicable\n\n## Approved Plan:\nStep 1. Do the thing."
        );
        assert!(!session.lock().await.plan_mode);
    }

    #[tokio::test]
    async fn exit_in_agent_context_reports_is_agent() {
        let (bctx, sink, session, mut use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        session.lock().await.plan_mode = true; // pre-arm
        use_ctx.agent_id = Some(AgentId::new());
        let tool = ExitPlanModeTool::new(bctx);
        let res = tool
            .call(json!({}), use_ctx, fresh_tx())
            .await
            .expect("exit must succeed in agent context");
        assert_eq!(res.data["isAgent"], true);
        assert_eq!(res.data["plan"], Value::Null);
        // Agent branch wins over plan state (TS `ExitPlanModeV2Tool.ts:452-459`).
        assert_eq!(res.data["model_content"], EXIT_PLAN_APPROVED_AGENT_MSG);
        assert_eq!(
            res.data["model_content"],
            "User has approved the plan. There is nothing else needed from you now. Please respond with \"ok\""
        );
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
        // Out-of-plan-mode rejection now matches TS `validateInput`
        // (`ExitPlanModeV2Tool.ts:212-216`).
        assert_eq!(
            format!("{err}"),
            format!("invalid input: {EXIT_PLAN_MODE_NOT_IN_PLAN_MODE_MSG}")
        );
        assert_eq!(
            format!("{err}"),
            "invalid input: You are not in plan mode. To enter plan mode, call the EnterPlanMode tool first. If your plan was already approved, continue with implementation."
        );
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&EXIT_PLAN_MODE_FAILED.to_string()));
    }

    #[tokio::test]
    async fn exit_prompt_returns_full_ts_tool_prompt() {
        let (bctx, _sink, _session, _use_ctx) = make_ctx();
        let tool = ExitPlanModeTool::new(bctx);
        let prompt = tool
            .prompt(&PromptOptions {
                include_examples: true,
                model: None,
            })
            .await;
        // Verbatim port of `EXIT_PLAN_MODE_V2_TOOL_PROMPT` (prompt.ts:6-29).
        assert_eq!(prompt, EXIT_PLAN_MODE_V2_TOOL_PROMPT);
        // Multi-section, not the old one-line stub.
        assert!(prompt.starts_with(
            "Use this tool when you are in plan mode and have finished writing your plan"
        ));
        assert!(prompt.contains("## How This Tool Works"));
        assert!(prompt.contains("## When to Use This Tool"));
        assert!(prompt.contains("## Before Using This Tool"));
        assert!(prompt.contains("## Examples"));
        // `${ASK_USER_QUESTION_TOOL_NAME}` was interpolated to "AskUserQuestion".
        assert!(prompt.contains("use AskUserQuestion first (in earlier phases)"));
        assert!(!prompt.contains("${ASK_USER_QUESTION_TOOL_NAME}"));
        // Trailing newline preserved from the TS template literal.
        assert!(prompt.ends_with("after clarifying the approach.\n"));
    }

    #[test]
    fn exit_model_content_branch_strings_match_ts() {
        // Byte-checks against the TS `mapToolResultToToolResultBlockParam` strings.
        assert_eq!(
            EXIT_PLAN_APPROVED_AGENT_MSG,
            "User has approved the plan. There is nothing else needed from you now. Please respond with \"ok\""
        );
        assert_eq!(
            EXIT_PLAN_APPROVED_EMPTY_MSG,
            "User has approved exiting plan mode. You can now proceed."
        );
        assert_eq!(
            EXIT_PLAN_APPROVED_PREFIX,
            "User has approved your plan. You can now start coding. Start with updating your todo list if applicable"
        );
    }

    #[tokio::test]
    async fn exit_whitespace_only_plan_uses_empty_branch() {
        // TS empty-plan guard is `!plan || plan.trim() === ''` — whitespace-only
        // plan text takes the empty branch, not the approved branch.
        let (bctx, sink, session, use_ctx) = make_ctx();
        bctx.bus.attach_sink(sink.clone()).await;
        session.lock().await.plan_mode = true; // pre-arm
        let tool = ExitPlanModeTool::new(bctx);
        let res = tool
            .call(json!({ "plan": "   \n\t " }), use_ctx, fresh_tx())
            .await
            .expect("exit must succeed with whitespace plan");
        assert_eq!(res.data["model_content"], EXIT_PLAN_APPROVED_EMPTY_MSG);
    }
}
