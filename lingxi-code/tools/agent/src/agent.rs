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
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{AGENT_COMPLETED_M4_05, AGENT_FAILED, AGENT_STARTED};
use telemetry::AnalyticsBus;
use traits::budget::BudgetError;
use traits::subagent_spawn::{SubagentInheritance, SubagentResult, SubagentSpawnRequest};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// `Agent` — canonical tool name (claude-code `AGENT_TOOL_NAME`).
pub const AGENT_TOOL_NAME: &str = "Agent";

/// `Task` — legacy alias the dispatcher must accept (claude-code
/// `LEGACY_AGENT_TOOL_NAME`).
pub const LEGACY_AGENT_TOOL_NAME: &str = "Task";

/// Six built-in subagent types — byte-aligned with upstream
/// `claude-code/src/tools/AgentTool/built-in/*.ts`.
///
/// ADVISORY ONLY. `AgentTool` no longer rejects a `subagent_type` outside this
/// list: the catalog-aware [`traits::subagent_spawn::SubagentSpawner`] resolves
/// any type (user/project catalog overrides built-ins; an unknown type →
/// `general-purpose`, matching claude-code's `effectiveType ?? GENERAL_PURPOSE`).
/// `tool-agent` cannot depend on the `agent` crate (cycle — see the module
/// header), so the canonical definitions live there; this literal is kept for
/// tests and documentation of the built-in set, not as a gate.
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

/// Default `subagent_type` when the caller omits it — byte-aligned with
/// upstream `GENERAL_PURPOSE_AGENT.agentType` (`"general-purpose"`). See
/// `claude-code/src/tools/AgentTool/AgentTool.tsx:322`
/// (`subagent_type ?? GENERAL_PURPOSE_AGENT.agentType`) +
/// `built-in/generalPurposeAgent.ts:26` (`agentType: 'general-purpose'`).
fn default_subagent_type() -> String {
    "general-purpose".to_string()
}

/// Input shape accepted by `AgentTool`.
///
/// Mirrors claude-code's `AgentTool` Zod schema (`AgentTool.tsx:82-101` +
/// type alias `:132-138`): `description` + `prompt` required; the rest
/// optional. The `.describe()` strings are lifted verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentToolInput {
    /// `description` — REQUIRED in TS (`z.string().describe('A short (3-5
    /// word) description of the task')`, AgentTool.tsx:83).
    pub description: String,
    /// `prompt` — REQUIRED (`z.string().describe('The task for the agent to
    /// perform')`, AgentTool.tsx:84).
    pub prompt: String,
    /// `subagent_type?` — optional (`z.string().optional()`, AgentTool.tsx:85).
    /// When omitted it defaults to `"general-purpose"` (TS `subagent_type ??
    /// GENERAL_PURPOSE_AGENT.agentType`, AgentTool.tsx:322). The spawner
    /// resolves any value (no hard-reject anymore).
    #[serde(default = "default_subagent_type")]
    pub subagent_type: String,
    /// `model?` — optional model-family override `'sonnet' | 'opus' |
    /// 'haiku'` (AgentTool.tsx:86).
    #[serde(default)]
    pub model: Option<String>,
    /// `run_in_background?` — optional (AgentTool.tsx:87). Carried; background
    /// dispatch is handled by the host runtime / coordinator.
    #[serde(default)]
    pub run_in_background: Option<bool>,
    /// `name?` — optional teammate name (AgentTool.tsx:94).
    #[serde(default)]
    pub name: Option<String>,
    /// `team_name?` — optional team name (AgentTool.tsx:95).
    #[serde(default)]
    pub team_name: Option<String>,
    /// `mode?` — optional permission mode (AgentTool.tsx:96).
    #[serde(default)]
    pub mode: Option<String>,
    /// `isolation?` — optional `'worktree' | 'remote'` (AgentTool.tsx:99).
    #[serde(default)]
    pub isolation: Option<String>,
    /// `cwd?` — optional absolute path to run the agent in (AgentTool.tsx:100).
    #[serde(default)]
    pub cwd: Option<String>,
    /// INTERNAL plumbing only — NOT part of the model-facing schema (claude-code
    /// has no `context_paths` field). Kept so internal callers/tests that seed
    /// context files keep working; defaults to empty so the model never sees it.
    #[serde(default)]
    pub context_paths: Vec<PathBuf>,
}

static AGENT_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "description": {
                "type": "string",
                "description": "A short (3-5 word) description of the task"
            },
            "prompt": {
                "type": "string",
                "description": "The task for the agent to perform"
            },
            "subagent_type": {
                "type": "string",
                "description": "The type of specialized agent to use for this task"
            },
            "model": {
                "type": "string",
                "enum": ["sonnet", "opus", "haiku"],
                "description": "Optional model override for this agent. Takes precedence over the agent definition's model frontmatter. If omitted, uses the agent definition's model, or inherits from the parent."
            },
            "run_in_background": {
                "type": "boolean",
                "description": "Set to true to run this agent in the background. You will be notified when it completes."
            },
            "name": {
                "type": "string",
                "description": "Name for the spawned agent. Makes it addressable via SendMessage({to: name}) while running."
            },
            "team_name": {
                "type": "string",
                "description": "Team name for spawning. Uses current team context if omitted."
            },
            "mode": {
                "type": "string",
                "description": "Permission mode for spawned teammate (e.g., \"plan\" to require plan approval)."
            },
            "isolation": {
                "type": "string",
                "enum": ["worktree", "remote"],
                "description": "Isolation mode. \"worktree\" creates a temporary git worktree so the agent works on an isolated copy of the repo. \"remote\" launches the agent in a remote CCR environment (always runs in background)."
            },
            "cwd": {
                "type": "string",
                "description": "Absolute path to run the agent in. Overrides the working directory for all filesystem and shell operations within this agent. Mutually exclusive with isolation: \"worktree\"."
            }
        },
        "required": ["description", "prompt"]
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
        tool_api::util::ids::ulid_or_uuid()
    }

    /// Format one agent catalog line for the tool prompt, matching claude-code's
    /// `formatAgentLine` (AgentTool/prompt.ts:43-46):
    /// `- {agentType}: {whenToUse} (Tools: {toolsDescription})`. Delegates to the
    /// single source of truth in `traits` so the inline prompt path here and the
    /// `agent_listing_delta` attachment path (orchestrator) render identical
    /// lines. The `toolsDescription` is pre-rendered by the spawner (TS
    /// `getToolsDescription`).
    fn format_agent_line(agent: &traits::subagent_spawn::SubagentListingEntry) -> String {
        traits::subagent_spawn::format_agent_line(agent)
    }

    /// Build the dynamic Agent tool prompt, porting claude-code's `getPrompt`
    /// (AgentTool/prompt.ts:66-287) for the non-fork path. The catalog is
    /// embedded INLINE by default; when
    /// `traits::subagent_spawn::should_inject_agent_list_in_messages()` is ON
    /// (the `CLAUDE_CODE_AGENT_LIST_IN_MESSAGES` override, default OFF), the
    /// catalog instead moves to a per-turn `agent_listing_delta`
    /// `<system-reminder>` attachment built by the orchestrator and this prompt
    /// carries only the static pointer line (AgentTool/prompt.ts:194-199).
    ///
    /// `is_coordinator` selects the slim coordinator prompt (the coordinator
    /// system prompt already covers usage notes / examples). Not yet wired from
    /// host state — see [`Tool::prompt`].
    ///
    /// Deferred vs TS (no behavioral surface in this port): the fork-subagent
    /// branch (item 1g), the embedded-search-tools (`bfs`/`ugrep`) hint swap,
    /// the subscription / teammate gating on the concurrency + name/team/mode
    /// notes, and the `USER_TYPE === 'ant'` remote-isolation note.
    fn format_mcp_servers_note(mcp_server_names: &[String]) -> String {
        if mcp_server_names.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n# MCP Servers\n\nThe following MCP servers are available; spawned agents may have access to their tools:\n{}",
                mcp_server_names
                    .iter()
                    .map(|n| format!("- {n}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    }

    fn build_prompt(
        agents: &[traits::subagent_spawn::SubagentListingEntry],
        mcp_server_names: &[String],
        is_coordinator: bool,
    ) -> String {
        // `agent_listing_delta` gate (AgentTool/prompt.ts:194-199): when ON, the
        // catalog moves to a per-turn `<system-reminder>` attachment (built by
        // the orchestrator) and this description carries only a STATIC pointer
        // line — so the tool-schema prompt cache no longer busts every time an
        // agent loads. OFF by default (no GrowthBook in Rust), so the inline
        // catalog below is byte-identical to the pre-gate behavior.
        let agent_list_section = if traits::subagent_spawn::should_inject_agent_list_in_messages() {
            "Available agent types are listed in <system-reminder> messages in the conversation."
                .to_string()
        } else {
            let agent_lines = agents
                .iter()
                .map(Self::format_agent_line)
                .collect::<Vec<_>>()
                .join("\n");
            format!("Available agent types and the tools they have access to:\n{agent_lines}")
        };
        let mcp_note = Self::format_mcp_servers_note(mcp_server_names);

        // Shared core (TS `shared`): intro + agent list + when-to-use note.
        let shared = format!(
            "Launch a new agent to handle complex, multi-step tasks autonomously.\n\n\
The {AGENT_TOOL_NAME} tool launches specialized agents (subprocesses) that autonomously handle complex tasks. Each agent type has specific capabilities and tools available to it.\n\n\
{agent_list_section}{mcp_note}\n\n\
When using the {AGENT_TOOL_NAME} tool, specify a subagent_type parameter to select which agent type to use. If omitted, the general-purpose agent is used."
        );

        // Coordinator mode gets the slim prompt (TS: `if (isCoordinator) return shared`).
        if is_coordinator {
            return shared;
        }

        // Non-coordinator: full prompt with when-not-to-use + usage notes +
        // examples (TS non-coordinator return, AgentTool/prompt.ts:252-286,
        // non-embedded-search-tools branch).
        let when_not_to_use = format!(
            "\nWhen NOT to use the {AGENT_TOOL_NAME} tool:\n\
- If you want to read a specific file path, use the Read tool or the Glob tool instead of the {AGENT_TOOL_NAME} tool, to find the match more quickly\n\
- If you are searching for a specific class definition like \"class Foo\", use the Glob tool instead, to find the match more quickly\n\
- If you are searching for code within a specific file or set of 2-3 files, use the Read tool instead of the {AGENT_TOOL_NAME} tool, to find the match more quickly\n\
- Other tasks that are not related to the agent descriptions above\n"
        );

        let examples = format!(
            "Example usage:\n\n\
<example_agent_descriptions>\n\
\"test-runner\": use this agent after you are done writing code to run tests\n\
\"greeting-responder\": use this agent to respond to user greetings with a friendly joke\n\
</example_agent_descriptions>\n\n\
<example>\n\
user: \"Please write a function that checks if a number is prime\"\n\
assistant: I'm going to use the Write tool to write the following code:\n\
<code>\n\
function isPrime(n) {{\n\
  if (n <= 1) return false\n\
  for (let i = 2; i * i <= n; i++) {{\n\
    if (n % i === 0) return false\n\
  }}\n\
  return true\n\
}}\n\
</code>\n\
<commentary>\n\
Since a significant piece of code was written and the task was completed, now use the test-runner agent to run the tests\n\
</commentary>\n\
assistant: Uses the {AGENT_TOOL_NAME} tool to launch the test-runner agent\n\
</example>\n\n\
<example>\n\
user: \"Hello\"\n\
<commentary>\n\
Since the user is greeting, use the greeting-responder agent to respond with a friendly joke\n\
</commentary>\n\
assistant: \"I'm going to use the {AGENT_TOOL_NAME} tool to launch the greeting-responder agent\"\n\
</example>\n"
        );

        format!(
            "{shared}\n\
{when_not_to_use}\n\n\
Usage notes:\n\
- Always include a short description (3-5 words) summarizing what the agent will do\n\
- Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses\n\
- When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.\n\
- You can optionally run agents in the background using the run_in_background parameter. When an agent runs in the background, you will be automatically notified when it completes — do NOT sleep, poll, or proactively check on its progress. Continue with other work or respond to the user instead.\n\
- **Foreground vs background**: Use foreground (default) when you need the agent's results before you can proceed — e.g., research agents whose findings inform your next steps. Use background when you have genuinely independent work to do in parallel.\n\
- To continue a previously spawned agent, use SendMessage with the agent's ID or name as the `to` field. The agent resumes with its full context preserved. Each Agent invocation starts fresh — provide a complete task description.\n\
- The agent's outputs should generally be trusted\n\
- Clearly tell the agent whether you expect it to write code or just to do research (search, file reads, web fetches, etc.), since it is not aware of the user's intent\n\
- If the agent description mentions that it should be used proactively, then you should try your best to use it without the user having to ask for it first. Use your judgement.\n\
- If the user specifies that they want you to run agents \"in parallel\", you MUST send a single message with multiple {AGENT_TOOL_NAME} tool use content blocks. For example, if you need to launch both a build-validator agent and a test-runner agent in parallel, send a single message with both tool calls.\n\
- You can optionally set `isolation: \"worktree\"` to run the agent in a temporary git worktree, giving it an isolated copy of the repository. The worktree is automatically cleaned up if the agent makes no changes; if changes are made, the worktree path and branch are returned in the result.\n\n\
{examples}"
        )
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
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
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
        // claude-code builds the Agent tool prompt dynamically (getPrompt,
        // AgentTool/prompt.ts:66-287): it injects the live agent catalog (one
        // `formatAgentLine` per resolved AgentDefinition) and the available MCP
        // server names. Pull the catalog from the spawner (defaulted-empty when
        // unwired) and the MCP server names from the registry.
        let agents = match &self.ctx.subagent_spawner {
            Some(s) => s.agent_listing().await,
            None => Vec::new(),
        };
        let mcp_server_names: Vec<String> = match &self.ctx.mcp_registry {
            Some(reg) => reg
                .snapshot()
                .await
                .into_iter()
                .map(|info| info.name)
                .collect(),
            None => Vec::new(),
        };
        // The coordinator-mode signal (TS `isCoordinatorMode()`) is not yet
        // threaded onto `BuiltinToolContext` / `PromptOptions`, so the slim
        // coordinator branch is structurally ported but driven by `false`
        // (the full prompt) for now — see `build_prompt`.
        Self::build_prompt(&agents, &mcp_server_names, false)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
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

        // 2. Resolve the subagent type via the catalog-aware spawner — do NOT
        // hard-reject unknown types. claude-code accepts any `subagent_type`:
        // the catalog (user/project agents) overrides built-ins, and an unknown
        // type falls back to `general-purpose` (`effectiveType ?? GENERAL_
        // PURPOSE_AGENT`, AgentTool.tsx:322). That resolution lives in
        // `PoolSubagentSpawner::lookup_definition`; the former static 6-type
        // gate here made user/project agents + the unknown→general-purpose
        // fallback unreachable through the tool, so it is removed.

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

        // 4. Wiring guard: spawner + budget + registry must be present.
        let spawner = self.ctx.subagent_spawner.clone().ok_or_else(|| {
            ToolError::Internal(
                "AgentTool: SubagentSpawner not wired into BuiltinToolContext".into(),
            )
        })?;
        let budget = self.ctx.budget_enforcer.clone().ok_or_else(|| {
            ToolError::Internal(
                "AgentTool: BudgetEnforcerHandle not wired into BuiltinToolContext".into(),
            )
        })?;
        let parent_registry = ctx.subagent_registry.clone().ok_or_else(|| {
            ToolError::Internal(
                "AgentTool: parent ToolRegistry not threaded via ToolUseContext.subagent_registry"
                    .into(),
            )
        })?;

        // 5. M3-05 byte-locked budget gate. `check_and_charge(0)` re-runs the
        // pre-call gate; on Exceeded we surface the locked denial string.
        if let Err(BudgetError::Exceeded { current_nano_usd }) = budget.check_and_charge(0).await {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "budget_exceeded",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::Internal(format_budget_denied(current_nano_usd)));
        }

        // 6. Emit started.
        Self::emit_started(
            &bus,
            &invocation_id,
            &parsed.subagent_type,
            parsed.prompt.chars().count(),
        )
        .await;

        // 7. Build the inheritance bundle and dispatch into the spawner.
        // The recursion-lock invariant: `parent_registry` is passed verbatim
        // into the bundle via `RegistryToolInvoker::new(parent_registry)`.
        // The budget Arc is cloned (no deep clone — `Arc::clone` only bumps
        // the refcount), so `Arc::ptr_eq` between parent + child holds.
        // (3b) Gate the subagent's tool dispatch with the same permission gate
        // the main loop uses, when one is wired. `None` → unconditional dispatch
        // (legacy). The gate rides into the spawner via the inheritance bundle.
        let mut invoker_impl =
            tool_api::tool_invoker_impl::RegistryToolInvoker::new(parent_registry.clone());
        if let Some(gate) = self.ctx.permission_gate.clone() {
            invoker_impl = invoker_impl.with_gate(gate);
        }
        let invoker: Arc<dyn traits::tool_invoker::ToolInvoker> = Arc::new(invoker_impl);
        let inherit = SubagentInheritance {
            tool_invoker: invoker,
            budget: budget.clone(),
        };
        let request = SubagentSpawnRequest {
            subagent_type: parsed.subagent_type.clone(),
            prompt: parsed.prompt.clone(),
            context_paths: parsed.context_paths.clone(),
            // AgentTool spawn-surface parity: thread the new params through.
            // `model` is mapped to the agent model override by the spawner; the
            // rest are carried with their behavior deferred (teammate routing /
            // worktree-remote isolation / cwd override land with later batches).
            description: Some(parsed.description.clone()),
            model: parsed.model.clone(),
            name: parsed.name.clone(),
            team_name: parsed.team_name.clone(),
            mode: parsed.mode.clone(),
            isolation: parsed.isolation.clone(),
            cwd: parsed.cwd.clone(),
        };

        let outcome = spawner.spawn(request, inherit).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match outcome {
            Ok(SubagentResult::Completed { content, .. }) => {
                Self::emit_completed(&bus, &invocation_id, duration_ms, &parsed.subagent_type)
                    .await;
                Ok(ToolCallResult {
                    data: json!({
                        "subagent_type": parsed.subagent_type,
                        "result": content,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Ok(SubagentResult::Failed { reason }) => {
                Self::emit_failed(&bus, &invocation_id, "subagent_failed", duration_ms).await;
                Err(ToolError::Internal(reason))
            }
            Ok(SubagentResult::Killed) => {
                Self::emit_failed(&bus, &invocation_id, "killed", duration_ms).await;
                Err(ToolError::Internal("Agent: subagent was killed".into()))
            }
            Err(e) => {
                Self::emit_failed(&bus, &invocation_id, "spawn_error", duration_ms).await;
                Err(ToolError::Internal(format!("Agent: spawner failed: {e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_test_support::{
        arc_mock_budget, arc_mock_mailbox, arc_mock_spawner, arc_mock_task_registry,
        MockBudgetEnforcerHandle, MockSubagentSpawner,
    };
    use std::path::PathBuf;
    use telemetry::AnalyticsBus;
    use tool_api::context::{ToolUseContext, ToolUseOptions};
    use tool_api::test_support::{ctx_for_file_tools, fresh_tx, make_dummy_fs};
    use tool_api::ToolRegistry;
    use traits::budget::BudgetEnforcerHandle;
    use traits::subagent_spawn::SubagentSpawner;

    /// `CLAUDE_CODE_AGENT_LIST_IN_MESSAGES` is process-global; serialize the
    /// tests whose `build_prompt`/`prompt` output depends on the
    /// `should_inject_agent_list_in_messages()` gate so a gate-ON test never
    /// races a default-OFF test. Every such test acquires this AND removes the
    /// var first, neutralizing ordering (mirrors `tools/shell/src/prompt.rs`).
    static AGENT_LIST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Build a `BuiltinToolContext` wired with all four M4-05 mocks.
    fn wired_ctx(
        spawner: Arc<MockSubagentSpawner>,
        registry: Arc<crate::agent_test_support::MockTaskRegistryHandle>,
        mailbox: Arc<crate::agent_test_support::MockMailboxRouterHandle>,
        budget: Arc<MockBudgetEnforcerHandle>,
    ) -> BuiltinToolContext {
        let mut bctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        );
        bctx.subagent_spawner = Some(spawner.clone() as Arc<dyn SubagentSpawner>);
        bctx.task_registry = Some(registry as Arc<dyn traits::task_registry::TaskRegistryHandle>);
        bctx.mailbox_router = Some(mailbox as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        bctx.budget_enforcer = Some(budget.clone() as Arc<dyn BudgetEnforcerHandle>);
        bctx
    }

    fn fresh_ctx_with_registry(registry: Arc<ToolRegistry>) -> ToolUseContext {
        ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: "test".into(),
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: None,
            agent_name: None,
            team_name: None,
            content_replacement_state: None,
            session: None,
            subagent_registry: Some(registry),
            cancel: None,
        }
    }

    // =====================================================================
    // CRITICAL TEST 1 — recursion-lock: AgentTool passes parent's
    // Arc<ToolRegistry> verbatim into the child via SubagentInheritance.
    // The mock spawner captures the inheritance bundle so we can read back
    // the Arc<dyn ToolInvoker> and pull the inner Arc<ToolRegistry> out.
    // =====================================================================
    #[tokio::test]
    async fn recursion_lock_child_inherits_parent_tool_registry_arc() {
        let parent_registry = Arc::new(ToolRegistry::new());
        let spawner = arc_mock_spawner();
        let budget = arc_mock_budget(u64::MAX);
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            budget,
        );

        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(parent_registry.clone());
        let input = serde_json::json!({
            "description": "say hi",
            "subagent_type": "general-purpose",
            "prompt": "hi"
        });
        tool.call(input, ctx, fresh_tx())
            .await
            .expect("spawner returns Completed by default");

        let invocations = spawner.invocations();
        assert_eq!(invocations.len(), 1, "exactly one spawn call");
        let captured_invoker = &invocations[0].inherit.tool_invoker;
        // Downcast to RegistryToolInvoker via Arc::downcast on the concrete
        // type. Since we can't downcast Arc<dyn>, we instead introspect
        // through the public accessor on our concrete wrapper. The
        // production wrapper preserves the Arc<ToolRegistry> verbatim, so
        // we reach for it via the test-only seam.
        // SAFETY: the mock spawner returns the exact Arc the production
        // AgentTool::call passed; we wrap parent_registry in a fresh
        // RegistryToolInvoker on the call path, so the trait-object pointer
        // is unique to this invocation. We compare the inner Arcs.
        let captured = (**captured_invoker)
            .as_any()
            .downcast_ref::<tool_api::tool_invoker_impl::RegistryToolInvoker>()
            .map(|i| i.registry_arc().clone());
        assert!(
            captured.is_some(),
            "captured invoker must be a RegistryToolInvoker"
        );
        assert!(
            Arc::ptr_eq(&parent_registry, captured.as_ref().unwrap()),
            "recursion lock: child must inherit parent's Arc<ToolRegistry> verbatim"
        );
    }

    // =====================================================================
    // CRITICAL TEST 2 — budget inheritance: AgentTool passes parent's
    // Arc<dyn BudgetEnforcerHandle> verbatim into the child via
    // SubagentInheritance. Arc::ptr_eq on the trait-object Arc holds.
    // =====================================================================
    #[tokio::test]
    async fn budget_inheritance_child_inherits_parent_budget_arc() {
        let parent_budget: Arc<dyn BudgetEnforcerHandle> =
            Arc::new(MockBudgetEnforcerHandle::new(u64::MAX));
        let spawner = arc_mock_spawner();

        let mut bctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        );
        bctx.subagent_spawner = Some(spawner.clone() as Arc<dyn SubagentSpawner>);
        bctx.task_registry =
            Some(arc_mock_task_registry() as Arc<dyn traits::task_registry::TaskRegistryHandle>);
        bctx.mailbox_router =
            Some(arc_mock_mailbox() as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        bctx.budget_enforcer = Some(parent_budget.clone());

        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = serde_json::json!({
            "description": "say hi",
            "subagent_type": "general-purpose",
            "prompt": "hi"
        });
        tool.call(input, ctx, fresh_tx()).await.unwrap();

        let invocations = spawner.invocations();
        assert_eq!(invocations.len(), 1);
        let captured = &invocations[0].inherit.budget;
        assert!(
            Arc::ptr_eq(&parent_budget, captured),
            "budget inheritance: child must inherit parent's Arc<dyn BudgetEnforcerHandle> verbatim"
        );
    }

    // =====================================================================
    // M3-05 byte-locked denial format flows through Budget -> AgentTool.
    // =====================================================================
    #[tokio::test]
    async fn budget_exceeded_yields_m3_05_byte_locked_denial_string() {
        let budget = Arc::new(MockBudgetEnforcerHandle::new(1_500_000_000)); // $1.50 cap
        budget.set_total(2_000_000_000); // $2.00 already spent

        let spawner = arc_mock_spawner();
        let mut bctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        );
        bctx.subagent_spawner = Some(spawner.clone() as Arc<dyn SubagentSpawner>);
        bctx.task_registry =
            Some(arc_mock_task_registry() as Arc<dyn traits::task_registry::TaskRegistryHandle>);
        bctx.mailbox_router =
            Some(arc_mock_mailbox() as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        bctx.budget_enforcer = Some(budget.clone() as Arc<dyn BudgetEnforcerHandle>);

        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = serde_json::json!({
            "description": "do work",
            "subagent_type": "Plan",
            "prompt": "do work"
        });
        let err = tool
            .call(input, ctx, fresh_tx())
            .await
            .expect_err("budget gate must trip and surface denial");
        let msg = format!("{err}");
        // M3-05 byte-locked: "Budget exceeded ($X.YZ); stopped."
        assert!(
            msg.contains(SUBAGENT_BUDGET_DENIED_PREFIX),
            "denial msg must start with M3-05 byte-locked prefix: {msg}"
        );
        assert!(msg.contains("); stopped."));
        // Spawner must NOT have been called.
        assert!(
            spawner.invocations().is_empty(),
            "spawner must not be invoked once budget gate trips"
        );
    }

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
        // `description` + `prompt` are the required fields (TS schema). The
        // optional params round-trip; `context_paths` is internal-only.
        let v = json!({
            "description": "explore repo",
            "subagent_type": "general-purpose",
            "prompt": "Explore the repo structure.",
            "model": "haiku",
            "run_in_background": true,
            "isolation": "worktree"
        });
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.description, "explore repo");
        assert_eq!(parsed.subagent_type, "general-purpose");
        assert_eq!(parsed.prompt, "Explore the repo structure.");
        assert_eq!(parsed.model.as_deref(), Some("haiku"));
        assert_eq!(parsed.run_in_background, Some(true));
        assert_eq!(parsed.isolation.as_deref(), Some("worktree"));
        // Internal-only plumbing defaults to empty when omitted.
        assert!(parsed.context_paths.is_empty());
    }

    // `description` is REQUIRED (TS `z.string()`, not `.optional()`); omitting
    // it is a parse error surfaced as InvalidInput by the call path.
    #[test]
    fn agent_input_missing_description_is_rejected() {
        let v = json!({ "prompt": "Design." });
        let parsed: Result<AgentToolInput, _> = serde_json::from_value(v);
        assert!(parsed.is_err(), "description is required");
    }

    #[test]
    fn agent_input_optional_fields_default_to_none_and_empty() {
        let v = json!({"description": "d", "subagent_type": "Plan", "prompt": "Design."});
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert!(parsed.model.is_none());
        assert!(parsed.run_in_background.is_none());
        assert!(parsed.name.is_none());
        assert!(parsed.team_name.is_none());
        assert!(parsed.mode.is_none());
        assert!(parsed.isolation.is_none());
        assert!(parsed.cwd.is_none());
        assert!(parsed.context_paths.is_empty());
    }

    // AGENT.1 — omitting `subagent_type` defaults to "general-purpose",
    // matching TS `subagent_type ?? GENERAL_PURPOSE_AGENT.agentType`
    // (AgentTool.tsx:85 optional + :322 default).
    #[test]
    fn agent_input_defaults_subagent_type_to_general_purpose() {
        let v = json!({ "description": "d", "prompt": "Explore the repo." });
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.subagent_type, "general-purpose");
        // The default is one of the six known built-in types.
        assert!(BUILTIN_SUBAGENT_TYPES.contains(&parsed.subagent_type.as_str()));
    }

    // The advertised schema requires `description` + `prompt` (TS schema), and
    // exposes NO `context_paths` field to the model.
    #[test]
    fn agent_schema_requires_description_and_prompt() {
        let required = AGENT_INPUT_SCHEMA["required"]
            .as_array()
            .expect("required is an array");
        assert_eq!(required, &[json!("description"), json!("prompt")]);
        let props = AGENT_INPUT_SCHEMA["properties"]
            .as_object()
            .expect("properties is an object");
        assert!(props.contains_key("description"));
        assert!(props.contains_key("prompt"));
        // model-facing schema must NOT expose context_paths (removed; TS has no
        // such field) but must expose the new optional params.
        assert!(!props.contains_key("context_paths"));
        for k in ["model", "run_in_background", "name", "team_name", "mode", "isolation", "cwd"] {
            assert!(props.contains_key(k), "schema exposes {k}");
        }
        // `model` + `isolation` carry the TS enum constraint.
        assert_eq!(
            AGENT_INPUT_SCHEMA["properties"]["model"]["enum"],
            json!(["sonnet", "opus", "haiku"])
        );
        assert_eq!(
            AGENT_INPUT_SCHEMA["properties"]["isolation"]["enum"],
            json!(["worktree", "remote"])
        );
        // Verbatim `.describe()` text on a representative field.
        assert_eq!(
            AGENT_INPUT_SCHEMA["properties"]["description"]["description"],
            json!("A short (3-5 word) description of the task")
        );
    }

    // Headline-bug fix: an unknown `subagent_type` is ACCEPTED (no hard-reject)
    // and resolves through the catalog-aware spawner (unknown → general-purpose
    // happens inside the spawner). The tool surface must not error on it.
    #[tokio::test]
    async fn unknown_subagent_type_is_accepted_and_dispatched() {
        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = serde_json::json!({
            "description": "custom agent",
            "subagent_type": "my-custom-project-agent",
            "prompt": "do it"
        });
        // No InvalidInput error — the former static 6-type gate is gone.
        tool.call(input, ctx, fresh_tx())
            .await
            .expect("unknown subagent_type must be accepted, not rejected");
        let invocations = spawner.invocations();
        assert_eq!(invocations.len(), 1, "spawner is invoked for unknown type");
        assert_eq!(invocations[0].request.subagent_type, "my-custom-project-agent");
    }

    // The new params thread into the spawn request.
    #[tokio::test]
    async fn spawn_request_carries_new_parity_params() {
        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = serde_json::json!({
            "description": "desc here",
            "subagent_type": "general-purpose",
            "prompt": "go",
            "model": "opus",
            "name": "scout",
            "team_name": "alpha",
            "mode": "plan",
            "isolation": "worktree",
            "cwd": "/work"
        });
        tool.call(input, ctx, fresh_tx()).await.unwrap();
        let req = &spawner.invocations()[0].request;
        assert_eq!(req.description.as_deref(), Some("desc here"));
        assert_eq!(req.model.as_deref(), Some("opus"));
        assert_eq!(req.name.as_deref(), Some("scout"));
        assert_eq!(req.team_name.as_deref(), Some("alpha"));
        assert_eq!(req.mode.as_deref(), Some("plan"));
        assert_eq!(req.isolation.as_deref(), Some("worktree"));
        assert_eq!(req.cwd.as_deref(), Some("/work"));
    }

    // Dynamic prompt: catalog lines (formatAgentLine) appear, sourced from the
    // spawner's `agent_listing`. The mock spawner surfaces two entries.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // brief; serializes the gate env var
    async fn prompt_injects_dynamic_agent_catalog_lines() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let prompt = tool
            .prompt(&PromptOptions {
                include_examples: true,
            })
            .await;
        assert!(prompt.contains("Available agent types and the tools they have access to:"));
        // formatAgentLine: `- {type}: {whenToUse} (Tools: {tools})`.
        assert!(
            prompt.contains("- general-purpose: use for anything (Tools: All tools)"),
            "catalog line missing; prompt was:\n{prompt}"
        );
        assert!(prompt.contains("- Explore: search (Tools: All tools except Edit)"));
        // Core structural anchors from getPrompt.
        assert!(prompt.contains("Launch a new agent to handle complex, multi-step tasks"));
        assert!(prompt.contains("If omitted, the general-purpose agent is used."));
    }

    // build_prompt's coordinator branch returns the slim shared prompt only
    // (no "Usage notes:" / examples), matching TS `if (isCoordinator) return shared`.
    #[test]
    fn build_prompt_coordinator_branch_is_slim() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
        let agents = vec![traits::subagent_spawn::SubagentListingEntry {
            agent_type: "general-purpose".into(),
            when_to_use: "anything".into(),
            tools_description: "All tools".into(),
        }];
        let full = AgentTool::build_prompt(&agents, &[], false);
        let slim = AgentTool::build_prompt(&agents, &[], true);
        assert!(full.contains("Usage notes:"));
        assert!(!slim.contains("Usage notes:"));
        // Both carry the agent catalog.
        assert!(slim.contains("- general-purpose: anything (Tools: All tools)"));
    }

    // MCP server names surface in the prompt when the registry exposes them.
    #[test]
    fn build_prompt_lists_mcp_servers_when_present() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
        let agents = vec![traits::subagent_spawn::SubagentListingEntry {
            agent_type: "general-purpose".into(),
            when_to_use: "anything".into(),
            tools_description: "All tools".into(),
        }];
        let p = AgentTool::build_prompt(&agents, &["github".into(), "linear".into()], false);
        assert!(p.contains("# MCP Servers"));
        assert!(p.contains("- github"));
        assert!(p.contains("- linear"));
    }

    // `agent_listing_delta` gate ON (AgentTool/prompt.ts:194-199): the inline
    // catalog is replaced by the static pointer line, and the per-agent
    // `formatAgentLine` lines are NOT in the description (they move to the
    // orchestrator's per-turn `<system-reminder>` attachment).
    #[test]
    fn build_prompt_gate_on_emits_static_pointer_line_not_inline_catalog() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES", "1");

        let agents = vec![traits::subagent_spawn::SubagentListingEntry {
            agent_type: "general-purpose".into(),
            when_to_use: "anything".into(),
            tools_description: "All tools".into(),
        }];
        let p = AgentTool::build_prompt(&agents, &[], false);

        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");

        assert!(
            p.contains(
                "Available agent types are listed in <system-reminder> messages in the conversation."
            ),
            "gate-ON prompt must carry the static pointer line; was:\n{p}"
        );
        // The inline catalog header + the per-agent line must be ABSENT.
        assert!(
            !p.contains("Available agent types and the tools they have access to:"),
            "gate-ON prompt must NOT carry the inline catalog header"
        );
        assert!(
            !p.contains("- general-purpose: anything (Tools: All tools)"),
            "gate-ON prompt must NOT carry inline formatAgentLine lines"
        );
        // The rest of the prompt scaffold is unchanged.
        assert!(p.contains("Launch a new agent to handle complex, multi-step tasks"));
        assert!(p.contains("Usage notes:"));
    }
}
