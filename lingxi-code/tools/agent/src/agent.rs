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
use telemetry::tengu::agent::{CACHE_EVICTION_HINT, TOOL_COMPLETED, TOOL_SELECTED, TOOL_TERMINATED};
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

/// Built-in agents that run once and return a report — the parent never
/// `SendMessage`s back to continue them. claude-code `ONE_SHOT_BUILTIN_AGENT_TYPES`
/// (`constants.ts:9-12`). The model-facing `tool_result` for these skips the
/// `agentId`/SendMessage/`<usage>` trailer (`AgentTool.tsx:1356`), saving tokens.
pub const ONE_SHOT_BUILTIN_AGENT_TYPES: &[&str] = &["Explore", "Plan"];

/// claude `maxResultSizeChars` for the Agent tool (`AgentTool.tsx:229`): a flat
/// `100_000`, NOT the shared `MAX_TOOL_OUTPUT_LENGTH`.
const AGENT_MAX_RESULT_SIZE_CHARS: usize = 100_000;

/// `general-purpose` — byte-aligned with upstream `GENERAL_PURPOSE_AGENT.agentType`
/// (`built-in/generalPurposeAgent.ts:26`). The effective type when the caller
/// OMITS `subagent_type` (claude `subagent_type ?? GENERAL_PURPOSE_AGENT.agentType`,
/// AgentTool.tsx:322).
const GENERAL_PURPOSE_AGENT_TYPE: &str = "general-purpose";

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
    /// `None` (omitted) ⇒ the effective type is `general-purpose` (TS
    /// `subagent_type ?? GENERAL_PURPOSE_AGENT.agentType`, AgentTool.tsx:322).
    /// `Some(x)` (explicit) is validated against the agent listing in `call`: a
    /// known type spawns it, an unknown type is rejected with claude's
    /// "Agent type 'x' not found" error (AgentTool.tsx:345-354). The `Option`
    /// distinguishes omitted (→ general-purpose) from explicit (→ validated),
    /// exactly as claude's `?? ` does.
    #[serde(default)]
    pub subagent_type: Option<String>,
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
                "enum": ["sonnet", "opus", "haiku", "fable"],
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
                "enum": ["acceptEdits", "auto", "bypassPermissions", "default", "dontAsk", "plan"],
                "description": "Permission mode for spawned teammate (e.g., \"plan\" to require plan approval)."
            },
            "isolation": {
                "type": "string",
                "enum": ["worktree", "remote"],
                "description": "Isolation mode. \"worktree\" creates a temporary git worktree so the agent works on an isolated copy of the repo. \"remote\" launches the agent in a remote cloud environment (always runs in background; availability is gated)."
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

/// Extract claude's `content: [{type:'text', text}]` array from a subagent's
/// terminal result JSON.
///
/// The runner builds `result.content` as the agent's final text blocks (claude
/// `finalizeAgentTool`, agentToolUtils.ts) on the clean-stop path. Other terminal
/// results (max-turns / stub `{reason}`) carry no `content` key → empty array,
/// which the caller renders as the no-output marker. Returns each block's `text`.
fn extract_content_texts(result: &Value) -> Vec<String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|b| {
                    if b.get("type").and_then(Value::as_str) == Some("text") {
                        b.get("text").and_then(Value::as_str).map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Render the model-facing `tool_result` text for a COMPLETED subagent, byte-for-byte
/// per claude-code `mapToolResultToToolResultBlockParam` (AgentTool.tsx:1340-1373).
///
/// - Empty content → the single no-output marker
///   `(Subagent completed but returned no output.)` (AgentTool.tsx:1347-1350).
/// - One-shot built-ins (`Explore` / `Plan`) with NO worktree → content texts ONLY,
///   no trailer (AgentTool.tsx:1356-1362).
/// - Otherwise → content texts + the `agentId`/SendMessage hint (+ worktree fields)
///   + the `<usage>` block (AgentTool.tsx:1363-1373).
///
/// claude joins the `tool_result` content blocks with `\n` when collapsing to a
/// single string; LingXi's `model_content` IS that single string, so the text
/// blocks (and the trailer block) are `\n`-joined here.
fn render_completed_model_content(
    content_texts: &[String],
    agent_id: &str,
    agent_type: &str,
    total_tokens: u64,
    total_tool_use_count: u64,
    total_duration_ms: u64,
) -> String {
    // contentOrMarker (AgentTool.tsx:1347-1350).
    let content_or_marker: Vec<String> = if content_texts.is_empty() {
        vec!["(Subagent completed but returned no output.)".to_string()]
    } else {
        content_texts.to_vec()
    };

    // One-shot built-ins skip the trailer when there is no worktree info
    // (AgentTool.tsx:1356). Worktree fields are not wired in LingXi → always "".
    let worktree_info_text = String::new();
    let is_one_shot = ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&agent_type);
    if is_one_shot && worktree_info_text.is_empty() {
        return content_or_marker.join("\n");
    }

    // Trailer (AgentTool.tsx:1366-1372): agentId/SendMessage hint + worktree +
    // <usage> block, appended after the content blocks. v2.1.185 renamed the
    // token key `total_tokens` → `subagent_tokens` (binary off 202120031;
    // confirmed live in subagent result trailers); the leaked TS still shows the
    // old `total_tokens`, so the binary is canonical here.
    let trailer = format!(
        "agentId: {agent_id} (use SendMessage with to: '{agent_id}' to continue this agent){worktree_info_text}\n<usage>subagent_tokens: {total_tokens}\ntool_uses: {total_tool_use_count}\nduration_ms: {total_duration_ms}</usage>"
    );
    let mut blocks = content_or_marker;
    blocks.push(trailer);
    blocks.join("\n")
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

    /// (G11) Emit claude `tengu_agent_tool_selected` (AgentTool.tsx:419-428):
    /// the resolved agent + model + source + color + the resume/async/fork flags,
    /// fired AFTER selection resolution and BEFORE dispatch.
    #[allow(clippy::too_many_arguments)]
    async fn emit_agent_tool_selected(
        bus: &Arc<AnalyticsBus>,
        agent_type: &str,
        model: &str,
        source: &str,
        color: Option<&str>,
        is_built_in_agent: bool,
        is_resume: bool,
        is_async: bool,
        is_fork: bool,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "agent_type".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(agent_type.to_string()).into_inner(),
            ),
        );
        md.insert(
            "model".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(model.to_string()).into_inner(),
            ),
        );
        md.insert(
            "source".into(),
            AnalyticsValue::String(Verified::assert_safe(source.to_string()).into_inner()),
        );
        // claude `color: selectedAgent.color` — omitted (not zero-faked) when
        // the agent declares no color.
        if let Some(c) = color {
            md.insert(
                "color".into(),
                AnalyticsValue::String(Verified::assert_safe(c.to_string()).into_inner()),
            );
        }
        md.insert(
            "is_built_in_agent".into(),
            AnalyticsValue::Bool(is_built_in_agent),
        );
        md.insert("is_resume".into(), AnalyticsValue::Bool(is_resume));
        md.insert("is_async".into(), AnalyticsValue::Bool(is_async));
        md.insert("is_fork".into(), AnalyticsValue::Bool(is_fork));
        bus.log_event(TOOL_SELECTED, md).await;
    }

    /// (G11) Emit claude `tengu_agent_tool_completed` (agentToolUtils.ts:322-335).
    #[allow(clippy::too_many_arguments)]
    async fn emit_agent_tool_completed(
        bus: &Arc<AnalyticsBus>,
        agent_type: &str,
        model: &str,
        prompt_char_count: u64,
        response_char_count: u64,
        assistant_message_count: u64,
        total_tool_uses: u64,
        duration_ms: u64,
        total_tokens: u64,
        is_built_in_agent: bool,
        is_async: bool,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "agent_type".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(agent_type.to_string()).into_inner(),
            ),
        );
        md.insert(
            "model".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(model.to_string()).into_inner(),
            ),
        );
        md.insert(
            "prompt_char_count".into(),
            AnalyticsValue::Int(prompt_char_count as i64),
        );
        md.insert(
            "response_char_count".into(),
            AnalyticsValue::Int(response_char_count as i64),
        );
        md.insert(
            "assistant_message_count".into(),
            AnalyticsValue::Int(assistant_message_count as i64),
        );
        md.insert(
            "total_tool_uses".into(),
            AnalyticsValue::Int(total_tool_uses as i64),
        );
        md.insert("duration_ms".into(), AnalyticsValue::Int(duration_ms as i64));
        md.insert(
            "total_tokens".into(),
            AnalyticsValue::Int(total_tokens as i64),
        );
        md.insert(
            "is_built_in_agent".into(),
            AnalyticsValue::Bool(is_built_in_agent),
        );
        md.insert("is_async".into(), AnalyticsValue::Bool(is_async));
        bus.log_event(TOOL_COMPLETED, md).await;
    }

    /// (G11) Emit claude `tengu_cache_eviction_hint` (agentToolUtils.ts:340-345):
    /// `scope:'subagent_end'` + the final turn's `last_request_id`.
    async fn emit_cache_eviction_hint(bus: &Arc<AnalyticsBus>, last_request_id: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "scope".into(),
            AnalyticsValue::String(Verified::assert_safe("subagent_end".to_string()).into_inner()),
        );
        md.insert(
            "last_request_id".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(last_request_id.to_string()).into_inner(),
            ),
        );
        bus.log_event(CACHE_EVICTION_HINT, md).await;
    }

    /// (G11) Emit claude `tengu_agent_tool_terminated` (agentToolUtils.ts:646-656):
    /// an ASYNC subagent killed by the user (`reason:'user_kill_async'`). Fired by
    /// the async lifecycle on kill; not reached on the sync path.
    #[allow(dead_code)]
    async fn emit_agent_tool_terminated(
        bus: &Arc<AnalyticsBus>,
        agent_type: &str,
        model: &str,
        duration_ms: u64,
        is_built_in_agent: bool,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "agent_type".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(agent_type.to_string()).into_inner(),
            ),
        );
        md.insert(
            "model".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(model.to_string()).into_inner(),
            ),
        );
        md.insert("duration_ms".into(), AnalyticsValue::Int(duration_ms as i64));
        md.insert("is_async".into(), AnalyticsValue::Bool(true));
        md.insert(
            "is_built_in_agent".into(),
            AnalyticsValue::Bool(is_built_in_agent),
        );
        md.insert(
            "reason".into(),
            AnalyticsValue::String(
                Verified::assert_safe("user_kill_async".to_string()).into_inner(),
            ),
        );
        bus.log_event(TOOL_TERMINATED, md).await;
    }

    /// PLANNED #2/G13 — async (`run_in_background`) dispatch.
    ///
    /// claude returns the `async_launched` payload immediately and drives the
    /// lifecycle detached (AgentTool.tsx:686-764). This calls the
    /// [`traits::subagent_spawn::SubagentSpawner::spawn_async`] seam, which
    /// DEFAULTS to a clear error when unwired — so an unwired async branch
    /// surfaces an explicit message rather than silently running synchronously
    /// (the task forbids a silent wrong path). When the production spawner
    /// overrides `spawn_async`, this returns claude's `async_launched` JSON and
    /// registers `name → agentId` (G14, async-only, AgentTool.tsx:703-712).
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_async(
        &self,
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        started: Instant,
        spawner: &dyn traits::subagent_spawn::SubagentSpawner,
        parsed: &AgentToolInput,
        effective_type: &str,
        _selected: &traits::subagent_spawn::SelectedAgentMeta,
        is_fork: bool,
        ctx: &ToolUseContext,
        budget: Arc<dyn traits::budget::BudgetEnforcerHandle>,
        parent_registry: Arc<tool_api::ToolRegistry>,
    ) -> Result<ToolCallResult, ToolError> {
        let mut invoker_impl =
            tool_api::tool_invoker_impl::RegistryToolInvoker::new(parent_registry);
        if let Some(gate) = self.ctx.permission_gate.clone() {
            invoker_impl = invoker_impl.with_gate(gate);
        }
        let invoker: Arc<dyn traits::tool_invoker::ToolInvoker> = Arc::new(invoker_impl);
        let inherit = SubagentInheritance {
            tool_invoker: invoker,
            budget,
        };
        let request = SubagentSpawnRequest {
            subagent_type: effective_type.to_string(),
            prompt: parsed.prompt.clone(),
            context_paths: parsed.context_paths.clone(),
            description: Some(parsed.description.clone()),
            model: if is_fork { None } else { parsed.model.clone() },
            run_in_background: true,
            name: if is_fork { None } else { parsed.name.clone() },
            team_name: if is_fork { None } else { parsed.team_name.clone() },
            mode: if is_fork { None } else { parsed.mode.clone() },
            isolation: if is_fork { None } else { parsed.isolation.clone() },
            cwd: if is_fork { None } else { parsed.cwd.clone() },
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            // The Agent (Task) tool has no structured-output schema param.
            schema: None,
            effort: None,
        };

        match spawner.spawn_async(request, inherit).await {
            Ok(launch) => {
                let agent_id_str = launch.agent_id.to_string();
                // (G14) Register name → agentId for SendMessage routing — ASYNC
                // ONLY, post-launch so a failed spawn leaves no stale entry
                // (claude AgentTool.tsx:700-712). Prefer the ctx-level registry
                // when wired; else the spawner's internal map.
                if let Some(name) = parsed.name.as_deref() {
                    if let Some(reg) = &self.ctx.agent_name_registry {
                        reg.register(name, launch.agent_id).await;
                    } else {
                        spawner.register_name(name, launch.agent_id).await;
                    }
                }
                // claude `canReadOutputFile = tools.some(Read || Bash)`
                // (AgentTool.tsx:753): whether the parent pool can read the
                // output file. Approximated via the wired parent registry.
                let can_read_output_file = ctx.subagent_registry.as_ref().is_some_and(|reg| {
                    reg.find_by_name("Read").is_some() || reg.find_by_name("Bash").is_some()
                });
                // claude `async_launched` tool_result text, byte-for-byte
                // (AgentTool.tsx:1328-1330): a fixed prefix + a
                // `canReadOutputFile`-branched instruction tail, joined by `\n`.
                let output_file = &launch.output_file;
                let prefix = format!(
                    "Async agent launched successfully.\nagentId: {agent_id_str} (internal ID - do not mention to user. Use SendMessage with to: '{agent_id_str}' to continue this agent.)\nThe agent is working in the background. You will be notified automatically when it completes."
                );
                let instructions = if can_read_output_file {
                    // claude uses FILE_READ_TOOL_NAME / BASH_TOOL_NAME — the
                    // canonical tool names (`Read` / `Bash`).
                    format!(
                        "Do not duplicate this agent's work — avoid working with the same files or topics it is using. Work on non-overlapping tasks, or briefly tell the user what you launched and end your response.\noutput_file: {output_file}\nIf asked, you can check progress before completion by using Read or Bash tail on the output file."
                    )
                } else {
                    "Briefly tell the user what you launched and end your response. Do not generate any other text — agent results will arrive in a subsequent message.".to_string()
                };
                let model_content = format!("{prefix}\n{instructions}");
                Ok(ToolCallResult {
                    data: json!({
                        "isAsync": true,
                        "status": "async_launched",
                        "agentId": agent_id_str,
                        "description": parsed.description,
                        "prompt": parsed.prompt,
                        "outputFile": launch.output_file,
                        "canReadOutputFile": can_read_output_file,
                        "model_content": model_content,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(e) => {
                Self::emit_failed(
                    bus,
                    invocation_id,
                    "async_spawn_error",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                Err(ToolError::Internal(format!(
                    "Agent: async spawn failed: {e}"
                )))
            }
        }
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
    fn search_hint(&self) -> Option<&str> {
        // claude `searchHint: 'delegate work to a subagent'` (AgentTool.tsx:227).
        Some("delegate work to a subagent")
    }
    fn get_activity_description(&self, input: &Value) -> Option<String> {
        // claude `getActivityDescription(input) { return input?.description ??
        // 'Running task' }` (AgentTool.tsx:1278-1280).
        Some(
            input
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("Running task")
                .to_string(),
        )
    }
    fn user_facing_name_for_input(&self, input: &Value) -> Option<String> {
        // claude `userFacingName(input)` (UI.tsx:760-775): show the subagent type
        // (except `general-purpose`/`worker`, which display as "Agent").
        let subagent_type = input.get("subagent_type").and_then(Value::as_str);
        Some(match subagent_type {
            Some(t) if t != GENERAL_PURPOSE_AGENT_TYPE => {
                if t == "worker" {
                    // claude: display "worker" agents as "Agent" for cleaner UI.
                    "Agent".to_string()
                } else {
                    t.to_string()
                }
            }
            _ => "Agent".to_string(),
        })
    }
    fn user_facing_name_background_color(&self, input: &Value) -> Option<String> {
        // claude `userFacingNameBackgroundColor(input)` (UI.tsx:776-787): no
        // subagent_type ⇒ None; else the agent's color via `getAgentColor`.
        // claude's `getAgentColor` is process-global; the Rust color manager
        // lives in the `agent` crate and is NOT threaded onto
        // `BuiltinToolContext`, so we return None here (a faithful degraded
        // behavior — no wrong color; documented residual).
        input.get("subagent_type").and_then(Value::as_str)?;
        None
    }
    fn input_schema(&self) -> &Value {
        &AGENT_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        // claude `maxResultSizeChars: 100_000` (AgentTool.tsx:229) — a flat cap,
        // NOT the shared `MAX_TOOL_OUTPUT_LENGTH`.
        AGENT_MAX_RESULT_SIZE_CHARS
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // claude `isConcurrencySafe() { return true }` (AgentTool.tsx:1273-1275):
        // the Agent tool delegates to its underlying tools' own permission /
        // concurrency checks, so it is itself concurrency-safe — it is not an
        // exclusive drain barrier in the streaming executor.
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        // claude `isReadOnly() { return true }` (AgentTool.tsx:1264-1266):
        // "delegates permission checks to its underlying tools".
        true
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
        // claude `async description() { return 'Launch a new agent' }`
        // (AgentTool.tsx:230-232). The dynamic catalog text lives in `prompt()`.
        "Launch a new agent".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        // claude-code builds the Agent tool prompt dynamically (getPrompt,
        // AgentTool/prompt.ts:66-287): it injects the live agent catalog (one
        // `formatAgentLine` per resolved AgentDefinition) and the available MCP
        // server names. Pull the catalog from the spawner (defaulted-empty when
        // unwired) and the MCP server names from the registry.
        let mut agents = match &self.ctx.subagent_spawner {
            Some(s) => s.agent_listing().await,
            None => Vec::new(),
        };
        // Filter out agent types denied by a content-ful `Agent(<x>)` rule, so the
        // advertised catalog the model sees excludes them (claude-code `Pxe` —
        // the 2.1.186 Agent(type)-restriction prompt filter). No gate / no rules
        // ⇒ nothing removed (byte-identical to before).
        if let Some(gate) = &self.ctx.permission_gate {
            let denied = gate.agent_deny_content_types().await;
            if !denied.is_empty() {
                agents.retain(|a| !denied.iter().any(|d| d == &a.agent_type));
            }
        }
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

        // (G7) NO empty-prompt validation: claude-code has no such guard — a
        // `prompt: ""` spawn must succeed (AgentTool.call accepts any prompt).
        // The former hard-reject + `empty_prompt` emit are removed for parity.

        // 2. Wiring guard: spawner must be present (needed for the type
        // validation + MCP gate below, and the dispatch).
        let spawner = self.ctx.subagent_spawner.clone().ok_or_else(|| {
            ToolError::Internal(
                "AgentTool: SubagentSpawner not wired into BuiltinToolContext".into(),
            )
        })?;

        // 3. Fork-subagent routing (codex #5, claude `AgentTool.tsx:318-335`):
        //   effectiveType = subagent_type ?? (gate ? undefined : general-purpose)
        //   isForkPath    = effectiveType === undefined
        // In Rust: an OMITTED `subagent_type` is `None`; with the fork gate ON we
        // take the fork path; with it OFF we fall back to `general-purpose`. An
        // EXPLICIT type always wins (never forks), even when the gate is ON.
        //
        // The gate (`is_fork_subagent_enabled`) is mutually exclusive with
        // coordinator mode + non-interactive sessions. `is_coordinator` is not
        // threaded onto `BuiltinToolContext` today (the `prompt()` path already
        // notes this with `false`), so it is passed `false` here — the coordinator
        // arm of the mutual-exclusion is therefore inert until that signal is
        // wired (documented thin-coordinator residual). The non-interactive arm IS
        // honored via `ctx.options.is_non_interactive_session`.
        let is_fork = parsed.subagent_type.is_none()
            && traits::fork_subagent::is_fork_subagent_enabled(
                false, // is_coordinator — not threaded yet (see above)
                ctx.options.is_non_interactive_session,
            );

        // Recursion guard (claude `AgentTool.tsx:332-334`): fork children keep the
        // Agent tool in their pool for cache-identical tool defs, so a fork inside
        // a forked worker must be rejected. claude's primary check is `querySource
        // === agent:builtin:fork`, which has NO carrier on `ToolUseContext` in
        // this arch — so the message-scan fallback (`is_in_fork_child`) is the
        // sole guard. It relies on the `<fork-boilerplate>` tag being present in
        // the child's inherited transcript, which `build_child_message`
        // guarantees. (FLAG: faithful but narrower than claude's dual check.)
        if is_fork && traits::fork_subagent::is_in_fork_child(&ctx.messages) {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "fork_in_fork_child",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(
                "Fork is not available inside a forked worker. Complete your task directly using your tools.".into(),
            ));
        }

        // Resolve the EFFECTIVE subagent type (claude `effectiveType`,
        // AgentTool.tsx:322 + :337-356):
        //   - FORK PATH ⇒ the synthetic `fork` type.
        //   - OMITTED (`None`, gate OFF) ⇒ `general-purpose` (claude `?? GENERAL_PURPOSE`).
        //   - EXPLICIT (`Some(x)`) ⇒ validated against the agent listing; an
        //     unknown explicit type is REJECTED with claude's "Agent type 'x'
        //     not found. Available agents: …" error (AgentTool.tsx:353).
        // Denied-by-permission-rule (claude-code `getDenyRuleForAgent` →
        // `AgentTypeError`, AgentTool.tsx:349-351): a content-ful `Agent(<type>)`
        // deny rule rejects the resolved subagent type — checked BEFORE the
        // not-found lookup (binary `o5e` precedes the catalog match) and applied
        // to the `general-purpose` default too (deny `Agent(general-purpose)`
        // blocks an omitted type). Byte-exact message + raw `SettingSource`.
        let effective_type: String = if is_fork {
            traits::fork_subagent::FORK_SUBAGENT_TYPE.to_string()
        } else {
            let candidate = parsed
                .subagent_type
                .as_deref()
                .unwrap_or(GENERAL_PURPOSE_AGENT_TYPE);
            if let Some(gate) = &self.ctx.permission_gate {
                if let Some(source) = gate.agent_type_deny(candidate).await {
                    Self::emit_failed(
                        &bus,
                        &invocation_id,
                        "agent_type_denied",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::InvalidInput(format!(
                        "Agent type '{candidate}' has been denied by permission rule 'Agent({candidate})' from {source}."
                    )));
                }
            }
            match parsed.subagent_type.as_deref() {
                None => GENERAL_PURPOSE_AGENT_TYPE.to_string(),
                Some(explicit) => {
                    let listing = spawner.agent_listing().await;
                    if listing.iter().any(|a| a.agent_type == explicit) {
                        explicit.to_string()
                    } else {
                        // The `Available agents:` set is the deny-filtered listing
                        // (claude-code `Pxe`), so a denied type never appears as a
                        // suggestion.
                        let denied = match &self.ctx.permission_gate {
                            Some(gate) => gate.agent_deny_content_types().await,
                            None => Vec::new(),
                        };
                        let available = listing
                            .iter()
                            .filter(|a| !denied.iter().any(|d| d == &a.agent_type))
                            .map(|a| a.agent_type.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        Self::emit_failed(
                            &bus,
                            &invocation_id,
                            "agent_type_not_found",
                            started.elapsed().as_millis() as u64,
                        )
                        .await;
                        return Err(ToolError::InvalidInput(format!(
                            "Agent type '{explicit}' not found. Available agents: {available}"
                        )));
                    }
                }
            }
        };

        // 4. (G3) Required-MCP-servers gate (claude AgentTool.tsx:367-409): if the
        // resolved agent declares `required_mcp_servers`, every required pattern
        // must match an MCP server that currently exposes tools (connected AND
        // authenticated). A missing requirement is a hard error listing the
        // unmatched patterns + the servers that DO have tools.
        //
        // DIVERGENCE (flagged): claude first waits up to 30s (500ms poll) for any
        // required server still in the `pending` (connecting) state before
        // checking tool availability. LingXi's `McpStatus` collapses
        // Connecting/AwaitingOAuth/Reconnecting → `Disconnected` (registry.rs
        // `project_status`), so a `pending` server cannot be distinguished from a
        // failed/absent one — the poll-wait is NOT reproducible. We check tool
        // availability immediately. See the step report's follow-ups.
        let required_mcp_servers = spawner.resolve_required_mcp_servers(&effective_type).await;
        if !required_mcp_servers.is_empty() {
            let servers_with_tools: Vec<String> = match &self.ctx.mcp_registry {
                Some(reg) => reg.servers_with_tools().await,
                None => Vec::new(),
            };
            // claude `hasRequiredMcpServers` (loadAgentsDir.ts:229-242): every
            // required pattern must match (case-insensitive substring) at least
            // one server-with-tools.
            let missing: Vec<String> = required_mcp_servers
                .iter()
                .filter(|pattern| {
                    let p = pattern.to_lowercase();
                    !servers_with_tools
                        .iter()
                        .any(|server| server.to_lowercase().contains(&p))
                })
                .cloned()
                .collect();
            if !missing.is_empty() {
                let servers_list = if servers_with_tools.is_empty() {
                    "none".to_string()
                } else {
                    servers_with_tools.join(", ")
                };
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "missing_required_mcp_servers",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Internal(format!(
                    "Agent '{effective_type}' requires MCP servers matching: {}. \
MCP servers with tools: {servers_list}. \
Use /mcp to configure and authenticate the required MCP servers.",
                    missing.join(", ")
                )));
            }
        }

        // 5. Wiring guard: budget + registry must be present.
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

        // 6. M3-05 byte-locked budget gate. `check_and_charge(0)` re-runs the
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

        // 7. Emit started (telemetry keys on the EFFECTIVE type, not the raw
        // optional input).
        Self::emit_started(
            &bus,
            &invocation_id,
            &effective_type,
            parsed.prompt.chars().count(),
        )
        .await;

        // (G11) Resolve the pre-spawn selection metadata (source / color /
        // resolved model / is_built_in) so we can emit claude's
        // `tengu_agent_tool_selected` (AgentTool.tsx:419-428). One cheap
        // catalog lookup via the spawner seam.
        let selected = spawner
            .resolve_selection(&effective_type, parsed.model.as_deref())
            .await;
        // claude `is_async = (run_in_background === true || selectedAgent.background
        // === true) && !isBackgroundTasksDisabled` (AgentTool.tsx:426). The agent
        // definition's `background` frontmatter flag is now surfaced on
        // `SelectedAgentMeta.background` and honored here, so a `background: true`
        // agent dispatches async even when the caller omits `run_in_background`.
        // `isBackgroundTasksDisabled` is not threaded → assumed false (documented
        // residual). The same `run_in_background` value drives BOTH the telemetry
        // `is_async` flag and the async-dispatch branch below.
        let run_in_background = parsed.run_in_background.unwrap_or(false) || selected.background;
        let is_async = run_in_background;
        Self::emit_agent_tool_selected(
            &bus,
            &selected.agent_type,
            &selected.resolved_model,
            &selected.source,
            selected.color.as_deref(),
            selected.is_built_in,
            false, // is_resume — resume path not modeled
            is_async,
            is_fork,
        )
        .await;

        // PLANNED #2/G13: async (`run_in_background`) spawn. claude returns an
        // `async_launched` payload immediately and drives the lifecycle detached
        // (AgentTool.tsx:686-764). The async seam (`spawn_async`) defaults to a
        // CLEAR error when unwired — surface it rather than silently falling back
        // to a sync spawn (the task forbids a silent wrong path). When the
        // production spawner overrides `spawn_async`, this branch returns the
        // `async_launched` JSON + registers `name → agentId` (G14, async-only).
        if run_in_background {
            return self
                .dispatch_async(
                    &bus,
                    &invocation_id,
                    started,
                    spawner.as_ref(),
                    &parsed,
                    &effective_type,
                    &selected,
                    is_fork,
                    &ctx,
                    budget.clone(),
                    parent_registry.clone(),
                )
                .await;
        }

        // 8. Build the inheritance bundle and dispatch into the spawner.
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
        // Fork path: build the byte-exact forked prefix from the parent's LAST
        // assistant message (claude `assistantMessage` = the in-flight assistant
        // turn that issued THIS tool_use — the most recent
        // `ConversationMessage::Assistant` in `ctx.messages`). With no assistant
        // message present, `build_forked_messages` falls back to a single
        // directive user message (its no-tool_use branch). The directive text is
        // ALREADY the trailing block inside that prefix, so on the fork path
        // `request.prompt` is unused as a seed (the spawner seeds an empty
        // prompt_messages — see handle.rs spawn note (A)).
        let fork_context_messages: Option<Vec<protocol::ConversationMessage>> = if is_fork {
            let assistant = ctx
                .messages
                .iter()
                .rev()
                .find(|m| matches!(m, protocol::ConversationMessage::Assistant { .. }));
            let fork_msgs = match assistant {
                Some(a) => traits::fork_subagent::build_forked_messages(&parsed.prompt, a),
                // No assistant turn yet → fallback directive-only user message.
                None => traits::fork_subagent::build_forked_messages(
                    &parsed.prompt,
                    &protocol::ConversationMessage::Assistant {
                        id: protocol::MessageId::new(),
                        content: vec![],
                        stop_reason: None,
                    },
                ),
            };
            // Worktree notice (claude AgentTool.tsx:598-602): when the fork child
            // runs in an isolated worktree, claude appends
            // `build_worktree_notice(getCwd(), worktreeInfo.worktreePath)` to the
            // fork prefix. Worktree isolation is DEFERRED in this arch
            // (subagent_spawn.rs documents isolation deferred), so there is no
            // worktree to notice today — this is intentionally a NO-OP (do NOT
            // fabricate a worktree). Lands with the isolation batch via
            // `traits::fork_subagent::build_worktree_notice`.
            Some(fork_msgs)
        } else {
            None
        };
        let request = SubagentSpawnRequest {
            // Propagate the RESOLVED effective type (fork → `fork`; omitted →
            // general-purpose; explicit-validated otherwise), not the raw input.
            subagent_type: effective_type.clone(),
            prompt: parsed.prompt.clone(),
            context_paths: parsed.context_paths.clone(),
            // AgentTool spawn-surface parity: thread the new params through.
            // `model` is mapped to the agent model override by the spawner; the
            // rest are carried with their behavior deferred (teammate routing /
            // worktree-remote isolation / cwd override land with later batches).
            description: Some(parsed.description.clone()),
            // Fork path sends `model: None` (claude `model: undefined`) so the
            // FORK_AGENT's `Inherit` resolves to the parent model unchanged; the
            // explicit-model override is honored only on the non-fork path.
            model: if is_fork { None } else { parsed.model.clone() },
            // claude collapses `run_in_background === true`; absent ⇒ false.
            run_in_background: parsed.run_in_background.unwrap_or(false),
            // Fork path carries no teammate/isolation/cwd overrides.
            name: if is_fork { None } else { parsed.name.clone() },
            team_name: if is_fork { None } else { parsed.team_name.clone() },
            mode: if is_fork { None } else { parsed.mode.clone() },
            isolation: if is_fork { None } else { parsed.isolation.clone() },
            cwd: if is_fork { None } else { parsed.cwd.clone() },
            // Fork-subagent carriers (codex #5): the byte-exact forked prefix the
            // spawner replays as the cache prefix, and the parent's already-
            // rendered system prompt bytes (TS `forkContextMessages` /
            // `override.systemPrompt = forkParentSystemPrompt`). Both `None` on
            // the non-fork path. A `None` `fork_parent_system_prompt` (the
            // orchestrator has not yet threaded `renderedSystemPrompt` onto
            // `ToolUseContext`) means the child runs with FORK_AGENT's empty
            // system prompt — functional, not byte-identical — see follow-ups.
            fork_context_messages,
            fork_parent_system_prompt: if is_fork {
                ctx.fork_parent_system_prompt.clone()
            } else {
                None
            },
            schema: None,
            effort: None,
        };

        let outcome = spawner.spawn(request, inherit).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match outcome {
            Ok(SubagentResult::Completed {
                agent_id,
                content,
                usage,
                total_tool_use_count,
                total_duration_ms,
                total_tokens,
                assistant_message_count,
                response_char_count,
                last_request_id,
            }) => {
                // Internal back-compat event (kept).
                Self::emit_completed(&bus, &invocation_id, duration_ms, &effective_type).await;
                // (G11) claude `tengu_agent_tool_completed` (agentToolUtils.ts:322).
                Self::emit_agent_tool_completed(
                    &bus,
                    &effective_type,
                    &selected.resolved_model,
                    parsed.prompt.chars().count() as u64,
                    response_char_count,
                    assistant_message_count,
                    total_tool_use_count,
                    duration_ms,
                    total_tokens,
                    selected.is_built_in,
                    is_async,
                )
                .await;
                // (G11) claude `tengu_cache_eviction_hint` — only when a
                // last_request_id is present (agentToolUtils.ts:339).
                if let Some(req_id) = last_request_id.as_deref() {
                    Self::emit_cache_eviction_hint(&bus, req_id).await;
                }

                // claude `finalizeAgentTool` content[] (agentToolUtils.ts:348-356):
                // the agent's final text blocks (backward-scan applied in the
                // runner). Re-materialize the `{type:'text', text}` blocks for the
                // result's `content` array.
                let content_texts = extract_content_texts(&content);
                let content_blocks: Vec<Value> = content_texts
                    .iter()
                    .map(|t| json!({ "type": "text", "text": t }))
                    .collect();

                let agent_id_str = agent_id.to_string();

                // The model-facing string (claude `mapToolResultToToolResultBlockParam`,
                // AgentTool.tsx:1340-1373) — consumed by orchestrator
                // `tool_result_to_model_text` (reads `data.model_content`). This is
                // what keeps the bytes the MODEL sees byte-identical to claude.
                let model_content = render_completed_model_content(
                    &content_texts,
                    &agent_id_str,
                    &effective_type,
                    total_tokens,
                    total_tool_use_count,
                    total_duration_ms,
                );

                // claude completed return shape (AgentTool.tsx:1253-1260 +
                // finalizeAgentTool return). DROP the `subagent_type`/`result`
                // keys claude does not emit; surface claude's structured fields.
                // `usage` mirrors claude's Anthropic usage object
                // (`agentToolUtils.ts:238-256`). The nullable sub-objects
                // server_tool_use/service_tier/cache_creation are ALWAYS present
                // in claude's shape (emitted as `null` when absent), so we emit
                // them as `null` here too — null is the correct shape, NOT
                // zero-faked numerics. Faithfully populating them (web-search /
                // web-fetch request counts, service tier, 1h/5m cache split)
                // is DEFERRED pending the deeper `llm_client::Usage` extension
                // (see SubagentUsage doc); the key SHAPE matches now.
                Ok(ToolCallResult {
                    data: json!({
                        "status": "completed",
                        "prompt": parsed.prompt,
                        "agentId": agent_id_str,
                        "agentType": effective_type,
                        "content": content_blocks,
                        "totalToolUseCount": total_tool_use_count,
                        "totalDurationMs": total_duration_ms,
                        "totalTokens": total_tokens,
                        "usage": {
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                            "cache_creation_input_tokens": usage.cache_creation_input_tokens,
                            "cache_read_input_tokens": usage.cache_read_input_tokens,
                            "server_tool_use": serde_json::Value::Null,
                            "service_tier": serde_json::Value::Null,
                            "cache_creation": serde_json::Value::Null,
                        },
                        // model-facing string consumed by orchestrator
                        // `tool_result_to_model_text` (turn_loop.rs).
                        "model_content": model_content,
                        // R7 (single-fire): the child runner (`run_subagent`)
                        // already fired the canonical `SubagentStart` (collecting
                        // additionalContext for the child) and the child's own
                        // frontmatter `Stop`→`SubagentStop`, exactly as claude
                        // does inside `runAgent`. This engine-internal flag tells
                        // the orchestrator chokepoint to SKIP its own
                        // SubagentStart fire (no double-fire) and to fire only the
                        // session/plugin `SubagentStop` complement. Invisible to
                        // the model (it reads `model_content`) and never written
                        // to JSONL — same category as `model_content` itself.
                        "subagentHooksFired": true,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Ok(SubagentResult::Failed { reason, .. }) => {
                Self::emit_failed(&bus, &invocation_id, "subagent_failed", duration_ms).await;
                Err(ToolError::Internal(reason))
            }
            Ok(SubagentResult::Killed { .. }) => {
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
            fork_parent_system_prompt: None,
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
        assert_eq!(parsed.subagent_type.as_deref(), Some("general-purpose"));
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

    // AGENT.1 — omitting `subagent_type` parses to `None`; the
    // `general-purpose` default is applied at call time via `effective_type`,
    // matching TS `subagent_type ?? GENERAL_PURPOSE_AGENT.agentType`
    // (AgentTool.tsx:85 optional + :322 default-on-use, not default-on-parse).
    #[test]
    fn agent_input_defaults_subagent_type_to_general_purpose() {
        let v = json!({ "description": "d", "prompt": "Explore the repo." });
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert!(parsed.subagent_type.is_none());
        // The effective default `general-purpose` is a known built-in type.
        assert!(BUILTIN_SUBAGENT_TYPES.contains(&GENERAL_PURPOSE_AGENT_TYPE));
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
            json!(["sonnet", "opus", "haiku", "fable"])
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

    // #1 — an EXPLICIT unknown `subagent_type` is REJECTED with claude's
    // "Agent type 'x' not found. Available agents: …" error
    // (AgentTool.tsx:345-354), and the spawner is NOT invoked. (Only an OMITTED
    // type falls back to general-purpose; see
    // `omitted_subagent_type_spawns_general_purpose`.)
    #[tokio::test]
    async fn explicit_unknown_subagent_type_is_rejected_with_not_found() {
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
        let err = tool
            .call(input, ctx, fresh_tx())
            .await
            .expect_err("explicit unknown subagent_type must be rejected");
        let msg = format!("{err}");
        // The available-agents list comes from the mock listing
        // (general-purpose, Explore, Plan), in listing order. The INNER message
        // is byte-locked to claude (AgentTool.tsx:353); the LingXi
        // `ToolError::InvalidInput` Display adds a crate-wide `invalid input: `
        // prefix (a constant across EVERY tool, not Agent-specific), so we assert
        // the inner string is present verbatim rather than the whole wrapped form.
        assert!(
            msg.contains(
                "Agent type 'my-custom-project-agent' not found. Available agents: general-purpose, Explore, Plan"
            ),
            "byte-locked not-found error (inner); got: {msg}"
        );
        assert!(
            spawner.invocations().is_empty(),
            "spawner must NOT be invoked for an unknown explicit type"
        );
    }

    // #1 — OMITTING `subagent_type` (None) falls back to `general-purpose`
    // (claude `subagent_type ?? GENERAL_PURPOSE_AGENT.agentType`,
    // AgentTool.tsx:322) and dispatches; the request carries the resolved
    // effective type.
    #[tokio::test]
    async fn omitted_subagent_type_spawns_general_purpose() {
        // Acquire the fork-gate lock + clear the var: with the gate ON an omitted
        // subagent_type would take the FORK path, not general-purpose. Serialize
        // against the gate-ON tests so this default-OFF assertion is stable.
        let _g = FORK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_FORK_SUBAGENT");

        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        // No `subagent_type` key → omitted → general-purpose.
        let input = serde_json::json!({
            "description": "do anything",
            "prompt": "do it"
        });
        tool.call(input, ctx, fresh_tx())
            .await
            .expect("omitted subagent_type must default to general-purpose");
        let invocations = spawner.invocations();
        assert_eq!(invocations.len(), 1, "spawner is invoked");
        assert_eq!(
            invocations[0].request.subagent_type, "general-purpose",
            "omitted → effective general-purpose threaded into the request"
        );
    }

    // #1 — a KNOWN explicit type (in the listing) spawns and threads through.
    #[tokio::test]
    async fn known_explicit_subagent_type_is_dispatched() {
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
            "description": "explore",
            "subagent_type": "Explore",
            "prompt": "look around"
        });
        tool.call(input, ctx, fresh_tx())
            .await
            .expect("known explicit type must be dispatched");
        let invocations = spawner.invocations();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].request.subagent_type, "Explore");
    }

    // ── codex #5: fork-subagent path ──

    /// `CLAUDE_CODE_FORK_SUBAGENT` is process-global; serialize the tests whose
    /// behavior depends on the fork gate so a gate-ON test never races a
    /// default-OFF assertion. Every such test acquires this AND removes the var
    /// first (mirrors `AGENT_LIST_ENV_LOCK`).
    static FORK_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Build a `ToolUseContext` carrying the given conversation history (for the
    /// fork-path assistant-message selection + recursion guard).
    fn ctx_with_messages(
        registry: Arc<ToolRegistry>,
        messages: Vec<protocol::ConversationMessage>,
    ) -> ToolUseContext {
        let mut c = fresh_ctx_with_registry(registry);
        c.messages = messages;
        c
    }

    fn parent_assistant_with_tool_use() -> protocol::ConversationMessage {
        protocol::ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![
                protocol::ContentBlock::Text {
                    text: "I'll run a command".into(),
                },
                protocol::ContentBlock::ToolUse {
                    id: protocol::ToolUseId::new(),
                    name: "Bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                    provider_id: Some("toolu_x".into()),
                },
            ],
            stop_reason: Some("tool_use".into()),
        }
    }

    // Gate OFF (default): an OMITTED subagent_type still spawns general-purpose
    // with NO fork fields set. (Acquires the gate lock so a concurrent gate-ON
    // test never flips the env under it.)
    #[tokio::test]
    async fn fork_gate_off_omitted_spawns_general_purpose_no_fork_fields() {
        let _g = FORK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_FORK_SUBAGENT");

        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = ctx_with_messages(
            Arc::new(ToolRegistry::new()),
            vec![parent_assistant_with_tool_use()],
        );
        let input = serde_json::json!({ "description": "do anything", "prompt": "do it" });
        tool.call(input, ctx, fresh_tx()).await.expect("spawns");
        let inv = spawner.invocations();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].request.subagent_type, "general-purpose");
        assert!(inv[0].request.fork_context_messages.is_none());
        assert!(inv[0].request.fork_parent_system_prompt.is_none());

        std::env::remove_var("CLAUDE_CODE_FORK_SUBAGENT");
    }

    // Gate ON + omitted subagent_type + a parent assistant-with-tool_use in
    // ctx.messages → fork path: request.subagent_type == "fork",
    // fork_context_messages == [assistant_clone, user(tool_results + directive)].
    #[tokio::test]
    async fn fork_gate_on_omitted_takes_fork_path() {
        let _g = FORK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_FORK_SUBAGENT", "1");

        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        // is_non_interactive_session must be false (fresh_ctx_with_registry sets
        // false) for the gate to fire.
        let ctx = ctx_with_messages(
            Arc::new(ToolRegistry::new()),
            vec![parent_assistant_with_tool_use()],
        );
        let input = serde_json::json!({ "description": "fork it", "prompt": "Do the subtask" });
        tool.call(input, ctx, fresh_tx()).await.expect("fork spawns");

        let inv = spawner.invocations();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].request.subagent_type, "fork");
        // Fork path sends model: None (claude model: undefined).
        assert!(inv[0].request.model.is_none());
        let fc = inv[0]
            .request
            .fork_context_messages
            .as_ref()
            .expect("fork_context_messages set on fork path");
        assert_eq!(fc.len(), 2, "[assistant_clone, user(tool_results+directive)]");
        assert!(matches!(fc[0], protocol::ConversationMessage::Assistant { .. }));
        match &fc[1] {
            protocol::ConversationMessage::User { content, .. } => {
                // 1 tool_result (one tool_use) + the directive Text block.
                assert_eq!(content.len(), 2);
                assert!(matches!(content[0], protocol::ContentBlock::ToolResult { .. }));
                match &content[1] {
                    protocol::ContentBlock::Text { text } => {
                        assert!(text.starts_with("<fork-boilerplate>"));
                        assert!(text.ends_with("Your directive: Do the subtask"));
                    }
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected User, got {other:?}"),
        }

        std::env::remove_var("CLAUDE_CODE_FORK_SUBAGENT");
    }

    // Fork system-prompt threading (codex #5 follow-up): when the turn loop has
    // populated `ctx.fork_parent_system_prompt` with the parent's rendered bytes,
    // the fork spawn request carries those EXACT bytes (claude
    // `override.systemPrompt = forkParentSystemPrompt`, AgentTool.tsx:622-623).
    #[tokio::test]
    async fn fork_threads_parent_system_prompt_onto_request() {
        let _g = FORK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_FORK_SUBAGENT", "1");

        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let parent_bytes = "PARENT RENDERED SYSTEM PROMPT\n\n<env>cwd=/x</env>";
        let mut ctx = ctx_with_messages(
            Arc::new(ToolRegistry::new()),
            vec![parent_assistant_with_tool_use()],
        );
        ctx.fork_parent_system_prompt = Some(parent_bytes.to_string());
        let input = serde_json::json!({ "description": "fork it", "prompt": "Do the subtask" });
        tool.call(input, ctx, fresh_tx()).await.expect("fork spawns");

        let inv = spawner.invocations();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].request.subagent_type, "fork");
        assert_eq!(
            inv[0].request.fork_parent_system_prompt.as_deref(),
            Some(parent_bytes),
            "fork child must carry the parent's exact rendered system prompt bytes"
        );

        std::env::remove_var("CLAUDE_CODE_FORK_SUBAGENT");
    }

    // Recursion guard: gate ON + ctx.messages already contains a
    // `<fork-boilerplate>` user Text block → Err with the byte-exact message,
    // and the spawner is NOT invoked.
    #[tokio::test]
    async fn fork_recursion_guard_rejects_inside_fork_child() {
        let _g = FORK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_FORK_SUBAGENT", "1");

        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let boilerplate = protocol::ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: traits::fork_subagent::build_child_message("prior directive"),
            }],
            is_meta: false,
        };
        let ctx = ctx_with_messages(Arc::new(ToolRegistry::new()), vec![boilerplate]);
        let input = serde_json::json!({ "description": "fork again", "prompt": "nested" });
        let err = tool
            .call(input, ctx, fresh_tx())
            .await
            .expect_err("fork inside a fork child must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains(
                "Fork is not available inside a forked worker. Complete your task directly using your tools."
            ),
            "byte-exact recursion-guard message; got: {msg}"
        );
        assert!(spawner.invocations().is_empty(), "spawner must NOT be invoked");

        std::env::remove_var("CLAUDE_CODE_FORK_SUBAGENT");
    }

    // Explicit subagent_type wins over fork even when the gate is ON (claude:
    // an explicit type never forks).
    #[tokio::test]
    async fn fork_gate_on_explicit_type_does_not_fork() {
        let _g = FORK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_FORK_SUBAGENT", "1");

        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = ctx_with_messages(
            Arc::new(ToolRegistry::new()),
            vec![parent_assistant_with_tool_use()],
        );
        let input = serde_json::json!({
            "description": "explore",
            "subagent_type": "Explore",
            "prompt": "look around"
        });
        tool.call(input, ctx, fresh_tx()).await.expect("explicit dispatch");
        let inv = spawner.invocations();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].request.subagent_type, "Explore", "explicit wins; no fork");
        assert!(inv[0].request.fork_context_messages.is_none());

        std::env::remove_var("CLAUDE_CODE_FORK_SUBAGENT");
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
                model: None,
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

    // A permission gate that denies the `Explore` agent type, for the
    // Agent(type)-restriction filter tests (claude-code `Pxe` / `getDenyRuleForAgent`).
    struct DenyExploreGate;
    #[async_trait::async_trait]
    impl traits::permission_gate::PermissionGate for DenyExploreGate {
        async fn check(
            &self,
            _name: &str,
            _input: &serde_json::Value,
        ) -> traits::permission_gate::PermissionDecision {
            traits::permission_gate::PermissionDecision::Allow
        }
        async fn agent_type_deny(&self, agent_type: &str) -> Option<String> {
            (agent_type == "Explore").then(|| "localSettings".to_string())
        }
        async fn agent_deny_content_types(&self) -> Vec<String> {
            vec!["Explore".to_string()]
        }
    }

    // The advertised catalog excludes a denied agent type (claude-code `Pxe`):
    // `Explore` is denied, so it must NOT appear in the prompt while
    // `general-purpose` still does.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn prompt_filters_denied_agent_types() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
        let spawner = arc_mock_spawner();
        let mut bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        bctx.permission_gate = Some(Arc::new(DenyExploreGate));
        let tool = AgentTool::new(bctx);
        let prompt = tool
            .prompt(&PromptOptions {
                include_examples: true,
                model: None,
            })
            .await;
        assert!(
            prompt.contains("- general-purpose:"),
            "general-purpose should remain; prompt was:\n{prompt}"
        );
        assert!(
            !prompt.contains("- Explore:"),
            "denied Explore must be filtered out; prompt was:\n{prompt}"
        );
    }

    // An explicit denied subagent_type is rejected with the byte-exact
    // claude-code `AgentTypeError` message (raw `SettingSource` identifier).
    #[tokio::test]
    async fn call_rejects_denied_agent_type_with_byte_exact_message() {
        let spawner = arc_mock_spawner();
        let mut bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        bctx.permission_gate = Some(Arc::new(DenyExploreGate));
        let tool = AgentTool::new(bctx);
        let input = serde_json::json!({
            "description": "desc here",
            "prompt": "do a thing",
            "subagent_type": "Explore"
        });
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let err = tool.call(input, ctx, fresh_tx()).await.unwrap_err();
        match err {
            ToolError::InvalidInput(msg) => assert_eq!(
                msg,
                "Agent type 'Explore' has been denied by permission rule 'Agent(Explore)' from localSettings."
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        // No spawn happened.
        assert!(spawner.invocations().is_empty());
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

    // ── #4 meta props (AgentTool.tsx:229, 1264-1266, 1273-1275) + G8 ──

    #[tokio::test]
    async fn meta_props_match_claude() {
        let bctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        );
        let tool = AgentTool::new(bctx);
        let v = json!({});
        // #4: maxResultSizeChars 100_000, isConcurrencySafe true, isReadOnly true.
        assert_eq!(tool.max_result_size_chars(), 100_000);
        assert!(tool.is_concurrency_safe(&v));
        assert!(tool.is_read_only(&v));
        // G8: static description is exactly "Launch a new agent".
        let desc = tool
            .description(
                &v,
                &DescriptionOptions {
                    is_non_interactive_session: false,
                },
            )
            .await;
        assert_eq!(desc, "Launch a new agent");
    }

    #[test]
    fn one_shot_builtin_agent_types_locked() {
        assert_eq!(ONE_SHOT_BUILTIN_AGENT_TYPES, &["Explore", "Plan"]);
    }

    // ── G7: empty/whitespace prompt now succeeds (claude has no guard) ──

    #[tokio::test]
    async fn empty_prompt_now_succeeds() {
        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        // Whitespace-only prompt — claude-code accepts it (no validation).
        let input = json!({
            "description": "d",
            "subagent_type": "general-purpose",
            "prompt": "   "
        });
        tool.call(input, ctx, fresh_tx())
            .await
            .expect("empty/whitespace prompt must succeed (no G7 guard)");
        assert_eq!(spawner.invocations().len(), 1, "spawner is invoked");
    }

    // ── #3 + G1: claude finalizeAgentTool return shape + model_content ──

    #[tokio::test]
    async fn completed_result_uses_claude_finalize_shape() {
        let spawner = arc_mock_spawner();
        let child_id = protocol::AgentId::new();
        // Runner-shaped result JSON: claude `content` array of text blocks.
        spawner.script_completed_with(
            child_id,
            json!({
                "content": [{ "type": "text", "text": "the answer" }],
                "text": "the answer",
                "stop_reason": "end_turn",
            }),
            traits::subagent_spawn::SubagentUsage {
                total_tokens: 42,
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: 7,
                cache_read_input_tokens: 20,
            },
            3,    // total_tool_use_count
            1234, // total_duration_ms
            42,   // total_tokens
        );
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = json!({
            "description": "d",
            "subagent_type": "general-purpose",
            "prompt": "do it"
        });
        let result = tool.call(input, ctx, fresh_tx()).await.unwrap();
        let data = &result.data;
        // claude finalize shape: status/prompt/agentId/agentType/content/totals/usage.
        assert_eq!(data["status"], "completed");
        assert_eq!(data["prompt"], "do it");
        assert_eq!(data["agentId"], child_id.to_string());
        assert_eq!(data["agentType"], "general-purpose");
        assert_eq!(
            data["content"],
            json!([{ "type": "text", "text": "the answer" }])
        );
        assert_eq!(data["totalToolUseCount"], 3);
        assert_eq!(data["totalDurationMs"], 1234);
        assert_eq!(data["totalTokens"], 42);
        assert_eq!(data["usage"]["input_tokens"], 10);
        assert_eq!(data["usage"]["output_tokens"], 5);
        assert_eq!(data["usage"]["cache_creation_input_tokens"], 7);
        assert_eq!(data["usage"]["cache_read_input_tokens"], 20);
        // claude's usage object ALWAYS carries these three nullable sub-objects
        // (agentToolUtils.ts:243-256), emitted as `null` when absent — present
        // as keys (NOT omitted), null-valued (NOT zero-faked).
        assert!(
            data["usage"].get("server_tool_use").is_some_and(serde_json::Value::is_null),
            "usage.server_tool_use must be present and null"
        );
        assert!(
            data["usage"].get("service_tier").is_some_and(serde_json::Value::is_null),
            "usage.service_tier must be present and null"
        );
        assert!(
            data["usage"].get("cache_creation").is_some_and(serde_json::Value::is_null),
            "usage.cache_creation must be present and null"
        );
        // The DROPPED legacy keys claude does not emit.
        assert!(data.get("subagent_type").is_none(), "subagent_type key dropped");
        assert!(data.get("result").is_none(), "result key dropped");
        // model_content: content text + agentId/SendMessage hint + <usage>.
        let mc = data["model_content"].as_str().unwrap();
        assert_eq!(
            mc,
            format!(
                "the answer\nagentId: {child_id} (use SendMessage with to: '{child_id}' to continue this agent)\n<usage>subagent_tokens: 42\ntool_uses: 3\nduration_ms: 1234</usage>"
            )
        );
    }

    #[tokio::test]
    async fn completed_one_shot_explore_skips_trailer() {
        // One-shot built-ins (Explore/Plan) → content texts ONLY, no
        // agentId/<usage> trailer (AgentTool.tsx:1356-1362).
        let spawner = arc_mock_spawner();
        let child_id = protocol::AgentId::new();
        spawner.script_completed_with(
            child_id,
            json!({
                "content": [{ "type": "text", "text": "explored" }],
                "text": "explored",
                "stop_reason": "end_turn",
            }),
            traits::subagent_spawn::SubagentUsage::default(),
            1,
            5,
            99,
        );
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = json!({
            "description": "d",
            "subagent_type": "Explore",
            "prompt": "look"
        });
        let result = tool.call(input, ctx, fresh_tx()).await.unwrap();
        let mc = result.data["model_content"].as_str().unwrap();
        // Exactly the content text — NO trailer.
        assert_eq!(mc, "explored");
        assert!(!mc.contains("<usage>"), "one-shot must skip the <usage> trailer");
        assert!(!mc.contains("agentId:"), "one-shot must skip the agentId hint");
    }

    #[tokio::test]
    async fn completed_no_output_uses_marker() {
        // Empty content → the no-output marker (AgentTool.tsx:1347-1350). A
        // non-one-shot agent still gets the trailer after the marker.
        let spawner = arc_mock_spawner();
        let child_id = protocol::AgentId::new();
        spawner.script_completed_with(
            child_id,
            // Runner max-turns / stub shape: no `content` key at all.
            json!({ "reason": "max_turns_exhausted" }),
            traits::subagent_spawn::SubagentUsage::default(),
            0,
            0,
            0,
        );
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = json!({
            "description": "d",
            "subagent_type": "general-purpose",
            "prompt": "do it"
        });
        let result = tool.call(input, ctx, fresh_tx()).await.unwrap();
        let data = &result.data;
        // content[] is empty in the structured result.
        assert_eq!(data["content"], json!([]));
        let mc = data["model_content"].as_str().unwrap();
        assert!(
            mc.starts_with("(Subagent completed but returned no output.)"),
            "empty content → no-output marker; got: {mc}"
        );
        // Non-one-shot → trailer still present after the marker.
        assert!(mc.contains("<usage>subagent_tokens: 0"));
    }

    // ── G3: required-MCP-servers gate (AgentTool.tsx:367-409) ──

    #[tokio::test]
    async fn required_mcp_servers_missing_hard_errors() {
        // The agent requires "github"; no MCP server with tools is wired, so
        // the gate hard-errors listing the missing pattern + "none".
        let spawner = arc_mock_spawner();
        spawner.script_required_mcp_servers(vec!["github".to_string()]);
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = json!({
            "description": "d",
            "subagent_type": "general-purpose",
            "prompt": "do it"
        });
        let err = tool
            .call(input, ctx, fresh_tx())
            .await
            .expect_err("required MCP servers missing → hard error");
        let msg = format!("{err}");
        // Byte-locked claude message (AgentTool.tsx:406-408), inner string.
        assert!(
            msg.contains(
                "Agent 'general-purpose' requires MCP servers matching: github. MCP servers with tools: none. Use /mcp to configure and authenticate the required MCP servers."
            ),
            "byte-locked required-MCP error; got: {msg}"
        );
        // The gate precedes the spawn — the spawner is NOT invoked.
        assert!(
            spawner.invocations().is_empty(),
            "spawner must not be invoked when the MCP gate fails"
        );
    }

    #[tokio::test]
    async fn no_required_mcp_servers_skips_gate() {
        // Default: empty required_mcp_servers → gate skipped → spawn proceeds.
        let spawner = arc_mock_spawner();
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = json!({
            "description": "d",
            "subagent_type": "general-purpose",
            "prompt": "do it"
        });
        tool.call(input, ctx, fresh_tx())
            .await
            .expect("no required MCP servers → spawn proceeds");
        assert_eq!(spawner.invocations().len(), 1);
    }

    // =====================================================================
    // G9 — searchHint / userFacingName / userFacingNameBackgroundColor /
    // getActivityDescription byte-parity with claude (AgentTool.tsx + UI.tsx).
    // =====================================================================
    fn bare_agent_tool() -> AgentTool {
        AgentTool::new(ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        ))
    }

    #[test]
    fn g9_search_hint_byte_locked() {
        assert_eq!(bare_agent_tool().search_hint(), Some("delegate work to a subagent"));
    }

    #[test]
    fn g9_get_activity_description_uses_input_else_fallback() {
        let tool = bare_agent_tool();
        assert_eq!(
            tool.get_activity_description(&json!({ "description": "find the bug" })),
            Some("find the bug".to_string())
        );
        // Missing description → "Running task" (AgentTool.tsx:1278-1280).
        assert_eq!(
            tool.get_activity_description(&json!({})),
            Some("Running task".to_string())
        );
    }

    #[test]
    fn g9_user_facing_name_cases() {
        let tool = bare_agent_tool();
        // general-purpose → "Agent" (UI.tsx:773).
        assert_eq!(
            tool.user_facing_name_for_input(&json!({ "subagent_type": "general-purpose" })),
            Some("Agent".to_string())
        );
        // worker → "Agent" (UI.tsx:769-771).
        assert_eq!(
            tool.user_facing_name_for_input(&json!({ "subagent_type": "worker" })),
            Some("Agent".to_string())
        );
        // Explore → "Explore" (UI.tsx:772).
        assert_eq!(
            tool.user_facing_name_for_input(&json!({ "subagent_type": "Explore" })),
            Some("Explore".to_string())
        );
        // missing → "Agent" (UI.tsx:774).
        assert_eq!(tool.user_facing_name_for_input(&json!({})), Some("Agent".to_string()));
    }

    #[test]
    fn g9_user_facing_name_background_color_none_without_type_and_color_unwired() {
        let tool = bare_agent_tool();
        // No subagent_type → None (UI.tsx:781-783).
        assert_eq!(tool.user_facing_name_background_color(&json!({})), None);
        // With subagent_type but no wired color manager → None (documented residual).
        assert_eq!(
            tool.user_facing_name_background_color(&json!({ "subagent_type": "Explore" })),
            None
        );
    }

    // =====================================================================
    // G11 — claude-named telemetry events emitted on the dispatch path.
    // =====================================================================
    use telemetry::sinks::InMemorySink;

    async fn wired_ctx_with_bus(
        spawner: Arc<MockSubagentSpawner>,
        bus: Arc<AnalyticsBus>,
    ) -> BuiltinToolContext {
        let mut bctx =
            ctx_for_file_tools(make_dummy_fs(), bus, vec![PathBuf::from("/tmp")]);
        bctx.subagent_spawner = Some(spawner as Arc<dyn SubagentSpawner>);
        bctx.task_registry =
            Some(arc_mock_task_registry() as Arc<dyn traits::task_registry::TaskRegistryHandle>);
        bctx.mailbox_router =
            Some(arc_mock_mailbox() as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        bctx.budget_enforcer =
            Some(arc_mock_budget(u64::MAX) as Arc<dyn BudgetEnforcerHandle>);
        bctx
    }

    #[tokio::test]
    async fn g11_emits_selected_and_completed_with_claude_fields() {
        let sink = Arc::new(InMemorySink::new());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = arc_mock_spawner();
        // Script a completed result that carries the G11 rollups.
        spawner.script_selection(traits::subagent_spawn::SelectedAgentMeta {
            agent_type: "Explore".into(),
            resolved_model: "claude-sonnet".into(),
            source: "built-in".into(),
            color: Some("blue".into()),
            is_built_in: true,
            background: false,
        });
        spawner.script_completed_full(
            protocol::AgentId::new(),
            json!({ "content": [{ "type": "text", "text": "hi there" }] }),
            traits::subagent_spawn::SubagentUsage::default(),
            3,    // total_tool_use_count
            42,   // total_duration_ms
            123,  // total_tokens
            5,    // assistant_message_count
            1,    // response_char_count = content.length (1 text block)
            Some("req_abc".into()),
        );

        let bctx = wired_ctx_with_bus(spawner, bus).await;
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        tool.call(
            json!({ "description": "d", "subagent_type": "Explore", "prompt": "go" }),
            ctx,
            fresh_tx(),
        )
        .await
        .expect("spawn completes");

        let events = sink.events().await;
        let selected = events
            .iter()
            .find(|e| e.name == "tengu_agent_tool_selected")
            .expect("tengu_agent_tool_selected emitted");
        for f in [
            "agent_type",
            "model",
            "source",
            "color",
            "is_built_in_agent",
            "is_resume",
            "is_async",
            "is_fork",
        ] {
            assert!(selected.metadata.contains_key(f), "selected missing {f}");
        }
        let completed = events
            .iter()
            .find(|e| e.name == "tengu_agent_tool_completed")
            .expect("tengu_agent_tool_completed emitted");
        for f in [
            "agent_type",
            "model",
            "prompt_char_count",
            "response_char_count",
            "assistant_message_count",
            "total_tool_uses",
            "duration_ms",
            "total_tokens",
            "is_built_in_agent",
            "is_async",
        ] {
            assert!(completed.metadata.contains_key(f), "completed missing {f}");
        }
        // tengu_cache_eviction_hint emitted (req id present), scope locked.
        let hint = events
            .iter()
            .find(|e| e.name == "tengu_cache_eviction_hint")
            .expect("cache eviction hint emitted when last_request_id present");
        assert!(matches!(
            hint.metadata.get("scope"),
            Some(telemetry::AnalyticsValue::String(s)) if s == "subagent_end"
        ));
    }

    #[tokio::test]
    async fn g11_cache_eviction_hint_omitted_when_no_request_id() {
        let sink = Arc::new(InMemorySink::new());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = arc_mock_spawner(); // default Completed → last_request_id None
        let bctx = wired_ctx_with_bus(spawner, bus).await;
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        tool.call(
            json!({ "description": "d", "subagent_type": "general-purpose", "prompt": "go" }),
            ctx,
            fresh_tx(),
        )
        .await
        .expect("spawn completes");

        let events = sink.events().await;
        assert!(
            !events.iter().any(|e| e.name == "tengu_cache_eviction_hint"),
            "no cache eviction hint when last_request_id absent"
        );
    }

    // =====================================================================
    // #2/G13 — async (run_in_background) surfaces a CLEAR error when the
    // spawn_async seam is unwired (no silent sync fallback).
    // =====================================================================
    #[tokio::test]
    async fn g13_async_unwired_returns_clear_error_not_sync() {
        let spawner = arc_mock_spawner(); // default spawn_async → Internal error
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            arc_mock_budget(u64::MAX),
        );
        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let err = tool
            .call(
                json!({
                    "description": "d",
                    "subagent_type": "general-purpose",
                    "prompt": "go",
                    "run_in_background": true
                }),
                ctx,
                fresh_tx(),
            )
            .await
            .expect_err("async unwired → clear error, not a sync result");
        let msg = format!("{err}");
        assert!(msg.contains("async spawn failed"), "clear async error: {msg}");
        // The SYNC spawn must NOT have been invoked (no silent fallback).
        assert!(
            spawner.invocations().is_empty(),
            "async path must not fall through to a sync spawn"
        );
    }
}
