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
//!   - `<name> is a built-in CLI command, not a skill. Ask the user to run /<name> themselves — it cannot be invoked via the Skill tool.`
//! - Output (inline path) mirrors the TS inline output union (TS `:301-326`):
//!   `{success:true, commandName, allowedTools?, model?, status:"inline"}`.
//!
//! ## `context: fork`
//! A skill declaring `context: fork` does NOT expand inline. It launches a
//! background subagent under the skill's own permission scoping (persisted to
//! the scoping sidecars so a later resume can re-establish it — see
//! `session::forked_skill`) and returns a handle instead of the body. The
//! decision logic — the background predicate, the caps, the one-live-fork-per-
//! skill guard, and the result shapes — lives in [`crate::fork`].
//!
//! Every guard degrades to the INLINE path rather than failing the call, which
//! is claude's own behaviour: a skill that cannot fork still runs, just here.
//! The one exception is the depth-chain cap, which is a hard error.
//!
//! Both fork shapes are implemented: the BACKGROUND fork detaches and returns
//! a handle, the SYNCHRONOUS fork runs the subagent to completion and returns
//! its answer. Both apply the skill's scoping and inject nothing into the
//! caller's conversation; only the background one is resumable, so only it
//! writes the scoping sidecars.
//!
//! MCP prompts reach this tool as ordinary `<server>:<prompt>` commands —
//! the `getAllCommands` merge lives in `command_api::mcp_prompts` and runs at
//! the composition root. They resolve as [`SkillCommandType::Other`], so the
//! locked "is a built-in CLI command, not a skill" rejection applies: an MCP
//! prompt's body comes from a remote server and is never shell-expanded here.
//!
//! (`EXPERIMENTAL_SKILL_SEARCH` is ported in `skill_api::prefetch`, gated off
//! by default — an earlier revision of this comment claimed otherwise.)
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
use telemetry::tengu::tool::{SKILL_COMPLETED, SKILL_FAILED, SKILL_INVOKED, SKILL_STARTED};
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
    /// "is a built-in CLI command, not a skill…" error.
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
    /// Tools this skill REMOVES (frontmatter `disallowed-tools`). For a
    /// `context: fork` skill these ride onto the spawned agent as
    /// `SubagentSpawnRequest::additional_disallowed_tools` — without them the
    /// fork would run with the parent's tools, which is the scoping the fork
    /// exists to narrow.
    pub disallowed_tools: Vec<String>,
    /// Declared positional argument names (markdown frontmatter `arguments`).
    /// Threaded into [`command_api::substitute_arguments_faithful`] so a named
    /// placeholder like `$ticket` in the skill body resolves to the matching
    /// positional argument. Empty → only `$ARGUMENTS` / `$N` placeholders expand
    /// (TS `processPromptSlashCommand` passes the command's `argNames`).
    pub argument_names: Vec<String>,
    /// Shell to route embedded `!command` expansion through (the markdown
    /// frontmatter `shell` selector). `None` -> bash (the TS default). Forwarded
    /// to [`command_api::execute_shell_commands_in_prompt`] as the `shell` arg —
    /// 1:1 with TS `getPromptForCommand(..., shell)` (`loadSkillsDir.ts:394`).
    pub shell: Option<command_api::FrontmatterShell>,
    /// Skip embedded `!command` shell expansion for this skill. Mirrors the TS
    /// `loadedFrom !== 'mcp'` gate (`loadSkillsDir.ts:374`): MCP skills are
    /// remote/untrusted, so their markdown body is NEVER shell-expanded. `false`
    /// (the default) -> expansion runs (the on-disk / plugin markdown case).
    pub skip_shell_expansion: bool,
    /// The skill's own base directory (TS `baseDir`). `Some(dir)` for file-based
    /// skills (the SKILL.md / command markdown's parent directory); `None` for
    /// non-file skills (e.g. MCP-sourced, which have no `baseDir`). When `Some`,
    /// `${LINGXI_SKILL_DIR}` in the body is replaced with this path so embedded
    /// `!command` blocks can reference bundled scripts
    /// (`loadSkillsDir.ts:359-363`).
    pub skill_root: Option<std::path::PathBuf>,
    /// Execution context from the frontmatter: `Some("fork")` runs the skill as
    /// a subagent under its own permission scoping instead of expanding it
    /// inline. Anything else (including `None`) is inline.
    pub context: Option<String>,
    /// Whether a forking skill runs in the BACKGROUND. `None` ⇒ background (the
    /// claude default); see `crate::fork::should_background_fork`.
    pub background: Option<bool>,
    /// The agent type a forking skill spawns (frontmatter `agent`). `None` ⇒
    /// the `general-purpose` fallback.
    pub agent: Option<String>,
    /// The current session id, surfaced to substitute `${LINGXI_SESSION_ID}` in
    /// the body (TS `getSessionId()`, `loadSkillsDir.ts:366-369`). `None` for
    /// hermetic/loaderless construction (the token is then left as-is); the
    /// production loader stamps the engine's per-session id here. Carried on the
    /// descriptor rather than the frozen `BuiltinToolContext` (whose mobile
    /// construction site cannot be extended).
    pub session_id: Option<String>,
    /// Dynamic prompt builder for bundled skills (port of the reference
    /// `getPromptForCommand`, `loop.ts:84`). When `Some`, [`SkillTool::call`]
    /// calls it with the raw args to produce the prompt INSTEAD of
    /// `substitute_arguments_faithful` over `body` — letting a bundled skill
    /// branch on empty vs non-empty args. `None` for all file/MCP skills (the
    /// byte-identical static path).
    pub dynamic_body: Option<std::sync::Arc<dyn command_api::BundledPromptFn>>,
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
            disallowed_tools: Vec::new(),
            argument_names: Vec::new(),
            shell: None,
            skip_shell_expansion: false,
            skill_root: None,
            context: None,
            background: None,
            agent: None,
            session_id: None,
            dynamic_body: None,
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

// SKILLEXEC.6: the embedded-`!command` host runner + permission gate for skill
// bodies live in this crate's `crate::prompt_shell` module (the former
// `SkillShellRunner` → `crate::PromptShellRunner`) so the dispatcher, the
// TUI, and this `Skill` tool share ONE real runner + one policy-backed gate.
// (It bridges `tool_api::BuiltinToolContext` with `command_api`'s shell traits,
// so it cannot live in the impl-free `tool-api` API crate — §8.1.)
// The gate is no longer allow-all: it is now the faithful port of
// `hasPermissionsToUseTool(BashTool, {command})` (see `PolicyShellPermissionGate`),
// with the skill's own frontmatter `allowedTools` injected per expansion. This
// tool builds it via `crate::build_prompt_shell_provider(&self.ctx)` at the
// expansion site below.

/// `SkillTool` — resolves + validates a slash-command skill.
pub struct SkillTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
    pub(crate) loader: Arc<dyn SkillLoader>,
    /// SESSION-scoped prompt shell-expansion provider. `SkillTool` is registered
    /// ONCE per session, so building the provider (and its lazily-created shell
    /// snapshot) here — rather than fresh in every `call()` — makes the snapshot
    /// shell run at most ONCE per session (the snapshot's `OnceCell`), matching
    /// the oracle. Re-creating it per invocation re-executed the user's
    /// `~/.zshrc`/`~/.bashrc` (unsandboxed, 10s budget) on every skill call.
    pub(crate) prompt_shell: Arc<dyn command_api::ShellExpansionProvider>,
}

impl SkillTool {
    /// Construct with the default `EmptySkillLoader`.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        let prompt_shell = crate::build_prompt_shell_provider(&ctx);
        Self {
            ctx,
            loader: Arc::new(EmptySkillLoader),
            prompt_shell,
        }
    }

    /// Construct with a caller-supplied loader.
    #[must_use]
    pub fn with_loader(ctx: tool_api::BuiltinToolContext, loader: Arc<dyn SkillLoader>) -> Self {
        let prompt_shell = crate::build_prompt_shell_provider(&ctx);
        Self {
            ctx,
            loader,
            prompt_shell,
        }
    }

    /// Attempt the `context: fork` path.
    ///
    /// `Ok(None)` means "run inline instead" — every guard except the
    /// depth-chain cap degrades that way, and so does a host with no spawner or
    /// task registry wired. `Ok(Some(result))` is a launched fork.
    ///
    /// Both fork shapes are handled. The BACKGROUND fork detaches through
    /// `spawn_async` and returns a handle; the SYNCHRONOUS fork (a skill that
    /// opts out, a session with background tasks off, or a caller that is
    /// already a subagent) runs the subagent to completion through `spawn` and
    /// returns its answer. Only the background one is addressable and
    /// resumable, so only it claims a `SendMessage` name, a fork identity on
    /// the task index, and the scoping sidecars.
    async fn try_fork(
        &self,
        bus: &Arc<AnalyticsBus>,
        ctx: &ToolUseContext,
        desc: &SkillDescriptor,
        command_name: &str,
        prompt: &str,
    ) -> Result<Option<ToolCallResult>, ToolError> {
        let (Some(spawner), Some(registry)) = (
            self.ctx.subagent_spawner.as_ref(),
            self.ctx.task_registry.as_ref(),
        ) else {
            return Ok(None);
        };
        let background_tasks_disabled = traits::env::is_env_truthy(
            std::env::var("LINGXI_DISABLE_BACKGROUND_TASKS")
                .ok()
                .as_deref(),
        );
        let background = crate::fork::should_background_fork(
            desc.background,
            false,
            background_tasks_disabled,
            ctx.depth > 0,
        );

        let tasks = registry
            .list(traits::task_registry::TaskListFilter::default())
            .await
            .unwrap_or_default();
        let frozen_command_denies = frozen_command_denies(&self.ctx.permission_policy);
        let spawn_cap = max_subagents_per_session();
        let inputs = crate::fork::ForkLaunchInputs {
            skill_name: command_name,
            attribution_name: command_name,
            // `SkillDescriptor` carries no effort today; the sidecar's
            // `effort` key stays absent, which is claude's shape for a skill
            // that declares none.
            effort: None,
            // Snapshot the command deny rules in force RIGHT NOW (claude's
            // `freezeCommandDenies`, which it sets only on the background
            // path). Persisted with the scoping so a resume can replay them
            // ahead of whatever is live then.
            frozen_command_denies: frozen_command_denies.clone(),
            depth: ctx.depth as usize + 1,
            depth_limit: traits::subagent_spawn::max_subagent_spawn_depth() as usize,
            total_spawns: registry.get_total_agent_spawns(),
            spawn_cap,
            tasks: &tasks,
        };
        let scoping = match crate::fork::decide_fork_launch(&inputs) {
            crate::fork::ForkDecision::Launch(s) => s,
            crate::fork::ForkDecision::Inline(reason) => {
                emit_failed(bus, reason.reason(), 0).await;
                return Ok(None);
            }
            crate::fork::ForkDecision::DepthChainCap { spawned, cap } => {
                emit_failed(bus, "forked_skill_depth_chain_cap", 0).await;
                return Err(ToolError::InvalidInput(
                    crate::fork::depth_chain_cap_message(spawned, cap),
                ));
            }
        };

        let request = traits::subagent_spawn::SubagentSpawnRequest {
            subagent_type: desc
                .agent
                .clone()
                .unwrap_or_else(|| "general-purpose".to_string()),
            prompt: prompt.to_string(),
            description: Some(format!("/{command_name}")),
            model: desc.model.clone(),
            run_in_background: true,
            // The `SendMessage` handle AND the fork identity: the launch is
            // announced as `@<skill>`, and the identity is what the resume gate
            // corroborates against the scoping record on disk.
            name: Some(command_name.to_string()),
            forked_skill_name: Some(scoping.skill_name.clone()),
            forked_skill_attribution: Some(scoping.attribution_name.clone()),
            forked_skill_effort: None,
            frozen_command_denies: scoping.frozen_command_denies.clone().unwrap_or_default(),
            depth: ctx.depth + 1,
            context_paths: Vec::new(),
            model_profile: None,
            team_name: None,
            creator_teammate_name: None,
            creator_team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
            worktree: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: ctx.tool_use_id.as_ref().map(ToString::to_string),
            system_prompt_override: None,
            system_prompt_addendum: None,
            // The skill's own `disallowed-tools` — half of the scoping a fork
            // exists to narrow. Without this the forked agent would run with
            // the PARENT's tools, which is strictly wider than the skill's.
            additional_disallowed_tools: desc.disallowed_tools.clone(),
            parent_model_override: None,
            resumed_history: None,
        };
        let inherit = traits::subagent_spawn::SubagentInheritance {
            tool_invoker: Arc::new(tool_api::tool_invoker_impl::RegistryToolInvoker::new(
                ctx.subagent_registry.clone().ok_or_else(|| {
                    ToolError::Internal("Skill: subagent tool registry is not configured".into())
                })?,
            )),
            budget: self.ctx.budget_enforcer.clone().ok_or_else(|| {
                ToolError::Internal("Skill: budget enforcer is not configured".into())
            })?,
        };

        // Re-check across the await window: a concurrent tool call can launch
        // the same skill or exhaust the spawn budget while this one is in
        // flight (claude re-runs both tests after its scoping write).
        //
        // The budget half RESERVES atomically rather than re-reading the
        // counter. Read-then-increment lets two concurrent forks both observe
        // `spawns < cap` and both spawn, overshooting the session cap; the
        // reservation is compare-and-increment in one step. Released on every
        // path that then declines to spawn, so a duplicate-loss does not burn
        // budget.
        let reserved = registry.try_reserve_total_agent_spawn(spawn_cap).is_ok();
        if !reserved {
            emit_failed(bus, crate::fork::InlineFallback::SpawnCap.reason(), 0).await;
            return Ok(None);
        }
        if crate::fork::has_live_fork(
            &registry
                .list(traits::task_registry::TaskListFilter::default())
                .await
                .unwrap_or_default(),
            command_name,
        ) {
            registry.release_total_agent_spawn_reservation();
            emit_failed(
                bus,
                crate::fork::InlineFallback::LiveDuplicate.reason(),
                0,
            )
            .await;
            return Ok(None);
        }

        // SYNCHRONOUS fork (claude's `background: false` branch, and the branch
        // its predicate routes to when background tasks are off or the caller
        // is already a subagent): run the subagent to completion HERE and
        // return its answer. Still a fork — the skill's scoping applies and
        // nothing is injected into this conversation — just not detached.
        //
        // It is NOT resumable, so it writes no scoping sidecars: there is
        // nothing to resume, and a record with no agent behind it would only
        // give a later resume something spurious to corroborate.
        if !background {
            let mut sync_request = request;
            sync_request.run_in_background = false;
            // A synchronous fork is not addressable, so it claims no
            // `SendMessage` name and no fork identity on the task index — both
            // exist to route to, and later resume, a LIVE background agent.
            sync_request.name = None;
            sync_request.forked_skill_name = None;
            let (agent_id, result) = match spawner.spawn(sync_request, inherit).await {
                Ok(traits::subagent_spawn::SubagentResult::Completed {
                    agent_id, content, ..
                }) => {
                    let text = crate::fork::final_text(&content);
                    (
                        agent_id,
                        if text.is_empty() {
                            crate::fork::SYNC_FORK_EMPTY_RESULT.to_string()
                        } else {
                            text
                        },
                    )
                }
                // A failed or killed fork surfaces as a tool error rather than
                // silently reporting success with an empty result.
                Ok(traits::subagent_spawn::SubagentResult::Failed { reason, .. }) => {
                    // The spawn HAPPENED, so the reservation is correctly
                    // consumed — only a fork that never launched releases it.
                    return Err(ToolError::Internal(format!(
                        "Skill {command_name} (forked execution) failed: {reason}"
                    )));
                }
                Ok(traits::subagent_spawn::SubagentResult::Killed { .. }) => {
                    return Err(ToolError::Internal(format!(
                        "Skill {command_name} (forked execution) was stopped"
                    )));
                }
                // An unwired spawner falls back to inline — the skill still
                // runs, just in this context.
                Err(_) => {
                    registry.release_total_agent_spawn_reservation();
                    return Ok(None);
                }
            };
            return Ok(Some(ToolCallResult {
                data: crate::fork::fork_result(
                    command_name,
                    &agent_id.to_string(),
                    false,
                    &result,
                ),
                model_content: Some(crate::fork::fork_tool_result_text(
                    command_name,
                    false,
                    &result,
                )),
                new_messages: Vec::new(),
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            }));
        }

        let launch = match spawner.spawn_async(request, inherit).await {
            Ok(l) => l,
            // A spawner that cannot background (unwired seam, or the scoping
            // write failed) falls back to inline rather than failing the call —
            // the skill still runs, just here.
            Err(_) => {
                registry.release_total_agent_spawn_reservation();
                emit_failed(
                    bus,
                    crate::fork::InlineFallback::ScopingWriteFailed.reason(),
                    0,
                )
                .await;
                return Ok(None);
            }
        };

        let result_line = crate::fork::running_in_background_line(command_name);
        Ok(Some(ToolCallResult {
            data: crate::fork::fork_result(
                command_name,
                &launch.agent_id.to_string(),
                true,
                &result_line,
            ),
            model_content: Some(crate::fork::fork_tool_result_text(
                command_name,
                true,
                &result_line,
            )),
            // A forked skill injects NOTHING into this conversation — that is
            // the whole point of forking. The inline path's `new_messages` is
            // what this replaces.
            new_messages: Vec::new(),
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        }))
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "skill": {
                "type": "string",
                "description": "The name of a skill from the available-skills list. Do not guess names."
            },
            "args": {
                "type": "string",
                "description": "Optional arguments for the skill"
            }
        },
        "required": ["skill"]
    })
});

/// Snapshot the COMMAND deny rules currently in force, as canonical rule
/// strings (claude `freezeCommandDenies` — `getAppState().toolPermissionContext
/// .alwaysDenyRules.command ?? []`, captured only on the background fork path).
///
/// Claude freezes these because its resume rebuilds the permission context from
/// LIVE app state, so without a snapshot a settings edit made while the fork was
/// parked could REMOVE a deny that was in force when it launched. The port's
/// `PolicyPermissionGate` holds a boot-snapshot `Arc<PermissionPolicy>` (only
/// the MODE is live), so within one process the two cannot drift — but the
/// record outlives the process, and a cross-session resume reads it against a
/// freshly-loaded policy. Persisting it is what makes that resume checkable.
fn frozen_command_denies(policy: &permission::PermissionPolicy) -> Vec<String> {
    let mut out: Vec<String> = policy
        .deny_rules
        .values()
        .flatten()
        // `Bash` is the command tool — claude's `alwaysDenyRules.command`
        // bucket. Deny rules for other tools are not command rules.
        .filter(|r| r.value.tool_name == "Bash")
        .map(|r| r.value.to_rule_string())
        .collect();
    // `deny_rules` is keyed by source, and a HashMap has no stable iteration
    // order — sort so the persisted record is deterministic across runs.
    out.sort();
    out.dedup();
    out
}

/// The per-session subagent spawn cap (claude 2.1.212 `xtu()` =
/// `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION ?? 200`). Duplicated from the Agent
/// tool rather than shared: both read the same env var, and `tool-skill` does
/// not depend on `tool-agent`. An unset or unparseable value falls back to the
/// default, so a garbage env string cannot silently disable the cap.
fn max_subagents_per_session() -> u64 {
    std::env::var("CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(200)
}

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

/// Sanitize a normalized skill name for the `SKILL_INVOKED` telemetry
/// `skill_name` dimension (a `Verified`/whitelisted, non-PII field). Builtin
/// command names pass through; any non-builtin collapses to `"custom"` — TS
/// `command_name = NOT_CODE_OR_FILEPATHS` over `builtInCommandNames`
/// (`commands.ts:350-353`). The descriptor carries no bundled/official-source
/// flag, so bundled/official skills are indistinguishable here and all
/// non-builtins map to `"custom"`.
fn skill_name_dimension(command_name: &str) -> String {
    if command_api::builtin_support::names::BUILTIN_COMMAND_NAMES.contains(&command_name) {
        command_name.to_string()
    } else {
        "custom".to_string()
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        SKILL_TOOL_NAME
    }
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("invoke a slash-command skill")
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
        // Binary-grounded text (bytes 198002225+): the exact Skill tool
        // description sent to the model. MUST NOT be changed without a binary
        // re-audit.
        "Execute a skill within the main conversation\n\
\n\
When users ask you to perform tasks, check if any of the available skills match. \
Skills provide specialized capabilities and domain knowledge.\n\
\n\
When users reference a \"slash command\" or \"/<something>\", they are referring to a skill. \
Use this tool to invoke it.\n\
\n\
How to invoke:\n\
- Set `skill` to the exact name of an available skill (no leading slash). \
For plugin-namespaced skills use the fully qualified `plugin:skill` form.\n\
- Set `args` to pass optional arguments.\n\
- Some skills are scoped to a directory: their name is prefixed with the directory \
(e.g. `apps/web:deploy`) and their description says which directory they apply to. \
When a skill name has both a scoped and an unscoped variant, pick by the files you \
are working on: if the files are under a variant's directory, invoke that variant \
(most specific directory wins); otherwise invoke the unscoped one.\n\
\n\
Important:\n\
- Available skills are listed in system-reminder messages in the conversation\n\
- Only invoke a skill that appears in that list, or one the user explicitly typed as \
`/<name>` in their message. Never guess or invent a skill name from training data; \
otherwise do not call this tool\n\
- When a skill matches the user's request, this is a BLOCKING REQUIREMENT: invoke \
the relevant Skill tool BEFORE generating any other response about the task\n\
- NEVER mention a skill without actually calling this tool\n\
- Do not invoke a skill that is already running\n\
- Do not use this tool for built-in CLI commands (like /help, /clear, etc.)\n\
- If you see a <command-name> tag in the current conversation turn, the skill has \
ALREADY been loaded - follow the instructions directly instead of calling this tool again"
            .into()
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
                    // Binary `skill_invoke_not_prompt_type`: `${name} is a ${u}
                    // command, not a skill. Ask the user to run /${name}
                    // themselves — it cannot be invoked via the Skill tool.`
                    // where `u = type==="local-jsx" ? "UI" : "built-in CLI"`. The
                    // port collapses non-prompt to `Other` (it does not surface
                    // local-jsx commands as skills), so `u` = "built-in CLI".
                    return Err(ValidationError(format!(
                        "{normalized} is a built-in CLI command, not a skill. Ask the user to run /{normalized} themselves — it cannot be invoked via the {SKILL_TOOL_NAME} tool."
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
        ctx: ToolUseContext,
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
            // Binary `skill_invoke_not_prompt_type` reject (see validate_input).
            return Err(ToolError::InvalidInput(format!(
                "{command_name} is a built-in CLI command, not a skill. Ask the user to run /{command_name} themselves — it cannot be invoked via the {SKILL_TOOL_NAME} tool."
            )));
        }

        // SKILLEXEC.5: fire the registered `SKILL_INVOKED` event on the success
        // path — after validateInput passes (descriptor loaded, prompt-type, not
        // disable-model-invocation) — mirroring TS `SkillTool.ts:654-709`. The
        // `skill_name` dimension is a `Verified` (whitelisted, non-PII) field, so
        // it is sanitized to the builtin command name, or `"custom"` for any
        // non-builtin skill (TS `command_name = NOT_CODE_OR_FILEPATHS` over
        // `builtInCommandNames`, `commands.ts:350-353`). The `SkillDescriptor`
        // carries no bundled/official-source flag, so every non-builtin collapses
        // to `"custom"` — faithful for the hermetic substrate.
        let skill_name_dim = skill_name_dimension(&command_name);
        let mut inv_md: LogEventMetadata = HashMap::new();
        inv_md.insert(
            "invocation_id".into(),
            verified_str(&tool_api::util::ids::ulid_or_uuid()),
        );
        inv_md.insert("skill_name".into(), verified_str(&skill_name_dim));
        bus.log_event(SKILL_INVOKED, inv_md).await;

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

        // SKILLEXEC.3: expand the skill body into the prompt the model must
        // process. Faithful to TS `processPromptSlashCommand`
        // (SkillTool.ts:634-643 → `getPromptForCommand`): `$ARGUMENTS` / `$N` /
        // `$name` substitution over the markdown body with the raw args string
        // (`args || ''`), `appendIfNoPlaceholder = true`, and the command's
        // declared argument names. The result is returned as `new_messages` (a
        // user message) so the turn loop appends it after this tool_result and
        // the model acts on the skill (SkillTool.ts:735-774 `newMessages`).
        //
        // SKILLEXEC.6: the embedded `!command` shell-expansion step (TS
        // `getPromptForCommand` step 4) RUNS here, after argument substitution
        // and before the user message is built — see below. The TS tagging of the
        // message with the parent toolUseID (transient-until-resolved) has no Rust
        // `ConversationMessage` substrate and is scoped out: the expanded prompt
        // enters as a plain user message. The adjacent TS `${LINGXI_SKILL_DIR}` /
        // `${LINGXI_SESSION_ID}` token replacements (steps 2-3) run between
        // argument substitution and shell expansion — see just below.
        let args_for_expansion = args.as_deref().unwrap_or("");
        let expanded_prompt = if let Some(builder) = desc.dynamic_body.as_ref() {
            // SKILLEXEC.3 (bundled): the dynamic builder replaces static
            // templating — mirrors the reference `getPromptForCommand(args)`
            // (`loop.ts:84`). The builder does its own arg handling
            // (empty→usage, else→buildPrompt(trimmed)), so `$ARGUMENTS`/`$N`
            // substitution is bypassed entirely.
            builder.build(args_for_expansion)
        } else {
            match command_api::substitute_arguments_faithful(
                &desc.body,
                Some(args_for_expansion),
                true,
                &desc.argument_names,
            ) {
                Ok(p) => p,
                Err(e) => {
                    emit_failed(
                        &bus,
                        "expansion_error",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::Internal(format!(
                        "Skill {command_name} expansion failed: {e}"
                    )));
                }
            }
        };

        // SKILLEXEC: `${LINGXI_SKILL_DIR}` / `${LINGXI_SESSION_ID}` token
        // replacement (TS `getPromptForCommand` steps 2-3,
        // `loadSkillsDir.ts:359-369`). Faithful ordering: AFTER argument
        // substitution, BEFORE the embedded `!command` shell expansion below, so
        // a `!command` that references `${LINGXI_SKILL_DIR}` / `${LINGXI_SESSION_ID}`
        // sees the substituted value. Both are plain literal-token replacements
        // (the tokens carry no regex metacharacters), so `str::replace` (global by
        // default) matches the TS global-regex `.replace(/.../g, …)` exactly.
        //
        // SAFETY (byte-identical): a body containing NEITHER token is returned
        // unchanged by both `replace` calls (no match → no allocation difference
        // in output), preserving the byte-for-byte invariant for the common case.
        let mut expanded_prompt = expanded_prompt;
        // Step 2: `${LINGXI_SKILL_DIR}` — only for file-based skills (TS gates on
        // `baseDir`). On Windows, normalize backslashes to forward slashes BEFORE
        // the replace so embedded shell commands don't treat them as escapes
        // (`loadSkillsDir.ts:360-361`).
        if let Some(skill_root) = desc.skill_root.as_ref() {
            let skill_dir = skill_root.to_string_lossy();
            let skill_dir = if cfg!(windows) {
                skill_dir.replace('\\', "/")
            } else {
                skill_dir.into_owned()
            };
            expanded_prompt = expanded_prompt.replace("${LINGXI_SKILL_DIR}", &skill_dir);
        }
        // Step 3: `${LINGXI_SESSION_ID}` — always replaced (TS calls
        // `getSessionId()` unconditionally). `None` (hermetic/loaderless) leaves
        // the token untouched rather than substituting an empty string.
        if let Some(session_id) = desc.session_id.as_ref() {
            expanded_prompt = expanded_prompt.replace("${LINGXI_SESSION_ID}", session_id);
        }

        // SKILLEXEC.6: embedded `!command` shell expansion over the substituted
        // body, faithful to TS `getPromptForCommand` step 4
        // (`loadSkillsDir.ts:374-395` -> `executeShellCommandsInPrompt`). Gated on
        // `!skip_shell_expansion` (the TS `loadedFrom !== 'mcp'` guard): MCP skills
        // are remote/untrusted and never shell-expand their body. The slash-command
        // name arg is `/${command_name}`; the shell arg is the descriptor's
        // frontmatter `shell` selector.
        //
        // SAFETY (byte-identical): when the substituted body contains NO `!command`
        // block, `execute_shell_commands_in_prompt` returns it UNCHANGED (block
        // scan finds nothing; the inline scan is gated behind a `"!`"` substring
        // fast-path), so the common case is byte-identical to today. MCP-sourced
        // skills (`skip_shell_expansion = true`) skip the call entirely.
        let mut expanded_prompt = if desc.skip_shell_expansion {
            expanded_prompt
        } else {
            // Build the shared per-command expansion context: the real host
            // runner (`crate::PromptShellRunner`) + the policy-backed gate
            // (`PolicyShellPermissionGate`), with THIS skill's frontmatter
            // `allowed_tools` injected on top of the base policy — 1:1 with
            // claude-code building a fresh `toolPermissionContext` before
            // `executeShellCommandsInPrompt`. The `shell` selector drives both the
            // gate's tool-name choice and the runner's routing.
            let shell_ctx = self
                .prompt_shell
                .build(&desc.allowed_tools, desc.shell);
            match command_api::execute_shell_commands_in_prompt(
                &expanded_prompt,
                &shell_ctx,
                &format!("/{command_name}"),
                desc.shell,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    emit_failed(
                        &bus,
                        "shell_expansion_error",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::Internal(format!(
                        "Skill {command_name} shell expansion failed: {e}"
                    )));
                }
            }
        };
        if let Some(skill_root) = desc.skill_root.as_ref() {
            let skill_dir = skill_root.to_string_lossy();
            let skill_dir = if cfg!(windows) {
                skill_dir.replace('\\', "/")
            } else {
                skill_dir.into_owned()
            };
            expanded_prompt =
                format!("Base directory for this skill: {skill_dir}\n\n{expanded_prompt}");
        }

        // ── `context: fork` ─────────────────────────────────────────────────
        // A forking skill does not expand into this conversation. It runs as a
        // subagent under its OWN permission scoping, normally in the
        // background, and this call returns a handle instead of the body.
        //
        // Every guard below degrades to the inline path rather than failing the
        // call (claude returns `null` from its launch helper and falls through),
        // with one exception: past the nesting cap AND out of spawn budget is a
        // hard error, because running inline there would hide an exhausted
        // chain from the model.
        if crate::fork::declares_fork(desc.context.as_deref())
            && crate::fork::is_forkable_skill_name(&command_name)
        {
            if let Some(res) = self
                .try_fork(&bus, &ctx, &desc, &command_name, &expanded_prompt)
                .await?
            {
                return Ok(res);
            }
        }

        // P2-12 / `zSr(e.name,u,d,r.agentId??null)`: record this invocation in
        // the process-global invoked-skill registry so its content can be
        // re-injected after a compaction (`rRg`) — the model regains the full
        // skill instructions across the compact boundary even though the
        // boundary summary drops the verbatim body. The content is the expanded
        // prompt the model receives (the binary's `d`); the path is the skill's
        // base dir (`u`). `agentId` is `None`: every LingXi orchestrator runs as
        // the main thread, so the key is `":{command_name}"`.
        let skill_path = desc.skill_root.clone().unwrap_or_default();
        compaction::invoked_skills::register(&command_name, &skill_path, &expanded_prompt, None);

        let new_messages = vec![protocol::ConversationMessage::user(
            protocol::MessageId::new(),
            expanded_prompt,
        )];

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
        //
        // `model_content` is the model-facing tool_result string. TS
        // `mapToolResultToToolResultBlockParam` (SkillTool.ts:843-862) makes the
        // model see ONLY a short line, never a JSON dump of the output object
        // (which would leak the full skill `body`). The orchestrator's
        // `tool_result_to_model_text` (turn_loop.rs) prefers `model_content`.
        // This tool only produces the inline path (TS `status:'inline'` default
        // → `Launching skill: ${commandName}`); the forked branch has no Rust
        // substrate (see module doc), so only the inline string is emitted.
        let mut data = json!({
            "success": true,
            "commandName": command_name,
            "status": "inline",
            "body": desc.body,
            "descriptor_truncated": truncated,
            "model_content": format!("Launching skill: {command_name}"),
        });
        // Capture the model override BEFORE `desc.model` is moved into the
        // result JSON below — it feeds the SKILLEXEC.3 `context_modifier`.
        let model_override = desc.model.clone();

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

        // SKILLEXEC.3 (model scope): when the skill declares `model:` in its
        // frontmatter, return a `context_modifier` that switches the session's
        // main-loop model for the rest of the session — 1:1 with TS
        // `SkillTool.ts:808-821` (`contextModifier` sets `options.mainLoopModel
        // = resolveSkillModelOverride(model, ctx.options.mainLoopModel)`),
        // including the `[1m]`-suffix preservation rule. The turn loop seeds the
        // closure's `ctx` with the live `session.model` (the `currentModel`
        // argument) and folds it POST-BATCH. When `model` is absent the modifier
        // stays `None`, so the no-override path is byte-identical.
        let context_modifier: Option<tool_api::ContextModifier> =
            model_override.map(|model| -> tool_api::ContextModifier {
                Box::new(move |mut ctx: ToolUseContext| {
                    let current = ctx.options.main_loop_model.clone();
                    ctx.options.main_loop_model =
                        crate::model_override::resolve_skill_model_override(&model, &current);
                    ctx
                })
            });

        Ok(ToolCallResult {
            data,
            model_content: None,
            new_messages,
            context_modifier,
            is_error: false,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
#[path = "skill_test.rs"]
mod skill_test;
