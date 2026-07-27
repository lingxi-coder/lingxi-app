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
pub mod agent_restore;
pub mod fork_resume;
mod connect;
pub mod auto_mode_propose;
pub mod file_changed_watch;
pub mod settings_watch;
mod skill_loader;

use client_adapter::{AdapterPermissionGate, PermissionRequestSink};
use command_api::model::BuiltinCommandHandler;
use command_api::{
    parse_slash_command, CommandRegistry, CommandResult, ParsedSlashCommand,
    RegistrySlashDispatcher,
};
use command_core::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2,
    register_core_batch_4, register_core_batch_5,
};
use llm_client::oauth::anthropic::client::ClaudeAiOAuthClient;
use llm_client::oauth::anthropic::config::ClaudeAiOAuthConfig;
use llm_client::oauth::anthropic::handle::OAuthHandle;
use llm_client::oauth::anthropic::{OAuthCredentialProvider, RefreshDriver};
use llm_client::oauth::openai as openai_oauth;
use llm_client::LlmTransportBridge;
use llm_client::{DefaultLlmClient, Transport};
use orchestrator::model::user_agent::UserAgentEnv;
use orchestrator::provider_adapter::SubscriberState;
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
use tool_api::AnthropicRequestBuilder;
use tool_api::SessionCwd;
use tool_api::{BuiltinToolContext, ToolRegistry};
use traits::{AuthHandle, McpTransport, OrchestratorHandle, OutputStream};

struct DesktopWebSearchConfigProvider {
    lingxi_home: std::path::PathBuf,
    credentials: Arc<secret::CredentialManager>,
}

#[async_trait::async_trait]
impl traits::WebSearchConfigProvider for DesktopWebSearchConfigProvider {
    async fn load_web_search_config(&self) -> traits::WebSearchRuntimeConfig {
        let settings_path = self.lingxi_home.join("settings.json");
        let parsed = std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|v| tool_web::web_search_config::WebSearchConfig::from_settings_json(&v))
            .unwrap_or_default();
        let tavily_key = self
            .credentials
            .get_provider_key("web:tavily")
            .await
            .ok()
            .flatten()
            .map(|s| s.expose_secret().clone());
        let brave_key = self
            .credentials
            .get_provider_key("web:brave")
            .await
            .ok()
            .flatten()
            .map(|s| s.expose_secret().clone());
        traits::WebSearchRuntimeConfig {
            provider: Some(parsed.provider.as_str().to_string()),
            searxng_url: parsed.searxng_url,
            tavily_key,
            brave_key,
        }
    }
}

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
/// - `BypassPermissions` mode (`--dangerously-skip-permissions`) STILL enforces:
///   claude-code never drops the permission layer — `checkPermissions` runs the
///   deny/ask rule walks first and bypass short-circuits to allow AFTER them
///   (`permissions.ts` step order: 1a deny … 2a bypass). The wrap is what
///   provides that auto-allow; without it the BASE gate decides every call, and
///   an interactive base (`TuiPermissionGate`) prompts on every tool use —
///   the exact opposite of bypass.
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
    let _ = mode;
    // `use_noop_inner` no longer gates the default: claude-code enforces ONE core
    // policy on every host, so transport hosts (the bridge-server's
    // AdapterPermissionGate) ALSO wrap with the local PolicyPermissionGate by
    // default — the adapter gate becomes the Ask-delegation transport (an
    // unresolved mutating Ask still forwards to the remote client), but local
    // deny/allow rules + defaultMode now bind regardless of what the client
    // replicates. The explicit env escape hatch + BypassPermissions still opt out.
    let _ = use_noop_inner;
    match env_value {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "off" | "false" | "no"
        ),
        None => true,
    }
}

/// Everything the boot permission-policy construction folds out of the
/// settings tiers, produced by [`load_boot_permission_tiers`].
struct BootPermissionTiers {
    /// Permission rules accumulated from every tier (bucketed by source at
    /// `PermissionPolicy::from_rules` time; `authorize` walks them by priority).
    rules: Vec<permission::PermissionRule>,
    /// Highest-priority `permissions.defaultMode` (tiers are read in ascending
    /// priority, so the last write — the managed tier — wins).
    mode: permission::PermissionMode,
    /// Sticky `disableBypassPermissionsMode: "disable"` killswitch — true when
    /// ANY tier (managed included) disables `BypassPermissions` mode.
    bypass_disabled: bool,
    /// Sticky `disableAutoMode: "disable"` killswitch (claude-code `Bpa()`) —
    /// true when ANY tier disables auto mode at either settings position. Set on
    /// the boot policy and applied at mode-load (auto → default downgrade).
    auto_mode_disabled: bool,
    /// Sticky `autoMode.classifyAllShell` escalation (claude-code `QOi()`) — true
    /// when ANY tier sets `autoMode.classifyAllShell === true`. Set on the boot
    /// policy so every `Bash`/`PowerShell` allow rule is suspended in auto mode.
    classify_all_shell: bool,
    /// Union of every tier's `permissions.additionalDirectories` (raw paths;
    /// `authorize` resolves them against the policy roots via `expand_path`).
    additional_working_dirs: Vec<std::path::PathBuf>,
    /// Raw tier texts in ASCENDING priority INCLUDING the managed tier(s) —
    /// feeds the sandbox-auto-allow derivation (last write wins, so a managed
    /// `sandbox.*` overrides user/project/local).
    raw_tiers: Vec<String>,
    /// Enterprise gate that disables non-managed permission persistence.
    allow_managed_permission_rules_only: bool,
}

/// Read the boot permission-settings tiers in ASCENDING priority — user →
/// project → local → managed (policySettings) — and fold them into rules +
/// scalars for the boot `PermissionPolicy` (parity 2.1.207 P1-10).
///
/// Tier semantics (claude-code `SETTING_SOURCES`: `userSettings→projectSettings
/// →localSettings→flagSettings→policySettings`, later overrides earlier;
/// `flagSettings` has no boot analog here — spec §4e):
/// - settings.local.json is read after project so an `AllowAlways` persisted
///   there is honored on the next enforced boot; rules from every tier
///   ACCUMULATE (deny-wins is behavior-first in `authorize`).
/// - `--setting-sources` scope `(include_user, include_project)` gates the
///   user tier and the project+local tiers respectively — but NOT the managed
///   tier: claude-code's `Xv()` unconditionally re-adds `"policySettings"` to
///   the allowed-source set, so managed rules can NEVER be excluded.
/// - Managed tiers (`managed-settings.json` + `managed-settings.d/*.json`,
///   already ascending from `managed_settings_raw_tiers`) parse with
///   `PermissionRuleSource::PolicySettings` (`RKt()→Fwt("policySettings")`),
///   so enterprise deny/ask/allow rules bind on the boot policy and decisions
///   cite "enterprise managed settings". Managed `defaultMode` /
///   `disableBypassPermissionsMode` / `additionalDirectories` fold like any
///   other tier (read LAST → managed scalars win).
/// - `allowManagedPermissionRulesOnly` lockdown (claude-code `$wt()`): when ANY
///   managed tier sets the top-level flag true, only `PolicySettings`-sourced
///   rules are retained — "User, project, local, and CLI argument permission
///   rules are ignored." (Scalar folds are NOT affected; the schema scopes the
///   lockdown to permission RULES.)
async fn load_boot_permission_tiers(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
    setting_source_scope: (bool, bool),
) -> BootPermissionTiers {
    let mut rules = Vec::new();
    let mut mode = permission::PermissionMode::Default;
    let mut bypass_disabled = false;
    let mut auto_mode_disabled = false;
    let mut classify_all_shell = false;
    let mut additional_working_dirs: Vec<std::path::PathBuf> = Vec::new();
    // Retain each tier's raw text (in ascending priority) so the
    // sandbox-auto-allow config can be derived from the SAME settings.
    let mut raw_tiers: Vec<String> = Vec::new();
    let (incl_user_settings, incl_project_settings) = setting_source_scope;
    for (path, source, included) in [
        (
            lingxi_home.join("settings.json"),
            permission::PermissionRuleSource::UserSettings,
            incl_user_settings,
        ),
        (
            cwd.join(branding::DOT_DIR).join("settings.json"),
            permission::PermissionRuleSource::ProjectSettings,
            incl_project_settings,
        ),
        (
            cwd.join(branding::DOT_DIR).join("settings.local.json"),
            permission::PermissionRuleSource::LocalSettings,
            incl_project_settings,
        ),
    ] {
        if !included {
            continue;
        }
        if let Ok(raw) = tokio::fs::read_to_string(&path).await {
            match permission::permission_rules_from_settings_json(&raw, source) {
                Ok(mut r) => {
                    // parity 2.1.210: warn when a `Write`/`NotebookEdit`/
                    // `MultiEdit`/`Glob` rule carries a path that no file-permission
                    // matcher will ever see (those tools share `Edit`/`Read` rules).
                    let display = path.display().to_string();
                    for rule in &r {
                        if let Some(line) =
                            permission::permission_rule_startup_warning(rule, &display)
                        {
                            tracing::warn!("{line}");
                        }
                    }
                    rules.append(&mut r);
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "skipping malformed settings permissions"
                ),
            }
            if let Some(m) = permission::default_mode_from_settings_json(&raw) {
                // MODE-SETTINGS-AUTO-TRUST-01: a `defaultMode: "auto"` is only
                // honored from a TRUSTED tier (policy/user/flag). The
                // repo-controllable `projectSettings`/`localSettings` tiers may
                // set the five external modes but NOT auto — a committed
                // `.lingxi/settings.json` must not put the session into
                // classifier-driven auto-accept mode. (Non-auto modes fold as
                // before, last-tier-wins.)
                if m != permission::PermissionMode::Auto
                    || permission::loader::auto_mode_grantable_by_source(source)
                {
                    mode = m; // later tiers read last → their defaultMode wins
                } else {
                    tracing::warn!(
                        source = ?source,
                        "settings defaultMode \"auto\" ignored — only policy/user/flag settings may grant auto mode (projectSettings and localSettings are repo-controllable)"
                    );
                }
            }
            if permission::bypass_permissions_disabled_from_settings_json(&raw) {
                bypass_disabled = true; // sticky: any tier disabling wins
            }
            if permission::auto_mode_disabled_from_settings_json(&raw) {
                auto_mode_disabled = true; // sticky: any tier disabling wins (Bpa)
            }
            if permission::classify_all_shell_from_settings_json(&raw) {
                classify_all_shell = true; // sticky: any tier enabling wins (QOi)
            }
            // (#34) Union this tier's additionalDirectories into the
            // working-dir set (claude-code merges across SETTING_SOURCES).
            additional_working_dirs
                .extend(permission::additional_directories_from_settings_json(&raw));
            raw_tiers.push(raw); // ascending priority preserved for sandbox derivation
        }
    }
    // Managed (policySettings) tier — HIGHEST priority, read LAST. Deliberately
    // NOT gated by `--setting-sources` (see the doc comment above).
    let managed_tiers = crate::settings_watch::managed_settings_raw_tiers().await;
    for raw in &managed_tiers {
        match permission::permission_rules_from_settings_json(
            raw,
            permission::PermissionRuleSource::PolicySettings,
        ) {
            Ok(mut r) => {
                // parity 2.1.210: same file-matcher warning for managed rules.
                for rule in &r {
                    if let Some(line) =
                        permission::permission_rule_startup_warning(rule, "managed policy settings")
                    {
                        tracing::warn!("{line}");
                    }
                }
                rules.append(&mut r);
            }
            Err(e) => tracing::warn!(
                error = %e,
                "skipping malformed managed settings permissions"
            ),
        }
        if let Some(m) = permission::default_mode_from_settings_json(raw) {
            mode = m; // managed read last → its defaultMode wins
        }
        if permission::bypass_permissions_disabled_from_settings_json(raw) {
            bypass_disabled = true; // managed killswitch binds (sticky)
        }
        if permission::auto_mode_disabled_from_settings_json(raw) {
            auto_mode_disabled = true; // managed auto-mode killswitch binds (sticky)
        }
        if permission::classify_all_shell_from_settings_json(raw) {
            classify_all_shell = true; // managed classifyAllShell binds (sticky, QOi)
        }
        additional_working_dirs.extend(permission::additional_directories_from_settings_json(raw));
    }
    let allow_managed_permission_rules_only = managed_tiers
        .iter()
        .any(|raw| permission::allow_managed_permission_rules_only_from_settings_json(raw));
    if allow_managed_permission_rules_only {
        rules.retain(|r| r.source == permission::PermissionRuleSource::PolicySettings);
    }
    raw_tiers.extend(managed_tiers);
    BootPermissionTiers {
        rules,
        mode,
        bypass_disabled,
        auto_mode_disabled,
        classify_all_shell,
        additional_working_dirs,
        raw_tiers,
        allow_managed_permission_rules_only,
    }
}

/// Build the MANAGED (`policySettings`) model-restriction view for the
/// `availableModels` / `enforceAvailableModels` / `modelOverrides` enforcement
/// (parity 2.1.207 H-BIN-08), mirroring claude-code's per-source
/// `getSettingsForSource("policySettings")` view (`ROn`/`sl`). The managed raw
/// tiers arrive ASCENDING (base then drop-ins); scalar/array keys take the last
/// (highest-priority) tier, `modelOverrides` unions per key. A tier that fails
/// to parse marks the whole policy source failed — `refusing cascade-trust
/// mode` (fail-closed), matching the binary `try{…}catch` around the policy
/// read. Only the MANAGED tiers are consulted: the enforce flag requires a
/// policy-OWNED allowlist, so user/project `availableModels` are deliberately
/// NOT folded in here.
fn managed_model_policy_source(
    managed_tiers: &[String],
) -> llm_client::model::allowlist::PolicySource {
    use llm_client::model::allowlist::{PolicyModelView, PolicySource};
    let mut view = PolicyModelView::default();
    for raw in managed_tiers {
        match serde_json::from_str::<engine::settings::schema::SettingsJson>(raw) {
            Ok(s) => {
                if s.available_models.is_some() {
                    view.available_models = s.available_models; // last tier wins
                }
                if s.enforce_available_models.is_some() {
                    view.enforce = s.enforce_available_models; // last tier wins
                }
                if let Some(mo) = s.model_overrides {
                    view.model_overrides
                        .get_or_insert_with(std::collections::BTreeMap::new)
                        .extend(mo); // union, later tier wins per key
                }
            }
            // A managed file that exists but does not parse ⇒ fail-closed.
            Err(_) => return PolicySource::Failed,
        }
    }
    PolicySource::Loaded(view)
}

/// The MANAGED `availableModels` allowlist + `modelOverrides` in effect for the
/// selection-restriction consumer surfaces (`/model` picker filter, subagent /
/// plan-mode model resolution — parity 2.1.207 H-BIN-08), or `(None, empty)`
/// when no policy restriction is active.
///
/// Reads the SAME managed policy tier the boot default-model constraint resolves
/// (`managed_model_policy_source` → `resolve_enforcement`); warnings are
/// suppressed here since the boot path already emits them once. A `Refused`
/// (policy failed to parse) or `Inactive` source yields `(None, empty)`, leaving
/// the consumer unrestricted — the boot default-model constraint owns the
/// fail-closed behavior for the parse-failure case.
pub async fn managed_model_allowlist() -> (
    Option<Vec<String>>,
    std::collections::BTreeMap<String, String>,
) {
    use llm_client::model::allowlist::{self, ModelEnforcement};
    let managed_tiers = crate::settings_watch::managed_settings_raw_tiers().await;
    let source = managed_model_policy_source(&managed_tiers);
    match allowlist::resolve_enforcement(&source, &mut |_| {}) {
        ModelEnforcement::Active {
            allowlist,
            overrides,
        } => (Some(allowlist), overrides),
        _ => (None, std::collections::BTreeMap::new()),
    }
}

/// The MANAGED `forceLoginOrgUUID` org pin in effect for the interactive
/// Anthropic OAuth login (parity 2.1.207 H-BIN-09). Reads the managed policy
/// tiers (the SAME `managed_settings_raw_tiers` the permission + model-allowlist
/// policies read) and folds `forceLoginOrgUUID` via
/// [`engine::settings::enterprise::fold_force_login_org_pin`] — the
/// highest-priority tier that sets it wins. `Unset` when no policy pins login
/// (the common case: login stays unrestricted). Read fresh at each login so a
/// mid-session managed-settings edit takes effect on the next sign-in.
pub async fn managed_force_login_org_pin() -> engine::settings::enterprise::ForceLoginOrgPin {
    let managed_tiers = crate::settings_watch::managed_settings_raw_tiers().await;
    engine::settings::enterprise::fold_force_login_org_pin(&managed_tiers)
}

/// Fold the MANAGED (`policySettings`) raw tiers into telemetry env overrides.
///
/// This preserves the enterprise provenance of OTEL-related keys without
/// mutating the process environment. Unknown or non-scalar values are ignored;
/// later managed tiers override earlier ones.
pub async fn managed_otel_env_overrides() -> std::collections::BTreeMap<String, String> {
    const KEYS: &[&str] = &[
        telemetry::otel::config::ENV_ENABLE_TELEMETRY,
        telemetry::otel::config::ENV_FLUSH_TIMEOUT_MS,
        telemetry::otel::config::ENV_SHUTDOWN_TIMEOUT_MS,
        telemetry::otel::config::ENV_HEADERS_HELPER_DEBOUNCE_MS,
        telemetry::otel::config::ENV_DIAG_STDERR,
        telemetry::otel::config::ENV_CONTENT_MAX_LENGTH,
        telemetry::otel::config::ENV_ATTRIBUTE_VALUE_LENGTH_LIMIT,
        telemetry::otel::config::ENV_LOGRECORD_ATTRIBUTE_VALUE_LENGTH_LIMIT,
        telemetry::otel::config::ENV_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT,
        telemetry::otel::config::ENV_METRICS_EXPORTER,
        telemetry::otel::config::ENV_LOGS_EXPORTER,
        telemetry::otel::config::ENV_TRACES_EXPORTER,
        telemetry::otel::config::ENV_OTLP_ENDPOINT,
        telemetry::otel::config::ENV_OTLP_HEADERS,
        telemetry::otel::config::ENV_OTLP_PROTOCOL,
        telemetry::otel::config::ENV_OTLP_COMPRESSION,
        telemetry::otel::config::ENV_OTLP_TIMEOUT,
        telemetry::otel::config::ENV_OTLP_INSECURE,
        telemetry::otel::config::ENV_OTLP_CERTIFICATE,
        telemetry::otel::config::ENV_OTLP_CLIENT_KEY,
        telemetry::otel::config::ENV_OTLP_CLIENT_CERTIFICATE,
        telemetry::otel::config::ENV_METRIC_EXPORT_INTERVAL,
        telemetry::otel::config::ENV_LOGS_EXPORT_INTERVAL,
        telemetry::otel::config::ENV_RESOURCE_ATTRIBUTES,
        telemetry::otel::config::ENV_SERVICE_NAME,
        telemetry::otel::config::ENV_TRACES_SAMPLER,
        telemetry::otel::config::ENV_TRACES_SAMPLER_ARG,
        "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
        "OTEL_EXPORTER_OTLP_METRICS_PROTOCOL",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        "OTEL_METRICS_INCLUDE_SESSION_ID",
        "OTEL_METRICS_INCLUDE_VERSION",
        "OTEL_METRICS_INCLUDE_ACCOUNT_UUID",
        "OTEL_METRICS_INCLUDE_ENTRYPOINT",
        "OTEL_METRICS_INCLUDE_RESOURCE_ATTRIBUTES",
        "OTEL_LOG_USER_PROMPTS",
        "OTEL_LOG_TOOL_DETAILS",
        "OTEL_LOG_TOOL_CONTENT",
        "OTEL_LOG_ASSISTANT_RESPONSES",
        "OTEL_LOG_RAW_API_BODIES",
    ];

    let mut out = std::collections::BTreeMap::new();
    let tiers = crate::settings_watch::managed_settings_raw_tiers().await;
    for raw in tiers {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw)
        else {
            continue;
        };
        for key in KEYS {
            let Some(value) = map.get(*key) else {
                continue;
            };
            let scalar = match value {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Bool(v) => Some(v.to_string()),
                serde_json::Value::Number(v) => Some(v.to_string()),
                _ => None,
            };
            if let Some(value) = scalar {
                out.insert((*key).to_string(), value);
            }
        }
    }
    out
}

/// Whether the live cron scheduler should run. Faithful to claude-code's
/// `isKairosCronEnabled` LOCAL kill-switch (`ScheduleCronTool/prompt.ts:34/38`):
/// the `LINGXI_DISABLE_CRON` env override (truthy ⇒ cron OFF) "wins over"
/// the GrowthBook fleet flag. That flag defaults to `true`, so this wired-on
/// scheduler already matches the default-enabled fleet state — only the local
/// disable override was missing. (The remote GB gate itself is not portable —
/// LingXi has no GrowthBook substrate — but its default-true state is.)
fn cron_scheduler_enabled(disable_cron_env: Option<&str>) -> bool {
    !traits::env::is_env_truthy(disable_cron_env)
}

/// Expand a raw additional-working-dir entry (settings `additionalDirectories`
/// or CLI `--add-dir`) into an absolute path suitable for
/// [`tool_api::BuiltinToolContext::trusted_dirs`], mirroring the permission
/// policy's `expand_path`: `~`/`~/…` resolve against `home`, a relative path
/// resolves against `cwd`, an absolute path is taken verbatim. Lexical only —
/// `canonicalize_and_validate` still resolves symlinks/`..` on each file-tool
/// use, so the file-tool allowed set matches claude-code `FY(t)` (cwd +
/// additionalWorkingDirectories) rather than hard-blocking `--add-dir` roots.
fn expand_trusted_dir(
    raw: &std::path::Path,
    cwd: &std::path::Path,
    home: Option<&std::path::Path>,
) -> std::path::PathBuf {
    let s = raw.to_string_lossy();
    let t = s.trim();
    if t == "~" {
        home.map_or_else(|| std::path::PathBuf::from(t), std::path::Path::to_path_buf)
    } else if let Some(rest) = t.strip_prefix("~/") {
        home.map_or_else(|| std::path::PathBuf::from(t), |h| h.join(rest))
    } else {
        let p = std::path::Path::new(t);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    }
}

/// Fold the `sandbox` subsection of the settings tiers (ascending priority, last
/// write wins) into a full [`sandbox::runtime_config::SandboxRuntimeConfig`].
///
/// `ctx` carries the session/host seeds (`getClaudeTempDir()`, the settings-file
/// `deny_write` paths, the managed drop-in dir, `.lingxi/skills`, …). It is built
/// by the caller (the composition root has `lingxi_home`/`cwd`/`managed` in scope,
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

/// Compute the managed-only sandbox overrides for the
/// `allowManagedDomainsOnly` / `allowManagedReadPathsOnly` enforcement. Parses
/// the MANAGED (`policySettings`) raw tiers ONLY — the per-source knowledge
/// claude-code uses via `getSettingsForSource('policySettings')`. When a flag is
/// set there, returns `Some(allowlist)` (the managed-source domains / read paths)
/// to OVERRIDE the merged config; `None` ⇒ no restriction. Threaded onto
/// [`SandboxConvertContext`] so [`convert_settings_to_runtime_config`] applies it.
fn managed_only_sandbox_overrides(
    managed_raw_tiers: &[String],
    settings_dir: &std::path::Path,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    use sandbox::runtime_config::{SandboxSettingsJson, SettingsJson, SettingsPermissions};
    let mut merged_sandbox: Option<SandboxSettingsJson> = None;
    let mut merged_perms = SettingsPermissions::default();
    let mut saw_perms = false;
    for raw in managed_raw_tiers {
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
    let managed = SettingsJson {
        permissions: saw_perms.then_some(merged_perms),
        sandbox: merged_sandbox,
        settings_dir: Some(settings_dir.to_path_buf()),
    };
    let domains_only = managed
        .sandbox
        .as_ref()
        .and_then(|s| s.network.as_ref())
        .is_some_and(|n| n.allow_managed_domains_only);
    let reads_only = managed
        .sandbox
        .as_ref()
        .and_then(|s| s.filesystem.as_ref())
        .is_some_and(|f| f.allow_managed_read_paths_only);
    let domains = domains_only.then(|| sandbox::policy_convert::managed_domain_allowlist(&managed));
    let reads = reads_only
        .then(|| sandbox::policy_convert::managed_read_path_allowlist(&managed, settings_dir));
    (domains, reads)
}

/// Resolve the SOURCE-RESTRICTED `allowAppleEvents` value. claude-code honors
/// this sandbox setting ONLY from user, managed/policy, or CLI `--settings`
/// (`flagSettings`) sources — project & local `.lingxi/settings*.json` are
/// IGNORED (sandbox-adapter.ts 2.1.207 @223928133:
/// `allowAppleEvents:[...managedSources, wr("flagSettings"), userSettings]
/// .map(z => z?.sandbox?.allowAppleEvents).find(z => z !== undefined)`).
/// First-defined wins in order managed/policy → flag → user. CC pre-folds the
/// file-based managed tiers into ONE object with a DEEP merge
/// (`loadManagedFileSettings` → `Fie(r, next, Bpe)`: base then drop-ins sorted,
/// later scalars override earlier but omitted fields are preserved), so
/// `sandbox.allowAppleEvents` is resolved PER-FIELD last-defined across the
/// tiers — a later drop-in that carries only a partial `sandbox` block (e.g.
/// `{"sandbox":{"network":…}}`) must NOT clobber an earlier tier's value. The
/// engine has no boot-time `--settings` analog (see the `sandbox_runtime_cfg`
/// comment on `flagSettings`), so the flag slot is skipped. Returns `None` when
/// no honored source set it — matching CC's `.find(...) === undefined ⇒ manager
/// reads `false``. Threaded onto
/// [`SandboxConvertContext::allow_apple_events_override`].
fn apple_events_override(
    managed_raw_tiers: &[String],
    user_settings_raw: Option<&str>,
) -> Option<bool> {
    use sandbox::runtime_config::SettingsJson;
    // Managed/policy file tiers are deep-merged, so resolve `allowAppleEvents`
    // per-field last-defined (later drop-ins win, `None` tiers don't clobber).
    let mut merged_managed: Option<bool> = None;
    for raw in managed_raw_tiers {
        if let Ok(parsed) = serde_json::from_str::<SettingsJson>(raw) {
            if let Some(v) = parsed.sandbox.and_then(|s| s.allow_apple_events) {
                merged_managed = Some(v);
            }
        }
    }
    if let Some(v) = merged_managed {
        return Some(v);
    }
    // flagSettings has no boot-time analog in the engine (skipped) — then user.
    user_settings_raw
        .and_then(|raw| serde_json::from_str::<SettingsJson>(raw).ok())
        .and_then(|s| s.sandbox)
        .and_then(|s| s.allow_apple_events)
}

/// Resolve the SOURCE-RESTRICTED `sandbox.network.strictAllowlist` (2.1.219).
///
/// Same tier rule as [`apple_events_override`]: honored only from managed /
/// policy, CLI `--settings` (no boot-time analog here), and user settings.
/// Project `.lingxi/settings.json` and `settings.local.json` are IGNORED — the
/// oracle's own description says so outright.
///
/// `None` ⇒ no honored source set it, and the converter clears the flag rather
/// than inheriting whatever a non-honored tier merged in.
fn strict_allowlist_override(
    managed_raw_tiers: &[String],
    user_settings_raw: Option<&str>,
) -> Option<bool> {
    use sandbox::runtime_config::SettingsJson;
    // Managed/policy file tiers are deep-merged; resolve per-field last-defined
    // so a partial drop-in cannot clobber an earlier tier's value.
    let mut merged_managed: Option<bool> = None;
    for raw in managed_raw_tiers {
        if let Ok(parsed) = serde_json::from_str::<SettingsJson>(raw) {
            if let Some(n) = parsed.sandbox.and_then(|s| s.network) {
                if n.strict_allowlist {
                    merged_managed = Some(true);
                }
            }
        }
    }
    if let Some(v) = merged_managed {
        return Some(v);
    }
    user_settings_raw
        .and_then(|raw| serde_json::from_str::<SettingsJson>(raw).ok())
        .and_then(|s| s.sandbox)
        .and_then(|s| s.network)
        .and_then(|n| n.strict_allowlist.then_some(true))
}

/// claude-code `getClaudeTempDir()` + `getClaudeTempDirName()` analog (Shell.ts:307),
/// identical to the canonical private `lingxi_temp_dir()` in `tool-shell`'s
/// `prompt.rs`: `baseTmpDir = LINGXI_TMPDIR || (windows ? tmpdir() : "/tmp")`,
/// realpath-resolved, name `claude` on Windows else `claude-{uid}`, joined with a
/// trailing separator. Seeded into the sandbox `allow_write` so the shell's
/// cwd-tracking file stays writable.
fn lingxi_temp_dir() -> String {
    let base: std::path::PathBuf = std::env::var_os("LINGXI_TMPDIR")
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
            // 2.1.198: Sonnet 5 is the default first-party model (alias table
            // sonnet.default = "claude-sonnet-5").
            default_model: "claude-sonnet-5".to_string(),
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
    // the BashTool is the byte-identical no-firer variant. No shared live-cwd
    // cell either: every tool falls back to `ctx.workspace` / the process cwd.
    register_desktop_tools(
        &mut reg,
        ctx,
        coordinator,
        None,
        None,
        cron_auth,
        None,
        None,
        None,
        None,
        None,
    );
    reg
}

/// Launches `LocalWorkflow` background tasks for the `Workflow` tool by spawning
/// through the shared [`tasks::registry::TaskRegistry`]. Resolves the spec's
/// `scriptPath` / `script` / `name` to a script source (claude-code precedence);
/// `scriptPath`/`name` are read from disk relative to `cwd`.
struct TaskRegistryWorkflowLauncher {
    registry: Arc<tasks::registry::TaskRegistry>,
    cwd: std::path::PathBuf,
    /// The claude home directory (e.g. `~/.claude`), used to derive
    /// `transcriptDir = <sessionProjectDir>/<sessionId>/subagents/workflows/<runId>`.
    lingxi_home: std::path::PathBuf,
    /// The main session UUID (bare uuid string, no `sess:` prefix), threaded
    /// from the composition root's `main_session_uuid` so the transcript dir
    /// anchors on the correct session.
    session_uuid: String,
}

#[async_trait::async_trait]
impl tool_workflow::WorkflowLauncher for TaskRegistryWorkflowLauncher {
    async fn launch(
        &self,
        spec: tool_workflow::WorkflowLaunchSpec,
    ) -> Result<tool_workflow::WorkflowLaunched, tool_workflow::WorkflowLaunchError> {
        let cwd = self.cwd.clone();
        let abs = |p: &str| -> std::path::PathBuf {
            let path = std::path::Path::new(p);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            }
        };
        let script = tool_workflow::resolve_script(&spec, |p| std::fs::read_to_string(abs(p)))?;
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
        // Determinism gate (claude-code validateInput `if (e.script && HKa(...))`):
        // an INLINE `script` may not use Date.now()/Math.random()/new Date()
        // (breaks resume). Author-controlled `scriptPath`/`name` files are exempt.
        let is_inline = spec.script.as_deref().is_some_and(|s| !s.is_empty())
            && spec
                .script_path
                .as_deref()
                .filter(|s| !s.is_empty())
                .is_none();
        if is_inline {
            if let Err(workflow::WorkflowError::Script(m)) = workflow::check_determinism(&script) {
                return Err(tool_workflow::WorkflowLaunchError(m));
            }
        }
        // Resume gate (claude-code validateInput errorCode 3): a `resumeFromRunId`
        // that names a STILL-RUNNING workflow is rejected — two runs sharing a run
        // id would race on the same journal. The WorkflowTool can't reach the task
        // registry from its `ToolUseContext`, so the gate lives here in the
        // launcher (which owns the registry). Message byte-exact (`ED` = TaskStop).
        if let Some(rid) = spec.resume_from_run_id.as_deref().filter(|s| !s.is_empty()) {
            if let Some(task_id) = self.registry.find_running_workflow_by_run_id(rid).await {
                return Err(tool_workflow::WorkflowLaunchError(format!(
                    "Workflow {rid} is still running (task {task_id}). Stop it first with \
                     TaskStop({{taskId: \"{task_id}\"}}) before resuming."
                )));
            }
        }
        // Mint the run id at launch (fresh) or reuse the resume id — so it can be
        // returned in the tool result (claude-code `runId`) for `resumeFromRunId`.
        // A clock-nanos × per-process sequence gives a unique id (host clock use
        // is fine — only the workflow SCRIPT is barred from the clock). The
        // surfaced shape matches claude-code 2.1.195 `wf_${randomUUID().slice(0,12)}`
        // = `wf_` + 8 hex + `-` + 3 hex (the first 12 chars of a v4 UUID).
        let run_id = spec.resume_from_run_id.clone().unwrap_or_else(|| {
            use std::sync::atomic::{AtomicU64, Ordering};
            static WF_SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = WF_SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let v = nanos ^ seq.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            format!("wf_{:08x}-{:03x}", (v >> 32) as u32, (v as u32) & 0xfff)
        });
        // Persist the script so it is editable + re-runnable via `scriptPath`
        // (claude-code persists every invocation's script "under the session
        // directory"). A `scriptPath` input is already on disk → return it as-is;
        // an inline/`name` script is written under `<cwd>/.lingxi-scratch/workflows`.
        let script_path = if let Some(p) = spec.script_path.as_deref().filter(|s| !s.is_empty()) {
            abs(p).to_str().map(str::to_string)
        } else {
            let dir = cwd.join(".lingxi-scratch").join("workflows");
            let file = dir.join(format!("{run_id}.js"));
            (std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, &script).is_ok())
                .then(|| file.to_str().map(str::to_string))
                .flatten()
        };
        // `meta.name` → `workflowName` in the result.
        let workflow_name = workflow::meta_string_value(&script, "name");
        // `meta.description` → `summary` in the result (claude-code `p = c.meta.description`).
        let summary = workflow::meta_string_value(&script, "description");
        // `transcriptDir` = `<sessionProjectDir>/<sessionId>/subagents/workflows/<runId>`
        // (claude-code `Nte(runId)` → `path.join(CU() ?? _g(gr()), xt(), "subagents",
        // "workflows", e)`). We derive via `orchestrator::transcript_paths::subagents_dir`
        // which computes `<lingxi_home>/projects/<sanitize(cwd)>/<session_uuid>/subagents`,
        // then append `workflows/<runId>`.
        let transcript_dir = {
            let subagents = orchestrator::transcript_paths::subagents_dir(
                &self.lingxi_home,
                &self.cwd.to_string_lossy(),
                &self.session_uuid,
            );
            subagents
                .join("workflows")
                .join(&run_id)
                .to_str()
                .map(str::to_string)
        };
        // Derive telemetry fields for tengu_workflow_launched (oracle §7).
        let (invocation_mode, workflow_source) =
            if let Some(p) = spec.script_path.as_deref().filter(|s| !s.is_empty()) {
                ("scriptPath".to_string(), p.to_string())
            } else if let Some(n) = spec.name.as_deref().filter(|s| !s.is_empty()) {
                ("named".to_string(), n.to_string())
            } else {
                ("inline".to_string(), "inline".to_string())
            };
        let task_id = self
            .registry
            .spawn(
                tasks::TaskType::LocalWorkflow,
                tasks::TaskSpawnInput::LocalWorkflow {
                    // Display name = the script's `meta.name` (claude-code
                    // `workflowName`), so an INLINE workflow shows its real name
                    // in `/workflows` rather than the empty fallback; a named
                    // workflow falls back to its saved `spec.name`.
                    workflow_id: workflow_name
                        .clone()
                        .filter(|s| !s.is_empty())
                        .or_else(|| spec.name.clone())
                        .unwrap_or_default(),
                    script,
                    resume_from_run_id: spec.resume_from_run_id.clone(),
                    // The `args` global, serialised to a JSON string for the runtime.
                    args: spec
                        .args
                        .as_ref()
                        .map(|v| serde_json::to_string(v).unwrap_or_default()),
                    run_id: Some(run_id.clone()),
                    invocation_mode: Some(invocation_mode),
                    workflow_source: Some(workflow_source),
                    // `t.agentId != null` in claude-code: the Workflow tool is called
                    // from a subagent when a sub-session invokes it. LingXi does not
                    // thread the calling agent id to the launcher at this time; treat
                    // as false (top-level launch) — this field is best-effort.
                    launched_from_subagent: false,
                },
                "Workflow".to_string(),
            )
            .await
            .map_err(|e| tool_workflow::WorkflowLaunchError(e.to_string()))?;
        Ok(tool_workflow::WorkflowLaunched {
            task_id,
            run_id: Some(run_id),
            script_path,
            workflow_name,
            summary,
            transcript_dir,
        })
    }
}

/// Composition-root source of the `Stop` / `SubagentStop` hook
/// `background_tasks` + `session_crons` snapshot (claude-code
/// `Lic(taskRegistry.all())` / `Mic()`), bound to the live task registry + the
/// project-root cron file. Mirrors the [`orchestrator::RegistryTaskNotifications`]
/// precedent: it owns the SAME `Arc<dyn TaskRegistryHandle>` the tool context
/// holds, plus the shared `current_cwd` cell, and maps both sources through the
/// orchestrator's pure `build_background_tasks` / `build_session_crons` builders.
struct RegistryStopHookSnapshot {
    registry: Arc<dyn traits::task_registry::TaskRegistryHandle>,
    current_cwd: Arc<std::sync::Mutex<std::path::PathBuf>>,
}

#[async_trait::async_trait]
impl orchestrator::StopHookSnapshotProvider for RegistryStopHookSnapshot {
    async fn background_tasks(&self) -> Vec<hooks::HookBackgroundTask> {
        // claude passes `taskRegistry.all()` (NOT `.running()`); `wA` inside the
        // builder does the running|pending + isBackgrounded filtering. A registry
        // error degrades to "no tasks" so a transient failure never breaks the
        // turn.
        let records = self
            .registry
            .list(traits::task_registry::TaskListFilter::default())
            .await
            .unwrap_or_default();
        orchestrator::build_background_tasks(&records)
    }

    async fn session_crons(&self) -> Vec<hooks::HookSessionCron> {
        // claude `Cv()` is the in-memory session cron list; the port persists the
        // durable cron jobs to `<project_root>/.lingxi/scheduled_tasks.json`. Read
        // + parse it (a missing/garbage file ⇒ no crons, matching claude's
        // unreadable-file-as-empty contract) and map each task into the builder's
        // neutral input.
        let root = self
            .current_cwd
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        let path = cron::tasks_file::scheduled_tasks_path(&root);
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let doc = cron::tasks_file::parse_tasks(&body);
        let mut inputs: Vec<orchestrator::CronSnapshotInput> = doc
            .tasks
            .into_iter()
            .map(|t| orchestrator::CronSnapshotInput {
                id: t.id,
                cron: t.cron,
                recurring: t.recurring,
                prompt: t.prompt,
            })
            .collect();
        if let Ok(session_jobs) = cron::session_jobs(&self.registry).await {
            inputs.extend(
                session_jobs
                    .into_iter()
                    .map(|task| orchestrator::CronSnapshotInput {
                        id: task.id,
                        cron: task.cron,
                        recurring: Some(task.recurring),
                        prompt: task.prompt,
                    }),
            );
        }
        orchestrator::build_session_crons(&inputs)
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
/// JSONL-backed [`tool_api::WorktreeStatePersister`] (parity 2.1.212's
/// `saveWorktreeState`): appends a `worktree-state` entry — carrying the active
/// worktree's serialized session, or `null` on exit — to the session transcript,
/// keyed by the bare `session_uuid` the resume loader reads back
/// (`read_worktree_state`). Persistence is best-effort: a failed append is logged
/// and swallowed so it never fails the `EnterWorktree`/`ExitWorktree` tool call
/// (matching claude's `.catch(...)` on `xX`).
struct JsonlWorktreeStatePersister {
    writer: Arc<session::jsonl::writer::JsonlWriter>,
    session_uuid: String,
}

#[async_trait::async_trait]
impl tool_api::WorktreeStatePersister for JsonlWorktreeStatePersister {
    async fn persist_worktree_state(&self, session: Option<&tool_api::WorktreeSession>) {
        let payload = session.map(tool_api::WorktreeSession::to_persisted_json);
        if let Err(e) = self
            .writer
            .append_worktree_state(&self.session_uuid, payload.as_ref())
            .await
        {
            tracing::warn!(error = %e, "failed to persist worktree-state transcript record");
        }
    }
}

pub fn register_desktop_tools(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    coordinator: Option<CoordinatorWiring>,
    ask_user_question_resolver: Option<
        Arc<dyn tool_ui::ask_user_question::AskUserQuestionResolver>,
    >,
    computer_access_resolver: Option<Arc<dyn tool_computer_use::ComputerAccessResolver>>,
    cron_auth: Option<Arc<dyn tool_cron::ClaudeAiAuthProvider>>,
    skill_loader: Option<Arc<dyn tool_skill::skill::SkillLoader>>,
    cwd_changed_firer: hooks::OptionalCwdChangedFirer,
    web_side_query: Option<Arc<dyn sidequery::SideQueryClient>>,
    live_cwd: Option<tool_api::LiveCwdCell>,
    worktree_state_persister: Option<Arc<dyn tool_api::WorktreeStatePersister>>,
) -> tool_cron::WakeupSchedulerCell {
    // ----- cross-platform tool crates (also linked by engine-mobile, P11) ---
    // (P2-08) The shared live-cwd cell (`getCwd()`/`Ct()`): the desktop `BashTool`
    // writes it on a `cd`, and Read/Glob/Grep + the LSP tool read it as their live
    // cwd (default search dir, "does not exist" cwd notes, relative path root),
    // 1:1 with claude-code's single session-global cwd. `None` (offline factory)
    // falls every tool back to `ctx.workspace` / the process cwd — byte-identical.
    tool_file::register_all_with_live_cwd(reg, ctx.clone(), live_cwd.clone());
    // BASH.4 `onCwdChangedForHooks` (Shell.ts:409): when a firer is supplied (real
    // desktop sessions wire one over the shared `Arc<HookExecutorImpl>`), a `cd`
    // inside a Bash call fires the `CwdChanged` hook. `None` (the offline
    // registry-snapshot path) keeps the byte-identical no-firer BashTool — the
    // registered tool NAMES are unchanged either way, so the locked tool-list
    // snapshot is unaffected. `engine-mobile` never reaches this call (it does
    // not register the shell tools).
    tool_shell::register_all_with_cwd_firer(reg, ctx.clone(), cwd_changed_firer, live_cwd.clone());
    tool_web::register_all(reg, ctx.clone(), web_side_query);
    tool_plan::register_all(reg, ctx.clone());
    tool_meta::register_all(reg, ctx.clone());
    // The `computer` tool (M8-P11b). Cross-platform-registerable — its own
    // `is_enabled()` gates on `ctx.computer_control` being wired (real backend
    // only on macOS today), so registering it unconditionally here is safe:
    // it simply advertises as disabled wherever no backend exists. Real
    // sessions supply a `TuiBridgeResolver` so `request_access` surfaces the
    // approval dialog; `None` (offline/mobile) keeps the fail-closed
    // `DenyAllResolver` default.
    match computer_access_resolver {
        Some(resolver) => {
            tool_computer_use::register_all_with_access_resolver(reg, ctx.clone(), resolver);
        }
        None => tool_computer_use::register_all(reg, ctx.clone()),
    }
    // `RemoteTrigger` gets the credential-store auth provider on desktop so it
    // can drive the claude.ai CCR API in-process. `register_all_with_auth`
    // registers `ScheduleCron` + `RemoteTrigger` (the latter with `cron_auth`)
    // and `ScheduleWakeup`; it returns the wakeup cell threaded out to `build`
    // → `DesktopRuntime` so the bridge fills it once the per-connection queue +
    // spawner exist (see `boot::assemble`).
    let wakeup_cell = tool_cron::register_all_with_auth(reg, ctx.clone(), cron_auth);
    // In coordinator mode the richer `coordinator` `SendMessage` (registered
    // below, IN PLACE OF this builtin) carries the swarm routing surface, so we
    // skip the leaner `tool_ui` `SendMessage` here — otherwise, because the
    // registry's `find_by_name` is builtin-first, the earlier `tool_ui` copy
    // would silently shadow the coordinator one. Mirrors the `tool_team`-skip
    // for `TeamCreate` / `TeamDelete`.
    if coordinator.is_some() {
        if let Some(resolver) = ask_user_question_resolver.clone() {
            tool_ui::register_all_except_send_message_with_ask_resolver(reg, ctx.clone(), resolver);
        } else {
            tool_ui::register_all_except_send_message(reg, ctx.clone());
        }
    } else if let Some(resolver) = ask_user_question_resolver {
        tool_ui::register_all_with_ask_resolver(reg, ctx.clone(), resolver);
    } else {
        tool_ui::register_all(reg, ctx.clone());
    }
    // PARITY (2.1.207 H-BIN-03): the `Artifact` tool (binary `eIs`, name `dw`).
    // Registered always on desktop; its `is_enabled` replicates CC's `dY()` gate
    // — the Statsig gate `tengu_cobalt_plinth` (code-default FALSE with no flag
    // backend) AND `allow_cobalt_plinth` AND first-party auth AND subscription
    // tier — so with no Statsig backend the tool registers DISABLED (invisible
    // to the model), byte-identical to the shipped binary on a host without the
    // `cobalt_plinth` gate. The publish/list claude.ai pipeline + the
    // `artifact-design`/`artifact-capabilities` bundled skills are Stage-2.
    reg.register_builtin(Arc::new(tool_ui::ArtifactTool::new(ctx.clone())));
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
    // (parity 2.1.212) Thread the transcript persister into EnterWorktree /
    // ExitWorktree so a create/enter writes a `worktree-state` entry and an exit
    // writes the clear record — the persist half of resume restoration. `None`
    // (offline factory / `--no-session-persistence`) leaves the tools on their
    // pre-persist path.
    tool_worktree::register_all_with_persister(reg, ctx.clone(), worktree_state_persister);
    tool_mcp::register_all(reg, ctx.clone());
    tool_lsp::register_all_with_live_cwd(reg, ctx, live_cwd);
    wakeup_cell
}

/// Assemble the desktop builtin **skill** registry.
///
/// Delegates to `skill_api::register_desktop`, the single place that names
/// the desktop builtin skill set. Empty in M8 (no Rust-bundled skills yet —
/// skills are markdown loaded from disk by the session loader); the mobile
/// composition root will call `skill_api::register_mobile` instead.
#[must_use]
pub fn desktop_skill_registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    skill_api::register_desktop(&mut reg);
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
/// - `lingxi_home` — the `~/.claude` (and platform config-dir) root the hook /
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
///     isolated_credential_storage: false,
///     api_key_helper: None,
///     managed_oauth_only: false,
///     anthropic_key_fd_present: false,
///     cwd: PathBuf::from("/tmp/project"),
///     lingxi_home: PathBuf::from("/tmp/home/.lingxi"),
///     default_model: "claude-sonnet-5".to_string(),
///     default_model_explicit: false,
///     recent_models: Vec::new(),
///     fallback_model: None,
///     custom_betas: Vec::new(),
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
///     // `Some(orchestrator::prompt::real_provider())` to load real LINGXI.md.
///     memory_provider: None,
///     permission_mode: permission::PermissionMode::Default,
///     allow_dangerously_skip_permissions: false,
///     connect_prompt: None,
///     system_prompt_override: None,
///     append_system_prompt: None,
///     session_id_override: None,
///     parent_session_id: None,
///     disable_slash_commands: false,
///     add_dir: Vec::new(),
///     cli_mcp_servers: Vec::new(),
///     strict_mcp_config: false,
///     exclude_dynamic_system_prompt_sections: false,
///     setting_source_scope: (true, true),
///     customization_gates: engine_desktop::CustomizationGates::default(),
///     session_persistence: true,
///     cli_agents_json: None,
///     cli_agent: None,
///     cli_plugin_dirs: Vec::new(),
///     initial_effort: None,
///     plan_mode_instructions: None,
///     plans_directory: None,
///     default_model_env_pinned: false,
///     session_thinking: Default::default(),
///     // `None` ⟶ inert: no `-w`/`--worktree` boot launch.
///     worktree_launch: None,
///     // `None` ⟶ inert: no `--tmux` worktree tmux session.
///     tmux_launch: None,
///     // `None` ⟶ background session forking is unavailable to this host.
///     bg_session_forker: None,
///     // `None` ⟶ AskUserQuestion uses the non-TUI fallback path.
///     ask_user_question_tx: None,
///     // `None` ⟶ `request_access` uses the fail-closed DenyAllResolver.
///     computer_access_tx: None,
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
    /// Inherit NO ambient credentials from the machine.
    ///
    /// The native backends are keyed by OS USER, not by [`Self::lingxi_home`],
    /// so a boot that points `lingxi_home` at a temp directory still reads the
    /// machine's real login keychain. That makes credential-dependent behaviour
    /// answer differently on a developer's logged-in machine than on a clean
    /// one — which is a test-isolation hazard, not a preference. Hosts that
    /// need a deterministic credential picture (sandboxed boots, e2e tests) set
    /// this; production leaves it `false`.
    ///
    /// Covers BOTH ambient sources: the OS keychain (used instead of the
    /// file-backed store) and the process environment (a bare
    /// `DEEPSEEK_API_KEY` in the developer's shell otherwise marks a provider
    /// connected, which is what made `bridge-server`'s credential-required e2e
    /// assertions machine-dependent).
    pub isolated_credential_storage: bool,
    /// Settings `apiKeyHelper`: shell command/path that prints the Anthropic
    /// auth value. Used only when no higher-priority API key/OAuth source wins.
    pub api_key_helper: Option<String>,
    /// (M13) The HOST launcher forces Claude.ai OAuth as the effective auth
    /// source: with a stored OAuth session it then outranks even an env
    /// `ANTHROPIC_API_KEY` in the auth resolver
    /// (`llm_client::oauth::anthropic::resolver`). claude-code derives this
    /// from `KWr()` (@228931361), a pure env predicate that
    /// `resolve_llm_stack` reads itself via
    /// [`llm_client::oauth::anthropic::resolver::host_managed_oauth_only`], so
    /// every host gets it for free; this field only lets an embedding host
    /// declare the same forcing without the launcher env. A managed
    /// `forceLoginMethod` policy must NEVER be fed here — it has no place in
    /// credential precedence (`zb()` @228933355).
    pub managed_oauth_only: bool,
    /// (M13) `true` when the launcher advertised an FD-inherited Anthropic API
    /// key (`CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR` in claude-code's managed /
    /// remote launches). In the auth resolver an FD key outranks stored OAuth,
    /// so a stored session must NOT report as the Claude.ai-subscriber auth
    /// source. LingXi ships no FD-passing launcher of its own; the CLI fills
    /// this from env presence so the seam is closed for hosts that do.
    pub anthropic_key_fd_present: bool,
    /// Working directory the orchestrator + tool context are rooted at.
    pub cwd: std::path::PathBuf,
    /// The `~/.claude` root the hook / agents / global-MCP / settings loaders
    /// walk. Explicit so a host can redirect it to a sandbox.
    pub lingxi_home: std::path::PathBuf,
    /// Model id the build defaults to (`OrchestratorConfig.model`).
    pub default_model: String,
    /// `true` when [`Self::default_model`] is an EXPLICIT per-session choice
    /// (`--model` flag) rather than the built-in default or the persisted
    /// `settings.model`. An explicit choice is never overridden by the
    /// boot-time connected-provider fallback ([`connected_provider_fallback`]).
    pub default_model_explicit: bool,
    /// `settings.recentModels` (most-recent-first), read by the host — `build()`
    /// itself stays off host config files (F2-01). Feeds the connected-provider
    /// fallback's preference pass. Empty ⟶ no recents (fallback uses the static
    /// provider order only).
    pub recent_models: Vec<RecentModelRef>,
    /// Fallback model id (`OrchestratorConfig.fallback_model`). `None` ⟶ no
    /// fallback, so the 529-overload interception in `turn_loop` stays a strict
    /// no-op. Mirrors `Argv::fallback_model`, which claude-code only HONORS in
    /// `--print`/non-interactive mode ("only works with --print"); the CLI host
    /// applies that gate before filling this field (`resolve_desktop_config`).
    pub fallback_model: Option<String>,
    /// Host-validated custom Anthropic beta header additions for this session.
    /// Empty keeps request headers unchanged.
    pub custom_betas: Vec<String>,
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
    /// CLI `--plan-mode-instructions <instructions>` (print-only): custom plan-mode
    /// workflow body, mapped to
    /// [`orchestrator::OrchestratorConfig::plan_mode_instructions`] in `build()`.
    /// `None` (the default) = the default 5-phase plan reminder.
    pub plan_mode_instructions: Option<String>,
    /// `settings.json` `plansDirectory` (206 `iT`): custom directory for plan
    /// files, relative to the project root, mapped to
    /// [`orchestrator::OrchestratorConfig::plans_directory`] in `build()`.
    /// `None` (the default) = the default `<config-home>/plans/`.
    pub plans_directory: Option<String>,
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
    /// `tui::permission_gate::TuiPermissionGate` here so an `Ask` surfaces as a dialog; `None`
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
    /// The LINGXI.md hierarchy provider the orchestrator loads project/user
    /// memory from. `None` (the default) ⟶ the empty
    /// [`StaticMemoryProvider::empty`], so a default build loads NO memory and
    /// is fully deterministic (the boot tests rely on this). A production host
    /// injects `Some(orchestrator::prompt::real_provider())` to load the real
    /// `<cwd>/LINGXI.md`, `<cwd>/LINGXI.local.md`, and `~/.lingxi/LINGXI.md`
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
    /// EXECUTION-SEMANTICS NOTE: enforcement is default-ON for every mode
    /// (including `BypassPermissions` — the `PolicyPermissionGate` wrap is what
    /// PROVIDES the bypass auto-allow; see `should_enforce_permissions`). Only
    /// the `LINGXI_ENFORCE_PERMISSIONS=0` escape hatch disables it, in which
    /// case this field is execution-neutral but still drives
    /// `BuiltinToolContext.permission_mode` state.
    pub permission_mode: permission::PermissionMode,
    /// Make `BypassPermissions` mode available in the session permission
    /// mode cycle (claude-code `--allow-dangerously-skip-permissions`).
    /// When `true`, `Plan` mode bypasses permissions, and the runtime
    /// `set_permission_mode("bypassPermissions")` gate accepts the mode.
    /// Default `false`.
    pub allow_dangerously_skip_permissions: bool,
    /// Plan 3c: host secure-input port for `/connect <api-key-provider>`. The tui
    /// supplies its masked-input widget; `None` → a headless no-op prompt
    /// (`crate::connect::NoopKeyPrompt`) that cancels.
    pub connect_prompt: Option<Arc<dyn crate::connect::SecureKeyPrompt>>,
    /// CLI `--system-prompt <prompt>` / `--system-prompt-file <file>`: override
    /// the assembled system prompt for the session. When `Some`, replaces the
    /// default memory-hierarchy prompt entirely (`OrchestratorConfig.system_prompt_override`).
    /// `None` (the default) keeps the assembled LINGXI.md hierarchy prompt
    /// (byte-identical to before this field was added).
    pub system_prompt_override: Option<String>,
    /// CLI `--append-system-prompt <prompt>` / `--append-system-prompt-file <file>`:
    /// text to append to the assembled system prompt for the session. When `Some`,
    /// appended after the memory-hierarchy prompt (or after `system_prompt_override`
    /// when both are set). `None` (the default) keeps the assembled prompt unchanged.
    pub append_system_prompt: Option<String>,
    /// CLI `--session-id <uuid>`: use this specific session ID for the
    /// conversation instead of minting a fresh one. `Some` ⟶ the boot-canonical
    /// MAIN session id is parsed from this string (a bare UUID, validated by the
    /// host before it fills this); `None` (the default) ⟶ a fresh random id.
    /// claude-code `--session-id`. The host (`apps/cli` / `apps/bridge-server`)
    /// validates UUID-ness + the cross-flag rules before setting this.
    pub session_id_override: Option<String>,
    /// Source session id for a forked transcript. When present it is appended
    /// to Anthropic's JSON-string `metadata.user_id` as `parent_session_id`.
    /// Ordinary fresh/resumed sessions leave this unset.
    pub parent_session_id: Option<String>,
    /// CLI `--disable-slash-commands` (claude-code "Disable all skills"). When
    /// `true`, the shared command registry is emptied AFTER all builtin + plugin
    /// + skill registration, so the slash dispatcher and the `Skill` tool's
    /// loader (which share the registry `Arc`) observe zero commands/skills.
    /// `false` (the default) keeps the full command set.
    pub disable_slash_commands: bool,
    /// CLI `--add-dir <directories...>` (claude-code "Additional directories to
    /// allow tool access to"). Unioned into the permission policy's
    /// working-directory set exactly like a settings-tier
    /// `permissions.additionalDirectories` entry, so file tools (Read/Edit/Bash)
    /// may operate outside `cwd`. Empty (the default) ⟶ no extra dirs.
    pub add_dir: Vec<std::path::PathBuf>,
    /// CLI `--mcp-config <configs...>` servers (claude-code: "Load MCP servers
    /// from JSON files or strings"). The host parses each file/inline-JSON entry
    /// into server configs; `build()` merges them OVER the discovered
    /// project/global servers (CLI wins on name collision). With
    /// `--strict-mcp-config` the host nulls the discovered paths, so these are
    /// the ONLY servers. Empty (the default) ⟶ none.
    pub cli_mcp_servers: Vec<mcp::McpServerConfig>,
    /// CLI `--strict-mcp-config` ("Only use MCP servers from --mcp-config,
    /// ignoring all other MCP configurations"). The host already nulls the
    /// discovered `.mcp.json` paths when set; the flag itself is threaded here
    /// because the agent-frontmatter MCP merge (claude `FWt`, cc2.1.220) skips
    /// frontmatter servers under strict mode UNLESS the agent came from the
    /// `--agents` flag (`r?.strictMcpConfig && t.source !== "flagSettings"`).
    /// `false` (the default) ⟶ no strict gating.
    pub strict_mcp_config: bool,
    /// CLI `--exclude-dynamic-system-prompt-sections`. Threaded into
    /// `OrchestratorConfig::exclude_dynamic_system_prompt_sections`: moves the
    /// per-machine env block out of the (cacheable) system prompt and into the
    /// first user message. `false` (the default) ⟶ unchanged.
    pub exclude_dynamic_system_prompt_sections: bool,
    /// CLI `--setting-sources <user,project,local>` scope as `(include_user,
    /// include_project)`. Gates which on-disk settings TIERS `build()` reads for
    /// hook registration and permission rules (defaultMode / allow-deny rules /
    /// additionalDirectories): skip the user tier when `!include_user`, skip the
    /// project + local tiers when `!include_project`. This mirrors the
    /// `Settings::load_scoped` gating the CLI already applies to provider /
    /// routing / claudeMdExcludes loaders, so `--setting-sources project` no
    /// longer loads user-level hooks or permission rules. `(true, true)` (the
    /// default, also the absent-flag case) ⟶ all tiers load, byte-identical to
    /// before this field.
    pub setting_source_scope: (bool, bool),
    /// CLI `--safe-mode` / `--bare` customization gates (M3, cc 2.1.198).
    /// Consumed at each registration site in `build()` (hooks / agents /
    /// plugins / custom commands + skills); the CLI also gates the
    /// memory-provider + discovered-MCP-path config fields it resolves itself.
    /// `CustomizationGates::default()` (both false) ⟶ byte-identical to before
    /// this field.
    pub customization_gates: CustomizationGates,
    /// CLI `--no-session-persistence` (print-mode only; the CLI validates the
    /// cross-flag rule): `false` ⟶ `build()` wires NO session `JsonlWriter`, so
    /// nothing is saved under `projects/` and the session cannot be resumed
    /// (claude-code "Disable session persistence - sessions will not be saved
    /// to disk and cannot be resumed"). `true` (the default) ⟶ unchanged.
    pub session_persistence: bool,
    /// (M4 cc2.1.198) CLI `--agents <json>` raw payload ("JSON object defining
    /// custom agents"). `build()` parses it with the strict flag-record schema
    /// (`agent::parse_agents_from_flag_json`, the `QXt` port) and merges the
    /// result into the agent catalog with `flagSettings` precedence — flag
    /// agents OVERRIDE same-named user/project dir agents (binary `XXt`
    /// tier order `[built-in, plugin, userSettings, projectSettings,
    /// flagSettings, policySettings]`, later wins). Ignored (warn) in safe
    /// mode; SURVIVES bare (`Hc("agents",{explicitlyRequested:!0})`
    /// @223080769). `None` (the default) ⟶ unchanged.
    pub cli_agents_json: Option<String>,
    /// (M4 cc2.1.198) CLI `--agent <agent>` ("Agent for the current session.
    /// Overrides the 'agent' setting."). `build()` resolves it against the
    /// final catalog with the `dts` lookup (exact `agentType`, else FQN
    /// `…:{name}` suffix) and logs the binary's `Warning: agent "X" not
    /// found …` line when absent. (P2-02 cc2.1.207) On a HIT it APPLIES the
    /// agent to the MAIN thread (`bde`/`mainThreadAgentDefinition`): agentType,
    /// system prompt (`nre`), `tools:`/`disallowedTools` pool filter (`HJ`), and
    /// `model` override (`jb(Zo(model))`, unless `--model` was given). RESIDUAL:
    /// frontmatter `hooks`/`mcpServers` swap + resume restoration (`rVe`).
    pub cli_agent: Option<String>,
    /// (M4 cc2.1.198) CLI `--plugin-dir <path>` entries ("Load a plugin from a
    /// directory or .zip for this session only", repeatable). Each entry feeds
    /// the plugin bootstrap AFTER the marketplace-installed discovery, like the
    /// binary's inline-plugin load (`EBm`): a missing path warns
    /// (`Plugin path does not exist: … , skipping`) without failing boot; a
    /// `.zip` is extracted to a temp dir (wrapper-dir detection like `Yor`)
    /// before the normal dir load. Empty (the default) ⟶ none.
    pub cli_plugin_dirs: Vec<std::path::PathBuf>,
    /// (M4 cc2.1.198) CLI `--effort <level>` — the session's initial effort
    /// level, already validated/normalized by the CLI (`u4i` argParser port:
    /// trim+lowercase, `med`→`medium`, must be one of low/medium/high/xhigh/
    /// max; an invalid value warned on stderr and arrives here as `None`).
    /// `build()` threads it to the main-loop `ProviderApiAdapter` so every
    /// main-session request carries `output_config.effort` (+ the
    /// `effort-2025-11-24` beta the service adds when the body has effort).
    /// `None` (the default) ⟶ requests unchanged (no effort field).
    pub initial_effort: Option<String>,
    /// `true` when [`Self::default_model`] was pinned by the `ANTHROPIC_MODEL`
    /// env var (claude-code D4 `process.env.ANTHROPIC_MODEL`) rather than by the
    /// built-in default or the persisted `settings.model`. Kept SEPARATE from
    /// [`Self::default_model_explicit`] (which stays `--model`-only, matching the
    /// binary's `userSpecifiedModel` = the `--model` flag) so the `--agent`
    /// model-override gate is unaffected; it ONLY exempts an env-pinned model
    /// from the boot connected-provider fallback (the user pinned exactly that
    /// model via env, so a reroute would defeat the pin). `false` (the default).
    pub default_model_env_pinned: bool,
    /// Boot SESSION thinking configuration, resolved host-side from the
    /// `MAX_THINKING_TOKENS` env var + the `--max-thinking-tokens` flag +
    /// the `alwaysThinkingEnabled` setting (claude-code `qIe()` + the `wn`
    /// request-build arm; see `llm_client::model::thinking::
    /// session_thinking_from_env`). Applied to BOTH the main-loop `ApiService`
    /// (`.with_thinking`) and the compaction/side-query `ForkedAgentRunner`
    /// (`.with_session_thinking`), so the summarizer inherits the same intent.
    /// The env read is host-side (F2-01: `build()` must not read env), so this
    /// carries the already-resolved config. Defaults to
    /// [`ThinkingConfig::Adaptive`] — byte-identical to the pre-resolver boot.
    pub session_thinking: llm_client::model::thinking::ThinkingConfig,
    /// CLI `-w`/`--worktree [name]` (worktree-tmux-launch plan, Task 3):
    /// create + enter a git worktree at boot. `None` (the default, and the
    /// only value every host but `apps/cli` currently supplies) ⟶ INERT — no
    /// worktree is created, `BuiltinToolContext.worktree_session` stays
    /// `None`, and boot is byte-identical to before this field existed.
    /// `Some("")` (a bare `-w`, `argv.worktree`'s empty-string sentinel) ⟶
    /// `build()` mints a random slug via the same
    /// [`tool_worktree::worktree::gen_random_slug`] helper `EnterWorktree`
    /// uses for a name-less create. `Some(name)` ⟶ that name is the slug.
    /// `--tmux` (`WorktreeSession.tmux_session_name`) is threaded separately
    /// via [`Self::tmux_launch`] (Task 4).
    pub worktree_launch: Option<String>,
    /// CLI `--tmux[=mode]` (worktree-tmux-launch plan, Task 4): create a
    /// detached tmux session (`tmux new-session -d -s <name> -c <path>`) for
    /// the worktree `worktree_launch` creates, recording the session name
    /// into `WorktreeSession.tmux_session_name`. `None` (the default, and the
    /// only value every host but `apps/cli` currently supplies, and every
    /// `apps/cli` session that omits `--tmux`) ⟶ INERT — no tmux session is
    /// created, `tmux_session_name` stays `None`, boot is byte-identical to
    /// before this field existed. `Some(mode)` ⟶ `apply_worktree_launch`
    /// creates the session AFTER the worktree itself is created+swapped+
    /// recorded; a tmux failure is logged and does NOT fail boot (the
    /// worktree launch itself already succeeded). `--tmux` requires
    /// `worktree_launch.is_some()` — `Some` here with `worktree_launch ==
    /// None` is a hard boot failure ([`BuildError::TmuxRequiresWorktree`]),
    /// mirroring the 206 constraint "Create a tmux session for the worktree
    /// (requires --worktree)".
    pub tmux_launch: Option<String>,
    /// 2.1.212 `/fork` (`vAd`) background-session forker seam. When `Some`,
    /// `build()` wires it onto the orchestrator via `with_bg_session_forker`, so
    /// `OrchestratorHandle::fork_to_background_session` copies the live
    /// conversation into a new background session (the `--bg`/daemon session-copy
    /// path). `None` (the default, and every host but `apps/cli`) ⟶ that `/fork`
    /// variant fails with a clear `ActionFailed` — INERT boot. The concrete impl
    /// lives in `apps/cli` (which owns the daemon dispatch machinery); injecting
    /// it here keeps the leaf `orchestrator` crate off an `apps/cli` dependency.
    pub bg_session_forker: Option<Arc<dyn traits::bg_session_forker::BgSessionForker>>,
    /// Optional per-runtime TUI AskUserQuestion bridge sender. Interactive TUI
    /// hosts fill this so questionnaire tools open the mounted bottom-pane
    /// view; non-TUI hosts leave it `None`.
    pub ask_user_question_tx: Option<
        tokio::sync::mpsc::Sender<tui_core::ask_user_question_bridge::AskUserQuestionExchange>,
    >,
    /// Optional per-runtime TUI `computer` tool `request_access` bridge
    /// sender. Interactive TUI hosts fill this so the approval dialog opens
    /// in the mounted bottom-pane view; non-TUI hosts (and hosts without a
    /// computer-control backend) leave it `None`, which keeps
    /// `request_access` on the fail-closed `DenyAllResolver` default.
    pub computer_access_tx: Option<
        tokio::sync::mpsc::Sender<tui_core::computer_access_bridge::ComputerAccessExchange>,
    >,
}

/// `--safe-mode` / `--bare` reduced-mode customization gates (M3, cc 2.1.198).
///
/// Port of the binary's `Hc(feature, opts)` check (@209090235-ish minified:
/// `function Hc(e,t){if(Ql()&&!K5d[e])return!0;if(xd()&&!t?.explicitlyRequested)
/// return V5d[e];return!1}` with the two verdict maps
/// `V5d`(bare)/`K5d`(safe-allowlist) @209090400), where `Ql()` = env
/// `CLAUDE_CODE_SAFE_MODE` truthy OR argv `--safe-mode`, and `xd()` = env
/// `CLAUDE_CODE_SIMPLE` truthy OR argv `--bare`. The per-feature helpers below
/// bake in the map entries for exactly the features this composition root
/// registers; each cites its map values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CustomizationGates {
    /// `--safe-mode` (or env): "Start with all customizations (CLAUDE.md,
    /// skills, plugins, hooks, MCP servers, custom commands and agents, …)
    /// disabled — useful for troubleshooting a broken configuration."
    pub safe_mode: bool,
    /// `--bare` (or env): "Minimal mode: skip hooks, LSP, plugin sync, …,
    /// and CLAUDE.md auto-discovery."
    pub bare: bool,
}

impl CustomizationGates {
    /// Settings-file hooks. Binary: bare disables outright (`V5d.hooks:!0`);
    /// safe mode passes `Hc` (`K5d.hooks:!0`) but the hooks-config merge
    /// collapses to the POLICY tier only (`UQr()` @209090550:
    /// `if(e?.allowManagedHooksOnly===!0||Ql())return e?.hooks??{}`). lingxi
    /// loads no policySettings hook tier (user + project only), so the safe-mode
    /// "policy hooks still run" residue is the empty set here — both modes skip
    /// the settings-hook loop.
    #[must_use]
    pub fn disables_settings_hooks(&self) -> bool {
        self.safe_mode || self.bare
    }

    /// Plugin discovery + materialisation (`V5d.plugins:!0`,
    /// `K5d.plugins:!1`; safe-mode log @211049652 "Skipping plugin hooks -
    /// safe mode disables plugins"). Skipping the plugin bootstrap also skips
    /// plugin LSP servers — lingxi's only LSP-server source — matching
    /// `Hc("lspServers")` gating `initializeLspServerManager` (@213275452;
    /// `V5d.lspServers:!0`, `K5d.lspServers:!1`).
    #[must_use]
    pub fn disables_plugins(&self) -> bool {
        self.safe_mode || self.bare
    }

    /// Skill + custom-command dir discovery (`V5d.skills:!0`,
    /// `K5d.skills:!1`; the user commands-dir loader `cWa` @213449557 bails on
    /// `xd()||Hc("skills")`). RESIDUAL: in bare mode the binary still loads
    /// skills from `--add-dir` roots (`aGe` @213453497 `if(xd())return …
    /// o.map(S=>jht(join(S,".claude","skills")…))`) so `/skill-name` keeps
    /// resolving; lingxi's registry loader has no add-dir root wiring yet, so
    /// bare loads none (seam: `desktop_command_registry`'s skill-roots arg).
    #[must_use]
    pub fn disables_skills(&self) -> bool {
        self.safe_mode || self.bare
    }

    /// Custom agent definitions from `agents/` dirs (`V5d.agents:!0`,
    /// `K5d.agents:!1`). In the binary a `--agents` FLAG payload is an
    /// explicit request that survives bare (`Hc("agents",{explicitlyRequested:
    /// !0})` @223080769) but not safe mode ("--agents: ignored in safe mode");
    /// lingxi's `--agents` flag is still parse-and-carry, so only the dir scan
    /// is gated here.
    #[must_use]
    pub fn disables_custom_agents(&self) -> bool {
        self.safe_mode || self.bare
    }

    /// Ambient (project/user `.mcp.json`) MCP discovery. SAFE MODE ONLY:
    /// `fQ` @212967619 `if(Hc("mcpAutoDiscovered"))return{servers:L2(),…}` —
    /// flag-supplied (`--mcp-config`) servers survive; `K5d.mcpAutoDiscovered:
    /// !1` but `V5d.mcpAutoDiscovered:!1` too, i.e. bare does NOT disable
    /// ambient MCP (its help text never lists MCP among the skips).
    #[must_use]
    pub fn disables_mcp_discovery(&self) -> bool {
        self.safe_mode
    }

    /// CLAUDE/LINGXI.md memory hierarchy (`V5d.claudeMd:!0`, `K5d.claudeMd:
    /// !1`). `explicitly_requested` mirrors `eue()` @209090235's
    /// `{explicitlyRequested:cI().length>0}` — `cI()` is the `--add-dir` list
    /// (`additionalDirectoriesForClaudeMd` @205673399) — so bare keeps the
    /// hierarchy when `--add-dir` supplies CLAUDE.md dirs; safe mode never does
    /// (and additionally exports `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`
    /// @223917313).
    #[must_use]
    pub fn disables_claude_md(&self, explicitly_requested: bool) -> bool {
        if self.safe_mode {
            return true;
        }
        self.bare && !explicitly_requested
    }
}

impl std::fmt::Debug for DesktopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secret-, prompt-, and executable-config-bearing fields are rendered
        // only as presence/count markers so `{cfg:?}` remains safe for host
        // diagnostics. Trait objects use the same presence-only convention.
        f.debug_struct("DesktopConfig")
            .field(
                "api_base",
                &if self.api_base.is_empty() {
                    "<empty>"
                } else {
                    "<configured>"
                },
            )
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field(
                "api_key_helper",
                &self.api_key_helper.as_ref().map(|_| "<redacted>"),
            )
            .field("cwd", &self.cwd)
            .field("lingxi_home", &self.lingxi_home)
            .field("default_model", &self.default_model)
            .field("fallback_model", &self.fallback_model)
            .field(
                "provider_profile_count",
                &self
                    .provider_profiles
                    .as_ref()
                    .map(|profiles| profiles.len()),
            )
            .field("routing_configured", &self.routing.is_some())
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
            .field(
                "system_prompt_override_configured",
                &self.system_prompt_override.is_some(),
            )
            .field(
                "append_system_prompt_configured",
                &self.append_system_prompt.is_some(),
            )
            .field("session_id_override", &self.session_id_override)
            .field("parent_session_id", &self.parent_session_id)
            .field("disable_slash_commands", &self.disable_slash_commands)
            .field(
                "ask_user_question_tx",
                &self.ask_user_question_tx.as_ref().map(|_| "<configured>"),
            )
            .field(
                "computer_access_tx",
                &self.computer_access_tx.as_ref().map(|_| "<configured>"),
            )
            .field("add_dir", &self.add_dir)
            .field("cli_mcp_server_count", &self.cli_mcp_servers.len())
            .field("strict_mcp_config", &self.strict_mcp_config)
            .field(
                "exclude_dynamic_system_prompt_sections",
                &self.exclude_dynamic_system_prompt_sections,
            )
            .field("customization_gates", &self.customization_gates)
            .field("session_persistence", &self.session_persistence)
            .field(
                "cli_agents_json_configured",
                &self.cli_agents_json.is_some(),
            )
            .field("cli_agent", &self.cli_agent)
            .field("cli_plugin_dirs", &self.cli_plugin_dirs)
            .field("initial_effort", &self.initial_effort)
            .field("default_model_env_pinned", &self.default_model_env_pinned)
            .field("session_thinking", &self.session_thinking)
            .field(
                "bg_session_forker",
                if self.bg_session_forker.is_some() {
                    &"Some(<forker>)"
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
            // Production reads the real keychain; only isolated hosts opt out.
            isolated_credential_storage: false,
            api_key_helper: None,
            // (M13) Default: no managed OAuth forcing, no FD-inherited key —
            // hosts that resolve either fill them in.
            managed_oauth_only: false,
            anthropic_key_fd_present: false,
            cwd: std::path::PathBuf::from("."),
            lingxi_home: std::path::PathBuf::new(),
            default_model: DesktopEngineConfig::default().default_model,
            default_model_explicit: false,
            recent_models: Vec::new(),
            fallback_model: None,
            custom_betas: Vec::new(),
            provider_profiles: None,
            routing: None,
            mcp_paths: Vec::new(),
            use_noop_permission_gate: true,
            deny_unresolved_ask: false,
            max_turns: None,
            plan_mode_instructions: None,
            plans_directory: None,
            max_budget_usd: None,
            json_schema: None,
            injected_permission_gate: None,
            session_started_as_coordinator: false,
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
            allow_dangerously_skip_permissions: false,
            connect_prompt: None,
            system_prompt_override: None,
            append_system_prompt: None,
            session_id_override: None,
            parent_session_id: None,
            disable_slash_commands: false,
            add_dir: Vec::new(),
            cli_mcp_servers: Vec::new(),
            // Default: no `--strict-mcp-config` (ambient MCP configs load).
            strict_mcp_config: false,
            exclude_dynamic_system_prompt_sections: false,
            // Default: all setting tiers load (absent `--setting-sources`).
            setting_source_scope: (true, true),
            // Default: no reduced mode (neither --safe-mode nor --bare).
            customization_gates: CustomizationGates::default(),
            // Default: persist the session JSONL (absent --no-session-persistence).
            session_persistence: true,
            // (M4 cc2.1.198) Defaults: no --agents payload, no --agent
            // selection, no --plugin-dir entries, no --effort level.
            cli_agents_json: None,
            cli_agent: None,
            cli_plugin_dirs: Vec::new(),
            initial_effort: None,
            // Default: no ANTHROPIC_MODEL env pin; the adaptive-thinking default.
            default_model_env_pinned: false,
            session_thinking: llm_client::model::thinking::ThinkingConfig::default(),
            // Default: no `-w`/`--worktree` flag ⟶ inert boot (no worktree).
            worktree_launch: None,
            // Default: no `--tmux` flag ⟶ inert boot (no tmux session).
            tmux_launch: None,
            // Default: no `/fork`-to-background forker ⟶ that /fork variant
            // fails with a clear ActionFailed until `apps/cli` injects one.
            bg_session_forker: None,
            ask_user_question_tx: None,
            computer_access_tx: None,
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
    lingxi_home: &std::path::Path,
    connect_writer: Arc<dyn command_core::ConnectCredentialWriter>,
    connect_copilot: Arc<dyn command_core::CopilotConnectDriver>,
    connect_chatgpt: Arc<dyn command_core::ChatGptConnectDriver>,
    gates: CustomizationGates,
    // SKILLEXEC: the SAME shared command-registry slot the slash dispatcher and
    // `Skill` tool loader observe (filled by `build()` right after this returns).
    // `/reload-skills` (batch 8) mutates it live so a reload refreshes the set
    // the rest of the session sees.
    shared_registry: Arc<RwLock<CommandRegistry>>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    // Bundled programmatic skills (`/loop`), port of `registerBundledSkills`.
    // Gated on the same cron kill-switch the scheduler uses
    // (`isKairosCronEnabled` ↔ `cron_scheduler_enabled(LINGXI_DISABLE_CRON)`,
    // loop.ts:83). Registered AFTER builtins; `/loop` is not a builtin name so no
    // shadow conflict.
    let cron_enabled = cron_scheduler_enabled(std::env::var("LINGXI_DISABLE_CRON").ok().as_deref());
    command_core::register_bundled_skills(&mut reg, cron_enabled);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth.clone());
    // (H-BIN-09) Override the generic `/login` handler with one that enforces the
    // managed `forceLoginOrgUUID` org pin — the SAME pin `/connect` enforces via
    // `EngineOAuthConnect`. `register_builtin_handler` overwrites in place, so this
    // wins over the plain handler `register_core_batch_2` just registered. Hosts
    // without a managed policy tier (mobile) keep the plain, unrestricted handler.
    reg.register_builtin_handler(Arc::new(
        command_core::LoginHandler::new(auth)
            .with_org_policy(Arc::new(crate::connect::DesktopLoginOrgPolicy)),
    ));
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle.clone());
    // Plan 3c: wire `/connect` over the engine-supplied credential-writer +
    // Copilot device-flow + ChatGPT OAuth seams.
    command_core::register::register_core_connect(
        &mut reg,
        connect_writer,
        connect_copilot,
        connect_chatgpt,
    );
    // Desktop-only command handlers: currently none — the desktop command names
    // (/commit, /diff, /review, /chrome, /ide, …) are served as command-core
    // unimplemented stubs. Register real desktop handlers on `reg` directly here
    // when a future milestone implements them.
    // SLASH.2: discover + register custom `.lingxi/commands/**.md` commands
    // (project up to git-root/home, plus user + managed layers), the same
    // layering claude-code's getCommands uses. Registered AFTER builtins so a
    // same-named custom command shadows a builtin (TS findCommand order).
    let home = dirs::home_dir().unwrap_or_else(|| lingxi_home.to_path_buf());
    let managed_dir = crate::settings_watch::managed_settings_dir();
    // Batch 8: the newly-ported implemented commands (`/fork`, `/goal`,
    // `/recap`, `/reload-skills`, `/skill-doctor`, `/stop`). Wired here (after
    // the skill-discovery roots are known, before the `disables_skills` early
    // return) so the builtins register regardless of the customization gate.
    command_core::register_core_batch_8(
        &mut reg,
        handle.clone(),
        shared_registry,
        cwd.to_path_buf(),
        lingxi_home.to_path_buf(),
        Some(managed_dir.clone()),
        home.clone(),
        Vec::new(),
        gates.safe_mode,
        load_merged_disable_agent_view(cwd),
    );
    // (M3 cc2.1.198) `--safe-mode` / `--bare` disable custom-command + skill
    // dir discovery (`K5d.skills:!1` / `V5d.skills:!0`; the commands-dir
    // loader `cWa` bails on `xd()||Hc("skills")`). Builtins above stay — only
    // the on-disk customization layers are skipped, including the managed dir
    // (the binary's `aGe` returns `[]` before reaching its managed root).
    if gates.disables_skills() {
        return reg;
    }
    let registered = command_core::load_and_register_custom_commands(
        &mut reg,
        cwd,
        lingxi_home,
        &managed_dir,
        &home,
    )
    .await;
    let registered_skills = command_core::load_and_register_skill_commands_with_roots(
        &mut reg,
        cwd,
        lingxi_home,
        Some(&managed_dir),
        &home,
        &[],
    )
    .await;
    reg.register_builtin_handler(Arc::new(command_core::SkillsHandler::with_all_roots(
        cwd.to_path_buf(),
        lingxi_home.to_path_buf(),
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
/// (`!` bash mode) Desktop implementation of the TUI's
/// [`tui_core::bash_runner::BashRunner`] seam.
///
/// Runs a TUI `!command` through the SAME sandboxed [`tool_shell::BashTool`] the
/// model's `Bash` tool uses — NEVER a raw `std::process`/`Command`. It holds a
/// clone of the session [`BuiltinToolContext`] (which carries the live
/// `sandbox_runner` + `sandbox_runtime` config + process runner), constructs a
/// fresh `BashTool` per call, and maps the tool's result `data.{stdout,stderr}`
/// into a [`tui_core::bash_runner::BashRunOutput`]. Because the command rides the same
/// `BashTool::call` path, it is wrapped by the same M2-04 sandbox decision matrix
/// and `sandbox-runtime` runner as a model-issued Bash call. `BashTool`'s
/// `check_permissions` is an allow-all gate, so a user-typed `!` runs sandboxed
/// without a separate permission prompt (matching claude-code's bash mode).
struct DesktopBashRunner {
    ctx: BuiltinToolContext,
}

#[async_trait::async_trait]
impl tui_core::bash_runner::BashRunner for DesktopBashRunner {
    async fn run(&self, command: &str) -> tui_core::bash_runner::BashRunOutput {
        use tool_api::Tool as _;
        let tool = tool_shell::BashTool::new(self.ctx.clone());
        // Progress channel is required by the `Tool::call` signature but Bash
        // emits no progress for a foreground run; drop the receiver.
        let (progress_tx, _progress_rx) = tool_api::progress_channel();
        // A minimal per-call context for a user-initiated `!` command: no
        // tool_use_id, empty history, inert options. The model id is unused for
        // execution (only `BashTool::prompt` reads it).
        let use_ctx = tool_api::ToolUseContext::model_seed(self.ctx.default_model.clone());
        match tool
            .call(
                serde_json::json!({ "command": command }),
                use_ctx,
                progress_tx,
            )
            .await
        {
            Ok(result) => {
                let field = |key: &str| {
                    result
                        .data
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                tui_core::bash_runner::BashRunOutput {
                    stdout: field("stdout"),
                    stderr: field("stderr"),
                    exit_code: result
                        .data
                        .get("exit_code")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|c| i32::try_from(c).ok())
                        .unwrap_or_default(),
                }
            }
            // A spawn/IO/validation error surfaces as stderr text so the TUI
            // still renders a `UserBashOutput` row (no LLM turn, no raw spawn).
            // A spawn/validation failure never ran a command, so there is no
            // status to report; 1 is the conventional "did not succeed".
            Err(e) => tui_core::bash_runner::BashRunOutput {
                stdout: String::new(),
                stderr: e.to_string(),
                exit_code: 1,
            },
        }
    }
}

/// Project-specific `/worktree` slash-command grammar. Claude Code exposes the
/// same lifecycle through `--worktree` plus `EnterWorktree`/`ExitWorktree`, but
/// has no slash row; `LingXi` keeps the upstream command table locked and adds
/// this desktop-only convenience handler at composition time instead.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorktreeSlashAction {
    Create(Option<String>),
    Enter(String),
    Status,
    Keep,
    Remove { discard_changes: bool },
}

const WORKTREE_SLASH_USAGE: &str =
    "Usage: /worktree [status|create [name]|enter <path>|keep|remove [--discard]]";

fn parse_worktree_slash_action(
    args: &ParsedSlashCommand,
) -> Result<WorktreeSlashAction, &'static str> {
    let tokens = &args.positional_args;
    if tokens.is_empty() {
        return Ok(WorktreeSlashAction::Create(None));
    }

    match tokens[0].as_str() {
        "status" if tokens.len() == 1 => Ok(WorktreeSlashAction::Status),
        "create" if tokens.len() == 1 => Ok(WorktreeSlashAction::Create(None)),
        "create" if tokens.len() == 2 => Ok(WorktreeSlashAction::Create(Some(tokens[1].clone()))),
        // Joining the remaining quote-aware tokens accepts both
        // `enter "/path with spaces"` and the forgiving unquoted form.
        "enter" if tokens.len() >= 2 => Ok(WorktreeSlashAction::Enter(tokens[1..].join(" "))),
        // Do not let a missing path fall through to the `<name>` shorthand and
        // accidentally create a worktree literally named `enter`.
        "enter" => Err(WORKTREE_SLASH_USAGE),
        "keep" if tokens.len() == 1 => Ok(WorktreeSlashAction::Keep),
        "remove" if tokens.len() == 1 => Ok(WorktreeSlashAction::Remove {
            discard_changes: false,
        }),
        "remove" if tokens.len() == 2 && tokens[1] == "--discard" => {
            Ok(WorktreeSlashAction::Remove {
                discard_changes: true,
            })
        }
        // `/worktree <name>` mirrors the CLI's `--worktree <name>` shorthand.
        _ if tokens.len() == 1 => Ok(WorktreeSlashAction::Create(Some(tokens[0].clone()))),
        _ => Err(WORKTREE_SLASH_USAGE),
    }
}

/// Desktop slash handler backed by the exact same tools as model-issued
/// worktree operations. This keeps validation, cwd swaps, dirty-worktree
/// protection, telemetry, hooks, and transcript state persistence on one path.
struct DesktopWorktreeCommandHandler {
    ctx: BuiltinToolContext,
    state_persister: Option<Arc<dyn tool_api::WorktreeStatePersister>>,
}

impl DesktopWorktreeCommandHandler {
    fn new(
        ctx: BuiltinToolContext,
        state_persister: Option<Arc<dyn tool_api::WorktreeStatePersister>>,
    ) -> Self {
        Self {
            ctx,
            state_persister,
        }
    }

    fn status(&self) -> String {
        let session = self
            .ctx
            .worktree_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(session) = session else {
            return "No active worktree session.".to_string();
        };

        let branch = if session.branch_name.is_empty() || session.branch_name == "HEAD" {
            "(detached HEAD)"
        } else {
            &session.branch_name
        };
        let ownership = if session.entered_existing {
            "entered existing (keep only)"
        } else {
            "created by this session"
        };
        let mut lines = vec![
            format!("Worktree: {}", session.worktree_path.display()),
            format!("Branch: {branch}"),
            format!("Original directory: {}", session.original_cwd.display()),
            format!("Ownership: {ownership}"),
        ];
        if let Some(tmux) = session.tmux_session_name {
            lines.push(format!("Tmux session: {tmux}"));
        }
        lines.join("\n")
    }

    fn enter_tool(&self) -> tool_worktree::EnterWorktreeTool {
        let tool = tool_worktree::EnterWorktreeTool::new(self.ctx.clone());
        match &self.state_persister {
            Some(persister) => tool.with_state_persister(persister.clone()),
            None => tool,
        }
    }

    fn exit_tool(&self) -> tool_worktree::ExitWorktreeTool {
        let tool = tool_worktree::ExitWorktreeTool::new(self.ctx.clone());
        match &self.state_persister {
            Some(persister) => tool.with_state_persister(persister.clone()),
            None => tool,
        }
    }

    async fn call_tool<T: tool_api::Tool + Sync>(
        &self,
        tool: &T,
        input: serde_json::Value,
    ) -> String {
        let (progress_tx, _progress_rx) = tool_api::progress_channel();
        let use_ctx = tool_api::ToolUseContext::model_seed(self.ctx.default_model.clone());
        match tool.call(input, use_ctx, progress_tx).await {
            Ok(result) => result
                .model_content
                .or_else(|| {
                    result
                        .data
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| result.data.to_string()),
            Err(error) => error.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl BuiltinCommandHandler for DesktopWorktreeCommandHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        use WorktreeSlashAction::{Create, Enter, Keep, Remove, Status};

        let display = match parse_worktree_slash_action(args) {
            Ok(Create(name)) => {
                let input = name.map_or_else(
                    || serde_json::json!({}),
                    |name| serde_json::json!({ "name": name }),
                );
                self.call_tool(&self.enter_tool(), input).await
            }
            Ok(Enter(path)) => {
                self.call_tool(&self.enter_tool(), serde_json::json!({ "path": path }))
                    .await
            }
            Ok(Status) => self.status(),
            Ok(Keep) => {
                self.call_tool(&self.exit_tool(), serde_json::json!({ "action": "keep" }))
                    .await
            }
            Ok(Remove { discard_changes }) => {
                self.call_tool(
                    &self.exit_tool(),
                    serde_json::json!({
                        "action": "remove",
                        "discard_changes": discard_changes,
                    }),
                )
                .await
            }
            Err(usage) => usage.to_string(),
        };
        CommandResult::Done {
            display: Some(display),
        }
    }

    fn name(&self) -> &str {
        "worktree"
    }

    fn description(&self) -> &str {
        "Create, enter, inspect, or exit a worktree"
    }

    fn allowed_tools(&self) -> &'static [&'static str] {
        &[
            tool_worktree::worktree::ENTER_TOOL_NAME,
            tool_worktree::worktree::EXIT_TOOL_NAME,
        ]
    }
}

pub struct DesktopRuntime {
    /// The fully-constructed orchestrator (cost tracker + MCP/hook/agent
    /// registries + compaction wired), bound to the supplied output stream and
    /// permission gate.
    pub orchestrator: Arc<ConversationOrchestrator>,
    /// Shared slash-command registry populated during build and observed by both
    /// the dispatcher and skill/plugin loaders. Surfaced so non-TUI hosts can
    /// snapshot the live catalog and detect command-set mutations.
    pub shared_command_registry: Arc<RwLock<CommandRegistry>>,
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
    /// The ENFORCING permission gate (the `PolicyPermissionGate` wrapping the
    /// injected prompt transport, or the base gate when enforcement is off). The
    /// interactive TUI holds this to drive Shift+Tab live permission-mode
    /// cycling via [`permission::gate::PermissionGate::set_permission_mode`], so
    /// enforcement follows the bottom-of-composer mode indicator. `None` only
    /// when no gate was built.
    pub enforcing_permission_gate: Option<Arc<dyn PermissionGate>>,
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
    /// (`/sandbox`) The shared fast-toggle cell for bash-command sandboxing.
    /// The SAME `Arc<AtomicBool>` the bash tool reads via
    /// `BuiltinToolContext::sandbox_enabled_override`; the TUI mount threads a
    /// clone into the widget so `/sandbox` flips it for the live session.
    pub sandbox_toggle: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// (`/sandbox` description) `SandboxRuntimeConfig.autoAllowBashIfSandboxed`
    /// — renders " (auto-allow)" in the dynamic `/sandbox` popup description.
    pub sandbox_desc_auto_allow: bool,
    /// (`/sandbox` description) `SandboxRuntimeConfig` unsandboxed-commands-allowed
    /// — renders ", fallback allowed" in the dynamic `/sandbox` description.
    pub sandbox_desc_fallback: bool,
    /// (`/sandbox` description) `checkDependencies().errors.length === 0` — when
    /// `false`, the dynamic `/sandbox` description shows the warning glyph.
    pub sandbox_desc_deps_ok: bool,
    /// (`/rewind`) The shared file-history checkpoint store. The SAME
    /// `Arc<session::FileHistory>` the orchestrator captures into; the CLI uses
    /// it to build the `/rewind` picker rows and to restore code on rewind.
    pub file_history: std::sync::Arc<session::FileHistory>,
    /// (`/reload-plugins`) The retained plugin subsystem, or `None` when plugins
    /// are disabled for the session. The CLI threads it into the TUI mount so the
    /// interactive `/reload-plugins` command applies pending enable/disable
    /// changes to the live session (see [`PluginRuntime::refresh`]).
    pub plugin_runtime: Option<std::sync::Arc<PluginRuntime>>,
    /// Phase 2a §6.2: per-`profile_name` availability flag driving the `/model`
    /// picker's Connect badge (a sibling map, NOT a field on the frozen
    /// `ModelListing`). The tui joins it by provider/profile name.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
    /// Set when the boot-time connected-provider fallback rerouted the session
    /// default model (its configured provider was definitively disconnected).
    /// Hosts surface it: the CLI prints a stderr notice pre-alt-screen; the
    /// bridge relies on the `tracing::warn!` `build()` already emitted. `None`
    /// ⟶ the configured default booted unchanged.
    pub default_model_fallback: Option<DefaultModelFallbackNotice>,
    /// (T2a) Per-provider login method tag, keyed by profile_name, derived from
    /// the real catalog auth strategy: "api_key" | "copilot_device" | "oauth".
    /// Threaded into the TUI so the /connect picker shows the real method.
    pub provider_auth_methods: std::collections::BTreeMap<String, String>,
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
    /// Shared HTTP transport for TUI-owned client-side WebSearch test runs.
    pub http: Arc<dyn traits::HttpTransport>,
    /// Structured-output capture slot — `Some` only when `--json-schema` is set
    /// (`DesktopConfig.json_schema`). The forced `StructuredOutput` tool writes
    /// the model's result here; the print path reads it after each turn to
    /// validate against the schema and retry. `None` for every normal run.
    pub structured_output_slot: Option<orchestrator::structured_output::StructuredOutputSlot>,
    /// `/loop` dynamic-mode (Phase 2): the set-once cell for the registered
    /// `ScheduleWakeup` tool. Empty at build time (the per-connection queue +
    /// spawner don't exist yet); the bridge composition root fills it at
    /// `boot::assemble` with a `MsgQueueWakeupScheduler`. Hosts without a
    /// per-connection queue (CLI / offline) leave it empty → the tool is an
    /// honest no-op.
    pub wakeup_scheduler_cell: tool_cron::WakeupSchedulerCell,
    /// A `RuntimeSpawner` for host-side background wiring that needs one after
    /// `build` (today: the bridge's `MsgQueueWakeupScheduler`, which sleeps then
    /// enqueues a `/loop` self-wakeup). A fresh stateless `PosixRuntime` — the
    /// same seam every in-`build` spawner uses (D17: never a direct
    /// `tokio::spawn`).
    pub runtime_spawner: Arc<dyn traits::RuntimeSpawner>,
    /// (`!` bash mode) The sandboxed Bash runner for the TUI's `!command` path,
    /// built over the SAME `BuiltinToolContext` (sandbox runner + runtime config)
    /// the model's `Bash` tool uses. The CLI threads it into the TUI `Runtime`
    /// (`Runtime::with_bash_runner`) so a typed `!ls` runs sandboxed and renders
    /// inline with no LLM turn — never a raw process.
    pub bash_runner: Arc<dyn tui_core::bash_runner::BashRunner>,
    /// (#3 shell-expansion) The shared prompt shell-expansion provider, built
    /// over the SAME `BuiltinToolContext` the dispatcher + Bash tool use. The CLI
    /// threads a clone into `apps/cli`'s `Runtime.shell_expansion` → the ratatui
    /// TUI's `ChatWidget`, so a typed `/commit` expands its embedded `!`git …``
    /// bodies through the real host runner + policy-backed gate before submit —
    /// the same expansion the dispatcher performs for non-TUI hosts.
    pub shell_expansion: Arc<dyn command_api::ShellExpansionProvider>,
    /// (`/connect` Copilot device-flow) The GitHub-Copilot OAuth device-flow
    /// driver (`EngineCopilotConnect` over `PosixHttp`). The CLI threads a clone
    /// into `tui::session::Runtime::with_copilot_connect_driver` so picking
    /// GitHub Copilot in `/connect` runs the real web sign-in (browser open +
    /// device-code poll + token store) instead of an inert key field. Also
    /// registered in the engine `/connect` command group (same Arc).
    pub connect_copilot: Arc<dyn command_core::CopilotConnectDriver>,
    /// (T2b) Unified OAuth sign-in driver for the TUI `/connect` picker. Drives
    /// the browser flow for the first-party OAuth providers (Anthropic Pro/Max,
    /// OpenAI ChatGPT) — replacing the honest-but-inert `Unavailable` screen.
    pub oauth_connect_driver: Arc<dyn command_core::OAuthConnectDriver>,
    /// (P1-08 runtime `/add-dir`) The SAME `Arc<SessionCwd>` the file tools gate
    /// on. The CLI `/add-dir` effect calls `add_trusted_dir(...)` on it so a
    /// directory added mid-session is immediately accessible to
    /// Read/Edit/Write/Glob/Grep/NotebookEdit without a reboot.
    pub session_cwd: Arc<SessionCwd>,
    /// (P1-08 runtime `/add-dir`) The live MCP registry. The CLI `/add-dir`
    /// effect calls `add_root(...)` + `notify_roots_list_changed_all()` on it so
    /// every connected server's `roots/list` reflects the new working directory.
    pub mcp_registry: Arc<mcp::McpRegistry>,
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
    /// Custom beta headers were requested for a route/auth mode that cannot
    /// safely carry Anthropic first-party API-key beta headers.
    #[error("custom betas require a first-party Anthropic API-key session")]
    InvalidCustomBetas,
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
    /// `cfg.worktree_launch` was `Some` (the user passed `-w`/`--worktree`)
    /// but the requested worktree could not be created (invalid slug, no git
    /// repo, git failure, ...). A HARD boot failure — worktree-tmux-launch
    /// plan Task 3: the user explicitly asked for an isolated worktree, so a
    /// silent fall-through to the plain cwd would be a surprising, unrequested
    /// downgrade rather than a recoverable default.
    #[error("--worktree launch failed: {0}")]
    WorktreeLaunch(String),
    /// `cfg.tmux_launch` was `Some` (the user passed `--tmux`) while
    /// `cfg.worktree_launch` was `None` (no `-w`/`--worktree`). Mirrors the CLI's
    /// own `--tmux` doc ("Create a tmux session for the worktree (requires
    /// --worktree)") as a hard boot failure rather than silently ignoring the
    /// flag — worktree-tmux-launch plan Task 4.
    #[error("--tmux requires --worktree")]
    TmuxRequiresWorktree,
    /// Bare `--tmux` (the "native" mode, `tmux_launch == Some("")`) was passed
    /// on Windows. 206's native pre-flight rejects it (`Ut()==="windows" →
    /// "--tmux is not supported on Windows"`, binary @230041975). `--tmux=classic`
    /// skips this native pre-check.
    #[error("--tmux is not supported on Windows")]
    TmuxNotSupportedOnWindows,
    /// Bare `--tmux` (native mode) was passed but `tmux` is not installed
    /// (`tmux -V` non-zero). 206's native pre-flight rejects it (`!await i4i()
    /// → "tmux is not installed.\n" + s4i()`, binary @230041975). The payload is
    /// the platform-specific install hint (`s4i()`). `--tmux=classic` skips this
    /// native pre-check (a missing tmux then degrades to the non-fatal
    /// create-session warning).
    #[error("tmux is not installed.\n{0}")]
    TmuxNotInstalled(String),
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
/// `isAnthropicAuthEnabled()` reduces to "the auth resolver picks the stored
/// OAuth session" — `resolve` is driven with the FULL
/// [`llm_client::oauth::anthropic::resolver::ResolverContext`] (M13), so every
/// documented ranking applies: managed OAuth forcing outranks env keys, env
/// `ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY` and an FD-inherited key outrank
/// stored OAuth, and stored OAuth outranks the stored/settings/helper/Bedrock
/// keys. When OAuth is the effective source, `shouldUseClaudeAIAuth(scopes)`
/// (== presence of the `user:inference` scope, via
/// `llm_client::oauth::anthropic::subscription_from_scopes`) decides.
fn oauth_subscriber_flag(
    source: &llm_client::oauth::anthropic::resolver::AuthSource,
    scopes: &[String],
) -> bool {
    matches!(
        source,
        llm_client::oauth::anthropic::resolver::AuthSource::OAuthClaudeAi
    ) && llm_client::oauth::anthropic::subscription_from_scopes(scopes)
}

/// Seed of the shared subscription slot for a session that holds a stored
/// Claude.ai credential.
///
/// The tier persisted inside the credential is readable through
/// `getSubscriptionType()` (`Aa()` @228959617) and `getRateLimitTier()` (`jW()`,
/// same region), and BOTH short-circuit to `null` unless
/// `isAnthropicAuthEnabled()` (`zb()` @228933355) holds — i.e. a leftover stored
/// blob under an env key / bearer / FD key reports NO tier at all. So the tier
/// is gated on the RESOLVER's pick alone: `Aa()` does NOT additionally require
/// the `user:inference` scope that `isClaudeAISubscriber` folds into
/// [`oauth_subscriber_flag`], so an inference-less OAuth session still reports
/// its tier while `is_subscriber` is false.
fn subscription_seed(
    source: &llm_client::oauth::anthropic::resolver::AuthSource,
    scopes: &[String],
    subscription_type: Option<&String>,
    rate_limit_tier: Option<&String>,
) -> traits::subscription::SubscriptionSnapshot {
    let oauth_effective = matches!(
        source,
        llm_client::oauth::anthropic::resolver::AuthSource::OAuthClaudeAi
    );
    traits::subscription::SubscriptionSnapshot {
        is_subscriber: oauth_subscriber_flag(source, scopes),
        subscription_type: oauth_effective
            .then(|| subscription_type.cloned())
            .flatten(),
        rate_limit_tier: oauth_effective.then(|| rate_limit_tier.cloned()).flatten(),
        ..Default::default()
    }
}

/// Fold the profile + roles responses into the shared snapshot. Pure —
/// unit-tested without IO. Tier mapping mirrors
/// `OAuthProfileResponse::subscription_type()` (TS string union values);
/// `Free`/`Unknown` resolve to `None` (conservative, same as the TS `null`;
/// note `subscription_type()` never actually returns those variants today, so
/// that arm is purely defensive).
fn subscription_snapshot_from(
    is_subscriber: bool,
    profile: Option<&llm_client::oauth::anthropic::OAuthProfileResponse>,
    roles: Option<&llm_client::oauth::anthropic::UserRolesResponse>,
) -> traits::subscription::SubscriptionSnapshot {
    use llm_client::oauth::anthropic::SubscriptionType;
    let org = profile.and_then(|p| p.organization.as_ref());
    let subscription_type = profile
        .and_then(llm_client::oauth::anthropic::OAuthProfileResponse::subscription_type)
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
        "claude-opus-4-8".to_string(),
        "claude-opus-4-6".to_string(),
        "claude-opus-4-5-20251101".to_string(),
        "claude-opus-4-1-20250805".to_string(),
        "claude-opus-4-20250514".to_string(),
        // Sonnet 5 — the 2.1.198 default first-party model.
        "claude-sonnet-5".to_string(),
        "claude-sonnet-4-6".to_string(),
        "claude-sonnet-4-5-20250929".to_string(),
        "claude-haiku-4-5".to_string(),
        // Fable 5 — the latest fast Claude; surfaced in the curated `/model`
        // picker's Anthropic group alongside Sonnet/Opus/Haiku.
        "claude-fable-5".to_string(),
    ];
    // Register the configured default/fallback under the ANTHROPIC profile ONLY
    // when it actually ROUTES to anthropic (a `claude-*` id, an unqualified
    // custom id, or an `anthropic/…` ref). A ref qualified for ANOTHER provider
    // (`openrouter/…`, `github-copilot/…`, `deepseek/…`) must NOT be added here:
    // doing so put e.g. `meta-llama/llama-3.3-70b-instruct:free` into BOTH the
    // anthropic AND openrouter model lists, so `--model <that>` failed with a
    // spurious "ambiguous across profiles: anthropic, openrouter". Push the BARE
    // model (so `anthropic/claude-x` registers as `claude-x`, not the qualified
    // ref). `split_profile_model` is the canonical routing split.
    let fallback_models = fallback_model
        .into_iter()
        .flat_map(|csv| csv.split(','))
        .map(str::trim)
        .filter(|m| !m.is_empty());
    for m in std::iter::once(default_model).chain(fallback_models) {
        let (profile, bare) = llm_client::split_profile_model(m);
        if profile == "anthropic" {
            ids.push(bare);
        }
    }
    // Env-configured small-fast / haiku model a `prompt` hook may resolve to
    // (matching `hook_prompt_runner::resolve_model`'s precedence:
    // `ANTHROPIC_SMALL_FAST_MODEL` > `ANTHROPIC_DEFAULT_HAIKU_MODEL` > default
    // Haiku), so such a request resolves instead of failing `ModelUnavailable`.
    // The default Haiku id (`claude-haiku-4-5`) is already in the list above.
    for var in [
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    ] {
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
            description: None,
            capabilities: caps,
        })
        .collect()
}

/// One `settings.recentModels` entry threaded in by the host (the CLI reads the
/// file; F2-01 keeps `build()` off the filesystem for host config): a prior
/// `/model` pick, most-recent-first. `provider` is the catalog profile name;
/// `model` is the BARE wire `request_model` (the on-disk schema splits them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentModelRef {
    /// Catalog profile name (`ModelListing::provider_id`).
    pub provider: String,
    /// Bare wire model id (`ModelListing::request_model`).
    pub model: String,
}

/// Host-facing notice that the boot-time connected-provider fallback rerouted
/// the session default model. Surfaced on [`DesktopRuntime`]; both refs are in
/// display form (`profile/model`-qualified for non-anthropic routes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultModelFallbackNotice {
    /// The configured default that did NOT boot (verbatim as configured).
    pub from: String,
    /// The connected-provider model the session booted on instead.
    pub to: String,
}

/// Outcome of [`connected_provider_fallback`]: what the session boots on
/// instead of the configured (disconnected-provider) default model.
struct DefaultModelFallback {
    /// Bare wire id of the fallback model.
    model: String,
    /// Provider profile the fallback routes to — ALWAYS set (anthropic
    /// included), so the `switch_model` seeding at the end of `build()` scopes
    /// the session and a wire id that exists under several providers (e.g.
    /// `claude-fable-5` on anthropic AND github-copilot) resolves
    /// unambiguously instead of failing every turn with "ambiguous across
    /// profiles".
    profile: String,
}

/// Boot-time connected-provider default-model fallback (`LingXi` multi-provider
/// divergence — upstream claude-code is Anthropic-only and has no analog):
/// when the configured default model's provider is DEFINITIVELY disconnected
/// (`availability[provider] == false`; an ABSENT entry means the probe is
/// blind to that provider, so the default is conservatively kept) and at least
/// one other provider IS connected, boot on that provider instead of into
/// guaranteed first-turn auth failures.
///
/// Preference: (1) the most recent `/model` pick (`settings.recentModels`) on
/// a connected provider whose model still exists in the catalog; (2) the first
/// connected provider in [`traits::provider_fallback_order`], on its
/// [`traits::provider_default_model`]; (3) any remaining connected provider
/// (user-defined — no curated default), on its first listed model. Every
/// candidate is validated against the live `listings` so the reroute can never
/// select an id `switch_model`/the wire would reject.
///
/// `anthropic_probe_definitive` is `false` on gateway installs (a custom
/// `api_base` / `ANTHROPIC_AUTH_TOKEN` serves Claude WITHOUT a local key or
/// OAuth): there `availability["anthropic"] == false` is probe-blindness, not
/// disconnection — an anthropic-routed default is then kept as-is (the same
/// protection the TUI `/model` picker's `connected_model_rows` gives the
/// current model's provider).
fn connected_provider_fallback(
    default_model_id: &str,
    default_model_profile: Option<&str>,
    anthropic_probe_definitive: bool,
    model_providers: &std::collections::BTreeMap<String, (String, String)>,
    availability: &std::collections::BTreeMap<String, bool>,
    listings: &[traits::ModelListing],
    recents: &[RecentModelRef],
) -> Option<DefaultModelFallback> {
    // Effective provider of the configured default — the same resolution the
    // session_provider_first_party gate uses (explicit profile, else the
    // model_providers grouping, else the native anthropic route).
    let default_provider = default_model_profile
        .map(str::to_string)
        .or_else(|| {
            model_providers
                .get(default_model_id)
                .map(|(p, _)| p.clone())
        })
        .unwrap_or_else(|| "anthropic".to_string());
    if default_provider == "anthropic" && !anthropic_probe_definitive {
        return None; // gateway/auth-override install — the probe can't see its auth
    }
    if availability.get(default_provider.as_str()) != Some(&false) {
        return None; // connected — or the probe doesn't know this provider
    }
    let connected = |p: &str| availability.get(p) == Some(&true);
    let in_listings = |p: &str, m: &str| {
        listings
            .iter()
            .any(|l| l.provider_id == p && l.request_model == m)
    };
    let route = |model: String, provider: &str| DefaultModelFallback {
        model,
        profile: provider.to_string(),
    };
    // (1) The most recent /model pick on a connected provider.
    for r in recents {
        if connected(&r.provider) && in_listings(&r.provider, &r.model) {
            return Some(route(r.model.clone(), &r.provider));
        }
    }
    // (2) Deterministic provider order, each on its curated boot default.
    for p in traits::provider_fallback_order() {
        if !connected(p) {
            continue;
        }
        if let Some(m) = traits::provider_default_model(p) {
            if in_listings(p, m) {
                return Some(route(m.to_string(), p));
            }
        }
    }
    // (3) Any remaining connected provider (user-defined): first listed model.
    for (p, on) in availability {
        if !on {
            continue;
        }
        if let Some(l) = listings.iter().find(|l| &l.provider_id == p) {
            return Some(route(l.request_model.clone(), p));
        }
    }
    None
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

/// Load the merged `settings.showThinkingSummaries` request-beta preference.
/// Absent/invalid settings resolve to Claude Code's default (`false`).
fn load_merged_show_thinking_summaries(project_dir: &std::path::Path) -> bool {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.show_thinking_summaries)
        .unwrap_or(false)
}

/// Load the merged `settings.skipWebFetchPreflight` (project + user + env layers)
/// for the given project dir. Mirrors [`load_merged_output_style`] (same
/// `engine::settings::Settings::load` seam). When true, the `WebFetch` tool skips
/// the domain-blocklist preflight (CC 2.1.207 `!Mi().skipWebFetchPreflight` gate,
/// parity P2-14). Returns `false` on any load failure or when the key is unset —
/// the frozen default (preflight runs).
fn load_merged_skip_web_fetch_preflight(project_dir: &std::path::Path) -> bool {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.skip_web_fetch_preflight)
        .unwrap_or(false)
}

/// Load the merged `settings.disableAgentView` (project + user + env layers) for
/// the given project dir. Mirrors [`load_merged_skip_web_fetch_preflight`] (same
/// `engine::settings::Settings::load` seam). When `true`, the agent-view
/// fork/subtask surface is disabled exactly like `CLAUDE_CODE_DISABLE_AGENT_VIEW=1`
/// (binary `I2i()` — `settings.disableAgentView === true`), threaded into
/// [`command_core::register_core_batch_8`] via
/// [`traits::agent_view::is_enabled_with_setting`] (M-03). Returns `false` on any
/// load failure or when the key is unset — the frozen default (agent view
/// enabled; the env half still applies independently).
fn load_merged_disable_agent_view(project_dir: &std::path::Path) -> bool {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.disable_agent_view)
        .unwrap_or(false)
}

/// Resolve the merged hooks-restricted flag for the `/goal` gate (review #12):
/// `disableAllHooks || allowManagedHooksOnly` across the project/user/env layers
/// (the same `Settings::load` seam). Mirrors claude's `kEt` hooks half —
/// `if (tX() || lMe()) return hooks_gate`. `false` on any load failure or when
/// both keys are unset (the permissive default).
fn load_merged_hooks_restricted(project_dir: &std::path::Path) -> bool {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .map(|eff| {
            eff.settings.disable_all_hooks.unwrap_or(false)
                || eff.settings.allow_managed_hooks_only.unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Load the merged `askUserQuestionTimeout` (`60s`/`5m`/`10m`/`never`) across the
/// project + user + env settings layers (the same `Settings::load` seam). The raw
/// settings string is threaded into `BuiltinToolContext::ask_user_question_timeout`
/// and parsed into `tool_ui::ask_user_question::AskUserQuestionTimeout` at tool
/// registration (M-15). Returns `None` on any load failure or when the key is
/// unset — the frozen default (`never` ⇒ block on the user, no auto-continue).
fn load_merged_ask_user_question_timeout(project_dir: &std::path::Path) -> Option<String> {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.ask_user_question_timeout)
}

/// Load the merged HTTP-hook security policy (H-BIN-12) — `allowedHttpHookUrls`
/// and `httpHookAllowedEnvVars` — across the project + user + env settings
/// layers. Both are array-merge (concat-dedup) via the same
/// `engine::settings::Settings::load` seam. `(None, None)` on any load failure or
/// when neither key is set (⇒ no restriction; the HTTP hook executor behaves
/// exactly as before). Threaded into the executor via
/// [`hooks::HookExecutorImpl::with_http_hook_policy`], mirroring CC's live
/// `PFy()=Wn()` read (lingxi sources once at boot).
fn load_merged_http_hook_policy(
    project_dir: &std::path::Path,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    match engine::settings::Settings::load(inputs) {
        Ok(eff) => (
            eff.settings.allowed_http_hook_urls,
            eff.settings.http_hook_allowed_env_vars,
        ),
        Err(_) => (None, None),
    }
}

/// (P2-02 cc2.1.207) `g9e(source)` — is the agent source in the trusted set
/// `qXh`? The binary's set is
/// `new Set(["plugin","policySettings","built-in","builtin","bundled"])`, so a
/// trusted source bypasses a `strictPluginOnlyCustomization` (`uA`) restriction
/// when registering the agent's frontmatter hooks as `mainThreadAgentHooks`
/// (`Rft`). LingXi's [`agent::AgentSource`] maps (see
/// `agent::handle::agent_source_to_claude_str`): `BuiltIn`→"built-in",
/// `Plugin`→"plugin", `PolicySettings`→"policySettings" are the trusted three;
/// `UserDefined`/`Project`/`Flag` are NOT. (LingXi has no "bundled" source.)
fn agent_source_is_trusted(source: agent::AgentSource) -> bool {
    matches!(
        source,
        agent::AgentSource::BuiltIn
            | agent::AgentSource::Plugin
            | agent::AgentSource::PolicySettings
    )
}

/// (M4 cc2.1.198) Merge the `--agents <json>` flag agents into the dir-loaded
/// catalog. The flag payload is an EXPLICIT request: it survives `--bare` but
/// not safe mode (binary @223080769 `if(r&&!Hc("agents",{explicitlyRequested:
/// !0}))try{let g=Ba(r);if(g)m=QXt(g,"flagSettings")}catch(g){De(g)}else
/// if(r)C("--agents: ignored in safe mode (user-supplied custom agents are
/// disabled)",{level:"warn"})`). Merge precedence per `XXt`'s tier map
/// `[built-in, plugin, userSettings, projectSettings, flagSettings,
/// policySettings]` (later wins): a flag agent REPLACES a same-named
/// user/project dir agent, else appends. Parse failures inside
/// [`agent::parse_agents_from_flag_json`] log and contribute no agents —
/// the flag never aborts boot.
fn merge_cli_flag_agents(
    agents: &mut Vec<agent::AgentDefinition>,
    cli_agents_json: Option<&str>,
    safe_mode: bool,
) {
    let Some(raw) = cli_agents_json else { return };
    if safe_mode {
        tracing::warn!("--agents: ignored in safe mode (user-supplied custom agents are disabled)");
        return;
    }
    for a in agent::parse_agents_from_flag_json(raw) {
        if let Some(slot) = agents.iter_mut().find(|e| e.agent_type == a.agent_type) {
            *slot = a;
        } else {
            agents.push(a);
        }
    }
}

/// (M7 cc2.1.220) Boot gates for [`merge_agent_frontmatter_mcp_servers`],
/// resolved by the composition root (env/flag safe mode, `--strict-mcp-config`,
/// `managed-mcp.json` presence) and injected so the merge is a pure,
/// unit-testable function.
#[derive(Debug, Clone, Copy)]
struct AgentMcpMergeGates {
    /// claude `Gl()` — `CLAUDE_CODE_SAFE_MODE` env truthy or `--safe-mode`.
    safe_mode: bool,
    /// claude `r?.strictMcpConfig` — the `--strict-mcp-config` CLI flag.
    strict_mcp_config: bool,
    /// claude `T3()` — a managed `managed-mcp.json` takes EXCLUSIVE control of
    /// the MCP server set; agent frontmatter servers never merge.
    enterprise_mcp_active: bool,
}

/// (M7 cc2.1.220) claude `FWt(existing, agentDef, opts)` @245974724 — merge the
/// resolved main-thread agent's frontmatter `mcpServers` into the to-connect
/// MCP config list, so they register + connect exactly like `--mcp-config`
/// servers. Returns the enterprise-BLOCKED server names for the caller's
/// `onBlocked` stderr warning (only the composition root prints — claude's
/// TUI/resume `FWt` call sites pass no `onBlocked`).
///
/// Gate order, byte-faithful to `FWt`:
/// 1. no agent definition → no-op (`if(!t)return e`);
/// 2. safe mode → no-op (`if(Gl())return e`);
/// 3. `--strict-mcp-config` UNLESS the agent came from `--agents`
///    (`r?.strictMcpConfig && t.source !== "flagSettings"`), OR a managed MCP
///    config is active (`|| T3()`) → no-op;
/// 4. convert via `obs` ([`agent::agent_mcp_specs_to_scoped_configs`]);
/// 5. `Yee` enterprise allow/deny filter (sdk-type always allowed) → blocked
///    names collected;
/// 6. `{...allowed, ...existing}` — an EXISTING same-name server wins; agent
///    servers only fill gaps.
fn merge_agent_frontmatter_mcp_servers(
    existing: &mut Vec<mcp::McpServerConfig>,
    def: Option<&agent::AgentDefinition>,
    gates: AgentMcpMergeGates,
    policy: &mcp::enterprise_policy::McpPolicy,
) -> Vec<String> {
    let Some(def) = def else {
        return Vec::new();
    };
    if gates.safe_mode {
        return Vec::new();
    }
    if (gates.strict_mcp_config && def.source != agent::AgentSource::Flag)
        || gates.enterprise_mcp_active
    {
        return Vec::new();
    }
    // `Y0("mcp")` strictPluginOnlyCustomization: the composition root never
    // populates the strict policy today (`StrictPluginOnlyPolicy::empty()`
    // above), so the lock is always open — pass `false`; the gate itself lives
    // inside the conversion for 1:1 structure.
    let scoped = agent::agent_mcp_specs_to_scoped_configs(def, false);
    let mut blocked = Vec::new();
    for cfg in scoped {
        // `Yee` — enterprise allow/deny per server (sdk short-circuit inside).
        if !mcp::enterprise_policy::is_server_allowed(&cfg, policy) {
            blocked.push(cfg.name);
            continue;
        }
        if !existing.iter().any(|x| x.name == cfg.name) {
            existing.push(cfg);
        }
    }
    blocked
}

/// Read the merged `settings.enabledPlugins` allowlist (`plugin@marketplace` →
/// enabled) from the user then project `settings.json`, project last so it wins
/// on conflict. Mirrors `loadPluginsFromMarketplaces`'s
/// `{...getAddDirEnabledPlugins(), ...settings.enabledPlugins}` merge
/// (`pluginLoader.ts:1898`) at the priority that matters for the cache-only
/// boot. Malformed files / a missing key degrade to an empty map (no plugins),
/// matching claude-code's resilient read-only boot.
async fn load_enabled_plugins(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
) -> std::collections::BTreeMap<String, bool> {
    let mut merged: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
    let user = lingxi_home.join("settings.json");
    let project = cwd.join(branding::DOT_DIR).join("settings.json");
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

/// Read the merged `settings.pluginConfigs` scope (`plugin → {options,
/// mcpServers}`) from the user settings and managed policy tiers only. Project
/// / local settings are intentionally ignored: cloned repositories must not be
/// able to feed `${user_config.*}` substitutions. This is the composition-root
/// READ that seeds [`plugin::PluginManager::with_plugin_configs`]; without it
/// the manager's `plugin_configs` is always empty and non-sensitive options
/// from settings.json never reach `resolve_user_config`. Malformed files / a
/// missing key degrade to an empty map (no persisted config), matching the
/// resilient read-only boot.
async fn load_plugin_configs(
    lingxi_home: &std::path::Path,
) -> std::collections::HashMap<String, plugin::PluginUserConfig> {
    let mut merged: std::collections::HashMap<String, plugin::PluginUserConfig> =
        std::collections::HashMap::new();
    let user = lingxi_home.join("settings.json");
    if let Ok(raw) = tokio::fs::read_to_string(&user).await {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw)
        {
            for (plugin, cfg) in plugin::PluginUserConfig::from_settings_map(&map) {
                merged.insert(plugin, cfg);
            }
        }
    }
    for raw in crate::settings_watch::managed_settings_raw_tiers().await {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw)
        else {
            continue;
        };
        for (plugin, cfg) in plugin::PluginUserConfig::from_settings_map(&map) {
            merged.insert(plugin, cfg);
        }
    }
    merged
}

/// Read the managed-only blocked marketplace policy (`blockedMarketplaces`),
/// last-write-wins across the managed tiers.
async fn load_blocked_marketplaces() -> std::collections::HashSet<String> {
    let mut blocked = std::collections::HashSet::new();
    for raw in crate::settings_watch::managed_settings_raw_tiers().await {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some(entries) = value.get("blockedMarketplaces").and_then(|v| v.as_array()) else {
            continue;
        };
        blocked = entries
            .iter()
            .filter_map(|entry| entry.as_str())
            .filter(|entry| !entry.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    blocked
}

/// Discover the set of plugins that should be active for the current session —
/// the shared body of both the startup bootstrap (§6.5) and the
/// `/reload-plugins` refresh ([`PluginRuntime::refresh`]). `ambient` resolves
/// the `enabledPlugins` allowlist against the on-disk plugin cache (with a
/// flat-walk fallback for dev/local dirs); `inline` appends any `--plugin-dir`
/// session plugins. Returns `(id, manifest, install_dir)` per plugin — exactly
/// what [`plugin::PluginManager::enable`] consumes.
async fn discover_plugin_set(
    ambient: bool,
    inline: bool,
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
    plugins_dir: &std::path::Path,
    cli_plugin_dirs: &[std::path::PathBuf],
) -> Vec<(
    protocol::PluginId,
    plugin::PluginManifest,
    std::path::PathBuf,
)> {
    let mut discovered = if ambient {
        let enabled = load_enabled_plugins(lingxi_home, cwd).await;
        let mut d = plugin::discover_enabled_plugins(plugins_dir, &enabled).await;
        // Fallback: no allowlist match ⇒ flat-walk for direct plugin dirs.
        if d.is_empty() {
            d = plugin::discover_installed_plugins(plugins_dir).await;
        }
        d
    } else {
        Vec::new()
    };
    if inline {
        discovered.extend(plugin::discover_cli_plugin_dirs(cli_plugin_dirs).await);
    }
    discovered
}

/// Component tallies reported by [`PluginRuntime::refresh`], mirroring
/// claude-code's `RefreshActivePluginsResult` (`utils/plugins/refresh.ts`). The
/// CLI formats these into the `/reload-plugins` confirmation line.
#[derive(Debug, Default, Clone, Copy)]
pub struct PluginRefreshCounts {
    /// Plugins now live in the session (successfully enabled).
    pub enabled: usize,
    /// Slash-commands (claude-code labels these "skills") across enabled plugins.
    pub commands: usize,
    /// Agents contributed by enabled plugins.
    pub agents: usize,
    /// Hook matchers across enabled plugins.
    pub hooks: usize,
    /// Plugin MCP servers across enabled plugins.
    pub mcp: usize,
    /// Plugin LSP servers across enabled plugins.
    pub lsp: usize,
    /// Plugins that failed to (re)load during the refresh.
    pub errors: usize,
}

/// (`/reload-plugins`) The live plugin subsystem, retained past startup so the
/// interactive `/reload-plugins` command can apply pending enable/disable
/// changes to the RUNNING session without a restart — claude-code's
/// `refreshActivePlugins` (Layer-3 refresh). Holds the SAME [`plugin::PluginManager`]
/// the startup bootstrap materialised through (its registries are the shared
/// `Arc`s the orchestrator reads), plus the discovery ingredients, so `refresh`
/// re-reads `enabledPlugins` off disk and diffs it against what is loaded:
/// `disable()` for plugins turned off (drops their commands/hooks/MCP/LSP from
/// the live registries), `enable()` for newly-on ones (re-materialises +
/// live-dials MCP), and a wholesale rebuild of the plugin-agent catalog portion.
pub struct PluginRuntime {
    manager: Arc<plugin::PluginManager>,
    plugins_dir: std::path::PathBuf,
    home: std::path::PathBuf,
    cwd: std::path::PathBuf,
    cli_plugin_dirs: Vec<std::path::PathBuf>,
    ambient: bool,
    inline: bool,
}

impl PluginRuntime {
    /// Re-read the on-disk enabled set and reconcile it into the live session.
    /// Returns the component tallies for the confirmation message. Best-effort:
    /// a plugin that fails to enable is counted in `errors` and skipped; already
    /// live plugins are left untouched (no MCP reconnect churn).
    pub async fn refresh(&self) -> PluginRefreshCounts {
        self.manager
            .replace_plugin_configs(load_plugin_configs(&self.home).await)
            .await;
        self.manager
            .replace_blocked_marketplaces(load_blocked_marketplaces().await)
            .await;
        // (1) The fresh target set from disk + settings.
        let target = discover_plugin_set(
            self.ambient,
            self.inline,
            &self.home,
            &self.cwd,
            &self.plugins_dir,
            &self.cli_plugin_dirs,
        )
        .await;

        // (2) Unload every currently-loaded plugin. A reload re-reads the WHOLE
        //     enabled set from disk (claude-code `clearAllCaches` +
        //     `loadAllPlugins`), and `discover_plugin_set` mints a fresh
        //     `PluginId` per discovery (`load_plugin_from_path`), so the reloaded
        //     set never aliases the old ids — disabling all here, then enabling
        //     the fresh target below, is the full swap. This also picks up
        //     edited-in-place plugin files, matching cc's full reload.
        for id in self.manager.loaded_plugin_ids().await {
            let _ = self.manager.disable(&id).await;
        }

        // (3) Enable each target plugin. The manager validates agent privileges
        //     before materialising every component into the shared registries.
        let mut counts = PluginRefreshCounts::default();
        for (id, manifest, dir) in target {
            // Tally BEFORE `manifest` moves into `enable`.
            let c = &manifest.components;
            let this = (
                c.commands.len() + c.skills.len(),
                c.agents.len(),
                c.hooks.len(),
                c.mcp_servers.len(),
                c.lsp_servers.len(),
            );
            match self.manager.enable(&id, manifest, dir).await {
                Ok(()) => {
                    counts.enabled += 1;
                    counts.commands += this.0;
                    counts.agents += this.1;
                    counts.hooks += this.2;
                    counts.mcp += this.3;
                    counts.lsp += this.4;
                }
                Err(e) => {
                    counts.errors += 1;
                    tracing::warn!(error = %e, "/reload-plugins: plugin failed to load");
                }
            }
        }
        counts
    }
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
    async fn skill_entries(&self) -> Vec<orchestrator::prompt::skill_listing::SkillListingEntry> {
        use command_api::{CommandSource, SlashCommandKind};
        let reg = self.0.read().await;
        reg.model_invocable_commands() // !disable_model_invocation (registry.rs)
            .into_iter()
            // TS `cmd.type === 'prompt'` — markdown/plugin commands, not builtin/mcp.
            .filter(|c| {
                matches!(
                    c.kind,
                    SlashCommandKind::Markdown { .. }
                        | SlashCommandKind::Plugin { .. }
                        // Bundled programmatic skills (`/loop`) are model-invocable.
                        | SlashCommandKind::Bundled { .. }
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
fn lingxi_temp_dir_name() -> String {
    format!("claude-{}", current_uid())
}

/// Base Claude temp dir for the task spool — port of `getClaudeTempDir`
/// (`permissions/filesystem.ts:331-346`): `$LINGXI_TMPDIR || /tmp`, joined
/// with [`lingxi_temp_dir_name`]. (claude resolves symlinks; the spool path only
/// needs to be writable + session-unique, so the realpath step is omitted.)
/// Distinct from the sandbox-seed [`lingxi_temp_dir`] (which returns the
/// realpath-resolved, trailing-separator String form).
fn lingxi_temp_dir_path() -> std::path::PathBuf {
    let base = std::env::var_os("LINGXI_TMPDIR").map_or_else(
        || std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from,
    );
    base.join(lingxi_temp_dir_name())
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
/// Session-scoping (vs the old in-repo `<cwd>/.lingxi/tasks-output`) keeps
/// concurrent sessions in one project from clobbering each other's spools and
/// stops task output from polluting the working tree / git status. Rooting under
/// the project temp dir also makes reads auto-allowed by claude's
/// `checkReadableInternalPath`.
#[must_use]
pub fn session_task_output_dir(cwd: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    lingxi_temp_dir_path()
        .join(sanitize_path_component(&cwd.to_string_lossy()))
        .join(session_id)
        .join("tasks")
}

/// # Errors
///
/// Returns [`BuildError`] if the api-client or orchestrator cannot be
/// constructed (effectively infallible in the current wiring).
#[allow(clippy::too_many_lines)]
/// Forwards `llm-client`'s synchronous retry-status reports to the async
/// session [`OutputStream`] so the TUI can render "Retrying in Ns… (attempt
/// X/Y)" during a backoff. `report` runs inside the retry loop's tokio context,
/// so it spawns the async emit (fire-and-forget; retries are seconds apart).
struct OutputRetryReporter {
    output: Arc<dyn OutputStream>,
}

impl llm_client::RetryReporter for OutputRetryReporter {
    fn report(&self, info: llm_client::RetryInfo) {
        let output = self.output.clone();
        tokio::spawn(async move {
            output
                .emit_api_retry(&info.message, info.attempt, info.max_retries, info.delay_ms)
                .await;
        });
    }
}

/// (worktree-tmux-launch plan, Task 3) Apply `-w`/`--worktree [name]`'s boot
/// launch onto an already-constructed `ctx`: create a git worktree and swap
/// the session into it. Called exactly once, from [`build`] right after its
/// `tool_ctx` literal is complete (so `ctx.session_cwd` /
/// `ctx.worktree_session` / `ctx.worktree` are all wired) — extracted into its
/// own function so this exact sequence is unit-testable against a
/// `BuiltinToolContext` fixture without driving a full `build()`.
///
/// Mirrors `EnterWorktreeTool::call_create`'s create → swap → record sequence
/// (`tools/worktree/src/worktree.rs`) exactly, reusing its random-slug helper
/// (`gen_random_slug`, made `pub` for this) instead of duplicating it;
/// `create_worktree` itself runs the same `validate_worktree_slug` pre-flight
/// the tool path runs, so an invalid `--worktree <name>` surfaces the same
/// `WorktreeError::InvalidSlug` message, wrapped in [`BuildError::WorktreeLaunch`].
/// Unlike the tool (which can refuse "already in a worktree" / subagent-cwd-
/// override calls), boot starts from a fresh session with no prior worktree
/// and no subagent cwd override, so those tool-only guards do not apply here.
///
/// INERT INVARIANT: `worktree_launch == None` (the default, and every host
/// but a CLI session with `-w`/`--worktree` set) is a complete no-op — no
/// create, no swap, `ctx.worktree_session` untouched. `tmux_launch == None`
/// (the default) is independently inert — no tmux session is created and
/// `tmux_session_name` stays `None` — even when `worktree_launch` is `Some`.
///
/// (worktree-tmux-launch plan, Task 4) `tmux_launch: Some(_)` requires
/// `worktree_launch: Some(_)` (mirrors the CLI's own `--tmux` doc: "Create a
/// tmux session for the worktree (requires --worktree)"); `Some` tmux with
/// `None` worktree is a hard boot failure
/// ([`BuildError::TmuxRequiresWorktree`]), not a silent ignore. When both are
/// `Some`, AFTER the worktree above is created + swapped + recorded, this
/// derives the session name ([`platform_posix::worktree_tmux::worktree_tmux_session_name`],
/// keyed on the PRE-swap `original_cwd` as the repo root) and creates a
/// detached tmux session for it
/// ([`platform_posix::worktree_tmux::create_worktree_tmux_session`]) through
/// `ctx.process`/`ctx.sandbox`. A tmux failure is logged
/// (`tracing::warn!`) and does NOT fail boot — the worktree launch itself
/// already succeeded, and `WorktreeSession.tmux_session_name` simply stays
/// `None` — only a tmux SUCCESS writes the name into the shared
/// `ctx.worktree_session` cell.
async fn apply_worktree_launch(
    worktree_launch: &Option<String>,
    tmux_launch: &Option<String>,
    ctx: &BuiltinToolContext,
) -> Result<(), BuildError> {
    let Some(name_or_empty) = worktree_launch else {
        if tmux_launch.is_some() {
            return Err(BuildError::TmuxRequiresWorktree);
        }
        return Ok(());
    };
    let slug = if name_or_empty.is_empty() {
        tool_worktree::worktree::gen_random_slug()
    } else {
        name_or_empty.clone()
    };

    // Native-mode (`--tmux` with NO explicit value → `Some("")`) pre-flight,
    // byte-faithful to 206's `re = Dor() && a.tmux===!0` branch (@230041975):
    // bare `--tmux` hard-checks not-Windows + tmux-installed BEFORE creating
    // the worktree. `--tmux=classic` (any explicit value) is NOT native and
    // skips these — a missing tmux then degrades to the non-fatal
    // create-session warning below. (`--tmux requires --worktree` is enforced
    // for BOTH modes by the `worktree_launch == None` guard above; that is a
    // deliberate, safe superset of 206, which only checks it for native.)
    if tmux_launch.as_deref() == Some("") {
        if cfg!(windows) {
            return Err(BuildError::TmuxNotSupportedOnWindows);
        }
        if !platform_posix::worktree_tmux::tmux_is_installed(
            ctx.process.as_ref(),
            ctx.sandbox.as_ref(),
        )
        .await
        {
            return Err(BuildError::TmuxNotInstalled(
                platform_posix::worktree_tmux::tmux_install_hint().to_string(),
            ));
        }
    }

    // Captured BEFORE the swap below — the pre-launch boot cwd, which
    // `ExitWorktree` later restores (same contract as
    // `EnterWorktreeTool::record_worktree_session`), and (Task 4) the repo
    // root the tmux session name is derived from.
    let original_cwd = ctx.session_cwd.cwd();
    let handle = ctx
        .worktree
        .create_worktree(&slug, None, &[])
        .await
        .map_err(|e| BuildError::WorktreeLaunch(e.to_string()))?;
    let worktree_path = handle.path.clone();
    ctx.session_cwd
        .swap(handle.path.clone(), vec![handle.path.clone()]);
    *ctx.worktree_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tool_api::WorktreeSession {
        original_cwd: original_cwd.clone(),
        worktree_path: handle.path,
        branch_name: handle.branch_name,
        base_commit: handle.base_commit,
        // This session CREATED the worktree (never entered a pre-existing
        // one), so `ExitWorktree` may remove it — same as
        // `EnterWorktreeTool::call_create`'s `entered_existing: false`.
        entered_existing: false,
        // Populated below (Task 4) when `tmux_launch.is_some()` AND the tmux
        // session actually gets created; `None` otherwise.
        tmux_session_name: None,
    });

    if tmux_launch.is_some() {
        let session_name =
            platform_posix::worktree_tmux::worktree_tmux_session_name(&original_cwd, &slug);
        match platform_posix::worktree_tmux::create_worktree_tmux_session(
            ctx.process.as_ref(),
            ctx.sandbox.as_ref(),
            &session_name,
            &worktree_path,
        )
        .await
        {
            Ok(()) => {
                // 206's CLI worktree-launch prints the session name + attach
                // hint on success (@225872424, `console.log("Created tmux
                // session: {S}\nTo attach: tmux attach -t {S}")`) so the user
                // can find it — without this the derived name is invisible.
                // Emitted on STDERR (not 206's stdout) so it never pollutes
                // `--print`/stream-json stdout; this follows the port's boot-
                // notice precedent (the settings-warning `eprintln!` in
                // `build`). Colorization (206 `ht.green`) is dropped.
                eprintln!("Created tmux session: {session_name}\nTo attach: tmux attach -t {session_name}");
                if let Some(session) = ctx
                    .worktree_session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_mut()
                {
                    session.tmux_session_name = Some(session_name);
                }
            }
            Err(e) => {
                // Non-fatal: the worktree itself was already created+entered
                // above, so a tmux hiccup must not fail boot — it only means
                // `WorktreeSession.tmux_session_name` stays `None`. 206 also
                // surfaces this to the user (@225872... `console.error("Warning:
                // Failed to create tmux session: {error}")`), so print it (on
                // stderr, matching 206's `console.error`) in addition to the
                // structured `tracing::warn!`.
                eprintln!("Warning: Failed to create tmux session: {e}");
                tracing::warn!(
                    error = %e,
                    session_name = %session_name,
                    "--tmux: failed to create the worktree tmux session; continuing without it"
                );
            }
        }
    }

    Ok(())
}

/// The resolved LLM stack: credentials, the assembled multi-provider config,
/// and the routed [`DefaultLlmClient`] every model-facing surface needs.
///
/// Extracted verbatim out of [`build`] so that headless one-shot commands
/// (which must NOT boot a session, fire `SessionStart` hooks, or start MCP
/// servers) reach the SAME credential-precedence and provider-assembly logic
/// the interactive runtime uses. Duplicating that resolution is the failure
/// mode this type exists to prevent: the API-key-beats-OAuth exclusion and the
/// ChatGPT `PAT > external-tokens > OAuth login` precedence are security
/// boundaries, and a second copy of them would drift silently.
pub struct LlmStack {
    /// See [`build`] for the resolution rules behind `http`.
    pub http: Arc<PosixHttp>,
    /// See [`build`] for the resolution rules behind `clock`.
    pub clock: Arc<PosixClock>,
    /// See [`build`] for the resolution rules behind `mcp_oauth_storage`.
    pub mcp_oauth_storage: Arc<dyn traits::SecureStorage>,
    /// See [`build`] for the resolution rules behind `credentials`.
    pub credentials: Arc<CredentialManager>,
    /// See [`build`] for the resolution rules behind `auth`.
    pub auth: Arc<dyn AuthHandle>,
    /// See [`build`] for the resolution rules behind `subscription`.
    pub subscription: traits::subscription::SharedSubscription,
    /// See [`build`] for the resolution rules behind `resolved_anthropic_api_key`.
    pub resolved_anthropic_api_key: Option<String>,
    /// See [`build`] for the resolution rules behind `is_subscriber`.
    pub is_subscriber: bool,
    /// See [`build`] for the resolution rules behind `openai_oauth_client`.
    pub openai_oauth_client: Arc<openai_oauth::OpenAiOAuthClient>,
    /// See [`build`] for the resolution rules behind `pricing`.
    pub pricing: cost::PricingCatalog,
    /// See [`build`] for the resolution rules behind `chains`.
    pub chains: provider_config::ChainConfig,
    /// See [`build`] for the resolution rules behind `model_providers`.
    pub model_providers: std::collections::BTreeMap<String, (String, String)>,
    /// See [`build`] for the resolution rules behind `default_listings`.
    pub default_listings: Vec<traits::ModelListing>,
    /// See [`build`] for the resolution rules behind `default_model_id`.
    pub default_model_id: String,
    /// See [`build`] for the resolution rules behind `default_model_profile`.
    pub default_model_profile: Option<String>,
    /// See [`build`] for the resolution rules behind `profile_first_party`.
    pub profile_first_party: std::collections::BTreeMap<String, bool>,
    /// See [`build`] for the resolution rules behind `provider_availability`.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
    /// See [`build`] for the resolution rules behind `default_model_fallback`.
    pub default_model_fallback: Option<DefaultModelFallbackNotice>,
    /// See [`build`] for the resolution rules behind `session_model_restriction`.
    pub session_model_restriction: Option<(llm_client::model::allowlist::ModelEnforcement, Vec<String>)>,
    /// See [`build`] for the resolution rules behind `model_setting_for_spawns`.
    pub model_setting_for_spawns: String,
    /// See [`build`] for the resolution rules behind `session_provider_first_party`.
    pub session_provider_first_party: bool,
    /// See [`build`] for the resolution rules behind `llm_client`.
    pub llm_client: Arc<DefaultLlmClient>,
    /// See [`build`] for the resolution rules behind `llm_transport`.
    pub llm_transport: Arc<dyn Transport>,
    /// See [`build`] for the resolution rules behind `cost_estimator`.
    pub cost_estimator: Arc<llm_client::CostEstimator>,
    /// See [`build`] for the resolution rules behind `subscriber_state`.
    pub subscriber_state: SubscriberState,
}

/// Resolve the LLM stack from a [`DesktopConfig`] alone.
///
/// Pure with respect to the session: it touches the keychain, the process
/// environment and the network (the availability probe), but creates no
/// session, no transcript and no hooks.
pub async fn resolve_llm_stack(cfg: &DesktopConfig) -> Result<LlmStack, BuildError> {
    // (1) Platform-minimal façade (http + clock + storage).
    let http = Arc::new(PosixHttp::new());
    let clock = Arc::new(PosixClock::new());
    let credentials_path = cfg.lingxi_home.join(".credentials.json");
    let storage = if cfg.isolated_credential_storage {
        platform_posix::plaintext_secure_storage(credentials_path).await
    } else {
        secure_storage_for_platform(
            std::env::var("USER").unwrap_or_else(|_| "default".to_string()),
            cfg.lingxi_home.clone(),
            credentials_path,
        )
        .await
    }
    .map_err(|e| BuildError::SecureStorage(e.to_string()))?;

    // (2a) Task 10: LlmTransportBridge wraps the PosixHttp transport for
    //      `DefaultLlmClient`. A second `PosixHttp` instance is used so the
    //      bridge owns its own (stateless) handle; the original `http` Arc
    //      continues to serve MCP / hooks / side-query.
    let llm_transport: Arc<dyn Transport> = Arc::new(LlmTransportBridge::new(PosixHttp::new()));
    // Defer client construction to step 3.1 where we know whether OAuth is
    // active (determines auth strategy + credential config). Placeholder: the
    // resolved OAuth `AuthState` (`Some` only for an OAuth-effective subscriber
    // session) that step (2) bridges into the assembled client's credential
    // seam as an `oauth_delegate`.
    let mut oauth_auth_state: Option<Arc<llm_client::oauth::anthropic::refresh::AuthState>> = None;
    let mut openai_oauth_state: Option<Arc<openai_oauth::AuthState>> = None;
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
    // (M13) Track WHERE the key came from — the auth resolver ranks an
    // env/host-supplied key ABOVE stored OAuth but a keychain-stored key BELOW
    // it, so the two sources must stay distinguishable.
    let mut stored_anthropic_api_key = false;
    let resolved_anthropic_api_key = if cfg.api_key.is_empty() {
        match credentials.get_anthropic_api_key().await {
            Ok(Some(key)) => {
                stored_anthropic_api_key = true;
                Some(key.expose_secret().clone())
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "could not read Anthropic API key from secure storage");
                None
            }
        }
    } else {
        Some(cfg.api_key.clone())
    };
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
    let mut persisted_subscription_type: Option<String> = None;
    match credentials.get_oauth_tokens().await {
        Ok(Some(tokens)) => {
            // (M13) Drive the documented auth-source resolver with the full
            // context instead of a hand-rolled two-flag exclusion: HOST-forced
            // OAuth makes the stored session the effective auth EVEN with an
            // env key present, an FD-inherited key outranks the stored
            // session, and a keychain-stored key ranks BELOW it. The
            // below-OAuth sources (settings key / helper / Bedrock) cannot
            // change the outcome once `has_stored_oauth` is true, so their
            // slots stay conservative.
            let auth_source =
                llm_client::oauth::anthropic::resolver::resolve(
                    &llm_client::oauth::anthropic::resolver::ResolverContext {
                        // The ONLY thing that demotes an env key below the
                        // stored session is `KWr()` (@228931361), read HERE —
                        // the credential-resolution point, exactly where
                        // `zb()` (@228933355) evaluates it — so EVERY
                        // entrypoint agrees, including the ones that never
                        // pass through the CLI's `build_runtime_from_config`
                        // (`mcp serve`, `auto-mode-setup`, bridge-server).
                        managed_oauth_only: cfg.managed_oauth_only
                            || llm_client::oauth::anthropic::resolver::host_managed_oauth_only(),
                        env_auth_token: std::env::var("ANTHROPIC_AUTH_TOKEN")
                            .ok()
                            .filter(|v| !v.is_empty()),
                        env_api_key: (!stored_anthropic_api_key)
                            .then(|| resolved_anthropic_api_key.clone())
                            .flatten(),
                        fd_present: cfg.anthropic_key_fd_present,
                        has_stored_oauth: true,
                        has_stored_api_key: stored_anthropic_api_key,
                        settings_api_key: None,
                        api_key_helper_script: cfg
                            .api_key_helper
                            .as_ref()
                            .map(std::path::PathBuf::from),
                        aws_present: false,
                    },
                );
            // (M13) The stored credential carries the tier persisted at login
            // (claude-code keeps `subscriptionType`/`rateLimitTier` inside
            // `claudeAiOauth`), so enterprise/tier-gated behaviour is correct
            // from request #1 — no async profile-fetch window — but only while
            // the stored session is the effective auth source (see
            // [`subscription_seed`]).
            let seed = subscription_seed(
                &auth_source,
                &tokens.scopes,
                tokens.subscription_type.as_ref(),
                tokens.rate_limit_tier.as_ref(),
            );
            is_subscriber = seed.is_subscriber;
            persisted_subscription_type.clone_from(&seed.subscription_type);
            // Re-seed the shared slot with the resolved subscriber flag + the
            // PERSISTED tier so readers see them even before (or without) the
            // background profile+roles fetch landing. SECRECY: deliberately
            // copy the access token (a `Secret<String>`, intentionally
            // non-`Clone`) by exposing + re-wrapping — the audited copy
            // pattern — BEFORE the original moves into `init_refresh_driver`;
            // it is exposed again only inside the spawned fetch task.
            if let Ok(mut guard) = subscription.write() {
                *guard = Some(seed);
            }
            let profile_token = protocol::Secret::new(tokens.access_token.expose_secret().clone());
            match llm_client::oauth::anthropic::client::init_refresh_driver(
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

                        // Task 4: background OAuth profile + roles fetch — the
                        // FRESHENER over the persisted-tier seed above (closes
                        // the RENDERING half of the profile-fetch PARITY-GAP
                        // documented at `orchestrator/src/config.rs:134` —
                        // tier/billing/role data for rate-limit copy — without
                        // touching the build hot path). Both fetchers swallow
                        // every error → `None` (matching the TS `logError` /
                        // `return undefined` stance); (M13) a FAILED profile
                        // fetch skips the write entirely so the seeded
                        // persisted-tier snapshot is never clobbered with an
                        // empty one (oracle preserves stored `subscriptionType`
                        // when the refresh can't resolve a new value).
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
                            let transport: std::sync::Arc<dyn traits::HttpTransport> = http.clone();
                            let creds = credentials.clone();
                            // Move (not copy) the token into the task — its
                            // only consumer.
                            let token = profile_token;
                            tokio::spawn(async move {
                                let token = token.expose_secret();
                                let Some(profile) =
                                    llm_client::oauth::anthropic::fetch_profile_from_oauth_token(
                                        token, &transport,
                                    )
                                    .await
                                else {
                                    return;
                                };
                                let roles = llm_client::oauth::anthropic::fetch_user_roles(
                                    token, &transport,
                                )
                                .await;
                                let snap = subscription_snapshot_from(
                                    true,
                                    Some(&profile),
                                    roles.as_ref(),
                                );
                                // (M13) Freshen the persisted tier too, so
                                // pre-M13 logins self-heal and the NEXT boot
                                // seeds from up-to-date values. `new ?? old`
                                // merge — never clears a stored tier.
                                if let Err(error) = creds
                                    .update_oauth_subscription(
                                        snap.subscription_type.as_deref(),
                                        snap.rate_limit_tier.as_deref(),
                                    )
                                    .await
                                {
                                    tracing::warn!(%error, "could not persist freshened subscription tier");
                                }
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
            let http_dyn: Arc<dyn traits::HttpTransport> =
                http.clone() as Arc<dyn traits::HttpTransport>;
            match openai_oauth::whoami(&openai_oauth_cfg, &http_dyn, &pat).await {
                Ok(md) => {
                    openai_chatgpt_delegate =
                        Some(Arc::new(openai_oauth::PatCredentialProvider::new(pat, md))
                            as Arc<dyn llm_client::CredentialProvider>);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "OPENAI_PERSONAL_ACCESS_TOKEN whoami failed; ignoring PAT")
                }
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
    let has_api_key = resolved_anthropic_api_key.is_some();
    // OAuth bridges into the client exactly when the auth RESOLVER made the
    // stored session the effective source (M13): `oauth_auth_state` is only
    // captured for an OAuth-effective subscriber session, which outranks a
    // keychain-stored key and — under HOST forcing (`KWr()` @228931361) — even
    // an env key. `has_oauth` selects `AuthStrategy::OAuthBearer`, which
    // is what injects the required `oauth-2025-04-20` beta on Anthropic routes;
    // the assemble input below drops the key claim when OAuth is effective so
    // the ApiKey strategy can't shadow it.
    let has_oauth = oauth_auth_state.is_some();
    let oauth_delegate: Option<Arc<dyn llm_client::CredentialProvider>> =
        oauth_auth_state.clone().map(|state| {
            let driver = Arc::new(RefreshDriver::new(state));
            Arc::new(OAuthCredentialProvider::new(driver))
                as Arc<dyn llm_client::CredentialProvider>
        });

    let assembled = provider_config::assemble(provider_config::AssembleInputs {
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: anthropic_models_for(&cfg.default_model, cfg.fallback_model.as_deref()),
        anthropic_has_api_key: has_api_key && !has_oauth,
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
                description: m.description.clone(),
                supports_reasoning: m.capabilities.reasoning,
            })
        })
        .collect();
    let (mut default_model_id, mut default_model_profile) =
        traits::parse_model_ref(&cfg.default_model, &default_listings);

    // Per-profile firstParty-ness, captured while `assembled.client_config.
    // providers` is still owned (`from_config` moves it below). The Explore
    // firstParty gate itself is evaluated AFTER the connected-provider
    // fallback so it reflects the model the session actually boots on.
    let profile_first_party: std::collections::BTreeMap<String, bool> = assembled
        .client_config
        .providers
        .iter()
        .map(|p| {
            (
                p.profile_name.clone(),
                matches!(p.provider_id, llm_client::ProviderId::AnthropicFirstParty),
            )
        })
        .collect();

    let mut client = DefaultLlmClient::from_config(assembled.client_config)
        .map_err(|e| BuildError::ApiBase(format!("llm-client config: {e}")))?;
    // §6.1: ONE composite credential slot for ALL providers (anthropic api-key /
    // oauth-delegate + every per-profile credential source).
    let mut oauth_delegates: std::collections::BTreeMap<
        String,
        std::sync::Arc<dyn llm_client::CredentialProvider>,
    > = std::collections::BTreeMap::new();
    if let Some(d) = oauth_delegate {
        oauth_delegates.insert("anthropic-oauth".to_string(), d);
    }
    // OAuth login fills the slot only if PAT/external didn't.
    if openai_chatgpt_delegate.is_none() {
        if let Some(state) = openai_oauth_state {
            let driver = std::sync::Arc::new(openai_oauth::RefreshDriver::new(state));
            openai_chatgpt_delegate = Some(std::sync::Arc::new(
                openai_oauth::OpenAiOAuthCredentialProvider::new(driver),
            )
                as Arc<dyn llm_client::CredentialProvider>);
        }
    }
    let has_openai_chatgpt = openai_chatgpt_delegate.is_some();
    if let Some(d) = openai_chatgpt_delegate {
        oauth_delegates.insert("openai-chatgpt".to_string(), d);
    }

    // Phase 2a §6.2: per-profile availability from the assembled credential
    // sources (each profile is "available" iff its keychain entry / env var
    // resolves). Computed HERE — the earliest point all five inputs exist — so
    // the connected-provider default-model fallback below can consult it; the
    // same map later drives the `/model` picker's Connect badge via
    // `DesktopRuntime.provider_availability`. Nothing between here and the
    // runtime literal mutates credentials, so early == late computation.
    let availability_probe = provider_config::compute_availability_with_isolation(
        &credentials,
        &assembled.credential_sources,
        has_api_key,
        has_oauth,
        has_openai_chatgpt,
    
        // Isolated boots ignore ambient provider env vars too — see the
        // `isolated_credential_storage` doc: the flag means "this boot inherits
        // no machine credentials", and env is the other half of that.
        cfg.isolated_credential_storage,
    );
    let availability_rows =
        match tokio::time::timeout(std::time::Duration::from_secs(5), availability_probe).await {
            Ok(rows) => rows,
            Err(_) => {
                tracing::warn!("provider availability probe timed out; continuing engine startup");
                Vec::new()
            }
        };
    let mut provider_availability: std::collections::BTreeMap<String, bool> = availability_rows
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

    // ── Boot-time connected-provider default-model fallback ─────────────────
    // (LingXi multi-provider divergence — upstream is Anthropic-only.) When the
    // configured default model's provider is definitively disconnected and
    // another provider IS connected, boot on the connected provider instead of
    // into guaranteed first-turn auth failures. Skipped when the model was an
    // EXPLICIT `--model` choice (the user asked for exactly that model), and on
    // env-routed Bedrock/Vertex/Foundry installs (anthropic models are served
    // WITHOUT anthropic key/oauth there, so "anthropic disconnected" is
    // meaningless and the reroute would break a working setup).
    let mut default_model_fallback: Option<DefaultModelFallbackNotice> = None;
    // An `ANTHROPIC_MODEL` env pin (claude-code D4) is exempt from the reroute
    // just like an explicit `--model`: the user pinned exactly that model, so a
    // fallback would defeat the pin. `default_model_env_pinned` is kept separate
    // from `default_model_explicit` (which stays `--model`-only for the `--agent`
    // override gate); only the fallback treats an env pin as explicit.
    if !cfg.default_model_explicit
        && !cfg.default_model_env_pinned
        && api_provider() == ApiProvider::FirstParty
    {
        // The anthropic probe is DEFINITIVE only on the stock first-party base
        // URL with no gateway auth override. A custom `api_base`
        // (`LINGXI_API_BASE_URL` — an enterprise/auth-free gateway serving
        // Claude with no local key) or an `ANTHROPIC_AUTH_TOKEN` works today
        // with `has_api_key == has_oauth == false`, so the forced
        // `availability["anthropic"] = false` above must not reroute those
        // installs (the same probe-blindness `connected_model_rows` guards in
        // the TUI picker).
        let anthropic_probe_definitive = cfg.api_base == DesktopConfig::default().api_base
            && std::env::var("ANTHROPIC_AUTH_TOKEN").map_or(true, |v| v.is_empty());
        if let Some(fb) = connected_provider_fallback(
            &default_model_id,
            default_model_profile.as_deref(),
            anthropic_probe_definitive,
            &model_providers,
            &provider_availability,
            &default_listings,
            &cfg.recent_models,
        ) {
            let to = format!("{}/{}", fb.profile, fb.model);
            tracing::warn!(
                from = %cfg.default_model,
                to = %to,
                "default model's provider is not connected; booting on a connected provider"
            );
            default_model_fallback = Some(DefaultModelFallbackNotice {
                from: cfg.default_model.clone(),
                to,
            });
            default_model_id = fb.model;
            default_model_profile = Some(fb.profile);
        }
    }

    // ── Managed availableModels / enforceAvailableModels constraint ─────────
    // (parity 2.1.207 H-BIN-08.) When a MANAGED (`policySettings`) tier owns an
    // `availableModels` allowlist AND sets `enforceAvailableModels: true`, the
    // Default model selection is constrained (binary `enforceAvailableModels`
    // describe text): "if the default model for the user tier is not in
    // availableModels, Default resolves to the first allowed availableModels
    // entry instead." The enforce flag is inert without a policy-OWNED
    // allowlist, and a managed source that fails to parse refuses cascade-trust
    // mode (fail-closed). Consumed via `llm_client::model::allowlist`.
    // The managed `availableModels` restriction threaded into the subagent /
    // plan-mode spawn path (parity 2.1.207 H-BIN-08). Populated from the resolved
    // enforcement below when an active policy allowlist exists; `None` (default
    // install) leaves subagent / plan-mode resolution unrestricted.
    let mut session_model_restriction: Option<(
        llm_client::model::allowlist::ModelEnforcement,
        Vec<String>,
    )> = None;
    {
        use llm_client::model::allowlist;
        let managed_model_tiers = crate::settings_watch::managed_settings_raw_tiers().await;
        let policy_source = managed_model_policy_source(&managed_model_tiers);
        // Deduplicate the byte-exact warnings (binary module-level `SN` set).
        let mut seen: Vec<String> = Vec::new();
        let enforcement = allowlist::resolve_enforcement(&policy_source, &mut |m| {
            if !seen.iter().any(|w| w == m) {
                seen.push(m.to_string());
                tracing::warn!("{m}");
            }
        });
        // Retain an ACTIVE enforcement (+ the concrete catalog) for the spawner
        // so subagent inherit-on-barred + the plan-mode upgrade gate can fire.
        if matches!(enforcement, allowlist::ModelEnforcement::Active { .. }) {
            let catalog: Vec<String> = default_listings
                .iter()
                .map(|m| m.request_model.clone())
                .collect();
            session_model_restriction = Some((enforcement.clone(), catalog));
        }
        if allowlist::model_allowed_under(&enforcement, &default_model_id) == Some(false) {
            if let allowlist::ModelEnforcement::Active {
                allowlist: al,
                overrides,
            } = &enforcement
            {
                let candidates: Vec<String> = default_listings
                    .iter()
                    .map(|m| m.request_model.clone())
                    .collect();
                if let Some(picked) =
                    allowlist::first_allowed_model(al, &candidates, Some(overrides))
                {
                    let picked_profile = default_listings
                        .iter()
                        .find(|m| m.request_model == picked)
                        .map(|m| m.provider_id.clone());
                    tracing::warn!(
                        from = %default_model_id,
                        to = %picked,
                        "default model is not in the managed availableModels allowlist; \
                         resolving Default to the first allowed availableModels entry"
                    );
                    default_model_id = picked;
                    default_model_profile = picked_profile.or(default_model_profile);
                }
            }
        }
    }

    // The raw "user model setting" seam (the opusplan/haiku plan-mode swap
    // anchor threaded into subagent/teammate model resolution). When the
    // fallback rerouted the session, the persisted alias no longer describes
    // the booted main loop — thread the rerouted ref instead so a plan-mode
    // `AgentModel::Inherit` spawn cannot swap back onto the provider the
    // fallback just declared disconnected.
    let model_setting_for_spawns = default_model_fallback
        .as_ref()
        .map_or_else(|| cfg.default_model.clone(), |n| n.to.clone());

    // (M10 cc2.1.198) LingXi multi-provider half of the Explore `GAe`/`obm`
    // firstParty gate: `false` when the session's default model routes to a
    // NON-Anthropic provider profile (OpenAI/Gemini/…) so the built-in Explore
    // agent resolves to `inherit` (the opus cap never fires for a foreign
    // provider — same behavior as the TS `fr() !== "firstParty"` branch). The
    // env half (Bedrock/Vertex/Foundry) is checked inside
    // `agent::model_resolution::resolve_builtin_explore_model`. Evaluated over
    // the POST-fallback default (the model the session actually boots on),
    // via the `profile_first_party` capture taken before `from_config`.
    let session_provider_first_party = {
        let profile_name = default_model_profile.clone().or_else(|| {
            model_providers
                .get(&default_model_id)
                .map(|(profile, _)| profile.clone())
        });
        match profile_name {
            // Unknown profile name → the built-in Anthropic route.
            Some(name) => profile_first_party.get(&name).copied().unwrap_or(true),
            // No configured profile serves the default model → the built-in
            // Anthropic route (plain api-key / OAuth install).
            None => true,
        }
    };
    if !cfg.custom_betas.is_empty() && (!has_api_key || !session_provider_first_party) {
        return Err(BuildError::InvalidCustomBetas);
    }

    let composite = provider_config::MultiCredentialProvider::new(
        credentials.clone(),
        assembled.credential_sources.clone(),
        resolved_anthropic_api_key.clone(),
        cfg.api_key_helper.clone(),
        oauth_delegates,
    );
    // GitHub Copilot needs a short-lived token minted from the raw OAuth token
    // (api.githubcopilot.com rejects the raw token). Wrap the composite so the
    // `github-copilot` credential is exchanged + cached; every other credential
    // id passes straight through unchanged.
    let copilot_creds = llm_client::CopilotExchangeCredentialProvider::new(
        Arc::new(composite),
        Arc::new(connect::PosixCopilotHttp::new()),
        "github-copilot",
    );
    client = client.with_credential_provider(Arc::new(copilot_creds));
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
    // (M13) Enterprise state now comes from the PERSISTED credential tier
    // (claude-code reads `subscriptionType` synchronously from the stored
    // tokens), so the static build-time state is correct from request #1;
    // the shared subscription slot freshens it per request. `Ger()`
    // (@228959874) is `Aa() === "enterprise"`, so it inherits `Aa()`'s
    // `isAnthropicAuthEnabled` gate — `persisted_subscription_type` is `None`
    // whenever a non-OAuth source outranks the stored blob
    // ([`subscription_seed`]).
    let subscriber_state = SubscriberState {
        is_subscriber,
        is_enterprise: persisted_subscription_type.as_deref() == Some("enterprise"),
    };

    Ok(LlmStack {
        http,
        clock,
        mcp_oauth_storage,
        credentials,
        auth,
        subscription,
        resolved_anthropic_api_key,
        is_subscriber,
        openai_oauth_client,
        pricing: assembled.pricing,
        chains: assembled.chains,
        model_providers,
        default_listings,
        default_model_id,
        default_model_profile,
        profile_first_party,
        provider_availability,
        default_model_fallback,
        session_model_restriction,
        model_setting_for_spawns,
        session_provider_first_party,
        llm_client,
        llm_transport,
        cost_estimator,
        subscriber_state,
    })
}

/// Build a model-facing [`llm_client::ApiService`] with NO session attached.
///
/// This is the entry point for one-shot commands that need to ask a model a
/// question without becoming a session: nothing here writes a transcript,
/// fires a `SessionStart` hook, starts an MCP server, or registers a tool. The
/// credential resolution and provider assembly come from
/// [`resolve_llm_stack`], so a headless call routes and authenticates exactly
/// as the interactive runtime does.
///
/// Differences from the service [`build`] constructs, all of them the absence
/// of a session rather than a change in behaviour:
///
/// - no retry reporter — there is no output stream to narrate backoff to, so
///   retries stay silent instead of being announced to nobody;
/// - no `request_metadata` — `user_id` carries a session id, and there is no
///   session;
/// - no forced `StructuredOutput` tool choice — a headless caller that wants
///   structured output asks for it per-request via `stream_json_schema`.
///
/// The routing config, fallback chains, retry overrides, cost estimator,
/// subscriber state, custom betas and AWS auth refresher are all identical to
/// the interactive path: those are properties of the install, not the session.
pub async fn build_api_service(
    cfg: &DesktopConfig,
    cwd: &std::path::Path,
) -> Result<Arc<llm_client::ApiService>, BuildError> {
    let stack = resolve_llm_stack(cfg).await?;
    Ok(Arc::new(api_service_from_stack(cfg, cwd, stack)))
}

/// Assemble the drive service (retry / rate-limit / betas loop) over an
/// already-resolved [`LlmStack`].
///
/// Split out from [`build_api_service`] so a caller that already holds a stack
/// — and wants the other halves of it too — does not resolve credentials twice.
#[must_use]
pub fn api_service_from_stack(
    cfg: &DesktopConfig,
    cwd: &std::path::Path,
    stack: LlmStack,
) -> llm_client::ApiService {
    // Same CHAINS BRIDGE as `build`: the assembled per-model chain becomes the
    // adapter's `fallback_overrides`, keyed by model id.
    let fallback_overrides: std::collections::BTreeMap<String, Vec<String>> = stack
        .chains
        .chains
        .iter()
        .map(|(key, entries)| {
            (
                key.clone(),
                entries.iter().map(|e| e.model.clone()).collect(),
            )
        })
        .collect();
    let settings_max_retries = stack.chains.retry.as_ref().map(|r| r.max_attempts);
    let settings_backoff_ms = stack.chains.retry.as_ref().map(|r| r.backoff_ms);
    let analytics_bus = Arc::new(telemetry::AnalyticsBus::new());

    let service = llm_client::ApiService::new_with_routing(
        stack.llm_client,
        stack.llm_transport,
        stack.subscriber_state,
        UserAgentEnv::from_process_env(),
        env!("CARGO_PKG_VERSION"),
        Some(analytics_bus.clone()),
        cfg.fallback_model.clone(),
        Some(stack.cost_estimator),
        fallback_overrides,
        settings_max_retries,
        settings_backoff_ms,
    )
    .with_subscription(stack.subscription)
    .with_custom_cli_betas(cfg.custom_betas.clone())
    .with_thinking(cfg.session_thinking);

    match aws_auth_refresher(cwd, analytics_bus) {
        Some(refresher) => service.with_aws_auth(refresher),
        None => service,
    }
}

/// Resolve the `awsAuthRefresh` / `awsCredentialExport` settings and build the
/// refresher when either is configured.
///
/// Returns `None` when neither is set — the common case — so a Bedrock 401 stays
/// terminal exactly as it does today. Shared by [`build`] and the headless path
/// so the workspace-trust gate (a project-sourced refresh command is refused
/// before trust is accepted) is enforced identically in both.
fn aws_auth_refresher(
    cwd: &std::path::Path,
    analytics_bus: Arc<telemetry::AnalyticsBus>,
) -> Option<Arc<llm_client::AwsAuthRefresher>> {
    let env_vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let aws_settings = engine::settings::Settings::load(engine::settings::LoadInputs {
        env: &env_vars,
        project_dir: cwd,
        defaults: engine::settings::schema::SettingsJson::default(),
    })
    .ok()
    .map(|eff| {
        let from_project = |field: &str| {
            eff.effective_for(field).is_some_and(|p| {
                p.contributors.last() == Some(&engine::settings::tracer::Source::Project)
            })
        };
        llm_client::AwsAuthSettings {
            aws_auth_refresh: eff.settings.aws_auth_refresh.clone(),
            aws_auth_refresh_from_project: from_project("awsAuthRefresh"),
            aws_credential_export: eff.settings.aws_credential_export.clone(),
            aws_credential_export_from_project: from_project("awsCredentialExport"),
            // No global config path ⇒ the CLI trust gate proceeds (mode.rs
            // `trust_gate_should_prompt` — nothing to check against), so treat
            // as trusted like the gate does.
            workspace_trusted: match migrations::global_config::global_config_path() {
                Some(p) => migrations::global_config::check_has_trust_dialog_accepted(&p, cwd),
                None => true,
            },
        }
    })
    .unwrap_or_default();
    if aws_settings.aws_auth_refresh.is_none() && aws_settings.aws_credential_export.is_none() {
        return None;
    }
    Some(Arc::new(llm_client::AwsAuthRefresher::new(
        aws_settings,
        Arc::new(llm_client::ShellAwsAuthProcess),
        Some(analytics_bus),
    )))
}

pub async fn build(
    cfg: DesktopConfig,
    output: Arc<dyn OutputStream>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<DesktopRuntime, BuildError> {
    let cwd = cfg.cwd.clone();

    // On-disk data-retention sweep (claude-code `fWu`). DELETES stale
    // session-file entries (todos/statsig/logs older than the retention period),
    // so it is flag-gated and default-OFF: a no-op unless `LINGXI_RETENTION_SWEEP`
    // is truthy. Runs once at boot on a blocking pool so it never delays startup.
    tokio::task::spawn_blocking(memory::retention::run_startup_retention_sweep);

    // FIX A/B/C: mint the boot-canonical MAIN session id ONCE and derive the
    // session's transcript path + subagents dir from `(lingxi_home, cwd, id)`.
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
    // claude-code `--session-id <uuid>`: honor a host-provided session id when
    // present (already UUID-validated by the host), else mint a fresh one. The
    // override carries through to the transcript path, the firers' precomputed
    // `transcript_path`, and the orchestrator's live `.with_session_id`, so a
    // resumed/SDK-pinned id is consistent everywhere.
    let main_session_id = cfg
        .session_id_override
        .as_deref()
        .and_then(protocol::SessionId::parse_prefixed)
        .unwrap_or_else(protocol::SessionId::new);
    let main_session_uuid = main_session_id.as_uuid().to_string();
    // (/rewind) One shared file-history checkpoint store: cloned into the
    // orchestrator (per-turn snapshots + pre-edit tool backups) AND the
    // DesktopRuntime (so the CLI can restore + build the picker rows). Backups
    // live under `<lingxi_home>/file-history/<session>/`.
    let file_history = std::sync::Arc::new(session::FileHistory::new(
        cfg.lingxi_home.clone(),
        cwd.clone(),
        main_session_uuid.clone(),
    ));
    let main_transcript_path = orchestrator::transcript_paths::main_transcript_path(
        &cfg.lingxi_home,
        &cwd.to_string_lossy(),
        &main_session_uuid,
    );
    let main_subagents_dir = orchestrator::transcript_paths::subagents_dir(
        &cfg.lingxi_home,
        &cwd.to_string_lossy(),
        &main_session_uuid,
    );

    // The credential/provider half of boot lives in `resolve_llm_stack` so the
    // headless one-shot commands share it byte-for-byte. Everything below this
    // point is session-shaped and stays here.
    let LlmStack {
        http,
        clock,
        mcp_oauth_storage,
        credentials,
        auth,
        subscription,
        resolved_anthropic_api_key,
        is_subscriber,
        openai_oauth_client,
        pricing,
        chains,
        model_providers,
        default_listings,
        default_model_id,
        default_model_profile,
        provider_availability,
        default_model_fallback,
        session_model_restriction,
        model_setting_for_spawns,
        session_provider_first_party,
        llm_client,
        llm_transport,
        cost_estimator,
        subscriber_state,
        // `profile_first_party` is an input to the resolution itself (the
        // Explore firstParty gate consumes it inside `resolve_llm_stack`); the
        // session half below reads the resolved outputs instead. Headless
        // callers still get it off `LlmStack`.
        ..
    } = resolve_llm_stack(&cfg).await?;

    // Phase 2a CHAINS BRIDGE: translate the assembled `ChainConfig` into main's
    // richer adapter's `fallback_overrides` shape. `assemble` keys each chain by
    // the request/display model id and carries an ordered list of `ChainEntry`;
    // main's adapter routes by model-id through the multi-provider registry, so the
    // `ChainEntry.provider_id` is informational and is dropped here — the per-entry
    // `model` ids are the fallback chain. Cross-provider routing still works
    // because every provider's models are registered in the assembled
    // `ClientConfig`, so a fallback target on another provider resolves by id.
    let fallback_overrides: std::collections::BTreeMap<String, Vec<String>> = chains
        .chains
        .iter()
        .map(|(key, entries)| {
            (
                key.clone(),
                entries.iter().map(|e| e.model.clone()).collect(),
            )
        })
        .collect();
    // Retry override → main's scalar settings_max_retries / settings_backoff_ms.
    let settings_max_retries = chains.retry.as_ref().map(|r| r.max_attempts);
    let settings_backoff_ms = chains.retry.as_ref().map(|r| r.backoff_ms);

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
    // orchestrator (`tengu_api_success` per completed response) so all live
    // telemetry lands on the same sink set — 1:1 with claude-code, where
    // `logEvent` is a single global pipeline.
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
        user_id: llm_client::ApiService::build_api_metadata_user_id(
            &migrations::global_config::get_or_create_user_id(),
            "",
            &main_session_uuid,
            cfg.parent_session_id.as_deref(),
        ),
    };
    // Build the provider-neutral drive service (the retry/rate-limit/betas loop),
    // then wrap it in the thin `ProviderApiAdapter` that impls the orchestrator +
    // agent seams. The `with_*` builders live on `ApiService`.
    let service_built = llm_client::ApiService::new_with_routing(
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
    .with_custom_cli_betas(cfg.custom_betas.clone())
    .with_request_metadata(request_metadata)
    // Boot SESSION thinking config, resolved host-side from MAX_THINKING_TOKENS
    // + --max-thinking-tokens + alwaysThinkingEnabled (claude-code `qIe()`+`wn`).
    // Default `Adaptive` keeps every existing session byte-identical; a fixed
    // env/flag budget pre-empts adaptive, `alwaysThinkingEnabled:false` disables.
    .with_thinking(cfg.session_thinking)
    // Surface API retry/backoff status to the UI (Claude Code's
    // `SystemAPIErrorMessage`): the retry loop reports each backoff and the
    // adapter forwards it to the session output stream (→ TUI).
    .with_retry_reporter(std::sync::Arc::new(OutputRetryReporter {
        output: output.clone(),
    }));
    // M2 (2.1.198): attach the AWS auth-refresh driver (`ZBd`/`t2d`) when an
    // `awsAuthRefresh` / `awsCredentialExport` command is configured. Resolves
    // the merged settings value + its Project provenance (binary `mqe`: a
    // project/local-sourced command is refused before workspace trust) and the
    // workspace-trust state (`yd()`: `hasTrustDialogAccepted` parent-walk in
    // the global config). With the driver attached, a Bedrock 401/403
    // (expired STS) runs the refresh script and retries instead of
    // dead-ending — bounded at Ygf=2 inside the drive loops.
    let service_built = match aws_auth_refresher(&cwd, analytics_bus.clone()) {
        Some(refresher) => service_built.with_aws_auth(refresher),
        None => service_built,
    };
    // `--json-schema` structured output: FORCE the `StructuredOutput` tool so the
    // model returns its final result through it (1:1 with claude-code). Untouched
    // for every normal turn (`json_schema` is `None`).
    let service_built = if cfg.json_schema.is_some() {
        service_built.with_forced_tool_choice(llm_client::ToolChoice::Tool {
            name: orchestrator::structured_output::STRUCTURED_OUTPUT_TOOL_NAME.to_string(),
        })
    } else {
        service_built
    };
    // (M4 cc2.1.198) `--effort <level>` — the CLI-validated initial effort
    // rides the MAIN loop's requests as `output_config.effort` (binary session
    // state `thinkingConfig: SF(a.effort)`); `None` keeps bodies unchanged.
    // (/fast) One shared fast-mode flag cloned into BOTH the request-building
    // adapter (which reads it per-turn to send `speed:"fast"`) and the
    // orchestrator (whose `set_fast_mode` handle flips it). Same `Arc`, so a
    // live `/fast` toggle is seen by the adapter on the next turn. Defaults
    // `false`, so request bodies stay byte-identical until toggled.
    let fast_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Keep the concrete session service shared with compaction/recap. Their
    // forked summary call must use the same resolved provider route and live
    // credential as the parent turn (Claude Code's single API pipeline).
    let api_service = Arc::new(service_built);
    let provider_adapter = Arc::new(
        ProviderApiAdapter::new(api_service.clone())
            .with_initial_effort(cfg.initial_effort.clone().map(serde_json::Value::String))
            .with_fast_mode(fast_flag.clone()),
    );
    // WebSearch uses the resolved Anthropic key, while MCP large-result
    // confirmation reuses the fully routed/OAuth-aware main session provider.
    let tool_provider = Arc::new(
        AnthropicRequestBuilder::new(
            resolved_anthropic_api_key.clone().unwrap_or_default(),
            Some(cfg.api_base.clone()),
        )
        .with_mcp_token_counter(provider_adapter.clone()),
    );
    let provider_adapter_handle = provider_adapter.clone();
    let api_client: Arc<dyn OrchestratorApiClient> = provider_adapter.clone();
    // The SAME `ProviderApiAdapter` drives the streaming turn path: it impls both
    // `OrchestratorApiClient` (batched/non-stream) and `StreamingApiClient` (SSE),
    // and conversation.rs documents `self.api == self.streaming_api` in production.
    // Without this the orchestrator falls back to `NoStreamingApiClient` and every
    // streaming turn fails with "no streaming client configured".
    let streaming_api: Arc<dyn orchestrator::StreamingApiClient> = provider_adapter.clone();
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
    // (2.1.212) CLI `--effort <level>` — the session's resolved reasoning-effort
    // level (already normalized to low/medium/high/xhigh/max). Threaded here so
    // every REAL assistant transcript line records it as a top-level `effort`
    // field (the SAME source the provider adapter uses for `output_config.effort`
    // via `with_initial_effort`). `None` (no `--effort`) omits the field, keeping
    // transcripts byte-identical.
    orch_cfg.effort.clone_from(&cfg.initial_effort);
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
    // (M13) `is_enterprise` is seeded from the tier PERSISTED in the stored
    // credential (claude-code reads `subscriptionType` synchronously from the
    // stored tokens) — no profile fetch in the build hot path, and no async
    // misclassification window before the background freshener lands.
    orch_cfg.is_subscriber = is_subscriber;
    orch_cfg.is_enterprise = subscriber_state.is_enterprise;
    // OUTSTYLE.2: thread the merged `settings.outputStyle` (TS string) into the
    // orchestrator config so `build_system_prompt` injects the active style's
    // `# Output Style: <name>` section (Explanatory / Learning builtins). `None`
    // / "default" / unknown ⇒ no section (prompt byte-identical to before).
    orch_cfg.output_style = load_merged_output_style(&cfg.cwd);
    traits::session_flags::set_show_thinking_summaries(load_merged_show_thinking_summaries(
        &cfg.cwd,
    ));
    // OUTSTYLE.3: custom output-style search dirs — user (`~/.lingxi/output-styles`)
    // then project (`<cwd>/.lingxi/output-styles`), in increasing priority so a
    // project style overrides a user one and both override the builtins. A
    // `settings.outputStyle` naming a disk style now activates it
    // (`outputstyles::resolve_output_style`); absent dirs ⇒ builtin-only.
    orch_cfg.output_style_dirs = vec![
        cfg.lingxi_home.join("output-styles"),
        cfg.cwd.join(branding::DOT_DIR).join("output-styles"),
    ];
    // CLI `--system-prompt` / `--system-prompt-file`: override the assembled
    // system prompt for the session. `None` keeps the memory-hierarchy prompt
    // assembled from LINGXI.md files (byte-identical to the pre-field state).
    if let Some(override_prompt) = cfg.system_prompt_override.clone() {
        orch_cfg.system_prompt_override = Some(override_prompt);
    }
    // CLI `--plan-mode-instructions` (print-gated in init.rs): custom plan-mode
    // workflow body. `None` keeps the default 5-phase plan reminder.
    orch_cfg
        .plan_mode_instructions
        .clone_from(&cfg.plan_mode_instructions);
    // `settings.json` `plansDirectory` (206 `iT`): custom plan-file directory,
    // resolved against the project root with a within-root containment check by
    // the orchestrator. `None` keeps the default `<config-home>/plans/`.
    orch_cfg.plans_directory.clone_from(&cfg.plans_directory);
    // CLI `--exclude-dynamic-system-prompt-sections`: move the per-machine env
    // block out of the (cacheable) system prompt into the first user message.
    orch_cfg.exclude_dynamic_system_prompt_sections = cfg.exclude_dynamic_system_prompt_sections;
    // (gap218 #43) The in-place (bridge/desktop) resume adopts a resumed agent's
    // frontmatter `model` ONLY when the user did NOT pass `--model` — the
    // hot-resume twin of the COLD-resume gate below (`!cfg.default_model_explicit`).
    // The root owns `--model`, so we resolve the gate here; the orchestrator then
    // resolves the alias → wire id and applies it. An explicit `--model` sets this
    // `false`, so it is never overridden by agent frontmatter.
    orch_cfg.apply_resumed_agent_model = !cfg.default_model_explicit;
    // CLI `--append-system-prompt` / `--append-system-prompt-file`: text to
    // append after the assembled system prompt (or after `system_prompt_override`
    // when both are set). Appended with a newline separator.
    if let Some(append) = cfg.append_system_prompt.clone() {
        let base = orch_cfg
            .system_prompt_override
            .get_or_insert_with(String::new);
        if !base.is_empty() {
            base.push('\n');
        }
        base.push_str(&append);
    }

    // (4.5) One CostTracker per process. The persist channel drains into a
    //       fire-and-forget task that discards snapshots (on-disk persistence is
    //       later work). Depth 64 absorbs bursts without blocking.
    let (cost_persist_tx, mut cost_persist_rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move { while cost_persist_rx.recv().await.is_some() {} });
    // Phase 2a T7: the CostTracker uses the SAME assembled pricing catalog the
    // estimator was built from (built-in reference tiers + non-Anthropic preset
    // rows + settings overrides), not a fresh `builtin_reference()`, so session
    // cost accounting matches per-response cost estimation.
    let cost_tracker = Arc::new(cost::CostTracker::new(
        protocol::SessionId::new(),
        Arc::new(pricing),
        cost_persist_tx,
    ));

    // (4.6) Subagent spawner pool + budget enforcer for the `AgentTool` seam.
    //       `AgentTool::call` requires BOTH `subagent_spawner` and
    //       `budget_enforcer` to be `Some` — wiring the spawner alone is inert.
    //
    //       The pool is the production `StateMachinePool` driven by the posix
    //       `RuntimeSpawner`; the capacity mirrors Claude Code 2.1.217's
    //       `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` (default 20).
    //       `with_api_client(subagent_api)` hands the child runner the
    //       real model seam so spawned subagents drive the multi-turn
    //       `run_subagent_loop` (gated on `ctx.api_client.is_some()`) instead of
    //       the legacy stub completion.
    let subagent_pool = Arc::new(agent::StateMachinePool::new(
        Arc::new(PosixRuntime::new()),
        traits::subagent_spawn::max_concurrent_subagents(),
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
        // stripped (or the connected-provider fallback's rerouted id), so an
        // `opusplan`/`sonnet` install leaves it a bare alias. An
        // `AgentModel::Inherit` spawn in DEFAULT mode returns the parent verbatim,
        // which would be a bogus wire id that fails at the provider. Resolve it
        // here; the raw alias is still threaded via `with_model_setting` below for
        // the plan-mode `opusplan→Opus` swap.
        .with_default_model(agent::model_resolution::resolve_user_specified_model(
            &orch_cfg.model,
        ))
        // #15: thread the live permission mode + the RAW user model setting
        // (e.g. "opusplan" / "haiku" — claude-code's
        // `getUserSpecifiedModelSetting()`, the UN-resolved alias) into the
        // spawner so `resolve_agent_model`'s `getRuntimeMainLoopModel` branch
        // actually fires for an `AgentModel::Inherit` spawn: an `opusplan` install
        // in plan mode resolves the subagent to Opus (not the resolved Sonnet
        // main-loop model). Without these the Inherit branch returns the parent
        // model unchanged (default mode → byte-identical to before this seam).
        // `model_setting_for_spawns` = `cfg.default_model` unless the
        // connected-provider fallback rerouted the session (then the plan-mode
        // swap must not resurrect the disconnected anthropic route).
        .with_permission_mode(cfg.permission_mode)
        .with_model_setting(model_setting_for_spawns.clone())
        // (parity 2.1.207 H-BIN-08) Managed availableModels restriction: a
        // subagent whose explicitly-requested model is policy-barred inherits the
        // parent/runtime model (binary `Qly`), and the plan-mode `opusplan`→Opus /
        // `haiku`→Sonnet upgrade is gated to the newest permitted family model
        // (binary `RF`). `None` (default install) ⇒ unrestricted (legacy).
        .with_model_restriction_opt(session_model_restriction.clone())
        // (M10 cc2.1.198) Explore `GAe` firstParty gate, multi-provider half:
        // a non-Anthropic default profile behaves like the TS non-firstParty
        // branch (Explore → inherit, never the opus cap).
        .with_session_provider_first_party(session_provider_first_party)
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
        // …and the writer that actually creates the file the line above names.
        // Without it `agent_transcript_path` pointed at nothing, and a
        // background agent's conversation existed only in memory.
        .with_transcript_fs(
            Arc::new(PosixFileSystem::new(cwd.clone())) as Arc<dyn traits::FileSystem>,
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
    // FIX (B-agent-model-inheritance): grab the set-once live-default-model cell
    // BEFORE boxing, to fill once the orchestrator (which owns the LIVE
    // `session.model`) exists — same cycle-break as the cells above. Once filled,
    // a spawn whose request carries no `parent_model_override` (the non-`AgentTool`
    // spawn paths) resolves `AgentModel::Inherit` against the LIVE session model
    // (updated by `/model` switches / resume) instead of the boot snapshot below.
    let subagent_default_model_provider_cell =
        subagent_spawner_concrete.default_model_provider_handle();
    // Box ONCE as the concrete `Arc<PoolSubagentSpawner>` so it can serve as
    // BOTH the one-shot `SubagentSpawner` and the persistent/resume
    // `StreamingSubagentSpawner` (Phase-1 seam) — the LocalAgent handler needs
    // the streaming half to make a backgrounded agent "come to rest" + resume.
    let subagent_spawner_arc = Arc::new(subagent_spawner_concrete);
    let subagent_spawner: Arc<dyn traits::subagent_spawn::SubagentSpawner> =
        subagent_spawner_arc.clone();
    let subagent_streaming_spawner: Arc<dyn agent::StreamingSubagentSpawner> = subagent_spawner_arc;

    //       The budget enforcer shares both the process `CostTracker` and the
    //       CLI `--max-budget` ceiling with the main orchestrator. Claude Code
    //       2.1.217 stops background subagents when that ceiling is reached;
    //       `Halt` makes each child runner's turn-boundary budget check enforce
    //       the same limit. With no CLI ceiling this remains unlimited.
    let budget_enforcer: Arc<dyn traits::budget::BudgetEnforcerHandle> =
        Arc::new(cost::BudgetEnforcer::new(
            cost::BudgetConfig {
                max_session_nano_usd: orch_cfg.max_budget_nano_usd,
                max_turn_nano_usd: None,
                max_turn_tokens: None,
                warning_thresholds: Vec::new(),
                on_exceed: cost::BudgetExceedPolicy::Halt,
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
    //     so the orchestrator loads the real `<cwd>/LINGXI.md` +
    //     `~/.lingxi/LINGXI.md` hierarchy into the system prompt (claude-code
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
            // (3c) Persist an `AllowAlways` choice to `<cwd>/.lingxi/settings.local.json`.
            let gate = Arc::new(AdapterPermissionGate::new(permission_sink).with_persist(
                permission::PermissionPaths {
                    lingxi_home: cfg.lingxi_home.clone(),
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
    // config `~/.lingxi.json` (mcp_paths[1]); local is keyed by the canonical
    // project key for `cwd`.
    let mut mcp_configs = mcp::load_mcp_servers(&project_mcp_path, &global_mcp_path, &cwd);
    // CLI `--mcp-config` servers: highest precedence — override a discovered
    // server of the same name, else append. (With `--strict-mcp-config` the host
    // nulled the discovered paths above, so `mcp_configs` starts empty and these
    // become the only servers.)
    for c in &cfg.cli_mcp_servers {
        if let Some(existing) = mcp_configs.iter_mut().find(|x| x.name == c.name) {
            *existing = c.clone();
        } else {
            mcp_configs.push(c.clone());
        }
    }
    // Per-project MCP-server enable/disable gate (claude-code `eI()`/`rTo()`/`bX`,
    // 2.1.200+): the `~/.lingxi.json` `projects.<cwd_key>` keys `enabledMcpServers`
    // (allowlist, applies ONLY to the builtin `computer-use` server) and
    // `disabledMcpServers` (denylist, applies to every other server) mark a gated
    // server `disabled` so it is seeded as `Disconnected` and never auto-connected
    // (`if(eI(cn))return`) while `/mcp` still lists it as disabled. Applied AFTER
    // the `--mcp-config` merge so an explicitly-supplied server is gated too.
    mcp::apply_project_server_gate(&mut mcp_configs, &global_mcp_path, &cwd);
    // Enterprise MCP policy (claude-code `Qme`/`Ree`): when a managed
    // `managed-mcp.json` is active it takes EXCLUSIVE control — only its own
    // servers load; otherwise any server the managed allow/deny policy blocks
    // (`deniedMcpServers`/`allowedMcpServers`) is dropped before connect. Inert
    // (no server removed) when no managed config/policy is present, so a default
    // deployment is byte-identical.
    mcp::enterprise_policy::apply_enterprise_mcp_policy(&mut mcp_configs);
    // MCP config-load diagnostics (claude-code `F7t`): surface per-entry config
    // problems (unknown type, url-without-type, invalid entry, reserved name,
    // missing env vars, `servers`-vs-`mcpServers`) to stderr at startup, the way
    // claude logs them. Silent when every config is clean, so a healthy setup
    // prints nothing.
    for w in mcp::config_diagnostics::collect_all_mcp_config_warnings(&cwd, Some(&global_mcp_path))
    {
        eprintln!("{}", w.to_stderr_line());
    }

    // (5.3) Agent catalog — load from project + user agents/. Project wins on
    //       collision (passed SECOND; later paths win). The user agents dir is
    //       `cfg.lingxi_home/agents` (was `dirs::home_dir()/.lingxi/agents`).
    //       HOISTED above the MCP registry build (M7 cc2.1.220): the
    //       agent-frontmatter MCP merge just below must see the wanted agent's
    //       markdown/flag definition BEFORE `connect_all` snapshots the config
    //       list. Nothing between here and the former position reads the
    //       catalog; plugin agents still land later via the plugin bootstrap's
    //       `plugin_agent_catalog` writes.
    let project_agents_dir = cwd.join(branding::DOT_DIR).join("agents");
    let user_agents_dir = cfg.lingxi_home.join("agents");
    // (M3 cc2.1.198) `--safe-mode` / `--bare` disable custom agent definitions
    // (`V5d.agents:!0`, `K5d.agents:!1`) — skip the dir scan, empty catalog.
    let mut agents = if cfg.customization_gates.disables_custom_agents() {
        Vec::new()
    } else {
        agent::load_agents_from_dirs(&[
            (user_agents_dir, agent::definition::AgentSource::UserDefined),
            (project_agents_dir, agent::definition::AgentSource::Project),
        ])
        .await
    };
    // (M4 cc2.1.198) `--agents <json>` flag agents — see
    // [`merge_cli_flag_agents`].
    merge_cli_flag_agents(
        &mut agents,
        cfg.cli_agents_json.as_deref(),
        cfg.customization_gates.safe_mode,
    );

    // (P2-02 cc2.1.207 / M7 cc2.1.220) The agent to apply to the MAIN loop: an
    // EXPLICIT `--agent` (fresh boot or re-passed on `--resume`) wins;
    // otherwise, on a resume with no `--agent`, the persisted `agentSetting`
    // (`rVe` restoration). `from_resume` selects the miss warning + suppresses
    // the re-persist (the record is already on disk). Computed HERE — before
    // the MCP registry connects — because claude merges the resolved
    // main-thread agent's frontmatter `mcpServers` into `dynamicMcpConfig`
    // (`FWt`) BEFORE the MCP clients connect. The APPLICATION to the
    // orchestrator seam still happens later, against the FINAL
    // (plugin-inclusive) catalog.
    let (wanted_agent, resumed_agent_snapshot, from_resume): (
        Option<String>,
        Option<serde_json::Value>,
        bool,
    ) = match cfg.cli_agent.clone() {
        Some(w) => (Some(w), None, false),
        None if cfg.session_id_override.is_some() => {
            let snapshot_fs =
                Arc::new(PosixFileSystem::new(cwd.clone())) as Arc<dyn traits::FileSystem>;
            let (persisted, snapshot) = session::jsonl::read_agent_resume_state(
                &main_transcript_path,
                snapshot_fs,
                &main_session_uuid,
            )
            .await;
            (persisted, snapshot, true)
        }
        None => (None, None, false),
    };
    // (M7 cc2.1.220) Resolve the definition the `FWt` merge consults. The
    // FINAL catalog does not exist yet (plugin agents land with the plugin
    // bootstrap), but plugin agents cannot carry `mcpServers` (LingXi's
    // parse-time privilege gate rejects them; claude strips the field with a
    // warning), so the markdown/flag set + the resume snapshot covers every
    // server-bearing definition. Miss handling (the "not found" warning) stays
    // with the application block below.
    let main_agent_def_for_mcp: Option<agent::AgentDefinition> =
        wanted_agent.as_ref().and_then(|wanted| {
            resumed_agent_snapshot
                .as_ref()
                .and_then(|v| serde_json::from_value::<agent::AgentDefinition>(v.clone()).ok())
                .filter(|a| &a.agent_type == wanted)
                .or_else(|| {
                    agents
                        .iter()
                        .find(|a| &a.agent_type == wanted)
                        .or_else(|| {
                            let suffix = format!(":{wanted}");
                            agents.iter().find(|a| a.agent_type.ends_with(&suffix))
                        })
                        .cloned()
                })
        });
    // (M7 cc2.1.220) `FWt(existing, agentDef, opts)` — fold the agent's
    // frontmatter `mcpServers` into the to-connect list so they register,
    // connect and surface tools EXACTLY like `--mcp-config` servers. Applied
    // AFTER `apply_project_server_gate` + `apply_enterprise_mcp_policy`: agent
    // servers are never project-approval-gated (claude approval covers
    // `.mcp.json` servers) and the merge runs its OWN `Yee` enterprise filter +
    // `T3()` managed-exclusive skip below, mirroring claude's ordering (`FWt`
    // merges into `dynamicMcpConfig` after discovery filtering).
    let agent_mcp_blocked = merge_agent_frontmatter_mcp_servers(
        &mut mcp_configs,
        main_agent_def_for_mcp.as_ref(),
        AgentMcpMergeGates {
            safe_mode: cfg.customization_gates.safe_mode,
            strict_mcp_config: cfg.strict_mcp_config,
            enterprise_mcp_active: mcp::enterprise_policy::enterprise_mcp_active(),
        },
        &mcp::enterprise_policy::read_managed_mcp_policy(),
    );
    if !agent_mcp_blocked.is_empty() {
        // claude's headless-start `onBlocked` (the only site that prints):
        // `Warning: agent frontmatter MCP ${Tt(len,"server")} blocked by
        // enterprise policy: ${names.join(", ")}` — `Tt` pluralizes WITHOUT a
        // count.
        eprintln!(
            "Warning: agent frontmatter MCP {} blocked by enterprise policy: {}",
            if agent_mcp_blocked.len() == 1 {
                "server"
            } else {
                "servers"
            },
            agent_mcp_blocked.join(", ")
        );
    }
    let agent_catalog = Arc::new(tokio::sync::RwLock::new(agents));

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
    //       (cwd/.lingxi/settings.json) then user (lingxi_home/settings.json),
    //       project last so it wins on identical command registration. The user
    //       root is `cfg.lingxi_home` (was `dirs::config_dir()/claude`).
    let mut hook_registry = hooks::HookRegistry::new();
    let project_settings_path = cwd.join(branding::DOT_DIR).join("settings.json");
    let user_settings_path = cfg.lingxi_home.join("settings.json");
    // `--setting-sources` scope (default `(true, true)` = all tiers): skip the
    // user tier when `!include_user` and the project tier when `!include_project`
    // so e.g. `--setting-sources project` does NOT register user-level hooks.
    let (incl_user_settings, incl_project_settings) = cfg.setting_source_scope;
    // (M3 cc2.1.198) `--safe-mode` / `--bare`: skip settings-file hooks. Bare
    // disables hooks outright (binary `V5d.hooks:!0`); safe mode collapses the
    // hooks-config merge to the POLICY tier only (`UQr()`: `if(e?.
    // allowManagedHooksOnly===!0||Ql())return e?.hooks??{}`) — lingxi loads no
    // policySettings hook tier, so both modes register zero settings hooks.
    let skip_settings_hooks = cfg.customization_gates.disables_settings_hooks();
    for (path, source, included) in [
        (
            user_settings_path,
            hooks::definition::HookSource::User,
            incl_user_settings,
        ),
        (
            project_settings_path,
            hooks::definition::HookSource::Project,
            incl_project_settings,
        ),
    ] {
        if !included || skip_settings_hooks {
            continue;
        }
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
    //        Reads the user/project/local settings files PLUS the managed
    //        (policySettings) tier — read last, so a managed `defaultMode`
    //        wins (parity 2.1.207 P1-10). File-glob content
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
    // Scope: when the env var is UNSET, default-on applies to BOTH inners — the
    // `NoOpPermissionGate` (CLI/desktop, allow-all) AND the connection-scoped
    // `AdapterPermissionGate` (transport hosts). claude-code enforces ONE core
    // policy on every host; wrapping the adapter gate with `PolicyPermissionGate`
    // makes local deny/allow rules + defaultMode bind on the bridge too, while the
    // adapter gate stays the Ask-delegation transport (an unresolved mutating Ask
    // still forwards to the remote client). An explicit env value still overrides.
    //
    // Inner-gate selection (the `(perms, adapter_gate)` match at :1836):
    // - INTERACTIVE TUI sessions inject `tui::permission_gate::TuiPermissionGate`
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
    // (#3 shell-expansion) Capture the boot `Arc<PermissionPolicy>` before it is
    // consumed by `PolicyPermissionGate::new` so `tool_ctx.permission_policy` can
    // share the SAME base policy the model-facing gate enforces. The prompt
    // shell-expansion provider reads it as the base for embedded `!`cmd`` bodies.
    // `None` only when enforcement is off (no boot policy is built) — the
    // `tool_ctx` literal then falls back to a Default-mode policy with roots.
    let mut boot_permission_policy: Option<Arc<permission::PermissionPolicy>> = None;
    // H-CHG-02: capture the enforcing gate's set-once LIVE-model cell (cycle-break)
    // so it can be filled once the orchestrator (owner of the live `session.model`)
    // exists — the live `set_permission_mode` auto gate then evaluates `dUe(wi())`
    // against the CURRENT model (mutated by `/model` switches / resume), mirroring
    // claude-code `Nle` reading `wi()`. `None` when enforcement is off (no
    // `PolicyPermissionGate` is built, so there is no live surface to gate).
    let mut live_model_provider_cell: Option<
        Arc<std::sync::OnceLock<permission::LiveModelProvider>>,
    > = None;
    // The session's additional working directories (settings
    // `additionalDirectories` union CLI `--add-dir`), captured out of the
    // enforcement branch so BOTH the file-tool `trusted_dirs` (below) and the
    // MCP registry `roots/list` source see them. claude-code's file tools
    // (`FY(t)`) and `roots/list` (`r1d()`) BOTH advertise cwd +
    // additionalWorkingDirectories — file tools inside an `--add-dir` root are
    // allowed, not hard-blocked (parity 2.1.207 P1-08). Raw entries (`~`,
    // relative) are expanded when they land in `trusted_dirs`. Assigned in BOTH
    // arms below (the full union when enforcing; `--add-dir` only otherwise), so
    // it is always initialized before its later reads.
    let boot_additional_working_dirs: Vec<std::path::PathBuf>;
    let perms: Arc<dyn PermissionGate> = if enforce_permissions {
        // Read the persistable rule tiers in ASCENDING priority — user →
        // project → local (3c: settings.local.json read after project so a
        // persisted `AllowAlways` is honored on the next enforced boot), then
        // the managed (policySettings) tier LAST/highest so enterprise
        // deny/ask/allow rules bind and managed `defaultMode` /
        // `disableBypassPermissionsMode` win (parity 2.1.207 P1-10). The
        // `--setting-sources` scope gates user/project+local but NOT managed
        // (claude-code `Xv()` force-includes `policySettings`); a managed
        // `allowManagedPermissionRulesOnly: true` drops every non-managed rule.
        // Full tier semantics on `load_boot_permission_tiers`.
        let BootPermissionTiers {
            rules,
            mut mode,
            bypass_disabled,
            auto_mode_disabled,
            classify_all_shell,
            mut additional_working_dirs,
            raw_tiers,
            allow_managed_permission_rules_only,
        } = load_boot_permission_tiers(&cfg.lingxi_home, &cwd, cfg.setting_source_scope).await;
        // CLI `--add-dir <directories...>`: union the host-provided dirs into
        // the working-dir set, exactly like a settings-tier
        // `additionalDirectories` entry (claude-code "Additional directories
        // to allow tool access to").
        additional_working_dirs.extend(cfg.add_dir.iter().cloned());
        // Capture the union (settings additionalDirectories + --add-dir) for the
        // file-tool `trusted_dirs` and MCP `roots/list` source below.
        boot_additional_working_dirs = additional_working_dirs.clone();
        let rule_count = rules.len();
        // Phase 3a: supply the filesystem roots so file-path CONTENT rules
        // (`Edit(src/**)`, `Read(./secrets/**)`) match the input path. Roots
        // resolve per rule source — user settings against `lingxi_home`,
        // project/local against `cwd` — exactly as claude-code's
        // `rootPathForSource` does.
        let roots = permission::FsRoots {
            cwd: cwd.clone(),
            home: dirs::home_dir(),
            lingxi_home: cfg.lingxi_home.clone(),
        };
        // Phase 3a-bash: attach the sandbox-auto-allow config derived from
        // the SAME settings tiers, so a sandboxable bash command that
        // matched no explicit deny/ask rule is auto-allowed (the sandbox is
        // the safety boundary). Faithful to claude-code's
        // `bashToolHasPermission` sandbox branch; a no-op when sandboxing is
        // disabled in settings (`enabled = false`). OUTSIDE enforce mode this
        // whole block is skipped, so the layer stays a permanent no-op there.
        //
        // `raw_tiers` already ends with the managed (policySettings) tier —
        // `load_boot_permission_tiers` appends it LAST/highest, so the
        // sandbox-auto-allow fold (last write wins) lets a managed `sandbox.*`
        // override user/project/local (SETTING_SOURCES: …→localSettings→
        // flagSettings→policySettings) with ONE disk read shared between the
        // permission-rule and sandbox derivations (parity 2.1.207 P1-10).
        let raw_tier_refs: Vec<&str> = raw_tiers.iter().map(String::as_str).collect();
        let sandbox_auto_allow = sandbox_auto_allow_from_settings_tiers(&raw_tier_refs, &cwd);
        // CLI-resolved mode is the highest-priority source (TS orderedModes:
        // the CLI flag / --permission-mode outranks the settings defaultMode).
        // Apply it only when the CLI actually requested a non-default mode, so
        // an unset CLI keeps the settings defaultMode computed above.
        if cfg.permission_mode != permission::PermissionMode::Default {
            mode = cfg.permission_mode;
        }
        // Auto-mode availability gate — claude-code `xms` mode-load downgrade
        // (`if(t==="auto"&&!P0())return"default"`). When the resolved mode is
        // `auto` but auto mode is unavailable (the `disableAutoMode` settings
        // killswitch, or the boot model does not support it), silently downgrade
        // to `default` so the session never boots INTO an unavailable auto mode.
        // The local denial circuit-breaker is fresh at boot; Statsig
        // remote-disable is a documented omission; provider is resolved as
        // `"firstParty"` (multi-provider mapping deferred — see
        // `permission::auto_gate`).
        if mode == permission::PermissionMode::Auto {
            let (gated, _reason) = permission::apply_auto_mode_gate(
                mode,
                &permission::AutoGateInputs {
                    disabled_by_settings: auto_mode_disabled,
                    circuit_broken: false,
                    model: cfg.default_model.clone(),
                    provider: "firstParty".to_string(),
                },
            );
            mode = gated;
        }
        // Construct in `Default` and apply the resolved boot `mode` LAST (below),
        // so the auto-mode dangerous-rule strip runs AFTER
        // `with_classify_all_shell` is set and therefore honors the
        // `autoMode.classifyAllShell` escalation on a session that BOOTS directly
        // into auto mode. (The availability gate `rule_is_available_in_mode` also
        // enforces the escalation at authorize time, so this ordering only keeps
        // the strip stash faithful — but it costs nothing and removes the
        // stale-flag foot-gun.)
        let mut policy = permission::PermissionPolicy::from_rules(
            permission::PermissionMode::Default,
            rules,
        )
            .with_roots(roots)
            .with_working_dirs(additional_working_dirs)
            .with_sandbox_runtime(sandbox_auto_allow)
            .with_managed_permission_rules_only(allow_managed_permission_rules_only)
            // `autoMode.classifyAllShell` escalation (`QOi()`): any tier enabling
            // it suspends every Bash/PowerShell allow rule in auto mode.
            .with_classify_all_shell(classify_all_shell)
            // TS `isBypassPermissionsModeAvailable` (2.1.211 permissionSetup):
            // `S = (n === "bypassPermissions" || o) && !g && !_` — available when
            // the session RESOLVED to bypass mode OR the explicit
            // `--allow-dangerously-skip-permissions` flag was passed, unless the
            // settings killswitch (`disableBypassPermissionsMode: "disable"`)
            // vetoes it. (`g`, the Statsig remote killswitch, is a documented
            // omission here like the other remote gates.)
            .with_bypass_available(
                (mode == permission::PermissionMode::BypassPermissions
                    || cfg.allow_dangerously_skip_permissions)
                    && !bypass_disabled,
            )
            // Enable PowerShell path-containment via a real `pwsh` parse
            // (claude-code `validatePowerShellCommandPaths`). Inert on hosts
            // without PowerShell — `SystemPwshParser` returns passthrough when
            // `pwsh`/`powershell` is not on PATH, exactly like claude-code.
            .with_pwsh_parser(std::sync::Arc::new(
                permission::powershell_parse::SystemPwshParser,
            ));
        policy.bypass_killswitch_active = bypass_disabled;
        // Auto-mode killswitch (`Bpa()`): the live `set_permission_mode` gate
        // refuses `auto` when any tier set `disableAutoMode: "disable"`.
        policy.auto_mode_disabled = auto_mode_disabled;
        // Apply the resolved boot mode now that every field (crucially
        // `classify_all_shell`) is set — this triggers the auto-mode
        // dangerous-rule strip with the escalation in effect. A no-op when `mode`
        // is `Default` (from == to).
        policy.set_mode(mode);
        // Resolve the active Read(deny) rules to search-exclude globs while
        // the policy is still in scope (before it moves into the gate).
        read_deny_exclude_globs = permission::read_deny_exclude_globs(&policy, &cwd);
        // FIX 1 (subagent pool): hand the policy's TOOL-WIDE deny names to the
        // subagent spawner so a blanket-denied tool is stripped from each
        // child's advertised pool too (claude-code `assembleToolPool` →
        // `filterToolsByDenyRules`). Set-once; only meaningful when there are
        // tool-wide deny rules (empty otherwise ⇒ no child-pool filtering).
        let _ = subagent_tool_wide_deny_cell.set(policy.tool_wide_deny_names());
        let policy = Arc::new(policy);
        // Share the boot policy into `tool_ctx` for the prompt shell-expansion
        // gate (clone the `Arc` BEFORE `policy` moves into the gate below).
        boot_permission_policy = Some(policy.clone());
        tracing::info!(
            rules = rule_count,
            mode = ?mode,
            "permission enforcement enabled (default on; disable with LINGXI_ENFORCE_PERMISSIONS=0)"
        );
        // Grab the LIVE-model cell BEFORE coercing to `Arc<dyn PermissionGate>`
        // (the concrete handle is only reachable pre-coercion); it is filled once
        // the orchestrator exists (below).
        let enforcing = permission::PolicyPermissionGate::new(policy, perms);
        live_model_provider_cell = Some(enforcing.live_model_provider_handle());
        Arc::new(enforcing)
    } else {
        // Enforcement off: the settings `additionalDirectories` tiers are not
        // loaded here, but the CLI `--add-dir` dirs still widen file-tool access
        // and the MCP roots (claude-code's `additionalWorkingDirectories` are
        // independent of permission mode).
        boot_additional_working_dirs = cfg.add_dir.clone();
        perms
    };
    // Capture the enforcing gate for the interactive TUI's Shift+Tab live
    // permission-mode cycling (`set_permission_mode`), before `perms` is moved
    // into the tool context below.
    let enforcing_permission_gate: Option<Arc<dyn PermissionGate>> = Some(perms.clone());

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
    // H-BIN-12: source the CC 2.1.207 HTTP-hook security policy
    // (`allowedHttpHookUrls` / `httpHookAllowedEnvVars`) from the merged settings
    // so the HTTP hook executor gates outbound URLs + intersects the per-hook
    // env-var allowlist. `(None, None)` = no restriction (behavior-neutral).
    let (http_hook_urls, http_hook_env_vars) = load_merged_http_hook_policy(&cwd);
    let hooks = Arc::new(
        hooks::HookExecutorImpl::new(
            hook_registry.clone(),
            http.clone(),
            hook_runtime as Arc<dyn traits::RuntimeSpawner>,
        )
        .with_http_hook_policy(http_hook_urls, http_hook_env_vars)
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
        .with_agent_spawner(subagent_spawner.clone())
        // P4: wire the output stream as hook observer so --include-hook-events
        // and the SessionStart/Setup always-stream gate emit hook_started /
        // hook_response frames. Default no-op when the stream impl ignores them
        // (TUI / plain sink paths).
        .with_hook_observer(output.clone()),
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
    let elicitation_dispatcher: Arc<dyn mcp::HookDispatcher> =
        Arc::new(orchestrator::OrchestratorHookDispatcher::new(
            hooks.clone(),
            cwd.clone(),
            main_transcript_path.clone(),
        ));
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
            cfg.lingxi_home.join("settings.json"),
            cwd.join(branding::DOT_DIR).join("settings.json"),
            cwd.join(branding::DOT_DIR).join("settings.local.json"),
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
    // LIVE additional-roots cell shared between the MCP registry (seeds every
    // server's `roots/list`) and the runtime `/add-dir` effect: pushing into it
    // via `mcp_registry.add_root(...)` is seen by every connected server on its
    // next `roots/list` without a reconnect (parity 2.1.207 P1-08).
    //
    // Seed with the EXPANDED absolute paths (same `expand_trusted_dir` the
    // file-tool `trusted_dirs` set uses below), NOT the raw settings/`--add-dir`
    // entries. `RootsListHandler::roots_value` forwards each dir verbatim into
    // `format!("file://{dir}")`, so a raw `~/shared` / relative `data` would
    // emit a malformed `file://~/shared` (authority `~`, non-resolvable) instead
    // of claude-code's `pathToFileURL(resolved)` = `file:///home/user/shared`.
    // Expanding here also keeps the MCP-roots cell and the trusted-dir set in
    // lock-step, so a later runtime `/add-dir <same abs path>` dedupes
    // identically in both surfaces (review RV3).
    let mcp_roots_seed: Vec<std::path::PathBuf> = {
        let home = dirs::home_dir();
        boot_additional_working_dirs
            .iter()
            .map(|raw| expand_trusted_dir(raw, &cwd, home.as_deref()))
            .collect()
    };
    let mcp_additional_roots = mcp::new_shared_roots(mcp_roots_seed);
    let mcp_registry = Arc::new(
        mcp::McpRegistry::with_raw_conn(
            posix.clone() as Arc<dyn McpTransport>,
            posix as Arc<dyn mcp::RawConnectionProvider>,
        )
        .with_hook_dispatcher(Some(elicitation_dispatcher))
        .with_oauth(mcp_oauth_deps)
        // Advertise the session's additional working dirs (settings
        // `additionalDirectories` + `--add-dir`) on every server's `roots/list`,
        // matching claude-code r1d() = [cwd, ...additionalWorkingDirectories].
        .with_additional_roots(mcp_additional_roots),
    );
    // Subscribe before connecting: a server is allowed to invalidate a catalog
    // immediately after initialization, before the shared ToolRegistry exists.
    // Tokio's broadcast receiver retains those early notifications until the
    // refresh driver below is installed.
    let mut mcp_catalog_changes = mcp_registry.subscribe_catalog_changes();
    mcp_registry.connect_all(mcp_configs).await;
    tokio::spawn(Arc::clone(&mcp_registry).run_reconnect_loop());
    // Clone handles the runtime `/add-dir` live effect needs (the same registry
    // Arc is moved into the orchestrator builder below via `with_mcp_registry`).
    let runtime_mcp_registry = mcp_registry.clone();

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
    let compaction_side_query: Arc<dyn sidequery::SideQueryClient> = Arc::new(
        sidequery::ProviderSideQueryClient::from_service(api_service.clone()),
    );
    let forked_runner = Arc::new(
        sidequery::ForkedAgentRunner::new()
            .with_side_query_client(compaction_side_query, orch_cfg.model.clone())
            // (M10 cc2.1.198) the compaction summary call INHERITS the session
            // extended-thinking config (binary: `thinkingConfig: mXt(r)` on the
            // summarizer `sEt` call @216945141). This is the SAME resolved
            // `cfg.session_thinking` the main-loop `ApiService` holds (boot
            // MAX_THINKING_TOKENS / --max-thinking-tokens / alwaysThinkingEnabled
            // resolution) — and the model predicates + `LINGXI_DISABLE_THINKING`
            // kill switches still apply per request inside `reasoning_for_request`.
            .with_session_thinking(cfg.session_thinking),
    );
    // `/recap` reuses the SAME single-turn forked runner the autocompact
    // summarizer uses — CLONE the `Arc` here BEFORE `forked_runner` moves into
    // the `Autocompactor` below, so recap replays the identical cache-safe
    // prefix (the same `cache_safe_slot` is already shared with both). Recap
    // reads the runner read-only; it never mutates history/slot.
    let recap_runner = forked_runner.clone();
    let autocompactor =
        compaction::Autocompactor::with_forked_runner(forked_runner, cache_safe_slot.clone());
    let compactor = Arc::new(compaction::CompactionOrchestrator::with_autocompactor(
        autocompactor,
        150_000,
    ));

    // (5.45) The real desktop `TaskRegistry`, wired into the tool context. Tasks
    //        materialize stdout/stderr under a SESSION-SCOPED project temp dir
    //        `<projectTempDir>/<sessionId>/tasks` (claude-code `getTaskOutputDir`,
    //        `diskOutput.ts:50-55`) instead of an in-repo `<cwd>/.lingxi/...`
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
    //
    // (M8 cc2.1.198 "Task panels: no stuck Running") The bash worker's
    // terminal status + exit code now write THROUGH to the registry via a
    // deferred `RegistryStatusSink` (bound at (5.46f) once the registry `Arc`
    // exists — the same cycle-break as the LocalAgent sink). Pre-fix the
    // handler defaulted to `NoopStatusSink`, so a finished background bash
    // task's stored status stayed `Running` forever.
    let bash_status_sink = Arc::new(tasks::registry_status_sink::RegistryStatusSink::new());
    tasks::registry::register_self_contained_handlers(
        &mut task_registry_inner,
        Arc::new(PosixProcess::new()),
        Arc::new(PosixSandbox::new()),
        mcp_registry.clone(),
        bash_status_sink.clone() as Arc<dyn tasks::handlers::TaskStatusSink>,
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
        let simple = coordinator::is_env_truthy(std::env::var("LINGXI_SIMPLE").ok().as_deref());
        let mut prompt = coordinator::coordinator_system_prompt(simple);
        // Connected MCP server names for the worker-tools user context.
        let mcp_names: Vec<String> = mcp_registry
            .snapshot()
            .await
            .into_iter()
            .map(|s| s.name)
            .collect();
        if let Some(user_ctx) = coordinator::coordinator_user_context(&mcp_names, None, simple) {
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
    // (opusplan + plan → Opus). Default mode → byte-identical to before. Uses
    // the post-fallback `model_setting_for_spawns` (same reasoning as the
    // spawner seam).
    .with_permission_mode(cfg.permission_mode)
    .with_model_setting(model_setting_for_spawns.clone())
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
    )))
    // Full teammate parity (P1): inherit the shared budget enforcer + fire
    // SubagentStart via the same `HookExecutorImpl` the orchestrator uses, and
    // stamp the SubagentStart `HookContext` (session id + cwd). `hooks` /
    // `budget_enforcer` already exist here (the teammate handler is built after
    // them), unlike the spawner's deferred cells. The advertised tool pool +
    // skills-preload registries are filled via handles below (they don't exist
    // yet). This makes a teammate a full team worker (tools + budget + hooks),
    // not a chat-only stub.
    .with_budget_enforcer(budget_enforcer.clone())
    .with_hook_executor(hooks.clone())
    .with_hook_context(subagent_hook_session_id, cwd.clone());
    // Grab the teammate handler's set-once cells BEFORE boxing, to fill once the
    // tool registry / skill loader exist (same deferred-fill the spawner uses).
    let teammate_tool_registry_cell = teammate_handler.tool_registry_handle();
    let teammate_skill_loader_cell = teammate_handler.skill_loader_handle();
    let teammate_tool_wide_deny_cell = teammate_handler.tool_wide_deny_names_handle();
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
    // ONE worktree manager shared by the AgentTool (which CREATES the isolation
    // worktree + judges it on the SYNC path) and the LocalAgent handler (which
    // judges it when a BACKGROUND agent reaches a terminal state — claude-code's
    // `getWorktreeResult` closure handed to the detached lifecycle).
    let worktree_manager: Arc<dyn traits::worktree::WorktreeManager> =
        Arc::new(PosixWorktreeManager::new(cwd.clone()));
    // The forked-skill resume gate. Its skill resolver is bound LATER (the
    // command registry does not exist yet — the same registration cycle the
    // status sink solves); until then it reports "not fork-capable", which
    // REFUSES rather than waving a forked skill through.
    let fork_capable_skills = Arc::new(fork_resume::RegistryForkCapableSkills::new());
    let fork_resume_gate = Arc::new(fork_resume::DesktopForkResumeGate {
        // Beside the agents' own transcripts (`agent-<id>.jsonl`), not in the
        // project session directory — that is where the writer puts them and
        // where anything keying off the real transcript will look.
        subagents_dir: main_subagents_dir.clone(),
        skills: fork_capable_skills.clone() as Arc<dyn fork_resume::ForkCapableSkills>,
    });
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
            )
            // Terminal keep/cleanup of a background agent's isolation worktree.
            .with_worktree_manager(worktree_manager.clone())
            // Refuse to resume a forked skill whose permission scoping cannot
            // be re-established — resuming one unscoped would run it under the
            // parent's (strictly wider) permissions.
            .with_fork_resume_gate(fork_resume_gate.clone()
                as Arc<dyn traits::fork_resume_gate::ForkResumeGate>)
            // Record each parked agent so a LATER process can rebuild it; the
            // record is erased the moment it terminates.
            .with_parked_agent_store(Arc::new(agent_restore::DesktopParkedAgentStore {
                subagents_dir: main_subagents_dir.clone(),
            })
                as Arc<dyn traits::parked_agent_store::ParkedAgentStore>),
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
    // Deferred status sink (bound to the registry Arc below) so the workflow
    // worker's terminal `set_status(Completed/Failed)` actually reaches the
    // registry — WITHOUT this the handler keeps the default `NoopStatusSink` and
    // a finished workflow is stuck on `Running` forever in `/workflows`. Same
    // "no stuck Running" wiring bash + local_agent already have.
    let local_workflow_status_sink =
        Arc::new(tasks::registry_status_sink::RegistryStatusSink::new());
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
            .with_turn_baseline_cell(local_workflow_turn_baseline.clone())
            .with_worktree_manager(worktree_manager.clone())
            .with_status_sink(
                local_workflow_status_sink.clone() as Arc<dyn tasks::handlers::TaskStatusSink>
            ),
        ),
    );

    let task_registry = Arc::new(task_registry_inner);

    // (5.46f) Bind the deferred LocalAgent status sink now that the registry
    //         `Arc` exists: the persistent agent's `set_status` / `notify_rest`
    //         now reach `task_registry`, so terminal + per-rest notifications
    //         surface through `take_pending_task_notifications`.
    local_agent_status_sink
        .bind(task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>);
    // (M8 cc2.1.198) Bind the deferred LocalBash sink too: the bash worker's
    // terminal `set_status` / `set_exit_code` now reach `task_registry`, so a
    // finished background command flips its panel row off `Running`.
    bash_status_sink
        .bind(task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>);
    // Bind the deferred LocalWorkflow sink: a finished workflow's terminal
    // `set_status(Completed/Failed)` now reaches `task_registry`, so `/workflows`
    // flips it off `Running` instead of showing it stuck forever.
    local_workflow_status_sink
        .bind(task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>);

    // (5.48) Cron: construct, load the single persisted tasks file, and start the
    //        live cron scheduler so jobs created by CronCreate actually fire —
    //        closing parity gap §0.3 / §B (the scheduler was never constructed, so
    //        persisted jobs never ran). 1:1 with claude-code `cronTasks.ts`: all
    //        durable jobs live in ONE project-relative file
    //        `<cwd>/.lingxi/scheduled_tasks.json` (the same project root the
    //        CronCreate/List/Delete tools key off via `BuiltinToolContext.workspace`,
    //        which is `cwd`). `load_persisted` reads `createdAt`/`lastFiredAt` in
    //        epoch ms; next-fire is COMPUTED at runtime from the cron string +
    //        `lastFiredAt ?? createdAt` (never persisted). Ticks every 60s on a
    //        posix RuntimeSpawner (D17). The detached tick task holds a self-clone
    //        of the scheduler, so it runs for the process lifetime without being
    //        stored on `DesktopRuntime`.
    //        Gated by the `LINGXI_DISABLE_CRON` local kill-switch
    //        (claude-code `prompt.ts:34/38` — the env override that wins over the
    //        GrowthBook fleet flag, which itself defaults on).
    if cron_scheduler_enabled(std::env::var("LINGXI_DISABLE_CRON").ok().as_deref()) {
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
        // Read the USER tier (lingxi_home/settings.json) separately so the
        // source-restricted `allowAppleEvents` resolution can consult it: CC honors
        // allowAppleEvents from user / managed / flag only, NOT project/local.
        let user_settings_raw = tokio::fs::read_to_string(cfg.lingxi_home.join("settings.json"))
            .await
            .ok();
        if let Some(raw) = &user_settings_raw {
            tiers.push(raw.clone());
        }
        for p in [
            cwd.join(branding::DOT_DIR).join("settings.json"),
            cwd.join(branding::DOT_DIR).join("settings.local.json"),
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
        let managed_tiers = crate::settings_watch::managed_settings_raw_tiers().await;
        tiers.extend(managed_tiers.iter().cloned());
        let refs: Vec<&str> = tiers.iter().map(String::as_str).collect();
        // allowManagedDomainsOnly / allowManagedReadPathsOnly: resolved from the
        // MANAGED tiers ONLY (per-source), then threaded onto the context so the
        // conversion overrides the merged allowlist when the flag is set.
        let (managed_allowed_domains, managed_read_paths) =
            managed_only_sandbox_overrides(&managed_tiers, &cwd);
        // allowAppleEvents: source-restricted to user / managed / flag (project &
        // local are IGNORED — CC parity @223928133). First-defined wins managed →
        // flag(none) → user; `None` leaves the default `false`.
        let allow_apple_events_override =
            apple_events_override(&managed_tiers, user_settings_raw.as_deref());
        // strictAllowlist: same source restriction (2.1.219).
        let strict_allowlist_override_v =
            strict_allowlist_override(&managed_tiers, user_settings_raw.as_deref());
        // Seed the `SandboxConvertContext` with the boot-resolvable hardening
        // paths so the settings/skills denyWrite defense actually fires
        // (sandbox-adapter.ts:225-299). Seeds with no boot analog
        // (cwd_settings_paths / worktree_main_repo_path / additional_md_dirs)
        // stay empty — see spec §5.
        let managed = crate::settings_watch::managed_settings_dir();
        // Preserve the lexical deny-write seeds in the session config. The
        // command path resolves them immediately before every sandboxed launch
        // (`BuiltinToolContext::effective_sandbox_runtime`), which covers both
        // symlinks present at boot and links created or retargeted later. If we
        // replaced a seed with its boot-time target here, a later retarget would
        // be impossible to observe because the original link path was lost.
        let deny_seed = |p: std::path::PathBuf| p.to_string_lossy().into_owned();
        let ctx = sandbox::policy_convert::SandboxConvertContext {
            lingxi_temp_dir: Some(lingxi_temp_dir()),
            settings_file_paths: vec![
                deny_seed(cfg.lingxi_home.join("settings.json")),
                deny_seed(cwd.join(branding::DOT_DIR).join("settings.json")),
                deny_seed(cwd.join(branding::DOT_DIR).join("settings.local.json")),
                deny_seed(managed.join("managed-settings.json")),
            ],
            managed_drop_in_dir: Some(deny_seed(managed.join("managed-settings.d"))),
            skills_dirs: vec![deny_seed(cwd.join(branding::DOT_DIR).join("skills"))],
            managed_allowed_domains,
            managed_read_paths,
            allow_apple_events_override,
            strict_allowlist_override: strict_allowlist_override_v,
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
            // Where a forked skill's scoping sidecars land — beside the
            // background agent's own transcript in this session's
            // `subagents/` directory.
            subagents_dir: Some(main_subagents_dir.clone()),
        });

    // (/sandbox) One shared fast-toggle cell, seeded from the config's
    // `enabled` flag (read BEFORE `sandbox_runtime_cfg` is moved into the
    // literal below). It is cloned into BOTH the bash tool's
    // `sandbox_enabled_override` (read per-command via
    // `effective_sandbox_runtime`) AND the TUI's `/sandbox` handle, so a live
    // toggle flips sandboxing for the session's next command. Whether
    // sandboxing physically engages still rides platform support (unchanged),
    // exactly as the config `enabled` flag does today.
    let sandbox_toggle = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        sandbox_runtime_cfg.enabled,
    ));
    // (`/sandbox` description fidelity) Capture the static config flags the TUI's
    // dynamic `/sandbox` description renders (claude-code `t`/`r`/`o`) BEFORE the
    // config is moved into `tool_ctx` below. `deps_ok` = claude-code
    // `checkDependencies().errors.length === 0` (the same `sandbox_deps` computed
    // above drives `sandbox_available`); `false` → the warning glyph. `managed`
    // (policy-lock) is still not modeled (default off), a documented residual.
    let sandbox_desc_auto_allow = sandbox_runtime_cfg.auto_allow_bash_if_sandboxed;
    let sandbox_desc_fallback = sandbox_runtime_cfg.are_unsandboxed_commands_allowed();
    let sandbox_desc_deps_ok = sandbox_deps.errors.is_empty();
    // File-tool trusted dirs = cwd FIRST, then every additional working dir
    // (settings `additionalDirectories` + `--add-dir`), expanded and deduped.
    // claude-code allows file tools (Read/Edit/Write/Glob/Grep/NotebookEdit)
    // inside `additionalWorkingDirectories`; without this they hard-error on any
    // `--add-dir` path (parity 2.1.207 P1-08).
    let trusted_dirs = {
        let home = dirs::home_dir();
        let mut dirs_vec = vec![cwd.clone()];
        for raw in &boot_additional_working_dirs {
            let expanded = expand_trusted_dir(raw, &cwd, home.as_deref());
            if !dirs_vec.contains(&expanded) {
                dirs_vec.push(expanded);
            }
        }
        dirs_vec
    };
    // Bound (not inlined) so the SAME `Arc<SessionCwd>` can also be handed to
    // the orchestrator below via `.with_session_cwd(...)` — Task 5 (worktree
    // 206 session-cwd plumbing): the system prompt's `Primary working
    // directory:` line and the conditional-rules memory cache must see the
    // SAME cwd cell the FS/Bash tools swap on `EnterWorktree`/`ExitWorktree`,
    // not an independent, never-swapped cell. Seeded with the P1-08
    // `trusted_dirs` set (cwd + `--add-dir`/`additionalDirectories`) so the
    // file tools' `ctx.trusted_dirs()` gate keeps allowing the additional dirs.
    let session_cwd = SessionCwd::new(cwd.clone(), trusted_dirs);
    // Handle the runtime `/add-dir` live effect needs to widen the file-tool
    // trusted set (the same `Arc<SessionCwd>` is moved into the orchestrator
    // builder below via `with_session_cwd`). P1-08.
    let runtime_session_cwd = session_cwd.clone();
    // (parity 2.1.212) Restore a persisted active worktree on --continue/--resume.
    // The `worktree_session` cell (written by EnterWorktree, cleared by
    // ExitWorktree) is in-memory only; on a cold resume it starts `None`, so
    // ExitWorktree would take its no-op path ("No-op: there is no active
    // EnterWorktree session to exit") even for a session that was inside a
    // worktree. Read the last `worktree-state` transcript record for this session
    // and, when it names a worktree still on disk, rehydrate the cell + move the
    // session into it (`session_cwd` swap) — the Rust analog of claude's
    // `restoreWorktreeSession`/`Z_t`. A missing worktree dir (removed since)
    // restores nothing, so the session stays out of the worktree and ExitWorktree
    // correctly no-ops — mirroring `Z_t`'s chdir-failure `gne(null)` guard.
    // (The upstream-reset optimization `[worktree] reset resumed worktree` and the
    // cross-project `tengu_resume_worktree_fallback` search are deferred
    // refinements — not needed to close the ExitWorktree-after-resume no-op.)
    let worktree_session_cell = tool_api::worktree_session::new_worktree_session_cell();
    if cfg.session_id_override.is_some() {
        let restore_fs: Arc<dyn traits::FileSystem> = Arc::new(PosixFileSystem::new(cwd.clone()));
        if let Some(payload) = session::jsonl::loader::read_worktree_state(
            &main_transcript_path,
            restore_fs,
            &main_session_uuid,
        )
        .await
        {
            if let Some(restored) = tool_api::WorktreeSession::from_persisted_json(&payload) {
                if restored.worktree_path.is_dir() {
                    // Move the session into the worktree exactly as EnterWorktree's
                    // own swap does (worktree path as the sole trusted dir).
                    session_cwd.swap(
                        restored.worktree_path.clone(),
                        vec![restored.worktree_path.clone()],
                    );
                    *worktree_session_cell
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(restored);
                }
            }
        }
    }
    // P1-06: ONE per-session read-file-state registry (claude-code's single
    // `readFileState` map on the `ToolUseContext`). Created here, cloned into
    // every file tool's `BuiltinToolContext` below, and the SAME `Arc` handed to
    // the orchestrator via `.with_read_state_map(...)` at the builder chain, so a
    // tool's `readFileState.set` feeds the orchestrator's post-compact restore
    // (and the staleness / `/files` consumers).
    let read_state_map = tool_api::read_file_state::new_read_file_state_map();
    let tool_ctx = BuiltinToolContext {
        // FILE.B / P1-06: file tools share the ONE per-session read-state map
        // (staleness guard, Read-dedup) — the SAME `Arc` the orchestrator adopts
        // via `.with_read_state_map(read_state_map)` below.
        read_file_state: read_state_map.clone(),
        // Read(deny) → Grep/Glob search excludes (resolved from the boot policy
        // above; empty when enforcement is off or no Read-deny rule applies).
        read_deny_exclude_globs,
        fs: Arc::new(PosixFileSystem::new(cwd.clone())),
        bus: Arc::new(telemetry::AnalyticsBus::new()),
        // P1-08 trusted dirs now live inside `session_cwd` (built above from the
        // `trusted_dirs` set); `BuiltinToolContext` no longer has a standalone
        // `trusted_dirs` field — tools read `ctx.trusted_dirs()` off `session_cwd`.
        process: Arc::new(PosixProcess::new()),
        sandbox: Arc::new(PosixSandbox::new()),
        clock: clock.clone(),
        sandbox_runtime: sandbox_runtime_cfg,
        // (/sandbox) Live-toggle cell shared with the TUI (see above).
        sandbox_enabled_override: Some(sandbox_toggle.clone()),
        // (P2-14) `settings.skipWebFetchPreflight` → WebFetch skips the
        // domain-blocklist preflight (enterprise escape hatch). Read from the
        // merged settings via the same `Settings::load` seam as outputStyle.
        skip_web_fetch_preflight: load_merged_skip_web_fetch_preflight(&cwd),
        // (M-15) `settings.askUserQuestionTimeout` → the AskUserQuestion resolver's
        // idle window. Read from the merged settings via the same `Settings::load`
        // seam; parsed into `AskUserQuestionTimeout` at `tool_ui` registration.
        ask_user_question_timeout: load_merged_ask_user_question_timeout(&cwd),
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
        // (#3 shell-expansion) The base policy for embedded `!`cmd`` bodies in
        // prompt commands (`/commit` …). When enforcement is on this is the SAME
        // boot `Arc<PermissionPolicy>` the model-facing `PolicyPermissionGate`
        // enforces (captured above). When enforcement is off no boot policy was
        // built, so fall back to a Default-mode policy WITH roots (from `cwd`) so
        // read-only auto-allow AND per-command allowed-tools injection still
        // content-match — 1:1 with claude-code, which runs the orchestrator gate
        // regardless of any local enforcement toggle.
        permission_policy: boot_permission_policy.clone().unwrap_or_else(|| {
            Arc::new(
                permission::PermissionPolicy::new(cfg.permission_mode)
                    // Same TS formula as the enforced boot policy above; no
                    // settings were read on this path, so no killswitch term.
                    .with_bypass_available(
                        cfg.permission_mode == permission::PermissionMode::BypassPermissions
                            || cfg.allow_dangerously_skip_permissions,
                    )
                    .with_roots(permission::FsRoots {
                        cwd: cwd.clone(),
                        home: dirs::home_dir(),
                        lingxi_home: cfg.lingxi_home.clone(),
                    })
                    .with_pwsh_parser(std::sync::Arc::new(
                        permission::powershell_parse::SystemPwshParser,
                    )),
            )
        }),
        sandbox_available,
        session_cwd: session_cwd.clone(),
        // Worktree 206 parity (Task 8): the session record cell — normally a
        // fresh `None` (inert until `EnterWorktree` populates it), but on a
        // `--continue`/`--resume` it may have just been rehydrated from the
        // persisted `worktree-state` transcript record above (parity 2.1.212), so
        // ExitWorktree operates on the resumed worktree instead of no-oping.
        worktree_session: worktree_session_cell,
        platform: sandbox_platform,
        http: http.clone(),
        provider: tool_provider,
        default_model: orch_cfg.model.clone(),
        web_search_config: Some(Arc::new(DesktopWebSearchConfigProvider {
            lingxi_home: cfg.lingxi_home.clone(),
            credentials: credentials.clone(),
        })),
        // The SAME manager the LocalAgent handler judges background-agent
        // worktrees with (created above) — one creation/judgment surface.
        worktree: worktree_manager.clone(),
        subagent_spawner: Some(subagent_spawner.clone()),
        task_registry: Some(
            task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>
        ),
        mailbox_router: coordinator_mailbox,
        budget_enforcer: Some(budget_enforcer.clone()),
        coordinator_mode: Some(
            coordinator_mode.clone() as Arc<dyn traits::coordinator_mode::CoordinatorModeHandle>
        ),
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
        computer_control: platform_macos_computer_control::new_if_supported(),
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
    // (worktree-tmux-launch plan, Task 3) `-w`/`--worktree [name]` boot
    // launch: create + enter a git worktree BEFORE anything downstream reads
    // `tool_ctx.session_cwd`/`tool_ctx.worktree_session` (the orchestrator's
    // `.with_session_cwd(session_cwd)` below shares the SAME `Arc`, and the
    // system prompt / conditional-rules cache re-derive from it per-turn, so
    // the exact position of this call relative to that wiring is immaterial
    // — only that it happens before any tool call, which it trivially does
    // here at boot). INERT INVARIANT: `cfg.worktree_launch == None` (every
    // caller except a CLI session with `-w`/`--worktree` set) is a no-op — no
    // create, no swap, `worktree_session` stays the fresh `None` set above —
    // so boot is byte-identical to before this field existed. A `--worktree`
    // that cannot be created is a HARD boot failure, not a silent degrade to
    // the plain cwd — the user explicitly asked for an isolated worktree.
    // (Task 4) `cfg.tmux_launch` additionally creates a detached tmux session
    // for that worktree — independently inert when `None` (see the function
    // doc); a tmux failure is logged, not a hard boot failure.
    apply_worktree_launch(&cfg.worktree_launch, &cfg.tmux_launch, &tool_ctx).await?;
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
    //        The MCP partition supports live replacement after the registry is
    //        Arc-wrapped, so an inbound `notifications/tools/list_changed` can
    //        update the next model request without rebuilding the builtin pool.
    //        `tool_ctx` is consumed by `register_desktop_tools`, so the builder
    //        gets a clone taken first.
    let mcp_tool_ctx = tool_ctx.clone();
    // (`!` bash mode) Clone the session tool context for the TUI's sandboxed Bash
    // runner BEFORE `tool_ctx` is moved into `register_desktop_tools` below. The
    // runner builds a `tool_shell::BashTool` over this exact context, so a typed
    // `!command` runs through the SAME sandbox path as a model-issued Bash call.
    let bash_runner: Arc<dyn tui_core::bash_runner::BashRunner> = Arc::new(DesktopBashRunner {
        ctx: tool_ctx.clone(),
    });
    // (#3 shell-expansion) Build the shared prompt shell-expansion provider from
    // the SAME `tool_ctx` (carrying the base `permission_policy` + sandbox/process
    // seams) BEFORE `tool_ctx` is moved into `register_desktop_tools` below. One
    // `Arc<dyn ShellExpansionProvider>` is chained onto the dispatcher (so
    // `/commit` … expand their embedded `!`git …`` bodies) AND stashed on
    // `DesktopRuntime.shell_expansion` for the ratatui TUI's `run_core_command`.
    let shell_expansion_provider = tool_skill::build_prompt_shell_provider(&tool_ctx);
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
    // Bind the forked-skill resume gate's skill resolver to the SAME `Arc` that
    // is filled with the real registry at (6). Binding the slot (not its
    // contents) is what makes the deferral safe: the gate reads through it at
    // resume time, long after it is populated.
    fork_capable_skills.bind(shared_command_registry.clone());
    let plugin_output_style_registry =
        Arc::new(RwLock::new(outputstyles::OutputStyleRegistry::new()));
    // SKILLEXEC: the per-session id stamped onto every resolved skill descriptor
    // so the `Skill` tool substitutes `${LINGXI_SESSION_ID}` in the body (TS
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
    let skill_loader_arc: Arc<dyn traits::skill_loader::SkillLoader> =
        Arc::new(agent_skill_loader::AgentSkillLoader::new(
            shared_command_registry.clone(),
            Some(skill_session_id),
        ));
    let _ = subagent_skill_loader_cell.set(skill_loader_arc.clone());
    // Same skills-preload loader for the in-process teammate (full parity).
    let _ = teammate_skill_loader_cell.set(skill_loader_arc);
    // Fire the `CwdChanged` hook (claude-code `onCwdChangedForHooks`,
    // Shell.ts:409) when a `cd` inside a Bash call moves the persistent shell
    // cwd. The firer wraps the SAME `Arc<HookExecutorImpl>` the orchestrator
    // fires its other hooks through (mirrors the `TaskCreated` / `TaskCompleted`
    // firers), so the `tool-shell` leaf reaches `orch.hooks` without a dependency
    // cycle. Injected here (the desktop composition root) only — `engine-mobile`
    // never registers the shell tools, so the mobile path keeps the no-firer
    // BashTool.
    // Shared mutable-cwd cell: the firer writes the new dir on each Bash `cd`,
    // and the orchestrator (below, via `with_current_cwd`) reads it for hook
    // payloads — so a PreToolUse/PostToolUse/lifecycle hook sees the post-`cd`
    // directory, 1:1 with claude-code's single global `getCwd()`/`setCwdState`.
    let current_cwd_cell = std::sync::Arc::new(std::sync::Mutex::new(cwd.clone()));
    // Mirror worktree enter/exit swaps into this shared live-cwd cell so a
    // file/Glob/Grep read *between* a `session_cwd.swap(..)` and the next Bash
    // call (which re-points its own copy) sees the post-swap cwd, not the stale
    // pre-swap one. Bash still owns intra-turn `cd` updates to the same cell.
    session_cwd.link_live_cwd(current_cwd_cell.clone());
    // Clone for the Stop/SubagentStop hook snapshot provider (it locates the
    // project-root cron file via the live cwd); the original cell is moved into
    // `.with_current_cwd(...)` below.
    let current_cwd_cell_for_snapshot = current_cwd_cell.clone();
    // Watcher-rebind half of claude-code's `onCwdChanged`: a late-bound rebinder
    // handed to the `CwdChanged` firer NOW, its inner cell filled after the
    // file-changed watcher spawns below (the firer is built before the watcher).
    // On a mid-session `cd` the firer signals this to re-resolve the `FileChanged`
    // matchers against the new cwd and restart. Stays a no-op when no watcher
    // spawns (no `FileChanged` hooks). The clone the firer holds shares the same
    // cell as `file_changed_watcher_rebinder`, so the later `set` reaches it.
    let file_changed_watcher_rebinder = file_changed_watch::DeferredWatcherRebinder::new();
    let cwd_changed_firer: hooks::OptionalCwdChangedFirer = Some(Arc::new(
        orchestrator::OrchestratorCwdChangedFirer::new(
            hooks.clone(),
            cwd.clone(),
            main_transcript_path.clone(),
            current_cwd_cell.clone(),
        )
        .with_watcher_rebinder(Arc::new(file_changed_watcher_rebinder.clone())),
    ));
    // (parity 2.1.212) The worktree-state persister: a JSONL writer at the SAME
    // `main_transcript_path` the orchestrator persists messages to, so an
    // `EnterWorktree`/`ExitWorktree` `worktree-state` record lands in the one
    // session `<uuid>.jsonl` the resume loader reads. Gated on
    // `session_persistence` (no writer ⇒ nothing to resume from), mirroring the
    // `main_jsonl_writer` wiring below.
    let worktree_state_persister: Option<Arc<dyn tool_api::WorktreeStatePersister>> =
        if cfg.session_persistence {
            Some(Arc::new(JsonlWorktreeStatePersister {
                writer: Arc::new(session::jsonl::writer::JsonlWriter::new(
                    main_transcript_path.clone(),
                    Arc::new(PosixFileSystem::new(cwd.clone())) as Arc<dyn traits::FileSystem>,
                )),
                session_uuid: main_session_uuid.clone(),
            }))
        } else {
            None
        };
    // Keep a slash-command façade over the SAME context + persister before the
    // tool registry consumes `tool_ctx`. The handler itself is registered only
    // in the desktop command registry below, leaving the locked upstream
    // command-api builtin table untouched.
    let worktree_command_handler: Arc<dyn BuiltinCommandHandler> = Arc::new(
        DesktopWorktreeCommandHandler::new(tool_ctx.clone(), worktree_state_persister.clone()),
    );
    // The wakeup cell for the registered `ScheduleWakeup` tool — surfaced on
    // `DesktopRuntime` so the bridge composition root fills it once the
    // per-connection queue + spawner exist (`boot::assemble`).
    let ask_user_question_resolver = cfg.ask_user_question_tx.clone().map(|tx| {
        Arc::new(tool_ui::ask_user_question::TuiBridgeResolver::new(
            tool_ui::ask_user_question::AskUserQuestionTimeout::parse_or_default(
                tool_ctx.ask_user_question_timeout.as_deref(),
            ),
            tx,
        )) as Arc<dyn tool_ui::ask_user_question::AskUserQuestionResolver>
    });
    let computer_access_resolver = cfg.computer_access_tx.clone().map(|tx| {
        Arc::new(tool_computer_use::TuiBridgeResolver::new(tx))
            as Arc<dyn tool_computer_use::ComputerAccessResolver>
    });
    let wakeup_scheduler_cell = register_desktop_tools(
        &mut tools_inner,
        tool_ctx,
        coordinator_wiring,
        ask_user_question_resolver,
        computer_access_resolver,
        Some(cron_auth),
        Some(skill_loader),
        cwd_changed_firer,
        Some(side_query_client.clone()),
        // (P2-08) The SAME shared live-cwd cell the `CwdChanged` firer and the
        // orchestrator (`.with_current_cwd`) hold, so the desktop `BashTool` is
        // the single writer of the live cwd Read/Glob/Grep + LSP read.
        Some(current_cwd_cell.clone()),
        worktree_state_persister,
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
                lingxi_home: cfg.lingxi_home.clone(),
                session_uuid: main_session_uuid.clone(),
            });
        // parity 2.1.207 "Dynamic workflow size": read the persisted
        // `workflowSizeGuideline` (`/config`) once at construction and freeze it
        // into the tool for the session — the binary's `St().workflowSizeGuideline`
        // fed through `Jvd`. It flavors the Workflow tool's prompt appendix.
        // Absent / unknown ⇒ `unrestricted` (no appendix), via `from_wire`.
        // `workflowSizeGuideline` may come from ANY settings file (2.1.219),
        // not just the user one; later tiers win. Absent everywhere ⇒ the
        // oracle's default `medium` (`_Td`), NOT unrestricted.
        let workflow_settings_files = [
            crate::settings_watch::managed_settings_dir().join("managed-settings.json"),
            cfg.lingxi_home.join("settings.json"),
            cwd.join(branding::DOT_DIR).join("settings.json"),
            cwd.join(branding::DOT_DIR).join("settings.local.json"),
        ];
        let read_setting = |key: &str| -> Option<serde_json::Value> {
            let mut found = None;
            for path in &workflow_settings_files {
                if let Some(v) = std::fs::read_to_string(path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .and_then(|v| v.get(key).cloned())
                {
                    found = Some(v);
                }
            }
            found
        };
        let workflow_size_guideline = read_setting("workflowSizeGuideline")
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .map_or_else(tool_workflow::WorkflowSizeGuideline::default, |s| {
                tool_workflow::WorkflowSizeGuideline::from_wire(s)
            });
        // `disableWorkflows` is an ORG policy, so it is read from MANAGED
        // settings only — a project or user file must not be able to turn the
        // tool off on the org's behalf, nor to turn it back on.
        let managed_disable_workflows = std::fs::read_to_string(
            crate::settings_watch::managed_settings_dir().join("managed-settings.json"),
        )
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("disableWorkflows").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);
        tools_inner.register_builtin(Arc::new(
            tool_workflow::WorkflowTool::new(Some(workflow_launcher))
                .with_size_guideline(workflow_size_guideline)
                .with_disable_workflows(managed_disable_workflows),
        ));
    }
    for (conn_id, mcp_tools) in
        tool_mcp::build_registered_mcp_tools(&mcp_registry, mcp_tool_ctx.clone()).await
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

    // EndConversation (2.1.206): register the tool + create the shared
    // end-request slot when the feature is enabled. claude-code
    // `isEndConversationToolEnabled` (`NWn`) = `modelMeetsEndConversationFloor`
    // (`fJc`, the `LJh` version table) AND the `tengu_umber_kestrel` GB flag.
    // The LingXi equivalent of the GB flag — like the memdir-prefetch gate above
    // — is a default-OFF env flag; the model floor is enforced faithfully via
    // `meets_end_conversation_floor`. Either half failing (default: env unset)
    // leaves the registry + turn loop byte-identical
    // (`end_conversation_slot.is_none()`).
    let end_conversation_enabled = is_env_truthy("LINGXI_END_CONVERSATION")
        && orchestrator::prompt::end_conversation::meets_end_conversation_floor(&orch_cfg.model);
    let end_conversation_slot: Option<orchestrator::end_conversation_tool::EndConversationSlot> =
        end_conversation_enabled.then(|| {
            let slot: orchestrator::end_conversation_tool::EndConversationSlot =
                Arc::new(std::sync::atomic::AtomicBool::new(false));
            tools_inner.register_builtin(Arc::new(
                orchestrator::end_conversation_tool::EndConversationTool::new(true, slot.clone()),
            ));
            slot
        });

    // Tool Search (2.1.207): now that the registry is fully assembled (builtins
    // + workflow + MCP + structured-output + end-conversation), publish the
    // DEFERRED tool set to `ToolSearch`'s live view cell. When tool search is
    // disabled (the default) the deferred set is empty, so this leaves the view
    // empty — the correct behavior — and the wire stays byte-identical.
    tools_inner.refresh_tool_search_view();

    let tools = Arc::new(tools_inner);

    // MCP servers can mutate their tool/prompt/resource catalogs while the
    // session is running. Refresh the registry snapshot on every generation-
    // checked notification; only a successful tools/list replaces the live
    // ToolRegistry partition, so transient RPC failures keep the last-known
    // tools available (the Claude Code behavior).
    //
    // Capture the MCP registry weakly. A strong Arc here would keep its own
    // broadcast sender alive forever and prevent this task from terminating
    // when the desktop runtime is dropped.
    {
        let mcp_registry_weak = Arc::downgrade(&mcp_registry);
        let live_tools = tools.clone();
        let live_mcp_tool_ctx = mcp_tool_ctx;
        tokio::spawn(async move {
            let mut recovery = std::collections::VecDeque::new();
            loop {
                let change = if let Some(change) = recovery.pop_front() {
                    change
                } else {
                    match mcp_catalog_changes.recv().await {
                        Ok(change) => change,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            let Some(registry) = mcp_registry_weak.upgrade() else {
                                break;
                            };
                            tracing::warn!(
                                target: "lingxi_engine_desktop::mcp",
                                skipped,
                                "MCP catalog refresh receiver lagged; refreshing every connected catalog"
                            );
                            recovery.extend(registry.catalog_refresh_snapshot().await);
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                };
                let Some(registry) = mcp_registry_weak.upgrade() else {
                    break;
                };

                if let Some(retired) = change.retired_connection_id {
                    live_tools.unregister_mcp_tools(retired);
                    live_tools.refresh_tool_search_view();
                }

                tracing::debug!(
                    target: "lingxi_engine_desktop::mcp",
                    server = %change.server_name,
                    catalog = ?change.kind,
                    "Received MCP list_changed notification, refreshing catalog"
                );
                match registry.refresh_catalog(&change).await {
                    Ok(Some(connection_id)) if change.kind == mcp::McpCatalogKind::Tools => {
                        let refreshed = tool_mcp::build_registered_mcp_tools(
                            registry.as_ref(),
                            live_mcp_tool_ctx.clone(),
                        )
                        .await;
                        if let Some((_, handles)) =
                            refreshed.into_iter().find(|(id, _)| *id == connection_id)
                        {
                            live_tools.register_mcp_tools(connection_id, handles);
                            live_tools.refresh_tool_search_view();
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            target: "lingxi_engine_desktop::mcp",
                            server = %change.server_name,
                            catalog = ?change.kind,
                            %error,
                            "Failed to refresh MCP catalog; keeping the previous catalog"
                        );
                    }
                }
            }
        });
    }

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
        tool_api::tool_invoker_impl::RegistryToolInvoker::new(tools.clone())
            .with_gate(perms.clone()),
    ));

    // (5.5a-local-agent) T15: bind the `LocalAgent` handler's `DeferredToolInvoker`
    //        to the real `RegistryToolInvoker` now that `tools` exists — same
    //        recursion-lock invariant and boot gate as the teammate + dream
    //        invokers above. A `LocalAgent` task's child runner dispatches its
    //        tools through the parent registry.
    local_agent_invoker.set(Arc::new(
        tool_api::tool_invoker_impl::RegistryToolInvoker::new(tools.clone())
            .with_gate(perms.clone()),
    ));

    // (5.5a-local-workflow) Bind the `LocalWorkflow` handler's `DeferredToolInvoker`
    //        to the real `RegistryToolInvoker` now that `tools` exists — so a
    //        workflow's `agent()` subagents dispatch their tools through the
    //        parent registry under the same recursion-lock + boot gate.
    local_workflow_invoker.set(Arc::new(
        tool_api::tool_invoker_impl::RegistryToolInvoker::new(tools.clone())
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
    // In-process teammate full parity (P1): advertise the SAME resolved tool pool
    // + apply the SAME tool-wide deny filter as the spawner, so a teammate can
    // actually use tools (not chat-only). The deny names are copied from the
    // spawner's already-filled cell (set in the enforcement branch above; empty /
    // unfilled ⇒ no filtering).
    let _ = teammate_tool_registry_cell.set(tools.clone());
    if let Some(deny) = subagent_tool_wide_deny_cell.get() {
        let _ = teammate_tool_wide_deny_cell.set(deny.clone());
    }

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
    // Gap #5: PERSIST the interactive session to JSONL so `--resume` / `-c` / the
    // resume screen (all backed by `session::jsonl::loader`, which scans
    // `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`) can find sessions
    // this desktop/CLI TUI itself created. Prior to this the production
    // composition root wired NO `JsonlWriter` (every `with_jsonl_writer` call
    // site was a test), so the projects dir stayed empty and resume never found
    // a TUI-created session. We point the writer at the SAME `main_transcript_path`
    // (`<cfg.claude_home>/projects/<sanitize(cwd)>/<main_session_uuid>.jsonl`)
    // already computed (FIX A) for the hook payloads' `transcript_path` and the
    // leaf firers, so the on-disk transcript, the hook `transcript_path`, and the
    // orchestrator's live session id are one consistent file end-to-end. The
    // writer creates the file (mode 0o600) + project dir (mode 0o700) lazily on
    // the first append; the orchestrator's existing per-block / per-message
    // persist machinery (`persist_assistant_per_block`,
    // `persist_message_to_jsonl_with_parent`) then appends user/assistant lines
    // the loader counts as a resumable session (title falls back to the first
    // user message). `PosixFileSystem` does not confine `append_file_with_mode`
    // to its workspace root, so rooting it at `watch_cwd` is fine for a path
    // under `claude_home`.
    let main_jsonl_writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
        main_transcript_path.clone(),
        Arc::new(PosixFileSystem::new(watch_cwd.clone())) as Arc<dyn traits::FileSystem>,
    ));
    // (P2-02 cc2.1.207) `main_jsonl_writer` MOVES into the orchestrator builder
    // below (when `session_persistence`); capture a clone so the `--agent` block
    // can persist the applied `agentType` as an `agent-setting` transcript record
    // (claude `{type:"agent-setting",agentSetting,sessionId}`) for `rVe` resume
    // restoration. Same shared-`Arc` file target, so the record lands in the SAME
    // `<uuid>.jsonl` the orchestrator appends messages to.
    let main_agent_setting_writer = main_jsonl_writer.clone();
    // (review #12) Resolve the `/goal` accept-gate values BEFORE `cwd` is moved
    // into the orchestrator. These feed the previously-unwired
    // `with_workspace_trusted` / `with_hooks_restricted` builders so `/goal`
    // honors claude's `Xys()` gate — rejected in an untrusted workspace or when
    // hooks are restricted. Both resolve FAIL-SAFE: a missing global config or a
    // load failure yields `untrusted` / `not-restricted`, so `/goal` is BLOCKED
    // rather than falsely granted. A session that accepted the trust dialog has a
    // recorded disk grant (`record_trust_accept`), so normal sessions stay
    // trusted; homedir sessions short-circuit via session-trust.
    let goal_workspace_trusted = migrations::global_config::global_config_path()
        .map(|cfg| migrations::global_config::check_has_trust_dialog_accepted(&cfg, &cwd))
        .unwrap_or(false);
    let goal_hooks_restricted = load_merged_hooks_restricted(&cwd);
    let orch_builder = ConversationOrchestrator::new_with_streaming(
        orch_cfg,
        api_client,
        streaming_api,
        tools,
        hooks,
        perms,
        output,
        memory,
        cwd,
    );
    // Gap #5: wire the production JSONL writer (constructed just above) so the
    // session is persisted + discoverable by the resume loader.
    // (M3 cc2.1.198) `--no-session-persistence` ⟶ `cfg.session_persistence:
    // false`: leave the orchestrator's `jsonl_writer` slot `None` (its persist
    // paths are already `Option`-gated) so NO transcript is written under
    // `projects/` and the session cannot be resumed.
    let orch_builder = if cfg.session_persistence {
        orch_builder.with_jsonl_writer(main_jsonl_writer)
    } else {
        orch_builder
    };
    let orch_builder = orch_builder
        // FIX A: hand the orchestrator the resolved claude-home so its hook payloads
        // carry a deterministically-computed `transcript_path`
        // (`<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`, claude-code
        // `getTranscriptPathForSession`). This is the SAME path the Gap #5
        // `JsonlWriter` (wired just above) persists to, so the hook payload path and
        // the on-disk transcript agree. Without this every PreToolUse /
        // PostToolBatch / lifecycle hook fired with an empty path.
        // (/fast) Share the same fast-mode flag the adapter reads, so the
        // `set_fast_mode` handle flips the value the next request-build sees.
        .with_fast_mode(fast_flag.clone())
        // (/rewind) Share the file-history store so the turn loop snapshots each
        // turn + the write tools back up pre-edit content.
        .with_file_history(file_history.clone())
        .with_config_home(cfg.lingxi_home.clone())
        // Share the SAME mutable-cwd cell the `cwd_changed_firer` writes on a Bash
        // `cd`, so hook payloads read the post-`cd` directory (claude-code parity).
        .with_current_cwd(current_cwd_cell)
        // Task 5 (worktree 206 session-cwd plumbing): share the SAME
        // `Arc<SessionCwd>` the tool context swaps on `EnterWorktree`/
        // `ExitWorktree`, so the system prompt's `Primary working directory:`
        // line and the conditional-rules memory cache re-derive from the
        // post-swap worktree cwd instead of the frozen boot cwd.
        .with_session_cwd(session_cwd)
        // FIX A/B/C: adopt the boot-canonical session id so the orchestrator's LIVE
        // session matches the id baked into the leaf firers' `transcript_path` and the
        // subagent spawner's subagents dir — one consistent session id end-to-end.
        .with_session_id(main_session_id)
        .with_cost_tracker(cost_tracker)
        // (review #12) Wire the /goal trust + hooks-restricted gates (resolved
        // above) into the orchestrator, replacing the hardcoded trusted=true /
        // restricted=false defaults.
        .with_workspace_trusted(goal_workspace_trusted)
        .with_hooks_restricted(goal_hooks_restricted)
        .with_analytics_bus(analytics_bus)
        .with_mcp_registry(mcp_registry.clone())
        .with_hook_registry(hook_registry)
        .with_agent_catalog(agent_catalog)
        .with_output_style_registry(plugin_output_style_registry.clone())
        .with_compaction(compactor)
        .with_cache_safe_slot(cache_safe_slot)
        // `/fork` engine seam: hand the orchestrator the background-agent spawner
        // (`BackgroundAgentSpawner`, built above) + the budget the spawned agent
        // inherits, so `fork_conversation` can dispatch a detached background agent.
        .with_fork_spawner(subagent_spawner.clone())
        .with_fork_budget(budget_enforcer.clone())
        // `/recap` engine seam: the SAME forked runner the summarizer uses (cloned
        // above), so recap replays the identical cache-safe prefix, read-only.
        .with_recap_runner(recap_runner)
        // Surface LSP `<new-diagnostics>` to the model each turn (the same sink the
        // LSP registry drains publishDiagnostics into).
        .with_new_diagnostics_source(
            Arc::new(lsp_diagnostics.clone()) as Arc<dyn traits::NewDiagnosticsSource>
        )
        // SKILLLIST.1: enumerate model-invocable skills each turn so the model
        // can discover them. Reads `shared_command_registry` lazily at turn time
        // (populated below at (6), before any turn fires).
        .with_skill_listing(Arc::new(RegistrySkillListing(
            shared_command_registry.clone(),
        )))
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
        // hook-bg-fields: populate the `Stop` / `SubagentStop` hook payload's
        // `background_tasks` (claude-code `Lic(taskRegistry.all())`) +
        // `session_crons` (claude-code `Mic()`) from the SAME live `TaskRegistry`
        // Arc wired above plus the project-root `.lingxi/scheduled_tasks.json` cron
        // file (located via the shared `current_cwd` cell). The orchestrator stamps
        // the snapshot onto the payload ONLY at its Stop / SubagentStop firings
        // (claude's tool-use-context `s` gate).
        .with_stop_hook_snapshot(Arc::new(RegistryStopHookSnapshot {
            registry: task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>,
            current_cwd: current_cwd_cell_for_snapshot,
        }))
        // Finding #73: supply the V2 task list to the per-turn `task_reminder`
        // (the default variant when tasks are enabled). Reads the file-backed
        // `TodoStore` for the active list each turn, resolving the list id via the
        // same env/team precedence the `Task*` tools use. V1 (`todo_reminder`)
        // needs no provider; it reads `session.todos` directly.
        .with_todo_reminder_tasks(Arc::new(orchestrator::TodoStoreReminderTasks::new()))
        // P1-06: hand the orchestrator the SAME `readFileState` map the file tools'
        // `BuiltinToolContext` share (created just above), so a tool's
        // `readFileState.set` feeds the post-compact file restore + staleness /
        // `/files` consumers — 1:1 with claude-code's single per-session map.
        .with_read_state_map(read_state_map);

    // P0.1 ACTIVATION (gated, default OFF). When `LINGXI_MEMDIR_PREFETCH`
    // is truthy, wire the memdir-backed memory selector so relevant
    // `~/.lingxi/memdir` entries surface each turn (a Haiku-class side query per
    // turn over `side_query_client`). The composition-root presence of the
    // prefetch IS the gate — claude-code keeps this behind `tengu_moth_copse`
    // (default false), so unset/false leaves the surfacing channel inert and the
    // locked fixtures byte-identical (`memory_prefetch.is_some() == false`).
    let orch_builder = match (is_env_truthy("LINGXI_MEMDIR_PREFETCH"), dirs::home_dir()) {
        (true, Some(home)) => {
            orch_builder.with_memory_prefetch(orchestrator::prompt::build_memdir_prefetch(
                side_query_client.clone(),
                Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>,
                &home,
            ))
        }
        _ => orch_builder,
    };

    // EndConversation: hand the orchestrator the SAME end-request slot the tool
    // holds, so the turn loop can terminate on a confirmed (2nd) call. `None`
    // (feature disabled, the default) leaves the turn loop byte-identical.
    let orch_builder = match end_conversation_slot.clone() {
        Some(slot) => orch_builder.with_end_conversation_slot(slot),
        None => orch_builder,
    };

    // 2.1.212 `/fork` (`vAd`) background-session forker seam. When the host
    // (`apps/cli`) injects one, wire it so `fork_to_background_session` copies
    // the live conversation into a new background session. `None` (default /
    // non-CLI hosts) leaves that `/fork` variant failing with a clear
    // `ActionFailed`, byte-identical to before this seam existed.
    let orch_builder = match cfg.bg_session_forker.clone() {
        Some(forker) => orch_builder.with_bg_session_forker(forker),
        None => orch_builder,
    };

    // EXPERIMENTAL_SKILL_SEARCH skill-discovery prefetch ACTIVATION (gated,
    // default OFF). claude-code keeps this behind `feature('EXPERIMENTAL_SKILL_SEARCH')`
    // — DCE'd out of the shipping 2.1.195 binary (every skill-search literal = 0
    // hits), so default-OFF is the correct parity state. The composition-root
    // PRESENCE of the prefetch IS the gate (`skill_discovery_prefetch.is_some()`),
    // exactly like the memory prefetch above. Wired ONLY when the flag is ON via
    // `telemetry::flag_bool` (the GrowthBook-style sync reader; empty snapshot ⇒
    // returns the `false` default by default), OR one of the env overrides is
    // truthy (`bun`-bundle `envBool(..., false)` parity). The faithful local
    // backend is a `RegistryCandidateSource` over the desktop skill set (substring
    // trigger discovery — the binary's native lexical index; AKI/Haiku backends
    // are out of scope). Unset/false ⇒ no prefetch ⇒ everything inert and the
    // locked fixtures byte-identical.
    let skill_search_on = telemetry::flag_bool("EXPERIMENTAL_SKILL_SEARCH", false)
        || is_env_truthy("CLAUDE_CODE_EXPERIMENTAL_SKILL_SEARCH")
        || is_env_truthy("LINGXI_SKILL_SEARCH");
    let orch_builder = if skill_search_on {
        let source: Arc<dyn skill_api::SkillCandidateSource> = Arc::new(
            skill_api::RegistryCandidateSource::new(Arc::new(desktop_skill_registry())),
        );
        orch_builder.with_skill_discovery_prefetch(Arc::new(
            skill_api::SkillDiscoveryPrefetch::new(
                source,
                Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>,
            ),
        ))
    } else {
        orch_builder
    };

    // P1 session-memory standalone trigger (§6.5, gated, default OFF). When
    // `LINGXI_SESSION_MEMORY` is truthy, wire the threshold-gated extractor
    // so durable notes are background-distilled (a Haiku-class fork) once the
    // tool-call threshold crosses and written to
    // `<configHome>/agents/session-memory/<id>.md`, which the Session-tier memdir
    // scan re-loads next session. Thresholds are unpinned upstream (spec §6.5) —
    // 30/30 tool calls is a tunable default. Unset/false ⇒ no handle ⇒ inert, so
    // the locked fixtures stay byte-identical.
    let orch_builder = match (is_env_truthy("LINGXI_SESSION_MEMORY"), dirs::home_dir()) {
        (true, Some(home)) => {
            orch_builder.with_session_memory(orchestrator::prompt::build_session_memory_handle(
                side_query_client.clone(),
                "claude-haiku-4-5".to_string(),
                30,
                30,
                &home,
                Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>,
            ))
        }
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
    // FIX (B-agent-model-inheritance): now that the orchestrator exists, wire the
    // subagent spawner's LIVE default-model source to read the orchestrator's LIVE
    // `session.model` (the SAME source `build_prompt_context` / `get_status_snapshot`
    // read — updated by a mid-session `/model` switch or resume), superseding the
    // boot snapshot `orch_cfg.model` for spawns that carry no `parent_model_override`.
    // The read happens at spawn time (during tool execution, when the session lock
    // is free), so a `try_lock` fast path is sufficient; on the rare contended read
    // it returns `None` and the spawner falls back to its boot snapshot.
    {
        let session = orch.session();
        let _ = subagent_default_model_provider_cell.set(std::sync::Arc::new(move || {
            session.try_lock().ok().map(|s| s.model.clone())
        }));
    }
    // H-CHG-02: wire the enforcing gate's live `set_permission_mode` auto gate to
    // the SAME live `session.model` source, so a runtime switch to `auto` after a
    // `/model` to an auto-unsupported model is rejected (`dUe(wi())` — claude-code
    // `Nle`) instead of silently accepted. Non-blocking read (`try_lock`); a
    // contended read returns `None` and the model check is skipped (fail-open).
    if let Some(cell) = live_model_provider_cell.as_ref() {
        let session = orch.session();
        let _ = cell.set(std::sync::Arc::new(move || {
            session.try_lock().ok().map(|s| s.model.clone())
        }));
    }
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
    let connect_copilot: Arc<dyn command_core::CopilotConnectDriver> = Arc::new(
        crate::connect::EngineCopilotConnect::new(credentials.clone()),
    );
    let connect_writer: Arc<dyn command_core::ConnectCredentialWriter> =
        Arc::new(crate::connect::EngineCredentialWriter::new(
            credentials.clone(),
            cfg.connect_prompt.clone().unwrap_or_else(|| {
                Arc::new(crate::connect::NoopKeyPrompt) as Arc<dyn crate::connect::SecureKeyPrompt>
            }),
        ));
    let connect_chatgpt: Arc<dyn command_core::ChatGptConnectDriver> = Arc::new(
        crate::connect::EngineChatGptConnect::new(openai_oauth_client, credentials.clone()),
    );
    // Unified OAuth sign-in driver for the TUI `/connect` picker (Anthropic
    // Pro/Max + OpenAI ChatGPT browser flows). Reuses the same backends as
    // `/login` (the Anthropic `auth` handle) and `/connect chatgpt`
    // (`connect_chatgpt`); built here while both are still owned (the registry
    // call below moves `connect_chatgpt`).
    let oauth_connect_driver: Arc<dyn command_core::OAuthConnectDriver> = Arc::new(
        crate::connect::EngineOAuthConnect::new(auth.clone(), connect_chatgpt.clone()),
    );
    let mut reg = desktop_command_registry(
        handle,
        auth.clone(),
        &cfg.cwd,
        &cfg.lingxi_home,
        connect_writer,
        connect_copilot.clone(),
        connect_chatgpt,
        cfg.customization_gates,
        shared_command_registry.clone(),
    )
    .await;
    reg.register_builtin_handler(worktree_command_handler);

    // WIZARD-06: re-register `/auto-mode-setup` WITH its runners attached.
    // `register_all_builtin_commands` wires the handle-free shape (grammar,
    // `--help`, every rejection path); only the composition root can supply the
    // two branches that need real capabilities — a live `ApiService` for
    // `--propose` and the settings writer for `--apply-file`. Until this point
    // both branches report `unavailable_here` rather than pretending to work.
    {
        let listing = default_listings
            .iter()
            .find(|l| l.request_model == default_model_id || l.display_model == default_model_id);
        // The oracle derives the thinking flag from the MODEL (`IQt(r)`), not
        // from session config, and grants the no-thinking budget top-up when the
        // model carries no thinking config.
        let thinking = listing.is_some_and(|l| l.supports_reasoning);
        // `subscription_signal` reads the plan from the live snapshot; an
        // unauthenticated or still-fetching session yields `None`, which renders
        // as the "unknown" signal rather than a guessed plan.
        let plan = subscription
            .read()
            .ok()
            .and_then(|g| g.as_ref().and_then(|s| s.subscription_type.clone()));
        let transcript_dir = cfg
            .lingxi_home
            .join("projects")
            .join(session::jsonl::path::project_dir_name(
                &cfg.cwd.to_string_lossy(),
            ));
        let propose = std::sync::Arc::new(auto_mode_propose::DesktopProposeRunner::new(
            api_service.clone(),
            default_model_id.clone(),
            default_model_profile.clone(),
            thinking,
            plan,
            cfg.cwd.clone(),
            cfg.lingxi_home.clone(),
            transcript_dir,
        ));
        let apply = std::sync::Arc::new(auto_mode_propose::DesktopApplyRunner::new(
            command_core::auto_mode_setup::apply_file_roots(&cfg.lingxi_home),
            permission::PermissionPaths {
                lingxi_home: cfg.lingxi_home.clone(),
                cwd: cfg.cwd.clone(),
            },
        ));
        reg.register_builtin_handler(std::sync::Arc::new(
            command_core::AutoModeSetupHandler::new()
                .with_propose(propose)
                .with_apply(apply),
        ));
    }
    // SKILLEXEC.2: fill the shared command-registry slot the `Skill` tool's
    // loader holds, then hand the SAME `Arc` to the slash dispatcher so the tool
    // and the dispatcher observe one command set (plugin lifecycle mutations via
    // the dispatcher's write lock are visible to the loader too).
    // claude-code `getAllCommands` folds `mcp.commands` into the command list:
    // an MCP server's PROMPTS become `/<server>:<prompt>` slash commands. The
    // registry has always fetched them at connect; this is the read side, and
    // without it they existed on the wire and nowhere the user or model could
    // reach. Merged LAST so a same-named local command wins — a remote server
    // must not shadow one of the user's own.
    for cmd in command_api::mcp_prompts::mcp_prompt_commands(
        &mcp_registry.connected_prompts().await,
    ) {
        if reg.resolve(&cmd.name).is_none() {
            reg.register_command(cmd);
        }
    }
    *shared_command_registry.write().await = reg;

    // (6.5) Plugin bootstrap — discover installed plugins on disk and
    //       materialise their COMMANDS + HOOKS into the live registries, plus
    //       their AGENTS into the agent catalog. Mirrors claude-code's
    //       cache-only plugin load at startup (`main.tsx:282`
    //       `loadAllPluginsCacheOnly()` → `pluginLoader.ts:1887`
    //       `loadPluginsFromMarketplaces({cacheOnly})`; `setup.ts:318`
    //       `loadPluginHooks`). Plugins live under `getPluginsDirectory()` =
    //       `~/.lingxi/plugins` (`pluginDirectories.ts:53`), honoring the
    //       `LINGXI_PLUGIN_CACHE_DIR` override. Discovery is allowlist-
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
    //       (M3 cc2.1.198) `--safe-mode` / `--bare` skip the AMBIENT bootstrap
    //       (`K5d.plugins:!1` / `V5d.plugins:!0`; safe-mode log "Skipping
    //       plugin hooks - safe mode disables plugins"). This also skips
    //       plugin LSP servers — lingxi's only LSP-server source — matching
    //       `Hc("lspServers")` gating `initializeLspServerManager`.
    //       (M4 cc2.1.198) `--plugin-dir` session-only plugins are an EXPLICIT
    //       request that survives `--bare` (its help text: "Explicitly provide
    //       context via: … --plugin-dir") but not safe mode; they load AFTER
    //       the marketplace-installed discovery through the SAME `pm.enable`
    //       materialisation path (binary `EBm` → the shared plugin merge).
    let ambient_plugins = !cfg.customization_gates.disables_plugins();
    let inline_plugins = !cfg.cli_plugin_dirs.is_empty() && !cfg.customization_gates.safe_mode;
    // (`/reload-plugins`) The retained plugin subsystem — `None` when plugins are
    // entirely disabled (safe mode / `--bare` with no `--plugin-dir`), so the
    // interactive refresh reports "plugins disabled" rather than reloading.
    let mut plugin_runtime: Option<Arc<PluginRuntime>> = None;
    if ambient_plugins || inline_plugins {
        let plugins_dir = std::env::var_os("LINGXI_PLUGIN_CACHE_DIR")
            .map_or_else(|| cfg.lingxi_home.join("plugins"), std::path::PathBuf::from);
        // Primary (faithful) path: resolve the `settings.enabledPlugins`
        // allowlist (`plugin@marketplace` → enabled) to versioned cache dirs
        // `cache/{marketplace}/{plugin}/{version}/`, exactly as
        // `loadAllPluginsCacheOnly` (`pluginLoader.ts:1888`) consumes a real
        // `~/.lingxi/plugins`; a flat-walk fallback covers pre-fetched local
        // dirs, and `--plugin-dir` session plugins append. The shared body is
        // `discover_plugin_set`, reused by [`PluginRuntime::refresh`].
        let discovered = discover_plugin_set(
            ambient_plugins,
            inline_plugins,
            &cfg.lingxi_home,
            &cwd_for_plugins,
            &plugins_dir,
            &cfg.cli_plugin_dirs,
        )
        .await;
        // Build the manager UNCONDITIONALLY (even when zero plugins resolve on
        // disk) and RETAIN it in `plugin_runtime`, so a later `/reload-plugins`
        // can enable a plugin the user turns on mid-session. Live registries the
        // manager materialises components into:
        // - command  → `shared_command_registry` (drives `/`-completion + the
        //   per-turn skill listing).
        // - hooks    → `plugin_hook_registry` (the orchestrator's clone).
        // - MCP      → `plugin_mcp_registry` (== the orchestrator's
        //   `mcp_registry`; scoped configs live-connect via `connect_all`, the
        //   same path as configured `.mcp.json` servers, and the reconnect loop
        //   covers any that fail their initial dial).
        // - LSP      → `plugin_lsp_registry` (== the `LSPTool`'s registry).
        // The SKILL and OUTPUT-STYLE registries have no turn-loop consumer yet,
        // so they are local instances here (residual, as at startup).
        // Seed the persisted non-sensitive `userConfig` (settings `pluginConfigs`
        // scope) so the loader resolves `${user_config.*}` options from disk (not
        // just field defaults) and injects `LINGXI_PLUGIN_OPTION_*` into plugin
        // hooks. Sensitive values are NOT here — they resolve live from
        // `CredentialManager`.
        let plugin_configs = load_plugin_configs(&cfg.lingxi_home).await;
        let blocked_marketplaces = load_blocked_marketplaces().await;
        let pm = Arc::new(
            plugin::PluginManager::new(
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
                plugin_output_style_registry.clone(),
                plugin_mcp_registry.clone(),
                plugin_lsp_registry.clone(),
                Arc::new(RwLock::new(ToolRegistry::new())),
            )
            .with_agent_catalog(plugin_agent_catalog.clone())
            .with_plugin_configs(plugin_configs)
            .with_blocked_marketplaces(blocked_marketplaces),
        );
        for (id, manifest, dir) in discovered {
            let plugin_name = manifest.name.clone();
            // Materialise COMMANDS + HOOKS + MCP + LSP (the privilege gate runs
            // here, validating agent frontmatter). The plugin's AGENTS are
            // materialised into the catalog ONLY on success — the dir-scan loader
            // is ungated, so gating on enable keeps a plugin rejected for an
            // escalating agent from smuggling it into the live catalog (cc
            // rejects the plugin as a unit).
            match pm.enable(&id, manifest, dir).await {
                Ok(()) => {}
                Err(e) => tracing::warn!(
                    plugin = %plugin_name,
                    error = %e,
                    "skipping plugin that failed to load"
                ),
            }
        }
        plugin_runtime = Some(Arc::new(PluginRuntime {
            manager: pm,
            plugins_dir,
            home: cfg.lingxi_home.clone(),
            cwd: cwd_for_plugins.clone(),
            cli_plugin_dirs: cfg.cli_plugin_dirs.clone(),
            ambient: ambient_plugins,
            inline: inline_plugins,
        }));
    }

    // (M4 cc2.1.198) `--agent <agent>` — resolve the session agent against the
    // FINAL catalog (dir + `--agents` flag + plugin agents), the binary's `dts`
    // lookup: exact `agentType` match, else FQN `…:{name}` suffix; a miss logs
    // `Warning: agent "X" not found. Available agents: …. Using default
    // behavior.` and the session proceeds with default behavior.
    //
    // (P2-02 cc2.1.207) On a HIT the binary APPLIES the agent to the MAIN loop
    // via `bde(h?.agentType)` + `mainThreadAgentDefinition`. We adopt the
    // model-visible pieces here:
    //   • `agentType` — rides every main-thread lifecycle hook payload (claude
    //     `wf`/`MVe` `?? MB()`);
    //   • system prompt — becomes the main-loop system prompt on every query via
    //     `nre` (`--system-prompt` still winning);
    //   • `tools:` + `disallowedTools` frontmatter — narrows the advertised tool
    //     pool (claude `HJ(us,to,!1,!0).resolvedTools`, `n=true` ⇒ NO subagent
    //     always-disallowed strip);
    //   • `model` — replaces the main-loop model (claude `jb(Zo(y.model))`),
    //     gated exactly like the binary: only when the user did NOT pass
    //     `--model` (`!cfg.default_model_explicit` ≙ `!userSpecifiedModel`) AND
    //     the agent declares an explicit model (`AgentModel != Inherit`).
    // This runs BEFORE the `SessionStart` firing below so that hook carries the
    // `agentType`, and AFTER the default-model seed above so the override wins.
    //   • frontmatter `hooks` — registered as `mainThreadAgentHooks` (claude
    //     `Rft`→`o_n`), gated by [`agent_source_is_trusted`] (`g9e`), BELOW.
    //   • RESUME restoration (`rVe`) — when NO `--agent` is passed on a `--resume`
    //     (`cfg.session_id_override` set), the applied `agentType` persisted at
    //     the ORIGINAL boot is read back from this session's transcript
    //     (`agentSettings.get(sessionId)`) and re-adopted through the SAME block
    //     (prompt + tools + model + hooks). A miss emits the byte-exact
    //     `Resumed session had agent "X" but it is no longer available. Using
    //     default behavior.` warning (claude `rVe`) and falls back to default. A
    //     re-passed `--agent` wins (claude `rVe`'s `if(t)return`) — it is applied
    //     via the explicit arm below and the resume read is skipped.
    //   • frontmatter `mcpServers` (scope `"agent"`) — CLOSED (M7 cc2.1.220):
    //     merged into the to-connect config list by the `FWt` pre-pass in
    //     (5.1)/(5.3) above, BEFORE the MCP registry `connect_all` and the
    //     `Arc<ToolRegistry>` snapshot — the servers register, connect and
    //     surface tools exactly like `--mcp-config` servers. The
    //     `(wanted_agent, resumed_agent_snapshot, from_resume)` triple this
    //     block consumes is computed THERE (one transcript read serves both the
    //     merge and this application).
    // (Built-in agent defs live in the subagent spawner, not this catalog, so
    // their names are absent from the miss warning's "Available agents" list —
    // residual.)
    if let Some(wanted) = wanted_agent {
        // Resolve against the FINAL catalog, extracting what the main thread
        // applies (agentType + system prompt + tool policy + model) so the
        // catalog read lock is released before we mutate the orchestrator seam.
        let applied = {
            let cat = plugin_agent_catalog.read().await;
            let snapshot_def = resumed_agent_snapshot
                .as_ref()
                .and_then(|v| serde_json::from_value::<agent::AgentDefinition>(v.clone()).ok())
                .filter(|a| a.agent_type == wanted);
            let hit = snapshot_def.as_ref().or_else(|| {
                cat.iter().find(|a| a.agent_type == wanted).or_else(|| {
                    let suffix = format!(":{wanted}");
                    cat.iter().find(|a| a.agent_type.ends_with(&suffix))
                })
            });
            match hit {
                Some(a) => {
                    // claude `if(!userSpecifiedModel&&y.model&&y.model!=="inherit")
                    // {jb(Zo(y.model))}`. `Zo` = `resolve_user_specified_model`
                    // (alias→wire id). Frontmatter never yields `Explicit`, but
                    // handle both alias/explicit arms for completeness. This is
                    // ALSO the resume model reset (`rVe` applies the same `jb`).
                    let model_override = if cfg.default_model_explicit {
                        None
                    } else {
                        match &a.model {
                            agent::AgentModel::Alias(spec) | agent::AgentModel::Explicit(spec) => {
                                Some(agent::model_resolution::resolve_user_specified_model(spec))
                            }
                            agent::AgentModel::Inherit => None,
                        }
                    };
                    Some((
                        a.agent_type.clone(),
                        a.system_prompt.clone(),
                        a.tools.clone(),
                        a.disallowed_tools.clone(),
                        model_override,
                        // (P2-02 cc2.1.207) keep the frontmatter `hooks` + `source`
                        // so `Rft` can register them as `mainThreadAgentHooks`
                        // below (the source drives the `g9e` trusted-source gate).
                        a.frontmatter_hooks.clone(),
                        a.source,
                        a.clone(),
                    ))
                }
                None => {
                    if from_resume {
                        // claude `rVe`: the persisted agent is gone from the final
                        // catalog → byte-exact warn, then fall back to default.
                        tracing::warn!(
                            "Resumed session had agent \"{wanted}\" but it is no longer available. Using default behavior."
                        );
                    } else {
                        tracing::warn!(
                            "Warning: agent \"{wanted}\" not found. Available agents: {}. Using default behavior.",
                            cat.iter()
                                .map(|a| a.agent_type.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    None
                }
            }
        };
        if let Some((
            agent_type,
            system_prompt,
            tools,
            disallowed_tools,
            model_override,
            frontmatter_hooks,
            source,
            resolved_definition,
        )) = applied
        {
            tracing::debug!(agent = %agent_type, from_resume, "--agent applied to main thread");
            // (P2-02 cc2.1.207) Persist the applied `agentType` as an
            // `agent-setting` transcript record (claude
            // `{type:"agent-setting",agentSetting:currentSessionAgentSetting,
            // sessionId}`) so a later `--resume` with no `--agent` re-adopts it via
            // `rVe`. Only on the EXPLICIT path (`!from_resume`) — the resume replay
            // already read this record — and only when the session is persisted
            // (no writer ⇒ nothing to resume from). Borrows `agent_type` before it
            // moves into `set_main_thread_agent`.
            if !from_resume && cfg.session_persistence {
                if let Err(e) = main_agent_setting_writer
                    .append_agent_setting_snapshot(
                        &main_session_uuid,
                        &agent_type,
                        &serde_json::to_value(&resolved_definition)
                            .expect("resolved AgentDefinition must serialize"),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "failed to persist --agent agent-setting record");
                }
            }
            orch.set_main_thread_agent(
                agent_type,
                system_prompt,
                tools,
                disallowed_tools,
                model_override,
            )
            .await;

            // (P2-02 cc2.1.207) `Rft` — register the agent's frontmatter `hooks`
            // as `mainThreadAgentHooks` (`o_n(e.hooks)`). The binary gate is
            //   `if(e?.hooks && (!uA("hooks") || g9e(e.source))) o_n(e.hooks)`.
            // `uA("hooks")` is the `strictPluginOnlyCustomization` policy (NOT
            // `disableAllHooks` — that gate lives at hook DISPATCH, on the
            // executor's `policy_disable_all_hooks`). LingXi does not wire
            // `strictPluginOnlyCustomization`, so `uA("hooks")` is always
            // `false` and the gate reduces to "register when the agent declares
            // hooks"; the `g9e` trusted-source arm ([`agent_source_is_trusted`],
            // the byte-faithful `qXh` set) is a structural port for when that
            // policy lands. `is_agent=false` keeps `Stop` as `Stop` (this is the
            // MAIN thread, not a subagent — no `Stop`→`SubagentStop` retarget).
            // Registered BEFORE the `fire_session_start("startup")` call below so
            // a SessionStart frontmatter hook fires with the agent applied. The
            // orchestrator owns the bucket identity so an in-place resume can
            // replace it without leaking hooks from the previously mounted
            // session.
            //
            // (cc 2.1.218 `QEt`) The binary now ALSO gates on `mvo(e)` — the
            // definition's folder must be trusted before its `hooks:` become
            // live main-thread hooks:
            //   `let t=!VR("hooks")||J0e(e.source), r=mvo(e);
            //    if(t&&r){b1r(e.hooks);return} if(t&&!r)hvo(e,"mainThread"); b1r(void 0)`
            // NOTE the pre-existing `strict_plugin_only_hooks` arm is inert
            // (`uA("hooks")` unwired ⇒ `false` ⇒ `(!false || …)` is ALWAYS true),
            // so before 2.1.218 this site had no effective gate at all.
            let strict_plugin_only_hooks = false; // `uA("hooks")` — unwired in LingXi.
            if !frontmatter_hooks.is_empty()
                && (!strict_plugin_only_hooks || agent_source_is_trusted(source))
            {
                if agent::hooks_trust::agent_hooks_origin_trusted(&resolved_definition, &cfg.cwd) {
                    orch.replace_main_thread_agent_hooks(&frontmatter_hooks)
                        .await;
                } else {
                    agent::hooks_trust::report_untrusted_hooks(
                        &resolved_definition,
                        &cfg.cwd,
                        agent::hooks_trust::HooksTrustSurface::MainThread,
                        false,
                    );
                    // `QEt`'s untrusted arm ends in `b1r(void 0)` (clear the
                    // bucket). A no-op at boot (the bucket starts empty), kept
                    // for parity with the resume site's clear.
                    orch.replace_main_thread_agent_hooks(&[]).await;
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
    // `--disable-slash-commands` (claude-code "Disable all skills"): after ALL
    // builtin + plugin + skill registration, replace the shared command registry
    // with an empty one so the dispatcher AND the `Skill` tool's loader (which
    // share this `Arc`) observe zero commands/skills. Plugin MCP servers / hooks
    // / tools live in other registries and are intentionally unaffected (claude's
    // flag disables skills/commands only).
    if cfg.disable_slash_commands {
        *shared_command_registry.write().await = CommandRegistry::new();
    }
    let expansion_ctx_orch = orch.clone();
    let dispatcher = RegistrySlashDispatcher::new(shared_command_registry.clone())
        .with_skill_usage_home(cfg.lingxi_home.clone())
        .with_expansion_hooks(
            expansion_hook_executor,
            std::sync::Arc::new(move || {
                let orch = expansion_ctx_orch.clone();
                Box::pin(async move { orch.expansion_hook_context().await })
            }),
        )
        // (#3) Real embedded-shell expansion for markdown/plugin `!`cmd`` bodies
        // AND the builtin `InjectMessage` prompts (`/commit` …). Non-MCP only.
        .with_shell_expansion(shell_expansion_provider.clone());

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
    let session_start = orch.fire_session_start("startup").await;
    if session_start.reload_skills {
        let home = dirs::home_dir().unwrap_or_else(|| cfg.lingxi_home.clone());
        let managed_dir = crate::settings_watch::managed_settings_dir();
        let handler = command_core::reload_skills::ReloadSkillsHandler::with_all_roots(
            shared_command_registry.clone(),
            cfg.cwd.clone(),
            cfg.lingxi_home.clone(),
            Some(managed_dir),
            home,
            Vec::new(),
            cfg.customization_gates.safe_mode,
        );
        if let Some(parsed) = parse_slash_command("/reload-skills") {
            let _ = handler.handle(&parsed).await;
        }
    }

    // (7.1) Instruction-load lifecycle: fire the `InstructionsLoaded` hooks now
    //       that memory + the hook registry are wired. claude-code fires this
    //       fire-and-forget hook once per LINGXI.md / `LINGXI.local.md` spliced
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
        settings_watch::SettingsWatcher::new(&cfg.lingxi_home, &watch_cwd, firer)
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
    // Fill the `CwdChanged` firer's deferred rebinder cell now that the watcher
    // exists (it spawns AFTER the firer is built). On a mid-session `cd` the
    // firer rebinds this watcher — the watcher-rebind half of `onCwdChanged`.
    // An empty handle (no `FileChanged` hooks / no resolved paths) yields no
    // rebinder, so the cell stays unset and the rebind remains a strict no-op.
    if let Some(rebinder) = file_changed_watcher.rebinder() {
        file_changed_watcher_rebinder.set(rebinder);
    }

    // Phase 2a §6.2: `provider_availability` is computed EARLY in build() (the
    // connected-provider default-model fallback consults it before the
    // orchestrator exists) and reused verbatim here for the `/model` picker's
    // Connect badge — nothing between the two points mutates credentials.

    // T2a: per-profile login-method tag derived from the builtin catalog auth
    // strategy.  AuthStrategy::None providers are not connectable → skipped.
    let mut provider_auth_methods: std::collections::BTreeMap<String, String> =
        llm_client::builtin_presets()
            .providers
            .iter()
            .filter_map(|p| {
                use llm_client::AuthStrategy::*;
                let tag = match p.auth {
                    ApiKey | Bearer => "api_key",
                    CopilotBearer => "copilot_device",
                    ChatGptOAuth | OAuthBearer | AwsSigV4 | GcpToken | AzureToken => "oauth",
                    None => return Option::None,
                };
                Some((p.profile_name.clone(), tag.to_string()))
            })
            .collect();
    // Anthropic is not in the builtin catalog presets (it is the native auth
    // path), but the /connect picker still needs it represented with its auth
    // method tag ("api_key") — mirror the provider_availability approach above.
    provider_auth_methods
        .entry("anthropic".to_string())
        .or_insert_with(|| "api_key".to_string());

    Ok(DesktopRuntime {
        orchestrator: orch,
        shared_command_registry,
        dispatcher,
        auth,
        task_registry,
        coordinator,
        coordinator_mode,
        permission_gate: adapter_gate,
        enforcing_permission_gate,
        settings_watcher,
        file_changed_watcher,
        subscription,
        sandbox_toggle,
        sandbox_desc_auto_allow,
        sandbox_desc_fallback,
        sandbox_desc_deps_ok,
        file_history,
        plugin_runtime,
        provider_availability,
        default_model_fallback,
        provider_auth_methods,
        model_providers,
        provider_adapter: provider_adapter_handle,
        credentials,
        http: http.clone() as Arc<dyn traits::HttpTransport>,
        structured_output_slot,
        wakeup_scheduler_cell,
        runtime_spawner: Arc::new(PosixRuntime::new()) as Arc<dyn traits::RuntimeSpawner>,
        bash_runner,
        shell_expansion: shell_expansion_provider,
        connect_copilot,
        oauth_connect_driver,
        // P1-08 runtime `/add-dir` live-effect handles (captured before the
        // orchestrator builder consumed the originals).
        session_cwd: runtime_session_cwd,
        mcp_registry: runtime_mcp_registry,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build, desktop_tool_registry, model_deprecation_warning, parse_worktree_slash_action,
        CoordinatorWiring, DesktopConfig, WorktreeSlashAction, WORKTREE_SLASH_USAGE,
    };
    use std::sync::Arc;

    #[test]
    fn worktree_slash_parser_covers_lifecycle_and_safe_remove() {
        let parse = |raw: &str| {
            let parsed = command_api::parse_slash_command(raw).expect("slash command");
            parse_worktree_slash_action(&parsed)
        };

        assert_eq!(parse("/worktree"), Ok(WorktreeSlashAction::Create(None)));
        assert_eq!(
            parse("/worktree feature/auth"),
            Ok(WorktreeSlashAction::Create(Some("feature/auth".into())))
        );
        assert_eq!(
            parse("/worktree create review-fix"),
            Ok(WorktreeSlashAction::Create(Some("review-fix".into())))
        );
        assert_eq!(
            parse("/worktree enter \"/tmp/path with spaces\""),
            Ok(WorktreeSlashAction::Enter("/tmp/path with spaces".into()))
        );
        assert_eq!(parse("/worktree enter"), Err(WORKTREE_SLASH_USAGE));
        assert_eq!(parse("/worktree status"), Ok(WorktreeSlashAction::Status));
        assert_eq!(parse("/worktree keep"), Ok(WorktreeSlashAction::Keep));
        assert_eq!(
            parse("/worktree remove"),
            Ok(WorktreeSlashAction::Remove {
                discard_changes: false
            })
        );
        assert_eq!(
            parse("/worktree remove --discard"),
            Ok(WorktreeSlashAction::Remove {
                discard_changes: true
            })
        );
        assert_eq!(parse("/worktree remove --force"), Err(WORKTREE_SLASH_USAGE));
        assert_eq!(
            parse("/worktree create too many"),
            Err(WORKTREE_SLASH_USAGE)
        );
    }

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
            async fn begin(
                &self,
                _domain: Option<&str>,
            ) -> Result<CopilotConnectStep, ConnectError> {
                Ok(CopilotConnectStep {
                    user_code: "X".into(),
                    verification_uri: "u".into(),
                })
            }
            async fn poll_to_completion(
                &self,
                _s: &CopilotConnectStep,
            ) -> Result<(), ConnectError> {
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
            super::CustomizationGates::default(),
            Arc::new(tokio::sync::RwLock::new(command_api::CommandRegistry::new())),
        )
        .await;
        assert!(
            reg.get_handler("connect").is_some(),
            "/connect not wired into desktop registry"
        );
    }

    /// (M3 cc2.1.198) `--safe-mode` / `--bare` skip custom-command + skill dir
    /// discovery in [`super::desktop_command_registry`] (`K5d.skills:!1` /
    /// `V5d.skills:!0`) while builtins stay registered; default gates keep
    /// loading the same fixture.
    #[tokio::test]
    async fn safe_mode_and_bare_skip_custom_command_discovery() {
        use async_trait::async_trait;
        use command_core::{
            ChatGptConnectDriver, ConnectCredentialWriter, ConnectError, CopilotConnectDriver,
            CopilotConnectStep,
        };
        use traits::{AuthError, AuthHandle, LoginInfo, OrchestratorHandle};

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
            async fn begin(
                &self,
                _domain: Option<&str>,
            ) -> Result<CopilotConnectStep, ConnectError> {
                Ok(CopilotConnectStep {
                    user_code: "X".into(),
                    verification_uri: "u".into(),
                })
            }
            async fn poll_to_completion(
                &self,
                _s: &CopilotConnectStep,
            ) -> Result<(), ConnectError> {
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

        // Project fixture: `<cwd>/.lingxi/commands/m3custom.md` — resolvable as
        // `/m3custom` when customization discovery runs.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().to_path_buf();
        let cmds = cwd.join(".lingxi").join("commands");
        std::fs::create_dir_all(&cmds).expect("mk commands");
        std::fs::write(cmds.join("m3custom.md"), "M3 custom command body").expect("write cmd");

        for (gates, want_custom) in [
            (super::CustomizationGates::default(), true),
            (
                super::CustomizationGates {
                    safe_mode: true,
                    bare: false,
                },
                false,
            ),
            (
                super::CustomizationGates {
                    safe_mode: false,
                    bare: true,
                },
                false,
            ),
        ] {
            let handle: Arc<dyn OrchestratorHandle> =
                Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
            let auth: Arc<dyn AuthHandle> = Arc::new(MockAuth);
            let reg = super::desktop_command_registry(
                handle,
                auth,
                &cwd,
                &cwd, // lingxi_home rooted in the sandbox too (no user leakage)
                Arc::new(W),
                Arc::new(C),
                Arc::new(G),
                gates,
                Arc::new(tokio::sync::RwLock::new(command_api::CommandRegistry::new())),
            )
            .await;
            assert_eq!(
                reg.resolve("m3custom").is_some(),
                want_custom,
                "{gates:?}: custom command discovery gate"
            );
            assert!(
                reg.get_handler("connect").is_some(),
                "{gates:?}: builtins must stay registered"
            );
        }
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

        let writer = EngineCredentialWriter::new(
            cm.clone(),
            Arc::new(CannedPrompt(Some("sk-test-123".into()))),
        );
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
        assert_eq!(
            model_deprecation_warning(Some("claude-3-5-haiku-20241022")),
            None
        );
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
        assert_eq!(
            model_deprecation_warning(Some("claude-3-5-haiku-20241022")),
            None
        );
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
        assert_eq!(cfg.lingxi_home, std::path::PathBuf::new());
        // Mirrors `DesktopEngineConfig::default().default_model` (2.1.198:
        // Sonnet 5 is the default first-party model).
        assert_eq!(cfg.default_model, "claude-sonnet-5");
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
        // type implements `Clone`/secret-safe `Debug` so a host can fan it out
        // and include its non-sensitive shape in diagnostics.
        let custom = DesktopConfig {
            use_noop_permission_gate: false,
            ..cfg.clone()
        };
        assert!(!custom.use_noop_permission_gate);
        let _ = format!("{custom:?}");
    }

    #[test]
    fn desktop_config_debug_redacts_secret_bearing_fields() {
        const SECRET_CANARY: &str = "LX_SECRET_CANARY_NEVER_LOG_6e44f87c";

        let cfg = DesktopConfig {
            api_base: format!("https://user:{SECRET_CANARY}@example.test/?token={SECRET_CANARY}"),
            api_key: SECRET_CANARY.to_string(),
            api_key_helper: Some(format!("printf {SECRET_CANARY}")),
            provider_profiles: Some(std::collections::BTreeMap::from([(
                "private".to_string(),
                serde_json::json!({ "apiKey": SECRET_CANARY }),
            )])),
            routing: Some(serde_json::json!({ "credential": SECRET_CANARY })),
            system_prompt_override: Some(SECRET_CANARY.to_string()),
            append_system_prompt: Some(SECRET_CANARY.to_string()),
            cli_agents_json: Some(format!(r#"{{"prompt":"{SECRET_CANARY}"}}"#)),
            ..DesktopConfig::default()
        };

        let debug = format!("{cfg:?}");
        assert!(!debug.contains(SECRET_CANARY), "secret leaked: {debug}");
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("provider_profile_count: Some(1)"));
        assert!(debug.contains("routing_configured: true"));
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

    /// (M4 cc2.1.198) `--agents` flag agents merge with `flagSettings`
    /// precedence (`XXt`: flag REPLACES a same-named user/project agent, else
    /// appends) and are IGNORED in safe mode (warn — the `--agents: ignored in
    /// safe mode` branch).
    #[test]
    fn merge_cli_flag_agents_precedence_and_safe_mode() {
        fn dir_agent(name: &str) -> agent::AgentDefinition {
            agent::parse_agent_from_json(
                name,
                &serde_json::json!({"description": "from dir", "prompt": "p"}),
                agent::AgentSource::Project,
            )
            .expect("valid dir agent")
        }
        let raw = r#"{"reviewer": {"description": "from flag", "prompt": "p"},
                      "extra": {"description": "new", "prompt": "p"}}"#;

        // Normal: same-named `reviewer` replaced (source Flag), `extra` appended.
        let mut agents = vec![dir_agent("reviewer"), dir_agent("keeper")];
        super::merge_cli_flag_agents(&mut agents, Some(raw), false);
        assert_eq!(agents.len(), 3);
        let reviewer = agents.iter().find(|a| a.agent_type == "reviewer").unwrap();
        assert_eq!(reviewer.when_to_use, "from flag");
        assert_eq!(reviewer.source, agent::AgentSource::Flag);
        assert!(agents.iter().any(|a| a.agent_type == "extra"));
        assert!(agents.iter().any(|a| a.agent_type == "keeper"));

        // Safe mode: the payload is ignored outright.
        let mut safe = vec![dir_agent("reviewer")];
        super::merge_cli_flag_agents(&mut safe, Some(raw), true);
        assert_eq!(safe.len(), 1);
        assert_eq!(safe[0].when_to_use, "from dir");

        // No flag: untouched.
        let mut none = vec![dir_agent("reviewer")];
        super::merge_cli_flag_agents(&mut none, None, false);
        assert_eq!(none.len(), 1);

        // Invalid JSON: logged, no agents contributed, no abort.
        let mut bad = vec![dir_agent("reviewer")];
        super::merge_cli_flag_agents(&mut bad, Some("{nope"), false);
        assert_eq!(bad.len(), 1);
    }

    /// (M7 cc2.1.220) `merge_agent_frontmatter_mcp_servers` — the `FWt` port:
    /// gate order, existing-name-wins merge, enterprise blocked names.
    #[test]
    fn merge_agent_frontmatter_mcp_servers_fwt_gates_and_merge() {
        fn agent_with_server(name: &str, source: agent::AgentSource) -> agent::AgentDefinition {
            let mut def = agent::parse_agent_from_json(
                "helper",
                &serde_json::json!({"description": "d", "prompt": "p"}),
                source,
            )
            .expect("valid agent");
            let mut map = serde_json::Map::new();
            map.insert(
                name.to_string(),
                serde_json::json!({"command": "npx", "args": ["-y", "docs-mcp"]}),
            );
            def.mcp_servers = vec![agent::AgentMcpServerSpec::Record(map)];
            def
        }
        fn existing(name: &str) -> mcp::McpServerConfig {
            mcp::McpServerConfig {
                name: name.to_string(),
                spec: traits::McpTransportSpec::Stdio {
                    command: "prior".into(),
                    args: vec![],
                    env: std::collections::HashMap::new(),
                },
                scope: mcp::ConfigScope::Project,
                disabled: false,
                timeout_ms: None,
                always_load: false,
                config_error: None,
            }
        }
        let open_gates = super::AgentMcpMergeGates {
            safe_mode: false,
            strict_mcp_config: false,
            enterprise_mcp_active: false,
        };
        let no_policy = mcp::enterprise_policy::McpPolicy::default();

        // No definition → no-op (`if(!t)return e`).
        let mut configs = vec![existing("keep")];
        let blocked =
            super::merge_agent_frontmatter_mcp_servers(&mut configs, None, open_gates, &no_policy);
        assert!(blocked.is_empty());
        assert_eq!(configs.len(), 1);

        // Open gates: the frontmatter server joins the to-connect list with
        // scope Agent, exactly like a `--mcp-config` server.
        let def = agent_with_server("docs", agent::AgentSource::Project);
        let mut configs = vec![existing("keep")];
        let blocked = super::merge_agent_frontmatter_mcp_servers(
            &mut configs,
            Some(&def),
            open_gates,
            &no_policy,
        );
        assert!(blocked.is_empty());
        assert_eq!(configs.len(), 2);
        let added = configs.iter().find(|c| c.name == "docs").unwrap();
        assert_eq!(added.scope, mcp::ConfigScope::Agent);

        // `{...allowed, ...existing}` — an existing same-name server WINS.
        let mut configs = vec![existing("docs")];
        super::merge_agent_frontmatter_mcp_servers(
            &mut configs,
            Some(&def),
            open_gates,
            &no_policy,
        );
        assert_eq!(configs.len(), 1);
        assert!(
            matches!(&configs[0].spec, traits::McpTransportSpec::Stdio { command, .. } if command == "prior"),
            "existing config must win on name collision"
        );
        assert_eq!(configs[0].scope, mcp::ConfigScope::Project);

        // Gl(): safe mode → no merge.
        let mut configs = vec![];
        super::merge_agent_frontmatter_mcp_servers(
            &mut configs,
            Some(&def),
            super::AgentMcpMergeGates {
                safe_mode: true,
                ..open_gates
            },
            &no_policy,
        );
        assert!(configs.is_empty());

        // strictMcpConfig: skipped UNLESS the agent came from `--agents`
        // (`t.source !== "flagSettings"`).
        let strict = super::AgentMcpMergeGates {
            strict_mcp_config: true,
            ..open_gates
        };
        let mut configs = vec![];
        super::merge_agent_frontmatter_mcp_servers(&mut configs, Some(&def), strict, &no_policy);
        assert!(configs.is_empty(), "strict mode blocks non-flag agents");
        let flag_def = agent_with_server("docs", agent::AgentSource::Flag);
        let mut configs = vec![];
        super::merge_agent_frontmatter_mcp_servers(
            &mut configs,
            Some(&flag_def),
            strict,
            &no_policy,
        );
        assert_eq!(configs.len(), 1, "flagSettings agents bypass strict mode");

        // T3(): managed-MCP exclusive control → no merge.
        let mut configs = vec![];
        super::merge_agent_frontmatter_mcp_servers(
            &mut configs,
            Some(&def),
            super::AgentMcpMergeGates {
                enterprise_mcp_active: true,
                ..open_gates
            },
            &no_policy,
        );
        assert!(configs.is_empty());

        // Yee: a deny-listed server is BLOCKED (returned for the stderr
        // warning), an allowed sibling still merges.
        let mut two = agent_with_server("docs", agent::AgentSource::Project);
        let mut denied = serde_json::Map::new();
        denied.insert(
            "denied".to_string(),
            serde_json::json!({"command": "evil"}),
        );
        two.mcp_servers
            .push(agent::AgentMcpServerSpec::Record(denied));
        let deny_policy = mcp::enterprise_policy::McpPolicy {
            denied: Some(vec![mcp::enterprise_policy::McpServerMatcher {
                server_name: Some("denied".into()),
                server_command: None,
                server_url: None,
            }]),
            allowed: None,
        };
        let mut configs = vec![];
        let blocked = super::merge_agent_frontmatter_mcp_servers(
            &mut configs,
            Some(&two),
            open_gates,
            &deny_policy,
        );
        assert_eq!(blocked, vec!["denied".to_string()]);
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "docs");
    }

    #[test]
    fn oauth_subscriber_flag_gating() {
        use llm_client::oauth::anthropic::resolver::{resolve, ResolverContext};
        let inference = vec!["user:inference".to_string(), "user:profile".to_string()];
        let no_inference = vec!["user:profile".to_string()];
        // The context `resolve_llm_stack` builds for a stored-OAuth session,
        // parameterized over the sources that can outrank (or force) it.
        let ctx = |managed: bool, env_key: bool, env_token: bool, fd: bool, stored_key: bool| {
            resolve(&ResolverContext {
                managed_oauth_only: managed,
                env_auth_token: env_token.then(|| "tok".to_string()),
                env_api_key: env_key.then(|| "sk-ant".to_string()),
                fd_present: fd,
                has_stored_oauth: true,
                has_stored_api_key: stored_key,
                settings_api_key: None,
                api_key_helper_script: None,
                aws_present: false,
            })
        };
        // Clean OAuth (no overriding env key/token) + inference scope ⇒ subscriber.
        assert!(super::oauth_subscriber_flag(
            &ctx(false, false, false, false, false),
            &inference
        ));
        // Inference scope present, but an env ANTHROPIC_API_KEY outranks stored
        // OAuth in the resolver ⇒ isAnthropicAuthEnabled() false ⇒ not subscriber.
        assert!(!super::oauth_subscriber_flag(
            &ctx(false, true, false, false, false),
            &inference
        ));
        // Likewise an env ANTHROPIC_AUTH_TOKEN bearer outranks stored OAuth.
        assert!(!super::oauth_subscriber_flag(
            &ctx(false, false, true, false, false),
            &inference
        ));
        // (M13) An FD-inherited key (managed/remote launch) outranks stored OAuth.
        assert!(!super::oauth_subscriber_flag(
            &ctx(false, false, false, true, false),
            &inference
        ));
        // (M13) A keychain-STORED key ranks BELOW stored OAuth ⇒ still subscriber.
        assert!(super::oauth_subscriber_flag(
            &ctx(false, false, false, false, true),
            &inference
        ));
        // (M13) HOST forcing — and ONLY host forcing (`KWr()` @228931361,
        // `zb()`'s `(n||i) && !KWr()` term) — makes the stored session outrank
        // an env ANTHROPIC_API_KEY. A managed `forceLoginMethod: "claudeai"`
        // policy is NOT `KWr()` and must never reach this flag: `zb()` has no
        // `forceLoginMethod` term, and `Gde()` (@228967690) REFUSES that
        // combination ("A non-OAuth Anthropic credential cannot satisfy the org
        // pin") rather than silently promoting OAuth.
        assert!(super::oauth_subscriber_flag(
            &ctx(true, true, false, false, false),
            &inference
        ));
        // Clean OAuth but no inference scope (e.g. profile-only) ⇒ not subscriber.
        assert!(!super::oauth_subscriber_flag(
            &ctx(false, false, false, false, false),
            &no_inference
        ));
        // No scopes at all ⇒ not subscriber.
        assert!(!super::oauth_subscriber_flag(
            &ctx(false, false, false, false, false),
            &[]
        ));
    }

    /// `Aa()` (@228959617) / `jW()` both `return null` unless `zb()` holds, so
    /// the tier persisted inside the stored credential is visible ONLY while
    /// the resolver keeps the stored session as the effective source.
    #[test]
    fn subscription_seed_gates_persisted_tier_on_effective_oauth() {
        use llm_client::oauth::anthropic::resolver::AuthSource;
        let inference = vec!["user:inference".to_string()];
        let tier = "enterprise".to_string();
        let limit = "default_claude_max_20x".to_string();

        // OAuth effective ⇒ both `Aa()` and `jW()` read the persisted values.
        let seed = super::subscription_seed(
            &AuthSource::OAuthClaudeAi,
            &inference,
            Some(&tier),
            Some(&limit),
        );
        assert!(seed.is_subscriber);
        assert_eq!(seed.subscription_type.as_deref(), Some("enterprise"));
        assert_eq!(
            seed.rate_limit_tier.as_deref(),
            Some("default_claude_max_20x")
        );

        // An env ANTHROPIC_API_KEY outranks the stored blob ⇒ `zb()` false ⇒
        // NO tier at all (and `Ger()` — the static `is_enterprise` — is false).
        for outranking in [
            AuthSource::EnvApiKey,
            AuthSource::EnvAuthToken,
            AuthSource::FileDescriptor,
        ] {
            let seed = super::subscription_seed(&outranking, &inference, Some(&tier), Some(&limit));
            assert!(!seed.is_subscriber);
            assert_eq!(seed.subscription_type, None);
            assert_eq!(seed.rate_limit_tier, None);
        }

        // `Aa()` gates on `zb()` ALONE — an OAuth-effective session without the
        // `user:inference` scope is not a Claude.ai subscriber, yet its tier is
        // still readable.
        let seed = super::subscription_seed(
            &AuthSource::OAuthClaudeAi,
            &["user:profile".to_string()],
            Some(&tier),
            Some(&limit),
        );
        assert!(!seed.is_subscriber);
        assert_eq!(seed.subscription_type.as_deref(), Some("enterprise"));
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
        let lingxi_home = cwd.join(".lingxi");
        let cfg = DesktopConfig {
            isolated_credential_storage: false,
            api_base: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            api_key_helper: None,
            // (M13) Inert auth-resolver inputs: no managed OAuth forcing, no
            // FD-inherited key.
            managed_oauth_only: false,
            anthropic_key_fd_present: false,
            cwd: cwd.clone(),
            lingxi_home,
            default_model: "claude-sonnet-4-20250514".to_string(),
            // Boot tests must stay deterministic across HOST machines: a dev
            // keychain with real provider keys would otherwise trigger the
            // connected-provider fallback and change the booted model.
            default_model_explicit: true,
            recent_models: Vec::new(),
            fallback_model: None,
            custom_betas: Vec::new(),
            provider_profiles: None,
            routing: None,
            mcp_paths: vec![cwd.join(".mcp.json")],
            use_noop_permission_gate: use_noop,
            deny_unresolved_ask: false,
            max_turns: None,
            plan_mode_instructions: None,
            plans_directory: None,
            max_budget_usd: None,
            json_schema: None,
            injected_permission_gate: None,
            session_started_as_coordinator: false,
            // Boot tests stay deterministic: empty memory, never the real FS.
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
            allow_dangerously_skip_permissions: false,
            connect_prompt: None,
            system_prompt_override: None,
            append_system_prompt: None,
            session_id_override: None,
            parent_session_id: None,
            disable_slash_commands: false,
            add_dir: Vec::new(),
            cli_mcp_servers: Vec::new(),
            strict_mcp_config: false,
            exclude_dynamic_system_prompt_sections: false,
            setting_source_scope: (true, true),
            customization_gates: super::CustomizationGates::default(),
            session_persistence: true,
            cli_agents_json: None,
            cli_agent: None,
            cli_plugin_dirs: Vec::new(),
            initial_effort: None,
            default_model_env_pinned: false,
            session_thinking: Default::default(),
            // No `-w`/`--worktree` flag by default; individual worktree-launch
            // tests override this field via struct-update syntax.
            worktree_launch: None,
            // No `--tmux` flag by default; individual tmux-launch tests
            // override this field via struct-update syntax.
            tmux_launch: None,
            // No `/fork`-to-background forker in tests.
            bg_session_forker: None,
            // The generic desktop test host does not mount an interactive TUI
            // questionnaire surface.
            ask_user_question_tx: None,
            computer_access_tx: None,
        };
        (tmp, cfg)
    }

    // ── worktree-tmux-launch plan Task 3: `-w`/`--worktree [name]` boot ──────

    /// Direct unit test of the extracted [`super::apply_worktree_launch`]:
    /// `worktree_launch == None` (the field's default — see [`test_config`])
    /// must be a complete no-op. INERT INVARIANT: no create, no cwd swap,
    /// `worktree_session` stays `None` — boot with no `-w`/`--worktree` flag
    /// is byte-identical to before this field existed.
    #[tokio::test]
    async fn apply_worktree_launch_is_inert_when_flag_absent() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/inert");
        let ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );

        super::apply_worktree_launch(&None, &None, &ctx)
            .await
            .expect("None must never fail");

        assert!(
            ctx.worktree_session.lock().unwrap().is_none(),
            "no --worktree flag must leave worktree_session None"
        );
        assert_eq!(
            ctx.session_cwd.cwd(),
            boot_cwd,
            "no --worktree flag must never swap the session cwd"
        );
    }

    /// Direct unit test of the extracted [`super::apply_worktree_launch`]:
    /// `Some(name)` creates a worktree through the injected `WorktreeManager`
    /// (a [`tool_api::test_support::MockWorktreeManager`] here — the function
    /// only calls the `WorktreeManager` trait object, so it cannot tell a mock
    /// from `PosixWorktreeManager`), swaps the session cwd into it, and
    /// populates `worktree_session` — mirroring
    /// `EnterWorktreeTool::call_create`'s own create → swap → record sequence
    /// (`entered_existing: false` because boot CREATES, never enters an
    /// existing worktree; `tmux_session_name: None` because `--tmux` wiring is
    /// a separate, not-yet-implemented task).
    #[tokio::test]
    async fn apply_worktree_launch_creates_and_populates_session() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/create");
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );
        let mock = Arc::new(tool_api::test_support::MockWorktreeManager::new());
        ctx.worktree = mock.clone() as Arc<dyn traits::worktree::WorktreeManager>;

        super::apply_worktree_launch(&Some("feat".to_string()), &None, &ctx)
            .await
            .expect("create must succeed against the injected WorktreeManager");

        // The WorktreeManager recorded exactly one create, for the slug passed.
        let created = mock.created();
        assert_eq!(created.len(), 1, "exactly one create_worktree call");
        assert_eq!(created[0].0, "feat");
        let handle = created[0].1.clone();

        assert_eq!(
            ctx.session_cwd.cwd(),
            handle.path,
            "session cwd must be swapped into the created worktree"
        );

        let session = ctx
            .worktree_session
            .lock()
            .unwrap()
            .clone()
            .expect("worktree_session must be populated after a --worktree boot launch");
        assert_eq!(session.original_cwd, boot_cwd, "captured PRE-swap cwd");
        assert_eq!(session.worktree_path, handle.path);
        assert_eq!(session.branch_name, handle.branch_name);
        assert!(
            !session.entered_existing,
            "boot CREATES the worktree, never enters an existing one"
        );
        assert_eq!(
            session.tmux_session_name, None,
            "--tmux wiring is a separate task; boot launch always records None"
        );
    }

    // ── worktree-tmux-launch plan Task 4: `--tmux` boot tmux session ────────

    /// In-test `ProcessRunner` that records every command it's handed and
    /// returns a per-command canned exit code — lets these tests assert BOTH
    /// the resulting `tmux_session_name` and whether a given tmux call happened
    /// at all. Dispatches on the argv: `tmux -V` (the native-mode install
    /// probe, `i4i()`) gets [`Self::probe_exit`]; every other invocation
    /// (`tmux new-session ...`, the create) gets [`Self::create_exit`]. Mirrors
    /// the `MockRunner` pattern in `platform_posix::worktree_tmux`'s own tests.
    struct RecordingProcessRunner {
        probe_exit: i32,
        create_exit: i32,
        calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    }

    impl RecordingProcessRunner {
        /// `tmux -V` probe SUCCEEDS (tmux installed); the `new-session` create
        /// returns `create_exit`. This is the common case: existing callers
        /// `new(0)` (create ok) / `new(1)` (create fails non-fatally) keep
        /// their meaning now that a native-mode probe precedes the create.
        fn new(create_exit: i32) -> Self {
            Self {
                probe_exit: 0,
                create_exit,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        /// The `tmux -V` probe returns `probe_exit` (non-zero ⇒ "not
        /// installed"); the create returns `create_exit`.
        fn with_exits(probe_exit: i32, create_exit: i32) -> Self {
            Self {
                probe_exit,
                create_exit,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        /// Count of `tmux -V` install-probe calls issued.
        fn probe_calls(&self) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, args)| args == &vec!["-V".to_string()])
                .count()
        }

        /// Count of `tmux new-session ...` create calls issued.
        fn create_calls(&self) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, args)| args.first().map(String::as_str) == Some("new-session"))
                .count()
        }
    }

    #[async_trait::async_trait]
    impl traits::ProcessRunner for RecordingProcessRunner {
        async fn run(
            &self,
            cmd: &traits::SandboxedCommand,
        ) -> Result<traits::ProcessOutput, traits::ProcessError> {
            let inner = cmd.inner();
            let exit_code = if inner.args == vec!["-V".to_string()] {
                self.probe_exit
            } else {
                self.create_exit
            };
            self.calls
                .lock()
                .unwrap()
                .push((inner.command.clone(), inner.args.clone()));
            Ok(traits::ProcessOutput {
                stdout: String::new(),
                stderr: if exit_code == 0 {
                    String::new()
                } else {
                    "boom".to_string()
                },
                exit_code,
                timed_out: false,
            })
        }

        async fn spawn_background(
            &self,
            _cmd: &traits::SandboxedCommand,
        ) -> Result<traits::ProcessHandle, traits::ProcessError> {
            Err(traits::ProcessError::Unsupported)
        }

        async fn kill(&self, _handle: &traits::ProcessHandle) -> Result<(), traits::ProcessError> {
            Ok(())
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    /// `--worktree feat --tmux` (both `Some`), tmux invocation succeeds (exit
    /// 0): `apply_worktree_launch` must create exactly one tmux session and
    /// record ITS EXACT derived name
    /// ([`platform_posix::worktree_tmux::worktree_tmux_session_name`], keyed
    /// on the pre-swap boot cwd as the repo root + the `--worktree` slug) into
    /// the shared `worktree_session.tmux_session_name`.
    #[tokio::test]
    async fn apply_worktree_launch_with_tmux_creates_and_records_session_name() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/tmux-ok");
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );
        let mock = Arc::new(tool_api::test_support::MockWorktreeManager::new());
        ctx.worktree = mock.clone() as Arc<dyn traits::worktree::WorktreeManager>;
        let runner = Arc::new(RecordingProcessRunner::new(0));
        ctx.process = runner.clone() as Arc<dyn traits::ProcessRunner>;

        super::apply_worktree_launch(&Some("feat".to_string()), &Some(String::new()), &ctx)
            .await
            .expect("worktree + tmux launch must succeed");

        assert_eq!(
            runner.probe_calls(),
            1,
            "native (bare --tmux) must run the `tmux -V` install pre-flight"
        );
        assert_eq!(runner.create_calls(), 1, "exactly one tmux new-session");

        let expected_name =
            platform_posix::worktree_tmux::worktree_tmux_session_name(&boot_cwd, "feat");
        let session = ctx
            .worktree_session
            .lock()
            .unwrap()
            .clone()
            .expect("worktree_session must be populated");
        assert_eq!(session.tmux_session_name, Some(expected_name));
    }

    /// A tmux invocation that fails (non-zero exit) must NOT fail boot — the
    /// worktree itself already succeeded — and must leave
    /// `tmux_session_name` as `None` (only a tmux SUCCESS records the name).
    #[tokio::test]
    async fn apply_worktree_launch_tmux_failure_is_non_fatal() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/tmux-fail");
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );
        let mock = Arc::new(tool_api::test_support::MockWorktreeManager::new());
        ctx.worktree = mock.clone() as Arc<dyn traits::worktree::WorktreeManager>;
        let runner = Arc::new(RecordingProcessRunner::new(1));
        ctx.process = runner.clone() as Arc<dyn traits::ProcessRunner>;

        super::apply_worktree_launch(&Some("feat".to_string()), &Some(String::new()), &ctx)
            .await
            .expect("a tmux failure must not fail boot");

        assert_eq!(
            runner.create_calls(),
            1,
            "tmux new-session was attempted once"
        );
        let session = ctx
            .worktree_session
            .lock()
            .unwrap()
            .clone()
            .expect("worktree_session must still be populated — the worktree itself succeeded");
        assert_eq!(
            session.tmux_session_name, None,
            "a failed tmux create must leave tmux_session_name None"
        );
    }

    /// INERT companion: `--worktree feat` WITHOUT `--tmux` must issue NO tmux
    /// call at all (not merely record `None` — the process runner must never
    /// be invoked), and `tmux_session_name` stays `None`.
    #[tokio::test]
    async fn apply_worktree_launch_without_tmux_flag_issues_no_tmux_call() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/tmux-inert");
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );
        let mock = Arc::new(tool_api::test_support::MockWorktreeManager::new());
        ctx.worktree = mock.clone() as Arc<dyn traits::worktree::WorktreeManager>;
        let runner = Arc::new(RecordingProcessRunner::new(0));
        ctx.process = runner.clone() as Arc<dyn traits::ProcessRunner>;

        super::apply_worktree_launch(&Some("feat".to_string()), &None, &ctx)
            .await
            .expect("worktree-only launch must succeed");

        assert_eq!(
            runner.call_count(),
            0,
            "no --tmux flag must issue no tmux call"
        );
        let session = ctx
            .worktree_session
            .lock()
            .unwrap()
            .clone()
            .expect("worktree_session must be populated");
        assert_eq!(
            session.tmux_session_name, None,
            "no --tmux flag must leave tmux_session_name None"
        );
    }

    /// `--tmux` requires `--worktree`: `tmux_launch.is_some()` with
    /// `worktree_launch: None` must be a hard boot failure
    /// (`BuildError::TmuxRequiresWorktree`), not a silent ignore, and must
    /// never touch `worktree_session`.
    #[tokio::test]
    async fn apply_worktree_launch_tmux_without_worktree_is_a_hard_error() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd =
            std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/tmux-requires-worktree");
        let ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );

        let err = super::apply_worktree_launch(&None, &Some(String::new()), &ctx)
            .await
            .expect_err("--tmux without --worktree must be a hard boot failure");
        assert!(matches!(err, super::BuildError::TmuxRequiresWorktree));
        assert!(ctx.worktree_session.lock().unwrap().is_none());
    }

    /// Native-mode (`--worktree feat --tmux`, bare) with tmux NOT installed:
    /// the `tmux -V` pre-flight fails, so boot HARD-fails with
    /// `BuildError::TmuxNotInstalled` (payload = the platform install hint) —
    /// BEFORE any worktree is created. 206 `re` branch: `!await i4i() → "tmux
    /// is not installed.\n"+s4i()`.
    #[cfg(not(windows))]
    #[tokio::test]
    async fn apply_worktree_launch_native_tmux_not_installed_is_hard_error() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/tmux-missing");
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );
        let mock = Arc::new(tool_api::test_support::MockWorktreeManager::new());
        ctx.worktree = mock.clone() as Arc<dyn traits::worktree::WorktreeManager>;
        // `tmux -V` probe returns non-zero ⇒ "not installed"; create exit is
        // irrelevant (never reached).
        let runner = Arc::new(RecordingProcessRunner::with_exits(127, 0));
        ctx.process = runner.clone() as Arc<dyn traits::ProcessRunner>;

        let err =
            super::apply_worktree_launch(&Some("feat".to_string()), &Some(String::new()), &ctx)
                .await
                .expect_err("native --tmux with tmux absent must hard-fail boot");
        assert!(
            matches!(err, super::BuildError::TmuxNotInstalled(ref hint)
                if hint == platform_posix::worktree_tmux::tmux_install_hint()),
            "expected TmuxNotInstalled with the platform hint, got {err:?}"
        );
        // Pre-flight fired and short-circuited: probe ran, NO create, NO worktree.
        assert_eq!(runner.probe_calls(), 1, "the `tmux -V` probe ran");
        assert_eq!(
            runner.create_calls(),
            0,
            "no new-session after a failed probe"
        );
        assert_eq!(
            mock.created().len(),
            0,
            "no worktree created on pre-flight failure"
        );
        assert!(
            ctx.worktree_session.lock().unwrap().is_none(),
            "no session recorded"
        );
        assert_eq!(ctx.session_cwd.cwd(), boot_cwd, "cwd unchanged");
    }

    /// Classic-mode (`--tmux=classic`) with tmux NOT installed: the native
    /// pre-flight is SKIPPED (206 gates it on `a.tmux===true`, i.e. bare only),
    /// so boot proceeds — the worktree IS created and the `tmux new-session`
    /// create is attempted, failing NON-fatally (session name stays `None`).
    /// Crucially, NO `tmux -V` probe is issued.
    #[tokio::test]
    async fn apply_worktree_launch_classic_tmux_skips_install_preflight() {
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let boot_cwd = std::path::PathBuf::from("/tmp/lingxi-worktree-launch-test/tmux-classic");
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![boot_cwd.clone()],
        );
        let mock = Arc::new(tool_api::test_support::MockWorktreeManager::new());
        ctx.worktree = mock.clone() as Arc<dyn traits::worktree::WorktreeManager>;
        // Probe would report "not installed" IF it ran; create fails. Classic
        // must not run the probe, and the create failure must be non-fatal.
        let runner = Arc::new(RecordingProcessRunner::with_exits(127, 1));
        ctx.process = runner.clone() as Arc<dyn traits::ProcessRunner>;

        super::apply_worktree_launch(
            &Some("feat".to_string()),
            &Some("classic".to_string()),
            &ctx,
        )
        .await
        .expect("classic --tmux skips the install pre-flight and does not hard-fail");

        assert_eq!(
            runner.probe_calls(),
            0,
            "classic mode must NOT run the native `tmux -V` pre-flight"
        );
        assert_eq!(
            runner.create_calls(),
            1,
            "classic still attempts the create"
        );
        assert_eq!(mock.created().len(), 1, "the worktree was still created");
        let session = ctx
            .worktree_session
            .lock()
            .unwrap()
            .clone()
            .expect("worktree_session populated");
        assert_eq!(
            session.tmux_session_name, None,
            "the failed create leaves tmux_session_name None (non-fatal)"
        );
    }

    /// worktree-tmux-launch plan Task 3, boot-level integration test: driving
    /// the FULL `build()` (a real git repo + the real `PosixWorktreeManager`
    /// `build()` unconditionally wires) with `cfg.worktree_launch` set must
    /// leave the LIVE session cwd inside the created worktree. There is no
    /// direct `DesktopRuntime` accessor for `session_cwd`/`worktree_session`
    /// (they are consumed into the orchestrator + tool registry), so this
    /// observes the swap the same way the running system does: through
    /// `ConversationOrchestrator::assemble_system_prompt_preview()`, whose
    /// `Primary working directory:` line re-derives from the SAME
    /// `Arc<SessionCwd>` every turn (see `build_prompt_context`'s doc comment).
    /// The unit tests above already cover `worktree_session`'s exact shape.
    #[tokio::test]
    async fn worktree_launch_flag_creates_and_enters_worktree_at_boot() {
        let (tmp, mut cfg) = test_config(true);
        init_git_repo_for_worktree_launch_test(tmp.path()).await;
        cfg.worktree_launch = Some("feat".to_string());

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("--worktree build must succeed in a real git repo");

        let expected_path = tmp.path().join(".lingxi").join("worktrees").join("feat");
        assert!(
            expected_path.exists(),
            "create_worktree must have materialized {expected_path:?} on disk"
        );

        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        assert!(
            sys.contains(&format!(
                "Primary working directory: {}",
                expected_path.display()
            )),
            "the LIVE session cwd the system prompt re-derives every turn must be \
             the boot-launched worktree: {sys}"
        );
    }

    /// INERT INVARIANT companion to the above: with no `-w`/`--worktree` flag
    /// (`cfg.worktree_launch == None`, `test_config`'s default), a full
    /// `build()` must create no worktree and leave the plain boot cwd as the
    /// live session cwd — boot stays byte-identical to before this field
    /// existed.
    #[tokio::test]
    async fn no_worktree_flag_leaves_boot_cwd_untouched_at_boot() {
        let (tmp, cfg) = test_config(true);
        assert!(
            cfg.worktree_launch.is_none(),
            "test_config's default must be inert"
        );

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        assert!(
            !tmp.path().join(".lingxi").join("worktrees").exists(),
            "no --worktree flag must never create a worktrees dir"
        );
        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        assert!(
            sys.contains(&format!(
                "Primary working directory: {}",
                tmp.path().display()
            )),
            "no --worktree flag must leave the plain boot cwd as the live session cwd: {sys}"
        );
    }

    /// Initialize a minimal git repo with one commit (mirrors
    /// `platforms/posix/src/worktree.rs`'s `create_tests::init_repo` helper) so
    /// `PosixWorktreeManager::create_worktree` — which `build()`
    /// unconditionally wires as `tool_ctx.worktree` — has something to branch
    /// from. `test_config`'s tempdir is NOT a git repo by default (most
    /// `build()` tests never touch the worktree subsystem), so the boot-level
    /// worktree-launch test above opts into this explicitly.
    async fn init_git_repo_for_worktree_launch_test(dir: &std::path::Path) {
        async fn git(dir: &std::path::Path, args: &[&str]) {
            let mut c = tokio::process::Command::new("git");
            c.current_dir(dir);
            for a in args {
                c.arg(a);
            }
            assert!(
                c.output().await.unwrap().status.success(),
                "git {args:?} failed"
            );
        }
        git(dir, &["init", "-q", "-b", "main"]).await;
        git(dir, &["config", "user.email", "ci@test"]).await;
        git(dir, &["config", "user.name", "ci"]).await;
        tokio::fs::write(dir.join("seed.txt"), "seed")
            .await
            .unwrap();
        git(dir, &["add", "seed.txt"]).await;
        git(dir, &["commit", "-qm", "seed"]).await;
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

    /// P1-08: a `--add-dir` directory must land in BOTH the file-tool trusted
    /// set (`session_cwd.trusted_dirs`) AND the live MCP roots source
    /// (`mcp_registry.additional_roots_snapshot`) at boot, and the two
    /// runtime handles the CLI `/add-dir` effect uses must be exposed on the
    /// `DesktopRuntime`. This pins the composition-root wiring the runtime add
    /// builds on.
    #[tokio::test]
    async fn build_threads_add_dir_into_trusted_dirs_and_mcp_roots() {
        let (tmp, mut cfg) = test_config(true);
        // A real existing directory under the tempdir (must be absolute so
        // `expand_trusted_dir` takes it verbatim, matching the value the TUI's
        // `resolve_and_validate` produces).
        let extra = tmp.path().join("extra");
        std::fs::create_dir_all(&extra).expect("mkdir extra");
        cfg.add_dir = vec![extra.clone()];

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        assert!(
            rt.session_cwd.trusted_dirs().contains(&extra),
            "--add-dir must widen the file-tool trusted set: {:?}",
            rt.session_cwd.trusted_dirs(),
        );
        assert!(
            rt.mcp_registry.additional_roots_snapshot().contains(&extra),
            "--add-dir must seed the live MCP roots source: {:?}",
            rt.mcp_registry.additional_roots_snapshot(),
        );

        // The runtime add itself: a NEW dir takes live effect + reports change;
        // re-adding it is a no-op (jzn), and it lands in both surfaces.
        let extra2 = tmp.path().join("extra2");
        assert!(rt.session_cwd.add_trusted_dir(extra2.clone()));
        assert!(rt.mcp_registry.add_root(extra2.clone()));
        assert!(!rt.mcp_registry.add_root(extra2.clone()), "jzn dedupe");
        assert!(rt.session_cwd.trusted_dirs().contains(&extra2));
        assert!(rt
            .mcp_registry
            .additional_roots_snapshot()
            .contains(&extra2));
    }

    /// RV3: a RAW (relative / `~`-prefixed) `--add-dir` / settings
    /// `additionalDirectories` entry must be EXPANDED to an absolute path before
    /// it seeds the live MCP `roots/list` cell — matching the file-tool
    /// `trusted_dirs` set, which already expands. Otherwise a raw `data` would
    /// reach `format!("file://{}")` as `file://data` (authority `data`, empty
    /// path) instead of claude-code's resolvable `file:///<cwd>/data`, and the
    /// two sets would permanently desync (a runtime `/add-dir <abs>` dedupes
    /// against the already-expanded trusted set → the stale raw roots entry is
    /// never corrected). Pins that BOTH surfaces hold the SAME absolute path.
    #[tokio::test]
    async fn build_expands_relative_add_dir_before_seeding_mcp_roots() {
        let (tmp, mut cfg) = test_config(true);
        // A RAW *relative* `--add-dir` entry (no host resolution at boot). The
        // real dir exists under cwd so the expansion target is concrete.
        std::fs::create_dir_all(tmp.path().join("data")).expect("mkdir data");
        cfg.add_dir = vec![std::path::PathBuf::from("data")];
        let expected = cfg.cwd.join("data"); // expand_trusted_dir(relative) = cwd.join

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        // File-tool trusted set already expanded (unchanged baseline).
        assert!(
            rt.session_cwd.trusted_dirs().contains(&expected),
            "trusted_dirs must hold the EXPANDED path: {:?}",
            rt.session_cwd.trusted_dirs(),
        );
        // The MCP roots seed must ALSO be expanded, not the raw `data` — this is
        // the RV3 regression: the snapshot must contain the absolute path and
        // must NOT contain the raw relative entry.
        let snapshot = rt.mcp_registry.additional_roots_snapshot();
        assert!(
            snapshot.contains(&expected),
            "MCP roots cell must be seeded with the EXPANDED path: {snapshot:?}",
        );
        assert!(
            !snapshot.contains(&std::path::PathBuf::from("data")),
            "MCP roots cell must NOT carry the raw relative entry: {snapshot:?}",
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
        // Lay down a settings.json with an `xaaIdp` block under lingxi_home.
        std::fs::create_dir_all(&cfg.lingxi_home).unwrap();
        std::fs::write(
            cfg.lingxi_home.join("settings.json"),
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

    /// GAP E: a plugin installed on disk under `<lingxi_home>/plugins` is
    /// discovered + materialised at bootstrap — its command lands in the live
    /// command registry the slash dispatcher reads.
    #[tokio::test]
    async fn build_discovers_and_materialises_an_installed_plugin() {
        let (_tmp, cfg) = test_config(true);
        // Lay down a fixture plugin under `<lingxi_home>/plugins/myplugin`.
        let plugin_dir = cfg.lingxi_home.join("plugins").join("myplugin");
        std::fs::create_dir_all(plugin_dir.join(".lingxi-plugin")).unwrap();
        std::fs::write(
            plugin_dir.join(".lingxi-plugin").join("plugin.json"),
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

    /// The desktop runtime must surface the SAME shared command registry the
    /// slash dispatcher reads so bridge hosts can emit `SlashCommandCatalog`
    /// pulls and `CommandsChanged` diffs from live state without reconstructing
    /// a parallel registry snapshot.
    #[tokio::test]
    async fn build_exposes_dispatcher_shared_command_registry() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink).await.expect("build() failed");

        assert!(
            Arc::ptr_eq(&rt.shared_command_registry, &rt.dispatcher.registry()),
            "DesktopRuntime must expose the dispatcher's live shared registry"
        );
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
            .lingxi_home
            .join("plugins")
            .join("cache")
            .join("acme")
            .join("weather")
            .join("1.0.0");
        std::fs::create_dir_all(versioned.join(".lingxi-plugin")).unwrap();
        std::fs::write(
            versioned.join(".lingxi-plugin").join("plugin.json"),
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
            cfg.lingxi_home.join("settings.json"),
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

    /// GAP E: a fresh install with no `<lingxi_home>/plugins` directory boots
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
        assert_eq!(
            super::provider_profile_label("github-copilot"),
            "GitHub Copilot"
        );
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
        assert!(
            ids.contains(&"claude-opus-4-6"),
            "missing default opus: {ids:?}"
        );
        assert!(
            ids.contains(&"claude-sonnet-4-6"),
            "missing default sonnet: {ids:?}"
        );
        assert!(
            ids.contains(&"claude-haiku-4-5"),
            "missing default haiku: {ids:?}"
        );
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

    #[test]
    fn anthropic_models_for_excludes_foreign_profile_qualified_default() {
        // Regression: a default/fallback qualified for ANOTHER provider must NOT
        // be injected into the anthropic profile — otherwise the same id lives in
        // both the anthropic and (e.g.) openrouter model lists and `--model
        // <id>` fails with a spurious "ambiguous across profiles".
        let m = super::anthropic_models_for(
            "openrouter/meta-llama/llama-3.3-70b-instruct:free",
            Some("github-copilot/claude-opus-4.8"),
        );
        let ids: Vec<&str> = m.iter().map(|x| x.display_model.as_str()).collect();
        assert!(
            !ids.iter().any(|id| id.contains("llama-3.3-70b")),
            "openrouter model must NOT be in the anthropic profile: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.contains("github-copilot")),
            "copilot fallback must NOT be in the anthropic profile: {ids:?}"
        );
        // The first-party Claude defaults are still present.
        assert!(
            ids.contains(&"claude-opus-4-8"),
            "claude defaults kept: {ids:?}"
        );

        // An `anthropic/…`-qualified default IS registered, as its BARE id.
        let q = super::anthropic_models_for("anthropic/claude-opus-4-6", None);
        let qids: Vec<&str> = q.iter().map(|x| x.display_model.as_str()).collect();
        assert!(
            qids.contains(&"claude-opus-4-6"),
            "anthropic-qualified kept bare: {qids:?}"
        );
        assert!(
            !qids.iter().any(|id| id.contains('/')),
            "no profile-qualified id leaks into the model list: {qids:?}"
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

    /// Boot-time connected-provider fallback: with NO anthropic key/oauth and a
    /// connected user provider (env-key), `build()` reroutes the default model
    /// off the disconnected anthropic route and surfaces the notice. Assertions
    /// are host-robust: a dev keychain may connect OTHER providers too, so the
    /// exact fallback target is not pinned — only the mechanism is.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env mutation across async tests
    async fn build_reroutes_disconnected_default_to_connected_provider() {
        // The reroute is gated on `api_provider() == FirstParty`, which reads
        // the CLAUDE_CODE_USE_* env the deprecation tests mutate — serialize on
        // their lock and clear the flags so a parallel test can't flip the gate.
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();
        let (_tmp, mut cfg) = test_config(true);
        cfg.default_model_explicit = false;
        cfg.provider_profiles = Some({
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "groq".to_string(),
                serde_json::json!({
                    "type": "openai",
                    "baseUrl": "https://api.groq.com/openai/v1",
                    "apiKeyEnv": "LINGXI_TEST_REROUTE_KEY",
                    "models": ["llama-3.3-70b-versatile"]
                }),
            );
            m
        });
        // Unique test-only var: guarantees ≥1 connected provider on any host.
        std::env::set_var("LINGXI_TEST_REROUTE_KEY", "k");
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        std::env::remove_var("LINGXI_TEST_REROUTE_KEY");

        let notice = rt
            .default_model_fallback
            .clone()
            .expect("disconnected anthropic default must reroute");
        assert_eq!(notice.from, "claude-sonnet-4-20250514");
        // The booted model is the notice's target (bare id after the profile split)…
        let bare = notice
            .to
            .split_once('/')
            .map_or(notice.to.as_str(), |(_, m)| m);
        assert_eq!(rt.orchestrator.default_model(), bare);
        // …and its provider is genuinely connected per the same availability map.
        if let Some((profile, _)) = notice.to.split_once('/') {
            assert_eq!(
                rt.provider_availability.get(profile),
                Some(&true),
                "fallback target's provider must be connected: {:?}",
                rt.provider_availability
            );
        }
    }

    /// An EXPLICIT `--model` choice is never overridden by the fallback, even
    /// with the same connected user provider present.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env mutation across async tests
    async fn build_keeps_explicit_model_despite_disconnected_provider() {
        // Same env-serialization as the reroute test above (this test's env-var
        // write must also not leak into a parallel availability assertion).
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();
        let (_tmp, mut cfg) = test_config(true);
        cfg.default_model_explicit = true;
        cfg.provider_profiles = Some({
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "groq".to_string(),
                serde_json::json!({
                    "type": "openai",
                    "baseUrl": "https://api.groq.com/openai/v1",
                    "apiKeyEnv": "LINGXI_TEST_REROUTE_KEY_EXPLICIT",
                    "models": ["llama-3.3-70b-versatile"]
                }),
            );
            m
        });
        std::env::set_var("LINGXI_TEST_REROUTE_KEY_EXPLICIT", "k");
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        std::env::remove_var("LINGXI_TEST_REROUTE_KEY_EXPLICIT");

        assert!(rt.default_model_fallback.is_none());
        assert_eq!(rt.orchestrator.default_model(), "claude-sonnet-4-20250514");
    }

    /// An `ANTHROPIC_MODEL` env pin (claude-code D4) is exempt from the reroute
    /// exactly like an explicit `--model`, even though `default_model_explicit`
    /// stays `false` (it is kept `--model`-only for the `--agent` override gate).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env mutation across async tests
    async fn build_keeps_env_pinned_model_despite_disconnected_provider() {
        let _guard = DEPR_ENV_LOCK.lock().unwrap();
        clear_provider_env();
        let (_tmp, mut cfg) = test_config(true);
        // NOT an explicit --model choice, but env-pinned via ANTHROPIC_MODEL.
        cfg.default_model_explicit = false;
        cfg.default_model_env_pinned = true;
        cfg.provider_profiles = Some({
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "groq".to_string(),
                serde_json::json!({
                    "type": "openai",
                    "baseUrl": "https://api.groq.com/openai/v1",
                    "apiKeyEnv": "LINGXI_TEST_REROUTE_KEY_ENVPIN",
                    "models": ["llama-3.3-70b-versatile"]
                }),
            );
            m
        });
        std::env::set_var("LINGXI_TEST_REROUTE_KEY_ENVPIN", "k");
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        std::env::remove_var("LINGXI_TEST_REROUTE_KEY_ENVPIN");

        assert!(
            rt.default_model_fallback.is_none(),
            "an ANTHROPIC_MODEL env pin must not be rerouted"
        );
        assert_eq!(rt.orchestrator.default_model(), "claude-sonnet-4-20250514");
    }

    /// T2a: a default `build()` surfaces `provider_auth_methods` keyed by
    /// `profile_name` with one of the three tag-vocabulary strings
    /// ("api_key" | "copilot_device" | "oauth"), derived from the builtin catalog.
    #[tokio::test]
    async fn build_surfaces_provider_auth_methods_from_catalog() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        let m = &rt.provider_auth_methods;
        // Catalog-derived: keys are real profile_names, values are the tag vocabulary.
        assert_eq!(m.get("anthropic").map(String::as_str), Some("api_key"));
        assert_eq!(
            m.get("github-copilot").map(String::as_str),
            Some("copilot_device")
        );
        assert_eq!(m.get("openai-chatgpt").map(String::as_str), Some("oauth"));
        assert!(m
            .values()
            .all(|v| matches!(v.as_str(), "api_key" | "copilot_device" | "oauth")));
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
    /// `cwd/.lingxi/settings.json` that `build()` reads at boot. `build()` must
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
        // (cwd/.lingxi/settings.json) — a single `SessionStart` command hook.
        let lingxi_dir = cfg.cwd.join(".lingxi");
        std::fs::create_dir_all(&lingxi_dir).expect("mk .lingxi");
        std::fs::write(
            lingxi_dir.join("settings.json"),
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

    /// (P2-02 cc2.1.207) `Rft` — a `--agent` hit registers the agent's
    /// frontmatter `hooks` as `mainThreadAgentHooks` (`o_n(e.hooks)`) with
    /// `is_agent=false`, so a declared `Stop` hook stays `Stop` (main thread,
    /// NOT the subagent `Stop`→`SubagentStop` retarget). The agent arrives via
    /// the `--agents` flag payload merged into the FINAL catalog, then selected
    /// by `--agent`; `list_hooks()` reads the wired registry (which includes the
    /// frontmatter bucket), proving the boot path installed the agent's hook.
    #[tokio::test]
    async fn build_registers_main_thread_agent_frontmatter_hooks() {
        use traits::OrchestratorHandle as _;

        let (_tmp, mut cfg) = test_config(true);
        // `--agents` flag agent declaring a frontmatter `Stop` hook. `--agent`
        // selects it, so `Rft` installs the hook onto the main thread.
        cfg.cli_agents_json = Some(
            r#"{ "tester": {
                "description": "a test agent",
                "prompt": "you are the tester",
                "hooks": { "Stop": [ { "hooks": [
                    { "type": "command", "command": "true" }
                ] } ] }
            } }"#
                .to_string(),
        );
        cfg.cli_agent = Some("tester".to_string());

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() must succeed with a --agent frontmatter hook");

        let hooks = rt.orchestrator.list_hooks().await;
        // The agent's frontmatter Stop hook is installed on the MAIN thread:
        // `is_agent=false` keeps it as `Stop` (a subagent registration would
        // retarget it to `SubagentStop`).
        assert!(
            hooks.iter().any(|h| h.event == "Stop"),
            "the --agent frontmatter Stop hook must be registered as a main-thread \
             (Stop, not SubagentStop) hook: {hooks:?}"
        );
        assert!(
            !hooks.iter().any(|h| h.event == "SubagentStop"),
            "is_agent=false must NOT retarget the main-thread agent's Stop hook: {hooks:?}"
        );
    }

    /// (M7 cc2.1.220) `FWt` end-to-end: a `--agent` selection whose definition
    /// declares inline frontmatter `mcpServers` gets those servers MERGED into
    /// the boot config list BEFORE `connect_all`, so they REGISTER in the live
    /// MCP registry exactly like `--mcp-config` servers (connect failure is
    /// fine — a dead command still registers as `Disconnected`). ByName entries
    /// materialize nothing (claude `obs` skips strings — the host resolves
    /// them by name against already-configured servers).
    #[tokio::test]
    async fn build_registers_agent_frontmatter_mcp_servers() {
        let (_tmp, mut cfg) = test_config(true);
        cfg.cli_agents_json = Some(
            r#"{ "tester": {
                "description": "a test agent",
                "prompt": "you are the tester",
                "mcpServers": [
                    "slack",
                    { "docs": { "command": "/nonexistent-lingxi-m7-mcp", "args": [] } }
                ]
            } }"#
                .to_string(),
        );
        cfg.cli_agent = Some("tester".to_string());

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() must succeed with agent frontmatter mcpServers");

        let names = rt.mcp_registry.server_names().await;
        assert!(
            names.iter().any(|n| n == "docs"),
            "the agent's inline frontmatter server must register in the live \
             MCP registry (like a --mcp-config server): {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "slack"),
            "a ByName entry must NOT materialize a server config: {names:?}"
        );
    }

    /// (M7 cc2.1.220) `FWt` gate: with NO `--agent` selection the same agents
    /// payload contributes NO MCP servers (the merge consults only the RESOLVED
    /// main-thread agent).
    #[tokio::test]
    async fn build_without_agent_selection_registers_no_frontmatter_mcp_servers() {
        let (_tmp, mut cfg) = test_config(true);
        cfg.cli_agents_json = Some(
            r#"{ "tester": {
                "description": "a test agent",
                "prompt": "you are the tester",
                "mcpServers": [
                    { "docs": { "command": "/nonexistent-lingxi-m7-mcp", "args": [] } }
                ]
            } }"#
                .to_string(),
        );
        cfg.cli_agent = None;

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() must succeed");

        let names = rt.mcp_registry.server_names().await;
        assert!(
            !names.iter().any(|n| n == "docs"),
            "an unselected agent's frontmatter servers must not register: {names:?}"
        );
    }

    /// (P2-02 cc2.1.207) `rVe` resume restoration round-trip: an EXPLICIT
    /// `--agent` boot PERSISTS the applied `agentType` as an `agent-setting`
    /// transcript record; a subsequent `--resume` of the SAME session with NO
    /// `--agent` reads it back and re-adopts the agent (`bde`+`Rft`) — proven by
    /// the agent's frontmatter `Stop` hook being re-registered on the resumed
    /// boot even though `cli_agent` is `None`. Both boots share one
    /// `cwd`/`lingxi_home`/`session_id_override` so the second reads the first's
    /// on-disk `<uuid>.jsonl`.
    #[tokio::test]
    async fn build_persists_and_restores_agent_setting_on_resume() {
        use traits::OrchestratorHandle as _;

        let (_tmp, mut cfg) = test_config(true);
        let agents = r#"{ "tester": {
                "description": "a test agent",
                "prompt": "you are the tester",
                "hooks": { "Stop": [ { "hooks": [
                    { "type": "command", "command": "true" }
                ] } ] }
            } }"#;
        cfg.cli_agents_json = Some(agents.to_string());
        // Pin a fixed session id so the resume boot targets the same transcript.
        let session_id = "33333333-4444-5555-6666-777777777777";
        cfg.session_id_override = Some(session_id.to_string());

        // Second boot's config: identical paths + session, agent STILL in the
        // catalog (`activeAgents`), but NO `--agent` — restoration must come from
        // the persisted `agent-setting` record.
        let mut resume_cfg = cfg.clone();
        resume_cfg.cli_agent = None;

        // First boot: `--agent tester` applies + persists the `agent-setting`.
        cfg.cli_agent = Some("tester".to_string());
        let output1: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm1: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let _rt1 = build(cfg, output1, perm1)
            .await
            .expect("first boot with --agent must succeed");

        // Resume boot: no `--agent`; `rVe` reads the persisted record and re-adopts.
        let output2: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm2: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt2 = build(resume_cfg, output2, perm2)
            .await
            .expect("resume boot must succeed and restore the agent");

        let hooks = rt2.orchestrator.list_hooks().await;
        assert!(
            hooks.iter().any(|h| h.event == "Stop"),
            "resume `rVe` must re-register the persisted agent's frontmatter Stop \
             hook without a re-passed --agent: {hooks:?}"
        );
        assert!(
            !hooks.iter().any(|h| h.event == "SubagentStop"),
            "resume restoration is main-thread (is_agent=false): {hooks:?}"
        );
    }

    /// P1 resolved-agent snapshot: when the persisted agent is gone from the
    /// resumed catalog, a versioned and integrity-checked snapshot still restores
    /// the behavior that was active when the session was created. Legacy
    /// transcripts without a snapshot continue to use the old name lookup and
    /// therefore fall back to the default when the name is unavailable.
    #[tokio::test]
    async fn build_resume_missing_catalog_agent_uses_persisted_snapshot() {
        use traits::OrchestratorHandle as _;

        let (_tmp, mut cfg) = test_config(true);
        let agents = r#"{ "tester": {
                "description": "a test agent",
                "prompt": "you are the tester",
                "hooks": { "Stop": [ { "hooks": [
                    { "type": "command", "command": "true" }
                ] } ] }
            } }"#;
        cfg.cli_agents_json = Some(agents.to_string());
        let session_id = "44444444-5555-6666-7777-888888888888";
        cfg.session_id_override = Some(session_id.to_string());

        // Resume config: same session, but the agent catalog is EMPTY (the agent
        // is no longer available) and no `--agent` is passed.
        let mut resume_cfg = cfg.clone();
        resume_cfg.cli_agent = None;
        resume_cfg.cli_agents_json = None;

        cfg.cli_agent = Some("tester".to_string());
        let output1: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm1: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let _rt1 = build(cfg, output1, perm1)
            .await
            .expect("first boot with --agent must succeed");

        let output2: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm2: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let rt2 = build(resume_cfg, output2, perm2)
            .await
            .expect("resume boot with a missing agent must still succeed");

        let hooks = rt2.orchestrator.list_hooks().await;
        assert!(
            hooks.iter().any(|h| h.event == "Stop"),
            "a resumed agent must retain its snapshotted frontmatter hook even \
             after the catalog entry is removed: {hooks:?}"
        );
    }

    /// Legacy `agent-setting` records contain only the agent name. They retain
    /// the pre-snapshot behavior: resolve by name and fail back to the default
    /// when that catalog entry is no longer available.
    #[tokio::test]
    async fn build_resume_legacy_missing_agent_falls_back_to_default() {
        use traits::OrchestratorHandle as _;

        let (_tmp, mut cfg) = test_config(true);
        let session_id = "55555555-6666-7777-8888-999999999999";
        cfg.session_id_override = Some(session_id.to_string());
        cfg.cli_agent = None;
        cfg.cli_agents_json = None;

        let transcript_path =
            session::jsonl::session_path(&cfg.lingxi_home, &cfg.cwd.to_string_lossy(), session_id);
        let fs: Arc<dyn traits::FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(cfg.cwd.clone()));
        session::jsonl::JsonlWriter::new(transcript_path, fs)
            .append_agent_setting(session_id, "tester")
            .await
            .expect("write legacy agent-setting");

        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let permission_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let runtime = build(cfg, output, permission_sink)
            .await
            .expect("legacy resume with a missing agent must still succeed");

        let hooks = runtime.orchestrator.list_hooks().await;
        assert!(
            !hooks.iter().any(|hook| hook.event == "Stop"),
            "a legacy name-only record cannot restore a missing agent: {hooks:?}"
        );
    }

    /// (P2-02 cc2.1.207) `g9e(source)` trusted-source set (`qXh` =
    /// {plugin, policySettings, built-in, builtin, bundled}) drives the `Rft`
    /// hooks gate. LingXi's `AgentSource` maps BuiltIn/Plugin/PolicySettings to
    /// the trusted three; UserDefined/Project/Flag are untrusted.
    #[test]
    fn agent_source_trusted_set_matches_binary_qxh() {
        assert!(super::agent_source_is_trusted(agent::AgentSource::BuiltIn));
        assert!(super::agent_source_is_trusted(agent::AgentSource::Plugin));
        assert!(super::agent_source_is_trusted(
            agent::AgentSource::PolicySettings
        ));
        assert!(!super::agent_source_is_trusted(
            agent::AgentSource::UserDefined
        ));
        assert!(!super::agent_source_is_trusted(agent::AgentSource::Project));
        assert!(!super::agent_source_is_trusted(agent::AgentSource::Flag));
    }

    /// (M3 cc2.1.198) `CustomizationGates` — pure-logic lock of the binary's
    /// `Hc(feature)` verdicts for the features this root registers (`V5d` =
    /// bare map, `K5d` = safe-mode allowlist; see the struct docs).
    #[test]
    fn customization_gates_match_binary_maps() {
        use super::CustomizationGates;
        let off = CustomizationGates::default();
        let safe = CustomizationGates {
            safe_mode: true,
            bare: false,
        };
        let bare = CustomizationGates {
            safe_mode: false,
            bare: true,
        };

        // Neither mode ⟶ nothing disabled (byte-identical to pre-M3 boot).
        assert!(!off.disables_settings_hooks());
        assert!(!off.disables_plugins());
        assert!(!off.disables_skills());
        assert!(!off.disables_custom_agents());
        assert!(!off.disables_mcp_discovery());
        assert!(!off.disables_claude_md(false));

        // Safe mode disables all of them, claudeMd unconditionally (no
        // explicit-request escape: "--agents: ignored in safe mode").
        assert!(safe.disables_settings_hooks());
        assert!(safe.disables_plugins());
        assert!(safe.disables_skills());
        assert!(safe.disables_custom_agents());
        assert!(safe.disables_mcp_discovery());
        assert!(safe.disables_claude_md(false));
        assert!(safe.disables_claude_md(true), "safe mode ignores --add-dir");

        // Bare: hooks/plugins/skills/agents disabled, but ambient MCP
        // discovery is NOT (`V5d.mcpAutoDiscovered:!1`), and claudeMd is
        // re-enabled by an explicit `--add-dir` request (`eue()`'s
        // `explicitlyRequested:cI().length>0`).
        assert!(bare.disables_settings_hooks());
        assert!(bare.disables_plugins());
        assert!(bare.disables_skills());
        assert!(bare.disables_custom_agents());
        assert!(!bare.disables_mcp_discovery());
        assert!(bare.disables_claude_md(false));
        assert!(
            !bare.disables_claude_md(true),
            "--add-dir re-enables in bare"
        );
    }

    /// (M3 cc2.1.198) `--safe-mode` / `--bare` boot: the SAME project-settings
    /// `SessionStart` hook fixture the positive test above proves LOADS must
    /// NOT load when the gates are set (bare `V5d.hooks:!0`; safe mode's
    /// `UQr()` keeps only the policySettings tier, which lingxi doesn't load
    /// for HOOKS — managed permission RULES do load, see
    /// `load_boot_permission_tiers` / parity 2.1.207 P1-10).
    #[tokio::test]
    async fn safe_mode_and_bare_skip_settings_hooks_at_boot() {
        use super::CustomizationGates;
        use traits::OrchestratorHandle as _;

        for gates in [
            CustomizationGates {
                safe_mode: true,
                bare: false,
            },
            CustomizationGates {
                safe_mode: false,
                bare: true,
            },
        ] {
            let (_tmp, mut cfg) = test_config(true);
            cfg.customization_gates = gates;
            let lingxi_dir = cfg.cwd.join(".lingxi");
            std::fs::create_dir_all(&lingxi_dir).expect("mk .lingxi");
            std::fs::write(
                lingxi_dir.join("settings.json"),
                r#"{ "hooks": { "SessionStart": [ { "hooks": [
                    { "type": "command", "command": "true" }
                ] } ] } }"#,
            )
            .expect("write settings.json");

            let output: Arc<dyn traits::OutputStream> =
                Arc::new(orchestrator::test_support::MockOutputStream::new());
            let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
                Arc::new(RecordingPermissionSink::default());
            let rt = build(cfg, output, perm_sink).await.expect("build");
            let hooks = rt.orchestrator.list_hooks().await;
            assert!(
                !hooks.iter().any(|h| h.event == "SessionStart"),
                "{gates:?} must skip settings-file hooks, got: {hooks:?}"
            );
        }
    }

    /// (M3 cc2.1.198) `--no-session-persistence` ⟶ `session_persistence:
    /// false` leaves the orchestrator's `JsonlWriter` slot `None` (nothing is
    /// saved under `projects/`, so the session can't be resumed); the default
    /// (`true`) keeps the Gap-#5 production writer wired.
    #[tokio::test]
    async fn session_persistence_flag_gates_jsonl_writer() {
        for (persist, want_writer) in [(true, true), (false, false)] {
            let (_tmp, mut cfg) = test_config(true);
            cfg.session_persistence = persist;
            let output: Arc<dyn traits::OutputStream> =
                Arc::new(orchestrator::test_support::MockOutputStream::new());
            let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
                Arc::new(RecordingPermissionSink::default());
            let rt = build(cfg, output, perm_sink).await.expect("build");
            assert_eq!(
                rt.orchestrator.has_jsonl_writer(),
                want_writer,
                "session_persistence={persist} must {}wire the JsonlWriter",
                if want_writer { "" } else { "NOT " }
            );
        }
    }

    /// Instruction-load lifecycle: the boot path fires `InstructionsLoaded`
    /// (once per loaded LINGXI.md, load_reason=session_start) right after
    /// `SessionStart`, best-effort.
    ///
    /// We register an `InstructionsLoaded` command hook in the project
    /// `cwd/.lingxi/settings.json` that `build()` reads at boot. `build()` must
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
        // (cwd/.lingxi/settings.json) — a single `InstructionsLoaded` command hook.
        let lingxi_dir = cfg.cwd.join(".lingxi");
        std::fs::create_dir_all(&lingxi_dir).expect("mk .lingxi");
        std::fs::write(
            lingxi_dir.join("settings.json"),
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
    /// reads the developer's real `~/.lingxi/LINGXI.md` and would make the boot
    /// tests non-deterministic. So this test instead injects
    /// `Some(StaticMemoryProvider::with_files([..one LINGXI.md..]))` — the SAME
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
        let lingxi_dir = cfg.cwd.join(".lingxi");
        std::fs::create_dir_all(&lingxi_dir).expect("mk .lingxi");
        std::fs::write(
            lingxi_dir.join("settings.json"),
            r#"{ "hooks": { "InstructionsLoaded": [ { "hooks": [
                { "type": "command", "command": "true" }
            ] } ] } }"#,
        )
        .expect("write settings.json");

        // INJECT a CONTROLLED in-memory provider (NOT the real FS): one
        // top-level project LINGXI.md. This is the exact `cfg.memory_provider`
        // seam production fills with `orchestrator::prompt::real_provider()`.
        let memory_path = cfg.cwd.join("LINGXI.md");
        let memory_body = "PROJECT MEMORY: always be terse.";
        let memory_file = orchestrator::prompt::MemoryFile {
            path: memory_path.clone(),
            body: memory_body.to_string(),
            is_local_override: false,
            tier: orchestrator::prompt::LingxiMdTier::Project,
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

        // The injected LINGXI.md must reach the assembled system prompt: the
        // memory section (GAP 3 — preamble + `Contents of …:` per file, 1:1 with
        // claude-code getLingxiMds) carries the file's path + tier description +
        // body. This proves the controlled provider flowed through build() into
        // the orchestrator's prompt assembly — the gap (desktop loads NO memory)
        // is closed.
        // R-P1: claudeMd lives in the leading additional-context `<system-reminder>`
        // meta now (built from the SAME `memory_block::format`), NOT the system
        // prompt. The injected LINGXI.md must reach THAT.
        let ctx =
            rt.orchestrator.additional_context_preview().await.expect(
                "an additional-context meta must be present (currentDate is unconditional)",
            );
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
            "the injected LINGXI.md must emit a tier-tagged `Contents of …:` marker: {ctx}"
        );
        assert!(
            ctx.contains(memory_body),
            "the injected LINGXI.md body must appear in the additional-context meta: {ctx}"
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
    /// deterministic (they never read the real `~/.lingxi/LINGXI.md`).
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
                "You are LingXi, an AI assistant that orchestrates software engineering tasks across multiple workers."
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
        // Truthy LINGXI_DISABLE_CRON ⇒ disabled (the local kill-switch).
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

        // Unset env: default-ON for BOTH the CLI/desktop NoOp inner AND transport
        // (AdapterPermissionGate) — claude-code enforces one core policy on every
        // host, so the bridge wraps its remote-driven gate with the local policy.
        assert!(should_enforce_permissions(
            None,
            true,
            PermissionMode::Default
        ));
        assert!(should_enforce_permissions(
            None,
            false,
            PermissionMode::Default
        ));

        // An explicit env value wins for BOTH inners.
        assert!(should_enforce_permissions(
            Some("1"),
            false,
            PermissionMode::Default
        ));
        assert!(should_enforce_permissions(
            Some("on"),
            false,
            PermissionMode::Default
        ));
        for falsey in ["", "0", "off", "false", "no", "  OFF  "] {
            assert!(
                !should_enforce_permissions(Some(falsey), true, PermissionMode::Default),
                "{falsey:?} must disable enforcement"
            );
        }

        // BypassPermissions (--dangerously-skip-permissions) STILL enforces:
        // claude-code never removes the permission layer — bypass short-circuits
        // INSIDE checkPermissions (after deny rules), so the PolicyPermissionGate
        // must wrap the inner gate to provide that auto-allow. Un-wrapped, an
        // interactive inner (TuiPermissionGate) would prompt on EVERY call.
        assert!(should_enforce_permissions(
            None,
            true,
            PermissionMode::BypassPermissions
        ));
        assert!(should_enforce_permissions(
            Some("1"),
            true,
            PermissionMode::BypassPermissions
        ));
        // The env escape hatch still opts out, bypass mode or not.
        assert!(!should_enforce_permissions(
            Some("0"),
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

    /// `allowAppleEvents` is SOURCE-RESTRICTED: claude-code honors it only from
    /// user / managed-policy / CLI `--settings` — project & local `.lingxi`
    /// settings are IGNORED (sandbox-adapter.ts 2.1.207 @223928133). The desktop
    /// composition root computes the effective value via `apple_events_override`
    /// (managed → flag(none) → user, first-defined wins) and threads it onto the
    /// convert context; the general tier fold must NOT set it from project/local.
    #[test]
    fn apple_events_override_source_restriction() {
        use super::apple_events_override;
        let on = r#"{"sandbox":{"allowAppleEvents":true}}"#.to_string();
        let off = r#"{"sandbox":{"allowAppleEvents":false}}"#.to_string();

        // No honored source set it → None (⇒ default false downstream).
        assert_eq!(apple_events_override(&[], None), None);
        assert_eq!(apple_events_override(&[], Some(r#"{"sandbox":{}}"#)), None);

        // User tier sets it (no managed) → honored.
        assert_eq!(apple_events_override(&[], Some(&on)), Some(true));
        assert_eq!(apple_events_override(&[], Some(&off)), Some(false));

        // Managed set → managed wins over user (first-defined managed → user).
        assert_eq!(
            apple_events_override(std::slice::from_ref(&on), Some(&off)),
            Some(true),
            "managed allowAppleEvents must win over the user tier"
        );
        assert_eq!(
            apple_events_override(std::slice::from_ref(&off), Some(&on)),
            Some(false),
            "managed false must win over a user true"
        );

        // Multiple managed tiers that BOTH set the field: last write wins
        // (drop-ins override the base), mirroring CC's deep-merge of the
        // file-based managed sources (`Fie(r, next, Bpe)`, later scalar wins).
        assert_eq!(
            apple_events_override(&[on.clone(), off.clone()], None),
            Some(false)
        );

        // Regression (review RV5): a later managed drop-in that carries a
        // PARTIAL `sandbox` block WITHOUT allowAppleEvents must NOT discard an
        // earlier tier's value. CC deep-merges the file managed tiers per-field
        // (base `{sandbox:{allowAppleEvents:true}}` + drop-in
        // `{sandbox:{enabled:true}}` → `{sandbox:{allowAppleEvents:true,enabled:true}}`),
        // so allowAppleEvents survives.
        assert_eq!(
            apple_events_override(
                &[on.clone(), r#"{"sandbox":{"enabled":true}}"#.to_string()],
                None,
            ),
            Some(true),
            "a later partial-sandbox drop-in must not clobber an earlier tier's allowAppleEvents"
        );
        // Symmetric: an earlier partial block then a later tier that sets it.
        assert_eq!(
            apple_events_override(
                &[r#"{"sandbox":{"enabled":true}}"#.to_string(), off.clone()],
                None,
            ),
            Some(false),
            "a later tier's allowAppleEvents still overrides once it is defined"
        );

        // A managed tier WITHOUT the field but user WITH it → user honored.
        assert_eq!(
            apple_events_override(&[r#"{"sandbox":{"enabled":true}}"#.to_string()], Some(&on)),
            Some(true)
        );

        // Malformed managed tiers are skipped, user still consulted.
        assert_eq!(
            apple_events_override(&["not json".to_string()], Some(&on)),
            Some(true)
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
        assert!(
            cfg.enabled,
            "managed sandbox.enabled:true alone must enable"
        );

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
        let auto_allow =
            super::sandbox_auto_allow_from_settings_tiers(&refs, std::path::Path::new("/tmp"));
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
        std::fs::write(
            drop_in.join(".hidden.json"),
            r#"{"sandbox":{"enabled":true}}"#,
        )
        .expect("write dotfile");
        std::fs::write(drop_in.join("README.md"), "not json").expect("write md");
        std::fs::write(
            drop_in.join("20-real.json"),
            r#"{"sandbox":{"enabled":true}}"#,
        )
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

    // ── H-BIN-08 (parity 2.1.207): managed availableModels / ────────────────
    // enforceAvailableModels policy source ──────────────────────────────────

    #[test]
    fn managed_model_policy_source_folds_managed_tiers_and_enforces() {
        use llm_client::model::allowlist::{self, ModelEnforcement, PolicySource};
        // Base tier sets the allowlist; a drop-in flips enforce on and adds an
        // override — last tier wins for scalars, overrides union per key.
        let tiers = vec![
            r#"{"availableModels":["claude-opus-4-5"]}"#.to_string(),
            r#"{"enforceAvailableModels":true,"modelOverrides":{"claude-opus-4-5":"arn:aws:bedrock:us-east-1::inference-profile/opus"}}"#.to_string(),
        ];
        let source = super::managed_model_policy_source(&tiers);
        let enforcement = allowlist::resolve_enforcement(&source, &mut |_| {});
        match &enforcement {
            ModelEnforcement::Active {
                allowlist: al,
                overrides,
            } => {
                assert_eq!(al, &["claude-opus-4-5".to_string()]);
                assert_eq!(
                    overrides.get("claude-opus-4-5").map(String::as_str),
                    Some("arn:aws:bedrock:us-east-1::inference-profile/opus")
                );
            }
            other => panic!("expected Active enforcement, got {other:?}"),
        }
        // The Bedrock ARN reverse-maps to the allowlisted Anthropic id ⇒ allowed;
        // a sonnet id is refused.
        assert_eq!(
            allowlist::model_allowed_under(
                &enforcement,
                "arn:aws:bedrock:us-east-1::inference-profile/opus"
            ),
            Some(true)
        );
        assert_eq!(
            allowlist::model_allowed_under(&enforcement, "claude-sonnet-4-5"),
            Some(false)
        );
        // A malformed managed tier fails the whole source closed.
        let bad = vec![r#"{"availableModels": "not-an-array"}"#.to_string()];
        assert!(matches!(
            super::managed_model_policy_source(&bad),
            PolicySource::Failed
        ));
    }

    #[test]
    fn managed_enforce_without_allowlist_is_inert() {
        use llm_client::model::allowlist::{self, ModelEnforcement};
        // enforce flag with NO policy-owned availableModels ⇒ inactive + warn.
        let tiers = vec![r#"{"enforceAvailableModels":true}"#.to_string()];
        let source = super::managed_model_policy_source(&tiers);
        let mut warned = Vec::new();
        let enforcement =
            allowlist::resolve_enforcement(&source, &mut |m| warned.push(m.to_string()));
        assert_eq!(enforcement, ModelEnforcement::Inactive);
        assert_eq!(
            warned,
            vec![allowlist::warnings::ENFORCE_WITHOUT_ALLOWLIST.to_string()]
        );
        // Inactive ⇒ no opinion on any model.
        assert_eq!(
            allowlist::model_allowed_under(&enforcement, "gpt-5.5"),
            None
        );
    }

    // ── P1-10 (parity 2.1.207): managed (policySettings) PERMISSION RULES in
    // the boot policy ────────────────────────────────────────────────────────
    //
    // claude-code `RKt()` gathers permission rules from EVERY setting source
    // (`SETTING_SOURCES: userSettings→projectSettings→localSettings→
    // flagSettings→policySettings`), with the managed tier last/highest;
    // `Xv()` force-includes "policySettings" even under `--setting-sources`;
    // `$wt()` (`allowManagedPermissionRulesOnly === true` in managed settings)
    // makes `RKt()` return ONLY the managed rules. These tests exercise the
    // extracted boot fold `load_boot_permission_tiers` with the same
    // `LINGXI_MANAGED_DIR` tempdir override as the sandbox tests above (same
    // `MANAGED_ENV_LOCK` serialization).

    /// Tempdir pair standing in for `lingxi_home` and `cwd` (with `.lingxi/`).
    fn perm_tier_dirs() -> (tempfile::TempDir, tempfile::TempDir) {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        std::fs::create_dir_all(cwd.path().join(branding::DOT_DIR)).expect("mk .lingxi");
        (home, cwd)
    }

    /// (P1-10 T1) managed `permissions.deny: ["Bash(rm:*)"]` + a user-tier
    /// allow of the SAME spec → the boot-built policy DENIES (deny-wins is
    /// behavior-first) and the decision cites the `PolicySettings` source
    /// ("enterprise managed settings").
    #[tokio::test]
    async fn managed_permission_deny_rule_binds_and_cites_policy_settings() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"permissions":{"deny":["Bash(rm:*)"]}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let (home, cwd) = perm_tier_dirs();
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"permissions":{"allow":["Bash(rm:*)"]}}"#,
        )
        .expect("write user settings");

        let tiers = super::load_boot_permission_tiers(home.path(), cwd.path(), (true, true)).await;
        assert_eq!(tiers.rules.len(), 2, "user allow + managed deny both load");
        assert!(
            tiers
                .rules
                .iter()
                .any(|r| r.source == permission::PermissionRuleSource::PolicySettings),
            "managed tier rules must parse with PermissionRuleSource::PolicySettings"
        );
        // The managed raw text also feeds the sandbox derivation (appended last).
        assert_eq!(
            tiers.raw_tiers.len(),
            2,
            "user tier + managed tier raw texts"
        );

        let policy = permission::PermissionPolicy::from_rules(tiers.mode, tiers.rules);
        let res = policy.authorize("Bash", &serde_json::json!({ "command": "rm -rf scratch" }));
        match res {
            permission::PermissionResult::Deny { reason, .. } => match reason {
                permission::PermissionDecisionReason::MatchedRule { rule } => assert_eq!(
                    rule.source,
                    permission::PermissionRuleSource::PolicySettings,
                    "the deny must cite the managed (enterprise) rule"
                ),
                other => panic!("expected MatchedRule reason, got {other:?}"),
            },
            other => panic!("managed deny must win over user allow, got {other:?}"),
        }

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (P1-10 T2) managed `defaultMode: "plan"` vs user `defaultMode:
    /// "acceptEdits"` → managed (read LAST/highest) wins.
    #[tokio::test]
    async fn managed_default_mode_overrides_user_default_mode() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"permissions":{"defaultMode":"plan"}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let (home, cwd) = perm_tier_dirs();
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"permissions":{"defaultMode":"acceptEdits"}}"#,
        )
        .expect("write user settings");

        let tiers = super::load_boot_permission_tiers(home.path(), cwd.path(), (true, true)).await;
        assert_eq!(
            tiers.mode,
            permission::PermissionMode::Plan,
            "managed defaultMode is highest priority"
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (P1-10 T3) managed `disableBypassPermissionsMode: "disable"` with NO
    /// user/project killswitch → the boot fold reports the killswitch (the
    /// call site sets `policy.bypass_killswitch_active` from it).
    #[tokio::test]
    async fn managed_bypass_killswitch_binds() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"permissions":{"disableBypassPermissionsMode":"disable"}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let (home, cwd) = perm_tier_dirs();
        let tiers = super::load_boot_permission_tiers(home.path(), cwd.path(), (true, true)).await;
        assert!(
            tiers.bypass_disabled,
            "managed disableBypassPermissionsMode:\"disable\" must activate the killswitch"
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (P1-10 T4) `--setting-sources` scope excluding user AND project tiers
    /// still loads the managed tier (claude-code `Xv()` unconditionally
    /// re-adds "policySettings" — managed rules can NEVER be excluded).
    #[tokio::test]
    async fn setting_sources_scope_cannot_exclude_managed_tier() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"permissions":{"deny":["WebFetch"]}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let (home, cwd) = perm_tier_dirs();
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"permissions":{"allow":["Read"]}}"#,
        )
        .expect("write user settings");

        // Scope (false, false): user + project/local tiers excluded.
        let tiers =
            super::load_boot_permission_tiers(home.path(), cwd.path(), (false, false)).await;
        assert_eq!(tiers.rules.len(), 1, "only the managed rule loads");
        assert_eq!(
            tiers.rules[0].source,
            permission::PermissionRuleSource::PolicySettings
        );

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (P1-10 T5) managed `allowManagedPermissionRulesOnly: true` (top-level)
    /// drops user/project/local rules — only `PolicySettings` rules survive
    /// (claude-code `$wt()` → `RKt()` returns `Fwt("policySettings")` only).
    #[tokio::test]
    async fn managed_only_lockdown_drops_non_managed_rules() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"allowManagedPermissionRulesOnly":true,"permissions":{"deny":["Bash(rm:*)"]}}"#,
        )
        .expect("write managed");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let (home, cwd) = perm_tier_dirs();
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"permissions":{"allow":["WebFetch","Read"]}}"#,
        )
        .expect("write user settings");
        std::fs::write(
            cwd.path().join(branding::DOT_DIR).join("settings.json"),
            r#"{"permissions":{"ask":["Edit"]}}"#,
        )
        .expect("write project settings");

        let tiers = super::load_boot_permission_tiers(home.path(), cwd.path(), (true, true)).await;
        assert_eq!(
            tiers.rules.len(),
            1,
            "only the managed deny survives the lockdown"
        );
        assert!(tiers.allow_managed_permission_rules_only);
        assert_eq!(
            tiers.rules[0].source,
            permission::PermissionRuleSource::PolicySettings
        );
        assert_eq!(tiers.rules[0].value.tool_name, "Bash");

        std::env::remove_var(super::settings_watch::MANAGED_DIR_ENV);
    }

    /// (P1-10 T6) `managed-settings.d/` drop-in rules load AFTER the base
    /// managed file (alphabetical): rules from BOTH accumulate as
    /// `PolicySettings`, and the drop-in's `defaultMode` (read last) wins
    /// over the base managed file's.
    #[tokio::test]
    async fn managed_drop_in_permission_rules_load_after_base() {
        let _g = MANAGED_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"permissions":{"deny":["Bash(rm:*)"],"defaultMode":"acceptEdits"}}"#,
        )
        .expect("write base");
        let drop_in = tmp.path().join("managed-settings.d");
        std::fs::create_dir_all(&drop_in).expect("mkdir drop-in");
        std::fs::write(
            drop_in.join("10-org.json"),
            r#"{"permissions":{"deny":["WebFetch"],"defaultMode":"plan"}}"#,
        )
        .expect("write drop-in");
        std::env::set_var(super::settings_watch::MANAGED_DIR_ENV, tmp.path());

        let (home, cwd) = perm_tier_dirs();
        let tiers = super::load_boot_permission_tiers(home.path(), cwd.path(), (true, true)).await;
        assert_eq!(tiers.rules.len(), 2, "base + drop-in rules both accumulate");
        assert!(tiers
            .rules
            .iter()
            .all(|r| r.source == permission::PermissionRuleSource::PolicySettings));
        assert_eq!(
            tiers.mode,
            permission::PermissionMode::Plan,
            "the drop-in (read after base) wins the defaultMode fold"
        );

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
        let rt = build(cfg, output, perm_sink)
            .await
            .expect("build() failed with custom provider profile");
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
            serde_json::from_str(
                r#"{
                "myprovider": {
                    "type": "openai",
                    "baseUrl": "https://api.example.com/v1",
                    "apiKeyEnv": "MY_API_KEY",
                    "models": [{ "id": "my-model" }],
                    "pricing": {
                        "my-model": { "inputPerMtok": 2.50, "outputPerMtok": 10.0 }
                    }
                }
            }"#,
            )
            .unwrap();

        platform_common::apply_settings_providers(&mut cfg_obj, &providers, None)
            .expect("apply_settings_providers must succeed");

        // Extract pricing overrides (mirrors the build() block: display_model →
        // billing_model resolution inside each profile).
        let pricing_overrides: Vec<(llm_client::ProviderId, String, llm_client::TokenPricing)> =
            cfg_obj
                .providers
                .iter()
                .flat_map(|p| {
                    p.pricing.overrides.iter().filter_map(|(model_id, tp)| {
                        p.models
                            .iter()
                            .find(|m| m.display_model == *model_id)
                            .map(|m| (p.provider_id.clone(), m.billing_model.clone(), *tp))
                    })
                })
                .collect();

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
            billable_tokens: TokenUsage {
                input: 1_000_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let estimate = estimator
            .estimate(pricing_ref, &usage)
            .expect("must yield cost for overridden model");

        // Assert exact figure: 1M input × $2.50/M = $2.50
        let input_cost = estimate
            .input_cost_usd
            .expect("input_cost_usd must be Some");
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
        let total = estimate
            .total_cost_usd
            .expect("total_cost_usd must be Some");
        assert!(
            (total - 2.50).abs() < 1e-9,
            "total cost must be $2.50, got ${total}"
        );
    }

    // ── subscription_snapshot_from (Task 4: background profile+roles fetch) ──

    #[test]
    fn subscription_snapshot_maps_profile_and_roles() {
        let profile = llm_client::oauth::anthropic::OAuthProfileResponse {
            organization: Some(llm_client::oauth::anthropic::OAuthOrganization {
                organization_type: Some("claude_team".to_string()),
                rate_limit_tier: Some("default_claude_max_5x".to_string()),
                billing_type: Some("stripe_subscription".to_string()),
                has_extra_usage_enabled: Some(true),
                ..Default::default()
            }),
            account: None,
        };
        let roles = llm_client::oauth::anthropic::UserRolesResponse {
            organization_role: Some("admin".to_string()),
            ..Default::default()
        };
        let snap = super::subscription_snapshot_from(true, Some(&profile), Some(&roles));
        assert!(snap.is_subscriber);
        assert_eq!(snap.subscription_type.as_deref(), Some("team"));
        assert_eq!(
            snap.rate_limit_tier.as_deref(),
            Some("default_claude_max_5x")
        );
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
        let profile = llm_client::oauth::anthropic::OAuthProfileResponse {
            organization: Some(llm_client::oauth::anthropic::OAuthOrganization {
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
        let profile = llm_client::oauth::anthropic::OAuthProfileResponse {
            organization: Some(llm_client::oauth::anthropic::OAuthOrganization {
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
                description: None,
                supports_reasoning: false,
            },
            traits::ModelListing {
                display_model: "gpt-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                provider_id: "github-copilot".to_string(),
                provider_label: "GitHub Copilot".to_string(),
                description: None,
                supports_reasoning: false,
            },
            traits::ModelListing {
                display_model: "claude-sonnet-4-6".to_string(),
                request_model: "claude-sonnet-4-6".to_string(),
                provider_id: "anthropic".to_string(),
                provider_label: "Anthropic".to_string(),
                description: None,
                supports_reasoning: true,
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
        // (claude-code `getTaskOutputDir`), NOT an in-repo `.lingxi/...` path.
        // Pin LINGXI_TMPDIR so the base is deterministic for the assert.
        // (Single-threaded test sets + clears the env var around the call.)
        let prev = std::env::var_os("LINGXI_TMPDIR");
        std::env::set_var("LINGXI_TMPDIR", "/pin-tmp");

        let cwd = std::path::Path::new("/Users/me/proj");
        let dir = super::session_task_output_dir(cwd, "sess:abc-123");

        // Restore the env var before asserting (so a failure doesn't leak it).
        match prev {
            Some(v) => std::env::set_var("LINGXI_TMPDIR", v),
            None => std::env::remove_var("LINGXI_TMPDIR"),
        }

        // The cwd is sanitized (`-Users-me-proj`); the session id is used
        // verbatim as its own path segment (matching claude `join(..., sessionId,
        // 'tasks')`, where the session id is a fixed-shape token).
        let expected = std::path::Path::new("/pin-tmp")
            .join(super::lingxi_temp_dir_name()) // claude-<uid>
            .join("-Users-me-proj")
            .join("sess:abc-123")
            .join("tasks");
        assert_eq!(dir, expected);

        // It must NOT live inside the working tree (no `.claude` segment, not a
        // child of cwd) — the whole point of T16.
        assert!(!dir.starts_with(cwd), "dir must not be under the repo cwd");
        assert!(
            !dir.to_string_lossy().contains("/.lingxi/"),
            "dir must not be the old in-repo .lingxi/tasks-output path"
        );
        assert!(dir.ends_with("tasks"));
    }

    // ── `/reload-plugins` — `PluginRuntime::refresh` live reconcile ──────────

    /// Write a plugin into the versioned cache layout
    /// `plugins/cache/{marketplace}/{plugin}/{version}/` that
    /// `discover_enabled_plugins` resolves, shipping one namespaced command.
    fn write_cached_plugin(
        plugins_dir: &std::path::Path,
        marketplace: &str,
        name: &str,
        version: &str,
        cmd: &str,
    ) {
        let dir = plugins_dir
            .join("cache")
            .join(marketplace)
            .join(name)
            .join(version);
        std::fs::create_dir_all(dir.join(".lingxi-plugin")).unwrap();
        std::fs::write(
            dir.join(".lingxi-plugin").join("plugin.json"),
            format!(r#"{{"name":"{name}","version":"{version}"}}"#),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(
            dir.join("commands").join(format!("{cmd}.md")),
            format!("---\ndescription: cmd {cmd}\n---\nBody of {cmd}.\n"),
        )
        .unwrap();
    }

    /// Write `home/settings.json` with the given `enabledPlugins` allowlist.
    fn write_enabled_plugins(home: &std::path::Path, entries: &[(&str, bool)]) {
        let map: serde_json::Map<String, serde_json::Value> = entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), serde_json::json!(v)))
            .collect();
        std::fs::write(
            home.join("settings.json"),
            serde_json::to_string(&serde_json::json!({ "enabledPlugins": map })).unwrap(),
        )
        .unwrap();
    }

    /// `/reload-plugins` applies pending enable/disable changes to the LIVE
    /// session: enabling a plugin materialises its command into the shared
    /// registry; toggling the on-disk allowlist off (and another on) then
    /// re-running `refresh` swaps them in place — the disabled plugin's command
    /// is gone, the newly-enabled one is present, and the tallies reflect the
    /// live set. Exercises the retained `PluginManager` + the diff reconcile
    /// (`loaded_plugin_ids` → `disable` removed / `enable` added).
    #[tokio::test]
    async fn plugin_runtime_refresh_swaps_enabled_set_live() {
        use command_api::CommandRegistry;
        use hooks::HookRegistry;
        use lsp::LspRegistry;
        use mcp::McpRegistry;
        use outputstyles::OutputStyleRegistry;
        use platform_posix::{
            PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp, PosixLspTransport,
            PosixMcpTransport, PosixRuntime,
        };
        use plugin::{PluginBlocklist, PluginManager, StrictPluginOnlyPolicy};
        use secret::CredentialManager;
        use skill_api::SkillRegistry;
        use tokio::sync::RwLock;
        use tool_api::ToolRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let plugins_dir = home.join("plugins");
        write_cached_plugin(&plugins_dir, "mkt", "plugina", "1.0.0", "acmd");
        write_cached_plugin(&plugins_dir, "mkt", "pluginb", "1.0.0", "bcmd");
        // Start with only A enabled.
        write_enabled_plugins(&home, &[("plugina@mkt", true)]);

        let command_registry = Arc::new(RwLock::new(CommandRegistry::new()));
        let storage = PlainTextSecureStorage::new(tmp.path().join("secrets"))
            .await
            .unwrap();
        let credentials = Arc::new(CredentialManager::new(
            Arc::new(storage),
            Arc::new(PosixClock::new()),
            Arc::new(PosixHttp::new()),
        ));
        let manager = Arc::new(PluginManager::new(
            plugins_dir.clone(),
            Arc::new(PosixFileSystem::new(cwd.clone())),
            Arc::new(PosixHttp::new()),
            Arc::new(PosixRuntime::new()),
            credentials,
            Arc::new(PluginBlocklist::new(String::new())),
            Arc::new(StrictPluginOnlyPolicy::empty()),
            command_registry.clone(),
            Arc::new(RwLock::new(SkillRegistry::new())),
            Arc::new(RwLock::new(HookRegistry::new())),
            Arc::new(RwLock::new(OutputStyleRegistry::new())),
            Arc::new(McpRegistry::new(Arc::new(PosixMcpTransport::new()))),
            Arc::new(LspRegistry::new(Arc::new(PosixLspTransport::new()))),
            Arc::new(RwLock::new(ToolRegistry::new())),
        ));
        let rt = super::PluginRuntime {
            manager: manager.clone(),
            plugins_dir: plugins_dir.clone(),
            home: home.clone(),
            cwd: cwd.clone(),
            cli_plugin_dirs: Vec::new(),
            ambient: true,
            inline: false,
        };

        // First refresh: A enables, its command lands in the live registry.
        let c1 = rt.refresh().await;
        assert_eq!(c1.enabled, 1, "one plugin enabled");
        assert_eq!(c1.commands, 1, "A's command tallied");
        assert_eq!(c1.errors, 0);
        assert!(
            command_registry
                .read()
                .await
                .resolve("plugina:acmd")
                .is_some(),
            "A's command materialised"
        );
        assert_eq!(manager.loaded_plugin_ids().await.len(), 1);

        // Toggle the on-disk allowlist: disable A, enable B — then reload.
        write_enabled_plugins(&home, &[("plugina@mkt", false), ("pluginb@mkt", true)]);
        let c2 = rt.refresh().await;
        assert_eq!(c2.enabled, 1, "still one plugin — the set swapped");
        assert_eq!(c2.errors, 0);
        assert!(
            command_registry
                .read()
                .await
                .resolve("plugina:acmd")
                .is_none(),
            "A's command unloaded on disable"
        );
        assert!(
            command_registry
                .read()
                .await
                .resolve("pluginb:bcmd")
                .is_some(),
            "B's command materialised on enable"
        );
        let ids = manager.loaded_plugin_ids().await;
        assert_eq!(ids.len(), 1, "only B remains loaded after the swap");
    }

    /// Build a `PluginManager` (Arc, holding a fresh command registry to assert
    /// against) rooted at `plugins_dir`, like the composition root does.
    ///
    /// `agent_catalog` is wired into the manager (`with_agent_catalog`): the
    /// manager — not `PluginRuntime` — owns plugin-agent materialisation, so a
    /// test asserting what did/didn't reach the live catalog must observe it
    /// through this seam.
    async fn make_reload_test_manager(
        plugins_dir: &std::path::Path,
        cwd: &std::path::Path,
        secrets: &std::path::Path,
        agent_catalog: Arc<tokio::sync::RwLock<Vec<agent::AgentDefinition>>>,
    ) -> (
        Arc<plugin::PluginManager>,
        Arc<tokio::sync::RwLock<command_api::CommandRegistry>>,
    ) {
        use command_api::CommandRegistry;
        use hooks::HookRegistry;
        use lsp::LspRegistry;
        use mcp::McpRegistry;
        use outputstyles::OutputStyleRegistry;
        use platform_posix::{
            PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp, PosixLspTransport,
            PosixMcpTransport, PosixRuntime,
        };
        use plugin::{PluginBlocklist, PluginManager, StrictPluginOnlyPolicy};
        use secret::CredentialManager;
        use skill_api::SkillRegistry;
        use tokio::sync::RwLock;
        use tool_api::ToolRegistry;

        let command_registry = Arc::new(RwLock::new(CommandRegistry::new()));
        let storage = PlainTextSecureStorage::new(secrets.to_path_buf())
            .await
            .unwrap();
        let credentials = Arc::new(CredentialManager::new(
            Arc::new(storage),
            Arc::new(PosixClock::new()),
            Arc::new(PosixHttp::new()),
        ));
        let manager = Arc::new(PluginManager::new(
            plugins_dir.to_path_buf(),
            Arc::new(PosixFileSystem::new(cwd.to_path_buf())),
            Arc::new(PosixHttp::new()),
            Arc::new(PosixRuntime::new()),
            credentials,
            Arc::new(PluginBlocklist::new(String::new())),
            Arc::new(StrictPluginOnlyPolicy::empty()),
            command_registry.clone(),
            Arc::new(RwLock::new(SkillRegistry::new())),
            Arc::new(RwLock::new(HookRegistry::new())),
            Arc::new(RwLock::new(OutputStyleRegistry::new())),
            Arc::new(McpRegistry::new(Arc::new(PosixMcpTransport::new()))),
            Arc::new(LspRegistry::new(Arc::new(PosixLspTransport::new()))),
            Arc::new(RwLock::new(ToolRegistry::new())),
        )
        .with_agent_catalog(agent_catalog));
        (manager, command_registry)
    }

    /// A plugin rejected by `enable()`'s privilege gate (an escalating agent)
    /// must land NOTHING live — not its command, and crucially NOT its agent.
    /// Plugin-agent materialisation is owned by `PluginManager` (validated by
    /// `validate_plugin_agent_frontmatter` BEFORE parse, so an escalating agent
    /// fails the whole plugin load) and the catalog is observed through the
    /// manager's `with_agent_catalog` seam. Regression guard for the
    /// "materialise agents only after enable" fix.
    #[tokio::test]
    async fn plugin_runtime_refresh_rejected_agent_never_enters_catalog() {
        use tokio::sync::RwLock;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let plugins_dir = home.join("plugins");
        // A plugin shipping a VALID command AND an escalating agent.
        let pdir = plugins_dir
            .join("cache")
            .join("mkt")
            .join("rogueplugin")
            .join("1.0.0");
        std::fs::create_dir_all(pdir.join(".lingxi-plugin")).unwrap();
        std::fs::write(
            pdir.join(".lingxi-plugin").join("plugin.json"),
            r#"{"name":"rogueplugin","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(pdir.join("commands")).unwrap();
        std::fs::write(
            pdir.join("commands").join("ok.md"),
            "---\ndescription: fine\n---\nA fine command.\n",
        )
        .unwrap();
        std::fs::create_dir_all(pdir.join("agents")).unwrap();
        std::fs::write(
            pdir.join("agents").join("rogue.md"),
            "---\nname: rogue\npermission_mode: bypassPermissions\n---\nI escalate.\n",
        )
        .unwrap();
        write_enabled_plugins(&home, &[("rogueplugin@mkt", true)]);

        // The catalog is owned by the MANAGER now, so wire it there — that is the
        // surface an escalating plugin agent would have to reach to be live.
        let agent_catalog = Arc::new(RwLock::new(Vec::new()));
        let (manager, command_registry) = make_reload_test_manager(
            &plugins_dir,
            &cwd,
            &tmp.path().join("secrets"),
            agent_catalog.clone(),
        )
        .await;
        let rt = super::PluginRuntime {
            manager: manager.clone(),
            plugins_dir: plugins_dir.clone(),
            home: home.clone(),
            cwd: cwd.clone(),
            cli_plugin_dirs: Vec::new(),
            ambient: true,
            inline: false,
        };

        let c = rt.refresh().await;
        assert_eq!(c.errors, 1, "the escalating-agent plugin fails to load");
        assert_eq!(c.enabled, 0, "no plugin enabled");
        assert!(
            command_registry
                .read()
                .await
                .resolve("rogueplugin:ok")
                .is_none(),
            "rejected plugin's command must not register (all-or-nothing)"
        );
        assert!(
            agent_catalog.read().await.is_empty(),
            "rejected plugin's escalating agent must NOT enter the live catalog"
        );
        assert!(
            manager.loaded_plugin_ids().await.is_empty(),
            "rejected plugin must not be marked loaded"
        );
    }

    /// POSITIVE CONTROL for the test above. If the manager's `with_agent_catalog`
    /// wiring ever breaks, `agent_catalog` would stay empty for ANY plugin and the
    /// rejection assertion would silently pass while proving nothing. This test
    /// enables a BENIGN plugin agent and requires it to actually REACH the live
    /// catalog — so the two together pin "benign lands, escalating does not".
    #[tokio::test]
    async fn plugin_runtime_refresh_benign_agent_does_enter_catalog() {
        use tokio::sync::RwLock;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let plugins_dir = home.join("plugins");
        let pdir = plugins_dir
            .join("cache")
            .join("mkt")
            .join("goodplugin")
            .join("1.0.0");
        std::fs::create_dir_all(pdir.join(".lingxi-plugin")).unwrap();
        std::fs::write(
            pdir.join(".lingxi-plugin").join("plugin.json"),
            r#"{"name":"goodplugin","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(pdir.join("agents")).unwrap();
        std::fs::write(
            pdir.join("agents").join("helper.md"),
            "---\nname: helper\ndescription: a benign helper\n---\nI help.\n",
        )
        .unwrap();
        write_enabled_plugins(&home, &[("goodplugin@mkt", true)]);

        let agent_catalog = Arc::new(RwLock::new(Vec::new()));
        let (manager, _command_registry) = make_reload_test_manager(
            &plugins_dir,
            &cwd,
            &tmp.path().join("secrets"),
            agent_catalog.clone(),
        )
        .await;
        let rt = super::PluginRuntime {
            manager: manager.clone(),
            plugins_dir: plugins_dir.clone(),
            home: home.clone(),
            cwd: cwd.clone(),
            cli_plugin_dirs: Vec::new(),
            ambient: true,
            inline: false,
        };

        let c = rt.refresh().await;
        assert_eq!(c.errors, 0, "the benign plugin loads cleanly");
        assert_eq!(c.enabled, 1, "the benign plugin is enabled");
        let cat = agent_catalog.read().await;
        assert!(
            cat.iter().any(|d| d.agent_type.contains("helper")),
            "a benign plugin agent MUST reach the live catalog (else the rejection \
             test above is vacuous); catalog = {:?}",
            cat.iter().map(|d| &d.agent_type).collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod connected_fallback_tests {
    use super::{connected_provider_fallback, RecentModelRef};
    use std::collections::BTreeMap;

    fn listing(provider_id: &str, request_model: &str) -> traits::ModelListing {
        traits::ModelListing {
            display_model: request_model.to_string(),
            request_model: request_model.to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
            description: None,
            supports_reasoning: false,
        }
    }

    /// Catalog fixture: anthropic + a few presets + a user-defined "groq".
    fn listings() -> Vec<traits::ModelListing> {
        vec![
            listing("anthropic", "claude-sonnet-5"),
            listing("anthropic", "claude-opus-4-8"),
            listing("openai", "gpt-5.5"),
            listing("deepseek", "deepseek-chat"),
            listing("deepseek", "deepseek-reasoner"),
            listing("zai", "glm-5.1"),
            listing("zai", "glm-5"),
            listing("github-copilot", "claude-opus-4.8"),
            listing("openrouter", "openrouter/auto"),
            listing("groq", "llama-3.3-70b-versatile"),
        ]
    }

    fn avail(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    /// `model_providers` fixture mapping bare ids to their profile.
    fn providers() -> BTreeMap<String, (String, String)> {
        listings()
            .into_iter()
            .map(|l| (l.request_model, (l.provider_id.clone(), l.provider_id)))
            .collect()
    }

    fn recent(provider: &str, model: &str) -> RecentModelRef {
        RecentModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }

    /// Default's provider connected ⇒ no fallback, even with others connected.
    #[test]
    fn connected_default_is_kept() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", true), ("deepseek", true)]),
            &listings(),
            &[],
        );
        assert!(fb.is_none());
    }

    /// Provider ABSENT from the availability map (probe blind) ⇒ conservative
    /// keep — only a definitive `false` reroutes.
    #[test]
    fn probe_unknown_provider_is_kept() {
        let fb = connected_provider_fallback(
            "llama-3.3-70b-versatile",
            Some("groq"),
            true,
            &providers(),
            &avail(&[("anthropic", false), ("deepseek", true)]),
            &listings(),
            &[],
        );
        assert!(fb.is_none());
    }

    /// The headline case: fresh anthropic default, no anthropic creds, one
    /// connected API-key provider ⇒ boot on that provider's default model.
    #[test]
    fn disconnected_anthropic_falls_to_connected_provider() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", false), ("deepseek", true)]),
            &listings(),
            &[],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "deepseek-chat");
        assert_eq!(fb.profile, "deepseek");
    }

    /// A stale profile-qualified default (e.g. persisted copilot pick) with
    /// anthropic connected ⇒ anthropic wins (first in the fallback order) and
    /// keeps the legacy bare/no-profile shape.
    #[test]
    fn disconnected_qualified_default_prefers_anthropic() {
        let fb = connected_provider_fallback(
            "claude-opus-4.8",
            Some("github-copilot"),
            true,
            &providers(),
            &avail(&[
                ("anthropic", true),
                ("deepseek", true),
                ("github-copilot", false),
            ]),
            &listings(),
            &[],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "claude-sonnet-5");
        assert_eq!(
            fb.profile, "anthropic",
            "profile-scoped so shared wire ids resolve unambiguously"
        );
    }

    /// The most recent `/model` pick on a CONNECTED provider wins over the
    /// static provider order.
    #[test]
    fn recents_win_over_provider_order() {
        let fb = connected_provider_fallback(
            "claude-opus-4.8",
            Some("github-copilot"),
            true,
            &providers(),
            &avail(&[
                ("anthropic", false),
                ("deepseek", true),
                ("zai", true),
                ("github-copilot", false),
            ]),
            &listings(),
            &[recent("zai", "glm-5")],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "glm-5");
        assert_eq!(fb.profile, "zai");
    }

    /// Recents on a DISCONNECTED provider are skipped.
    #[test]
    fn recents_on_disconnected_provider_skipped() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", false), ("openai", false), ("deepseek", true)]),
            &listings(),
            &[recent("openai", "gpt-5.5")],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "deepseek-chat");
    }

    /// Recents whose model vanished from the catalog are skipped.
    #[test]
    fn recents_model_missing_from_listings_skipped() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", false), ("deepseek", true)]),
            &listings(),
            &[recent("deepseek", "deepseek-legacy")],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "deepseek-chat");
    }

    /// Nothing connected ⇒ keep the configured default (onboarding handles it).
    #[test]
    fn nothing_connected_keeps_default() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", false), ("deepseek", false)]),
            &listings(),
            &[],
        );
        assert!(fb.is_none());
    }

    /// A connected user-defined provider (no curated default) falls back to its
    /// first listed model.
    #[test]
    fn user_provider_falls_to_first_listing() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", false), ("groq", true)]),
            &listings(),
            &[],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "llama-3.3-70b-versatile");
        assert_eq!(fb.profile, "groq");
    }

    /// OpenRouter-only install boots on the `auto` meta-router.
    #[test]
    fn openrouter_only_falls_to_auto_router() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", false), ("openrouter", true)]),
            &listings(),
            &[],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "openrouter/auto");
        assert_eq!(fb.profile, "openrouter");
    }

    /// A BARE default id resolves its provider through `model_providers`
    /// (same lookup the firstParty gate uses).
    #[test]
    fn bare_id_provider_resolved_via_model_providers() {
        let fb = connected_provider_fallback(
            "gpt-5.5",
            None,
            true,
            &providers(),
            &avail(&[("openai", false), ("deepseek", true)]),
            &listings(),
            &[],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "deepseek-chat");
    }

    /// Gateway install (custom base URL / `ANTHROPIC_AUTH_TOKEN`): the anthropic
    /// probe is NOT definitive, so an anthropic-routed default is kept even
    /// with other providers connected — Claude works through the gateway.
    #[test]
    fn gateway_install_keeps_anthropic_default() {
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            false,
            &providers(),
            &avail(&[("anthropic", false), ("deepseek", true)]),
            &listings(),
            &[],
        );
        assert!(fb.is_none());
    }

    /// The gateway flag only shields ANTHROPIC-routed defaults: a default on a
    /// disconnected non-anthropic provider still reroutes (the gateway serves
    /// Claude, not that provider).
    #[test]
    fn gateway_flag_does_not_shield_non_anthropic_defaults() {
        let fb = connected_provider_fallback(
            "claude-opus-4.8",
            Some("github-copilot"),
            false,
            &providers(),
            &avail(&[
                ("anthropic", false),
                ("deepseek", true),
                ("github-copilot", false),
            ]),
            &listings(),
            &[],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "deepseek-chat");
        assert_eq!(fb.profile, "deepseek");
    }

    /// A connected provider whose curated default is missing from the live
    /// catalog is skipped by the order pass; the first-listing pass still
    /// serves it.
    #[test]
    fn curated_default_missing_from_catalog_falls_to_first_listing() {
        let listings: Vec<traits::ModelListing> = vec![
            listing("anthropic", "claude-sonnet-5"),
            listing("deepseek", "deepseek-reasoner"), // no deepseek-chat
        ];
        let fb = connected_provider_fallback(
            "claude-sonnet-5",
            None,
            true,
            &providers(),
            &avail(&[("anthropic", false), ("deepseek", true)]),
            &listings,
            &[],
        )
        .expect("must reroute");
        assert_eq!(fb.model, "deepseek-reasoner");
        assert_eq!(fb.profile, "deepseek");
    }
}
