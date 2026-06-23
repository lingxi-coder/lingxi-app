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

mod agent_skill_loader;
mod background_agent;
mod connect;
pub mod file_changed_watch;
pub mod settings_watch;
mod skill_loader;

use anthropic_oauth::client::ClaudeAiOAuthClient;
use anthropic_oauth::config::ClaudeAiOAuthConfig;
use anthropic_oauth::handle::OAuthHandle;
use anthropic_oauth::{OAuthCredentialProvider, RefreshDriver};
use tool_api::AnthropicRequestBuilder;
use client_adapter::{AdapterPermissionGate, PermissionRequestSink};
use llm_client::{DefaultLlmClient, Transport};
use orchestrator::model::user_agent::UserAgentEnv;
use orchestrator::provider_adapter::SubscriberState;
use llm_client::LlmTransportBridge;
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
use platform_posix::{
    secure_storage_for_platform, PosixClock, PosixFileSystem, PosixHttp, PosixMcpTransport,
    PosixProcess, PosixRuntime, PosixSandbox, PosixWorktreeManager,
};
use sandbox::runtime_config::Platform as SandboxPlatform;
use secret::CredentialManager;
use skill_api::SkillRegistry;
use std::sync::Arc;
use tokio::sync::RwLock;
use tool_api::{BuiltinToolContext, ToolRegistry};
use traits::{AuthHandle, McpTransport, OrchestratorHandle, OutputStream};

/// API provider, mirroring `APIProvider` (`utils/model/providers.ts:4`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ApiProvider {
    FirstParty,
    Bedrock,
    Vertex,
    Foundry,
}

/// Port of `isEnvTruthy` (`envUtils.ts:32-37`); value test delegated to
/// [`traits::env::is_env_truthy`].
fn is_env_truthy(key: &str) -> bool {
    traits::env::is_env_truthy(std::env::var(key).ok().as_deref())
}

/// Port of `getAPIProvider()` (`utils/model/providers.ts:6-14`).
fn api_provider() -> ApiProvider {
    if is_env_truthy("CLAUDE_CODE_USE_BEDROCK") {
        ApiProvider::Bedrock
    } else if is_env_truthy("CLAUDE_CODE_USE_VERTEX") {
        ApiProvider::Vertex
    } else if is_env_truthy("CLAUDE_CODE_USE_FOUNDRY") {
        ApiProvider::Foundry
    } else {
        ApiProvider::FirstParty
    }
}

/// A deprecated model's display name and its per-provider retirement dates.
struct DeprecationEntry {
    model_name: &'static str,
    first_party: Option<&'static str>,
    bedrock: Option<&'static str>,
    vertex: Option<&'static str>,
    foundry: Option<&'static str>,
}

impl DeprecationEntry {
    fn retirement_date(&self, provider: ApiProvider) -> Option<&'static str> {
        match provider {
            ApiProvider::FirstParty => self.first_party,
            ApiProvider::Bedrock => self.bedrock,
            ApiProvider::Vertex => self.vertex,
            ApiProvider::Foundry => self.foundry,
        }
    }
}

/// Deprecated models and their retirement dates by provider.
/// Byte-locked to `DEPRECATED_MODELS` (`utils/model/deprecation.ts:33-61`).
const DEPRECATED_MODELS: &[(&str, DeprecationEntry)] = &[
    (
        "claude-3-opus",
        DeprecationEntry {
            model_name: "Claude 3 Opus",
            first_party: Some("January 5, 2026"),
            bedrock: Some("January 15, 2026"),
            vertex: Some("January 5, 2026"),
            foundry: Some("January 5, 2026"),
        },
    ),
    (
        "claude-3-7-sonnet",
        DeprecationEntry {
            model_name: "Claude 3.7 Sonnet",
            first_party: Some("February 19, 2026"),
            bedrock: Some("April 28, 2026"),
            vertex: Some("May 11, 2026"),
            foundry: Some("February 19, 2026"),
        },
    ),
    (
        "claude-3-5-haiku",
        DeprecationEntry {
            model_name: "Claude 3.5 Haiku",
            first_party: Some("February 19, 2026"),
            bedrock: None,
            vertex: None,
            foundry: None,
        },
    ),
];

struct DeprecatedModelInfo {
    model_name: &'static str,
    retirement_date: &'static str,
}

fn deprecated_model_info(model_id: &str) -> Option<DeprecatedModelInfo> {
    let lowercase = model_id.to_lowercase();
    let provider = api_provider();
    for (key, value) in DEPRECATED_MODELS {
        let Some(retirement_date) = value.retirement_date(provider) else {
            continue;
        };
        if !lowercase.contains(key) {
            continue;
        }
        return Some(DeprecatedModelInfo {
            model_name: value.model_name,
            retirement_date,
        });
    }
    None
}

/// Get a deprecation warning message for a model, or `None` if not deprecated.
///
/// Direct port of `getModelDeprecationWarning` (`deprecation.ts:88-101`).
/// Moved here from `providers::deprecation` (Plan 3b Task 3) so the CLI host
/// can call it without a direct `providers` dependency; re-exported at this
/// composition-root surface for backward compatibility.
///
/// With any current (Claude 4-generation) default model the lookup returns
/// `None`, so the startup output stays byte-identical until a user configures
/// one of the deprecated Claude 3 ids.
#[must_use]
pub fn model_deprecation_warning(model_id: Option<&str>) -> Option<String> {
    let model_id = model_id.filter(|m| !m.is_empty())?;
    let info = deprecated_model_info(model_id)?;
    Some(format!(
        "⚠ {} will be retired on {}. Consider switching to a newer model.",
        info.model_name, info.retirement_date
    ))
}

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
    settings_dir: &std::path::Path,
) -> permission::sandbox_auto_allow::SandboxAutoAllowConfig {
    use sandbox::runtime_config::{SandboxSettingsJson, SettingsJson, SettingsPermissions};

    // Fold each tier's `sandbox` subsection, last write wins per the whole
    // subsection (matching how the converter consumes a single `SettingsJson`).
    let mut merged_sandbox: Option<SandboxSettingsJson> = None;
    // Accumulate the merged `permissions` across tiers so the converter sees the
    // full allow/deny/additionalDirectories feed (extend, not last-write-wins, for
    // the rule lists). The auto-allow config only reads `enabled` /
    // `excluded_commands`, but threading permissions keeps both folds symmetric.
    let mut merged_perms = SettingsPermissions::default();
    let mut saw_perms = false;
    // Track the explicit auto-allow override separately so the TS default (true)
    // can be applied only when NO tier set it.
    let mut explicit_auto_allow: Option<bool> = None;
    for raw in raw_tiers {
        let Ok(parsed) = serde_json::from_str::<SettingsJson>(raw) else {
            continue;
        };
        if let Some(p) = parsed.permissions {
            saw_perms = true;
            merged_perms.allow.extend(p.allow);
            merged_perms.deny.extend(p.deny);
            merged_perms
                .additional_directories
                .extend(p.additional_directories);
        }
        if let Some(s) = parsed.sandbox {
            if let Some(v) = s.auto_allow_bash_if_sandboxed {
                explicit_auto_allow = Some(v);
            }
            merged_sandbox = Some(s);
        }
    }

    let runtime = sandbox::policy_convert::convert_settings_to_runtime_config(
        &SettingsJson {
            permissions: saw_perms.then_some(merged_perms),
            sandbox: merged_sandbox,
            settings_dir: Some(settings_dir.to_path_buf()),
        },
        // The auto-allow config reads only `enabled` / `excluded_commands`, so no
        // session seed context is needed (claude temp dir / settings-file /
        // worktree paths are owned by the posix `prepare` layer here).
        &sandbox::policy_convert::SandboxConvertContext::default(),
    );

    permission::sandbox_auto_allow::SandboxAutoAllowConfig::new(
        runtime.enabled,
        // claude-code `isAutoAllowBashIfSandboxedEnabled()` defaults TRUE.
        explicit_auto_allow.unwrap_or(true),
        runtime.excluded_commands,
    )
}

/// (SANDBOX.1) Fold the `sandbox` subsection of the settings tiers (ascending
/// priority, last write wins) into a full [`sandbox::runtime_config::SandboxRuntimeConfig`].
///
/// Sibling of [`sandbox_auto_allow_from_settings_tiers`], but returns the whole
/// runtime config so the composition root can (a) decide `sandbox_available` from
/// `cfg.enabled` and (b) hand the live network/filesystem policy to the bash
/// sandbox path. Matches claude-code: the sandbox is opt-in via `sandbox.enabled`
/// (default OFF — an empty/absent subsection yields `enabled = false`).
/// (PERM.1) Decide whether `build()` wraps the base gate with
/// [`permission::PolicyPermissionGate`] (i.e. enforces deny/allow rules + mode +
/// sandbox-auto-allow). Pure so it is unit-testable; see the call site in
/// [`build`] for the full rationale.
///
/// - `BypassPermissions` mode (`--dangerously-skip-permissions`) ⇒ never enforce
///   (allow-all), matching claude-code's bypass.
/// - An explicit `LINGXI_ENFORCE_PERMISSIONS` value wins: falsey
///   (`""|0|off|false|no`) ⇒ off, anything else ⇒ on.
/// - Unset ⇒ default-on ONLY for the CLI/desktop `NoOpPermissionGate` inner
///   (`use_noop_inner == true`); transport hosts (`AdapterPermissionGate`) keep
///   their prior env-opt-in behavior so their remote-driven gate is unchanged.
fn should_enforce_permissions(
    env_value: Option<&str>,
    use_noop_inner: bool,
    mode: permission::PermissionMode,
) -> bool {
    if mode == permission::PermissionMode::BypassPermissions {
        return false;
    }
    match env_value {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "off" | "false" | "no"
        ),
        None => use_noop_inner,
    }
}

/// Whether the live cron scheduler should run. Faithful to claude-code's
/// `isKairosCronEnabled` LOCAL kill-switch (`ScheduleCronTool/prompt.ts:34/38`):
/// the `CLAUDE_CODE_DISABLE_CRON` env override (truthy ⇒ cron OFF) "wins over"
/// the GrowthBook fleet flag. That flag defaults to `true`, so this wired-on
/// scheduler already matches the default-enabled fleet state — only the local
/// disable override was missing. (The remote GB gate itself is not portable —
/// LingXi has no GrowthBook substrate — but its default-true state is.)
fn cron_scheduler_enabled(disable_cron_env: Option<&str>) -> bool {
    !traits::env::is_env_truthy(disable_cron_env)
}

/// Fold the `sandbox` subsection of the settings tiers (ascending priority, last
/// write wins) into a full [`sandbox::runtime_config::SandboxRuntimeConfig`].
///
/// `ctx` carries the session/host seeds (`getClaudeTempDir()`, the settings-file
/// `deny_write` paths, the managed drop-in dir, `.claude/skills`, …). It is built
/// by the caller (the composition root has `claude_home`/`cwd`/`managed` in scope,
/// so the helper stays pure and unit-testable; tests pass a minimal seed). See
/// the `build()` call site and spec §5 for which seeds have a boot-time analog.
#[must_use]
fn sandbox_runtime_config_from_settings_tiers(
    raw_tiers: &[&str],
    settings_dir: &std::path::Path,
    ctx: &sandbox::policy_convert::SandboxConvertContext,
) -> sandbox::runtime_config::SandboxRuntimeConfig {
    use sandbox::runtime_config::{SandboxSettingsJson, SettingsJson, SettingsPermissions};

    let mut merged_sandbox: Option<SandboxSettingsJson> = None;
    // Accumulate the merged `permissions` across tiers (extend allow/deny/
    // additionalDirectories — these feed the filesystem allow_write / deny_read /
    // network allowed_domains derivation in `convert_settings_to_runtime_config`).
    let mut merged_perms = SettingsPermissions::default();
    let mut saw_perms = false;
    for raw in raw_tiers {
        let Ok(parsed) = serde_json::from_str::<SettingsJson>(raw) else {
            continue;
        };
        if let Some(p) = parsed.permissions {
            saw_perms = true;
            merged_perms.allow.extend(p.allow);
            merged_perms.deny.extend(p.deny);
            merged_perms
                .additional_directories
                .extend(p.additional_directories);
        }
        if let Some(s) = parsed.sandbox {
            merged_sandbox = Some(s);
        }
    }
    sandbox::policy_convert::convert_settings_to_runtime_config(
        &SettingsJson {
            permissions: saw_perms.then_some(merged_perms),
            sandbox: merged_sandbox,
            settings_dir: Some(settings_dir.to_path_buf()),
        },
        ctx,
    )
}

/// claude-code `getClaudeTempDir()` + `getClaudeTempDirName()` analog (Shell.ts:307),
/// identical to the canonical private `claude_temp_dir()` in `tool-shell`'s
/// `prompt.rs`: `baseTmpDir = CLAUDE_CODE_TMPDIR || (windows ? tmpdir() : "/tmp")`,
/// realpath-resolved, name `claude` on Windows else `claude-{uid}`, joined with a
/// trailing separator. Seeded into the sandbox `allow_write` so the shell's
/// cwd-tracking file stays writable.
fn claude_temp_dir() -> String {
    let base: std::path::PathBuf = std::env::var_os("CLAUDE_CODE_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(target_os = "windows") {
                std::env::temp_dir()
            } else {
                std::path::PathBuf::from("/tmp")
            }
        });
    let resolved_base = std::fs::canonicalize(&base).unwrap_or(base);
    let name = if cfg!(target_os = "windows") {
        "claude".to_string()
    } else {
        format!("claude-{}", current_uid())
    };
    let joined = resolved_base.join(name);
    let mut s = joined.to_string_lossy().into_owned();
    s.push(std::path::MAIN_SEPARATOR);
    s
}

/// The real (not effective) UID, mirroring TS `process.getuid?.() ?? 0`.
/// `nix::unistd::getuid()` is a SAFE wrapper, so this crate keeps its
/// `#![forbid(unsafe_code)]` (same pattern as `apps/cli/src/bypass_env.rs`).
#[cfg(unix)]
fn current_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// Port of claude-code `isPlatformInEnabledList()` (sandbox-adapter.ts:505): is
/// the current platform in `sandbox.enabledPlatforms`? `None` (unset) ⇒ all
/// supported platforms allowed (`true`); empty list ⇒ none allowed (`false`,
/// which is how an operator turns the sandbox off everywhere); otherwise the list
/// must contain the current platform.
#[must_use]
pub fn platform_in_enabled_list(
    enabled: Option<&[sandbox::runtime_config::Platform]>,
    current: sandbox::runtime_config::Platform,
) -> bool {
    match enabled {
        None => true,
        Some([]) => false,
        Some(list) => list.contains(&current),
    }
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
    /// The (optional) analytics bus the coordinator `TeamCreate` / `TeamDelete`
    /// tools fire their telemetry through (`tengu_team_created` /
    /// `tengu_team_deleted`). `build()` passes the orchestrator bus; the offline
    /// snapshot factory passes `None` (telemetry → `tracing`).
    pub bus: Option<Arc<telemetry::AnalyticsBus>>,
    /// The (optional) background-task spawner the coordinator `TeamCreate` tool
    /// uses to start each teammate's mailbox→runner PUMP — the bridge that
    /// delivers a coordinator `SendMessage` into the teammate's turn loop. `None`
    /// ⇒ no pump (routed messages queue in the mailbox but are not auto-drained;
    /// the offline registry-snapshot factory passes `None`). `build()` passes the
    /// session `PosixRuntime` so the pump runs (D17 — never a direct
    /// `tokio::spawn`).
    pub runtime: Option<Arc<dyn traits::RuntimeSpawner>>,
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

/// Launches `LocalWorkflow` background tasks for the `Workflow` tool by spawning
/// through the shared [`tasks::registry::TaskRegistry`]. Resolves the spec's
/// `scriptPath` / `script` / `name` to a script source (claude-code precedence);
/// `scriptPath`/`name` are read from disk relative to `cwd`.
struct TaskRegistryWorkflowLauncher {
    registry: Arc<tasks::registry::TaskRegistry>,
    cwd: std::path::PathBuf,
}

#[async_trait::async_trait]
impl tool_workflow::WorkflowLauncher for TaskRegistryWorkflowLauncher {
    async fn launch(
        &self,
        spec: tool_workflow::WorkflowLaunchSpec,
    ) -> Result<tool_workflow::WorkflowLaunched, tool_workflow::WorkflowLaunchError> {
        let cwd = self.cwd.clone();
        let script = tool_workflow::resolve_script(&spec, |p| {
            let path = std::path::Path::new(p);
            let full = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            std::fs::read_to_string(full)
        })?;
        // Reject a malformed `meta` block at the tool boundary (claude-code parses
        // + validates `meta` when the Workflow tool accepts a script). The
        // byte-exact message surfaces to the model as the tool error.
        workflow::validate_meta(&script).map_err(|e| {
            let msg = match e {
                workflow::WorkflowError::Script(m) => m,
                other => other.to_string(),
            };
            tool_workflow::WorkflowLaunchError(msg)
        })?;
        let task_id = self
            .registry
            .spawn(
                tasks::TaskType::LocalWorkflow,
                tasks::TaskSpawnInput::LocalWorkflow {
                    workflow_id: spec.name.clone().unwrap_or_default(),
                    script,
                    resume_from_run_id: spec.resume_from_run_id.clone(),
                    // The `args` global, serialised to a JSON string for the runtime.
                    args: spec
                        .args
                        .as_ref()
                        .map(|v| serde_json::to_string(v).unwrap_or_default()),
                },
                "Workflow".to_string(),
            )
            .await
            .map_err(|e| tool_workflow::WorkflowLaunchError(e.to_string()))?;
        Ok(tool_workflow::WorkflowLaunched { task_id })
    }
}

/// Register the desktop tool set into an existing (empty) registry.
///
/// Each `tool_*::register_all` consumes a clone of `ctx`; the final crate
/// takes ownership to avoid a redundant clone.
///
/// When `coordinator` is `Some(..)` (a coordinator-capable session), the
/// coordinator `TeamCreate` / `TeamDelete` / `SendMessage` tools are registered
/// IN PLACE OF their builtin namesakes: `tool_team::register_all` is SKIPPED
/// entirely (it registers exactly `TeamCreate` + `TeamDelete`), and
/// `tool_ui::register_all_except_send_message` drops the builtin `SendMessage`,
/// so the richer coordinator versions are the ONLY ones with those names. This
/// keeps exactly ONE of each in the registry (no silent shadow — `find_by_name`
/// is builtin-first — and no duplicate name in the system prompt). When `None`,
/// `tool_team::register_all` + the full `tool_ui::register_all` run as before and
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
    // In coordinator mode the richer `coordinator` `SendMessage` (registered
    // below, IN PLACE OF this builtin) carries the swarm routing surface, so we
    // skip the leaner `tool_ui` `SendMessage` here — otherwise, because the
    // registry's `find_by_name` is builtin-first, the earlier `tool_ui` copy
    // would silently shadow the coordinator one. Mirrors the `tool_team`-skip
    // for `TeamCreate` / `TeamDelete`.
    if coordinator.is_some() {
        tool_ui::register_all_except_send_message(reg, ctx.clone());
    } else {
        tool_ui::register_all(reg, ctx.clone());
    }
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
            bus,
            runtime,
        }) => {
            for tool in coordinator::internal_tools::coordinator_internal_tools(
                team, mode, spawn_seam, output, bus, runtime,
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
///   verbatim to `llm_client::ClientConfig` via `build()`. `None` ⟶ built-in
///   profiles only.
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
///     deny_unresolved_ask: false,
///     max_turns: None,
///     max_budget_usd: None,
///     json_schema: None,
///     injected_permission_gate: None,
///     session_started_as_coordinator: false,
///     // `None` ⟶ empty memory (deterministic). A production host injects
///     // `Some(orchestrator::prompt::real_provider())` to load real CLAUDE.md.
///     memory_provider: None,
///     permission_mode: permission::PermissionMode::Default,
///     connect_prompt: None,
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
    /// `llm_client::ClientConfig` via `build()`. `None` ⟶ built-in profiles only.
    pub provider_profiles: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Settings-declared `routing` block as raw JSON, fed verbatim to
    /// `llm_client::ClientConfig` (model aliases / fallback / retry). `None` ⟶
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
    /// HEADLESS deny-on-ask (claude-code `--print` parity). When `true` AND
    /// `use_noop_permission_gate` is set, the inner gate becomes
    /// [`permission::DenyOnAskGate`] instead of the always-allow
    /// `NoOpPermissionGate`: a tool whose policy outcome is an unresolved `Ask`
    /// (a mutating tool with no matching allow rule, in a session with no
    /// interactive prompt) is DENIED rather than allowed. Allow/deny rules, the
    /// active mode, and read-only auto-allow are still resolved by
    /// `PolicyPermissionGate` first. Defaults to `false` → the prior always-allow
    /// inner, so interactive/transport builds and every existing caller stay
    /// byte-identical. The CLI sets this from `argv.print`.
    pub deny_unresolved_ask: bool,
    /// CLI `--max-turns N`: cap on agent turns, mapped to
    /// [`orchestrator::OrchestratorConfig::max_turns`] in `build()`. `None` (the
    /// default) = unbounded.
    pub max_turns: Option<u32>,
    /// CLI `--max-budget USD`: cost ceiling in USD, mapped to
    /// [`orchestrator::OrchestratorConfig::max_budget_nano_usd`] (× 1e9) in
    /// `build()`. `None` (the default) = no cap.
    pub max_budget_usd: Option<f64>,
    /// CLI `--json-schema <schema>`: when set, `build()` forces structured
    /// output — it registers a `StructuredOutput` tool whose `input_schema` is
    /// this schema, forces `tool_choice` to it, and surfaces a capture slot on
    /// [`DesktopRuntime`] for the print path to validate + retry. `None` (the
    /// default) leaves every turn unconstrained (byte-identical to before).
    pub json_schema: Option<serde_json::Value>,
    /// Host-injected base permission gate (the INTERACTIVE prompt transport).
    /// When `Some`, `build()` uses it as the base gate instead of the
    /// `NoOpPermissionGate`/`DenyOnAskGate`/`AdapterPermissionGate` it would
    /// otherwise select — still wrapped by `PolicyPermissionGate` when
    /// enforcement is on (the CLI default), so rules + the active mode +
    /// read-only auto-allow resolve first and only an unresolved `Ask` reaches
    /// the injected prompt. The interactive TUI injects a
    /// `tui::TuiPermissionGate` here so an `Ask` surfaces as a dialog; `None`
    /// (the default + every headless/transport caller) keeps the prior
    /// selection, byte-identical.
    pub injected_permission_gate: Option<Arc<dyn PermissionGate>>,
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
    /// Plan 3c: host secure-input port for `/connect <api-key-provider>`. The tui
    /// supplies its masked-input widget; `None` → a headless no-op prompt
    /// (`crate::connect::NoopKeyPrompt`) that cancels.
    pub connect_prompt: Option<Arc<dyn crate::connect::SecureKeyPrompt>>,
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
            .field("deny_unresolved_ask", &self.deny_unresolved_ask)
            .field(
                "injected_permission_gate",
                if self.injected_permission_gate.is_some() {
                    &"Some(<gate>)"
                } else {
                    &"None"
                },
            )
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
            .field(
                "connect_prompt",
                if self.connect_prompt.is_some() {
                    &"Some(<prompt>)"
                } else {
                    &"None"
                },
            )
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
            deny_unresolved_ask: false,
            max_turns: None,
            max_budget_usd: None,
            json_schema: None,
            injected_permission_gate: None,
            session_started_as_coordinator: false,
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
            connect_prompt: None,
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
    connect_writer: Arc<dyn command_core::ConnectCredentialWriter>,
    connect_copilot: Arc<dyn command_core::CopilotConnectDriver>,
    connect_chatgpt: Arc<dyn command_core::ChatGptConnectDriver>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    // Plan 3c: wire `/connect` over the engine-supplied credential-writer +
    // Copilot device-flow + ChatGPT OAuth seams.
    command_core::register::register_core_connect(&mut reg, connect_writer, connect_copilot, connect_chatgpt);
    // Desktop-only command handlers (no-op in M8 — the names remain
    // command-core unimplemented stubs until future milestones fill them).
    command_desktop::register(&mut reg);
    // SLASH.2: discover + register custom `.claude/commands/**.md` commands
    // (project up to git-root/home, plus user + managed layers), the same
    // layering claude-code's getCommands uses. Registered AFTER builtins so a
    // same-named custom command shadows a builtin (TS findCommand order).
    let home = dirs::home_dir().unwrap_or_else(|| claude_home.to_path_buf());
    let managed_dir = crate::settings_watch::managed_settings_dir();
    let registered = command_core::load_and_register_custom_commands(
        &mut reg,
        cwd,
        claude_home,
        &managed_dir,
        &home,
    )
    .await;
    let registered_skills = command_core::load_and_register_skill_commands_with_roots(
        &mut reg,
        cwd,
        claude_home,
        Some(&managed_dir),
        &home,
        &[],
    )
    .await;
    reg.register_builtin_handler(Arc::new(command_core::SkillsHandler::with_all_roots(
        cwd.to_path_buf(),
        claude_home.to_path_buf(),
        Some(managed_dir),
        Vec::new(),
    )));
    tracing::debug!(
        custom_commands = registered,
        skill_commands = registered_skills,
        "registered custom slash commands"
    );
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
    /// Shared Claude.ai subscription snapshot (Task 4). Seeded at build time
    /// with the scope-derived `is_subscriber` flag; for subscribers a
    /// background OAuth profile + roles fetch overwrites it with the full
    /// tier/billing/role snapshot once the endpoints respond. UI layers read
    /// it at compose time and treat `None` / a poisoned lock as the
    /// conservative default snapshot.
    pub subscription: traits::subscription::SharedSubscription,
    /// Phase 2a §6.2: per-`profile_name` availability flag driving the `/model`
    /// picker's Connect badge (a sibling map, NOT a field on the frozen
    /// `ModelListing`). The tui joins it by provider/profile name.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
    /// Phase 2a I1/I2: authoritative `request_model -> (profile_name,
    /// provider_label)` map assembled from the LIVE multi-provider
    /// `ClientConfig.providers` (every profile's `models[].request_model`). The
    /// tui joins it in the `/model` picker so a bare available-model id from a
    /// USER-defined provider resolves to its OWN provider group and gates on
    /// `provider_availability` — instead of mis-falling into `"builtin"`/`true`.
    /// Built-in CATALOG rows are unaffected (they group via the orchestrator's
    /// `list_model_listings`).
    pub model_providers: std::collections::BTreeMap<String, (String, String)>,
    /// Phase 2a: the concrete routing adapter, surfaced read-only so host/tests
    /// can inspect the wired fallback chains.
    pub provider_adapter: Arc<ProviderApiAdapter>,
    /// Phase 2a C1: the shared credential manager (keychain-backed). Surfaced so
    /// the host can thread it onto the TUI App, where the `/connect` screen
    /// persists a collected key (`CredentialManager::set_provider_key`). Same
    /// `Arc` the orchestrator already holds — no second store is constructed.
    pub credentials: Arc<secret::CredentialManager>,
    /// Structured-output capture slot — `Some` only when `--json-schema` is set
    /// (`DesktopConfig.json_schema`). The forced `StructuredOutput` tool writes
    /// the model's result here; the print path reads it after each turn to
    /// validate against the schema and retry. `None` for every normal run.
    pub structured_output_slot: Option<orchestrator::structured_output::StructuredOutputSlot>,
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
    /// Secure-storage backend initialization failed.
    #[error("secure storage init failed: {0}")]
    SecureStorage(String),
    /// `sandbox.enabled` and `sandbox.failIfUnavailable` are both set, but the
    /// sandbox cannot run on this host (unsupported platform / WSL1 / missing
    /// deps / platform not in `sandbox.enabledPlatforms`). Faithful to
    /// claude-code's `isSandboxRequired()` startup refusal (sandbox-adapter.ts:479)
    /// — refusing rather than silently ignoring the operator's security posture
    /// (issue #34044).
    #[error("sandbox required but unavailable: {0}")]
    SandboxUnavailable(String),
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
fn oauth_subscriber_flag(
    api_key_present: bool,
    auth_token_present: bool,
    scopes: &[String],
) -> bool {
    !api_key_present && !auth_token_present && anthropic_oauth::subscription_from_scopes(scopes)
}

/// Fold the profile + roles responses into the shared snapshot. Pure —
/// unit-tested without IO. Tier mapping mirrors
/// `OAuthProfileResponse::subscription_type()` (TS string union values);
/// `Free`/`Unknown` resolve to `None` (conservative, same as the TS `null`;
/// note `subscription_type()` never actually returns those variants today, so
/// that arm is purely defensive).
fn subscription_snapshot_from(
    is_subscriber: bool,
    profile: Option<&anthropic_oauth::OAuthProfileResponse>,
    roles: Option<&anthropic_oauth::UserRolesResponse>,
) -> traits::subscription::SubscriptionSnapshot {
    use anthropic_oauth::SubscriptionType;
    let org = profile.and_then(|p| p.organization.as_ref());
    let subscription_type = profile
        .and_then(anthropic_oauth::OAuthProfileResponse::subscription_type)
        .and_then(|t| match t {
            SubscriptionType::Pro => Some("pro"),
            SubscriptionType::Max => Some("max"),
            SubscriptionType::Team => Some("team"),
            SubscriptionType::Enterprise => Some("enterprise"),
            SubscriptionType::Free | SubscriptionType::Unknown => None,
        });
    traits::subscription::SubscriptionSnapshot {
        is_subscriber,
        subscription_type: subscription_type.map(str::to_owned),
        rate_limit_tier: org.and_then(|o| o.rate_limit_tier.clone()),
        has_extra_usage_enabled: org.and_then(|o| o.has_extra_usage_enabled) == Some(true),
        billing_type: org.and_then(|o| o.billing_type.clone()),
        organization_role: roles.and_then(|r| r.organization_role.clone()),
    }
}

// Phase 2a: the multi-provider client config / chains / credential sources /
// pricing catalog are now assembled by `provider_config::assemble` (which owns
// the byte-equivalent Anthropic profile + the builtin catalog presets + the
// settings-`providers` merge). The old single-Anthropic `builtin_anthropic_config`
// / `apply_settings_providers` / `parse_routing_overrides` helpers from
// `platform_common::llm_config` are no longer wired into `build()`; they remain
// in `platform_common` and are still exercised by the e2e tests below via their
// fully-qualified `platform_common::` paths.

/// (Phase 2a I1/I2) Human provider header for an assembled profile name, used to
/// label the engine's `model_providers` map the `/model` picker joins. Mirrors
/// the orchestrator catalog's `provider_label` for the built-in profiles
/// (`list_model_listings` parity) and Title-Cases an unknown USER profile name
/// (e.g. `groq` -> `Groq`, `my-provider` -> `My Provider`) so a user-defined
/// provider reads cleanly in its own group.
fn provider_profile_label(profile_name: &str) -> String {
    match profile_name {
        "anthropic" => "Anthropic".to_string(),
        "openrouter" => "OpenRouter".to_string(),
        "deepseek" => "DeepSeek".to_string(),
        "glm-coding" => "GLM (coding)".to_string(),
        "zai" => "Z.AI".to_string(),
        "openai" => "OpenAI".to_string(),
        "openai-chatgpt" => "OpenAI (ChatGPT login)".to_string(),
        "github-copilot" => "GitHub Copilot".to_string(),
        other => other
            .split(['-', '_', ' '])
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut chars = w.chars();
                match chars.next() {
                    Some(first) => {
                        first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// First-party Anthropic models the engine routes by default, plus the
/// configured `default_model` / `fallback_model` and any env-configured
/// small-fast / haiku model a `prompt` hook may resolve to. The `llm_client`
/// registry resolves a request model by exact id, so every model the host may
/// request must appear here.
fn anthropic_models_for(
    default_model: &str,
    fallback_model: Option<&str>,
) -> Vec<llm_client::ModelProfile> {
    let caps = llm_client::Capabilities {
        streaming: true,
        tools: true,
        vision: true,
        documents: true,
        reasoning: true,
        structured_output: true,
    };
    let mut ids: Vec<String> = vec![
        "claude-opus-4-6".to_string(),
        "claude-opus-4-5-20251101".to_string(),
        "claude-opus-4-1-20250805".to_string(),
        "claude-opus-4-20250514".to_string(),
        "claude-sonnet-4-6".to_string(),
        "claude-sonnet-4-5-20250929".to_string(),
        "claude-haiku-4-5".to_string(),
    ];
    ids.push(default_model.to_string());
    if let Some(fb) = fallback_model {
        ids.push(fb.to_string());
    }
    // Env-configured small-fast / haiku model a `prompt` hook may resolve to
    // (matching `hook_prompt_runner::resolve_model`'s precedence:
    // `ANTHROPIC_SMALL_FAST_MODEL` > `ANTHROPIC_DEFAULT_HAIKU_MODEL` > default
    // Haiku), so such a request resolves instead of failing `ModelUnavailable`.
    // The default Haiku id (`claude-haiku-4-5`) is already in the list above.
    for var in ["ANTHROPIC_SMALL_FAST_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL"] {
        if let Ok(m) = std::env::var(var) {
            if !m.is_empty() {
                ids.push(m);
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|id| llm_client::ModelProfile {
            display_model: id.clone(),
            request_model: id.clone(),
            billing_model: id,
            aliases: Vec::new(),
            capabilities: caps,
        })
        .collect()
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

/// Read the merged `settings.enabledPlugins` allowlist (`plugin@marketplace` →
/// enabled) from the user then project `settings.json`, project last so it wins
/// on conflict. Mirrors `loadPluginsFromMarketplaces`'s
/// `{...getAddDirEnabledPlugins(), ...settings.enabledPlugins}` merge
/// (`pluginLoader.ts:1898`) at the priority that matters for the cache-only
/// boot. Malformed files / a missing key degrade to an empty map (no plugins),
/// matching claude-code's resilient read-only boot.
async fn load_enabled_plugins(
    claude_home: &std::path::Path,
    cwd: &std::path::Path,
) -> std::collections::BTreeMap<String, bool> {
    let mut merged: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
    let user = claude_home.join("settings.json");
    let project = cwd.join(".claude").join("settings.json");
    // User first, project second → project overrides on identical keys.
    for path in [user, project] {
        let Ok(raw) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            tracing::warn!(path = %path.display(), "skipping malformed settings.json for enabledPlugins");
            continue;
        };
        if let Some(map) = json.get("enabledPlugins").and_then(|v| v.as_object()) {
            for (k, v) in map {
                if let Some(b) = v.as_bool() {
                    merged.insert(k.clone(), b);
                }
            }
        }
    }
    merged
}

/// SKILLLIST.1: `CommandRegistry`-backed skill-listing provider for the per-turn
/// `skill_listing` system-reminder. Reads the shared registry lazily at turn time
/// and applies the TS `getSkillToolCommands` eligibility filter
/// (`commands.ts:565-583`): model-invocable prompt skills, excluding builtins,
/// keeping bundled/skills/deprecated-dir entries plus any with a user-specified
/// description or `whenToUse`.
/// Collects the `system_message` of each completed background (`async`) hook so
/// the orchestrator's per-turn `async_hook_response` reminder can fold them into
/// the next turn (claude-code `getAsyncHookResponseAttachments`). CONSUME-ONCE:
/// [`AsyncHookResponseProvider::take_pending_responses`] drains the buffer
/// (mirrors TS `removeDeliveredAsyncHooks`). A plain `std::sync::Mutex` — every
/// critical section is a brief push / `mem::take`, never held across an `await`.
#[derive(Clone, Default)]
struct AsyncHookResponseBuffer(Arc<std::sync::Mutex<Vec<String>>>);

impl AsyncHookResponseBuffer {
    fn push(&self, text: String) {
        if let Ok(mut v) = self.0.lock() {
            v.push(text);
        }
    }
}

#[async_trait::async_trait]
impl orchestrator::prompt::async_hook_response::AsyncHookResponseProvider
    for AsyncHookResponseBuffer
{
    async fn take_pending_responses(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

struct RegistrySkillListing(Arc<RwLock<CommandRegistry>>);

#[async_trait::async_trait]
impl orchestrator::prompt::skill_listing::SkillListingProvider for RegistrySkillListing {
    async fn skill_entries(
        &self,
    ) -> Vec<orchestrator::prompt::skill_listing::SkillListingEntry> {
        use command_api::{CommandSource, SlashCommandKind};
        let reg = self.0.read().await;
        reg.model_invocable_commands() // !disable_model_invocation (registry.rs)
            .into_iter()
            // TS `cmd.type === 'prompt'` — markdown/plugin commands, not builtin/mcp.
            .filter(|c| {
                matches!(
                    c.kind,
                    SlashCommandKind::Markdown { .. } | SlashCommandKind::Plugin { .. }
                )
            })
            // TS `cmd.source !== 'builtin'`.
            .filter(|c| c.source != CommandSource::Builtin)
            // TS loadedFrom ∈ {bundled,skills,commands_DEPRECATED} ||
            //    hasUserSpecifiedDescription || whenToUse.
            .filter(|c| {
                matches!(
                    c.loaded_from.as_deref(),
                    Some("bundled" | "skills" | "commands_DEPRECATED")
                ) || c.has_user_specified_description
                    || c.when_to_use.is_some()
            })
            .map(|c| orchestrator::prompt::skill_listing::SkillListingEntry {
                name: c.name.clone(),
                description: c.description.clone(),
                when_to_use: c.when_to_use.clone(),
                // TS `cmd.source === 'bundled'` (prompt.ts) — bundled skills are
                // never truncated; mirror via loadedFrom == "bundled".
                is_bundled: c.loaded_from.as_deref() == Some("bundled"),
            })
            .collect()
    }
}

/// Production [`mcp::oauth::OnAuthorizationUrl`] callback for OAuth-configured
/// remote MCP servers. The MCP OAuth flow ([`mcp::McpRegistry::connect`]) fires
/// this once per interactive flow with the authorization URL the user must visit
/// to grant consent.
///
/// There is no browser-open util in this workspace, so this is best-effort:
/// 1. Log the URL prominently at `info` level (the engine's only surfacing path
///    from this depth — the TUI/transport tails the tracing stream).
/// 2. Attempt a detached OS-native browser open (`open` on macOS, `xdg-open` on
///    Linux, `cmd /c start` on Windows), ignoring any failure.
///
/// Non-panicking and non-blocking: a failed spawn leaves the logged URL as the
/// fallback the user can copy by hand.
fn mcp_on_authorization_url() -> mcp::oauth::OnAuthorizationUrl {
    Arc::new(|url: &str| {
        tracing::info!(
            target: "lingxi::mcp::oauth",
            authorization_url = %url,
            "MCP OAuth: open this URL in a browser to authorize the server:\n  {url}",
        );
        // Best-effort detached browser open; failures are intentionally ignored.
        #[cfg(target_os = "macos")]
        let cmd: Option<(&str, &[&str])> = Some(("open", &[]));
        #[cfg(target_os = "linux")]
        let cmd: Option<(&str, &[&str])> = Some(("xdg-open", &[]));
        #[cfg(target_os = "windows")]
        let cmd: Option<(&str, &[&str])> = Some(("cmd", &["/c", "start", ""]));
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let cmd: Option<(&str, &[&str])> = None;

        if let Some((program, prefix)) = cmd {
            let _ = std::process::Command::new(program)
                .args(prefix)
                .arg(url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    })
}

/// Per-user Claude temp-dir name — port of claude-code `getClaudeTempDirName`
/// (`permissions/filesystem.ts:307-315`): `claude-<uid>` on Unix (the uid keeps
/// per-user dirs apart in a shared `/tmp`). Shares the crate's single
/// [`current_uid`] helper (a SAFE `nix::unistd::getuid` wrapper) rather than a
/// second `getuid` crate, so the sandbox seed and the task-spool dir agree.
fn claude_temp_dir_name() -> String {
    format!("claude-{}", current_uid())
}

/// Base Claude temp dir for the task spool — port of `getClaudeTempDir`
/// (`permissions/filesystem.ts:331-346`): `$CLAUDE_CODE_TMPDIR || /tmp`, joined
/// with [`claude_temp_dir_name`]. (claude resolves symlinks; the spool path only
/// needs to be writable + session-unique, so the realpath step is omitted.)
/// Distinct from the sandbox-seed [`claude_temp_dir`] (which returns the
/// realpath-resolved, trailing-separator String form).
fn claude_temp_dir_path() -> std::path::PathBuf {
    let base = std::env::var_os("CLAUDE_CODE_TMPDIR").map_or_else(
        || std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from,
    );
    base.join(claude_temp_dir_name())
}

/// Sanitize a path string for use as a single dir component — port of
/// `sanitizePath` (`sessionStoragePortable.ts:311-319`): every non-alphanumeric
/// char becomes `-`. (The >255-char hash-suffix branch is omitted; project
/// paths in practice stay well under it.)
fn sanitize_path_component(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Session-scoped task-output dir — port of `getTaskOutputDir`
/// (`diskOutput.ts:50-55`): `<projectTempDir>/<sessionId>/tasks`, where
/// `projectTempDir = <claudeTempDir>/<sanitized-cwd>` (`getProjectTempDir`,
/// `permissions/filesystem.ts:376-378`).
///
/// Session-scoping (vs the old in-repo `<cwd>/.claude/tasks-output`) keeps
/// concurrent sessions in one project from clobbering each other's spools and
/// stops task output from polluting the working tree / git status. Rooting under
/// the project temp dir also makes reads auto-allowed by claude's
/// `checkReadableInternalPath`.
#[must_use]
pub fn session_task_output_dir(cwd: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    claude_temp_dir_path()
        .join(sanitize_path_component(&cwd.to_string_lossy()))
        .join(session_id)
        .join("tasks")
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

    // FIX A/B/C: mint the boot-canonical MAIN session id ONCE and derive the
    // session's transcript path + subagents dir from `(claude_home, cwd, id)`.
    // claude-code's `createBaseHookInput` (utils/hooks.ts:322) ALWAYS stamps
    // `transcript_path: getTranscriptPathForSession(sessionId)` on EVERY hook
    // payload, and `getAgentTranscriptPath` anchors spawned-subagent transcripts
    // under `<projectDir>/<sessionId>/subagents`. The orchestrator generates its
    // own `SessionId` INSIDE `ConversationOrchestrator::new`, so historically no
    // single id was knowable at boot — the leaf firers / subagent spawner (built
    // BEFORE the orchestrator) fired with an EMPTY `transcript_path` / a `/tmp`
    // subdir. We close that by minting the id here and:
    //   - handing it to the orchestrator via `.with_session_id` (so its live
    //     session matches), and to the firers as the precomputed `transcript_path`;
    //   - handing the subagents dir to the spawner via `with_hook_context`.
    // The path helpers live in `orchestrator::transcript_paths` (a facade over
    // `session::jsonl::path`) so this app needs no direct `session` dep.
    let main_session_id = protocol::SessionId::new();
    let main_session_uuid = main_session_id.as_uuid().to_string();
    let main_transcript_path = orchestrator::transcript_paths::main_transcript_path(
        &cfg.claude_home,
        &cwd.to_string_lossy(),
        &main_session_uuid,
    );
    let main_subagents_dir = orchestrator::transcript_paths::subagents_dir(
        &cfg.claude_home,
        &cwd.to_string_lossy(),
        &main_session_uuid,
    );

    // (1) Platform-minimal façade (http + clock + storage).
    let http = Arc::new(PosixHttp::new());
    let clock = Arc::new(PosixClock::new());
    let storage = secure_storage_for_platform(
        std::env::var("USER").unwrap_or_else(|_| "default".to_string()),
        cfg.claude_home.clone(),
        cfg.claude_home.join(".credentials.json"),
    )
    .await
    .map_err(|e| BuildError::SecureStorage(e.to_string()))?;

    // (2a) Task 10: LlmTransportBridge wraps the PosixHttp transport for
    //      `DefaultLlmClient`. A second `PosixHttp` instance is used so the
    //      bridge owns its own (stateless) handle; the original `http` Arc
    //      continues to serve MCP / hooks / side-query.
    let llm_transport: Arc<dyn Transport> =
        Arc::new(LlmTransportBridge::new(PosixHttp::new()));
    // Defer client construction to step 3.1 where we know whether OAuth is
    // active (determines auth strategy + credential config). Placeholder: the
    // resolved OAuth `AuthState` (`Some` only for an OAuth-effective subscriber
    // session) that step (2) bridges into the assembled client's credential
    // seam as an `oauth_delegate`.
    let mut oauth_auth_state: Option<Arc<anthropic_oauth::refresh::AuthState>> = None;
    let mut openai_oauth_state: Option<Arc<openai_oauth::AuthState>> = None;
    // WebSearch builds Anthropic `POST /v1/messages` requests via its own
    // provider (server-side web search is Anthropic-only in v1).
    let tool_provider = Arc::new(AnthropicRequestBuilder::new(
        cfg.api_key.clone(),
        Some(cfg.api_base.clone()),
    ));

    // (3) Credential manager + OAuth client (used by /login, /logout).
    //
    // Task 4 (future-work batch 4): the shared subscription slot UI layers read
    // at compose time. Seeded with the conservative default snapshot here; the
    // `Ok(Some(tokens))` arm below re-seeds it with the resolved subscriber
    // flag, and (for subscribers) a background profile+roles fetch overwrites
    // it with the full snapshot once the endpoints respond.
    let subscription: traits::subscription::SharedSubscription = std::sync::Arc::new(
        std::sync::RwLock::new(Some(traits::subscription::SubscriptionSnapshot::default())),
    );
    // Retain a clone for the MCP OAuth seam (5.26): `CredentialManager::new`
    // moves `storage`, but the registry's `OAuthDeps.storage` needs the SAME
    // platform `Arc<dyn SecureStorage>` for per-server token persistence.
    let mcp_oauth_storage = storage.clone();
    let credentials = Arc::new(CredentialManager::new(storage, clock.clone(), http.clone()));
    let oauth_cfg = ClaudeAiOAuthConfig::default_with_port(0);
    let oauth_client = Arc::new(ClaudeAiOAuthClient::new(
        oauth_cfg.clone(),
        http.clone(),
        credentials.clone(),
    ));
    let auth: Arc<dyn AuthHandle> = Arc::new(OAuthHandle::new(oauth_client));

    // (3.1) M5-13 / Task 10: build the OAuth refresh driver when the keychain
    //        already holds a logged-in OAuth token.  `init_refresh_driver` spawns
    //        the proactive-refresh task and returns the shared `AuthState`.  The
    //        returned state is used BOTH for the old api-client hook path (removed
    //        in Plan 3a Task 9) and to wire `OAuthCredentialProvider` into the
    //        new `DefaultLlmClient` path.
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
            // Re-seed the shared slot with the resolved subscriber flag so
            // readers see it even before (or without) the background
            // profile+roles fetch landing. SECRECY: deliberately copy the
            // access token (a `Secret<String>`, intentionally non-`Clone`) by
            // exposing + re-wrapping — the audited copy pattern — BEFORE the
            // original moves into `init_refresh_driver`; it is exposed again
            // only inside the spawned fetch task.
            if let Ok(mut guard) = subscription.write() {
                *guard = Some(traits::subscription::SubscriptionSnapshot {
                    is_subscriber,
                    ..Default::default()
                });
            }
            let profile_token =
                protocol::Secret::new(tokens.access_token.expose_secret().clone());
            match anthropic_oauth::client::init_refresh_driver(
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
                Ok(auth_state) => {
                    // Task 10: capture the OAuth `AuthState` so the assembled
                    // client gets an `OAuthBearer` credential delegate below.
                    // Only active when the subscriber flag confirms OAuth is the
                    // effective auth source (API-key overrides it).
                    if is_subscriber {
                        oauth_auth_state = Some(auth_state);

                        // Task 4: background OAuth profile + roles fetch. This
                        // closes the RENDERING half of the profile-fetch
                        // PARITY-GAP documented at `orchestrator/src/config.rs:134`
                        // (tier/billing/role data for rate-limit copy), without
                        // touching the build hot path. Both fetchers swallow
                        // every error → `None` (matching the TS `logError` /
                        // `return undefined` stance), so on any failure the
                        // seeded `is_subscriber`-only snapshot simply stays.
                        //
                        // SharedSubscription locking contract (std `RwLock`):
                        // the guard must NEVER be held across an `.await` —
                        // build the full snapshot FIRST, then write-and-drop.
                        // Poisoned-lock stance: writer skips on poison
                        // (`if let Ok(mut guard)`); readers degrade to the
                        // default snapshot. SECRECY: the access token is
                        // exposed (`expose_secret`) only into the two fetch
                        // calls and never logged or formatted.
                        {
                            let slot = subscription.clone();
                            let transport: std::sync::Arc<dyn traits::HttpTransport> =
                                http.clone();
                            // Move (not copy) the token into the task — its
                            // only consumer.
                            let token = profile_token;
                            tokio::spawn(async move {
                                let token = token.expose_secret();
                                let profile = anthropic_oauth::fetch_profile_from_oauth_token(
                                    token, &transport,
                                )
                                .await;
                                let roles =
                                    anthropic_oauth::fetch_user_roles(token, &transport).await;
                                let snap = subscription_snapshot_from(
                                    true,
                                    profile.as_ref(),
                                    roles.as_ref(),
                                );
                                if let Ok(mut guard) = slot.write() {
                                    *guard = Some(snap);
                                }
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to attach OAuth refresh driver; 401 auto-refresh disabled");
                }
            }
        }
        Ok(None) => {
            // No stored OAuth session — API-key path.
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not read OAuth tokens from keychain; skipping refresh-driver wiring");
        }
    }

    // (3.2a) Wire the OpenAI ChatGPT OAuth refresh driver when the keychain
    //        already holds a ChatGPT session. Mirrors the anthropic block above.
    //        Returns `Arc<openai_oauth::AuthState>` for the credential delegate;
    //        on Ok(None) / Err we leave `openai_oauth_state = None` (warn on Err).
    //        No subscriber-flag / profile-fetch needed for OpenAI — minimal path.
    let openai_oauth_cfg = openai_oauth::OpenAiOAuthConfig::default();
    let openai_oauth_client = Arc::new(openai_oauth::OpenAiOAuthClient::new(
        openai_oauth_cfg.clone(),
        http.clone(),
    ));

    // (3.2a-pre) P3 enterprise precedence for the openai-chatgpt credential:
    // PAT env  >  external-tokens env  >  OAuth login session. First hit wins.
    let mut openai_chatgpt_delegate: Option<Arc<dyn llm_client::CredentialProvider>> = None;
    if let Ok(pat) = std::env::var("OPENAI_PERSONAL_ACCESS_TOKEN") {
        if !pat.trim().is_empty() {
            let http_dyn: Arc<dyn traits::HttpTransport> = http.clone() as Arc<dyn traits::HttpTransport>;
            match openai_oauth::whoami(&openai_oauth_cfg, &http_dyn, &pat).await {
                Ok(md) => {
                    openai_chatgpt_delegate = Some(Arc::new(
                        openai_oauth::PatCredentialProvider::new(pat, md),
                    ) as Arc<dyn llm_client::CredentialProvider>);
                }
                Err(e) => tracing::warn!(error = %e, "OPENAI_PERSONAL_ACCESS_TOKEN whoami failed; ignoring PAT"),
            }
        }
    }
    if openai_chatgpt_delegate.is_none() {
        match (
            std::env::var("OPENAI_CHATGPT_ACCESS_TOKEN").ok().filter(|s| !s.trim().is_empty()),
            std::env::var("OPENAI_CHATGPT_ACCOUNT_ID").ok().filter(|s| !s.trim().is_empty()),
        ) {
            (Some(tok), Some(acc)) => {
                openai_chatgpt_delegate = Some(Arc::new(
                    openai_oauth::ExternalTokensCredentialProvider::from_supplied(tok, Some(acc)),
                ) as Arc<dyn llm_client::CredentialProvider>);
            }
            (Some(_), None) | (None, Some(_)) => tracing::warn!(
                "incomplete external ChatGPT tokens: set BOTH OPENAI_CHATGPT_ACCESS_TOKEN and OPENAI_CHATGPT_ACCOUNT_ID"
            ),
            (None, None) => {}
        }
    }

    if openai_chatgpt_delegate.is_none() {
        match credentials.get_openai_oauth_tokens().await {
            Ok(Some(tokens)) => {
                match openai_oauth::client::init_refresh_driver(
                    openai_oauth_cfg.clone(),
                    tokens.access_token,
                    tokens.refresh_token,
                    tokens.expires_at,
                    tokens.account_id,
                    tokens.fedramp,
                    http.clone(),
                    clock.clone(),
                    Some(Arc::new(telemetry::AnalyticsBus::new())),
                    Some(credentials.clone()),
                    Arc::new(PosixRuntime::new()),
                )
                .await
                {
                    Ok(state) => {
                        openai_oauth_state = Some(state);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to attach OpenAI OAuth refresh driver; ChatGPT routing disabled");
                    }
                }
            }
            Ok(None) => {
                // No stored ChatGPT OAuth session.
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not read OpenAI OAuth tokens from keychain; skipping chatgpt refresh-driver wiring");
            }
        }
    }

    // (3.3) Phase 2a §8: assemble the FULL multi-provider client config
    //       (Anthropic + builtin catalog presets + settings `providers`) + chains
    //       + credential sources + pricing catalog, instead of the single-Anthropic
    //       config. `provider_config::assemble` owns the byte-equivalent Anthropic
    //       profile + the catalog merge; the engine bridges OAuth in via a
    //       pre-built delegate so provider-config stays free of an anthropic-oauth
    //       dep. A bad settings entry only emits a warning — the engine still boots
    //       with every well-formed profile (incl. the built-in Anthropic one).
    let has_api_key = !cfg.api_key.is_empty();
    // OAuth bridges into the client ONLY when there is no API key (api-key wins;
    // the single credential slot + `oauth_subscriber_flag` enforce the
    // exclusion). `has_oauth` selects `AuthStrategy::OAuthBearer`, which is what
    // injects the required `oauth-2025-04-20` beta on Anthropic routes.
    let has_oauth = !has_api_key && oauth_auth_state.is_some();
    let oauth_delegate: Option<Arc<dyn llm_client::CredentialProvider>> =
        oauth_auth_state.clone().map(|state| {
            let driver = Arc::new(RefreshDriver::new(state));
            Arc::new(OAuthCredentialProvider::new(driver)) as Arc<dyn llm_client::CredentialProvider>
        });

    let assembled = provider_config::assemble(provider_config::AssembleInputs {
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: anthropic_models_for(&cfg.default_model, cfg.fallback_model.as_deref()),
        anthropic_has_api_key: has_api_key,
        anthropic_has_oauth: has_oauth,
        user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
        routing: cfg.routing.clone(),
    });
    for w in &assembled.warnings {
        tracing::warn!(warning = %w, "provider-config assembly");
    }

    // (Phase 2a I1/I2) Authoritative `request_model -> (profile_name,
    // provider_label)` map for the `/model` picker, built from the assembled
    // multi-provider config BEFORE `client_config` is consumed by `from_config`.
    // Every profile (anthropic + presets + USER providers) contributes its
    // `models[].request_model`. First-profile-wins on a duplicate request_model
    // (anthropic + presets come before user profiles in `assemble`).
    let mut model_providers: std::collections::BTreeMap<String, (String, String)> =
        std::collections::BTreeMap::new();
    for profile in &assembled.client_config.providers {
        let label = provider_profile_label(&profile.profile_name);
        for model in &profile.models {
            model_providers
                .entry(model.request_model.clone())
                .or_insert_with(|| (profile.profile_name.clone(), label.clone()));
        }
    }

    // Task-5 (TPM-C): resolve an optional `profile/model` qualifier in the
    // configured default_model so a shared id routes deterministically on the
    // first turn.  Must run while `assembled.client_config.providers` is still
    // owned (before `from_config` moves it).
    let default_listings: Vec<traits::ModelListing> = assembled
        .client_config
        .providers
        .iter()
        .flat_map(|p| {
            let profile = p.profile_name.clone();
            let label = provider_profile_label(&p.profile_name);
            p.models.iter().map(move |m| traits::ModelListing {
                display_model: m.display_model.clone(),
                request_model: m.request_model.clone(),
                provider_id: profile.clone(),
                provider_label: label.clone(),
            })
        })
        .collect();
    let (default_model_id, default_model_profile) =
        traits::parse_model_ref(&cfg.default_model, &default_listings);

    let mut client = DefaultLlmClient::from_config(assembled.client_config)
        .map_err(|e| BuildError::ApiBase(format!("llm-client config: {e}")))?;
    // §6.1: ONE composite credential slot for ALL providers (anthropic api-key /
    // oauth-delegate + every per-profile credential source).
    let mut oauth_delegates: std::collections::BTreeMap<String, std::sync::Arc<dyn llm_client::CredentialProvider>> = std::collections::BTreeMap::new();
    if let Some(d) = oauth_delegate {
        oauth_delegates.insert("anthropic-oauth".to_string(), d);
    }
    // OAuth login fills the slot only if PAT/external didn't.
    if openai_chatgpt_delegate.is_none() {
        if let Some(state) = openai_oauth_state {
            let driver = std::sync::Arc::new(openai_oauth::RefreshDriver::new(state));
            openai_chatgpt_delegate = Some(std::sync::Arc::new(
                openai_oauth::OpenAiOAuthCredentialProvider::new(driver),
            ) as Arc<dyn llm_client::CredentialProvider>);
        }
    }
    let has_openai_chatgpt = openai_chatgpt_delegate.is_some();
    if let Some(d) = openai_chatgpt_delegate {
        oauth_delegates.insert("openai-chatgpt".to_string(), d);
    }
    let composite = provider_config::MultiCredentialProvider::new(
        credentials.clone(),
        assembled.credential_sources.clone(),
        if has_api_key { Some(cfg.api_key.clone()) } else { None },
        oauth_delegates,
    );
    client = client.with_credential_provider(Arc::new(composite));
    let llm_client = Arc::new(client);

    // 3c-T3: build the cost estimator from the assembled pricing catalog so
    // LlmResponse.cost is populated on every successful decode. The catalog
    // already carries the built-in reference tiers + non-Anthropic preset rows +
    // any settings per-profile pricing overrides folded in by `assemble`. Unpriced
    // / unknown models leave cost = None (never an error).
    let cost_estimator = {
        use llm_client::{CostEstimator, PricingPolicy};
        use orchestrator::cost_wiring::llm_catalog_from_cost;
        let llm_cat = llm_catalog_from_cost(&assembled.pricing);
        Arc::new(CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated))
    };
    let subscriber_state = SubscriberState { is_subscriber, is_enterprise: false };

    // Phase 2a CHAINS BRIDGE: translate the assembled `ChainConfig` into main's
    // richer adapter's `fallback_overrides` shape. `assemble` keys each chain by
    // the request/display model id and carries an ordered list of `ChainEntry`;
    // main's adapter routes by model-id through the multi-provider registry, so the
    // `ChainEntry.provider_id` is informational and is dropped here — the per-entry
    // `model` ids are the fallback chain. Cross-provider routing still works
    // because every provider's models are registered in the assembled
    // `ClientConfig`, so a fallback target on another provider resolves by id.
    let fallback_overrides: std::collections::BTreeMap<String, Vec<String>> = assembled
        .chains
        .chains
        .iter()
        .map(|(key, entries)| (key.clone(), entries.iter().map(|e| e.model.clone()).collect()))
        .collect();
    // Retry override → main's scalar settings_max_retries / settings_backoff_ms.
    let settings_max_retries = assembled.chains.retry.as_ref().map(|r| r.max_attempts);
    let settings_backoff_ms = assembled.chains.retry.as_ref().map(|r| r.backoff_ms);

    // Build the CONCRETE adapter so it can be coerced to BOTH the orchestrator
    // seam (`OrchestratorApiClient`) and the agent seam (`agent::SubagentApiClient`).
    // `ProviderApiAdapter` impls both. The adapter's own `alias_to_display` map is
    // rebuilt from the client's `available_models()` (whose aliases `assemble`
    // already populated from `chains.aliases`), so the alias map needs no separate
    // pass here.
    //
    // Batch-5 Task 3: attach the live subscription slot (filled by the background
    // profile/roles fetch) so the drive loops read subscriber/enterprise state at
    // call time — `subscriber_state` remains the build-time seed/fallback.
    // M7: one analytics bus shared by the provider adapter (`tengu_api_*`) and the
    // orchestrator (`tengu_cost_recorded`) so all live telemetry lands on the same
    // sink set — 1:1 with claude-code, where `logEvent` is a single global pipeline.
    let analytics_bus = Arc::new(telemetry::AnalyticsBus::new());
    // metadata.user_id (getAPIMetadata, claude.ts:519): the JSON-string identity
    // `{...extra, device_id, account_uuid, session_id}`. `device_id` =
    // getOrCreateUserID (persisted, stable per install); `session_id` = the main
    // session id (claude-code's getSessionId()); `account_uuid` = "" — the OAuth
    // profile carrying the account UUID is fetched asynchronously in the
    // background and is not available at construction, so this is the faithful
    // `getOauthAccountInfo()?.accountUuid ?? ''` fallback (the value is per-account
    // and never byte-matches claude-code regardless).
    let request_metadata = llm_client::RequestMetadata {
        user_id: ProviderApiAdapter::build_api_metadata_user_id(
            &migrations::global_config::get_or_create_user_id(),
            "",
            &main_session_uuid,
        ),
    };
    let provider_adapter_built = ProviderApiAdapter::new_with_routing(
        llm_client,
        llm_transport,
        subscriber_state,
        UserAgentEnv::from_process_env(),
        env!("CARGO_PKG_VERSION"),
        Some(analytics_bus.clone()),
        cfg.fallback_model.clone(),
        Some(cost_estimator),
        fallback_overrides,
        settings_max_retries,
        settings_backoff_ms,
    )
    .with_subscription(subscription.clone())
    .with_request_metadata(request_metadata);
    // `--json-schema` structured output: FORCE the `StructuredOutput` tool so the
    // model returns its final result through it (1:1 with claude-code). Untouched
    // for every normal turn (`json_schema` is `None`).
    let provider_adapter_built = if cfg.json_schema.is_some() {
        provider_adapter_built.with_forced_tool_choice(llm_client::ToolChoice::Tool {
            name: orchestrator::structured_output::STRUCTURED_OUTPUT_TOOL_NAME.to_string(),
        })
    } else {
        provider_adapter_built
    };
    let provider_adapter = Arc::new(provider_adapter_built);
    let provider_adapter_handle = provider_adapter.clone();
    let api_client: Arc<dyn OrchestratorApiClient> = provider_adapter.clone();
    let subagent_api: Arc<dyn agent::SubagentApiClient> = provider_adapter;

    // (4) Orchestrator config from `cfg` (was `argv.model`).
    let mut orch_cfg = OrchestratorConfig::default();
    // TPM-C: use the bare id produced by parse_model_ref (strips a profile/ prefix
    // so a qualified default_model like "openai/gpt-4o" never reaches the wire).
    orch_cfg.model.clone_from(&default_model_id);
    // Opus-fallback hop: thread the (already print-mode-gated) fallback model
    // into `OrchestratorConfig.fallback_model`. `None` keeps the turn_loop's
    // 529-overload interception a strict no-op (`turn_loop.rs:496`).
    orch_cfg.fallback_model.clone_from(&cfg.fallback_model);
    // CLI `--max-turns` / `--max-budget` caps. Unset leaves the OrchestratorConfig
    // defaults (unbounded turns / no cost cap). USD → nano-USD for the cost cap.
    if let Some(max_turns) = cfg.max_turns {
        orch_cfg.max_turns = max_turns;
    }
    // Structured output forces `tool_choice` to `StructuredOutput`, which compels
    // the model to call it on EVERY assistant turn — so cap the turn at a SINGLE
    // model call: the model calls the tool once (capturing the result), then the
    // cap ends the turn. The print path reads the captured slot regardless of the
    // resulting MaxTurns stop and drives its own validate/retry loop. (Overrides
    // any `--max-turns` here; structured output is inherently one-shot per turn.)
    if cfg.json_schema.is_some() {
        orch_cfg.max_turns = 1;
    }
    orch_cfg.max_budget_nano_usd = cfg.max_budget_usd.map(|usd| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let nano = (usd.max(0.0) * 1_000_000_000.0) as u64;
        nano
    });
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
    // OUTSTYLE.3: custom output-style search dirs — user (`~/.claude/output-styles`)
    // then project (`<cwd>/.claude/output-styles`), in increasing priority so a
    // project style overrides a user one and both override the builtins. A
    // `settings.outputStyle` naming a disk style now activates it
    // (`outputstyles::resolve_output_style`); absent dirs ⇒ builtin-only.
    orch_cfg.output_style_dirs = vec![
        cfg.claude_home.join("output-styles"),
        cfg.cwd.join(".claude").join("output-styles"),
    ];

    // (4.5) One CostTracker per process. The persist channel drains into a
    //       fire-and-forget task that discards snapshots (on-disk persistence is
    //       later work). Depth 64 absorbs bursts without blocking.
    let (cost_persist_tx, mut cost_persist_rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        while cost_persist_rx.recv().await.is_some() {}
    });
    // Phase 2a T7: the CostTracker uses the SAME assembled pricing catalog the
    // estimator was built from (built-in reference tiers + non-Anthropic preset
    // rows + settings overrides), not a fresh `builtin_reference()`, so session
    // cost accounting matches per-response cost estimation.
    let cost_tracker = Arc::new(cost::CostTracker::new(
        protocol::SessionId::new(),
        Arc::new(assembled.pricing),
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
    let subagent_pool = Arc::new(agent::StateMachinePool::new(
        Arc::new(PosixRuntime::new()),
        4,
    ));
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
    // G4: boot-stable session id stamped on the child runner's SubagentStart
    // HookContext (the runtime orchestrator session id is not available at boot;
    // this is cosmetic on the SubagentStart wire payload — the additionalContext
    // collection keys on agent_id/agent_type, set by the runner per spawn).
    let subagent_hook_session_id = protocol::SessionId::new();
    let subagent_spawner_concrete = agent::PoolSubagentSpawner::new(subagent_pool)
        .with_api_client(subagent_api)
        // #15: the parent model handed to the spawner must be the RESOLVED
        // main-loop wire id (claude `getMainLoopModel()`), NOT the raw alias —
        // `orch_cfg.model` is `cfg.default_model` with only a `profile/` prefix
        // stripped, so an `opusplan`/`sonnet` install leaves it a bare alias. An
        // `AgentModel::Inherit` spawn in DEFAULT mode returns the parent verbatim,
        // which would be a bogus wire id that fails at the provider. Resolve it
        // here; the raw alias is still threaded via `with_model_setting` below for
        // the plan-mode `opusplan→Opus` swap.
        .with_default_model(agent::model_resolution::resolve_user_specified_model(
            &orch_cfg.model,
        ))
        // #15: thread the live permission mode + the RAW user model setting
        // (e.g. "opusplan" / "haiku" — `cfg.default_model` is claude-code's
        // `getUserSpecifiedModelSetting()`, the UN-resolved alias) into the
        // spawner so `resolve_agent_model`'s `getRuntimeMainLoopModel` branch
        // actually fires for an `AgentModel::Inherit` spawn: an `opusplan` install
        // in plan mode resolves the subagent to Opus (not the resolved Sonnet
        // main-loop model). Without these the Inherit branch returns the parent
        // model unchanged (default mode → byte-identical to before this seam).
        .with_permission_mode(cfg.permission_mode)
        .with_model_setting(cfg.default_model.clone())
        // G4/G5: stamp the session id + cwd on the `HookContext` the child runner
        // builds for the SubagentStart fire (the orchestrator's hook context is
        // session-scoped at runtime; the spawner uses a boot-stable session id —
        // the field is cosmetic on the wire payload, the load-bearing
        // agent_id/agent_type are set by the runner per spawn).
        // FIX C: also thread the boot-computed MAIN-session subagents dir
        // (`…/projects/<sanitize(cwd)>/<main_session>/subagents`) so each spawned
        // child's `agent_transcript_path` (the agent-scoped `SubagentStop` field)
        // resolves to the real `…/subagents/agent-<id>.jsonl` (claude-code
        // `getAgentTranscriptPath`) instead of the prior `/tmp` placeholder. The
        // subdir keys on the MAIN session id (claude `getSessionId()`), NOT the
        // cosmetic `subagent_hook_session_id`.
        .with_hook_context(
            subagent_hook_session_id,
            cwd.clone(),
            Some(main_subagents_dir.clone()),
        )
        // 2.1.186: append the subagent `<env>` block (`tIm`) after the `Notes:`
        // trailer on every NON-fork spawn. The renderer probes the boot-stable
        // environment once (cwd/git/platform/shell/OS) via the orchestrator's own
        // helpers and fills in the spawn's resolved model id per call. Lives at the
        // composition root because the `agent` crate cannot reach
        // `orchestrator::prompt` (dep cycle).
        .with_subagent_env_renderer(std::sync::Arc::new(
            orchestrator::prompt::subagent_env::boot_renderer(cwd.clone()),
        ));
    let subagent_tool_registry_cell = subagent_spawner_concrete.tool_registry_handle();
    let subagent_agent_catalog_cell = subagent_spawner_concrete.agent_catalog_handle();
    // G4/G5: grab the set-once hook-executor + skill-loader cells BEFORE boxing,
    // to fill once the `HookExecutorImpl` (5.25) and shared command registry exist
    // (same cycle-break as the tool-registry / agent-catalog cells above).
    let subagent_hook_executor_cell = subagent_spawner_concrete.hook_executor_handle();
    let subagent_skill_loader_cell = subagent_spawner_concrete.skill_loader_handle();
    // FIX 1 (subagent pool): grab the set-once tool-wide-deny-names cell BEFORE
    // boxing, to fill once the permission policy is built (same cycle-break as
    // the registry/catalog/hook cells). Filled inside the enforcement branch
    // below from `policy.tool_wide_deny_names()`; left empty otherwise ⇒ the
    // subagent tool pool is unfiltered (byte-identical to before).
    let subagent_tool_wide_deny_cell = subagent_spawner_concrete.tool_wide_deny_names_handle();
    // Box ONCE as the concrete `Arc<PoolSubagentSpawner>` so it can serve as
    // BOTH the one-shot `SubagentSpawner` and the persistent/resume
    // `StreamingSubagentSpawner` (Phase-1 seam) — the LocalAgent handler needs
    // the streaming half to make a backgrounded agent "come to rest" + resume.
    let subagent_spawner_arc = Arc::new(subagent_spawner_concrete);
    let subagent_spawner: Arc<dyn traits::subagent_spawn::SubagentSpawner> =
        subagent_spawner_arc.clone();
    let subagent_streaming_spawner: Arc<dyn agent::StreamingSubagentSpawner> =
        subagent_spawner_arc;

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
        if let Some(injected) = cfg.injected_permission_gate.clone() {
            // INTERACTIVE prompt transport (the TUI's `TuiPermissionGate`): used
            // as the base gate, still wrapped by `PolicyPermissionGate` below when
            // enforcement is on, so an unresolved `Ask` surfaces as a dialog. No
            // `AdapterPermissionGate` handle (that is the bridge transport's gate).
            (injected, None)
        } else if cfg.use_noop_permission_gate {
            // HEADLESS deny-on-ask (`--print` parity): a non-interactive session
            // has no prompt to surface an unresolved `Ask`, so deny it instead of
            // allowing. `PolicyPermissionGate` (wrapped below when enforcement is
            // on — the CLI default) still resolves allow/deny rules + read-only
            // auto-allow BEFORE delegating here, so only an otherwise-unresolved
            // mutating ask is denied. Defaults off → the prior always-allow inner.
            if cfg.deny_unresolved_ask {
                (Arc::new(permission::DenyOnAskGate), None)
            } else {
                (Arc::new(NoOpPermissionGate), None)
            }
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
    // All three claude-code MCP scopes (precedence local > project > user):
    // project `.mcp.json` (mcp_paths[0]), user + local both inside the global
    // config `~/.claude.json` (mcp_paths[1]); local is keyed by the canonical
    // project key for `cwd`.
    let mcp_configs = mcp::load_mcp_servers(&project_mcp_path, &global_mcp_path, &cwd);
    // Build one concrete `PosixMcpTransport` and hand it to the registry as
    // BOTH the `McpTransport` (discovery) and the `RawConnectionProvider`
    // (live-client bridge), so a connected server yields a working `McpClient`
    // via `get_client`. The real transport is now wired: for a connected
    // server its `RawConnectionProvider::connection_for` returns the live
    // `Arc<jsonrpc::Connection>` (Stdio/Sse/Http), so the bridge hands back a
    // working client; only an unknown connection id yields `None`.
    let posix = Arc::new(PosixMcpTransport::new());
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
    // (PERM.1) Enforce permissions BY DEFAULT on the CLI/desktop path (parity
    // §0.1 / §B). claude-code's default mode enforces deny/allow rules + the active
    // permission mode + sandbox-auto-allow; only the explicit
    // `--dangerously-skip-permissions` (BypassPermissions) opts out into allow-all.
    // We mirror that: wrap the base gate with `PolicyPermissionGate` unless either
    // (a) the env escape hatch `LINGXI_ENFORCE_PERMISSIONS` is explicitly set falsey
    // (`0|off|false|no|""`), or (b) the session is in BypassPermissions mode
    // (already root/Docker-guarded upstream by `enforce_bypass_safety`).
    //
    // Scope: when the env var is UNSET, default-on applies only to the
    // `NoOpPermissionGate` (CLI/desktop) inner — the path finding §0.1 is about
    // (allow-all). Transport hosts (the bridge-server, `use_noop=false`) bind the
    // connection-scoped `AdapterPermissionGate`, whose remote client IS the
    // enforcement; they keep the prior env-opt-in behavior so their transport-driven
    // semantics are unchanged. An explicit env value still overrides either way.
    //
    // Inner-gate selection (the `(perms, adapter_gate)` match at :1836):
    // - INTERACTIVE TUI sessions inject `tui::permission_bridge::TuiPermissionGate`
    //   via `cfg.injected_permission_gate` (the `if let Some(injected)` arm), so an
    //   unresolved mutating `Ask` (a `DenyByDefault` tool with no matching rule)
    //   surfaces the permission dialog instead of silently resolving — wired by
    //   `build_runtime_for_tui` → root permission pump.
    // - The HEADLESS `-p`/`--print` and `--no-tui` stdio REPL paths have no dialog
    //   to surface a prompt, so they keep the `NoOpPermissionGate` (always-allow) or
    //   `DenyOnAskGate` (deny-on-ask) inner per `use_noop_permission_gate` /
    //   `deny_unresolved_ask`. Either way deny rules + modes are enforced below.
    let enforce_permissions = should_enforce_permissions(
        std::env::var("LINGXI_ENFORCE_PERMISSIONS").ok().as_deref(),
        cfg.use_noop_permission_gate,
        cfg.permission_mode,
    );
    // Read(deny) → search-exclude globs (GrepTool.ts:417-427, glob.ts lLa()).
    // Populated inside the enforcement branch below from the boot policy and
    // threaded into the tool ctx so `Grep`/`Glob` skip denied/sensitive paths.
    // Empty (no enforcement / no Read-deny rule) ⇒ VCS-only behavior unchanged.
    let mut read_deny_exclude_globs: Vec<String> = Vec::new();
    let perms: Arc<dyn PermissionGate> = if enforce_permissions {
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
            // (#34) Extra working dirs from `permissions.additionalDirectories`,
            // unioned across tiers (claude-code `TGd` folds each tier's
            // `additionalDirectories` into `additionalWorkingDirectories`, which
            // `b$` unions with cwd for the `kF` acceptEdits auto-allow set). The
            // sole populator of `PermissionPolicy::additional_working_dirs`; without
            // it an acceptEdits write under an `additionalDirectories` entry ASKS
            // instead of auto-allowing. Entries stay RAW (relative / `~` / absolute);
            // `authorize` resolves them against `roots` via `expand_path` (same as
            // claude-code's `LXr` path resolution).
            let mut additional_working_dirs: Vec<std::path::PathBuf> = Vec::new();
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
                    // (#34) Union this tier's additionalDirectories into the
                    // working-dir set (claude-code merges across SETTING_SOURCES).
                    additional_working_dirs
                        .extend(permission::additional_directories_from_settings_json(&raw));
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
            //
            // Managed (policySettings) tier — HIGHEST priority, appended LAST so
            // the sandbox-auto-allow fold (last write wins) lets a managed
            // `sandbox.*` override user/project/local (SETTING_SOURCES:
            // …→localSettings→flagSettings→policySettings). This is for the
            // SANDBOX-AUTO-ALLOW derivation ONLY: it is built on a clone, so the
            // managed raw text is NOT injected into `raw_tiers` (which feeds no
            // rule parsing here — rules use `rules`/`mode`/`bypass_disabled`
            // accumulated above). Managed permission RULES are a separate concern
            // (spec §6) and are deliberately NOT loaded here.
            let sandbox_raw_tiers: Vec<String> = {
                let mut v = raw_tiers.clone();
                v.extend(crate::settings_watch::managed_settings_raw_tiers().await);
                v
            };
            let raw_tier_refs: Vec<&str> =
                sandbox_raw_tiers.iter().map(String::as_str).collect();
            let sandbox_auto_allow =
                sandbox_auto_allow_from_settings_tiers(&raw_tier_refs, &cwd);
            // CLI-resolved mode is the highest-priority source (TS orderedModes:
            // the CLI flag / --permission-mode outranks the settings defaultMode).
            // Apply it only when the CLI actually requested a non-default mode, so
            // an unset CLI keeps the settings defaultMode computed above.
            if cfg.permission_mode != permission::PermissionMode::Default {
                mode = cfg.permission_mode;
            }
            let mut policy = permission::PermissionPolicy::from_rules(mode, rules)
                .with_roots(roots)
                .with_working_dirs(additional_working_dirs)
                .with_sandbox_runtime(sandbox_auto_allow);
            policy.bypass_killswitch_active = bypass_disabled;
            // Resolve the active Read(deny) rules to search-exclude globs while
            // the policy is still in scope (before it moves into the gate).
            read_deny_exclude_globs =
                permission::read_deny_exclude_globs(&policy, &cwd);
            // FIX 1 (subagent pool): hand the policy's TOOL-WIDE deny names to the
            // subagent spawner so a blanket-denied tool is stripped from each
            // child's advertised pool too (claude-code `assembleToolPool` →
            // `filterToolsByDenyRules`). Set-once; only meaningful when there are
            // tool-wide deny rules (empty otherwise ⇒ no child-pool filtering).
            let _ = subagent_tool_wide_deny_cell.set(policy.tool_wide_deny_names());
            let policy = Arc::new(policy);
            tracing::info!(
                rules = rule_count,
                mode = ?mode,
                "permission enforcement enabled (default on; disable with LINGXI_ENFORCE_PERMISSIONS=0)"
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
    //        The hooks Agent arm IS now wired via `.with_agent_spawner(..)`
    //        (see the builder chain below): an `agent`-type hook action spawns a
    //        subagent through the SAME pool spawner the tool-context
    //        `subagent_spawner` uses (4.6). They remain distinct injection points
    //        on `HookExecutorImpl` but share one spawner. The orchestrator's
    //        `hooks` param is the concrete `Arc<hooks::HookExecutorImpl>`, so no
    //        trait-object coercion is needed.
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
    // B5 fold-back (claude-code `getAsyncHookResponseAttachments` +
    // `normalizeAttachmentForAPI` case `async_hook_response`, `messages.ts:4026`):
    // drain the completion channel and stash each completed background hook's
    // `systemMessage` AND `additionalContext` into the buffer, for the
    // orchestrator to re-inject as an `async_hook_response` reminder on the NEXT
    // turn. UNLIKE the synchronous PreToolUse/PostToolUse path (where ONLY
    // `additionalContext` is model-facing and `systemMessage` is suppressed,
    // `messages.ts:4258`), the `async_hook_response` attachment surfaces BOTH as
    // separate meta user messages that reach the model (`messages.ts:4030-4055`).
    // So we push both fields here, each on its own line. Hooks that returned
    // neither contribute nothing. Draining still keeps the bounded channel from
    // back-pressuring a fire-and-forget hook; when no hooks are configured
    // nothing is ever published, so this stays a no-op for the common case.
    let async_hook_response_buffer = AsyncHookResponseBuffer::default();
    let async_hook_drain_buffer = async_hook_response_buffer.clone();
    tokio::spawn(async move {
        while let Some((_id, result)) = async_hook_completion_rx.recv().await {
            if let Some(resp) = result.response.as_ref() {
                if let Some(text) = resp.system_message.clone() {
                    async_hook_drain_buffer.push(text);
                }
                if let Some(text) = resp.additional_context.clone() {
                    async_hook_drain_buffer.push(text);
                }
            }
        }
    });
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
        .with_async_registry(async_hook_registry)
        // Wire the Agent hook arm: an `agent`-type hook action spawns a subagent
        // through the SAME pool spawner the `AgentTool` uses (4.6 below). The arm
        // + `with_agent_spawner` builder already exist in the hooks crate; only
        // this production wiring was missing, so an `agent` hook now runs instead
        // of degrading to a no-op. Opt-in: byte-identical when no `agent` hook is
        // configured. (`subagent_spawner` is an `Arc`; cloned here, still moved
        // into the tool context below.)
        .with_agent_spawner(subagent_spawner.clone()),
    );

    // G4: fill the subagent spawner's hook-executor cell now that `hooks` exists,
    // so a child runner can fire `SubagentStart` (collecting + injecting the
    // hooks' `additionalContexts`) and register/clear the agent's frontmatter
    // hooks (Stop→SubagentStop) scoped to the child id. First fill wins.
    let _ = subagent_hook_executor_cell.set(hooks.clone());

    // (5.26) Build the MCP registry NOW (deferred from (5.1)) so it can carry
    //         the elicitation hook dispatcher, then auto-connect. The
    //         `OrchestratorHookDispatcher` shares the SAME `hooks` executor, so
    //         an inbound `elicitation/create` fires the `Elicitation` hook
    //         (claude-code `runElicitationHooks`): a hook may PROVIDE the answer
    //         or DENY it; with no hook it falls through to `{"action":"cancel"}`.
    //         `with_hook_dispatcher(Some(..))` is the only behavioral delta from
    //         the previous `with_raw_conn` wiring.
    let elicitation_dispatcher: Arc<dyn mcp::HookDispatcher> = Arc::new(
        orchestrator::OrchestratorHookDispatcher::new(
            hooks.clone(),
            cwd.clone(),
            main_transcript_path.clone(),
        ),
    );
    // OAuth 2.1 + PKCE seam for OAuth-configured remote (SSE/HTTP) MCP servers.
    // Reuses the platform `http` / `clock` / `storage` already built in step (1);
    // `on_authorization_url` surfaces the consent URL to the user (logs it
    // prominently + best-effort detached OS browser open). When a server has no
    // `oauth` config this is entirely inert — static-token / no-oauth servers
    // take the unchanged path.
    let mcp_on_auth_url = mcp_on_authorization_url();
    // XAA IdP-login config layer. When an `xaaIdp` settings tier is present
    // (`{issuer, clientId, callbackPort}` — mirror of claude-code
    // `getXaaIdpSettings`), wire a concrete `XaaConfigProvider` so an
    // `oauth.xaa==Some(true)` server resolves its token via the Cross-App-Access
    // token-exchange chain. The provider supplies the IdP `id_token` (cached or a
    // one-time OIDC browser pop), the AS `client_secret`
    // (`mcpOAuthClientConfig[serverKey]`), and the IdP token endpoint
    // (`discoverOidc`). Reuses the SAME http/clock/storage/on_authorization_url
    // Arcs as OAuthDeps. Absent the settings, `xaa_config` stays `None` and an
    // XAA-flagged server keeps its actionable hard-fail (XAA stays opt-in).
    let xaa_config: Option<Arc<dyn mcp::registry::XaaConfigProvider>> = {
        let mut tiers: Vec<String> = Vec::new();
        for p in [
            cfg.claude_home.join("settings.json"),
            cwd.join(".claude").join("settings.json"),
            cwd.join(".claude").join("settings.local.json"),
        ] {
            if let Ok(raw) = tokio::fs::read_to_string(&p).await {
                tiers.push(raw);
            }
        }
        let refs: Vec<&str> = tiers.iter().map(String::as_str).collect();
        mcp::XaaIdpSettings::from_settings_tiers(&refs).map(|settings| {
            // Build the server→(AS client_id, server_key) lookup from the known
            // MCP configs so the provider can resolve the AS `client_secret`.
            let lookup = mcp::MapServerOAuthLookup::from_specs(
                mcp_configs.iter().map(|c| (c.name.as_str(), &c.spec)),
            );
            Arc::new(mcp::XaaIdpConfigProvider::new(
                http.clone() as Arc<dyn traits::HttpTransport>,
                clock.clone() as Arc<dyn traits::Clock>,
                mcp_oauth_storage.clone(),
                mcp_on_auth_url.clone(),
                settings,
                Arc::new(lookup) as Arc<dyn mcp::ServerOAuthLookup>,
            )) as Arc<dyn mcp::registry::XaaConfigProvider>
        })
    };
    let mcp_oauth_deps = mcp::registry::OAuthDeps {
        http: http.clone() as Arc<dyn traits::HttpTransport>,
        clock: clock.clone() as Arc<dyn traits::Clock>,
        storage: mcp_oauth_storage,
        on_authorization_url: mcp_on_auth_url,
        xaa_config,
    };
    let mcp_registry = Arc::new(
        mcp::McpRegistry::with_raw_conn(
            posix.clone() as Arc<dyn McpTransport>,
            posix as Arc<dyn mcp::RawConnectionProvider>,
        )
        .with_hook_dispatcher(Some(elicitation_dispatcher))
        .with_oauth(mcp_oauth_deps),
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
    //        materialize stdout/stderr under a SESSION-SCOPED project temp dir
    //        `<projectTempDir>/<sessionId>/tasks` (claude-code `getTaskOutputDir`,
    //        `diskOutput.ts:50-55`) instead of an in-repo `<cwd>/.claude/...`
    //        path: the session id keeps concurrent sessions in one project from
    //        clobbering each other's spools, and the temp root keeps task output
    //        out of the working tree / git status (T16). The spawner is the
    //        tokio-backed `PosixRuntime`. The same handle is returned for a
    //        transport/TUI poller to read live state.
    let task_session_id = protocol::SessionId::new().to_string();
    let task_output_dir = session_task_output_dir(&cwd, &task_session_id);
    // Eagerly create the dir (claude-code `ensureOutputDir`'s `mkdir(recursive)`)
    // so the very first spool `allocate` (exclusive create) finds its parent.
    if let Err(e) = std::fs::create_dir_all(&task_output_dir) {
        tracing::warn!(
            target: "engine_desktop::tasks",
            dir = %task_output_dir.display(),
            error = %e,
            "could not create the session task-output dir; task spools may fail to allocate"
        );
    }
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
    .with_task_completed_firer(Arc::new(orchestrator::OrchestratorTaskCompletedFirer::new(
        hooks.clone(),
        cwd.clone(),
        main_transcript_path.clone(),
    )))
    // Fire the `TaskCreated` hook (claude-code `executeTaskCreatedHooks`) when a
    // task is created. Counterpart to the `TaskCompleted` firer above — wraps
    // the SAME `Arc<HookExecutorImpl>` so the `tasks` leaf reaches `orch.hooks`
    // without a dependency cycle.
    .with_task_created_firer(Arc::new(orchestrator::OrchestratorTaskCreatedFirer::new(
        hooks.clone(),
        cwd.clone(),
        main_transcript_path.clone(),
    )));
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

    // (5.46-prompt) D1 ITEM 4: coordinator-mode system prompt + user context.
    //        Mirrors TS `buildEffectiveSystemPrompt` (systemPrompt.ts:59-75):
    //        when coordinator mode is active AND nothing has already overridden
    //        the system prompt (the Rust analog of "no main-thread agent
    //        definition" + "no explicit overrideSystemPrompt"), swap in the
    //        coordinator system prompt. `system_prompt_override` being `None` is
    //        precisely that condition here — the desktop host does not set it for
    //        a normal session, and a CLI `--system-prompt` / agent override would
    //        have populated it (TS: `overrideSystemPrompt` wins first). The
    //        per-turn coordinator USER context (TS `getCoordinatorUserContext`,
    //        injected at QueryEngine.ts:304) has no per-turn user-context seam in
    //        this orchestrator yet, so it is appended to the coordinator system
    //        prompt as a trailing `<system-reminder>` block (the worker-tools
    //        allow-list + connected-MCP names; scratchpad is omitted — no
    //        scratchpad gate/path is wired on desktop). A true per-turn
    //        recomputation is DEFERRED until a per-turn user-context seam exists.
    if coordinator_mode.is_enabled() && orch_cfg.system_prompt_override.is_none() {
        let simple = coordinator::is_env_truthy(std::env::var("CLAUDE_CODE_SIMPLE").ok().as_deref());
        let mut prompt = coordinator::coordinator_system_prompt(simple);
        // Connected MCP server names for the worker-tools user context.
        let mcp_names: Vec<String> = mcp_registry
            .snapshot()
            .await
            .into_iter()
            .map(|s| s.name)
            .collect();
        if let Some(user_ctx) =
            coordinator::coordinator_user_context(&mcp_names, None, simple)
        {
            // Wrap as a system-reminder, mirroring how claude-code injects
            // per-turn meta context (`wrapInSystemReminder`).
            prompt.push_str("\n\n<system-reminder>\n");
            prompt.push_str(&user_ctx);
            prompt.push_str("\n</system-reminder>");
        }
        orch_cfg.system_prompt_override = Some(prompt);
    }

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
    .with_tool_invoker(teammate_invoker.clone() as Arc<dyn traits::tool_invoker::ToolInvoker>)
    // Anchor the teammate's `AgentModel::Inherit` / family aliases to the parent
    // model — the same seam the `PoolSubagentSpawner` gets above. #15: resolve
    // the alias to the concrete main-loop wire id (claude `getMainLoopModel()`)
    // so an `Inherit` teammate in default mode runs against a real id, not the
    // raw `orch_cfg.model` alias (which would fail at the provider).
    .with_default_model(agent::model_resolution::resolve_user_specified_model(
        &orch_cfg.model,
    ))
    // #15: thread the live permission mode + the RAW user model setting (the
    // un-resolved alias, e.g. "opusplan") so the teammate's `Inherit` resolution
    // gets the same `getRuntimeMainLoopModel` plan-mode swap as the spawner above
    // (opusplan + plan → Opus). Default mode → byte-identical to before.
    .with_permission_mode(cfg.permission_mode)
    .with_model_setting(cfg.default_model.clone())
    .with_status_sink(coordinator_sink as Arc<dyn tasks::handlers::TaskStatusSink>)
    // Fire the `TeammateIdle` hook (claude-code `executeTeammateIdleHooks`,
    // `stopHooks.ts:403`) each time a teammate finishes a turn-set and parks
    // awaiting the next message ("about to go idle"). The firer wraps the SAME
    // `Arc<HookExecutorImpl>` the orchestrator fires its other hooks through, so
    // the `tasks` leaf reaches `orch.hooks` without a dependency cycle —
    // mirroring the `TaskCompleted` / `TaskCreated` firers above.
    .with_teammate_idle_firer(Arc::new(orchestrator::OrchestratorTeammateIdleFirer::new(
        hooks.clone(),
        cwd.clone(),
        main_transcript_path.clone(),
    )));
    task_registry_inner.register_handler(
        tasks::TaskType::InProcessTeammate,
        Arc::new(teammate_handler),
    );

    // (5.46c) Cron: register the `Dream` handler so cron-spawned `TaskType::Dream`
    //        tasks actually run. The `CronScheduler` (constructed + started below,
    //        after the registry is shared) creates `Dream` tasks; without a handler
    //        each fire would be an inert task-state row. Same deferred-invoker
    //        pattern as the teammate handler above: the real `RegistryToolInvoker`
    //        needs `tools` (built after this point), so a `DeferredToolInvoker` is
    //        injected now and bound to the real invoker at (5.5a) below.
    let dream_invoker = Arc::new(DeferredToolInvoker::new());
    tasks::registry::register_dream_handler(
        &mut task_registry_inner,
        subagent_spawner.clone(),
        dream_invoker.clone() as Arc<dyn traits::tool_invoker::ToolInvoker>,
        budget_enforcer.clone(),
    );

    // (5.46d) T15: register the `LocalAgent` handler so `TaskType::LocalAgent`
    //        tasks dispatch to a real one-shot subagent worker instead of failing
    //        with `UnknownType`. This closes the gap where `register_agent_handlers`
    //        was authored but never called from any composition root, leaving
    //        `LocalAgent`/`LocalWorkflow` with state variants but no handler.
    //
    //        We register the LocalAgent handler DIRECTLY rather than calling
    //        `register_agent_handlers` (which ALSO registers `InProcessTeammate`)
    //        because the teammate handler was already registered above (5.46a)
    //        with the coordinator `CoordinatorStatusSink` attached — calling the
    //        combined helper here would clobber that sink-bearing handler with a
    //        sink-less one.
    //
    //        Same deferred-invoker pattern as the teammate + dream handlers: the
    //        real `RegistryToolInvoker` needs `tools` (assembled after this
    //        point), so a `DeferredToolInvoker` is injected now and bound at
    //        (5.5a) below once `tools` exists.
    //
    //        DEFERRED (out of scope here): routing the BACKGROUNDED `AgentTool`
    //        spawn (claude-code `registerAsyncAgent`) through
    //        `TaskRegistry::spawn(TaskType::LocalAgent)` so background agents
    //        surface in TaskList/Get/Output. `AgentTool::call` always dispatches
    //        synchronously through the spawner today and exposes no clean
    //        backgrounded seam to re-route; that wiring lands with the async-agent
    //        work. Registering the handler here is the prerequisite for it.
    let local_agent_invoker = Arc::new(DeferredToolInvoker::new());
    // Bridge the LocalAgent worker's status (and rest signals) THROUGH to the
    // registry so list/get reflect reality and `take_pending_task_notifications`
    // actually fires (terminal completion + each "comes to rest"). Deferred: the
    // handler is registered before the registry `Arc` exists, so this is bound
    // at (5.46f) below once `task_registry` is built.
    let local_agent_status_sink = Arc::new(tasks::registry_status_sink::RegistryStatusSink::new());
    task_registry_inner.register_handler(
        tasks::TaskType::LocalAgent,
        Arc::new(
            tasks::handlers::LocalAgentHandler::new(
                subagent_spawner.clone(),
                local_agent_invoker.clone() as Arc<dyn traits::tool_invoker::ToolInvoker>,
                budget_enforcer.clone(),
                task_registry_inner.output_manager.clone(),
            )
            // Wire the persistent/resume seam: a BACKGROUNDED LocalAgent now
            // parks ("comes to rest") after each turn-set and accepts
            // `send_message` to resume — claude-code's unified agent lifecycle
            // (`resumeAgentBackground` / `injectUserMessageToTeammate`).
            .with_streaming_spawner(subagent_streaming_spawner.clone())
            .with_status_sink(
                local_agent_status_sink.clone() as Arc<dyn tasks::handlers::TaskStatusSink>
            ),
        ),
    );

    // (5.46e) Register the `LocalWorkflow` handler so the `Workflow` tool's
    //        `TaskRegistry::spawn(TaskType::LocalWorkflow)` dispatches to a real
    //        workflow worker (the embedded QuickJS runtime + agent()→subagent
    //        bridge) instead of failing with `UnknownType`. Same deferred-invoker
    //        pattern as the LocalAgent handler above (bound at (5.5a) once `tools`
    //        exists): a workflow's `agent()` calls inherit this invoker so their
    //        child runners dispatch tools through the parent registry.
    let local_workflow_invoker = Arc::new(DeferredToolInvoker::new());
    // Shared `budget.spent()` pool: published once the orchestrator exists
    // (built below) — the same `Arc<AtomicU64>` the main loop feeds per response,
    // so a workflow's `spent()` reads main loop + all workflows. Same deferred
    // pattern as `local_workflow_invoker` (handler registered before the orch).
    let local_workflow_output_pool: Arc<std::sync::OnceLock<Arc<std::sync::atomic::AtomicU64>>> =
        Arc::new(std::sync::OnceLock::new());
    // Turn-start output baseline (claude-code `xtr`) backing the workflow's
    // turn-relative `budget.spent()`; published from the orchestrator below.
    let local_workflow_turn_baseline: Arc<std::sync::OnceLock<Arc<std::sync::atomic::AtomicU64>>> =
        Arc::new(std::sync::OnceLock::new());
    task_registry_inner.register_handler(
        tasks::TaskType::LocalWorkflow,
        Arc::new(
            tasks::handlers::LocalWorkflowHandler::new(
                subagent_spawner.clone(),
                local_workflow_invoker.clone() as Arc<dyn traits::tool_invoker::ToolInvoker>,
                budget_enforcer.clone(),
                task_registry_inner.output_manager.clone(),
            )
            // The script's `budget.total` = the turn's token target
            // (`OrchestratorConfig.token_budget`); `spent()` reads the shared
            // pool (main loop + all workflows) once `output_pool_cell` is bound.
            .with_token_budget(orch_cfg.token_budget)
            .with_output_pool_cell(local_workflow_output_pool.clone())
            .with_turn_baseline_cell(local_workflow_turn_baseline.clone()),
        ),
    );

    let task_registry = Arc::new(task_registry_inner);

    // (5.46f) Bind the deferred LocalAgent status sink now that the registry
    //         `Arc` exists: the persistent agent's `set_status` / `notify_rest`
    //         now reach `task_registry`, so terminal + per-rest notifications
    //         surface through `take_pending_task_notifications`.
    local_agent_status_sink
        .bind(task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>);

    // (5.48) Cron: construct, load the single persisted tasks file, and start the
    //        live cron scheduler so jobs created by CronCreate actually fire —
    //        closing parity gap §0.3 / §B (the scheduler was never constructed, so
    //        persisted jobs never ran). 1:1 with claude-code `cronTasks.ts`: all
    //        durable jobs live in ONE project-relative file
    //        `<cwd>/.claude/scheduled_tasks.json` (the same project root the
    //        CronCreate/List/Delete tools key off via `BuiltinToolContext.workspace`,
    //        which is `cwd`). `load_persisted` reads `createdAt`/`lastFiredAt` in
    //        epoch ms; next-fire is COMPUTED at runtime from the cron string +
    //        `lastFiredAt ?? createdAt` (never persisted). Ticks every 60s on a
    //        posix RuntimeSpawner (D17). The detached tick task holds a self-clone
    //        of the scheduler, so it runs for the process lifetime without being
    //        stored on `DesktopRuntime`.
    //        Gated by the `CLAUDE_CODE_DISABLE_CRON` local kill-switch
    //        (claude-code `prompt.ts:34/38` — the env override that wins over the
    //        GrowthBook fleet flag, which itself defaults on).
    if cron_scheduler_enabled(std::env::var("CLAUDE_CODE_DISABLE_CRON").ok().as_deref()) {
        let tasks_file = cron::tasks_file::scheduled_tasks_path(&cwd);
        let scheduler = Arc::new(cron::CronScheduler::new(
            task_registry.clone(),
            Arc::new(PosixFileSystem::new(cwd.clone())),
            clock.clone(),
            Arc::new(PosixRuntime::new()),
            tasks_file,
        ));
        // Load every durable job from the single tasks file (a recurring job
        // created days ago is aged correctly on load; its restored `lastFiredAt`
        // prevents a missed-run catch-up from re-firing an already-fired run).
        scheduler.load_persisted().await;
        if let Err(e) = scheduler.clone().start().await {
            tracing::error!("cron: failed to start scheduler: {e}");
        }
    }

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
    // Wire the shared `MailboxRouter` for EVERY session (was coordinator-only):
    // a backgrounded local_agent (`run_in_background`) registers its mailbox on
    // this router, and the `SendMessage` tool must be able to route to it even
    // outside a coordinator team. Faithful to claude-code, where `SendMessage`
    // always resolves a running async agent. Non-async default sessions are
    // unaffected — with no teammates/agents registered a send resolves to
    // `NotFound`, the same effective outcome as the prior `None`.
    let coordinator_mailbox: Option<Arc<dyn traits::mailbox::MailboxRouterHandle>> =
        Some(coordinator.mailbox_router.clone() as Arc<dyn traits::mailbox::MailboxRouterHandle>);
    // (SANDBOX.1) Make the bash sandbox path LIVE (parity §0.2 / §B). Previously
    // `sandbox_available` was hardcoded `false`, so bash NEVER sandboxed — even
    // when the user enabled it in settings — leaving the macOS SBPL / Linux bwrap /
    // sandbox-runtime stack as dead code. Resolve the runtime config from the
    // `sandbox` settings subsection (claude-code: opt-in via `sandbox.enabled`,
    // default OFF) and probe host deps (sandbox-exec on macOS / bwrap on Linux).
    // `should_use_sandbox` keys on `sandbox_available`, so it is
    // `settings-enabled AND deps-present`. Unset settings ⇒ off ⇒ byte-identical
    // to today; opt-in now actually sandboxes on a capable host.
    let sandbox_platform = if cfg!(target_os = "macos") {
        SandboxPlatform::Mac
    } else {
        SandboxPlatform::Linux
    };
    let sandbox_runtime_cfg = {
        let mut tiers: Vec<String> = Vec::new();
        for p in [
            cfg.claude_home.join("settings.json"),
            cwd.join(".claude").join("settings.json"),
            cwd.join(".claude").join("settings.local.json"),
        ] {
            if let Ok(raw) = tokio::fs::read_to_string(&p).await {
                tiers.push(raw);
            }
        }
        // Managed (policySettings) tier — HIGHEST priority (SETTING_SOURCES:
        // …→localSettings→flagSettings→policySettings). Appended LAST so the
        // ascending-priority fold lets a managed `sandbox.*` win over user/
        // project/local (faithful to getInitialSettings()/loadSettingsFromDisk).
        // flagSettings is omitted: the engine has no boot-time `--settings`
        // analog (see spec §4e); if one is added, push its raw text BEFORE the
        // managed tier to honor `localSettings→flagSettings→policySettings`.
        tiers.extend(crate::settings_watch::managed_settings_raw_tiers().await);
        let refs: Vec<&str> = tiers.iter().map(String::as_str).collect();
        // Seed the `SandboxConvertContext` with the boot-resolvable hardening
        // paths so the settings/skills denyWrite defense actually fires
        // (sandbox-adapter.ts:225-299). Seeds with no boot analog
        // (cwd_settings_paths / worktree_main_repo_path / additional_md_dirs)
        // stay empty — see spec §5.
        let managed = crate::settings_watch::managed_settings_dir();
        let to_s = |p: std::path::PathBuf| p.to_string_lossy().into_owned();
        let ctx = sandbox::policy_convert::SandboxConvertContext {
            claude_temp_dir: Some(claude_temp_dir()),
            settings_file_paths: vec![
                to_s(cfg.claude_home.join("settings.json")),
                to_s(cwd.join(".claude").join("settings.json")),
                to_s(cwd.join(".claude").join("settings.local.json")),
                to_s(managed.join("managed-settings.json")),
            ],
            managed_drop_in_dir: Some(to_s(managed.join("managed-settings.d"))),
            skills_dirs: vec![to_s(cwd.join(".claude").join("skills"))],
            ..Default::default()
        };
        sandbox_runtime_config_from_settings_tiers(&refs, &cwd, &ctx)
    };
    // Faithful to claude-code `isSandboxingEnabled()` (sandbox-adapter.ts:532):
    // supported-platform AND deps present AND in the `enabledPlatforms` list AND
    // the user opted in via `sandbox.enabled`. We fold the `enabledPlatforms`
    // gate (`isPlatformInEnabledList`) into BOTH `check_dependencies` (so a
    // missing dep is reported alongside an out-of-list platform) and
    // `sandbox_available`, replacing the old hardcoded `true`.
    // WSL-aware host detection: `platform_posix::sandbox::host_platform()` mirrors
    // claude-code's `getPlatform()` (returns `None`/refused on WSL1), so on WSL1
    // we do NOT report `Linux` and wrongly compute `sandbox_available == true`.
    // A coarse `cfg!(target_os = "linux") ⇒ Linux` would miss the WSL1 refusal.
    let current_platform = platform_posix::sandbox::host_platform();
    // `isPlatformInEnabledList` only makes sense for a supported platform; on an
    // unsupported host (WSL1 / non-POSIX) the platform can never be in the list.
    let in_enabled_list = current_platform.is_some_and(|p| {
        platform_in_enabled_list(sandbox_runtime_cfg.enabled_platforms.as_deref(), p)
    });
    // `check_dependencies(None, …)` yields the "platform not supported" error,
    // so `sandbox_available` correctly drops to false on WSL1 / non-POSIX.
    let sandbox_deps =
        sandbox::dependency_check::check_dependencies(current_platform, in_enabled_list);
    let sandbox_available =
        sandbox_runtime_cfg.enabled && in_enabled_list && sandbox_deps.errors.is_empty();

    // Startup reject/degrade, faithful to claude-code `isSandboxRequired()`
    // (sandbox-adapter.ts:479) + `getSandboxUnavailableReason()` (:562). When the
    // user explicitly enabled the sandbox but it cannot run here:
    //   - `failIfUnavailable: true`  ⇒ this is a HARD failure (their security
    //     posture is being silently ignored otherwise — issue #34044), so we
    //     refuse the build with `BuildError::SandboxUnavailable`;
    //   - otherwise ⇒ degrade to no-sandbox execution but WARN, so the operator
    //     knows commands run unsandboxed.
    let sandbox_required = sandbox_runtime_cfg.enabled && sandbox_runtime_cfg.fail_if_unavailable;
    if let Some(reason) =
        PosixSandbox::unavailable_reason_for(sandbox_runtime_cfg.enabled, in_enabled_list)
    {
        if sandbox_required {
            return Err(BuildError::SandboxUnavailable(reason));
        }
        tracing::warn!(%reason, "Sandbox disabled: commands will run WITHOUT sandboxing");
    }

    // Shared LSP registry: the SAME `Arc<LspRegistry>` is handed to the LSP
    // tool (via `tool_ctx.lsp_registry`) AND to the plugin manager below, so
    // plugin-supplied LSP servers (`.lsp.json`) are registered into the very
    // registry the `LSPTool` reads at runtime (the registry's
    // `register_plugin_servers` is the ONLY supported registration path).
    // Shared LSP diagnostics sink: the registry's `ensure_server_for_file`
    // spawns a passive subscriber per started server that drains
    // `publishDiagnostics` into it, and the orchestrator polls it each turn to
    // surface the `<new-diagnostics>` reminder to the model.
    let lsp_diagnostics = lsp::diagnostic_registry::LspDiagnosticRegistry::new();
    let plugin_lsp_registry = Arc::new(
        lsp::LspRegistry::new(Arc::new(platform_posix::PosixLspTransport::new()))
            .with_diagnostics(lsp_diagnostics.clone()),
    );

    // (5.5b) Decorate the subagent spawner so AgentTool's `run_in_background`
    // path is LIVE: `spawn_async` spawns a PERSISTENT LocalAgent through the
    // registry, registers its mailbox on the shared router, and starts the
    // mailbox→runner pump (the `registerAsyncAgent` lifecycle). Built HERE —
    // after `task_registry` exists — so NO deferred cell is needed; the
    // one-shot / teammate / workflow handlers keep the raw spawner captured
    // earlier (they only use the sync `spawn`, which the decorator delegates).
    let subagent_spawner: Arc<dyn traits::subagent_spawn::SubagentSpawner> =
        Arc::new(background_agent::BackgroundAgentSpawner {
            inner: subagent_spawner,
            registry: task_registry.clone(),
            mailbox_router: coordinator.mailbox_router.clone(),
            runtime: Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>,
        });

    let tool_ctx = BuiltinToolContext {
        // FILE.B: file tools share one read-state map for the (future) staleness
        // guard / Read-dedup; the composition-root Arc-share with the orchestrator
        // is wired when a consumer (FILE.A/D/E/F) reads it.
        read_file_state: tool_api::read_file_state::new_read_file_state_map(),
        // Read(deny) → Grep/Glob search excludes (resolved from the boot policy
        // above; empty when enforcement is off or no Read-deny rule applies).
        read_deny_exclude_globs,
        fs: Arc::new(PosixFileSystem::new(cwd.clone())),
        bus: Arc::new(telemetry::AnalyticsBus::new()),
        trusted_dirs: vec![cwd.clone()],
        process: Arc::new(PosixProcess::new()),
        sandbox: Arc::new(PosixSandbox::new()),
        clock: clock.clone(),
        sandbox_runtime: sandbox_runtime_cfg,
        // Inject the LIVE runner: the desktop session routes its sandboxed
        // bash/powershell/skill commands through `sandbox-runtime`'s
        // `SandboxManager` (forward proxies + Linux socat bridge + MITM/seccomp),
        // rather than the legacy sync `wrap_with_sandbox`. The manager is brought
        // up lazily on the first `wrap` and reused for the session.
        //
        // Teardown is Drop-based: engine-desktop has NO per-session teardown hook
        // (see the `fire_session_start` note below — `build` returns the runtime
        // and the host drops it on process exit; there is no hook-capable shutdown
        // seam, so `reset().await` cannot be called from here). The `Arc<dyn
        // SandboxRunner>` lives inside `tool_ctx` → the tool registry → the
        // runtime; when the last `Arc` ref drops, `SandboxRuntimeRunner` drops,
        // dropping its `SandboxManager` and the owned `RunningState`. That abort
        // the proxy accept-loop tasks (`JoinHandle` aborts on drop) and drops the
        // `LinuxBridge`, whose `Drop` SIGTERMs the `socat` bridge children
        // (`sandbox-runtime/src/linux.rs:404`). The only thing the explicit
        // `SandboxManager::reset()` does that Drop does not is remove the leftover
        // Unix socket files / dispose the ephemeral MITM-CA temp dir — cosmetic
        // temp-file cleanup, not a leaked process. When a host teardown seam is
        // added (the future-batch note on `fire_session_end`), call
        // `sandbox_runner.reset().await` there for the tidy socket/CA cleanup.
        sandbox_runner: std::sync::Arc::new(sandbox_runtime_runner::SandboxRuntimeRunner::new()),
        permission_mode: cfg.permission_mode,
        sandbox_available,
        workspace: cwd.clone(),
        platform: sandbox_platform,
        http: http.clone(),
        provider: tool_provider,
        default_model: orch_cfg.model.clone(),
        worktree: Arc::new(PosixWorktreeManager::new(cwd.clone())),
        subagent_spawner: Some(subagent_spawner),
        task_registry: Some(
            task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>
        ),
        mailbox_router: coordinator_mailbox,
        budget_enforcer: Some(budget_enforcer),
        // (3b) AgentTool threads this into the subagent's RegistryToolInvoker so
        // spawned subagents are gated by the same boot gate as the main loop.
        permission_gate: Some(perms.clone()),
        // G14: the AgentTool registers async-agent `name → agentId` in the
        // spawner's OWN internal registry (PoolSubagentSpawner::register_name),
        // so no separate ctx-level registry is wired here. A shared
        // `Some(Arc<dyn AgentNameRegistry>)` can be threaded once a SendMessage
        // resolver needs to read the same map outside the spawner.
        agent_name_registry: None,
        mcp_registry: Some(mcp_registry.clone()),
        lsp_registry: Some(plugin_lsp_registry.clone()),
        camera: None,
        voice: None,
        stt: None,
        tts: None,
        share: None,
        notifications: None,
        clipboard: None,
        computer_control: None,
        android_shell: None,
        android_git: None,
        android_git_secret: None,
        // BLOCKING TaskCreated/TaskCompleted hooks for the V2 Task* tool path
        // (claude-code `executeTaskCreatedHooks` / `executeTaskCompletedHooks`).
        // Wraps the SAME `Arc<HookExecutorImpl>` + cwd the registry firers use
        // (the fire-and-forget `OrchestratorTaskCreated/CompletedFirer` injected
        // into the `TaskRegistry` above), but REPORTS a Block decision so the
        // tool can roll back creation / refuse a completion. Separate seam — the
        // registry firers' observe-only contract is unchanged.
        task_lifecycle_hooks: Some(Arc::new(
            orchestrator::OrchestratorTaskLifecycleHookFirer::new(
                hooks.clone(),
                cwd.clone(),
                main_transcript_path.clone(),
            ),
        )),
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
            // The tool-context analytics bus, so TeamCreate/TeamDelete fire
            // tengu_team_created / tengu_team_deleted through the same bus the
            // rest of the builtin tools use.
            bus: Some(tool_ctx.bus.clone()),
            // (5.47a) The background-task spawner for each teammate's mailbox→
            //         runner PUMP. A fresh `PosixRuntime` (the canonical desktop
            //         `RuntimeSpawner`, as used for the task registry / hooks /
            //         cron throughout `build`); D17-compliant (no direct
            //         `tokio::spawn`). With this wired, a coordinator
            //         `SendMessage` to a teammate is drained from its mailbox
            //         into the teammate's turn loop (via the `TaskRegistry`
            //         `TeamSpawnSeam::send_message` override) — the
            //         `injectUserMessageToTeammate` path. The pump exits on its
            //         own when the teammate is killed (send → Terminated), so it
            //         needs no separate teardown hook.
            runtime: Some(Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>),
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
            skill_session_id.clone(),
        ));
    // G5: fill the subagent spawner's skill-loader cell with a
    // `traits::skill_loader::SkillLoader` over the SAME shared command registry,
    // so a child agent runner can preload its frontmatter `skills:` (claude
    // runAgent.ts:577-646). First fill wins; the registry is filled at (6) before
    // any spawn fires, so the loader never reads the empty registry.
    let _ = subagent_skill_loader_cell.set(Arc::new(
        agent_skill_loader::AgentSkillLoader::new(
            shared_command_registry.clone(),
            Some(skill_session_id),
        ),
    ) as Arc<dyn traits::skill_loader::SkillLoader>);
    // Fire the `CwdChanged` hook (claude-code `onCwdChangedForHooks`,
    // Shell.ts:409) when a `cd` inside a Bash call moves the persistent shell
    // cwd. The firer wraps the SAME `Arc<HookExecutorImpl>` the orchestrator
    // fires its other hooks through (mirrors the `TaskCreated` / `TaskCompleted`
    // firers), so the `tool-shell` leaf reaches `orch.hooks` without a dependency
    // cycle. Injected here (the desktop composition root) only — `engine-mobile`
    // never registers the shell tools, so the mobile path keeps the no-firer
    // BashTool.
    let cwd_changed_firer: hooks::OptionalCwdChangedFirer = Some(Arc::new(
        orchestrator::OrchestratorCwdChangedFirer::new(
            hooks.clone(),
            cwd.clone(),
            main_transcript_path.clone(),
        ),
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
    // Workflow tool (desktop-only — it fans out subagents). Registered here,
    // after `register_desktop_tools`, because its launcher needs `task_registry`
    // (constructed above): `Workflow.call` spawns a `LocalWorkflow` background
    // task through it and returns `{status:"async_launched", taskId, taskType}`.
    {
        let workflow_launcher: Arc<dyn tool_workflow::WorkflowLauncher> =
            Arc::new(TaskRegistryWorkflowLauncher {
                registry: task_registry.clone(),
                cwd: cwd.clone(),
            });
        tools_inner.register_builtin(Arc::new(tool_workflow::WorkflowTool::new(Some(
            workflow_launcher,
        ))));
    }
    for (conn_id, mcp_tools) in
        tool_mcp::build_registered_mcp_tools(&mcp_registry, mcp_tool_ctx).await
    {
        tools_inner.register_mcp_tools(conn_id, mcp_tools);
    }
    // Structured output (`--json-schema`): register the forced `StructuredOutput`
    // tool whose `input_schema` IS the user schema; its `call` captures the model's
    // result into `structured_output_slot` for the print path to validate + retry.
    // `None` (no `--json-schema`) leaves the registry + the slot untouched.
    let structured_output_slot: Option<orchestrator::structured_output::StructuredOutputSlot> =
        cfg.json_schema.as_ref().map(|schema| {
            let slot: orchestrator::structured_output::StructuredOutputSlot =
                Arc::new(std::sync::Mutex::new(None));
            tools_inner.register_builtin(Arc::new(
                orchestrator::structured_output::StructuredOutputTool::new(
                    schema.clone(),
                    slot.clone(),
                ),
            ));
            slot
        });
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

    // (5.5a-cron) Bind the cron `Dream` handler's `DeferredToolInvoker` to the
    //        real `RegistryToolInvoker` now that `tools` exists — same recursion-
    //        lock invariant and boot gate as the teammate invoker above.
    dream_invoker.set(Arc::new(
        tool_api::tool_invoker_impl::RegistryToolInvoker::new(tools.clone()).with_gate(perms.clone()),
    ));

    // (5.5a-local-agent) T15: bind the `LocalAgent` handler's `DeferredToolInvoker`
    //        to the real `RegistryToolInvoker` now that `tools` exists — same
    //        recursion-lock invariant and boot gate as the teammate + dream
    //        invokers above. A `LocalAgent` task's child runner dispatches its
    //        tools through the parent registry.
    local_agent_invoker.set(Arc::new(
        tool_api::tool_invoker_impl::RegistryToolInvoker::new(tools.clone()).with_gate(perms.clone()),
    ));

    // (5.5a-local-workflow) Bind the `LocalWorkflow` handler's `DeferredToolInvoker`
    //        to the real `RegistryToolInvoker` now that `tools` exists — so a
    //        workflow's `agent()` subagents dispatch their tools through the
    //        parent registry under the same recursion-lock + boot gate.
    local_workflow_invoker.set(Arc::new(
        tool_api::tool_invoker_impl::RegistryToolInvoker::new(tools.clone()).with_gate(perms.clone()),
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
        let has_config_change = all.iter().any(|h| {
            h.events
                .contains(&hooks::events::HookEventType::ConfigChange)
        });
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
                main_transcript_path.clone(),
            )))
        };
    // (6.5-pre) Clone the registry Arcs the plugin bootstrap (below, after the
    //           command registry is filled at (6)) writes through, BEFORE they
    //           are moved into the orchestrator constructor. `Arc<RwLock<…>>`
    //           shares state, so plugin hooks/agents registered after the move
    //           are still observed by the orchestrator's clone.
    let plugin_hook_registry = hook_registry.clone();
    let plugin_agent_catalog = agent_catalog.clone();
    let plugin_mcp_registry = mcp_registry.clone();
    // `cwd` is moved into the orchestrator below; the plugin bootstrap's
    // sandboxed `PosixFileSystem` (a Plan-16 dead-code field on `PluginManager`)
    // needs a workspace root, so snapshot it here.
    let cwd_for_plugins = watch_cwd.clone();
    // #39 UserPromptExpansion: capture the SAME `Arc<HookExecutorImpl>` BEFORE
    // it is moved into the orchestrator, so the slash-command dispatcher (built
    // at (6), below) can fire `UserPromptExpansion` through it at command
    // expansion. The dispatcher pairs it with a context provider that reads the
    // orchestrator's live session id (`orch.expansion_hook_context()`).
    let expansion_hook_executor = hooks.clone();
    let orch_builder = ConversationOrchestrator::new(
        orch_cfg, api_client, tools, hooks, perms, output, memory, cwd,
    )
    // FIX A: hand the orchestrator the resolved claude-home so its hook payloads
    // carry a deterministically-computed `transcript_path`
    // (`<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`, claude-code
    // `getTranscriptPathForSession`) even though PRODUCTION wires NO `JsonlWriter`
    // (every `with_jsonl_writer` call site is a test). Without this every
    // PreToolUse / PostToolBatch / lifecycle hook fired with an empty path.
    .with_config_home(cfg.claude_home.clone())
    // FIX A/B/C: adopt the boot-canonical session id so the orchestrator's LIVE
    // session matches the id baked into the leaf firers' `transcript_path` and the
    // subagent spawner's subagents dir — one consistent session id end-to-end.
    .with_session_id(main_session_id)
    .with_cost_tracker(cost_tracker)
    .with_analytics_bus(analytics_bus)
    .with_mcp_registry(mcp_registry)
    .with_hook_registry(hook_registry)
    .with_agent_catalog(agent_catalog)
    .with_compaction(compactor)
    .with_cache_safe_slot(cache_safe_slot)
    // Surface LSP `<new-diagnostics>` to the model each turn (the same sink the
    // LSP registry drains publishDiagnostics into).
    .with_new_diagnostics_source(
        Arc::new(lsp_diagnostics.clone()) as Arc<dyn traits::NewDiagnosticsSource>
    )
    // SKILLLIST.1: enumerate model-invocable skills each turn so the model
    // can discover them. Reads `shared_command_registry` lazily at turn time
    // (populated below at (6), before any turn fires).
    .with_skill_listing(Arc::new(RegistrySkillListing(shared_command_registry.clone())))
    // B5: fold completed background (`async`) hook responses back into the
    // next turn. Backed by the completion-channel drain buffer above.
    .with_async_hook_responses(Arc::new(async_hook_response_buffer))
    // T35: fold terminal background tasks (a backgrounded `local_bash` /
    // `local_agent` / MCP `monitor` …) back into the next turn as a
    // `<task-notification>` reminder so the model learns its async task
    // finished. Backed by the SAME `TaskRegistry` Arc wired into the tool
    // context above; the provider drains the registry's terminal-not-notified
    // tasks each turn (mark-notified + evict ⇒ each completion surfaces once).
    .with_task_notifications(Arc::new(orchestrator::RegistryTaskNotifications::new(
        task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>,
    )))
    // Finding #73: supply the V2 task list to the per-turn `task_reminder`
    // (the default variant when tasks are enabled). Reads the file-backed
    // `TodoStore` for the active list each turn, resolving the list id via the
    // same env/team precedence the `Task*` tools use. V1 (`todo_reminder`)
    // needs no provider; it reads `session.todos` directly.
    .with_todo_reminder_tasks(Arc::new(orchestrator::TodoStoreReminderTasks::new()));

    // P0.1 ACTIVATION (gated, default OFF). When `CLAUDE_CODE_MEMDIR_PREFETCH`
    // is truthy, wire the memdir-backed memory selector so relevant
    // `~/.claude/memdir` entries surface each turn (a Haiku-class side query per
    // turn over `side_query_client`). The composition-root presence of the
    // prefetch IS the gate — claude-code keeps this behind `tengu_moth_copse`
    // (default false), so unset/false leaves the surfacing channel inert and the
    // locked fixtures byte-identical (`memory_prefetch.is_some() == false`).
    let orch_builder = match (
        is_env_truthy("CLAUDE_CODE_MEMDIR_PREFETCH"),
        dirs::home_dir(),
    ) {
        (true, Some(home)) => orch_builder.with_memory_prefetch(
            orchestrator::prompt::build_memdir_prefetch(
                side_query_client.clone(),
                Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>,
                &home,
            ),
        ),
        _ => orch_builder,
    };

    // P1 session-memory standalone trigger (§6.5, gated, default OFF). When
    // `CLAUDE_CODE_SESSION_MEMORY` is truthy, wire the threshold-gated extractor
    // so durable notes are background-distilled (a Haiku-class fork) once the
    // tool-call threshold crosses and written to
    // `<configHome>/agents/session-memory/<id>.md`, which the Session-tier memdir
    // scan re-loads next session. Thresholds are unpinned upstream (spec §6.5) —
    // 30/30 tool calls is a tunable default. Unset/false ⇒ no handle ⇒ inert, so
    // the locked fixtures stay byte-identical.
    let orch_builder = match (
        is_env_truthy("CLAUDE_CODE_SESSION_MEMORY"),
        dirs::home_dir(),
    ) {
        (true, Some(home)) => orch_builder.with_session_memory(
            orchestrator::prompt::build_session_memory_handle(
                side_query_client.clone(),
                "claude-haiku-4-5".to_string(),
                30,
                30,
                &home,
                Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>,
            ),
        ),
        _ => orch_builder,
    };
    let orch = Arc::new(orch_builder);

    // Publish the orchestrator's shared output-token pool to the workflow
    // handler (registered above with a still-empty cell). From here, a launched
    // workflow's `budget.spent()` reads the same `Arc<AtomicU64>` the main loop
    // feeds per response — main loop + all workflows, claude-code's shared pool.
    let _ = local_workflow_output_pool.set(orch.output_token_pool());
    let _ = local_workflow_turn_baseline.set(orch.turn_start_output_baseline());

    // (6) Command registry through the desktop composition root.
    let handle: Arc<dyn OrchestratorHandle> = orch.clone();
    // TPM-C (Task 5 step 2): seed the initial model_profile from a
    // profile-qualified default_model.  SessionState::empty starts model_profile
    // at None; this is a no-op when default_model is a bare id.
    if let Some(profile) = default_model_profile.as_deref() {
        if let Err(e) = handle.switch_model(&default_model_id, Some(profile)).await {
            tracing::warn!(error = %e, "failed to seed default model profile");
        }
    }
    orch.spawn_startup_responses_websocket_prewarm();
    // Plan 3c: `/connect` seams — Copilot device-flow over `PosixHttp`, and the
    // API-key writer over the host secure prompt (tui-supplied; headless no-op).
    // M8: also wire the ChatGPT OAuth seam (`/connect chatgpt`).
    let connect_copilot: Arc<dyn command_core::CopilotConnectDriver> =
        Arc::new(crate::connect::EngineCopilotConnect::new(credentials.clone()));
    let connect_writer: Arc<dyn command_core::ConnectCredentialWriter> =
        Arc::new(crate::connect::EngineCredentialWriter::new(
            credentials.clone(),
            cfg.connect_prompt.clone().unwrap_or_else(|| {
                Arc::new(crate::connect::NoopKeyPrompt) as Arc<dyn crate::connect::SecureKeyPrompt>
            }),
        ));
    let connect_chatgpt: Arc<dyn command_core::ChatGptConnectDriver> =
        Arc::new(crate::connect::EngineChatGptConnect::new(
            openai_oauth_client,
            credentials.clone(),
        ));
    let reg = desktop_command_registry(
        handle,
        auth.clone(),
        &cfg.cwd,
        &cfg.claude_home,
        connect_writer,
        connect_copilot,
        connect_chatgpt,
    )
    .await;
    // SKILLEXEC.2: fill the shared command-registry slot the `Skill` tool's
    // loader holds, then hand the SAME `Arc` to the slash dispatcher so the tool
    // and the dispatcher observe one command set (plugin lifecycle mutations via
    // the dispatcher's write lock are visible to the loader too).
    *shared_command_registry.write().await = reg;

    // (6.5) Plugin bootstrap — discover installed plugins on disk and
    //       materialise their COMMANDS + HOOKS into the live registries, plus
    //       their AGENTS into the agent catalog. Mirrors claude-code's
    //       cache-only plugin load at startup (`main.tsx:282`
    //       `loadAllPluginsCacheOnly()` → `pluginLoader.ts:1887`
    //       `loadPluginsFromMarketplaces({cacheOnly})`; `setup.ts:318`
    //       `loadPluginHooks`). Plugins live under `getPluginsDirectory()` =
    //       `~/.claude/plugins` (`pluginDirectories.ts:53`), honoring the
    //       `CLAUDE_CODE_PLUGIN_CACHE_DIR` override. Discovery is allowlist-
    //       driven (faithful): the `settings.enabledPlugins`
    //       (`plugin@marketplace` → enabled) entries resolve to versioned cache
    //       dirs `cache/{marketplace}/{plugin}/{version}/`, the layout
    //       `loadAllPluginsCacheOnly` consumes; a flat-walk fallback covers
    //       pre-fetched local plugin dirs. Each plugin command's BODY +
    //       frontmatter are loaded from its markdown file (not empty), and a
    //       plugin loads all-or-nothing (agent frontmatter is validated before
    //       any registry mutation). Best-effort: a malformed plugin logs a
    //       warning and is skipped — discovery never breaks boot (a fresh
    //       install with no `plugins/` dir yields zero plugins, an exact
    //       no-op). Plugin MCP servers live-connect through the same
    //       `connect_all` path as configured `.mcp.json` servers (the manager
    //       owns the same `mcp_registry` Arc and dials them at `enable()`).
    //       RESIDUAL: marketplace-catalog source
    //       resolution + enterprise allow/blocklist policy, and reading the
    //       exact installed version from `installed_plugins.json` (we probe the
    //       single-version cache dir instead). The manager is given a real but
    //       isolated LSP/skill/output-style/tool registry so `enable()` is
    //       non-panicking while only commands + hooks reach the engine's live
    //       registries.
    {
        let plugins_dir = std::env::var_os("CLAUDE_CODE_PLUGIN_CACHE_DIR").map_or_else(
            || cfg.claude_home.join("plugins"),
            std::path::PathBuf::from,
        );
        // Primary (faithful) path: resolve the `settings.enabledPlugins`
        // allowlist (`plugin@marketplace` → enabled) to versioned cache dirs
        // `cache/{marketplace}/{plugin}/{version}/`, exactly as
        // `loadAllPluginsCacheOnly` (`pluginLoader.ts:1888`) consumes a real
        // `~/.claude/plugins`. Read `enabledPlugins` from the user then project
        // settings (project wins), mirroring `getSettings_DEPRECATED()`.
        let enabled = load_enabled_plugins(&cfg.claude_home, &cwd_for_plugins).await;
        let mut discovered = plugin::discover_enabled_plugins(&plugins_dir, &enabled).await;
        // Fallback: when no allowlist resolves anything (e.g. a flat directory
        // of pre-fetched plugin dirs supplied directly, as with `--add-dir`),
        // flat-walk for direct `.claude-plugin/plugin.json` children. This is
        // NOT the real cache layout but keeps local/dev plugin dirs loadable.
        if discovered.is_empty() {
            discovered = plugin::discover_installed_plugins(&plugins_dir).await;
        }
        if !discovered.is_empty() {
            // Live registries the manager materialises plugin components into:
            // - command  → `shared_command_registry` (drives `/`-completion +
            //   the per-turn skill listing).
            // - hooks    → `plugin_hook_registry` (the orchestrator's clone).
            // - MCP      → `plugin_mcp_registry` (== the orchestrator's
            //   `mcp_registry`; scoped configs are live-connected via
            //   `connect_all`, the same path as configured `.mcp.json`
            //   servers, and the already-spawned reconnect loop covers any
            //   that fail their initial dial).
            // - LSP      → `plugin_lsp_registry` (== the `LSPTool`'s registry).
            // The SKILL and OUTPUT-STYLE registries have no turn-loop consumer
            // yet (skills surface to the model via the command-registry listing,
            // output styles via the dir-based resolver `resolve_output_style`),
            // so they are still local instances here: registration is faithful
            // to `loadAllPlugins`' registry population, but end-to-end
            // consumption of these two registries is separate existing-arch work
            // (residual).
            let pm = plugin::PluginManager::new(
                plugins_dir.clone(),
                Arc::new(PosixFileSystem::new(cwd_for_plugins.clone())),
                http.clone(),
                Arc::new(PosixRuntime::new()),
                credentials.clone(),
                Arc::new(plugin::PluginBlocklist::new(String::new())),
                Arc::new(plugin::StrictPluginOnlyPolicy::empty()),
                shared_command_registry.clone(),
                Arc::new(RwLock::new(SkillRegistry::new())),
                plugin_hook_registry.clone(),
                Arc::new(RwLock::new(outputstyles::OutputStyleRegistry::new())),
                plugin_mcp_registry.clone(),
                plugin_lsp_registry.clone(),
                Arc::new(RwLock::new(ToolRegistry::new())),
            );
            for (id, manifest, dir) in discovered {
                let plugin_name = manifest.name.clone();
                // Materialise the plugin's AGENTS into the live catalog via the
                // dir-scan loader (the manager validates agent frontmatter but
                // does not own the catalog). Plugin agents win on collision
                // (passed as a later contribution).
                let agents_dir = dir.join("agents");
                if agents_dir.is_dir() {
                    let plugin_agents = agent::load_agents_from_dirs(&[(
                        agents_dir,
                        agent::definition::AgentSource::Plugin,
                    )])
                    .await;
                    if !plugin_agents.is_empty() {
                        let mut cat = plugin_agent_catalog.write().await;
                        for a in plugin_agents {
                            // Replace any same-named agent; otherwise append.
                            if let Some(slot) =
                                cat.iter_mut().find(|e| e.agent_type == a.agent_type)
                            {
                                *slot = a;
                            } else {
                                cat.push(a);
                            }
                        }
                    }
                }
                // Materialise COMMANDS + HOOKS (and validate agent frontmatter).
                if let Err(e) = pm.enable(&id, manifest, dir).await {
                    tracing::warn!(
                        plugin = %plugin_name,
                        error = %e,
                        "skipping plugin that failed to load"
                    );
                }
            }
        }
    }

    // #39 UserPromptExpansion: wire the dispatcher to fire `UserPromptExpansion`
    // (claude-code `WFa`→`b$t`) the moment it expands a markdown / MCP-prompt
    // slash command. The context provider reads THIS conversation's live
    // session id + cwd from the orchestrator (`expansion_hook_context`), matching
    // the base hook input the orchestrator's own lifecycle hooks build. A strict
    // no-op unless a `UserPromptExpansion` hook is registered.
    let expansion_ctx_orch = orch.clone();
    let dispatcher = RegistrySlashDispatcher::new(shared_command_registry.clone())
        .with_expansion_hooks(
            expansion_hook_executor,
            std::sync::Arc::new(move || {
                let orch = expansion_ctx_orch.clone();
                Box::pin(async move { orch.expansion_hook_context().await })
            }),
        );

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
            Arc::new(PosixFileSystem::new(watch_cwd.clone()));
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
            let watcher =
                file_changed_watch::FileChangedWatcher::new(&matcher_refs, &watch_cwd, firer);
            // Empty resolved-path set (matcher-less hooks only) ⇒ spawn returns
            // an empty handle, so this stays a no-op even when a `FileChanged`
            // hook is present but specifies no watch target.
            if watcher.watch_paths().is_empty() {
                file_changed_watch::FileChangedWatcherHandle::empty()
            } else {
                let watch_fs: Arc<dyn traits::FileSystem> =
                    Arc::new(PosixFileSystem::new(watch_cwd.clone()));
                watcher.spawn(watch_fs).await
            }
        }
    };

    // Phase 2a §6.2: per-profile availability from the assembled credential
    // sources (each profile is "available" iff its keychain entry / env var
    // resolves). Drives the `/model` picker's Connect badge.
    let mut provider_availability: std::collections::BTreeMap<String, bool> =
        provider_config::compute_availability(
            &credentials,
            &assembled.credential_sources,
            has_api_key,
            has_oauth,
            has_openai_chatgpt,
        )
        .await
        .into_iter()
        .map(|a| (a.profile_name, a.available))
        .collect();
    // `assemble` emits NO anthropic credential source in the unauthenticated
    // (no key / no oauth) path, so `compute_availability` yields no "anthropic"
    // entry there. The picker's Connect badge still needs anthropic represented,
    // so surface it unconditionally from the engine's resolved auth state.
    provider_availability
        .entry("anthropic".to_string())
        .or_insert(has_api_key || has_oauth);

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
        subscription,
        provider_availability,
        model_providers,
        provider_adapter: provider_adapter_handle,
        credentials,
        structured_output_slot,
    })
}

#[cfg(test)]
mod tests {
    use super::{build, desktop_tool_registry, model_deprecation_warning, CoordinatorWiring, DesktopConfig};
    use std::sync::Arc;

    // ── Plan 3c `/connect` wiring tests ──────────────────────────────────────

    /// `/connect` is wired into the desktop registry through
    /// [`super::desktop_command_registry`] (additive, not a locked builtin name).
    #[tokio::test]
    async fn desktop_registry_exposes_connect() {
        use async_trait::async_trait;
        use command_core::{
            ChatGptConnectDriver, ConnectCredentialWriter, ConnectError, CopilotConnectDriver,
            CopilotConnectStep,
        };
        use traits::{AuthError, AuthHandle, LoginInfo, OrchestratorHandle};

        // Minimal `AuthHandle` double — no sibling registry test exists in this
        // module, so we construct the lightest object-safe stand-in here.
        struct MockAuth;
        #[async_trait]
        impl AuthHandle for MockAuth {
            async fn login(&self) -> Result<LoginInfo, AuthError> {
                Err(AuthError::Cancelled)
            }
            async fn logout(&self) -> Result<(), AuthError> {
                Ok(())
            }
            async fn current_user(&self) -> Option<LoginInfo> {
                None
            }
        }

        struct W;
        #[async_trait]
        impl ConnectCredentialWriter for W {
            async fn prompt_and_store_key(&self, _id: &str) -> Result<(), ConnectError> {
                Ok(())
            }
        }
        struct C;
        #[async_trait]
        impl CopilotConnectDriver for C {
            async fn begin(&self) -> Result<CopilotConnectStep, ConnectError> {
                Ok(CopilotConnectStep {
                    user_code: "X".into(),
                    verification_uri: "u".into(),
                })
            }
            async fn poll_to_completion(&self, _s: &CopilotConnectStep) -> Result<(), ConnectError> {
                Ok(())
            }
        }
        struct G;
        #[async_trait]
        impl ChatGptConnectDriver for G {
            async fn connect(&self) -> Result<String, ConnectError> {
                Ok("Connected chatgpt.".into())
            }
        }

        let handle: Arc<dyn OrchestratorHandle> =
            Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let auth: Arc<dyn AuthHandle> = Arc::new(MockAuth);
        let tmp = std::env::temp_dir();
        let reg = super::desktop_command_registry(
            handle,
            auth,
            &tmp,
            &tmp,
            Arc::new(W),
            Arc::new(C),
            Arc::new(G),
        )
        .await;
        assert!(
            reg.get_handler("connect").is_some(),
            "/connect not wired into desktop registry"
        );
    }

    /// (Plan 3c C1) The [`super::connect::EngineCredentialWriter`] persists the
    /// prompted key through `CredentialManager::set_provider_key`; a later
    /// `get_provider_key` returns the exact secret — proving the keychain bridge
    /// roundtrips (no log-and-drop).
    #[tokio::test]
    async fn engine_credential_writer_roundtrips_through_keychain() {
        use super::connect::{EngineCredentialWriter, SecureKeyPrompt};
        use async_trait::async_trait;
        use command_core::ConnectCredentialWriter;
        use std::collections::HashMap;
        use std::sync::Mutex as StdMutex;
        use traits::{
            Clock, HttpTransport, SecureStorage, SecureStorageBackend, SecureStorageError,
        };

        // In-memory secure store (the posix-minimal stub does not persist).
        #[derive(Default)]
        struct MemStorage {
            map: StdMutex<HashMap<(String, String), protocol::SecureStorageData>>,
        }
        #[async_trait]
        impl SecureStorage for MemStorage {
            async fn store(
                &self,
                service: &str,
                account: &str,
                data: protocol::SecureStorageData,
            ) -> Result<(), SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .insert((service.into(), account.into()), data);
                Ok(())
            }
            async fn retrieve(
                &self,
                service: &str,
                account: &str,
            ) -> Result<Option<protocol::SecureStorageData>, SecureStorageError> {
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .get(&(service.into(), account.into()))
                    .cloned())
            }
            async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .remove(&(service.into(), account.into()));
                Ok(())
            }
            async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError> {
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .keys()
                    .filter(|(s, _)| s == service)
                    .map(|(_, a)| a.clone())
                    .collect())
            }
            fn is_encrypted(&self) -> bool {
                false
            }
            fn backend(&self) -> SecureStorageBackend {
                SecureStorageBackend::PlainText
            }
        }

        struct CannedPrompt(Option<String>);
        #[async_trait]
        impl SecureKeyPrompt for CannedPrompt {
            async fn prompt(&self, _label: &str) -> Option<String> {
                self.0.clone()
            }
        }

        let storage: Arc<dyn SecureStorage> = Arc::new(MemStorage::default());
        let clock: Arc<dyn Clock> = Arc::new(platform_posix::PosixClock::new());
        let http: Arc<dyn HttpTransport> = Arc::new(platform_posix::PosixHttp::new());
        let cm = Arc::new(secret::CredentialManager::new(storage, clock, http));

        let writer =
            EngineCredentialWriter::new(cm.clone(), Arc::new(CannedPrompt(Some("sk-test-123".into()))));
        writer
            .prompt_and_store_key("openrouter")
            .await
            .expect("store ok");
        let got = cm
            .get_provider_key("openrouter")
            .await
            .expect("read ok")
            .expect("present");
        assert_eq!(got.expose_secret(), "sk-test-123");
    }

    // ── deprecation tests ────────────────────────────────────────────────────
    // Ported from the deleted `providers/src/deprecation.rs` unit tests
    // (Plan 3b Task 5) so the behaviour stays covered at the new home.

    /// Env access in these tests is process-global; serialize them so the
    /// provider flags one test sets can't leak into another running in parallel.
    static DEPR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_provider_env() {
        std::env::remove_var("CLAUDE_CODE_USE_BEDROCK");
        std::env::remove_var("CLAUDE_CODE_USE_VERTEX");
        std::env::remove_var("CLAUDE_CODE_USE_FOUNDRY");
    }

    #[test]
    fn depr_bedrock_and_vertex_env_matrix() {
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();

        // Bedrock: Opus has a different (later) date; Haiku 3.5 has none → None.
        std::env::set_var("CLAUDE_CODE_USE_BEDROCK", "1");
        assert_eq!(
            model_deprecation_warning(Some("claude-3-opus-20240229")).as_deref(),
            Some("⚠ Claude 3 Opus will be retired on January 15, 2026. Consider switching to a newer model.")
        );
        assert_eq!(model_deprecation_warning(Some("claude-3-5-haiku-20241022")), None);
        clear_provider_env();

        // Vertex: 3.7 Sonnet has a different date.
        std::env::set_var("CLAUDE_CODE_USE_VERTEX", "1");
        assert_eq!(
            model_deprecation_warning(Some("claude-3-7-sonnet-20250219")).as_deref(),
            Some("⚠ Claude 3.7 Sonnet will be retired on May 11, 2026. Consider switching to a newer model.")
        );
        clear_provider_env();
    }

    #[test]
    fn depr_bedrock_null_haiku() {
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();
        std::env::set_var("CLAUDE_CODE_USE_BEDROCK", "1");
        // claude-3-5-haiku has `bedrock: None` in the table → no warning.
        assert_eq!(model_deprecation_warning(Some("claude-3-5-haiku-20241022")), None);
        clear_provider_env();
    }

    #[test]
    fn depr_case_insensitive_substring() {
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();
        // The key match is lowercased, so uppercase input still matches.
        assert_eq!(
            model_deprecation_warning(Some("CLAUDE-3-OPUS-20240229")).as_deref(),
            Some("⚠ Claude 3 Opus will be retired on January 5, 2026. Consider switching to a newer model.")
        );
        // Bedrock-prefixed id still matches the substring.
        assert!(model_deprecation_warning(Some("anthropic.claude-3-opus-20240229-v1:0")).is_some());
    }

    #[test]
    fn depr_first_party_deprecated_models_warn() {
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();
        assert_eq!(
            model_deprecation_warning(Some("claude-3-opus-20240229")).as_deref(),
            Some("⚠ Claude 3 Opus will be retired on January 5, 2026. Consider switching to a newer model.")
        );
        assert_eq!(
            model_deprecation_warning(Some("claude-3-7-sonnet-20250219")).as_deref(),
            Some("⚠ Claude 3.7 Sonnet will be retired on February 19, 2026. Consider switching to a newer model.")
        );
        assert_eq!(
            model_deprecation_warning(Some("claude-3-5-haiku-20241022")).as_deref(),
            Some("⚠ Claude 3.5 Haiku will be retired on February 19, 2026. Consider switching to a newer model.")
        );
    }

    #[test]
    fn depr_none_or_empty_model_yields_no_warning() {
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();
        assert_eq!(model_deprecation_warning(None), None);
        assert_eq!(model_deprecation_warning(Some("")), None);
    }
    // ── end deprecation tests ────────────────────────────────────────────────

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
        async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
            deny_unresolved_ask: false,
            max_turns: None,
            max_budget_usd: None,
            json_schema: None,
            injected_permission_gate: None,
            session_started_as_coordinator: false,
            // Boot tests stay deterministic: empty memory, never the real FS.
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
            connect_prompt: None,
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

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        assert!(rt.orchestrator.has_cost_tracker(), "no CostTracker");
        assert!(rt.orchestrator.has_mcp_registry(), "no McpRegistry");
        assert!(rt.orchestrator.has_hook_registry(), "no HookRegistry");
        assert!(rt.orchestrator.has_agent_catalog(), "no agent catalog");
        assert!(
            rt.orchestrator.has_compaction(),
            "no CompactionOrchestrator"
        );
    }

    /// The production-built `McpRegistry` must carry the OAuth seam
    /// ([`mcp::registry::OAuthDeps`]). Without `.with_oauth(..)` in `build()`,
    /// OAuth-configured remote MCP servers can't authenticate (they silently
    /// fall back to static headers). This asserts the composition root wires it.
    #[tokio::test]
    async fn build_wires_mcp_oauth_seam() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        assert!(
            rt.orchestrator.has_mcp_oauth(),
            "OAuthDeps not wired into the production MCP registry"
        );
        // Without an `xaaIdp` settings tier, the XAA config layer stays opt-in:
        // `xaa_config` is None, so an XAA-flagged server keeps its hard-fail.
        assert!(
            !rt.orchestrator.has_mcp_xaa(),
            "XAA must stay opt-in when no `xaaIdp` settings tier is present"
        );
    }

    /// With an `xaaIdp` settings tier present (`{issuer, clientId}`), `build()`
    /// constructs a concrete [`mcp::registry::XaaConfigProvider`] and wires it
    /// into the registry's `OAuthDeps`, so an `oauth.xaa` server can resolve a
    /// token via the Cross-App-Access chain. Mirror of claude-code's
    /// `getXaaIdpSettings` gating `performMCPXaaAuth`.
    #[tokio::test]
    async fn build_wires_xaa_config_when_xaaidp_settings_present() {
        let (_tmp, cfg) = test_config(true);
        // Lay down a settings.json with an `xaaIdp` block under claude_home.
        std::fs::create_dir_all(&cfg.claude_home).unwrap();
        std::fs::write(
            cfg.claude_home.join("settings.json"),
            r#"{"xaaIdp":{"issuer":"https://idp.example.com","clientId":"idp-client-id"}}"#,
        )
        .unwrap();

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        assert!(
            rt.orchestrator.has_mcp_xaa(),
            "XaaConfigProvider not wired into OAuthDeps despite `xaaIdp` settings"
        );
    }

    /// GAP E: a plugin installed on disk under `<claude_home>/plugins` is
    /// discovered + materialised at bootstrap — its command lands in the live
    /// command registry the slash dispatcher reads.
    #[tokio::test]
    async fn build_discovers_and_materialises_an_installed_plugin() {
        let (_tmp, cfg) = test_config(true);
        // Lay down a fixture plugin under `<claude_home>/plugins/myplugin`.
        let plugin_dir = cfg.claude_home.join("plugins").join("myplugin");
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            plugin_dir.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"myplugin","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(plugin_dir.join("commands")).unwrap();
        std::fs::write(
            plugin_dir.join("commands").join("hello.md"),
            "---\ndescription: greets\n---\nHello from the plugin.\n",
        )
        .unwrap();

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        // The plugin command is reachable through the dispatcher's registry.
        let reg = rt.dispatcher.registry();
        let reg = reg.read().await;
        let cmd = reg
            .resolve("myplugin:hello")
            .expect("plugin command `myplugin:hello` should be discovered at bootstrap");
        assert_eq!(cmd.source, command_api::CommandSource::Plugin);
        // Verification fix #2: the command body must be loaded — an empty
        // prompt_template would expand to an inert prompt.
        assert_eq!(cmd.description, "greets");
        match &cmd.kind {
            command_api::SlashCommandKind::Plugin {
                prompt_template, ..
            } => assert!(
                prompt_template.contains("Hello from the plugin."),
                "plugin command body must reach the live registry"
            ),
            other => panic!("expected Plugin kind, got {other:?}"),
        }
    }

    /// GAP E (verification fix #1): a plugin laid out under the REAL claude-code
    /// cache layout `plugins/cache/{marketplace}/{plugin}/{version}/` and named
    /// in `settings.enabledPlugins` is discovered + materialised at bootstrap.
    /// The flat walk would find nothing here — only the allowlist-driven
    /// resolution does.
    #[tokio::test]
    async fn build_discovers_a_plugin_via_enabledplugins_cache_layout() {
        let (_tmp, cfg) = test_config(true);
        // Versioned cache dir, exactly as getVersionedCachePath lays it out.
        let versioned = cfg
            .claude_home
            .join("plugins")
            .join("cache")
            .join("acme")
            .join("weather")
            .join("1.0.0");
        std::fs::create_dir_all(versioned.join(".claude-plugin")).unwrap();
        std::fs::write(
            versioned.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"weather","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(versioned.join("commands")).unwrap();
        std::fs::write(
            versioned.join("commands").join("forecast.md"),
            "---\ndescription: forecast\n---\nThe forecast is sunny.\n",
        )
        .unwrap();
        // Enable it via user settings.json `enabledPlugins`.
        std::fs::write(
            cfg.claude_home.join("settings.json"),
            r#"{"enabledPlugins":{"weather@acme":true}}"#,
        )
        .unwrap();

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        let reg = rt.dispatcher.registry();
        let reg = reg.read().await;
        let cmd = reg
            .resolve("weather:forecast")
            .expect("namespaced plugin command discovered via enabledPlugins cache layout");
        assert_eq!(cmd.source, command_api::CommandSource::Plugin);
        match &cmd.kind {
            command_api::SlashCommandKind::Plugin {
                prompt_template, ..
            } => assert!(prompt_template.contains("The forecast is sunny.")),
            other => panic!("expected Plugin kind, got {other:?}"),
        }
    }

    /// GAP E: a fresh install with no `<claude_home>/plugins` directory boots
    /// with zero plugins — discovery is a strict no-op (non-breaking).
    #[tokio::test]
    async fn build_with_no_plugins_dir_is_a_noop() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        // Must not panic / error; no plugin commands present.
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        let reg = rt.dispatcher.registry();
        let reg = reg.read().await;
        assert!(reg.resolve("hello").is_none());
    }

    #[tokio::test]
    async fn build_with_json_schema_surfaces_structured_output_slot() {
        // `--json-schema` ⇒ build() registers the forced `StructuredOutput` tool
        // and surfaces its capture slot for the print path.
        let (_tmp, mut cfg) = test_config(true);
        cfg.json_schema = Some(serde_json::json!({ "type": "object" }));
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        assert!(
            rt.structured_output_slot.is_some(),
            "--json-schema must surface a structured-output capture slot"
        );
    }

    #[tokio::test]
    async fn build_without_json_schema_has_no_structured_output_slot() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        assert!(
            rt.structured_output_slot.is_none(),
            "a default build (no --json-schema) must not surface a slot"
        );
    }

    // ── Phase 2a: helper unit tests (T2) ─────────────────────────────────────

    /// `provider_profile_label` returns the hand-curated header for known
    /// profiles and Title-Cases an unknown USER profile name on `-`/`_`/space.
    #[test]
    fn provider_profile_label_known_and_titlecased() {
        assert_eq!(super::provider_profile_label("anthropic"), "Anthropic");
        assert_eq!(super::provider_profile_label("openrouter"), "OpenRouter");
        assert_eq!(super::provider_profile_label("deepseek"), "DeepSeek");
        assert_eq!(super::provider_profile_label("glm-coding"), "GLM (coding)");
        assert_eq!(super::provider_profile_label("github-copilot"), "GitHub Copilot");
        // Unknown user profiles are Title-Cased across separators.
        assert_eq!(super::provider_profile_label("groq"), "Groq");
        assert_eq!(super::provider_profile_label("my-provider"), "My Provider");
        assert_eq!(super::provider_profile_label("ACME_corp"), "Acme Corp");
    }

    /// `anthropic_models_for` always includes the first-party defaults plus the
    /// configured default + fallback model, deduped, with a `ModelProfile` per id.
    #[test]
    fn anthropic_models_for_includes_defaults_and_configured() {
        let models = super::anthropic_models_for("claude-sonnet-4-6", Some("claude-opus-4-6"));
        let ids: Vec<&str> = models.iter().map(|m| m.display_model.as_str()).collect();
        // First-party defaults are present.
        assert!(ids.contains(&"claude-opus-4-6"), "missing default opus: {ids:?}");
        assert!(ids.contains(&"claude-sonnet-4-6"), "missing default sonnet: {ids:?}");
        assert!(ids.contains(&"claude-haiku-4-5"), "missing default haiku: {ids:?}");
        // A configured default/fallback already in the list does not duplicate.
        assert_eq!(
            ids.iter().filter(|id| **id == "claude-sonnet-4-6").count(),
            1,
            "configured default must be deduped, ids: {ids:?}"
        );
        // request_model / billing_model mirror display_model for these profiles.
        for m in &models {
            assert_eq!(m.request_model, m.display_model);
            assert_eq!(m.billing_model, m.display_model);
        }
        // A NEW configured default id is added.
        let custom = super::anthropic_models_for("my-custom-model", None);
        assert!(
            custom.iter().any(|m| m.display_model == "my-custom-model"),
            "configured default must be registered"
        );
    }

    // ── Phase 2a: build() provider-routing wiring tests (T10) ─────────────────

    /// Phase 2a: a default `build()` (no api key, no oauth, no settings
    /// providers) still surfaces the multi-provider `provider_availability` map
    /// (anthropic unavailable + every built-in catalog preset) and the concrete
    /// routing adapter handle. Ported from parity's
    /// `build_surfaces_provider_availability_and_adapter`.
    #[tokio::test]
    async fn build_surfaces_provider_availability_and_adapter() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        // Anthropic always represented; with no key/oauth it is unavailable.
        assert_eq!(rt.provider_availability.get("anthropic"), Some(&false));
        // Built-in catalog presets are merged into the availability map.
        assert!(
            rt.provider_availability.contains_key("deepseek"),
            "availability map missing builtin preset: {:?}",
            rt.provider_availability
        );
        // The concrete routing adapter is surfaced.
        assert!(Arc::strong_count(&rt.provider_adapter) >= 1);
        // The default `model_providers` map groups a built-in Anthropic model
        // under the anthropic profile.
        assert_eq!(
            rt.model_providers.get("claude-sonnet-4-6"),
            Some(&("anthropic".to_string(), "Anthropic".to_string())),
        );
    }

    /// Phase 2a (T10 integration): a `build()` with BOTH a user-defined provider
    /// profile AND a routing fallback chain must merge into the assembled
    /// `ClientConfig` (a user-provider model + a built-in catalog model), surface
    /// the user profile in `model_providers` + `provider_availability`, and the
    /// routing chain must translate into main's `fallback_overrides` shape (the
    /// translation `build()` performs is re-derived here from `assemble`, since
    /// the adapter's overrides field is private).
    #[tokio::test]
    async fn build_with_providers_and_routing_merges_config_chains_availability() {
        let (_tmp, mut cfg) = test_config(true);
        cfg.provider_profiles = Some({
            let mut m = std::collections::BTreeMap::new();
            // NB: `provider-config`'s `parse_user_providers` (the dialect `build()`
            // now routes through via `assemble`) takes `models` as a STRING ARRAY
            // of model ids — distinct from the legacy `apply_settings_providers`
            // dialect (`[{ "id": ... }]`) the surviving e2e tests below still use.
            m.insert(
                "groq".to_string(),
                serde_json::json!({
                    "type": "openai",
                    "baseUrl": "https://api.groq.com/openai/v1",
                    "apiKeyEnv": "GROQ_API_KEY",
                    "models": ["llama-3.3-70b-versatile"]
                }),
            );
            m
        });
        // A fallback chain keyed on a builtin Anthropic model, falling back to the
        // user-defined groq model (cross-provider routing by model id).
        cfg.routing = Some(serde_json::json!({
            "fallback": {
                "claude-sonnet-4-6": ["groq/llama-3.3-70b-versatile"]
            }
        }));

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg.clone(), output, perm_sink)
            .await
            .expect("build() failed with providers + routing");

        // (1) The merged config exposes BOTH a user-provider model and a built-in
        //     catalog model via `model_providers` (built from
        //     `assembled.client_config.providers`).
        assert_eq!(
            rt.model_providers.get("llama-3.3-70b-versatile"),
            Some(&("groq".to_string(), "Groq".to_string())),
            "user provider model must group under its own profile; got: {:?}",
            rt.model_providers
        );
        assert_eq!(
            rt.model_providers.get("claude-sonnet-4-6"),
            Some(&("anthropic".to_string(), "Anthropic".to_string())),
            "built-in anthropic model must group under anthropic"
        );
        // A built-in CATALOG preset model is also present (e.g. a deepseek model).
        assert!(
            rt.model_providers
                .values()
                .any(|(profile, _)| profile == "deepseek"),
            "a built-in catalog preset model must appear in model_providers"
        );

        // (2) The availability map carries the user profile (no GROQ_API_KEY in
        //     the test env ⇒ unavailable) alongside anthropic + presets.
        assert_eq!(
            rt.provider_availability.get("groq"),
            Some(&false),
            "user provider must be present + unavailable without its key"
        );
        assert_eq!(rt.provider_availability.get("anthropic"), Some(&false));
        assert!(rt.provider_availability.contains_key("deepseek"));

        // (3) The routing chain translates into main's `fallback_overrides` shape.
        //     Re-derive the same translation `build()` performs from `assemble`
        //     (the adapter's private `fallback_overrides` field is not inspectable).
        let assembled = provider_config::assemble(provider_config::AssembleInputs {
            anthropic_api_base: cfg.api_base.clone(),
            anthropic_models: super::anthropic_models_for(
                &cfg.default_model,
                cfg.fallback_model.as_deref(),
            ),
            anthropic_has_api_key: false,
            anthropic_has_oauth: false,
            user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
            routing: cfg.routing.clone(),
        });
        let fallback_overrides: std::collections::BTreeMap<String, Vec<String>> = assembled
            .chains
            .chains
            .iter()
            .map(|(k, entries)| (k.clone(), entries.iter().map(|e| e.model.clone()).collect()))
            .collect();
        assert_eq!(
            fallback_overrides.get("claude-sonnet-4-6").map(Vec::as_slice),
            Some(&["llama-3.3-70b-versatile".to_string()][..]),
            "fallback chain must translate to the bare model-id list (provider_id dropped); got: {fallback_overrides:?}"
        );
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

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        assert!(
            rt.permission_gate.is_none(),
            "noop build must not surface an adapter gate handle"
        );
    }

    /// Unit 2 seam: a host-injected base gate (the interactive TUI's
    /// `TuiPermissionGate`) takes precedence over the `use_noop`/`deny`
    /// selection and surfaces NO adapter handle (it is its own transport). Build
    /// succeeds with the injected gate as the base perms; its WRAP behavior
    /// (rules + read-only auto-allow resolved before the gate sees an `Ask`) is
    /// covered by `permission::policy_gate`'s `PolicyPermissionGate` tests.
    #[tokio::test]
    async fn build_with_injected_gate_prefers_it_over_noop() {
        // `test_config(true)` would normally bind `NoOpPermissionGate`; the
        // injected gate must win.
        let (_tmp, mut cfg) = test_config(true);
        cfg.injected_permission_gate = Some(Arc::new(permission::DenyOnAskGate));
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() with an injected gate failed");

        assert!(
            rt.permission_gate.is_none(),
            "an injected base gate is its own transport — no adapter handle"
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

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

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
            gate.resolve(
                1,
                client_protocol::permission::PermissionResponseDto::Deny,
                "Bash"
            )
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

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

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
        let rt = build(cfg, output, perm_sink).await.expect(
            "build() must succeed even with a (failing) InstructionsLoaded hook registered",
        );

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
    /// and lands in the assembled SYSTEM PROMPT (the GAP-3 memory section —
    /// preamble + `Contents of …:` with the file's path + tier + body). The
    /// default-empty sibling ([`build_constructs_runtime_deterministically`]
    /// etc.) elides the memory section entirely, so the section's presence is
    /// the load-bearing difference the injected provider makes.
    ///
    /// The end-to-end "`fire_instructions_loaded()` fires the registered
    /// `InstructionsLoaded` hook over the controlled memory" half is proven at
    /// the orchestrator layer in `orchestrator/tests/instructions_loaded_hook_test.rs`
    /// (a `RecordingHandler` observes the per-file fire). It is NOT re-asserted
    /// here because `build()` exposes no outside-observable channel for an
    /// in-build hook fire's side effect: the `command` hook runs on the real
    /// `platform_posix::PosixProcess` runner now, but its `"true"` command is a
    /// side-effect-free no-op whose output `build()` does not surface. This test
    /// therefore asserts hook *registration* (via `list_hooks()` below), not the
    /// hook's execution effect. We register the hook anyway, so the fire still
    /// runs over the injected file (best-effort) inside `build()`.
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
            tier: orchestrator::prompt::ClaudeMdTier::Project,
            globs: None,
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
        // memory section (GAP 3 — preamble + `Contents of …:` per file, 1:1 with
        // claude-code getClaudeMds) carries the file's path + tier description +
        // body. This proves the controlled provider flowed through build() into
        // the orchestrator's prompt assembly — the gap (desktop loads NO memory)
        // is closed.
        // R-P1: claudeMd lives in the leading additional-context `<system-reminder>`
        // meta now (built from the SAME `memory_block::format`), NOT the system
        // prompt. The injected CLAUDE.md must reach THAT.
        let ctx = rt
            .orchestrator
            .additional_context_preview()
            .await
            .expect("an additional-context meta must be present (currentDate is unconditional)");
        assert!(
            ctx.contains(
                "Codebase and user instructions are shown below. Be sure to adhere to these instructions."
            ),
            "injected memory must emit the memory preamble in the additional-context meta: {ctx}"
        );
        assert!(
            ctx.contains(&format!(
                "Contents of {} (project instructions, checked into the codebase):",
                memory_path.display()
            )),
            "the injected CLAUDE.md must emit a tier-tagged `Contents of …:` marker: {ctx}"
        );
        assert!(
            ctx.contains(memory_body),
            "the injected CLAUDE.md body must appear in the additional-context meta: {ctx}"
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
    /// NO memory, so the system prompt emits NO memory section (no preamble, no
    /// `Contents of …:` markers). This pins that the existing boot tests stay
    /// deterministic (they never read the real `~/.claude/CLAUDE.md`).
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

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        assert!(
            !sys.contains("Codebase and user instructions are shown below."),
            "default (empty) memory provider must elide the memory section: {sys}"
        );
        assert!(
            !sys.contains("Contents of "),
            "default (empty) memory provider must emit no `Contents of …:` marker: {sys}"
        );
    }

    /// D1 ITEM 4: a coordinator session's assembled system prompt IS the
    /// coordinator prompt (TS `buildEffectiveSystemPrompt` coordinator branch),
    /// carrying the role header + tool names + the worker-tools USER context as a
    /// trailing `<system-reminder>`. The default-session control proves the swap
    /// is load-bearing (a normal session emits the standard prompt, NOT the
    /// coordinator one).
    #[tokio::test]
    async fn coordinator_session_assembles_coordinator_system_prompt() {
        let (_tmp, mut cfg) = test_config(true);
        cfg.session_started_as_coordinator = true;

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("coordinator build must succeed");

        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        // Coordinator role header + interpolated tool names.
        assert!(
            sys.contains(
                "You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers."
            ),
            "coordinator session must assemble the coordinator system prompt: {sys}"
        );
        assert!(sys.contains("You are a **coordinator**."));
        assert!(sys.contains("**Agent** - Spawn a new worker"));
        assert!(sys.contains("**SendMessage** - Continue an existing worker"));
        assert!(sys.contains("**TaskStop** - Stop a running worker"));
        // The per-turn worker-tools user context rides along as a system-reminder.
        assert!(
            sys.contains("<system-reminder>")
                && sys.contains("Workers spawned via the Agent tool have access to these tools:"),
            "coordinator user context must be injected as a system-reminder: {sys}"
        );
    }

    /// Control for the above: a DEFAULT session must NOT assemble the coordinator
    /// prompt — its system prompt is the standard LingXi header.
    #[tokio::test]
    async fn default_session_does_not_assemble_coordinator_prompt() {
        let (_tmp, cfg) = test_config(true);
        assert!(!cfg.session_started_as_coordinator);

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink).await.expect("build failed");
        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        assert!(
            !sys.contains("You are a **coordinator**."),
            "a default session must NOT use the coordinator system prompt: {sys}"
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
            _team_name: String,
            _description: String,
        ) -> Result<String, traits::team_spawn::TeamSpawnError> {
            Ok(String::new())
        }
        async fn kill(&self, _task_id: &str) -> Result<(), traits::team_spawn::TeamSpawnError> {
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
            bus: None,
            // Tool-selection tests don't exercise the pump; no spawner needed.
            runtime: None,
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

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

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

    /// A coordinator session can route a `SendMessage` to a registered worker
    /// mailbox. Since batch D2b the coordinator session registers the richer
    /// `coordinator` `SendMessage` IN PLACE OF the `tool_ui` builtin (it routes
    /// through the `TeamRegistry`'s OWN `MailboxRouter` directly, resolving the
    /// recipient by teammate name / agent id). A route to a registered worker
    /// therefore SUCCEEDS. The shared router is also still wired onto
    /// `ctx.mailbox_router` (the load-bearing T13 decision for the OTHER tools
    /// that read it, e.g. `TaskUpdate`'s owner-change notification).
    ///
    /// The negative side: a DEFAULT (non-coordinator) session registers the
    /// leaner `tool_ui` builtin, which reads `ctx.mailbox_router`; with `None`
    /// (the default-session default) its route takes the "router not wired"
    /// `Internal` error path. This locks both sides of the wiring decision.
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
        // context (still wired for the tools that read it), plus the coordinator
        // wiring that splices the coordinator `SendMessage` in.
        let mut ctx = stub_tool_ctx();
        ctx.mailbox_router =
            Some(team.mailbox_router.clone() as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        let wiring = CoordinatorWiring {
            team: team.clone(),
            mode: Arc::new(coordinator::CoordinatorMode::new()),
            spawn_seam: Arc::new(NoopSeam),
            output: Arc::new(orchestrator::test_support::MockOutputStream::new()),
            bus: None,
            // This test exercises SendMessage→mailbox routing only, not the
            // teammate pump; no spawner needed.
            runtime: None,
        };
        let reg = desktop_tool_registry(ctx, Some(wiring), None);

        // The coordinator `SendMessage` resolves the recipient by name and routes
        // through `team.mailbox_router` directly — a route to the registered
        // worker "alpha" succeeds.
        let send = reg
            .find_by_name("SendMessage")
            .expect("SendMessage must be registered");
        let result = send
            .call(
                serde_json::json!({
                    "to": "alpha",
                    "summary": "kick off",
                    "message": "hello teammate",
                }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("coordinator SendMessage must route to the registered worker mailbox");
        assert_eq!(
            result.data["success"], true,
            "a route to a registered worker returns success"
        );

        // Negative side: the DEFAULT branch registers the `tool_ui` builtin and
        // leaves `mailbox_router` `None`, so its route takes the "router not
        // wired" `Internal` error path.
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
        let enabled = sandbox_auto_allow_from_settings_tiers(
            &[r#"{ "sandbox": { "enabled": true } }"#],
            std::path::Path::new("/tmp"),
        );
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
        let with_excludes = sandbox_auto_allow_from_settings_tiers(
            &[r#"{ "sandbox": { "enabled": true, "excludedCommands": ["bazel:*", "make"] } }"#],
            std::path::Path::new("/tmp"),
        );
        assert!(with_excludes.enabled);
        assert_eq!(
            with_excludes.excluded_commands,
            vec!["bazel:*".to_string(), "make".to_string()]
        );
        assert!(!with_excludes.auto_allows("bazel build //..."));
        assert!(with_excludes.auto_allows("echo hi"));

        // (3) Explicit autoAllowBashIfSandboxed:false overrides the TS default;
        //     the command would still be sandboxed but is NOT auto-allowed.
        let auto_off = sandbox_auto_allow_from_settings_tiers(
            &[r#"{ "sandbox": { "enabled": true, "autoAllowBashIfSandboxed": false } }"#],
            std::path::Path::new("/tmp"),
        );
        assert!(auto_off.enabled);
        assert!(!auto_off.auto_allow_bash_if_sandboxed);
        assert!(!auto_off.auto_allows("echo hi"));

        // (4) Sandbox DISABLED → never auto-allows even though autoAllow defaults
        //     true.
        let disabled = sandbox_auto_allow_from_settings_tiers(
            &[r#"{ "sandbox": { "enabled": false } }"#],
            std::path::Path::new("/tmp"),
        );
        assert!(!disabled.enabled);
        assert!(!disabled.auto_allows("echo hi"));

        // (5) No `sandbox` subsection at all (the common case) → disabled, inert.
        let none = sandbox_auto_allow_from_settings_tiers(
            &[r#"{ "permissions": { "allow": [] } }"#],
            std::path::Path::new("/tmp"),
        );
        assert!(!none.enabled);
        assert!(!none.auto_allows("echo hi"));

        // (6) Tier precedence: a later tier's sandbox subsection overrides an
        //     earlier one (ascending priority, last write wins).
        let layered = sandbox_auto_allow_from_settings_tiers(
            &[
                r#"{ "sandbox": { "enabled": false } }"#, // user
                r#"{ "sandbox": { "enabled": true } }"#,  // project (wins)
            ],
            std::path::Path::new("/tmp"),
        );
        assert!(layered.enabled, "later tier's sandbox.enabled wins");

        // (7) Empty / malformed tiers are skipped without panicking.
        let robust = sandbox_auto_allow_from_settings_tiers(
            &["", "not json", r#"{ "sandbox": { "enabled": true } }"#],
            std::path::Path::new("/tmp"),
        );
        assert!(robust.enabled);
    }

    #[test]
    fn cron_scheduler_enabled_honors_disable_cron_env() {
        use super::cron_scheduler_enabled;
        // Unset ⇒ enabled (matches the GrowthBook fleet flag's `true` default).
        assert!(cron_scheduler_enabled(None));
        // Truthy CLAUDE_CODE_DISABLE_CRON ⇒ disabled (the local kill-switch).
        assert!(!cron_scheduler_enabled(Some("1")));
        assert!(!cron_scheduler_enabled(Some("true")));
        assert!(!cron_scheduler_enabled(Some("on")));
        // Falsy / empty / other ⇒ still enabled (isEnvTruthy semantics).
        assert!(cron_scheduler_enabled(Some("0")));
        assert!(cron_scheduler_enabled(Some("false")));
        assert!(cron_scheduler_enabled(Some("")));
    }

    #[test]
    fn should_enforce_permissions_default_on_for_cli_off_for_transport() {
        use super::should_enforce_permissions;
        use permission::PermissionMode;

        // Unset env: default-ON for the CLI/desktop NoOp inner; OFF for transport
        // (AdapterPermissionGate) so the bridge-server's remote-driven gate is
        // unchanged.
        assert!(should_enforce_permissions(None, true, PermissionMode::Default));
        assert!(!should_enforce_permissions(None, false, PermissionMode::Default));

        // An explicit env value wins for BOTH inners.
        assert!(should_enforce_permissions(Some("1"), false, PermissionMode::Default));
        assert!(should_enforce_permissions(Some("on"), false, PermissionMode::Default));
        for falsey in ["", "0", "off", "false", "no", "  OFF  "] {
            assert!(
                !should_enforce_permissions(Some(falsey), true, PermissionMode::Default),
                "{falsey:?} must disable enforcement"
            );
        }

        // BypassPermissions (--dangerously-skip-permissions) ⇒ never enforce.
        assert!(!should_enforce_permissions(None, true, PermissionMode::BypassPermissions));
        assert!(!should_enforce_permissions(
            Some("1"),
            true,
            PermissionMode::BypassPermissions
        ));
    }

    #[test]
    fn sandbox_runtime_config_from_settings_tiers_is_opt_in() {
        use super::sandbox_runtime_config_from_settings_tiers;
        let ctx = sandbox::policy_convert::SandboxConvertContext::default();

        let dir = std::path::Path::new("/tmp");
        // Default (no `sandbox` subsection) → disabled (claude-code opt-in posture).
        assert!(!sandbox_runtime_config_from_settings_tiers(&[], dir, &ctx).enabled);
        assert!(
            !sandbox_runtime_config_from_settings_tiers(&[r#"{ "permissions": {} }"#], dir, &ctx)
                .enabled
        );
        // Explicit enable.
        assert!(
            sandbox_runtime_config_from_settings_tiers(
                &[r#"{ "sandbox": { "enabled": true } }"#],
                dir,
                &ctx
            )
            .enabled
        );
        // Tier precedence: a later tier overrides an earlier one (last write wins).
        assert!(
            !sandbox_runtime_config_from_settings_tiers(
                &[
                    r#"{ "sandbox": { "enabled": true } }"#,
                    r#"{ "sandbox": { "enabled": false } }"#,
                ],
                dir,
                &ctx
            )
            .enabled
        );
        // Malformed / empty tiers are skipped without panicking.
        assert!(
            sandbox_runtime_config_from_settings_tiers(
                &["", "not json", r#"{ "sandbox": { "enabled": true } }"#],
                dir,
                &ctx
            )
            .enabled
        );
    }

    // ── IMPL-R3: managed (policySettings) tier in the sandbox derivation ──────
    //
    // claude-code's `getInitialSettings()`/`loadSettingsFromDisk()` always folds
    // `policySettings` (managed) at the HIGHEST priority (SETTING_SOURCES:
    // …→localSettings→flagSettings→policySettings, "later sources override
    // earlier"). The desktop composition root appends
    // `managed_settings_raw_tiers()` AFTER the user/project/local file tiers so a
    // managed `sandbox.*` wins. These tests point `LINGXI_MANAGED_DIR` at a
    // tempdir (the real managed path is an absolute, unwritable OS path) and
    // assert the loader + fold honor managed precedence.
    //
    // `LINGXI_MANAGED_DIR` is process-global; serialize so a managed dir one test
    // sets can't leak into another running in parallel. `#[tokio::test]` defaults
    // to a current-thread runtime, so holding the (non-Send) `MutexGuard` across
    // the `.await`s here is fine.
    static MANAGED_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// (1) No user/project/local `sandbox` block; managed `managed-settings.json`
    /// `{"sandbox":{"enabled":true}}` → managed alone enables.
    #[tokio::test]
    async fn managed_sandbox_enabled_overrides_absent_user_setting() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"sandbox":{"enabled":true}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let managed = super::settings_watch::managed_settings_raw_tiers().await;
        // No user/project/local tiers (none present); managed is the only tier.
        let refs: Vec<&str> = managed.iter().map(String::as_str).collect();
        let ctx = sandbox::policy_convert::SandboxConvertContext::default();
        let cfg = super::sandbox_runtime_config_from_settings_tiers(
            &refs,
            std::path::Path::new("/tmp"),
            &ctx,
        );
        assert!(cfg.enabled, "managed sandbox.enabled:true alone must enable");

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (2) User tier `{"sandbox":{"enabled":false}}`, managed
    /// `{"sandbox":{"enabled":true}}` → policy (highest priority) wins.
    #[tokio::test]
    async fn managed_sandbox_enabled_overrides_user_disabled() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"sandbox":{"enabled":true}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        // user tier (disabled) first, then managed appended LAST (highest).
        let mut tiers = vec![r#"{"sandbox":{"enabled":false}}"#.to_string()];
        tiers.extend(super::settings_watch::managed_settings_raw_tiers().await);
        let refs: Vec<&str> = tiers.iter().map(String::as_str).collect();
        let ctx = sandbox::policy_convert::SandboxConvertContext::default();
        let cfg = super::sandbox_runtime_config_from_settings_tiers(
            &refs,
            std::path::Path::new("/tmp"),
            &ctx,
        );
        assert!(
            cfg.enabled,
            "policySettings is highest priority and must override a user-disabled sandbox"
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (3) Managed `{"sandbox":{"enabled":true,"failIfUnavailable":true}}` yields
    /// `enabled && fail_if_unavailable` from the merged config — the
    /// `sandbox_required` predicate that drives the `BuildError::SandboxUnavailable`
    /// hard-reject path at the `build()` call site (mirrors :2647-2655). We assert
    /// the merged-config predicate plus `unavailable_reason_for` returning `Some`
    /// under a forced-unavailable platform (the same inputs `build()` feeds), per
    /// spec §7.3 (a full `build()` is too heavy / host-dependent here).
    #[tokio::test]
    async fn managed_fail_if_unavailable_triggers_hard_reject() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"sandbox":{"enabled":true,"failIfUnavailable":true}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let tiers = super::settings_watch::managed_settings_raw_tiers().await;
        let refs: Vec<&str> = tiers.iter().map(String::as_str).collect();
        let ctx = sandbox::policy_convert::SandboxConvertContext::default();
        let cfg = super::sandbox_runtime_config_from_settings_tiers(
            &refs,
            std::path::Path::new("/tmp"),
            &ctx,
        );
        // The `sandbox_required` predicate (lib.rs:2647) = enabled && fail_if_unavailable.
        assert!(
            cfg.enabled && cfg.fail_if_unavailable,
            "managed failIfUnavailable:true must produce a sandbox_required config"
        );
        // Forced-unavailable: an enabled sandbox NOT in the enabled-platform list
        // is unavailable (mirrors the `unavailable_reason_for` inputs `build()`
        // feeds), so the `BuildError::SandboxUnavailable` branch would be taken.
        assert!(
            platform_posix::PosixSandbox::unavailable_reason_for(cfg.enabled, false).is_some(),
            "an enabled-but-unavailable sandbox must yield Some(reason) → hard reject"
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (4) Managed `excludedCommands` flow into the sandbox-auto-allow fold: an
    /// excluded command is NOT auto-allowed, a normal one still is.
    #[tokio::test]
    async fn managed_excluded_commands_flow_into_auto_allow() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"sandbox":{"enabled":true,"excludedCommands":["bazel:*"]}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let tiers = super::settings_watch::managed_settings_raw_tiers().await;
        let refs: Vec<&str> = tiers.iter().map(String::as_str).collect();
        let auto_allow = super::sandbox_auto_allow_from_settings_tiers(
            &refs,
            std::path::Path::new("/tmp"),
        );
        assert!(auto_allow.enabled, "managed enabled must flow through");
        assert!(
            !auto_allow.auto_allows("bazel build"),
            "managed excludedCommands must exclude `bazel build` from auto-allow"
        );
        assert!(
            auto_allow.auto_allows("echo hi"),
            "a non-excluded command stays auto-allowed"
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (5) `managed-settings.json` `{"sandbox":{"enabled":false}}` +
    /// `managed-settings.d/10-org.json` `{"sandbox":{"enabled":true}}` → the
    /// drop-in (sorted-alphabetical-last) wins, exercising
    /// `managed_settings_raw_tiers` ordering.
    #[tokio::test]
    async fn managed_drop_in_overrides_base_managed_file() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"sandbox":{"enabled":false}}"#,
        )
        .expect("write base");
        let drop_in = tmp.path().join("managed-settings.d");
        std::fs::create_dir_all(&drop_in).expect("mkdir drop-in");
        std::fs::write(
            drop_in.join("10-org.json"),
            r#"{"sandbox":{"enabled":true}}"#,
        )
        .expect("write drop-in");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let tiers = super::settings_watch::managed_settings_raw_tiers().await;
        // base then drop-in (the loader's ascending order).
        assert_eq!(tiers.len(), 2, "base + one drop-in");
        let refs: Vec<&str> = tiers.iter().map(String::as_str).collect();
        let ctx = sandbox::policy_convert::SandboxConvertContext::default();
        let cfg = super::sandbox_runtime_config_from_settings_tiers(
            &refs,
            std::path::Path::new("/tmp"),
            &ctx,
        );
        assert!(
            cfg.enabled,
            "the drop-in (loaded last) must override the base managed file"
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (6) `managed-settings.d/.hidden.json` and `managed-settings.d/README.md`
    /// are ignored; only `*.json` non-dotfiles are read (mirrors the watcher's
    /// `classify` behavior).
    #[tokio::test]
    async fn managed_settings_raw_tiers_skips_dotfiles_and_nonjson() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        // No base managed-settings.json (absent → skipped).
        let drop_in = tmp.path().join("managed-settings.d");
        std::fs::create_dir_all(&drop_in).expect("mkdir drop-in");
        std::fs::write(drop_in.join(".hidden.json"), r#"{"sandbox":{"enabled":true}}"#)
            .expect("write dotfile");
        std::fs::write(drop_in.join("README.md"), "not json").expect("write md");
        std::fs::write(drop_in.join("20-real.json"), r#"{"sandbox":{"enabled":true}}"#)
            .expect("write real");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let tiers = super::settings_watch::managed_settings_raw_tiers().await;
        assert_eq!(
            tiers.len(),
            1,
            "only the single `*.json` non-dotfile drop-in is read (.hidden.json + README.md skipped)"
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (7) Override points at a non-existent dir → `managed_settings_raw_tiers()`
    /// is empty, so both helpers behave exactly as the pre-fix 3-tier path
    /// (regression guard for the common case = byte-identical boot).
    #[tokio::test]
    async fn absent_managed_dir_is_noop() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, &missing);

        let tiers = super::settings_watch::managed_settings_raw_tiers().await;
        assert!(tiers.is_empty(), "absent managed dir → no tiers");

        // With no managed tier and no other tiers, the sandbox stays disabled
        // (byte-identical to the pre-fix opt-in posture).
        let ctx = sandbox::policy_convert::SandboxConvertContext::default();
        let cfg = super::sandbox_runtime_config_from_settings_tiers(
            &[],
            std::path::Path::new("/tmp"),
            &ctx,
        );
        assert!(!cfg.enabled, "no tiers → sandbox disabled (opt-in default)");

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    // ── 3c-T2: providers/routing settings → ClientConfig (e2e-flavored) ───

    /// 3c-T2: a `DesktopConfig` with a groq-style openai-compat provider profile
    /// and a routing alias produces a `build()` that succeeds, and the same
    /// `apply_settings_providers` path exposes the custom model in
    /// `available_models()` + the alias resolves via the registry.
    ///
    /// This exercises the full settings → `apply_settings_providers` →
    /// `DefaultLlmClient::from_config` → registry path without any network call.
    /// The `build()` call is the composition-root assertion; the model/alias
    /// assertions use `apply_settings_providers` directly (same code path, but
    /// callable without digging into the orchestrator internals).
    #[tokio::test]
    async fn custom_openai_profile_available_and_alias_resolves() {
        let (_tmp, mut cfg) = test_config(true);

        // Inject a groq-style provider profile + an alias.
        cfg.provider_profiles = Some({
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "groq".to_string(),
                serde_json::json!({
                    "type": "openai",
                    "baseUrl": "https://api.groq.com/openai/v1",
                    "apiKeyEnv": "GROQ_API_KEY",
                    "models": [{ "id": "llama-3.3-70b-versatile" }]
                }),
            );
            m
        });
        cfg.routing = Some(serde_json::json!({
            "aliases": { "llama": "groq/llama-3.3-70b-versatile" }
        }));

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        // The composition-root assertion: build() must not fail when
        // provider_profiles is set.
        let rt = build(cfg, output, perm_sink).await.expect("build() failed with custom provider profile");
        let _ = rt;

        // Model/alias assertions via apply_settings_providers directly (same
        // code path build() uses; no need to crack open orchestrator internals).
        let mut llm_cfg =
            platform_common::builtin_anthropic_config("https://api.anthropic.com", false);
        let providers = {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "groq".to_string(),
                serde_json::json!({
                    "type": "openai",
                    "baseUrl": "https://api.groq.com/openai/v1",
                    "apiKeyEnv": "GROQ_API_KEY",
                    "models": [{ "id": "llama-3.3-70b-versatile" }]
                }),
            );
            m
        };
        let routing = serde_json::json!({
            "aliases": { "llama": "groq/llama-3.3-70b-versatile" }
        });
        platform_common::apply_settings_providers(&mut llm_cfg, &providers, Some(&routing))
            .expect("apply_settings_providers must succeed");

        let client =
            llm_client::DefaultLlmClient::from_config(llm_cfg).expect("config must be valid");

        // (1) available_models() includes the custom groq model.
        let available: Vec<String> = client
            .available_models()
            .into_iter()
            .map(|m| m.display_model)
            .collect();
        assert!(
            available.contains(&"llama-3.3-70b-versatile".to_string()),
            "custom model must be in available_models; got: {available:?}"
        );

        // (2) alias "llama" resolves to the groq model.
        let groq_with_alias = client
            .available_models()
            .into_iter()
            .find(|m| m.aliases.contains(&"llama".to_string()));
        assert!(
            groq_with_alias.is_some(),
            "alias 'llama' must appear on the groq model; available_models: {available:?}"
        );
        assert_eq!(
            groq_with_alias.unwrap().display_model,
            "llama-3.3-70b-versatile"
        );
    }

    /// 3c final-review fix: a ROUTING-ONLY settings file (aliases onto builtin
    /// models, no custom `providers` key) must still be applied — the apply
    /// gate runs when EITHER key is present.
    #[tokio::test]
    async fn routing_only_settings_alias_applies_to_builtin_model() {
        let (_tmp, mut cfg) = test_config(true);
        cfg.provider_profiles = None;
        cfg.routing = Some(serde_json::json!({
            "aliases": { "best": "anthropic/claude-opus-4-7" }
        }));

        // Composition-root assertion: build() succeeds with routing-only settings.
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed with routing-only settings");
        let _ = rt;

        // Same code path, directly: the alias lands on the BUILTIN model.
        let mut llm_cfg =
            platform_common::builtin_anthropic_config("https://api.anthropic.com", false);
        let empty = std::collections::BTreeMap::new();
        let routing = serde_json::json!({
            "aliases": { "best": "anthropic/claude-opus-4-7" }
        });
        platform_common::apply_settings_providers(&mut llm_cfg, &empty, Some(&routing))
            .expect("routing-only apply must succeed");
        let client =
            llm_client::DefaultLlmClient::from_config(llm_cfg).expect("config must be valid");
        let aliased = client
            .available_models()
            .into_iter()
            .find(|m| m.aliases.contains(&"best".to_string()));
        assert_eq!(
            aliased.map(|m| m.display_model).as_deref(),
            Some("claude-opus-4-7"),
            "routing-only alias must land on the builtin model"
        );
    }

    // ── Task 2: per-profile pricing override end-to-end ───────────────────────

    /// Full pipeline test: a settings-declared custom profile with a `"pricing"`
    /// block flows through `apply_settings_providers` → extract overrides →
    /// `llm_catalog_from_cost` → `add_override` → `CostEstimator`.
    ///
    /// The estimator must yield the user-declared override price (not the
    /// built-in catalog price) for the custom model.  Asserted figure: 1M input
    /// tokens × $2.50/M = $2.50 exactly.
    #[test]
    fn pricing_override_end_to_end_estimator_yields_overridden_cost() {
        use llm_client::{CostEstimator, PricingModelRef, PricingPolicy, TokenUsage, Usage};
        use orchestrator::cost_wiring::llm_catalog_from_cost;

        // Build a ClientConfig with a custom "myprovider" profile that declares a
        // pricing override for "my-model" at $2.50 input / $10.0 output.
        let mut cfg_obj =
            platform_common::builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_str(r#"{
                "myprovider": {
                    "type": "openai",
                    "baseUrl": "https://api.example.com/v1",
                    "apiKeyEnv": "MY_API_KEY",
                    "models": [{ "id": "my-model" }],
                    "pricing": {
                        "my-model": { "inputPerMtok": 2.50, "outputPerMtok": 10.0 }
                    }
                }
            }"#)
            .unwrap();

        platform_common::apply_settings_providers(&mut cfg_obj, &providers, None)
            .expect("apply_settings_providers must succeed");

        // Extract pricing overrides (mirrors the build() block: display_model →
        // billing_model resolution inside each profile).
        let pricing_overrides: Vec<(llm_client::ProviderId, String, llm_client::TokenPricing)> =
            cfg_obj.providers.iter().flat_map(|p| {
                p.pricing.overrides.iter().filter_map(|(model_id, tp)| {
                    p.models.iter()
                        .find(|m| m.display_model == *model_id)
                        .map(|m| (p.provider_id.clone(), m.billing_model.clone(), *tp))
                })
            }).collect();

        assert_eq!(pricing_overrides.len(), 1, "one override expected");
        let (ref prov_id, ref billing_model, _) = pricing_overrides[0];
        assert_eq!(billing_model, "my-model");

        // Build the catalog the same way build() does.
        let cost_cat = cost::pricing::PricingCatalog::builtin_reference();
        let mut llm_cat = llm_catalog_from_cost(&cost_cat);
        for (provider_id, bm, tp) in &pricing_overrides {
            llm_cat.add_override(provider_id.clone(), bm.clone(), *tp);
        }
        let estimator = CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated);

        // Construct the PricingModelRef: the estimator key is (provider_id, billing_model).
        let pricing_ref = PricingModelRef {
            pricing_provider_id: prov_id.clone(),
            billing_model: billing_model.clone(),
            request_model: "my-model".to_string(),
            display_model: "my-model".to_string(),
        };
        let usage = Usage {
            billable_tokens: TokenUsage { input: 1_000_000, ..Default::default() },
            ..Default::default()
        };
        let estimate = estimator.estimate(pricing_ref, &usage).expect("must yield cost for overridden model");

        // Assert exact figure: 1M input × $2.50/M = $2.50
        let input_cost = estimate.input_cost_usd.expect("input_cost_usd must be Some");
        assert!(
            (input_cost - 2.50).abs() < 1e-9,
            "overridden input cost must be $2.50 (1M tokens × $2.50/M), got ${input_cost}"
        );
        assert_eq!(
            estimate.pricing_source.as_deref(),
            Some("override"),
            "pricing_source must reflect that an override was used"
        );
        // Total: 1M input × $2.50 + 0 output = $2.50 exactly.
        let total = estimate.total_cost_usd.expect("total_cost_usd must be Some");
        assert!(
            (total - 2.50).abs() < 1e-9,
            "total cost must be $2.50, got ${total}"
        );
    }

    // ── subscription_snapshot_from (Task 4: background profile+roles fetch) ──

    #[test]
    fn subscription_snapshot_maps_profile_and_roles() {
        let profile = anthropic_oauth::OAuthProfileResponse {
            organization: Some(anthropic_oauth::OAuthOrganization {
                organization_type: Some("claude_team".to_string()),
                rate_limit_tier: Some("default_claude_max_5x".to_string()),
                billing_type: Some("stripe_subscription".to_string()),
                has_extra_usage_enabled: Some(true),
                ..Default::default()
            }),
            account: None,
        };
        let roles = anthropic_oauth::UserRolesResponse {
            organization_role: Some("admin".to_string()),
            ..Default::default()
        };
        let snap = super::subscription_snapshot_from(true, Some(&profile), Some(&roles));
        assert!(snap.is_subscriber);
        assert_eq!(snap.subscription_type.as_deref(), Some("team"));
        assert_eq!(snap.rate_limit_tier.as_deref(), Some("default_claude_max_5x"));
        assert_eq!(snap.billing_type.as_deref(), Some("stripe_subscription"));
        assert!(snap.has_extra_usage_enabled);
        assert_eq!(snap.organization_role.as_deref(), Some("admin"));
        // Team + admin org role ⇒ billing access (the predicate the TUI gates on).
        assert!(snap.has_claude_ai_billing_access());
    }

    #[test]
    fn subscription_snapshot_absent_profile_is_conservative() {
        let snap = super::subscription_snapshot_from(true, None, None);
        assert!(snap.is_subscriber);
        assert_eq!(snap.subscription_type, None);
        assert_eq!(snap.rate_limit_tier, None);
        assert_eq!(snap.billing_type, None);
        assert!(!snap.has_extra_usage_enabled);
        assert_eq!(snap.organization_role, None);
        assert!(!snap.has_claude_ai_billing_access());
    }

    #[test]
    fn subscription_snapshot_free_or_unknown_tier_maps_to_none() {
        // `OAuthProfileResponse::subscription_type()` (profile.rs:104-117) only
        // ever returns Max/Pro/Enterprise/Team — an unrecognized
        // `organization_type` already resolves to `None` at that layer, so the
        // `Free | Unknown → None` arm of `subscription_snapshot_from`'s match
        // is unreachable from real profile parsing (purely defensive). This
        // test pins the observable contract: a non-paid/unknown org type folds
        // to `subscription_type: None` in the snapshot.
        let profile = anthropic_oauth::OAuthProfileResponse {
            organization: Some(anthropic_oauth::OAuthOrganization {
                organization_type: Some("claude_free".to_string()),
                ..Default::default()
            }),
            account: None,
        };
        let snap = super::subscription_snapshot_from(true, Some(&profile), None);
        assert_eq!(snap.subscription_type, None);
        assert!(!snap.is_team_or_enterprise());
    }

    #[test]
    fn subscription_snapshot_explicit_extra_usage_false_stays_false() {
        // Pins the `== Some(true)` flattening: a profile org that EXPLICITLY
        // reports `has_extra_usage_enabled: Some(false)` must fold to `false`
        // in the snapshot (same as the absent-`None` case, distinct from
        // `Some(true)`).
        let profile = anthropic_oauth::OAuthProfileResponse {
            organization: Some(anthropic_oauth::OAuthOrganization {
                has_extra_usage_enabled: Some(false),
                ..Default::default()
            }),
            account: None,
        };
        let snap = super::subscription_snapshot_from(true, Some(&profile), None);
        assert!(!snap.has_extra_usage_enabled);
    }

    // ── TPM-C (Task 5): default_model profile/model parsing ──────────────────

    /// Verifies the listings-building + `parse_model_ref` logic used at
    /// composition-root time: a qualified `profile/model` default_model splits
    /// into the bare id (written to `orch_cfg.model`) and `Some(profile)` (used
    /// to seed `switch_model`), while a bare id passes through unchanged with
    /// `None` profile (no-op seed path).
    #[test]
    fn default_model_parse_qualified_and_bare() {
        // Construct the same listing shape the composition root builds from
        // `assembled.client_config.providers`.
        let listings = vec![
            traits::ModelListing {
                display_model: "gpt-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                provider_id: "openai".to_string(),
                provider_label: "OpenAI".to_string(),
            },
            traits::ModelListing {
                display_model: "gpt-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                provider_id: "github-copilot".to_string(),
                provider_label: "GitHub Copilot".to_string(),
            },
            traits::ModelListing {
                display_model: "claude-sonnet-4-6".to_string(),
                request_model: "claude-sonnet-4-6".to_string(),
                provider_id: "anthropic".to_string(),
                provider_label: "Anthropic".to_string(),
            },
        ];

        // Qualified: "openai/gpt-4o" → bare id "gpt-4o" + profile "openai"
        let (id, profile) = traits::parse_model_ref("openai/gpt-4o", &listings);
        assert_eq!(id, "gpt-4o", "qualified ref must strip the profile prefix");
        assert_eq!(
            profile.as_deref(),
            Some("openai"),
            "qualified ref must extract the profile"
        );

        // Bare: "claude-sonnet-4-6" → same id, no profile (no-op seed path)
        let (id2, profile2) = traits::parse_model_ref("claude-sonnet-4-6", &listings);
        assert_eq!(id2, "claude-sonnet-4-6", "bare model id must pass through");
        assert!(profile2.is_none(), "bare model must yield None profile");

        // Shared id with two providers and explicit profile qualifier
        let (id3, profile3) = traits::parse_model_ref("github-copilot/gpt-4o", &listings);
        assert_eq!(id3, "gpt-4o");
        assert_eq!(profile3.as_deref(), Some("github-copilot"));
    }

    // ── T16: session-scoped task-output dir ──────────────────────────────────

    #[test]
    fn sanitize_path_component_replaces_non_alphanumeric() {
        // Port of claude-code `sanitizePath` — every non-alphanumeric char → '-'.
        assert_eq!(
            super::sanitize_path_component("/Users/me/my-project"),
            "-Users-me-my-project"
        );
        assert_eq!(super::sanitize_path_component("ok09AZ"), "ok09AZ");
        assert_eq!(super::sanitize_path_component("a b:c/d"), "a-b-c-d");
    }

    #[test]
    fn session_task_output_dir_is_session_scoped_under_project_temp() {
        // T16: the task-output dir must be `<projectTempDir>/<sessionId>/tasks`
        // (claude-code `getTaskOutputDir`), NOT an in-repo `.claude/...` path.
        // Pin CLAUDE_CODE_TMPDIR so the base is deterministic for the assert.
        // (Single-threaded test sets + clears the env var around the call.)
        let prev = std::env::var_os("CLAUDE_CODE_TMPDIR");
        std::env::set_var("CLAUDE_CODE_TMPDIR", "/pin-tmp");

        let cwd = std::path::Path::new("/Users/me/proj");
        let dir = super::session_task_output_dir(cwd, "sess:abc-123");

        // Restore the env var before asserting (so a failure doesn't leak it).
        match prev {
            Some(v) => std::env::set_var("CLAUDE_CODE_TMPDIR", v),
            None => std::env::remove_var("CLAUDE_CODE_TMPDIR"),
        }

        // The cwd is sanitized (`-Users-me-proj`); the session id is used
        // verbatim as its own path segment (matching claude `join(..., sessionId,
        // 'tasks')`, where the session id is a fixed-shape token).
        let expected = std::path::Path::new("/pin-tmp")
            .join(super::claude_temp_dir_name()) // claude-<uid>
            .join("-Users-me-proj")
            .join("sess:abc-123")
            .join("tasks");
        assert_eq!(dir, expected);

        // It must NOT live inside the working tree (no `.claude` segment, not a
        // child of cwd) — the whole point of T16.
        assert!(!dir.starts_with(cwd), "dir must not be under the repo cwd");
        assert!(
            !dir.to_string_lossy().contains("/.claude/"),
            "dir must not be the old in-repo .claude/tasks-output path"
        );
        assert!(dir.ends_with("tasks"));
    }
}
