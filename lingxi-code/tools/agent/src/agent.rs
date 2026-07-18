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
use telemetry::tengu::agent::{
    CACHE_EVICTION_HINT, SUBAGENT_OUTPUT_FLAGGED, TOOL_COMPLETED, TOOL_SELECTED, TOOL_TERMINATED,
};
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

/// claude `Agt()` (`AgentTool.tsx`): normalize a subagent-type candidate for the
/// fuzzy fallback match — lowercase, then strip all whitespace, dashes (Unicode
/// `Pd`), and underscores, so `"Explore"` / `"explore"` / `"ex-plore"` /
/// `"general_purpose"` collapse to a comparable key.
///
/// claude additionally applies `NFKC` first; agent-type names are ASCII (where
/// NFKC is the identity), so it is omitted here — add `unicode-normalization`
/// if non-ASCII custom agent types ever need compatibility folding.
fn normalize_agent_type(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !(c.is_whitespace() || *c == '_' || is_pd_dash(*c)))
        .collect()
}

/// Unicode `Pd` (dash punctuation) membership test for [`normalize_agent_type`].
fn is_pd_dash(c: char) -> bool {
    matches!(
        c,
        '-' | '\u{058A}' | '\u{05BE}' | '\u{1400}' | '\u{1806}' | '\u{2010}'
            ..='\u{2015}'
                | '\u{2E17}'
                | '\u{2E1A}'
                | '\u{2E3A}'
                | '\u{2E3B}'
                | '\u{2E40}'
                | '\u{301C}'
                | '\u{3030}'
                | '\u{30A0}'
                | '\u{FE31}'
                | '\u{FE32}'
                | '\u{FE58}'
                | '\u{FE63}'
                | '\u{FF0D}'
    )
}

/// `uZc` (claude-code): the `name` regex body — first char a letter/digit, then
/// up to 63 more of letter / digit / underscore / hyphen (max 64 chars total).
/// Emitted verbatim as the wire `input_schema` `name.pattern` (zod-to-json-schema
/// `case"regex"` → `addPattern`) AND enforced at the tool boundary by
/// [`validate_agent_name`].
const AGENT_NAME_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$";

/// `K9` (claude-code): the reserved agent name. `SendMessage` routes it to the
/// main conversation, so a spawned agent may not claim it.
const RESERVED_AGENT_NAME: &str = "main";

/// `.regex(uZc)` message — the byte-exact zod validation message for a name that
/// violates [`AGENT_NAME_PATTERN`].
const AGENT_NAME_REGEX_MESSAGE: &str = "name must start with a letter or digit and contain only letters, digits, underscores, or hyphens (max 64 chars)";

/// `.refine(t=>t!==K9)` message — byte-exact (em-dash is U+2014); the literal
/// `"main"` is [`RESERVED_AGENT_NAME`] interpolated as `${K9}`.
fn reserved_agent_name_message() -> String {
    format!(
        "\"{RESERVED_AGENT_NAME}\" is reserved \u{2014} SendMessage routes it to the main conversation"
    )
}

/// `uZc.test(name)` — hand-rolled `/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/` (no `regex`
/// crate dep): a non-empty name of at most 64 chars whose first char is ASCII
/// alnum and whose remainder is ASCII alnum / `_` / `-`. The char class is
/// ASCII-only, so any non-ASCII byte fails the class (and the match); operating
/// on bytes is therefore equivalent to JS's UTF-16 length counting.
fn matches_agent_name_pattern(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Port of the `name` zod chain `z.string().regex(uZc).refine(t=>t!==K9)` (the
/// binary's `exy` schema): validate a spawned-agent name at the tool boundary.
/// Returns the byte-exact zod message on failure — the `.regex` message first
/// (zod evaluates `.regex` before `.refine`), then the reserved-name message.
/// `Ok(())` when the name is well-formed and not reserved.
fn validate_agent_name(name: &str) -> Result<(), String> {
    if !matches_agent_name_pattern(name) {
        return Err(AGENT_NAME_REGEX_MESSAGE.to_string());
    }
    if name == RESERVED_AGENT_NAME {
        return Err(reserved_agent_name_message());
    }
    Ok(())
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
                "description": "Optional model override for this agent. Takes precedence over the agent definition's model frontmatter. If omitted, uses the agent definition's model, or inherits from the parent. Ignored for subagent_type: \"fork\" — forks always inherit the parent model."
            },
            "run_in_background": {
                "type": "boolean",
                "description": "Agents run in the background by default; you will be notified when one completes. Set to false to run this agent synchronously when you need its result before continuing."
            },
            "name": {
                "type": "string",
                // `z.string().regex(uZc)` → zod-to-json-schema `addPattern`
                // (`case"regex"`), so the wire input_schema carries the regex body
                // verbatim (`uZc=/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/`). The
                // `.refine(t => t !== "main")` reserved-name check has NO JSON
                // Schema equivalent and is enforced at the tool boundary in
                // `call` (see `AGENT_NAME_PATTERN` / `validate_agent_name`).
                "pattern": AGENT_NAME_PATTERN,
                "description": "Name for the spawned agent. Makes it addressable via SendMessage({to: name}) while running."
            },
            "team_name": {
                "type": "string",
                "description": "Deprecated; ignored. The session has a single implicit team."
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

/// The MODEL-FACING input schema = [`AGENT_INPUT_SCHEMA`] with `cwd` removed.
///
/// claude resolves the AgentTool's advertised schema as `yJp().omit({cwd:!0})`
/// (`eEo`): `cwd` exists on the call signature — set internally by
/// `isolation:"worktree"` or an explicit override — but is NEVER advertised to
/// the model, so the model cannot pass it. `AGENT_INPUT_SCHEMA` (with `cwd`)
/// stays the canonical/full schema for deserialization; this projection is what
/// [`Tool::input_schema`] exposes.
///
/// claude additionally omits `run_in_background` when background tasks are
/// disabled or on the pro plan (`K8t||MY() ? e.omit({run_in_background:!0}) : e`);
/// that is runtime-conditional and `input_schema(&self)` has no context, so it
/// is left in place (deferred).
static AGENT_INPUT_SCHEMA_MODEL: Lazy<Value> = Lazy::new(|| {
    let mut schema = AGENT_INPUT_SCHEMA.clone();
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        props.remove("cwd");
    }
    schema
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
    worktree_info: Option<(&str, &str)>,
) -> String {
    // contentOrMarker (AgentTool.tsx:1347-1350).
    let content_or_marker: Vec<String> = if content_texts.is_empty() {
        vec!["(Subagent completed but returned no output.)".to_string()]
    } else {
        content_texts.to_vec()
    };

    // One-shot built-ins skip the trailer when there is no worktree info
    // (AgentTool.tsx:1356). A KEPT worktree (the agent made changes) appends its
    // path + branch (byte-exact `\nworktreePath: …\nworktreeBranch: …`,
    // AgentTool.tsx:1368-1370) right before the `<usage>` block; `None` (no
    // worktree, or a clean one that was removed) keeps the trailer empty.
    let worktree_info_text = match worktree_info {
        Some((path, branch)) => {
            format!("\nworktreePath: {path}\nworktreeBranch: {branch}")
        }
        None => String::new(),
    };
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
        "agentId: {agent_id} (use SendMessage with to: '{agent_id}', summary: '<5-10 word recap>' to continue this agent){worktree_info_text}\n<usage>subagent_tokens: {total_tokens}\ntool_uses: {total_tool_use_count}\nduration_ms: {total_duration_ms}</usage>"
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

/// Normalize a subagent `description` the way the binary does — `replace(/\s+/g,
/// " ").trim()`: collapse every run of whitespace to a single space and trim the
/// ends. (`split_whitespace` does both; it tracks Unicode White_Space, matching
/// JS `\s` for the typical ASCII description.)
fn normalize_description_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Default per-session subagent spawn cap (claude 2.1.212 `ofg = 200`).
const MAX_SUBAGENTS_PER_SESSION_DEFAULT: u64 = 200;

/// Resolve the per-session subagent spawn cap from a raw
/// `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION` value (claude 2.1.212 `xtu()` =
/// `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION ?? 200`). Split from the env read for
/// testability. An unset OR unparseable value falls back to the default 200 (the
/// binary keeps a non-numeric env string, whose numeric comparison never trips
/// the cap; treating garbage as "default 200" is the faithful common-case
/// behavior and avoids a silently-disabled cap).
fn max_subagents_per_session_from(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(MAX_SUBAGENTS_PER_SESSION_DEFAULT)
}

/// The live per-session subagent spawn cap (claude 2.1.212 `xtu()`).
fn max_subagents_per_session() -> u64 {
    max_subagents_per_session_from(
        std::env::var("CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION")
            .ok()
            .as_deref(),
    )
}

/// The parent / main-loop model a spawn's `AgentModel::Inherit` + bare family
/// aliases resolve against — claude-code `AgentTool.tsx:418`
/// `getAgentModel(selectedAgent.model, toolUseContext.options.mainLoopModel, …)`.
///
/// At the top level `ToolUseContext.options.main_loop_model` is the LIVE session
/// model (the orchestrator sets it from `session.model`, updated by `/model`
/// switches / resume), so a mid-session model change is reflected in subsequently
/// spawned subagents. On a NESTED spawn the `RegistryToolInvoker` seeds it from
/// the dispatching subagent's own resolved model (claude `runAgent.ts:678`
/// `mainLoopModel: resolvedAgentModel`), so a child inherits its IMMEDIATE
/// parent's model. Threaded onto `SubagentSpawnRequest.parent_model_override`,
/// where it takes precedence over the spawner's boot/live `default_model`. An
/// empty model (the invoker's legacy placeholder / an unset context) yields
/// `None`, leaving the spawner to fall back to its own default.
fn main_loop_model_parent(ctx: &ToolUseContext) -> Option<String> {
    let model = ctx.options.main_loop_model.trim();
    if model.is_empty() || model == "subagent" {
        None
    } else {
        Some(model.to_string())
    }
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

    /// Build the dynamic Agent tool prompt, porting claude-code v2.1.193's
    /// `getPrompt` (binary `bin/claude.exe` offset ~208233723). 2.1.193 ALWAYS
    /// externalizes the agent catalog to a per-turn `agent_listing_delta`
    /// `<system-reminder>` attachment (built by the orchestrator,
    /// [`crate::PoolSubagentSpawner`]-fed); the tool DESCRIPTION carries only the
    /// static pointer line "Available agent types are listed in <system-reminder>
    /// messages in the conversation." — there is NO inline-catalog variant in the
    /// description. The `should_inject_agent_list_in_messages()` gate (now default
    /// ON, see [`traits::subagent_spawn`]) reflects that: ON ⇒ pointer (the
    /// 2.1.193 default); an explicit `LINGXI_AGENT_LIST_IN_MESSAGES=false`
    /// opt-out keeps a LEGACY inline-catalog body (not a 2.1.193 form).
    ///
    /// The body is the 2.1.193 SHORT form (`if(c)` branch — the live default):
    /// intro + `## When to use` + four terse bullets. (The FULL `if(c)`-false
    /// form — `## When not to use` + `## Usage notes` + `<example>`s — is the
    /// alternate; we render the SHORT form, which is what a live 2.1.193 session
    /// emits.)
    ///
    /// `is_coordinator` selects the slim coordinator prompt (intro only; the
    /// coordinator system prompt already covers usage / examples).
    ///
    /// The `d` pro-plan gate (`vi()==="pro"`) IS modeled: a `pro` subscription
    /// (read from the process-global [`traits::subscription::is_pro_plan`]) injects
    /// the "Do not spawn agents unless the user asks" block after the catalog
    /// pointer line and suppresses `## When to use`. It is inert until a
    /// composition root resolves the plan via
    /// [`traits::subscription::set_current_subscription`] (subscription resolution
    /// may be unwired ⇒ `None` ⇒ no block, matching the binary's unknown-plan
    /// default).
    ///
    /// The `o` fork-subagent gate (`isForkSubagentEnabled`) IS modeled: when fork
    /// is enabled (`is_fork_subagent_enabled(is_coordinator, is_non_interactive)`,
    /// reading the process-global [`traits::session_flags::is_non_interactive_session`]),
    /// the subagent_type sentence explains `"fork"`, a fork addendum follows
    /// `## When to use`, and the SendMessage bullet gains the `(except
    /// subagent_type: "fork", …)` qualifier. Inert unless `LINGXI_FORK_SUBAGENT`
    /// is set (default OFF) ⇒ the non-fork text, byte-identical to the pre-F4 prompt.
    ///
    /// Deferred vs binary (no behavioral surface here): the `m` embedded-grep hint
    /// swap, teammate (`yB`/`_m`) notes, and the remote-isolation (`V8t`) note. The
    /// fabricated "# MCP Servers" note (NOT present in the 2.1.193 binary) is
    /// removed; `_mcp_server_names` is retained for signature stability.
    fn build_prompt(
        agents: &[traits::subagent_spawn::SubagentListingEntry],
        _mcp_server_names: &[String],
        is_coordinator: bool,
    ) -> String {
        // Catalog placement (binary intro `p`): the 2.1.193 default externalizes
        // the catalog to the orchestrator's `<system-reminder>` attachment, so the
        // description carries only the static pointer line. A LEGACY inline body is
        // retained behind an explicit `LINGXI_AGENT_LIST_IN_MESSAGES=false`
        // opt-out (gate OFF) — not a 2.1.193 form, but a usable escape hatch.
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

        // Pro-plan gate `d` (binary `d=vi()==="pro"?<block>:""`): on the `pro`
        // plan, discourage spawning. Read from the process-global subscription
        // (the port's `vi()` analog); `None`/non-pro ⇒ empty (the common case,
        // since subscription resolution may be unwired ⇒ gate inert, matching the
        // binary's unknown-plan default). The block is injected right after the
        // catalog pointer line, and (below) it SUPPRESSES the `## When to use`
        // section — both per the binary's `${d}` / `${d?"":…}` placements.
        let pro_block = if traits::subscription::is_pro_plan() {
            // Binary `d=Pi()==="pro"?`\n\n**Do not spawn…`:""` — DOUBLE leading `\n`
            // (injected as `…conversation.${d}\n\n${subagent}`; od -c verified on
            // 2.1.195 @211615145).
            "\n\n**Do not spawn agents unless the user asks.** Each spawn starts cold and re-derives context you already have — it's the expensive path on this plan. A task with \"multiple angles,\" \"thorough,\" or several parts is not a request to spawn; handle it inline with your own tools. Only use this tool when the user explicitly says to use a subagent, or names one of the available agent types."
        } else {
            ""
        };

        // Fork-subagent gate `o` (binary `o=isForkSubagentEnabled()`): when fork
        // is enabled, the subagent_type sentence explains the `"fork"` type, the
        // body gains a fork addendum, and the SendMessage bullet notes the fork
        // exception. The gate is `is_fork_subagent_enabled(is_coordinator,
        // is_non_interactive)` — env `LINGXI_FORK_SUBAGENT` AND !coordinator
        // AND !non-interactive; the non-interactive flag is read from the
        // process-global session flag (the port's `getIsNonInteractiveSession()`
        // analog, set by `ConversationOrchestrator::new`). Default OFF ⇒ the
        // non-fork text below, byte-identical to the pre-F4 prompt.
        let is_fork = traits::fork_subagent::is_fork_subagent_enabled(
            is_coordinator,
            traits::session_flags::is_non_interactive_session(),
        );

        // Subagent_type sentence — fork variant (binary `${o?…:…}`).
        let subagent_sentence = if is_fork {
            format!(
                "When using the {AGENT_TOOL_NAME} tool, specify a subagent_type to select an agent: `\"fork\"` forks yourself (the fork inherits your full conversation context and always runs on your model — a `model` override is ignored); any other type — or omitting it — starts a fresh agent (general-purpose by default)."
            )
        } else {
            format!(
                "When using the {AGENT_TOOL_NAME} tool, specify a subagent_type parameter to select which agent type to use. If omitted, the general-purpose agent is used."
            )
        };

        // Intro `p` (binary JS source @211615462): the catalog line sits between
        // the two intro sentences (with the `${d}` pro-block appended to it); the
        // subagent_type sentence closes it.
        // Binary `p`: `…available to it.\n\nAvailable agent types…conversation.${d}\n\n${subagent}`
        // — DOUBLE `\n` separators around the catalog line + `${d}` pro-block
        // (od -c verified on 2.1.195; an earlier pass misread the `strings` dump
        // as single `\n`).
        let intro = format!(
            "Launch a new agent to handle complex, multi-step tasks. Each agent type has specific capabilities and tools available to it.\n\n\
{agent_list_section}{pro_block}\n\n\
{subagent_sentence}"
        );

        // Coordinator mode gets the slim intro only (binary `if(t)return p`).
        if is_coordinator {
            return intro;
        }

        // Non-coordinator SHORT form (binary `if(c)` branch — the live 2.1.193
        // default): `## When to use` + four terse bullets. NO `## When not to
        // use`, NO `## Usage notes`, NO `<example>`s (those are the FULL form).
        // Em-dashes are U+2014. The `run_in_background` bullet is the
        // background-enabled 2.1.206 default (the binary's `h`; the
        // `LINGXI_DISABLE_BACKGROUND_TASKS` / teammate suppressions are not
        // modeled here).
        //
        // `## When to use` is SUPPRESSED on the pro plan (binary `${d?"":…}`):
        // when the pro-block is present, the discouragement replaces the
        // when-to-use guidance. The four bullets are NOT gated and always render.
        let when_to_use = if pro_block.is_empty() {
            // Binary `${d?"":`\n\n## When to use\n\nReach…`}` — DOUBLE `\n` before
            // the heading AND before the paragraph (od -c verified on 2.1.195).
            "\n\n## When to use\n\n\
Reach for this when the task matches an available agent type, when you have independent work to run in parallel, or when answering would mean reading across several files — delegate it and you keep the conclusion, not the file dumps. For a single-fact lookup where you already know the file, symbol, or value, search directly. Once you've delegated a search, don't also run it yourself — wait for the result."
        } else {
            ""
        };
        // Fork addendum (binary `${o?…:""}`), after `## When to use`, before the
        // bullets.
        let fork_addendum = if is_fork {
            // Binary `${o?`\n\nA fork runs…`:""}` — DOUBLE leading `\n` (od -c
            // verified on 2.1.195).
            "\n\nA fork runs in the background and keeps its tool output out of your context. If you are the fork, execute directly — don't re-delegate."
        } else {
            ""
        };
        // SendMessage bullet — fork qualifier (binary `${o?' (except …)':""}`).
        let send_message_bullet = if is_fork {
            format!(
                "- Use SendMessage with the agent's ID or name to continue a previously spawned agent with its context intact; a new {AGENT_TOOL_NAME} call starts fresh (except subagent_type: \"fork\", which inherits your context)."
            )
        } else {
            format!(
                "- Use SendMessage with the agent's ID or name to continue a previously spawned agent with its context intact; a new {AGENT_TOOL_NAME} call starts fresh."
            )
        };
        // Binary: `…re-delegate.`:""}\n\n- The agent's final message…\n- Use…`
        // — DOUBLE `\n` before the FIRST bullet, SINGLE `\n` between subsequent
        // bullets (od -c verified on 2.1.195).
        //
        // The agent-definition bullet is an unconditional fixed literal between
        // the SendMessage bullet and the isolation bullet (2.1.207 binary @
        // ~222815108: `- Each agent type's model, reasoning effort, and tools come
        // from its definition (\`.claude/agents/*.md\` frontmatter or SDK
        // \`agents\`).`). Ported verbatim except the path is rebranded
        // `.claude/agents/*.md` → `.lingxi/agents/*.md` per the accepted .lingxi
        // naming divergence (cf. sandbox-runtime path_utils.rs `.lingxi/agents`).
        format!(
            "{intro}{when_to_use}{fork_addendum}\n\n\
- The agent's final message is returned to you as the tool result; it is not shown to the user — relay what matters.\n\
{send_message_bullet}\n\
- Each agent type's model, reasoning effort, and tools come from its definition (`.lingxi/agents/*.md` frontmatter or SDK `agents`).\n\
- `isolation: \"worktree\"` gives the agent its own git worktree (auto-cleaned if unchanged).\n\
- Subagents run in the background by default; you'll be notified when one completes. Pass `run_in_background: false` for a synchronous run when you need the result before continuing."
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
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
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

    /// (2.1.212) Emit claude `tengu_subagent_output_flagged` (`tHu`): the output
    /// guard neutralized control tags and/or flagged escalation patterns in a
    /// completed subagent's returned text (`surface:'finalize'`). Only called
    /// when at least one *reportable* pattern matched.
    async fn emit_subagent_output_flagged(
        bus: &Arc<AnalyticsBus>,
        agent_id: &str,
        result: &traits::subagent_output_guard::SanitizeResult,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "agent_id".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(agent_id.to_string()).into_inner(),
            ),
        );
        md.insert(
            "surface".into(),
            AnalyticsValue::String(Verified::assert_safe("finalize".to_string()).into_inner()),
        );
        // `D5(Oo(…))`: sorted-unique reportable pattern / category names, joined
        // with `,`. The names are a fixed vocabulary (not user data).
        md.insert(
            "patterns".into(),
            AnalyticsValue::String(
                Verified::assert_safe(result.reportable_patterns_sorted().join(",")).into_inner(),
            ),
        );
        md.insert(
            "categories".into(),
            AnalyticsValue::String(
                Verified::assert_safe(result.reportable_categories_sorted().join(",")).into_inner(),
            ),
        );
        md.insert(
            "match_count".into(),
            AnalyticsValue::Int(result.reportable_match_count() as i64),
        );
        bus.log_event(SUBAGENT_OUTPUT_FLAGGED, md).await;
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
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
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
    ///
    /// `resolved_cwd` / `agent_worktree` are the isolation products the caller
    /// resolved BEFORE branching (claude 2.1.207 creates the worktree before
    /// the sync/async split): the effective cwd (`cwd ?? worktreePath`) rides
    /// `request.cwd` so the background agent's tools operate in the worktree,
    /// and the handle rides `request.worktree` so the detached lifecycle owner
    /// runs the terminal keep/cleanup judgment (claude's `getWorktreeResult`
    /// closure) — NOT here at launch time.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_async(
        &self,
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        started: Instant,
        spawner: &dyn traits::subagent_spawn::SubagentSpawner,
        parsed: &AgentToolInput,
        effective_type: &str,
        selected: &traits::subagent_spawn::SelectedAgentMeta,
        is_fork: bool,
        ctx: &ToolUseContext,
        budget: Arc<dyn traits::budget::BudgetEnforcerHandle>,
        parent_registry: Arc<tool_api::ToolRegistry>,
        effective_isolation: Option<String>,
        resolved_cwd: Option<String>,
        agent_worktree: Option<traits::worktree::WorktreeHandle>,
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
            model_profile: None,
            run_in_background: true,
            name: if is_fork { None } else { parsed.name.clone() },
            team_name: if is_fork {
                None
            } else {
                parsed.team_name.clone()
            },
            mode: if is_fork { None } else { parsed.mode.clone() },
            isolation: if is_fork { None } else { effective_isolation },
            // The RESOLVED cwd (explicit `cwd` override, else the isolation
            // worktree's path — claude `cwd ?? worktreePath`); `None` on fork.
            cwd: resolved_cwd,
            // Ownership transfer of the isolation worktree (claude
            // `getWorktreeResult` handed to the detached task): the local_agent
            // handler runs the terminal keep/cleanup judgment.
            worktree: agent_worktree,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            // The Agent (Task) tool has no structured-output schema param.
            schema: None,
            effort: None,
            // Thread the originating tool_use_id so the backgrounded agent's
            // `<task-notification>` carries `<tool-use-id>` (claude-code parity).
            tool_use_id: ctx
                .tool_use_id
                .as_ref()
                .map(std::string::ToString::to_string),
            // Workflow-only spawn seam (defaults; the Agent tool doesn't use the
            // workflow-subagent prompt override/addendum or disallow-union).
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            // Child depth = this agent's depth + 1 (claude `spawnDepth =
            // z6(parentContext) + 1`). The spawner stamps it onto the child's
            // SubagentContext; the resolver gates the child's `Agent` at depth<5.
            depth: ctx.depth + 1,
            // Parent / main-loop model for this child's `Inherit` + family aliases
            // (claude `getAgentModel(…, toolUseContext.options.mainLoopModel, …)`,
            // AgentTool.tsx:418) — see the sync spawn path for the full note.
            parent_model_override: main_loop_model_parent(ctx),
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
                    "Async agent launched successfully. (This tool result is internal metadata — never quote or paste any part of it, including the agentId below, into a user-facing reply.)\nagentId: {agent_id_str} (internal ID - do not mention to user. Use SendMessage with to: '{agent_id_str}', summary: '<5-10 word recap>' to continue this agent.)\nThe agent is working in the background. You will be notified automatically when it completes."
                );
                let instructions = if can_read_output_file {
                    // claude `canReadOutputFile` branch (AgentTool.tsx, v2.1.193):
                    // warn the model NOT to read the `.output` file — it is the
                    // full subagent JSONL transcript and would overflow context.
                    // (`${Ds}` resolves to `Read`.)
                    format!(
                        "Do not duplicate this agent's work — avoid working with the same files or topics it is using.\noutput_file: {output_file}\nDo NOT Read or tail this file via the shell tool — it is the full subagent JSONL transcript and reading it will overflow your context. If the user asks for progress, say the agent is still running; you'll get a completion notification."
                    )
                } else {
                    "In your own words, briefly tell the user what you launched — do not echo this tool result. Agent results will arrive in a subsequent message.".to_string()
                };
                let model_content = format!("{prefix}\n{instructions}");
                Ok(ToolCallResult {
                    data: json!({
                        "isAsync": true,
                        "status": "async_launched",
                        "agentId": agent_id_str,
                        "description": parsed.description,
                        // claude `async_launched` payload includes the resolved
                        // model id (`resolvedModel: U`).
                        "resolvedModel": selected.resolved_model.clone(),
                        "prompt": parsed.prompt,
                        "outputFile": launch.output_file,
                        "canReadOutputFile": can_read_output_file,
                        "model_content": model_content,
                    }),
                    model_content: None,
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
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
        // Binary `getActivityDescription(e){return e?.description?.replace(/\s+/g,
        // " ").trim()||"Running task"}` — collapse internal whitespace runs to a
        // single space and trim; fall back to "Running task" when absent OR when
        // the normalized value is empty (JS `||`, empty string is falsy). (The
        // leaked TS `?? 'Running task'` was stale — the 2.1.195 binary normalizes.)
        let desc = input
            .get("description")
            .and_then(Value::as_str)
            .map(normalize_description_ws)
            .filter(|s| !s.is_empty());
        Some(desc.unwrap_or_else(|| "Running task".to_string()))
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
        // claude advertises `yJp().omit({cwd:!0})` — the model-facing schema
        // never exposes `cwd` (set internally by isolation / explicit override).
        &AGENT_INPUT_SCHEMA_MODEL
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
        // Coordinator-mode signal (TS `isCoordinatorMode()`), read LIVE from the
        // `coordinator_mode` seam on `BuiltinToolContext` (`None` ⇒ not
        // coordinator). Selects the slim coordinator prompt in `build_prompt`.
        let is_coordinator = self
            .ctx
            .coordinator_mode
            .as_ref()
            .is_some_and(|m| m.is_enabled());
        Self::build_prompt(&agents, &mcp_server_names, is_coordinator)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = Self::fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        // 1. Parse input.
        let mut parsed: AgentToolInput = match serde_json::from_value(input) {
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
        // Binary `AgentTool.call`: `n=n.replace(/\s+/g," ").trim()` — normalize the
        // `description` ONCE at entry so every downstream use (the spawn request,
        // the async-launch payload, the completed `data.description`) carries the
        // collapsed/trimmed value.
        parsed.description = normalize_description_ws(&parsed.description);

        // `name` zod chain `z.string().regex(uZc).refine(t=>t!==K9)` (binary
        // `exy`). The wire `pattern` (on the advertised `name` property) covers
        // the regex at the turn-loop input gate, but `.refine()` (reserved
        // "main") has NO JSON Schema form, so enforce the FULL chain here at the
        // tool boundary with the byte-exact zod messages. A spawn naming its agent
        // "main" would collide with SendMessage's main-conversation routing, so
        // this is behavioral as well as byte parity.
        if let Some(name) = parsed.name.as_deref() {
            if let Err(msg) = validate_agent_name(name) {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "invalid_input",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(msg));
            }
        }

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
        // coordinator mode + non-interactive sessions. `is_coordinator` is read
        // LIVE from the `coordinator_mode` seam on `BuiltinToolContext` (`None` ⇒
        // not coordinator), so a mid-session mode switch immediately disables
        // forking. The non-interactive arm is honored via
        // `ctx.options.is_non_interactive_session`.
        let is_coordinator = self
            .ctx
            .coordinator_mode
            .as_ref()
            .is_some_and(|m| m.is_enabled());
        let is_fork = parsed.subagent_type.is_none()
            && traits::fork_subagent::is_fork_subagent_enabled(
                is_coordinator,
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
                        let is_denied = |t: &str| denied.iter().any(|d| d.as_str() == t);
                        let available = || {
                            listing
                                .iter()
                                .filter(|a| !denied.iter().any(|d| d == &a.agent_type))
                                .map(|a| a.agent_type.clone())
                                .collect::<Vec<_>>()
                        };
                        // claude `Agt()` normalized fallback (AgentTool.tsx): when the
                        // exact name misses, match candidates whose normalized form
                        // (NFKC/lowercase/strip ws+dash+underscore) equals the
                        // candidate's. A single available match resolves; multiple is
                        // an ambiguity error; zero (or a single denied match) falls
                        // through to not-found.
                        let norm = normalize_agent_type(explicit);
                        let matches: Vec<String> = listing
                            .iter()
                            .filter(|a| normalize_agent_type(&a.agent_type) == norm)
                            .map(|a| a.agent_type.clone())
                            .collect();
                        if matches.len() > 1 {
                            let avail_matches: Vec<&String> =
                                matches.iter().filter(|&m| !is_denied(m)).collect();
                            let matches_disp = matches
                                .iter()
                                .map(|m| {
                                    if is_denied(m) {
                                        format!("{m} (unavailable)")
                                    } else {
                                        m.clone()
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            let tail = if avail_matches.is_empty() {
                                format!(
                                    "None of these are available. Available agents: {}",
                                    available().join(", ")
                                )
                            } else {
                                format!(
                                    "Use the exact name: {}",
                                    avail_matches
                                        .iter()
                                        .map(|s| s.as_str())
                                        .collect::<Vec<_>>()
                                        .join(" or ")
                                )
                            };
                            Self::emit_failed(
                                &bus,
                                &invocation_id,
                                "agent_type_ambiguous",
                                started.elapsed().as_millis() as u64,
                            )
                            .await;
                            return Err(ToolError::InvalidInput(format!(
                                "Agent type '{explicit}' is ambiguous — matches {matches_disp}. {tail}"
                            )));
                        }
                        if matches.len() == 1 && !is_denied(&matches[0]) {
                            matches.into_iter().next().unwrap()
                        } else {
                            Self::emit_failed(
                                &bus,
                                &invocation_id,
                                "agent_type_not_found",
                                started.elapsed().as_millis() as u64,
                            )
                            .await;
                            return Err(ToolError::InvalidInput(format!(
                                "Agent type '{explicit}' not found. Available agents: {}",
                                available().join(", ")
                            )));
                        }
                    }
                }
            }
        };

        // Per-session subagent spawn cap (claude 2.1.212 `AgentTool.call` `N()`):
        // before every spawn, reject the launch once the session has already
        // spawned `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION` agents (default 200),
        // otherwise bump the counter. The counter lives on the session
        // `taskRegistry` (`getTotalAgentSpawns` / `incrementTotalAgentSpawns`) so
        // it is shared across every `AgentTool::call` in the session and across
        // the fork / regular / teammate spawn paths. Matches the binary's
        // placement: after type resolution, before the required-MCP gate and the
        // sync/async dispatch. When no registry is wired (defaulted seam) the cap
        // is inert.
        if let Some(registry) = &self.ctx.task_registry {
            let cap = max_subagents_per_session();
            let spawned = registry.get_total_agent_spawns();
            if spawned >= cap {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "subagent_count_cap",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "Subagent spawn limit reached ({spawned} of {cap} agents spawned). \
Complete the remaining work directly with your tools instead of spawning more agents. \
If more agents are genuinely needed, ask the user to raise CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION."
                )));
            }
            registry.increment_total_agent_spawns();
        }

        // 4. (G3) Required-MCP-servers gate (claude AgentTool.tsx:367-409): if the
        // resolved agent declares `required_mcp_servers`, every required pattern
        // must match an MCP server that currently exposes tools (connected AND
        // authenticated). A missing requirement is a hard error listing the
        // unmatched patterns + the servers that DO have tools.
        //
        // Pending-wait (claude AgentTool.tsx): if any REQUIRED server is currently
        // PENDING (connecting / awaiting-OAuth / reconnecting), wait up to 30s
        // (500ms poll) for it to either expose tools or fail BEFORE checking
        // availability — so an agent that needs an OAuth/slow-login MCP server is
        // not spuriously failed mid-connect. The loop stops early when a required
        // server FAILS (no point waiting) or when none remain pending. Reads the
        // registry's INTERNAL pending/failed state (the public `McpStatus` UI
        // projection collapses Connecting/AwaitingOAuth/Reconnecting →
        // `Disconnected`; `servers_pending`/`servers_failed` read the real state).
        let required_mcp_servers = spawner.resolve_required_mcp_servers(&effective_type).await;
        if !required_mcp_servers.is_empty() {
            if let Some(reg) = &self.ctx.mcp_registry {
                // claude `he.name.toLowerCase().includes(pattern.toLowerCase())`:
                // a server name matches a required pattern by case-insensitive
                // substring. Reused for the pending + failed lists.
                let any_required = |names: &[String]| {
                    names.iter().any(|name| {
                        let n = name.to_lowercase();
                        required_mcp_servers
                            .iter()
                            .any(|pat| n.contains(&pat.to_lowercase()))
                    })
                };
                if any_required(&reg.servers_pending().await) {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    while std::time::Instant::now() < deadline {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        // A required server FAILED → stop waiting.
                        if any_required(&reg.servers_failed().await) {
                            break;
                        }
                        // No required server still pending → stop waiting.
                        if !any_required(&reg.servers_pending().await) {
                            break;
                        }
                    }
                }
            }
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
        // Claude Code 2.1.206 defaults a local subagent to background execution:
        // `run_in_background !== false`, with an agent definition's
        // `background: true` also forcing the async path. The binary then gates
        // the local group on background tasks being enabled. The
        // agent definition's `background` frontmatter flag is surfaced on
        // `SelectedAgentMeta.background`, so a `background: true` agent dispatches
        // async even when the caller omits `run_in_background`. The env kill-switch
        // forces the whole local-async group off — an explicit `run_in_background:
        // true` then runs SYNCHRONOUSLY. (claude gates only the LOCAL group with
        // `!dqt`; the remote path is separate and out of scope here.) The same
        // `run_in_background` value drives BOTH the telemetry `is_async` flag and
        // the async-dispatch branch below.
        let background_tasks_disabled = traits::env::is_env_truthy(
            std::env::var("LINGXI_DISABLE_BACKGROUND_TASKS")
                .ok()
                .as_deref(),
        );
        let run_in_background = (parsed.run_in_background.unwrap_or(true) || selected.background)
            && !background_tasks_disabled;
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

        // Worktree / cwd isolation — resolved BEFORE the sync/async branch,
        // matching claude 2.1.207 (`ye=null; if(Y==="worktree") ye=await
        // createAgentWorktree(agentWorktreeSlug(id))` precedes the
        // `run_in_background` branch, and the effective cwd `ge = l ??
        // ye?.worktreePath` is threaded into BOTH). When the caller requests
        // `isolation:"worktree"` or the selected agent definition declares
        // `isolation: worktree` (non-fork), create a git worktree (slug
        // `agent-<id>` → branch `worktree-agent-<id>` under `.lingxi/worktrees/`,
        // matching claude's scheme) and run the agent in it; an explicit `cwd`
        // takes precedence as the run dir (claude `cwd ?? worktreePath`).
        // `remote` is deferred (run local). The handle is held for the
        // post-completion keep/cleanup judgment: run HERE on the sync path, and
        // by the detached background lifecycle on the async path (claude hands
        // the `getWorktreeResult` closure to the task — see `dispatch_async`).
        // The model-facing `isolation` argument wins over the definition's
        // frontmatter; omitted args inherit `SelectedAgentMeta.isolation`.
        let effective_isolation = if is_fork {
            None
        } else {
            parsed
                .isolation
                .clone()
                .or_else(|| selected.isolation.clone())
        };
        let mut agent_worktree: Option<traits::worktree::WorktreeHandle> = None;
        let mut resolved_cwd: Option<String> = if is_fork { None } else { parsed.cwd.clone() };
        if effective_isolation.as_deref() == Some("worktree") {
            let slug = format!("agent-{invocation_id}");
            match self.ctx.worktree.create_worktree(&slug, None, &[]).await {
                Ok(handle) => {
                    if resolved_cwd.is_none() {
                        resolved_cwd = Some(handle.path.to_string_lossy().into_owned());
                    }
                    agent_worktree = Some(handle);
                }
                Err(e) => {
                    Self::emit_failed(
                        &bus,
                        &invocation_id,
                        "worktree_create_failed",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::Internal(format!(
                        "Cannot create agent worktree: {e}"
                    )));
                }
            }
        }

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
                    effective_isolation.clone(),
                    resolved_cwd,
                    agent_worktree,
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
            // fork prefix. LingXi's fork path deliberately carries no
            // teammate/isolation/cwd overrides (see the request fields below), so
            // there is no worktree to notice here — do NOT fabricate one.
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
            // resolved `cwd`/`worktree` are computed above before the sync/async
            // branch. Remote/team-specific execution remains a higher-level
            // router concern.
            description: Some(parsed.description.clone()),
            // Fork path sends `model: None` (claude `model: undefined`) so the
            // FORK_AGENT's `Inherit` resolves to the parent model unchanged; the
            // explicit-model override is honored only on the non-fork path.
            model: if is_fork { None } else { parsed.model.clone() },
            model_profile: None,
            // claude collapses `run_in_background === true`; absent ⇒ false.
            run_in_background: parsed.run_in_background.unwrap_or(false),
            // Fork path carries no teammate/isolation/cwd overrides.
            name: if is_fork { None } else { parsed.name.clone() },
            team_name: if is_fork {
                None
            } else {
                parsed.team_name.clone()
            },
            mode: if is_fork { None } else { parsed.mode.clone() },
            isolation: if is_fork {
                None
            } else {
                effective_isolation.clone()
            },
            // The RESOLVED cwd (the explicit `cwd` override, else the worktree
            // path for `isolation:"worktree"`) — the spawner sets
            // `SubagentContext.cwd` from this so the agent's tools operate there.
            cwd: resolved_cwd.clone(),
            // The resolved isolation worktree rides the request for shape
            // consistency with the async path; the SYNC judgment below stays
            // authoritative here (the spawner ignores the field).
            worktree: agent_worktree.clone(),
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
            // Sync spawn: no background task / notification, so no tool_use_id
            // to stamp (only the async/background path threads it).
            tool_use_id: None,
            // Workflow-only spawn seam (defaults; unused by the Agent tool).
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            // Child depth = this agent's depth + 1 (claude `spawnDepth =
            // z6(parentContext) + 1`). The spawner stamps it onto the child's
            // SubagentContext; the resolver gates the child's `Agent` at depth<5.
            depth: ctx.depth + 1,
            // The parent / main-loop model this child's `Inherit` + family aliases
            // resolve against (claude `getAgentModel(selectedAgent.model,
            // toolUseContext.options.mainLoopModel, …)`, AgentTool.tsx:418): the
            // LIVE session model at the top level (a mid-session `/model` switch is
            // reflected), or the IMMEDIATE parent subagent's resolved model on a
            // nested spawn (the RegistryToolInvoker seeds `main_loop_model` from the
            // dispatching runner's own model, mirroring runAgent.ts:678). When set
            // it takes precedence over the spawner's boot/live `default_model`.
            parent_model_override: main_loop_model_parent(&ctx),
        };

        // Nested-progress bridge: `spawn_with_progress` feeds one String line per
        // subagent tool call; forward each as a `ToolProgress` the turn loop
        // re-emits as `SubagentActivity`, so the subagent's work renders under
        // this Task cell. The forwarder ends when `spawn_with_progress` returns
        // (its `prog_tx` drops → `prog_rx` closes).
        let (prog_tx, mut prog_rx) = tokio::sync::mpsc::channel::<String>(64);
        let forward_progress = progress.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(line) = prog_rx.recv().await {
                let _ = forward_progress
                    .send(tool_api::progress::ToolProgress {
                        tool_use_id: protocol::ToolUseId::new(),
                        data: serde_json::json!({ "subagent_activity": line }),
                    })
                    .await;
            }
        });

        let outcome = spawner
            .spawn_with_progress(request, inherit, Some(prog_tx))
            .await;
        // `prog_tx` is now dropped → the forwarder drains and exits.
        let _ = forwarder.await;
        let duration_ms = started.elapsed().as_millis() as u64;

        // Worktree lifecycle (claude `fe()` / `getWorktreeResult`): once the
        // agent finished, KEEP the worktree (return its path + branch) if it
        // left changes, else REMOVE it (auto-clean). The judgment itself lives
        // in `traits::worktree::agent_worktree_result` (shared with the ASYNC
        // lifecycle owner in the local_agent task handler); it runs for ANY
        // outcome so a worktree never leaks on a failed/killed agent.
        let worktree_result: Option<(String, String)> = match &agent_worktree {
            Some(handle) => {
                traits::worktree::agent_worktree_result(self.ctx.worktree.as_ref(), handle).await
            }
            None => None,
        };

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
                let raw_content_texts = extract_content_texts(&content);

                let agent_id_str = agent_id.to_string();

                // (2.1.212) Indirect-prompt-injection hardening (claude
                // `ZDu`/`tHu`): run the output guard over the subagent's returned
                // text before it reaches the parent model — neutralize control /
                // model-layer tags, flag escalation patterns, and (when anything
                // reportable matched) prepend a warning block. The sanitized
                // blocks feed BOTH the result `content` array and the model-facing
                // string, and a `tengu_subagent_output_flagged` event is emitted.
                let sanitized = traits::subagent_output_guard::sanitize_blocks(&raw_content_texts);
                if sanitized.any_reportable() {
                    Self::emit_subagent_output_flagged(&bus, &agent_id_str, &sanitized).await;
                }
                let content_texts = sanitized.content;
                let content_blocks: Vec<Value> = content_texts
                    .iter()
                    .map(|t| json!({ "type": "text", "text": t }))
                    .collect();

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
                    worktree_result
                        .as_ref()
                        .map(|(p, b)| (p.as_str(), b.as_str())),
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
                let mut data = json!({
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
                });
                // claude spreads `worktreePath`/`worktreeBranch` into `data` ONLY
                // when the worktree was KEPT (the agent left changes).
                if let Some((path, branch)) = &worktree_result {
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert("worktreePath".into(), json!(path));
                        obj.insert("worktreeBranch".into(), json!(branch));
                    }
                }
                Ok(ToolCallResult {
                    data,
                    model_content: None,
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
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
#[path = "agent_test.rs"]
mod agent_test;
