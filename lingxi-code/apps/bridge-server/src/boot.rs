//! S2 — env/argv → [`DesktopConfig`] resolution and connection assembly for the
//! production bridge-server binary.
//!
//! This module is the env/argv-reading half of the binary (mirroring
//! `apps/cli/src/init.rs`'s `resolve_desktop_config`), kept separate from
//! `main.rs` so the resolution + a clear non-secret startup error are unit
//! testable without spawning a process or a runtime.
//!
//! The assembly mirrors the F2-06 e2e harness (`tests/e2e_permission_test.rs`)
//! and the desktop composition root:
//!
//! 1. [`BridgeConnection::new`] is created and its connection-scoped
//!    `event_sink()` + `permission_sink()` are pulled.
//! 2. `engine_desktop::build` assembles a real [`DesktopRuntime`] from the
//!    resolved [`DesktopConfig`], wiring the orchestrator's
//!    [`client_adapter::AdapterOutputStream`] to that event sink and binding the
//!    connection-scoped `AdapterPermissionGate` to that permission sink
//!    (`use_noop_permission_gate: false`).
//! 3. The runtime's handles back a production [`OrchestratorTurnDriver`] +
//!    [`EngineCommandRouter`], which are bound onto the connection.
//!
//! ## Single-client model
//!
//! Per the [`crate::server`] docs, the walking skeleton models ONE client per
//! server (the design's single Electron child): the per-connection state lives
//! directly on one [`BridgeConnection`], shared across the endpoint's
//! `Arc<dyn FramePump>`. The runtime is built once at boot and that one
//! connection is handed to the endpoint.

use std::collections::BTreeMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use bridge::lockfile::{IdeLockfile, LockfileGuard};
use bridge::McpEndpoint;
use engine_desktop::{build, DesktopAudio, DesktopConfig, DesktopRuntime};
use platform_api::{
    CredentialStoragePolicy, OrchestratorHandle, OutputStream, SlashCommandDispatcher,
};
use platform_posix::PosixFileSystem;

use crate::audio_bridge::{new_audio_bridge, AudioBridge};
use crate::driver::{CredentialRequiredTurnDriver, OrchestratorTurnDriver};
use crate::mcp_bridge::McpPaths;
use crate::router::{EngineCommandRouter, SessionStoreContext};
use crate::server::{BridgeConnection, TurnDriver};
use crate::settings_bridge::{active_settings_baseline, SettingsContext, SettingsPaths};

/// Env override for the API base URL (mirrors `apps/cli`'s `resolve_api_base`).
pub const API_BASE_ENV: &str = "LINGXI_API_BASE_URL";
/// The canonical Anthropic endpoint used when [`API_BASE_ENV`] is unset.
pub const DEFAULT_API_BASE: &str = "https://api.anthropic.com";
/// Maximum accepted credential length from `--api-key-stdin`.
pub const MAX_STDIN_API_KEY_BYTES: usize = 16 * 1024;
/// Maximum accepted JSON credential envelope size from `--credential-stdin`.
pub const MAX_STDIN_CREDENTIAL_BYTES: usize = 64 * 1024;

/// Parsed command-line flags the binary accepts (a tiny hand-rolled parser;
/// `apps/bridge-server` deliberately avoids a clap dependency).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeArgs {
    /// `--cwd <dir>`: working directory to root the engine at. The process
    /// `chdir`s into this before resolving the rest of the config so the
    /// settings / hook / `.mcp.json` loaders read the same dir.
    pub cwd: Option<PathBuf>,
    /// `--session-id <uuid>`: stable session identity supplied by the Desktop host.
    pub session_id: Option<String>,
    /// `--list-sessions-json`: read the session catalog and exit without building an engine.
    pub list_sessions_json: bool,
    /// `--model <id>`: the default model id (overrides the desktop default).
    pub model: Option<String>,
    /// Read one bounded credential line from stdin before assembling the engine.
    pub api_key_stdin: bool,
    /// Read a bounded JSON envelope containing the API key and provider keys.
    pub credential_stdin: bool,
    /// Allow workspace-controlled configuration and customization sources.
    pub trusted_workspace: bool,
    /// Enforce the packaged desktop credential boundary: trusted workspace
    /// customizations still load, but credential-bearing settings must not.
    pub packaged_credential_stdin_only: bool,
    /// Override the discovery lockfile directory. Must be absolute.
    pub bridge_dir: Option<PathBuf>,
    /// `--help` / `-h`: print usage and exit.
    pub help: bool,
}

/// One-line outcome of [`BridgeArgs::parse`]: either the parsed args, or a
/// non-secret usage error string.
pub type ParseResult = Result<BridgeArgs, String>;

impl BridgeArgs {
    /// Parse from an argument iterator (excluding `argv[0]`). Accepts
    /// `--cwd <dir>`, `--model <id>`, `--help`/`-h`. Unknown flags are an error
    /// (returned as a non-secret string so the binary can print usage).
    ///
    /// # Errors
    /// Returns a human-readable usage error for an unknown flag or a flag that
    /// is missing its required value.
    pub fn parse<I, S>(args: I) -> ParseResult
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = BridgeArgs::default();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_ref() {
                "--help" | "-h" => out.help = true,
                "--list-sessions-json" => out.list_sessions_json = true,
                "--cwd" => {
                    let v = it
                        .next()
                        .ok_or_else(|| "--cwd requires a directory argument".to_string())?;
                    out.cwd = Some(PathBuf::from(v.as_ref()));
                }
                "--session-id" => {
                    let value = it
                        .next()
                        .ok_or_else(|| "--session-id requires a UUID argument".to_string())?;
                    let parsed = protocol::SessionId::parse_prefixed(value.as_ref())
                        .ok_or_else(|| "--session-id must be a valid UUID".to_string())?;
                    out.session_id = Some(parsed.as_uuid().to_string());
                }
                "--model" => {
                    let v = it
                        .next()
                        .ok_or_else(|| "--model requires a model id argument".to_string())?;
                    out.model = Some(v.as_ref().to_string());
                }
                "--api-key-stdin" => out.api_key_stdin = true,
                "--credential-stdin" => out.credential_stdin = true,
                "--trusted-workspace" => out.trusted_workspace = true,
                "--packaged-credential-stdin-only" => {
                    out.packaged_credential_stdin_only = true;
                }
                "--bridge-dir" => {
                    let v = it
                        .next()
                        .ok_or_else(|| "--bridge-dir requires a directory argument".to_string())?;
                    let path = PathBuf::from(v.as_ref());
                    if !path.is_absolute() {
                        return Err("--bridge-dir must be an absolute path".to_string());
                    }
                    out.bridge_dir = Some(path);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(out)
    }
}

/// The usage text printed by `--help` (and on a parse error).
#[must_use]
pub fn usage() -> String {
    format!(
        "lingxi-bridge-server {}\n\
         \n\
         Serves one local conversation over a loopback WebSocket for the Electron/iOS shell.\n\
         The LLM API key can be supplied as one bounded line on stdin.\n\
         \n\
         USAGE:\n    \
             bridge-server [OPTIONS]\n\
         \n\
         OPTIONS:\n    \
             --cwd <DIR>      Working directory to root the engine at (default: current dir)\n    \
             --session-id <UUID>\n                              Stable session UUID (generated when omitted)\n    \
             --list-sessions-json\n                              List persisted sessions for --cwd and exit\n    \
             --model <ID>     Default model id (default: the desktop build default)\n    \
             --api-key-stdin  Read the API key from one line on stdin\n    \
             --credential-stdin\n    \
                              Read a provider credential envelope from stdin\n    \
             --trusted-workspace\n    \
                              Enable workspace settings, hooks, MCP, agents, plugins, and memory\n    \
             --packaged-credential-stdin-only\n    \
                              In trusted packaged desktop mode, accept credentials only from stdin\n    \
             --bridge-dir <DIR>\n    \
                              Absolute discovery lockfile directory\n    \
             -h, --help       Print this help\n\
         \n\
         ENVIRONMENT:\n    \
             {API_BASE_ENV}  Override the API base URL (default: {DEFAULT_API_BASE})\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Read exactly one credential line with a hard allocation bound.
///
/// Only the trailing line ending is removed; credential whitespace is otherwise
/// preserved. Error text never contains input bytes.
pub fn read_api_key_line<R: BufRead>(reader: &mut R) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(256);
    let mut limited = (&mut *reader).take((MAX_STDIN_API_KEY_BYTES + 2) as u64);
    limited
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "failed to read API key from stdin".to_string())?;

    let without_lf = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    let content_len = without_lf.strip_suffix(b"\r").unwrap_or(without_lf).len();
    if content_len > MAX_STDIN_API_KEY_BYTES
        || (bytes.len() > MAX_STDIN_API_KEY_BYTES && !bytes.ends_with(b"\n"))
    {
        return Err(format!(
            "API key from stdin exceeds the {MAX_STDIN_API_KEY_BYTES}-byte limit"
        ));
    }
    if !bytes.ends_with(b"\n") && bytes.len() == MAX_STDIN_API_KEY_BYTES + 2 {
        return Err(format!(
            "API key from stdin exceeds the {MAX_STDIN_API_KEY_BYTES}-byte limit"
        ));
    }
    if bytes.ends_with(b"\n") {
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
    }
    String::from_utf8(bytes).map_err(|_| "API key from stdin must be valid UTF-8".to_string())
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// One bounded credential payload accepted from the Electron parent.
pub struct CredentialEnvelope {
    /// Optional legacy Anthropic API key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Provider id to API key/bearer token mappings.
    #[serde(default)]
    pub provider_keys: BTreeMap<String, String>,
    /// Sensitive plugin configuration, keyed by plugin identity then manifest
    /// field name. Values are injected into the runtime's process-local
    /// credential cache before plugin discovery.
    #[serde(default)]
    pub plugin_secrets: BTreeMap<String, BTreeMap<String, String>>,
}

/// Read and validate the one-line JSON envelope used by the packaged desktop.
/// The returned values never appear in an error message or command-line flag.
pub fn read_credential_envelope<R: BufRead>(reader: &mut R) -> Result<CredentialEnvelope, String> {
    let mut bytes = Vec::with_capacity(512);
    let mut limited = (&mut *reader).take((MAX_STDIN_CREDENTIAL_BYTES + 2) as u64);
    limited
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "failed to read credentials from stdin".to_string())?;
    let line = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.is_empty() || line.len() > MAX_STDIN_CREDENTIAL_BYTES {
        return Err(format!(
            "credential envelope exceeds the {MAX_STDIN_CREDENTIAL_BYTES}-byte limit"
        ));
    }
    let envelope: CredentialEnvelope =
        serde_json::from_slice(line).map_err(|_| "invalid credential envelope".to_string())?;
    if let Some(key) = envelope.api_key.as_ref() {
        if key.is_empty() || key.len() > MAX_STDIN_API_KEY_BYTES || key.contains('\0') {
            return Err("invalid API key in credential envelope".to_string());
        }
    }
    if envelope.provider_keys.len() > 32 {
        return Err("credential envelope contains too many providers".to_string());
    }
    for (provider, key) in &envelope.provider_keys {
        if provider.is_empty()
            || provider.len() > 64
            || !provider.chars().enumerate().all(|(index, ch)| {
                ch.is_ascii_lowercase()
                    || ch.is_ascii_digit()
                    || (index > 0 && matches!(ch, '-' | '_' | '.'))
            })
        {
            return Err("invalid provider id in credential envelope".to_string());
        }
        if key.is_empty() || key.len() > MAX_STDIN_API_KEY_BYTES || key.contains('\0') {
            return Err("invalid provider credential in credential envelope".to_string());
        }
    }
    let plugin_secret_count: usize = envelope.plugin_secrets.values().map(BTreeMap::len).sum();
    if plugin_secret_count > 256 {
        return Err("credential envelope contains too many plugin secrets".to_string());
    }
    for (plugin, values) in &envelope.plugin_secrets {
        if plugin.is_empty()
            || plugin.len() > 128
            || !plugin
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '@'))
        {
            return Err("invalid plugin id in credential envelope".to_string());
        }
        for (field, secret) in values {
            if field.is_empty()
                || field.len() > 128
                || !field.chars().enumerate().all(|(index, ch)| {
                    ch.is_ascii_alphabetic()
                        || ch == '_'
                        || (index > 0 && (ch.is_ascii_digit() || matches!(ch, '.' | '-')))
                })
            {
                return Err("invalid plugin secret key in credential envelope".to_string());
            }
            if secret.is_empty() || secret.len() > MAX_STDIN_API_KEY_BYTES || secret.contains('\0')
            {
                return Err("invalid plugin secret in credential envelope".to_string());
            }
        }
    }
    Ok(envelope)
}

/// Resolve the API base URL: honours [`API_BASE_ENV`], else [`DEFAULT_API_BASE`].
#[must_use]
pub fn resolve_api_base() -> String {
    std::env::var(API_BASE_ENV).unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
}

fn desktop_provider_catalog_listings(cfg: &DesktopConfig) -> Vec<platform_api::ModelListing> {
    let assembled = provider_config::assemble(provider_config::AssembleInputs {
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: desktop_provider_catalog_anthropic_models(
            &cfg.default_model,
            cfg.fallback_model.as_deref(),
        ),
        anthropic_has_api_key: false,
        anthropic_has_oauth: false,
        user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
        routing: cfg.routing.clone(),
    });
    for warning in &assembled.warnings {
        tracing::warn!(warning = %warning, "bridge-server provider catalog assembly");
    }
    llm_client::ModelRegistry::from_config(assembled.client_config)
        .map(|registry| {
            registry
                .available_models()
                .into_iter()
                .map(orchestrator::provider_adapter::lower_model_listing)
                .collect()
        })
        .unwrap_or_default()
}

fn desktop_provider_catalog_anthropic_models(
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
        "claude-opus-5".to_string(),
        "claude-opus-4-8".to_string(),
        "claude-opus-4-6".to_string(),
        "claude-opus-4-5-20251101".to_string(),
        "claude-opus-4-1-20250805".to_string(),
        "claude-opus-4-20250514".to_string(),
        "claude-sonnet-5".to_string(),
        "claude-sonnet-4-6".to_string(),
        "claude-sonnet-4-5-20250929".to_string(),
        "claude-haiku-4-5".to_string(),
        "claude-fable-5-1".to_string(),
    ];
    let admit = |value: &str| -> Option<String> {
        let (profile, bare) = llm_client::split_profile_model(value);
        (profile == "anthropic" && !bare.is_empty() && !bare.contains('/')).then_some(bare)
    };
    let fallback_models = fallback_model
        .into_iter()
        .flat_map(|csv| csv.split(','))
        .map(str::trim)
        .filter(|model| !model.is_empty());
    for model in std::iter::once(default_model).chain(fallback_models) {
        if let Some(bare) = admit(model) {
            ids.push(bare);
        }
    }
    for var in [
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    ] {
        if let Ok(model) = std::env::var(var) {
            if let Some(bare) = admit(model.trim()) {
                ids.push(bare);
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
            metadata: Default::default(),
            capabilities: caps,
        })
        .collect()
}

/// Load the merged settings `providers` / `routing` / `apiKeyHelper` blocks from the layered
/// settings (project + user + env), rooted at the *current* working directory
/// (the process has already `chdir`'d into any `--cwd`). Returns `(None, None)`
/// on any load failure — callers then fall back to built-in profiles + the
/// default routing. Mirrors `apps/cli/src/init.rs`'s `load_provider_profiles`
/// + `load_routing`.
#[must_use]
fn load_settings_blocks() -> (
    Option<BTreeMap<String, serde_json::Value>>,
    Option<serde_json::Value>,
    Option<String>,
) {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let inputs = lingxi_core::settings::LoadInputs {
        env: &env,
        project_dir: &project_dir,
        defaults: lingxi_core::settings::schema::SettingsJson::default(),
    };
    match lingxi_core::settings::Settings::load(inputs) {
        Ok(eff) => (
            eff.settings.providers,
            eff.settings.routing,
            eff.settings
                .api_key_helper
                .filter(|helper| !helper.trim().is_empty()),
        ),
        Err(_) => (None, None, None),
    }
}

#[must_use]
fn without_settings_credentials(
    profiles: Option<BTreeMap<String, serde_json::Value>>,
) -> Option<BTreeMap<String, serde_json::Value>> {
    profiles.map(|mut profiles| {
        for profile in profiles.values_mut() {
            if let Some(fields) = profile.as_object_mut() {
                fields.remove("apiKeyEnv");
            }
        }
        profiles
    })
}

/// User config-home: `$LINGXI_CONFIG_DIR` when set (including an empty value),
/// else `~/.lingxi`. Shared by desktop config and lockfile resolution.
#[must_use]
pub fn lingxi_config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(branding::CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(branding::DOT_DIR))
}

/// Resolve a deterministic [`DesktopConfig`] from [`BridgeArgs`] + `std::env`.
///
/// Mirrors `apps/cli/src/init.rs`'s `resolve_desktop_config` field-for-field,
/// with the one production difference for a transport: `use_noop_permission_gate`
/// is `false`, so `engine_desktop::build` binds the connection-scoped
/// `AdapterPermissionGate` (the WS permission round-trip, F2-06).
///
/// `cwd` is `std::env::current_dir()` — the caller is expected to have already
/// `chdir`'d into any `--cwd` (so the settings / hook / `.mcp.json` loaders read
/// the same dir).
///
/// The API key is deliberately left empty here; `main` may assign the bounded
/// stdin value immediately before assembly. An empty key is valid for transport
/// testing and only fails when a live turn needs credentials.
#[must_use]
pub fn resolve_desktop_config(args: &BridgeArgs) -> DesktopConfig {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let lingxi_home = lingxi_config_home().unwrap_or_else(|| PathBuf::from("/dev/null"));
    let project_mcp_path = cwd.join(".mcp.json");
    // User/global-scope MCP servers live INSIDE `~/.lingxi.json` (top-level
    // `mcpServers`), exactly like claude-code — NOT a standalone file under the
    // OS config dir. The loader reads only that key
    // (`mcp::parse_global_config_mcp_servers`).
    let global_mcp_path = migrations::global_config::global_config_path()
        .unwrap_or_else(|| PathBuf::from("/dev/null"));

    let mut default_model = DesktopConfig::default().default_model;
    if let Some(m) = &args.model {
        default_model.clone_from(m);
    }

    let (provider_profiles, routing, api_key_helper) = if args.trusted_workspace {
        let (provider_profiles, routing, api_key_helper) = load_settings_blocks();
        if !args.packaged_credential_stdin_only {
            (provider_profiles, routing, api_key_helper)
        } else {
            // Keep routes available for credentials supplied by the parent,
            // without permitting settings to introduce credential sources.
            (
                without_settings_credentials(provider_profiles),
                routing,
                None,
            )
        }
    } else {
        (None, None, None)
    };
    let trusted = args.trusted_workspace;

    DesktopConfig {
        initial_teammate_team_name: None,
        api_base: resolve_api_base(),
        // Credentials are supplied explicitly by the parent over stdin and
        // assigned by `main` immediately before assembly. Never inherit them
        // from environment or argv.
        api_key: String::new(),
        // The packaged bridge gets an explicit process-local store. Desktop,
        // CLI, and TUI own persistent broker access; this sidecar receives only
        // its current session credential over stdin.
        isolated_credential_storage: false,
        credential_storage_policy: if args.packaged_credential_stdin_only {
            CredentialStoragePolicy::NativeOrMemory
        } else {
            CredentialStoragePolicy::NativePreferred
        },
        injected_plugin_secrets: BTreeMap::new(),
        api_key_helper,
        // (M13) The bridge host does not resolve managed login-method forcing
        // (the Electron parent owns credential policy) and passes no
        // FD-inherited key — both auth-resolver inputs stay at their inert
        // defaults.
        managed_oauth_only: false,
        anthropic_key_fd_present: false,
        cwd,
        lingxi_home,
        default_model,
        // Only a `--model` arg is an explicit per-session choice; the built-in
        // default stays eligible for the engine's connected-provider fallback.
        default_model_explicit: args.model.is_some(),
        // The bridge does not read the TUI's `settings.recentModels`; the
        // fallback uses the static provider order only.
        recent_models: Vec::new(),
        // The Electron bridge does not expose --fallback-model (CLI --print
        // only); the Opus consecutive-529 fallback stays disabled here.
        fallback_model: None,
        // The Electron host currently exposes no custom beta-header flag.
        custom_betas: Vec::new(),
        // The Electron host does not expose CLI `--settings` / flagSettings.
        flag_settings: None,
        provider_profiles,
        routing,
        mcp_paths: if trusted {
            vec![project_mcp_path, global_mcp_path]
        } else {
            Vec::new()
        },
        // A transport binds the connection-scoped AdapterPermissionGate (F2-06).
        use_noop_permission_gate: false,
        // Transport host: the adapter gate IS the enforcement, so headless
        // deny-on-ask does not apply (only consulted when use_noop is true).
        deny_unresolved_ask: false,
        // Transport host: no process stdout TTY for `tengu_api_success.isTTY`.
        is_tty: false,
        // Transport host: no injected interactive gate (that is the TUI's path).
        injected_permission_gate: None,
        // The headless bridge has no interactive /fork or /resume-as-background
        // surface, so it wires no background-session forker seam.
        bg_session_forker: None,
        // Placeholder — `assemble_with_provider_keys` installs the
        // connection-scoped AskUserQuestion broker once the event sink exists.
        ask_user_question_tx: None,
        // Placeholder — `resolve_desktop_config` has no live connection to
        // build a sink from yet. `assemble_with_provider_keys` overwrites this
        // to `Some(sender)` once the `BridgeConnection` (and therefore its
        // `computer_access_sink()`) exists, wiring the Electron-facing
        // `BridgeComputerAccessBroker`.
        computer_access_tx: None,
        // Filled by `assemble_with_provider_keys` after the connection-scoped
        // event sink exists; this keeps subagent lifecycle events on the same
        // authenticated session stream as ordinary turn events.
        session_agent_observer: None,
        // M10: the bridge-server does not start a coordinator session by
        // default (threading this from session metadata is a follow-up).
        session_started_as_coordinator: false,
        // Production memory: load the real `<cwd>/LINGXI.md` +
        // `~/.lingxi/LINGXI.md` hierarchy into the system prompt (claude-code
        // parity), which also makes the session-start
        // `fire_instructions_loaded()` fire over those files. Tests inject a
        // controlled provider (or `None`); only this real-host path reads the FS.
        memory_provider: trusted.then(orchestrator::prompt::real_provider),
        // Trusted desktop sessions share the engine Auto default. Workspace
        // trust is still required before granting autonomous permissions.
        permission_mode: if trusted {
            permission::PermissionMode::Auto
        } else {
            permission::PermissionMode::Default
        },
        permission_mode_cli: None,
        permission_mode_cli_explicit: false,
        // Plan 3c: bridge has no interactive secure prompt; headless no-op.
        connect_prompt: None,
        // The Electron bridge exposes no --max-turns / --max-budget flags;
        // both stay unset (unbounded), matching the CLI defaults.
        max_turns: None,
        plan_mode_instructions: None,
        plans_directory: None,
        max_budget_usd: None,
        // The bridge has no structured-output flag; unconstrained turns.
        json_schema: None,
        // The bridge has no --system-prompt / --append-system-prompt CLI flags.
        system_prompt_override: None,
        append_system_prompt: None,
        // The Desktop host owns the session UUID so multiple bridge processes can
        // address one another without relying on generated display names.
        session_id_override: args.session_id.clone(),
        parent_session_id: None,
        // The Electron bridge has no --disable-slash-commands flag.
        disable_slash_commands: false,
        // The Electron bridge has no --add-dir flag.
        add_dir: Vec::new(),
        // The Electron bridge has no --mcp-config flag.
        cli_mcp_servers: Vec::new(),
        // The Electron bridge has no --strict-mcp-config flag either; ambient
        // MCP configs stay eligible (trust gating above still applies) and
        // agent-frontmatter servers are never strict-skipped.
        strict_mcp_config: false,
        restricted: false,
        restricted_tools: None,
        // The Electron bridge has no --exclude-dynamic-system-prompt-sections flag.
        exclude_dynamic_system_prompt_sections: false,
        // The bridge has no `--setting-sources` flag; load all tiers.
        setting_source_scope: if trusted {
            (true, true)
        } else {
            (false, false)
        },
        // The Electron bridge has no --safe-mode / --bare flags (CLI-only
        // reduced modes); all customizations load.
        customization_gates: if trusted {
            engine_desktop::CustomizationGates::default()
        } else {
            engine_desktop::CustomizationGates {
                safe_mode: true,
                bare: false,
            }
        },
        // The Electron bridge has no --no-session-persistence flag; persist.
        session_persistence: true,
        // (M4 cc2.1.198) The Electron bridge exposes none of --agents /
        // --agent / --plugin-dir / --effort (CLI session flags).
        cli_agents_json: None,
        cli_agent: None,
        cli_plugin_dirs: Vec::new(),
        initial_effort: None,
        // The Electron bridge (accepted divergence) keeps the byte-identical
        // defaults for the ANTHROPIC_MODEL env-pin exemption + boot thinking
        // config; the CLI `resolve_desktop_config` is the parity surface.
        default_model_env_pinned: false,
        session_thinking: Default::default(),
        // The Electron bridge has no `-w`/`--worktree` flag; inert boot (no
        // worktree launch).
        worktree_launch: None,
        // The Electron bridge has no `--tmux` flag; inert (no tmux session).
        tmux_launch: None,
        // The trusted Electron shell exposes an explicit, user-operated Full
        // access selector. This only makes the live bypass mode AVAILABLE; the
        // session still boots in Default and changes mode only after an
        // authenticated renderer command crosses the bridge IPC boundary.
        allow_dangerously_skip_permissions: trusted,
        // Device audio is CONNECTION-scoped, not argv-scoped: the bridge it
        // proxies through cannot exist until `assemble` has a connection to
        // build it over, so it is filled there and never here.
        audio: None,
    }
}

/// True iff the config has neither an API key/apiKeyHelper NOR any
/// settings-configured provider profile — i.e. no way to authenticate a live
/// turn. The server still boots (transport testing and read-only listings remain
/// valuable), but [`assemble`] binds a fail-fast turn driver so no provider call
/// or retry is attempted.
#[must_use]
pub fn has_no_credential_source(cfg: &DesktopConfig) -> bool {
    cfg.api_key.is_empty()
        && cfg
            .api_key_helper
            .as_deref()
            .is_none_or(|helper| helper.trim().is_empty())
        && cfg
            .provider_profiles
            .as_ref()
            .is_none_or(BTreeMap::is_empty)
}

fn needs_credential_driver(
    parent_credential_supplied: bool,
    provider_availability: &BTreeMap<String, bool>,
    late_credential_route: bool,
) -> bool {
    !parent_credential_supplied
        && !provider_availability.values().any(|available| *available)
        && !late_credential_route
}

/// The assembled, ready-to-serve connection plus its auth-relevant facts.
///
/// The connection is the [`bridge::FramePump`] the endpoint drives; the runtime
/// is held so its background tasks (cost-persist drain, etc.) stay alive for the
/// process lifetime.
pub struct BoundServer {
    /// The fully-bound connection (driver + router attached).
    pub connection: BridgeConnection,
    /// The desktop runtime — held (not read) so its handles and the spawned
    /// background tasks it owns (the cost-persist drain, the orchestrator's
    /// registries) stay alive for as long as the server serves. Not exposed:
    /// `DesktopRuntime` carries broad handles (the tool registry among them)
    /// that nothing outside this struct has a reason to hold.
    runtime: DesktopRuntime,
    /// The request half of THIS connection's audio bridge — the same object
    /// that went into `DesktopConfig::audio` and whose responder half is bound
    /// to `connection`. Held so the pairing is visible at the composition root
    /// rather than only implied by the order of two statements inside
    /// [`assemble_with_provider_keys`].
    audio: Arc<AudioBridge>,
    /// Keeps the cross-session live identity and inbox registered for the
    /// lifetime of this bridge process.
    #[allow(dead_code)]
    live_session: LiveSessionGuard,
}

impl BoundServer {
    /// The names of the tools the assembled engine registered.
    ///
    /// Delegates to [`DesktopRuntime::registered_tool_names`]; the runtime
    /// itself stays inside this struct.
    #[must_use]
    pub fn registered_tool_names(&self) -> Vec<String> {
        self.runtime.registered_tool_names()
    }

    /// This connection's audio bridge — the engine-side object every
    /// `SpeechToText` / `TextToSpeech` / `VoiceRecorder` call on this
    /// connection goes through.
    #[must_use]
    pub fn audio(&self) -> &Arc<AudioBridge> {
        &self.audio
    }
}

/// Process-local live-session registration owned by a bridge runtime.
///
/// The registry and inbox are intentionally process-wide in `traits`, just as
/// they are for the CLI. Holding this guard in `BoundServer` makes the bridge
/// lifecycle explicit and guarantees graceful cleanup without changing the
/// engine wire protocol.
struct LiveSessionGuard {
    dir: platform_api::live_sessions::LiveSessionDir,
    session_id: String,
    inbox_started: bool,
    _writer_claim: platform_api::live_sessions::SessionIdClaim,
}

impl Drop for LiveSessionGuard {
    fn drop(&mut self) {
        if self.inbox_started {
            platform_api::uds_inbox::stop_process_inbox();
        }
        let _ = self.dir.unregister(&self.session_id);
    }
}

fn canonical_session_id(raw: Option<&str>) -> Result<String, String> {
    raw.map(|value| {
        protocol::SessionId::parse_prefixed(value)
            .map(|id| id.as_uuid().to_string())
            .ok_or_else(|| "bridge session id must be a valid UUID".to_string())
    })
    .unwrap_or_else(|| Ok(protocol::SessionId::new().as_uuid().to_string()))
}

fn initialize_live_session(cfg: &mut DesktopConfig) -> Result<LiveSessionGuard, String> {
    let session_id = canonical_session_id(cfg.session_id_override.as_deref())?;
    cfg.session_id_override = Some(session_id.clone());

    let dir =
        platform_api::live_sessions::LiveSessionDir::at_live(cfg.lingxi_home.join("sessions"));
    let pid = std::process::id();
    // Only live records/PIDs participate in writer ownership. The persisted
    // `<session-id>.jsonl` transcript is intentionally ignored here because a
    // historical resume must reuse that same UUID.
    let writer_claim =
        dir.claim_session_id(&session_id, pid)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    "bridge session id is already active".to_string()
                }
                _ => "bridge live-session registry is unavailable".to_string(),
            })?;

    // Keep the claim in a guard from this point onward. If any later live
    // registration step fails, Drop releases the claim and does not strand a
    // writer lock for the next historical resume.
    let mut live_session = LiveSessionGuard {
        dir: dir.clone(),
        session_id: session_id.clone(),
        inbox_started: false,
        _writer_claim: writer_claim,
    };

    let display_name = cfg
        .cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty());
    let claim = platform_api::live_sessions::install_process(
        dir.clone(),
        &session_id,
        display_name.as_deref(),
    );
    if let Some(claim) = claim.as_ref() {
        if claim.notice.is_some() {
            tracing::debug!("bridge live-session display name was disambiguated");
        }
    }
    if claim.is_none() {
        if let Some(name) = display_name.as_deref() {
            platform_api::live_sessions::set_process_name(name);
        }
    }

    let permission_mode = cfg.permission_mode.wire_str().to_string();
    platform_api::live_sessions::set_process_permission_mode(
        &permission_mode,
        cfg.allow_dangerously_skip_permissions,
    );

    let socket = platform_api::uds_inbox::default_socket_path(pid);
    let inbox_started = match platform_api::uds_inbox::start_process_inbox(socket) {
        Ok(path) => {
            live_session.inbox_started = true;
            dir.upsert_identity(
                pid,
                &session_id,
                platform_api::live_sessions::process_name().as_deref(),
                None,
                Some(&path),
                platform_api::live_sessions::process_permission_class().as_deref(),
            )
            .map_err(|_| "bridge live-session identity is unavailable".to_string())?;
            true
        }
        Err(error) => {
            // File inbox remains a valid fallback on platforms without UDS.
            tracing::warn!(%error, "bridge cross-session UDS inbox unavailable; using file inbox fallback");
            dir.upsert_identity(
                pid,
                &session_id,
                platform_api::live_sessions::process_name().as_deref(),
                None,
                None,
                platform_api::live_sessions::process_permission_class().as_deref(),
            )
            .map_err(|_| "bridge live-session identity is unavailable".to_string())?;
            false
        }
    };
    platform_api::live_sessions::set_process_status("idle", None);
    debug_assert_eq!(live_session.inbox_started, inbox_started);
    Ok(live_session)
}

/// Enumerate the persisted session catalog without constructing an engine.
///
/// This path deliberately does not call config resolution, credential loading,
/// customization loaders, or `engine_desktop::build`; it only reads the JSONL
/// catalog rooted at `cwd` and returns the stable Desktop envelope.
pub async fn list_sessions_json(cwd: &Path) -> Result<String, String> {
    let lingxi_home =
        lingxi_config_home().ok_or_else(|| "session catalog requires a config home".to_string())?;
    list_sessions_json_from(cwd, &lingxi_home).await
}

async fn list_sessions_json_from(cwd: &Path, lingxi_home: &Path) -> Result<String, String> {
    let fs: Arc<dyn platform_api::FileSystem> = Arc::new(PosixFileSystem::new(cwd.to_path_buf()));
    let catalog = match session::jsonl::list_recent_sessions_with_diagnostics(
        lingxi_home,
        &cwd.to_string_lossy(),
        usize::MAX,
        fs.clone(),
    )
    .await
    {
        Ok(catalog) => catalog,
        Err(session::jsonl::LoaderError::EmptyDirectory) => session::jsonl::SessionCatalog {
            sessions: Vec::new(),
            skipped_files: 0,
        },
        Err(_) => return Err("session catalog is unavailable".to_string()),
    };
    let mut sessions = Vec::with_capacity(catalog.sessions.len());
    for row in catalog.sessions {
        let empty_session = if row.message_count == 0 {
            session::jsonl::JsonlReader::new(row.path.clone(), fs.clone())
                .read_routed()
                .await
                .ok()
                .is_some_and(|loaded| {
                    loaded.messages_in_order.is_empty()
                        && loaded.mobile_empty_sessions.contains(&row.uuid.to_string())
                })
        } else {
            false
        };
        let lowered = client_adapter::lowering::lower_session_metadata(&row);
        let mut value = serde_json::to_value(lowered)
            .map_err(|_| "failed to encode session catalog".to_string())?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "empty_session".to_string(),
                serde_json::Value::Bool(empty_session),
            );
            if let Some(model) = row.resume_model.as_deref() {
                object.insert(
                    "resume_model".to_string(),
                    serde_json::Value::String(platform_api::qualified_model_ref(
                        model,
                        row.resume_model_profile.as_deref(),
                    )),
                );
            }
        }
        sessions.push(value);
    }
    serde_json::to_string(&serde_json::json!({
        "version": 1,
        "sessions": sessions,
    }))
    .map_err(|_| "failed to encode session catalog".to_string())
}

/// Regression ceiling used by the credential-publication tests. Production no
/// longer waits for the broker-backed reconciliation probe at all: the known
/// mutation is published synchronously and the probe is detached.
///
/// Round-10 finding N3. The detached engine refresh re-runs
/// `provider_config::compute_availability_with_isolation` over every
/// `CredentialSource` of each registered refresher, which bottoms out in the
/// (on macOS, possibly brokered) keychain — the exact call the boot probe
/// deliberately wraps in a 5s `tokio::time::timeout`
/// (`apps/engine-desktop/src/lib.rs`, `resolve_llm_stack`) because a contended
/// broker can stall it. The historical five-second budget remains a generous
/// assertion ceiling for tests of the now-immediate publication path.
#[cfg(test)]
pub(crate) const FUSION_CATALOG_REFRESH_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(5);

/// Publish each known credential mutation, then start one detached reconciliation
/// probe. The historical name is retained for the two bridge call sites.
///
/// Two properties this seam exists for, both of which the inline
/// `for … { refresh(id).await }` it replaced got wrong:
///
/// * **Non-blocking.** `engine_desktop::refresh_fusion_catalog_after_credential_write`
///   is a keychain re-probe with no internal timeout. Awaiting it inline put an
///   unbounded stall on two paths that must not have one: connection assembly
///   (nothing serves the Electron client until `assemble_with_provider_keys`
///   returns) and `EngineCommandRouter::route` (awaited straight from the
///   connection's read loop in `server.rs`'s `on_frame`, whose contract is
///   "return promptly" — a stall there stops the client's interrupts and
///   permission replies from even being READ).
/// * **One detached probe for N ids.** The packaged Electron host supplies one
///   key per configured provider. Publishing all known ids first and spawning
///   once avoids N identical full-source scans.
///
/// The spawned task keeps running, so the availability map still converges once
/// the broker answers. The caller never waits for it: readiness depends only on
/// the cheap publication above, not on task scheduling or keychain latency.
pub(crate) async fn refresh_fusion_catalog_bounded(provider_ids: Vec<String>) -> bool {
    if provider_ids.is_empty() {
        return true;
    }
    // Publish the known mutation before returning readiness to the caller.
    // The full broker scan has one detached owner for the whole runtime and is
    // epoch-guarded, so it cannot block this connection or resurrect a newer
    // delete.  Anthropic's bridge provider id is a UI id; normalize it before
    // the engine records the mutation so OAuth is never mistaken for a key.
    let mut routable = true;
    for provider_id in provider_ids {
        let mutation_id = if provider_id == "anthropic" {
            "anthropic-api-key"
        } else {
            provider_id.as_str()
        };
        routable &= engine_desktop::publish_fusion_catalog_credential(mutation_id).await;
    }
    if routable {
        engine_desktop::spawn_fusion_catalog_refresh();
    }
    routable
}

/// Seed the provider keys the parent process handed over the dedicated stdin
/// boundary into the runtime's process-local credential cache, then tell
/// Fusion's catalog filter about them.
///
/// Round 9, item 2's class sweep: these keys are seeded AFTER `build` already
/// computed the boot `provider_availability` map and registered the
/// process-wide `FusionCatalogRefresher`, so without the refresh Fusion's
/// `FusionCatalogModelSource::list()` keeps filtering against a map in which the
/// parent-supplied profile is absent/false — its rows are dropped for the whole
/// engine process even though `/model` and the turn loop route it. An ephemeral
/// key is never persisted and so can never be re-probed;
/// `refresh_after_credential_write` force-marks the named profile available,
/// which is the only correct answer here.
///
/// Round-10 finding N3: the refresh is bounded (see
/// [`refresh_fusion_catalog_bounded`]) — it used to be one unbounded keychain
/// re-probe PER key, inline on the assembly path.
///
/// Split out of `assemble_with_provider_keys` so it is reachable from a test
/// without a full engine build.
pub(crate) async fn seed_parent_supplied_provider_keys(
    credentials: &Arc<secret::CredentialManager>,
    provider_keys: &BTreeMap<String, String>,
) {
    for (provider_id, secret) in provider_keys {
        credentials
            .set_provider_key_ephemeral(provider_id, secret)
            .await;
    }
    let _ = refresh_fusion_catalog_bounded(provider_keys.keys().cloned().collect()).await;
}

/// Assemble a fully-bound [`BridgeConnection`] from a resolved [`DesktopConfig`].
///
/// Wires the connection-scoped sinks into a real [`DesktopRuntime`]
/// (`engine_desktop::build`), then binds the production
/// [`OrchestratorTurnDriver`] (with an error sink so a turn-level failure
/// reaches the client) and the [`EngineCommandRouter`] over the runtime handles.
///
/// # Errors
/// Returns the [`engine_desktop::BuildError`] string if the engine cannot be
/// assembled (effectively infallible today).
pub async fn assemble(cfg: DesktopConfig) -> Result<BoundServer, String> {
    assemble_with_provider_keys(cfg, BTreeMap::new()).await
}

/// Assemble a runtime after injecting provider credentials received over the
/// dedicated stdin boundary. Provider secrets never enter `DesktopConfig` or
/// the process environment; they are held only in the engine process. The
/// Electron host owns persistent Keychain storage, so packaged startup does
/// not write the same key into the Rust Keychain on every launch.
pub async fn assemble_with_provider_keys(
    mut cfg: DesktopConfig,
    provider_keys: BTreeMap<String, String>,
) -> Result<BoundServer, String> {
    let provider_credentials_ephemeral = matches!(
        cfg.credential_storage_policy,
        platform_api::CredentialStoragePolicy::NativeOrMemory
    );
    let live_session = initialize_live_session(&mut cfg)?;
    let connection = BridgeConnection::new();

    // Preserve only the non-secret parent-source fact before `cfg` moves. The
    // authoritative decision is completed after `build`, when the runtime has
    // checked the same Rust secure store used by CLI and TUI.
    let parent_credential_supplied = !has_no_credential_source(&cfg) || !provider_keys.is_empty();

    // Capture the persisted-session inputs before `cfg` moves into the desktop
    // composition root. The router uses the same cwd/config-home pair as the
    // orchestrator's JSONL writer, so list and resume address the exact store
    // this connection writes.
    let session_store = SessionStoreContext::new(
        cfg.lingxi_home.clone(),
        cfg.cwd.to_string_lossy().into_owned(),
        Arc::new(PosixFileSystem::new(cfg.cwd.clone())),
    );

    // The layered-settings read path, captured from the SAME roots the engine
    // loads its own settings from, again before `cfg` moves into the desktop
    // composition root.
    //
    // `active` is snapshotted ONCE, here, and answers "what did this session
    // load at boot" — the baseline a later on-disk edit, or a later listing's
    // freshly re-read `effective`, is shown as diverging from. It DOES fold in
    // the managed overlay, even though it is otherwise file-layers-only and
    // never re-read: the engine loads managed (policy) settings at boot too,
    // same as the three files, so a managed key is just as much "already
    // loaded" as a `user`/`project`/`local` one. Leaving it out here made a
    // managed key differ from `effective` (which always re-applies the same
    // overlay) permanently and unfixably — a "restart to apply" banner for a
    // change no restart can ever apply, since the user never wrote it and no
    // restart changes it. Folding it in here, once, from the same overlay
    // `effective` re-applies every time, makes the two agree on managed keys
    // forever, which is the correct answer: nothing IS pending on a key the
    // user cannot change.
    let settings_context = {
        let paths = SettingsPaths {
            lingxi_home: cfg.lingxi_home.clone(),
            project_dir: cfg.cwd.clone(),
        };
        // Managed (policy) discovery is the desktop composition root's job;
        // bridge-server does not locate those tiers itself. Resolved once,
        // here, and reused for both `active`'s one-time bake-in below and the
        // `managed` field every later listing re-applies to `effective` — the
        // same map both places, so the two can never drift apart.
        let managed = engine_desktop::managed_settings_overlay().await;
        let active = active_settings_baseline(&paths, &managed);
        SettingsContext {
            paths,
            active: Arc::new(std::sync::RwLock::new(active)),
            managed,
        }
    };

    // The MCP write-side roots, captured from `cfg` before it moves into the
    // desktop composition root below — same pattern as `settings_context`
    // above. `global_config_path` is resolved the SAME way
    // `resolve_desktop_config`'s `global_mcp_path` is (rather than being
    // derived from `cfg.mcp_paths`, which the trust gate can null out): the
    // desktop's OWN edit to `~/.lingxi.json` is a deliberate user action, not
    // an automatic load of workspace-supplied config, so it always targets
    // the real file regardless of whether THIS workspace is currently
    // trusted to auto-load a repo-supplied `.mcp.json`.
    let mcp_paths = McpPaths {
        project_dir: cfg.cwd.clone(),
        global_config_path: migrations::global_config::global_config_path()
            .unwrap_or_else(|| PathBuf::from("/dev/null")),
    };

    // The orchestrator's output stream + the gate's request sink BOTH ride the
    // same connection-scoped outbound channel (the F2-06 contract).
    let event_sink = connection.event_sink();
    // Subagent lifecycle/message events use the same authenticated sink as the
    // main turn. `initialize_live_session` canonicalizes this id before this
    // function is entered, so the observer and engine transcript share one
    // session fence even when the host did not provide an id explicitly.
    cfg.session_agent_observer = Some(Arc::new(
        engine_desktop::session_agents::DesktopSessionAgentObserver::new(
            event_sink.clone(),
            cfg.session_id_override.clone().unwrap_or_default(),
        ),
    ));
    let message_output = client_adapter::AdapterOutputStream::new(event_sink.clone());
    let output: Arc<dyn OutputStream> = Arc::new(message_output.clone());
    let permission_sink = connection.permission_sink();

    // The `computer` tool's `request_access` approval — the Electron-facing
    // sibling of the permission round-trip above. A fresh channel: the SENDER
    // half fills `cfg.computer_access_tx`, which `engine_desktop::build` wires
    // into the GENERIC `tool_computer_use::TuiBridgeResolver` (the same
    // resolver the TUI host uses — see its own doc comment for why it is
    // transport-agnostic); the RECEIVER half is drained by a connection-scoped
    // `BridgeComputerAccessBroker`, which lowers each exchange into a
    // `Frame::ComputerAccessRequest` push and parks the reply channel keyed by
    // a fresh `request_id`, exactly mirroring the permission gate's shape.
    let (computer_access_tx, computer_access_rx) =
        tokio::sync::mpsc::channel::<tui_core::computer_access_bridge::ComputerAccessExchange>(8);
    cfg.computer_access_tx = Some(computer_access_tx);
    let computer_access_broker = Arc::new(client_adapter::BridgeComputerAccessBroker::new(
        connection.computer_access_sink(),
    ));
    // Device audio (microphone / recognizer / synthesizer). The desktop has no
    // native implementation — those devices belong to the Electron client — so
    // one `AudioBridge` over THIS connection's sink stands in for all three
    // traits: each engine-side call becomes a `ClientEvent::AudioRequest` parked
    // until the client's `ClientCommand::AudioResponse` comes back.
    //
    // Both halves are connection-scoped ON PURPOSE, and this is the only place
    // that can make that true: the bridge parks into a table the responder
    // resolves out of, and BOTH are created here, per `assemble`, per
    // `BridgeConnection`. A later connection gets a fresh pair, so its
    // `AudioResponse` cannot resolve a request this one parked (the ids are
    // per-bridge counters into per-bridge tables), and `on_close` drains what is
    // still parked instead of leaving a caller to wait out its deadline.
    //
    // There is deliberately NO capability handshake: this runs before any
    // client connects, so the desktop cannot know whether the renderer that
    // eventually attaches implements audio at all. That is answered honestly at
    // call time instead — `AudioBridge` reports "no desktop client is connected"
    // when nothing is listening, and a deadline when nothing answers.
    //
    // Any capability the caller put on `cfg` is REPLACED, not honored: it could
    // only have been built over some other connection, and the engine's audio
    // calls must reach THIS one.
    let (audio_bridge, audio_responder) = new_audio_bridge(connection.audio_sink());
    cfg.audio = Some(DesktopAudio::from_single(audio_bridge.clone()));
    let (ask_user_question_tx, ask_user_question_rx) = tokio::sync::mpsc::channel::<
        tui_core::ask_user_question_bridge::AskUserQuestionExchange,
    >(8);
    cfg.ask_user_question_tx = Some(ask_user_question_tx);
    let ask_user_question_broker = Arc::new(client_adapter::BridgeAskUserQuestionBroker::new(
        connection.event_sink(),
    ));
    let provider_model_catalog_listings = desktop_provider_catalog_listings(&cfg);

    let runtime = build(cfg, output, permission_sink)
        .await
        .map_err(|e| e.to_string())?;

    // Seeds the parent-supplied keys and refreshes Fusion's catalog filter for
    // them under ONE bounded budget -- see `seed_parent_supplied_provider_keys`
    // for why both halves live there (round 9 item 2; round-10 finding N3).
    seed_parent_supplied_provider_keys(&runtime.credentials, &provider_keys).await;

    // The parent-source fact is authoritative for packaged Electron sessions.
    // Unpackaged CLI/TUI-oriented hosts may still derive availability from
    // their own persistent storage before binding the fail-fast driver.
    let credential_required = needs_credential_driver(
        parent_credential_supplied,
        &runtime.provider_availability,
        // `provider-config` keeps Anthropic's API-key route live even on
        // a cold unauthenticated boot.  The composite credential provider
        // reads the process-local store per request, so after Settings
        // adds the first key this same connection can authenticate a live
        // turn; a permanently fail-fast driver would make the catalog
        // publication a false positive.
        runtime
            .provider_auth_methods
            .get("anthropic")
            .is_some_and(|method| method == "api_key"),
    );

    // `use_noop_permission_gate: false` ⇒ build MUST surface the adapter gate.
    let gate = runtime.permission_gate.clone().ok_or_else(|| {
        "engine_desktop::build did not surface an AdapterPermissionGate despite \
         use_noop_permission_gate=false"
            .to_string()
    })?;

    // §27 mid-turn drain + Now-abort wiring. The per-connection queue is shared
    // (Arc) across THREE consumers: (1) the orchestrator's mid-turn input source
    // (injects queued `Next`/`Now` prompts WITHIN a running turn), (2) the turn
    // driver (registers each turn's cancel token so a `Now` enqueue aborts it),
    // and (3) the existing between-turn `drain_main_thread` loop. A
    // `CancelReasonFlag` is shared between the orchestrator (reads it to tell a
    // `Now`-command abort from a user interrupt) and the queue's now-abort hook
    // (sets it to `QueueNowCommand` right before firing the active-turn token).
    let queue = connection.queue_handle();
    if let Some(inbox) = runtime
        .coordinator
        .mailbox_router
        .get(&runtime.coordinator.coordinator_id)
        .await
    {
        let weak_queue = Arc::downgrade(&queue);
        tokio::spawn(async move {
            while weak_queue.strong_count() > 0 {
                let Some(message) = inbox
                    .wait_for_message(std::time::Duration::from_millis(500))
                    .await
                else {
                    continue;
                };
                let Some(queue) = weak_queue.upgrade() else {
                    break;
                };
                queue
                    .enqueue(msgqueue::QueuedCommand {
                        uuid: message.message_id,
                        content: msgqueue::QueuedCommandContent::UserInput {
                            text: engine_desktop::teammate_message_envelope_with_summary(
                                &message.from_name,
                                &message.content,
                                message.summary.as_deref(),
                            ),
                        },
                        priority: msgqueue::QueuePriority::Next,
                        queued_at: message.timestamp,
                        source: msgqueue::QueueSource::AgentSendMessage,
                        agent_id: None,
                        skip_slash_commands: true,
                        is_meta: true,
                    })
                    .await;
            }
        });
    }
    let loop_runtime = connection.loop_runtime_handle();
    let cancel_reason = orchestrator::prompt::mid_turn_input::CancelReasonFlag::new();
    let mid_turn_input = crate::driver::MsgQueueMidTurnInput::new(queue.clone());
    // The `/loop` fold's `foreign_user_input` veto reads this counter at the
    // turn edge; the source bumps it whenever a human's prompt is folded into a
    // turn already in flight.
    let foreign_input_counter = mid_turn_input.foreign_input_counter();
    runtime
        .orchestrator
        .set_mid_turn_input(Arc::new(mid_turn_input));
    runtime
        .orchestrator
        .set_cancel_reason(cancel_reason.clone());
    {
        let reason = cancel_reason.clone();
        queue
            .set_now_abort_hook(Arc::new(move || {
                reason.set(orchestrator::prompt::mid_turn_input::CancelReason::QueueNowCommand);
            }))
            .await;
    }

    // Phase-2 /loop dynamic mode (ScheduleWakeup): fill the registered tool's
    // set-once wakeup cell now that the per-connection `queue` + a `RuntimeSpawner`
    // both exist. `MsgQueueWakeupScheduler` sleeps for the (clamped) delay, resolves
    // the `<<autonomous-loop-dynamic>>` sentinel, then enqueues the `/loop` input at
    // `Next` so the between-turn drain folds it back into the session. The cell was
    // threaded out of `engine_desktop::build` on `DesktopRuntime` precisely because
    // the tool is constructed before this seam. Setting it more than once is a no-op
    // (`OnceLock`); a fresh per-connection `assemble` builds a fresh runtime + cell.
    let wakeup_scheduler: Arc<dyn tool_cron::WakeupScheduler> = Arc::new(
        crate::driver::MsgQueueWakeupScheduler::with_loop_runtime(
            queue.clone(),
            runtime.runtime_spawner.clone(),
            loop_runtime,
        )
        // Announce each fire on the connection's UNSCOPED sink (binary
        // `onFireTask`), including the no-op fold's streak line after quiet
        // ticks. Unscoped because a wakeup fires between turns: the
        // turn-ownership filter on the regular sink drops a `SystemNotice` that
        // no active turn owns, which is every wakeup announcement.
        .with_event_sink(connection.loop_wakeup_event_sink()),
    );
    // The driver re-uses the SAME scheduler at its turn-completion edge to arm the
    // `/loop` keepalive fallback (binary `lKi`); clone before the cell consumes it.
    let driver_wakeup_scheduler = wakeup_scheduler.clone();
    if runtime.wakeup_scheduler_cell.set(wakeup_scheduler).is_err() {
        // Already filled — should not happen for a fresh runtime, but never panic
        // at the composition root over a benign double-wire.
    }

    // Keep the full runtime/router alive without credentials, but reject model
    // turns at the bridge boundary before provider retries begin. With any
    // configured source, use the production driver and its queue/cancel wiring.
    let driver: Arc<dyn TurnDriver> = if credential_required {
        Arc::new(CredentialRequiredTurnDriver::new(event_sink))
    } else {
        Arc::new(
            OrchestratorTurnDriver::with_error_sink(runtime.orchestrator.clone(), event_sink)
                .with_message_output(message_output)
                .with_queue(queue, cancel_reason)
                .with_wakeup_scheduler(driver_wakeup_scheduler)
                .with_foreign_input_counter(foreign_input_counter),
        )
    };

    // The full command-routing seam over the real engine handles.
    let handle: Arc<dyn OrchestratorHandle> = runtime.orchestrator.clone();
    let dispatcher: Arc<dyn SlashCommandDispatcher> =
        Arc::new(RegistrySlashDispatcherClone::wrap(&runtime));
    let router = Arc::new(
        EngineCommandRouter::new(
            handle,
            runtime.auth.clone(),
            runtime.task_registry.clone()
                as Arc<dyn platform_api::task_registry::TaskRegistryHandle>,
            Some(dispatcher),
            Some(runtime.shared_command_registry.clone()),
        )
        .with_credentials(runtime.credentials.clone())
        .with_provider_model_catalog_listings(provider_model_catalog_listings)
        .with_ephemeral_provider_credentials(provider_credentials_ephemeral)
        .with_http(runtime.http.clone())
        .with_session_store(session_store)
        .with_settings_context(settings_context)
        .with_mcp_paths(mcp_paths)
        .with_mcp_registry(runtime.mcp_registry.clone())
        .with_plugin_runtime(runtime.plugin_runtime.clone())
        .with_hook_registry(runtime.hook_registry.clone())
        .with_repo_root_reloader(runtime.repo_root_reloader.clone())
        .with_file_changed_watcher(runtime.file_changed_watcher.controller()),
    );

    let connection = connection
        .bind(gate, driver)
        .bind_router(router)
        .bind_computer_access(computer_access_broker, computer_access_rx)
        .bind_ask_user_question(ask_user_question_broker, ask_user_question_rx)
        // The response half of the SAME pair whose request half went into
        // `cfg.audio` above — this is what makes an inbound `AudioResponse` on
        // this connection resolve a call the engine parked on this connection,
        // and a disconnect drain them.
        .bind_audio(audio_responder);
    let connection = if credential_required {
        connection
    } else {
        connection.with_task_notification_registry(runtime.task_registry.clone())
    }
    .with_queue_wakeup();
    Ok(BoundServer {
        connection,
        runtime,
        audio: audio_bridge,
        live_session,
    })
}

/// The `RegistrySlashDispatcher` returned by `engine_desktop::build` is owned by
/// value on the runtime, but the router needs an `Arc<dyn SlashCommandDispatcher>`
/// it can hold for the connection's lifetime. We cannot move the dispatcher out
/// of the runtime (we keep the runtime alive), so this thin newtype clones the
/// dispatcher's shared registry handle into a fresh dispatcher pointing at the
/// SAME registry — dispatching is identical.
struct RegistrySlashDispatcherClone {
    inner: command_api::RegistrySlashDispatcher,
}

impl RegistrySlashDispatcherClone {
    fn wrap(runtime: &DesktopRuntime) -> Self {
        Self {
            inner: runtime
                .dispatcher
                .clone_shared()
                .with_injected_messages_as_turns(),
        }
    }
}

#[async_trait::async_trait]
impl SlashCommandDispatcher for RegistrySlashDispatcherClone {
    async fn dispatch(&self, raw: &str) -> platform_api::SlashDispatchResult {
        self.inner.dispatch(raw).await
    }
}

/// A running endpoint paired with the Drop-guard that reaps its discovery
/// lockfile.
///
/// Returned by [`publish_lockfile`]: holding the [`LockfileGuard`] keeps the
/// `~/.lingxi/bridge/<port>.lock` file on disk for the server's lifetime; it is
/// removed when this struct is dropped (clean shutdown OR panic unwind).
pub struct ServedEndpoint {
    /// The bound loopback endpoint serving the connection.
    pub endpoint: McpEndpoint,
    /// Absolute path of the discovery lockfile this endpoint published.
    pub lockfile_path: PathBuf,
    /// Drop-guard that removes [`Self::lockfile_path`] when this struct drops.
    /// Kept as a field (not `_`) so callers can [`LockfileGuard::disarm`] it in
    /// tests; in the binary it simply lives for the process lifetime.
    pub lock_guard: LockfileGuard,
}

/// Write the F2-04 discovery lockfile for `endpoint`'s port and enforce its
/// `authToken` on the endpoint — the env-free half of the binary's step (4),
/// factored into the library so the headless serve-path test drives the SAME
/// code (keeping `main.rs` thin).
///
/// `lockfile_dir` is the directory the `<port>.lock` file is written into: the
/// binary passes its `~/.lingxi/bridge` dir (via [`IdeLockfile::for_bridge`]);
/// the test passes a `tempfile::TempDir` so no real `$HOME` is touched. The
/// `workspace_folders` are recorded verbatim in the lockfile body.
///
/// The freshly-generated token is enforced on the endpoint (so the published
/// `authToken` is the one a connecting client MUST present) and is NEVER logged.
/// The returned [`ServedEndpoint`] owns a [`LockfileGuard`] that reaps the file
/// on drop.
pub fn publish_lockfile(
    endpoint: McpEndpoint,
    lockfile_dir: PathBuf,
    workspace_folders: Vec<PathBuf>,
) -> std::io::Result<ServedEndpoint> {
    let port = endpoint.port();
    let lockfile = IdeLockfile::new_for_bridge_dir(lockfile_dir, port, workspace_folders);
    let lockfile_path = lockfile.path();
    lockfile.write()?;
    // The endpoint MUST enforce the SAME token the lockfile published.
    endpoint.set_auth_token(lockfile.auth_token().to_string());
    let lock_guard = LockfileGuard::new(lockfile_path.clone());
    Ok(ServedEndpoint {
        endpoint,
        lockfile_path,
        lock_guard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::live_sessions::LiveSessionDir;

    #[test]
    fn parse_defaults_when_no_args() {
        let args = BridgeArgs::parse(Vec::<String>::new()).expect("empty parses");
        assert_eq!(args, BridgeArgs::default());
        assert!(!args.help);
    }

    #[test]
    fn parse_cwd_and_model() {
        let args =
            BridgeArgs::parse(["--cwd", "/tmp/p", "--model", "claude-x"]).expect("flags parse");
        assert_eq!(args.cwd, Some(PathBuf::from("/tmp/p")));
        assert_eq!(args.model.as_deref(), Some("claude-x"));
    }

    #[test]
    fn parse_session_id_and_list_mode() {
        let args = BridgeArgs::parse([
            "--session-id",
            "11111111-2222-3333-4444-555555555555",
            "--list-sessions-json",
        ])
        .expect("session catalog flags parse");
        assert_eq!(
            args.session_id.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert!(args.list_sessions_json);
        assert!(BridgeArgs::parse(["--session-id", "not-a-uuid"]).is_err());
    }

    #[tokio::test]
    async fn list_mode_is_empty_and_does_not_create_catalog_files() {
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = tempfile::tempdir().expect("config tempdir");
        let json = list_sessions_json_from(cwd.path(), home.path())
            .await
            .expect("empty catalog succeeds");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid envelope");
        assert_eq!(value, serde_json::json!({"version": 1, "sessions": []}));
        assert!(
            std::fs::read_dir(home.path())
                .expect("config home remains readable")
                .next()
                .is_none(),
            "catalog-only mode must not create engine/session files"
        );
    }

    #[tokio::test]
    async fn list_mode_marks_only_explicit_mobile_empty_anchors() {
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = tempfile::tempdir().expect("config tempdir");
        let historical_id = "11111111-2222-4333-8444-555555555555";
        let empty_id = "66666666-7777-4888-8999-aaaaaaaaaaaa";
        let historical_path =
            session::jsonl::session_path(home.path(), &cwd.path().to_string_lossy(), historical_id);
        std::fs::create_dir_all(historical_path.parent().expect("catalog dir"))
            .expect("create catalog dir");
        let system_line = serde_json::json!({
            "type": "system",
            "uuid": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "parentUuid": null,
            "sessionId": historical_id,
            "timestamp": "2026-08-26T00:00:00.000Z",
            "cwd": cwd.path().to_string_lossy(),
            "version": "0.12.0",
            "isSidechain": false,
            "message": {"role": "system", "content": "hook result"},
        });
        std::fs::write(&historical_path, format!("{system_line}\n")).expect("write historical row");
        let empty_path =
            session::jsonl::session_path(home.path(), &cwd.path().to_string_lossy(), empty_id);
        let empty_line = serde_json::json!({
            "type": "custom-title",
            "sessionId": empty_id,
            "customTitle": "New session",
            "mobileEmptySession": 1,
        });
        std::fs::write(&empty_path, format!("{empty_line}\n")).expect("write empty anchor");

        let json = list_sessions_json_from(cwd.path(), home.path())
            .await
            .expect("catalog succeeds");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid envelope");
        let sessions = value["sessions"].as_array().expect("session rows");
        let historical = sessions
            .iter()
            .find(|row| row["uuid"] == historical_id)
            .expect("historical row");
        let empty = sessions
            .iter()
            .find(|row| row["uuid"] == empty_id)
            .expect("empty row");

        assert_eq!(historical["message_count"], 0);
        assert_eq!(historical["empty_session"], false);
        assert_eq!(empty["message_count"], 0);
        assert_eq!(empty["empty_session"], true);
    }

    #[tokio::test]
    async fn list_mode_exposes_the_private_resume_model_hint() {
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = tempfile::tempdir().expect("config tempdir");
        let session_id = "77777777-8888-4999-8aaa-bbbbbbbbbbbb";
        let transcript =
            session::jsonl::session_path(home.path(), &cwd.path().to_string_lossy(), session_id);
        std::fs::create_dir_all(transcript.parent().expect("catalog dir"))
            .expect("create catalog dir");
        let user_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let assistant_id = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
        let sidechain_id = "cccccccc-dddd-4eee-8fff-aaaaaaaaaaaa";
        let user = serde_json::json!({
            "type": "user",
            "uuid": user_id,
            "parentUuid": null,
            "sessionId": session_id,
            "timestamp": "2026-09-03T00:00:00.000Z",
            "cwd": cwd.path().to_string_lossy(),
            "version": "0.12.0",
            "isSidechain": false,
            "message": {"role": "user", "content": "hello"},
        });
        let assistant = serde_json::json!({
            "type": "assistant",
            "uuid": assistant_id,
            "parentUuid": user_id,
            "sessionId": session_id,
            "timestamp": "2026-09-03T00:00:01.000Z",
            "cwd": cwd.path().to_string_lossy(),
            "version": "0.12.0",
            "isSidechain": false,
            "modelProfile": "openrouter",
            "message": {
                "role": "assistant",
                "model": "cohere/north-mini-code:free",
                "content": [{"type": "text", "text": "hello"}]
            },
        });
        let newer_sidechain = serde_json::json!({
            "type": "assistant",
            "uuid": sidechain_id,
            "parentUuid": user_id,
            "sessionId": session_id,
            "timestamp": "2026-09-03T00:00:02.000Z",
            "cwd": cwd.path().to_string_lossy(),
            "version": "0.12.0",
            "isSidechain": true,
            "modelProfile": "deepseek",
            "message": {
                "role": "assistant",
                "model": "deepseek-v4-flash",
                "content": [{"type": "text", "text": "off branch"}]
            },
        });
        std::fs::write(
            &transcript,
            format!("{user}\n{assistant}\n{newer_sidechain}\n"),
        )
        .expect("write transcript");

        let json = list_sessions_json_from(cwd.path(), home.path())
            .await
            .expect("catalog succeeds");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid envelope");
        let row = value["sessions"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["uuid"] == session_id))
            .expect("session row");
        assert_eq!(
            row["resume_model"],
            "openrouter/cohere/north-mini-code:free"
        );
    }

    #[test]
    fn live_registration_allows_resume_of_existing_transcript_and_cleans_up() {
        // `LiveSessionDir` and the UDS inbox are process globals in tests just
        // as they are in the CLI. Keep the entire guard lifetime serialized so
        // another test cannot stop this test's inbox or overwrite its globals.
        let _serial = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = tempfile::tempdir().expect("config tempdir");
        let session_id = "11111111-2222-4333-8444-555555555555";
        let transcript =
            session::jsonl::session_path(home.path(), &cwd.path().to_string_lossy(), session_id);
        std::fs::create_dir_all(transcript.parent().expect("transcript parent")).unwrap();
        std::fs::write(&transcript, "{}\n").unwrap();

        let mut cfg = resolve_desktop_config(&BridgeArgs::default());
        cfg.cwd = cwd.path().to_path_buf();
        cfg.lingxi_home = home.path().to_path_buf();
        cfg.session_id_override = Some(session_id.to_string());

        let guard = initialize_live_session(&mut cfg).expect("transcript is not a live writer");
        assert_eq!(guard.session_id, session_id);
        assert!(guard
            .dir
            .list_live()
            .unwrap()
            .iter()
            .any(|record| record.sid() == session_id));
        drop(guard);
        assert!(LiveSessionDir::at_live(home.path().join("sessions"))
            .list_live()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parse_help_flag() {
        assert!(BridgeArgs::parse(["--help"]).unwrap().help);
        assert!(BridgeArgs::parse(["-h"]).unwrap().help);
    }

    #[test]
    fn shared_cli_tui_credential_disables_fail_fast_driver() {
        let availability = BTreeMap::from([
            ("deepseek".to_string(), true),
            ("openrouter".to_string(), false),
        ]);

        assert!(!needs_credential_driver(false, &availability, false));
        assert!(needs_credential_driver(false, &BTreeMap::new(), false));
        assert!(!needs_credential_driver(true, &BTreeMap::new(), false));
        assert!(
            !needs_credential_driver(false, &BTreeMap::new(), true),
            "a route that resolves credentials at request time must remain live after a cold boot"
        );
    }

    #[test]
    fn parse_missing_value_is_error() {
        assert!(BridgeArgs::parse(["--cwd"]).is_err());
        assert!(BridgeArgs::parse(["--model"]).is_err());
        assert!(BridgeArgs::parse(["--bridge-dir"]).is_err());
    }

    #[test]
    fn parse_security_flags_and_absolute_bridge_dir() {
        let args = BridgeArgs::parse([
            "--api-key-stdin",
            "--credential-stdin",
            "--trusted-workspace",
            "--packaged-credential-stdin-only",
            "--bridge-dir",
            "/tmp/lingxi-bridge",
        ])
        .expect("security flags parse");
        assert!(args.api_key_stdin);
        assert!(args.credential_stdin);
        assert!(args.trusted_workspace);
        assert!(args.packaged_credential_stdin_only);
        assert_eq!(
            args.bridge_dir.as_deref(),
            Some(std::path::Path::new("/tmp/lingxi-bridge"))
        );
        assert!(BridgeArgs::parse(["--bridge-dir", "relative/path"]).is_err());
    }

    #[test]
    fn parse_unknown_flag_is_error() {
        let err = BridgeArgs::parse(["--frobnicate"]).unwrap_err();
        assert!(
            err.contains("--frobnicate")
                || err.contains("frobnicate")
                || err.contains("frobni")
                || err.contains("unknown"),
            "got: {err}"
        );
    }

    #[test]
    fn usage_mentions_stdin_but_never_a_value_or_key_env() {
        let u = usage();
        assert!(u.contains("--api-key-stdin"));
        assert!(!u.contains("ANTHROPIC_API_KEY"));
        assert!(!u.contains("sk-"), "usage must not embed a key literal");
    }

    #[test]
    fn stdin_key_is_one_line_bounded_and_errors_never_echo_it() {
        let mut input = std::io::Cursor::new(b"secret value\r\nignored\n".to_vec());
        assert_eq!(read_api_key_line(&mut input).unwrap(), "secret value");

        let secret = "z".repeat(MAX_STDIN_API_KEY_BYTES + 1);
        let mut oversized = std::io::Cursor::new(format!("{secret}\n").into_bytes());
        let error = read_api_key_line(&mut oversized).unwrap_err();
        assert!(error.contains("limit"));
        assert!(!error.contains(&secret));
    }

    #[test]
    fn credential_envelope_accepts_provider_keys_without_echoing_secrets() {
        let mut input = std::io::Cursor::new(
            br#"{"api_key":null,"provider_keys":{"openai":"sk-secret","deepseek":"ds-secret"},"plugin_secrets":{"weather@official":{"API_KEY":"plugin-secret"}}}"#
                .to_vec(),
        );
        let envelope = read_credential_envelope(&mut input).expect("envelope parses");
        assert_eq!(
            envelope.provider_keys.get("openai").map(String::as_str),
            Some("sk-secret")
        );
        assert_eq!(envelope.provider_keys.len(), 2);
        assert_eq!(
            envelope
                .plugin_secrets
                .get("weather@official")
                .and_then(|values| values.get("API_KEY"))
                .map(String::as_str),
            Some("plugin-secret")
        );

        let secret = "x".repeat(MAX_STDIN_CREDENTIAL_BYTES + 1);
        let mut oversized = std::io::Cursor::new(
            format!(r#"{{"provider_keys":{{"openai":"{secret}"}}}}"#).into_bytes(),
        );
        let error = read_credential_envelope(&mut oversized)
            .err()
            .expect("oversized envelope must fail");
        assert!(error.contains("limit"));
        assert!(!error.contains(&secret));
    }

    #[test]
    fn credential_envelope_rejects_javascript_camel_case_fields() {
        let mut input = std::io::Cursor::new(
            br#"{"apiKey":null,"providerKeys":{"deepseek":"ds-secret"}}"#.to_vec(),
        );
        let error = read_credential_envelope(&mut input).expect_err("protocol mismatch must fail");
        assert_eq!(error, "invalid credential envelope");
    }

    #[test]
    fn resolve_uses_model_override() {
        let args = BridgeArgs {
            model: Some("custom-model".to_string()),
            ..BridgeArgs::default()
        };
        let cfg = resolve_desktop_config(&args);
        assert_eq!(cfg.default_model, "custom-model");
        // A transport always binds the adapter gate.
        assert!(!cfg.use_noop_permission_gate);
    }

    #[test]
    fn workspace_is_untrusted_by_default() {
        let cfg = resolve_desktop_config(&BridgeArgs::default());
        assert!(cfg.api_key.is_empty(), "credentials must not come from env");
        assert!(cfg.api_key_helper.is_none());
        assert!(cfg.provider_profiles.is_none());
        assert!(cfg.routing.is_none());
        assert!(cfg.mcp_paths.is_empty());
        assert_eq!(cfg.setting_source_scope, (false, false));
        assert!(cfg.customization_gates.safe_mode);
        assert!(cfg.memory_provider.is_none());
        assert_eq!(cfg.permission_mode, permission::PermissionMode::Default);
        assert!(!cfg.allow_dangerously_skip_permissions);
    }

    #[test]
    fn trusted_workspace_preserves_customization_sources() {
        let cfg = resolve_desktop_config(&BridgeArgs {
            trusted_workspace: true,
            ..BridgeArgs::default()
        });
        assert_eq!(cfg.permission_mode, permission::PermissionMode::Auto);
        assert!(!cfg.permission_mode_cli_explicit);
        assert_eq!(cfg.setting_source_scope, (true, true));
        assert_eq!(
            cfg.customization_gates,
            engine_desktop::CustomizationGates::default()
        );
        assert_eq!(cfg.mcp_paths.len(), 2);
        assert!(cfg.memory_provider.is_some());
        assert!(
            cfg.allow_dangerously_skip_permissions,
            "trusted desktop sessions expose Full access only as an explicit live choice"
        );
    }

    #[test]
    fn packaged_provider_routes_survive_without_settings_credentials() {
        let profiles = BTreeMap::from([(
            "custom".to_string(),
            serde_json::json!({
                "type": "openai",
                "baseUrl": "http://127.0.0.1:12345/v1",
                "models": [{"id": "smoke-model"}],
                "apiKeyEnv": "CUSTOM_SECRET"
            }),
        )]);
        let filtered = without_settings_credentials(Some(profiles)).unwrap();
        let (parsed, warnings) = provider_config::parse_user_providers(&filtered);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].profile.profile_name, "custom");
        assert_eq!(parsed[0].profile.models[0].request_model, "smoke-model");
        assert_eq!(parsed[0].env_var, None);
        assert!(without_settings_credentials(None).is_none());
    }

    #[test]
    fn packaged_boundary_keeps_trusted_customizations_but_rejects_settings_credentials() {
        let cfg = resolve_desktop_config(&BridgeArgs {
            trusted_workspace: true,
            packaged_credential_stdin_only: true,
            ..BridgeArgs::default()
        });
        assert_eq!(cfg.permission_mode, permission::PermissionMode::Auto);
        assert!(!cfg.permission_mode_cli_explicit);
        assert_eq!(cfg.setting_source_scope, (true, true));
        assert_eq!(
            cfg.customization_gates,
            engine_desktop::CustomizationGates::default()
        );
        assert_eq!(cfg.mcp_paths.len(), 2);
        assert!(cfg.memory_provider.is_some());
        assert!(cfg.api_key_helper.is_none());
        assert!(cfg
            .provider_profiles
            .as_ref()
            .is_none_or(|profiles| profiles
                .values()
                .all(|profile| profile.get("apiKeyEnv").is_none())));
        assert_eq!(
            cfg.credential_storage_policy,
            CredentialStoragePolicy::NativeOrMemory,
            "packaged bridge credentials must remain process-local"
        );
    }

    #[test]
    fn has_no_credential_source_detects_empty() {
        let mut cfg = DesktopConfig {
            api_key: String::new(),
            provider_profiles: None,
            ..DesktopConfig::default()
        };
        assert!(has_no_credential_source(&cfg));

        cfg.api_key = "x".to_string();
        assert!(!has_no_credential_source(&cfg));

        cfg.api_key = String::new();
        cfg.api_key_helper = Some("printf sk-test".to_string());
        assert!(!has_no_credential_source(&cfg));

        cfg.api_key_helper = None;
        cfg.provider_profiles = Some(BTreeMap::new());
        assert!(
            has_no_credential_source(&cfg),
            "empty profiles map is still no source"
        );

        let mut profiles = BTreeMap::new();
        profiles.insert("p".to_string(), serde_json::json!({}));
        cfg.provider_profiles = Some(profiles);
        assert!(!has_no_credential_source(&cfg));
    }

    /// Assembly produces a fully-bound connection (gate + driver attached) from a
    /// deterministic config — no env reads, no live network. Proves the S2 wiring
    /// links end to end.
    #[tokio::test]
    async fn assemble_binds_connection() {
        let _loop_state = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().to_path_buf();
        let cfg = DesktopConfig {
            initial_teammate_team_name: None,
            // This unit test must not require the signed macOS Credential
            // Broker or inherit a developer login keychain.
            isolated_credential_storage: true,
            credential_storage_policy: CredentialStoragePolicy::NativePreferred,
            api_base: DEFAULT_API_BASE.to_string(),
            api_key: String::new(),
            api_key_helper: None,
            // (M13) Inert auth-resolver inputs: no managed OAuth forcing, no
            // FD-inherited key.
            managed_oauth_only: false,
            anthropic_key_fd_present: false,
            cwd: cwd.clone(),
            lingxi_home: cwd.join(".lingxi"),
            default_model: "claude-sonnet-4-20250514".to_string(),
            // Deterministic across host machines: a dev keychain with real
            // provider keys must not trigger the connected-provider fallback.
            default_model_explicit: true,
            recent_models: Vec::new(),
            fallback_model: None,
            custom_betas: Vec::new(),
            flag_settings: None,
            provider_profiles: None,
            routing: None,
            mcp_paths: vec![cwd.join(".mcp.json")],
            use_noop_permission_gate: false,
            deny_unresolved_ask: false,
            is_tty: false,
            injected_permission_gate: None,
            injected_plugin_secrets: BTreeMap::new(),
            ask_user_question_tx: None,
            computer_access_tx: None,
            session_agent_observer: None,
            session_started_as_coordinator: false,
            // Deterministic test: empty memory, never the real FS.
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
            permission_mode_cli: None,
            permission_mode_cli_explicit: false,
            allow_dangerously_skip_permissions: false,
            connect_prompt: None,
            max_turns: None,
            plan_mode_instructions: None,
            plans_directory: None,
            max_budget_usd: None,
            json_schema: None,
            system_prompt_override: None,
            append_system_prompt: None,
            session_id_override: None,
            parent_session_id: None,
            disable_slash_commands: false,
            add_dir: Vec::new(),
            cli_mcp_servers: Vec::new(),
            strict_mcp_config: false,
            restricted: false,
            restricted_tools: None,
            exclude_dynamic_system_prompt_sections: false,
            setting_source_scope: (true, true),
            customization_gates: engine_desktop::CustomizationGates::default(),
            session_persistence: true,
            cli_agents_json: None,
            cli_agent: None,
            cli_plugin_dirs: Vec::new(),
            initial_effort: None,
            default_model_env_pinned: false,
            session_thinking: Default::default(),
            bg_session_forker: None,
            worktree_launch: None,
            tmux_launch: None,
            // `assemble` fills this with the connection's own bridge; a
            // caller-supplied value would be a lie about which connection the
            // engine's audio calls reach.
            audio: None,
        };
        let bound = assemble(cfg).await.expect("assemble must succeed");
        // The gate handle is reachable only when bind() ran with a real gate.
        let gate = bound.connection.gate_handle();
        assert_eq!(
            gate.pending_count().await,
            0,
            "fresh gate has no parked requests"
        );
        // Phase-2 /loop wiring: assemble fills the ScheduleWakeup cell with the
        // msgqueue-backed scheduler (so the tool is no longer a no-op on the bridge).
        assert!(
            bound.runtime.wakeup_scheduler_cell.get().is_some(),
            "assemble must wire the ScheduleWakeup self-wakeup scheduler"
        );
        assert!(
            Arc::ptr_eq(
                &bound.runtime.shared_command_registry,
                &bound.runtime.dispatcher.registry()
            ),
            "assemble must retain the live slash-command registry on DesktopRuntime"
        );
    }
}

/// Test-only support shared by the two round-10 finding N3 budget tests (one
/// here, one in `router.rs`), because both drive the SAME process-wide
/// `engine_desktop::FUSION_CATALOG_REFRESHERS` registry.
#[cfg(test)]
pub(crate) mod fusion_refresh_test_support {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Serializes every test in this crate that registers into — or fans out
    /// over — the PROCESS-WIDE Fusion catalog refresher registry.
    ///
    /// `engine_desktop::refresh_fusion_catalog_after_credential_write` walks
    /// that global `Vec` SERIALLY, so a test that deliberately registers a
    /// refresher over a never-answering credential backend would otherwise park
    /// a concurrently-running test's refresh behind its own stall (and, with
    /// the budget in place, make that test's refresh silently miss its
    /// deadline). Every test that touches the registry takes this lock.
    pub(crate) static REGISTRY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A credential backend whose READS never answer — the contended macOS
    /// keychain broker whose 5s stall the boot availability probe already wraps
    /// in `tokio::time::timeout` (`apps/engine-desktop/src/lib.rs`,
    /// `resolve_llm_stack`).
    ///
    /// Writes succeed, so seeding an ephemeral key never blocks; only the
    /// availability RE-PROBE a credential write triggers does, which is exactly
    /// the shape of the production hazard.
    pub(crate) struct StallingSecureStorage {
        reads_started: Arc<AtomicUsize>,
    }

    impl StallingSecureStorage {
        /// Returns the storage plus the counter of reads it has swallowed, so a
        /// test can prove the stall was actually REACHED rather than passing
        /// because the probe never ran.
        pub(crate) fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
            let reads_started = Arc::new(AtomicUsize::new(0));
            (
                Arc::new(Self {
                    reads_started: reads_started.clone(),
                }),
                reads_started,
            )
        }
    }

    #[async_trait::async_trait]
    impl platform_api::SecureStorage for StallingSecureStorage {
        async fn store(
            &self,
            _service: &str,
            _account: &str,
            _data: protocol::SecureStorageData,
        ) -> Result<(), platform_api::SecureStorageError> {
            Ok(())
        }

        async fn retrieve(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<Option<protocol::SecureStorageData>, platform_api::SecureStorageError> {
            self.reads_started.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }

        async fn delete(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<(), platform_api::SecureStorageError> {
            Ok(())
        }

        async fn list(
            &self,
            _service: &str,
        ) -> Result<Vec<String>, platform_api::SecureStorageError> {
            Ok(Vec::new())
        }

        fn is_encrypted(&self) -> bool {
            true
        }

        fn backend(&self) -> platform_api::SecureStorageBackend {
            platform_api::SecureStorageBackend::MacOsKeychain
        }
    }

    /// Build a `CredentialManager` over [`StallingSecureStorage`] and publish a
    /// `FusionCatalogRefresher` for `profiles` into the process-wide registry —
    /// the registered refresher a credential write then fans out to.
    pub(crate) fn register_stalling_refresher(
        profiles: &[&str],
    ) -> (
        Arc<secret::CredentialManager>,
        Arc<std::sync::RwLock<std::collections::BTreeMap<String, bool>>>,
        Arc<AtomicUsize>,
    ) {
        let (storage, reads) = StallingSecureStorage::new();
        let credentials = Arc::new(secret::CredentialManager::new(
            storage,
            Arc::new(platform_posix::PosixClock::new()),
            Arc::new(platform_posix::PosixHttp::new()),
        ));
        let availability = Arc::new(std::sync::RwLock::new(
            profiles
                .iter()
                .map(|profile| ((*profile).to_string(), false))
                .collect::<std::collections::BTreeMap<String, bool>>(),
        ));
        engine_desktop::register_fusion_catalog_refresher(
            engine_desktop::FusionCatalogRefresher::for_keychain_profiles(
                availability.clone(),
                credentials.clone(),
                profiles,
            ),
        );
        (credentials, availability, reads)
    }

    /// Give a detached refresh a bounded number of scheduler turns to reach
    /// the injected backend. The production seam intentionally does not yield
    /// on behalf of this observation.
    pub(crate) async fn wait_for_refresh_start(reads: &AtomicUsize) {
        for _ in 0..128 {
            if reads.load(Ordering::SeqCst) > 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    }
}

/// Round-10 finding N3: the Fusion catalog refresh that follows a credential
/// write must never put an UNBOUNDED keychain stall on connection assembly.
#[cfg(test)]
mod fusion_catalog_refresh_budget_tests {
    use super::fusion_refresh_test_support::{
        register_stalling_refresher, wait_for_refresh_start, REGISTRY_LOCK,
    };
    use super::{seed_parent_supplied_provider_keys, FUSION_CATALOG_REFRESH_BUDGET};
    use std::collections::BTreeMap;
    use std::sync::atomic::Ordering;

    /// The packaged Electron host hands N provider keys over the credential
    /// stdin boundary; each one triggers a re-probe of EVERY credential source
    /// through the (possibly brokered) keychain. Awaiting those inline made
    /// `assemble_with_provider_keys` — and therefore the whole engine startup —
    /// hang for as long as the broker did, N times over, with no timeout
    /// anywhere on the path.
    ///
    /// Virtual time (`start_paused`): the assertion is on the DEADLINE, so the
    /// clock only has to advance to it.
    #[tokio::test(start_paused = true)]
    async fn parent_supplied_keys_never_block_assembly_on_a_stalled_keychain() {
        let _registry = REGISTRY_LOCK.lock().await;
        // The refresher covers four profiles; the parent supplies keys for
        // three. The fourth has no ephemeral key, so its probe reaches the
        // never-answering backend — exactly like a real install where the
        // parent-supplied providers are a subset of the configured ones.
        let (credentials, _availability, reads) =
            register_stalling_refresher(&["openrouter", "deepseek", "groq", "never-answers"]);

        let provider_keys: BTreeMap<String, String> = [
            ("openrouter", "sk-or-n3"),
            ("deepseek", "sk-ds-n3"),
            ("groq", "sk-gq-n3"),
        ]
        .into_iter()
        .map(|(id, key)| (id.to_string(), key.to_string()))
        .collect();

        let began = tokio::time::Instant::now();
        let seeded = tokio::time::timeout(
            FUSION_CATALOG_REFRESH_BUDGET * 3,
            seed_parent_supplied_provider_keys(&credentials, &provider_keys),
        )
        .await;
        assert!(
            seeded.is_ok(),
            "seeding the parent-supplied provider keys must return even when the \
credential backend never answers: a stalled keychain here blocks \
`assemble_with_provider_keys`, so no client is served at all"
        );
        let waited = began.elapsed();
        assert!(
            waited < FUSION_CATALOG_REFRESH_BUDGET * 2,
            "the {} parent-supplied keys must share ONE {FUSION_CATALOG_REFRESH_BUDGET:?} \
budget, not one each; waited {waited:?}",
            provider_keys.len()
        );
        wait_for_refresh_start(&reads).await;
        assert!(
            reads.load(Ordering::SeqCst) >= 1,
            "the refresh must actually have reached the credential backend — with \
zero reads this test would pass without exercising the stall at all"
        );
        for provider_id in provider_keys.keys() {
            assert!(
                credentials
                    .get_provider_key_ephemeral(provider_id)
                    .await
                    .is_some(),
                "bounding the refresh must not skip the SEEDING it follows: \
{provider_id} is missing from the process-local credential cache"
            );
        }
    }
}
