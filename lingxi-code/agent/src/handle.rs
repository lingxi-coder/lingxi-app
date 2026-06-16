//! `SubagentSpawner` trait impl.
//!
//! `PoolSubagentSpawner` wraps a `StateMachinePool` reference, allocates one
//! slot per spawn, and pumps the slot's `SubagentEvent` channel until a
//! terminal event arrives. The trait surface lives in `lingxi-traits` so
//! `AgentTool` in `lingxi-tools` can dispatch into the production pool
//! without taking a cyclic path-dep.
//!
//! The recursion-lock + budget-inheritance invariants flow through the
//! `SubagentInheritance` bundle (`Arc<dyn ToolInvoker>`,
//! `Arc<dyn BudgetEnforcerHandle>`) — the adapter stashes them on the child
//! `SubagentContext` so the child runner sees the same `Arc`s as the parent.

use crate::api::SubagentApiClient;
use crate::builtins::builtin_agent_definitions;
use crate::context::SubagentContext;
use crate::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use crate::display::{AgentColor, AgentDisplay};
use crate::pool::StateMachinePool;
use crate::runner::SubagentEvent;
use crate::tool_resolver::AgentToolResolver;
use async_trait::async_trait;
use protocol::{AgentId, ConversationMessage, MessageId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tool_api::tool_trait::{PromptOptions, ToolStaticContext};
use tool_api::ToolRegistry;
use traits::subagent_spawn::{
    SubagentInheritance, SubagentListingEntry, SubagentResult, SubagentSpawnError,
    SubagentSpawnRequest, SubagentSpawner, SubagentUsage,
};

/// Production [`SubagentSpawner`] backed by a [`StateMachinePool`].
///
/// Constructed and registered on the host `BuiltinToolContext` so
/// `AgentTool` can dispatch real subagent spawns. The parent's
/// `Arc<dyn ToolInvoker>` and `Arc<dyn BudgetEnforcerHandle>` arrive on
/// every `spawn` call via [`SubagentInheritance`] — the adapter is
/// responsible for handing those Arcs to the child's runner without
/// cloning them.
pub struct PoolSubagentSpawner {
    pool: Arc<StateMachinePool>,
    /// Optional model API seam handed to every child runner via the
    /// child's [`SubagentContext`]. `None` keeps the legacy stub behavior
    /// (the runner emits a synthetic completion without calling the model).
    api_client: Option<Arc<dyn SubagentApiClient>>,
    /// The live tool registry, used to resolve each spawn's advertised tools +
    /// allow-list PER-SPAWN: resolution reads whatever the registry holds at
    /// spawn time (rather than a one-time serialized snapshot taken at boot),
    /// so it auto-narrows once the spawn path loads real per-agent definitions.
    /// Unset (the default) = no tools. NOTE: `ToolRegistry` mutators take
    /// `&mut self`, so once shared as an immutable `Arc` here its contents are
    /// fixed — re-resolving per spawn reflects boot-time registry state, not
    /// live post-boot mutation (there is no `register_mcp_tools` caller on this
    /// `Arc` today; MCP tools flow through the separate `McpRegistry`).
    ///
    /// A SET-ONCE cell so the boot path can break the construction cycle: the
    /// spawner is consumed into the `BuiltinToolContext` that builds the registry,
    /// so the registry does not exist when the spawner is constructed. The host
    /// grabs a clone via [`Self::tool_registry_handle`] BEFORE boxing the spawner,
    /// then fills it AFTER the registry is built. Each `spawn` runs
    /// [`AgentToolResolver`] over the registry's `available_tools` per the child's
    /// [`AgentToolPolicy`], serializing the result into
    /// [`SubagentContext::tool_schemas`] (advertised) and recording the resolved
    /// names into [`SubagentContext::allowed_tools`] (the runner's dispatch
    /// allow-list). For today's hardcoded `AgentToolPolicy::All` default this is
    /// the full set (no filtering); it auto-narrows once the spawn path loads
    /// real per-agent definitions.
    tool_registry: Arc<std::sync::OnceLock<Arc<ToolRegistry>>>,
    /// The 6 built-in subagent definitions, keyed by `agent_type`. Built once
    /// in [`Self::new`] from [`builtin_agent_definitions`]. The spawn path
    /// resolves `subagent_type -> AgentDefinition` against this (overridden by
    /// the file catalog below) instead of fabricating a generic stub.
    builtins: Arc<HashMap<String, AgentDefinition>>,
    /// File-loaded user/project agent catalog (set-once, mirrors the registry
    /// cycle-break). When set it takes PRECEDENCE over [`Self::builtins`] on an
    /// `agent_type` collision — matching claude-code's later-wins ordering
    /// (built-in < user < project). Shares the SAME `Arc<RwLock<…>>` the
    /// orchestrator holds, so the spawner and `/agents` never drift. Unset (the
    /// default / tests) = built-ins only.
    agent_catalog: Arc<std::sync::OnceLock<Arc<RwLock<Vec<AgentDefinition>>>>>,
    /// Parent / main-loop model used as the `AgentModel::Inherit` target and the
    /// tier-match anchor when resolving a spawn's model preference to a concrete
    /// wire id (see [`crate::model_resolution::resolve_agent_model`]). Set at
    /// boot from `cfg.model` (a snapshot — a mid-session `/model` switch is not
    /// reflected; documented in `model_resolution`). `None` (the default /
    /// tests) leaves the definition's model string RAW (legacy behavior: the
    /// runner's `resolve_model` emits `Inherit`→`"inherit"` / the bare alias).
    default_model: Option<String>,
}

impl PoolSubagentSpawner {
    /// Construct an adapter wrapping `pool` with no API client (legacy stub
    /// runner). Use [`Self::with_api_client`] to enable the real multi-turn
    /// loop.
    #[must_use]
    pub fn new(pool: Arc<StateMachinePool>) -> Self {
        let builtins = builtin_agent_definitions()
            .into_iter()
            .map(|d| (d.agent_type.clone(), d))
            .collect();
        Self {
            pool,
            api_client: None,
            tool_registry: Arc::new(std::sync::OnceLock::new()),
            builtins: Arc::new(builtins),
            agent_catalog: Arc::new(std::sync::OnceLock::new()),
            default_model: None,
        }
    }

    /// Builder: set the parent / main-loop model used to resolve a spawn's
    /// `AgentModel::Inherit` and bare family aliases to a concrete wire id.
    /// Wire this from `cfg.model` at boot; without it, definition model strings
    /// are passed through raw (legacy). See [`crate::model_resolution`].
    #[must_use]
    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Builder: attach the model API seam the child runner uses to drive the
    /// real multi-turn loop. Without this, `spawn` produces stub completions.
    #[must_use]
    pub fn with_api_client(mut self, api_client: Arc<dyn SubagentApiClient>) -> Self {
        self.api_client = Some(api_client);
        self
    }

    /// Builder: set the live tool registry every spawn resolves its advertised
    /// tools + allow-list from. Sets the cell immediately — use this when the
    /// registry is available at construction (tests). The boot path instead uses
    /// [`Self::tool_registry_handle`] to fill the cell later (the registry does
    /// not exist yet at construction — see the field doc).
    #[must_use]
    pub fn with_tool_registry(self, registry: Arc<ToolRegistry>) -> Self {
        let _ = self.tool_registry.set(registry);
        self
    }

    /// Return a clone of the set-once registry cell so the host can fill it AFTER
    /// the registry is built (breaking the construction cycle). The cell is
    /// shared with the boxed spawner, so a later `cell.set(...)` is seen by every
    /// `spawn`. Filling more than once is a no-op (the first wins).
    #[must_use]
    pub fn tool_registry_handle(&self) -> Arc<std::sync::OnceLock<Arc<ToolRegistry>>> {
        self.tool_registry.clone()
    }

    /// Builder: set the file-loaded user/project agent catalog the spawn path
    /// resolves against (it overrides built-ins on `agent_type` collision). Sets
    /// the cell immediately — use when the catalog is available at construction
    /// (tests). The boot path instead fills it later via
    /// [`Self::agent_catalog_handle`] (the catalog does not exist when the
    /// spawner is boxed — same cycle as the registry).
    #[must_use]
    pub fn with_agent_catalog(self, catalog: Arc<RwLock<Vec<AgentDefinition>>>) -> Self {
        let _ = self.agent_catalog.set(catalog);
        self
    }

    /// Return a clone of the set-once agent-catalog cell so the host can fill it
    /// AFTER the catalog is built (breaking the construction cycle, exactly like
    /// [`Self::tool_registry_handle`]). The cell is shared with the boxed
    /// spawner; a later `cell.set(...)` is seen by every `spawn`. First fill wins.
    #[must_use]
    pub fn agent_catalog_handle(
        &self,
    ) -> Arc<std::sync::OnceLock<Arc<RwLock<Vec<AgentDefinition>>>>> {
        self.agent_catalog.clone()
    }

    /// Resolve a spawn's [`AgentDefinition`] from `subagent_type`, including its
    /// model preference.
    ///
    /// First looks the definition up by precedence (see [`Self::lookup_definition`]),
    /// then resolves its [`AgentModel`] to a concrete wire model id via
    /// [`crate::model_resolution::resolve_agent_model`] (when a `default_model`
    /// is wired): `Inherit`→parent model; a bare family alias→the parent's exact
    /// id when same-tier, else the family's concrete default id. Without
    /// `default_model` the model string is left RAW (legacy behavior).
    async fn resolve_definition(&self, subagent_type: &str) -> AgentDefinition {
        let mut def = self.lookup_definition(subagent_type).await;
        if let Some(parent_model) = &self.default_model {
            def.model = AgentModel::Explicit(crate::model_resolution::resolve_agent_model(
                &def.model,
                parent_model,
            ));
        }
        def
    }

    /// Look up the [`AgentDefinition`] for `subagent_type` by precedence.
    ///
    /// Precedence (claude-code parity — later wins): file catalog
    /// (user/project) overrides built-ins. An unknown type defaults to
    /// `general-purpose` (claude-code's `effectiveType ?? GENERAL_PURPOSE`); a
    /// last-resort all-tools stub covers the impossible empty-built-ins case.
    async fn lookup_definition(&self, subagent_type: &str) -> AgentDefinition {
        // 1. File catalog (user/project) wins on collision.
        if let Some(catalog) = self.agent_catalog.get() {
            if let Some(def) = catalog
                .read()
                .await
                .iter()
                .find(|d| d.agent_type == subagent_type)
                .cloned()
            {
                return def;
            }
        }
        // 2. Built-in by exact type.
        if let Some(def) = self.builtins.get(subagent_type).cloned() {
            return def;
        }
        // 3. Unknown type → general-purpose (matches claude-code's default).
        if let Some(def) = self.builtins.get("general-purpose").cloned() {
            return def;
        }
        // 4. Last resort (built-ins somehow empty): a permissive stub.
        Self::fallback_definition(subagent_type)
    }

    /// Minimal all-tools definition used only when neither the catalog nor the
    /// built-ins can supply one (built-ins always include `general-purpose`, so
    /// this is defensive). Uses the high built-in turn cap, not the old
    /// `max_turns: 1`, so a fallback agent can still run a tool-using loop.
    fn fallback_definition(subagent_type: &str) -> AgentDefinition {
        AgentDefinition {
            agent_type: subagent_type.into(),
            when_to_use: String::new(),
            tools: AgentToolPolicy::All {
                use_exact_tools: false,
            },
            max_turns: crate::builtins::BUILTIN_AGENT_MAX_TURNS,
            model: AgentModel::Inherit,
            permission_mode: AgentPermissionMode::Bubble,
            source: AgentSource::BuiltIn,
            base_dir: "built-in".into(),
            system_prompt: None,
            mcp_servers: vec![],
            frontmatter_hooks: vec![],
            icon: None,
            allowed_tools: vec![],
            worktree_requirement: None,
        }
    }

    /// Resolve a spawn's advertised tool schemas + dispatch allow-list from the
    /// live registry per `agent_def`'s [`AgentToolPolicy`]. Returns
    /// `(tool_schemas, allowed_tool_names)`. Unset registry → `(empty, empty)`
    /// (no tools advertised, allow-list guard skipped).
    async fn resolve_tools(
        &self,
        agent_def: &AgentDefinition,
    ) -> (Vec<serde_json::Value>, Vec<String>) {
        let Some(registry) = self.tool_registry.get() else {
            return (Vec::new(), Vec::new());
        };
        let parent_tools = registry.available_tools(&ToolStaticContext::default());
        let resolved = AgentToolResolver::resolve(agent_def, &parent_tools, &[], false);
        // The allow-list must cover the SAME surface the inherited
        // `RegistryToolInvoker` accepts: `find_by_name` matches a tool by
        // `name()` OR any `aliases()` entry (registry.rs). Building the list
        // from canonical names alone would leave a legacy alias (e.g.
        // `AgentTool`'s `"Task"`) advertised+dispatchable yet refused by the
        // runner guard. Include each resolved tool's aliases so the guard's
        // name set matches the invoker's. The advertised schemas stay
        // canonical-name-only — claude-code advertises the canonical name.
        let allowed: Vec<String> = resolved
            .iter()
            .flat_map(|t| {
                std::iter::once(t.name().to_string())
                    .chain(t.aliases().iter().map(|a| (*a).to_string()))
            })
            .collect();
        let schemas = tool_api::wire::tools_to_wire(
            &resolved,
            &PromptOptions {
                include_examples: true,
            },
        )
        .await;
        (schemas, allowed)
    }

    /// Build the child context from a RESOLVED [`AgentDefinition`] and the
    /// caller's task prompt.
    ///
    /// Prompt channels follow claude-code (AgentTool.tsx / runAgent.ts): the
    /// agent definition's body is the SYSTEM prompt
    /// ([`SubagentContext::rendered_system_prompt`]), and the caller's task
    /// `prompt` is the FIRST USER MESSAGE ([`SubagentContext::prompt_messages`])
    /// — distinct channels. (Previously the task prompt was jammed into
    /// `rendered_system_prompt` with no user message at all.) `None` system
    /// prompt = the model gets no system prompt, the correct semantic for a
    /// definition without a body.
    /// The `Notes:` trailer claude-code appends to every subagent system
    /// prompt. In TS this is the `notes` element prepended ahead of the
    /// `<env>` block by `enhanceSystemPromptWithEnvDetails`
    /// (claude-code/src/constants/prompts.ts:766-770), invoked for subagents
    /// via `getAgentSystemPrompt` (runAgent.ts:918) which returns
    /// `[agentBody, notes, envInfo]`.
    ///
    /// Byte-locked: the em-dash `—` (U+2014) appears once, in bullet 2; the
    /// literal carries NO trailing newline — TS keeps the `notes` array
    /// element newline-free and joins the following block with a blank line.
    /// (Same bytes as `orchestrator::prompt::locked_templates::FOOTER`, minus
    /// that copy's terminal `\n`; the orchestrator crate is not reachable from
    /// here — it depends on `agent` — so the literal is single-sourced locally.)
    ///
    /// NOTE: TS appends the `<env>` block (cwd / git / platform / shell / OS /
    /// resolved model + cutoff) AFTER this trailer. That block is NOT ported
    /// here: its byte-locked formatter lives in `orchestrator::prompt`
    /// (`env_block` + `env_meta`), unreachable from `agent` without a
    /// dependency cycle, and its inputs (the *resolved* model id, git/uname
    /// probes) are not available at this synchronous call site. See the
    /// SYSPROMPT.4 report for the unblock path.
    const SUBAGENT_NOTES_TRAILER: &'static str = "Notes:\n\
- Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths.\n\
- In your final response, share file paths (always absolute, never relative) that are relevant to the task. Include code snippets only when the exact text is load-bearing (e.g., a bug you found, a function signature the caller asked for) — do not recap code you merely read.\n\
- For clear communication with the user the assistant MUST avoid using emojis.\n\
- Do not use a colon before tool calls. Text like \"Let me read the file:\" followed by a read tool call should just be \"Let me read the file.\" with a period.";

    fn make_subagent_context(def: AgentDefinition, prompt: &str) -> SubagentContext {
        // claude-code renders the subagent system prompt as the agent body
        // followed by the env-details trailer (see `SUBAGENT_NOTES_TRAILER`).
        // Append the `Notes:` trailer after the body, joined by a blank line
        // (the codebase section separator, matching orchestrator's
        // `SECTION_SEP`). A `None` body stays `None` (no body, no trailer) —
        // the existing "definition without a body" semantic is preserved.
        let rendered_system_prompt: Option<Arc<str>> = def.system_prompt.as_deref().map(|body| {
            Arc::from(format!("{body}\n\n{}", Self::SUBAGENT_NOTES_TRAILER))
        });
        SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_definition: def,
            prompt_messages: vec![ConversationMessage::user(MessageId::new(), prompt.to_string())],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            is_async: false,
            persistent: false,
            can_show_permission_prompts: false,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            rendered_system_prompt,
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon: None,
            },
            // Set by `spawn` from `self.api_client` / `inherit.tool_invoker` /
            // `inherit.budget` just before pool allocation. `tool_schemas` +
            // `allowed_tools` are overwritten by `spawn` from `resolve_tools`
            // over the live registry per the resolved definition's policy.
            api_client: None,
            tool_invoker: None,
            tool_schemas: vec![],
            budget: None,
        }
    }

    /// Resolve the full subagent catalog (built-ins overlaid by the file
    /// catalog, claude-code later-wins precedence) into listing entries for
    /// the dynamic Agent tool prompt. Delegates to the crate-level
    /// [`crate::agent_listing_entries`] free fn (shared with the
    /// `agent_listing_delta` attachment path) after snapshotting the catalog.
    async fn listing_entries(&self) -> Vec<SubagentListingEntry> {
        // Snapshot built-ins + any wired catalog into one slice, then run the
        // shared merge. Built-ins are listed first; the shared fn applies
        // later-wins precedence so a same-named catalog entry overrides them.
        let mut defs: Vec<AgentDefinition> = self.builtins.values().cloned().collect();
        if let Some(catalog) = self.agent_catalog.get() {
            defs.extend(catalog.read().await.iter().cloned());
        }
        crate::agent_listing_entries(&defs)
    }
}

/// Render an [`AgentDefinition`]'s tool policy into the human "tools
/// description" claude-code shows for an agent type (`AgentTool/prompt.ts:15-37`
/// `getToolsDescription`). The Rust [`AgentToolPolicy`] folds TS's `tools`
/// (allowlist) + `disallowedTools` (denylist) into one enum, so the mapping is:
/// - `All { .. }` → `"All tools"` (no restrictions)
/// - `Explicit(names)` → `names.join(", ")` (allowlist), `"None"` if empty
/// - `Except(names)` → `"All tools except {names.join(", ")}"` (denylist)
#[must_use]
pub fn tools_description(def: &AgentDefinition) -> String {
    match &def.tools {
        AgentToolPolicy::All { .. } => "All tools".to_string(),
        AgentToolPolicy::Explicit(names) => {
            if names.is_empty() {
                "None".to_string()
            } else {
                names.join(", ")
            }
        }
        AgentToolPolicy::Except(names) => {
            format!("All tools except {}", names.join(", "))
        }
    }
}

/// Merge a flat slice of [`AgentDefinition`]s into the deduplicated
/// [`SubagentListingEntry`] set the dynamic Agent listing renders — the single
/// source of truth shared by the inline tool-prompt path
/// (`PoolSubagentSpawner::listing_entries`) and the `agent_listing_delta`
/// attachment path (the orchestrator's per-turn reminder).
///
/// Precedence is claude-code's later-wins: when two definitions share an
/// `agent_type`, the LAST one in `defs` wins. Callers therefore pass built-ins
/// FIRST and the user/project catalog AFTER (built-in < user < project). Each
/// entry's model is left unresolved (the listing only needs type / when-to-use
/// / tools). Output is sorted by `agent_type` for deterministic bytes (agent
/// load order is nondeterministic — plugin load races, MCP async connect —
/// matching TS `getAgentListingDeltaAttachment`'s sort, attachments.ts:1543).
#[must_use]
pub fn agent_listing_entries(defs: &[AgentDefinition]) -> Vec<SubagentListingEntry> {
    let mut by_type: HashMap<String, &AgentDefinition> = HashMap::new();
    for def in defs {
        // Later-wins: a same-typed definition later in the slice overrides.
        by_type.insert(def.agent_type.clone(), def);
    }
    let mut entries: Vec<SubagentListingEntry> = by_type
        .into_values()
        .map(|def| SubagentListingEntry {
            tools_description: tools_description(def),
            agent_type: def.agent_type.clone(),
            when_to_use: def.when_to_use.clone(),
        })
        .collect();
    entries.sort_by(|a, b| a.agent_type.cmp(&b.agent_type));
    entries
}

#[async_trait]
impl SubagentSpawner for PoolSubagentSpawner {
    async fn spawn(
        &self,
        request: SubagentSpawnRequest,
        // `inherit` carries the parent's Arc<dyn ToolInvoker> +
        // Arc<dyn BudgetEnforcerHandle>. The adapter stashes the tool invoker
        // on the child's `SubagentContext` so the recursion-lock + budget-
        // inheritance invariants survive across the spawn boundary; the
        // child runner dispatches `tool_use` blocks through the very same
        // `Arc<dyn ToolInvoker>` the parent holds.
        inherit: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError> {
        // Resolve the REAL definition for this subagent_type (file catalog
        // overrides built-ins; unknown → general-purpose). Its tools policy /
        // model / max_turns / system prompt flow into the runner, and its
        // policy drives the per-spawn tool resolution below.
        let mut def = self.resolve_definition(&request.subagent_type).await;
        // AgentTool spawn-surface parity: an explicit `model` from the caller
        // (TS schema `model: 'sonnet' | 'opus' | 'haiku'`) takes precedence
        // over the definition's model frontmatter (AgentTool.tsx:86). Resolve
        // the requested family to a concrete wire id via the same machinery
        // (`resolve_agent_model`) `resolve_definition` uses when a default
        // model is wired; without one, the bare alias is passed through raw
        // (legacy back-compat — the runner resolves it later).
        if let Some(model_pref) = request.model.as_deref() {
            let requested = AgentModel::Alias(model_pref.to_string());
            def.model = match &self.default_model {
                Some(parent) => AgentModel::Explicit(
                    crate::model_resolution::resolve_agent_model(&requested, parent),
                ),
                None => requested,
            };
        }
        // The other parity params (`name` / `team_name` / `mode` / `isolation`
        // / `cwd`) are carried on the request but their behavioral effects are
        // DEFERRED: teammate routing (name/team_name/mode), worktree/remote
        // isolation, and per-agent cwd override are separate features whose
        // wiring lands with the multi-agent + worktree spawn paths. They are
        // intentionally not faked here.
        let mut ctx = Self::make_subagent_context(def, &request.prompt);
        // Hand the child the parent's tool invoker, the parent's budget
        // enforcer, and our model API seam so the runner can drive the real
        // multi-turn loop and enforce the inherited budget per turn.
        ctx.tool_invoker = Some(inherit.tool_invoker);
        ctx.budget = Some(inherit.budget);
        ctx.api_client.clone_from(&self.api_client);
        // Resolve THIS spawn's advertised tools + dispatch allow-list from the
        // live registry per the child's policy (unset registry → no tools).
        // Populating `allowed_tools` here ACTIVATES the runner's dispatch guard
        // (runner.rs: an empty list = guard skipped). Its safety rests on the
        // allow-list covering every name the inherited `RegistryToolInvoker`
        // could dispatch — true because both derive from the same registry
        // snapshot (and `resolve_tools` now folds in aliases). A future change
        // that let the spawner's registry and the invoker's registry diverge
        // would have to re-establish that invariant.
        let (tool_schemas, allowed_tools) = self.resolve_tools(&ctx.agent_definition).await;
        ctx.tool_schemas = tool_schemas;
        ctx.allowed_tools = allowed_tools;
        let agent_id = ctx.agent_id;
        let (_aid, mut rx) = self
            .pool
            .allocate(ctx)
            .await
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))?;

        // Pump the slot until terminal. The runner emits Progress/Message
        // events as it streams turns; we ignore those here and surface only
        // the terminal Completed/Failed/Killed.
        let result = loop {
            match rx.recv().await {
                Some(SubagentEvent::Completed { result, .. }) => {
                    break SubagentResult::Completed {
                        content: result,
                        usage: SubagentUsage::default(),
                    };
                }
                Some(SubagentEvent::Failed { error, .. }) => {
                    break SubagentResult::Failed { reason: error };
                }
                Some(SubagentEvent::Killed { .. }) => {
                    break SubagentResult::Killed;
                }
                Some(_) => continue,
                None => {
                    break SubagentResult::Failed {
                        reason: "subagent channel closed unexpectedly".into(),
                    };
                }
            }
        };

        // Best-effort deallocate; failures here don't change the surfaced
        // result.
        let _ = self.pool.deallocate(&agent_id).await;
        Ok(result)
    }

    /// Surface the resolved subagent catalog (built-ins + user/project agents)
    /// so `AgentTool` can render its dynamic tool prompt. See
    /// [`Self::listing_entries`].
    async fn agent_listing(&self) -> Vec<SubagentListingEntry> {
        self.listing_entries().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
    use test_harness::mocks::MockRuntimeSpawner;
    use tool_api::Tool;
    use traits::budget::{BudgetEnforcerHandle, BudgetError};
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};

    struct DummyInvoker;

    #[async_trait]
    impl ToolInvoker for DummyInvoker {
        async fn invoke(
            &self,
            _: &str,
            _: Value,
            _: SubagentInvocationContext,
        ) -> Result<Value, ToolInvokerError> {
            Ok(Value::Null)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct DummyBudget;

    #[async_trait]
    impl BudgetEnforcerHandle for DummyBudget {
        async fn check_and_charge(&self, _: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    #[test]
    fn pool_spawner_constructs_with_arc_pool() {
        // The production wiring uses Arc<StateMachinePool>; this test
        // confirms the adapter accepts and stores the Arc cleanly. Driving
        // the runner end-to-end requires the M1.11 stub to receive an
        // inbound `engine::Event`, which lands when the agentic loop
        // arrives in Plan 09+.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let _spawner = PoolSubagentSpawner::new(pool);
    }

    /// Minimal stub tool with a configurable name + aliases (for the resolver
    /// tests).
    struct StubTool {
        name: &'static str,
        aliases: &'static [&'static str],
    }

    #[async_trait]
    impl Tool for StubTool {
        fn name(&self) -> &str {
            self.name
        }
        fn aliases(&self) -> &[&str] {
            self.aliases
        }
        fn input_schema(&self) -> &Value {
            static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| serde_json::json!({"type": "object"}))
        }
        fn is_enabled(&self, _ctx: &tool_api::tool_trait::ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &Value) -> bool {
            true
        }
        async fn check_permissions(
            &self,
            _input: &Value,
            _ctx: &tool_api::context::ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &Value,
            _opts: &tool_api::tool_trait::DescriptionOptions,
        ) -> String {
            self.name.into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            format!("{} tool prompt", self.name)
        }
        async fn call(
            &self,
            _input: Value,
            _ctx: tool_api::context::ToolUseContext,
            _tx: tool_api::progress::ToolProgressSender,
        ) -> Result<tool_api::tool_trait::ToolCallResult, tool_api::tool_trait::ToolError> {
            unreachable!("not invoked in this test")
        }
    }

    /// Build an `AgentDefinition` with the given tool policy (other fields are
    /// the spawn-path defaults).
    fn agent_def(tools: AgentToolPolicy) -> AgentDefinition {
        AgentDefinition {
            agent_type: "test".into(),
            when_to_use: String::new(),
            tools,
            max_turns: 1,
            model: AgentModel::Inherit,
            permission_mode: AgentPermissionMode::Bubble,
            source: AgentSource::BuiltIn,
            base_dir: "/tmp".into(),
            system_prompt: None,
            mcp_servers: vec![],
            frontmatter_hooks: vec![],
            icon: None,
            allowed_tools: vec![],
            worktree_requirement: None,
        }
    }

    fn registry_with(names: &[&'static str]) -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        for name in names {
            reg.register_builtin(Arc::new(StubTool {
                name,
                aliases: &[],
            }));
        }
        Arc::new(reg)
    }

    /// Like `agent_def` but in Plan permission mode, which makes
    /// [`AgentToolResolver`] retain only the read-only tool set.
    fn agent_def_plan(tools: AgentToolPolicy) -> AgentDefinition {
        AgentDefinition {
            permission_mode: AgentPermissionMode::Plan,
            ..agent_def(tools)
        }
    }

    #[test]
    fn registry_cell_starts_empty_and_late_fill_is_visible() {
        // The cycle-break primitive: the host grabs a handle, fills it AFTER the
        // registry exists, and the spawner's spawn-time read sees it.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);

        let cell = spawner.tool_registry_handle();
        assert!(cell.get().is_none(), "unset by default");
        let registry = registry_with(&["Read"]);
        assert!(cell.set(registry).is_ok(), "first fill wins");
        assert!(spawner.tool_registry_handle().get().is_some());
        // Set-once: a second fill is rejected.
        assert!(cell.set(registry_with(&[])).is_err());
    }

    #[tokio::test]
    async fn resolve_tools_unset_registry_is_empty() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        let (schemas, allowed) = spawner
            .resolve_tools(&agent_def(AgentToolPolicy::All {
                use_exact_tools: true,
            }))
            .await;
        assert!(schemas.is_empty());
        assert!(allowed.is_empty());
    }

    #[tokio::test]
    async fn resolve_tools_all_policy_advertises_full_set_and_allow_list() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Read", "Bash"]));

        let (schemas, allowed) = spawner
            .resolve_tools(&agent_def(AgentToolPolicy::All {
                use_exact_tools: true,
            }))
            .await;
        // Full set (sorted by name), and allow-list = resolved names.
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Bash", "Read"]);
        assert_eq!(allowed, vec!["Read".to_string(), "Bash".to_string()]);
    }

    #[tokio::test]
    async fn resolve_tools_explicit_policy_filters_advertised_and_allow_list() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Read", "Bash", "Edit"]));

        // Explicit allow-list: only "Read" survives — both the advertised set
        // AND the dispatch allow-list narrow together.
        let (schemas, allowed) = spawner
            .resolve_tools(&agent_def(AgentToolPolicy::Explicit(vec!["Read".to_string()])))
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Read"]);
        assert_eq!(allowed, vec!["Read".to_string()]);
    }

    #[tokio::test]
    async fn resolve_tools_includes_aliases_in_allow_list() {
        // The dispatch allow-list must accept every name the inherited invoker's
        // `find_by_name` accepts — including aliases — or a `tool_use` for a
        // legacy alias (e.g. AgentTool's "Task") would be wrongly refused by the
        // runner guard. Advertised schemas stay canonical-name-only.
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(StubTool {
            name: "Agent",
            aliases: &["Task"],
        }));
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool).with_tool_registry(Arc::new(reg));

        let (schemas, allowed) = spawner
            .resolve_tools(&agent_def(AgentToolPolicy::All {
                use_exact_tools: true,
            }))
            .await;
        // Advertised: canonical name only.
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Agent"]);
        // Allow-list: canonical name AND the legacy alias.
        assert_eq!(allowed, vec!["Agent".to_string(), "Task".to_string()]);
    }

    #[tokio::test]
    async fn resolve_tools_plan_mode_keeps_only_readonly() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Read", "Bash", "Grep", "WebFetch"]));

        // Plan permission mode retains only the read-only set
        // (Read/Grep/Glob/WebSearch/WebFetch) at BOTH advertisement and the
        // dispatch allow-list — `Bash` is dropped from both.
        let (schemas, allowed) = spawner
            .resolve_tools(&agent_def_plan(AgentToolPolicy::All {
                use_exact_tools: true,
            }))
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        // tools_to_wire sorts by name.
        assert_eq!(names, vec!["Grep", "Read", "WebFetch"]);
        assert!(!allowed.contains(&"Bash".to_string()));
        assert_eq!(
            allowed,
            vec![
                "Read".to_string(),
                "Grep".to_string(),
                "WebFetch".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn resolve_tools_except_policy() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Read", "Bash", "Edit"]));

        // Except drops the named tools from BOTH the advertised set and the
        // allow-list.
        let (schemas, allowed) = spawner
            .resolve_tools(&agent_def(AgentToolPolicy::Except(vec!["Bash".to_string()])))
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Edit", "Read"]); // sorted by name
        assert_eq!(allowed, vec!["Read".to_string(), "Edit".to_string()]); // resolved order
    }

    // ── batch 21: real AgentDefinition resolution + prompt placement ──

    #[tokio::test]
    async fn resolve_definition_returns_builtin_for_known_type() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        // Explore is a read-only built-in: Except the write tools, model haiku,
        // a real system prompt, and the high built-in turn cap (not the old 1).
        let def = spawner.resolve_definition("Explore").await;
        assert_eq!(def.agent_type, "Explore");
        assert!(matches!(def.tools, AgentToolPolicy::Except(_)));
        assert!(matches!(&def.model, AgentModel::Alias(m) if m == "haiku"));
        assert!(def.system_prompt.is_some());
        assert_eq!(def.max_turns, crate::builtins::BUILTIN_AGENT_MAX_TURNS);
    }

    #[tokio::test]
    async fn resolve_definition_unknown_defaults_to_general_purpose() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        let def = spawner.resolve_definition("no-such-agent").await;
        assert_eq!(def.agent_type, "general-purpose");
        assert!(matches!(def.tools, AgentToolPolicy::All { .. }));
    }

    #[tokio::test]
    async fn resolve_definition_catalog_overrides_builtin() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // A user/project agent named "Explore" must override the built-in.
        let base = agent_def(AgentToolPolicy::Explicit(vec!["Read".to_string()]));
        let custom = AgentDefinition {
            agent_type: "Explore".to_string(),
            system_prompt: Some("custom".to_string()),
            ..base
        };
        let catalog = Arc::new(RwLock::new(vec![custom]));
        let spawner = PoolSubagentSpawner::new(pool).with_agent_catalog(catalog);
        let def = spawner.resolve_definition("Explore").await;
        assert_eq!(def.agent_type, "Explore");
        // The catalog one (Explicit[Read]) wins over the built-in (Except[…]).
        assert!(matches!(def.tools, AgentToolPolicy::Explicit(_)));
        assert_eq!(def.system_prompt.as_deref(), Some("custom"));
    }

    // ── batch 22: model resolution wired into resolve_definition ──

    #[tokio::test]
    async fn resolve_definition_resolves_inherit_to_default_model() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // general-purpose is AgentModel::Inherit; with a default model wired it
        // resolves to that concrete parent model id.
        let spawner = PoolSubagentSpawner::new(pool).with_default_model("claude-opus-4-7");
        let def = spawner.resolve_definition("general-purpose").await;
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-opus-4-7"));
    }

    #[tokio::test]
    async fn resolve_definition_resolves_family_alias_to_concrete_id() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // Explore is Alias("haiku"); parent is opus (different tier) → resolves
        // to haiku's concrete default id, NOT the parent.
        let spawner = PoolSubagentSpawner::new(pool).with_default_model("claude-opus-4-7");
        let def = spawner.resolve_definition("Explore").await;
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-haiku-4-5"));
    }

    #[tokio::test]
    async fn resolve_definition_without_default_model_leaves_model_raw() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // No default model wired (legacy/tests): the alias is NOT resolved — the
        // runner's resolve_model then emits it raw (back-compat).
        let spawner = PoolSubagentSpawner::new(pool);
        let def = spawner.resolve_definition("Explore").await;
        assert!(matches!(&def.model, AgentModel::Alias(m) if m == "haiku"));
    }

    #[test]
    fn make_subagent_context_seeds_task_prompt_as_user_msg_and_def_body_as_system() {
        let def = AgentDefinition {
            system_prompt: Some("AGENT SYSTEM PROMPT".to_string()),
            ..agent_def(AgentToolPolicy::All {
                use_exact_tools: false,
            })
        };
        let ctx = PoolSubagentSpawner::make_subagent_context(def, "do the task");
        // Def body -> system prompt, with the appended `Notes:` env-details
        // trailer (claude-code `enhanceSystemPromptWithEnvDetails`). The body
        // stays first, joined to the trailer by a blank line.
        let sys = ctx.rendered_system_prompt.as_deref().unwrap();
        assert!(sys.starts_with("AGENT SYSTEM PROMPT\n\n"));
        assert!(sys.contains(
            "Notes:\n- Agent threads always have their cwd reset between bash calls"
        ));
        // Task prompt -> first (and only) user message (NOT the system slot).
        assert_eq!(ctx.prompt_messages.len(), 1);
        assert!(matches!(
            ctx.prompt_messages[0],
            ConversationMessage::User { .. }
        ));
        assert_eq!(ctx.prompt_messages[0].text_content(), "do the task");
    }

    #[test]
    fn make_subagent_context_appends_byte_locked_notes_trailer() {
        // SYSPROMPT.4: the subagent system prompt must carry the `Notes:`
        // env-details trailer claude-code appends via
        // `enhanceSystemPromptWithEnvDetails` (prompts.ts:766-770).
        let def = AgentDefinition {
            system_prompt: Some("AGENT BODY".to_string()),
            ..agent_def(AgentToolPolicy::All {
                use_exact_tools: false,
            })
        };
        let ctx = PoolSubagentSpawner::make_subagent_context(def, "task");
        let sys = ctx.rendered_system_prompt.as_deref().unwrap();
        // Body first, trailer joined by a blank line; whole string is exactly
        // `body \n\n trailer`.
        assert_eq!(
            sys,
            format!(
                "AGENT BODY\n\n{}",
                PoolSubagentSpawner::SUBAGENT_NOTES_TRAILER
            )
        );
        // All four byte-locked bullets, including the em-dash (U+2014) in
        // bullet 2 surviving byte-for-byte.
        assert!(sys.contains(
            "Notes:\n- Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths."
        ));
        assert!(sys.contains(
            "the caller asked for) — do not recap code you merely read."
        ));
        assert!(sys.contains("the assistant MUST avoid using emojis."));
        assert!(sys.contains(
            "just be \"Let me read the file.\" with a period."
        ));
        // No trailing newline — the `notes` element is newline-free in TS
        // (the next block, `<env>`, is joined with a blank line, not appended
        // to the notes literal).
        assert!(sys.ends_with("with a period."));
    }

    #[test]
    fn make_subagent_context_none_system_prompt_yields_no_system() {
        let def = AgentDefinition {
            system_prompt: None,
            ..agent_def(AgentToolPolicy::All {
                use_exact_tools: false,
            })
        };
        let ctx = PoolSubagentSpawner::make_subagent_context(def, "task");
        assert!(ctx.rendered_system_prompt.is_none());
        assert_eq!(ctx.prompt_messages[0].text_content(), "task");
    }

    #[tokio::test]
    async fn explore_definition_narrows_resolved_tools_to_read_only() {
        // End-to-end: resolve the Explore built-in, then resolve_tools over a
        // registry with write tools -> Edit/Write dropped from BOTH the
        // advertised schemas and the allow-list (the Except policy is now LIVE).
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Read", "Grep", "Edit", "Write"]));
        let def = spawner.resolve_definition("Explore").await;
        let (schemas, allowed) = spawner.resolve_tools(&def).await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Grep", "Read"]); // sorted; Edit+Write dropped
        assert!(allowed.contains(&"Read".to_string()));
        assert!(allowed.contains(&"Grep".to_string()));
        assert!(!allowed.contains(&"Edit".to_string()));
        assert!(!allowed.contains(&"Write".to_string()));
    }

    // ── AgentTool spawn-surface parity (coordinator batch D2a) ──

    #[tokio::test]
    async fn agent_listing_surfaces_builtins_with_tools_description() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        let entries = spawner.agent_listing().await;
        // All 6 built-ins, sorted by type.
        assert_eq!(entries.len(), 6);
        let by: std::collections::HashMap<&str, &SubagentListingEntry> =
            entries.iter().map(|e| (e.agent_type.as_str(), e)).collect();
        // general-purpose: All { .. } → "All tools".
        assert_eq!(by["general-purpose"].tools_description, "All tools");
        // Explore: Except([Agent, ExitPlanMode, Edit, Write, NotebookEdit]).
        assert_eq!(
            by["Explore"].tools_description,
            "All tools except Agent, ExitPlanMode, Edit, Write, NotebookEdit"
        );
        // statusline-setup: Explicit([Read, Edit]).
        assert_eq!(by["statusline-setup"].tools_description, "Read, Edit");
        // when_to_use carried through verbatim.
        assert!(by["Explore"].when_to_use.contains("exploring codebases"));
    }

    #[tokio::test]
    async fn agent_listing_catalog_overrides_builtin() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let custom = AgentDefinition {
            agent_type: "Explore".to_string(),
            when_to_use: "CUSTOM EXPLORE".to_string(),
            ..agent_def(AgentToolPolicy::Explicit(vec!["Read".to_string()]))
        };
        let catalog = Arc::new(RwLock::new(vec![custom]));
        let spawner = PoolSubagentSpawner::new(pool).with_agent_catalog(catalog);
        let entries = spawner.agent_listing().await;
        let explore = entries.iter().find(|e| e.agent_type == "Explore").unwrap();
        // The catalog entry (Explicit[Read] → "Read") wins over the built-in.
        assert_eq!(explore.when_to_use, "CUSTOM EXPLORE");
        assert_eq!(explore.tools_description, "Read");
        // Still 6 (override, not addition).
        assert_eq!(entries.len(), 6);
    }

    #[test]
    fn tools_description_maps_empty_explicit_to_none() {
        let def = agent_def(AgentToolPolicy::Explicit(vec![]));
        assert_eq!(crate::tools_description(&def), "None");
    }

    #[test]
    fn agent_listing_entries_merges_builtins_and_catalog_later_wins() {
        // built-ins FIRST, then a catalog override for a same-named type.
        let mut defs = builtin_agent_definitions();
        let n_builtins = defs.len();
        defs.push(AgentDefinition {
            agent_type: "Explore".to_string(),
            when_to_use: "CATALOG OVERRIDE".to_string(),
            ..agent_def(AgentToolPolicy::Explicit(vec!["Read".to_string()]))
        });
        // …and a brand-new type only the catalog defines.
        defs.push(AgentDefinition {
            agent_type: "custom-agent".to_string(),
            when_to_use: "a project agent".to_string(),
            ..agent_def(AgentToolPolicy::All { use_exact_tools: false })
        });

        let entries = crate::agent_listing_entries(&defs);
        // Override replaces (not adds); the brand-new type is +1.
        assert_eq!(entries.len(), n_builtins + 1);

        let by: std::collections::HashMap<&str, &SubagentListingEntry> =
            entries.iter().map(|e| (e.agent_type.as_str(), e)).collect();
        // Later-wins: the catalog Explore overrides the built-in.
        assert_eq!(by["Explore"].when_to_use, "CATALOG OVERRIDE");
        assert_eq!(by["Explore"].tools_description, "Read");
        // The catalog-only agent is present.
        assert_eq!(by["custom-agent"].tools_description, "All tools");
        // Deterministic sort by agent_type.
        let mut sorted = entries.clone();
        sorted.sort_by(|a, b| a.agent_type.cmp(&b.agent_type));
        assert_eq!(entries, sorted);
    }

    #[tokio::test]
    async fn spawn_request_model_override_takes_precedence() {
        // The caller's `model` (AgentTool schema) overrides the definition's
        // model; with a default model wired it resolves to a concrete wire id.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool).with_default_model("claude-opus-4-7");
        // general-purpose is Inherit; request a haiku override → resolves to the
        // concrete haiku id (different tier from the opus parent).
        let mut req = SubagentSpawnRequest {
            subagent_type: "general-purpose".to_string(),
            prompt: "go".to_string(),
            context_paths: vec![],
            description: None,
            model: Some("haiku".to_string()),
            name: None,
            team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
        };
        // Drive resolve_definition + the override branch directly by replicating
        // the spawn-path logic (spawn() would require a live runner).
        let mut def = spawner.resolve_definition(&req.subagent_type).await;
        if let Some(model_pref) = req.model.as_deref() {
            let requested = AgentModel::Alias(model_pref.to_string());
            def.model = AgentModel::Explicit(crate::model_resolution::resolve_agent_model(
                &requested,
                "claude-opus-4-7",
            ));
        }
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-haiku-4-5"));
        // Sanity: the request struct carries the rest of the parity params.
        req.name = Some("scout".into());
        assert_eq!(req.name.as_deref(), Some("scout"));
    }

    #[test]
    fn inheritance_carries_invoker_and_budget_arcs() {
        let inherit = SubagentInheritance {
            tool_invoker: Arc::new(DummyInvoker),
            budget: Arc::new(DummyBudget),
        };
        // Arc::ptr_eq round-trip — the trait-object Arcs are clonable and
        // equality survives clone (used by the recursion-lock + budget-
        // inheritance tests in lingxi-tools).
        let cloned = inherit.clone();
        assert!(Arc::ptr_eq(&inherit.tool_invoker, &cloned.tool_invoker));
        assert!(Arc::ptr_eq(&inherit.budget, &cloned.budget));
    }
}
