//! Hook definition + executor / source taxonomy (spec §9.2, §9.3).

use crate::events::HookEventType;
use lingxi_protocol::HookId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// A registered hook: which events to fire on, optional matcher, how to
/// execute, and where the definition was loaded from.
///
/// Hooks are evaluated in priority-descending order; the executor stops
/// processing further hooks for the same event as soon as one returns a
/// [`crate::HookDecision::Block`] decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Stable engine-assigned ID for telemetry and cancellation.
    pub id: HookId,
    /// Human-readable name (used in logs and user messages).
    pub name: String,
    /// Event kinds this hook subscribes to.
    pub events: Vec<HookEventType>,
    /// Optional regex / matcher applied before executing the hook.
    /// When `None` the hook fires unconditionally for its subscribed events.
    pub if_condition: Option<HookCondition>,
    /// How to actually run the hook body.
    pub executor: HookExecutor,
    /// Where the hook definition came from (used for trust / display).
    pub source: HookSource,
    /// If `true` the engine awaits the result before continuing the in-flight
    /// action. If `false` the hook is dispatched to the background async
    /// registry and the engine proceeds immediately.
    pub blocking: bool,
    /// Optional wall-clock timeout. `None` means use the executor's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// Higher values execute first. Ties resolved by registration order.
    pub priority: i32,
}

/// How the engine actually invokes a hook body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookExecutor {
    /// Spawn an external process. Hook payload is delivered via stdin; the
    /// process replies with a JSON [`crate::HookResponse`] on stdout.
    Command {
        /// Executable path or PATH-resolvable command name.
        command: String,
        /// Positional CLI arguments.
        args: Vec<String>,
        /// Environment variables to set for the child process.
        env: HashMap<String, String>,
        /// Working directory for the child; `None` inherits the engine's cwd.
        cwd: Option<PathBuf>,
    },
    /// POST the event payload to an HTTP endpoint and parse the JSON body
    /// as the [`crate::HookResponse`]. Subject to the [`crate::SsrfGuard`].
    Http {
        /// Endpoint URL (scheme must satisfy the SSRF guard).
        url: String,
        /// HTTP method (typically `"POST"`).
        method: String,
        /// Request headers to attach.
        headers: HashMap<String, String>,
        /// Request timeout enforced by the executor.
        timeout: Duration,
    },
    /// Fork an agent of the named type, running the supplied prompt. The
    /// agent's tool result becomes the hook response.
    Agent {
        /// Agent type discriminator.
        agent_type: String,
        /// Prompt text supplied to the spawned agent.
        prompt: String,
    },
    /// Dispatch to an in-process Rust handler registered via
    /// [`crate::HookExecutorImpl::register_builtin`].
    Builtin {
        /// Handler ID, looked up in the executor's builtin table.
        handler_id: String,
    },
}

/// Optional matcher applied before executing a hook.
///
/// Currently a single pattern flagged for either tool-name or input matching.
/// The full regex / glob engine lands with the Plan 09 hook plumbing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCondition {
    /// Pattern text (regex or glob — interpretation TBD in Plan 09).
    pub pattern: String,
    /// If `true` the pattern is matched against `tool_name`.
    pub match_tool_name: bool,
    /// If `true` the pattern is matched against `tool_input` serialized JSON.
    pub match_input: bool,
}

/// Origin of a hook definition. Used by the engine to display trust info to
/// the user and by policy code to decide whether a hook can run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookSource {
    /// `~/.claude/hooks.json` (global, user-owned).
    User,
    /// `<project>/.claude/hooks.json` (committed project config).
    Project,
    /// `<project>/.claude/hooks.local.json` (developer-local override).
    Local,
    /// Managed policy hook (org-level, supplied via managed settings).
    Managed,
    /// Hook supplied by an installed plugin.
    Plugin,
    /// Hook embedded in an agent file's front-matter.
    FrontMatter,
    /// Hook registered at runtime by another part of the engine for the
    /// duration of a single session.
    Session,
    /// Hook registered as part of a skill bundle.
    Skill,
}
