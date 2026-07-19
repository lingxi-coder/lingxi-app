//! Static description of an agent (a.k.a. subagent type).
//!
//! An [`AgentDefinition`] is parsed from a markdown frontmatter file or
//! synthesized in code; it carries the tool policy, permission mode, model
//! preference, and worktree requirement that downstream subsystems consult
//! when a new agent is spawned. See spec §10.2.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Self-contained description of an agent type used by
/// [`crate::context::SubagentContext`] and [`crate::pool::StateMachinePool`].
///
/// In addition to the resolver-consumed fields (`tools`, `permission_mode`,
/// `model`, `worktree_requirement`), this struct also carries the full set of
/// claude-code `BaseAgentDefinition` frontmatter fields parsed from markdown /
/// JSON agent files: `disallowed_tools`, `skills`, `required_mcp_servers`,
/// `background`, `isolation`, `memory`, `effort`, `initial_prompt`, and
/// `color`. Several of these are stored-but-not-yet-consumed (field-level
/// parity); see the per-field rustdoc and `catalog.rs`. See spec §10.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Stable agent kind label (e.g. `"general"`, `"reviewer"`, `"fork"`).
    pub agent_type: String,
    /// Free-form guidance shown to the orchestrator about when to dispatch
    /// this agent.
    pub when_to_use: String,
    /// Tool exposure policy resolved by
    /// [`crate::tool_resolver::AgentToolResolver`].
    pub tools: AgentToolPolicy,
    /// Hard upper bound on turns this agent may take before forced shutdown.
    pub max_turns: u32,
    /// Model preference. Resolved against the host's model catalog at spawn.
    pub model: AgentModel,
    /// Permission mode applied to tool calls — see [`AgentPermissionMode`].
    pub permission_mode: AgentPermissionMode,
    /// Origin of the definition (built-in, user file, plugin, ...).
    pub source: AgentSource,
    /// Directory the definition was loaded from. Used to resolve relative
    /// includes (e.g. system-prompt fragments).
    pub base_dir: PathBuf,
    /// Optional system-prompt body to prepend to the per-spawn template.
    pub system_prompt: Option<String>,
    /// MCP servers the agent should connect to.
    pub mcp_servers: Vec<AgentMcpServerSpec>,
    /// Hooks declared inside the agent's frontmatter.
    pub frontmatter_hooks: Vec<hooks::HookDefinition>,
    /// Optional emoji or short icon for the UI.
    pub icon: Option<String>,
    /// Allow-list of tool names appended to the resolver output. Often used
    /// to expose MCP-server tools without expanding `AgentToolPolicy::All`.
    pub allowed_tools: Vec<String>,
    /// Worktree policy (required, optional, none) — see
    /// [`crate::worktree_policy::create_worktree_or_degrade`].
    pub worktree_requirement: Option<WorktreeRequirement>,
    /// Deny-list of tool names (claude `disallowedTools`). Field-level parity:
    /// parsed and stored, but not yet consumed by
    /// [`crate::tool_resolver::AgentToolResolver`] (a separate follow-up).
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    /// Skill names to preload (claude `skills` — parsed from
    /// comma-separated/list frontmatter).
    #[serde(default)]
    pub skills: Vec<String>,
    /// MCP server name patterns that must be configured for the agent to be
    /// available (claude `requiredMcpServers`). NOTE: claude does NOT parse
    /// this from markdown/JSON frontmatter — it is set only on
    /// built-in/programmatic defs. The catalog/JSON parsers leave it empty.
    #[serde(default)]
    pub required_mcp_servers: Vec<String>,
    /// Always run as a background task when spawned (claude `background`).
    #[serde(default)]
    pub background: bool,
    /// Isolation mode: run in a git worktree, or remotely (claude
    /// `isolation`). `Remote` is ant-only and rejected by the parsers on
    /// non-ant builds.
    #[serde(default)]
    pub isolation: Option<AgentIsolation>,
    /// Persistent memory scope (claude `memory`). Parsed, stored, and EXECUTED:
    /// when set, [`crate::tool_resolver::AgentToolResolver::resolve`] injects the
    /// auto-memory tools (`Read`/`Write`/`Edit`) into the spawned agent's tool
    /// pool, mirroring claude's `isAutoMemoryEnabled` → Write/Edit/Read
    /// injection (the scope selects only WHERE memory lives, not which tools).
    #[serde(default)]
    pub memory: Option<AgentMemoryScope>,
    /// Reasoning effort preference (claude `effort` = level OR integer).
    #[serde(default)]
    pub effort: Option<AgentEffort>,
    /// Prepended to the first user turn (claude `initialPrompt`). The raw
    /// (untrimmed) value is kept when its trimmed form is non-empty.
    #[serde(default)]
    pub initial_prompt: Option<String>,
    /// Validated agent color name (claude `color`, one of [`AGENT_COLORS`]).
    /// Stored distinct from the `icon` emoji field.
    #[serde(default)]
    pub color: Option<String>,
}

/// Strategy the [`crate::tool_resolver::AgentToolResolver`] uses to project
/// the parent agent's tool set onto the child.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentToolPolicy {
    /// Inherit every tool from the parent. `use_exact_tools` controls whether
    /// the child receives the parent's exact tool instances or fresh ones.
    All {
        /// `true` when the child must reuse the parent's `Arc<dyn Tool>`
        /// references (e.g. to share state); `false` when it may be re-bound.
        use_exact_tools: bool,
    },
    /// Inherit only the explicitly named tools.
    Explicit(Vec<String>),
    /// Inherit every tool except the explicitly named ones.
    Except(Vec<String>),
}

/// Model resolution strategy for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentModel {
    /// Use whichever model the parent picked.
    Inherit,
    /// Resolve via a logical alias (e.g. `"sonnet"`, `"haiku-fast"`).
    Alias(String),
    /// Use the specific model id verbatim.
    Explicit(String),
}

/// Permission mode applied to tool calls inside the subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentPermissionMode {
    /// Bubble prompts up to the parent agent for approval.
    Bubble,
    /// Run isolated — never prompt; deny if not pre-approved.
    Isolated,
    /// Auto-approve every tool call (sandboxed contexts).
    Auto,
    /// Plan-only mode — read-only tools only.
    Plan,
}

/// Origin of an [`AgentDefinition`]. Used by precedence rules at load time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSource {
    /// Built into the binary.
    BuiltIn,
    /// Defined by the user (e.g. `~/.config/lingxi/agents/`).
    UserDefined,
    /// Defined inside the current project tree.
    Project,
    /// Provided by a plugin.
    Plugin,
    /// Sourced from policy settings (org-wide).
    PolicySettings,
    /// Sourced from a CLI flag / `--agents` JSON (claude `flagSettings`). This
    /// is the default `source` for the JSON-agent parsers
    /// ([`crate::catalog::parse_agent_from_json`]).
    Flag,
}

/// Isolation mode for an agent (claude `isolation`).
///
/// `Remote` is ant-only; the markdown/JSON parsers reject it on non-ant
/// builds (gated on `USER_TYPE == "ant"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentIsolation {
    /// Run in an isolated git worktree (claude `'worktree'`).
    Worktree,
    /// Run remotely in CCR (claude `'remote'`; ant-only).
    Remote,
}

/// Persistent memory scope for an agent (claude `memory`).
///
/// Order mirrors claude `VALID_MEMORY_SCOPES = ['user', 'project', 'local']`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentMemoryScope {
    /// User-global memory (claude `'user'`).
    User,
    /// Project-scoped memory (claude `'project'`).
    Project,
    /// Local (uncommitted) memory (claude `'local'`).
    Local,
}

/// Reasoning effort preference (claude `EffortValue = EffortLevel | number`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEffort {
    /// A named level — one of [`EFFORT_LEVELS`] (claude `EffortLevel`).
    Level(String),
    /// An integer effort value (claude numeric effort; ant-only at runtime).
    Numeric(i64),
}

impl AgentEffort {
    /// The wire value for `output_config.effort`: a level string or an integer
    /// budget (claude-code `SF`'s normalized output).
    #[must_use]
    pub fn to_wire(&self) -> serde_json::Value {
        match self {
            AgentEffort::Level(s) => serde_json::Value::String(s.clone()),
            AgentEffort::Numeric(n) => serde_json::Value::Number((*n).into()),
        }
    }

    /// Parse a JSON effort opt (claude-code workflow `agent({effort})`): a level
    /// string (validated against [`EFFORT_LEVELS`], `med`→`medium`) or an
    /// integer. `None` for anything else.
    #[must_use]
    pub fn from_json(value: &serde_json::Value) -> Option<AgentEffort> {
        match value {
            serde_json::Value::String(s) => parse_effort_from_string(s),
            serde_json::Value::Number(n) => n.as_i64().map(AgentEffort::Numeric),
            _ => None,
        }
    }
}

/// Valid named effort levels (claude `EFFORT_LEVELS` / `nP =
/// ["low","medium","high","xhigh","max"]`, v2.1.183).
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Coerce a YAML/JSON frontmatter value into an [`AgentEffort`], mirroring
/// claude `parseEffortValue` (effort.ts:71-87):
/// - `undefined`/`null`/empty-string -> `None`
/// - integer number -> `Numeric`
/// - string whose lowercased form is in [`EFFORT_LEVELS`] -> `Level`
/// - else `parseInt(str, 10)`; if an integer -> `Numeric`, else `None`
#[must_use]
pub fn parse_effort_value(value: &serde_yaml::Value) -> Option<AgentEffort> {
    match value {
        // value === undefined / null
        serde_yaml::Value::Null => None,
        // typeof value === 'number' && isValidNumericEffort(value)
        // (isValidNumericEffort = Number.isInteger)
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(AgentEffort::Numeric(i))
            } else {
                // Non-integer number (float): falls through to String(value)
                // path. parseInt of a float string truncates to its integer
                // part, matching JS `parseInt(String(1.5), 10) === 1`.
                let s = n.to_string();
                parse_effort_from_string(&s)
            }
        }
        serde_yaml::Value::String(s) => {
            // value === '' -> undefined
            if s.is_empty() {
                return None;
            }
            parse_effort_from_string(s)
        }
        serde_yaml::Value::Bool(b) => {
            // String(true)='true' / String(false)='false' -> not a level,
            // parseInt -> NaN -> None.
            let s = if *b { "true" } else { "false" };
            parse_effort_from_string(s)
        }
        // Arrays/maps: String(value) is non-numeric -> None.
        _ => None,
    }
}

/// String branch of [`parse_effort_value`]: lowercased level, else
/// `parseInt(str, 10)`.
fn parse_effort_from_string(value: &str) -> Option<AgentEffort> {
    let lower = value.to_lowercase();
    if EFFORT_LEVELS.contains(&lower.as_str()) {
        return Some(AgentEffort::Level(lower));
    }
    // JS parseInt(str, 10): consumes an optional sign and leading digits,
    // stopping at the first non-digit; NaN if no leading integer.
    parse_int_radix10(value).map(AgentEffort::Numeric)
}

/// JS-`parseInt(str, 10)` semantics: skip leading whitespace, take an optional
/// `+`/`-`, then consume leading ASCII digits; ignore any trailing garbage;
/// return `None` (NaN) when no digits are found.
fn parse_int_radix10(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let mut chars = t.chars().peekable();
    let mut out = String::new();
    if let Some(&c) = chars.peek() {
        if c == '+' || c == '-' {
            out.push(c);
            chars.next();
        }
    }
    let mut saw_digit = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            out.push(c);
            saw_digit = true;
            chars.next();
        } else {
            break;
        }
    }
    if !saw_digit {
        return None;
    }
    out.parse::<i64>().ok()
}

/// Reference to an MCP server an agent should connect to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMcpServerSpec {
    /// Refer to a server already registered by name in the host's
    /// [`mcp::McpRegistry`].
    ByName(String),
    /// Provide a full inline configuration, registered on demand.
    Inline {
        /// Logical name of the inline server.
        name: String,
        /// Connection configuration.
        config: mcp::McpServerConfig,
    },
}

/// Whether the agent needs an isolated git worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeRequirement {
    /// Spawn must fail if a worktree cannot be created.
    Required,
    /// Try to create one; fall back to in-place execution on failure.
    Optional,
    /// Don't bother with worktrees.
    None,
}

impl AgentDefinition {
    /// `true` if this is the special `fork` agent that runs a byte-exact
    /// copy of the parent's prompt cache. See spec §10.4.
    #[must_use]
    pub fn is_fork(&self) -> bool {
        self.agent_type == "fork"
    }
}
