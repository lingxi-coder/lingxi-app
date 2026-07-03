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
use permission::PermissionMode;
use crate::display::{AgentColor, AgentDisplay};
use crate::pool::StateMachinePool;
use crate::runner::SubagentEvent;
use async_trait::async_trait;
use protocol::{AgentId, ConversationMessage, MessageId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
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
    /// allow-list). Even under `AgentToolPolicy::All` the resolver strips the
    /// always-disallowed agent-tool set (`Agent`/`TaskOutput`/`ExitPlanMode`/
    /// `EnterPlanMode`/`AskUserQuestion`/`TaskStop`, gated by `USER_TYPE !==
    /// 'ant'`) plus the definition's own `disallowed_tools`, so a
    /// general-purpose child no longer inherits `Agent`/`Task`; it narrows
    /// further once the spawn path loads real per-agent definitions.
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
    /// Live/boot permission-mode anchor threaded into
    /// [`crate::model_resolution::resolve_agent_model`] so an `AgentModel::Inherit`
    /// spawn gets the plan-mode runtime resolution (`opusplan`→Opus / `haiku`→
    /// Sonnet) when `permission_mode == Plan`. Default `PermissionMode::Default`
    /// (the common case → the Inherit branch returns the parent model unchanged,
    /// byte-identical to before this seam).
    permission_mode: PermissionMode,
    /// RAW user model setting string (mirrors claude-code
    /// `getUserSpecifiedModelSetting()`, e.g. `"opusplan"` / `"haiku"` / `None`)
    /// — NOT the resolved id. Used ONLY for the opusplan/haiku plan-mode runtime
    /// resolution in [`crate::model_resolution::resolve_agent_model`]. Without it
    /// (the default) the Inherit branch returns the parent model unchanged
    /// (faithful: a non-opusplan setting never triggers the plan-mode swap).
    model_setting: Option<String>,
    /// LingXi multi-provider half of the 2.1.198 `GAe`/`obm` firstParty gate
    /// (`fr() !== "firstParty"`): `false` when the session's default model
    /// routes to a non-Anthropic provider profile (OpenAI/Gemini/…), which
    /// makes the built-in Explore agent resolve to `"inherit"` exactly like
    /// the TS non-firstParty branch. Default `true` (the Anthropic default
    /// install); the env half (Bedrock/Vertex/Foundry) is checked inside
    /// [`crate::model_resolution::resolve_builtin_explore_model`].
    session_provider_first_party: bool,
    /// Hook executor handed to every child runner via
    /// [`SubagentContext::hook_executor`] so the runner can fire `SubagentStart`
    /// (collecting + injecting the hooks' `additionalContexts`, claude
    /// runAgent.ts:530-555) and register/clear the agent's frontmatter hooks
    /// (Stop→SubagentStop, runAgent.ts:557-575). A SET-ONCE cell mirroring
    /// [`Self::tool_registry`]: the executor is built AFTER the spawner is boxed
    /// (it consumes the spawner via `with_agent_spawner`), so the boot path grabs
    /// [`Self::hook_executor_handle`] before boxing and fills it once the executor
    /// exists. Unfilled (the default / tests) ⇒ the child runner skips the
    /// SubagentStart fire + frontmatter-hook registration (byte-identical legacy).
    hook_executor: Arc<std::sync::OnceLock<Arc<hooks::HookExecutorImpl>>>,
    /// Skill loader handed to every child runner via
    /// [`SubagentContext::skill_loader`] so the runner can preload the agent
    /// definition's frontmatter `skills:` (claude runAgent.ts:577-646). A leaf
    /// trait ([`traits::skill_loader::SkillLoader`]) so the agent crate avoids a
    /// cycle into the command/skill registry; the concrete impl is built at the
    /// composition root. SET-ONCE cell (same cycle-break as the others). Unfilled
    /// ⇒ no skill preloading (byte-identical legacy).
    skill_loader: Arc<std::sync::OnceLock<Arc<dyn traits::skill_loader::SkillLoader>>>,
    /// Session id stamped on the `HookContext` the child runner builds for the
    /// SubagentStart fire (claude `createBaseHookInput`). Set at boot via
    /// [`Self::with_hook_context`]; defaults to a nil session (only consulted when
    /// [`Self::hook_executor`] is filled).
    hook_session_id: protocol::SessionId,
    /// Engine cwd stamped on that `HookContext`. Set at boot via
    /// [`Self::with_hook_context`]; defaults to an empty path.
    hook_cwd: std::path::PathBuf,
    /// FIX C: the MAIN session's subagents directory —
    /// `<lingxi_home>/projects/<sanitize(cwd)>/<session_uuid>/subagents`
    /// (claude-code `getAgentTranscriptPath`'s base dir). Precomputed at the
    /// composition root and set at boot via [`Self::with_hook_context`] (the
    /// `agent` crate has no `session`/`orchestrator` dep to derive it, so the host
    /// — which does — passes the finished `PathBuf`). When `Some`, [`Self::spawn`]
    /// seeds each child's `transcript_subdir` from it so the agent-scoped
    /// `SubagentStop`'s `agent_transcript_path` is the true session-scoped
    /// `…/subagents/agent-<id>.jsonl` instead of the prior `/tmp` placeholder.
    /// `None` ⇒ the `/tmp` placeholder stands (byte-identical legacy).
    hook_subagents_dir: Option<std::path::PathBuf>,
    /// G14: name → child agent-id registry for `SendMessage` routing of spawned
    /// ASYNC subagents (claude `AppState.agentNameRegistry`, AgentTool.tsx:704-711).
    /// `AgentTool` calls [`SubagentSpawner::register_name`] after a successful
    /// async spawn that carried a `name`; a `SendMessage({ to: name })` resolver
    /// reads it via [`SubagentSpawner::resolve_name`]. Shared `Arc` so the same
    /// map is visible across spawner clones. Sync agents are NOT registered.
    name_registry: Arc<RwLock<HashMap<String, AgentId>>>,
    /// TOOL-WIDE deny-rule names from the boot permission policy, applied in
    /// [`Self::resolve_tools`] so a blanket-denied tool never leaks into a
    /// subagent's advertised wire `tools` array — matching claude-code, where
    /// `assembleToolPool` (the SAME pool builder used for coordinator workers,
    /// `runAgent.ts`) runs `filterToolsByDenyRules`. A SET-ONCE cell mirroring
    /// [`Self::tool_registry`]: the permission policy is built at the composition
    /// root AFTER the spawner is boxed, so the host fills this once it exists via
    /// [`Self::tool_wide_deny_names_handle`]. Unfilled (the default / tests /
    /// no-enforcement) ⇒ NO names ⇒ the subagent tool pool is UNCHANGED
    /// (byte-identical / regression-safe). Each entry is matched against a
    /// resolved tool's name by [`permission::tool_wide_name_matches`] (exact name
    /// OR an `mcp__server` prefix).
    tool_wide_deny_names: Arc<std::sync::OnceLock<Vec<String>>>,
    /// Renderer for the subagent `<env>` block claude-code 2.1.186 appends to a
    /// NON-fork subagent's system prompt after the `Notes:` trailer (`tIm` — see
    /// `orchestrator::prompt::subagent_env`). Given a RESOLVED model id it returns
    /// the byte-exact block (cwd / git / platform / shell / OS / model + cutoff);
    /// the static environment inputs are captured at the composition root. A
    /// SET-ONCE cell mirroring [`Self::tool_wide_deny_names`]: the renderer is
    /// built at the composition root (which can reach the orchestrator formatter +
    /// the git/uname probes; the `agent` crate cannot, to avoid a dep cycle) and
    /// filled via [`Self::subagent_env_renderer_handle`] after the spawner is
    /// boxed. Unfilled (the default / tests) ⇒ NO env block appended
    /// (byte-identical legacy). Fork spawns NEVER get it (the parent's rendered
    /// prompt is replayed verbatim — no `enhanceSystemPromptWithEnvDetails`).
    subagent_env_renderer: Arc<std::sync::OnceLock<SubagentEnvRenderer>>,
}

/// Renders the subagent `<env>` block for a resolved model id (claude-code
/// `tIm`). The static environment is captured by the closure at the composition
/// root; only the resolved model id varies per spawn.
pub type SubagentEnvRenderer =
    Arc<dyn Fn(&str, Option<&std::path::Path>) -> String + Send + Sync>;

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
            permission_mode: PermissionMode::Default,
            model_setting: None,
            session_provider_first_party: true,
            hook_executor: Arc::new(std::sync::OnceLock::new()),
            skill_loader: Arc::new(std::sync::OnceLock::new()),
            hook_session_id: protocol::SessionId::nil(),
            hook_cwd: std::path::PathBuf::new(),
            hook_subagents_dir: None,
            name_registry: Arc::new(RwLock::new(HashMap::new())),
            tool_wide_deny_names: Arc::new(std::sync::OnceLock::new()),
            subagent_env_renderer: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Return a clone of the set-once subagent-`<env>`-renderer cell so the host
    /// can fill it AFTER the orchestrator env formatter + probes are available
    /// (same cycle-break as [`Self::tool_wide_deny_names_handle`]). First fill
    /// wins. Unfilled ⇒ no env block appended (byte-identical legacy).
    #[must_use]
    pub fn subagent_env_renderer_handle(
        &self,
    ) -> Arc<std::sync::OnceLock<SubagentEnvRenderer>> {
        self.subagent_env_renderer.clone()
    }

    /// Builder: set the subagent `<env>` renderer immediately (tests). The boot
    /// path uses [`Self::subagent_env_renderer_handle`] to fill it later.
    #[must_use]
    pub fn with_subagent_env_renderer(self, renderer: SubagentEnvRenderer) -> Self {
        let _ = self.subagent_env_renderer.set(renderer);
        self
    }

    /// Return a clone of the set-once tool-wide-deny-names cell so the host can
    /// fill it AFTER the permission policy is built (same cycle-break as
    /// [`Self::tool_registry_handle`]). The subagent tool resolver
    /// ([`Self::resolve_tools`]) then strips any blanket-denied tool from each
    /// child's advertised pool (claude-code `filterToolsByDenyRules`). First fill
    /// wins. Unfilled ⇒ no filtering (byte-identical legacy).
    #[must_use]
    pub fn tool_wide_deny_names_handle(&self) -> Arc<std::sync::OnceLock<Vec<String>>> {
        self.tool_wide_deny_names.clone()
    }

    /// Builder: set the tool-wide deny names immediately (tests). The boot path
    /// uses [`Self::tool_wide_deny_names_handle`] to fill it later (the policy
    /// does not exist at construction). Applied in [`Self::resolve_tools`].
    #[must_use]
    pub fn with_tool_wide_deny_names(self, names: Vec<String>) -> Self {
        let _ = self.tool_wide_deny_names.set(names);
        self
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

    /// Builder: set the live/boot permission-mode anchor threaded into
    /// `resolve_agent_model` (so an `AgentModel::Inherit` spawn gets the plan-mode
    /// runtime resolution `opusplan`→Opus / `haiku`→Sonnet when in plan mode).
    /// Without it the default (`PermissionMode::Default`) keeps the Inherit branch
    /// returning the parent model unchanged.
    #[must_use]
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Builder: set the RAW user model setting string (mirrors claude-code
    /// `getUserSpecifiedModelSetting()`, e.g. `"opusplan"` / `"haiku"`). Used ONLY
    /// for the opusplan/haiku plan-mode runtime resolution; without it the Inherit
    /// branch returns the parent model unchanged (a non-opusplan setting never
    /// triggers the plan-mode swap).
    #[must_use]
    pub fn with_model_setting(mut self, setting: impl Into<String>) -> Self {
        self.model_setting = Some(setting.into());
        self
    }

    /// Builder: LingXi multi-provider half of the 2.1.198 Explore firstParty
    /// gate — pass `false` when the session's default model routes to a
    /// non-Anthropic provider profile so the built-in Explore agent resolves
    /// to `inherit` (never the opus cap). See the field docs.
    #[must_use]
    pub fn with_session_provider_first_party(mut self, first_party: bool) -> Self {
        self.session_provider_first_party = first_party;
        self
    }

    /// Builder: attach the model API seam the child runner uses to drive the
    /// real multi-turn loop. Without this, `spawn` produces stub completions.
    #[must_use]
    pub fn with_api_client(mut self, api_client: Arc<dyn SubagentApiClient>) -> Self {
        self.api_client = Some(api_client);
        self
    }

    /// Builder: set the hook executor immediately (use when it is available at
    /// construction — tests). The boot path instead uses
    /// [`Self::hook_executor_handle`] to fill the cell later (the executor
    /// consumes the spawner, so it does not exist at construction). See the field
    /// doc. Threaded onto every child via [`SubagentContext::hook_executor`].
    #[must_use]
    pub fn with_hook_executor(self, executor: Arc<hooks::HookExecutorImpl>) -> Self {
        let _ = self.hook_executor.set(executor);
        self
    }

    /// Return a clone of the set-once hook-executor cell so the host can fill it
    /// AFTER the executor is built (breaking the construction cycle, exactly like
    /// [`Self::tool_registry_handle`]). First fill wins; later fills are no-ops.
    #[must_use]
    pub fn hook_executor_handle(&self) -> Arc<std::sync::OnceLock<Arc<hooks::HookExecutorImpl>>> {
        self.hook_executor.clone()
    }

    /// Builder: set the skill loader immediately (tests). The boot path uses
    /// [`Self::skill_loader_handle`] to fill it later. Threaded onto every child
    /// via [`SubagentContext::skill_loader`].
    #[must_use]
    pub fn with_skill_loader(self, loader: Arc<dyn traits::skill_loader::SkillLoader>) -> Self {
        let _ = self.skill_loader.set(loader);
        self
    }

    /// Return a clone of the set-once skill-loader cell so the host can fill it
    /// AFTER the concrete loader is built (same cycle-break as the others). First
    /// fill wins.
    #[must_use]
    pub fn skill_loader_handle(
        &self,
    ) -> Arc<std::sync::OnceLock<Arc<dyn traits::skill_loader::SkillLoader>>> {
        self.skill_loader.clone()
    }

    /// Builder: set the session id + cwd stamped on the `HookContext` the child
    /// runner builds for the SubagentStart fire, plus the precomputed
    /// `subagents_dir` used to seed each spawned child's REAL `transcript_subdir`
    /// (FIX C — `<lingxi_home>/projects/<sanitize(cwd)>/<session>/subagents`,
    /// supplied by the host since `agent` has no `session`/`orchestrator` dep to
    /// derive it). Without it the defaults (nil session / empty cwd / no subagents
    /// dir ⇒ `/tmp` placeholder subdir) are used — only consulted when a hook
    /// executor is wired. Pass `subagents_dir = None` to keep the legacy `/tmp`
    /// placeholder.
    #[must_use]
    pub fn with_hook_context(
        mut self,
        session_id: protocol::SessionId,
        cwd: std::path::PathBuf,
        subagents_dir: Option<std::path::PathBuf>,
    ) -> Self {
        self.hook_session_id = session_id;
        self.hook_cwd = cwd;
        self.hook_subagents_dir = subagents_dir;
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
            // 2.1.198 `GAe`: the built-in Explore definition's model is derived
            // from the SESSION model (inherit, capped at "opus" for
            // fable/mythos-class firstParty sessions) BEFORE the normal
            // alias/Inherit resolution. Non-Explore / non-built-in definitions
            // pass through unchanged.
            def.model = crate::model_resolution::resolve_builtin_explore_model(
                &def,
                parent_model,
                self.session_provider_first_party,
            );
            def.model = AgentModel::Explicit(crate::model_resolution::resolve_agent_model(
                &def.model,
                parent_model,
                self.permission_mode,
                self.model_setting.as_deref(),
            ));
        }
        def
    }

    /// Look up the [`AgentDefinition`] for `subagent_type` by precedence.
    ///
    /// Precedence (claude-code parity — later wins): file catalog
    /// (user/project) overrides built-ins. An unknown type defaults to
    /// `general-purpose`; a last-resort all-tools stub covers the impossible
    /// empty-built-ins case.
    ///
    /// NOTE: in claude-code the unknown→general-purpose fallback only fires when
    /// `subagent_type` is OMITTED (`effectiveType ?? GENERAL_PURPOSE`,
    /// AgentTool.tsx:322); an EXPLICIT unknown type throws `Agent type 'x' not
    /// found`. That distinction is enforced UPSTREAM in `AgentTool::call`
    /// (tools/agent), which validates an explicit type against `agent_listing()`
    /// before spawning, so this method only ever receives an omitted (→
    /// general-purpose) or a resolvable type from the tool path. Internal
    /// callers that bypass the tool still get the permissive fallback.
    async fn lookup_definition(&self, subagent_type: &str) -> AgentDefinition {
        // 0. Fork path (codex #5): the synthetic FORK_AGENT is resolved FIRST,
        // unconditionally, so a user agent literally named "fork" cannot shadow
        // it (claude uses the synthetic FORK_AGENT on the fork path, never the
        // catalog — forkSubagent.ts:60-71 / AgentTool.tsx:335). It is NOT in the
        // 6-element built-in vec (claude does not register it in builtInAgents).
        if subagent_type == traits::fork_subagent::FORK_SUBAGENT_TYPE {
            return crate::builtins::fork_agent_definition();
        }
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
            // Defensive stub: no extended frontmatter — all defaults.
            disallowed_tools: vec![],
            skills: vec![],
            required_mcp_servers: vec![],
            background: false,
            isolation: None,
            memory: None,
            effort: None,
            initial_prompt: None,
            color: None,
        }
    }

    /// Resolve a spawn's advertised tool schemas + dispatch allow-list from the
    /// live registry per `agent_def`'s [`AgentToolPolicy`]. Returns
    /// `(tool_schemas, allowed_tool_names)`. Unset registry → `(empty, empty)`
    /// (no tools advertised, allow-list guard skipped).
    async fn resolve_tools(
        &self,
        agent_def: &AgentDefinition,
        // The resolved subagent's own recursion depth — gates its `Agent` tool
        // at `depth < 5` (claude `e9t`). Threaded from `request.depth`.
        depth: u32,
    ) -> (Vec<serde_json::Value>, Vec<String>) {
        let Some(registry) = self.tool_registry.get() else {
            return (Vec::new(), Vec::new());
        };
        // Delegate to the shared resolver (single source of truth, also used by
        // the in-process teammate handler). The tool-wide deny names come from the
        // boot policy via the set-once cell (UNFILLED / EMPTY ⇒ no tools dropped,
        // regression-safe); the resolved model anchors the model-gated tool prompt.
        let empty: Vec<String> = Vec::new();
        let denied = self.tool_wide_deny_names.get().unwrap_or(&empty);
        crate::tool_resolver::resolve_subagent_tools(
            registry,
            agent_def,
            denied,
            self.default_model.as_deref(),
            depth,
        )
        .await
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
    /// NOTE: claude-code 2.1.186 appends the `<env>` block (cwd / git / platform
    /// / shell / OS / resolved model + cutoff) AFTER this trailer (`tIm`). Its
    /// byte-locked formatter lives in `orchestrator::prompt::subagent_env`,
    /// unreachable from `agent` without a dependency cycle, so the composition
    /// root renders it into a [`SubagentEnvRenderer`] closure (capturing the
    /// git/uname/cwd probes) and fills [`Self::subagent_env_renderer`]; the
    /// non-fork [`Self::build_subagent_context`] path invokes it with the spawn's
    /// resolved model id and appends the result here.
    const SUBAGENT_NOTES_TRAILER: &'static str = "Notes:\n\
- Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths.\n\
- In your final response, share file paths (always absolute, never relative) that are relevant to the task. Include code snippets only when the exact text is load-bearing (e.g., a bug you found, a function signature the caller asked for) — do not recap code you merely read.\n\
- For clear communication with the user the assistant MUST avoid using emojis.\n\
- Do not use a colon before tool calls. Text like \"Let me read the file:\" followed by a read tool call should just be \"Let me read the file.\" with a period.\n\
- Do NOT Write report/summary/findings/analysis .md files. Return findings directly as your final assistant message — the parent agent reads your text output, not files you create. (Files written as input to another tool are fine; this note is about report files.)";

    /// Build the child context from a RESOLVED [`AgentDefinition`] + the
    /// caller's task prompt, plus the optional fork carriers.
    ///
    /// Non-fork path (`fork_*` both `None`): the agent body becomes the system
    /// prompt with the appended `Notes:` trailer, and the task `prompt` is the
    /// first (and only) user message — byte-identical to before codex #5.
    ///
    /// Fork path (codex #5):
    /// - `fork_parent_system_prompt = Some` → use the parent's already-rendered
    ///   bytes VERBATIM as the system prompt and SKIP the `Notes:` trailer
    ///   (re-appending it would bust the prompt cache; claude passes
    ///   `override.systemPrompt` verbatim with no
    ///   `enhanceSystemPromptWithEnvDetails`, AgentTool.tsx:622-623).
    /// - `fork_context_messages = Some` → seed `ctx.fork_context_messages` with
    ///   the byte-exact forked prefix and leave `prompt_messages` EMPTY (the
    ///   directive is already the trailing Text block inside that prefix, built
    ///   by `build_forked_messages`; `runner.rs` replays
    ///   `fork_context_messages ++ prompt_messages`, so `[]` prompt_messages
    ///   yields exactly the forked prefix — AgentTool.tsx:630 / spec note (A)).
    fn make_subagent_context(
        def: AgentDefinition,
        prompt: &str,
        fork_context_messages: Option<Vec<ConversationMessage>>,
        fork_parent_system_prompt: Option<String>,
    ) -> SubagentContext {
        // System prompt: fork path uses the parent's rendered bytes verbatim
        // (no trailer); non-fork path = agent body + the `Notes:` trailer
        // (claude `enhanceSystemPromptWithEnvDetails`). A `None` body on the
        // non-fork path stays `None` (no body, no trailer).
        let rendered_system_prompt: Option<Arc<str>> = match &fork_parent_system_prompt {
            Some(parent) => Some(Arc::from(parent.as_str())),
            None => def
                .system_prompt
                .as_deref()
                .map(|body| Arc::from(format!("{body}\n\n{}", Self::SUBAGENT_NOTES_TRAILER))),
        };
        // Fork path seeds prompt_messages EMPTY (the directive lives in the fork
        // prefix); non-fork path seeds it with the task prompt user message.
        let is_fork = fork_context_messages.is_some();
        let prompt_messages = if is_fork {
            vec![]
        } else {
            vec![ConversationMessage::user(MessageId::new(), prompt.to_string())]
        };
        SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_name: None,
            team_name: None,
            agent_definition: def,
            prompt_messages,
            fork_context_messages,
            allowed_tools: vec![],
            worktree_handle: None,
            // Set by `build_subagent_context` from the resolved isolation/cwd.
            cwd: None,
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
            // Set by `build_subagent_context` from `request.model_profile`.
            model_profile: None,
            // Set by `spawn` from `self.api_client` / `inherit.tool_invoker` /
            // `inherit.budget` just before pool allocation. `tool_schemas` +
            // `allowed_tools` are overwritten by `spawn` from `resolve_tools`
            // over the live registry per the resolved definition's policy.
            api_client: None,
            tool_invoker: None,
            tool_schemas: vec![],
            // Overwritten by `spawn` from `request.schema` (like `tool_schemas`).
            schema: None,
            budget: None,
            // Filled by `spawn` from the set-once `hook_executor` / `skill_loader`
            // cells (None when unfilled — tests / minimal builds). `hook_session_id`
            // / `hook_cwd` carry the boot-set values.
            hook_executor: None,
            skill_loader: None,
            hook_session_id: protocol::SessionId::nil(),
            hook_cwd: std::path::PathBuf::new(),
            // Default 0; `build_subagent_context` overwrites it with `request.depth`.
            depth: 0,
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

    /// Build the child [`SubagentContext`] for a spawn: definition resolution +
    /// caller model override + inheritance (tool invoker / budget / api seam) +
    /// hook cells + transcript dir + per-spawn tool resolution. Shared by the
    /// one-shot [`SubagentSpawner::spawn`] and the resumable
    /// [`StreamingSubagentSpawner::spawn_persistent`].
    ///
    /// `persistent = true` makes the runner "come to rest" after each terminal
    /// turn-set — it parks awaiting the next inbound `UserMessage` (delivered via
    /// [`StateMachinePool::send_event`]) instead of returning — and marks it
    /// async (background-scheduled). This is the basis of the resumable
    /// background local_agent (claude-code `run_in_background` + comes-to-rest).
    async fn build_subagent_context(
        &self,
        request: &SubagentSpawnRequest,
        inherit: SubagentInheritance,
        persistent: bool,
    ) -> SubagentContext {
        let mut def = self.resolve_definition(&request.subagent_type).await;
        // Per-spawn system-prompt override (workflow xBp / DBp): replace the
        // resolved definition's body with the caller's override BEFORE the Notes
        // trailer is appended by `make_subagent_context`.
        if let Some(override_prompt) = &request.system_prompt_override {
            def.system_prompt = Some(override_prompt.clone());
        }
        // Per-spawn disallowed-tools union (workflow §6): augment the resolved
        // definition's deny list with the caller's additional names. Dedup so a
        // builtin that already has "SendUserMessage" doesn't double-list it.
        if !request.additional_disallowed_tools.is_empty() {
            for name in &request.additional_disallowed_tools {
                if !def.disallowed_tools.contains(name) {
                    def.disallowed_tools.push(name.clone());
                }
            }
        }
        // AgentTool spawn-surface parity: an explicit `model` from the caller
        // (TS schema `model: 'sonnet' | 'opus' | 'haiku'`) takes precedence over
        // the definition's model frontmatter (AgentTool.tsx:86).
        if let Some(model_pref) = request.model.as_deref() {
            // Dual-LLM dual-PROVIDER routing: when the caller pinned a provider
            // profile (`model_profile`), `request.model` is ALREADY the concrete
            // provider-local wire model (the candidate's resolved `request_model`,
            // e.g. `gpt-4o` / `gemini-1.5-pro`). The family-alias logic in
            // `resolve_agent_model` (alias→parent-tier matching, parent region
            // prefix) is Claude-shaped and would mangle a foreign concrete id, so
            // it is BYPASSED here: the model is used verbatim as `Explicit`. The
            // `profile` (set just below from `request.model_profile`) selects the
            // provider in `messages_create_*_in`.
            if request.model_profile.is_some() {
                def.model = AgentModel::Explicit(model_pref.to_string());
            } else {
                let requested = AgentModel::Alias(model_pref.to_string());
                def.model = match &self.default_model {
                    Some(parent) => AgentModel::Explicit(
                        crate::model_resolution::resolve_agent_model(
                            &requested,
                            parent,
                            self.permission_mode,
                            self.model_setting.as_deref(),
                        ),
                    ),
                    None => requested,
                };
            }
        }
        // Per-spawn effort override (claude-code workflow `agent({effort})` →
        // `me={...ie,effort:ae}`): a level/integer opt overrides the resolved
        // definition's effort frontmatter. Ignored when unparseable.
        if let Some(effort) = &request.effort {
            if let Some(parsed) = crate::definition::AgentEffort::from_json(effort) {
                def.effort = Some(parsed);
            }
        }
        // Fork carriers (codex #5): on the fork path `fork_context_messages`
        // carries the byte-exact forked prefix and `fork_parent_system_prompt`
        // the parent's rendered system prompt; both `None` for a normal spawn.
        let mut ctx = Self::make_subagent_context(
            def,
            &request.prompt,
            request.fork_context_messages.clone(),
            request.fork_parent_system_prompt.clone(),
        );
        // Append the subagent `<env>` block (claude-code 2.1.186 `tIm`, after the
        // `Notes:` trailer) on the NON-fork path only — the fork path replays the
        // parent's rendered prompt verbatim with no `enhanceSystemPromptWithEnvDetails`.
        // Rendered with THIS spawn's resolved model id so a model-override agent's
        // env line matches the model it actually runs as. Unfilled cell ⇒ no-op.
        let is_fork_spawn = request.fork_parent_system_prompt.is_some()
            || request.fork_context_messages.is_some();
        if !is_fork_spawn {
            if let (Some(render), Some(body)) = (
                self.subagent_env_renderer.get(),
                ctx.rendered_system_prompt.as_ref(),
            ) {
                let model_id = crate::runner::resolve_model(&ctx);
                // Per-agent cwd (worktree isolation / explicit `cwd`) → the env
                // block's `Working directory` + the "git worktree" notice, so the
                // isolated agent forms absolute paths under it.
                let cwd_override = request.cwd.as_deref().map(std::path::Path::new);
                let env = render(&model_id, cwd_override);
                ctx.rendered_system_prompt = Some(Arc::from(format!("{body}\n\n{env}")));
            }
        }
        // Per-spawn system-prompt addendum (workflow HBp/IBp NOTE): appended
        // AFTER the Notes trailer + env block so it is the final content the model
        // sees. Used when the caller specifies an explicit agentType in a workflow
        // agent() call.
        if let Some(addendum) = &request.system_prompt_addendum {
            if let Some(body) = ctx.rendered_system_prompt.as_ref() {
                ctx.rendered_system_prompt = Some(Arc::from(format!("{body}{addendum}")));
            }
        }
        // Hand the child the parent's tool invoker + budget enforcer + our model
        // API seam (recursion-lock / budget-inheritance invariants).
        ctx.tool_invoker = Some(inherit.tool_invoker);
        ctx.budget = Some(inherit.budget);
        ctx.api_client.clone_from(&self.api_client);
        // Per-spawn provider routing (dual-LLM dual-PROVIDER): the runner passes
        // this as the `profile` arg of the api client's `messages_create_*_in`
        // methods so the round-trip targets the candidate's resolved provider.
        ctx.model_profile = request.model_profile.clone();
        // G4/G5: thread the runner's hook executor + skill loader + hook context
        // seed from the set-once cells (None ⇒ runner skips those steps).
        ctx.hook_executor = self.hook_executor.get().cloned();
        ctx.skill_loader = self.skill_loader.get().cloned();
        ctx.hook_session_id = self.hook_session_id;
        ctx.hook_cwd = self.hook_cwd.clone();
        // Seed the child's REAL transcript_subdir when the host wired one.
        if let Some(subagents_dir) = &self.hook_subagents_dir {
            ctx.transcript_subdir = subagents_dir.clone();
        }
        // Resolve THIS spawn's advertised tools + dispatch allow-list.
        // This child's recursion depth (claude `spawnDepth`): the Agent tool
        // stamped it as parent.depth + 1. Drives the resolver's `Agent` depth-gate
        // and is threaded by the runner into the child's dispatched tools.
        ctx.depth = request.depth;
        let (tool_schemas, allowed_tools) =
            self.resolve_tools(&ctx.agent_definition, request.depth).await;
        ctx.tool_schemas = tool_schemas;
        ctx.allowed_tools = allowed_tools;
        ctx.schema = request.schema.clone();
        // Per-agent working directory (claude-code `me = cwd ?? worktreePath`):
        // the AgentTool resolves `isolation:"worktree"` to a freshly-created
        // worktree path (or honours an explicit `cwd`) and threads it via
        // `request.cwd`. Set it on the context so the runner threads it into every
        // dispatched tool's `cwd`. `None` ⇒ the shared session workspace (legacy).
        ctx.cwd = request.cwd.as_ref().map(std::path::PathBuf::from);
        // A persistent (background/resumable) agent parks after each turn-set;
        // `is_async` marks background scheduling (vs the foreground one-shot).
        ctx.persistent = persistent;
        ctx.is_async = persistent;
        ctx
    }
}

/// The persistent / resumable subagent seam (claude-code `run_in_background` +
/// "comes to rest" + `resumeAgentBackground`).
///
/// Distinct from the cross-crate [`traits::SubagentSpawner`] (whose return type
/// is the traits-level [`SubagentResult`] — it cannot reference the `agent`-crate
/// [`SubagentEvent`] stream). The task-layer LocalAgent handler — which already
/// depends on `agent` — drives a persistent (background/resumable) local_agent
/// through this trait: it pumps the [`SubagentEvent`] stream (one `Completed`
/// per turn-set, then the runner parks awaiting the next message) and resumes a
/// resting agent via [`Self::resume`].
#[async_trait]
pub trait StreamingSubagentSpawner: Send + Sync {
    /// Spawn a PERSISTENT subagent (`persistent: true`): the runner "comes to
    /// rest" after each terminal turn-set instead of returning. Returns its id
    /// plus the outbound [`SubagentEvent`] stream the caller pumps.
    async fn spawn_persistent(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<(AgentId, tokio::sync::mpsc::Receiver<SubagentEvent>), SubagentSpawnError>;

    /// Resume a resting persistent subagent by delivering a user `message` (the
    /// `injectUserMessageToTeammate` analogue): the parked runner wakes, appends
    /// it to history, and runs the next turn-set. Errors when the agent id is
    /// unknown / its runner has terminated.
    async fn resume(&self, agent_id: &AgentId, message: String)
        -> Result<(), SubagentSpawnError>;
}

#[async_trait]
impl StreamingSubagentSpawner for PoolSubagentSpawner {
    async fn spawn_persistent(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<(AgentId, tokio::sync::mpsc::Receiver<SubagentEvent>), SubagentSpawnError> {
        let ctx = self.build_subagent_context(&request, inherit, true).await;
        let agent_id = ctx.agent_id;
        let (_aid, rx) = self
            .pool
            .allocate(ctx)
            .await
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))?;
        Ok((agent_id, rx))
    }

    async fn resume(
        &self,
        agent_id: &AgentId,
        message: String,
    ) -> Result<(), SubagentSpawnError> {
        self.pool
            .send_event(
                agent_id,
                engine::Event::UserMessage {
                    message_id: protocol::MessageId::new(),
                    request_id: protocol::RequestId::new(),
                    content: message,
                },
            )
            .await
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))
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

/// Cancel-safety guard for [`PoolSubagentSpawner::spawn`]. The runner runs as a
/// DETACHED pool task (`allocate` spawns it via the `RuntimeSpawner`); the
/// `spawn` future only pumps events and calls `deallocate` on the terminal
/// event. If a caller races `spawn` against a `CancellationToken` and DROPS the
/// future before that terminal event (timeout / cancel), `deallocate` would
/// never run and the detached runner would keep executing tools + leak its
/// slot. This guard deallocates (which `runtime.cancel`s the task) on early
/// drop; it is disarmed on the normal terminal path.
struct SpawnDeallocGuard {
    pool: Arc<StateMachinePool>,
    agent_id: AgentId,
    armed: bool,
}

impl Drop for SpawnDeallocGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // `deallocate` is async; hand it to the current runtime best-effort. If
        // no runtime is active (shutdown) there is nothing left to clean up.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let pool = self.pool.clone();
            let id = self.agent_id;
            handle.spawn(async move {
                let _ = pool.deallocate(&id).await;
            });
        }
    }
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
        // Build the child context (non-persistent: the one-shot `spawn` returns
        // on the first terminal stop). The persistent/resumable variant is
        // `spawn_persistent` below.
        let ctx = self.build_subagent_context(&request, inherit, false).await;
        let agent_id = ctx.agent_id;
        let (_aid, mut rx) = self
            .pool
            .allocate(ctx)
            .await
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))?;

        // Cancel-safety: deallocate the detached runner if THIS future is
        // dropped before reaching a terminal event (disarmed on the normal
        // path below). Without this a cancelled/timed-out spawn orphans the
        // runner and leaks its pool slot.
        let mut dealloc_guard = SpawnDeallocGuard {
            pool: self.pool.clone(),
            agent_id,
            armed: true,
        };

        // Pump the slot until terminal. The runner emits Progress/Message
        // events as it streams turns; we ignore those here and surface only
        // the terminal Completed/Failed/Killed.
        let result = loop {
            match rx.recv().await {
                Some(SubagentEvent::Completed {
                    agent_id: child_id,
                    result,
                    usage,
                    total_tool_use_count,
                    total_duration_ms,
                    assistant_message_count,
                    last_request_id,
                }) => {
                    // Translate the wire usage into the trait rollup. claude
                    // `getTokenCountFromUsage` = input + cache_creation + cache_read
                    // + output of the FINAL turn's usage (tokens.ts:46-54); the
                    // runner already carries that final usage (no cross-turn sum).
                    let bt = usage.billable_tokens;
                    let total_tokens = bt
                        .input
                        .saturating_add(bt.cache_write)
                        .saturating_add(bt.cache_read)
                        .saturating_add(bt.output);
                    // claude `response_char_count: content.length`
                    // (agentToolUtils.ts:328) — despite the name, this is the
                    // NUMBER of text BLOCKS in the final response (`content` is the
                    // `[{type:'text', text}]` array; `.length` is its element
                    // count), NOT a summed character count. Count the text blocks
                    // from the runner's `content` array to match byte-for-byte.
                    let response_char_count = result
                        .get("content")
                        .and_then(serde_json::Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter(|b| {
                                    b.get("type").and_then(serde_json::Value::as_str)
                                        == Some("text")
                                })
                                .count() as u64
                        })
                        .unwrap_or(0);
                    break SubagentResult::Completed {
                        agent_id: child_id,
                        content: result,
                        usage: SubagentUsage {
                            total_tokens,
                            input_tokens: bt.input,
                            output_tokens: bt.output,
                            cache_creation_input_tokens: bt.cache_write,
                            cache_read_input_tokens: bt.cache_read,
                        },
                        total_tool_use_count,
                        total_duration_ms,
                        total_tokens,
                        assistant_message_count,
                        response_char_count,
                        last_request_id,
                    };
                }
                Some(SubagentEvent::Failed {
                    agent_id: child_id,
                    error,
                }) => {
                    break SubagentResult::Failed {
                        agent_id: child_id,
                        reason: error,
                    };
                }
                Some(SubagentEvent::Killed {
                    agent_id: child_id,
                }) => {
                    break SubagentResult::Killed {
                        agent_id: child_id,
                    };
                }
                Some(_) => continue,
                None => {
                    // No terminal event ever arrived; fall back to the bound
                    // ctx agent_id (still the REAL child id, never a fresh one).
                    break SubagentResult::Failed {
                        agent_id,
                        reason: "subagent channel closed unexpectedly".into(),
                    };
                }
            }
        };

        // Normal terminal path: deallocate explicitly and disarm the guard so
        // it does not double-deallocate on drop.
        dealloc_guard.armed = false;
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

    /// Resolve the `required_mcp_servers` declared by `subagent_type`'s
    /// definition (claude-code `AgentDefinition.requiredMcpServers`) so
    /// `AgentTool` can run the pre-spawn MCP-servers gate. Resolves the same
    /// definition `spawn` would (file catalog overrides built-ins; unknown →
    /// general-purpose) and returns its `required_mcp_servers` (built-ins
    /// declare none → empty → gate skipped). Skips the model resolution
    /// `resolve_definition` does — only the MCP-requirements field is needed.
    async fn resolve_required_mcp_servers(&self, subagent_type: &str) -> Vec<String> {
        self.lookup_definition(subagent_type)
            .await
            .required_mcp_servers
    }

    /// G11: surface the pre-spawn selection metadata for the
    /// `tengu_agent_tool_selected` event (claude `AgentTool.tsx:419-428`):
    /// resolve the definition, map its [`AgentSource`] to claude's source string,
    /// resolve the concrete model (honoring the caller's optional `model`
    /// family override), pull the `color`, and flag `is_built_in`.
    async fn resolve_selection(
        &self,
        subagent_type: &str,
        model: Option<&str>,
    ) -> traits::subagent_spawn::SelectedAgentMeta {
        let def = self.lookup_definition(subagent_type).await;
        // claude `getAgentModel(selectedAgent.model, mainLoopModel, model,
        // permissionMode)` (AgentTool.tsx:418): the caller's `model` override
        // takes precedence over the definition's model frontmatter. Resolve to a
        // concrete id when a parent/main-loop model is wired; without one the
        // resolved id is left empty (no default to anchor against).
        let resolved_model = match &self.default_model {
            Some(parent) => {
                let pref = match model {
                    Some(m) => AgentModel::Alias(m.to_string()),
                    // 2.1.198 `GAe`: same session-model derivation for the
                    // built-in Explore definition as the spawn path, so the
                    // `tengu_agent_tool_selected` metadata reports the model
                    // the spawn will actually use.
                    None => crate::model_resolution::resolve_builtin_explore_model(
                        &def,
                        parent,
                        self.session_provider_first_party,
                    ),
                };
                crate::model_resolution::resolve_agent_model(
                    &pref,
                    parent,
                    self.permission_mode,
                    self.model_setting.as_deref(),
                )
            }
            None => String::new(),
        };
        traits::subagent_spawn::SelectedAgentMeta {
            agent_type: def.agent_type.clone(),
            resolved_model,
            source: agent_source_to_claude_str(def.source).to_string(),
            color: def.color.clone(),
            is_built_in: matches!(def.source, AgentSource::BuiltIn),
            // claude `selectedAgent.background` (AgentTool.tsx:426): the
            // definition's `background` frontmatter flag, folded into `is_async`.
            background: def.background,
        }
    }

    /// G14: register `name → child agent-id` for `SendMessage` routing of a
    /// spawned ASYNC subagent (claude `agentNameRegistry.set`, AgentTool.tsx:706).
    async fn register_name(&self, name: &str, agent_id: AgentId) {
        self.name_registry
            .write()
            .await
            .insert(name.to_string(), agent_id);
    }

    /// G14: resolve a previously-registered async-agent name to its child id.
    async fn resolve_name(&self, name: &str) -> Option<AgentId> {
        self.name_registry.read().await.get(name).copied()
    }
}

/// Map a LingXi [`AgentSource`] to claude-code's `selectedAgent.source` literal
/// (`SettingSource` ∪ `'built-in'` / `'plugin'`, loadAgentsDir.ts:137/156 +
/// settings/constants.ts:7-21). Used by [`PoolSubagentSpawner::resolve_selection`]
/// to emit `tengu_agent_tool_selected`'s `source` field byte-faithfully.
fn agent_source_to_claude_str(source: AgentSource) -> &'static str {
    match source {
        AgentSource::BuiltIn => "built-in",
        AgentSource::Plugin => "plugin",
        AgentSource::UserDefined => "userSettings",
        AgentSource::Project => "projectSettings",
        AgentSource::PolicySettings => "policySettings",
        AgentSource::Flag => "flagSettings",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
    use test_harness::mocks::MockRuntimeSpawner;
    use tool_api::tool_trait::PromptOptions;
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
            disallowed_tools: vec![],
            skills: vec![],
            required_mcp_servers: vec![],
            background: false,
            isolation: None,
            memory: None,
            effort: None,
            initial_prompt: None,
            color: None,
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
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    use_exact_tools: true,
                }),
                0,
            )
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
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    use_exact_tools: true,
                }),
                0,
            )
            .await;
        // Full set, and allow-list = resolved names — both in the faithful
        // `assembleToolPool` order (builtins sorted by name).
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Bash", "Read"]);
        // allow-list mirrors the resolved order, which now follows
        // `available_tools()`'s locale-sorted order (Bash < Read).
        assert_eq!(allowed, vec!["Bash".to_string(), "Read".to_string()]);
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
            .resolve_tools(&agent_def(AgentToolPolicy::Explicit(vec!["Read".to_string()])), 0)
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Read"]);
        assert_eq!(allowed, vec!["Read".to_string()]);
    }

    #[tokio::test]
    async fn resolve_tools_strips_tool_wide_denied_tool_from_subagent_pool() {
        // FIX 1 (subagent pool): a tool-wide deny rule fed via the set-once cell
        // strips the tool from the child's advertised pool AND its dispatch
        // allow-list (claude-code `assembleToolPool` → `filterToolsByDenyRules`).
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Read", "Bash", "WebFetch"]))
            .with_tool_wide_deny_names(vec!["WebFetch".to_string()]);

        let (schemas, allowed) = spawner
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    use_exact_tools: true,
                }),
                0,
            )
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Bash", "Read"], "WebFetch denied → not advertised");
        assert!(
            !allowed.contains(&"WebFetch".to_string()),
            "denied tool must not be in the dispatch allow-list either"
        );
    }

    #[tokio::test]
    async fn resolve_tools_mcp_server_deny_strips_all_server_tools_from_subagent() {
        // A tool-wide `mcp__github` deny strips every `mcp__github__*` from the
        // child pool (MCP server-prefix blanket strip) but keeps other servers.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&[
                "Read",
                "mcp__github__issue",
                "mcp__slack__post",
            ]))
            .with_tool_wide_deny_names(vec!["mcp__github".to_string()]);

        let (schemas, _allowed) = spawner
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    use_exact_tools: true,
                }),
                0,
            )
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(
            !names.contains(&"mcp__github__issue"),
            "mcp__github deny must strip mcp__github__issue, got: {names:?}"
        );
        assert!(
            names.contains(&"mcp__slack__post") && names.contains(&"Read"),
            "other server + builtins survive, got: {names:?}"
        );
    }

    #[tokio::test]
    async fn resolve_tools_empty_deny_leaves_subagent_pool_unchanged() {
        // Regression safety: an unset / empty deny-names cell must leave the
        // child pool byte-identical to before (no filtering).
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let unfiltered = PoolSubagentSpawner::new(pool.clone())
            .with_tool_registry(registry_with(&["Read", "Bash"]));
        let (schemas_a, allowed_a) = unfiltered
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    use_exact_tools: true,
                }),
                0,
            )
            .await;
        let empty_deny = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Read", "Bash"]))
            .with_tool_wide_deny_names(vec![]);
        let (schemas_b, allowed_b) = empty_deny
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    use_exact_tools: true,
                }),
                0,
            )
            .await;
        assert_eq!(schemas_a, schemas_b, "empty deny → identical schemas");
        assert_eq!(allowed_a, allowed_b, "empty deny → identical allow-list");
    }

    #[tokio::test]
    async fn resolve_tools_includes_aliases_in_allow_list() {
        // The dispatch allow-list must accept every name the inherited invoker's
        // `find_by_name` accepts — including aliases — or a `tool_use` for a
        // legacy alias would be wrongly refused by the runner guard. Advertised
        // schemas stay canonical-name-only. Re-based on a benign tool (Bash /
        // alias Shell) because "Agent" is now stripped by the always-disallowed
        // default drop (see `resolve_tools_strips_agent_by_default`).
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(StubTool {
            name: "Bash",
            aliases: &["Shell"],
        }));
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool).with_tool_registry(Arc::new(reg));

        let (schemas, allowed) = spawner
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    use_exact_tools: true,
                }),
                0,
            )
            .await;
        // Advertised: canonical name only.
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Bash"]);
        // Allow-list: canonical name AND the alias.
        assert_eq!(allowed, vec!["Bash".to_string(), "Shell".to_string()]);
    }

    #[tokio::test]
    async fn resolve_tools_gates_agent_by_depth() {
        // `Agent` is depth-gated (claude `s < e9t`, e9t=5), not flat-denied: a
        // depth-0 subagent keeps it; a depth-5 subagent has it stripped. With
        // ONLY an Agent tool registered, the depth-5 pool is empty.
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(StubTool {
            name: "Agent",
            aliases: &["Task"],
        }));
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool).with_tool_registry(Arc::new(reg));

        let policy = || {
            agent_def(AgentToolPolicy::All {
                // use_exact_tools: false → the resolver (incl. the depth gate)
                // applies. (The `true` / fork path BYPASSES it — see the
                // tool_resolver `use_exact_tools_*` tests.)
                use_exact_tools: false,
            })
        };
        // depth 0: Agent kept (0 < 5).
        let (schemas0, allowed0) = spawner.resolve_tools(&policy(), 0).await;
        let names0: Vec<&str> = schemas0.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names0, vec!["Agent"], "Agent kept at depth 0");
        assert!(allowed0.contains(&"Agent".to_string()));
        assert!(allowed0.contains(&"Task".to_string()), "alias in allow-list");
        // depth 5: Agent gated → empty pool.
        let (schemas5, allowed5) = spawner.resolve_tools(&policy(), 5).await;
        assert!(schemas5.is_empty(), "Agent gated at depth 5 → no schemas");
        assert!(allowed5.is_empty(), "Agent (and alias Task) gated → empty allow-list");
    }

    #[tokio::test]
    async fn resolve_tools_all_policy_keeps_agent_at_depth_0() {
        // End-to-end: resolve_tools → tool_schemas + allowed_tools. At depth 0
        // Agent is KEPT (depth-gated, not flat-denied); Bash+Read survive too,
        // in assembleToolPool/localeCompare-sorted order.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_tool_registry(registry_with(&["Agent", "Bash", "Read"]));

        let (schemas, allowed) = spawner
            .resolve_tools(
                &agent_def(AgentToolPolicy::All {
                    // use_exact_tools: false → the always-disallowed strip applies
                    // (the fork/`true` path keeps Agent — tool_resolver bypass test).
                    use_exact_tools: false,
                }),
                0,
            )
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Agent", "Bash", "Read"]);
        assert!(allowed.contains(&"Agent".to_string()));
        assert!(allowed.contains(&"Bash".to_string()));
        assert!(allowed.contains(&"Read".to_string()));
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
            .resolve_tools(
                &agent_def_plan(AgentToolPolicy::All {
                    // use_exact_tools: false → Plan-mode narrowing applies (the
                    // fork/`true` path bypasses it — tool_resolver bypass test).
                    use_exact_tools: false,
                }),
                0,
            )
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        // available_tools() locale-sorts the builtin set (Grep < Read < WebFetch).
        assert_eq!(names, vec!["Grep", "Read", "WebFetch"]);
        assert!(!allowed.contains(&"Bash".to_string()));
        // Resolved/allow-list order follows `available_tools()` locale sort.
        assert_eq!(
            allowed,
            vec![
                "Grep".to_string(),
                "Read".to_string(),
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
            .resolve_tools(&agent_def(AgentToolPolicy::Except(vec!["Bash".to_string()])), 0)
            .await;
        let names: Vec<&str> = schemas.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Edit", "Read"]); // sorted by name
        // resolved/allow-list order = available_tools() locale sort (Edit < Read)
        assert_eq!(allowed, vec!["Edit".to_string(), "Read".to_string()]);
    }

    // ── batch 21: real AgentDefinition resolution + prompt placement ──

    #[tokio::test]
    async fn resolve_definition_returns_builtin_for_known_type() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        // Explore is a read-only built-in: Except the write tools, model
        // `inherit` (2.1.198 `qme` frontmatter; the session cap is applied by
        // GAe on the resolved path), a real system prompt, and the high
        // built-in turn cap (not the old 1).
        let def = spawner.resolve_definition("Explore").await;
        assert_eq!(def.agent_type, "Explore");
        assert!(matches!(def.tools, AgentToolPolicy::Except(_)));
        assert!(matches!(&def.model, AgentModel::Inherit));
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
        // claude-code-guide is Alias("haiku"); parent is opus (different tier)
        // → resolves to haiku's concrete default id, NOT the parent.
        let spawner = PoolSubagentSpawner::new(pool).with_default_model("claude-opus-4-7");
        let def = spawner.resolve_definition("claude-code-guide").await;
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-haiku-4-5"));
    }

    // ── 2.1.198 GAe: built-in Explore inherits the session model capped at opus ──

    #[tokio::test]
    async fn resolve_definition_explore_inherits_claude_family_session_model() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // A haiku/sonnet/opus-named session model → GAe "inherit" → the parent
        // model verbatim (NOT the old haiku alias resolution).
        let spawner = PoolSubagentSpawner::new(pool).with_default_model("claude-opus-4-7");
        let def = spawner.resolve_definition("Explore").await;
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-opus-4-7"));
    }

    #[tokio::test]
    async fn resolve_definition_explore_caps_fable_class_session_at_opus() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // A fable/mythos-class session model (names none of haiku/sonnet/opus)
        // on firstParty → GAe "opus" → the opus family default id.
        let spawner = PoolSubagentSpawner::new(pool).with_default_model("claude-fable-5");
        let def = spawner.resolve_definition("Explore").await;
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-opus-4-8"));
    }

    #[tokio::test]
    async fn resolve_definition_explore_on_non_anthropic_profile_inherits() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // LingXi multi-provider: the composition root passes first_party=false
        // when the session routes to a non-Anthropic profile → GAe behaves
        // like the TS non-firstParty branch → inherit the session model (the
        // opus cap NEVER fires for a foreign provider).
        let spawner = PoolSubagentSpawner::new(pool)
            .with_default_model("gpt-4o")
            .with_session_provider_first_party(false);
        let def = spawner.resolve_definition("Explore").await;
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "gpt-4o"));
    }

    #[tokio::test]
    async fn resolve_definition_user_defined_explore_keeps_its_own_model() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // A user/project agent literally named "Explore" (source != built-in)
        // is untouched by GAe: its own model frontmatter resolves normally.
        let base = agent_def(AgentToolPolicy::Explicit(vec!["Read".to_string()]));
        let custom = AgentDefinition {
            agent_type: "Explore".to_string(),
            model: AgentModel::Alias("haiku".to_string()),
            source: AgentSource::UserDefined,
            ..base
        };
        let catalog = Arc::new(RwLock::new(vec![custom]));
        let spawner = PoolSubagentSpawner::new(pool)
            .with_agent_catalog(catalog)
            .with_default_model("claude-fable-5");
        let def = spawner.resolve_definition("Explore").await;
        // haiku alias, parent fable (no tier match) → the haiku default id —
        // NOT the opus cap.
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-haiku-4-5"));
    }

    #[tokio::test]
    async fn resolve_definition_without_default_model_leaves_model_raw() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // No default model wired (legacy/tests): the alias is NOT resolved — the
        // runner's resolve_model then emits it raw (back-compat).
        let spawner = PoolSubagentSpawner::new(pool);
        let def = spawner.resolve_definition("claude-code-guide").await;
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
        let ctx = PoolSubagentSpawner::make_subagent_context(def, "do the task", None, None);
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
        let ctx = PoolSubagentSpawner::make_subagent_context(def, "task", None, None);
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
        // All five byte-locked bullets, including the em-dash (U+2014) in
        // bullets 2 and 5 surviving byte-for-byte.
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
        // Bullet 5 is SUBAGENT-specific (the main FOOTER omits it — a main agent
        // has no parent): never write report/summary .md files; return findings
        // in the final assistant message.
        assert!(sys.contains(
            "- Do NOT Write report/summary/findings/analysis .md files. Return findings directly as your final assistant message — the parent agent reads your text output, not files you create. (Files written as input to another tool are fine; this note is about report files.)"
        ));
        // No trailing newline — the `notes` element is newline-free in TS
        // (the next block, `<env>`, is joined with a blank line, not appended
        // to the notes literal).
        assert!(sys.ends_with("this note is about report files.)"));
    }

    #[test]
    fn make_subagent_context_none_system_prompt_yields_no_system() {
        let def = AgentDefinition {
            system_prompt: None,
            ..agent_def(AgentToolPolicy::All {
                use_exact_tools: false,
            })
        };
        let ctx = PoolSubagentSpawner::make_subagent_context(def, "task", None, None);
        assert!(ctx.rendered_system_prompt.is_none());
        assert_eq!(ctx.prompt_messages[0].text_content(), "task");
    }

    // ── codex #5: fork-subagent resolution + context ──

    #[tokio::test]
    async fn lookup_definition_resolves_fork_synthetic_agent() {
        // subagent_type "fork" resolves to the synthetic FORK_AGENT, NOT a
        // catalog/general-purpose lookup — even when a catalog agent is named
        // "fork" (the synthetic one wins unconditionally).
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let shadow = AgentDefinition {
            agent_type: "fork".to_string(),
            when_to_use: "user shadow".to_string(),
            ..agent_def(AgentToolPolicy::Explicit(vec!["Read".to_string()]))
        };
        let catalog = Arc::new(RwLock::new(vec![shadow]));
        let spawner = PoolSubagentSpawner::new(pool).with_agent_catalog(catalog);
        let def = spawner.lookup_definition("fork").await;
        assert_eq!(def.agent_type, "fork");
        // Synthetic, not the catalog shadow.
        assert!(matches!(
            def.tools,
            AgentToolPolicy::All { use_exact_tools: true }
        ));
        assert_eq!(def.max_turns, 200);
        assert!(matches!(def.permission_mode, AgentPermissionMode::Bubble));
    }

    #[test]
    fn make_subagent_context_fork_parent_prompt_skips_notes_trailer() {
        // fork_parent_system_prompt → rendered_system_prompt is the parent's
        // bytes VERBATIM, with NO `Notes:` trailer (re-appending busts cache).
        let def = crate::builtins::fork_agent_definition();
        let parent_prompt = "PARENT SYSTEM PROMPT BYTES\n\n<env>cwd: /x</env>".to_string();
        let ctx = PoolSubagentSpawner::make_subagent_context(
            def,
            "unused directive",
            Some(vec![ConversationMessage::user(
                MessageId::new(),
                "prefix".to_string(),
            )]),
            Some(parent_prompt.clone()),
        );
        let sys = ctx.rendered_system_prompt.as_deref().unwrap();
        assert_eq!(sys, parent_prompt);
        assert!(!sys.contains("Notes:"), "fork must NOT append the Notes trailer");
    }

    #[test]
    fn make_subagent_context_fork_seeds_prefix_and_empty_prompt_messages() {
        // fork_context_messages → ctx.fork_context_messages, prompt_messages = [].
        let def = crate::builtins::fork_agent_definition();
        let prefix = vec![
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![protocol::ContentBlock::Text {
                    text: "assistant turn".to_string(),
                }],
                stop_reason: Some("tool_use".to_string()),
            },
            ConversationMessage::user(MessageId::new(), "directive prefix".to_string()),
        ];
        let ctx = PoolSubagentSpawner::make_subagent_context(
            def,
            "unused",
            Some(prefix.clone()),
            Some("parent sys".to_string()),
        );
        assert!(ctx.prompt_messages.is_empty(), "fork seeds empty prompt_messages");
        let fc = ctx.fork_context_messages.expect("fork_context_messages set");
        assert_eq!(fc.len(), 2);
        // runner replays fork_context_messages ++ prompt_messages = the prefix.
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
        let (schemas, allowed) = spawner.resolve_tools(&def, 0).await;
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
        // All 7 built-ins, sorted by type.
        assert_eq!(entries.len(), 7);
        let by: std::collections::HashMap<&str, &SubagentListingEntry> =
            entries.iter().map(|e| (e.agent_type.as_str(), e)).collect();
        // general-purpose: All { .. } → "All tools".
        assert_eq!(by["general-purpose"].tools_description, "All tools");
        // Explore: Except([Agent, Artifact, ExitPlanMode, Edit, Write, NotebookEdit]).
        assert_eq!(
            by["Explore"].tools_description,
            "All tools except Agent, Artifact, ExitPlanMode, Edit, Write, NotebookEdit"
        );
        // statusline-setup: Explicit([Read, Edit]).
        assert_eq!(by["statusline-setup"].tools_description, "Read, Edit");
        // when_to_use carried through verbatim (claude 2.1.193 lean variant N6p).
        assert!(by["Explore"].when_to_use.contains("broad fan-out searches"));
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
        // Still 7 (override, not addition).
        assert_eq!(entries.len(), 7);
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
            model_profile: None,
            run_in_background: false,
            name: None,
            team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: None,
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            depth: 0,
        };
        // Drive resolve_definition + the override branch directly by replicating
        // the spawn-path logic (spawn() would require a live runner).
        let mut def = spawner.resolve_definition(&req.subagent_type).await;
        if let Some(model_pref) = req.model.as_deref() {
            let requested = AgentModel::Alias(model_pref.to_string());
            def.model = AgentModel::Explicit(crate::model_resolution::resolve_agent_model(
                &requested,
                "claude-opus-4-7",
                permission::PermissionMode::Default,
                None,
            ));
        }
        assert!(matches!(&def.model, AgentModel::Explicit(m) if m == "claude-haiku-4-5"));
        // Sanity: the request struct carries the rest of the parity params.
        req.name = Some("scout".into());
        assert_eq!(req.name.as_deref(), Some("scout"));
    }

    #[tokio::test]
    async fn resolve_required_mcp_servers_builtins_are_empty() {
        // Built-ins declare no required MCP servers → the spawner surfaces an
        // empty list (gate skipped). G3/C2.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        assert!(spawner
            .resolve_required_mcp_servers("general-purpose")
            .await
            .is_empty());
        // Unknown → general-purpose fallback → also empty.
        assert!(spawner
            .resolve_required_mcp_servers("no-such-agent")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn resolve_required_mcp_servers_reads_catalog_definition() {
        // A catalog agent that DECLARES required_mcp_servers surfaces them.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let custom = AgentDefinition {
            agent_type: "needs-github".to_string(),
            required_mcp_servers: vec!["github".to_string()],
            ..agent_def(AgentToolPolicy::All {
                use_exact_tools: false,
            })
        };
        let catalog = Arc::new(RwLock::new(vec![custom]));
        let spawner = PoolSubagentSpawner::new(pool).with_agent_catalog(catalog);
        assert_eq!(
            spawner.resolve_required_mcp_servers("needs-github").await,
            vec!["github".to_string()]
        );
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

    /// local_agent "resume" Phase 1: `build_subagent_context(persistent=true)`
    /// sets `SubagentContext.persistent` + `is_async`, so the runner "comes to
    /// rest" (parks awaiting the next inbound message) after each turn-set
    /// instead of returning — the basis of the resumable background local_agent.
    /// `persistent=false` (the one-shot `spawn` path) keeps both `false`.
    #[tokio::test]
    async fn build_subagent_context_threads_persistent_flag() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        let req = SubagentSpawnRequest {
            subagent_type: "general-purpose".to_string(),
            prompt: "go".to_string(),
            context_paths: vec![],
            description: None,
            model: None,
            model_profile: None,
            run_in_background: true,
            name: None,
            team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: None,
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            depth: 0,
        };
        let mk_inherit = || SubagentInheritance {
            tool_invoker: Arc::new(DummyInvoker),
            budget: Arc::new(DummyBudget),
        };

        let persistent = spawner.build_subagent_context(&req, mk_inherit(), true).await;
        assert!(persistent.persistent, "persistent agent must park (come to rest)");
        assert!(persistent.is_async, "persistent agent is background-scheduled");

        let one_shot = spawner.build_subagent_context(&req, mk_inherit(), false).await;
        assert!(!one_shot.persistent, "the one-shot spawn path must NOT park");
        assert!(!one_shot.is_async);
    }

    /// 2.1.186: the subagent `<env>` block (`tIm`) is appended after the
    /// `Notes:` trailer on a NON-fork spawn, rendered with the spawn's RESOLVED
    /// model id. The fork path is byte-verbatim (no env block). An unfilled
    /// renderer cell is a no-op.
    #[tokio::test]
    async fn build_subagent_context_appends_env_block_nonfork_only() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        // A renderer that echoes the resolved model id into a sentinel block.
        let spawner = PoolSubagentSpawner::new(pool)
            .with_default_model("claude-opus-4-8[1m]")
            .with_subagent_env_renderer(Arc::new(|model_id: &str, cwd: Option<&std::path::Path>| {
                format!(
                    "<env>\nMODEL: {model_id}\nCWD: {}\n</env>",
                    cwd.map_or("<none>".to_string(), |p| p.display().to_string())
                )
            }));
        let mk_inherit = || SubagentInheritance {
            tool_invoker: Arc::new(DummyInvoker),
            budget: Arc::new(DummyBudget),
        };
        let mut req = SubagentSpawnRequest {
            subagent_type: "general-purpose".to_string(),
            prompt: "go".to_string(),
            context_paths: vec![],
            description: None,
            model: None,
            model_profile: None,
            run_in_background: false,
            name: None,
            team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: None,
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            depth: 0,
        };

        // Non-fork: env block appended after the body, joined by a blank line,
        // rendered with the resolved default model id.
        let ctx = spawner.build_subagent_context(&req, mk_inherit(), false).await;
        let sys = ctx.rendered_system_prompt.as_deref().unwrap();
        assert!(
            sys.ends_with("\n\n<env>\nMODEL: claude-opus-4-8[1m]\nCWD: <none>\n</env>"),
            "env block must be appended with the resolved model + no cwd override; got:\n{sys}"
        );
        // The Notes trailer still precedes it.
        assert!(sys.contains("this note is about report files.)\n\n<env>"));

        // A worktree-isolated spawn (request.cwd Some) threads the cwd into the
        // env renderer so the agent's env block reflects the worktree.
        let mut wt_req = req.clone();
        wt_req.cwd = Some("/repo/.lingxi/worktrees/agent-x".to_string());
        let wt_ctx = spawner.build_subagent_context(&wt_req, mk_inherit(), false).await;
        let wt_sys = wt_ctx.rendered_system_prompt.as_deref().unwrap();
        assert!(
            wt_sys.contains("CWD: /repo/.lingxi/worktrees/agent-x"),
            "worktree cwd must reach the env renderer; got:\n{wt_sys}"
        );
        assert_eq!(
            wt_ctx.cwd.as_deref(),
            Some(std::path::Path::new("/repo/.lingxi/worktrees/agent-x")),
            "SubagentContext.cwd is set from request.cwd"
        );

        // Fork path: the parent's rendered prompt is replayed verbatim — NO env.
        req.fork_parent_system_prompt = Some("PARENT VERBATIM".to_string());
        let fork_ctx = spawner.build_subagent_context(&req, mk_inherit(), false).await;
        assert_eq!(
            fork_ctx.rendered_system_prompt.as_deref(),
            Some("PARENT VERBATIM"),
            "fork path must not append the env block"
        );
    }

    // ── G11: resolve_selection source mapping + model resolution ──

    #[test]
    fn agent_source_to_claude_str_byte_locked() {
        // claude SettingSource literals + 'built-in'/'plugin' (loadAgentsDir.ts
        // + settings/constants.ts).
        assert_eq!(agent_source_to_claude_str(AgentSource::BuiltIn), "built-in");
        assert_eq!(agent_source_to_claude_str(AgentSource::Plugin), "plugin");
        assert_eq!(
            agent_source_to_claude_str(AgentSource::UserDefined),
            "userSettings"
        );
        assert_eq!(
            agent_source_to_claude_str(AgentSource::Project),
            "projectSettings"
        );
        assert_eq!(
            agent_source_to_claude_str(AgentSource::PolicySettings),
            "policySettings"
        );
        assert_eq!(agent_source_to_claude_str(AgentSource::Flag), "flagSettings");
    }

    #[tokio::test]
    async fn resolve_selection_builtin_is_built_in_and_source() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        let meta = spawner.resolve_selection("Explore", None).await;
        assert_eq!(meta.agent_type, "Explore");
        assert_eq!(meta.source, "built-in");
        assert!(meta.is_built_in);
    }

    #[tokio::test]
    async fn resolve_selection_catalog_project_source_mapped() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let mut def = agent_def(AgentToolPolicy::All {
            use_exact_tools: false,
        });
        def.agent_type = "proj-agent".into();
        def.source = AgentSource::Project;
        def.color = Some("green".into());
        let catalog = Arc::new(RwLock::new(vec![def]));
        let spawner = PoolSubagentSpawner::new(pool).with_agent_catalog(catalog);
        let meta = spawner.resolve_selection("proj-agent", None).await;
        assert_eq!(meta.source, "projectSettings");
        assert!(!meta.is_built_in);
        assert_eq!(meta.color.as_deref(), Some("green"));
    }

    // ── G14: name → agent-id registry round-trip ──

    #[tokio::test]
    async fn register_name_resolve_round_trip() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        let id = AgentId::new();
        assert_eq!(spawner.resolve_name("worker-x").await, None);
        spawner.register_name("worker-x", id).await;
        assert_eq!(spawner.resolve_name("worker-x").await, Some(id));
    }

    // ── #2/G13: spawn_async default surfaces a clear error (unwired) ──

    #[tokio::test]
    async fn spawn_async_default_returns_internal_error() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);
        let invoker: Arc<dyn ToolInvoker> = Arc::new(DummyInvoker);
        let budget: Arc<dyn BudgetEnforcerHandle> = Arc::new(DummyBudget);
        let req = SubagentSpawnRequest {
            subagent_type: "general-purpose".into(),
            prompt: "go".into(),
            context_paths: vec![],
            description: None,
            model: None,
            model_profile: None,
            run_in_background: true,
            name: None,
            team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: None,
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            depth: 0,
        };
        let err = spawner
            .spawn_async(req, SubagentInheritance { tool_invoker: invoker, budget })
            .await
            .expect_err("default spawn_async is unwired → clear error");
        assert!(format!("{err}").contains("not wired"));
    }
}
