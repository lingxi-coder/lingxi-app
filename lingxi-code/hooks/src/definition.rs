//! Hook definition + executor / source taxonomy (spec §9.2, §9.3).

use crate::events::HookEventType;
use protocol::HookId;
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
    /// If `true`, the hook runs once and is removed after a successful
    /// execution (claude-code `once` field; `schemas/hooks.ts:51-54`).
    ///
    /// This carries the parsed flag from settings; the actual self-removal
    /// RUNTIME behavior (dropping the hook from the registry after it
    /// succeeds) is executor work and is NOT wired here — see the loader
    /// module docs. Defaults to `false`.
    #[serde(default)]
    pub once: bool,
    /// Custom status message shown in the spinner while the hook runs
    /// (claude-code `statusMessage` field; `schemas/hooks.ts:47-50`).
    ///
    /// This carries the parsed text from settings; the TUI spinner wiring
    /// that actually displays it is presentation work and is NOT done here.
    /// Defaults to `None` (use the engine's generic running message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
}

impl HookDefinition {
    /// The hook's tool-name matcher pattern, if any (B3).
    ///
    /// In claude-code a `Pre/PostToolUse` hook declares a `matcher` string
    /// (e.g. `"Write|Edit"`, `"^Bash$"`, `"*"`) that gates whether the hook
    /// fires for a given tool. Here that pattern is canonically carried on
    /// [`Self::if_condition`] as the [`HookCondition::pattern`] of a condition
    /// whose [`HookCondition::match_tool_name`] is `true` — that is how the
    /// settings loader (`loader.rs`) and the orchestrator's `list_hooks`
    /// (`handle_impl.rs`) already model it. This accessor surfaces that pattern
    /// as the `Option<String>`-shaped "matcher" the B3 spec calls for, WITHOUT
    /// adding a new struct field (which would break the explicit
    /// `HookDefinition { … }` literals constructed in the locked test-harness
    /// and orchestrator integration tests).
    ///
    /// Returns `None` when the hook has no condition, or has an
    /// `if`-condition that is NOT a tool-name matcher (`match_tool_name ==
    /// false`) — such a condition is a permission-rule / input matcher,
    /// surfaced separately by [`Self::if_pattern`].
    #[must_use]
    pub fn matcher(&self) -> Option<&str> {
        self.if_condition
            .as_ref()
            .filter(|c| c.match_tool_name)
            .map(|c| c.pattern.as_str())
    }

    /// The hook's `if`-condition permission-rule string, if any (e.g.
    /// `"Bash(git push:*)"`).
    ///
    /// This is the per-hook `if` field from claude-code's hook schema
    /// (`schemas/hooks.ts:35` `IfConditionSchema`): a permission-rule-syntax
    /// pattern evaluated against the event's tool name + tool input by
    /// [`crate::matcher::matches_if_condition`] in
    /// [`crate::registry::HookRegistry::match_event`]. It is INDEPENDENT of the
    /// tool-name [`Self::matcher`] (the parent group `matcher`); a hook may
    /// declare both, and claude-code requires BOTH to pass before the hook
    /// fires (`utils/hooks.ts:1681-1685` then `:1808-1850`). Returns `None` when
    /// the hook carries no `if`-condition.
    #[must_use]
    pub fn if_pattern(&self) -> Option<&str> {
        self.if_condition
            .as_ref()
            .and_then(|c| c.if_pattern.as_deref())
    }
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
        /// Request headers to attach. Values may reference environment variables
        /// via `$VAR`/`${VAR}`; only names in [`Self::Http::allowed_env_vars`]
        /// are interpolated (claude-code `cHm`).
        headers: HashMap<String, String>,
        /// Env-var names that header values may interpolate (`allowedEnvVars`,
        /// `schemas/hooks.ts`). A `$VAR`/`${VAR}` reference whose name is NOT in
        /// this list resolves to an empty string (claude-code `cHm` warns +
        /// substitutes `""`); a name present resolves to the process env value
        /// (`process.env[VAR] ?? ""`). Empty ⇒ no interpolation (every `$VAR`
        /// reference is blanked).
        allowed_env_vars: Vec<String>,
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
        /// Optional model override (`schemas/hooks.ts:128-163` `model`). When
        /// `None` the spawned verifier inherits the spawner's default model.
        model: Option<String>,
    },
    /// Evaluate an inline single-turn LLM query and map the model's
    /// `{ok, reason?}` JSON to a hook decision (claude-code
    /// `utils/hooks/execPromptHook.ts`; `schemas/hooks.ts:67-95`).
    ///
    /// The query is run through the injected
    /// [`crate::prompt_executor::HookPromptRunner`] (wired on the
    /// [`crate::HookExecutorImpl`] via `with_prompt_runner`). When no runner is
    /// wired the arm is a structured "not wired" no-op — it never blocks.
    Prompt {
        /// Prompt text to evaluate. May contain `$ARGUMENTS` placeholders that
        /// the executor substitutes with the serialized event payload
        /// (`addArgumentsToPrompt`; `execPromptHook.ts:35`).
        prompt: String,
        /// Optional model override (`schemas/hooks.ts:81-86`). When `None` the
        /// runner falls back to its default small-fast model
        /// (`getSmallFastModel()`; `execPromptHook.ts:79`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// `continueOnBlock` (`schemas/hooks.ts`): when the hook blocks
        /// (`ok:false`), whether the turn may still continue. Drives the prompt
        /// executor's `prevent_continuation = !continue_on_block`. Default false
        /// (a block ends the turn).
        #[serde(default)]
        continue_on_block: bool,
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
/// A single pattern flagged for either tool-name or input matching.
///
/// When `match_tool_name` is `true`, [`Self::pattern`] is the B3 tool-name
/// matcher (`"Write|Edit"`, `"^Bash$"`, `"*"`, …) and is evaluated by
/// [`crate::matcher::matches_pattern`] against the event's tool name in
/// [`crate::registry::HookRegistry::match_event`]. This is the MATCHER half of
/// B3 and is fully ported.
///
/// When `match_input` is `true`, [`Self::if_pattern`] carries a permission-rule
/// string (e.g. `"Bash(git push:*)"`) matched against the tool name + tool
/// input — the `if`-condition half of B3. That rule-content matching is now
/// PORTED ([`crate::matcher::matches_if_condition`], reusing the permission
/// crate's `PermissionRuleValue::from_rule_string` + per-tool content matchers)
/// and gates firing in [`crate::registry::HookRegistry::match_event`].
///
/// The tool-name [`Self::pattern`] (group `matcher`) and the [`Self::if_pattern`]
/// (`if`-condition) are INDEPENDENT — a hook may carry both, and both must pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCondition {
    /// Pattern text of the B3 tool-name matcher, evaluated when
    /// `match_tool_name` is `true`. (The `if`-condition lives in
    /// [`Self::if_pattern`], not here.)
    pub pattern: String,
    /// If `true` [`Self::pattern`] is the B3 tool-name matcher (evaluated).
    pub match_tool_name: bool,
    /// If `true` an [`Self::if_pattern`] permission-rule string is present and
    /// matched against the tool name + serialized `tool_input` (the
    /// `if`-condition half of B3 — now ported). Kept in sync with
    /// `if_pattern.is_some()`.
    pub match_input: bool,
    /// The `if`-condition permission-rule string (claude-code per-hook `if`
    /// field, `schemas/hooks.ts:35`). `None` when the hook has no `if`. Additive
    /// (`#[serde(default)]`) so existing serialized hooks deserialize unchanged,
    /// and `skip_serializing_if` keeps a `None` byte-identical to the pre-`if`
    /// serialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_pattern: Option<String>,
}

/// Origin of a hook definition. Used by the engine to display trust info to
/// the user and by policy code to decide whether a hook can run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookSource {
    /// `~/.lingxi/hooks.json` (global, user-owned).
    User,
    /// `<project>/.lingxi/hooks.json` (committed project config).
    Project,
    /// `<project>/.lingxi/hooks.local.json` (developer-local override).
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
