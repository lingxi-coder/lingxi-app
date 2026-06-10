//! Desktop composition root (M8-P6).
//!
//! `engine-desktop` is the single declarative place that decides **which**
//! builtin tools and slash-commands (and, from P8, skills) ship in the
//! desktop build. It names each capability crate via a Cargo dependency edge
//! in `Cargo.toml` — the mobile composition root (`engine-mobile`, P11) will
//! depend on a different subset. There is no `#[cfg(target_os)]` switching:
//! the shipped capability set is chosen by *which composition root the app
//! links*, not by conditional compilation scattered through library crates.
//!
//! ## What lives here
//! - [`desktop_tool_registry`] / [`register_desktop_tools`] — assemble the 14
//!   desktop tool crates into a [`ToolRegistry`].
//! - [`desktop_command_registry`] — assemble the builtin slash-commands.
//! - [`DesktopEngineConfig`] — the knobs the composition root needs.
//!
//! ## Construction order (owned by the host binary)
//! The runtime wiring has an inherent cycle: tools must exist before the
//! orchestrator (it owns the [`ToolRegistry`]), and the command handlers bind
//! to an `Arc<dyn OrchestratorHandle>` produced *by* that orchestrator. So the
//! host binary (`apps/cli`) drives the order — build tools → build orchestrator
//! → build commands — and this crate provides the two pure assembly functions
//! it calls. A future consolidation can fold the orchestrator wiring into a
//! single `build(platform, config) -> Engine` here once the platform aggregate
//! trait (P10) lands; see the design doc §6.4.

#![forbid(unsafe_code)]

pub mod file_changed_watch;
pub mod settings_watch;
mod skill_loader;

use anthropic_oauth::client::ClaudeAiOAuthClient;
use anthropic_oauth::config::ClaudeAiOAuthConfig;
use anthropic_oauth::handle::OAuthHandle;
use api_client::AnthropicProvider;
use client_adapter::{AdapterPermissionGate, PermissionRequestSink};
use command_api::{CommandRegistry, RegistrySlashDispatcher};
use command_core::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2,
    register_core_batch_4, register_core_batch_5,
};
use orchestrator::test_support::{NoOpPermissionGate, StaticMemoryProvider};
use orchestrator::{
    ConversationOrchestrator, OrchestratorApiClient, OrchestratorConfig, ProviderApiAdapter,
};
use permission::gate::PermissionGate;
use platform_posix_minimal::{
    PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp, PosixMcp, PosixProcess,
    PosixRuntime, PosixSandbox, PosixWorktree,
};
use providers::{builtin_profiles, parse_profiles, parse_routing, ModelRouter, ProviderRegistry};
use sandbox::decision::ProjectTrustLevel;
use sandbox::runtime_config::{Platform as SandboxPlatform, SandboxRuntimeConfig};
use secret::CredentialManager;
use skill_api::SkillRegistry;
use std::sync::Arc;
use tokio::sync::RwLock;
use tool_api::{BuiltinToolContext, ToolRegistry};
use traits::{AuthHandle, McpTransport, OrchestratorHandle, OutputStream};

/// Re-export the pure model-deprecation lookup (`providers::deprecation`) at the
/// composition-root surface so the host binary can compute the startup
/// deprecation notice without taking a direct `providers` dependency.
///
/// The CLI host (`apps/cli`) resolves the same model id it threads into
/// [`DesktopConfig::default_model`] (argv `--model`, else the desktop default),
/// passes it here, and surfaces the returned warning at startup — the bounded
/// stand-in for claude-code's `getModelDeprecationWarning(resolvedInitialModel)`
/// startup-notification-queue entry (`main.tsx:2873`/`2889-2896`). With any
/// current (Claude 4-generation) default model the lookup returns `None`, so the
/// startup output stays byte-identical until a user configures one of the
/// deprecated Claude 3 ids.
pub use providers::deprecation::model_deprecation_warning;

/// M10 (T13): per-teammate `StateMachinePool` slot cap.
///
/// Teammates are PERSISTENT: each one parks on `wait_for_message` between
/// turn-sets and NEVER frees its pool slot until killed. Sharing the
/// `AgentTool` `subagent_pool` (cap 4) would let parked teammates starve
/// one-shot subagent spawns, so the teammate handler gets its OWN pool with
/// this cap (T14 adds the pool-starvation regression that proves the
/// separation). Sized to match the `subagent_pool` cap so a coordinator can run
/// a small team without immediately exhausting slots.
///
/// `pub` so the T14 `pool_starvation` regression test can pin its parked-teammate
/// count to the single production source of truth (no magic-number drift).
pub const TEAMMATE_POOL_CAP: usize = 4;

/// Derive the [`permission::SandboxAutoAllowConfig`] the enforced
/// [`permission::PermissionPolicy`] consults from the same `settings.json`
/// tiers the policy block already reads.
///
/// `raw_tiers` is the per-tier raw `settings.json` text in ASCENDING priority
/// (user → project → local), exactly the order the enforcement block loads its
/// rules; a later tier's `sandbox` field overrides an earlier one (last write
/// wins), mirroring how `defaultMode` is resolved there. Each tier is parsed as
/// a [`sandbox::runtime_config::SettingsJson`]; only the `sandbox` subsection
/// (and its `permissions` are irrelevant to the three auto-allow fields) is
/// consulted, folded into one merged
/// [`sandbox::runtime_config::SandboxRuntimeConfig`] via
/// [`sandbox::policy_convert::convert_settings_to_runtime_config`].
///
/// The three fields the bash sandbox-auto-allow branch reads are then copied
/// out (`enabled`, `auto_allow_bash_if_sandboxed`, `excluded_commands`). One
/// faithfulness fix vs. the raw conversion: claude-code's
/// `isAutoAllowBashIfSandboxedEnabled()` defaults **true**, but the Rust
/// `SandboxRuntimeConfig::auto_allow_bash_if_sandboxed` is a bare `bool` that
/// `serde`-defaults to `false` and the converter only sets it when the settings
/// explicitly carry it. So we recover the explicit/absent distinction from the
/// per-tier [`sandbox::runtime_config::SandboxSettingsJson::auto_allow_bash_if_sandboxed`]
/// (`Option<bool>`): the last tier that set it wins; if NO tier set it the TS
/// default `true` applies.
#[must_use]
fn sandbox_auto_allow_from_settings_tiers(
    raw_tiers: &[&str],
) -> permission::sandbox_auto_allow::SandboxAutoAllowConfig {
    use sandbox::runtime_config::{SandboxSettingsJson, SettingsJson};

    // Fold each tier's `sandbox` subsection, last write wins per the whole
    // subsection (matching how the converter consumes a single `SettingsJson`).
    let mut merged_sandbox: Option<SandboxSettingsJson> = None;
    // Track the explicit auto-allow override separately so the TS default (true)
    // can be applied only when NO tier set it.
    let mut explicit_auto_allow: Option<bool> = None;
    for raw in raw_tiers {
        let Ok(parsed) = serde_json::from_str::<SettingsJson>(raw) else {
            continue;
        };
        if let Some(s) = parsed.sandbox {
            if let Some(v) = s.auto_allow_bash_if_sandboxed {
                explicit_auto_allow = Some(v);
            }
            merged_sandbox = Some(s);
        }
    }

    let runtime = sandbox::policy_convert::convert_settings_to_runtime_config(&SettingsJson {
        sandbox: merged_sandbox,
        ..Default::default()
    });

    permission::sandbox_auto_allow::SandboxAutoAllowConfig::new(
        runtime.enabled,
        // claude-code `isAutoAllowBashIfSandboxedEnabled()` defaults TRUE.
        explicit_auto_allow.unwrap_or(true),
        runtime.excluded_commands,
    )
}

/// M10 (T13): a late-bound [`traits::tool_invoker::ToolInvoker`] resolving the
/// composition-root construction cycle.
///
/// The teammate handler is registered into the `TaskRegistry` (which needs
/// `&mut self`, so BEFORE the registry is `Arc`-wrapped) yet must inherit the
/// parent's `Arc<ToolRegistry>` as its tool-dispatch seam — and that registry is
/// assembled AFTER the task registry exists (its `BuiltinToolContext` carries
/// `task_registry.clone()`). Naively this is a cycle.
///
/// `DeferredToolInvoker` breaks it: it is constructed empty, injected into the
/// teammate handler up front, and [`set`](Self::set) is called exactly once with
/// the real `RegistryToolInvoker` after `tools` is built. This preserves the
/// recursion-lock invariant (the teammate dispatches through the SAME
/// `Arc<ToolRegistry>` the parent owns — `RegistryToolInvoker` stores that Arc
/// verbatim) while satisfying the construction order. A teammate cannot dispatch
/// a tool before `build()` returns, so the cell is always filled before first
/// use.
struct DeferredToolInvoker {
    inner: std::sync::OnceLock<Arc<dyn traits::tool_invoker::ToolInvoker>>,
}

impl DeferredToolInvoker {
    fn new() -> Self {
        Self {
            inner: std::sync::OnceLock::new(),
        }
    }

    /// Fill the cell with the real invoker. Idempotent-safe: a second call is a
    /// no-op (the first binding wins), matching the build-once semantics.
    fn set(&self, invoker: Arc<dyn traits::tool_invoker::ToolInvoker>) {
        let _ = self.inner.set(invoker);
    }
}

#[async_trait::async_trait]
impl traits::tool_invoker::ToolInvoker for DeferredToolInvoker {
    async fn invoke(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: traits::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, traits::tool_invoker::ToolInvokerError> {
        match self.inner.get() {
            Some(invoker) => invoker.invoke(name, input, ctx).await,
            None => Err(traits::tool_invoker::ToolInvokerError::Internal(
                "DeferredToolInvoker: tool dispatch attempted before build() bound the registry"
                    .to_string(),
            )),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Desktop engine knobs.
///
/// Intentionally small in P6 — it grows as P8/P9 fold skills + commands and a
/// full `build()` entrypoint into the composition root.
#[derive(Clone, Debug)]
pub struct DesktopEngineConfig {
    /// Model id the desktop build defaults to when argv omits `--model`.
    pub default_model: String,
}

impl Default for DesktopEngineConfig {
    fn default() -> Self {
        Self {
            default_model: "claude-sonnet-4-20250514".to_string(),
        }
    }
}

/// M10 (T12): the three coordinator handles a coordinator-capable session
/// passes to [`register_desktop_tools`] / [`desktop_tool_registry`] so the
/// composition root can register the coordinator `TeamCreate` / `TeamDelete`
/// tools IN PLACE OF `tool_team`'s pair.
///
/// All four coordinator tool names collide byte-for-byte with the already
/// registered `tool_team` / `tool_ui` builtins, and [`ToolRegistry`] is
/// push-no-dedup with first-match-wins ([`ToolRegistry::find_by_name`]).
/// Naively splicing the coordinator tools *after* `tool_team::register_all`
/// would therefore silently shadow nothing (the `tool_team` copy wins every
/// lookup). Mode-exclusivity is the only safe option, and the registry is
/// built-once-and-moved, so the choice MUST be made at BUILD time — hence this
/// is threaded through as an `Option` rather than toggled later.
///
/// `build()` populates this with `Some(..)` ONLY when
/// `DesktopConfig::session_started_as_coordinator` is `true`; a default session
/// passes `None`, leaving the assembled tool set byte-identical to the pre-M10
/// build.
pub struct CoordinatorWiring {
    /// The per-session team registry the coordinator tools mutate.
    pub team: Arc<coordinator::TeamRegistry>,
    /// The coordinator-mode gate the tools consult in `call()`
    /// (defense-in-depth) so a future `/coordinator exit()` can neutralize them
    /// without rebuilding the registry.
    pub mode: Arc<coordinator::CoordinatorMode>,
    /// The spawn/kill seam the tools use to start / stop the real backing
    /// `InProcessTeammate` task.
    pub spawn_seam: Arc<dyn traits::team_spawn::TeamSpawnSeam>,
    /// The orchestrator-facing output stream the `TeamCreate` tool pushes the
    /// live active-worker count through immediately after a spawn is reconciled
    /// — the same `Arc<dyn OutputStream>` `build()` gives the orchestrator and
    /// the `CoordinatorStatusSink`. This makes `active_workers > 0` reach every
    /// client deterministically, independent of the teammate's racy startup
    /// status emit.
    pub output: Arc<dyn traits::OutputStream>,
}

/// Desktop [`ClaudeAiAuthProvider`](tool_cron::ClaudeAiAuthProvider) backed by
/// the credential store.
///
/// `RemoteTrigger` calls this in-process to add the refreshed claude.ai OAuth
/// access token + organization UUID to its requests — the token never reaches
/// the shell. The token is read at call-time (not snapshotted at boot) so the
/// proactive/reactive OAuth refresh driver — which persists rotated tokens back
/// to the same keychain — is always reflected. Mirrors TS
/// `checkAndRefreshOAuthTokenIfNeeded()` + `getClaudeAIOAuthTokens()`.
///
/// The credential read is async; the trait surface is sync. Desktop runs on a
/// multi-thread tokio runtime, so we bridge with
/// `block_in_place` + `Handle::block_on` (safe only on `rt-multi-thread`,
/// which the desktop binary uses).
struct CredentialStoreAuthProvider {
    credentials: Arc<CredentialManager>,
    base_api_url: String,
}

impl CredentialStoreAuthProvider {
    /// Snapshot the current persisted tokens (`None` when unauthenticated or the
    /// keychain read fails / the bridge cannot run — e.g. off a multi-thread
    /// runtime). Reading at call-time keeps the token fresh across refreshes.
    fn snapshot(&self) -> Option<secret::credential::OAuthTokens> {
        let creds = self.credentials.clone();
        let read = move || {
            tokio::runtime::Handle::try_current()
                .ok()
                .and_then(|h| h.block_on(async { creds.get_oauth_tokens().await.ok().flatten() }))
        };
        // `block_on` inside an async task requires `block_in_place` (multi-thread
        // runtime). If we're already off-runtime, call directly.
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(read),
            Err(_) => None,
        }
    }
}

impl tool_cron::ClaudeAiAuthProvider for CredentialStoreAuthProvider {
    fn access_token(&self) -> Option<String> {
        self.snapshot()
            .map(|t| t.access_token.expose_secret().clone())
    }
    fn org_uuid(&self) -> Option<String> {
        self.snapshot().map(|t| t.org_id).filter(|o| !o.is_empty())
    }
    fn base_api_url(&self) -> String {
        self.base_api_url.clone()
    }
}

/// Assemble the desktop builtin **tool** registry from a freshly-built
/// [`BuiltinToolContext`].
///
/// This is the canonical desktop tool set: 9 cross-platform crates
/// (`tool-file/shell/task/web/plan/meta/cron/ui/skill`) + 5 desktop-only
/// crates (`tool-agent/team/worktree/mcp/lsp`). The mobile composition root
/// links only the cross-platform subset plus mobile-specific crates.
///
/// `coordinator` selects the team-tool variant at build time: `None` registers
/// `tool_team`'s `TeamCreate` / `TeamDelete` (default), `Some(..)` registers the
/// coordinator pair IN PLACE OF them. See [`CoordinatorWiring`].
///
/// `cron_auth` is the in-process OAuth resolver `RemoteTrigger` uses; `None`
/// leaves the tool on its "not authenticated" pre-flight path (used by the
/// offline registry-snapshot tests).
#[must_use]
pub fn desktop_tool_registry(
    ctx: BuiltinToolContext,
    coordinator: Option<CoordinatorWiring>,
    cron_auth: Option<Arc<dyn tool_cron::ClaudeAiAuthProvider>>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    // Offline / snapshot path: no command registry to back the Skill tool, so it
    // gets the hermetic `EmptySkillLoader` (tool name unchanged → snapshot-safe).
    // No `CwdChanged` firer here either (offline factory has no hook executor) —
    // the BashTool is the byte-identical no-firer variant.
    register_desktop_tools(&mut reg, ctx, coordinator, cron_auth, None, None, None);
    reg
}

/// Register the desktop tool set into an existing (empty) registry.
///
/// Each `tool_*::register_all` consumes a clone of `ctx`; the final crate
/// takes ownership to avoid a redundant clone.
///
/// When `coordinator` is `Some(..)` (a coordinator-capable session), the
/// coordinator `TeamCreate` / `TeamDelete` tools are registered IN PLACE OF
/// `tool_team`'s pair: `tool_team::register_all` is SKIPPED entirely (it
/// registers exactly those two and no others — see `tools/team/src/lib.rs`), and
/// the coordinator pair is pushed instead. This keeps exactly ONE `TeamCreate`
/// and ONE `TeamDelete` in the registry (no silent shadow, no duplicate name in
/// the system prompt). When `None`, `tool_team::register_all` runs as before and
/// the coordinator tools are absent — byte-identical to the pre-M10 build.
pub fn register_desktop_tools(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    coordinator: Option<CoordinatorWiring>,
    cron_auth: Option<Arc<dyn tool_cron::ClaudeAiAuthProvider>>,
    skill_loader: Option<Arc<dyn tool_skill::skill::SkillLoader>>,
    cwd_changed_firer: hooks::OptionalCwdChangedFirer,
    web_side_query: Option<Arc<dyn sidequery::SideQueryClient>>,
) {
    // ----- cross-platform tool crates (also linked by engine-mobile, P11) ---
    tool_file::register_all(reg, ctx.clone());
    // BASH.4 `onCwdChangedForHooks` (Shell.ts:409): when a firer is supplied (real
    // desktop sessions wire one over the shared `Arc<HookExecutorImpl>`), a `cd`
    // inside a Bash call fires the `CwdChanged` hook. `None` (the offline
    // registry-snapshot path) keeps the byte-identical no-firer BashTool — the
    // registered tool NAMES are unchanged either way, so the locked tool-list
    // snapshot is unaffected. `engine-mobile` never reaches this call (it does
    // not register the shell tools).
    tool_shell::register_all_with_cwd_firer(reg, ctx.clone(), cwd_changed_firer);
    tool_web::register_all(reg, ctx.clone(), web_side_query);
    tool_plan::register_all(reg, ctx.clone());
    tool_meta::register_all(reg, ctx.clone());
    // `RemoteTrigger` gets the credential-store auth provider on desktop so it
    // can drive the claude.ai CCR API in-process. `register_all_with_auth`
    // registers `ScheduleCron` + `RemoteTrigger` (the latter with `cron_auth`).
    tool_cron::register_all_with_auth(reg, ctx.clone(), cron_auth);
    tool_ui::register_all(reg, ctx.clone());
    // SKILLEXEC.2: when a `SkillLoader` is supplied (real sessions wire the
    // `CommandRegistry`-backed loader), register the `Skill` tool with it so a
    // model-invoked skill resolves to a real slash command and expands. The
    // `None` path (offline registry-snapshot tests) keeps the hermetic
    // `EmptySkillLoader` — the registered tool NAME ("Skill") is identical
    // either way, so the locked tool-list snapshot is unaffected.
    match skill_loader {
        Some(loader) => {
            reg.register_builtin(Arc::new(tool_skill::SkillTool::with_loader(
                ctx.clone(),
                loader,
            )));
        }
        None => tool_skill::register_all(reg, ctx.clone()),
    }
    tool_task::register_all(reg, ctx.clone());
    // ----- desktop-only tool crates ----------------------------------------
    tool_agent::register_all(reg, ctx.clone());
    match coordinator {
        // Coordinator-capable session: register the coordinator `TeamCreate` /
        // `TeamDelete` IN PLACE OF `tool_team`'s pair. `tool_team::register_all`
        // is deliberately NOT called — splicing-after would silently shadow.
        Some(CoordinatorWiring {
            team,
            mode,
            spawn_seam,
            output,
        }) => {
            for tool in coordinator::internal_tools::coordinator_internal_tools(
                team, mode, spawn_seam, output,
            ) {
                reg.register_builtin(tool);
            }
        }
        // Default session: `tool_team`'s pair, coordinator tools absent.
        None => tool_team::register_all(reg, ctx.clone()),
    }
    tool_worktree::register_all(reg, ctx.clone());
    tool_mcp::register_all(reg, ctx.clone());
    tool_lsp::register_all(reg, ctx);
}

/// Assemble the desktop builtin **skill** registry.
///
/// Delegates to `skill_builtin::register_desktop`, the single place that names
/// the desktop builtin skill set. Empty in M8 (no Rust-bundled skills yet —
/// skills are markdown loaded from disk by the session loader); the mobile
/// composition root will call `skill_builtin::register_mobile` instead.
#[must_use]
pub fn desktop_skill_registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    skill_builtin::register_desktop(&mut reg);
    reg
}

/// Deterministic, env/argv-free recipe for building a desktop runtime.
///
/// F2-00 (deliverable-zero): every value the ~270-line `build_runtime`
/// (`apps/cli/src/init.rs:148`) currently reads from `std::env`/`Argv` becomes
/// an explicit field here, so both the CLI host **and** the bridge-server (and
/// the F2 end-to-end test) can construct an identical runtime *without*
/// touching the process environment. F2-01 lifts the actual wiring into
/// `engine_desktop::build(DesktopConfig) -> DesktopRuntime`; this task only
/// freezes the field set.
///
/// Field provenance (each mirrors a concrete read in `build_runtime`):
/// - `api_base` — `resolve_api_base()` (env `LINGXI_API_BASE_URL`, init.rs:95).
/// - `api_key` — env `ANTHROPIC_API_KEY` (init.rs:153).
/// - `cwd` — `std::env::current_dir()` (init.rs:248).
/// - `claude_home` — the `~/.claude` (and platform config-dir) root the hook /
///   agents / global-MCP loaders walk (init.rs:253-317); made explicit so the
///   bridge-server can point it at a sandbox in tests.
/// - `default_model` — `Argv::model` ⟶ `OrchestratorConfig.model` (init.rs:213).
/// - `provider_profiles` — the settings `providers` block as raw JSON, fed
///   verbatim to `providers::parse_profiles` (`load_provider_profiles()`,
///   init.rs:175). `None` ⟶ built-in profiles only.
/// - `mcp_paths` — the precedence-ordered `.mcp.json` paths (project then
///   global) handed to `mcp::load_mcp_json_with_precedence` (init.rs:253-258).
/// - `use_noop_permission_gate` — lets the CLI opt into `NoOpPermissionGate`
///   (init.rs:245) while the bridge-server binds `AdapterPermissionGate`.
///
/// # Examples
///
/// Field-by-field construction (no env / argv reads — fully deterministic):
///
/// ```
/// use engine_desktop::DesktopConfig;
/// use std::collections::BTreeMap;
/// use std::path::PathBuf;
///
/// let cfg = DesktopConfig {
///     api_base: "https://api.anthropic.com".to_string(),
///     api_key: "sk-test".to_string(),
///     cwd: PathBuf::from("/tmp/project"),
///     claude_home: PathBuf::from("/tmp/home/.claude"),
///     default_model: "claude-sonnet-4-20250514".to_string(),
///     fallback_model: None,
///     provider_profiles: Some(BTreeMap::new()),
///     routing: None,
///     mcp_paths: vec![PathBuf::from("/tmp/project/.mcp.json")],
///     use_noop_permission_gate: false,
///     session_started_as_coordinator: false,
///     // `None` ⟶ empty memory (deterministic). A production host injects
///     // `Some(orchestrator::prompt::real_provider())` to load real CLAUDE.md.
///     memory_provider: None,
///     permission_mode: permission::PermissionMode::Default,
/// };
///
/// assert_eq!(cfg.cwd, PathBuf::from("/tmp/project"));
/// assert!(!cfg.use_noop_permission_gate);
/// // `DesktopConfig` is `Clone` so a host can fan it out to multiple builders.
/// let _clone = cfg.clone();
/// ```
#[derive(Clone)]
pub struct DesktopConfig {
    /// API base URL (default `https://api.anthropic.com`); env override
    /// `LINGXI_API_BASE_URL` is resolved by the host *before* it fills this.
    pub api_base: String,
    /// Anthropic API key. Empty string is valid — the orchestrator builds
    /// successfully and only fails at `run_turn` with a 401, so slash-command
    /// dispatch still works with no key configured.
    pub api_key: String,
    /// Working directory the orchestrator + tool context are rooted at.
    pub cwd: std::path::PathBuf,
    /// The `~/.claude` root the hook / agents / global-MCP / settings loaders
    /// walk. Explicit so a host can redirect it to a sandbox.
    pub claude_home: std::path::PathBuf,
    /// Model id the build defaults to (`OrchestratorConfig.model`).
    pub default_model: String,
    /// Fallback model id (`OrchestratorConfig.fallback_model`). `None` ⟶ no
    /// fallback, so the 529-overload interception in `turn_loop` stays a strict
    /// no-op. Mirrors `Argv::fallback_model`, which claude-code only HONORS in
    /// `--print`/non-interactive mode ("only works with --print"); the CLI host
    /// applies that gate before filling this field (`resolve_desktop_config`).
    pub fallback_model: Option<String>,
    /// Settings-declared `providers` block as raw JSON, fed verbatim to
    /// `providers::parse_profiles`. `None` ⟶ built-in profiles only.
    pub provider_profiles: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Settings-declared `routing` block as raw JSON, fed verbatim to
    /// `providers::parse_routing` (model aliases / fallback / retry). `None` ⟶
    /// the default (empty) routing config. Mirrors `load_routing()` in
    /// `apps/cli/src/init.rs` — additive to the F2-00 field set so the lift
    /// preserves byte-equivalent engine behavior (no silent routing drop).
    pub routing: Option<serde_json::Value>,
    /// Precedence-ordered `.mcp.json` paths (project preferred over global)
    /// handed to `mcp::load_mcp_json_with_precedence`.
    pub mcp_paths: Vec<std::path::PathBuf>,
    /// When `true`, bind `NoOpPermissionGate` (the CLI default); when `false`,
    /// the host binds the connection-scoped `AdapterPermissionGate`.
    pub use_noop_permission_gate: bool,
    /// M10 build-time coordinator-activation flag. When `true`, `build()`
    /// enters coordinator multi-agent mode and registers the coordinator
    /// `TeamCreate`/`TeamDelete` tools IN PLACE OF `tool_team`'s pair (decided
    /// at build time — the registry is built-once-and-moved). Defaults to
    /// `false`: a default session is byte-identical to the pre-M10 build
    /// (mode off, `tool_team` unchanged, no teammate spawned). Additive to the
    /// frozen field set.
    pub session_started_as_coordinator: bool,
    /// The CLAUDE.md hierarchy provider the orchestrator loads project/user
    /// memory from. `None` (the default) ⟶ the empty
    /// [`StaticMemoryProvider::empty`], so a default build loads NO memory and
    /// is fully deterministic (the boot tests rely on this). A production host
    /// injects `Some(orchestrator::prompt::real_provider())` to load the real
    /// `<cwd>/CLAUDE.md`, `<cwd>/CLAUDE.local.md`, and `~/.claude/CLAUDE.md`
    /// into the system prompt (claude-code parity), which also makes the
    /// session-start `fire_instructions_loaded()` fire over those files. The
    /// field is injectable (not a `bool` flag) so tests can supply a CONTROLLED
    /// in-memory [`StaticMemoryProvider::with_files`] and never touch the real
    /// filesystem.
    pub memory_provider: Option<Arc<dyn orchestrator::prompt::MemoryHierarchyProvider>>,
    /// CLI-resolved session permission mode (claude-code
    /// `initialPermissionModeFromCLI`). Replaces the previously hardwired
    /// `BuiltinToolContext.permission_mode = Default`. When
    /// `LINGXI_ENFORCE_PERMISSIONS` enforcement is ON, this OVERRIDES the
    /// settings `defaultMode` as the highest-priority source; `BypassPermissions`
    /// makes the policy allow-all (unless the bypass killswitch is set).
    ///
    /// EXECUTION-SEMANTICS NOTE: with the default `NoOpPermissionGate`
    /// (enforcement OFF) this field is execution-neutral — tools already
    /// all-allow — but it still drives `BuiltinToolContext.permission_mode`
    /// state. The CLI flag deliberately does NOT switch enforcement on.
    pub permission_mode: permission::PermissionMode,
}

impl std::fmt::Debug for DesktopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn MemoryHierarchyProvider` is not `Debug`, so render the
        // `memory_provider` field as a presence marker. Every other field is
        // printed verbatim so `{cfg:?}` stays useful for host logging.
        f.debug_struct("DesktopConfig")
            .field("api_base", &self.api_base)
            .field("api_key", &self.api_key)
            .field("cwd", &self.cwd)
            .field("claude_home", &self.claude_home)
            .field("default_model", &self.default_model)
            .field("fallback_model", &self.fallback_model)
            .field("provider_profiles", &self.provider_profiles)
            .field("routing", &self.routing)
            .field("mcp_paths", &self.mcp_paths)
            .field("use_noop_permission_gate", &self.use_noop_permission_gate)
            .field(
                "session_started_as_coordinator",
                &self.session_started_as_coordinator,
            )
            .field(
                "memory_provider",
                if self.memory_provider.is_some() {
                    &"Some(<provider>)"
                } else {
                    &"None"
                },
            )
            .field("permission_mode", &self.permission_mode)
            .finish()
    }
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            cwd: std::path::PathBuf::from("."),
            claude_home: std::path::PathBuf::new(),
            default_model: DesktopEngineConfig::default().default_model,
            fallback_model: None,
            provider_profiles: None,
            routing: None,
            mcp_paths: Vec::new(),
            use_noop_permission_gate: true,
            session_started_as_coordinator: false,
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
        }
    }
}

/// Assemble the desktop slash-command registry.
///
/// Mirrors the boot sequence the CLI used inline before P6:
/// [`register_all_builtin_commands`] seeds the builtin handlers, then
/// [`register_core_batch_1`] + [`register_core_batch_2`] overwrite the wired
/// core handlers with their orchestrator/auth-bound implementations.
#[must_use]
pub async fn desktop_command_registry(
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
    cwd: &std::path::Path,
    claude_home: &std::path::Path,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    // Desktop-only command handlers (no-op in M8 — the names remain
    // command-core unimplemented stubs until future milestones fill them).
    command_desktop::register(&mut reg);
    // SLASH.2: discover + register custom `.claude/commands/**.md` commands
    // (project up to git-root/home, plus user + managed layers), the same
    // layering claude-code's getCommands uses. Registered AFTER builtins so a
    // same-named custom command shadows a builtin (TS findCommand order).
    let home = dirs::home_dir().unwrap_or_else(|| claude_home.to_path_buf());
    let registered = command_core::load_and_register_custom_commands(
        &mut reg,
        cwd,
        claude_home,
        &crate::settings_watch::managed_settings_dir(),
        &home,
    )
    .await;
    tracing::debug!(custom_commands = registered, "registered custom slash commands");
    reg
}

/// Everything a host needs to drive a conversation, built deterministically by
/// [`build`] from a [`DesktopConfig`].
///
/// This is the lifted shape of `apps/cli`'s `Runtime` (F2-01): moving the
/// runtime wiring out of the CLI binary lets the bridge-server (which CANNOT
/// depend on `apps/cli` — app→app is a leaf, `scripts/check_deps.py:99`)
/// construct an identical orchestrator. The CLI now derives a `DesktopConfig`
/// from `Argv`/env and calls `build`.
pub struct DesktopRuntime {
    /// The fully-constructed orchestrator (cost tracker + MCP/hook/agent
    /// registries + compaction wired), bound to the supplied output stream and
    /// permission gate.
    pub orchestrator: Arc<ConversationOrchestrator>,
    /// Slash-command dispatcher seeded with the builtin handlers + wired core
    /// handlers (the `register_all_builtin_commands` → `register_core_batch_1`
    /// → `register_core_batch_2` sequence).
    pub dispatcher: RegistrySlashDispatcher,
    /// Auth handle for `/login` and `/logout`.
    pub auth: Arc<dyn AuthHandle>,
    /// The desktop task registry shared with the tool context (the TUI / a
    /// transport wraps it in a poller to read live background-task state).
    pub task_registry: Arc<tasks::registry::TaskRegistry>,
    /// M10: the per-session coordinator team registry. One is constructed per
    /// `build()` regardless of mode so the status feed (and the PHASE-2 command
    /// router) always have a handle to read; it is observable but empty (no
    /// workers) unless a coordinator session spawns teammates via `TeamCreate`.
    pub coordinator: Arc<coordinator::TeamRegistry>,
    /// M10: the per-session coordinator-mode flag. Entered at build time only
    /// when `cfg.session_started_as_coordinator` is `true`; otherwise this is
    /// constructed disabled (`is_enabled() == false`) and a default session is
    /// byte-identical to the pre-M10 build. A `call()`-time gate also consults
    /// it (defense-in-depth) so a future `/coordinator exit()` can neutralize
    /// the tools without a registry rebuild.
    pub coordinator_mode: Arc<coordinator::CoordinatorMode>,
    /// The connection-scoped [`AdapterPermissionGate`] handle, present ONLY when
    /// `cfg.use_noop_permission_gate` is `false`. The transport calls
    /// [`AdapterPermissionGate::resolve`] on this to satisfy a parked `check()`
    /// from an inbound `ApprovePermission`/`DenyPermission` (F2-06). `None` when
    /// the host opted into the always-allow `NoOpPermissionGate` (the CLI).
    pub permission_gate: Option<Arc<AdapterPermissionGate>>,
    /// Live settings watcher firing `ConfigChange` hooks when the user /
    /// project / local / policy settings files mutate on disk (parity:
    /// claude-code `changeDetector.ts` → `executeConfigChangeHooks`). Held by
    /// the runtime so it lives for the session; dropping the runtime aborts the
    /// watch tasks (RAII teardown). `None`-shaped as an empty handle (no tasks)
    /// when no `.claude` directory exists to watch.
    pub settings_watcher: settings_watch::SettingsWatcherHandle,
    /// Live file-changed watcher firing `FileChanged` hooks when a path resolved
    /// from a `FileChanged` hook's `matcher` mutates on disk (parity: claude-code
    /// `fileChangedWatcher.ts` → `executeFileChangedHooks`). Held by the runtime
    /// so it lives for the session; dropping the runtime aborts the watch tasks
    /// (RAII teardown). An empty handle (no tasks) when no `FileChanged` hook is
    /// configured — the no-watch case is byte-identical to before.
    pub file_changed_watcher: file_changed_watch::FileChangedWatcherHandle,
}

/// Errors surfaced while building a [`DesktopRuntime`].
///
/// Lifted from `apps/cli`'s `InitError`. Construction is effectively infallible
/// today (the orchestrator constructor cannot fail), but the typed error is kept
/// so future iterations (real OAuth bootstrap, MCP connect) can surface a cause
/// without changing every call site.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// API base URL resolution / api-client construction failed.
    #[error("api base resolution failed: {0}")]
    ApiBase(String),
    /// Orchestrator construction failed.
    #[error("orchestrator construction failed: {0}")]
    Orchestrator(String),
}

/// Build a fully-wired desktop [`DesktopRuntime`] from a deterministic
/// [`DesktopConfig`] (F2-01 — the runtime-wiring lift out of `apps/cli`).
///
/// This is the byte-equivalent move of `apps/cli`'s `build_runtime`: every value
/// it read from `std::env`/`Argv` now arrives as an explicit `cfg` field, so
/// BOTH the CLI host and the bridge-server can construct an identical runtime
/// WITHOUT touching the process environment (the F2 end-to-end test depends on
/// this determinism). No `std::env`/`Argv` reads occur inside `build`.
///
/// The engine behavior is preserved verbatim (spec §1 non-goal — no
/// orchestrator / api-client semantic changes); only the *source* of each input
/// moved from env/argv to `cfg`, and the output/permission sinks become
/// connection-scoped parameters:
///
/// - `output` is the [`traits::OutputStream`] the orchestrator pushes turn
///   events to. The CLI supplies its NDJSON/plain/TUI sink; the bridge-server
///   supplies a `client_adapter::AdapterOutputStream`. The SAME `build` serves
///   both.
/// - `permission_sink` is the destination for the [`AdapterPermissionGate`]'s
///   outbound `PermissionRequest`s. It is wired ONLY when
///   `cfg.use_noop_permission_gate` is `false`; the CLI passes a sink that is
///   never used because it opts into `NoOpPermissionGate`.
///
/// Resolve the Claude.ai-subscriber flag (`isClaudeAISubscriber`, `auth.ts:1564`)
/// for a session that holds a stored OAuth token.
///
/// `isClaudeAISubscriber()` is `isAnthropicAuthEnabled() && shouldUseClaudeAIAuth(scopes)`.
/// `isAnthropicAuthEnabled()` is `false` whenever a non-OAuth source OUTRANKS
/// stored OAuth in the auth resolver (`anthropic_oauth::resolver`). The two such
/// sources surfaced into the desktop build are the env `ANTHROPIC_API_KEY`
/// (`api_key_present`) and `ANTHROPIC_AUTH_TOKEN` (`auth_token_present`); when
/// either is set the effective auth is that key/bearer, not Claude.ai OAuth.
/// Bedrock / api-key-helper / settings keys rank BELOW stored OAuth, so OAuth
/// wins over them — no exclusion needed. With neither override present, the token
/// is the effective auth and `shouldUseClaudeAIAuth(scopes)` (== presence of the
/// `user:inference` scope, via `anthropic_oauth::subscription_from_scopes`)
/// decides.
///
/// PARITY-GAP: FD-inherited keys + managed-context OAuth forcing are not surfaced
/// into [`DesktopConfig`]; the common desktop API-key-vs-OAuth split is covered.
fn oauth_subscriber_flag(api_key_present: bool, auth_token_present: bool, scopes: &[String]) -> bool {
    !api_key_present
        && !auth_token_present
        && anthropic_oauth::subscription_from_scopes(scopes)
}

/// Load the merged `settings.outputStyle` (project + user + env layers) for the
/// given project dir. Mirrors the CLI's `load_routing`/`load_provider_profiles`
/// helpers (same `engine::settings::Settings::load` seam). Returns `None` on any
/// load failure or when the field is unset — the caller then injects no output
/// style section (OUTSTYLE.2).
fn load_merged_output_style(project_dir: &std::path::Path) -> Option<String> {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.output_style)
}

/// # Errors
///
/// Returns [`BuildError`] if the api-client or orchestrator cannot be
/// constructed (effectively infallible in the current wiring).
#[allow(clippy::too_many_lines)]
pub async fn build(
    cfg: DesktopConfig,
    output: Arc<dyn OutputStream>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<DesktopRuntime, BuildError> {
    let cwd = cfg.cwd.clone();

    // (1) Platform-minimal façade (http + clock + storage).
    let http = Arc::new(PosixHttp::new());
    let clock = Arc::new(PosixClock::new());
    let storage = Arc::new(PlainTextSecureStorage::new());

    // (2) api-client via ProviderRegistry. An empty `api_key` is accepted — the
    //     orchestrator builds and only fails at `run_turn` with a 401, so
    //     slash-command dispatch still works with no key configured. The
    //     settings `providers` / `routing` blocks now arrive via `cfg`
    //     (previously `load_provider_profiles()` / `load_routing()` read env).
    let env_snapshot: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let mut profiles = builtin_profiles(Some(cfg.api_base.clone()));
    match parse_profiles(cfg.provider_profiles.as_ref()) {
        Ok(extra) => profiles.extend(extra),
        Err(e) => tracing::warn!(error = %e, "ignoring malformed settings `providers` block"),
    }
    let routing = parse_routing(cfg.routing.as_ref());
    let registry = Arc::new(ProviderRegistry::new(
        profiles,
        env_snapshot,
        http.clone(),
        routing,
    ));
    // Build the CONCRETE adapter first so it can be coerced to BOTH the
    // orchestrator seam (`OrchestratorApiClient`) and the agent seam
    // (`agent::SubagentApiClient`). `ProviderApiAdapter` impls both (see
    // orchestrator/src/provider_adapter.rs); type-erasing to one trait object
    // up front would forfeit the other coercion.
    let provider_adapter = Arc::new(ProviderApiAdapter::new(
        Arc::clone(&registry) as Arc<dyn ModelRouter>,
    ));
    let api_client: Arc<dyn OrchestratorApiClient> = provider_adapter.clone();
    let subagent_api: Arc<dyn agent::SubagentApiClient> = provider_adapter;
    // WebSearch builds Anthropic `POST /v1/messages` requests via its own
    // provider (server-side web search is Anthropic-only in v1).
    let tool_provider = Arc::new(AnthropicProvider::new(
        cfg.api_key.clone(),
        Some(cfg.api_base.clone()),
    ));

    // (3) Credential manager + OAuth client (used by /login, /logout).
    let credentials = Arc::new(CredentialManager::new(storage, clock.clone(), http.clone()));
    let oauth_cfg = ClaudeAiOAuthConfig::default_with_port(0);
    let oauth_client = Arc::new(ClaudeAiOAuthClient::new(
        oauth_cfg.clone(),
        http.clone(),
        credentials.clone(),
    ));
    let auth: Arc<dyn AuthHandle> = Arc::new(OAuthHandle::new(oauth_client));

    // (3.1) M5-13: attach the OAuth refresh driver to the api-client when the
    //        keychain already holds a logged-in OAuth token. `init_refresh_driver`
    //        registers the process-global `OAuthRefreshHook` (so the api-client's
    //        401-retry path calls `refresh` instead of `NoOpOAuthHook`) and spawns
    //        the proactive-refresh task. It MUST be called at most once per
    //        process; gating it on "tokens present" keeps the API-key path on the
    //        correct `NoOpOAuthHook`. When no OAuth token is stored (the common
    //        API-key case) we skip it entirely.
    //
    //        (3.2) API.6: while we have the token in hand, resolve the Claude.ai
    //        subscriber flag from its scopes (see [`oauth_subscriber_flag`]).
    let mut is_subscriber = false;
    match credentials.get_oauth_tokens().await {
        Ok(Some(tokens)) => {
            is_subscriber = oauth_subscriber_flag(
                !cfg.api_key.is_empty(),
                std::env::var_os("ANTHROPIC_AUTH_TOKEN").is_some(),
                &tokens.scopes,
            );
            if let Err(e) = anthropic_oauth::client::init_refresh_driver(
                oauth_cfg,
                tokens.access_token,
                tokens.refresh_token,
                tokens.expires_at,
                http.clone(),
                clock.clone(),
                Some(Arc::new(telemetry::AnalyticsBus::new())),
                Some(credentials.clone()),
                Arc::new(PosixRuntime::new()),
            )
            .await
            {
                tracing::warn!(error = %e, "failed to attach OAuth refresh driver; 401 auto-refresh disabled");
            }
        }
        Ok(None) => {
            // No stored OAuth session — API-key path. Leave `current_hook()` as
            // the NoOpOAuthHook (correct: nothing to refresh).
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not read OAuth tokens from keychain; skipping refresh-driver wiring");
        }
    }

    // (4) Orchestrator config from `cfg` (was `argv.model`).
    let mut orch_cfg = OrchestratorConfig::default();
    orch_cfg.model.clone_from(&cfg.default_model);
    // Opus-fallback hop: thread the (already print-mode-gated) fallback model
    // into `OrchestratorConfig.fallback_model`. `None` keeps the turn_loop's
    // 529-overload interception a strict no-op (`turn_loop.rs:496`).
    orch_cfg.fallback_model.clone_from(&cfg.fallback_model);
    // Subscription-flag hop (API.6): thread the resolved Claude.ai-subscriber flag
    // (computed in step 3.2 from the OAuth token scopes) into the orchestrator
    // config so the fallback-aware api-client seam resolves the consecutive-529
    // Opus-fallback gate and the 429-retry gate exactly as claude-code does.
    // `is_enterprise` stays `false` (PARITY-GAP: enterprise tier needs a profile
    // fetch not performed in this build hot path).
    orch_cfg.is_subscriber = is_subscriber;
    // OUTSTYLE.2: thread the merged `settings.outputStyle` (TS string) into the
    // orchestrator config so `build_system_prompt` injects the active style's
    // `# Output Style: <name>` section (Explanatory / Learning builtins). `None`
    // / "default" / unknown ⇒ no section (prompt byte-identical to before).
    orch_cfg.output_style = load_merged_output_style(&cfg.cwd);

    // (4.5) One CostTracker per process. The persist channel drains into a
    //       fire-and-forget task that discards snapshots (on-disk persistence is
    //       later work). Depth 64 absorbs bursts without blocking.
    let (cost_persist_tx, mut cost_persist_rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        while cost_persist_rx.recv().await.is_some() {}
    });
    let cost_tracker = Arc::new(cost::CostTracker::new(
        protocol::SessionId::new(),
        Arc::new(cost::PricingCatalog::builtin_reference()),
        cost_persist_tx,
    ));

    // (4.6) Subagent spawner pool + budget enforcer for the `AgentTool` seam.
    //       `AgentTool::call` requires BOTH `subagent_spawner` and
    //       `budget_enforcer` to be `Some` — wiring the spawner alone is inert.
    //
    //       The pool is the production `StateMachinePool` driven by the posix
    //       `RuntimeSpawner`; `max_concurrent = 4` matches the agent-crate
    //       fixtures. `with_api_client(subagent_api)` hands the child runner the
    //       real model seam so spawned subagents drive the multi-turn
    //       `run_subagent_loop` (gated on `ctx.api_client.is_some()`) instead of
    //       the legacy stub completion.
    let subagent_pool = Arc::new(agent::StateMachinePool::new(Arc::new(PosixRuntime::new()), 4));
    // Clone the subagent model seam BEFORE it is moved into the spawner — the
    // M10 coordinator teammate handler (T13) hands the SAME seam to every
    // spawned `InProcessTeammate` so it drives the real multi-turn loop.
    let teammate_api = subagent_api.clone();
    // The spawner cannot receive the tool registry / agent catalog here: both
    // are built below, and the registry construction forms a cycle through
    // `BuiltinToolContext` (which consumes `subagent_spawner`). So we grab clones
    // of the spawner's set-once cells BEFORE boxing it, and fill them once the
    // registry + catalog exist (just after `desktop_tool_registry`, below).
    // `with_default_model` anchors `AgentModel::Inherit` + family-alias tiers to
    // the parent model so built-in subagent spawns resolve to a concrete wire id
    // instead of passing `"inherit"`/`"haiku"` raw (parity batch 22).
    let subagent_spawner_concrete = agent::PoolSubagentSpawner::new(subagent_pool)
        .with_api_client(subagent_api)
        .with_default_model(orch_cfg.model.clone());
    let subagent_tool_registry_cell = subagent_spawner_concrete.tool_registry_handle();
    let subagent_agent_catalog_cell = subagent_spawner_concrete.agent_catalog_handle();
    let subagent_spawner: Arc<dyn traits::subagent_spawn::SubagentSpawner> =
        Arc::new(subagent_spawner_concrete);

    //       The budget enforcer is an unlimited / non-blocking config (every
    //       limit `None`, no warning thresholds, `WarnOnly` policy) so it never
    //       halts a turn — `BudgetConfig` has no production `Default`, so all
    //       five fields are spelled out. It shares the process `CostTracker`
    //       (cloned because the tracker is also moved into `.with_cost_tracker`
    //       below).
    let budget_enforcer: Arc<dyn traits::budget::BudgetEnforcerHandle> =
        Arc::new(cost::BudgetEnforcer::new(
            cost::BudgetConfig {
                max_session_nano_usd: None,
                max_turn_nano_usd: None,
                max_turn_tokens: None,
                warning_thresholds: Vec::new(),
                on_exceed: cost::BudgetExceedPolicy::WarnOnly,
            },
            cost_tracker.clone(),
        ));

    // (5) Memory filler + the permission gate. The gate is the F2-01 branch
    //     point: the CLI opts into the always-allow `NoOpPermissionGate`; a
    //     transport binds the connection-scoped `AdapterPermissionGate` and
    //     keeps its handle to `resolve()` inbound approvals (F2-06).
    //
    //     M5-13: the hook executor is no longer the `noop_hook_executor()` stub
    //     — it is constructed below (5.25) once `hook_registry` exists, so the
    //     HTTP / Command hook arms run for real.
    //     Memory provider: the production host injects
    //     `cfg.memory_provider = Some(orchestrator::prompt::real_provider())`
    //     so the orchestrator loads the real `<cwd>/CLAUDE.md` +
    //     `~/.claude/CLAUDE.md` hierarchy into the system prompt (claude-code
    //     parity) and `fire_instructions_loaded()` fires over those files.
    //     `None` (the default + every test caller) falls back to the empty
    //     `StaticMemoryProvider`, so a default build loads NO memory and the
    //     boot tests stay deterministic (they never read the real filesystem).
    let memory: Arc<dyn orchestrator::prompt::MemoryHierarchyProvider> = cfg
        .memory_provider
        .clone()
        .unwrap_or_else(|| Arc::new(StaticMemoryProvider::empty()));

    let (perms, adapter_gate): (Arc<dyn PermissionGate>, Option<Arc<AdapterPermissionGate>>) =
        if cfg.use_noop_permission_gate {
            (Arc::new(NoOpPermissionGate), None)
        } else {
            // (3c) Persist an `AllowAlways` choice to `<cwd>/.claude/settings.local.json`.
            let gate = Arc::new(AdapterPermissionGate::new(permission_sink).with_persist(
                permission::PermissionPaths {
                    claude_home: cfg.claude_home.clone(),
                    cwd: cwd.clone(),
                },
            ));
            (gate.clone() as Arc<dyn PermissionGate>, Some(gate))
        };

    // (5.1) Load `.mcp.json` (project preferred over user-global) and
    //       auto-connect every enabled server (Plan 13). `connect_all` seeds
    //       disabled servers as `Disconnected` so `/mcp` still lists them,
    //       connects the rest, and records per-server failures as loop-eligible
    //       `Disconnected { last_error }`. A background reconnect/backoff task
    //       then retries dropped remote servers. The precedence-ordered paths
    //       arrive via `cfg` (previously derived from `dirs::config_dir()` + cwd
    //       in the CLI).
    let project_mcp_path = cfg
        .mcp_paths
        .first()
        .cloned()
        .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));
    let global_mcp_path = cfg
        .mcp_paths
        .get(1)
        .cloned()
        .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));
    let mcp_configs = mcp::load_mcp_json_with_precedence(&project_mcp_path, &global_mcp_path);
    // Build one concrete `PosixMcp` and hand it to the registry as BOTH the
    // `McpTransport` (discovery) and the `RawConnectionProvider` (live-client
    // bridge), so a connected server yields a working `McpClient` via
    // `get_client`. The minimal stub owns no live connections, so the bridge
    // hands back `None` here today; the real `PosixMcpTransport` returns a live
    // `Arc<jsonrpc::Connection>` under the same wiring.
    let posix = Arc::new(PosixMcp::new());
    // The registry is BUILT here but `connect_all` is deferred to (5.26),
    // after the real `hooks` executor exists: the elicitation hook dispatcher
    // (`OrchestratorHookDispatcher`) must be wired via `with_hook_dispatcher`
    // BEFORE any server connects, so an incoming `elicitation/create` consults
    // the `Elicitation` hook. The registry is not used by anything between here
    // and (5.26), so deferring the connect is behavior-neutral aside from the
    // dispatcher wiring.

    // (5.2) HookRegistry — read settings.json hooks from project
    //       (cwd/.claude/settings.json) then user (claude_home/settings.json),
    //       project last so it wins on identical command registration. The user
    //       root is `cfg.claude_home` (was `dirs::config_dir()/claude`).
    let mut hook_registry = hooks::HookRegistry::new();
    let project_settings_path = cwd.join(".claude").join("settings.json");
    let user_settings_path = cfg.claude_home.join("settings.json");
    for (path, source) in [
        (user_settings_path, hooks::definition::HookSource::User),
        (
            project_settings_path,
            hooks::definition::HookSource::Project,
        ),
    ] {
        if let Ok(raw) = tokio::fs::read_to_string(&path).await {
            match hooks::parse_hooks_from_settings_json(&raw, source) {
                Ok(hooks_vec) => {
                    for h in hooks_vec {
                        hook_registry.register(h);
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "skipping malformed settings hooks"
                ),
            }
        }
    }
    let hook_registry = Arc::new(tokio::sync::RwLock::new(hook_registry));

    // (5.2b) Permission enforcement (OPT-IN, parity phase 2). When
    //        LINGXI_ENFORCE_PERMISSIONS is set, load the settings permission
    //        rules into a PermissionPolicy and WRAP the gate selected above with
    //        PolicyPermissionGate: deny/allow rules are now enforced, an `Ask`
    //        delegates to the inner gate's prompt (or auto-allows a read-only
    //        tool to avoid an ask-storm). Unset (the DEFAULT) leaves the
    //        always-allow NoOp/Adapter gate untouched — NO behavior change.
    //        Reads the SAME two settings files as the hooks loader above
    //        (project read last → its `defaultMode` wins). File-glob content
    //        matching (3a) + subagent/teammate-path enforcement (3b) now land
    //        too; only Bash/WebFetch content matching (3a-bash) stays tool-wide.
    let perms: Arc<dyn PermissionGate> =
        if std::env::var_os("LINGXI_ENFORCE_PERMISSIONS").is_some_and(|v| !v.is_empty()) {
            let mut rules = Vec::new();
            let mut mode = permission::PermissionMode::Default;
            // Retain each tier's raw text (in ascending priority) so the
            // sandbox-auto-allow config can be derived from the SAME settings.
            let mut raw_tiers: Vec<String> = Vec::new();
            // Bypass-permissions killswitch: if ANY tier sets
            // `disableBypassPermissionsMode: "disable"`, the policy refuses
            // `BypassPermissions` mode (`authorize` falls back to Ask). Sticky
            // across tiers — a disable is not overridable upward (claude-code).
            let mut bypass_disabled = false;
            // Read the three persistable rule tiers in ASCENDING priority so
            // the highest-priority `defaultMode` wins (last write). settings.local.json
            // (3c) is read LAST so an `AllowAlways` persisted there is loaded back
            // and honored on the next enforced boot (closing the persist↔enforce
            // round-trip); rules from every tier accumulate (bucketed by source,
            // `authorize` walks them by priority).
            for (path, source) in [
                (
                    cfg.claude_home.join("settings.json"),
                    permission::PermissionRuleSource::UserSettings,
                ),
                (
                    cwd.join(".claude").join("settings.json"),
                    permission::PermissionRuleSource::ProjectSettings,
                ),
                (
                    cwd.join(".claude").join("settings.local.json"),
                    permission::PermissionRuleSource::LocalSettings,
                ),
            ] {
                if let Ok(raw) = tokio::fs::read_to_string(&path).await {
                    match permission::permission_rules_from_settings_json(&raw, source) {
                        Ok(mut r) => rules.append(&mut r),
                        Err(e) => tracing::warn!(
                            error = %e,
                            path = %path.display(),
                            "skipping malformed settings permissions"
                        ),
                    }
                    if let Some(m) = permission::default_mode_from_settings_json(&raw) {
                        mode = m; // local settings read last → its defaultMode wins
                    }
                    if permission::bypass_permissions_disabled_from_settings_json(&raw) {
                        bypass_disabled = true; // sticky: any tier disabling wins
                    }
                    raw_tiers.push(raw); // ascending priority preserved for sandbox derivation
                }
            }
            let rule_count = rules.len();
            // Phase 3a: supply the filesystem roots so file-path CONTENT rules
            // (`Edit(src/**)`, `Read(./secrets/**)`) match the input path. Roots
            // resolve per rule source — user settings against `claude_home`,
            // project/local against `cwd` — exactly as claude-code's
            // `rootPathForSource` does.
            let roots = permission::FsRoots {
                cwd: cwd.clone(),
                home: dirs::home_dir(),
                claude_home: cfg.claude_home.clone(),
            };
            // Phase 3a-bash: attach the sandbox-auto-allow config derived from
            // the SAME settings tiers, so a sandboxable bash command that
            // matched no explicit deny/ask rule is auto-allowed (the sandbox is
            // the safety boundary). Faithful to claude-code's
            // `bashToolHasPermission` sandbox branch; a no-op when sandboxing is
            // disabled in settings (`enabled = false`). OUTSIDE enforce mode this
            // whole block is skipped, so the layer stays a permanent no-op there.
            let raw_tier_refs: Vec<&str> = raw_tiers.iter().map(String::as_str).collect();
            let sandbox_auto_allow = sandbox_auto_allow_from_settings_tiers(&raw_tier_refs);
            // CLI-resolved mode is the highest-priority source (TS orderedModes:
            // the CLI flag / --permission-mode outranks the settings defaultMode).
            // Apply it only when the CLI actually requested a non-default mode, so
            // an unset CLI keeps the settings defaultMode computed above.
            if cfg.permission_mode != permission::PermissionMode::Default {
                mode = cfg.permission_mode;
            }
            let mut policy = permission::PermissionPolicy::from_rules(mode, rules)
                .with_roots(roots)
                .with_sandbox_runtime(sandbox_auto_allow);
            policy.bypass_killswitch_active = bypass_disabled;
            let policy = Arc::new(policy);
            tracing::info!(
                rules = rule_count,
                mode = ?mode,
                "permission enforcement enabled (LINGXI_ENFORCE_PERMISSIONS)"
            );
            Arc::new(permission::PolicyPermissionGate::new(policy, perms))
        } else {
            perms
        };

    // (5.25) M5-13: build the real hook executor now that `hook_registry`
    //        exists. This replaces the `noop_hook_executor()` stub (which fed
    //        `UnusedHttp` + `UnusedRuntime` and a `(None, None)` Command guard):
    //        - `http.clone()` is the real `PosixHttp`, so the HTTP arm performs
    //          real (SSRF-guarded) requests.
    //        - `PosixRuntime` is the real `RuntimeSpawner`.
    //        - `with_process_runner(PosixProcess, PosixSandbox)` makes the
    //          Command arm spawn real child processes (the runner only accepts a
    //          `SandboxedCommand`, which the sandbox mints).
    //        The hooks Agent arm stays "not wired" (no `.with_agent_spawner(..)`
    //        builder on `HookExecutorImpl` yet — M9+). NOTE: this is a *separate*
    //        seam from the tool-context `subagent_spawner`, which IS now wired
    //        below (4.6) — the hooks Agent arm and the `AgentTool` spawner are
    //        distinct injection points. The orchestrator's `hooks` param is the
    //        concrete `Arc<hooks::HookExecutorImpl>`, so no trait-object coercion
    //        is needed.
    //        The Prompt arm is wired via `with_prompt_runner`: the
    //        `ApiClientHookPromptRunner` reuses the SAME `api_client`
    //        (`OrchestratorApiClient::messages_create`) the orchestrator uses
    //        for its other one-shot LLM passes, so a `prompt` hook
    //        (`execPromptHook.ts`) runs an inline single-turn query through the
    //        shared provider/routing/telemetry plumbing. Decoupled: the hooks
    //        crate only sees the `HookPromptRunner` trait, never the api-client.
    // (5.255) B5 async hook registry. A matched hook with `blocking == false`
    //         (the config-`async` analog — claude-code `hooks.ts:995-1030`
    //         `executeInBackground`) is handed to this registry instead of being
    //         awaited inline, so the originating turn proceeds IMMEDIATELY rather
    //         than blocking on a slow non-blocking hook. The registry tracks each
    //         in-flight handle (so it can be cancelled/joined), races it against
    //         its `asyncTimeout` (default 15s), and publishes the eventual
    //         `(HookId, HookResult)` on `async_hook_completion_tx`. Without this
    //         wiring `HookExecutorImpl::background_hook` degrades to running the
    //         hook inline-and-discard — which still can't `Block`, but DOES block
    //         the turn — so attaching it here is what realizes the async behavior.
    //
    //         The SAME `Arc<PosixRuntime>` backs both the executor's
    //         `RuntimeSpawner` and the registry's spawner, so backgrounded hooks
    //         run on the one process runtime. The completion channel is drained by
    //         a best-effort background loop (below) — the full claude-code
    //         `getAsyncHookResponseAttachments` fold-back (re-injecting completed
    //         async-hook stdout as `async_hook_response` attachments into the next
    //         turn) is a separate, larger feature and is NOT part of this seam; the
    //         drain keeps the bounded channel from back-pressuring a fire-and-forget
    //         hook. When no hooks are configured nothing is ever backgrounded, so
    //         this wiring is a no-op for the common case (byte-identical).
    let hook_runtime = Arc::new(PosixRuntime::new());
    let (async_hook_completion_tx, mut async_hook_completion_rx) =
        tokio::sync::mpsc::channel::<(protocol::HookId, hooks::HookResult)>(64);
    let async_hook_registry = Arc::new(hooks::AsyncHookRegistry::new(
        hook_runtime.clone() as Arc<dyn traits::RuntimeSpawner>,
        async_hook_completion_tx,
    ));
    // Best-effort drain so a completed background hook never back-pressures the
    // channel (mirrors the cost-persist drain idiom above). Fire-and-forget:
    // the result is observed only for in-flight bookkeeping, which the registry
    // already cleared before publishing.
    tokio::spawn(async move { while async_hook_completion_rx.recv().await.is_some() {} });
    let hooks = Arc::new(
        hooks::HookExecutorImpl::new(
            hook_registry.clone(),
            http.clone(),
            hook_runtime as Arc<dyn traits::RuntimeSpawner>,
        )
        .with_process_runner(
            Arc::new(PosixProcess::new()) as Arc<dyn traits::ProcessRunner>,
            Arc::new(PosixSandbox::new()) as Arc<dyn traits::Sandbox>,
        )
        .with_prompt_runner(Arc::new(orchestrator::ApiClientHookPromptRunner::new(
            api_client.clone(),
        )))
        .with_async_registry(async_hook_registry),
    );

    // (5.26) Build the MCP registry NOW (deferred from (5.1)) so it can carry
    //         the elicitation hook dispatcher, then auto-connect. The
    //         `OrchestratorHookDispatcher` shares the SAME `hooks` executor, so
    //         an inbound `elicitation/create` fires the `Elicitation` hook
    //         (claude-code `runElicitationHooks`): a hook may PROVIDE the answer
    //         or DENY it; with no hook it falls through to `{"action":"cancel"}`.
    //         `with_hook_dispatcher(Some(..))` is the only behavioral delta from
    //         the previous `with_raw_conn` wiring.
    let elicitation_dispatcher: Arc<dyn mcp::HookDispatcher> =
        Arc::new(orchestrator::OrchestratorHookDispatcher::new(
            hooks.clone(),
            cwd.clone(),
        ));
    let mcp_registry = Arc::new(
        mcp::McpRegistry::with_raw_conn(
            posix.clone() as Arc<dyn McpTransport>,
            posix as Arc<dyn mcp::RawConnectionProvider>,
        )
        .with_hook_dispatcher(Some(elicitation_dispatcher)),
    );
    mcp_registry.connect_all(mcp_configs).await;
    tokio::spawn(Arc::clone(&mcp_registry).run_reconnect_loop());

    // (5.3) Agent catalog — load from project + user agents/. Project wins on
    //       collision (passed SECOND; later paths win). The user agents dir is
    //       `cfg.claude_home/agents` (was `dirs::home_dir()/.claude/agents`).
    let project_agents_dir = cwd.join(".claude").join("agents");
    let user_agents_dir = cfg.claude_home.join("agents");
    let agents = agent::load_agents_from_dirs(&[
        (user_agents_dir, agent::definition::AgentSource::UserDefined),
        (project_agents_dir, agent::definition::AgentSource::Project),
    ])
    .await;
    let agent_catalog = Arc::new(tokio::sync::RwLock::new(agents));

    // (5.4) Real compaction. Threshold 150_000 tokens (M3 design lock for the
    //       Anthropic prod context window). In-Loop Compaction Batch 6: back the
    //       autocompact layer with a REAL forked summary call (sharing the
    //       parent's prompt cache) instead of the deterministic-fallback
    //       summarizer. The same `cache_safe_slot` is handed to BOTH the
    //       summarizer (here) and the orchestrator (`with_cache_safe_slot`
    //       below), so the turn loop's per-call snapshot is what the summary
    //       call replays.
    let cache_safe_slot = Arc::new(sidequery::CacheSafeParamsSlot::new());
    let side_query_client: Arc<dyn sidequery::SideQueryClient> =
        Arc::new(sidequery::ProviderSideQueryClient::new(
            cfg.api_key.clone(),
            Some(cfg.api_base.clone()),
            http.clone() as Arc<dyn traits::HttpTransport>,
        ));
    let forked_runner = Arc::new(
        sidequery::ForkedAgentRunner::new(Arc::new(sidequery::NoopSubagentSlotProvider))
            .with_side_query_client(side_query_client.clone(), orch_cfg.model.clone()),
    );
    let autocompactor =
        compaction::Autocompactor::with_forked_runner(forked_runner, cache_safe_slot.clone());
    let compactor = Arc::new(compaction::CompactionOrchestrator::with_autocompactor(
        autocompactor,
        150_000,
    ));

    // (5.45) The real desktop `TaskRegistry`, wired into the tool context. Tasks
    //        materialize stdout/stderr under `<cwd>/.claude/tasks-output`; the
    //        spawner is the tokio-backed `PosixRuntime`. The same handle is
    //        returned for a transport/TUI poller to read live state.
    let task_output_dir = cwd.join(".claude").join("tasks-output");
    let mut task_registry_inner = tasks::registry::TaskRegistry::new(
        Arc::new(PosixRuntime::new()),
        Arc::new(PosixFileSystem::new(cwd.clone())),
        Arc::new(tasks::output_manager::TaskOutputManager::new(
            task_output_dir,
            Arc::new(PosixFileSystem::new(cwd.clone())),
        )),
    )
    // Fire the `TaskCompleted` hook (claude-code `executeTaskCompletedHooks`)
    // when a task reaches a terminal status. The firer wraps the SAME
    // `Arc<HookExecutorImpl>` the orchestrator fires its other hooks through, so
    // the `tasks` leaf reaches `orch.hooks` without a dependency cycle.
    .with_task_completed_firer(Arc::new(
        orchestrator::OrchestratorTaskCompletedFirer::new(hooks.clone(), cwd.clone()),
    ))
    // Fire the `TaskCreated` hook (claude-code `executeTaskCreatedHooks`) when a
    // task is created. Counterpart to the `TaskCompleted` firer above — wraps
    // the SAME `Arc<HookExecutorImpl>` so the `tasks` leaf reaches `orch.hooks`
    // without a dependency cycle.
    .with_task_created_firer(Arc::new(
        orchestrator::OrchestratorTaskCreatedFirer::new(hooks.clone(), cwd.clone()),
    ));
    // Register the M2 self-contained per-type handlers (LocalBash + MonitorMcp)
    // before the registry is shared. Both depend only on platform traits we
    // already build here; agent/teammate/workflow/remote/dream handlers register
    // once their production pools are wired (M9+).
    tasks::registry::register_self_contained_handlers(
        &mut task_registry_inner,
        Arc::new(PosixProcess::new()),
        Arc::new(PosixSandbox::new()),
        mcp_registry.clone(),
    );

    // (5.46) M10 (T13): construct the per-session coordinator subsystem — one
    //        `TeamRegistry` + one `CoordinatorMode` per `build()`. The registry
    //        is observable (the status feed / PHASE-2 command router read it)
    //        but stays empty unless a coordinator session spawns teammates. The
    //        mode is entered at BUILD time ONLY when the session was started as a
    //        coordinator (the registry is built-once-and-moved, so mode-exclusive
    //        tool selection must be decided here); a default session leaves it
    //        DISABLED so the build is byte-identical to the pre-M10 build.
    let coordinator_id = protocol::AgentId::new();
    let coordinator = Arc::new(coordinator::TeamRegistry::new(coordinator_id));
    let coordinator_mode = {
        let mut mode = coordinator::CoordinatorMode::new();
        mode.session_started_as_coordinator = cfg.session_started_as_coordinator;
        if cfg.session_started_as_coordinator {
            mode.enter();
        }
        Arc::new(mode)
    };

    // (5.46a) M10 (T13): register the `InProcessTeammate` handler DIRECTLY (not
    //        via `register_agent_handlers`) so the coordinator's
    //        `CoordinatorStatusSink` is attached — that sink maps the teammate's
    //        `TaskStatus` transitions onto `WorkerStatus` AND pushes the live
    //        `active_workers` scalar to the orchestrator-facing `OutputStream`.
    //        Registration takes `&mut self`, so it MUST happen before the
    //        registry is `Arc`-wrapped below.
    //
    //        Teammates run in their OWN `StateMachinePool` (`TEAMMATE_POOL_CAP`),
    //        separate from the `AgentTool` `subagent_pool`: persistent teammates
    //        park on `wait_for_message` and never free their slot, so a shared
    //        pool would risk starving one-shot subagent spawns (T14 regression).
    //
    //        The handler's tool-dispatch seam is a `DeferredToolInvoker`: the
    //        teammate must inherit the parent's `Arc<ToolRegistry>` (recursion
    //        lock), but that registry is assembled AFTER this point (its
    //        `BuiltinToolContext` carries `task_registry.clone()`). The deferred
    //        invoker is injected now and bound to the real `RegistryToolInvoker`
    //        once `tools` exists (5.5a). Definition resolution relies on the
    //        handler default `DefaultTeammateDefinition` (permissive) — we do NOT
    //        attach a catalog-backed resolver, which would return `None` for the
    //        team-lead name and silently fail every spawn.
    let coordinator_sink = Arc::new(coordinator::CoordinatorStatusSink::new(
        coordinator.clone(),
        output.clone(),
    ));
    let teammate_invoker = Arc::new(DeferredToolInvoker::new());
    let teammate_pool = Arc::new(agent::StateMachinePool::new(
        Arc::new(PosixRuntime::new()),
        TEAMMATE_POOL_CAP,
    ));
    let teammate_handler = tasks::handlers::InProcessTeammateHandler::new(
        teammate_pool,
        task_registry_inner.output_manager.clone(),
        teammate_api,
    )
    .with_tool_invoker(
        teammate_invoker.clone() as Arc<dyn traits::tool_invoker::ToolInvoker>
    )
    // Anchor the teammate's `AgentModel::Inherit` / family aliases to the parent
    // model — the same seam the `PoolSubagentSpawner` gets above — so a spawned
    // teammate runs against a concrete wire id instead of passing `"inherit"` raw.
    .with_default_model(orch_cfg.model.clone())
    .with_status_sink(coordinator_sink as Arc<dyn tasks::handlers::TaskStatusSink>)
    // Fire the `TeammateIdle` hook (claude-code `executeTeammateIdleHooks`,
    // `stopHooks.ts:403`) each time a teammate finishes a turn-set and parks
    // awaiting the next message ("about to go idle"). The firer wraps the SAME
    // `Arc<HookExecutorImpl>` the orchestrator fires its other hooks through, so
    // the `tasks` leaf reaches `orch.hooks` without a dependency cycle —
    // mirroring the `TaskCompleted` / `TaskCreated` firers above.
    .with_teammate_idle_firer(Arc::new(
        orchestrator::OrchestratorTeammateIdleFirer::new(hooks.clone(), cwd.clone()),
    ));
    task_registry_inner
        .register_handler(tasks::TaskType::InProcessTeammate, Arc::new(teammate_handler));

    let task_registry = Arc::new(task_registry_inner);

    // (5.47) M10 (T13): the typed spawn/kill seam the coordinator's `TeamCreate` /
    //        `TeamDelete` use to start / stop the real backing `InProcessTeammate`
    //        task. `TaskRegistry` impls `TeamSpawnSeam` (T04); the same `Arc` the
    //        tool context holds is reused so the spawned teammate is keyed on the
    //        worker identity threaded through.
    let spawn_seam: Arc<dyn traits::team_spawn::TeamSpawnSeam> = task_registry.clone();

    // (5.5) Assemble the desktop tool registry through the composition root.
    //       A coordinator session shares the team's `MailboxRouter` with the
    //       builtin `SendMessage` tool by casting it onto `tool_ctx.mailbox_router`
    //       (the trait impl lives on `MailboxRouter`); a default session leaves it
    //       `None` — byte-identical to the pre-M10 build.
    let coordinator_mailbox: Option<Arc<dyn traits::mailbox::MailboxRouterHandle>> =
        if cfg.session_started_as_coordinator {
            Some(coordinator.mailbox_router.clone() as Arc<dyn traits::mailbox::MailboxRouterHandle>)
        } else {
            None
        };
    let tool_ctx = BuiltinToolContext {
        // FILE.B: file tools share one read-state map for the (future) staleness
        // guard / Read-dedup; the composition-root Arc-share with the orchestrator
        // is wired when a consumer (FILE.A/D/E/F) reads it.
        read_file_state: tool_api::read_file_state::new_read_file_state_map(),
        fs: Arc::new(PosixFileSystem::new(cwd.clone())),
        bus: Arc::new(telemetry::AnalyticsBus::new()),
        trusted_dirs: vec![cwd.clone()],
        process: Arc::new(PosixProcess::new()),
        sandbox: Arc::new(PosixSandbox::new()),
        clock: clock.clone(),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        permission_mode: cfg.permission_mode,
        project_trust: ProjectTrustLevel::Trusted,
        sandbox_available: false,
        workspace: cwd.clone(),
        platform: if cfg!(target_os = "macos") {
            SandboxPlatform::Mac
        } else {
            SandboxPlatform::Linux
        },
        http: http.clone(),
        provider: tool_provider,
        default_model: orch_cfg.model.clone(),
        worktree: Arc::new(PosixWorktree::new()),
        subagent_spawner: Some(subagent_spawner),
        task_registry: Some(
            task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>
        ),
        mailbox_router: coordinator_mailbox,
        budget_enforcer: Some(budget_enforcer),
        // (3b) AgentTool threads this into the subagent's RegistryToolInvoker so
        // spawned subagents are gated by the same boot gate as the main loop.
        permission_gate: Some(perms.clone()),
        mcp_registry: Some(mcp_registry.clone()),
        lsp_registry: None,
        camera: None,
        voice: None,
        stt: None,
        tts: None,
        share: None,
        notifications: None,
        clipboard: None,
        computer_control: None,
    };
    // (5.5) M10 (T12/T13): select the team-tool variant at BUILD time. A
    //        coordinator session passes `Some(CoordinatorWiring { team, mode,
    //        spawn_seam })` so the coordinator `TeamCreate` / `TeamDelete` are
    //        registered IN PLACE OF `tool_team`'s pair; a default session passes
    //        `None`, leaving `tool_team`'s pair and the coordinator tools absent
    //        — byte-identical to the pre-M10 build.
    let coordinator_wiring = if cfg.session_started_as_coordinator {
        Some(CoordinatorWiring {
            team: coordinator.clone(),
            mode: coordinator_mode.clone(),
            spawn_seam: spawn_seam.clone(),
            // The SAME output stream the orchestrator + CoordinatorStatusSink use,
            // so the TeamCreate activation PUSH and the sink's later transitions
            // share one client feed. Cloned here because `output` is moved into
            // the `ConversationOrchestrator` below.
            output: output.clone(),
        })
    } else {
        // Drop the spawn-seam clone path; it is unused in a default session.
        let _ = &spawn_seam;
        None
    };
    // (5.5b) MCP-invocation Batch 3: expose each Connected server's tools by
    //        their real `mcp__<server>__<tool>` FQN as individual wire entries
    //        (server `inputSchema` + truncated description), routed back to that
    //        connection's `McpClient::call_tool`. Mirrors claude-code's
    //        `fetchToolsForClient` per-tool `Tool` (services/mcp/client.ts:1766-1990).
    //
    //        SEQUENCING: this MUST run BEFORE the registry is sealed into its
    //        final `Arc` (registration is `&mut self`), and the MCP servers are
    //        ALREADY connected at this point — `mcp_registry.connect_all` ran at
    //        (5.1). So we build the registry mutably, register the per-connection
    //        MCP partitions, then `Arc`-wrap. `tool_ctx` is consumed by
    //        `register_desktop_tools`, so the builder gets a clone taken first.
    let mcp_tool_ctx = tool_ctx.clone();
    let mut tools_inner = ToolRegistry::new();
    // `RemoteTrigger`'s in-process OAuth resolver, backed by the credential
    // store built at (3). Reads tokens at call-time so the refresh driver wired
    // at (3.1) is always reflected.
    let cron_auth: Arc<dyn tool_cron::ClaudeAiAuthProvider> =
        Arc::new(CredentialStoreAuthProvider {
            credentials: credentials.clone(),
            base_api_url: cfg.api_base.clone(),
        });
    // SKILLEXEC.2: break the Skill-tool ↔ CommandRegistry init cycle. The tool
    // registry (which holds the `Skill` tool) must exist before the orchestrator,
    // and the command registry needs the orchestrator handle — so build the
    // shared command-registry slot now (empty), hand a `CommandRegistry`-backed
    // loader to the `Skill` tool, then FILL the same `Arc` with the real registry
    // at (6) and reuse it for the slash dispatcher. No skill call can fire before
    // `build()` returns, so the loader never reads the empty registry.
    let shared_command_registry: Arc<RwLock<CommandRegistry>> =
        Arc::new(RwLock::new(CommandRegistry::new()));
    // SKILLEXEC: the per-session id stamped onto every resolved skill descriptor
    // so the `Skill` tool substitutes `${CLAUDE_SESSION_ID}` in the body (TS
    // `getSessionId()`, a per-process session value). Generated once here at build
    // time; format mirrors the engine's `SessionId` Display (`sess:<uuid>`).
    let skill_session_id = protocol::SessionId::new().to_string();
    let skill_loader: Arc<dyn tool_skill::skill::SkillLoader> =
        Arc::new(skill_loader::CommandRegistrySkillLoader::with_session_id(
            shared_command_registry.clone(),
            skill_session_id,
        ));
    // Fire the `CwdChanged` hook (claude-code `onCwdChangedForHooks`,
    // Shell.ts:409) when a `cd` inside a Bash call moves the persistent shell
    // cwd. The firer wraps the SAME `Arc<HookExecutorImpl>` the orchestrator
    // fires its other hooks through (mirrors the `TaskCreated` / `TaskCompleted`
    // firers), so the `tool-shell` leaf reaches `orch.hooks` without a dependency
    // cycle. Injected here (the desktop composition root) only — `engine-mobile`
    // never registers the shell tools, so the mobile path keeps the no-firer
    // BashTool.
    let cwd_changed_firer: hooks::OptionalCwdChangedFirer = Some(Arc::new(
        orchestrator::OrchestratorCwdChangedFirer::new(hooks.clone(), cwd.clone()),
    ));
    register_desktop_tools(
        &mut tools_inner,
        tool_ctx,
        coordinator_wiring,
        Some(cron_auth),
        Some(skill_loader),
        cwd_changed_firer,
        Some(side_query_client.clone()),
    );
    for (conn_id, mcp_tools) in
        tool_mcp::build_registered_mcp_tools(&mcp_registry, mcp_tool_ctx).await
    {
        tools_inner.register_mcp_tools(conn_id, mcp_tools);
    }
    let tools = Arc::new(tools_inner);

    // (5.5a) M10 (T13): bind the teammate handler's `DeferredToolInvoker` to the
    //        real `RegistryToolInvoker` now that `tools` exists. The invoker
    //        stores the SAME `Arc<ToolRegistry>` the orchestrator owns, so a
    //        teammate's tool dispatch reuses the parent registry (recursion-lock
    //        invariant). No teammate can dispatch before `build()` returns, so
    //        the cell is always filled before first use.
    teammate_invoker.set(Arc::new(
        tool_api::tool_invoker_impl::RegistryToolInvoker::new(tools.clone())
            // (3b) Gate teammate tool dispatch with the same boot gate as the
            // main loop + subagents. Default-off `perms` is the no-op gate, so
            // this is behavior-neutral unless LINGXI_ENFORCE_PERMISSIONS is set.
            .with_gate(perms.clone()),
    ));

    // Break the subagent construction cycle now that `tools` + `agent_catalog`
    // exist: fill the spawner's set-once cells (filled before `build()` returns,
    // so before any spawn). Each spawn then resolves its advertised tools +
    // dispatch allow-list from this registry at spawn time (parity batch 20) and
    // real user/project `AgentDefinition`s from the SAME catalog `Arc` the
    // orchestrator holds — `.with_agent_catalog` below shares the lock, not a
    // copy (parity batch 21). First fill wins.
    let _ = subagent_tool_registry_cell.set(tools.clone());
    let _ = subagent_agent_catalog_cell.set(agent_catalog.clone());

    // Clone `cwd` for the settings watcher before it is moved into the
    // orchestrator constructor below.
    let watch_cwd = cwd.clone();
    // Decide whether to spawn the settings watcher (7.2) BEFORE `hook_registry`
    // is moved into the orchestrator: spawn only when a `ConfigChange` hook is
    // registered (the fire is a strict no-op otherwise, so the background
    // watcher would be pure overhead).
    // Snapshot under ONE registry read: both the `ConfigChange` gate and the
    // `FileChanged` watch-path matchers (collected before `hook_registry` is
    // moved into the orchestrator). A `FileChanged` hook's group `matcher`
    // (`HookDefinition::matcher()`) is the pipe-separated filename list
    // claude-code's `resolveWatchPaths` reads (`fileChangedWatcher.ts:48-65`).
    let (has_config_change_hook, file_changed_matchers): (bool, Vec<String>) = {
        let reg = hook_registry.read().await;
        let all = reg.all_hooks();
        let has_config_change = all
            .iter()
            .any(|h| h.events.contains(&hooks::events::HookEventType::ConfigChange));
        let matchers = all
            .iter()
            .filter(|h| {
                h.events
                    .contains(&hooks::events::HookEventType::FileChanged)
            })
            .filter_map(|h| h.matcher().map(ToString::to_string))
            .collect();
        (has_config_change, matchers)
    };
    // Build the `FileChanged` firer over the SAME `Arc<HookExecutorImpl>` the
    // orchestrator is about to take ownership of (mirrors the `cwd_changed_firer`
    // built from `hooks.clone()` at (5.5)). Captured BEFORE `hooks` is moved into
    // the orchestrator constructor below so the watcher (spawned at (7.3), after
    // `hooks` is moved) reaches `orch.hooks` without a getter. Only built when at
    // least one `FileChanged` matcher exists — otherwise it would be unused.
    let file_changed_firer: Option<Arc<dyn hooks::FileChangedFirer>> =
        if file_changed_matchers.is_empty() {
            None
        } else {
            Some(Arc::new(orchestrator::OrchestratorFileChangedFirer::new(
                hooks.clone(),
                watch_cwd.clone(),
            )))
        };
    let orch = Arc::new(
        ConversationOrchestrator::new(
            orch_cfg, api_client, tools, hooks, perms, output, memory, cwd,
        )
        .with_cost_tracker(cost_tracker)
        .with_mcp_registry(mcp_registry)
        .with_hook_registry(hook_registry)
        .with_agent_catalog(agent_catalog)
        .with_compaction(compactor)
        .with_cache_safe_slot(cache_safe_slot),
    );

    // (6) Command registry through the desktop composition root.
    let handle: Arc<dyn OrchestratorHandle> = orch.clone();
    let reg = desktop_command_registry(handle, auth.clone(), &cfg.cwd, &cfg.claude_home).await;
    // SKILLEXEC.2: fill the shared command-registry slot the `Skill` tool's
    // loader holds, then hand the SAME `Arc` to the slash dispatcher so the tool
    // and the dispatcher observe one command set (plugin lifecycle mutations via
    // the dispatcher's write lock are visible to the loader too).
    *shared_command_registry.write().await = reg;
    let dispatcher = RegistrySlashDispatcher::new(shared_command_registry.clone());

    // (7) Session lifecycle: fire the `SessionStart` hooks now that the
    //     orchestrator + hook registry are fully wired. claude-code fires the
    //     `SessionStart` hook event at session startup (`utils/hooks.ts:3876-3881`,
    //     the SessionStart path) with `source` = one of
    //     `startup` / `resume` / `clear` / `compact`. The desktop composition root
    //     OWNS the session lifecycle (it constructs the orchestrator), and `build`
    //     assembles exactly one fresh session per call, so the byte-faithful
    //     `source` here is `"startup"`. Best-effort: `fire_session_start` discards
    //     the hook aggregate, so a failing or malformed `SessionStart` hook never
    //     breaks boot, and it is a strict no-op when no `SessionStart` hook is
    //     registered (the common case). NOTE: there is no engine-desktop-local
    //     teardown seam — `build` returns the runtime and the host (`apps/cli` /
    //     the bridge-server) drops it on process exit with no hook-capable
    //     shutdown path — so the matching `SessionEnd` is NOT fired here. The
    //     `ConversationOrchestrator::fire_session_end` helper exists for a future
    //     batch that adds an explicit host teardown seam.
    orch.fire_session_start("startup").await;

    // (7.1) Instruction-load lifecycle: fire the `InstructionsLoaded` hooks now
    //       that memory + the hook registry are wired. claude-code fires this
    //       fire-and-forget hook once per CLAUDE.md / `CLAUDE.local.md` spliced
    //       into context by the eager session-start `getMemoryFiles` pass
    //       (`utils/claudemd.ts:1054-1071`, `utils/hooks.ts:4335-4369`), each
    //       carrying the file's `file_path` / `memory_type` / `load_reason`
    //       (`session_start` for top-level files). The orchestrator owns the
    //       memory provider, so it loads memory once and fires from that single
    //       point. Best-effort: `fire_instructions_loaded` discards each hook
    //       aggregate, so a failing/malformed `InstructionsLoaded` hook never
    //       breaks boot, and it is a strict no-op when none is registered (the
    //       common case) or when no instruction files are present.
    orch.fire_instructions_loaded().await;

    // (7.2) ConfigChange lifecycle: start the settings watcher now that the
    //       orchestrator + hook registry are wired. claude-code watches the
    //       user / project / local / policy settings files and, on every
    //       detected change, fires the `ConfigChange` hook with the layer
    //       `source` + changed `file_path` BEFORE applying the change
    //       (`changeDetector.ts:285-297` → `executeConfigChangeHooks`,
    //       `utils/hooks.ts:4214`). The Rust port had no watcher; this wires it
    //       at the composition root via the in-tree `notify`-backed
    //       `FileSystem::watch` primitive (`platform-posix`'s `watch_helper`).
    //       SCOPE is firing the hook only — the live settings RELOAD/re-apply
    //       (claude-code's `fanOut`) is a separate concern, intentionally not
    //       done here. Best-effort: the watcher fires `fire_config_change`,
    //       which discards the aggregate (a failing/blocking `ConfigChange`
    //       hook never breaks the watch loop) and is a strict no-op when no
    //       `ConfigChange` hook is registered. The handle is returned on the
    //       runtime so it lives for the session; dropping the runtime aborts
    //       the watch tasks (RAII), releasing the OS handles cleanly.
    //
    //       The full `platform-posix` `FileSystem` is used here (NOT the
    //       `posix-minimal` one wired into the engine) because only it has the
    //       real `notify`-backed `watch`; `posix-minimal::watch` is an
    //       empty-stream stub, so wiring it would observe no events.
    //
    //       GATED (decided above, before `hook_registry` moved into the
    //       orchestrator): only spawn the watcher when at least one
    //       `ConfigChange` hook is registered. The fire is a strict no-op
    //       otherwise, so the background `notify` watcher (and its blocking pump
    //       thread) would be pure overhead in the common no-hook case — gating
    //       keeps boot cheap and avoids holding an OS watch handle nobody
    //       consumes.
    let settings_watcher = if has_config_change_hook {
        let watch_fs: Arc<dyn traits::FileSystem> =
            Arc::new(platform_posix::PosixFileSystem::new(watch_cwd.clone()));
        let firer: Arc<dyn settings_watch::ConfigChangeFirer> = orch.clone();
        settings_watch::SettingsWatcher::new(&cfg.claude_home, &watch_cwd, firer)
            .spawn(watch_fs)
            .await
    } else {
        settings_watch::SettingsWatcherHandle::empty()
    };

    // (7.3) FileChanged lifecycle: start the file-changed watcher now that the
    //       orchestrator + hook registry are wired. claude-code resolves a set
    //       of watch paths from the user's `FileChanged` hook config (each
    //       hook's `matcher` is a pipe-separated filename list,
    //       `fileChangedWatcher.ts:48-65`), watches them, and on every debounced
    //       `change` / `add` / `unlink` fires the `FileChanged` hook with the
    //       path + chokidar event name (`handleFileEvent` →
    //       `executeFileChangedHooks`, `utils/hooks.ts:4278`). The Rust port had
    //       no watcher; this wires it at the composition root via the in-tree
    //       `notify`-backed `FileSystem::watch` primitive (`platform-posix`'s
    //       `watch_helper`), exactly as the settings watcher (7.2) does.
    //
    //       The firer is the `OrchestratorFileChangedFirer` over the SAME
    //       `Arc<HookExecutorImpl>` the orchestrator fires its other hooks
    //       through (mirrors the `CwdChanged` / task firers), so the watcher
    //       reaches `orch.hooks` without a dependency cycle. Best-effort: a
    //       failing/blocking `FileChanged` hook never breaks the watch loop.
    //
    //       GATED: spawn ONLY when at least one `FileChanged` hook is registered
    //       AND it resolves to a non-empty watch-path set (a matcher-less hook
    //       watches nothing — claude-code's `if (paths.length === 0) return`).
    //       With no `FileChanged` hook the matcher list is empty, the watcher
    //       resolves to zero paths, and the empty handle is returned — the
    //       no-watch case is byte-identical to before. As with the settings
    //       watcher, the full `platform-posix` `FileSystem` is used (the engine's
    //       `posix-minimal::watch` is an empty-stream stub).
    let file_changed_watcher = match file_changed_firer {
        None => file_changed_watch::FileChangedWatcherHandle::empty(),
        Some(firer) => {
            let matcher_refs: Vec<&str> =
                file_changed_matchers.iter().map(String::as_str).collect();
            let watcher = file_changed_watch::FileChangedWatcher::new(
                &matcher_refs,
                &watch_cwd,
                firer,
            );
            // Empty resolved-path set (matcher-less hooks only) ⇒ spawn returns
            // an empty handle, so this stays a no-op even when a `FileChanged`
            // hook is present but specifies no watch target.
            if watcher.watch_paths().is_empty() {
                file_changed_watch::FileChangedWatcherHandle::empty()
            } else {
                let watch_fs: Arc<dyn traits::FileSystem> =
                    Arc::new(platform_posix::PosixFileSystem::new(watch_cwd.clone()));
                watcher.spawn(watch_fs).await
            }
        }
    };

    Ok(DesktopRuntime {
        orchestrator: orch,
        dispatcher,
        auth,
        task_registry,
        coordinator,
        coordinator_mode,
        permission_gate: adapter_gate,
        settings_watcher,
        file_changed_watcher,
    })
}

#[cfg(test)]
mod tests {
    use super::{build, desktop_tool_registry, CoordinatorWiring, DesktopConfig};
    use std::sync::Arc;

    /// F2-00: the deliverable-zero config is constructible from `Default` and
    /// its frozen field set is reachable. The actual `build()` lift is F2-01;
    /// here we only prove the type compiles and the defaults are sane.
    #[test]
    fn desktop_config_default_is_constructible() {
        let cfg = DesktopConfig::default();

        assert_eq!(cfg.api_base, "https://api.anthropic.com");
        assert!(cfg.api_key.is_empty());
        assert_eq!(cfg.cwd, std::path::PathBuf::from("."));
        assert_eq!(cfg.claude_home, std::path::PathBuf::new());
        // Mirrors `DesktopEngineConfig::default().default_model`.
        assert_eq!(cfg.default_model, "claude-sonnet-4-20250514");
        // Opus-fallback default: no fallback model unless argv supplies one.
        assert!(cfg.fallback_model.is_none());
        assert!(cfg.provider_profiles.is_none());
        assert!(cfg.routing.is_none());
        assert!(cfg.mcp_paths.is_empty());
        // CLI default — opt into `NoOpPermissionGate`.
        assert!(cfg.use_noop_permission_gate);
        // The CLI-resolved permission mode defaults to `Default` (no override).
        assert_eq!(cfg.permission_mode, permission::PermissionMode::Default);

        // Frozen field set is fully reachable via struct-update syntax, and the
        // type derives `Clone`/`Debug` so a host can fan it out + log it.
        let custom = DesktopConfig {
            use_noop_permission_gate: false,
            ..cfg.clone()
        };
        assert!(!custom.use_noop_permission_gate);
        let _ = format!("{custom:?}");
    }

    /// The CLI-resolved `permission_mode` threads from `DesktopConfig` into the
    /// `BuiltinToolContext` and (under enforcement) into the policy mode.
    ///
    /// Two assertions, both via the lightest available seams:
    ///   1. The field is reachable + struct-updatable on `DesktopConfig` to the
    ///      non-default `BypassPermissions` value; `build()` then copies it into
    ///      `BuiltinToolContext.permission_mode` (`permission_mode: cfg.permission_mode`).
    ///   2. The policy the enforce block builds from that mode is allow-all for an
    ///      unmatched tool, and with the bypass killswitch active falls back to Ask
    ///      — i.e. exactly what `PermissionPolicy::from_rules(mode, rules)` yields
    ///      for the overridden `mode = cfg.permission_mode`.
    ///
    /// The fuller end-to-end assertion (drive a real `build()` under
    /// `LINGXI_ENFORCE_PERMISSIONS` and probe the wrapped `PolicyPermissionGate`)
    /// is DEFERRED: it requires mutating process env behind a shared `Mutex`,
    /// writing settings tiers to disk, and a live orchestrator/provider — heavier
    /// than the engine-desktop unit-test patterns warrant. The override is a
    /// one-line conditional over `cfg.permission_mode`; the policy semantics it
    /// relies on are asserted directly here against the same constructor the
    /// enforce block calls.
    #[test]
    fn permission_mode_threads_into_context_and_policy() {
        use permission::{PermissionMode, PermissionPolicy, PermissionResult};

        // (1) The field carries the CLI-resolved mode through struct-update —
        //     this is the value `build()` copies into the tool context.
        let cfg = DesktopConfig {
            permission_mode: PermissionMode::BypassPermissions,
            ..DesktopConfig::default()
        };
        assert_eq!(cfg.permission_mode, PermissionMode::BypassPermissions);

        // (2) Policy semantics the enforce-block override relies on. The override
        //     sets `mode = cfg.permission_mode` (non-default), then builds the
        //     policy with no rules — an unmatched tool must be allow-all.
        let policy = PermissionPolicy::from_rules(cfg.permission_mode, std::iter::empty());
        let input = serde_json::json!({});
        assert!(
            matches!(
                policy.authorize("SomeUnmatchedTool", &input),
                PermissionResult::Allow { .. }
            ),
            "BypassPermissions with no rules must allow an unmatched tool"
        );

        // With the bypass killswitch active the policy refuses bypass and falls
        // back to Ask for the same unmatched tool.
        let mut gated = PermissionPolicy::from_rules(cfg.permission_mode, std::iter::empty());
        gated.bypass_killswitch_active = true;
        assert!(
            matches!(
                gated.authorize("SomeUnmatchedTool", &input),
                PermissionResult::Ask { .. }
            ),
            "killswitch must override BypassPermissions back to Ask"
        );
    }

    #[test]
    fn oauth_subscriber_flag_gating() {
        let inference = vec!["user:inference".to_string(), "user:profile".to_string()];
        let no_inference = vec!["user:profile".to_string()];
        // Clean OAuth (no overriding env key/token) + inference scope ⇒ subscriber.
        assert!(super::oauth_subscriber_flag(false, false, &inference));
        // Inference scope present, but an env ANTHROPIC_API_KEY outranks stored
        // OAuth in the resolver ⇒ isAnthropicAuthEnabled() false ⇒ not subscriber.
        assert!(!super::oauth_subscriber_flag(true, false, &inference));
        // Likewise an env ANTHROPIC_AUTH_TOKEN bearer outranks stored OAuth.
        assert!(!super::oauth_subscriber_flag(false, true, &inference));
        // Clean OAuth but no inference scope (e.g. profile-only) ⇒ not subscriber.
        assert!(!super::oauth_subscriber_flag(false, false, &no_inference));
        // No scopes at all ⇒ not subscriber.
        assert!(!super::oauth_subscriber_flag(false, false, &[]));
    }

    /// A [`client_adapter::PermissionRequestSink`] that records the requests the
    /// gate emits, so a test can prove a turn's `check()` actually reached the
    /// adapter gate (and not the always-allow `NoOpPermissionGate`).
    #[derive(Default)]
    struct RecordingPermissionSink {
        count: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl client_adapter::PermissionRequestSink for RecordingPermissionSink {
        async fn emit_request(
            &self,
            _request: client_protocol::permission::PermissionRequest,
        ) {
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Deterministic, env/argv-free config rooted at a sandbox temp dir.
    fn test_config(use_noop: bool) -> (tempfile::TempDir, DesktopConfig) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().to_path_buf();
        let claude_home = cwd.join(".claude");
        let cfg = DesktopConfig {
            api_base: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            cwd: cwd.clone(),
            claude_home,
            default_model: "claude-sonnet-4-20250514".to_string(),
            fallback_model: None,
            provider_profiles: None,
            routing: None,
            mcp_paths: vec![cwd.join(".mcp.json")],
            use_noop_permission_gate: use_noop,
            session_started_as_coordinator: false,
            // Boot tests stay deterministic: empty memory, never the real FS.
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
        };
        (tmp, cfg)
    }

    /// F2-01: `build()` constructs a fully-wired runtime from a `DesktopConfig`
    /// alone — no `Argv`, no `std::env`. The wiring parity with the old
    /// `build_runtime` is asserted via the same `has_*` predicates the CLI
    /// regression test used (cost tracker + the three registries + compaction).
    #[tokio::test]
    async fn build_constructs_runtime_deterministically() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed");

        assert!(rt.orchestrator.has_cost_tracker(), "no CostTracker");
        assert!(rt.orchestrator.has_mcp_registry(), "no McpRegistry");
        assert!(rt.orchestrator.has_hook_registry(), "no HookRegistry");
        assert!(rt.orchestrator.has_agent_catalog(), "no agent catalog");
        assert!(rt.orchestrator.has_compaction(), "no CompactionOrchestrator");
    }

    /// F2-01: `use_noop_permission_gate: true` binds the `NoOpPermissionGate`,
    /// so no `AdapterPermissionGate` handle is surfaced for the transport to
    /// resolve against.
    #[tokio::test]
    async fn build_with_noop_gate_uses_noop() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed");

        assert!(
            rt.permission_gate.is_none(),
            "noop build must not surface an adapter gate handle"
        );
    }

    /// F2-01: `use_noop_permission_gate: false` binds the connection-scoped
    /// `AdapterPermissionGate`. The returned handle is what the transport calls
    /// `resolve()` on; a `check()` against it parks a request on the supplied
    /// sink (proving it is NOT the always-allow no-op gate).
    #[tokio::test]
    async fn build_default_uses_adapter_gate() {
        let (_tmp, cfg) = test_config(false);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let sink = Arc::new(RecordingPermissionSink::default());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> = sink.clone();

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed");

        let gate = rt
            .permission_gate
            .clone()
            .expect("adapter build must surface a gate handle");

        // Drive a `check()` on a spawned task; it parks a request on the sink
        // (deny-by-default tool) then resolve it so the future completes.
        let g = gate.clone();
        let task = tokio::spawn(async move {
            use permission::gate::PermissionGate;
            g.check("Bash", &serde_json::json!({"command": "ls"})).await
        });

        // The request must have reached the adapter sink — a `NoOpPermissionGate`
        // would have returned `Allow` without ever emitting a request.
        for _ in 0..2000 {
            if sink.count.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            sink.count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "adapter gate must emit a PermissionRequest"
        );

        // Resolve so the parked future returns.
        assert!(
            gate.resolve(1, client_protocol::permission::PermissionResponseDto::Deny, "Bash")
                .await
        );
        let _ = task.await.unwrap();
    }

    /// T11: the build-time coordinator-activation flag defaults to `false`, so a
    /// default-constructed `DesktopConfig` is NOT a coordinator session.
    /// Additive guardrail: default sessions must be byte-identical, mode off.
    #[test]
    fn default_config_is_not_coordinator() {
        let cfg = DesktopConfig::default();
        assert!(
            !cfg.session_started_as_coordinator,
            "default DesktopConfig must not start as coordinator"
        );

        // The frozen field set remains reachable via struct-update syntax, and
        // the flag flips cleanly to opt into a coordinator session.
        let coord = DesktopConfig {
            session_started_as_coordinator: true,
            ..cfg
        };
        assert!(coord.session_started_as_coordinator);
    }

    /// T11: `build()` surfaces the per-session coordinator subsystem handles
    /// (`TeamRegistry` + `CoordinatorMode`) on the runtime so the status feed
    /// (and PHASE-2 command router) can read them. A default-config build is
    /// additive-only: the mode is constructed but NOT entered.
    #[tokio::test]
    async fn runtime_exposes_coordinator_handles() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed");

        // The handles exist on the runtime …
        assert!(
            rt.coordinator.list().await.is_empty(),
            "a fresh coordinator session has no workers"
        );
        // … and a default (non-coordinator) build leaves the mode disabled.
        assert!(
            !rt.coordinator_mode.is_enabled(),
            "default build must not enter coordinator mode"
        );
    }

    /// Session lifecycle: the boot path fires `SessionStart` (source=startup)
    /// once the orchestrator + hook registry are wired, and does so best-effort.
    ///
    /// We register a `SessionStart` command hook in the project
    /// `cwd/.claude/settings.json` that `build()` reads at boot. `build()` must
    /// (a) complete successfully — proving the wired `fire_session_start`
    /// (which uses the minimal stub process runner, so the hook command itself
    /// errors `Unsupported`) is best-effort and never breaks boot — and (b)
    /// surface the loaded `SessionStart` hook via the orchestrator's
    /// `list_hooks`, proving the boot path actually loaded the session-lifecycle
    /// hook the wired `fire_session_start("startup")` call dispatched against.
    #[tokio::test]
    async fn build_fires_session_start_against_a_registered_hook() {
        use traits::OrchestratorHandle as _;

        let (_tmp, cfg) = test_config(true);
        // Project settings the hooks loader reads at boot
        // (cwd/.claude/settings.json) — a single `SessionStart` command hook.
        let claude_dir = cfg.cwd.join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("mk .claude");
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{ "hooks": { "SessionStart": [ { "hooks": [
                { "type": "command", "command": "true" }
            ] } ] } }"#,
        )
        .expect("write settings.json");

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        // The wired `fire_session_start("startup")` runs INSIDE build(): a
        // failing/unsupported hook command must NOT break boot (best-effort).
        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() must succeed even with a (failing) SessionStart hook registered");

        // The boot path loaded the SessionStart hook into the wired registry —
        // exactly the hook the in-build `fire_session_start("startup")` fired.
        let hooks = rt.orchestrator.list_hooks().await;
        assert!(
            hooks.iter().any(|h| h.event == "SessionStart"),
            "boot must load the SessionStart hook the lifecycle fire dispatches against: {hooks:?}"
        );
    }

    /// Instruction-load lifecycle: the boot path fires `InstructionsLoaded`
    /// (once per loaded CLAUDE.md, load_reason=session_start) right after
    /// `SessionStart`, best-effort.
    ///
    /// We register an `InstructionsLoaded` command hook in the project
    /// `cwd/.claude/settings.json` that `build()` reads at boot. `build()` must
    /// (a) complete successfully — proving the wired `fire_instructions_loaded`
    /// (using the minimal stub process runner, so the hook command itself errors
    /// `Unsupported`) is best-effort and never breaks boot — and (b) surface the
    /// loaded `InstructionsLoaded` hook via the orchestrator's `list_hooks`,
    /// proving the boot path loaded the instruction-load-lifecycle hook the wired
    /// `fire_instructions_loaded()` call dispatched against.
    ///
    /// NOTE: this test uses the DEFAULT empty memory provider
    /// (`cfg.memory_provider == None` ⟶ `StaticMemoryProvider::empty()`), so no
    /// instruction file actually fires through the command hook here — the
    /// helper is a no-op over zero files, and the assertion only pins the
    /// boot-path fire seam + best-effort contract. The injectable
    /// `cfg.memory_provider` (production wires `real_provider()`) closes the
    /// load-NO-memory gap; the end-to-end "memory flows through build() and the
    /// hook actually fires over it" path is proven with a CONTROLLED in-memory
    /// provider in [`build_with_injected_memory_fires_instructions_loaded`]
    /// (never the real filesystem).
    #[tokio::test]
    async fn build_fires_instructions_loaded_against_a_registered_hook() {
        use traits::OrchestratorHandle as _;

        let (_tmp, cfg) = test_config(true);
        // Project settings the hooks loader reads at boot
        // (cwd/.claude/settings.json) — a single `InstructionsLoaded` command hook.
        let claude_dir = cfg.cwd.join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("mk .claude");
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{ "hooks": { "InstructionsLoaded": [ { "hooks": [
                { "type": "command", "command": "true" }
            ] } ] } }"#,
        )
        .expect("write settings.json");

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        // The wired `fire_instructions_loaded()` runs INSIDE build(): a
        // failing/unsupported hook command must NOT break boot (best-effort).
        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() must succeed even with a (failing) InstructionsLoaded hook registered");

        // The boot path loaded the InstructionsLoaded hook into the wired
        // registry — exactly the hook the in-build `fire_instructions_loaded()`
        // fires against once the memory provider yields instruction files.
        let hooks = rt.orchestrator.list_hooks().await;
        assert!(
            hooks.iter().any(|h| h.event == "InstructionsLoaded"),
            "boot must load the InstructionsLoaded hook the lifecycle fire dispatches against: {hooks:?}"
        );
    }

    /// End-to-end proof of the injectable memory-provider seam (the real-provider
    /// path) using a CONTROLLED in-memory provider — NEVER the real filesystem.
    ///
    /// Production wires `cfg.memory_provider = Some(real_provider())`, which
    /// reads the developer's real `~/.claude/CLAUDE.md` and would make the boot
    /// tests non-deterministic. So this test instead injects
    /// `Some(StaticMemoryProvider::with_files([..one CLAUDE.md..]))` — the SAME
    /// `cfg.memory_provider` seam the real provider flows through — and proves
    /// that the injected memory flows through `build()` into the orchestrator
    /// and lands in the assembled SYSTEM PROMPT (the `<memory>` block with the
    /// file's path + body). The default-empty sibling
    /// ([`build_constructs_runtime_deterministically`] etc.) elides the
    /// `<memory>` section entirely, so the block's presence is the load-bearing
    /// difference the injected provider makes.
    ///
    /// The end-to-end "`fire_instructions_loaded()` fires the registered
    /// `InstructionsLoaded` hook over the controlled memory" half is proven at
    /// the orchestrator layer in `orchestrator/tests/instructions_loaded_hook_test.rs`
    /// (a `RecordingHandler` observes the per-file fire). It is NOT re-asserted
    /// here because `build()` wires the minimal-platform STUB process runner
    /// (`platform_posix_minimal::PosixProcess::run` always returns
    /// `ProcessError::Unsupported`), so a `command` hook produces no side effect
    /// to observe from outside `build()`. We register the hook anyway, so the
    /// fire still runs over the injected file (best-effort) inside `build()`.
    #[tokio::test]
    async fn build_with_injected_memory_reaches_system_prompt() {
        use traits::OrchestratorHandle as _;

        let (_tmp, mut cfg) = test_config(true);

        // Register an InstructionsLoaded hook so the in-build
        // `fire_instructions_loaded()` actually dispatches over the injected
        // file (best-effort; the stub runner makes it a no-op side-effect-wise).
        let claude_dir = cfg.cwd.join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("mk .claude");
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{ "hooks": { "InstructionsLoaded": [ { "hooks": [
                { "type": "command", "command": "true" }
            ] } ] } }"#,
        )
        .expect("write settings.json");

        // INJECT a CONTROLLED in-memory provider (NOT the real FS): one
        // top-level project CLAUDE.md. This is the exact `cfg.memory_provider`
        // seam production fills with `orchestrator::prompt::real_provider()`.
        let memory_path = cfg.cwd.join("CLAUDE.md");
        let memory_body = "PROJECT MEMORY: always be terse.";
        let memory_file = orchestrator::prompt::MemoryFile {
            path: memory_path.clone(),
            body: memory_body.to_string(),
            is_local_override: false,
        };
        cfg.memory_provider = Some(Arc::new(
            orchestrator::test_support::StaticMemoryProvider::with_files(vec![memory_file]),
        ));

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        // build() runs the wired `fire_instructions_loaded()` over the injected
        // memory (best-effort) and returns an orchestrator that loads that SAME
        // provider for its system prompt.
        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() with an injected memory provider must succeed");

        // The injected CLAUDE.md must reach the assembled system prompt: the
        // `<memory>` block carries the file's path + body. This proves the
        // controlled provider flowed through build() into the orchestrator's
        // prompt assembly — the gap (desktop loads NO memory) is closed.
        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        assert!(
            sys.contains("<memory>"),
            "injected memory must emit a <memory> block in the system prompt: {sys}"
        );
        assert!(
            sys.contains(memory_body),
            "the injected CLAUDE.md body must appear in the system prompt: {sys}"
        );
        assert!(
            sys.contains(&memory_path.display().to_string()),
            "the injected CLAUDE.md path must appear in the <memory> block: {sys}"
        );

        // Sanity: the InstructionsLoaded hook the in-build fire dispatched
        // against was loaded into the wired registry.
        let hooks = rt.orchestrator.list_hooks().await;
        assert!(
            hooks.iter().any(|h| h.event == "InstructionsLoaded"),
            "boot must load the InstructionsLoaded hook: {hooks:?}"
        );
    }

    /// Determinism guard for the default seam: a default-config build
    /// (`cfg.memory_provider == None` ⟶ `StaticMemoryProvider::empty()`) loads
    /// NO memory, so the system prompt has NO `<memory>` block. This pins that
    /// the existing boot tests stay deterministic (they never read the real
    /// `~/.claude/CLAUDE.md`).
    #[tokio::test]
    async fn build_default_loads_no_memory() {
        let (_tmp, cfg) = test_config(true);
        assert!(
            cfg.memory_provider.is_none(),
            "default config must leave memory_provider None (empty, deterministic)"
        );
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed");

        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        assert!(
            !sys.contains("<memory>"),
            "default (empty) memory provider must elide the <memory> block: {sys}"
        );
    }

    // ----- T12: mode-exclusive coordinator tool selection -------------------

    /// No-op spawn seam — the tool-selection tests never invoke it; they only
    /// need a concrete `Arc<dyn TeamSpawnSeam>` to construct `CoordinatorWiring`.
    struct NoopSeam;

    #[async_trait::async_trait]
    impl traits::team_spawn::TeamSpawnSeam for NoopSeam {
        async fn spawn_teammate(
            &self,
            _agent_id: protocol::AgentId,
            _name: String,
            _description: String,
        ) -> Result<String, traits::team_spawn::TeamSpawnError> {
            Ok(String::new())
        }
        async fn kill(
            &self,
            _task_id: &str,
        ) -> Result<(), traits::team_spawn::TeamSpawnError> {
            Ok(())
        }
    }

    /// A fully-stubbed `BuiltinToolContext` — enough to enumerate registered
    /// names and probe per-tool behavior markers; no tool is ever invoked.
    fn stub_tool_ctx() -> tool_api::BuiltinToolContext {
        tool_api::test_support::shell_test_ctx(traits::process::ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }

    fn coordinator_wiring() -> CoordinatorWiring {
        CoordinatorWiring {
            team: Arc::new(coordinator::TeamRegistry::new(protocol::AgentId::new())),
            mode: Arc::new(coordinator::CoordinatorMode::new()),
            spawn_seam: Arc::new(NoopSeam),
            output: Arc::new(orchestrator::test_support::MockOutputStream::new()),
        }
    }

    /// T12: a default session (`coordinator: None`) registers exactly ONE
    /// `TeamCreate`, and it is `tool_team`'s — distinguished by its behavior
    /// marker `max_result_size_chars() == 30_000` (`MAX_TOOL_OUTPUT_LENGTH`),
    /// vs. the coordinator tool's `100_000`. Guardrail: byte-identical default.
    #[test]
    fn tool_registry_default_mode_registers_tool_team_create() {
        let reg = desktop_tool_registry(stub_tool_ctx(), None, None);

        let names = reg.all_names();
        assert_eq!(
            names.iter().filter(|n| *n == "TeamCreate").count(),
            1,
            "default mode must register exactly one TeamCreate"
        );
        assert_eq!(
            names.iter().filter(|n| *n == "TeamDelete").count(),
            1,
            "default mode must register exactly one TeamDelete"
        );

        // Behavior marker: tool_team's TeamCreate caps results at 30_000;
        // the coordinator's caps at 100_000.
        let create = reg
            .find_by_name("TeamCreate")
            .expect("TeamCreate must be registered");
        assert_eq!(
            create.max_result_size_chars(),
            tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH,
            "default mode must register tool_team's TeamCreate (30_000 cap)"
        );
    }

    /// T12: a coordinator-capable session (`coordinator: Some`) registers the
    /// coordinator `TeamCreate` IN PLACE OF `tool_team`'s — still exactly ONE
    /// `TeamCreate` and ONE `TeamDelete` (no silent shadow, no duplicate name
    /// in the system prompt). The registered `TeamCreate` is the coordinator's,
    /// distinguished by `max_result_size_chars() == 100_000`.
    #[test]
    fn tool_registry_coordinator_mode_registers_coordinator_create() {
        let reg = desktop_tool_registry(stub_tool_ctx(), Some(coordinator_wiring()), None);

        let names = reg.all_names();
        assert_eq!(
            names.iter().filter(|n| *n == "TeamCreate").count(),
            1,
            "coordinator mode must register exactly one TeamCreate (no shadow)"
        );
        assert_eq!(
            names.iter().filter(|n| *n == "TeamDelete").count(),
            1,
            "coordinator mode must register exactly one TeamDelete (no shadow)"
        );

        // No duplicate names ANYWHERE in the assembled registry.
        let mut sorted = names.clone();
        sorted.sort();
        let mut deduped = sorted.clone();
        deduped.dedup();
        assert_eq!(
            sorted, deduped,
            "no tool name may appear twice in the assembled registry"
        );

        // Behavior marker: the registered TeamCreate is the coordinator's.
        let create = reg
            .find_by_name("TeamCreate")
            .expect("TeamCreate must be registered");
        assert_eq!(
            create.max_result_size_chars(),
            100_000,
            "coordinator mode must register the coordinator TeamCreate (100_000 cap)"
        );
    }

    // ----- T13: build() composition-root coordinator wiring -----------------

    /// T13: a `build()` with `session_started_as_coordinator: true` enters
    /// coordinator mode at BUILD time, so `runtime.coordinator_mode.is_enabled()`
    /// is `true` and the `session_started_as_coordinator` flag is recorded on the
    /// mode. A default-config build leaves the mode disabled (asserted in
    /// `runtime_exposes_coordinator_handles`) — the additive guardrail.
    #[tokio::test]
    async fn build_coordinator_session_enters_mode() {
        let (_tmp, mut cfg) = test_config(true);
        cfg.session_started_as_coordinator = true;
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed");

        assert!(
            rt.coordinator_mode.is_enabled(),
            "a coordinator session must enter coordinator mode at build time"
        );
        assert!(
            rt.coordinator_mode.session_started_as_coordinator,
            "the build-time activation flag must be recorded on the mode"
        );
        // A coordinator session still surfaces an (empty) registry.
        assert!(
            rt.coordinator.list().await.is_empty(),
            "a freshly-built coordinator session has no workers yet"
        );
    }

    /// T13: a coordinator session wires the shared `MailboxRouter` (the one the
    /// `TeamRegistry` owns) into `BuiltinToolContext.mailbox_router`, so the
    /// builtin `SendMessage` tool and coordinator routing share the SAME
    /// mailboxes. This exercises the exact `tool_ctx.mailbox_router = Some(..)`
    /// wiring decision T13 introduces in `build()`: we assemble the desktop tool
    /// registry the way the coordinator branch does (the team's router cast to
    /// `dyn MailboxRouterHandle` placed on the context), then drive the builtin
    /// `SendMessage` tool. Because the router IS wired, a route to a registered
    /// worker mailbox SUCCEEDS — the tool no longer takes the "router not wired"
    /// `Internal` error path it returns when `mailbox_router` is `None`.
    #[tokio::test]
    async fn mailbox_router_is_wired_when_coordinator() {
        // The shared coordinator router — exactly what `build()` clones into
        // `tool_ctx.mailbox_router` on the coordinator branch.
        let team = Arc::new(coordinator::TeamRegistry::new(protocol::AgentId::new()));
        let worker = team
            .spawn_worker("explorer".into(), "alpha".into(), String::new())
            .await
            .expect("spawn_worker registers a mailbox on the shared router");

        // Assemble the tool registry the way `build()`'s coordinator branch does:
        // the team's `MailboxRouter` cast to `dyn MailboxRouterHandle` on the
        // context (the load-bearing wiring — `None` here is the pre-M10 default).
        let mut ctx = stub_tool_ctx();
        ctx.mailbox_router = Some(team.mailbox_router.clone()
            as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        let wiring = CoordinatorWiring {
            team: team.clone(),
            mode: Arc::new(coordinator::CoordinatorMode::new()),
            spawn_seam: Arc::new(NoopSeam),
            output: Arc::new(orchestrator::test_support::MockOutputStream::new()),
        };
        let reg = desktop_tool_registry(ctx, Some(wiring), None);

        // The builtin SendMessage tool (from `tool_ui`) reads
        // `ctx.mailbox_router`. With the router wired, routing to the registered
        // worker succeeds. With `None` it would return the
        // "MailboxRouterHandle not wired" `Internal` error instead.
        let send = reg
            .find_by_name("SendMessage")
            .expect("SendMessage builtin must be registered");
        let result = send
            .call(
                serde_json::json!({
                    "to_agent_id": worker.as_uuid().to_string(),
                    "message": "hello teammate",
                }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("SendMessage must route through the wired router (not the unwired error path)");
        // A successful route returns a result the tool surfaces (non-error path).
        let _ = result;

        // Negative side of the conditional: the DEFAULT branch leaves
        // `mailbox_router` `None` (exactly what `build()` does for a non-
        // coordinator session), so the SAME SendMessage call takes the
        // "router not wired" `Internal` error path. This locks both sides of
        // the T13 wiring decision so a regression in either is caught.
        let default_reg = desktop_tool_registry(stub_tool_ctx(), None, None);
        let default_send = default_reg
            .find_by_name("SendMessage")
            .expect("SendMessage builtin must be registered in the default set too");
        let err = default_send
            .call(
                serde_json::json!({
                    "to_agent_id": worker.as_uuid().to_string(),
                    "message": "hello teammate",
                }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect_err("default (unwired) SendMessage must error: no router on the context");
        assert!(
            format!("{err}").contains("not wired"),
            "default session must hit the 'MailboxRouterHandle not wired' path, got: {err}"
        );
    }

    /// Phase 3a-bash: the sandbox-auto-allow config the enforced policy carries
    /// is derived faithfully from the `settings.json` sandbox subsection across
    /// tiers — `enabled`, the TS-default-true `autoAllowBashIfSandboxed`, and
    /// `excludedCommands` — and is an inert/disabled config when sandboxing is
    /// off or unconfigured.
    #[test]
    fn sandbox_auto_allow_from_settings_tiers_maps_faithfully() {
        use super::sandbox_auto_allow_from_settings_tiers;

        // (1) Sandbox enabled, no explicit autoAllow override → TS default TRUE,
        //     no excluded commands → every command would be sandboxed +
        //     auto-allowed.
        let enabled = sandbox_auto_allow_from_settings_tiers(&[r#"{ "sandbox": { "enabled": true } }"#]);
        assert!(enabled.enabled, "settings enabled → config enabled");
        assert!(
            enabled.auto_allow_bash_if_sandboxed,
            "autoAllowBashIfSandboxed must default TRUE (claude-code parity)"
        );
        assert!(enabled.excluded_commands.is_empty());
        assert!(
            enabled.auto_allows("echo hi"),
            "enabled + default auto-allow → a sandboxable command is auto-allowed"
        );

        // (2) Explicit excludedCommands flow through; an excluded command is NOT
        //     auto-allowed, a normal one still is.
        let with_excludes = sandbox_auto_allow_from_settings_tiers(&[
            r#"{ "sandbox": { "enabled": true, "excludedCommands": ["bazel:*", "make"] } }"#,
        ]);
        assert!(with_excludes.enabled);
        assert_eq!(
            with_excludes.excluded_commands,
            vec!["bazel:*".to_string(), "make".to_string()]
        );
        assert!(!with_excludes.auto_allows("bazel build //..."));
        assert!(with_excludes.auto_allows("echo hi"));

        // (3) Explicit autoAllowBashIfSandboxed:false overrides the TS default;
        //     the command would still be sandboxed but is NOT auto-allowed.
        let auto_off = sandbox_auto_allow_from_settings_tiers(&[
            r#"{ "sandbox": { "enabled": true, "autoAllowBashIfSandboxed": false } }"#,
        ]);
        assert!(auto_off.enabled);
        assert!(!auto_off.auto_allow_bash_if_sandboxed);
        assert!(!auto_off.auto_allows("echo hi"));

        // (4) Sandbox DISABLED → never auto-allows even though autoAllow defaults
        //     true.
        let disabled = sandbox_auto_allow_from_settings_tiers(&[r#"{ "sandbox": { "enabled": false } }"#]);
        assert!(!disabled.enabled);
        assert!(!disabled.auto_allows("echo hi"));

        // (5) No `sandbox` subsection at all (the common case) → disabled, inert.
        let none = sandbox_auto_allow_from_settings_tiers(&[r#"{ "permissions": { "allow": [] } }"#]);
        assert!(!none.enabled);
        assert!(!none.auto_allows("echo hi"));

        // (6) Tier precedence: a later tier's sandbox subsection overrides an
        //     earlier one (ascending priority, last write wins).
        let layered = sandbox_auto_allow_from_settings_tiers(&[
            r#"{ "sandbox": { "enabled": false } }"#,            // user
            r#"{ "sandbox": { "enabled": true } }"#,             // project (wins)
        ]);
        assert!(layered.enabled, "later tier's sandbox.enabled wins");

        // (7) Empty / malformed tiers are skipped without panicking.
        let robust = sandbox_auto_allow_from_settings_tiers(&["", "not json", r#"{ "sandbox": { "enabled": true } }"#]);
        assert!(robust.enabled);
    }
}
